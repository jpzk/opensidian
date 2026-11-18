// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Blocking `/v1` client over std `TcpStream`, one request per connection
//! (`Connection: close`). TLS is rustls 1.3-only on the AWS-LC FIPS provider
//! (the same pinned stack the server uses); the config is refused unless
//! rustls reports FIPS mode. Plain `http://` is accepted only for a loopback
//! host, matching the server's own rule (spec §5 TLS note).
//!
//! Response bodies are bounded ([`MAX_RESPONSE`]) so a hostile server cannot
//! make the client allocate without limit.

use std::io::{Read, Write};
use std::net::{IpAddr, TcpStream, ToSocketAddrs};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sync_proto::api::{
    object_path, DestroyChallenge, DestroyConfirm, DestroyReceipt, DestroyRequest, DestroyScope,
    EnrollRequest, EnrollResponse, ErrorBody, Health, ListQuery, ListResponse, MissingRequest,
    MissingResponse, ObjKind, DEFAULT_MAX_CONTROL_BODY, DEFAULT_MAX_OBJECT, PROTO_HEADER,
};
use sync_proto::Id16;

use super::{Fetched, PutOutcome, Range, Result, Tokens, Transport, TransportError};

/// Largest response body accepted (max object + slack for error JSON).
pub const MAX_RESPONSE: u64 = DEFAULT_MAX_OBJECT + DEFAULT_MAX_CONTROL_BODY as u64;
/// Largest response header block.
pub const MAX_HEAD: usize = 16 * 1024;
/// TCP connect timeout.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Read/write timeout of a call without long-poll.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Where trusted TLS roots come from.
#[derive(Debug, Clone)]
pub enum Roots {
    /// The OS bundle: `$SSL_CERT_FILE`, else the usual distro paths.
    System,
    /// Exactly these PEM certificates (self-hosted CA / pinning).
    Pem(Vec<u8>),
}

/// OS CA bundle locations tried by [`Roots::System`] (Debian/Ubuntu/Arch,
/// Fedora/RHEL, Alpine/macOS-brew, openSUSE).
pub const SYSTEM_BUNDLES: &[&str] = &[
    "/etc/ssl/certs/ca-certificates.crt",
    "/etc/pki/tls/certs/ca-bundle.crt",
    "/etc/ssl/cert.pem",
    "/etc/ssl/ca-bundle.pem",
];

/// Parsed base URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUrl {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Path prefix without trailing slash ("" or "/sync").
    pub prefix: String,
}

impl BaseUrl {
    /// `http(s)://host[:port][/prefix]`. No userinfo, query or fragment.
    /// `http://` only for `localhost` / loopback IPs.
    pub fn parse(url: &str) -> Result<BaseUrl> {
        let bad = |m: &str| TransportError::Config(format!("server URL: {m}"));
        let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            return Err(bad("must start with https:// (or http:// for loopback)"));
        };
        if rest.contains(['?', '#', '@', ' ', '\\']) || rest.chars().any(|c| c.is_control()) {
            return Err(bad("no userinfo, query, fragment, spaces or backslashes"));
        }
        let (auth, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        let (host, port) = if let Some(r) = auth.strip_prefix('[') {
            let (h, after) = r.split_once(']').ok_or_else(|| bad("unclosed [ipv6]"))?;
            h.parse::<std::net::Ipv6Addr>().map_err(|_| bad("bad ipv6 literal"))?;
            (h.to_string(), after.strip_prefix(':'))
        } else {
            match auth.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p)),
                None => (auth.to_string(), None),
            }
        };
        if host.is_empty() {
            return Err(bad("empty host"));
        }
        let port = match port {
            Some(p) => p.parse::<u16>().ok().filter(|p| *p != 0).ok_or_else(|| bad("bad port"))?,
            None if https => 443,
            None => 80,
        };
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
        if !https && !loopback {
            return Err(bad("plain http:// is only allowed to a loopback host; use https://"));
        }
        let prefix = path.trim_end_matches('/').to_string();
        if prefix.split('/').any(|s| s == ".." || s == ".") {
            return Err(bad("dot segments in path"));
        }
        Ok(BaseUrl {
            https,
            host,
            port,
            prefix,
        })
    }

    fn host_header(&self) -> String {
        let h = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        match (self.https, self.port) {
            (true, 443) | (false, 80) => h,
            _ => format!("{h}:{}", self.port),
        }
    }
}

