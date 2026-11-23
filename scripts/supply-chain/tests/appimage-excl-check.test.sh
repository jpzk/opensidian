#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# appimage-excl-check.sh (#298) on fixtures: the REAL recipe passes on a clean AppDir/SBOM, and the
# gate turns red when an excluded library reappears in an AppDir (file or symlink, any depth), in a
# bundle SBOM's opensidian:bundled-libs, or when a guarded soname is dropped from EXCL.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
chk=$here/appimage-excl-check.sh
recipe=$(cd "$here/../.." && pwd)/packaging/ci/build-appimage.sh
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
expect() { # <want rc> <name> <args...>
  local want=$1 name=$2; shift 2; local rc=0
  "$chk" "$@" >"$t/out" 2>&1 || rc=$?
  if [ "$rc" = "$want" ]; then ok "$name (rc=$rc)"; else bad "$name: rc=$rc want $want"; sed 's/^/     /' "$t/out"; fi; }

# ---- clean fixture = what the slimmed portable AppDir bundles ----
A=$t/AppDir; mkdir -p "$A/usr/lib/webkit2gtk-4.1/injected-bundle"
for l in libwebkit2gtk-4.1.so.0 libjavascriptcoregtk-4.1.so.0 libsoup-3.0.so.0 libgio-2.0.so.0 \
         libglib-2.0.so.0 libsystemd.so.0 libxml2.so.16 libharfbuzz.so.0 libbrotlidec.so.1; do : >"$A/usr/lib/$l"; done
: >"$A/usr/lib/webkit2gtk-4.1/injected-bundle/libwebkit2gtkinjectedbundle.so"
sbom() { # <out> <comma-separated bundled libs...>
  local o=$1; shift
  jq -n --args '{bomFormat: "CycloneDX", components: [ $ARGS.positional[] | {name: ., properties: [{name: "opensidian:bundled-libs", value: .}]} ]}' "$@" >"$o"; }
sbom "$t/clean.json" "libwebkit2gtk-4.1.so.0" "libsoup-3.0.so.0" "libgio-2.0.so.0,libglib-2.0.so.0" "libsystemd.so.0,libudev.so.1"

expect 0 "real recipe: guard list all in EXCL" --recipe "$recipe"
expect 0 "clean AppDir" --appdir "$A"
expect 0 "clean SBOM" --sbom "$t/clean.json"
expect 0 "clean AppDir + SBOM together" --appdir "$A" --sbom "$t/clean.json"
grep -q "guarded sonames excluded" "$t/out" && ok "OK line printed" || bad "no OK line"

# ---- every #298 exclusion reappearing in the AppDir is caught ----
for l in libgssapi_krb5.so.2 libkrb5.so.3 libk5crypto.so.3 libkrb5support.so.0 libsqlite3.so.0 \
         libdbus-1.so.3 libmount.so.1 libblkid.so.1 libcom_err.so.2 libkeyutils.so.1 libX11.so.6 libc.so.6; do
  cp -a "$A" "$t/bad"; : >"$t/bad/usr/lib/$l"
  expect 1 "AppDir with $l" --appdir "$t/bad"
  grep -q "excluded library bundled: usr/lib/$l" "$t/out" || bad "$l not named in the output"
  rm -rf "$t/bad"
done
# versioned real file + symlink, in a subdir
cp -a "$A" "$t/bad"; mkdir -p "$t/bad/usr/lib/extra"; : >"$t/bad/usr/lib/extra/libsqlite3.so.0.8.6"
ln -s libsqlite3.so.0.8.6 "$t/bad/usr/lib/extra/libsqlite3.so.0"
expect 1 "AppDir with versioned file + symlink in a subdir" --appdir "$t/bad"
[ "$(grep -c 'excluded library bundled' "$t/out")" = 2 ] && ok "both file and symlink reported" || bad "file+symlink not both reported"
rm -rf "$t/bad"

# ---- SBOM listing an excluded lib is caught (also mid-list) ----
sbom "$t/bad.json" "libwebkit2gtk-4.1.so.0" "libsoup-3.0.so.0" "libgssapi_krb5.so.2,libkrb5.so.3"
expect 1 "SBOM listing krb5" --sbom "$t/bad.json"
sbom "$t/bad2.json" "libgio-2.0.so.0,libmount.so.1,libglib-2.0.so.0"
expect 1 "SBOM listing libmount mid-list" --sbom "$t/bad2.json"
grep -q "libmount.so.1" "$t/out" || bad "libmount not named"
expect 1 "clean AppDir but dirty SBOM" --appdir "$A" --sbom "$t/bad.json"

# ---- dropping a guarded entry from EXCL is caught ----
for drop in 'libsqlite3|' 'libgssapi_krb5|' 'libmount|' 'libdbus-1|' 'libkeyutils|'; do
  sed "s/$drop//" "$recipe" >"$t/recipe.sh"
  grep "^ *EXCL=" "$recipe" | grep -qF "$drop" || { bad "fixture: '$drop' not in the real EXCL"; continue; }
  expect 1 "EXCL without ${drop%|}" --recipe "$t/recipe.sh"
done

# ---- fail closed (rc=2) on unusable input ----
printf '#!/bin/sh\necho no excl here\n' >"$t/noexcl.sh"
expect 2 "recipe without EXCL" --recipe "$t/noexcl.sh"
mkdir -p "$t/empty"
expect 2 "AppDir without any .so" --appdir "$t/empty"
expect 2 "missing AppDir" --appdir "$t/nope"
echo '{"components": [{"name": "x"}]}' >"$t/nolibs.json"
expect 2 "SBOM without bundled-libs" --sbom "$t/nolibs.json"
echo 'not json' >"$t/garbage.json"
expect 2 "SBOM not JSON" --sbom "$t/garbage.json"
expect 2 "unknown flag" --frobnicate

[ "$fails" = 0 ] && echo "appimage-excl-check.test: all ok"
exit "$fails"
