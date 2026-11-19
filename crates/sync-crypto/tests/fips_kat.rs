// SPDX-License-Identifier: GPL-3.0-or-later
//! M2b known-answer tests against the AWS-LC FIPS module (FipsProvider):
//! crates/sync-crypto/vectors/*.vec (format docs/sync/test-vectors.md) plus
//! property checks (nonce uniqueness, tamper rejection, P-256 sign/verify,
//! provider parity with the reference SoftProvider).
use std::collections::BTreeMap;

use sync_crypto::fips::{FipsProvider, FipsSigningKey, P256_SIG_LEN};
use sync_crypto::soft::{hex, SoftProvider};
use sync_crypto::{
    check_commitment, derive_object_keys, key_commitment, CryptoError, CryptoProvider, SigAlg,
    HKDF_MAX,
};

type Case = BTreeMap<String, String>;

/// Minimal osy-vec-1 reader (std only): returns (area, cases).
fn load(name: &str) -> (String, Vec<(String, Case)>) {
    let path = format!("{}/vectors/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut area = String::new();
    let mut format_ok = false;
    let mut cases: Vec<(String, Case)> = Vec::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(id) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            cases.push((id.to_string(), Case::new()));
            continue;
        }
        let (k, v) = line.split_once(" =").unwrap_or_else(|| panic!("bad line {line:?}"));
        let v = v.strip_prefix(' ').unwrap_or(v).to_string();
        match cases.last_mut() {
            Some((_, c)) => {
                assert!(c.insert(k.to_string(), v).is_none(), "dup key {k}");
            }
            None => match k {
                "format" => format_ok = v == "osy-vec-1",
                "area" => area = v,
                "suite" => {}
                _ => panic!("unknown header key {k}"),
            },
        }
    }
    assert!(format_ok, "{name}: format line");
    assert!(!cases.is_empty(), "{name}: no cases");
    (area, cases)
}

fn unhex(s: &str) -> Vec<u8> {
    assert_eq!(s.len() % 2, 0, "odd hex");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn f<'a>(c: &'a Case, k: &str) -> &'a str {
    c.get(k).unwrap_or_else(|| panic!("missing field {k}"))
}

fn fips() -> FipsProvider {
    FipsProvider::new().expect("FIPS provider")
}

#[test]
fn provider_reports_fips() {
    let p = fips();
    assert!(p.fips_mode());
    assert!(p.name().contains("FIPS"));
}

#[test]
fn kat_aes_256_gcm() {
    let p = fips();
    let (area, cases) = load("aes-gcm.vec");
    assert_eq!(area, "aes-gcm");
    let mut n = 0;
    for (id, c) in &cases {
        let key: [u8; 32] = unhex(f(c, "key")).try_into().unwrap();
        let nonce: [u8; 12] = unhex(f(c, "nonce")).try_into().unwrap();
        let got = p.aead_open(&key, &nonce, &unhex(f(c, "aad")), &unhex(f(c, "ct")));
        match f(c, "result") {
            "valid" => assert_eq!(got.as_deref(), Ok(&unhex(f(c, "pt"))[..]), "{id}"),
            _ => assert_eq!(got, Err(CryptoError::Verify), "{id}"),
        }
        n += 1;
    }
    assert_eq!(n, 6);
}

#[test]
fn kat_hmac_sha256_both_providers() {
    let (p, s) = (fips(), SoftProvider);
    let (_, cases) = load("hmac-sha256.vec");
    for (id, c) in &cases {
        let (k, m) = (unhex(f(c, "key")), unhex(f(c, "msg")));
        assert_eq!(hex(&p.hmac_sha256(&k, &m)), f(c, "mac"), "fips {id}");
        assert_eq!(hex(&s.hmac_sha256(&k, &m)), f(c, "mac"), "soft {id}");
    }
}

#[test]
fn kat_hkdf_sha256_both_providers() {
    let (p, s) = (fips(), SoftProvider);
    let (_, cases) = load("hkdf-sha256.vec");
    for (id, c) in &cases {
        let want = unhex(f(c, "okm"));
        let (ikm, salt, info) = (unhex(f(c, "ikm")), unhex(f(c, "salt")), unhex(f(c, "info")));
        let mut a = vec![0u8; want.len()];
        p.hkdf_sha256(&ikm, &salt, &info, &mut a).unwrap();
        assert_eq!(a, want, "fips {id}");
        let mut b = vec![0u8; want.len()];
        s.hkdf_sha256(&ikm, &salt, &info, &mut b).unwrap();
        assert_eq!(b, want, "soft {id}");
    }
}

