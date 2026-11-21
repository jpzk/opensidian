// SPDX-License-Identifier: GPL-3.0-or-later
/* desk_proc (Windows backend) — the pieces of desk_proc.rs that cannot be the unix ones.
   Everything platform-neutral (Spec, the trust epoch, the live runs, shell_target, the
   consent plumbing) stays in desk_proc.rs and is shared, so v0.9.6's behaviour is ONE code
   path; this file only supplies what the unix side does with fds, signals and /proc:

   - Pinned / Ident: Windows has no O_PATH fd and no fexecve either. pin() holds the program
     (and pin_dir() the cwd) open, records its identity (volume serial + file index, size,
     last-write time: GetFileInformationByHandle, from the OPEN object), and start() rechecks
     that identity under the lifecycle lock and launches the open object's CURRENT path
     (GetFinalPathNameByHandleW): a rename-away during the dialog still runs the approved
     object, a rewrite in place or a different file at the old name is EPERM. A reparse point
     (symlink, junction, app-execution alias) is opened as itself and refused, the twin of
     O_NOFOLLOW. WEAKER than Linux's exec-by-fd: a swap in the instant between the recheck and
     CreateProcess is not caught (same stance as the macOS port, which has no fexecve either).
   - Run kill: each child is assigned to a Job Object, so kill() / kill_all reach the whole
     tree (the process group's job on unix). A Windows kill has no signal: every signal ends
     the job, and the signal asked for is what the page reads back.
   - child_path: the same rules as the unix PATH cleaning (R4/S1-S3/V1/W1), on ';' lists and
     drive paths: only absolute entries, each in its canonical form, none inside the vault or
     the child's cwd (Windows would look a bare name up in the cwd FIRST, and the cwd is
     inside the vault, which plugins write). */
use super::{ChildPath, Launch, Spec};
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

type Handle = *mut core::ffi::c_void;

const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attr: *mut core::ffi::c_void, name: *const u16) -> Handle;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn TerminateJobObject(job: Handle, code: u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetFileInformationByHandle(h: Handle, info: *mut ByHandleFileInfo) -> i32;
    fn GetFinalPathNameByHandleW(h: Handle, buf: *mut u16, len: u32, flags: u32) -> u32;
}

/// BY_HANDLE_FILE_INFORMATION (FILETIMEs as two u32: the struct is 4-aligned, 52 bytes)
#[repr(C)]
struct ByHandleFileInfo {
    attributes: u32,
    created: [u32; 2],
    accessed: [u32; 2],
    written: [u32; 2],
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

/* ---- the Job Object that owns one child (and what it starts) */

pub(super) struct JobHandle(Handle);

// SAFETY: the HANDLE is only used by kill() and Drop; Run sits behind an Arc and both take &self.
unsafe impl Send for JobHandle {}
unsafe impl Sync for JobHandle {}

impl JobHandle {
    /// a job holding `child` (null job = no tree kill: kill() falls back to the child alone)
    pub(super) fn assign(child: &Child) -> JobHandle {
        // SAFETY: plain Win32 calls; the child's own process handle (std keeps it open) carries
        // PROCESS_SET_QUOTA | PROCESS_TERMINATE from CreateProcess
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if !job.is_null() && AssignProcessToJobObject(job, child.as_raw_handle() as Handle) == 0 {
                CloseHandle(job);
                return JobHandle(std::ptr::null_mut());
            }
            JobHandle(job)
        }
    }

    /// end the whole tree; false when there is no job or the call failed
    pub(super) fn terminate(&self) -> bool {
        // SAFETY: self.0 is a live job handle until Drop
        !self.0.is_null() && unsafe { TerminateJobObject(self.0, 1) != 0 }
    }

    pub(super) fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: closing the handle we created; a forgotten run takes its tree with it
            unsafe {
                TerminateJobObject(self.0, 1);
                CloseHandle(self.0);
            }
            self.0 = std::ptr::null_mut();
        }
    }
}

