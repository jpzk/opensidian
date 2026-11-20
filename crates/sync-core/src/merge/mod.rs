// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Tip merge (docs/sync/spec.md §4.7, M4b).
//!
//! Inputs: the exact DAG base ([`merge_base`]) and the two tips' trees.
//! Per path (b = base, l = local, r = remote; equality = class + SHA-256):
//!
//! - l == r → l. l == b → r. r == b → l (incl. one-sided add/delete).
//! - one side deleted, the other edited → the edit wins (edit beats delete).
//! - both changed, markdown on both sides:
//!   - mode [`ConflictMode::AutoMerge`] (default): diff-match-patch
//!     with the exact base ([`dmp::merge3`]); the diff base → loser is
//!     applied onto the winner. Patch failure, no base (add/add), non-UTF-8
//!     → conflict file.
//!   - mode [`ConflictMode::ConflictFile`] → conflict file.
//!   - conflict file: the path keeps the winner, the loser's bytes go to
//!     `"<note> (Conflicted copy <loser device> YYYYMMDDHHMM).md"` (UTC
//!     of the loser tip's signed ts).
//! - both changed, settings JSON on both: local keys on top of remote
//!   (deep for nested objects); unparsable / non-object → local.
//! - otherwise last-write-wins: larger tip `ts_ms`, tie → larger tip
//!   commit_id.
//!
//! "Winner" is always the LWW winner of the two tips, so both devices
//! compute the same tree (apart from the per-device conflict mode and the
//! settings JSON rule, which are local by design). Every version that does
//! not survive verbatim is reported in [`Merge::losers`] so the engine
//! keeps it in file history (threat model D4/D5). A merged tree equal to a
//! tip's tree produces no commit (§4.3).

pub mod dmp;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use serde_json::Value;
use sync_crypto::CryptoProvider;
use sync_proto::batch::{tree_hash_input, valid_path, Class, Parent};
use sync_proto::Id16;

use crate::commit::{build_commit, Built, CommitError, ContentRef, Edit, Signer, Tree, TreeEntry, VaultCtx};

/// Per-device merge setting (spec §4.7; plan "stock-parity UX").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictMode {
    /// "Automatically merge" — default.
    #[default]
    AutoMerge,
    /// "Create conflict file".
    ConflictFile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeError {
    Commit(CommitError),
    /// A commit needed to find the base is not known locally.
    MissingCommit(Id16),
    /// The merged tree does not hash as planned (internal bug guard).
    Inconsistent,
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MergeError::Commit(e) => write!(f, "merge: {e}"),
            MergeError::MissingCommit(id) => write!(f, "merge: commit {id:?} not available"),
            MergeError::Inconsistent => f.write_str("merge: tree hash mismatch"),
        }
    }
}

impl std::error::Error for MergeError {}

impl From<CommitError> for MergeError {
    fn from(e: CommitError) -> Self {
        MergeError::Commit(e)
    }
}

pub type Result<T> = std::result::Result<T, MergeError>;

/// Parent links of known commits.
pub trait Dag {
    fn parents(&self, id: &Id16) -> Option<Vec<Id16>>;
}

impl Dag for BTreeMap<Id16, Vec<Id16>> {
    fn parents(&self, id: &Id16) -> Option<Vec<Id16>> {
        self.get(id).cloned()
    }
}

fn ancestors(dag: &dyn Dag, starts: &[Id16], inclusive: bool) -> Result<BTreeSet<Id16>> {
    let mut seen = BTreeSet::new();
    let mut q: VecDeque<Id16> = VecDeque::new();
    for s in starts {
        if inclusive {
            if seen.insert(*s) {
                q.push_back(*s);
            }
        } else {
            for p in dag.parents(s).ok_or(MergeError::MissingCommit(*s))? {
                if seen.insert(p) {
                    q.push_back(p);
                }
            }
        }
    }
    while let Some(id) = q.pop_front() {
        for p in dag.parents(&id).ok_or(MergeError::MissingCommit(id))? {
            if seen.insert(p) {
                q.push_back(p);
            }
        }
    }
    Ok(seen)
}

