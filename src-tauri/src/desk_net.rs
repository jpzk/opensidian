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
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
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

/* N5 (PR #41): what desk_net_listen runs. The lifecycle lock (desk_proc::lifecycle, the
   one plugins_set holds across a trust-OFF: flag write, kill_all, close_all) is taken
   BEFORE the trust check and held through the bind and the register, so a trust-OFF
   either lands before the check (EPERM, nothing bound) or after the register (its
   close_all closes this listener). listen and close_all demand a &Life for that. */
pub fn listen_trusted(trusted: impl FnOnce() -> bool, host: &str, port: u16) -> Result<(u64, Arc<Srv>), String> {
    let life = crate::desk_proc::lifecycle();
    if !trusted() {
        return Err("EPERM: restricted mode, plugins cannot open servers".into());
    }
    listen(host, port, &life)
}

/// bind host:port (Node: no host = all interfaces; "localhost" resolves) and start accepting
fn listen(host: &str, port: u16, _life: &Life) -> Result<(u64, Arc<Srv>), String> {
    let h = if host.is_empty() { "0.0.0.0" } else { host };
    let addrs: Vec<_> = (h, port).to_socket_addrs().map_err(|e| format!("ENOTFOUND: listen {h}: {e}"))?.collect();
    let l = TcpListener::bind(&addrs[..]).map_err(|e| listen_err(h, port, &e))?;
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
                        let _ = sock.set_nonblocking(false);
                        let _ = sock.set_nodelay(true);
                        let c = Arc::new(Conn { sock, st: Mutex::new(ConnSt::default()), cv: Condvar::new() });
                        let cid = next_id();
                        reg().lock().unwrap_or_else(|e| e.into_inner()).conns.insert(cid, c.clone());
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
    if let Some(s) = reg().lock().unwrap_or_else(|e| e.into_inner()).srvs.remove(&id) {
        s.close();
    }
}

pub fn forget_conn(id: u64) {
    if let Some(c) = reg().lock().unwrap_or_else(|e| e.into_inner()).conns.remove(&id) {
        c.destroy();
    }
}

/// flip OFF: every listener and socket plugins opened goes away
pub fn close_all(_life: &Life) {
    let (s, c) = {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        (r.srvs.drain().map(|(_, s)| s).collect::<Vec<_>>(), r.conns.drain().map(|(_, c)| c).collect::<Vec<_>>())
    };
    for x in s {
        x.close();
    }
    for x in c {
        x.destroy();
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
        let (sid, s) = listen("127.0.0.1", 0, &lifecycle()).unwrap();
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
        let (sid, s) = listen("127.0.0.1", 0, &lifecycle()).unwrap();
        let e = listen("127.0.0.1", s.port, &lifecycle()).err().unwrap();
        assert!(e.starts_with("EADDRINUSE: listen 127.0.0.1:"), "{e}");
        close_srv(sid);
        let mut ok = false;
        for _ in 0..40 {
            if let Ok((id, _)) = listen("127.0.0.1", s.port, &lifecycle()) {
                close_srv(id);
                ok = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(ok, "port not freed after close");
    }

    /* N5 (PR #41): a trust-OFF (flag write, close_all) arrives while a listen is between
       its trust check and its bind. B is that trust-OFF, run on another thread from INSIDE
       the check: it takes the lifecycle lock without blocking if it can. With the fix the
       listen holds the lock there, so B cannot: it runs for real once listen_trusted
       returns, and its close_all closes the new listener. If the check ran outside the
       lock, B would run first, and the listener bound after it would stay up in
       restricted mode. No sleep: the order is forced by where B runs. */
    #[test]
    fn desk_net_n5_listener_cannot_stay_bound_after_trust_off() {
        static TRUSTED: AtomicBool = AtomicBool::new(true);
        let _one = one();
        TRUSTED.store(true, Ordering::SeqCst);
        let trust_off = |life: &Life| {
            TRUSTED.store(false, Ordering::SeqCst);
            close_all(life);
        };
        let mut b_ran_inside = false;
        let got = listen_trusted(
            || {
                let ok = TRUSTED.load(Ordering::SeqCst);
                // B, now: only if the lock is free (other tests hold it for microseconds,
                // so it is retried a bounded number of times; while held, it stays held)
                b_ran_inside = std::thread::spawn(move || {
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
                ok
            },
            "127.0.0.1",
            0,
        );
        if !b_ran_inside {
            trust_off(&lifecycle()); // B waited for the lock: it runs after the register
        }
        let (id, s) = got.expect("the check passed before the trust-OFF");
        let bound = srv(id).is_ok();
        let accepting = !s.accept(Duration::from_millis(10)).closed;
        close_srv(id);
        assert!(!bound && !accepting, "a listener that passed its trust check stayed bound after trust-OFF (registered: {bound}, accepting: {accepting})");
        assert!(!b_ran_inside, "the trust-OFF ran between the listen's trust check and its bind");
    }
}
