#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# webkit-wsa-gate.sh <webkit.cdx.json> [webkit-wsa.tsv]
# BLOCKING WebKit CVE gate for the Fedora-built bundle. osv.dev has no Fedora ecosystem, so
# osv-scanner cannot score pkg:rpm/fedora purls (it would pass vacuously). Instead: the bundled
# webkit2gtk4.1 rpm (exactly one pkg:rpm/fedora purl) must have an upstream version >= the
# fixed-in version of EVERY WebKitGTK security advisory in webkit-wsa.tsv. Fails closed when the
# component is missing/duplicated, the version is unparsable, or the table is empty.
set -euo pipefail
s=${1:?usage: $0 <webkit.cdx.json> [webkit-wsa.tsv]}
t=${2:-$(dirname "$0")/webkit-wsa.tsv}
jq -e '.components | type == "array"' "$s" >/dev/null || { echo "webkit-wsa-gate: $s is not a CycloneDX sbom" >&2; exit 1; }
pl=$(jq -r '.components[] | .purl // empty | select(startswith("pkg:rpm/fedora/webkit2gtk4.1@"))' "$s" | sort -u)
mapfile -t P <<< "$pl"; [ -n "$pl" ] || P=()
[ "${#P[@]}" -eq 1 ] || { echo "webkit-wsa-gate: expected exactly 1 pkg:rpm/fedora/webkit2gtk4.1 purl, found ${#P[@]}" >&2; exit 1; }
evr=${P[0]#pkg:rpm/fedora/webkit2gtk4.1@}; evr=${evr%%\?*}; v=${evr%-*}
[[ $v =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "webkit-wsa-gate: unparsable version '$evr' in ${P[0]}" >&2; exit 1; }
echo "bundled webkit2gtk4.1 $evr (upstream $v)"
n=0 bad=0
while IFS=$'\t' read -r id rep fx; do
  case $id in ''|'#'*) continue;; esac
  [[ $fx =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "webkit-wsa-gate: bad row '$id'" >&2; exit 1; }
  n=$((n+1))
  if [ "$(printf '%s\n%s\n' "$fx" "$v" | sort -V | head -1)" = "$fx" ]; then echo "fixed      $id ($rep) fixed in $fx"
  else echo "VULNERABLE $id ($rep) fixed in $fx > bundled $v"; bad=$((bad+1)); fi
done < "$t"
[ "$n" -gt 0 ] || { echo "webkit-wsa-gate: no advisories in $t" >&2; exit 1; }
echo "summary: $n advisories, $bad not fixed in the bundle"
[ "$bad" -eq 0 ] && echo "RESULT: PASS" || { echo "RESULT: FAIL"; exit 1; }
