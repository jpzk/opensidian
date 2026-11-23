#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# webkit-bundle-sbom.sh + osv-fixed-gate.sh on a FIXTURE: ldd / dpkg / dpkg-query are stubbed
# on PATH over a fake library tree, so the closure -> dpkg -S -> source-package mapping and the
# CycloneDX output are tested on any host (the real run needs the ubuntu-26.04 runner).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }

# ---- fixture: fake tree + stubs ----
L=$t/fs/usr/lib/x86_64-linux-gnu; W=$L/webkit2gtk-4.1
mkdir -p "$W/injected-bundle" "$t/bin"
for x in WebKitWebProcess WebKitNetworkProcess WebKitGPUProcess; do : >"$W/$x"; chmod +x "$W/$x"; done
: >"$W/injected-bundle/libwebkit2gtkinjectedbundle.so"
for l in libwebkit2gtk-4.1.so.0 libjavascriptcoregtk-4.1.so.0 libgtk-3.so.0 libgdk-3.so.0 \
         libsoup-3.0.so.0 libbrotlidec.so.1 libbrotlicommon.so.1 libc.so.6 libX11.so.6 libgcrypt.so.20; do : >"$L/$l"; done
# ldd: every root links the same set (+ a missing one ldd prints as "not found")
cat >"$t/bin/ldd" <<S
#!/bin/sh
for l in libwebkit2gtk-4.1.so.0 libjavascriptcoregtk-4.1.so.0 libgtk-3.so.0 libgdk-3.so.0 libsoup-3.0.so.0 \\
         libbrotlidec.so.1 libbrotlicommon.so.1 libc.so.6 libX11.so.6 libgcrypt.so.20; do
  echo "	\$l => $L/\$l (0x00007f0000000000)"; done
echo "	linux-vdso.so.1 (0x00007ffd00000000)"
[ -z "\${NOTFOUND:-}" ] || echo "	\$NOTFOUND => not found"
S
# dpkg -S: owner per basename; $t/unowned lists basenames dpkg does not know
cat >"$t/bin/dpkg" <<'S'
#!/bin/sh
[ "$1" = -S ] || exit 2
b=$(basename "$2"); grep -qx "$b" "$UNOWNED" 2>/dev/null && { echo "dpkg-query: no path found matching pattern $2" >&2; exit 1; }
case "$b" in
  libwebkit2gtk-4.1.so.0|WebKit*Process|libwebkit2gtkinjectedbundle.so) p=libwebkit2gtk-4.1-0;;
  libjavascriptcoregtk-4.1.so.0) p=libjavascriptcoregtk-4.1-0;;
  libgtk-3.so.0|libgdk-3.so.0) p=libgtk-3-0t64;;
  libsoup-3.0.so.0) p=libsoup-3.0-0;;
  libbrotli*) p=libbrotli1;;
  libgcrypt.so.20) p=libgcrypt20;;
  *) exit 1;;
esac
echo "$p:amd64: $2"
S
cat >"$t/bin/dpkg-query" <<'S'
#!/bin/sh
case "${3%%:*}" in
  libwebkit2gtk-4.1-0)        echo "libwebkit2gtk-4.1-0:amd64	2.50.1-0ubuntu1	webkit2gtk	2.50.1-0ubuntu1";;
  libjavascriptcoregtk-4.1-0) echo "libjavascriptcoregtk-4.1-0:amd64	2.50.1-0ubuntu1	webkit2gtk	2.50.1-0ubuntu1";;
  libgtk-3-0t64)              echo "libgtk-3-0t64:amd64	3.24.50-1ubuntu1	gtk+3.0	3.24.50-1ubuntu1";;
  libsoup-3.0-0)              echo "libsoup-3.0-0:amd64	3.6.5-3	libsoup3	3.6.5-3";;
  libgcrypt20)                echo "libgcrypt20:amd64	1.11.0-7	libgcrypt20	1:1.11.0-7";;
  *) exit 1;;
esac
S
# copyright files (DOC_ROOT): DEP-5 for webkit/jsc (BSD-2-clause, LGPL-2+, "or" split, an
# unmappable "with ... exception" name), gtk (LGPL-2+), libsoup (Expat); libgcrypt20 is NOT DEP-5
D=$t/doc; mkdir -p "$D"
dep5() { local p=$1; mkdir -p "$D/$p"; { echo "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/"; echo
  shift; for l in "$@"; do printf 'Files: *\nCopyright: x\nLicense: %s\n\nLicense: %s\n some text\n\n' "$l" "$l"; done; } >"$D/$p/copyright"; }
