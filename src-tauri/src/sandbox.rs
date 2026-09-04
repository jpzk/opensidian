/* Landlock self-sandbox — hidden, OPT-IN: RUSTIDIAN_LANDLOCK=1. Applied once in main()
   BEFORE tauri spawns webkit, so every thread/child process inherits it:
   filesystem writes are confined to the vault, ~/.rustidian.json and the
   caches webkit/mesa/fontconfig need; the rest of the system is read-only
   and $HOME is not even readable (dir listing only, for the picker).
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

pub fn enforce(vault: &Path, cfg: &Path) -> Result<RulesetStatus, Box<dyn std::error::Error>> {
    if std::env::var_os("RUSTIDIAN_LANDLOCK").is_none() {
        return Ok(RulesetStatus::NotEnforced);
    }
    let vault = vault.canonicalize()?;
    let home = env_path("HOME").unwrap_or_else(|| PathBuf::from("/"));
    // config file must exist before the rule can point at it
    if !cfg.exists() {
        std::fs::write(cfg, "{}")?;
    }
    let abi = ABI::V1;
    let mut ro: Vec<PathBuf> = ["/usr", "/etc", "/lib", "/lib64", "/bin", "/sbin", "/opt", "/proc", "/sys", "/var/lib"]
        .iter()
        .map(PathBuf::from)
        .collect();
    ro.extend([home.join(".config"), home.join(".local/share"), home.join(".fonts"), home.join(".Xauthority")]);
    ro.extend(["APPIMAGE", "APPDIR", "XAUTHORITY"].iter().filter_map(|k| env_path(k)));
    if let Ok(exe) = std::env::current_exe() {
        ro.push(exe);
    }
    // webkit/mesa/fontconfig scratch + sockets (X11, wayland, dbus, shm, gpu)
    let mut rw: Vec<PathBuf> = ["/tmp", "/dev", "/run", "/var/tmp"].iter().map(PathBuf::from).collect();
    rw.extend([home.join(".cache"), home.join(".local/share/dev.koto.rustidian"), vault.clone(), cfg.to_path_buf()]);
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
            std::env::set_var("RUSTIDIAN_LANDLOCK", "1");
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
