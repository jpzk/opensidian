// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Daemon IPC: a unix socket in a 0700 directory
//! (`$XDG_RUNTIME_DIR/opensidian-syncd/<vault>.sock`, socket 0600), every
//! connection's peer uid checked with SO_PEERCRED (tokio's safe
//! `UnixStream::peer_cred`) against the daemon's own uid. One JSON
//! [`Request`] per line, one [`Response`] per line.

use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::api::{dispatch, ApiError, ApiResult, HistoryItem, Receipt, Request, Response, Status, SyncApi, SyncSummary};

/// Longest accepted request line.
pub const MAX_LINE: u64 = 1 << 20;

/// Default socket dir: `$XDG_RUNTIME_DIR/opensidian-syncd`, else `<state>/run`.
pub fn default_socket_dir(state: &Path) -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("opensidian-syncd"),
        _ => state.join("run"),
    }
}

pub fn socket_path(dir: &Path, vault_hex: &str) -> PathBuf {
    dir.join(format!("{vault_hex}.sock"))
}

/// This process's effective uid without unsafe code: the owner of a file
/// we just created.
pub fn own_uid(dir: &Path) -> io::Result<u32> {
    let probe = dir.join(format!(".uid-probe-{}", std::process::id()));
    fs::write(&probe, b"")?;
    let uid = fs::metadata(&probe)?.uid();
    fs::remove_file(&probe)?;
    Ok(uid)
}

/// Create (or adopt) the socket dir: must end up a real dir, 0700, ours.
pub fn prepare_dir(dir: &Path) -> io::Result<u32> {
    fs::create_dir_all(dir)?;
    let md = fs::symlink_metadata(dir)?;
    if !md.is_dir() {
        return Err(io::Error::other(format!("{} is not a directory", dir.display())));
    }
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let uid = own_uid(dir)?;
    if fs::symlink_metadata(dir)?.uid() != uid {
        return Err(io::Error::other(format!("{} is owned by another user", dir.display())));
    }
    Ok(uid)
}

fn write_resp(w: &mut impl Write, r: &Response) -> io::Result<()> {
    let mut line = serde_json::to_vec(r).map_err(io::Error::other)?;
    line.push(b'\n');
    w.write_all(&line)?;
    w.flush()
}

fn handle(api: &dyn SyncApi, s: std::os::unix::net::UnixStream) -> io::Result<()> {
    s.set_nonblocking(false)?;
    let mut w = s.try_clone()?;
    let mut r = BufReader::new(s.take(MAX_LINE));
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Ok(());
        }
        if !line.ends_with('\n') {
            return write_resp(&mut w, &Response::Err(ApiError::new("bad-request", "request line too long")));
        }
        let resp = match serde_json::from_str::<Request>(line.trim_end()) {
            Ok(req) => {
                eprintln!("opensidian-syncd: {req}");
                dispatch(api, req)
            }
            Err(e) => Response::Err(ApiError::new("bad-request", e.to_string())),
        };
        write_resp(&mut w, &resp)?;
        // a fresh byte budget per request
        r.get_mut().set_limit(MAX_LINE);
    }
}

/// Bind `sock` (refusing if a live daemon already answers there) and serve
/// until the process exits. Connections from another uid are refused.
pub fn serve(api: Arc<dyn SyncApi + Send + Sync>, sock: &Path) -> io::Result<()> {
    let dir = sock.parent().ok_or_else(|| io::Error::other("socket path has no parent"))?;
    if sock.as_os_str().len() > 100 {
        return Err(io::Error::other(format!("socket path too long for a unix socket: {}", sock.display())));
    }
    let uid = prepare_dir(dir)?;
    if sock.exists() {
        if UnixStream::connect(sock).is_ok() {
            return Err(io::Error::new(io::ErrorKind::AddrInUse, "a daemon is already serving this vault"));
        }
        fs::remove_file(sock)?;
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_io().build()?;
    rt.block_on(async move {
        let l = tokio::net::UnixListener::bind(sock)?;
        fs::set_permissions(sock, fs::Permissions::from_mode(0o600))?;
        eprintln!("opensidian-syncd: listening on {}", sock.display());
        loop {
            let (s, _) = l.accept().await?;
            let peer = match s.peer_cred() {
                Ok(c) => c.uid(),
                Err(e) => {
                    eprintln!("opensidian-syncd: peer_cred failed: {e}");
                    continue;
                }
            };
            let s = s.into_std()?;
            if peer != uid {
                eprintln!("opensidian-syncd: refused connection from uid {peer}");
                let mut s = s;
                let _ = s.set_nonblocking(false);
                let _ = write_resp(&mut s, &Response::Err(ApiError::new("denied", "peer uid mismatch")));
                continue;
            }
            let api = api.clone();
            std::thread::spawn(move || {
                if let Err(e) = handle(api.as_ref(), s) {
                    eprintln!("opensidian-syncd: connection: {e}");
                }
            });
        }
    })
}

/// [`SyncApi`] over the daemon socket.
#[derive(Debug, Clone)]
pub struct IpcClient {
    sock: PathBuf,
    timeout: Duration,
}

impl IpcClient {
    /// Connects once to check a daemon answers.
    pub fn connect(sock: &Path) -> io::Result<IpcClient> {
        UnixStream::connect(sock)?;
        Ok(IpcClient { sock: sock.to_path_buf(), timeout: Duration::from_secs(600) })
    }

    fn call<T: serde::de::DeserializeOwned>(&self, req: &Request) -> ApiResult<T> {
        let io = |e: io::Error| ApiError::new("io", format!("daemon: {e}"));
        let mut s = UnixStream::connect(&self.sock).map_err(io)?;
        s.set_read_timeout(Some(self.timeout)).map_err(io)?;
        let mut line = serde_json::to_vec(req).map_err(|e| ApiError::new("io", e.to_string()))?;
        line.push(b'\n');
        s.write_all(&line).map_err(io)?;
        let mut r = BufReader::new(s.take(64 << 20));
        let mut resp = String::new();
        r.read_line(&mut resp).map_err(io)?;
        match serde_json::from_str::<Response>(resp.trim_end()).map_err(|e| ApiError::new("io", format!("daemon reply: {e}")))? {
            Response::Ok(v) => serde_json::from_value(v).map_err(|e| ApiError::new("io", e.to_string())),
            Response::Err(e) => Err(e),
        }
    }
}

impl SyncApi for IpcClient {
    fn status(&self) -> ApiResult<Status> {
        self.call(&Request::Status)
    }
    fn sync_now(&self) -> ApiResult<SyncSummary> {
        self.call(&Request::SyncNow)
    }
    fn history(&self, path: &str) -> ApiResult<Vec<HistoryItem>> {
        self.call(&Request::History { path: path.to_string() })
    }
    fn history_read(&self, seq: u64) -> ApiResult<Vec<u8>> {
        self.call(&Request::HistoryRead { seq })
    }
    fn restore(&self, seq: u64) -> ApiResult<HistoryItem> {
        self.call(&Request::Restore { seq })
    }
    fn destroy(&self, words: &str) -> ApiResult<Receipt> {
        self.call(&Request::Destroy { words: words.to_string() })
    }
}
