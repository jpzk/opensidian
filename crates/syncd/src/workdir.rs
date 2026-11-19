// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! The vault folder as seen by the engine: scan it into (path → sha256),
//! classify paths, and write the merged head tree back atomically. Only
//! regular files are synced; symlinks are never followed.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use sync_core::sync_crypto::CryptoProvider;
use sync_core::sync_proto::batch::Class;
use sync_core::vaultfs::{self, Found};

/// Directory names never synced (anywhere in the tree).
pub const SKIP_DIRS: &[&str] = &[".git", ".trash", ".opensidian-sync"];
/// Prefix of our own in-flight temp files.
pub const TMP_PREFIX: &str = ".osy-tmp-";
/// The app's settings folder: `*.json` inside it merge as settings JSON.
pub const SETTINGS_DIR: &str = ".obsidian/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskFile {
    pub sha256: [u8; 32],
    pub size: u64,
    pub mtime_ms: i64,
}

pub fn class_of(path: &str) -> Class {
    if path.starts_with(SETTINGS_DIR) && path.ends_with(".json") {
        Class::SettingsJson
    } else if path.ends_with(".md") {
        Class::Markdown
    } else {
        Class::Other
    }
}

pub fn abs(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for c in rel.split('/') {
        p.push(c);
    }
    p
}

fn mtime_ms(md: &fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Every regular file under `root` (vault-relative, `/` separated). Names
/// that are not UTF-8 or fail path validation are skipped and counted.
pub fn scan<P: CryptoProvider + ?Sized>(p: &P, root: &Path) -> io::Result<(BTreeMap<String, DiskFile>, usize)> {
    let mut out = BTreeMap::new();
    let mut skipped = 0usize;
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        for ent in fs::read_dir(&dir)? {
            let ent = ent?;
            let Some(name) = ent.file_name().to_str().map(str::to_string) else {
                skipped += 1;
                continue;
            };
            let ft = ent.file_type()?;
            let rel = format!("{prefix}{name}");
            if ft.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push((ent.path(), format!("{rel}/")));
                }
            } else if ft.is_file() {
                if name.starts_with(TMP_PREFIX) {
                    continue;
                }
                if sync_core::history::validate_path(&rel).is_err() {
                    skipped += 1;
                    continue;
                }
                // read the file the scan saw, anchored (no symlinked parent,
                // handle == lstat inode); it may vanish or turn into a link
                let (bytes, md) = match vaultfs::read_regular(root, &rel, u64::MAX)? {
                    Found::Bytes(b, md) => (b, md),
                    // enumerated as a regular file, gone or no longer regular
                    // now: abort the scan for a retry, never read it as a delete
                    _ => return Err(io::Error::other(format!("{rel}: changed while scanning, retry"))),
                };
                out.insert(rel, DiskFile { sha256: p.sha256(&bytes), size: bytes.len() as u64, mtime_ms: mtime_ms(&md) });
            }
        }
    }
    Ok((out, skipped))
}

/// Does the disk still hold `want` at `rel` (`None` = no regular file)?
/// A path vaultfs refuses (symlinked parent, unsafe segment) is `false`:
/// a per-path skip for the caller, not an error that aborts the pass;
/// genuine read failures still propagate.
pub fn disk_is<P: CryptoProvider + ?Sized>(p: &P, root: &Path, rel: &str, want: Option<[u8; 32]>) -> io::Result<bool> {
    match vaultfs::read_regular(root, rel, u64::MAX) {
        Ok(Found::Bytes(b, _)) => Ok(want == Some(p.sha256(&b))),
        Ok(_) => Ok(want.is_none()),
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => Ok(false),
        Err(e) => Err(e),
    }
}

/// Bytes of one file, only if they still hash to `want` (the scan result):
/// a file edited between scan and read is an error, so a commit never
/// pairs the scanned hash with other content.
pub fn read_expect<P: CryptoProvider + ?Sized>(p: &P, root: &Path, rel: &str, want: &[u8; 32]) -> io::Result<Vec<u8>> {
    match vaultfs::read_regular(root, rel, u64::MAX)? {
        Found::Bytes(b, _) if p.sha256(&b) == *want => Ok(b),
        _ => Err(io::Error::other(format!("{rel}: changed while syncing, retry"))),
    }
}

/// Temp file next to the target + rename; parents are created as real
/// directories and a symlinked parent is refused (anchored to the vault).
pub fn write_atomic<P: CryptoProvider + ?Sized>(p: &P, root: &Path, rel: &str, bytes: &[u8]) -> io::Result<()> {
    let mut r = [0u8; 8];
    p.random(&mut r).map_err(|e| io::Error::other(e.to_string()))?;
    let tmp = format!("{TMP_PREFIX}{}", sync_core::sync_proto::hex_lower(&r));
    vaultfs::write_atomic(root, rel, bytes, &tmp).map(|_| ())
}

/// Unlink one file (anchored; a symlink is unlinked, never its target).
pub fn remove(root: &Path, rel: &str) -> io::Result<()> {
    vaultfs::remove(root, rel)
}
