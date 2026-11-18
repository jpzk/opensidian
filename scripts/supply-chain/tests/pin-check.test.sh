#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# pin-check.sh (#189) against fixtures/pin-check/<rule>-<n>/ (each a mini repo with ONE violation of
# <rule>; it must be rc=1 and report ONLY <rule>), allow-<n>/ (broken / stale allowlist entries: rc=1
# with a "PIN allow" line) and ok-1/ (every pattern pinned, comments/echo/step names, a valid named
# scanner entry: rc=0, 0 findings). Then mutations of a scratch copy of ok-1 must each turn it red.
# Read-only on the checkout (mktemp copies). PIN_CHECK_TODAY is fixed so expiries are deterministic.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); fx=$here/tests/fixtures/pin-check
export PIN_CHECK_TODAY=2026-11-17
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0; n=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
run() { set +e; bash "$here/pin-check.sh" "$1" >"$t/out" 2>&1; rc=$?; set -e; }

for d in "$fx"/*/; do d=${d%/}; c=$(basename "$d"); rule=${c%-*}; n=$((n+1)); run "$d"
  case "$rule" in
    ok)    if [ $rc = 0 ] && grep -q ', 0 finding(s)$' "$t/out"; then ok "$c: rc=0"; else cat "$t/out"; bad "$c: rc=$rc (want 0)"; fi ;;
    allow) if [ $rc = 1 ] && grep -q '^PIN allow ' "$t/out"; then ok "$c: rc=1 $(grep -m1 '^PIN allow' "$t/out" | cut -d: -f3- | cut -c1-70)"; else cat "$t/out"; bad "$c: rc=$rc (want 1 + PIN allow)"; fi ;;
    *)     other=$(grep '^PIN ' "$t/out" | grep -v "^PIN $rule " || true)
           if [ $rc = 1 ] && grep -q "^PIN $rule " "$t/out" && [ -z "$other" ]; then ok "$c: rc=1 $(grep -c "^PIN $rule " "$t/out")x $rule"
           else cat "$t/out"; bad "$c: rc=$rc (want 1, only PIN $rule)"; fi ;;
  esac
done
for r in uses image apt dnf cargo flatpak fetch pkgmgr allow; do ls -d "$fx/$r"-* >/dev/null 2>&1 || bad "no fixture for rule $r"; done

# mutations of ok-1: each one unpins one thing
mut() { local name=$1 file=$2 sedx=$3 want=$4
  rm -rf "$t/m"; cp -a "$fx/ok-1" "$t/m"; sed -i -E "$sedx" "$t/m/$file"
  cmp -s "$fx/ok-1/$file" "$t/m/$file" 2>/dev/null && { bad "mutation $name did not change $file"; return; }
  run "$t/m"; if [ $rc = 1 ] && grep -q "^PIN $want " "$t/out"; then ok "mutation $name: rc=1 PIN $want"; else cat "$t/out"; bad "mutation $name: rc=$rc (want PIN $want)"; fi; }
command -v cmp >/dev/null || cmp() { [ "$(sha256sum < "$2")" = "$(sha256sum < "$3")" ]; }
mut "uses sha -> tag"      .github/workflows/w.yml 's#actions/checkout@[0-9a-f]{40}#actions/checkout@v6#' uses
mut "apt drop =version"    .github/workflows/w.yml 's#curl=8.14.1-2#curl#' apt
mut "apt continuation"     .github/workflows/w.yml 's#elfutils=0.192-4#elfutils#' apt
mut "cargo drop --version" .github/workflows/w.yml 's#cargo-about --version 0.8.4#cargo-about#' cargo
mut "flatpak drop commit"  .github/workflows/w.yml '/--commit=/d' flatpak
mut "flatpak var -> branch" packaging/flatpak/runtimes.sh 's#^(  aarch64\) COMMIT_PLATFORM=)[0-9a-f]{64}#\149#' flatpak
mut "fetch drop sha check" .github/workflows/w.yml '/sha256sum -c/d' fetch
mut "image digest -> tag"  .github/workflows/w.yml 's#debian@sha256:[0-9a-f]{64}#debian:trixie#' image
mut "IMG var digest"       packaging/build.sh 's#^DIGEST=.*#DIGEST=trixie#' image
mut "TOOLS var version"    packaging/build.sh 's#lintian=2.122.0#lintian#' apt
mut "dnf drop version"     packaging/build.sh 's#rpm-build-6.0.1-2.fc44#rpm-build#' dnf
mut "oci drop digest check" packaging/build.sh 's#^podman image exists "\$FED" \|\| exit 1$#true#' image
mut "oci check other image" packaging/build.sh 's#^podman image exists "\$FED" \|\| exit 1$#podman image exists "$IMG" || exit 1#' image
mut "oci drop tag"         packaging/build.sh 's#&& podman tag "\$ID" localhost/fedora:44##' image
mut "FROM digest -> tag"  Containerfile 's#debian@sha256:[0-9a-f]{64}#debian:trixie#' image
mut "pip drop hashes"      .github/workflows/w.yml 's# --require-hashes##' pkgmgr
mut "allowlist expired"    supply-chain/pin-allow.tsv 's#2027-05-17#2026-11-16#' allow
echo "pin-check.test: $n fixtures"
[ $fails = 0 ] && echo "ALL PASS" || { echo "SOME FAILED"; exit 1; }
