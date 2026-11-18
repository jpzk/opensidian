// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Object container framing (docs/sync/spec.md §1, §3): the 56-byte header,
//! segment sizes, the 39-byte per-segment AAD, the inner plaintext header and
//! Padme padding. Pure layout — the AEAD itself lives behind
//! `sync_crypto::CryptoProvider`.

use crate::enc::{DecodeError, EncodeError, Reader, Writer};
use crate::Id16;

/// Container magic "OSYN".
pub const MAGIC: [u8; 4] = *b"OSYN";
/// Container format byte of v1.
pub const FMT_V1: u8 = 0x01;
/// Header length in bytes.
pub const HEADER_LEN: usize = 56;
/// Plaintext segment size.
pub const SEG: usize = 65536;
/// Per-segment wire overhead: 12-byte nonce + 16-byte GCM tag.
pub const SEG_OVERHEAD: usize = 12 + 16;
/// Per-segment AAD length.
pub const AAD_LEN: usize = 39;
/// Inner header length: kind u8 + comp u8 + body_len u64.
pub const INNER_HEADER_LEN: usize = 10;
/// Uniform chunk plaintext size: 4 MiB body capacity + inner header.
pub const CHUNK_PLAINTEXT_LEN: usize = 4 * 1024 * 1024 + INNER_HEADER_LEN;

/// Crypto suite byte (spec §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Suite {
    /// AES-256-GCM, HKDF/HMAC/SHA-256, Ed25519.
    Ed25519 = 0x01,
    /// Same, signatures ECDSA P-256 / SHA-256.
    P256 = 0x02,
}

impl Suite {
    /// Parse a suite byte. `0x00`, unknown values and the test range
    /// `0xF0..=0xFF` are rejected (no fallback, spec §1).
    pub fn from_byte(b: u8) -> Result<Self, DecodeError> {
        match b {
            0x01 => Ok(Suite::Ed25519),
            0x02 => Ok(Suite::P256),
            _ => Err(DecodeError::Invalid("unsupported suite")),
        }
    }

    pub fn byte(self) -> u8 {
        self as u8
    }
}

/// Inner record kind (spec §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Batch = 0x01,
    Chunk = 0x02,
    Envelope = 0x03,
    Cfg = 0x04,
    Pairing = 0x05,
}

impl Kind {
    pub fn from_byte(b: u8) -> Result<Self, DecodeError> {
        match b {
            0x01 => Ok(Kind::Batch),
            0x02 => Ok(Kind::Chunk),
            0x03 => Ok(Kind::Envelope),
            0x04 => Ok(Kind::Cfg),
            0x05 => Ok(Kind::Pairing),
            _ => Err(DecodeError::Invalid("unknown kind")),
        }
    }
}

/// Compression byte (spec §3.4). Only `None` is produced before M4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Comp {
    None = 0x00,
    Zstd = 0x01,
}

impl Comp {
    pub fn from_byte(b: u8) -> Result<Self, DecodeError> {
        match b {
            0x00 => Ok(Comp::None),
            0x01 => Ok(Comp::Zstd),
            _ => Err(DecodeError::Invalid("unknown compression")),
        }
    }
}

/// The 56-byte container header (spec §3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub suite: Suite,
    pub obj_id: Id16,
    /// Key-commitment tag (spec §3.3).
    pub commit: [u8; 32],
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut w = Writer::new();
        w.fixed(&MAGIC)
            .u8(FMT_V1)
            .u8(self.suite.byte())
            .u16(0)
            .fixed(&self.obj_id.0)
            .fixed(&self.commit);
        let mut out = [0u8; HEADER_LEN];
        out.copy_from_slice(w.as_bytes());
        out
    }

    /// Parse the header at the front of a container; returns the header and
    /// the segment bytes after it. This is all the server ever validates.
    pub fn parse(container: &[u8]) -> Result<(Header, &[u8]), DecodeError> {
        if container.len() < HEADER_LEN {
            return Err(DecodeError::Truncated);
        }
        let (head, rest) = container.split_at(HEADER_LEN);
        let mut r = Reader::new(head);
        if r.array::<4>()? != MAGIC {
            return Err(DecodeError::Invalid("bad magic"));
        }
        if r.u8()? != FMT_V1 {
            return Err(DecodeError::Invalid("unsupported fmt"));
        }
        let suite = Suite::from_byte(r.u8()?)?;
        if r.u16()? != 0 {
            return Err(DecodeError::Invalid("flags must be 0"));
        }
        let obj_id = Id16(r.array::<16>()?);
        let commit = r.array::<32>()?;
        r.finish()?;
        Ok((
            Header {
                suite,
                obj_id,
                commit,
            },
            rest,
        ))
    }
}

