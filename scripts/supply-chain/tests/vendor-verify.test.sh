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
[ $rc = 0 ] && echo "vendor-verify.test: ALL PASS" || echo "vendor-verify.test: FAILURES"
exit $rc
