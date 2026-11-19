#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# webkit-bundle-sbom.sh [--bin <elf>] [--pkgset <pkgset-<ver>-<arch>.tsv>] > webkit-bundle.cdx.json
#
# CycloneDX 1.6 SBOM of the WebKitGTK/GTK closure that the PORTABLE AppImage bundles
# (packaging/ci/build-appimage.sh portable). Run it INSIDE the build environment that produced
# the AppImage: release.yml runs it in the packaging/ci/appimage-build.sh container right after
# the build (digest-pinned ubuntu image + snapshot.ubuntu.com apt pins), so the dpkg database
# read here IS the one the AppImage was bundled from. (Locally: sbom-local.sh recreates that
# container with the same pins.)
# Source of truth = that dpkg database: every bundled library is mapped with dpkg -S to its
# binary package, and dpkg-query to that package's source package + version.
#
#   roots    WebKitWebProcess / WebKitNetworkProcess / WebKitGPUProcess, the injected bundle,
#            libwebkit2gtk-4.1.so.0 and (with --bin) the opensidian binary — same search paths
#            as build-appimage.sh.
#   closure  ldd of the roots, minus build-appimage.sh's EXCL list (read from that script, so
#            the two cannot drift), plus the webkit helpers themselves.
#   brotli   libbrotli*.so.1 are REPLACED by build-appimage.sh with upstream brotli $BROTLI_VER
#            (also read from the script): they are emitted as pkg:github/google/brotli@<ver>
#            (license MIT, the same form as flatpak-sbom.sh so sbom-merge.sh merges them),
#            not as the Ubuntu package that ldd found.
#   purls    one component per SOURCE package:
#              pkg:deb/ubuntu/<src>@<srcver>?arch=source&distro=ubuntu-26.04
#            OSV keys Debian/Ubuntu advisories by source package + source version, so binary
#            package names (libwebkit2gtk-4.1-0) would match nothing and scan green. The binary
#            packages and the bundled sonames are kept as properties.
#   licenses from each binary package's /usr/share/doc/<pkg>/copyright (DEP-5 -> SPDX, below).
#   hashes   SHA-256 of every binary .deb (one hashes[] entry per binary package, mapped by the
#            opensidian:deb-sha256 property), from --pkgset or the apt index (below).
# Fails closed: any bundled library not owned by a dpkg package, a missing webkit helper, a
# missing copyright file, a pkgset that disagrees with dpkg, or an unreadable build-appimage.sh
# -> rc=1 and nothing on stdout.
#
# The apt set is PINNED (#189): snapshot.ubuntu.com $SNAP with pkg=version pins
# (packaging/ci/appimage-build.sh); everything they pull in is fixed by the frozen snapshot.
#
# Env: DISTRO (default <ID>-<VERSION_ID> from /etc/os-release), SOURCE_DATE_EPOCH,
#      WK_EXEC_DIR / WK_BUNDLE / WK_LIB / DOC_ROOT override the search (tests only).
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
die() { echo "webkit-bundle-sbom: $*" >&2; exit 1; }
BIN= PKGSET=
while [ $# -gt 0 ]; do case "$1" in
  --bin) BIN=${2:?--bin needs a path}; shift 2;;
  --pkgset) PKGSET=${2:?--pkgset needs a path}; shift 2;;
  *) die "usage: $0 [--bin <elf>] [--pkgset <pkgset.tsv>]";; esac; done
for c in ldd dpkg dpkg-query jq awk sort; do command -v "$c" >/dev/null || die "missing $c"; done

recipe="$root/packaging/ci/build-appimage.sh"
EXCL=$(sed -n "s/^ *EXCL='\(.*\)'$/\1/p" "$recipe")
BROTLI_VER=$(sed -n 's/^ *BROTLI_VER=\([0-9.]*\)$/\1/p' "$recipe")
[ -n "$EXCL" ] && [ -n "$BROTLI_VER" ] || die "cannot read EXCL / BROTLI_VER from $recipe"

