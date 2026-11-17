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
chmod +x "$t/bin/"*
run() { env PATH="$t/bin:$PATH" UNOWNED="$t/unowned" DISTRO=${DISTRO:-ubuntu-26.04} SOURCE_DATE_EPOCH=1700000000 \
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
chk "unique bom-refs + dependency graph complete" \
  '([.components[]."bom-ref"]|length)==([.components[]."bom-ref"]|unique|length) and (.dependencies[0].dependsOn|sort)==([.components[]."bom-ref"]|sort)'

# ---- negatives ----
echo libsoup-3.0.so.0 >"$t/unowned"
expect_fail "library not owned by dpkg" "not owned by any dpkg package" run
: >"$t/unowned"
rm "$W/WebKitGPUProcess"
expect_fail "missing webkit helper" "missing root" run
: >"$W/WebKitGPUProcess"; chmod +x "$W/WebKitGPUProcess"
DISTRO=fedora-44 expect_fail "non-deb distro" "is not ubuntu" run
DISTRO=debian-13 run >"$t/sbom.json" 2>/dev/null && chk "debian-13 namespace" 'all(.components[]|select(.purl|startswith("pkg:deb")); .purl|test("^pkg:deb/debian/.*distro=debian-13$"))'

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
