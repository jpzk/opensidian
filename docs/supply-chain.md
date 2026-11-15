# Supply-chain checks

Workflow: `.github/workflows/supply-chain.yml`. It runs on every `pull_request`, on every push to
`develop`/`main`, and once a week (Monday 04:23 UTC), so new advisories also show up when no code
changes. It covers the Rust graph in `src-tauri/Cargo.lock`. The workflow uses
`permissions: contents: read` and no secrets. It uses `pull_request`, never `pull_request_target`, so
fork PRs run without write access. Every action is pinned by full commit SHA, and every tool is
installed at an exact version with `cargo install --locked --version X`. Versions are chosen to be at
least 6 weeks old.

## What each check catches

- **cargo audit** (`cargo audit --deny warnings`, run in `src-tauri/`). Checks every crate in
  `Cargo.lock` against the RustSec advisory DB. It fails on vulnerabilities and, because of
  `--deny warnings`, also on unmaintained, unsound and yanked crates. Ignored ids live in
  `src-tauri/.cargo/audit.toml`.
- **cargo deny** (`cargo deny --manifest-path src-tauri/Cargo.toml --config deny.toml --locked check`).
  It runs four checks:
  - `advisories`: the same RustSec DB, plus a deny on yanked versions. Ignores are in `deny.toml` and
    must match `audit.toml`.
  - `sources`: only the crates.io registry is allowed. Any other registry or any git dependency
    fails (`unknown-registry`/`unknown-git = "deny"`, `allow-git = []`). The vendored `glib` `[patch]`
    is a path crate and is allowed.
  - `bans`: wildcard (`*`) version requirements fail. Duplicate versions only warn.
  - `licenses`: every dependency license has to be on the allow-list. Licenses outside the list
    (GPL, AGPL, unknown, unlicensed) fail. The app crate's own GPL-3.0-or-later is an exception that
    applies to `opensidian` only.
- **cargo vet** (`cargo vet --locked`, run in `src-tauri/`). Every crate version has to be covered
  by an audit. That can be our own (`supply-chain/audits.toml`), an import from mozilla, google,
  bytecode-alliance, zcash or isrg (pinned in `supply-chain/imports.lock`), or an exemption
  (`supply-chain/config.toml`). The baseline exempts the graph as of 2026-11-15. A **new crate or a
  version bump without an audit fails**. `--locked` means CI uses the committed `imports.lock` and
  never pulls new imported audits by itself.

## Adding or updating a dependency

1. Change `Cargo.toml` and commit the updated `Cargo.lock`.
2. Run the checks locally with the same versions as CI (see `env:` in the workflow):
   ```sh
   cargo install --locked --version 0.10.2 cargo-vet   # likewise cargo-deny 0.20.2, cargo-audit 0.22.2
   cd src-tauri && cargo vet
   ```
3. If `cargo vet` reports an unvetted crate, pick one of these options:
   - **Import.** `cargo vet` (without `--locked`) may find that a trusted importer already audited it.
     Commit the updated `imports.lock`.
   - **Audit it yourself.** Run `cargo vet inspect <crate> <version>` (or
     `cargo vet diff <crate> <old> <new>` for a bump), read the code against the criteria
     (`safe-to-deploy` covers anything that ships in the app), then
     `cargo vet certify <crate> <version>`. This records the audit in `supply-chain/audits.toml`, and
     the PR reviewer checks it.
   - **Exempt it (last resort).** Run `cargo vet add-exemption <crate> <version>` and say why in the
     PR. Exemptions are debt. Run `cargo vet prune` from time to time to drop the ones imports now
     cover.
4. If you need a new license, add it to `[licenses].allow` in `deny.toml` in the same PR and explain
   it there. If you need a git dependency, add its URL to `[sources].allow-git` and pin it by `rev`.
5. To ignore an advisory, add its RUSTSEC id to **both** `deny.toml` (`[advisories].ignore`, with
   `reason`) and `src-tauri/.cargo/audit.toml`, with a reason and an **expiry date**. Remove it when
   it expires.

## Limits

- **The advisory DB is not pinned.** cargo audit and cargo deny fetch the RustSec DB fresh on every
  run. A red run with no code change usually means a new advisory was published, so use the weekly
  schedule as an alarm. It also means results can't be reproduced bit for bit.
- **cargo vet trusts people, not code.** Imported audits are only as good as the organisations
  behind them. Exemptions in the baseline were not audited. They only freeze today's graph.
- `cargo vet --locked` does not pick up audits that importers publish later, so run `cargo vet` (or
  `cargo vet regenerate imports`) on purpose to refresh `imports.lock`.
- A git dependency for a crate version that is already exempted or audited still passes cargo vet.
  That case is caught by cargo deny `sources` (`unknown-git = "deny"`), so keep both checks.
- Only the Rust graph is covered. The npm frontend, the GitHub Actions themselves (they are only
  SHA-pinned) and system libraries (GTK/WebKitGTK from the distro) are out of scope.
- None of these checks catch malicious code in an already-trusted version, or a compromised build
  environment. Pinning plus the 6-week age rule just shrinks that window.
