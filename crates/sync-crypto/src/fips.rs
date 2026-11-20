// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Release provider: the OpenSSL 3 FIPS provider 3.5.9 (CMVP #5586, FIPS
//! 140-3, docs/sync/cmvp.md) through rust-openssl. Operator decision
//! 2026-11-19: OpenSSL 3 (Apache-2.0, GPLv3-compatible) replaces AWS-LC.
//!
//! Approved mode is enforced once per process in [`FipsProvider::new`]:
//! load `fips` (its own `version` parameter must be 3.5.9, `status` 1 and the
//! -pedantic checks security-checks / hmac-key-check / hkdf-key-check /
//! signature-digest-check reported as 1) + `base` (an explicit load disables
//! the `default` fallback),
//! set the default property query to `fips=yes`, then refuse unless
//! - `EVP_default_properties_is_fips_enabled` is 1,
//! - the `fips` provider is available and `default` is NOT,
//! - SHA2-256 fetched with the default query is implemented by `fips`, and
//! - MD5 (not approved) cannot be fetched.
//!
//! Every primitive is an EVP fetch with `fips=yes` (explicit query where
//! rust-openssl takes one, otherwise the enforced default query):
//! - AEAD: AES-256-GCM, initialised WITHOUT an IV so the provider generates
//!   the 96-bit IV from its DRBG (SP 800-38D §8.2.2; Security Policy §2.7a:
//!   an imported IV for encryption is non-conformant). The provider's
//!   `iv-generated` flag is checked on every seal; a caller never chooses it.
//! - HKDF-SHA256 (`EVP_KDF` "HKDF"), HMAC-SHA256 (`EVP_MAC` "HMAC"), SHA-256
//!   (`EVP_MD` "SHA2-256"), DRBG (`RAND_bytes`, the fips provider's DRBG).
//!   In approved mode keys < 112 bits are refused by the module (SP §2.6).
//! - Signatures: Ed25519 (suite 0x01, v1; #5586 lists EDDSA KeyGen/SigGen/
//!   SigVer, pure Ed25519, RFC 8032) and ECDSA P-256 / SHA-256 (suite 0x02,
//!   fixed 64-byte r||s, 65-byte uncompressed point) as fallback.

use std::ffi::CStr;
use std::sync::OnceLock;

use openssl::bn::BigNum;
use openssl::cipher::{Cipher, CipherRef};
use openssl::cipher_ctx::CipherCtx;
use openssl::ecdsa::EcdsaSig;
use openssl::hash::MessageDigest;
use openssl::kdf::{hkdf, HkdfMode};
use openssl::md::{Md, MdRef};
use openssl::md_ctx::MdCtx;
use openssl::pkey::{Id, KeyType, PKey, Private};
use openssl::provider::Provider;
use openssl::sign::{Signer, Verifier};
use zeroize::Zeroizing;

use crate::ossl_ffi;
use crate::{CryptoError, CryptoProvider, Sealed, SigAlg, HKDF_MAX, NONCE_LEN, TAG_LEN};

/// ECDSA P-256 fixed-length signature (r || s).
pub const P256_SIG_LEN: usize = 64;
/// Ed25519 signature (R || S, RFC 8032 §5.1.6).
pub const ED25519_SIG_LEN: usize = 64;

/// Property query used for every explicit fetch.
const FIPS_PROPS: &str = "fips=yes";
const FIPS_PROPS_C: &CStr = c"fips=yes";

/// Version parameter of the validated module (CMVP #5586, cmvp.md).
pub const FIPS_MODULE_VERSION: &str = "3.5.9";

/// SubjectPublicKeyInfo prefix of an uncompressed P-256 point
/// (id-ecPublicKey, prime256v1, BIT STRING of 66 bytes).
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// Why approved mode could not be established (logged by callers that
/// refuse to start; never contains key material).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FipsInitError(pub String);

impl std::fmt::Display for FipsInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OpenSSL FIPS provider not active: {}", self.0)
    }
}

impl std::error::Error for FipsInitError {}

struct Module {
    _fips: Provider,
    _base: Provider,
    sha256: Md,
    aes_gcm: Cipher,
    version: String,
}

static MODULE: OnceLock<Result<Module, FipsInitError>> = OnceLock::new();

