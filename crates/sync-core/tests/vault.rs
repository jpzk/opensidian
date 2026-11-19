// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! M5 onboarding on the OpenSSL 3 FIPS provider against a real
//! opensidian-sync-server on loopback: create a vault, a fresh device from
//! the words + URL only, symmetric pairing, the no-revocation rules (one VK
//! epoch, no slot ever leaves the table, unknown devices have no key),
//! destroy with/without proof,
//! keystore entries.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sync_core::commit::{DeviceKeys, Edit};
use sync_core::engine::{Engine, Report};
use sync_core::keystore::{Entry, FileKeystore, FixedPresence, Keystore, KeystoreError};
use sync_core::merge::Loser;
use sync_core::transport::http::{HttpTransport, Roots};
use sync_core::transport::{Transport, TransportError};
use sync_core::vault::{
    bootstrap, cfg_digest, create_vault, destroy, pair_accept, pair_complete, pair_join, pair_offer,
    root_of, verify_gen, Member, PairCode, VaultError,
};
use sync_crypto::fips::FipsProvider;
use sync_crypto::words::Phrase;
use sync_crypto::CryptoProvider;
use sync_proto::api::{DestroyConfirm, ErrorCode, ObjKind};
use sync_proto::batch::Class;
use sync_proto::vault::CfgSigner;
use sync_proto::Id16;
use sync_server::{serve, App, Config};

static N: AtomicU32 = AtomicU32::new(0);

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("osy-va-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    d
}

struct Server {
    _rt: tokio::runtime::Runtime,
    app: Arc<App>,
    url: String,
}

fn server() -> Server {
    let mut cfg = Config::new(tmpdir("srv"));
    cfg.anon_rate_per_min = 10_000;
    let app = Arc::new(App::open(cfg).expect("app"));
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let l = rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
    let port = l.local_addr().unwrap().port();
    let a = app.clone();
    rt.spawn(async move { serve(a, l).await });
    Server { _rt: rt, app, url: format!("http://127.0.0.1:{port}") }
}

fn client(s: &Server) -> HttpTransport {
    HttpTransport::new(&s.url, Roots::System).unwrap()
}

fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

const T0: i64 = 1_763_000_000_000;

fn put(path: &str, content: &[u8]) -> Edit {
    Edit::Put { path: path.into(), class: Class::Markdown, mtime_ms: T0, content: content.to_vec() }
}

fn engine<'p>(p: &'p FipsProvider, m: &Member, tag: &str) -> Engine<'p, FipsProvider> {
    Engine::open(p, m.ctx(p).unwrap(), tmpdir(tag), &m.roster()).unwrap()
}

fn sync(p: &FipsProvider, e: &mut Engine<'_, FipsProvider>, m: &Member, t: &HttpTransport, ts: i64) -> Report {
    let _ = p;
    let mut hook = |_: &Loser, _: &[u8]| {};
    e.sync(&[t as &dyn Transport], &m.device.signer(), &m.roster(), ts, &mut hook).unwrap()
}

fn read(e: &mut Engine<'_, FipsProvider>, keys: &dyn DeviceKeys, path: &str) -> Option<Vec<u8>> {
    let t = e.head_tree().unwrap();
    t.0.get(path).map(|x| e.read(x, keys).unwrap())
}

fn clean(r: &Report) {
    assert!(r.rejected.is_empty(), "rejected: {:?}", r.rejected);
    assert!(r.replica_errors.is_empty(), "replica errors: {:?}", r.replica_errors);
    assert!(r.invalid.is_empty(), "invalid: {:?}", r.invalid);
}

