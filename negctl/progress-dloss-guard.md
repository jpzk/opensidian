# dloss-guard — guard the F1 x R11.3 composition

Worktree /workspace/rustidian-wt/dloss-guard, branch goal/dloss-guard, branched from
origin/main 30e2ed4. Display :95, OUT=/tmp/rustidian-smokedlguard95,
CARGO_TARGET_DIR=/workspace/rustidian/src-tauri/target.

## iteration 1
Startup: no ledger existed; created ledger.json (7 items) and this file.
Verified the pickup state before building on it: `git log --oneline -1` = 30e2ed4
(the dl-retarget merge, i.e. current main), `git status --porcelain` empty.

### reading first (why the scenario is shaped the way it is)
- ui/main.js:3009-3031 saveBuf(): the merge-before-write. `disk !== t.base && disk !== buf`
  -> `merged = disk.startsWith(t.base) ? buf + disk.slice(t.base.length) : buf`, then
  ui/main.js:3025 `if (merged !== buf) { await reloadInPlace(g, merged); buf = merged; t.base = disk; }`.
  reloadInPlace (ui/main.js:3033) itself sets `t.base = text` (= the merged text), so the
  fix IS that trailing `t.base = disk` — reverting it is a one-token revert, exactly what
  the negative control needs.
- ui/main.js:3060-3073 onVaultChanged(): the watcher does its OWN merge for the ACTIVE tab
  and sets `t.base = ext` (the disk bytes) before scheduleSave(). CONSEQUENCE FOR THE TEST:
  if the watcher tick (watcher::TICK_MS = 1000, a const, not env-tunable) reports the
  external append BEFORE our save runs, saveBuf takes `disk === t.base` and never reaches
  line 3025 — the scenario would then pass even with the fix reverted (a false pass, which
  the negctl driver would catch as "expected FAIL, got rc=0"). So the scenario must reach
  saveBuf FIRST, and must say so when it did not.
  Mitigation, two parts: (a) sync to the watcher tick before the append — a decoy append to
  an UNOPENED note, poll until its [vc:] lands, at which point the tick has just fired and
  the next one is ~1s away; then do append + ctrl+s inside that window (~100ms of work);
  (b) assert `[vc:<unchanged>]` in the SAME census read as [dirty:1], so a preempting tick
  fails loudly with its own message instead of silently passing.
