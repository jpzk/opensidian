#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# tests/vendor-verify.test.sh — vendor-verify.sh must PASS on the repo and FAIL on each tampering, done in
# a scratch copy of ui/vendor (the repo is never modified). Network: npm registry.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
v=$here/vendor-verify.sh
scratch=$(mktemp -d); trap 'rm -rf "$scratch"' EXIT
fresh() { rm -rf "$scratch/vendor"; cp -a "$root/ui/vendor" "$scratch/vendor"; }
rc=0
expect() { # expect <0|nonzero> <label>
  local want=$1 label=$2 got=0
  VENDOR_DIR=$scratch/vendor "$v" >"$scratch/out" 2>&1 || got=$?
  if { [ "$want" = 0 ] && [ "$got" = 0 ]; } || { [ "$want" != 0 ] && [ "$got" != 0 ]; }; then
    echo "PASS  $label (rc=$got)"; grep FAIL "$scratch/out" | head -3 | sed 's/^/      /' || true
  else echo "FAIL  $label (rc=$got, wanted $want)"; sed 's/^/      /' "$scratch/out"; rc=1; fi
}
fresh; expect 0 "clean copy of ui/vendor"
fresh; f=$scratch/vendor/pdfjs/pdf.min.mjs
b=$(od -An -tx1 -j 5000 -N1 "$f" | tr -d ' '); printf "\\x$(printf %02x $((0x$b ^ 1)))" | dd of="$f" bs=1 seek=5000 conv=notrunc status=none
expect 1 "flipped byte in pdfjs/pdf.min.mjs"
fresh; jq '(.libraries[]|select(.name=="pdfjs-dist")|.version)="6.3.288" | (.libraries[]|select(.name=="pdfjs-dist")|.purl)="pkg:npm/pdfjs-dist@6.3.288"' \
  "$scratch/vendor/vendor.lock.json" >"$scratch/l" && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"
expect 1 "wrong version in lock (pdfjs-dist 6.3.288)"
fresh; printf 'x\n' >"$scratch/vendor/prism/extra.js"; expect 1 "extra unlisted file prism/extra.js"
fresh; printf 'x\n' >"$scratch/vendor/codemirror/build/evil.js"; expect 1 "extra unlisted file codemirror/build/evil.js"
fresh; jq '.packages["node_modules/crelt"].integrity="sha512-AAAA"' "$scratch/vendor/codemirror/build/package-lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/codemirror/build/package-lock.json"; expect 1 "CodeMirror package-lock integrity tampered"
fresh; jq '(.bundles[0].packages)|=.[1:]' "$scratch/vendor/vendor.lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; expect 1 "bundle package dropped from lock (CodeMirror)"
fresh; printf '# prose\n' >"$scratch/vendor/pdfjs/VENDOR.md"; expect 1 "VENDOR.md added back (pdfjs/VENDOR.md)"
fresh; jq '(.libraries[]|select(.name=="prismjs")|.published)="2025-03-11T00:00:00.000Z"' "$scratch/vendor/vendor.lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; expect 1 "lock publish date != registry time (prismjs)"
fresh; jq '(.libraries[]|select(.name=="pdfjs-dist")|.added)="2026-09-15"' "$scratch/vendor/vendor.lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; expect 1 "six-week rule (pdfjs-dist added 17 days after publish)"
# age_exemption: the only way past the six-week rule (mermaid 12.3.0, security fix). Every malformed form fails.
mmsel='(.libraries[]|select(.name=="mermaid"))'
fresh; jq "del($mmsel.age_exemption)" "$scratch/vendor/vendor.lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; expect 1 "six-week rule: mermaid 12.3.0 without its age_exemption"
fresh; jq "$mmsel.age_exemption.reason=\"convenience\"" "$scratch/vendor/vendor.lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; expect 1 "age_exemption: reason is not security-fix"
fresh; jq "$mmsel.age_exemption.advisories=[]" "$scratch/vendor/vendor.lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; expect 1 "age_exemption: no advisory named"
fresh; jq "$mmsel.age_exemption.advisories+=[\"see issue\"] | $mmsel.age_exemption.max_days=0" "$scratch/vendor/vendor.lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; expect 1 "age_exemption: non-advisory id + unknown key"
fresh; printf 'Prism.languages.x={};\n' >>"$scratch/vendor/prism/prism-components.js"; expect 1 "appended bytes in prism/prism-components.js (#186 concat)"
fresh; jq '(.libraries[]|select(.name=="prismjs")|.files[]|select(.concat)|.concat)|=(.[1:]+.[:1])' "$scratch/vendor/vendor.lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; expect 1 "concat order changed in lock (prism-components.js)"
fresh; jq '(.libraries[]|select(.name=="prismjs")|.files[]|select(.concat)|.concat[0])="package/../evil.js"' "$scratch/vendor/vendor.lock.json" >"$scratch/l" \
  && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; expect 1 "unsafe concat path (prism-components.js)"
