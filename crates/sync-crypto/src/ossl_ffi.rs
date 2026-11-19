// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! The only `unsafe` in sync-crypto: thin RAII wrappers over the OpenSSL 3
//! EVP calls that rust-openssl 0.10.81 has no safe API for (docs/sync/cmvp.md
//! §5): approved-mode enforcement (`EVP_default_properties_enable_fips`,
//! `OSSL_PROVIDER_available`, provider of a fetched algorithm), HMAC through
//! `EVP_MAC` fetched with `fips=yes`, and reading the module-generated GCM IV
//! plus the provider's `iv-generated` flag. Symbols come from the libcrypto
//! that openssl-sys links (OpenSSL 3.5.9 built from the pinned tarball).
#![allow(unsafe_code)]

use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::ptr;

use foreign_types::ForeignTypeRef;
use openssl::cipher_ctx::CipherCtxRef;
use openssl::md::MdRef;
use openssl_sys as ffi;

// Public OpenSSL 3.0 API (include/openssl/{provider,evp,params}.h) that
// openssl-sys 0.9.117 does not declare.
extern "C" {
    fn OSSL_PROVIDER_available(libctx: *mut ffi::OSSL_LIB_CTX, name: *const c_char) -> c_int;
    fn OSSL_PROVIDER_get0_name(prov: *const ffi::OSSL_PROVIDER) -> *const c_char;
    fn EVP_MD_get0_provider(md: *const ffi::EVP_MD) -> *const ffi::OSSL_PROVIDER;
    fn EVP_CIPHER_CTX_get_params(ctx: *mut ffi::EVP_CIPHER_CTX, params: *mut ffi::OSSL_PARAM) -> c_int;
    fn OSSL_PARAM_construct_utf8_string(key: *const c_char, buf: *mut c_char, bsize: usize) -> ffi::OSSL_PARAM;
    fn OSSL_PROVIDER_get_params(prov: *const ffi::OSSL_PROVIDER, params: *mut ffi::OSSL_PARAM) -> c_int;
    fn OSSL_PARAM_construct_utf8_ptr(key: *const c_char, buf: *mut *mut c_char, bsize: usize) -> ffi::OSSL_PARAM;
    fn OSSL_PARAM_construct_int(key: *const c_char, buf: *mut c_int) -> ffi::OSSL_PARAM;
}

/// Approved-mode checks of the loaded FIPS provider that [`provider_status`]
/// reads (fipsmodule.cnf from `fipsinstall -pedantic`, cmvp.md §5). Each must
/// be reported AND be 1, else sync refuses to start.
pub const REQUIRED_CHECKS: [&CStr; 4] = [c"security-checks", c"hmac-key-check", c"hkdf-key-check", c"signature-digest-check"];

/// What `OSSL_PROVIDER_get_params` reports for a loaded provider: its own
/// `version` (the module version, independent of the libcrypto core),
/// `status` (1 = running, self-tests passed) and each of [`REQUIRED_CHECKS`]
/// (`None` = not reported by the provider).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStatus {
    pub version: Option<String>,
    pub status: Option<i32>,
    pub checks: Vec<(&'static str, Option<i32>)>,
}

/// `OSSL_PROVIDER_get_params(prov, {version, status, REQUIRED_CHECKS..})`.
pub fn provider_status(prov: &openssl::provider::ProviderRef) -> Option<ProviderStatus> {
    unsafe {
        let mut version: *mut c_char = ptr::null_mut();
        let mut status: c_int = -1;
        let mut vals: [c_int; REQUIRED_CHECKS.len()] = [-1; REQUIRED_CHECKS.len()];
        let mut params = Vec::with_capacity(REQUIRED_CHECKS.len() + 3);
        params.push(OSSL_PARAM_construct_utf8_ptr(c"version".as_ptr(), &mut version, 0));
        params.push(OSSL_PARAM_construct_int(c"status".as_ptr(), &mut status));
        for (k, v) in REQUIRED_CHECKS.iter().zip(vals.iter_mut()) {
            params.push(OSSL_PARAM_construct_int(k.as_ptr(), v));
        }
        params.push(ffi::OSSL_PARAM_construct_end());
        if OSSL_PROVIDER_get_params(prov.as_ptr(), params.as_mut_ptr()) != 1 {
            return None;
        }
        let got = |i: usize| ffi::OSSL_PARAM_modified(&params[i]) == 1;
        let version = (got(0) && !version.is_null()).then(|| CStr::from_ptr(version).to_string_lossy().into_owned());
        let status = got(1).then_some(status);
        let checks = REQUIRED_CHECKS
            .iter()
            .enumerate()
            .map(|(i, k)| (k.to_str().unwrap_or("?"), got(i + 2).then_some(vals[i])))
            .collect();
        Some(ProviderStatus { version, status, checks })
    }
}

