#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# release-sums.sh — SHA256SUMS covers EVERY release asset (except SHA256SUMS itself), and the
# release's assets are exactly that set. Used by .github/workflows/release.yml; fails closed.
#
#   make <dir> <ver>          write <dir>/SHA256SUMS over every regular file in <dir> except
#                             SHA256SUMS: the ten Linux packages first in scripts/release.sh order
#                             (x86_64 then aarch64; each must exist), then every other file in
#                             LC_ALL=C name order. Prints the file.
#   check <SHA256SUMS> <assets.txt>
#                             assets.txt: one "<name> <digest>" line per release asset, digest
#                             "sha256:<64 hex>" (the GitHub asset digest). rc 0 only when the asset
#                             names minus SHA256SUMS == the names in SHA256SUMS (no extra, no missing,
#                             no duplicates, SHA256SUMS itself present) and every listed sha256 ==
#                             that asset's digest. A missing/malformed digest fails.
set -euo pipefail
die() { echo "release-sums: $*" >&2; exit 1; }
cmd=${1:-}; shift || true
case "$cmd" in
  make)
    [ $# -eq 2 ] || die "usage: make <dir> <ver>"
    D=$1; V=$2; [ -d "$D" ] || die "no dir $D"
    cd "$D"; rm -f SHA256SUMS
    F=()
    for A in x86_64 aarch64; do
      for K in -slim.AppImage -portable.AppImage .flatpak .rpm .deb; do
        f="opensidian-$V-$A$K"; [ -f "$f" ] || die "missing $f"; F+=("$f")
      done
    done
    while IFS= read -r f; do
      [ -f "$f" ] || die "not a regular file: $f"
      case " ${F[*]} " in *" $f "*) continue ;; esac
      F+=("$f")
    done <<< "$(LC_ALL=C ls -1A | LC_ALL=C sort | grep -vx SHA256SUMS)"
    sha256sum -- "${F[@]}" > SHA256SUMS
    cat SHA256SUMS ;;
  check)
    [ $# -eq 2 ] || die "usage: check <SHA256SUMS> <assets.txt>"
    S=$1; L=$2; [ -s "$S" ] || die "empty or missing $S"; [ -f "$L" ] || die "missing $L"
    grep -qvE '^[0-9a-f]{64}  [^ /]+$' "$S" && die "malformed SHA256SUMS line: $(grep -vE '^[0-9a-f]{64}  [^ /]+$' "$S" | head -1)"
    grep -qvE '^[^ ]+ [^ ]+$' "$L" && die "malformed assets line: $(grep -vE '^[^ ]+ [^ ]+$' "$L" | head -1)"
    t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
    cut -d' ' -f3 "$S" | LC_ALL=C sort > "$t/sums"
    cut -d' ' -f1 "$L" | LC_ALL=C sort > "$t/assets"
    [ -z "$(uniq -d "$t/sums")" ]   || die "duplicate SHA256SUMS entries: $(uniq -d "$t/sums" | tr '\n' ' ')"
    [ -z "$(uniq -d "$t/assets")" ] || die "duplicate asset names: $(uniq -d "$t/assets" | tr '\n' ' ')"
    grep -qx SHA256SUMS "$t/assets" || die "SHA256SUMS is not an asset"
    grep -qx SHA256SUMS "$t/sums" && die "SHA256SUMS lists itself"
    grep -vx SHA256SUMS "$t/assets" > "$t/want" || true
    if [ "$(cat "$t/want")" != "$(cat "$t/sums")" ]; then
      echo "assets not in SHA256SUMS: $(LC_ALL=C comm -23 "$t/want" "$t/sums" | tr '\n' ' ')" >&2
      echo "SHA256SUMS entries with no asset: $(LC_ALL=C comm -13 "$t/want" "$t/sums" | tr '\n' ' ')" >&2
      die "asset set != SHA256SUMS set"
    fi
    n=0
    while read -r h f; do
      d=$(awk -v f="$f" '$1 == f { print $2 }' "$L")
      [[ "$d" =~ ^sha256:[0-9a-f]{64}$ ]] || die "$f: no sha256 digest ('$d')"
      [ "${d#sha256:}" = "$h" ] || die "$f: SHA256SUMS $h != asset digest ${d#sha256:}"
      n=$((n+1))
    done <<< "$(sed 's/  / /' "$S")"
    echo "release-sums: OK, $n assets == SHA256SUMS entries, every digest matches" ;;
  *) die "usage: make <dir> <ver> | check <SHA256SUMS> <assets.txt>" ;;
esac
