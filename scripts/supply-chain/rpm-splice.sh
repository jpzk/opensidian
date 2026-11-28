#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# rpm-splice.sh <unsigned.rpm> <sighdr.sig> <out.rpm>
# Rebuild Koji's SIGNED copy of an rpm from the two immutable files kojipkgs keeps for every build:
#   packages/<n>/<v>/<r>/<arch>/<file>.rpm                    the unsigned build output
#   packages/<n>/<v>/<r>/data/sigcache/<keyid>/<arch>/<file>.rpm.sig   the signature header Fedora signed
# (data/signed/<keyid>/ copies are garbage-collected once a build is superseded; the sigcache is not).
# out = lead(96) + signature header (8-byte aligned) + the unsigned file's main header + payload,
# exactly what koji's write-signed-rpm does. The result is a normal signed rpm: rpm -K verifies
# the OpenPGP header signature against the imported Fedora key plus the header/payload digests, so a
# wrong or tampered piece fails verification - this script only concatenates, it trusts nothing.
set -euo pipefail
u=${1:?unsigned rpm} s=${2:?sigcache .sig} o=${3:?out rpm}
be32(){ od -An -tu4 --endian=big -j "$2" -N4 "$1" | tr -d ' '; }
hlen(){ local n h; n=$(be32 "$1" $(( $2 + 8 ))); h=$(be32 "$1" $(( $2 + 12 ))); echo $(( 16 + n*16 + h )); }
magic(){ od -An -tx1 -j "$2" -N4 "$1" | tr -d ' '; }
[ "$(od -An -tx1 -N4 "$u" | tr -d ' ')" = edabeedb ] || { echo "rpm-splice: $u is not an rpm (lead magic)" >&2; exit 1; }
[ "$(magic "$u" 96)" = 8eade801 ] || { echo "rpm-splice: $u: no signature header at 96" >&2; exit 1; }
[ "$(magic "$s" 0)" = 8eade801 ] || { echo "rpm-splice: $s is not a signature header" >&2; exit 1; }
ul=$(hlen "$u" 96); up=$(( (ul + 7) / 8 * 8 ))
[ "$(magic "$u" $(( 96 + up )))" = 8eade801 ] || { echo "rpm-splice: $u: main header not where expected" >&2; exit 1; }
sl=$(hlen "$s" 0); sp=$(( (sl + 7) / 8 * 8 ))
{ head -c 96 "$u"; head -c "$sl" "$s"; head -c $(( sp - sl )) /dev/zero; tail -c +$(( 96 + up + 1 )) "$u"; } > "$o.part"
mv "$o.part" "$o"
