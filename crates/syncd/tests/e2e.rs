// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! End-to-end: a real opensidian-sync-server on 127.0.0.1 and three
//! independent clients (separate state dirs, keystores, vault folders) —
//! laptop + desktop + a fresh device that has nothing but the words and the
//! URL. Every client goes through the same SyncApi the CLI, the daemon and
//! the app use (syncd::local::LocalEngine), so this drives the whole stack:
//! workdir scan -> commit -> HTTP -> server -> pull/verify -> tip merge ->
//! materialise -> file history.
//!
//! Scenario (one vault, in order; every step prints `E2E PASS: <step>`):
//!   1. create vault (one-time enrollment token), second device joins from words
//!   2. concurrent offline edits, different lines -> clean merge on both
//!   3. concurrent offline edits, same line -> conflict file with the stock name
//!   4. conflict mode "Create conflict file" -> conflict file even when mergeable
//!   5. attachment > 1 MB -> 4 MiB chunk objects, bytes identical on the peer
//!   6. delete on one device vs edit on the other -> the edit wins
//!   7. revoke the desktop -> its sync is refused, roster shrinks
//!   8. fresh device bootstraps from the words + URL only, same tree
//!   9. destroy without a proof (no / forged signature, wrong words) refused;
//!      with the words' K_destroy proof -> signed receipt, vault gone for all
//!
//! Run: cargo test -p syncd --test e2e -- --nocapture
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use sync_core::merge::ConflictMode;
use sync_core::sync_crypto::fips::FipsProvider;
use sync_core::sync_crypto::words::Phrase;
use sync_core::sync_proto::api::DestroyConfirm;
use sync_core::transport::Transport;
use sync_server::{serve, App, Config};
use syncd::api::SyncApi;
use syncd::local::{Join, LocalEngine, Setup};

static N: AtomicU32 = AtomicU32::new(0);

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ose-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Server {
    _rt: tokio::runtime::Runtime,
    app: Arc<App>,
    url: String,
}

/// opensidian-sync-server (the same `serve` the binary runs) on a loopback port.
fn server() -> Server {
    let mut cfg = Config::new(tmpdir("srv"));
    cfg.anon_rate_per_min = 10_000;
    let app = Arc::new(App::open(cfg).expect("server app"));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let l = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = l.local_addr().unwrap().port();
    let a = app.clone();
    rt.spawn(async move { serve(a, l).await });
    Server { _rt: rt, app, url: format!("http://127.0.0.1:{port}") }
}

struct Client {
    name: &'static str,
    state: PathBuf,
    vault: PathBuf,
}

fn client(name: &'static str) -> Client {
    let base = tmpdir(name);
    let c = Client { name, state: base.join("state"), vault: base.join("vault") };
    std::fs::create_dir_all(&c.vault).unwrap();
    c
}

fn setup(c: &Client, url: &str, join: Join<'_>) -> (LocalEngine, Option<String>) {
    let (e, w) = LocalEngine::setup(Setup {
        state: &c.state,
        url,
        vault_dir: &c.vault,
        device_name: c.name,
        ca_file: None,
        join,
    })
    .unwrap_or_else(|e| panic!("{} setup: {e}", c.name));
    (e, w.map(|w| w.to_string()))
}

fn sync(c: &Client, e: &LocalEngine) -> syncd::api::SyncSummary {
    let s = e.sync_now().unwrap_or_else(|err| panic!("{} sync: {err}", c.name));
    assert!(s.replica_errors.is_empty(), "{}: replica errors {:?}", c.name, s.replica_errors);
    assert_eq!(s.rejected, 0, "{}: rejected objects", c.name);
    eprintln!(
        "  {} sync: committed={} pulled={} pushed={} losers={} written={} removed={}",
        c.name,
        s.committed.is_some(),
        s.pulled,
        s.pushed,
        s.losers,
        s.written,
        s.removed
    );
    s
}