/// The merge base of `a` and `b`: a best common ancestor (one that is no
/// ancestor of another common ancestor). Several (criss-cross) → the
/// smallest commit_id (deterministic; the merge then may raise more
/// conflict files, it never loses data). `None` = unrelated roots (base =
/// empty tree).
pub fn merge_base(dag: &dyn Dag, a: Id16, b: Id16) -> Result<Option<Id16>> {
    let aa = ancestors(dag, &[a], true)?;
    let ab = ancestors(dag, &[b], true)?;
    let common: Vec<Id16> = aa.intersection(&ab).copied().collect();
    if common.is_empty() {
        return Ok(None);
    }
    let below = ancestors(dag, &common, false)?;
    Ok(common.into_iter().find(|c| !below.contains(c)))
}

/// One tip of the merge.
#[derive(Debug, Clone)]
pub struct Side<'a> {
    pub tree: &'a Tree,
    pub tip: Id16,
    /// Signed `ts_ms` of the tip commit (LWW).
    pub ts_ms: i64,
    /// Display name of the tip's device (conflict file names).
    pub device_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Local,
    Remote,
}

/// How a path was resolved (logged by the engine).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Only one side changed it (or both identically).
    Clean,
    /// diff-match-patch three-way merge.
    TextMerged,
    /// Conflict copy written at `copy`.
    ConflictCopy { copy: String },
    LastWriteWins,
    EditBeatsDelete,
    SettingsLocalOverRemote,
}

/// A version that does not survive verbatim; the engine keeps it in file
/// history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loser {
    pub path: String,
    pub origin: Origin,
    pub entry: TreeEntry,
}

/// A merged tree entry: an existing entry, or new bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Merged {
    Keep(TreeEntry),
    New { class: Class, mtime_ms: i64, bytes: Vec<u8>, sha256: [u8; 32] },
}

impl Merged {
    fn key(&self) -> (Class, [u8; 32]) {
        match self {
            Merged::Keep(e) => (e.class, e.sha256),
            Merged::New { class, sha256, .. } => (*class, *sha256),
        }
    }
}

/// What the engine does with a merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Merged tree = local tree: no commit, vault unchanged.
    UpToDate,
    /// Merged tree = remote tree: adopt remote tip, no commit.
    FastForward,
    /// Needs a merge commit with both tips as parents.
    Commit,
}

#[derive(Debug, Clone)]
pub struct Merge {
    pub tree: BTreeMap<String, Merged>,
    pub tree_hash: [u8; 32],
    pub action: Action,
    pub losers: Vec<Loser>,
    pub log: Vec<(String, Resolution)>,
}

fn same(a: Option<&TreeEntry>, b: Option<&TreeEntry>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.class == y.class && x.sha256 == y.sha256,
        _ => false,
    }
}

/// `true` if `a` beats `b` under last-write-wins.
fn lww_wins(a: &Side<'_>, b: &Side<'_>) -> bool {
    (a.ts_ms, a.tip.0) > (b.ts_ms, b.tip.0)
}

/// Device name as it may appear in a file name.
pub fn sanitize_device(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
        .take(64)
        .collect();
    let s = s.trim().trim_matches('.').trim().to_string();
    if s.is_empty() {
        "device".to_string()
    } else {
        s
    }
}

/// (year, month, day) of a day count since 1970-01-01 (proleptic
/// Gregorian; H. Hinnant's civil_from_days).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { yoe + era * 400 + 1 } else { yoe + era * 400 }, m, d)
}

/// `YYYYMMDDHHMM` (UTC) of a unix-ms timestamp.
pub fn stamp(ts_ms: i64) -> String {
    let secs = ts_ms.div_euclid(1000);
    let (y, mo, d) = civil(secs.div_euclid(86_400));
    let sod = secs.rem_euclid(86_400);
    format!("{y:04}{mo:02}{d:02}{:02}{:02}", sod / 3600, (sod % 3600) / 60)
}

