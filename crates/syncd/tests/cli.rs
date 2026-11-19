// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! opensidian-syncd CLI + daemon against a real opensidian-sync-server on
//! loopback: help, setup (create / join from words), status, sync-now of
//! vault folders between two devices, the daemon socket (0700 dir, 0600
//! socket, a second daemon refused, CLI calls routed over IPC), history +
//! restore, destroy without / with the words.
#![cfg(unix)]

use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;
use sync_core::keystore::FixedPresence;
use sync_server::{serve, App, Config};
use syncd::local::LocalEngine;

static N: AtomicU32 = AtomicU32::new(0);
const BIN: &str = env!("CARGO_BIN_EXE_opensidian-syncd");

fn tmpdir(tag: &str) -> PathBuf {
    // short: the socket path must fit sun_path
    let d = std::env::temp_dir().join(format!("osd-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Server {
    _rt: tokio::runtime::Runtime,
    app: Arc<App>,
    url: String,
}

fn server() -> Server {
    let mut cfg = Config::new(tmpdir("srv"));
    cfg.anon_rate_per_min = 10_000;
    let app = Arc::new(App::open(cfg).expect("app"));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let l = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = l.local_addr().unwrap().port();
    let a = app.clone();
    rt.spawn(async move { serve(a, l).await });
    Server { _rt: rt, app, url: format!("http://127.0.0.1:{port}") }
}

struct Dev {
    state: PathBuf,
    vault: PathBuf,
    run: PathBuf,
}

fn dev(tag: &str) -> Dev {
    let base = tmpdir(tag);
    let d = Dev { state: base.join("st"), vault: base.join("v"), run: short_run(tag) };
    std::fs::create_dir_all(&d.vault).unwrap();
    d
}

fn cli(d: &Dev, args: &[&str], stdin: Option<&str>) -> Output {
    let mut c = Command::new(BIN);
    c.arg("--state").arg(&d.state).args(args).env("XDG_RUNTIME_DIR", &d.run);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut ch = c.spawn().unwrap();
    {
        let mut si = ch.stdin.take().unwrap();
        if let Some(s) = stdin {
            si.write_all(s.as_bytes()).unwrap();
            si.write_all(b"\n").unwrap();
        }
    }
    let o = ch.wait_with_output().unwrap();
    eprintln!(
        "$ opensidian-syncd {}\n{}{}rc={:?}",
        args.join(" "),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr),
        o.status.code()
    );
    o
}

fn ok(o: &Output) -> Value {
    assert!(o.status.success(), "failed: {}", String::from_utf8_lossy(&o.stderr));
    serde_json::from_slice(&o.stdout).expect("json stdout")
}

fn mode(p: &Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

struct Daemon(Child, PathBuf);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn daemon(d: &Dev, vault_hex: &str) -> Daemon {
    let log = d.state.parent().unwrap().join("daemon.log");
    let f = std::fs::File::create(&log).unwrap();
    let ch = Command::new(BIN)
        .arg("--state")
        .arg(&d.state)
        .args(["daemon", "--interval", "3600"])
        .env("XDG_RUNTIME_DIR", &d.run)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(f)
        .spawn()
        .unwrap();
    let sock = d.run.join("opensidian-syncd").join(format!("{vault_hex}.sock"));
    let t0 = Instant::now();
    while std::os::unix::net::UnixStream::connect(&sock).is_err() {
        assert!(t0.elapsed() < Duration::from_secs(30), "daemon did not come up: {}", std::fs::read_to_string(&log).unwrap_or_default());
        std::thread::sleep(Duration::from_millis(50));
    }
    Daemon(ch, log)
}

#[test]
fn help_lists_every_command() {
    let d = dev("help");
    let o = cli(&d, &["help"], None);
    assert!(o.status.success());
    let h = String::from_utf8(o.stdout).unwrap();
    for c in ["setup", "--words", "--pair", "--enroll", "daemon", "status", "sync-now", "history", "restore", "destroy", "pair"] {
        assert!(h.contains(c), "help misses {c}");
    }
    let o = cli(&d, &["status"], None);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("not-configured"));
}

#[test]
fn two_devices_daemon_history_destroy() {
    let s = server();
    let a = dev("a");
    let b = dev("b");
    let tok = s.app.create_enrollment(None, 600).unwrap();

    // A creates the vault
    let st = ok(&cli(&a, &["setup", "--url", &s.url, "--vault", a.vault.to_str().unwrap(), "--name", "laptop", "--enroll", &tok], None));
    assert_eq!(st["server_ok"], true);
    assert_eq!(st["cfg_gen"], 0);
    let vid = st["vault_id"].as_str().unwrap().to_string();
    assert_eq!(mode(&a.state), 0o700);
    assert_eq!(mode(&a.state.join("keys")), 0o700);
    // the enrollment token is single use
    let again = dev("a2");
    assert!(!cli(&again, &["setup", "--url", &s.url, "--vault", again.vault.to_str().unwrap(), "--name", "x", "--enroll", &tok], None)
        .status
        .success());

    std::fs::write(a.vault.join("note.md"), "line one\nline two\n").unwrap();
    std::fs::create_dir_all(a.vault.join("img")).unwrap();
    std::fs::write(a.vault.join("img/pic.bin"), vec![7u8; 3 << 20]).unwrap();
    let r = ok(&cli(&a, &["--local", "sync-now"], None));
    assert!(r["committed"].is_string());
    assert!(r["pushed"].as_u64().unwrap() >= 2, "commit + chunks pushed");

    // B joins from the words + URL only (words via stdin)
    let words = LocalEngine::open(&a.state, Box::new(FixedPresence(true))).unwrap().reveal_words().unwrap();
    let st = ok(&cli(&b, &["setup", "--url", &s.url, "--vault", b.vault.to_str().unwrap(), "--name", "desktop", "--words"], Some(words.as_str())));
    assert_eq!(st["vault_id"], vid.as_str());
    assert_eq!(st["cfg_gen"], 1);
    assert_eq!(st["active_devices"], 2);
    let r = ok(&cli(&b, &["sync-now"], None));
    assert_eq!(r["written"], 2);
    assert_eq!(std::fs::read_to_string(b.vault.join("note.md")).unwrap(), "line one\nline two\n");
    assert_eq!(std::fs::read(b.vault.join("img/pic.bin")).unwrap(), vec![7u8; 3 << 20]);

    // A's daemon: socket dir 0700, socket 0600, a second daemon is refused
    let dm = daemon(&a, &vid);
    let sdir = a.run.join("opensidian-syncd");
    assert_eq!(mode(&sdir), 0o700);
    assert_eq!(mode(&sdir.join(format!("{vid}.sock"))), 0o600);
    let o = cli(&a, &["daemon"], None);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("already serving"));

    // B edits + deletes; A syncs through the daemon
    std::fs::write(b.vault.join("note.md"), "line one\nline two edited on B\n").unwrap();
    ok(&cli(&b, &["sync-now"], None));
    let st = ok(&cli(&a, &["status"], None));
    assert_eq!(st["server_ok"], true);
    assert_eq!(st["device_name"], "laptop");
    let r = ok(&cli(&a, &["sync-now"], None));
    assert_eq!(r["written"], 1);
    assert_eq!(std::fs::read_to_string(a.vault.join("note.md")).unwrap(), "line one\nline two edited on B\n");
    let log = std::fs::read_to_string(&dm.1).unwrap();
    assert!(log.contains(r#"{"op":"status"}"#) && log.contains(r#"{"op":"sync-now"}"#), "calls went over IPC: {log}");

    // history: the overwritten version is kept and restorable
    let h = ok(&cli(&a, &["history", "note.md"], None));
    let h = h.as_array().unwrap();
    assert!(h.len() >= 2, "versions: {h:?}");
    // newest first; the oldest is the first local version
    let first = h.iter().map(|e| e["seq"].as_u64().unwrap()).min().unwrap();
    let o = cli(&a, &["show", &first.to_string()], None);
    assert_eq!(String::from_utf8_lossy(&o.stdout), "line one\nline two\n");
    ok(&cli(&a, &["restore", &first.to_string()], None));
    assert_eq!(std::fs::read_to_string(a.vault.join("note.md")).unwrap(), "line one\nline two\n");
    let r = ok(&cli(&a, &["sync-now"], None));
    assert!(r["committed"].is_string());
    ok(&cli(&b, &["sync-now"], None));
    assert_eq!(std::fs::read_to_string(b.vault.join("note.md")).unwrap(), "line one\nline two\n");
    assert!(!cli(&a, &["history", "../etc/passwd"], None).status.success());

    // destroy: wrong words refused, the right words return a receipt
    let other = sync_core::sync_crypto::words::Phrase::generate(&sync_core::sync_crypto::fips::FipsProvider::new().unwrap()).unwrap();
    let ow = other.to_text(&sync_core::sync_crypto::fips::FipsProvider::new().unwrap());
    let o = cli(&a, &["destroy"], Some(ow.as_str()));
    assert!(!o.status.success());
    let o = cli(&a, &["destroy"], Some("not even words"));
    assert!(!o.status.success());
    let rc = ok(&cli(&a, &["destroy"], Some(words.as_str())));
    assert!(rc["objects"].as_u64().unwrap() > 0);
    let st = ok(&cli(&b, &["--local", "status"], None));
    assert_eq!(st["server_ok"], false, "vault gone for B too");
    drop(dm);
}

/// `$XDG_RUNTIME_DIR` stand-in: TMPDIR can be too long for sun_path
/// (108 bytes) once `opensidian-syncd/<32 hex>.sock` is appended.
fn short_run(tag: &str) -> PathBuf {
    let d = PathBuf::from("/tmp").join(format!("osr{tag}{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    d
}
