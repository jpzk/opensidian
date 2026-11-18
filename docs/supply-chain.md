# Supply-chain checks

Workflow: `.github/workflows/supply-chain.yml`. It runs on every `pull_request`, on every push to
`develop`/`main`, and once a week (Monday 04:23 UTC), so new advisories also show up when no code
changes. It covers the Rust graph in `src-tauri/Cargo.lock`. The workflow uses
`permissions: contents: read` and no secrets. It uses `pull_request`, never `pull_request_target`, so
fork PRs run without write access. Every action is pinned by full commit SHA and the Rust toolchain
is pinned to an exact release. Pins are chosen to be at least 6 weeks old.

**Exception: the three scanners are not pinned** (operator decision, 2026-11-15). cargo-audit,
cargo-deny and cargo-vet are installed with `cargo install --locked <tool>`, so every run gets the
latest crates.io release, built from that release's own `Cargo.lock`. Each job prints the result in
a `cargo-<tool> resolved version` step, so the log shows exactly which version ran. Why:

- They are vulnerability scanners. The RustSec advisory DB and its format, and the cargo-vet
  import/audit formats, keep moving. An old release can fail to parse new advisories, or skip them,
  and then the check stops protecting anything.
- The blast radius is bounded. The workflow has `contents: read`, no secrets and no caches, and it
  never runs on `pull_request_target`. A compromised scanner release could make a run pass or fail
  falsely or waste runner time, but it can't push code, read secrets or poison later runs.

The cost: a run is not reproducible by tool version, and a broken new release can turn CI red with no
change in this repo. Check the `resolved version` step first when that happens.

## What each check catches

- **cargo audit** (`cargo audit --deny warnings`, run in `src-tauri/`). Checks every crate in
  `Cargo.lock` against the RustSec advisory DB. It fails on vulnerabilities and, because of
  `--deny warnings`, also on unmaintained, unsound and yanked crates. Ignored ids live in
  `src-tauri/.cargo/audit.toml`.
- **cargo deny** (`cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml --locked check`).
  It runs four checks:
  - `advisories`: the same RustSec DB, plus a deny on yanked versions. Ignores are in `deny.toml` and
    must match `audit.toml`.
  - `sources`: only the crates.io registry is allowed. Any other registry or any git dependency
    fails (`unknown-registry`/`unknown-git = "deny"`, `allow-git = []`). The vendored `glib` `[patch]`
    is a path crate and is allowed.
  - `bans`: wildcard (`*`) version requirements fail. Duplicate versions only warn.
  - `licenses`: every dependency license has to be on the allow-list. Licenses outside the list
    (GPL, AGPL, unknown, unlicensed) fail. The app crate's own GPL-3.0-or-later is an exception that
    applies to `opensidian` only.
- **cargo vet** (`cargo vet --locked`, run in `src-tauri/`). Every crate version has to be covered
  by an audit. That can be our own (`supply-chain/audits.toml`), an import from mozilla, google,
  bytecode-alliance, zcash or isrg (pinned in `supply-chain/imports.lock`), or an exemption
  (`supply-chain/config.toml`). The baseline exempts the graph as of 2026-11-15. A **new crate or a
  version bump without an audit fails**. `--locked` means CI uses the committed `imports.lock` and
  never pulls new imported audits by itself.

## Adding or updating a dependency

1. Change `Cargo.toml` and commit the updated `Cargo.lock`.
2. Run the checks locally with the latest tools, like CI does:
   ```sh
   cargo install --locked cargo-vet   # likewise cargo-deny, cargo-audit
   cd src-tauri && cargo vet
   ```
3. If `cargo vet` reports an unvetted crate, pick one of these options:
   - **Import.** `cargo vet` (without `--locked`) may find that a trusted importer already audited it.
     Commit the updated `imports.lock`.
   - **Audit it yourself.** Run `cargo vet inspect <crate> <version>` (or
     `cargo vet diff <crate> <old> <new>` for a bump), read the code against the criteria
     (`safe-to-deploy` covers anything that ships in the app), then
     `cargo vet certify <crate> <version>`. This records the audit in `supply-chain/audits.toml`, and
     the PR reviewer checks it.
   - **Exempt it (last resort).** Run `cargo vet add-exemption <crate> <version>` and say why in the
     PR. Exemptions are debt. Run `cargo vet prune` from time to time to drop the ones imports now
     cover.
4. If you need a new license, add it to `[licenses].allow` in `deny.toml` in the same PR and explain
   it there. If you need a git dependency, add its URL to `[sources].allow-git` and pin it by `rev`.
5. To ignore an advisory, add its RUSTSEC id to **both** `deny.toml` (`[advisories].ignore`, with
   `reason`) and `src-tauri/.cargo/audit.toml`, with a reason and an **expiry date**. Remove it when
   it expires.

## Limits