#[test]
fn create_join_pair_destroy_no_revocation() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let s = server();
    let words = Phrase::generate(&p).unwrap();

    // A: new vault -> phrase, envelope under loc, cfg gen 0 (signed by K_rsig)
    let ta = client(&s);
    let tok = s.app.create_enrollment(None, 600).unwrap();
    let mut a = create_vault(&p, &ta, &s.url, &tok, &words, "laptop").expect("create");
    assert_eq!(a.chain.gens.len(), 1);
    assert_eq!(a.chain.head().signer, CfgSigner::Recovery);
    assert_eq!(a.chain.active_slot(&a.device.id), Some(0));
    assert!(create_vault(&p, &client(&s), &s.url, &tok, &words, "x").is_err(), "enrollment token is one-time");
    let mut ea = engine(&p, &a, "ea");
    ea.commit_local(&a.device.signer(), &a.roster(), T0, vec![put("a.md", b"from A\n")]).unwrap();
    clean(&sync(&p, &mut ea, &a, &ta, T0));

    // B: fresh device from the 12 words (as typed) + URL only
    let typed = words.to_text(&p);
    let tb = client(&s);
    let wrong = Phrase::generate(&p).unwrap();
    assert_eq!(bootstrap(&p, &tb, &s.url, &wrong, "nope").unwrap_err(), VaultError::NoVault);
    let bw = Phrase::parse(&p, &typed.to_uppercase()).unwrap();
    let b = bootstrap(&p, &tb, &s.url, &bw, "desktop").expect("bootstrap from words");
    assert_eq!(b.env.vault_id, a.env.vault_id);
    assert_eq!(b.chain.gens.len(), 2);
    assert_eq!(b.chain.head().signer, CfgSigner::Recovery);
    let mut eb = engine(&p, &b, "eb");
    clean(&sync(&p, &mut eb, &b, &tb, T0 + 1));
    assert_eq!(read(&mut eb, &b.roster(), "a.md").as_deref(), Some(&b"from A\n"[..]));
    eb.commit_local(&b.device.signer(), &b.roster(), T0 + 2, vec![put("b.md", b"from B\n")]).unwrap();
    clean(&sync(&p, &mut eb, &b, &tb, T0 + 2));

    // C: symmetric pairing (offer -> accept + reply -> complete -> join).
    // A's chain is stale (B appended gen 1): complete re-fetches and retries.
    let code = pair_offer(&p, &ta, &a, Some(&words), now_s()).unwrap();
    let qr = code.to_text().unwrap();
    assert!(qr.starts_with("osy-pair1:"));
    assert!(!format!("{code:?}").contains(&qr[10..20]), "debug redacts K_p");
    let parsed = PairCode::parse(&qr).unwrap();
    assert_eq!(parsed, code);
    let tc = client(&s);
    let j = pair_accept(&p, &tc, &parsed, "phone", now_s()).expect("accept");
    assert!(pair_accept(&p, &client(&s), &parsed, "again", now_s()).is_err(), "offer is single use");
    let (cid, cname) = pair_complete(&p, &ta, &mut a, &code).expect("complete");
    assert_eq!((cid, cname.as_str()), (j.device.id, "phone"));
    assert_eq!(a.chain.gens.len(), 3);
    assert_eq!(a.chain.head().signer, CfgSigner::Slot(0));
    let (c, cwords) = pair_join(&p, &tc, j).expect("join");
    assert_eq!(cwords.as_ref(), Some(&words));
    let mut ec = engine(&p, &c, "ec");
    clean(&sync(&p, &mut ec, &c, &tc, T0 + 3));
    assert_eq!(read(&mut ec, &c.roster(), "b.md").as_deref(), Some(&b"from B\n"[..]));
    // expired offer
    let late = pair_offer(&p, &ta, &a, None, now_s()).unwrap();
    assert_eq!(pair_accept(&p, &client(&s), &late, "late", now_s() + 3600).unwrap_err(), VaultError::Expired);
    clean(&sync(&p, &mut ea, &a, &ta, T0 + 4));

    // no revocation (operator decision 2026-11-19): one VK epoch, slots never leave
    let root = root_of(&p, &words).unwrap();
    assert_eq!(a.env.keyring.len(), 1);
    assert!(a.roster().key(&b.device.id).is_some());
    let mut b = b;
    b.refresh_chain(&p, &tb).expect("B stays a member");
    let head = a.chain.head().clone();
    let head_hash = p.sha256(&head.encode().unwrap());
    let next_by = |g0: &sync_proto::vault::CfgGen, slot: u8, key: &sync_crypto::fips::FipsSigningKey| {
        let mut g = g0.clone();
        g.gen += 1;
        g.prev = head_hash;
        g.signer = CfgSigner::Slot(slot);
        g.sig.clear();
        let signed = g.encode_signed().unwrap();
        g.sig = p.sign(key, &cfg_digest(&p, &signed)).unwrap();
        (g, signed)
    };
    let (g, sg) = next_by(&head, 0, &a.device.key);
    verify_gen(&p, Some((&head, &head_hash)), &g, &sg).expect("control: an active slot may sign gen+1");
    // a key-epoch bump (what revocation used to publish) is refused
    let mut bump = head.clone();
    bump.vk_epoch = 1;
    let (g, sg) = next_by(&bump, 0, &a.device.key);
    assert_eq!(
        verify_gen(&p, Some((&head, &head_hash)), &g, &sg).unwrap_err(),
        VaultError::Chain("vk_epoch must be 0 (suite v1 has no key rotation)".into())
    );
    // dropping a device's slot (an implicit revocation) is refused
    let mut drop_b = head.clone();
    assert_eq!(drop_b.slots[1].device_id, b.device.id);
    drop_b.slots[1] = sync_proto::vault::Slot::empty();
    let (g, sg) = next_by(&drop_b, 0, &a.device.key);
    assert_eq!(
        verify_gen(&p, Some((&head, &head_hash)), &g, &sg).unwrap_err(),
        VaultError::Chain("a device disappeared from the slot table".into())
    );
    // an unknown device (signed commit, never on the roster) is rejected
    let stranger = Phrase::generate(&p).unwrap();
    let tx = client(&s);
    let tok2 = s.app.create_enrollment(None, 600).unwrap();
    let x = create_vault(&p, &tx, &s.url, &tok2, &stranger, "stranger").unwrap();
    assert!(a.roster().key(&x.device.id).is_none(), "a device of another vault is unknown here");

    // D: fresh device from the words later sees the same tree, one key epoch
    let td = client(&s);
    let d = bootstrap(&p, &td, &s.url, &words, "tablet").unwrap();
    assert_eq!(d.env.keyring.len(), 1);
    assert_eq!(d.chain.head().vk_epoch, 0);
    let mut ed = engine(&p, &d, "ed");
    clean(&sync(&p, &mut ed, &d, &td, T0 + 11));
    assert_eq!(read(&mut ed, &d.roster(), "a.md").as_deref(), Some(&b"from A\n"[..]));
    assert_eq!(read(&mut ed, &d.roster(), "b.md").as_deref(), Some(&b"from B\n"[..]));

    // destroy: without proof refused, with the words' K_destroy accepted
    let ch = ta.destroy_request(&a.ns()).unwrap();
    let bad = ta.destroy_confirm(&a.ns(), &DestroyConfirm { confirm_token: ch.confirm_token, ts: now_s() as i64, sig: "A".repeat(86) });
    assert!(matches!(bad, Err(TransportError::Api { code: ErrorCode::Forbidden, .. })), "{bad:?}");
    let receipt = destroy(&p, &ta, &a.env, a.server, &root, now_s() as i64).expect("destroy with proof");
    assert_eq!(receipt.ns, a.ns());
    let after = ta.list(&a.ns(), sync_proto::api::ListQuery { after: 0, wait: 0, limit: 10 });
    assert!(after.is_err(), "vault gone after destroy");
}

