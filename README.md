# shots/179-tabmenu-mode-toggle

Screenshots for jpzk/opensidian#179 (tab menu "Reading view" / "Source mode" rows are toggles, like stock Obsidian 1.13.7).
Taken on the box by harness phase `cftmode` (opensidian-harness phase/179-tabmenu-mode-toggle 15b835b5), vault `cftmode-vault`, note `Tm` = `some **bold** text`, 1100x700, no window manager.
Each shot is the editor right after the tab menu's CHECKED row was clicked a second time (fresh right-click each time), starting in Live Preview.

- before-8e9199d-cftmode-source-x2.png: develop 8e9199d, Source mode clicked twice: still Source mode (`**bold**` markers shown)
- after-f68c8fb-cftmode-source-x2.png: fix/179 f68c8fb, Source mode clicked twice: back in Live Preview (bold rendered), as stock
- before-8e9199d-cftmode-reading-x2.png: develop 8e9199d, Reading view clicked twice: still Reading view
- after-f68c8fb-cftmode-reading-x2.png: fix/179 f68c8fb, Reading view clicked twice: back to editing (Live Preview), as stock

Stock 1.13.7 reference (behaviour, recorded by the harness on the box): the checked Source mode goes back to Live Preview, the checked Reading view goes back to editing.
