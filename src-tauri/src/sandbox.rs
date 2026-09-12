/* Landlock self-sandbox — ON by default, RUSTIDIAN_NO_LANDLOCK=1 disables. Applied once in main()
   BEFORE tauri spawns webkit, so every thread/child process inherits it:
   filesystem writes are confined to the vault, ~/.rustidian.json and the
   caches webkit/mesa/fontconfig need; the rest of the system is read-only
   and $HOME is not readable as a whole — dir listing only (for the picker),
   plus READ on the three R30.12 drop-source folders (see DROP_READ_DIRS),
   which is what makes drag & drop of an image possible at all.
   Best-effort: kernels without Landlock (< 5.13 / LSM disabled) run as
   before (stderr says why). Threads restrict only
   themselves, hence "once, at boot" — switching vaults needs a restart. */
use landlock::{
    path_beneath_rules, Access, AccessFs, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
    RulesetCreatedAttr, RulesetStatus, ABI,
};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// vault root the process is confined to (None = not enforced)
static CONFINED: OnceLock<PathBuf> = OnceLock::new();

pub fn confined_to() -> Option<&'static Path> {
    CONFINED.get().map(|p| p.as_path())
}

/// true when `p` is usable as a vault under the active sandbox
pub fn allows(p: &Path) -> bool {
    match confined_to() {
        None => true,
        Some(root) => p.canonicalize().map(|c| c.starts_with(root)).unwrap_or(false),
    }
}

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).map(PathBuf::from).filter(|p| p.exists())
}

/* R30.12 DROP SOURCES — the sandbox is why drag & drop is not "read the file".
   A drop hands us a path the user picked in a file manager; the landlock
   ruleset is immutable after restrict_self(), so a folder not named HERE can
   never be read later, and ~/Pictures/cat.png fails with EACCES before a
   single byte is copied. /tmp was always RW, which is exactly why a drop test
   whose source lives in /tmp proves nothing about the feature.

   DECISION (narrowest of the three options in the brief): grant READ on the
   three XDG dirs a user actually drags images out of, and nothing more.
   NOT all of $HOME: that hands ~/.ssh, ~/.gnupg, ~/.aws and every
   token-bearing dotfile to a webkit process, and "still better than stock
   Obsidian, which has no sandbox" is not a reason to leak keys.
   COST, stated plainly: a drop from anywhere else — ~/work/shots, ~/tmp, a
   second disk under /mnt — is REFUSED with a permission error, not copied.
   Paths under /media are not granted either; /run/media happens to be
   reachable because /run is already RW for the X11/dbus sockets.
   Widening further is an OPERATOR decision, one line away, deliberately not
   taken here. $HOME itself keeps ReadDir only (the picker lists, never reads). */
pub const DROP_READ_DIRS: [&str; 3] = ["Pictures", "Downloads", "Desktop"];

/// The read-only half of the ruleset, AS DATA. This kernel has no Landlock
/// (the enforce test below skips), so enforcement cannot be tested here and a
/// green suite proves nothing about it — what CAN be tested is the content of
/// this vector, which is what the kernel would be told. Paths that do not
/// exist are dropped by `path_beneath_rules` (landlock-0.4.7 fs.rs:617 returns
/// None on an unopenable path), so listing a missing ~/Desktop is harmless.
pub fn read_roots(home: &Path) -> Vec<PathBuf> {
    let mut ro: Vec<PathBuf> = ["/usr", "/etc", "/lib", "/lib64", "/bin", "/sbin", "/opt", "/proc", "/sys", "/var/lib"]
        .iter()
        .map(PathBuf::from)
        .collect();
    ro.extend([home.join(".config"), home.join(".local/share"), home.join(".fonts"), home.join(".Xauthority")]);
    // R30.12: drop sources, read-only and enumerated — never $HOME itself
    ro.extend(DROP_READ_DIRS.iter().map(|d| home.join(d)));
    ro.extend(["APPIMAGE", "APPDIR", "XAUTHORITY"].iter().filter_map(|k| env_path(k)));
    if let Ok(exe) = std::env::current_exe() {
        ro.push(exe);
    }
    ro
}

/// The read-write half, as data for the same reason. A drop source is NEVER in
/// here: rustidian copies out of those folders and never writes into them.
pub fn write_roots(home: &Path, vault: &Path, cfg: &Path) -> Vec<PathBuf> {
    // webkit/mesa/fontconfig scratch + sockets (X11, wayland, dbus, shm, gpu)
    let mut rw: Vec<PathBuf> = ["/tmp", "/dev", "/run", "/var/tmp"].iter().map(PathBuf::from).collect();
    rw.extend([
        home.join(".cache"),
        home.join(".local/share/dev.koto.rustidian"),
        vault.to_path_buf(),
        cfg.to_path_buf(),
    ]);
    rw
}

