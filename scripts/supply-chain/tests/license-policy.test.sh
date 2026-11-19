#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# license-policy.sh (G7), offline: a green fixture covering every license form (expression with
# OR/AND/WITH/parens/"/", license.id, license.name, scoped exceptions, root), then RED cases built
# from it with jq: each must exit non-zero and name the offending component.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fx=$here/tests/fixtures/license-policy/green.cdx.json
gate=$here/license-policy.sh
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
expect_pass() { local name=$1; shift
  set +e; "$@" >"$t/so" 2>"$t/se"; rc=$?; set -e
  if [ "$rc" -eq 0 ] && grep -q 'license-policy: PASS' "$t/se"; then ok "$name"; else cat "$t/se"; bad "$name: rc=$rc"; fi; }
expect_fail() { local name=$1 pat=$2; shift 2
  set +e; "$@" >"$t/so" 2>"$t/se"; rc=$?; set -e
  if [ "$rc" -ne 0 ] && grep -qF -- "$pat" "$t/se"; then ok "$name: rc=$rc"
  else cat "$t/se"; bad "$name: rc=$rc (want !=0 and '$pat')"; fi; }
mut() { jq "$1" "$fx" >"$t/m.json"; echo "$t/m.json"; }

expect_pass "green fixture (every license form, scoped exceptions, root)" "$gate" "$fx"

# the allow list is deny.toml's, plus ISC; nothing else globally
want=$(printf '%s\n' "Apache-2.0" "Apache-2.0 WITH LLVM-exception" "BSD-3-Clause" "MIT" "MPL-2.0" "Unicode-3.0" "Zlib")
got=$(awk '/^\[/{s=$0} s=="[licenses]" && /^allow *= *\[/{on=1} on{print; if(/\]/) on=0}' "$root/deny.toml" | grep -oE '"[^"]+"' | tr -d '"')
[ "$got" = "$want" ] && ok "deny.toml allow list is the one this test expects (update both deliberately)" || bad "deny.toml allow changed: $got"
[ "$(jq -c .extra_allow "$here/license-policy.json")" = '["ISC"]' ] && ok "extra_allow == [ISC]" || bad "extra_allow"
jq -e '[.exceptions[] | (.reason | length >= 10) and ((.match // []) | length > 0 or .root == true)] | all' "$here/license-policy.json" >/dev/null \
  && ok "every shipped exception has a reason and a scope" || bad "policy exception without reason/scope"
sed 's/^  "MIT",$//' "$root/deny.toml" >"$t/deny-nomit.toml"
expect_fail "RED: MIT removed from deny.toml -> MIT crates denied (allow list is read, not copied)" "DENY pkg:github/google/brotli@v1.2.1: not allowed: MIT" "$gate" -d "$t/deny-nomit.toml" "$fx"

# RED: disallowed licenses
expect_fail "RED: GPL-3.0-or-later on a dependency crate (exception is opensidian-only)" "DENY pkg:cargo/serde@1.0.0: not allowed: GPL-3.0-or-later" \
  "$gate" "$(mut '(.components[] | select(.name == "serde") | .licenses) = [{expression: "GPL-3.0-or-later"}]')"
expect_fail "RED: license.id SSPL-1.0" "DENY pkg:npm/d3-array@3.2.4: not allowed: SSPL-1.0" \
  "$gate" "$(mut '(.components[] | select(.name == "d3-array") | .licenses) = [{license: {id: "SSPL-1.0"}}]')"
expect_fail "RED: AND with a disallowed side" "not allowed: GPL-2.0-only" \
  "$gate" "$(mut '(.components[] | select(.name == "serde") | .licenses) = [{expression: "MIT AND GPL-2.0-only"}]')"
expect_fail "RED: OR where no side is allowed" "not allowed: BUSL-1.1, SSPL-1.0" \
  "$gate" "$(mut '(.components[] | select(.name == "serde") | .licenses) = [{expression: "(SSPL-1.0 OR BUSL-1.1)"}]')"
expect_fail "RED: WITH an unlisted exception" "not allowed: Apache-2.0 WITH Foo-exception" \
  "$gate" "$(mut '(.components[] | select(.name == "wasi") | .licenses) = [{expression: "Apache-2.0 WITH Foo-exception"}]')"
expect_fail "RED: deb-scoped LGPL on a crate" "DENY pkg:cargo/serde@1.0.0: not allowed: LGPL-2.1-or-later" \
  "$gate" "$(mut '(.components[] | select(.name == "serde") | .licenses) = [{license: {id: "LGPL-2.1-or-later"}}]')"
expect_fail "RED: AGPL on a deb (not an exception)" "not allowed: AGPL-3.0-or-later" \
  "$gate" "$(mut '(.components[] | select(.name == "webkit2gtk") | .licenses) += [{license: {id: "AGPL-3.0-or-later"}}]')"
expect_fail "RED: unmapped deb license.name" 'not allowed: "BSD-like" (license.name)' \
  "$gate" "$(mut '(.components[] | select(.name == "libfoo") | .licenses) = [{license: {name: "BSD-like"}}]')"
