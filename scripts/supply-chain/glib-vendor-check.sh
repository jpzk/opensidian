#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# glib-vendor-check.sh [repo-root] — prove src-tauri/vendor/glib-0.18.5 is the crates.io
# glib 0.18.5 .crate byte-for-byte + EXACTLY scripts/supply-chain/glib-fix/variantstriter.patch.
#
# Source of truth: the crates.io sparse index (index.crates.io/gl/ib/glib, "cksum" of 0.18.5).
# It must equal the pinned sha256 below (Cargo.toml [patch.crates-io] comment), the downloaded
# .crate must hash to it, and pristine + pinned patch must be identical (contents, file set,
# modes) to the vendor dir, with pristine..vendor differing in exactly src/variant_iter.rs.
# The patch is upstream gtk-rs/gtk-rs-core b5a4071e (PR 1343, GHSA-wrw7-89jp-8q8g /
# RUSTSEC-2024-0429) with a/glib/ b/glib/ rebased to the crate root; the original commit is
# kept beside it as glib-fix/upstream-b5a4071e.patch.
# Needs: bash, curl, jq, tar, sha256sum, git (git diff --no-index / git apply; no repo needed).
# No cargo. Network: index.crates.io + static.crates.io only. Cache: $GLIB_CRATE_CACHE.
# Output: last line "GLIB-VENDOR OK ..." (rc 0) or "GLIB-VENDOR REFUSED <why>" (rc 1).
set -uo pipefail
H=$(cd "$(dirname "$0")" && pwd)
T=${1:-$(cd "$H/../.." && pwd)}
T=$(cd "$T" 2>/dev/null && pwd) || { echo "GLIB-VENDOR REFUSED no tree ${1:-}"; exit 1; }
CRATE_SHA=233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5
PATCH=$H/glib-fix/variantstriter.patch
PATCH_SHA=611b3fb246e554df98301d21cd1625ebb0a318951ad42bdf1cdf26cab69af2fe
FIXED_FILE=src/variant_iter.rs
# GLIB_INDEX_URL: test hook only (tests/glib-vendor-check.test.sh serves a doctored index via file://)
INDEX_URL=${GLIB_INDEX_URL:-https://index.crates.io/gl/ib/glib}
CRATE_URL=https://static.crates.io/crates/glib/glib-0.18.5.crate
# sha256 of the 0.18.5 index record (jq -cS {name,vers,deps,cksum,features,yanked}, yanked=false),
# read 2026-11-18. The index FILE is mutable (every new glib release appends a line), the 0.18.5
# record is not: any change to it (deps, features, cksum, yanked) fails closed here.
INDEX_ENTRY_SHA=25a5c5b4653fae1c49d0680f3a29443f45f1cd9f89b3a98cdb6d27f8d1acb2c7
V=$T/src-tauri/vendor/glib-0.18.5
refuse() { echo "GLIB-VENDOR REFUSED $*"; exit 1; }
sha() { sha256sum "$1" | cut -d' ' -f1; }

[ -d "$V" ] || refuse "no vendor dir $V"
grep -A2 '^\[patch.crates-io\]' "$T/src-tauri/Cargo.toml" | grep -q 'glib = { path = "vendor/glib-0.18.5" }' \
  || refuse "src-tauri/Cargo.toml has no [patch.crates-io] glib = { path = \"vendor/glib-0.18.5\" }"
grep -q 'glib-0.18.5 = crates.io glib 0.18.5 byte-for-byte (sha256 233daaf6' "$T/src-tauri/Cargo.toml" \
  || refuse "src-tauri/Cargo.toml glib-fix comment no longer names sha256 233daaf6…"
# A path patch drops the lock checksum; if one reappears it must be the pinned one.
LC=$(awk '/^name = "glib"$/{g=1} g&&/^checksum = /{print $3; exit} /^$/{g=0}' "$T/src-tauri/Cargo.lock" | tr -d '"')
[ -z "$LC" ] || [ "$LC" = "$CRATE_SHA" ] || refuse "Cargo.lock glib checksum $LC != pinned $CRATE_SHA"
[ "$(sha "$PATCH")" = "$PATCH_SHA" ] || refuse "patch $PATCH does not hash to pinned $PATCH_SHA"

W=$(mktemp -d "${TMPDIR:-/tmp}/glibvc.XXXXXX") || refuse "mktemp"
trap 'rm -rf "$W"' EXIT

# 1. crates.io index: checksum of glib 0.18.5 (not yanked) == pinned
curl -4 -fsSL -m 60 -o "$W/index" "$INDEX_URL" || refuse "index fetch failed: $INDEX_URL"
IX=$(jq -r 'select(.vers=="0.18.5") | "\(.cksum) \(.yanked)"' "$W/index" | head -1)
[ -n "$IX" ] || refuse "glib 0.18.5 not in crates.io index"
[ "${IX% *}" = "$CRATE_SHA" ] || refuse "crates.io index cksum ${IX% *} != pinned $CRATE_SHA"
[ "${IX#* }" = "false" ] || refuse "glib 0.18.5 is yanked on crates.io (yanked=${IX#* }): pick a non-yanked base"
jq -cS 'select(.vers=="0.18.5") | {name,vers,deps,cksum,features,yanked}' "$W/index" | head -1 >"$W/entry"
echo "$INDEX_ENTRY_SHA  $W/entry" | sha256sum -c --status - || refuse "crates.io index record of glib 0.18.5 != pinned sha256 $INDEX_ENTRY_SHA"
echo "[glib-vendor] crates.io index cksum ${IX% *} (== pinned), yanked=${IX#* }"

# 2. the .crate itself
C=${GLIB_CRATE_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/glib-vendor-check}/glib-0.18.5.crate
mkdir -p "$(dirname "$C")" || refuse "cache dir"
if [ ! -s "$C" ] || [ "$(sha "$C")" != "$CRATE_SHA" ]; then
  curl -4 -fsSL -m 120 -o "$C.part" "$CRATE_URL" || refuse "download failed: $CRATE_URL"
  echo "$CRATE_SHA  $C.part" | sha256sum -c --status - || { rm -f "$C.part"; refuse "crate sha256 != pinned $CRATE_SHA: $CRATE_URL"; }
  mv "$C.part" "$C"
fi
GOT=$(sha "$C")
[ "$GOT" = "$CRATE_SHA" ] || refuse "crate sha256 $GOT != pinned $CRATE_SHA"
echo "[glib-vendor] crate sha256 $GOT"
tar -xzf "$C" -C "$W" || refuse "untar"
[ -d "$W/glib-0.18.5" ] || refuse "crate has no glib-0.18.5/"
mkdir "$W/p" && cp -a "$W/glib-0.18.5" "$W/p/" || refuse "copy"

# 3. pristine vs vendor: changed-file set must be exactly the fixed file
CH=$(cd "$W" && git diff --no-index --name-only glib-0.18.5 "$V" 2>/dev/null \
  | sed 's#.*glib-0[.]18[.]5/##' | sort -u)
[ "$CH" = "$FIXED_FILE" ] || refuse "vendor differs from pristine in [$(echo $CH)], expected exactly $FIXED_FILE"

# 4. pristine + pinned patch == vendor, byte for byte (also catches mode changes)
( cd "$W/p/glib-0.18.5" && GIT_DIR=/nonexistent git apply -p1 "$PATCH" ) \
  || refuse "pinned patch does not apply to pristine 0.18.5"
if ! git diff --no-index --quiet "$W/p/glib-0.18.5" "$V"; then
  git diff --no-index --stat "$W/p/glib-0.18.5" "$V" | tail -5
  refuse "vendor != pristine + upstream fix (extra or missing changes)"
fi
echo "[glib-vendor] pristine..vendor ($FIXED_FILE):"
git diff --no-index "$W/glib-0.18.5/$FIXED_FILE" "$V/$FIXED_FILE" | grep '^[-+] '
echo "GLIB-VENDOR OK glib-0.18.5 (sha256 $CRATE_SHA, crates.io index) + upstream b5a4071e (patch sha256 $PATCH_SHA), nothing else"
