// SPDX-License-Identifier: GPL-3.0-or-later
//! M2b object containers (spec §3) on the AWS-LC FIPS module: round trips
//! over every kind and segment boundary, zstd, and every rejection the spec
//! requires (commitment before decrypt, tamper, truncation, extension,
//! reordering, wrong id/vault/key, padding, zip bomb).
use std::cell::Cell;

use sync_core::object::{
    open, open_inner, seal, seal_inner, Compress, ObjectCtx, ObjectError, MAX_BODY,
};
use sync_crypto::fips::FipsProvider;
use sync_crypto::{CryptoError, CryptoProvider, Sealed, SigAlg, NONCE_LEN};
use sync_proto::container::{
    container_len, encode_inner, padme, Comp, Kind, Suite, CHUNK_PLAINTEXT_LEN, HEADER_LEN,
    INNER_HEADER_LEN, SEG, SEG_OVERHEAD,
};
use sync_proto::Id16;

fn fips() -> FipsProvider {
    FipsProvider::new().expect("FIPS module in FIPS mode")
}

fn rand<const N: usize>(p: &FipsProvider) -> [u8; N] {
    let mut b = [0u8; N];
    p.random(&mut b).unwrap();
    b
}

fn ctx(p: &FipsProvider) -> ObjectCtx {
    ObjectCtx {
        suite: Suite::P256,
        vault_id: Id16(rand(p)),
        obj_id: Id16(rand(p)),
    }
}

/// Counts AEAD opens so a test can prove the commitment is checked first.
struct Counting<'a> {
    inner: &'a FipsProvider,
    opens: Cell<usize>,
}

impl CryptoProvider for Counting<'_> {
    type SigningKey = <FipsProvider as CryptoProvider>::SigningKey;
    fn name(&self) -> &'static str {
        "counting"
    }
    fn fips_mode(&self) -> bool {
        self.inner.fips_mode()
    }
    fn sha256(&self, d: &[u8]) -> [u8; 32] {
        self.inner.sha256(d)
    }
    fn hmac_sha256(&self, k: &[u8], m: &[u8]) -> [u8; 32] {
        self.inner.hmac_sha256(k, m)
    }
    fn hkdf_sha256(&self, i: &[u8], s: &[u8], f: &[u8], o: &mut [u8]) -> Result<(), CryptoError> {
        self.inner.hkdf_sha256(i, s, f, o)
    }
    fn random(&self, o: &mut [u8]) -> Result<(), CryptoError> {
        self.inner.random(o)
    }
    fn aead_seal(&self, k: &[u8; 32], a: &[u8], pt: &[u8]) -> Result<Sealed, CryptoError> {
        self.inner.aead_seal(k, a, pt)
    }
    fn aead_open(
        &self,
        k: &[u8; 32],
        n: &[u8; NONCE_LEN],
        a: &[u8],
        c: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        self.opens.set(self.opens.get() + 1);
        self.inner.aead_open(k, n, a, c)
    }
    fn generate_signing_key(&self, a: SigAlg) -> Result<(Self::SigningKey, Vec<u8>), CryptoError> {
        self.inner.generate_signing_key(a)
    }
    fn sign(&self, k: &Self::SigningKey, m: &[u8]) -> Result<Vec<u8>, CryptoError> {
        self.inner.sign(k, m)
    }
    fn verify(&self, a: SigAlg, pk: &[u8], m: &[u8], s: &[u8]) -> Result<(), CryptoError> {
        self.inner.verify(a, pk, m, s)
    }
}

/// Wire offsets of the segments of a container: (start, len).
fn segments(c: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut off = HEADER_LEN;
    while off < c.len() {
        let n = (c.len() - off).min(SEG + SEG_OVERHEAD);
        out.push((off, n));
        off += n;
    }
    out
}

