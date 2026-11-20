// SPDX-License-Identifier: GPL-3.0-or-later
/* goal android — WHERE THINGS LIVE ON A PHONE. Desktop keeps $HOME
   (~/.opensidian.json, ~/.local/share/dev.koto.opensidian/locks): this module
   is compiled only for android (and for its own tests), so the desktop build
   does not even see it.

   An android app has no usable $HOME and may only write its private storage.
   Tauri hands us that storage in setup() as app_config_dir/app_data_dir
   (Context.getDataDir() underneath); run() stores it here ONCE, before the
   window exists, and every path the backend needs is derived from it:
     config  <app_config_dir>/opensidian.json
     locks   <app_data_dir>/locks
     vault   <app_data_dir>/vault   (seeded from the repo's vault/ on first run)
   Before setup() nothing is set: cfg reads see {} and nothing writes. */
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub const CFG_NAME: &str = "opensidian.json";

pub struct Dirs {
    pub config: PathBuf,
    pub data: PathBuf,
}

static DIRS: OnceLock<Dirs> = OnceLock::new();

/// set once, from setup(); false if it was already set
pub fn set(d: Dirs) -> bool {
    DIRS.set(d).is_ok()
}

pub fn get() -> Option<&'static Dirs> {
    DIRS.get()
}

/// the config file; empty before setup() (a read of "" is {}, see cfgstore)
pub fn cfg_path() -> PathBuf {
    get().map(|d| d.config.join(CFG_NAME)).unwrap_or_default()
}

pub fn lock_dir() -> PathBuf {
    get().map(|d| d.data.join("locks")).unwrap_or_default()
}

pub fn vault_dir(data: &Path) -> PathBuf {
    data.join("vault")
}

/// the repo's sample vault, embedded at build time (4 notes, ~4 KB)
pub const SAMPLE: &[(&str, &str)] = &[
    ("Welcome.md", include_str!("../../vault/Welcome.md")),
    ("Graph Theory.md", include_str!("../../vault/Graph Theory.md")),
    ("Ideas.md", include_str!("../../vault/Ideas.md")),
    ("sub/Nested.md", include_str!("../../vault/sub/Nested.md")),
];

/// First run: write SAMPLE into `dst`. Never touches an existing `dst` (the
/// user's vault after the first run). The files go into a sibling staging dir
/// that is renamed into place, so a crash mid-seed leaves no half vault that a
/// later run would mistake for the user's. Ok(true) = seeded now.
pub fn seed(dst: &Path) -> io::Result<bool> {
    if dst.exists() {
        return Ok(false);
    }
    let parent = dst.parent().ok_or_else(|| io::Error::other("vault dir has no parent"))?;
    fs::create_dir_all(parent)?;
    let stage = parent.join(".vault-seeding");
    let _ = fs::remove_dir_all(&stage); // leftovers of a crashed seed, never the vault
    for (rel, body) in SAMPLE {
        let p = stage.join(rel);
        if let Some(d) = p.parent() {
            fs::create_dir_all(d)?;
        }
        fs::write(&p, body)?;
    }
    fs::rename(&stage, dst)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("appdirs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn appdirs_seed_first_run_writes_sample() {
        let root = tmp("first");
        let v = vault_dir(&root);
        assert!(seed(&v).unwrap());
        for (rel, body) in SAMPLE {
            assert_eq!(fs::read_to_string(v.join(rel)).unwrap(), *body);
        }
        assert!(fs::read_to_string(v.join("Welcome.md")).unwrap().contains("[["), "welcome note links (graph non-empty)");
        assert!(!root.join(".vault-seeding").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn appdirs_seed_never_touches_existing_vault() {
        let root = tmp("again");
        let v = vault_dir(&root);
        fs::create_dir_all(&v).unwrap();
        fs::write(v.join("Welcome.md"), "mine").unwrap();
        assert!(!seed(&v).unwrap());
        assert_eq!(fs::read_to_string(v.join("Welcome.md")).unwrap(), "mine");
        assert!(!v.join("Ideas.md").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn appdirs_paths_before_setup_are_empty() {
        // DIRS is process-global and never set in tests: reads see "" -> {}
        assert_eq!(cfg_path(), PathBuf::new());
        assert_eq!(lock_dir(), PathBuf::new());
    }
}
