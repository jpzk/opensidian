#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# macos-sbom.sh, offline: per-target and UNIVERSAL (aarch64 + x86_64 union) SBOMs from the repo's
# Cargo.lock (rust-sbom.sh) + synthetic `cargo metadata --filter-platform` resolves (no cargo here) +
# a stand-in vendor part + build-info files; negatives on scratch inputs. Never touches the checkout.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
export SOURCE_DATE_EPOCH=1700000000
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
S=$here/macos-sbom.sh

"$here/rust-sbom.sh" > "$t/rust.json"
jq -n '{bomFormat:"CycloneDX",specVersion:"1.6",version:1,
  metadata:{component:{type:"application","bom-ref":"opensidian-ui-vendor",name:"v"}},
  components:[{type:"library","bom-ref":"pkg:npm/pdfjs-dist@5.4.296",name:"pdfjs-dist",version:"5.4.296",purl:"pkg:npm/pdfjs-dist@5.4.296"}],
  dependencies:[{ref:"opensidian-ui-vendor",dependsOn:["pkg:npm/pdfjs-dist@5.4.296"]}]}' > "$t/vendor.json"
# two crates that only one target resolves each: A only on aarch64, B only on x86_64
A=$(jq -r '[.components[] | select(.name != "objc2")][0] | "\(.name)@\(.version)"' "$t/rust.json")
B=$(jq -r '[.components[] | select(.name != "objc2")][1] | "\(.name)@\(.version)"' "$t/rust.json")
meta() { # meta <excluded name@version> -> cargo-metadata-shaped resolve of every non-Linux lock crate
  jq --arg ex "$1" '([.metadata.component] + .components) | map(select(.name as $n | ["gtk","webkit2gtk","landlock"] | index($n) | not))
     | map("\(.name)@\(.version)") | map(select(. != $ex))
     | {packages: map({id: ., name: split("@")[0], version: split("@")[1]}), resolve: {nodes: map({id: .})}}' "$t/rust.json"
}
meta "$B" > "$t/meta-aarch64.json"; meta "$A" > "$t/meta-x86_64.json"
info() { printf 'TRIPLE=%s\nMINOS=%s\nSDK_VERSION=26.5\nBINARY_SDK=26.5\nXCODE_VERSION=26.6 (17F00)\nMACOS_BUILD_VERSION=macOS 26.1 (25B78)\n' "$1" "$2"; }
info aarch64-apple-darwin 11.0 > "$t/bi-aarch64.env"; info x86_64-apple-darwin 10.12 > "$t/bi-x86_64.env"
U=(-t aarch64-apple-darwin -m "$t/meta-aarch64.json" -b "$t/bi-aarch64.env" -t x86_64-apple-darwin -m "$t/meta-x86_64.json" -b "$t/bi-x86_64.env" -r "$t/rust.json" -j "$t/vendor.json" -v 0.9.6)

# single target (unchanged contract)
if p=$("$S" -t aarch64-apple-darwin -m "$t/meta-aarch64.json" -b "$t/bi-aarch64.env" -r "$t/rust.json" -j "$t/vendor.json" -o "$t/one" -v 0.9.6 2>"$t/err") \
   && [ "$(basename "$p")" = opensidian-0.9.6-macos-aarch64.cdx.json ] \
   && jq -e --arg a "$A" --arg b "$B" '[.components[] | "\(.name)@\(.version)"] | (index($a) != null) and (index($b) == null)' "$p" >/dev/null; then
  ok "single target: opensidian-0.9.6-macos-aarch64.cdx.json, only that target's crates"
else bad "single target: $(cat "$t/err")"; fi

