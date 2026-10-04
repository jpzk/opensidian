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

/* LEGACY BUILT-IN THEMES (goal themes4, operator 2026-10-03). v0.1 seeded three
   in-house themes into every vault it opened; v0.2 ships four upstream themes
   instead (builtins.rs). THE DECISION, and why nothing in this file runs for it:

   - The old "palette" key in ~/.opensidian.json was already dead before this
     goal (main.rs, "THE PALETTE AXIS IS GONE"): unread, round-tripped, never
     applied. Nothing to migrate.
   - A vault's CHOICE lives in stock's own `.obsidian/appearance.json`
     (`cssTheme`). It is NOT rewritten: that file is shared with the stock app
     and we round-trip it byte-wise (themefs.rs), so an upgrade must not edit
     it behind the user's back.
   - The vault still has the old theme dir (every vault v0.1 opened does,
     because seeding wrote it and seeding never deletes): it stays an ordinary
     third-party theme — listed, chosen, painted, exactly as before.
   - The dir is gone: the name is unlisted, and the existing rule "an unlisted
     cssTheme paints the default theme" (main.rs vault_css_watch; ui/main.js) applies —
     i.e. it paints the default theme (AnuPpuccin) without writing anything, and reappears the
     moment the user drops the folder back in.
   So the mapping "old value -> Default unless the dir exists" is the scan's
   own predicate; the tests below pin each leg through the production paths
   (seed_builtin_themes, themes_scan, css_theme) so a later change to seeding
   or listing that broke an upgraded vault is a red unit test. */
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
        let old = br#"{"last":"/v","list":["/v"],"theme":"light","palette":"1984"}"#;
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

    // ---- legacy built-in themes (goal themes4) — see the header above ----

    /// names the v0.1 binary seeded; test INPUTS only (lint-themes B skips
    /// test modules): no production code knows these names any more.
    const LEGACY: [&str; 3] = ["1984", "Slate", "Wasp"];

    fn legacy_vault(tag: &str, with_dirs: bool) -> PathBuf {
        let v = tmp(tag);
        fs::create_dir_all(v.join(".obsidian")).unwrap();
        fs::write(v.join(".obsidian/appearance.json"), b"{\n  \"cssTheme\": \"Wasp\"\n}").unwrap();
        if with_dirs {
            for n in LEGACY {
                let d = v.join(".obsidian/themes").join(n);
                fs::create_dir_all(&d).unwrap();
                let m = format!("{{\"name\":\"{n}\",\"version\":\"0.1.0\",\"minAppVersion\":\"0.0.0\",\"author\":\"opensidian\"}}");
                fs::write(d.join("manifest.json"), m).unwrap();
                fs::write(d.join("theme.css"), "body.theme-dark { --background-primary: #010203; }\n").unwrap();
            }
        }
        v
    }

    /// Upgrade leg 1: the vault still holds the v0.1-seeded dirs. Seeding the
    /// v0.2 built-ins leaves every legacy byte AND mtime alone, the legacy
    /// themes stay LISTED, and the choice in appearance.json is untouched —
    /// so the user's `Wasp` keeps painting after the upgrade.
    #[test]
    fn legacy_theme_dirs_survive_the_upgrade_and_stay_chosen() {
        let v = legacy_vault("legacy-kept", true);
        let snap = |v: &Path| {
            LEGACY
                .iter()
                .flat_map(|n| ["manifest.json", "theme.css"].map(|f| v.join(".obsidian/themes").join(n).join(f)))
                .map(|p| (fs::read(&p).unwrap(), fs::metadata(&p).unwrap().modified().unwrap()))
                .collect::<Vec<_>>()
        };
        let before = snap(&v);
        let app_before = fs::read(v.join(".obsidian/appearance.json")).unwrap();
        let rep = crate::builtins::seed_builtin_themes(&v);
        assert!(rep.failed.is_empty(), "{rep:?}");
        assert_eq!(rep.wrote.len(), crate::builtins::BUILTIN_THEMES.len(), "the new four are added: {rep:?}");
        for n in LEGACY {
            assert!(!rep.wrote.iter().chain(&rep.kept).any(|w| w == n), "seeding must not even consider {n}");
        }
        assert_eq!(snap(&v), before, "a legacy theme's bytes or mtime changed");
        assert_eq!(fs::read(v.join(".obsidian/appearance.json")).unwrap(), app_before, "appearance.json rewritten");
        let s = crate::themefs::themes_scan(&v);
        for n in LEGACY {
            assert!(s.listed.iter().any(|l| l == n), "{n} must stay listed: {s:?}");
        }
        assert_eq!(s.listed.len(), 3 + crate::builtins::BUILTIN_THEMES.len(), "{s:?}");
        assert_eq!(crate::themefs::css_theme(&v), "Wasp", "the choice is kept");
        let _ = fs::remove_dir_all(&v);
    }

    /// Upgrade leg 2: the dir is gone. The choice is NOT rewritten (stock's
    /// file), and the name is not listed — which is exactly the condition
    /// under which the frontend paints the default theme (AnuPpuccin). Dropping the folder back in
    /// lists it again with no other step.
    #[test]
    fn a_legacy_choice_without_its_dir_resolves_to_default_without_a_write() {
        let v = legacy_vault("legacy-gone", false);
        let app_before = fs::read(v.join(".obsidian/appearance.json")).unwrap();
        crate::builtins::seed_builtin_themes(&v);
        let s = crate::themefs::themes_scan(&v);
        assert!(!s.listed.iter().any(|l| l == "Wasp"), "a theme with no dir cannot be listed: {s:?}");
        assert_eq!(s.listed.len(), crate::builtins::BUILTIN_THEMES.len(), "{s:?}");
        assert_eq!(crate::themefs::css_theme(&v), "Wasp", "the stored choice is left as the user/stock wrote it");
        assert_eq!(fs::read(v.join(".obsidian/appearance.json")).unwrap(), app_before, "appearance.json rewritten");
        // reversible: the folder comes back -> the choice is live again
        let d = v.join(".obsidian/themes/Wasp");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("manifest.json"), r#"{"name":"Wasp","version":"0.1.0","minAppVersion":"0.0.0","author":"x"}"#).unwrap();
        fs::write(d.join("theme.css"), "body.theme-dark { --background-primary: #010203; }\n").unwrap();
        assert!(crate::themefs::themes_scan(&v).listed.iter().any(|l| l == "Wasp"));
        let _ = fs::remove_dir_all(&v);
    }
}
