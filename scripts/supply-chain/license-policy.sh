#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# license-policy.sh — license gate over the merged release SBOM (G7). bash + jq.
# Allowed for every component: deny.toml [licenses] allow (read from deny.toml, so the cargo gate
# and this gate cannot drift) + license-policy.json extra_allow (ISC). Any other license passes
# only where a license-policy.json exception covers it: exact license string, match = prefixes
# of the component's purl or bom-ref (root: true = metadata.component), and a reason.
# Every license form is read: licenses[].expression (SPDX; AND/OR/WITH/parens, "/" = OR as cargo
# does; an OR passes if one side passes), licenses[].license.id and .license.name (exact).
# Several licenses[] entries on one component = all must pass.
# Part roots (the release root's direct dependsOn that are opensidian-owned: bom-ref opensidian@<ver>
# = the rust root crate, opensidian-* = the flatpak app) are opensidian's own and get the root
# exceptions. NO component may lack a license (no license-note excuse; aggregate grouping nodes are
# dropped by sbom-merge.sh instead, #249 item D); third-party libs the root dependsOn get no root exception.
# FAILS CLOSED (exit 1, every violation listed) on: a component or the root without licenses, a
# malformed licenses[] entry or expression, a license not allowed, a malformed policy (exception
# without reason/match), an unreadable deny.toml allow list, input that is not CycloneDX.
#
# Usage: license-policy.sh [-p <policy.json>] [-d <deny.toml>] <merged.cdx.json>
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
die() { echo "license-policy: $*" >&2; exit 1; }
usage() { echo "usage: license-policy.sh [-p <policy.json>] [-d <deny.toml>] <merged.cdx.json>" >&2; exit 2; }
policy=$here/license-policy.json deny=$root/deny.toml
while getopts p:d: o; do case $o in p) policy=$OPTARG;; d) deny=$OPTARG;; *) usage;; esac; done
shift $((OPTIND - 1))
[ $# -eq 1 ] || usage
sbom=$1
[ -f "$sbom" ] || die "$sbom: no such file"
[ -f "$deny" ] || die "$deny: no such file"
[ -f "$policy" ] || die "$policy: no such file"

# allow = [ "A", "B", ... ] inside [licenses] (multi-line or one line)
allow=$(awk '
  /^\[/ { sec = $0 }
  sec == "[licenses]" && /^allow *= *\[/ { on = 1 }
  on { line = $0; sub(/#.*/, "", line); buf = buf " " line; if (line ~ /\]/) { on = 0; done = 1 } }
  END { if (!done) exit 1; print buf }' "$deny" | grep -oE '"[^"]+"' | tr -d '"') \
  || die "$deny: no [licenses] allow = [...] list"
[ -n "$allow" ] || die "$deny: empty [licenses] allow list"

jq -e '.bomFormat == "CycloneDX" and (.specVersion | type == "string") and (.metadata.component | type == "object")' "$sbom" >/dev/null 2>&1 \
  || die "$sbom is not a CycloneDX SBOM with a metadata.component"

t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
jq -r --arg allow "$allow" --slurpfile pol "$policy" '
  # ---- SPDX expression evaluator: input string, output [failing license terms] ----
  def toks: gsub("/"; " OR ") | gsub("\\("; " ( ") | gsub("\\)"; " ) ")
            | [splits("[[:space:]]+")] | map(select(length > 0))
            | map(if (ascii_upcase | IN("AND", "OR", "WITH")) then ascii_upcase else . end);
  def term($ok; $l): if ($l | IN($ok[])) then [] else [$l] end;
  def orx($ok):
    def atom:
      if .t[.i] == "(" then (.i += 1 | orx($ok) | if .t[.i] != ")" then error("unbalanced parentheses") else .i += 1 end)
      else .t[.i] as $x
        | if $x == null or ($x | IN("AND", "OR", "WITH", ")")) then error("unexpected \($x // "end of expression")")
          elif .t[.i + 1] == "WITH" then
            (.t[.i + 2]) as $e
            | if $e == null or ($e | IN("AND", "OR", "WITH", "(", ")")) then error("WITH without an exception id")
              else .i += 3 | .f = term($ok; "\($x) WITH \($e)") end
          else .i += 1 | .f = term($ok; $x) end
      end;
    def andx: atom | until(.t[.i] != "AND"; .f as $a | .i += 1 | atom | .f = ($a + .f));
    andx | until(.t[.i] != "OR"; .f as $a | .i += 1 | andx
                                  | .f = (if ($a | length) == 0 or (.f | length) == 0 then [] else $a + .f end));
  def evalx($ok): {t: toks, i: 0} | orx($ok)
                  | if .i != (.t | length) then error("trailing \(.t[.i:] | join(" "))") else .f | unique end;
  # ---- per component ----
  def okfor($A; $X; $root): . as $c
    | $A + [ $X[] | select(
        (($root and .root == true))
        or (.match // [] | any(.[]; . as $m | (($c.purl // "") | startswith($m)) or (($c."bom-ref" // "") | startswith($m))))
      ) | .license ];
  def check($A; $X; $root; $part): . as $c | okfor($A; $X; $root or $part) as $ok
    | (if $root then "<root \(.name)>" else (."bom-ref" // .purl // .name // "?") end) as $id
    | if ((.licenses // []) | length) == 0 then
        "VIOL\t\($id)\tno license"
      else (.licenses[] |
        if (.expression | type) == "string" then
          (try (.expression | evalx($ok)) catch ("PARSE " + .)) as $r
          | if ($r | type) == "string" then "VIOL\t\($id)\tmalformed expression \(.expression | @json): \($r[6:])"
            elif ($r | length) > 0 then "VIOL\t\($id)\tnot allowed: \($r | join(", ")) (in \(.expression | @json))"
            else "OK\t\($id)\t\(.expression)" end
        elif (.license.id | type) == "string" then
          if (.license.id | IN($ok[])) then "OK\t\($id)\t\(.license.id)" else "VIOL\t\($id)\tnot allowed: \(.license.id)" end
        elif (.license.name | type) == "string" and .license.name != "" then
          if (.license.name | IN($ok[])) then "OK\t\($id)\t\(.license.name)" else "VIOL\t\($id)\tnot allowed: \(.license.name | @json) (license.name)" end
        else "VIOL\t\($id)\tmalformed licenses[] entry \(tojson)" end)
      end;
  .metadata.component as $c0 | $pol[0] as $p
  # ---- policy sanity (fail closed on a sloppy exception) ----
  | ([ ($p.extra_allow // [] | if type != "array" then "extra_allow is not an array" else (.[] | select(type != "string" or . == "") | "extra_allow has a bad entry") end),
       ($p.exceptions // [] | if type != "array" then "exceptions is not an array" else (to_entries[] | .key as $k | .value
         | (if (.license | type) != "string" or .license == "" then "exception #\($k): no license" else empty end),
           (if (.reason | type) != "string" or (.reason | length) < 10 then "exception #\($k) (\(.license)): no reason" else empty end),
           (if (.root != true) and ((.match // []) | length) == 0 then "exception #\($k) (\(.license)): matches nothing (no match prefixes, not root)" else empty end),
           (.match // [] | if type != "array" then "exception #\($k): match is not an array" else (.[] | select(type != "string" or . == "") | "exception #\($k) (\(.license)): empty match prefix") end)
         ) end) ] ) as $perr
  | if ($perr | length) > 0 then ($perr[] | "POLICY\t\(.)") else
    (($allow | split("\n") | map(select(length > 0))) + ($p.extra_allow // [])) as $A
  | ($p.exceptions // []) as $X
  | ([.dependencies // [] | .[] | select(.ref == $c0."bom-ref") | .dependsOn // [] | .[] | select(test("^opensidian([-@]|$)"))]) as $parts
  | (.metadata.component | check($A; $X; true; false)),
    (.components // [] | .[] | check($A; $X; false; (."bom-ref" | IN($parts[]))))
  end
' "$sbom" >"$t/res" 2>"$t/err" || die "jq failed: $(cat "$t/err")"

if grep -q '^POLICY' "$t/res"; then
  cut -f2- "$t/res" | sed "s|^|license-policy: $policy: |" >&2; exit 1
fi
n=$(grep -c . "$t/res" || true); [ "$n" -gt 0 ] || die "no license verdicts (empty result)"
comps=$(jq '(.components // []) | length' "$sbom")
if grep -q '^VIOL' "$t/res"; then
  grep '^VIOL' "$t/res" | cut -f2- | sed 's/\t/: /' | sed 's/^/license-policy: DENY /' >&2
  echo "license-policy: FAILED $(grep -c '^VIOL' "$t/res") violation(s) in $sbom ($comps components + root)" >&2
  exit 1
fi
echo "license-policy: PASS $sbom ($comps components + root, $(cut -f3 "$t/res" | sort -u | wc -l) distinct license terms)" >&2
cut -f3 "$t/res" | sort | uniq -c | sort -rn | sed 's/^/license-policy:   /' >&2
