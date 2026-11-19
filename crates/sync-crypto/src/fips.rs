// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Release provider: every operation goes to the AWS-LC FIPS module through
//! aws-lc-rs (`fips` feature; pin + CMVP status in docs/sync/cmvp.md §7).
//!
//! - AEAD: AES-256-GCM via `RandomizedNonceKey` — the 96-bit nonce comes from
//!   the module DRBG inside the seal call, a caller can never choose it.
//! - HKDF-SHA256 / HMAC-SHA256 / SHA-256 / DRBG: module services.
//! - Signatures: Ed25519 (suite 0x01, the v1 suite: certificate #5314 lists
//!   EDDSA KeyGen/SigGen/SigVer, cmvp.md §2 + §7; 32-byte public key, 64-byte
//!   signature, RFC 8032 pure Ed25519). ECDSA P-256 / SHA-256 (suite 0x02,
//!   fixed 64-byte r||s, 65-byte uncompressed point) stays available for
//!   verification and as the documented fallback suite.

use aws_lc_rs::aead::{Aad, Nonce, RandomizedNonceKey, AES_256_GCM};
use aws_lc_rs::signature::{
    EcdsaKeyPair, Ed25519KeyPair, KeyPair, UnparsedPublicKey, ECDSA_P256_SHA256_FIXED,
    ECDSA_P256_SHA256_FIXED_SIGNING, ED25519,
};
use aws_lc_rs::{digest, hkdf, hmac, rand};
use zeroize::Zeroizing;

use crate::{CryptoError, CryptoProvider, Sealed, SigAlg, HKDF_MAX, NONCE_LEN};

/// ECDSA P-256 fixed-length signature (r || s).
pub const P256_SIG_LEN: usize = 64;
/// Ed25519 signature (R || S, RFC 8032 §5.1.6).
pub const ED25519_SIG_LEN: usize = 64;

/// The AWS-LC FIPS module provider. Construct with [`FipsProvider::new`],
/// which refuses to hand out a provider unless the module reports FIPS mode
/// (power-on self tests passed).
#[derive(Debug)]
pub struct FipsProvider {
    _private: (),
}

enum Pair {
    Ed25519(Ed25519KeyPair),
    P256(EcdsaKeyPair),
}

/// A signing key held by the module (Ed25519 for suite v1, or P-256).
pub struct FipsSigningKey {
    pair: Pair,
}

impl std::fmt::Debug for FipsSigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.pair {
            Pair::Ed25519(_) => "FipsSigningKey(Ed25519, <secret>)",
            Pair::P256(_) => "FipsSigningKey(P-256, <secret>)",
        })
    }
}

impl FipsSigningKey {
    /// Algorithm of this key.
    pub fn alg(&self) -> SigAlg {
        match self.pair {
            Pair::Ed25519(_) => SigAlg::Ed25519,
            Pair::P256(_) => SigAlg::EcdsaP256Sha256,
        }
    }