/// kill() of a child with no usable job: end just that process (by pid; it is a child of ours that
/// kill() has just seen not reaped)
pub(super) fn terminate_pid(pid: u32) -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        fn TerminateProcess(h: Handle, code: u32) -> i32;
    }
    const PROCESS_TERMINATE: u32 = 0x0001;
    // SAFETY: plain Win32 calls; the handle is closed before returning
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            return false;
        }
        let ok = TerminateProcess(h, 1) != 0;
        CloseHandle(h);
        ok
    }
}

pub fn sig_name(n: i32) -> &'static str {
    if n == 9 {
        "SIGKILL"
    } else {
        "SIGTERM"
    }
}

pub fn sig_num(name: &str) -> Option<i32> {
    Some(match name.trim_start_matches("SIG").to_ascii_uppercase().as_str() {
        "KILL" => 9,
        "" | "TERM" | "INT" | "HUP" | "QUIT" | "ABRT" => 15,
        _ => return None,
    })
}

/* ---- identity of an open file / directory */

/// what one GetFileInformationByHandle of the OPEN object says it is. A directory is compared
/// on (volume, index) only: its size / write time move whenever an entry in it changes (R1)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    volume: u32,
    index: u64,
    size: u64,
    written: u64,
    is_dir: bool,
}

impl Ident {
    pub(super) fn same_as(&self, was: &Ident) -> bool {
        if self.is_dir != was.is_dir {
            return false;
        }
        if was.is_dir {
            return (self.volume, self.index) == (was.volume, was.index);
        }
        self == was
    }

    /// (volume, index, size, write time) as the unix (dev, ino, size, mtime) of #74's cache key
    fn key(&self) -> (u64, u64, u64, (i64, i64), (i64, i64)) {
        (self.volume as u64, self.index, self.size, (self.written as i64, 0), (0, 0))
    }
}

fn handle_ident(f: &File) -> Option<(Ident, u32)> {
    let mut info = std::mem::MaybeUninit::<ByHandleFileInfo>::zeroed();
    // SAFETY: the handle is open; the call fills the struct, read only on success
    if unsafe { GetFileInformationByHandle(f.as_raw_handle() as Handle, info.as_mut_ptr()) } == 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    let ident = Ident {
        volume: info.volume,
        index: (info.index_high as u64) << 32 | info.index_low as u64,
        size: (info.size_high as u64) << 32 | info.size_low as u64,
        written: (info.written[1] as u64) << 32 | info.written[0] as u64,
        is_dir: info.attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
    };
    Some((ident, info.attributes))
}

/// open `p` as ITSELF: a reparse point is not followed (the O_NOFOLLOW twin)
fn open_nofollow(p: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS).open(p)
}

/// the name the kernel gives the open object right now (normalized, DOS volume), `\\?\` dropped
fn final_path(f: &File) -> std::io::Result<PathBuf> {
    let h = f.as_raw_handle() as Handle;
    let mut buf = vec![0u16; 512];
    loop {
        // SAFETY: buf has buf.len() u16 of room; flags 0 = FILE_NAME_NORMALIZED | VOLUME_NAME_DOS
        let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
        if n == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n >= buf.len() {
            buf.resize(n + 1, 0);
            continue;
        }
        buf.truncate(n);
        return Ok(plain(&PathBuf::from(OsString::from(String::from_utf16_lossy(&buf)))));
    }
}

/// `\\?\C:\x` -> `C:\x`, `\\?\UNC\h\s` -> `\\h\s` (what canonicalize returns is verbatim; child
/// programs, PATH lists and the comparisons below want the plain form)
pub(super) fn plain(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(r) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{r}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(r) if r.as_bytes().get(1) == Some(&b':') => PathBuf::from(r),
        _ => p.to_path_buf(),
    }
}

/// Windows file names compare without case
fn same_name(a: &Path, b: &Path) -> bool {
    plain(a).to_string_lossy().to_lowercase() == plain(b).to_string_lossy().to_lowercase()
}

