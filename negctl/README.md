# negctl — negative control for the F1 x R11.3 composition guard

The guard: scripts/smoke.sh phase_dloss lines 1500-1533 — a save that FAILS AFTER AN
EXTERNAL APPEND must stay [dirty:1], surface [saveerr:], leave the append (and not the
buffer) on disk, and a chmod u+w retry must land BOTH.

The control (negctl-comp.sh, driver copied from goal/dl-retarget/negctl-ui.sh, exits 9 if
a run comes out the wrong way round): revert ONE token on ui/main.js:3025 — t.base takes
the MERGED text instead of the DISK bytes — and the guard must FAIL; restore, it must PASS.

- comp-fail.log ....... reverted tree, rc=1, "left the tab CLEAN (base == buffer)".
  Note its census: [saveerr:A-LP] present, NO [dirty:] token — banner up, tab clean.
- comp-pass.log ....... restored tree, rc=0, all four F1xR11.3 lines green.
- run-first.log ....... the guard's first green run on the unmodified tree.
- negctl-driver-run.txt  full driver transcript incl. the reverting diff and the exit-9
  direction checks; last line "NEGCTL OK: fail then pass, direction checked".
- progress-dloss-guard.md  the run's progress log (source of truth lives at
  /workspace/goal/dloss-guard/progress.md).
