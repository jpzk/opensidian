# cve-bump: mermaid 11.17.2 vs 12.3.0 renders (box, harness phase `mermaid`)

- before-*.png = develop 323838c (vendored mermaid 11.17.2), box t1 20261119T211638Z, host hz-176764494
- after-*.png  = fix/mermaid-12-katex 9989e1d (vendored mermaid 12.3.0, layout pinned dagre), box t1 20261119T204959Z, same host

Per pair:
- *-guard.png   : M1 guarded reading view (stock guard + Allow button, Mermaid not loaded) - byte-identical
- *-reading.png : M2 allowed reading view, 7 diagram types (flowchart, sequence, class, state, gantt, er, pie)
- *-lp.png      : L1 Live Preview render
- *-hostile.png : H1 hostile note (pwn=0 net=0 sl=strict, oversized diagram errors inline)

Visible 12.x differences: flowchart decision diamond sized to its label, sequence actor boxes get the 12.x drop shadow. Same layout engine (dagre), same theme colours, same diagram kinds (oracle kinds= identical).
