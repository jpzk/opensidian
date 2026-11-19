// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! M4b: tip merge (spec §4.7) on the AWS-LC FIPS module over a dir
//! replica — exact DAG base, markdown diff-match-patch, conflict file with
//! the stock name, LWW (signed ts, commit-id tie-break), settings JSON
//! local-over-remote, edit beats delete, losers reported + kept in file
//! history, chunked attachments re-referenced, deterministic merge (no
//! commit when the tree equals a tip, both devices compute the same tree).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use sync_core::commit::{
    build_commit, fetch_commit, fetch_entry, push_commit, Built, CommitError, ContentRef, Edit, Signer, Tree,
    TreeEntry, VaultCtx,
};
use sync_core::history::{Config, Outcome, Reason, Retention, Store};
use sync_core::merge::{
    commit_merge, conflict_name, merge_base, merge_settings, merge_trees, stamp, Action, ConflictMode, Merge,
    MergeError, Merged, Origin, Resolution, Side,
};
use sync_core::transport::fs::FsTransport;
use sync_crypto::fips::{FipsProvider, FipsSigningKey};
use sync_crypto::{CryptoProvider, SigAlg};
use sync_proto::batch::{Class, Op, Where};
use sync_proto::container::Suite;
use sync_proto::Id16;

static N: AtomicU32 = AtomicU32::new(0);

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "osy-mg-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn fips() -> FipsProvider {
    FipsProvider::new().expect("FIPS module in FIPS mode")
}

fn rid(p: &FipsProvider) -> Id16 {
    let mut b = [0u8; 16];
    p.random(&mut b).unwrap();
    Id16(b)
}

struct Dev {
    id: Id16,
    key: FipsSigningKey,
    pk: Vec<u8>,
    name: &'static str,
}

fn device(p: &FipsProvider, name: &'static str) -> Dev {
    let (key, pk) = p.generate_signing_key(SigAlg::Ed25519).unwrap();
    Dev { id: rid(p), key, pk, name }
}

type Keys = BTreeMap<Id16, (SigAlg, Vec<u8>)>;

struct World {
    p: FipsProvider,
    v: VaultCtx,
    t: FsTransport,
    a: Dev,
    b: Dev,
    ks: Keys,
    dag: BTreeMap<Id16, Vec<Id16>>,
    trees: BTreeMap<Id16, Tree>,
    ts: BTreeMap<Id16, i64>,
}

const T0: i64 = 1_763_000_000_000;

fn put(path: &str, class: Class, content: &[u8]) -> Edit {
    Edit::Put { path: path.into(), class, mtime_ms: T0, content: content.to_vec() }
}

