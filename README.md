# rustidian
Minimal **open-source** Obsidian clone. Not based on Electron or JS but compiled down to machine code: Rust + Tauri v2, no npm yay. It's **under heavy development right now**.

## This is an experiment 
I'm using a DIY computer use to let AI write requirements to copy the UI/UX of Obsidian and implement it. Imitation is the best form of flattery. Obsidian is great, but it sucks that one has to run proprietary closed source blobs just for editing markdown files. This is maxxing out my Fable plan to open source Obsidian UX. 

## Security is top priority
I'm running as many top frontier models to scan for vulnerabilities, but since this is open source and you can do this, please do. Don't trust, verify.

## Features
- markdown editor + live preview (pulldown-cmark, rendered in Rust) — live preview is the default mode
- [[wikilinks]] -> clickable links + local graph view (canvas force sim)
- `[[note#heading]]`, `[[note#^block]]`, `[[note|alias]]` links: alias text, scroll-to-target + flash, `[[note#` heading autocomplete, block ids (` ^id`); Backlinks pane with Unlinked mentions + Link button
- splits / tab groups with tab drag & drop
- full-text search, quick switcher, command palette, bookmarks
- #tags: inline pills + frontmatter `tags:`, Tags pane in the right sidebar (nested a/b, counts), `tag:foo` search filter
- safe renames (H1 or F2) that rewrite [[links]] across the vault
- external edits (R11): the vault is watched (1s tick, std-only); shell edits to an open note appear in place with the caret kept, unsaved local text is merged with external appends, external create/rename/delete update the explorer and close the affected tab
- source mode (R12): live preview with every marker revealed — grey `#`/`**`/`[[` markers in the text flow, headings keep their size, bold/italic/strike/highlight/code styled, links + `#tag` in accent, fences shaded with the fence lines visible, literal `- [ ]` tasks; same font and measure as live preview, one raw caret row, `[[` autocomplete
- hotkeys (R14): Settings (Ctrl+,) ▸ Hotkeys — every command from one registry (also feeds the palette) with the stock Obsidian defaults, fuzzy filter + All/Assigned/Assigned by me/Unassigned chips, click ⊕ to record a chord, ✕ to remove, ↺ restore default, duplicate chords flagged red with a `Conflicts N` chip; overrides persist in `~/.rustidian.json` `hotkeys` (stock shape, `[]` = removed default)
- vault picker with persistence (or env VAULT_DIR, default ./vault)
- graph (R16): stock-faithful force layout (d3 semantics, world-space, camera fit), WebGL renderer by default with a Canvas 2D fallback (`RUSTIDIAN_GRAPH_RENDERER=gl|2d`)
- typography (R15): the exact Obsidian 1.13.7 type metrics in reading, live preview and source mode — bundled Inter Variable 4.001 + Source Code Pro 2.030 (SIL OFL, sha256-pinned via `scripts/fetch-fonts.sh --verify`), stock font stacks, h1–h6 sizes/weights/line-heights, list/checkbox/code/blockquote geometry within 1px of stock
- hardening: vault paths canonicalised and confined to the vault root, system dirs refused, symlinks and >32 MiB files skipped, link scheme allowlist (http/https/mailto only; opened via `xdg-open`), in-app navigation locked to the app origin, Landlock self-sandbox on by default

## Screenshots


## install
Two AppImage flavors on the [latest release](https://github.com/jpzk/rustidian/releases):

**slim** (recommended, ~3MB) — just the binary; uses your system's webkit.
Needs `libwebkit2gtk-4.1` installed (`apt install libwebkit2gtk-4.1-0` /
`dnf install webkit2gtk4.1`) — the AppImage tells you if it's missing:

    curl -LO https://github.com/jpzk/rustidian/releases/download/v0.4/rustidian-0.4-x86_64-slim.AppImage
    chmod +x rustidian-0.4-x86_64-slim.AppImage
    ./rustidian-0.4-x86_64-slim.AppImage

**portable** (~110MB) — bundles the entire webkit/gtk closure, zero system deps:

    curl -LO https://github.com/jpzk/rustidian/releases/download/v0.4/rustidian-0.4-x86_64-portable.AppImage
    chmod +x rustidian-0.4-x86_64-portable.AppImage
    ./rustidian-0.4-x86_64-portable.AppImage

No FUSE on your box (containers, minimal VMs)? Run either without mounting:

    ./rustidian-0.4-x86_64-slim.AppImage --appimage-extract-and-run

## sandboxed run (recommended)

Since v0.4 the app also sandboxes ITSELF with Landlock (kernel ≥ 5.13): once a vault
is open the whole process tree can only write inside that vault (+ its own config
and caches) and cannot read your home directory. Nothing to configure; blank window
on an exotic setup? `RUSTIDIAN_NO_LANDLOCK=1` turns it off, please file an issue.

Your notes are just files, but the app doesn't need to see the rest of your
home directory. With [bubblewrap](https://github.com/containers/bubblewrap)
(`apt/dnf install bubblewrap` — unprivileged, no SUID) you can confine
rustidian to ONLY your vault. Grab the launcher next to the AppImage and:

    curl -LO https://github.com/jpzk/rustidian/releases/download/v0.4/rustidian-sandboxed.sh
    chmod +x rustidian-sandboxed.sh
    ./rustidian-sandboxed.sh ~/vault

It finds the AppImage in the current dir, handles X11/Wayland + xauth, and
bwraps everything else away behind a tmpfs: `~/.ssh`, browser profiles, the
lot — the app sees only the vault. (`scripts/rustidian-sandboxed.sh` in the
repo if you'd rather read it first — you should.) Vault persistence
(`~/.rustidian.json`) lands on the tmpfs, so the picker asks again each
launch; bind a scratch dir over `$HOME` in the script if you want it kept.
