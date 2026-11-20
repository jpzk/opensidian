#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# sbom-scan.sh self-test on fixtures (no network, no osv-scanner binary):
#   SBOM SARIF -> security-severity / level / fixed version mapping (CVSS, Ubuntu priority, unknown),
#   SBOM -> dependency snapshot (pkg:deb / generic / no-purl skipped + counted, npm + cargo kept),
#   sha256 verification (mismatch / missing / malformed refused), platform/arch naming,
#   per-category merge, summary counts, advisory-DB freshness (missing / stale DB fails).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
S=$here/sbom-scan.sh
F=$here/tests/fixtures/sbom-scan
SBOM=$F/opensidian-0.0.0-x86_64.cdx.json
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
eq()  { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2' want '$3'"; fi; }
refuses() { local name=$1; shift
  if "$@" >"$t/so" 2>"$t/se"; then bad "$name: accepted (rc 0)"; else ok "$name: refused ($(grep -o 'sbom-scan: .*' "$t/se" | head -c 90))"; fi; }

# --- SARIF enrichment -----------------------------------------------------------------------
"$S" enrich "$F/osv.sarif" "$F/osv.json" release-assets/v0.0.0/x.cdx.json "published v0.0.0" >"$t/e.sarif"
rule() { jq -r --arg i "$1" --arg k "$2" '.runs[0].tool.driver.rules[] | select(.id == $i) | .properties[$k] // "none"' "$t/e.sarif"; }
res()  { jq -r --arg i "$1" --arg k "$2" '.runs[0].results[] | select(.ruleId == $i) | (if $k == "level" then .level elif $k == "msg" then .message.text else .properties[$k] end) | tostring' "$t/e.sarif"; }
eq "CVSS 2.1 -> security-severity 2.1"            "$(rule CVE-2026-103923 security-severity)" 2.1
eq "CVSS 2.1 -> severity low / level note"        "$(res CVE-2026-103923 severity)/$(res CVE-2026-103923 level)" low/note
eq "CVSS 7.5 -> high / error"                     "$(res CVE-2026-84375 severity)/$(res CVE-2026-84375 level)" high/error
eq "CVSS 9.8 -> security-severity 9.8 critical"   "$(rule CVE-2026-1 security-severity)/$(res CVE-2026-1 severity)" 9.8/critical
eq "Ubuntu priority medium -> 4.0 (bucket floor)" "$(rule CVE-2025-7425 security-severity)/$(rule CVE-2025-7425 severity-source)" 4.0/ubuntu-priority
eq "no severity data -> no security-severity"     "$(rule UBUNTU-CVE-2026-9 security-severity)" none
eq "no severity data -> unknown / warning"        "$(res UBUNTU-CVE-2026-9 severity)/$(res UBUNTU-CVE-2026-9 level)" unknown/warning
eq "fixed version of the SAME package only"       "$(res CVE-2026-84375 fixed-versions)" '["4.3.2"]'
eq "fixed version in the message"                 "$(res CVE-2026-103923 msg | grep -o 'Fixed in 0.18.2')" "Fixed in 0.18.2"
eq "no fix -> fix-available false + message"      "$(res CVE-2026-1 fix-available)/$(res CVE-2026-1 msg | grep -c 'No fixed version yet')" false/1
eq "every rule tagged security"                   "$(jq '[.runs[0].tool.driver.rules[] | select(.properties.tags | index("security"))] | length' "$t/e.sarif")" 5
eq "locations point at the release asset"         "$(jq -r '[.runs[0].results[].locations[0].physicalLocation.artifactLocation.uri] | unique | join(",")' "$t/e.sarif")" release-assets/v0.0.0/x.cdx.json
eq "no ruleIndex / artifacts left (rules merged)" "$(jq '([.runs[0].results[] | select(has("ruleIndex"))] | length) + (if .runs[0] | has("artifacts") then 1 else 0 end)' "$t/e.sarif")" 0
jq '.runs[0].results[0].message.text = "garbage"' "$F/osv.sarif" >"$t/bad.sarif"
refuses "unparseable osv-scanner message" "$S" enrich "$t/bad.sarif" "$F/osv.json" u
refuses "json without .results"           "$S" enrich "$F/osv.sarif" "$SBOM" u

# --- merge + summary -------------------------------------------------------------------------
"$S" merge sbom-linux-x86_64 "$t/e.sarif" "$t/e.sarif" >"$t/m.sarif"
eq "merge: category automationDetails.id"         "$(jq -r '.runs[0].automationDetails.id' "$t/m.sarif")" sbom-linux-x86_64/
eq "merge: one run, rules unique, results concat" "$(jq -c '[(.runs|length), (.runs[0].tool.driver.rules|length), (.runs[0].results|length)]' "$t/m.sarif")" '[1,5,10]'
eq "summary counts"                               "$("$S" summary x "$t/e.sarif")" \
  "- **x**: 5 findings | critical 1, high 1, medium 1, low 1, unknown 1 | fix available 2, no fix yet 3"

# --- dependency snapshot ---------------------------------------------------------------------
SHA=0123456789abcdef0123456789abcdef01234567
"$S" snapshot $SHA refs/heads/develop sbom-scan/release-v0.0.0 2026-11-20T00:00:00Z \
  release-v0.0.0/linux-x86_64="$SBOM" >"$t/snap.json" 2>"$t/snap.err"
eq "snapshot: only pkg:npm + pkg:cargo purls"             "$(jq -r '[.manifests[].resolved[].package_url | capture("^pkg:(?<t>[a-z]+)/").t] | unique | join(",")' "$t/snap.json")" cargo,npm
eq "snapshot: 4 packages kept"                    "$(jq '[.manifests[].resolved[]] | length' "$t/snap.json")" 4
eq "snapshot: deb skipped + counted"              "$(grep -c 'skipped 2 pkg:deb' "$t/snap.err")" 1
eq "snapshot: generic skipped + counted"          "$(grep -c 'skipped 1 pkg:generic' "$t/snap.err")" 1
eq "snapshot: no deb anywhere in the body"        "$(grep -c 'pkg:deb' "$t/snap.json" || true)" 0
eq "snapshot: direct / indirect"                  "$(jq -r '.manifests[].resolved | [."pkg:npm/katex@0.16.47".relationship, ."pkg:npm/js-yaml@4.3.0".relationship] | join("/")' "$t/snap.json")" direct/indirect
eq "snapshot: dependency edges kept"              "$(jq -c '.manifests[].resolved."pkg:cargo/serde@1.0.200".dependencies' "$t/snap.json")" '["pkg:cargo/serde_derive@1.0.200"]'
eq "snapshot: stable correlator (= job id)"       "$(jq -r '[.job.correlator, .job.id, .sha, .ref, (.version|tostring)] | join(" ")' "$t/snap.json")" \
  "sbom-scan/release-v0.0.0 sbom-scan/release-v0.0.0 $SHA refs/heads/develop 0"
"$S" snapshot $SHA refs/heads/develop sbom-scan/release-v0.0.0 2026-11-20T00:00:00Z release-v0.0.0/linux-x86_64="$SBOM" >"$t/snap2.json" 2>/dev/null
eq "snapshot: deterministic (resubmission replaces)" "$([ "$(sha256sum <"$t/snap.json")" = "$(sha256sum <"$t/snap2.json")" ] && echo same)" same
refuses "snapshot: short sha"    "$S" snapshot abc refs/heads/develop c 2026-11-20T00:00:00Z a="$SBOM"
refuses "snapshot: bare ref"     "$S" snapshot $SHA develop c 2026-11-20T00:00:00Z a="$SBOM"
refuses "snapshot: not CycloneDX" "$S" snapshot $SHA refs/heads/develop c 2026-11-20T00:00:00Z a="$F/osv.json"

# --- sha256 verification ---------------------------------------------------------------------
cp "$SBOM" "$t/s.cdx.json"
sha256sum "$t/s.cdx.json" | sed "s#  .*#  s.cdx.json#" >"$t/s.cdx.json.sha256"
eq "verify: matching digest accepted"             "$("$S" verify "$t/s.cdx.json" "$t/s.cdx.json.sha256" 2>/dev/null)" "$(sha256sum "$SBOM" | cut -d' ' -f1)"
printf ' ' >>"$t/s.cdx.json"
refuses "verify: sha256 mismatch"         "$S" verify "$t/s.cdx.json" "$t/s.cdx.json.sha256"
refuses "verify: missing .sha256"         "$S" verify "$t/s.cdx.json" "$t/nope.sha256"
echo 'not-a-digest  s.cdx.json' >"$t/m.sha256"
refuses "verify: malformed .sha256"       "$S" verify "$SBOM" "$t/m.sha256"
: >"$t/empty.cdx.json"
refuses "verify: empty SBOM"              "$S" verify "$t/empty.cdx.json" "$t/s.cdx.json.sha256"

# --- platform / arch -> category -------------------------------------------------------------
eq "platform: linux x86_64 (no platform word)"    "$("$S" platform opensidian-0.9.6-x86_64.cdx.json)" "linux x86_64"
eq "platform: linux aarch64"                      "$("$S" platform opensidian-0.9.6-aarch64.cdx.json)" "linux aarch64"
eq "platform: macos arm64 -> aarch64"             "$("$S" platform opensidian-0.9.6-macos-arm64.cdx.json)" "macos aarch64"
eq "platform: macos x86_64"                       "$("$S" platform opensidian-0.9.6-macos-x86_64.cdx.json)" "macos x86_64"
refuses "platform: unknown arch"          "$S" platform opensidian-0.9.6.cdx.json

# --- advisory DB freshness -------------------------------------------------------------------
mkdir -p "$t/db/osv-scalibr/npm" "$t/rec"
echo '{}' >"$t/rec/GHSA-x.json"; touch -d '2026-11-19 12:00:00 UTC' "$t/rec/GHSA-x.json"
(cd "$t/rec" && TZ=UTC zip -q -X "$t/db/osv-scalibr/npm/all.zip" GHSA-x.json)
NOW=$(date -u -d '2026-11-20 00:00:00' +%s)
if NOW=$NOW "$S" dbrev "$t/db" "npm" >"$t/so" 2>&1; then ok "dbrev: fresh DB accepted ($(grep -o 'newest record [^ ]*' "$t/so"))"; else cat "$t/so"; bad "dbrev: fresh DB refused"; fi
refuses "dbrev: DB not fetched"           env NOW="$NOW" "$S" dbrev "$t/db" crates.io
refuses "dbrev: DB older than 3 days"     env NOW=$((NOW + 5 * 86400)) "$S" dbrev "$t/db" npm
# crates.io ships zip entries without times (1980): freshness from the records' "modified"
mkdir -p "$t/db/osv-scalibr/crates.io" "$t/rec2"
echo '{"id":"RUSTSEC-1","modified":"2026-11-18T08:00:00Z"}' >"$t/rec2/RUSTSEC-1.json"
echo '{"id":"RUSTSEC-2","modified":"2026-11-19T09:30:00Z"}' >"$t/rec2/RUSTSEC-2.json"
touch -d '1980-01-01 00:00:00 UTC' "$t/rec2"/*.json
(cd "$t/rec2" && TZ=UTC zip -q -X "$t/db/osv-scalibr/crates.io/all.zip" RUSTSEC-1.json RUSTSEC-2.json)
if NOW=$NOW "$S" dbrev "$t/db" crates.io >"$t/so" 2>&1 && grep -q 'newest record 2026-11-19T09:30:00Z' "$t/so"; then ok "dbrev: 1980 zip times -> newest record modified"; else cat "$t/so"; bad "dbrev: 1980 zip times"; fi
refuses "dbrev: 1980 zip times + stale modified" env NOW=$((NOW + 5 * 86400)) "$S" dbrev "$t/db" crates.io
echo '{"id":"RUSTSEC-3"}' >"$t/rec2/RUSTSEC-3.json"; touch -d '1980-01-01 00:00:00 UTC' "$t/rec2/RUSTSEC-3.json"; rm -f "$t/db/osv-scalibr/crates.io/all.zip"
(cd "$t/rec2" && TZ=UTC zip -q -X "$t/db/osv-scalibr/crates.io/all.zip" RUSTSEC-3.json)
refuses "dbrev: 1980 zip times + no modified"    env NOW="$NOW" "$S" dbrev "$t/db" crates.io

# --- snapshot of a real-sized SBOM (manifest JSON larger than one argv string, 128 KiB) ------
jq -n '{bomFormat: "CycloneDX", specVersion: "1.6", components: [range(4000) | {"bom-ref": "n\(.)", purl: "pkg:npm/big-package-name-\(.)@1.0.\(.)"}]}' >"$t/big.cdx.json"
if "$S" snapshot $SHA refs/heads/develop c 2026-11-20T00:00:00Z a="$t/big.cdx.json" b="$SBOM" >"$t/big.json" 2>"$t/big.err"; then
  eq "snapshot: big SBOM kept 4000 + 4 packages" "$(jq '[.manifests[].resolved[]] | length' "$t/big.json")" 4004
else cat "$t/big.err"; bad "snapshot: big SBOM failed"; fi

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
