// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* cp-unrestricted (operator ruling 2026-10-18, stock parity): the TCP listener
   behind node `net.createServer` / `http.createServer` in ui/plugins/node-desktop.js.
   Stock lets a plugin of a trusted vault open a server (advanced-slides serves
   its preview over http, local-rest-api its API); so do we, with the same one
   gate: main.rs checks the vault's trust flip on every listen. Connections are
   handed out only by a listener this page opened.

   Model: a listener thread polls a non-blocking accept (so close() needs no
   self-connect), every accepted socket gets a pump thread that fills a byte
   buffer the page drains with read(); writes go straight to the socket. HTTP is
   parsed in the page (node-desktop.js), not here: this is net, not http.
   Bytes cross the IPC as latin1 strings, as in desk_proc.rs. Flip OFF closes
   every listener and connection (close_all). */
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;

use crate::desk_proc::{bytes_to_latin1, Life};

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[derive(Default)]
struct ConnSt {
    inb: Vec<u8>,
    eof: bool,
}

pub struct Conn {
    sock: TcpStream,
    st: Mutex<ConnSt>,
    cv: Condvar,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Data {
    /// latin1 byte string
    pub data: String,
    /// the peer finished sending (or the socket is gone) and the buffer is drained
    pub eof: bool,
}

impl Conn {
    pub fn read(&self, wait: Duration) -> Data {
        let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
        if s.inb.is_empty() && !s.eof {
            s = self.cv.wait_timeout_while(s, wait, |s| s.inb.is_empty() && !s.eof).unwrap_or_else(|e| e.into_inner()).0;
        }
        let data = bytes_to_latin1(&std::mem::take(&mut s.inb));
        Data { data, eof: s.eof }
    }
    pub fn write(&self, b: &[u8]) -> Result<(), String> {
        (&self.sock).write_all(b).and_then(|_| (&self.sock).flush()).map_err(|e| format!("EPIPE: {e}"))
    }
    /// our side is done sending (Node socket.end)
    pub fn end(&self) {
        let _ = self.sock.shutdown(Shutdown::Write);
    }
    pub fn destroy(&self) {
        let _ = self.sock.shutdown(Shutdown::Both);
    }
}

fn pump(c: Arc<Conn>) {
    let mut b = [0u8; 16 * 1024];
    let mut r = match c.sock.try_clone() {
        Ok(r) => r,
        Err(_) => {
            c.st.lock().unwrap_or_else(|e| e.into_inner()).eof = true;
            c.cv.notify_all();
            return;
        }
    };
    loop {
        match r.read(&mut b) {
            Ok(0) | Err(_) => break,
            Ok(n) => c.st.lock().unwrap_or_else(|e| e.into_inner()).inb.extend_from_slice(&b[..n]),
        }
        c.cv.notify_all();
    }
    c.st.lock().unwrap_or_else(|e| e.into_inner()).eof = true;
    c.cv.notify_all();
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct Accepted {
    pub id: u64,
    #[serde(rename = "remoteAddress")]
    pub remote_address: String,
    #[serde(rename = "remotePort")]
    pub remote_port: u16,
}

#[derive(Default)]
struct SrvSt {
    pending: Vec<Accepted>,
}

pub struct Srv {
    closed: AtomicBool,
    st: Mutex<SrvSt>,
    cv: Condvar,
    pub port: u16,
    pub address: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Batch {
    pub conns: Vec<Accepted>,
    pub closed: bool,
}

impl Srv {
    pub fn accept(&self, wait: Duration) -> Batch {
        let mut s = self.st.lock().unwrap_or_else(|e| e.into_inner());
        let idle = |s: &SrvSt| s.pending.is_empty() && !self.closed.load(Ordering::SeqCst);
        if idle(&s) {
            s = self.cv.wait_timeout_while(s, wait, |s| idle(s)).unwrap_or_else(|e| e.into_inner()).0;
        }
        Batch { conns: std::mem::take(&mut s.pending), closed: self.closed.load(Ordering::SeqCst) }
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.cv.notify_all();
    }
}

#[derive(Default)]
struct Reg {
    srvs: HashMap<u64, Arc<Srv>>,
    conns: HashMap<u64, Arc<Conn>>,
}

fn reg() -> &'static Mutex<Reg> {
    static R: OnceLock<Mutex<Reg>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Reg::default()))
}

