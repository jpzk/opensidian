// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Local file history (plan §2, docs/sync/spec.md §6).
//!
//! Layout, OUTSIDE the vault (checked on open):
//!
//! ```text
//! <app_data>/history/<vault_id>/          0700
//!   store.json                            {"format":1,"vault_id":"…"}
//!   index.log                             append-only journal (journal.rs)
//!   objects/<aa>/<sha256-hex>             plaintext blobs, 0600 (cas.rs)
//!   objects/tmp/                          in-flight blob writes
//! ```
//!
//! Invariant (the whole point of M1): before ANY overwrite or delete of a
//! vault file, its current on-disk bytes are in `objects/` and journaled.
//! Hooks, all synchronous, all "blob first, then journal line":
//!
//! - [`Store::snapshot_before_overwrite`] — call before the app (or a sync
//!   write, later) replaces a file; then [`Store::record_own_save`] after.
//! - [`Store::snapshot_before_delete`] — call before removing a file; stores
//!   the final bytes in a tombstone.
//! - [`Store::record_external`] — call for every watcher event (debounced):
//!   each on-disk state an external editor produces is stored as it
//!   appears, so the next external overwrite cannot lose it. An external
//!   delete becomes a tombstone pointing at the last captured bytes.
//! - [`Store::snapshot_idle`] + [`Debouncer`] — settle-time snapshots.
//!
//! Timestamps are caller-supplied unix milliseconds (testable, no clock).
//! Single writer per store: the per-vault engine lock (plan §1) is the
//! caller's job.

mod cas;
mod fsx;
pub mod idle;
mod journal;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use idle::Debouncer;

/// On-disk store format (store.json `format`, index.log header).
pub const STORE_FORMAT: u32 = 1;

/// Why a version was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// Bytes the app itself just wrote.
    OwnSave,
    /// Bytes found on disk immediately before an overwrite.
    PreWrite,
    /// Bytes an external editor produced (seen by the watcher).
    External,
    /// Tombstone: the file was deleted; `sha` = its final bytes if known.
    Delete,
    /// The file moved from `from` to `path` with these bytes.
    Rename,
    /// Settle-time snapshot after a quiet period.
    Idle,
    /// Bytes written back by a restore.
    Restore,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::OwnSave => "own-save",
            Reason::PreWrite => "pre-write",
            Reason::External => "external",
            Reason::Delete => "delete",
            Reason::Rename => "rename",
            Reason::Idle => "idle",
            Reason::Restore => "restore",
        }
    }

    pub fn parse(s: &str) -> Option<Reason> {
        Some(match s {
            "own-save" => Reason::OwnSave,
            "pre-write" => Reason::PreWrite,
            "external" => Reason::External,
            "delete" => Reason::Delete,
            "rename" => Reason::Rename,
            "idle" => Reason::Idle,
            "restore" => Reason::Restore,
            _ => return None,
        })
    }
}

/// One journal record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub seq: u64,
    pub ts_ms: u64,
    pub reason: Reason,
    /// sha256 hex of the blob; `None` only for a tombstone whose bytes
    /// were never captured.
    pub sha: Option<String>,
    pub size: u64,
    /// Vault-relative path, `/` separated.
    pub path: String,
    /// Previous path (renames only).
    pub from: Option<String>,
}

/// Last journaled state of a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    Live { sha: String, size: u64 },
    Deleted { last: Option<String> },
}

/// Result of a hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A new journal record was written.
    Stored(Entry),
    /// On-disk bytes equal the last stored version; nothing to do.
    Unchanged,
    /// No file on disk and nothing to record.
    Missing,
    /// Not captured (see [`Skip`]).
    Skipped(Skip),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Symlink, directory, device… — history only tracks regular files.
    NotRegularFile,
    /// Larger than [`Config::max_file_bytes`] (size given).
    TooLarge(u64),
}

