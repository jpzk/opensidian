// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* cp-unrestricted (operator ruling 2026-10-18, stock parity): the real
   `child_process` behind ui/plugins/node-desktop.js. Stock runs a plugin's
   spawn/exec with the user's own rights; so do we, with one gate: the vault is
   trusted (the restricted-mode flip, ~/.opensidian.json). main.rs checks that
   on every spawn, and (spawn-consent, operator 2026-11-01, overriding the
   2026-10-18 "no per-plugin consent" ruling for spawns) asks the user in a
   native dialog (consent.rs) before EVERY start() — Deny = nothing forked.

   Process model, adapted from goal/td-desktop-caps desktop_claude.rs (our code)
   without its grants, broker dir, minimal env or output caps:
     - one long-lived thread does every fork, so PR_SET_PDEATHSIG (bound to the
       forking thread) fires only when the app itself goes away;
     - the child gets its own process group (kill reaches git's helpers too),
       stdio pipes, and no other fd of ours (close_range CLOEXEC);
     - two pump threads fill byte buffers the page drains with read();
     - a waiter reaps and records the exit code or the signal.
   cwd: the page sees the vault as "/vault" (getBasePath), so "/vault" and
   anything under it map to the real vault dir; a relative cwd or none at all
   is the vault root. Any other absolute dir is used as given (stock).
   env: the page's process.env is an empty shim, so a plugin's
   {...process.env, X: y} names only X. The child starts from the app's real
   environment and the plugin's keys are laid over it — what stock's child sees
   for the same call.
   Bytes cross the IPC as latin1 strings (one char per byte, lossless both ways),
   so no codec crate is needed. */
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

/// the page-side name of the vault root (FileSystemAdapter.getBasePath)
pub const VAULT_ALIAS: &str = "/vault";

/// "/vault[/x]" -> root[/x]; relative or empty -> under root; other absolute -> as given.
/// R3 (PR #42): exactly "" / "." / "/vault" are special; nothing is trimmed (" a .md" is that name)
pub fn map_cwd(root: &Path, cwd: Option<&str>) -> PathBuf {
    let c = cwd.unwrap_or(""); // R3: never trimmed — " a.md" and "a.md" are different paths
    if c.is_empty() || c == "." {
        return root.to_path_buf();
    }
    if c == VAULT_ALIAS {
        return root.to_path_buf();
    }
    if let Some(rest) = c.strip_prefix(VAULT_ALIAS).and_then(|r| r.strip_prefix('/')) {
        return root.join(rest);
    }
    if c.starts_with('/') {
        return PathBuf::from(c);
    }
    root.join(c)
}

/// an argument naming a path under "/vault" means the same file in the real dir
/// (simple-git passes `-C <basePath>`-style paths, git config core.worktree, ...)
pub fn map_arg(root: &Path, a: &str) -> String {
    if a == VAULT_ALIAS || a.starts_with("/vault/") {
        return map_cwd(root, Some(a)).to_string_lossy().into_owned();
    }
    a.to_string()
}

/// app.openWithDefaultApp(path): a VAULT path only. "x" / "/vault/x" name a file or folder in
/// the vault; any other absolute path, any ".." component, a NUL, or a path whose real location
/// (symlinks resolved) is outside the vault's real root is refused with EPERM. The target must
/// exist (ENOENT otherwise), so the opener is never handed a name it would guess at.
pub fn vault_open_target(root: &Path, target: &str) -> Result<PathBuf, String> {
    let t = target; // R3: never trimmed — a name with leading/trailing spaces is that name
    if t.contains('\0') {
        return Err(format!("EPERM: vault-escape refused: {target}"));
    }
    let rel = if t == VAULT_ALIAS {
        ""
    } else if let Some(r) = t.strip_prefix(VAULT_ALIAS).and_then(|r| r.strip_prefix('/')) {
        r
    } else if t.starts_with('/') || t.starts_with('\\') || t.starts_with('~') {
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
    /// latin1 byte strings
    pub out: String,
    pub err: String,
    pub exit: Option<i32>,
    /// Node signal name ("SIGTERM") when the child died of a signal
    pub signal: Option<&'static str>,
    /// reaped and both pipes drained: nothing more will ever come
    pub done: bool,
}

pub fn sig_name(n: i32) -> &'static str {
    match n {
        libc::SIGHUP => "SIGHUP",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        libc::SIGKILL => "SIGKILL",
        libc::SIGPIPE => "SIGPIPE",
        libc::SIGALRM => "SIGALRM",
        libc::SIGTERM => "SIGTERM",
        libc::SIGUSR1 => "SIGUSR1",
        libc::SIGUSR2 => "SIGUSR2",
        libc::SIGABRT => "SIGABRT",
        libc::SIGSEGV => "SIGSEGV",
        _ => "SIGTERM",
    }
}

pub fn sig_num(name: &str) -> Option<i32> {
    Some(match name.trim_start_matches("SIG").to_ascii_uppercase().as_str() {
        "HUP" => libc::SIGHUP,
        "INT" => libc::SIGINT,
        "QUIT" => libc::SIGQUIT,
        "KILL" => libc::SIGKILL,
        "PIPE" => libc::SIGPIPE,
        "ALRM" => libc::SIGALRM,
        "" | "TERM" => libc::SIGTERM,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        "ABRT" => libc::SIGABRT,
        _ => return None,
    })
}

impl Run {
    pub fn pid(&self) -> i32 {
        self.pid
    }

    /// signal the child's process group; false once it is reaped (Node: kill() -> false)
    pub fn kill(&self, sig: i32) -> bool {
        if self.st.lock().unwrap_or_else(|e| e.into_inner()).reaped {
            return false;
        }
        // SAFETY: plain syscall on our child's own group (process_group(0))
        unsafe { libc::kill(-self.pid, sig) == 0 }
    }

    pub fn write(&self, data: &[u8]) -> Result<(), String> {
        let mut g = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
        let w = g.as_mut().ok_or("EPIPE: stdin is closed")?;
        w.write_all(data).and_then(|_| w.flush()).map_err(|e| format!("EPIPE: {e}"))
    }

    pub fn end(&self) {
        *self.stdin.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// what arrived since the last read, waiting up to `wait` for something new
    pub fn read(&self, wait: Duration) -> Chunk {
        let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
        let idle = |s: &State| s.out.is_empty() && s.err.is_empty() && !(s.reaped && s.pumps == 0);
        if idle(&s) {
            s = self.cv.wait_timeout_while(s, wait, |s| idle(s)).unwrap_or_else(|e| e.into_inner()).0;
        }
        let out = bytes_to_latin1(&std::mem::take(&mut s.out));
        let err = bytes_to_latin1(&std::mem::take(&mut s.err));
        let done = s.reaped && s.pumps == 0;
        Chunk { out, err, exit: if done { s.exit } else { None }, signal: if done { s.signal.map(sig_name) } else { None }, done }
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
    /// what is exec'd. main.rs sets it to the ABSOLUTE path consent::resolve found and the
    /// dialog named (F1): the child never repeats a PATH lookup of its own
    pub file: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// S3: the vault the child is started for: PATH entries inside it are dropped
    pub root: PathBuf,
    pub env: Vec<(String, String)>,
    /// argv[0] the child sees (the name the plugin asked for); None = file
    pub argv0: Option<String>,
    /// Q1 (PR #42 round 4): the PATH the child runs with, cleaned by prepare() BEFORE the
    /// lifecycle lock is taken. None = not prepared: start refuses, it never cleans one itself
    pub path: Option<ChildPath>,
}

/* Q1 (PR #42 round 4, Greptile + Qodo + CodeAnt): cleaning a PATH canonicalizes every
   entry (path_dir_ok) — filesystem lookups of plugin-chosen directories. An entry on a
   stalled NFS/FUSE mount blocks there for as long as the mount does. This used to run in
   build, i.e. inside start, i.e. while desk_proc_spawn held the lifecycle lock — and a
   trust-OFF (plugins_set: flag write, kill_all, close_all) waits on that lock, so one
   stalled PATH entry kept restricted mode from ever coming back. Now every lookup is done
   by prepare() (spawn) and opener_path() (the shell opener) before the lock; under it only
   the trust recheck, the fork/exec and the registration run. A ChildPath is made only in
   this file (prepare, opener_path), so a Spec cannot carry an uncleaned PATH into start.
   Still under the lock, inherently: the cwd check and the fork/exec itself (chdir + execve
   of the absolute program the dialog named). */
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildPath(OsString);

impl ChildPath {
    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

/// Q1: the Spec with its child PATH cleaned — call BEFORE desk_proc::lifecycle().
/// `inherited` = the app's PATH (std::env::var_os), used when the plugin sets none (S2)
pub fn prepare(mut sp: Spec, inherited: Option<OsString>) -> Spec {
    let p = child_path(&spec_path(&sp, inherited), &[sp.root.as_path(), sp.cwd.as_path()]);
    sp.path = Some(ChildPath(p));
    sp
}

/// Q1: the opener's PATH (the app's, cleaned; `root` = the vault) — call BEFORE the lock
pub fn opener_path(ours: Option<OsString>, root: &Path) -> ChildPath {
    ChildPath(child_path(&ours.unwrap_or_default(), &[root, Path::new(OPENER_CWD)]))
}

/* R4 (PR #42, Qodo): F1 already refuses an empty / "." / relative PATH entry when the
   PROGRAM is looked up; the child does lookups of its own (xdg-open is a shell script
   that finds gio, kde-open, ... on PATH; git finds its helpers; sh finds every command),
   and exec resolves such an entry against the child's cwd — inside the vault, which
   plugins write. So every child (spawn and shell opener) gets PATH with only its
   absolute entries, in order.
   S1 (PR #42 round 3, Greptile): when nothing absolute is left (PATH "tools", ".", "") the
   child gets SAFE_PATH — never an empty PATH, which sh and execvp read as "the cwd".
   S2 (round 3, CodeAnt): the PATH is bytes (OsStr), filtered byte-wise; a non-UTF-8
   inherited PATH is cleaned like any other, never passed through raw, and every child
   gets PATH set explicitly.
   S3 (round 3, CodeAnt): an absolute entry can still lead into the cwd or the vault:
   /proc/self/cwd, /proc/<pid>/cwd, /proc/self/fd/N, /dev/fd/N, or a plain path under the
   vault (PATH=/abs/vault/bin). Dropped: every entry under /proc or /dev/fd, and every
   entry inside one of the `confine` roots (the vault, the child's cwd) — tested on its
   lexical form (., .., // folded) and on its canonical form when it resolves. A confine
   root of "/" confines nothing (the opener's cwd: plugins cannot write it). */
pub const SAFE_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// S3: `p` with "." and "//" folded and ".." popped, without touching the filesystem
fn lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::from("/");
    for c in p.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::ParentDir => {
                out.pop();
            }
            _ => {}
        }
    }
    out
}

/* V1 (PR #42 round 5, CodeAnt): magic links are resolved PER PROCESS. /proc/self/cwd,
   /proc/thread-self/..., /proc/<pid>/{cwd,root,fd/N} and /dev/fd/N name a different
   target in every process that follows them, so a symlink outside the vault pointing at
   /proc/self/cwd (directly or through a chain) canonicalizes IN THE APP to the app's own
   cwd — passing every check here — while the child, following the same link, lands in ITS
   cwd: the vault. No check of the entry as written can see that. So the child never gets
   the entry as written: it gets the entry's canonical form as resolved here (absolute, no
   symlink or magic link left, so the child resolves it to the same directory we
   checked), and the /proc, /dev/fd and vault/cwd rules are applied to that form (and the
   lexical one). An entry that does not canonicalize (missing, not a dir we can resolve,
   a loop) is dropped; one whose canonical form holds a ':' (it would split PATH) too. */
/// S3 + V1: the directory the child may look programs up in for `dir` (an absolute PATH
/// entry): its canonical form, or None = drop the entry
fn path_dir_ok(dir: &Path, confine: &[&Path]) -> Option<PathBuf> {
    #[cfg(test)]
    tests::q1_gate(dir);
    let canon = dir.canonicalize().ok().filter(|c| c.is_absolute() && !c.as_os_str().as_bytes().contains(&b':'))?;
    let forms = [lexical(dir), canon.clone()];
    let mut roots: Vec<PathBuf> = Vec::new();
    for r in confine.iter().filter(|r| r.is_absolute()) {
        roots.push(lexical(r));
        if let Ok(c) = r.canonicalize() {
            roots.push(c);
        }
    }
    roots.retain(|r| r.as_os_str() != "/");
    let bad = forms.iter().any(|f| f.starts_with("/proc") || f.starts_with("/dev/fd") || roots.iter().any(|r| f.starts_with(r)));
    (!bad).then_some(canon)
}

/// R4 + S1-S3 + V1: the PATH a child runs with, from the PATH it would have had — each
/// kept entry in its canonical form (V1)
pub fn child_path(path: &OsStr, confine: &[&Path]) -> OsString {
    let mut out: Vec<u8> = Vec::new();
    for d in path.as_bytes().split(|b| *b == b':') {
        if d.first() != Some(&b'/') {
            continue;
        }
        let Some(c) = path_dir_ok(Path::new(OsStr::from_bytes(d)), confine) else {
            continue;
        };
        if !out.is_empty() {
            out.push(b':');
        }
        out.extend_from_slice(c.as_os_str().as_bytes());
    }
    if out.is_empty() {
        return OsString::from(SAFE_PATH);
    }
    OsString::from_vec(out)
}

/// S2: what build's child PATH is cleaned from — the plugin's PATH, else the app's
/// (`inherited`, read by the caller with var_os, never as a String)
fn spec_path(sp: &Spec, inherited: Option<OsString>) -> OsString {
    sp.env.iter().rev().find(|(k, _)| k == "PATH").map(|(_, v)| OsString::from(v)).or(inherited).unwrap_or_default()
}

fn build(sp: &Spec, path: &ChildPath) -> Command {
    let mut c = Command::new(&sp.file);
    if let Some(a0) = &sp.argv0 {
        c.arg0(a0);
    }
    c.args(&sp.args).current_dir(&sp.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    for (k, v) in &sp.env {
        c.env(k, v);
    }
    // R4/S1-S3: the child's own PATH lookups never land in its cwd or the vault; always set.
    // Q1: cleaned by prepare() before the lifecycle lock — no PATH entry is looked up here
    c.env("PATH", path.as_os_str());
    // SAFETY: async-signal-safe syscalls only, between fork and exec
    unsafe {
        c.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0, 0, 0);
            if libc::getppid() == 1 {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            // CLOEXEC, not close: std reports a failed exec (ENOENT) over its own cloexec
            // pipe, which a plain close_range would cut (spawn would "succeed")
            libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32 /* CLOSE_RANGE_CLOEXEC */);
            Ok(())
        });
    }
    c
}

