<img width="1000" height="525" alt="image-1794573234435" src="https://github.com/user-attachments/assets/8e448f62-eb11-4213-9848-4f99455d3d43" />

# Opensidian
Open-source and non-profit under GPLv3, vault-compatible cross-platform markdown notes app. It's based on Rust + Tauri v2 and under heavy development. We're looking for Linux distribution maintainers. Not affiliated with or endorsed by Obsidian or Dynalist Inc.; "Obsidian" is their trademark. Opensidian is a clean-room functional clone: it reimplements Obsidian's behaviour from the outside and contains none of its code.

## Install

### Linux x86_64 — packages

Download one from the [v0.9.3 release](https://github.com/jpzk/opensidian/releases/tag/v0.9.3):

| Download | Choose it if | Tested on |
|---|---|---|
| [slim AppImage](https://github.com/jpzk/opensidian/releases/download/v0.9.3/opensidian-0.9.3-x86_64-slim.AppImage) | you want the smallest file and have WebKitGTK 4.1 | Fedora 44 (`sudo dnf install webkit2gtk4.1`), Ubuntu 26.04 LTS (`sudo apt install libwebkit2gtk-4.1-0`) |
| [portable AppImage](https://github.com/jpzk/opensidian/releases/download/v0.9.3/opensidian-0.9.3-x86_64-portable.AppImage) | you want nothing to install: WebKit is bundled (needs only the X11/GL/font libraries every desktop has) | Fedora 44, Ubuntu 26.04 LTS |
| [Flatpak](https://github.com/jpzk/opensidian/releases/download/v0.9.3/opensidian-0.9.3-x86_64.flatpak) | you want it sandboxed; pulls the GNOME 49 runtime from Flathub on first install | Fedora 44, Ubuntu 26.04 LTS (`flatpak` package installed) |
| [RPM](https://github.com/jpzk/opensidian/releases/download/v0.9.3/opensidian-0.9.3-x86_64.rpm) | you run Fedora and want a system package with a menu entry and icon; dnf pulls WebKitGTK 4.1 for you | Fedora 44 (see the RPM note below) |
| [DEB](https://github.com/jpzk/opensidian/releases/download/v0.9.3/opensidian-0.9.3-x86_64.deb) | you run Debian or Ubuntu and want a system package with a menu entry and icon; apt pulls WebKitGTK 4.1 for you | Debian 13, Ubuntu 26.04 LTS (`sudo apt install ./opensidian-0.9.3-x86_64.deb`) |

**RPM on rpm 6 (e.g. Fedora 44):** the RPM itself is not GPG-signed (as in v0.9.2), and rpm 6 rejects unsigned
packages by default (`does not verify: no signature`). [Verify the file](#verify-your-download) first. Then install the dependencies from your
signed Fedora repositories, and skip the signature check only for the one file you verified:

```sh
sudo dnf install webkit2gtk4.1                           # dependencies, signature-checked as usual
sudo rpm -Uvh --nosignature opensidian-0.9.3-x86_64.rpm  # install or upgrade; fetches nothing else
```

Avoid `dnf install --nogpgcheck` here: it also turns off the signature checks for every dependency in the same transaction.

A signed RPM is planned for the next release.

### Nix & NixOS

```sh
nix profile add github:jpzk/opensidian
```

## Verify your download
Put the file(s) you downloaded in one directory together with `SHA256SUMS` and `SHA256SUMS.asc`.

1. **Signature:** `SHA256SUMS` is signed with key `A6E6 9ED6 BC47 79F3 2817  2148 4CC1 AFDE 15B6 4EA3`
   (the same key as v0.9.2):

   ```sh
   curl -LO https://github.com/jpzk/opensidian/releases/download/v0.9.3/SHA256SUMS \
        -LO https://github.com/jpzk/opensidian/releases/download/v0.9.3/SHA256SUMS.asc
   gpg --keyserver hkps://keys.openpgp.org --recv-keys A6E69ED6BC4779F3281721484CC1AFDE15B64EA3
   gpg --export A6E69ED6BC4779F3281721484CC1AFDE15B64EA3 > opensidian.gpg
   gpgv --keyring ./opensidian.gpg SHA256SUMS.asc SHA256SUMS   # accepts only this key
   ```

   Unlike a plain `gpg --verify`, `gpgv` with this one-key keyring rejects a signature made by any other key you may have.

2. **Checksums:** every file you downloaded must report `OK`:

   ```sh
   sha256sum -c --ignore-missing SHA256SUMS
   ```

3. **Build provenance:** v0.9.3 is the first release with GitHub artifact attestations (SLSA provenance).
   They prove each file was built by this repository's `release.yml` workflow on GitHub-hosted runners.
   With the [GitHub CLI](https://cli.github.com/):

   ```sh
   # repeat for each file you downloaded
   gh attestation verify opensidian-0.9.3-x86_64.rpm -R jpzk/opensidian \
     --signer-workflow jpzk/opensidian/.github/workflows/release.yml \
     --deny-self-hosted-runners
   gh release verify v0.9.3 -R jpzk/opensidian   # the release itself
   ```

   v0.9.2 and earlier releases have no attestations.

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

**Windows (MSVC / WebView2):** same `cargo build --release --locked` with Visual Studio
Build Tools and the WebView2 Runtime. Experimental — no MSI / signed release yet.

**macOS:** same cargo command; experimental path-walk vaultfs (not the Linux openat
backend). Not packaged or notarized — builds from source only.

## Nix
The repo ships a flake (Nix 2.4+). It pins nixpkgs unstable and builds with
`--locked` against `src-tauri/Cargo.lock`, so the toolchain and every crate
are pinned.

```sh
nix run .     # build and run
nix build .   # -> result/bin/opensidian, with the desktop file installed
nix develop   # dev shell: rust-analyzer, clippy, rustfmt,
              # clippy, rustfmt and all native deps; then:
              #   cd src-tauri && cargo test --locked
```

On NixOS, the `spawn::tests::switch_alive_probe_reads_proc` test fails:
it hardcodes `/bin/true`, which does not exist there. The flock/landlock
integration tests also need a wider sandbox than the Nix build sandbox
grants, which is why the flake runs `cargo test` from the dev shell and
not as a flake check.

## Licence
GPL-3.0-or-later ([LICENSE](LICENSE)). Read [THIRD-PARTY.md](THIRD-PARTY.md) before redistributing.

## Themes
Most Obsidian community themes work as they are. Ships with **AnuPpuccin** (default), **Material Gruvbox**,
**Minimal** and **Solarized**, unmodified ([THIRD-PARTY.md](THIRD-PARTY.md)).

To add one, put its `manifest.json` and `theme.css` in `<vault>/.obsidian/themes/<Name>/` (folder name =
`"name"` in the manifest) and pick it in **Settings ▸ Appearance ▸ Themes**. CSS snippets go in
`.obsidian/snippets/`. Both reload live. A broken theme shows greyed out with the reason, and the app
stays on AnuPpuccin.

## Community plugins
Off by default. Each vault starts in restricted mode: no community plugin code runs.

**Turning it on.** Open **Settings ▸ Community plugins** and switch **Restricted mode** off. The
choice is per vault and is saved in `~/.opensidian.json`, outside the vault, so a vault you open
cannot turn its own plugins on.

**What that means.** With restricted mode off, the plugins you enable run code from the internet
with your user's rights, the same as in Obsidian. They can run programs, open network connections
and servers, send HTTP requests to any host, and open files and links with your system's default
apps. There is no per-plugin permission prompt. Enable only plugins you trust.

**Browse.** **Settings ▸ Community plugins ▸ Browse** lists the community plugins. You can search,
sort, and open a plugin to read its details and README. Install, Update and Uninstall download
only from GitHub over HTTPS. The sha256 of every installed file is recorded. Installing does not
enable a plugin; you enable it yourself. Copying a plugin folder into `<vault>/.obsidian/plugins/`
still works.

**Secrets.** Plugins that store secrets (API keys, tokens) use your system keyring through the
Secret Service: gnome-keyring, KWallet or KeePassXC. Secrets are never written to a file. Without
a running keyring, secret storage reports that it is unsupported and those plugins cannot save
secrets.

## Donate

Donation makes it possible! 

bc1qzyh8c8kr8aylywqacsjwau6frzwjqt3cpa603w (BTC)

## AI disclosure

opensidian was built predominantly with AI models. Nearly all of the code, the design docs, and this README were written by Claude (Anthropic's models, via Claude Code) working from prompts, reviews, and corrections by a single human maintainer. The human decided what to build and what the trust model must guarantee, read and pushed back on the output, and ran it; the models wrote most of the lines. Commit messages record the design rationale in the same way — many were drafted by the model and edited by the maintainer.
