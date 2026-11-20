#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# rust-sbom.sh — CycloneDX 1.6 SBOM of the Rust crates in src-tauri/Cargo.lock (issue #188).
# bash + awk + jq only; no cargo plugin, no network.
#
# Source of truth: Cargo.lock (+ the crates.io checksum it records for every registry crate).
#   registry crate -> pkg:cargo/<name>@<ver>, hashes[SHA-256] = Cargo.lock checksum
#   glib 0.18.5    -> path-patched ([patch.crates-io] vendor/glib-0.18.5), so Cargo.lock has NO
#                     source/checksum for it. Emitted EXPLICITLY as pkg:cargo/glib@0.18.5 with the
#                     .crate sha256 pinned in glib-vendor-check.sh (CRATE_SHA, which that script
#                     proves against the crates.io index) and a CycloneDX pedigree naming the one
#                     upstream backport (b5a4071e, RUSTSEC-2024-0429 / GHSA-wrw7-89jp-8q8g).
#   opensidian     -> metadata.component (the workspace root), dependsOn its direct deps.
# Any OTHER crate without a crates.io source (git/path/other registry) fails closed: it would
# otherwise enter the SBOM without a verifiable hash.
#
# Scope, deliberately conservative for CVE scanning: Cargo.lock lists every target and the
# dev-dependencies (sha2) too, so the SBOM is a superset of what the Linux release links. Nothing
# is dropped: with --resolve, crates outside the shipped graph are kept with scope "excluded".
#
# Licenses + scope (G2 / G11, optional inputs; release builds pass both, sbom-local.sh --release
# refuses without them):
#   --metadata <json>  `cargo metadata --locked --all-features --format-version 1` (every target, every
#                      feature, so it covers every Cargo.lock package). .license (SPDX; legacy "A/B" ->
#                      "A OR B") -> licenses[].expression; license_file-only crates -> licenses[].license.name
#                      "LicenseRef-cargo-license-file" + property opensidian:license-note naming the file.
#                      A Cargo.lock crate missing from it fails closed.
#   --resolve <json>   `cargo metadata --locked --filter-platform <triple> --format-version 1` (default
#                      features = how release builds compile). Crates reachable from the root over
#                      normal/build edges are scope "required"; every other Cargo.lock crate (dev-only,
#                      other platform, feature not enabled) stays listed with scope "excluded" + a reason.
#   --target <triple>  recorded as metadata property opensidian:rust:target (with --resolve).
#
# Usage: scripts/supply-chain/rust-sbom.sh [--metadata f] [--resolve f] [--target t] [Cargo.lock] [> rust.cdx.json]
#   SOURCE_DATE_EPOCH=<s> fixes metadata.timestamp
# Exit 0 = SBOM on stdout; 1 = bad input (nothing on stdout); 2 = usage.
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
die() { echo "rust-sbom: $*" >&2; exit 1; }
usage() { echo "usage: rust-sbom.sh [--metadata f] [--resolve f] [--target triple] [Cargo.lock]" >&2; exit 2; }
command -v jq >/dev/null && command -v awk >/dev/null || { echo "rust-sbom: jq + awk required" >&2; exit 2; }
meta= res= tgt= lock=
while [ $# -gt 0 ]; do
  case $1 in
    --metadata) [ $# -ge 2 ] || usage; meta=$2; shift 2 ;;
    --resolve)  [ $# -ge 2 ] || usage; res=$2; shift 2 ;;
    --target)   [ $# -ge 2 ] || usage; tgt=$2; shift 2 ;;
    -*) echo "rust-sbom: unknown option $1" >&2; usage ;;
    *) [ -z "$lock" ] || usage; lock=$1; shift ;;
  esac
done
lock=${lock:-$root/src-tauri/Cargo.lock}
[ -f "$lock" ] || die "no $lock"
for f in $meta $res; do
  jq -e '.packages | type == "array"' "$f" >/dev/null 2>&1 || die "$f is not cargo metadata JSON"
done
[ -z "$res" ] || jq -e '.resolve.root | type == "string"' "$res" >/dev/null 2>&1 \
  || die "$res has no resolve graph (cargo metadata --no-deps?)"

glib_sha=$(sed -n 's/^CRATE_SHA=\([0-9a-f]\{64\}\)$/\1/p' "$here/glib-vendor-check.sh")
[ -n "$glib_sha" ] || die "CRATE_SHA not found in glib-vendor-check.sh"

