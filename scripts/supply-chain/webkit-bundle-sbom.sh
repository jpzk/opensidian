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
# #249 item D: the bundle root (what opensidian ships inside the portable AppImage) carries the app
# license from src-tauri/Cargo.toml - no unlicensed aggregate, no license-note excuse; every Ubuntu
# package / brotli keeps its own license on its own component.
applic=$(awk -F'"' '/^\[package\]/{p=1; next} /^\[/{p=0} p && /^license *=/{print $2; exit}' "$root/src-tauri/Cargo.toml" 2>/dev/null || true)
[ -n "$applic" ] || die "no [package] license in $root/src-tauri/Cargo.toml"
BIN= PKGSET=
while [ $# -gt 0 ]; do case "$1" in
  --bin) BIN=${2:?--bin needs a path}; shift 2;;
  --pkgset) PKGSET=${2:?--pkgset needs a path}; shift 2;;
  *) die "usage: $0 [--bin <elf>] [--pkgset <pkgset.tsv>]";; esac; done

recipe="$root/packaging/ci/build-appimage.sh"
EXCL=$(sed -n "s/^ *EXCL='\(.*\)'$/\1/p" "$recipe")
BROTLI_VER=$(sed -n 's/^ *BROTLI_VER=\([0-9.]*\)$/\1/p' "$recipe")
[ -n "$EXCL" ] && [ -n "$BROTLI_VER" ] || die "cannot read EXCL / BROTLI_VER from $recipe"

if [ -z "${DISTRO:-}" ]; then
  # shellcheck disable=SC1091
  DISTRO=$( . /etc/os-release && echo "${ID}-${VERSION_ID}")
fi
# fedport (2026-11-28): the release build env is Fedora 44 (packaging/ci/fedora-env.sh) -> rpm
# database; ubuntu/debian (dpkg) stays for local/podman runs of the old env and the tests.
case "$DISTRO" in ubuntu-*|debian-*) PM=dpkg ;; fedora-*) PM=rpm ;; *) die "DISTRO=$DISTRO is not ubuntu-* / debian-* / fedora-*";; esac
NS=${DISTRO%%-*}
if [ $PM = rpm ]; then need="ldd rpm jq awk sort"; else need="ldd dpkg dpkg-query jq awk sort"; fi
for c in $need; do command -v "$c" >/dev/null || die "missing $c"; done
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
#      (rpm: rpm -qf, same path variants: /lib64 is a symlink to usr/lib64 on Fedora)
owner() { local p cand
  for p in "$1" "$(readlink -f "$1")"; do
    for cand in "$p" "${p#/usr}" "/usr$p"; do
      if [ $PM = rpm ]; then
        rpm -qf --qf '%{NAME}.%{ARCH}\n' "$cand" 2>/dev/null | awk 'NR==1 && !/not owned|is not/ {print; exit}' | grep . && return 0
      else
        dpkg -S "$cand" 2>/dev/null | awk -F': ' 'NR==1 && $1 !~ /^diversion/ {sub(/,.*/, "", $1); print $1; exit}' | grep . && return 0
      fi
    done; done; return 1; }
: >"$t/map"; unowned=
while read -r f; do
  b=$(basename "$f")
  case "$b" in libbrotli*.so.1) printf 'brotli\t%s\n' "$b" >>"$t/map"; continue;; esac
  if pkg=$(owner "$f"); then printf '%s\t%s\n' "$pkg" "$b" >>"$t/map"; else unowned+=" $f"; fi
done <"$t/files"
[ -z "$unowned" ] || die "not owned by any $PM package:$unowned"

# ---- dpkg-query: binary package -> version, source package, source version ----
# rpm: name.arch -> name, epoch:version-release, source rpm name, its version-release (+ t/rpmx:
#      arch, LICENSE (Fedora: SPDX expression), epoch)
: >"$t/pkgs"; : >"$t/rpmx"
for p in $(awk -F'\t' '$1 != "brotli" {print $1}' "$t/map" | sort -u); do
  if [ $PM = rpm ]; then
    rpm -q --qf '%{NAME}.%{ARCH}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}\t%{SOURCERPM}\n' "$p" \
      | awk -F'\t' '{s=$3; sub(/\.src\.rpm$/,"",s); n=split(s,a,"-"); sv=a[n-1]"-"a[n]; sn=a[1]; for(i=2;i<=n-2;i++) sn=sn"-"a[i]; print $1"\t"$2"\t"sn"\t"sv}' >>"$t/pkgs" \
      || die "rpm -q failed for $p"
    rpm -q --qf '%{NAME}.%{ARCH}\t%{NAME}\t%{ARCH}\t%{EPOCHNUM}\t%{VERSION}-%{RELEASE}\t%{LICENSE}\n' "$p" >>"$t/rpmx" || die "rpm -q failed for $p"
  else
    dpkg-query -W -f='${binary:Package}\t${Version}\t${source:Package}\t${source:Version}\n' "$p" >>"$t/pkgs" \
      || die "dpkg-query failed for $p"
  fi
