#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# APPIMAGE STAGE (#189 item 1): the release binary + both AppImages, built inside a PINNED
# build environment instead of on whatever the ubuntu-26.04 runner image serves that day.
#   appimage-build.sh <opensidian-tree> <ver>        (needs podman; host arch = uname -m)
# The runner is only a host: nothing it ships ends up in an artifact.
# Container (fedport, operator 2026-11-28): Fedora 44 - packaging/ci/fedora-env.sh. The GA base image
# Fedora-Container-Base-Generic-44-1.7 from its sha256-pinned OCI archive (manifest digest checked,
# = rpm-build.sh), and EVERY installed rpm (base image packages included) pinned exactly in
# packaging/ci/fedora-pkgset.lock.tsv: name + epoch:version-release + sha256 of the Fedora-signed rpm,
# fetched from kojipkgs (immutable build outputs, never a mirror), rpm -K checked against the image's
# Fedora 44 key, installed with rpm (no repo configured) and the installed set must EQUAL the lock.
# Versions: newest F44 stable build at the lock's koji event (operator waived the 6-week age rule 2026-11-28),
# webkitgtk 2.54.3-2.fc44.
# (Was: digest-pinned ubuntu:26.04 + snapshot.ubuntu.com, webkit2gtk 2.52.6 - 27 webkit CVEs.)
# PKGS = the requested build packages (name-version-release, each must be a lock row for BOTH arches);
# everything else in the lock is their dependency closure + the base image.
# Rust 1.98.0: the dist manifest is checked against its pinned sha256, the per-arch xz_hash is
# read from that manifest AND must equal the pin below, then the tarball is checked against it.
# Writes next to the AppImages (dist/):
#   pkgset-<ver>-<arch>.tsv   every installed package: name, epoch:version-release, arch, sha256 of the
#                             signed rpm (= the lock, verified in the container)
#   libset-<ver>-<arch>.tsv   sha256 of every library in the portable AppDir (same tag -> same file)
#   sbom/opensidian-<ver>-<arch>.cdx.json (+ .sha256, parts/, logs/)   only with SBOM_TAG=<tag>: the release
#                             SBOM, generated in THIS container after the build (sbom-local.sh --release)
set -euo pipefail
T=$(readlink -f "${1:?tree}"); V=${2:?ver}
say(){ echo "[$(date +%H:%M:%S)] [appimage] $*"; }
die(){ say "FAIL: $*"; exit 1; }
PKGS="gcc-16.2.1-3.fc44 gcc-c++-16.2.1-3.fc44"
SBOM_PKGS="jq-1.8.1-2.fc44"   # = sbom-local.sh's extra tool (in the lock like everything else)
RUST=1.98.0   # = release.yml RUST_TOOLCHAIN (checked below)
RUST_MANIFEST_SHA256=3f7d139b73bbbd0004ef6e58b430831c68cdad2b1f64ee2eb35d54c09199489a
[ "$RUST" = "$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$(dirname "$0")/../../rust-toolchain.toml")" ] || { echo "rust-toolchain.toml channel != $RUST" >&2; exit 1; }
grep -q "^RUST_MANIFEST_SHA256=$RUST_MANIFEST_SHA256 " "$(dirname "$0")/../../scripts/supply-chain/rust-toolchain.sh" || { echo "rust-toolchain.sh manifest sha256 != $RUST_MANIFEST_SHA256" >&2; exit 1; }
RUST_XZ_SHA256_x86_64=ed8ee2df70909c88cbaf87a6cfa3920dac00b537de12a6abe6906641e0f5952f
RUST_XZ_SHA256_aarch64=ac9283184301aeed06ecc9f5aa4c1be7041e18a1b197b6cb6c5d162d98f566da
ARCH=$(uname -m)
case "$ARCH" in x86_64|aarch64) ;; *) die "unsupported arch $ARCH (x86_64|aarch64)";; esac
TGT=$ARCH-unknown-linux-gnu
[ -z "${RUST_TOOLCHAIN:-}" ] || [ "$RUST_TOOLCHAIN" = "$RUST" ] || die "RUST_TOOLCHAIN=$RUST_TOOLCHAIN but this script pins $RUST"
eval "RSHA=\$RUST_XZ_SHA256_$ARCH"
TOOL=$(readlink -f "${APPIMAGETOOL:?APPIMAGETOOL (sha256-checked appimagetool r13)}")
[ -x "$TOOL" ] || die "missing $TOOL"
command -v podman >/dev/null || die "no podman"
CACHE=${APPIMAGE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/opensidian-appimage}; mkdir -p "$CACHE"
CARGO_CACHE=${CARGO_CACHE:-$HOME/.cargo}; mkdir -p "$CARGO_CACHE"

