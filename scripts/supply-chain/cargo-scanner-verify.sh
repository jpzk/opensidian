#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# cargo-scanner-verify.sh <crate> — run right AFTER
#   cargo install --locked --version =<v> <crate>
# and BEFORE the tool is used. Fails closed (exit 1, installed binary removed) unless:
#   1. cargo-scanners.pins holds exactly one well-formed line for <crate>;
#   2. `cargo install --list` reports <crate> v<pinned version> (the workflow literal == the pin);
#   3. exactly one <crate>-<version>.crate sits in $CARGO_HOME/registry/cache/*/ and its sha256
#      == the pinned sha256 (cargo verified it against the live index; this checks it against the repo).
# Logs crate, version, sha256 on success. env: CARGO (default cargo), CARGO_HOME (default ~/.cargo).
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
pins=${CARGO_SCANNER_PINS:-$here/cargo-scanners.pins}
ch=${CARGO_HOME:-$HOME/.cargo}
cargo=${CARGO:-cargo}
log() { printf '%s\n' "$*" >&2; }
name=${1:-}
fail() { log "::error::$*"; [ -z "$name" ] || rm -f "$ch/bin/$name"; exit 1; }
case "$name" in cargo-audit|cargo-deny|cargo-vet) ;; *) name=; fail "usage: $0 cargo-audit|cargo-deny|cargo-vet" ;; esac
[ -r "$pins" ] || fail "pin file $pins missing"
bad=$(grep -Ev '^(#.*|crate [a-z0-9_-]+ [0-9]+\.[0-9]+\.[0-9]+ [0-9a-f]{64}|)$' "$pins" || true)
[ -z "$bad" ] || fail "malformed line(s) in $pins: $bad"
[ "$(awk -v n="$name" '$1 == "crate" && $2 == n' "$pins" | wc -l)" = 1 ] || fail "need exactly one pin for $name in $pins"
line=$(awk -v n="$name" '$1 == "crate" && $2 == n {print $3, $4}' "$pins"); ver=${line% *}; sha=${line#* }
# read the list once (no `| grep -q` under pipefail: an early grep exit SIGPIPEs cargo -> flaky red)
inst=$("$cargo" install --list) || fail "could not list installed crates via $cargo"
grep -Fqx "$name v$ver:" <<<"$inst" || fail "$name installed version != pinned $ver ($(grep -F "$name v" <<<"$inst" || echo none))"
shopt -s nullglob
c=("$ch"/registry/cache/*/"$name-$ver.crate")
[ "${#c[@]}" = 1 ] || fail "expected exactly one cached $name-$ver.crate under $ch/registry/cache, found ${#c[@]}"
echo "$sha  ${c[0]}" | sha256sum -c - >&2 || fail "sha256 mismatch for $name-$ver.crate (pinned $sha)"
log "scanner pinned: $name $ver sha256 $sha"
