

https://github.com/user-attachments/assets/71390471-2ee2-47e8-b413-7bb6949fc6e6

# opensidian
Open-source, vault-compatible markdown notes app. Rust + Tauri v2, no Electron, no npm. AI-built experiment under heavy development - don't trust, verify. We're looking for Linux distribution maintainers. 

## Features
- live preview, source and reading modes
- [[wikilinks]], backlinks, tags, graph view
- tabs, splits, search, quick switcher, command palette, bookmarks
- themes, fonts, hotkeys; open a vault with `opensidian <folder>`

## Install
**slim** (needs `libwebkit2gtk-4.1`):

    curl -LO https://github.com/jpzk/opensidian/releases/download/v0.18/opensidian-0.18-x86_64-slim.AppImage
    chmod +x opensidian-0.18-x86_64-slim.AppImage && ./opensidian-0.18-x86_64-slim.AppImage

**portable** (no system deps): [opensidian-0.18-x86_64-portable.AppImage](https://github.com/jpzk/opensidian/releases/download/v0.18/opensidian-0.18-x86_64-portable.AppImage)

Opt-in Landlock sandbox: `OPENSIDIAN_LANDLOCK=1`.

## Licence
GPL-3.0-or-later ([LICENSE](LICENSE)). Read [THIRD-PARTY.md](THIRD-PARTY.md) before redistributing.
