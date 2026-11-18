// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! The `/v1` HTTP surface (spec §5) on hyper 1.x, HTTP/1.1.
//!
//! Order of checks on every request: route + method -> proto header ->
//! size limit (Content-Length before reading, `Limited` while reading) ->
//! auth -> body parse. Store calls run on the blocking pool.

use std::convert::Infallible;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use bytes::Bytes;
use http::{header, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::Serialize;
use sync_crypto::{CryptoProvider, SigAlg};
use sync_proto::api::{
    destroy_payload, DestroyChallenge, DestroyConfirm, DestroyReceipt, DestroyRequest, EnrollRequest,
    EnrollResponse, ErrorBody, ErrorCode, Health, ListQuery, ListResponse, MissingRequest,
    MissingResponse, ObjKind, ReceiptBody, DESTROY_CHALLENGE_TTL, DESTROY_TS_SKEW, MAX_LIST_LIMIT,
    PROTO_HEADER, PROTO_VERSIONS,
};
use sync_proto::container::Header;
use sync_proto::Id16;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

use crate::store::{PutOutcome, Scope, StoreError, ENV_NS};
use crate::{decode_token, now_unix, App};

/// Concurrent connections served; further accepts wait.
pub const MAX_CONNECTIONS: usize = 1024;
/// Header read timeout (slow-loris bound).
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(20);
/// How often expired rows are purged.
pub const GC_INTERVAL: Duration = Duration::from_secs(60);

type Resp = Response<Full<Bytes>>;

/// Accept loop on a plain TCP listener. Also spawns the GC ticker. Returns
/// only on an accept error.
pub async fn serve(app: Arc<App>, listener: TcpListener) -> std::io::Result<()> {
    spawn_gc(app.clone());
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let permit = slots.clone().acquire_owned().await.expect("semaphore never closed");
        let (stream, peer) = listener.accept().await?;
        let app = app.clone();
        tokio::spawn(async move {
            serve_conn(app, peer.ip(), stream).await;
            drop(permit);
        });
    }
}

/// Serve one HTTP/1.1 connection on any byte stream (plain TCP or TLS).
pub async fn serve_conn<S>(app: Arc<App>, ip: IpAddr, io: S)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let svc = service_fn(move |req| {
        let app = app.clone();
        async move { Ok::<_, Infallible>(handle(app, ip, req).await) }
    });
    let _ = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .max_buf_size(64 * 1024)
        .serve_connection(TokioIo::new(io), svc)
        .await;
}

pub(crate) fn spawn_gc(app: Arc<App>) {
    tokio::spawn(async move {
        let mut t = tokio::time::interval(GC_INTERVAL);
        loop {
            t.tick().await;
            let a = app.clone();
            let _ = tokio::task::spawn_blocking(move || a.store.gc(now_unix())).await;
        }
    });
}

// ---- responses ---------------------------------------------------------------

fn json<T: Serialize>(status: StatusCode, v: &T) -> Resp {
    let body = serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec());
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Full::new(Bytes::from(body)))
        .expect("static response parts")
}

fn err(code: ErrorCode, detail: &str) -> Resp {
    let status = StatusCode::from_u16(code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    json(
        status,
        &ErrorBody {
            error: code,
            detail: detail.to_string(),
        },
    )
}

fn store_err(e: StoreError) -> Resp {
    err(e.code, e.detail)
}

fn empty(status: StatusCode) -> Resp {
    Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Full::new(Bytes::new()))
        .expect("static response parts")
}

async fn blocking<T, F>(app: &Arc<App>, f: F) -> Result<T, Resp>
where
    T: Send + 'static,
    F: FnOnce(&App) -> Result<T, StoreError> + Send + 'static,
{
    let a = app.clone();
    match tokio::task::spawn_blocking(move || f(&a)).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(store_err(e)),
        Err(_) => Err(err(ErrorCode::Internal, "worker failed")),
    }
}

// ---- request helpers ---------------------------------------------------------

/// Read the body with a hard cap; 413 past it (Content-Length checked first).
async fn read_body(req: Request<Incoming>, limit: u64) -> Result<Bytes, Resp> {
    if let Some(len) = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
    {
        if len > limit {
            return Err(err(ErrorCode::TooLarge, "body over the size limit"));
        }
    }
    match Limited::new(req.into_body(), limit as usize).collect().await {
        Ok(b) => Ok(b.to_bytes()),
        Err(e) if e.is::<http_body_util::LengthLimitError>() => {
            Err(err(ErrorCode::TooLarge, "body over the size limit"))
        }
        Err(_) => Err(err(ErrorCode::BadRequest, "body read failed")),
    }
}

