// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! `/v1` control-call JSON bodies (docs/sync/spec.md §5). Every struct
//! rejects unknown fields. Base64 values (tokens, signatures, receipts) stay
//! opaque `String`s here — decoding them is the caller's job (no new deps).
//! No HTTP client or server code lives in this crate.

use serde::{Deserialize, Serialize};

use crate::enc::{EncodeError, Writer};
use crate::Id16;

/// Request header carrying the protocol major version.
pub const PROTO_HEADER: &str = "Opensidian-Proto";
/// Protocol major versions this crate speaks.
pub const PROTO_VERSIONS: &[u32] = &[1];
/// Max ids per `POST /v1/missing`.
pub const MAX_MISSING_IDS: usize = 1024;
/// Max `wait` seconds on `GET /v1/list`.
pub const MAX_LIST_WAIT: u32 = 60;
/// Max `limit` on `GET /v1/list`.
pub const MAX_LIST_LIMIT: u32 = 1000;
/// Default max object size (server-configurable).
pub const DEFAULT_MAX_OBJECT: u64 = 16 * 1024 * 1024;
/// Default max control body size (server-configurable).
pub const DEFAULT_MAX_CONTROL_BODY: usize = 64 * 1024;

/// `{kind}` path segment / list item kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjKind {
    Obj,
    Cfg,
    Env,
    Pair,
}

impl ObjKind {
    pub fn path_segment(self) -> &'static str {
        match self {
            ObjKind::Obj => "obj",
            ObjKind::Cfg => "cfg",
            ObjKind::Env => "env",
            ObjKind::Pair => "pair",
        }
    }

    pub fn from_path_segment(s: &str) -> Option<Self> {
        match s {
            "obj" => Some(ObjKind::Obj),
            "cfg" => Some(ObjKind::Cfg),
            "env" => Some(ObjKind::Env),
            "pair" => Some(ObjKind::Pair),
            _ => None,
        }
    }
}

/// Object path `/v1/{kind}/{ns}/{id}`.
pub fn object_path(kind: ObjKind, ns: &Id16, id: &Id16) -> String {
    format!("/v1/{}/{}/{}", kind.path_segment(), ns, id)
}

/// `POST /v1/missing/{ns}` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissingRequest {
    pub ids: Vec<Id16>,
}

impl MissingRequest {
    /// Spec limit: at most [`MAX_MISSING_IDS`] ids.
    pub fn validate(&self) -> Result<(), ErrorCode> {
        if self.ids.len() > MAX_MISSING_IDS {
            return Err(ErrorCode::TooLarge);
        }
        Ok(())
    }
}

/// `POST /v1/missing/{ns}` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissingResponse {
    pub missing: Vec<Id16>,
}

/// Query of `GET /v1/list/{ns}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListQuery {
    pub after: u64,
    pub wait: u32,
    pub limit: u32,
}

impl ListQuery {
    pub fn validate(&self) -> Result<(), ErrorCode> {
        if self.wait > MAX_LIST_WAIT || !(1..=MAX_LIST_LIMIT).contains(&self.limit) {
            return Err(ErrorCode::BadRequest);
        }
        Ok(())
    }

    pub fn to_query_string(&self) -> String {
        format!("after={}&wait={}&limit={}", self.after, self.wait, self.limit)
    }
}

/// One entry of the list cursor. No timestamps (minimal metadata).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListItem {
    pub seq: u64,
    pub kind: ObjKind,
    pub id: Id16,
    pub size: u64,
}

/// `GET /v1/list/{ns}` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListResponse {
    pub items: Vec<ListItem>,
    pub next: u64,
}

/// Error codes (spec §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Gone,
    Conflict,
    TooLarge,
    Quota,
    Unsupported,
    RateLimited,
    Internal,
}

impl ErrorCode {
    /// HTTP status the server answers with.
    pub fn http_status(self) -> u16 {
        match self {
            ErrorCode::BadRequest | ErrorCode::Unsupported => 400,
            ErrorCode::Unauthorized => 401,
            ErrorCode::Forbidden => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::Conflict => 409,
            ErrorCode::Gone => 410,
            ErrorCode::TooLarge => 413,
            ErrorCode::RateLimited => 429,
            ErrorCode::Internal => 500,
            ErrorCode::Quota => 507,
        }
    }
}

