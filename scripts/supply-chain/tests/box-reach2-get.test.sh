#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# box-reach2.sh get() (#307, CodeAnt): the download helper must fail closed. A failed or empty
# download, or a sha256 that differs from the pin, returns non-zero and leaves no archive behind
# for the later probes; every call site pins a sha256 and aborts the script when get fails.
# Offline: file:// URLs only, no network.
set -euo pipefail
root=$(cd "$(dirname "$0")/../../.." && pwd)
s=$root/packaging/appimage/inventory/reachability/box-reach2.sh
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }

# load die()/get() exactly as written in the script (one-line or multi-line definitions)
awk '/^(die|get)\(\) *\{/ { if ($0 ~ /\}[[:space:]]*$/) { print; next } p=1 }
     p { print; if ($0 ~ /^\}[[:space:]]*$/) p=0 }' "$s" >"$t/fns.sh"
grep -q '^get()' "$t/fns.sh" || { echo "FAIL get() not found in $s"; exit 1; }
# shellcheck disable=SC1091
. "$t/fns.sh"

printf 'archive bytes\n' >"$t/src.tar"; : >"$t/empty.tar"
good=$(sha256sum "$t/src.tar" | cut -c1-64)
wrong=0000000000000000000000000000000000000000000000000000000000000000

run() { # <want: ok|fail> <name> <file> <get args...>
  local want=$1 name=$2 f=$3; shift 3; local rc=0
  ( cd "$t/w" && set +e && get "$@" ) >"$t/out" 2>&1 || rc=$?
  if [ "$want" = ok ]; then
    if [ "$rc" = 0 ] && [ -s "$t/w/$f" ]; then ok "$name (rc=0)"; else bad "$name: rc=$rc, want 0 + $f present"; sed 's/^/     /' "$t/out"; fi
  else
    if [ "$rc" != 0 ] && [ ! -e "$t/w/$f" ]; then ok "$name (rc=$rc, no $f left)"
    else bad "$name: rc=$rc, want non-zero and no $f"; sed 's/^/     /' "$t/out"; fi
  fi; }

mkdir -p "$t/w"
run fail "download fails (missing URL)"  a.tar "file://$t/nope.tar" a.tar "$good"
[ ! -e "$t/w/a.tar.part" ] && ok "no .part left after a failed download" || bad "a.tar.part left behind"
run fail "download is empty"              b.tar "file://$t/empty.tar" b.tar "$good"
run fail "sha256 differs from the pin"    c.tar "file://$t/src.tar" c.tar "$wrong"
run fail "no pinned sha256 given"         d.tar "file://$t/src.tar" d.tar
run ok   "good download, pinned sha256"   e.tar "file://$t/src.tar" e.tar "$good"
grep -qx "e.tar $good" "$t/out" && ok "prints <file> <sha256>" || bad "no '<file> <sha256>' line"
run ok   "cached archive, no re-download" e.tar "file://$t/nope.tar" e.tar "$good"
printf 'tampered\n' >"$t/w/f.tar"
rc=0; ( cd "$t/w" && set +e && get "file://$t/src.tar" f.tar "$good" ) >"$t/out" 2>&1 || rc=$?
[ "$rc" != 0 ] && ok "cached archive with a wrong sha256 rejected (rc=$rc)" || bad "tampered cached archive accepted"

# every call site pins a 64-hex sha256 and aborts the script when get fails
n=0
while IFS= read -r l; do
  n=$((n+1))
  if [[ $l =~ ^get\ https://[^[:space:]]+\ [^[:space:]]+\ [0-9a-f]{64}\ \|\|\ die ]]; then ok "call site pinned + fatal: ${l:0:60}..."
  else bad "call site not pinned/fatal: $l"; fi
done < <(grep -E '^[[:space:]]*get ' "$s")
[ "$n" -ge 3 ] && ok "$n get call sites checked" || bad "only $n get call sites found"

[ "$fails" = 0 ] && echo "box-reach2-get: all ok" || { echo "box-reach2-get: FAILED"; exit 1; }
