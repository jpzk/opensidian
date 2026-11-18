// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! opensidian-sync-server — self-host storage server (plan M3, spec §5).
//! Stores ciphertext only, holds no keys.
//!
//! ```text
//! opensidian-sync-server serve --data DIR [--listen ADDR] [--max-object BYTES]
//!                              [--retention-days N]
//! opensidian-sync-server vault create --data DIR [--quota BYTES] [--ttl-hours N]
//! ```
#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use sync_server::{effective_uid, serve, App, Config};

const USAGE: &str = "opensidian-sync-server — self-host storage for Opensidian Sync (ciphertext only)

USAGE:
  opensidian-sync-server serve --data DIR [--listen ADDR] [--max-object BYTES]
                               [--retention-days N]
  opensidian-sync-server vault create --data DIR [--quota BYTES] [--ttl-hours N]
  opensidian-sync-server --help | --version

serve         run the /v1 API. Default --listen 127.0.0.1:8471. Plain HTTP is
              accepted on loopback addresses only (put TLS in front otherwise).
vault create  print a one-time enrollment token for a new vault (default
              valid 168 h). Hand it to the client's setup; there is no open
              registration.

Refuses to run as root.";

fn die(msg: &str) -> ! {
    eprintln!("opensidian-sync-server: {msg}");
    std::process::exit(2);
}

struct Args {
    data: Option<PathBuf>,
    listen: SocketAddr,
    max_object: Option<u64>,
    retention_days: Option<i64>,
    quota: Option<u64>,
    ttl_hours: i64,
}

fn parse_flags(it: &mut dyn Iterator<Item = String>) -> Args {
    let mut a = Args {
        data: None,
        listen: "127.0.0.1:8471".parse().expect("static addr"),
        max_object: None,
        retention_days: None,
        quota: None,
        ttl_hours: 168,
    };
    while let Some(f) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| die(&format!("{f} needs a value")));
        match f.as_str() {
            "--data" => a.data = Some(PathBuf::from(val())),
            "--listen" => a.listen = val().parse().unwrap_or_else(|_| die("bad --listen")),
            "--max-object" => a.max_object = Some(val().parse().unwrap_or_else(|_| die("bad --max-object"))),
            "--retention-days" => {
                a.retention_days = Some(val().parse().unwrap_or_else(|_| die("bad --retention-days")))
            }
            "--quota" => a.quota = Some(val().parse().unwrap_or_else(|_| die("bad --quota"))),
            "--ttl-hours" => a.ttl_hours = val().parse().unwrap_or_else(|_| die("bad --ttl-hours")),
            other => die(&format!("unknown flag {other}\n\n{USAGE}")),
        }
    }
    a
}

fn app(a: &Args) -> Arc<App> {
    let data = a.data.clone().unwrap_or_else(|| die("--data DIR is required"));
    let mut cfg = Config::new(data);
    if let Some(m) = a.max_object {
        cfg.max_object = m;
    }
    if let Some(d) = a.retention_days {
        cfg.retention_secs = d.saturating_mul(86_400);
    }
    Arc::new(App::open(cfg).unwrap_or_else(|e| die(&e)))
}

fn main() {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_default();
    match cmd.as_str() {
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            return;
        }
        "--version" => {
            println!("opensidian-sync-server {} (API {})", env!("CARGO_PKG_VERSION"), sync_proto::API_VERSION);
            return;
        }
        _ => {}
    }
    if effective_uid() == Some(0) {
        die("refusing to run as root; use a dedicated unprivileged user");
    }
    match cmd.as_str() {
        "serve" => {
            let a = parse_flags(&mut it);
            if !a.listen.ip().is_loopback() {
                die("plain HTTP only on loopback; terminate TLS in front or bind 127.0.0.1");
            }
            let app = app(&a);
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap_or_else(|e| die(&format!("runtime: {e}")));
            rt.block_on(async move {
                let l = tokio::net::TcpListener::bind(a.listen)
                    .await
                    .unwrap_or_else(|e| die(&format!("bind {}: {e}", a.listen)));
                let addr = l.local_addr().map(|x| x.to_string()).unwrap_or_default();
                println!("opensidian-sync-server listening on http://{addr}");
                if let Err(e) = serve(app, l).await {
                    die(&format!("accept: {e}"));
                }
            });
        }
        "vault" => {
            if it.next().as_deref() != Some("create") {
                die(&format!("expected `vault create`\n\n{USAGE}"));
            }
            let a = parse_flags(&mut it);
            let app = app(&a);
            match app.create_enrollment(a.quota, a.ttl_hours.saturating_mul(3600)) {
                Ok(tok) => println!("{tok}"),
                Err(e) => die(&e),
            }
        }
        "" => die(USAGE),
        other => die(&format!("unknown command {other}\n\n{USAGE}")),
    }
}
