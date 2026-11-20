#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# cargo-scanner-verify.sh + advisory-db-rev.sh (scanners pinned, advisory data fresh; operator
# decision 2026-11-19 15:49Z). No network, no real cargo: `cargo install --list` is a shim and the
# registry cache is a fake CARGO_HOME.
# RED: sha256 mismatch, installed version != pin, cached .crate missing, two cached copies, unknown
# crate, malformed pin file, duplicate pin (each: rc!=0 AND the installed binary removed);
# advisory DB missing / not a git checkout / older than the max age.
# GREEN: matching version + sha256; fresh advisory DB logs its revision.
# Also: every `cargo install --locked --version =<v> <crate>` in supply-chain.yml == the pin.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd); root=$(cd "$here/../.." && pwd)
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }

# workflow literal versions == pins (both directions)
# spelled via $ci so pin-check does not read the grep patterns as an unpinned install
ci='cargo install'
wf=$root/.github/workflows/supply-chain.yml
for c in cargo-audit cargo-deny cargo-vet; do
  pv=$(awk -v n="$c" '$1 == "crate" && $2 == n {print $3}' "$here/cargo-scanners.pins")
  wv=$(grep -Eo "$ci --locked --version =[0-9.]+ $c\$" "$wf" | awk '{print $5}' | tr -d =)
  [ -n "$pv" ] && [ "$wv" = "$pv" ] && grep -q "cargo-scanner-verify.sh $c\$" "$wf" \
    && ok "$c: workflow --version =$wv == pin, verified after install" || bad "$c: workflow '$wv' vs pin '$pv' (or no verify step)"
done
grep -Eq "$ci( --locked)? (cargo-audit|cargo-deny|cargo-vet)\$" "$wf" && bad "an unpinned scanner install is left in $wf" || ok "no unpinned scanner install in supply-chain.yml"

# fake crate + pins
mkdir -p "$t/src"; echo "fake crate body" > "$t/src/cargo-deny-9.9.9.crate"
good_sha=$(sha256sum "$t/src/cargo-deny-9.9.9.crate" | cut -d' ' -f1)
other_sha=$(printf x | sha256sum | cut -d' ' -f1)
mkpins() { printf '# test pins\n%s\n' "$@" > "$t/pins"; }
# case <name> <want pass|fail> <installed version> <ncache 0|1|2> <crate arg>
case_() { local name=$1 want=$2 iv=$3 nc=$4 crate=$5 rc
  rm -rf "$t/ch"; mkdir -p "$t/ch/bin" "$t/ch/registry/cache/index.crates.io-aaaa" "$t/ch/registry/cache/index.crates.io-bbbb"
  echo bin > "$t/ch/bin/$crate"
  [ "$nc" -ge 1 ] && cp "$t/src/cargo-deny-9.9.9.crate" "$t/ch/registry/cache/index.crates.io-aaaa/"
  [ "$nc" -ge 2 ] && cp "$t/src/cargo-deny-9.9.9.crate" "$t/ch/registry/cache/index.crates.io-bbbb/"
  printf '#!/bin/sh\n[ "$1 $2" = "install --list" ] || exit 9\necho "%s v%s:"\necho "    %s"\n' "$crate" "$iv" "$crate" > "$t/cargo"; chmod +x "$t/cargo"
  set +e; CARGO=$t/cargo CARGO_HOME=$t/ch CARGO_SCANNER_PINS=$t/pins bash "$here/cargo-scanner-verify.sh" "$crate" >"$t/so" 2>"$t/se"; rc=$?; set -e
  if [ "$want" = fail ]; then
    if [ "$rc" -ne 0 ] && [ ! -e "$t/ch/bin/$crate" ]; then ok "$name: rc=$rc, binary removed ($(grep -o '::error::.*' "$t/se" | head -1 | cut -c1-90))"
    else cat "$t/se"; bad "$name: rc=$rc (want fail + binary removed)"; fi
  else
    if [ "$rc" -eq 0 ] && [ -e "$t/ch/bin/$crate" ] && grep -q "scanner pinned: $crate 9.9.9 sha256 $good_sha" "$t/se"; then ok "$name: rc=0, logged"
    else cat "$t/se"; bad "$name: rc=$rc (want pass)"; fi
  fi; }
mkpins "crate cargo-deny 9.9.9 $good_sha";  case_ "green: version + sha256 match"  pass 9.9.9 1 cargo-deny
mkpins "crate cargo-deny 9.9.9 $other_sha"; case_ "sha256 mismatch"                fail 9.9.9 1 cargo-deny
mkpins "crate cargo-deny 9.9.9 $good_sha";  case_ "installed version != pin"       fail 9.9.8 1 cargo-deny
mkpins "crate cargo-deny 9.9.9 $good_sha";  case_ "no cached .crate"               fail 9.9.9 0 cargo-deny
mkpins "crate cargo-deny 9.9.9 $good_sha";  case_ "two cached copies"              fail 9.9.9 2 cargo-deny
mkpins "crate cargo-deny 9.9.9 $good_sha";  case_ "no pin for the crate"           fail 9.9.9 1 cargo-vet
mkpins "crate cargo-deny 9.9.9 XYZ";        case_ "malformed pin"                  fail 9.9.9 1 cargo-deny
mkpins "crate cargo-deny 9.9.9 $good_sha" "crate cargo-deny 9.9.9 $good_sha"; case_ "duplicate pin" fail 9.9.9 1 cargo-deny
set +e; bash "$here/cargo-scanner-verify.sh" semgrep >/dev/null 2>&1; rc=$?; set -e
[ "$rc" -ne 0 ] && ok "unknown crate name refused (rc=$rc)" || bad "unknown crate accepted"

# advisory-db-rev.sh
g() { git -C "$1" -c user.name=t -c user.email=t@t "${@:2}"; }
mkdir -p "$t/db"; git init -q "$t/db"; echo a > "$t/db/a"; g "$t/db" add a
GIT_COMMITTER_DATE='2026-11-18T00:00:00Z' g "$t/db" commit -qm a
now=$(date -u -d 2026-11-19T00:00:00Z +%s); old=$(date -u -d 2026-12-19T00:00:00Z +%s)
arun() { set +e; NOW=$1 bash "$here/advisory-db-rev.sh" test "${@:2}" >"$t/so" 2>"$t/se"; rc=$?; set -e; }
arun "$now" "$t/db";           [ "$rc" = 0 ] && grep -q "test advisory DB: $t/db [0-9a-f]\{40\} 2026-11-18T00:00:00Z" "$t/so" && ok "fresh DB: revision + date logged" || { cat "$t/so" "$t/se"; bad "fresh DB"; }
arun "$old" "$t/db";           [ "$rc" != 0 ] && grep -q stale "$t/se" && ok "DB older than 14 days: rc=$rc (stale)" || bad "stale DB passed"
arun "$now" "$t/nope";         [ "$rc" != 0 ] && ok "no DB dir: rc=$rc" || bad "missing DB passed"
mkdir -p "$t/plain"; arun "$now" "$t/plain"; [ "$rc" != 0 ] && ok "dir that is not a git checkout: rc=$rc" || bad "non-git DB passed"
arun "$now";                   [ "$rc" != 0 ] && ok "no dir given: rc=$rc" || bad "empty arg list passed"

[ "$fails" = 0 ] && echo "ALL PASS" || { echo "FAILED"; exit 1; }