fn fork(sp: Spec, path: ChildPath) -> std::io::Result<Child> {
    let (tx, rx) = std::sync::mpsc::channel();
    forker()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .send(Box::new(move || {
            let _ = tx.send(build(&sp, &path).spawn());
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
                if is_err { s.err.extend_from_slice(&b[..n]) } else { s.out.extend_from_slice(&b[..n]) }
            }
        }
        r.cv.notify_all();
    }
    r.st.lock().unwrap_or_else(|e| e.into_inner()).pumps -= 1;
    r.cv.notify_all();
}

/// Node-style error text: "<CODE>: spawn <file> <reason>"
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
    if !sp.cwd.is_dir() {
        return Err(format!("ENOENT: spawn {} cwd {} is not a directory", sp.file, sp.cwd.display()));
    }
    // Q1: the PATH was cleaned before the lock (prepare); unprepared = refused, never cleaned here
    let Some(path) = sp.path.clone() else {
        return Err(format!("EINVAL: spawn {}: child PATH not prepared before the lifecycle lock (desk_proc::prepare)", sp.file));
    };
    let file = sp.file.clone();
    let mut child = fork(sp, path).map_err(|e| spawn_err(&file, &e))?;
    let run = Arc::new(Run {
        pid: child.id() as i32,
        stdin: Mutex::new(child.stdin.take()),
        st: Mutex::new(State { pumps: 2, ..State::default() }),
        cv: Condvar::new(),
    });
    let (o, e) = (child.stdout.take(), child.stderr.take());
    for (src, is_err) in [(o.map(|x| Box::new(x) as Box<dyn Read + Send>), false), (e.map(|x| Box::new(x) as Box<dyn Read + Send>), true)] {
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
            s.signal = st.signal();
        }
        s.reaped = true;
        drop(s);
        r.cv.notify_all();
    });
    Ok(run)
}

/* ---- F5 (PR #41): one lock over the spawn and the trust-OFF lifecycles. desk_proc_spawn
   holds it across (trust recheck, start, register); plugins_set holds it across (flag
   write OFF, kill_all, close_all). Without it a spawn whose recheck passed just
   before the flip could start and register AFTER kill_all ran, and outlive restricted
   mode. start, register and kill_all take a &Life, so none of them can be called
   without holding it. */
pub struct Life(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

static LIFE: Mutex<()> = Mutex::new(());

pub fn lifecycle() -> Life {
    let g = LIFE.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(test)]
    HELD.with(|h| h.set(true));
    Life(g)
}

/* P2 (PR #42): tests ask whether THIS thread holds the lifecycle lock right now (a Life
   is !Send, so it lives and drops on the thread that took it). Lets a test assert, at an
   exact point and with no timing, that slow work (a DNS lookup) runs outside the lock. */
