// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! M4c: the sync engine on the OpenSSL 3 FIPS provider — replicas as equal
//! peers synced by set union (spec §4.8). Three replicas (a real
//! opensidian-sync-server over HTTP + two dir replicas), two devices with
//! their own engine state. Devices only ever talk to SOME replicas, the
//! union still converges: same object set everywhere, same head tree.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use sync_core::commit::{build_commit, push_commit, Edit, Signer, Tree, VaultCtx};
use sync_core::engine::{Engine, EngineError, Report};
use sync_core::merge::{Action, Loser, Origin};
use sync_core::object::{seal, Compress, ObjectCtx};
use sync_core::transport::fs::FsTransport;
use sync_core::transport::http::{HttpTransport, Roots};
use sync_core::transport::{Tokens, Transport};
use sync_crypto::fips::{FipsProvider, FipsSigningKey};
use sync_crypto::{CryptoProvider, SigAlg};
use sync_proto::api::{ListQuery, ObjKind};
use sync_proto::batch::{Class, Parent, BATCH_SIG_DOMAIN};
use sync_proto::container::{Kind, Suite};
use sync_proto::Id16;
use sync_server::{serve, App, Config};

static N: AtomicU32 = AtomicU32::new(0);

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("osy-en-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    d
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
}

fn device(p: &FipsProvider) -> Dev {
    let (key, pk) = p.generate_signing_key(SigAlg::Ed25519).unwrap();
    Dev { id: rid(p), key, pk }
}

fn signer(d: &Dev) -> Signer<'_, FipsSigningKey> {
    Signer { device_id: d.id, key: &d.key }
}

type Keys = BTreeMap<Id16, (SigAlg, Vec<u8>)>;

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

const T0: i64 = 1_763_000_000_000;
const NOTE: &str = "# Plan\n\nline one\nline two\nline three\nline four\nline five\n";

/// Running sync-server (kept alive by the runtime) + an enrolled client.
struct Server {
    _rt: tokio::runtime::Runtime,
    t: HttpTransport,
    ns: Id16,
}

fn server(p: &FipsProvider) -> Server {
    let mut cfg = Config::new(tmpdir("srv"));
    cfg.anon_rate_per_min = 10_000;
    let app = Arc::new(App::open(cfg).expect("app"));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let l = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = l.local_addr().unwrap().port();
    let a = app.clone();
    rt.spawn(async move { serve(a, l).await });
    let t = HttpTransport::new(&format!("http://127.0.0.1:{port}"), Roots::System).unwrap();
    let (_k, pk) = p.generate_signing_key(SigAlg::Ed25519).unwrap();
    let tok = app.create_enrollment(None, 600).unwrap();
    let e = t.enroll(&tok, &pk).expect("enroll");
    t.set_tokens(Tokens::from(&e));
    Server { _rt: rt, t, ns: e.ns }
}

fn ids(t: &dyn Transport, ns: &Id16) -> BTreeSet<Id16> {
    let r = t.list(ns, ListQuery { after: 0, wait: 0, limit: 1000 }).unwrap();
    r.items.iter().filter(|i| i.kind == ObjKind::Obj).map(|i| i.id).collect()
}

fn text(e: &mut Engine<'_, FipsProvider>, ks: &Keys, path: &str) -> Option<Vec<u8>> {
    let t = e.head_tree().unwrap();
    t.0.get(path).map(|x| e.read(x, ks).unwrap())
}

fn hash(p: &FipsProvider, t: &Tree) -> [u8; 32] {
    t.hash(p).unwrap()
}

fn clean(r: &Report) {
    assert!(r.rejected.is_empty(), "rejected: {:?}", r.rejected);
    assert!(r.replica_errors.is_empty(), "replica errors: {:?}", r.replica_errors);
    assert!(r.invalid.is_empty(), "invalid: {:?}", r.invalid);
}

