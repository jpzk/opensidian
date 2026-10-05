/* vaultfs — every write the app makes INSIDE a vault goes through here.

   The vault's contents are not ours: a sync client, a shared folder or a
   git pull can put a symlink anywhere in it, and can swap a directory for a
   symlink while we are mid-operation. A path-based write (`fs::write`,
   `File::create`, `fs::rename`, `create_dir_all`) re-resolves the path at the
   moment of use and follows whatever is there by then, so a check made a
   moment earlier proves nothing (audit #1 #2 #4 #5 #6).

   The rule here: the vault root is opened ONCE as a directory fd, and every
   vault-relative path is walked component by component with
   openat(O_NOFOLLOW|O_DIRECTORY). A symlink at ANY component is refused
   (Err, logged) — never followed. The leaf is then created / renamed /
   unlinked relative to the parent fd we hold, so a swap after the walk can
   only move the directory we already have open; it cannot redirect us.

   Atomic replace = a RANDOM sibling name opened O_CREAT|O_EXCL|O_NOFOLLOW
   (mode 0600 until written), fsync, fchmod to the final mode, renameat over
   the leaf in the same parent fd. No predictable temp name exists to plant a
   symlink on, and O_EXCL would refuse one anyway.

   The vault ROOT itself is the user's choice and is trusted: it is
   canonicalized once (so `~/Notes -> /data/notes` keeps working) and the
   canonical path is opened O_NOFOLLOW. Everything below it is untrusted. */

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

// test seam: a named point between resolving a parent and using it, so a
// test can swap a directory for a symlink deterministically. Production: a
// no-op the optimiser deletes. Thread-local, so parallel tests never collide.
#[cfg(test)]
thread_local! {
    static HOOK: std::cell::RefCell<Option<Box<dyn FnMut(&'static str)>>> = std::cell::RefCell::new(None);
}
#[inline(always)]
fn hook(_at: &'static str) {
    #[cfg(test)]
    HOOK.with(|h| {
        if let Some(f) = h.borrow_mut().as_mut() {
            f(_at)
        }
    });
}
#[cfg(test)]
fn set_hook(f: Option<Box<dyn FnMut(&'static str)>>) {
    HOOK.with(|h| *h.borrow_mut() = f);
}

fn refused(rel: &Path, why: &str) -> io::Error {
    let e = io::Error::new(io::ErrorKind::PermissionDenied, format!("refused: {why}: {}", rel.display()));
    eprintln!("[vaultfs] {e}");
    e
}

fn cstr(s: &OsStr) -> io::Result<CString> {
    CString::new(s.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))
}

fn cvt(r: libc::c_int) -> io::Result<libc::c_int> {
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(r) }
}

fn stat_at(dfd: RawFd, name: &CString) -> io::Result<libc::stat> {
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: valid fd, NUL-terminated name, st is written by the kernel on success
    cvt(unsafe { libc::fstatat(dfd, name.as_ptr(), st.as_mut_ptr(), libc::AT_SYMLINK_NOFOLLOW) })?;
    Ok(unsafe { st.assume_init() })
}

fn fstat(fd: RawFd) -> io::Result<libc::stat> {
    let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: valid fd; st is written by the kernel on success
    cvt(unsafe { libc::fstat(fd, st.as_mut_ptr()) })?;
    Ok(unsafe { st.assume_init() })
}

fn is_reg(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFREG
}

fn open_dir_at(dfd: RawFd, name: &CString) -> io::Result<OwnedFd> {
    // SAFETY: valid fd + NUL-terminated name; the returned fd is owned below
    let fd = cvt(unsafe {
        libc::openat(dfd, name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
    })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// the process umask, read without changing it (umask(2) would set it, racing
/// every other thread that creates a file meanwhile)
fn umask() -> u32 {
    static M: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *M.get_or_init(|| {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| s.lines().find_map(|l| l.strip_prefix("Umask:").map(|v| v.trim().to_string())))
            .and_then(|v| u32::from_str_radix(&v, 8).ok())
            .unwrap_or(0o022)
    })
}

/// a temp name nobody can predict: 16 hex chars from the kernel's CSPRNG.
/// Dot-prefixed and `~`-suffixed, so no index/watcher (dot-skipping, .md/.json
/// matching) ever picks it up even if a crash leaves it behind.
fn tmp_name(leaf: &OsStr) -> io::Result<CString> {
    let mut b = [0u8; 8];
    File::open("/dev/urandom")?.read_exact(&mut b)?;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    let mut n = Vec::from(&b"."[..]);
    n.extend_from_slice(&leaf.as_bytes()[..leaf.len().min(64)]);
    n.extend_from_slice(format!(".{hex}~").as_bytes());
    CString::new(n).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in path"))
}

/// a vault-relative path split into checked components; anything but plain
/// names (`..`, `/`, `.`, empty) is refused before any syscall
fn parts(rel: &Path) -> io::Result<Vec<&OsStr>> {
    let mut v = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => v.push(s),
            _ => return Err(refused(rel, "not a plain vault-relative path")),
        }
    }
    if v.is_empty() {
        return Err(refused(rel, "empty path"));
    }
    Ok(v)
}

