// SPDX-License-Identifier: GPL-3.0-or-later
/* desk_proc (Windows / macOS) — same public API as linux.rs.

   Linux pins programs with O_PATH fds and execs via /proc/self/fd/N. Here the
   Spec still carries prog/dir placeholders so consent::resolve and main.rs
   compile unchanged; start() runs Command::new on the resolved path. Shell
   opens hand the target to the caller's Launch (main uses ShellExecuteW on
   Windows / `open` on macOS when it builds the Command). */
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

pub const VAULT_ALIAS: &str = "/vault";
pub const SAFE_PATH: &str = "";
pub const NO_EXEC_PATH: &str = "";
#[cfg(windows)]
pub const OPENER_CWD: &str = "C:\\";
#[cfg(not(windows))]
pub const OPENER_CWD: &str = "/";

pub fn map_cwd(root: &Path, cwd: Option<&str>) -> PathBuf {
    let c = cwd.unwrap_or("").trim();
    if c.is_empty() || c == "." {
        return root.to_path_buf();
    }
    if c == VAULT_ALIAS {
        return root.to_path_buf();
    }
    if let Some(rest) = c.strip_prefix(VAULT_ALIAS).and_then(|r| r.strip_prefix('/')) {
        return root.join(rest);
    }
    let p = Path::new(c);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    root.join(c)
}

pub fn map_arg(root: &Path, a: &str) -> String {
    if a == VAULT_ALIAS || a.starts_with("/vault/") {
        return map_cwd(root, Some(a)).to_string_lossy().into_owned();
    }
    a.to_string()
}

pub fn vault_open_target(root: &Path, target: &str) -> Result<PathBuf, String> {
    let t = target.trim();
    if t.contains('\0') {
        return Err(format!("EPERM: vault-escape refused: {target}"));
    }
    let rel = if t == VAULT_ALIAS {
        ""
    } else if let Some(r) = t.strip_prefix(VAULT_ALIAS).and_then(|r| r.strip_prefix('/')) {
        r
    } else if Path::new(t).is_absolute() || t.starts_with('~') {
        return Err(format!("EPERM: vault-escape refused: {target}"));
    } else {
        t
    };
    if rel.split(['/', '\\']).any(|c| c == "..") {
        return Err(format!("EPERM: vault-escape refused: {target}"));
    }
    let real_root = root.canonicalize().map_err(|e| format!("ENOENT: vault root: {e}"))?;
    let p = real_root.join(rel);
    let real = p.canonicalize().map_err(|_| format!("ENOENT: no such file or directory, '{target}'"))?;
    if !real.starts_with(&real_root) {
        return Err(format!("EPERM: vault-escape refused (symlink out of the vault): {target}"));
    }
    Ok(real)
}

