// SPDX-License-Identifier: GPL-3.0-or-later
/* vaultbleed A — ONE VAULT PER PROCESS. "Open another vault" never re-roots this
   process (the root is a OnceLock, main.rs bind_vault); it starts a NEW process
   of the same app on that vault: `opensidian <dir>`.

   "The same app" depends on how this one was launched — the three entrypoints
   opensidian ships as, decided by `launcher()` from facts the caller reads:
     * flatpak  (/.flatpak-info exists): `flatpak-spawn opensidian <dir>`.
       flatpak-spawn WITHOUT --host asks the org.freedesktop.portal.Flatpak
       Spawn portal for a NEW SANDBOX of the same app id. Every flatpak app may
       talk to that portal (no extra finish-arg, unlike --host which needs
       org.freedesktop.Flatpak = full host escape). A plain child would live in
       OUR sandbox's pid namespace and die with it when this window closes, which
       is exactly what "switch" does. The new sandbox has the same --filesystem
       grants, so <dir> means the same directory there.
     * AppImage ($APPIMAGE set by the AppImage runtime): exec the .AppImage FILE
       again. current_exe() would be <mount>/usr/bin/opensidian inside a squashfs
       mount that is unmounted when this process exits — a switch would pull the
       binary out from under the child.
     * plain binary: current_exe().
   The child loses VAULT_DIR (probes/tests set it; it beats argv in main(), so an
   inherited one would reopen THIS vault), gets stdin=null, and its own process
   group so a terminal ^C on one window does not take the other one down. */
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/* vaultbleed item 14 — THE SWITCH HANDSHAKE. "Switch vault" must feel like the
   same window changing vault (operator 16:00) while staying a new process:
     1. the old window flushes every buffer + the layout to disk (ui leaveVault,
        durable writes) BEFORE it asks for the switch;
     2. it spawns the new process with `--switch=x,y,w,h,max,token`: its own
        outer rect (logical px) and maximized state, plus a one-off token;
     3. the new process creates its window HIDDEN at that rect (main(): the
        window config is patched before the window exists — no frame at the
        default size), enters its vault, shows the window, waits for two
        painted frames, then appends `ready <token>` to ITS vault lock file
        (vaultlock::VaultLock::mark_ready — the fd it already holds, so no new
        path and nothing a sandbox has to allow);
     4. the old window polls that file (await_ready) and exits only on Ready —
        or on Timeout while the child is still alive (a slow child must not pin
        two windows forever). A child that DIED before Ready (refused lock,
        crash) leaves the old window running, with the error shown.
   The lock file is the channel because it already exists in every launch mode
   (plain / AppImage / flatpak's shared per-app runtime dir) and inside
   landlock's write roots — vaultlock.rs explains where it lives. */
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Switch {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    pub max: bool,
    pub token: String,
}

/// the argv flag that carries a Switch to the new process
pub const SWITCH_ARG: &str = "--switch=";

impl Switch {
    pub fn to_arg(&self) -> OsString {
        format!("{SWITCH_ARG}{},{},{},{},{},{}", self.x, self.y, self.w, self.h, u8::from(self.max), self.token).into()
    }

    /// parse + validate. Every field is bounded: this crosses a process
    /// boundary (and, confined, the spawner's trust boundary).
    pub fn parse(s: &str) -> Result<Switch, String> {
        let body = s.strip_prefix(SWITCH_ARG).ok_or("not a --switch= flag")?;
        let f: Vec<&str> = body.split(',').collect();
        let [x, y, w, h, m, t] = f[..] else { return Err(format!("--switch wants 6 fields, got {}", f.len())) };
        let sw = Switch {
            x: x.parse().map_err(|_| "--switch: bad x")?,
            y: y.parse().map_err(|_| "--switch: bad y")?,
            w: w.parse().map_err(|_| "--switch: bad w")?,
            h: h.parse().map_err(|_| "--switch: bad h")?,
            max: match m {
                "0" => false,
                "1" => true,
                _ => return Err("--switch: bad max".into()),
            },
            token: t.to_string(),
        };
        sw.check()?;
        Ok(sw)
    }

