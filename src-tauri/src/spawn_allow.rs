// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* #74 "Always allow this program" (operator 2026-11-13, overriding PR #41 N2/N3 for the
   spawn prompt ONLY). The Git plugin starts ~13 programs at startup, each asked one by one;
   the operator decided that the user may answer a spawn question with "Always allow this
   program", and that the answer is remembered — narrowly:

   key   = (canonical vault path, plugin id, ABSOLUTE canonical program path — the file that
           is exec'd, PR #41 F1). Arguments, cwd and env are NOT part of the key (git's args
           vary on every call): a grant for /usr/bin/git lets that plugin run git with any
           arguments. The dialog says so.
   lapse = the grant also holds two fingerprints, and a spawn is pre-allowed only when ALL
           five values match what is on disk NOW:
             pluginSha256  = sha256 over the plugin's main.js and manifest.json (in that
                             order, each prefixed by its length as u64 little-endian),
                             read HERE from <vault>/.obsidian/plugins/<id>/ — never taken
                             from the webview (ADDENDUM 1): a new plugin version asks again;
             programSha256 = sha256 of the PINNED program (the open file the exec runs,
                             desk_proc::pin), cached by (dev, ino, size, mtime, ctime) once both
                             times are older than HASH_RACY_MARGIN (racy rule): a plugin
                             that rewrites a program in the vault cannot ride an old
                             grant, and an update of /usr/bin/git asks once more.
           A grant whose fingerprints no longer match stays listed ("outdated") until it is
           revoked or replaced by a new Always for the same key; it is never used.
   store = the per-user config ~/.opensidian.json, key "spawnAllow" — NEVER the vault or
           .obsidian (plugin-writable). Written only through the cfgstore Ops SpawnAllowAdd /
           SpawnAllowRevoke (flock + merge + atomic rename; a Landlock-confined window sends
           them to its unconfined helper like every config write, vaultbleed D). The generic
           Ops (Set / SetIn / MergeIn / InsertIn) refuse the key (cfgstore::apply).
   who   = an entry is CREATED in exactly one place: the `grant` closure desk_proc_spawn hands
           to consent::ask, which runs it only after the user pressed "Always allow this
           program" in the native dialog. No tauri command adds one: the webview may only
           list (spawn_allow_list) and revoke (spawn_allow_revoke).
   limit = the plugin id is reported by the page (all plugins share one JS realm), so a
           plugin of the same trusted vault that claims another plugin's id rides that
           plugin's grants. The fingerprint is of the CLAIMED plugin's files. Trust in a vault
           already means every enabled plugin runs with the user's rights. */
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// the config key the grants live under (~/.opensidian.json)
pub const KEY: &str = "spawnAllow";
/// the most grants kept; a new one past this drops the oldest
pub const MAX_GRANTS: usize = 256;
/// a main.js / manifest.json bigger than this is not fingerprinted (no Always offered)
pub const PLUGIN_FILE_MAX: u64 = 64 * 1024 * 1024;

/// one remembered "Always allow this program"
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub vault: String,
    pub plugin: String,
    pub plugin_sha256: String,
    pub program: String,
    pub program_sha256: String,
    /// unix seconds
    pub added: u64,
    /// manifest.json "version" at the time of the grant (shown in Settings)
    #[serde(default)]
    pub plugin_version: String,
}

/// what a spawn is, measured on the Rust side right before it is asked
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Want {
    pub vault: String,
    pub plugin: String,
    pub plugin_sha256: String,
    pub plugin_version: String,
    pub program: String,
    pub program_sha256: String,
}

impl Want {
    pub fn grant(&self, added: u64) -> Grant {
        Grant {
            vault: self.vault.clone(),
            plugin: self.plugin.clone(),
            plugin_sha256: self.plugin_sha256.clone(),
            program: self.program.clone(),
            program_sha256: self.program_sha256.clone(),
            added,
            plugin_version: self.plugin_version.clone(),
        }
    }
    fn same_key(&self, g: &Grant) -> bool {
        g.vault == self.vault && g.plugin == self.plugin && g.program == self.program
    }
}

/// every grant in a config Value (malformed entries are skipped)
pub fn grants(cfg: &serde_json::Value) -> Vec<Grant> {
    cfg[KEY].as_array().map(|a| a.iter().filter_map(|x| serde_json::from_value(x.clone()).ok()).collect()).unwrap_or_default()
}

