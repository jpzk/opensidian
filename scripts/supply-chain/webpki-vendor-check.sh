#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# webpki-vendor-check.sh [repo-root] — prove src-tauri/vendor/rustls-webpki-0.103.15 is the
# crates.io rustls-webpki 0.103.15 .crate byte-for-byte + EXACTLY
# scripts/supply-chain/webpki-noring/noring.patch (Cargo.toml only).
#
# Why (sync OpenSSL swap, 2026-11-19, docs/sync/deps.md §TLS-OpenSSL): the TLS stack is rustls on
# the OpenSSL 3 FIPS provider (rustls-openssl); webpki verifies with the provider's algorithms
# and never uses its own crypto backends. But webpki's `alloc` feature carries the weak feature
# `ring?/alloc`, and Cargo writes weakly-referenced optional deps into Cargo.lock even when
# nothing enables them (rust-lang/cargo#10801), so an unused `ring` (and its build graph)
# showed up in the lock. The patch drops the optional ring dependency and the weak
# reference and keeps `ring = []` declared so cfg(feature = "ring") stays known and off.
# No source file is changed. Same proof pattern as glib-vendor-check.sh.
# Needs: bash, curl, jq, tar, sha256sum, git. No cargo. Network: index/static.crates.io only.
# Output: last line "WEBPKI-VENDOR OK ..." (rc 0) or "WEBPKI-VENDOR REFUSED <why>" (rc 1).
set -uo pipefail
H=$(cd "$(dirname "$0")" && pwd)
T=${1:-$(cd "$H/../.." && pwd)}
T=$(cd "$T" 2>/dev/null && pwd) || { echo "WEBPKI-VENDOR REFUSED no tree ${1:-}"; exit 1; }
N=rustls-webpki; VER=0.103.15
CRATE_SHA=f3c3cf1d8b1e7d4927e2d154c3fcb02979afb9939629c62cd9048d4f07b60ac2
PATCH=$H/webpki-noring/noring.patch
PATCH_SHA=022975c760e338eaf9dd670f50a31fd04e0313a6765b507b399e5f460320a8b9
FIXED_FILE=Cargo.toml
INDEX_URL=${WEBPKI_INDEX_URL:-https://index.crates.io/ru/st/$N}
CRATE_URL=https://static.crates.io/crates/$N/$N-$VER.crate
V=$T/src-tauri/vendor/$N-$VER
refuse() { echo "WEBPKI-VENDOR REFUSED $*"; exit 1; }
sha() { sha256sum "$1" | cut -d' ' -f1; }

[ -d "$V" ] || refuse "no vendor dir $V"
grep -A3 '^\[patch.crates-io\]' "$T/src-tauri/Cargo.toml" | grep -q "^$N = { path = \"vendor/$N-$VER\" }" \
  || refuse "src-tauri/Cargo.toml has no [patch.crates-io] $N = { path = \"vendor/$N-$VER\" }"
[ "$(sha "$PATCH")" = "$PATCH_SHA" ] || refuse "patch $PATCH does not hash to pinned $PATCH_SHA"
grep -q '^name = "ring"$' "$T/src-tauri/Cargo.lock" && refuse "Cargo.lock still lists ring"

W=$(mktemp -d "${TMPDIR:-/tmp}/webpkivc.XXXXXX") || refuse "mktemp"
trap 'rm -rf "$W"' EXIT

curl -4 -fsSL -m 60 -o "$W/index" "$INDEX_URL" || refuse "index fetch failed: $INDEX_URL"
IX=$(jq -r "select(.vers==\"$VER\") | \"\(.cksum) \(.yanked)\"" "$W/index" | head -1)
[ -n "$IX" ] || refuse "$N $VER not in crates.io index"
[ "${IX% *}" = "$CRATE_SHA" ] || refuse "crates.io index cksum ${IX% *} != pinned $CRATE_SHA"
[ "${IX#* }" = "false" ] || refuse "$N $VER is yanked on crates.io"
echo "[webpki-vendor] crates.io index cksum ${IX% *} (== pinned), yanked=${IX#* }"

C=${WEBPKI_CRATE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/webpki-vendor-check}/$N-$VER.crate
mkdir -p "$(dirname "$C")" || refuse "cache dir"
if [ ! -s "$C" ] || [ "$(sha "$C")" != "$CRATE_SHA" ]; then
  curl -4 -fsSL -m 120 -o "$C.part" "$CRATE_URL" || refuse "download failed: $CRATE_URL"
  mv "$C.part" "$C"
fi
GOT=$(sha "$C")
[ "$GOT" = "$CRATE_SHA" ] || refuse "crate sha256 $GOT != pinned $CRATE_SHA"
echo "[webpki-vendor] crate sha256 $GOT"
tar -xzf "$C" -C "$W" || refuse "untar"
[ -d "$W/$N-$VER" ] || refuse "crate has no $N-$VER/"
mkdir "$W/p" && cp -a "$W/$N-$VER" "$W/p/" || refuse "copy"

CH=$(cd "$W" && git diff --no-index --name-only "$N-$VER" "$V" 2>/dev/null | sed "s#.*$N-$VER/##" | sort -u)
[ "$CH" = "$FIXED_FILE" ] || refuse "vendor differs from pristine in [$(echo $CH)], expected exactly $FIXED_FILE"
( cd "$W/p/$N-$VER" && GIT_DIR=/nonexistent git apply -p1 "$PATCH" ) || refuse "pinned patch does not apply to pristine $VER"
if ! git diff --no-index --quiet "$W/p/$N-$VER" "$V"; then
  git diff --no-index --stat "$W/p/$N-$VER" "$V" | tail -5
  refuse "vendor != pristine + noring.patch (extra or missing changes)"
fi
echo "[webpki-vendor] pristine..vendor ($FIXED_FILE):"
git diff --no-index "$W/$N-$VER/$FIXED_FILE" "$V/$FIXED_FILE" | grep '^[-+] \|^[-+]\[\|^[-+]#\|^[-+]ring\|^[-+]version\|^[-+]optional\|^[-+]default'
echo "WEBPKI-VENDOR OK $N-$VER (sha256 $CRATE_SHA, crates.io index) + noring.patch (sha256 $PATCH_SHA), nothing else"
