
<img width="800" height="574" alt="GH" src="https://github.com/user-attachments/assets/fff70236-3976-4b87-b3e0-f1ec2aca1e10" />


# opensidian
Open-source, vault-compatible markdown notes app. Rust + Tauri v2, no Electron, no npm. AI-built experiment under heavy development - don't trust, verify. We're looking for Linux distribution maintainers. Not affiliated with or endorsed by Obsidian or Dynalist Inc.; "Obsidian" is their trademark.

## Features
- live preview, source and reading modes
- [[wikilinks]], backlinks, tags, graph view
- tabs, splits, search, quick switcher, command palette, bookmarks
- themes, fonts, hotkeys; open a vault with `opensidian <folder>`

## Install
Linux x86_64. Download one:

| Download | Choose it if | Tested on (fresh install, 2026-10-06) |
|---|---|---|
| [slim AppImage](https://github.com/jpzk/opensidian/releases/download/v0.4/opensidian-0.4-x86_64-slim.AppImage) | you want the smallest file and have WebKitGTK 4.1 | Fedora 44 (`sudo dnf install webkit2gtk4.1`), Ubuntu 26.04 LTS (`sudo apt install libwebkit2gtk-4.1-0`) |
| [portable AppImage](https://github.com/jpzk/opensidian/releases/download/v0.4/opensidian-0.4-x86_64-portable.AppImage) | you want nothing to install: WebKit is bundled (needs only the X11/GL/font libraries every desktop has) | Fedora 44, Ubuntu 26.04 LTS |
| [Flatpak](https://github.com/jpzk/opensidian/releases/download/v0.4/opensidian-0.4-x86_64.flatpak) | you want it sandboxed; pulls the GNOME 49 runtime from Flathub on first install | Fedora 44, Ubuntu 26.04 LTS (`flatpak` package installed) |
| [RPM](https://github.com/jpzk/opensidian/releases/download/v0.4/opensidian-0.4-x86_64.rpm) | you run Fedora and want a system package with a menu entry and icon; dnf pulls WebKitGTK 4.1 for you | Fedora 44 (`sudo dnf install ./opensidian-0.4-x86_64.rpm`) |
| [DEB](https://github.com/jpzk/opensidian/releases/download/v0.4/opensidian-0.4-x86_64.deb) | you run Debian or Ubuntu and want a system package with a menu entry and icon; apt pulls WebKitGTK 4.1 for you | Debian 13, Ubuntu 26.04 LTS (`sudo apt install ./opensidian-0.4-x86_64.deb`) |

AppImage: `chmod +x opensidian-*.AppImage && ./opensidian-*.AppImage` (without FUSE: `--appimage-extract`, then run `squashfs-root/AppRun`). Flatpak: `flatpak install opensidian-0.4-x86_64.flatpak && flatpak run dev.koto.opensidian`. RPM: `sudo dnf install ./opensidian-0.4-x86_64.rpm`, remove with `sudo dnf remove opensidian`. DEB: `sudo apt install ./opensidian-0.4-x86_64.deb`, remove with `sudo apt purge opensidian`. The rpm and deb are unsigned, so check them against the signed `SHA256SUMS` below. Opt-in Landlock sandbox: `OPENSIDIAN_LANDLOCK=1`.

## Verify your download
`SHA256SUMS` is signed with key `A6E6 9ED6 BC47 79F3 2817  2148 4CC1 AFDE 15B6 4EA3`:

    curl -LO https://github.com/jpzk/opensidian/releases/download/v0.4/SHA256SUMS \
         -LO https://github.com/jpzk/opensidian/releases/download/v0.4/SHA256SUMS.asc
    gpg --keyserver hkps://keys.openpgp.org --recv-keys A6E69ED6BC4779F3281721484CC1AFDE15B64EA3
    gpg --verify SHA256SUMS.asc SHA256SUMS
    sha256sum -c --ignore-missing SHA256SUMS

## Build from source
Tested 2026-10-03 on fresh Ubuntu 26.04 LTS and Fedora 44 with Rust 1.98.0
(build ~2.5 min on 4 cores, binary runs and renders on both).

1. System deps. Ubuntu/Debian:

       sudo apt install build-essential libssl-dev libwebkit2gtk-4.1-dev \
         libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev

   Fedora:

       sudo dnf install gcc gcc-c++ make openssl-devel webkit2gtk4.1-devel \
         gtk3-devel libappindicator-gtk3-devel librsvg2-devel

2. Rust, if you don't have it (distro rustc is often older than 1.98):

       curl --proto '=https' -sSf https://sh.rustup.rs | sh -s -- --default-toolchain 1.98.0
       . "$HOME/.cargo/env"

3. Build:

       git clone https://github.com/jpzk/opensidian && cd opensidian/src-tauri
       cargo build --release --locked   # -> target/release/opensidian

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
