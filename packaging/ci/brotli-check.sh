#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# CI COPY of harness scripts/brotli-check.sh @ 8e91f13 (unchanged), run in packaging/ci/appimage.sh.
# brotli-check.sh <AppImage|dir> — which libbrotlidec does this artifact BUNDLE?
# CVE-2026-111497 ("Node2Shell") is fixed in brotli 1.2.1 / 1.1.1 / 1.0.10.
# WebKitGTK decodes HTTP Content-Encoding br and WOFF2 through libbrotlidec.so.1,
# so a portable AppImage that bundles a pre-fix copy ships the bug.
#
# The AppImage is NEVER executed: the squashfs offset is the end of the runtime
# ELF (e_shoff + e_shentsize*e_shnum, read with od), and unsquashfs extracts
# from there. A tiny C probe dlopen()s the BUNDLED libbrotlidec by absolute
# path (LD_LIBRARY_PATH = that lib dir, so its libbrotlicommon is ours too) and
# prints BrotliDecoderVersion(). Output, one line:
#   BROTLI <file> version=<x.y.z> OK|REFUSED sha256=<artifact sha256>
# Exit 0 on OK, 1 on REFUSED (pre-fix or no bundled libbrotlidec), 2 on usage/tool error.
set -euo pipefail
[ $# -eq 1 ] || { echo "usage: $0 <AppImage|dir>" >&2; exit 2; }
A=$1
[ -e "$A" ] || { echo "brotli-check: $A does not exist" >&2; exit 2; }
for t in gcc od; do command -v $t >/dev/null || { echo "brotli-check: need $t" >&2; exit 2; }; done
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
SHA=-
if [ -d "$A" ]; then
  ROOT=$A
else
  command -v unsquashfs >/dev/null || { echo "brotli-check: need unsquashfs" >&2; exit 2; }
  SHA=$(sha256sum "$A" | cut -d' ' -f1)
  # ELF64 little-endian runtime: magic, class 2, data 1
  [ "$(od -An -tx1 -N6 "$A" | tr -d ' \n')" = 7f454c460201 ] || { echo "brotli-check: $A is not an ELF64 LE AppImage" >&2; exit 2; }
  u() { od -An -tu"$2" -j"$1" -N"$2" "$A" | tr -d ' \n'; }
  SHOFF=$(u 40 8); SHENT=$(u 58 2); SHNUM=$(u 60 2)
  OFF=$((SHOFF + SHENT * SHNUM))
  [ "$(od -An -c -j$OFF -N4 "$A" | tr -d ' \n')" = hsqs ] || { echo "brotli-check: no squashfs magic at offset $OFF" >&2; exit 2; }
  unsquashfs -q -n -o "$OFF" -d "$W/root" "$A" >/dev/null 2>&1 || { echo "brotli-check: unsquashfs -o $OFF failed" >&2; exit 2; }
  ROOT=$W/root
fi
DEC=$(find "$ROOT" \( -type f -o -type l \) -name libbrotlidec.so.1 2>/dev/null | sort | head -1)
if [ -z "$DEC" ]; then
  echo "BROTLI $A version=absent REFUSED sha256=$SHA"; exit 1
fi
DEC=$(readlink -f "$DEC")
cat > "$W/probe.c" <<'C'
#include <dlfcn.h>
#include <stdio.h>
#include <stdint.h>
int main(int c, char **v) {
  void *h = dlopen(v[1], RTLD_NOW | RTLD_LOCAL);
  if (!h) { fprintf(stderr, "dlopen: %s\n", dlerror()); return 3; }
  uint32_t (*f)(void) = (uint32_t (*)(void))dlsym(h, "BrotliDecoderVersion");
  if (!f) { fprintf(stderr, "dlsym: %s\n", dlerror()); return 3; }
  uint32_t x = f();
  printf("%u.%u.%u\n", x >> 24, (x >> 12) & 0xfff, x & 0xfff);
  return 0;
}
C
gcc -O0 -o "$W/probe" "$W/probe.c" -ldl
VER=$(LD_LIBRARY_PATH="$(dirname "$DEC")" "$W/probe" "$DEC") || { echo "brotli-check: probe failed on $DEC" >&2; exit 2; }
IFS=. read -r MA MI PA <<<"$VER"
# fixed lines: 1.2.1+, 1.1.1+, 1.0.10+; anything >1.2 is post-fix; <1.0 is ancient.
ok=0
if   [ "$MA" -gt 1 ]; then ok=1
elif [ "$MA" -eq 1 ]; then
  case "$MI" in
    0) [ "$PA" -ge 10 ] && ok=1;;
    1) [ "$PA" -ge 1 ] && ok=1;;
    2) [ "$PA" -ge 1 ] && ok=1;;
    *) [ "$MI" -gt 2 ] && ok=1;;
  esac
fi
if [ $ok = 1 ]; then echo "BROTLI $A version=$VER OK sha256=$SHA"; exit 0; fi
echo "BROTLI $A version=$VER REFUSED sha256=$SHA"; exit 1