if [ -z "${DISTRO:-}" ]; then
  # shellcheck disable=SC1091
  DISTRO=$( . /etc/os-release && echo "${ID}-${VERSION_ID}")
fi
case "$DISTRO" in ubuntu-*|debian-*) ;; *) die "DISTRO=$DISTRO is not ubuntu-* / debian-*";; esac
NS=${DISTRO%%-*}
ARCH=$(uname -m)

# ---- roots: same search order as build-appimage.sh ----
WKEXEC=${WK_EXEC_DIR:-}
if [ -z "$WKEXEC" ]; then
  for d in /usr/libexec/webkit2gtk-4.1 /usr/lib64/webkit2gtk-4.1 \
           /usr/lib/$ARCH-linux-gnu/webkit2gtk-4.1 /usr/lib/webkit2gtk-4.1; do
    [ -x "$d/WebKitWebProcess" ] && { WKEXEC=$d; break; }; done
fi
[ -n "$WKEXEC" ] && [ -x "$WKEXEC/WebKitWebProcess" ] || die "no WebKitWebProcess (install libwebkit2gtk-4.1-dev as the AppImage job does)"
WKBUNDLE=${WK_BUNDLE:-}
if [ -z "$WKBUNDLE" ]; then
  for d in /usr/lib64/webkit2gtk-4.1 /usr/lib/$ARCH-linux-gnu/webkit2gtk-4.1 \
           /usr/lib/webkit2gtk-4.1 /usr/libexec/webkit2gtk-4.1; do
    f=$d/injected-bundle/libwebkit2gtkinjectedbundle.so; [ -f "$f" ] && { WKBUNDLE=$f; break; }; done
fi
[ -n "$WKBUNDLE" ] && [ -f "$WKBUNDLE" ] || die "no libwebkit2gtkinjectedbundle.so"
WKLIB=${WK_LIB:-}
if [ -z "$WKLIB" ]; then
  for d in /usr/lib/$ARCH-linux-gnu /usr/lib64 /usr/lib; do
    [ -f "$d/libwebkit2gtk-4.1.so.0" ] && { WKLIB=$d/libwebkit2gtk-4.1.so.0; break; }; done
fi
[ -n "$WKLIB" ] && [ -f "$WKLIB" ] || die "no libwebkit2gtk-4.1.so.0"
roots=("$WKEXEC/WebKitWebProcess" "$WKEXEC/WebKitNetworkProcess" "$WKEXEC/WebKitGPUProcess" "$WKBUNDLE" "$WKLIB")
[ -n "$BIN" ] && roots+=("$BIN")
for r in "${roots[@]}"; do [ -f "$r" ] || die "missing root $r"; done

t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
# ldd of every root; an unresolved library ("libfoo.so.1 => not found") would silently drop out
# of the closure (and of the SBOM), so it is fatal here
for r in "${roots[@]}"; do ldd "$r" || die "ldd failed on $r"; done >"$t/ldd"
nf=$(awk '/=> *not found/ {print $1}' "$t/ldd" | sort -u | tr '\n' ' ')
[ -z "$nf" ] || die "ldd cannot resolve (=> not found): $nf"
# bundled files: the helpers + injected bundle are copied verbatim; libs = ldd closure - EXCL
{ printf '%s\n' "$WKEXEC/WebKitWebProcess" "$WKEXEC/WebKitNetworkProcess" "$WKEXEC/WebKitGPUProcess" "$WKBUNDLE"
  awk '/=>/ {print $3}' "$t/ldd" | while read -r lib; do
    [ -f "$lib" ] || continue
    basename "$lib" | grep -qE "$EXCL" && continue
    echo "$lib"
  done
} | sort -u >"$t/files"
[ -s "$t/files" ] || die "empty closure"
grep -q '/libwebkit2gtk-4\.1\.so\.0$' "$t/files" || die "libwebkit2gtk-4.1.so.0 not in the closure"

