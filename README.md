# #185 MathJax — screenshots

Product PR #233 head 056c6b4 (fix/185-mathjax) vs develop 8e9199d; harness phase mathjax (phase/185-mathjax ecb4a02b). Box hz-176635734, display leases, same fixture vault (MJ-Math = the stock m185 note, dark vault theme).

- before-develop-reading.png — develop 8e9199d, reading view: TeX source as plain text
- before-develop-livepreview.png — develop 8e9199d, Live Preview: TeX source as plain text
- after-head-reading.png — head 056c6b4, reading view: inline + block typeset by MathJax 3.2.2 (CHTML)
- after-head-livepreview.png — head 056c6b4, Live Preview: inline + block math widgets
- after-head-source.png — head 056c6b4, source mode: raw TeX, no widgets
- after-head-hostile.png — head 056c6b4, MJ-Hostile: \href/\cssId/\class/\data/<img onerror>/\require are inert (no link/id/class/data-*/img; \data and \require render as TeX errors). The \style{position:fixed;...background:red;100vw x 100vh} formula loses position:fixed, so the red box stays in the note's flow inside the pane and does not cover the window (0 red px in the window's top-left corner).
- stock-1.13.7-reading.png / stock-1.13.7-livepreview.png — stock Obsidian 1.13.7 (black box, default light theme, note m185) for comparison