#[cfg(test)]
thread_local! {
    static HELD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
impl Drop for Life {
    fn drop(&mut self) {
        HELD.with(|h| h.set(false));
    }
}

/// true while this thread holds the lifecycle lock (the P2 test's probe)
#[cfg(test)]
pub fn held_here() -> bool {
    HELD.with(|h| h.get())
}

/// the lock if it is free right now, None while another holder has it: lets a test put
/// the other side of a race at an exact point without a sleep (N4/N5)
#[cfg(test)]
pub fn try_lifecycle() -> Option<Life> {
    match LIFE.try_lock() {
        Ok(g) => {
            HELD.with(|h| h.set(true));
            Some(Life(g))
        }
        Err(std::sync::TryLockError::Poisoned(e)) => {
            HELD.with(|h| h.set(true));
            Some(Life(e.into_inner()))
        }
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

/* ---- the live runs, keyed by an id the page holds */
fn runs() -> &'static Mutex<HashMap<u64, Arc<Run>>> {
    static R: OnceLock<Mutex<HashMap<u64, Arc<Run>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register(r: Arc<Run>, _life: &Life) -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    runs().lock().unwrap_or_else(|e| e.into_inner()).insert(id, r);
    id
}

pub fn get(id: u64) -> Result<Arc<Run>, String> {
    runs().lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned().ok_or_else(|| "ESRCH: no such child".into())
}

/// a read that reported done drops the run
pub fn forget(id: u64) {
    runs().lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
}

/// flip OFF: plugin code stops, and so does everything it started
pub fn kill_all(_life: &Life) {
    let all: Vec<Arc<Run>> = runs().lock().unwrap_or_else(|e| e.into_inner()).drain().map(|(_, r)| r).collect();
    for r in all {
        r.kill(libc::SIGKILL);
        r.end();
    }
}

/* ---- G1 (PR #41, Qodo #2): the trust epoch. plugins_set reads it (under the lifecycle
   lock) before the trust dialog opens and hands it to trust_write() under the lock it
   writes the flag under. Every trust write — ON or OFF, asked or not — goes through
   trust_write and moves the epoch on, so an answer from a dialog that was open across
   another flip (a trust-OFF with its kill_all + close_all, or another ON) is stale:
   it is discarded with an error and nothing is written. Only &Life holders touch it. */
static TRUST_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn trust_epoch(_life: &Life) -> u64 {
    TRUST_EPOCH.load(std::sync::atomic::Ordering::SeqCst)
}

/// call under `life` right before writing the trust flag. `asked` = the epoch read before
/// the trust dialog opened (None = nothing was asked). Err = a stale answer: write nothing.
pub fn trust_write(asked: Option<u64>, _life: &Life) -> Result<(), String> {
    use std::sync::atomic::Ordering::SeqCst;
    if let Some(e) = asked {
        if TRUST_EPOCH.load(SeqCst) != e {
            return Err("EAGAIN: community plugins were switched while the trust question was open; that answer was discarded, nothing written — ask again".into());
        }
    }
    TRUST_EPOCH.fetch_add(1, SeqCst);
    Ok(())
}

/* ---- P1 (PR #42, CodeAnt): what a plugins_set call decides before its write. `seen` is
   the epoch read FIRST; `trusted_now` (the vault's trust flag) is read after it, outside
   the lifecycle lock. A trust-OFF can land after that read and before the write takes the
   lock — so EVERY ON write carries `seen`, asked or not: a vault that was already trusted
   is not asked, but its write is still refused (EAGAIN, nothing written) when a flip came
   in between, instead of silently writing trusted:true over the OFF. An OFF (or no trust
   change) carries nothing: turning trust OFF is never refused. Returns (ask the user?,
   the epoch for trust_write). */
pub fn trust_plan(on: bool, seen: u64, trusted_now: impl FnOnce() -> bool) -> (bool, Option<u64>) {
    if !on {
        return (false, None);
    }
    (!trusted_now(), Some(seen))
}

/* ---- G3 (PR #41): electron shell / app.openWithDefaultApp hand a target to xdg-open,
   which runs whatever the desktop maps it to: a .desktop file's Exec=, an executable or a
   script (by mime handler), an AppImage, a jar — and a custom scheme (steam:, any
   x-scheme-handler an app registered) runs that handler. Telling a harmless local file
   from one that runs code is not decidable here (mime sniffing, user handlers, the exec
   bit), so this does not try: EVERY local target (op path / vault / reveal, a file: URL,
   and op external with any scheme but http, https, mailto) is asked exactly like a spawn —
   program xdg-open, the target in full — and like every spawn it is asked each time (no
   answer is remembered between runs). http(s) and mailto URLs go to the browser / mail
   client unasked, as before.
   P4 (PR #42, CodeAnt): the dialog approves a PATHNAME, and the plugin can rewrite the
   vault while the dialog is open — replace the file (another inode) or swap a symlink to a
   .desktop/script in. So a local target is resolved BEFORE asking (canonicalize: every
   symlink followed, the dialog shows the final path, and that final path — never the
   plugin's spelling — is what xdg-open gets), its identity is recorded (lstat: dev, ino,
   mode, size, mtime, ctime — Q2), and after Allow `Local::recheck` lstats the same path again right
   before the exec (desk_shell, under the lifecycle lock): any difference — another inode,
   a symlink where the file was, changed contents/size/mtime, gone — refuses with EPERM and
   nothing is opened. A file: URL is turned into its path (percent-decoded) and goes the
   same way, so it cannot route around the binding.
   RESIDUAL WINDOW — S4 (PR #42 round 3, CodeAnt "High Risk" TOCTOU), documented, NOT
   closed: Local::recheck compares identities by path, and xdg-open then opens the target
   by NAME, twice over: its own mime lookup (xdg-mime / gio sniff the file) and then the
   handler it starts, which opens the name again. A plugin that rewrites the vault (page
   script through the app's file calls, or a child it was allowed to start) and wins the
   race between the recheck and that later open makes the handler act on another file
   than the one allowed — e.g. a .desktop or an executable script swapped in for a note.
   The window is the fork/exec of xdg-open plus its mime lookup and the handler's start
   (milliseconds up to about a second), no longer "as long as the user reads the dialog".
   Why there is no fix inside this module: xdg-open takes only a pathname or URL. Handing
   it a /proc/<pid>/fd/N magic link of an fd we verified (fstat) pins the inode but loses
   the name: the mime type is guessed from the extension, a .desktop is recognised by its
   name, a directory (reveal) shows a /proc path, and the link dies with the fd before the
   handler has opened it. Handing it a private read-only copy outside the vault pins the
   contents but breaks editing in place and cannot copy a directory. Holding off vault
   writes for "a while" is a timing guess, and does not cover a running child. So: the
   dialog names the canonical path, the identity is rechecked right before the exec, the
   opener runs in "/" with a cleaned PATH, and this residual stays (pr2d.md, Known
   limitations). Custom-scheme URLs (steam:, obsidian:, ...) name no file: nothing to
   bind, asked as text. */
pub enum ShellOpen {
    /// an http(s)/mailto URL: opened without asking
    Url(String),
    /// anything else: the spawn consent dialog first, then xdg-open <arg> after Local::recheck
    Ask(Local),
}

/// what one stat of the target says it is (lstat: a symlink is its own identity)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    dev: u64,
    ino: u64,
    mode: u32,
    size: u64,
    mtime: (i64, i64),
    /// Q2 (PR #42 round 4, CodeAnt): the inode change time. mtime can be set back by any
    /// writer (utimensat / touch -d); ctime cannot be set from userspace: every write, chmod,
    /// rename or utimensat moves it to "now". A same-size rewrite with its mtime restored
    /// keeps (dev, ino, mode, size, mtime) — not ctime.
    ctime: (i64, i64),
}

fn ident_of(p: &Path) -> Result<Ident, String> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(p).map_err(|e| format!("ENOENT: {}: {e}", p.display()))?;
    Ok(Ident { dev: m.dev(), ino: m.ino(), mode: m.mode(), size: m.size(), mtime: (m.mtime(), m.mtime_nsec()), ctime: (m.ctime(), m.ctime_nsec()) })
}

/// a target the dialog asks about: `arg` is what the dialog shows and what xdg-open gets;
/// `ident` is the recorded identity of a local file (None for a custom-scheme URL)
pub struct Local {
    pub arg: String,
    ident: Option<Ident>,
}

impl Local {
    /// an http(s)/mailto URL handed on unasked: no file to bind
    pub fn url(u: String) -> Local {
        Local { arg: u, ident: None }
    }

    fn file(p: PathBuf, target: &str) -> Result<Local, String> {
        let real = p.canonicalize().map_err(|_| format!("ENOENT: no such file or directory, '{target}'"))?;
        let ident = ident_of(&real)?;
        let arg = real.into_os_string().into_string().map_err(|_| format!("EINVAL: not a UTF-8 path: {target}"))?;
        Ok(Local { arg, ident: Some(ident) })
    }

    /// P4: right before the exec — the path the user allowed is still the very same file.
    /// R1 (PR #42, Greptile): a DIRECTORY (reveal opens the parent dir; a folder through
    /// path/vault) is the same directory while (dev, ino, mode) are: its size/mtime change
    /// whenever any entry in it is created, renamed or removed (saving a sibling note while
    /// the dialog is open), which is not a swap. Files keep the full identity.
    pub fn recheck(&self) -> Result<(), String> {
        let Some(was) = &self.ident else { return Ok(()) };
        match ident_of(Path::new(&self.arg)) {
            Ok(now) if now.same_as(was) => Ok(()),
            Ok(_) => Err(format!("EPERM: {} changed after it was allowed (replaced, swapped for a symlink, or rewritten); not opened", self.arg)),
            Err(_) => Err(format!("EPERM: {} is gone since it was allowed; not opened", self.arg)),
        }
    }
}

impl Ident {
    /// R1: a directory is compared on (dev, ino, mode) only; anything else on all of it
    fn same_as(&self, was: &Ident) -> bool {
        if was.mode & libc::S_IFMT == libc::S_IFDIR {
            return (self.dev, self.ino, self.mode) == (was.dev, was.ino, was.mode);
        }
        self == was
    }
}