/// Node error text for a failed listen: "<CODE>: listen <host>:<port> <reason>"
fn listen_err(host: &str, port: u16, e: &std::io::Error) -> String {
    let code = match e.kind() {
        std::io::ErrorKind::AddrInUse => "EADDRINUSE",
        std::io::ErrorKind::PermissionDenied => "EACCES",
        std::io::ErrorKind::AddrNotAvailable => "EADDRNOTAVAIL",
        _ => "EIO",
    };
    format!("{code}: listen {host}:{port} {e}")
}

/* N5 (PR #41) + P2 (PR #42): what desk_net_listen runs. The lifecycle lock
   (desk_proc::lifecycle, the one plugins_set holds across a trust-OFF: flag write,
   kill_all, close_all) is taken BEFORE the deciding trust check and held through the bind
   and the register, so a trust-OFF either lands before that check (EPERM, nothing bound)
   or after the register (its close_all closes this listener). listen and close_all
   demand a &Life for that.
   P2: the host is resolved (to_socket_addrs: a DNS lookup that can take seconds) BEFORE
   the lock, so a slow lookup never holds up a trust-OFF, kill_all or a spawn. Under the
   lock run only (trust check, bind, register). A first, unlocked trust check precedes the
   lookup, so a restricted vault's page cannot make the app resolve names it picks (a DNS
   query is itself a way out); it decides nothing — the check under the lock does. */
pub fn listen_trusted(trusted: impl FnMut() -> bool, host: &str, port: u16) -> Result<(u64, Arc<Srv>), String> {
    listen_trusted_via(resolve, trusted, host, port)
}

const NOT_TRUSTED: &str = "EPERM: restricted mode, plugins cannot open servers";

/// Node: no host = all interfaces; "localhost" resolves
fn resolve(h: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    Ok((h, port).to_socket_addrs().map_err(|e| format!("ENOTFOUND: listen {h}: {e}"))?.collect())
}

fn listen_trusted_via(
    resolve: impl FnOnce(&str, u16) -> Result<Vec<SocketAddr>, String>,
    mut trusted: impl FnMut() -> bool,
    host: &str,
    port: u16,
) -> Result<(u64, Arc<Srv>), String> {
    let h = if host.is_empty() { "0.0.0.0" } else { host };
    if !trusted() {
        return Err(NOT_TRUSTED.into());
    }
    let addrs = resolve(h, port)?;
    let life = crate::desk_proc::lifecycle();
    if !trusted() {
        return Err(NOT_TRUSTED.into());
    }
    listen(h, port, &addrs, &life)
}