fn noise(n: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

const NOTE: &str = "# Plan\n\nline one\nline two\nline three\nline four\nline five\n";

impl World {
    fn new() -> World {
        let p = fips();
        let mut k = [0u8; 32];
        p.random(&mut k).unwrap();
        let v = VaultCtx { suite: Suite::Ed25519, vault_id: rid(&p), ns: rid(&p), k_obj: k, prev_k_obj: vec![] };
        let a = device(&p, "Laptop");
        let b = device(&p, "Phone");
        let ks = [&a, &b].iter().map(|d| (d.id, (SigAlg::Ed25519, d.pk.clone()))).collect();
        World {
            v,
            t: FsTransport::open(tmpdir("fs")).unwrap(),
            a,
            b,
            ks,
            dag: BTreeMap::new(),
            trees: BTreeMap::new(),
            ts: BTreeMap::new(),
            p,
        }
    }

    fn commit(&mut self, dev_b: bool, ts: i64, parents: &[Id16], edits: Vec<Edit>) -> Built {
        let mut ps: Vec<_> = parents
            .iter()
            .map(|id| sync_proto::batch::Parent { commit_id: *id, tree_hash: self.trees[id].hash(&self.p).unwrap() })
            .collect();
        ps.sort();
        let base = ps.first().map(|x| self.trees[&x.commit_id].clone()).unwrap_or_default();
        let d = if dev_b { &self.b } else { &self.a };
        let built = build_commit(&self.p, &self.v, &Signer { device_id: d.id, key: &d.key }, ts, ps, &base, edits).unwrap();
        self.record(&built, ts);
        built
    }

    fn record(&mut self, built: &Built, ts: i64) {
        push_commit(&self.t, &self.v, built).unwrap();
        self.dag.insert(built.id, built.batch.parents.iter().map(|x| x.commit_id).collect());
        self.trees.insert(built.id, built.tree.clone());
        self.ts.insert(built.id, ts);
    }

    fn side(&self, id: Id16, b_dev: bool) -> Side<'_> {
        Side {
            tree: &self.trees[&id],
            tip: id,
            ts_ms: self.ts[&id],
            device_name: if b_dev { self.b.name } else { self.a.name }.to_string(),
        }
    }

    fn bytes(&self, e: &TreeEntry) -> Vec<u8> {
        fetch_entry(&self.t, &self.p, &self.v, e, None, &self.ks).unwrap()
    }

    fn merge(&self, local: (Id16, bool), remote: (Id16, bool), mode: ConflictMode) -> Merge {
        let base = merge_base(&self.dag, local.0, remote.0).unwrap();
        let bt = base.map(|b| self.trees[&b].clone()).unwrap_or_default();
        let mut f = |e: &TreeEntry| fetch_entry(&self.t, &self.p, &self.v, e, None, &self.ks);
        merge_trees(&self.p, &bt, &self.side(local.0, local.1), &self.side(remote.0, remote.1), mode, &mut f).unwrap()
    }

    fn merged_bytes(&self, m: &Merge, path: &str) -> Vec<u8> {
        match &m.tree[path] {
            Merged::Keep(e) => self.bytes(e),
            Merged::New { bytes, .. } => bytes.clone(),
        }
    }

    fn seal_merge(&mut self, m: &Merge, local: Id16, remote: Id16, dev_b: bool, ts: i64) -> Built {
        let lp = sync_proto::batch::Parent { commit_id: local, tree_hash: self.trees[&local].hash(&self.p).unwrap() };
        let rp = sync_proto::batch::Parent { commit_id: remote, tree_hash: self.trees[&remote].hash(&self.p).unwrap() };
        let d = if dev_b { &self.b } else { &self.a };
        let built = {
            let (t, p, v, ks) = (&self.t, &self.p, &self.v, &self.ks);
            let mut f = |e: &TreeEntry| fetch_entry(t, p, v, e, None, ks);
            commit_merge(
                &self.p,
                &self.v,
                &Signer { device_id: d.id, key: &d.key },
                ts,
                m,
                (lp, &self.trees[&local]),
                (rp, &self.trees[&remote]),
                &mut f,
            )
            .unwrap()
        };
        self.record(&built, ts);
        built
    }
}

