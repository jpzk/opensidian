// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! sync-proto — the `/v1` wire types and the canonical binary encodings of
//! docs/sync/spec.md. Pure data: no I/O, no crypto, no network code.
#![forbid(unsafe_code)]

pub mod api;
pub mod batch;
pub mod container;
pub mod enc;

use std::fmt;

/// API path version this crate speaks (spec.md §1, "path" axis).
pub const API_VERSION: &str = "v1";

/// 16-byte identifier (obj_id, vault_id, device_id, ns). On the wire in
/// JSON and URLs it is exactly 32 lowercase hex digits — one canonical form.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Id16(pub [u8; 16]);

impl Id16 {
    /// All-zero id (envelope/pairing AAD `vault_id`, spec §3.3).
    pub const ZERO: Id16 = Id16([0; 16]);

    pub fn to_hex(self) -> String {
        hex_lower(&self.0)
    }

    /// Parse 32 lowercase hex digits. Uppercase is rejected (canonical form).
    pub fn from_hex(s: &str) -> Result<Self, IdError> {
        let b = s.as_bytes();
        if b.len() != 32 {
            return Err(IdError::Length(b.len()));
        }
        let mut out = [0u8; 16];
        for (i, pair) in b.chunks_exact(2).enumerate() {
            out[i] = (hex_val(pair[0])? << 4) | hex_val(pair[1])?;
        }
        Ok(Id16(out))
    }
}

impl fmt::Debug for Id16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id16({})", self.to_hex())
    }
}

impl fmt::Display for Id16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Why an [`Id16`] hex string was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    Length(usize),
    NotLowerHex(u8),
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IdError::Length(n) => write!(f, "id must be 32 hex digits, got {n}"),
            IdError::NotLowerHex(c) => write!(f, "id byte {c:#04x} is not lowercase hex"),
        }
    }
}

impl std::error::Error for IdError {}

fn hex_val(c: u8) -> Result<u8, IdError> {
    match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        _ => Err(IdError::NotLowerHex(c)),
    }
}

/// Lowercase hex of arbitrary bytes.
pub fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 0x0f) as usize] as char);
    }
    s
}

impl serde::Serialize for Id16 {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for Id16 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        Id16::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_hex_roundtrip() {
        let id = Id16([0x00, 0x01, 0xab, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10]);
        let h = id.to_hex();
        assert_eq!(h, "0001abff000000000000000000000010");
        assert_eq!(Id16::from_hex(&h).unwrap(), id);
    }

    #[test]
    fn id_hex_rejects_non_canonical() {
        assert_eq!(Id16::from_hex("00"), Err(IdError::Length(2)));
        let upper = "0001ABFF000000000000000000000010";
        assert_eq!(Id16::from_hex(upper), Err(IdError::NotLowerHex(b'A')));
        let junk = "0001abff00000000000000000000001g";
        assert!(Id16::from_hex(junk).is_err());
    }
}
