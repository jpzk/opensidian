// SPDX-License-Identifier: GPL-3.0-or-later
/* rename migration (goal opensidian, operator 2026-09-30: "change all names
   from rustidian to opensidian"). The product was called rustidian up to
   v0.15; a user's data must survive the upgrade. THE ONE TABLE, old -> new:

     ~/.rustidian.json                    -> ~/.opensidian.json
     <vault>/.rustidian-bookmarks         -> <vault>/.opensidian-bookmarks
     ~/.local/share/dev.koto.rustidian    -> ~/.local/share/dev.koto.opensidian

   ONE RULE for every row (`carry`): the NEW path wins whenever it exists; if
   it is absent and the OLD one exists, the old one is COPIED to the new path
   (bytes for a file, recursively for a directory) and the new one is used
   from then on. The old path is never written, moved or deleted — a user who
   downgrades still finds v0.15's files exactly as v0.15 left them.

   Env vars are a CLEAN rename (RUSTIDIAN_* is not read at all; every switch is
   OPENSIDIAN_* now) — they are a developer/test surface, not user data.

   Runs BEFORE the config is first read and before Landlock closes (main()),
   so neither the old file's location nor the write roots constrain it. The
   bookmark row runs per vault, lazily, from read_bm_tree (the vault is a
   write root, so it works under Landlock too). */
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const OLD_CFG: &str = ".rustidian.json";
pub const NEW_CFG: &str = ".opensidian.json";
pub const OLD_BM: &str = ".rustidian-bookmarks";
pub const NEW_BM: &str = ".opensidian-bookmarks";
pub const OLD_DATA: &str = ".local/share/dev.koto.rustidian";
pub const NEW_DATA: &str = ".local/share/dev.koto.opensidian";

/// what `carry` did — the unit tests and the console line read this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carry {
    /// the new path already existed: used as-is, the old one not even read
    NewWins,
    /// new absent, old present: old copied to new
    Copied,
    /// neither exists: nothing to do (fresh install)
    Neither,
}