/// Error body `{"error":"<code>","detail":"<text>"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErrorBody {
    pub error: ErrorCode,
    pub detail: String,
}

/// `GET /v1/health`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Health {
    pub ok: bool,
    pub proto: Vec<u32>,
}

/// `POST /v1/enroll` request. `key_alg` 1 = Ed25519, 2 = P-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollRequest {
    pub enrollment_token: String,
    /// base64 public key of K_destroy.
    pub pub_destroy: String,
    pub key_alg: u8,
}

/// `POST /v1/enroll` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnrollResponse {
    pub ns: Id16,
    pub read_token: String,
    pub write_token: String,
}

/// Destroy scope; v1 knows only the whole vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DestroyScope {
    Vault,
}

/// `POST /v1/destroy/{ns}/request` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestroyRequest {
    pub scope: DestroyScope,
}

/// `POST /v1/destroy/{ns}/request` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestroyChallenge {
    /// 64 lowercase hex digits (32 bytes).
    pub server_nonce: String,
    pub confirm_token: String,
    pub expires: u64,
}

/// `POST /v1/destroy/{ns}/confirm` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestroyConfirm {
    pub confirm_token: String,
    pub ts: i64,
    /// base64 signature by K_destroy over [`destroy_payload`].
    pub sig: String,
}

/// `POST /v1/destroy/{ns}/confirm` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestroyReceipt {
    pub receipt: String,
    pub server_key: String,
}

/// Canonical bytes K_destroy signs (spec §5):
/// `"osy destroy v1" || u8 action(1) || u8 scope(1) || id16 ns ||
/// 32 server_nonce || i64 ts`.
pub fn destroy_payload(ns: &Id16, server_nonce: &[u8; 32], ts: i64) -> Vec<u8> {
    let mut w = Writer::new();
    w.fixed(b"osy destroy v1")
        .u8(1)
        .u8(1)
        .fixed(&ns.0)
        .fixed(server_nonce)
        .i64(ts);
    w.into_bytes()
}

