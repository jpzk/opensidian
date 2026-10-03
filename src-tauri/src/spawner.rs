// SPDX-License-Identifier: GPL-3.0-or-later
/* vaultbleed D — THE UNCONFINED SPAWNER.

   Landlock is inherited and can only ever narrow: every child of a confined
   window is confined to THAT window's vault. So a confined window cannot start
   a window on another vault by itself — the new process would be locked into
   the old vault and get EACCES on its own. And ~/.opensidian.json cannot be a
   rule at all: a landlock rule on a FILE is bound to its inode, and every
   cfgstore write (any unconfined window's, cfgstore.rs) renames a new inode
   over it — after the first one the confined window's rule points at a dead
   inode and its reads and writes of the config are EACCES.

   So main() starts THIS helper BEFORE sandbox::enforce: the same binary,
   re-executed as `opensidian --opensidian-spawner`, stdin = one end of a
   socketpair. It never confines itself and does exactly four things, each a
   JSON line in, a JSON line out:
     Open{path}           validate (same vault_rules + lock probe the window
                          would run) and spawn::open_in_new_process -> Pid
     Create{parent,name}  make_vault_dir (mkdir + seed, binds nothing) -> Path
     Cfg{ops}             cfgstore::update_in (locked RMW, atomic rename)
     ReadCfg              cfgstore::read_value_in -> Value
   It exits on EOF, i.e. when the window that started it is gone (its socket
   end is CLOEXEC, so webkit's children never hold it open).

   THREAT MODEL in short (full text: goal progress.md, item 6): the socket is
   only in the Rust main process of the confined window, not in webkit's
   web/network processes. A compromised main process can, through the helper,
   (a) start another opensidian process on any directory vault_rules accepts —
   that process is itself confined to that one directory — and (b) set keys in
   ~/.opensidian.json. It cannot name any other path to write, read files, or
   run any program but this app's own entrypoint. */
use crate::cfgstore::Op;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// argv[1] that turns this binary into the spawner (and nothing else)
pub const ARG: &str = "--opensidian-spawner";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Req {
    Open { path: String },
    Create { parent: String, name: String },
    Cfg { ops: Vec<Op> },
    ReadCfg,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Resp {
    Pid(u32),
    Path(String),
    Value(Value),
    Done,
    Err(String),
}

/// what the helper does for each request — main.rs implements it with the
/// live vault rules; tests implement it with fakes.
pub trait Handler {
    fn open(&self, p: &Path) -> Result<u32, String>;
    fn create(&self, parent: &str, name: &str) -> Result<PathBuf, String>;
    fn cfg(&self, ops: &[Op]) -> Result<(), String>;
    fn read_cfg(&self) -> Value;
}

pub fn handle(h: &dyn Handler, req: Req) -> Resp {
    match req {
        Req::Open { path } => h.open(Path::new(&path)).map(Resp::Pid).unwrap_or_else(Resp::Err),
        Req::Create { parent, name } => h.create(&parent, &name).map(|p| Resp::Path(p.display().to_string())).unwrap_or_else(Resp::Err),
        Req::Cfg { ops } => h.cfg(&ops).map(|_| Resp::Done).unwrap_or_else(Resp::Err),
        Req::ReadCfg => Resp::Value(h.read_cfg()),
    }
}

/// serve until EOF. A line that is not a Req gets Err, never a crash.
pub fn serve(r: impl BufRead, mut w: impl Write, h: &dyn Handler) {
    for line in r.lines() {
        let Ok(line) = line else { return };
        let resp = match serde_json::from_str::<Req>(&line) {
            Ok(req) => handle(h, req),
            Err(e) => Resp::Err(format!("spawner: bad request: {e}")),
        };
        let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| "{\"Err\":\"encode\"}".into());
        out.push('\n');
        if w.write_all(out.as_bytes()).and_then(|_| w.flush()).is_err() {
            return;
        }
    }
}

/// the confined window's end. One request in flight at a time (Mutex), so
/// replies cannot cross between tauri command threads.
pub struct Client {
    io: Mutex<(BufReader<UnixStream>, UnixStream)>,
}

impl Client {
    pub fn new(s: UnixStream) -> std::io::Result<Client> {
        Ok(Client { io: Mutex::new((BufReader::new(s.try_clone()?), s)) })
    }

    pub fn call(&self, req: &Req) -> Result<Resp, String> {
        let mut g = self.io.lock().unwrap_or_else(|e| e.into_inner());
        let mut line = serde_json::to_string(req).map_err(|e| e.to_string())?;
        line.push('\n');
        g.1.write_all(line.as_bytes()).map_err(|e| format!("spawner gone: {e}"))?;
        let mut back = String::new();
        if g.0.read_line(&mut back).map_err(|e| format!("spawner gone: {e}"))? == 0 {
            return Err("spawner gone: EOF".into());
        }
        serde_json::from_str(&back).map_err(|e| format!("spawner: bad reply: {e}"))
    }

    pub fn open(&self, p: &Path) -> Result<u32, String> {
        match self.call(&Req::Open { path: p.display().to_string() })? {
            Resp::Pid(n) => Ok(n),
            Resp::Err(e) => Err(e),
            r => Err(format!("spawner: unexpected {r:?}")),
        }
    }

