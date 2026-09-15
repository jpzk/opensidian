// SPDX-License-Identifier: GPL-3.0-or-later
/* Landlock self-sandbox — ON by default, RUSTIDIAN_NO_LANDLOCK=1 disables. Applied once in main()
   BEFORE tauri spawns webkit, so every thread/child process inherits it:
   filesystem writes are confined to the vault, ~/.rustidian.json and the
   caches webkit/mesa/fontconfig need; the rest of the system is read-only
   and $HOME is not readable as a whole — dir listing only (for the picker),
   plus READ on the three R31.12 drop-source folders (see DROP_READ_DIRS),
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

/* R31.12 DROP SOURCES — the sandbox is why drag & drop is not "read the file".
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
    // R31.12: drop sources, read-only and enumerated — never $HOME itself
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

/* S3 — THE SWITCH AND THE RULESET, EACH IN ONE PLACE.
   The env var name was a string literal in TWO files (here and main.rs's
   startup guard), so "off" could have come to mean two different things. It is
   a const now, read through `no_landlock_requested()` by both. */
pub const NO_LANDLOCK_ENV: &str = "RUSTIDIAN_NO_LANDLOCK";

/// The documented off-switch (docs/features.md, req S3). Read it HERE, never
/// by spelling the variable's name a second time somewhere else.
pub fn no_landlock_requested() -> bool {
    std::env::var_os(NO_LANDLOCK_ENV).is_some()
}

/// The ruleset rustidian hands the kernel, AS DATA — the three vectors and the
/// access class each is granted. `enforce()` below builds its rules from THIS
/// value and nothing else, so a test that reads a plan reads what the kernel
/// would be told, on a kernel that can enforce it and on this one, which
/// cannot.
#[derive(Debug, PartialEq, Eq)]
pub struct RulesetPlan {
    /// `AccessFs::from_read` — readable, never writable
    pub read: Vec<PathBuf>,
    /// `AccessFs::from_all` — read AND write
    pub write: Vec<PathBuf>,
    /// `AccessFs::ReadDir` only — the picker lists these, it cannot read a file
    pub list_only: Vec<PathBuf>,
}

/// `None` = NO RULESET IS BUILT AT ALL, i.e. the process stays unsandboxed.
/// That is what `RUSTIDIAN_NO_LANDLOCK=1` buys, and it is the whole meaning of
/// the switch: not "a looser ruleset", but no ruleset.
///
/// `disabled` is a PARAMETER rather than an env read so both branches are
/// testable without mutating process-global env — cargo runs this binary's
/// tests as threads in ONE process, so a `set_var` here would race
/// `confines_reads_to_vault` (which calls `enforce`) and make the suite
/// flaky-green. The env read itself is `no_landlock_requested()`, one line.
pub fn ruleset_plan(disabled: bool, home: &Path, vault: &Path, cfg: &Path) -> Option<RulesetPlan> {
    if disabled {
        return None;
    }
    Some(RulesetPlan {
        read: read_roots(home),
        write: write_roots(home, vault, cfg),
        // picker may list dirs under $HOME, never read files there
        list_only: vec![home.to_path_buf()],
    })
}