/// TLS 1.3 client config on the FIPS provider; Err unless rustls is in FIPS mode.
pub fn client_config(roots: &Roots) -> Result<Arc<ClientConfig>> {
    let pem = match roots {
        Roots::Pem(p) => p.clone(),
        Roots::System => load_system_bundle()?,
    };
    let mut store = RootCertStore::empty();
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&pem)
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| TransportError::Config(format!("CA PEM: {e}")))?;
    let (added, _ignored) = store.add_parsable_certificates(certs);
    if added == 0 {
        return Err(TransportError::Config("no usable CA certificate".into()));
    }
    let mut cfg = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::default_fips_provider()))
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| TransportError::Config(format!("tls versions: {e}")))?
        .with_root_certificates(store)
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    if !cfg.fips() {
        return Err(TransportError::Config("rustls client config is not in FIPS mode".into()));
    }
    Ok(Arc::new(cfg))
}

fn load_system_bundle() -> Result<Vec<u8>> {
    let env = std::env::var_os("SSL_CERT_FILE").map(std::path::PathBuf::from);
    for p in env.into_iter().chain(SYSTEM_BUNDLES.iter().map(std::path::PathBuf::from)) {
        if let Ok(b) = std::fs::read(&p) {
            return Ok(b);
        }
    }
    Err(TransportError::Config("no system CA bundle found (set SSL_CERT_FILE)".into()))
}

/// The `/v1` HTTP client. One instance per server; tokens per vault are set
/// with [`HttpTransport::set_tokens`] after enrollment / bootstrap.
pub struct HttpTransport {
    base: BaseUrl,
    tls: Option<(Arc<ClientConfig>, ServerName<'static>)>,
    tokens: RwLock<Tokens>,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport").field("base", &self.base).finish_non_exhaustive()
    }
}

enum Auth {
    None,
    Read,
    Write,
}

struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.as_str())
    }
}

impl HttpTransport {
    pub fn new(url: &str, roots: Roots) -> Result<HttpTransport> {
        let base = BaseUrl::parse(url)?;
        let tls = if base.https {
            let name = ServerName::try_from(base.host.as_str())
                .map(|n| n.to_owned())
                .map_err(|_| TransportError::Config("server URL: host is not a valid TLS name".into()))?;
            Some((client_config(&roots)?, name))
        } else {
            None
        };
        Ok(HttpTransport {
            base,
            tls,
            tokens: RwLock::new(Tokens::default()),
        })
    }

    pub fn base(&self) -> &BaseUrl {
        &self.base
    }

    pub fn set_tokens(&self, t: Tokens) {
        *self.tokens.write().unwrap_or_else(|e| e.into_inner()) = t;
    }

    fn token(&self, a: &Auth) -> Result<Option<String>> {
        let t = self.tokens.read().unwrap_or_else(|e| e.into_inner());
        match a {
            Auth::None => Ok(None),
            // A write token also reads (server scope order Read < Write).
            Auth::Read => t
                .read
                .clone()
                .or_else(|| t.write.clone())
                .map(Some)
                .ok_or_else(|| TransportError::Config("no read token for this vault".into())),
            Auth::Write => t
                .write
                .clone()
                .map(Some)
                .ok_or_else(|| TransportError::Config("no write token for this vault".into())),
        }
    }

