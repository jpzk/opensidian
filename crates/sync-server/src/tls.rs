// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! TLS termination with rustls on the AWS-LC FIPS module (spec §5, plan M3).
//!
//! The provider is `rustls::crypto::default_fips_provider()` (aws-lc-rs built
//! with `fips` -> aws-lc-fips-sys, cmvp.md §7). Protocol: TLS 1.3 only, ALPN
//! `http/1.1`, no client auth (bearer tokens authenticate). [`server_config`]
//! refuses to return a config for which `ServerConfig::fips()` is false.

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

/// The FIPS crypto provider shared by server and (sync-core) client.
pub fn fips_provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::default_fips_provider())
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
    let mut cfg = ServerConfig::builder_with_provider(fips_provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| format!("tls versions: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("certificate/key: {e}"))?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    if !cfg.fips() {
        return Err("rustls config is not in FIPS mode (aws-lc-rs fips module unavailable)".into());
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
