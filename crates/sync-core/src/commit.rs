// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Commit DAG objects (docs/sync/spec.md §4.1–4.3, M4a).
//!
//! A commit is ONE `obj` container of kind batch: manifest (parents, tree
//! hash, sorted changes) + inline blobs (files ≤ 1 MiB) + the device
//! signature. Files > 1 MiB are cut into 4 MiB slices, each its own chunk
//! object (uniform 4 MiB + 10 plaintext, so all chunks look alike to the
//! server); the batch lists the chunk ids in file order and the whole-file
//! SHA-256, so reassembly is verified end to end.
//!
//! Write order: chunks first, then the batch — a reader that sees a batch
//! can always fetch its chunks. Read order: container auth (commitment,
//! GCM) → kind = batch → canonical decode + validate → commit_id = obj_id
//! → sig_alg matches the suite → device key lookup → signature → inline
//! blob hashes. Changes are relative to the tree of `parents[0]` (lowest
//! commit_id; empty tree for a root); `tree_hash` is the tree AFTER the
//! commit and is what the merge compares (§4.3).

use std::collections::BTreeMap;
use std::fmt;

use sync_crypto::{CryptoError, CryptoProvider, SigAlg};
use sync_proto::api::ObjKind;
use sync_proto::batch::{
    tree_hash_input, Batch, Change, Class, Op, Parent, Put, Where, BATCH_SIG_DOMAIN, CHUNK_BODY,
    MAX_INLINE, MAX_PARENTS,
};
use sync_proto::container::{Kind, Suite};
use sync_proto::enc::{DecodeError, EncodeError};
use sync_proto::Id16;

use crate::object::{open, seal, Compress, ObjectCtx, ObjectError};
use crate::transport::{PutOutcome, Transport, TransportError};

/// Encoded batch body cap before compression/padding: Padme (≤ 12 %) +
/// segment overhead stays under the server's default 16 MiB object limit.
/// The engine splits larger edit sets into several commits.
pub const MAX_BATCH_BODY: usize = 12 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitError {
    Object(ObjectError),
    Format(DecodeError),
    Encode(EncodeError),
    Crypto(CryptoError),
    Transport(TransportError),
    /// The container is not a batch (or not a chunk where one is expected).
    WrongKind,
    /// Decoded commit_id differs from the object id it was fetched as.
    WrongId,
    /// sig_alg does not belong to the vault suite.
    WrongAlg,
    /// Signing device is not an active device of the vault.
    UnknownDevice,
    BadSignature,
    /// Inline blob or reassembled file does not match its SHA-256 / size.
    ContentMismatch,
    /// The edit set does not apply to the base tree.
    Tree(&'static str),
    /// Encoded batch over [`MAX_BATCH_BODY`].
    TooLarge,
}

impl fmt::Display for CommitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommitError::Object(e) => write!(f, "commit object: {e}"),
            CommitError::Format(e) => write!(f, "commit format: {e}"),
            CommitError::Encode(e) => write!(f, "commit encode: {e}"),
            CommitError::Crypto(e) => write!(f, "commit crypto: {e}"),
            CommitError::Transport(e) => write!(f, "commit transport: {e}"),
            CommitError::WrongKind => f.write_str("unexpected object kind"),
            CommitError::WrongId => f.write_str("commit id mismatch"),
            CommitError::WrongAlg => f.write_str("signature algorithm not allowed by suite"),
            CommitError::UnknownDevice => f.write_str("unknown or revoked device"),
            CommitError::BadSignature => f.write_str("bad commit signature"),
            CommitError::ContentMismatch => f.write_str("content hash/size mismatch"),
            CommitError::Tree(w) => write!(f, "tree: {w}"),
            CommitError::TooLarge => f.write_str("batch too large"),
        }
    }
}

impl std::error::Error for CommitError {}

impl From<ObjectError> for CommitError {
    fn from(e: ObjectError) -> Self {
        CommitError::Object(e)
    }
}
impl From<DecodeError> for CommitError {
    fn from(e: DecodeError) -> Self {
        CommitError::Format(e)
    }
}
impl From<EncodeError> for CommitError {
    fn from(e: EncodeError) -> Self {
        CommitError::Encode(e)
    }
}
impl From<TransportError> for CommitError {
    fn from(e: TransportError) -> Self {
        CommitError::Transport(e)
    }
}

pub type Result<T> = std::result::Result<T, CommitError>;

/// Signature algorithm of a suite (spec §1).
pub fn suite_sig_alg(s: Suite) -> SigAlg {
    match s {
        Suite::Ed25519 => SigAlg::Ed25519,
        Suite::P256 => SigAlg::EcdsaP256Sha256,
    }
}

