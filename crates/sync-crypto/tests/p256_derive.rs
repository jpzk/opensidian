// SPDX-License-Identifier: GPL-3.0-or-later
//! M2d / open questions 10 + 12: raw P-256 private-key import works in FIPS
//! mode on the pinned module, and phrase-derived signing keys (K_destroy,
//! K_rsig) re-derive identically from the words alone.
//!
//! Independent answers: d = 1 -> Q = G and d = n - 1 -> Q = -G (SP 800-186
//! base point; -G = (Gx, p - Gy), computed outside this code). The key-tree
//! HKDF chain is cross-checked against the sha2-based SoftProvider.
use sync_crypto::fips::{FipsProvider, FipsSigningKey};
use sync_crypto::keytree::{
    ct_le, derive_root, p256_scalar_from_candidate, p256_scalar_from_seed, p256_scalar_select,
    P256_N,
};
use sync_crypto::soft::{hex, SoftProvider};
use sync_crypto::words::Phrase;
use sync_crypto::{CryptoError, CryptoProvider, SigAlg};

const GX: &str = "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296";
const GY: &str = "4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5";
const P_MINUS_GY: &str = "b01cbd1c01e58065711814b583f061e9d431cca994cea1313449bf97c840ae0a";

fn fips() -> FipsProvider {
    aws_lc_rs::try_fips_mode().expect("FIPS mode");
    FipsProvider::new().expect("FIPS provider")
}

fn be(last: u8) -> [u8; 32] {
    let mut b = [0u8; 32];
    b[31] = last;
    b
}

fn n_minus(k: u8) -> [u8; 32] {
    let mut b = P256_N;
    b[31] -= k; // n ends in 0x51, no borrow for small k
    b
}

#[test]
fn raw_scalar_import_in_fips_mode_d1_is_g() {
    let _ = fips();
    let k = FipsSigningKey::from_p256_scalar(&be(1)).expect("module accepts raw d=1");
    assert_eq!(hex(&k.public_key()), format!("04{GX}{GY}"));
}

#[test]
fn raw_scalar_import_n_minus_1_is_minus_g() {
    let _ = fips();
    let k = FipsSigningKey::from_p256_scalar(&n_minus(1)).unwrap();
    assert_eq!(hex(&k.public_key()), format!("04{GX}{P_MINUS_GY}"));
}

#[test]
fn raw_scalar_out_of_range_rejected() {
    let _ = fips();
    for d in [[0u8; 32], P256_N, [0xffu8; 32], {
        let mut b = P256_N;
        b[31] += 1;
        b
    }] {
        assert_eq!(FipsSigningKey::from_p256_scalar(&d).err(), Some(CryptoError::Invalid));
    }
}

#[test]
fn imported_key_signs_verifies_and_round_trips_pkcs8() {
    let p = fips();
    let k = FipsSigningKey::from_seed(&p, &[7u8; 32]).unwrap();
    let pk = k.public_key();
    let sig = p.sign(&k, b"osy destroy v1 test").unwrap();
    p.verify(SigAlg::EcdsaP256Sha256, &pk, b"osy destroy v1 test", &sig).unwrap();
    assert!(p.verify(SigAlg::EcdsaP256Sha256, &pk, b"other", &sig).is_err());
    let back = FipsSigningKey::from_pkcs8(&k.to_pkcs8().unwrap()).unwrap();
    assert_eq!(back.public_key(), pk);
}

