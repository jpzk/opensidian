// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! opensidian-sync-server — the self-host storage server (plan M3, spec §5).
//! A dumb write-once store: it keeps ciphertext containers, validates only
//! their 56-byte header, and holds no key. Library half: [`App`] + [`serve`];
//! the binary (`src/main.rs`) adds the admin CLI.
#![forbid(unsafe_code)]

pub mod http;
pub mod store;
pub mod tls;

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sync_crypto::fips::FipsProvider;
use sync_crypto::{CryptoError, CryptoProvider};
use tokio::sync::Notify;

pub use http::serve;
pub use store::Store;
pub use tls::{serve_tls, server_config};

/// Server configuration (CLI flags in `main.rs`).
#[derive(Debug, Clone)]
pub struct Config {
    pub data: PathBuf,
    /// Max object (container) size in bytes.
    pub max_object: u64,
    /// Max JSON control body in bytes.
    pub max_control: usize,
    /// Soft-delete retention window in seconds (default 30 d).
    pub retention_secs: i64,
    /// Lifetime of `pair` objects in seconds (spec §4.6: 15 min).
    pub pair_ttl_secs: i64,
    /// Requests per minute per client IP on the tokenless paths (`env`,
    /// `pair` reads, `enroll`).
    pub anon_rate_per_min: u32,
}

impl Config {
    pub fn new(data: PathBuf) -> Config {
        Config {
            data,
            max_object: sync_proto::api::DEFAULT_MAX_OBJECT,
            max_control: sync_proto::api::DEFAULT_MAX_CONTROL_BODY,
            retention_secs: 30 * 24 * 3600,
            pair_ttl_secs: 15 * 60,
            anon_rate_per_min: 120,
        }
    }
}

/// Shared server state.
pub struct App {
    pub cfg: Config,
    pub store: Store,
    pub prov: FipsProvider,
    /// Woken on every successful PUT (long-poll `GET /v1/list`).
    pub changed: Notify,
    limiter: Mutex<HashMap<IpAddr, (i64, u32)>>,
}

impl App {
    pub fn open(cfg: Config) -> Result<App, String> {
        let prov = FipsProvider::new().map_err(|e| format!("FIPS module unavailable: {e:?}"))?;
        let store = Store::open(&cfg.data).map_err(|e| format!("store: {}", e.detail))?;
        Ok(App {
            cfg,
            store,
            prov,
            changed: Notify::new(),
            limiter: Mutex::new(HashMap::new()),
        })
    }

    /// Fixed-window per-IP limiter for the tokenless paths.
    pub fn allow_anon(&self, ip: IpAddr) -> bool {
        let minute = now_unix() / 60;
        let mut m = self.limiter.lock().unwrap_or_else(|p| p.into_inner());
        if m.len() > 65_536 {
            m.retain(|_, (w, _)| *w == minute);
        }
        let e = m.entry(ip).or_insert((minute, 0));
        if e.0 != minute {
            *e = (minute, 0);
        }
        e.1 += 1;
        e.1 <= self.cfg.anon_rate_per_min
    }

    /// SHA-256 of a decoded bearer token (the only form the server stores).
    pub fn token_hash(&self, raw: &[u8; 32]) -> [u8; 32] {
        self.prov.sha256(raw)
    }

    /// Fresh 32-byte token from the module DRBG: (wire string, hash).
    pub fn new_token(&self) -> Result<(String, [u8; 32]), CryptoError> {
        let mut raw = [0u8; 32];
        self.prov.random(&mut raw)?;
        Ok((URL_SAFE_NO_PAD.encode(raw), self.token_hash(&raw)))
    }

    /// Admin `vault create`: a one-time enrollment token, valid `ttl_secs`.
    pub fn create_enrollment(&self, quota: Option<u64>, ttl_secs: i64) -> Result<String, String> {
        let (tok, hash) = self.new_token().map_err(|e| format!("drbg: {e:?}"))?;
        self.store
            .add_enrollment(&hash, quota, now_unix() + ttl_secs)
            .map_err(|e| e.detail.to_string())?;
        Ok(tok)
    }
}

/// Decode a wire token (base64url, no padding, exactly 32 bytes).
pub fn decode_token(s: &str) -> Option<[u8; 32]> {
    let v = URL_SAFE_NO_PAD.decode(s.as_bytes()).ok()?;
    v.try_into().ok()
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Effective uid of this process, read without libc: `/proc/self` is owned by
/// the euid. `None` off Linux or if procfs is missing.
pub fn effective_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}