for b in libwebkit2gtk-4.1-0 libjavascriptcoregtk-4.1-0; do dep5 $b "BSD-2-clause" "LGPL-2+ or MPL-1.1" "LGPL-2.1+, GPL-3+" "GPL-2+ with Bison exception"; done
b=libgtk-3-0t64; dep5 $b "LGPL-2+"
b=libsoup-3.0-0; dep5 $b "Expat"
mkdir -p "$D/libgcrypt20"; printf 'This is Debian libgcrypt.\nGNU Lesser General Public License...\n' >"$D/libgcrypt20/copyright"
# pkgset (appimage-build.sh format); libgcrypt20 came with the image -> "image"
H1=$(printf a%.0s {1..64}); H2=$(printf b%.0s {1..64}); H3=$(printf c%.0s {1..64}); H4=$(printf d%.0s {1..64})
{ printf '# image x  snapshot y\n# package\tversion\tarch\tdeb-sha256\n'
  printf 'libgcrypt20\t1.11.0-7\tamd64\timage\n'
  printf 'libgtk-3-0t64\t3.24.50-1ubuntu1\tamd64\t%s\n' "$H3"
  printf 'libjavascriptcoregtk-4.1-0\t2.50.1-0ubuntu1\tamd64\t%s\n' "$H2"
  printf 'libsoup-3.0-0\t3.6.5-3\tamd64\t%s\n' "$H4"
  printf 'libwebkit2gtk-4.1-0\t2.50.1-0ubuntu1\tamd64\t%s\n' "$H1"; } >"$t/pkgset.tsv"
# apt-cache stub (no --pkgset): only webkit is in the index
cat >"$t/bin/apt-cache" <<S
#!/bin/sh
case "\$*" in *libwebkit2gtk-4.1-0*=2.50.1-0ubuntu1*) printf 'Package: libwebkit2gtk-4.1-0\nSHA256: $H1\n';; *) exit 100;; esac
S
chmod +x "$t/bin/"*
# #299 G1: embedded-component table for the fixture webkit (2.50.1); rows for another version are ignored
printf "# comment\n2.50.1\tskia\tm140\tpkg:generic/skia@m140\tBSD-3-Clause\tSkMilestone.h 140\n2.50.1\tpdfjs-dist\t5.0.1\tpkg:npm/pdfjs-dist@5.0.1\tApache-2.0\tpdf.mjs\n9.9.9\tangle\t1\tpkg:generic/angle@1\tBSD-3-Clause\tx\n" >"$t/emb.tsv"
run() { env PATH="$t/bin:$PATH" UNOWNED="$t/unowned" DOC_ROOT="$t/doc" WK_EMBEDDED=${WK_EMBEDDED:-$t/emb.tsv} DISTRO=${DISTRO:-ubuntu-26.04} SOURCE_DATE_EPOCH=1700000000 \
  WK_EXEC_DIR="$W" WK_BUNDLE="$W/injected-bundle/libwebkit2gtkinjectedbundle.so" WK_LIB="$L/libwebkit2gtk-4.1.so.0" \
  "$here/webkit-bundle-sbom.sh" "$@"; }
expect_fail() { local name=$1 pat=$2; shift 2
  set +e; "$@" >"$t/so" 2>"$t/se"; rc=$?; set -e
  if [ "$rc" -ne 0 ] && [ ! -s "$t/so" ] && grep -q -- "$pat" "$t/se"; then ok "$name: rc=$rc, no stdout"
  else cat "$t/so" "$t/se"; bad "$name: rc=$rc (want !=0, empty stdout, '$pat')"; fi; }

# ---- clean ----
: >"$t/unowned"
if run >"$t/sbom.json" 2>"$t/err"; then ok "fixture: rc=0"; else cat "$t/err"; bad "fixture: rc!=0"; fi
chk() { if jq -e "$2" "$t/sbom.json" >/dev/null; then ok "$1"; else bad "$1"; fi; }
chk "CycloneDX 1.6"        '.bomFormat=="CycloneDX" and .specVersion=="1.6" and (.components|length)==5'
chk "webkit2gtk = one source component for 2 binary pkgs" \
  '[.components[]|select(.name=="webkit2gtk")] | length==1 and (.[0].purl=="pkg:deb/ubuntu/webkit2gtk@2.50.1-0ubuntu1?arch=source&distro=ubuntu-26.04") and (.[0].properties[]|select(.name=="opensidian:binary-packages").value=="libjavascriptcoregtk-4.1-0=2.50.1-0ubuntu1,libwebkit2gtk-4.1-0=2.50.1-0ubuntu1")'
