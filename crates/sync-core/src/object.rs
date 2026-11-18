// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Object seal / open (docs/sync/spec.md §3): the whole container pipeline
//! on top of `sync_proto::container` (layout) and a
//! `sync_crypto::CryptoProvider` (AEAD, HKDF, HMAC).
//!
//! seal:  body → optional zstd → inner header + padding → 64 KiB segments,
//!        each AES-256-GCM with a module nonce and the 39-byte AAD
//!        (`seg_idx`, `last`) → 56-byte header with the key-commitment tag.
//! open:  header parse → obj_id must equal the requested id → commitment
//!        checked in constant time BEFORE any segment is decrypted →
//!        segments opened in order with `last` set only on the final one
//!        (truncation at a segment boundary or appended segments fail
//!        authentication; reordering fails via `seg_idx`) → inner header,
//!        exact padding length, zero padding → optional bounded zstd decode.

use std::fmt;
use std::io::Read;

use sync_crypto::{
    check_commitment, derive_object_keys, key_commitment, CryptoError, CryptoProvider, NONCE_LEN,
    TAG_LEN,
};
use sync_proto::container::{
    decode_inner, encode_inner, segment_aad, segment_lens, Comp, Header, Kind, Suite, FMT_V1, SEG,
    SEG_OVERHEAD,
};
use sync_proto::enc::{DecodeError, EncodeError};
use sync_proto::Id16;

/// zstd level for bodies (fast, ~text ratio of level 3 default).
pub const ZSTD_LEVEL: i32 = 3;
/// Upper bound for a decompressed body (zip-bomb guard). Bodies are batches
/// (≤ a few MiB of manifest + small files) or chunks (≤ 4 MiB).
pub const MAX_BODY: usize = 64 * 1024 * 1024;
/// Upper bound for a whole container accepted by [`open`].
pub const MAX_CONTAINER: usize = 80 * 1024 * 1024;

/// Why sealing or opening failed. Authentication failures are a single
/// variant on purpose (no oracle on which check failed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectError {
    /// Bad framing (header, segment sizes, inner header, padding).
    Format(DecodeError),
    /// Body cannot be encoded (e.g. chunk body over 4 MiB).
    Encode(EncodeError),
    /// Commitment, GCM tag, segment order/last flag — wrong key or tampered.
    Auth,
    /// The header's obj_id is not the id that was requested.
    WrongId,
    /// The header's suite is not the one this reader expects.
    WrongSuite,
    /// zstd failed, or the decoded body is over [`MAX_BODY`].
    Compression,
    /// Container over [`MAX_CONTAINER`].
    TooLarge,
    /// Provider failure other than authentication (RNG, HKDF, module).
    Crypto(CryptoError),
}

impl fmt::Display for ObjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ObjectError::Format(e) => write!(f, "object format: {e}"),
            ObjectError::Encode(e) => write!(f, "object encode: {e}"),
            ObjectError::Auth => f.write_str("object authentication failed"),
            ObjectError::WrongId => f.write_str("object id mismatch"),
            ObjectError::WrongSuite => f.write_str("object suite mismatch"),
            ObjectError::Compression => f.write_str("object decompression failed"),
            ObjectError::TooLarge => f.write_str("object too large"),
            ObjectError::Crypto(e) => write!(f, "object crypto: {e}"),
        }
    }
}

impl std::error::Error for ObjectError {}

impl From<DecodeError> for ObjectError {
    fn from(e: DecodeError) -> Self {
        ObjectError::Format(e)
    }
}

fn crypto(e: CryptoError) -> ObjectError {
    match e {
        CryptoError::Verify => ObjectError::Auth,
        other => ObjectError::Crypto(other),
    }
}

/// Where an object lives: the AAD/commitment context. Envelope and pairing
/// objects use `vault_id = Id16::ZERO` (spec §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectCtx {
    pub suite: Suite,
    pub vault_id: Id16,
    pub obj_id: Id16,
}

/// Compression request for [`seal`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compress {
    /// Store as is (comp 0x00).
    Never,
    /// zstd, kept only if it is strictly smaller than the raw body.
    IfSmaller,
}

/// A decrypted object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub kind: Kind,
    /// Compression the writer chose (informational; `body` is decoded).
    pub comp: Comp,
    pub body: Vec<u8>,
}

/// Seal `body` as one container (spec §3). `k_root` is the root key for the
/// kind (`K_obj` batch/chunk, `K_cfg`, `K_wrap`, `K_p`).
pub fn seal<P: CryptoProvider + ?Sized>(
    p: &P,
    k_root: &[u8; 32],
    ctx: &ObjectCtx,
    kind: Kind,
    body: &[u8],
    compress: Compress,
) -> Result<Vec<u8>, ObjectError> {
    let packed;
    let (comp, stored): (Comp, &[u8]) = match compress {
        Compress::Never => (Comp::None, body),
        Compress::IfSmaller => {
            packed =
                zstd::bulk::compress(body, ZSTD_LEVEL).map_err(|_| ObjectError::Compression)?;
            if packed.len() < body.len() {
                (Comp::Zstd, &packed[..])
            } else {
                (Comp::None, body)
            }
        }
    };
    let inner = encode_inner(kind, comp, stored).map_err(ObjectError::Encode)?;
    seal_inner(p, k_root, ctx, &inner)
}