fn bearer(req: &Request<Incoming>) -> Option<[u8; 32]> {
    let v = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    decode_token(v.strip_prefix("Bearer ")?)
}

/// Require a token on `ns` with at least `need`. Unknown namespace and wrong
/// token answer the same 401 (no namespace oracle).
async fn authorize(app: &Arc<App>, token: Option<[u8; 32]>, ns: Id16, need: Scope) -> Result<(), Resp> {
    let Some(raw) = token else {
        return Err(err(ErrorCode::Unauthorized, "missing or malformed bearer token"));
    };
    let hash = app.token_hash(&raw);
    match blocking(app, move |a| a.store.token_scope(&ns, &hash)).await? {
        Some(s) if s >= need => Ok(()),
        Some(_) => Err(err(ErrorCode::Forbidden, "token scope is read-only")),
        None => Err(err(ErrorCode::Unauthorized, "invalid token")),
    }
}

fn anon(app: &App, ip: IpAddr) -> Result<(), Resp> {
    if app.allow_anon(ip) {
        Ok(())
    } else {
        Err(err(ErrorCode::RateLimited, "too many requests"))
    }
}

fn query_param<'a>(q: Option<&'a str>, key: &str) -> Option<&'a str> {
    q?.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == key).then_some(v)
    })
}

fn parse_id(s: &str) -> Result<Id16, Resp> {
    Id16::from_hex(s).map_err(|_| err(ErrorCode::BadRequest, "id must be 32 lowercase hex digits"))
}

/// `Range: bytes=a-b | a- | -n` against `size` -> inclusive (start, end).
/// `Ok(None)` = no Range header; `Err` = unsatisfiable / malformed.
fn parse_range(req: &Request<Incoming>, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(v) = req.headers().get(header::RANGE) else { return Ok(None) };
    let v = v.to_str().map_err(|_| ())?;
    let spec = v.strip_prefix("bytes=").ok_or(())?;
    if spec.contains(',') || size == 0 {
        return Err(());
    }
    let (a, b) = spec.split_once('-').ok_or(())?;
    let (start, end) = match (a.is_empty(), b.is_empty()) {
        (false, false) => (a.parse::<u64>().map_err(|_| ())?, b.parse::<u64>().map_err(|_| ())?),
        (false, true) => (a.parse::<u64>().map_err(|_| ())?, size - 1),
        (true, false) => {
            let n = b.parse::<u64>().map_err(|_| ())?;
            if n == 0 {
                return Err(());
            }
            (size.saturating_sub(n), size - 1)
        }
        (true, true) => return Err(()),
    };
    if start > end || start >= size {
        return Err(());
    }
    Ok(Some((start, end.min(size - 1))))
}

// ---- router ------------------------------------------------------------------

pub async fn handle(app: Arc<App>, ip: IpAddr, req: Request<Incoming>) -> Resp {
    if let Some(v) = req.headers().get(PROTO_HEADER) {
        let ok = v
            .to_str()
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .is_some_and(|p| PROTO_VERSIONS.contains(&p));
        if !ok {
            return err(ErrorCode::Unsupported, "unsupported protocol version");
        }
    }
    let path = req.uri().path().to_string();
    let seg: Vec<&str> = path.split('/').skip(1).collect();
    let method = req.method().clone();
    let r = match (method, seg.as_slice()) {
        (Method::GET, ["v1", "health"]) => Ok(json(
            StatusCode::OK,
            &Health {
                ok: true,
                proto: PROTO_VERSIONS.to_vec(),
            },
        )),
        (Method::POST, ["v1", "enroll"]) => enroll(&app, ip, req).await,
        (Method::POST, ["v1", "missing", ns]) => match parse_id(ns) {
            Ok(ns) => missing(&app, ns, req).await,
            Err(e) => Err(e),
        },
        (Method::GET, ["v1", "list", ns]) => match parse_id(ns) {
            Ok(ns) => list(&app, ns, req).await,
            Err(e) => Err(e),
        },
        (Method::POST, ["v1", "destroy", ns, step @ ("request" | "confirm")]) => match parse_id(ns) {
            Ok(ns) if *step == "request" => destroy_request(&app, ip, ns, req).await,
            Ok(ns) => destroy_confirm(&app, ip, ns, req).await,
            Err(e) => Err(e),
        },
        (m @ (Method::PUT | Method::GET | Method::DELETE), ["v1", kind, ns, id]) => {
            match (ObjKind::from_path_segment(kind), parse_id(ns), parse_id(id)) {
                (Some(kind), Ok(ns), Ok(id)) => object(&app, ip, m, kind, ns, id, req).await,
                (None, _, _) => Err(err(ErrorCode::NotFound, "no such route")),
                (_, Err(e), _) | (_, _, Err(e)) => Err(e),
            }
        }
        (_, ["v1", ..]) => Err(err(ErrorCode::NotFound, "no such route")),
        _ => Err(err(ErrorCode::NotFound, "no such route")),
    };
    r.unwrap_or_else(|e| e)
}

