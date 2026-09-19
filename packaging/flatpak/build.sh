#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Build the rustidian flatpak: generate the offline cargo sources, then build.
#
#   packaging/flatpak/build.sh [BUILDROOT]
#
# BUILDROOT (default: ./dist/flatpak) holds build/ (the sandbox build dir),
# state/ (flatpak-builder's cache — reuse it and the rebuild is warm, see E2)
# and repo/ (the ostree repo the bundle is exported from). Nothing is written
# outside BUILDROOT (cargo-sources.json is generated there too).
#
# The build sandbox gets NO network: flatpak-builder only shares it when a
# module asks with --share=network, and none does. Sources are downloaded in a
# separate phase before the build (--download-only), exactly as Flathub does.
set -eu
cd "$(dirname "$0")/../.."
ROOT=${1:-dist/flatpak}
MAN=packaging/flatpak/dev.koto.rustidian.yml
mkdir -p "$ROOT"
echo "[$(date +%H:%M:%S)] 1/4 cargo sources from src-tauri/Cargo.lock"
packaging/flatpak/gen-cargo-sources.sh > "$ROOT/cargo-sources.json"
echo "[$(date +%H:%M:%S)] 2/4 fetching sources (network; build phase has none)"
flatpak-builder --download-only --state-dir="$ROOT/state" "$ROOT/build" "$MAN"
echo "[$(date +%H:%M:%S)] 3/4 building (offline)"
flatpak-builder --disable-download --disable-updates --force-clean \
  --state-dir="$ROOT/state" --repo="$ROOT/repo" "$ROOT/build" "$MAN"
echo "[$(date +%H:%M:%S)] 4/4 exporting bundle"
flatpak build-bundle "$ROOT/repo" "$ROOT/rustidian.flatpak" dev.koto.rustidian master
ls -l "$ROOT/rustidian.flatpak"
echo "[$(date +%H:%M:%S)] done"
