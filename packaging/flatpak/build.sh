#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Build the rustidian flatpak: generate the offline cargo sources, then build.
#
#   packaging/flatpak/build.sh
#
# Everything is written under dist/flatpak (already gitignored): the generated
# cargo-sources.json, build/ (the sandbox build dir), state/ (flatpak-builder's
# cache — reuse it and the rebuild is warm, see E2) and repo/ (the ostree repo
# the bundle is exported from). The path is FIXED, not an argument: the
# manifest includes ../../dist/flatpak/cargo-sources.json by name, and a
# BUILDROOT that did not match silently produced a module with NO sources at
# all (see below), so the two must not be able to disagree.
#
# The build sandbox gets NO network: flatpak-builder only shares it when a
# module asks with --share=network, and none does. Sources are downloaded in a
# separate phase before the build (--download-only), exactly as Flathub does.
set -eu
cd "$(dirname "$0")/../.."
ROOT=dist/flatpak
MAN=packaging/flatpak/dev.koto.rustidian.yml
mkdir -p "$ROOT"
echo "[$(date +%H:%M:%S)] 1/5 cargo sources from src-tauri/Cargo.lock"
packaging/flatpak/gen-cargo-sources.sh > "$ROOT/cargo-sources.json"

# PREFLIGHT — flatpak-builder does NOT fail on an unreadable sources include.
# Measured 2026-09-19 at ab8b04a: with the include missing it printed
#   Can't open .../cargo-sources.json
#   Json-WARNING: Failed to deserialize "sources" property ...
# on stderr, dropped EVERY source (the module's own `dir` source included) and
# went on to build an empty directory, failing 200 lines later with the
# misleading "manifest path src-tauri/Cargo.toml does not exist". A warning
# that costs a full build to diagnose is a failure; turn it into one here.
n=$(flatpak-builder --show-manifest "$MAN" | grep -c '"dest": "cargo/vendor/') || true
[ "$n" -ge 400 ] || { echo "PREFLIGHT FAILED: flatpak-builder sees $n vendored crates in $MAN (expected >=400) — the sources include did not load"; exit 4; }
echo "[$(date +%H:%M:%S)] preflight: flatpak-builder sees $n vendored crate sources"

echo "[$(date +%H:%M:%S)] 2/5 fetching sources (network; the build phase has none)"
flatpak-builder --download-only --state-dir="$ROOT/state" "$ROOT/build" "$MAN"
echo "[$(date +%H:%M:%S)] 3/5 building (offline)"
flatpak-builder --disable-download --disable-updates --force-clean \
  --state-dir="$ROOT/state" --repo="$ROOT/repo" "$ROOT/build" "$MAN"
echo "[$(date +%H:%M:%S)] 4/5 exporting bundle"
flatpak build-bundle "$ROOT/repo" "$ROOT/rustidian.flatpak" dev.koto.rustidian master
ls -l "$ROOT/rustidian.flatpak"
echo "[$(date +%H:%M:%S)] 5/5 done"