fresh; printf '# prose\n' >"$scratch/vendor/fluent/VENDOR.md"; expect 1 "VENDOR.md added back (fluent/VENDOR.md)"
# files[] "source" entries (fluent/LICENSE: not in the npm tarball, pinned to an upstream commit URL).
# The clean copy above is the good-source fixture; it must be locked by source, not allowed_unlocked.
srcsel='(.libraries[]|select(.name=="@fluent/bundle")|.files[]|select(.path=="fluent/LICENSE"))'
if jq -e "$srcsel|.source|test(\"^https://raw.githubusercontent.com/[^/]+/[^/]+/[0-9a-f]{40}/\")" "$root/ui/vendor/vendor.lock.json" >/dev/null \
   && ! jq -e '.allowed_unlocked|index("fluent/LICENSE")' "$root/ui/vendor/vendor.lock.json" >/dev/null; then
  echo "PASS  fluent/LICENSE locked by commit-pinned source + sha256, not in allowed_unlocked"
else echo "FAIL  fluent/LICENSE is not locked by a commit-pinned source"; rc=1; fi
lockmut() { jq "$1" "$scratch/vendor/vendor.lock.json" >"$scratch/l" && mv "$scratch/l" "$scratch/vendor/vendor.lock.json"; }
fresh; lockmut "$srcsel.sha256=\"$(printf '0%.0s' {1..64})\""; expect 1 "source entry: wrong sha256 (fluent/LICENSE)"
fresh; lockmut "$srcsel.source|=sub(\"^https:\";\"http:\")"; expect 1 "source entry: non-https URL (fluent/LICENSE)"
fresh; lockmut "$srcsel.source|=sub(\"/[0-9a-f]{40}/\";\"/main/\")"; expect 1 "source entry: branch ref instead of commit (fluent/LICENSE)"
fresh; lockmut "$srcsel.source|=sub(\"raw.githubusercontent.com\";\"example.com\")"; expect 1 "source entry: other host (fluent/LICENSE)"
fresh; lockmut "$srcsel.tarball_path=\"package/LICENSE\""; expect 1 "source entry with tarball_path too (fluent/LICENSE)"
fresh; printf ' ' >>"$scratch/vendor/fluent/LICENSE"; expect 1 "vendored fluent/LICENSE differs from the fetched source bytes"
fresh; lockmut "(.libraries[]|select(.name==\"@fluent/bundle\")|.files)|=map(select(.path!=\"fluent/LICENSE\"))"
expect 1 "fluent/LICENSE dropped from the lock (no longer covered)"
# G5: bundles[].files[] — codemirror/LICENSE, pinned to an upstream commit, only the source form allowed
cmsel='(.bundles[]|select(.name=="codemirror")|.files[]|select(.path=="codemirror/LICENSE"))'
if jq -e "$cmsel|.source|test(\"^https://raw.githubusercontent.com/[^/]+/[^/]+/[0-9a-f]{40}/LICENSE$\")" "$root/ui/vendor/vendor.lock.json" >/dev/null \
   && [ "$(jq -r "$cmsel|.sha256" "$root/ui/vendor/vendor.lock.json")" = "$(sha256sum "$root/ui/vendor/codemirror/LICENSE" | cut -d' ' -f1)" ]; then
  echo "PASS  codemirror/LICENSE locked by commit-pinned source + sha256 of the vendored bytes"
else echo "FAIL  codemirror/LICENSE is not locked by a commit-pinned source"; rc=1; fi
fresh; lockmut "$cmsel.sha256=\"$(printf '0%.0s' {1..64})\""; expect 1 "bundle file: wrong sha256 (codemirror/LICENSE)"
fresh; lockmut "$cmsel.source|=sub(\"/[0-9a-f]{40}/\";\"/main/\")"; expect 1 "bundle file: branch ref instead of commit (codemirror/LICENSE)"
fresh; lockmut "$cmsel|=(del(.source)|.tarball_path=\"package/LICENSE\")"; expect 1 "bundle file: tarball_path form (bundles have no tarball)"
fresh; printf 'x\n' >>"$scratch/vendor/codemirror/LICENSE"; expect 1 "vendored codemirror/LICENSE differs from the fetched source bytes"
fresh; lockmut '(.bundles[]|select(.name=="codemirror")|.files)|=[]'; expect 1 "codemirror/LICENSE dropped from the lock (no longer covered)"
fresh; rm "$scratch/vendor/codemirror/LICENSE"; expect 1 "codemirror/LICENSE missing from the tree"
[ $rc = 0 ] && echo "vendor-verify.test: ALL PASS" || echo "vendor-verify.test: FAILURES"
exit $rc