done
[ "$(wc -l <"$t/pkgs")" -gt 0 ] || die "no packages for the bundle"


# ---- #299 G1: components compiled INTO libwebkit2gtk (Skia, ANGLE, pdf.js, ...) ----
# No package database lists them, so no scanner can see them. They come from the pinned table
# webkit-embedded.tsv, keyed by the upstream webkitgtk version of the bundled webkit2gtk source
# package, and are emitted as nested components of it. Fails closed: no table, a webkit version
# with no rows (a bump without re-deriving the table), or a malformed row -> rc=1.
EMB=${WK_EMBEDDED:-$root/scripts/supply-chain/webkit-embedded.tsv}
[ -f "$EMB" ] || die "embedded-component table $EMB missing"
if [ $PM = rpm ]; then WKSRC=webkitgtk; else WKSRC=webkit2gtk; fi   # Fedora source rpm / Ubuntu source package
wksv=$(awk -F'\t' -v s="$WKSRC" '$3==s {print $4; exit}' "$t/pkgs")
[ -n "$wksv" ] || die "$WKSRC source package not in the bundle"
wkup=${wksv#*:}; wkup=${wkup%%-*}
awk -F'\t' -v v="$wkup" '!/^#/ && $1==v' "$EMB" >"$t/emb"
[ -s "$t/emb" ] || die "no embedded-component rows for webkit $wkup in $EMB (re-derive it from the webkitgtk-$wkup tarball, #299)"
awk -F'\t' 'NF!=6 || $2=="" || $3=="" || $4 !~ /^pkg:[a-z]+\/./ || $5=="" || $6=="" {print; bad=1} END{exit bad}' "$t/emb" >"$t/embbad" \
  || die "bad row in $EMB: $(head -1 "$t/embbad")"

# ---- rpm (Fedora 44 build env): one component per BINARY rpm ----
#   purl     pkg:rpm/fedora/<name>@<version>-<release>?arch=<arch>[&epoch=N]&distro=fedora-44
#            (purl-spec rpm type; the source rpm is a property). OSV has no Fedora ecosystem, so
#            no scanner scores these: the webkit CVE status comes from WebKitGTK's WSA advisories.
#   license  the rpm's own License: tag (Fedora's SPDX expression) -> licenses[].expression
#   hash     SHA-256 of the signed rpm from --pkgset (fedora-env.sh install: the lock, re-checked
#            in the container and rpm -K verified); required in release runs. Disagreement = fail.
if [ $PM = rpm ]; then
  : >"$t/sha"
  while IFS=$'\t' read -r key n a ep vr lic; do
    [ -n "$lic" ] && [ "$lic" != "(none)" ] || die "$key: rpm has no License tag"
    h=-
    if [ -n "$PKGSET" ]; then
      row=$(awk -F'\t' -v n="$n" -v a="$a" '!/^#/ && $1==n && $3==a' "$PKGSET" | head -1)
      [ -n "$row" ] || die "$key: not in pkgset $PKGSET"
      [ "$(cut -f2 <<<"$row")" = "$ep:$vr" ] || die "$key: pkgset says $(cut -f2 <<<"$row"), rpm says $ep:$vr"
      h=$(cut -f4 <<<"$row"); [[ "$h" =~ ^[0-9a-f]{64}$ ]] || die "$key: bad sha256 '$h'"
    fi
    printf '%s\t%s\n' "$key" "$h" >>"$t/sha"
  done <"$t/rpmx"
  jq -n --rawfile map "$t/map" --rawfile pkgs "$t/pkgs" --rawfile x "$t/rpmx" --rawfile sha "$t/sha" --rawfile emb "$t/emb" \
     --arg distro "$DISTRO" --arg arch "$ARCH" --arg brotli "$BROTLI_VER" --arg applic "$applic" --arg wksrc "$WKSRC" \
     --arg hsrc "$(if [ -n "$PKGSET" ]; then echo "pkgset $(basename "$PKGSET") (packaging/ci/fedora-pkgset.lock.tsv: sha256 of the Fedora-signed rpm, re-checked + rpm -K in the build container)"; else echo "none (no --pkgset)"; fi)" \
     --arg ts "$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%dT%H:%M:%SZ)" '
    def rows($s): $s | split("\n") | map(select(length>0) | split("\t"));
    (rows($map) | map({pkg: .[0], lib: .[1]})) as $m
    | (rows($pkgs) | map({key: .[0], value: {src: .[2], srcver: .[3]}}) | from_entries) as $src
    | (rows($sha) | map({key: .[0], value: .[1]}) | from_entries) as $h
    | (rows($emb) | map({name: .[1], version: .[2], purl: .[3], lic: .[4], ev: .[5]})) as $e
    | (rows($x) | map(
        . as [$key, $n, $a, $ep, $vr, $lic]
        | ($src[$key]) as $s
        | { type: "library", "bom-ref": "rpm:\($n)@\($ep):\($vr).\($a)", name: $n,
            version: (if $ep == "0" then $vr else "\($ep):\($vr)" end),
            purl: "pkg:rpm/fedora/\($n)@\($vr)?arch=\($a)\(if $ep == "0" then "" else "&epoch=\($ep)" end)&distro=\($distro)",
            licenses: [{expression: $lic}] }
          + (if $h[$key] != "-" then {hashes: [{alg: "SHA-256", content: $h[$key]}]} else {} end)
          + (if $s.src == $wksrc and ($n | startswith("webkit2gtk4.1")) then {components: ($e | map(
              { type: "library", "bom-ref": "rpm:\($n)@\($ep):\($vr).\($a)/embedded:\(.name)@\(.version)",
                name: .name, version: .version, purl: .purl, licenses: [{expression: .lic}],
                properties: [ {name: "opensidian:embedded-in", value: "\($s.src)@\($s.srcver)"},
                              {name: "opensidian:evidence", value: .ev},
                              {name: "opensidian:source", value: "scripts/supply-chain/webkit-embedded.tsv (compiled into libwebkit2gtk-4.1; not in the rpm database, invisible to scanners without this entry, #299)"} ] }))}
            else {} end)
          + { properties: ([
              {name: "opensidian:source-rpm", value: "\($s.src)-\($s.srcver).src.rpm"},
              {name: "opensidian:bundled-libs", value: ($m | map(select(.pkg == $key)) | map(.lib) | unique | join(","))},
              {name: "opensidian:arch", value: $arch},
              {name: "opensidian:license-source", value: "rpm License tag of \($n)-\($vr).\($a) (Fedora SPDX expression)"} ]
            + (if $h[$key] != "-" then [{name: "opensidian:rpm-sha256", value: $h[$key]}, {name: "opensidian:hash-source", value: $hsrc}]
               else [{name: "opensidian:hash-note", value: "no --pkgset: rpm SHA-256 not recorded"}] end)) })) as $rpms
    | ($m | map(select(.pkg == "brotli") | .lib) | unique) as $bl
    | {
        bomFormat: "CycloneDX", specVersion: "1.6", version: 1,
        metadata: { timestamp: $ts,
          component: { type: "application", "bom-ref": "opensidian-portable-appimage-bundle",
            name: "opensidian portable AppImage WebKit bundle", version: $distro,
            licenses: [{expression: $applic}],
            properties: [{name: "opensidian:aggregate", value: "aggregate part root: the Fedora packages and brotli bundled into the portable AppImage, conveyed as part of the opensidian app under its license (src-tauri/Cargo.toml); each component carries its own license"}] },
          properties: [ {name: "opensidian:source", value: "rpm database of the AppImage build environment (\($distro), packaging/ci/fedora-pkgset.lock.tsv installed exactly)"},
                        {name: "opensidian:hash-source", value: $hsrc} ] },
        components: ($rpms + (if ($bl | length) > 0 then [{
            type: "library", "bom-ref": "brotli@\($brotli)", name: "brotli", version: $brotli,
            purl: "pkg:github/google/brotli@v\($brotli)",
            cpe: "cpe:2.3:a:google:brotli:\($brotli):*:*:*:*:*:*:*",
            licenses: [{expression: "MIT"}],
            properties: [ {name: "opensidian:bundled-libs", value: ($bl | join(","))},
                          {name: "opensidian:note", value: "built from the sha256-pinned upstream tarball by build-appimage.sh; replaces the distro libbrotli"} ] }]
          else [] end)),
        dependencies: [ { ref: "opensidian-portable-appimage-bundle",
          dependsOn: ($rpms | map(."bom-ref")) + ($bl | if length > 0 then ["brotli@\($brotli)"] else [] end) } ]
          + ($rpms | map(select(has("components")) | {ref: ."bom-ref", dependsOn: (.components | map(."bom-ref"))}))
      }' >"$t/out"
  jq -e '.specVersion == "1.6" and (.components | length) > 0 and any(.components[]; .purl | startswith("pkg:rpm/fedora/webkit2gtk4.1@"))' "$t/out" >/dev/null || die "bad SBOM"
  cat "$t/out"; exit 0
fi

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

jq -n --rawfile map "$t/map" --rawfile pkgs "$t/pkgs" --rawfile lic "$t/lic" --rawfile sha "$t/sha" --rawfile emb "$t/emb" \
   --arg ns "$NS" --arg distro "$DISTRO" --arg arch "$ARCH" --arg brotli "$BROTLI_VER" --arg doc "$DOC" --arg applic "$applic" \
   --arg hsrc "$(if [ -n "$PKGSET" ]; then echo "pkgset $(basename "$PKGSET") (snapshot index SHA256, checked against the downloaded .deb by appimage-build.sh)"; else echo "apt-cache show (the configured apt index SHA256)"; fi)" \
   --arg ts "$(date -u -d "@${SOURCE_DATE_EPOCH:-$(date +%s)}" +%Y-%m-%dT%H:%M:%SZ)" '
  def rows($s): $s | split("\n") | map(select(length>0) | split("\t"));
  def nopkgarch: sub(":[a-z0-9]+$"; "");
  def enc: gsub(":"; "%3A");
  (rows($map) | map({pkg: (.[0] | nopkgarch), lib: .[1]})) as $m
  | (rows($pkgs) | map({bin: (.[0] | nopkgarch), ver: .[1], src: .[2], srcver: .[3]})) as $p
  | (rows($lic) | map({bin: .[0], kind: .[1], v: .[2]})) as $l
  | (rows($sha) | map({key: .[0], value: .[1]}) | from_entries) as $h
  | (rows($emb) | map({name: .[1], version: .[2], purl: .[3], lic: .[4], ev: .[5]})) as $e
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
        + (if $g[0].src == "webkit2gtk" then {components: ($e | map(
            { type: "library", "bom-ref": "deb:\($g[0].src)@\($g[0].srcver)/embedded:\(.name)@\(.version)",
              name: .name, version: .version, purl: .purl, licenses: [{expression: .lic}],
              properties: [ {name: "opensidian:embedded-in", value: "\($g[0].src)@\($g[0].srcver)"},
                            {name: "opensidian:evidence", value: .ev},
                            {name: "opensidian:source", value: "scripts/supply-chain/webkit-embedded.tsv (compiled into libwebkit2gtk-4.1; not in dpkg, invisible to scanners without this entry, #299)"} ] }))}
          else {} end)
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
          licenses: [{expression: $applic}],
          properties: [{name: "opensidian:aggregate", value: "aggregate part root: the Ubuntu packages and brotli bundled into the portable AppImage, conveyed as part of the opensidian app under its license (src-tauri/Cargo.toml); each component carries its own license"}] },
        properties: [ {name: "opensidian:source", value: "dpkg database of the AppImage build environment (\($distro), apt pinned to the appimage-build.sh snapshot)"},
                      {name: "opensidian:hash-source", value: $hsrc} ] },
      components: ($debs + (if ($bl | length) > 0 then [{
          type: "library", "bom-ref": "brotli@\($brotli)", name: "brotli", version: $brotli,
          purl: "pkg:github/google/brotli@v\($brotli)",
          cpe: "cpe:2.3:a:google:brotli:\($brotli):*:*:*:*:*:*:*",
          licenses: [{expression: "MIT"}],
          properties: [ {name: "opensidian:bundled-libs", value: ($bl | join(","))},
                        {name: "opensidian:note", value: "built from the sha256-pinned upstream tarball by build-appimage.sh; replaces the distro libbrotli"} ] }]
        else [] end)),
      dependencies: [ { ref: "opensidian-portable-appimage-bundle",
        dependsOn: ($debs | map(."bom-ref")) + ($bl | if length > 0 then ["brotli@\($brotli)"] else [] end) } ]
        + ($debs | map(select(has("components")) | {ref: ."bom-ref", dependsOn: (.components | map(."bom-ref"))}))
    }' >"$t/out"
jq -e '.specVersion == "1.6" and (.components | length) > 0' "$t/out" >/dev/null || die "bad SBOM"
cat "$t/out"