fn init_module() -> Result<Module, FipsInitError> {
    let e = |s: &str| FipsInitError(s.to_string());
    openssl::init();
    let fips = Provider::load(None, "fips").map_err(|x| FipsInitError(format!("load fips: {x}")))?;
    // the module itself must be the 3.5.9 provider that CMVP #5586 covers
    // (its own version parameter, independent of the libcrypto core), its
    // self-tests must have passed (status 1) and the -pedantic approved-mode
    // checks must be on (a fipsmodule.cnf with security-checks = 0 still
    // loads and serves fips=yes, but without the SP §2.6 key/digest checks)
    let st = ossl_ffi::provider_status(&fips).ok_or_else(|| e("OSSL_PROVIDER_get_params(fips) failed"))?;
    if st.version.as_deref() != Some(FIPS_MODULE_VERSION) {
        return Err(FipsInitError(format!(
            "fips provider version {:?} is not the CMVP #5586 module {FIPS_MODULE_VERSION}",
            st.version
        )));
    }
    if st.status != Some(1) {
        return Err(FipsInitError(format!("fips provider status {:?} (self-tests not passed)", st.status)));
    }
    for (k, v) in &st.checks {
        if *v != Some(1) {
            return Err(FipsInitError(format!("fips provider {k} = {v:?}, approved mode needs 1 (fipsinstall -pedantic)")));
        }
    }
    let base = Provider::load(None, "base").map_err(|x| FipsInitError(format!("load base: {x}")))?;
    if !ossl_ffi::enable_fips_default_properties() {
        return Err(e("EVP_default_properties_enable_fips failed"));
    }
    if !ossl_ffi::fips_default_properties() {
        return Err(e("default properties are not fips=yes"));
    }
    if !ossl_ffi::provider_available(c"fips") {
        return Err(e("fips provider not available"));
    }
    if ossl_ffi::provider_available(c"default") {
        return Err(e("the non-approved default provider is active"));
    }
    let sha256 = Md::fetch(None, "SHA2-256", Some(FIPS_PROPS)).map_err(|x| FipsInitError(format!("fetch SHA2-256: {x}")))?;
    let implicit = Md::fetch(None, "SHA2-256", None).map_err(|x| FipsInitError(format!("fetch SHA2-256: {x}")))?;
    if ossl_ffi::md_provider_name(&implicit).as_deref() != Some("fips") {
        return Err(e("SHA2-256 is not served by the fips provider"));
    }
    if Md::fetch(None, "MD5", None).is_ok() {
        return Err(e("MD5 is fetchable: approved-mode default query not in force"));
    }
    let aes_gcm = Cipher::fetch(None, "AES-256-GCM", Some(FIPS_PROPS)).map_err(|x| FipsInitError(format!("fetch AES-256-GCM: {x}")))?;
    Ok(Module {
        _fips: fips,
        _base: base,
        sha256,
        aes_gcm,
        version: openssl::version::version().to_string(),
    })
}

fn module() -> Result<&'static Module, CryptoError> {
    MODULE.get_or_init(init_module).as_ref().map_err(|_| CryptoError::Module)
}

/// Establish approved mode (idempotent, process-wide) and report why it
/// failed. `syncd`, `sync-server` and the app's sync window call this and
/// refuse to start on `Err`.
pub fn init() -> Result<(), FipsInitError> {
    MODULE.get_or_init(init_module).as_ref().map(|_| ()).map_err(Clone::clone)
}

/// The OpenSSL FIPS provider. Construct with [`FipsProvider::new`], which
/// refuses unless approved mode is established (see the module docs).
#[derive(Debug)]
pub struct FipsProvider {
    _private: (),
}

enum Pair {
    Ed25519(PKey<Private>),
    P256(PKey<Private>),
}

/// A signing key held by the provider (Ed25519 for suite v1, or P-256).
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

fn pct(key: FipsSigningKey) -> Result<FipsSigningKey, CryptoError> {
    // pairwise-consistency check through the provider before a key is handed out
    let p = FipsProvider::new()?;
    let msg = b"osy pct v1";
    let sig = p.sign(&key, msg).map_err(|_| CryptoError::Module)?;
    p.verify(key.alg(), &key.public_key(), msg, &sig).map_err(|_| CryptoError::Module)?;
    Ok(key)
}

impl FipsSigningKey {
    /// Algorithm of this key.
    pub fn alg(&self) -> SigAlg {
        match self.pair {
            Pair::Ed25519(_) => SigAlg::Ed25519,
            Pair::P256(_) => SigAlg::EcdsaP256Sha256,
        }
    }

    fn pkey(&self) -> &PKey<Private> {
        match &self.pair {
            Pair::Ed25519(k) | Pair::P256(k) => k,
        }
    }

