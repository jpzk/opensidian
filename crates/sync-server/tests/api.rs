// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! /v1 API behaviour against a real listener on 127.0.0.1 (spec §5). The
//! client is a minimal blocking HTTP/1.1 writer over std TcpStream, so the
//! tests exercise the wire, not the handler functions.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sync_crypto::fips::{FipsProvider, FipsSigningKey};
use sync_crypto::{CryptoProvider, SigAlg};
use sync_proto::api::{
    destroy_payload, DestroyChallenge, DestroyReceipt, EnrollResponse, ErrorBody, ErrorCode, ListResponse,
    MissingResponse, ReceiptBody,
};
use sync_proto::container::{Header, Suite};
use sync_proto::Id16;
use sync_server::{now_unix, serve, App, Config};

struct Srv {
    addr: SocketAddr,
    app: Arc<App>,
    dir: PathBuf,
    _rt: tokio::runtime::Runtime,
}

static N: AtomicU32 = AtomicU32::new(0);

fn start(tweak: impl FnOnce(&mut Config)) -> Srv {
    let dir = std::env::temp_dir().join(format!(
        "osy-srv-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = Config::new(dir.clone());
    tweak(&mut cfg);
    let app = Arc::new(App::open(cfg).expect("app"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let l = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = l.local_addr().unwrap();
    let a = app.clone();
    rt.spawn(async move { serve(a, l).await });
    Srv { addr, app, dir, _rt: rt }
}

struct R {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl R {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
    fn error(&self) -> ErrorCode {
        serde_json::from_slice::<ErrorBody>(&self.body)
            .unwrap_or_else(|_| panic!("not an error body: {}", String::from_utf8_lossy(&self.body)))
            .error
    }
}

fn call(addr: SocketAddr, method: &str, path: &str, hdrs: &[(&str, &str)], body: &[u8]) -> R {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(90))).unwrap();
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in hdrs {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    // The server may answer early (e.g. 413 off Content-Length) and close.
    let _ = s.write_all(body);
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("no header end: {:?}", String::from_utf8_lossy(&raw)));
    let head = String::from_utf8(raw[..split].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
    let headers = lines
        .filter_map(|l| l.split_once(": "))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    R {
        status,
        headers,
        body: raw[split + 4..].to_vec(),
    }
}

fn bearer(t: &str) -> String {
    format!("Bearer {t}")
}

/// A syntactically valid container: real header + `n` opaque body bytes.
fn container(id: Id16, n: usize, fill: u8) -> Vec<u8> {
    let mut v = Header {
        suite: Suite::P256,
        obj_id: id,
        commit: [7; 32],
    }
    .encode()
    .to_vec();
    v.extend(std::iter::repeat_n(fill, n));
    v
}

fn id(b: u8) -> Id16 {
    Id16([b; 16])
}

fn pub_destroy() -> String {
    let p = FipsProvider::new().unwrap();
    let (_k, pk) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
    URL_SAFE_NO_PAD.encode(pk)
}

fn enroll_body(tok: &str, pk: &str, alg: u8) -> Vec<u8> {
    format!(r#"{{"enrollment_token":"{tok}","pub_destroy":"{pk}","key_alg":{alg}}}"#).into_bytes()
}

fn enroll(s: &Srv, quota: Option<u64>) -> EnrollResponse {
    let tok = s.app.create_enrollment(quota, 3600).unwrap();
    let r = call(s.addr, "POST", "/v1/enroll", &[], &enroll_body(&tok, &pub_destroy(), 2));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    serde_json::from_slice(&r.body).unwrap()
}

fn obj_path(ns: &Id16, i: &Id16) -> String {
    format!("/v1/obj/{ns}/{i}")
}

#[test]
fn health_and_routes() {
    let s = start(|_| {});
    let r = call(s.addr, "GET", "/v1/health", &[], b"");
    assert_eq!(r.status, 200);
    assert_eq!(r.body, br#"{"ok":true,"proto":[1]}"#);
    assert_eq!(call(s.addr, "GET", "/v2/health", &[], b"").status, 404);
    assert_eq!(call(s.addr, "GET", "/v1/blob/x/y", &[], b"").status, 404);
    let r = call(s.addr, "GET", "/v1/health", &[("Opensidian-Proto", "2")], b"");
    assert_eq!((r.status, r.error()), (400, ErrorCode::Unsupported));
    let r = call(s.addr, "GET", "/v1/health", &[("Opensidian-Proto", "1")], b"");
    assert_eq!(r.status, 200);
}

#[test]
fn enroll_is_one_time_and_p256_only() {
    let s = start(|_| {});
    let tok = s.app.create_enrollment(None, 3600).unwrap();
    let pk = pub_destroy();
    let r = call(s.addr, "POST", "/v1/enroll", &[], &enroll_body(&tok, &pk, 1));
    assert_eq!((r.status, r.error()), (400, ErrorCode::Unsupported));
    let r = call(s.addr, "POST", "/v1/enroll", &[], &enroll_body(&tok, "AAAA", 2));
    assert_eq!((r.status, r.error()), (400, ErrorCode::BadRequest));
    let r = call(s.addr, "POST", "/v1/enroll", &[], &enroll_body(&tok, &pk, 2));
    assert_eq!(r.status, 201);
    let e: EnrollResponse = serde_json::from_slice(&r.body).unwrap();
    assert_ne!(e.read_token, e.write_token);
    let r = call(s.addr, "POST", "/v1/enroll", &[], &enroll_body(&tok, &pk, 2));
    assert_eq!((r.status, r.error()), (401, ErrorCode::Unauthorized), "token reuse");
    let r = call(s.addr, "POST", "/v1/enroll", &[], br#"{"enrollment_token":"x","pub_destroy":"y","key_alg":2,"z":1}"#);
    assert_eq!((r.status, r.error()), (400, ErrorCode::BadRequest), "unknown field");
    // expired token
    let old = s.app.create_enrollment(None, -1).unwrap();
    let r = call(s.addr, "POST", "/v1/enroll", &[], &enroll_body(&old, &pk, 2));
    assert_eq!(r.status, 401);
}

#[test]
fn tokens_are_stored_hashed() {
    let s = start(|_| {});
    let tok = s.app.create_enrollment(None, 3600).unwrap();
    let r = call(s.addr, "POST", "/v1/enroll", &[], &enroll_body(&tok, &pub_destroy(), 2));
    let e: EnrollResponse = serde_json::from_slice(&r.body).unwrap();
    drop(s._rt);
    let mut db = Vec::new();
    for f in ["index.sqlite", "index.sqlite-wal"] {
        if let Ok(b) = std::fs::read(s.dir.join(f)) {
            db.extend(b);
        }
    }
    assert!(!db.is_empty());
    for t in [&tok, &e.read_token, &e.write_token] {
        let raw = URL_SAFE_NO_PAD.decode(t).unwrap();
        assert!(!db.windows(32).any(|w| w == raw.as_slice()), "raw token bytes in index");
        assert!(!db.windows(t.len()).any(|w| w == t.as_bytes()), "token string in index");
    }
}

#[test]
fn put_is_write_once_and_authorized() {
    let s = start(|c| c.max_object = 4096);
    let e = enroll(&s, None);
    let (w, rd) = (bearer(&e.write_token), bearer(&e.read_token));
    let p = obj_path(&e.ns, &id(1));
    let c1 = container(id(1), 100, 1);
    assert_eq!(call(s.addr, "PUT", &p, &[], &c1).status, 401);
    assert_eq!(call(s.addr, "PUT", &p, &[("Authorization", "Bearer nope")], &c1).status, 401);
    let r = call(s.addr, "PUT", &p, &[("Authorization", &rd)], &c1);
    assert_eq!((r.status, r.error()), (403, ErrorCode::Forbidden));
    // a valid token of another vault does not open this one
    let other = enroll(&s, None);
    assert_eq!(call(s.addr, "PUT", &p, &[("Authorization", &bearer(&other.write_token))], &c1).status, 401);
    assert_eq!(call(s.addr, "PUT", &p, &[("Authorization", &w)], &c1).status, 201);
    assert_eq!(call(s.addr, "PUT", &p, &[("Authorization", &w)], &c1).status, 200, "idempotent");
    let r = call(s.addr, "PUT", &p, &[("Authorization", &w)], &container(id(1), 100, 2));
    assert_eq!((r.status, r.error()), (409, ErrorCode::Conflict));
    let r = call(s.addr, "PUT", &obj_path(&e.ns, &id(2)), &[("Authorization", &w)], &c1);
    assert_eq!((r.status, r.error()), (400, ErrorCode::BadRequest), "url id != header id");
    let mut bad = container(id(3), 10, 0);
    bad[0] = b'X';
    let r = call(s.addr, "PUT", &obj_path(&e.ns, &id(3)), &[("Authorization", &w)], &bad);
    assert_eq!(r.status, 400);
    let r = call(s.addr, "PUT", &obj_path(&e.ns, &id(4)), &[("Authorization", &w)], &container(id(4), 5000, 0));
    assert_eq!((r.status, r.error()), (413, ErrorCode::TooLarge));
    let r = call(s.addr, "PUT", &format!("/v1/obj/{}/ABC", e.ns), &[("Authorization", &w)], &c1);
    assert_eq!(r.status, 400);
}

#[test]
fn get_full_and_ranges() {
    let s = start(|_| {});
    let e = enroll(&s, None);
    let (w, rd) = (bearer(&e.write_token), bearer(&e.read_token));
    let p = obj_path(&e.ns, &id(9));
    let mut c = container(id(9), 1000, 0);
    for (i, b) in c.iter_mut().enumerate().skip(56) {
        *b = i as u8;
    }
    assert_eq!(call(s.addr, "GET", &p, &[("Authorization", &rd)], b"").status, 404);
    assert_eq!(call(s.addr, "PUT", &p, &[("Authorization", &w)], &c).status, 201);
    assert_eq!(call(s.addr, "GET", &p, &[], b"").status, 401);
    let r = call(s.addr, "GET", &p, &[("Authorization", &rd)], b"");
    assert_eq!((r.status, &r.body), (200, &c));
    assert_eq!(r.header("accept-ranges"), Some("bytes"));
    let r = call(s.addr, "GET", &p, &[("Authorization", &w)], b"");
    assert_eq!(r.status, 200, "write implies read");
    let r = call(s.addr, "GET", &p, &[("Authorization", &rd), ("Range", "bytes=100-199")], b"");
    assert_eq!(r.status, 206);
    assert_eq!(r.body, c[100..200]);
    assert_eq!(r.header("content-range"), Some("bytes 100-199/1056"));
    let r = call(s.addr, "GET", &p, &[("Authorization", &rd), ("Range", "bytes=1000-")], b"");
    assert_eq!((r.status, &r.body[..]), (206, &c[1000..]));
    let r = call(s.addr, "GET", &p, &[("Authorization", &rd), ("Range", "bytes=-6")], b"");
    assert_eq!((r.status, &r.body[..]), (206, &c[1050..]));
    let r = call(s.addr, "GET", &p, &[("Authorization", &rd), ("Range", "bytes=1000-99999")], b"");
    assert_eq!((r.status, &r.body[..]), (206, &c[1000..]), "end clamps");
    for bad in ["bytes=2000-", "bytes=5-1", "bytes=0-1,4-5", "items=0-1", "bytes=-0"] {
        let r = call(s.addr, "GET", &p, &[("Authorization", &rd), ("Range", bad)], b"");
        assert_eq!(r.status, 416, "{bad}");
        assert_eq!(r.header("content-range"), Some("bytes */1056"));
    }
}

#[test]
fn missing_reports_unknown_ids() {
    let s = start(|_| {});
    let e = enroll(&s, None);
    let w = bearer(&e.write_token);
    for b in [1u8, 3] {
        let r = call(s.addr, "PUT", &obj_path(&e.ns, &id(b)), &[("Authorization", &w)], &container(id(b), 8, b));
        assert_eq!(r.status, 201);
    }
    let body = format!(r#"{{"ids":["{}","{}","{}"]}}"#, id(1), id(2), id(3));
    let p = format!("/v1/missing/{}", e.ns);
    assert_eq!(call(s.addr, "POST", &p, &[], body.as_bytes()).status, 401);
    let r = call(s.addr, "POST", &p, &[("Authorization", &bearer(&e.read_token))], body.as_bytes());
    assert_eq!(r.status, 200);
    let m: MissingResponse = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(m.missing, vec![id(2)]);
    let many = format!(
        r#"{{"ids":[{}]}}"#,
        vec![format!("\"{}\"", id(5)); 1025].join(",")
    );
    let r = call(s.addr, "POST", &p, &[("Authorization", &w)], many.as_bytes());
    assert_eq!((r.status, r.error()), (413, ErrorCode::TooLarge));
}

#[test]
fn control_body_limit() {
    let s = start(|_| {});
    let e = enroll(&s, None);
    let big = vec![b' '; 70 * 1024];
    let r = call(s.addr, "POST", &format!("/v1/missing/{}", e.ns), &[("Authorization", &bearer(&e.write_token))], &big);
    assert_eq!((r.status, r.error()), (413, ErrorCode::TooLarge));
}

#[test]
fn list_cursor_and_long_poll() {
    let s = start(|_| {});
    let e = enroll(&s, None);
    let (w, rd) = (bearer(&e.write_token), bearer(&e.read_token));
    for b in 1..=5u8 {
        call(s.addr, "PUT", &obj_path(&e.ns, &id(b)), &[("Authorization", &w)], &container(id(b), b as usize, 0));
    }
    let cfg = format!("/v1/cfg/{}/{}", e.ns, id(6));
    assert_eq!(call(s.addr, "PUT", &cfg, &[("Authorization", &w)], &container(id(6), 1, 0)).status, 201);
    let lp = |q: &str| {
        let r = call(s.addr, "GET", &format!("/v1/list/{}?{q}", e.ns), &[("Authorization", &rd)], b"");
        assert_eq!(r.status, 200, "{q}: {}", String::from_utf8_lossy(&r.body));
        serde_json::from_slice::<ListResponse>(&r.body).unwrap()
    };
    let all = lp("after=0");
    assert_eq!(all.items.len(), 6);
    assert_eq!(all.items.iter().map(|i| i.seq).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(all.items[5].kind, sync_proto::api::ObjKind::Cfg);
    assert_eq!(all.items[2].size, 56 + 3);
    assert_eq!(all.next, 6);
    let page = lp("after=2&limit=2");
    assert_eq!((page.items.len(), page.next), (2, 4));
    for bad in ["wait=61", "limit=0", "limit=1001", "after=x"] {
        let r = call(s.addr, "GET", &format!("/v1/list/{}?{bad}", e.ns), &[("Authorization", &rd)], b"");
        assert_eq!(r.status, 400, "{bad}");
    }
    assert_eq!(call(s.addr, "GET", &format!("/v1/list/{}", e.ns), &[], b"").status, 401);
    // empty long-poll returns after `wait`
    let t = Instant::now();
    let r = lp("after=6&wait=1");
    assert!(r.items.is_empty() && r.next == 6);
    assert!(t.elapsed() >= Duration::from_millis(900), "{:?}", t.elapsed());
    // a PUT wakes a waiting poll early
    let (addr, ns, w2) = (s.addr, e.ns, w.clone());
    let h = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        call(addr, "PUT", &obj_path(&ns, &id(7)), &[("Authorization", &w2)], &container(id(7), 1, 0)).status
    });
    let t = Instant::now();
    let r = lp("after=6&wait=30");
    assert_eq!(h.join().unwrap(), 201);
    assert_eq!(r.items.len(), 1);
    assert_eq!((r.items[0].id, r.next), (id(7), 7));
    assert!(t.elapsed() < Duration::from_secs(10), "woke at {:?}", t.elapsed());
}

#[test]
fn soft_delete_retention_and_gc() {
    let s = start(|c| c.retention_secs = 100);
    let e = enroll(&s, Some(10_000));
    let (w, rd) = (bearer(&e.write_token), bearer(&e.read_token));
    let p = obj_path(&e.ns, &id(1));
    let c = container(id(1), 44, 5);
    assert_eq!(call(s.addr, "PUT", &p, &[("Authorization", &w)], &c).status, 201);
    assert_eq!(call(s.addr, "DELETE", &p, &[("Authorization", &rd)], b"").status, 403);
    assert_eq!(call(s.addr, "DELETE", &p, &[("Authorization", &w)], b"").status, 202);
    assert_eq!(call(s.addr, "DELETE", &p, &[("Authorization", &w)], b"").status, 404, "already deleted");
    let r = call(s.addr, "GET", &p, &[("Authorization", &rd)], b"");
    assert_eq!((r.status, r.error()), (410, ErrorCode::Gone));
    let r = call(s.addr, "GET", &format!("{p}?deleted=1"), &[("Authorization", &rd)], b"");
    assert_eq!((r.status, &r.body), (200, &c), "readable within retention");
    assert_eq!(call(s.addr, "PUT", &p, &[("Authorization", &w)], &c).status, 410, "id not reusable");
    let r = call(s.addr, "GET", &format!("/v1/list/{}", e.ns), &[("Authorization", &rd)], b"");
    let l: ListResponse = serde_json::from_slice(&r.body).unwrap();
    assert!(l.items.is_empty(), "deleted rows leave the list");
    assert_eq!(s.app.store.vault(&e.ns).unwrap().unwrap().used, 100);
    assert_eq!(s.app.store.gc(now_unix() + 50).unwrap(), 0, "inside retention");
    assert_eq!(s.app.store.gc(now_unix() + 101).unwrap(), 1);
    let r = call(s.addr, "GET", &format!("{p}?deleted=1"), &[("Authorization", &rd)], b"");
    assert_eq!(r.status, 404, "purged");
    assert_eq!(s.app.store.vault(&e.ns).unwrap().unwrap().used, 0, "quota released");
}

#[test]
fn quota_answers_507() {
    let s = start(|_| {});
    let e = enroll(&s, Some(300));
    let w = bearer(&e.write_token);
    assert_eq!(call(s.addr, "PUT", &obj_path(&e.ns, &id(1)), &[("Authorization", &w)], &container(id(1), 144, 0)).status, 201);
    let r = call(s.addr, "PUT", &obj_path(&e.ns, &id(2)), &[("Authorization", &w)], &container(id(2), 144, 0));
    assert_eq!((r.status, r.error()), (507, ErrorCode::Quota));
    let r = call(s.addr, "PUT", &obj_path(&e.ns, &id(3)), &[("Authorization", &w)], &container(id(3), 44, 0));
    assert_eq!(r.status, 201, "fits exactly: 200 + 100 = 300");
}

#[test]
fn env_by_loc_without_token() {
    let s = start(|_| {});
    let loc = id(0xab);
    let p = format!("/v1/env/{}/{loc}", Id16::ZERO);
    let c = container(loc, 64, 3);
    assert_eq!(call(s.addr, "GET", &p, &[], b"").status, 404);
    assert_eq!(call(s.addr, "PUT", &p, &[], &c).status, 201);
    assert_eq!(call(s.addr, "PUT", &p, &[], &container(loc, 64, 4)).status, 409, "write-once");
    let r = call(s.addr, "GET", &p, &[], b"");
    assert_eq!((r.status, &r.body), (200, &c));
    assert_eq!(call(s.addr, "DELETE", &p, &[], b"").status, 403);
    let r = call(s.addr, "GET", &format!("/v1/env/{}/{loc}", id(1)), &[], b"");
    assert_eq!(r.status, 400, "env only in the zero namespace");
}

#[test]
fn pair_written_by_vault_read_once_by_id() {
    let s = start(|_| {});
    let e = enroll(&s, None);
    let p = format!("/v1/pair/{}/{}", e.ns, id(0x55));
    let c = container(id(0x55), 30, 1);
    assert_eq!(call(s.addr, "PUT", &p, &[], &c).status, 401);
    assert_eq!(call(s.addr, "PUT", &p, &[("Authorization", &bearer(&e.write_token))], &c).status, 201);
    let r = call(s.addr, "GET", &p, &[], b"");
    assert_eq!((r.status, &r.body), (200, &c));
    assert_eq!(call(s.addr, "DELETE", &p, &[], b"").status, 202);
    assert_eq!(call(s.addr, "GET", &p, &[], b"").status, 404, "purged immediately");
    // TTL expiry
    let p2 = format!("/v1/pair/{}/{}", e.ns, id(0x56));
    assert_eq!(call(s.addr, "PUT", &p2, &[("Authorization", &bearer(&e.write_token))], &container(id(0x56), 1, 0)).status, 201);
    assert_eq!(s.app.store.gc(now_unix() + 15 * 60 + 1).unwrap(), 1);
    assert_eq!(call(s.addr, "GET", &p2, &[], b"").status, 404);
}

#[test]
fn anonymous_paths_are_rate_limited() {
    let s = start(|c| c.anon_rate_per_min = 5);
    let p = format!("/v1/env/{}/{}", Id16::ZERO, id(1));
    let codes: Vec<u16> = (0..7).map(|_| call(s.addr, "GET", &p, &[], b"").status).collect();
    // a minute boundary may reset the window once; at least one 429 overall
    assert!(codes.contains(&429), "{codes:?}");
    assert_eq!(codes[0], 404);
}

// ---- destroy (M3b, spec §5: 2-step, K_destroy proof, signed receipt) -------------

fn enroll_with_key(s: &Srv) -> (EnrollResponse, FipsSigningKey) {
    let p = FipsProvider::new().unwrap();
    let (k, pk) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
    let tok = s.app.create_enrollment(None, 3600).unwrap();
    let r = call(s.addr, "POST", "/v1/enroll", &[], &enroll_body(&tok, &URL_SAFE_NO_PAD.encode(pk), 2));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let e: EnrollResponse = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(e.server_key, s.app.receipt_pub);
    (e, k)
}

fn unhex32(s: &str) -> [u8; 32] {
    assert_eq!(s.len(), 64);
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}

fn destroy_request(s: &Srv, ns: &Id16) -> R {
    call(s.addr, "POST", &format!("/v1/destroy/{ns}/request"), &[], br#"{"scope":"vault"}"#)
}

fn challenge(s: &Srv, ns: &Id16) -> DestroyChallenge {
    let r = destroy_request(s, ns);
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let c: DestroyChallenge = serde_json::from_slice(&r.body).unwrap();
    let now = now_unix() as u64;
    assert!(c.expires > now && c.expires <= now + 301, "ttl");
    c
}

fn sign_destroy(key: &FipsSigningKey, ns: &Id16, c: &DestroyChallenge, ts: i64) -> String {
    let p = FipsProvider::new().unwrap();
    let sig = p.sign(key, &destroy_payload(ns, &unhex32(&c.server_nonce), ts)).unwrap();
    URL_SAFE_NO_PAD.encode(sig)
}

fn confirm(s: &Srv, ns: &Id16, token: &str, ts: i64, sig: &str) -> R {
    let body = format!(r#"{{"confirm_token":"{token}","ts":{ts},"sig":"{sig}"}}"#);
    call(s.addr, "POST", &format!("/v1/destroy/{ns}/confirm"), &[], body.as_bytes())
}

/// The vault still serves its objects (destroy did not happen).
fn vault_alive(s: &Srv, e: &EnrollResponse, i: &Id16) {
    let r = call(s.addr, "GET", &obj_path(&e.ns, i), &[("Authorization", &bearer(&e.read_token))], b"");
    assert_eq!(r.status, 200, "vault must be intact");
}

#[test]
fn destroy_without_valid_proof_is_refused() {
    let s = start(|_| {});
    let (e, key) = enroll_with_key(&s);
    let i = id(1);
    let wt = bearer(&e.write_token);
    assert_eq!(call(s.addr, "PUT", &obj_path(&e.ns, &i), &[("Authorization", &wt)], &container(i, 10, 1)).status, 201);
    let now = now_unix();

    // request: unknown vault / bad scope / unknown field
    let r = destroy_request(&s, &id(0xee));
    assert_eq!((r.status, r.error()), (401, ErrorCode::Unauthorized));
    let r = call(s.addr, "POST", &format!("/v1/destroy/{}/request", Id16::ZERO), &[], br#"{"scope":"vault"}"#);
    assert_eq!(r.status, 401, "env namespace is not a vault");
    let r = call(s.addr, "POST", &format!("/v1/destroy/{}/request", e.ns), &[], br#"{"scope":"all"}"#);
    assert_eq!((r.status, r.error()), (400, ErrorCode::BadRequest));

    // no proof at all: body without sig
    let c = challenge(&s, &e.ns);
    let body = format!(r#"{{"confirm_token":"{}","ts":{now}}}"#, c.confirm_token);
    let r = call(s.addr, "POST", &format!("/v1/destroy/{}/confirm", e.ns), &[], body.as_bytes());
    assert_eq!((r.status, r.error()), (400, ErrorCode::BadRequest));
    // empty / garbage signature
    let r = confirm(&s, &e.ns, &c.confirm_token, now, "");
    assert_eq!((r.status, r.error()), (403, ErrorCode::Forbidden));
    vault_alive(&s, &e, &i);
    // the challenge was consumed by the failed attempt: a correct proof on it is refused
    let r = confirm(&s, &e.ns, &c.confirm_token, now, &sign_destroy(&key, &e.ns, &c, now));
    assert_eq!((r.status, r.error()), (401, ErrorCode::Unauthorized), "single use");

    // signature by another key (e.g. a device key, not K_destroy)
    let p = FipsProvider::new().unwrap();
    let (other, _) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
    let c = challenge(&s, &e.ns);
    let r = confirm(&s, &e.ns, &c.confirm_token, now, &sign_destroy(&other, &e.ns, &c, now));
    assert_eq!((r.status, r.error()), (403, ErrorCode::Forbidden), "wrong key");

    // right key, but signed over a different ts / a different nonce
    let c = challenge(&s, &e.ns);
    let r = confirm(&s, &e.ns, &c.confirm_token, now, &sign_destroy(&key, &e.ns, &c, now + 1));
    assert_eq!((r.status, r.error()), (403, ErrorCode::Forbidden), "ts not bound");
    let c1 = challenge(&s, &e.ns);
    let c2 = challenge(&s, &e.ns);
    let r = confirm(&s, &e.ns, &c2.confirm_token, now, &sign_destroy(&key, &e.ns, &c1, now));
    assert_eq!((r.status, r.error()), (403, ErrorCode::Forbidden), "nonce not bound");

    // right key + nonce, ts outside the skew window
    let c = challenge(&s, &e.ns);
    let old = now - 3600;
    let r = confirm(&s, &e.ns, &c.confirm_token, old, &sign_destroy(&key, &e.ns, &c, old));
    assert_eq!((r.status, r.error()), (400, ErrorCode::BadRequest), "stale ts");

    // a challenge of vault A cannot confirm vault B
    let (e2, key2) = enroll_with_key(&s);
    let c = challenge(&s, &e.ns);
    let r = confirm(&s, &e2.ns, &c.confirm_token, now, &sign_destroy(&key2, &e2.ns, &c, now));
    assert_eq!((r.status, r.error()), (401, ErrorCode::Unauthorized), "cross-vault token");

    // malformed / unknown confirm token
    let r = confirm(&s, &e.ns, "nope", now, "AAAA");
    assert_eq!((r.status, r.error()), (401, ErrorCode::Unauthorized));
    let r = confirm(&s, &e.ns, &URL_SAFE_NO_PAD.encode([5u8; 32]), now, "AAAA");
    assert_eq!((r.status, r.error()), (401, ErrorCode::Unauthorized));

    vault_alive(&s, &e, &i);
    assert!(!s.app.store.destroyed(&e.ns).unwrap());
}

#[test]
fn destroy_challenge_expires() {
    let s = start(|_| {});
    let (e, key) = enroll_with_key(&s);
    let i = id(2);
    let wt = bearer(&e.write_token);
    assert_eq!(call(s.addr, "PUT", &obj_path(&e.ns, &i), &[("Authorization", &wt)], &container(i, 10, 1)).status, 201);
    let raw = [9u8; 32];
    let nonce = [4u8; 32];
    let now = now_unix();
    s.app
        .store
        .add_destroy_challenge(&e.ns, &s.app.token_hash(&raw), &nonce, now - 400, now - 100)
        .unwrap();
    let c = DestroyChallenge {
        server_nonce: sync_proto::hex_lower(&nonce),
        confirm_token: URL_SAFE_NO_PAD.encode(raw),
        expires: (now - 100) as u64,
    };
    let r = confirm(&s, &e.ns, &c.confirm_token, now, &sign_destroy(&key, &e.ns, &c, now));
    assert_eq!((r.status, r.error()), (401, ErrorCode::Unauthorized), "expired challenge");
    vault_alive(&s, &e, &i);
}

#[test]
fn destroy_open_challenges_are_capped() {
    let s = start(|_| {});
    let (e, _) = enroll_with_key(&s);
    for _ in 0..sync_server::store::MAX_DESTROY_CHALLENGES {
        challenge(&s, &e.ns);
    }
    let r = destroy_request(&s, &e.ns);
    assert_eq!((r.status, r.error()), (429, ErrorCode::RateLimited));
}

#[test]
fn destroy_with_proof_purges_vault_and_signs_receipt() {
    let s = start(|_| {});
    let (e, key) = enroll_with_key(&s);
    let (keep, _) = enroll_with_key(&s);
    let wt = bearer(&e.write_token);
    let mut stored = 0u64;
    for b in 1..=3u8 {
        let body = container(id(b), 100 * b as usize, b);
        stored += body.len() as u64;
        assert_eq!(call(s.addr, "PUT", &obj_path(&e.ns, &id(b)), &[("Authorization", &wt)], &body).status, 201);
    }
    let kt = bearer(&keep.write_token);
    assert_eq!(call(s.addr, "PUT", &obj_path(&keep.ns, &id(9)), &[("Authorization", &kt)], &container(id(9), 5, 9)).status, 201);
    assert!(s.dir.join("objects").join(e.ns.to_hex()).exists());

    let c = challenge(&s, &e.ns);
    let ts = now_unix();
    let r = confirm(&s, &e.ns, &c.confirm_token, ts, &sign_destroy(&key, &e.ns, &c, ts));
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let rc: DestroyReceipt = serde_json::from_slice(&r.body).unwrap();

    // receipt: signed by the key pinned at enrollment, attests this destroy
    assert_eq!(rc.server_key, e.server_key);
    let wire = URL_SAFE_NO_PAD.decode(&rc.receipt).unwrap();
    let (body, signed, sig) = ReceiptBody::split(&wire).unwrap();
    let spk = URL_SAFE_NO_PAD.decode(&rc.server_key).unwrap();
    let p = FipsProvider::new().unwrap();
    p.verify(SigAlg::EcdsaP256Sha256, &spk, signed, sig).expect("receipt signature");
    let mut tampered = signed.to_vec();
    tampered[40] ^= 1;
    assert!(p.verify(SigAlg::EcdsaP256Sha256, &spk, &tampered, sig).is_err());
    assert_eq!(body.ns, e.ns);
    assert_eq!(body.server_nonce, unhex32(&c.server_nonce));
    assert_eq!(body.ts, ts);
    assert!((body.destroyed_at - ts).abs() <= 5);
    assert_eq!((body.objects, body.bytes), (3, stored));

    // the vault is gone: tokens dead, files gone, tombstone answers 410
    let r = call(s.addr, "GET", &obj_path(&e.ns, &id(1)), &[("Authorization", &bearer(&e.read_token))], b"");
    assert_eq!(r.status, 401);
    let r = call(s.addr, "PUT", &obj_path(&e.ns, &id(4)), &[("Authorization", &wt)], &container(id(4), 1, 4));
    assert_eq!(r.status, 401);
    assert!(!s.dir.join("objects").join(e.ns.to_hex()).exists());
    assert!(s.app.store.destroyed(&e.ns).unwrap());
    let r = destroy_request(&s, &e.ns);
    assert_eq!((r.status, r.error()), (410, ErrorCode::Gone));
    let r = confirm(&s, &e.ns, &c.confirm_token, ts, &sign_destroy(&key, &e.ns, &c, ts));
    assert_eq!((r.status, r.error()), (410, ErrorCode::Gone), "replay");
    let leftovers: Vec<_> = std::fs::read_dir(s.dir.join("tmp")).unwrap().collect();
    assert!(leftovers.is_empty(), "trash removed");

    // other vaults untouched
    let r = call(s.addr, "GET", &obj_path(&keep.ns, &id(9)), &[("Authorization", &bearer(&keep.read_token))], b"");
    assert_eq!(r.status, 200);
}

#[test]
fn receipt_key_persists_and_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let s = start(|_| {});
    let path = s.dir.join(sync_server::RECEIPT_KEY_FILE);
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let again = App::open(Config::new(s.dir.clone())).unwrap();
    assert_eq!(again.receipt_pub, s.app.receipt_pub);
    let pk = URL_SAFE_NO_PAD.decode(&s.app.receipt_pub).unwrap();
    assert_eq!((pk.len(), pk[0]), (65, 4));
}

#[test]
fn env_object_and_namespace_caps() {
    // Tokenless env writes have no quota owner: per-object and whole-namespace
    // caps (CodeRabbit #238: bound anonymous envelope storage).
    let s = start(|c| {
        c.max_env_object = 1024;
        c.max_env_total = 3000;
    });
    let p = |loc: Id16| format!("/v1/env/{}/{loc}", Id16::ZERO);
    let r = call(s.addr, "PUT", &p(id(0x10)), &[], &container(id(0x10), 2000, 1));
    assert_eq!(r.status, 413, "env object over max_env_object");
    for b in 0x11..0x14u8 {
        let r = call(s.addr, "PUT", &p(id(b)), &[], &container(id(b), 900, b));
        assert_eq!(r.status, 201, "env {b:#x} under the namespace cap");
    }
    let r = call(s.addr, "PUT", &p(id(0x14)), &[], &container(id(0x14), 900, 9));
    assert_eq!((r.status, r.error()), (507, ErrorCode::Quota), "env namespace full");
    // Vault objects are not affected by the env caps.
    let e = enroll(&s, None);
    let tok = bearer(&e.write_token);
    let ns = e.ns;
    let r = call(s.addr, "PUT", &obj_path(&ns, &id(0x20)), &[("Authorization", &tok)], &container(id(0x20), 4000, 2));
    assert_eq!(r.status, 201);
}

/// Send the headers and `sent` of `len` body bytes, then stall.
fn stalled_put(addr: SocketAddr, path: &str, body: &[u8], sent: usize) -> TcpStream {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let head = format!(
        "PUT {path} HTTP/1.1\r\nHost: t\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).unwrap();
    s.write_all(&body[..sent]).unwrap();
    s
}

fn status_of(mut s: TcpStream) -> u16 {
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let line = String::from_utf8_lossy(&raw).lines().next().unwrap_or("").to_string();
    line.split(' ').nth(1).and_then(|v| v.parse().ok()).unwrap_or(0)
}

#[test]
fn stalled_body_hits_the_deadline() {
    let s = start(|c| c.body_timeout_secs = 1);
    let loc = id(0x31);
    let c = container(loc, 500, 1);
    let t = Instant::now();
    let conn = stalled_put(s.addr, &format!("/v1/env/{}/{loc}", Id16::ZERO), &c, 10);
    assert_eq!(status_of(conn), 400, "stalled body answered, not held open");
    assert!(t.elapsed() < Duration::from_secs(10), "deadline enforced: {:?}", t.elapsed());
}

#[test]
fn body_budget_is_shared_across_connections() {
    // Budget = one max_object: a second upload waits until the first one's
    // reservation is released (here: its deadline), instead of buffering both.
    let s = start(|c| {
        c.max_object = 4096;
        c.max_inflight_body = 4096;
        c.max_control = 1024;
        c.body_timeout_secs = 3;
    });
    let a = stalled_put(s.addr, &format!("/v1/env/{}/{}", Id16::ZERO, id(0x41)), &container(id(0x41), 3900, 1), 10);
    std::thread::sleep(Duration::from_millis(1000));
    let t = Instant::now();
    let b = container(id(0x42), 3900, 2);
    let r = call(s.addr, "PUT", &format!("/v1/env/{}/{}", Id16::ZERO, id(0x42)), &[], &b);
    assert_eq!(r.status, 201, "second upload proceeds once the budget frees");
    assert!(t.elapsed() >= Duration::from_millis(1500), "second upload waited: {:?}", t.elapsed());
    assert_eq!(status_of(a), 400, "first upload hit its deadline");
}