#[test]
fn three_replicas_converge_by_set_union() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let srv = server(&p);
    let mut k = [0u8; 32];
    p.random(&mut k).unwrap();
    let v = VaultCtx { suite: Suite::Ed25519, vault_id: rid(&p), ns: srv.ns, k_obj: k };
    let d1 = FsTransport::open(tmpdir("d1")).unwrap();
    let d2 = FsTransport::open(tmpdir("d2")).unwrap();
    let (da, db) = (device(&p), device(&p));
    let ks: Keys = [&da, &db].iter().map(|d| (d.id, (SigAlg::Ed25519, d.pk.clone()))).collect();
    let (dir_a, dir_b) = (tmpdir("ea"), tmpdir("eb"));
    let mut a = Engine::open(&p, v.clone(), &dir_a, &ks).unwrap();
    let mut b = Engine::open(&p, v.clone(), &dir_b, &ks).unwrap();
    a.names.insert(da.id, "Laptop".into());
    a.names.insert(db.id, "Phone".into());
    b.names = a.names.clone();
    let lost: RefCell<Vec<(String, Origin, Vec<u8>)>> = RefCell::new(Vec::new());
    let mut hook = |l: &Loser, bytes: &[u8]| lost.borrow_mut().push((l.path.clone(), l.origin, bytes.to_vec()));

    // second engine on the same state dir is refused
    assert_eq!(Engine::open(&p, v.clone(), &dir_a, &ks).unwrap_err(), EngineError::Busy);

    // 1. A creates the vault content (incl. a chunked attachment) and pushes to srv + d1
    let att = noise(5 * 1024 * 1024 + 3, 11);
    let c1 = a
        .commit_local(
            &signer(&da),
            &ks,
            T0,
            vec![put("note.md", Class::Markdown, NOTE.as_bytes()), put("bin.dat", Class::Other, b"v0"), put("att.bin", Class::Other, &att)],
        )
        .unwrap();
    let r = a.sync(&[&srv.t, &d1], &signer(&da), &ks, T0, &mut hook).unwrap();
    clean(&r);
    assert_eq!(r.pushed, 2 * 3, "1 batch + 2 chunks to each of 2 replicas");
    assert_eq!(ids(&srv.t, &v.ns), a.objects());
    assert_eq!(ids(&d1, &v.ns), a.objects());

    // B only sees d1 -> fast-forwards to A's root, reads the chunked file
    let r = b.sync(&[&d1], &signer(&db), &ks, T0, &mut hook).unwrap();
    clean(&r);
    assert_eq!(r.pulled, 3);
    assert_eq!(b.head(), Some(c1));
    assert_eq!(text(&mut b, &ks, "att.bin").unwrap(), att);

    // 2. concurrent offline edits: A line one + bin.dat, B line five + bin.dat
    let na = NOTE.replace("line one", "line ONE (laptop)");
    let nb = NOTE.replace("line five", "line FIVE (phone)");
    a.commit_local(&signer(&da), &ks, T0 + 1000, vec![put("note.md", Class::Markdown, na.as_bytes()), put("bin.dat", Class::Other, b"a1")]).unwrap();
    b.commit_local(&signer(&db), &ks, T0 + 2000, vec![put("note.md", Class::Markdown, nb.as_bytes()), put("bin.dat", Class::Other, b"b1")]).unwrap();
    // A -> srv only; B -> d2 only (disjoint replicas)
    clean(&a.sync(&[&srv.t], &signer(&da), &ks, T0 + 3000, &mut hook).unwrap());
    clean(&b.sync(&[&d2], &signer(&db), &ks, T0 + 3000, &mut hook).unwrap());
    // A syncs everything: pulls B's commit from d2, merges, pushes the union everywhere
    let r = a.sync(&[&srv.t, &d1, &d2], &signer(&da), &ks, T0 + 4000, &mut hook).unwrap();
    clean(&r);
    assert_eq!(r.folded.len(), 1);
    assert_eq!(r.folded[0].action, Action::Commit);
    let m = r.folded[0].merge_commit.unwrap();
    assert_eq!(a.head(), Some(m));
    let merged = String::from_utf8(text(&mut a, &ks, "note.md").unwrap()).unwrap();
    assert!(merged.contains("line ONE (laptop)") && merged.contains("line FIVE (phone)"), "{merged}");
    assert_eq!(text(&mut a, &ks, "bin.dat").unwrap(), b"b1", "LWW: B's tip is newer");
    assert!(lost.borrow().iter().any(|(pth, o, by)| pth == "bin.dat" && *o == Origin::Local && by == b"a1"), "loser kept");
    let all = a.objects();
    for (n, t) in [("srv", &srv.t as &dyn Transport), ("d1", &d1), ("d2", &d2)] {
        assert_eq!(ids(t, &v.ns), all, "{n} holds the union");
    }
    // B syncs only srv: fast-forward to the merge commit, same tree, no new commit
    let r = b.sync(&[&srv.t], &signer(&db), &ks, T0 + 5000, &mut hook).unwrap();
    clean(&r);
    assert_eq!(b.head(), Some(m));
    assert_eq!(r.pushed, 0);
    let (ta, tb) = (a.head_tree().unwrap(), b.head_tree().unwrap());
    assert_eq!(hash(&p, &ta), hash(&p, &tb));
    // a second pass is a no-op everywhere
    let r = a.sync(&[&srv.t, &d1, &d2], &signer(&da), &ks, T0 + 6000, &mut hook).unwrap();
    clean(&r);
    assert_eq!((r.pulled, r.pushed, r.folded.len()), (0, 0, 0));

    // 3. both merge the same tips independently -> same tree, no ping-pong:
    //    the other merge is absorbed and becomes a parent of the next commit
    a.commit_local(&signer(&da), &ks, T0 + 7000, vec![put("a.md", Class::Markdown, b"from a\n")]).unwrap();
    b.commit_local(&signer(&db), &ks, T0 + 7000, vec![put("b.md", Class::Markdown, b"from b\n")]).unwrap();
    clean(&a.sync(&[&d1], &signer(&da), &ks, T0 + 8000, &mut hook).unwrap());
    clean(&b.sync(&[&d2], &signer(&db), &ks, T0 + 8000, &mut hook).unwrap());
    let ra = a.sync(&[&d2], &signer(&da), &ks, T0 + 9000, &mut hook).unwrap(); // A merges -> M_A on d2
    let rb = b.sync(&[&d1], &signer(&db), &ks, T0 + 9000, &mut hook).unwrap(); // B merges -> M_B on d1
    clean(&ra);
    clean(&rb);
    let (ma, mb) = (ra.folded[0].merge_commit.unwrap(), rb.folded[0].merge_commit.unwrap());
    assert_ne!(ma, mb);
    let n_before = b.commit_count();
    let r = b.sync(&[&d2], &signer(&db), &ks, T0 + 10_000, &mut hook).unwrap();
    clean(&r);
    assert_eq!(r.folded.len(), 1);
    assert_eq!(r.folded[0].action, Action::UpToDate);
    assert_eq!(b.head(), Some(mb), "same tree -> no commit");
    assert_eq!(b.commit_count(), n_before + 1, "only M_A arrived, nothing new built");
    assert!(b.absorbed().contains(&ma));
    assert_eq!(b.tips().len(), 2);
    // re-sync does not fold M_A again
    assert!(b.sync(&[&d2], &signer(&db), &ks, T0 + 10_500, &mut hook).unwrap().folded.is_empty());
    let c = b.commit_local(&signer(&db), &ks, T0 + 11_000, vec![put("b.md", Class::Markdown, b"from b, again\n")]).unwrap();
    assert!(b.absorbed().is_empty());
    assert_eq!(b.tips(), vec![c], "absorbed tip is now a parent");
    clean(&b.sync(&[&srv.t, &d1, &d2], &signer(&db), &ks, T0 + 12_000, &mut hook).unwrap());
    let r = a.sync(&[&srv.t], &signer(&da), &ks, T0 + 13_000, &mut hook).unwrap();
    clean(&r);
    assert_eq!(a.head(), Some(c));
    assert_eq!(a.tips(), vec![c]);
    assert_eq!(text(&mut a, &ks, "b.md").unwrap(), b"from b, again\n");
    assert_eq!(text(&mut a, &ks, "a.md").unwrap(), b"from a\n");

    // 4. restart: the state dir reopens with the same head + object set
    let (head_a, objs_a) = (a.head(), a.objects());
    drop(a);
    let mut a = Engine::open(&p, v.clone(), &dir_a, &ks).unwrap();
    assert_eq!((a.head(), a.objects()), (head_a, objs_a));
    assert_eq!(hash(&p, &a.head_tree().unwrap()), hash(&p, &b.head_tree().unwrap()));

    assert!(!lost.borrow().is_empty());
}

