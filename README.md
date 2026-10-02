

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

    curl -LO https://github.com/jpzk/opensidian/releases/download/v0.1/opensidian-0.1-x86_64-slim.AppImage
    chmod +x opensidian-0.1-x86_64-slim.AppImage && ./opensidian-0.1-x86_64-slim.AppImage

**portable** (no system deps): [opensidian-0.1-x86_64-portable.AppImage](https://github.com/jpzk/opensidian/releases/download/v0.1/opensidian-0.1-x86_64-portable.AppImage)

Opt-in Landlock sandbox: `OPENSIDIAN_LANDLOCK=1`.

## Build from source
Debian/Ubuntu deps, then Rust (tested on 1.98.0):

    sudo apt install build-essential libssl-dev libwebkit2gtk-4.1-dev \
      libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
    git clone https://github.com/jpzk/opensidian && cd opensidian/src-tauri
    cargo build --release --locked   # -> target/release/opensidian

## Licence
GPL-3.0-or-later ([LICENSE](LICENSE)). Read [THIRD-PARTY.md](THIRD-PARTY.md) before redistributing.
