# #172 graph: horizontal scroll zooms the graph

Same vault, window size and state. Each set: `before` (graph view opened), `after-6-horizontal` (6 horizontal-only wheel notches, deltaY == 0), `control-1-vertical` (1 vertical notch, should zoom).

- `dev-172-*.png`: develop `8e9199d`. The horizontal notches zoom the graph out (bug).
- `pr172-*.png`: PR jpzk/opensidian#199 head `22f9909222fcde10082eeeed2fccc6b7584b5fb3`. The horizontal notches leave the zoom unchanged (before == after, same bytes); the vertical notch still zooms.
- `stock-172-*.png`: stock Obsidian 1.13.7. The horizontal notches leave the zoom unchanged (before == after, same bytes); the vertical notch zooms.
