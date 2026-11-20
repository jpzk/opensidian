// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! opensidian-sync-server — self-host storage server (plan M3, spec §5).
//! Stores ciphertext only, holds no keys.
//!
//! ```text
//! opensidian-sync-server serve --data DIR [--listen ADDR] [--max-object BYTES]
//!                              [--retention-days N] [--max-env-total BYTES]
//! opensidian-sync-server vault create --data DIR [--quota BYTES] [--ttl-hours N]
//! ```
#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use sync_server::{effective_uid, serve, serve_tls, server_config, App, Config};

const USAGE: &str = "opensidian-sync-server — self-host storage for Opensidian Sync (ciphertext only)

USAGE:
  opensidian-sync-server serve --data DIR [--listen ADDR] [--max-object BYTES]
                               [--retention-days N] [--max-env-total BYTES]
                               [--tls-cert PEM --tls-key PEM]
  opensidian-sync-server vault create --data DIR [--quota BYTES] [--ttl-hours N]
  opensidian-sync-server --help | --version

serve         run the /v1 API. Default --listen 127.0.0.1:8471. With --tls-cert
              (PEM chain, leaf first) and --tls-key (PEM key) it speaks TLS 1.3
              on the OpenSSL 3 FIPS provider. Without TLS only loopback addresses
              are accepted. --max-env-total caps the tokenless envelope
              namespace (default 256 MiB); a full one answers 507 to every
              new envelope write: raise it and restart (envelopes are kept).
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
    max_env_total: Option<u64>,
    retention_days: Option<i64>,
    quota: Option<u64>,
    ttl_hours: i64,
    tls_cert: Option<PathBuf>,
    tls_key: Option<PathBuf>,
}

fn parse_flags(it: &mut dyn Iterator<Item = String>) -> Args {
    let mut a = Args {
        data: None,
        listen: "127.0.0.1:8471".parse().expect("static addr"),
        max_object: None,
        max_env_total: None,
        retention_days: None,
        quota: None,
        ttl_hours: 168,
        tls_cert: None,
        tls_key: None,
    };
    while let Some(f) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| die(&format!("{f} needs a value")));
        match f.as_str() {
            "--data" => a.data = Some(PathBuf::from(val())),
            "--listen" => a.listen = val().parse().unwrap_or_else(|_| die("bad --listen")),
            "--max-object" => a.max_object = Some(val().parse().unwrap_or_else(|_| die("bad --max-object"))),
            "--max-env-total" => a.max_env_total = Some(val().parse().unwrap_or_else(|_| die("bad --max-env-total"))),
            "--retention-days" => {
                a.retention_days = Some(val().parse().unwrap_or_else(|_| die("bad --retention-days")))
            }
            "--quota" => a.quota = Some(val().parse().unwrap_or_else(|_| die("bad --quota"))),
            "--ttl-hours" => a.ttl_hours = val().parse().unwrap_or_else(|_| die("bad --ttl-hours")),
            "--tls-cert" => a.tls_cert = Some(PathBuf::from(val())),
            "--tls-key" => a.tls_key = Some(PathBuf::from(val())),
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
    if let Some(m) = a.max_env_total {
        cfg.max_env_total = m;
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
    // approved mode or nothing (cmvp.md §6): every subcommand below touches keys or TLS
    if let Err(e) = sync_crypto::fips::init() {
        die(&format!("refusing to start: {e}"));
    }
    match cmd.as_str() {
        "serve" => {
            let a = parse_flags(&mut it);
            let tls = match (&a.tls_cert, &a.tls_key) {
                (Some(c), Some(k)) => {
                    let read = |p: &PathBuf| {
                        std::fs::read(p).unwrap_or_else(|e| die(&format!("read {}: {e}", p.display())))
                    };
                    Some(server_config(&read(c), &read(k)).unwrap_or_else(|e| die(&e)))
                }
                (None, None) => None,
                _ => die("--tls-cert and --tls-key go together"),
            };
            if tls.is_none() && !a.listen.ip().is_loopback() {
                die("plain HTTP only on loopback; pass --tls-cert/--tls-key or bind 127.0.0.1");
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
                let r = match tls {
                    Some(cfg) => {
                        println!("opensidian-sync-server listening on https://{addr} (TLS 1.3, FIPS)");
                        serve_tls(app, l, cfg).await
                    }
                    None => {
                        println!("opensidian-sync-server listening on http://{addr}");
                        serve(app, l).await
                    }
                };
                if let Err(e) = r {
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
