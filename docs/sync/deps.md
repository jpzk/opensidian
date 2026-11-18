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

### M2b — zstd body compression (sync-core)

Direct (sync-core/Cargo.toml), all three pinned exact with default features
off (no legacy-format decoder, no dictionary builder). The newer 0.14.1 /
8.1.0 / 2.1.1 (2026-10-09) and later are < 6 weeks old. zstd-sys compiles
the vendored libzstd 1.5.7 C sources with `cc` (already locked; no CMake,
no pkg-config probe without its feature). Licence BSD-3-Clause (already in
the deny.toml allow list).

- `zstd =0.14.0` — 2026-09-04 — `bf06bd8162af0734b344780deb55b42a2429ae430870d13fcc12f238e880fe6e`
  — BSD-3-Clause — inner body compression, spec §3.4 `comp = 0x01`
  (`sync_core::object`, kept only when smaller; decode bounded at 64 MiB).
- `zstd-safe =8.0.0` — 2026-09-04 — `ae42c0555055784c70058d19ba8e275528e8a99a706684868ace5da4e716a4ab`
  — BSD-3-Clause — pinned directly so zstd's `^8.0.0` cannot float.
- `zstd-sys =2.1.0+zstd.1.5.7` — 2026-09-04 — `0ef0a8027ec3ee71300ab3bcbcd0393f434aa72b91ca6d635a39941deae8eea0`
  — BSD-3-Clause — pinned directly (`^2.1.0` would float to 2.1.3, 2026-11-12).

Transitive: `jobserver 0.1.35` — 2026-07-06 — `1c00acbd29eabad4a2392fa0e921c874934dbbf4194312ad20f04a0ed67a3cb3`
— MIT OR Apache-2.0 — zstd-sys turns on the `parallel` feature of cc, which
pulls jobserver (build time only; newest release).

Reused from the lock: cc 1.4.4 (now with `parallel`), libc, getrandom 0.4.3,
pkg-config 0.3.34 (build-deps of zstd-sys).
Lock delta = exactly these 4 package blocks + the sync-core and cc dependency
lists (resolved by cargo on the box, sha256 of the lock identical to the one
committed);
cargo-vet exemptions added for the four.

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

### M3a — sync-server storage + HTTP (sync-server)

The HTTP stack reuses crates already locked: `hyper =1.11.1` (features
`server`, `http1`), `hyper-util =0.1.20` (`tokio`), `tokio =1.53.1`
(`rt-multi-thread`, `net`, `sync`, `time`; no `macros` -> no tokio-macros),
`http =1.5.0`, `http-body-util =0.1.5`, `bytes =1.12.1`, `base64 =0.22.1`,
plus serde/serde_json as above. No axum (routing is a 20-line match).

Direct (sync-server/Cargo.toml), default features off, feature `bundled`
(vendored SQLite amalgamation compiled with `cc`; no system libsqlite):

- `rusqlite =0.40.2` — 2026-08-08 — `23f2a97da3e3873c73cb2a2e71b35c40ff95e0b1eefa8d72d8499a6928c3b5b3`
  — MIT — metadata index (vaults, SHA-256 token hashes, object rows, list
  cursor). 0.41.0+ (2026-10-24) is < 6 weeks old. Statement cache off ->
  no hashlink.
- `libsqlite3-sys =0.38.2` — 2026-08-08 — `f1d20bef17f513b9b3004532233187769cd072d790971f4e4da0e346eb6401e8`
  — MIT (bundled SQLite: public domain) — pinned directly so rusqlite's
  `^0.38.2` cannot float to 0.39.x (2026-10-24, too young).

Transitive (fixed by Cargo.lock):

- `fallible-iterator 0.3.0` — 2023-05-22 — `2acce4a10f12dc2fb14a218589d4f1f62ef011b2d0cc4b3cb1bba8e94da14649`
  — MIT OR Apache-2.0 — rusqlite.
- `fallible-streaming-iterator 0.1.9` — 2018-05-02 — `7360491ce676a36bf9bb3c56c1aa791658183a54d2744120f27285738d90465a`
  — MIT OR Apache-2.0 — rusqlite.
- `httpdate 1.0.3` — 2023-08-13 — `df3b46402a9d5adb4c86a0cf463f42e19994e3ee891101b1841f30a545cb49a9`
  — MIT OR Apache-2.0 — hyper `server` feature (Date header).
- `vcpkg 0.2.15` — 2021-06-19 — `accd4ea62f7bb7a82fe23066fb0957d48ef677f6eeb8215f372f52e48bb32426`
  — MIT OR Apache-2.0 — build-dep of libsqlite3-sys (Windows library probe;
  unused on Linux with `bundled`).

Lock delta = exactly these 6 package blocks + the sync-server dependency
list (resolved by cargo on the box without `--locked`, lock sha256
`5c5b9df25b73edce9e009f94092776e6e63f6c636f07d68d85362e25e341719d` copied
back unchanged); cargo-vet exemptions added for all six; licences are in
the deny.toml allow list.

