#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# webkit-apt.sh --check (#189): rc=0 on the repo; rc!=0 when appimage-build.sh's SNAP or one of
# its pins differs from webkit-apt.sh (the CVE scan must install exactly what the AppImage bundles),
# or when appimage-build.sh has no PKGS. No sudo, no network (only --check is exercised).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
AB=$root/packaging/ci/appimage-build.sh
run() { set +e; APPIMAGE_BUILD=$1 bash "$here/webkit-apt.sh" --check >"$t/out" 2>&1; rc=$?; set -e; }
expect_fail() { local name=$1 pat=$2; run "$t/ab.sh"
  if [ "$rc" -ne 0 ] && grep -q -- "$pat" "$t/out"; then ok "$name: rc=$rc"; else cat "$t/out"; bad "$name: rc=$rc (want !=0 and '$pat')"; fi; }

run "$AB"; if [ "$rc" = 0 ]; then ok "repo: rc=0 ($(tail -1 "$t/out" | cut -c1-60))"; else cat "$t/out"; bad "repo: rc=$rc"; fi
sed 's/^SNAP=20261004T000000Z/SNAP=20261101T000000Z/' "$AB" >"$t/ab.sh"
expect_fail "appimage-build.sh SNAP bumped alone" "!= appimage-build.sh SNAP"
sed 's/libwebkit2gtk-4.1-dev=2.52.6-0ubuntu0.26.04.1/libwebkit2gtk-4.1-dev=2.52.7-0ubuntu0.26.04.1/' "$AB" >"$t/ab.sh"
expect_fail "appimage-build.sh WebKit pin bumped alone" "is not in appimage-build.sh PKGS"
sed '/^PKGS=/d' "$AB" >"$t/ab.sh"
expect_fail "appimage-build.sh without PKGS" "no PKGS="

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
