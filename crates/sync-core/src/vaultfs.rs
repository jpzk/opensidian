// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Vault-anchored file access for sync and history (CodeRabbit #238
//! majors: workdir.rs 84/106, history 567/571).
//!
//! Every vault-relative path is resolved component by component from the
//! vault root; a parent that is a symlink (or not a directory) is refused,
//! so a link inside the vault can never steer a read or a write outside it.
//! Reads open the file and check that the opened handle is the regular file
//! that `lstat` saw (same device + inode on unix), so the bytes hashed are
//! the bytes of that file. Temp files are created with `create_new`
//! (O_CREAT|O_EXCL: never opens or follows an existing entry) and renamed
//! over the target (rename replaces the directory entry, never a link's
//! target).
//!
//! Residual: a same-user process racing the parent walk can still swap a
//! parent between the check and the open (no openat/O_NOFOLLOW without a
//! new dependency); remote peers cannot create symlinks (only regular
//! files sync), so the remote attack surface is closed.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

fn refused(rel: &str, why: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{rel}: {why}"))
}

fn components(rel: &str) -> io::Result<Vec<&str>> {
    let parts: Vec<&str> = rel.split('/').collect();
    if rel.is_empty() || parts.iter().any(|c| c.is_empty() || *c == "." || *c == ".." || c.contains('\\') || c.contains('\0')) {
        return Err(refused(rel, "not a plain vault-relative path"));
    }
    Ok(parts)
}

/// Absolute path of `rel` under `root` after checking that every parent
/// directory is a real directory (not a symlink). `create` makes missing
/// parents one level at a time (each re-checked). Returns `Ok(None)` when a
/// parent is missing and `create` is false.
pub fn anchored(root: &Path, rel: &str, create: bool) -> io::Result<Option<PathBuf>> {
    let parts = components(rel)?;
    let mut p = root.to_path_buf();
    for c in &parts[..parts.len() - 1] {
        p.push(c);
        let md = match fs::symlink_metadata(&p) {
            Ok(md) => md,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                match fs::create_dir(&p) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
                fs::symlink_metadata(&p)?
            }
            Err(e) => return Err(e),
        };
        if md.file_type().is_symlink() {
            return Err(refused(rel, "a parent directory is a symlink"));
        }
        if !md.is_dir() {
            return Err(refused(rel, "a parent is not a directory"));
        }
    }
    p.push(parts[parts.len() - 1]);
    Ok(Some(p))
}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// What [`read_regular`] found at a vault path.
#[derive(Debug)]
pub enum Found {
    Missing,
    /// Symlink, directory, device, … (never followed).
    NotRegular,
    /// Larger than the caller's limit (size given); not read.
    TooLarge(u64),
    Bytes(Vec<u8>, fs::Metadata),
}

/// Read the regular file at `rel` (anchored, no symlink anywhere on the
/// path). The opened handle must be the file `lstat` reported; a swap in
/// between is an error, not a silent read of something else.
pub fn read_regular(root: &Path, rel: &str, max: u64) -> io::Result<Found> {
    let Some(p) = anchored(root, rel, false)? else { return Ok(Found::Missing) };
    let before = match fs::symlink_metadata(&p) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Found::Missing),
        Err(e) => return Err(e),
    };
    if !before.file_type().is_file() {
        return Ok(Found::NotRegular);
    }
    if before.len() > max {
        return Ok(Found::TooLarge(before.len()));
    }
    let f = match File::open(&p) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Found::Missing),
        Err(e) => return Err(e),
    };
    let opened = f.metadata()?;
    if !opened.is_file() || !same_file(&before, &opened) {
        return Err(refused(rel, "file changed while it was opened"));
    }
    let mut buf = Vec::with_capacity(usize::try_from(opened.len().min(max)).unwrap_or(0));
    f.take(max.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        return Ok(Found::TooLarge(buf.len() as u64));
    }
    Ok(Found::Bytes(buf, opened))
}

/// Write `bytes` to `rel` atomically: parents anchored (created as real
/// directories), a fresh `create_new` temp file named `{tmp_prefix}{tag}`
/// next to the target, fsync, rename over the target. The temp file is
/// removed on failure.
pub fn write_atomic(root: &Path, rel: &str, bytes: &[u8], tmp_name: &str) -> io::Result<PathBuf> {
    let dst = anchored(root, rel, true)?.ok_or_else(|| refused(rel, "parent missing"))?;
    let dir = dst.parent().ok_or_else(|| refused(rel, "no parent"))?.to_path_buf();
    if tmp_name.contains('/') || tmp_name.is_empty() {
        return Err(refused(rel, "bad temp name"));
    }
    let tmp = dir.join(tmp_name);
    let res = (|| {
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, &dst)
    })();
    if let Err(e) = res {
        if e.kind() != io::ErrorKind::AlreadyExists {
            let _ = fs::remove_file(&tmp);
        }
        return Err(e);
    }
    if let Ok(d) = File::open(&dir) {
        let _ = d.sync_all();
    }
    Ok(dst)
}

