# Opensidian Sync — open questions

Each item: question, current default (what the code/spec does until
decided), who decides. "Operator" = product decision; "M2 review" =
settled when the crypto pin is chosen.

## From the plan (§9)

1. **Android FIPS** — deferred (plan §4 options A desktop-only /
   B platform crypto / C fleet). Default: Android out of scope until M7.
   Operator.
2. **CMVP cert** — **RESOLVED (2026-11-19, cmvp.md):** the crypto module is
   the **OpenSSL FIPS Provider 3.5.9**, CMVP certificate **#5586** (FIPS
   140-3 L1, Active, sunset 2031-11-12), built on the box from the same
   OpenSSL 3.5.9 tarball (core = provider version). #5586 lists EDDSA
   ED-25519 as approved -> suite v1 stays Ed25519 (0x01, key_alg 1).
   Caveat kept: the box (Fedora 44) is not one of the tested OEs, so the
   test runs are not themselves on a validated configuration; a release
   claim needs a tested OE or an OE statement. History: AWS-LC 4.2.0 (in
   process) -> AWS-LC 3.1.0 / #5314 -> OpenSSL (licence, #13); text in git.
3. **Mobile data default** — suggestion: commits + files < 1 MB auto, rest
   on Wi-Fi/on demand. Operator.
4. **Self-hosting without a domain** (SPKI pinning) in v1? Default: spec
   allows a pinned SPKI in the envelope; no "ignore cert" ever. Operator.
5. **Destroy with proof** — immediate, or 24 h cancel window? Default in
   spec: 2-step request → confirm token; no delay window. Operator.
6. **Linux reveal** — extra app PIN on top of Secret Service? Operator.
7. **sync-server licence** (AGPL?). Default: crates inherit the repo's
   GPL-3.0-or-later until decided. Operator.
8. **Release artifacts** — server binary x86_64/aarch64 + container; FIPS
   means glibc, not musl. Nothing added to the release set in this goal;
   what each existing artifact must carry for the FIPS provider: cmvp.md §7.
   Operator.

## Raised while writing the M0 spec

9. **`fmt` byte in the AAD** — spec.md §3.2 adds the container format byte
   to the plan's AAD list (vault_id | obj_id | idx | last | suite), giving a
   39-byte AAD, so a header-byte flip cannot reinterpret a segment under a
   different container format. Small deviation from the binding plan;
   needs a yes/no. Default: included. Operator / M8 review.
10. **Raw private-key import in FIPS mode** — **RESOLVED again for OpenSSL
    (2026-11-19, cmvp.md §5):** the fips provider accepts a raw P-256
    scalar (PKCS#8 / RFC 5915 encoding without the public key; the
    provider's decoder computes Q = d·G) and a raw 32-byte Ed25519 seed
    (`EVP_PKEY_new_raw_private_key_ex(.., "fips=yes")`); the SP's PCT on
    key import runs, and sync-crypto adds its own sign/verify PCT
    (`fips::pct`). Proven on the box through the provider by
    `tests/p256_derive.rs` (d = 1 -> G, d = n−1 -> −G, d = 0 / ≥ n rejected)
    and the RFC 8032 seed KATs in `tests/fips_kat.rs` (goal log
    ossl-sync-crypto-test.log). Decision unchanged: K_destroy and K_rsig
    stay phrase-derived (destroy + recovery from the words alone); device
    keys and VK are DRBG-generated in the module. Phrase-derived keys are
    "externally loaded SSPs" — the #5586 caveat case ("no assurance of the
    minimum strength"), recorded, not hidden.
11. **History store encryption at rest** — M1 stores snapshots in
    plaintext under the app data dir (parity with the vault itself, which
    is plaintext). Option: wrap with a keystore-held key. Default:
    plaintext, documented in threat-model.md (T6). Operator.
12. **P-256 scalar derivation** — **RESOLVED (M2d, key-tree.md §2;
    re-checked for OpenSSL 2026-11-19):** counter-retry HKDF kept (raw
    import is permitted, see 10), FIPS 186-5 A.2.2 arithmetic (accept
    c ≤ n − 2, d = c + 1); the bit source is the provider's HKDF (EVP_KDF
    under fips=yes, IKM ≥ 112 bits as approved mode requires) instead of an
    RBG, noted in cmvp.md §5. Suite 0x02 only; v1 default is Ed25519.