async fn enroll(app: &Arc<App>, ip: IpAddr, req: Request<Incoming>) -> Result<Resp, Resp> {
    anon(app, ip)?;
    let body = read_body(req, app.cfg.max_control as u64).await?;
    let r: EnrollRequest =
        serde_json::from_slice(&body).map_err(|_| err(ErrorCode::BadRequest, "bad enroll body"))?;
    // FIPS build: the module's approved signature is ECDSA P-256 (cmvp.md §7).
    if r.key_alg != sync_crypto::SigAlg::EcdsaP256Sha256 as u8 {
        return Err(err(ErrorCode::Unsupported, "key_alg must be 2 (P-256)"));
    }
    let pubk = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, &r.pub_destroy)
        .map_err(|_| err(ErrorCode::BadRequest, "pub_destroy is not base64url"))?;
    if pubk.len() != 65 || pubk[0] != 0x04 {
        return Err(err(ErrorCode::BadRequest, "pub_destroy must be an uncompressed P-256 point"));
    }
    let tok = decode_token(&r.enrollment_token)
        .ok_or_else(|| err(ErrorCode::Unauthorized, "malformed enrollment token"))?;
    let enroll_hash = app.token_hash(&tok);
    let mut ns = [0u8; 16];
    app.prov.random(&mut ns).map_err(|_| err(ErrorCode::Internal, "drbg"))?;
    let ns = Id16(ns);
    let (read_token, rh) = app.new_token().map_err(|_| err(ErrorCode::Internal, "drbg"))?;
    let (write_token, wh) = app.new_token().map_err(|_| err(ErrorCode::Internal, "drbg"))?;
    let alg = r.key_alg;
    blocking(app, move |a| a.store.enroll(&enroll_hash, now_unix(), &ns, &rh, &wh, &pubk, alg)).await?;
    Ok(json(
        StatusCode::CREATED,
        &EnrollResponse {
            ns,
            read_token,
            write_token,
            server_key: app.receipt_pub.clone(),
        },
    ))
}

async fn missing(app: &Arc<App>, ns: Id16, req: Request<Incoming>) -> Result<Resp, Resp> {
    authorize(app, bearer(&req), ns, Scope::Read).await?;
    let body = read_body(req, app.cfg.max_control as u64).await?;
    let r: MissingRequest =
        serde_json::from_slice(&body).map_err(|_| err(ErrorCode::BadRequest, "bad missing body"))?;
    r.validate().map_err(|c| err(c, "too many ids"))?;
    let ids = r.ids;
    let missing = blocking(app, move |a| a.store.missing(&ns, &ids)).await?;
    Ok(json(StatusCode::OK, &MissingResponse { missing }))
}