# ---- dpkg -S: file -> binary package (try the path, its realpath, and the usrmerge twin) ----
owner() { local p cand
  for p in "$1" "$(readlink -f "$1")"; do
    for cand in "$p" "${p#/usr}" "/usr$p"; do
      dpkg -S "$cand" 2>/dev/null | awk -F': ' 'NR==1 && $1 !~ /^diversion/ {sub(/,.*/, "", $1); print $1; exit}' | grep . && return 0
    done; done; return 1; }
: >"$t/map"; unowned=
while read -r f; do
  b=$(basename "$f")
  case "$b" in libbrotli*.so.1) printf 'brotli\t%s\n' "$b" >>"$t/map"; continue;; esac
  if pkg=$(owner "$f"); then printf '%s\t%s\n' "$pkg" "$b" >>"$t/map"; else unowned+=" $f"; fi
done <"$t/files"
[ -z "$unowned" ] || die "not owned by any dpkg package:$unowned"

# ---- dpkg-query: binary package -> version, source package, source version ----
: >"$t/pkgs"
for p in $(awk -F'\t' '$1 != "brotli" {print $1}' "$t/map" | sort -u); do
  dpkg-query -W -f='${binary:Package}\t${Version}\t${source:Package}\t${source:Version}\n' "$p" >>"$t/pkgs" \
    || die "dpkg-query failed for $p"
done

# ---- per binary package: license (DEP-5 copyright) + SHA-256 of the .deb ----
# LICENSE: /usr/share/doc/<pkg>/copyright (the ubuntu image's dpkg excludes keep exactly this
# file). Machine-readable (DEP-5, "Format: ...copyright-format...") -> every "License:" short
# name, split on or/and/commas, mapped to SPDX (spdx_of below); unmappable names -> license.name
# verbatim. Not machine-readable -> license.name LicenseRef-deb-copyright-unstructured + a
# license-note pointing at the file. Missing copyright file -> fail closed.
# HASH: --pkgset <appimage-build.sh pkgset-<ver>-<arch>.tsv> (the snapshot index SHA256 apt
# verified the download against, re-checked against the cached .deb) wins; without it,
# apt-cache show <pkg>:<arch>=<ver>. No hash anywhere (a package that came with the pinned image
# and is no longer in the index) -> opensidian:hash-note says so. A pkgset that disagrees with
# dpkg on a version, or does not list a bundled package -> fail closed.
DOC=${DOC_ROOT:-/usr/share/doc}
spdx_of() { # $1 = one Debian short license name -> SPDX id on stdout, rc=1 if unmappable
  local l; l=$(printf '%s' "$1" | tr 'A-Z' 'a-z')
  case "$l" in
    gpl-[123]|gpl-[123].0|lgpl-[23]|lgpl-[23].0|agpl-3|agpl-3.0|lgpl-2.1|gpl-[123]+|gpl-[123].0+|lgpl-[23]+|lgpl-[23].0+|lgpl-2.1+|agpl-3+|agpl-3.0+|\
    gpl-[123].0-only|gpl-[123].0-or-later|lgpl-2.[01]-only|lgpl-2.[01]-or-later|lgpl-3.0-only|lgpl-3.0-or-later)
      local fam=${l%%-*} v=${l#*-} suf=only
      case "$v" in *-or-later) v=${v%-or-later}; suf=or-later;; *-only) v=${v%-only};; *+) v=${v%+}; suf=or-later;; esac
      case "$v" in *.*) ;; *) v=$v.0;; esac
      printf '%s-%s-%s\n' "$(printf '%s' "$fam" | tr 'a-z' 'A-Z')" "$v" "$suf";;
    expat|mit) echo MIT;;
    bsd-2-clause) echo BSD-2-Clause;; bsd-3-clause) echo BSD-3-Clause;; bsd-4-clause) echo BSD-4-Clause;;
    isc) echo ISC;; zlib) echo Zlib;; x11) echo X11;; curl) echo curl;; openssl) echo OpenSSL;;
    apache-2|apache-2.0) echo Apache-2.0;; mpl-1.1) echo MPL-1.1;; mpl-2|mpl-2.0) echo MPL-2.0;;
    cc0|cc0-1.0) echo CC0-1.0;; bsl-1.0|boost-1.0) echo BSL-1.0;; ftl) echo FTL;;
    libpng) echo Libpng;; libpng-2.0) echo libpng-2.0;; ijg) echo IJG;; icu) echo ICU;; hpnd) echo HPND;;
    unicode-dfs-2016) echo Unicode-DFS-2016;; sgi-b-2.0) echo SGI-B-2.0;; 0bsd) echo 0BSD;; unlicense) echo Unlicense;;
    *) return 1;;
  esac; }
