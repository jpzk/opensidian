#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# android-sbom.sh — CycloneDX 1.6 SBOM of the Android app for ONE Rust target, built from a
# CHECKOUT of the android product branch (operator 2026-11-20: SBOM for android although no binary
# is released yet; results -> code scanning). bash + awk + jq only, reuses the desktop/macOS
# generators unchanged (rust-sbom.sh, vendor-sbom.sh, sbom-merge.sh), nothing downloaded.
#
#   rust    rust-sbom.sh on <checkout>/src-tauri/Cargo.lock, FILTERED to the crates in
#           `cargo metadata --locked --filter-platform <triple> --format-version 1` .resolve.nodes
#           (normal + build + dev edges: a conservative superset of what links) — same filter as
#           macos-sbom.sh. A resolved crate missing from rust-sbom.sh output, a Linux/macOS-only crate
#           (webkit2gtk, gtk, landlock, objc2) surviving the filter, or no `jni` (not an Android
#           resolve) fails closed.
#   vendor  vendor-sbom.sh with VENDOR_DIR=<checkout>/ui/vendor (vendor-verify.sh proves it first).
#   maven   the Gradle project <checkout>/src-tauri/gen/android, if present, as pkg:maven purls:
#           * gradle lockfiles (*.lockfile) present -> every locked g:a:v (exact, transitive),
#             property opensidian:maven:locked=true;
#           * otherwise the DECLARED coordinates only (build.gradle[.kts] dependency configurations,
#             buildscript classpath, plugins {id(..) version ..}, gradle/libs.versions.toml refs),
#             property opensidian:maven:locked=false: transitive deps are NOT listed and versions are
#             what the build files say. A declared version that is not exact (dynamic +, ranges,
#             latest.*, SNAPSHOT, $variables, missing) is listed WITHOUT a purl version and flagged
#             opensidian:maven:version-exact=false — a version is never invented.
#           Role per coordinate (property opensidian:maven:role + CycloneDX scope): runtime
#           (implementation/api/runtimeOnly: shipped in the APK, scope required), build (buildscript
#           classpath, plugins, buildSrc, compileOnly, kapt/ksp: scope excluded), test (test*/
#           androidTest*: scope excluded). Files applied from the build but absent from the checkout
#           (tauri.build.gradle.kts / tauri.settings.gradle are generated at build time and
#           gitignored) are named in opensidian:maven:not-covered — their deps are NOT in the SBOM.
#           The Gradle wrapper distribution (version + distributionSha256Sum) is listed, no purl.
#   os      Android System WebView, the WebView the app renders with: provided by the device
#           (com.google.android.webview / AOSP WebView), updated through Google Play, NOT shipped by
#           us. Listed as an OS-provided component without purl (not scanned), together with
#           minSdk / targetSdk / compileSdk from app/build.gradle[.kts].
#   root    metadata.component "opensidian android", license = <checkout>/src-tauri/Cargo.toml
#           [package] license, which must be GPL-3.0-or-later.
# Deterministic: SOURCE_DATE_EPOCH (default: the checkout's HEAD commit time; no git + unset ->
# refused), sorted components, serialNumber derived from the content. Writes <file> + <file>.sha256.
#
# Usage:
#   android-sbom.sh -C <checkout> -t <triple> -o <dir> [-m metadata.json] [-j vendor.cdx.json]
#                   [-v version] [-R ref-label]        -> prints the SBOM path
#     -m  cargo metadata for <triple> (default: run cargo metadata --locked --filter-platform in
#         <checkout>/src-tauri — reads manifests, runs no build script)
#     -j  a vendor-sbom.sh output to use instead of running it (tests)
#     -R  provenance label recorded in the SBOM (default: android)
#   android-sbom.sh -L -C <checkout>   -> the Rust triples the branch's Gradle build compiles, one
#                                         per line (RustPlugin.kt targetList default; aarch64 only
#                                         when the checkout has no Gradle project)
#   android-sbom.sh -G -C <checkout>   -> the maven coordinates as TSV
#                                         group artifact version exact locked role configs files
# Exit 0 = ok; 1 = bad input / self-check failed; 2 = usage.
set -euo pipefail
export LC_ALL=C
here=$(cd "$(dirname "$0")" && pwd)
die() { echo "android-sbom: $*" >&2; exit 1; }
usage() { sed -n '/^# Usage:/,/^# Exit/p' "$0" >&2; exit 2; }
mode=sbom C= tgt= out= meta= vend= ver= ref=android
while getopts LGC:t:o:m:j:v:R: o; do
  case $o in L) mode=list;; G) mode=maven;; C) C=$OPTARG;; t) tgt=$OPTARG;; o) out=$OPTARG;;
             m) meta=$OPTARG;; j) vend=$OPTARG;; v) ver=$OPTARG;; R) ref=$OPTARG;; *) usage;; esac
