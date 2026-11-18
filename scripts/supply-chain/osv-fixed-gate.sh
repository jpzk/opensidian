#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# osv-fixed-gate.sh <osv-scanner.json>
# Gate for distro (deb) findings: osv-scanner reports every advisory that matches a package,
# including ones the distro has not fixed yet (nothing to upgrade to; these are listed, not
# fatal). rc=1 iff at least one finding has a FIXED version for the same package + ecosystem
# in its OSV record (ecosystem equal, or the package's ecosystem is a prefix of the record's,
# e.g. "Ubuntu" vs "Ubuntu:26.04:LTS": fails closed if osv-scanner drops the release) (an upgrade exists and the bundle still ships the vulnerable one).
# Input: `osv-scanner scan source --format json -L <sbom>.cdx.json > osv.json` (osv-scanner
# itself exits 1 on any finding; run it with `|| true` and let this script decide).
set -euo pipefail
f=${1:?usage: $0 <osv-scanner.json>}
jq -e '.results | type == "array"' "$f" >/dev/null || { echo "osv-fixed-gate: $f is not osv-scanner json" >&2; exit 1; }
out=$(jq -r '
  [ .results[]?.packages[]? as $p
    | $p.vulnerabilities[]? as $v
    | ([ $v.affected[]? | select(.package.name == $p.package.name and (.package.ecosystem == $p.package.ecosystem
                or (.package.ecosystem | startswith($p.package.ecosystem + ":"))))
         | .ranges[]?.events[]? | .fixed // empty ] | unique) as $fx
    | { pkg: $p.package.name, ver: $p.package.version, eco: $p.package.ecosystem,
        id: $v.id, aliases: (($v.aliases // []) | join(",")), fixed: $fx } ]
  | (map(select(.fixed | length > 0)) | sort_by(.pkg, .id)) as $fatal
  | (map(select(.fixed | length == 0)) | sort_by(.pkg, .id)) as $open
  | ($fatal[] | "FIXED-AVAILABLE \(.eco) \(.pkg)@\(.ver) \(.id) [\(.aliases)] fixed in \(.fixed | join(" "))"),
    ($open[]  | "no-fix-yet      \(.eco) \(.pkg)@\(.ver) \(.id) [\(.aliases)]"),
    "summary: \($fatal | length) with a fixed version, \($open | length) without",
    (if ($fatal | length) > 0 then "RESULT: FAIL" else "RESULT: PASS" end)
' "$f")
printf '%s\n' "$out"
[ "${out##*$'\n'}" = "RESULT: PASS" ]