### M3a-TLS — rustls on the AWS-LC FIPS provider (sync-server)

TLS 1.3 only, provider `rustls::crypto::default_fips_provider()`;
`server_config` refuses any config where `ServerConfig::fips()` is false
(tests/tls.rs). Every crate is pinned direct in sync-server/Cargo.toml so the
resolver cannot float to a younger release (cutoff 2026-10-07).

- `rustls =0.23.45` — 2026-09-14 — `0d41d731c7d2f962d1ccc364cec258de3c0e93b38c2fb3ba97ac74513048d634`
  — Apache-2.0 OR ISC OR MIT — features `std`, `fips` (no tls12, no logging,
  no prefer-post-quantum).
- `tokio-rustls =0.26.6` — 2026-09-28 — `c9cc2678c2cdd569ef8215e2afd7954ada2ae20b4fdd2c5fe6139a3b02d105db`
  — MIT OR Apache-2.0 — feature `fips`.
- `rustls-webpki =0.103.15` — 2026-08-21 — `f3c3cf1d8b1e7d4927e2d154c3fcb02979afb9939629c62cd9048d4f07b60ac2`
  — ISC — features `std`, `aws-lc-rs-fips`.
- `rustls-pki-types =1.15.1` — 2026-07-23 — `2f4925028c7eb5d1fcdaf196971378ed9d2c1c4efc7dc5d011256f76c99c0a96`
  — MIT OR Apache-2.0 — PEM parsing (`pem::PemObject`).
- `subtle =2.6.1` — 2024-06-24 — `13c2bddecc57b384dee18652358fb23172facb8a2c51ccc10d74c157bdea3292`
  — BSD-3-Clause — rustls.
- `untrusted =0.9.0` — 2021-07-13 — `8ecb6da28b8a351d773b68d5825ac39017e680750f980f3a1a85cd8dd28a47c1`
  — ISC — rustls-webpki.
- `aws-lc-sys =0.45.0` — 2026-09-01 — `9bff6c3b54fad79a2e60b8102caf565819711497c1f5f092f49508e2f5c31b27`
  — ISC AND Apache-2.0 AND MIT AND BSD-3-Clause (+ OR-alternatives) — NOT
  wanted, forced: rustls' `fips` feature implies `aws_lc_rs`, which sets
  `aws-lc-rs/aws-lc-sys`. It is compiled (non-FIPS AWS-LC, own symbol prefix)
  but aws-lc-rs binds `aws-lc-fips-sys` whenever its `fips` feature is on, so
  no sync code path reaches it. Avoiding it would need a hand-written rustls
  `CryptoProvider` — rejected as more risk than a dead static library. Same
  release day as aws-lc-fips-sys 0.14.2; 0.46.0 (2026-10-12) is too young.
  Build deps (cc, cmake, dunce, fs_extra, pkg-config) were already locked.

### M3c — sync-core transport (HTTP(S) client + dir replica): no new crate

`sync-core` now depends directly on `rustls =0.23.45`, `rustls-webpki
=0.103.15`, `rustls-pki-types =1.15.1` (same versions + features as
sync-server, above) and `base64 =0.22.1` (already locked). Dev-only:
`sync-server` (path; the HTTP suite runs a real server on loopback) and
`tokio =1.53.1` (already locked). Cargo.lock delta = the `sync-core`
dependency list only; no `[[package]]` added. The client is hand-written
HTTP/1.1 over `std::net::TcpStream` (no hyper client, no reqwest), TLS 1.3
only on `rustls::crypto::default_fips_provider()`, config refused unless
`ClientConfig::fips()`. System roots are read from `$SSL_CERT_FILE` or the
distro bundle paths (no rustls-native-certs / webpki-roots crate).
Consequence: the app crate (which depends on sync-core) now also links
rustls + the forced non-FIPS aws-lc-sys described above; no other crate in
the lock uses rustls, so no existing TLS user changes behaviour.

Lock-only (present in Cargo.lock as optional deps of rustls-webpki, NOT
compiled — `cargo tree -p sync-server -i ring` prints nothing):

- `ring 0.17.14` — 2025-03-11 — `a4689e6c2294d81e88dc6261c768b63bc4fcdb852be6d1352498b114f61383b7`
  — Apache-2.0 AND ISC.
- `windows-sys 0.52.0` — 2023-11-15 — `282be5f36a8ce781fad8c8ae18fa3f9beff57ec1b52cb3de0789201425d9a33d`
  — MIT OR Apache-2.0 — ring's Windows dep.

Lock delta = these 9 package blocks + the dependency lists of sync-server
and rustls-webpki/ring, resolved by cargo on the box (bin/lockres.sh), lock
sha256 `59c605f4966f9e19fc82003a90338643552393075116b06993b2c9b1512b3f58`
copied back unchanged. cargo-vet exemptions for all nine; ISC exceptions
for rustls-webpki, untrusted, aws-lc-sys, ring in deny.toml.

