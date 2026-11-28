#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# smoke-run.sh <target> <portable.AppImage> <binary-sha256> [<pkg.deb|pkg.rpm>]   (release.yml job smoke)
# Paths are relative to the repo root. Writes dist/smoke-portable.log (+ dist/smoke-installed.log).
# Nothing here comes from a floating repo except what the deb install itself is about:
#   X server  = Xvfb + xwininfo of the PINNED Fedora 44 env (fedora-env.sh: digest-pinned OCI +
#               fedora-pkgset.lock.tsv, sha256 + rpm -K), in a --network=host container: the X socket
#               (abstract namespace + the shared /tmp/.X11-unix) is reachable from the host and from
#               the target containers.
#   ubuntu-*  = the runner host itself; ubuntu-26.04 also `apt-get install ./<deb>`: that is the test
#               (do the deb's dependency names resolve on stock Ubuntu 26.04?), test-only, never shipped.
#   fedora-44 = the pinned env; `rpm -i <rpm>` WITHOUT dnf/--nodeps: every Requires of the rpm must be
#               satisfied by the pinned Fedora 44 set.
#   debian-12 = debian:bookworm-20261005 (2026-10-06) by multi-arch index digest, portable only.
# rc 0 = every launch PASS (and installed binary sha256 == the AppImage binary).
set -euo pipefail
TGT=${1:?target} P=${2:?portable} B=${3:?binary sha256} PKG=${4:-}
ROOT=$(cd "$(dirname "$0")/../.." && pwd); cd "$ROOT"
DEBIAN12=docker.io/library/debian@sha256:2c037a04925515fdd6ea85ea14a682d0e79931f5e9f5d07b6dbfc6ba12f9e858
say() { echo "[$(date -u +%H:%M:%S)] smoke-run[$TGT]: $*"; }
die() { say "FAIL: $*"; exit 1; }
F44=$(bash packaging/ci/fedora-env.sh image) || die "fedora image"
bash packaging/ci/fedora-env.sh fetch "$ROOT/dist/f44-rpms"
X11=/tmp/.X11-unix; [ -d "$X11" ] || sudo install -d -m 1777 "$X11"
XC=smoke-x-$$ TC=
trap 'podman rm -f -t 1 "$XC" ${TC:+"$TC"} >/dev/null 2>&1 || true' EXIT
podman run -d --name "$XC" --network=host -v "$X11:$X11" -v "$ROOT:/w" -w /w "$F44" sleep infinity >/dev/null
podman exec "$XC" bash packaging/ci/fedora-env.sh install /w/dist/f44-rpms /tmp/pkgset.tsv
podman exec "$XC" rpm -q xorg-x11-server-Xvfb xwininfo
podman exec -d "$XC" Xvfb :99 -screen 0 1280x800x24 -nolisten tcp
export DISPLAY=:99 XWININFO="podman exec $XC xwininfo"
for _ in $(seq 50); do $XWININFO -display :99 -root >/dev/null 2>&1 && break; sleep 0.2; done
$XWININFO -display :99 -root >/dev/null 2>&1 || die "Xvfb did not start"
say "Xvfb :99 up in $XC ($F44)"
RUN=() APPDIR=$ROOT
case "$TGT" in
  ubuntu-*)  ;;
  fedora-44) RUN=(podman exec "$XC") APPDIR=/w ;;
  debian-12) TC=smoke-t-$$
             podman run -d --name "$TC" --network=host -v "$X11:$X11" -v "$ROOT:/w" -w /w "$DEBIAN12" sleep infinity >/dev/null
             RUN=(podman exec "$TC") APPDIR=/w ;;
  *) die "unknown target $TGT" ;;
esac
rc=0
bash packaging/ci/launch-smoke.sh "$TGT portable" 120 "$APPDIR/$P" "${RUN[@]}" | tee dist/smoke-portable.log || rc=1
if [ -n "$PKG" ]; then
  case "$TGT" in
    ubuntu-26.04) sudo apt-get update -q && sudo apt-get install -y --no-install-recommends "./$PKG" ;;
    fedora-44)    podman exec "$XC" rpm -i --test "/w/$PKG" && podman exec "$XC" rpm -i "/w/$PKG" ;;
    *) die "no package install for $TGT" ;;
  esac
  got=$("${RUN[@]}" sha256sum /usr/bin/opensidian | cut -d' ' -f1); say "installed /usr/bin/opensidian sha256 $got"
  [ "$got" = "$B" ] || { say "installed binary != AppImage binary ($B)"; rc=1; }
  bash packaging/ci/launch-smoke.sh "$TGT installed" 120 /usr/bin/opensidian "${RUN[@]}" | tee dist/smoke-installed.log || rc=1
fi
exit $rc