pub fn enforce(vault: &Path, cfg: &Path) -> Result<RulesetStatus, Box<dyn std::error::Error>> {
    if std::env::var_os("RUSTIDIAN_NO_LANDLOCK").is_some() {
        return Ok(RulesetStatus::NotEnforced);
    }
    let vault = vault.canonicalize()?;
    let home = env_path("HOME").unwrap_or_else(|| PathBuf::from("/"));
    // config file must exist before the rule can point at it
    if !cfg.exists() {
        std::fs::write(cfg, "{}")?;
    }
    let abi = ABI::V1;
    let ro = read_roots(&home);
    let rw = write_roots(&home, &vault, cfg);
    for d in [home.join(".cache"), home.join(".local/share/dev.koto.rustidian")] {
        let _ = std::fs::create_dir_all(d);
    }
    let status = Ruleset::default()
        .set_compatibility(landlock::CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))?
        .create()?
        .add_rules(path_beneath_rules(&ro, AccessFs::from_read(abi)))?
        .add_rules(path_beneath_rules(&rw, AccessFs::from_all(abi)))?
        // picker may list dirs under $HOME, never read files there
        .add_rule(PathBeneath::new(PathFd::new(&home)?, AccessFs::ReadDir))?
        .restrict_self()?;
    if status.ruleset != RulesetStatus::NotEnforced {
        let _ = CONFINED.set(vault);
    }
    Ok(status.ruleset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /* R30.12 THE RULESET CONTENT IS THE ONLY TESTABLE PART HERE. This kernel
       has no Landlock, so `confines_reads_to_vault` below SKIPS and a green
       suite says nothing about enforcement. These three tests assert what the
       kernel would be told instead — the vectors themselves — so the drop
       feature cannot be quietly un-shipped by deleting one line from
       read_roots(), and all of $HOME cannot be quietly granted either. */

    /// the drop source folders are READABLE, or drag & drop is dead on arrival
    /// for every real user (and only /tmp, already RW, would keep working).
    #[test]
    fn ro_vec_grants_read_on_the_drop_source_dirs() {
        let home = Path::new("/home/u");
        let ro = read_roots(home);
        for d in DROP_READ_DIRS {
            assert!(ro.contains(&home.join(d)), "read_roots is missing ~/{d}: {ro:?}");
        }
        assert_eq!(DROP_READ_DIRS.to_vec(), vec!["Pictures", "Downloads", "Desktop"]);
    }

    /// ...and NOTHING more: option (b) of the brief (read all of $HOME) is the
    /// one that must stay un-taken, because it hands ~/.ssh to webkit.
    #[test]
    fn ro_vec_does_not_widen_to_all_of_home() {
        let home = Path::new("/home/u");
        let (ro, rw) = (read_roots(home), write_roots(home, Path::new("/home/u/vault"), Path::new("/home/u/.rustidian.json")));
        assert!(!ro.contains(&home.to_path_buf()), "$HOME itself must never be readable: {ro:?}");
        // no granted root may be an ancestor of a secret-bearing dotfile
        for secret in [".ssh/id_ed25519", ".gnupg/secring.gpg", ".aws/credentials", ".netrc", ".bash_history", ".mozilla/firefox"] {
            let s = home.join(secret);
            for (label, v) in [("ro", &ro), ("rw", &rw)] {
                assert!(!v.iter().any(|r| s.starts_with(r)), "{label} grants ~/{secret} via one of {v:?}");
            }
        }
        // /home and / must not appear by accident either
        for wide in ["/", "/home", "/home/u", "/root", "/mnt", "/media"] {
            assert!(!ro.contains(&PathBuf::from(wide)), "ro must not contain {wide}");
            assert!(!rw.contains(&PathBuf::from(wide)), "rw must not contain {wide}");
        }
    }

    /// a drop READS its source and WRITES only into the vault: the source dirs
    /// must not be writable, and the vault + config must be.
    #[test]
    fn rw_vec_is_the_vault_not_the_drop_sources() {
        let home = Path::new("/home/u");
        let (vault, cfg) = (home.join("vault"), home.join(".rustidian.json"));
        let rw = write_roots(home, &vault, &cfg);
        assert!(rw.contains(&vault) && rw.contains(&cfg), "{rw:?}");
        for d in DROP_READ_DIRS {
            assert!(!rw.contains(&home.join(d)), "~/{d} must be READ-only: {rw:?}");
        }
    }

    /// restrict_self is per-thread: enforce + probe in a child thread, the
    /// unrestricted parent cleans up. Skips on kernels without Landlock.
    #[test]
    fn confines_reads_to_vault() {
        let home = env_path("HOME").expect("HOME");
        let tmp = std::env::temp_dir().join(format!("rustidian-ll-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("vault")).unwrap();
        let probe = home.join(format!(".rustidian-probe-{}", std::process::id()));
        fs::write(&probe, "secret").unwrap();
        let (vault, cfg, probe2) = (tmp.join("vault"), tmp.join("cfg.json"), probe.clone());
        let res = std::thread::spawn(move || {
            if enforce(&vault, &cfg).is_err() {
                return None; // kernel without landlock (e.g. this firecracker guest)
            }
            fs::write(vault.join("a.md"), "x").expect("vault writable");
            let read_ok = fs::read(&probe2).is_ok();
            let home_write_ok = fs::write(home.join(".rustidian-probe-w"), "x").is_ok();
            let etc_ok = fs::read_dir("/etc").is_ok();
            Some((read_ok, home_write_ok, etc_ok, allows(&vault), allows(&home)))
        })
        .join()
        .unwrap();
        let _ = fs::remove_file(&probe);
        let _ = fs::remove_dir_all(&tmp);
        match res {
            None => eprintln!("landlock unsupported here — test skipped"),
            Some(r) => assert_eq!(r, (false, false, true, true, false)),
        }
    }
}
