#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# vendor-sbom.sh — CycloneDX 1.6 SBOM (JSON, stdout) of the JavaScript vendored under ui/vendor.
#
# Sources of truth (no network, bash + jq + coreutils only):
#   ui/vendor/<lib>/VENDOR.md     "- package: <npm name>" + "- version: <ver>" (mathjax, pdfjs, prism)
#   ui/vendor/codemirror/VENDOR.md "## Packages (bundled)" list "- <npm name> <ver> — ..." (bundled into cm6.js)
#   ui/vendor/moment.min.js       header "/*! moment.js <ver> (npm moment@<ver>, ...)"
#
# No silent gaps: every file under ui/vendor must be claimed by a component, either
#   - listed in <lib>/SHA256SUMS (and its hash must match), or
#   - named in <lib>/VENDOR.md as "- <file> sha256: `<hex>`" (and its hash must match), or
#   - the VENDOR.md / SHA256SUMS metadata of a directory that produced a component, or
#   - moment.min.js (claimed by its own header).
# Anything else (a new library, a stray file, a hash drift) => exit 1, nothing on stdout.
#
# Usage: scripts/vendor-sbom.sh [> sbom.cdx.json]     VENDOR_DIR=... overrides ui/vendor (tests)
set -euo pipefail
export LC_ALL=C

root=$(cd "$(dirname "$0")/.." && pwd)
vd=${VENDOR_DIR:-$root/ui/vendor}
[ -d "$vd" ] || { echo "vendor-sbom: no such dir: $vd" >&2; exit 1; }
command -v jq >/dev/null || { echo "vendor-sbom: jq required" >&2; exit 1; }

fail=0
err() { echo "vendor-sbom: $*" >&2; fail=1; }

comps=$(mktemp); covered=$(mktemp)
trap 'rm -f "$comps" "$covered"' EXIT

# sha512 SRI ("sha512-<base64>") -> hex
sri_hex() { printf '%s' "${1#sha512-}" | base64 -d | od -An -v -tx1 | tr -d ' \n'; }

# add_comp <npm name> <version> <evidence path> [sri]
add_comp() {
  local name=$1 ver=$2 ev=$3 sri=${4:-} enc purl hex=""
  enc=${name/#@/%40}
  purl="pkg:npm/$enc@$ver"
  [ -n "$sri" ] && hex=$(sri_hex "$sri")
  jq -cn --arg n "$name" --arg v "$ver" --arg p "$purl" --arg ev "$ev" --arg h "$hex" '{
      type: "library", "bom-ref": $p, name: $n, version: $v, purl: $p,
      properties: [{name: "opensidian:vendor:evidence", value: $ev}]
    } + (if $h == "" then {} else {hashes: [{alg: "SHA-512", content: $h}]} end)' >>"$comps"
}

claim() { printf '%s\n' "$1" >>"$covered"; }

# check + claim the files of <lib>/SHA256SUMS
claim_sums() {
  local dir=$1 rel=$2 sum f
  [ -f "$dir/SHA256SUMS" ] || return 0
  while read -r sum f; do
    [ -n "${f:-}" ] || continue
    if [ ! -f "$dir/$f" ]; then err "$rel/SHA256SUMS lists missing file $f"; continue; fi
    [ "$(sha256sum "$dir/$f" | cut -d' ' -f1)" = "$sum" ] || err "$rel/$f: sha256 differs from SHA256SUMS"
    claim "$rel/$f"
  done <"$dir/SHA256SUMS"
  claim "$rel/SHA256SUMS"
}

