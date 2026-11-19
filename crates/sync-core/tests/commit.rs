// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! M4a: commit DAG objects (spec §4.1–4.3) on the AWS-LC FIPS module —
//! signed batch = manifest + inline blobs + signature in one object,
//! attachments > 1 MiB as uniform 4 MiB chunk objects, tree hash, and the
//! round trip through BOTH transports (real sync-server on loopback + dir
//! replica). Every verification failure has its own test.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use sync_core::commit::{
    assemble, build_commit, fetch_commit, fetch_entry, open_chunk, open_commit, push_commit,
    CommitError, ContentRef, Edit, Signer, Tree, VaultCtx,
};
use sync_core::object::{seal, Compress, ObjectCtx};
use sync_core::transport::fs::FsTransport;
use sync_core::transport::http::{HttpTransport, Roots};
use sync_core::transport::{missing_all, Tokens, Transport};
use sync_crypto::fips::{FipsProvider, FipsSigningKey};
use sync_crypto::{CryptoProvider, SigAlg};
use sync_proto::api::ObjKind;
use sync_proto::batch::{Batch, Class, Op, Put, Where, BATCH_SIG_DOMAIN, CHUNK_BODY, MAX_INLINE};
use sync_proto::container::{container_len, Kind, Suite, CHUNK_PLAINTEXT_LEN};
use sync_proto::Id16;
use sync_server::{serve, App, Config};

static N: AtomicU32 = AtomicU32::new(0);

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "osy-cm-{tag}-{}-{}",
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

fn vault(p: &FipsProvider, ns: Id16) -> VaultCtx {
    let mut k = [0u8; 32];
    p.random(&mut k).unwrap();
    VaultCtx {
        suite: Suite::Ed25519,
        vault_id: rid(p),
        ns,
        k_obj: k,
        prev_k_obj: vec![],
    }
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

type Keys = BTreeMap<Id16, (SigAlg, Vec<u8>)>;

fn keys(devs: &[&Dev]) -> Keys {
    devs.iter()
        .map(|d| (d.id, (SigAlg::Ed25519, d.pk.clone())))
        .collect()
}

fn signer(d: &Dev) -> Signer<'_, FipsSigningKey> {
    Signer {
        device_id: d.id,
        key: &d.key,
    }
}

fn put(path: &str, class: Class, content: Vec<u8>) -> Edit {
    Edit::Put {
        path: path.into(),
        class,
        mtime_ms: 1_763_000_000_000,
        content,
    }
}

/// Deterministic pseudo-random bytes (not compressible, so chunk sizes are real).
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

#[test]
fn small_commit_roundtrip_and_tree_hash() {
    let p = fips();
    let v = vault(&p, rid(&p));
    let d = device(&p);
    let b = build_commit(
        &p,
        &v,
        &signer(&d),
        42,
        vec![],
        &Tree::default(),
        vec![
            put("notes/b.md", Class::Markdown, b"# B\n".to_vec()),
            put(".obsidian/app.json", Class::SettingsJson, b"{}".to_vec()),
            put("a.md", Class::Markdown, b"# A\n".to_vec()),
            put("empty.md", Class::Markdown, Vec::new()),
        ],
    )
    .unwrap();
    assert!(b.chunks.is_empty());
    // canonical order: sorted by path bytes, blobs in change order
    let paths: Vec<_> = b.batch.changes.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, [".obsidian/app.json", "a.md", "empty.md", "notes/b.md"]);
    assert_eq!(b.batch.blobs[1], b"# A\n");
    assert_eq!(b.batch.tree_hash, b.tree.hash(&p).unwrap());

    let got = open_commit(&p, &v, &b.id, &b.container, &keys(&[&d])).unwrap();
    assert_eq!(got, b.batch);
    // the receiver rebuilds the same tree from the manifest alone
    let mut t = Tree::default();
    t.apply(&got).unwrap();
    assert_eq!(t, b.tree);
    assert_eq!(t.hash(&p).unwrap(), got.tree_hash);

    // tree hash = SHA-256("osy tree v1" || list<entry>), insertion order irrelevant
    let mut want = b"osy tree v1".to_vec();
    want.extend(4u32.to_be_bytes());
    for (path, e) in &t.0 {
        want.extend((path.len() as u16).to_be_bytes());
        want.extend(path.as_bytes());
        want.push(e.class as u8);
        want.extend(e.sha256);
    }
    assert_eq!(p.sha256(&want), got.tree_hash);
}

