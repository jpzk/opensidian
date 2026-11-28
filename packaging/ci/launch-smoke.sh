#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# launch-smoke.sh <label> <timeout-s> <AppImage | binary> [runner-prefix...]
# Headless launch smoke (release.yml job smoke, driven by smoke-run.sh): start the app on the X
# display $DISPLAY (an Xvfb that smoke-run.sh runs in the pinned Fedora 44 env) and PASS iff a
# top-level X window titled "Opensidian" appears and the process is still alive 5 s later.
# [runner-prefix...] runs the app somewhere else (`podman exec <ctr>`); empty = on this host.
# $XWININFO = how to call xwininfo (default `xwininfo`; smoke-run.sh: `podman exec <xctr> xwininfo`).
# AppImages run with APPIMAGE_EXTRACT_AND_RUN=1 (no FUSE in containers / on fresh runners).
# Output: timestamped log; last line "RESULT: PASS" | "RESULT: FAIL <why>" (rc 0 / 1).
set -uo pipefail
LBL=${1:?usage: $0 <label> <timeout-s> <app> [runner...]} TO=${2:?timeout} APP=${3:?app}; shift 3
RUN=("$@"); read -ra XW <<< "${XWININFO:-xwininfo}"; D=${DISPLAY:?DISPLAY}
say() { echo "[$(date -u +%H:%M:%S)] smoke[$LBL]: $*"; }
L=$(mktemp); AP=
fail() { say "app output tail:"; tail -n 40 "$L" 2>/dev/null; echo "RESULT: FAIL $*"; exit 1; }
trap '[ -z "$AP" ] || { kill $AP 2>/dev/null; sleep 1; kill -9 $AP 2>/dev/null; }; rm -f "$L"' EXIT
"${RUN[@]}" sh -c 'grep -E "^PRETTY_NAME=" /etc/os-release; echo "glibc: $(ldd --version 2>/dev/null | head -1)"; sha256sum "$0"' "$APP" \
  || fail "app not reachable in the target env: $APP"
"${XW[@]}" -display "$D" -root >/dev/null 2>&1 || fail "no X server on $D"
# software rendering: no GPU on a runner; dbus may be absent (the app must still show its window)
"${RUN[@]}" sh -c 'h=$(mktemp -d); mkdir -m 700 "$h/run"; export HOME="$h" XDG_RUNTIME_DIR="$h/run"
  exec env DISPLAY="$0" APPIMAGE_EXTRACT_AND_RUN=1 LIBGL_ALWAYS_SOFTWARE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1 NO_AT_BRIDGE=1 "$1"' \
  "$D" "$APP" >"$L" 2>&1 & AP=$!
say "started $APP (pid $AP) on $D, waiting up to ${TO}s for the window"
for i in $(seq "$TO"); do
  kill -0 "$AP" 2>/dev/null || { wait "$AP"; rc=$?; AP=; fail "app exited (rc $rc) after ${i}s"; }
  if "${XW[@]}" -display "$D" -root -tree 2>/dev/null | grep -q '"Opensidian"'; then
    sleep 5; kill -0 "$AP" 2>/dev/null || { AP=; fail "app died 5s after mapping its window"; }
    say "window after ${i}s:"; "${XW[@]}" -display "$D" -root -tree | grep '"Opensidian"' | head -3
    echo "RESULT: PASS"; exit 0
  fi
  sleep 1
done
fail "no Opensidian window within ${TO}s"