/// Seal an already-encoded inner plaintext (also used to build negative
/// test vectors). Callers normally use [`seal`].
pub fn seal_inner<P: CryptoProvider + ?Sized>(
    p: &P,
    k_root: &[u8; 32],
    ctx: &ObjectCtx,
    inner: &[u8],
) -> Result<Vec<u8>, ObjectError> {
    let suite = ctx.suite.byte();
    let keys = derive_object_keys(p, k_root, &ctx.obj_id.0, suite).map_err(crypto)?;
    let commit = key_commitment(p, &keys.commit, &ctx.vault_id.0, &ctx.obj_id.0, suite, FMT_V1);
    let header = Header {
        suite: ctx.suite,
        obj_id: ctx.obj_id,
        commit,
    };
    let lens = segment_lens(inner.len());
    let mut out = Vec::with_capacity(sync_proto::container::container_len(inner.len()));
    out.extend_from_slice(&header.encode());
    let mut off = 0usize;
    for (i, len) in lens.iter().enumerate() {
        let idx = u32::try_from(i).map_err(|_| ObjectError::TooLarge)?;
        let last = i + 1 == lens.len();
        let aad = segment_aad(&ctx.vault_id, &ctx.obj_id, idx, last, ctx.suite);
        let sealed = p
            .aead_seal(&keys.enc, &aad, &inner[off..off + len])
            .map_err(crypto)?;
        out.extend_from_slice(&sealed.nonce);
        out.extend_from_slice(&sealed.ct_and_tag);
        off += len;
    }
    Ok(out)
}

/// Open a container fetched as `ctx.obj_id`. Returns the decoded body.
pub fn open<P: CryptoProvider + ?Sized>(
    p: &P,
    k_root: &[u8; 32],
    ctx: &ObjectCtx,
    container: &[u8],
) -> Result<Opened, ObjectError> {
    let inner = open_inner(p, k_root, ctx, container)?;
    let (kind, comp, stored) = decode_inner(&inner)?;
    let body = match comp {
        Comp::None => stored.to_vec(),
        Comp::Zstd => {
            let dec =
                zstd::stream::read::Decoder::new(stored).map_err(|_| ObjectError::Compression)?;
            let mut body = Vec::new();
            dec.take(MAX_BODY as u64 + 1)
                .read_to_end(&mut body)
                .map_err(|_| ObjectError::Compression)?;
            if body.len() > MAX_BODY {
                return Err(ObjectError::Compression);
            }
            body
        }
    };
    Ok(Opened { kind, comp, body })
}

/// Authenticate + decrypt to the inner plaintext (no inner parsing).
pub fn open_inner<P: CryptoProvider + ?Sized>(
    p: &P,
    k_root: &[u8; 32],
    ctx: &ObjectCtx,
    container: &[u8],
) -> Result<Vec<u8>, ObjectError> {
    if container.len() > MAX_CONTAINER {
        return Err(ObjectError::TooLarge);
    }
    let (header, mut rest) = Header::parse(container)?;
    if header.obj_id != ctx.obj_id {
        return Err(ObjectError::WrongId);
    }
    if header.suite != ctx.suite {
        return Err(ObjectError::WrongSuite);
    }
    let suite = header.suite.byte();
    let keys = derive_object_keys(p, k_root, &header.obj_id.0, suite).map_err(crypto)?;
    // Key commitment BEFORE touching any ciphertext (spec §3.3).
    check_commitment(
        p,
        &keys.commit,
        &ctx.vault_id.0,
        &header.obj_id.0,
        suite,
        FMT_V1,
        &header.commit,
    )
    .map_err(crypto)?;
    if rest.is_empty() {
        return Err(ObjectError::Format(DecodeError::Truncated));
    }
    let mut inner = Vec::with_capacity(rest.len());
    let mut idx: u32 = 0;
    while !rest.is_empty() {
        let take = rest.len().min(SEG + SEG_OVERHEAD);
        if take <= SEG_OVERHEAD {
            // a final segment must carry ≥ 1 plaintext byte
            return Err(ObjectError::Format(DecodeError::Truncated));
        }
        let (seg, tail) = rest.split_at(take);
        let last = tail.is_empty();
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&seg[..NONCE_LEN]);
        debug_assert!(seg.len() - NONCE_LEN > TAG_LEN);
        let aad = segment_aad(&ctx.vault_id, &header.obj_id, idx, last, header.suite);
        let pt = p
            .aead_open(&keys.enc, &nonce, &aad, &seg[NONCE_LEN..])
            .map_err(|_| ObjectError::Auth)?;
        inner.extend_from_slice(&pt);
        rest = tail;
        idx = idx.checked_add(1).ok_or(ObjectError::TooLarge)?;
    }
    Ok(inner)
}
