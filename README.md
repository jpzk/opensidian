# shots/159-inline-title-blur-rename

Screenshots for jpzk/opensidian#159 (leaving the inline title by a click into the body commits the rename, like stock Obsidian 1.13.7).
Taken on the box by harness phase `cfxtitle` (opensidian-harness phase/159-inline-title-blur-rename 22999017), vault `cfxtitle-vault`, note `CX Target`, 1100x700, no window manager.
The phase types ` X` at the end of the inline title, then clicks into the body (blur).

- before-8e9199d-cfxtitle-open.png / after-f367d55-cfxtitle-open.png: the note before the edit (same on both)
- before-8e9199d-cfxtitle-after-blur.png: develop 8e9199d, after the blur: title, tab and explorer still `CX Target` (the edit was reverted)
- after-f367d55-cfxtitle-after-blur.png: fix/159 f367d55, after the blur: title, tab and explorer read `CX Target X` (renamed), as stock

Stock 1.13.7 reference (behaviour, issue #159): a click into the body renames the note; only Escape reverts.