#[derive(Debug, Clone)]
pub struct Config {
    /// Files above this size are not copied into history.
    pub max_file_bytes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config { max_file_bytes: 64 * 1024 * 1024 }
    }
}

#[derive(Debug)]
pub enum HistoryError {
    Io(io::Error),
    InvalidVaultId(String),
    InvalidPath(String),
    /// The history root would live inside the vault (would sync/leak itself).
    StoreInsideVault,
    /// store.json missing fields, other vault id or unknown format.
    Meta(String),
    /// Malformed journal line (1-based line number).
    Corrupt { line: usize, msg: String },
    /// Blob content does not hash to its name.
    BlobMismatch(String),
    NotFound(String),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HistoryError::Io(e) => write!(f, "history i/o: {e}"),
            HistoryError::InvalidVaultId(s) => write!(f, "invalid vault id: {s:?}"),
            HistoryError::InvalidPath(s) => write!(f, "invalid vault path: {s:?}"),
            HistoryError::StoreInsideVault => write!(f, "history store must not be inside the vault"),
            HistoryError::Meta(s) => write!(f, "history store.json: {s}"),
            HistoryError::Corrupt { line, msg } => write!(f, "history index.log line {line}: {msg}"),
            HistoryError::BlobMismatch(s) => write!(f, "history blob {s} does not match its hash"),
            HistoryError::NotFound(s) => write!(f, "history: not found: {s}"),
        }
    }
}

impl std::error::Error for HistoryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HistoryError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for HistoryError {
    fn from(e: io::Error) -> Self {
        HistoryError::Io(e)
    }
}

/// Vault ids name a directory: `[A-Za-z0-9_-]{1,64}`.
pub fn validate_vault_id(id: &str) -> Result<(), HistoryError> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-');
    if ok {
        Ok(())
    } else {
        Err(HistoryError::InvalidVaultId(id.to_string()))
    }
}

/// Vault-relative paths: `/` separated, no empty/`.`/`..` components, no
/// leading `/`, no `\`, no NUL, no drive prefix.
pub fn validate_path(p: &str) -> Result<(), HistoryError> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.contains('\\')
        || p.contains('\0')
        || p.split('/').any(|c| c.is_empty() || c == "." || c == "..")
        || p.split('/').next().is_some_and(|c| c.len() == 2 && c.ends_with(':'));
    if bad {
        Err(HistoryError::InvalidPath(p.to_string()))
    } else {
        Ok(())
    }
}

/// `<app_data>/history/<vault_id>`.
pub fn store_root(app_data: &Path, vault_id: &str) -> Result<PathBuf, HistoryError> {
    validate_vault_id(vault_id)?;
    Ok(app_data.join("history").join(vault_id))
}

