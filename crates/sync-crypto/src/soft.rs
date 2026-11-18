// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Reference provider: SHA-256 / HMAC-SHA256 (RFC 2104) / HKDF-SHA256
//! (RFC 5869) on top of `sha2`, checked against the RFC 4231 and RFC 5869
//! vectors below. Used for test vectors and the local history store only.
//! AEAD, randomness and signatures are intentionally NOT implemented: they
//! come from the validated module (docs/sync/cmvp.md).

use sha2::{Digest, Sha256};

use crate::{CryptoError, CryptoProvider, Sealed, SigAlg, HKDF_MAX, NONCE_LEN};

const BLOCK: usize = 64;

/// Hash/MAC/KDF-only reference provider.
#[derive(Debug, Clone, Copy, Default)]
pub struct SoftProvider;

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for ((i, o), kb) in ipad.iter_mut().zip(opad.iter_mut()).zip(k.iter()) {
        *i ^= kb;
        *o ^= kb;
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(msg);
    let ih = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(ih);
    let mut out = [0u8; 32];
    out.copy_from_slice(&outer.finalize());
    out
}

/// HKDF-SHA256 extract + expand (RFC 5869).
pub fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) -> Result<(), CryptoError> {
    if out.len() > HKDF_MAX {
        return Err(CryptoError::Invalid);
    }
    // An empty salt equals HashLen zero bytes: both pad to a zero block.
    let prk = hmac_sha256(salt, ikm);
    let mut t: Vec<u8> = Vec::new();
    for (i, chunk) in out.chunks_mut(32).enumerate() {
        let mut msg = Vec::with_capacity(t.len() + info.len() + 1);
        msg.extend_from_slice(&t);
        msg.extend_from_slice(info);
        // i < 255 is guaranteed by the HKDF_MAX check.
        msg.push((i + 1) as u8);
        let block = hmac_sha256(&prk, &msg);
        chunk.copy_from_slice(&block[..chunk.len()]);
        t = block.to_vec();
    }
    Ok(())
}

/// Lowercase hex (diagnostics, tests).
pub fn hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 0x0f) as usize] as char);
    }
    s
}

impl CryptoProvider for SoftProvider {
    /// No signing keys: [`SoftProvider`] cannot sign.
    type SigningKey = ();

    fn name(&self) -> &'static str {
        "soft-sha2 (reference, hash/mac/kdf only)"
    }

    fn fips_mode(&self) -> bool {
        false
    }

    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        crate::content_sha256(data)
    }

    fn hmac_sha256(&self, key: &[u8], msg: &[u8]) -> [u8; 32] {
        hmac_sha256(key, msg)
    }

    fn hkdf_sha256(
        &self,
        ikm: &[u8],
        salt: &[u8],
        info: &[u8],
        out: &mut [u8],
    ) -> Result<(), CryptoError> {
        hkdf_sha256(ikm, salt, info, out)
    }

    fn random(&self, _out: &mut [u8]) -> Result<(), CryptoError> {
        Err(CryptoError::Unsupported)
    }

    fn aead_seal(&self, _key: &[u8; 32], _aad: &[u8], _pt: &[u8]) -> Result<Sealed, CryptoError> {
        Err(CryptoError::Unsupported)
    }

    fn aead_open(
        &self,
        _key: &[u8; 32],
        _nonce: &[u8; NONCE_LEN],
        _aad: &[u8],
        _ct_and_tag: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        Err(CryptoError::Unsupported)
    }

    fn generate_signing_key(&self, _alg: SigAlg) -> Result<((), Vec<u8>), CryptoError> {
        Err(CryptoError::Unsupported)
    }

    fn sign(&self, _key: &(), _msg: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Err(CryptoError::Unsupported)
    }

    fn verify(
        &self,
        _alg: SigAlg,
        _public_key: &[u8],
        _msg: &[u8],
        _sig: &[u8],
    ) -> Result<(), CryptoError> {
        Err(CryptoError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        assert_eq!(s.len() % 2, 0);
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn rfc4231_case1() {
        let mac = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex(&mac),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn rfc4231_case2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(
            hex(&mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn rfc4231_case6_long_key() {
        let mac = hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First",
        );
        assert_eq!(
            hex(&mac),
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        );
    }

    #[test]
    fn rfc5869_case1() {
        let salt = unhex("000102030405060708090a0b0c");
        let info = unhex("f0f1f2f3f4f5f6f7f8f9");
        let mut okm = [0u8; 42];
        hkdf_sha256(&[0x0b; 22], &salt, &info, &mut okm).unwrap();
        assert_eq!(
            hex(&okm),
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
        );
        // PRK of the same case
        assert_eq!(
            hex(&hmac_sha256(&salt, &[0x0b; 22])),
            "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5"
        );
    }

    #[test]
    fn rfc5869_case3_empty_salt_info() {
        let mut okm = [0u8; 42];
        hkdf_sha256(&[0x0b; 22], &[], &[], &mut okm).unwrap();
        assert_eq!(
            hex(&okm),
            "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8"
        );
    }

    #[test]
    fn hkdf_length_limit() {
        let mut ok = vec![0u8; HKDF_MAX];
        assert!(hkdf_sha256(b"k", b"", b"", &mut ok).is_ok());
        let mut too_long = vec![0u8; HKDF_MAX + 1];
        assert_eq!(
            hkdf_sha256(b"k", b"", b"", &mut too_long),
            Err(CryptoError::Invalid)
        );
    }

    #[test]
    fn soft_provider_refuses_aead_rng_sign() {
        let p = SoftProvider;
        assert!(!p.fips_mode());
        assert_eq!(p.random(&mut [0u8; 4]), Err(CryptoError::Unsupported));
        assert_eq!(p.aead_seal(&[0; 32], b"", b"x"), Err(CryptoError::Unsupported));
        assert_eq!(p.sign(&(), b"x"), Err(CryptoError::Unsupported));
        assert_eq!(p.sha256(b"abc"), crate::content_sha256(b"abc"));
    }
}
