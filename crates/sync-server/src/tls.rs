// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! TLS termination with rustls on the OpenSSL 3 FIPS provider (spec §5, plan M3).
//!
//! The rustls crypto provider is `rustls-openssl` restricted to the TLS 1.3
//! AES-GCM suites and the P-256/P-384 ECDHE groups ([`fips_provider`]); every
//! primitive is fetched through EVP under the process-wide default
//! properties `fips=yes` that [`sync_crypto::fips::init`] establishes
//! (cmvp.md §6). Protocol: TLS 1.3 only, ALPN `http/1.1`, no client auth
//! (bearer tokens authenticate). [`server_config`] refuses to return a config
//! unless approved mode is active and `ServerConfig::fips()` is true.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;

use crate::http::{serve_conn, spawn_gc, MAX_CONNECTIONS};
use crate::App;

/// Bound on the TLS handshake (slow-loris before HTTP starts).
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The rustls crypto provider on the OpenSSL FIPS provider: TLS 1.3
/// AES-256-GCM / AES-128-GCM (no ChaCha20-Poly1305: not approved), ECDHE
/// P-256 / P-384 (no X25519 / ML-KEM hybrids: keep to the groups the
/// certificate lists for KAS-ECC-SSC, SP 800-56Ar3). Refuses unless approved mode
/// is active.
pub fn fips_provider() -> Result<Arc<rustls::crypto::CryptoProvider>, String> {
    sync_crypto::fips::init().map_err(|e| e.to_string())?;
    let p = rustls_openssl::custom_provider(
        vec![
            rustls_openssl::cipher_suite::TLS13_AES_256_GCM_SHA384,
            rustls_openssl::cipher_suite::TLS13_AES_128_GCM_SHA256,
        ],
        vec![rustls_openssl::kx_group::SECP256R1, rustls_openssl::kx_group::SECP384R1],
    );
    if !p.fips() {
        return Err("rustls-openssl provider does not report FIPS mode".into());
    }
    Ok(Arc::new(p))
}

/// Build the server config from a PEM certificate chain (leaf first) and a
/// PEM private key (PKCS#8, SEC1 or PKCS#1). Errors are operator-facing text.
pub fn server_config(cert_pem: &[u8], key_pem: &[u8]) -> Result<Arc<ServerConfig>, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("certificate PEM: {e}"))?;
    if certs.is_empty() {
        return Err("certificate PEM: no CERTIFICATE block".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| format!("key PEM: {e}"))?;
    let mut cfg = ServerConfig::builder_with_provider(fips_provider()?)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| format!("tls versions: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("certificate/key: {e}"))?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    if !cfg.fips() {
        return Err("rustls config is not in FIPS mode (OpenSSL fips provider not active)".into());
    }
    Ok(Arc::new(cfg))
}

/// Accept loop with TLS: handshake (bounded by [`HANDSHAKE_TIMEOUT`]) then
/// the same HTTP/1.1 service as plain [`crate::serve`].
pub async fn serve_tls(
    app: Arc<App>,
    listener: TcpListener,
    cfg: Arc<ServerConfig>,
) -> std::io::Result<()> {
    spawn_gc(app.clone());
    let acceptor = TlsAcceptor::from(cfg);
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let permit = slots.clone().acquire_owned().await.expect("semaphore never closed");
        let (stream, peer) = listener.accept().await?;
        let app = app.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let ip: IpAddr = peer.ip();
            if let Ok(Ok(tls)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                serve_conn(app, ip, tls).await;
            }
            drop(permit);
        });
    }
}
