// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! opensidian-syncd — sync user service + CLI (plan M6). See `help`.
#![forbid(unsafe_code)]

use std::io::{BufRead as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use sync_core::keystore::FixedPresence;
use syncd::api::{ApiError, SyncApi};
use syncd::local::{config_path, fips_gate, Join, LocalEngine, Setup};
#[cfg(unix)]
use syncd::ipc;

const HELP: &str = "\
opensidian-syncd — end-to-end encrypted vault sync (user service + CLI)

USAGE: opensidian-syncd [--state DIR] [--socket PATH] [--local] <command> [args]

COMMANDS
  setup --url URL --vault DIR --name DEVICE [--ca PEM] (--enroll TOKEN | --words | --pair CODE)
        create a vault (--enroll: one-time token from `opensidian-sync-server vault create`;
        prints the 12 backup words ONCE), or join one from the words (read from stdin)
        or from a pairing code shown by a member device
  daemon [--interval SECS]    serve the unix socket and sync every SECS (default 60)
  status                      vault, device, server reachability, cfg gen, VK epoch, head
  sync-now                    commit the vault folder, sync, write the merged tree back
  history PATH                versions of one vault-relative file (file history)
  show SEQ                    print the bytes of one history record
  restore SEQ                 write history record SEQ back to its path
  pair [--with-words]         invite a device: print a pairing code, wait for it
                              (--with-words: words from stdin are handed over too)
  destroy                     delete the vault on the server (words from stdin = K_destroy proof)
  help                        this text

OPTIONS
  --state DIR    per-vault state (keystore, engine, history); default
                 $OPENSIDIAN_SYNCD_STATE, else $XDG_DATA_HOME/opensidian-syncd, else ~/.local/share/opensidian-syncd
  --socket PATH  daemon socket; default $XDG_RUNTIME_DIR/opensidian-syncd/<vault>.sock (dir 0700, peer uid checked)
  --local        never use the daemon; run the engine in this process

Secrets (words) are only ever read from stdin, never from argv. Output is JSON.
";

fn default_state() -> PathBuf {
    if let Some(s) = std::env::var_os("OPENSIDIAN_SYNCD_STATE").filter(|s| !s.is_empty()) {
        return PathBuf::from(s);
    }
    if let Some(s) = std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(s).join("opensidian-syncd");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".local/share/opensidian-syncd")
}

struct Args {
    rest: Vec<String>,
}

impl Args {
    fn flag(&mut self, name: &str) -> bool {
        if let Some(i) = self.rest.iter().position(|a| a == name) {
            self.rest.remove(i);
            true
        } else {
            false
        }
    }
    fn opt(&mut self, name: &str) -> Result<Option<String>, ApiError> {
        match self.rest.iter().position(|a| a == name) {
            Some(i) if i + 1 < self.rest.len() => {
                let v = self.rest.remove(i + 1);
                self.rest.remove(i);
                Ok(Some(v))
            }
            Some(_) => Err(ApiError::new("bad-request", format!("{name} needs a value"))),
            None => Ok(None),
        }
    }
    fn need(&mut self, name: &str) -> Result<String, ApiError> {
        self.opt(name)?.ok_or_else(|| ApiError::new("bad-request", format!("missing {name}")))
    }
    fn pos(&mut self, what: &str) -> Result<String, ApiError> {
        if self.rest.is_empty() || self.rest[0].starts_with("--") {
            return Err(ApiError::new("bad-request", format!("missing {what}")));
        }
        Ok(self.rest.remove(0))
    }
    fn done(&self) -> Result<(), ApiError> {
        match self.rest.first() {
            Some(a) => Err(ApiError::new("bad-request", format!("unexpected argument {a}"))),
            None => Ok(()),
        }
    }
}

fn stdin_words() -> Result<zeroize::Zeroizing<String>, ApiError> {
    eprintln!("reading the backup words from stdin…");
    let mut s = zeroize::Zeroizing::new(String::new());
    std::io::stdin().lock().read_line(&mut s).map_err(|e| ApiError::new("io", e.to_string()))?;
    Ok(s)
}

fn out<T: serde::Serialize>(v: &T) -> Result<(), ApiError> {
    let s = serde_json::to_string_pretty(v).map_err(|e| ApiError::new("io", e.to_string()))?;
    println!("{s}");
    Ok(())
}

fn local(state: &Path) -> Result<LocalEngine, ApiError> {
    LocalEngine::open(state, Box::new(FixedPresence(false)))
}

#[cfg(unix)]
fn sock_for(state: &Path, sock: &Option<PathBuf>) -> Result<PathBuf, ApiError> {
    if let Some(s) = sock {
        return Ok(s.clone());
    }
    let e = local(state)?;
    Ok(ipc::socket_path(&ipc::default_socket_dir(e.state_dir()), &e.config().vault_id.to_hex()))
}

/// The daemon if one answers on the socket, else the engine in-process.
fn api(state: &Path, sock: &Option<PathBuf>, force_local: bool) -> Result<Box<dyn SyncApi>, ApiError> {
    #[cfg(unix)]
    if !force_local {
        if let Ok(c) = ipc::IpcClient::connect(&sock_for(state, sock)?) {
            return Ok(Box::new(c));
        }
    }
    let _ = (sock, force_local);
    Ok(Box::new(local(state)?))
}

