// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! sync-crypto — the cipher suite and the `CryptoProvider` trait
//! (docs/sync/spec.md, key-tree.md). Callers depend on the trait only, so the
//! FIPS backend can be swapped without touching sync-core or sync-server.
//!
//! What is here:
//! - [`fips::FipsProvider`] (M2): the release provider, AWS-LC FIPS module via
//!   aws-lc-rs (docs/sync/cmvp.md §7) — AES-256-GCM with module nonces,
//!   HKDF/HMAC/SHA-256, module DRBG, ECDSA P-256 (suite 0x02). KATs:
//!   `vectors/*.vec`, `tests/fips_kat.rs`.
//! - [`CryptoProvider`]: hash, HMAC, HKDF, randomness, AEAD (module-chosen
//!   nonces), signatures — the whole surface the sync engine may use.
//! - [`content_sha256`]: plain SHA-256 for content addressing (history store,
//!   batch `content_sha256`). Not a secret-dependent operation, so it uses
//!   `sha2` directly and needs no provider.
//! - spec helpers over any provider: per-object key derivation and the key
//!   commitment tag (spec §3.3), constant-time compare.
//! - [`words`] (M2c): the 12 backup words S (EFF large list, checksum word,
//!   dice input), docs/sync/key-tree.md §1. Tests: `tests/words.rs`.
//! - [`soft::SoftProvider`]: a reference provider for hash/HMAC/HKDF built on
//!   `sha2`, used for test vectors. It does NOT implement AEAD, randomness or
//!   signatures (returns [`CryptoError::Unsupported`]); those come from
//!   [`fips::FipsProvider`].
#![forbid(unsafe_code)]

use std::fmt;

use sha2::{Digest, Sha256};

pub mod fips;
pub mod soft;
pub mod words;

/// AES-256-GCM nonce length.
pub const NONCE_LEN: usize = 12;
/// AES-256-GCM tag length.
pub const TAG_LEN: usize = 16;
/// Max HKDF-SHA256 output (RFC 5869: 255 × HashLen).
pub const HKDF_MAX: usize = 255 * 32;

/// Crypto failure. Deliberately coarse: callers must not branch on why an
/// authentication check failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    /// Authentication / verification failed (bad tag, bad signature, bad
    /// commitment).
    Verify,
    /// Invalid input length or parameter.
    Invalid,
    /// The provider does not implement this operation.
    Unsupported,
    /// The underlying module failed (RNG, self-test, …).
    Module,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CryptoError::Verify => "verification failed",
            CryptoError::Invalid => "invalid crypto parameter",
            CryptoError::Unsupported => "operation not supported by this provider",
            CryptoError::Module => "crypto module failure",
        })
    }
}

impl std::error::Error for CryptoError {}

/// Signature algorithm (matches `key_alg` / `sig_alg` bytes on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SigAlg {
    Ed25519 = 1,
    EcdsaP256Sha256 = 2,
}

impl SigAlg {
    pub fn from_byte(b: u8) -> Result<Self, CryptoError> {
        match b {
            1 => Ok(SigAlg::Ed25519),
            2 => Ok(SigAlg::EcdsaP256Sha256),
            _ => Err(CryptoError::Unsupported),
        }
    }

    /// Public key length on the wire (32 Ed25519 / 65 P-256 uncompressed).
    pub fn public_key_len(self) -> usize {
        match self {
            SigAlg::Ed25519 => 32,
            SigAlg::EcdsaP256Sha256 => 65,
        }
    }
}

/// AEAD output: module-generated nonce + ciphertext with the tag appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub nonce: [u8; NONCE_LEN],
    pub ct_and_tag: Vec<u8>,
}

/// Every cryptographic operation the sync engine may perform. One
/// implementation per backend; the release backend is a FIPS 140-3 module
/// (docs/sync/cmvp.md). Nonces are never caller-chosen.
pub trait CryptoProvider {
    /// Private signing key handle (may be non-exportable).
    type SigningKey;

    /// Human-readable backend name, logged in diagnostics.
    fn name(&self) -> &'static str;

    /// True only when running inside an approved FIPS module in FIPS mode.
    fn fips_mode(&self) -> bool;

    fn sha256(&self, data: &[u8]) -> [u8; 32];

    fn hmac_sha256(&self, key: &[u8], msg: &[u8]) -> [u8; 32];

    /// HKDF-SHA256 (RFC 5869) extract+expand into `out`
    /// (`out.len() <= HKDF_MAX`).
    fn hkdf_sha256(
        &self,
        ikm: &[u8],
        salt: &[u8],
        info: &[u8],
        out: &mut [u8],
    ) -> Result<(), CryptoError>;

    /// Fill `out` from the module DRBG.
    fn random(&self, out: &mut [u8]) -> Result<(), CryptoError>;

    /// AES-256-GCM seal with a module-generated random nonce.
    fn aead_seal(&self, key: &[u8; 32], aad: &[u8], plaintext: &[u8])
        -> Result<Sealed, CryptoError>;

    /// AES-256-GCM open. Any failure is [`CryptoError::Verify`].
    fn aead_open(
        &self,
        key: &[u8; 32],
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        ct_and_tag: &[u8],
    ) -> Result<Vec<u8>, CryptoError>;

