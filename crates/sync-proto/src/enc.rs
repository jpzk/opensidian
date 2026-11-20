// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Canonical encoding primitives (docs/sync/spec.md §2).
//!
//! Fixed-width big-endian integers, `bytes` = u32 len prefix, `str` = u16 len
//! prefix (UTF-8, no NUL, ≤ [`MAX_STR`] bytes), `list<T>` = u32 count,
//! `opt<T>` = tag 0 | tag 1 + value. Exactly one encoding per value: the
//! [`Reader`] rejects trailing bytes ([`Reader::finish`]), opt tags other than
//! 0/1, over-limit lengths and invalid strings.
//!
//! NFC normalisation of `str` is NOT checked here (it would need a Unicode
//! table crate); callers that produce paths normalise before encoding —
//! docs/sync/open-questions.md.

use std::fmt;

/// Maximum encoded length of a `str` (spec §2).
pub const MAX_STR: usize = 4096;

/// Why a canonical decode failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// Input ended before the value was complete.
    Truncated,
    /// Bytes left over after the outermost value.
    TrailingBytes(usize),
    /// `opt` tag other than 0 or 1.
    BadOptTag(u8),
    /// A length or count exceeds its limit.
    TooLong { len: usize, max: usize },
    /// `str` is not valid UTF-8.
    BadUtf8,
    /// `str` contains a NUL byte.
    Nul,
    /// A record-level field has a value the spec forbids.
    Invalid(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "truncated input"),
            DecodeError::TrailingBytes(n) => write!(f, "{n} trailing bytes"),
            DecodeError::BadOptTag(t) => write!(f, "non-canonical opt tag {t}"),
            DecodeError::TooLong { len, max } => write!(f, "length {len} over limit {max}"),
            DecodeError::BadUtf8 => write!(f, "invalid UTF-8"),
            DecodeError::Nul => write!(f, "NUL in string"),
            DecodeError::Invalid(what) => write!(f, "invalid field: {what}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Why an encode failed (value not representable canonically).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    /// Length does not fit the prefix or exceeds the spec limit.
    TooLong { len: usize, max: usize },
    /// `str` contains a NUL byte.
    Nul,
    /// The value violates a record-level rule (same reason decode gives).
    Invalid(&'static str),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EncodeError::TooLong { len, max } => write!(f, "length {len} over limit {max}"),
            EncodeError::Nul => write!(f, "NUL in string"),
            EncodeError::Invalid(what) => write!(f, "invalid field: {what}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Append-only canonical encoder.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Fixed-size field with no length prefix (id16, h32, nonces).
    pub fn fixed(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }

    /// `bytes` = u32 len || bytes.
    pub fn bytes(&mut self, v: &[u8]) -> Result<&mut Self, EncodeError> {
        let len = u32::try_from(v.len()).map_err(|_| EncodeError::TooLong {
            len: v.len(),
            max: u32::MAX as usize,
        })?;
        self.buf.extend_from_slice(&len.to_be_bytes());
        self.buf.extend_from_slice(v);
        Ok(self)
    }

    /// `str` = u16 len || UTF-8, no NUL, ≤ [`MAX_STR`] bytes.
    pub fn str(&mut self, v: &str) -> Result<&mut Self, EncodeError> {
        if v.len() > MAX_STR {
            return Err(EncodeError::TooLong {
                len: v.len(),
                max: MAX_STR,
            });
        }
        if v.as_bytes().contains(&0) {
            return Err(EncodeError::Nul);
        }
        // MAX_STR < u16::MAX, so the cast is lossless.
        self.buf.extend_from_slice(&(v.len() as u16).to_be_bytes());
        self.buf.extend_from_slice(v.as_bytes());
        Ok(self)
    }

    /// `list<T>` count prefix; the caller then writes `count` items.
    pub fn list_len(&mut self, count: usize) -> Result<&mut Self, EncodeError> {
        let n = u32::try_from(count).map_err(|_| EncodeError::TooLong {
            len: count,
            max: u32::MAX as usize,
        })?;
        self.buf.extend_from_slice(&n.to_be_bytes());
        Ok(self)
    }

    /// `opt<T>` tag; the caller writes the value after `true`.
    pub fn opt_tag(&mut self, present: bool) -> &mut Self {
        self.buf.push(u8::from(present));
        self
    }
}

/// Canonical decoder over a borrowed buffer. Call [`Reader::finish`] after
/// the outermost value: leftover bytes are a canonicality violation.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes not consumed yet.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Error unless every byte was consumed.
    pub fn finish(self) -> Result<(), DecodeError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(DecodeError::TrailingBytes(n)),
        }
    }

    /// Next `n` raw bytes.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.remaining() < n {
            return Err(DecodeError::Truncated);
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Fixed-size array (id16, h32).
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.array::<2>()?))
    }

    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.array::<4>()?))
    }

    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.array::<8>()?))
    }

    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        Ok(i64::from_be_bytes(self.array::<8>()?))
    }

    /// `bytes` with an upper bound on the length.
    pub fn bytes(&mut self, max: usize) -> Result<&'a [u8], DecodeError> {
        let len = self.u32()? as usize;
        if len > max {
            return Err(DecodeError::TooLong { len, max });
        }
        self.take(len)
    }

    /// `str`: u16 len, UTF-8, no NUL, ≤ [`MAX_STR`].
    pub fn str(&mut self) -> Result<&'a str, DecodeError> {
        let len = self.u16()? as usize;
        if len > MAX_STR {
            return Err(DecodeError::TooLong { len, max: MAX_STR });
        }
        let raw = self.take(len)?;
        if raw.contains(&0) {
            return Err(DecodeError::Nul);
        }
        std::str::from_utf8(raw).map_err(|_| DecodeError::BadUtf8)
    }

    /// `list<T>` count with an upper bound.
    pub fn list_len(&mut self, max: usize) -> Result<usize, DecodeError> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(DecodeError::TooLong { len: n, max });
        }
        Ok(n)
    }

    /// `opt<T>` tag: only 0 and 1 are canonical.
    pub fn opt_tag(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            t => Err(DecodeError::BadOptTag(t)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_primitives() {
        let mut w = Writer::new();
        w.u8(7).u16(0x0102).u32(0x0304_0506).u64(u64::MAX).i64(-2);
        w.fixed(&[0xaa; 16]);
        w.bytes(b"blob").unwrap();
        w.str("notes/a.md").unwrap();
        w.list_len(2).unwrap().u8(1).u8(2);
        w.opt_tag(false).opt_tag(true).u8(9);
        let buf = w.into_bytes();

        let mut r = Reader::new(&buf);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u16().unwrap(), 0x0102);
        assert_eq!(r.u32().unwrap(), 0x0304_0506);
        assert_eq!(r.u64().unwrap(), u64::MAX);
        assert_eq!(r.i64().unwrap(), -2);
        assert_eq!(r.array::<16>().unwrap(), [0xaa; 16]);
        assert_eq!(r.bytes(16).unwrap(), b"blob");
        assert_eq!(r.str().unwrap(), "notes/a.md");
        assert_eq!(r.list_len(4).unwrap(), 2);
        assert_eq!((r.u8().unwrap(), r.u8().unwrap()), (1, 2));
        assert!(!r.opt_tag().unwrap());
        assert!(r.opt_tag().unwrap());
        assert_eq!(r.u8().unwrap(), 9);
        r.finish().unwrap();
    }

    #[test]
    fn big_endian_layout() {
        let mut w = Writer::new();
        w.u32(1).str("ab").unwrap();
        assert_eq!(w.as_bytes(), &[0, 0, 0, 1, 0, 2, b'a', b'b']);
    }

    #[test]
    fn rejects_trailing_bytes() {
        let mut r = Reader::new(&[1, 2]);
        r.u8().unwrap();
        assert_eq!(r.finish(), Err(DecodeError::TrailingBytes(1)));
    }

    #[test]
    fn rejects_truncation() {
        assert_eq!(Reader::new(&[0, 0, 0]).u32(), Err(DecodeError::Truncated));
        // bytes claims 5, only 2 present
        let mut r = Reader::new(&[0, 0, 0, 5, 1, 2]);
        assert_eq!(r.bytes(100), Err(DecodeError::Truncated));
    }

    #[test]
    fn rejects_non_canonical_opt_tag() {
        assert_eq!(Reader::new(&[2]).opt_tag(), Err(DecodeError::BadOptTag(2)));
    }

    #[test]
    fn rejects_over_limit_lengths() {
        let mut r = Reader::new(&[0, 0, 0, 9]);
        assert_eq!(r.bytes(8), Err(DecodeError::TooLong { len: 9, max: 8 }));
        let mut r = Reader::new(&[0, 0, 0, 3]);
        assert_eq!(r.list_len(2), Err(DecodeError::TooLong { len: 3, max: 2 }));
        let big = [0x10u8, 0x01]; // 4097
        assert_eq!(
            Reader::new(&big).str(),
            Err(DecodeError::TooLong {
                len: MAX_STR + 1,
                max: MAX_STR
            })
        );
        let long = "x".repeat(MAX_STR + 1);
        assert!(Writer::new().str(&long).is_err());
        assert!(Writer::new().str(&"x".repeat(MAX_STR)).is_ok());
    }

    #[test]
    fn rejects_bad_strings() {
        assert_eq!(Reader::new(&[0, 2, 0xff, 0xfe]).str(), Err(DecodeError::BadUtf8));
        assert_eq!(Reader::new(&[0, 2, b'a', 0]).str(), Err(DecodeError::Nul));
        assert_eq!(Writer::new().str("a\0b").unwrap_err(), EncodeError::Nul);
    }
}