    fn connect(&self, timeout: Duration) -> Result<TcpStream> {
        let addrs = (self.base.host.as_str(), self.base.port)
            .to_socket_addrs()
            .map_err(|e| TransportError::Io(format!("resolve {}: {e}", self.base.host)))?;
        let mut last = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
                Ok(s) => {
                    s.set_read_timeout(Some(timeout)).map_err(io)?;
                    s.set_write_timeout(Some(timeout)).map_err(io)?;
                    let _ = s.set_nodelay(true);
                    return Ok(s);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(TransportError::Io(format!(
            "connect {}:{}: {}",
            self.base.host,
            self.base.port,
            last.map(|e| e.to_string()).unwrap_or_else(|| "no address".into())
        )))
    }

    fn call(
        &self,
        method: &str,
        path_q: &str,
        auth: Auth,
        extra: &[(&str, String)],
        body: &[u8],
        timeout: Duration,
    ) -> Result<Resp> {
        let token = self.token(&auth)?;
        let mut head = format!(
            "{method} {}{path_q} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n{PROTO_HEADER}: 1\r\nContent-Length: {}\r\n",
            self.base.prefix,
            self.base.host_header(),
            body.len()
        );
        if let Some(t) = token {
            head.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        for (k, v) in extra {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        let tcp = self.connect(timeout)?;
        match &self.tls {
            None => exchange(tcp, head.as_bytes(), body),
            Some((cfg, name)) => {
                let conn = ClientConnection::new(cfg.clone(), name.clone())
                    .map_err(|e| TransportError::Io(format!("tls: {e}")))?;
                exchange(StreamOwned::new(conn, tcp), head.as_bytes(), body)
            }
        }
    }

    fn json_call<Q: Serialize, A: DeserializeOwned>(&self, path: &str, auth: Auth, q: &Q) -> Result<A> {
        let body = serde_json::to_vec(q).map_err(|e| TransportError::Protocol(e.to_string()))?;
        let r = self.call(
            "POST",
            path,
            auth,
            &[("Content-Type", "application/json".into())],
            &body,
            IO_TIMEOUT,
        )?;
        ok_json(r)
    }
}

fn io(e: std::io::Error) -> TransportError {
    TransportError::Io(e.to_string())
}

/// Write request, read one response. The server may answer early (413 off
/// Content-Length) and close, so a failed body write still reads the answer.
fn exchange<S: Read + Write>(mut s: S, head: &[u8], body: &[u8]) -> Result<Resp> {
    s.write_all(head).map_err(io)?;
    let wrote = s.write_all(body).and_then(|_| s.flush());
    let r = read_response(&mut s);
    match (r, wrote) {
        (Ok(r), _) => Ok(r),
        (Err(e), Ok(())) => Err(e),
        (Err(_), Err(w)) => Err(io(w)),
    }
}

fn read_response<S: Read>(s: &mut S) -> Result<Resp> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    let split = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            if p > MAX_HEAD {
                return Err(TransportError::Protocol("response header too large".into()));
            }
            break p;
        }
        if buf.len() > MAX_HEAD {
            return Err(TransportError::Protocol("response header too large".into()));
        }
        let n = read_some(s, &mut chunk)?;
        if n == 0 {
            return Err(TransportError::Protocol("connection closed before response header".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..split]).map_err(|_| TransportError::Protocol("non-utf8 header".into()))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    let (ver, code) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if !ver.starts_with("HTTP/1.") {
        return Err(TransportError::Protocol("not an HTTP/1.x response".into()));
    }
    let status: u16 = code
        .parse()
        .map_err(|_| TransportError::Protocol("bad status code".into()))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let mut r = Resp {
        status,
        headers,
        body: buf[split + 4..].to_vec(),
    };
    if r.header("transfer-encoding").is_some() {
        return Err(TransportError::Protocol("chunked responses are not used by /v1".into()));
    }
    match r.header("content-length").map(|v| v.parse::<u64>()) {
        Some(Ok(len)) => {
            if len > MAX_RESPONSE {
                return Err(TransportError::Protocol("response body over the client limit".into()));
            }
            let len = len as usize;
            if r.body.len() > len {
                return Err(TransportError::Protocol("more body than Content-Length".into()));
            }
            r.body.reserve(len - r.body.len());
            while r.body.len() < len {
                let want = (len - r.body.len()).min(chunk.len());
                let n = read_some(s, &mut chunk[..want])?;
                if n == 0 {
                    return Err(TransportError::Protocol("truncated response body".into()));
                }
                r.body.extend_from_slice(&chunk[..n]);
            }
        }
        Some(Err(_)) => return Err(TransportError::Protocol("bad Content-Length".into())),
        None => {
            loop {
                let n = read_some(s, &mut chunk)?;
                if n == 0 {
                    break;
                }
                r.body.extend_from_slice(&chunk[..n]);
                if r.body.len() as u64 > MAX_RESPONSE {
                    return Err(TransportError::Protocol("response body over the client limit".into()));
                }
            }
        }
    }
    Ok(r)
}

/// `read`, mapping a TLS peer close without close_notify to EOF (we only
/// rely on EOF where Content-Length is absent, and then a missing
/// close_notify is not a truncation risk we can detect anyway).
fn read_some<S: Read>(s: &mut S, buf: &mut [u8]) -> Result<usize> {
    loop {
        match s.read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(0),
            Err(e) => return Err(io(e)),
        }
    }
}

fn api_err(r: &Resp) -> TransportError {
    if r.status == 416 {
        return TransportError::RangeNotSatisfiable;
    }
    match serde_json::from_slice::<ErrorBody>(&r.body) {
        Ok(e) => TransportError::Api {
            status: r.status,
            code: e.error,
            detail: e.detail,
        },
        Err(_) => TransportError::Protocol(format!("HTTP {} without a /v1 error body", r.status)),
    }
}

fn ok_json<A: DeserializeOwned>(r: Resp) -> Result<A> {
    if !(200..300).contains(&r.status) {
        return Err(api_err(&r));
    }
    serde_json::from_slice(&r.body).map_err(|e| TransportError::Protocol(format!("bad JSON: {e}")))
}

fn object_auth(kind: ObjKind, write: bool) -> Auth {
    match (kind, write) {
        (ObjKind::Env, _) => Auth::None,
        (ObjKind::Pair, false) => Auth::None,
        (_, true) => Auth::Write,
        (_, false) => Auth::Read,
    }
}

impl Transport for HttpTransport {
    fn health(&self) -> Result<Health> {
        ok_json(self.call("GET", "/v1/health", Auth::None, &[], &[], IO_TIMEOUT)?)
    }

