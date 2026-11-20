#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# osv-json.sh <out.json> <osv-scanner> <args...>
# Runs `<osv-scanner> <args...>` (args must include --format json) and writes stdout to <out.json>.
# osv-scanner exits 0 (no findings) or 1 (findings); anything else (127 general error,
# 128 no packages, a crash) is a failed scan and must not be read as "zero findings".
# Exit 0 iff the scan itself succeeded (rc 0 or 1) AND the output is osv-scanner JSON with a
# .results array; the caller's gate (osv-fixed-gate.sh / jq) then decides on the findings.
set -uo pipefail
out=${1:?usage: $0 <out.json> <osv-scanner> <args...>}; shift
[ $# -ge 1 ] || { echo "usage: $0 <out.json> <osv-scanner> <args...>" >&2; exit 2; }
"$@" >"$out"; rc=$?
case $rc in
  0|1) ;;
  *) echo "::error::osv-scanner failed (rc=$rc): scan did not complete, refusing to gate on it" >&2; exit 1;;
esac
jq -e '.results | type == "array"' "$out" >/dev/null 2>&1 \
  || { echo "::error::osv-scanner rc=$rc but $out has no .results array" >&2; exit 1; }
n=$(jq '[.results[].packages[]? | select((.vulnerabilities // []) | length > 0)] | length' "$out")
if [ "$rc" -eq 1 ] && [ "$n" -eq 0 ]; then
  echo "::error::osv-scanner rc=1 (findings) but $out lists none: inconsistent output" >&2; exit 1
fi
echo "osv-json: rc=$rc, packages with findings: $n"
