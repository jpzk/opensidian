// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! M3c: one behaviour suite, run against BOTH transports — the HTTP client
//! talking to a real opensidian-sync-server on 127.0.0.1 (plain and TLS 1.3)
//! and the directory replica — so the engine sees identical semantics.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sync_core::transport::fs::{FsTransport, MARKER};
use sync_core::transport::http::{HttpTransport, Roots};
use sync_core::transport::{
    destroy_vault, missing_all, verify_receipt, PutOutcome, Range, Tokens, Transport, TransportError,
};
use sync_crypto::fips::{FipsProvider, FipsSigningKey};
use sync_crypto::{CryptoProvider, SigAlg};
use sync_proto::api::{destroy_payload, DestroyConfirm, ErrorCode, ListQuery, ObjKind};
use sync_proto::container::{Header, Suite};
use sync_proto::Id16;
use sync_server::{now_unix, serve, serve_tls, server_config, App, Config};

const CA: &[u8] = include_bytes!("../../sync-server/tests/fixtures/test-ca.pem");
const LEAF: &[u8] = include_bytes!("../../sync-server/tests/fixtures/test-leaf.pem");
const KEY: &[u8] = include_bytes!("../../sync-server/tests/fixtures/test-leaf.key");

static N: AtomicU32 = AtomicU32::new(0);

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "osy-tr-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

struct Srv {
    port: u16,
    app: Arc<App>,
    _rt: tokio::runtime::Runtime,
}