#[test]
fn dag_chain_delete_rename_and_no_op_detection() {
    let p = fips();
    let v = vault(&p, rid(&p));
    let d = device(&p);
    let ks = keys(&[&d]);
    let c1 = build_commit(
        &p,
        &v,
        &signer(&d),
        1,
        vec![],
        &Tree::default(),
        vec![
            put("a.md", Class::Markdown, b"a".to_vec()),
            put("b.md", Class::Markdown, b"b".to_vec()),
        ],
    )
    .unwrap();
    let c2 = build_commit(
        &p,
        &v,
        &signer(&d),
        2,
        vec![c1.parent()],
        &c1.tree,
        vec![
            Edit::Delete { path: "a.md".into() },
            Edit::Rename {
                from: "b.md".into(),
                to: "dir/b.md".into(),
            },
        ],
    )
    .unwrap();
    let got = open_commit(&p, &v, &c2.id, &c2.container, &ks).unwrap();
    assert_eq!(got.parents, vec![c1.parent()]);
    let mut t = c1.tree.clone();
    t.apply(&got).unwrap();
    assert_eq!(t.0.keys().collect::<Vec<_>>(), ["dir/b.md"]);
    // the renamed entry still points at c1's blob
    assert_eq!(
        t.0["dir/b.md"].content,
        ContentRef::Inline {
            commit: c1.id,
            blob: 1
        }
    );
    assert_eq!(t.hash(&p).unwrap(), got.tree_hash);

    // re-putting identical content gives an identical tree hash (merge no-op rule §4.3)
    let c3 = build_commit(
        &p,
        &v,
        &signer(&d),
        3,
        vec![c2.parent()],
        &c2.tree,
        vec![put("dir/b.md", Class::Markdown, b"b".to_vec())],
    )
    .unwrap();
    assert_eq!(c3.batch.tree_hash, c2.batch.tree_hash);
    assert_ne!(c3.batch.tree_hash, c1.batch.tree_hash);

    // edits that do not apply to the base are refused at build time
    let e = |edits| build_commit(&p, &v, &signer(&d), 4, vec![c2.parent()], &c2.tree, edits);
    assert!(matches!(
        e(vec![Edit::Delete { path: "a.md".into() }]),
        Err(CommitError::Tree(_))
    ));
    assert!(matches!(
        e(vec![Edit::Rename {
            from: "nope.md".into(),
            to: "x.md".into()
        }]),
        Err(CommitError::Tree(_))
    ));
    assert!(matches!(
        e(vec![
            put("x.md", Class::Markdown, vec![]),
            Edit::Delete { path: "x.md".into() }
        ]),
        Err(CommitError::Tree(_))
    ));
    assert!(e(vec![put("../evil.md", Class::Markdown, vec![])]).is_err());
}