/// `"<note> (Conflicted copy <device> YYYYMMDDHHMM).md"`; `n > 1` adds
/// ` n` inside the parentheses for collisions.
pub fn conflict_name(path: &str, device: &str, ts_ms: i64, n: u32) -> String {
    let (stem, ext) = match path.strip_suffix(".md") {
        Some(s) => (s, ".md"),
        None => (path, ""),
    };
    let dev = sanitize_device(device);
    let suffix = if n > 1 { format!(" {n}") } else { String::new() };
    format!("{stem} (Conflicted copy {dev} {}{suffix}){ext}", stamp(ts_ms))
}

/// Local keys on top of remote (deep); `None` if either is not a JSON
/// object (caller then keeps local).
pub fn merge_settings(local: &[u8], remote: &[u8]) -> Option<Vec<u8>> {
    fn deep(r: &mut serde_json::Map<String, Value>, l: &serde_json::Map<String, Value>) {
        for (k, lv) in l {
            if let (Some(Value::Object(ro)), Value::Object(lo)) = (r.get_mut(k), lv) {
                deep(ro, lo);
                continue;
            }
            r.insert(k.clone(), lv.clone());
        }
    }
    let l: Value = serde_json::from_slice(local).ok()?;
    let mut r: Value = serde_json::from_slice(remote).ok()?;
    match (&mut r, &l) {
        (Value::Object(ro), Value::Object(lo)) => deep(ro, lo),
        _ => return None,
    }
    let mut out = serde_json::to_vec_pretty(&r).ok()?;
    out.push(b'\n');
    Some(out)
}

/// Bytes of a tree entry (the engine wraps [`crate::commit::fetch_entry`]).
pub type Fetch<'f> = dyn FnMut(&TreeEntry) -> std::result::Result<Vec<u8>, CommitError> + 'f;