# check + claim files named "- <file> sha256: `<hex>`" in VENDOR.md
claim_md_hashes() {
  local dir=$1 rel=$2 f sum
  while read -r f sum; do
    [ -n "$f" ] || continue
    if [ ! -f "$dir/$f" ]; then err "$rel/VENDOR.md names missing file $f"; continue; fi
    [ "$(sha256sum "$dir/$f" | cut -d' ' -f1)" = "$sum" ] || err "$rel/$f: sha256 differs from VENDOR.md"
    claim "$rel/$f"
  done <<EOF_MD
$(sed -nE 's/^- ([^ `]+\.[A-Za-z0-9]+) sha256: `([0-9a-f]{64})`.*/\1 \2/p' "$dir/VENDOR.md")
EOF_MD
}

for dir in "$vd"/*/; do
  dir=${dir%/}; rel=${dir##*/}
  md=$dir/VENDOR.md
  if [ ! -f "$md" ]; then err "ui/vendor/$rel/ has no VENDOR.md (unknown library)"; continue; fi
  n0=$(wc -l <"$comps")
  pkg=$(sed -nE 's/^- package: *([^ ]+).*/\1/p' "$md" | head -n1)
  ver=$(sed -nE 's/^- version: *([0-9][^ ]*).*/\1/p' "$md" | head -n1)
  sri=$(sed -nE 's/^- tarball integrity.*`(sha512-[^`]+)`.*/\1/p' "$md" | head -n1)
  if [ -n "$pkg" ] && [ -n "$ver" ]; then
    add_comp "$pkg" "$ver" "ui/vendor/$rel/VENDOR.md" "$sri"
  fi
  # bundle list (codemirror): "- <name> <ver> — published ... — `sha512-...`", only in "## Packages"
  while read -r name v s; do
    [ -n "$name" ] || continue
    add_comp "$name" "$v" "ui/vendor/$rel/VENDOR.md#packages-bundled" "$s"
  done <<EOF_PK
$(awk '/^## /{p = ($0 ~ /^## Packages/)} p' "$md" \
    | sed -nE 's/^- (@?[A-Za-z0-9._/-]+) ([0-9][0-9A-Za-z.+-]*) — .*`(sha512-[^`]+)`.*/\1 \2 \3/p')
EOF_PK
  if [ "$(wc -l <"$comps")" -eq "$n0" ]; then
    err "ui/vendor/$rel/VENDOR.md declares no package/version (no component)"; continue
  fi
  claim "$rel/VENDOR.md"
  claim_sums "$dir" "$rel"
  claim_md_hashes "$dir" "$rel"
done

# moment: single file, version from its header comment
if [ -f "$vd/moment.min.js" ]; then
  mv=$(head -c 512 "$vd/moment.min.js" | sed -nE 's/^\/\*! moment\.js ([0-9]+\.[0-9]+\.[0-9]+) \(npm moment@\1.*/\1/p' | head -n1)
  msri=$(head -c 1024 "$vd/moment.min.js" | sed -nE 's/.*tarball integrity (sha512-[A-Za-z0-9+\/=]+).*/\1/p' | head -n1)
  if [ -n "$mv" ]; then add_comp moment "$mv" "ui/vendor/moment.min.js header" "$msri"; claim moment.min.js
  else err "ui/vendor/moment.min.js: no '/*! moment.js <ver> (npm moment@<ver>' header"; fi
fi

# no silent gaps
while IFS= read -r f; do
  [ -n "$f" ] || continue
  grep -qxF "$f" "$covered" || err "ui/vendor/$f is not covered by any SBOM component"
done <<EOF_LS
$(cd "$vd" && find . \( -type f -o -type l \) | sed 's|^\./||' | sort)
EOF_LS

[ "$fail" -eq 0 ] || { echo "vendor-sbom: FAILED" >&2; exit 1; }
[ -s "$comps" ] || { echo "vendor-sbom: no components" >&2; exit 1; }

jq -s '{
  bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
  metadata: {component: {type: "application", "bom-ref": "opensidian-ui-vendor",
                         name: "opensidian ui/vendor", description: "JavaScript vendored under ui/vendor"}},
  components: (sort_by(.purl) | unique_by(.purl))
}' "$comps"
