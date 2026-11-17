#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# rust-sbom.sh / flatpak-sbom.sh / sbom-merge.sh / vex-osv-config.sh, offline (no network, no dpkg):
# positives on the repo's Cargo.lock + flatpak manifest + a stand-in AppImage SBOM sharing brotli@1.2.1;
# negatives on scratch copies. Never touches the checkout.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
export SOURCE_DATE_EPOCH=1700000000
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
check() { local name=$1 f=$2 q=$3; if jq -e "$q" "$f" >/dev/null; then ok "$name"; else bad "$name"; fi; }
expect_fail() { local name=$1 pat=$2; shift 2
  set +e; "$@" >"$t/so" 2>"$t/se"; rc=$?; set -e
  if [ "$rc" -ne 0 ] && [ ! -s "$t/so" ] && grep -q -- "$pat" "$t/se"; then ok "$name: rc=$rc"
  else cat "$t/se"; bad "$name: rc=$rc stdout=$(wc -c <"$t/so")B (want !=0, empty stdout, '$pat')"; fi; }

n_lock=$(grep -c '^\[\[package\]\]' "$root/src-tauri/Cargo.lock")
"$here/rust-sbom.sh" >"$t/rust.json"
check "rust: CycloneDX 1.6, one comp per non-root Cargo.lock package ($((n_lock-1)))" "$t/rust.json" \
  ".bomFormat == \"CycloneDX\" and .specVersion == \"1.6\" and (.components | length) == $((n_lock-1))"
check "rust: every registry crate has the Cargo.lock sha256" "$t/rust.json" \
  '[.components[] | select(.name != "glib") | .hashes[0].content | test("^[0-9a-f]{64}$")] | all'
grep -A4 '^name = "serde"$' "$root/src-tauri/Cargo.lock" | sed -n 's/^checksum = "\(.*\)"/\1/p' >"$t/serde.sum"
check "rust: serde hash == Cargo.lock checksum" "$t/rust.json" \
  "[.components[] | select(.name == \"serde\") | .hashes[0].content] == [\"$(cat "$t/serde.sum")\"]"
check "rust: glib explicit pkg:cargo/glib@0.18.5, sha256 233daaf6…0ee5, pedigree b5a4071e" "$t/rust.json" \
  '[.components[] | select(.purl == "pkg:cargo/glib@0.18.5")] | length == 1 and .[0].hashes[0].content == "233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5" and (.[0].pedigree.patches[0].diff.url | test("b5a4071e"))'
check "rust: root opensidian is metadata.component, not a component" "$t/rust.json" \
  '.metadata.component.name == "opensidian" and ([.components[] | select(.name == "opensidian")] | length == 0)'
check "rust: every dependency ref resolves" "$t/rust.json" \
  '([.components[]."bom-ref"] + [.metadata.component."bom-ref"]) as $k | [.dependencies[] | .ref, .dependsOn[]] | unique | all(. as $r | $k | index($r))'

"$here/flatpak-sbom.sh" >"$t/flatpak.json"
check "flatpak: org.gnome.Platform 49 platform comp + brotli v1.2.1 sha256" "$t/flatpak.json" \
  '([.components[] | select(.type == "platform" and .name == "org.gnome.Platform" and .version == "49")] | length == 1)
   and ([.components[] | select(.purl == "pkg:github/google/brotli@v1.2.1") | .hashes[0].content] == ["98d35d11a78dc453aebb7d10c3748f3f73ff78c8267c0dc697b8631f67bf7e8f"])'

# stand-in for webkit-bundle-sbom.sh / vendor-sbom.sh output (both need dpkg / the npm registry)
jq -n '{bomFormat:"CycloneDX",specVersion:"1.6",version:1,
  metadata:{component:{type:"application","bom-ref":"opensidian-portable-appimage-bundle",name:"b"}},
  components:[{type:"library","bom-ref":"brotli@1.2.1",name:"brotli",version:"1.2.1",purl:"pkg:github/google/brotli@v1.2.1",
               properties:[{name:"opensidian:note",value:"appimage"}]},
              {type:"library","bom-ref":"deb:webkit2gtk@2.52.5-1",name:"webkit2gtk",version:"2.52.5-1",purl:"pkg:deb/ubuntu/webkit2gtk@2.52.5-1?arch=source&distro=ubuntu-26.04"}],
  dependencies:[{ref:"opensidian-portable-appimage-bundle",dependsOn:["brotli@1.2.1","deb:webkit2gtk@2.52.5-1"]}]}' >"$t/webkit.json"
jq -n '{bomFormat:"CycloneDX",specVersion:"1.6",version:1,
  metadata:{component:{type:"application","bom-ref":"opensidian-ui-vendor",name:"v"}},
  components:[{type:"library","bom-ref":"pkg:npm/pdfjs-dist@5.4.296",name:"pdfjs-dist",version:"5.4.296",purl:"pkg:npm/pdfjs-dist@5.4.296"}],
  dependencies:[{ref:"opensidian-ui-vendor",dependsOn:["pkg:npm/pdfjs-dist@5.4.296"]}]}' >"$t/vendor.json"

