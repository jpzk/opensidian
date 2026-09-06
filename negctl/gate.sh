#!/bin/sh
# dloss-guard final gate: cargo test + FULL smoke (once) + check-loss.
# Every rc from a redirect, never from a pipeline (12:22 lesson).
set -u
cd /workspace/rustidian-wt/dloss-guard || exit 1
export CARGO_TARGET_DIR=/workspace/rustidian/src-tauri/target DISPLAY_NUM=:95 OUT=/tmp/rustidian-smokedlguard95
L=/workspace/goal/dloss-guard/negctl
ts() { date +%H:%M:%S; }
rc_all=0

echo "[$(ts)] 1/3 cargo test"
( cd src-tauri && cargo test ) > "$L/gate-cargo.log" 2>&1; rc=$?
echo "[$(ts)] cargo rc=$rc"; grep -E "^test result|error\[|^error" "$L/gate-cargo.log" | tail -5
[ "$rc" -eq 0 ] || rc_all=1

echo "[$(ts)] 2/3 FULL smoke (13 phases, single run) - peek $L/gate-full.log"
sh scripts/smoke.sh full > "$L/gate-full.log" 2>&1; rc=$?
echo "[$(ts)] full smoke rc=$rc"; tail -6 "$L/gate-full.log"
[ "$rc" -eq 0 ] || rc_all=1

echo "[$(ts)] 3/3 check-loss.sh"
sh /workspace/notes/check-loss.sh > "$L/gate-checkloss.log" 2>&1; rc=$?
echo "[$(ts)] check-loss rc=$rc"; tail -3 "$L/gate-checkloss.log"
[ "$rc" -eq 0 ] || rc_all=1

echo "[$(ts)] GATE rc_all=$rc_all"
exit $rc_all