/// The 39-byte AAD of segment `seg_idx` (spec §3.2):
/// `vault_id || obj_id || seg_idx u32 || last u8 || suite u8 || fmt u8`.
/// Envelope and pairing objects pass `Id16::ZERO` as `vault_id`.
pub fn segment_aad(
    vault_id: &Id16,
    obj_id: &Id16,
    seg_idx: u32,
    last: bool,
    suite: Suite,
) -> [u8; AAD_LEN] {
    let mut w = Writer::new();
    w.fixed(&vault_id.0)
        .fixed(&obj_id.0)
        .u32(seg_idx)
        .u8(u8::from(last))
        .u8(suite.byte())
        .u8(FMT_V1);
    let mut out = [0u8; AAD_LEN];
    out.copy_from_slice(w.as_bytes());
    out
}

/// Plaintext segment lengths for a plaintext of `len` bytes: all `SEG`
/// except the last (1..=SEG). `len == 0` is impossible in v1 (the inner
/// header alone is 10 bytes) and yields no segments.
pub fn segment_lens(len: usize) -> Vec<usize> {
    let mut out = vec![SEG; len / SEG];
    if len % SEG != 0 {
        out.push(len % SEG);
    }
    out
}

/// Total container size for `plaintext_len` (spec §3.2):
/// `56 + Σ(seg_len + 28)`.
pub fn container_len(plaintext_len: usize) -> usize {
    HEADER_LEN + plaintext_len + segment_lens(plaintext_len).len() * SEG_OVERHEAD
}

/// Validate segment framing of a received stream: `lasts[i]` is the `last`
/// flag segment `i` was authenticated with. Exactly the final segment must
/// carry `last = 1` (truncation / extension detection, spec §3.2).
pub fn check_last_flags(lasts: &[bool]) -> Result<(), DecodeError> {
    match lasts.split_last() {
        None => Err(DecodeError::Truncated),
        Some((true, init)) if init.iter().all(|l| !l) => Ok(()),
        Some((false, _)) => Err(DecodeError::Invalid("stream truncated (last=0)")),
        Some(_) => Err(DecodeError::Invalid("segment after last=1")),
    }
}

/// Padme length (spec §3.5) for `l` = len of `kind..body`.
pub fn padme(l: u64) -> u64 {
    if l < 2 {
        return l;
    }
    let e = 63 - u64::from(l.leading_zeros()); // floor(log2 l)
    let s = 64 - u64::from(e.leading_zeros()); // floor(log2 e) + 1 (e ≥ 1)
    let z = e - s;
    let mask = (1u64 << z) - 1;
    (l + mask) & !mask
}

/// Inner plaintext (spec §3.4): `kind || comp || body_len u64 || body ||
/// zero padding`. Chunks pad to [`CHUNK_PLAINTEXT_LEN`], everything else to
/// Padme.
pub fn encode_inner(kind: Kind, comp: Comp, body: &[u8]) -> Result<Vec<u8>, EncodeError> {
    let l = INNER_HEADER_LEN + body.len();
    let total = match kind {
        Kind::Chunk => {
            if l > CHUNK_PLAINTEXT_LEN {
                return Err(EncodeError::TooLong {
                    len: body.len(),
                    max: CHUNK_PLAINTEXT_LEN - INNER_HEADER_LEN,
                });
            }
            CHUNK_PLAINTEXT_LEN
        }
        _ => padme(l as u64) as usize,
    };
    let mut w = Writer::new();
    w.u8(kind as u8)
        .u8(comp as u8)
        .u64(body.len() as u64)
        .fixed(body);
    let mut out = w.into_bytes();
    out.resize(total, 0);
    Ok(out)
}

