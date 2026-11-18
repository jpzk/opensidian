// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Commit records (docs/sync/spec.md §4.1–4.3): the batch = one commit of
//! the DAG (manifest + inline blobs + signature in ONE object), chunk
//! limits, and the canonical tree-hash input. Pure layout: hashing and
//! signing happen in `sync_core::commit` on the crypto provider.
//!
//! Canonical rules enforced on BOTH encode and decode ([`Batch::validate`]):
//! parents strictly ascending by commit_id (≤ 16), changes strictly
//! ascending by path bytes (≤ 1 change per path), valid relative paths,
//! inline blobs referenced exactly once and in change order, inline size
//! = blob length ≤ 1 MiB, chunked size > 1 MiB with exactly
//! ceil(size / 4 MiB) chunk ids.

use crate::enc::{DecodeError, EncodeError, Reader, Writer};
use crate::Id16;

pub const COMMIT_VERSION: u8 = 0x01;
/// Domain prefix of the batch signature digest.
pub const BATCH_SIG_DOMAIN: &[u8] = b"osy batch v1";
/// Domain prefix of the tree hash.
pub const TREE_DOMAIN: &[u8] = b"osy tree v1";
pub const MAX_PARENTS: usize = 16;
/// Files up to this size travel inline in the batch; larger ones as chunks.
pub const MAX_INLINE: usize = 1024 * 1024;
/// Chunk body size (last chunk of a file may be shorter).
pub const CHUNK_BODY: usize = 4 * 1024 * 1024;
pub const MAX_CHANGES: usize = 1 << 16;
/// 16 GiB per file.
pub const MAX_CHUNKS_PER_FILE: usize = 4096;
pub const MAX_SIG: usize = 512;

/// File class (spec §4.1) — selects the merge rule (M4b).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Markdown = 1,
    Other = 2,
    SettingsJson = 3,
}

