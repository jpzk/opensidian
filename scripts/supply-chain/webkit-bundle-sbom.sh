#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# webkit-bundle-sbom.sh [--bin <elf>] > webkit-bundle.cdx.json
#
# CycloneDX 1.6 SBOM of the WebKitGTK/GTK closure that the PORTABLE AppImage bundles
# (packaging/ci/build-appimage.sh portable). Run it on the SAME runner image the AppImage job
# uses (release.yml: ubuntu-26.04 / ubuntu-26.04-arm) after installing the SAME apt set.
# Source of truth = this machine's dpkg database: every bundled library is mapped with
# dpkg -S to its binary package, and dpkg-query to that package's source package + version.
#
#   roots    WebKitWebProcess / WebKitNetworkProcess / WebKitGPUProcess, the injected bundle,
#            libwebkit2gtk-4.1.so.0 and (with --bin) the opensidian binary — same search paths
#            as build-appimage.sh.
#   closure  ldd of the roots, minus build-appimage.sh's EXCL list (read from that script, so
#            the two cannot drift), plus the webkit helpers themselves.
#   brotli   libbrotli*.so.1 are REPLACED by build-appimage.sh with upstream brotli $BROTLI_VER
#            (also read from the script): they are emitted as pkg:github/google/brotli@<ver>,
#            not as the Ubuntu package that ldd found.
#   purls    one component per SOURCE package:
#              pkg:deb/ubuntu/<src>@<srcver>?arch=source&distro=ubuntu-26.04
#            OSV keys Debian/Ubuntu advisories by source package + source version, so binary
#            package names (libwebkit2gtk-4.1-0) would match nothing and scan green. The binary
#            packages and the bundled sonames are kept as properties.
# Fails closed: any bundled library not owned by a dpkg package, a missing webkit helper, or an
# unreadable build-appimage.sh -> rc=1 and nothing on stdout.
#
# NOTE (finding, #188): the AppImage job's apt-get install is UNPINNED (no versions, no
# snapshot), so this SBOM describes what ubuntu-26.04 served on the day it ran, which is what
# a release built that day bundles. It is not a lock.
#
# Env: DISTRO (default <ID>-<VERSION_ID> from /etc/os-release), SOURCE_DATE_EPOCH,
#      WK_EXEC_DIR / WK_BUNDLE / WK_LIB override the search (tests only).
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
die() { echo "webkit-bundle-sbom: $*" >&2; exit 1; }
BIN=
while [ $# -gt 0 ]; do case "$1" in
  --bin) BIN=${2:?--bin needs a path}; shift 2;;
  *) die "usage: $0 [--bin <elf>]";; esac; done
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

jq -n --rawfile map "$t/map" --rawfile pkgs "$t/pkgs" \
   --arg ns "$NS" --arg distro "$DISTRO" --arg arch "$ARCH" --arg brotli "$BROTLI_VER" \
   --arg ts "$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%dT%H:%M:%SZ)" '
  def rows($s): $s | split("\n") | map(select(length>0) | split("\t"));
  def nopkgarch: sub(":[a-z0-9]+$"; "");
  def enc: gsub(":"; "%3A");
  (rows($map) | map({pkg: (.[0] | nopkgarch), lib: .[1]})) as $m
  | (rows($pkgs) | map({bin: (.[0] | nopkgarch), ver: .[1], src: .[2], srcver: .[3]})) as $p
  | ($p | group_by([.src, .srcver]) | map(
      . as $g | ($g | map(.bin)) as $bins
      | { type: "library", "bom-ref": "deb:\($g[0].src)@\($g[0].srcver)",
          name: $g[0].src, version: $g[0].srcver,
          purl: "pkg:deb/\($ns)/\($g[0].src)@\($g[0].srcver | enc)?arch=source&distro=\($distro)",
          properties: [
            {name: "opensidian:binary-packages", value: ($g | map("\(.bin)=\(.ver)") | unique | join(","))},
            {name: "opensidian:bundled-libs", value: ($m | map(select(.pkg as $x | $bins | index($x))) | map(.lib) | unique | join(","))},
            {name: "opensidian:arch", value: $arch} ] })) as $debs
  | ($m | map(select(.pkg == "brotli") | .lib) | unique) as $bl
  | {
      bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
      metadata: { timestamp: $ts,
        component: { type: "application", "bom-ref": "opensidian-portable-appimage-bundle",
          name: "opensidian portable AppImage WebKit bundle", version: $distro },
        properties: [ {name: "opensidian:source", value: "dpkg database of the AppImage build image (\($distro), apt UNPINNED)"} ] },
      components: ($debs + (if ($bl | length) > 0 then [{
          type: "library", "bom-ref": "brotli@\($brotli)", name: "brotli", version: $brotli,
          purl: "pkg:github/google/brotli@v\($brotli)",
          properties: [ {name: "opensidian:bundled-libs", value: ($bl | join(","))},
                        {name: "opensidian:note", value: "built from the sha256-pinned upstream tarball by build-appimage.sh; replaces the distro libbrotli"} ] }]
        else [] end)),
      dependencies: [ { ref: "opensidian-portable-appimage-bundle",
        dependsOn: ($debs | map(."bom-ref")) + ($bl | if length > 0 then ["brotli@\($brotli)"] else [] end) } ]
    }' >"$t/out"
jq -e '.specVersion == "1.6" and (.components | length) > 0' "$t/out" >/dev/null || die "bad SBOM"
cat "$t/out"
