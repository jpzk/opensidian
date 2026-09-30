# opensidian
Minimal **open-source** Obsidian clone. Not based on Electron or JS but compiled down to machine code: Rust + Tauri v2, no npm yay. It's **under heavy development right now**.

## This is an experiment 
I'm using a DIY computer use to let AI write requirements to copy the UI/UX of Obsidian and implement it. Imitation is the best form of flattery. Obsidian is great, but it sucks that one has to run proprietary closed source blobs just for editing markdown files. This is maxxing out my Fable plan to open source Obsidian UX. 

## Security is top priority
I'm running as many top frontier models to scan for vulnerabilities, but since this is open source and you can do this, please do. Don't trust, verify.

## Features
- markdown editor with three modes — live preview (default), reading, source. Live preview runs the R17 editor core: a line model, one DOM row per source line and a token map, patched incrementally per keystroke with no Rust round-trip; reading mode renders through `pulldown-cmark` in Rust. Selection spans rows, copy/cut yield raw markdown, undo coalesces bursts, and the caret reveals only the TOKEN under it — not the whole line
- [[wikilinks]] -> clickable links + local graph view (canvas force sim)
- `[[note#heading]]`, `[[note#^block]]`, `[[note|alias]]` links: alias text, scroll-to-target + flash, `[[note#` heading autocomplete, block ids (` ^id`); Backlinks pane with Unlinked mentions + Link button
- splits / tab groups with tab drag & drop
- full-text search, quick switcher, command palette, bookmarks
- #tags: inline pills + frontmatter `tags:`, Tags pane in the right sidebar (nested a/b, counts), `tag:foo` search filter
- safe renames (H1 or F2) that rewrite [[links]] across the vault
- external edits (R11): the vault is watched (1s tick, std-only); shell edits to an open note appear in place with the caret kept, unsaved local text is merged with external appends, external create/rename/delete update the explorer and close the affected tab
- source mode (R12): live preview with every marker revealed — grey `#`/`**`/`[[` markers in the text flow, headings keep their size, bold/italic/strike/highlight/code styled, links + `#tag` in accent, fences shaded with the fence lines visible, literal `- [ ]` tasks; same font and measure as live preview, one raw caret row, `[[` autocomplete
- hotkeys (R14): Settings (Ctrl+,) ▸ Hotkeys — every command from one registry (also feeds the palette) with the stock Obsidian defaults, fuzzy filter + All/Assigned/Assigned by me/Unassigned chips, click ⊕ to record a chord, ✕ to remove, ↺ restore default, duplicate chords flagged red with a `Conflicts N` chip; overrides persist in `~/.opensidian.json` `hotkeys` (stock shape, `[]` = removed default)
- vault picker with persistence (or env VAULT_DIR, default ./vault)
- graph (R16): stock-faithful force layout (d3 semantics, world-space, camera fit), drag a node and it stays pinned where you drop it (empty-canvas drag pans), WebGL renderer by default with a Canvas 2D fallback (`OPENSIDIAN_GRAPH_RENDERER=gl|2d`)
- typography (R15): the exact Obsidian 1.13.7 type metrics in reading, live preview and source mode — bundled Inter Variable 4.001 + Source Code Pro 2.030 (SIL OFL, sha256-pinned via `scripts/fetch-fonts.sh --verify`), stock font stacks, h1–h6 sizes/weights/line-heights, list/checkbox/code/blockquote geometry within 1px of stock
- hardening: vault paths canonicalised and confined to the vault root, system dirs refused, symlinks and >32 MiB files skipped, link scheme allowlist (http/https/mailto only; opened via `xdg-open`), in-app navigation locked to the app origin, Landlock self-sandbox, opt-in via `OPENSIDIAN_LANDLOCK=1` (off by default)
- themes: dark and light, chosen in Settings or followed from the system preference in BOTH directions, and the stored choice survives a restart — asserted from pixels, not from a class name; plus an accent PALETTE axis independent of the light/dark axis. `ui/style.css` is one token block: 239 colour literals became tokens with no pixel moved, and a lint keeps new literals out
- panes and window (R37): drag a pane divider to resize (the split keeps its ratio across a window resize), and the window frame — move, resize, maximise, restore — round-trips from the keyboard alone
- bookmarks follow the note (R9.8): rename or MOVE a bookmarked note and its bookmark moves with it, rather than dangling at the old path
- note titles (R34.18): the title surfaces resolve the same type metrics as the note column, so an h1 is one size wherever you read it
- developer: a perf console that prints `[perf][SLOW] op=... ms=... ceiling=100` for any operation over a fixed 100 ms ceiling, with a healthy run staying SILENT (the gate asserts both halves)

## Screenshots


## install
Two AppImage flavors on the [latest release](https://github.com/jpzk/opensidian/releases):

**slim** (recommended, ~3MB) — just the binary; uses your system's webkit.
Needs `libwebkit2gtk-4.1` installed (`apt install libwebkit2gtk-4.1-0` /
`dnf install webkit2gtk4.1`) — the AppImage tells you if it's missing:

    curl -LO https://github.com/jpzk/opensidian/releases/download/v0.16/opensidian-0.16-x86_64-slim.AppImage
    chmod +x opensidian-0.16-x86_64-slim.AppImage
    ./opensidian-0.16-x86_64-slim.AppImage

**portable** (~110MB) — bundles the entire webkit/gtk closure, zero system deps:

    curl -LO https://github.com/jpzk/opensidian/releases/download/v0.16/opensidian-0.16-x86_64-portable.AppImage
    chmod +x opensidian-0.16-x86_64-portable.AppImage
    ./opensidian-0.16-x86_64-portable.AppImage

No FUSE on your box (containers, minimal VMs)? Run either without mounting:

    ./opensidian-0.16-x86_64-slim.AppImage --appimage-extract-and-run

## Landlock (opt-in)

The app can sandbox ITSELF with Landlock (kernel ≥ 5.13). It is OFF by
default; opt in with:

    OPENSIDIAN_LANDLOCK=1 ./opensidian-*-x86_64-slim.AppImage

Once a vault is open, the whole process tree can then only write inside that
vault (+ its own config and caches) and cannot read your home directory. A
stderr line at launch tells you which mode you got: `landlock: off (default;
OPENSIDIAN_LANDLOCK=1 enables)` or `landlock: FullyEnforced`.
`OPENSIDIAN_NO_LANDLOCK=1` always wins and turns it off, even when
`OPENSIDIAN_LANDLOCK=1` is also set. While enforced, switching to a vault
outside the one you opened needs a restart.

## licence

**GPL-3.0-or-later.** Full text in [LICENSE](LICENSE).

Why GPL: this repo exists because "it sucks that one has to
run proprietary closed source blobs just for editing markdown files" (above).
A permissive licence would let someone take this, close it, and ship exactly
the blob the project was written to avoid. The GPL does not. If you distribute a modified
opensidian, you pass on the source under the same terms.

Third-party material that travels with the code — the Catppuccin Mocha palette
(MIT), and an unresolved contamination that is nobody's to license — is
itemised in [THIRD-PARTY.md](THIRD-PARTY.md). Read that one before you
redistribute.