[ -z "$PKGSET" ] || [ -s "$PKGSET" ] || die "--pkgset $PKGSET: missing or empty"
: >"$t/lic"; : >"$t/sha"
while IFS=$'\t' read -r bp ver _ _; do
  b=${bp%%:*}; a=${bp#*:}; [ "$a" = "$bp" ] && a=
  cf=$DOC/$b/copyright
  [ -f "$cf" ] || die "$b: no $cf (license unknown)"
  if grep -qiE '^Format: .*copyright-format' "$cf"; then
    names=$(sed -n 's/^License:[[:space:]]*//p' "$cf" | sed 's/[[:space:]]*$//' | sed -E 's/[[:space:]]+(or|and)[[:space:]]+/\n/g; s/,[[:space:]]*/\n/g' | { grep . || true; } | LC_ALL=C sort -u)
    [ -n "$names" ] || die "$b: DEP-5 $cf has no License: line"
    while read -r n; do
      if s=$(spdx_of "$n"); then printf '%s\tid\t%s\n' "$b" "$s"; else printf '%s\tname\t%s\n' "$b" "$n"; fi
    done <<<"$names" >>"$t/lic"
  else
    printf '%s\tname\t%s\n' "$b" "LicenseRef-deb-copyright-unstructured" >>"$t/lic"
  fi
  h=
  if [ -n "$PKGSET" ]; then
    row=$(awk -F'\t' -v n="$b" -v a="$a" '!/^#/ && $1==n && (a=="" || $3==a || $3=="all")' "$PKGSET" | head -1)
    [ -n "$row" ] || die "$b: not in pkgset $PKGSET"
    pv=$(cut -f2 <<<"$row"); [ "$pv" = "$ver" ] || die "$b: pkgset says $pv, dpkg says $ver"
    h=$(cut -f4 <<<"$row")
  elif command -v apt-cache >/dev/null; then
    h=$( { apt-cache show --no-all-versions "$bp=$ver" 2>/dev/null || apt-cache show --no-all-versions "$b=$ver" 2>/dev/null; } | sed -n 's/^SHA256: //p' | head -1) || true
  fi
  case "$h" in
    [0-9a-f]*) [[ "$h" =~ ^[0-9a-f]{64}$ ]] || die "$b: bad sha256 '$h'"; printf '%s\t%s\n' "$b" "$h" >>"$t/sha";;
    *) printf '%s\t-\n' "$b" >>"$t/sha";;
  esac
done <"$t/pkgs"

