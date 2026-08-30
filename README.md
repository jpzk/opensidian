# rustidian
Minimal **open-source** Obsidian clone. Not based on Electron or JS but compiled down to machine code: Rust + Tauri v2, no npm yay. It's **under heavy development right now**.

## This is an experiment 
I'm using a DIY computer use to let AI write requirements to copy the UI/UX of Obsidian and implement it. Imitation is the best form of flattery. Obsidian is great, but it sucks that one has to run proprietary closed source blobs just for editing markdown files. This is maxxing out my Fable plan to open source Obsidian UX. 

## Security is top priority
I'm running as many top frontier models to scan for vulnerabilities, but since this is open source and you can do this, please do. Don't trust, verify.

## Features
- markdown editor + live preview (pulldown-cmark, rendered in Rust) — live preview is the default mode
- [[wikilinks]] -> clickable links + local graph view (canvas force sim)
- splits / tab groups with tab drag & drop
- full-text search, quick switcher, command palette, bookmarks
- safe renames (H1 or F2) that rewrite [[links]] across the vault
- vault picker with persistence (or env VAULT_DIR, default ./vault)

## Screenshots


## install
Two AppImage flavors on the [latest release](https://github.com/jpzk/rustidian/releases):

**slim** (recommended, ~3MB) — just the binary; uses your system's webkit.
Needs `libwebkit2gtk-4.1` installed (`apt install libwebkit2gtk-4.1-0` /
`dnf install webkit2gtk4.1`) — the AppImage tells you if it's missing:

    curl -LO https://github.com/jpzk/rustidian/releases/download/v0.2/rustidian-0.2-x86_64-slim.AppImage
    chmod +x rustidian-0.2-x86_64-slim.AppImage
    ./rustidian-0.2-x86_64-slim.AppImage

**portable** (~110MB) — bundles the entire webkit/gtk closure, zero system deps:

    curl -LO https://github.com/jpzk/rustidian/releases/download/v0.2/rustidian-0.2-x86_64-portable.AppImage
    chmod +x rustidian-0.2-x86_64-portable.AppImage
    ./rustidian-0.2-x86_64-portable.AppImage

No FUSE on your box (containers, minimal VMs)? Run either without mounting:

    ./rustidian-0.2-x86_64-slim.AppImage --appimage-extract-and-run

## sandboxed run (recommended)

Your notes are just files, but the app doesn't need to see the rest of your
home directory. With [bubblewrap](https://github.com/containers/bubblewrap)
(`apt/dnf install bubblewrap` — unprivileged, no SUID) you can confine
rustidian to ONLY your vault:

    bwrap \
      --ro-bind /usr /usr --ro-bind /etc /etc \
      --symlink usr/lib64 /lib64 --symlink usr/lib /lib \
      --proc /proc --dev /dev --dev-bind /dev/dri /dev/dri \
      --tmpfs /home --tmpfs /tmp --bind ~/vault ~/vault \
      --ro-bind /tmp/.X11-unix /tmp/.X11-unix \
      --setenv HOME "$HOME" --setenv DISPLAY "$DISPLAY" \
      --unshare-all --die-with-parent \
      ./rustidian-0.2-x86_64-slim.AppImage --appimage-extract-and-run

Everything outside the binds is invisible: `~/.ssh`, browser profiles, the
lot. Swap `~/vault` for your vault path (it's bound read-write; vault picker
naturally only sees that dir). On Wayland, also bind
`$XDG_RUNTIME_DIR/wayland-0` and pass `WAYLAND_DISPLAY` through. If webkit
complains about fonts or dbus, add
`--ro-bind ~/.cache/fontconfig ~/.cache/fontconfig` or
`--ro-bind $XDG_RUNTIME_DIR/bus $XDG_RUNTIME_DIR/bus` as needed.
`--appimage-extract-and-run` is required — FUSE mounts don't work inside
the namespace. Vault persistence (`~/.rustidian.json`) lands on the tmpfs
and is forgotten on exit; bind a scratch dir over `$HOME` instead of
`--tmpfs /home` if you want it kept.