/// bind the resolved addrs of h:port and start accepting
fn listen(h: &str, port: u16, addrs: &[SocketAddr], _life: &Life) -> Result<(u64, Arc<Srv>), String> {
    let l = TcpListener::bind(addrs).map_err(|e| listen_err(h, port, &e))?;
    l.set_nonblocking(true).map_err(|e| listen_err(h, port, &e))?;
    let la = l.local_addr().map_err(|e| listen_err(h, port, &e))?;
    let srv = Arc::new(Srv { closed: AtomicBool::new(false), st: Mutex::new(SrvSt::default()), cv: Condvar::new(), port: la.port(), address: la.ip().to_string() });
    let id = next_id();
    reg().lock().unwrap_or_else(|e| e.into_inner()).srvs.insert(id, srv.clone());
    let s = srv.clone();
    std::thread::Builder::new()
        .name("desk-net-accept".into())
        .spawn(move || {
            while !s.closed.load(Ordering::SeqCst) {
                match l.accept() {
                    Ok((sock, peer)) => {
                        #[cfg(test)]
                        accept_hook("accepted");
                        let _ = sock.set_nonblocking(false);
                        let _ = sock.set_nodelay(true);
                        let c = Arc::new(Conn { sock, st: Mutex::new(ConnSt::default()), cv: Condvar::new() });
                        let cid = next_id();
                        // P3 (PR #42): the closed check and the insert are one step under the reg
                        // lock, the lock close_all / close_srv mark `closed` under: a socket
                        // accepted across a close is destroyed here, never registered
                        let kept = {
                            let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
                            if s.closed.load(Ordering::SeqCst) {
                                false
                            } else {
                                r.conns.insert(cid, c.clone());
                                true
                            }
                        };
                        #[cfg(test)]
                        accept_hook("decided");
                        if !kept {
                            c.destroy();
                            break;
                        }
                        std::thread::spawn(move || pump(c));
                        s.st.lock().unwrap_or_else(|e| e.into_inner()).pending.push(Accepted { id: cid, remote_address: peer.ip().to_string(), remote_port: peer.port() });
                        s.cv.notify_all();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(25)),
                    Err(_) => std::thread::sleep(Duration::from_millis(100)),
                }
            }
            // the listener drops here: the port is free again once close() was seen
        })
        .map_err(|e| format!("EIO: listen thread: {e}"))?;
    Ok((id, srv))
}

pub fn srv(id: u64) -> Result<Arc<Srv>, String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).srvs.get(&id).cloned().ok_or_else(|| "ERR_SERVER_NOT_RUNNING".into())
}

pub fn conn(id: u64) -> Result<Arc<Conn>, String> {
    reg().lock().unwrap_or_else(|e| e.into_inner()).conns.get(&id).cloned().ok_or_else(|| "EBADF: no such socket".into())
}

pub fn close_srv(id: u64) {
    // P3: marked closed under the reg lock, the lock the accept thread inserts under
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = r.srvs.remove(&id) {
        s.close();
    }
}

pub fn forget_conn(id: u64) {
    if let Some(c) = reg().lock().unwrap_or_else(|e| e.into_inner()).conns.remove(&id) {
        c.destroy();
    }
}

/* flip OFF: every listener and socket plugins opened goes away.
   P3 (PR #42, Strix): every listener is marked closed INSIDE the reg lock, before the
   drain. The accept thread checks `closed` and inserts under that same lock, so a socket
   it accepted across this call is either in `conns` before the drain (and destroyed below)
   or sees `closed` and destroys itself — it can never be registered after the drain and
   outlive restricted mode (pump reading, socket commands addressing it by id). */
pub fn close_all(_life: &Life) {
    let (s, c) = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        for s in r.srvs.values() {
            s.closed.store(true, Ordering::SeqCst);
        }
        (r.srvs.drain().map(|(_, s)| s).collect::<Vec<_>>(), r.conns.drain().map(|(_, c)| c).collect::<Vec<_>>())
    };
    for x in s {
        x.close();
    }
    for x in c {
        x.destroy();
    }
}

/* P3 test probe: the accept thread calls it right after accept() ("accepted") and right
   after its insert-or-destroy decision ("decided"), so a test can run a trust-OFF at that
   exact point and wait for the decision, with no timing. */