/// a directory INSIDE the vault, held open: the thing every leaf op is
/// relative to
pub struct Dir {
    fd: OwnedFd,
}

/// the vault root, opened once
pub struct Vault {
    root: Dir,
    croot: PathBuf,
}

impl Vault {
    pub fn open(root: &Path) -> io::Result<Vault> {
        let croot = root.canonicalize()?;
        let c = cstr(croot.as_os_str())?;
        let root = Dir { fd: open_dir_at(libc::AT_FDCWD, &c)? };
        Ok(Vault { root, croot })
    }

    /// the canonical root this handle was opened on
    pub fn path(&self) -> &Path {
        &self.croot
    }

    /// walk `comps` from the root, one openat(O_NOFOLLOW|O_DIRECTORY) per
    /// component; `create` makes missing ones with mkdirat (never through a
    /// symlink: mkdirat on an existing symlink is EEXIST, and the openat after
    /// it refuses the symlink)
    fn walk(&self, rel: &Path, comps: &[&OsStr], create: bool) -> io::Result<Dir> {
        let mut cur: Option<OwnedFd> = None;
        for c in comps {
            let dfd = cur.as_ref().map_or(self.root.fd.as_raw_fd(), |f| f.as_raw_fd());
            let name = cstr(c)?;
            let next = match open_dir_at(dfd, &name) {
                Ok(f) => f,
                Err(e) if e.raw_os_error() == Some(libc::ENOENT) && create => {
                    // SAFETY: valid fd + NUL-terminated name
                    match cvt(unsafe { libc::mkdirat(dfd, name.as_ptr(), 0o777) }) {
                        Ok(_) => {}
                        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {}
                        Err(e) => return Err(e),
                    }
                    open_dir_at(dfd, &name).map_err(|e| self.why(rel, e))?
                }
                Err(e) => return Err(self.why(rel, e)),
            };
            cur = Some(next);
        }
        Ok(match cur {
            Some(fd) => Dir { fd },
            None => Dir { fd: self.root.fd.try_clone()? },
        })
    }

    /// ELOOP/ENOTDIR from an O_NOFOLLOW|O_DIRECTORY open = a symlink (or a
    /// file) where a directory should be: say so, and log it
    fn why(&self, rel: &Path, e: io::Error) -> io::Error {
        match e.raw_os_error() {
            Some(libc::ELOOP) | Some(libc::ENOTDIR) => refused(rel, "symlink or non-directory in vault path"),
            _ => e,
        }
    }

    /// the directory `rel` names, created (`mkdir -p`) when `create`
    pub fn dir(&self, rel: &Path, create: bool) -> io::Result<Dir> {
        if rel.as_os_str().is_empty() {
            return Ok(Dir { fd: self.root.fd.try_clone()? });
        }
        let comps = parts(rel)?;
        self.walk(rel, &comps, create)
    }

    /// `mkdir -p rel`, never through a symlink
    pub fn mkdir_p(&self, rel: &Path) -> io::Result<()> {
        self.dir(rel, true).map(|_| ())
    }

    /// (parent dir fd, leaf name) for a file path
    fn parent(&self, rel: &Path, create: bool) -> io::Result<(Dir, CString)> {
        let comps = parts(rel)?;
        let (leaf, dirs) = comps.split_last().expect("parts is non-empty");
        let d = self.walk(rel, dirs, create)?;
        Ok((d, cstr(leaf)?))
    }

    /// is `rel` a regular file (NOT a symlink to one)?
    pub fn is_file(&self, rel: &Path) -> bool {
        self.parent(rel, false)
            .and_then(|(d, leaf)| stat_at(d.fd.as_raw_fd(), &leaf))
            .is_ok_and(|st| is_reg(&st))
    }

