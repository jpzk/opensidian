#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# rpm signature check pinned to ONE signer (PR #369 review, operator 2026-11-28 "the signature check is
# not properly done"). WHY: `rpm -K` = "digests signatures OK" for a package signed by ANY key the rpmdb
# trusts, and asserting only that our key is PRESENT among gpg-pubkeys proves nothing about the signer.
# So: verify against a PRIVATE keyring (--dbpath, _keyring=rpmdb) that holds exactly the pinned key,
# and per package require that every signature line names the pinned key and says OK.
#   rpm-sigcheck.sh keyring <keyfile> <fingerprint> <dbdir>   import keyfile into <dbdir>; fail unless
#                                                            the keyring is exactly {fingerprint}
#   rpm-sigcheck.sh keys <fingerprint> [dbdir]               fail unless keyring (default: system) == {fpr}
#   rpm-sigcheck.sh check <dbdir> <fingerprint> <rpm>...     per rpm: >=1 signature, all signed by fpr, all OK
# rpm 6 only: gpg-pubkey %{VERSION} and `rpmkeys --list` print the FULL v4 fingerprint and `rpmkeys -Kv`
# prints "key fingerprint: <fpr>: OK" (rpm 4.x printed the 8-hex short key ID there - not comparable).
set -euo pipefail
die() { echo "rpm-sigcheck: $*" >&2; exit 1; }
K=(--define '_keyring rpmdb')
v=$(rpm --version | sed -n 's/^RPM version \([0-9]*\)\..*/\1/p')
[ "$v" = 6 ] || die "rpm major '$v' != 6: key/signer output format not the one this check parses"
keys() {  # keys <fpr> [dbdir]
  local fpr=$1 l; l=$(rpmkeys "${K[@]}" ${2:+--dbpath "$2"} --list | awk '{print tolower($1)}' | LC_ALL=C sort -u)
  [ "$l" = "$fpr" ] || die "keyring ${2:-system} trusts [$(echo $l)], expected exactly $fpr"
}
case "${1:-}" in
keyring)
  kf=${2:?keyfile} fpr=${3:?fpr} db=${4:?dbdir}; mkdir -p "$db"
  [ -z "$(ls -A "$db")" ] || die "$db not empty (keyring must start empty)"
  rpmkeys "${K[@]}" --dbpath "$db" --import "$kf" || die "import $kf"
  keys "$fpr" "$db" ;;
keys) keys "${2:?fpr}" "${3:-}" ;;
check)
  db=${2:?dbdir} fpr=${3:?fpr}; shift 3; keys "$fpr" "$db"; bad=0
  long=${fpr: -16}
  for f in "$@"; do
    out=$(LC_ALL=C rpmkeys "${K[@]}" --dbpath "$db" -Kv "$f" 2>&1) || { echo "$out" >&2; echo "rpm-sigcheck: $f: rpmkeys -Kv failed" >&2; bad=1; continue; }
    ns=0 nd=0 e=""
    while IFS= read -r l; do
      l=${l#"${l%%[![:space:]]*}"}
      case "$l" in
        "$f:"|"") ;;
        *[Ss]ignature*)
          case "$l" in *"key fingerprint: $fpr: OK"|*"key ID $long: OK") ns=$((ns+1)) ;; *) e="$e | $l" ;; esac ;;
        *digest:\ OK) nd=$((nd+1)) ;;
        *) e="$e | $l" ;;
      esac
    done <<< "$out"
    [ $ns -ge 1 ] || e="$e | no signature by $fpr"
    [ $nd -ge 1 ] || e="$e | no digest"
    [ -z "$e" ] || { echo "rpm-sigcheck: $f:$e" >&2; bad=1; }
  done
  [ $bad = 0 ] || die "signer check failed" ;;
*) echo "usage: rpm-sigcheck.sh keyring <keyfile> <fpr> <dbdir> | keys <fpr> [dbdir] | check <dbdir> <fpr> <rpm>..." >&2; exit 2 ;;
esac
