// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Invariant tests for the history store (temp dirs, std only).

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

/// Temp dir removed on drop.
struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let p = std::env::temp_dir().join(format!(
            "osy-hist-{tag}-{}-{}-{nanos}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// app_data and vault as siblings under one temp dir.
struct Fx {
    _t: Tmp,
    app: PathBuf,
    vault: PathBuf,
}

fn fx(tag: &str) -> Fx {
    let t = Tmp::new(tag);
    let app = t.0.join("appdata");
    let vault = t.0.join("vault");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&vault).unwrap();
    Fx { _t: t, app, vault }
}

fn open(f: &Fx) -> Store {
    Store::open(&f.app, "v1", &f.vault, Config::default()).unwrap()
}

fn write(f: &Fx, p: &str, b: &[u8]) {
    let dst = f.vault.join(p);
    fs::create_dir_all(dst.parent().unwrap()).unwrap();
    fs::write(dst, b).unwrap();
}

fn stored(o: Outcome) -> Entry {
    match o {
        Outcome::Stored(e) => e,
        other => panic!("expected Stored, got {other:?}"),
    }
}

// ---- layout / validation ------------------------------------------------

#[test]
fn store_root_is_app_data_history_vault_id() {
    let f = fx("root");
    let s = open(&f);
    assert_eq!(s.root(), fs::canonicalize(f.app.join("history").join("v1")).unwrap());
    assert!(!s.root().starts_with(s.vault_root()));
    assert!(s.root().join("store.json").is_file());
    assert!(s.root().join("index.log").is_file());
    assert!(s.root().join("objects").is_dir());
}

#[test]
fn refuses_store_inside_vault_and_creates_nothing() {
    let f = fx("inside");
    let app_in_vault = f.vault.join(".appdata");
    let r = Store::open(&app_in_vault, "v1", &f.vault, Config::default());
    assert!(matches!(r, Err(HistoryError::StoreInsideVault)));
    assert!(!app_in_vault.exists(), "nothing may be created inside the vault");
    // the vault itself as app data dir
    assert!(matches!(
        Store::open(&f.vault, "v1", &f.vault, Config::default()),
        Err(HistoryError::StoreInsideVault)
    ));
}

#[test]
fn store_bound_to_its_vault_id() {
    let f = fx("meta");
    drop(open(&f));
    let other = f.app.join("history").join("v2");
    fs::create_dir_all(&other).unwrap();
    fs::copy(f.app.join("history/v1/store.json"), other.join("store.json")).unwrap();
    assert!(matches!(
        Store::open(&f.app, "v2", &f.vault, Config::default()),
        Err(HistoryError::Meta(_))
    ));
}

#[test]
fn vault_id_and_path_validation() {
    for bad in ["", "a/b", "..", "a b", "x\0", "a".repeat(65).as_str()] {
        assert!(validate_vault_id(bad).is_err(), "{bad:?}");
    }
    validate_vault_id("Abc_09-z").unwrap();
    for bad in ["", "/abs", "a//b", "./a", "a/../b", "..", "a\\b", "C:/x", "a/", "n\0ul"] {
        assert!(validate_path(bad).is_err(), "{bad:?}");
    }
    for ok in ["a.md", "dir/sub/n.md", ".obsidian/app.json", "tab\tand\nnl.md", "ünï.md"] {
        validate_path(ok).unwrap();
    }
    let f = fx("val");
    let mut s = open(&f);
    assert!(matches!(s.snapshot_before_overwrite("../escape", 1), Err(HistoryError::InvalidPath(_))));
}

#[cfg(unix)]
#[test]
fn private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = fx("perm");
    let mut s = open(&f);
    write(&f, "a.md", b"secret");
    let e = stored(s.snapshot_before_overwrite("a.md", 1).unwrap());
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(s.root()), 0o700);
    assert_eq!(mode(&s.root().join("index.log")), 0o600);
    assert_eq!(mode(&s.root().join("store.json")), 0o600);
    assert_eq!(mode(&s.cas.path_of(e.sha.as_deref().unwrap())), 0o600);
}

// ---- CAS ----------------------------------------------------------------