    fn enroll(&self, enrollment_token: &str, pub_destroy: &[u8]) -> Result<EnrollResponse> {
        self.json_call(
            "/v1/enroll",
            Auth::None,
            &EnrollRequest {
                enrollment_token: enrollment_token.to_string(),
                pub_destroy: URL_SAFE_NO_PAD.encode(pub_destroy),
                key_alg: sync_crypto::SigAlg::EcdsaP256Sha256 as u8,
            },
        )
    }

    fn put(&self, kind: ObjKind, ns: &Id16, id: &Id16, bytes: &[u8]) -> Result<PutOutcome> {
        let r = self.call(
            "PUT",
            &object_path(kind, ns, id),
            object_auth(kind, true),
            &[("Content-Type", "application/octet-stream".into())],
            bytes,
            IO_TIMEOUT,
        )?;
        match r.status {
            201 => Ok(PutOutcome::Created),
            200 => Ok(PutOutcome::Identical),
            _ => Err(api_err(&r)),
        }
    }

    fn get(&self, kind: ObjKind, ns: &Id16, id: &Id16, range: Option<Range>) -> Result<Fetched> {
        let extra: Vec<(&str, String)> = range.map(|r| ("Range", r.header_value())).into_iter().collect();
        let r = self.call("GET", &object_path(kind, ns, id), object_auth(kind, false), &extra, &[], IO_TIMEOUT)?;
        match r.status {
            200 => Ok(Fetched {
                total: r.body.len() as u64,
                bytes: r.body,
            }),
            206 => {
                let total = r
                    .header("content-range")
                    .and_then(|v| v.rsplit_once('/'))
                    .and_then(|(_, t)| t.parse::<u64>().ok())
                    .ok_or_else(|| TransportError::Protocol("206 without Content-Range total".into()))?;
                Ok(Fetched { bytes: r.body, total })
            }
            _ => Err(api_err(&r)),
        }
    }

