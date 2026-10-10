// SPDX-License-Identifier: GPL-3.0-or-later
/* vaultbleed C — ~/.opensidian.json IS SHARED BY N PROCESSES NOW.
   One vault per process means two windows = two writers of the per-user config
   (recent vaults, last vault, theme, zoom, hotkeys, window geometry, ...). The
   old pattern, `v = read(); v[k] = x; fs::write(path, v)`, loses updates in
   both of the ways that matter:
     * read-modify-write race: A reads, B reads, A writes, B writes -> A's key
       (e.g. A's vault in the recent list) is gone;
     * torn file: fs::write truncates then writes, so a reader in between
       parses "" or half a JSON, falls back to {}, and its next write turns
       every key the user had into nothing.
   Fix, all of it here (main.rs only says WHICH key changes, as an Op):
     1. ONE exclusive flock(2) per update, on a lock file keyed by the canonical
        config path in vaultlock's lock dir (same dir as the vault locks, so it
        is writable under landlock and shared across flatpak instances). NOT a
        lock on the config file itself: step 3 replaces that inode, and a lock
        on a replaced inode excludes nobody.
     2. under the lock: re-read the file and apply the Ops to what is on disk
        NOW (merge), never to a copy read earlier. PushRecent merges into the
        current list, so two windows each adding a vault keep both.
     3. write <dir>/.<name>.tmp.<pid>, fsync, rename(2) over the config, fsync
        the dir. A reader sees the old file or the new one, never a torn one.
        A symlinked config (dotfile managers) is resolved first and the temp is
        made next to the TARGET, so the link survives and the rename stays on
        one filesystem.
     4. if the rename is refused (a landlock-confined process may write the
        config file but not create files in $HOME: EACCES/EPERM/EXDEV), fall
        back to an in-place write of the same merged bytes, still under the
        lock — no lost update, but not crash-atomic. Readers take the SHARED
        lock (read_value), so they never see that write half done.
   Ops are data (serde), not closures, on purpose: item D moves config writes of
   a confined process to its unconfined helper, which receives exactly these. */
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// MRU cap of the recent-vaults list
pub const RECENT_MAX: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// set a top-level key
    Set(String, Value),
    /// "last" = path, and path moved to the front of "list" (dedup, capped)
    PushRecent(String),
    /// winsize: v[key][subkey] gets the FIELDS of the object (it must be one);
    /// every other field of that record, every other subkey and every other key
    /// stay as they are on disk. Two windows on two vaults never drop each
    /// other's records, and keys we do not write (stock's devTools/zoom) survive.
    MergeIn(String, String, Value),
    /// treefold: set ONE key inside the object at top-level `parent` (created if
    /// absent or not an object); every other key in it round-trips. Two vault
    /// windows writing their own "<vaultId>-..." keys never clobber each other.
    SetIn(String, String, Value),
    /// treefold: like SetIn, but only if `parent.key` is absent — first writer
    /// wins (vault_ids: two windows minting an id for one fresh vault agree).
    InsertIn(String, String, Value),
}

/// MRU push: dedup, newest first, capped — pure for testability
pub fn push_recent(mut list: Vec<String>, path: &str) -> Vec<String> {
    list.retain(|x| x != path);
    list.insert(0, path.to_string());
    list.truncate(RECENT_MAX);
    list
}

pub fn apply(v: &mut Value, ops: &[Op]) {
    if !v.is_object() {
        *v = serde_json::json!({});
    }
    for op in ops {
        match op {
            Op::Set(k, x) => v[k.as_str()] = x.clone(),
            Op::PushRecent(p) => {
                let list = v["list"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                v["last"] = serde_json::json!(p);
                v["list"] = serde_json::json!(push_recent(list, p));
            }
            Op::MergeIn(k, sub, obj) => {
                if !v[k.as_str()].is_object() {
                    v[k.as_str()] = serde_json::json!({});
                }
                let rec = &mut v[k.as_str()][sub.as_str()];
                if !rec.is_object() {
                    *rec = serde_json::json!({});
                }
                if let (Some(dst), Some(src)) = (rec.as_object_mut(), obj.as_object()) {
                    for (f, x) in src {
                        dst.insert(f.clone(), x.clone());
                    }
                }
            }
            Op::SetIn(p, k, x) | Op::InsertIn(p, k, x) => {
                if !v[p.as_str()].is_object() {
                    v[p.as_str()] = serde_json::json!({});
                }
                let o = v[p.as_str()].as_object_mut().expect("object");
                if matches!(op, Op::SetIn(..)) {
                    o.insert(k.clone(), x.clone());
                } else if matches!(o.get(k.as_str()), None | Some(Value::Null)) {
                    // a null left by a hand edit counts as absent
                    o.insert(k.clone(), x.clone());
                }
            }
        }
    }
}

/// the file the bytes really live in (a symlinked config is followed)
fn target(cfg: &Path) -> PathBuf {
    fs::canonicalize(cfg).unwrap_or_else(|_| cfg.to_path_buf())
}

/// lock file for the config at `cfg`, inside `lock_dir`
pub fn lock_file(lock_dir: &Path, cfg: &Path) -> PathBuf {
    let t = target(cfg);
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in t.as_os_str().as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    lock_dir.join(format!("cfg-{h:016x}.lock"))
}

fn open_lock(lock_dir: &Path, cfg: &Path) -> std::io::Result<File> {
    fs::create_dir_all(lock_dir)?;
    OpenOptions::new().read(true).write(true).create(true).truncate(false).open(lock_file(lock_dir, cfg))
}

fn parse(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({}))
}

