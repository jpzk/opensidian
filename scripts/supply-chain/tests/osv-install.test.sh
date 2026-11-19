#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# osv-install.sh arch selection + pins (run from a copy so the pin file can be faulted; `uname`
# is shimmed on PATH). RED: unknown arch (with and without --pinned), --latest refused (mode removed), no pinned digest for the arch, malformed
# pin file, two versions, unknown option, digest mismatch (real download of the amd64 asset of
# the pinned version checked against the arm64 digest) -> rc!=0 and no binary left behind.
# GREEN: --pinned on the host arch installs the pinned version (osv-scanner --version), and
# the asset chosen for aarch64/arm64 is the linux_arm64 asset of the same pinned version.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
pins=$here/osv-scanner.pins
tag=$(awk '$1 == "version" {print $2}' "$pins")
d_amd=$(awk '$1 == "sha256" && $2 == "amd64" {print $3}' "$pins")
d_arm=$(awk '$1 == "sha256" && $2 == "arm64" {print $3}' "$pins")
[ -n "$tag" ] && [ -n "$d_amd" ] && [ -n "$d_arm" ] && [ "$d_amd" != "$d_arm" ] \
  && ok "pin file: $tag, one digest per arch" || bad "pin file incomplete"

# kit <name> <pins content> : a copy of the installer next to a (possibly faulted) pin file
kit() { mkdir -p "$t/$1"; cp "$here/osv-install.sh" "$t/$1/"; printf '%s\n' "$2" >"$t/$1/osv-scanner.pins"; }
# fake uname -m
shim() { mkdir -p "$t/shim-$1"; printf '#!/bin/sh\n[ "$1" = -m ] && { echo %s; exit 0; }\nexec /usr/bin/env -i PATH=/usr/bin:/bin uname "$@"\n' "$1" >"$t/shim-$1/uname"; chmod +x "$t/shim-$1/uname"; }
# try <name> <want pass|fail> <kit> <machine|host> <args...>
try() { local name=$1 want=$2 k=$3 m=$4 rc p=$PATH; shift 4
  rm -rf "$t/dest"; [ "$m" = host ] || { shim "$m"; p=$t/shim-$m:$PATH; }
  set +e; PATH=$p "$t/$k/osv-install.sh" "$@" "$t/dest" >"$t/so" 2>"$t/se"; rc=$?; set -e
  if [ "$want" = fail ]; then
    if [ "$rc" -ne 0 ] && [ ! -e "$t/dest/osv-scanner" ] && [ ! -e "$t/dest/osv-scanner.tmp" ]; then
      ok "$name: rc=$rc, no binary ($(grep -o '::error::.*' "$t/se" | head -1))"
    else cat "$t/se"; bad "$name: rc=$rc (want fail, no binary)"; fi
  else
    if [ "$rc" -eq 0 ] && [ -x "$t/dest/osv-scanner" ]; then ok "$name: rc=0"
    else cat "$t/se"; bad "$name: rc=$rc (want pass)"; fi
  fi; }

good=$(cat "$pins")
kit good "$good"
kit noarm "$(grep -v '^sha256 arm64' "$pins")"
kit badhex "$(sed 's/^sha256 arm64 .*/sha256 arm64 XYZ/' "$pins")"
kit twover "$good
version v9.9.9"
kit swapped "$(sed "s/^sha256 amd64 .*/sha256 amd64 $d_arm/" "$pins")"

try "unknown arch mips64 (--pinned)"         fail good   mips64  --pinned
try "unknown arch riscv64 (no flag)"        fail good   riscv64
try "--latest is refused (no latest mode)"  fail good   x86_64  --latest
try "no flag on host: still pinned only"    pass good   host
try "aarch64 without a pinned arm64 digest"  fail noarm  aarch64 --pinned
try "malformed pinned digest"                fail badhex aarch64 --pinned
try "two version lines"                      fail twover x86_64  --pinned
try "unknown option"                         fail good   x86_64  --pin
try "digest mismatch (amd64 asset vs arm64 digest)" fail swapped x86_64 --pinned
grep -q "sha256 mismatch for osv-scanner_linux_amd64 $tag" "$t/se" && ok "mismatch names asset + pinned tag" || bad "mismatch message"

# green on the host arch: pinned version actually installed and runs
try "--pinned on host $(uname -m)" pass good host --pinned
grep -q "osv-scanner release: $tag (pinned)" "$t/se" && ok "installed the pinned $tag" || bad "not the pinned tag"
"$t/dest/osv-scanner" --version 2>&1 | grep -q "version: ${tag#v}" && ok "binary reports ${tag#v}" || bad "binary version != ${tag#v}"
# the other arch: same version, its own asset + digest (downloaded + verified; exec of a foreign
# binary usually fails after the digest check -> look at the log)
case $(uname -m) in x86_64) other=aarch64 oa=arm64 ;; aarch64) other=x86_64 oa=amd64 ;; *) other= ;; esac
if [ -n "$other" ]; then
  # rc depends on binfmt (qemu-user may run it); the point is asset choice + digest check
  shim "$other"; rm -rf "$t/dest"
  set +e; PATH=$t/shim-$other:$PATH "$t/good/osv-install.sh" --pinned "$t/dest" >"$t/so" 2>"$t/se"; rc=$?; set -e
  if grep -q "osv-scanner_linux_$oa" "$t/se" && grep -q ": OK$" "$t/se" && ! grep -q mismatch "$t/se" \
     && { [ "$rc" -eq 0 ] || grep -q "does not run on this machine" "$t/se"; }; then
    ok "$other -> osv-scanner_linux_$oa $tag digest verified (rc=$rc)"
  else cat "$t/se"; bad "$other asset not verified (rc=$rc)"; fi
fi
[ "$fails" = 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
