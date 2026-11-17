# shots/177-window-title-graph-tab

Screenshots for jpzk/opensidian#177 (the window title names a focused graph / local graph tab, like stock Obsidian 1.13.7).
Taken on the box by harness phase `cfttitle` (opensidian-harness phase/177-window-title-graph-tab 23c49642), vault with notes `note` -> `other`, 1100x700, no window manager.
The test display has no WM, so the X window title is not painted; the phase prints the real X window title (WM_NAME) in the strip under the window.

- before-8e9199d-cfttitle-global-graph.png: develop 8e9199d, global graph focused: title `vault - Opensidian` (name dropped)
- after-f8e984e-cfttitle-global-graph.png: fix/177 f8e984e, global graph focused: `Graph view - vault - Opensidian`, as stock
- before-8e9199d-cfttitle-local-graph.png: develop 8e9199d, local graph focused: `vault - Opensidian`
- after-f8e984e-cfttitle-local-graph.png: fix/177 f8e984e, local graph focused: `Graph of note - vault - Opensidian`, as stock
- before/after-*-cfttitle-final.png: note tab focused again at the end: `note - vault - Opensidian` on both (unchanged)

Stock 1.13.7 reference (document.title, recorded on the box, issue #177): `Graph view - vault - Obsidian 1.13.7`, `Graph of cfA - vault - Obsidian 1.13.7`.