#[test]
fn roundtrip_every_kind_and_segment_boundary() {
    let p = fips();
    let k = rand::<32>(&p);
    // inner len = 10 + body: one segment exactly full, one byte over, 3 segments
    for blen in [0usize, 1, 100, SEG - INNER_HEADER_LEN, SEG - INNER_HEADER_LEN + 1, 3 * SEG + 7] {
        let body: Vec<u8> = (0..blen).map(|i| (i * 31 % 251) as u8).collect();
        for kind in [Kind::Batch, Kind::Envelope, Kind::Cfg, Kind::Pairing] {
            let c = ctx(&p);
            let obj = seal(&p, &k, &c, kind, &body, Compress::Never).unwrap();
            let inner_len = padme((INNER_HEADER_LEN + blen) as u64) as usize;
            assert_eq!(obj.len(), container_len(inner_len), "blen {blen}");
            let o = open(&p, &k, &c, &obj).unwrap();
            assert_eq!((o.kind, o.comp), (kind, Comp::None));
            assert_eq!(o.body, body);
        }
    }
}

#[test]
fn chunk_is_uniform_4mib_and_roundtrips() {
    let p = fips();
    let k = rand::<32>(&p);
    let c = ctx(&p);
    let small = seal(&p, &k, &c, Kind::Chunk, b"tail", Compress::Never).unwrap();
    let mut full = vec![0u8; 4 * 1024 * 1024];
    p.random(&mut full).unwrap();
    let big = seal(&p, &k, &c, Kind::Chunk, &full, Compress::IfSmaller).unwrap();
    assert_eq!(small.len(), container_len(CHUNK_PLAINTEXT_LEN));
    assert_eq!(big.len(), small.len(), "chunks must be size-uniform");
    let o = open(&p, &k, &c, &big).unwrap();
    assert_eq!(o.comp, Comp::None, "random data is not kept compressed");
    assert_eq!(o.body, full);
    assert!(matches!(
        seal(&p, &k, &c, Kind::Chunk, &vec![0u8; 4 * 1024 * 1024 + 1], Compress::Never),
        Err(ObjectError::Encode(_))
    ));
}

#[test]
fn zstd_used_when_smaller() {
    let p = fips();
    let k = rand::<32>(&p);
    let c = ctx(&p);
    let text = "# note\n\nsome markdown that repeats. ".repeat(4000).into_bytes();
    let raw = seal(&p, &k, &c, Kind::Batch, &text, Compress::Never).unwrap();
    let z = seal(&p, &k, &c, Kind::Batch, &text, Compress::IfSmaller).unwrap();
    assert!(z.len() * 10 < raw.len(), "zstd {} vs raw {}", z.len(), raw.len());
    let o = open(&p, &k, &c, &z).unwrap();
    assert_eq!((o.kind, o.comp), (Kind::Batch, Comp::Zstd));
    assert_eq!(o.body, text);
    // tiny body: zstd frame overhead makes it larger -> stored raw
    let t = open(&p, &k, &c, &seal(&p, &k, &c, Kind::Cfg, b"x", Compress::IfSmaller).unwrap());
    assert_eq!(t.unwrap().comp, Comp::None);
}

#[test]
fn nonces_differ_so_ciphertexts_differ() {
    let p = fips();
    let k = rand::<32>(&p);
    let c = ctx(&p);
    let a = seal(&p, &k, &c, Kind::Batch, b"same", Compress::Never).unwrap();
    let b = seal(&p, &k, &c, Kind::Batch, b"same", Compress::Never).unwrap();
    assert_eq!(a[..HEADER_LEN], b[..HEADER_LEN], "header is deterministic");
    assert_ne!(a[HEADER_LEN..HEADER_LEN + NONCE_LEN], b[HEADER_LEN..HEADER_LEN + NONCE_LEN]);
    assert_ne!(a, b);
}

#[test]
fn commitment_checked_before_any_decrypt() {
    let p = fips();
    let k = rand::<32>(&p);
    let c = ctx(&p);
    let obj = seal(&p, &k, &c, Kind::Batch, &[5u8; 200_000], Compress::Never).unwrap();
    let cp = Counting { inner: &p, opens: Cell::new(0) };
    // wrong key, wrong vault id, flipped commitment byte: all rejected with 0 AEAD opens
    let wrong_vault = ObjectCtx { vault_id: Id16(rand(&p)), ..c };
    let mut bad_commit = obj.clone();
    bad_commit[30] ^= 1;
    let cases: [(&[u8; 32], &ObjectCtx, &[u8]); 3] =
        [(&rand::<32>(&p), &c, &obj[..]), (&k, &wrong_vault, &obj[..]), (&k, &c, &bad_commit[..])];
    for (key, cx, o) in cases {
        assert_eq!(open(&cp, key, cx, o), Err(ObjectError::Auth));
        assert_eq!(cp.opens.get(), 0, "AEAD touched before commitment check");
    }
    assert!(open(&cp, &k, &c, &obj).is_ok());
    assert_eq!(cp.opens.get(), segments(&obj).len());
}