    /// Generate a signing key; returns the handle and the wire public key.
    fn generate_signing_key(&self, alg: SigAlg)
        -> Result<(Self::SigningKey, Vec<u8>), CryptoError>;

    fn sign(&self, key: &Self::SigningKey, msg: &[u8]) -> Result<Vec<u8>, CryptoError>;

    fn verify(
        &self,
        alg: SigAlg,
        public_key: &[u8],
        msg: &[u8],
        sig: &[u8],
    ) -> Result<(), CryptoError>;
}

/// SHA-256 of content bytes (content addressing). Provider-independent.
pub fn content_sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(data));
    out
}

/// Constant-time equality for tags and commitments (length is public).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    std::hint::black_box(diff) == 0
}

/// Per-object keys (spec §3.3).
pub struct ObjectKeys {
    /// AES-256-GCM key.
    pub enc: [u8; 32],
    /// Key-commitment MAC key.
    pub commit: [u8; 32],
}

/// `okm = HKDF-SHA256(ikm = k_root, salt = obj_id, info = "osy obj v1" ||
/// suite, L = 64)`; `K_enc = okm[0..32]`, `K_c = okm[32..64]`.
pub fn derive_object_keys<P: CryptoProvider + ?Sized>(
    p: &P,
    k_root: &[u8; 32],
    obj_id: &[u8; 16],
    suite: u8,
) -> Result<ObjectKeys, CryptoError> {
    let mut info = b"osy obj v1".to_vec();
    info.push(suite);
    let mut okm = [0u8; 64];
    p.hkdf_sha256(k_root, obj_id, &info, &mut okm)?;
    let mut keys = ObjectKeys {
        enc: [0; 32],
        commit: [0; 32],
    };
    keys.enc.copy_from_slice(&okm[..32]);
    keys.commit.copy_from_slice(&okm[32..]);
    Ok(keys)
}

/// `commit = HMAC-SHA256(K_c, "osy commit v1" || vault_id || obj_id ||
/// suite || fmt)` (spec §3.3).
pub fn key_commitment<P: CryptoProvider + ?Sized>(
    p: &P,
    k_c: &[u8; 32],
    vault_id: &[u8; 16],
    obj_id: &[u8; 16],
    suite: u8,
    fmt: u8,
) -> [u8; 32] {
    let mut msg = Vec::with_capacity(13 + 16 + 16 + 2);
    msg.extend_from_slice(b"osy commit v1");
    msg.extend_from_slice(vault_id);
    msg.extend_from_slice(obj_id);
    msg.push(suite);
    msg.push(fmt);
    p.hmac_sha256(k_c, &msg)
}

/// Check a header's commitment tag in constant time BEFORE decrypting.
pub fn check_commitment<P: CryptoProvider + ?Sized>(
    p: &P,
    k_c: &[u8; 32],
    vault_id: &[u8; 16],
    obj_id: &[u8; 16],
    suite: u8,
    fmt: u8,
    tag: &[u8; 32],
) -> Result<(), CryptoError> {
    let want = key_commitment(p, k_c, vault_id, obj_id, suite, fmt);
    if ct_eq(&want, tag) {
        Ok(())
    } else {
        Err(CryptoError::Verify)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soft::SoftProvider;

    #[test]
    fn content_hash_abc() {
        assert_eq!(
            soft::hex(&content_sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn ct_eq_cases() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn object_keys_differ_per_object_and_suite() {
        let p = SoftProvider;
        let root = [7u8; 32];
        let a = derive_object_keys(&p, &root, &[1; 16], 1).unwrap();
        let b = derive_object_keys(&p, &root, &[2; 16], 1).unwrap();
        let c = derive_object_keys(&p, &root, &[1; 16], 2).unwrap();
        assert_ne!(a.enc, a.commit);
        assert_ne!(a.enc, b.enc);
        assert_ne!(a.enc, c.enc);
        // deterministic
        assert_eq!(a.enc, derive_object_keys(&p, &root, &[1; 16], 1).unwrap().enc);
    }

    #[test]
    fn commitment_binds_every_input() {
        let p = SoftProvider;
        let k = [5u8; 32];
        let t = key_commitment(&p, &k, &[1; 16], &[2; 16], 1, 1);
        check_commitment(&p, &k, &[1; 16], &[2; 16], 1, 1, &t).unwrap();
        let wrong = [
            check_commitment(&p, &[6; 32], &[1; 16], &[2; 16], 1, 1, &t),
            check_commitment(&p, &k, &[0; 16], &[2; 16], 1, 1, &t),
            check_commitment(&p, &k, &[1; 16], &[3; 16], 1, 1, &t),
            check_commitment(&p, &k, &[1; 16], &[2; 16], 2, 1, &t),
            check_commitment(&p, &k, &[1; 16], &[2; 16], 1, 2, &t),
        ];
        for r in wrong {
            assert_eq!(r, Err(CryptoError::Verify));
        }
    }

    #[test]
    fn sig_alg_bytes() {
        assert_eq!(SigAlg::from_byte(1).unwrap().public_key_len(), 32);
        assert_eq!(SigAlg::from_byte(2).unwrap().public_key_len(), 65);
        assert!(SigAlg::from_byte(0).is_err());
    }
}
