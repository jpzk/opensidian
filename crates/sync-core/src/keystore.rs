// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Device secret storage (key-tree.md §5, M5): two entries per vault.
//!
//! - **daily** — device signing key + VK keyring + server tokens. Read
//!   without any prompt (syncd runs unattended).
//! - **backup** — the backup words `S_bytes`. Every read must pass a
//!   user-presence check ([`Presence`]: OS auth, Touch ID, …).
//!
//! [`Keystore`] is the seam for OS backends (Secret Service, Keychain with
//! `ThisDeviceOnly`, Windows Credential Manager — follow-ups, each one a new
//! dependency to vet). [`FileKeystore`] is the headless / test backend:
//! a 0700 directory, 0600 files written tmp + fsync + rename, and the same
//! presence gate on backup reads.

use std::fmt;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use zeroize::Zeroizing;
use sync_proto::Id16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    Daily,
    Backup,
}

impl Entry {
    fn suffix(self) -> &'static str {
        match self {
            Entry::Daily => "daily",
            Entry::Backup => "backup",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeystoreError {
    /// The user-presence check failed or was cancelled.
    Denied,
    Io(String),
}

impl fmt::Display for KeystoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeystoreError::Denied => f.write_str("keystore: user presence check denied"),
            KeystoreError::Io(e) => write!(f, "keystore io: {e}"),
        }
    }
}

impl std::error::Error for KeystoreError {}

fn io(e: std::io::Error) -> KeystoreError {
    KeystoreError::Io(e.to_string())
}

/// A user-presence check (OS authentication). `reason` is shown to the user.
pub trait Presence: Send + Sync {
    fn confirm(&self, reason: &str) -> bool;
}

/// Always answers the same (tests, or a CLI that already asked).
#[derive(Debug, Clone, Copy)]
pub struct FixedPresence(pub bool);

impl Presence for FixedPresence {
    fn confirm(&self, _reason: &str) -> bool {
        self.0
    }
}

pub trait Keystore: Send + Sync {
    fn put(&self, vault: &Id16, entry: Entry, secret: &[u8]) -> Result<(), KeystoreError>;
    /// `Backup` reads pass the presence check first; `Daily` never prompts.
    fn get(&self, vault: &Id16, entry: Entry) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError>;
    fn delete(&self, vault: &Id16, entry: Entry) -> Result<(), KeystoreError>;
}

/// File-backed keystore (headless hosts and tests).
pub struct FileKeystore {
    dir: PathBuf,
    presence: Box<dyn Presence>,
}

impl fmt::Debug for FileKeystore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileKeystore").field("dir", &self.dir).finish()
    }
}

#[cfg(unix)]
fn private_dir(d: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    fs::DirBuilder::new().recursive(true).mode(0o700).create(d)?;
    fs::set_permissions(d, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn private_dir(d: &Path) -> std::io::Result<()> {
    fs::create_dir_all(d)
}

#[cfg(unix)]
fn create_private(p: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt as _;
    File::options().write(true).create_new(true).mode(0o600).open(p)
}

#[cfg(not(unix))]
fn create_private(p: &Path) -> std::io::Result<File> {
    File::options().write(true).create_new(true).open(p)
}

#[cfg(unix)]
fn sync_dir(d: &Path) -> std::io::Result<()> {
    File::open(d)?.sync_all()
}

// std cannot open (or fsync) a directory handle on Windows; the rename is
// the durability point there.
#[cfg(not(unix))]
fn sync_dir(_d: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Unique temp name per write (pid + process-wide counter + clock + retry),
/// so two writers never share or truncate each other's temp file.
fn temp_name(p: &Path, attempt: u32) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut s = p.as_os_str().to_owned();
    s.push(format!(".{}.{n}.{t}.{attempt}.tmp", std::process::id()));
    PathBuf::from(s)
}

impl FileKeystore {
    /// Unix only: the secrets rely on 0700/0600 owner-only modes. There is
    /// no Windows ACL code, so elsewhere the file keystore refuses to open
    /// (fail closed) instead of writing secrets other users could read.
    pub fn open(dir: impl Into<PathBuf>, presence: Box<dyn Presence>) -> Result<FileKeystore, KeystoreError> {
        let dir = dir.into();
        if !cfg!(unix) {
            return Err(KeystoreError::Io(
                "the file keystore needs owner-only file modes (unix); not supported on this platform".into(),
            ));
        }
        private_dir(&dir).map_err(io)?;
        Ok(FileKeystore { dir, presence })
    }

    pub fn path(&self, vault: &Id16, entry: Entry) -> PathBuf {
        self.dir.join(format!("{}.{}", vault.to_hex(), entry.suffix()))
    }
}

impl Keystore for FileKeystore {
    fn put(&self, vault: &Id16, entry: Entry, secret: &[u8]) -> Result<(), KeystoreError> {
        let p = self.path(vault, entry);
        let mut attempt = 0;
        let (tmp, mut f) = loop {
            let tmp = temp_name(&p, attempt);
            match create_private(&tmp) {
                Ok(f) => break (tmp, f),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 16 => attempt += 1,
                Err(e) => return Err(io(e)),
            }
        };
        let r = f.write_all(secret).and_then(|()| f.sync_all());
        drop(f);
        if let Err(e) = r.and_then(|()| fs::rename(&tmp, &p)) {
            let _ = fs::remove_file(&tmp);
            return Err(io(e));
        }
        sync_dir(&self.dir).map_err(io)
    }

    fn get(&self, vault: &Id16, entry: Entry) -> Result<Option<Zeroizing<Vec<u8>>>, KeystoreError> {
        let p = self.path(vault, entry);
        if !p.exists() {
            return Ok(None);
        }
        if entry == Entry::Backup && !self.presence.confirm("Reveal the backup words of this vault") {
            return Err(KeystoreError::Denied);
        }
        match fs::read(&p) {
            Ok(b) => Ok(Some(Zeroizing::new(b))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io(e)),
        }
    }

    fn delete(&self, vault: &Id16, entry: Entry) -> Result<(), KeystoreError> {
        match fs::remove_file(self.path(vault, entry)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io(e)),
        }
    }
}
