# shots/t2r-mswdom (T2 096 red phases mswdom + themevars)

- `stock-msw-reading.png`: stock Obsidian 1.13.7, black-box, MS-Typical fixture in reading view. The rust fence renders Prism-highlighted: `pre.language-rust > code.language-rust.is-loaded > span.token` (56 token spans, keyword colour rgb(213, 57, 132)). Observed through the DOM/computed styles over devtools only. No app source was read.
- What it shows: Opensidian #231 (1e949e1, Prism highlighting) produces the same reading-view structure. The harness `mswdom` `[pvh:]` pin was recorded before #231 and is stale, so it gets re-recorded in harness `phase/t2r-mswdom`.
- themevars has no GUI change. Its re-pin covers added nodes only (scripts and Prism spans), and the computed style of every existing node is unchanged.