#[test]
fn header_id_and_suite_must_match_request() {
    let p = fips();
    let k = rand::<32>(&p);
    let c = ctx(&p);
    let obj = seal(&p, &k, &c, Kind::Batch, b"hello", Compress::Never).unwrap();
    let other = ObjectCtx { obj_id: Id16(rand(&p)), ..c };
    assert_eq!(open(&p, &k, &other, &obj), Err(ObjectError::WrongId));
    let ed = ObjectCtx { suite: Suite::Ed25519, ..c };
    assert_eq!(open(&p, &k, &ed, &obj), Err(ObjectError::WrongSuite));
    // header suite byte rewritten to 0x01: suite mismatch (and the commit would fail too)
    let mut s = obj.clone();
    s[5] = 0x01;
    assert_eq!(open(&p, &k, &c, &s), Err(ObjectError::WrongSuite));
    assert!(matches!(open(&p, &k, &c, &obj[..40]), Err(ObjectError::Format(_))));
    assert!(matches!(open(&p, &k, &c, &obj[..HEADER_LEN]), Err(ObjectError::Format(_))));
}

#[test]
fn tamper_truncate_extend_reorder_rejected() {
    let p = fips();
    let k = rand::<32>(&p);
    let c = ctx(&p);
    let body: Vec<u8> = (0..3 * SEG).map(|i| i as u8).collect();
    let obj = seal(&p, &k, &c, Kind::Batch, &body, Compress::Never).unwrap();
    let segs = segments(&obj);
    assert_eq!(segs.len(), 4);

    // any flipped byte in any segment (nonce, ct, tag)
    for &(off, len) in &segs {
        for at in [off, off + NONCE_LEN, off + len - 1] {
            let mut t = obj.clone();
            t[at] ^= 0x80;
            assert_eq!(open(&p, &k, &c, &t), Err(ObjectError::Auth), "flip at {at}");
        }
    }
    // truncation at a segment boundary: new final segment was sealed with last=0
    for &(off, _) in &segs[1..] {
        assert_eq!(open(&p, &k, &c, &obj[..off]), Err(ObjectError::Auth), "cut at {off}");
    }
    // truncation inside the final segment
    assert_eq!(open(&p, &k, &c, &obj[..obj.len() - 1]), Err(ObjectError::Auth));
    // extension: a copy of the final segment appended
    let (lo, ll) = *segs.last().unwrap();
    let mut ext = obj.clone();
    ext.extend_from_slice(&obj[lo..lo + ll]);
    assert_eq!(open(&p, &k, &c, &ext), Err(ObjectError::Auth));
    // reordering two full segments
    let (a, n) = segs[0];
    let (b, _) = segs[1];
    let mut sw = obj[..HEADER_LEN].to_vec();
    sw.extend_from_slice(&obj[b..b + n]);
    sw.extend_from_slice(&obj[a..a + n]);
    sw.extend_from_slice(&obj[segs[2].0..]);
    assert_eq!(sw.len(), obj.len());
    assert_eq!(open(&p, &k, &c, &sw), Err(ObjectError::Auth));
    // a dangling final piece shorter than nonce+tag+1
    let mut dang = obj.clone();
    dang.extend_from_slice(&[0u8; SEG_OVERHEAD]);
    assert!(open(&p, &k, &c, &dang).is_err());
    // a single segment moved to another object id is rejected (AAD + commitment)
    let moved = ObjectCtx { obj_id: Id16(rand(&p)), ..c };
    let mut re = obj.clone();
    re[8..24].copy_from_slice(&moved.obj_id.0);
    assert_eq!(open(&p, &k, &moved, &re), Err(ObjectError::Auth));
}

