// SPDX-License-Identifier: GPL-3.0-or-later
//! cp-unrestricted: the HTTP client behind obsidian's request / requestUrl for the
//! plugins of a trusted vault (stock parity: any host, no CORS, redirects followed).
//! Plain http:// over std TCP, HTTP/1.1 with Connection: close. https:// uses the
//! already-pinned reqwest+rustls client (Windows/macOS plugin-store dep); on other
//! targets it stays ENOTSUP until a TLS client is wired there. Bodies cross IPC as
//! latin1 strings (one char per byte, desk_proc::latin1_to_bytes), so binary payloads survive.
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const MAX_HEAD: usize = 64 * 1024;
const MAX_BODY: usize = 256 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(serde::Deserialize, Debug, Clone)]
pub struct Req {
    pub url: String,
    pub method: Option<String>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// latin1, one char per byte
    pub body: Option<String>,
}

#[derive(serde::Serialize, Debug)]
pub struct Resp {
    pub status: u16,
    /// names lowercased, in arrival order (duplicates kept)
    pub headers: Vec<(String, String)>,
    /// latin1, one char per byte
    pub body: String,
    /// the final URL after redirects
    pub url: String,
}

#[derive(Debug, PartialEq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Target {
    fn host_header(&self) -> String {
        let h = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        if self.port == 80 { h } else { format!("{h}:{}", self.port) }
    }
}

pub fn parse(url: &str) -> Result<Target, String> {
    let lower = url.trim_start().to_ascii_lowercase();
    if lower.starts_with("https://") {
        #[cfg(any(windows, target_os = "macos"))]
        {
            // Target is only for the plain-TCP path; HTTPS goes through request_tls.
            // Still parse host/path so callers can inspect redirects.
            let rest = &url.trim_start()[8..];
            return parse_authority(rest, 443);
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            return Err(format!("ENOTSUP: https requests are not available on this platform yet: {url}"));
        }
    }
    let rest = if lower.starts_with("http://") { &url.trim_start()[7..] } else {
        return Err(format!("EINVAL: unsupported URL (http(s):// only): {url}"));
    };
    parse_authority(rest, 80)
}

fn parse_authority(rest: &str, default_port: u16) -> Result<Target, String> {
    let url_hint = rest;
    let rest = rest.split('#').next().unwrap_or("");
    let (auth, path) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
    let auth = auth.rsplit('@').next().unwrap_or("");
    let (host, port) = if let Some(h) = auth.strip_prefix('[') {
        let (h, r) = h.split_once(']').ok_or("EINVAL: bad IPv6 host")?;
        let port = match r.strip_prefix(':') {
            Some(p) if !p.is_empty() => p.parse().map_err(|_| "EINVAL: bad port")?,
            _ => default_port,
        };
        (h.to_string(), port)
    } else {
        match auth.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() => (h.to_string(), p.parse().map_err(|_| "EINVAL: bad port")?),
            Some((h, _)) => (h.to_string(), default_port),
            None => (auth.to_string(), default_port),
        }
    };
    if host.is_empty() {
        return Err(format!("EINVAL: no host in {url_hint}"));
    }
    Ok(Target { host, port, path })
}

/// where a Location header points, relative to the URL it came from
pub fn resolve(base: &str, loc: &str) -> String {
    let l = loc.trim();
    let ll = l.to_ascii_lowercase();
    if ll.starts_with("http://") || ll.starts_with("https://") {
        return l.to_string();
    }
    let b = base.trim();
    let after = b.find("://").map(|i| i + 3).unwrap_or(0);
    let path_at = b[after..].find(['/', '?', '#']).map(|i| i + after).unwrap_or(b.len());
    let origin = &b[..path_at];
    if let Some(r) = l.strip_prefix("//") {
        return format!("{}//{r}", &b[..after.saturating_sub(2)]);
    }
    if l.starts_with('/') {
        return format!("{origin}{l}");
    }
    let path = b[path_at..].split(['?', '#']).next().unwrap_or("");
    let dir = match path.rfind('/') { Some(i) => &path[..=i], None => "/" };
    format!("{origin}{dir}{l}")
}

