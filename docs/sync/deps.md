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

### M2a — OpenSSL 3 FIPS provider (sync-crypto)

Operator decision 2026-11-19 10:16Z ("okay openssl it is",
open-questions #13): the AWS-LC module is gone (aws-lc-fips-sys carries the
GPL-incompatible OpenSSL/SSLeay licence). The crypto suite now runs on the
**OpenSSL 3.5.9 FIPS provider** (CMVP #5586, cmvp.md) through rust-openssl,
behind the same `CryptoProvider` trait. Removed with it (Cargo.lock,
5966065): aws-lc-rs 1.16.0, aws-lc-fips-sys 0.13.11, aws-lc-sys 0.37.1,
ring 0.17.14 and their build graph (bindgen 0.72.1, cexpr, clang-sys,
cmake, either 1.19.0, fs_extra, itertools 0.13.0, libloading 0.8.9,
minimal-lexical, nom 7.1.3, prettyplease, shlex 1.3.0, windows-sys
0.52.0). Their rows are in git history (deps.md before 2692bd7).

Direct (sync-crypto/Cargo.toml), crates.io `created_at` checked
2026-11-19, cutoff for the 6-week rule = **2026-10-08**:

- `openssl =0.10.81` — 2026-06-12 — `77823a27f0babb03091cb9ed9ef80af3b39dbc82f97e8fa530374b7dafd87a45`
  — Apache-2.0 — safe EVP wrappers (cipher, MAC via ffi, KDF, Md, PKey,
  rand, Provider). default features off (no `vendored` -> no openssl-src;
  the C library is the box-built OpenSSL 3.5.9, below). 0.10.82
  (2026-11-01) is too young. All 2026 openssl advisories are patched
  <= 0.10.80 (§Advisories-5b).
- `openssl-sys =0.9.117` — 2026-06-12 — `b47e7e6bb2c38cd930d25a23b40fa52e068c10e85f3e03a7f5ba5aaca5713695`
  — MIT — FFI bindings; linked against `OPENSSL_DIR` (fips-env.sh,
  `OPENSSL_NO_VENDOR=1`). Its optional deps (aws-lc-fips-sys, aws-lc-sys,
  bssl-sys, openssl-src, bindgen) stay off — none is in Cargo.lock.
- `foreign-types =0.3.2` — 2017-11-28 — `f6f339eb8adc052cd2ca78910fda869aefa38d22d5cb648e6485e4d3fc06f3b1`
  — MIT/Apache-2.0 — `ForeignType::as_ptr` for the five raw calls below;
  same version rust-openssl uses (a second, 0.5.0, was already locked).
- `zeroize =1.9.1` — 2026-10-06 — `e13084392c5e4bc371903e2935a5eaeed24905a7511356b883835e18a78f6879`
  — Apache-2.0 OR MIT — wipes key material (1.9.2 = 2026-11-17 too young).

Transitive (fixed by Cargo.lock):

- `openssl-macros 0.1.1` — 2023-04-01 — `a948666b637a0f465e8564c73e89d4dde00d72d4d473cc972f390fc3dcee7d9c`
  — MIT/Apache-2.0 — proc-macro of openssl.
- `foreign-types-shared 0.1.1` — 2017-11-28 — `00b0228411908ca8685dba7fc2cdd70ec9990a6e753e89b6ac91a84c40fbaf4b`
  — MIT/Apache-2.0 — foreign-types (second version next to 0.3.1).

Reused from the lock, versions unchanged: bitflags 2.13.1, cfg-if 1.0.4,
libc 0.2.189, once_cell 1.21.4 (openssl); cc 1.4.4, pkg-config 0.3.34,
vcpkg 0.2.15 (openssl-sys build script).

**unsafe**: sync-crypto is `#![deny(unsafe_code)]` (was `forbid`); the only
exception is `src/ossl_ffi.rs` (`#![allow(unsafe_code)]`), which declares
five OpenSSL 3.0 APIs openssl-sys 0.9.117 does not expose:
`OSSL_PROVIDER_available`, `OSSL_PROVIDER_get0_name`,
`EVP_MD_get0_provider`, `EVP_CIPHER_CTX_get_params`,
`OSSL_PARAM_construct_utf8_string` (enforcement checks + reading the
provider-generated GCM IV, cmvp.md §4).

**OpenSSL C library (not a crate)**: `openssl-3.5.9.tar.gz` (release
2026-09-29, 51 days before 2026-11-19), sha256
`603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a`,
signature checked with gpg against the OpenSSL signing key
`B146 647E 45A7 B339 47AB 226B 2A2C 87D1 6169 2D40`; Apache-2.0
(GPLv3-compatible). Built on the box only, `./Configure enable-fips`, core
and FIPS provider from the same tarball (= the #5586 module version), then
`openssl fipsinstall -pedantic` (build log + per-file sha256: cmvp.md §3).

**Licence**: RESOLVED (open-questions #13). Nothing in the graph carries the
OpenSSL/SSLeay licence any more; the OpenSSL-licence and aws-lc ISC
exceptions are gone from deny.toml (90ae732, 3afbf3e) and `cargo deny check
licenses` passes (CI run 44515121547).

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

## Toolchain (box only)

Installed in user space under `/var/tmp/sync-e2e/toolchain` on the test box
(never on the development VM, nothing system-wide), each from the upstream
release artefact, sha256-checked against the upstream checksum before
unpacking; versions are the newest stable release ≥ 6 weeks old on
2026-11-18. Activated by `fips-env.sh` (goal work dir).

- OpenSSL `3.5.9` with `enable-fips` (see M2a): prefix
  `/var/tmp/sync-e2e/ossl/openssl-3.5.9`; fips-env.sh exports
  `OPENSSL_DIR`, `OPENSSL_LIB_DIR`, `OPENSSL_INCLUDE_DIR`,
  `OPENSSL_NO_VENDOR=1`, `OPENSSL_CONF` (openssl.cnf: `.include
  fipsmodule.cnf`, activate `base`, `default_properties = fips=yes`),
  `OPENSSL_MODULES`, `LD_LIBRARY_PATH`. Build: goal `bin/osslbuild.sh`,
  log `logs/ossl-3.5.9-build.log`.
- Perl `5.44.0` (released 2026-07-15; newest stable, 5.45.x are dev
  releases) — `perl-5.44.0.tar.gz` sha256
  `3b855066b92491cb40e86affb1ca57d1a388aa43e51b91c7806a32c2f65f96c3`
  (cpan.org `.sha256.txt`), built from source with the box's gcc
  (`Configure -des`, no man pages). Needed by OpenSSL's `Configure`.
- C compiler: the box harness gcc 16.0.1 (`/var/tmp/gate-boxd/env`, already
  used for every T0 build); no C++ compiler is needed.
- No longer used (AWS-LC only, removed from fips-env.sh 2026-11-19): Go
  1.27.1 (sha256 `63d339f0..8445`) and CMake 4.4.4 (sha256
  `e5bb807f..f5bb`); still on the box directory, not on PATH.
- clippy component for the pinned box toolchain (operator decision C,
  2026-11-19): `rustup component add clippy --toolchain 1.98.0` on the box
  only (rustup 1.28.2, shared `/var/tmp/gate-boxd/env/rustup`), same release
  as rustc 1.98.0 (88d9e12ae 2026-08-18; dist 2026-08-20): clippy 0.1.98,
  `clippy-1.98.0-x86_64-unknown-linux-gnu.tar.xz` sha256
  `646c6bd2450ea4c32a3c78ad7a3727294eea1f1519718a3f1487424021695ffa`
  (rustup checked it against the 1.98.0 channel manifest). Used for
  `cargo clippy -D warnings` on the sync crates in T0.
- Reinstalled unchanged (same versions + sha256) on the fresh box
  hz-176764494 on 2026-11-19 (goal bin/toolchain.sh, logs/toolchain.log).

## FIPS-0.13 — SUPERSEDED (2026-11-19 10:16Z)

The AWS-LC validated-line pin (aws-lc-fips-sys 0.13.11, cert #5314) and the
rustls/webpki caps it forced are gone with the OpenSSL swap; the section is
in git history (deps.md at 84d2606 and before). Current advisory triage:
§Advisories-5b.

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

### M3a-TLS — rustls on the OpenSSL FIPS provider (sync-server, sync-core)

TLS 1.3 only. Provider = `rustls_openssl::custom_provider` with exactly
TLS13_AES_256_GCM_SHA384 + TLS13_AES_128_GCM_SHA256 and key exchange
SECP256R1 + SECP384R1 (KAS-ECC-SSC in #5586; no ChaCha20, no X25519 /
ML-KEM hybrids), built only after `sync_crypto::fips::init()` succeeded;
`sync-server tls::fips_provider()` and `sync-core
transport::http::tls_provider()` refuse unless `CryptoProvider::fips()`
(tests/tls.rs, tests/fips_refuse.rs). Every crate is pinned direct so the
resolver cannot float to a younger release.

- `rustls =0.23.45` — 2026-09-14 — `0d41d731c7d2f962d1ccc364cec258de3c0e93b38c2fb3ba97ac74513048d634`
  — Apache-2.0 OR ISC OR MIT — feature `std` only (no `aws_lc_rs`, no
  `fips`, no tls12, no brotli, no logging). Bumped from 0.23.43 for
  RUSTSEC-2026-0285 once the aws-lc-rs 1.16 cap was gone. Advisories:
  §Advisories-5b.
- `rustls-openssl =0.4.2` — 2026-10-02 — `ddabe409dbf2e374bae07c4b8cc90dbb890de5a47890312c46951661eb78760c`
  — MIT — rustls `CryptoProvider` on rust-openssl (EVP); default features
  off (drops `tls12` and `prefer-post-quantum`); its `fips` feature is a
  no-op and is not used (enforcement is sync-crypto's). Requires rustls
  ^0.23.20, openssl ^0.10. Smallest compliant option (rule e): the
  alternative, a hand-written rustls CryptoProvider over EVP, is more
  unaudited code in the TLS path.
- `tokio-rustls =0.26.6` — 2026-09-28 — `c9cc2678c2cdd569ef8215e2afd7954ada2ae20b4fdd2c5fe6139a3b02d105db`
  — MIT OR Apache-2.0 — default features off (was `fips`).
- `rustls-webpki =0.103.15` — 2026-08-21 — `f3c3cf1d8b1e7d4927e2d154c3fcb02979afb9939629c62cd9048d4f07b60ac2`
  — ISC — feature `std`. Required by rustls 0.23.45 (`^0.103.14`); newest
  >= 6 weeks. VENDORED: src-tauri/vendor/rustls-webpki-0.103.15 = the crates.io
  crate byte-for-byte + Cargo.toml-only noring.patch (sha256 `022975c7..a8b9`),
  proved by scripts/supply-chain/webpki-vendor-check.sh. Why: its `alloc`
  feature names `ring?/alloc`, a weak dependency feature that makes Cargo
  lock the optional ring crate even though nothing compiles it
  (rust-lang/cargo#10801); the patch drops the dep so ring leaves
  Cargo.lock. Advisories: §Advisories-5b.
- `rustls-pki-types =1.15.1` — 2026-07-23 — `2f4925028c7eb5d1fcdaf196971378ed9d2c1c4efc7dc5d011256f76c99c0a96`
  — MIT OR Apache-2.0 — PEM parsing (`pem::PemObject`).
- `subtle =2.6.1` — 2024-06-24 — `13c2bddecc57b384dee18652358fb23172facb8a2c51ccc10d74c157bdea3292`
  — BSD-3-Clause — rustls.
- `untrusted 0.9.0` — 2021-07-13 — `8ecb6da28b8a351d773b68d5825ac39017e680750f980f3a1a85cd8dd28a47c1`
  — ISC — rustls-webpki (transitive; the direct pin was dropped).
- REMOVED: `aws-lc-sys =0.37.1` (forced by rustls' `fips` feature, dead
  code) and the lock-only `ring 0.17.14` / `windows-sys 0.52.0`.
  `cargo tree -p sync-server -p sync-core -p syncd -e normal` shows no
  aws-lc*, ring or tauri crate (goal log ossl-tls-tree.log).

### M3c — sync-core transport (HTTP(S) client + dir replica): no new crate

`sync-core` depends directly on `rustls =0.23.45`, `rustls-openssl
=0.4.2`, `rustls-webpki =0.103.15`, `rustls-pki-types =1.15.1` (same
versions + features as sync-server, above) and `base64 =0.22.1` (already
locked). Dev-only: `sync-server` (path; the HTTP suite runs a real server
on loopback) and `tokio =1.53.1` (already locked). The client is
hand-written HTTP/1.1 over `std::net::TcpStream` (no hyper client, no
reqwest), TLS 1.3 only on `transport::http::tls_provider()` (OpenSSL FIPS
provider, see M3a-TLS), config refused unless `ClientConfig::fips()`.
System roots are read from `$SSL_CERT_FILE` or the distro bundle paths (no
rustls-native-certs / webpki-roots crate). Consequence: the app crate
(which depends on sync-core) also links rustls + rust-openssl; no other
crate in the lock uses rustls, so no existing TLS user changes behaviour.

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

### M6a — opensidian-syncd daemon + CLI (syncd): no new crate

No crate added to the lock. `syncd` lists `serde =1.0.229`, `serde_json
=1.0.151`, `zeroize =1.9.1` and `tokio =1.53.1` (features `rt`, `net`
only) — all already locked and pinned by sync-core/sync-server. tokio is
there for one reason: `tokio::net::UnixStream::peer_cred()` is the safe,
stable SO_PEERCRED/getpeereid wrapper (std's `UnixStream::peer_cred` is
still unstable, and the sync crates are `#![forbid(unsafe_code)]`, so
libc's `getsockopt` is out). The CLI argument parser is hand-written (no
clap). The daemon's own uid is read without `getuid()` as the owner of a
file it just created in the socket dir.

Later (CodeRabbit #238 api.rs:150): `syncd` also lists `base64 =0.22.1`
(already locked, pinned by sync-core and sync-server) to send history bytes
on the daemon socket as one base64 string; the lock only gains the
`"base64 0.22.1"` line in the `syncd` entry, no new package.

### M6b — embedded engine + command gate (app, src-tauri): no new crate

No crate added to the lock. The app (`opensidian`) gains two dependency
lines in its lock entry: `syncd` (workspace path crate) and `zeroize
=1.9.1` (already locked, used by sync-core/syncd; wipes the setup secret).
The plugin-realm IPC test uses tauri's own mock runtime: a dev-dependency
line enabling tauri's `test` feature, which tauri 2.11.5 declares as
`test = []` (no optional dependency behind it), so the lock is unchanged.

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

Per crate (direct deps): `sync-proto` → serde (dev: serde_json, JSON tests of the `/v1` bodies); `sync-crypto` → sha2, openssl, openssl-sys, foreign-types, zeroize (M2a) (dev: serde_json, Wycheproof JSON);
`sync-core` → sync-proto, sync-crypto, sha2, serde, serde_json, zstd, zstd-safe, zstd-sys, rustls, rustls-openssl, rustls-webpki, rustls-pki-types, base64 (M3c; dev: sync-server, tokio); `syncd` →
sync-core, serde, serde_json, zeroize, tokio (M6a: `rt` + `net` for the safe SO_PEERCRED `peer_cred`; dev: sync-server); `sync-server` → sync-proto, sync-crypto, zeroize, serde, serde_json, base64, bytes, http, http-body-util, hyper, hyper-util, tokio, rusqlite, libsqlite3-sys (M3a), rustls, rustls-openssl, tokio-rustls, rustls-webpki, rustls-pki-types, subtle (M3a-TLS). The Cargo.lock delta of
the skeleton is exactly the five path packages (`[[package]]` entries without
`source`/`checksum`); no registry package is added, removed or moved.

Each new crate only uses the subset it needs; `sync-server` may depend
only on `sync-proto` + `sync-crypto` (checked with `cargo tree -p sync-server`,
log in the goal work dir).

## Declared, NOT added yet

- diff-match-patch port — NOT added (M4b hand-written, see M4a/M4b above).

- `aws-lc-rs` — added in M2a, REMOVED 2026-11-19 (OpenSSL swap, open-questions #13).
- `rustls` — ADDED in M3a-TLS (0.23.45, feature `std` only; the former `fips`
  feature is gone with aws-lc-rs); 0.24.0-dev.* are pre-releases.
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

## Advisories-5b — RustSec triage of the sync dependency graph (2026-11-19)

cargo deny (CI supply-chain.yml run 44515121547 on 3afbf3e, cargo-deny 0.21.0)
reported 19 advisories on sync dependencies after the OpenSSL swap. Policy:
bump to a fixed release only if it is >= 6 weeks old (cutoff 2026-10-08),
else ignore with a reachability reason and an EXPIRY = the day the fix turns
6 weeks old. Source of truth: supply-chain/vex.cdx.json (CycloneDX 1.6, per-id
analysis); deny.toml and src-tauri/.cargo/audit.toml hold the same ids
(scripts/supply-chain/vex-check.sh). An expired entry fails CI.

- FIXED by bump: RUSTSEC-2026-0285 (rustls handshake messages across
  encryption levels) — rustls 0.23.43 -> 0.23.45 (+ webpki 0.103.15).
- rustls 0.23.45, fix 0.23.46 (2026-10-14), EXPIRES 2026-11-25:
  0346 (Debug of session values: resumption disabled, no rustls Debug),
  0347 (ECH, introduced at 0.23.44: ECH never configured), 0348 (brotli
  feature off), 0349 (classical ECDHE only, no hybrid group to downgrade
  from; resumption disabled).
- zstd 0.14.0 / zstd-safe 8.0.0 / zstd-sys 2.1.0, fixes zstd 0.14.3 /
  zstd-safe 8.2.0 / zstd-sys 2.1.2 (2026-10-16), EXPIRES 2026-11-27:
  0391, 0395-0406 per affected function — sync-core calls only
  `zstd::bulk::compress` and `zstd::stream::read::Decoder::new` with a fresh
  context per object, on AEAD-authenticated input; features seekable,
  zstdmt, legacy, zdict_builder are off (box `cargo tree -e features`,
  goal log 5b-zstd-features.log); 0410 is wasm32-only.
- libsqlite3-sys 0.38.2 (bundled SQLite 3.53.2), fix 0.39.0 / rusqlite 0.41
  (2026-10-24), EXPIRES 2026-12-05: 0459 needs a crafted `-wal` next to the
  index in sync-server's private 0700 data dir.
- rustls-webpki 0.103.15 (vendored path crate: cargo deny/audit cannot match
  it), fix 0.103.16 (2026-10-20), EXPIRES 2026-12-01: 0382 (distrusted
  emSign intermediates) is reachable only with `Roots::System`; pin the
  server CA (`Roots::Pem`). Listed in both ignore lists so the triage is
  visible; cargo deny reports it as advisory-not-detected (warning).
- RUSTSEC-2026-0049/0098/0099/0104 rustls-webpki: patched <= 0.103.13.
- openssl 0.10.81: RUSTSEC-2026-0463/0473-0479 all patched <= 0.10.80.

## Supply-chain gates for the OpenSSL swap (2026-11-19, 9e7be73)

- **cargo vet** (src-tauri/supply-chain/config.toml): exemptions — not
  audits; nobody line-reviewed these crates — each with a note (pin, date,
  sha256, why) for openssl 0.10.81, openssl-macros 0.1.1, openssl-sys
  0.9.117, rustls-openssl 0.4.2. `[policy.rustls-webpki]
  audit-as-crates-io = true` (same as glib) so the vendored path crate is
  checked against its crates.io exemption. 17 exemptions of the
  aws-lc/ring era whose (name, version) is no longer in Cargo.lock were
  dropped. CI run 44518429210: vet reports only the 9 fluent/intl crates
  that come from develop's i18n work (not sync, QUESTION-ci-i18n.md in
  the goal dir).
- **rust-sbom.sh**: emits the vendored rustls-webpki 0.103.15 like glib
  (version, crate sha256 and patch sha256 read from
  webpki-vendor-check.sh; pedigree = noring.patch) and the five sync
  workspace crates as first-party GPL-3.0-or-later components WITHOUT a
  purl (a `pkg:cargo/...` purl would make the CVE scan match unrelated
  crates.io crates of the same name). Any other source-less crate still
  fails the script. sbom-merge.test.sh covers both, plus two negatives
  that must fail (stray non-workspace path crate, wrong webpki version).
- **cargo deny**: licenses/bans/sources ok without any OpenSSL-licence
  exception (CI 44515121547); advisories ok after §Advisories-5b (CI
  44517061643).

### CLI release workflow (sync-cli.yml, 2026-11-20)

`.github/workflows/sync-cli.yml` (docs/sync/release-cli.md) adds **no new
crate and no new action**: it reuses release.yml's pins (actions/checkout
`d23441a4…` v6.1.0, actions/attest-build-provenance `4d101475…` v4.2.2,
actions/upload-artifact `043fb46d…` v7.0.1, actions/download-artifact
`3e5f45b2…` v8.0.1). New pinned inputs:

- **OpenSSL release keyring**: `supply-chain/keys/openssl-release-keyring.gpg`
  (`gpg --export` of `B146 647E 45A7 B339 47AB 226B 2A2C 87D1 6169 2D40`,
  OpenSSL <openssl@openssl.org>, rsa4096 created 2026-05-26, signing subkey
  `C46E D3F2 CBEF DA1F DAAD A442 64ED 7B1D CCE7 1CB2`, fetched from
  keys.openpgp.org 2026-11-20), sha256 in `supply-chain/keyrings.tsv`,
  re-checked by `packaging/sync/openssl-fips.sh` before `gpgv`.
- **openssl-3.5.9.tar.gz.asc** sha256
  `bdccf2057a7c4ae377b40e832cb8c632ae8b532d4fb64cc7b843066eee748f89`
  (same file as the box build's research copy), tarball sha256 unchanged.
- OpenSSL is configured `enable-fips no-shared` for the CLIs (libcrypto
  static; fips.so stays the separate module). The box build stays shared.
