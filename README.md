
<img width="800" height="574" alt="GH" src="https://github.com/user-attachments/assets/fff70236-3976-4b87-b3e0-f1ec2aca1e10" />


# opensidian
Open-source and non-profit under GPLv3, vault-compatible cross-platform markdown notes app. It's based on Rust + Tauri v2 and under heavy development. We're looking for Linux distribution maintainers. Not affiliated with or endorsed by Obsidian or Dynalist Inc.; "Obsidian" is their trademark.

## Features
- live preview, source and reading modes
- [[wikilinks]], backlinks, tags, graph view
- tabs, splits, search, quick switcher, command palette, bookmarks
- themes, fonts, hotkeys; open a vault with `opensidian <folder>`
- drag an image (png, jpg, gif, webp) or a PDF from your file manager onto a note:
  an image is **moved** into the vault (the original is removed only after the
  vault copy is complete and on disk), a PDF is **copied** (the original stays,
  as in Obsidian), and `![[name]]` is inserted where you dropped it. A name that
  already exists gets a new one, never overwritten. A .pdf must really be a PDF (it starts with `%PDF-`); other file
  types are refused and left where they are.

## Install
Linux x86_64. Download one:

| Download | Choose it if | Tested on (fresh install, 2026-10-09) |
|---|---|---|
| [slim AppImage](https://github.com/jpzk/opensidian/releases/download/v0.6.2/opensidian-0.6.2-x86_64-slim.AppImage) | you want the smallest file and have WebKitGTK 4.1 | Fedora 44 (`sudo dnf install webkit2gtk4.1`), Ubuntu 26.04 LTS (`sudo apt install libwebkit2gtk-4.1-0`) |
| [portable AppImage](https://github.com/jpzk/opensidian/releases/download/v0.6.2/opensidian-0.6.2-x86_64-portable.AppImage) | you want nothing to install: WebKit is bundled (needs only the X11/GL/font libraries every desktop has) | Fedora 44, Ubuntu 26.04 LTS |
| [Flatpak](https://github.com/jpzk/opensidian/releases/download/v0.6.2/opensidian-0.6.2-x86_64.flatpak) | you want it sandboxed; pulls the GNOME 49 runtime from Flathub on first install | Fedora 44, Ubuntu 26.04 LTS (`flatpak` package installed) |
| [RPM](https://github.com/jpzk/opensidian/releases/download/v0.6.2/opensidian-0.6.2-x86_64.rpm) | you run Fedora and want a system package with a menu entry and icon; dnf pulls WebKitGTK 4.1 for you | Fedora 44 (`sudo dnf install ./opensidian-0.6.2-x86_64.rpm`) |
| [DEB](https://github.com/jpzk/opensidian/releases/download/v0.6.2/opensidian-0.6.2-x86_64.deb) | you run Debian or Ubuntu and want a system package with a menu entry and icon; apt pulls WebKitGTK 4.1 for you | Debian 13, Ubuntu 26.04 LTS (`sudo apt install ./opensidian-0.6.2-x86_64.deb`) |

## Verify your download
`SHA256SUMS` is signed with key `A6E6 9ED6 BC47 79F3 2817  2148 4CC1 AFDE 15B6 4EA3`:

```sh
curl -LO https://github.com/jpzk/opensidian/releases/download/v0.6.2/SHA256SUMS \
     -LO https://github.com/jpzk/opensidian/releases/download/v0.6.2/SHA256SUMS.asc
gpg --keyserver hkps://keys.openpgp.org --recv-keys A6E69ED6BC4779F3281721484CC1AFDE15B64EA3
gpg --verify SHA256SUMS.asc SHA256SUMS
sha256sum -c --ignore-missing SHA256SUMS
```

## Build from source
Tested 2026-10-03 on fresh Ubuntu 26.04 LTS and Fedora 44 with Rust 1.98.0
(build ~2.5 min on 4 cores, binary runs and renders on both).

1. System deps. Ubuntu/Debian:

   ```sh
   sudo apt install build-essential libssl-dev libwebkit2gtk-4.1-dev \
     libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev
   ```

   Fedora:

   ```sh
   sudo dnf install gcc gcc-c++ make openssl-devel webkit2gtk4.1-devel \
     gtk3-devel libappindicator-gtk3-devel librsvg2-devel
   ```

2. Rust, if you don't have it (distro rustc is often older than 1.98):

   ```sh
   curl --proto '=https' -sSf https://sh.rustup.rs | sh -s -- --default-toolchain 1.98.0
   . "$HOME/.cargo/env"
   ```

3. Build:

   ```sh
   git clone https://github.com/jpzk/opensidian && cd opensidian/src-tauri
   cargo build --release --locked   # -> target/release/opensidian
   ```

## Licence
GPL-3.0-or-later ([LICENSE](LICENSE)). Read [THIRD-PARTY.md](THIRD-PARTY.md) before redistributing.

## Themes
Most Obsidian community themes work as they are. Ships with **AnuPpuccin** (default), **Material Gruvbox**,
**Minimal** and **Solarized**, unmodified ([THIRD-PARTY.md](THIRD-PARTY.md)).

To add one, put its `manifest.json` and `theme.css` in `<vault>/.obsidian/themes/<Name>/` (folder name =
`"name"` in the manifest) and pick it in **Settings ▸ Appearance ▸ Themes**. CSS snippets go in
`.obsidian/snippets/`. Both reload live. A broken theme shows greyed out with the reason, and the app
stays on AnuPpuccin.

## Donate

Donation makes it possible! 

bc1qzyh8c8kr8aylywqacsjwau6frzwjqt3cpa603w (BTC)

## AI disclosure

opensidian was built predominantly with AI models. Nearly all of the code, the design docs, and this README were written by Claude (Anthropic's models, via Claude Code) working from prompts, reviews, and corrections by a single human maintainer. The human decided what to build and what the trust model must guarantee, read and pushed back on the output, and ran it; the models wrote most of the lines. Commit messages record the design rationale in the same way — many were drafted by the model and edited by the maintainer.
