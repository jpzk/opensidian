#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# macos-sbom.sh — CycloneDX 1.6 SBOM of ONE macOS build target (.github/workflows/macos.yml).
# bash + jq only; reuses the Linux generators unchanged (rust-sbom.sh, vendor-sbom.sh, sbom-merge.sh):
#   rust    rust-sbom.sh (every Cargo.lock crate, Cargo.lock sha256) FILTERED to the crates in
#           `cargo metadata --locked --filter-platform <triple> --format-version 1` .resolve.nodes
#           (normal + build + dev edges for that target: a conservative superset of what links).
#           A resolved crate missing from rust-sbom.sh output, or a filtered SBOM that still carries
#           a Linux-only crate (webkit2gtk, gtk, landlock) or lacks the macOS ones (objc2), fails closed.
#   vendor  vendor-sbom.sh (ui/vendor, proven by vendor-verify.sh first) as given.
#   os      the system WebKit the app renders with: WKWebView from WebKit.framework, shipped and
#           patched by Apple with macOS (NOT bundled). Recorded as an OS-provided component
#           (no purl: OSV has no ecosystem for it, so it is listed, not scanned), together with the
#           minimum macOS the binary declares (LC_BUILD_VERSION minos) and the build SDK.
# Usage: macos-sbom.sh -t <triple> -m <cargo-metadata.json> -r <rust.cdx.json> -j <vendor.cdx.json>
#                      -b <build-info.env> -o <dir> [-v <version>]
#   build-info.env: KEY=VALUE lines from the macOS build job (MINOS, SDK_VERSION, XCODE_VERSION,
#   MACOS_BUILD_VERSION). Prints the output path; SOURCE_DATE_EPOCH fixes timestamps.
# Exit 0 = written; 1 = bad input / self-check failed; 2 = usage.
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd)
die() { echo "macos-sbom: $*" >&2; exit 1; }
usage() { echo "usage: macos-sbom.sh -t triple -m metadata.json -r rust.cdx.json -j vendor.cdx.json -b build-info.env -o dir [-v ver]" >&2; exit 2; }
tgt= meta= rust= vend= info= out= ver=
while getopts t:m:r:j:b:o:v: o; do
  case $o in t) tgt=$OPTARG;; m) meta=$OPTARG;; r) rust=$OPTARG;; j) vend=$OPTARG;; b) info=$OPTARG;;
             o) out=$OPTARG;; v) ver=$OPTARG;; *) usage;; esac
done
[ -n "$tgt" ] && [ -n "$meta" ] && [ -n "$rust" ] && [ -n "$vend" ] && [ -n "$info" ] && [ -n "$out" ] || usage
case "$tgt" in aarch64-apple-darwin|x86_64-apple-darwin) ;; *) die "target '$tgt' is not a macOS triple" ;; esac
for f in "$meta" "$rust" "$vend" "$info"; do [ -s "$f" ] || die "missing/empty input $f"; done
kv() { # KEY from build-info.env, must be non-empty and from a safe charset
  local v; v=$(sed -n "s/^$1=//p" "$info" | tail -1)
  printf '%s' "$v" | grep -Eqx '[0-9A-Za-z._ ()-]{1,64}' || die "build-info $1='$v' missing or malformed"
  printf '%s' "$v"
}
minos=$(kv MINOS); sdk=$(kv SDK_VERSION); xcode=$(kv XCODE_VERSION); osb=$(kv MACOS_BUILD_VERSION)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT

# 1. Rust: keep only the crates cargo resolves for $tgt
jq -e '.resolve.nodes | type == "array" and length > 0' "$meta" >/dev/null || die "$meta has no .resolve.nodes (cargo metadata without --no-deps?)"
jq -r '(.resolve.nodes | map(.id)) as $ids | .packages[] | select(.id as $i | $ids | index($i)) | "\(.name)@\(.version)"' "$meta" \
  | sort -u > "$t/resolved"