fn put(c: &Client, path: &str, bytes: &[u8]) {
    let p = c.vault.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn read(c: &Client, path: &str) -> String {
    std::fs::read_to_string(c.vault.join(path)).unwrap_or_else(|e| panic!("{}: {path}: {e}", c.name))
}

/// Every regular file under the vault folder -> bytes (relative '/' paths).
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, d: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(d).unwrap() {
            let e = e.unwrap();
            let p = e.path();
            let ft = e.file_type().unwrap();
            if ft.is_dir() {
                walk(root, &p, out);
            } else if ft.is_file() {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// `"<stem> (Conflicted copy <device> YYYYMMDDHHMM).md"` -> device.
fn conflict_device(name: &str, stem: &str) -> Option<String> {
    let rest = name.strip_prefix(&format!("{stem} (Conflicted copy "))?.strip_suffix(").md")?;
    let (dev, stamp) = rest.rsplit_once(' ')?;
    if dev.is_empty() || stamp.len() != 12 || !stamp.bytes().all(|b| b.is_ascii_digit()) || !stamp.starts_with("20") {
        return None;
    }
    Some(dev.to_string())
}

fn conflict_copies(c: &Client, stem: &str) -> Vec<(String, String)> {
    tree(&c.vault)
        .into_iter()
        .filter_map(|(p, b)| conflict_device(&p, stem).map(|d| (p, d, b)))
        .map(|(p, d, b)| {
            eprintln!("  {}: conflict file {p:?} (device {d})", c.name);
            (p, String::from_utf8(b).unwrap())
        })
        .collect()
}

#[test]
fn conflict_name_pattern() {
    assert_eq!(conflict_device("n (Conflicted copy laptop 202611181530).md", "n").as_deref(), Some("laptop"));
    assert_eq!(conflict_device("n (Conflicted copy my pc 202611181530).md", "n").as_deref(), Some("my pc"));
    assert!(conflict_device("n (Conflicted copy laptop 2026111815).md", "n").is_none());
    assert!(conflict_device("n (conflicted copy laptop 202611181530).md", "n").is_none());
    assert!(conflict_device("n.md", "n").is_none());
}

#[test]
fn two_clients_and_a_fresh_device_end_to_end() {
    let srv = server();
    let lap = client("laptop");
    let desk = client("desktop");
    let fresh = client("fresh");

    // 1. create + join -------------------------------------------------------
    let tok = srv.app.create_enrollment(None, 600).unwrap();
    let (mut a, words) = setup(&lap, &srv.url, Join::Create { enrollment_token: &tok });
    let words = words.expect("create returns the words");
    assert_eq!(words.split_whitespace().count(), 12, "11 entropy words + 1 checksum word");
    let st = a.status().unwrap();
    assert!(st.server_ok, "{:?}", st.server_error);
    let vid = st.vault_id.clone();
    put(&lap, "note.md", b"line one of the shared note\nline two\nline three\nline four\nline five\nline six\nline seven\nline eight of the shared note\n");
    put(&lap, "c.md", b"alpha beta gamma\n");
    put(&lap, "m.md", b"first line here\nmiddle line\nmiddle line two\nmiddle line three\nlast line here\n");
    put(&lap, "d.md", b"to be deleted on the laptop\n");
    assert!(sync(&lap, &a).committed.is_some());
    let (mut b, _) = setup(&desk, &srv.url, Join::Words { words: &words });
    let st = b.status().unwrap();
    assert_eq!(st.vault_id, vid);
    assert_eq!(st.active_devices, Some(2));
    assert_eq!(sync(&desk, &b).written, 4);
    assert_eq!(tree(&lap.vault), tree(&desk.vault));
    eprintln!("E2E PASS: create vault + second client joins from words");

    // 2 + 3. concurrent offline edits (neither syncs in between) --------------
    put(&lap, "note.md", b"LINE ONE edited on the laptop\nline two\nline three\nline four\nline five\nline six\nline seven\nline eight of the shared note\n");
    put(&desk, "note.md", b"line one of the shared note\nline two\nline three\nline four\nline five\nline six\nline seven\nLINE EIGHT edited on the desktop\n");
    put(&lap, "c.md", b"one two three\n");
    put(&desk, "c.md", b"four five six\n");
    assert!(sync(&lap, &a).committed.is_some());
    let s = sync(&desk, &b);
    assert!(s.committed.is_some());
    assert!(s.losers >= 1, "the conflict loser goes to file history");
    sync(&lap, &a);
    let merged = "LINE ONE edited on the laptop\nline two\nline three\nline four\nline five\nline six\nline seven\nLINE EIGHT edited on the desktop\n";
    assert_eq!(read(&lap, "note.md"), merged);
    assert_eq!(read(&desk, "note.md"), merged);
    eprintln!("E2E PASS: concurrent edits, different lines -> clean merge on both clients");

    for c in [&lap, &desk] {
        let copies = conflict_copies(c, "c");
        assert_eq!(copies.len(), 1, "{}: one conflict file", c.name);
        let kept = read(c, "c.md");
        let mut both = [kept.as_str(), copies[0].1.as_str()];
        both.sort();
        assert_eq!(both, ["four five six\n", "one two three\n"], "{}: both versions survive", c.name);
    }
    assert_eq!(tree(&lap.vault), tree(&desk.vault));
    // the loser is in the merging client's file history too
    let h = b.history("c.md").unwrap();
    assert!(h.iter().any(|i| i.reason == "sync-loser"), "history: {h:?}");
    eprintln!("E2E PASS: concurrent edits, same line -> conflict file '<note> (Conflicted copy <device> YYYYMMDDHHMM).md'");

    // 4. per-device conflict mode "Create conflict file" ------------------------
    b.set_conflict_mode(ConflictMode::ConflictFile).unwrap();
    assert_eq!(b.status().unwrap().conflict_mode, "conflict-file");
    put(&lap, "m.md", b"first line here EDITED BY LAPTOP\nmiddle line\nmiddle line two\nmiddle line three\nlast line here\n");
    put(&desk, "m.md", b"first line here\nmiddle line\nmiddle line two\nmiddle line three\nlast line here EDITED BY DESKTOP\n");
    sync(&lap, &a);
    sync(&desk, &b);
    sync(&lap, &a);
    for c in [&lap, &desk] {
        assert_eq!(conflict_copies(c, "m").len(), 1, "{}: mergeable, but the mode says conflict file", c.name);
    }
    assert_eq!(tree(&lap.vault), tree(&desk.vault));
    b.set_conflict_mode(ConflictMode::AutoMerge).unwrap();
    a.set_conflict_mode(ConflictMode::AutoMerge).unwrap();
    eprintln!("E2E PASS: conflict mode 'Create conflict file' -> conflict file instead of a merge");

    // 5. attachment > 1 MB ------------------------------------------------------
    let att: Vec<u8> = (0..(5u32 << 20) + 4321).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
    put(&lap, "attachments/scan.pdf", &att);
    let s = sync(&lap, &a);
    // 5 MiB + 4321 B -> ceil(size / 4 MiB) = 2 chunk objects + the commit
    assert!(s.pushed >= 3, "commit + 2 chunk objects pushed, got {}", s.pushed);
    let s = sync(&desk, &b);
    assert!(s.pulled >= 3, "commit + 2 chunk objects pulled, got {}", s.pulled);
    assert_eq!(std::fs::read(desk.vault.join("attachments/scan.pdf")).unwrap(), att);
    eprintln!("E2E PASS: attachment {} B (> 1 MB) synced as 4 MiB chunk objects, bytes identical", att.len());

    // 6. delete vs edit -----------------------------------------------------------
    // the edit is committed FIRST and the delete later (offline, unseen), so
    // last-write-wins alone would pick the delete — edit must still win
    put(&desk, "d.md", b"edited on the desktop while the laptop deleted it\n");
    std::fs::remove_file(lap.vault.join("d.md")).unwrap();
    sync(&desk, &b);
    sync(&lap, &a);
    sync(&desk, &b);
    assert_eq!(read(&lap, "d.md"), "edited on the desktop while the laptop deleted it\n");
    assert_eq!(read(&desk, "d.md"), "edited on the desktop while the laptop deleted it\n");
    assert_eq!(tree(&lap.vault), tree(&desk.vault));
    eprintln!("E2E PASS: delete on laptop vs edit on desktop -> edit beats delete");

    // 7. revoke the desktop -------------------------------------------------------
    let desk_id = b.status().unwrap().device_id;
    let epoch = a.revoke(&desk_id, &words).unwrap();
    assert!(epoch >= 1, "revocation publishes a new VK epoch");
    put(&desk, "after-revoke.md", b"written by the revoked device\n");
    let e = b.sync_now().expect_err("revoked device must be refused");
    assert_eq!(e.code, "revoked", "{e}");
    let st = b.status().unwrap();
    assert!(!st.server_ok);
    let st = a.status().unwrap();
    assert!(st.server_ok);
    assert_eq!(st.active_devices, Some(1));
    assert_eq!(st.vk_epoch, Some(epoch));
    sync(&lap, &a);
    assert!(!lap.vault.join("after-revoke.md").exists());
    eprintln!("E2E PASS: revoked device rejected (sync refused 'revoked', new VK epoch {epoch}, roster 1)");

    // 8. fresh device: words + URL only ----------------------------------------------
    let (c, _) = setup(&fresh, &srv.url, Join::Words { words: &words });
    let st = c.status().unwrap();
    assert!(st.server_ok, "{:?}", st.server_error);
    assert_eq!(st.vault_id, vid);
    assert_eq!(st.vk_epoch, Some(epoch));
    assert_eq!(st.active_devices, Some(2));
    sync(&fresh, &c);
    assert_eq!(tree(&fresh.vault), tree(&lap.vault), "fresh device materialises the same tree");
    assert_eq!(std::fs::read(fresh.vault.join("attachments/scan.pdf")).unwrap(), att);
    eprintln!("E2E PASS: fresh client bootstraps from words + URL only ({} files)", tree(&fresh.vault).len());

    // 9. destroy ------------------------------------------------------------------------
    let (t, m) = a.member().unwrap();
    let ns = m.env.servers[m.server].ns;
    // no signature at all
    let ch = t.destroy_request(&ns).unwrap();
    let r = t.destroy_confirm(&ns, &DestroyConfirm { confirm_token: ch.confirm_token, ts: now_s(), sig: String::new() });
    assert!(r.is_err(), "destroy without a proof must fail");
    // a forged (all-zero) signature
    let ch = t.destroy_request(&ns).unwrap();
    let zero_sig = format!("{}==", "A".repeat(86));
    let r = t.destroy_confirm(&ns, &DestroyConfirm { confirm_token: ch.confirm_token, ts: now_s(), sig: zero_sig });
    assert!(r.is_err(), "destroy with a forged proof must fail");
    // a valid phrase of ANOTHER vault: well-formed proof by the wrong K_destroy
    let p = FipsProvider::new().unwrap();
    let other = Phrase::generate(&p).unwrap().to_text(&p).to_string();
    assert!(a.destroy(&other).is_err(), "wrong words must not destroy");
    assert!(a.status().unwrap().server_ok, "vault still alive after the refused attempts");
    sync(&fresh, &c);
    eprintln!("E2E PASS: destroy without proof refused (no sig, forged sig, wrong words)");
    let rc = a.destroy(&words).expect("destroy with the words' K_destroy proof");
    assert_eq!(rc.ns, ns.to_hex());
    assert!(rc.objects > 0 && rc.bytes > 0, "{rc:?}");
    let st = c.status().unwrap();
    assert!(!st.server_ok, "vault gone for the other devices too");
    assert!(c.sync_now().is_err());
    eprintln!("E2E PASS: destroy with proof -> signed receipt ({} objects, {} B), vault gone", rc.objects, rc.bytes);
}

fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
