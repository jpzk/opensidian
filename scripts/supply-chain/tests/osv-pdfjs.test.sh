#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# osv-install.sh installs the latest osv-scanner (API digest verified, version logged) and
# that scanner flags a CycloneDX 1.6 test SBOM carrying pkg:npm/pdfjs-dist@4.1.392
# (CVE-2024-4367 / GHSA-wgrm-67xf-hhpq) with exit code 1 (= vulnerabilities found, not a
# tool error). Scratch dir only (mktemp). Needs network (GitHub API + api.osv.dev).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }

if bin=$("$here/osv-install.sh" "$t/bin" 2>"$t/install.log"); then
  cat "$t/install.log"
  grep -Eq '^osv-scanner release: v[0-9]' "$t/install.log" && grep -Eq '^osv-scanner sha256:  [0-9a-f]{64}$' "$t/install.log" \
    && ok "install: latest release + sha256 logged" || bad "install: version/digest not logged"
else
  cat "$t/install.log"; bad "osv-install.sh failed"; exit 1
fi

# osv-scanner detects CycloneDX by the *.cdx.json suffix
cat >"$t/pdfjs.cdx.json" <<'JSON'
{"bomFormat":"CycloneDX","specVersion":"1.6","version":1,
 "components":[{"type":"library","bom-ref":"pkg:npm/pdfjs-dist@4.1.392","name":"pdfjs-dist",
   "version":"4.1.392","purl":"pkg:npm/pdfjs-dist@4.1.392"}]}
JSON
set +e
"$bin" scan source --format json -L "$t/pdfjs.cdx.json" >"$t/out.json" 2>"$t/err"
rc=$?
set -e
[ "$rc" -eq 1 ] && ok "pdfjs-dist 4.1.392: rc=1 (vulnerabilities found)" || { cat "$t/err"; bad "pdfjs-dist 4.1.392: rc=$rc, want 1"; }
if jq -e '[.results[].packages[] | select(.package.name == "pdfjs-dist")
           | .vulnerabilities[] | .id, (.aliases // [])[]] | index("CVE-2024-4367")' "$t/out.json" >/dev/null 2>&1; then
  ok "pdfjs-dist 4.1.392: CVE-2024-4367 reported"
else
  bad "pdfjs-dist 4.1.392: CVE-2024-4367 not in osv-scanner output"
fi

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