fn run(argv: Vec<String>) -> Result<(), ApiError> {
    let mut a = Args { rest: argv };
    let state = a.opt("--state")?.map(PathBuf::from).unwrap_or_else(default_state);
    let sock = a.opt("--socket")?.map(PathBuf::from);
    let force_local = a.flag("--local");
    if a.rest.is_empty() || a.flag("--help") || a.flag("-h") {
        print!("{HELP}");
        return Ok(());
    }
    let cmd = a.rest.remove(0);
    if cmd != "help" {
        // approved mode or nothing (cmvp.md §6): refuse before any state, key or socket use
        fips_gate()?;
    }
    match cmd.as_str() {
        "help" => {
            print!("{HELP}");
            Ok(())
        }
        "setup" => {
            let url = a.need("--url")?;
            let vault = PathBuf::from(a.need("--vault")?);
            let name = a.need("--name")?;
            let ca = a.opt("--ca")?.map(PathBuf::from);
            let enroll = a.opt("--enroll")?;
            let pair = a.opt("--pair")?;
            let words_flag = a.flag("--words");
            a.done()?;
            let words;
            let join = match (enroll.as_deref(), pair.as_deref(), words_flag) {
                (Some(t), None, false) => Join::Create { enrollment_token: t },
                (None, Some(c), false) => Join::Pair { code: c, wait: Duration::from_secs(600) },
                (None, None, true) => {
                    words = stdin_words()?;
                    Join::Words { words: words.as_str() }
                }
                _ => return Err(ApiError::new("bad-request", "exactly one of --enroll, --words, --pair")),
            };
            let creating = matches!(join, Join::Create { .. });
            let (e, w) = LocalEngine::setup(Setup {
                state: &state,
                url: &url,
                vault_dir: &vault,
                device_name: &name,
                ca_file: ca,
                join,
            })?;
            if creating {
                if let Some(w) = w {
                    let mut err = std::io::stderr().lock();
                    let _ = writeln!(err, "\nBACKUP WORDS — write them down now, they are shown only once:\n\n  {}\n", w.as_str());
                }
            }
            out(&e.status()?)
        }
        "status" => {
            a.done()?;
            out(&api(&state, &sock, force_local)?.status()?)
        }
        "sync-now" => {
            a.done()?;
            out(&api(&state, &sock, force_local)?.sync_now()?)
        }
        "history" => {
            let p = a.pos("PATH")?;
            a.done()?;
            out(&api(&state, &sock, force_local)?.history(&p)?)
        }
        "show" => {
            let seq = a.pos("SEQ")?.parse::<u64>().map_err(|e| ApiError::new("bad-request", e.to_string()))?;
            a.done()?;
            let b = api(&state, &sock, force_local)?.history_read(seq)?;
            std::io::stdout().write_all(&b).map_err(|e| ApiError::new("io", e.to_string()))
        }
        "restore" => {
            let seq = a.pos("SEQ")?.parse::<u64>().map_err(|e| ApiError::new("bad-request", e.to_string()))?;
            a.done()?;
            out(&api(&state, &sock, force_local)?.restore(seq)?)
        }
        "destroy" => {
            a.done()?;
            let w = stdin_words()?;
            out(&api(&state, &sock, force_local)?.destroy(&w)?)
        }
        "pair" => {
            let with = a.flag("--with-words");
            a.done()?;
            let w = if with { Some(stdin_words()?) } else { None };
            let e = local(&state)?;
            let mut show = |c: &str| {
                eprintln!("pairing code (enter on the new device within 10 minutes):\n\n  {c}\n");
            };
            let joined = e.pair(w.as_ref().map(|w| w.as_str()), Duration::from_secs(600), &mut show)?;
            out(&serde_json::json!({ "paired": joined }))
        }
        #[cfg(unix)]
        "daemon" => {
            let secs = a.opt("--interval")?.map(|s| s.parse::<u64>()).transpose().map_err(|e| ApiError::new("bad-request", e.to_string()))?;
            a.done()?;
            if !config_path(&state).exists() {
                return Err(ApiError::new("not-configured", format!("run setup first ({})", state.display())));
            }
            let path = sock_for(&state, &sock)?;
            let e: Arc<dyn SyncApi + Send + Sync> = Arc::new(local(&state)?);
            let every = Duration::from_secs(secs.unwrap_or(60).max(5));
            let bg = e.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(every);
                match bg.sync_now() {
                    Ok(s) => eprintln!("opensidian-syncd: synced head={:?} pulled={} pushed={}", s.head, s.pulled, s.pushed),
                    Err(err) => eprintln!("opensidian-syncd: sync: {err}"),
                }
            });
            ipc::serve(e, &path).map_err(|e| ApiError::new("io", e.to_string()))
        }
        other => Err(ApiError::new("bad-request", format!("unknown command {other} (see help)"))),
    }
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr(), "opensidian-syncd: {e}");
            let _ = out(&serde_json::json!({ "error": e }));
            ExitCode::from(1)
        }
    }
}
