# rustidian
Minimal open-source Obsidian clone. Rust + Tauri v2, no npm.
- markdown editor + live preview (pulldown-cmark, rendered in Rust)
- [[wikilinks]] -> clickable links + graph view (canvas force sim)
- vault = a directory of .md files (env VAULT_DIR, default ./vault)

## run
    VAULT_DIR=$PWD/vault cargo run --manifest-path src-tauri/Cargo.toml

## headless smoke test
    scripts/smoke.sh
## vnc into the headless display
    scripts/vnc.sh
