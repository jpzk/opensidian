# Opensidian Sync — dependency record

Rules (AGENTS.md + sync goal): Rust only; every crate pinned exact
(`=x.y.z`); released ≥ 6 weeks before it is added; no git deps; prefer
crates already in `src-tauri/Cargo.lock`; record publish date (crates.io)
and the `checksum` from Cargo.lock. Rust crates are resolved through the
committed `src-tauri/Cargo.lock` and built with `--locked` (repo convention;
`ui/vendor/` is the vendoring path for frontend code only).

## New crates added by the sync work

M0 + M1 + the crate skeleton added none. From M2 on, every crate that is
NOT in `src-tauri/Cargo.lock` at the branch base is listed here with exact
version, crates.io publish date (UTC, `created_at` from the crates.io API,
checked 2026-11-18; cutoff for the 6-week rule = **2026-10-07**), the
`checksum` from Cargo.lock (= crates.io sha256 of the .crate), licence, and
why it is there. "direct" = declared with `=x.y.z` in a sync crate;
"transitive" = pulled in by a direct one and fixed by Cargo.lock
(`--locked`); a transitive crate whose newest semver-compatible release was
too young was pinned back (`cargo update --precise` / direct pin), noted.

### M2a — AWS-LC FIPS module (sync-crypto)

Direct (sync-crypto/Cargo.toml):

- `aws-lc-rs =1.18.1` — 2026-09-01 — `b281d307588d634de920874890732659e2e7672f72b5e10e81badc1a8a83621e`
  — ISC AND (Apache-2.0 OR ISC) — the crypto suite (AES-256-GCM, HKDF,
  HMAC, SHA-256, ECDSA P-256, DRBG) from the AWS-LC FIPS module; features
  `fips`, `alloc`, default features off (no non-FIPS `aws-lc-sys`). CMVP
  status: cmvp.md §7. No std/existing crate offers a FIPS module.
- `aws-lc-fips-sys =0.14.2` — 2026-09-01 — `03367707e92796b190a4207d4d39b0a4271d574503d2969c2b0cfbf5c87658ee`
  — ISC AND (Apache-2.0 OR ISC) AND OpenSSL — the vendored AWS-LC 4.2.0 C
  module; pinned directly so the `^0.14.2` requirement of aws-lc-rs cannot
  float.
- `zeroize =1.9.1` — 2026-10-06 — `e13084392c5e4bc371903e2935a5eaeed24905a7511356b883835e18a78f6879`
  — Apache-2.0 OR MIT — required by aws-lc-rs (`^1.8.1`); the newest, 1.9.2,
  is from 2026-11-17 (< 6 weeks) so 1.9.1 is pinned directly. Also used to
  wipe key material.

Transitive (build-dependencies of aws-lc-fips-sys; build time only, none is
linked into a binary except via the generated code of aws-lc-fips-sys):

- `bindgen 0.72.1` — 2025-08-31 — `993776b509cfb49c750f11b8f07a46fa23e0a1386ffc01fb1e7d343efc387895` — BSD-3-Clause (non-optional build-dep of fips-sys; pregenerated bindings are used on x86_64-linux, libclang is not loaded)
- `cexpr 0.6.0` — 2021-10-11 — `6fac387a98bb7c37292057cffc56d62ecb629900026402633ae9160df93a8766` — Apache-2.0/MIT
- `clang-sys 1.9.1` — 2026-07-29 — `157a8ba7b480713b56f4c09fd13fc3e0a22a5dfab8097ba61cbc5feef950788a` — Apache-2.0
- `cmake 0.1.58` — 2026-03-26 — `c0f78a02292a74a88ac736019ab962ece0bc380e3f977bf72e376c5d78ff0678` — MIT OR Apache-2.0
- `either 1.19.0` — 2026-10-06 — `0e9c71c2167ca323c882b99918929403426e2373ea17242ff5653e0d5e1058be` — MIT OR Apache-2.0
- `fs_extra 1.3.0` — 2023-02-03 — `42703706b716c37f96a77aea830392ad231f44c9e9a67872fa5548707e11b11c` — MIT
- `itertools 0.13.0` — 2024-05-16 — `413ee7dfc52ee1a4949ceeb7dbc8a33f2d6c088194d9f922fb8318faf1f01186` — MIT OR Apache-2.0
- `libloading 0.8.9` — 2025-09-21 — `d7c4b02199fee7c5d21a5ae7d8cfa79a6ef5bb2fc834d6e9058e89c825efdc55` — ISC (a second version; 0.7.4 is already locked)
- `minimal-lexical 0.2.1` — 2021-10-04 — `68354c5c6bd36d73ff3feceb05efa59b6acb7626617f4962be322a825e61f79a` — MIT/Apache-2.0
- `nom 7.1.3` — 2023-01-15 — `d273983c5a657a70a3e8f2a01329822f3b8c8172b73826411a55751e404a0a4a` — MIT
- `prettyplease 0.2.37` — 2025-08-19 — `479ca8adacdd7ce8f1fb39ce9ecccbfe93a3f1344b3d0d97f20bc0196208f62b` — MIT OR Apache-2.0
- `shlex 1.3.0` — 2024-01-22 — `0fda2ff0d084019ba4d7c6f371c95d8fd75ce3524c3cb8fb653a3023f6323e64` — MIT OR Apache-2.0 (a second version; 2.0.1 is already locked)