p=$("$here/sbom-merge.sh" -o "$t/out" "$t/vendor.json" "$t/rust.json" "$t/webkit.json" "$t/flatpak.json")
v=$(awk -F'"' '/^\[package\]/{p=1;next} /^\[/{p=0} p&&/^version *=/{print $2;exit}' "$root/src-tauri/Cargo.toml")
[ "$p" = "$t/out/opensidian-$v.cdx.json" ] && ok "merge: writes opensidian-$v.cdx.json" || bad "merge path $p"
want=$(( n_lock-1 + 1 + 2 + 2 + 4 ))   # rust + vendor + webkit + flatpak(runtime,brotli) + 4 sub-roots
want=$(( want - 1 ))                         # brotli@1.2.1 kept once
check "merge: CycloneDX 1.6 (jq -e), urn:uuid serial, $want unique components" "$p" \
  ".bomFormat == \"CycloneDX\" and .specVersion == \"1.6\" and (.serialNumber | test(\"^urn:uuid:\")) and (.components | length) == $want and ([.components[].\"bom-ref\"] | length == (unique | length))"
check "merge: brotli@1.2.1 once, both notes + flatpak sha256 kept" "$p" \
  '[.components[] | select(."bom-ref" == "brotli@1.2.1")] | length == 1 and (.[0].properties | length) == 2 and (.[0].hashes | length) == 1'
check "merge: release root dependsOn the 4 sub-roots" "$p" \
  '[.dependencies[] | select(.ref == .ref and .ref == "opensidian-release@'"$v"'") | .dependsOn | length] == [4]'
check "merge: glib + org.gnome.Platform + pdfjs-dist + webkit deb present" "$p" \
  '[.components[].purl] as $u | ["pkg:cargo/glib@0.18.5","pkg:npm/pdfjs-dist@5.4.296"] | all(. as $x | $u | index($x))
   and ([$u[] | select(. != null and startswith("pkg:generic/org.gnome.Platform@49"))] | length == 1)'
p2=$("$here/sbom-merge.sh" -o "$t/out2" "$t/vendor.json" "$t/rust.json" "$t/webkit.json" "$t/flatpak.json")
[ "$(sha256sum <"$p")" = "$(sha256sum <"$p2")" ] && ok "merge: reproducible bytes (SOURCE_DATE_EPOCH)" || bad "merge not reproducible"
p3=$("$here/sbom-merge.sh" -o "$t/out3" -v 1.2.3 "$t/rust.json")
[ "$(basename "$p3")" = opensidian-1.2.3.cdx.json ] && ok "merge: -v names the file" || bad "merge -v"

# negatives
cp "$root/src-tauri/Cargo.lock" "$t/git.lock"
printf '\n[[package]]\nname = "evil"\nversion = "0.1.0"\nsource = "git+https://example.invalid/evil#abc"\n' >>"$t/git.lock"
expect_fail "rust: git-sourced crate fails closed" "without a crates.io source" "$here/rust-sbom.sh" "$t/git.lock"
sed '0,/^checksum = /s/^checksum = .*/checksum = "nothex"/' "$root/src-tauri/Cargo.lock" >"$t/sum.lock"
expect_fail "rust: registry crate with a bad checksum" "without a sha256" "$here/rust-sbom.sh" "$t/sum.lock"
sed 's/^runtime-version:.*$//' "$root/packaging/flatpak/dev.koto.opensidian.yml" >"$t/m.yml"
expect_fail "flatpak: no runtime-version" "runtime/runtime-version" "$here/flatpak-sbom.sh" "$t/m.yml"
sed '/^        sha256: 98d3/d' "$root/packaging/flatpak/dev.koto.opensidian.yml" >"$t/m2.yml"
expect_fail "flatpak: archive without sha256" "without url/sha256" "$here/flatpak-sbom.sh" "$t/m2.yml"
jq '.components[0].version = "1.2.0"' "$t/webkit.json" >"$t/wk-conflict.json"
expect_fail "merge: same bom-ref, different version" "different purl/version/hashes" "$here/sbom-merge.sh" -o "$t/x" "$t/wk-conflict.json" "$t/flatpak.json"
jq '.components[0].hashes = [{alg:"SHA-256",content:("0"*64)}]' "$t/webkit.json" >"$t/wk-hash.json"
expect_fail "merge: same bom-ref, different sha256" "different purl/version/hashes" "$here/sbom-merge.sh" -o "$t/x" "$t/wk-hash.json" "$t/flatpak.json"
jq '.dependencies[0].dependsOn += ["ghost"]' "$t/vendor.json" >"$t/dangle.json"
expect_fail "merge: dangling dependency ref" "naming no component: ghost" "$here/sbom-merge.sh" -o "$t/x" "$t/dangle.json"
jq '.specVersion = "1.5"' "$t/vendor.json" >"$t/v15.json"
expect_fail "merge: non-1.6 input" "not a CycloneDX 1.6" "$here/sbom-merge.sh" -o "$t/x" "$t/v15.json"

"$here/vex-osv-config.sh" >"$t/osv.toml"
n_vex=$(jq '[.vulnerabilities[] | [.id] + [.references[]?.id] | unique[]] | length' "$root/supply-chain/vex.cdx.json")
[ "$(grep -c '^\[\[IgnoredVulns\]\]$' "$t/osv.toml")" -eq "$n_vex" ] && ok "osv config: $n_vex IgnoredVulns (ids + aliases)" || bad "osv config count"
grep -q '^id = "RUSTSEC-2026-0433"$' "$t/osv.toml" && grep -q '^effectiveUntil = 2027-01-15T23:59:59Z$' "$t/osv.toml" \
  && ok "osv config: effectiveUntil = VEX expiry" || bad "osv config ignoreUntil"
jq '.vulnerabilities[0].properties = []' "$root/supply-chain/vex.cdx.json" >"$t/vex-noexp.json"
expect_fail "osv config: VEX entry without expiry" "no opensidian:expires" "$here/vex-osv-config.sh" "$t/vex-noexp.json"

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "FAILURES"; exit 1; }