#[test]
fn bad_objects_are_rejected_and_not_propagated() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let srv = server(&p);
    let mut k = [0u8; 32];
    p.random(&mut k).unwrap();
    let v = VaultCtx { suite: Suite::Ed25519, vault_id: rid(&p), ns: srv.ns, k_obj: k };
    let d1 = FsTransport::open(tmpdir("bad")).unwrap();
    let (da, evil) = (device(&p), device(&p));
    let ks: Keys = [(da.id, (SigAlg::Ed25519, da.pk.clone()))].into_iter().collect();
    let mut a = Engine::open(&p, v.clone(), tmpdir("ebad"), &ks).unwrap();
    let mut hook = |_: &Loser, _: &[u8]| {};
    let c1 = a.commit_local(&signer(&da), &ks, T0, vec![put("n.md", Class::Markdown, b"hi\n")]).unwrap();

    // a container with a valid header whose ciphertext was tampered with
    let g = rid(&p);
    let mut bad = seal(&p, &k, &ObjectCtx { suite: v.suite, vault_id: v.vault_id, obj_id: g }, Kind::Chunk, b"y", Compress::Never).unwrap();
    let n = bad.len();
    bad[n - 1] ^= 1;
    d1.put(ObjKind::Obj, &v.ns, &g, &bad).unwrap();
    // a valid chunk container sealed under ANOTHER vault key
    let mut k2 = [0u8; 32];
    p.random(&mut k2).unwrap();
    let f = rid(&p);
    let foreign = seal(&p, &k2, &ObjectCtx { suite: v.suite, vault_id: v.vault_id, obj_id: f }, Kind::Chunk, b"x", Compress::Never).unwrap();
    d1.put(ObjKind::Obj, &v.ns, &f, &foreign).unwrap();
    // a commit signed by a device that is not (any more) in the vault
    let u = build_commit(&p, &v, &signer(&evil), T0, vec![], &Tree::default(), vec![put("evil.md", Class::Markdown, b"x\n")]).unwrap();
    push_commit(&d1, &v, &u).unwrap();
    // a correctly signed commit whose parent is nowhere -> pending, not a tip
    let orphan_parent = Parent { commit_id: rid(&p), tree_hash: [7; 32] };
    let o = build_commit(&p, &v, &signer(&da), T0 + 1, vec![orphan_parent], &Tree::default(), vec![put("o.md", Class::Markdown, b"o\n")]).unwrap();
    push_commit(&d1, &v, &o).unwrap();

    // a dead replica in the list is reported, the others still sync
    let dead = HttpTransport::new("http://127.0.0.1:1", Roots::System).unwrap();
    dead.set_tokens(Tokens { read: Some("r".into()), write: Some("w".into()) });
    let r = a.sync(&[&d1, &dead, &srv.t], &signer(&da), &ks, T0 + 2, &mut hook).unwrap();
    let rej: BTreeSet<Id16> = r.rejected.iter().map(|(i, id, _)| {
        assert_eq!(*i, 0);
        *id
    }).collect();
    assert_eq!(rej, [g, f, u.id].into_iter().collect(), "{:?}", r.rejected);
    assert_eq!(r.replica_errors.len(), 1);
    assert_eq!(r.replica_errors[0].0, 1);
    assert_eq!(r.pending, 1);
    assert_eq!(r.pulled, 1, "only the orphan (valid signature) is stored");
    assert_eq!(a.head(), Some(c1));
    assert_eq!(a.tips(), vec![c1]);
    let on_srv = ids(&srv.t, &v.ns);
    assert!(on_srv.contains(&c1) && on_srv.contains(&o.id));
    for bad in [g, f, u.id] {
        assert!(!on_srv.contains(&bad), "rejected object must not reach other replicas");
        assert!(!a.objects().contains(&bad));
    }
    assert!(text(&mut a, &ks, "evil.md").is_none());
}

