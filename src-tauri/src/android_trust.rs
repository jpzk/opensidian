// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* #127 on android — THE VAULT FOLDER IDENTITY WHERE THERE IS NO BIRTH TIME.

   Desktop binds a trust entry to "<inode>:<birth time ns>" of the vault root
   (plugins::dir_identity): a folder removed and replaced at a trusted path is
   another folder and does not inherit trust. Android shared storage
   (/storage/emulated, the FUSE view every vault lives in) reports no birth
   time, so that identity is None there and every vault stayed restricted.

   This module is compiled only for android (and for its own tests): the
   desktop build does not see it, plugins::dir_identity on desktop is
   unchanged.

   The identity here is "android:<root inode>:<marker inode>:<nonce>":
     marker  <root>/.obsidian/opensidian-trust-id, a regular file holding 32
             random bytes (hex) from /dev/urandom, minted the first time the
             vault's key is computed (O_EXCL: never overwritten).
   Birth time on desktop separates a reused inode number; the marker does the
   same here: a folder created at the path later (rm -rf + mkdir, a clone, an
   unpacked archive, another vault moved there) has no marker, or another
   vault's marker, or a copy of the marker in a file with another inode under
   a root with another inode. Its identity differs from the one recorded with
   the flag in the app-PRIVATE config (appdirs: <app_config_dir>/opensidian.json,
   not readable or writable by other apps), so it opens restricted.

   FAIL CLOSED: anything unexpected is None (= never trusted, exactly what a
   root without birth time is on desktop): the root not a directory, .obsidian
   or the marker a symlink / not the expected type, a marker that cannot be
   created, read, or does not hold exactly 64 lowercase hex digits (a foreign
   or tampered marker is never "repaired" by overwriting it), no randomness,
   or the root swapped while the marker was handled. */
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;

/// the marker, relative to the vault root
pub const MARKER: &str = ".obsidian/opensidian-trust-id";
const NONCE_BYTES: usize = 32;

/// one app process mints and reads markers one at a time: a page's trust check and plugins_state
/// racing on a fresh vault never read a marker that is still being written
static LOCK: Mutex<()> = Mutex::new(());

/// the identity of the vault root `canon` (canonical path), minting its marker if it has none
pub fn identity(canon: &Path) -> Option<String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = fs::symlink_metadata(canon).ok()?;
    if !root.is_dir() {
        return None;
    }
    let (mino, nonce) = marker(canon)?;
    // the root handled is still the root stat'ed (not swapped meanwhile)
    let again = fs::symlink_metadata(canon).ok()?;
    if !again.is_dir() || again.ino() != root.ino() || again.dev() != root.dev() {
        return None;
    }
    Some(format!("android:{}:{mino}:{nonce}", root.ino()))
}

/// (marker inode, nonce) of the vault at `canon`; mints the marker when it is absent
fn marker(canon: &Path) -> Option<(u64, String)> {
    let dir = canon.join(".obsidian");
    match fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return None, // a symlink or a file named .obsidian
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&dir).ok()?,
        Err(_) => return None,
    }
    if !fs::symlink_metadata(&dir).ok()?.is_dir() {
        return None;
    }
    let path = canon.join(MARKER);
    match fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => mint(&path)?,
        Err(_) => return None,
    }
    read(&path)
}

/// a new marker: O_CREAT|O_EXCL (never follows or replaces anything at the path)
fn mint(path: &Path) -> Option<()> {
    let mut b = [0u8; NONCE_BYTES];
    fs::File::open("/dev/urandom").ok()?.read_exact(&mut b).ok()?;
    let mut f = fs::OpenOptions::new().write(true).create_new(true).open(path).ok()?;
    f.write_all(format!("{}\n", hex(&b)).as_bytes()).ok()?;
    f.sync_all().ok()
}