done
[ -n "$C" ] || usage
[ -d "$C" ] || die "checkout '$C' is not a directory"
C=$(cd "$C" && pwd)
[[ $ref =~ ^[A-Za-z0-9._/-]{1,100}$ ]] || die "bad -R '$ref'"
G="$C/src-tauri/gen/android"
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT

# --- the Android triples, as the branch's Gradle build compiles them ---------------------------
known_triple() { # short name (RustPlugin targetList) -> rust triple
  case $1 in aarch64) echo aarch64-linux-android;; armv7) echo armv7-linux-androideabi;;
             i686) echo i686-linux-android;; x86_64) echo x86_64-linux-android;; *) return 1;; esac
}
list_targets() {
  local plug short tl
  plug=
  [ ! -d "$G/buildSrc" ] || plug=$(find "$G/buildSrc" -name RustPlugin.kt -type f | sort | head -1)
  if [ -z "$plug" ]; then echo aarch64-linux-android; return 0; fi
  tl=$(grep -E 'targetList' "$plug" | grep -oE 'listOf\([^)]*\)' | head -1)
  [ -n "$tl" ] || die "$plug: no targetList default (listOf(...)) found"
  for short in $(grep -oE '"[^"]*"' <<<"$tl" | tr -d '"'); do
    known_triple "$short" || die "$plug: unknown Android target '$short' in targetList"
  done | sort -u
}