/// Per-vault object context. `Debug` redacts the key.
#[derive(Clone, PartialEq, Eq)]
pub struct VaultCtx {
    pub suite: Suite,
    pub vault_id: Id16,
    /// Server namespace of the vault.
    pub ns: Id16,
    /// `K_obj` (key-tree.md) — root key for batch + chunk objects.
    pub k_obj: [u8; 32],
}

impl fmt::Debug for VaultCtx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultCtx")
            .field("suite", &self.suite)
            .field("vault_id", &self.vault_id)
            .field("ns", &self.ns)
            .field("k_obj", &"<redacted>")
            .finish()
    }
}

impl VaultCtx {
    fn obj(&self, obj_id: Id16) -> ObjectCtx {
        ObjectCtx {
            suite: self.suite,
            vault_id: self.vault_id,
            obj_id,
        }
    }
}

/// Where a tree entry's bytes can be fetched from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentRef {
    /// Blob `blob` inside commit `commit`.
    Inline { commit: Id16, blob: u32 },
    /// Chunk objects in file order.
    Chunks(Vec<Id16>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub class: Class,
    pub size: u64,
    pub sha256: [u8; 32],
    pub mtime_ms: i64,
    pub content: ContentRef,
}

/// A full vault tree (path → entry). `BTreeMap<String, _>` iterates in
/// bytewise path order, which is the canonical tree-hash order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tree(pub BTreeMap<String, TreeEntry>);

impl Tree {
    /// SHA-256(TREE_DOMAIN || list<path, class, sha256>) — spec §4.3.
    pub fn hash<P: CryptoProvider + ?Sized>(&self, p: &P) -> Result<[u8; 32]> {
        let input =
            tree_hash_input(self.0.iter().map(|(k, e)| (k.as_str(), e.class, &e.sha256)))?;
        Ok(p.sha256(&input))
    }

    /// Apply a commit's changes (relative to this tree) in path order.
    /// Delete/rename need an existing source; a rename must carry the
    /// source's hash and may not land on an existing path.
    pub fn apply(&mut self, b: &Batch) -> Result<()> {
        for c in &b.changes {
            match &c.op {
                Op::Put(p) => {
                    let content = match &p.loc {
                        Where::Inline(i) => ContentRef::Inline {
                            commit: b.commit_id,
                            blob: *i,
                        },
                        Where::Chunks(ids) => ContentRef::Chunks(ids.clone()),
                    };
                    self.0.insert(
                        c.path.clone(),
                        TreeEntry {
                            class: p.class,
                            size: p.size,
                            sha256: p.sha256,
                            mtime_ms: p.mtime_ms,
                            content,
                        },
                    );
                }
                Op::Delete => {
                    self.0
                        .remove(&c.path)
                        .ok_or(CommitError::Tree("delete of a missing path"))?;
                }
                Op::Rename { new_path, sha256 } => {
                    let e = self
                        .0
                        .get(&c.path)
                        .ok_or(CommitError::Tree("rename of a missing path"))?;
                    if &e.sha256 != sha256 {
                        return Err(CommitError::Tree("rename hash mismatch"));
                    }
                    if self.0.contains_key(new_path) {
                        return Err(CommitError::Tree("rename target exists"));
                    }
                    let e = self.0.remove(&c.path).expect("checked above");
                    self.0.insert(new_path.clone(), e);
                }
            }
        }
        Ok(())
    }
}

/// One local edit for [`build_commit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Put {
        path: String,
        class: Class,
        mtime_ms: i64,
        content: Vec<u8>,
    },
    Delete {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
}

impl Edit {
    fn path(&self) -> &str {
        match self {
            Edit::Put { path, .. } | Edit::Delete { path } => path,
            Edit::Rename { from, .. } => from,
        }
    }
}

/// The signing device (its key is listed in the current cfg generation).
pub struct Signer<'a, K> {
    pub device_id: Id16,
    pub key: &'a K,
}

/// A sealed commit, ready to push.
#[derive(Debug, Clone)]
pub struct Built {
    pub id: Id16,
    pub batch: Batch,
    /// The batch container (kind 0x01).
    pub container: Vec<u8>,
    /// Chunk containers `(obj_id, bytes)`, in file order per file.
    pub chunks: Vec<(Id16, Vec<u8>)>,
    /// The tree after this commit (`batch.tree_hash` = `tree.hash()`).
    pub tree: Tree,
}

impl Built {
    pub fn parent(&self) -> Parent {
        Parent {
            commit_id: self.id,
            tree_hash: self.batch.tree_hash,
        }
    }
}

fn random_id<P: CryptoProvider + ?Sized>(p: &P) -> Result<Id16> {
    let mut b = [0u8; 16];
    p.random(&mut b).map_err(CommitError::Crypto)?;
    Ok(Id16(b))
}

