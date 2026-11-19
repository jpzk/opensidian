// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Sync engine (docs/sync/spec.md §4.8, M4c): replicas as equal peers,
//! synced by set union.
//!
//! There is no "the server". A sync-server, a dir replica on a USB stick
//! and a NAS mount are all just [`Transport`]s holding opaque containers.
//! The engine keeps a local replica of its own (an [`FsTransport`] under
//! the state dir) and one pass does, per replica:
//!
//! 1. **pull** — list the replica; every unknown object is fetched and
//!    VERIFIED before it is stored (batch: container auth + signature by an
//!    active device; chunk: container auth under `K_obj`). Anything that
//!    does not verify is reported and neither stored nor passed on, so one
//!    bad replica cannot poison the others.
//! 2. **resolve** — the tips of the verified DAG are folded into the local
//!    head in commit_id order through [`merge_base`] / [`merge_trees`] /
//!    [`commit_merge`]. `FastForward` adopts the tip, `Commit` seals a
//!    merge commit, `UpToDate` (same tree, e.g. two devices that merged
//!    the same tips concurrently) makes no commit: the tip is *absorbed*
//!    and becomes an extra parent of the next local commit, so dangling
//!    tips do not accumulate. Losers go to the `on_loser` hook (file
//!    history).
//! 3. **push** — every local object the replica lacks: chunks first, then
//!    batches parents-first, so a reader never sees a dangling reference.
//!    A deleted id (410) is skipped.
//!
//! A replica that fails is reported and skipped; the others still sync.
//! After a pass over replicas R₁..Rₙ that all succeeded, every Rᵢ holds
//! ⋃ objects (local ∪ R₁ ∪ … ∪ Rₙ) minus whatever failed verification.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sync_crypto::CryptoProvider;
use sync_proto::api::{ErrorCode, ListQuery, ObjKind, MAX_LIST_LIMIT};
use sync_proto::batch::{Batch, Parent, MAX_PARENTS};
use sync_proto::container::Kind;
use sync_proto::Id16;

use crate::commit::{
    build_commit, fetch_entry, open_commit, push_commit, Built, CommitError, ContentRef, DeviceKeys, Edit, Signer,
    Tree, TreeEntry, VaultCtx,
};
use crate::merge::{
    commit_merge, merge_base, merge_trees, Action, ConflictMode, Dag, Loser, Merge, MergeError, Merged, Side,
};
use crate::object::{open_any, ObjectCtx};
use crate::transport::fs::FsTransport;
use crate::transport::{Transport, TransportError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    Commit(CommitError),
    Merge(MergeError),
    Transport(TransportError),
    Io(String),
    /// Another engine holds the state dir lock.
    Busy,
    /// Corrupt or foreign state file.
    State(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Commit(e) => write!(f, "engine: {e}"),
            EngineError::Merge(e) => write!(f, "engine: {e}"),
            EngineError::Transport(e) => write!(f, "engine: {e}"),
            EngineError::Io(e) => write!(f, "engine io: {e}"),
            EngineError::Busy => f.write_str("engine: state dir is locked by another engine"),
            EngineError::State(e) => write!(f, "engine state: {e}"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<CommitError> for EngineError {
    fn from(e: CommitError) -> Self {
        EngineError::Commit(e)
    }
}
impl From<MergeError> for EngineError {
    fn from(e: MergeError) -> Self {
        EngineError::Merge(e)
    }
}
impl From<TransportError> for EngineError {
    fn from(e: TransportError) -> Self {
        EngineError::Transport(e)
    }
}

fn io(e: std::io::Error) -> EngineError {
    EngineError::Io(e.to_string())
}

pub type Result<T> = std::result::Result<T, EngineError>;

/// Persisted engine state (`<dir>/state.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    v: u32,
    head: Option<Id16>,
    absorbed: BTreeSet<Id16>,
}

const STATE_V: u32 = 1;

/// A verified commit (blobs dropped; inline bytes are re-read from the
/// local replica and re-verified on demand).
#[derive(Debug, Clone)]
struct Meta {
    batch: Batch,
}

/// Outcome of one tip fold step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folded {
    pub tip: Id16,
    pub action: Action,
    /// The merge commit, for [`Action::Commit`].
    pub merge_commit: Option<Id16>,
}

/// What a [`Engine::sync`] pass did. Replica indices refer to the slice
/// passed in.
#[derive(Debug, Clone, Default)]
pub struct Report {
    /// Objects fetched, verified and stored locally.
    pub pulled: usize,
    /// Objects uploaded, summed over replicas.
    pub pushed: usize,
    /// `(replica, id, reason)` — failed verification; not stored, not passed on.
    pub rejected: Vec<(usize, Id16, String)>,
    /// `(replica, error)` — replica skipped for the rest of the pass.
    pub replica_errors: Vec<(usize, String)>,
    /// Commits whose ancestry is incomplete (waiting for a parent).
    pub pending: usize,
    pub folded: Vec<Folded>,
    pub losers: usize,
    pub head: Option<Id16>,
    /// `(tip, reason)` — verified commits whose tree does not match their
    /// declared `tree_hash`; never adopted as head or merged.
    pub invalid: Vec<(Id16, String)>,
}

/// Device display names for conflict file names (falls back to the
/// first 8 hex digits of the device id).
pub type Names = BTreeMap<Id16, String>;

pub struct Engine<'p, P: CryptoProvider + ?Sized> {
    p: &'p P,
    v: VaultCtx,
    dir: PathBuf,
    local: FsTransport,
    commits: BTreeMap<Id16, Meta>,
    chunks: BTreeSet<Id16>,
    trees: HashMap<Id16, Tree>,
    state: State,
    pub mode: ConflictMode,
    pub names: Names,
    _lock: File,
}

