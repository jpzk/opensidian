#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# FEDORA 44 BUILD ENV (fedport, operator 2026-11-28 "change the portable build system from ubuntu to
# fedora"): the pinned container that packaging/ci/appimage-build.sh (release binary + AppImages + SBOM)
# and packaging/ci/linux-test.sh (linux.yml PR tests) run in. WHY: the portable AppImage bundles the
# WebKitGTK closure; Fedora 44 ships webkitgtk 2.54.x (WSA-2026-0006 fixed), Ubuntu 26.04 2.52.6.
#   fedora-env.sh image              host: load the pinned OCI archive into podman, print the image ref
#   fedora-env.sh fetch <dir>        host: put every rpm of the lock (this arch) into <dir>, sha256-checked
#   fedora-env.sh install <dir> <pkgset-out>   IN THE CONTAINER: verify + install exactly the lock
# PINS (no floating repo, nothing from a mirror that can change):
#   image  Fedora-Container-Base-Generic-44-1.7 (GA, 2026-04-22) OCI archive per arch: file sha256 from
#          the signed CHECKSUM + the manifest digest podman must carry (= packaging/ci/rpm-build.sh, checked).
#   rpms   packaging/ci/fedora-pkgset.lock.tsv: the WHOLE installed set per arch (base image packages
#          included), name + epoch:version-release + arch + sha256 of the SIGNED rpm + koji build path.
#          Versions = newest build in f44-updates (inherits GA) at ONE koji event (lock header). The
#          operator waived the 6-week age rule for this env 2026-11-28: that event is the CURRENT one at
#          resolve time; webkitgtk = 2.54.3-2.fc44. Resolved by scripts/supply-chain/fedora-resolve.sh.
#          Bytes come from kojipkgs (immutable build outputs): the unsigned rpm + its Fedora-44 signature
#          header from the sigcache, spliced (scripts/supply-chain/rpm-splice.sh) into the signed rpm.
#   checks host: sha256 == lock. container: sha256 == lock again, the image's own Fedora 44 key
#          (RPM-GPG-KEY-fedora-44-primary, fingerprint below) imported, rpm -K = "digests signatures OK"
#          for every file, rpm -U of the set with no repo at all, then rpm -qa == lock EXACTLY (no
#          package more, less or at another version) - any drift fails the build.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
LOCK=$HERE/fedora-pkgset.lock.tsv
KOJI=https://kojipkgs.fedoraproject.org/packages
KEYID=6d9f90a6
KEYFPR=36f612dcf27f7d1a48a835e4dbfcf71c6d9f90a6
ARCH=$(uname -m)
case "$ARCH" in
  x86_64)  OCI_SHA=75200f5752a74a21a616ca9a75e25beb594e2e117a0195c54f87c0b3e3974d1b
           DIGEST=sha256:f1e66cdd6eff2c9ccad192f8865af9be6d69b46b3f13329d2975a2d61a1296c5 ;;
  aarch64) OCI_SHA=eca19542a48a8e39b84e869713a1fa2408cbcc578de26c25ae72e3334ef968c1
           DIGEST=sha256:bebe5bd9e882469b64cc950da834288bdebca8921ec4bb50646378d672da0ff5 ;;
  *) echo "fedora-env: unsupported arch $ARCH (x86_64|aarch64)" >&2; exit 1 ;;
esac
OCI_URL=https://dl.fedoraproject.org/pub/fedora/linux/releases/44/Container/$ARCH/images/Fedora-Container-Base-Generic-44-1.7.$ARCH.oci.tar.xz
IMG=localhost/fedora@$DIGEST
say(){ echo "[$(date +%H:%M:%S)] [fedora-env/$ARCH] $*" >&2; }
die(){ say "FAIL: $*"; exit 1; }
[ -s "$LOCK" ] || die "no $LOCK"
# the lock itself is pinned: re-resolving it (scripts/supply-chain/fedora-resolve.sh) means updating
# this hash in the same commit; pin-check.sh (rule rpmlock) checks the same equality in CI
LOCK_SHA256=a85246f962f69eb2bb195f845db161bc8f22a00fa946bcf6607871e5bdb41cd0
[ "$(sha256sum "$LOCK" | cut -d' ' -f1)" = "$LOCK_SHA256" ] || die "fedora-pkgset.lock.tsv sha256 $(sha256sum "$LOCK" | cut -d' ' -f1) != LOCK_SHA256 $LOCK_SHA256"
# lock rows for this arch: target-arch name evr arch sha256 src/ver/rel
rows(){ awk -F'\t' -v a="$ARCH" '!/^#/ && $1==a' "$LOCK"; }
# rpm file name from the BINARY epoch:version-release (a subpackage can have its own version);
# the koji build directory is column 6 (srcname/version/release)
fname(){ awk -F'\t' '{e=$3; sub(/^[0-9]+:/,"",e); print $2"-"e"."$4".rpm"}'; }

