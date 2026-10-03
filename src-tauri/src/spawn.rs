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
pub fn argv(l: &Launcher, vault: &Path) -> (OsString, Vec<OsString>) {
    match l {
        Launcher::Flatpak => ("flatpak-spawn".into(), vec![FLATPAK_COMMAND.into(), vault.as_os_str().to_owned()]),
        Launcher::AppImage(a) => (a.as_os_str().to_owned(), vec![vault.as_os_str().to_owned()]),
        Launcher::Plain(e) => (e.as_os_str().to_owned(), vec![vault.as_os_str().to_owned()]),
    }
}

/// the launcher of THIS process, read from the live environment.
pub fn current() -> Result<Launcher, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    Ok(launcher(Path::new("/.flatpak-info").exists(), std::env::var_os("APPIMAGE"), exe))
}

/// build the Command (not spawned) — split out so the env scrub is testable.
pub fn command(l: &Launcher, vault: &Path) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    let (prog, args) = argv(l, vault);
    let mut c = std::process::Command::new(prog);
    c.args(args).env_remove("VAULT_DIR").stdin(std::process::Stdio::null()).process_group(0);
    c
}

/// spawn and reap in the background (no zombie while this window lives on).
/// Returns the child's pid.
pub fn open_in_new_process(vault: &Path) -> Result<u32, String> {
    let l = current()?;
    let mut child = command(&l, vault).spawn().map_err(|e| format!("could not start a new window ({l:?}): {e}"))?;
    let pid = child.id();
    eprintln!("[vaultwin] spawned pid={pid} via {l:?} for {}", vault.display());
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
    fn spawn_plain_binary_reexecs_current_exe_with_the_vault() {
        let l = launcher(false, None, "/opt/o/opensidian".into());
        assert_eq!(l, Launcher::Plain("/opt/o/opensidian".into()));
        assert_eq!(argv(&l, Path::new("/home/u/B")), ("/opt/o/opensidian".into(), os(&["/home/u/B"])));
        // an EMPTY $APPIMAGE is not an AppImage
        assert_eq!(launcher(false, Some("".into()), "/x".into()), Launcher::Plain("/x".into()));
    }

    #[test]
    fn spawn_appimage_reexecs_the_appimage_file_not_the_mounted_binary() {
        let l = launcher(false, Some("/home/u/Opensidian.AppImage".into()), "/tmp/.mount_Opx/usr/bin/opensidian".into());
        assert_eq!(l, Launcher::AppImage("/home/u/Opensidian.AppImage".into()));
        assert_eq!(argv(&l, Path::new("/home/u/B")), ("/home/u/Opensidian.AppImage".into(), os(&["/home/u/B"])));
    }

    #[test]
    fn spawn_flatpak_asks_the_portal_for_a_new_sandbox() {
        // flatpak wins over a leaked $APPIMAGE
        let l = launcher(true, Some("/x.AppImage".into()), "/app/bin/opensidian".into());
        assert_eq!(l, Launcher::Flatpak);
        let (p, a) = argv(&l, Path::new("/home/u/B"));
        assert_eq!(p, OsString::from("flatpak-spawn"));
        assert_eq!(a, os(&["opensidian", "/home/u/B"]), "no --host: the portal spawns a new sandbox of the same app");
        // the command name is the manifest's `command:`
        let m = include_str!("../../packaging/flatpak/dev.koto.opensidian.yml");
        assert!(m.lines().any(|l| l.trim() == format!("command: {FLATPAK_COMMAND}")), "manifest command drifted");
    }

    #[test]
    fn spawn_command_drops_vault_dir_and_detaches_stdin() {
        let c = command(&Launcher::Plain("/bin/true".into()), Path::new("/v/B"));
        assert_eq!(c.get_program(), "/bin/true");
        assert_eq!(c.get_args().collect::<Vec<_>>(), vec!["/v/B"]);
        assert!(
            c.get_envs().any(|(k, v)| k == "VAULT_DIR" && v.is_none()),
            "VAULT_DIR must be REMOVED in the child (it beats argv in main())"
        );
    }
}