# universal = union of both resolves, every purl once, minimum = lowest slice minimum
if p=$("$S" "${U[@]}" -o "$t/uni" 2>"$t/err"); then
  [ "$(basename "$p")" = opensidian-0.9.6-macos-universal.cdx.json ] && ok "universal: file name" || bad "universal: name $(basename "$p")"
  jq -e --arg a "$A" --arg b "$B" '[.components[] | "\(.name)@\(.version)"] | (index($a) != null) and (index($b) != null)' "$p" >/dev/null \
    && ok "universal: carries the aarch64-only ($A) and the x86_64-only ($B) crate" || bad "universal: union missing $A or $B"
  want=$(jq '[.components[] | select(.name as $n | ["gtk","webkit2gtk","landlock"] | index($n) | not)] | length' "$t/rust.json")
  got=$(jq '[.components[] | select(.purl // "" | startswith("pkg:cargo/"))] | length' "$p")
  [ "$got" -eq "$want" ] && ok "universal: $got cargo components = union of the resolves" || bad "universal: $got cargo components, want $want"
  jq -e '[.components[] | .purl // empty] | length == (unique | length)' "$p" >/dev/null && ok "universal: every purl once" || bad "universal: duplicate purl"
  jq -e '[.components[] | select(."bom-ref" == "os:apple-macos@10.12")] | length == 1' "$p" >/dev/null \
    && ok "universal: macOS minimum = lowest slice minimum (10.12)" || bad "universal: wrong macOS minimum"
  jq -e '[.components[] | select(."bom-ref" == "os:apple-webkit-wkwebview")] | length == 1' "$p" >/dev/null \
    && ok "universal: WKWebView OS component" || bad "universal: no WKWebView"
  jq -e '[.metadata.properties[] | select(.name == "opensidian:rust:target") | .value] | sort == ["aarch64-apple-darwin","x86_64-apple-darwin"]' "$p" >/dev/null \
    && ok "universal: both targets recorded" || bad "universal: targets not recorded"
  "$S" "${U[@]}" -o "$t/uni2" >/dev/null 2>&1 && [ "$(sha256sum < "$p")" = "$(sha256sum < "$t/uni2/$(basename "$p")")" ] && ok "universal: deterministic" || bad "universal: not deterministic"
else bad "universal: $(cat "$t/err")"; fi

# negatives
"$S" -t aarch64-apple-darwin -m "$t/meta-aarch64.json" -b "$t/bi-aarch64.env" -t aarch64-apple-darwin -m "$t/meta-aarch64.json" -b "$t/bi-aarch64.env" \
  -r "$t/rust.json" -j "$t/vendor.json" -o "$t/n1" >/dev/null 2>&1 && bad "same target twice accepted" || ok "same target twice refused"
"$S" -t aarch64-apple-darwin -m "$t/meta-aarch64.json" -t x86_64-apple-darwin -m "$t/meta-x86_64.json" -b "$t/bi-aarch64.env" \
  -r "$t/rust.json" -j "$t/vendor.json" -o "$t/n2" >/dev/null 2>&1 && bad "unpaired -b accepted" || ok "unpaired -t/-m/-b refused (usage)"
jq '.packages += [{id:"gtk@0.18.2",name:"gtk",version:"0.18.2"}] | .resolve.nodes += [{id:"gtk@0.18.2"}]' "$t/meta-x86_64.json" > "$t/meta-gtk.json"
"$S" -t aarch64-apple-darwin -m "$t/meta-aarch64.json" -b "$t/bi-aarch64.env" -t x86_64-apple-darwin -m "$t/meta-gtk.json" -b "$t/bi-x86_64.env" \
  -r "$t/rust.json" -j "$t/vendor.json" -o "$t/n3" >/dev/null 2>"$t/err" && bad "Linux crate in one slice accepted" || ok "Linux-only crate in the x86_64 slice refused: $(tail -1 "$t/err")"
jq '.packages += [{id:"notinlock@9.9.9",name:"notinlock",version:"9.9.9"}] | .resolve.nodes += [{id:"notinlock@9.9.9"}]' "$t/meta-aarch64.json" > "$t/meta-x.json"
"$S" -t aarch64-apple-darwin -m "$t/meta-x.json" -b "$t/bi-aarch64.env" -t x86_64-apple-darwin -m "$t/meta-x86_64.json" -b "$t/bi-x86_64.env" \
  -r "$t/rust.json" -j "$t/vendor.json" -o "$t/n4" >/dev/null 2>&1 && bad "crate missing from rust-sbom accepted" || ok "resolved crate missing from rust-sbom.sh refused"
sed 's/^MINOS=.*/MINOS=$(id)/' "$t/bi-x86_64.env" > "$t/bi-bad.env"
"$S" -t aarch64-apple-darwin -m "$t/meta-aarch64.json" -b "$t/bi-aarch64.env" -t x86_64-apple-darwin -m "$t/meta-x86_64.json" -b "$t/bi-bad.env" \
  -r "$t/rust.json" -j "$t/vendor.json" -o "$t/n5" >/dev/null 2>&1 && bad "malformed build-info accepted" || ok "malformed build-info refused"
exit "$fails"
