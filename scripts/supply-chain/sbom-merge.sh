#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# sbom-merge.sh — merge the per-source CycloneDX 1.6 SBOMs into ONE opensidian-<ver>.cdx.json (#188).
# bash + jq + sha256sum. Inputs (any order): vendor-sbom.sh, rust-sbom.sh, webkit-bundle-sbom.sh,
# flatpak-sbom.sh output. Each input's metadata.component becomes a component that the release
# root (opensidian <ver>) dependsOn, so the tree stays: release -> {rust, ui/vendor, AppImage
# bundle, flatpak} -> their components.
# Dedup: components are keyed by bom-ref. The same bom-ref in two inputs (brotli@1.2.1 is both
# the AppImage's and the flatpak's) is kept ONCE, properties unioned; the same bom-ref with a
# different purl/version/hash set/license set fails closed (two different things under one name).
# Hashes and licenses are compared as SETS PER INPUT COMPONENT, not pooled: one component may
# legitimately carry several SHA-256 (webkit-bundle-sbom.sh: one per binary .deb of a source
# package) and several licenses (one per binary copyright); every other input naming that
# bom-ref must carry the identical set or none (a member without hashes/licenses adds nothing).
# Fails closed on: an input that is not CycloneDX 1.6, a dependency ref that names no component.
# serialNumber is a UUID derived from the content hash, so equal inputs give equal bytes.
#
# Usage: sbom-merge.sh -o <dir> [-v <version>] [-t <tag>] <in.cdx.json>...
#   version default: the tag minus its "v" (-t v0.9.4 -> 0.9.4), else src-tauri/Cargo.toml [package]
#   version; -v and -t together must agree. Root metadata.component: license = Cargo.toml license
#   (GPL-3.0-or-later); purl pkg:github/jpzk/opensidian@<tag> only with -t (a local build names no
#   tag, so it gets no purl rather than a made-up one). prints the output path on stdout.
#   SOURCE_DATE_EPOCH fixes metadata.timestamp.
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd); root=$(cd "$here/../.." && pwd)
die() { echo "sbom-merge: $*" >&2; exit 1; }
usage() { echo "usage: sbom-merge.sh -o <dir> [-v <version>] [-t <tag>] <in.cdx.json>..." >&2; exit 2; }
out= ver= tag=
while getopts o:v:t: o; do case $o in o) out=$OPTARG;; v) ver=$OPTARG;; t) tag=$OPTARG;; *) usage;; esac; done
shift $((OPTIND - 1))
[ -n "$out" ] && [ $# -ge 1 ] || usage
if [ -n "$tag" ]; then
  [[ $tag =~ ^v[0-9]+\.[0-9]+(\.[0-9]+)?([-+][0-9A-Za-z.-]+)?$ ]] || die "bad tag '$tag' (want v<version>)"
  tv=${tag#v}; [[ $tv =~ ^[0-9]+\.[0-9]+([-+]|$) ]] && tv=$(sed -E 's/^([0-9]+\.[0-9]+)/\1.0/' <<<"$tv")
  [ -z "$ver" ] || [ "$ver" = "$tv" ] || die "-v $ver disagrees with -t $tag"
  ver=$tv
fi
lic=$(awk -F'"' '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^license *=/{print $2; exit}' "$root/src-tauri/Cargo.toml")
[ "$lic" = "GPL-3.0-or-later" ] || die "src-tauri/Cargo.toml license '$lic' != GPL-3.0-or-later (update sbom-merge.sh deliberately)"
[ -n "$ver" ] || ver=$(awk -F'"' '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^version *=/{print $2; exit}' "$root/src-tauri/Cargo.toml")
[[ $ver =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || die "bad version '$ver'"
for f in "$@"; do
  jq -e '.bomFormat == "CycloneDX" and .specVersion == "1.6" and (.metadata.component."bom-ref" | type == "string")' "$f" >/dev/null 2>&1 \
    || die "$f is not a CycloneDX 1.6 SBOM with a metadata.component bom-ref"
done
mkdir -p "$out"; dest=$out/opensidian-$ver.cdx.json
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
ts=$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%dT%H:%M:%SZ)
jq -s --arg ver "$ver" --arg ts "$ts" --arg tag "$tag" --arg lic "$lic" '
  def key: {purl, version};
  ("opensidian-release@\($ver)") as $rel
  | [ .[] | (.components // []) + [.metadata.component] | .[] ] as $all
  | ($all | group_by(."bom-ref")) as $groups
  | def hsets: [.[] | .hashes // [] | unique | select(length > 0)] | unique;
    def lsets: [.[] | .licenses // [] | unique | select(length > 0)] | unique;
    def hs: hsets[0] // [];
    def ls: lsets[0] // [];
    ($groups | map(select((map(key) | unique | length) > 1
                          or (hsets | length) > 1 or (lsets | length) > 1)
                   | .[0]."bom-ref")) as $conflict
  | if ($conflict | length) > 0 then error("bom-ref with different purl/version/hashes/licenses across inputs: \($conflict | join(", "))") else . end
  | [ $groups[] | .[0] + (if (hs | length) > 0 then {hashes: hs} else {} end)
                  + (if (ls | length) > 0 then {licenses: ls} else {} end)
                  + (if (map(.properties // []) | add | length) > 0
                          then {properties: (map(.properties // []) | add | unique)} else {} end) ] as $comps
  | [ .[] | .dependencies // [] | .[] ] as $deps
  | ([ .[] | .metadata.component."bom-ref" ]) as $roots
  | ($deps | group_by(.ref) | map({ref: .[0].ref, dependsOn: (map(.dependsOn // []) | add | unique)})
     + [{ref: $rel, dependsOn: ($roots | unique)}]) as $d
  | ([$comps[]."bom-ref"] + [$rel]) as $known
  | ([$d[] | .ref, .dependsOn[]] | unique | map(select(. as $r | $known | index($r) | not))) as $dangling
  | if ($dangling | length) > 0 then error("dependency refs naming no component: \($dangling | join(", "))") else . end
  | { bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
      metadata: { timestamp: $ts,
        tools: { components: [{type: "application", name: "scripts/supply-chain/sbom-merge.sh"}] },
        component: ({ type: "application", "bom-ref": $rel, name: "opensidian", version: $ver,
                      licenses: [{expression: $lic}] }
                    + (if $tag != "" then {purl: "pkg:github/jpzk/opensidian@\($tag)"} else {} end)
                    + {externalReferences: [{type: "vcs", url: "https://github.com/jpzk/opensidian"}]}) },
      components: ($comps | sort_by(."bom-ref")),
      dependencies: ($d | sort_by(.ref)) }
' "$@" >"$t/body" 2>"$t/err" || die "$(sed 's/^jq: error[^:]*: //' "$t/err")"
h=$(sha256sum "$t/body" | cut -c1-32)
uuid="${h:0:8}-${h:8:4}-5${h:13:3}-8${h:17:3}-${h:20:12}"
jq --arg sn "urn:uuid:$uuid" '{bomFormat, specVersion, serialNumber: $sn} + del(.bomFormat, .specVersion)' "$t/body" >"$t/out"
jq -e '.specVersion == "1.6" and (.serialNumber | test("^urn:uuid:[0-9a-f-]{36}$"))
       and ([.components[]."bom-ref"] | length == (unique | length))' "$t/out" >/dev/null || die "merged SBOM failed self-check"
mv "$t/out" "$dest"
echo "sbom-merge: $dest ($(jq '.components | length' "$dest") components)" >&2
echo "$dest"