/// Remove the file at `rel` (anchored; a missing file or parent is Ok).
/// Removes the directory entry itself, so a symlink is unlinked, never its
/// target.
pub fn remove(root: &Path, rel: &str) -> io::Result<()> {
    let Some(p) = anchored(root, rel, false)? else { return Ok(()) };
    match fs::remove_file(p) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Remove the directory at `rel` if it is an EMPTY real directory (anchored;
/// a symlink or file is left alone). Used before a file replaces a folder.
/// Ok(true) = removed, Ok(false) = nothing there; a non-empty directory is
/// an error (its files are not ours to drop).
pub fn remove_empty_dir(root: &Path, rel: &str) -> io::Result<bool> {
    let Some(p) = anchored(root, rel, false)? else { return Ok(false) };
    match fs::symlink_metadata(&p) {
        Ok(md) if md.is_dir() && !md.file_type().is_symlink() => fs::remove_dir(&p).map(|_| true),
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("osy-vaultfs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn symlinked_parent_is_refused_for_read_write_remove() {
        let t = tmpdir("parent");
        let vault = t.join("vault");
        let outside = t.join("outside");
        fs::create_dir_all(&vault).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.md"), b"outside").unwrap();
        symlink(&outside, vault.join("d")).unwrap();
        assert!(read_regular(&vault, "d/secret.md", 1 << 20).is_err());
        assert!(write_atomic(&vault, "d/new.md", b"x", ".osy-tmp-1").is_err());
        assert!(!outside.join("new.md").exists(), "nothing written outside the vault");
        assert!(remove(&vault, "d/secret.md").is_err());
        assert!(outside.join("secret.md").exists());
        let _ = fs::remove_dir_all(&t);
    }

    #[test]
    fn final_symlink_is_not_read_and_write_replaces_the_link() {
        let t = tmpdir("final");
        let vault = t.join("vault");
        fs::create_dir_all(&vault).unwrap();
        let outside = t.join("outside.md");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, vault.join("l.md")).unwrap();
        assert!(matches!(read_regular(&vault, "l.md", 100).unwrap(), Found::NotRegular));
        write_atomic(&vault, "l.md", b"new", ".osy-tmp-2").unwrap();
        assert_eq!(fs::read(&outside).unwrap(), b"outside", "link target untouched");
        assert!(fs::symlink_metadata(vault.join("l.md")).unwrap().is_file());
        let _ = fs::remove_dir_all(&t);
    }

    #[test]
    fn existing_temp_entry_is_never_reused() {
        let t = tmpdir("tmp");
        let vault = t.join("vault");
        fs::create_dir_all(&vault).unwrap();
        let outside = t.join("victim");
        fs::write(&outside, b"victim").unwrap();
        symlink(&outside, vault.join(".osy-tmp-3")).unwrap();
        assert!(write_atomic(&vault, "a.md", b"x", ".osy-tmp-3").is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"victim");
        let _ = fs::remove_dir_all(&t);
    }

    #[test]
    fn creates_parents_and_reads_back_with_limit() {
        let t = tmpdir("ok");
        let vault = t.join("vault");
        fs::create_dir_all(&vault).unwrap();
        write_atomic(&vault, "a/b/c.md", b"hello", ".osy-tmp-4").unwrap();
        assert!(matches!(read_regular(&vault, "a/b/c.md", 5).unwrap(), Found::Bytes(b, _) if b == b"hello"));
        assert!(matches!(read_regular(&vault, "a/b/c.md", 4).unwrap(), Found::TooLarge(5)));
        assert!(matches!(read_regular(&vault, "a/x/c.md", 5).unwrap(), Found::Missing));
        assert!(read_regular(&vault, "../c.md", 5).is_err());
        remove(&vault, "a/b/c.md").unwrap();
        remove(&vault, "a/b/c.md").unwrap();
        let _ = fs::remove_dir_all(&t);
    }

    #[test]
    fn remove_empty_dir_only_removes_empty_real_dirs() {
        let t = tmpdir("emptydir");
        fs::create_dir_all(t.join("a")).unwrap();
        fs::create_dir_all(t.join("full")).unwrap();
        fs::write(t.join("full/x.md"), b"x").unwrap();
        fs::write(t.join("f.md"), b"f").unwrap();
        fs::create_dir_all(t.join("target")).unwrap();
        symlink(t.join("target"), t.join("link")).unwrap();
        assert!(remove_empty_dir(&t, "a").unwrap());
        assert!(!t.join("a").exists());
        assert!(remove_empty_dir(&t, "full").is_err(), "files inside are kept");
        assert!(t.join("full/x.md").is_file());
        assert!(!remove_empty_dir(&t, "f.md").unwrap(), "a file is left alone");
        assert!(!remove_empty_dir(&t, "link").unwrap(), "a symlink is left alone");
        assert!(t.join("target").is_dir());
        assert!(!remove_empty_dir(&t, "nope").unwrap());
        let _ = fs::remove_dir_all(&t);
    }
}
