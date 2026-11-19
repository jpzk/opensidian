#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# sbom-local.sh (local release SBOM, operator 2026-11-19), offline: runs in a scratch tree (git repo)
# with the REAL rust-sbom.sh / flatpak-sbom.sh / sbom-merge.sh on the repo's Cargo.lock + flatpak
# manifest and a STUB vendor-sbom.sh (the real one needs the npm registry); webkit (needs dpkg/podman)
# is --skip'ped. Checks: merged output + .sha256, skip recorded in metadata, and fail-closed on a
# failing / empty part, a missing flatpak input, unknown --skip, everything skipped.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
export SOURCE_DATE_EPOCH=1700000000
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
check() { local name=$1 f=$2 q=$3; if jq -e "$q" "$f" >/dev/null 2>&1; then ok "$name"; else bad "$name"; fi; }

s=$t/tree; mkdir -p "$s/scripts/supply-chain" "$s/src-tauri" "$s/packaging/flatpak" "$s/packaging/ci"
cp "$here/sbom-local.sh" "$here/rust-sbom.sh" "$here/flatpak-sbom.sh" "$here/sbom-merge.sh" "$s/scripts/supply-chain/"
cp "$here/glib-vendor-check.sh" "$s/scripts/supply-chain/"   # rust-sbom.sh reads CRATE_SHA from it
cp "$root/src-tauri/Cargo.lock" "$root/src-tauri/Cargo.toml" "$s/src-tauri/"
cp "$root/packaging/flatpak/dev.koto.opensidian.yml" "$s/packaging/flatpak/"
echo '{"version":"9.8.7"}' >"$s/src-tauri/tauri.conf.json"
cat >"$s/scripts/supply-chain/vendor-sbom.sh" <<'STUB'
#!/usr/bin/env bash
case "${STUB_VENDOR:-ok}" in
  fail) echo "vendor-verify: FAIL (stub)" >&2; exit 1 ;;
  empty) echo '{"bomFormat":"CycloneDX","specVersion":"1.6","metadata":{"component":{"type":"library","bom-ref":"ui-vendor","name":"ui/vendor"}},"components":[]}' ;;
  *) echo '{"bomFormat":"CycloneDX","specVersion":"1.6","version":1,"metadata":{"component":{"type":"library","bom-ref":"ui-vendor","name":"ui/vendor"}},"components":[{"type":"library","bom-ref":"pkg:npm/stub@1.0.0","name":"stub","version":"1.0.0","purl":"pkg:npm/stub@1.0.0"}],"dependencies":[{"ref":"ui-vendor","dependsOn":["pkg:npm/stub@1.0.0"]}]}' ;;
esac
STUB
chmod +x "$s/scripts/supply-chain/"*.sh
git -C "$s" init -q && git -C "$s" add -A && git -C "$s" -c user.name=t -c user.email=t@t commit -q -m fixture
L="$s/scripts/supply-chain/sbom-local.sh"
run() { set +e; "$@" >"$t/so" 2>"$t/se"; rc=$?; set -e; }
expect_fail() { local name=$1 want=$2 pat=$3; shift 3; rm -rf "$t/out"; run "$@"
  if [ "$rc" -eq "$want" ] && ! ls "$t/out"/*.cdx.json >/dev/null 2>&1 && grep -q -- "$pat" "$t/se"; then ok "$name: rc=$rc, nothing written"
  else cat "$t/se"; bad "$name: rc=$rc (want $want, no SBOM, stderr '$pat')"; fi; }

# ---- positive: rust + vendor(stub) + flatpak, webkit skipped ----
run bash "$L" -o "$t/out" --skip webkit
F=$t/out/opensidian-9.8.7.cdx.json
if [ "$rc" -eq 0 ] && [ "$(tail -1 "$t/so")" = "$F" ]; then ok "skip webkit: rc=0, prints $F"; else cat "$t/se"; bad "skip webkit: rc=$rc stdout=$(tail -1 "$t/so")"; fi
n_lock=$(grep -c '^\[\[package\]\]' "$s/src-tauri/Cargo.lock")
check "merged CycloneDX 1.6, root opensidian-release@9.8.7" "$F" \
  '.bomFormat == "CycloneDX" and .specVersion == "1.6" and .metadata.component."bom-ref" == "opensidian-release@9.8.7"'
check "all $((n_lock-1)) cargo crates + pkg:npm/stub present" "$F" \
  "([.components[] | select(.purl // \"\" | startswith(\"pkg:cargo/\"))] | length) == $((n_lock-1)) and ([.components[] | select(.purl == \"pkg:npm/stub@1.0.0\")] | length) == 1"
check "skip recorded: opensidian:sbom-local:skipped == webkit, parts == rust vendor flatpak" "$F" \
  '(.metadata.properties | from_entries) as $p | $p["opensidian:sbom-local:skipped"] == "webkit" and $p["opensidian:sbom-local:parts"] == "rust vendor flatpak" and $p["opensidian:sbom-local:dirty"] == "false"'
check "commit recorded" "$F" "(.metadata.properties | from_entries)[\"opensidian:sbom-local:commit\"] == \"$(git -C "$s" rev-parse HEAD)\""
if (cd "$t/out" && sha256sum -c --quiet opensidian-9.8.7.cdx.json.sha256); then ok ".sha256 matches"; else bad ".sha256 does not match"; fi
grep -q 'INCOMPLETE' "$t/se" && ok "skip warns INCOMPLETE" || bad "no INCOMPLETE warning on skip"

# ---- fail closed ----
expect_fail "vendor part fails" 1 "part vendor failed" env STUB_VENDOR=fail bash "$L" -o "$t/out" --skip webkit
expect_fail "vendor part empty" 1 "part vendor is not a non-empty" env STUB_VENDOR=empty bash "$L" -o "$t/out" --skip webkit
mv "$s/packaging/flatpak/dev.koto.opensidian.yml" "$t/manifest.yml"
expect_fail "flatpak input missing" 1 "part flatpak: input" bash "$L" -o "$t/out" --skip webkit
rm -rf "$t/out"; run bash "$L" -o "$t/out" --skip webkit --skip flatpak
check "flatpak input missing + --skip flatpak: rc=0, skipped == flatpak webkit" "$t/out/opensidian-9.8.7.cdx.json" \
  '(.metadata.properties | from_entries)["opensidian:sbom-local:skipped"] == "flatpak webkit"'
mv "$t/manifest.yml" "$s/packaging/flatpak/dev.koto.opensidian.yml"
expect_fail "unknown --skip part" 2 "unknown part" bash "$L" -o "$t/out" --skip bogus
expect_fail "bad --webkit mode" 2 "usage" bash "$L" -o "$t/out" --webkit bogus
expect_fail "every part skipped" 1 "every part skipped" bash "$L" -o "$t/out" --skip rust --skip vendor --skip flatpak --skip webkit

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
