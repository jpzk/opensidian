// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! The `SyncApi` surface shared by the CLI, the daemon and the Tauri
//! backend. Two implementations: [`crate::local::LocalEngine`] (runs the
//! sync-core engine in-process) and [`crate::ipc::IpcClient`] (forwards to a
//! running `opensidian-syncd daemon` over its unix socket). The wire form is
//! one JSON object per line ([`Request`] → [`Response`]).

use std::fmt;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// Stable machine code: `not-configured`, `busy`, `not-member` (device not on the vault roster),
    /// `server`, `io`, `bad-request`, `denied`, `history`, `vault`, `engine`,
    /// `offline` (the net gate refused the connection), `fips` (the OpenSSL FIPS
    /// provider is not active: sync refuses to start, cmvp.md §6).
    pub code: String,
    pub message: String,
}

impl ApiError {
    pub fn new(code: &str, message: impl Into<String>) -> ApiError {
        ApiError { code: code.to_string(), message: message.into() }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub vault_id: String,
    pub vault_dir: String,
    pub url: String,
    pub device_id: String,
    pub device_name: String,
    /// "auto-merge" | "conflict-file"
    pub conflict_mode: String,
    /// The server answered and the cfg chain verified.
    pub server_ok: bool,
    pub server_error: Option<String>,
    pub cfg_gen: Option<u32>,
    pub vk_epoch: Option<u32>,
    pub active_devices: Option<usize>,
    pub head: Option<String>,
    pub commits: usize,
    pub tips: usize,
    pub files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SyncSummary {
    /// The local commit made from the vault folder, if anything changed.
    pub committed: Option<String>,
    pub pulled: usize,
    pub pushed: usize,
    pub rejected: usize,
    /// One line per rejected object: replica index, object id, reason.
    #[serde(default)]
    pub rejected_detail: Vec<String>,
    pub replica_errors: Vec<String>,
    pub losers: usize,
    /// Files written into / removed from the vault folder.
    pub written: usize,
    pub removed: usize,
    /// Changed on disk while syncing: left alone, picked up next pass.
    pub skipped: usize,
    pub head: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub seq: u64,
    pub ts_ms: u64,
    pub reason: String,
    pub size: u64,
    pub path: String,
    pub sha: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub ns: String,
    pub destroyed_at: i64,
    pub objects: u64,
    pub bytes: u64,
    /// The server purge succeeded and the receipt verified, but removing the
    /// local keys failed (the caller must surface it; the vault is gone).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleanup_error: Option<String>,
}

/// Result of a burn-and-move (`opensidian-syncd move`, the app's "Move to a
/// new vault", spec §4.8). The new words are NOT in here (shown once by the
/// caller, kept in the new backup entry).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Moved {
    pub old_vault_id: String,
    pub new_vault_id: String,
    /// files in the new vault's head after seeding
    pub files: usize,
    /// file-history journal lines carried over to the new vault
    pub history_entries: usize,
    /// the old vault's destroy receipt (K_destroy proof accepted, verified)
    pub destroyed: Receipt,
    /// the old state dir, renamed aside (engine objects of a vault that no
    /// longer exists + its history copy; delete it when you like)
    pub burned_state: String,
}

pub trait SyncApi {
    fn status(&self) -> ApiResult<Status>;
    fn sync_now(&self) -> ApiResult<SyncSummary>;
    /// Versions of one vault-relative path, newest first.
    fn history(&self, path: &str) -> ApiResult<Vec<HistoryItem>>;
    /// Bytes of one history record (diff view).
    fn history_read(&self, seq: u64) -> ApiResult<Vec<u8>>;
    /// Write record `seq` back to its path (the next sync commits it).
    fn restore(&self, seq: u64) -> ApiResult<HistoryItem>;
    /// Destroy the vault on the server with the words (K_destroy proof).
    fn destroy(&self, words: &str) -> ApiResult<Receipt>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Request {
    Status,
    SyncNow,
    History { path: String },
    HistoryRead { seq: u64 },
    Restore { seq: u64 },
    Destroy { words: String },
}

impl fmt::Display for Request {
    /// Never prints the words.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Request::Destroy { .. } => f.write_str("destroy"),
            r => write!(f, "{}", serde_json::to_string(r).unwrap_or_default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Response {
    Ok(serde_json::Value),
    Err(ApiError),
}

/// Run one request against any implementation (the daemon side).
pub fn dispatch(api: &dyn SyncApi, req: Request) -> Response {
    fn wrap<T: Serialize>(r: ApiResult<T>) -> Response {
        match r {
            Ok(v) => match serde_json::to_value(v) {
                Ok(v) => Response::Ok(v),
                Err(e) => Response::Err(ApiError::new("io", e.to_string())),
            },
            Err(e) => Response::Err(e),
        }
    }
    match req {
        Request::Status => wrap(api.status()),
        Request::SyncNow => wrap(api.sync_now()),
        Request::History { path } => wrap(api.history(&path)),
        // base64 string: ~1.33x the bytes instead of ~3.6x as a JSON array
        Request::HistoryRead { seq } => wrap(api.history_read(seq).map(|b| B64.encode(b))),
        Request::Restore { seq } => wrap(api.restore(seq)),
        Request::Destroy { words } => wrap(api.destroy(&words)),
    }
}

/// Decode the `history-read` reply (see [`dispatch`]).
pub fn decode_history_bytes(s: &str) -> ApiResult<Vec<u8>> {
    B64.decode(s.as_bytes()).map_err(|e| ApiError::new("io", format!("daemon history bytes: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Blob;
    impl SyncApi for Blob {
        fn status(&self) -> ApiResult<Status> {
            Err(ApiError::new("io", "unused"))
        }
        fn sync_now(&self) -> ApiResult<SyncSummary> {
            Err(ApiError::new("io", "unused"))
        }
        fn history(&self, _: &str) -> ApiResult<Vec<HistoryItem>> {
            Ok(Vec::new())
        }
        fn history_read(&self, _: u64) -> ApiResult<Vec<u8>> {
            Ok((0..=255u8).cycle().take(3000).collect())
        }
        fn restore(&self, _: u64) -> ApiResult<HistoryItem> {
            Err(ApiError::new("io", "unused"))
        }
        fn destroy(&self, _: &str) -> ApiResult<Receipt> {
            Err(ApiError::new("io", "unused"))
        }
    }

    #[test]
    fn history_bytes_travel_as_one_base64_string() {
        let Response::Ok(v) = dispatch(&Blob, Request::HistoryRead { seq: 1 }) else { panic!("err") };
        let s = v.as_str().expect("a string, not a number array");
        assert!(s.len() <= 3000 * 4 / 3 + 4);
        let line = serde_json::to_string(&Response::Ok(v.clone())).unwrap();
        assert!(line.len() < 4100, "{}", line.len());
        assert_eq!(decode_history_bytes(s).unwrap(), Blob.history_read(1).unwrap());
    }
}