pub fn enforce(vault: &Path, cfg: &Path) -> Result<RulesetStatus, Box<dyn std::error::Error>> {
    let off = no_landlock_requested();
    if off {
        return Ok(RulesetStatus::NotEnforced);
    }
    let vault = vault.canonicalize()?;
    let home = env_path("HOME").unwrap_or_else(|| PathBuf::from("/"));
    // config file must exist before the rule can point at it
    if !cfg.exists() {
        std::fs::write(cfg, "{}")?;
    }
    let abi = ABI::V1;
    // ONE source of truth: the vectors below are the tested ones, or the tests
    // are testing a ruleset the kernel never sees.
    let plan = match ruleset_plan(off, &home, &vault, cfg) {
        Some(p) => p,
        None => return Ok(RulesetStatus::NotEnforced),
    };
    for d in [home.join(".cache"), home.join(".local/share/dev.koto.rustidian")] {
        let _ = std::fs::create_dir_all(d);
    }
    let mut created = Ruleset::default()
        .set_compatibility(landlock::CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))?
        .create()?
        .add_rules(path_beneath_rules(&plan.read, AccessFs::from_read(abi)))?
        .add_rules(path_beneath_rules(&plan.write, AccessFs::from_all(abi)))?;
    for d in &plan.list_only {
        created = created.add_rule(PathBeneath::new(PathFd::new(d)?, AccessFs::ReadDir))?;
    }
    let status = created.restrict_self()?;
    if status.ruleset != RulesetStatus::NotEnforced {
        let _ = CONFINED.set(vault);
    }
    Ok(status.ruleset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /* R31.12 THE RULESET CONTENT IS THE ONLY TESTABLE PART HERE. This kernel
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

    /* S3 — THE RULESET PLAN. The three tests above are R31.12's (the drop
       sources). These four are S3 itself: the switch documented in
       docs/features.md, and the shape of the ruleset `enforce()` actually
       hands the kernel. They assert DATA, which is all that can be asserted on
       a kernel with no Landlock — see the module header and progress.md. */

    /// `RUSTIDIAN_NO_LANDLOCK=1` means NO RULESET, not a looser one. If this
    /// ever returns Some, the switch has quietly become a no-op and the app
    /// would sandbox itself in the very configuration documented as "off".
    #[test]
    fn ruleset_plan_is_none_when_the_no_landlock_switch_is_set() {
        let (home, vault, cfg) = (Path::new("/home/u"), Path::new("/home/u/vault"), Path::new("/home/u/.rustidian.json"));
        assert_eq!(ruleset_plan(true, home, vault, cfg), None, "the off-switch must build no ruleset at all");
        assert!(ruleset_plan(false, home, vault, cfg).is_some(), "default is ON: a ruleset must be built");
        // ...and the switch is that variable, spelled once (main.rs reads it
        // through no_landlock_requested(), features.md names the same string).
        assert_eq!(NO_LANDLOCK_ENV, "RUSTIDIAN_NO_LANDLOCK");
    }

    /// the plan IS the ruleset: what a test reads must be what `enforce()`
    /// hands the kernel, or these tests guard a vector nobody applies.
    #[test]
    fn ruleset_plan_is_built_from_the_same_vectors_enforce_applies() {
        let (home, vault, cfg) = (Path::new("/home/u"), Path::new("/home/u/vault"), Path::new("/home/u/.rustidian.json"));
        let p = ruleset_plan(false, home, vault, cfg).expect("switch is off");
        assert_eq!(p.read, read_roots(home), "plan.read must BE read_roots()");
        assert_eq!(p.write, write_roots(home, vault, cfg), "plan.write must BE write_roots()");
        // $HOME is ReadDir-only — listable for the picker, never readable
        assert_eq!(p.list_only, vec![home.to_path_buf()]);
        assert!(!p.read.contains(&home.to_path_buf()) && !p.write.contains(&home.to_path_buf()));
    }

    /// S3's actual invariant: WRITES are confined to the vault, its config and
    /// named scratch. Nothing writable may be an ancestor of $HOME or of the
    /// system — a widening here is the one that turns the sandbox into
    /// decoration, and it is the mutation in docs/negctl-lands control B.
    #[test]
    fn ruleset_plan_confines_writes_to_the_vault_cfg_and_named_scratch() {
        let (home, vault, cfg) = (Path::new("/home/u"), Path::new("/home/u/vault"), Path::new("/home/u/.rustidian.json"));
        let w = ruleset_plan(false, home, vault, cfg).expect("switch is off").write;
        assert!(w.contains(&vault.to_path_buf()) && w.contains(&cfg.to_path_buf()), "{w:?}");
        let allowed: Vec<PathBuf> = ["/tmp", "/dev", "/run", "/var/tmp"]
            .iter()
            .map(PathBuf::from)
            .chain([home.join(".cache"), home.join(".local/share/dev.koto.rustidian"), vault.to_path_buf(), cfg.to_path_buf()])
            .collect();
        assert_eq!(w, allowed, "the writable set grew or shrank — say so in features.md before changing it");
        // no writable root may CONTAIN the home dir, the vault's parent or /
        for wide in ["/", "/home", "/home/u", "/usr", "/etc", "/var", "/var/lib", "/opt"] {
            assert!(!w.contains(&PathBuf::from(wide)), "{wide} must not be writable: {w:?}");
        }
        for root in &w {
            assert!(!home.starts_with(root), "{root:?} is an ancestor of $HOME — every dotfile is writable through it");
        }
    }

    /// the read half may not overlap the write half's intent: a path granted
    /// read-only must not also appear in the writable set (that silently
    /// upgrades it to RW, since landlock unions the rules for a path).
    #[test]
    fn ruleset_plan_read_only_roots_are_not_also_writable() {
        let (home, vault, cfg) = (Path::new("/home/u"), Path::new("/home/u/vault"), Path::new("/home/u/.rustidian.json"));
        let p = ruleset_plan(false, home, vault, cfg).expect("switch is off");
        for r in &p.read {
            assert!(!p.write.contains(r), "{r:?} is in BOTH halves — read-only is a lie for it");
        }
        for d in DROP_READ_DIRS {
            assert!(p.read.contains(&home.join(d)) && !p.write.contains(&home.join(d)), "~/{d}");
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
