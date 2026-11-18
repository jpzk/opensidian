#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# vendor-sbom.sh — CycloneDX 1.6 SBOM of the vendored JS in ui/vendor (issue #188).
# bash + jq + coreutils only; no npm, no node.
#
# Source of truth: ui/vendor/vendor.lock.json, and ONLY after vendor-verify.sh has proven it
# (registry integrity + tarball URL, per-file sha256, embedded versions, no uncovered file, no
# VENDOR.md, publish dates / six-week rule). This script runs vendor-verify.sh first and emits
# nothing if it fails. VENDOR.md prose is never read (and its presence fails the verify).
#
# Components:
#   libraries[]  -> one pkg:npm component each (license, sha512 from dist.integrity, tarball URL,
#                   loaded_by, per-file sha256 as properties);
#   bundles[]    -> one "file" component for the bundle bytes (sha256) that dependsOn the npm
#                   components read from its COMMITTED package-lock.json (non-dev packages:
#                   version + integrity + resolved); dev-only build tools (esbuild) are left out
#                   because they are not shipped.
#
# Usage: scripts/supply-chain/vendor-sbom.sh [> vendor.cdx.json]
#   VENDOR_DIR=<dir> (default ui/vendor), LOCK=<file>, NPM_REGISTRY=<url> are passed to vendor-verify.sh
#   SOURCE_DATE_EPOCH=<s> fixes metadata.timestamp (reproducible output)
# Exit 0 = SBOM on stdout; 1 = verify failed / bad input (nothing on stdout); 2 = usage.
set -euo pipefail
export LC_ALL=C

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
export VENDOR_DIR=${VENDOR_DIR:-$root/ui/vendor}
export LOCK=${LOCK:-$VENDOR_DIR/vendor.lock.json}
vd=$VENDOR_DIR lock=$LOCK
for t in jq base64 od; do command -v "$t" >/dev/null || { echo "vendor-sbom: $t required" >&2; exit 2; }; done

# a)-e) first; its report goes to stderr so stdout stays pure JSON
"$here/vendor-verify.sh" >&2 || { echo "vendor-sbom: vendor-verify.sh failed, no SBOM" >&2; exit 1; }

sri_hex() { # sha512-<base64> -> hex
  printf '%s' "${1#sha512-}" | base64 -d | od -An -v -tx1 | tr -d ' \n'
}
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT

# libraries
jq -c '.libraries[]' "$lock" >"$tmp/libs"
: >"$tmp/comps"
while IFS= read -r l; do
  hex=$(sri_hex "$(jq -r .integrity <<<"$l")")
  jq -c --arg hex "$hex" '{
      type: "library", "bom-ref": .purl, name, version, purl,
      licenses: [{license: {id: .license}}],
      hashes: [{alg: "SHA-512", content: $hex}],
      externalReferences: [{type: "distribution", url: .tarball}],
      properties: ([{name: "opensidian:loaded_by", value: .loaded_by},
                    {name: "opensidian:npm_published", value: .published},
                    {name: "opensidian:vendored_added", value: .added}]
                   + [.files[] | {name: "opensidian:file", value: "ui/vendor/\(.path) sha256:\(.sha256) <- \(.tarball_path // "concat of \(.concat | length) tarball files: \(.concat | join(" "))")"}])
    }' <<<"$l" >>"$tmp/comps"
done <"$tmp/libs"

# bundles: the bundle file + its package-lock packages
: >"$tmp/deps"
jq -c '.bundles[]' "$lock" >"$tmp/bundles"
while IFS= read -r b; do
  pl=$vd/$(jq -r .package_lock <<<"$b")
  [ -f "$pl" ] || { echo "vendor-sbom: missing $pl" >&2; exit 1; }
  bref="file:ui/vendor/$(jq -r .path <<<"$b")"
  : >"$tmp/bdeps"
  jq -c '.packages | to_entries[] | select(.key != "" and (.value.dev // false) == false)
         | {name: (.key | sub("^.*node_modules/"; "")), version: .value.version,
            integrity: .value.integrity, resolved: .value.resolved, license: .value.license}' "$pl" >"$tmp/pkgs"
  while IFS= read -r p; do
    hex=$(sri_hex "$(jq -r .integrity <<<"$p")")
    jq -c --arg hex "$hex" --arg pl "ui/vendor/$(jq -r .package_lock <<<"$b")" '
      (.name | sub("^@"; "%40")) as $pn
      | ("pkg:npm/\($pn)@\(.version)") as $purl
      | {type: "library", "bom-ref": $purl, name, version, purl: $purl,
         licenses: (if .license then [{license: {id: .license}}] else [] end),
         hashes: [{alg: "SHA-512", content: $hex}],
         externalReferences: [{type: "distribution", url: .resolved}],
         properties: [{name: "opensidian:source", value: $pl}]}' <<<"$p" >>"$tmp/comps"
    jq -r '(.name | sub("^@"; "%40")) as $pn | "pkg:npm/\($pn)@\(.version)"' <<<"$p" >>"$tmp/bdeps"
  done <"$tmp/pkgs"
  [ -s "$tmp/bdeps" ] || { echo "vendor-sbom: $pl has no packages" >&2; exit 1; }
  jq -c --arg ref "$bref" '{type: "file", "bom-ref": $ref, name: $ref | sub("^file:"; ""),
      licenses: [{license: {id: .license}}], hashes: [{alg: "SHA-256", content: .sha256}],
      description: ."$comment",
      properties: [{name: "opensidian:loaded_by", value: .loaded_by},
                   {name: "opensidian:built_with", value: "\(.built_with.tool) \(.built_with.version)"}]}' <<<"$b" >>"$tmp/comps"
  jq -Rn -c --arg ref "$bref" '{ref: $ref, dependsOn: [inputs]}' <"$tmp/bdeps" >>"$tmp/deps"
done <"$tmp/bundles"

ts=$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%dT%H:%M:%SZ)
jq -s --slurpfile deps "$tmp/deps" --arg ts "$ts" '{
    bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
    metadata: {timestamp: $ts,
               tools: {components: [{type: "application", name: "scripts/supply-chain/vendor-sbom.sh"}]},
               component: {type: "application", "bom-ref": "opensidian-ui-vendor", name: "opensidian ui/vendor"}},
    components: (unique_by(."bom-ref") | sort_by(."bom-ref")),
    dependencies: ([{ref: "opensidian-ui-vendor",
                     dependsOn: ([.[] | select(.type == "file" or (.properties | map(.name) | index("opensidian:loaded_by")))
                                 | ."bom-ref"] | unique)}] + $deps)
  }' "$tmp/comps" >"$tmp/bom.json"
# no duplicate refs between libraries and bundle packages hidden by unique_by
[ "$(jq 'length' -s "$tmp/comps")" = "$(jq '.components|length' "$tmp/bom.json")" ] \
  || { echo "vendor-sbom: duplicate bom-ref across libraries/bundles" >&2; exit 1; }
cat "$tmp/bom.json"