# --- Gradle / maven ----------------------------------------------------------------------------
confs='classpath|implementation|api|compileOnly|runtimeOnly|testImplementation|testRuntimeOnly|testCompileOnly|androidTestImplementation|androidTestUtil|debugImplementation|releaseImplementation|kapt|ksp|annotationProcessor|coreLibraryDesugaring|lintChecks'
role_of() { # <config> <relative file>
  case $2 in buildSrc/*) echo build; return;; esac
  case $1 in test*|androidTest*) echo test;; classpath|plugin|compileOnly|kapt|ksp|annotationProcessor|lintChecks) echo build;;
             *) echo runtime;; esac
}
exact_ver() { [[ $1 =~ ^[0-9A-Za-z][0-9A-Za-z._-]*$ ]] && [[ ! $1 =~ SNAPSHOT|^latest\. ]]; }
# gradle/libs.versions.toml -> alias(normalised a.b.c) TAB group TAB artifact TAB version
catalog() {
  local f=$G/gradle/libs.versions.toml
  [ -f "$f" ] || return 0
  awk '
    function unq(s) { gsub(/^[ \t"]+|[ \t"]+$/, "", s); return s }
    function field(s, k,   i, r) { i = index(s, k); if (!i) return ""; r = substr(s, i + length(k));
      sub(/^[ \t]*=[ \t]*"/, "", r); sub(/".*/, "", r); return r }
    /^[ \t]*#/ || /^[ \t]*$/ { next }
    /^\[/ { sec = $0; gsub(/[][ \t]/, "", sec); next }
    { line = $0; sub(/[ \t]#.*$/, "", line); eq = index(line, "="); if (!eq) next
      k = unq(substr(line, 1, eq - 1)); v = substr(line, eq + 1) }
    sec == "versions" { if (v ~ /^[ \t]*"/) ver[k] = unq(v); else ver[k] = "?" ; next }
    sec == "libraries" { gsub(/[-_]/, ".", k)
      if (v ~ /^[ \t]*"/) { n = split(unq(v), p, ":"); lib[k] = p[1] "\t" p[2] "\t" (n >= 3 ? p[3] : ""); next }
      mod = field(v, "module"); if (mod != "") { split(mod, p, ":"); g = p[1]; a = p[2] } else { g = field(v, "group"); a = field(v, "name") }
      vr = field(v, "version.ref"); vv = (vr != "" ? "\001" vr : field(v, "version"))
      lib[k] = g "\t" a "\t" vv }
    END { for (k in lib) { split(lib[k], p, "\t"); vv = p[3]
            if (substr(vv, 1, 1) == "\001") { r = substr(vv, 2); vv = (r in ver ? ver[r] : "") }
            print k "\t" p[1] "\t" p[2] "\t" vv } }' "$f" | sort
}
gradle_files() { find "$G" -type f \( -name '*.gradle' -o -name '*.gradle.kts' \) -not -path '*/build/*' -not -path '*/.gradle/*' | sort; }
# declared coordinates: group TAB artifact TAB version TAB config TAB relfile
declared() {
  local f rel cat
  cat=$t/catalog.tsv; catalog >"$cat"
  gradle_files >"$t/gfiles"
  while IFS= read -r f; do
    rel=${f#"$G"/}
    # strip // comments, then one statement per line is what tauri/AGP templates use
    sed -E 's#(^|[[:space:]])//.*$##' "$f" >"$t/src"
    sed -nE "s/^[[:space:]]*($confs)[[:space:]]*(\(|[[:space:]])[[:space:]]*((enforced)?[pP]latform\()?[[:space:]]*[\"']([^\"']+)[\"'].*/\1\t\5/p" "$t/src" \
      | while IFS=$'\t' read -r c coord; do
          IFS=: read -r g a v _ <<<"${coord%@*}"
          [ -n "$g" ] && [ -n "${a:-}" ] || die "$rel: cannot read coordinate '$coord' ($c)"
          printf '%s\t%s\t%s\t%s\t%s\n' "$g" "$a" "${v:-}" "$c" "$rel"
        done
    sed -nE "s/^[[:space:]]*($confs)[[:space:]]*\(?[[:space:]]*((enforced)?[pP]latform\()?[[:space:]]*libs\.([A-Za-z0-9_.]+).*/\1\t\4/p" "$t/src" \
      | while IFS=$'\t' read -r c alias; do
          alias=$(tr '_-' '..' <<<"$alias")
          row=$(awk -F'\t' -v k="$alias" '$1 == k' "$cat")
          [ -n "$row" ] || die "$rel: $c(libs.$alias) is not in gradle/libs.versions.toml [libraries]"
          printf '%s\t%s\t%s\n' "$(cut -f2-4 <<<"$row")" "$c" "$rel"
        done
    sed -nE "s/^[[:space:]]*id[[:space:]]*\(?[[:space:]]*[\"']([^\"']+)[\"'][[:space:]]*\)?[[:space:]]+version[[:space:]]*\(?[[:space:]]*[\"']([^\"']+)[\"'].*/\1\t\2/p" "$t/src" \
      | while IFS=$'\t' read -r id v; do printf '%s\t%s\t%s\t%s\t%s\n' "$id" "$id.gradle.plugin" "$v" plugin "$rel"; done
  done <"$t/gfiles"
}
# locked coordinates: group TAB artifact TAB version TAB configs TAB relfile
locked() {
  local f rel
  find "$G" -type f -name '*.lockfile' -not -path '*/build/*' -not -path '*/.gradle/*' | sort >"$t/lfiles"
  while IFS= read -r f; do
    rel=${f#"$G"/}
    grep -vE '^[[:space:]]*(#|$)|^empty=' "$f" | while IFS= read -r l; do
      [[ $l =~ ^([^:=[:space:]]+):([^:=[:space:]]+):([^:=[:space:]]+)=(.*)$ ]] || die "$rel: unreadable lockfile line '$l'"
      printf '%s\t%s\t%s\t%s\t%s\n' "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "${BASH_REMATCH[3]}" "${BASH_REMATCH[4]}" "$rel"
    done
  done <"$t/lfiles"
}
# merged view: group artifact version exact locked role configs files (one row per g:a:v)
maven_tsv() {
  [ -d "$G" ] || return 0
  declared >"$t/decl.tsv"; locked >"$t/lock.tsv"
  local src lk
  if [ -s "$t/lock.tsv" ]; then src=$t/lock.tsv; lk=true; else src=$t/decl.tsv; lk=false; fi
  while IFS=$'\t' read -r g a v c rel; do
    local cs r; r=build
    for cc in ${c//,/ }; do case $(role_of "$cc" "$rel") in runtime) r=runtime;; test) [ "$r" = runtime ] || r=test;; esac; done
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$g" "$a" "$v" "$(exact_ver "$v" && echo true || echo false)" "$lk" "$r" "$c" "$rel"
  done <"$src" | sort | awk -F'\t' -v OFS='\t' '
    { k = $1 "\t" $2 "\t" $3
      if (!(k in row)) { row[k] = $4 "\t" $5; role[k] = $6; cf[k] = $7; fl[k] = $8; order[++n] = k; next }
      if ($6 == "runtime" || (role[k] == "build" && $6 == "test")) role[k] = $6
      if (index("," cf[k] ",", "," $7 ",") == 0) cf[k] = cf[k] "," $7
      if (index("," fl[k] ",", "," $8 ",") == 0) fl[k] = fl[k] "," $8 }
    END { for (i = 1; i <= n; i++) { k = order[i]; print k, row[k], role[k], cf[k], fl[k] } }'
  # declared deps that a lockfile does not pin are still reported (flagged, never versioned by us)
  if [ "$lk" = true ]; then
    awk -F'\t' 'NR==FNR {L[$1 "\t" $2] = 1; next} !(($1 "\t" $2) in L)' "$t/lock.tsv" "$t/decl.tsv" \
      | while IFS=$'\t' read -r g a v c rel; do
          printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$g" "$a" "$v" "$(exact_ver "$v" && echo true || echo false)" false "$(role_of "$c" "$rel")" "$c" "$rel"
        done | sort -u
  fi
}
not_covered() { # files applied by the build that the checkout does not contain
  [ -d "$G" ] || return 0
  local f a d
  gradle_files >"$t/ncfiles"
  while IFS= read -r f; do
    d=$(dirname "$f")
    sed -nE "s/.*apply[[:space:]]*\(?[[:space:]]*from[[:space:]]*(=|:)[[:space:]]*[\"']([^\"']+)[\"'].*/\2/p" "$f" >"$t/applied"
    while IFS= read -r a; do
      [ -e "$d/$a" ] || { [ "$d" = "$G" ] && echo "$a" || echo "${d#"$G"/}/$a"; }
    done <"$t/applied"
  done <"$t/ncfiles" | sort -u
}

case $mode in
  list) list_targets; exit 0 ;;
  maven) [ -d "$G" ] || die "no Gradle project at src-tauri/gen/android"; maven_tsv; exit 0 ;;
esac

# --- SBOM mode -------------------------------------------------------------------------------
[ -n "$tgt" ] && [ -n "$out" ] || usage
case "$tgt" in aarch64-linux-android|armv7-linux-androideabi|i686-linux-android|x86_64-linux-android) ;;
  *) die "target '$tgt' is not an Android triple (aarch64-linux-android, armv7-linux-androideabi, i686-linux-android, x86_64-linux-android)" ;; esac
list_targets >"$t/targets"
grep -qx -- "$tgt" "$t/targets" || die "target '$tgt' is not built by this checkout (builds: $(paste -sd' ' "$t/targets"))"
lock=$C/src-tauri/Cargo.lock
[ -s "$lock" ] || die "no src-tauri/Cargo.lock in $C (refusing an unlocked Cargo graph)"
[ -s "$C/src-tauri/Cargo.toml" ] || die "no src-tauri/Cargo.toml in $C"
pkgf() { awk -F'"' -v k="$1" '/^\[package\]/{p=1; next} /^\[/{p=0} p && $1 ~ "^"k" *= *$" {print $2; exit}' "$C/src-tauri/Cargo.toml"; }
lic=$(pkgf license)
[ "$lic" = "GPL-3.0-or-later" ] || die "checkout src-tauri/Cargo.toml license '$lic' != GPL-3.0-or-later"
[ -n "$ver" ] || ver=$(pkgf version)
[[ $ver =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$ ]] || die "bad version '$ver'"
commit=
if git -C "$C" rev-parse --verify -q HEAD >/dev/null 2>&1; then
  commit=$(git -C "$C" rev-parse HEAD)
  : "${SOURCE_DATE_EPOCH:=$(git -C "$C" log -1 --format=%ct HEAD)}"
fi
[ -n "${SOURCE_DATE_EPOCH:-}" ] || die "SOURCE_DATE_EPOCH unset and $C is not a git checkout (output would not be deterministic)"
[[ $SOURCE_DATE_EPOCH =~ ^[0-9]+$ ]] || die "SOURCE_DATE_EPOCH '$SOURCE_DATE_EPOCH' is not epoch seconds"
export SOURCE_DATE_EPOCH

# 1. Rust: Cargo.lock SBOM filtered to the crates cargo resolves for $tgt
if [ -z "$meta" ]; then
  meta=$t/metadata.json
  (cd "$C/src-tauri" && cargo metadata --locked --format-version 1 --filter-platform "$tgt") >"$meta" \
    || die "cargo metadata --locked --filter-platform $tgt failed in $C/src-tauri"
fi
[ -s "$meta" ] || die "missing/empty metadata $meta"
"$here/rust-sbom.sh" "$lock" >"$t/rust.full.json" || die "rust-sbom.sh failed on $lock"
jq -e '.resolve.nodes | type == "array" and length > 0' "$meta" >/dev/null 2>&1 || die "$meta has no .resolve.nodes (cargo metadata without --no-deps?)"
jq -r '(.resolve.nodes | map(.id)) as $ids | .packages[] | select(.id as $i | $ids | index($i)) | "\(.name)@\(.version)"' "$meta" \
  | sort -u > "$t/resolved"
[ -s "$t/resolved" ] || die "no resolved packages in $meta"
jq -n --arg tgt "$tgt" --rawfile res "$t/resolved" --slurpfile sbs "$t/rust.full.json" '
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
for bad in webkit2gtk gtk landlock objc2; do
  jq -e --arg n "$bad" '[.components[] | select(.name == $n)] | length == 0' "$t/rust.cdx.json" >/dev/null \
    || die "desktop-only crate '$bad' survived the $tgt filter (not an Android resolve?)"
done
jq -e '[.components[] | select(.name == "jni")] | length >= 1' "$t/rust.cdx.json" >/dev/null \
  || die "no jni in the $tgt graph (not an Android resolve?)"

# 2. vendored JS (ui/vendor of the checkout)
if [ -z "$vend" ]; then
  vend=$t/vendor.cdx.json
  VENDOR_DIR=$C/ui/vendor LOCK=$C/ui/vendor/vendor.lock.json "$here/vendor-sbom.sh" >"$vend" || die "vendor-sbom.sh failed on $C/ui/vendor"
fi
jq -e '[.components[]? | select(.purl // "" | startswith("pkg:npm/"))] | length > 0' "$vend" >/dev/null 2>&1 \
  || die "vendor SBOM $vend has no pkg:npm component"

# 3. maven (Gradle project, if any)
parts=("$t/rust.cdx.json" "$vend")
sdk() { # minSdk / targetSdk / compileSdk from app/build.gradle(.kts)
  local f; f=$(ls "$G/app/build.gradle.kts" "$G/app/build.gradle" 2>/dev/null | head -1)
  [ -n "$f" ] && sed -nE "s/^[[:space:]]*$1[[:space:]]*(=|[[:space:]])[[:space:]]*([0-9]+).*/\2/p" "$f" | head -1
}
minsdk=$(sdk minSdk || true); tsdk=$(sdk targetSdk || true); csdk=$(sdk compileSdk || true)
if [ -d "$G" ]; then
  maven_tsv >"$t/maven.tsv"
  [ -s "$t/maven.tsv" ] || die "Gradle project present but no maven coordinate could be read (parser out of date?)"
  not_covered >"$t/nc"
  wp=$G/gradle/wrapper/gradle-wrapper.properties
  gw= gws=
  if [ -f "$wp" ]; then
    gw=$(sed -nE 's#^distributionUrl=.*/gradle-([0-9][0-9A-Za-z.-]*)-(bin|all)\.zip$#\1#p' "$wp")
    gws=$(sed -nE 's/^distributionSha256Sum=([0-9a-f]{64})$/\1/p' "$wp")
  fi
  jq -Rn --arg nc "$(paste -sd, "$t/nc")" --arg gw "$gw" --arg gws "$gws" '
    [inputs | split("\t") | {g: .[0], a: .[1], v: .[2], exact: (.[3] == "true"), locked: (.[4] == "true"),
       role: .[5], confs: .[6], files: .[7]}] as $m
    | ($m | map(
        (if .exact then "pkg:maven/\(.g)/\(.a)@\(.v)" else "pkg:maven/\(.g)/\(.a)" end) as $p
        | { type: "library", "bom-ref": (if .exact then $p else "\($p)#declared=\(.v)" end),
            group: .g, name: .a, version: (if .v == "" then "unspecified" else .v end),
            scope: (if .role == "runtime" then "required" else "excluded" end),
            properties: ([
              {name: "opensidian:maven:locked", value: (.locked | tostring)},
              {name: "opensidian:maven:version-exact", value: (.exact | tostring)},
              {name: "opensidian:maven:version-source", value: (if .locked then "gradle lockfile" else "declared in build files (not locked)" end)},
              {name: "opensidian:maven:role", value: .role},
              {name: "opensidian:maven:configurations", value: .confs},
              {name: "opensidian:maven:declared-in", value: .files}] | sort_by(.name)) }
          + (if .exact then {purl: $p} else {} end))) as $c
    | ($c + (if $gw != "" then [{ type: "application", "bom-ref": "gradle-wrapper:gradle@\($gw)", name: "gradle", version: $gw,
              scope: "excluded", description: "Gradle distribution fetched by gradlew (build tool, not shipped). No purl: not scanned.",
              properties: [{name: "opensidian:maven:role", value: "build"},
                           {name: "opensidian:gradle:distribution-sha256-pinned", value: (if $gws != "" then "true" else "false" end)}] }
              + (if $gws != "" then {hashes: [{alg: "SHA-256", content: $gws}]} else {} end)] else [] end)) as $all
    | { bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
        metadata: { component: { type: "application", "bom-ref": "gradle:src-tauri/gen/android", name: "src-tauri/gen/android (Gradle project)",
          version: "declared", properties: ([
            {name: "opensidian:maven:lockfiles", value: (if any($m[]; .locked) then "present" else "none: versions are the declared ones, transitive deps not listed" end)},
            {name: "opensidian:maven:unlocked-count", value: ($m | map(select(.locked | not)) | length | tostring)},
            {name: "opensidian:maven:inexact-count", value: ($m | map(select(.exact | not)) | length | tostring)}]
            + (if $nc != "" then [{name: "opensidian:maven:not-covered", value: "applied by the build but not in the checkout (generated, gitignored): \($nc)"}] else [] end)) } },
        components: ($all | sort_by(."bom-ref")),
        dependencies: ([{ref: "gradle:src-tauri/gen/android", dependsOn: ($all | map(."bom-ref") | sort)}]
                       + ($all | map({ref: ."bom-ref", dependsOn: []}) | sort_by(.ref))) }' <"$t/maven.tsv" >"$t/maven.cdx.json"
  parts+=("$t/maven.cdx.json")
  awk -F'\t' '$5 == "false" {u++} $4 == "false" {x++} END {printf "android-sbom: maven: %d coordinate(s), %d not locked, %d without an exact version\n", NR, u, x}' "$t/maven.tsv" >&2
  awk -F'\t' '$5 == "false" {printf "android-sbom: UNLOCKED maven %s:%s:%s (%s, %s)%s\n", $1, $2, ($3 == "" ? "?" : $3), $7, $8, ($4 == "false" ? " NOT EXACT" : "")}' "$t/maven.tsv" >&2
  [ ! -s "$t/nc" ] || echo "android-sbom: NOT COVERED (applied by the build, absent from the checkout): $(paste -sd' ' "$t/nc")" >&2
else
  echo "android-sbom: no Gradle project at src-tauri/gen/android: no maven components" >&2
fi

# 4. OS-provided WebView
jq -n --arg tgt "$tgt" --arg min "${minsdk:-unknown}" --arg ts "${tsdk:-unknown}" --arg cs "${csdk:-unknown}" '
  { bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
    metadata: { component: { type: "operating-system", "bom-ref": "os:android@api-\($min)", name: "Android",
                             version: ">= API \($min)", supplier: {name: "Google LLC / AOSP"},
                             properties: [{name: "opensidian:os:minimum", value: "minSdk \($min)"},
                                          {name: "opensidian:build:targetSdk", value: $ts},
                                          {name: "opensidian:build:compileSdk", value: $cs},
                                          {name: "opensidian:rust:target", value: $tgt}] } },
    components: [ { type: "framework", "bom-ref": "os:android-system-webview", name: "Android System WebView",
                    version: "provided by the device (Android >= API \($min))", supplier: {name: "Google LLC / AOSP"},
                    scope: "required",
                    description: "The WebView the app renders with (com.google.android.webview / AOSP WebView, Chromium-based). Provided by the device and updated through the Google Play Store (or the OS vendor), NOT shipped in our APK. No purl: not covered by the OSV scan.",
                    properties: [{name: "opensidian:provided-by", value: "operating-system"},
                                 {name: "opensidian:bundled", value: "false"},
                                 {name: "opensidian:updated-via", value: "Google Play Store (Android System WebView), not by opensidian releases"},
                                 {name: "opensidian:api", value: "android.webkit.WebView (tauri wry)"}] } ],
    dependencies: [ {ref: "os:android@api-\($min)", dependsOn: ["os:android-system-webview"]},
                    {ref: "os:android-system-webview", dependsOn: []} ] }' > "$t/os.cdx.json"
parts+=("$t/os.cdx.json")

# 5. merge (same merger as the release SBOMs), then name the root + record provenance
m=$("$here/sbom-merge.sh" -o "$t/merged" -v "$ver" "${parts[@]}") || die "sbom-merge.sh failed"
[ -s "$m" ] || die "sbom-merge.sh wrote nothing"
arch=${tgt%%-*}
mkdir -p "$out"
dest="$out/opensidian-$ver-android-$arch.cdx.json"
jq --arg tgt "$tgt" --arg lic "$lic" --arg ref "$ref" --arg commit "$commit" '
  .metadata.component.name = "opensidian android"
  | .metadata.component.licenses = [{expression: $lic}]
  | .metadata.component.properties = [{name: "opensidian:platform", value: "android"},
                                      {name: "opensidian:rust:target", value: $tgt},
                                      {name: "opensidian:source:ref", value: $ref}]
                                     + (if $commit != "" then [{name: "opensidian:source:commit", value: $commit}] else [] end)
  | .metadata.tools.components += [{type: "application", name: "scripts/supply-chain/android-sbom.sh"}]
  | .metadata.properties = ((.metadata.properties // []) + [{name: "opensidian:rust:target", value: $tgt}])
  | del(.serialNumber)' "$m" >"$t/body"
h=$(sha256sum "$t/body" | cut -c1-32)
jq --arg sn "urn:uuid:${h:0:8}-${h:8:4}-5${h:13:3}-8${h:17:3}-${h:20:12}" \
  '{bomFormat, specVersion, serialNumber: $sn} + del(.bomFormat, .specVersion)' "$t/body" >"$t/final"
jq -e --arg lic "$lic" '.bomFormat == "CycloneDX" and .specVersion == "1.6"
       and .metadata.component.name == "opensidian android"
       and .metadata.component.licenses == [{expression: $lic}]
       and ([.components[] | select(."bom-ref" == "os:android-system-webview" and .scope == "required")] | length == 1)
       and ([.components[] | select(.purl // "" | startswith("pkg:cargo/"))] | length > 0)
       and ([.components[] | select(.purl // "" | startswith("pkg:npm/"))] | length > 0)
       and ([.components[]."bom-ref"] | length == (unique | length))' "$t/final" >/dev/null \
  || die "merged SBOM failed self-check"
if [ -d "$G" ]; then
  jq -e '[.components[] | select(.group != null and (."bom-ref" | startswith("pkg:maven/")))] | length > 0' "$t/final" >/dev/null \
    || die "Gradle project present but the merged SBOM has no maven component"
fi
mv "$t/final" "$dest"
(cd "$out" && sha256sum "$(basename "$dest")" >"$(basename "$dest").sha256")
echo "android-sbom: $dest: $(jq -r '[.components[] | (.purl // "" | capture("^pkg:(?<t>[a-z]+)/").t) // "no-purl"] | group_by(.) | map("\(length) \(.[0])") | join(", ")' "$dest")" >&2
printf '%s\n' "$dest"
