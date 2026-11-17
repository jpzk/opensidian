#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vex-check.sh — supply-chain/vex.cdx.json (CycloneDX 1.6 VEX) is the SINGLE source of
# truth for ignored advisories (issue #188). Fails (rc=1) when:
#   - the VEX is not CycloneDX 1.6, or an entry lacks id / analysis.state / analysis.detail /
#     an opensidian:expires date (YYYY-MM-DD), or an affects ref has no component;
#   - any opensidian:expires is in the past (UTC);
#   - the RustSec ids of the VEX, of /deny.toml [advisories].ignore and of
#     src-tauri/.cargo/audit.toml [advisories].ignore are not the same set (or have duplicates).
# Non-RustSec VEX entries (GHSA/CVE for npm/deb, consumed by osv-scanner) are checked for
# shape + expiry only; cargo tools never see them.
# usage: vex-check.sh [repo-root]      (default: this checkout; tests pass a scratch copy)
# env:   VEX_TODAY=YYYY-MM-DD overrides today (tests only)
set -euo pipefail
root=${1:-$(cd "$(dirname "$0")/../.." && pwd)}
vex=$root/supply-chain/vex.cdx.json
deny=$root/deny.toml
audit=$root/src-tauri/.cargo/audit.toml
today=${VEX_TODAY:-$(date -u +%F)}
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fail=0
err() { echo "vex-check: FAIL $*" >&2; fail=1; }

for f in "$vex" "$deny" "$audit"; do [ -f "$f" ] || { echo "vex-check: FAIL missing $f" >&2; exit 1; }; done

# ids in [advisories] ignore = [ ... ] of a TOML file: drop comments (outside strings),
# drop reason = "..." values, then every remaining quoted string in the array is an id.
toml_ignore_ids() {
  awk '
    function strip(s,   i,c,q,o) { q=0; o="";
      for (i=1;i<=length(s);i++) { c=substr(s,i,1);
        if (c=="\"") q=!q; else if (c=="#" && !q) break;
        o=o c }
      return o }
    /^[[:space:]]*\[/ { sec=$0; gsub(/[[:space:]]/,"",sec); inadv=(sec=="[advisories]"); next }
    inadv { l=strip($0)
      if (!inarr && l ~ /^[[:space:]]*ignore[[:space:]]*=[[:space:]]*\[/) { inarr=1; sub(/^[^[]*\[/,"",l) }
      if (inarr) { done=(l ~ /\][[:space:]]*$/ && l !~ /\{[^}]*$/); print l; if (done) inarr=0 } }
  ' "$1" | sed -E 's/reason[[:space:]]*=[[:space:]]*"[^"]*"//g' | grep -oE '"[^"]+"' | tr -d '"' || true
}

jq -e '.bomFormat=="CycloneDX" and .specVersion=="1.6" and (.vulnerabilities|type=="array")' "$vex" >/dev/null \
  || { echo "vex-check: FAIL $vex is not a CycloneDX 1.6 document with vulnerabilities[]" >&2; exit 1; }

# shape
jq -r '.vulnerabilities[] | select(
    (.id|type)!="string" or (.id=="") or
    ((.analysis.state//"")=="") or ((.analysis.detail//"")=="") or
    ([.properties[]? | select(.name=="opensidian:expires") | .value
      | select(test("^[0-9]{4}-[0-9]{2}-[0-9]{2}$"))] | length) != 1
  ) | "entry \(.id // "<no id>"): needs id, analysis.state, analysis.detail and one opensidian:expires YYYY-MM-DD"' "$vex" >"$t/shape"
while IFS= read -r l; do err "$l"; done <"$t/shape"
jq -r '([.components[]?."bom-ref"]) as $c | .vulnerabilities[] | .id as $i
  | .affects[]?.ref | select(. as $r | $c | index($r) | not) | "entry \($i): affects ref \(.) is not a component"' "$vex" >"$t/refs"
while IFS= read -r l; do err "$l"; done <"$t/refs"

# expiry
jq -r --arg today "$today" '.vulnerabilities[] | .id as $i
  | .properties[]? | select(.name=="opensidian:expires") | select(.value < $today)
  | "entry \($i): expired on \(.value) (today \($today)); fix or re-triage"' "$vex" >"$t/exp"
while IFS= read -r l; do err "$l"; done <"$t/exp"

# id sets
# "resolved" = fixed in what we ship (glib-fix: path-patched crate, invisible to cargo audit/deny), so it
# is NOT an ignore and must NOT be in deny.toml / audit.toml; every other RustSec entry must be.
jq -r '.vulnerabilities[] | select((.source.name//"")=="RustSec" or (.id|startswith("RUSTSEC-"))) | select(.analysis.state != "resolved") | .id' "$vex" >"$t/vex.ids"
jq -r '.vulnerabilities[] | select(.analysis.state == "resolved") | .id' "$vex" >"$t/resolved.ids"
toml_ignore_ids "$deny"  >"$t/deny.ids"
toml_ignore_ids "$audit" >"$t/audit.ids"
for n in vex deny audit; do
  d=$(sort "$t/$n.ids" | uniq -d); [ -z "$d" ] || err "$n: duplicate ids: $(echo $d)"
  sort -u "$t/$n.ids" >"$t/$n.sorted"
done
[ -s "$t/vex.sorted" ] || err "VEX has no RustSec entries"
for n in deny audit; do
  if [ "$(cat "$t/vex.sorted")" != "$(cat "$t/$n.sorted")" ]; then
    only_vex=$(comm -23 "$t/vex.sorted" "$t/$n.sorted"); only_n=$(comm -13 "$t/vex.sorted" "$t/$n.sorted")
    err "VEX vs $n ignore list differ: only in VEX: [$(echo $only_vex)] only in $n: [$(echo $only_n)]"
  fi
done

while IFS= read -r r; do
  [ -n "$r" ] || continue
  for n in deny audit; do grep -qxF "$r" "$t/$n.ids" && err "$r is 'resolved' in the VEX but ignored in $n (a fix is not an ignore)"; done
  jq -e --arg r "$r" '[.vulnerabilities[] | select(.id == $r) | .analysis.detail | test("scripts/supply-chain/[a-z-]+\\.sh")] | all' "$vex" >/dev/null \
    || err "$r is 'resolved' but its detail names no scripts/supply-chain/*.sh that proves the fix"
done <"$t/resolved.ids"

if [ "$fail" -eq 0 ]; then
  echo "vex-check: PASS $(wc -l <"$t/vex.sorted") RustSec ids identical in VEX, deny.toml, audit.toml; $(jq '.vulnerabilities|length' "$vex") VEX entries, none expired (today $today)"
  jq -r '.vulnerabilities[] | "  \(.id)  \(.analysis.state)  expires \(.properties[]|select(.name=="opensidian:expires")|.value)"' "$vex"
fi
exit "$fail"