/// file:[//[localhost]]/abs/path[?q][#f] -> /abs/path, percent-decoded; anything else is EINVAL
fn file_url_path(target: &str) -> Result<PathBuf, String> {
    let bad = || format!("EINVAL: not a local file: URL: {target}");
    let rest = &target[5..]; // "file:" (any case) checked by the caller
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let path = match rest.strip_prefix("//") {
        Some(r) => {
            let (host, p) = r.split_at(r.find('/').unwrap_or(r.len()));
            if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
                return Err(bad());
            }
            p
        }
        None => rest,
    };
    if !path.starts_with('/') {
        return Err(bad());
    }
    let b = path.as_bytes();
    let (mut out, mut i) = (Vec::with_capacity(b.len()), 0);
    while i < b.len() {
        if b[i] == b'%' {
            let h = b.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or_else(bad)?;
            out.push(h);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    let s = String::from_utf8(out).map_err(|_| bad())?;
    if s.contains('\0') {
        return Err(bad());
    }
    Ok(PathBuf::from(s))
}

pub fn shell_target(root: &Path, op: &str, target: &str) -> Result<ShellOpen, String> {
    match op {
        "external" => {
            let scheme = target.split(':').next().unwrap_or("");
            let ok = target.contains(':') && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
            if !ok {
                return Err(format!("EINVAL: not a URL: {target}"));
            }
            match scheme.to_ascii_lowercase().as_str() {
                "http" | "https" | "mailto" => Ok(ShellOpen::Url(target.to_string())),
                "file" => Local::file(file_url_path(target)?, target).map(ShellOpen::Ask),
                _ => Ok(ShellOpen::Ask(Local { arg: target.to_string(), ident: None })),
            }
        }
        "path" => Local::file(map_cwd(root, Some(target)), target).map(ShellOpen::Ask),
        // app.openWithDefaultApp: vault paths only, symlinks resolved and confined to the vault
        "vault" => Local::file(vault_open_target(root, target)?, target).map(ShellOpen::Ask),
        "reveal" => {
            let p = map_cwd(root, Some(target));
            Local::file(p.parent().map(|d| d.to_path_buf()).unwrap_or(p), target).map(ShellOpen::Ask)
        }
        _ => Err(format!("EINVAL: unknown shell op {op}")),
    }
}

/* P4: the exec of xdg-open itself — the identity recheck, then `exec` (the caller's
   fork of xdg-open with t.arg), nothing in between (desk_shell calls it under the
   lifecycle lock, after the trust recheck). A changed target returns the EPERM and
   `exec` is never called. (The process is built by the caller: the only
   command builder in this file is fn build — consent-nobypass S1.)
   R4 (PR #42, Qodo): the opener is a script that looks up its helpers on PATH, so it
   runs in OPENER_CWD ("/", which plugins cannot write) — never the vault — with PATH
   cut to its absolute entries (child_path). `exec` gets both in a Launch and applies
   them with Launch::apply; the PATH is opener_path(the app's PATH), made before the lock (Q1).
   S1-S3 (PR #42 round 3): the same child_path as a spawn — bytes, /proc and /dev/fd and
   the vault (`root`) dropped, SAFE_PATH when nothing is left — and PATH is always set. */
pub const OPENER_CWD: &str = "/";

/// R4: what the opener is started with: its one argument (the allowed target), cwd, PATH
pub struct Launch<'a> {
    pub arg: &'a str,
    pub cwd: &'static Path,
    pub path: OsString,
}

impl Launch<'_> {
    /// the target, the cwd and the cleaned PATH, onto the caller's opener Command
    pub fn apply<'c>(&self, c: &'c mut Command) -> &'c mut Command {
        c.arg(self.arg).current_dir(self.cwd).env("PATH", &self.path)
    }
}

