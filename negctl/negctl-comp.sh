#!/bin/sh
# dloss-guard negative control: the F1 x R11.3 COMPOSITION fix (ui/main.js:3025).
# Revert ONE thing — base takes the MERGED text instead of the DISK bytes — and the new
# phase_dloss assertion must FAIL naming the clean-tab symptom; restore -> must PASS.
# Driver copied from /workspace/goal/dl-retarget/negctl-ui.sh (exits 9 if a run comes out
# the wrong way round, so direction is CHECKED, not assumed). rc from a redirect, never
# from a pipeline.
set -u
cd /workspace/rustidian-wt/dloss-guard || exit 1
export CARGO_TARGET_DIR=/workspace/rustidian/src-tauri/target DISPLAY_NUM=:95 OUT=/tmp/rustidian-smokedlguard95
L=/workspace/goal/dloss-guard/negctl
mkdir -p "$L"
ts() { date +%H:%M:%S; }
run() {  # run <logname> <expect: fail|pass> [required string on failure]
  echo "[$(ts)] === sh scripts/smoke.sh fast dloss -> $1 (expect $2) ==="
  sh scripts/smoke.sh fast dloss > "$L/$1.log" 2>&1; rc=$?
  echo "[$(ts)] rc=$rc  (tail)"; tail -n 8 "$L/$1.log"
  case "$2" in
    fail) [ "$rc" -ne 0 ] || { echo "NEGCTL BROKEN: expected FAIL, got rc=0 ($1) — the scenario does not exercise the composition"; exit 9; }
          [ "$rc" -eq 1 ] || { echo "NEGCTL BROKEN: rc=$rc is an ENVIRONMENT failure, not an assertion ($1)"; exit 9; }
          if [ $# -ge 3 ]; then grep -q "$3" "$L/$1.log" || { echo "NEGCTL BROKEN: failed for the WRONG reason, no '$3' ($1)"; exit 9; }; fi ;;
    pass) [ "$rc" -eq 0 ] || { echo "NEGCTL BROKEN: expected PASS, got rc=$rc ($1)"; exit 9; } ;;
  esac
}
git status --porcelain | grep -q . && { echo "tree dirty, abort"; exit 1; }

echo "[$(ts)] --- F1xR11.3: revert ui/main.js:3025 to set t.base to the MERGED text ---"
sed -i 's|buf = merged; t.base = disk; }|buf = merged; t.base = merged; }  // NEGCTL: composition fix reverted (base = the text we are ABOUT to write)|' ui/main.js
git diff -U0 ui/main.js
git diff --quiet ui/main.js && { echo "NEGCTL BROKEN: the sed matched nothing, nothing was reverted"; exit 9; }
run comp-fail fail "left the tab CLEAN"
git checkout ui/main.js
run comp-pass pass

echo "[$(ts)] --- tree restored ---"; git status --porcelain; echo "[$(ts)] NEGCTL OK: fail then pass, direction checked"