/// Canonical form of a possibly not-yet-existing path: canonicalize the
/// deepest existing ancestor, re-append the rest (no `..` allowed in it).
fn resolve(p: &Path) -> Result<PathBuf, HistoryError> {
    let mut base = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if base.exists() {
            let mut out = fs::canonicalize(&base)?;
            for c in rest.iter().rev() {
                out.push(c);
            }
            return Ok(out);
        }
        match (base.file_name(), base.parent()) {
            (Some(name), Some(parent)) => {
                rest.push(name.to_os_string());
                base = if parent.as_os_str().is_empty() { PathBuf::from(".") } else { parent.to_path_buf() };
            }
            _ => return Err(HistoryError::NotFound(format!("{}", p.display()))),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Meta {
    format: u32,
    vault_id: String,
}

enum Disk {
    Bytes(Vec<u8>),
    Missing,
    Skip(Skip),
}

/// The history store of one vault.
pub struct Store {
    root: PathBuf,
    vault: PathBuf,
    cfg: Config,
    cas: cas::Cas,
    journal: File,
    entries: Vec<Entry>,
    heads: BTreeMap<String, Head>,
    next_seq: u64,
}

impl Store {
    /// Open (creating) the store for `vault_id` under `app_data`, for the
    /// vault rooted at `vault_root` (must exist). Fails if the store root
    /// resolves inside the vault — checked BEFORE anything is created.
    pub fn open(app_data: &Path, vault_id: &str, vault_root: &Path, cfg: Config) -> Result<Store, HistoryError> {
        let root = store_root(app_data, vault_id)?;
        if app_data.components().any(|c| c == Component::ParentDir) {
            return Err(HistoryError::InvalidPath(app_data.display().to_string()));
        }
        let vault = fs::canonicalize(vault_root)?;
        let root_c = resolve(&root)?;
        if root_c.starts_with(&vault) {
            return Err(HistoryError::StoreInsideVault);
        }
        fsx::mkdir_private(&root)?;
        let root = fs::canonicalize(&root)?;

        let meta_p = root.join("store.json");
        match fs::read(&meta_p) {
            Ok(b) => {
                let m: Meta = serde_json::from_slice(&b).map_err(|e| HistoryError::Meta(e.to_string()))?;
                if m.format != STORE_FORMAT {
                    return Err(HistoryError::Meta(format!("unsupported format {}", m.format)));
                }
                if m.vault_id != vault_id {
                    return Err(HistoryError::Meta(format!("belongs to vault {:?}", m.vault_id)));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let m = Meta { format: STORE_FORMAT, vault_id: vault_id.to_string() };
                let body = serde_json::to_vec(&m).map_err(|e| HistoryError::Meta(e.to_string()))?;
                let tmp = root.join("store.json.tmp");
                {
                    let mut f = fsx::create_private(&tmp)?;
                    f.write_all(&body)?;
                    f.sync_all()?;
                }
                fs::rename(&tmp, &meta_p)?;
                fsx::sync_dir(&root);
            }
            Err(e) => return Err(e.into()),
        }

        let cas = cas::Cas::open(root.join("objects"))?;

        let log_p = root.join("index.log");
        let mut raw = Vec::new();
        match File::open(&log_p) {
            Ok(mut f) => {
                f.read_to_end(&mut raw)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let text = std::str::from_utf8(&raw).map_err(|_| HistoryError::Corrupt { line: 0, msg: "not utf-8".into() })?;
        let (entries, valid) = journal::parse(text)?;
        if valid == 0 {
            let mut f = fsx::create_private(&log_p)?;
            f.write_all(journal::HEADER.as_bytes())?;
            f.sync_all()?;
        } else if valid < raw.len() {
            // torn final line from a crash: drop it, keep everything before
            let f = fs::OpenOptions::new().write(true).open(&log_p)?;
            f.set_len(valid as u64)?;
            f.sync_all()?;
        }
        let journal = fsx::append_private(&log_p)?;

        let mut st = Store {
            root,
            vault,
            cfg,
            cas,
            journal,
            entries: Vec::new(),
            heads: BTreeMap::new(),
            next_seq: 1,
        };
        for e in entries {
            st.apply(&e);
            st.next_seq = e.seq + 1;
            st.entries.push(e);
        }
        Ok(st)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn vault_root(&self) -> &Path {
        &self.vault
    }

    /// Every journal record, oldest first.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Last journaled state of `path`.
    pub fn head(&self, path: &str) -> Option<&Head> {
        self.heads.get(path)
    }

    /// Stored versions of `path` (records with bytes), newest first.
    pub fn versions(&self, path: &str) -> Vec<Entry> {
        self.entries.iter().rev().filter(|e| e.path == path && e.sha.is_some()).cloned().collect()
    }

    /// Bytes of a blob, hash-verified.
    pub fn read_blob(&self, sha: &str) -> Result<Vec<u8>, HistoryError> {
        if !journal::is_sha_hex(sha) {
            return Err(HistoryError::NotFound(sha.to_string()));
        }
        self.cas.get(sha)
    }

    pub fn entry(&self, seq: u64) -> Option<&Entry> {
        self.entries.binary_search_by_key(&seq, |e| e.seq).ok().map(|i| &self.entries[i])
    }

    /// Bytes of the version recorded by journal record `seq`.
    pub fn read_version(&self, seq: u64) -> Result<Vec<u8>, HistoryError> {
        let e = self.entry(seq).ok_or_else(|| HistoryError::NotFound(format!("seq {seq}")))?;
        let sha = e.sha.as_deref().ok_or_else(|| HistoryError::NotFound(format!("seq {seq} has no bytes")))?;
        self.cas.get(sha)
    }

    // ---- hooks -------------------------------------------------------

    /// Call immediately BEFORE overwriting `path` (own save, restore, sync
    /// write). Guarantees the current on-disk bytes are stored.
    pub fn snapshot_before_overwrite(&mut self, path: &str, now_ms: u64) -> Result<Outcome, HistoryError> {
        self.capture(path, Reason::PreWrite, now_ms)
    }

    /// Call right AFTER the app wrote `path`: stores the new bytes so the
    /// watcher echo of our own write is recognised (not "external").
    pub fn record_own_save(&mut self, path: &str, now_ms: u64) -> Result<Outcome, HistoryError> {
        self.capture(path, Reason::OwnSave, now_ms)
    }

    /// Settle-time snapshot (see [`Debouncer`]).
    pub fn snapshot_idle(&mut self, path: &str, now_ms: u64) -> Result<Outcome, HistoryError> {
        self.capture(path, Reason::Idle, now_ms)
    }

    /// Watcher event for `path` (create/modify/remove by anyone). New
    /// bytes are stored as `external`; a vanished live file becomes a
    /// tombstone pointing at its last captured bytes.
    pub fn record_external(&mut self, path: &str, now_ms: u64) -> Result<Outcome, HistoryError> {
        validate_path(path)?;
        match self.read_disk(path)? {
            Disk::Missing => match self.heads.get(path) {
                Some(Head::Live { sha, size }) => {
                    let (sha, size) = (sha.clone(), *size);
                    Ok(Outcome::Stored(self.append(Reason::Delete, path, Some(sha), size, None, now_ms)?))
                }
                _ => Ok(Outcome::Missing),
            },
            Disk::Skip(s) => Ok(Outcome::Skipped(s)),
            Disk::Bytes(b) => self.store_bytes(path, &b, Reason::External, now_ms),
        }
    }

    /// Call immediately BEFORE deleting `path`. Stores the final bytes and
    /// writes a tombstone (always, so "restore deleted" can list it).
    pub fn snapshot_before_delete(&mut self, path: &str, now_ms: u64) -> Result<Outcome, HistoryError> {
        validate_path(path)?;
        let last = match self.heads.get(path) {
            Some(Head::Live { sha, size }) => Some((sha.clone(), *size)),
            _ => None,
        };
        let (sha, size) = match self.read_disk(path)? {
            Disk::Bytes(b) => {
                let sha = self.cas.put(&b)?;
                (Some(sha), b.len() as u64)
            }
            Disk::Skip(Skip::NotRegularFile) => return Ok(Outcome::Skipped(Skip::NotRegularFile)),
            // already gone: tombstone the last captured bytes, if any
            Disk::Missing => match last {
                Some((s, z)) => (Some(s), z),
                None => return Ok(Outcome::Missing),
            },
            // too large to copy: tombstone without bytes (older versions,
            // if any, stay listed under versions())
            Disk::Skip(Skip::TooLarge(_)) => (None, 0),
        };
        Ok(Outcome::Stored(self.append(Reason::Delete, path, sha, size, None, now_ms)?))
    }

    /// Write the bytes of record `seq` back to its path (also restores a
    /// deleted file from its tombstone). Snapshots whatever is on disk
    /// first, writes via temp file + rename next to the target, then
    /// journals a `restore` record. Returns that record.
    pub fn restore(&mut self, seq: u64, now_ms: u64) -> Result<Entry, HistoryError> {
        let e = self.entry(seq).cloned().ok_or_else(|| HistoryError::NotFound(format!("seq {seq}")))?;
        let sha = e.sha.clone().ok_or_else(|| HistoryError::NotFound(format!("seq {seq} has no bytes")))?;
        let data = self.cas.get(&sha)?;
        validate_path(&e.path)?;
        self.snapshot_before_overwrite(&e.path, now_ms)?;
        let dst = self.vault.join(&e.path);
        let dir = dst.parent().ok_or_else(|| HistoryError::InvalidPath(e.path.clone()))?.to_path_buf();
        fs::create_dir_all(&dir)?;
        let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let tmp = dir.join(format!(".{name}.osy-restore.tmp"));
        {
            let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
            f.write_all(&data)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &dst)?;
        fsx::sync_dir(&dir);
        self.append(Reason::Restore, &e.path, Some(sha), data.len() as u64, None, now_ms)
    }

    // ---- internals ---------------------------------------------------

    fn capture(&mut self, path: &str, reason: Reason, now_ms: u64) -> Result<Outcome, HistoryError> {
        validate_path(path)?;
        match self.read_disk(path)? {
            Disk::Missing => Ok(Outcome::Missing),
            Disk::Skip(s) => Ok(Outcome::Skipped(s)),
            Disk::Bytes(b) => self.store_bytes(path, &b, reason, now_ms),
        }
    }

    fn store_bytes(&mut self, path: &str, b: &[u8], reason: Reason, now_ms: u64) -> Result<Outcome, HistoryError> {
        let sha = cas::sha_hex(b);
        if let Some(Head::Live { sha: h, .. }) = self.heads.get(path) {
            if *h == sha && self.cas.contains(&sha) {
                return Ok(Outcome::Unchanged);
            }
        }
        let put = self.cas.put(b)?;
        debug_assert_eq!(put, sha);
        Ok(Outcome::Stored(self.append(reason, path, Some(sha), b.len() as u64, None, now_ms)?))
    }

    fn read_disk(&self, path: &str) -> Result<Disk, HistoryError> {
        let p = self.vault.join(path);
        let md = match fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Disk::Missing),
            Err(e) => return Err(e.into()),
        };
        if !md.file_type().is_file() {
            return Ok(Disk::Skip(Skip::NotRegularFile));
        }
        if md.len() > self.cfg.max_file_bytes {
            return Ok(Disk::Skip(Skip::TooLarge(md.len())));
        }
        match fs::read(&p) {
            Ok(b) => Ok(Disk::Bytes(b)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Disk::Missing),
            Err(e) => Err(e.into()),
        }
    }

    fn apply(&mut self, e: &Entry) {
        match e.reason {
            Reason::Delete => {
                self.heads.insert(e.path.clone(), Head::Deleted { last: e.sha.clone() });
            }
            _ => {
                if let Some(from) = &e.from {
                    self.heads.remove(from);
                }
                if let Some(sha) = &e.sha {
                    self.heads.insert(e.path.clone(), Head::Live { sha: sha.clone(), size: e.size });
                }
            }
        }
    }

    /// Journal one record (blob must already be stored) and fsync.
    fn append(
        &mut self,
        reason: Reason,
        path: &str,
        sha: Option<String>,
        size: u64,
        from: Option<String>,
        now_ms: u64,
    ) -> Result<Entry, HistoryError> {
        let e = Entry { seq: self.next_seq, ts_ms: now_ms, reason, sha, size, path: path.to_string(), from };
        self.journal.write_all(journal::encode(&e).as_bytes())?;
        self.journal.sync_data()?;
        self.apply(&e);
        self.next_seq += 1;
        self.entries.push(e.clone());
        Ok(e)
    }
}
