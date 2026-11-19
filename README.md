# shots: #239 Excalidraw drawing rename (tab double-click + view-header title)

Harness phase `excren` on the box (Linux, Xvfb display lease).

- `dev-b6a64f1-*` = develop b6a64f1 (before): tab double-click and header title do nothing for a drawing; header shows no title.
- `head-4b0a61e-*` = fix/239-excal-rename 4b0a61e (after): tab double-click opens the rename editor, header title shows the basename and is click-to-rename; file renamed on disk, tab + explorer updated.

Steps: 0 setup, 1 tab editor, 2 tab renamed, 3 header editor, 4 header renamed.