    pub fn check(&self) -> Result<(), String> {
        let span = -100_000..=100_000;
        if !span.contains(&self.x) || !span.contains(&self.y) {
            return Err("--switch: position out of range".into());
        }
        if !plausible_size(self.w as f64, self.h as f64) {
            return Err("--switch: size out of range".into());
        }
        if self.token.is_empty() || self.token.len() > 64 || !self.token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("--switch: bad token".into());
        }
        Ok(())
    }
}

/// a size a real window can have (the --switch bound; also how the old window
/// tells a cached 1x1 frame rect from a real one, main.rs switch_rect)
pub fn plausible_size(w: f64, h: f64) -> bool {
    (100.0..=20_000.0).contains(&w) && (100.0..=20_000.0).contains(&h)
}

/// a fresh token: our pid + the clock (unique per switch; the child's lock
/// file is truncated on acquire, so a stale `ready` line cannot match it)
pub fn new_token() -> String {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{n:x}", std::process::id())
}

/// split `--switch=` out of argv: (the Switch if one was valid, the rest).
/// A bad flag is reported and dropped — the window still opens, unswitched.
pub fn take_switch(args: Vec<OsString>) -> (Option<Switch>, Vec<OsString>, Option<String>) {
    let mut sw = None;
    let mut err = None;
    let mut rest = Vec::with_capacity(args.len());
    for a in args {
        match a.to_str().filter(|s| s.starts_with(SWITCH_ARG)) {
            Some(s) => match Switch::parse(s) {
                Ok(v) => sw = Some(v),
                Err(e) => err = Some(e),
            },
            None => rest.push(a),
        }
    }
    (sw, rest, err)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handoff {
    Ready,
    Died,
    Timeout,
}

/// the old window's wait. `ready`/`alive` are probes (lock file, /proc) so the
/// loop is testable with fakes. Ready wins over Died: a child that marked
/// ready and then exited still took the handoff.
pub fn await_ready(ready: impl Fn() -> bool, alive: impl Fn() -> bool, timeout: std::time::Duration, poll: std::time::Duration) -> Handoff {
    let t0 = std::time::Instant::now();
    loop {
        if ready() {
            return Handoff::Ready;
        }
        if !alive() {
            return if ready() { Handoff::Ready } else { Handoff::Died };
        }
        if t0.elapsed() >= timeout {
            return Handoff::Timeout;
        }
        std::thread::sleep(poll);
    }
}

/// is `pid` a live (non-zombie) process? Reads /proc/<pid>/stat — a read
/// root under landlock; signals (kill 0) may be scoped there.
pub fn alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        // "pid (comm) S ..." — comm may hold spaces/parens: state follows the LAST ')'
        Ok(s) => s.rfind(')').and_then(|i| s[i + 1..].trim_start().chars().next()).is_some_and(|c| c != 'Z' && c != 'X'),
        Err(_) => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher {
    Flatpak,
    AppImage(PathBuf),
    Plain(PathBuf),
}

/// which entrypoint started this process. Flatpak wins (an AppImage cannot run
/// inside a flatpak sandbox, but an inherited $APPIMAGE can leak into one).
pub fn launcher(in_flatpak: bool, appimage: Option<OsString>, exe: PathBuf) -> Launcher {
    if in_flatpak {
        return Launcher::Flatpak;
    }
    match appimage.filter(|a| !a.is_empty()) {
        Some(a) => Launcher::AppImage(PathBuf::from(a)),
        None => Launcher::Plain(exe),
    }
}

/// the flatpak command name: packaging/flatpak/dev.koto.opensidian.yml `command:`
pub const FLATPAK_COMMAND: &str = "opensidian";

/// (program, args) for "open `vault` in a new process" — pure, unit-tested.
/// A switch puts `--switch=...` BEFORE the vault (argv_pick takes the first
/// non-flag as the vault; flatpak-spawn passes everything after the command).
pub fn argv(l: &Launcher, vault: &Path, sw: Option<&Switch>) -> (OsString, Vec<OsString>) {
    let mut tail: Vec<OsString> = sw.map(Switch::to_arg).into_iter().collect();
    tail.push(vault.as_os_str().to_owned());
    match l {
        Launcher::Flatpak => ("flatpak-spawn".into(), std::iter::once(OsString::from(FLATPAK_COMMAND)).chain(tail).collect()),
        Launcher::AppImage(a) => (a.as_os_str().to_owned(), tail),
        Launcher::Plain(e) => (e.as_os_str().to_owned(), tail),
    }
}

/// the launcher of THIS process, read from the live environment.
pub fn current() -> Result<Launcher, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    Ok(launcher(Path::new("/.flatpak-info").exists(), std::env::var_os("APPIMAGE"), exe))
}

