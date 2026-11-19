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
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sync_crypto::fips::{FipsProvider, FipsSigningKey};
use sync_crypto::{CryptoError, CryptoProvider, SigAlg};
use tokio::sync::{Notify, Semaphore};

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
    /// Max size of one anonymous `env` object (envelopes are small, §4.5).
    pub max_env_object: u64,
    /// Total bytes the tokenless `env` namespace may hold (no quota owner).
    pub max_env_total: u64,
    /// Whole-body read deadline in seconds, after headers (slow-body bound).
    pub body_timeout_secs: u64,
    /// Request bodies buffered at once, in bytes, across all connections.
    pub max_inflight_body: u64,
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
            max_env_object: 64 * 1024,
            max_env_total: 256 * 1024 * 1024,
            body_timeout_secs: 60,
            max_inflight_body: 256 * 1024 * 1024,
        }
    }
}

/// Shared server state.
pub struct App {
    pub cfg: Config,
    pub store: Store,
    pub prov: FipsProvider,
    /// Destroy-receipt signing key (P-256, module DRBG, `receipt_key.p8`).
    receipt_key: FipsSigningKey,
    /// Its wire public key (65-byte SEC1), base64url.
    pub receipt_pub: String,
    /// Woken on every successful PUT (long-poll `GET /v1/list`).
    pub changed: Notify,
    limiter: Mutex<HashMap<IpAddr, (i64, u32)>>,
    /// Body-buffer budget in KiB units (`cfg.max_inflight_body`): a request
    /// holds its worst-case size from before the first body byte until the
    /// handler drops the bytes, so N slow uploads cannot pin N × max_object.
    pub body_budget: Arc<Semaphore>,
}

/// KiB units a body of at most `limit` bytes reserves from the budget.
pub fn body_units(limit: u64) -> u32 {
    u32::try_from(limit.div_ceil(1024).max(1)).unwrap_or(u32::MAX)
}

impl App {
    pub fn open(cfg: Config) -> Result<App, String> {
        let prov = FipsProvider::new().map_err(|e| format!("FIPS module unavailable: {e:?}"))?;
        let store = Store::open(&cfg.data).map_err(|e| format!("store: {}", e.detail))?;
        let receipt_key = load_or_create_receipt_key(&prov, &cfg.data)?;
        let receipt_pub = URL_SAFE_NO_PAD.encode(receipt_key.public_key());
        // Never below one maximal request, or a lone upload could never start.
        let budget = cfg.max_inflight_body.max(cfg.max_object).max(cfg.max_control as u64);
        Ok(App {
            cfg,
            store,
            prov,
            receipt_key,
            receipt_pub,
            changed: Notify::new(),
            limiter: Mutex::new(HashMap::new()),
            body_budget: Arc::new(Semaphore::new(body_units(budget) as usize)),
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

impl App {
    /// Sign a destroy receipt body; returns the wire receipt (body || sig).
    pub fn sign_receipt(&self, body: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let sig = self.prov.sign(&self.receipt_key, body)?;
        let mut out = body.to_vec();
        out.extend_from_slice(&sig);
        Ok(out)
    }
}

/// File name of the receipt key (PKCS#8 v1 DER, mode 0600) in the data dir.
pub const RECEIPT_KEY_FILE: &str = "receipt_key.p8";

/// Load the receipt key, or generate it with the module (first start).
/// The key is written in full (fsync'd) to a unique temp file and published
/// with `hard_link`, which fails if the name exists: a crash can never leave
/// a half-written key under the final name, and of two racing starts the
/// loser adopts the winner's key instead of writing its own.
fn load_or_create_receipt_key(prov: &FipsProvider, data: &std::path::Path) -> Result<FipsSigningKey, String> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let path = data.join(RECEIPT_KEY_FILE);
    let load = || -> Result<Option<FipsSigningKey>, String> {
        match std::fs::read(&path) {
            Ok(der) => {
                let der = zeroize::Zeroizing::new(der);
                FipsSigningKey::from_pkcs8(&der)
                    .map(Some)
                    .map_err(|_| format!("{}: not a P-256 PKCS#8 key", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    };
    if let Some(k) = load()? {
        return Ok(k);
    }
    let (key, _) = prov
        .generate_signing_key(SigAlg::EcdsaP256Sha256)
        .map_err(|e| format!("receipt key: {e:?}"))?;
    let der = key.to_pkcs8().map_err(|e| format!("receipt key: {e:?}"))?;
    let tmp = data.join(format!(".{RECEIPT_KEY_FILE}.{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&tmp); // stale leftover of a crashed start with our pid
    let staged = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(&der)?;
        f.sync_all()?;
        std::fs::hard_link(&tmp, &path)
    })();
    let _ = std::fs::remove_file(&tmp);
    match staged {
        Ok(()) => {
            std::fs::File::open(data)
                .and_then(|d| d.sync_all())
                .map_err(|e| format!("{}: {e}", data.display()))?;
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            load()?.ok_or_else(|| format!("{}: vanished during start", path.display()))
        }
        Err(e) => Err(format!("{}: {e}", path.display())),
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