[ -s "$t/resolved" ] || die "no resolved packages in $meta"
jq -n --arg tgt "$tgt" --rawfile res "$t/resolved" --slurpfile sbs "$rust" '
  ($res | split("\n") | map(select(length > 0))) as $keep
  | $sbs[0] as $sb
  | ($sb.metadata.component."bom-ref") as $root
  | ($sb.components | map(select("\(.name)@\(.version)" as $k | $keep | index($k)))) as $c
  | ([$c[]."bom-ref"] + [$root]) as $refs
  | ($keep - ([$c[] | "\(.name)@\(.version)"] + ["\($sb.metadata.component.name)@\($sb.metadata.component.version)"])) as $missing
  | if ($missing | length) > 0 then error("resolved for \($tgt) but not in rust-sbom.sh output: \($missing | join(", "))") else . end
  | $sb
  | .metadata.component.properties += [{name: "opensidian:rust:target", value: $tgt},
                                       {name: "opensidian:rust:scope", value: "cargo metadata --filter-platform \($tgt) .resolve.nodes (normal+build+dev)"}]
  | .components = $c
  | .dependencies = [ .dependencies[] | select(.ref as $r | $refs | index($r))
                      | .dependsOn = [ .dependsOn[] | select(. as $d | $refs | index($d)) ] ]
' > "$t/rust.cdx.json" 2>"$t/err" || die "$(sed 's/^jq: error[^:]*: //' "$t/err")"
for bad in webkit2gtk gtk landlock; do
  jq -e --arg n "$bad" '[.components[] | select(.name == $n)] | length == 0' "$t/rust.cdx.json" >/dev/null \
    || die "Linux-only crate '$bad' survived the $tgt filter"
done
jq -e '[.components[] | select(.name == "objc2")] | length >= 1' "$t/rust.cdx.json" >/dev/null \
  || die "no objc2 in the $tgt graph (not a macOS resolve?)"

# 2. OS-provided WebKit (WKWebView), not bundled
jq -n --arg minos "$minos" --arg sdk "$sdk" --arg xcode "$xcode" --arg osb "$osb" --arg tgt "$tgt" '
  { bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
    metadata: { component: { type: "operating-system", "bom-ref": "os:apple-macos@\($minos)", name: "macOS",
                             version: ">= \($minos)", supplier: {name: "Apple Inc."},
                             properties: [{name: "opensidian:os:minimum", value: $minos},
                                          {name: "opensidian:os:build-host", value: $osb},
                                          {name: "opensidian:build:sdk", value: "macOS SDK \($sdk) (Xcode \($xcode))"},
                                          {name: "opensidian:rust:target", value: $tgt}] } },
    components: [ { type: "framework", "bom-ref": "os:apple-webkit-wkwebview", name: "WebKit.framework (WKWebView)",
                    version: "provided by macOS >= \($minos)", supplier: {name: "Apple Inc."},
                    scope: "required",
                    description: "System WebKit linked from /System/Library/Frameworks/WebKit.framework; not bundled, updated by macOS / Safari updates. No purl: not covered by the OSV scan.",
                    properties: [{name: "opensidian:provided-by", value: "operating-system"},
                                 {name: "opensidian:bundled", value: "false"},
                                 {name: "opensidian:api", value: "WKWebView (tauri wry)"}] } ],
    dependencies: [ {ref: "os:apple-macos@\($minos)", dependsOn: ["os:apple-webkit-wkwebview"]},
                    {ref: "os:apple-webkit-wkwebview", dependsOn: []} ] }' > "$t/os.cdx.json"

# 3. merge (same merger as the Linux release SBOM)
mkdir -p "$out"
m=$("$here/sbom-merge.sh" -o "$t/merged" ${ver:+-v "$ver"} "$t/rust.cdx.json" "$vend" "$t/os.cdx.json")
[ -s "$m" ] || die "sbom-merge.sh wrote nothing"
arch=${tgt%%-*}
dest="$out/$(basename "$m" .cdx.json)-macos-$arch.cdx.json"
jq --arg tgt "$tgt" '.metadata.properties = ((.metadata.properties // []) + [{name: "opensidian:rust:target", value: $tgt}])' "$m" > "$dest"
jq -e '.bomFormat == "CycloneDX" and .specVersion == "1.6"
       and ([.components[] | select(."bom-ref" == "os:apple-webkit-wkwebview")] | length == 1)
       and ([.components[] | select(.purl // "" | startswith("pkg:cargo/"))] | length > 0)
       and ([.components[] | select(.purl // "" | startswith("pkg:npm/"))] | length > 0)' "$dest" >/dev/null \
  || die "merged SBOM failed self-check"
printf '%s\n' "$dest"
