// SPDX-License-Identifier: GPL-3.0-or-later
/* vaultfs (Windows) — same public API as unix.rs.

   Linux uses openat(O_NOFOLLOW) fd walks. Here each vault-relative component is
   opened with FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS so a
   reparse point (symlink/junction) is never followed; FILE_ATTRIBUTE_REPARSE_POINT
   on the opened object is refused. Parent identity uses volume serial + file
   index (BY_HANDLE_FILE_INFORMATION via MetadataExt). Import always copies
   (no hardlink path). p_chmod is a documented no-op. */
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
// No FILE_SHARE_DELETE: a held directory cannot be renamed/deleted out from under us.
const FILE_SHARE_READ: u32 = 0x0000_0001;
const FILE_SHARE_WRITE: u32 = 0x0000_0002;

// Compiled under debug_assertions too (not just cfg(test)): since lib.rs the
// bin's tests (main.rs) link vaultfs as a DEPENDENCY, where cfg(test) is off.
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

#[derive(Debug, PartialEq)]
pub enum SrcFate {
    Removed,
    Kept(String),
    Copied,
}

#[derive(Debug)]
pub enum ImportErr {
    OpenSrc(io::Error),
    NotRegular,
    TooBig(u64),
    BadMagic,
    Changed,
    NoFreeName,
    Io(io::Error),
}

#[derive(Debug)]
pub struct Imported {
    pub name: String,
    pub src: SrcFate,
    pub how: &'static str,
}

fn refused(rel: &Path, why: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("vault-escape refused ({}): {}", why, rel.display()),
    )
}

fn is_reparse(meta: &fs::Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn is_dir_attr(meta: &fs::Metadata) -> bool {
    meta.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

fn open_nofollow(path: &Path, write: bool) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true);
    if write {
        o.write(true);
    }
    o.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    o.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
    let f = o.open(path)?;
    let meta = f.metadata()?;
    if is_reparse(&meta) {
        return Err(refused(path, "reparse point in vault path"));
    }
    Ok(f)
}

fn open_dir_nofollow(path: &Path) -> io::Result<File> {
    let f = open_nofollow(path, false)?;
    let meta = f.metadata()?;
    if !is_dir_attr(&meta) {
        return Err(refused(path, "symlink or non-directory in vault path"));
    }
    Ok(f)
}

#[repr(C)]
struct ByHandleFileInformation {
    file_attributes: u32,
    creation_time: [u32; 2],
    last_access_time: [u32; 2],
    last_write_time: [u32; 2],
    volume_serial_number: u32,
    file_size_high: u32,
    file_size_low: u32,
    number_of_links: u32,
    file_index_high: u32,
    file_index_low: u32,
}

fn file_id_handle(f: &File) -> io::Result<(u32, u64)> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(h: *mut core::ffi::c_void, info: *mut ByHandleFileInformation) -> i32;
    }
    let mut info = std::mem::MaybeUninit::<ByHandleFileInformation>::uninit();
    let ok = unsafe { GetFileInformationByHandle(f.as_raw_handle(), info.as_mut_ptr()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    let index = ((info.file_index_high as u64) << 32) | info.file_index_low as u64;
    Ok((info.volume_serial_number, index))
}

fn file_id_path(path: &Path) -> io::Result<(u32, u64)> {
    let f = open_nofollow(path, false)?;
    file_id_handle(&f)
}

fn parts(rel: &Path) -> io::Result<Vec<&std::ffi::OsStr>> {
    let mut out = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => return Err(refused(rel, "bad path component")),
        }
    }
    Ok(out)
}

fn tmp_name(leaf: &std::ffi::OsStr) -> io::Result<String> {
    let mut buf = [0u8; 8];
    getrandom_fill(&mut buf)?;
    let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(".os-{hex}-{}", leaf.to_string_lossy()))
}