/// a spawn is pre-allowed only when one grant matches ALL of key and both fingerprints
pub fn granted(cfg: &serde_json::Value, w: &Want) -> bool {
    grants(cfg).iter().any(|g| w.same_key(g) && g.plugin_sha256 == w.plugin_sha256 && g.program_sha256 == w.program_sha256)
}

/// cfgstore::apply for Op::SpawnAllowAdd: one entry per (vault, plugin, program) — a new
/// Always replaces the old (outdated) one; capped at MAX_GRANTS, oldest dropped first
pub fn apply_add(v: &mut serde_json::Value, g: &Grant) {
    let mut list: Vec<serde_json::Value> = v[KEY].as_array().cloned().unwrap_or_default();
    list.retain(|x| !(x["vault"] == g.vault.as_str() && x["plugin"] == g.plugin.as_str() && x["program"] == g.program.as_str()));
    list.push(serde_json::to_value(g).unwrap_or(serde_json::Value::Null));
    list.retain(|x| x.is_object());
    let over = list.len().saturating_sub(MAX_GRANTS);
    list.drain(..over);
    v[KEY] = serde_json::Value::Array(list);
}

/// cfgstore::apply for Op::SpawnAllowRevoke: drop the entry for (vault, plugin, program)
pub fn apply_revoke(v: &mut serde_json::Value, vault: &str, plugin: &str, program: &str) {
    if let Some(a) = v[KEY].as_array_mut() {
        a.retain(|x| !(x["vault"] == vault && x["plugin"] == plugin && x["program"] == program));
    }
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// sha256 over (len u64 LE, bytes) of each part, in order
pub fn fingerprint_parts(parts: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    hex(&h.finalize())
}

/// ADDENDUM 1: the plugin's fingerprint and manifest version, from the files ON DISK under the
/// confined plugin dir (plugins::plugin_file_in: no symlinked dir or file, no escape from the
/// vault). Err = not fingerprintable -> no Always is offered, the question is asked as before.
pub fn plugin_fingerprint(root: &Path, id: &str) -> Result<(String, String), String> {
    let read = |f: &str| -> Result<Vec<u8>, String> {
        // #250: rule 5 + the fd read: the bytes hashed are the bytes of the inode that was checked
        crate::plugins::plugin_read_in(root, id, f, PLUGIN_FILE_MAX).map_err(|e| format!("{id}/{f}: {e}"))?.ok_or_else(|| format!("{id}/{f}: missing"))
    };
    let (main, manifest) = (read("main.js")?, read("manifest.json")?);
    let version = serde_json::from_slice::<serde_json::Value>(&manifest).ok().and_then(|m| m["version"].as_str().map(String::from)).unwrap_or_default();
    Ok((fingerprint_parts(&[&main, &manifest]), version))
}

/// (dev, ino, size, mtime, ctime) -> sha256: a program is re-hashed only when its stat moved
type StatKey = (u64, u64, u64, (i64, i64), (i64, i64));
static HASHES: Mutex<Option<HashMap<StatKey, String>>> = Mutex::new(None);

/* #74 racy-hash rule (as git's "racily clean" index entries): a cached hash is only as good as
   the promise that any later change of the file moves its stat key. That holds only once the
   file's timestamps are older than the clock granularity that stamps the NEXT change: a
   same-size rewrite with mtime restored, in the same tick as the previous change, keeps
   (dev, ino, size, mtime, ctime) — Linux < 6.13 stamps ctime from the coarse clock (one jiffy,
   up to 10 ms), ext3/HFS+/most network fs store whole seconds, FAT/exFAT 2 s. So a hash is
   CACHED only when both mtime and ctime are older than now - HASH_RACY_MARGIN; a younger
   program is hashed and returned uncached, and the next call reads it again. 3 s = FAT's 2 s
   granularity + 1 s for filesystems that round up and for the few microseconds between the
   caller's stat and the clock read here. Timestamps in the future (clock skew) are never old
   enough. Limit: a network fs whose server clock lags ours by more than the margin can still
   alias; such a program is re-hashed whenever its key moves, as before. */
const HASH_RACY_MARGIN: std::time::Duration = std::time::Duration::from_secs(3);

/// sha256 of the bytes behind `f`, cached by `key` once the file is old enough (racy rule above)
pub fn program_sha256(key: StatKey, f: impl FnOnce() -> std::io::Result<std::fs::File>) -> Result<String, String> {
    program_sha256_at(key, std::time::SystemTime::now(), f)
}

/// program_sha256 with the clock read by the caller (`now` is taken before the file is read)
fn program_sha256_at(key: StatKey, now: std::time::SystemTime, f: impl FnOnce() -> std::io::Result<std::fs::File>) -> Result<String, String> {
    if let Some(h) = HASHES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get(&key) {
        return Ok(h.clone());
    }
    let mut file = f().map_err(|e| format!("program hash: {e}"))?;
    let mut h = Sha256::new();
    std::io::copy(&mut file, &mut h).map_err(|e| format!("program hash: {e}"))?;
    let d = hex(&h.finalize());
    if !stat_is_old(&key, now) {
        return Ok(d);
    }
    let mut m = HASHES.lock().unwrap_or_else(|e| e.into_inner());
    let m = m.get_or_insert_with(HashMap::new);
    if m.len() > 1024 {
        m.clear();
    }
    m.insert(key, d.clone());
    Ok(d)
}

/// both mtime and ctime of `key` are older than `now` - HASH_RACY_MARGIN
fn stat_is_old(key: &StatKey, now: std::time::SystemTime) -> bool {
    let ns = |(s, n): (i64, i64)| s as i128 * 1_000_000_000 + n as i128;
    let Ok(now) = now.duration_since(std::time::UNIX_EPOCH) else { return false };
    let limit = now.as_nanos() as i128 - HASH_RACY_MARGIN.as_nanos() as i128;
    ns(key.3) < limit && ns(key.4) < limit
}

/// a grant as Settings lists it: the entry plus whether its fingerprints still match disk
/// ("ok"), no longer match ("outdated": never used) or cannot be checked from here ("unknown")
pub fn status(g: &Grant) -> &'static str {
    let plugin = plugin_fingerprint(Path::new(&g.vault), &g.plugin).map(|(fp, _)| fp);
    let program = crate::desk_proc::pin(&g.program).and_then(|p| program_sha256(p.stat_key(), || p.open_read()));
    match (plugin, program) {
        (Ok(a), Ok(b)) if a == g.plugin_sha256 && b == g.program_sha256 => "ok",
        (Ok(a), _) if a != g.plugin_sha256 => "outdated",
        (_, Ok(b)) if b != g.program_sha256 => "outdated",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want(vault: &str, plugin: &str, program: &str) -> Want {
        Want { vault: vault.into(), plugin: plugin.into(), plugin_sha256: "p1".into(), plugin_version: "1.0".into(), program: program.into(), program_sha256: "x1".into() }
    }

    #[test]
    fn spawn_allow_fingerprint_is_length_prefixed_and_ordered() {
        // "ab"+"c" and "a"+"bc" must differ (length prefix), and the order matters
        assert_ne!(fingerprint_parts(&[b"ab", b"c"]), fingerprint_parts(&[b"a", b"bc"]));
        assert_ne!(fingerprint_parts(&[b"a", b"b"]), fingerprint_parts(&[b"b", b"a"]));
        assert_eq!(fingerprint_parts(&[b"a", b"b"]).len(), 64);
    }

    #[test]
    fn spawn_allow_other_vault_plugin_program_or_fingerprint_is_not_granted() {
        let w = want("/v", "obsidian-git", "/usr/bin/git");
        let mut cfg = serde_json::json!({});
        apply_add(&mut cfg, &w.grant(1));
        assert!(granted(&cfg, &w));
        for other in [
            want("/other", "obsidian-git", "/usr/bin/git"),
            want("/v", "other-plugin", "/usr/bin/git"),
            want("/v", "obsidian-git", "/usr/bin/ssh"),
            Want { plugin_sha256: "p2".into(), ..w.clone() },
            Want { program_sha256: "x2".into(), ..w.clone() },
        ] {
            assert!(!granted(&cfg, &other), "{other:?} rode a grant for {w:?}");
        }
    }

    #[test]
    fn spawn_allow_new_always_replaces_the_outdated_entry_and_revoke_drops_it() {
        let w = want("/v", "obsidian-git", "/usr/bin/git");
        let mut cfg = serde_json::json!({"theme":"dark"});
        apply_add(&mut cfg, &w.grant(1));
        let w2 = Want { plugin_sha256: "p2".into(), ..w.clone() };
        assert!(!granted(&cfg, &w2), "a new plugin version rode the old grant");
        assert_eq!(grants(&cfg).len(), 1, "the outdated grant stays listed");
        apply_add(&mut cfg, &w2.grant(2));
        assert_eq!(grants(&cfg).len(), 1, "{cfg}");
        assert!(granted(&cfg, &w2) && !granted(&cfg, &w));
        apply_add(&mut cfg, &want("/v", "obsidian-git", "/usr/bin/ssh").grant(3));
        apply_revoke(&mut cfg, "/v", "obsidian-git", "/usr/bin/git");
        assert!(!granted(&cfg, &w2));
        assert_eq!(grants(&cfg).len(), 1);
        assert_eq!(cfg["theme"], "dark");
    }

    #[test]
    fn spawn_allow_is_capped_oldest_first() {
        let mut cfg = serde_json::json!({});
        for i in 0..MAX_GRANTS + 3 {
            apply_add(&mut cfg, &want("/v", "p", &format!("/bin/p{i}")).grant(i as u64));
        }
        let g = grants(&cfg);
        assert_eq!(g.len(), MAX_GRANTS);
        assert_eq!(g[0].program, "/bin/p3");
    }

    #[cfg(unix)]
    #[test]
    fn spawn_allow_plugin_fingerprint_changes_with_main_js_or_manifest() {
        let base = std::env::temp_dir().join(format!("spawnallow-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join(".obsidian/plugins/obsidian-git");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.js"), "module.exports=1").unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"id":"obsidian-git","version":"2.0.0"}"#).unwrap();
        let (fp1, ver) = plugin_fingerprint(&base, "obsidian-git").unwrap();
        assert_eq!(ver, "2.0.0");
        std::fs::write(dir.join("main.js"), "module.exports=2").unwrap();
        let (fp2, _) = plugin_fingerprint(&base, "obsidian-git").unwrap();
        assert_ne!(fp1, fp2, "main.js change kept the fingerprint");
        std::fs::write(dir.join("manifest.json"), r#"{"id":"obsidian-git","version":"2.0.1"}"#).unwrap();
        let (fp3, ver3) = plugin_fingerprint(&base, "obsidian-git").unwrap();
        assert_ne!(fp2, fp3, "manifest.json change kept the fingerprint");
        assert_eq!(ver3, "2.0.1");
        // no manifest, a bad id, a symlinked main.js: not fingerprintable
        assert!(plugin_fingerprint(&base, "../x").is_err());
        assert!(plugin_fingerprint(&base, "absent").is_err());
        let ev = base.join(".obsidian/plugins/evil");
        std::fs::create_dir_all(&ev).unwrap();
        std::fs::write(base.join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(base.join("secret"), ev.join("main.js")).unwrap();
        std::fs::write(ev.join("manifest.json"), "{}").unwrap();
        assert!(plugin_fingerprint(&base, "evil").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }

    /* #74: the program hash is of the pinned file's bytes: a rewrite in place (same size,
       mtime set back) or a replace by rename gives another hash, so a grant for the old
       program no longer matches and the spawn is asked again; unchanged = same hash (cached). */
    #[cfg(unix)]
    #[test]
    fn spawn_allow_program_hash_change_asks_again() {
        // canonical: pin() takes the canonical path (macOS: $TMPDIR is under /var -> /private/var)
        let d = std::env::temp_dir().canonicalize().unwrap().join(format!("spawnallow-prog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let prog = d.join("prog");
        crate::testutil::write_exe(&prog, "#!/bin/sh\necho A\n").unwrap();
        let p = prog.to_str().unwrap().to_string();
        let hash = || {
            let pin = crate::desk_proc::pin(&p).unwrap();
            program_sha256(pin.stat_key(), || pin.open_read()).unwrap()
        };
        let h1 = hash();
        assert_eq!(h1, fingerprint_hex_of(b"#!/bin/sh\necho A\n"));
        assert_eq!(hash(), h1, "an unchanged program changed its hash");
        let w1 = Want { program: p.clone(), program_sha256: h1.clone(), ..want("/v", "obsidian-git", "") };
        let mut cfg = serde_json::json!({});
        apply_add(&mut cfg, &w1.grant(1));
        // same size, mtime restored: ctime still moves -> re-hashed
        let mtime = std::fs::metadata(&prog).unwrap().modified().unwrap();
        std::fs::write(&prog, "#!/bin/sh\necho B\n").unwrap();
        std::fs::File::options().write(true).open(&prog).unwrap().set_modified(mtime).unwrap();
        let h2 = hash();
        assert_ne!(h1, h2, "an in-place rewrite kept the hash");
        assert!(!granted(&cfg, &Want { program_sha256: h2, ..w1.clone() }), "a rewritten program rode the grant");
        // replaced by rename (new inode)
        let tmp = d.join("prog.new");
        crate::testutil::write_exe(&tmp, "#!/bin/sh\necho C\n").unwrap();
        std::fs::rename(&tmp, &prog).unwrap();
        let h3 = hash();
        assert_eq!(h3, fingerprint_hex_of(b"#!/bin/sh\necho C\n"));
        assert!(!granted(&cfg, &Want { program_sha256: h3, ..w1.clone() }), "a replaced program rode the grant");
        let _ = std::fs::remove_dir_all(&d);
    }

    /* #74 racy rule, without kernel tick timing: the key of a fresh program is reused AS IS
       for the rewrite — the worst case of a coarse clock (same-size rewrite, mtime restored,
       ctime in the same tick). A program hashed right after its change is not cached, so the
       rewrite is read again and gives another hash. */
    #[test]
    #[cfg(unix)] // a #!/bin/sh program with no extension is no program on Windows
    fn spawn_allow_racily_new_program_is_not_cached() {
        let d = std::env::temp_dir().canonicalize().unwrap().join(format!("spawnallow-racy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let prog = d.join("prog");
        crate::testutil::write_exe(&prog, "#!/bin/sh\necho A\n").unwrap();
        let pin = crate::desk_proc::pin(prog.to_str().unwrap()).unwrap();
        let key = pin.stat_key();
        // hashed in the very tick of its last change: now = max(mtime, ctime)
        let t = |(s, n): (i64, i64)| std::time::UNIX_EPOCH + std::time::Duration::new(s as u64, n as u32);
        let at = t(key.3).max(t(key.4));
        let h1 = program_sha256_at(key, at, || pin.open_read()).unwrap();
        assert_eq!(h1, fingerprint_hex_of(b"#!/bin/sh\necho A\n"));
        // exactly at the margin: still too new, so nothing was cached and the next call reads
        assert_eq!(program_sha256_at(key, at + HASH_RACY_MARGIN, || pin.open_read()).unwrap(), h1);
        assert!(program_sha256_at(key, at, || Err(std::io::Error::other("re-read"))).is_err(), "a racily-new program was cached");
        // same size, mtime restored, SAME key: the rewrite must be read, not served from cache
        let mtime = std::fs::metadata(&prog).unwrap().modified().unwrap();
        std::fs::write(&prog, "#!/bin/sh\necho B\n").unwrap();
        std::fs::File::options().write(true).open(&prog).unwrap().set_modified(mtime).unwrap();
        let h2 = program_sha256_at(key, at, || std::fs::File::open(&prog)).unwrap();
        assert_eq!(h2, fingerprint_hex_of(b"#!/bin/sh\necho B\n"), "a racily-new rewrite kept the old hash");
        let _ = std::fs::remove_dir_all(&d);
    }

    /* #74: the cache still does its job: an unchanged program whose mtime and ctime are older
       than the margin is hashed once and then served from the cache (no re-read). */
    #[test]
    #[cfg(unix)] // a #!/bin/sh program with no extension is no program on Windows
    fn spawn_allow_old_unchanged_program_is_cached() {
        let d = std::env::temp_dir().canonicalize().unwrap().join(format!("spawnallow-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let prog = d.join("prog");
        crate::testutil::write_exe(&prog, "#!/bin/sh\necho old\n").unwrap();
        let pin = crate::desk_proc::pin(prog.to_str().unwrap()).unwrap();
        let key = pin.stat_key();
        // hashed a margin and a second after its last change (the clock seam: no sleeping)
        let t = |(s, n): (i64, i64)| std::time::UNIX_EPOCH + std::time::Duration::new(s as u64, n as u32);
        let later = t(key.3).max(t(key.4)) + HASH_RACY_MARGIN + std::time::Duration::from_secs(1);
        let h1 = program_sha256_at(key, later, || pin.open_read()).unwrap();
        assert_eq!(h1, fingerprint_hex_of(b"#!/bin/sh\necho old\n"));
        let h2 = program_sha256_at(key, later, || Err(std::io::Error::other("re-read"))).expect("an old unchanged program was re-read");
        assert_eq!(h2, h1);
        // the public path serves it from the cache too
        assert_eq!(program_sha256(key, || Err(std::io::Error::other("re-read"))).unwrap(), h1);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn fingerprint_hex_of(b: &[u8]) -> String {
        hex(&Sha256::digest(b))
    }
}