# per-source-package deb exceptions (DEP-5 terms reviewed per package): they cover only the named package
expect_pass "per-package deb exception covers its package (webkit2gtk BSD-3-clause-apple, MPL-1.1)" \
  "$gate" "$(mut '(.components[] | select(.name == "webkit2gtk") | .licenses) += [{license: {name: "BSD-3-clause-apple"}}, {license: {id: "MPL-1.1"}}]')"
expect_fail "RED: a per-package deb exception does not cover another deb" 'DENY pkg:deb/ubuntu/libfoo@1.0?arch=source&distro=resolute: not allowed: "BSD-3-clause-apple" (license.name)' \
  "$gate" "$(mut '(.components[] | select(.name == "libfoo") | .licenses) += [{license: {name: "BSD-3-clause-apple"}}]')"
expect_fail "RED: GPL-2.0-only is excepted per package only (util-linux programs), not for every deb" "not allowed: GPL-2.0-only" \
  "$gate" "$(mut '(.components[] | select(.name == "libfoo") | .licenses) += [{license: {id: "GPL-2.0-only"}}]')"
expect_fail "RED: LicenseRef-flathub-runtime outside the flatpak runtime" "DENY pkg:github/google/brotli@v1.2.1: not allowed: \"LicenseRef-flathub-runtime\" (license.name)" \
  "$gate" "$(mut '(.components[] | select(.name == "brotli") | .licenses) = [{license: {name: "LicenseRef-flathub-runtime"}}]')"
expect_fail "RED: root with a non-GPL license" "DENY <root opensidian>: not allowed: \"Proprietary\" (license.name)" \
  "$gate" "$(mut '.metadata.component.licenses = [{license: {name: "Proprietary"}}]')"
# part roots (the release root's direct dependsOn: rust root crate, flatpak app, ui/vendor, AppImage bundle)
expect_fail "RED: aggregate part without license and without license-note" "DENY opensidian-ui-vendor: no license" \
  "$gate" "$(mut '(.components[] | select(."bom-ref" == "opensidian-ui-vendor")) |= del(.properties)')"
expect_fail "RED: license-note does not excuse a non-part component" "DENY pkg:cargo/serde@1.0.0: no license" \
  "$gate" "$(mut '(.components[] | select(.name == "serde")) |= (del(.licenses) | .properties = [{name: "opensidian:license-note", value: "trust me, it is fine"}])')"
expect_fail "RED: a part that is no longer a release dependency loses the root exception" "DENY opensidian-flatpak: not allowed: GPL-3.0-or-later" \
  "$gate" "$(mut '.dependencies[0].dependsOn -= ["opensidian-flatpak"]')"
# RED: missing / malformed
expect_fail "RED: component without licenses" "DENY pkg:cargo/serde@1.0.0: no license" "$gate" "$(mut '(.components[] | select(.name == "serde")) |= del(.licenses)')"
expect_fail "RED: component with empty licenses[]" "DENY pkg:cargo/serde@1.0.0: no license" "$gate" "$(mut '(.components[] | select(.name == "serde") | .licenses) = []')"
expect_fail "RED: root without licenses" "DENY <root opensidian>: no license" "$gate" "$(mut '.metadata.component |= del(.licenses)')"
expect_fail "RED: unbalanced expression" "malformed expression" "$gate" "$(mut '(.components[] | select(.name == "serde") | .licenses) = [{expression: "(MIT OR Apache-2.0"}]')"
expect_fail "RED: dangling operator" "malformed expression" "$gate" "$(mut '(.components[] | select(.name == "serde") | .licenses) = [{expression: "MIT OR"}]')"
expect_fail "RED: licenses[] entry with neither form" "malformed licenses[] entry" "$gate" "$(mut '(.components[] | select(.name == "serde") | .licenses) = [{license: {url: "x"}}]')"
expect_fail "RED: excluded-scope crate is still checked" "DENY pkg:cargo/memchr@2.0.0: not allowed: GPL-2.0-only" \
  "$gate" "$(mut '(.components[] | select(.name == "memchr") | .licenses) = [{expression: "GPL-2.0-only"}]')"
expect_fail "RED: not CycloneDX" "is not a CycloneDX SBOM" "$gate" "$(mut '.bomFormat = "SPDX"')"
# RED: sloppy policy
jq '.exceptions[0].reason = ""' "$here/license-policy.json" >"$t/p1.json"
expect_fail "RED: policy exception without reason" "no reason" "$gate" -p "$t/p1.json" "$fx"
jq '.exceptions[1].match = []' "$here/license-policy.json" >"$t/p2.json"
expect_fail "RED: policy exception without scope" "matches nothing" "$gate" -p "$t/p2.json" "$fx"
printf '[licenses]\nversion = 2\n' >"$t/deny-empty.toml"
expect_fail "RED: deny.toml without allow list" "no [licenses] allow" "$gate" -d "$t/deny-empty.toml" "$fx"

# every violation is listed, not just the first
"$gate" "$(mut '(.components[] | select(.name == "serde" or .name == "d3-array") | .licenses) = [{license: {id: "SSPL-1.0"}}]')" 2>"$t/se" && bad "two violations: passed" \
  || { [ "$(grep -c '^license-policy: DENY' "$t/se")" -eq 2 ] && grep -q 'FAILED 2 violation' "$t/se" && ok "two violations both reported" || { cat "$t/se"; bad "two violations"; }; }

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "FAILURES"; exit 1; }
