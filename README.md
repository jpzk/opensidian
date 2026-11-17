# #183 Live Preview entry drift at the end of a long note — screenshots

Box t1 phase `lpdrift` (harness phase/183-lpdrift 290c9b4a), vault note LP-Long, scrolled to the end (top line L3118), then 4 round trips source -> Live Preview.

- `start.png` — source mode, top line L3118 (identical on develop and the fix, sha e8513823).
- `before-develop-8e9199d.png` — develop 8e9199d after 4 trips: top line drifted to L3116 (~0.3 line per entry into LP).
- `after-fix-fe0c774.png` — fix/183-lpdrift fe0c774 after 4 trips: top line still L3118.

Stock Obsidian 1.13.7 not shown: on this note stock itself drifts 3..72 lines at the end, so it is no parity reference here; the fix targets the anchor invariant (top line kept).
