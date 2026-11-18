# Supply-chain checks

Workflow: `.github/workflows/supply-chain.yml` (#188). It is the **one** workflow for all SBOM, CVE
scanning and dependency policy work. Scripts live in `scripts/supply-chain/` (bash + jq + coreutils,
no python/js tooling); their tests are in `scripts/supply-chain/tests/`.

## Triggers (operator decision, 2026-11-17)

- push to `main`, `develop` and `release/**`
- push of tags `v*`
- weekly cron (Monday 04:23 UTC), so new advisories show up when no code changes
- `workflow_dispatch` (input `codemirror_rebuild` turns on the opt-in rebuild job)
- `workflow_call`, so `release.yml` can call it (input `attest`, default `true`)

There is **no `pull_request` trigger** (and never `pull_request_target`). Earlier versions of this
workflow ran on every PR; that trigger was removed on purpose.

**Consequence (accepted by the operator):** cargo audit / deny / vet and the CVE scans no longer gate
PRs. An unaudited crate, a disallowed license or a vulnerable dependency is caught only **after** it
has landed on `develop` (the push run turns red), not before the merge. Review new dependencies in
the PR by hand and fix forward on develop.

`release.yml` branches named `v<ver>-release` are not matched by `release/**`; releases reach this
workflow through `workflow_call` instead. The concurrency group includes `github.workflow` (the
caller's name under `workflow_call`), so a tag push and a `release.yml` call on the same tag do not
cancel each other.

## Hardening

- Top-level `permissions: contents: read`, no secrets (`github.token` only raises the GitHub API
  rate limit for `osv-install.sh`), `persist-credentials: false`, no caches.
- Only `attest-sbom` gets `id-token: write` + `attestations: write`, at job level (same pattern as
  `release.yml`).
- Every action is pinned by full commit SHA, the Rust toolchain to an exact release, node and
  esbuild (rebuild job only) by sha256/sha512. Pins are at least 6 weeks old.
- actionlint: `.github/actionlint.yaml` lists `ubuntu-26.04` / `ubuntu-26.04-arm`. Those are
  GitHub-hosted labels that actionlint 1.7.12's built-in table predates. They are not self-hosted
  runners; the file is just how actionlint is told about labels it does not know.

### Exception: the scanners are not pinned

cargo-audit, cargo-deny and cargo-vet (operator decision 2026-11-15) and **osv-scanner** (operator
2026-11-17, #189 item 3: "dont pin scanners in workflow but everything else") always run their
latest release. Everything else stays pinned.

- cargo tools: `cargo install --locked <tool>`. Each job prints a `resolved version` step.
- osv-scanner: `scripts/supply-chain/osv-install.sh` makes one call to the GitHub
  `releases/latest` API and refuses prereleases and drafts. It downloads the linux-amd64 asset,
  checks it with `sha256sum -c` against the asset `digest` from the API, and fails closed if
  there is no digest (same pattern as `opengrep.yml`). It logs the tag, sha256 and `--version`.

Why: these are vulnerability scanners. Advisory DBs and formats keep changing, so an old release
can fail to parse new advisories, or skip them, and the check stops protecting anything. The blast
radius is bounded: the workflow has `contents: read`, no secrets, no caches and no privileged
trigger. A compromised scanner release could make a run pass or fail falsely, but it can't push
code or read secrets.

The cost: runs can't be reproduced by tool version, and a broken new release can turn CI red with
no change here. Check the `resolved version` step first. The exception also overrides the 6-week
rule. At the time of writing osv-scanner resolved to **v2.7.1**
(sha256 `fb7dab19…6ee8`), built 2026-11-09, so it is less than 6 weeks old. That is allowed by the
exception and is exactly the trade-off it accepts.

## Jobs

### audit — cargo audit

`cargo audit --deny warnings` in `src-tauri/`. Checks every crate in `Cargo.lock` against the
RustSec advisory DB. It fails on vulnerabilities and, because of `--deny warnings`, also on
unmaintained, unsound and yanked crates. Ignored ids are in `src-tauri/.cargo/audit.toml` and must
match the VEX (see vex-check).

### deny — cargo deny

`cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml --locked check`:

- `advisories`: the same RustSec DB, plus a deny on yanked versions. Ignores are in `deny.toml`
  and must match `audit.toml` and the VEX.
- `sources`: only crates.io. Any other registry or git dependency fails. The vendored `glib`
  `[patch]` is a path crate and is allowed.
- `bans`: wildcard version requirements fail. Duplicate versions only warn.
- `licenses`: every dependency license must be on the allow-list. The app crate's own
  GPL-3.0-or-later is an exception for `opensidian` only.

### vet — cargo vet

`cargo vet --locked` in `src-tauri/`. Every crate version needs an audit. That can be our own
(`supply-chain/audits.toml`), an import (mozilla, google, bytecode-alliance, zcash, isrg; pinned in
`supply-chain/imports.lock`) or an exemption (`supply-chain/config.toml`, baseline 2026-11-15).
**A new crate or a version bump without an audit fails** (after landing, see Triggers).

### vendor-js-cve — vendored JS

Source of truth: `ui/vendor/vendor.lock.json`. The old per-library prose notes (one markdown file per library) were
deleted (operator 2026-11-17) and every fact they held moved into the lock: SPDX license, npm
publish date, tarball URL, `dist.integrity`, tarball sha256, files taken (tarball path + sha256 for
each) and `loaded_by`. Each upstream `LICENSE` and `SHA256SUMS` stay beside their library.

`scripts/supply-chain/vendor-verify.sh` proves the lock:

1. registry metadata for `name@version`: `dist.integrity` and `dist.tarball` must equal the lock,
   field by field and as the sha256 of the canonical record `{version, integrity, tarball}` (the
   packument is a mutable index, so the per-version record is what is pinned; #189);
2. download the tarball, verify sha512, extract with `tar`, and require each vendored file to
   match the file at its recorded tarball path byte for byte (sha256);
3. check the version embedded in the bytes (pdfjsVersion, the moment header, MathJax/Prism strings);
4. fail on any file under `ui/vendor` the lock doesn't cover (only `LICENSE`, `SHA256SUMS`,
   `vendor.lock.json` allowed), and on any retired prose notes file that comes back;
5. the lock publish date must equal registry `time[version]` and be ≥ 6 weeks before the entry
   was added.

CodeMirror (`cm6.js`) is an esbuild bundle, so no upstream file matches its bytes. Its build inputs
(`package.json`, `package-lock.json`, `entry.js`, `build.sh`) are committed under
`ui/vendor/codemirror/build/`. The verify script checks every `package-lock.json` integrity
against the registry and the bundle sha256 against the lock.

Only after that passes, `vendor-sbom.sh` writes the vendor SBOM: `pkg:npm` components for mathjax,
pdfjs-dist, prismjs and moment, plus a file component for `cm6.js` that depends on the 22
shipped (non-dev) packages from the CodeMirror `package-lock.json`. esbuild is a dev dependency, is
not shipped and is excluded. The job uploads it as `sbom-part-vendor` and scans it with osv-scanner
against the VEX-derived config.

### webkit-bundle-cve — the WebKit the portable AppImage bundles

Only the **portable** AppImage bundles WebKitGTK/GTK. The slim AppImage, the .deb and the flatpak
use the system/runtime WebKit and are not covered here (the flatpak runtime is a component in the
SBOM, see sbom).

The portable AppImage is built by `packaging/ci/appimage-build.sh` from `snapshot.ubuntu.com` at
`SNAP` with every requested package pinned `pkg=version` (#189). The job runs on `ubuntu-26.04`,
installs that same pinned set from the same snapshot (`scripts/supply-chain/webkit-apt.sh`, which
refuses unless its `SNAP` and every pin equal `appimage-build.sh`'s and checks the installed
versions) and reads the truth from that dpkg database. `scripts/supply-chain/webkit-bundle-sbom.sh`:

- roots: WebKit{Web,Network,GPU}Process, the injected bundle and `libwebkit2gtk-4.1.so.0` (plus the
  app binary if `--bin` is given), found in the same dirs as `build-appimage.sh`;
- `ldd` closure, minus the exclude regex read from `build-appimage.sh`;
- `dpkg -S` → binary package → `dpkg-query` → **source** package + source version;
- one component per source package:
  `pkg:deb/ubuntu/<src>@<srcver>?arch=source&distro=ubuntu-26.04`. Binary package names and sonames
  are kept as properties;
- `libbrotli*` is replaced by `build-appimage.sh`, so it maps to `pkg:github/google/brotli@v1.2.1`;
- fails closed on an unowned library, a library `ldd` cannot resolve (`=> not found`), a missing
  helper or a non-deb distro.

`osv-fixed-gate.sh` then fails the job only on findings that have a `fixed` version for that
package/ecosystem. No-fix-yet findings are listed but don't fail the job.

Decisions and known gaps:

- **Source-package purls.** OSV keys Debian/Ubuntu advisories by source package. Scanning binary
  package names would come out falsely green.
- **Pinned apt (#189).** The scanned set is the release set: same snapshot, same `pkg=version`
  pins (`webkit-apt.sh`). Bumping WebKitGTK means bumping `SNAP` + `PKGS` in `appimage-build.sh`
  and `webkit-apt.sh` together (the script fails when they differ). Residual gap: a dependency the
  ubuntu-26.04 runner image already ships is not re-installed from the snapshot, so its version
  can differ from the release container's; the release `pkgset-<ver>-<arch>.tsv` is the exact set.
- **No `--bin`.** The CI job does not build the opensidian binary, so libraries linked only by the
  app binary (not by WebKit) are not in the closure. Pass `--bin <elf>` to close that gap.

### glib-vendor-check

`scripts/supply-chain/glib-vendor-check.sh` proves `src-tauri/vendor/glib-0.18.5` is crates.io glib
0.18.5 plus exactly one documented fix:

- the crates.io sparse index cksum for glib 0.18.5 equals the sha256 pinned in `Cargo.toml`
  (`233daaf6…0ee5`) and is not yanked (`yanked` must be `false`, else REFUSED), and the downloaded
  `.crate` matches it;
- the fetched index is a mutable file (each glib release adds a line), so the 0.18.5 *record* is
  pinned: the sha256 of its canonical JSON `{name,vers,deps,cksum,features,yanked}`
  (`INDEX_ENTRY_SHA`) must match, and the `.crate` download is `sha256sum -c` checked before it
  enters the cache (#189);
- the only file that differs between pristine and vendor is `src/variant_iter.rs`;
- pristine + `glib-fix/variantstriter.patch` (sha256 pinned) == vendor, including file modes.

The `Cargo.toml` comment at the `[patch.crates-io]` entry points at this script.

### vex-check — ignored advisories

`supply-chain/vex.cdx.json` (CycloneDX 1.6 VEX) is the **single source** for ignored advisories: each
entry has a reason (`analysis.detail`) and an `opensidian:expires` date.
`scripts/supply-chain/vex-check.sh` fails when:

- the RustSec ids in `deny.toml` `[advisories].ignore`, `src-tauri/.cargo/audit.toml` and the VEX
  (not counting resolved entries) differ;
- any expiry is in the past, or an entry has no detail.

**Resolved rule:** an entry with `analysis.state = resolved` (today RUSTSEC-2024-0429 on
`pkg:cargo/glib@0.18.5`, fixed by the vendored patch) must **not** be in either ignore list. Its
detail must name the `scripts/supply-chain/*.sh` that proves the fix (glib-vendor-check.sh).

`vex-osv-config.sh` turns the VEX into an osv-scanner config (`[[IgnoredVulns]]` with id and
aliases, `effectiveUntil` = expiry). The config is written to `$RUNNER_TEMP/osvcfg`, outside the
SBOM dir, because osv-scanner automatically loads an `osv-scanner.toml` next to the scanned file.

To ignore an advisory: add it to the VEX with reason and expiry, **and** to `deny.toml` and
`audit.toml` if it is a RustSec id. Remove it when it expires.

### sbom — merged opensidian-<ver>.cdx.json

Needs vendor-js-cve and webkit-bundle-cve (runs if they were not cancelled, so a red CVE scan
still produces an SBOM). It downloads the `sbom-part-*` artifacts and adds:

- `rust-sbom.sh`: `Cargo.lock` → `pkg:cargo` components with crates.io sha256 each. glib 0.18.5 is
  an explicit component with the `.crate` sha256 and pedigree. Only the source-less workspace root
  `opensidian` is exempt (a git/path crate merely NAMED opensidian is not). Any non-crates.io source fails
  closed. This is a superset: dev-dependencies and all targets are included.
- `flatpak-sbom.sh`: runtime `org.gnome.Platform` 49 and the brotli module.

`sbom-merge.sh` dedups by bom-ref and fails on purl/version/hash conflicts or dangling dependency
refs. It adds a root `opensidian-release@<ver>` (version from `tauri.conf.json`) and derives a
reproducible serial number from the content. Output: `opensidian-<ver>.cdx.json`, valid
CycloneDX 1.6, uploaded as artifact **`sbom`**.

The job then scans it with osv-scanner against the VEX config. It fails if any **non-deb**
package has a finding. Deb packages are gated in webkit-bundle-cve by the fixed-version rule.
Both JSON scans (here and in webkit-bundle-cve) go through `osv-json.sh`: osv-scanner must exit 0
(no findings) or 1 (findings) and its output must carry a `.results` array (and list findings when
rc=1). Any other exit (127 error, 128 no packages, a crash) or malformed output fails the step, so a
scan that did not run is never counted as "zero findings".

### gate-tests — the gates still fail on their fixtures

Runs every `scripts/supply-chain/tests/*.test.sh`. Each test runs its gate on the repo (must pass)
and on scratch copies / stubs (must fail): past-expiry and missing VEX entries, a
`pdfjs-dist@4.1.392` SBOM (CVE-2024-4367), an unlisted file in `ui/vendor`, a flipped vendored
byte, a yanked or edited glib, an `ldd` "not found", a git crate named `opensidian`, a failed or
malformed osv-scanner run. A gate that silently stops failing turns this job red.

**Formerly red, fixed:** `npm moment@2.30.1`, GHSA-4p3w-j4w9-5jqw / CVE-2026-17495 (path traversal
via a crafted non-string locale name). moment is exposed to plugins as `require("obsidian").moment`,
so `not_affected` couldn't be proven and it was never suppressed. Fixed by vendoring moment 2.31.0
(published 2026-09-15, 63 days before it was added, six-week rule), with `package/LICENSE` shipped
unmodified as `ui/vendor/moment.LICENSE`; vendor-verify proves both files against the tarball.

**chrono unmaintained:** RUSTSEC-2026-0543 (informational, no vulnerability) hit chrono 0.4.45,
which comes in transitively through serde_with (tauri-utils). It is `in_triage` in the VEX and in
both ignore lists, expiring 2027-05-15, the same as the other unmaintained transitive crates.

Local test scan (vendor + rust + flatpak): 499 scanned packages, 12 VEX ignores applied,
0 findings.

### attest-sbom

Runs on tags `v*` and when called with `attest: true` (release.yml). It uses
`actions/attest-sbom` (pinned SHA, v4.1.0) with job-level `id-token: write` +
`attestations: write`. The SBOM predicate is attested over these **subjects**: the lock inputs it
was generated from (`src-tauri/Cargo.lock`, `ui/vendor/vendor.lock.json`,
`ui/vendor/codemirror/build/package-lock.json`, the flatpak manifest). The subjects are not the
release binaries.

### codemirror-rebuild (opt-in)

Runs on the weekly cron, or on `workflow_dispatch` with `codemirror_rebuild: true`. It is the only
job with node tooling:

- node v24.21.0 LTS (2026-09-07), tarball sha256 pinned and checked with `sha256sum -c`;
- esbuild 0.28.2 (2026-08-08), pinned through the `package-lock.json` sha512 integrity
  (`npm ci --ignore-scripts`, registry.npmjs.org only);
- **network isolation:** both esbuild calls run under `sudo unshare --net` (loopback only, dropped
  back to the runner uid). The step first proves that a curl to the registry *fails* inside that
  wrapper;
- `build.sh check` (dependency publish dates ≤ 2026-09-01), then `cm6.js` sha256 must equal
  `vendor.lock.json` `bundles[codemirror].sha256`.

**Reproducibility:** tested locally (node v22.23.2, esbuild 0.28.2 from the lock, with and without
the netns wrapper), the rebuild is **byte-for-byte identical**: sha256
`19fc56d3959c27c6599d979c5f6333aecf15d19b98e1ef15758559040ca59392`, 645224 bytes. A flipped-byte
copy fails. CI confirmation on node 24.21.0 is recorded in the PR / goal progress notes.

### release.yml integration

`release.yml` has a job `supply-chain: uses: ./.github/workflows/supply-chain.yml` with
`attest: true`. Its job permissions (contents read, id-token + attestations write) are the
ceiling for the called attest-sbom job. `draft-release` needs `[appimage, sums, supply-chain]`.
It downloads the `sbom` artifact into a separate `sbom/` dir, requires exactly one
`opensidian-$V.cdx.json` that passes a CycloneDX 1.6 `jq -e` check, and attaches it next to
`SHA256SUMS` (12 assets).

**The SBOM is not listed in SHA256SUMS**, so the operator's offline gpg signature over SHA256SUMS
does not cover it. Its integrity comes from the GitHub attestation (attest-sbom) instead.

## Adding or updating a dependency

1. Change `Cargo.toml` and commit the updated `Cargo.lock`.
2. Run the checks locally with the latest tools, like CI does, **before** merging (CI only checks
   after landing):
   ```sh
   cargo install --locked cargo-vet   # likewise cargo-deny, cargo-audit
   cd src-tauri && cargo vet
   ```
3. If `cargo vet` reports an unvetted crate: import (`cargo vet` without `--locked`, commit
   `imports.lock`), audit it yourself (`cargo vet inspect` / `diff`, then `certify`), or, as a last
   resort, `cargo vet add-exemption` with a reason in the PR.
4. New license → `[licenses].allow` in `deny.toml`. Git dependency → `[sources].allow-git`, pinned
   by `rev`.
5. Ignoring an advisory → VEX entry plus `deny.toml` + `audit.toml` (see vex-check).
6. Vendored JS → update `ui/vendor/vendor.lock.json` (registry integrity, tarball URL, publish
   date ≥ 6 weeks old, per-file tarball path + sha256), then run
   `scripts/supply-chain/vendor-verify.sh`.

## Limits

- **Advisory DBs (RustSec, OSV) are not pinned.** cargo audit, cargo deny and osv-scanner fetch them fresh on every
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
- The GitHub Actions themselves are only SHA-pinned (pin-check enforces the SHA), not scanned.
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
   Set the same `SNAP` and the same pins in `scripts/supply-chain/webkit-apt.sh` (the CVE scan
   of the portable AppImage's WebKit closure); it refuses to run while they differ.
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