fn getrandom_fill(buf: &mut [u8]) -> io::Result<()> {
    // RtlGenRandom (SystemFunction036) — no new crate; already linked via the
    // Windows SDK. Temp sibling names must not be guessable across processes.
    #[link(name = "advapi32")]
    extern "system" {
        fn SystemFunction036(buf: *mut u8, len: u32) -> u8;
    }
    let ok = unsafe { SystemFunction036(buf.as_mut_ptr(), buf.len() as u32) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn ms_since_epoch(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn starts_with_at0(f: &mut File, magic: &[u8]) -> io::Result<bool> {
    let pos = f.stream_position()?;
    f.seek(SeekFrom::Start(0))?;
    let mut buf = vec![0u8; magic.len()];
    let n = f.read(&mut buf)?;
    f.seek(SeekFrom::Start(pos))?;
    Ok(n == magic.len() && buf == magic)
}

fn same_bytes(a: &mut File, b: &mut File) -> io::Result<bool> {
    a.seek(SeekFrom::Start(0))?;
    b.seek(SeekFrom::Start(0))?;
    let mut ba = [0u8; 64 * 1024];
    let mut bb = [0u8; 64 * 1024];
    loop {
        let na = a.read(&mut ba)?;
        let nb = b.read(&mut bb)?;
        if na != nb || ba[..na] != bb[..nb] {
            return Ok(false);
        }
        if na == 0 {
            return Ok(true);
        }
    }
}

/// Windows has no dir fd; Dir holds the verified absolute path plus open
/// handles for every walked component so a junction cannot be swapped in
/// (sharing without DELETE + live handles).
pub struct Dir {
    path: PathBuf,
    _held: Vec<File>,
}

pub struct Vault {
    root: PathBuf,
}

impl Vault {
    pub fn open(root: &Path) -> io::Result<Vault> {
        let croot = root.canonicalize()?;
        let _ = open_dir_nofollow(&croot)?;
        Ok(Vault { root: croot })
    }

    fn abs(&self, rel: &Path) -> PathBuf {
        if rel.as_os_str().is_empty() {
            self.root.clone()
        } else {
            self.root.join(rel)
        }
    }

    fn walk(&self, rel: &Path, comps: &[&std::ffi::OsStr], create: bool) -> io::Result<Dir> {
        let mut cur = self.root.clone();
        let mut held: Vec<File> = Vec::with_capacity(comps.len() + 1);
        held.push(open_dir_nofollow(&cur)?);
        for c in comps {
            cur.push(c);
            match open_dir_nofollow(&cur) {
                Ok(f) => held.push(f),
                Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
                    fs::create_dir(&cur).or_else(|e2| {
                        if e2.kind() == io::ErrorKind::AlreadyExists {
                            Ok(())
                        } else {
                            Err(e2)
                        }
                    })?;
                    held.push(open_dir_nofollow(&cur).map_err(|e| self.why(rel, e))?);
                }
                Err(e) => return Err(self.why(rel, e)),
            }
        }
        Ok(Dir { path: cur, _held: held })
    }

    fn why(&self, rel: &Path, e: io::Error) -> io::Error {
        if e.kind() == io::ErrorKind::PermissionDenied {
            refused(rel, "symlink or non-directory in vault path")
        } else {
            e
        }
    }

    pub fn dir(&self, rel: &Path, create: bool) -> io::Result<Dir> {
        if rel.as_os_str().is_empty() {
            return Ok(Dir {
                path: self.root.clone(),
                _held: vec![open_dir_nofollow(&self.root)?],
            });
        }
        let comps = parts(rel)?;
        self.walk(rel, &comps, create)
    }

    pub fn mkdir_p(&self, rel: &Path) -> io::Result<()> {
        self.dir(rel, true).map(|_| ())
    }

    fn parent(&self, rel: &Path, create: bool) -> io::Result<(Dir, std::ffi::OsString)> {
        let comps = parts(rel)?;
        let (leaf, dirs) = comps.split_last().ok_or_else(|| refused(rel, "empty path"))?;
        let d = self.walk(rel, dirs, create)?;
        Ok((d, (*leaf).to_owned()))
    }

    pub fn is_file(&self, rel: &Path) -> bool {
        let p = self.abs(rel);
        match open_nofollow(&p, false) {
            Ok(f) => f.metadata().map(|m| m.is_file() && !is_reparse(&m)).unwrap_or(false),
            Err(_) => false,
        }
    }

    /// read-only open of a vault-relative regular file (reparse points refused)
    pub fn open_read(&self, rel: &Path) -> io::Result<File> {
        let (d, leaf) = self.parent(rel, false)?;
        let full = d.path.join(&leaf);
        let f = open_nofollow(&full, false)?;
        let meta = f.metadata()?;
        if is_reparse(&meta) || !meta.is_file() {
            return Err(refused(rel, "not a regular file"));
        }
        Ok(f)
    }

    pub fn write_atomic(&self, rel: &Path, bytes: &[u8], durable: bool) -> io::Result<()> {
        let (d, leaf) = self.parent(rel, true)?;
        hook("write_atomic:resolved");
        if let Some(pr) = rel.parent().filter(|p| !p.as_os_str().is_empty()) {
            let again = self.dir(pr, false)?;
            let a = file_id_path(&d.path)?;
            let b = file_id_path(&again.path)?;
            if a != b {
                return Err(refused(rel, "parent directory changed during write"));
            }
        }
        let target = d.path.join(&leaf);
        if target.exists() {
            let meta = fs::symlink_metadata(&target)?;
            if is_reparse(&meta) {
                return Err(refused(rel, "target is not a regular file"));
            }
            if !meta.is_file() {
                return Err(refused(rel, "target is not a regular file"));
            }
        }
        let tmp = d.path.join(tmp_name(&leaf)?);
        let r = (|| -> io::Result<()> {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, &target)?;
            Ok(())
        })();
        if let Err(e) = r {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        if durable {
            let _ = open_dir_nofollow(&d.path).and_then(|f| f.sync_all());
        }
        Ok(())
    }

    pub fn create_new(&self, rel: &Path, parents: bool) -> io::Result<File> {
        let (d, leaf) = self.parent(rel, parents)?;
        hook("create_new:resolved");
        let p = d.path.join(&leaf);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&p)
    }

    pub fn unlink(&self, rel: &Path) -> io::Result<()> {
        let (d, leaf) = self.parent(rel, false)?;
        let p = d.path.join(&leaf);
        let meta = fs::symlink_metadata(&p)?;
        if is_reparse(&meta) || !meta.is_file() {
            return Err(refused(rel, "not a regular file"));
        }
        fs::remove_file(&p)
    }

    pub fn move_file(&self, from: &Path, to: &Path) -> io::Result<()> {
        let (fd_from, lf) = self.parent(from, false)?;
        let src = fd_from.path.join(&lf);
        let meta = fs::symlink_metadata(&src)?;
        if is_reparse(&meta) || !meta.is_file() {
            return Err(refused(from, "source is not a regular file"));
        }
        let (fd_to, lt) = self.parent(to, true)?;
        let dest = fd_to.path.join(&lt);
        drop(self.create_new(to, true)?);
        hook("move_file:claimed");
        if let Err(e) = fs::rename(&src, &dest) {
            let _ = fs::remove_file(&dest);
            return Err(e);
        }
        Ok(())
    }

    pub fn trash(&self, rel: &Path, tdir: &Path, stem: &str, ext: &str) -> io::Result<PathBuf> {
        let (fd_from, lf) = self.parent(rel, false)?;
        let src = fd_from.path.join(&lf);
        let meta = fs::symlink_metadata(&src)?;
        if is_reparse(&meta) || !meta.is_file() {
            return Err(refused(rel, "not a regular file"));
        }
        let t = self.dir(tdir, true)?;
        hook("trash:resolved");
        for i in 0..10_000u32 {
            let name = if i == 0 {
                format!("{stem}.{ext}")
            } else {
                format!("{stem}.{i}.{ext}")
            };
            let dest = t.path.join(&name);
            match OpenOptions::new().write(true).create_new(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(&dest) {
                Ok(f) => drop(f),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
            if let Err(e) = fs::rename(&src, &dest) {
                let _ = fs::remove_file(&dest);
                return Err(e);
            }
            return Ok(tdir.join(name));
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name in the trash"))
    }

    pub fn import_move(
        &self,
        src: &Path,
        dir: &Path,
        names: &mut dyn Iterator<Item = String>,
        cap: u64,
        magic: &[u8],
        keep_src: bool,
    ) -> Result<Imported, ImportErr> {
        let d = self.dir(dir, false).map_err(ImportErr::Io)?;
        let mut sf = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(src)
            .map_err(|e| {
                if e.kind() == io::ErrorKind::PermissionDenied {
                    ImportErr::NotRegular
                } else {
                    ImportErr::OpenSrc(e)
                }
            })?;
        let st0 = sf.metadata().map_err(ImportErr::Io)?;
        if is_reparse(&st0) || !st0.is_file() {
            return Err(ImportErr::NotRegular);
        }
        if st0.len() > cap {
            return Err(ImportErr::TooBig(st0.len()));
        }
        if !magic.is_empty() && !starts_with_at0(&mut sf, magic).map_err(ImportErr::Io)? {
            return Err(ImportErr::BadMagic);
        }
        let id0 = file_id_handle(&sf).map_err(ImportErr::Io)?;
        let mtime0 = st0.modified().ok();
        let mut names = names.peekable();
        let first = names.peek().cloned().ok_or(ImportErr::NoFreeName)?;
        let tmp = d.path.join(tmp_name(std::ffi::OsStr::new(&first)).map_err(ImportErr::Io)?);
        let unstage = || {
            let _ = fs::remove_file(&tmp);
        };
        // always copy on Windows
        let how = "copy";
        let r = (|| -> Result<(), ImportErr> {
            let mut out = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&tmp)
                .map_err(ImportErr::Io)?;
            let mut buf = vec![0u8; 64 * 1024];
            let mut total: u64 = 0;
            loop {
                let n = sf.read(&mut buf).map_err(ImportErr::Io)?;
                if n == 0 {
                    break;
                }
                total += n as u64;
                if total > cap {
                    return Err(ImportErr::TooBig(total));
                }
                out.write_all(&buf[..n]).map_err(ImportErr::Io)?;
                fault("import:copying").map_err(ImportErr::Io)?;
            }
            out.sync_all().map_err(ImportErr::Io)?;
            let st1 = sf.metadata().map_err(ImportErr::Io)?;
            if st1.len() != st0.len() || st1.modified().ok() != mtime0 || total != st0.len() {
                return Err(ImportErr::Changed);
            }
            if !same_bytes(&mut sf, &mut out).map_err(ImportErr::Io)? {
                return Err(ImportErr::Io(io::Error::other("verify: the staged copy does not match the source")));
            }
            fault("import:staged").map_err(ImportErr::Io)
        })();
        if let Err(e) = r {
            unstage();
            return Err(e);
        }
        let mut committed = None;
        for cand in names {
            if cand.is_empty() || cand.contains('/') || cand.contains('\\') || cand.starts_with('.') {
                continue;
            }
            let dest = d.path.join(&cand);
            // fs::rename on Windows replaces an existing name. Claim with
            // CREATE_NEW first (same as move_file/trash) so a taken name is
            // skipped; rename then only overwrites our zero-byte placeholder.
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&dest)
            {
                Ok(f) => drop(f),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    unstage();
                    return Err(ImportErr::Io(e));
                }
            }
            match fs::rename(&tmp, &dest) {
                Ok(()) => {
                    committed = Some(cand);
                    break;
                }
                Err(e) => {
                    let _ = fs::remove_file(&dest);
                    unstage();
                    return Err(ImportErr::Io(e));
                }
            }
        }
        let Some(name) = committed else {
            unstage();
            return Err(ImportErr::NoFreeName);
        };
        let _ = open_dir_nofollow(&d.path).and_then(|f| f.sync_all());
        if keep_src {
            return Ok(Imported { name, src: SrcFate::Copied, how });
        }
        let fate = match OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(src)
            .and_then(|f| file_id_handle(&f))
        {
            Ok(id) if id == id0 => match fault("import:unlink-src").and_then(|_| fs::remove_file(src)) {
                Ok(()) => SrcFate::Removed,
                Err(e) => SrcFate::Kept(e.to_string()),
            },
            Ok(_) => SrcFate::Kept("the source was replaced during the move".into()),
            Err(e) => SrcFate::Kept(e.to_string()),
        };
        Ok(Imported { name, src: fate, how })
    }
}

pub const PLUGIN_READ_CAP: u64 = 64 << 20;
pub const PLUGIN_MODE_MASK: u32 = 0o777;

pub fn plugin_rel(s: &str) -> io::Result<PathBuf> {
    if s.contains('\0') {
        return Err(refused(Path::new(s), "NUL in path"));
    }
    let t = s.trim_start_matches('/');
    if t.contains('\\') || t.contains(':') || t.contains('%') {
        return Err(refused(Path::new(s), "Windows / encoded / absolute form"));
    }
    let p = Path::new(t);
    if p.is_absolute() {
        return Err(refused(p, "absolute path"));
    }
    let mut out = PathBuf::new();
    let mut n = 0usize;
    for c in p.components() {
        match c {
            Component::Normal(x) => {
                n += 1;
                if n > MAX_WALK_DEPTH {
                    return Err(refused(p, "path too deep"));
                }
                out.push(x);
            }
            Component::CurDir => {}
            Component::ParentDir => return Err(refused(p, ".. component")),
            _ => return Err(refused(p, "bad path component")),
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
pub struct PStat {
    pub kind: &'static str,
    pub size: u64,
    pub mtime_ms: i64,
    pub ctime_ms: i64,
}

impl Vault {
    pub fn p_stat(&self, rel: &Path) -> io::Result<PStat> {
        let p = self.abs(rel);
        let meta = if rel.as_os_str().is_empty() {
            open_dir_nofollow(&p)?.metadata()?
        } else {
            let (d, leaf) = self.parent(rel, false)?;
            let full = d.path.join(&leaf);
            let meta = fs::symlink_metadata(&full)?;
            if is_reparse(&meta) {
                return Err(refused(rel, "symlink in vault path"));
            }
            meta
        };
        let kind = if meta.is_dir() {
            "folder"
        } else if meta.is_file() {
            "file"
        } else {
            return Err(refused(rel, "not a regular file or directory"));
        };
        Ok(PStat {
            kind,
            size: if kind == "file" { meta.len() } else { 0 },
            mtime_ms: meta.modified().map(ms_since_epoch).unwrap_or(0),
            ctime_ms: meta.created().map(ms_since_epoch).unwrap_or(0),
        })
    }

    fn p_open(&self, rel: &Path, append: bool) -> io::Result<File> {
        let (d, leaf) = self.parent(rel, false)?;
        let full = d.path.join(&leaf);
        // Always lstat: Path::exists() follows links, so a dangling reparse
        // would skip the refusal and create the target outside the vault.
        match fs::symlink_metadata(&full) {
            Ok(m) if is_reparse(&m) => return Err(refused(rel, "symlink in vault path")),
            Ok(m) if !m.is_file() => return Err(refused(rel, "not a regular file")),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if !append {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
        let mut o = OpenOptions::new();
        o.read(true).write(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        if append {
            o.append(true).create(true);
        }
        let f = o.open(&full)?;
        let meta = f.metadata()?;
        if is_reparse(&meta) || !meta.is_file() {
            return Err(refused(rel, "not a regular file"));
        }
        Ok(f)
    }

    pub fn p_read(&self, rel: &Path, cap: u64) -> io::Result<Vec<u8>> {
        // Read-only open — never create or request write access.
        let mut f = self.open_read(rel)?;
        let n = f.metadata()?.len();
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

    pub fn p_write(&self, rel: &Path, bytes: &[u8], append: bool) -> io::Result<()> {
        let (_d, _leaf) = self.parent(rel, false)?;
        if !append {
            return self.write_atomic(rel, bytes, false);
        }
        let mut f = self.p_open(rel, true)?;
        f.write_all(bytes)
    }

    pub fn p_list(&self, rel: &Path) -> io::Result<Vec<(String, &'static str)>> {
        let d = self.dir(rel, false)?;
        let mut out = Vec::new();
        for e in fs::read_dir(&d.path)? {
            let e = e?;
            let name = e.file_name();
            let ns = name.to_string_lossy();
            if ns == "." || ns == ".." {
                continue;
            }
            let meta = match fs::symlink_metadata(e.path()) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if is_reparse(&meta) {
                continue;
            }
            let kind = if meta.is_file() {
                "file"
            } else if meta.is_dir() {
                "folder"
            } else {
                continue;
            };
            out.push((ns.into_owned(), kind));
        }
        out.sort();
        Ok(out)
    }

    pub fn p_mkdir(&self, rel: &Path, recursive: bool) -> io::Result<()> {
        if recursive {
            return self.mkdir_p(rel);
        }
        let (d, leaf) = self.parent(rel, false)?;
        let p = d.path.join(&leaf);
        if p.exists() {
            let st = fs::symlink_metadata(&p)?;
            if is_reparse(&st) {
                return Err(refused(rel, "symlink in vault path"));
            }
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "exists"));
        }
        fs::create_dir(&p)
    }

    pub fn p_remove(&self, rel: &Path, dir: bool) -> io::Result<()> {
        let st = self.p_stat(rel)?;
        let (d, leaf) = self.parent(rel, false)?;
        let p = d.path.join(&leaf);
        match (st.kind, dir) {
            ("file", _) => fs::remove_file(&p),
            ("folder", true) => fs::remove_dir(&p),
            _ => Err(io::Error::new(io::ErrorKind::IsADirectory, "EISDIR")),
        }
    }

    pub fn p_rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let _ = self.p_stat(from)?;
        let (fd_from, lf) = self.parent(from, false)?;
        let (fd_to, lt) = self.parent(to, false)?;
        if let Ok(st) = fs::symlink_metadata(fd_to.path.join(&lt)) {
            if is_reparse(&st) {
                return Err(refused(to, "symlink in vault path"));
            }
            let _ = self.p_stat(to)?;
        }
        fs::rename(fd_from.path.join(&lf), fd_to.path.join(&lt))
    }

    pub fn p_copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        let bytes = self.p_read(from, PLUGIN_READ_CAP)?;
        self.p_write(to, &bytes, false)
    }

    pub fn p_chmod(&self, rel: &Path, _mode: u32) -> io::Result<()> {
        // Windows: no Unix mode bits; refuse symlinks, accept file/dir as no-op.
        let _ = self.p_stat(rel)?;
        hook("p_chmod:opened");
        Ok(())
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
        }
    }
    Some(out)
}

pub fn plugin_err(op: &str, path: &str, e: &io::Error) -> String {
    let p = path.replace('\0', "\\0");
    if e.kind() == io::ErrorKind::PermissionDenied {
        return format!("EPERM: vault-escape refused, {op} '{p}'");
    }
    let code = match e.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::AlreadyExists => "EEXIST",
        io::ErrorKind::NotADirectory => "ENOTDIR",
        io::ErrorKind::IsADirectory => "EISDIR",
        io::ErrorKind::DirectoryNotEmpty => "ENOTEMPTY",
        io::ErrorKind::InvalidInput => "EINVAL",
        io::ErrorKind::PermissionDenied => "EACCES",
        _ => "EIO",
    };
    format!("{code}: {e}, {op} '{p}'")
}

pub fn plugin_op(root: &Path, op: &str, path: &str, to: Option<&str>, data: Option<&str>, recursive: bool) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let e = |x: io::Error| plugin_err(op, path, &x);
    let rel = plugin_rel(path).map_err(e)?;
    let need = |rel: &Path| {
        if rel.as_os_str().is_empty() {
            Err(plugin_err(op, path, &refused(rel, "the vault root itself")))
        } else {
            Ok(())
        }
    };
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
        "stat" => vfs
            .p_stat(&rel)
            .map(|s| json!({"type": s.kind, "size": s.size, "mtime": s.mtime_ms, "ctime": s.ctime_ms}))
            .map_err(e),
        "exists" => match vfs.p_stat(&rel) {
            Ok(_) => Ok(json!(true)),
            Err(x) if x.kind() == io::ErrorKind::NotFound => Ok(json!(false)),
            Err(x) => Err(e(x)),
        },
        "read" => {
            need(&rel)?;
            vfs.p_read(&rel, PLUGIN_READ_CAP).map(|b| json!(b64_encode(&b))).map_err(e)
        }
        "write" | "append" => {
            need(&rel)?;
            vfs.p_write(&rel, &bytes()?, op == "append").map(|_| json!(null)).map_err(e)
        }
        "list" => vfs
            .p_list(&rel)
            .map(|v| json!(v.into_iter().map(|(n, k)| json!([n, k])).collect::<Vec<_>>()))
            .map_err(e),
        "mkdir" => {
            need(&rel)?;
            vfs.p_mkdir(&rel, recursive).map(|_| json!(null)).map_err(e)
        }
        "unlink" | "rmdir" => {
            need(&rel)?;
            vfs.p_remove(&rel, op == "rmdir").map(|_| json!(null)).map_err(e)
        }
        "rename" => {
            need(&rel)?;
            let t = dest()?;
            vfs.p_rename(&rel, &t).map(|_| json!(null)).map_err(e)
        }
        "copy" => {
            need(&rel)?;
            let t = dest()?;
            vfs.p_copy(&rel, &t).map(|_| json!(null)).map_err(e)
        }
        _ => Err(format!("ENOSYS: fs op {op:?} is not available in opensidian")),
    }
}