#[test]
fn cas_dedups_and_detects_corruption() {
    let f = fx("cas");
    let mut s = open(&f);
    write(&f, "a.md", b"same");
    write(&f, "b.md", b"same");
    let a = stored(s.record_own_save("a.md", 1).unwrap());
    let b = stored(s.record_own_save("b.md", 2).unwrap());
    assert_eq!(a.sha, b.sha);
    let sha = a.sha.unwrap();
    assert_eq!(sha, cas::sha_hex(b"same"));
    assert_eq!(s.read_blob(&sha).unwrap(), b"same");
    fs::write(s.cas.path_of(&sha), b"tampered").unwrap();
    assert!(matches!(s.read_blob(&sha), Err(HistoryError::BlobMismatch(_))));
    assert!(matches!(s.read_blob("../../etc"), Err(HistoryError::NotFound(_))));
}

// ---- invariant: snapshot before overwrite -------------------------------

#[test]
fn snapshot_before_overwrite_keeps_old_bytes() {
    let f = fx("ovw");
    let mut s = open(&f);
    write(&f, "n.md", b"v1");
    let pre = stored(s.snapshot_before_overwrite("n.md", 10).unwrap());
    assert_eq!(pre.reason, Reason::PreWrite);
    write(&f, "n.md", b"v2"); // the app's save
    let own = stored(s.record_own_save("n.md", 11).unwrap());
    assert_eq!(own.reason, Reason::OwnSave);
    // next save: v2 already stored -> nothing new, but still guaranteed
    assert_eq!(s.snapshot_before_overwrite("n.md", 12).unwrap(), Outcome::Unchanged);
    write(&f, "n.md", b"v3");
    let vs = s.versions("n.md");
    assert_eq!(vs.len(), 2);
    assert_eq!(s.read_version(vs[0].seq).unwrap(), b"v2");
    assert_eq!(s.read_version(vs[1].seq).unwrap(), b"v1");
}

#[test]
fn every_overwritten_state_is_recoverable() {
    let f = fx("chain");
    let mut s = open(&f);
    let states: Vec<Vec<u8>> = (0..20).map(|i| format!("state {i}").into_bytes()).collect();
    write(&f, "n.md", &states[0]);
    for (i, next) in states.iter().enumerate().skip(1) {
        s.snapshot_before_overwrite("n.md", i as u64).unwrap();
        write(&f, "n.md", next); // no record_own_save: invariant must hold anyway
    }
    let have: Vec<Vec<u8>> = s.versions("n.md").iter().map(|e| s.read_version(e.seq).unwrap()).collect();
    for st in &states[..states.len() - 1] {
        assert!(have.contains(st), "lost {:?}", String::from_utf8_lossy(st));
    }
}

#[test]
fn missing_file_before_overwrite_is_noop() {
    let f = fx("new");
    let mut s = open(&f);
    assert_eq!(s.snapshot_before_overwrite("new.md", 1).unwrap(), Outcome::Missing);
    assert!(s.entries().is_empty());
}

// ---- invariant: snapshot before delete ----------------------------------

#[test]
fn snapshot_before_delete_writes_tombstone_with_bytes() {
    let f = fx("del");
    let mut s = open(&f);
    write(&f, "d/x.md", b"last words");
    let t = stored(s.snapshot_before_delete("d/x.md", 5).unwrap());
    fs::remove_file(f.vault.join("d/x.md")).unwrap();
    assert_eq!(t.reason, Reason::Delete);
    assert_eq!(t.size, 10);
    assert_eq!(s.read_version(t.seq).unwrap(), b"last words");
    assert_eq!(s.head("d/x.md"), Some(&Head::Deleted { last: t.sha.clone() }));
    // restore from the tombstone brings it back
    let r = s.restore(t.seq, 6).unwrap();
    assert_eq!(r.reason, Reason::Restore);
    assert_eq!(fs::read(f.vault.join("d/x.md")).unwrap(), b"last words");
    assert!(matches!(s.head("d/x.md"), Some(Head::Live { .. })));
}

#[test]
fn delete_of_already_gone_file_tombstones_last_capture() {
    let f = fx("gone");
    let mut s = open(&f);
    write(&f, "g.md", b"seen");
    s.record_own_save("g.md", 1).unwrap();
    fs::remove_file(f.vault.join("g.md")).unwrap();
    let t = stored(s.snapshot_before_delete("g.md", 2).unwrap());
    assert_eq!(s.read_version(t.seq).unwrap(), b"seen");
    assert_eq!(s.snapshot_before_delete("never.md", 3).unwrap(), Outcome::Missing);
}

