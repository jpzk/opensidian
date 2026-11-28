// SPDX-License-Identifier: GPL-3.0-or-later
/* http_win — the plugin store's HTTPS client on Windows (plugin_store::http_get).

   Linux uses libsoup, the library webkit already links. The Windows equivalent
   of "what the platform already ships" is WinHTTP: TLS is Schannel, the trust
   store is the system's, the proxy is the one Windows is configured with. No
   TLS stack and no CA bundle enter the binary.

   The rules are plugin_store's, not this file's, and they are the same ones:
     - every URL goes through plugin_store::check_url BEFORE it is requested
       (https only, exact host allowlist, no credentials, default port);
     - WinHTTP never follows a redirect by itself (WINHTTP_DISABLE_REDIRECTS):
       a 3xx comes back to us, its Location goes through next_hop -> check_url,
       at most MAX_REDIRECTS times;
     - no cookies, no automatic authentication: nothing of the user's rides
       along on a request to GitHub;
     - a per-I/O timeout, a whole-request deadline, and a byte cap that is
       enforced on the declared length and again on the bytes actually read.
   This file only speaks HTTP. A handle is closed by `H`'s Drop, in every path. */

use crate::plugin_store::{check_url, mirror_base, next_hop, wire_url, DEADLINE, IO_TIMEOUT_S, MAX_REDIRECTS};
use std::ffi::c_void;
use std::time::Instant;
use windows_sys::Win32::Networking::WinHttp::*;

/// an HINTERNET, closed on drop
struct H(*mut c_void);
impl Drop for H {
    fn drop(&mut self) {
        // SAFETY: a handle WinHTTP gave us, closed exactly once
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last(what: &str, step: &str, host: &str) -> String {
    format!("{what}: {host} — {step}: {}", std::io::Error::last_os_error())
}

/// a handle or the error of the call that should have produced it
fn handle(h: *mut c_void, what: &str, step: &str, host: &str) -> Result<H, String> {
    if h.is_null() { Err(last(what, step, host)) } else { Ok(H(h)) }
}

/// a numeric response header (status, content length), None when absent
fn header_u32(req: &H, which: u32) -> Option<u32> {
    let (mut v, mut len) = (0u32, 4u32);
    // SAFETY: a 4-byte buffer for a header asked as a number
    let ok = unsafe { WinHttpQueryHeaders(req.0, which | WINHTTP_QUERY_FLAG_NUMBER, std::ptr::null(), &mut v as *mut u32 as *mut c_void, &mut len, std::ptr::null_mut()) };
    (ok != 0).then_some(v)
}

fn location(req: &H) -> Option<String> {
    let mut buf = vec![0u16; 4096];
    let mut len = (buf.len() * 2) as u32; // in BYTES, in and out
    // SAFETY: the buffer is `len` bytes long
    let ok = unsafe { WinHttpQueryHeaders(req.0, WINHTTP_QUERY_LOCATION, std::ptr::null(), buf.as_mut_ptr() as *mut c_void, &mut len, std::ptr::null_mut()) };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).min(buf.len())]))
}

