#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# rust-sbom.sh fixture tests (G2 licenses, G9 fail-closed sources, G11 scope), offline.
# Fixture: fixtures/rust-sbom/{Cargo.lock, metadata.json (= cargo metadata --all-features),
# resolve.json (= cargo metadata --filter-platform x86_64-unknown-linux-gnu, default features)}:
#   required: glib (path-patched), libc, serde, serde_derive (build edge)
#   excluded: sha2 (dev-only), winapi (not in the target resolve), libloading (feature not enabled)
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); fx=$(cd "$(dirname "$0")" && pwd)/fixtures/rust-sbom
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

R="$here/rust-sbom.sh"
"$R" "$fx/Cargo.lock" >"$t/plain.json"
check "plain: 7 components, no licenses, no scope (lock-only mode unchanged)" "$t/plain.json" \
  '(.components | length) == 7 and all(.components[]; .licenses == null and .scope == null)'

"$R" --metadata "$fx/metadata.json" --resolve "$fx/resolve.json" --target x86_64-unknown-linux-gnu "$fx/Cargo.lock" >"$t/full.json"
check "G2: every component has a license" "$t/full.json" 'all(.components[]; (.licenses | length) > 0)'
check "G2: SPDX expression carried (libc MIT OR Apache-2.0)" "$t/full.json" \
  '[.components[] | select(.name == "libc") | .licenses[0].expression] == ["MIT OR Apache-2.0"]'
check "G2: legacy 'MIT/Apache-2.0' normalised to 'MIT OR Apache-2.0' (serde)" "$t/full.json" \
  '[.components[] | select(.name == "serde") | .licenses[0].expression] == ["MIT OR Apache-2.0"]'
check "G2: license_file crate -> license.name + note naming the file (winapi)" "$t/full.json" \
  '[.components[] | select(.name == "winapi")][0] | .licenses[0].license.name == "LicenseRef-cargo-license-file"
     and any(.properties[]; .name == "opensidian:license-note" and (.value | test("LICENSE-MIT")))'
check "G2: path-patched glib gets its license, pedigree kept" "$t/full.json" \
  '[.components[] | select(.name == "glib")][0] | .licenses[0].expression == "MIT" and (.pedigree.patches | length) == 1'
check "G2: root metadata.component license GPL-3.0-or-later" "$t/full.json" \
  '.metadata.component.licenses == [{expression: "GPL-3.0-or-later"}]'
check "G11: required = glib libc serde serde_derive" "$t/full.json" \
  '[.components[] | select(.scope == "required") | .name] | sort == ["glib","libc","serde","serde_derive"]'
check "G11: excluded = libloading sha2 winapi, each with a reason, none dropped" "$t/full.json" \
  '([.components[] | select(.scope == "excluded") | .name] | sort == ["libloading","sha2","winapi"])
   and all(.components[] | select(.scope == "excluded"); any(.properties[]; .name == "opensidian:scope-reason"))
   and (.components | length) == 7'
check "G11: target + counts in metadata.properties" "$t/full.json" \
  '(.metadata.properties | from_entries) as $p | $p["opensidian:rust:target"] == "x86_64-unknown-linux-gnu"
     and $p["opensidian:rust:required"] == "4" and $p["opensidian:rust:excluded"] == "3"'
check "hashes still the Cargo.lock checksums" "$t/full.json" \
  '[.components[] | select(.name == "serde") | .hashes[0].content] == ["3333333333333333333333333333333333333333333333333333333333333333"]'

# ---- fail closed ----
sed 's|^\(checksum = "6666.*\)$|\1|; /^name = "winapi"/{n;n;s|^source = .*|source = "git+https://github.com/x/winapi?rev=abc#abc"|}' "$fx/Cargo.lock" >"$t/git.lock"
grep -q '^source = "git+' "$t/git.lock" || bad "fixture: git source not planted"
expect_fail "G9: git source fails closed" "without a crates.io source" "$R" "$t/git.lock"
awk '/^name = "sha2"/{print; getline; print; getline; next} {print}' "$fx/Cargo.lock" | grep -v '^checksum = "5555' >"$t/path.lock"
grep -A2 '^name = "sha2"' "$t/path.lock" | grep -q '^source' && bad "fixture: sha2 source not removed"
expect_fail "G9: path (source-less) crate other than glib fails closed" "without a crates.io source" "$R" "$t/path.lock"
sed '/^checksum = "1111/d' "$fx/Cargo.lock" >"$t/nosum.lock"
expect_fail "G9: registry crate without checksum fails closed" "without a sha256 checksum" "$R" "$t/nosum.lock"
jq '.packages |= map(select(.name != "serde"))' "$fx/metadata.json" >"$t/miss.json"
expect_fail "G2: crate missing from cargo metadata fails closed" "missing from cargo metadata: serde@1.0.219" \
  "$R" --metadata "$t/miss.json" "$fx/Cargo.lock"
jq '.packages |= map(if .name == "libc" then .license = null else . end)' "$fx/metadata.json" >"$t/nolic.json"
expect_fail "G2: crate with neither license nor license_file fails closed" "neither license nor license_file: libc" \
  "$R" --metadata "$t/nolic.json" "$fx/Cargo.lock"
jq 'del(.resolve)' "$fx/resolve.json" >"$t/nores.json"
expect_fail "G11: --resolve without a resolve graph fails closed" "no resolve graph" \
  "$R" --resolve "$t/nores.json" "$fx/Cargo.lock"
echo '{}' >"$t/notmeta.json"
expect_fail "--metadata that is not cargo metadata fails closed" "not cargo metadata" \
  "$R" --metadata "$t/notmeta.json" "$fx/Cargo.lock"

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "FAILURES"; exit 1; }