fn sig_digest<P: CryptoProvider + ?Sized>(p: &P, signed: &[u8]) -> [u8; 32] {
    let mut m = Vec::with_capacity(BATCH_SIG_DOMAIN.len() + signed.len());
    m.extend_from_slice(BATCH_SIG_DOMAIN);
    m.extend_from_slice(signed);
    p.sha256(&m)
}

/// Build + sign + seal one commit. `base` is the tree of `parents[0]`
/// after sorting (lowest commit_id; empty for a root commit).
#[allow(clippy::too_many_arguments)]
pub fn build_commit<P: CryptoProvider + ?Sized>(
    p: &P,
    v: &VaultCtx,
    signer: &Signer<'_, P::SigningKey>,
    ts_ms: i64,
    mut parents: Vec<Parent>,
    base: &Tree,
    mut edits: Vec<Edit>,
) -> Result<Built> {
    parents.sort();
    parents.dedup_by(|a, b| a.commit_id == b.commit_id);
    if parents.len() > MAX_PARENTS {
        return Err(CommitError::Tree("too many parents"));
    }
    edits.sort_by(|a, b| a.path().as_bytes().cmp(b.path().as_bytes()));
    if edits.windows(2).any(|w| w[0].path() == w[1].path()) {
        return Err(CommitError::Tree("two edits for one path"));
    }
    let commit_id = random_id(p)?;
    let mut changes = Vec::with_capacity(edits.len());
    let mut blobs = Vec::new();
    let mut chunks = Vec::new();
    for e in edits {
        match e {
            Edit::Put {
                path,
                class,
                mtime_ms,
                content,
            } => {
                let sha256 = p.sha256(&content);
                let size = content.len() as u64;
                let loc = if content.len() <= MAX_INLINE {
                    let i = u32::try_from(blobs.len()).map_err(|_| CommitError::TooLarge)?;
                    blobs.push(content);
                    Where::Inline(i)
                } else {
                    let mut ids = Vec::new();
                    for slice in content.chunks(CHUNK_BODY) {
                        let id = random_id(p)?;
                        let c = seal(p, &v.k_obj, &v.obj(id), Kind::Chunk, slice, Compress::Never)?;
                        chunks.push((id, c));
                        ids.push(id);
                    }
                    Where::Chunks(ids)
                };
                changes.push(Change {
                    path,
                    op: Op::Put(Put {
                        class,
                        size,
                        sha256,
                        mtime_ms,
                        loc,
                    }),
                });
            }
            Edit::Delete { path } => changes.push(Change {
                path,
                op: Op::Delete,
            }),
            Edit::Rename { from, to } => {
                let sha256 = base
                    .0
                    .get(&from)
                    .ok_or(CommitError::Tree("rename of a missing path"))?
                    .sha256;
                changes.push(Change {
                    path: from,
                    op: Op::Rename {
                        new_path: to,
                        sha256,
                    },
                });
            }
        }
    }
    let mut batch = Batch {
        commit_id,
        device_id: signer.device_id,
        ts_ms,
        parents,
        tree_hash: [0; 32],
        changes,
        blobs,
        sig_alg: suite_sig_alg(v.suite) as u8,
        sig: Vec::new(),
    };
    batch.validate()?;
    let mut tree = base.clone();
    tree.apply(&batch)?;
    batch.tree_hash = tree.hash(p)?;
    let signed = batch.encode_signed()?;
    if signed.len() > MAX_BATCH_BODY {
        return Err(CommitError::TooLarge);
    }
    batch.sig = p
        .sign(signer.key, &sig_digest(p, &signed))
        .map_err(CommitError::Crypto)?;
    let body = batch.encode()?;
    let container = seal(
        p,
        &v.k_obj,
        &v.obj(commit_id),
        Kind::Batch,
        &body,
        Compress::IfSmaller,
    )?;
    Ok(Built {
        id: commit_id,
        batch,
        container,
        chunks,
        tree,
    })
}

/// Active device keys: `device_id → (alg, wire public key)` (from cfg).
pub trait DeviceKeys {
    fn key(&self, device_id: &Id16) -> Option<(SigAlg, Vec<u8>)>;
}

impl DeviceKeys for BTreeMap<Id16, (SigAlg, Vec<u8>)> {
    fn key(&self, device_id: &Id16) -> Option<(SigAlg, Vec<u8>)> {
        self.get(device_id).cloned()
    }
}