t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
# one TSV row per [[package]]: name version source checksum deps(|-separated)
awk '
  function flush() { if (n != "") printf "%s\t%s\t%s\t%s\t%s\n", n, v, s, c, d; n=v=s=c=d=""; indeps=0 }
  function unq(x) { gsub(/^[^"]*"|"[^"]*$/, "", x); return x }
  /^\[\[package\]\]/ { flush(); next }
  /^\[/              { flush(); next }
  indeps && /^\]/    { indeps=0; next }
  indeps             { x=unq($0); d = (d == "" ? x : d "|" x); next }
  /^name = /         { n=unq($0); next }
  /^version = /      { v=unq($0); next }
  /^source = /       { s=unq($0); next }
  /^checksum = /     { c=unq($0); next }
  /^dependencies = \[/ { indeps=1; next }
  END { flush() }' "$lock" >"$t/pkgs.tsv"
[ -s "$t/pkgs.tsv" ] || die "no [[package]] in $lock"

ts=$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%dT%H:%M:%SZ)
jq -R -s --arg ts "$ts" --arg glib_sha "$glib_sha" '
  def reg: "registry+https://github.com/rust-lang/crates.io-index";
  [split("\n")[] | select(length > 0) | split("\t")
   | {name: .[0], version: .[1], source: .[2], checksum: .[3],
      deps: (.[4] | if . == "" then [] else split("|") end)}] as $p
  | ($p | map(select(.source == "" and .name == "opensidian"))) as $roots
  | ($p | map(select(.source != reg and ((.name == "opensidian" and .source == "") | not)
                     and ((.name == "glib" and .version == "0.18.5" and .source == "") | not)))) as $bad
  | if ($roots | length) != 1 then error("want exactly one source-less opensidian root, got \($roots | length)")
    elif ($bad | length) > 0 then error("crate(s) without a crates.io source: \($bad | map("\(.name)@\(.version) [\(.source)]") | join(", "))")
    elif ($p | map(select(.source == reg and (.checksum | test("^[0-9a-f]{64}$") | not))) | length) > 0
      then error("registry crate without a sha256 checksum")
    else . end
  | $roots[0] as $root
  | def ref($x): if $x.name == "opensidian" and $x.source == "" then "opensidian@\($x.version)"
                 else "pkg:cargo/\($x.name)@\($x.version)" end;
    # dependency string -> bom-ref: "name" (unique) or "name ver" or "name ver (source)"
    def resolve($d): ($d | split(" ")) as $w
      | [$p[] | select(.name == $w[0] and (($w | length) < 2 or .version == $w[1]))] as $m
      | if ($m | length) == 1 then ref($m[0]) else error("ambiguous/unknown dependency \"\($d)\"") end;
  ($p | map(select(. != $root))) as $crates
  | { bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
      metadata: { timestamp: $ts,
        tools: { components: [{type: "application", name: "scripts/supply-chain/rust-sbom.sh"}] },
        component: { type: "application", "bom-ref": ref($root), name: "opensidian", version: $root.version,
                     properties: [{name: "opensidian:source", value: "src-tauri/Cargo.lock"}] } },
      components: [ $crates[] |
        if .source == "" then   # glib 0.18.5, path-patched
          { type: "library", "bom-ref": ref(.), name, version, purl: ref(.),
            hashes: [{alg: "SHA-256", content: $glib_sha}],
            externalReferences: [{type: "distribution", url: "https://crates.io/api/v1/crates/glib/0.18.5/download"}],
            pedigree: {
              ancestors: [{type: "library", "bom-ref": "ancestor:pkg:cargo/glib@0.18.5", name: "glib", version: "0.18.5",
                           purl: "pkg:cargo/glib@0.18.5", hashes: [{alg: "SHA-256", content: $glib_sha}]}],
              patches: [{type: "backport",
                         diff: {url: "https://github.com/gtk-rs/gtk-rs-core/commit/b5a4071e"},
                         resolves: [{type: "security", id: "RUSTSEC-2024-0429"},
                                    {type: "security", id: "GHSA-wrw7-89jp-8q8g"}]}],
              notes: "src-tauri/vendor/glib-0.18.5 = crates.io .crate + upstream b5a4071e only; proven by scripts/supply-chain/glib-vendor-check.sh" },
            properties: [{name: "opensidian:vendored", value: "src-tauri/vendor/glib-0.18.5"}] }
        else
          { type: "library", "bom-ref": ref(.), name, version, purl: ref(.),
            hashes: [{alg: "SHA-256", content: .checksum}],
            externalReferences: [{type: "distribution", url: "https://crates.io/api/v1/crates/\(.name)/\(.version)/download"}] }
        end ] | sort_by(."bom-ref"),
      dependencies: [ $p[] | {ref: ref(.), dependsOn: ([.deps[] as $d | resolve($d)] | unique)} ] | sort_by(.ref) }