#[cfg(test)]
type Hook = Box<dyn Fn(&'static str) + Send>;
#[cfg(test)]
static ACCEPT_HOOK: Mutex<Option<Hook>> = Mutex::new(None);
#[cfg(test)]
fn accept_hook(at: &'static str) {
    if let Some(h) = ACCEPT_HOOK.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        h(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desk_proc::{latin1_to_bytes, lifecycle, try_lifecycle};

    /// close_all drops EVERY listener: the tests of this module run one at a time
    static NET_TESTS: Mutex<()> = Mutex::new(());
    fn one() -> std::sync::MutexGuard<'static, ()> {
        NET_TESTS.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn desk_net_listen_accept_read_write_end_close() {
        let _one = one();
        let (sid, s) = lst("127.0.0.1", 0).unwrap();
        assert_eq!(s.address, "127.0.0.1");
        let mut cl = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
        let b = s.accept(Duration::from_secs(3));
        assert_eq!(b.conns.len(), 1, "{b:?}");
        assert_eq!(b.conns[0].remote_address, "127.0.0.1");
        let c = conn(b.conns[0].id).unwrap();
        cl.write_all(b"GET / HTTP/1.1\r\n\r\n\xff").unwrap();
        cl.shutdown(Shutdown::Write).unwrap();
        let mut got = vec![];
        for _ in 0..100 {
            let d = c.read(Duration::from_millis(50));
            got.extend(latin1_to_bytes(&d.data));
            if d.eof {
                break;
            }
        }
        assert_eq!(got, b"GET / HTTP/1.1\r\n\r\n\xff");
        c.write(b"HTTP/1.1 200 OK\r\n\r\nhi").unwrap();
        c.end();
        let mut resp = vec![];
        cl.read_to_end(&mut resp).unwrap();
        assert_eq!(resp, b"HTTP/1.1 200 OK\r\n\r\nhi");
        forget_conn(b.conns[0].id);
        assert!(conn(b.conns[0].id).is_err());
        close_srv(sid);
        assert!(s.accept(Duration::from_millis(10)).closed);
        assert!(srv(sid).is_err());
    }

    #[test]
    fn desk_net_port_in_use_is_eaddrinuse_and_close_frees_it() {
        let _one = one();
        let (sid, s) = lst("127.0.0.1", 0).unwrap();
        let e = lst("127.0.0.1", s.port).err().unwrap();
        assert!(e.starts_with("EADDRINUSE: listen 127.0.0.1:"), "{e}");
        close_srv(sid);
        let mut ok = false;
        for _ in 0..40 {
            if let Ok((id, _)) = lst("127.0.0.1", s.port) {
                close_srv(id);
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ok, "port not freed after close");
    }

    /* N5 (PR #41): a trust-OFF (flag write, close_all) arrives while a listen is between a
       trust check and its bind. B is that trust-OFF, run on another thread from INSIDE
       check number `at`: it takes the lifecycle lock without blocking if it can. P2 made
       listen_trusted check twice (an early unlocked check before the DNS lookup, the
       deciding one under the lock), so B is put into each check in turn. Wherever it lands,
       no listener may stay up: with the deciding check under the lock, B either runs
       before it (EPERM) or cannot get the lock and runs once listen_trusted returns, and
       its close_all closes the new listener. If the deciding check ran outside the lock,
       B would run between it and the bind, and that listener would stay up in restricted
       mode. No sleep: the order is forced by where B runs. */
    #[test]
    fn desk_net_n5_listener_cannot_stay_bound_after_trust_off() {
        static TRUSTED: AtomicBool = AtomicBool::new(true);
        let _one = one();
        let trust_off = |life: &Life| {
            TRUSTED.store(false, Ordering::SeqCst);
            close_all(life);
        };
        for at in 1..=2 {
            TRUSTED.store(true, Ordering::SeqCst);
            let mut n = 0;
            let mut b_ran = false;
            let got = listen_trusted(
                || {
                    let ok = TRUSTED.load(Ordering::SeqCst);
                    n += 1;
                    if n == at {
                        // B, now: only if the lock is free (other tests hold it for
                        // microseconds, so it is retried a bounded number of times; while
                        // this listen holds it, it stays held)
                        b_ran = std::thread::spawn(move || {
                            for _ in 0..10_000 {
                                if let Some(life) = try_lifecycle() {
                                    trust_off(&life);
                                    return true;
                                }
                                std::thread::yield_now();
                            }
                            false
                        })
                        .join()
                        .unwrap();
                    }
                    ok
                },
                "127.0.0.1",
                0,
            );
            if !b_ran {
                trust_off(&lifecycle()); // B waited for the lock: it runs after the register
            }
            match got {
                Err(e) => assert!(e.starts_with("EPERM"), "B in check {at}: {e}"),
                Ok((id, s)) => {
                    let bound = srv(id).is_ok();
                    let accepting = !s.accept(Duration::from_millis(10)).closed;
                    close_srv(id);
                    assert!(!bound && !accepting, "B in check {at}: a listener that passed its trust check stayed bound after trust-OFF (registered: {bound}, accepting: {accepting})");
                }
            }
        }
    }

    /* P2 (PR #42, Greptile): the host lookup (to_socket_addrs, DNS, seconds when slow) runs
       with the lifecycle lock NOT held — else a slow lookup holds up every trust-OFF,
       kill_all and spawn. The stub resolver asks, at the moment it runs, whether this
       thread holds the lock (desk_proc::held_here: set when a Life is taken, cleared when
       it drops — no timing involved). The bind and the register still run under it. And a
       restricted vault resolves nothing (a lookup it picks the name of is a way out). */
    #[test]
    fn desk_net_p2_host_lookup_does_not_hold_the_lifecycle_lock() {
        use crate::desk_proc::held_here;
        let _one = one();
        let mut held_in_lookup = None;
        let (id, s) = listen_trusted_via(
            |h, p| {
                held_in_lookup = Some(held_here());
                resolve(h, p)
            },
            || true,
            "127.0.0.1",
            0,
        )
        .expect("binds");
        close_srv(id);
        assert!(s.port != 0);
        assert_eq!(held_in_lookup, Some(false), "the host lookup ran while holding the lifecycle lock");
        let mut held_in_check = vec![];
        let (id, _) = listen_trusted_via(resolve, || { held_in_check.push(held_here()); true }, "127.0.0.1", 0).unwrap();
        close_srv(id);
        assert_eq!(held_in_check.last(), Some(&true), "the deciding trust check ran without the lifecycle lock");
        let mut looked_up = false;
        let e = listen_trusted_via(|h, p| { looked_up = true; resolve(h, p) }, || false, "example.invalid", 0).err().unwrap();
        assert!(e.starts_with("EPERM"), "{e}");
        assert!(!looked_up, "a restricted vault made the app resolve a host");
    }

    /* P3 (PR #42, Strix, CWE-362): a connection the accept thread took right before a
       trust-OFF. The hook runs the trust-OFF (close_all under the lifecycle lock) ON the
       accept thread right after accept() returned the socket — the exact window: past the
       loop's closed check, before the register. Then it reports the thread's
       insert-or-destroy decision over a channel. That socket must not be registered (no
       socket command can address it) and the client must see it closed. */
    #[test]
    fn desk_net_p3_conn_accepted_across_trust_off_does_not_survive() {
        use std::sync::mpsc::channel;
        let _one = one();
        let (decided, wait_decided) = channel::<()>();
        let decided = Mutex::new(decided);
        *ACCEPT_HOOK.lock().unwrap() = Some(Box::new(move |at: &'static str| match at {
            "accepted" => close_all(&lifecycle()),
            _ => decided.lock().unwrap().send(()).unwrap(),
        }));
        let (sid, s) = lst("127.0.0.1", 0).unwrap();
        let mut cl = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
        wait_decided.recv().unwrap();
        *ACCEPT_HOOK.lock().unwrap() = None;
        let left = reg().lock().unwrap().conns.len();
        let batch = s.accept(Duration::from_millis(10));
        close_srv(sid);
        close_all(&lifecycle());
        assert_eq!(left, 0, "a connection accepted across trust-OFF was registered after close_all drained the registry");
        assert!(batch.closed && batch.conns.is_empty(), "{batch:?}");
        // the client end: the socket was destroyed, so the read ends (EOF / reset), it does not hang
        cl.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut b = [0u8; 1];
        match cl.read(&mut b) {
            Ok(0) => {}
            Ok(n) => panic!("read {n} bytes from a socket that should be closed"),
            Err(e) => assert!(!matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut), "the accepted socket is still open: {e}"),
        }
    }

    fn lst(host: &str, port: u16) -> Result<(u64, Arc<Srv>), String> {
        listen(host, port, &resolve(host, port)?, &lifecycle())
    }
}