impl Class {
    pub fn from_byte(b: u8) -> Result<Self, DecodeError> {
        match b {
            1 => Ok(Class::Markdown),
            2 => Ok(Class::Other),
            3 => Ok(Class::SettingsJson),
            _ => Err(DecodeError::Invalid("unknown file class")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Parent {
    pub commit_id: Id16,
    pub tree_hash: [u8; 32],
}

/// Where the content of a put lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Where {
    /// Index into [`Batch::blobs`].
    Inline(u32),
    /// Chunk object ids in file order.
    Chunks(Vec<Id16>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Put {
    pub class: Class,
    pub size: u64,
    pub sha256: [u8; 32],
    pub mtime_ms: i64,
    pub loc: Where,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Put(Put),
    Delete,
    /// Move `path` to `new_path`; `sha256` must match the old content.
    Rename { new_path: String, sha256: [u8; 32] },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: String,
    pub op: Op,
}

/// One commit (kind 0x01). `sig` covers
/// `SHA-256(BATCH_SIG_DOMAIN || encode_signed())`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub commit_id: Id16,
    pub device_id: Id16,
    pub ts_ms: i64,
    pub parents: Vec<Parent>,
    pub tree_hash: [u8; 32],
    pub changes: Vec<Change>,
    pub blobs: Vec<Vec<u8>>,
    pub sig_alg: u8,
    pub sig: Vec<u8>,
}

/// Number of chunks a file of `size` bytes is cut into.
pub fn chunk_count(size: u64) -> u64 {
    size.div_ceil(CHUNK_BODY as u64)
}

/// A vault-relative path: non-empty, `/`-separated, no leading/trailing
/// `/`, no empty, `.` or `..` segment, no backslash, no control chars.
pub fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= crate::enc::MAX_STR
        && !p.chars().any(|c| c.is_control() || c == '\\')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

fn bad(what: &'static str) -> DecodeError {
    DecodeError::Invalid(what)
}

impl Batch {
    /// Structural/canonical checks shared by encode and decode.
    pub fn validate(&self) -> Result<(), DecodeError> {
        if self.parents.len() > MAX_PARENTS {
            return Err(bad("too many parents"));
        }
        if !self.parents.windows(2).all(|w| w[0].commit_id < w[1].commit_id) {
            return Err(bad("parents not strictly sorted"));
        }
        if self.parents.iter().any(|p| p.commit_id == self.commit_id) {
            return Err(bad("commit is its own parent"));
        }
        if self.changes.len() > MAX_CHANGES {
            return Err(bad("too many changes"));
        }
        if !self
            .changes
            .windows(2)
            .all(|w| w[0].path.as_bytes() < w[1].path.as_bytes())
        {
            return Err(bad("changes not strictly sorted by path"));
        }
        let mut next_blob: u32 = 0;
        for c in &self.changes {
            if !valid_path(&c.path) {
                return Err(bad("invalid path"));
            }
            match &c.op {
                Op::Put(p) => match &p.loc {
                    Where::Inline(i) => {
                        if *i != next_blob {
                            return Err(bad("blob index out of order"));
                        }
                        let blob = self
                            .blobs
                            .get(*i as usize)
                            .ok_or(bad("blob index out of range"))?;
                        if blob.len() > MAX_INLINE || blob.len() as u64 != p.size {
                            return Err(bad("inline size"));
                        }
                        next_blob += 1;
                    }
                    Where::Chunks(ids) => {
                        if p.size <= MAX_INLINE as u64 {
                            return Err(bad("small file stored as chunks"));
                        }
                        if ids.len() > MAX_CHUNKS_PER_FILE || ids.len() as u64 != chunk_count(p.size)
                        {
                            return Err(bad("chunk count"));
                        }
                    }
                },
                Op::Delete => {}
                Op::Rename { new_path, .. } => {
                    if !valid_path(new_path) || new_path == &c.path {
                        return Err(bad("invalid rename target"));
                    }
                }
            }
        }
        if next_blob as usize != self.blobs.len() {
            return Err(bad("unreferenced blob"));
        }
        if self.sig.len() > MAX_SIG {
            return Err(bad("signature too long"));
        }
        Ok(())
    }

    /// Every byte the signature covers (up to and including `sig_alg`).
    pub fn encode_signed(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate().map_err(|e| match e {
            DecodeError::Invalid(w) => EncodeError::Invalid(w),
            _ => EncodeError::Invalid("batch"),
        })?;
        let mut w = Writer::new();
        w.u8(COMMIT_VERSION)
            .fixed(&self.commit_id.0)
            .fixed(&self.device_id.0)
            .i64(self.ts_ms);
        w.list_len(self.parents.len())?;
        for p in &self.parents {
            w.fixed(&p.commit_id.0).fixed(&p.tree_hash);
        }
        w.fixed(&self.tree_hash);
        w.list_len(self.changes.len())?;
        for c in &self.changes {
            match &c.op {
                Op::Put(p) => {
                    w.u8(0x01).str(&c.path)?;
                    w.u8(p.class as u8)
                        .u64(p.size)
                        .fixed(&p.sha256)
                        .i64(p.mtime_ms);
                    match &p.loc {
                        Where::Inline(i) => {
                            w.u8(0x01).u32(*i);
                        }
                        Where::Chunks(ids) => {
                            w.u8(0x02).list_len(ids.len())?;
                            for id in ids {
                                w.fixed(&id.0);
                            }
                        }
                    }
                }
                Op::Delete => {
                    w.u8(0x02).str(&c.path)?;
                }
                Op::Rename { new_path, sha256 } => {
                    w.u8(0x03).str(&c.path)?;
                    w.str(new_path)?.fixed(sha256);
                }
            }
        }
        w.list_len(self.blobs.len())?;
        for b in &self.blobs {
            w.bytes(b)?;
        }
        w.u8(self.sig_alg);
        Ok(w.into_bytes())
    }

    /// Full canonical record (signed part || `bytes sig`).
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut out = self.encode_signed()?;
        let mut w = Writer::new();
        w.bytes(&self.sig)?;
        out.extend_from_slice(w.as_bytes());
        Ok(out)
    }

    /// Parse + validate. Returns the batch and the length of the signed
    /// prefix (`buf[..len]` is what [`Batch::encode_signed`] produces).
    pub fn decode(buf: &[u8]) -> Result<(Batch, usize), DecodeError> {
        let mut r = Reader::new(buf);
        if r.u8()? != COMMIT_VERSION {
            return Err(bad("unsupported commit_version"));
        }
        let commit_id = Id16(r.array()?);
        let device_id = Id16(r.array()?);
        let ts_ms = r.i64()?;
        let np = r.list_len(MAX_PARENTS)?;
        let mut parents = Vec::with_capacity(np);
        for _ in 0..np {
            parents.push(Parent {
                commit_id: Id16(r.array()?),
                tree_hash: r.array()?,
            });
        }
        let tree_hash = r.array()?;
        let nc = r.list_len(MAX_CHANGES)?;
        let mut changes = Vec::with_capacity(nc.min(1024));
        for _ in 0..nc {
            let op = r.u8()?;
            let path = r.str()?.to_owned();
            let op = match op {
                0x01 => {
                    let class = Class::from_byte(r.u8()?)?;
                    let size = r.u64()?;
                    let sha256 = r.array()?;
                    let mtime_ms = r.i64()?;
                    let loc = match r.u8()? {
                        0x01 => Where::Inline(r.u32()?),
                        0x02 => {
                            let n = r.list_len(MAX_CHUNKS_PER_FILE)?;
                            let mut ids = Vec::with_capacity(n);
                            for _ in 0..n {
                                ids.push(Id16(r.array()?));
                            }
                            Where::Chunks(ids)
                        }
                        _ => return Err(bad("unknown content location")),
                    };
                    Op::Put(Put {
                        class,
                        size,
                        sha256,
                        mtime_ms,
                        loc,
                    })
                }
                0x02 => Op::Delete,
                0x03 => Op::Rename {
                    new_path: r.str()?.to_owned(),
                    sha256: r.array()?,
                },
                _ => return Err(bad("unknown change op")),
            };
            changes.push(Change { path, op });
        }
        let nb = r.list_len(MAX_CHANGES)?;
        let mut blobs = Vec::with_capacity(nb.min(1024));
        for _ in 0..nb {
            blobs.push(r.bytes(MAX_INLINE)?.to_vec());
        }
        let sig_alg = r.u8()?;
        let signed_len = buf.len() - r.remaining();
        let sig = r.bytes(MAX_SIG)?.to_vec();
        r.finish()?;
        let b = Batch {
            commit_id,
            device_id,
            ts_ms,
            parents,
            tree_hash,
            changes,
            blobs,
            sig_alg,
            sig,
        };
        b.validate()?;
        Ok((b, signed_len))
    }
}

/// Tree-hash input (spec §4.3): `TREE_DOMAIN || list<str path || u8 class
/// || h32 sha256>`, entries strictly ascending by path bytes (a BTreeMap
/// over `String` iterates in exactly that order).
pub fn tree_hash_input<'a, I>(entries: I) -> Result<Vec<u8>, EncodeError>
where
    I: ExactSizeIterator<Item = (&'a str, Class, &'a [u8; 32])>,
{
    let mut w = Writer::new();
    w.fixed(TREE_DOMAIN).list_len(entries.len())?;
    let mut prev: Option<&str> = None;
    for (path, class, sha) in entries {
        if prev.is_some_and(|p| p.as_bytes() >= path.as_bytes()) {
            return Err(EncodeError::Invalid("tree entries not strictly sorted"));
        }
        prev = Some(path);
        w.str(path)?.u8(class as u8).fixed(sha);
    }
    Ok(w.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_inline(path: &str, i: u32, size: u64) -> Change {
        Change {
            path: path.into(),
            op: Op::Put(Put {
                class: Class::Markdown,
                size,
                sha256: [3; 32],
                mtime_ms: -5,
                loc: Where::Inline(i),
            }),
        }
    }

    fn sample() -> Batch {
        Batch {
            commit_id: Id16([9; 16]),
            device_id: Id16([1; 16]),
            ts_ms: 1_700_000_000_000,
            parents: vec![
                Parent {
                    commit_id: Id16([2; 16]),
                    tree_hash: [4; 32],
                },
                Parent {
                    commit_id: Id16([5; 16]),
                    tree_hash: [6; 32],
                },
            ],
            tree_hash: [7; 32],
            changes: vec![
                put_inline("a.md", 0, 3),
                Change {
                    path: "b/big.bin".into(),
                    op: Op::Put(Put {
                        class: Class::Other,
                        size: (CHUNK_BODY + 1) as u64,
                        sha256: [8; 32],
                        mtime_ms: 1,
                        loc: Where::Chunks(vec![Id16([10; 16]), Id16([11; 16])]),
                    }),
                },
                Change {
                    path: "c.md".into(),
                    op: Op::Delete,
                },
                Change {
                    path: "d.md".into(),
                    op: Op::Rename {
                        new_path: "e/d.md".into(),
                        sha256: [12; 32],
                    },
                },
                put_inline("z.json", 1, 0),
            ],
            blobs: vec![b"abc".to_vec(), Vec::new()],
            sig_alg: 2,
            sig: vec![0xee; 64],
        }
    }

    #[test]
    fn roundtrip_and_signed_prefix() {
        let b = sample();
        let enc = b.encode().unwrap();
        let (d, n) = Batch::decode(&enc).unwrap();
        assert_eq!(d, b);
        assert_eq!(&enc[..n], &b.encode_signed().unwrap()[..]);
        assert_eq!(enc.len() - n, 4 + 64);
    }

    #[test]
    fn decode_rejects_non_canonical() {
        let enc = sample().encode().unwrap();
        let mut t = enc.clone();
        t.push(0);
        assert!(matches!(Batch::decode(&t), Err(DecodeError::TrailingBytes(1))));
        assert!(Batch::decode(&enc[..enc.len() - 1]).is_err());
        let mut v = enc.clone();
        v[0] = 2;
        assert!(Batch::decode(&v).is_err(), "commit_version");
    }

    #[test]
    fn validate_rules() {
        let ok = sample();
        ok.validate().unwrap();
        let mut b = ok.clone();
        b.parents.swap(0, 1);
        assert!(b.validate().is_err(), "unsorted parents");
        let mut b = ok.clone();
        b.parents[1].commit_id = b.commit_id;
        assert!(b.validate().is_err(), "self parent");
        let mut b = ok.clone();
        b.changes.swap(0, 2);
        assert!(b.validate().is_err(), "unsorted changes");
        let mut b = ok.clone();
        b.changes[2].path = "a.md".into();
        assert!(b.validate().is_err(), "duplicate path");
        let mut b = ok.clone();
        b.blobs.push(vec![1]);
        assert!(b.validate().is_err(), "unreferenced blob");
        let mut b = ok.clone();
        b.blobs[0] = b"abcd".to_vec();
        assert!(b.validate().is_err(), "inline size mismatch");
        let mut b = ok.clone();
        if let Op::Put(p) = &mut b.changes[1].op {
            p.loc = Where::Chunks(vec![Id16([10; 16])]);
        }
        assert!(b.validate().is_err(), "chunk count");
        let mut b = ok.clone();
        if let Op::Put(p) = &mut b.changes[1].op {
            p.size = MAX_INLINE as u64;
            p.loc = Where::Chunks(vec![Id16([10; 16])]);
        }
        assert!(b.validate().is_err(), "small file as chunks");
        let mut b = ok.clone();
        b.changes[3].op = Op::Rename {
            new_path: "d.md".into(),
            sha256: [0; 32],
        };
        assert!(b.validate().is_err(), "rename onto itself");
        let mut b = ok.clone();
        b.parents = (0..17u8)
            .map(|i| Parent {
                commit_id: Id16([100 + i; 16]),
                tree_hash: [0; 32],
            })
            .collect();
        assert!(b.validate().is_err(), "too many parents");
        assert!(b.encode().is_err());
    }

    #[test]
    fn paths() {
        for p in ["a.md", "dir/sub/x y.md", "ü/ñ.md", ".obsidian/app.json"] {
            assert!(valid_path(p), "{p}");
        }
        for p in ["", "/a", "a/", "a//b", "./a", "a/../b", "..", "a\\b", "a\nb"] {
            assert!(!valid_path(p), "{p:?}");
        }
    }

    #[test]
    fn tree_input_layout() {
        let h = [1u8; 32];
        let v = tree_hash_input([("a", Class::Markdown, &h)].into_iter()).unwrap();
        let mut want = TREE_DOMAIN.to_vec();
        want.extend([0, 0, 0, 1, 0, 1, b'a', 1]);
        want.extend([1u8; 32]);
        assert_eq!(v, want);
        assert!(tree_hash_input([("b", Class::Other, &h), ("a", Class::Other, &h)].into_iter()).is_err());
        assert_eq!(chunk_count(0), 0);
        assert_eq!(chunk_count(CHUNK_BODY as u64), 1);
        assert_eq!(chunk_count(CHUNK_BODY as u64 + 1), 2);
    }
}
