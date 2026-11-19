#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vendor-sbom.sh: on the repo it emits valid CycloneDX 1.6 (jq -e) built from vendor.lock.json,
# with mathjax, pdfjs-dist, prismjs, moment and every non-dev package of the committed
# CodeMirror package-lock.json; it emits NOTHING and exits non-zero when vendor-verify fails
# (flipped byte, VENDOR.md back, unlisted file). The 74 npm packages bundled in mermaid.min.js
# (LICENSES-bundled.txt) are components the mermaid component dependsOn; a malformed list fails
# closed. Scratch copies only (mktemp). Needs network
# (vendor-verify.sh talks to the npm registry).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
s=$here/vendor-sbom.sh; t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }

pl=$root/ui/vendor/codemirror/build/package-lock.json
want=$(jq -c '[.packages | to_entries[] | select(.key != "" and (.value.dev // false) == false)
               | {n: (.key | sub("^.*node_modules/"; "")), v: .value.version}] | sort' "$pl")
if SOURCE_DATE_EPOCH=0 "$s" >"$t/bom.json" 2>"$t/err"; then
  if jq -e --argjson want "$want" '
       .bomFormat == "CycloneDX" and .specVersion == "1.6" and .version == 1
       and (.metadata.timestamp == "1970-01-01T00:00:00Z")
       and ([.components[].purl // empty] as $p
            | all("pkg:npm/mathjax@3.2.2", "pkg:npm/pdfjs-dist@6.3.289",
                  "pkg:npm/prismjs@1.30.0", "pkg:npm/moment@2.31.0"; . as $x | $p | index($x)))
       and ([.components[] | select(.properties // [] | map(.value) | index("ui/vendor/codemirror/build/package-lock.json"))
             | {n: .name, v: .version}] | sort) == $want
       and ([.components[] | select(.name | test("esbuild"))] | length) == 0
       and ([.components[] | (.hashes // [] | length > 0) or (.properties // [] | map(.name) | index("opensidian:hash-note") != null)] | all)
       and ([.components[]."bom-ref"] | length == (unique | length))
       and (.dependencies[] | select(.ref == "file:ui/vendor/codemirror/cm6.js") | .dependsOn | length) == ($want | length)
     ' "$t/bom.json" >/dev/null; then
    ok "repo: CycloneDX 1.6, $(jq '.components|length' "$t/bom.json") components ($(jq length <<<"$want") from codemirror package-lock)"
  else bad "repo: SBOM content check (jq -e)"; fi
else bad "repo run rc!=0: $(tail -3 "$t/err")"; fi


# G5: the CodeMirror bundle ships its LICENSE (codemirror/LICENSE, commit-pinned upstream source,
# sha256 in the lock) and the bundle component names it
if jq -e --arg sha "$(sha256sum "$root/ui/vendor/codemirror/LICENSE" | cut -d' ' -f1)" '
     .components[] | select(."bom-ref" == "file:ui/vendor/codemirror/cm6.js")
     | (.licenses == [{license: {id: "MIT"}}])
       and ([.properties[] | select(.name == "opensidian:file") | .value
             | select(test("^ui/vendor/codemirror/LICENSE sha256:" + $sha + " <- https://raw.githubusercontent.com/[^/]+/[^/]+/[0-9a-f]{40}/LICENSE$"))] | length) == 1
   ' "$t/bom.json" >/dev/null; then ok "G5: cm6.js component carries codemirror/LICENSE sha256 + pinned source"
else bad "G5: cm6.js component has no codemirror/LICENSE file property"; fi

jq -e 'any(.metadata.component.properties[]; .name == "opensidian:license-note")' "$t/bom.json" >/dev/null \
  && ok "G7: ui/vendor aggregate root carries opensidian:license-note" || bad "G7: ui/vendor root has no license-note"

# G4: the npm packages bundled inside mermaid.min.js (LICENSES-bundled.txt) are components,
# each with version + license + registry URL, and the mermaid component dependsOn all of them
bl=$root/ui/vendor/mermaid/LICENSES-bundled.txt
nb=$(grep -cE '^={20,}$' "$bl")
if [ -s "$t/bom.json" ] && jq -e --argjson nb "$nb" '
     [.components[] | select(.properties // [] | map(.name) | index("opensidian:bundled_in"))] as $b
     | ($b | length) == $nb and $nb == 74
     and ($b | all(.version != null and (.purl | startswith("pkg:npm/"))
                   and (.licenses | length) == 1
                   and (.externalReferences[0].url | startswith("https://registry.npmjs.org/"))))
     and ((.dependencies[] | select(.ref == "pkg:npm/mermaid@11.17.2") | .dependsOn | sort) == ($b | map(."bom-ref") | sort))
     and ($b[] | select(.name == "dompurify") | .licenses) == [{expression: "MPL-2.0 OR Apache-2.0"}]
     and ($b[] | select(.name == "chevrotain") | .licenses == [{license: {id: "Apache-2.0"}}]
              and (.properties | map(.name) | index("opensidian:version-note")))
     and ([$b[] | select(.name == "d3-array") | .version] | sort) == ["2.12.1", "3.2.4"]
     and ([$b[] | select(.purl == "pkg:npm/%40braintree/sanitize-url@7.1.2")] | length) == 1
     and ([.dependencies[].dependsOn[]] - [.components[]."bom-ref"] | length) == 0
   ' "$t/bom.json" >/dev/null; then ok "mermaid: $nb bundled npm components, mermaid dependsOn all, licenses (id + expression)"
else bad "mermaid bundled components (jq -e)"; fi

fresh() { rm -rf "$t/v"; cp -a "$root/ui/vendor" "$t/v"; }
neg() { # $1 = label; expects rc!=0 and empty stdout
  if VENDOR_DIR=$t/v "$s" >"$t/o" 2>"$t/e"; then bad "$1 accepted"
  elif [ -s "$t/o" ]; then bad "$1: rc!=0 but wrote stdout"
  else ok "$1 -> non-zero, no SBOM ($(tail -1 "$t/e" | sed "s|$t/v|<v>|"))"; fi
}
fresh; printf 'x' | dd of="$t/v/pdfjs/pdf.min.mjs" bs=1 seek=1000 conv=notrunc status=none; neg "flipped byte in pdfjs/pdf.min.mjs"
fresh; echo prose >"$t/v/pdfjs/VENDOR.md"; neg "pdfjs/VENDOR.md added back"
fresh; echo x >"$t/v/stray.js"; neg "unlisted ui/vendor/stray.js"
mb=$t/v/mermaid/LICENSES-bundled.txt
fresh; sed -i 's/^d3-array@3.2.4  (SPDX: ISC)$/d3-array 3.2.4 (ISC)/' "$mb"; neg "mermaid bundled: malformed package line"
fresh; sed -i 's|^source: https://registry.npmjs.org/cytoscape/-/cytoscape-3.34.0.tgz$|source: https://evil.example/cytoscape-3.34.0.tgz|' "$mb"; neg "mermaid bundled: non-registry source URL"
fresh; sed -i '1s/(mermaid 11.17.2)/(mermaid 11.0.0)/' "$mb"; neg "mermaid bundled: title names another mermaid version"
fresh; printf '%s
' '=====================================================================' 'stray@1.0.0  (SPDX: MIT)' >>"$mb"; neg "mermaid bundled: truncated last block"
fresh; rm "$mb"; neg "mermaid bundled: LICENSES-bundled.txt missing"
exit $fails