    /// PKCS#8 DER of the private key, for the OS keystore. Wiped on drop.
    pub fn to_pkcs8(&self) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
        self.pkey()
            .private_key_to_pkcs8()
            .map(Zeroizing::new)
            .map_err(|_| CryptoError::Module)
    }

    /// Load a key previously exported with [`FipsSigningKey::to_pkcs8`]
    /// (Ed25519 or P-256; the algorithm OID decides). Pairwise-checked.
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, CryptoError> {
        module()?;
        let k = PKey::private_key_from_pkcs8(der).map_err(|_| CryptoError::Invalid)?;
        let pair = match k.id() {
            Id::ED25519 => Pair::Ed25519(k),
            Id::EC if k.bits() == 256 && p256_public(&k).is_ok() => Pair::P256(k),
            _ => return Err(CryptoError::Invalid),
        };
        pct(FipsSigningKey { pair })
    }

    /// Import a raw P-256 private scalar `d` (big-endian, `1 <= d < n`):
    /// PKCS#8 PrivateKeyInfo around an RFC 5915 ECPrivateKey without the
    /// public key; OpenSSL computes Q = d·G during decoding and the key is
    /// imported into the fips provider, then pairwise-checked. Suite 0x02 only
    /// (open-questions 10/12, cmvp.md §6).
    pub fn from_p256_scalar(d: &[u8; 32]) -> Result<Self, CryptoError> {
        module()?;
        let zero = [0u8; 32];
        // reject d == 0 and d >= n up front (constant time); the module checks too
        if crate::keytree::ct_le(d, &zero) || crate::keytree::ct_le(&crate::keytree::P256_N, d) {
            return Err(CryptoError::Invalid);
        }
        let mut der = Zeroizing::new(Vec::with_capacity(67));
        der.extend_from_slice(&[0x30, 0x41, 0x02, 0x01, 0x00]);
        // AlgorithmIdentifier: id-ecPublicKey, prime256v1
        der.extend_from_slice(&[
            0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86,
            0x48, 0xce, 0x3d, 0x03, 0x01, 0x07,
        ]);
        // OCTET STRING { ECPrivateKey { version 1, privateKey d } }
        der.extend_from_slice(&[0x04, 0x27, 0x30, 0x25, 0x02, 0x01, 0x01, 0x04, 0x20]);
        der.extend_from_slice(d);
        let k = PKey::private_key_from_pkcs8(&der).map_err(|_| CryptoError::Invalid)?;
        if k.id() != Id::EC || k.bits() != 256 {
            return Err(CryptoError::Invalid);
        }
        pct(FipsSigningKey { pair: Pair::P256(k) })
    }

    /// Import a 32-byte Ed25519 private key (RFC 8032 §5.1.5) into the fips
    /// provider (`EVP_PKEY_new_raw_private_key_ex(.., "ED25519", "fips=yes")`).
    /// The provider derives the public key; a sign + verify round trip is run
    /// as a pairwise-consistency check before the key is handed out.
    pub fn from_ed25519_seed(seed: &[u8; 32]) -> Result<Self, CryptoError> {
        module()?;
        let k = PKey::private_key_from_raw_bytes_ex(None, KeyType::ED25519, Some(FIPS_PROPS_C), seed)
            .map_err(|_| CryptoError::Invalid)?;
        pct(FipsSigningKey { pair: Pair::Ed25519(k) })
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
            Pair::Ed25519(k) => k.raw_public_key().unwrap_or_default(),
            Pair::P256(k) => p256_public(k).unwrap_or_default(),
        }
    }
}

/// 65-byte uncompressed point of a P-256 key (from its SubjectPublicKeyInfo).
fn p256_public<T: openssl::pkey::HasPublic>(k: &openssl::pkey::PKeyRef<T>) -> Result<Vec<u8>, CryptoError> {
    let spki = k.public_key_to_der().map_err(|_| CryptoError::Module)?;
    match spki.strip_prefix(&P256_SPKI_PREFIX[..]) {
        Some(pt) if pt.len() == 65 && pt[0] == 0x04 => Ok(pt.to_vec()),
        _ => Err(CryptoError::Invalid),
    }
}

impl FipsProvider {
    /// Fails with [`CryptoError::Module`] unless approved mode is
    /// established ([`init`] gives the reason).
    pub fn new() -> Result<Self, CryptoError> {
        module()?;
        Ok(FipsProvider { _private: () })
    }

