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
//! - [`Store::record_rename`] — explicit move; watcher moves that arrive
//!   as delete + create of identical non-empty bytes within
//!   [`Config::rename_window_ms`] (either order) are folded into one
//!   `rename` record automatically.
//!
//! Maintenance: [`Store::deleted`] / [`Store::restore_deleted`],
//! [`Store::thin`] ([`Retention`]), [`Store::purge_path`] /
//! [`Store::purge_version`]. Thin and purge rewrite index.log atomically
//! (temp + rename), then GC blobs nothing references. `seq` values stay
//! unique and increasing within a journal; a purged newest record's seq
//! may be reused after reopen, so callers must re-list after a purge.
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
    /// A sync merge version that did not survive verbatim (LWW loser,
    /// pre-merge side, conflict-copy source). Never changes the head.
    SyncLoser,
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
            Reason::SyncLoser => "sync-loser",
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
            "sync-loser" => Reason::SyncLoser,
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
    /// A delete and a create of identical non-empty bytes on two paths
    /// within this many ms (either order) are journaled as ONE `rename`.
    /// 0 disables the heuristic ([`Store::record_rename`] still works).
    pub rename_window_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config { max_file_bytes: 64 * 1024 * 1024, rename_window_ms: 10_000 }
    }
}

pub const HOUR_MS: u64 = 3_600_000;
pub const DAY_MS: u64 = 24 * HOUR_MS;

/// Retention thinning policy ([`Store::thin`]). Ages are `now - ts_ms`.
///
/// - the newest record of every LIVE path is always kept;
/// - anything older than the max age (notes vs attachments, plan §2
///   "separate attachment rule") is dropped — so a file deleted longer
///   ago than that disappears from "restore deleted";
/// - otherwise `delete` and `rename` records are always kept;
/// - versions younger than `keep_all_ms` are all kept;
/// - then the newest version per clock hour for `hourly_for_ms`;
/// - then the newest version per UTC day.
#[derive(Debug, Clone)]
pub struct Retention {
    pub keep_all_ms: u64,
    pub hourly_for_ms: u64,
    /// `.md` / `.canvas` files; `None` = keep daily versions forever.
    pub note_max_age_ms: Option<u64>,
    /// Everything else (attachments).
    pub attachment_max_age_ms: Option<u64>,
}

impl Default for Retention {
    fn default() -> Self {
        Retention {
            keep_all_ms: DAY_MS,
            hourly_for_ms: 7 * DAY_MS,
            note_max_age_ms: Some(365 * DAY_MS),
            attachment_max_age_ms: Some(14 * DAY_MS),
        }
    }
}

/// Notes (vs attachments) for [`Retention`].
pub fn is_note(path: &str) -> bool {
    let l = path.to_ascii_lowercase();
    l.ends_with(".md") || l.ends_with(".canvas")
}

