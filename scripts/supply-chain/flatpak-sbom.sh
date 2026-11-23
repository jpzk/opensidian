#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# flatpak-sbom.sh — CycloneDX 1.6 SBOM of what the flatpak adds besides the Rust binary (#188).
# bash + awk + jq only. Source of truth: packaging/flatpak/dev.koto.opensidian.yml.
#   runtime: + runtime-version:  -> one "platform" component (org.gnome.Platform 49). The runtime's
#                                   own contents (incl. its WebKitGTK) are Flathub's SBOM, not
#                                   ours; this records WHICH runtime the app trusts.
#   modules[].sources type: archive with url + sha256 -> one library component each
#                                   (github tag archives -> pkg:github/<owner>/<repo>@<tag>, else
#                                   pkg:generic). Today: brotli v1.2.1 (same bom-ref as the
#                                   AppImage's brotli in webkit-bundle-sbom.sh, merged once).
# The manifest is parsed with a narrow awk reader of these keys only (no YAML library); an archive
# without a sha256 fails closed.
# LICENSES: the manifest carries none, so module licenses come from MODLIC below (SPDX id, read
# from the module's upstream LICENSE file at the pinned tag); a module missing from MODLIC fails
# closed. The runtime is an aggregate of many projects (Flathub's scope): it gets license.name
# LicenseRef-flathub-runtime plus an opensidian:license-note, and opensidian:hash-note says why
# no hash is recorded (the manifest pins a branch, not an ostree commit; flatpak verifies the
# commit against Flathub's signed summary at install time).
# Usage: flatpak-sbom.sh [manifest.yml] [> flatpak.cdx.json]; SOURCE_DATE_EPOCH fixes the timestamp.
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
mf=${1:-$root/packaging/flatpak/dev.koto.opensidian.yml}
die() { echo "flatpak-sbom: $*" >&2; exit 1; }
# module name -> SPDX license of the pinned upstream source. brotli v1.2.1: LICENSE = MIT.
MODLIC='{"brotli":"MIT"}'
# github owner/repo -> NVD CPE vendor:product (#299 G4). pkg:github has no OSV ecosystem, so a
# github-sourced module is only matchable by a CPE-based scanner (grype); a github module with no
# entry here fails closed instead of shipping an unscannable component.
MODCPE='{"google/brotli":"google:brotli"}'
[ -f "$mf" ] || die "no $mf"
val() { sed -n "s/^$1:[[:space:]]*['\"]\{0,1\}\([^'\" ]*\)['\"]\{0,1\}[[:space:]]*$/\1/p" "$mf" | head -1; }
rt=$(val runtime); rv=$(val runtime-version); app=$(val app-id)
[ -n "$rt" ] && [ -n "$rv" ] && [ -n "$app" ] || die "runtime/runtime-version/app-id missing in $mf"
# the flatpak root IS the opensidian app: same license as src-tauri/Cargo.toml (sbom-merge asserts that one)
applic=$(awk -F'"' '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^license *=/{print $2; exit}' "$root/src-tauri/Cargo.toml")
[ -n "$applic" ] || die "no [package] license in src-tauri/Cargo.toml"
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
# module name, archive url, sha256 — one TSV row per archive source
awk '
  function emit() { if (typ == "archive") printf "%s\t%s\t%s\n", mod, url, sha; typ=url=sha="" }
  function v(x) { sub(/^[^:]*:[[:space:]]*/, "", x); gsub(/["\047]/, "", x); sub(/[[:space:]]+$/, "", x); return x }
  /^  - name:/            { emit(); mod=v($0); next }
  /^      - type:/        { emit(); typ=v($0); next }
  /^        url:/         { url=v($0); next }
  /^        sha256:/      { sha=v($0); next }
  END { emit() }' "$mf" >"$t/arch.tsv"
ts=$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%dT%H:%M:%SZ)
jq -R -s --arg ts "$ts" --arg applic "$applic" --arg rt "$rt" --arg rv "$rv" --arg app "$app" --argjson lic "$MODLIC" --argjson cpe "$MODCPE" '
  [split("\n")[] | select(length > 0) | split("\t") | {mod: .[0], url: .[1], sha: .[2]}] as $a
  | if ($a | map(select((.sha | test("^[0-9a-f]{64}$") | not) or .url == "")) | length) > 0
      then error("archive source without url/sha256: \($a | map(select((.sha | test("^[0-9a-f]{64}$") | not))) | map(.mod) | join(", "))")
    else . end
  | if ($a | map(select($lic[.mod] == null)) | length) > 0
      then error("module without a known license (add it to MODLIC): \($a | map(select($lic[.mod] == null) | .mod) | join(", "))")
    else . end
  | def comp($x):
      # first(...)? // null: a bare capture()? on a non-matching url yields NO value, which made
      # `as $g` emit nothing and silently dropped the module from the SBOM
      (first($x.url | capture("^https://github\\.com/(?<o>[^/]+)/(?<r>[^/]+)/archive/refs/tags/(?<tag>.+)\\.tar\\.gz$"))? // null) as $g
      | if $g then
          {type: "library", "bom-ref": "\($g.r)@\($g.tag | ltrimstr("v"))", name: $g.r, version: ($g.tag | ltrimstr("v")),
           purl: "pkg:github/\($g.o)/\($g.r)@\($g.tag)",
           cpe: ($cpe["\($g.o)/\($g.r)"] // error("github module without a CPE mapping (add it to MODCPE): \($g.o)/\($g.r)")
                 | "cpe:2.3:a:\(.):\($g.tag | ltrimstr("v")):*:*:*:*:*:*:*")}
        elif ($x.url | test("^https?://([^/]*\\.)?(github\\.com|githubusercontent\\.com)/"; "i")) then
          error("github-hosted archive not in the https://github.com/<o>/<r>/archive/refs/tags/<tag>.tar.gz form (it would ship as a CPE-less pkg:generic no scanner matches): \($x.mod) \($x.url)")
        else
          {type: "library", "bom-ref": "flatpak-module:\($x.mod)", name: $x.mod,
           purl: "pkg:generic/\($x.mod)?download_url=\($x.url | @uri)"}
        end
      + {licenses: [{expression: $lic[$x.mod]}],
         hashes: [{alg: "SHA-256", content: $x.sha}],
         externalReferences: [{type: "distribution", url: $x.url}],
         properties: [{name: "opensidian:flatpak-module", value: $x.mod}]};
  ($a | map(comp(.))) as $mods
  | {type: "platform", "bom-ref": "flatpak-runtime:\($rt)//\($rv)", name: $rt, version: $rv,
     purl: "pkg:generic/\($rt)@\($rv)?repository_url=https%3A%2F%2Fdl.flathub.org%2Frepo",
     licenses: [{license: {name: "LicenseRef-flathub-runtime"}}],
     externalReferences: [{type: "distribution", url: "https://dl.flathub.org/repo/flathub.flatpakrepo"}],
     properties: [{name: "opensidian:note", value: "flatpak runtime; its WebKitGTK and other libraries are the runtime vendor scope (slim flatpak uses the runtime WebKit)"},
                  {name: "opensidian:license-note", value: "aggregate of many projects under their own licenses; per-package licenses are in the runtime vendor (Flathub/GNOME) SBOM"},
                  {name: "opensidian:hash-note", value: "no hash: the manifest pins runtime branch \($rv), not an ostree commit; flatpak verifies the installed commit against the GPG-signed Flathub summary"}]} as $runtime
  | { bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
      metadata: { timestamp: $ts,
        tools: { components: [{type: "application", name: "scripts/supply-chain/flatpak-sbom.sh"}] },
        component: { type: "application", "bom-ref": "opensidian-flatpak", name: $app, licenses: [{expression: $applic}],
                     properties: [{name: "opensidian:source", value: "packaging/flatpak/dev.koto.opensidian.yml"}] } },
      components: ([$runtime] + $mods),
      dependencies: [{ref: "opensidian-flatpak", dependsOn: ([$runtime] + $mods | map(."bom-ref"))}] }
' "$t/arch.tsv" >"$t/out" 2>"$t/err" || die "$(sed 's/^jq: error[^:]*: //' "$t/err")"
jq -e '.specVersion == "1.6" and ([.components[] | select(.type == "platform")] | length == 1)' "$t/out" >/dev/null \
  || die "generated SBOM failed self-check"
cat "$t/out"