/// Parse an inner plaintext; enforces exact padding length and zero padding.
pub fn decode_inner(pt: &[u8]) -> Result<(Kind, Comp, &[u8]), DecodeError> {
    let mut r = Reader::new(pt);
    let kind = Kind::from_byte(r.u8()?)?;
    let comp = Comp::from_byte(r.u8()?)?;
    let body_len = usize::try_from(r.u64()?).map_err(|_| DecodeError::Invalid("body_len"))?;
    let body = r.take(body_len)?;
    let pad = r.take(r.remaining())?;
    r.finish()?;
    let l = INNER_HEADER_LEN + body_len;
    let want = match kind {
        Kind::Chunk => CHUNK_PLAINTEXT_LEN,
        _ => padme(l as u64) as usize,
    };
    if pt.len() != want {
        return Err(DecodeError::Invalid("padding length"));
    }
    if pad.iter().any(|&b| b != 0) {
        return Err(DecodeError::Invalid("nonzero padding"));
    }
    Ok((kind, comp, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr() -> Header {
        Header {
            suite: Suite::Ed25519,
            obj_id: Id16([0x11; 16]),
            commit: [0x22; 32],
        }
    }

    #[test]
    fn header_layout_and_roundtrip() {
        let b = hdr().encode();
        assert_eq!(&b[0..4], b"OSYN");
        assert_eq!(b[4], 0x01);
        assert_eq!(b[5], 0x01);
        assert_eq!(&b[6..8], &[0, 0]);
        assert_eq!(&b[8..24], &[0x11; 16]);
        assert_eq!(&b[24..56], &[0x22; 32]);
        let mut c = b.to_vec();
        c.extend_from_slice(b"segs");
        let (h, rest) = Header::parse(&c).unwrap();
        assert_eq!(h, hdr());
        assert_eq!(rest, b"segs");
    }

    #[test]
    fn header_rejects() {
        let good = hdr().encode();
        assert_eq!(Header::parse(&good[..55]), Err(DecodeError::Truncated));
        for (off, val) in [(0usize, b'X'), (4, 0x02), (5, 0x00), (5, 0xF0), (5, 0x03), (7, 1)] {
            let mut b = good;
            b[off] = val;
            assert!(Header::parse(&b).is_err(), "offset {off} value {val:#x}");
        }
    }

    #[test]
    fn aad_layout() {
        let a = segment_aad(&Id16([1; 16]), &Id16([2; 16]), 0x0102_0304, true, Suite::P256);
        assert_eq!(a.len(), 39);
        assert_eq!(&a[0..16], &[1; 16]);
        assert_eq!(&a[16..32], &[2; 16]);
        assert_eq!(&a[32..36], &[1, 2, 3, 4]);
        assert_eq!(&a[36..39], &[1, 0x02, 0x01]);
        let b = segment_aad(&Id16::ZERO, &Id16([2; 16]), 0x0102_0304, false, Suite::P256);
        assert_eq!(b[36], 0);
        assert_eq!(&b[0..16], &[0; 16]);
    }

    #[test]
    fn segments_and_sizes() {
        assert!(segment_lens(0).is_empty());
        assert_eq!(segment_lens(10), vec![10]);
        assert_eq!(segment_lens(SEG), vec![SEG]);
        assert_eq!(segment_lens(SEG + 1), vec![SEG, 1]);
        assert_eq!(container_len(10), 56 + 10 + 28);
        assert_eq!(container_len(2 * SEG + 5), 56 + 2 * SEG + 5 + 3 * 28);
    }

    #[test]
    fn last_flag_rules() {
        assert!(check_last_flags(&[true]).is_ok());
        assert!(check_last_flags(&[false, false, true]).is_ok());
        assert!(check_last_flags(&[]).is_err());
        assert!(check_last_flags(&[false]).is_err());
        assert!(check_last_flags(&[true, true]).is_err());
        assert!(check_last_flags(&[false, true, false]).is_err());
    }

    #[test]
    fn padme_values() {
        assert_eq!(padme(0), 0);
        assert_eq!(padme(1), 1);
        assert_eq!(padme(2), 2);
        assert_eq!(padme(10), 10);
        // E=3,S=2,z=1 -> round to even
        assert_eq!(padme(9), 10);
        // 1000: E=9, S=4, z=5 -> multiple of 32
        assert_eq!(padme(1000), 1024);
        assert_eq!(padme(1025), 1088);
        for l in 2..5000u64 {
            let p = padme(l);
            assert!(p >= l && p - l <= l / 8 + 1, "l={l} p={p}");
        }
    }

    #[test]
    fn inner_roundtrip_and_padding() {
        let pt = encode_inner(Kind::Batch, Comp::None, &[7u8; 1000]).unwrap();
        assert_eq!(pt.len() as u64, padme(1010));
        let (k, c, body) = decode_inner(&pt).unwrap();
        assert_eq!((k, c), (Kind::Batch, Comp::None));
        assert_eq!(body, &[7u8; 1000][..]);

        let mut bad = pt.clone();
        *bad.last_mut().unwrap() = 1;
        assert_eq!(decode_inner(&bad), Err(DecodeError::Invalid("nonzero padding")));
        let mut short = pt.clone();
        short.pop();
        assert!(decode_inner(&short).is_err());
        let mut long = pt;
        long.push(0);
        assert!(decode_inner(&long).is_err());
    }

    #[test]
    fn chunk_is_uniform() {
        let a = encode_inner(Kind::Chunk, Comp::None, b"x").unwrap();
        let b = encode_inner(Kind::Chunk, Comp::None, &vec![1u8; 4 * 1024 * 1024]).unwrap();
        assert_eq!(a.len(), CHUNK_PLAINTEXT_LEN);
        assert_eq!(b.len(), CHUNK_PLAINTEXT_LEN);
        assert!(decode_inner(&a).is_ok());
        assert!(encode_inner(Kind::Chunk, Comp::None, &vec![0u8; 4 * 1024 * 1024 + 1]).is_err());
    }

    #[test]
    fn suite_rejects_unknown_and_test_range() {
        assert!(Suite::from_byte(0).is_err());
        assert!(Suite::from_byte(3).is_err());
        assert!(Suite::from_byte(0xF0).is_err());
        assert_eq!(Suite::from_byte(1).unwrap(), Suite::Ed25519);
    }
}
