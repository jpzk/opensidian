#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# release-sums.sh self-test (offline): make covers every file in release order; check accepts the
# exact asset set with matching digests and refuses every mismatch (negative cases).
set -euo pipefail
S=$(cd "$(dirname "$0")/.." && pwd)/release-sums.sh
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
fails=0
ok()  { echo "ok   $*"; }
bad() { echo "FAIL $*"; fails=1; }
eq()  { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2' want '$3'"; fi; }
refuses() { local name=$1 why=$2; shift 2
  if "$@" >"$t/so" 2>"$t/se"; then bad "$name: accepted (rc 0)"; elif ! grep -q -- "$why" "$t/se"; then bad "$name: refused for the wrong reason: $(tail -1 "$t/se")"; else ok "$name: refused ($(grep -o 'release-sums: .*' "$t/se" | head -c 90))"; fi; }
V=9.9.9; D=$t/d; mkdir "$D"
for A in x86_64 aarch64; do for K in -slim.AppImage -portable.AppImage .flatpak .rpm .deb; do echo "$A$K" > "$D/opensidian-$V-$A$K"; done; done
for f in opensidian-$V-universal.dmg opensidian-$V-universal.dmg.sha256 opensidian-$V-macos-aarch64-unsigned.app.zip \
         opensidian-$V-x86_64.cdx.json opensidian-$V-x86_64.cdx.json.sha256 opensidian-$V-macos-universal.cdx.json; do echo "$f" > "$D/$f"; done
"$S" make "$D" "$V" > /dev/null
eq "make: one line per file (16)" "$(wc -l < "$D/SHA256SUMS")" 16
eq "make: release.sh order first" "$(sed -n '1p;5p;6p;10p' "$D/SHA256SUMS" | cut -d' ' -f3 | tr '\n' ,)" \
   "opensidian-$V-x86_64-slim.AppImage,opensidian-$V-x86_64.deb,opensidian-$V-aarch64-slim.AppImage,opensidian-$V-aarch64.deb,"
eq "make: rest in C order" "$(sed -n '11,$p' "$D/SHA256SUMS" | cut -d' ' -f3 | LC_ALL=C sort -c 2>&1 && echo sorted)" sorted
eq "make: no self entry" "$(grep -c ' SHA256SUMS$' "$D/SHA256SUMS" || true)" 0
(cd "$D" && sha256sum -c --quiet SHA256SUMS) && ok "make: sums verify" || bad "make: sums verify"
"$S" make "$D" "$V" > /dev/null; eq "make: rerun idempotent (16)" "$(wc -l < "$D/SHA256SUMS")" 16
mkdir "$t/m"; cp "$D"/opensidian-$V-universal.dmg "$t/m/"; refuses "make: missing Linux package" "missing opensidian-" "$S" make "$t/m" "$V"
# asset list as GitHub reports it
(cd "$D" && for f in *; do echo "$f sha256:$(sha256sum "$f" | cut -c1-64)"; done) > "$t/a.txt"
"$S" check "$D/SHA256SUMS" "$t/a.txt" > "$t/so" && ok "check: exact set accepted ($(cat "$t/so"))" || bad "check: exact set refused: $(cat "$t/so")"
grep -v '\.cdx\.json\.sha256 ' "$t/a.txt" > "$t/n1"; refuses "check: asset missing" "asset set != SHA256SUMS set" "$S" check "$D/SHA256SUMS" "$t/n1"
{ cat "$t/a.txt"; echo "opensidian-$V-extra.zip sha256:$(printf '%064d' 0)"; } > "$t/n2"; refuses "check: extra asset not in SHA256SUMS" "asset set != SHA256SUMS set" "$S" check "$D/SHA256SUMS" "$t/n2"
grep -v '^SHA256SUMS ' "$t/a.txt" > "$t/n3"; refuses "check: SHA256SUMS not an asset" "SHA256SUMS is not an asset" "$S" check "$D/SHA256SUMS" "$t/n3"
sed "s/^\(opensidian-$V-universal.dmg sha256:\).*/\1$(printf '%064d' 1)/" "$t/a.txt" > "$t/n4"; refuses "check: digest mismatch" "!= asset digest" "$S" check "$D/SHA256SUMS" "$t/n4"
sed "s/^\(opensidian-$V-universal.dmg\) .*/\1 null/" "$t/a.txt" > "$t/n5"; refuses "check: null digest" "no sha256 digest" "$S" check "$D/SHA256SUMS" "$t/n5"
{ cat "$t/a.txt"; grep '\.dmg ' "$t/a.txt"; } > "$t/n6"; refuses "check: duplicate asset" "duplicate asset names" "$S" check "$D/SHA256SUMS" "$t/n6"
{ cat "$D/SHA256SUMS"; head -1 "$D/SHA256SUMS"; } > "$t/s7"; refuses "check: duplicate SHA256SUMS line" "duplicate SHA256SUMS entries" "$S" check "$t/s7" "$t/a.txt"
grep -v 'universal.dmg$' "$D/SHA256SUMS" > "$t/s8"; refuses "check: SHA256SUMS misses an asset" "asset set != SHA256SUMS set" "$S" check "$t/s8" "$t/a.txt"
{ cat "$D/SHA256SUMS"; echo "$(printf '%064d' 0)  SHA256SUMS"; } > "$t/s9"; refuses "check: SHA256SUMS lists itself" "lists itself" "$S" check "$t/s9" "$t/a.txt"
echo "garbage" >> "$t/s10"; refuses "check: malformed SHA256SUMS" "malformed SHA256SUMS" "$S" check "$t/s10" "$t/a.txt"
[ "$fails" -eq 0 ] && echo "release-sums.test: PASS" || { echo "release-sums.test: FAIL"; exit 1; }