# ---- rust toolchain: manifest sha256 -> per-arch hash -> tarball sha256 ----
M=$CACHE/channel-rust-$RUST.toml
curl -4 -fsSL -m 120 -o "$M" "https://static.rust-lang.org/dist/channel-rust-$RUST.toml"
echo "$RUST_MANIFEST_SHA256  $M" | sha256sum -c - || die "rust $RUST dist manifest sha256 mismatch"
MH=$(awk -v H="[pkg.rust.target.$TGT]" '$0==H{f=1;next} /^\[/{f=0} f&&/^xz_hash /{gsub(/"/,"",$3);print $3}' "$M")
MU=$(awk -v H="[pkg.rust.target.$TGT]" '$0==H{f=1;next} /^\[/{f=0} f&&/^xz_url /{gsub(/"/,"",$3);print $3}' "$M")
[ "$MH" = "$RSHA" ] || die "rust manifest xz_hash for $TGT is '$MH', pinned $RSHA"
RT=$CACHE/rust-$RUST-$TGT.tar.xz
[ -s "$RT" ] && echo "$RSHA  $RT" | sha256sum -c --status - || curl -4 -fsSL -m 900 -o "$RT" "$MU"
echo "$RSHA  $RT" | sha256sum -c - || die "rust $RUST $TGT tarball sha256 mismatch"
say "rust $RUST $TGT sha256 $RSHA (manifest $RUST_MANIFEST_SHA256)"
IMG=$(bash "$T/packaging/ci/fedora-env.sh" image) || die "fedora image"
LOCK=$T/packaging/ci/fedora-pkgset.lock.tsv
# every requested pin is a lock row for BOTH arches (a pin outside the lock = drift)
for p in $PKGS $SBOM_PKGS; do for la in x86_64 aarch64; do
  awk -F'\t' -v p="$p" -v a="$la" '!/^#/ && $1==a { e=$3; sub(/^[0-9]+:/,"",e); if ($2"-"e == p && $5 ~ /^[0-9a-f]{64}$/) f=1 } END{exit !f}' "$LOCK" || die "$p ($la) not in $LOCK with a sha256"
done; done
say "tree $T  ver $V  arch $ARCH  image $IMG (Fedora-Container-Base-Generic-44-1.7)  lock $(sha256sum "$LOCK" | cut -d' ' -f1)"
RPMS=${FEDORA_RPM_CACHE:-$CACHE/rpms-$ARCH}
bash "$T/packaging/ci/fedora-env.sh" fetch "$RPMS"
mkdir -p "$T/dist"
# SBOM_TAG=<tag> (release.yml): also write dist/sbom/opensidian-<ver>-<arch>.cdx.json in the container.
# commit / dirty / epoch come from the host checkout (the container has no git).
SBOM_TAG=${SBOM_TAG:-} SBOM_COMMIT= SBOM_DIRTY= SBOM_EPOCH=
if [ -n "$SBOM_TAG" ]; then
  SBOM_COMMIT=$(git -C "$T" rev-parse HEAD) SBOM_EPOCH=$(git -C "$T" log -1 --format=%ct)
  SBOM_DIRTY=false; [ -z "$(git -C "$T" status --porcelain --untracked-files=no)" ] || SBOM_DIRTY=true
