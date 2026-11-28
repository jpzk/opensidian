# AGENTS.md — working on opensidian

Read this before changing anything. It is for coding agents and humans alike.

## What this is
opensidian is a desktop markdown notes app that opens Obsidian-style vaults
(plain `.md` files plus a `.obsidian/` config folder). Rust backend on
Tauri v2, plain-JS frontend, Linux first (AppImage, Flatpak, plain binary).
It also runs community plugins through an API shim. Licence:
GPL-3.0-or-later for all of it, since the first commit.

## Layout
- `src-tauri/` — the Rust crate (`opensidian`).
  - `src/main.rs`: app wiring, almost all of the ~170 `#[tauri::command]`s
    (a few live in plugin_store, clip, secrets, settings, updates, mdtable),
    the page CSP (`page_csp`, `csp_by_trust`), vault binding (`bind_vault`;
    the vault root is a `OnceLock`, set once per process).
  - `src/index.rs`: in-memory note index (search, graph, backlinks, tags).
  - `src/watcher.rs`: polling watcher that reconciles external edits.
  - `src/spawn.rs`, `src/spawner.rs`, `src/vaultlock.rs`, `src/cfgstore.rs`:
    one vault per process. "Open another vault" starts a new process; a flock
    per vault stops a second backend; `~/.opensidian.json` is shared by all
    processes, so writes go through cfgstore.
  - `src/themefs.rs`, `src/builtins.rs`: themes and CSS snippets are files in
    the vault. The bundled community themes in `themes/` are vendored
    **byte-exact** and pinned by sha256 in builtins.rs. Never edit them.
  - `src/settings.rs`, `src/migrate.rs`, `src/srcmode.rs`, `src/outline.rs`,
    `src/datefmt.rs`, `src/perf.rs`: settings table, rename migration from the
    old product name, source mode, outline, date formats, lag telemetry.
  - Plugin and security layer:
    - `src/vaultfs.rs`: every write inside a vault; walks from a vault dir fd
      with `O_NOFOLLOW`, refuses symlinks, atomic replace.
    - `src/canvasfs.rs`: file I/O of the Canvas core feature (JSON Canvas
      1.0), its own narrow vault-confined surface.
    - `src/plugins.rs`: the `opensidian-plugin://` scheme that serves a
      plugin's `main.js` only for a trusted vault; trust keyed per vault.
    - `src/plugin_store.rs`: community plugin browser; Rust does every fetch
      (https, host allowlist re-checked per redirect, timeouts, byte caps).
    - `src/security.rs`: Settings › Security, the one reader of the
      `security` config key; CSP layers and the lockdown overlay.
    - `src/consent.rs`: native GTK consent dialogs (trust a vault, start a
      program), text built in Rust, default answer = deny.
    - `src/spawn_allow.rs`: "Always allow this program" grants, keyed by
      vault, plugin id and program, lapsing when plugin or program change.
    - `src/desk_proc.rs`: the `child_process` behind plugin spawn/exec
      (argv only, own process group, trusted vault + consent).
    - `src/desk_net.rs`: TCP listeners behind node `net`/`http` servers
      for plugins of a trusted vault.
    - `src/desk_http.rs`: HTTP client behind `request`/`requestUrl`
      (plain http; https is refused, no TLS crate).
    - `src/secrets.rs`: `app.secretStorage` on the OS keyring (Secret
      Service); never plaintext on disk.
    - `src/sandbox.rs`: optional Landlock self-sandbox
      (`OPENSIDIAN_LANDLOCK=1`), applied once at boot before webkit.
  - `tests/`: integration tests and fixtures (`cargo test` needs nothing else).