/// root (A) → A1 (A, T0+1m) and B1 (B, T0+2m): B is the LWW winner.
fn diverge(w: &mut World) -> (Id16, Id16, Id16) {
    let att = noise(2 * 1024 * 1024 + 17, 7);
    let root = w.commit(
        false,
        T0,
        &[],
        vec![
            put("note.md", Class::Markdown, NOTE.as_bytes()),
            put("same.md", Class::Markdown, b"same line\n"),
            put("img.png", Class::Other, b"png v0"),
            put("gone.md", Class::Markdown, b"keep me\n"),
            put("att/big.bin", Class::Other, &att),
            put(".obsidian/app.json", Class::SettingsJson, br#"{"a":1,"nested":{"x":1,"y":1}}"#),
        ],
    );
    let a1 = w.commit(
        false,
        T0 + 60_000,
        &[root.id],
        vec![
            put("note.md", Class::Markdown, NOTE.replace("line one", "line ONE (laptop)").as_bytes()),
            put("img.png", Class::Other, b"png laptop"),
            Edit::Delete { path: "gone.md".into() },
            put(".obsidian/app.json", Class::SettingsJson, br#"{"a":2,"nested":{"x":2},"onlyA":true}"#),
        ],
    );
    let b1 = w.commit(
        true,
        T0 + 120_000,
        &[root.id],
        vec![
            put("note.md", Class::Markdown, NOTE.replace("line five", "line FIVE (phone)").as_bytes()),
            put("img.png", Class::Other, b"png phone"),
            put("gone.md", Class::Markdown, b"keep me\nedited on phone\n"),
            put("att/big.bin", Class::Other, &noise(3 * 1024 * 1024, 9)),
            put(".obsidian/app.json", Class::SettingsJson, br#"{"a":3,"nested":{"y":3},"onlyB":true}"#),
        ],
    );
    (root.id, a1.id, b1.id)
}

#[test]
fn merge_base_is_exact_and_deterministic() {
    let x = |n: u8| Id16([n; 16]);
    let mut dag: BTreeMap<Id16, Vec<Id16>> = BTreeMap::new();
    dag.insert(x(1), vec![]);
    dag.insert(x(2), vec![x(1)]);
    dag.insert(x(3), vec![x(1)]);
    dag.insert(x(4), vec![x(2)]);
    assert_eq!(merge_base(&dag, x(4), x(3)).unwrap(), Some(x(1)));
    assert_eq!(merge_base(&dag, x(4), x(2)).unwrap(), Some(x(2)), "ancestor is its own base");
    assert_eq!(merge_base(&dag, x(4), x(4)).unwrap(), Some(x(4)));
    // criss-cross: 5 = merge(2,3), 6 = merge(3,2) → bases {2,3} → smallest
    dag.insert(x(5), vec![x(2), x(3)]);
    dag.insert(x(6), vec![x(2), x(3)]);
    assert_eq!(merge_base(&dag, x(5), x(6)).unwrap(), Some(x(2)));
    assert_eq!(merge_base(&dag, x(6), x(5)).unwrap(), Some(x(2)));
    // unrelated roots
    dag.insert(x(9), vec![]);
    assert_eq!(merge_base(&dag, x(9), x(4)).unwrap(), None);
    // unknown commit → error, never a guessed base
    dag.insert(x(7), vec![x(8)]);
    assert_eq!(merge_base(&dag, x(7), x(4)), Err(MergeError::MissingCommit(x(8))));
}

#[test]
fn names_and_stamps() {
    assert_eq!(stamp(0), "197001010000");
    assert_eq!(stamp(T0), "202511130213");
    assert_eq!(stamp(1_709_164_800_000), "202402290000");
    assert_eq!(stamp(-1), "196912312359");
    assert_eq!(
        conflict_name("dir/My note.md", "Phone", T0, 1),
        "dir/My note (Conflicted copy Phone 202511130213).md"
    );
    assert_eq!(conflict_name("a.md", "x/y:z", T0, 2), "a (Conflicted copy x-y-z 202511130213 2).md");
    assert_eq!(conflict_name("a.md", " \u{7}.. ", T0, 1), "a (Conflicted copy - 202511130213).md");
    assert_eq!(conflict_name("a.md", "", T0, 1), "a (Conflicted copy device 202511130213).md");
}

#[test]
fn settings_json_local_keys_on_top() {
    let out = merge_settings(br#"{"a":2,"n":{"x":2},"l":1}"#, br#"{"a":3,"n":{"y":3},"r":1}"#).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v, serde_json::json!({"a":2,"n":{"x":2,"y":3},"l":1,"r":1}));
    assert!(merge_settings(b"[1]", b"{}").is_none());
    assert!(merge_settings(b"{", b"{}").is_none());
}

#[test]
fn tip_merge_all_rules_and_commit() {
    let mut w = World::new();
    let (root, a1, b1) = diverge(&mut w);
    assert_eq!(merge_base(&w.dag, a1, b1).unwrap(), Some(root));

    // device A merges (local A1, remote B1)
    let ma = w.merge((a1, false), (b1, true), ConflictMode::AutoMerge);
    assert_eq!(ma.action, Action::Commit);
    let log: BTreeMap<_, _> = ma.log.iter().cloned().collect();
    // markdown: disjoint edits → DMP merge with both
    assert_eq!(log["note.md"], Resolution::TextMerged);
    let note = String::from_utf8(w.merged_bytes(&ma, "note.md")).unwrap();
    assert_eq!(
        note,
        NOTE.replace("line one", "line ONE (laptop)").replace("line five", "line FIVE (phone)")
    );
    // other file: B's tip is later → B wins on both devices
    assert_eq!(log["img.png"], Resolution::LastWriteWins);
    assert_eq!(w.merged_bytes(&ma, "img.png"), b"png phone");
    // edit beats delete
    assert_eq!(log["gone.md"], Resolution::EditBeatsDelete);
    assert_eq!(w.merged_bytes(&ma, "gone.md"), b"keep me\nedited on phone\n");
    // one-sided change (chunked attachment): clean, B's chunks re-used
    assert!(!log.contains_key("att/big.bin"));
    assert_eq!(ma.tree["att/big.bin"], Merged::Keep(w.trees[&b1].0["att/big.bin"].clone()));
    // untouched file stays
    assert_eq!(ma.tree["same.md"], Merged::Keep(w.trees[&root].0["same.md"].clone()));
    // settings: local (A) keys on top of remote (B), deep
    assert_eq!(log[".obsidian/app.json"], Resolution::SettingsLocalOverRemote);
    let s: serde_json::Value = serde_json::from_slice(&w.merged_bytes(&ma, ".obsidian/app.json")).unwrap();
    assert_eq!(s, serde_json::json!({"a":2,"nested":{"x":2,"y":3},"onlyA":true,"onlyB":true}));

    // losers: everything that did not survive verbatim is reported
    let lost: Vec<(&str, Origin)> = ma.losers.iter().map(|l| (l.path.as_str(), l.origin)).collect();
    assert!(lost.contains(&("img.png", Origin::Local)), "{lost:?}");
    assert!(lost.contains(&("note.md", Origin::Local)) && lost.contains(&("note.md", Origin::Remote)));
    assert!(lost.contains(&(".obsidian/app.json", Origin::Local)) && lost.contains(&(".obsidian/app.json", Origin::Remote)));
    let png_loser = ma.losers.iter().find(|l| l.path == "img.png").unwrap();
    assert_eq!(w.bytes(&png_loser.entry), b"png laptop");

    // device B merges the same tips the other way round: same tree for
    // every path except the (local-by-design) settings file
    let mb = w.merge((b1, true), (a1, false), ConflictMode::AutoMerge);
    for path in ma.tree.keys() {
        if path != ".obsidian/app.json" {
            assert_eq!(w.merged_bytes(&mb, path), w.merged_bytes(&ma, path), "{path}");
        }
    }
    let sb: serde_json::Value = serde_json::from_slice(&w.merged_bytes(&mb, ".obsidian/app.json")).unwrap();
    assert_eq!(sb, serde_json::json!({"a":3,"nested":{"x":2,"y":3},"onlyA":true,"onlyB":true}));
    // merging twice is deterministic
    let ma2 = w.merge((a1, false), (b1, true), ConflictMode::AutoMerge);
    assert_eq!(ma2.tree_hash, ma.tree_hash);

    // seal the merge commit on A, read it back from the replica
    let m = w.seal_merge(&ma, a1, b1, false, T0 + 180_000);
    assert_eq!(m.batch.tree_hash, ma.tree_hash);
    assert_eq!(m.batch.parents.len(), 2);
    let back = fetch_commit(&w.t, &w.p, &w.v, &m.id, &w.ks).unwrap();
    assert_eq!(back, m.batch);
    let base0 = w.trees[&m.batch.parents[0].commit_id].clone();
    let mut rebuilt = base0.clone();
    rebuilt.apply(&back).unwrap();
    assert_eq!(rebuilt.hash(&w.p).unwrap(), ma.tree_hash);
    for (path, e) in &rebuilt.0 {
        assert_eq!(w.bytes(e), w.merged_bytes(&ma, path), "{path}");
    }
    // the chunked attachment is re-referenced (same chunk ids), not re-uploaded
    let b_big = &w.trees[&b1].0["att/big.bin"];
    assert_eq!(rebuilt.0["att/big.bin"].content, b_big.content);
    if base0.0["att/big.bin"].sha256 != b_big.sha256 {
        let ch = back.changes.iter().find(|c| c.path == "att/big.bin").unwrap();
        match (&ch.op, &b_big.content) {
            (Op::Put(p), ContentRef::Chunks(ids)) => assert_eq!(p.loc, Where::Chunks(ids.clone())),
            other => panic!("{other:?}"),
        }
    }

    // no ping-pong: B merging (B1, M) fast-forwards, A merging (M, B1) and
    // (M, A1) is up to date — no further commits
    assert_eq!(w.merge((b1, true), (m.id, false), ConflictMode::AutoMerge).action, Action::FastForward);
    assert_eq!(w.merge((m.id, false), (b1, true), ConflictMode::AutoMerge).action, Action::UpToDate);
    assert_eq!(w.merge((m.id, false), (a1, false), ConflictMode::AutoMerge).action, Action::UpToDate);
    // sealing an up-to-date merge is refused
    let up = w.merge((m.id, false), (b1, true), ConflictMode::AutoMerge);
    let r = {
        let lp = m.parent();
        let (t, p, v, ks) = (&w.t, &w.p, &w.v, &w.ks);
        let mut f = |e: &TreeEntry| fetch_entry(t, p, v, e, None, ks);
        commit_merge(
            p,
            v,
            &Signer { device_id: w.a.id, key: &w.a.key },
            T0,
            &up,
            (lp, &w.trees[&m.id]),
            (lp, &w.trees[&m.id]),
            &mut f,
        )
    };
    assert!(matches!(r, Err(MergeError::Commit(CommitError::Tree(_)))));
}

#[test]
fn overlapping_edit_creates_conflict_file_with_stock_name() {
    let mut w = World::new();
    let root = w.commit(false, T0, &[], vec![put("dir/Plan.md", Class::Markdown, NOTE.as_bytes())]);
    let a1 = w.commit(false, T0 + 60_000, &[root.id], vec![put("dir/Plan.md", Class::Markdown, NOTE.replace("line three", "line 3 laptop").as_bytes())]);
    let b1 = w.commit(true, T0 + 120_000, &[root.id], vec![put("dir/Plan.md", Class::Markdown, NOTE.replace("line three", "line 3 phone").as_bytes())]);
    for (local, remote) in [((a1.id, false), (b1.id, true)), ((b1.id, true), (a1.id, false))] {
        let m = w.merge(local, remote, ConflictMode::AutoMerge);
        // winner (B, later) keeps the path; loser (A) → conflict copy named
        // after the LOSER's device and tip time
        let copy = format!("dir/Plan (Conflicted copy Laptop {}).md", stamp(T0 + 60_000));
        assert_eq!(copy, "dir/Plan (Conflicted copy Laptop 202511130214).md");
        assert_eq!(m.log, vec![("dir/Plan.md".to_string(), Resolution::ConflictCopy { copy: copy.clone() })]);
        assert_eq!(w.merged_bytes(&m, "dir/Plan.md"), NOTE.replace("line three", "line 3 phone").as_bytes());
        assert_eq!(w.merged_bytes(&m, &copy), NOTE.replace("line three", "line 3 laptop").as_bytes());
        assert_eq!(m.losers.len(), 1);
        assert_eq!(m.losers[0].path, "dir/Plan.md");
        assert_eq!(m.action, Action::Commit);
    }
    // the conflict copy survives a round trip through a merge commit
    let m = w.merge((a1.id, false), (b1.id, true), ConflictMode::AutoMerge);
    let c = w.seal_merge(&m, a1.id, b1.id, false, T0 + 180_000);
    assert!(c.tree.0.contains_key("dir/Plan (Conflicted copy Laptop 202511130214).md"));
}

#[test]
fn conflict_file_mode_and_add_add_and_binary_markdown() {
    let mut w = World::new();
    let root = w.commit(false, T0, &[], vec![put("n.md", Class::Markdown, NOTE.as_bytes())]);
    let a1 = w.commit(
        false,
        T0 + 60_000,
        &[root.id],
        vec![
            put("n.md", Class::Markdown, NOTE.replace("line one", "ONE").as_bytes()),
            put("new.md", Class::Markdown, b"added on laptop\n"),
            put("bin.md", Class::Markdown, &[0xff, 0xfe, 0x00]),
        ],
    );
    let b1 = w.commit(
        true,
        T0 + 120_000,
        &[root.id],
        vec![
            put("n.md", Class::Markdown, NOTE.replace("line five", "FIVE").as_bytes()),
            put("new.md", Class::Markdown, b"added on phone\n"),
            put("bin.md", Class::Markdown, b"text\n"),
        ],
    );
    // "Create conflict file": disjoint edits are NOT auto-merged
    let m = w.merge((a1.id, false), (b1.id, true), ConflictMode::ConflictFile);
    let log: BTreeMap<_, _> = m.log.iter().cloned().collect();
    assert!(matches!(&log["n.md"], Resolution::ConflictCopy { copy } if copy == "n (Conflicted copy Laptop 202511130214).md"));
    // add/add without base → conflict copy in both modes
    let m2 = w.merge((a1.id, false), (b1.id, true), ConflictMode::AutoMerge);
    let log2: BTreeMap<_, _> = m2.log.iter().cloned().collect();
    assert_eq!(log2["n.md"], Resolution::TextMerged);
    assert!(matches!(&log2["new.md"], Resolution::ConflictCopy { .. }));
    // non-UTF-8 markdown → conflict copy (bin.md has no base either; no crash)
    assert!(matches!(&log2["bin.md"], Resolution::ConflictCopy { .. }));
    assert_eq!(w.merged_bytes(&m2, "new.md"), b"added on phone\n");
    // name collision: an existing copy name gets " 2"
    let taken = "new (Conflicted copy Laptop 202511130214).md";
    let a2 = w.commit(false, T0 + 60_000, &[a1.id], vec![put(taken, Class::Markdown, b"older copy\n")]);
    let m3 = w.merge((a2.id, false), (b1.id, true), ConflictMode::AutoMerge);
    let log3: BTreeMap<_, _> = m3.log.iter().cloned().collect();
    assert_eq!(
        log3["new.md"],
        Resolution::ConflictCopy { copy: "new (Conflicted copy Laptop 202511130214 2).md".into() }
    );
    assert_eq!(w.merged_bytes(&m3, taken), b"older copy\n");
}

#[test]
fn lww_tie_breaks_on_commit_id() {
    let mut w = World::new();
    let root = w.commit(false, T0, &[], vec![put("x.bin", Class::Other, b"0")]);
    let a1 = w.commit(false, T0 + 5, &[root.id], vec![put("x.bin", Class::Other, b"a")]);
    let b1 = w.commit(true, T0 + 5, &[root.id], vec![put("x.bin", Class::Other, b"b")]);
    let want: &[u8] = if a1.id > b1.id { b"a" } else { b"b" };
    for (l, r) in [((a1.id, false), (b1.id, true)), ((b1.id, true), (a1.id, false))] {
        let m = w.merge(l, r, ConflictMode::AutoMerge);
        assert_eq!(w.merged_bytes(&m, "x.bin"), want);
    }
    // both deleted / both identical → clean
    let a2 = w.commit(false, T0 + 9, &[a1.id], vec![Edit::Delete { path: "x.bin".into() }]);
    let b2 = w.commit(true, T0 + 9, &[b1.id], vec![Edit::Delete { path: "x.bin".into() }]);
    let m = w.merge((a2.id, false), (b2.id, true), ConflictMode::AutoMerge);
    assert!(m.tree.is_empty() && m.log.is_empty() && m.losers.is_empty());
}

#[test]
fn losers_are_kept_in_file_history_without_moving_the_head() {
    let app = tmpdir("app");
    let vault = tmpdir("vault");
    std::fs::create_dir_all(&vault).unwrap();
    std::fs::write(vault.join("n.md"), b"local bytes\n").unwrap();
    let mut s = Store::open(&app, "v1", &vault, Config::default()).unwrap();
    assert!(matches!(s.record_own_save("n.md", 1_000).unwrap(), Outcome::Stored(_)));
    let head = s.head("n.md").cloned();
    let o = s.record_sync_loser("n.md", b"remote loser\n", 2_000).unwrap();
    let e = match o {
        Outcome::Stored(e) => e,
        other => panic!("{other:?}"),
    };
    assert_eq!(e.reason, Reason::SyncLoser);
    assert_eq!(s.head("n.md").cloned(), head, "head unchanged");
    assert_eq!(s.record_sync_loser("n.md", b"remote loser\n", 3_000).unwrap(), Outcome::Unchanged);
    assert_eq!(s.read_version(e.seq).unwrap(), b"remote loser\n");
    assert_eq!(s.versions("n.md").len(), 2);
    // persisted + replayed; thinning keeps the live head and still works
    drop(s);
    let mut s = Store::open(&app, "v1", &vault, Config::default()).unwrap();
    assert_eq!(s.head("n.md").cloned(), head);
    assert_eq!(s.versions("n.md").first().unwrap().reason, Reason::SyncLoser);
    s.thin(400 * 86_400_000, &Retention::default()).unwrap();
    assert_eq!(s.head("n.md").cloned(), head);
}

#[test]
fn file_and_folder_at_the_same_path_never_both_land_in_the_tree() {
    let mut w = World::new();
    let root = w.commit(false, T0, &[], vec![put("keep.md", Class::Markdown, b"k\n")]);
    // laptop creates a note "Ideas.md"; the phone creates a folder of that name
    let a1 = w.commit(false, T0 + 60_000, &[root.id], vec![put("Ideas.md", Class::Markdown, b"file\n")]);
    let b1 = w.commit(true, T0 + 120_000, &[root.id], vec![put("Ideas.md/one.md", Class::Markdown, b"in folder\n")]);
    for mode in [ConflictMode::AutoMerge, ConflictMode::ConflictFile] {
        let m = w.merge((a1.id, false), (b1.id, true), mode);
        let keys: Vec<&String> = m.tree.keys().collect();
        assert!(!m.tree.contains_key("Ideas.md"), "file moved out of the folder's way: {keys:?}");
        assert!(m.tree.contains_key("Ideas.md/one.md"));
        let log: BTreeMap<_, _> = m.log.iter().cloned().collect();
        let Resolution::ConflictCopy { copy } = &log["Ideas.md"] else { panic!("{log:?}") };
        assert_eq!(w.merged_bytes(&m, copy), b"file\n", "the file's bytes are kept");
        for k in m.tree.keys() {
            assert!(!m.tree.keys().any(|o| o.starts_with(&format!("{k}/"))), "{k} is both file and folder");
        }
    }
}
