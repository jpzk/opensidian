#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# glib-vendor-check.sh: rc=0 on the repo; rc!=0 on scratch copies where (1) one byte of a
# vendored file is edited, (2) one byte of the fixed file is edited, (3) an extra file is
# added, (4) a file is removed, (5) the patch is reverted (pristine vendored), (6) the
# Cargo.toml [patch.crates-io] line is gone. Never touches the checkout (mktemp copies).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
export GLIB_CRATE_CACHE=$t/cache
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
scratch() { rm -rf "$t/r"; mkdir -p "$t/r/src-tauri/vendor"
  cp "$root/src-tauri/Cargo.toml" "$root/src-tauri/Cargo.lock" "$t/r/src-tauri/"
  cp -a "$root/src-tauri/vendor/glib-0.18.5" "$t/r/src-tauri/vendor/"; }
expect_fail() { local name=$1 pat=$2
  set +e; "$here/glib-vendor-check.sh" "$t/r" >"$t/out" 2>&1; rc=$?; set -e
  if [ "$rc" -ne 0 ] && grep -q -- "$pat" "$t/out"; then ok "$name: rc=$rc"; else cat "$t/out"; bad "$name: rc=$rc (want !=0 and '$pat')"; fi; }
flip() { # flip the first byte of $1 (xor 1) in place
  local f=$1 b; b=$(od -An -tu1 -N1 "$f" | tr -d ' ')
  { printf "\\$(printf %03o $((b ^ 1)))"; tail -c +2 "$f"; } >"$f.new" && mv "$f.new" "$f"; }
V=src-tauri/vendor/glib-0.18.5

if "$here/glib-vendor-check.sh" >"$t/out" 2>&1; then ok "repo: rc=0 ($(tail -1 "$t/out" | cut -c1-40))"; else cat "$t/out"; bad "repo: rc!=0"; fi

scratch; flip "$t/r/$V/src/lib.rs"
expect_fail "one byte edited in src/lib.rs" "vendor differs from pristine"
scratch; sed -i 's/let mut p: \*mut libc::c_char/let mut p: *mut  libc::c_char/' "$t/r/$V/src/variant_iter.rs"
expect_fail "one byte added in the fixed file" "vendor != pristine + upstream fix"
scratch; echo x >"$t/r/$V/src/extra.rs"
expect_fail "extra file" "vendor differs from pristine"
scratch; rm "$t/r/$V/README.md"
expect_fail "file removed" "vendor differs from pristine"
scratch; ( cd "$t/r/$V" && GIT_DIR=/nonexistent git apply -R -p1 "$here/glib-fix/variantstriter.patch" )
expect_fail "fix reverted (pristine vendored)" "expected exactly src/variant_iter.rs"
scratch; sed -i '/^glib = { path = "vendor\/glib-0.18.5" }/d' "$t/r/src-tauri/Cargo.toml"
expect_fail "[patch.crates-io] line removed" "no \[patch.crates-io\]"

# crates.io marks 0.18.5 yanked (checksum unchanged): must refuse
scratch; curl -4 -fsSL -m 60 -o "$t/index" https://index.crates.io/gl/ib/glib
jq -c 'if .vers == "0.18.5" then .yanked = true else . end' "$t/index" >"$t/index.yanked"
GLIB_INDEX_URL="file://$t/index.yanked" expect_fail "0.18.5 yanked on crates.io" "is yanked on crates.io"

[ "$fails" -eq 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
