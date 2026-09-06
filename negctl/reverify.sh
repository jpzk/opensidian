#!/bin/sh
# dloss-guard re-verification at the MERGED head (origin/main merged in at b3fb573).
# Every rc from a redirect, never from a pipeline.
set -u
cd /workspace/rustidian-wt/dloss-guard || exit 1
export CARGO_TARGET_DIR=/workspace/rustidian/src-tauri/target DISPLAY_NUM=:95 OUT=/tmp/rustidian-smokedlguard95
L=/workspace/goal/dloss-guard/negctl
ts() { date +%H:%M:%S; }
rc_all=0

echo "[$(ts)] 1/5 fast dloss"
sh scripts/smoke.sh fast dloss > "$L/rv-fast-dloss.log" 2>&1; rc=$?
echo "[$(ts)] fast dloss rc=$rc"; grep -E "F1xR11.3|PASS|FAIL" "$L/rv-fast-dloss.log" | tail -5
[ "$rc" -eq 0 ] || rc_all=1

echo "[$(ts)] 2/5 cargo test"
( cd src-tauri && cargo test ) > "$L/rv-cargo.log" 2>&1; rc=$?
echo "[$(ts)] cargo rc=$rc"; grep -E "^test result|^error" "$L/rv-cargo.log" | tail -4
[ "$rc" -eq 0 ] || rc_all=1

echo "[$(ts)] 3/5 FULL smoke (once, at the merged head)"
sh scripts/smoke.sh full > "$L/rv-full.log" 2>&1; rc=$?
echo "[$(ts)] full smoke rc=$rc"; tail -4 "$L/rv-full.log"
[ "$rc" -eq 0 ] || rc_all=1

echo "[$(ts)] 4/5 check-loss"
sh /workspace/notes/check-loss.sh > "$L/rv-checkloss.log" 2>&1; rc=$?
echo "[$(ts)] check-loss rc=$rc"; tail -3 "$L/rv-checkloss.log"
[ "$rc" -eq 0 ] || rc_all=1

echo "[$(ts)] 5/5 git ancestry / clean / pushed"
git merge-base --is-ancestor origin/main goal/dloss-guard; rc=$?
echo "[$(ts)] ancestry rc=$rc"; [ "$rc" -eq 0 ] || rc_all=1
git status --porcelain > "$L/rv-status.txt" 2>&1
echo "[$(ts)] status lines: $(wc -l < "$L/rv-status.txt")"
[ -s "$L/rv-status.txt" ] && rc_all=1

echo "[$(ts)] REVERIFY rc_all=$rc_all"
exit $rc_all
