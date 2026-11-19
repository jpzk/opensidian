#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Resolve pkg=version + .deb sha256 for the AppImage build set at ONE dated snapshot.ubuntu.com
# timestamp, exactly the way apt inside appimage-build.sh picks them (all suites priority 500 ->
# highest version across resolute, resolute-updates, resolute-security; main universe), for amd64
# AND arm64. Used for the snapshot bump (docs/supply-chain.md) and by snapshot-bump.yml.
#   snapshot-resolve.sh [--keyring K] <SNAP YYYYMMDDTHHMMSSZ> <pkg>...  > lock.tsv
# Every InRelease is checked with gpgv against K (default: the ubuntu-archive-keyring of the
# host; the bump workflow runs on ubuntu-26.04), every Packages.xz against the sha256 in the
# VERIFIED InRelease. Fails closed: bad signature, hash mismatch, a package missing on one arch,
# or amd64 != arm64 version (one PKGS line pins both arches).
# Output (TSV): name  version  deb-arch(amd64|arm64)  sha256  suite  filename
set -euo pipefail
say(){ echo "[$(date +%H:%M:%S)] [snapshot-resolve] $*" >&2; }
die(){ say "FAIL: $*"; exit 1; }
KR=/usr/share/keyrings/ubuntu-archive-keyring.gpg
[ "${1:-}" = --keyring ] && { KR=${2:?keyring}; shift 2; }
SNAP=${1:?SNAP}; shift
[[ "$SNAP" =~ ^[0-9]{8}T[0-9]{6}Z$ ]] || die "SNAP '$SNAP' is not a fixed YYYYMMDDTHHMMSSZ timestamp"
[ $# -gt 0 ] || die "no packages"
[ -s "$KR" ] || die "no keyring $KR"
for t in curl gpgv xz dpkg awk sha256sum; do command -v $t >/dev/null || die "missing $t"; done
W=$(mktemp -d); trap 'rm -rf "$W"' EXIT
BASE=https://snapshot.ubuntu.com/ubuntu/$SNAP
SUITES="resolute resolute-updates resolute-security"; COMPS="main universe"; ARCHES="amd64 arm64"
WANT=" $* "
for s in $SUITES; do
  curl -4 -fsS --retry 3 -m 60 -o "$W/$s.InRelease" "$BASE/dists/$s/InRelease" || die "fetch $s/InRelease"
  gpgv --keyring "$KR" --output "$W/$s.Release" "$W/$s.InRelease" 2>"$W/$s.gpgv" || { cat "$W/$s.gpgv" >&2; die "$s InRelease signature"; }
  say "$s: $(grep -m1 '^Date:' "$W/$s.Release") (signature OK)"
  for c in $COMPS; do for a in $ARCHES; do
    p=$c/binary-$a/Packages.xz
    want=$(awk -v p="$p" '/^SHA256:/{f=1;next} /^[A-Za-z]/{f=0} f&&$3==p{print $1}' "$W/$s.Release")
    [[ "$want" =~ ^[0-9a-f]{64}$ ]] || die "$s: no sha256 for $p in InRelease"
    curl -4 -fsS --retry 3 -m 300 -o "$W/pk.xz" "$BASE/dists/$s/$p" || die "fetch $s/$p"
    [ "$(sha256sum "$W/pk.xz" | cut -d' ' -f1)" = "$want" ] || die "$s/$p sha256 != InRelease"
    xz -dc "$W/pk.xz" | awk -v want="$WANT" -v s="$s" -v a="$a" '
      /^Package: /{n=$2} /^Version: /{v=$2} /^Filename: /{f=$2} /^SHA256: /{h=$2}
      /^$/{ if (n!="" && index(want," " n " ")) print n "\t" v "\t" a "\t" h "\t" s "\t" f; n=v=f=h="" }
      END{ if (n!="" && index(want," " n " ")) print n "\t" v "\t" a "\t" h "\t" s "\t" f }' >> "$W/all.tsv"
  done; done
done
printf '# snapshot.ubuntu.com %s  suites %s  components %s\n# name\tversion\tdeb-arch\tsha256\tsuite\tfilename\n' "$SNAP" "$SUITES" "$COMPS"
for n in "$@"; do
  declare -A best=()
  for a in $ARCHES; do
    line=""; while IFS=$'\t' read -r pn pv pa ph ps pf; do
      [ "$pn" = "$n" ] && [ "$pa" = "$a" ] || continue
      if [ -z "$line" ] || dpkg --compare-versions "$pv" gt "$(cut -f2 <<<"$line")"; then line="$pn	$pv	$pa	$ph	$ps	$pf"; fi
    done < "$W/all.tsv"
    [ -n "$line" ] || die "$n: not in the $a indexes at $SNAP"
    [[ "$(cut -f4 <<<"$line")" =~ ^[0-9a-f]{64}$ ]] || die "$n/$a: no sha256"
    best[$a]=$line; printf '%s\n' "$line"
  done
  [ "$(cut -f2 <<<"${best[amd64]}")" = "$(cut -f2 <<<"${best[arm64]}")" ] || die "$n: amd64 $(cut -f2 <<<"${best[amd64]}") != arm64 $(cut -f2 <<<"${best[arm64]}")"
  unset best
done
say "resolved $# packages x 2 arches at $SNAP"