/// Q1: the identity recheck and the exec, nothing else: `path` = opener_path(), computed
/// before the caller took the lifecycle lock — no PATH entry is looked up in here
pub fn shell_spawn<R>(t: &Local, path: ChildPath, exec: impl FnOnce(Launch) -> Result<R, String>) -> Result<R, String> {
    t.recheck()?;
    exec(Launch { arg: &t.arg, cwd: Path::new(OPENER_CWD), path: path.0 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(file: &str, args: &[&str], cwd: &Path) -> Spec {
        Spec { file: file.into(), args: args.iter().map(|s| s.to_string()).collect(), cwd: cwd.into(), root: cwd.into(), env: vec![], argv0: None, path: None }
    }

    /// Q1: what desk_proc_spawn does before the lock — the child PATH cleaned from ours
    fn ready(sp: Spec) -> Spec {
        prepare(sp, std::env::var_os("PATH"))
    }

    /* Q1 test seam: path_dir_ok calls q1_gate(dir) first. While a test has armed it for
       one marker directory, the lookup of THAT entry stands still — it reports "entered"
       and waits for "go", like a canonicalize stuck on a stalled NFS/FUSE mount. Every
       other entry (other tests running in parallel) passes straight through: the armed
       pair is taken out of the mutex before waiting, so nobody waits on the mutex. */
    type Q1Gate = (PathBuf, std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>);
    static Q1_GATE: Mutex<Option<Q1Gate>> = Mutex::new(None);

    pub(super) fn q1_gate(dir: &Path) {
        let armed = {
            let mut g = Q1_GATE.lock().unwrap_or_else(|e| e.into_inner());
            if g.as_ref().is_some_and(|(d, _, _)| d == dir) { g.take() } else { None }
        };
        if let Some((_, entered, go)) = armed {
            let _ = entered.send(());
            let _ = go.recv();
        }
    }

    /* Q1 (PR #42 round 4, Greptile + Qodo + CodeAnt): a PATH entry whose lookup hangs (a
       stalled NFS/FUSE mount) must not hold the lifecycle lock — a trust-OFF waits on it.
       Thread A runs desk_proc_spawn's sequence after Allow: the PATH cleaned (prepare), the
       lock, the trust recheck, start, register. Its plugin PATH holds a marker entry whose
       lookup stands still (q1_gate) until the test says go. A marks the moment it holds the
       lock; while the lookup stands still that mark must be unset — exact, no sleep, no
       timeout, and not fooled by another test holding the lock for a moment (a try_lock
       was: it failed whenever a parallel test held it). The lookup may run on the fork
       thread, so held_here (per thread) cannot tell. The trust-OFF (plugins_set: lock, flag
       OFF, kill_all) then takes the lock, and afterwards A's recheck must refuse the spawn:
       nothing started, nothing registered. The gate is released before any assert, so a
       failure cannot leave A or the fork thread hanging. */
    #[test]
    fn desk_q1_a_stalled_path_entry_does_not_hold_the_lifecycle_lock() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc::channel;
        static TRUSTED: AtomicBool = AtomicBool::new(true);
        TRUSTED.store(true, Ordering::SeqCst);
        static A_LOCKED: AtomicBool = AtomicBool::new(false);
        A_LOCKED.store(false, Ordering::SeqCst);
        let stalled = std::env::temp_dir().join(format!("desk-q1-stalled-{}", std::process::id()));
        let (entered_tx, entered) = channel::<()>();
        let (go, go_rx) = channel::<()>();
        *Q1_GATE.lock().unwrap_or_else(|e| e.into_inner()) = Some((stalled.clone(), entered_tx, go_rx));
        let mut sp = spec("/bin/sleep", &["30"], &std::env::temp_dir());
        sp.env.push(("PATH".into(), format!("{}:/usr/bin:/bin", stalled.display())));
        let a = std::thread::spawn(move || {
            let sp = prepare(sp, None); // desk_proc_spawn: before the lock
            let life = lifecycle();
            A_LOCKED.store(true, Ordering::SeqCst); // A holds the lifecycle lock from here
            if !TRUSTED.load(Ordering::SeqCst) {
                return Err("EPERM: restricted mode".to_string());
            }
            let r = start(sp, &life)?;
            Ok((register(r.clone(), &life), r))
        });
        entered.recv().expect("the stalled PATH entry was never looked up");
        let held = A_LOCKED.load(Ordering::SeqCst);
        // the lookup stands still now. If A holds the lock here, a trust-OFF would wait on it:
        // release A and fail (taking the lock now would deadlock). Otherwise the trust-OFF
        // takes the lock — blocking is safe, A does not hold it; other tests hold it briefly.
        if !held {
            let life = lifecycle();
            TRUSTED.store(false, Ordering::SeqCst);
            kill_all(&life);
        }
        let _ = go.send(());
        let got = a.join().unwrap();
        if let Ok((id, r)) = &got {
            r.kill(libc::SIGKILL);
            forget(*id);
        }
        assert!(!held, "a PATH entry lookup stood still while A held the lifecycle lock");
        assert!(got.is_err(), "the spawn started after a trust-OFF that landed while its PATH was being cleaned");
        assert!(sp_never_cleaned_under_the_lock());
    }

    /// Q1: an unprepared Spec is refused by start — it never cleans a PATH under the lock
    fn sp_never_cleaned_under_the_lock() -> bool {
        start(spec("/bin/true", &[], &std::env::temp_dir()), &lifecycle()).is_err_and(|e| e.starts_with("EINVAL"))
    }

    fn drain(r: &Run) -> (Vec<u8>, Vec<u8>, Chunk) {
        let (mut o, mut e) = (vec![], vec![]);
        for _ in 0..200 {
            let c = r.read(Duration::from_millis(50));
            o.extend(latin1_to_bytes(&c.out));
            e.extend(latin1_to_bytes(&c.err));
            if c.done {
                return (o, e, c);
            }
        }
        panic!("child never finished");
    }

    #[test]
    fn desk_cwd_maps_the_vault_alias_to_the_real_dir() {
        let root = Path::new("/home/u/notes");
        assert_eq!(map_cwd(root, None), root);
        assert_eq!(map_cwd(root, Some("")), root);
        assert_eq!(map_cwd(root, Some("/vault")), root);
        assert_eq!(map_cwd(root, Some("/vault/sub/dir")), root.join("sub/dir"));
        assert_eq!(map_cwd(root, Some("sub")), root.join("sub"));
        assert_eq!(map_cwd(root, Some("/vaultx")), PathBuf::from("/vaultx"));
        assert_eq!(map_cwd(root, Some("/tmp")), PathBuf::from("/tmp"));
        assert_eq!(map_arg(root, "/vault/a.md"), "/home/u/notes/a.md");
        assert_eq!(map_arg(root, "--vault"), "--vault");
    }


    #[test]
    fn desk_open_target_is_vault_confined_and_symlink_safe() {
        let base = std::env::temp_dir().join(format!("desk-vot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("vault");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/a.md"), b"a").unwrap();
        std::fs::write(base.join("outside.txt"), b"o").unwrap();
        std::os::unix::fs::symlink(base.join("outside.txt"), root.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(root.join("sub/a.md"), root.join("in.md")).unwrap();
        let rr = root.canonicalize().unwrap();
        assert_eq!(vault_open_target(&root, "sub/a.md").unwrap(), rr.join("sub/a.md"));
        assert_eq!(vault_open_target(&root, "/vault/sub/a.md").unwrap(), rr.join("sub/a.md"));
        assert_eq!(vault_open_target(&root, "sub").unwrap(), rr.join("sub"));
        assert_eq!(vault_open_target(&root, "/vault").unwrap(), rr);
        assert_eq!(vault_open_target(&root, "in.md").unwrap(), rr.join("sub/a.md"));
        for bad in ["../outside.txt", "sub/../../outside.txt", "/etc/passwd", "/vaultx/a", "~/x", "link.txt", "a\0b"] {
            let e = vault_open_target(&root, bad).unwrap_err();
            assert!(e.starts_with("EPERM: vault-escape refused"), "{bad}: {e}");
        }
        assert!(vault_open_target(&root, "nope.md").unwrap_err().starts_with("ENOENT"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn desk_latin1_round_trips_every_byte() {
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(latin1_to_bytes(&bytes_to_latin1(&all)), all);
    }

    #[test]
    fn desk_spawn_runs_in_cwd_with_env_stdin_and_exit_code() {
        let d = std::env::temp_dir();
        let mut sp = spec("/bin/sh", &["-c", "pwd; printf %s \"$DESK_T\"; cat; echo e >&2; exit 3"], &d);
        sp.env.push(("DESK_T".into(), "v1".into()));
        let r = start(ready(sp), &lifecycle()).unwrap();
        r.write(b"in\xff").unwrap();
        r.end();
        let (o, e, c) = drain(&r);
        let want = format!("{}\nv1in\u{ff}", d.canonicalize().unwrap().display());
        assert_eq!(bytes_to_latin1(&o), want);
        assert_eq!(e, b"e\n");
        assert_eq!((c.exit, c.signal), (Some(3), None));
    }

    #[test]
    fn desk_kill_reports_the_signal_and_missing_exe_is_enoent() {
        let r = start(ready(spec("/bin/sleep", &["30"], &std::env::temp_dir())), &lifecycle()).unwrap();
        assert!(r.kill(libc::SIGTERM));
        let (_, _, c) = drain(&r);
        assert_eq!((c.exit, c.signal), (None, Some("SIGTERM")));
        assert!(!r.kill(libc::SIGTERM));
        let e = start(ready(spec("no-such-exe-desk", &[], &std::env::temp_dir())), &lifecycle()).err().unwrap();
        assert!(e.starts_with("ENOENT: spawn no-such-exe-desk"), "{e}");
        assert_eq!(sig_num("SIGKILL"), Some(libc::SIGKILL));
        assert_eq!(sig_num(""), Some(libc::SIGTERM));
    }

    #[test]
    fn desk_argv0_is_the_asked_name_while_the_absolute_path_is_execd() {
        let mut sp = spec("/bin/cat", &["/proc/self/cmdline"], &std::env::temp_dir());
        sp.argv0 = Some("cat-as-asked".into());
        let r = start(ready(sp), &lifecycle()).unwrap();
        r.end();
        let (o, _, c) = drain(&r);
        assert_eq!(o, b"cat-as-asked\0/proc/self/cmdline\0");
        assert_eq!(c.exit, Some(0));
    }

    /* F5: a spawn whose trust recheck passed must not start + register after a trust-OFF
       kill_all. Thread A is desk_proc_spawn (recheck, start, register), thread B is
       plugins_set OFF (flag write, kill_all). N4 (PR #41): the order is forced, no sleep
       decides it — B is started only once A holds the lock and its recheck passed; B
       first proves it cannot take the lock then (try_lifecycle = None, while A sits
       between recheck and start), tells A, and only then blocks on the lock for real.
       A starts and registers only after that message, so B's kill_all can only come
       after A's register, and must kill A's child. */
    #[test]
    fn desk_trust_off_cannot_slip_between_recheck_and_register() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc::channel;
        static TRUSTED: AtomicBool = AtomicBool::new(true);
        TRUSTED.store(true, Ordering::SeqCst);
        let (b_tried, a_hears) = channel::<bool>();
        let a = std::thread::spawn(move || {
            let life = lifecycle();
            if !TRUSTED.load(Ordering::SeqCst) {
                return None;
            }
            // the recheck passed: the trust-OFF arrives now
            let b = std::thread::spawn(move || {
                let got_in = try_lifecycle().is_some();
                b_tried.send(got_in).unwrap();
                let life = lifecycle();
                TRUSTED.store(false, Ordering::SeqCst);
                kill_all(&life);
            });
            let b_got_in = a_hears.recv().unwrap();
            let r = start(ready(spec("/bin/sleep", &["30"], &std::env::temp_dir())), &life).unwrap();
            Some((register(r.clone(), &life), r, b, b_got_in))
        });
        let (id, r, b, b_got_in) = a.join().unwrap().expect("A's recheck passed");
        b.join().unwrap();
        assert!(!b_got_in, "the trust-OFF took the lifecycle lock between A's recheck and its start");
        assert!(get(id).is_err(), "a child started across a trust-OFF is still registered");
        let (_, _, c) = drain(&r);
        assert_eq!(c.signal, Some("SIGKILL"), "the child started before the flip was killed by it");
    }

    /* F1 (PR #41): with PATH ":<bin>" or ".:<bin>" and the cwd in the vault, exec's own
       lookup would take the empty / "." entry against the child's cwd and run the vault's
       copy, while the dialog named <bin>/fakeprog. consent::resolve makes the file absolute
       (the path the dialog names) and that is what runs. */
    #[test]
    fn desk_the_path_the_dialog_names_is_the_one_that_runs() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("desk-f1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for (dir, say) in [("vault", "VAULT"), ("bin", "BIN")] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
            let f = base.join(dir).join("fakeprog");
            std::fs::write(&f, format!("#!/bin/sh\necho {say}\n")).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let bin = base.join("bin");
        for path in [format!(":{}", bin.display()), format!(".:{}", bin.display())] {
            let mut sp = spec("fakeprog", &[], &base.join("vault"));
            sp.env.push(("PATH".into(), path.clone()));
            let sp = crate::consent::resolve(sp).unwrap();
            assert_eq!(sp.file, bin.join("fakeprog").to_string_lossy(), "PATH={path:?}");
            assert!(crate::consent::spawn_text(&sp).contains(&format!("Program: {:?}", sp.file)));
            let r = start(ready(sp), &lifecycle()).unwrap();
            r.end();
            let (o, _, _) = drain(&r);
            assert_eq!(o, b"BIN\n", "PATH={path:?}: the vault's fakeprog ran");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /* G1 (PR #41, Qodo #2): plugins_set ON reads the epoch, the trust dialog stays open,
       a trust-OFF (flag write, kill_all, close_all) runs meanwhile, then the user
       presses Trust. That answer is stale: trust_write refuses it, so the flag stays OFF.
       An answer with no flip in between is written. (Tests that write the epoch run one at a
       time, EPOCH_TESTS: one running in parallel would make the other's answers stale.) */
    #[test]
    fn desk_g1_a_stale_trust_answer_is_discarded() {
        let _one = epoch_tests();
        let mut flag = false; // the vault's trust flag, as plugins_set writes it: OFF
        let seen = trust_epoch(&lifecycle()); // ON: read before the dialog opens
        {
            // trust-OFF while the dialog is open (plugins_set also runs kill_all + close_all here)
            let life = lifecycle();
            trust_write(None, &life).unwrap();
        }
        {
            // the user presses Trust now
            let life = lifecycle();
            match trust_write(Some(seen), &life) {
                Ok(()) => flag = true,
                Err(e) => assert!(e.starts_with("EAGAIN"), "{e}"),
            }
        }
        assert!(!flag, "a Trust answer from a dialog open across a trust-OFF was written");
        // no flip in between: the answer is fresh and written
        let seen = trust_epoch(&lifecycle());
        let life = lifecycle();
        trust_write(Some(seen), &life).expect("a fresh Trust answer is written");
        drop(life);
        // and the write moved the epoch on: that same answer again is stale
        assert!(trust_write(Some(seen), &lifecycle()).is_err(), "an answer outlived the write it caused");
    }

    /// the tests that write TRUST_EPOCH run one at a time
    static EPOCH_TESTS: Mutex<()> = Mutex::new(());
    fn epoch_tests() -> std::sync::MutexGuard<'static, ()> {
        EPOCH_TESTS.lock().unwrap_or_else(|e| e.into_inner())
    }

    /* P1 (PR #42, CodeAnt): plugins_set ON for a vault that is ALREADY trusted: nothing is
       asked. It reads the epoch, then is_trusted (true, outside the lock); a trust-OFF
       (flag write, kill_all, close_all) lands right after that read; then the ON takes the
       lock and writes. That write must be refused — else trusted:true is written over the
       OFF, never asked. The order is the program order of this one thread (each step
       takes and drops the lock as plugins_set's do), so nothing but the code decides. */
    #[test]
    fn desk_p1_trust_off_between_on_request_and_write_is_not_undone() {
        let _one = epoch_tests();
        let mut flag = true; // the vault is trusted
        let seen = trust_epoch(&lifecycle()); // plugins_set ON: the epoch first
        let (ask, carries) = trust_plan(true, seen, || flag); // then is_trusted: true
        assert!(!ask, "an already trusted vault is asked again");
        {
            // the trust-OFF, between the is_trusted read and the ON's lock
            let life = lifecycle();
            trust_write(None, &life).expect("a trust-OFF is never refused");
            flag = false;
        }
        {
            // the ON request takes the lock and writes
            let life = lifecycle();
            if trust_write(carries, &life).is_ok() {
                flag = true;
            }
        }
        assert!(!flag, "a trust-OFF that landed between an ON request and its write was undone (trusted:true written, never asked)");
        // no flip in between: the ON for an already trusted vault is written
        let seen = trust_epoch(&lifecycle());
        let (ask, carries) = trust_plan(true, seen, || true);
        assert!(!ask);
        trust_write(carries, &lifecycle()).expect("an ON with no flip in between is written");
        // an OFF carries nothing and is never refused, whatever ran before it
        assert_eq!(trust_plan(false, seen, || unreachable!("an OFF reads no trust flag")), (false, None));
        // a vault that is not trusted is asked, and its write carries the epoch too
        assert_eq!(trust_plan(true, seen, || false), (true, Some(seen)));
    }

    /* G3 (PR #41): every local target of the electron shell / openWithDefaultApp — a
       .desktop file, a script, an executable, through path / vault / reveal, or a file: /
       custom-scheme URL — is asked (consent dialog, program xdg-open) before xdg-open
       sees it; http(s) and mailto URLs are not. */
    #[test]
    fn desk_g3_local_and_custom_scheme_opens_are_asked() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("desk-g3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub")).unwrap();
        for (n, body) in [("run.desktop", "[Desktop Entry]\nType=Application\nExec=touch /tmp/pwn\n"), ("x.sh", "#!/bin/sh\ntouch /tmp/pwn\n"), ("sub/bin", "#!/bin/sh\n")] {
            std::fs::write(base.join(n), body).unwrap();
            std::fs::set_permissions(base.join(n), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let real = base.canonicalize().unwrap();
        let desk = format!("file://{}", real.join("run.desktop").display());
        let asked: &[(&str, &str)] = &[
            ("path", "/vault/run.desktop"),
            ("path", "/vault/x.sh"),
            ("path", "/vault/sub/bin"),
            ("path", "/vault/sub"),
            ("vault", "run.desktop"),
            ("vault", "/vault/x.sh"),
            ("reveal", "/vault/run.desktop/x"),
            ("reveal", "/vault/sub/bin"),
            ("external", &desk),
            ("external", "FILE:///etc/passwd"),
            ("external", "steam://run/1"),
            ("external", "obsidian://open?vault=x"),
        ];
        for (op, t) in asked {
            match shell_target(&base, op, t) {
                Ok(ShellOpen::Ask(a)) => assert!(!a.arg.is_empty(), "{op} {t}"),
                Ok(ShellOpen::Url(u)) => panic!("{op} {t}: handed to xdg-open unasked as {u}"),
                Err(e) => panic!("{op} {t}: {e}"),
            }
        }
        match shell_target(&base, "path", "/vault/run.desktop") {
            Ok(ShellOpen::Ask(a)) => assert_eq!(a.arg, real.join("run.desktop").to_string_lossy(), "the dialog names the real target in full"),
            _ => panic!("path not asked"),
        }
        for u in ["https://example.org/a?b=c", "http://127.0.0.1:8080/", "HTTPS://x.org", "mailto:a@b.org"] {
            assert!(matches!(shell_target(&base, "external", u), Ok(ShellOpen::Url(ref x)) if x == u), "{u}: a web/mail URL is asked or changed");
        }
        assert!(shell_target(&base, "external", "/etc/passwd").is_err(), "not a URL");
        assert!(shell_target(&base, "vault", "../x").is_err(), "vault escape");
        let _ = std::fs::remove_dir_all(&base);
    }

    /* P4 (PR #42, CodeAnt): the dialog approves a path; a target replaced (another inode)
       or swapped for a symlink while the dialog is open is refused, not opened. The
       "dialog" is the hook between shell_target (what is asked) and shell_spawn (what
       runs): it runs the swap to completion before the answer, so the order is forced —
       no sleeps. The opener is a stub script that logs every target it is handed. */
    #[test]
    fn desk_p4_target_swapped_between_ask_and_run_is_not_opened() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("desk-p4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("v")).unwrap();
        let vault = base.join("v");
        let log = base.join("opened.log");
        let opener = base.join("opener");
        std::fs::write(&opener, format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n", log.display())).unwrap();
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(vault.join("Welcome.md"), "# hi\n").unwrap();
        std::fs::write(vault.join("evil.desktop"), "[Desktop Entry]\nType=Application\nExec=touch /tmp/pwn\n").unwrap();
        let real = vault.canonicalize().unwrap();
        let opened = || std::fs::read_to_string(&log).unwrap_or_default();
        // ask (shell_target), the user reads the dialog (`dialog` hook), Allow, run (shell_spawn)
        let ask_then_run = |op: &str, target: &str, dialog: &dyn Fn()| -> Result<String, String> {
            let t = match shell_target(&vault, op, target)? {
                ShellOpen::Ask(t) => t,
                ShellOpen::Url(u) => panic!("{op} {target}: not asked ({u})"),
            };
            dialog();
            let mut c = shell_spawn(&t, opener_path(None, &vault), |l| Command::new(&opener).arg(l.arg).spawn().map_err(|e| e.to_string()))?;
            assert!(c.wait().unwrap().success(), "the stub opener failed");
            Ok(t.arg)
        };
        let welcome = real.join("Welcome.md").to_string_lossy().into_owned();

        // control: nothing changes while the dialog is open -> opened (the stub works)
        assert_eq!(ask_then_run("path", "/vault/Welcome.md", &|| {}).as_deref(), Ok(welcome.as_str()));
        assert_eq!(opened(), format!("{welcome}\n"), "control: the untouched target reached the opener");

        // (a) replaced by another inode (write a new file, rename it over the asked path)
        let r = ask_then_run("path", "/vault/Welcome.md", &|| {
            std::fs::write(vault.join(".swap"), "[Desktop Entry]\nExec=touch /tmp/pwn\n").unwrap();
            std::fs::rename(vault.join(".swap"), vault.join("Welcome.md")).unwrap();
        });
        assert!(matches!(&r, Err(e) if e.starts_with("EPERM")), "(a) a replaced target was not refused: {r:?}");

        // (b) swapped for a symlink to a .desktop (through openWithDefaultApp's vault op)
        let r = ask_then_run("vault", "Welcome.md", &|| {
            std::fs::remove_file(vault.join("Welcome.md")).unwrap();
            std::os::unix::fs::symlink(vault.join("evil.desktop"), vault.join("Welcome.md")).unwrap();
        });
        assert!(matches!(&r, Err(e) if e.starts_with("EPERM")), "(b) a target swapped for a symlink was not refused: {r:?}");

        // (c) the same through a file: URL (openExternal): no route around the binding
        std::fs::remove_file(vault.join("Welcome.md")).unwrap();
        std::fs::write(vault.join("Welcome.md"), "# hi\n").unwrap();
        let url = format!("file://{}", welcome.replace(' ', "%20"));
        let r = ask_then_run("external", &url, &|| {
            std::fs::write(vault.join(".swap"), "#!/bin/sh\n").unwrap();
            std::fs::rename(vault.join(".swap"), vault.join("Welcome.md")).unwrap();
        });
        assert!(matches!(&r, Err(e) if e.starts_with("EPERM")), "(c) a file: URL target replaced was not refused: {r:?}");

        // (d) a target that already WAS a symlink: the dialog names (and xdg-open gets) the final path
        std::os::unix::fs::symlink(vault.join("evil.desktop"), vault.join("link.md")).unwrap();
        let shown = ask_then_run("path", "/vault/link.md", &|| {}).unwrap();
        assert_eq!(shown, real.join("evil.desktop").to_string_lossy(), "(d) the dialog does not name the symlink's final target");

        assert_eq!(opened(), format!("{welcome}\n{shown}\n"), "a refused target reached the opener");
        let _ = std::fs::remove_dir_all(&base);
    }

    /* Q2 (PR #42 round 4, CodeAnt): while the dialog is open the plugin rewrites the allowed
       file IN PLACE (same inode, same size) and sets its mtime back (utimensat) — every
       field P4 recorded (dev, ino, mode, size, mtime) is as before. ctime is not: the kernel
       moves it on the write and on the utimensat, and nothing in userspace sets it back.
       The rewrite is repeated until ctime has moved (the clock tick it is stamped with may
       be coarse; a user reading a dialog is always slower than one tick) — that loop is the
       only wait, and it ends on the kernel's own timestamp, not on a sleep. */
    #[test]
    fn desk_q2_a_same_size_rewrite_with_mtime_restored_is_refused() {
        use std::os::unix::fs::MetadataExt;
        let base = std::env::temp_dir().join(format!("desk-q2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let vault = base.join("v");
        std::fs::create_dir_all(&vault).unwrap();
        let note = vault.join("Note.md");
        std::fs::write(&note, "# safe note\n").unwrap();
        let t = match shell_target(&vault, "path", "/vault/Note.md").unwrap() {
            ShellOpen::Ask(t) => t,
            ShellOpen::Url(u) => panic!("not asked ({u})"),
        };
        t.recheck().expect("control: an untouched file passes the recheck");
        let was = std::fs::symlink_metadata(&note).unwrap();
        let mtime = was.modified().unwrap();
        // the dialog is open: rewrite in place, same size, mtime set back
        let t0 = std::time::Instant::now();
        loop {
            let f = std::fs::OpenOptions::new().write(true).open(&note).unwrap();
            std::io::Write::write_all(&mut &f, b"#!/bin/sh -x").unwrap(); // 12 bytes, as before
            f.set_modified(mtime).unwrap();
            drop(f);
            let now = std::fs::symlink_metadata(&note).unwrap();
            if (now.ctime(), now.ctime_nsec()) != (was.ctime(), was.ctime_nsec()) {
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "ctime never moved");
        }
        let now = std::fs::symlink_metadata(&note).unwrap();
        assert_eq!(std::fs::read(&note).unwrap(), b"#!/bin/sh -x", "the rewrite did not land");
        assert_eq!((now.dev(), now.ino(), now.mode(), now.size(), now.mtime(), now.mtime_nsec()), (was.dev(), was.ino(), was.mode(), was.size(), was.mtime(), was.mtime_nsec()), "the rewrite did not keep dev/ino/mode/size/mtime");
        let r = t.recheck();
        assert!(matches!(&r, Err(e) if e.starts_with("EPERM")), "a same-size rewrite with its mtime restored passed the recheck: {r:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /* R1 (PR #42, Greptile): a DIRECTORY target (showItemInFolder -> reveal opens the
       parent dir; a folder through path) changes size/mtime whenever an entry in it is
       created — saving a sibling note while the dialog is open. Still the same directory:
       opened. A directory replaced by another one (another inode) is still refused. The
       "dialog" hook runs the change to completion before the answer: no sleeps. */
    #[test]
    fn desk_r1_directory_target_opens_after_a_sibling_file_changes_during_the_dialog() {
        let base = std::env::temp_dir().join(format!("desk-r1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let vault = base.join("v");
        std::fs::create_dir_all(vault.join("sub")).unwrap();
        std::fs::write(vault.join("sub/a.md"), "a\n").unwrap();
        let real = vault.canonicalize().unwrap();
        let ask_then_run = |op: &str, target: &str, dialog: &dyn Fn()| -> Result<String, String> {
            let t = match shell_target(&vault, op, target)? {
                ShellOpen::Ask(t) => t,
                ShellOpen::Url(u) => panic!("{op} {target}: not asked ({u})"),
            };
            dialog();
            shell_spawn(&t, opener_path(None, &vault), |l| Ok(l.arg.to_string()))
        };
        // reveal(sub/a.md) opens sub/; a sibling is created, rewritten and renamed meanwhile
        let r = ask_then_run("reveal", "/vault/sub/a.md", &|| {
            std::fs::write(real.join("sub/new.md"), "x".repeat(5000)).unwrap();
            std::fs::rename(real.join("sub/new.md"), real.join("sub/moved.md")).unwrap();
            std::fs::write(real.join("sub/a.md"), "a changed\n").unwrap();
        });
        assert_eq!(r.as_deref(), Ok(&*real.join("sub").to_string_lossy()), "reveal: a dir whose entries changed was refused");
        // openPath of the vault folder itself, a file created at its top
        let r = ask_then_run("path", "/vault", &|| std::fs::write(real.join("Saved.md"), "s\n").unwrap());
        assert_eq!(r.as_deref(), Ok(&*real.to_string_lossy()), "path: a dir whose entries changed was refused");
        // still bound: a directory replaced by another one is refused
        let r = ask_then_run("path", "/vault/sub", &|| {
            std::fs::rename(real.join("sub"), real.join("old-sub")).unwrap();
            std::fs::create_dir(real.join("sub")).unwrap();
        });
        assert!(matches!(&r, Err(e) if e.starts_with("EPERM")), "a replaced directory was not refused: {r:?}");
        // and a FILE keeps the full identity (P4): rewritten in place -> refused
        std::fs::write(real.join("f.md"), "f\n").unwrap();
        let r = ask_then_run("path", "/vault/f.md", &|| std::fs::write(real.join("f.md"), "rewritten, longer\n").unwrap());
        assert!(matches!(&r, Err(e) if e.starts_with("EPERM")), "a rewritten file was not refused: {r:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /* R3 (PR #42, CodeAnt): a path with leading / trailing spaces is that path — not
       trimmed into another file (" a .md" must not open "a .md"), in shell targets
       (path / vault / reveal) and in a spawn's cwd. */
    #[test]
    fn desk_r3_a_path_with_leading_or_trailing_spaces_is_not_altered() {
        let base = std::env::temp_dir().join(format!("desk-r3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let vault = base.join("v");
        std::fs::create_dir_all(vault.join(" d ")).unwrap();
        std::fs::create_dir_all(vault.join("d")).unwrap();
        for n in [" a .md", "a .md", " d /x.md", "d/x.md"] {
            std::fs::write(vault.join(n), n).unwrap();
        }
        let real = vault.canonicalize().unwrap();
        let arg = |op: &str, t: &str| match shell_target(&vault, op, t) {
            Ok(ShellOpen::Ask(l)) => l.arg,
            Ok(ShellOpen::Url(u)) => panic!("{op} {t:?}: not asked ({u})"),
            Err(e) => format!("ERR {e}"),
        };
        let p = |n: &str| real.join(n).to_string_lossy().into_owned();
        assert_eq!(arg("path", "/vault/ a .md"), p(" a .md"), "path");
        assert_eq!(arg("path", " a .md"), p(" a .md"), "path relative");
        assert_eq!(arg("vault", " a .md"), p(" a .md"), "vault");
        assert_eq!(arg("vault", "/vault/ a .md"), p(" a .md"), "vault alias");
        assert_eq!(arg("reveal", "/vault/ d /x.md"), p(" d "), "reveal");
        assert_eq!(arg("path", "a .md"), p("a .md"), "control");
        // "a .md " (trailing space) does not exist: ENOENT, never "a .md"
        assert!(arg("path", "/vault/a .md ").starts_with("ERR ENOENT"), "a trailing space was trimmed");
        assert!(arg("vault", "a .md ").starts_with("ERR ENOENT"), "a trailing space was trimmed (vault)");
        // a spawn's cwd
        assert_eq!(map_cwd(&vault, Some(" d ")), vault.join(" d "));
        assert_eq!(map_cwd(&vault, Some("/vault/ d ")), vault.join(" d "));
        assert_eq!(map_cwd(&vault, Some(" ")), vault.join(" "));
        assert_eq!(map_arg(&vault, "/vault/ a .md"), vault.join(" a .md").to_string_lossy());
        let _ = std::fs::remove_dir_all(&base);
    }

    /* R4 (PR #42, Qodo): every child's own PATH lookups stay out of the vault. The
       opener (xdg-open, a script that finds gio / kde-open on PATH) runs in "/" with PATH
       cut to its absolute entries; a spawned child gets the same cleaning of the PATH it
       runs with (the plugin's, or ours). The vault holds a fake helper that marks when it
       runs; PATH puts "." and relative entries first. */
    #[test]
    fn desk_r4_opener_cwd_is_root_and_child_path_has_no_relative_entries() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("desk-r4-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // V1: kept entries come out canonical; every kept entry here already is
        let base = base.canonicalize().unwrap();
        let ok = base.join("okbin");
        std::fs::create_dir_all(&ok).unwrap();
        let ok = ok.display().to_string();
        let vault = base.join("v");
        std::fs::create_dir_all(vault.join("bin")).unwrap();
        let mark = base.join("HELPER-RAN");
        for h in ["r4helper", "bin/r4helper"] {
            std::fs::write(vault.join(h), format!("#!/bin/sh\ntouch '{}'\n", mark.display())).unwrap();
            std::fs::set_permissions(vault.join(h), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(vault.join("Welcome.md"), "# hi\n").unwrap();
        // the stub opener reports its cwd and PATH, then looks up a helper the way xdg-open does
        let log = base.join("opener.log");
        let opener = base.join("opener");
        std::fs::write(&opener, format!("#!/bin/sh\nprintf '%s|%s\\n' \"$(pwd -P)\" \"$PATH\" >> '{}'\nr4helper 2>/dev/null\nexit 0\n", log.display())).unwrap();
        std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(child_path(OsStr::new(&format!(".::bin:/usr/bin:./x:{ok}:")), &[vault.as_path()]), OsString::from(format!("/usr/bin:{ok}")));
        let dirty = format!(".::bin:/usr/bin:{ok}");

        // (a) the opener
        let t = match shell_target(&vault, "path", "/vault/Welcome.md").unwrap() {
            ShellOpen::Ask(t) => t,
            ShellOpen::Url(u) => panic!("not asked ({u})"),
        };
        let mut c = shell_spawn(&t, opener_path(Some(dirty.clone().into()), &vault), |l| {
            let mut cmd = Command::new(&opener);
            l.apply(&mut cmd).spawn().map_err(|e| e.to_string())
        })
        .unwrap();
        assert!(c.wait().unwrap().success());
        let got = std::fs::read_to_string(&log).unwrap();
        let (cwd, path) = got.trim_end().split_once('|').unwrap();
        assert_eq!(cwd, "/", "the opener's cwd is not /");
        assert!(path.split(':').all(|d| d.starts_with('/')), "the opener's PATH has an empty/relative entry: {path:?}");
        assert!(!mark.exists(), "the opener reached the vault's helper through PATH");

        // (b) a spawned child, cwd = the vault, the plugin's PATH with "." / "" / "bin" first
        let mut sp = spec("/bin/sh", &["-c", "printf %s \"$PATH\"; r4helper 2>/dev/null; exit 0"], &vault);
        sp.env.push(("PATH".into(), dirty));
        let r = start(ready(sp), &lifecycle()).unwrap();
        r.end();
        let (o, _, ch) = drain(&r);
        assert_eq!(ch.exit, Some(0));
        let path = bytes_to_latin1(&o);
        assert_eq!(path, format!("/usr/bin:{ok}"), "the child's PATH kept an empty/relative entry");
        assert!(!mark.exists(), "the child reached the vault's helper through PATH");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// a vault whose helper `name` (in the vault and in vault/bin, vault/tools) marks when it runs
    fn helper_vault(tag: &str, name: &str) -> (PathBuf, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("desk-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // V1: kept PATH entries come out canonical; dirs under this base already are
        let base = base.canonicalize().unwrap();
        let vault = base.join("v");
        let mark = base.join("HELPER-RAN");
        for d in ["", "bin", "tools", "sub"] {
            std::fs::create_dir_all(vault.join(d)).unwrap();
            let h = vault.join(d).join(name);
            std::fs::write(&h, format!("#!/bin/sh\ntouch '{}'\n", mark.display())).unwrap();
            std::fs::set_permissions(&h, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        (base, vault, mark)
    }

    /// the PATH a spawned child sees, after it tried to run `helper` by name
    fn child_sees(sp: Spec) -> String {
        let r = start(ready(sp), &lifecycle()).unwrap();
        r.end();
        let (o, _, ch) = drain(&r);
        assert_eq!(ch.exit, Some(0));
        bytes_to_latin1(&o)
    }

    /* S1 (PR #42 round 3, Greptile): a PATH with no absolute entry ("tools", ".", "") used to
       be cut to "" — and sh / execvp read an empty PATH as the cwd, the vault. The child now
       gets SAFE_PATH, a spawn and the opener alike, and the vault's helper is not reached. */
    #[test]
    fn desk_s1_a_path_without_absolute_entries_gives_the_safe_default_not_an_empty_path() {
        let (base, vault, mark) = helper_vault("s1", "s1helper");
        for p in ["tools", ".", "", "tools:.:bin"] {
            let mut sp = spec("/bin/sh", &["-c", "printf %s \"$PATH\"; s1helper 2>/dev/null; exit 0"], &vault);
            sp.env.push(("PATH".into(), p.into()));
            assert_eq!(child_sees(sp), SAFE_PATH, "plugin PATH={p:?}: the child's PATH is not the safe default");
            assert!(!mark.exists(), "plugin PATH={p:?}: the child reached the vault's helper");
        }
        let t = match shell_target(&vault, "path", "/vault/sub").unwrap() {
            ShellOpen::Ask(t) => t,
            ShellOpen::Url(u) => panic!("not asked ({u})"),
        };
        let got = shell_spawn(&t, opener_path(Some("tools:.".into()), &vault), |l| Ok(l.path.clone())).unwrap();
        assert_eq!(got, SAFE_PATH, "the opener's PATH is not the safe default");
        let _ = std::fs::remove_dir_all(&base);
    }

    /* S2 (PR #42 round 3, CodeAnt): the inherited PATH was read with std::env::var — a
       non-UTF-8 PATH gave None, nothing was set, and the child inherited the raw PATH
       (relative entries and all). It is now an OsString (var_os) filtered byte-wise, and
       PATH is always set on the child. prepare takes the inherited value as a parameter, so
       this needs no process-wide env change. */
    #[test]
    fn desk_s2_a_non_utf8_inherited_path_is_cleaned_not_passed_through_raw() {
        let (base, vault, _) = helper_vault("s2", "s2helper");
        // V1: an entry is kept only if it resolves, so the non-UTF-8 dir exists
        let odd = base.join(OsStr::from_bytes(b"s2\xffx"));
        std::fs::create_dir_all(&odd).unwrap();
        let odd = odd.into_os_string().into_vec();
        let inherited = OsString::from_vec([b"bin:".as_slice(), odd.as_slice(), b":.::/usr/bin".as_slice()].concat());
        assert!(inherited.to_str().is_none(), "the test PATH must not be UTF-8");
        let sp = prepare(spec("/bin/true", &[], &vault), Some(inherited));
        let c = build(&sp, sp.path.as_ref().unwrap());
        let got = c.get_envs().find(|(k, _)| *k == OsStr::new("PATH")).map(|(_, v)| v.map(|v| v.to_os_string()));
        assert_eq!(got, Some(Some(OsString::from_vec([odd.as_slice(), b":/usr/bin".as_slice()].concat()))), "the child's PATH was not set to the cleaned bytes");
        let _ = std::fs::remove_dir_all(&base);
    }

    /* S3 (PR #42 round 3, CodeAnt): absolute entries that still lead into the cwd or the
       vault — /proc/self/cwd, /proc/<pid>/cwd, /proc/self/fd/N, /dev/fd/N, a path under
       the vault (plain, with "..", or through a symlink outside it), a path under the
       child's cwd — are dropped; /usr/bin and /bin are kept, in order. */
    #[test]
    fn desk_s3_path_entries_leading_into_the_vault_or_cwd_are_dropped() {
        use std::os::unix::fs::symlink;
        let (base, vault, mark) = helper_vault("s3", "s3helper");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let first = base.join("first");
        std::fs::create_dir_all(&first).unwrap();
        let first = first.display().to_string();
        symlink(vault.join("bin"), base.join("link")).unwrap();
        let v = vault.display().to_string();
        let cwd = base.join("cwd");
        std::fs::create_dir_all(cwd.join("bin")).unwrap();
        let bad = [
            "/proc/self/cwd".to_string(),
            "/proc/self/cwd/bin".into(),
            format!("/proc/{}/cwd", std::process::id()),
            "/proc/self/fd/3".into(),
            "/dev/fd/3".into(),
            "//proc/self/cwd".into(),
            "/./dev/fd/4".into(),
            format!("{v}/bin"),
            format!("{v}/tools/"),
            v.clone(),
            format!("{}/../v/bin", base.join("elsewhere").display()),
            base.join("link").display().to_string(),
            cwd.join("bin").display().to_string(),
        ];
        let mut all = vec![first.clone()];
        all.extend(bad.iter().cloned());
        all.push(elsewhere.display().to_string());
        all.push("/usr/bin".into());
        let joined = all.join(":");
        let want = format!("{first}:{}:/usr/bin", elsewhere.display());
        assert_eq!(child_path(OsStr::new(&joined), &[vault.as_path(), cwd.as_path()]), OsString::from(&want));
        for b in &bad {
            assert_eq!(child_path(OsStr::new(&format!("{b}:/usr/bin")), &[vault.as_path(), cwd.as_path()]), "/usr/bin", "{b} was kept");
        }
        // a spawn: cwd in the vault, the plugin's PATH leads into it absolutely
        let mut sp = spec("/bin/sh", &["-c", "printf %s \"$PATH\"; s3helper 2>/dev/null; exit 0"], &vault.join("sub"));
        sp.root = vault.clone();
        sp.env.push(("PATH".into(), format!("{v}/bin:/proc/self/cwd:/dev/fd/3:/usr/bin:{first}")));
        assert_eq!(child_sees(sp), format!("/usr/bin:{first}"), "the child's PATH kept an entry into the vault");
        assert!(!mark.exists(), "the child reached the vault's helper through an absolute PATH entry");
        // the opener: the app's PATH with an entry into the vault
        let t = match shell_target(&vault, "path", "/vault/sub").unwrap() {
            ShellOpen::Ask(t) => t,
            ShellOpen::Url(u) => panic!("not asked ({u})"),
        };
        let got = shell_spawn(&t, opener_path(Some(format!("{v}/bin:/proc/self/cwd:/usr/bin").into()), &vault), |l| Ok(l.path.clone())).unwrap();
        assert_eq!(got, "/usr/bin", "the opener's PATH kept an entry into the vault");
        let _ = std::fs::remove_dir_all(&base);
    }

    /* V1 (PR #42 round 5, CodeAnt): a PATH entry outside the vault that is a symlink to
       /proc/self/cwd (directly, through a chain, or via /proc/thread-self/cwd) resolves to
       the APP's cwd here and to the CHILD's cwd — the vault — in the child: magic links are
       resolved per process. The child gets the entry's canonical form as resolved here, so
       it never follows the link itself: its PATH does not hold the link, and the vault's
       helper is not reached. A missing entry is dropped. */
    #[test]
    fn desk_v1_a_path_symlink_to_proc_self_cwd_does_not_reach_the_childs_cwd() {
        use std::os::unix::fs::symlink;
        let (base, vault, mark) = helper_vault("v1", "v1helper");
        let link = base.join("link");
        symlink("/proc/self/cwd", &link).unwrap();
        let chain = base.join("chain");
        symlink(&link, &chain).unwrap();
        let tlink = base.join("tlink");
        symlink("/proc/thread-self/cwd", &tlink).unwrap();
        let ours = std::env::current_dir().unwrap().canonicalize().unwrap().display().to_string();
        let gone = base.join("missing").display().to_string();
        let links = [link.display().to_string(), chain.display().to_string(), tlink.display().to_string()];
        for l in &links {
            // a spawn, cwd = the vault (where the vault's helper sits)
            let mut sp = spec("/bin/sh", &["-c", "printf %s \"$PATH\"; v1helper 2>/dev/null; exit 0"], &vault);
            sp.env.push(("PATH".into(), format!("{l}:{gone}:/usr/bin")));
            let path = child_sees(sp);
            assert!(!path.split(':').any(|d| d == l.as_str() || d == gone.as_str()), "{l}: the child's PATH holds the link (or a missing entry): {path:?}");
            assert_eq!(path, format!("{ours}:/usr/bin"), "{l}: the child did not get the canonical form resolved in the app");
            assert!(!mark.exists(), "{l}: the child reached the vault's helper through a link to /proc/self/cwd");
        }
        // the opener: the app's PATH with such a link
        let t = match shell_target(&vault, "path", "/vault/sub").unwrap() {
            ShellOpen::Ask(t) => t,
            ShellOpen::Url(u) => panic!("not asked ({u})"),
        };
        let got = shell_spawn(&t, opener_path(Some(format!("{}:/usr/bin", links[0]).into()), &vault), |l| Ok(l.path.clone())).unwrap();
        assert_eq!(got, OsString::from(format!("{ours}:/usr/bin")), "the opener's PATH holds the link");
        let _ = std::fs::remove_dir_all(&base);
    }
}
