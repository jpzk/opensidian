#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# vendor-sbom.sh — CycloneDX 1.6 SBOM of the vendored JS in ui/vendor (issue #188).
# bash + jq + coreutils only; no npm, no node.
#
# Source of truth: ui/vendor/vendor.lock.json, and ONLY after vendor-verify.sh has proven it
# (registry integrity + tarball URL, per-file sha256, embedded versions, no uncovered file, no
# VENDOR.md, publish dates / six-week rule). This script runs vendor-verify.sh first and emits
# nothing if it fails. VENDOR.md prose is never read (and its presence fails the verify).
#
# Components:
#   libraries[]  -> one pkg:npm component each (license, sha512 from dist.integrity, tarball URL,
#                   loaded_by, per-file sha256 as properties);
#   bundles[]    -> one "file" component for the bundle bytes (sha256) that dependsOn the npm
#                   components read from its COMMITTED package-lock.json (non-dev packages:
#                   version + integrity + resolved); dev-only build tools (esbuild) are left out
#                   because they are not shipped.
#
# Usage: scripts/supply-chain/vendor-sbom.sh [> vendor.cdx.json]
#   VENDOR_DIR=<dir> (default ui/vendor), LOCK=<file>, NPM_REGISTRY=<url> are passed to vendor-verify.sh
#   VENDOR_EMBEDDED=<tsv> (default scripts/supply-chain/vendor-embedded.tsv): packages compiled into vendored bundles (#299)
#   SOURCE_DATE_EPOCH=<s> fixes metadata.timestamp (reproducible output)
# Exit 0 = SBOM on stdout; 1 = verify failed / bad input (nothing on stdout); 2 = usage.
set -euo pipefail
export LC_ALL=C

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
export VENDOR_DIR=${VENDOR_DIR:-$root/ui/vendor}
export LOCK=${LOCK:-$VENDOR_DIR/vendor.lock.json}
vd=$VENDOR_DIR lock=$LOCK
for t in jq base64 od; do command -v "$t" >/dev/null || { echo "vendor-sbom: $t required" >&2; exit 2; }; done
# #249 item D: the ui/vendor aggregate root is opensidian's own distribution unit (the ui/vendor tree
# of the GPL app) and carries the app license from src-tauri/Cargo.toml - no unlicensed aggregate,
# no license-note excuse; every vendored library keeps its own license on its own component.
applic=$(awk -F'"' '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^license *=/{print $2; exit}' "$root/src-tauri/Cargo.toml" 2>/dev/null || true)
[ -n "$applic" ] || { echo "vendor-sbom: no [package] license in $root/src-tauri/Cargo.toml" >&2; exit 1; }

# a)-e) first; its report goes to stderr so stdout stays pure JSON
"$here/vendor-verify.sh" >&2 || { echo "vendor-sbom: vendor-verify.sh failed, no SBOM" >&2; exit 1; }

sri_hex() { # sha512-<base64> -> hex
  printf '%s' "${1#sha512-}" | base64 -d | od -An -v -tx1 | tr -d ' \n'
}
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT

# libraries
jq -c '.libraries[]' "$lock" >"$tmp/libs"
: >"$tmp/comps"
while IFS= read -r l; do
  hex=$(sri_hex "$(jq -r .integrity <<<"$l")")
  jq -c --arg hex "$hex" '{
      type: "library", "bom-ref": .purl, name, version, purl,
      licenses: [{license: {id: .license}}],
      hashes: [{alg: "SHA-512", content: $hex}],
      externalReferences: [{type: "distribution", url: .tarball}],
      properties: ([{name: "opensidian:loaded_by", value: .loaded_by},
                    {name: "opensidian:npm_published", value: .published},
                    {name: "opensidian:vendored_added", value: .added}]
                   + [.files[] | {name: "opensidian:file", value: "ui/vendor/\(.path) sha256:\(.sha256) <- \(.tarball_path // .source // "concat of \(.concat | length) tarball files: \(.concat | join(" "))")"}])
    }' <<<"$l" >>"$tmp/comps"
done <"$tmp/libs"

