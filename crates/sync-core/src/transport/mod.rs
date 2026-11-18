// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Transport (M3c): how the engine reaches a replica. One trait mirrors the
//! `/v1` surface (docs/sync/spec.md §5); two backends implement it:
//!
//! * [`http::HttpTransport`] — blocking HTTP/1.1 client for
//!   opensidian-sync-server, TLS 1.3 on rustls + the AWS-LC FIPS provider
//!   (plain `http://` only to a loopback host, mirroring the server rule).
//! * [`fs::FsTransport`] — a directory replica (USB stick, NAS mount,
//!   synced folder) with the same write-once / 410 / list-cursor behaviour.
//!
//! Both answer errors as the same [`TransportError::Api`] codes, so the
//! engine never branches on the backend. Neither backend sees plaintext:
//! callers hand in sealed containers ([`crate::object`]).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sync_crypto::{ct_eq, CryptoProvider, SigAlg};
use sync_proto::api::{
    destroy_payload, DestroyChallenge, DestroyConfirm, DestroyReceipt, EnrollResponse, ErrorCode, Health,
    ListQuery, ListResponse, ObjKind, ReceiptBody, MAX_MISSING_IDS,
};
use sync_proto::Id16;

pub mod fs;
pub mod http;

/// Errors of every backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The replica refused with a spec error code (spec §5 table).
    Api { status: u16, code: ErrorCode, detail: String },
    /// 416: the requested range is outside the object.
    RangeNotSatisfiable,
    /// The backend does not offer this operation (e.g. destroy on a dir replica).
    Unsupported(&'static str),
    /// Local configuration problem (bad URL, missing token, no TLS roots).
    Config(String),
    /// Network / filesystem failure.
    Io(String),
    /// The peer spoke something that is not `/v1`.
    Protocol(String),
    /// A destroy receipt did not verify or did not match the request.
    BadReceipt(&'static str),
}

impl TransportError {
    /// Spec error code, when the replica answered with one.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            TransportError::Api { code, .. } => Some(*code),
            _ => None,
        }
    }

    pub(crate) fn api(code: ErrorCode, detail: &str) -> TransportError {
        TransportError::Api {
            status: code.http_status(),
            code,
            detail: detail.to_string(),
        }
    }
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Api { status, code, detail } => write!(f, "replica error {status} {code:?}: {detail}"),
            TransportError::RangeNotSatisfiable => f.write_str("range not satisfiable"),
            TransportError::Unsupported(w) => write!(f, "unsupported by this replica: {w}"),
            TransportError::Config(m) => write!(f, "transport configuration: {m}"),
            TransportError::Io(m) => write!(f, "transport i/o: {m}"),
            TransportError::Protocol(m) => write!(f, "protocol: {m}"),
            TransportError::BadReceipt(m) => write!(f, "destroy receipt rejected: {m}"),
        }
    }
}

impl std::error::Error for TransportError {}

pub type Result<T> = std::result::Result<T, TransportError>;

/// Outcome of a write-once PUT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    /// 201: stored now.
    Created,
    /// 200: the same bytes were already stored (idempotent retry).
    Identical,
}

/// HTTP-style byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// `bytes=a-b` (inclusive, end clamped to the size).
    Inclusive(u64, u64),
    /// `bytes=a-`.
    From(u64),
    /// `bytes=-n` (last n bytes).
    Suffix(u64),
}

impl Range {
    pub fn header_value(self) -> String {
        match self {
            Range::Inclusive(a, b) => format!("bytes={a}-{b}"),
            Range::From(a) => format!("bytes={a}-"),
            Range::Suffix(n) => format!("bytes=-{n}"),
        }
    }

    /// Resolve against `size` -> inclusive (start, end), same rules as the
    /// server (spec §5): empty objects and start >= size are unsatisfiable.
    pub fn resolve(self, size: u64) -> Result<(u64, u64)> {
        if size == 0 {
            return Err(TransportError::RangeNotSatisfiable);
        }
        let (s, e) = match self {
            Range::Inclusive(a, b) => (a, b),
            Range::From(a) => (a, size - 1),
            Range::Suffix(0) => return Err(TransportError::RangeNotSatisfiable),
            Range::Suffix(n) => (size.saturating_sub(n), size - 1),
        };
        if s > e || s >= size {
            return Err(TransportError::RangeNotSatisfiable);
        }
        Ok((s, e.min(size - 1)))
    }
}