Test fixture (not a crate): `crates/sync-server/tests/fixtures/test-{ca,leaf}.pem`
+ `test-leaf.key` — throwaway P-256 CA + leaf (SAN localhost, 127.0.0.1,
100 y validity) generated with the box's `openssl` 3.5.7 CLI; test-only key.

### M4a/M4b — commit DAG + tip merge (sync-core): no new crate

No crate added, Cargo.lock unchanged. The markdown three-way merge uses a
hand-written diff-match-patch (`crates/sync-core/src/merge/dmp.rs`: Myers
O(ND) line mode + char refinement, context patches, exact-match apply);
the lock has no diff crate (no `similar`, `dissimilar`, `diff-match-patch*`,
`imara-diff`) and adding one was rejected under "minimise new crates".
Settings JSON merge uses the already pinned `serde_json =1.0.151`.

### M5 — onboarding + keystore (sync-proto, sync-core): no new crate

No crate added to the lock. `sync-core` now lists `zeroize =1.9.1` directly
(already locked and pinned by sync-crypto; the only Cargo.lock change is
the extra `"zeroize"` line in sync-core's dependency list). The OS keystore
backends (Secret Service, macOS Keychain, Windows Credential Manager)
would each need a new crate (`keyring`, `secret-service`,
`security-framework`, … — none in the lock); they are a follow-up behind
the `sync_core::keystore::Keystore` trait. The headless/test backend
`FileKeystore` uses std only.

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

Per crate (direct deps): `sync-proto` → serde (dev: serde_json, JSON tests of the `/v1` bodies); `sync-crypto` → sha2, aws-lc-rs, aws-lc-fips-sys, zeroize (dev: serde_json, Wycheproof JSON);
`sync-core` → sync-proto, sync-crypto, sha2, serde, serde_json, zstd, zstd-safe, zstd-sys, rustls, rustls-webpki, rustls-pki-types, base64 (M3c; dev: sync-server, tokio); `syncd` →
sync-core; `sync-server` → sync-proto, sync-crypto, serde, serde_json, base64, bytes, http, http-body-util, hyper, hyper-util, tokio, rusqlite, libsqlite3-sys (M3a), rustls, tokio-rustls, rustls-webpki, rustls-pki-types, subtle, untrusted, aws-lc-sys (M3a-TLS). The Cargo.lock delta of
the skeleton is exactly the five path packages (`[[package]]` entries without
`source`/`checksum`); no registry package is added, removed or moved.

Each new crate only uses the subset it needs; `sync-server` may depend
only on `sync-proto` + `sync-crypto` (checked with `cargo tree -p sync-server`,
log in the goal work dir).

## Declared, NOT added yet

- diff-match-patch port — NOT added (M4b hand-written, see M4a/M4b above).

- `aws-lc-rs` — ADDED in M2a (above, pin decision cmvp.md §7).
- `rustls` with feature `fips` — ADDED in M3a-TLS (0.23.45). Was: 0.23.44 (2026-09-07) / 0.23.45
  (2026-09-14) satisfy the age rule; 0.24.0-dev.* are pre-releases.
- `axum` — not needed (plain hyper, M3a). `hyper`/`rusqlite` — ADDED in M3a above.

Checksums for the declared-not-added crates are recorded only when they are
actually added (from the Cargo.lock that adds them).

## Test data (not crates)

- Wycheproof (C2SP/wycheproof, Apache-2.0) commit
  `12fd3aaf33eb5fa1f52e026912ee00c054f9d984` (2026-10-06, newest commit
  ≥ 6 weeks old): four `testvectors_v1/*.json` files copied unmodified to
  `crates/sync-crypto/vectors/wycheproof/`; per-file sha256 in that
  directory's README.md, re-checked by `tests/wycheproof.rs`.
- EFF large wordlist (Electronic Frontier Foundation, CC BY 3.0 US), published
  2016-07-18 at https://www.eff.org/files/2016/07/18/eff_large_wordlist.txt,
  fetched 2026-11-18 as data (curl, no software), stored unmodified as
  `crates/sync-crypto/wordlists/eff_large_wordlist.txt` (108800 bytes, 7776
  lines), sha256
  `addd35536511597a02fa0a9ff1e5284677b8883b83e986e43f15a3db996b903e`, pinned
  as `sync_crypto::words::WORDLIST_SHA256` and re-checked by
  `tests/words.rs`. Embedded via `include_str!` (shipped). Attribution in
  THIRD-PARTY.md. No crate added for M2c.
- BIP-0039 English list: NOT vendored. The rejection tests use the 12-word
  mnemonics of the reference test vectors (inline strings) plus words shared
  by both lists, so no BIP39 file is shipped or committed.