# bundles: the bundle file + its package-lock packages
: >"$tmp/deps"
jq -c '.bundles[]' "$lock" >"$tmp/bundles"
while IFS= read -r b; do
  pl=$vd/$(jq -r .package_lock <<<"$b")
  [ -f "$pl" ] || { echo "vendor-sbom: missing $pl" >&2; exit 1; }
  bref="file:ui/vendor/$(jq -r .path <<<"$b")"
  : >"$tmp/bdeps"
  jq -c '.packages | to_entries[] | select(.key != "" and (.value.dev // false) == false)
         | {name: (.key | sub("^.*node_modules/"; "")), version: .value.version,
            integrity: .value.integrity, resolved: .value.resolved, license: .value.license}' "$pl" >"$tmp/pkgs"
  while IFS= read -r p; do
    hex=$(sri_hex "$(jq -r .integrity <<<"$p")")
    jq -c --arg hex "$hex" --arg pl "ui/vendor/$(jq -r .package_lock <<<"$b")" '
      (.name | sub("^@"; "%40")) as $pn
      | ("pkg:npm/\($pn)@\(.version)") as $purl
      | {type: "library", "bom-ref": $purl, name, version, purl: $purl,
         licenses: (if .license then [{license: {id: .license}}] else [] end),
         hashes: [{alg: "SHA-512", content: $hex}],
         externalReferences: [{type: "distribution", url: .resolved}],
         properties: [{name: "opensidian:source", value: $pl}]}' <<<"$p" >>"$tmp/comps"
    jq -r '(.name | sub("^@"; "%40")) as $pn | "pkg:npm/\($pn)@\(.version)"' <<<"$p" >>"$tmp/bdeps"
  done <"$tmp/pkgs"
  [ -s "$tmp/bdeps" ] || { echo "vendor-sbom: $pl has no packages" >&2; exit 1; }
  jq -c --arg ref "$bref" '{type: "file", "bom-ref": $ref, name: $ref | sub("^file:"; ""),
      licenses: [{license: {id: .license}}], hashes: [{alg: "SHA-256", content: .sha256}],
      description: ."$comment",
      properties: ([{name: "opensidian:loaded_by", value: .loaded_by},
                   {name: "opensidian:built_with", value: "\(.built_with.tool) \(.built_with.version)"}]
                  + [(.files // [])[] | {name: "opensidian:file", value: "ui/vendor/\(.path) sha256:\(.sha256) <- \(.source)"}])}' <<<"$b" >>"$tmp/comps"
  jq -Rn -c --arg ref "$bref" '{ref: $ref, dependsOn: [inputs]}' <"$tmp/bdeps" >>"$tmp/deps"
done <"$tmp/bundles"

# mermaid.min.js bundles third-party npm packages (no package-lock ships with it). Their list is
# ui/vendor/mermaid/LICENSES-bundled.txt: one block per package, each opened by a line of 20+ '='
# then "<name>@<version>  (SPDX: <expr>[; <note>])", "source: <registry tarball url>", a line of
# '-'. Every block must parse (fail closed), the title must name the locked mermaid version, and
# the tarball URL must be the registry URL of that name@version. No hash: the bytes are inlined
# into mermaid.min.js (whose own SHA-256 is on the file property of the mermaid component).
bl=$vd/mermaid/LICENSES-bundled.txt
mref=$(jq -r '.libraries[] | select(.name == "mermaid") | .purl' "$lock")
mver=$(jq -r '.libraries[] | select(.name == "mermaid") | .version' "$lock")
if [ -n "$mref" ]; then
  [ -f "$bl" ] || { echo "vendor-sbom: missing $bl (mermaid bundled packages)" >&2; exit 1; }
  head -1 "$bl" | grep -qF "(mermaid $mver)" \
    || { echo "vendor-sbom: $bl does not name mermaid $mver in its title" >&2; exit 1; }
  : >"$tmp/mdeps"
  st=0 n=0
  while IFS= read -r line || [ -n "$line" ]; do
    case $st in
      0) [[ $line =~ ^={20,}$ ]] && st=1 ;;
      1) [[ $line =~ ^(@?[a-z0-9][a-z0-9._-]*(/[a-z0-9._-]+)?)@([0-9]+\.[0-9]+\.[0-9]+[0-9A-Za-z.+-]*)\ +\(SPDX:\ (.+)\)$ ]] \
           || { echo "vendor-sbom: $bl: bad package line: $line" >&2; exit 1; }
         pn=${BASH_REMATCH[1]} pv=${BASH_REMATCH[3]} rest=${BASH_REMATCH[4]}
         lic=${rest%%;*} note=""; [ "$lic" = "$rest" ] || note=${rest#*; }
         st=2 ;;
      2) base=${pn##*/}
         want="source: https://registry.npmjs.org/$pn/-/$base-$pv.tgz"
         [ "$line" = "$want" ] || { echo "vendor-sbom: $bl: $pn@$pv: expected '$want', got '$line'" >&2; exit 1; }
         st=3 ;;
      3) [[ $line =~ ^-+$ ]] || { echo "vendor-sbom: $bl: $pn@$pv: no '---' line after source" >&2; exit 1; }
         [[ $lic =~ ^[A-Za-z0-9.+-]+$ ]] && lk=id || lk=expression
         jq -nc --arg n "$pn" --arg v "$pv" --arg lic "$lic" --arg lk "$lk" --arg note "$note" \
               --arg url "${want#source: }" --arg in "$mref" '
           ($n | sub("^@"; "%40")) as $pn | "pkg:npm/\($pn)@\($v)" as $purl
           | {type: "library", "bom-ref": $purl, name: $n, version: $v, purl: $purl,
              licenses: (if $lk == "id" then [{license: {id: $lic}}] else [{expression: ($lic | sub("^\\((?<e>.*)\\)$"; "\(.e)"))}] end),
              externalReferences: [{type: "distribution", url: $url}],
              properties: ([{name: "opensidian:bundled_in", value: "ui/vendor/mermaid/mermaid.min.js (\($in))"},
                            {name: "opensidian:source", value: "ui/vendor/mermaid/LICENSES-bundled.txt"},
                            {name: "opensidian:hash-note", value: "inlined into mermaid.min.js, no separate artifact; see the mermaid component file sha256"}]
                           + (if $note != "" then [{name: "opensidian:version-note", value: $note}] else [] end))}' >>"$tmp/comps"
         jq -rn --arg n "$pn" --arg v "$pv" '"pkg:npm/\($n | sub("^@"; "%40"))@\($v)"' >>"$tmp/mdeps"
         n=$((n + 1)) st=0 ;;
    esac
  done <"$bl"
  [ "$st" = 0 ] || { echo "vendor-sbom: $bl: truncated block" >&2; exit 1; }
  [ "$n" -gt 0 ] || { echo "vendor-sbom: $bl lists no packages" >&2; exit 1; }
  [ "$(grep -cE "^={20,}\$" "$bl")" = "$n" ] || { echo "vendor-sbom: $bl: block count != parsed packages" >&2; exit 1; }
  [ "$(sort "$tmp/mdeps" | uniq -d | wc -l)" = 0 ] || { echo "vendor-sbom: $bl lists a package twice" >&2; exit 1; }
  jq -Rn -c --arg ref "$mref" '{ref: $ref, dependsOn: [inputs]}' <"$tmp/mdeps" >>"$tmp/deps"
fi

# #299 G2: npm packages compiled INTO a vendored single-file bundle that ships no package-lock
# (mathjax/tex-chtml-full.js embeds speech-rule-engine, mj-context-menu, mhchemparser). Source: the
# pinned table vendor-embedded.tsv (VENDOR_EMBEDDED overrides; columns host, host_version, file, name,
# version, SPDX license id, registry sha512 integrity, marker, evidence). Each row -> one pkg:npm
# component (embedded_in, marker, evidence properties; SHA-512 = registry integrity of the package
# tarball), and the host library component dependsOn them. FAILS CLOSED on: no table, a malformed
# row, a row whose host is not a locked library or whose bundle file is not one of the host's locked
# files, a locked host with rows only for ANOTHER version (stale table after a bump), a marker that
# grep -F does not find in the bundle (the bytes themselves were proven by vendor-verify.sh), or the
# same name@version twice. Versions/licenses are never guessed: provenance is in the table header.
et=${VENDOR_EMBEDDED:-$here/vendor-embedded.tsv}
[ -f "$et" ] || { echo "vendor-sbom: embedded table $et missing" >&2; exit 1; }
: >"$tmp/edeps"; : >"$tmp/eedges"
while IFS=$'\t' read -r eh ehv ef en ev el ei em ee extra || [ -n "$eh" ]; do
  case $eh in '#'*|'') continue ;; esac
  row="$eh $ehv $en@$ev"
  [ -z "${extra:-}" ] && [ -n "$ee" ] && [ -n "$em" ] \
    && [[ $ehv =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && [[ $ev =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    && [[ $en =~ ^[a-z0-9][a-z0-9._-]*$ ]] && [[ $el =~ ^[A-Za-z0-9.+-]+$ ]] \
    && [[ $ei =~ ^sha512-[A-Za-z0-9+/]+=*$ ]] && [[ $ef =~ ^[A-Za-z0-9._/-]+$ ]] \
    || { echo "vendor-sbom: $et: bad row: $row" >&2; exit 1; }
  lv=$(jq -r --arg n "$eh" '[.libraries[] | select(.name == $n) | .version] | if length == 1 then .[0] else "" end' "$lock")
  [ -n "$lv" ] || { echo "vendor-sbom: $et: $row: $eh is not a locked library" >&2; exit 1; }
  [ "$lv" = "$ehv" ] || continue   # rows of another host version: checked below (stale table)
  jq -e --arg n "$eh" --arg f "$ef" 'any(.libraries[] | select(.name == $n) | .files[]; .path == $f)' "$lock" >/dev/null \
    || { echo "vendor-sbom: $et: $row: $ef is not a locked file of $eh" >&2; exit 1; }
  grep -qF -- "$em" "$vd/$ef" || { echo "vendor-sbom: $et: $row: marker not found in ui/vendor/$ef" >&2; exit 1; }
  href=$(jq -r --arg n "$eh" '.libraries[] | select(.name == $n) | .purl' "$lock")
  purl="pkg:npm/$en@$ev"
  grep -qxF "$purl" "$tmp/edeps" && { echo "vendor-sbom: $et: $purl listed twice" >&2; exit 1; }
  printf '%s\n' "$purl" >>"$tmp/edeps"; printf '%s\t%s\n' "$href" "$purl" >>"$tmp/eedges"
  jq -nc --arg n "$en" --arg v "$ev" --arg lic "$el" --arg hex "$(sri_hex "$ei")" --arg purl "$purl" \
        --arg f "$ef" --arg in "$href" --arg m "$em" --arg e "$ee" --arg src "scripts/supply-chain/$(basename "$et")" '
    {type: "library", "bom-ref": $purl, name: $n, version: $v, purl: $purl,
     licenses: [{license: {id: $lic}}],
     hashes: [{alg: "SHA-512", content: $hex}],
     externalReferences: [{type: "distribution", url: "https://registry.npmjs.org/\($n)/-/\($n)-\($v).tgz"}],
     properties: [{name: "opensidian:embedded_in", value: "ui/vendor/\($f) (\($in))"},
                  {name: "opensidian:source", value: $src},
                  {name: "opensidian:embedded-marker", value: $m},
                  {name: "opensidian:evidence", value: $e},
                  {name: "opensidian:hash-note", value: "SHA-512 = registry.npmjs.org integrity of the package tarball; the code is compiled into ui/vendor/\($f), whose sha256 is on the \($in) component"}]}' >>"$tmp/comps"
done <"$et"
# every locked host the table knows about must have rows for its locked version
while IFS= read -r eh; do
  lv=$(jq -r --arg n "$eh" '.libraries[] | select(.name == $n) | .version' "$lock")
  [ -z "$lv" ] && continue
  awk -F'\t' -v h="$eh" -v v="$lv" '!/^#/ && $1 == h && $2 == v {f=1} END {exit !f}' "$et" \
    || { echo "vendor-sbom: $et: no embedded rows for locked $eh $lv (table has $(awk -F'\t' -v h="$eh" '!/^#/ && $1 == h {print $2}' "$et" | sort -u | tr '\n' ' ')) - re-derive the table" >&2; exit 1; }
done < <(awk -F'\t' '!/^#/ && NF {print $1}' "$et" | sort -u)
if [ -s "$tmp/edeps" ]; then
  cut -f1 "$tmp/eedges" | sort -u | while IFS= read -r h; do
    awk -F'\t' -v h="$h" '$1 == h {print $2}' "$tmp/eedges" | jq -Rn -c --arg ref "$h" '{ref: $ref, dependsOn: [inputs]}'
  done >>"$tmp/deps"
fi

ts=$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%dT%H:%M:%SZ)
jq -s --slurpfile deps "$tmp/deps" --arg ts "$ts" --arg applic "$applic" '{
    bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
    metadata: {timestamp: $ts,
               tools: {components: [{type: "application", name: "scripts/supply-chain/vendor-sbom.sh"}]},
               component: {type: "application", "bom-ref": "opensidian-ui-vendor", name: "opensidian ui/vendor",
                          licenses: [{expression: $applic}],
                          properties: [{name: "opensidian:aggregate", value: "aggregate part root: the third-party libraries vendored under ui/vendor, conveyed as part of the opensidian app under its license (src-tauri/Cargo.toml); each library component carries its own license"}]}},
    components: (unique_by(."bom-ref") | sort_by(."bom-ref")),
    dependencies: ([{ref: "opensidian-ui-vendor",
                     dependsOn: ([.[] | select(.type == "file" or (.properties | map(.name) | index("opensidian:loaded_by")))
                                 | ."bom-ref"] | unique)}] + $deps)
  }' "$tmp/comps" >"$tmp/bom.json"
# no duplicate refs between libraries and bundle packages hidden by unique_by
[ "$(jq 'length' -s "$tmp/comps")" = "$(jq '.components|length' "$tmp/bom.json")" ] \
  || { echo "vendor-sbom: duplicate bom-ref across libraries/bundles" >&2; exit 1; }
cat "$tmp/bom.json"