- **The advisory DB is not pinned.** cargo audit and cargo deny fetch the RustSec DB fresh on every
  run. A red run with no code change usually means a new advisory was published, so use the weekly
  schedule as an alarm. It also means results can't be reproduced bit for bit.
  Example: RUSTSEC-2026-0543 (chrono 0.4.45 unmaintained, informational, no vulnerability; chrono
  comes in transitively through serde_with / tauri-utils) turned `cargo audit` red with no code
  change. It is ignored in both `src-tauri/.cargo/audit.toml` and `deny.toml`, expiring 2027-05-15,
  the same as the other unmaintained transitive crates and the same exception #188 carries.
- **cargo vet trusts people, not code.** Imported audits are only as good as the organisations
  behind them. Exemptions in the baseline were not audited. They only freeze today's graph.
- `cargo vet --locked` does not pick up audits that importers publish later, so run `cargo vet` (or
  `cargo vet regenerate imports`) on purpose to refresh `imports.lock`.
- A git dependency for a crate version that is already exempted or audited still passes cargo vet.
  That case is caught by cargo deny `sources` (`unknown-git = "deny"`), so keep both checks.
- Only the Rust graph is covered. The npm frontend, the GitHub Actions themselves (they are only
  SHA-pinned) and system libraries (GTK/WebKitGTK from the distro) are out of scope.
- None of these checks catch malicious code in an already-trusted version, or a compromised build
  environment. Pinning plus the 6-week age rule just shrinks that window, and the three scanners
  are deliberately outside it (see the exception above).

## pin-check (#189)

`scripts/supply-chain/pin-check.sh` fails CI (`supply-chain.yml` job `pin-check`) when any build,
CI, packaging or bundled dependency is not pinned to an exact version **and** a content hash:

- `uses:` without a 40-hex commit SHA (local `./` actions excepted)
- `image:` / `container:` / `FROM` / `podman|docker pull|run|create` without `@sha256:<64 hex>`
- `apt-get install` without `pkg=version`, `dnf install` without `pkg-version-release`
- `cargo install` without `--locked` and `--version`
- `flatpak install` without `--commit=<64 hex>`
- `curl`/`wget` downloads without a `sha256sum`/`sha512sum` check within 6 lines; `curl … | sh`
- `npm`/`npx`/`yarn`/`pnpm`/`bun`/`pipx`/`go install` (forbidden), `pip install` without `--require-hashes`

Exceptions live in `supply-chain/pin-allow.tsv` (TAB-separated: rule, path, needle, expiry, scanner,
reason). An entry needs a reason and an unexpired `YYYY-MM-DD`; an entry that no longer matches
anything is itself a finding. The scanner exception is a *named* list (semgrep, opengrep,
cargo-audit, cargo-deny, cargo-vet, osv-scanner): a scanner entry's needle must contain the
scanner's name, so the exception cannot cover anything else, even inside a scanner job.
Self-test: `bash scripts/supply-chain/tests/pin-check.test.sh` (one red fixture per rule, plus
mutations that unpin one thing each in an all-green fixture tree).

### AppImage build environment (#189 item 1)

The slim and portable AppImages (and the binary the rpm/deb wrap) are built by
`packaging/ci/appimage-build.sh` inside `ubuntu:resolute-20260927` pinned by its
multi-arch manifest-list digest. apt reads only `snapshot.ubuntu.com` at `SNAP`
(resolute, -updates, -security; main, universe), with the archive signatures still
checked. Every requested package is pinned `pkg=version`, and the script fails if
the installed version differs. Everything they pull in is fixed by the frozen
snapshot. The `ubuntu-26.04` runner floats by design, but it is only the host that
runs podman: nothing it ships is compiled in or bundled.

Each build records two workflow artifacts, `appimage-pkgset-<ver>-<arch>`:
- `pkgset-<ver>-<arch>.tsv`: every installed package with its name, version, arch
  and the sha256 of the `.deb`. This is the SBOM input for #188.
- `libset-<ver>-<arch>.tsv`: the sha256 of every library in the portable AppDir.
  Two builds of the same tag must produce identical files.

Rust 1.98.0 comes from the dist tarball. The script checks
`channel-rust-1.98.0.toml` against `RUST_MANIFEST_SHA256`. The per-arch `xz_hash`
in that manifest must equal `RUST_XZ_SHA256_<arch>`, and the tarball must match it.

**Bump the snapshot** when the weekly CVE scan flags WebKitGTK, GTK or OpenSSL in the
pkgset, or on a routine refresh:
1. Pick a `SNAP` timestamp and an image tag that are both at least 6 weeks old. The
   snapshot must not be older than the image.
2. Read `docker-content-digest` from the registry for the tag's manifest list, and
   check that it contains both linux/amd64 and linux/arm64/v8. Set `DIGEST`.
3. For each package in `PKGS`, read its version from the snapshot's `Packages`
   indexes, preferring -updates, then -security, then the release pocket. Check both
   `binary-amd64` and `binary-arm64`, since `PKGS` holds one version for both arches.
4. Run the dry-run release twice. Compare the `pkgset` and `libset` files: they must
   be identical between the two runs. Diff them against the previous snapshot and
   review what changed.