case "${1:-}" in
image)
  # the SAME archive + digest as rpm-build.sh (the rpm wrap): one Fedora base for every Fedora stage
  RB=$HERE/rpm-build.sh
  grep -q "OCI_SHA=$OCI_SHA" "$RB" && grep -q "DIGEST=$DIGEST" "$RB" || die "OCI pin != packaging/ci/rpm-build.sh"
  command -v podman >/dev/null || die "no podman"
  if ! podman image exists "$IMG"; then
    C=${FEDORA_OCI_CACHE:-${RPM_OCI_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/fedora44}}; mkdir -p "$C"; A=$C/${OCI_URL##*/}
    [ -s "$A" ] && echo "$OCI_SHA  $A" | sha256sum -c --status - || curl -4 -fsSL -m 900 -o "$A" "$OCI_URL"
    echo "$OCI_SHA  $A" | sha256sum -c - >&2 || die "OCI archive sha256 mismatch"
    rm -rf "$C/oci" && mkdir "$C/oci" && tar xJf "$A" -C "$C/oci"
    ID=$(podman pull -q "oci:$C/oci") && podman tag "$ID" localhost/fedora:44
    podman image exists "$IMG" || die "loaded image does not carry $DIGEST"
  fi
  echo "$IMG" ;;
fetch)
  D=$(mkdir -p "${2:?dir}" && cd "$2" && pwd)
  n=$(rows | wc -l); [ "$n" -gt 0 ] || die "lock has no $ARCH rows"
  say "fetch $n rpms (lock $(sha256sum "$LOCK" | cut -c1-16)) -> $D"
  i=0
  while IFS=$'\t' read -r _ name evr ar sha path; do
    f=$(printf '%s\t%s\t%s\t%s\t%s\t%s\n' x "$name" "$evr" "$ar" "$sha" "$path" | fname); o=$D/$f; i=$((i+1))
    if [ -s "$o" ] && echo "$sha  $o" | sha256sum -c --status -; then continue; fi
    curl -4 -fsS --retry 3 -m 900 -o "$D/.u.rpm" "$KOJI/$path/$ar/$f" || die "$f: download $KOJI/$path/$ar/$f"
    curl -4 -fsS --retry 3 -m 120 -o "$D/.s.sig" "$KOJI/$path/data/sigcache/$KEYID/$ar/$f.sig" || die "$f: download sigcache"
    bash "$HERE/../../scripts/supply-chain/rpm-splice.sh" "$D/.u.rpm" "$D/.s.sig" "$o"
    rm -f "$D/.u.rpm" "$D/.s.sig"
    echo "$sha  $o" | sha256sum -c --status - || die "$f: sha256 $(sha256sum "$o" | cut -c1-64) != lock $sha"
    if [ $((i % 50)) = 0 ]; then say "  $i/$n"; fi
  done < <(rows)
  say "fetched + sha256-checked $n rpms" ;;
install)
  D=${2:?dir} O=${3:?pkgset out}
  [ -f /etc/fedora-release ] || die "install runs inside the Fedora container"
  say "$(cat /etc/fedora-release); lock $(sha256sum "$LOCK" | cut -d' ' -f1)"
  K=/etc/pki/rpm-gpg/RPM-GPG-KEY-fedora-44-primary
  rpm --import "$K"
  case " $(rpm -q gpg-pubkey --qf '%{VERSION} ') " in *" $KEYFPR "*) ;; *) die "imported key is not $KEYFPR";; esac
  files=() bad=0
  while IFS=$'\t' read -r _ name evr ar sha path; do
    f=$(printf '%s\t%s\t%s\t%s\t%s\t%s\n' x "$name" "$evr" "$ar" "$sha" "$path" | fname)
    echo "$sha  $D/$f" | sha256sum -c --status - || { say "sha256 mismatch: $f"; bad=1; continue; }
    k=$(rpm -K "$D/$f" 2>&1) || true
    case "$k" in *": digests signatures OK") ;; *) say "rpm -K: $k"; bad=1;; esac
    files+=("$D/$f")
  done < <(rows)
  [ $bad = 0 ] || die "lock verification failed"
  say "${#files[@]} rpms: sha256 == lock, rpm -K digests signatures OK (key $KEYFPR)"
  # no repo is ever configured: dnf is not used, rpm installs exactly these files
  rpm -U --replacepkgs --quiet "${files[@]}" || die "rpm -U of the pinned set failed"
  rows | awk -F'\t' '{print $2"\t"$3"\t"$4}' | LC_ALL=C sort > /tmp/want.tsv
  rpm -qa --qf '%{NAME}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}\t%{ARCH}\n' | grep -v '^gpg-pubkey	' | LC_ALL=C sort > /tmp/got.tsv
  if [ "$(cat /tmp/want.tsv)" != "$(cat /tmp/got.tsv)" ]; then
    diff /tmp/want.tsv /tmp/got.tsv >&2 || true; die "installed set != fedora-pkgset.lock.tsv ($ARCH)"
  fi
  say "installed set == lock: $(wc -l < /tmp/got.tsv) packages, nothing else"
  { printf '# fedora 44 pinned set (%s)  %s\n# package\tevr\tarch\trpm-sha256\n' "$(sed -n 1p "$LOCK" | sed 's/^# //')" "$IMG"
    rows | awk -F'\t' '{print $2"\t"$3"\t"$4"\t"$5}' | LC_ALL=C sort; } > "$O"
  say "pkgset $O sha256 $(sha256sum "$O" | cut -d' ' -f1)" ;;
*) echo "usage: fedora-env.sh image | fetch <dir> | install <dir> <pkgset-out>" >&2; exit 2 ;;
esac
