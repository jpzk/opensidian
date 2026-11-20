// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Key tree §2 (docs/sync/key-tree.md): root R and its children from the
//! backup words, plus the P-256 scalar derivation for phrase-derived signing
//! keys (K_rsig, K_destroy).
//!
//! Open questions 10 + 12 (re-checked for OpenSSL 3, cmvp.md): suite v1
//! imports the 32-byte K_destroy / K_rsig outputs as raw Ed25519 private
//! keys into the OpenSSL FIPS provider (`EVP_PKEY_new_raw_private_key_ex`,
//! `fips=yes`, provider + our sign/verify pairwise check); the P-256 fallback
//! imports a raw scalar via PKCS#8 (`fips::FipsSigningKey::from_p256_scalar`,
//! pairwise-checked). So K_destroy / K_rsig stay derived from the phrase:
//! a fresh device must be able to prove destroy and recovery from the words
//! alone. The scalar is chosen with the arithmetic of FIPS 186-5 A.2.2
//! (rejection sampling: accept `c <= n - 2`, `d = c + 1`); the candidate
//! bits come from HKDF (SP 800-56C) instead of an RBG — that difference is
//! recorded as an "externally derived SSP" in cmvp.md, not hidden.

use zeroize::Zeroizing;

use crate::{CryptoError, CryptoProvider};

/// P-256 group order n (SP 800-186 §3.2.1.3), big-endian.
pub const P256_N: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
];

/// `n - 2`, the largest accepted candidate (FIPS 186-5 A.2.2 step 6).
const P256_N_MINUS_2: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x4f,
];

/// Label of the P-256 candidate expansion; the counter byte is appended.
pub const P256_INFO: &[u8] = b"osy p256 v1";

/// Retry cap. A candidate is rejected with p ~= 2^-32, so 256 rejections in a
/// row (p ~= 2^-8192) means a broken provider, reported as `Module`.
const MAX_TRIES: u16 = 256;

/// Constant-time `a <= b` for big-endian 32-byte integers (no early exit:
/// the candidate is secret).
pub fn ct_le(a: &[u8; 32], b: &[u8; 32]) -> bool {
    // compute b - a over all bytes; a <= b iff no final borrow
    let mut borrow: i16 = 0;
    for i in (0..32).rev() {
        let d = b[i] as i16 - a[i] as i16 - borrow;
        borrow = (d >> 15) & 1; // 1 iff d < 0
    }
    borrow == 0
}

/// FIPS 186-5 A.2.2 steps 6-7 on one candidate: `Some(c + 1)` iff
/// `c <= n - 2`, i.e. the result is in `[1, n - 1]`.
pub fn p256_scalar_from_candidate(c: &[u8; 32]) -> Option<Zeroizing<[u8; 32]>> {
    if !ct_le(c, &P256_N_MINUS_2) {
        return None;
    }
    let mut d = Zeroizing::new(*c);
    let mut carry: u16 = 1;
    for i in (0..32).rev() {
        let s = d[i] as u16 + carry;
        d[i] = s as u8;
        carry = s >> 8;
    }
    Some(d)
}

/// Counter-retry selection: ask `candidate(ctr, out)` for ctr = 0, 1, ...
/// until one is accepted. Split out so the retry path is testable.
pub fn p256_scalar_select<F>(mut candidate: F) -> Result<Zeroizing<[u8; 32]>, CryptoError>
where
    F: FnMut(u8, &mut [u8; 32]) -> Result<(), CryptoError>,
{
    let mut c = Zeroizing::new([0u8; 32]);
    for ctr in 0..MAX_TRIES {
        candidate(ctr as u8, &mut c)?;
        if let Some(d) = p256_scalar_from_candidate(&c) {
            return Ok(d);
        }
    }
    Err(CryptoError::Module)
}

/// P-256 private scalar for a 32-byte signing seed:
/// `c_ctr = HKDF(seed, salt=00*32, info="osy p256 v1" || u8 ctr, 32)`,
/// first `c_ctr <= n - 2` wins, `d = c_ctr + 1`.
pub fn p256_scalar_from_seed<P: CryptoProvider + ?Sized>(
    p: &P,
    seed: &[u8; 32],
) -> Result<Zeroizing<[u8; 32]>, CryptoError> {
    let mut info = [0u8; 12];
    info[..11].copy_from_slice(P256_INFO);
    p256_scalar_select(|ctr, out| {
        info[11] = ctr;
        p.hkdf_sha256(seed, &[0u8; 32], &info, out)
    })
}

/// Root R and its children (key-tree.md §2). Secrets are wiped on drop.
pub struct RootKeys {
    pub k_wrap: Zeroizing<[u8; 32]>,
    pub loc: [u8; 16],
    pub k_rsig: Zeroizing<[u8; 32]>,
    pub k_destroy: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for RootKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RootKeys(<secret>)")
    }
}

/// Derive R from `S_bytes`, then K_wrap / loc / K_rsig / K_destroy.
pub fn derive_root<P: CryptoProvider + ?Sized>(
    p: &P,
    s_bytes: &[u8; crate::words::S_BYTES_LEN],
) -> Result<RootKeys, CryptoError> {
    let mut r = Zeroizing::new([0u8; 32]);
    p.hkdf_sha256(s_bytes, b"osy root v1", b"root", &mut *r)?;
    let child = |info: &[u8], out: &mut [u8]| p.hkdf_sha256(&*r, &[0u8; 32], info, out);
    let mut k = RootKeys {
        k_wrap: Zeroizing::new([0u8; 32]),
        loc: [0u8; 16],
        k_rsig: Zeroizing::new([0u8; 32]),
        k_destroy: Zeroizing::new([0u8; 32]),
    };
    child(b"osy wrap v1", &mut *k.k_wrap)?;
    child(b"osy loc v1", &mut k.loc)?;
    child(b"osy rsig v1", &mut *k.k_rsig)?;
    child(b"osy destroy v1", &mut *k.k_destroy)?;
    Ok(k)
}
