// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! TLS termination (M3a): rustls on the OpenSSL 3 FIPS provider (rustls-openssl), TLS 1.3 only.
//! Fixtures in tests/fixtures are a throwaway test CA + leaf (P-256,
//! SAN localhost/127.0.0.1) generated with the box's openssl CLI; the leaf
//! key is test-only and protects nothing.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use sync_server::tls::fips_provider;
use sync_server::{serve_tls, server_config, App, Config};

const CA: &[u8] = include_bytes!("fixtures/test-ca.pem");
const LEAF: &[u8] = include_bytes!("fixtures/test-leaf.pem");
const KEY: &[u8] = include_bytes!("fixtures/test-leaf.key");

static N: AtomicU32 = AtomicU32::new(0);

fn start() -> (SocketAddr, tokio::runtime::Runtime) {
    let dir = std::env::temp_dir().join(format!(
        "osy-tls-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let app = Arc::new(App::open(Config::new(dir)).expect("app"));
    let cfg = server_config(LEAF, KEY).expect("server config");
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let l = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let addr = l.local_addr().unwrap();
    rt.spawn(async move { serve_tls(app, l, cfg).await });
    (addr, rt)
}

fn client(roots_pem: &[u8]) -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    for c in CertificateDer::pem_slice_iter(roots_pem) {
        roots.add(c.unwrap()).unwrap();
    }
    let cfg = ClientConfig::builder_with_provider(fips_provider().expect("fips provider"))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    assert!(cfg.fips(), "client config must be FIPS");
    Arc::new(cfg)
}

fn get(addr: SocketAddr, cfg: Arc<ClientConfig>, name: &'static str) -> std::io::Result<(String, String)> {
    let sock = TcpStream::connect(addr)?;
    sock.set_read_timeout(Some(Duration::from_secs(20)))?;
    let conn = ClientConnection::new(cfg, ServerName::try_from(name).unwrap()).unwrap();
    let mut s = StreamOwned::new(conn, sock);
    s.write_all(b"GET /v1/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut out = Vec::new();
    match s.read_to_end(&mut out) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof && !out.is_empty() => {}
        Err(e) => return Err(e),
    }
    let suite = format!("{:?}", s.conn.negotiated_cipher_suite().map(|c| c.suite()));
    Ok((String::from_utf8_lossy(&out).into_owned(), suite))
}

#[test]
fn server_config_is_fips() {
    let cfg = server_config(LEAF, KEY).expect("server config");
    assert!(cfg.fips());
    assert_eq!(cfg.alpn_protocols, vec![b"http/1.1".to_vec()]);
}

#[test]
fn bad_pem_is_refused() {
    assert!(server_config(b"", KEY).is_err());
    assert!(server_config(LEAF, b"not a key").is_err());
    assert!(server_config(KEY, KEY).is_err());
}

#[test]
fn health_over_tls13_with_trusted_ca() {
    let (addr, _rt) = start();
    let (resp, suite) = get(addr, client(CA), "localhost").expect("tls get");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    assert!(resp.contains("\"ok\":true"), "{resp}");
    assert!(suite.contains("TLS13_AES"), "suite {suite}");
}

#[test]
fn ip_san_also_verifies() {
    let (addr, _rt) = start();
    let (resp, _) = get(addr, client(CA), "127.0.0.1").expect("tls get by ip");
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
}

#[test]
fn untrusted_ca_and_wrong_name_fail() {
    let (addr, _rt) = start();
    // the leaf is not a CA: trusting it as a root does not validate
    assert!(get(addr, client(LEAF), "localhost").is_err());
    assert!(get(addr, client(CA), "example.org").is_err());
}

#[test]
fn plain_http_on_tls_port_gets_no_http_response() {
    let (addr, _rt) = start();
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    s.write_all(b"GET /v1/health HTTP/1.1\r\nHost: t\r\nConnection: close\r\n\r\n").unwrap();
    let mut out = Vec::new();
    let _ = s.read_to_end(&mut out);
    assert!(!out.starts_with(b"HTTP/"), "plaintext got an HTTP answer");
}