pub fn latin1_to_bytes(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

pub fn bytes_to_latin1(b: &[u8]) -> String {
    b.iter().map(|&x| x as char).collect()
}

#[derive(Default)]
struct State {
    out: Vec<u8>,
    err: Vec<u8>,
    exit: Option<i32>,
    signal: Option<i32>,
    reaped: bool,
    pumps: u8,
}

pub struct Run {
    pid: i32,
    stdin: Mutex<Option<ChildStdin>>,
    st: Mutex<State>,
    cv: Condvar,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Chunk {
    pub out: String,
    pub err: String,
    pub exit: Option<i32>,
    pub signal: Option<&'static str>,
    pub done: bool,
}

pub fn sig_name(_n: i32) -> &'static str {
    "SIGTERM"
}

pub fn sig_num(name: &str) -> Option<i32> {
    Some(match name.trim_start_matches("SIG").to_ascii_uppercase().as_str() {
        "KILL" => 9,
        "" | "TERM" | "INT" | "HUP" | "QUIT" | "ABRT" => 15,
        _ => return None,
    })
}

impl Run {
    pub fn pid(&self) -> i32 {
        self.pid
    }

    pub fn kill(&self, sig: i32) -> bool {
        if self.st.lock().unwrap_or_else(|e| e.into_inner()).reaped {
            return false;
        }
        let _ = sig;
        #[cfg(windows)]
        {
            #[link(name = "kernel32")]
            extern "system" {
                fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
                fn CloseHandle(h: *mut core::ffi::c_void) -> i32;
                fn TerminateProcess(h: *mut core::ffi::c_void, code: u32) -> i32;
            }
            const PROCESS_TERMINATE: u32 = 0x0001;
            unsafe {
                let h = OpenProcess(PROCESS_TERMINATE, 0, self.pid as u32);
                if h.is_null() {
                    return false;
                }
                let ok = TerminateProcess(h, 1) != 0;
                CloseHandle(h);
                ok
            }
        }
        #[cfg(not(windows))]
        {
            // SAFETY: signal the process we started.
            unsafe { libc::kill(self.pid, libc::SIGTERM) == 0 }
        }
    }

    pub fn write(&self, data: &[u8]) -> Result<(), String> {
        let mut g = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        let w = g.as_mut().ok_or("EPIPE: stdin is closed")?;
        w.write_all(data).and_then(|_| w.flush()).map_err(|e| format!("EPIPE: {e}"))
    }

    pub fn end(&self) {
        *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub fn read(&self, wait: Duration) -> Chunk {
        let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
        let idle = |s: &State| s.out.is_empty() && s.err.is_empty() && !(s.reaped && s.pumps == 0);
        if idle(&s) {
            s = self.cv.wait_timeout_while(s, wait, |s| idle(s)).unwrap_or_else(|e| e.into_inner()).0;
        }
        let out = bytes_to_latin1(&std::mem::take(&mut s.out));
        let err = bytes_to_latin1(&std::mem::take(&mut s.err));
        let done = s.reaped && s.pumps == 0;
        Chunk {
            out,
            err,
            exit: if done { s.exit } else { None },
            signal: if done { s.signal.map(sig_name) } else { None },
            done,
        }
    }
}

type Job = Box<dyn FnOnce() + Send>;

fn forker() -> &'static Mutex<std::sync::mpsc::Sender<Job>> {
    static S: OnceLock<Mutex<std::sync::mpsc::Sender<Job>>> = OnceLock::new();
    S.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("desk-proc-fork".into())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("desk_proc fork thread");
        Mutex::new(tx)
    })
}