impl<P: CryptoProvider + ?Sized> fmt::Debug for Engine<'_, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("v", &self.v)
            .field("dir", &self.dir)
            .field("commits", &self.commits.len())
            .field("chunks", &self.chunks.len())
            .field("head", &self.state.head)
            .finish()
    }
}

struct DagView<'a>(&'a BTreeMap<Id16, Meta>);

impl Dag for DagView<'_> {
    fn parents(&self, id: &Id16) -> Option<Vec<Id16>> {
        self.0.get(id).map(|m| m.batch.parents.iter().map(|x| x.commit_id).collect())
    }
}

/// What a fetched container turned out to be.
enum Verified {
    Commit(Batch),
    Chunk,
}

fn verify<P: CryptoProvider + ?Sized>(
    p: &P,
    v: &VaultCtx,
    id: &Id16,
    bytes: &[u8],
    devices: &dyn DeviceKeys,
) -> std::result::Result<Verified, CommitError> {
    let ctx = ObjectCtx { suite: v.suite, vault_id: v.vault_id, obj_id: *id };
    let o = open_any(p, &v.read_keys(), &ctx, bytes)?;
    match o.kind {
        Kind::Batch => {
            let mut b = open_commit(p, v, id, bytes, devices)?;
            b.blobs.clear();
            Ok(Verified::Commit(b))
        }
        Kind::Chunk => Ok(Verified::Chunk),
        _ => Err(CommitError::WrongKind),
    }
}

/// All live obj ids of a replica (paged list, no long-poll).
fn list_all(t: &dyn Transport, ns: &Id16) -> std::result::Result<BTreeSet<Id16>, TransportError> {
    let mut out = BTreeSet::new();
    let mut after = 0;
    loop {
        let r = t.list(ns, ListQuery { after, wait: 0, limit: MAX_LIST_LIMIT })?;
        if r.items.is_empty() {
            return Ok(out);
        }
        out.extend(r.items.iter().filter(|i| i.kind == ObjKind::Obj).map(|i| i.id));
        if r.next <= after {
            return Ok(out);
        }
        after = r.next;
    }
}

fn parent_ids(b: &Batch) -> Vec<Id16> {
    b.parents.iter().map(|x| x.commit_id).collect()
}

/// Every id reachable from `roots` through `parents` (roots included).
fn reach(roots: &[Id16], parents: impl Fn(&Id16) -> Option<Vec<Id16>>) -> BTreeSet<Id16> {
    let mut seen = BTreeSet::new();
    let mut stack = roots.to_vec();
    while let Some(id) = stack.pop() {
        if seen.insert(id) {
            if let Some(ps) = parents(&id) {
                stack.extend(ps);
            }
        }
    }
    seen
}