/// the whole config, read under the SHARED lock (never a half-written file).
/// No lock dir reachable -> unlocked read, as before.
pub fn read_value_in(lock_dir: &Path, cfg: &Path) -> Value {
    let _g = open_lock(lock_dir, cfg).ok().filter(|f| f.lock_shared().is_ok());
    fs::read_to_string(cfg).map(|s| parse(&s)).unwrap_or_else(|_| serde_json::json!({}))
}

/// How the bytes landed (tests and the log want to know which path ran).
#[derive(Debug, PartialEq)]
pub enum Wrote {
    Renamed,
    InPlace,
}

fn write_renamed(t: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = t.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = t.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tmp.{}", std::process::id()));
    let r = (|| {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, t)?;
        File::open(dir).and_then(|d| d.sync_all())
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    r
}

fn write_in_place(t: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).create(true).truncate(true).open(t)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// read-modify-write `cfg` under the exclusive lock in `lock_dir`
pub fn update_in(lock_dir: &Path, cfg: &Path, ops: &[Op]) -> Result<Wrote, String> {
    let lf = open_lock(lock_dir, cfg).map_err(|e| format!("cfg lock {}: {e}", lock_dir.display()))?;
    lf.lock().map_err(|e| format!("cfg lock: {e}"))?;
    let t = target(cfg);
    let mut v = fs::read_to_string(&t).map(|s| parse(&s)).unwrap_or_else(|_| serde_json::json!({}));
    apply(&mut v, ops);
    let bytes = v.to_string().into_bytes();
    match write_renamed(&t, &bytes) {
        Ok(()) => Ok(Wrote::Renamed),
        // EPERM, EACCES, EXDEV: the sandbox refused the create/rename, not the write
        Err(e) if matches!(e.raw_os_error(), Some(1 | 13 | 18)) => {
            write_in_place(&t, &bytes).map(|_| Wrote::InPlace).map_err(|e2| format!("{}: {e2} (rename: {e})", t.display()))
        }
        Err(e) => Err(format!("{}: {e}", t.display())),
    } // lf dropped here -> unlock, after the bytes are on disk
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cfgst-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn cfgstore_update_merges_into_what_is_on_disk_and_keeps_other_keys() {
        let d = tmp("merge");
        let (cfg, locks) = (d.join(".opensidian.json"), d.join("locks"));
        fs::write(&cfg, r#"{"theme":"light","list":["/a"],"hotkeys":{"x":[]}}"#).unwrap();
        assert_eq!(update_in(&locks, &cfg, &[Op::PushRecent("/b".into())]), Ok(Wrote::Renamed));
        // another process changes the file between our updates: we must not clobber it
        let mut v = parse(&fs::read_to_string(&cfg).unwrap());
        v["zoom"] = serde_json::json!(1.0);
        fs::write(&cfg, v.to_string()).unwrap();
        update_in(&locks, &cfg, &[Op::Set("sidebar_w".into(), serde_json::json!(250))]).unwrap();
        let v = read_value_in(&locks, &cfg);
        assert_eq!(v["list"], serde_json::json!(["/b", "/a"]));
        assert_eq!(v["last"], "/b");
        assert_eq!((v["theme"].as_str(), v["zoom"].as_f64(), v["sidebar_w"].as_u64()), (Some("light"), Some(1.0), Some(250)));
        assert!(v["hotkeys"].is_object(), "{v}");
        let strays: Vec<_> = fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name()).filter(|n| n.to_string_lossy().contains(".tmp.")).collect();
        assert!(strays.is_empty(), "temp left behind: {strays:?}");
    }

    #[test]
    fn cfgstore_merge_in_keeps_unknown_fields_other_records_and_other_keys() {
        let d = tmp("mergein");
        let (cfg, locks) = (d.join(".opensidian.json"), d.join("locks"));
        fs::write(&cfg, r#"{"theme":"light","windows":{"/a":{"x":1,"y":2,"width":3,"height":4,"isMaximized":false,"devTools":true},"/b":{"x":9}}}"#).unwrap();
        let rec = serde_json::json!({"x":10,"y":20,"width":800,"height":600,"isMaximized":true});
        update_in(&locks, &cfg, &[Op::MergeIn("windows".into(), "/a".into(), rec)]).unwrap();
        let v = read_value_in(&locks, &cfg);
        assert_eq!(v["windows"]["/a"], serde_json::json!({"x":10,"y":20,"width":800,"height":600,"isMaximized":true,"devTools":true}), "{v}");
        assert_eq!(v["windows"]["/b"], serde_json::json!({"x":9}), "{v}");
        assert_eq!(v["theme"], "light");
        // a partial record (Wayland: no position) leaves the old x/y alone
        update_in(&locks, &cfg, &[Op::MergeIn("windows".into(), "/a".into(), serde_json::json!({"width":500}))]).unwrap();
        let v = read_value_in(&locks, &cfg);
        assert_eq!((v["windows"]["/a"]["x"].as_i64(), v["windows"]["/a"]["width"].as_i64()), (Some(10), Some(500)), "{v}");
    }

    #[test]
    fn cfgstore_merge_in_creates_key_and_record_and_replaces_non_objects() {
        let mut v = serde_json::json!({"windows":"garbage"});
        apply(&mut v, &[Op::MergeIn("windows".into(), "/a".into(), serde_json::json!({"width":5}))]);
        assert_eq!(v, serde_json::json!({"windows":{"/a":{"width":5}}}));
        let mut v = serde_json::json!({"windows":{"/a":7}});
        apply(&mut v, &[Op::MergeIn("windows".into(), "/a".into(), serde_json::json!({"width":5}))]);
        assert_eq!(v["windows"]["/a"], serde_json::json!({"width":5}));
        let mut v = serde_json::json!({});
        apply(&mut v, &[Op::MergeIn("windows".into(), "/b".into(), serde_json::json!({"x":1}))]);
        assert_eq!(v, serde_json::json!({"windows":{"/b":{"x":1}}}));
    }

    #[test]
    fn cfgstore_symlinked_config_stays_a_symlink() {
        let d = tmp("link");
        fs::create_dir_all(d.join("dotfiles")).unwrap();
        let real = d.join("dotfiles/opensidian.json");
        fs::write(&real, r#"{"theme":"dark"}"#).unwrap();
        let cfg = d.join(".opensidian.json");
        symlink(&real, &cfg).unwrap();
        update_in(&d.join("locks"), &cfg, &[Op::Set("zoom".into(), serde_json::json!(0.5))]).unwrap();
        assert!(fs::symlink_metadata(&cfg).unwrap().file_type().is_symlink(), "rename replaced the dotfile link");
        let v = parse(&fs::read_to_string(&real).unwrap());
        assert_eq!((v["theme"].as_str(), v["zoom"].as_f64()), (Some("dark"), Some(0.5)));
    }

    /// THE requirement: N concurrent writers, each adding its own recent vault
    /// and its own key, lose nothing. Threads use their own open file
    /// descriptions, so flock contends between them exactly as between
    /// processes; a real second process is exercised below.
    #[test]
    fn cfgstore_concurrent_writers_lose_no_entry() {
        let d = tmp("conc");
        let (cfg, locks) = (d.join(".opensidian.json"), d.join("locks"));
        fs::write(&cfg, r#"{"theme":"light"}"#).unwrap();
        const N: usize = RECENT_MAX; // every vault must still fit in the MRU
        const ROUNDS: usize = 25;
        let hs: Vec<_> = (0..N)
            .map(|i| {
                let (cfg, locks) = (cfg.clone(), locks.clone());
                std::thread::spawn(move || {
                    for r in 0..ROUNDS {
                        update_in(&locks, &cfg, &[Op::PushRecent(format!("/v{i}")), Op::Set(format!("k{i}"), serde_json::json!(r))]).unwrap();
                        // a concurrent READER must never see a torn file
                        let v = read_value_in(&locks, &cfg);
                        assert_eq!(v["theme"], "light", "reader saw a torn/empty config: {v}");
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        let v = read_value_in(&locks, &cfg);
        let mut list: Vec<String> = serde_json::from_value(v["list"].clone()).unwrap();
        list.sort();
        let mut want: Vec<String> = (0..N).map(|i| format!("/v{i}")).collect();
        want.sort();
        assert_eq!(list, want, "lost recent-vault entries: {v}");
        for i in 0..N {
            assert_eq!(v[format!("k{i}").as_str()].as_u64(), Some(ROUNDS as u64 - 1), "lost update of k{i}: {v}");
        }
    }

    /// cross-PROCESS: an outside process holds the cfg lock; our update must
    /// wait for it (not write around it) and then merge onto ITS write.
    #[test]
    fn cfgstore_update_waits_for_another_process_holding_the_lock() {
        let d = tmp("proc");
        let (cfg, locks) = (d.join(".opensidian.json"), d.join("locks"));
        fs::write(&cfg, "{}").unwrap();
        fs::create_dir_all(&locks).unwrap();
        let lf = lock_file(&locks, &cfg);
        // holder: takes the lock on fd 9, writes a key while holding it, then releases
        let script = format!(
            "exec 9>>'{lf}'; flock -x 9; echo locked; sleep 0.6; printf '%s' '{{\"other\":1,\"list\":[\"/other\"]}}' > '{cfg}'; exit 0",
            lf = lf.display(),
            cfg = cfg.display()
        );
        let mut ch = std::process::Command::new("sh").arg("-c").arg(script).stdout(std::process::Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(ch.stdout.take().unwrap()), &mut line).unwrap();
        assert_eq!(line.trim(), "locked");
        let t0 = std::time::Instant::now();
        update_in(&locks, &cfg, &[Op::PushRecent("/mine".into())]).unwrap();
        assert!(t0.elapsed().as_millis() >= 300, "update did not wait for the holder ({:?})", t0.elapsed());
        ch.wait().unwrap();
        let v = read_value_in(&locks, &cfg);
        assert_eq!(v["list"], serde_json::json!(["/mine", "/other"]), "{v}");
        assert_eq!(v["other"], 1, "{v}");
    }

    /// treefold: SetIn merges one key into a sub-object; siblings, unknown keys
    /// and every top-level key round-trip, a non-object parent is replaced
    #[test]
    fn cfgstore_set_in_merges_one_key_and_keeps_the_rest() {
        let d = tmp("setin");
        let (cfg, locks) = (d.join(".opensidian.json"), d.join("locks"));
        fs::write(&cfg, r#"{"theme":"dark","localStorage":{"aaaa-file-explorer-unfold":["x"],"foreign":"keep"}}"#).unwrap();
        update_in(&locks, &cfg, &[Op::SetIn("localStorage".into(), "bbbb-bookmarks-folds".into(), serde_json::json!(["item-1"]))]).unwrap();
        update_in(&locks, &cfg, &[Op::SetIn("localStorage".into(), "aaaa-file-explorer-unfold".into(), serde_json::json!([]))]).unwrap();
        let v = read_value_in(&locks, &cfg);
        assert_eq!(v["theme"], "dark", "{v}");
        assert_eq!(v["localStorage"], serde_json::json!({"aaaa-file-explorer-unfold": [], "foreign": "keep", "bbbb-bookmarks-folds": ["item-1"]}), "{v}");
        let mut w = serde_json::json!({"localStorage": 7});
        apply(&mut w, &[Op::SetIn("localStorage".into(), "k".into(), serde_json::json!(1))]);
        assert_eq!(w, serde_json::json!({"localStorage": {"k": 1}}));
    }

    /// treefold: InsertIn is insert-if-absent — the first id minted for a vault
    /// wins, a later (racing) mint is a no-op; null counts as absent
    #[test]
    fn cfgstore_insert_in_first_writer_wins() {
        let mut v = serde_json::json!({"vault_ids": {"/v/b": null}});
        apply(&mut v, &[Op::InsertIn("vault_ids".into(), "/v/a".into(), serde_json::json!("1111"))]);
        apply(&mut v, &[Op::InsertIn("vault_ids".into(), "/v/a".into(), serde_json::json!("2222"))]);
        apply(&mut v, &[Op::InsertIn("vault_ids".into(), "/v/b".into(), serde_json::json!("3333"))]);
        assert_eq!(v["vault_ids"], serde_json::json!({"/v/a": "1111", "/v/b": "3333"}));
        // two "windows" racing through the real locked path on one file
        let d = tmp("insin");
        let (cfg, locks) = (d.join(".opensidian.json"), d.join("locks"));
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let (cfg, locks) = (cfg.clone(), locks.clone());
                std::thread::spawn(move || update_in(&locks, &cfg, &[Op::InsertIn("vault_ids".into(), "/v/c".into(), serde_json::json!(format!("id{i}")))]).unwrap())
            })
            .collect();
        hs.into_iter().for_each(|h| {
            h.join().unwrap();
        });
        let first = read_value_in(&locks, &cfg)["vault_ids"]["/v/c"].clone();
        assert!(first.as_str().is_some_and(|s| s.starts_with("id")), "{first}");
        update_in(&locks, &cfg, &[Op::InsertIn("vault_ids".into(), "/v/c".into(), serde_json::json!("late"))]).unwrap();
        assert_eq!(read_value_in(&locks, &cfg)["vault_ids"]["/v/c"], first);
    }
}