/// Merge two tips over their exact base tree.
pub fn merge_trees<P: CryptoProvider + ?Sized>(
    p: &P,
    base: &Tree,
    local: &Side<'_>,
    remote: &Side<'_>,
    mode: ConflictMode,
    fetch: &mut Fetch<'_>,
) -> Result<Merge> {
    let local_wins = lww_wins(local, remote);
    let (lose, win_o, lose_o) = if local_wins {
        (remote, Origin::Local, Origin::Remote)
    } else {
        (local, Origin::Remote, Origin::Local)
    };
    let paths: BTreeSet<&String> = base.0.keys().chain(local.tree.0.keys()).chain(remote.tree.0.keys()).collect();
    let mut tree: BTreeMap<String, Merged> = BTreeMap::new();
    let mut losers = Vec::new();
    let mut log = Vec::new();
    // (path, loser entry) — conflict copies are named in a second pass
    let mut copies: Vec<(String, TreeEntry)> = Vec::new();
    for path in paths {
        let b = base.0.get(path);
        let l = local.tree.0.get(path);
        let r = remote.tree.0.get(path);
        let lost = |e: &TreeEntry, o: Origin, losers: &mut Vec<Loser>| {
            losers.push(Loser { path: path.clone(), origin: o, entry: e.clone() })
        };
        let take = |tree: &mut BTreeMap<String, Merged>, e: Option<&TreeEntry>| {
            if let Some(e) = e {
                tree.insert(path.clone(), Merged::Keep(e.clone()));
            }
        };
        if same(l, r) || same(r, b) {
            take(&mut tree, l);
            continue;
        }
        if same(l, b) {
            take(&mut tree, r);
            continue;
        }
        // both sides changed, differently
        let (we, le) = if local_wins { (l, r) } else { (r, l) };
        match (we, le) {
            (Some(e), None) | (None, Some(e)) => {
                tree.insert(path.clone(), Merged::Keep(e.clone()));
                log.push((path.clone(), Resolution::EditBeatsDelete));
            }
            (Some(w), Some(x)) if w.class == Class::Markdown && x.class == Class::Markdown => {
                let merged = match (mode, b) {
                    (ConflictMode::AutoMerge, Some(be)) if be.class == Class::Markdown => {
                        let bb = fetch(be)?;
                        let wb = fetch(w)?;
                        let xb = fetch(x)?;
                        match (std::str::from_utf8(&bb), std::str::from_utf8(&wb), std::str::from_utf8(&xb)) {
                            (Ok(bs), Ok(ws), Ok(xs)) => dmp::merge3(bs, ws, xs).ok(),
                            _ => None,
                        }
                    }
                    _ => None,
                };
                match merged {
                    Some(text) => {
                        let bytes = text.into_bytes();
                        let sha256 = p.sha256(&bytes);
                        let mtime_ms = w.mtime_ms.max(x.mtime_ms);
                        tree.insert(path.clone(), Merged::New { class: Class::Markdown, mtime_ms, bytes, sha256 });
                        lost(w, win_o, &mut losers);
                        lost(x, lose_o, &mut losers);
                        log.push((path.clone(), Resolution::TextMerged));
                    }
                    None => {
                        tree.insert(path.clone(), Merged::Keep(w.clone()));
                        copies.push((path.clone(), x.clone()));
                    }
                }
            }
            (Some(w), Some(x)) if w.class == Class::SettingsJson && x.class == Class::SettingsJson => {
                let (le_, re_) = if local_wins { (w, x) } else { (x, w) };
                let lb = fetch(le_)?;
                let rb = fetch(re_)?;
                let out = merge_settings(&lb, &rb).unwrap_or(lb);
                let sha256 = p.sha256(&out);
                if sha256 == le_.sha256 {
                    tree.insert(path.clone(), Merged::Keep(le_.clone()));
                } else {
                    let mtime_ms = le_.mtime_ms.max(re_.mtime_ms);
                    tree.insert(path.clone(), Merged::New { class: Class::SettingsJson, mtime_ms, bytes: out, sha256 });
                    lost(le_, Origin::Local, &mut losers);
                }
                lost(re_, Origin::Remote, &mut losers);
                log.push((path.clone(), Resolution::SettingsLocalOverRemote));
            }
            (Some(w), Some(x)) => {
                tree.insert(path.clone(), Merged::Keep(w.clone()));
                lost(x, lose_o, &mut losers);
                log.push((path.clone(), Resolution::LastWriteWins));
            }
            (None, None) => unreachable!("same(None, None) handled above"),
        }
    }
    for (path, e) in copies {
        let mut n = 1;
        let copy = loop {
            let c = conflict_name(&path, &lose.device_name, lose.ts_ms, n);
            if !tree.contains_key(&c) && valid_path(&c) {
                break c;
            }
            n += 1;
            if n > 1000 {
                return Err(MergeError::Commit(CommitError::Tree("no free conflict file name")));
            }
        };
        tree.insert(copy.clone(), Merged::Keep(e.clone()));
        losers.push(Loser { path: path.clone(), origin: lose_o, entry: e });
        log.push((path, Resolution::ConflictCopy { copy }));
    }
    // One side added a file where the other side added a folder (`a` vs
    // `a/b.md`): a tree with both cannot be written to disk. The folder
    // stays, the file moves to a conflict-copy name (its bytes are kept, so
    // nothing is lost and the result is materialisable).
    let is_dir_prefix = |t: &BTreeMap<String, Merged>, k: &str| {
        let pre = format!("{k}/");
        t.range(pre.clone()..).next().is_some_and(|(n, _)| n.starts_with(&pre))
    };
    let clashing: Vec<String> = tree.keys().filter(|k| is_dir_prefix(&tree, k)).cloned().collect();
    for path in clashing {
        let Some(m) = tree.remove(&path) else { continue };
        let mut n = 1;
        let copy = loop {
            let c = conflict_name(&path, &lose.device_name, lose.ts_ms, n);
            if !tree.contains_key(&c) && valid_path(&c) && !is_dir_prefix(&tree, &c) {
                break c;
            }
            n += 1;
            if n > 1000 {
                return Err(MergeError::Commit(CommitError::Tree("no free conflict file name")));
            }
        };
        tree.insert(copy.clone(), m);
        log.push((path, Resolution::ConflictCopy { copy }));
    }
    let tree_hash = hash_merged(p, &tree)?;
    let action = if tree_hash == local.tree.hash(p)? {
        Action::UpToDate
    } else if tree_hash == remote.tree.hash(p)? {
        Action::FastForward
    } else {
        Action::Commit
    };
    Ok(Merge { tree, tree_hash, action, losers, log })
}