async fn list(app: &Arc<App>, ns: Id16, req: Request<Incoming>) -> Result<Resp, Resp> {
    authorize(app, bearer(&req), ns, Scope::Read).await?;
    let q = req.uri().query();
    let num = |k: &str, d: u64| -> Result<u64, Resp> {
        match query_param(q, k) {
            None => Ok(d),
            Some(v) => v.parse::<u64>().map_err(|_| err(ErrorCode::BadRequest, "bad query")),
        }
    };
    let after = num("after", 0)?;
    let wait = u32::try_from(num("wait", 0)?).map_err(|_| err(ErrorCode::BadRequest, "bad wait"))?;
    let limit = u32::try_from(num("limit", MAX_LIST_LIMIT as u64)?)
        .map_err(|_| err(ErrorCode::BadRequest, "bad limit"))?;
    let lq = ListQuery { after, wait, limit };
    lq.validate().map_err(|c| err(c, "wait <= 60, 1 <= limit <= 1000"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait as u64);
    loop {
        // Register interest BEFORE querying so a PUT between query and wait
        // is not missed.
        let notified = app.changed.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let items = blocking(app, move |a| a.store.list(&ns, after, limit)).await?;
        if !items.is_empty() || tokio::time::Instant::now() >= deadline {
            let next = items.last().map(|i| i.seq).unwrap_or(after);
            return Ok(json(StatusCode::OK, &ListResponse { items, next }));
        }
        if tokio::time::timeout_at(deadline, notified).await.is_err() {
            return Ok(json(StatusCode::OK, &ListResponse { items, next: after }));
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn object(
    app: &Arc<App>,
    ip: IpAddr,
    m: Method,
    kind: ObjKind,
    ns: Id16,
    id: Id16,
    req: Request<Incoming>,
) -> Result<Resp, Resp> {
    {
        // Auth model per kind (spec §5): env is addressed by the secret `loc`
        // alone (fixed all-zero ns, no token, rate-limited per IP, never
        // deleted by clients); pair objects are written with the vault's write
        // token and fetched/removed by holders of the QR (id knowledge).
        match (kind, &m) {
            (ObjKind::Env, _) if ns != ENV_NS => {
                return Err(err(ErrorCode::BadRequest, "env objects live in the all-zero namespace"))
            }
            (ObjKind::Env, &Method::DELETE) => return Err(err(ErrorCode::Forbidden, "env objects are not deletable")),
            (ObjKind::Env, _) => anon(app, ip)?,
            (ObjKind::Pair, &Method::PUT) => authorize(app, bearer(&req), ns, Scope::Write).await?,
            (ObjKind::Pair, _) => anon(app, ip)?,
            (_, &Method::GET) => authorize(app, bearer(&req), ns, Scope::Read).await?,
            _ => authorize(app, bearer(&req), ns, Scope::Write).await?,
        }
        match m {
            Method::PUT => put(app, kind, ns, id, req).await,
            Method::GET => get(app, kind, ns, id, req).await,
            _ => delete(app, kind, ns, id).await,
        }
    }
}

async fn put(app: &Arc<App>, kind: ObjKind, ns: Id16, id: Id16, req: Request<Incoming>) -> Result<Resp, Resp> {
    let body = read_body(req, app.cfg.max_object).await?;
    let (h, _) = Header::parse(&body).map_err(|_| err(ErrorCode::BadRequest, "bad container header"))?;
    if h.obj_id != id {
        return Err(err(ErrorCode::BadRequest, "URL id differs from header obj_id"));
    }
    let sha = app.prov.sha256(&body);
    let expires = (kind == ObjKind::Pair).then(|| now_unix() + app.cfg.pair_ttl_secs);
    let out = blocking(app, move |a| a.store.put(&ns, kind, &id, &body, &sha, expires)).await?;
    Ok(match out {
        PutOutcome::Created => {
            app.changed.notify_waiters();
            empty(StatusCode::CREATED)
        }
        PutOutcome::Identical => empty(StatusCode::OK),
    })
}

async fn get(app: &Arc<App>, kind: ObjKind, ns: Id16, id: Id16, req: Request<Incoming>) -> Result<Resp, Resp> {
    let meta = blocking(app, move |a| a.store.meta(&ns, kind, &id))
        .await?
        .ok_or_else(|| err(ErrorCode::NotFound, "no such object"))?;
    let want_deleted = query_param(req.uri().query(), "deleted") == Some("1");
    if meta.deleted.is_some() && (!want_deleted || kind == ObjKind::Pair) {
        return Err(err(ErrorCode::Gone, "object deleted"));
    }
    if kind == ObjKind::Pair && meta.expires.is_some_and(|e| e <= now_unix()) {
        return Err(err(ErrorCode::Gone, "pairing object expired"));
    }
    let size = meta.size;
    let range = parse_range(&req, size).map_err(|_| {
        let mut r = err(ErrorCode::BadRequest, "unsatisfiable range");
        *r.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
        if let Ok(v) = format!("bytes */{size}").parse() {
            r.headers_mut().insert(header::CONTENT_RANGE, v);
        }
        r
    })?;
    let (start, end) = range.unwrap_or((0, size.saturating_sub(1)));
    let len = if size == 0 { 0 } else { end - start + 1 };
    let bytes = blocking(app, move |a| a.store.read(&ns, kind, &id, start, len)).await?;
    let mut b = Response::builder()
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store");
    b = if range.is_some() {
        b.status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"))
    } else {
        b.status(StatusCode::OK)
    };
    b.body(Full::new(Bytes::from(bytes)))
        .map_err(|_| err(ErrorCode::Internal, "response"))
}

async fn delete(app: &Arc<App>, kind: ObjKind, ns: Id16, id: Id16) -> Result<Resp, Resp> {
    let now = now_unix();
    // Pairing objects are single-use: the receiver's DELETE purges at once.
    let purge_at = if kind == ObjKind::Pair { now } else { now + app.cfg.retention_secs };
    let hit = blocking(app, move |a| {
        let hit = a.store.soft_delete(&ns, kind, &id, now, purge_at)?;
        if hit && kind == ObjKind::Pair {
            a.store.gc(now)?;
        }
        Ok(hit)
    })
    .await?;
    if hit {
        Ok(empty(StatusCode::ACCEPTED))
    } else {
        Err(err(ErrorCode::NotFound, "no live object"))
    }
}

/// `POST /v1/destroy/{ns}/request` — step 1. Tokenless (the proof in step 2
/// is the authority, so a device that only has the words can destroy), per-IP
/// rate-limited, at most [`crate::store::MAX_DESTROY_CHALLENGES`] open per vault.
async fn destroy_request(app: &Arc<App>, ip: IpAddr, ns: Id16, req: Request<Incoming>) -> Result<Resp, Resp> {
    anon(app, ip)?;
    let body = read_body(req, app.cfg.max_control as u64).await?;
    let _r: DestroyRequest =
        serde_json::from_slice(&body).map_err(|_| err(ErrorCode::BadRequest, "bad destroy request body"))?;
    let mut nonce = [0u8; 32];
    app.prov.random(&mut nonce).map_err(|_| err(ErrorCode::Internal, "drbg"))?;
    let (confirm_token, hash) = app.new_token().map_err(|_| err(ErrorCode::Internal, "drbg"))?;
    let now = now_unix();
    let expires = now + DESTROY_CHALLENGE_TTL as i64;
    blocking(app, move |a| a.store.add_destroy_challenge(&ns, &hash, &nonce, now, expires)).await?;
    Ok(json(
        StatusCode::OK,
        &DestroyChallenge {
            server_nonce: sync_proto::hex_lower(&nonce),
            confirm_token,
            expires: expires as u64,
        },
    ))
}

/// `POST /v1/destroy/{ns}/confirm` — step 2. The challenge is consumed
/// first (single use, even on a bad proof), then `ts` must be within
/// [`DESTROY_TS_SKEW`] of server time and `sig` must verify under the
/// `pub_destroy` stored at enrollment over [`destroy_payload`]. Only then is
/// the vault purged and a receipt signed. No proof -> no destroy.
async fn destroy_confirm(app: &Arc<App>, ip: IpAddr, ns: Id16, req: Request<Incoming>) -> Result<Resp, Resp> {
    anon(app, ip)?;
    let body = read_body(req, app.cfg.max_control as u64).await?;
    let r: DestroyConfirm =
        serde_json::from_slice(&body).map_err(|_| err(ErrorCode::BadRequest, "bad destroy confirm body"))?;
    let tok = decode_token(&r.confirm_token).ok_or_else(|| err(ErrorCode::Unauthorized, "malformed confirm token"))?;
    let hash = app.token_hash(&tok);
    let now = now_unix();
    let (nonce, pubk) = blocking(app, move |a| a.store.take_destroy_challenge(&ns, &hash, now)).await?;
    if (r.ts - now).abs() > DESTROY_TS_SKEW {
        return Err(err(ErrorCode::BadRequest, "ts outside the accepted window"));
    }
    let sig = URL_SAFE_NO_PAD
        .decode(r.sig.as_bytes())
        .map_err(|_| err(ErrorCode::Forbidden, "destroy proof invalid"))?;
    app.prov
        .verify(SigAlg::EcdsaP256Sha256, &pubk, &destroy_payload(&ns, &nonce, r.ts), &sig)
        .map_err(|_| err(ErrorCode::Forbidden, "destroy proof invalid"))?;
    let at = now_unix();
    let purged = blocking(app, move |a| a.store.destroy_vault(&ns, at)).await?;
    app.changed.notify_waiters();
    let rb = ReceiptBody {
        ns,
        server_nonce: nonce,
        ts: r.ts,
        destroyed_at: at,
        objects: purged.objects,
        bytes: purged.bytes,
    };
    let receipt = app
        .sign_receipt(&rb.encode())
        .map_err(|_| err(ErrorCode::Internal, "receipt signing failed"))?;
    Ok(json(
        StatusCode::OK,
        &DestroyReceipt {
            receipt: URL_SAFE_NO_PAD.encode(receipt),
            server_key: app.receipt_pub.clone(),
        },
    ))
}