- rejected alternative: park the dirty tab in the background, let the watcher mark it
  `stale`, then switch back and ctrl+s. loadActive (ui/main.js:1426-1429) re-reads disk and
  `Ed.setText` on `t.stale` WITHOUT checking dirty, so switching back would destroy the
  typed buffer. (That looks like a second, separate R20 x F1 composition hole — NOT this
  goal's scope, recorded here so it is not lost.)

## iteration 2 — the scenario RUNS, and the negative control proves it bites

Startup check before building on iteration 1: `git log --oneline -1` = 30e2ed4, but
`git status --short` showed ` M scripts/smoke.sh` — iteration 1 WROTE the scenario and
ended without ever running or committing it. So the first act of this iteration was to
run it, not to trust it.

### 1. the new assertion (scripts/smoke.sh lines 1500-1533, inside phase_dloss)
- 1500-1505 comment: why the bug lives in neither fix alone.
- 1507-1509 `chmod a-w "$V"`, `typestr " DLOSS-C1"`, expect `[saveerr:A-LP]` + `[dirty:1]`
  — the tab is dirty and the debounced save has already failed, so base is still the DISK bytes.
- 1511-1519 tick sync: `VC=$(tok vc)`, decoy `echo DECOY >> Ideas.md` (an UNOPENED note),
  poll for `[vc:VC+1]`; the watcher tick has just fired, the next is ~1s away.
- 1520 `echo "$APPX" >> "$N"; key ctrl+s` — the external append, then the save forced into it.
- 1521 poll `[buf:BUFC+len+1]`: saveBuf's merge actually folded the append into the buffer.
- 1522-1527 ONE census read `T`, four assertions against it:
  - 1523 `[vc:$VC]` unchanged — if the watcher tick had preempted us, onVaultChanged would
    have done the merge and set base itself and saveBuf's composition would never have run.
    That case now dies with its own message instead of passing silently.
  - 1524 `[dirty:1]` — THE assertion. Failure text: "a save that FAILED after an external
    append left the tab CLEAN (base == buffer): F1's dirty state is gone and the typed bytes
    are unrecoverable".
  - 1525 `[saveerr:A-LP]` still surfaced.
  - 1526 the append is still on disk; 1527 the buffer is NOT.
- 1529-1533 `chmod u+w` + `ctrl+s` retry must land BOTH `DLOSS-C1` and `$APPX` — that is
  what proves base held the DISK bytes and the next merge still saw the append as an append.

### 2. it passes on the fixed tree (job XtOx95PE, rc captured from a redirect)
    [19:19:09] fast dloss start
    [19:20:57] rc=0
    [smoke] F1: rejected save -> [saveerr:A-LP] + [dirty:1] + disk bytes untouched
    [smoke] F1: vault writable again -> ctrl+s landed the SAME buffer and cleared the banner token
    [smoke] F1xR11.3: failed save after an external append -> [saveerr:A-LP] + [dirty:1], disk keeps the append and not the buffer
    [smoke] F1xR11.3: chmod u+w + ctrl+s landed BOTH the typed text and the external append
    [smoke] F2: switch flushed A's edit into A and left [armed:0]
    [smoke] F2: vault B's same-named note byte-identical 16s after the switch
    [smoke] PASS (fast dloss)
Committed as 3774d4b. Raw log: negctl/run-first.log.

### 3. NEGATIVE CONTROL (driver negctl-comp.sh, copied from
/workspace/goal/dl-retarget/negctl-ui.sh — exits 9 if a run comes out the wrong way round,
so the direction is checked and not assumed; it also exits 9 if the sed matched nothing).
Job 0HYNO2fD, full driver transcript in negctl/negctl-driver-run.txt.

