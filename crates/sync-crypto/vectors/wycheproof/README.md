# Wycheproof vectors (imported, unmodified)

Source: https://github.com/C2SP/wycheproof, commit
`12fd3aaf33eb5fa1f52e026912ee00c054f9d984` (2026-10-06, the newest commit
≥ 6 weeks before 2026-11-18), directory `testvectors_v1/`. Licence:
Apache-2.0 (Wycheproof project). Files are byte-identical to upstream:

- `aes_gcm_test.json` — sha256 `985e5ecc172e181eaf49e89508b9470dcf478002eb7e8559c707eb42dc97dfe7`
- `ecdsa_secp256r1_sha256_p1363_test.json` — sha256 `c60de693930e386c3a5472d08081623ef8504decc54b38ac01ec6b2a2575c986`
- `hkdf_sha256_test.json` — sha256 `bb2b462a38b251cb52a2aede706d6d4b62b26864f4e80c95497507ddb07c5f1e`
- `hmac_sha256_test.json` — sha256 `2d201cfa61d1bf95e6f5d07d96634b4a348b31e8eaa277ad7c8d09677b7a743f`

`tests/wycheproof.rs` re-checks these hashes before running, then runs
every case the suite-v1 API can express: AES-GCM with a 256-bit key,
96-bit IV and 128-bit tag (other shapes are not constructible through
`CryptoProvider`, counted as skipped); every P-256/SHA-256 P1363
signature; every HMAC-SHA256 case (truncated tags compared as prefix);
every HKDF-SHA256 case on both providers.