#[test]
fn chunking_boundaries() {
    let p = fips();
    let v = vault(&p, rid(&p));
    let d = device(&p);
    let ks = keys(&[&d]);
    for (size, want_chunks) in [
        (MAX_INLINE, 0usize),
        (MAX_INLINE + 1, 1),
        (CHUNK_BODY, 1),
        (CHUNK_BODY + 1, 2),
        (2 * CHUNK_BODY + 123, 3),
    ] {
        let data = noise(size, size as u64);
        let b = build_commit(
            &p,
            &v,
            &signer(&d),
            0,
            vec![],
            &Tree::default(),
            vec![put("att/file.bin", Class::Other, data.clone())],
        )
        .unwrap();
        assert_eq!(b.chunks.len(), want_chunks, "size {size}");
        let Op::Put(Put { loc, size: s, .. }) = &b.batch.changes[0].op else {
            panic!()
        };
        assert_eq!(*s, size as u64);
        match loc {
            Where::Inline(_) => {
                assert_eq!(want_chunks, 0);
                assert_eq!(b.batch.blobs[0], data);
            }
            Where::Chunks(ids) => {
                assert_eq!(ids, &b.chunks.iter().map(|c| c.0).collect::<Vec<_>>());
                assert!(b.batch.blobs.is_empty());
            }
        }
        // every chunk container has the same uniform size (spec §3.5)
        for (id, c) in &b.chunks {
            assert_eq!(c.len(), container_len(CHUNK_PLAINTEXT_LEN), "uniform chunk size");
            assert!(open_chunk(&p, &v, id, c).unwrap().len() <= CHUNK_BODY);
        }
        let got = open_commit(&p, &v, &b.id, &b.container, &ks).unwrap();
        if want_chunks > 0 {
            let slices: Vec<_> = b
                .chunks
                .iter()
                .map(|(id, c)| open_chunk(&p, &v, id, c).unwrap())
                .collect();
            let Op::Put(pu) = &got.changes[0].op else { panic!() };
            assert_eq!(assemble(&p, pu.size, &pu.sha256, &slices).unwrap(), data);
            if slices.len() > 1 {
                let mut swapped = slices.clone();
                swapped.swap(0, 1);
                assert_eq!(
                    assemble(&p, pu.size, &pu.sha256, &swapped),
                    Err(CommitError::ContentMismatch),
                    "chunk order is verified"
                );
            }
            let mut flipped = slices.clone();
            flipped[0][7] ^= 1;
            assert_eq!(
                assemble(&p, pu.size, &pu.sha256, &flipped),
                Err(CommitError::ContentMismatch)
            );
        }
    }
}

