#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# vendor-sbom.sh: on the repo it emits valid CycloneDX 1.6 (jq -e) built from vendor.lock.json,
# with mathjax, pdfjs-dist, prismjs, moment and every non-dev package of the committed
# CodeMirror package-lock.json; it emits NOTHING and exits non-zero when vendor-verify fails
# (flipped byte, VENDOR.md back, unlisted file). Scratch copies only (mktemp). Needs network
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
       and ([.components[] | .hashes | length > 0] | all)
       and ([.components[]."bom-ref"] | length == (unique | length))
       and (.dependencies[] | select(.ref == "file:ui/vendor/codemirror/cm6.js") | .dependsOn | length) == ($want | length)
     ' "$t/bom.json" >/dev/null; then
    ok "repo: CycloneDX 1.6, $(jq '.components|length' "$t/bom.json") components ($(jq length <<<"$want") from codemirror package-lock)"
  else bad "repo: SBOM content check (jq -e)"; fi
else bad "repo run rc!=0: $(tail -3 "$t/err")"; fi

fresh() { rm -rf "$t/v"; cp -a "$root/ui/vendor" "$t/v"; }
neg() { # $1 = label; expects rc!=0 and empty stdout
  if VENDOR_DIR=$t/v "$s" >"$t/o" 2>/dev/null; then bad "$1 accepted"
  elif [ -s "$t/o" ]; then bad "$1: rc!=0 but wrote stdout"
  else ok "$1 -> non-zero, no SBOM"; fi
}
fresh; printf 'x' | dd of="$t/v/pdfjs/pdf.min.mjs" bs=1 seek=1000 conv=notrunc status=none; neg "flipped byte in pdfjs/pdf.min.mjs"
fresh; echo prose >"$t/v/pdfjs/VENDOR.md"; neg "pdfjs/VENDOR.md added back"
fresh; echo x >"$t/v/stray.js"; neg "unlisted ui/vendor/stray.js"
exit $fails
