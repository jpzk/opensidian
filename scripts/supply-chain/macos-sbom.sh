#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# macos-sbom.sh — CycloneDX 1.6 SBOM of the macOS build (.github/workflows/macos.yml): ONE apple
# target, or the UNIVERSAL app (one fat Mach-O, aarch64 + x86_64 slices) when -t/-m/-b are given
# once per target. bash + jq only; reuses the Linux generators unchanged (rust-sbom.sh,
# vendor-sbom.sh, sbom-merge.sh):
#   rust    rust-sbom.sh (every Cargo.lock crate, Cargo.lock sha256) FILTERED, per target, to the
#           crates in `cargo metadata --locked --filter-platform <triple> --format-version 1`
#           .resolve.nodes (normal + build + dev edges for that target: a conservative superset of
#           what links). Universal = the UNION of the per-target graphs, deduplicated (sbom-merge.sh
#           keys by bom-ref and fails closed on a bom-ref with two purls; the result is checked to
#           carry every purl once). Per target: a resolved crate missing from rust-sbom.sh output, or
#           a filtered SBOM that still carries a Linux-only crate (webkit2gtk, gtk, landlock) or
#           lacks the macOS ones (objc2), fails closed.
#   vendor  vendor-sbom.sh (ui/vendor, proven by vendor-verify.sh first) as given.
#   os      the system WebKit the app renders with: WKWebView from WebKit.framework, shipped and
#           patched by Apple with macOS (NOT bundled). Recorded as an OS-provided component
#           (no purl: OSV has no ecosystem for it, so it is listed, not scanned), together with the
#           minimum macOS (LC_BUILD_VERSION minos, or LC_VERSION_MIN_MACOSX version on x86_64) and
#           the build SDK. Universal: the OS minimum is the LOWEST slice minimum (= the app's
#           LSMinimumSystemVersion); every slice's own minimum is kept as a property.
# Usage: macos-sbom.sh -t <triple> -m <cargo-metadata.json> -b <build-info.env> [-t .. -m .. -b ..]
#                      -r <rust.cdx.json> -j <vendor.cdx.json> -o <dir> [-v <version>]
#   -t/-m/-b pair up by position. One target -> opensidian-<ver>-macos-<arch>.cdx.json; the two
#   targets aarch64-apple-darwin + x86_64-apple-darwin -> opensidian-<ver>-macos-universal.cdx.json.
#   build-info.env: KEY=VALUE lines from the macOS build job (MINOS, SDK_VERSION, XCODE_VERSION,
#   MACOS_BUILD_VERSION). Prints the output path; SOURCE_DATE_EPOCH fixes timestamps.
# Exit 0 = written; 1 = bad input / self-check failed; 2 = usage.
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd)
die() { echo "macos-sbom: $*" >&2; exit 1; }
usage() { echo "usage: macos-sbom.sh -t triple -m metadata.json -b build-info.env [-t .. -m .. -b ..] -r rust.cdx.json -j vendor.cdx.json -o dir [-v ver]" >&2; exit 2; }
tgts=() metas=() infos=() rust= vend= out= ver=
while getopts t:m:r:j:b:o:v: o; do
  case $o in t) tgts+=("$OPTARG");; m) metas+=("$OPTARG");; b) infos+=("$OPTARG");; r) rust=$OPTARG;; j) vend=$OPTARG;;
             o) out=$OPTARG;; v) ver=$OPTARG;; *) usage;; esac
done
n=${#tgts[@]}
[ "$n" -ge 1 ] && [ "${#metas[@]}" -eq "$n" ] && [ "${#infos[@]}" -eq "$n" ] && [ -n "$rust" ] && [ -n "$vend" ] && [ -n "$out" ] || usage
for tgt in "${tgts[@]}"; do
  case "$tgt" in aarch64-apple-darwin|x86_64-apple-darwin) ;; *) die "target '$tgt' is not a macOS triple" ;; esac
done
if [ "$n" -eq 1 ]; then
  arch=${tgts[0]%%-*}
else
  [ "$(printf '%s\n' "${tgts[@]}" | sort -u | tr '\n' ' ')" = "aarch64-apple-darwin x86_64-apple-darwin " ] \
    || die "universal needs exactly the targets aarch64-apple-darwin and x86_64-apple-darwin once each (got: ${tgts[*]})"
  arch=universal
fi
for f in "${metas[@]}" "${infos[@]}" "$rust" "$vend"; do [ -s "$f" ] || die "missing/empty input $f"; done
kv() { # kv <build-info.env> KEY, must be non-empty and from a safe charset
  local v; v=$(sed -n "s/^$2=//p" "$1" | tail -1)
  printf '%s' "$v" | grep -Eqx '[0-9A-Za-z._ ()-]{1,64}' || die "build-info $1 $2='$v' missing or malformed"
  printf '%s' "$v"
}
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT

# 1. Rust, per target: keep only the crates cargo resolves for that target
parts=()
for i in "${!tgts[@]}"; do
  tgt=${tgts[$i]} meta=${metas[$i]}
  jq -e '.resolve.nodes | type == "array" and length > 0' "$meta" >/dev/null || die "$meta has no .resolve.nodes (cargo metadata without --no-deps?)"
  jq -r '(.resolve.nodes | map(.id)) as $ids | .packages[] | select(.id as $i | $ids | index($i)) | "\(.name)@\(.version)"' "$meta" \
    | sort -u > "$t/resolved-$i"
  [ -s "$t/resolved-$i" ] || die "no resolved packages in $meta"
  jq -n --arg tgt "$tgt" --rawfile res "$t/resolved-$i" --slurpfile sbs "$rust" '
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
  ' > "$t/rust-$i.cdx.json" 2>"$t/err" || die "$(sed 's/^jq: error[^:]*: //' "$t/err")"
  for bad in webkit2gtk gtk landlock; do
    jq -e --arg n "$bad" '[.components[] | select(.name == $n)] | length == 0' "$t/rust-$i.cdx.json" >/dev/null \
      || die "Linux-only crate '$bad' survived the $tgt filter"
  done
  jq -e '[.components[] | select(.name == "objc2")] | length >= 1' "$t/rust-$i.cdx.json" >/dev/null \
    || die "no objc2 in the $tgt graph (not a macOS resolve?)"
  parts+=("$t/rust-$i.cdx.json")
done

# 2. OS-provided WebKit (WKWebView), not bundled. minimum = lowest slice minimum (version sort)
: > "$t/os-props.jsonl"
for i in "${!tgts[@]}"; do
  f=${infos[$i]}; m=$(kv "$f" MINOS); kv "$f" SDK_VERSION >/dev/null; kv "$f" XCODE_VERSION >/dev/null; kv "$f" MACOS_BUILD_VERSION >/dev/null
  printf '%s\n' "$m" >> "$t/minos"
  jq -nc --arg tgt "${tgts[$i]}" --arg m "$m" --arg sdk "$(kv "$f" SDK_VERSION)" \
         --arg xcode "$(kv "$f" XCODE_VERSION)" --arg osb "$(kv "$f" MACOS_BUILD_VERSION)" '
    [{name: "opensidian:rust:target", value: $tgt},
     {name: "opensidian:os:minimum:\($tgt)", value: $m},
     {name: "opensidian:os:build-host:\($tgt)", value: $osb},
     {name: "opensidian:build:sdk:\($tgt)", value: "macOS SDK \($sdk) (Xcode \($xcode))"}][]' >> "$t/os-props.jsonl"
done
minos=$(sort -V "$t/minos" | head -1)
jq -n --arg minos "$minos" --arg arch "$arch" --slurpfile props "$t/os-props.jsonl" '
  { bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
    metadata: { component: { type: "operating-system", "bom-ref": "os:apple-macos@\($minos)", name: "macOS",
                             version: ">= \($minos)", supplier: {name: "Apple Inc."},
                             properties: ([{name: "opensidian:os:minimum", value: $minos},
                                           {name: "opensidian:macos:binary", value: $arch}] + $props) } },
    components: [ { type: "framework", "bom-ref": "os:apple-webkit-wkwebview", name: "WebKit.framework (WKWebView)",
                    version: "provided by macOS >= \($minos)", supplier: {name: "Apple Inc."},
                    scope: "required",
                    description: "System WebKit linked from /System/Library/Frameworks/WebKit.framework; not bundled, updated by macOS / Safari updates. No purl: not covered by the OSV scan.",
                    properties: [{name: "opensidian:provided-by", value: "operating-system"},
                                 {name: "opensidian:bundled", value: "false"},
                                 {name: "opensidian:api", value: "WKWebView (tauri wry)"}] } ],
    dependencies: [ {ref: "os:apple-macos@\($minos)", dependsOn: ["os:apple-webkit-wkwebview"]},
                    {ref: "os:apple-webkit-wkwebview", dependsOn: []} ] }' > "$t/os.cdx.json"

# 3. merge (same merger as the Linux release SBOM; dedups the per-target rust parts by bom-ref)
mkdir -p "$out"
m=$("$here/sbom-merge.sh" -o "$t/merged" ${ver:+-v "$ver"} "${parts[@]}" "$vend" "$t/os.cdx.json")
[ -s "$m" ] || die "sbom-merge.sh wrote nothing"
dest="$out/$(basename "$m" .cdx.json)-macos-$arch.cdx.json"
printf '%s\n' "${tgts[@]}" | jq -R . | jq -s '{name: "opensidian:rust:target", value: .[]}' > "$t/tprops.jsonl"
jq --arg arch "$arch" --slurpfile tp "$t/tprops.jsonl" \
  '.metadata.properties = ((.metadata.properties // []) + $tp + [{name: "opensidian:macos:binary", value: $arch}])' "$m" > "$dest"
# the cargo part must be exactly the union of the per-target resolves (name@version), every purl once
sort -u "$t"/resolved-* > "$t/union"
jq -r '.components[] | select(.purl // "" | startswith("pkg:cargo/")) | "\(.name)@\(.version)"' "$dest" | sort > "$t/got"
jq -r '"\(.metadata.component.name)@\(.metadata.component.version)"' "$rust" > "$t/root"
grep -vxF -f "$t/root" "$t/union" > "$t/want" || true
comm -3 "$t/want" "$t/got" > "$t/diff"
[ ! -s "$t/diff" ] || die "cargo components != union of the per-target resolves: $(head -5 "$t/diff" | tr "\n\t" "  ")"
jq -e '.bomFormat == "CycloneDX" and .specVersion == "1.6"
       and ([.components[] | select(."bom-ref" == "os:apple-webkit-wkwebview")] | length == 1)
       and ([.components[] | select(.purl // "" | startswith("pkg:cargo/"))] | length > 0)
       and ([.components[] | select(.purl // "" | startswith("pkg:npm/"))] | length > 0)
       and ([.components[] | .purl // empty] | length == (unique | length))' "$dest" >/dev/null \
  || die "merged SBOM failed self-check"
printf '%s\n' "$dest"