/// Encode a list of ids as canonical `list<id16>` (used inside batches).
pub fn encode_id_list(w: &mut Writer, ids: &[Id16]) -> Result<(), EncodeError> {
    w.list_len(ids.len())?;
    for id in ids {
        w.fixed(&id.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "000102030405060708090a0b0c0d0e0f";

    #[test]
    fn missing_roundtrip() {
        let j = format!(r#"{{"ids":["{ID}"]}}"#);
        let req: MissingRequest = serde_json::from_str(&j).unwrap();
        assert_eq!(req.ids[0].0[15], 0x0f);
        assert_eq!(serde_json::to_string(&req).unwrap(), j);
        req.validate().unwrap();
        let big = MissingRequest {
            ids: vec![Id16::ZERO; MAX_MISSING_IDS + 1],
        };
        assert_eq!(big.validate(), Err(ErrorCode::TooLarge));
    }

    #[test]
    fn unknown_fields_rejected() {
        let j = format!(r#"{{"ids":["{ID}"],"extra":1}}"#);
        assert!(serde_json::from_str::<MissingRequest>(&j).is_err());
        let e = r#"{"error":"conflict","detail":"x","trace":"y"}"#;
        assert!(serde_json::from_str::<ErrorBody>(e).is_err());
        let h = r#"{"ok":true,"proto":[1],"version":"9"}"#;
        assert!(serde_json::from_str::<Health>(h).is_err());
    }

    #[test]
    fn bad_ids_rejected() {
        let upper = r#"{"ids":["000102030405060708090A0B0C0D0E0F"]}"#;
        assert!(serde_json::from_str::<MissingRequest>(upper).is_err());
        let short = r#"{"ids":["0001"]}"#;
        assert!(serde_json::from_str::<MissingRequest>(short).is_err());
    }

    #[test]
    fn list_roundtrip() {
        let j = format!(r#"{{"items":[{{"seq":3,"kind":"cfg","id":"{ID}","size":120}}],"next":4}}"#);
        let r: ListResponse = serde_json::from_str(&j).unwrap();
        assert_eq!(r.items[0].kind, ObjKind::Cfg);
        assert_eq!(r.next, 4);
        assert_eq!(serde_json::to_string(&r).unwrap(), j);
        assert!(serde_json::from_str::<ListResponse>(&j.replace("cfg", "blob")).is_err());
    }

    #[test]
    fn list_query_limits() {
        let q = ListQuery {
            after: 7,
            wait: 60,
            limit: 1000,
        };
        q.validate().unwrap();
        assert_eq!(q.to_query_string(), "after=7&wait=60&limit=1000");
        assert!(ListQuery { wait: 61, ..q }.validate().is_err());
        assert!(ListQuery { limit: 0, ..q }.validate().is_err());
        assert!(ListQuery { limit: 1001, ..q }.validate().is_err());
    }

    #[test]
    fn error_codes() {
        let e = ErrorBody {
            error: ErrorCode::RateLimited,
            detail: "slow down".into(),
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"error":"rate_limited","detail":"slow down"}"#
        );
        assert_eq!(ErrorCode::Conflict.http_status(), 409);
        assert_eq!(ErrorCode::Quota.http_status(), 507);
        assert!(serde_json::from_str::<ErrorBody>(r#"{"error":"teapot","detail":""}"#).is_err());
    }

    #[test]
    fn health_enroll_destroy_shapes() {
        let h: Health = serde_json::from_str(r#"{"ok":true,"proto":[1]}"#).unwrap();
        assert_eq!(h.proto, PROTO_VERSIONS);
        let er: EnrollResponse = serde_json::from_str(&format!(
            r#"{{"ns":"{ID}","read_token":"r","write_token":"w"}}"#
        ))
        .unwrap();
        assert_eq!(er.ns.to_hex(), ID);
        let d: DestroyRequest = serde_json::from_str(r#"{"scope":"vault"}"#).unwrap();
        assert_eq!(d.scope, DestroyScope::Vault);
        assert!(serde_json::from_str::<DestroyRequest>(r#"{"scope":"all"}"#).is_err());
        let c: DestroyConfirm =
            serde_json::from_str(r#"{"confirm_token":"t","ts":-1,"sig":"s"}"#).unwrap();
        assert_eq!(c.ts, -1);
    }

    #[test]
    fn destroy_payload_layout() {
        let ns = Id16([3; 16]);
        let p = destroy_payload(&ns, &[9; 32], 0x0102);
        assert_eq!(p.len(), 14 + 1 + 1 + 16 + 32 + 8);
        assert_eq!(&p[..14], b"osy destroy v1");
        assert_eq!(&p[14..16], &[1, 1]);
        assert_eq!(&p[16..32], &[3; 16]);
        assert_eq!(&p[32..64], &[9; 32]);
        assert_eq!(&p[64..], &[0, 0, 0, 0, 0, 0, 1, 2]);
    }

    #[test]
    fn paths() {
        let id = Id16::from_hex(ID).unwrap();
        assert_eq!(
            object_path(ObjKind::Env, &Id16::ZERO, &id),
            format!("/v1/env/00000000000000000000000000000000/{ID}")
        );
        for k in [ObjKind::Obj, ObjKind::Cfg, ObjKind::Env, ObjKind::Pair] {
            assert_eq!(ObjKind::from_path_segment(k.path_segment()), Some(k));
        }
        assert_eq!(ObjKind::from_path_segment("../x"), None);
    }

    #[test]
    fn id_list_encoding() {
        let mut w = Writer::new();
        encode_id_list(&mut w, &[Id16([1; 16]), Id16([2; 16])]).unwrap();
        let b = w.into_bytes();
        assert_eq!(&b[..4], &[0, 0, 0, 2]);
        assert_eq!(b.len(), 4 + 32);
    }
}