#[test]
fn too_large_is_skipped_not_silently_stored() {
    let f = fx("big");
    let mut s = Store::open(&f.app, "v1", &f.vault, Config { max_file_bytes: 4 }).unwrap();
    write(&f, "big.bin", b"12345");
    assert_eq!(s.snapshot_before_overwrite("big.bin", 1).unwrap(), Outcome::Skipped(Skip::TooLarge(5)));
    let t = stored(s.snapshot_before_delete("big.bin", 2).unwrap());
    assert_eq!(t.sha, None);
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    let f = fx("link");
    let mut s = open(&f);
    let outside = f._t.0.join("outside-secret");
    fs::write(&outside, b"not vault data").unwrap();
    std::os::unix::fs::symlink(&outside, f.vault.join("l.md")).unwrap();
    assert_eq!(s.record_external("l.md", 1).unwrap(), Outcome::Skipped(Skip::NotRegularFile));
    assert_eq!(s.snapshot_before_delete("l.md", 2).unwrap(), Outcome::Skipped(Skip::NotRegularFile));
    assert!(s.entries().is_empty());
}

// ---- external edits ------------------------------------------------------

#[test]
fn external_edits_are_captured_and_own_echo_is_not_external() {
    let f = fx("ext");
    let mut s = open(&f);
    write(&f, "e.md", b"mine");
    s.record_own_save("e.md", 1).unwrap();
    // watcher echo of our own write
    assert_eq!(s.record_external("e.md", 2).unwrap(), Outcome::Unchanged);
    // another editor writes twice; each state captured as it appears
    write(&f, "e.md", b"theirs 1");
    let x1 = stored(s.record_external("e.md", 3).unwrap());
    write(&f, "e.md", b"theirs 2");
    let x2 = stored(s.record_external("e.md", 4).unwrap());
    assert_eq!((x1.reason, x2.reason), (Reason::External, Reason::External));
    // our next save overwrites theirs 2 -> already stored
    assert_eq!(s.snapshot_before_overwrite("e.md", 5).unwrap(), Outcome::Unchanged);
    let got: Vec<Vec<u8>> = s.versions("e.md").iter().map(|e| s.read_version(e.seq).unwrap()).collect();
    assert_eq!(got, vec![b"theirs 2".to_vec(), b"theirs 1".to_vec(), b"mine".to_vec()]);
}

#[test]
fn external_delete_becomes_tombstone() {
    let f = fx("extdel");
    let mut s = open(&f);
    write(&f, "x.md", b"before rm");
    s.record_external("x.md", 1).unwrap();
    fs::remove_file(f.vault.join("x.md")).unwrap();
    let t = stored(s.record_external("x.md", 2).unwrap());
    assert_eq!(t.reason, Reason::Delete);
    assert_eq!(s.read_version(t.seq).unwrap(), b"before rm");
    // repeated event for the gone file: nothing more
    assert_eq!(s.record_external("x.md", 3).unwrap(), Outcome::Missing);
}

// ---- idle snapshots ------------------------------------------------------

#[test]
fn debouncer_waits_for_quiet() {
    let mut d = Debouncer::new(1000);
    assert!(d.is_empty() && d.next_due().is_none());
    d.touch("a.md", 0);
    d.touch("b.md", 100);
    d.touch("a.md", 600); // restarts a's window
    assert_eq!(d.next_due(), Some(1100));
    assert!(d.due(999).is_empty());
    assert_eq!(d.due(1100), vec!["b.md".to_string()]);
    assert!(d.due(1599).is_empty());
    assert_eq!(d.due(1600), vec!["a.md".to_string()]);
    assert!(d.is_empty());
    d.touch("c.md", 0);
    d.cancel("c.md");
    assert!(d.due(u64::MAX).is_empty());
}

#[test]
fn idle_snapshot_stores_settled_state_once() {
    let f = fx("idle");
    let mut s = open(&f);
    let mut d = Debouncer::new(2000);
    for (t, body) in [(0u64, "h"), (300, "he"), (700, "hello")] {
        write(&f, "i.md", body.as_bytes());
        d.touch("i.md", t);
    }
    assert!(d.due(2000).is_empty());
    let due = d.due(2700);
    assert_eq!(due, vec!["i.md".to_string()]);
    let e = stored(s.snapshot_idle(&due[0], 2700).unwrap());
    assert_eq!(e.reason, Reason::Idle);
    assert_eq!(s.read_version(e.seq).unwrap(), b"hello");
    assert_eq!(s.snapshot_idle("i.md", 5000).unwrap(), Outcome::Unchanged);
}

// ---- persistence / journal ----------------------------------------------

