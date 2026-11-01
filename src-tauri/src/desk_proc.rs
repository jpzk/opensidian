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
use std::io::{Read, Write};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

/// the page-side name of the vault root (FileSystemAdapter.getBasePath)
pub const VAULT_ALIAS: &str = "/vault";

/// "/vault[/x]" -> root[/x]; relative or empty -> under root; other absolute -> as given
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
    let t = target.trim();
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
    pub env: Vec<(String, String)>,
    /// argv[0] the child sees (the name the plugin asked for); None = file
    pub argv0: Option<String>,
}

fn build(sp: &Spec) -> Command {
    let mut c = Command::new(&sp.file);
    if let Some(a0) = &sp.argv0 {
        c.arg0(a0);
    }
    c.args(&sp.args).current_dir(&sp.cwd).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).process_group(0);
    for (k, v) in &sp.env {
        c.env(k, v);
    }
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
    let file = sp.file.clone();
    let mut child = fork(sp).map_err(|e| spawn_err(&file, &e))?;
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
   script (by mime handler), an AppImage, a jar — and a custom scheme (steam:, file:, any
   x-scheme-handler an app registered) runs that handler. Telling a harmless local file
   from one that runs code is not decidable here (mime sniffing, user handlers, the exec
   bit, a symlink the plugin swaps after the check), so this does not try: EVERY local
   target (op path / vault / reveal, and op external with any scheme but http, https,
   mailto) is asked exactly like a spawn — program xdg-open, the target in full — and
   like every spawn it is asked each time (no answer is remembered between runs).
   http(s) and mailto URLs go to the browser / mail client unasked, as before. */
pub enum ShellOpen {
    /// an http(s)/mailto URL: opened without asking
    Url(String),
    /// anything else: the spawn consent dialog first, then xdg-open <this>
    Ask(String),
}

pub fn shell_target(root: &Path, op: &str, target: &str) -> Result<ShellOpen, String> {
    let local = |p: PathBuf| p.into_os_string().into_string().map(ShellOpen::Ask).map_err(|_| format!("EINVAL: not a UTF-8 path: {target}"));
    match op {
        "external" => {
            let scheme = target.split(':').next().unwrap_or("");
            let ok = target.contains(':') && scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
            if !ok {
                return Err(format!("EINVAL: not a URL: {target}"));
            }
            if matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "mailto") {
                Ok(ShellOpen::Url(target.to_string()))
            } else {
                Ok(ShellOpen::Ask(target.to_string()))
            }
        }
        "path" => local(map_cwd(root, Some(target))),
        // app.openWithDefaultApp: vault paths only, symlinks resolved and confined to the vault
        "vault" => local(vault_open_target(root, target)?),
        "reveal" => {
            let p = map_cwd(root, Some(target));
            local(p.parent().map(|d| d.to_path_buf()).unwrap_or(p))
        }
        _ => Err(format!("EINVAL: unknown shell op {op}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(file: &str, args: &[&str], cwd: &Path) -> Spec {
        Spec { file: file.into(), args: args.iter().map(|s| s.to_string()).collect(), cwd: cwd.into(), env: vec![], argv0: None }
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
        let r = start(sp, &lifecycle()).unwrap();
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
        let r = start(spec("/bin/sleep", &["30"], &std::env::temp_dir()), &lifecycle()).unwrap();
        assert!(r.kill(libc::SIGTERM));
        let (_, _, c) = drain(&r);
        assert_eq!((c.exit, c.signal), (None, Some("SIGTERM")));
        assert!(!r.kill(libc::SIGTERM));
        let e = start(spec("no-such-exe-desk", &[], &std::env::temp_dir()), &lifecycle()).err().unwrap();
        assert!(e.starts_with("ENOENT: spawn no-such-exe-desk"), "{e}");
        assert_eq!(sig_num("SIGKILL"), Some(libc::SIGKILL));
        assert_eq!(sig_num(""), Some(libc::SIGTERM));
    }

    #[test]
    fn desk_argv0_is_the_asked_name_while_the_absolute_path_is_execd() {
        let mut sp = spec("/bin/cat", &["/proc/self/cmdline"], &std::env::temp_dir());
        sp.argv0 = Some("cat-as-asked".into());
        let r = start(sp, &lifecycle()).unwrap();
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
            let r = start(spec("/bin/sleep", &["30"], &std::env::temp_dir()), &life).unwrap();
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
            let r = start(sp, &lifecycle()).unwrap();
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
                Ok(ShellOpen::Ask(a)) => assert!(!a.is_empty(), "{op} {t}"),
                Ok(ShellOpen::Url(u)) => panic!("{op} {t}: handed to xdg-open unasked as {u}"),
                Err(e) => panic!("{op} {t}: {e}"),
            }
        }
        match shell_target(&base, "path", "/vault/run.desktop") {
            Ok(ShellOpen::Ask(a)) => assert_eq!(a, base.join("run.desktop").to_string_lossy(), "the dialog names the real target in full"),
            _ => panic!("path not asked"),
        }
        for u in ["https://example.org/a?b=c", "http://127.0.0.1:8080/", "HTTPS://x.org", "mailto:a@b.org"] {
            assert!(matches!(shell_target(&base, "external", u), Ok(ShellOpen::Url(ref x)) if x == u), "{u}: a web/mail URL is asked or changed");
        }
        assert!(shell_target(&base, "external", "/etc/passwd").is_err(), "not a URL");
        assert!(shell_target(&base, "vault", "../x").is_err(), "vault escape");
        let _ = std::fs::remove_dir_all(&base);
    }
}