    pub fn create(&self, parent: &str, name: &str) -> Result<String, String> {
        match self.call(&Req::Create { parent: parent.into(), name: name.into() })? {
            Resp::Path(p) => Ok(p),
            Resp::Err(e) => Err(e),
            r => Err(format!("spawner: unexpected {r:?}")),
        }
    }

    pub fn cfg(&self, ops: &[Op]) -> Result<(), String> {
        match self.call(&Req::Cfg { ops: ops.to_vec() })? {
            Resp::Done => Ok(()),
            Resp::Err(e) => Err(e),
            r => Err(format!("spawner: unexpected {r:?}")),
        }
    }

    pub fn read_cfg(&self) -> Result<Value, String> {
        match self.call(&Req::ReadCfg)? {
            Resp::Value(v) => Ok(v),
            Resp::Err(e) => Err(e),
            r => Err(format!("spawner: unexpected {r:?}")),
        }
    }
}

static CLIENT: OnceLock<Client> = OnceLock::new();

/// Some = this process is confined and must go through the helper.
pub fn client() -> Option<&'static Client> {
    CLIENT.get()
}

/// publish the client (main(), only once landlock really is enforced; an
/// unpublished Client is dropped -> the helper reads EOF and exits).
pub fn install(c: Client) {
    let _ = CLIENT.set(c);
}

/// start the helper: this binary, argv[1] = ARG, stdin = its socket end.
/// MUST run before sandbox::enforce (an exec after it is confined for life).
pub fn start() -> Result<Client, String> {
    use std::os::fd::OwnedFd;
    let (mine, theirs) = UnixStream::pair().map_err(|e| format!("socketpair: {e}"))?;
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let mut child = std::process::Command::new(exe)
        .arg(ARG)
        .stdin(std::process::Stdio::from(OwnedFd::from(theirs)))
        .stdout(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawner: {e}"))?;
    eprintln!("[spawner] pid={} serves confined pid={}", child.id(), std::process::id());
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Client::new(mine).map_err(|e| e.to_string())
}

/// the helper's main: serve fd 0 until EOF.
pub fn run(h: &dyn Handler) -> i32 {
    use std::os::fd::FromRawFd;
    // SAFETY: fd 0 is the socketpair end start() handed us; nothing else owns it.
    let s = unsafe { UnixStream::from_raw_fd(0) };
    let r = match s.try_clone() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[spawner] fd 0 is not a socket: {e}");
            return 2;
        }
    };
    serve(BufReader::new(r), s, h);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        log: RefCell<Vec<String>>,
    }
    impl Handler for Fake {
        fn open(&self, p: &Path) -> Result<u32, String> {
            self.log.borrow_mut().push(format!("open {}", p.display()));
            if p.ends_with("busy") { Err("already open in another window".into()) } else { Ok(4242) }
        }
        fn create(&self, parent: &str, name: &str) -> Result<PathBuf, String> {
            Ok(Path::new(parent).join(name))
        }
        fn cfg(&self, ops: &[Op]) -> Result<(), String> {
            self.log.borrow_mut().push(format!("cfg {ops:?}"));
            Ok(())
        }
        fn read_cfg(&self) -> Value {
            serde_json::json!({"last": "/v/a"})
        }
    }

    /// every request kind round-trips over a real socketpair, errors come back
    /// as Err (not a hang), garbage gets an Err line, and EOF ends the server.
    #[test]
    fn spawner_protocol_round_trips_over_a_socketpair() {
        let (a, b) = UnixStream::pair().unwrap();
        let srv = std::thread::spawn(move || {
            let f = Fake { log: RefCell::new(vec![]) };
            serve(BufReader::new(b.try_clone().unwrap()), b, &f);
            f.log.into_inner()
        });
        let c = Client::new(a.try_clone().unwrap()).unwrap();
        assert_eq!(c.open(Path::new("/v/b")), Ok(4242));
        assert_eq!(c.open(Path::new("/v/busy")), Err("already open in another window".into()));
        assert_eq!(c.create("/v", "new"), Ok("/v/new".into()));
        assert_eq!(c.cfg(&[Op::PushRecent("/v/b".into())]), Ok(()));
        assert_eq!(c.read_cfg(), Ok(serde_json::json!({"last": "/v/a"})));
        {
            let mut g = c.io.lock().unwrap();
            g.1.write_all(b"not json\n").unwrap();
            let mut l = String::new();
            g.0.read_line(&mut l).unwrap();
            assert!(l.contains("bad request"), "{l}");
        }
        drop(c);
        drop(a); // EOF -> the server returns
        let log = srv.join().unwrap();
        assert_eq!(log, vec!["open /v/b".to_string(), "open /v/busy".into(), format!("cfg {:?}", [Op::PushRecent("/v/b".into())])]);
    }

    /// a dead helper is an Err for the caller, never a hang or a panic.
    #[test]
    fn spawner_gone_is_an_error() {
        let (a, b) = UnixStream::pair().unwrap();
        drop(b);
        let c = Client::new(a).unwrap();
        assert!(c.open(Path::new("/v/b")).unwrap_err().contains("spawner gone"));
    }
}