pub fn get(logical: &str, cap: u64, what: &str) -> Result<Vec<u8>, String> {
    let base = mirror_base();
    let mut cur = check_url(logical)?;
    let agent = wide(concat!("opensidian/", env!("CARGO_PKG_VERSION"), " plugin-browser"));
    // SAFETY (every call below): NUL-terminated UTF-16 strings that outlive the
    // call, handles checked for null before use, buffers of the stated length
    let sess = handle(
        unsafe { WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, std::ptr::null(), std::ptr::null(), 0) },
        what,
        "open",
        "winhttp",
    )?;
    let ms = (IO_TIMEOUT_S * 1000) as i32;
    if unsafe { WinHttpSetTimeouts(sess.0, ms, ms, ms, ms) } == 0 {
        return Err(last(what, "timeouts", "winhttp"));
    }
    let deadline = Instant::now() + DEADLINE;
    for _ in 0..=MAX_REDIRECTS {
        // what is really requested: the logical URL, or its path under the
        // loopback fixture mirror (tests), which may be plain http
        let wire = url::Url::parse(&wire_url(&cur, base.as_ref())).map_err(|_| format!("{what}: bad request URL {cur}"))?;
        let host = cur.host_str().unwrap_or("?").to_string();
        let secure = wire.scheme() == "https";
        let whost = wide(wire.host_str().ok_or_else(|| format!("{what}: bad request URL {cur}"))?);
        let port = wire.port_or_known_default().unwrap_or(443);
        let conn = handle(unsafe { WinHttpConnect(sess.0, whost.as_ptr(), port, 0) }, what, "connect", &host)?;
        let mut object = wire.path().to_string();
        if let Some(q) = wire.query() {
            object.push('?');
            object.push_str(q);
        }
        let (verb, object) = (wide("GET"), wide(&object));
        let req = handle(
            unsafe { WinHttpOpenRequest(conn.0, verb.as_ptr(), object.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null(), if secure { WINHTTP_FLAG_SECURE } else { 0 }) },
            what,
            "request",
            &host,
        )?;
        let off: u32 = WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION;
        if unsafe { WinHttpSetOption(req.0, WINHTTP_OPTION_DISABLE_FEATURE, &off as *const u32 as *const c_void, 4) } == 0 {
            // never go on with a client that would follow redirects by itself
            return Err(last(what, "disable redirects", &host));
        }
        if cur.host_str() == Some("api.github.com") {
            let h = wide("Accept: application/vnd.github+json");
            if unsafe { WinHttpAddRequestHeaders(req.0, h.as_ptr(), u32::MAX, WINHTTP_ADDREQ_FLAG_ADD) } == 0 {
                return Err(last(what, "header", &host));
            }
        }
        if unsafe { WinHttpSendRequest(req.0, std::ptr::null(), 0, std::ptr::null(), 0, 0, 0) } == 0 {
            return Err(last(what, "send", &host));
        }
        if unsafe { WinHttpReceiveResponse(req.0, std::ptr::null_mut()) } == 0 {
            return Err(last(what, "response", &host));
        }
        let code = header_u32(&req, WINHTTP_QUERY_STATUS_CODE).ok_or_else(|| last(what, "status", &host))?;
        if (300..400).contains(&code) && code != 304 {
            let loc = location(&req).ok_or_else(|| format!("{what}: HTTP {code} without a Location"))?;
            cur = next_hop(&cur, &loc, base.as_ref()).map_err(|e| format!("{what}: redirect {e}"))?;
            continue;
        }
        if code != 200 {
            let hint = if (code == 403 || code == 429) && cur.host_str() == Some("api.github.com") {
                " (GitHub's anonymous API rate limit; try again later)"
            } else {
                ""
            };
            return Err(format!("{what}: HTTP {code} from {host}{hint}"));
        }
        let declared = header_u32(&req, WINHTTP_QUERY_CONTENT_LENGTH).unwrap_or(0) as u64;
        if declared > cap {
            return Err(format!("refused: {what} is {declared} bytes, over the {cap}-byte cap"));
        }
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            if Instant::now() > deadline {
                return Err(format!("{what}: timed out"));
            }
            let mut n = 0u32;
            if unsafe { WinHttpReadData(req.0, buf.as_mut_ptr() as *mut c_void, buf.len() as u32, &mut n) } == 0 {
                return Err(last(what, "read", &host));
            }
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
            if out.len() as u64 > cap {
                return Err(format!("refused: {what} is larger than the {cap}-byte cap"));
            }
        }
        return Ok(out);
    }
    Err(format!("{what}: more than {MAX_REDIRECTS} redirects"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// a one-thread HTTP/1.1 fixture on a loopback port: `route(path)` answers
    /// (status, extra headers, body); it serves `n` requests and stops
    fn serve(n: usize, route: impl Fn(&str) -> (u16, String, Vec<u8>) + Send + 'static) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let t = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..n {
                let (mut s, _) = l.accept().unwrap();
                let mut req = Vec::new();
                let mut b = [0u8; 1024];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut b) {
                        Ok(0) | Err(_) => break,
                        Ok(k) => req.extend_from_slice(&b[..k]),
                    }
                }
                let text = String::from_utf8_lossy(&req).to_string();
                let path = text.split_whitespace().nth(1).unwrap_or("").to_string();
                let (code, headers, body) = route(&path);
                let head = format!("HTTP/1.1 {code} X\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n", body.len());
                let _ = s.write_all(head.as_bytes()).and_then(|_| s.write_all(&body));
                seen.push(text);
            }
            seen
        });
        (base, t)
    }

    /// The mirror env is process-global and cargo runs tests as threads of one
    /// process, so every case that needs it lives in this ONE test.
    #[test]
    fn http_win_fetches_follows_checked_redirects_and_enforces_the_rules() {
        let (base, t) = serve(6, |path| match path {
            "/raw.githubusercontent.com/o/r/main/ok.txt" => (200, String::new(), b"hello".to_vec()),
            // a redirect to an allowlisted host: followed, by us
            "/raw.githubusercontent.com/o/r/main/moved" => (302, "Location: https://raw.githubusercontent.com/o/r/main/ok.txt\r\n".into(), vec![]),
            // a redirect off the allowlist: never requested
            "/raw.githubusercontent.com/o/r/main/evil" => (302, "Location: https://evil.example/x\r\n".into(), vec![]),
            "/raw.githubusercontent.com/o/r/main/big" => (200, String::new(), vec![b'x'; 5000]),
            "/api.github.com/repos/o/r" => (403, String::new(), vec![]),
            _ => (404, String::new(), vec![]),
        });
        std::env::set_var(crate::plugin_store::BASE_ENV, &base);
        let u = |p: &str| format!("https://raw.githubusercontent.com/o/r/main/{p}");
        let ok = get(&u("ok.txt"), 1 << 20, "t");
        let moved = get(&u("moved"), 1 << 20, "t"); // 2 requests
        let evil = get(&u("evil"), 1 << 20, "t");
        let big = get(&u("big"), 1000, "t");
        let api = get("https://api.github.com/repos/o/r", 1 << 20, "t");
        std::env::remove_var(crate::plugin_store::BASE_ENV);
        let seen = t.join().unwrap();
        assert_eq!(ok.as_deref(), Ok(&b"hello"[..]));
        assert_eq!(moved.as_deref(), Ok(&b"hello"[..]), "a redirect to an allowed host is followed");
        let e = evil.unwrap_err();
        assert!(e.contains("redirect") && e.contains("not on the plugin store allowlist"), "{e}");
        assert!(big.unwrap_err().contains("over the 1000-byte cap"));
        let e = api.unwrap_err();
        assert!(e.contains("HTTP 403") && e.contains("rate limit"), "{e}");
        assert_eq!(seen.len(), 6, "exactly the requests we made: WinHTTP followed nothing by itself");
        assert!(seen.iter().all(|r| r.contains("plugin-browser")), "user agent");
        assert!(seen.iter().all(|r| !r.to_ascii_lowercase().contains("cookie:") && !r.to_ascii_lowercase().contains("authorization:")));
        assert!(seen[5].contains("Accept: application/vnd.github+json"), "{}", seen[5]);
        // and the egress check runs before any socket: nothing was listening for these
        assert!(get("http://raw.githubusercontent.com/x", 10, "t").unwrap_err().contains("not https"));
        assert!(get("https://example.org/x", 10, "t").unwrap_err().contains("allowlist"));
    }
}