chk "helpers counted as webkit libs" \
  '.components[]|select(.name=="webkit2gtk")|.properties[]|select(.name=="opensidian:bundled-libs").value|test("WebKitGPUProcess") and test("libwebkit2gtkinjectedbundle.so")'
chk "epoch percent-encoded" 'any(.components[]; .purl=="pkg:deb/ubuntu/libgcrypt20@1%3A1.11.0-7?arch=source&distro=ubuntu-26.04")'
chk "EXCL libs (libc, libX11) not bundled" '[.components[].properties[]?|select(.name=="opensidian:bundled-libs").value]|join(",")|(test("libc\\.so")|not) and (test("libX11")|not)'
chk "brotli = upstream pin, not the deb" \
  'any(.components[]; .purl=="pkg:github/google/brotli@v1.2.1" and (.properties[0].value=="libbrotlicommon.so.1,libbrotlidec.so.1")) and (any(.components[]; .purl|startswith("pkg:deb/ubuntu/brotli"))|not)'
chk "G4 (#299): brotli carries a CPE so grype can match it" \
  'any(.components[]; .name=="brotli" and .cpe=="cpe:2.3:a:google:brotli:1.2.1:*:*:*:*:*:*:*")'
chk "unique bom-refs + dependency graph complete" \
  '([.components[]."bom-ref"]|length)==([.components[]."bom-ref"]|unique|length) and (.dependencies[0].dependsOn|sort)==([.components[]."bom-ref"]|sort)'

# ---- negatives ----
echo libsoup-3.0.so.0 >"$t/unowned"
expect_fail "library not owned by dpkg" "not owned by any dpkg package" run
: >"$t/unowned"
rm "$W/WebKitGPUProcess"
expect_fail "missing webkit helper" "missing root" run
: >"$W/WebKitGPUProcess"; chmod +x "$W/WebKitGPUProcess"
NOTFOUND=libmissing.so.9 expect_fail "ldd => not found is fatal" "ldd cannot resolve (=> not found): libmissing.so.9" run
DISTRO=fedora-44 expect_fail "non-deb distro" "is not ubuntu" run
DISTRO=debian-13 run >"$t/sbom.json" 2>/dev/null && chk "debian-13 namespace" 'all(.components[]|select(.purl|startswith("pkg:deb")); .purl|test("^pkg:deb/debian/.*distro=debian-13$"))'


# ---- G3: licenses (DEP-5 -> SPDX) + .deb SHA-256 ----
: >"$t/unowned"
run --pkgset "$t/pkgset.tsv" >"$t/sbom.json" 2>"$t/err" && ok "--pkgset: rc=0" || { cat "$t/err"; bad "--pkgset rc!=0"; }
C='.components[]|select(.name=="webkit2gtk")'
chk "every component has licenses" 'all(.components[]; (.licenses|length)>0)'
chk "D: aggregate bundle root licensed GPL-3.0-or-later (src-tauri/Cargo.toml)" 'any(.metadata.component.properties[]; .name=="opensidian:aggregate") and .metadata.component.licenses==[{expression:"GPL-3.0-or-later"}]'
chk "DEP-5 short names -> SPDX ids (or split, comma split, + -> or-later)" \
  "[$C|.licenses[].license.id|select(.)]==[\"BSD-2-Clause\",\"GPL-3.0-or-later\",\"LGPL-2.0-or-later\",\"LGPL-2.1-or-later\",\"MPL-1.1\"]"
chk "unmappable DEP-5 name kept verbatim as license.name" "[$C|.licenses[].license.name|select(.)]==[\"GPL-2+ with Bison exception\"]"
chk "Expat -> MIT" '.components[]|select(.name=="libsoup3")|.licenses==[{license:{id:"MIT"}}]'
chk "non-DEP-5 copyright -> LicenseRef + license-note" \
  '.components[]|select(.name=="libgcrypt20")|(.licenses==[{license:{name:"LicenseRef-deb-copyright-unstructured"}}]) and any(.properties[]; .name=="opensidian:license-note" and (.value|test("libgcrypt20/copyright")))'
chk "one SHA-256 per binary .deb, mapped by property" \
  "($C|[.hashes[]|select(.alg==\"SHA-256\").content]|sort)==[\"$H1\",\"$H2\"] and ($C|.properties[]|select(.name==\"opensidian:deb-sha256\").value)==\"libjavascriptcoregtk-4.1-0=$H2,libwebkit2gtk-4.1-0=$H1\""