pub struct Spec {
    pub file: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub root: PathBuf,
    pub env: Vec<(String, String)>,
    pub argv0: Option<String>,
    pub path: Option<ChildPath>,
    pub prog: Option<Pinned>,
    pub dir: Option<Pinned>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildPath(OsString);

impl ChildPath {
    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

pub fn prepare(mut sp: Spec, inherited: Option<OsString>) -> Spec {
    let p = inherited.unwrap_or_default();
    sp.path = Some(ChildPath(p));
    sp
}

/// Held open file/dir so a swap after consent cannot silently retarget the path.
pub struct Pinned {
    _file: File,
    path: PathBuf,
}

impl Pinned {
    fn raw(&self) -> i32 {
        0
    }
}

pub fn pin(file: &str) -> Result<Pinned, String> {
    let p = PathBuf::from(file);
    let f = File::open(&p).map_err(|e| format!("ENOENT: pin {file}: {e}"))?;
    let meta = f.metadata().map_err(|e| format!("ENOENT: pin {file}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("ENOENT: pin {file}: not a regular file"));
    }
    Ok(Pinned { _file: f, path: p })
}

pub fn pin_dir(dir: &Path) -> Result<Pinned, String> {
    let f = File::open(dir).map_err(|e| format!("ENOENT: pin cwd {}: {e}", dir.display()))?;
    let meta = f.metadata().map_err(|e| format!("ENOENT: pin cwd {}: {e}", dir.display()))?;
    if !meta.is_dir() {
        return Err(format!("ENOENT: pin cwd {}: not a directory", dir.display()));
    }
    Ok(Pinned {
        _file: f,
        path: dir.to_path_buf(),
    })
}

pub fn opener_path(ours: Option<OsString>, _root: &Path) -> ChildPath {
    ChildPath(ours.unwrap_or_default())
}

fn build(sp: &Spec) -> Command {
    let mut c = Command::new(&sp.file);
    c.args(&sp.args)
        .current_dir(&sp.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &sp.env {
        c.env(k, v);
    }
    if let Some(p) = &sp.path {
        c.env("PATH", p.as_os_str());
    }
    c
}

fn fork(sp: Spec) -> std::io::Result<Child> {
    let (tx, rx) = std::sync::mpsc::channel();
    forker()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(Box::new(move || {
            let _ = tx.send(build(&sp).spawn());
        }))
        .map_err(|_| std::io::Error::other("fork thread gone"))?;
    rx.recv().map_err(|_| std::io::Error::other("fork thread gone"))?
}

fn pump(r: Arc<Run>, mut src: impl Read, is_err: bool) {
    let mut b = [0u8; 16 * 1024];
    loop {
        match src.read(&mut b) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let mut s = r.st.lock().unwrap_or_else(|e| e.into_inner());
                if is_err {
                    s.err.extend_from_slice(&b[..n])
                } else {
                    s.out.extend_from_slice(&b[..n])
                }
            }
        }
        r.cv.notify_all();
    }
    r.st.lock().unwrap_or_else(|e| e.into_inner()).pumps -= 1;
    r.cv.notify_all();
}

fn spawn_err(file: &str, e: &std::io::Error) -> String {
    let code = match e.kind() {
        std::io::ErrorKind::NotFound => "ENOENT",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        _ => "EIO",
    };
    format!("{code}: spawn {file} {e}")
}

pub fn start(sp: Spec, _life: &Life) -> Result<Arc<Run>, String> {
    if sp.file.is_empty() {
        return Err("EINVAL: spawn: empty file".into());
    }
    if sp.path.is_none() {
        return Err("EINVAL: spawn: PATH not prepared".into());
    }
    if sp.prog.is_none() || sp.dir.is_none() {
        return Err("EINVAL: spawn: program/cwd not pinned".into());
    }
    if !sp.cwd.is_dir() {
        return Err(format!("ENOENT: spawn {} cwd {} is not a directory", sp.file, sp.cwd.display()));
    }
    let file = sp.file.clone();
    let mut child = fork(sp).map_err(|e| spawn_err(&file, &e))?;
    let run = Arc::new(Run {
        pid: child.id() as i32,
        stdin: Mutex::new(child.stdin.take()),
        st: Mutex::new(State {
            pumps: 2,
            ..State::default()
        }),
        cv: Condvar::new(),
    });
    let (o, e) = (child.stdout.take(), child.stderr.take());
    for (src, is_err) in [
        (o.map(|x| Box::new(x) as Box<dyn Read + Send>), false),
        (e.map(|x| Box::new(x) as Box<dyn Read + Send>), true),
    ] {
        let r = run.clone();
        match src {
            Some(s) => {
                std::thread::spawn(move || pump(r, s, is_err));
            }
            None => r.st.lock().unwrap_or_else(|e| e.into_inner()).pumps -= 1,
        }
    }
    let r = run.clone();
    std::thread::spawn(move || {
        let st = child.wait();
        let mut s = r.st.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(st) = st {
            s.exit = st.code();
            s.signal = None;
        }
        s.reaped = true;
        drop(s);
        r.cv.notify_all();
    });
    Ok(run)
}

fn life_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

pub struct Life(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

pub fn lifecycle() -> Life {
    Life(life_lock().lock().unwrap_or_else(|e| e.into_inner()))
}

pub fn held_here() -> bool {
    life_lock().try_lock().is_err()
}

pub fn try_lifecycle() -> Option<Life> {
    life_lock().try_lock().ok().map(Life)
}

fn runs() -> &'static Mutex<HashMap<u64, Arc<Run>>> {
    static R: OnceLock<Mutex<HashMap<u64, Arc<Run>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register(r: Arc<Run>, _life: &Life) -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    runs().lock().unwrap_or_else(|e| e.into_inner()).insert(id, r);
    id
}

pub fn get(id: u64) -> Result<Arc<Run>, String> {
    runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
        .ok_or_else(|| "ESRCH: no such child".into())
}

pub fn forget(id: u64) {
    runs().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
}

pub fn kill_all(_life: &Life) {
    let all: Vec<Arc<Run>> = runs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .drain()
        .map(|(_, r)| r)
        .collect();
    for r in all {
        r.kill(sig_num("KILL").unwrap_or(9));
        r.end();
    }
}

static TRUST_EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn trust_epoch(_life: &Life) -> u64 {
    TRUST_EPOCH.load(Ordering::SeqCst)
}

pub fn trust_write(asked: Option<u64>, _life: &Life) -> Result<(), String> {
    if let Some(e) = asked {
        if TRUST_EPOCH.load(Ordering::SeqCst) != e {
            return Err("EAGAIN: community plugins were switched while the trust question was open; that answer was discarded, nothing written — ask again".into());
        }
    }
    TRUST_EPOCH.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustSeen(u64);

pub fn trust_epoch_now() -> TrustSeen {
    TrustSeen(TRUST_EPOCH.load(Ordering::SeqCst))
}

impl TrustSeen {
    pub fn unmoved(self, _life: &Life) -> Result<(), String> {
        if TRUST_EPOCH.load(Ordering::SeqCst) != self.0 {
            return Err("EAGAIN: community plugins were switched while this request was being prepared; nothing was started — try again".into());
        }
        Ok(())
    }
}

#[cfg(test)]
pub fn epoch_tests() -> std::sync::MutexGuard<'static, ()> {
    static EPOCH_TESTS: Mutex<()> = Mutex::new(());
    EPOCH_TESTS.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn trust_plan(on: bool, seen: u64, trusted_now: impl FnOnce() -> bool) -> (bool, Option<u64>) {
    if !on {
        return (false, None);
    }
    (!trusted_now(), Some(seen))
}

pub enum ShellOpen {
    Url(String),
    Ask(Local),
}

#[derive(Clone, Debug)]
pub struct Ident {
    pub path: PathBuf,
}

pub struct Local {
    pub arg: String,
    ident: Option<Ident>,
}

impl Local {
    fn url(arg: String) -> Self {
        Self { arg, ident: None }
    }

    fn file(p: PathBuf, shown: &str) -> Result<Self, String> {
        let real = p
            .canonicalize()
            .map_err(|_| format!("ENOENT: no such file or directory, '{shown}'"))?;
        Ok(Self {
            arg: real.to_string_lossy().into_owned(),
            ident: Some(Ident { path: real }),
        })
    }

    fn recheck(&self) -> Result<(), String> {
        let Some(id) = &self.ident else {
            return Ok(());
        };
        let now = id
            .path
            .canonicalize()
            .map_err(|_| format!("EPERM: open: target {} changed after it was allowed; not opened", self.arg))?;
        if now != id.path {
            return Err(format!("EPERM: open: target {} changed after it was allowed; not opened", self.arg));
        }
        Ok(())
    }
}

fn file_url_path(url: &str) -> Result<PathBuf, String> {
    let rest = url
        .strip_prefix("file:")
        .ok_or_else(|| format!("EINVAL: not a file URL: {url}"))?;
    let path = percent_decode(rest.trim_start_matches('/').trim_start_matches('/'));
    #[cfg(windows)]
    {
        // file:///C:/x -> C:/x
        Ok(PathBuf::from(path))
    }
    #[cfg(not(windows))]
    {
        Ok(PathBuf::from(format!("/{path}")))
    }
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (from_hex(b[i + 1]), from_hex(b[i + 2])) {
                out.push((h << 4 | l) as char);
                i += 3;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

fn from_hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub fn shell_target(root: &Path, op: &str, target: &str) -> Result<ShellOpen, String> {
    match op {
        "external" => {
            let scheme = target.split(':').next().unwrap_or("");
            let ok = target.contains(':')
                && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
            if !ok {
                return Err(format!("EINVAL: not a URL: {target}"));
            }
            match scheme.to_ascii_lowercase().as_str() {
                "http" | "https" | "mailto" => {
                    crate::consent::check_env(&opener_spec(root, target))?;
                    Ok(ShellOpen::Url(target.to_string()))
                }
                "file" => Local::file(file_url_path(target)?, target).map(ShellOpen::Ask),
                _ => Ok(ShellOpen::Ask(Local {
                    arg: target.to_string(),
                    ident: None,
                })),
            }
        }
        "path" => Local::file(map_cwd(root, Some(target)), target).map(ShellOpen::Ask),
        "vault" => Local::file(vault_open_target(root, target)?, target).map(ShellOpen::Ask),
        "reveal" => {
            let p = map_cwd(root, Some(target));
            Local::file(p.parent().map(|d| d.to_path_buf()).unwrap_or(p), target).map(ShellOpen::Ask)
        }
        _ => Err(format!("EINVAL: unknown shell op {op}")),
    }
}

pub struct Launch<'a> {
    pub arg: &'a str,
    pub cwd: &'static Path,
    pub path: OsString,
    prog: &'a Pinned,
    argv0: &'a str,
}

impl Launch<'_> {
    /// On non-Linux the opener is the held path itself (no /proc fd exec).
    pub fn program(&self) -> String {
        self.prog.path.to_string_lossy().into_owned()
    }

    pub fn apply<'c>(&self, c: &'c mut Command) -> &'c mut Command {
        c.arg(self.arg).current_dir(self.cwd).env("PATH", &self.path);
        let _ = (self.prog, self.argv0);
        c
    }
}

pub fn opener_err(file: &str, e: &std::io::Error) -> String {
    format!("ENOENT: opener {file}: {e}")
}

pub fn opener_spec(root: &Path, target: &str) -> Spec {
    // Pin a real helper binary so consent::resolve succeeds; the Windows desk_shell
    // callback opens `target` via ShellExecuteW and ignores Launch::program().
    #[cfg(windows)]
    let file = "C:\\Windows\\explorer.exe".to_string();
    #[cfg(not(windows))]
    let file = if Path::new("/usr/bin/open").is_file() {
        "/usr/bin/open".into()
    } else {
        "open".into()
    };
    let _ = target;
    Spec {
        file,
        args: vec![target.to_string()],
        cwd: OPENER_CWD.into(),
        root: root.to_path_buf(),
        env: vec![],
        argv0: None,
        path: None,
        prog: None,
        dir: None,
    }
}

pub fn shell_open<R>(
    seen: TrustSeen,
    t: &Local,
    opener: &Spec,
    path: ChildPath,
    trusted: impl FnOnce() -> bool,
    exec: impl FnOnce(Launch) -> Result<R, String>,
) -> Result<R, String> {
    t.recheck()?;
    let life = lifecycle();
    seen.unmoved(&life)?;
    if !trusted() {
        return Err("EPERM: restricted mode, plugins cannot use the shell".into());
    }
    let prog = opener.prog.as_ref().ok_or("EINVAL: opener not pinned")?;
    let launch = Launch {
        arg: &t.arg,
        cwd: Path::new(OPENER_CWD),
        path: path.0,
        prog,
        argv0: opener.argv0.as_deref().unwrap_or(&opener.file),
    };
    exec(launch)
}

pub struct ShellPrep {
    pub slot: crate::consent::Slot,
    pub opener: Spec,
    pub target: Local,
    pub ask: bool,
}

pub fn shell_prepare(root: &Path, op: &str, target: &str) -> Result<ShellPrep, String> {
    let slot = crate::consent::admit()?;
    let (opener, target, ask) = match shell_target(root, op, target)? {
        ShellOpen::Url(url) => (crate::consent::resolve(opener_spec(root, &url))?, Local::url(url), false),
        ShellOpen::Ask(t) => {
            let sp = opener_spec(root, &t.arg);
            crate::consent::check_env(&sp)?;
            (crate::consent::resolve(sp)?, t, true)
        }
    };
    Ok(ShellPrep {
        slot,
        opener,
        target,
        ask,
    })
}

#[cfg(test)]
pub fn fs_work() {}

#[cfg(test)]
pub fn fs_work_count() -> usize {
    0
}