fi
podman run --rm --network=host -e SBOM_TAG="$SBOM_TAG" -e SBOM_COMMIT="$SBOM_COMMIT" -e SBOM_DIRTY="$SBOM_DIRTY" -e SBOM_EPOCH="$SBOM_EPOCH" -e SBOM_PKGS="$SBOM_PKGS" -e V="$V" -e ARCH="$ARCH" -e TGT="$TGT" -e RUST="$RUST" -e PKGS="$PKGS" -e IMGREF="$IMG" \
  -v "$RPMS":/rpms:ro -v "$T":/w -v "$RT":/rust.tar.xz:ro -v "$TOOL":/appimagetool:ro -v "$CARGO_CACHE":/cargo-home -w /w "$IMG" bash -euo pipefail -c '
  say(){ echo "[$(date +%H:%M:%S)] [appimage/fc44] $*"; }
  say "$(cat /etc/fedora-release)"
  # the pinned set: sha256 + rpm -K + rpm -U, then installed == lock (fedora-env.sh install)
  O=/w/dist/pkgset-$V-$ARCH.tsv
  bash packaging/ci/fedora-env.sh install /rpms "$O" 2>&1 | tee /w/dist/rpm-$ARCH.log
  for p in $PKGS $SBOM_PKGS; do n=${p%-*}; n=${n%-*}; got=$(rpm -q --qf "%{NAME}-%{VERSION}-%{RELEASE}" "$n"); [ "$got" = "$p" ] || { say "FAIL: $n is $got, pinned $p"; exit 1; }; done
  say "pkgset: $(grep -vc "^#" $O) packages, sha256 $(sha256sum $O | cut -d" " -f1)"
  rpm -q webkit2gtk4.1 javascriptcoregtk4.1 gtk3 glib2 openssl-libs libsoup3 glibc | sed "s/^/    /"
  mkdir /tmp/rust && tar -xJf /rust.tar.xz -C /tmp/rust --strip-components=1
  /tmp/rust/install.sh --prefix=/opt/rust --components=rustc,cargo,rust-std-$TGT >/dev/null
  export PATH=/opt/rust/bin:$PATH CARGO_HOME=/cargo-home
  rustc --version | grep -q "^rustc $RUST " || { say "FAIL: $(rustc --version) != $RUST"; exit 1; }
  rustc -vV | grep -E "^host:"
  cd /w/src-tauri
  # ui/ is not a cargo input (release.sh): force our crate to re-embed it.
  cargo clean -p opensidian --release
  cargo build --release --locked
  B=target/release/opensidian; ls -l $B; sha256sum $B
  case "$ARCH" in x86_64) W="x86-64" ;; aarch64) W="ARM aarch64" ;; esac
  file -b $B; file -b $B | grep -q "$W" || { say "FAIL: binary is not $W"; exit 1; }
  echo "glibc floor ($ARCH): $(objdump -T $B | grep -oE "GLIBC_[0-9.]+" | sort -uV | tail -1)"; ldd --version | head -1
  cd /w
  export APPIMAGETOOL=/appimagetool APPIMAGE_EXTRACT_AND_RUN=1
  for F in slim portable; do bash packaging/ci/build-appimage.sh "$F" "$V"; done
  L=/w/dist/libset-$V-$ARCH.tsv
  (cd dist/AppDir-portable && find usr/lib -type f | LC_ALL=C sort | xargs sha256sum) > $L
  say "libset: $(wc -l < $L) files, sha256 $(sha256sum $L | cut -d" " -f1)"
  # which rpm the BUNDLED webkit came from (rpm -qf of the source file; strings of the AppDir copy)
  say "bundled libwebkit2gtk-4.1.so.0 <- $(rpm -qf $(readlink -f /usr/lib64/libwebkit2gtk-4.1.so.0))"
  # ---- SBOM, IN THIS SAME CONTAINER, from the binary just built (G1/G6) ----
  # The rpm database here IS the one the portable AppImage was bundled from (jq is in the lock).
  if [ -n "$SBOM_TAG" ]; then
    say "SBOM: sbom-local.sh --release in the build container ($SBOM_TAG, $ARCH, commit $SBOM_COMMIT dirty=$SBOM_DIRTY)"
    rm -rf /w/dist/sbom
    SBOM_COMMIT="$SBOM_COMMIT" SBOM_DIRTY="$SBOM_DIRTY" SOURCE_DATE_EPOCH="$SBOM_EPOCH" \
      bash scripts/supply-chain/sbom-local.sh --release --webkit native -o /w/dist/sbom -v "$V" -t "$SBOM_TAG" --arch "$ARCH" \
        --bin /w/src-tauri/target/release/opensidian --pkgset "$O"
    ls -l /w/dist/sbom; cat /w/dist/sbom/opensidian-$V-$ARCH.cdx.json.sha256
  fi
'
if [ -n "${SBOM_TAG:-}" ]; then ls -l "$T/dist/sbom/opensidian-$V-$ARCH.cdx.json" "$T/dist/sbom/opensidian-$V-$ARCH.cdx.json.sha256"; fi
ls -l "$T/dist/opensidian-$V-$ARCH-slim.AppImage" "$T/dist/opensidian-$V-$ARCH-portable.AppImage" "$T/dist/pkgset-$V-$ARCH.tsv" "$T/dist/libset-$V-$ARCH.tsv"
