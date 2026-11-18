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
                let bytes = fs::read(ent.path())?;
                let md = ent.metadata()?;
                out.insert(rel, DiskFile { sha256: p.sha256(&bytes), size: bytes.len() as u64, mtime_ms: mtime_ms(&md) });
            }
        }
    }
    Ok((out, skipped))
}

/// Current sha256 of one file (None = absent / not a regular file).
pub fn disk_sha<P: CryptoProvider + ?Sized>(p: &P, root: &Path, rel: &str) -> io::Result<Option<[u8; 32]>> {
    let path = abs(root, rel);
    match fs::symlink_metadata(&path) {
        Ok(md) if md.is_file() => Ok(Some(p.sha256(&fs::read(&path)?))),
        Ok(_) => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Temp file next to the target + rename (parents created).
pub fn write_atomic<P: CryptoProvider + ?Sized>(p: &P, root: &Path, rel: &str, bytes: &[u8]) -> io::Result<()> {
    let path = abs(root, rel);
    let dir = path.parent().ok_or_else(|| io::Error::other("no parent"))?;
    fs::create_dir_all(dir)?;
    let mut r = [0u8; 8];
    p.random(&mut r).map_err(|e| io::Error::other(e.to_string()))?;
    let tmp = dir.join(format!("{TMP_PREFIX}{}", sync_core::sync_proto::hex_lower(&r)));
    fs::write(&tmp, bytes)?;
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

pub fn remove(root: &Path, rel: &str) -> io::Result<()> {
    match fs::remove_file(abs(root, rel)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}
