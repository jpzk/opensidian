#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# launch-smoke.sh <AppImage | binary> [timeout-s]
# Headless launch smoke (release.yml job smoke): start an Xvfb display, start the app on it and
# PASS iff a top-level X window titled "Opensidian" appears and the process is still alive.
# Needs Xvfb + xwininfo on PATH (the caller installs them: TEST-ONLY tools, never shipped).
# AppImages run with APPIMAGE_EXTRACT_AND_RUN=1 (no FUSE in containers / on fresh runners).
# Output: timestamped log; last line "RESULT: PASS" | "RESULT: FAIL <why>" (rc 0 / 1).
set -uo pipefail
APP=${1:?usage: $0 <AppImage|binary> [timeout-s]}; TO=${2:-90}
say() { echo "[$(date -u +%H:%M:%S)] smoke: $*"; }
fail() { say "stderr tail:"; tail -n 40 "$L" 2>/dev/null; echo "RESULT: FAIL $*"; exit 1; }
[ -x "$APP" ] || chmod +x "$APP" 2>/dev/null; [ -x "$APP" ] || fail "not executable: $APP"
command -v Xvfb >/dev/null && command -v xwininfo >/dev/null || fail "Xvfb/xwininfo missing"
H=$(mktemp -d); L=$H/app.log; export HOME=$H XDG_RUNTIME_DIR=$H/run; mkdir -m 700 "$XDG_RUNTIME_DIR"
cat /etc/os-release 2>/dev/null | grep -E '^PRETTY_NAME='; say "glibc $(ldd --version 2>/dev/null | head -1)"
say "app $APP sha256 $(sha256sum "$APP" | cut -d' ' -f1)"
D=:$((90 + RANDOM % 100)); Xvfb "$D" -screen 0 1280x800x24 -nolisten tcp >"$H/xvfb.log" 2>&1 & XP=$!
for _ in $(seq 50); do xwininfo -display "$D" -root >/dev/null 2>&1 && break; sleep 0.2; done
xwininfo -display "$D" -root >/dev/null 2>&1 || fail "Xvfb did not start"
trap 'kill ${AP:-} $XP 2>/dev/null; sleep 1; kill -9 ${AP:-} $XP 2>/dev/null; rm -rf "$H"' EXIT
# software rendering: no GPU on a runner; dbus may be absent (the app must still show its window)
DISPLAY=$D APPIMAGE_EXTRACT_AND_RUN=1 LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 \
  NO_AT_BRIDGE=1 "$APP" >"$L" 2>&1 & AP=$!
say "started pid $AP on $D, waiting up to ${TO}s for the window"
for i in $(seq "$TO"); do
  kill -0 "$AP" 2>/dev/null || fail "app exited (rc $(wait $AP; echo $?)) after ${i}s"
  if xwininfo -display "$D" -root -tree 2>/dev/null | grep -q '"Opensidian"'; then
    sleep 5; kill -0 "$AP" 2>/dev/null || fail "app died 5s after mapping its window"
    say "window after ${i}s:"; xwininfo -display "$D" -root -tree | grep '"Opensidian"' | head -3
    echo "RESULT: PASS"; exit 0
  fi
  sleep 1
done
fail "no Opensidian window within ${TO}s"