fn connect(t: &Target, deadline: Instant) -> Result<TcpStream, String> {
    let addrs = (t.host.as_str(), t.port).to_socket_addrs().map_err(|e| format!("ENOTFOUND: {} ({e})", t.host))?;
    let mut last = format!("ENOTFOUND: {}", t.host);
    for a in addrs {
        match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
            Ok(s) => {
                let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
                let _ = s.set_read_timeout(Some(left));
                let _ = s.set_write_timeout(Some(left));
                return Ok(s);
            }
            Err(e) => last = format!("ECONNREFUSED: {}:{} ({e})", t.host, t.port),
        }
    }
    Err(last)
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// decode a chunked body; None while it is incomplete
pub fn dechunk(b: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let e = find(&b[i..], b"\r\n")? + i;
        let line = std::str::from_utf8(&b[i..e]).ok()?;
        let size = usize::from_str_radix(line.split(';').next()?.trim(), 16).ok()?;
        i = e + 2;
        if size == 0 {
            return Some(out);   // trailers are not read
        }
        if b.len() < i + size + 2 {
            return None;
        }
        out.extend_from_slice(&b[i..i + size]);
        i += size + 2;
    }
}

fn once(t: &Target, method: &str, headers: &[(String, String)], body: &[u8], deadline: Instant) -> Result<(u16, Vec<(String, String)>, Vec<u8>), String> {
    let mut s = connect(t, deadline)?;
    let mut head = format!("{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n", t.path, t.host_header());
    let has = |n: &str| headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(n));
    for (k, v) in headers {
        let lk = k.to_ascii_lowercase();
        if matches!(lk.as_str(), "host" | "connection" | "content-length" | "transfer-encoding") {
            continue;
        }
        if k.contains(['\r', '\n', ':']) || v.contains(['\r', '\n']) {
            return Err(format!("EINVAL: bad header {k:?}"));
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if !has("user-agent") {
        head.push_str("User-Agent: opensidian\r\n");
    }
    if !has("accept") {
        head.push_str("Accept: */*\r\n");
    }
    if !body.is_empty() || !matches!(method, "GET" | "HEAD" | "OPTIONS" | "DELETE") {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).and_then(|_| s.write_all(body)).map_err(|e| format!("ECONNRESET: write ({e})"))?;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut parsed: Option<(usize, u16, Vec<(String, String)>)> = None;
    loop {
        if let Some((start, status, ref hs)) = parsed {
            let no_body = method == "HEAD" || status == 204 || status == 304 || (100..200).contains(&status);
            let rest = &buf[start..];
            if no_body {
                return Ok((status, hs.clone(), Vec::new()));
            }
            let chunked = hs.iter().any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
            if chunked {
                if let Some(d) = dechunk(rest) {
                    return Ok((status, hs.clone(), d));
                }
            } else if let Some(n) = hs.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.trim().parse::<usize>().ok()) {
                if rest.len() >= n {
                    return Ok((status, hs.clone(), rest[..n].to_vec()));
                }
            }
        }
        let n = match s.read(&mut chunk) {
            Ok(n) => n,
            Err(e) if Instant::now() >= deadline => return Err(format!("ETIMEDOUT: {e}")),
            Err(e) => return Err(format!("ECONNRESET: read ({e})")),
        };
        if n == 0 {
            return match parsed {
                Some((start, status, hs)) => {
                    let chunked = hs.iter().any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
                    if chunked { Err("ECONNRESET: chunked body cut short".into()) } else { Ok((status, hs, buf[start..].to_vec())) }
                }
                None => Err("ECONNRESET: connection closed before a response head".into()),
            };
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_HEAD + MAX_BODY {
            return Err("E2BIG: response too large".into());
        }
        if parsed.is_none() {
            if let Some(e) = find(&buf, b"\r\n\r\n") {
                let text = String::from_utf8_lossy(&buf[..e]).into_owned();
                let mut lines = text.split("\r\n");
                let sl = lines.next().unwrap_or("");
                let status: u16 = sl.split_whitespace().nth(1).and_then(|x| x.parse().ok()).ok_or_else(|| format!("EPROTO: bad status line {sl:?}"))?;
                let hs = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
                parsed = Some((e + 4, status, hs));
            } else if buf.len() > MAX_HEAD {
                return Err("EPROTO: response head too large".into());
            }
        }
    }
}

pub fn request(r: &Req) -> Result<Resp, String> {
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    let mut url = r.url.clone();
    let mut method = r.method.clone().unwrap_or_else(|| "GET".into()).to_ascii_uppercase();
    let mut body = r.body.as_deref().map(crate::desk_proc::latin1_to_bytes).unwrap_or_default();
    let mut headers = r.headers.clone();
    for _ in 0..=MAX_REDIRECTS {
        if url.trim_start().to_ascii_lowercase().starts_with("https://") {
            #[cfg(any(windows, target_os = "macos"))]
            {
                // reqwest follows redirects itself; hand it the current method/body/headers
                return request_tls(&url, &method, &headers, &body);
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            {
                let _ = (&method, &headers, &body, deadline);
                return Err(format!("ENOTSUP: https requests are not available on this platform yet: {url}"));
            }
        }
        let t = parse(&url)?;
        let (status, hs, b) = once(&t, &method, &headers, &body, deadline)?;
        let loc = hs.iter().find(|(k, _)| k == "location").map(|(_, v)| v.clone());
        match (status, loc) {
            (301 | 302 | 303 | 307 | 308, Some(l)) => {
                if status == 303 || ((status == 301 || status == 302) && method == "POST") {
                    method = "GET".into();
                    body.clear();
                    headers.retain(|(k, _)| !k.eq_ignore_ascii_case("content-type"));
                }
                url = resolve(&url, &l);
                // http -> https redirect: next loop iteration takes the TLS branch
            }
            _ => return Ok(Resp { status, headers: hs, body: crate::desk_proc::bytes_to_latin1(&b), url }),
        }
    }
    Err(format!("ELOOP: more than {MAX_REDIRECTS} redirects"))
}

/// HTTPS via the already-locked reqwest+rustls (plugin-store). Same Resp shape as plain http.
#[cfg(any(windows, target_os = "macos"))]
fn request_tls(url: &str, method: &str, headers: &[(String, String)], body: &[u8]) -> Result<Resp, String> {
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .timeout(TOTAL_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent("opensidian")
        .build()
        .map_err(|e| format!("EIO: https client: {e}"))?;
    let mut rb = client.request(
        method.parse().map_err(|_| format!("EINVAL: bad method {method}"))?,
        url,
    );
    for (k, v) in headers {
        let lk = k.to_ascii_lowercase();
        if matches!(lk.as_str(), "host" | "connection" | "content-length" | "transfer-encoding") {
            continue;
        }
        rb = rb.header(k.as_str(), v.as_str());
    }
    if !body.is_empty() {
        rb = rb.body(body.to_vec());
    }
    let res = rb.send().map_err(|e| format!("EIO: https {url}: {e}"))?;
    let status = res.status().as_u16();
    let final_url = res.url().to_string();
    let mut hs = Vec::new();
    for (k, v) in res.headers().iter() {
        hs.push((k.as_str().to_ascii_lowercase(), v.to_str().unwrap_or("").to_string()));
    }
    let bytes = res.bytes().map_err(|e| format!("EIO: https body: {e}"))?;
    if bytes.len() > MAX_BODY {
        return Err("E2BIG: response too large".into());
    }
    Ok(Resp {
        status,
        headers: hs,
        body: crate::desk_proc::bytes_to_latin1(&bytes),
        url: final_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn parse_targets() {
        assert_eq!(parse("http://127.0.0.1:23119/better-bibtex/cayw?probe=true").unwrap(), Target { host: "127.0.0.1".into(), port: 23119, path: "/better-bibtex/cayw?probe=true".into() });
        assert_eq!(parse("http://example.org").unwrap(), Target { host: "example.org".into(), port: 80, path: "/".into() });
        assert_eq!(parse("http://u:p@[::1]:8080?x=1#f").unwrap(), Target { host: "::1".into(), port: 8080, path: "/?x=1".into() });
        #[cfg(any(windows, target_os = "macos"))]
        assert_eq!(parse("https://example.org/x").unwrap(), Target { host: "example.org".into(), port: 443, path: "/x".into() });
        #[cfg(not(any(windows, target_os = "macos")))]
        assert!(parse("https://example.org/").unwrap_err().starts_with("ENOTSUP"));
        assert!(parse("file:///etc/passwd").unwrap_err().starts_with("EINVAL"));
    }

    #[test]
    fn resolve_locations() {
        assert_eq!(resolve("http://h:1/a/b?q", "/c"), "http://h:1/c");
        assert_eq!(resolve("http://h:1/a/b?q", "c"), "http://h:1/a/c");
        assert_eq!(resolve("http://h:1/a/b", "http://x/y"), "http://x/y");
        assert_eq!(resolve("http://h:1/a", "//x/y"), "http://x/y");
    }

    #[test]
    fn dechunk_bodies() {
        assert_eq!(dechunk(b"3\r\nabc\r\n2;x=y\r\nde\r\n0\r\n\r\n").unwrap(), b"abcde");
        assert!(dechunk(b"3\r\nab").is_none());
    }

    fn serve(resps: Vec<Vec<u8>>) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for r in resps {
                let (mut c, _) = l.accept().unwrap();
                let mut b = Vec::new();
                let mut k = [0u8; 4096];
                loop {
                    let n = c.read(&mut k).unwrap();
                    b.extend_from_slice(&k[..n]);
                    if let Some(e) = find(&b, b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&b[..e]).to_string();
                        let cl = head.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                        if b.len() >= e + 4 + cl { break; }
                    }
                    if n == 0 { break; }
                }
                seen.push(String::from_utf8_lossy(&b).into_owned());
                c.write_all(&r).unwrap();
            }
            seen
        });
        (port, h)
    }

    #[test]
    fn post_follows_a_redirect_and_reads_binary_bodies() {
        let (port, h) = serve(vec![
            b"HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n".to_vec(),
            [b"HTTP/1.1 200 OK\r\nX-A: 1\r\nContent-Length: 3\r\n\r\n".as_slice(), &[0u8, 0xff, 7]].concat(),
        ]);
        let r = request(&Req { url: format!("http://127.0.0.1:{port}/first"), method: Some("post".into()), headers: vec![("Content-Type".into(), "application/json".into())], body: Some("{}".into()) }).unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(crate::desk_proc::latin1_to_bytes(&r.body), vec![0u8, 0xff, 7]);
        assert!(r.headers.contains(&("x-a".into(), "1".into())));
        assert_eq!(r.url, format!("http://127.0.0.1:{port}/next"));
        let seen = h.join().unwrap();
        assert!(seen[0].starts_with("POST /first HTTP/1.1\r\n") && seen[0].ends_with("\r\n\r\n{}"));
        assert!(seen[1].starts_with("GET /next HTTP/1.1\r\n") && !seen[1].to_ascii_lowercase().contains("content-type"));
    }

    #[test]
    fn chunked_and_error_statuses_come_back_as_responses() {
        let (port, h) = serve(vec![b"HTTP/1.1 404 Not Found\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nnope\r\n0\r\n\r\n".to_vec()]);
        let r = request(&Req { url: format!("http://127.0.0.1:{port}/x"), method: None, headers: vec![], body: None }).unwrap();
        assert_eq!((r.status, r.body.as_str()), (404, "nope"));
        h.join().unwrap();
        assert!(request(&Req { url: "http://127.0.0.1:1/".into(), method: None, headers: vec![], body: None }).unwrap_err().starts_with("ECONNREFUSED"));
    }
}