#[test]
fn verification_failures() {
    let p = fips();
    let v = vault(&p, rid(&p));
    let d = device(&p);
    let other = device(&p);
    let ks = keys(&[&d]);
    let b = build_commit(
        &p,
        &v,
        &signer(&d),
        7,
        vec![],
        &Tree::default(),
        vec![
            put("a.md", Class::Markdown, b"hello".to_vec()),
            put("big.bin", Class::Other, noise(MAX_INLINE + 10, 3)),
        ],
    )
    .unwrap();
    open_commit(&p, &v, &b.id, &b.container, &ks).unwrap();

    // unknown / revoked device
    assert_eq!(
        open_commit(&p, &v, &b.id, &b.container, &keys(&[&other])),
        Err(CommitError::UnknownDevice)
    );
    // device id known but with a different key (impersonation)
    let mut swapped: Keys = BTreeMap::new();
    swapped.insert(d.id, (SigAlg::Ed25519, other.pk.clone()));
    assert_eq!(
        open_commit(&p, &v, &b.id, &b.container, &swapped),
        Err(CommitError::BadSignature)
    );
    // key registered for the wrong algorithm
    let mut wrong_alg: Keys = BTreeMap::new();
    wrong_alg.insert(d.id, (SigAlg::EcdsaP256Sha256, d.pk.clone()));
    assert_eq!(
        open_commit(&p, &v, &b.id, &b.container, &wrong_alg),
        Err(CommitError::WrongAlg)
    );
    // wrong vault key / wrong id
    let mut v2 = v.clone();
    v2.k_obj[0] ^= 1;
    assert!(matches!(
        open_commit(&p, &v2, &b.id, &b.container, &ks),
        Err(CommitError::Object(_))
    ));
    assert!(matches!(
        open_commit(&p, &v, &rid(&p), &b.container, &ks),
        Err(CommitError::Object(_))
    ));
    // a chunk is not a commit, a commit is not a chunk
    let (cid, cc) = &b.chunks[0];
    assert_eq!(open_commit(&p, &v, cid, cc, &ks), Err(CommitError::WrongKind));
    assert_eq!(open_chunk(&p, &v, &b.id, &b.container), Err(CommitError::WrongKind));

    let reseal = |batch: &Batch| {
        seal(
            &p,
            &v.k_obj,
            &ObjectCtx {
                suite: v.suite,
                vault_id: v.vault_id,
                obj_id: batch.commit_id,
            },
            Kind::Batch,
            &batch.encode().unwrap(),
            Compress::IfSmaller,
        )
        .unwrap()
    };
    // a key holder edits a signed field but cannot re-sign: ts, parents, tree, content
    let mut t = b.batch.clone();
    t.ts_ms += 1;
    assert_eq!(open_commit(&p, &v, &b.id, &reseal(&t), &ks), Err(CommitError::BadSignature));
    let mut t = b.batch.clone();
    t.tree_hash[0] ^= 1;
    assert_eq!(open_commit(&p, &v, &b.id, &reseal(&t), &ks), Err(CommitError::BadSignature));
    let mut t = b.batch.clone();
    t.device_id = other.id;
    assert_eq!(
        open_commit(&p, &v, &b.id, &reseal(&t), &keys(&[&d, &other])),
        Err(CommitError::BadSignature)
    );
    // inline blob swapped for same-size bytes: hash differs; with a valid
    // signature over the bad hash the content check still catches it
    let mut t = b.batch.clone();
    t.blobs[0] = b"HELLO".to_vec();
    assert_eq!(open_commit(&p, &v, &b.id, &reseal(&t), &ks), Err(CommitError::BadSignature));
    // commit_id inside != object id (sealed under its own id)
    let mut t = b.batch.clone();
    t.commit_id = rid(&p);
    let c = seal(
        &p,
        &v.k_obj,
        &ObjectCtx {
            suite: v.suite,
            vault_id: v.vault_id,
            obj_id: b.id,
        },
        Kind::Batch,
        &t.encode().unwrap(),
        Compress::Never,
    )
    .unwrap();
    assert_eq!(open_commit(&p, &v, &b.id, &c, &ks), Err(CommitError::WrongId));
    // sig_alg byte not of the suite
    let mut t = b.batch.clone();
    t.sig_alg = SigAlg::EcdsaP256Sha256 as u8;
    assert_eq!(open_commit(&p, &v, &b.id, &reseal(&t), &ks), Err(CommitError::WrongAlg));
}

#[test]
fn signed_bad_inline_hash_is_rejected() {
    // A buggy/malicious device signs a manifest whose inline hash does not
    // match its blob: signature verifies, content check refuses.
    let p = fips();
    let v = vault(&p, rid(&p));
    let d = device(&p);
    let b = build_commit(
        &p,
        &v,
        &signer(&d),
        7,
        vec![],
        &Tree::default(),
        vec![put("a.md", Class::Markdown, b"hello".to_vec())],
    )
    .unwrap();
    let mut t = b.batch.clone();
    t.blobs[0] = b"HELLO".to_vec();
    let mut m = BATCH_SIG_DOMAIN.to_vec();
    m.extend(t.encode_signed().unwrap());
    t.sig = p.sign(&d.key, &p.sha256(&m)).unwrap();
    let c = seal(
        &p,
        &v.k_obj,
        &ObjectCtx {
            suite: v.suite,
            vault_id: v.vault_id,
            obj_id: b.id,
        },
        Kind::Batch,
        &t.encode().unwrap(),
        Compress::IfSmaller,
    )
    .unwrap();
    assert_eq!(
        open_commit(&p, &v, &b.id, &c, &keys(&[&d])),
        Err(CommitError::ContentMismatch)
    );
}