#[test]
fn commit_with_a_lying_tree_hash_never_becomes_head() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let mut k = [0u8; 32];
    p.random(&mut k).unwrap();
    let v = VaultCtx { suite: Suite::Ed25519, vault_id: rid(&p), ns: rid(&p), k_obj: k };
    let d1 = FsTransport::open(tmpdir("lie")).unwrap();
    let (da, db) = (device(&p), device(&p));
    let ks: Keys = [&da, &db].iter().map(|d| (d.id, (SigAlg::Ed25519, d.pk.clone()))).collect();
    let mut hook = |_: &Loser, _: &[u8]| {};
    // a vault member signs a ROOT commit whose tree_hash was computed over a
    // base tree it does not have (signature and content are valid)
    let fake = build_commit(&p, &v, &signer(&db), T0, vec![], &Tree::default(), vec![put("hidden.md", Class::Markdown, b"h\n")]).unwrap();
    let lie = build_commit(&p, &v, &signer(&db), T0 + 1, vec![], &fake.tree, vec![put("x.md", Class::Markdown, b"x\n")]).unwrap();
    push_commit(&d1, &v, &lie).unwrap();

    // fresh device: nothing to fast-forward to, no error
    let mut a = Engine::open(&p, v.clone(), tmpdir("elie"), &ks).unwrap();
    let r = a.sync(&[&d1], &signer(&da), &ks, T0 + 2, &mut hook).expect("pass completes");
    assert_eq!(r.invalid.len(), 1, "{:?}", r.invalid);
    assert_eq!(r.invalid[0].0, lie.id);
    assert_eq!(a.head(), None);
    assert!(r.folded.is_empty());

    // device with a head: the lying tip is skipped, not merged, pass completes
    let c1 = a.commit_local(&signer(&da), &ks, T0 + 3, vec![put("n.md", Class::Markdown, b"hi\n")]).unwrap();
    let r = a.sync(&[&d1], &signer(&da), &ks, T0 + 4, &mut hook).expect("pass completes");
    assert_eq!(r.invalid.iter().map(|x| x.0).collect::<Vec<_>>(), vec![lie.id]);
    assert_eq!(a.head(), Some(c1));
    assert!(text(&mut a, &ks, "x.md").is_none());
}