/// THE resolution rule, for a file or a directory.
pub fn carry(old: &Path, new: &Path) -> io::Result<Carry> {
    if new.exists() {
        return Ok(Carry::NewWins);
    }
    if old.is_dir() {
        // copy into a sibling scratch dir, then rename: a crash mid-copy must
        // not leave a half-populated NEW dir that would "win" next start
        let part = new.with_extension(format!("part-{}", std::process::id()));
        let _ = fs::remove_dir_all(&part);
        copy_dir(old, &part)?;
        fs::rename(&part, new)?;
        return Ok(Carry::Copied);
    }
    if old.is_file() {
        if let Some(d) = new.parent() {
            fs::create_dir_all(d)?;
        }
        let part = new.with_extension(format!("part-{}", std::process::id()));
        fs::copy(old, &part)?;
        fs::rename(&part, new)?;
        return Ok(Carry::Copied);
    }
    Ok(Carry::Neither)
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)? {
        let e = e?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        let ft = e.file_type()?;
        if ft.is_dir() {
            copy_dir(&src, &dst)?;
        } else if ft.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(&src)?, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

fn report(what: &str, old: &Path, new: &Path, r: io::Result<Carry>) {
    match r {
        Ok(Carry::Copied) => eprintln!("migrate: {what} {} -> {}", old.display(), new.display()),
        Ok(_) => {}
        Err(e) => eprintln!("migrate: {what} {} -> {} FAILED ({e}); starting without it", old.display(), new.display()),
    }
}

/// the two per-user rows. Called once, first thing in main().
pub fn run_home(home: &Path) {
    let (o, n) = (home.join(OLD_CFG), home.join(NEW_CFG));
    report("config", &o, &n, carry(&o, &n));
    let (o, n) = (home.join(OLD_DATA), home.join(NEW_DATA));
    report("data", &o, &n, carry(&o, &n));
}

/// the per-vault row: returns the bookmark dotfile to read, if any.
pub fn vault_bookmarks(vault: &Path) -> Option<PathBuf> {
    let (o, n) = (vault.join(OLD_BM), vault.join(NEW_BM));
    report("bookmarks", &o, &n, carry(&o, &n));
    n.is_file().then_some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("opensidian-migrate-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn old_only_is_copied_and_left_untouched() {
        let h = tmp("oldonly");
        let old = br#"{"last":"/v","list":["/v"],"theme":"light","palette":{"accent":"#ff0000"}}"#;
        fs::write(h.join(OLD_CFG), old).unwrap();
        assert_eq!(carry(&h.join(OLD_CFG), &h.join(NEW_CFG)).unwrap(), Carry::Copied);
        assert_eq!(fs::read(h.join(NEW_CFG)).unwrap(), old.to_vec(), "new = old, byte for byte");
        assert_eq!(fs::read(h.join(OLD_CFG)).unwrap(), old.to_vec(), "old never written");
        // second start: the new file now exists and wins
        assert_eq!(carry(&h.join(OLD_CFG), &h.join(NEW_CFG)).unwrap(), Carry::NewWins);
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn both_present_new_wins_and_nothing_is_rewritten() {
        let h = tmp("both");
        fs::write(h.join(OLD_CFG), br#"{"theme":"light"}"#).unwrap();
        fs::write(h.join(NEW_CFG), br#"{"theme":"dark"}"#).unwrap();
        assert_eq!(carry(&h.join(OLD_CFG), &h.join(NEW_CFG)).unwrap(), Carry::NewWins);
        assert_eq!(fs::read(h.join(NEW_CFG)).unwrap(), br#"{"theme":"dark"}"#.to_vec());
        assert_eq!(fs::read(h.join(OLD_CFG)).unwrap(), br#"{"theme":"light"}"#.to_vec());
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn neither_present_is_a_fresh_install() {
        let h = tmp("neither");
        assert_eq!(carry(&h.join(OLD_CFG), &h.join(NEW_CFG)).unwrap(), Carry::Neither);
        assert!(!h.join(NEW_CFG).exists(), "nothing invented");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn run_home_copies_config_and_data_dir_recursively() {
        let h = tmp("home");
        fs::write(h.join(OLD_CFG), b"{}").unwrap();
        let od = h.join(OLD_DATA);
        fs::create_dir_all(od.join("WebKitCache/x")).unwrap();
        fs::write(od.join("WebKitCache/x/blob"), b"abc").unwrap();
        fs::write(od.join("top"), b"t").unwrap();
        run_home(&h);
        assert_eq!(fs::read(h.join(NEW_CFG)).unwrap(), b"{}".to_vec());
        let nd = h.join(NEW_DATA);
        assert_eq!(fs::read(nd.join("WebKitCache/x/blob")).unwrap(), b"abc".to_vec());
        assert_eq!(fs::read(nd.join("top")).unwrap(), b"t".to_vec());
        assert!(od.join("top").is_file(), "copy, not move");
        // no scratch dir left behind
        let left: Vec<_> = fs::read_dir(nd.parent().unwrap()).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 2, "old + new only: {left:?}");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn existing_new_data_dir_is_not_merged_into() {
        let h = tmp("datanew");
        fs::create_dir_all(h.join(OLD_DATA)).unwrap();
        fs::write(h.join(OLD_DATA).join("old"), b"o").unwrap();
        fs::create_dir_all(h.join(NEW_DATA)).unwrap();
        run_home(&h);
        assert!(!h.join(NEW_DATA).join("old").exists(), "new wins: nothing copied into it");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn vault_bookmarks_resolution() {
        let v = tmp("bm");
        assert_eq!(vault_bookmarks(&v), None, "neither: no dotfile");
        fs::write(v.join(OLD_BM), b"Ideas\n").unwrap();
        assert_eq!(vault_bookmarks(&v), Some(v.join(NEW_BM)));
        assert_eq!(fs::read(v.join(NEW_BM)).unwrap(), b"Ideas\n".to_vec());
        fs::write(v.join(NEW_BM), b"Other\n").unwrap();
        assert_eq!(vault_bookmarks(&v), Some(v.join(NEW_BM)));
        assert_eq!(fs::read(v.join(NEW_BM)).unwrap(), b"Other\n".to_vec(), "new wins");
        assert_eq!(fs::read(v.join(OLD_BM)).unwrap(), b"Ideas\n".to_vec(), "old untouched");
        let _ = fs::remove_dir_all(&v);
    }
}
