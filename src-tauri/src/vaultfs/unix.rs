// SPDX-License-Identifier: GPL-3.0-or-later
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
// Compiled under debug_assertions too (not just cfg(test)): since lib.rs the
// bin's tests (main.rs) link vaultfs as a DEPENDENCY, where cfg(test) is off.
// Unset (the default) every seam is a no-op; release builds still drop them.
#[cfg(any(test, debug_assertions))]
thread_local! {
    static HOOK: std::cell::RefCell<Option<Box<dyn FnMut(&'static str)>>> = std::cell::RefCell::new(None);
}
#[inline(always)]
fn hook(_at: &'static str) {
    #[cfg(any(test, debug_assertions))]
    HOOK.with(|h| {
        if let Some(f) = h.borrow_mut().as_mut() {
            f(_at)
        }
    });
}
#[cfg(any(test, debug_assertions))]
pub fn set_hook(f: Option<Box<dyn FnMut(&'static str)>>) {
    HOOK.with(|h| *h.borrow_mut() = f);
}

/* osdrop crash-injection seam (tests only): `fault(at)` is an Err at exactly
   the named step, so a test can prove what an interrupted import leaves behind.
   In a release build it is `hook(at); Ok(())` — no branch, no state. */
#[cfg(any(test, debug_assertions))]
thread_local! {
    static FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static FORCE_COPY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
fn fault(at: &'static str) -> io::Result<()> {
    #[cfg(any(test, debug_assertions))]
    if FAULT.with(|f| f.get()) == Some(at) {
        return Err(io::Error::other(format!("injected fault at {at}")));
    }
    hook(at);
    Ok(())
}
#[cfg(any(test, debug_assertions))]
pub fn set_fault(at: Option<&'static str>) {
    FAULT.with(|f| f.set(at));
}
/// tests only: take the cross-filesystem (copy) road even on one filesystem
#[cfg(any(test, debug_assertions))]
pub fn set_force_copy(on: bool) {
    FORCE_COPY.with(|f| f.set(on));
}
fn force_copy() -> bool {
    #[cfg(any(test, debug_assertions))]
    return FORCE_COPY.with(|f| f.get());
    #[cfg(not(any(test, debug_assertions)))]
    false
}

/// osdrop: what happened to the SOURCE once the vault file was committed
#[derive(Debug, PartialEq)]
pub enum SrcFate {
    Removed,      // a move: the source name is gone
    Kept(String), // committed + verified in the vault, but the source could not be removed (why)
    Copied,       // pdfdrop: a COPY was asked for (stock copies a dropped PDF): the source is left as it was
}

/// osdrop: why an import did not happen. The source is intact in every case.
#[derive(Debug)]
pub enum ImportErr {
    OpenSrc(io::Error), // could not open the source (EACCES = the sandbox)
    NotRegular,         // a symlink, dir, fifo, device ... (or became one)
    TooBig(u64),        // over the cap, by metadata or by the bytes read
    BadMagic,           // pdfdrop: the opened source does not start with the type's magic bytes
    Changed,            // the source changed identity or content during the import
    NoFreeName,         // every candidate name was taken
    Io(io::Error),      // staging / verify / commit failed
}

#[derive(Debug)]
pub struct Imported {
    pub name: String,      // the leaf it got in the vault
    pub src: SrcFate,
    pub how: &'static str, // "link" (same filesystem) | "copy" (crossed one)
}

fn fs_lstat(p: &CString) -> io::Result<libc::stat> {
    stat_at(libc::AT_FDCWD, p)
}

/// pdfdrop: does the OPEN file start with `magic`? pread at offset 0, so the fd's
/// cursor is untouched for the copy that follows; a short file is a mismatch.
fn starts_with_at0(f: &File, magic: &[u8]) -> io::Result<bool> {
    use std::os::unix::fs::FileExt;
    let mut head = vec![0u8; magic.len()];
    let mut n = 0;
    while n < head.len() {
        match f.read_at(&mut head[n..], n as u64) {
            Ok(0) => return Ok(false),
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(head == magic)
}

/// both files re-read from offset 0 and compared whole: equal length, equal bytes
fn same_bytes(a: &mut File, b: &mut File) -> io::Result<bool> {
    use std::io::{Seek, SeekFrom};
    a.seek(SeekFrom::Start(0))?;
    b.seek(SeekFrom::Start(0))?;
    let (mut x, mut y) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let n = fill(a, &mut x)?;
        let m = fill(b, &mut y)?;
        if n != m || x[..n] != y[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

/// read until `buf` is full or EOF (a short read is not the end of the file)
fn fill(f: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
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
}

impl Vault {
    pub fn open(root: &Path) -> io::Result<Vault> {
        let croot = root.canonicalize()?;
        let c = cstr(croot.as_os_str())?;
        let root = Dir { fd: open_dir_at(libc::AT_FDCWD, &c)? };
        Ok(Vault { root })
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

    /* osdrop — MOVE a file from OUTSIDE the vault into `dir` (vault-relative),
       never overwriting, crash-safe. The ONE primitive drop-to-attach stands on.
       Invariant: until the final rename succeeds the source is untouched and the
       vault holds no new visible name; after it, the vault file is complete and
       durable, and only THEN is the source name removed.
         1. open the source O_NOFOLLOW, fstat: regular file, <= cap. That fd is
            the identity every later step is checked against (dev, ino).
            pdfdrop: if `magic` is non-empty, the first magic.len() bytes OF THAT FD
            (pread, offset 0) must equal it, else BadMagic before anything is staged.
         2. STAGE under a random dot-name (tmp_name: no index/watcher sees it):
            same filesystem -> linkat(src, tmp): no byte is copied, then the tmp's
                               (dev, ino) must equal the opened fd's — a source
                               swapped for a symlink/other file since step 1 is
                               refused, not imported.
            otherwise (EXDEV, EPERM from protected_hardlinks, EXDEV from a
            Landlock refer denial, ...) -> copy at most cap bytes into an O_EXCL
                               tmp, fsync it, then VERIFY: the source fd's size and
                               mtime did not change, and the tmp read back from disk
                               is byte-identical to the source (both re-read).
         3. renameat2(tmp, name, RENAME_NOREPLACE) over the candidate names
            (EEXIST -> next name; a filesystem without NOREPLACE -> linkat, which
            also refuses an existing name), then fsync the directory.
         4. remove the source name, only if it still IS the file we imported.
       Any error in 1-3 unlinks the tmp and returns Err with the source intact. An
       error in 4 is not a failure of the import: the vault file is committed and
       verified, the source simply stays (SrcFate::Kept, e.g. a read-only drop
       folder under Landlock) — a duplicate, never a loss.
       pdfdrop: keep_src = true is the COPY variant (stock 1.13.7 copies a dropped PDF, measured by OS drag):
       step 2 always takes the verified-copy road (never linkat: a hard link is not a copy)
       and step 4 is skipped — SrcFate::Copied. Steps 1 and 3 (fd identity, cap, magic,
       NOREPLACE) are the same. */
    pub fn import_move(&self, src: &Path, dir: &Path, names: &mut dyn Iterator<Item = String>, cap: u64, magic: &[u8], keep_src: bool) -> Result<Imported, ImportErr> {
        use std::os::unix::fs::OpenOptionsExt;
        let d = self.dir(dir, false).map_err(ImportErr::Io)?;
        let dfd = d.fd.as_raw_fd();
        // 1. the source, by fd
        let mut sf = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(src)
            .map_err(|e| if e.raw_os_error() == Some(libc::ELOOP) { ImportErr::NotRegular } else { ImportErr::OpenSrc(e) })?;
        let st0 = fstat(sf.as_raw_fd()).map_err(ImportErr::Io)?;
        if !is_reg(&st0) {
            return Err(ImportErr::NotRegular);
        }
        if st0.st_size as u64 > cap {
            return Err(ImportErr::TooBig(st0.st_size as u64));
        }
        if !magic.is_empty() && !starts_with_at0(&sf, magic).map_err(ImportErr::Io)? {
            return Err(ImportErr::BadMagic);
        }
        let mut names = names.peekable();
        let first = names.peek().cloned().ok_or(ImportErr::NoFreeName)?;
        let tmp = tmp_name(OsStr::new(&first)).map_err(ImportErr::Io)?;
        let csrc = cstr(src.as_os_str()).map_err(ImportErr::Io)?;
        let unstage = || unsafe { libc::unlinkat(dfd, tmp.as_ptr(), 0) };
        // 2. stage
        // keep_src: never the link road — a hard link would make the "copy" the SAME inode as
        // the source, so an edit to either would change both. A copy is bytes, verified.
        let linked = !keep_src
            && !force_copy()
            // SAFETY: NUL-terminated names, valid dir fd; flags 0 = never follow a symlink at src
            && cvt(unsafe { libc::linkat(libc::AT_FDCWD, csrc.as_ptr(), dfd, tmp.as_ptr(), 0) }).is_ok();
        let how = if linked {
            let r = stat_at(dfd, &tmp).map_err(ImportErr::Io).and_then(|t| {
                if (t.st_dev, t.st_ino) == (st0.st_dev, st0.st_ino) && is_reg(&t) { Ok(()) } else { Err(ImportErr::Changed) }
            });
            if let Err(e) = r.and_then(|_| fault("import:staged").map_err(ImportErr::Io)) {
                unstage();
                return Err(e);
            }
            "link"
        } else {
            // SAFETY: valid fd + NUL-terminated name; fd owned by the File below
            let raw = cvt(unsafe {
                libc::openat(dfd, tmp.as_ptr(), libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600 as libc::c_uint)
            })
            .map_err(ImportErr::Io)?;
            let mut out = unsafe { File::from_raw_fd(raw) };
            let r = (|| -> Result<(), ImportErr> {
                let mut buf = vec![0u8; 64 * 1024];
                let mut total: u64 = 0;
                loop {
                    let n = sf.read(&mut buf).map_err(ImportErr::Io)?;
                    if n == 0 {
                        break;
                    }
                    total += n as u64;
                    if total > cap {
                        return Err(ImportErr::TooBig(total));     // grew mid-copy
                    }
                    out.write_all(&buf[..n]).map_err(ImportErr::Io)?;
                    fault("import:copying").map_err(ImportErr::Io)?;
                }
                // SAFETY: valid fd
                cvt(unsafe { libc::fchmod(out.as_raw_fd(), (0o666 & !umask()) as libc::mode_t) }).map_err(ImportErr::Io)?;
                out.sync_all().map_err(ImportErr::Io)?;
                // VERIFY: the source did not change under us ...
                let st1 = fstat(sf.as_raw_fd()).map_err(ImportErr::Io)?;
                if st1.st_size != st0.st_size || (st1.st_mtime, st1.st_mtime_nsec) != (st0.st_mtime, st0.st_mtime_nsec) || total != st0.st_size as u64 {
                    return Err(ImportErr::Changed);
                }
                // ... and what is ON DISK is the source, byte for byte: both
                // re-read from offset 0 and compared whole (stronger than a
                // size + digest compare, and no hashing crate in the binary)
                if !same_bytes(&mut sf, &mut out).map_err(ImportErr::Io)? {
                    return Err(ImportErr::Io(io::Error::other("verify: the staged copy does not match the source")));
                }
                fault("import:staged").map_err(ImportErr::Io)
            })();
            drop(out);
            if let Err(e) = r {
                unstage();
                return Err(e);
            }
            "copy"
        };
        // 3. commit under the first free name — never over an existing one
        let mut committed = None;
        for cand in names {
            if cand.is_empty() || cand.contains('/') || cand.starts_with('.') {
                continue;
            }
            let cn = match cstr(OsStr::new(&cand)) {
                Ok(c) => c,
                Err(_) => continue,
            };
            // SAFETY: valid fd + NUL-terminated names
            let r = cvt(unsafe { libc::renameat2(dfd, tmp.as_ptr(), dfd, cn.as_ptr(), libc::RENAME_NOREPLACE) });
            let r = match r {
                Err(e) if matches!(e.raw_os_error(), Some(libc::EINVAL) | Some(libc::ENOSYS) | Some(libc::EOPNOTSUPP)) => {
                    // no NOREPLACE here: link(2) refuses an existing name too
                    cvt(unsafe { libc::linkat(dfd, tmp.as_ptr(), dfd, cn.as_ptr(), 0) }).map(|_| {
                        unstage();
                    })
                }
                r => r.map(|_| ()),
            };
            match r {
                Ok(()) => {
                    committed = Some(cand);
                    break;
                }
                Err(e) if e.raw_os_error() == Some(libc::EEXIST) => continue,
                Err(e) => {
                    unstage();
                    return Err(ImportErr::Io(e));
                }
            }
        }
        let Some(name) = committed else {
            unstage();
            return Err(ImportErr::NoFreeName);
        };
        // SAFETY: valid fd. The new name is durable before the source goes.
        let _ = unsafe { libc::fsync(dfd) };
        if keep_src {
            return Ok(Imported { name, src: SrcFate::Copied, how });
        }
        // 4. remove the source — only if the path still names the file we imported
        let fate = match fs_lstat(&csrc) {
            Ok(s) if (s.st_dev, s.st_ino) == (st0.st_dev, st0.st_ino) => {
                match fault("import:unlink-src").and_then(|_| cvt(unsafe { libc::unlink(csrc.as_ptr()) })) {
                    Ok(_) => {
                        if let Some(p) = src.parent() {
                            if let Ok(pd) = File::open(p) {
                                let _ = pd.sync_all();
                            }
                        }
                        SrcFate::Removed
                    }
                    Err(e) => SrcFate::Kept(e.to_string()),
                }
            }
            Ok(_) => SrcFate::Kept("the source was replaced during the move".into()),
            Err(e) => SrcFate::Kept(e.to_string()),
        };
        Ok(Imported { name, src: fate, how })
    }
}

/* ---- td-vault-io: the plugin `fs` / DataAdapter backend ------------------
   A community plugin's `fs.readFile("/vault/x")` / `adapter.writeBinary(..)`
   lands here as ONE command (main.rs plugin_vfs -> plugin_op). The JS side
   (ui/plugins/api.js) maps the virtual root `/vault` to "" and is NOT the
   guard: plugins share the page realm, so anything they can reach in JS they
   can also hand to invoke directly. Every rule is therefore enforced here:
     - plugin_rel refuses, before any syscall: NUL, a backslash (Windows /
       UNC forms), a drive letter, percent-encoded `.` `/` `\` `%` NUL, an
       absolute path, `.` / `..` components, an over-long path;
     - every component is then walked from the held vault fd with
       openat(O_NOFOLLOW|O_DIRECTORY) (Vault::walk), and every leaf is used
       relative to its parent fd with O_NOFOLLOW / AT_SYMLINK_NOFOLLOW, so a
       symlink at ANY position is refused, never followed;
     - only regular files are read/appended/copied (a FIFO or device node
       inside the vault is refused by fstat on the opened fd), only regular
       files and directories are stat'ed, renamed or removed.
   A refusal is io::ErrorKind::PermissionDenied with no OS errno (`refused`),
   which plugin_err turns into the stable "EPERM: vault-escape refused, ..." */

pub const PLUGIN_READ_CAP: u64 = 64 << 20;
/// plugin chmod sets permission bits only: no setuid, setgid or sticky
pub const PLUGIN_MODE_MASK: u32 = 0o777;
const PLUGIN_PATH_MAX: usize = 4096;

/// a plugin-supplied vault-relative path -> checked relative PathBuf ("" = the
/// vault root). Lexical refusals only; the fd walk does the rest.
pub fn plugin_rel(s: &str) -> io::Result<PathBuf> {
    let r = |why: &str| refused(Path::new(&s.replace('\0', "\\0")), why);
    if s.len() > PLUGIN_PATH_MAX {
        return Err(r("path too long"));
    }
    if s.contains('\0') {
        return Err(r("NUL in path"));
    }
    if s.contains('\\') {
        return Err(r("backslash (Windows/UNC form)"));
    }
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Err(r("drive-letter path"));
    }
    let low = s.to_ascii_lowercase();
    if ["%2e", "%2f", "%5c", "%00", "%25"].iter().any(|p| low.contains(p)) {
        return Err(r("percent-encoded traversal"));
    }
    if s.starts_with('/') {
        return Err(r("absolute host path"));
    }
    let mut out = PathBuf::new();
    for c in s.split('/') {
        match c {
            "" => {}
            "." | ".." => return Err(r("dot component")),
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// what stat answers: no inode, uid, mode or host path — synthetic fields only
#[derive(Debug, Clone, PartialEq)]
pub struct PStat {
    pub kind: &'static str, // "file" | "folder"
    pub size: u64,
    pub mtime_ms: i64,
    pub ctime_ms: i64,
}

fn pstat_of(rel: &Path, st: &libc::stat) -> io::Result<PStat> {
    let kind = match st.st_mode & libc::S_IFMT {
        libc::S_IFREG => "file",
        libc::S_IFDIR => "folder",
        libc::S_IFLNK => return Err(refused(rel, "symlink in vault path")),
        _ => return Err(refused(rel, "not a regular file or directory")),
    };
    let ms = |s: i64, ns: i64| s * 1000 + ns / 1_000_000;
    Ok(PStat {
        kind,
        size: if kind == "file" { st.st_size as u64 } else { 0 },
        mtime_ms: ms(st.st_mtime, st.st_mtime_nsec),
        ctime_ms: ms(st.st_ctime, st.st_ctime_nsec),
    })
}

impl Vault {
    /// the leaf's lstat relative to its held parent; "" = the root itself
    fn p_lstat(&self, rel: &Path) -> io::Result<libc::stat> {
        if rel.as_os_str().is_empty() {
            return fstat(self.root.fd.as_raw_fd());
        }
        let (d, leaf) = self.parent(rel, false)?;
        stat_at(d.fd.as_raw_fd(), &leaf)
    }

    pub fn p_stat(&self, rel: &Path) -> io::Result<PStat> {
        pstat_of(rel, &self.p_lstat(rel)?)
    }

    /// open the regular file `rel` (O_NOFOLLOW, non-blocking so a FIFO cannot
    /// hang us) and check the OPENED fd is a regular file
    fn p_open(&self, rel: &Path, flags: libc::c_int) -> io::Result<File> {
        let (d, leaf) = self.parent(rel, false)?;
        // a FIFO/device/socket/symlink already there is refused BEFORE the open
        // (opening a FIFO for write with no reader is ENXIO, not a refusal)
        match stat_at(d.fd.as_raw_fd(), &leaf) {
            Ok(st) if is_reg(&st) => {}
            Ok(st) => return Err(pstat_of(rel, &st).err().unwrap_or_else(|| refused(rel, "not a regular file"))),
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
            Err(e) => return Err(e),
        }
        // SAFETY: valid fd + NUL-terminated name; fd owned by the File below
        let raw = cvt(unsafe {
            libc::openat(d.fd.as_raw_fd(), leaf.as_ptr(), flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK, 0o666 as libc::c_uint)
        })
        .map_err(|e| self.leaf_why(rel, e))?;
        let f = unsafe { File::from_raw_fd(raw) };
        if !is_reg(&fstat(f.as_raw_fd())?) {
            return Err(refused(rel, "not a regular file"));
        }
        Ok(f)
    }

    /// ELOOP on an O_NOFOLLOW leaf open = the leaf is a symlink
    fn leaf_why(&self, rel: &Path, e: io::Error) -> io::Error {
        match e.raw_os_error() {
            Some(libc::ELOOP) => refused(rel, "symlink in vault path"),
            _ => e,
        }
    }

    pub fn p_read(&self, rel: &Path, cap: u64) -> io::Result<Vec<u8>> {
        let mut f = self.p_open(rel, libc::O_RDONLY)?;
        let n = fstat(f.as_raw_fd())?.st_size as u64;
        if n > cap {
            return Err(io::Error::new(io::ErrorKind::Other, format!("file too large ({n} > {cap} bytes)")));
        }
        let mut v = Vec::with_capacity(n as usize);
        (&mut f).take(cap + 1).read_to_end(&mut v)?;
        if v.len() as u64 > cap {
            return Err(io::Error::new(io::ErrorKind::Other, "file too large"));
        }
        Ok(v)
    }

    /// writeFile (atomic replace) or appendFile; like Node, parents are NOT
    /// created (ENOENT)
    pub fn p_write(&self, rel: &Path, bytes: &[u8], append: bool) -> io::Result<()> {
        let (_d, _leaf) = self.parent(rel, false)?;
        if !append {
            return self.write_atomic(rel, bytes, false);
        }
        let mut f = self.p_open(rel, libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT)?;
        f.write_all(bytes)
    }

    /// directory entries (name, "file"|"folder"); symlinks, devices, FIFOs and
    /// sockets are not listed — nothing a plugin could follow out
    pub fn p_list(&self, rel: &Path) -> io::Result<Vec<(String, &'static str)>> {
        let d = self.dir(rel, false)?;
        let dup = d.fd.try_clone()?;
        // SAFETY: fdopendir takes ownership of the dup'd fd; closedir frees both
        let dp = unsafe { libc::fdopendir(dup.as_raw_fd()) };
        if dp.is_null() {
            return Err(io::Error::last_os_error());
        }
        std::mem::forget(dup);
        let mut out = Vec::new();
        loop {
            // SAFETY: dp is a valid DIR*; the entry is copied out before the next call
            let e = unsafe { libc::readdir(dp) };
            if e.is_null() {
                break;
            }
            let name = unsafe { std::ffi::CStr::from_ptr((*e).d_name.as_ptr()) }.to_owned();
            if name.as_bytes() == b"." || name.as_bytes() == b".." {
                continue;
            }
            let kind = match stat_at(d.fd.as_raw_fd(), &name) {
                Ok(st) if is_reg(&st) => "file",
                Ok(st) if st.st_mode & libc::S_IFMT == libc::S_IFDIR => "folder",
                _ => continue,
            };
            out.push((String::from_utf8_lossy(name.as_bytes()).into_owned(), kind));
        }
        // SAFETY: dp from fdopendir above
        unsafe { libc::closedir(dp) };
        out.sort();
        Ok(out)
    }

    pub fn p_mkdir(&self, rel: &Path, recursive: bool) -> io::Result<()> {
        if recursive {
            return self.mkdir_p(rel);
        }
        let (d, leaf) = self.parent(rel, false)?;
        if let Ok(st) = stat_at(d.fd.as_raw_fd(), &leaf) {
            pstat_of(rel, &st)?; // a symlink there is a refusal, not EEXIST
            return Err(io::Error::from_raw_os_error(libc::EEXIST));
        }
        // SAFETY: valid fd + NUL-terminated name
        cvt(unsafe { libc::mkdirat(d.fd.as_raw_fd(), leaf.as_ptr(), 0o777) }).map(|_| ())
    }

    /// unlink a regular file, or rmdir an EMPTY directory (`dir`)
    pub fn p_remove(&self, rel: &Path, dir: bool) -> io::Result<()> {
        let (d, leaf) = self.parent(rel, false)?;
        let st = pstat_of(rel, &stat_at(d.fd.as_raw_fd(), &leaf)?)?;
        let flag = match (st.kind, dir) {
            ("file", _) => 0,
            ("folder", true) => libc::AT_REMOVEDIR,
            _ => return Err(io::Error::from_raw_os_error(libc::EISDIR)),
        };
        // SAFETY: valid fd + NUL-terminated name
        cvt(unsafe { libc::unlinkat(d.fd.as_raw_fd(), leaf.as_ptr(), flag) }).map(|_| ())
    }

    /// rename a regular file or directory; an existing regular-file target is
    /// replaced (Node semantics), a symlink target is refused. Both parents are
    /// held fds inside the vault, so the entry can only move within it.
    pub fn p_rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let (fd_from, lf) = self.parent(from, false)?;
        pstat_of(from, &stat_at(fd_from.fd.as_raw_fd(), &lf)?)?;
        let (fd_to, lt) = self.parent(to, false)?;
        match stat_at(fd_to.fd.as_raw_fd(), &lt) {
            Ok(st) => {
                pstat_of(to, &st)?;
            }
            Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
            Err(e) => return Err(e),
        }
        // SAFETY: valid fds + NUL-terminated names
        cvt(unsafe { libc::renameat(fd_from.fd.as_raw_fd(), lf.as_ptr(), fd_to.fd.as_raw_fd(), lt.as_ptr()) }).map(|_| ())
    }

    /// copyFile: the source's bytes by fd, then an atomic replace of `to`
    pub fn p_copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        let bytes = self.p_read(from, PLUGIN_READ_CAP)?;
        self.p_write(to, &bytes, false)
    }

    /// fs.chmod for plugins (obsidian-git marks its askpass helper executable).
    /// The leaf is opened O_PATH|O_NOFOLLOW relative to its held parent: on a
    /// symlink that yields an fd to the LINK itself, whose fstat is S_IFLNK and
    /// is refused — the target is never reached. Only a regular file or a
    /// directory is changed, through /proc/self/fd/<fd>, which names exactly the
    /// inode we hold (fchmod on an O_PATH fd is EBADF), so a swap after the open
    /// cannot redirect it. setuid/setgid/sticky never get through: mode & 0o777.
    pub fn p_chmod(&self, rel: &Path, mode: u32) -> io::Result<()> {
        let (d, leaf) = self.parent(rel, false)?;
        // SAFETY: valid fd + NUL-terminated name; the fd is owned below
        let raw = cvt(unsafe { libc::openat(d.fd.as_raw_fd(), leaf.as_ptr(), libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC) })?;
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        pstat_of(rel, &fstat(fd.as_raw_fd())?)?; // symlink, FIFO, device, socket -> refused
        hook("p_chmod:opened");
        let via = CString::new(format!("/proc/self/fd/{}", fd.as_raw_fd())).expect("no NUL in a decimal fd");
        // SAFETY: NUL-terminated path; the magic link resolves to the inode `fd` pins
        cvt(unsafe { libc::chmod(via.as_ptr(), (mode & PLUGIN_MODE_MASK) as libc::mode_t) }).map(|_| ())
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len().div_ceil(3) * 4);
    for c in b.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { B64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim_end_matches('=');
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in s.bytes() {
        let v = B64.iter().position(|&x| x == ch)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// an io::Error -> "CODE: message" for the JS facade (err.code = CODE). A
/// refusal (PermissionDenied with no errno) is the stable vault-escape error.
pub fn plugin_err(op: &str, path: &str, e: &io::Error) -> String {
    let p = path.replace('\0', "\\0");
    if e.kind() == io::ErrorKind::PermissionDenied && e.raw_os_error().is_none() {
        return format!("EPERM: vault-escape refused, {op} '{p}' ({e})");
    }
    let code = match e.raw_os_error() {
        Some(libc::ENOENT) => "ENOENT",
        Some(libc::EEXIST) => "EEXIST",
        Some(libc::ENOTDIR) => "ENOTDIR",
        Some(libc::EISDIR) => "EISDIR",
        Some(libc::ENOTEMPTY) => "ENOTEMPTY",
        Some(libc::EINVAL) => "EINVAL",
        Some(libc::EACCES) => "EACCES",
        Some(libc::ENAMETOOLONG) => "ENAMETOOLONG",
        Some(libc::EPERM) => "EPERM",
        Some(libc::ELOOP) => "ELOOP",
        _ if e.kind() == io::ErrorKind::NotFound => "ENOENT",
        _ if e.kind() == io::ErrorKind::AlreadyExists => "EEXIST",
        _ => "EIO",
    };
    format!("{code}: {e}, {op} '{p}'")
}

/// the whole plugin fs surface, one op at a time. `data` is base64 (bytes
/// cross the IPC as text; binary and UTF-8 alike). Ops not named here do not
/// exist: no fd, stream, watch, chown, link or realpath. chmod is not an op
/// here either: it is plugin_chmod, which checks the vault's trust itself.
pub fn plugin_op(root: &Path, op: &str, path: &str, to: Option<&str>, data: Option<&str>, recursive: bool) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let e = |x: io::Error| plugin_err(op, path, &x);
    let rel = plugin_rel(path).map_err(e)?;
    let need = |rel: &Path| if rel.as_os_str().is_empty() { Err(plugin_err(op, path, &refused(rel, "the vault root itself"))) } else { Ok(()) };
    let vfs = Vault::open(root).map_err(e)?;
    let bytes = || data.and_then(b64_decode).ok_or_else(|| format!("EINVAL: data is not base64, {op} '{path}'"));
    let dest = || -> Result<PathBuf, String> {
        let t = to.ok_or_else(|| format!("EINVAL: missing destination, {op} '{path}'"))?;
        let r = plugin_rel(t).map_err(|x| plugin_err(op, t, &x))?;
        if r.as_os_str().is_empty() {
            return Err(plugin_err(op, t, &refused(&r, "the vault root itself")));
        }
        Ok(r)
    };
    match op {
        "stat" => vfs.p_stat(&rel).map(|s| json!({"type": s.kind, "size": s.size, "mtime": s.mtime_ms, "ctime": s.ctime_ms})).map_err(e),
        "exists" => match vfs.p_stat(&rel) {
            Ok(_) => Ok(json!(true)),
            Err(x) if x.kind() == io::ErrorKind::NotFound || matches!(x.raw_os_error(), Some(libc::ENOENT) | Some(libc::ENOTDIR)) => Ok(json!(false)),
            Err(x) => Err(e(x)),
        },
        "read" => { need(&rel)?; vfs.p_read(&rel, PLUGIN_READ_CAP).map(|b| json!(b64_encode(&b))).map_err(e) }
        "write" | "append" => { need(&rel)?; vfs.p_write(&rel, &bytes()?, op == "append").map(|_| json!(null)).map_err(e) }
        "list" => vfs.p_list(&rel).map(|v| json!(v.into_iter().map(|(n, k)| json!([n, k])).collect::<Vec<_>>())).map_err(e),
        "mkdir" => { need(&rel)?; vfs.p_mkdir(&rel, recursive).map(|_| json!(null)).map_err(e) }
        "unlink" | "rmdir" => { need(&rel)?; vfs.p_remove(&rel, op == "rmdir").map(|_| json!(null)).map_err(e) }
        "rename" => { need(&rel)?; let t = dest()?; vfs.p_rename(&rel, &t).map(|_| json!(null)).map_err(e) }
        "copy" => { need(&rel)?; let t = dest()?; vfs.p_copy(&rel, &t).map(|_| json!(null)).map_err(e) }
        _ => Err(format!("ENOSYS: fs op {op:?} is not available in opensidian")),
    }
}

/// fs.chmod for plugins — TRUSTED vaults only (operator ruling 2026-10-20).
/// Refused EPERM (not ENOSYS) in restricted mode, checked here as well as at
/// the caller; the path is confined exactly like every plugin_op (plugin_rel +
/// the O_NOFOLLOW fd walk); the vault root itself is refused; the leaf is
/// changed only if it is a regular file or directory, never through a symlink
/// (Vault::p_chmod); the mode is masked to 0o777.
pub fn plugin_chmod(root: &Path, trusted: bool, path: &str, mode: u32) -> Result<serde_json::Value, String> {
    let e = |x: io::Error| plugin_err("chmod", path, &x);
    if !trusted {
        return Err(format!("EPERM: restricted mode, plugin chmod is off, chmod '{}'", path.replace('\0', "\\0")));
    }
    let rel = plugin_rel(path).map_err(e)?;
    if rel.as_os_str().is_empty() {
        return Err(e(refused(&rel, "the vault root itself")));
    }
    Vault::open(root).map_err(e)?.p_chmod(&rel, mode & PLUGIN_MODE_MASK).map(|_| serde_json::Value::Null).map_err(e)
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
/// non-hidden real directory's vault-relative path, depth first. Real dirs
/// cannot cycle; MAX_WALK_DEPTH still bounds the recursion (stack) against
/// a deliberately deep tree.
pub fn walk_real_dirs(dir: &Path, base: &Path, out: &mut Vec<String>) {
    walk_real_dirs_at(dir, base, out, 0)
}

pub const MAX_WALK_DEPTH: usize = 256;

fn walk_real_dirs_at(dir: &Path, base: &Path, out: &mut Vec<String>, depth: usize) {
    if depth >= MAX_WALK_DEPTH {
        return;
    }
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
        walk_real_dirs_at(&p, base, out, depth + 1);
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
    fn vaultfs_deep_real_tree_walk_is_depth_bounded() {
        let (root, _out) = fresh("deep");
        let mut d = root.clone();
        for _ in 0..(MAX_WALK_DEPTH + 10) {
            d.push("d");
        }
        fs::create_dir_all(&d).unwrap();
        let mut v = Vec::new();
        walk_real_dirs(&root, &root, &mut v);
        assert_eq!(v.len(), MAX_WALK_DEPTH, "one entry per level, capped");
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

    // ---- td-vault-io: the plugin fs backend ----
    fn pop(root: &Path, op: &str, p: &str, to: Option<&str>, data: Option<&[u8]>) -> Result<serde_json::Value, String> {
        let d = data.map(b64_encode);
        plugin_op(root, op, p, to, d.as_deref(), false)
    }

    #[test]
    fn pvfs_base64_round_trips_every_length() {
        for n in 0..70usize {
            let b: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&b)).unwrap(), b, "len {n}");
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert!(b64_decode("@@").is_none());
    }

    #[test]
    fn pvfs_positive_ops_stay_inside_the_vault() {
        let (root, _out) = fresh("pvfs-pos");
        let bin: Vec<u8> = (0..=255u8).collect();
        pop(&root, "write", "b.bin", None, Some(&bin)).unwrap();
        assert_eq!(fs::read(root.join("b.bin")).unwrap(), bin);
        let got = pop(&root, "read", "b.bin", None, None).unwrap();
        assert_eq!(b64_decode(got.as_str().unwrap()).unwrap(), bin);
        pop(&root, "write", "u.txt", None, Some("héllo ✓".as_bytes())).unwrap();
        pop(&root, "append", "u.txt", None, Some(b"!")).unwrap();
        assert_eq!(fs::read_to_string(root.join("u.txt")).unwrap(), "héllo ✓!");
        pop(&root, "append", "new.txt", None, Some(b"a")).unwrap();
        assert_eq!(fs::read(root.join("new.txt")).unwrap(), b"a");
        let st = pop(&root, "stat", "u.txt", None, None).unwrap();
        assert_eq!((st["type"].as_str(), st["size"].as_u64()), (Some("file"), Some("héllo ✓!".len() as u64)));
        assert!(st["mtime"].as_i64().unwrap() > 1_600_000_000_000);
        assert_eq!(pop(&root, "stat", "", None, None).unwrap()["type"], "folder");
        plugin_op(&root, "mkdir", "d/e", None, None, true).unwrap();
        assert!(pop(&root, "mkdir", "x/y", None, None).unwrap_err().starts_with("ENOENT"));
        assert!(pop(&root, "mkdir", "d", None, None).unwrap_err().starts_with("EEXIST"));
        assert!(pop(&root, "write", "nope/f", None, Some(b"x")).unwrap_err().starts_with("ENOENT"), "no implicit parents");
        pop(&root, "rename", "u.txt", Some("d/e/u2.txt"), None).unwrap();
        pop(&root, "copy", "d/e/u2.txt", Some("d/c.txt"), None).unwrap();
        assert_eq!(fs::read(root.join("d/c.txt")).unwrap(), fs::read(root.join("d/e/u2.txt")).unwrap());
        let l = pop(&root, "list", "d", None, None).unwrap();
        assert_eq!(l, serde_json::json!([["c.txt", "file"], ["e", "folder"]]));
        assert_eq!(pop(&root, "exists", "d/c.txt", None, None).unwrap(), true);
        assert_eq!(pop(&root, "exists", "d/zz", None, None).unwrap(), false);
        assert!(pop(&root, "rmdir", "d", None, None).unwrap_err().starts_with("ENOTEMPTY"));
        assert!(pop(&root, "unlink", "d", None, None).unwrap_err().starts_with("EISDIR"));
        pop(&root, "unlink", "d/c.txt", None, None).unwrap();
        pop(&root, "unlink", "d/e/u2.txt", None, None).unwrap();
        pop(&root, "rmdir", "d/e", None, None).unwrap();
        assert!(!root.join("d/e").exists());
        assert!(pop(&root, "open", "b.bin", None, None).unwrap_err().starts_with("ENOSYS"));
    }

    #[test]
    fn pvfs_every_escape_form_is_a_stable_refusal() {
        let (root, out) = fresh("pvfs-neg");
        fs::write(out.join("sentinel"), b"S").unwrap();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/b/f.txt"), b"in").unwrap();
        let o = out.display().to_string();
        let bad = [
            "../outside/sentinel".to_string(),
            "a/../../outside/sentinel".into(),
            o.clone() + "/sentinel",
            "/etc/passwd".into(),
            "/dev/null".into(),
            "%2e%2e/outside/sentinel".into(),
            "%2E%2E%2Foutside%2Fsentinel".into(),
            "C:/Windows/win.ini".into(),
            "c:sentinel".into(),
            "\\\\host\\share\\x".into(),
            "..\\outside\\sentinel".into(),
            "a/b\0/f.txt".into(),
            "./a/b/f.txt".into(),
        ];
        for p in &bad {
            for op in ["read", "write", "append", "stat", "list", "mkdir", "unlink", "rmdir", "exists"] {
                let r = pop(&root, op, p, None, Some(b"X"));
                let er = r.expect_err(&format!("{op} {p:?} must be refused"));
                assert!(er.starts_with("EPERM: vault-escape refused"), "{op} {p:?}: {er}");
            }
        }
        // a symlink at each traversed position: first dir, middle dir, leaf
        symlink(&out, root.join("L1")).unwrap();
        symlink(&out, root.join("a/L2")).unwrap();
        symlink(out.join("sentinel"), root.join("a/b/L3")).unwrap();
        for p in ["L1/sentinel", "a/L2/sentinel", "a/b/L3", "L1", "a/L2"] {
            for op in ["read", "write", "append", "stat", "mkdir", "unlink", "rmdir", "exists", "list"] {
                if op == "list" && p == "a/b/L3" { continue; }
                let er = pop(&root, op, p, None, Some(b"X")).expect_err(&format!("{op} {p}"));
                assert!(er.starts_with("EPERM: vault-escape refused"), "{op} {p}: {er}");
            }
        }
        // rename / copy with an escaping source or destination
        for (a, b) in [("a/b/f.txt", "../outside/x"), ("a/b/f.txt", "L1/x"), ("a/b/f.txt", "a/b/L3"), ("../outside/sentinel", "got"), ("L1/sentinel", "got"), ("a/b/L3", "got"), ("a/b/f.txt", "/tmp/x"), ("a/b/f.txt", "%2e%2e/x")] {
            for op in ["rename", "copy"] {
                let er = pop(&root, op, a, Some(b), None).expect_err(&format!("{op} {a} {b}"));
                assert!(er.starts_with("EPERM: vault-escape refused"), "{op} {a} -> {b}: {er}");
            }
        }
        assert_eq!(fs::read(out.join("sentinel")).unwrap(), b"S", "sentinel unchanged");
        assert_eq!(fs::read_dir(&out).unwrap().count(), 1, "nothing created outside");
        assert_eq!(fs::read(root.join("a/b/f.txt")).unwrap(), b"in");
        assert!(fs::symlink_metadata(root.join("a/b/L3")).unwrap().file_type().is_symlink());
        // symlinks are not listed (nothing to follow out)
        let l = pop(&root, "list", "a", None, None).unwrap();
        assert_eq!(l, serde_json::json!([["b", "folder"]]));
    }

    #[test]
    fn pvfs_fifo_is_not_a_regular_file() {
        let (root, _out) = fresh("pvfs-fifo");
        let c = CString::new(root.join("q").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        for op in ["read", "append", "stat"] {
            let er = pop(&root, op, "q", None, Some(b"x")).unwrap_err();
            assert!(er.starts_with("EPERM: vault-escape refused"), "{op}: {er}");
        }
    }

    // ---- git-chmod: plugin fs.chmod (trusted vaults only) ----
    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn pchmod_regular_file_mode_round_trips() {
        let (root, _out) = fresh("pchmod-reg");
        fs::create_dir_all(root.join("d")).unwrap();
        fs::write(root.join("d/askpass.sh"), b"#!/bin/sh\n").unwrap();
        plugin_chmod(&root, true, "d/askpass.sh", 0o755).unwrap();
        assert_eq!(mode_of(&root.join("d/askpass.sh")), 0o755);
        plugin_chmod(&root, true, "d/askpass.sh", 0o644).unwrap();
        assert_eq!(mode_of(&root.join("d/askpass.sh")), 0o644);
        assert!(plugin_chmod(&root, true, "d/nope.sh", 0o755).unwrap_err().starts_with("ENOENT"));
    }

    #[test]
    fn pchmod_setuid_setgid_sticky_stripped() {
        let (root, _out) = fresh("pchmod-suid");
        fs::write(root.join("f"), b"x").unwrap();
        plugin_chmod(&root, true, "f", 0o4755).unwrap();
        assert_eq!(mode_of(&root.join("f")), 0o755, "setuid stripped");
        plugin_chmod(&root, true, "f", 0o7777).unwrap();
        assert_eq!(mode_of(&root.join("f")), 0o777, "setuid/setgid/sticky stripped");
        plugin_chmod(&root, true, "f", 0o2644 | 0o10000).unwrap();
        assert_eq!(mode_of(&root.join("f")), 0o644, "setgid + high bits stripped");
    }

    #[test]
    fn pchmod_symlink_to_inside_vault_file_refused() {
        let (root, _out) = fresh("pchmod-lin");
        fs::write(root.join("real.sh"), b"x").unwrap();
        fs::set_permissions(root.join("real.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        symlink(root.join("real.sh"), root.join("link.sh")).unwrap();
        symlink("real.sh", root.join("rel.sh")).unwrap();
        for p in ["link.sh", "rel.sh"] {
            let er = plugin_chmod(&root, true, p, 0o777).expect_err(p);
            assert!(er.starts_with("EPERM: vault-escape refused"), "{p}: {er}");
        }
        assert_eq!(mode_of(&root.join("real.sh")), 0o600, "target unchanged");
    }

    #[test]
    fn pchmod_symlink_to_outside_vault_file_refused() {
        let (root, out) = fresh("pchmod-lout");
        fs::write(out.join("secret"), b"S").unwrap();
        fs::set_permissions(out.join("secret"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        symlink(out.join("secret"), root.join("a/leak")).unwrap();
        symlink(&out, root.join("L")).unwrap();
        for p in ["a/leak", "L/secret", "L"] {
            let er = plugin_chmod(&root, true, p, 0o777).expect_err(p);
            assert!(er.starts_with("EPERM: vault-escape refused"), "{p}: {er}");
        }
        assert_eq!(mode_of(&out.join("secret")), 0o600, "outside target mode unchanged");
        assert!(fs::symlink_metadata(root.join("a/leak")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn pchmod_dangling_symlink_refused() {
        let (root, out) = fresh("pchmod-dang");
        symlink(out.join("does-not-exist"), root.join("dang")).unwrap();
        symlink("nowhere", root.join("dang2")).unwrap();
        for p in ["dang", "dang2"] {
            let er = plugin_chmod(&root, true, p, 0o755).expect_err(p);
            assert!(er.starts_with("EPERM: vault-escape refused"), "{p}: {er}");
        }
        assert!(!out.join("does-not-exist").exists(), "nothing created through the link");
    }

    #[test]
    fn pchmod_dotdot_and_absolute_refused() {
        let (root, out) = fresh("pchmod-esc");
        fs::write(out.join("sentinel"), b"S").unwrap();
        fs::set_permissions(out.join("sentinel"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let abs = out.join("sentinel").display().to_string();
        for p in ["../outside/sentinel", "a/../../outside/sentinel", abs.as_str(), "/etc/passwd", "%2e%2e/outside/sentinel", "..\\outside\\sentinel", "./x", "", "/"] {
            let er = plugin_chmod(&root, true, p, 0o777).expect_err(p);
            assert!(er.starts_with("EPERM: vault-escape refused"), "{p:?}: {er}");
        }
        assert_eq!(mode_of(&out.join("sentinel")), 0o600, "outside file unchanged");
    }

    #[test]
    fn pchmod_directory_works() {
        let (root, _out) = fresh("pchmod-dir");
        fs::create_dir_all(root.join("d/e")).unwrap();
        plugin_chmod(&root, true, "d/e", 0o700).unwrap();
        assert_eq!(mode_of(&root.join("d/e")), 0o700);
        plugin_chmod(&root, true, "d/e", 0o3755).unwrap();
        assert_eq!(mode_of(&root.join("d/e")), 0o755, "dir: setgid/sticky stripped");
        // a FIFO is neither file nor directory: refused
        let c = CString::new(root.join("q").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        assert!(plugin_chmod(&root, true, "q", 0o777).unwrap_err().starts_with("EPERM: vault-escape refused"));
        assert_eq!(mode_of(&root.join("q")), 0o600);
    }

    #[test]
    fn pchmod_untrusted_vault_refused() {
        let (root, _out) = fresh("pchmod-untr");
        fs::write(root.join("f"), b"x").unwrap();
        fs::set_permissions(root.join("f"), std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        let er = plugin_chmod(&root, false, "f", 0o755).unwrap_err();
        assert!(er.starts_with("EPERM: restricted mode"), "{er}");
        assert!(!er.starts_with("ENOSYS"));
        assert_eq!(mode_of(&root.join("f")), 0o644, "untrusted: unchanged");
        // and chmod is not reachable through the trust-agnostic plugin_op
        assert!(pop(&root, "chmod", "f", None, None).unwrap_err().starts_with("ENOSYS"));
        assert_eq!(mode_of(&root.join("f")), 0o644);
    }

    #[test]
    fn pchmod_parent_swapped_for_symlink_after_open_still_hits_held_inode() {
        let (root, out) = fresh("pchmod-race");
        fs::create_dir_all(root.join("d")).unwrap();
        fs::write(root.join("d/f"), b"x").unwrap();
        fs::write(out.join("f"), b"S").unwrap();
        fs::set_permissions(out.join("f"), std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();
        let (r2, o2) = (root.clone(), out.clone());
        set_hook(Some(Box::new(move |at| {
            if at == "p_chmod:opened" {
                fs::rename(r2.join("d"), r2.join("d.moved")).unwrap();
                symlink(&o2, r2.join("d")).unwrap();
            }
        })));
        let r = plugin_chmod(&root, true, "d/f", 0o755);
        set_hook(None);
        r.unwrap();
        assert_eq!(mode_of(&out.join("f")), 0o600, "outside file never touched");
        assert_eq!(mode_of(&root.join("d.moved/f")), 0o755, "the held inode got the mode");
    }
}