    /// durable replace of `rel` with `bytes`. Parents are created. A leaf that
    /// exists and is not a regular file (a symlink) is refused. `durable`
    /// additionally fsyncs the parent directory so the rename survives a crash.
    pub fn write_atomic(&self, rel: &Path, bytes: &[u8], durable: bool) -> io::Result<()> {
        let (d, leaf) = self.parent(rel, true)?;
        hook("write_atomic:resolved");
        // the parent we hold must still BE `rel`'s parent: a directory swapped
        // (for a symlink, or moved) since the walk is refused, not written into
        if let Some(pr) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            let again = self.dir(pr, false)?;
            let (a, b) = (fstat(d.fd.as_raw_fd())?, fstat(again.fd.as_raw_fd())?);
            if (a.st_dev, a.st_ino) != (b.st_dev, b.st_ino) {
                return Err(refused(rel, "parent directory changed during write"));
            }
        }
        let dfd = d.fd.as_raw_fd();
        let mode = match stat_at(dfd, &leaf) {
            Ok(st) if is_reg(&st) => (st.st_mode as u32) & 0o7777,
            Ok(_) => return Err(refused(rel, "target is not a regular file")),
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => 0o666 & !umask(),
            Err(e) => return Err(e),
        };
        let tmp = tmp_name(OsStr::from_bytes(leaf.as_bytes()))?;
        // SAFETY: valid fd + NUL-terminated name; fd owned by the File below
        let raw = cvt(unsafe {
            libc::openat(
                dfd,
                tmp.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        })?;
        let mut f = unsafe { File::from_raw_fd(raw) };
        let r = (|| -> io::Result<()> {
            f.write_all(bytes)?;
            // SAFETY: valid fd
            cvt(unsafe { libc::fchmod(f.as_raw_fd(), mode as libc::mode_t) })?;
            f.sync_all()?;
            // SAFETY: both names NUL-terminated, same parent fd
            cvt(unsafe { libc::renameat(dfd, tmp.as_ptr(), dfd, leaf.as_ptr()) })?;
            Ok(())
        })();
        drop(f);
        if let Err(e) = r {
            // SAFETY: valid fd + name; removes only the temp WE created (O_EXCL)
            unsafe { libc::unlinkat(dfd, tmp.as_ptr(), 0) };
            return Err(e);
        }
        if durable {
            // SAFETY: valid fd
            cvt(unsafe { libc::fsync(dfd) })?;
        }
        Ok(())
    }

    /// create `rel` exclusively (O_EXCL|O_NOFOLLOW) and hand back the open
    /// file; AlreadyExists when anything (a symlink included) is there.
    /// Parents are created when `parents`.
    pub fn create_new(&self, rel: &Path, parents: bool) -> io::Result<File> {
        let (d, leaf) = self.parent(rel, parents)?;
        hook("create_new:resolved");
        // SAFETY: valid fd + NUL-terminated name; fd owned by the File below
        let raw = cvt(unsafe {
            libc::openat(
                d.fd.as_raw_fd(),
                leaf.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o666 as libc::c_uint,
            )
        })?;
        Ok(unsafe { File::from_raw_fd(raw) })
    }

    /// unlink the FILE `rel` (never a directory), relative to its parent fd
    pub fn unlink(&self, rel: &Path) -> io::Result<()> {
        let (d, leaf) = self.parent(rel, false)?;
        // SAFETY: valid fd + NUL-terminated name
        cvt(unsafe { libc::unlinkat(d.fd.as_raw_fd(), leaf.as_ptr(), 0) }).map(|_| ())
    }

