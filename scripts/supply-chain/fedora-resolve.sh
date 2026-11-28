#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# fedora-resolve.sh <arch> <image.tsv> <workdir> <build-pkg>...  > lock rows (stdout)
# MAINTAINER TOOL (never runs in CI): resolve the exact Fedora 44 package set of the portable build env.
#   1. NAMES: dnf5 resolves, into an empty installroot (--assumeno, nothing installed), the base image's
#      packages (<image.tsv> = rpm -qa of the pinned OCI rootfs) + <build-pkg>s -> binary name, arch, srpm.
#   2. VERSIONS: koji listTagged f44-updates (inherit = GA too, latest) AT ONE KOJI EVENT ($EVENT; default
#      = getLastEvent before $CUTOFF) -> for each binary its source build n-v-r at that event.
#      The mirrors are only used for step 1; versions never come from a mirror.
#   3. BYTES: kojipkgs unsigned rpm + data/sigcache/<key>/ signature header, spliced (rpm-splice.sh) into
#      the signed rpm, rpm -K against $KEYFILE in a PRIVATE rpmdb (the host rpmdb is never touched),
#      sha256 recorded.
#   4. CLOSURE PROOF: rpm --test -i of the whole set into an empty root: every dependency is satisfied by
#      the set itself.
# Output rows: name  epoch:version-release  arch  sha256(signed rpm)  srcname/version/release
set -euo pipefail
A=${1:?arch} IMG=${2:?image.tsv} W=${3:?workdir}; shift 3
[ $# -gt 0 ] || { echo "fedora-resolve: no build packages" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
KEYFILE=${KEYFILE:?KEYFILE = RPM-GPG-KEY-fedora-44-primary extracted from the pinned OCI rootfs}
KEYID=6d9f90a6
HUB=https://koji.fedoraproject.org/kojihub KOJI=https://kojipkgs.fedoraproject.org/packages
CUTOFF=${CUTOFF:-now}   # operator 2026-11-28 waived the 6-week age rule for this env: resolve at the current event
say(){ echo "[$(date +%H:%M:%S)] [resolve/$A] $*" >&2; }
die(){ say "FAIL: $*"; exit 1; }
mkdir -p "$W/rpms"; W=$(cd "$W" && pwd)
# kojihub drops TLS under load: 5 tries with backoff, the response must be well-formed XML and not a
# fault, written to a tmp file and only then emitted (a truncated reply never reaches a cache)
xr(){ local t=$W/.xr.$$ try
  for try in 1 2 3 4 5; do
    [ $try = 1 ] || { say "kojihub retry $try"; sleep $((try * try * 3)); }
    if curl -4 -fsS -m 900 -H 'Content-Type: text/xml' --data "$1" "$HUB" -o "$t.p" < /dev/null \
       && xmllint --noout "$t.p" 2>/dev/null && ! grep -q '<fault>' "$t.p"; then
      mv "$t.p" "$t"; cat "$t"; rm -f "$t"; return 0
    fi
    rm -f "$t.p"
  done
  return 1; }

# --- koji event + tagged builds ------------------------------------------------------------------
if [ -z "${EVENT:-}" ]; then
  ts=$(date -u -d "$CUTOFF" +%s)
  q="<?xml version='1.0'?><methodCall><methodName>getLastEvent</methodName><params><param><value><struct><member><name>__starstar</name><value><boolean>1</boolean></value></member><member><name>before</name><value><int>$ts</int></value></member></struct></value></param></params></methodCall>"
  EVENT=$(xr "$q" | xmllint --xpath '//member[name="id"]/value/int/text()' -) || die "getLastEvent"
fi
say "koji event $EVENT (cutoff $CUTOFF)"
TAGGED=${TAGGED:-$W/tagged-$EVENT.txt}
if [ ! -s "$TAGGED" ]; then
  q="<?xml version='1.0'?><methodCall><methodName>listTagged</methodName><params><param><value><string>f44-updates</string></value></param><param><value><struct><member><name>__starstar</name><value><boolean>1</boolean></value></member><member><name>event</name><value><int>$EVENT</int></value></member><member><name>inherit</name><value><boolean>1</boolean></value></member><member><name>latest</name><value><boolean>1</boolean></value></member></struct></value></param></params></methodCall>"
  xr "$q" > "$W/tagged.xml" || die "listTagged"
  xmllint --xpath '//member[name="nvr"]/value/string' "$W/tagged.xml" | sed 's#</string>#\n#g; s#<string>##g' | grep . | LC_ALL=C sort -u > "$TAGGED"
  rm -f "$W/tagged.xml"
fi
say "$(wc -l < "$TAGGED") builds tagged f44-updates(+GA) at event $EVENT"

# --- names: dnf5 transaction into an empty root --------------------------------------------------
if [ ! -s "$W/src-$A.tsv" ]; then
  R=$W/ir-$A; rm -rf "$R"; mkdir -p "$R" "$W/repos"
  printf '[fedora]\nname=f44\nmetalink=https://mirrors.fedoraproject.org/metalink?repo=fedora-44&arch=%s\n[updates]\nname=f44u\nmetalink=https://mirrors.fedoraproject.org/metalink?repo=updates-released-f44&arch=%s\n' "$A" "$A" > "$W/repos/f44.repo"
  D=(dnf5 -y --installroot="$R" --releasever=44 --forcearch="$A" --setopt=reposdir="$W/repos" --setopt=install_weak_deps=False --setopt=cachedir="$W/dnfcache-$A" --setopt=gpgcheck=False)
  { awk -F'\t' 'NF>=1 && $1!="" && $1!="gpg-pubkey" {print $1}' "$IMG"; printf '%s\n' "$@"; } | grep . | LC_ALL=C sort -u > "$W/want-$A.txt"; mapfile -t names < "$W/want-$A.txt"
  say "dnf5 resolve ${#names[@]} names (image + build)"
  "${D[@]}" install --assumeno "${names[@]}" > "$W/tx-$A.txt" 2>&1 || true
  # transaction table rows: " name arch evr repo size"
  awk '/^Installing|^ Installing/ {on=1; next} /^Transaction Summary/ {on=0} on && NF>=5 && $4 ~ /^(fedora|updates)$/ {print $1"\t"$2}' "$W/tx-$A.txt" | LC_ALL=C sort -u > "$W/names-$A.tsv"
  [ -s "$W/names-$A.tsv" ] || { tail -20 "$W/tx-$A.txt" >&2; die "dnf5 produced no transaction"; }
  awk -F"\t" '{print $1"."$2}' "$W/names-$A.tsv" > "$W/na-$A.txt"; mapfile -t na < "$W/na-$A.txt"
  "${D[@]}" -q repoquery --latest-limit=1 --qf '%{name}\t%{arch}\t%{sourcerpm}\n' "${na[@]}" < /dev/null | sed 's/\\t/\t/g' | grep . | LC_ALL=C sort -u > "$W/src-$A.tsv"
  [ "$(wc -l < "$W/src-$A.tsv")" = "$(wc -l < "$W/names-$A.tsv")" ] || die "repoquery: srpm missing for some names"
  rm -rf "$R" "$W/dnfcache-$A"
fi
n=$(wc -l < "$W/src-$A.tsv"); say "closure: $n binary packages"

# --- versions + bytes ------------------------------------------------------------------------------
# build_rpm <nvr> <name> <arch> -> "<version>-<release>" of that binary rpm in that build (cached per build)
mkdir -p "$W/brpms"
build_rpm(){ local c=$W/brpms/$1.tsv try
  for try in 1 2 3 4 5; do
    [ ! -s "$c" ] || break
    [ $try = 1 ] || sleep $((try * 5))
    xr "<?xml version='1.0'?><methodCall><methodName>listBuildRPMs</methodName><params><param><value><string>$1</string></value></param></params></methodCall>" < /dev/null \
      | xmllint --format - | awk '/<struct>/{d++; next} /<\/struct>/{ if (d==1 && r["name"]!="") print r["name"]"\t"r["version"]"\t"r["release"]"\t"r["arch"]; if (d==1) delete r; d--; k=""; next}
          d==1 && /<name>/{k=$0; gsub(/.*<name>|<\/name>.*/,"",k); next}
          d==1 && /<string>/ && k!=""{v=$0; gsub(/.*<string>|<\/string>.*/,"",v); r[k]=v; k=""; next}' > "$c.t" || true
    if [ -s "$c.t" ]; then mv "$c.t" "$c"; fi
  done
  awk -F'\t' -v n="$2" -v a="$3" '$1==n && $4==a {print $2"-"$3; f=1; exit} END{exit !f}' "$c" 2>/dev/null; }
K=$W/keydb-$A; rm -rf "$K"; mkdir -p "$K"
rpm --dbpath "$K" --import "$KEYFILE" < /dev/null
rpm --dbpath "$K" -q gpg-pubkey --qf '%{VERSION} %{SUMMARY}\n' >&2 < /dev/null
: > "$W/lock-$A.tsv"; i=0
while IFS=$'\t' read -r name ar srpm; do
  [ -n "$name" ] || continue
  i=$((i+1)); s=${srpm%.src.rpm}; sn=${s%-*}; sn=${sn%-*}
  b=$(awk -v n="$sn" '{r=$0; sub(/-[^-]+-[^-]+$/,"",r); if (r==n) print $0}' "$TAGGED" | tail -1)
  [ -n "$b" ] || die "$name: source $sn not tagged in f44-updates at event $EVENT"
  rel=${b##*-}; v=${b%-*}; ver=${v##*-}
  # a subpackage can carry its OWN version (perl-AutoLoader 5.74 from perl-5.42.3-525.fc44):
  # the binary version-release comes from koji listBuildRPMs of that build, never from the srpm
  bv=$(build_rpm "$b" "$name" "$ar") || die "$name.$ar: not a binary rpm of build $b (koji listBuildRPMs)"
  f=$name-$bv.$ar.rpm
  o=$W/rpms/$f
  if [ ! -s "$o" ]; then
    curl -4 -fsS --retry 5 --retry-all-errors --retry-delay 5 -m 900 -o "$W/.u" "$KOJI/$sn/$ver/$rel/$ar/$f" < /dev/null || die "$f: not in build $b ($KOJI/$sn/$ver/$rel/$ar/$f)"
    curl -4 -fsS --retry 5 --retry-all-errors --retry-delay 5 -m 120 -o "$W/.s" "$KOJI/$sn/$ver/$rel/data/sigcache/$KEYID/$ar/$f.sig" < /dev/null || die "$f: no $KEYID sigcache header"
    bash "$HERE/rpm-splice.sh" "$W/.u" "$W/.s" "$o"; rm -f "$W/.u" "$W/.s"
  fi
  k=$(rpm --dbpath "$K" -K "$o" 2>&1 < /dev/null) || true
  case "$k" in *": digests signatures OK") ;; *) rm -f "$o"; die "rpm -K $f: $k";; esac
  e=$(rpm --dbpath "$K" -qp --qf '%{EPOCHNUM}' "$o" < /dev/null)
  printf '%s\t%s:%s\t%s\t%s\t%s/%s/%s\n' "$name" "$e" "$bv" "$ar" "$(sha256sum "$o" | cut -d' ' -f1)" "$sn" "$ver" "$rel" >> "$W/lock-$A.tsv"
  [ $((i % 50)) != 0 ] || say "  $i/$n"
done < "$W/src-$A.tsv"
say "$i rpms: signed (splice), rpm -K digests signatures OK, sha256 recorded"

# --- closure proof ------------------------------------------------------------------------------
E=$W/empty-$A; rm -rf "$E"; mkdir -p "$E"; rpm --root "$E" --dbpath /rpmdb --initdb < /dev/null
awk -F'\t' '{e=$2; sub(/^[0-9]+:/,"",e); print "'"$W"'/rpms/"$1"-"e"."$3".rpm"}' "$W/lock-$A.tsv" > "$W/files-$A.txt"; mapfile -t files < "$W/files-$A.txt"
rpm --root "$E" --dbpath /rpmdb --ignorearch --nosignature --test -i "${files[@]}" < /dev/null >&2 || die "pinned set is not self-contained (rpm --test -i)"
rm -rf "$E" "$K"; [ -n "${KEEP_RPMS:-}" ] || rm -rf "$W/rpms"   # disk: only the hashes are kept
say "closure proof: rpm --test -i of ${#files[@]} rpms into an empty root OK"
LC_ALL=C sort "$W/lock-$A.tsv"