/// What a journal rewrite (thin / purge) did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rewrite {
    /// Records removed from the journal.
    pub dropped: usize,
    /// Records kept but rewritten (purge only, see [`Store::purge_path`]).
    pub rewritten: usize,
    /// Blobs deleted from `objects/` because nothing references them.
    pub blobs_removed: usize,
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
    /// A rewrite would change live state (bug guard; nothing was written).
    Inconsistent(String),
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
            HistoryError::Inconsistent(s) => write!(f, "history rewrite refused: {s}"),
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
                    // create-then-delete order of a move: B appeared with
                    // these bytes moments ago -> one rename record A -> B
                    if size > 0 {
                        if let Some(to) = self.rename_target(path, &sha, now_ms) {
                            let e = self.append(Reason::Rename, &to, Some(sha), size, Some(path.to_string()), now_ms)?;
                            return Ok(Outcome::Stored(e));
                        }
                    }
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
        // the current file must be captured first: a skipped file (too large,
        // symlink, symlinked parent, …) would be destroyed with no copy, so
        // restore stops instead (CodeRabbit #238: history 566)
        if let Outcome::Skipped(s) = self.snapshot_before_overwrite(&e.path, now_ms)? {
            return Err(HistoryError::Inconsistent(format!(
                "{}: current file not captured ({s:?}); restore refused so it is not lost",
                e.path
            )));
        }
        // anchored write (no symlinked parent, CodeRabbit #238: history 567)
        // through a create_new temp file (never reuses or follows an existing
        // entry, history 571), then rename over the target
        let name = e.path.rsplit('/').next().unwrap_or_default();
        let tmp = format!(".{name}.osy-restore-{}-{seq}-{now_ms}.tmp", std::process::id());
        crate::vaultfs::write_atomic(&self.vault, &e.path, &data, &tmp).map_err(|err| {
            if err.kind() == io::ErrorKind::InvalidInput {
                HistoryError::InvalidPath(format!("{}: {err}", e.path))
            } else {
                HistoryError::Io(err)
            }
        })?;
        self.append(Reason::Restore, &e.path, Some(sha), data.len() as u64, None, now_ms)
    }

    /// Call right AFTER the app (or a watcher rename event carrying both
    /// paths) moved `from` to `to`. If the move replaced an existing `to`,
    /// call [`Store::snapshot_before_overwrite`] on `to` BEFORE moving.
    pub fn record_rename(&mut self, from: &str, to: &str, now_ms: u64) -> Result<Outcome, HistoryError> {
        validate_path(from)?;
        validate_path(to)?;
        if from == to {
            return self.record_external(to, now_ms);
        }
        match self.read_disk(to)? {
            Disk::Missing => Ok(Outcome::Missing),
            Disk::Skip(s) => Ok(Outcome::Skipped(s)),
            Disk::Bytes(b) => {
                let sha = self.cas.put(&b)?;
                let e = self.append(Reason::Rename, to, Some(sha), b.len() as u64, Some(from.to_string()), now_ms)?;
                Ok(Outcome::Stored(e))
            }
        }
    }

    // ---- restore deleted ---------------------------------------------

    /// Files whose last record is a tombstone ("restore deleted" list):
    /// the tombstone record of each, sorted by path.
    /// Keep a merge loser's bytes (not on disk) as a version of `path`
    /// (threat model D4: the losing side of a sync merge is ALWAYS in
    /// history). Idempotent per (path, bytes); the head is untouched.
    pub fn record_sync_loser(&mut self, path: &str, bytes: &[u8], now_ms: u64) -> Result<Outcome, HistoryError> {
        validate_path(path)?;
        if bytes.len() as u64 > self.cfg.max_file_bytes {
            return Ok(Outcome::Skipped(Skip::TooLarge(bytes.len() as u64)));
        }
        let sha = cas::sha_hex(bytes);
        let known = self.entries.iter().any(|e| e.path == path && e.sha.as_deref() == Some(sha.as_str()));
        if known && self.cas.contains(&sha) {
            return Ok(Outcome::Unchanged);
        }
        let put = self.cas.put(bytes)?;
        debug_assert_eq!(put, sha);
        Ok(Outcome::Stored(self.append(Reason::SyncLoser, path, Some(sha), bytes.len() as u64, None, now_ms)?))
    }

    pub fn deleted(&self) -> Vec<Entry> {
        self.heads
            .iter()
            .filter(|(_, h)| matches!(h, Head::Deleted { .. }))
            .filter_map(|(p, _)| self.entries.iter().rev().find(|e| e.path == *p && e.reason == Reason::Delete).cloned())
            .collect()
    }

    /// Bring a deleted file back with its newest stored bytes (the
    /// tombstone's own, else the newest earlier version).
    pub fn restore_deleted(&mut self, path: &str, now_ms: u64) -> Result<Entry, HistoryError> {
        validate_path(path)?;
        if !matches!(self.heads.get(path), Some(Head::Deleted { .. })) {
            return Err(HistoryError::NotFound(format!("{path} is not deleted")));
        }
        let seq = self
            .versions(path)
            .first()
            .map(|e| e.seq)
            .ok_or_else(|| HistoryError::NotFound(format!("{path}: no stored bytes")))?;
        self.restore(seq, now_ms)
    }

    // ---- retention / purge -------------------------------------------

    /// Apply `r` (see [`Retention`]) and GC unreferenced blobs. Never
    /// changes the head of a live path; refuses (writes nothing) if it
    /// would.
    pub fn thin(&mut self, now_ms: u64, r: &Retention) -> Result<Rewrite, HistoryError> {
        let keep = self.thin_plan(now_ms, r);
        // guard: live heads unchanged; a deleted path may only vanish
        let new_heads = replay(&keep);
        for (p, h) in &self.heads {
            match (h, new_heads.get(p)) {
                (a, Some(b)) if a == b => {}
                (Head::Deleted { .. }, None) => {}
                (_, got) => return Err(HistoryError::Inconsistent(format!("{p}: {h:?} -> {got:?}"))),
            }
        }
        if let Some(p) = new_heads.keys().find(|p| !self.heads.contains_key(*p)) {
            return Err(HistoryError::Inconsistent(format!("{p} would reappear")));
        }
        self.rewrite(keep, 0)
    }

    fn thin_plan(&self, now_ms: u64, r: &Retention) -> Vec<Entry> {
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut buckets: std::collections::BTreeSet<(&str, u8, u64)> = std::collections::BTreeSet::new();
        let mut keep: Vec<Entry> = Vec::with_capacity(self.entries.len());
        for e in self.entries.iter().rev() {
            let age = now_ms.saturating_sub(e.ts_ms);
            let newest = e.reason != Reason::SyncLoser && seen.insert(e.path.as_str());
            let max_age = if is_note(&e.path) { r.note_max_age_ms } else { r.attachment_max_age_ms };
            let kept = if newest && matches!(self.heads.get(&e.path), Some(Head::Live { .. })) {
                true
            } else if max_age.is_some_and(|m| age > m) {
                false
            } else if matches!(e.reason, Reason::Delete | Reason::Rename) || age <= r.keep_all_ms {
                true
            } else if age <= r.keep_all_ms.saturating_add(r.hourly_for_ms) {
                buckets.insert((e.path.as_str(), 1, e.ts_ms / HOUR_MS))
            } else {
                buckets.insert((e.path.as_str(), 2, e.ts_ms / DAY_MS))
            };
            if kept {
                keep.push(e.clone());
            }
        }
        keep.reverse();
        keep
    }

    /// Erase `path` from history: every record of it is dropped, a rename
    /// AWAY from it keeps the new path's bytes but forgets the old name
    /// (rewritten as `external`), then unreferenced blobs are deleted.
    /// Bytes survive only where another path/record holds identical
    /// content. Blob deletion is an unlink, not a secure wipe.
    pub fn purge_path(&mut self, path: &str) -> Result<Rewrite, HistoryError> {
        validate_path(path)?;
        let mut rewritten = 0;
        let mut keep = Vec::with_capacity(self.entries.len());
        for e in &self.entries {
            if e.path == path {
                continue;
            }
            let mut e = e.clone();
            if e.from.as_deref() == Some(path) {
                e.reason = Reason::External;
                e.from = None;
                rewritten += 1;
            }
            keep.push(e);
        }
        self.rewrite(keep, rewritten)
    }

    /// Erase the bytes of ONE record: a version is dropped (the path's
    /// head falls back to its previous record; the next capture re-reads
    /// disk), a tombstone loses its bytes but stays, a rename becomes a
    /// bytes-less tombstone of its old path.
    pub fn purge_version(&mut self, seq: u64) -> Result<Rewrite, HistoryError> {
        let i = self
            .entries
            .binary_search_by_key(&seq, |e| e.seq)
            .map_err(|_| HistoryError::NotFound(format!("seq {seq}")))?;
        let mut keep = self.entries.clone();
        let e = keep[i].clone();
        let rewritten = match e.reason {
            Reason::Delete if e.sha.is_none() => return Ok(Rewrite::default()),
            Reason::Delete => {
                keep[i].sha = None;
                keep[i].size = 0;
                1
            }
            Reason::Rename => {
                let from = e.from.clone().unwrap_or_default();
                keep[i] = Entry { seq, ts_ms: e.ts_ms, reason: Reason::Delete, sha: None, size: 0, path: from, from: None };
                1
            }
            _ => {
                let _dropped = keep.remove(i);
                0
            }
        };
        self.rewrite(keep, rewritten)
    }

    // ---- internals ---------------------------------------------------

    /// Atomically replace index.log with `keep` (temp + fsync + rename,
    /// so a crash leaves the old or the new journal, never a mix), then
    /// delete blobs no record references. Journal first, blobs second:
    /// a crash in between only leaves orphans for the next GC.
    fn rewrite(&mut self, keep: Vec<Entry>, rewritten: usize) -> Result<Rewrite, HistoryError> {
        let dropped = self.entries.len() - keep.len();
        if dropped > 0 || rewritten > 0 {
            let log_p = self.root.join("index.log");
            let tmp = self.root.join("index.log.tmp");
            let mut body = String::from(journal::HEADER);
            for e in &keep {
                body.push_str(&journal::encode(e));
            }
            {
                let mut f = fsx::create_private(&tmp)?;
                f.write_all(body.as_bytes())?;
                f.sync_all()?;
            }
            fs::rename(&tmp, &log_p)?;
            fsx::sync_dir(&self.root);
            self.journal = fsx::append_private(&log_p)?;
            self.heads = replay(&keep);
            self.entries = keep;
        }
        let referenced: std::collections::BTreeSet<&str> = self.entries.iter().filter_map(|e| e.sha.as_deref()).collect();
        let blobs_removed = self.cas.sweep(|sha| referenced.contains(sha))?;
        Ok(Rewrite { dropped, rewritten, blobs_removed })
    }

    /// delete-then-create: newest recent tombstone on another, still
    /// deleted path with the same bytes.
    fn rename_source(&self, to: &str, sha: &str, now_ms: u64) -> Option<String> {
        let w = self.cfg.rename_window_ms;
        if w == 0 {
            return None;
        }
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for e in self.entries.iter().rev() {
            if e.ts_ms.saturating_add(w) < now_ms {
                break;
            }
            if !seen.insert(e.path.as_str()) {
                continue; // not the latest record of its path
            }
            if e.reason == Reason::Delete
                && e.path != to
                && e.sha.as_deref() == Some(sha)
                && matches!(self.heads.get(&e.path), Some(Head::Deleted { last }) if last.as_deref() == Some(sha))
            {
                return Some(e.path.clone());
            }
        }
        None
    }

    /// create-then-delete: newest recent create of these bytes on another
    /// path that did not exist before that create.
    fn rename_target(&self, from: &str, sha: &str, now_ms: u64) -> Option<String> {
        let w = self.cfg.rename_window_ms;
        if w == 0 {
            return None;
        }
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for (i, e) in self.entries.iter().enumerate().rev() {
            if e.ts_ms.saturating_add(w) < now_ms {
                break;
            }
            if !seen.insert(e.path.as_str()) {
                continue;
            }
            if matches!(e.reason, Reason::External | Reason::OwnSave)
                && e.path != from
                && e.sha.as_deref() == Some(sha)
                && matches!(self.heads.get(&e.path), Some(Head::Live { sha: h, .. }) if h == sha)
                && self.absent_before(i, &e.path)
            {
                return Some(e.path.clone());
            }
        }
        None
    }

    /// Was `path` absent (never seen, deleted, or renamed away) just
    /// before record `idx`?
    fn absent_before(&self, idx: usize, path: &str) -> bool {
        for e in self.entries[..idx].iter().rev() {
            if e.from.as_deref() == Some(path) {
                return true;
            }
            if e.path == path {
                return e.reason == Reason::Delete;
            }
        }
        true
    }

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
        // delete-then-create order of a move: one rename record
        let creating = !matches!(self.heads.get(path), Some(Head::Live { .. }));
        if creating && !b.is_empty() && matches!(reason, Reason::External | Reason::OwnSave) {
            if let Some(from) = self.rename_source(path, &sha, now_ms) {
                let e = self.append(Reason::Rename, path, Some(sha), b.len() as u64, Some(from), now_ms)?;
                return Ok(Outcome::Stored(e));
            }
        }
        Ok(Outcome::Stored(self.append(reason, path, Some(sha), b.len() as u64, None, now_ms)?))
    }

    /// Anchored read (CodeRabbit #238: history 567): a symlinked parent or a
    /// file swapped while opening is "not a regular file", never followed.
    fn read_disk(&self, path: &str) -> Result<Disk, HistoryError> {
        match crate::vaultfs::read_regular(&self.vault, path, self.cfg.max_file_bytes) {
            Ok(crate::vaultfs::Found::Missing) => Ok(Disk::Missing),
            Ok(crate::vaultfs::Found::NotRegular) => Ok(Disk::Skip(Skip::NotRegularFile)),
            Ok(crate::vaultfs::Found::TooLarge(n)) => Ok(Disk::Skip(Skip::TooLarge(n))),
            Ok(crate::vaultfs::Found::Bytes(b, _)) => Ok(Disk::Bytes(b)),
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => Ok(Disk::Skip(Skip::NotRegularFile)),
            Err(e) => Err(e.into()),
        }
    }

    fn apply(&mut self, e: &Entry) {
        apply_head(&mut self.heads, e);
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

fn apply_head(heads: &mut BTreeMap<String, Head>, e: &Entry) {
    match e.reason {
        Reason::SyncLoser => {}
        Reason::Delete => {
            heads.insert(e.path.clone(), Head::Deleted { last: e.sha.clone() });
        }
        _ => {
            if let Some(from) = &e.from {
                heads.remove(from);
            }
            if let Some(sha) = &e.sha {
                heads.insert(e.path.clone(), Head::Live { sha: sha.clone(), size: e.size });
            }
        }
    }
}

/// Heads after replaying `entries` from an empty store.
fn replay(entries: &[Entry]) -> BTreeMap<String, Head> {
    let mut h = BTreeMap::new();
    for e in entries {
        apply_head(&mut h, e);
    }
    h
}