#[test]
fn inner_padding_and_kind_enforced_after_decrypt() {
    let p = fips();
    let k = rand::<32>(&p);
    let c = ctx(&p);
    let mut inner = encode_inner(Kind::Batch, Comp::None, &[1u8; 100]).unwrap();
    *inner.last_mut().unwrap() = 9; // nonzero padding byte
    let obj = seal_inner(&p, &k, &c, &inner).unwrap();
    assert!(open_inner(&p, &k, &c, &obj).is_ok(), "authentic");
    assert!(matches!(open(&p, &k, &c, &obj), Err(ObjectError::Format(_))));
    let mut short = encode_inner(Kind::Batch, Comp::None, &[1u8; 100]).unwrap();
    short.pop();
    let obj = seal_inner(&p, &k, &c, &short).unwrap();
    assert!(matches!(open(&p, &k, &c, &obj), Err(ObjectError::Format(_))));
    let mut kind = encode_inner(Kind::Batch, Comp::None, b"k").unwrap();
    kind[0] = 0x09;
    let obj = seal_inner(&p, &k, &c, &kind).unwrap();
    assert!(matches!(open(&p, &k, &c, &obj), Err(ObjectError::Format(_))));
}

#[test]
fn zstd_bomb_and_garbage_rejected() {
    let p = fips();
    let k = rand::<32>(&p);
    let c = ctx(&p);
    let bomb = zstd::bulk::compress(&vec![0u8; MAX_BODY + 1], 3).unwrap();
    assert!(bomb.len() < 64 * 1024);
    let obj = seal_inner(&p, &k, &c, &encode_inner(Kind::Batch, Comp::Zstd, &bomb).unwrap()).unwrap();
    assert_eq!(open(&p, &k, &c, &obj), Err(ObjectError::Compression));
    let ok = zstd::bulk::compress(&vec![0u8; MAX_BODY], 3).unwrap();
    let obj = seal_inner(&p, &k, &c, &encode_inner(Kind::Batch, Comp::Zstd, &ok).unwrap()).unwrap();
    assert_eq!(open(&p, &k, &c, &obj).unwrap().body.len(), MAX_BODY);
    let junk = seal_inner(&p, &k, &c, &encode_inner(Kind::Batch, Comp::Zstd, b"not zstd").unwrap()).unwrap();
    assert_eq!(open(&p, &k, &c, &junk), Err(ObjectError::Compression));
}

#[test]
fn envelope_uses_zero_vault_id() {
    let p = fips();
    let k = rand::<32>(&p);
    let c = ObjectCtx { suite: Suite::P256, vault_id: Id16::ZERO, obj_id: Id16(rand(&p)) };
    let obj = seal(&p, &k, &c, Kind::Envelope, b"wrapped keys", Compress::Never).unwrap();
    assert_eq!(open(&p, &k, &c, &obj).unwrap().body, b"wrapped keys");
    let nz = ObjectCtx { vault_id: Id16([1; 16]), ..c };
    assert_eq!(open(&p, &k, &nz, &obj), Err(ObjectError::Auth));
}

/// Prints the fixed-input container vectors for vectors/container.vec
/// (osy-vec-1). Run once on the box: `cargo test -p sync-core --test object
/// -- --ignored --nocapture gen_container_kat`. The nonces come from the
/// module DRBG, so the output differs per run; the committed file is one
/// such run and `container_kat` (below) checks the OPEN direction.
#[test]
#[ignore]
fn gen_container_kat() {
    let p = fips();
    let k: [u8; 32] = std::array::from_fn(|i| i as u8);
    let c = ObjectCtx { suite: Suite::P256, vault_id: Id16([0x11; 16]), obj_id: Id16([0x22; 16]) };
    let hex = sync_crypto::soft::hex;
    for (id, kind, body, comp) in [
        ("batch-raw", Kind::Batch, b"osy container kat v1".to_vec(), Compress::Never),
        ("batch-zstd", Kind::Batch, "kat ".repeat(64).into_bytes(), Compress::IfSmaller),
    ] {
        let obj = seal(&p, &k, &c, kind, &body, comp).unwrap();
        println!("KAT [{id}]\nKAT body = {}\nKAT container = {}", hex(&body), hex(&obj));
    }
}