#[test]
fn parent_cycle_is_pending_not_a_hang() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let mut k = [0u8; 32];
    p.random(&mut k).unwrap();
    let v = VaultCtx { suite: Suite::Ed25519, vault_id: rid(&p), ns: rid(&p), k_obj: k };
    let d1 = FsTransport::open(tmpdir("cyc")).unwrap();
    let (da, db) = (device(&p), device(&p));
    let ks: Keys = [&da, &db].iter().map(|d| (d.id, (SigAlg::Ed25519, d.pk.clone()))).collect();
    let mut hook = |_: &Loser, _: &[u8]| {};
    // B's parent is A; A is re-signed (same id) with B as ITS parent: A <-> B
    let a0 = build_commit(&p, &v, &signer(&db), T0, vec![], &Tree::default(), vec![put("a.md", Class::Markdown, b"a\n")]).unwrap();
    let b = build_commit(&p, &v, &signer(&db), T0 + 1, vec![a0.parent()], &a0.tree, vec![put("b.md", Class::Markdown, b"b\n")]).unwrap();
    let mut t = a0.batch.clone();
    t.parents = vec![b.parent()];
    t.sig.clear();
    let mut m = BATCH_SIG_DOMAIN.to_vec();
    m.extend(t.encode_signed().unwrap());
    t.sig = p.sign(&db.key, &p.sha256(&m)).unwrap();
    let a1 = seal(&p, &k, &ObjectCtx { suite: v.suite, vault_id: v.vault_id, obj_id: a0.id }, Kind::Batch, &t.encode().unwrap(), Compress::IfSmaller).unwrap();
    d1.put(ObjKind::Obj, &v.ns, &a0.id, &a1).unwrap();
    push_commit(&d1, &v, &b).unwrap();

    let mut e = Engine::open(&p, v.clone(), tmpdir("ecyc"), &ks).unwrap();
    let c1 = e.commit_local(&signer(&da), &ks, T0 + 2, vec![put("n.md", Class::Markdown, b"hi\n")]).unwrap();
    let r = e.sync(&[&d1], &signer(&da), &ks, T0 + 3, &mut hook).expect("pass completes");
    assert!(r.rejected.is_empty(), "{:?}", r.rejected);
    assert_eq!(r.pending, 2, "both cycle members wait forever, neither is ready");
    assert_eq!(e.head(), Some(c1));
    assert_eq!(e.tips(), vec![c1]);
}

/// CodeRabbit #238 engine:613: a bad copy served by one replica does not
/// stop the pass from taking the good copy of the same id from another.
#[test]
fn bad_copy_on_one_replica_does_not_block_the_good_copy() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let mut k = [0u8; 32];
    p.random(&mut k).unwrap();
    let v = VaultCtx { suite: Suite::Ed25519, vault_id: rid(&p), ns: rid(&p), k_obj: k };
    let (d1, d2) = (FsTransport::open(tmpdir("bc1")).unwrap(), FsTransport::open(tmpdir("bc2")).unwrap());
    let da = device(&p);
    let ks: Keys = [(da.id, (SigAlg::Ed25519, da.pk.clone()))].into_iter().collect();
    let mut hook = |_: &Loser, _: &[u8]| {};
    let c = build_commit(&p, &v, &signer(&da), T0, vec![], &Tree::default(), vec![put("n.md", Class::Markdown, b"good\n")]).unwrap();
    push_commit(&d2, &v, &c).unwrap();
    let mut bad = d2.get(ObjKind::Obj, &v.ns, &c.id, None).unwrap().bytes;
    let n = bad.len();
    bad[n - 1] ^= 1;
    d1.put(ObjKind::Obj, &v.ns, &c.id, &bad).unwrap();

    let mut b = Engine::open(&p, v.clone(), tmpdir("ebc"), &ks).unwrap();
    let r = b.sync(&[&d1, &d2], &signer(&da), &ks, T0 + 1, &mut hook).unwrap();
    assert_eq!(r.rejected.iter().map(|(i, id, _)| (*i, *id)).collect::<Vec<_>>(), vec![(0, c.id)]);
    assert_eq!(b.head(), Some(c.id), "the good copy from replica 1 is taken");
    assert_eq!(text(&mut b, &ks, "n.md").as_deref(), Some(&b"good\n"[..]));
}