    fn m(&self) -> &'static Module {
        // new() succeeded, so the OnceLock holds Ok
        module().expect("fips module initialised")
    }

    /// OpenSSL core version string (e.g. "OpenSSL 3.5.9 30 Sep 2026").
    pub fn openssl_version(&self) -> &'static str {
        &self.m().version
    }

    fn sha256_md(&self) -> &'static MdRef {
        &self.m().sha256
    }

    fn gcm(&self) -> &'static CipherRef {
        &self.m().aes_gcm
    }

    /// HMAC-SHA256 that reports module refusals (e.g. a key < 112 bits in
    /// approved mode) instead of panicking.
    pub fn try_hmac_sha256(&self, key: &[u8], msg: &[u8]) -> Result<[u8; 32], CryptoError> {
        ossl_ffi::hmac_sha256(key, msg).ok_or(CryptoError::Module)
    }

    fn try_sha256(&self, data: &[u8]) -> Result<[u8; 32], CryptoError> {
        let mut ctx = MdCtx::new().map_err(|_| CryptoError::Module)?;
        ctx.digest_init(self.sha256_md()).map_err(|_| CryptoError::Module)?;
        ctx.digest_update(data).map_err(|_| CryptoError::Module)?;
        let mut out = [0u8; 32];
        let n = ctx.digest_final(&mut out).map_err(|_| CryptoError::Module)?;
        if n != 32 {
            return Err(CryptoError::Module);
        }
        Ok(out)
    }
}

fn der_to_fixed(der: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let s = EcdsaSig::from_der(der).map_err(|_| CryptoError::Module)?;
    let mut out = s.r().to_vec_padded(32).map_err(|_| CryptoError::Module)?;
    out.extend_from_slice(&s.s().to_vec_padded(32).map_err(|_| CryptoError::Module)?);
    Ok(out)
}

fn fixed_to_der(sig: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let r = BigNum::from_slice(&sig[..32]).map_err(|_| CryptoError::Verify)?;
    let s = BigNum::from_slice(&sig[32..]).map_err(|_| CryptoError::Verify)?;
    EcdsaSig::from_private_components(r, s)
        .and_then(|s| s.to_der())
        .map_err(|_| CryptoError::Verify)
}

impl CryptoProvider for FipsProvider {
    type SigningKey = FipsSigningKey;

