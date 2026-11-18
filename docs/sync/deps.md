# Opensidian Sync — dependency record

Rules (AGENTS.md + sync goal): Rust only; every crate pinned exact
(`=x.y.z`); released ≥ 6 weeks before it is added; no git deps; prefer
crates already in `src-tauri/Cargo.lock`; record publish date (crates.io)
and the `checksum` from Cargo.lock. Rust crates are resolved through the
committed `src-tauri/Cargo.lock` and built with `--locked` (repo convention;
`ui/vendor/` is the vendoring path for frontend code only).

## New crates added by the sync work

**None.** M0 + M1 + the crate skeleton use `std` plus crates that were
already in `src-tauri/Cargo.lock` before the branch (base d999420).
If this list ever gets an entry, it must carry: crate, exact version,
crates.io publish date, Cargo.lock sha256, reason, and why std/existing
crates were not enough.

## Existing crates reused by the new sync crates

Already in Cargo.lock at d999420; the new crates pin them exactly to the
locked version so nothing in the lock moves.

- `sha2 =0.10.9` — published 2025-04-30 — checksum
  `a7507d819769d01a365ab707794a4084392c824f54a7a6a7862f8c3d0892b283`
  — content addressing (SHA-256) in the history store; already pinned
  `=0.10.9` in src-tauri.
- `serde =1.0.229` (feature `derive`) — published 2026-07-18 — checksum
  `4148590afebada386688f18773da617792bf2ef03ffc1e4cbd2b1d45b023e0ba`
  — wire types / history index. (src-tauri itself declares `"1"`; the new
  crates pin `=1.0.229`.)
- `serde_json =1.0.151` — published 2026-07-20 — checksum
  `c841b55ecdae098c80dcae9cf767f6f8a0c2cdb3416bbef72181df4d0fe73f14`
  — history index file, debug encodings only (the wire format is the
  binary canonical encoding of spec.md, not JSON).

Each new crate only uses the subset it needs; `sync-server` may depend
only on `sync-proto` + `sync-crypto` (checked with `cargo tree -p sync-server`,
log in the goal work dir).

## Declared, NOT added (needs operator approval)

- `aws-lc-rs` with feature `fips` (pulls `aws-lc-fips-sys`, needs CMake +
  Go + cc; glibc only) — candidate pins and why are in cmvp.md §3/§5.
  Publish dates checked 2026-11-18: 1.17.4 = 2026-09-01 (fips-sys 0.13.17,
  AWS-LC 3.6.0); 1.18.1 = 2026-09-01 (fips-sys 0.14.2, AWS-LC 4.2.0, not yet
  validated); 1.18.2 = 2026-10-12 and 1.19.0 = 2026-10-28 are younger than
  6 weeks at the time of writing.
- `rustls` with feature `fips` — 0.23.44 (2026-09-07) / 0.23.45
  (2026-09-14) satisfy the age rule; 0.24.0-dev.* are pre-releases.
- `zstd`, `axum`/`hyper`, `rusqlite` (plan §1/§6) — M3+; not evaluated yet.

Checksums for the declared-not-added crates are recorded only when they are
actually added (from the Cargo.lock that adds them).