Reused from the lock, versions unchanged: cc 1.4.4, dunce 1.0.5,
pkg-config 0.3.34, regex 1.13.1 (aws-lc-fips-sys); bitflags 2.13.1,
log 0.4.34, proc-macro2 1.0.107, quote 1.0.47, rustc-hash 2.1.3,
syn 2.0.119 (bindgen); glob 0.3.4, libc 0.2.189 (clang-sys); memchr (nom).
The lock delta is exactly the 15 crates above (verified by diffing the
lock against the base: only `+` package blocks, the two `-` lines are
`"shlex"` / `"libloading"` dependency names gaining a version suffix
because two versions now exist).

**Licence note (operator-pending, open-questions.md):** aws-lc-fips-sys
carries the OpenSSL/SSLeay licence for parts of AWS-LC; the FSF lists the
OpenSSL licence as GPL-incompatible and the app is GPL-3.0-or-later. deny.toml
gets per-crate exceptions (ISC for aws-lc-rs/aws-lc-fips-sys/libloading,
OpenSSL for aws-lc-fips-sys) so CI reports exactly these, but whether
distributing a GPL binary statically linking AWS-LC is acceptable is a
licensing decision for the operator, not something this branch decides.

## Toolchain (box only, FIPS build of aws-lc-fips-sys)

Installed in user space under `/var/tmp/sync-e2e/toolchain` on the test box
(never on the development VM, nothing system-wide), each from the upstream
release artefact, sha256-checked against the upstream checksum before
unpacking; versions are the newest stable release ≥ 6 weeks old on
2026-11-18. Activated by `fips-env.sh` (goal work dir) with
`GOTOOLCHAIN=local GOPROXY=off` so Go never fetches a toolchain or module.

- Go `1.27.1` (released 2026-09-01; 1.27.2 = 2026-10-08 is too young) —
  `go1.27.1.linux-amd64.tar.gz` sha256
  `63d339f0da5ab53635a56f2490a7984dfe12dfcff22ad749f63edaf590168445`
  (go.dev/dl JSON).
- CMake `4.4.4` (released 2026-10-02; 4.4.5 = 2026-10-27, 4.5.0 =
  2026-11-12 too young) — `cmake-4.4.4-linux-x86_64.tar.gz` sha256
  `e5bb807f7728cb60cd8b27ebc97a2edb469b68655f21e844a600c3575b76f5bb`
  (Kitware `cmake-4.4.4-SHA-256.txt`).
- Perl `5.44.0` (released 2026-07-15; newest stable, 5.45.x are dev
  releases) — `perl-5.44.0.tar.gz` sha256
  `3b855066b92491cb40e86affb1ca57d1a388aa43e51b91c7806a32c2f65f96c3`
  (cpan.org `.sha256.txt`), built from source with the box's gcc
  (`Configure -des`, no man pages). Needed because AWS-LC refuses a FIPS
  build without Perl.
- C compiler: the box harness gcc 16.0.1 (`/var/tmp/gate-boxd/env`, already
  used for every T0 build); no C++ compiler is needed.

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

Per crate (direct deps): `sync-proto` → serde (dev: serde_json, JSON tests of the `/v1` bodies); `sync-crypto` → sha2;
`sync-core` → sync-proto, sync-crypto, sha2, serde, serde_json; `syncd` →
sync-core; `sync-server` → sync-proto, sync-crypto. The Cargo.lock delta of
the skeleton is exactly the five path packages (`[[package]]` entries without
`source`/`checksum`); no registry package is added, removed or moved.

Each new crate only uses the subset it needs; `sync-server` may depend
only on `sync-proto` + `sync-crypto` (checked with `cargo tree -p sync-server`,
log in the goal work dir).

## Declared, NOT added yet

- `aws-lc-rs` — ADDED in M2a (above, pin decision cmvp.md §7).
- `rustls` with feature `fips` — 0.23.44 (2026-09-07) / 0.23.45
  (2026-09-14) satisfy the age rule; 0.24.0-dev.* are pre-releases.
- `zstd`, `axum`/`hyper`, `rusqlite` (plan §1/§6) — M3+; not evaluated yet.

Checksums for the declared-not-added crates are recorded only when they are
actually added (from the Cargo.lock that adds them).