    fn name(&self) -> &'static str {
        "OpenSSL 3.5.9 FIPS provider 3.5.9 (CMVP #5586, FIPS 140-3)"
    }

    fn fips_mode(&self) -> bool {
        module().is_ok() && ossl_ffi::fips_default_properties() && ossl_ffi::provider_available(c"fips")
    }

    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        // a failure here means the module entered its error state
        self.try_sha256(data).expect("FIPS provider SHA2-256 failed")
    }

    fn hmac_sha256(&self, key: &[u8], msg: &[u8]) -> [u8; 32] {
        // every engine key is 32 bytes; a refusal is a programming error or
        // the module error state
        self.try_hmac_sha256(key, msg).expect("FIPS provider HMAC-SHA256 refused")
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
        hkdf(
            self.sha256_md(),
            ikm,
            Some(salt),
            Some(info),
            HkdfMode::ExtractAndExpand,
            None,
            out,
        )
        .map_err(|_| CryptoError::Invalid)
    }

    fn random(&self, out: &mut [u8]) -> Result<(), CryptoError> {
        self.m();
        openssl::rand::rand_priv_bytes(out).map_err(|_| CryptoError::Module)
    }

    fn aead_seal(&self, key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Sealed, CryptoError> {
        let mut ctx = CipherCtx::new().map_err(|_| CryptoError::Module)?;
        // no IV: the provider generates it (SP 800-38D §8.2.2)
        ctx.encrypt_init(Some(self.gcm()), Some(key), None).map_err(|_| CryptoError::Module)?;
        if !aad.is_empty() {
            ctx.cipher_update(aad, None).map_err(|_| CryptoError::Module)?;
        }
        let mut out = vec![0u8; plaintext.len() + TAG_LEN];
        let mut n = 0;
        if !plaintext.is_empty() {
            n = ctx
                .cipher_update(plaintext, Some(&mut out[..plaintext.len()]))
                .map_err(|_| CryptoError::Module)?;
        }
        n += ctx.cipher_final(&mut out[n..plaintext.len()]).map_err(|_| CryptoError::Module)?;
        if n != plaintext.len() {
            return Err(CryptoError::Module);
        }
        ctx.tag(&mut out[n..]).map_err(|_| CryptoError::Module)?;
        let nonce = ossl_ffi::gcm_generated_iv(&ctx).ok_or(CryptoError::Module)?;
        Ok(Sealed {
            nonce,
            ct_and_tag: out,
        })
    }

    fn aead_open(
        &self,
        key: &[u8; 32],
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        ct_and_tag: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if ct_and_tag.len() < TAG_LEN {
            return Err(CryptoError::Verify);
        }
        let (ct, tag) = ct_and_tag.split_at(ct_and_tag.len() - TAG_LEN);
        let mut ctx = CipherCtx::new().map_err(|_| CryptoError::Verify)?;
        ctx.decrypt_init(Some(self.gcm()), Some(key), Some(nonce)).map_err(|_| CryptoError::Verify)?;
        if !aad.is_empty() {
            ctx.cipher_update(aad, None).map_err(|_| CryptoError::Verify)?;
        }
        let mut out = vec![0u8; ct.len()];
        let mut n = 0;
        if !ct.is_empty() {
            n = ctx.cipher_update(ct, Some(&mut out[..])).map_err(|_| CryptoError::Verify)?;
        }
        ctx.set_tag(tag).map_err(|_| CryptoError::Verify)?;
        // the tag check happens in final; on failure the plaintext is dropped
        n += ctx.cipher_final(&mut out[n..]).map_err(|_| CryptoError::Verify)?;
        out.truncate(n);
        Ok(out)
    }

    fn generate_signing_key(&self, alg: SigAlg) -> Result<(FipsSigningKey, Vec<u8>), CryptoError> {
        self.m();
        let pair = match alg {
            SigAlg::Ed25519 => Pair::Ed25519(PKey::generate_ed25519().map_err(|_| CryptoError::Module)?),
            SigAlg::EcdsaP256Sha256 => Pair::P256(PKey::ec_gen("P-256").map_err(|_| CryptoError::Module)?),
        };
        let key = FipsSigningKey { pair };
        let pk = key.public_key();
        if pk.len() != alg.public_key_len() {
            return Err(CryptoError::Module);
        }
        Ok((key, pk))
    }

    fn sign(&self, key: &FipsSigningKey, msg: &[u8]) -> Result<Vec<u8>, CryptoError> {
        self.m();
        match &key.pair {
            Pair::Ed25519(k) => {
                let mut s = Signer::new_without_digest(k).map_err(|_| CryptoError::Module)?;
                let sig = s.sign_oneshot_to_vec(msg).map_err(|_| CryptoError::Module)?;
                if sig.len() != ED25519_SIG_LEN {
                    return Err(CryptoError::Module);
                }
                Ok(sig)
            }
            Pair::P256(k) => {
                let mut s = Signer::new(MessageDigest::sha256(), k).map_err(|_| CryptoError::Module)?;
                let der = s.sign_oneshot_to_vec(msg).map_err(|_| CryptoError::Module)?;
                der_to_fixed(&der)
            }
        }
    }

    fn verify(&self, alg: SigAlg, public_key: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        self.m();
        let sig_len = match alg {
            SigAlg::Ed25519 => ED25519_SIG_LEN,
            SigAlg::EcdsaP256Sha256 => P256_SIG_LEN,
        };
        if public_key.len() != alg.public_key_len() || sig.len() != sig_len {
            return Err(CryptoError::Verify);
        }
        let ok = match alg {
            SigAlg::Ed25519 => {
                let pk = PKey::public_key_from_raw_bytes_ex(None, KeyType::ED25519, Some(FIPS_PROPS_C), public_key)
                    .map_err(|_| CryptoError::Verify)?;
                let mut v = Verifier::new_without_digest(&pk).map_err(|_| CryptoError::Verify)?;
                v.verify_oneshot(sig, msg)
            }
            SigAlg::EcdsaP256Sha256 => {
                if public_key[0] != 0x04 {
                    return Err(CryptoError::Verify);
                }
                let mut spki = P256_SPKI_PREFIX.to_vec();
                spki.extend_from_slice(public_key);
                let pk = PKey::public_key_from_der(&spki).map_err(|_| CryptoError::Verify)?;
                if pk.id() != Id::EC || pk.bits() != 256 {
                    return Err(CryptoError::Verify);
                }
                let der = fixed_to_der(sig)?;
                let mut v = Verifier::new(MessageDigest::sha256(), &pk).map_err(|_| CryptoError::Verify)?;
                v.verify_oneshot(&der, msg)
            }
        };
        match ok {
            Ok(true) => Ok(()),
            _ => Err(CryptoError::Verify),
        }
    }
}
