// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Content-addressed blob store: `objects/<aa>/<sha256-hex>`, plaintext
//! bytes, written via temp file + fsync + rename (a crash never leaves a
//! partial blob under its final name). Reads verify the hash.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use super::journal::is_sha_hex;
use super::{fsx, HistoryError};

static TMP_CTR: AtomicU64 = AtomicU64::new(0);

pub(crate) fn hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 0x0f) as usize] as char);
    }
    s
}

pub(crate) fn sha_hex(data: &[u8]) -> String {
    hex(&sync_crypto::content_sha256(data))
}

pub(crate) struct Cas {
    dir: PathBuf,
}

impl Cas {
    pub(crate) fn open(dir: PathBuf) -> Result<Self, HistoryError> {
        fsx::mkdir_private(&dir)?;
        fsx::mkdir_private(&dir.join("tmp"))?;
        Ok(Cas { dir })
    }

    pub(crate) fn path_of(&self, sha: &str) -> PathBuf {
        self.dir.join(&sha[..2]).join(sha)
    }

    pub(crate) fn contains(&self, sha: &str) -> bool {
        self.path_of(sha).is_file()
    }

    /// Store `data`; returns its sha256 hex. Idempotent (dedup by content).
    pub(crate) fn put(&self, data: &[u8]) -> Result<String, HistoryError> {
        let sha = sha_hex(data);
        let dst = self.path_of(&sha);
        // Dedup only against a blob that still verifies: a truncated or
        // bit-rotted copy must not make a new snapshot look recoverable, so
        // it is replaced by the fresh bytes below.
        if dst.is_file() && matches!(fs::read(&dst), Ok(old) if sha_hex(&old) == sha) {
            return Ok(sha);
        }
        let shard = self.dir.join(&sha[..2]);
        let fresh_shard = !shard.is_dir();
        fsx::mkdir_private(&shard)?;
        if fresh_shard {
            // the shard's entry in objects/ must be durable before any
            // journal record names a blob inside it
            fsx::sync_dir(&self.dir)?;
        }
        let n = TMP_CTR.fetch_add(1, Ordering::Relaxed);
        let tmp = self.dir.join("tmp").join(format!("{}.{}.{}", sha, std::process::id(), n));
        {
            let mut f = fsx::create_private(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &dst)?;
        fsx::sync_dir(&shard)?;
        Ok(sha)
    }

    /// Read and verify a blob.
    pub(crate) fn get(&self, sha: &str) -> Result<Vec<u8>, HistoryError> {
        let p = self.path_of(sha);
        let data = match fs::read(&p) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(HistoryError::NotFound(format!("blob {sha}")))
            }
            Err(e) => return Err(e.into()),
        };
        if sha_hex(&data) != sha {
            return Err(HistoryError::BlobMismatch(sha.to_string()));
        }
        Ok(data)
    }

    /// Delete a blob (purge / GC). Missing is not an error.
    pub(crate) fn remove(&self, sha: &str) -> Result<(), HistoryError> {
        match fs::remove_file(self.path_of(sha)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// GC: delete every stored blob for which `keep(sha)` is false.
    /// Only `<aa>/<sha>` names are considered (tmp/ and strays are left
    /// alone). Returns the number removed.
    pub(crate) fn sweep(&self, keep: impl Fn(&str) -> bool) -> Result<usize, HistoryError> {
        let mut n = 0;
        for d in fs::read_dir(&self.dir)? {
            let d = d?;
            let name = d.file_name();
            let Some(pre) = name.to_str() else { continue };
            let is_shard = pre.len() == 2 && pre.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'));
            if !is_shard || !d.file_type()?.is_dir() {
                continue;
            }
            for f in fs::read_dir(d.path())? {
                let f = f?;
                let fname = f.file_name();
                let Some(sha) = fname.to_str() else { continue };
                if !is_sha_hex(sha) || !sha.starts_with(pre) || keep(sha) {
                    continue;
                }
                self.remove(sha)?;
                n += 1;
            }
        }
        Ok(n)
    }
}