/// Z2: the identity of `p` from the object itself (opened as itself, never through a link),
/// refused when it is a reparse point or is not at exactly `p`
pub(super) fn ident_of(p: &Path) -> Result<Ident, String> {
    let f = open_nofollow(p).map_err(|e| format!("ENOENT: {}: {e}", p.display()))?;
    let (id, attrs) = handle_ident(&f).ok_or_else(|| format!("ENOENT: {}: {}", p.display(), std::io::Error::last_os_error()))?;
    if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!("ELOOP: {} is a symlink (swapped in after it was resolved); not opened", p.display()));
    }
    let at = final_path(&f).map_err(|e| format!("ENOENT: {}: {e}", p.display()))?;
    if !same_name(&at, p) {
        return Err(format!("EAGAIN: {}: the object opened is {} (a directory on the way was swapped); not opened", p.display(), at.display()));
    }
    Ok(id)
}

/// Y1/AA2: the program, or the cwd, held open from the question to the exec
pub struct Pinned {
    file: File,
    path: PathBuf,
    ident: Ident,
}

impl Pinned {
    /// Y1: the open object is still what it was when pinned (compared on the held handle)
    pub(super) fn unchanged(&self) -> bool {
        handle_ident(&self.file).is_some_and(|(now, _)| now.same_as(&self.ident))
    }

    /// #74 (spawn_allow): the key of the program-hash cache
    pub fn stat_key(&self) -> (u64, u64, u64, (i64, i64), (i64, i64)) {
        self.ident.key()
    }

    /// #74: the pinned file read through a duplicate of the held handle — the very object that
    /// was pinned, never the pathname again
    pub fn open_read(&self) -> std::io::Result<File> {
        use std::io::Seek;
        let mut f = self.file.try_clone()?;
        f.rewind()?;
        Ok(f)
    }

    /// where the held object is NOW (it survives a rename-away of the pinned name)
    pub(super) fn launch_path(&self) -> Result<PathBuf, String> {
        final_path(&self.file).map_err(|e| format!("EPERM: pin {}: {e}", self.path.display()))
    }
}

/// Y1: hold `file` (an absolute canonical path) open from now until the exec. Refused: not
/// openable, a reparse point, not a regular file, or the open object is not at exactly `file`
pub fn pin(file: &str) -> Result<Pinned, String> {
    if file.contains('\0') {
        return Err(format!("EINVAL: spawn {file}: NUL in the path"));
    }
    let p = PathBuf::from(file);
    let f = open_nofollow(&p).map_err(|e| format!("ENOENT: spawn {file}: {e}"))?;
    let (ident, attrs) = handle_ident(&f).ok_or_else(|| format!("ENOENT: spawn {file}: {}", std::io::Error::last_os_error()))?;
    if ident.is_dir || attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!("EACCES: spawn {file}: not a regular executable file"));
    }
    let at = final_path(&f).map_err(|e| format!("ENOENT: spawn {file}: {e}"))?;
    if !same_name(&at, &p) {
        return Err(format!("EAGAIN: spawn {file}: the file opened is {} (moved while it was opened); nothing asked", at.display()));
    }
    Ok(Pinned { file: f, path: p, ident })
}

/// AA2: hold the canonical cwd `dir` open from now until the exec. Refused: not openable, a
/// reparse point, not a directory, or the open object is not at exactly `dir`
pub fn pin_dir(dir: &Path) -> Result<Pinned, String> {
    let f = open_nofollow(dir).map_err(|e| format!("ENOENT: spawn: cwd {}: {e}", dir.display()))?;
    let (ident, attrs) = handle_ident(&f).ok_or_else(|| format!("ENOENT: spawn: cwd {}: {}", dir.display(), std::io::Error::last_os_error()))?;
    if !ident.is_dir || attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!("ENOTDIR: spawn: cwd {} is not a directory", dir.display()));
    }
    let at = final_path(&f).map_err(|e| format!("ENOENT: spawn: cwd {}: {e}", dir.display()))?;
    if !same_name(&at, dir) {
        return Err(format!("EAGAIN: spawn: cwd {}: the directory opened is {} (moved while it was opened); nothing asked", dir.display(), at.display()));
    }
    Ok(Pinned { file: f, path: dir.to_path_buf(), ident })
}