jq -n --rawfile map "$t/map" --rawfile pkgs "$t/pkgs" --rawfile lic "$t/lic" --rawfile sha "$t/sha" \
   --arg ns "$NS" --arg distro "$DISTRO" --arg arch "$ARCH" --arg brotli "$BROTLI_VER" --arg doc "$DOC" \
   --arg hsrc "$(if [ -n "$PKGSET" ]; then echo "pkgset $(basename "$PKGSET") (snapshot index SHA256, checked against the downloaded .deb by appimage-build.sh)"; else echo "apt-cache show (the configured apt index SHA256)"; fi)" \
   --arg ts "$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%dT%H:%M:%SZ)" '
  def rows($s): $s | split("\n") | map(select(length>0) | split("\t"));
  def nopkgarch: sub(":[a-z0-9]+$"; "");
  def enc: gsub(":"; "%3A");
  (rows($map) | map({pkg: (.[0] | nopkgarch), lib: .[1]})) as $m
  | (rows($pkgs) | map({bin: (.[0] | nopkgarch), ver: .[1], src: .[2], srcver: .[3]})) as $p
  | (rows($lic) | map({bin: .[0], kind: .[1], v: .[2]})) as $l
  | (rows($sha) | map({key: .[0], value: .[1]}) | from_entries) as $h
  | ($p | group_by([.src, .srcver]) | map(
      . as $g | ($g | map(.bin) | unique) as $bins
      | ($l | map(select(.bin as $x | $bins | index($x))) | map({kind, v}) | unique) as $gl
      | ($bins | map(select($h[.] != "-"))) as $hashed
      | ($bins | map(select($h[.] == "-"))) as $nohash
      | { type: "library", "bom-ref": "deb:\($g[0].src)@\($g[0].srcver)",
          name: $g[0].src, version: $g[0].srcver,
          purl: "pkg:deb/\($ns)/\($g[0].src)@\($g[0].srcver | enc)?arch=source&distro=\($distro)",
          licenses: ($gl | map(if .kind == "id" then {license: {id: .v}} else {license: {name: .v}} end)) }
        + (if ($hashed | length) > 0 then {hashes: ($hashed | map({alg: "SHA-256", content: $h[.]}))} else {} end)
        + { properties: ([
            {name: "opensidian:binary-packages", value: ($g | map("\(.bin)=\(.ver)") | unique | join(","))},
            {name: "opensidian:bundled-libs", value: ($m | map(select(.pkg as $x | $bins | index($x))) | map(.lib) | unique | join(","))},
            {name: "opensidian:arch", value: $arch},
            {name: "opensidian:license-source", value: ($bins | map("\($doc)/\(.)/copyright") | join(","))} ]
          + (if ($hashed | length) > 0 then [{name: "opensidian:deb-sha256", value: ($hashed | map("\(.)=\($h[.])") | join(","))},
                                             {name: "opensidian:hash-source", value: $hsrc}] else [] end)
          + (if ($nohash | length) > 0 then [{name: "opensidian:hash-note", value: "no .deb SHA-256 for \($nohash | join(",")): came with the digest-pinned base image and is not in the apt index"}] else [] end)
          + (if any($gl[]; .v == "LicenseRef-deb-copyright-unstructured") then [{name: "opensidian:license-note", value: "copyright file is not machine-readable (no DEP-5 Format: header); read \($bins | map("\($doc)/\(.)/copyright") | join(","))"}] else [] end)) })) as $debs
  | ($m | map(select(.pkg == "brotli") | .lib) | unique) as $bl
  | {
      bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
      metadata: { timestamp: $ts,
        component: { type: "application", "bom-ref": "opensidian-portable-appimage-bundle",
          name: "opensidian portable AppImage WebKit bundle", version: $distro,
          properties: [{name: "opensidian:license-note", value: "aggregate: the Ubuntu packages and brotli bundled into the AppImage; each component carries its own license"}] },
        properties: [ {name: "opensidian:source", value: "dpkg database of the AppImage build environment (\($distro), apt pinned to the appimage-build.sh snapshot)"},
                      {name: "opensidian:hash-source", value: $hsrc} ] },
      components: ($debs + (if ($bl | length) > 0 then [{
          type: "library", "bom-ref": "brotli@\($brotli)", name: "brotli", version: $brotli,
          purl: "pkg:github/google/brotli@v\($brotli)",
          licenses: [{expression: "MIT"}],
          properties: [ {name: "opensidian:bundled-libs", value: ($bl | join(","))},
                        {name: "opensidian:note", value: "built from the sha256-pinned upstream tarball by build-appimage.sh; replaces the distro libbrotli"} ] }]
        else [] end)),
      dependencies: [ { ref: "opensidian-portable-appimage-bundle",
        dependsOn: ($debs | map(."bom-ref")) + ($bl | if length > 0 then ["brotli@\($brotli)"] else [] end) } ]
    }' >"$t/out"
jq -e '.specVersion == "1.6" and (.components | length) > 0' "$t/out" >/dev/null || die "bad SBOM"
cat "$t/out"
