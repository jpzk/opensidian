# shots/163-hotkey-plus-space

Screenshots for jpzk/opensidian#163 (a '+' key hotkey survives a restart, a Space hotkey is labelled, like stock Obsidian 1.13.7).
Taken on the box by harness phase `cfe2hkplus` (opensidian-harness phase/163-hotkey-plus-space 4059eaf2), Settings > Hotkeys filtered to "Toggle comment", 1100x700, no window manager.

- before-8e9199d-cfhk-space.png: develop 8e9199d: the Ctrl+Space binding is labelled `Ctrl +` (blank key)
- after-a1b80a5-cfhk-space.png: fix/163 a1b80a5: labelled `Ctrl + Space`, as stock

Stock 1.13.7 reference: docs/cf-edit2/STOCK-B4.txt on the harness branch (Ctrl+Shift+= saved as {"modifiers":["Mod","Shift"],"key":"="}, fires after reload; label `Ctrl + Space`).