fn start(tls: bool) -> Srv {
    let mut cfg = Config::new(tmpdir("srv"));
    cfg.anon_rate_per_min = 10_000;
    let app = Arc::new(App::open(cfg).expect("app"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let l = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = l.local_addr().unwrap().port();
    let a = app.clone();
    if tls {
        let sc = server_config(LEAF, KEY).expect("server config");
        rt.spawn(async move { serve_tls(a, l, sc).await });
    } else {
        rt.spawn(async move { serve(a, l).await });
    }
    Srv { port, app, _rt: rt }
}

struct Vault {
    ns: Id16,
    k_destroy: FipsSigningKey,
    server_key: Vec<u8>,
    tokens: Tokens,
}

fn enroll(srv: &Srv, t: &HttpTransport) -> Vault {
    let p = FipsProvider::new().unwrap();
    let (k, pk) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
    let tok = srv.app.create_enrollment(None, 600).unwrap();
    let e = t.enroll(&tok, &pk).expect("enroll");
    // one-time token
    assert_eq!(t.enroll(&tok, &pk).unwrap_err().code(), Some(ErrorCode::Unauthorized));
    let tokens = Tokens::from(&e);
    t.set_tokens(tokens.clone());
    Vault {
        ns: e.ns,
        k_destroy: k,
        server_key: URL_SAFE_NO_PAD.decode(&e.server_key).unwrap(),
        tokens,
    }
}

fn http(srv: &Srv) -> HttpTransport {
    HttpTransport::new(&format!("http://127.0.0.1:{}", srv.port), Roots::System).unwrap()
}

fn id(b: u8) -> Id16 {
    Id16([b; 16])
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

fn code<T: std::fmt::Debug>(r: Result<T, TransportError>) -> ErrorCode {
    r.unwrap_err().code().expect("an /v1 error code")
}

fn q(after: u64, wait: u32, limit: u32) -> ListQuery {
    ListQuery { after, wait, limit }
}

/// The shared behaviour suite (spec §5).
fn suite(t: &dyn Transport, ns: Id16) {
    use ObjKind::*;
    assert!(t.health().unwrap().ok);

    // write-once
    let a = id(1);
    let ca = container(a, 100, 1);
    assert_eq!(t.put(Obj, &ns, &a, &ca).unwrap(), PutOutcome::Created);
    assert_eq!(t.put(Obj, &ns, &a, &ca).unwrap(), PutOutcome::Identical);
    assert_eq!(code(t.put(Obj, &ns, &a, &container(a, 100, 2))), ErrorCode::Conflict);
    assert_eq!(code(t.put(Obj, &ns, &id(2), &ca)), ErrorCode::BadRequest, "url id != header id");
    assert_eq!(code(t.put(Obj, &ns, &id(2), b"not a container")), ErrorCode::BadRequest);

    // get + ranges
    let f = t.get(Obj, &ns, &a, None).unwrap();
    assert_eq!((f.bytes.as_slice(), f.total), (&ca[..], ca.len() as u64));
    let n = ca.len() as u64;
    let f = t.get(Obj, &ns, &a, Some(Range::Inclusive(0, 3))).unwrap();
    assert_eq!((f.bytes.as_slice(), f.total), (&ca[..4], n));
    let f = t.get(Obj, &ns, &a, Some(Range::From(n - 2))).unwrap();
    assert_eq!(f.bytes, &ca[ca.len() - 2..]);
    let f = t.get(Obj, &ns, &a, Some(Range::Suffix(5))).unwrap();
    assert_eq!(f.bytes, &ca[ca.len() - 5..]);
    let f = t.get(Obj, &ns, &a, Some(Range::Inclusive(10, n + 100))).unwrap();
    assert_eq!(f.bytes, &ca[10..]);
    assert_eq!(
        t.get(Obj, &ns, &a, Some(Range::Inclusive(n, n + 5))).unwrap_err(),
        TransportError::RangeNotSatisfiable
    );
    assert_eq!(code(t.get(Obj, &ns, &id(9), None)), ErrorCode::NotFound);

    // missing
    assert_eq!(t.missing(&ns, &[a, id(9)]).unwrap(), vec![id(9)]);
    let many: Vec<Id16> = (0..1500u32)
        .map(|i| {
            let mut b = [0xEE; 16];
            b[..4].copy_from_slice(&i.to_be_bytes());
            Id16(b)
        })
        .chain([a])
        .collect();
    assert_eq!(missing_all(t, &ns, &many).unwrap().len(), 1500);

    // list cursor
    let c = id(3);
    assert_eq!(t.put(Cfg, &ns, &c, &container(c, 10, 3)).unwrap(), PutOutcome::Created);
    let all = t.list(&ns, q(0, 0, 1000)).unwrap();
    let got: Vec<(ObjKind, Id16)> = all.items.iter().map(|i| (i.kind, i.id)).collect();
    assert_eq!(got, vec![(Obj, a), (Cfg, c)]);
    assert_eq!(all.items[0].size, n);
    assert!(all.items[0].seq < all.items[1].seq);
    assert_eq!(all.next, all.items[1].seq);
    let one = t.list(&ns, q(0, 0, 1)).unwrap();
    assert_eq!((one.items.len(), one.next), (1, all.items[0].seq));
    let rest = t.list(&ns, q(one.next, 0, 1000)).unwrap();
    assert_eq!(rest.items[0].id, c);
    let none = t.list(&ns, q(all.next, 0, 1000)).unwrap();
    assert!(none.items.is_empty());
    assert_eq!(none.next, all.next);
    assert!(t.list(&ns, q(0, 61, 10)).is_err());

    // long-poll: times out empty, and wakes on a write
    let t0 = Instant::now();
    let r = t.list(&ns, q(all.next, 1, 1000)).unwrap();
    assert!(r.items.is_empty() && t0.elapsed() >= Duration::from_millis(900));
    let d = id(4);
    let cd = container(d, 20, 4);
    let woke = std::thread::scope(|s| {
        let h = s.spawn(|| {
            let t0 = Instant::now();
            (t.list(&ns, q(all.next, 20, 1000)).unwrap(), t0.elapsed())
        });
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(t.put(Obj, &ns, &d, &cd).unwrap(), PutOutcome::Created);
        h.join().unwrap()
    });
    assert_eq!(woke.0.items.iter().map(|i| i.id).collect::<Vec<_>>(), vec![d]);
    assert!(woke.1 < Duration::from_secs(10), "long-poll woke late: {:?}", woke.1);

    // soft delete: 410, readable with deleted=1, id never reusable, still "known"
    t.delete(Obj, &ns, &a).unwrap();
    assert_eq!(code(t.get(Obj, &ns, &a, None)), ErrorCode::Gone);
    assert_eq!(t.get_deleted(Obj, &ns, &a).unwrap(), ca);
    assert_eq!(code(t.put(Obj, &ns, &a, &ca)), ErrorCode::Gone);
    assert_eq!(code(t.delete(Obj, &ns, &a)), ErrorCode::NotFound);
    assert_eq!(code(t.delete(Obj, &ns, &id(9))), ErrorCode::NotFound);
    assert!(t.missing(&ns, &[a]).unwrap().is_empty());
    let live: Vec<Id16> = t.list(&ns, q(0, 0, 1000)).unwrap().items.iter().map(|i| i.id).collect();
    assert_eq!(live, vec![c, d]);

    // env: all-zero namespace, not deletable
    let e = id(5);
    let ce = container(e, 30, 5);
    assert_eq!(t.put(Env, &Id16::ZERO, &e, &ce).unwrap(), PutOutcome::Created);
    assert_eq!(t.get(Env, &Id16::ZERO, &e, None).unwrap().bytes, ce);
    assert_eq!(code(t.put(Env, &ns, &e, &ce)), ErrorCode::BadRequest);
    assert_eq!(code(t.delete(Env, &Id16::ZERO, &e)), ErrorCode::Forbidden);

    // pair: single use
    let p = id(6);
    let cp = container(p, 40, 6);
    assert_eq!(t.put(Pair, &ns, &p, &cp).unwrap(), PutOutcome::Created);
    assert_eq!(t.get(Pair, &ns, &p, None).unwrap().bytes, cp);
    t.delete(Pair, &ns, &p).unwrap();
    // purged at once (single use): gone from the replica entirely
    assert_eq!(code(t.get(Pair, &ns, &p, None)), ErrorCode::NotFound);
    assert_eq!(code(t.get_deleted(Pair, &ns, &p)), ErrorCode::NotFound);
    assert_eq!(code(t.delete(Pair, &ns, &p)), ErrorCode::NotFound);

    // a 5 MiB object round-trips intact (beyond the 4 MiB chunk size)
    let big = id(7);
    let mut cb = container(big, 5 * 1024 * 1024, 0);
    for (i, b) in cb.iter_mut().enumerate().skip(64) {
        *b = (i % 251) as u8;
    }
    assert_eq!(t.put(Obj, &ns, &big, &cb).unwrap(), PutOutcome::Created);
    assert!(t.get(Obj, &ns, &big, None).unwrap().bytes == cb);
}

// ---- HTTP ---------------------------------------------------------------------

#[test]
fn http_suite_against_real_server() {
    let srv = start(false);
    let t = http(&srv);
    let v = enroll(&srv, &t);
    suite(&t, v.ns);
}

#[test]
fn http_token_scopes() {
    let srv = start(false);
    let t = http(&srv);
    let v = enroll(&srv, &t);
    let a = id(1);
    t.put(ObjKind::Obj, &v.ns, &a, &container(a, 8, 1)).unwrap();
    // read token used as a write token -> 403 from the server
    let ro = http(&srv);
    ro.set_tokens(Tokens {
        read: None,
        write: v.tokens.read.clone(),
    });
    assert_eq!(code(ro.put(ObjKind::Obj, &v.ns, &id(2), &container(id(2), 8, 1))), ErrorCode::Forbidden);
    // read-only tokens: reads work, writes refused client-side
    ro.set_tokens(Tokens {
        read: v.tokens.read.clone(),
        write: None,
    });
    assert_eq!(ro.get(ObjKind::Obj, &v.ns, &a, None).unwrap().total, 56 + 8);
    assert!(matches!(ro.delete(ObjKind::Obj, &v.ns, &a), Err(TransportError::Config(_))));
    // wrong token / other vault -> 401 (no namespace oracle)
    ro.set_tokens(Tokens {
        read: Some(URL_SAFE_NO_PAD.encode([0u8; 32])),
        write: None,
    });
    assert_eq!(code(ro.get(ObjKind::Obj, &v.ns, &a, None)), ErrorCode::Unauthorized);
    assert_eq!(code(t.list(&id(0x77), q(0, 0, 10))), ErrorCode::Unauthorized);
}

#[test]
fn http_destroy_without_proof_is_refused_with_proof_succeeds() {
    let srv = start(false);
    let t = http(&srv);
    let v = enroll(&srv, &t);
    let p = FipsProvider::new().unwrap();
    let a = id(1);
    t.put(ObjKind::Obj, &v.ns, &a, &container(a, 8, 1)).unwrap();

    // a key that is not K_destroy -> 403, vault intact
    let (wrong, _) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
    let r = destroy_vault(&t, &p, &wrong, &v.ns, &v.server_key, now_unix());
    assert_eq!(code(r), ErrorCode::Forbidden);
    // no signature at all -> 403
    let ch = t.destroy_request(&v.ns).unwrap();
    let r = t.destroy_confirm(
        &v.ns,
        &DestroyConfirm {
            confirm_token: ch.confirm_token,
            ts: now_unix(),
            sig: String::new(),
        },
    );
    assert_eq!(code(r), ErrorCode::Forbidden);
    assert_eq!(t.get(ObjKind::Obj, &v.ns, &a, None).unwrap().total, 64);

    // raw confirm with the real key; receipt verifies, a tampered one does not
    let ch = t.destroy_request(&v.ns).unwrap();
    let mut nonce = [0u8; 32];
    for (i, n) in nonce.iter_mut().enumerate() {
        *n = u8::from_str_radix(&ch.server_nonce[2 * i..2 * i + 2], 16).unwrap();
    }
    let ts = now_unix();
    let sig = p.sign(&v.k_destroy, &destroy_payload(&v.ns, &nonce, ts)).unwrap();
    let confirm = DestroyConfirm {
        confirm_token: ch.confirm_token.clone(),
        ts,
        sig: URL_SAFE_NO_PAD.encode(sig),
    };
    let rc = t.destroy_confirm(&v.ns, &confirm).expect("destroy with proof");
    let body = verify_receipt(&p, &v.server_key, &v.ns, &nonce, ts, &rc).expect("receipt");
    assert_eq!((body.ns, body.objects), (v.ns, 1));
    // a lost response is recoverable: replaying the same confirm returns the
    // stored receipt; another (unknown) confirm token gets nothing
    let again = t.destroy_confirm(&v.ns, &confirm).expect("replayed confirm");
    assert_eq!(again, rc, "same stored receipt");
    let mut other = confirm.clone();
    other.confirm_token = URL_SAFE_NO_PAD.encode([7u8; 32]);
    assert_eq!(code(t.destroy_confirm(&v.ns, &other)), ErrorCode::Gone);
    let mut wire = URL_SAFE_NO_PAD.decode(&rc.receipt).unwrap();
    wire[40] ^= 1;
    let mut bad = rc.clone();
    bad.receipt = URL_SAFE_NO_PAD.encode(&wire);
    assert!(matches!(
        verify_receipt(&p, &v.server_key, &v.ns, &nonce, ts, &bad),
        Err(TransportError::BadReceipt(_))
    ));
    assert!(matches!(
        verify_receipt(&p, &v.server_key, &v.ns, &nonce, ts + 1, &rc),
        Err(TransportError::BadReceipt(_))
    ));
    assert!(matches!(
        verify_receipt(&p, &v.server_key, &id(1), &nonce, ts, &rc),
        Err(TransportError::BadReceipt(_))
    ));
    // vault is gone: tokens dead, destroy again 410
    assert_eq!(code(t.get(ObjKind::Obj, &v.ns, &a, None)), ErrorCode::Unauthorized);
    assert_eq!(code(t.destroy_request(&v.ns)), ErrorCode::Gone);
}

#[test]
fn http_destroy_vault_helper_and_pinned_key_check() {
    let srv = start(false);
    let t = http(&srv);
    let p = FipsProvider::new().unwrap();
    let v = enroll(&srv, &t);
    let body = destroy_vault(&t, &p, &v.k_destroy, &v.ns, &v.server_key, now_unix()).expect("destroy");
    assert_eq!(body.ns, v.ns);
    // a server that signs with a key other than the pinned one is caught
    let w = enroll(&srv, &t);
    let (_, other_pub) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
    let r = destroy_vault(&t, &p, &w.k_destroy, &w.ns, &other_pub, now_unix());
    assert!(matches!(r, Err(TransportError::BadReceipt(_))), "{r:?}");
}

#[test]
fn https_fips_tls13_suite() {
    let srv = start(true);
    let url = format!("https://localhost:{}", srv.port);
    let t = HttpTransport::new(&url, Roots::Pem(CA.to_vec())).unwrap();
    let v = enroll(&srv, &t);
    suite(&t, v.ns);
    // IP SAN verifies too; untrusted roots / plaintext to the TLS port fail
    let ip = HttpTransport::new(&format!("https://127.0.0.1:{}", srv.port), Roots::Pem(CA.to_vec())).unwrap();
    assert!(ip.health().unwrap().ok);
    assert!(HttpTransport::new(&url, Roots::Pem(LEAF.to_vec()))
        .map(|t| t.health())
        .is_ok_and(|r| r.is_err()));
    if let Ok(sys) = HttpTransport::new(&url, Roots::System) {
        assert!(sys.health().is_err(), "test CA must not be in the system bundle");
    }
    let plain = HttpTransport::new(&format!("http://127.0.0.1:{}", srv.port), Roots::System).unwrap();
    assert!(plain.health().is_err());
}

#[test]
fn http_refuses_remote_plaintext_and_garbage_peers() {
    assert!(matches!(
        HttpTransport::new("http://sync.example.org", Roots::System),
        Err(TransportError::Config(_))
    ));
    // a peer that is not HTTP
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = std::thread::spawn(move || {
        use std::io::{Read, Write};
        let (mut s, _) = l.accept().unwrap();
        let mut b = [0u8; 512];
        let _ = s.read(&mut b);
        let _ = s.write_all(b"SSH-2.0-OpenSSH\r\n\r\n");
    });
    let t = HttpTransport::new(&format!("http://127.0.0.1:{port}"), Roots::System).unwrap();
    assert!(matches!(t.health(), Err(TransportError::Protocol(_))));
    h.join().unwrap();
    // nothing listening
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let t = HttpTransport::new(&format!("http://127.0.0.1:{dead}"), Roots::System).unwrap();
    assert!(matches!(t.health(), Err(TransportError::Io(_))));
}

// ---- directory replica -------------------------------------------------------------

#[test]
fn fs_suite() {
    let t = FsTransport::open(tmpdir("fs")).unwrap();
    suite(&t, id(0x42));
}

#[test]
fn fs_refuses_foreign_dir_and_server_only_ops() {
    let d = tmpdir("foreign");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("notes.md"), b"x").unwrap();
    assert!(matches!(FsTransport::open(&d), Err(TransportError::Config(_))));
    let t = FsTransport::open(tmpdir("ops")).unwrap();
    assert!(t.root().join(MARKER).exists());
    assert!(FsTransport::open(t.root()).is_ok(), "reopen");
    assert!(matches!(t.enroll("x", &[4; 65]), Err(TransportError::Unsupported(_))));
    assert!(matches!(t.destroy_request(&id(1)), Err(TransportError::Unsupported(_))));
    let p = FipsProvider::new().unwrap();
    let (k, pk) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
    assert!(matches!(
        destroy_vault(&t, &p, &k, &id(1), &pk, 0),
        Err(TransportError::Unsupported(_))
    ));
    assert_eq!(code(t.put(ObjKind::Obj, &Id16::ZERO, &id(1), &container(id(1), 1, 1))), ErrorCode::BadRequest);
}

#[test]
fn fs_concurrent_writers_get_unique_seqs() {
    let root = tmpdir("conc");
    let a = FsTransport::open(&root).unwrap();
    let b = FsTransport::open(&root).unwrap();
    let ns = id(0x10);
    let same = id(0x99);
    let c = container(same, 16, 9);
    std::thread::scope(|s| {
        for (w, t) in [(0u8, &a), (1u8, &b)] {
            s.spawn(move || {
                for i in 0..50u8 {
                    let mut x = [w; 16];
                    x[15] = i;
                    let x = Id16(x);
                    assert_eq!(t.put(ObjKind::Obj, &ns, &x, &container(x, 16, i)).unwrap(), PutOutcome::Created);
                }
            });
        }
        // both racing on the same id: exactly one Created
        let r = [s.spawn(|| a.put(ObjKind::Obj, &ns, &same, &c)), s.spawn(|| b.put(ObjKind::Obj, &ns, &same, &c))]
            .map(|h| h.join().unwrap().unwrap());
        assert_eq!(r.iter().filter(|o| **o == PutOutcome::Created).count(), 1, "{r:?}");
    });
    let l = a.list(&ns, q(0, 0, 1000)).unwrap();
    let seqs: Vec<u64> = l.items.iter().map(|i| i.seq).collect();
    assert_eq!(seqs, (1..=101).collect::<Vec<u64>>());
}

#[test]
fn fs_torn_log_tail_is_ignored_and_repaired() {
    let t = FsTransport::open(tmpdir("torn")).unwrap();
    let ns = id(0x11);
    t.put(ObjKind::Obj, &ns, &id(1), &container(id(1), 4, 1)).unwrap();
    let log = t.root().join(ns.to_hex()).join("log");
    let mut b = std::fs::read(&log).unwrap();
    b.extend_from_slice(&[1, 2, 3]);
    std::fs::write(&log, &b).unwrap();
    assert_eq!(t.list(&ns, q(0, 0, 10)).unwrap().items.len(), 1);
    t.put(ObjKind::Cfg, &ns, &id(2), &container(id(2), 4, 2)).unwrap();
    let l = t.list(&ns, q(0, 0, 10)).unwrap();
    assert_eq!(l.items.iter().map(|i| (i.seq, i.id)).collect::<Vec<_>>(), vec![(1, id(1)), (2, id(2))]);
}
