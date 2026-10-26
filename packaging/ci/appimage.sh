#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# CI: build the release binary ONCE and both AppImages from it, inside the pinned
# ubuntu:24.04 container (packaging/ci/pins.env). Runs on a GitHub-hosted ubuntu-24.04
# runner (podman preinstalled) — or any host with podman. Never on the coordinator VM.
#   packaging/ci/appimage.sh <version>
# -> dist/opensidian-<v>-x86_64-{slim,portable}.AppImage, src-tauri/target/release/opensidian
# Same steps as harness release.sh's build stage: cargo build --release (here --locked,
# fresh checkout so no stale-embed risk), build-appimage.sh slim + portable. The smoke /
# appimage-smoke / distro-matrix gates run on the box afterwards, on the uploaded bytes.
set -euo pipefail
V=${1:?version}; cd "$(dirname "$0")/../.."; R=$PWD
set -a; . packaging/ci/pins.env; set +a   # exported: podman -e NAME passes the value through
say(){ echo "[$(date +%H:%M:%S)] [appimage] $*"; }
command -v podman >/dev/null || { say "FAIL: no podman"; exit 1; }
CA=/etc/ssl/certs/ca-certificates.crt; [ -s $CA ] || { say "FAIL: host has no $CA"; exit 1; }
say "image $UBUNTU_IMG  apt snapshot $UBUNTU_SNAPSHOT  rust $RUST_TOOLCHAIN (rustup-init $RUSTUP_VER)"
podman pull -q "$UBUNTU_IMG" >/dev/null
mkdir -p dist
# The host CA bundle is mounted read-only only so apt/curl can speak https before the
# image has ca-certificates; apt still verifies the archive signatures (ubuntu-keyring).
podman run --rm --network=host -v "$R:/src" -v "$CA:/host-ca.crt:ro" -w /src \
  -e V="$V" -e UBUNTU_SNAPSHOT -e UBUNTU_PKGS -e RUSTUP_VER -e RUSTUP_SHA256 -e RUST_TOOLCHAIN \
  -e APPIMAGETOOL_URL -e APPIMAGETOOL_SHA256 -e CARGO_TERM_COLOR=never \
  "$UBUNTU_IMG" bash -euo pipefail -c '
  say(){ echo "[$(date +%H:%M:%S)] [appimage/noble] $*"; }
  say "$(. /etc/os-release; echo $PRETTY_NAME)"
  rm -f /etc/apt/sources.list.d/*.sources /etc/apt/sources.list.d/*.list
  S=https://snapshot.ubuntu.com/ubuntu/$UBUNTU_SNAPSHOT
  printf "Types: deb\nURIs: %s\nSuites: noble noble-updates noble-security\nComponents: main universe\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\nCheck-Valid-Until: no\n" "$S" > /etc/apt/sources.list.d/snapshot.sources
  echo "Acquire::https::CaInfo \"/host-ca.crt\";" > /etc/apt/apt.conf.d/99host-ca
  export DEBIAN_FRONTEND=noninteractive
  { apt-get -q update && apt-get -y -q --no-install-recommends install $UBUNTU_PKGS; } > /tmp/apt.log 2>&1 || { tail -30 /tmp/apt.log; exit 1; }
  say "webkit $(dpkg-query -W -f="\${Version}" libwebkit2gtk-4.1-dev)  gtk $(dpkg-query -W -f="\${Version}" libgtk-3-dev)"
  curl -4 -fsSL -o /tmp/rustup-init "https://static.rust-lang.org/rustup/archive/$RUSTUP_VER/x86_64-unknown-linux-gnu/rustup-init"
  echo "$RUSTUP_SHA256  /tmp/rustup-init" | sha256sum -c -
  chmod +x /tmp/rustup-init
  /tmp/rustup-init -y -q --no-modify-path --profile minimal --default-toolchain "$RUST_TOOLCHAIN"
  . "$HOME/.cargo/env"
  rustc --version | grep -q " $RUST_TOOLCHAIN " || { say "REFUSING: rustc is $(rustc --version)"; exit 14; }
  say "toolchain $(rustc --version)"
  T=/opt/appimagetool-x86_64.AppImage
  curl -4 -fsSL -o $T "$APPIMAGETOOL_URL"
  echo "$APPIMAGETOOL_SHA256  $T" | sha256sum -c -
  chmod +x $T
  git config --global --add safe.directory /src
  say "cargo build --release --locked"
  (cd src-tauri && cargo build --release --locked 2>&1 | tail -3)
  B=src-tauri/target/release/opensidian; [ -x $B ] || { say "FAIL: no binary"; exit 1; }
  say "binary sha256 $(sha256sum $B | cut -d" " -f1)"
  for F in slim portable; do
    say "appimage $F"
    APPIMAGETOOL=$T BROTLI_CACHE=/tmp/brotli bash packaging/ci/build-appimage.sh $F "$V" 2>&1 | tail -4
  done
  # the container runs as root and leaves root-owned files; hand them back to the host user
  chown -R "$(stat -c %u:%g /src)" dist src-tauri/target
'
for F in slim portable; do
  IMG=dist/opensidian-$V-x86_64-$F.AppImage
  [ -s "$IMG" ] || { say "FAIL: $IMG missing"; exit 1; }
  say "$IMG $(stat -c %s "$IMG") bytes sha256 $(sha256sum "$IMG" | cut -d' ' -f1)"
done
say "APPIMAGE STAGE OK"
