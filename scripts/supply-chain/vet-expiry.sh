#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vet-expiry.sh — #304 review: cargo vet reads an exemption's `notes` as plain text, so an
# "EXPIRES <date>" written there is not enforced by cargo vet. This gate enforces it.
#   bash scripts/supply-chain/vet-expiry.sh [CONFIG]   (default src-tauri/supply-chain/config.toml)
# For every [[exemptions.<crate>]] block whose notes name an expiry — "EXPIRES YYYY-MM-DD" or
# "revisit by YYYY-MM-DD" — the date must not be in the past (the day itself is still valid, as in
# pin-check.sh). A marker without a valid date is a finding too (fail closed).
# One line per finding: "VET expired <crate> <version> <config>:<line>: <date> (today <today>)".
# rc 0 = clean, 1 = findings, 2 = usage. VET_EXPIRY_TODAY=YYYY-MM-DD overrides today (tests).
# Pure bash + grep/sed/awk: no tool install, no network.
set -euo pipefail
here=$(cd "$(dirname "$0")/../.." && pwd)
cfg=${1:-$here/src-tauri/supply-chain/config.toml}
[ -f "$cfg" ] || { echo "vet-expiry: no config $cfg" >&2; exit 2; }
TODAY=${VET_EXPIRY_TODAY:-$(date -u +%F)}
[[ "$TODAY" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || { echo "vet-expiry: bad today '$TODAY'" >&2; exit 2; }
out=$(awk -v today="$TODAY" -v cfg="$cfg" '
  function flush() {
    if (crate == "" || notes == "") return
    n++
    s = notes; found = 0
    while (match(s, /(EXPIRES|[Rr]evisit by)[ :]+[0-9-]*/)) {
      found = 1; m = substr(s, RSTART, RLENGTH); s = substr(s, RSTART + RLENGTH)
      d = m; sub(/^(EXPIRES|[Rr]evisit by)[ :]+/, "", d)
      if (d !~ /^[0-9][0-9][0-9][0-9]-[0-1][0-9]-[0-3][0-9]$/) { print "VET baddate " crate " " ver " " cfg ":" nl ": \"" m "\""; bad++ }
      else if (d < today) { print "VET expired " crate " " ver " " cfg ":" nl ": " d " (today " today ")"; bad++ }
      else dated++
    }
  }
  /^\[\[exemptions\./ { flush(); crate = $0; sub(/^\[\[exemptions\./, "", crate); sub(/\]\].*$/, "", crate); ver = ""; notes = ""; nl = 0; next }
  /^\[/ { flush(); crate = ""; next }
  crate != "" && /^version *= */ { ver = $0; sub(/^version *= *"/, "", ver); sub(/".*$/, "", ver); next }
  crate != "" && /^notes *= */ { notes = $0; nl = NR; next }
  END { flush(); printf "SUMMARY %d dated %d finding(s)\n", dated + 0, bad + 0 }
' "$cfg")
echo "$out" | grep '^VET ' || true
s=$(echo "$out" | tail -1); dated=$(echo "$s" | cut -d' ' -f2); bad=$(echo "$s" | cut -d' ' -f4)
echo "vet-expiry: $cfg: $dated dated exemption expiries checked against $TODAY, $bad finding(s)"
[ "$bad" = 0 ]
