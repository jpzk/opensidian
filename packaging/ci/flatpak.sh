#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# CI: build the release flatpak on a GitHub-hosted ubuntu-24.04 runner.
#   packaging/ci/flatpak.sh <version>   -> dist/opensidian-<v>-x86_64.flatpak
# Same build as harness bin/hz/flatpak-build.sh (packaging/flatpak/build.sh, unchanged), with
# the box's cloud-init toolchain: flatpak + flatpak-builder at the cloud-init versions (apt from
# the frozen ubuntu snapshot in pins.env, so nothing newer than the cutoff is installed), and
# the GNOME 49 + rust-stable//25.08 runtimes installed per-user and moved to PINNED COMMITS.
# Needs sudo for apt only (GitHub-hosted runners have passwordless sudo).
set -euo pipefail
V=${1:?version}; cd "$(dirname "$0")/../.."
. packaging/ci/pins.env
say(){ echo "[$(date +%H:%M:%S)] [flatpak] $*"; }
die(){ say "FAIL: $*"; exit 1; }
BASEV=${V%%-*}
if grep -q "<release version=\"$V\"" packaging/flatpak/dev.koto.opensidian.metainfo.xml; then :
elif [ "$BASEV" != "$V" ]; then say "WARNING: prerelease $V — metainfo <release> not bumped (allowed only for a -suffixed tag)"
else die "metainfo <release> is not $V"; fi
# elfutils: flatpak-builder Recommends it (eu-strip/eu-elfcompress) and fails without it under --no-install-recommends.
# apt from the snapshot ONLY: a sources file of its own, used with the stock lists disabled for this call
SL=$(mktemp); trap 'rm -f "$SL"' EXIT
for s in noble noble-updates noble-security; do printf 'deb [check-valid-until=no] https://snapshot.ubuntu.com/ubuntu/%s %s main universe\n' "$UBUNTU_SNAPSHOT" "$s"; done > "$SL"
AOPT=(-o Dir::Etc::sourcelist="$SL" -o Dir::Etc::sourceparts=/nonexistent -o Acquire::Check-Valid-Until=false)
say "apt (snapshot $UBUNTU_SNAPSHOT): flatpak=$FLATPAK_VER flatpak-builder=$FLATPAK_BUILDER_VER"
{ sudo apt-get -q "${AOPT[@]}" update && sudo DEBIAN_FRONTEND=noninteractive apt-get -y -q "${AOPT[@]}" --no-install-recommends --allow-downgrades install \
    "flatpak=$FLATPAK_VER" "flatpak-builder=$FLATPAK_BUILDER_VER" elfutils; } > "${TMPDIR:-/tmp}/flatpak-apt.log" 2>&1 \
  || { tail -30 "${TMPDIR:-/tmp}/flatpak-apt.log"; die "apt"; }
say "$(flatpak --version), $(flatpak-builder --version), elfutils $(dpkg-query -W -f="${Version}" elfutils) (eu-strip for flatpak-builder; version frozen by the snapshot)"
flatpak --user remote-add --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
pin(){ # <ref> <commit>
  flatpak --user install -y --noninteractive flathub "$1" >/dev/null
  flatpak --user update -y --noninteractive --commit="$2" "$1" >/dev/null
  got=$(flatpak --user info --show-commit "$1")
  [ "$got" = "$2" ] || die "$1 is at $got, pinned $2"
  say "pinned $1 @ $2 ($(flatpak --user info "$1" | sed -n 's/^ *Date: //p'))"
}
pin runtime/org.gnome.Platform/x86_64/49 "$FP_PLATFORM_COMMIT"
pin runtime/org.gnome.Sdk/x86_64/49 "$FP_SDK_COMMIT"
pin runtime/org.freedesktop.Sdk.Extension.rust-stable/x86_64/25.08 "$FP_RUST_COMMIT"
sh packaging/flatpak/build.sh 2>&1 | grep -v -E '^(Downloading|Fetching) |^\s*$' | tail -60
[ -s dist/flatpak/opensidian.flatpak ] || die "build.sh produced no bundle"
cp dist/flatpak/opensidian.flatpak "dist/opensidian-$V-x86_64.flatpak"
F=dist/opensidian-$V-x86_64.flatpak
say "$F $(stat -c %s "$F") bytes sha256 $(sha256sum "$F" | cut -d' ' -f1)"
say "FLATPAK STAGE OK"