fn read(path: &Path) -> Option<(u64, String)> {
    let l = fs::symlink_metadata(path).ok()?;
    if !l.file_type().is_file() {
        return None;
    }
    let f = fs::File::open(path).ok()?;
    let m = f.metadata().ok()?;
    // the file opened is the regular file lstat'ed (not a symlink slipped in between)
    if !m.is_file() || m.ino() != l.ino() || m.dev() != l.dev() {
        return None;
    }
    let mut s = String::new();
    f.take(2 * NONCE_BYTES as u64 + 2).read_to_string(&mut s).ok()?;
    let n = s.strip_suffix('\n').unwrap_or(&s);
    (n.len() == 2 * NONCE_BYTES && n.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))).then(|| (m.ino(), n.to_string()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{bind_legacy, is_trusted_key, trust_bind, TrustKey, TRUST_ID_KEY, TRUST_KEY};
    use std::path::PathBuf;

    fn vault(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("opensidian-andtrust-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".obsidian/plugins/good")).unwrap();
        fs::write(root.join(".obsidian/community-plugins.json"), r#"["good"]"#).unwrap();
        fs::write(root.join("Home.md"), "home").unwrap();
        fs::canonicalize(&root).unwrap()
    }

    /// what plugins::trust_key computes on android
    fn key(r: &Path) -> TrustKey {
        TrustKey { path: r.display().to_string(), id: identity(r) }
    }

    /// what lib.rs plugins_set writes for a trust flip (as plugins.rs tests::flip)
    fn flip(cfg: &mut serde_json::Value, k: &TrustKey, on: bool) -> Result<(), String> {
        let bind = trust_bind(k, on)?;
        cfg[TRUST_KEY][&k.path] = serde_json::json!(on);
        if let Some(id) = bind {
            cfg[TRUST_ID_KEY][&k.path] = serde_json::json!(id);
        }
        Ok(())
    }

    fn trusted_on(r: &Path) -> (serde_json::Value, TrustKey) {
        let k = key(r);
        assert!(k.id.is_some(), "a vault folder on android has an identity");
        let mut cfg = serde_json::json!({});
        flip(&mut cfg, &k, true).unwrap();
        assert!(is_trusted_key(&cfg, &key(r)));
        (cfg, k)
    }

    #[test]
    fn an_unchanged_vault_stays_trusted() {
        let r = vault("same");
        let (cfg, k) = trusted_on(&r);
        let id = k.id.clone().unwrap();
        assert!(id.starts_with("android:"), "{id}");
        assert_eq!(id.split(':').count(), 4);
        let m = fs::read_to_string(r.join(MARKER)).unwrap();
        assert_eq!(m.len(), 65, "64 hex + newline");
        // recomputed keys agree; notes and other config changing changes nothing
        assert_eq!(identity(&r), k.id);
        fs::write(r.join("new.md"), "x").unwrap();
        fs::write(r.join(".obsidian/app.json"), "{}").unwrap();
        fs::remove_file(r.join("Home.md")).unwrap();
        assert_eq!(identity(&r), k.id);
        assert!(is_trusted_key(&cfg, &key(&r)));
        // the marker is never rewritten once minted
        assert_eq!(fs::read_to_string(r.join(MARKER)).unwrap(), m);
        let _ = fs::remove_dir_all(&r);
    }

    /* #127 case c on android: the folder at a trusted path removed and replaced */
    #[test]
    fn a_replaced_folder_at_the_same_path_is_untrusted() {
        let r = vault("replaced");
        let (cfg, k) = trusted_on(&r);
        // rm -rf + a fresh folder (a clone / an unpacked vault without a marker)
        fs::remove_dir_all(&r).unwrap();
        fs::create_dir_all(r.join(".obsidian/plugins/good")).unwrap();
        fs::write(r.join(".obsidian/community-plugins.json"), r#"["good"]"#).unwrap();
        assert_eq!(cfg[TRUST_KEY][&k.path].as_bool(), Some(true), "the path entry is still true");
        assert_ne!(identity(&r), k.id);
        assert!(!is_trusted_key(&cfg, &key(&r)), "a fresh folder inherited trust");
        // a copy of the old folder including the very marker bytes (cp -r of a backup)
        let (cfg2, k2) = trusted_on(&r);
        let marker = fs::read(r.join(MARKER)).unwrap();
        // the original moved aside (still on disk: its inodes are not free for reuse, as for a
        // replaced folder whose original was kept), the copy created at the path
        let aside = r.with_file_name(format!("{}-aside", r.file_name().unwrap().to_string_lossy()));
        let _ = fs::remove_dir_all(&aside);
        fs::rename(&r, &aside).unwrap();
        fs::create_dir_all(r.join(".obsidian")).unwrap();
        fs::write(r.join(MARKER), &marker).unwrap();
        assert_ne!(identity(&r), k2.id, "a copied marker in another folder is another identity");
        assert!(!is_trusted_key(&cfg2, &key(&r)));
        // another vault (with its own marker) moved onto the path
        let other = vault("replaced-other");
        assert!(identity(&other).is_some());
        fs::remove_dir_all(&r).unwrap();
        fs::rename(&other, &r).unwrap();
        assert!(!is_trusted_key(&cfg, &key(&r)));
        assert!(!is_trusted_key(&cfg2, &key(&r)));
        // trusting again (the asked flip) binds the folder now there
        let mut cfg3 = cfg.clone();
        flip(&mut cfg3, &key(&r), true).unwrap();
        assert!(is_trusted_key(&cfg3, &key(&r)));
        let _ = fs::remove_dir_all(&r);
        let _ = fs::remove_dir_all(&aside);
    }

    /* the identity record: missing marker / no identity source / a record that does not match */
    #[test]
    fn a_missing_marker_or_identity_source_is_untrusted() {
        // the marker deleted after the trust: a new one is minted, it is not the recorded one
        let r = vault("nomarker");
        let (cfg, k) = trusted_on(&r);
        fs::remove_file(r.join(MARKER)).unwrap();
        let now = key(&r);
        assert!(now.id.is_some() && now.id != k.id);
        assert!(!is_trusted_key(&cfg, &now));
        let _ = fs::remove_dir_all(&r);

        // no marker can be made (.obsidian is a file / a symlink out of the vault): no identity
        for how in ["file", "link"] {
            let r = vault(&format!("nosrc-{how}"));
            let (cfg, _) = trusted_on(&r);
            fs::remove_dir_all(r.join(".obsidian")).unwrap();
            if how == "file" {
                fs::write(r.join(".obsidian"), "x").unwrap();
            } else {
                let out = vault(&format!("nosrc-{how}-target"));
                std::os::unix::fs::symlink(out.join(".obsidian"), r.join(".obsidian")).unwrap();
            }
            let k = key(&r);
            assert_eq!(k.id, None, "{how}: no identity source");
            assert!(!is_trusted_key(&cfg, &k), "{how}");
            assert_eq!(trust_bind(&k, true), Err(crate::plugins::NO_IDENTITY.to_string()), "{how}: ON refused");
            let legacy = serde_json::json!({"plugins_trusted": {k.path.clone(): true}});
            assert!(!is_trusted_key(&legacy, &k), "{how}: a legacy entry is not honoured without an identity");
            assert!(bind_legacy(&legacy, &k).is_none());
            let _ = fs::remove_dir_all(&r);
            let _ = fs::remove_dir_all(vault(&format!("nosrc-{how}-target")));
        }

        // the root itself gone / not a directory
        let gone = std::env::temp_dir().join(format!("opensidian-andtrust-gone-{}", std::process::id()));
        assert_eq!(identity(&gone), None);
        let f = vault("notadir").join("Home.md");
        assert_eq!(identity(&f), None);
        let _ = fs::remove_dir_all(vault("notadir"));
    }

    #[test]
    fn a_tampered_marker_or_record_is_untrusted() {
        let r = vault("tamper");
        let (cfg, k) = trusted_on(&r);
        let good = fs::read_to_string(r.join(MARKER)).unwrap();
        // the marker bytes changed in place (same file, same inode): another nonce
        let mut flipped = good.clone().into_bytes();
        flipped[0] = if flipped[0] == b'a' { b'b' } else { b'a' };
        fs::OpenOptions::new().write(true).open(r.join(MARKER)).unwrap().write_all(&flipped).unwrap();
        assert!(!is_trusted_key(&cfg, &key(&r)));
        // garbage in the marker: no identity, and the marker is left as found (not re-minted)
        for bad in ["", "zz\n", &"A".repeat(64), &format!("{}x", &good[..64]), &format!("{good}{good}")] {
            fs::write(r.join(MARKER), bad).unwrap();
            assert_eq!(identity(&r), None, "{bad:?}");
            assert_eq!(fs::read_to_string(r.join(MARKER)).unwrap(), bad);
            assert!(!is_trusted_key(&cfg, &key(&r)));
        }
        // a marker that is a symlink (to another vault's marker) is no identity
        let other = vault("tamper-other");
        assert!(identity(&other).is_some());
        fs::remove_file(r.join(MARKER)).unwrap();
        std::os::unix::fs::symlink(other.join(MARKER), r.join(MARKER)).unwrap();
        assert_eq!(identity(&r), None);
        // the marker restored: trusted again (nothing else was part of the decision)
        fs::remove_file(r.join(MARKER)).unwrap();
        fs::write(r.join(MARKER), &good).unwrap();
        let restored = key(&r);
        assert!(restored.id.is_some());
        // (a rewritten file may get another inode: then it is another identity, fail closed)
        let same_inode = restored.id == k.id;
        assert_eq!(is_trusted_key(&cfg, &restored), same_inode);

        // the RECORD in the config tampered: wrong value, wrong type, another vault's identity
        let r2 = vault("tamper-rec");
        let (mut cfg, k) = trusted_on(&r2);
        for rec in [serde_json::json!("android:1:2:00"), serde_json::json!(true), serde_json::json!(identity(&other).unwrap()), serde_json::json!(k.id.clone().unwrap().to_uppercase())] {
            cfg[TRUST_ID_KEY][&k.path] = rec.clone();
            assert!(!is_trusted_key(&cfg, &key(&r2)), "{rec}");
        }
        // the flag tampered off / to a non-bool
        cfg[TRUST_ID_KEY][&k.path] = serde_json::json!(k.id.clone().unwrap());
        assert!(is_trusted_key(&cfg, &key(&r2)));
        cfg[TRUST_KEY][&k.path] = serde_json::json!("true");
        assert!(!is_trusted_key(&cfg, &key(&r2)));
        for d in [r, r2, other] {
            let _ = fs::remove_dir_all(d);
        }
    }

    /* #127 migration on android, as on desktop: an entry from before the binding (flag, no record)
       is bound once to the folder found (its marker minted then), after which a replacement is foreign */
    #[test]
    fn a_legacy_entry_is_bound_once_then_a_replacement_is_untrusted() {
        let r = vault("legacy");
        let mut cfg = serde_json::json!({"plugins_trusted": {r.display().to_string(): true}});
        assert!(!r.join(MARKER).exists());
        let k = key(&r);
        assert!(is_trusted_key(&cfg, &k));
        let (p, kk, v) = bind_legacy(&cfg, &k).expect("bound on the first boot");
        cfg[p][kk] = v;
        assert!(bind_legacy(&cfg, &key(&r)).is_none());
        assert!(is_trusted_key(&cfg, &key(&r)));
        fs::remove_dir_all(&r).unwrap();
        fs::create_dir_all(r.join(".obsidian")).unwrap();
        assert!(!is_trusted_key(&cfg, &key(&r)));
        let _ = fs::remove_dir_all(&r);
    }

    #[test]
    fn concurrent_first_keys_agree() {
        let r = vault("race");
        let ids: Vec<_> = (0..8).map(|_| std::thread::spawn({ let r = r.clone(); move || identity(&r) })).collect();
        let ids: Vec<_> = ids.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(ids[0].is_some());
        assert!(ids.iter().all(|i| i == &ids[0]), "{ids:?}");
        let _ = fs::remove_dir_all(&r);
    }
}