    fn get_deleted(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> Result<Vec<u8>> {
        let p = format!("{}?deleted=1", object_path(kind, ns, id));
        let r = self.call("GET", &p, object_auth(kind, false), &[], &[], IO_TIMEOUT)?;
        match r.status {
            200 => Ok(r.body),
            _ => Err(api_err(&r)),
        }
    }

    fn missing(&self, ns: &Id16, ids: &[Id16]) -> Result<Vec<Id16>> {
        let r: MissingResponse =
            self.json_call(&format!("/v1/missing/{ns}"), Auth::Read, &MissingRequest { ids: ids.to_vec() })?;
        Ok(r.missing)
    }

    fn list(&self, ns: &Id16, q: ListQuery) -> Result<ListResponse> {
        q.validate()
            .map_err(|_| TransportError::Config("list: wait <= 60, 1 <= limit <= 1000".into()))?;
        let p = format!("/v1/list/{ns}?{}", q.to_query_string());
        let t = IO_TIMEOUT + Duration::from_secs(q.wait as u64);
        ok_json(self.call("GET", &p, Auth::Read, &[], &[], t)?)
    }

    fn delete(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> Result<()> {
        let auth = if kind == ObjKind::Pair { Auth::None } else { Auth::Write };
        let r = self.call("DELETE", &object_path(kind, ns, id), auth, &[], &[], IO_TIMEOUT)?;
        match r.status {
            202 | 204 => Ok(()),
            _ => Err(api_err(&r)),
        }
    }

    fn destroy_request(&self, ns: &Id16) -> Result<DestroyChallenge> {
        self.json_call(
            &format!("/v1/destroy/{ns}/request"),
            Auth::None,
            &DestroyRequest {
                scope: DestroyScope::Vault,
            },
        )
    }

    fn destroy_confirm(&self, ns: &Id16, confirm: &DestroyConfirm) -> Result<DestroyReceipt> {
        self.json_call(&format!("/v1/destroy/{ns}/confirm"), Auth::None, confirm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_rules() {
        let u = BaseUrl::parse("https://sync.example.org/osy/").unwrap();
        assert_eq!((u.https, u.host.as_str(), u.port, u.prefix.as_str()), (true, "sync.example.org", 443, "/osy"));
        assert_eq!(u.host_header(), "sync.example.org");
        let u = BaseUrl::parse("http://127.0.0.1:8443").unwrap();
        assert_eq!((u.https, u.port, u.prefix.as_str()), (false, 8443, ""));
        assert_eq!(u.host_header(), "127.0.0.1:8443");
        let u = BaseUrl::parse("http://[::1]:9").unwrap();
        assert_eq!((u.host.as_str(), u.host_header()), ("::1", "[::1]:9".to_string()));
        assert!(BaseUrl::parse("http://localhost").is_ok());
        for bad in [
            "http://sync.example.org",
            "http://10.0.0.1:80",
            "ftp://x",
            "sync.example.org",
            "https://user:pw@h",
            "https://h/?q",
            "https://h/#f",
            "https://:443",
            "https://h:0",
            "https://h:99999",
            "https://h/a/../b",
            "https://[::1",
            "https://h\\x",
        ] {
            assert!(BaseUrl::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn response_parsing_bounds() {
        let ok = b"HTTP/1.1 201 Created\r\ncontent-length: 2\r\n\r\nhi".to_vec();
        let r = read_response(&mut &ok[..]).unwrap();
        assert_eq!((r.status, r.body.as_slice()), (201, &b"hi"[..]));
        let trunc = b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhi".to_vec();
        assert!(read_response(&mut &trunc[..]).is_err());
        let huge = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", MAX_RESPONSE + 1);
        assert!(read_response(&mut huge.as_bytes()).is_err());
        let chunked = b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n0\r\n\r\n".to_vec();
        assert!(read_response(&mut &chunked[..]).is_err());
        let longhead = format!("HTTP/1.1 200 OK\r\nx: {}\r\n\r\n", "a".repeat(MAX_HEAD + 10));
        assert!(read_response(&mut longhead.as_bytes()).is_err());
        assert!(read_response(&mut &b"SSH-2.0-x\r\n\r\n"[..]).is_err());
        let eof = b"HTTP/1.1 404 Not Found\r\n\r\n{\"error\":\"not_found\",\"detail\":\"x\"}".to_vec();
        let r = read_response(&mut &eof[..]).unwrap();
        assert_eq!(api_err(&r).code(), Some(sync_proto::api::ErrorCode::NotFound));
    }

    #[test]
    fn missing_tokens_fail_before_network() {
        // Port 9 on loopback is never contacted: the token check comes first.
        let t = HttpTransport::new("http://127.0.0.1:9", Roots::System).unwrap();
        let id = Id16([1; 16]);
        assert!(matches!(t.put(ObjKind::Obj, &id, &id, b"x"), Err(TransportError::Config(_))));
        assert!(matches!(t.list(&id, ListQuery { after: 0, wait: 0, limit: 1 }), Err(TransportError::Config(_))));
        t.set_tokens(Tokens { read: Some("r".into()), write: None });
        assert!(matches!(t.delete(ObjKind::Obj, &id, &id), Err(TransportError::Config(_))));
    }
}