**Bump Rust**: see "Rust toolchain" below. In this script set `RUST`, copy
`RUST_MANIFEST_SHA256` and both `xz_hash` values from the manifest. The script refuses
to run if `RUST` differs from `rust-toolchain.toml` or its sha256 from
`rust-toolchain.sh`.

### Flatpak build environment (#189 item 2)

`packaging/flatpak/runtimes.sh` sets up the flatpak job in `release.yml`:
- `host`: the runner's apt reads only `snapshot.ubuntu.com` at the same `SNAP` as the
  AppImage build. It installs `flatpak`, `flatpak-builder` and `elfutils` as
  `pkg=version`, then checks the installed versions.
- `remote`: adds flathub from `flathub.flatpakrepo` after checking its sha256
  (`FLATHUB_REPO_SHA256`). The file carries flathub's GPG key, so a changed key fails
  here. flatpak then checks every pulled commit's signature against that key.
- `log`: prints each pinned commit with its date, plus the newest flathub commits of
  the ref. This is the input for a bump.
- `install`: installs `org.gnome.Platform//49`, `org.gnome.Sdk//49` and
  `org.freedesktop.Sdk.Extension.rust-stable//25.08` with `--no-related`, moves each
  one to its pinned commit (`flatpak update --commit=`), and checks the deployed commit
  (`flatpak info --show-commit`). It fails if a commit is younger than 6 weeks, or if
  anything other than the 3 pinned refs is installed. The build log has one
  `PINNED <ref> <commit>` line per ref.

A branch such as `//49` is not a pin, because flathub moves it with every runtime
update. Each `COMMIT_<ref>` is a full 64-hex ostree commit for each arch. pin-check
accepts `--commit="$VAR"` only when every `VAR=` assignment in the file is 64 hex. The
flathub submission of `dev.koto.opensidian` must build against the same commits.

**Bump a runtime commit** for a runtime security update, or a new GNOME branch:
1. Read the `log` step of the last release run. For each ref and arch, pick the newest
   commit that is at least 6 weeks old. A new branch also means updating the manifest's
   `runtime-version` and the `REF_*` lines.
2. Set the six `COMMIT_*` values. Run the dry-run release, and check the `PINNED`
   lines and the flatpak artifacts.
3. Bump `HOST_PKGS` together with `SNAP`, using the same procedure as the AppImage
   snapshot, against the `resolute` indexes for both arches.

### Rust toolchain (#189 item 5)

`rust-toolchain.toml` names the channel (an exact `x.y.z`, never `stable`) and the
components. A version is not a pin, so `scripts/supply-chain/rust-toolchain.sh`:
1. downloads `channel-rust-<channel>.toml` and checks it against `RUST_MANIFEST_SHA256`
   in the script (the `.sha256` file next to it on the server is not a pin: it comes
   from the same place);
2. runs `rustup toolchain install`, which checks every component against that manifest;
3. checks that the installed `rustc -vV` commit-hash is the manifest's `[pkg.rustc]`
   `git_commit_hash`. The log has one `PINNED rust <v> rustc <commit>` line.

The `supply-chain.yml` jobs install Rust through this script. The AppImage build
installs from the dist tarball (above) and checks the same channel and sha256. The
build box (`bin/hz/release-build.sh` in the harness) must use the same channel.

**Bump the toolchain**: pick a release at least 6 weeks old. Set `channel` in
`rust-toolchain.toml`, `RUST_TOOLCHAIN` in `release.yml` and `supply-chain.yml`, and
`RUST`, `RUST_MANIFEST_SHA256` and both `RUST_XZ_SHA256_<arch>` in
`packaging/ci/appimage-build.sh`. Set the same `RUST_MANIFEST_SHA256` in
`rust-toolchain.sh`. Check the value with
`bash scripts/supply-chain/rust-toolchain.sh --manifest-only`.

### Scanners

cargo-audit, cargo-deny and cargo-vet (and semgrep/opengrep/osv-scanner) are the
named exception in `pin-allow.tsv` (see the top of this file). They have no bump
procedure: CI installs the latest release and logs its version. If a scanner release
breaks CI, fix the config or the finding. Do not pin the scanner around it.

### Runner images and Android

- The GitHub-hosted runner (`ubuntu-26.04`) floats by design (#189 item 8). Every
  build that ships runs inside a pinned container or a pinned flatpak runtime, so
  the runner only hosts podman/flatpak. Nothing from the runner is compiled in or
  bundled. The host tools it does install (flatpak, flatpak-builder, elfutils) come
  from the pinned snapshot.
- Android (#189 item 7): `develop` has no Android build. There is no Gradle project,
  wrapper, SDK or NDK in the tree, so there is nothing to lock yet. The android-merge
  branch is not part of this pin. When it lands it must add Gradle dependency locking
  (`gradle.lockfile`), `gradle/verification-metadata.xml` with sha256 for every
  artifact, the wrapper's `distributionSha256Sum`, and pinned SDK/NDK/build-tools
  versions with checksums. pin-check will be extended to cover them then.