fn roundtrip(t: &dyn Transport, ns: Id16) {
    let p = fips();
    let v = vault(&p, ns);
    let d = device(&p);
    let ks = keys(&[&d]);
    let big = noise(2 * CHUNK_BODY + 4321, 99);
    let c1 = build_commit(
        &p,
        &v,
        &signer(&d),
        1,
        vec![],
        &Tree::default(),
        vec![
            put("note.md", Class::Markdown, b"# hi\n".to_vec()),
            put("att/photo.bin", Class::Other, big.clone()),
        ],
    )
    .unwrap();
    assert_eq!(c1.chunks.len(), 3);
    push_commit(t, &v, &c1).unwrap();
    push_commit(t, &v, &c1).expect("retried push is idempotent (write-once, identical)");
    let mut all: Vec<Id16> = c1.chunks.iter().map(|c| c.0).collect();
    all.push(c1.id);
    assert!(missing_all(t, &v.ns, &all).unwrap().is_empty());

    let c2 = build_commit(
        &p,
        &v,
        &signer(&d),
        2,
        vec![c1.parent()],
        &c1.tree,
        vec![put("note.md", Class::Markdown, b"# hi again\n".to_vec())],
    )
    .unwrap();
    push_commit(t, &v, &c2).unwrap();

    // a reader with only the ids + keys rebuilds the tree and the bytes
    let r1 = fetch_commit(t, &p, &v, &c1.id, &ks).unwrap();
    let r2 = fetch_commit(t, &p, &v, &c2.id, &ks).unwrap();
    let mut tree = Tree::default();
    tree.apply(&r1).unwrap();
    tree.apply(&r2).unwrap();
    assert_eq!(tree.hash(&p).unwrap(), r2.tree_hash);
    assert_eq!(
        fetch_entry(t, &p, &v, &tree.0["att/photo.bin"], None, &ks).unwrap(),
        big
    );
    assert_eq!(
        fetch_entry(t, &p, &v, &tree.0["note.md"], Some(&r2), &ks).unwrap(),
        b"# hi again\n"
    );
    // inline content of an older commit is fetched + verified on demand
    let mut t1 = Tree::default();
    t1.apply(&r1).unwrap();
    assert_eq!(
        fetch_entry(t, &p, &v, &t1.0["note.md"], Some(&r2), &ks).unwrap(),
        b"# hi\n"
    );
    // a device that is not in the key set cannot be read as authentic
    assert_eq!(
        fetch_commit(t, &p, &v, &c1.id, &Keys::new()),
        Err(CommitError::UnknownDevice)
    );
    // a missing chunk surfaces as a transport error, not as wrong bytes
    let ContentRef::Chunks(ids) = &tree.0["att/photo.bin"].content else {
        panic!()
    };
    t.delete(ObjKind::Obj, &v.ns, &ids[1]).unwrap();
    assert!(matches!(
        fetch_entry(t, &p, &v, &tree.0["att/photo.bin"], None, &ks),
        Err(CommitError::Transport(_))
    ));
}

#[test]
fn roundtrip_fs_replica() {
    let t = FsTransport::open(tmpdir("fs")).unwrap();
    roundtrip(&t, Id16([0x42; 16]));
}

#[test]
fn roundtrip_http_server() {
    let mut cfg = Config::new(tmpdir("srv"));
    cfg.anon_rate_per_min = 10_000;
    let app = Arc::new(App::open(cfg).expect("app"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let l = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = l.local_addr().unwrap().port();
    let a = app.clone();
    rt.spawn(async move { serve(a, l).await });

    let t = HttpTransport::new(&format!("http://127.0.0.1:{port}"), Roots::System).unwrap();
    let p = fips();
    let (_k, pk) = p.generate_signing_key(SigAlg::Ed25519).unwrap();
    let tok = app.create_enrollment(None, 600).unwrap();
    let e = t.enroll(&tok, &pk).expect("enroll");
    t.set_tokens(Tokens::from(&e));
    roundtrip(&t, e.ns);
}
