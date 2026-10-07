#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Renders every committed launcher/window icon PNG from ONE file, the official logo
# packaging/linux/opensidian.svg. Run from anywhere; writes into the repo:
#   src-tauri/icons/icon.png            128x128 RGBA (window/taskbar icon tauri
#                                        compiles into the binary; icons/icon.png is
#                                        tauri-codegen's default_window_icon on Linux)
#   packaging/linux/icons/{48,64,128,256,512}.png   hicolor launcher icons
# Swapping the logo = replace opensidian.svg, re-run this, commit the outputs.
#
# PINNED RENDERER: rsvg-convert from librsvg2-tools-2.62.0-1.fc44, installed from the
# frozen Fedora 44 GA repo (dnf --repo=fedora) inside the Fedora 44 GA base image,
# pinned by digest (released 2026-04-22):
#   fedora:44@sha256:f1e66cdd6eff2c9ccad192f8865af9be6d69b46b3f13329d2975a2d61a1296c5
#   from Fedora-Container-Base-Generic-44-1.7.x86_64.oci.tar.xz
#   (file sha256 75200f5752a74a21a616ca9a75e25beb594e2e117a0195c54f87c0b3e3974d1b)
# Same svg + same renderer = same bytes; a re-run must leave `git status` clean.
# Needs podman. The image is loaded from the checksummed OCI archive if absent.
set -euo pipefail
R=$(cd "$(dirname "$(readlink -f "$0")")/../.." && pwd)
SVG_SHA=a09ac826e7354805cfa8c02d50718a5366bffccd5edd1c86fb6e9005ca779736
RSVG=librsvg2-tools-2.62.0-1.fc44
DIGEST=sha256:f1e66cdd6eff2c9ccad192f8865af9be6d69b46b3f13329d2975a2d61a1296c5
IMG=localhost/fedora@$DIGEST
OCI_URL=https://dl.fedoraproject.org/pub/fedora/linux/releases/44/Container/x86_64/images/Fedora-Container-Base-Generic-44-1.7.x86_64.oci.tar.xz
OCI_SHA=75200f5752a74a21a616ca9a75e25beb594e2e117a0195c54f87c0b3e3974d1b
say(){ echo "[render-icons] $*"; }
S=$R/packaging/linux/opensidian.svg
got=$(sha256sum "$S" | cut -d' ' -f1)
say "svg $got"
[ "$got" = "$SVG_SHA" ] || say "NOTE: svg is not the v0.4 official logo ($SVG_SHA) — update SVG_SHA when swapping on purpose"
command -v podman >/dev/null || { say "FAIL: needs podman"; exit 1; }
if ! podman image exists "$IMG"; then
  C=${XDG_CACHE_HOME:-$HOME/.cache}/opensidian-fedora44; mkdir -p "$C"; A=$C/${OCI_URL##*/}
  [ -s "$A" ] || curl -4 -fsSL -o "$A" "$OCI_URL"
  echo "$OCI_SHA  $A" | sha256sum -c - >/dev/null || { say "FAIL: OCI archive sha256 mismatch"; exit 1; }
  rm -rf "$C/oci" && mkdir "$C/oci" && tar xJf "$A" -C "$C/oci"
  podman tag "$(podman pull -q "oci:$C/oci")" localhost/fedora:44
  podman image exists "$IMG" || { say "FAIL: loaded image does not carry $DIGEST"; exit 1; }
fi
mkdir -p "$R/packaging/linux/icons"
podman run --rm --network=host -v "$R:/r:z" "$IMG" bash -euo pipefail -c '
  dnf -y -q --repo=fedora --setopt=install_weak_deps=False install '"$RSVG"' >/tmp/dnf.log 2>&1 || { tail -20 /tmp/dnf.log; exit 1; }
  echo "[render-icons] renderer $(rpm -q librsvg2-tools) ($(rsvg-convert --version))"
  r(){ rsvg-convert -w $1 -h $1 -f png -o "$2" /r/packaging/linux/opensidian.svg; }
  for s in 48 64 128 256 512; do r $s /r/packaging/linux/icons/$s.png; done
  r 128 /r/src-tauri/icons/icon.png
'
cd "$R"
for f in packaging/linux/icons/{48,64,128,256,512}.png src-tauri/icons/icon.png; do
  say "$(sha256sum "$f" | cut -c1-64)  $f  $(file -b "$f" | cut -d, -f2,3,4 | tr -d ' ' )"
done