## Raised while building M1 (file history)

(Appended as the history implementation lands.)

- **NFC of `str` fields.** spec §2 says paths are NFC, but `sync-proto`'s
  decoder cannot check NFC without a Unicode table crate (zero-new-deps
  rule). Today the decoder enforces UTF-8 / no NUL / ≤ 4096 bytes only;
  writers must normalise. Decide in M2: vendor a pinned
  `unicode-normalization` (≥ 6 weeks old) for both directions, or accept
  non-NFC paths and treat NFC-equivalent paths as a conflict.
- **`SoftProvider` scope.** `sync-crypto::soft` implements SHA-256 / HMAC /
  HKDF on top of `sha2` (RFC 4231 / 5869 vectors pass) so M1 and the test
  vectors need no new deps. It must never become the release provider: the
  FIPS backend replaces it wholesale. **Decided (2026-11-19):** refuse.
  `sync_crypto::fips::init()` must succeed before syncd, sync-server or the
  app's sync engine start; otherwise they exit / return error code `fips`
  ("refusing to start sync: OpenSSL FIPS provider not active: …"). Tests:
  sync-crypto `fips_refuse.rs`, syncd `cli.rs`
  `refuses_to_start_without_fips_provider`, sync-server `fips_refuse.rs`.
- **History per save.** The app journals every save (pre-write + own-save,
  deduplicated by content), so a long typing session keeps one version per
  autosave until thinning (24 h keep-all). Stock Sync and File Recovery
  snapshot at an interval instead. Options: keep the invariant and rely on
  thinning, or route own saves through the idle `Debouncer` and keep only
  the pre-write invariant for external/overwrite cases.
- **Attachments + config in history.** M1 wires notes only. Media writes
  (`pasteimg`, `media_rename`, canvas) need hooks of their own, plus a
  decision on the 14-day attachment rule vs. disk use.
- **Vault id minting.** History mints the treefold vault id on the first
  save, which writes `~/.opensidian.json`. If e2e tests assert that file
  is untouched after a save, they will see the new `vault_ids` entry.

## Raised while building M2 (crypto suite)

13. **AWS-LC licence vs GPL-3.0-or-later** — **RESOLVED (operator
    2026-11-19 10:16Z: "okay openssl it is"): OpenSSL 3 FIPS.** aws-lc-fips-sys
    was `ISC AND (Apache-2.0 OR ISC) AND OpenSSL`; the OpenSSL/SSLeay licence
    is GPL-incompatible. The module is now the OpenSSL 3.5.9 FIPS provider
    (OpenSSL 3 = Apache-2.0, GPLv3-compatible) via rust-openssl
    (openssl Apache-2.0, openssl-sys MIT) and rustls-openssl (MIT) for TLS.
    Cargo.lock has no aws-lc-* and no ring; no OpenSSL < 3.0 source (no
    openssl-src) is in the graph; deny.toml carries no OpenSSL-licence and
    no aws-lc ISC exception, `cargo deny check licenses` passes (CI run
    44515121547). TLS provider options considered (rule e): rustls-openssl
    0.4.2 (chosen: pinned, ≥ 6 weeks, EVP under fips=yes) vs a hand-written
    rustls CryptoProvider over EVP (more unaudited code). Residual: the
    provider is a loadable module that every artifact must ship and
    fipsinstall (cmvp.md §7); the release artifact set is unchanged.
14. **EFF wordlist licence (CC BY 3.0 US) next to GPL-3.0 code** — the
    backup-word list is embedded unmodified as data with attribution
    (THIRD-PARTY.md). The FSF lists CC BY 4.0 (not 3.0) as GPLv3-compatible;
    for an unmodified data file shipped in the same binary we treat it as an
    aggregate. Default: keep as is, attribution on the licence screen.
    Operator / legal (same review as 13).