/// Bytes of a GET plus the full object size (from `Content-Range` on 206).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub total: u64,
}

/// Per-vault bearer tokens (base64url, from enrollment). Debug is redacted.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Tokens {
    pub read: Option<String>,
    pub write: Option<String>,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("read", &self.read.as_ref().map(|_| "<redacted>"))
            .field("write", &self.write.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl From<&EnrollResponse> for Tokens {
    fn from(e: &EnrollResponse) -> Tokens {
        Tokens {
            read: Some(e.read_token.clone()),
            write: Some(e.write_token.clone()),
        }
    }
}

/// The `/v1` operations (spec §5). `ns` is the vault namespace; env objects
/// live in the all-zero namespace. Implementations are blocking and
/// thread-safe; the engine runs them off any UI thread.
pub trait Transport: Send + Sync {
    /// `GET /v1/health`.
    fn health(&self) -> Result<Health>;
    /// `POST /v1/enroll` with a one-time enrollment token; `pub_destroy` is
    /// the 65-byte uncompressed P-256 K_destroy public key.
    fn enroll(&self, enrollment_token: &str, pub_destroy: &[u8]) -> Result<EnrollResponse>;
    /// `PUT /v1/{kind}/{ns}/{id}` — write-once: same bytes -> Identical,
    /// different bytes -> Conflict, deleted id -> Gone.
    fn put(&self, kind: ObjKind, ns: &Id16, id: &Id16, bytes: &[u8]) -> Result<PutOutcome>;
    /// `GET /v1/{kind}/{ns}/{id}` with an optional range. Deleted -> Gone.
    fn get(&self, kind: ObjKind, ns: &Id16, id: &Id16, range: Option<Range>) -> Result<Fetched>;
    /// `GET ...?deleted=1` — a soft-deleted object inside its retention window.
    fn get_deleted(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> Result<Vec<u8>>;
    /// `POST /v1/missing/{ns}` for `obj` ids; batches of [`MAX_MISSING_IDS`]
    /// are sent as several calls by [`missing_all`].
    fn missing(&self, ns: &Id16, ids: &[Id16]) -> Result<Vec<Id16>>;
    /// `GET /v1/list/{ns}` — live obj/cfg after the cursor, optional long-poll.
    fn list(&self, ns: &Id16, q: ListQuery) -> Result<ListResponse>;
    /// `DELETE /v1/{kind}/{ns}/{id}` — soft delete (pair: purge).
    fn delete(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> Result<()>;
    /// `POST /v1/destroy/{ns}/request`.
    fn destroy_request(&self, ns: &Id16) -> Result<DestroyChallenge>;
    /// `POST /v1/destroy/{ns}/confirm` (raw; use [`destroy_vault`]).
    fn destroy_confirm(&self, ns: &Id16, confirm: &DestroyConfirm) -> Result<DestroyReceipt>;
}

/// `missing` over any number of ids, in spec-sized batches.
pub fn missing_all(t: &dyn Transport, ns: &Id16, ids: &[Id16]) -> Result<Vec<Id16>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(MAX_MISSING_IDS) {
        out.extend(t.missing(ns, chunk)?);
    }
    Ok(out)
}

/// Full 2-step destroy (spec §5): request a challenge, sign
/// [`destroy_payload`] with K_destroy at `ts`, confirm, then verify the
/// receipt against the server key pinned at enrollment. Returns the
/// verified receipt body.
pub fn destroy_vault<P: CryptoProvider>(
    t: &dyn Transport,
    prov: &P,
    k_destroy: &P::SigningKey,
    ns: &Id16,
    pinned_server_key: &[u8],
    ts: i64,
) -> Result<ReceiptBody> {
    let ch = t.destroy_request(ns)?;
    let nonce = parse_nonce(&ch.server_nonce)?;
    let sig = prov
        .sign(k_destroy, &destroy_payload(ns, &nonce, ts))
        .map_err(|_| TransportError::Config("K_destroy signing failed".into()))?;
    let receipt = t.destroy_confirm(
        ns,
        &DestroyConfirm {
            confirm_token: ch.confirm_token,
            ts,
            sig: URL_SAFE_NO_PAD.encode(sig),
        },
    )?;
    verify_receipt(prov, pinned_server_key, ns, &nonce, ts, &receipt)
}

/// Check a destroy receipt: `server_key` equals the pinned key, the ECDSA
/// P-256 signature verifies over the body, and the body names this vault,
/// this nonce and this `ts`.
pub fn verify_receipt<P: CryptoProvider + ?Sized>(
    prov: &P,
    pinned_server_key: &[u8],
    ns: &Id16,
    nonce: &[u8; 32],
    ts: i64,
    r: &DestroyReceipt,
) -> Result<ReceiptBody> {
    let sk = URL_SAFE_NO_PAD
        .decode(r.server_key.as_bytes())
        .map_err(|_| TransportError::BadReceipt("server_key not base64url"))?;
    if !ct_eq(&sk, pinned_server_key) {
        return Err(TransportError::BadReceipt("server key differs from the key pinned at enrollment"));
    }
    let wire = URL_SAFE_NO_PAD
        .decode(r.receipt.as_bytes())
        .map_err(|_| TransportError::BadReceipt("receipt not base64url"))?;
    let (body, signed, sig) = ReceiptBody::split(&wire).map_err(|_| TransportError::BadReceipt("receipt layout"))?;
    prov.verify(SigAlg::EcdsaP256Sha256, pinned_server_key, signed, sig)
        .map_err(|_| TransportError::BadReceipt("signature"))?;
    if body.ns != *ns || !ct_eq(&body.server_nonce, nonce) || body.ts != ts {
        return Err(TransportError::BadReceipt("receipt is for a different destroy"));
    }
    Ok(body)
}

fn parse_nonce(hex: &str) -> Result<[u8; 32]> {
    let b = hex.as_bytes();
    if b.len() != 64 {
        return Err(TransportError::Protocol("server_nonce length".into()));
    }
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        let n = |c: u8| match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        };
        match (n(b[2 * i]), n(b[2 * i + 1])) {
            (Some(h), Some(l)) => *o = h << 4 | l,
            _ => return Err(TransportError::Protocol("server_nonce not lowercase hex".into())),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_resolve_matches_server_rules() {
        assert_eq!(Range::Inclusive(2, 5).resolve(10), Ok((2, 5)));
        assert_eq!(Range::Inclusive(2, 50).resolve(10), Ok((2, 9)));
        assert_eq!(Range::From(7).resolve(10), Ok((7, 9)));
        assert_eq!(Range::Suffix(3).resolve(10), Ok((7, 9)));
        assert_eq!(Range::Suffix(30).resolve(10), Ok((0, 9)));
        for bad in [Range::Inclusive(5, 2), Range::From(10), Range::Suffix(0)] {
            assert_eq!(bad.resolve(10), Err(TransportError::RangeNotSatisfiable));
        }
        assert_eq!(Range::From(0).resolve(0), Err(TransportError::RangeNotSatisfiable));
        assert_eq!(Range::Suffix(4).header_value(), "bytes=-4");
    }

    #[test]
    fn nonce_parse() {
        let h = "00ff".repeat(16);
        let n = parse_nonce(&h).unwrap();
        assert_eq!((n[0], n[1]), (0, 0xff));
        assert!(parse_nonce(&h.to_uppercase()).is_err());
        assert!(parse_nonce("00").is_err());
    }

    #[test]
    fn tokens_debug_is_redacted() {
        let t = Tokens {
            read: Some("secret-r".into()),
            write: None,
        };
        let d = format!("{t:?}");
        assert!(!d.contains("secret"), "{d}");
    }
}