#[test]
fn hkdf_limits() {
    let p = fips();
    let mut max = vec![0u8; HKDF_MAX];
    p.hkdf_sha256(b"k", b"s", b"i", &mut max).unwrap();
    let mut over = vec![0u8; HKDF_MAX + 1];
    assert_eq!(p.hkdf_sha256(b"k", b"s", b"i", &mut over), Err(CryptoError::Invalid));
    // long output agrees with the reference implementation
    let mut soft = vec![0u8; HKDF_MAX];
    SoftProvider.hkdf_sha256(b"k", b"s", b"i", &mut soft).unwrap();
    assert_eq!(max, soft);
}

#[test]
fn sha256_kats() {
    let p = fips();
    assert_eq!(
        hex(&p.sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(&p.sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn spec_helpers_match_across_providers() {
    let (p, s) = (fips(), SoftProvider);
    let root = [0x42u8; 32];
    let a = derive_object_keys(&p, &root, &[9; 16], 2).unwrap();
    let b = derive_object_keys(&s, &root, &[9; 16], 2).unwrap();
    assert_eq!((a.enc, a.commit), (b.enc, b.commit));
    let t = key_commitment(&p, &a.commit, &[1; 16], &[9; 16], 2, 1);
    assert_eq!(t, key_commitment(&s, &b.commit, &[1; 16], &[9; 16], 2, 1));
    check_commitment(&p, &a.commit, &[1; 16], &[9; 16], 2, 1, &t).unwrap();
}

#[test]
fn aead_module_nonces_roundtrip_and_tamper() {
    let p = fips();
    let key = [7u8; 32];
    let aad = b"vault||obj||idx||last||suite||fmt";
    let mut nonces = std::collections::HashSet::new();
    for i in 0..256u32 {
        let pt = i.to_be_bytes().repeat(i as usize % 7 + 1);
        let s = p.aead_seal(&key, aad, &pt).unwrap();
        assert!(nonces.insert(s.nonce), "nonce repeated");
        assert_eq!(s.ct_and_tag.len(), pt.len() + 16);
        assert_eq!(p.aead_open(&key, &s.nonce, aad, &s.ct_and_tag).unwrap(), pt);
        let mut bad = s.ct_and_tag.clone();
        bad[0] ^= 1;
        assert_eq!(p.aead_open(&key, &s.nonce, aad, &bad), Err(CryptoError::Verify));
        assert_eq!(p.aead_open(&key, &s.nonce, b"other", &s.ct_and_tag), Err(CryptoError::Verify));
        assert_eq!(p.aead_open(&[8; 32], &s.nonce, aad, &s.ct_and_tag), Err(CryptoError::Verify));
    }
    assert_eq!(p.aead_open(&key, &[0; 12], aad, &[0; 15]), Err(CryptoError::Verify));
}

#[test]
fn drbg_fills_and_differs() {
    let p = fips();
    let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
    p.random(&mut a).unwrap();
    p.random(&mut b).unwrap();
    assert_ne!(a, b);
    assert_ne!(a, [0u8; 32]);
}

#[test]
fn p256_sign_verify_pkcs8() {
    let p = fips();
    let (k, pk) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
    assert_eq!(pk.len(), 65);
    assert_eq!(pk[0], 0x04);
    let sig = p.sign(&k, b"osy batch v1 digest").unwrap();
    assert_eq!(sig.len(), P256_SIG_LEN);
    p.verify(SigAlg::EcdsaP256Sha256, &pk, b"osy batch v1 digest", &sig).unwrap();
    let v = |pk: &[u8], m: &[u8], s: &[u8]| p.verify(SigAlg::EcdsaP256Sha256, pk, m, s);
    assert_eq!(v(&pk, b"osy batch v1 digesT", &sig), Err(CryptoError::Verify));
    let mut bad = sig.clone();
    bad[10] ^= 0x80;
    assert_eq!(v(&pk, b"osy batch v1 digest", &bad), Err(CryptoError::Verify));
    assert_eq!(v(&pk, b"osy batch v1 digest", &sig[..63]), Err(CryptoError::Verify));
    assert_eq!(v(&pk[..64], b"osy batch v1 digest", &sig), Err(CryptoError::Verify));
    // other key
    let (_, pk2) = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap();
    assert_eq!(v(&pk2, b"osy batch v1 digest", &sig), Err(CryptoError::Verify));
    // keystore round trip
    let der = k.to_pkcs8().unwrap();
    let k2 = FipsSigningKey::from_pkcs8(&der).unwrap();
    assert_eq!(k2.public_key(), pk);
    let sig2 = p.sign(&k2, b"m").unwrap();
    v(&pk, b"m", &sig2).unwrap();
    assert!(FipsSigningKey::from_pkcs8(b"not der").is_err());
}

#[test]
fn ed25519_rfc8032_known_answers() {
    // suite v1 signatures through the module: key import from the 32-byte
    // seed derives the RFC public key, signing is deterministic (RFC 8032),
    // and verification accepts exactly the valid cases.
    let p = fips();
    let (area, cases) = load("ed25519.vec");
    assert_eq!(area, "ed25519");
    assert_eq!(sync_crypto::V1_SIG_ALG, SigAlg::Ed25519);
    let mut valid = 0;
    for (id, c) in &cases {
        let sk: [u8; 32] = unhex(f(c, "sk")).try_into().unwrap();
        let (pk, msg, sig) = (unhex(f(c, "pk")), unhex(f(c, "msg")), unhex(f(c, "sig")));
        let k = FipsSigningKey::from_ed25519_seed(&sk).unwrap();
        assert_eq!(k.alg(), SigAlg::Ed25519, "{id}");
        assert_eq!(k.public_key(), pk, "{id}: derived public key");
        let got = p.verify(SigAlg::Ed25519, &pk, &msg, &sig);
        match f(c, "result") {
            "valid" => {
                got.unwrap_or_else(|e| panic!("{id}: {e:?}"));
                assert_eq!(p.sign(&k, &msg).unwrap(), sig, "{id}: deterministic signature");
                valid += 1;
            }
            "invalid" => assert_eq!(got, Err(CryptoError::Verify), "{id}"),
            r => panic!("{id}: result {r}"),
        }
    }
    assert_eq!(valid, 3);
}

#[test]
fn ed25519_sign_verify_pkcs8() {
    let p = fips();
    let (k, pk) = p.generate_signing_key(SigAlg::Ed25519).unwrap();
    assert_eq!(pk.len(), 32);
    let m = b"osy batch v1 digest";
    let sig = p.sign(&k, m).unwrap();
    assert_eq!(sig.len(), sync_crypto::fips::ED25519_SIG_LEN);
    let v = |pk: &[u8], m: &[u8], s: &[u8]| p.verify(SigAlg::Ed25519, pk, m, s);
    v(&pk, m, &sig).unwrap();
    assert_eq!(v(&pk, b"osy batch v1 digesT", &sig), Err(CryptoError::Verify));
    let mut bad = sig.clone();
    bad[10] ^= 0x80;
    assert_eq!(v(&pk, m, &bad), Err(CryptoError::Verify));
    assert_eq!(v(&pk, m, &sig[..63]), Err(CryptoError::Verify));
    assert_eq!(v(&pk[..31], m, &sig), Err(CryptoError::Verify));
    // a P-256 signature / key never verifies as Ed25519 and vice versa
    let pk_ec = p.generate_signing_key(SigAlg::EcdsaP256Sha256).unwrap().1;
    assert_eq!(v(&pk_ec, m, &sig), Err(CryptoError::Verify));
    assert_eq!(p.verify(SigAlg::EcdsaP256Sha256, &pk, m, &sig), Err(CryptoError::Verify));
    // keystore round trip keeps the algorithm
    let der = k.to_pkcs8().unwrap();
    let k2 = FipsSigningKey::from_pkcs8(&der).unwrap();
    assert_eq!((k2.alg(), k2.public_key()), (SigAlg::Ed25519, pk.clone()));
    v(&pk, b"m", &p.sign(&k2, b"m").unwrap()).unwrap();
    // phrase-derived (from_seed) keys are Ed25519 and stable
    let a = FipsSigningKey::from_seed(&p, &[7u8; 32]).unwrap();
    let b = FipsSigningKey::from_seed(&p, &[7u8; 32]).unwrap();
    assert_eq!((a.alg(), a.public_key()), (SigAlg::Ed25519, b.public_key()));
    assert_ne!(a.public_key(), FipsSigningKey::from_seed(&p, &[8u8; 32]).unwrap().public_key());
}
