// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! The `SyncApi` surface shared by the CLI, the daemon and the Tauri
//! backend. Two implementations: [`crate::local::LocalEngine`] (runs the
//! sync-core engine in-process) and [`crate::ipc::IpcClient`] (forwards to a
//! running `opensidian-syncd daemon` over its unix socket). The wire form is
//! one JSON object per line ([`Request`] → [`Response`]).

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    /// Stable machine code: `not-configured`, `busy`, `revoked`, `need-words`,
    /// `server`, `io`, `bad-request`, `denied`, `history`, `vault`, `engine`.
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
}

pub trait SyncApi {
    fn status(&self) -> ApiResult<Status>;
    fn sync_now(&self) -> ApiResult<SyncSummary>;
    /// Versions of one vault-relative path, oldest first.
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
        Request::HistoryRead { seq } => wrap(api.history_read(seq)),
        Request::Restore { seq } => wrap(api.restore(seq)),
        Request::Destroy { words } => wrap(api.destroy(&words)),
    }
}