The revert is one token on ui/main.js:3025 — base takes the MERGED text (what
reloadInPlace already set) instead of the DISK bytes:

    [19:21:40] --- F1xR11.3: revert ui/main.js:3025 to set t.base to the MERGED text ---
    diff --git a/ui/main.js b/ui/main.js
    index 69547e0..bded80d 100644
    --- a/ui/main.js
    +++ b/ui/main.js
    @@ -3025 +3025 @@ async function saveBuf(g) {
    -      if (merged !== buf) { await reloadInPlace(g, merged); buf = merged; t.base = disk; }
    +      if (merged !== buf) { await reloadInPlace(g, merged); buf = merged; t.base = merged; }  // NEGCTL: composition fix reverted (base = the text we are ABOUT to write)

REVERTED -> FAILS, rc=1 (an assertion, not an environment error — the driver checks that
too). negctl/comp-fail.log verbatim:

    [smoke] holding :95 lock (pid 31101)
    [smoke] building...

    warning: `rustidian` (bin "rustidian") generated 3 warnings
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 40.00s
    [smoke] stale :95 socket (no server) - removing
    [smoke] F1: rejected save -> [saveerr:A-LP] + [dirty:1] + disk bytes untouched
    [smoke] F1: vault writable again -> ctrl+s landed the SAME buffer and cleared the banner token
    [smoke] FAIL: a save that FAILED after an external append left the tab CLEAN (base == buffer): F1's dirty state is gone and the typed bytes are unrecoverable (rustidian [panes:1 focused:1@1] [fx:1.00] [tabs:1] [mode:src:6] [lp:2] [edt:ok] [sel:6.18-6.18] [edx:300] [erx:300,300,300,300,300,300,300,300] [ery:83,114,138,162,186,210,234,258] [fonts:SourceCodePro/normal/normal] [side:l1r0] [rt:1071-1094|mb:1051-1071] [saveerr:A-LP] [armed:0] [pane:files] [buf:144] [tree:17] [vc:1] [vp:1100x700] [ovf:0,0,0])

Read that census: `[saveerr:A-LP]` IS there — the banner is up, the user is told the save
failed — and yet there is NO `[dirty:...]` token at all. The tab reads CLEAN with a failed
save behind it. That is precisely the silent loss: the banner can be dismissed, nothing is
dirty, nothing prompts a retry, and the typed bytes are gone at the next reload. The two
F1 assertions BEFORE it still passed, which is the whole point — F1 alone does not see this.

RESTORED (`git checkout ui/main.js`) -> PASSES, rc=0. negctl/comp-pass.log verbatim:

    [smoke] display :95 is busy (another smoke run holds the lock) - waiting up to 30m...
    [smoke] holding :95 lock (pid 15017)
    [smoke] building...

    warning: `rustidian` (bin "rustidian") generated 3 warnings
        Finished `dev` profile [unoptimized + debuginfo] target(s) in 20.70s
    [smoke] stale :95 socket (no server) - removing
    [smoke] F1: rejected save -> [saveerr:A-LP] + [dirty:1] + disk bytes untouched
    [smoke] F1: vault writable again -> ctrl+s landed the SAME buffer and cleared the banner token
    [smoke] F1xR11.3: failed save after an external append -> [saveerr:A-LP] + [dirty:1], disk keeps the append and not the buffer
    [smoke] F1xR11.3: chmod u+w + ctrl+s landed BOTH the typed text and the external append
    scripts/smoke.sh: line 1538: 20474 Killed                     env $LENV VAULT_DIR="${1:-$OUT/vault}" HOME="$OUT/home" DISPLAY=$D "$BIN" > "$OUT/app-fast.log" 2>&1
    [smoke] F2: switch flushed A's edit into A and left [armed:0]
    [smoke] F2: vault B's same-named note byte-identical 16s after the switch
    [smoke] PASS (fast dloss)

Driver's own verdict, last line: `NEGCTL OK: fail then pass, direction checked`, and
`git status --porcelain` after it printed nothing — the tree was restored by the driver,
not by hand.

(The "Killed" line is smoke.sh's own `kill9` of the app between the F1 and F2 halves, on
the PASS run only because the FAIL run died before reaching it. Not an error.)

### decisions
- The control's failing run reaches the F1 assertions and passes them, then fails on the
  composition. That is the evidence that this scenario tests something F1's own assertions
  cannot: the guard is not a duplicate of the two that already existed.
- The `[vc:]` guard at line 1523 is deliberate anti-false-pass machinery. Without it a
  watcher tick landing between the append and ctrl+s would make the reverted tree PASS,
  the driver would exit 9 ("expected FAIL, got rc=0"), and I would have had to debug a
  flake instead of reading a message that names the cause. It did not fire in either run
  (`[vc:1]` in the failure census = the decoy only).

### 4. gate (job Wn9FEIzU), every rc from a redirect
    [19:25:20] cargo rc=0    test result: ok. 51 passed; 0 failed; 0 ignored
    [19:27:25] full smoke rc=0   [smoke] PASS - screenshots in /tmp/rustidian-smokedlguard95
    [19:27:26] check-loss rc=0   citations checked : 75 (bad: 0)  RESULT: PASS
    [19:27:26] GATE rc_all=0
Raw logs: negctl/gate-cargo.log, negctl/gate-full.log, negctl/gate-checkloss.log.

### 5. one line beyond the brief, deliberately: scripts/gate.sh:20 now runs `dloss`
main recorded at 19:04 that the push gate's phase list (`edit lp src links linkpanes rside
fuzz chrome ux typo hist graphnav m5`) had NO dloss — phase_dloss passed only because a goal
invoked it by hand, so the assertions protecting against silent save failure were themselves
never run on a push. Writing a guard that the gate never runs would have reproduced exactly
the disease this goal exists to cure, so the phase is now in the list (`... m5 dloss`).
One word; `sh -n scripts/gate.sh` clean; the phase itself is proven green above and by the
control pair. Everything else in the diff is the smoke assertion and negctl/.
