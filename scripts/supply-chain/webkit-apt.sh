#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# webkit-apt.sh — the pinned WebKit build set of packaging/ci/appimage-build.sh on an Ubuntu 26.04 HOST (#189).
# (No longer used by CI: the release WebKit SBOM + CVE gate run INSIDE the build container, release.yml;
#  kept for local native sbom-local.sh runs and for --check, which sbom-local.sh calls.)
# The job scans the WebKitGTK/GTK closure the portable AppImage bundles. That AppImage is built by
# packaging/ci/appimage-build.sh from snapshot.ubuntu.com $SNAP with pkg=version pins, so this
# script installs the SAME set on the ubuntu-26.04 runner from the SAME dated snapshot:
#   - runner apt reads ONLY https://snapshot.ubuntu.com/ubuntu/$SNAP/ (resolute, -updates,
#     -security; main universe), archive signatures still checked (ubuntu-archive-keyring);
#   - every package is pkg=version; the script refuses unless SNAP and every pin equal
#     appimage-build.sh's SNAP / PKGS (so the scan cannot drift from what ships);
#   - after the install every pinned package's installed version is checked (dpkg-query).
# --allow-downgrades: the runner image may carry newer libs than the snapshot (e.g. libssl3);
# the pins win, the scan sees exactly the snapshot closure.
# Usage: scripts/supply-chain/webkit-apt.sh            (needs sudo, network to snapshot.ubuntu.com)
#        scripts/supply-chain/webkit-apt.sh --check    (pins == appimage-build.sh only; no sudo/network)
set -euo pipefail
say(){ echo "[$(date +%H:%M:%S)] [webkit-apt] $*"; }
die(){ say "FAIL: $*"; exit 1; }
H=$(cd "$(dirname "$0")" && pwd)
AB=${APPIMAGE_BUILD:-$H/../../packaging/ci/appimage-build.sh}
SNAP=20261123T150000Z        # = packaging/ci/appimage-build.sh SNAP
# = release AppImage build set (appimage-build.sh PKGS), versions read from the snapshot indexes
# (resolute-updates > -security > release) for amd64 at $SNAP:
PKGS="build-essential=12.12ubuntu2.26.04.2 pkg-config=2.5.1-4 libssl-dev=3.5.5-1ubuntu3.7 libwebkit2gtk-4.1-dev=2.52.6-0ubuntu0.26.04.2 libwebkit2gtk-4.1-0=2.52.6-0ubuntu0.26.04.2 libjavascriptcoregtk-4.1-0=2.52.6-0ubuntu0.26.04.2 libgtk-3-dev=3.24.52-0ubuntu1 libayatana-appindicator3-dev=0.5.94-1build1 librsvg2-dev=2.61.3+dfsg-3ubuntu0.1 patchelf=0.18.0-1.4build1 file=1:5.46-5build2"

[ -f "$AB" ] || die "no $AB"
ABSNAP=$(sed -n 's/^SNAP=\([0-9TZ]*\).*/\1/p' "$AB" | head -1)
[ "$ABSNAP" = "$SNAP" ] || die "SNAP $SNAP != appimage-build.sh SNAP '$ABSNAP'"
ABPKGS=" $(sed -n 's/^PKGS="\([^"]*\)".*/\1/p' "$AB" | head -1) "
[ "$ABPKGS" != "  " ] || die "no PKGS= in $AB"
for p in $PKGS; do
  case "$p" in *=?*) ;; *) die "$p is not pkg=version" ;; esac
  case "$ABPKGS" in *" $p "*) ;; *) die "$p is not in appimage-build.sh PKGS (the scan must equal the bundle)" ;; esac
done
say "pins == appimage-build.sh (SNAP $SNAP, $(echo $PKGS | wc -w) packages)"
[ "${1:-}" = --check ] && exit 0

say "runner apt -> snapshot.ubuntu.com $SNAP only"
sudo mkdir -p /etc/apt/sources.list.d.runner
sudo find /etc/apt/sources.list.d -mindepth 1 -maxdepth 1 -exec mv -t /etc/apt/sources.list.d.runner {} +
[ ! -e /etc/apt/sources.list ] || sudo mv /etc/apt/sources.list /etc/apt/sources.list.d.runner/
printf 'Types: deb\nURIs: https://snapshot.ubuntu.com/ubuntu/%s/\nSuites: resolute resolute-updates resolute-security\nComponents: main universe\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\nCheck-Valid-Until: no\n' "$SNAP" \
  | sudo tee /etc/apt/sources.list.d/snapshot.sources
L=$(mktemp)
sudo apt-get -q update > "$L" 2>&1 || { cat "$L"; die "apt update"; }
cat "$L"
if grep -E "^(Err:|W:|E:|Error:|Warning:)" "$L"; then die "apt update against the snapshot was not clean"; fi
sudo apt-get install -y -q --no-install-recommends --allow-downgrades $PKGS
for p in $PKGS; do
  got=$(dpkg-query -W -f='${Version}' "${p%%=*}") || die "${p%%=*} not installed"
  [ "$got" = "${p#*=}" ] || die "${p%%=*} is $got, pinned ${p#*=}"
done
say "PINNED apt $PKGS"