/// Replica whose object reads fail (listing works): engine:644.
struct BrokenGet<'a> {
    inner: &'a FsTransport,
}

impl Transport for BrokenGet<'_> {
    fn health(&self) -> sync_core::transport::Result<sync_proto::api::Health> {
        self.inner.health()
    }
    fn enroll(&self, t: &str, k: &[u8]) -> sync_core::transport::Result<sync_proto::api::EnrollResponse> {
        self.inner.enroll(t, k)
    }
    fn put(&self, kind: ObjKind, ns: &Id16, id: &Id16, b: &[u8]) -> sync_core::transport::Result<sync_core::transport::PutOutcome> {
        self.inner.put(kind, ns, id, b)
    }
    fn get(&self, _: ObjKind, _: &Id16, _: &Id16, _: Option<sync_core::transport::Range>) -> sync_core::transport::Result<sync_core::transport::Fetched> {
        Err(sync_core::transport::TransportError::Io("read failed".into()))
    }
    fn get_deleted(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> sync_core::transport::Result<Vec<u8>> {
        self.inner.get_deleted(kind, ns, id)
    }
    fn missing(&self, ns: &Id16, ids: &[Id16]) -> sync_core::transport::Result<Vec<Id16>> {
        self.inner.missing(ns, ids)
    }
    fn list(&self, ns: &Id16, q: ListQuery) -> sync_core::transport::Result<sync_proto::api::ListResponse> {
        self.inner.list(ns, q)
    }
    fn delete(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> sync_core::transport::Result<()> {
        self.inner.delete(kind, ns, id)
    }
    fn destroy_request(&self, ns: &Id16) -> sync_core::transport::Result<sync_proto::api::DestroyChallenge> {
        self.inner.destroy_request(ns)
    }
    fn destroy_confirm(&self, ns: &Id16, c: &sync_proto::api::DestroyConfirm) -> sync_core::transport::Result<sync_proto::api::DestroyReceipt> {
        self.inner.destroy_confirm(ns, c)
    }
}

/// CodeRabbit #238 engine:644: a replica that fails during pull gets no
/// push in the same pass (its error is reported once).
#[test]
fn replica_failing_in_pull_is_not_pushed_to() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let mut k = [0u8; 32];
    p.random(&mut k).unwrap();
    let v = VaultCtx { suite: Suite::Ed25519, vault_id: rid(&p), ns: rid(&p), k_obj: k };
    let d1 = FsTransport::open(tmpdir("bg")).unwrap();
    let (da, db) = (device(&p), device(&p));
    let ks: Keys = [&da, &db].iter().map(|d| (d.id, (SigAlg::Ed25519, d.pk.clone()))).collect();
    let mut hook = |_: &Loser, _: &[u8]| {};
    let other = build_commit(&p, &v, &signer(&db), T0, vec![], &Tree::default(), vec![put("o.md", Class::Markdown, b"o\n")]).unwrap();
    push_commit(&d1, &v, &other).unwrap();
    let before = ids(&d1, &v.ns);
    let mut a = Engine::open(&p, v.clone(), tmpdir("ebg"), &ks).unwrap();
    a.commit_local(&signer(&da), &ks, T0 + 1, vec![put("n.md", Class::Markdown, b"n\n")]).unwrap();
    let broken = BrokenGet { inner: &d1 };
    let r = a.sync(&[&broken], &signer(&da), &ks, T0 + 2, &mut hook).unwrap();
    assert_eq!(r.replica_errors.len(), 1, "{:?}", r.replica_errors);
    assert_eq!(r.pushed, 0, "no push to a replica that failed in pull");
    assert_eq!(ids(&d1, &v.ns), before);
}
