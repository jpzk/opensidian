// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Release provider: every operation goes to the AWS-LC FIPS module through
//! aws-lc-rs (`fips` feature; pin + CMVP status in docs/sync/cmvp.md §7).
//!
//! - AEAD: AES-256-GCM via `RandomizedNonceKey` — the 96-bit nonce comes from
//!   the module DRBG inside the seal call, a caller can never choose it.
//! - HKDF-SHA256 / HMAC-SHA256 / SHA-256 / DRBG: module services.
//! - Signatures: ECDSA P-256 / SHA-256 (fixed 64-byte r||s, uncompressed
//!   65-byte public key). Ed25519 is NOT offered: the pinned module is not
//!   covered by a certificate listing EdDSA (cmvp.md §7), so suite 0x02 is the
//!   v1 suite and `SigAlg::Ed25519` returns `Unsupported`.

use aws_lc_rs::aead::{Aad, Nonce, RandomizedNonceKey, AES_256_GCM};
use aws_lc_rs::signature::{
    EcdsaKeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED,
    ECDSA_P256_SHA256_FIXED_SIGNING,
};
use aws_lc_rs::{digest, hkdf, hmac, rand};
use zeroize::Zeroizing;

use crate::{CryptoError, CryptoProvider, Sealed, SigAlg, HKDF_MAX, NONCE_LEN};

/// ECDSA P-256 fixed-length signature (r || s).
pub const P256_SIG_LEN: usize = 64;

/// The AWS-LC FIPS module provider. Construct with [`FipsProvider::new`],
/// which refuses to hand out a provider unless the module reports FIPS mode
/// (power-on self tests passed).
#[derive(Debug)]
pub struct FipsProvider {
    _private: (),
}

/// A device signing key held by the module (ECDSA P-256).
pub struct FipsSigningKey {
    pair: EcdsaKeyPair,
}

impl std::fmt::Debug for FipsSigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FipsSigningKey(P-256, <secret>)")
    }
}

impl FipsSigningKey {
    /// PKCS#8 v1 DER of the private key, for the OS keystore. Wiped on drop.
    pub fn to_pkcs8(&self) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let doc = self.pair.to_pkcs8v1().map_err(|_| CryptoError::Module)?;
        Ok(Zeroizing::new(doc.as_ref().to_vec()))
    }

    /// Load a key previously exported with [`FipsSigningKey::to_pkcs8`].
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, CryptoError> {
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der)
            .map(|pair| FipsSigningKey { pair })
            .map_err(|_| CryptoError::Invalid)
    }

    /// Wire public key (65 bytes, uncompressed SEC1 point).
    pub fn public_key(&self) -> Vec<u8> {
        self.pair.public_key().as_ref().to_vec()
    }
}

struct OutLen(usize);

impl hkdf::KeyType for OutLen {
    fn len(&self) -> usize {
        self.0
    }
}

impl FipsProvider {
    /// Fails with [`CryptoError::Module`] if AWS-LC is not the FIPS module
    /// or is not in FIPS mode.
    pub fn new() -> Result<Self, CryptoError> {
        aws_lc_rs::try_fips_mode().map_err(|_| CryptoError::Module)?;
        Ok(FipsProvider { _private: () })
    }
}

impl CryptoProvider for FipsProvider {
    type SigningKey = FipsSigningKey;

    fn name(&self) -> &'static str {
        "aws-lc-rs 1.18.1 / AWS-LC FIPS 4.2.0 (aws-lc-fips-sys 0.14.2)"
    }

    fn fips_mode(&self) -> bool {
        aws_lc_rs::try_fips_mode().is_ok()
    }

    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        let d = digest::digest(&digest::SHA256, data);
        let mut out = [0u8; 32];
        out.copy_from_slice(d.as_ref());
        out
    }

    fn hmac_sha256(&self, key: &[u8], msg: &[u8]) -> [u8; 32] {
        let k = hmac::Key::new(hmac::HMAC_SHA256, key);
        let tag = hmac::sign(&k, msg);
        let mut out = [0u8; 32];
        out.copy_from_slice(tag.as_ref());
        out
    }

    fn hkdf_sha256(
        &self,
        ikm: &[u8],
        salt: &[u8],
        info: &[u8],
        out: &mut [u8],
    ) -> Result<(), CryptoError> {
        if out.len() > HKDF_MAX {
            return Err(CryptoError::Invalid);
        }
        if out.is_empty() {
            return Ok(());
        }
        let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(ikm);
        let info_parts = [info];
        let okm = prk
            .expand(&info_parts, OutLen(out.len()))
            .map_err(|_| CryptoError::Invalid)?;
        okm.fill(out).map_err(|_| CryptoError::Module)
    }

    fn random(&self, out: &mut [u8]) -> Result<(), CryptoError> {
        rand::fill(out).map_err(|_| CryptoError::Module)
    }

    fn aead_seal(&self, key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Sealed, CryptoError> {
        let k = RandomizedNonceKey::new(&AES_256_GCM, key).map_err(|_| CryptoError::Module)?;
        let mut buf = plaintext.to_vec();
        let nonce = k
            .seal_in_place_append_tag(Aad::from(aad), &mut buf)
            .map_err(|_| CryptoError::Module)?;
        let n: [u8; NONCE_LEN] = *nonce.as_ref();
        Ok(Sealed {
            nonce: n,
            ct_and_tag: buf,
        })
    }

    fn aead_open(
        &self,
        key: &[u8; 32],
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        ct_and_tag: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let k = RandomizedNonceKey::new(&AES_256_GCM, key).map_err(|_| CryptoError::Module)?;
        let mut buf = ct_and_tag.to_vec();
        let pt_len = k
            .open_in_place(Nonce::assume_unique_for_key(*nonce), Aad::from(aad), &mut buf)
            .map_err(|_| CryptoError::Verify)?
            .len();
        buf.truncate(pt_len);
        Ok(buf)
    }

    fn generate_signing_key(&self, alg: SigAlg) -> Result<(FipsSigningKey, Vec<u8>), CryptoError> {
        match alg {
            SigAlg::EcdsaP256Sha256 => {
                let pair = EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING)
                    .map_err(|_| CryptoError::Module)?;
                let key = FipsSigningKey { pair };
                let pk = key.public_key();
                Ok((key, pk))
            }
            SigAlg::Ed25519 => Err(CryptoError::Unsupported),
        }
    }

    fn sign(&self, key: &FipsSigningKey, msg: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let rng = rand::SystemRandom::new();
        key.pair
            .sign(&rng, msg)
            .map(|s| s.as_ref().to_vec())
            .map_err(|_| CryptoError::Module)
    }

    fn verify(&self, alg: SigAlg, public_key: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        match alg {
            SigAlg::EcdsaP256Sha256 => {
                if public_key.len() != alg.public_key_len() || sig.len() != P256_SIG_LEN {
                    return Err(CryptoError::Verify);
                }
                UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, public_key)
                    .verify(msg, sig)
                    .map_err(|_| CryptoError::Verify)
            }
            SigAlg::Ed25519 => Err(CryptoError::Unsupported),
        }
    }
}
