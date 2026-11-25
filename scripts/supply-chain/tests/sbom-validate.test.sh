#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# sbom-validate.sh: a valid minimal SBOM passes (plain + --release), and every structural break
# (one per RED case) fails with rc=1 and names the problem.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
V="$here/sbom-validate.sh"
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
cat >"$t/good.json" <<'J'
{"bomFormat":"CycloneDX","specVersion":"1.6","serialNumber":"urn:uuid:01234567-89ab-cdef-0123-456789abcdef","version":1,
 "metadata":{"component":{"type":"application","bom-ref":"root","name":"opensidian","purl":"pkg:github/jpzk/opensidian@v1.0.0",
   "licenses":[{"expression":"GPL-3.0-or-later"}]},
   "properties":[{"name":"opensidian:sbom-local:commit","value":"0123456789abcdef0123456789abcdef01234567"},
     {"name":"opensidian:sbom-local:dirty","value":"false"},{"name":"opensidian:sbom-local:skipped","value":""}]},
 "components":[{"type":"library","bom-ref":"a","name":"a","scope":"required","licenses":[{"license":{"id":"MIT"}}],
     "hashes":[{"alg":"SHA-256","content":"0000000000000000000000000000000000000000000000000000000000000000"}]},
   {"type":"library","bom-ref":"b","name":"b","scope":"excluded","licenses":[{"license":{"name":"Custom"}}]}],
 "dependencies":[{"ref":"root","dependsOn":["a"]},{"ref":"a","dependsOn":["b"]}]}
J
green() { if bash "$V" "$@" 2>"$t/e"; then ok "green: $*"; else cat "$t/e"; bad "green: $*"; fi; }
red() { local name=$1 pat=$2 q=$3; shift 3; jq "$q" "$t/good.json" >"$t/r.json"
  set +e; bash "$V" "$@" "$t/r.json" 2>"$t/e"; rc=$?; set -e
  if [ "$rc" -eq 1 ] && grep -q -- "$pat" "$t/e"; then ok "RED $name (rc=1)"; else cat "$t/e"; bad "RED $name: rc=$rc, want 1 + '$pat'"; fi; }
green "$t/good.json"; green --release "$t/good.json"
red "bomFormat"            "bomFormat"               '.bomFormat = "SPDX"'
red "specVersion"          "specVersion"             '.specVersion = "1.5"'
red "serialNumber"         "serialNumber"            '.serialNumber = "x"'
red "version"              "version not"             '.version = 0'
red "no metadata.component" "no metadata.component"  'del(.metadata.component)'
red "empty components"     "components missing"      '.components = []'
red "component no bom-ref" "missing bom-ref"         'del(.components[0]."bom-ref")'
red "component no name"    "missing name"            'del(.components[1].name)'
red "bad type"             "bad type"                '.components[0].type = "lib"'
red "bad scope"            "bad scope"               '.components[0].scope = "dev"'
red "duplicate bom-ref"    "duplicate bom-ref a"     '.components[1]."bom-ref" = "a"'
red "dup with root"        "duplicate bom-ref root"  '.components[1]."bom-ref" = "root"'
red "dangling ref"         "names no component"      '.dependencies += [{"ref":"zzz"}]'
red "dangling dependsOn"   "dependsOn"               '.dependencies[1].dependsOn += ["zzz"]'
red "duplicate dep ref"    "duplicate dependencies"  '.dependencies += [{"ref":"a"}]'
red "bad sha256"           "SHA-256 not"             '.components[0].hashes[0].content = "ABC"'
red "malformed hash"       "malformed hash"          '.components[0].hashes[0] = {"alg":"SHA-256"}'
red "malformed license"    "malformed license"       '.components[0].licenses = [{"license":{"id":"MIT"},"expression":"MIT"}]'
red "license no id/name"   "malformed license"       '.components[0].licenses = [{"license":{}}]'
red "release: skipped"     "skipped part"            '.metadata.properties[2].value = "webkit"' --release
red "release: no skipped"  "no opensidian:sbom-local:skipped" 'del(.metadata.properties[2])' --release
red "release: dirty"       "dirty is"                '.metadata.properties[1].value = "true"' --release
red "release: commit"      "commit not"              '.metadata.properties[0].value = "HEAD"' --release
red "release: no purl"     "no purl"                 'del(.metadata.component.purl)' --release
red "two problems listed"  "2 problem"               '.bomFormat = "x" | .specVersion = "1"'
# nested components (CycloneDX components[].components[], e.g. code compiled into libwebkit2gtk, #299):
# their bom-refs are real refs (dependsOn may name them), take part in the uniqueness check, and each
# nested component gets the same per-component checks as a top-level one.
jq '.components[0].components = [{"type":"library","bom-ref":"a/embedded:e","name":"e","licenses":[{"license":{"id":"BSD-3-Clause"}}]}]
    | .dependencies[0].dependsOn += ["a/embedded:e"]' "$t/good.json" >"$t/nested.json"
green "$t/nested.json"; green --release "$t/nested.json"
red "nested duplicate bom-ref" "duplicate bom-ref b"  '.components[0].components = [{"type":"library","bom-ref":"b","name":"e"}]'
red "nested dup with root"     "duplicate bom-ref root" '.components[0].components = [{"type":"library","bom-ref":"root","name":"e"}]'
red "nested no name"           "missing name"         '.components[0].components = [{"type":"library","bom-ref":"a/embedded:e"}]'
red "nested bad type"          "bad type"             '.components[0].components = [{"type":"lib","bom-ref":"a/embedded:e","name":"e"}]'
red "nested not an array"      "components not an array" '.components[0].components = {"type":"library"}'
red "dangling nested-looking ref" "names no component" '.dependencies[0].dependsOn += ["a/embedded:zzz"]'
echo '[]' >"$t/arr.json"; set +e; bash "$V" "$t/arr.json" 2>/dev/null; rc=$?; set -e
[ "$rc" -eq 1 ] && ok "RED not an object (rc=1)" || bad "not an object rc=$rc"
set +e; bash "$V" 2>/dev/null; rc=$?; set -e; [ "$rc" -eq 2 ] && ok "usage rc=2" || bad "usage rc=$rc"
[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