#[test]
fn ct_le_and_candidate_boundaries() {
    assert!(ct_le(&be(1), &be(1)));
    assert!(ct_le(&be(0), &be(1)));
    assert!(!ct_le(&be(2), &be(1)));
    assert!(ct_le(&be(0xff), &P256_N));
    assert!(!ct_le(&P256_N, &n_minus(1)));
    // A.2.2: accept c <= n-2, d = c+1
    assert_eq!(*p256_scalar_from_candidate(&[0u8; 32]).unwrap(), be(1));
    assert_eq!(*p256_scalar_from_candidate(&n_minus(2)).unwrap(), n_minus(1));
    assert!(p256_scalar_from_candidate(&n_minus(1)).is_none());
    assert!(p256_scalar_from_candidate(&P256_N).is_none());
    assert!(p256_scalar_from_candidate(&[0xffu8; 32]).is_none());
    // carry propagates across bytes
    let mut c = [0u8; 32];
    c[30] = 0x01;
    c[31] = 0xff;
    let mut want = [0u8; 32];
    want[30] = 0x02;
    assert_eq!(*p256_scalar_from_candidate(&c).unwrap(), want);
}

#[test]
fn counter_retry_takes_first_accepted_candidate() {
    let mut seen = Vec::new();
    let d = p256_scalar_select(|ctr, out| {
        seen.push(ctr);
        *out = if ctr < 2 { [0xffu8; 32] } else { be(41) };
        Ok(())
    })
    .unwrap();
    assert_eq!(seen, vec![0, 1, 2]);
    assert_eq!(*d, be(42));
    // a provider that never yields an acceptable candidate fails closed
    let r = p256_scalar_select(|_, out| {
        *out = P256_N;
        Ok(())
    });
    assert_eq!(r.err(), Some(CryptoError::Module));
}

#[test]
fn scalar_derivation_matches_soft_provider() {
    let p = fips();
    for s in [[0u8; 32], [7u8; 32], [0xa5u8; 32]] {
        let a = p256_scalar_from_seed(&p, &s).unwrap();
        let b = p256_scalar_from_seed(&SoftProvider, &s).unwrap();
        assert_eq!(*a, *b);
        // ctr 0 candidate + 1 (accepted with p ~= 1 - 2^-32)
        let mut c = [0u8; 32];
        sync_crypto::soft::hkdf_sha256(&s, &[0u8; 32], b"osy p256 v1\x00", &mut c).unwrap();
        assert_eq!(*a, *p256_scalar_from_candidate(&c).unwrap());
    }
}

#[test]
fn k_destroy_rederives_from_words_on_a_fresh_device() {
    let p = fips();
    // device A creates the vault
    let ph = Phrase::generate(&p).unwrap();
    let words = ph.to_text(&p);
    let ra = derive_root(&p, &ph.s_bytes()).unwrap();
    let ka = FipsSigningKey::from_seed(&p, &ra.k_destroy).unwrap();
    // device B only has the words
    let ph_b = Phrase::parse(&p, &words).unwrap();
    let rb = derive_root(&p, &ph_b.s_bytes()).unwrap();
    let kb = FipsSigningKey::from_seed(&p, &rb.k_destroy).unwrap();
    assert_eq!(ka.public_key(), kb.public_key());
    assert_eq!(ra.loc, rb.loc);
    let msg = b"osy destroy v1 confirm";
    let sig = p.sign(&kb, msg).unwrap();
    p.verify(SigAlg::EcdsaP256Sha256, &ka.public_key(), msg, &sig).unwrap();
    // recovery key is a different key
    let krs = FipsSigningKey::from_seed(&p, &rb.k_rsig).unwrap();
    assert_ne!(krs.public_key(), kb.public_key());
}

#[test]
fn root_chain_matches_soft_provider() {
    let p = fips();
    let s = [0x11u8; 22];
    let a = derive_root(&p, &s).unwrap();
    let b = derive_root(&SoftProvider, &s).unwrap();
    assert_eq!(*a.k_wrap, *b.k_wrap);
    assert_eq!(a.loc, b.loc);
    assert_eq!(*a.k_rsig, *b.k_rsig);
    assert_eq!(*a.k_destroy, *b.k_destroy);
    // all four children distinct
    let v = [a.k_wrap.to_vec(), a.loc.to_vec(), a.k_rsig.to_vec(), a.k_destroy.to_vec()];
    for i in 0..4 {
        for j in i + 1..4 {
            assert_ne!(v[i], v[j]);
        }
    }
}