- `ui/` — the frontend, served as-is (no bundler, no npm).
  - `main.js` (app), `editor.js` (Live Preview / source editor core),
    `graph-gl.js` (WebGL graph), `otel.js` (frontend lag spans),
    `census.js` (test tokens, append-only), `menu.js` + `menu.css` (the
    Obsidian-shaped `.menu` context menu), `edmenu.js` (the editor
    right-click menu on it), `security.js` (Settings › Security),
    `style.css`, `fonts/` (OFL), `vendor/` (pinned in `vendor.lock.json`).
  - `plugins/`: the community plugin host.
    - `api.js` + `api-*.js`, `editor-api.js`, `views.js`: the `obsidian`
      module, written from the public `obsidian.d.ts` typings only (clean
      room).
    - `loader.js`: plugin loader and `require()` shim; seals the page
      (removes `window.__TAURI__`) before plugin code runs.
    - `node-leaves.js`, `node-modules.js`, `node-desktop.js`: allowlisted
      Node-compatible modules (pure leaves, vault-confined `fs`, desktop
      shims); any other name stays denied.
    - `canvas*.js`: the Canvas core feature (JSON Canvas 1.0 spec).
    - `settings.js`, `browser.js`: Settings › Community plugins (restricted
      mode switch) and the plugin browser UI.
- `packaging/flatpak/` — manifest, desktop file, metainfo.
- `packaging/ci/` — release build env (pinned Fedora 44 container + rpm lock
  `fedora-pkgset.lock.tsv`), AppImage/rpm/deb wraps, launch smoke; see its README.
- `vault/` — a small sample vault.
- `scripts/ docs/ notes/ bin/` are gitignored. The maintainer's test harness
  and design notes live in a separate private repo and appear here as
  symlinks. Never commit anything under those paths.

## Build and test
System deps and toolchain: see README "Build from source" (Rust 1.98.0).

    cd src-tauri
    cargo build --release --locked
    cargo test --locked

Always `--locked`. The listtoggle and procstdio tests run real `ui/` code in
JavaScriptCore, so the webkit dev packages must be installed for tests too.

Release builds (release.yml job `appimage`) do not use the host's packages:
`packaging/ci/appimage-build.sh` builds inside the digest-pinned Fedora 44 base
image with exactly the rpm set in `packaging/ci/fedora-pkgset.lock.tsv`
(sha256 + Fedora GPG `rpm -K`, offline install). The PR test env (linux.yml)
uses the same env. Never add a package from a repo or change a pin by hand:
re-resolve the lock (packaging/ci/README.md) — pin-check fails CI on drift.

## Testing levels
- **T0** — `cargo test --locked`: Rust unit tests in every module, the
  integration tests in `src-tauri/tests/`, and the JavaScriptCore tests of
  frontend code. Must pass before anything else runs.
- **T1 / T2** — the maintainer's private harness drives the built app (T1:
  feature phases, T2: wider end-to-end gates). The reference is stock
  Obsidian 1.13.7 behaviour, observed from outside only (clean room).
- **T3** — security gates, run by the same harness against the built app:
  consent dialogs (default = refuse, no bypass, no silent "Always"), the
  CSP of restricted and trusted pages, the trust gate on every
  side-effecting command, plugin identity and data separation, network
  containment, vault confinement and the sandbox. Every T3 gate has a
  negative control: it is shown to go red when the protection it guards
  is removed. Any change to consent, trust, CSP, IPC commands, plugin
  loading, process spawning, networking or the sandbox must pass T3.
A change in behaviour needs a test that fails before and passes after.

## Security invariants
`withGlobalTauri` exposes IPC to page scripts, so the page is not a trust
boundary. Rust is.
- The CSP is `tauri.conf.json` narrowed or widened only in `page_csp`.
  `script-src` never contains `'unsafe-inline'`. `'unsafe-eval'` and
  `blob:` are added to `script-src` only for a trusted vault (for plugins),
  never in restricted mode or lockdown. Never widen it further.
- Every new `#[tauri::command]` states its gate in a comment: trust (vault
  trusted), consent (native dialog), or vault-confined. A side effect
  outside the vault needs a native consent dialog whose default is refuse.
- Never take caller identity (plugin id, vault, window) from page-supplied
  arguments for an authorization decision. Derive it in Rust.
- Paths are canonicalised and confined to the vault in Rust, never in JS.
  Vault writes go through `vaultfs`.
- No shell strings: `Command::new(program)` with an argv, never `sh -c`.
- Network listeners bind loopback unless the user asked otherwise.
- Secrets are never logged and never written to disk by us.
- Any change to consent or trust comes with tests that fail before and
  pass after.

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
