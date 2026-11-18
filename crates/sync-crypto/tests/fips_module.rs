// SPDX-License-Identifier: GPL-3.0-or-later
//! M2a smoke: the linked AWS-LC is the FIPS module, it is in FIPS mode, its power-on
//! self tests passed, and it computes a known SHA-256 (FIPS 180-4 "abc" vector).
use aws_lc_rs::digest;

#[test]
fn aws_lc_runs_in_fips_mode() {
    aws_lc_rs::try_fips_mode().expect("AWS-LC FIPS module not in FIPS mode");
}

#[test]
fn fips_module_sha256_abc() {
    let d = digest::digest(&digest::SHA256, b"abc");
    let hex: String = d.as_ref().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(hex, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}