/// `EVP_default_properties_enable_fips(NULL, 1)`: every implicit fetch in the
/// default library context now carries `fips=yes`.
pub fn enable_fips_default_properties() -> bool {
    unsafe { ffi::EVP_default_properties_enable_fips(ptr::null_mut(), 1) == 1 }
}

/// `EVP_default_properties_is_fips_enabled(NULL) == 1`.
pub fn fips_default_properties() -> bool {
    unsafe { ffi::EVP_default_properties_is_fips_enabled(ptr::null_mut()) == 1 }
}

/// `OSSL_PROVIDER_available(NULL, name)`: loaded AND activated.
pub fn provider_available(name: &CStr) -> bool {
    unsafe { OSSL_PROVIDER_available(ptr::null_mut(), name.as_ptr()) == 1 }
}

/// Name of the provider that implements a fetched digest (`"fips"` expected).
pub fn md_provider_name(md: &MdRef) -> Option<String> {
    unsafe {
        let prov = EVP_MD_get0_provider(md.as_ptr());
        if prov.is_null() {
            return None;
        }
        let name = OSSL_PROVIDER_get0_name(prov);
        if name.is_null() {
            return None;
        }
        Some(CStr::from_ptr(name).to_string_lossy().into_owned())
    }
}

struct Mac(*mut ffi::EVP_MAC);
impl Drop for Mac {
    fn drop(&mut self) {
        unsafe { ffi::EVP_MAC_free(self.0) }
    }
}
struct MacCtx(*mut ffi::EVP_MAC_CTX);
impl Drop for MacCtx {
    fn drop(&mut self) {
        unsafe { ffi::EVP_MAC_CTX_free(self.0) }
    }
}

/// HMAC-SHA256 through `EVP_MAC_fetch(NULL, "HMAC", "fips=yes")` with
/// `digest=SHA2-256`. `None` on any module error — including the approved-mode
/// key check (`hmac-key-check`, keys < 112 bits are refused).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> Option<[u8; 32]> {
    unsafe {
        let mac = Mac(ffi::EVP_MAC_fetch(ptr::null_mut(), c"HMAC".as_ptr(), c"fips=yes".as_ptr()));
        if mac.0.is_null() {
            return None;
        }
        let ctx = MacCtx(ffi::EVP_MAC_CTX_new(mac.0));
        if ctx.0.is_null() {
            return None;
        }
        let params = [
            OSSL_PARAM_construct_utf8_string(c"digest".as_ptr(), c"SHA2-256".as_ptr() as *mut c_char, 0),
            ffi::OSSL_PARAM_construct_end(),
        ];
        if ffi::EVP_MAC_init(ctx.0, key.as_ptr(), key.len(), params.as_ptr()) != 1 {
            return None;
        }
        if ffi::EVP_MAC_update(ctx.0, msg.as_ptr(), msg.len()) != 1 {
            return None;
        }
        let mut out = [0u8; 32];
        let mut outl: usize = 0;
        if ffi::EVP_MAC_final(ctx.0, out.as_mut_ptr(), &mut outl, out.len()) != 1 || outl != 32 {
            return None;
        }
        Some(out)
    }
}

/// After an AES-GCM encryption that was initialised WITHOUT an IV: the IV the
/// provider generated from its DRBG (SP 800-38D §8.2.2, SP §2.7a) — `None`
/// unless the provider reports `iv-generated = 1`.
pub fn gcm_generated_iv(ctx: &CipherCtxRef) -> Option<[u8; 12]> {
    unsafe {
        let mut iv = [0u8; 12];
        let mut generated: c_uint = 0;
        let mut params = [
            ffi::OSSL_PARAM_construct_octet_string(c"iv".as_ptr(), iv.as_mut_ptr() as *mut c_void, iv.len()),
            ffi::OSSL_PARAM_construct_uint(c"iv-generated".as_ptr(), &mut generated),
            ffi::OSSL_PARAM_construct_end(),
        ];
        if EVP_CIPHER_CTX_get_params(ctx.as_ptr(), params.as_mut_ptr()) != 1 {
            return None;
        }
        if ffi::OSSL_PARAM_modified(&params[0]) != 1 || params[0].return_size != 12 {
            return None;
        }
        if ffi::OSSL_PARAM_modified(&params[1]) != 1 || generated != 1 {
            return None;
        }
        Some(iv)
    }
}
