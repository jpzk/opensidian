# packaging/ci — release build environment

The Linux release (`.github/workflows/release.yml`) compiles ONE binary per arch
(x86_64, aarch64) and wraps it five ways. The GitHub runner is only a host: the
binary and both AppImages are built inside a pinned Fedora 44 container.

## Portable build env (job `appimage`)
- `appimage-build.sh <tree> <ver>` — entry point (also usable on a box with podman).
- `fedora-env.sh` — the environment, shared with `linux-test.sh` (linux.yml PR tests):
  - base: `Fedora-Container-Base-Generic-44-1.7.<arch>.oci.tar.xz`, sha256 + manifest
    digest per arch, the SAME pin as `rpm-build.sh` (checked).
  - packages: exactly the rows of `fedora-pkgset.lock.tsv` (name, epoch:version-release,
    arch, sha256, koji build). Each rpm is fetched from kojipkgs (immutable build
    store, not a mirror), checked by sha256, GPG-verified (`rpm -K`) with the
    Fedora 44 key taken from the pinned image itself, and installed offline with
    `rpm` (no dnf repo at build time). `rpm -qa` must then equal the lock.
  - the lock's own sha256 is pinned in `fedora-env.sh` (`LOCK_SHA256`); `pin-check.sh`
    (rule `rpmlock`) fails CI on any drift between lock, env and `appimage-build.sh`.
  - Rust 1.98.0 from static.rust-lang.org, sha256-checked (`rust-toolchain.sh`).
- `build-appimage.sh slim|portable` — AppDirs; probes `/usr/lib64` and
  `/usr/libexec/webkit2gtk-4.1` (Fedora) as well as the Debian multiarch paths.
- SBOM: `scripts/supply-chain/sbom-local.sh --release` inside the same container; the
  WebKit closure comes from the rpm database as
  `pkg:rpm/fedora/<name>@<version>-<release>?arch=<arch>&distro=fedora-44`.
- WebKit CVE gate: `webkit-wsa-gate.sh` (bundled webkit2gtk4.1 >= every WebKitGTK
  advisory's fixed-in, `webkit-wsa.tsv`; osv.dev cannot score Fedora purls) plus
  `osv-fixed-gate.sh` for what osv can score.

## Updating the package set
1. `scripts/supply-chain/fedora-resolve.sh <arch> ...` per arch, ONE koji event for
   both (maintainer tool; resolves the closure, splices signed rpms, `rpm -K`,
   sha256, proves the closure with `rpm --test -i` into an empty root).
2. Merge both arches into `fedora-pkgset.lock.tsv` (header records the koji event and
   date), set `LOCK_SHA256` in `fedora-env.sh`, update `appimage-build.sh` pins.
3. Project rule: every package > 6 weeks old. The current lock is an operator
   exception (2026-11-28, webkitgtk 2.54.3-2.fc44 for WSA-2026-0007); see its header.

## Wraps and smoke
- `rpm-build.sh` (Fedora 44 base) and `deb-build.sh` (debian:trixie) wrap the SAME
  binary; each refuses unless its `/usr/bin/opensidian` sha256 equals the AppImage's.
- `smoke-run.sh` + `launch-smoke.sh` (job `smoke`): PASS iff the Opensidian window maps
  and the process stays alive. The X server is Xvfb + xwininfo from the pinned lock, run
  in the pinned Fedora 44 env (`--network=host`, shared `/tmp/.X11-unix`); nothing is
  installed from a floating repo except `apt-get install ./<deb>` on stock ubuntu-26.04,
  which is the point of that test. The rpm goes in with plain `rpm -i` (no dnf): its
  Requires must be met by the pinned set. Gates: portable AppImage on ubuntu-26.04 and
  Fedora 44, deb on ubuntu-26.04, rpm on Fedora 44 (installed binary sha256 == AppImage
  binary). Info only: ubuntu-24.04, debian-12 (glibc floor, digest-pinned image).