cp "$t/sbom.json" "$t/sbom-pkgset.json"
if mp=$("$here/sbom-merge.sh" -o "$t/merged" -v 0.0.1 "$t/sbom-pkgset.json" 2>"$t/merr") \
   && jq -e --arg a "$H1" --arg b "$H2" '[.components[]|select(.name=="webkit2gtk")|.hashes|map(.content)|sort]==[[$a,$b]|sort]' "$mp" >/dev/null; then
  ok "sbom-merge accepts the 2-binary source component and keeps both SHA-256 (dry run 44542613403)"
else cat "$t/merr"; bad "sbom-merge on webkit-bundle-sbom --pkgset output"; fi
chk "image package: no hash, hash-note says why" \
  '.components[]|select(.name=="libgcrypt20")|(has("hashes")|not) and any(.properties[]; .name=="opensidian:hash-note" and (.value|test("base image")))'
chk "every deb component: hash or hash-note" 'all(.components[]|select(.purl|startswith("pkg:deb")); has("hashes") or any(.properties[]; .name=="opensidian:hash-note"))'
chk "brotli licensed MIT (same form as flatpak-sbom.sh)" '.components[]|select(.name=="brotli")|.licenses==[{expression:"MIT"}]'
chk "metadata says pinned, not UNPINNED" '(.metadata.properties[]|select(.name=="opensidian:source").value)|(test("UNPINNED")|not) and test("pinned")'
run >"$t/sbom.json" 2>/dev/null
chk "no --pkgset: hash from the apt index" "($C|[.hashes[].content])==[\"$H1\"] and ([.components[]|select(.name==\"gtk+3.0\")|has(\"hashes\")]==[false])"
sed 's/^libsoup-3.0-0\t3.6.5-3\t/libsoup-3.0-0\t3.6.5-4\t/' "$t/pkgset.tsv" >"$t/ps2"
expect_fail "pkgset version != dpkg" "pkgset says 3.6.5-4, dpkg says 3.6.5-3" run --pkgset "$t/ps2"
grep -v '^libsoup-3.0-0' "$t/pkgset.tsv" >"$t/ps3"
expect_fail "bundled package missing from pkgset" "libsoup-3.0-0: not in pkgset" run --pkgset "$t/ps3"
sed 's/\t'"$H4"'$/\tdeadbeef/' "$t/pkgset.tsv" >"$t/ps4"
expect_fail "malformed sha256 in pkgset" "bad sha256" run --pkgset "$t/ps4"
expect_fail "missing pkgset file" "missing or empty" run --pkgset "$t/nope"
mv "$D/libsoup-3.0-0/copyright" "$t/cr"
expect_fail "missing copyright file" "libsoup-3.0-0: no .*copyright" run --pkgset "$t/pkgset.tsv"
mv "$t/cr" "$D/libsoup-3.0-0/copyright"
printf 'Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n\nFiles: *\n' >"$D/libgcrypt20/copyright"
expect_fail "DEP-5 without License: line" "has no License: line" run --pkgset "$t/pkgset.tsv"
# ---- #299 G1: components embedded INSIDE libwebkit2gtk (Skia, ANGLE, pdf.js, ...) ----
printf 'This is Debian libgcrypt.\nGNU Lesser General Public License...\n' >"$D/libgcrypt20/copyright"
: >"$t/unowned"
run >"$t/sbom.json" 2>"$t/err" || { cat "$t/err"; bad "G1 run rc!=0"; }
W2='.components[]|select(.name=="webkit2gtk")'
chk "G1: webkit2gtk carries embedded skia as nested component with version + purl" \
  "[$W2|.components[]?|select(.name==\"skia\")]|length==1 and .[0].version==\"m140\" and .[0].purl==\"pkg:generic/skia@m140\" and .[0].licenses==[{expression:\"BSD-3-Clause\"}]"
chk "G1: embedded pdf.js has a matchable pkg:npm purl" "any($W2|.components[]?; .purl==\"pkg:npm/pdfjs-dist@5.0.1\")"
chk "G1: only rows of THIS webkit version (2.50.1), not 9.9.9" "[$W2|.components[]?|.name]|sort==[\"pdfjs-dist\",\"skia\"]"
chk "G1: embedded-in + evidence properties" \
  "all($W2|.components[]; any(.properties[]; .name==\"opensidian:embedded-in\" and .value==\"webkit2gtk@2.50.1-0ubuntu1\") and any(.properties[]; .name==\"opensidian:evidence\"))"
