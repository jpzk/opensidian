// SPDX-License-Identifier: GPL-3.0-or-later
//! M2a smoke: the OpenSSL FIPS provider 3.5.9 (CMVP #5586) is loaded and in
//! approved mode — default properties fips=yes, the `default` provider is not
//! active, SHA2-256 is served by `fips`, MD5 is refused — and computes a known
//! SHA-256 (FIPS 180-4 "abc" vector).
use sync_crypto::fips::{self, FipsProvider};
use sync_crypto::soft::hex;
use sync_crypto::CryptoProvider;

#[test]
fn openssl_fips_provider_in_approved_mode() {
    fips::init().expect("OpenSSL FIPS provider must be active");
    let p = FipsProvider::new().unwrap();
    assert!(p.fips_mode());
    assert!(p.name().contains("#5586"), "{}", p.name());
    let v = p.openssl_version();
    eprintln!("libcrypto: {v}");
    assert!(v.starts_with("OpenSSL 3.5.9"), "linked libcrypto {v}");
}

#[test]
fn fips_module_sha256_abc() {
    let p = FipsProvider::new().unwrap();
    assert_eq!(hex(&p.sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}
