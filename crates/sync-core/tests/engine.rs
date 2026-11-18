// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! M4c: the sync engine on the AWS-LC FIPS module — replicas as equal
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
use sync_proto::batch::{Class, Parent};
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
    let (key, pk) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
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
    let (_k, pk) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
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
}

#[test]
fn three_replicas_converge_by_set_union() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let srv = server(&p);
    let mut k = [0u8; 32];
    p.random(&mut k).unwrap();
    let v = VaultCtx { suite: Suite::P256, vault_id: rid(&p), ns: srv.ns, k_obj: k };
    let d1 = FsTransport::open(tmpdir("d1")).unwrap();
    let d2 = FsTransport::open(tmpdir("d2")).unwrap();
    let (da, db) = (device(&p), device(&p));
    let ks: Keys = [&da, &db].iter().map(|d| (d.id, (SigAlg::EcdsaP256Sha256, d.pk.clone()))).collect();
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
    drop(hook);
    assert!(!lost.borrow().is_empty());
}

#[test]
fn bad_objects_are_rejected_and_not_propagated() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let srv = server(&p);
    let mut k = [0u8; 32];
    p.random(&mut k).unwrap();
    let v = VaultCtx { suite: Suite::P256, vault_id: rid(&p), ns: srv.ns, k_obj: k };
    let d1 = FsTransport::open(tmpdir("bad")).unwrap();
    let (da, evil) = (device(&p), device(&p));
    let ks: Keys = [(da.id, (SigAlg::EcdsaP256Sha256, da.pk.clone()))].into_iter().collect();
    let mut a = Engine::open(&p, v.clone(), tmpdir("ebad"), &ks).unwrap();
    let mut hook = |_: &Loser, _: &[u8]| {};
    let c1 = a.commit_local(&signer(&da), &ks, T0, vec![put("n.md", Class::Markdown, b"hi\n")]).unwrap();

    // garbage bytes
    let g = rid(&p);
    d1.put(ObjKind::Obj, &v.ns, &g, b"not a container at all").unwrap();
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
