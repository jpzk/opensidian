# rustidian
Minimal **open-source** Obsidian clone. Not based on Electron or JS but compiled down to machine code: Rust + Tauri v2, no npm yay. It's **under heavy development right now**.

## This is an experiment 
I'm using a DIY computer use to let AI write requirements to copy the UI/UX of Obsidian and implement it. Imitation is the best form of flattery. Obsidian is great, but it sucks that one has to run proprietary closed source blobs just for editing markdown files. This is maxxing out my Fable plan to open source Obsidian UX. 

## Security is top priority
I'm running as many top frontier models to scan for vulnerabilities, but since this is open source and you can do this, please do. Don't trust, verify.

## Features
- markdown editor + live preview (pulldown-cmark, rendered in Rust)
- [[wikilinks]] -> clickable links + graph view (canvas force sim)
- vault = a directory of .md files (env VAULT_DIR, default ./vault)

## Screenshots


## install
Grab the AppImage from the [latest release](https://github.com/jpzk/rustidian/releases):

    curl -LO https://github.com/jpzk/rustidian/releases/download/v0.1/rustidian-0.1-x86_64.AppImage
    chmod +x rustidian-0.1-x86_64.AppImage
    ./rustidian-0.1-x86_64.AppImage

No FUSE on your box (containers, minimal VMs)? Run it without mounting:

    ./rustidian-0.1-x86_64.AppImage --appimage-extract-and-run
