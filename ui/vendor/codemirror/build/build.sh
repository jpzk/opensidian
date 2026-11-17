#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# build.sh — rebuild ui/vendor/codemirror/cm6.js from the committed, pinned recipe (issue #188).
# Repo-local rewrite of the harness recipe's build mode (the original drove a remote build box over
# ssh; package.json, package-lock.json, entry.js are byte-identical to the original recipe, the
# banner.txt only differs in the two comment lines that pointed at the retired prose notes).
#
#   sh build.sh [out.js]   npm ci --ignore-scripts (lockfile-pinned tarballs, sha512 integrity)
#                          + esbuild with the original flags -> out.js (default: a temp file);
#                          prints CM6-SHA256 <hex>; with CMP=<file> asserts byte identity.
#   sh build.sh check      every package in package-lock.json published <= CUTOFF (registry time)
#
# Needs node + npm on PATH (CI: the opt-in cm6-rebuild job of supply-chain.yml, pinned node).
# Never runs package install scripts. Writes only under a temp dir.
set -eu
HERE=$(cd "$(dirname "$0")" && pwd)
CUTOFF=2026-09-01T23:59:59Z
REG=${NPM_REGISTRY:-https://registry.npmjs.org}
mode=${1:-build}
case $mode in
check)
  bad=0
  for kv in $(jq -r '.packages | to_entries[] | select(.key!="") | "\(.key|sub(".*node_modules/";""))@\(.value.version)"' "$HERE/package-lock.json"); do
    p=${kv%@*}; v=${kv##*@}
    t=$(curl -4 -fsS --retry 3 -m 60 "$REG/$p" | jq -r --arg v "$v" '.time[$v]')
    if [ "$t" \> "$CUTOFF" ]; then echo "LATE $p $v $t"; bad=1; else echo "ok   $p $v $t"; fi
  done
  [ $bad = 0 ] && echo "CM6-CHECK PASS (all <= $CUTOFF)" || { echo "CM6-CHECK FAIL"; exit 1; }
  ;;
*)
  w=$(mktemp -d); trap 'rm -rf "$w"' EXIT
  out=${1:-$w/cm6.js}; [ "$mode" = build ] && out=$w/cm6.js
  cp "$HERE/package.json" "$HERE/package-lock.json" "$HERE/entry.js" "$HERE/banner.txt" "$w/"
  ( cd "$w" && export HOME="$w/.home" npm_config_cache="$w/.npm" npm_config_registry="$REG/" \
    && npm ci --ignore-scripts --no-audit --no-fund \
    && ./node_modules/.bin/esbuild --version \
    && ./node_modules/.bin/esbuild entry.js --bundle --format=iife --platform=browser --target=es2020 \
         --minify --charset=utf8 --legal-comments=none --banner:js="$(cat banner.txt)" \
         --log-level=warning --outfile=cm6.build.js )
  cp "$w/cm6.build.js" "$out"
  h=$(sha256sum "$out" | cut -d' ' -f1)
  echo "CM6-SHA256 $h $(wc -c <"$out") bytes"
  if [ -n "${CMP:-}" ]; then
    c=$(sha256sum "$CMP" | cut -d' ' -f1)
    [ "$h" = "$c" ] && echo "CM6-REBUILD IDENTICAL ($CMP)" || { echo "CM6-REBUILD DIFFERS: $CMP=$c"; exit 1; }
  fi
  ;;
esac