/// build the Command (not spawned) — split out so the env scrub is testable.
pub fn command(l: &Launcher, vault: &Path, sw: Option<&Switch>) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    let (prog, args) = argv(l, vault, sw);
    let mut c = std::process::Command::new(prog);
    c.args(args).env_remove("VAULT_DIR").stdin(std::process::Stdio::null()).process_group(0);
    c
}

/// spawn and reap in the background (no zombie while this window lives on).
/// Returns the child's pid.
pub fn open_in_new_process(vault: &Path, sw: Option<&Switch>) -> Result<u32, String> {
    let l = current()?;
    let mut child = command(&l, vault, sw).spawn().map_err(|e| format!("could not start a new window ({l:?}): {e}"))?;
    let pid = child.id();
    eprintln!("[vaultwin] spawned pid={pid} via {l:?} for {}{}", vault.display(), if sw.is_some() { " (switch)" } else { "" });
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn switch_rect_1x1_is_not_a_window() {
        // the cached frame rect tao can be left with (vaultbleed phase switch #5)
        assert!(!plausible_size(1.0, 1.0));
        assert!(plausible_size(1000.0, 760.0));
        let sw = Switch { x: 0, y: 0, w: 1, h: 1, max: false, token: "1-a".into() };
        assert!(sw.check().is_err(), "the old window must refuse to hand this to a child");
        assert!(Switch { w: 1000, h: 760, ..sw }.check().is_ok());
    }

    #[test]
    fn spawn_plain_binary_reexecs_current_exe_with_the_vault() {
        let l = launcher(false, None, "/opt/o/opensidian".into());
        assert_eq!(l, Launcher::Plain("/opt/o/opensidian".into()));
        assert_eq!(argv(&l, Path::new("/home/u/B"), None), ("/opt/o/opensidian".into(), os(&["/home/u/B"])));
        // an EMPTY $APPIMAGE is not an AppImage
        assert_eq!(launcher(false, Some("".into()), "/x".into()), Launcher::Plain("/x".into()));
    }

    #[test]
    fn spawn_appimage_reexecs_the_appimage_file_not_the_mounted_binary() {
        let l = launcher(false, Some("/home/u/Opensidian.AppImage".into()), "/tmp/.mount_Opx/usr/bin/opensidian".into());
        assert_eq!(l, Launcher::AppImage("/home/u/Opensidian.AppImage".into()));
        assert_eq!(argv(&l, Path::new("/home/u/B"), None), ("/home/u/Opensidian.AppImage".into(), os(&["/home/u/B"])));
    }

    #[test]
    fn spawn_flatpak_asks_the_portal_for_a_new_sandbox() {
        // flatpak wins over a leaked $APPIMAGE
        let l = launcher(true, Some("/x.AppImage".into()), "/app/bin/opensidian".into());
        assert_eq!(l, Launcher::Flatpak);
        let (p, a) = argv(&l, Path::new("/home/u/B"), None);
        assert_eq!(p, OsString::from("flatpak-spawn"));
        assert_eq!(a, os(&["opensidian", "/home/u/B"]), "no --host: the portal spawns a new sandbox of the same app");
        // the command name is the manifest's `command:`
        let m = include_str!("../../packaging/flatpak/dev.koto.opensidian.yml");
        assert!(m.lines().any(|l| l.trim() == format!("command: {FLATPAK_COMMAND}")), "manifest command drifted");
    }

    #[test]
    fn spawn_command_drops_vault_dir_and_detaches_stdin() {
        let c = command(&Launcher::Plain("/bin/true".into()), Path::new("/v/B"), None);
        assert_eq!(c.get_program(), "/bin/true");
        assert_eq!(c.get_args().collect::<Vec<_>>(), vec!["/v/B"]);
        assert!(
            c.get_envs().any(|(k, v)| k == "VAULT_DIR" && v.is_none()),
            "VAULT_DIR must be REMOVED in the child (it beats argv in main())"
        );
    }

    fn sw() -> Switch {
        Switch { x: -12, y: 34, w: 1100, h: 700, max: true, token: "4242-abc".into() }
    }

    /// item 14: a switch carries the old window's rect + maximized + token as ONE
    /// flag BEFORE the vault, in every launch mode; it round-trips exactly.
    #[test]
    fn switch_geometry_rides_argv_before_the_vault_in_every_launch_mode() {
        let s = sw();
        assert_eq!(s.to_arg(), OsString::from("--switch=-12,34,1100,700,1,4242-abc"));
        assert_eq!(Switch::parse(s.to_arg().to_str().unwrap()), Ok(s.clone()));
        let b = Path::new("/home/u/B");
        let plain = argv(&Launcher::Plain("/o".into()), b, Some(&s)).1;
        assert_eq!(plain, vec![s.to_arg(), "/home/u/B".into()]);
        assert_eq!(argv(&Launcher::AppImage("/a.AppImage".into()), b, Some(&s)).1, plain);
        assert_eq!(argv(&Launcher::Flatpak, b, Some(&s)).1, vec![OsString::from(FLATPAK_COMMAND), s.to_arg(), "/home/u/B".into()]);
        // the child splits it back out; the vault is still the first non-flag
        let (got, rest, err) = take_switch(plain);
        assert_eq!((got, rest, err), (Some(s), vec![OsString::from("/home/u/B")], None));
    }

    /// every field crossing the process boundary is bounded; a bad flag is
    /// dropped with a reason (the window opens unswitched), never obeyed.
    #[test]
    fn switch_flag_is_validated_not_trusted() {
        for bad in [
            "--switch=1,2,3",                         // field count
            "--switch=a,2,1100,700,0,t",              // not a number
            "--switch=0,0,50,700,0,t",                // too small
            "--switch=0,0,1100,700,2,t",              // max not 0/1
            "--switch=0,0,1100,700,0,",               // empty token
            "--switch=0,0,1100,700,0,../../etc",      // token charset
            "--switch=999999,0,1100,700,0,t",         // off any screen
        ] {
            assert!(Switch::parse(bad).is_err(), "{bad} accepted");
            let (s, rest, e) = take_switch(vec![bad.into(), "/v/B".into()]);
            assert!(s.is_none() && e.is_some() && rest == vec![OsString::from("/v/B")], "{bad}");
        }
        assert_eq!(take_switch(vec!["/v/B".into()]), (None, vec![OsString::from("/v/B")], None));
    }

    /// the old window's wait: Ready -> exit; Died -> stay; Timeout with a live
    /// child -> exit. Ready seen in the same poll as death still counts.
    #[test]
    fn switch_handoff_waits_for_ready_and_stays_if_the_child_dies() {
        use std::cell::Cell;
        use std::time::Duration;
        let ms = Duration::from_millis;
        let n = Cell::new(0);
        let r = await_ready(|| { n.set(n.get() + 1); n.get() > 3 }, || true, ms(2000), ms(1));
        assert_eq!(r, Handoff::Ready);
        assert_eq!(await_ready(|| false, || false, ms(2000), ms(1)), Handoff::Died);
        let t0 = std::time::Instant::now();
        assert_eq!(await_ready(|| false, || true, ms(60), ms(5)), Handoff::Timeout);
        assert!(t0.elapsed() >= ms(60));
        let k = Cell::new(0);
        assert_eq!(await_ready(|| { k.set(k.get() + 1); k.get() > 1 }, || false, ms(2000), ms(1)), Handoff::Ready);
    }

    /// /proc liveness: we are alive, a reaped child is not, a zombie is not.
    #[test]
    fn switch_alive_probe_reads_proc() {
        assert!(alive(std::process::id()));
        let mut c = std::process::Command::new("/bin/true").spawn().unwrap();
        let pid = c.id();
        while std::fs::read_to_string(format!("/proc/{pid}/stat")).map(|s| !s.contains(") Z")).unwrap_or(false) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!alive(pid), "zombie counted as alive");
        c.wait().unwrap();
        assert!(!alive(pid), "reaped pid counted as alive");
    }
}