    /// move the regular file `from` to `to`, never overwriting: `to` is claimed
    /// with O_EXCL first (AlreadyExists if taken), then renameat replaces only
    /// that zero-byte claim. Parents of `to` are created.
    pub fn move_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        let (fd_from, lf) = self.parent(from, false)?;
        match stat_at(fd_from.fd.as_raw_fd(), &lf) {
            Ok(st) if is_reg(&st) => {}
            Ok(_) => return Err(refused(from, "source is not a regular file")),
            Err(e) => return Err(e),
        }
        let (fd_to, lt) = self.parent(to, true)?;
        drop(self.create_new_at(&fd_to, &lt)?);
        hook("move_file:claimed");
        // SAFETY: valid fds + NUL-terminated names
        if let Err(e) = cvt(unsafe { libc::renameat(fd_from.fd.as_raw_fd(), lf.as_ptr(), fd_to.fd.as_raw_fd(), lt.as_ptr()) }) {
            unsafe { libc::unlinkat(fd_to.fd.as_raw_fd(), lt.as_ptr(), 0) };
            return Err(e);
        }
        Ok(())
    }

    fn create_new_at(&self, d: &Dir, leaf: &CString) -> io::Result<File> {
        // SAFETY: valid fd + NUL-terminated name
        let raw = cvt(unsafe {
            libc::openat(
                d.fd.as_raw_fd(),
                leaf.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600 as libc::c_uint,
            )
        })?;
        Ok(unsafe { File::from_raw_fd(raw) })
    }

    /// move the regular file `rel` into the directory `tdir` (created, never
    /// through a symlink) under `stem.ext`, `stem.1.ext`, ... — the first name
    /// we can CLAIM with O_EXCL, so two deletes never overwrite each other.
    /// Returns the vault-relative destination.
    pub fn trash(&self, rel: &Path, tdir: &Path, stem: &str, ext: &str) -> io::Result<PathBuf> {
        let (fd_from, lf) = self.parent(rel, false)?;
        match stat_at(fd_from.fd.as_raw_fd(), &lf) {
            Ok(st) if is_reg(&st) => {}
            Ok(_) => return Err(refused(rel, "not a regular file")),
            Err(e) => return Err(e),
        }
        let t = self.dir(tdir, true)?;
        hook("trash:resolved");
        for i in 0..10_000u32 {
            let name = if i == 0 { format!("{stem}.{ext}") } else { format!("{stem}.{i}.{ext}") };
            let cn = cstr(OsStr::new(&name))?;
            match self.create_new_at(&t, &cn) {
                Ok(f) => drop(f),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
            // SAFETY: valid fds + NUL-terminated names
            if let Err(e) = cvt(unsafe { libc::renameat(fd_from.fd.as_raw_fd(), lf.as_ptr(), t.fd.as_raw_fd(), cn.as_ptr()) }) {
                unsafe { libc::unlinkat(t.fd.as_raw_fd(), cn.as_ptr(), 0) };
                return Err(e);
            }
            return Ok(tdir.join(name));
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name in the trash"))
    }
}

/// one-shot helpers for callers that hold only the root path
pub fn write_atomic(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
    Vault::open(root)?.write_atomic(rel, bytes, false)
}

pub fn write_atomic_durable(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
    Vault::open(root)?.write_atomic(rel, bytes, true)
}

pub fn mkdir_p(root: &Path, rel: &Path) -> io::Result<()> {
    Vault::open(root)?.mkdir_p(rel)
}

/// tree walk that never descends a symlinked directory (audit #9) — the same
/// boundary the note index draws with symlink_metadata. `out` gets every
/// non-hidden real directory's vault-relative path, depth first.
pub fn walk_real_dirs(dir: &Path, base: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if e.file_name().as_bytes().first() == Some(&b'.') {
            continue;
        }
        // DirEntry::file_type does NOT follow symlinks: a symlinked dir is
        // a symlink here, and is skipped
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let p = e.path();
        if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.display().to_string());
        }
        walk_real_dirs(&p, base, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn fresh(tag: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("vaultfs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, out) = (base.join("vault"), base.join("outside"));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&out).unwrap();
        (root, out)
    }

    #[test]
    fn vaultfs_planted_tmp_symlink_is_not_followed() {
        let (root, out) = fresh("tmp");
        let victim = out.join("victim.txt");
        fs::write(&victim, "precious").unwrap();
        // every predictable name the old writer used
        for t in ["X.md.tmp", "X.tmp", ".X.md.tmp"] {
            symlink(&victim, root.join(t)).unwrap();
        }
        let v = Vault::open(&root).unwrap();
        v.write_atomic(Path::new("X.md"), b"new body", false).unwrap();
        assert_eq!(fs::read_to_string(root.join("X.md")).unwrap(), "new body");
        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
        // no stray temp left behind
        let stray: Vec<_> = fs::read_dir(&root).unwrap().flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with('~')).collect();
        assert!(stray.is_empty());
    }

    #[test]
    fn vaultfs_symlinked_obsidian_dir_is_refused() {
        let (root, out) = fresh("obs");
        symlink(&out, root.join(".obsidian")).unwrap();
        let v = Vault::open(&root).unwrap();
        for f in ["app.json", "workspace.json", "bookmarks.json", "appearance.json", "themes/T/theme.css"] {
            let e = v.write_atomic(&Path::new(".obsidian").join(f), b"{}", true).unwrap_err();
            assert!(e.to_string().contains("refused"), "{f}: {e}");
        }
        assert!(v.mkdir_p(Path::new(".obsidian/themes")).is_err());
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0, "outside dir must stay empty");
    }

    #[test]
    fn vaultfs_symlinked_trash_is_refused() {
        let (root, out) = fresh("trash");
        fs::write(root.join("Note.md"), "keep me").unwrap();
        symlink(&out, root.join(".trash")).unwrap();
        let v = Vault::open(&root).unwrap();
        assert!(v.trash(Path::new("Note.md"), Path::new(".trash"), "Note", "md").is_err());
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
        assert_eq!(fs::read_to_string(root.join("Note.md")).unwrap(), "keep me");
        // a REAL .trash works, and never overwrites a previous delete
        fs::remove_file(root.join(".trash")).unwrap();
        assert_eq!(v.trash(Path::new("Note.md"), Path::new(".trash"), "Note", "md").unwrap(), Path::new(".trash/Note.md"));
        fs::write(root.join("Note.md"), "second").unwrap();
        assert_eq!(v.trash(Path::new("Note.md"), Path::new(".trash"), "Note", "md").unwrap(), Path::new(".trash/Note.1.md"));
        assert_eq!(fs::read_to_string(root.join(".trash/Note.md")).unwrap(), "keep me");
    }

    #[test]
    fn vaultfs_parent_swapped_for_symlink_mid_write_is_refused() {
        let (root, out) = fresh("swap");
        fs::create_dir_all(root.join("sub")).unwrap();
        let v = Vault::open(&root).unwrap();
        let (r2, o2) = (root.clone(), out.clone());
        set_hook(Some(Box::new(move |at| {
            if at == "write_atomic:resolved" {
                fs::rename(r2.join("sub"), r2.join("sub-moved")).unwrap();
                symlink(&o2, r2.join("sub")).unwrap();
            }
        })));
        let r = v.write_atomic(Path::new("sub/N.md"), b"evil", false);
        set_hook(None);
        assert!(r.is_err(), "write through a swapped parent must be refused");
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0, "nothing may land outside");
        assert_eq!(fs::read_dir(root.join("sub-moved")).unwrap().count(), 0, "nor in the moved dir");
        // and a symlinked parent present up front is refused at the walk
        assert!(v.write_atomic(Path::new("sub/N.md"), b"evil", false).is_err());
        assert!(v.create_new(Path::new("sub/N.md"), true).is_err());
        assert!(v.mkdir_p(Path::new("sub/deeper")).is_err());
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
    }

    #[test]
    fn vaultfs_dir_cycle_walk_terminates() {
        let (root, _out) = fresh("cycle");
        fs::create_dir_all(root.join("a")).unwrap();
        symlink(root.join("a"), root.join("a/b")).unwrap(); // a/b -> a
        symlink(&root, root.join("a/up")).unwrap(); // a/up -> vault root
        let mut v = Vec::new();
        walk_real_dirs(&root, &root, &mut v);
        assert_eq!(v, vec!["a".to_string()]);
    }

    #[test]
    fn vaultfs_symlinked_note_leaf_is_refused() {
        let (root, out) = fresh("leaf");
        let victim = out.join("v.md");
        fs::write(&victim, "outside").unwrap();
        fs::write(root.join("A.md"), "a").unwrap();
        symlink(&victim, root.join("B.md")).unwrap();
        let v = Vault::open(&root).unwrap();
        v.write_atomic(Path::new("A.md"), b"A2", false).unwrap();
        assert!(v.write_atomic(Path::new("B.md"), b"B2", false).is_err());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "outside");
        assert!(fs::symlink_metadata(root.join("B.md")).unwrap().file_type().is_symlink());
        // move refuses a symlink source and never overwrites a target
        assert!(v.move_file(Path::new("B.md"), Path::new("C.md")).is_err());
        fs::write(root.join("D.md"), "d").unwrap();
        assert_eq!(v.move_file(Path::new("A.md"), Path::new("D.md")).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        v.move_file(Path::new("A.md"), Path::new("new/dir/A.md")).unwrap();
        assert_eq!(fs::read_to_string(root.join("new/dir/A.md")).unwrap(), "A2");
    }

    #[test]
    fn vaultfs_rejects_non_plain_paths_and_keeps_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (root, _out) = fresh("plain");
        let v = Vault::open(&root).unwrap();
        for bad in ["../x.md", "/etc/x", "", "a/../../x"] {
            assert!(v.write_atomic(Path::new(bad), b"x", false).is_err(), "{bad}");
        }
        fs::write(root.join("M.md"), "m").unwrap();
        fs::set_permissions(root.join("M.md"), fs::Permissions::from_mode(0o640)).unwrap();
        v.write_atomic(Path::new("M.md"), b"m2", false).unwrap();
        assert_eq!(fs::metadata(root.join("M.md")).unwrap().permissions().mode() & 0o777, 0o640);
    }
}
