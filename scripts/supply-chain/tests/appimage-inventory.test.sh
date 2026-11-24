#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# appimage-inventory.sh (#307, CodeAnt nitpicks 4+5): the inventory must fail closed.
#   - a rerun must not carry stale edges from a previous run's resolved.tsv;
#   - an arch the host loader cannot inspect (A != uname -m) must abort, not print INV-DONE;
#   - a bundle entry the loader cannot list must abort, not be skipped silently.
# Offline: the AppImage is a local fixture (a shell stub that extracts a tiny tree); no network.
set -euo pipefail
root=$(cd "$(dirname "$0")/../../.." && pwd)
s=$root/scripts/supply-chain/appimage-inventory.sh
host=$(uname -m)
[ "$host" = x86_64 ] || { echo "skip: fixture needs an x86_64 host (got $host)"; exit 0; }
V=selftest-$$; base=/var/tmp/appimg298
trap 'rm -rf "$base/inv-$V-"*' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }

fixture() { # <arch> <payload: elf|text>: stub AppImage that extracts usr/bin/opensidian
  local w=$base/inv-$V-$1; rm -rf "$w"; mkdir -p "$w"
  if [ "$2" = elf ]; then src=/bin/true; else src=; fi
  cat >"$w/opensidian-$V-$1-portable.AppImage" <<STUB
#!/bin/sh
[ "\$1" = --appimage-extract ] || exit 2
mkdir -p squashfs-root/usr/bin squashfs-root/usr/lib
if [ -n "$src" ]; then cp "$src" squashfs-root/usr/bin/opensidian; else echo 'not an elf' > squashfs-root/usr/bin/opensidian; fi
STUB
  echo "$w"; }

# 1. stale resolved.tsv from an earlier run must not survive a rerun
w=$(fixture "$host" elf)
printf 'STALE\tlibstale.so.9\t/nowhere/libstale.so.9\n' >"$w/resolved.tsv"
rc=0; bash "$s" "$V" "$host" >"$w/out" 2>&1 || rc=$?
if [ "$rc" = 0 ] && grep -q INV-DONE "$w/out" && ! grep -q STALE "$w/resolved.tsv" && grep -q '^opensidian' "$w/resolved.tsv"
then ok "rerun clears resolved.tsv (rc=0, fresh edges only)"
else bad "rerun: rc=$rc, stale=$(grep -c STALE "$w/resolved.tsv" || true)"; sed 's/^/     /' "$w/out"; fi
grep -q libstale "$w/from-host.tsv" && bad "stale edge leaked into from-host.tsv" || ok "from-host.tsv has no stale edge"

# 2. foreign arch: a pre-extracted tree the host loader cannot inspect -> abort, no INV-DONE
w=$(fixture aarch64 text); (cd "$w" && sh ./opensidian-$V-aarch64-portable.AppImage --appimage-extract)
rc=0; bash "$s" "$V" aarch64 >"$w/out" 2>&1 || rc=$?
if [ "$rc" != 0 ] && ! grep -q INV-DONE "$w/out"; then ok "foreign arch aborts (rc=$rc)"
else bad "foreign arch: rc=$rc, want non-zero and no INV-DONE"; sed 's/^/     /' "$w/out"; fi

# 3. native arch, an entry the loader cannot list -> abort, no INV-DONE
w=$(fixture "$host" text)
rc=0; bash "$s" "$V" "$host" >"$w/out" 2>&1 || rc=$?
if [ "$rc" != 0 ] && ! grep -q INV-DONE "$w/out"; then ok "loader failure aborts (rc=$rc)"
else bad "loader failure: rc=$rc, want non-zero and no INV-DONE"; sed 's/^/     /' "$w/out"; fi

[ "$fails" = 0 ] && echo "appimage-inventory: all ok" || { echo "appimage-inventory: FAILED"; exit 1; }
