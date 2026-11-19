// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Approved mode or nothing (cmvp.md §6): opensidian-sync-server refuses to
//! start when the OpenSSL FIPS provider is not active (empty module dir,
//! empty config), before it binds or opens its data dir.
#![cfg(unix)]

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_opensidian-sync-server");

#[test]
fn server_refuses_to_start_without_fips_provider() {
    let d = std::env::temp_dir().join(format!("osy-srv-nofips-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("openssl.cnf"), "").unwrap();
    let data = d.join("data");
    let mut ch = Command::new(BIN)
        .args(["serve", "--listen", "127.0.0.1:0", "--data"])
        .arg(&data)
        .env("OPENSSL_MODULES", &d)
        .env("OPENSSL_CONF", d.join("openssl.cnf"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let t0 = Instant::now();
    let status = loop {
        if let Some(s) = ch.try_wait().unwrap() {
            break s;
        }
        if t0.elapsed() > Duration::from_secs(20) {
            let _ = ch.kill();
            panic!("server kept running without the fips provider");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let o = ch.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    eprintln!("$ (no fips) opensidian-sync-server serve\n{err}rc={:?}", status.code());
    assert_eq!(status.code(), Some(2));
    assert!(err.contains("refusing to start") && err.contains("OpenSSL FIPS provider not active"), "{err}");
    assert!(!data.exists(), "a refused start must not create the data dir");
    eprintln!("REFUSED: sync-server serve without the fips provider");
}
