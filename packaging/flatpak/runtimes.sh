#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# FLATPAK BUILD ENVIRONMENT (#189 item 2): host tools and runtimes pinned by version / commit.
#   packaging/flatpak/runtimes.sh host      runner apt -> snapshot.ubuntu.com $SNAP only, install
#                                           flatpak/flatpak-builder/elfutils pkg=version (sudo)
#   packaging/flatpak/runtimes.sh remote    add flathub from a sha256-checked .flatpakrepo (GPG key)
#   packaging/flatpak/runtimes.sh log       pinned commit + date, and the newest flathub commits of
#                                           each ref (what a bump would pick; docs/supply-chain.md)
#   packaging/flatpak/runtimes.sh install   install each ref (--no-related), move it to the pinned
#                                           commit, verify the deployed commit and its age
# ARCH = FLATPAK_ARCH or uname -m (x86_64|aarch64). The runtime branch alone (//49) is NOT a pin:
# flathub moves it on every runtime update. Each ref is pinned by its full ostree commit; flatpak
# checks the commit's GPG signature against the pinned flathub key while pulling it.
# The flathub submission of dev.koto.opensidian must build against the same commits.
set -euo pipefail
say(){ echo "[$(date +%H:%M:%S)] [flatpak-env] $*"; }
die(){ say "FAIL: $*"; exit 1; }
ARCH=${FLATPAK_ARCH:-$(uname -m)}
SNAP=20261004T000000Z        # = packaging/ci/appimage-build.sh (resolute; the runner is ubuntu-26.04)
HOST_PKGS="flatpak=1.16.6-1 flatpak-builder=1.4.8-1 elfutils=0.194-4"   # elfutils = eu-strip
FLATHUB_REPO_URL=https://dl.flathub.org/repo/flathub.flatpakrepo
FLATHUB_REPO_SHA256=3371dd250e61d9e1633630073fefda153cd4426f72f4afa0c3373ae2e8fea03a
MIN_AGE_DAYS=42              # the 6-week rule, checked against the commit's own date
REF_PLATFORM=runtime/org.gnome.Platform/$ARCH/49
REF_SDK=runtime/org.gnome.Sdk/$ARCH/49
REF_RUST=runtime/org.freedesktop.Sdk.Extension.rust-stable/$ARCH/25.08
case "$ARCH" in
  x86_64)
    COMMIT_PLATFORM=0000000000000000000000000000000000000000000000000000000000000000
    COMMIT_SDK=0000000000000000000000000000000000000000000000000000000000000000
    COMMIT_RUST=0000000000000000000000000000000000000000000000000000000000000000 ;;
  aarch64)
    COMMIT_PLATFORM=0000000000000000000000000000000000000000000000000000000000000000
    COMMIT_SDK=0000000000000000000000000000000000000000000000000000000000000000
    COMMIT_RUST=0000000000000000000000000000000000000000000000000000000000000000 ;;
  *) die "unsupported flatpak arch $ARCH (x86_64|aarch64)" ;;
esac
FP="flatpak --user"

# commit_date REF COMMIT -> "YYYY-MM-DD HH:MM:SS +0000" of that commit on flathub ("" if unknown)
commit_date() { $FP remote-info --arch="$ARCH" --commit="$2" flathub "$1" 2>/dev/null | sed -n 's/^ *Date: //p' | head -1; }
check_age() {  # REF COMMIT
  local d; d=$(commit_date "$1" "$2"); [ -n "$d" ] || die "$1: commit $2 is not on flathub"
  local age=$(( ($(date -u +%s) - $(date -u -d "$d" +%s)) / 86400 ))
  [ "$age" -ge "$MIN_AGE_DAYS" ] || die "$1: commit $2 is $age days old ($d), the rule is >= $MIN_AGE_DAYS"
  say "$1 commit $2 date $d age ${age}d"
}

case "${1:-}" in
host)
  say "runner apt -> snapshot.ubuntu.com $SNAP only"
  sudo mkdir -p /etc/apt/sources.list.d.runner
  sudo find /etc/apt/sources.list.d -mindepth 1 -maxdepth 1 -exec mv -t /etc/apt/sources.list.d.runner {} +
  [ ! -e /etc/apt/sources.list ] || sudo mv /etc/apt/sources.list /etc/apt/sources.list.d.runner/
  printf 'Types: deb\nURIs: https://snapshot.ubuntu.com/ubuntu/%s/\nSuites: resolute resolute-updates resolute-security\nComponents: main universe\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\nCheck-Valid-Until: no\n' "$SNAP" \
    | sudo tee /etc/apt/sources.list.d/snapshot.sources
  L=$(mktemp)
  sudo apt-get -q update > "$L" 2>&1 || { cat "$L"; die "apt update"; }
  cat "$L"
  if grep -E "^(Err:|W:|E:|Error:|Warning:)" "$L"; then die "apt update against the snapshot was not clean"; fi
  sudo apt-get install -y -q --no-install-recommends $HOST_PKGS
  dpkg-query -W flatpak flatpak-builder elfutils
  for p in $HOST_PKGS; do [ "$(dpkg-query -W -f='${Version}' "${p%%=*}")" = "${p#*=}" ] || die "${p%%=*} is not ${p#*=}"; done
  say "host tools pinned: $HOST_PKGS" ;;
remote)
  F=$(mktemp)
  curl -4 -fsSL -m 60 -o "$F" "$FLATHUB_REPO_URL"
  echo "$FLATHUB_REPO_SHA256  $F" | sha256sum -c - || die "flathub.flatpakrepo sha256 mismatch (GPG key changed?)"
  $FP remote-add --if-not-exists flathub "$F"
  $FP remotes --columns=name,url,gpg-verify
  say "flathub remote from $FLATHUB_REPO_URL sha256 $FLATHUB_REPO_SHA256" ;;
log)
  for r in PLATFORM SDK RUST; do eval "ref=\$REF_$r c=\$COMMIT_$r"
    say "$ref pinned $c ($(commit_date "$ref" "$c" || true))"
    $FP remote-info --log --arch="$ARCH" flathub "$ref" | grep -E '^ *(Commit|Date|Subject):' | head -36 || true
  done ;;
install)
  check_age "$REF_PLATFORM" "$COMMIT_PLATFORM"
  check_age "$REF_SDK" "$COMMIT_SDK"
  check_age "$REF_RUST" "$COMMIT_RUST"
  $FP install -y --noninteractive --no-related flathub "$REF_PLATFORM"
  $FP update -y --noninteractive --no-related --commit="$COMMIT_PLATFORM" "$REF_PLATFORM"
  $FP install -y --noninteractive --no-related flathub "$REF_SDK"
  $FP update -y --noninteractive --no-related --commit="$COMMIT_SDK" "$REF_SDK"
  $FP install -y --noninteractive --no-related flathub "$REF_RUST"
  $FP update -y --noninteractive --no-related --commit="$COMMIT_RUST" "$REF_RUST"
  for r in PLATFORM SDK RUST; do eval "ref=\$REF_$r c=\$COMMIT_$r"
    n3=${ref#runtime/}; got=$($FP info --show-commit --arch="$ARCH" "${n3%%/*}" "${n3##*/}")
    [ "$got" = "$c" ] || die "$ref deployed $got, pinned $c"
    say "PINNED $ref $got"
  done
  $FP list --runtime --columns=ref,active
  n=$($FP list --runtime --columns=ref | grep -c '/' || true)
  [ "$n" = 3 ] || die "$n runtimes installed, expected exactly the 3 pinned refs" ;;
*) echo "usage: $0 host|remote|log|install" >&2; exit 2 ;;
esac