chk "G1: bom-refs unique incl. nested" '[..|objects|select(has("bom-ref"))."bom-ref"]|length==(unique|length)'
chk "G1: webkit2gtk dependsOn its embedded components" \
  '(.dependencies[]|select(.ref|startswith("deb:webkit2gtk@"))|.dependsOn|sort)==([.components[]|select(.name=="webkit2gtk")|.components[]."bom-ref"]|sort)'
WK_EMBEDDED=$t/nope expect_fail "G1: missing embedded table fails closed" "embedded-component table" run
grep -v '^2\.50\.1' "$t/emb.tsv" >"$t/emb2.tsv"
WK_EMBEDDED=$t/emb2.tsv expect_fail "G1: webkit version absent from table fails closed" "no embedded-component rows for webkit 2.50.1" run
printf '2.50.1\tskia\tm140\t\tBSD-3-Clause\tx\n' >"$t/emb3.tsv"
WK_EMBEDDED=$t/emb3.tsv expect_fail "G1: row without purl fails closed" "bad row" run
# the REAL table covers the webkit the AppImage build pins (packaging/ci/appimage-build.sh)
pin=$(sed -n 's/.*libwebkit2gtk-4\.1-0=\([0-9.]*\)-.*/\1/p' "$here/../../packaging/ci/appimage-build.sh" | head -1)
real=$(awk -F'\t' -v v="$pin" '!/^#/ && $1==v && $2!="" && $3!="" && $4 ~ /^pkg:/ && $5!="" && $6!=""' "$here/webkit-embedded.tsv" | cut -f2 | sort | tr '\n' ' ')
[ "$real" = "angle libsysprof-capture pdfjs-dist skia xdgmime " ] && ok "G1: webkit-embedded.tsv covers pinned webkit $pin: $real" || bad "G1: webkit-embedded.tsv rows for pinned webkit '$pin': '$real'"

# ---- osv-fixed-gate ----
mk() { jq -n --arg eco "$1" --argjson ev "$2" '{results:[{packages:[{package:{name:"webkit2gtk",version:"2.50.1-0ubuntu1",ecosystem:"Ubuntu:26.04:LTS"},
  vulnerabilities:[{id:"UBUNTU-CVE-2026-0001",aliases:["CVE-2026-0001"],affected:[{package:{name:"webkit2gtk",ecosystem:$eco},ranges:[{type:"ECOSYSTEM",events:$ev}]}]}]}]}]}'; }
mk "Ubuntu:26.04:LTS" '[{"introduced":"0"},{"fixed":"2.50.2-0ubuntu1"}]' >"$t/o1.json"
mk "Ubuntu:26.04:LTS" '[{"introduced":"0"}]' >"$t/o2.json"
mk "Ubuntu:24.04:LTS" '[{"introduced":"0"},{"fixed":"2.48.0-1"}]' >"$t/o3.json"
gate() { set +e; "$here/osv-fixed-gate.sh" "$1" >"$t/g" 2>&1; rc=$?; set -e; }
gate "$t/o1.json"; [ "$rc" -ne 0 ] && grep -q "FIXED-AVAILABLE .*CVE-2026-0001.* fixed in 2.50.2-0ubuntu1" "$t/g" && ok "gate: fixed available -> rc=$rc" || { cat "$t/g"; bad "gate fixed"; }
gate "$t/o2.json"; [ "$rc" -eq 0 ] && grep -q "no-fix-yet" "$t/g" && ok "gate: no fix yet -> listed, rc=0" || { cat "$t/g"; bad "gate nofix"; }
gate "$t/o3.json"; [ "$rc" -eq 0 ] && ok "gate: fix only in another release -> rc=0" || { cat "$t/g"; bad "gate other release"; }
jq '.results[0].packages[0].package.ecosystem="Ubuntu"' "$t/o1.json" >"$t/o4.json"
gate "$t/o4.json"; [ "$rc" -ne 0 ] && ok "gate: release dropped by scanner -> fails closed rc=$rc" || { cat "$t/g"; bad "gate prefix"; }
echo '{}' >"$t/o5.json"; gate "$t/o5.json"; [ "$rc" -ne 0 ] && ok "gate: not osv json -> rc=$rc" || bad "gate bad json"
jq -n '{results:[]}' >"$t/o6.json"; gate "$t/o6.json"; [ "$rc" -eq 0 ] && ok "gate: no findings -> rc=0" || bad "gate empty"

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