/* ---- the child */

/// Y1/AA2: CreateProcess runs the held program at the path the open handle has now, in the held
/// cwd's current path, with the cleaned PATH. No console window flashes for a console program
/// (the app is a GUI process). argv0 has no meaning here: the program sees its own path.
pub(super) fn build(sp: &Spec, path: &ChildPath, prog: &Pinned, dir: &Pinned) -> Command {
    use std::os::windows::process::CommandExt;
    let file = prog.launch_path().unwrap_or_else(|_| prog.path.clone());
    let cwd = dir.launch_path().unwrap_or_else(|_| dir.path.clone());
    let mut c = Command::new(file);
    c.args(&sp.args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).creation_flags(CREATE_NO_WINDOW);
    for (k, v) in &sp.env {
        c.env(k, v);
    }
    c.env("PATH", path.as_os_str());
    c
}

impl Launch<'_> {
    /// the held opener's current path (there is no fd to exec)
    pub fn program(&self) -> String {
        self.prog.launch_path().unwrap_or_else(|_| self.prog.path.clone()).to_string_lossy().into_owned()
    }

    /// the target, the cwd and the cleaned PATH onto the caller's opener Command
    pub fn apply<'c>(&self, c: &'c mut Command) -> &'c mut Command {
        let _ = self.argv0;
        c.arg(self.arg).current_dir(self.cwd).env("PATH", &self.path)
    }
}

/// what the shell opener is on Windows: main.rs hands the allowed target to ShellExecuteW (one
/// LPCWSTR, never cmd.exe) and ignores Launch::program(); this is a real binary for the consent
/// plumbing to resolve, pin and name in the dialog
pub(super) fn opener_program() -> String {
    let root = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")).unwrap_or_else(|| OsString::from(r"C:\Windows"));
    PathBuf::from(root).join("explorer.exe").to_string_lossy().into_owned()
}

/* ---- the child's PATH (R4 + S1-S3 + V1 + W1, on Windows) */

/// a PATH that no lookup can ever resolve through: an absolute entry that is a device, never a
/// directory (W1; /dev/null on unix)
pub const NO_EXEC_PATH: &str = r"\\.\NUL";

/// a folded, case-less form for the confinement tests
fn fold(p: &Path) -> String {
    plain(p).to_string_lossy().to_lowercase().replace('/', "\\").trim_end_matches('\\').to_string()
}

/// is `f` the folded root `r`, or inside it?
fn inside(f: &str, r: &str) -> bool {
    f == r || f.strip_prefix(r).is_some_and(|rest| rest.starts_with('\\'))
}

/// S3 + V1: the directory the child may look programs up in for `dir` (an absolute PATH entry):
/// its canonical plain form, or None = drop the entry. A form holding ';' would split PATH.
fn path_dir_ok(dir: &Path, confine: &[&Path]) -> Option<PathBuf> {
    let canon = plain(&dir.canonicalize().ok().filter(|c| c.is_absolute() && c.is_dir())?);
    if canon.as_os_str().to_string_lossy().contains(';') {
        return None;
    }
    let forms = [fold(dir), fold(&canon)];
    let mut roots: Vec<String> = Vec::new();
    for r in confine.iter().filter(|r| r.is_absolute()) {
        roots.push(fold(r));
        if let Ok(c) = r.canonicalize() {
            roots.push(fold(&c));
        }
    }
    // a drive root confines nothing (the opener's cwd: plugins cannot write it)
    roots.retain(|r| r.len() > 2);
    let bad = forms.iter().any(|f| roots.iter().any(|r| inside(f, r)));
    (!bad).then_some(canon)
}