/// Reject reason for a revoked device's commit nobody active built on.
pub const NOT_VOUCHED: &str = "revoked device: commit not vouched for by an active device";

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = File::create(&tmp).map_err(io)?;
    f.write_all(bytes).map_err(io)?;
    f.sync_all().map_err(io)?;
    drop(f);
    fs::rename(&tmp, path).map_err(io)?;
    // std cannot open a directory handle on Windows (File::open fails
    // there); the rename is the durability point. Unix fsyncs the parent.
    #[cfg(unix)]
    if let Some(d) = path.parent() {
        File::open(d).and_then(|d| d.sync_all()).map_err(io)?;
    }
    Ok(())
}

impl<'p, P: CryptoProvider + ?Sized> Engine<'p, P> {
    /// Open (or create) the engine state under `dir`: `lock`, `state.json`
    /// and the local replica `objects/`. The local replica is re-verified
    /// on open (`devices` = keys allowed to have signed commits).
    pub fn open(p: &'p P, v: VaultCtx, dir: impl Into<PathBuf>, devices: &dyn DeviceKeys) -> Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(io)?;
        let lock = File::options().create(true).truncate(false).write(true).open(dir.join("lock")).map_err(io)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => return Err(EngineError::Busy),
            Err(fs::TryLockError::Error(e)) => return Err(io(e)),
        }
        let local = FsTransport::open(dir.join("objects"))?;
        let sp = dir.join("state.json");
        let state = match fs::read(&sp) {
            Ok(b) => {
                let s: State = serde_json::from_slice(&b).map_err(|e| EngineError::State(e.to_string()))?;
                if s.v != STATE_V {
                    return Err(EngineError::State(format!("unknown state version {}", s.v)));
                }
                s
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => State { v: STATE_V, ..State::default() },
            Err(e) => return Err(io(e)),
        };
        let mut e = Engine {
            p,
            v,
            dir,
            local,
            commits: BTreeMap::new(),
            chunks: BTreeSet::new(),
            trees: HashMap::new(),
            state,
            mode: ConflictMode::default(),
            names: Names::new(),
            _lock: lock,
        };
        for id in list_all(&e.local, &e.v.ns)? {
            let f = e.local.get(ObjKind::Obj, &e.v.ns, &id, None)?;
            // A commit that no longer verifies (e.g. its device was dropped
            // from `devices`) is simply not part of the DAG any more.
            match verify(e.p, &e.v, &id, &f.bytes, devices) {
                Ok(Verified::Commit(b)) => {
                    e.commits.insert(id, Meta { batch: b });
                }
                Ok(Verified::Chunk) => {
                    e.chunks.insert(id);
                }
                Err(_) => {}
            }
        }
        // M5: commits of a revoked device stay only where an active device
        // (or this device's own head / absorbed tips) built on them.
        let roots: Vec<Id16> = e
            .commits
            .iter()
            .filter(|(_, m)| !devices.revoked(&m.batch.device_id))
            .map(|(id, _)| *id)
            .chain(e.state.head)
            .chain(e.state.absorbed.iter().copied())
            .collect();
        let keep = reach(&roots, |id| e.commits.get(id).map(|m| parent_ids(&m.batch)));
        e.commits.retain(|id, m| !devices.revoked(&m.batch.device_id) || keep.contains(id));
        if let Some(h) = e.state.head {
            if !e.commits.contains_key(&h) {
                return Err(EngineError::State(format!("head {} not in the local replica", h.to_hex())));
            }
        }
        Ok(e)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Switch to a new keyring of the SAME vault (M5: a VK epoch was added).
    pub fn rekey(&mut self, v: VaultCtx) -> Result<()> {
        if v.vault_id != self.v.vault_id || v.ns != self.v.ns || v.suite != self.v.suite {
            return Err(EngineError::State("rekey: different vault".into()));
        }
        self.v = v;
        Ok(())
    }

    pub fn head(&self) -> Option<Id16> {
        self.state.head
    }

    /// Would revoking `device` lose commits this replica keeps? True when the
    /// head or an absorbed tip leads to a commit of `device` that no commit
    /// of another device reaches — i.e. it is vouched for only by local state
    /// (head adopted from `device`, or a same-tree tip absorbed without a
    /// merge commit). Sealing head + absorbed tips in a local commit
    /// ([`Engine::commit_local`] with no edits) makes the vouch part of the
    /// DAG that other and future devices check.
    pub fn needs_vouch_before_revoke(&self, device: &Id16) -> bool {
        let others: Vec<Id16> = self
            .commits
            .iter()
            .filter(|(_, m)| m.batch.device_id != *device)
            .map(|(id, _)| *id)
            .collect();
        let shared = reach(&others, |id| self.commits.get(id).map(|m| parent_ids(&m.batch)));
        let local: Vec<Id16> = self.state.head.into_iter().chain(self.state.absorbed.iter().copied()).collect();
        let mine = reach(&local, |id| self.commits.get(id).map(|m| parent_ids(&m.batch)));
        mine.iter()
            .any(|id| !shared.contains(id) && self.commits.get(id).is_some_and(|m| m.batch.device_id == *device))
    }

    /// Tips absorbed by an `UpToDate` fold, waiting to become parents.
    pub fn absorbed(&self) -> &BTreeSet<Id16> {
        &self.state.absorbed
    }

    /// All verified object ids in the local replica.
    pub fn objects(&self) -> BTreeSet<Id16> {
        self.commits.keys().chain(self.chunks.iter()).copied().collect()
    }

    pub fn commit_count(&self) -> usize {
        self.commits.len()
    }

    /// Tips of the complete (ready) part of the DAG.
    pub fn tips(&self) -> Vec<Id16> {
        let (ready, _) = self.ready_and_gen();
        let mut parents = BTreeSet::new();
        for id in &ready {
            for x in &self.commits[id].batch.parents {
                parents.insert(x.commit_id);
            }
        }
        ready.into_iter().filter(|id| !parents.contains(id)).collect()
    }

    /// The tree of `id` (memoised along the `parents[0]` chain).
    pub fn tree(&mut self, id: &Id16) -> Result<Tree> {
        let mut chain = Vec::new();
        let mut cur = Some(*id);
        let mut tree = Tree::default();
        while let Some(c) = cur {
            if let Some(t) = self.trees.get(&c) {
                tree = t.clone();
                break;
            }
            let m = self.commits.get(&c).ok_or(MergeError::MissingCommit(c))?;
            chain.push(c);
            cur = m.batch.parents.first().map(|x| x.commit_id);
            if chain.len() > self.commits.len() {
                return Err(EngineError::Commit(CommitError::Tree("parent cycle")));
            }
        }
        for c in chain.into_iter().rev() {
            tree.apply(&self.commits[&c].batch)?;
            if tree.hash(self.p)? != self.commits[&c].batch.tree_hash {
                return Err(EngineError::Commit(CommitError::Tree("tree hash mismatch")));
            }
            self.trees.insert(c, tree.clone());
        }
        Ok(tree)
    }

    pub fn head_tree(&mut self) -> Result<Tree> {
        match self.state.head {
            Some(h) => self.tree(&h),
            None => Ok(Tree::default()),
        }
    }

    /// Bytes of an entry, from the local replica (verified).
    pub fn read(&self, e: &TreeEntry, devices: &dyn DeviceKeys) -> Result<Vec<u8>> {
        Ok(fetch_entry(&self.local, self.p, &self.v, e, None, devices)?)
    }

    /// Ready set (every ancestor known) + generation (1 + max parent gen;
    /// unknown parent = 0) of every known commit.
    fn ready_and_gen(&self) -> (BTreeSet<Id16>, HashMap<Id16, u64>) {
        let mut st: HashMap<Id16, (bool, u64)> = HashMap::new();
        // expanded but not finished: meeting one of these again = a cycle
        // (not expanded twice, or the walk would never end)
        let mut open: BTreeSet<Id16> = BTreeSet::new();
        for id in self.commits.keys() {
            let mut stack = vec![(*id, false)];
            while let Some((x, expanded)) = stack.pop() {
                if st.contains_key(&x) || (!expanded && open.contains(&x)) {
                    continue;
                }
                let Some(m) = self.commits.get(&x) else {
                    st.insert(x, (false, 0));
                    continue;
                };
                if !expanded {
                    open.insert(x);
                    stack.push((x, true));
                    for par in &m.batch.parents {
                        if !st.contains_key(&par.commit_id) {
                            stack.push((par.commit_id, false));
                        }
                    }
                } else {
                    // a parent still missing from `st` here = cycle → not ready
                    let mut ready = true;
                    let mut g = 0;
                    for par in &m.batch.parents {
                        let (r, pg) = st.get(&par.commit_id).copied().unwrap_or((false, 0));
                        ready &= r;
                        g = g.max(pg);
                    }
                    open.remove(&x);
                    st.insert(x, (ready, g + 1));
                }
            }
        }
        let ready = st.iter().filter(|(id, (r, _))| *r && self.commits.contains_key(id)).map(|(id, _)| *id).collect();
        let gen = st.into_iter().map(|(id, (_, g))| (id, g)).collect();
        (ready, gen)
    }

    fn is_ancestor(&self, a: &Id16, b: &Id16) -> bool {
        let mut seen = BTreeSet::new();
        let mut q = vec![*b];
        while let Some(x) = q.pop() {
            if x == *a {
                return true;
            }
            if let Some(m) = self.commits.get(&x) {
                for par in &m.batch.parents {
                    if seen.insert(par.commit_id) {
                        q.push(par.commit_id);
                    }
                }
            }
        }
        false
    }

    fn name(&self, device: &Id16) -> String {
        self.names.get(device).cloned().unwrap_or_else(|| device.to_hex()[..8].to_string())
    }

    fn store_built(&mut self, b: &Built) -> Result<()> {
        push_commit(&self.local, &self.v, b)?;
        for (id, _) in &b.chunks {
            self.chunks.insert(*id);
        }
        let mut batch = b.batch.clone();
        batch.blobs.clear();
        self.commits.insert(b.id, Meta { batch });
        self.trees.insert(b.id, b.tree.clone());
        Ok(())
    }

    fn save(&self) -> Result<()> {
        let b = serde_json::to_vec(&self.state).map_err(|e| EngineError::State(e.to_string()))?;
        write_atomic(&self.dir.join("state.json"), &b)
    }

    fn parent(&self, id: &Id16) -> Parent {
        Parent { commit_id: *id, tree_hash: self.commits[id].batch.tree_hash }
    }

    /// Commit local edits (relative to the head tree) on top of the head
    /// (+ absorbed tips as extra parents). Stored locally; the next
    /// [`Engine::sync`] pushes it.
    pub fn commit_local(
        &mut self,
        signer: &Signer<'_, P::SigningKey>,
        devices: &dyn DeviceKeys,
        ts_ms: i64,
        edits: Vec<Edit>,
    ) -> Result<Id16> {
        let head_tree = self.head_tree()?;
        let mut ids: Vec<Id16> = self.state.head.into_iter().collect();
        for a in &self.state.absorbed {
            if ids.len() >= MAX_PARENTS {
                break;
            }
            if Some(*a) != self.state.head {
                ids.push(*a);
            }
        }
        ids.sort();
        let parents: Vec<Parent> = ids.iter().map(|id| self.parent(id)).collect();
        let built = if ids.len() <= 1 {
            build_commit(self.p, &self.v, signer, ts_ms, parents, &head_tree, edits)?
        } else {
            // edits are relative to the head; the batch is relative to
            // parents[0]: materialise the target tree and diff it.
            let mut target: BTreeMap<String, Merged> =
                head_tree.0.iter().map(|(k, e)| (k.clone(), Merged::Keep(e.clone()))).collect();
            for e in edits {
                match e {
                    Edit::Put { path, class, mtime_ms, content } => {
                        let sha256 = self.p.sha256(&content);
                        target.insert(path, Merged::New { class, mtime_ms, bytes: content, sha256 });
                    }
                    Edit::PutChunks { path, class, mtime_ms, size, sha256, chunks } => {
                        let entry = TreeEntry { class, size, sha256, mtime_ms, content: ContentRef::Chunks(chunks) };
                        target.insert(path, Merged::Keep(entry));
                    }
                    Edit::Delete { path } => {
                        target.remove(&path).ok_or(CommitError::Tree("delete of a missing path"))?;
                    }
                    Edit::Rename { from, to } => {
                        if target.contains_key(&to) {
                            return Err(CommitError::Tree("rename target exists").into());
                        }
                        let m = target.remove(&from).ok_or(CommitError::Tree("rename of a missing path"))?;
                        target.insert(to, m);
                    }
                }
            }
            let base0 = self.tree(&ids[0])?;
            let plan = Merge { tree: target, tree_hash: [0; 32], action: Action::Commit, losers: vec![], log: vec![] };
            let (local, p, v) = (&self.local, self.p, &self.v);
            let mut fetch = |e: &TreeEntry| fetch_entry(local, p, v, e, None, devices);
            let diff = plan.edits(&base0, &mut fetch)?;
            let b = build_commit(self.p, &self.v, signer, ts_ms, parents, &base0, diff)?;
            let want: BTreeMap<&String, ([u8; 32], sync_proto::batch::Class)> = plan
                .tree
                .iter()
                .map(|(k, m)| match m {
                    Merged::Keep(e) => (k, (e.sha256, e.class)),
                    Merged::New { sha256, class, .. } => (k, (*sha256, *class)),
                })
                .collect();
            let got: BTreeMap<&String, ([u8; 32], sync_proto::batch::Class)> =
                b.tree.0.iter().map(|(k, e)| (k, (e.sha256, e.class))).collect();
            if want != got {
                return Err(MergeError::Inconsistent.into());
            }
            b
        };
        self.store_built(&built)?;
        self.state.head = Some(built.id);
        self.state.absorbed.clear();
        self.save()?;
        Ok(built.id)
    }

    /// One set-union pass over `replicas` (see module docs). `on_loser`
    /// receives every version a merge did not keep verbatim (wire it to
    /// [`crate::history::Store::record_sync_loser`]).
    pub fn sync(
        &mut self,
        replicas: &[&dyn Transport],
        signer: &Signer<'_, P::SigningKey>,
        devices: &dyn DeviceKeys,
        now_ms: i64,
        on_loser: &mut dyn FnMut(&Loser, &[u8]),
    ) -> Result<Report> {
        let mut rep = Report::default();
        let ns = self.v.ns;
        // 1. pull
        let mut listed: Vec<Option<BTreeSet<Id16>>> = Vec::with_capacity(replicas.len());
        // verified commits of revoked devices, held back until vouched for
        let mut quarantine: BTreeMap<Id16, (usize, Batch, Vec<u8>)> = BTreeMap::new();
        for (i, r) in replicas.iter().enumerate() {
            let ids = match list_all(*r, &ns) {
                Ok(s) => s,
                Err(e) => {
                    rep.replica_errors.push((i, e.to_string()));
                    listed.push(None);
                    continue;
                }
            };
            let mut failed = false;
            for id in &ids {
                if self.commits.contains_key(id) || self.chunks.contains(id) {
                    continue;
                }
                // a copy one replica served that failed verification does not stop
                // another replica's copy of the same id (CodeRabbit #238 engine:613)
                if quarantine.contains_key(id) || rep.rejected.iter().any(|(j, x, _)| *j == i && x == id) {
                    continue;
                }
                let bytes = match r.get(ObjKind::Obj, &ns, id, None) {
                    Ok(f) => f.bytes,
                    // deleted/GC'd between list and get: not an error
                    Err(TransportError::Api { code: ErrorCode::NotFound | ErrorCode::Gone, .. }) => continue,
                    Err(e) => {
                        rep.replica_errors.push((i, e.to_string()));
                        break;
                    }
                };
                match verify(self.p, &self.v, id, &bytes, devices) {
                    Ok(Verified::Commit(b)) if devices.revoked(&b.device_id) => {
                        quarantine.insert(*id, (i, b, bytes));
                    }
                    Ok(v) => {
                        self.local.put(ObjKind::Obj, &ns, id, &bytes)?;
                        match v {
                            Verified::Commit(b) => {
                                self.commits.insert(*id, Meta { batch: b });
                            }
                            Verified::Chunk => {
                                self.chunks.insert(*id);
                            }
                        }
                        rep.pulled += 1;
                    }
                    Err(e) => rep.rejected.push((i, *id, e.to_string())),
                }
            }
            // a replica that failed mid-pull gets no push this pass: its listing
            // is no longer a safe picture of what it holds (CodeRabbit #238 engine:644)
            listed.push(if failed { None } else { Some(ids) });
        }
        if !quarantine.is_empty() {
            let roots: Vec<Id16> = self.commits.keys().copied().collect();
            let commits = &self.commits;
            let keep = reach(&roots, |id| {
                commits.get(id).map(|m| &m.batch).or_else(|| quarantine.get(id).map(|q| &q.1)).map(parent_ids)
            });
            for (id, (i, b, bytes)) in quarantine {
                if keep.contains(&id) {
                    self.local.put(ObjKind::Obj, &ns, &id, &bytes)?;
                    self.commits.insert(id, Meta { batch: b });
                    rep.pulled += 1;
                } else {
                    rep.rejected.push((i, id, NOT_VOUCHED.into()));
                }
            }
        }
        // 2. resolve
        let (ready, gen) = self.ready_and_gen();
        rep.pending = self.commits.len() - ready.len();
        if let Some(h) = self.state.head {
            if !ready.contains(&h) {
                return Err(EngineError::State("head lost its ancestry".into()));
            }
        }
        // A tip becomes head (fast-forward) or a merge side only after its
        // declared tree_hash checks out along the first-parent chain: a
        // validly signed commit that lies about its tree is reported and
        // skipped, it neither becomes head nor aborts the pass.
        let mut tips = Vec::new();
        for t in self.tips() {
            match self.tree(&t) {
                Ok(_) => tips.push(t),
                Err(e) => rep.invalid.push((t, e.to_string())),
            }
        }
        let mut cur = self.state.head;
        for t in tips {
            let Some(c) = cur else {
                cur = Some(t);
                rep.folded.push(Folded { tip: t, action: Action::FastForward, merge_commit: None });
                continue;
            };
            if t == c || self.state.absorbed.contains(&t) || self.is_ancestor(&t, &c) {
                continue;
            }
            let base = match merge_base(&DagView(&self.commits), c, t)? {
                Some(b) => self.tree(&b)?,
                None => Tree::default(),
            };
            let (lt, rt) = (self.tree(&c)?, self.tree(&t)?);
            let (lb, rb) = (&self.commits[&c].batch, &self.commits[&t].batch);
            let ls = Side { tree: &lt, tip: c, ts_ms: lb.ts_ms, device_name: self.name(&lb.device_id) };
            let rs = Side { tree: &rt, tip: t, ts_ms: rb.ts_ms, device_name: self.name(&rb.device_id) };
            let (local, p, v) = (&self.local, self.p, &self.v);
            let mut fetch = |e: &TreeEntry| fetch_entry(local, p, v, e, None, devices);
            let m = merge_trees(p, &base, &ls, &rs, self.mode, &mut fetch)?;
            for l in &m.losers {
                let bytes = fetch(&l.entry)?;
                on_loser(l, &bytes);
                rep.losers += 1;
            }
            let mut step = Folded { tip: t, action: m.action, merge_commit: None };
            match m.action {
                Action::UpToDate => {
                    self.state.absorbed.insert(t);
                }
                Action::FastForward => cur = Some(t),
                Action::Commit => {
                    let (lp, rp) = (self.parent(&c), self.parent(&t));
                    let b = commit_merge(p, v, signer, now_ms, &m, (lp, &lt), (rp, &rt), &mut fetch)?;
                    self.store_built(&b)?;
                    step.merge_commit = Some(b.id);
                    cur = Some(b.id);
                }
            }
            rep.folded.push(step);
        }
        self.state.head = cur;
        if let Some(h) = cur {
            let keep: BTreeSet<Id16> =
                self.state.absorbed.iter().filter(|a| !self.is_ancestor(a, &h)).copied().collect();
            self.state.absorbed = keep;
        }
        self.save()?;
        rep.head = cur;
        // 3. push: chunks, then batches parents-first (generation order)
        let mut order: Vec<Id16> = self.chunks.iter().copied().collect();
        let mut cs: Vec<(u64, Id16)> = self.commits.keys().map(|id| (gen.get(id).copied().unwrap_or(u64::MAX), *id)).collect();
        cs.sort();
        order.extend(cs.into_iter().map(|(_, id)| id));
        for (i, r) in replicas.iter().enumerate() {
            let Some(have) = &listed[i] else { continue };
            for id in order.iter().filter(|id| !have.contains(id)) {
                let f = self.local.get(ObjKind::Obj, &ns, id, None)?;
                match r.put(ObjKind::Obj, &ns, id, &f.bytes) {
                    Ok(_) => rep.pushed += 1,
                    Err(TransportError::Api { code: ErrorCode::Gone, .. }) => {}
                    Err(e) => {
                        rep.replica_errors.push((i, e.to_string()));
                        failed = true;
                        break;
                    }
                }
            }
        }
        Ok(rep)
    }
}
