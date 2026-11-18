# Opensidian Sync — open questions

Each item: question, current default (what the code/spec does until
decided), who decides. "Operator" = product decision; "M2 review" =
settled when the crypto pin is chosen.

## From the plan (§9)

1. **Android FIPS** — deferred (plan §4 options A desktop-only /
   B platform crypto / C fleet). Default: Android out of scope until M7.
   Operator.
2. **CMVP cert** — **RESOLVED (M2d, cmvp.md §7–8):** the pinned module
   (aws-lc-rs 1.18.1 → AWS-LC 4.2.0) is IN PROCESS, no certificate, so no
   FIPS 140-3 claim may be made; no cert covers Ed25519 → suite v1 = ECDSA
   P-256. The #5314 SP-version / tested-environment questions do not apply
   to this pin. Follow-up before any release: re-check the AWS-LC 4
   certificate (SP lists 4.2.0?, environments, EdDSA, caveats).
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
   means glibc, not musl. Nothing added to the release set in this goal.
   Operator.

## Raised while writing the M0 spec

9. **`fmt` byte in the AAD** — spec.md §3.2 adds the container format byte
   to the plan's AAD list (vault_id | obj_id | idx | last | suite), giving a
   39-byte AAD, so a header-byte flip cannot reinterpret a segment under a
   different container format. Small deviation from the binding plan;
   needs a yes/no. Default: included. Operator / M8 review.
10. **Raw private-key import in FIPS mode** — **RESOLVED (M2d, cmvp.md §8):**
    the module accepts a raw P-256 scalar in FIPS mode (RFC 5915 import,
    AWS-LC computes Q, `EC_KEY_check_fips` pairwise test), proven on the box
    by `crates/sync-crypto/tests/p256_derive.rs`. Decision: K_destroy and
    K_rsig stay phrase-derived (destroy + recovery must work from the words
    alone on a fresh device); device signing keys and VK stay
    module-generated. Recorded as an "SSP entered into the module" (the
    caveat case), not hidden.
11. **History store encryption at rest** — M1 stores snapshots in
    plaintext under the app data dir (parity with the vault itself, which
    is plaintext). Option: wrap with a keystore-held key. Default:
    plaintext, documented in threat-model.md (T6). Operator.
12. **P-256 scalar derivation** — **RESOLVED (M2d, key-tree.md §2):**
    counter-retry HKDF kept (raw import is permitted, see 10), re-shaped to
    the FIPS 186-5 A.2.2 arithmetic (accept c ≤ n − 2, d = c + 1); the bit
    source is module HKDF instead of an RBG, noted in cmvp.md §8.

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
  FIPS backend replaces it wholesale. Should a release build refuse to
  start sync when `fips_mode()` is false, or only warn?
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

13. **AWS-LC licence vs GPL-3.0-or-later** — aws-lc-fips-sys is
    `ISC AND (Apache-2.0 OR ISC) AND OpenSSL`; parts of AWS-LC still carry the
    OpenSSL/SSLeay licence, which the FSF lists as GPL-incompatible, and the
    app/sync crates are GPL-3.0-or-later and link it statically. deny.toml has
    per-crate exceptions so CI reports exactly this (deps.md). **Operator /
    legal decision** (options: accept with a linking exception in the repo
    licence, license the sync crates differently, or a different FIPS module);
    default for this branch: keep the repo licence, flag in PR #238.
14. **EFF wordlist licence (CC BY 3.0 US) next to GPL-3.0 code** — the
    backup-word list is embedded unmodified as data with attribution
    (THIRD-PARTY.md). The FSF lists CC BY 4.0 (not 3.0) as GPLv3-compatible;
    for an unmodified data file shipped in the same binary we treat it as an
    aggregate. Default: keep as is, attribution on the licence screen.
    Operator / legal (same review as 13).
