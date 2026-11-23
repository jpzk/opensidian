#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# flatpak-sbom.sh fixture tests (G9: sha256 carried, fail closed; G3: runtime/module licenses), offline.
# Fixtures: fixtures/flatpak-sbom/{ok,nosha,badsha,nolicense}.yml
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); fx=$(cd "$(dirname "$0")" && pwd)/fixtures/flatpak-sbom
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
export SOURCE_DATE_EPOCH=1700000000
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
check() { if jq -e "$3" "$2" >/dev/null; then ok "$1"; else bad "$1"; fi; }
expect_fail() { local name=$1 pat=$2; shift 2
  set +e; "$@" >"$t/so" 2>"$t/se"; rc=$?; set -e
  if [ "$rc" -ne 0 ] && [ ! -s "$t/so" ] && grep -q -- "$pat" "$t/se"; then ok "$name: rc=$rc"
  else cat "$t/se"; bad "$name: rc=$rc stdout=$(wc -c <"$t/so")B (want !=0, empty stdout, '$pat')"; fi; }

F="$here/flatpak-sbom.sh"
"$F" "$fx/ok.yml" >"$t/ok.json"
check "CycloneDX 1.6, 1 platform + 1 library (dir source not a component)" "$t/ok.json" \
  '.specVersion == "1.6" and (.components | map(.type) | sort) == ["library","platform"]'
check "G9: archive sha256 carried verbatim as SHA-256 hash" "$t/ok.json" \
  '[.components[] | select(.name == "brotli") | .hashes] == [[{alg: "SHA-256", content: ("1" * 64)}]]'
check "brotli: pkg:github purl + bom-ref shared with the AppImage SBOM" "$t/ok.json" \
  '[.components[] | select(.name == "brotli")][0] | .purl == "pkg:github/google/brotli@v1.2.1" and ."bom-ref" == "brotli@1.2.1"'
check "G7: flatpak root (the app) licensed GPL-3.0-or-later from src-tauri/Cargo.toml" "$t/ok.json" \
  '.metadata.component.licenses == [{expression: "GPL-3.0-or-later"}]'
check "G3: brotli license MIT" "$t/ok.json" \
  '[.components[] | select(.name == "brotli") | .licenses] == [[{expression: "MIT"}]]'
check "G3: runtime license.name + license-note + hash-note (reason for no hash)" "$t/ok.json" \
  '[.components[] | select(.type == "platform")][0]
   | .licenses[0].license.name == "LicenseRef-flathub-runtime" and .hashes == null
     and ([.properties[].name] | index("opensidian:license-note") != null and index("opensidian:hash-note") != null)'
check "every component has a license" "$t/ok.json" 'all(.components[]; (.licenses | length) > 0)'
check "root dependsOn every component" "$t/ok.json" \
  '(.dependencies[0].dependsOn | sort) == ([.components[]."bom-ref"] | sort)'
expect_fail "G9: archive without sha256 fails closed" "without url/sha256" "$F" "$fx/nosha.yml"
expect_fail "G9: malformed sha256 fails closed" "without url/sha256" "$F" "$fx/badsha.yml"
expect_fail "module with no known license fails closed" "without a known license" "$F" "$fx/nolicense.yml"
check "G4 (#299): brotli carries a CPE so grype can match it (pkg:github has no OSV ecosystem)" "$t/ok.json" \
  '[.components[] | select(.name == "brotli")][0].cpe == "cpe:2.3:a:google:brotli:1.2.1:*:*:*:*:*:*:*"'
expect_fail "G4 (#299): github module without a CPE mapping fails closed" "without a CPE" "$F" "$fx/nocpe.yml"
expect_fail "missing manifest fails" "no " "$F" "$fx/absent.yml"
"$F" >"$t/repo.json"
check "repo manifest: brotli + runtime, all licensed" "$t/repo.json" \
  '(.components | length) == 2 and all(.components[]; (.licenses | length) > 0)'
[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
