
<img width="800" height="574" alt="GH" src="https://github.com/user-attachments/assets/fff70236-3976-4b87-b3e0-f1ec2aca1e10" />


# opensidian
Open-source, vault-compatible markdown notes app. Rust + Tauri v2, no Electron, no npm. AI-built experiment under heavy development - don't trust, verify. We're looking for Linux distribution maintainers. 

## Features
- live preview, source and reading modes
- [[wikilinks]], backlinks, tags, graph view
- tabs, splits, search, quick switcher, command palette, bookmarks
- themes, fonts, hotkeys; open a vault with `opensidian <folder>`

## Install
Linux x86_64. Download one:

| Download | Choose it if |
|---|---|
| [slim AppImage](https://github.com/jpzk/opensidian/releases/download/v0.3/opensidian-0.3-x86_64-slim.AppImage) | you want the smallest file and already have WebKitGTK 4.1 installed (see below) |
| [portable AppImage](https://github.com/jpzk/opensidian/releases/download/v0.3/opensidian-0.3-x86_64-portable.AppImage) | you want nothing to install: WebKit is bundled (needs only the X11/GL/font libraries every desktop has) |
| [Flatpak](https://github.com/jpzk/opensidian/releases/download/v0.3/opensidian-0.3-x86_64.flatpak) | you want it sandboxed; pulls the GNOME 49 runtime from Flathub on first install |

AppImage: `chmod +x opensidian-*.AppImage && ./opensidian-*.AppImage`. Flatpak: `flatpak install opensidian-0.3-x86_64.flatpak && flatpak run dev.koto.opensidian`. Opt-in Landlock sandbox: `OPENSIDIAN_LANDLOCK=1`.

### Tested distributions
v0.3 on fresh installs, launched and rendered (2026-10-04):

- **Fedora 44**: slim works with `sudo dnf install webkit2gtk4.1` (tested 2.54.0); flatpak works (`sudo dnf install flatpak`)
- **Ubuntu 26.04 LTS**: slim works with `sudo apt install libwebkit2gtk-4.1-0` (tested 2.52.6); flatpak works (`sudo apt install flatpak`)

Without FUSE, run it with `--appimage-extract` and start `squashfs-root/AppRun`.

## Verify your download
`SHA256SUMS` is signed with key `A6E6 9ED6 BC47 79F3 2817  2148 4CC1 AFDE 15B6 4EA3`:

    curl -LO https://github.com/jpzk/opensidian/releases/download/v0.3/SHA256SUMS \
         -LO https://github.com/jpzk/opensidian/releases/download/v0.3/SHA256SUMS.asc
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
Themes are plain files in your vault, laid out the same way Obsidian lays them out, so most Obsidian
community themes work as they are.

Four community themes ship with the app, unmodified: **AnuPpuccin**, **Material Gruvbox**, **Minimal**
and **Solarized** (authors, pinned versions and licences in [THIRD-PARTY.md](THIRD-PARTY.md)). **AnuPpuccin is the
default**: a vault with no theme chosen (a new vault, or an older one whose `cssTheme` is empty) opens
in it. The four are copied into `<vault>/.obsidian/themes/` the first time you open a vault. Files you
already have are never overwritten, and themes from earlier versions (1984, Slate, Wasp) stay in vaults
that already have them.

Minimal is built for the Style Settings plugin. opensidian doesn't support plugins, so Minimal runs
with its default settings.

Install a theme:

1. Create a folder named after the theme in `<vault>/.obsidian/themes/`, e.g. `.obsidian/themes/Things/`.
2. Put the theme's `manifest.json` and `theme.css` in that folder. The `"name"` in `manifest.json`
   has to match the folder name exactly.
3. Pick it in **Settings ▸ Appearance ▸ Themes**, or from the command palette (`Ctrl+P` ▸ "Use theme: …").
   Your choice is saved as `cssTheme` in `.obsidian/appearance.json`. Pick **AnuPpuccin** to go back to
   the default.

If a theme folder is broken (no or unparsable `manifest.json`, wrong name, no `theme.css`), the dropdown
lists it greyed out with the reason and the app keeps painting AnuPpuccin. Dark/light mode still applies on
top of whichever theme you pick.

CSS snippets: put `<name>.css` in `<vault>/.obsidian/snippets/` and switch it on in
**Settings ▸ Appearance ▸ CSS snippets**. Snippets apply on top of the theme, in the order you turned them on.

Changes to theme and snippet files reload live. You don't need to restart.

## Donate

Donation makes it possible! 

bc1qzyh8c8kr8aylywqacsjwau6frzwjqt3cpa603w (BTC)

## AI disclosure

opensidian was built predominantly with AI models. Nearly all of the code, the design docs, and this README were written by Claude (Anthropic's models, via Claude Code) working from prompts, reviews, and corrections by a single human maintainer. The human decided what to build and what the trust model must guarantee, read and pushed back on the output, and ran it; the models wrote most of the lines. Commit messages record the design rationale in the same way — many were drafted by the model and edited by the maintainer.