#[test]
fn keystore_entries_and_daily_round_trip() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let s = server();
    let words = Phrase::generate(&p).unwrap();
    let t = client(&s);
    let tok = s.app.create_enrollment(None, 600).unwrap();
    let a = create_vault(&p, &t, &s.url, &tok, &words, "laptop").unwrap();
    let dir = tmpdir("ks");
    let ks = FileKeystore::open(&dir, Box::new(FixedPresence(true))).unwrap();
    let vid = a.env.vault_id;
    ks.put(&vid, Entry::Daily, &a.to_daily().unwrap()).unwrap();
    ks.put(&vid, Entry::Backup, &words.s_bytes()[..]).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&ks.path(&vid, Entry::Daily)), 0o600);
        assert_eq!(mode(&ks.path(&vid, Entry::Backup)), 0o600);
    }
    // daily: no prompt; reload re-verifies the chain and keeps the device key
    let daily = ks.get(&vid, Entry::Daily).unwrap().unwrap();
    let m = Member::from_daily(&p, &t, &daily).unwrap();
    assert_eq!(m.device.id, a.device.id);
    assert_eq!(m.device.pubkey, a.device.pubkey);
    assert_eq!(m.env, a.env);
    let mut e = engine(&p, &m, "ks");
    e.commit_local(&m.device.signer(), &m.roster(), T0, vec![put("n.md", b"x\n")]).unwrap();
    clean(&sync(&p, &mut e, &m, &t, T0));
    // backup: behind user presence
    let b = ks.get(&vid, Entry::Backup).unwrap().unwrap();
    assert_eq!(&b[..], &words.s_bytes()[..]);
    let denied = FileKeystore::open(&dir, Box::new(FixedPresence(false))).unwrap();
    assert_eq!(denied.get(&vid, Entry::Backup).unwrap_err(), KeystoreError::Denied);
    assert!(denied.get(&vid, Entry::Daily).unwrap().is_some(), "daily never prompts");
    denied.delete(&vid, Entry::Backup).unwrap();
    assert!(denied.get(&vid, Entry::Backup).unwrap().is_none());
}

