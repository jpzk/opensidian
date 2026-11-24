#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# sbom-validate.sh [--release] <sbom.cdx.json> — structural CycloneDX 1.6 check (jq only).
# No pinned CycloneDX JSON-schema validator is available without a new dependency (no python/node,
# no new binaries), so this checks the structure the release relies on:
#   bomFormat == "CycloneDX", specVersion == "1.6", serialNumber urn:uuid, version integer >= 1,
#   metadata.component with type + name + string bom-ref, components[] each with a known type,
#   a name and a string bom-ref (nested components[].components[] too, recursively); bom-refs UNIQUE
#   across metadata.component + components at every depth;
#   dependencies[]: unique ref, every ref and dependsOn entry names an existing bom-ref;
#   hashes[] alg/content well formed (SHA-256 = 64 hex); licenses[] entries are exactly one of
#   {expression} / {license:{id|name}}; scope (if set) required|optional|excluded.
# --release adds: metadata.properties opensidian:sbom-local:skipped present and EMPTY, dirty == false,
#   commit a 40-hex sha, and metadata.component.purl set.
# Every problem is listed. Exit 0 = valid; 1 = invalid; 2 = usage.
set -euo pipefail
export LC_ALL=C
rel=false
[ "${1:-}" = --release ] && { rel=true; shift; }
[ $# -eq 1 ] || { echo "usage: sbom-validate.sh [--release] <sbom.cdx.json>" >&2; exit 2; }
f=$1
jq -e 'type == "object"' "$f" >/dev/null 2>&1 || { echo "sbom-validate: $f is not a JSON object" >&2; exit 1; }
errs=$(jq -r --argjson rel "$rel" '
  def types: ["application","framework","library","container","platform","operating-system","device",
              "device-driver","firmware","file","machine-learning-model","data","cryptographic-asset"];
  def comp_errs($where):
    (if (.type | IN(types[])) then empty else "\($where): bad type \(.type | tojson)" end),
    (if (.name | type) == "string" and .name != "" then empty else "\($where): missing name" end),
    (if (."bom-ref" | type) == "string" and ."bom-ref" != "" then empty else "\($where): missing bom-ref" end),
    (if .scope == null or (.scope | IN("required","optional","excluded")) then empty else "\($where): bad scope \(.scope | tojson)" end),
    ((.hashes // []) | if type == "array" then .[] else "BAD" end
       | if . == "BAD" then "\($where): hashes not an array"
         elif (.alg | type) != "string" or (.content | type) != "string" then "\($where): malformed hash \(tojson)"
         elif .alg == "SHA-256" and (.content | test("^[0-9a-f]{64}$") | not) then "\($where): SHA-256 not 64 lowercase hex"
         else empty end),
    ((.licenses // []) | if type == "array" then .[] else "BAD" end
       | if . == "BAD" then "\($where): licenses not an array"
         elif (keys == ["expression"] and (.expression | type) == "string")
           or (keys == ["license"] and ((.license.id // .license.name) | type) == "string") then empty
         else "\($where): malformed license entry \(tojson)" end),
    # nested components (CycloneDX components[].components[], #299): same checks, recursively
    (if .components == null then empty
     elif (.components | type) != "array" then "\($where): nested components not an array"
     else (.components | to_entries[] | .key as $i | .value
           | comp_errs("\($where).components[\($i)] \(."bom-ref" // .name // "?")")) end);
  # every component object at any nesting depth (well-formed arrays only; malformed ones are reported above)
  def allcomps: (.components // [] | if type == "array" then .[] else empty end) | select(type == "object") | ., allcomps;
  [ (if .bomFormat == "CycloneDX" then empty else "bomFormat is \(.bomFormat | tojson)" end),
    (if .specVersion == "1.6" then empty else "specVersion is \(.specVersion | tojson)" end),
    (if (.serialNumber // "" | test("^urn:uuid:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")) then empty else "serialNumber not urn:uuid" end),
    (if (.version | type) == "number" and .version >= 1 then empty else "version not an integer >= 1" end),
    (if (.metadata.component | type) == "object" then (.metadata.component | comp_errs("metadata.component")) else "no metadata.component" end),
    (if (.components | type) == "array" and (.components | length) > 0 then (.components | to_entries[] | .key as $i | .value | comp_errs("components[\($i)] \(."bom-ref" // .name // "?")")) else "components missing or empty" end),
    ([.metadata.component."bom-ref", (allcomps | ."bom-ref")] | map(select(type == "string")) | group_by(.) | map(select(length > 1)[0]) | .[] | "duplicate bom-ref \(.)"),
    ([.metadata.component."bom-ref", (allcomps | ."bom-ref")] | map(select(type == "string"))) as $refs
    | ((.dependencies // []) | if type == "array" then . else [] end) as $d
    | (($d | map(.ref) | group_by(.) | map(select(length > 1)[0]) | .[] | "duplicate dependencies ref \(.)"),
       ($d[] | .ref as $r
         | (if ($r | IN($refs[])) then empty else "dependency ref \($r | tojson) names no component" end),
           ((.dependsOn // [])[] | select(IN($refs[]) | not) | "\($r) dependsOn \(tojson) names no component"))),
    (if $rel then
       ((.metadata.properties // []) | map({key: .name, value: .value}) | from_entries) as $p
       | (if $p | has("opensidian:sbom-local:skipped") | not then "release: no opensidian:sbom-local:skipped property"
          elif $p["opensidian:sbom-local:skipped"] != "" then "release: skipped part(s) \($p["opensidian:sbom-local:skipped"])" else empty end),
         (if $p["opensidian:sbom-local:dirty"] == "false" then empty else "release: dirty is \($p["opensidian:sbom-local:dirty"] | tojson)" end),
         (if ($p["opensidian:sbom-local:commit"] // "" | test("^[0-9a-f]{40}$")) then empty else "release: commit not a 40-hex sha" end),
         (if (.metadata.component.purl | type) == "string" then empty else "release: metadata.component has no purl" end)
     else empty end)
  ] | .[]' "$f") || { echo "sbom-validate: jq failed on $f" >&2; exit 1; }
if [ -n "$errs" ]; then
  printf '%s\n' "$errs" | sed 's/^/sbom-validate: /' >&2
  echo "sbom-validate: INVALID $f ($(printf '%s\n' "$errs" | wc -l) problem(s))" >&2; exit 1
fi
echo "sbom-validate: OK $f ($(jq '.components | length' "$f") components$([ "$rel" = true ] && echo ', release checks'))" >&2