#[test]
fn reopen_replays_journal() {
    let f = fx("reopen");
    let (seqs, head) = {
        let mut s = open(&f);
        write(&f, "a.md", b"1");
        s.record_own_save("a.md", 1).unwrap();
        write(&f, "a.md", b"2");
        s.record_external("a.md", 2).unwrap();
        s.snapshot_before_delete("a.md", 3).unwrap();
        (s.entries().iter().map(|e| e.seq).collect::<Vec<_>>(), s.head("a.md").cloned())
    };
    let mut s = open(&f);
    assert_eq!(s.entries().iter().map(|e| e.seq).collect::<Vec<_>>(), seqs);
    assert_eq!(s.head("a.md").cloned(), head);
    write(&f, "b.md", b"x");
    assert_eq!(stored(s.record_own_save("b.md", 4).unwrap()).seq, 4);
}

#[test]
fn torn_last_line_is_truncated_other_corruption_fails() {
    let f = fx("torn");
    {
        let mut s = open(&f);
        write(&f, "a.md", b"1");
        s.record_own_save("a.md", 1).unwrap();
    }
    let log = f.app.join("history/v1/index.log");
    let good = fs::read(&log).unwrap();
    let mut torn = good.clone();
    torn.extend_from_slice(b"2\t5\town-sa");
    fs::write(&log, &torn).unwrap();
    let s = open(&f);
    assert_eq!(s.entries().len(), 1);
    drop(s);
    assert_eq!(fs::read(&log).unwrap(), good);
    let mut bad = good.clone();
    bad.extend_from_slice(b"garbage line\n");
    fs::write(&log, &bad).unwrap();
    assert!(matches!(
        Store::open(&f.app, "v1", &f.vault, Config::default()),
        Err(HistoryError::Corrupt { line: 3, .. })
    ));
}

#[cfg(unix)] // tab/newline/DEL are legal in unix file names only
#[test]
fn journal_escaping_round_trips() {
    let f = fx("esc");
    let weird = "dir %41/tab\there\nnl\u{7f}.md";
    {
        let mut s = open(&f);
        write(&f, weird, b"w");
        stored(s.record_own_save(weird, 1).unwrap());
    }
    let s = open(&f);
    assert_eq!(s.entries()[0].path, weird);
    let line = fs::read_to_string(f.app.join("history/v1/index.log")).unwrap();
    assert_eq!(line.lines().count(), 2, "escapes keep one record per line");
    for x in ["%", "a%25b", "%0A", "%7F", "ü"] {
        let raw = journal::unescape(&journal::escape(x)).unwrap();
        assert_eq!(raw, x);
    }
    assert!(journal::unescape("%41").is_err(), "non-canonical");
    assert!(journal::unescape("%4").is_err());
}

#[test]
fn journal_decode_rejects_inconsistent_records() {
    let sha = "a".repeat(64);
    assert!(journal::decode(&format!("1\t1\town-save\t{sha}\t1\ta.md\t")).is_ok());
    assert!(journal::decode("1\t1\town-save\t\t0\ta.md\t").is_err()); // no sha
    assert!(journal::decode("1\t1\tdelete\t\t0\ta.md\t").is_ok()); // bare tombstone
    assert!(journal::decode(&format!("1\t1\town-save\t{sha}\t1\ta.md\tb.md")).is_err());
    assert!(journal::decode(&format!("1\t1\trename\t{sha}\t1\ta.md\t")).is_err());
    assert!(journal::decode(&format!("1\t1\tbogus\t{sha}\t1\ta.md\t")).is_err());
    assert!(journal::decode(&format!("1\t1\town-save\t{}\t1\ta.md\t", "A".repeat(64))).is_err());
}

#[test]
fn restore_snapshots_current_bytes_first() {
    let f = fx("restore");
    let mut s = open(&f);
    write(&f, "r.md", b"old");
    let old = stored(s.record_own_save("r.md", 1).unwrap());
    write(&f, "r.md", b"unsaved external");
    s.restore(old.seq, 2).unwrap();
    assert_eq!(fs::read(f.vault.join("r.md")).unwrap(), b"old");
    let got: Vec<Vec<u8>> = s.versions("r.md").iter().map(|e| s.read_version(e.seq).unwrap()).collect();
    assert!(got.contains(&b"unsaved external".to_vec()), "restore must not lose the bytes it replaced");
    assert!(!f.vault.join(".r.md.osy-restore.tmp").exists());
}