' "$t/pkgs.tsv" >"$t/out" 2>"$t/err" || die "$(sed 's/^jq: error[^:]*: //' "$t/err")"
jq -e '.specVersion == "1.6" and (.components | length) > 0
       and ([.components[]."bom-ref"] | length == (unique | length))
       and ([.components[] | select(.purl == "pkg:cargo/glib@0.18.5")] | length == 1)' "$t/out" >/dev/null \
  || die "generated SBOM failed self-check"

# ---- G2 licenses (cargo metadata --all-features) ----
if [ -n "$meta" ]; then
  jq --slurpfile md "$meta" '
    def lic: if (.license // "") != "" then [{expression: (.license | gsub(" */ *"; " OR "))}]
             elif (.license_file // "") != "" then [{license: {name: "LicenseRef-cargo-license-file"}}]
             else null end;
    ($md[0].packages | map({key: "\(.name)@\(.version)", value: .}) | from_entries) as $m
    | ([.components[] | "\(.name)@\(.version)" | select($m[.] == null)]) as $miss
    | if ($miss | length) > 0 then error("Cargo.lock crate(s) missing from cargo metadata: \($miss | join(", "))") else . end
    | ([.components[] | "\(.name)@\(.version)" | select(($m[.] | lic) == null)]) as $nolic
    | if ($nolic | length) > 0 then error("crate(s) with neither license nor license_file: \($nolic | join(", "))") else . end
    | .components |= map($m["\(.name)@\(.version)"] as $p
        | .licenses = ($p | lic)
        | if ($p.license // "") == "" then
            .properties = ((.properties // []) + [{name: "opensidian:license-note",
              value: "Cargo.toml license_file = \($p.license_file | split("/") | last) (no SPDX expression declared)"}])
          else . end)
    | ($m["\(.metadata.component.name)@\(.metadata.component.version)"] // null) as $r
    | if $r != null and ($r | lic) != null then .metadata.component.licenses = ($r | lic) else . end
  ' "$t/out" >"$t/out2" 2>"$t/err" || die "$(sed 's/^jq: error[^:]*: //' "$t/err")"
  mv "$t/out2" "$t/out"
fi

# ---- G11 scope (cargo metadata --filter-platform <triple>, default features) ----
if [ -n "$res" ]; then
  jq --slurpfile rs "$res" --arg tgt "${tgt:-unspecified}" '
    ($rs[0].packages | map({key: .id, value: "\(.name)@\(.version)"}) | from_entries) as $id
    | ($rs[0].resolve.nodes | map({key: .id, value: [(.deps // [])[]
         | select((.dep_kinds // [{kind: null}]) | any(.kind != "dev")) | .pkg]}) | from_entries) as $g
    | def closure: . as $s | ([$s[], ($s[] | $g[.] // [] | .[])] | unique) as $n
                   | if $n == $s then $s else ($n | closure) end;
    ([$rs[0].resolve.root] | closure | map($id[.])) as $live
    | ($live | map({key: ., value: true}) | from_entries) as $in
    | .components |= map(if $in["\(.name)@\(.version)"] then .scope = "required"
        else .scope = "excluded"
          | .properties = ((.properties // []) + [{name: "opensidian:scope-reason",
              value: "in Cargo.lock but not in the \($tgt) release resolve (cargo metadata --filter-platform, default features, normal+build edges): dev-only, other platform or feature not enabled"}])
        end)
    | .metadata.properties = ((.metadata.properties // []) + [{name: "opensidian:rust:target", value: $tgt},
        {name: "opensidian:rust:required", value: ([.components[] | select(.scope == "required")] | length | tostring)},
        {name: "opensidian:rust:excluded", value: ([.components[] | select(.scope == "excluded")] | length | tostring)}])
  ' "$t/out" >"$t/out2" 2>"$t/err" || die "$(sed 's/^jq: error[^:]*: //' "$t/err")"
  mv "$t/out2" "$t/out"
fi
cat "$t/out"
