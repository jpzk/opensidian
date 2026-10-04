# AGENTS.md — working on opensidian

Read this before changing anything. It is for coding agents and humans alike.

## What this is
opensidian is a desktop markdown notes app that opens Obsidian-style vaults
(plain `.md` files plus a `.obsidian/` config folder). Rust backend on
Tauri v2, plain-JS frontend, Linux first (AppImage, Flatpak, plain binary).
Licence: GPL-3.0-or-later for all of it, since the first commit.

## Layout
- `src-tauri/` — the Rust crate (`opensidian`).
  - `src/main.rs`: app wiring, the ~100 `#[tauri::command]`s, vault binding
    (`bind_vault`; the vault root is a `OnceLock`, set once per process).
  - `src/index.rs`: in-memory note index (search, graph, backlinks, tags).
  - `src/watcher.rs`: polling watcher that reconciles external edits.
  - `src/spawn.rs`, `src/spawner.rs`, `src/vaultlock.rs`, `src/cfgstore.rs`:
    one vault per process. "Open another vault" starts a new process; a flock
    per vault stops a second backend; `~/.opensidian.json` is shared by all
    processes, so writes go through cfgstore.
  - `src/sandbox.rs`: optional Landlock self-sandbox (`OPENSIDIAN_LANDLOCK=1`).
  - `src/themefs.rs`, `src/builtins.rs`: themes and CSS snippets are files in
    the vault. The bundled community themes in `themes/` are vendored
    **byte-exact** and pinned by sha256 in builtins.rs. Never edit them.
  - `src/settings.rs`, `src/migrate.rs`, `src/srcmode.rs`, `src/outline.rs`,
    `src/datefmt.rs`, `src/perf.rs`: settings table, rename migration from the
    old product name, source mode, outline, date formats, lag telemetry.
  - `tests/`: integration tests and fixtures (`cargo test` needs nothing else).
- `ui/` — the frontend, served as-is (no bundler, no npm).
  - `main.js` (app), `editor.js` (Live Preview / source editor core),
    `graph-gl.js` (WebGL graph), `otel.js` (frontend lag spans),
    `census.js` (test tokens, append-only), `style.css`, `fonts/` (OFL).
- `packaging/flatpak/` — manifest, desktop file, metainfo.
- `vault/` — a small sample vault.
- `scripts/ docs/ notes/ bin/` are gitignored. The maintainer's test harness
  and design notes live in a separate private repo and appear here as
  symlinks. Never commit anything under those paths.

## Build and test
System deps and toolchain: see README "Build from source" (Rust 1.98.0).

    cd src-tauri
    cargo build --release --locked
    cargo test --locked

Always `--locked`. The listtoggle tests run `ui/editor.js` in JavaScriptCore,
so the webkit dev packages must be installed for tests too.

## Rules
- **Clean room.** Compatibility comes from public file formats and observed
  behaviour only. Never copy Obsidian's code, CSS, values, assets or text, and
  never unpack or inspect its binaries. Every design value (sizes, spacing,
  colours) must be our own, with its reasoning written down.
- **Licence headers.** Every new `.rs .js .html .css .sh` file carries
  `SPDX-License-Identifier: GPL-3.0-or-later` near the top (the release check refuses
  files without it; `src-tauri/themes/` is exempt). Third-party material goes
  in THIRD-PARTY.md with author, pinned version and licence.
- **Dependencies.** No new crate unless unavoidable. Pin with `=`, prefer
  crates already in Cargo.lock, never a release younger than 6 weeks, and
  never an unpinned or auto-updated dependency. No npm, no Python tooling.
- **Commits.** Author and committer are `jpzk <jendrik@madewithtea.com>`.
  No `Co-Authored-By` or "Generated with" trailers. Commit messages explain
  why; keep them free of third-party values.
- **Never commit** `progress.md`, `.goalenv`, `x`, `target/`, screenshots,
  or helper `.sh` files outside `packaging/`.
- **Frontend.** No frameworks, no bundler. Tests read live CSS tokens, never
  hard-coded hex values. `ui/census.js` is append-only: one new line per token.
- **Vault safety.** Never write outside the bound vault and the user config.
  Unknown keys in `.obsidian/*.json` must round-trip unchanged.
- **Themes.** The bundled themes are seeded into a vault's
  `.obsidian/themes/` without overwriting user files. Changes to how a theme
  is chosen need a test fixture under `src-tauri/tests/fixtures/`.

## Releases
Version lives in `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`,
`Cargo.lock` and the Flatpak metainfo; bump all four together. A release ships
the plain binary, the AppImage and the Flatpak, a `SHA256SUMS` file, and the
maintainer's detached GPG signature (key fingerprint in README "Verify your
download"). Tags are annotated (`vX.Y`). Agents never hold the signing key.

## When in doubt
Ask one focused question rather than guess. Prefer small, reviewable
changes with a test that fails before and passes after.
