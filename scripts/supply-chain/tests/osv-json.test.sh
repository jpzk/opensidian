#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# osv-json.sh on a stubbed scanner (no network, no osv-scanner binary): a scan that did not
# complete (rc 127/128, crash, non-JSON, JSON without .results, rc=1 with no findings) must
# fail; rc 0 / rc 1 with a real .results array must pass and leave the JSON for the gate.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
# stub: prints $STUB_OUT, exits $STUB_RC; must receive the caller's args verbatim
cat >"$t/osv" <<'S'
#!/bin/sh
[ "$1 $2" = "scan source" ] || { echo "stub: bad args $*" >&2; exit 99; }
printf '%s' "$STUB_OUT"; exit "$STUB_RC"
S
chmod +x "$t/osv"
VULN='{"results":[{"packages":[{"package":{"name":"moment","version":"2.30.1","ecosystem":"npm"},"vulnerabilities":[{"id":"GHSA-x"}]}]}]}'
CLEAN='{"results":[]}'
try() { local name=$1 want=$2 rc
  set +e; "$here/osv-json.sh" "$t/o.json" "$t/osv" scan source --format json -L x.cdx.json >"$t/so" 2>"$t/se"; rc=$?; set -e
  if { [ "$want" = pass ] && [ "$rc" -eq 0 ]; } || { [ "$want" = fail ] && [ "$rc" -ne 0 ]; }; then ok "$name: rc=$rc"
  else cat "$t/so" "$t/se"; bad "$name: rc=$rc (want $want)"; fi; }

STUB_RC=0   STUB_OUT=$CLEAN        try "rc 0, empty results"            pass
STUB_RC=1   STUB_OUT=$VULN         try "rc 1, findings"                 pass
[ "$(jq -r '.results[0].packages[0].package.name' "$t/o.json")" = moment ] && ok "JSON kept for the gate" || bad "JSON not written"
STUB_RC=127 STUB_OUT=$CLEAN        try "rc 127 (general error)"         fail
STUB_RC=128 STUB_OUT=$CLEAN        try "rc 128 (no packages)"           fail
STUB_RC=0   STUB_OUT='{}'          try "rc 0, JSON without .results"    fail
STUB_RC=0   STUB_OUT='not json'    try "rc 0, not JSON"                 fail
STUB_RC=1   STUB_OUT=$CLEAN        try "rc 1 but no findings listed"    fail
STUB_RC=0   STUB_OUT=''            try "rc 0, empty output"             fail
[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