    /// PKCS#8 DER of the private key, for the OS keystore (Ed25519: v2 with
    /// the public key, re-checked on load; P-256: v1). Wiped on drop.
    pub fn to_pkcs8(&self) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        let doc = match &self.pair {
            Pair::Ed25519(k) => k.to_pkcs8(),
            Pair::P256(k) => k.to_pkcs8v1(),
        }
        .map_err(|_| CryptoError::Module)?;
        Ok(Zeroizing::new(doc.as_ref().to_vec()))
    }

    /// Load a key previously exported with [`FipsSigningKey::to_pkcs8`]
    /// (Ed25519 or P-256; the algorithm OID in the document decides).
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, CryptoError> {
        if let Ok(k) = Ed25519KeyPair::from_pkcs8(der) {
            return Ok(FipsSigningKey { pair: Pair::Ed25519(k) });
        }
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, der)
            .map(|k| FipsSigningKey { pair: Pair::P256(k) })
            .map_err(|_| CryptoError::Invalid)
    }

    /// Import a raw P-256 private scalar `d` (big-endian, `1 <= d < n`) into
    /// the module: RFC 5915 ECPrivateKey without the public key, parsed by
    /// AWS-LC (which computes Q = d·G) and validated in FIPS builds by
    /// `EC_KEY_check_fips` (pairwise-consistency test). Suite 0x02 only.
    pub fn from_p256_scalar(d: &[u8; 32]) -> Result<Self, CryptoError> {
        let zero = [0u8; 32];
        // reject d == 0 and d >= n up front (constant time); the module checks too
        if crate::keytree::ct_le(d, &zero) || crate::keytree::ct_le(&crate::keytree::P256_N, d) {
            return Err(CryptoError::Invalid);
        }
        let mut der = Zeroizing::new(Vec::with_capacity(51));
        der.extend_from_slice(&[0x30, 0x31, 0x02, 0x01, 0x01, 0x04, 0x20]);
        der.extend_from_slice(d);
        // [0] parameters: OID 1.2.840.10045.3.1.7 (prime256v1)
        der.extend_from_slice(&[0xa0, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07]);
        EcdsaKeyPair::from_private_key_der(&ECDSA_P256_SHA256_FIXED_SIGNING, &der)
            .map(|k| FipsSigningKey { pair: Pair::P256(k) })
            .map_err(|_| CryptoError::Invalid)
    }

    /// Import a 32-byte Ed25519 private key (RFC 8032 §5.1.5 seed) into the
    /// module. The module derives the public key; a sign + verify round trip
    /// through the module is then run as a pairwise-consistency check before
    /// the key is handed out.
    pub fn from_ed25519_seed(seed: &[u8; 32]) -> Result<Self, CryptoError> {
        let k = Ed25519KeyPair::from_seed_unchecked(seed).map_err(|_| CryptoError::Invalid)?;
        let msg = b"osy ed25519 pct v1";
        let sig = k.try_sign(msg).map_err(|_| CryptoError::Module)?;
        UnparsedPublicKey::new(&ED25519, k.public_key().as_ref())
            .verify(msg, sig.as_ref())
            .map_err(|_| CryptoError::Module)?;
        Ok(FipsSigningKey { pair: Pair::Ed25519(k) })
    }

    /// Phrase-derived signing key of suite v1 (K_destroy, K_rsig): the
    /// 32-byte key-tree output (module HKDF, key-tree.md §2) is the Ed25519
    /// private key, imported with [`FipsSigningKey::from_ed25519_seed`].
    /// Same seed -> same key on every device.
    pub fn from_seed(_p: &FipsProvider, seed: &[u8; 32]) -> Result<Self, CryptoError> {
        Self::from_ed25519_seed(seed)
    }

    /// Phrase-derived signing key of the P-256 fallback suite 0x02: scalar
    /// from [`crate::keytree::p256_scalar_from_seed`] (module HKDF),
    /// imported with [`FipsSigningKey::from_p256_scalar`].
    pub fn from_seed_p256(p: &FipsProvider, seed: &[u8; 32]) -> Result<Self, CryptoError> {
        let d = crate::keytree::p256_scalar_from_seed(p, seed)?;
        Self::from_p256_scalar(&d)
    }

    /// Wire public key (Ed25519: 32 bytes; P-256: 65-byte uncompressed point).
    pub fn public_key(&self) -> Vec<u8> {
        match &self.pair {
            Pair::Ed25519(k) => k.public_key().as_ref().to_vec(),
            Pair::P256(k) => k.public_key().as_ref().to_vec(),
        }
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
        "aws-lc-rs 1.16.0 / AWS-LC FIPS 3.1.0 (aws-lc-fips-sys 0.13.11, CMVP #5314)"
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
        let pair = match alg {
            SigAlg::Ed25519 => Pair::Ed25519(Ed25519KeyPair::generate().map_err(|_| CryptoError::Module)?),
            SigAlg::EcdsaP256Sha256 => Pair::P256(
                EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).map_err(|_| CryptoError::Module)?,
            ),
        };
        let key = FipsSigningKey { pair };
        let pk = key.public_key();
        Ok((key, pk))
    }

    fn sign(&self, key: &FipsSigningKey, msg: &[u8]) -> Result<Vec<u8>, CryptoError> {
        match &key.pair {
            Pair::Ed25519(k) => k.try_sign(msg).map(|s| s.as_ref().to_vec()).map_err(|_| CryptoError::Module),
            Pair::P256(k) => {
                let rng = rand::SystemRandom::new();
                k.sign(&rng, msg).map(|s| s.as_ref().to_vec()).map_err(|_| CryptoError::Module)
            }
        }
    }

    fn verify(&self, alg: SigAlg, public_key: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        let (v, sig_len): (&'static dyn aws_lc_rs::signature::VerificationAlgorithm, usize) = match alg {
            SigAlg::Ed25519 => (&ED25519, ED25519_SIG_LEN),
            SigAlg::EcdsaP256Sha256 => (&ECDSA_P256_SHA256_FIXED, P256_SIG_LEN),
        };
        if public_key.len() != alg.public_key_len() || sig.len() != sig_len {
            return Err(CryptoError::Verify);
        }
        UnparsedPublicKey::new(v, public_key)
            .verify(msg, sig)
            .map_err(|_| CryptoError::Verify)
    }
}