fn hash_merged<P: CryptoProvider + ?Sized>(p: &P, t: &BTreeMap<String, Merged>) -> Result<[u8; 32]> {
    let keys: Vec<(&str, Class, [u8; 32])> = t
        .iter()
        .map(|(k, m)| {
            let (c, s) = m.key();
            (k.as_str(), c, s)
        })
        .collect();
    let input = tree_hash_input(keys.iter().map(|(k, c, s)| (*k, *c, s))).map_err(CommitError::from)?;
    Ok(p.sha256(&input))
}

impl Merge {
    /// Edits turning `base0` (tree of the lowest-id parent) into the
    /// merged tree. Chunked entries are re-referenced, not re-uploaded.
    pub fn edits(&self, base0: &Tree, fetch: &mut Fetch<'_>) -> Result<Vec<Edit>> {
        let mut out = Vec::new();
        for (path, m) in &self.tree {
            let (class, sha) = m.key();
            if let Some(e) = base0.0.get(path) {
                if e.class == class && e.sha256 == sha {
                    continue;
                }
            }
            out.push(match m {
                Merged::New { class, mtime_ms, bytes, .. } => Edit::Put {
                    path: path.clone(),
                    class: *class,
                    mtime_ms: *mtime_ms,
                    content: bytes.clone(),
                },
                Merged::Keep(e) => match &e.content {
                    ContentRef::Chunks(ids) => Edit::PutChunks {
                        path: path.clone(),
                        class: e.class,
                        mtime_ms: e.mtime_ms,
                        size: e.size,
                        sha256: e.sha256,
                        chunks: ids.clone(),
                    },
                    ContentRef::Inline { .. } => Edit::Put {
                        path: path.clone(),
                        class: e.class,
                        mtime_ms: e.mtime_ms,
                        content: fetch(e)?,
                    },
                },
            });
        }
        for path in base0.0.keys() {
            if !self.tree.contains_key(path) {
                out.push(Edit::Delete { path: path.clone() });
            }
        }
        Ok(out)
    }
}

/// Seal the merge commit (parents = both tips). Only for
/// [`Action::Commit`]; checks the built tree hash equals the plan.
#[allow(clippy::too_many_arguments)]
pub fn commit_merge<P: CryptoProvider + ?Sized>(
    p: &P,
    v: &VaultCtx,
    signer: &Signer<'_, P::SigningKey>,
    ts_ms: i64,
    m: &Merge,
    local: (Parent, &Tree),
    remote: (Parent, &Tree),
    fetch: &mut Fetch<'_>,
) -> Result<Built> {
    if m.action != Action::Commit {
        return Err(MergeError::Commit(CommitError::Tree("merge needs no commit")));
    }
    let base0 = if local.0 < remote.0 { local.1 } else { remote.1 };
    let edits = m.edits(base0, fetch)?;
    let b = build_commit(p, v, signer, ts_ms, vec![local.0, remote.0], base0, edits)?;
    if b.batch.tree_hash != m.tree_hash {
        return Err(MergeError::Inconsistent);
    }
    Ok(b)
}