pub fn plugin_chmod(root: &Path, trusted: bool, path: &str, mode: u32) -> Result<serde_json::Value, String> {
    let e = |x: io::Error| plugin_err("chmod", path, &x);
    if !trusted {
        return Err(format!("EPERM: restricted mode, plugin chmod is off, chmod '{}'", path.replace('\0', "\\0")));
    }
    let rel = plugin_rel(path).map_err(e)?;
    if rel.as_os_str().is_empty() {
        return Err(e(refused(&rel, "the vault root itself")));
    }
    Vault::open(root)
        .map_err(e)?
        .p_chmod(&rel, mode & PLUGIN_MODE_MASK)
        .map(|_| serde_json::Value::Null)
        .map_err(e)
}

pub fn write_atomic(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
    Vault::open(root)?.write_atomic(rel, bytes, false)
}

pub fn write_atomic_durable(root: &Path, rel: &Path, bytes: &[u8]) -> io::Result<()> {
    Vault::open(root)?.write_atomic(rel, bytes, true)
}

pub fn mkdir_p(root: &Path, rel: &Path) -> io::Result<()> {
    Vault::open(root)?.mkdir_p(rel)
}

pub fn walk_real_dirs(dir: &Path, base: &Path, out: &mut Vec<String>) {
    walk_real_dirs_at(dir, base, out, 0)
}

pub const MAX_WALK_DEPTH: usize = 256;

fn walk_real_dirs_at(dir: &Path, base: &Path, out: &mut Vec<String>, depth: usize) {
    if depth >= MAX_WALK_DEPTH {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let p = e.path();
        if let Ok(meta) = fs::symlink_metadata(&p) {
            if is_reparse(&meta) {
                continue;
            }
        }
        if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
        walk_real_dirs_at(&p, base, out, depth + 1);
    }
}