/// CodeRabbit #238 keystore:134: concurrent writers each get their own
/// create_new temp file (no shared ".tmp" truncated under another writer);
/// the entry ends up as exactly one writer's bytes and no temp is left.
#[cfg(unix)]
#[test]
fn keystore_concurrent_puts_use_unique_temp_files() {
    let dir = tmpdir("ks-race");
    let ks = Arc::new(FileKeystore::open(&dir, Box::new(FixedPresence(true))).unwrap());
    let vid = sync_proto::Id16([7u8; 16]);
    let hs: Vec<_> = (0..8u8)
        .map(|i| {
            let ks = ks.clone();
            std::thread::spawn(move || {
                for _ in 0..25 {
                    ks.put(&vid, Entry::Daily, &[i; 4096]).unwrap();
                }
            })
        })
        .collect();
    for h in hs {
        h.join().unwrap();
    }
    let got = ks.get(&vid, Entry::Daily).unwrap().unwrap();
    assert_eq!(got.len(), 4096);
    assert!(got.iter().all(|b| *b == got[0]), "torn keystore entry");
    let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left.len(), 1, "temp files left behind: {left:?}");
}

/// Serves at most `cfg_gens` cfg generations per chain walk (a server
/// replaying an older cfg chain); everything else passes through.
struct StaleCfg<'a> {
    inner: &'a HttpTransport,
    cfg_gens: usize,
    seen: std::sync::Mutex<usize>,
}

impl Transport for StaleCfg<'_> {
    fn health(&self) -> sync_core::transport::Result<sync_proto::api::Health> {
        self.inner.health()
    }
    fn enroll(&self, t: &str, k: &[u8]) -> sync_core::transport::Result<sync_proto::api::EnrollResponse> {
        self.inner.enroll(t, k)
    }
    fn put(&self, kind: ObjKind, ns: &Id16, id: &Id16, b: &[u8]) -> sync_core::transport::Result<sync_core::transport::PutOutcome> {
        self.inner.put(kind, ns, id, b)
    }
    fn get(&self, kind: ObjKind, ns: &Id16, id: &Id16, r: Option<sync_core::transport::Range>) -> sync_core::transport::Result<sync_core::transport::Fetched> {
        if kind == ObjKind::Cfg {
            let mut n = self.seen.lock().unwrap();
            if *n >= self.cfg_gens {
                *n = 0;
                return Err(TransportError::Api { status: 404, code: ErrorCode::NotFound, detail: "stale".into() });
            }
            *n += 1;
        }
        self.inner.get(kind, ns, id, r)
    }
    fn get_deleted(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> sync_core::transport::Result<Vec<u8>> {
        self.inner.get_deleted(kind, ns, id)
    }
    fn missing(&self, ns: &Id16, ids: &[Id16]) -> sync_core::transport::Result<Vec<Id16>> {
        self.inner.missing(ns, ids)
    }
    fn list(&self, ns: &Id16, q: sync_proto::api::ListQuery) -> sync_core::transport::Result<sync_proto::api::ListResponse> {
        self.inner.list(ns, q)
    }
    fn delete(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> sync_core::transport::Result<()> {
        self.inner.delete(kind, ns, id)
    }
    fn destroy_request(&self, ns: &Id16) -> sync_core::transport::Result<sync_proto::api::DestroyChallenge> {
        self.inner.destroy_request(ns)
    }
    fn destroy_confirm(&self, ns: &Id16, c: &DestroyConfirm) -> sync_core::transport::Result<sync_proto::api::DestroyReceipt> {
        self.inner.destroy_confirm(ns, c)
    }
}

#[test]
fn refresh_refuses_a_rolled_back_cfg_chain() {
    let p = FipsProvider::new().expect("FIPS module in FIPS mode");
    let s = server();
    let words = Phrase::generate(&p).unwrap();
    let ta = client(&s);
    let tok = s.app.create_enrollment(None, 600).unwrap();
    let mut a = create_vault(&p, &ta, &s.url, &tok, &words, "laptop").expect("create");
    let tb = client(&s);
    let _b = bootstrap(&p, &tb, &s.url, &words, "desktop").expect("bootstrap appends gen 1");
    a.refresh_chain(&p, &ta).expect("honest refresh");
    assert_eq!(a.chain.gens.len(), 2);
    let accepted = a.chain.clone();

    // the server replays gen 0 only: valid signatures, but older than what A accepted
    let stale = StaleCfg { inner: &ta, cfg_gens: 1, seen: std::sync::Mutex::new(0) };
    assert_eq!(a.refresh_chain(&p, &stale).unwrap_err(), VaultError::Chain("cfg chain rolled back or forked".into()));
    assert_eq!(a.chain, accepted, "the accepted chain is kept");
    // control: the full chain still refreshes
    a.refresh_chain(&p, &ta).expect("refresh after the replay");
}
