#!/bin/bash
# rust-toolchain.sh (#189 item 5): install the Rust toolchain named in rust-toolchain.toml,
# pinned by the sha256 of its dist manifest.
#   1. download https://static.rust-lang.org/dist/channel-rust-<channel>.toml
#   2. sha256 must be RUST_MANIFEST_SHA256 (the .sha256 next to it on the server is not a pin)
#   3. rustup toolchain install <channel> (rustup checks each component against that manifest)
#   4. the installed rustc commit-hash must be the manifest's [pkg.rustc] git_commit_hash
# Also used by packaging/ci/appimage-build.sh and the build box, which carry the same sha256.
set -euo pipefail
RUST_MANIFEST_SHA256=3f7d139b73bbbd0004ef6e58b430831c68cdad2b1f64ee2eb35d54c09199489a  # 1.98.0, dist date 2026-08-20
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
die() { echo "[rust-toolchain] FAIL: $*" >&2; exit 1; }
say() { echo "[rust-toolchain] $*"; }
CH=$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$ROOT/rust-toolchain.toml")
printf '%s' "$CH" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "rust-toolchain.toml channel '$CH' is not an exact x.y.z version"
[ -z "${RUST_TOOLCHAIN:-}" ] || [ "$RUST_TOOLCHAIN" = "$CH" ] || die "RUST_TOOLCHAIN=$RUST_TOOLCHAIN but rust-toolchain.toml pins $CH"
M=$(mktemp); trap 'rm -f "$M"' EXIT
curl -4 -fsSL --proto '=https' -m 120 -o "$M" "https://static.rust-lang.org/dist/channel-rust-$CH.toml"
echo "$RUST_MANIFEST_SHA256  $M" | sha256sum -c --status - || die "channel-rust-$CH.toml sha256 $(sha256sum "$M" | cut -c1-64) != pinned $RUST_MANIFEST_SHA256"
D=$(sed -n 's/^date = "\(.*\)"/\1/p' "$M")
GC=$(awk '$0=="[pkg.rustc]"{f=1;next} /^\[/{f=0} f&&/^git_commit_hash /{gsub(/"/,"",$3);print $3}' "$M")
printf '%s' "$GC" | grep -Eq '^[0-9a-f]{40}$' || die "no [pkg.rustc] git_commit_hash in the manifest"
say "manifest channel-rust-$CH.toml sha256 $RUST_MANIFEST_SHA256 date $D rustc $GC"
[ "${1:-}" = "--manifest-only" ] && exit 0
COMP=$(sed -n 's/^components *= *\[\(.*\)\].*/\1/p' "$ROOT/rust-toolchain.toml" | tr -d '" ' | tr ',' ' ')
A=(); for c in $COMP; do A+=(--component "$c"); done
rustup toolchain install "$CH" --profile minimal --no-self-update "${A[@]}"
rustup default "$CH"
IC=$(rustc "+$CH" -vV | sed -n 's/^commit-hash: //p')
[ "$IC" = "$GC" ] || die "installed rustc $CH commit $IC != manifest $GC"
say "PINNED rust $CH rustc $IC (manifest sha256 $RUST_MANIFEST_SHA256)"
cargo "+$CH" --version
