# Opensidian Sync — CLI release (opensidian-sync-server, opensidian-syncd)

Workflow: `.github/workflows/sync-cli.yml`. Scripts: `packaging/sync/`
(`openssl-fips.sh`, `cli-package.sh`, `cli-smoke.sh`, `cli-README.md.in`).
The two sync binaries are released **independently of the app**, as plain
tarballs, for Flatpak / AppImage users who self-host or run the sync service
outside the sandbox. deb/rpm packages: later, not part of v1.

Scope v1: **Linux x86_64 + Linux aarch64**, each built natively
(`ubuntu-26.04`, `ubuntu-26.04-arm`), no cross-compilation. macOS: see the
follow-up at the end.

## Triggers

- push to `feat/sync` touching `crates/sync-*/**`, `crates/syncd/**`,
  `packaging/sync/**`, the OpenSSL keyring, the workflow, `src-tauri/Cargo.lock`;
  and `workflow_dispatch`: build + test + smoke + attest, artifacts only.
- tag `sync-v<ver>` (must equal the crates' version): the same, plus a
  **DRAFT** GitHub release with both tarballs + `SHA256SUMS`. The workflow never
  publishes; pushing the tag and publishing are operator decisions.

## Pipeline (per arch)

1. Rust 1.98.0 by dist-manifest sha256 (`scripts/supply-chain/rust-toolchain.sh`).
2. OpenSSL 3.5.9 (`packaging/sync/openssl-fips.sh`): tarball sha256
   `603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a`, its
   `.asc` sha256 `bdccf205…f89` and `gpgv` against the committed
   `supply-chain/keys/openssl-release-keyring.gpg` (listed in
   `supply-chain/keyrings.tsv`; VALIDSIG must chain to primary
   `B146 647E 45A7 B339 47AB 226B 2A2C 87D1 6169 2D40`), then
   `./Configure enable-fips no-shared`, `make install_sw install_fips`.
   `no-shared`: libcrypto is linked **statically** into both binaries and the
   bundled `openssl`; the FIPS provider stays the separate `fips.so` module by
   design (it is the validated boundary). `cli-package.sh` refuses any ELF
   that needs `libcrypto.so`/`libssl.so`.
3. `cargo build --release --locked` with `OPENSSL_DIR`, `OPENSSL_NO_VENDOR=1`,
   `OPENSSL_STATIC=1`.
4. `cargo test --locked -p sync-proto -p sync-crypto -p sync-core -p syncd -p sync-server`
   against that OpenSSL (fipsinstall run on the CI host for the tests);
   the step fails unless the FIPS module test, the FIPS KATs and
   `refuses_without_fips_module` ran and passed.
5. Package, then smoke the **packaged tarball** with no build env
   (`cli-smoke.sh`): launchers refuse before fipsinstall, the binary refuses
   with an empty module dir, `opensidian-sync-fipsinstall`, server on
   127.0.0.1, `vault create`, client A `setup --enroll` + `status` +
   `sync-now`, client B `setup --words` (stdin) + `sync-now`, B edits, A pulls.
6. glibc baseline in the job summary, `actions/attest-build-provenance` per
   tarball, artifacts uploaded on every run. Job `sums`: `SHA256SUMS` over both
   tarballs (attested too). Job `draft-release`: tags `sync-v*` only.

Actions: the same SHA pins as `release.yml` (checkout v6.1.0,
attest-build-provenance v4.2.2, upload-artifact v7.0.1, download-artifact
v8.0.1); no new action. Known gap: the build HOST toolchain (runner gcc, perl,
make) floats with the ubuntu-26.04 image; follow-up: build inside the
digest-pinned container like `packaging/ci/appimage-build.sh`.

## Artifact layout

`opensidian-sync-<ver>-linux-<arch>.tar.gz`, `<arch>` = `x86_64` | `aarch64`,
one top directory `opensidian-sync-<ver>-linux-<arch>/`:

    opensidian-sync-server          sh launcher (sets OPENSSL_CONF/OPENSSL_MODULES to this dir)
    opensidian-syncd                sh launcher
    opensidian-sync-fipsinstall     run once per machine (below)
    libexec/opensidian-sync-server  ELF, libcrypto static
    libexec/opensidian-syncd        ELF, libcrypto static
    libexec/openssl                 OpenSSL 3.5.9 CLI (for fipsinstall), libcrypto static
    lib/ossl-modules/fips.so        OpenSSL 3.5.9 FIPS provider (CMVP #5586)
    ssl/                            empty; fipsinstall writes fipsmodule.cnf + openssl.cnf here
    licenses/LICENSE-GPL-3.0                  Opensidian (GPL-3.0-or-later)
    licenses/LICENSE-OpenSSL-Apache-2.0.txt   OpenSSL (Apache-2.0)
    licenses/NOTICE-EFF-wordlist.txt          EFF large wordlist, CC BY 3.0 US attribution
    licenses/CRATES.txt                       linked Rust crates + licences (cargo tree)
    README.md  BUILDINFO.txt                  usage; commit, rustc, openssl, glibc, sha256s

No RPATH is needed: nothing links a shared libcrypto. The launchers resolve
their own directory (`readlink -f "$0"`), so the tree runs from wherever it
is extracted, and symlinks to the launchers (e.g. in `~/.local/bin`) work.
`OPENSIDIAN_SYNC_OPENSSL_CONF` overrides the bundled `openssl.cnf`.

## Verify a download

    sha256sum -c --ignore-missing SHA256SUMS
    gh attestation verify opensidian-sync-<ver>-linux-<arch>.tar.gz --repo jpzk/opensidian

The attestation is SLSA build provenance from `actions/attest-build-provenance`:
it binds the tarball's sha256 to this repository, the workflow file
(`sync-cli.yml`), the commit and the run.

## fipsinstall (once per machine)

    tar xzf opensidian-sync-<ver>-linux-<arch>.tar.gz
    cd opensidian-sync-<ver>-linux-<arch>
    ./opensidian-sync-fipsinstall

It runs `openssl fipsinstall -pedantic` with the bundled `openssl` on the
bundled `fips.so` — the module's self-tests run on THIS machine — and writes
`ssl/fipsmodule.cnf` plus an `ssl/openssl.cnf` (absolute `.include`, `fips` +
`base` providers, `default_properties = fips=yes`), then `-verify`s it. The
tarball deliberately ships no `fipsmodule.cnf` (OpenSSL: run fipsinstall on
the target). Run it again after moving the directory. Without it the
launchers stop with a hint; the binaries themselves refuse to start unless
the 3.5.9 provider is active in approved mode (`sync_crypto::fips::init`).
The directory path must not contain whitespace.

## FIPS note (no compliance claim)

The bundled provider is the CMVP **#5586** module version (OpenSSL FIPS
Provider 3.5.9), built from the signature-verified upstream source as
OpenSSL's Security Policy describes; see `cmvp.md`.

- **Linux x86_64**: the certificate's tested operational environments include
  Linux on x86_64 (Ubuntu 22.04.1 / 24.04.2, Debian 11.5 / 12; cmvp.md §2).
  A user's distribution/CPU is very likely not one of the exact tested OEs
  (porting case, no CMVP statement).
- **Linux aarch64: NOT a tested operational environment** of #5586 (its only
  ARM OE is macOS 15.5 on Apple M4). The aarch64 tarball runs the same
  module source in approved mode, but nothing about it is covered by the
  certificate's OE list.

## glibc baseline

Each tarball records the highest `GLIBC_` symbol version any of its ELFs
requires (`objdump -T`): `BUILDINFO.txt`, the README's Requirements line,
`glibc-<arch>.txt` in the run artifact and the job summary. Built on Ubuntu
26.04, so the floor is that image's glibc symbol set; older distributions
need a build on an older base (follow-up together with the pinned container).
Recorded for the first green run: see the run linked in PR #238.

## Follow-ups (not v1)

- **macOS** (aarch64 + x86_64): needs the Keychain keystore backend (the
  `FileKeystore` is the headless backend; `keystore.rs`), a launchd user agent
  instead of systemd, Developer ID code signing + notarization, and the
  OpenSSL provider loading story for a signed bundle. No mac target is built.
- deb/rpm packages; systemd user unit for `opensidian-syncd daemon`.
- Pinned build container (glibc floor, host-toolchain pinning).
