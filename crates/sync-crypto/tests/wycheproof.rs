// SPDX-License-Identifier: GPL-3.0-or-later
//! M2b: Wycheproof (C2SP, pinned commit, vectors/wycheproof/README.md)
//! against FipsProvider (+ SoftProvider for HMAC/HKDF).
use serde_json::Value;
use sync_crypto::fips::FipsProvider;
use sync_crypto::soft::{hex, SoftProvider};
use sync_crypto::{content_sha256, ct_eq, CryptoError, CryptoProvider, SigAlg};

fn load(name: &str, sha: &str) -> Value {
    let path = format!("{}/vectors/wycheproof/{name}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    assert_eq!(hex(&content_sha256(&bytes)), sha, "{name} differs from the pinned upstream file");
    serde_json::from_slice(&bytes).unwrap()
}

fn unhex(v: &Value) -> Vec<u8> {
    let s = v.as_str().expect("hex string");
    assert_eq!(s.len() % 2, 0);
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn groups(v: &Value) -> &Vec<Value> {
    v["testGroups"].as_array().unwrap()
}

fn tests(g: &Value) -> &Vec<Value> {
    g["tests"].as_array().unwrap()
}

/// Wycheproof `result`: valid / invalid / acceptable (acceptable = either).
fn expect_ok(t: &Value) -> Option<bool> {
    match t["result"].as_str().unwrap() {
        "valid" => Some(true),
        "invalid" => Some(false),
        _ => None,
    }
}

#[test]
fn wycheproof_aes_256_gcm() {
    let p = FipsProvider::new().unwrap();
    let v = load("aes_gcm_test.json", "985e5ecc172e181eaf49e89508b9470dcf478002eb7e8559c707eb42dc97dfe7");
    let (mut ran, mut skipped) = (0, 0);
    for g in groups(&v) {
        let shape = (g["keySize"].as_u64(), g["ivSize"].as_u64(), g["tagSize"].as_u64());
        if shape != (Some(256), Some(96), Some(128)) {
            skipped += tests(g).len();
            continue;
        }
        for t in tests(g) {
            let id = t["tcId"].as_u64().unwrap();
            let key: [u8; 32] = unhex(&t["key"]).try_into().unwrap();
            let iv: [u8; 12] = unhex(&t["iv"]).try_into().unwrap();
            let mut ct = unhex(&t["ct"]);
            ct.extend_from_slice(&unhex(&t["tag"]));
            let got = p.aead_open(&key, &iv, &unhex(&t["aad"]), &ct);
            match expect_ok(t) {
                Some(true) => assert_eq!(got.as_deref(), Ok(&unhex(&t["msg"])[..]), "tc {id}"),
                Some(false) => assert_eq!(got, Err(CryptoError::Verify), "tc {id}"),
                None => {}
            }
            ran += 1;
        }
    }
    assert!(ran >= 60, "ran {ran}");
    eprintln!("wycheproof aes-gcm: ran {ran}, skipped {skipped} (non-v1 shapes)");
}

#[test]
fn wycheproof_ecdsa_p256_sha256_p1363() {
    let p = FipsProvider::new().unwrap();
    let v = load(
        "ecdsa_secp256r1_sha256_p1363_test.json",
        "c60de693930e386c3a5472d08081623ef8504decc54b38ac01ec6b2a2575c986",
    );
    let mut ran = 0;
    for g in groups(&v) {
        assert_eq!(g["publicKey"]["curve"], "secp256r1");
        assert_eq!(g["sha"], "SHA-256");
        let pk = unhex(&g["publicKey"]["uncompressed"]);
        for t in tests(g) {
            let id = t["tcId"].as_u64().unwrap();
            let got = p.verify(SigAlg::EcdsaP256Sha256, &pk, &unhex(&t["msg"]), &unhex(&t["sig"]));
            match expect_ok(t) {
                Some(true) => assert_eq!(got, Ok(()), "tc {id}"),
                Some(false) => assert_eq!(got, Err(CryptoError::Verify), "tc {id}"),
                None => {}
            }
            ran += 1;
        }
    }
    assert_eq!(ran, 262);
}

/// Approved mode refuses HMAC / HKDF keys below 112 bits (SP #5586 §2.6).
const MIN_APPROVED_KEY: usize = 14;

#[test]
fn wycheproof_hmac_sha256() {
    let (p, s) = (FipsProvider::new().unwrap(), SoftProvider);
    let v = load("hmac_sha256_test.json", "2d201cfa61d1bf95e6f5d07d96634b4a348b31e8eaa277ad7c8d09677b7a743f");
    let (mut ran, mut refused) = (0, 0);
    for g in groups(&v) {
        let tag_len = g["tagSize"].as_u64().unwrap() as usize / 8;
        for t in tests(g) {
            let id = t["tcId"].as_u64().unwrap();
            let (k, m, tag) = (unhex(&t["key"]), unhex(&t["msg"]), unhex(&t["tag"]));
            let mut macs = vec![("soft", s.hmac_sha256(&k, &m))];
            if k.len() >= MIN_APPROVED_KEY {
                macs.push(("fips", p.hmac_sha256(&k, &m)));
            } else {
                assert_eq!(p.try_hmac_sha256(&k, &m), Err(CryptoError::Module), "fips tc {id} short key accepted");
                refused += 1;
            }
            for (name, mac) in macs {
                let ok = tag.len() == tag_len && ct_eq(&mac[..tag_len], &tag);
                if let Some(want) = expect_ok(t) {
                    assert_eq!(ok, want, "{name} tc {id}");
                }
            }
            ran += 1;
        }
    }
    assert_eq!(ran, 174);
    eprintln!("wycheproof hmac: ran {ran}, fips refused {refused} short-key cases (approved mode)");
}

#[test]
fn wycheproof_hkdf_sha256() {
    let (p, s) = (FipsProvider::new().unwrap(), SoftProvider);
    let v = load("hkdf_sha256_test.json", "bb2b462a38b251cb52a2aede706d6d4b62b26864f4e80c95497507ddb07c5f1e");
    let (mut ran, mut refused) = (0, 0);
    for g in groups(&v) {
        for t in tests(g) {
            let id = t["tcId"].as_u64().unwrap();
            let size = t["size"].as_u64().unwrap() as usize;
            let (ikm, salt, info) = (unhex(&t["ikm"]), unhex(&t["salt"]), unhex(&t["info"]));
            for (name, prov) in [("fips", &p as &dyn Hkdf), ("soft", &s as &dyn Hkdf)] {
                let mut out = vec![0u8; size];
                let r = prov.hkdf(&ikm, &salt, &info, &mut out);
                if name == "fips" && ikm.len() < MIN_APPROVED_KEY {
                    assert!(r.is_err(), "fips tc {id}: short IKM accepted");
                    refused += 1;
                    continue;
                }
                match expect_ok(t) {
                    Some(true) => {
                        assert_eq!(r, Ok(()), "{name} tc {id}");
                        assert_eq!(out, unhex(&t["okm"]), "{name} tc {id}");
                    }
                    Some(false) => assert!(r.is_err(), "{name} tc {id} accepted"),
                    None => {}
                }
            }
            ran += 1;
        }
    }
    assert_eq!(ran, 86);
    eprintln!("wycheproof hkdf: ran {ran}, fips refused {refused} short-IKM cases (approved mode)");
}

/// Object-safe view of the one provider method this test needs.
trait Hkdf {
    fn hkdf(&self, ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) -> Result<(), CryptoError>;
}

impl<P: CryptoProvider> Hkdf for P {
    fn hkdf(&self, ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) -> Result<(), CryptoError> {
        self.hkdf_sha256(ikm, salt, info, out)
    }
}
