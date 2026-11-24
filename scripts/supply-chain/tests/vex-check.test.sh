#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vex-check.sh: rc=0 on the repo; rc!=0 on scratch copies where (1) one id is removed from
# deny.toml only, (2) one id is removed from audit.toml only, (3) an extra id is added to the
# VEX only, (4) an expiry is set in the past, (5) an entry has no reason, (6) time moves past
# the real expiry (VEX_TODAY). Never touches the checkout (mktemp copies).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
scratch() { rm -rf "$t/r"; mkdir -p "$t/r/supply-chain" "$t/r/src-tauri/.cargo"
  cp "$root/deny.toml" "$t/r/"; cp "$root/supply-chain/vex.cdx.json" "$t/r/supply-chain/"
  cp "$root/src-tauri/.cargo/audit.toml" "$t/r/src-tauri/.cargo/"; }
expect_fail() { local name=$1 pat=$2; shift 2
  set +e; "$@" >"$t/out" 2>&1; rc=$?; set -e
  if [ "$rc" -ne 0 ] && grep -q -- "$pat" "$t/out"; then ok "$name: rc=$rc"; else cat "$t/out"; bad "$name: rc=$rc (want !=0 and '$pat')"; fi; }

if "$here/vex-check.sh" >"$t/out" 2>&1; then ok "repo: rc=0"; else cat "$t/out"; bad "repo: rc!=0"; fi

scratch; sed -i '/id = "RUSTSEC-2025-0098"/d' "$t/r/deny.toml"
expect_fail "id removed from deny.toml only" "only in VEX: \[RUSTSEC-2025-0098\]" "$here/vex-check.sh" "$t/r"

scratch; sed -i '/"RUSTSEC-2026-0543"/d' "$t/r/src-tauri/.cargo/audit.toml"
expect_fail "id removed from audit.toml only" "audit ignore list differ" "$here/vex-check.sh" "$t/r"

scratch; jq '.vulnerabilities += [.vulnerabilities[0] | .id="RUSTSEC-2099-0001" | ."bom-ref"="vex:x"]' \
  "$root/supply-chain/vex.cdx.json" >"$t/r/supply-chain/vex.cdx.json"
expect_fail "extra id in VEX only" "RUSTSEC-2099-0001" "$here/vex-check.sh" "$t/r"

scratch; jq '(.vulnerabilities[] | select(.id=="RUSTSEC-2026-0543") | .properties[] | select(.name=="opensidian:expires") | .value) = "2025-01-01"' \
  "$root/supply-chain/vex.cdx.json" >"$t/r/supply-chain/vex.cdx.json"
expect_fail "expiry in the past" "RUSTSEC-2026-0543: expired on 2025-01-01" "$here/vex-check.sh" "$t/r"

scratch; jq '(.vulnerabilities[1].analysis) |= del(.detail)' "$root/supply-chain/vex.cdx.json" >"$t/r/supply-chain/vex.cdx.json"
expect_fail "entry without reason" "needs id, analysis.state, analysis.detail" "$here/vex-check.sh" "$t/r"

expect_fail "today past real expiry" "expired on 2027-05-15" env VEX_TODAY=2027-06-01 "$here/vex-check.sh"

scratch; sed -i 's/^ignore = \[/ignore = [\n  "RUSTSEC-2024-0429",/' "$t/r/src-tauri/.cargo/audit.toml"
expect_fail "resolved id also ignored in audit.toml" "resolved' in the VEX but ignored in audit" "$here/vex-check.sh" "$t/r"

scratch; jq '(.vulnerabilities[] | select(.id=="RUSTSEC-2024-0429") | .analysis.detail) = "trust me"' \
  "$root/supply-chain/vex.cdx.json" >"$t/r/supply-chain/vex.cdx.json"
expect_fail "resolved without a proving script" "names no scripts/supply-chain" "$here/vex-check.sh" "$t/r"

# CVE-2024-43091: affected by operator decision 2026-11-24 (no self-built WebKitGTK, adopt >= 2.54.0)
A=UBUNTU-CVE-2024-43091
if "$here/vex-check.sh" 2>&1 | grep -qx "  $A  exploitable  expires 2027-02-23" \
  && jq -e --arg i "$A" '[.vulnerabilities[] | select(.id==$i)] | length == 1 and (.[0].properties | any(.name=="opensidian:vex-status" and .value=="affected"))
      and (.[0].affects[0].ref | startswith("pkg:deb/ubuntu/webkit2gtk@"))' "$root/supply-chain/vex.cdx.json" >/dev/null
then ok "repo: $A present, exploitable (= affected), on webkit2gtk"; else bad "repo: $A entry missing or wrong"; fi
mut() { scratch; jq --arg i "$A" "$1" "$root/supply-chain/vex.cdx.json" >"$t/r/supply-chain/vex.cdx.json"; }
mut '(.vulnerabilities[] | select(.id==$i) | .analysis) |= (.state="not_affected" | .justification="code_not_reachable")'
expect_fail "$A relabelled not_affected" "decided-affected $A: analysis.state is 'not_affected'" "$here/vex-check.sh" "$t/r"
mut '(.vulnerabilities[] | select(.id==$i) | .analysis.state) = "resolved"'
expect_fail "$A relabelled resolved" "decided-affected $A: analysis.state is 'resolved'" "$here/vex-check.sh" "$t/r"
mut 'del(.vulnerabilities[] | select(.id==$i))'
expect_fail "$A entry deleted" "decided-affected $A: want exactly 1 VEX entry, found 0" "$here/vex-check.sh" "$t/r"
mut '(.vulnerabilities[] | select(.id==$i) | .properties) |= map(select(.name!="opensidian:action_statement"))'
expect_fail "$A without action_statement" "needs an opensidian:action_statement" "$here/vex-check.sh" "$t/r"
mut '(.vulnerabilities[] | select(.id==$i) | .properties) |= map(select(.name!="opensidian:vex-status"))'
expect_fail "$A without vex-status" "needs opensidian:vex-status = affected" "$here/vex-check.sh" "$t/r"
mut '(.vulnerabilities[] | select(.id==$i) | .properties[] | select(.name=="opensidian:action_statement") | .value) |= gsub("2\\.54\\.0"; "2.52.7")'
expect_fail "$A action_statement without 2.54.0" "action_statement lacks '2.54.0'" "$here/vex-check.sh" "$t/r"
mut '(.vulnerabilities[] | select(.id==$i) | .properties[] | select(.name=="opensidian:vex-status") | .value) = "affected" | (.vulnerabilities[] | select(.id=="UBUNTU-CVE-2025-26416") | .properties) += [{name:"opensidian:vex-status",value:"affected"}]'
expect_fail "vex-status affected on a not_affected entry" "UBUNTU-CVE-2025-26416: opensidian:vex-status affected but analysis.state is not_affected" "$here/vex-check.sh" "$t/r"

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