/// Authenticate, decode and verify a batch container fetched as `id`.
pub fn open_commit<P: CryptoProvider + ?Sized>(
    p: &P,
    v: &VaultCtx,
    id: &Id16,
    container: &[u8],
    devices: &dyn DeviceKeys,
) -> Result<Batch> {
    let o = open(p, &v.k_obj, &v.obj(*id), container)?;
    if o.kind != Kind::Batch {
        return Err(CommitError::WrongKind);
    }
    let (b, signed_len) = Batch::decode(&o.body)?;
    if b.commit_id != *id {
        return Err(CommitError::WrongId);
    }
    let alg = suite_sig_alg(v.suite);
    if b.sig_alg != alg as u8 {
        return Err(CommitError::WrongAlg);
    }
    let (dev_alg, pk) = devices.key(&b.device_id).ok_or(CommitError::UnknownDevice)?;
    if dev_alg != alg {
        return Err(CommitError::WrongAlg);
    }
    p.verify(alg, &pk, &sig_digest(p, &o.body[..signed_len]), &b.sig)
        .map_err(|_| CommitError::BadSignature)?;
    for c in &b.changes {
        if let Op::Put(Put {
            sha256,
            loc: Where::Inline(i),
            ..
        }) = &c.op
        {
            if p.sha256(&b.blobs[*i as usize]) != *sha256 {
                return Err(CommitError::ContentMismatch);
            }
        }
    }
    Ok(b)
}

/// Open one chunk container; returns its body (one file slice).
pub fn open_chunk<P: CryptoProvider + ?Sized>(
    p: &P,
    v: &VaultCtx,
    id: &Id16,
    container: &[u8],
) -> Result<Vec<u8>> {
    let o = open(p, &v.k_obj, &v.obj(*id), container)?;
    if o.kind != Kind::Chunk {
        return Err(CommitError::WrongKind);
    }
    Ok(o.body)
}

/// Reassemble a chunked file: every slice but the last is exactly 4 MiB,
/// the total matches `size` and the bytes match `sha256`.
pub fn assemble<P: CryptoProvider + ?Sized>(
    p: &P,
    size: u64,
    sha256: &[u8; 32],
    slices: &[Vec<u8>],
) -> Result<Vec<u8>> {
    let n = slices.len();
    if slices
        .iter()
        .enumerate()
        .any(|(i, s)| s.is_empty() || (i + 1 < n && s.len() != CHUNK_BODY))
    {
        return Err(CommitError::ContentMismatch);
    }
    let total: u64 = slices.iter().map(|s| s.len() as u64).sum();
    if total != size {
        return Err(CommitError::ContentMismatch);
    }
    let mut out = Vec::with_capacity(size as usize);
    for s in slices {
        out.extend_from_slice(s);
    }
    if p.sha256(&out) != *sha256 {
        return Err(CommitError::ContentMismatch);
    }
    Ok(out)
}

fn put_ok(r: std::result::Result<PutOutcome, TransportError>) -> Result<()> {
    r.map(|_| ()).map_err(CommitError::Transport)
}

/// Upload a built commit: chunks first, then the batch (write-once; an
/// identical re-upload is fine, so a retried push is idempotent).
pub fn push_commit(t: &dyn Transport, v: &VaultCtx, b: &Built) -> Result<()> {
    for (id, c) in &b.chunks {
        put_ok(t.put(ObjKind::Obj, &v.ns, id, c))?;
    }
    put_ok(t.put(ObjKind::Obj, &v.ns, &b.id, &b.container))
}

/// Fetch + verify a commit.
pub fn fetch_commit<P: CryptoProvider + ?Sized>(
    t: &dyn Transport,
    p: &P,
    v: &VaultCtx,
    id: &Id16,
    devices: &dyn DeviceKeys,
) -> Result<Batch> {
    let f = t.get(ObjKind::Obj, &v.ns, id, None)?;
    open_commit(p, v, id, &f.bytes, devices)
}

/// Bytes of a tree entry. Inline content comes from `commit` (the batch
/// that introduced it — pass it if already fetched, else it is fetched and
/// verified); chunked content is fetched slice by slice and verified.
pub fn fetch_entry<P: CryptoProvider + ?Sized>(
    t: &dyn Transport,
    p: &P,
    v: &VaultCtx,
    e: &TreeEntry,
    commit: Option<&Batch>,
    devices: &dyn DeviceKeys,
) -> Result<Vec<u8>> {
    match &e.content {
        ContentRef::Inline { commit: cid, blob } => {
            let fetched;
            let b = match commit {
                Some(b) if b.commit_id == *cid => b,
                _ => {
                    fetched = fetch_commit(t, p, v, cid, devices)?;
                    &fetched
                }
            };
            let data = b
                .blobs
                .get(*blob as usize)
                .ok_or(CommitError::ContentMismatch)?;
            if data.len() as u64 != e.size || p.sha256(data) != e.sha256 {
                return Err(CommitError::ContentMismatch);
            }
            Ok(data.clone())
        }
        ContentRef::Chunks(ids) => {
            let mut slices = Vec::with_capacity(ids.len());
            for id in ids {
                let f = t.get(ObjKind::Obj, &v.ns, id, None)?;
                slices.push(open_chunk(p, v, id, &f.bytes)?);
            }
            assemble(p, e.size, &e.sha256, &slices)
        }
    }
}
