#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# appimage-excl-check.sh [--recipe <build-appimage.sh>] [--appdir <dir>]... [--sbom <cdx.json>]...
#
# #298 guard: libraries the portable AppImage must NOT bundle stay out of it.
#   1. GUARD (below) = sonames removed from the bundle on purpose (host-provided, with the
#      evidence in packaging/appimage/inventory/reachability/). Every one of them must still
#      match build-appimage.sh's EXCL — deleting an entry from EXCL turns this red.
#   2. --appdir: no regular file / symlink anywhere under the dir (an AppDir, or an unpacked
#      squashfs-root) may have a basename matching EXCL.
#   3. --sbom: no component's opensidian:bundled-libs (webkit-bundle-sbom.sh) may list a
#      soname matching EXCL — the SBOM must describe the slimmed bundle.
# EXCL is read from the recipe exactly like webkit-bundle-sbom.sh does, so the three cannot drift.
# rc=0 clean, rc=1 violation (each printed), rc=2 usage / unreadable input. Fails closed.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
die() { echo "appimage-excl-check: $*" >&2; exit 2; }

# sonames excluded by #298 (+ the pre-existing host-only class it relies on)
GUARD=(
  libgssapi_krb5.so.2 libkrb5.so.3 libk5crypto.so.3 libkrb5support.so.0  # krb5: only libsoup GSSAPI
  libsqlite3.so.0                                                          # sqlite3: host lib
  libdbus-1.so.3                                                           # dbus: client must match host bus
  libmount.so.1 libblkid.so.1                                              # util-linux: on every Linux
  libcom_err.so.2 libkeyutils.so.1                                         # krb5's own deps (already host)
  libc.so.6 libX11.so.6 libwayland-client.so.0 libEGL.so.1                 # glibc / X / GL class
)

recipe=$root/packaging/ci/build-appimage.sh; dirs=(); sboms=()
while [ $# -gt 0 ]; do case "$1" in
  --recipe) recipe=${2:?}; shift 2;;
  --appdir) dirs+=("${2:?}"); shift 2;;
  --sbom)   sboms+=("${2:?}"); shift 2;;
  *) die "usage: $0 [--recipe f] [--appdir d]... [--sbom f]...";; esac; done
[ -r "$recipe" ] || die "cannot read recipe $recipe"
EXCL=$(sed -n "s/^ *EXCL='\(.*\)'$/\1/p" "$recipe")
[ -n "$EXCL" ] || die "no EXCL='...' line in $recipe"
[ "$(printf '%s\n' "$EXCL" | wc -l)" -eq 1 ] || die "more than one EXCL line in $recipe"

fails=0
for s in "${GUARD[@]}"; do
  printf '%s\n' "$s" | grep -qE "$EXCL" || { echo "FAIL guard: $s is no longer matched by EXCL in $recipe"; fails=1; }
done

for d in ${dirs[@]+"${dirs[@]}"}; do
  [ -d "$d" ] || die "--appdir $d: not a directory"
  n=0
  while IFS= read -r f; do
    n=$((n+1))
    b=$(basename "$f")
    printf '%s\n' "$b" | grep -qE "$EXCL" && { echo "FAIL appdir: excluded library bundled: ${f#"$d"/}"; fails=1; }
  done < <(find "$d" \( -type f -o -type l \) -name '*.so*')
  [ "$n" -gt 0 ] || die "--appdir $d: no .so files at all (wrong dir?)"
  echo "appdir $d: $n .so files checked"
done

for j in ${sboms[@]+"${sboms[@]}"}; do
  [ -r "$j" ] || die "--sbom $j: unreadable"
  command -v jq >/dev/null || die "missing jq"
  libs=$(jq -r '[.. | objects | select(has("properties")) | .properties[]? | select(.name == "opensidian:bundled-libs") | .value]
               | map(split(","))| add // [] | .[] | select(length > 0)' "$j") || die "--sbom $j: not JSON"
  [ -n "$libs" ] || die "--sbom $j: no opensidian:bundled-libs at all (not a bundle SBOM?)"
  while IFS= read -r b; do
    printf '%s\n' "$b" | grep -qE "$EXCL" && { echo "FAIL sbom: excluded library listed as bundled: $b ($j)"; fails=1; }
  done <<<"$libs"
  echo "sbom $j: $(wc -l <<<"$libs") bundled libs checked"
done

[ "$fails" -eq 0 ] && echo "appimage-excl-check: OK (${#GUARD[@]} guarded sonames excluded)"
exit "$fails"