/// W1: the system directories, each confinement-checked; NO_EXEC_PATH when none passes
fn safe_path(confine: &[&Path]) -> OsString {
    let root = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir")).unwrap_or_else(|| OsString::from(r"C:\Windows"));
    let root = PathBuf::from(root);
    let mut out: Vec<PathBuf> = Vec::new();
    for d in [root.join("System32"), root] {
        if let Some(c) = path_dir_ok(&d, confine) {
            if !out.contains(&c) {
                out.push(c);
            }
        }
    }
    std::env::join_paths(out).ok().filter(|p| !p.is_empty()).unwrap_or_else(|| OsString::from(NO_EXEC_PATH))
}

/// R4 + S1-S3 + V1: the PATH a child runs with, from the PATH it would have had — only absolute
/// entries, each in its canonical form, none inside a `confine` root (the vault, the child's cwd)
pub fn child_path(path: &OsStr, confine: &[&Path]) -> OsString {
    let kept: Vec<PathBuf> = std::env::split_paths(path).filter(|d| d.is_absolute()).filter_map(|d| path_dir_ok(&d, confine)).collect();
    match std::env::join_paths(kept) {
        Ok(p) if !p.is_empty() => p,
        _ => safe_path(confine),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deskwin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.canonicalize().unwrap()
    }

    #[test]
    fn plain_drops_the_verbatim_prefix_only_where_it_is_safe() {
        assert_eq!(plain(Path::new(r"\\?\C:\a\b")), PathBuf::from(r"C:\a\b"));
        assert_eq!(plain(Path::new(r"\\?\UNC\h\s\x")), PathBuf::from(r"\\h\s\x"));
        assert_eq!(plain(Path::new(r"\\?\Volume{1}\x")), PathBuf::from(r"\\?\Volume{1}\x"));
        assert_eq!(plain(Path::new(r"C:\a")), PathBuf::from(r"C:\a"));
    }

    #[test]
    fn child_path_keeps_absolute_entries_outside_the_vault_only() {
        let v = tmp("cp");
        let inside_dir = v.join("bin");
        let out_dir = tmp("cp-out");
        std::fs::create_dir_all(&inside_dir).unwrap();
        let path = std::env::join_paths([inside_dir.clone(), PathBuf::from("tools"), PathBuf::from("."), out_dir.clone()]).unwrap();
        let got = child_path(&path, &[v.as_path()]);
        let entries: Vec<PathBuf> = std::env::split_paths(&got).collect();
        assert_eq!(entries, vec![plain(&out_dir)], "{got:?}");
        // nothing absolute left: the system directories, never an empty PATH
        let rel = child_path(OsStr::new("tools;."), &[v.as_path()]);
        assert!(!rel.is_empty());
        assert!(std::env::split_paths(&rel).all(|p| p.is_absolute()), "{rel:?}");
    }

    #[test]
    fn a_pinned_program_that_is_rewritten_or_swapped_is_no_longer_the_one_pinned() {
        let d = tmp("pin");
        let exe = d.join("tool.exe");
        std::fs::write(&exe, b"one").unwrap();
        let canon = exe.canonicalize().unwrap();
        let p = pin(canon.to_str().unwrap()).expect("pin");
        assert!(p.unchanged());
        // renamed away: the held object is the same, and launch_path follows it
        let moved = d.join("moved.exe");
        std::fs::rename(&exe, &moved).unwrap();
        assert!(p.unchanged());
        assert!(same_name(&p.launch_path().unwrap(), &moved));
        // rewritten in place: size / write time move
        std::fs::write(&moved, b"two-two").unwrap();
        assert!(!p.unchanged());
        // a directory is refused as a program, a file as a cwd
        assert!(pin(d.to_str().unwrap()).is_err());
        assert!(pin_dir(&moved).is_err());
        assert!(pin_dir(&d).is_ok());
    }
}
