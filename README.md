# rustidian
Minimal open-source Obsidian clone. Rust + Tauri v2, no npm.
- markdown editor + live preview (pulldown-cmark, rendered in Rust)
- [[wikilinks]] -> clickable links + graph view (canvas force sim)
- vault = a directory of .md files (env VAULT_DIR, default ./vault)

## install
Grab the AppImage from the [latest release](https://github.com/jpzk/rustidian/releases):

    curl -LO https://github.com/jpzk/rustidian/releases/download/v0.1/rustidian-0.1-x86_64.AppImage
    chmod +x rustidian-0.1-x86_64.AppImage
    ./rustidian-0.1-x86_64.AppImage

No FUSE on your box (containers, minimal VMs)? Run it without mounting:

    ./rustidian-0.1-x86_64.AppImage --appimage-extract-and-run

## run from source
    VAULT_DIR=$PWD/vault cargo run --manifest-path src-tauri/Cargo.toml

## headless smoke test
    scripts/smoke.sh
## vnc into the headless display
    scripts/vnc.sh
