<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# RFC 0001: Vault sync

- Status: draft
- Created: 2026-10-20
- Scope: design only. This RFC adds no product code. Each phase in the implementation plan is its own goal.

## Summary

Opensidian should sync a vault between devices through a server the user runs themselves. The design in one paragraph:

- **Server**: one small Rust binary, `opensidian-syncd`, shipped as a static binary and as a container. It stores ciphertext only: an append-only, per-vault log of signed commits plus content-addressed encrypted blobs. Its one piece of logic is compare-and-swap on the log head. That gives every vault a single total order without trusting any device clock.
- **Client**: a sync engine inside the existing vault process (src-tauri). It feeds on the watcher that already reconciles external edits, writes through `vaultfs`, and keeps an offline outbox.
- **Conflicts**: three-way line merge against a stored base revision, with file identity that survives renames. Text that cannot be merged, and every binary file, keeps the resolving device's bytes at the path. The other side's bytes are saved as a conflict copy, so nothing is lost.
- **Setup**: start the server, connect the first device with the token the server prints, pair the second device with a QR code or a short code. Five steps.
- **Security**: end-to-end encryption, keys in the OS keychain, Ed25519 signatures on commits, a hash chain so a rollback or fork by the server is detected.

The rest of the document explains how each choice was made, what it costs, and how to build it in six goals.

## Motivation and requirements

The operator asked for "a sync feature for opensidian, conflict resolution + self-hosted + easy setup". Read as testable requirements:

- **R1 No silent loss.** No edit made on any device disappears without leaving a copy the user can find. This is the same rule as the F-class work in the editor ("a failed save leaves the tab dirty") applied across devices.
- **R2 Convergence.** After all devices go quiet and come online, every device has a byte-identical vault, minus per-device excluded files.
- **R3 Self-hosted.** The server is code we ship under GPL-3.0-or-later. It runs on a small VPS, a NAS or a home box, with no third-party account.
- **R4 Easy.** From zero to two synced devices in five steps or fewer, with no hand-edited config file, no port forwarding beyond one HTTPS port, and no CA certificate required.
- **R5 Plain files stay the truth.** The vault on disk remains ordinary Markdown that git, Syncthing or another editor can touch. Sync must tolerate that, not take ownership of the folder.
- **R6 Our rules hold.** Pinned dependencies at least six weeks old, clean room, writes only inside the bound vault and through `vaultfs`, nothing secret in plaintext at rest.

## Survey of existing approaches

Every entry gives a primary source, the licence and whether it is compatible with GPL-3.0-or-later, maturity, dependency-age notes against our pin rule (published at least 6 weeks before use, pinned with `=`), and a one-line verdict. Licences and dates were checked on 2026-10-20. Inspiration only: nothing below is copied, and no proprietary or source-available code was read.

- **Syncthing**: <https://syncthing.net/>, protocol spec <https://docs.syncthing.net/specs/bep-v1.html>, conflict docs <https://docs.syncthing.net/users/syncing.html>.
  - Licence MPL-2.0, which is GPL-3.0 compatible. Go daemon, very mature (about 89k stars, active).
  - Peer-to-peer with block-level transfer. On a conflict, the copy with the older modification time is renamed to `<name>.sync-conflict-<date>-<time>-<device>.<ext>`, and the conflict file then syncs like any other file.
  - Dependency age: not applicable. It would be a separate daemon, not a crate.
  - **Verdict:** best-in-class transport, but conflict choice by wall-clock mtime and no content merge. Users can already run it under Opensidian today because the watcher reconciles external edits. Document it as the "no server" alternative. Do not embed it.
- **Self-hosted LiveSync (Obsidian plugin)**: <https://github.com/vrtmrz/obsidian-livesync>, P2P notes <https://github.com/vrtmrz/obsidian-livesync/blob/main/docs/p2p.md>.
  - Licence MIT, compatible. TypeScript, mature and very active (about 12.7k stars).
  - Backends are CouchDB or S3-like object storage, plus an opt-in WebRTC peer mode that still needs a signalling relay. Offers end-to-end encryption and automatic merge of simple conflicts.
  - Dependency age: not applicable. It is a JS plugin and we run no plugins.
  - **Verdict:** shows that chunked, encrypted, self-hosted sync works for vaults. CouchDB is a heavy server for "easy setup", though, and a plugin host is out of scope. Borrow the ideas of content chunk dedup and a passphrase-derived key.
- **Remotely Save (Obsidian plugin)**: <https://github.com/remotely-save/remotely-save>, algorithm write-up <https://github.com/remotely-save/remotely-save/blob/master/docs/sync_algorithm/v3/intro.md>.
  - Licence: Apache-2.0 for `src/`, which is compatible one way into GPL-3.0. Its `pro/` folder is PolyForm Strict (source-available, not open source) and was not read.
  - Last push was 2024-11, so maintenance is a risk.
  - Backends are S3-compatible stores, WebDAV and consumer clouds. Optional encryption in openssl or rclone-crypt format. Basic conflict detection is free; "smart" conflict handling is in the paid tier.
  - **Verdict:** shows that a dumb blob store is enough for transport. It also shows the weakness: WebDAV and S3 offer no atomic compare-and-swap across many files, so two devices can race, and conflicts end up as "pick one". We need a little more server than this.
- **obsidian-git**: <https://github.com/Vinzent03/obsidian-git>, merge semantics <https://git-scm.com/docs/git-merge-file>.
  - Licence MIT, compatible. Mature (about 12k stars).
  - Uses the git CLI on desktop and a JS git on mobile. Conflicts become `<<<<<<<` markers inside notes.
  - **Verdict:** the right conflict model (three-way merge against a common ancestor) with the wrong user experience. Markers sync into every device and break rendering, setup needs a git host and credentials, and history grows forever. Borrow the base-revision merge, not the UX.
- **Automerge**: <https://github.com/automerge/automerge>, docs <https://automerge.org/docs/hello/>.
  - Licence MIT, compatible. Rust core with JS bindings, actively developed.
  - Crate `automerge` 0.12.0 was published 2026-09-16, which is under 6 weeks old on 2026-10-20 and **not pinnable today** under our rule.
  - **Verdict:** a CRDT per file merges without ever conflicting, but its history has to live next to the `.md` file. Any external edit (git, Syncthing, another editor), which R5 requires us to accept, must be diffed back into CRDT operations anyway. Rejected for v1. See "Conflict resolution".
- **Yjs / y-crdt (yrs)**: <https://github.com/yjs/yjs>, docs <https://docs.yjs.dev/>, Rust port <https://github.com/y-crdt/y-crdt>.
  - Licence MIT, compatible. Mature in JS for real-time collaboration.
  - Crate `yrs` 0.28.0 was published 2026-09-17, also under 6 weeks old and not pinnable today.
  - **Verdict:** built for live multi-cursor editing, which is a non-goal here. Same external-edit problem as Automerge. Rejected for v1. Revisit if real-time collaboration ever becomes a goal.
- **Logseq sync**: <https://github.com/logseq/logseq>, docs <https://docs.logseq.com/>.
  - Client licence AGPL-3.0, which can be combined with GPL-3.0 under section 13, but we copy nothing.
  - The hosted file sync is a paid service with no self-hostable server that we could find. The new DB version ships "RTC" sync, which its README labels alpha with "data loss is possible".
  - **Verdict:** not a model for self-hosting. Its README warning is a useful reminder that sync is where notes apps lose data.
- **Obsidian Sync (behaviour from public docs only)**: <https://help.obsidian.md/Obsidian+Sync/Troubleshoot+Obsidian+Sync>.
  - Proprietary hosted service. Cited, not copied, and no binary inspected (clean room).
  - Its public docs say Markdown conflicts are auto-merged with diff-match-patch, other files use last-modified-wins, and settings JSON is merged with local keys over remote keys. A per-device option creates a conflict file instead.
  - **Verdict:** confirms the split we want: merge text, never merge binaries, and handle settings by key. We improve on fuzzy patching by merging against an exact stored base, and on last-modified-wins by never discarding the losing bytes.
- **Unison**: <https://github.com/bcpierce00/unison>, diff3 theory <https://www.cis.upenn.edu/~bcpierce/papers/diff3-short.pdf>.
  - Licence GPL-3.0, compatible. Decades of use.
  - Bidirectional reconciler that keeps an archive of the last synced state on each side, which is exactly a stored base.
  - **Verdict:** the closest prior art to our model. The base-archive idea is adopted. Unison is OCaml with pairwise (not hub) topology, so it is not embedded.
- **Plain WebDAV / rsync baseline**: <https://www.rfc-editor.org/rfc/rfc4918>, <https://rsync.samba.org/tech_report/>.
  - Open standards and tools. rsync is GPL-3.0; WebDAV servers vary.
  - rsync is one-way, and WebDAV has ETags and locks but no multi-file transaction.
  - **Verdict:** this is the floor. Without a base revision and an ordered log, every bidirectional setup ends in mtime-based last-writer-wins, which breaks R1.

Background reading that shaped the design: Ink and Switch, "Local-first software" <https://www.inkandswitch.com/local-first/>, and the restic repository format <https://restic.readthedocs.io/en/stable/100_references.html> for content-addressed encrypted storage with a server that only stores data.

## Conflict resolution

### Options compared

- **(a) Three-way text merge against a stored base.**
  - For each file, the client keeps the exact bytes both sides last agreed on (the base). Local and remote are each diffed against the base. Hunks that do not overlap are both applied; hunks that overlap are a conflict.
  - Pros: exact, explainable, matches git and Unison. Works on plain files edited by any tool. No extra history format inside the vault. Merge crates are available and old enough (`diffy` 0.5.2, published 2026-08-31, MIT OR Apache-2.0, has a built-in three-way merge; `similar` 3.2.0, 2026-08-17, Apache-2.0, as a fallback).
  - Cons: needs the base stored per file, and overlapping hunks need a policy.
- **(b) CRDT per file (Automerge or yrs).**
  - Pros: never conflicts, and character-level merges of concurrent typing.
  - Cons: every file needs a CRDT history document stored somewhere, either a sidecar in the vault (violates R5 and pollutes git) or a private store that drifts from disk. External edits arrive as whole-file replacements, so we would diff them into CRDT ops anyway: the work of (a) plus the CRDT. Interleaved merges of the same sentence produce text no one wrote, silently. Both crates are under 6 weeks old today.
- **(c) Last-writer-wins plus a conflict copy.**
  - Pros: trivial, works for every file type.
  - Cons: "last" needs a clock or an order, and wall clocks drift (see Syncthing). Two people editing different paragraphs always produce a conflict copy, which trains users to ignore them.

**Decision: (a) for text, (c) for everything that cannot be merged, ordered by the server log instead of clocks.** Concretely:

- Markdown and other text: line-based three-way merge. On overlap, the resolving device's hunk stays in the file and the other side's full version is written as a conflict copy. Conflict markers are never written into a note.
- `.obsidian/*.json`: key-level three-way merge on parsed JSON. Unknown keys round-trip unchanged (an existing vault-safety rule).
- Binary attachments (images, PDFs, anything not valid UTF-8 or over 2 MiB): no merge. The resolving device keeps its bytes at the path; the other bytes become a conflict copy.

### Who resolves, and why it converges

The server log is a single ordered chain per vault. A device pushes a commit with "my parent is seq N". If the head is still N, the server appends; otherwise it answers 409. The device whose push was refused is the **resolver**:

1. It pulls commits N+1 up to the head.
2. It merges its outbox onto them file by file.
3. It pushes again against the new head.

The device that pushed first never merges anything; it only fast-forwards later. Only one device ever resolves a given pair of concurrent changes, so every device applies the same result and R2 holds without a deterministic tie-break across devices. Because the order comes from the server's sequence numbers, **wall clocks are never used for ordering**. Timestamps are display-only (conflict copy names, history). mtime is only a hint to the watcher that something changed.

### What the base is

- Every synced file has a stable **file id** (128-bit random, created by the device that first commits the file). Commits refer to file ids, and the path is an attribute of the file id.
- For each file id, the client stores **base = the blob id of the last revision this device applied or committed**. Base blobs live in a local cache outside the vault (`$XDG_DATA_HOME/opensidian/sync/<vault-id>/base/`). If the cache misses, the base is fetched from the server, which still holds every blob referenced inside the retention window.
- Concretely, local and remote changes are diffed against the same revision: the one both sides last saw at seq N.

### Detecting renames, moves and deletes

- **In-app renames and moves** (the rename command, the file tree, drag and drop) are known exactly. The app calls the sync engine with `(file id, old path, new path)` before the filesystem rename, so identity is never guessed.
- **External renames** (another tool, git checkout) come through `watcher.rs`. The watcher already pairs a removed and an added path only when the pairing is unambiguous and the bytes are identical. The sync engine adds one more step: a removed path whose last base blob id equals the added path's content hash is the same file id. Anything ambiguous stays delete plus create. Content-addressed blobs mean the bytes are never uploaded twice, so the only cost of a missed rename is a lost identity (history restarts), never lost data.
- **Folders** are not objects. A folder exists while a file lives in it, and a folder rename is many file renames in one commit.
- **Deletes** are explicit `delete(file id, base blob)` operations. A delete records which revision it deleted, which is what makes delete-vs-edit decidable.
- **Partial walks.** The watcher's `heal_short_walk` re-stats every claimed removal before reporting it. The sync engine adds a mass-delete guard: a single local change set that deletes more than 50 files, or more than 20 percent of the vault, is held and the user is asked to confirm. A remounting filesystem must never turn into "delete everything on every device".

### Edge cases, decided

- **Delete vs edit:** the edit wins. The file is restored at its latest path on the deleting device, and the sync log notes "restored: edited on <device> after you deleted it". Losing an edit breaks R1; an unwanted file is one click to delete again.
- **Edit vs rename:** both apply, because the edit targets the file id and the rename changes the path attribute.
- **Simultaneous rename of one file:** the rename that reaches the server first wins. The resolver drops its own rename and shows "<file> was renamed to <path> on <device>". A file cannot live at two paths, and the content is unaffected.
- **Two files claiming one path** (both devices create `Inbox.md`, or one renames onto a name the other created): the committed file keeps the path. The resolver's file moves to the conflict-copy name with a new path, keeping its own file id.
- **Binary files:** never merged. Identical bytes on both sides are not a conflict. Different bytes mean the resolver keeps its own bytes and the other side's bytes are written as a conflict copy.
- **`.obsidian/` config:** per-device files are excluded by default: `workspace.json`, `workspace-mobile.json`, `cache`, `themes/` (re-seeded by builtins), and anything matching user patterns in `.opensidian-sync-ignore`. Other JSON gets a key-level three-way merge. When both sides change the same key, the resolver's value wins and the other value is written to the sync log, not to a conflict copy, because a stray JSON file in `.obsidian/` is noise. Invalid JSON falls back to binary rules.
- **Frontmatter:** the leading `---` block is merged separately from the body. If it parses as flat `key: value` lines (the common case), it is merged by key like JSON. A list value changed on both sides becomes the union of items in base order, then local additions, then remote additions. Same key with different scalar values means the resolver wins and the other value is saved in the conflict copy. Anything else falls back to line merge for the whole file. YAML parsing reuses whatever the properties code already uses; no new YAML crate.
- **Clock skew:** irrelevant to correctness, because order is server sequence numbers. A device whose clock is hours off still merges correctly; only the date shown in its conflict copy names is off.
- **Line endings and encodings:** merge works on bytes split at `\n`. A file that is not valid UTF-8 is binary. CRLF files keep CRLF; a pure line-ending flip is a whole-file change, which we accept.
- **Conflict copy name:** `<stem> (conflict <device-name> <YYYY-MM-DD HHmm>).<ext>` in the same folder. The name is our own, chosen to sort next to the original and to read as plain language in the file tree. Conflict copies sync like normal files, so every device sees them.

### Worked conflict scenarios

All scenarios use two devices, A (laptop) and B (desktop), with base revision r0 at server seq 10. "A first" means A's push reached the server first, so B is the resolver.

### Scenario 1: different paragraphs of one note

- Setup: `Plan.md` has paragraphs P1, P2 and P3. Offline, A edits P1 and B edits P3. A first.
- Process: B pulls seq 11, three-way merges r0, A's version and B's version. The hunks do not overlap.
- Expected outcome: `Plan.md` on both devices holds A's P1, unchanged P2 and B's P3. No conflict copy, no banner. Seq 12 is B's merge, and A fast-forwards to it.

### Scenario 2: same line edited on both devices

- Setup: line 4 of `Plan.md` is `Ship: Friday`. A changes it to `Ship: Monday`, B changes it to `Ship: Tuesday`. B also adds a new line 9. A first.
- Process: B merges. The line-9 hunk applies cleanly; the line-4 hunks overlap.
- Expected outcome: `Plan.md` keeps B's `Ship: Tuesday` and gains A's other non-overlapping changes plus B's line 9. A file `Plan (conflict laptop 2026-10-20 1412).md` holds A's full version. Both devices see the conflict copy and a banner with a "Compare" action. No markers are written into `Plan.md`.

### Scenario 3: delete on one device, edit on the other

- Setup: A deletes `Old idea.md` (delete op records base r0). B, offline, appends a paragraph to it. B first.
- Process: A is the resolver. Its delete names base r0, but the head revision of that file id is newer (B's edit).
- Expected outcome: the delete is dropped. `Old idea.md` with B's paragraph is restored on A, and A's sync log says it was restored because it was edited on desktop. If A had pushed first instead, B as resolver would see a delete of r0 against its own edit of r0 and re-create the file with B's content: the same final state.

### Scenario 4: rename on one device, edit on the other

- Setup: A renames `Draft.md` to `Essays/Final.md` with the in-app rename. B edits `Draft.md`. A first.
- Process: B pulls a rename of file id F. Its outbox holds an edit of F. The edit is merged against r0 and applied at F's new path.
- Expected outcome: `Essays/Final.md` contains B's edits on both devices, and `Draft.md` exists nowhere. On B, the open tab follows the rename and keeps the caret.

### Scenario 5: both devices rename the same file

- Setup: A renames `Untitled.md` to `Reading list.md`; B renames it to `Books.md`. A first.
- Process: B as resolver finds a rename of F already committed and drops its own rename.
- Expected outcome: the file is `Reading list.md` on both devices, with no duplicate. B shows the notice "Untitled.md was renamed to Reading list.md on laptop". Content edits made on either side are merged as in scenario 1.

### Scenario 6: both devices create the same new path

- Setup: A and B each create `Inbox.md` offline with different content. There is no base. A first.
- Process: B sees path `Inbox.md` owned by a different file id. There is no common ancestor, so no merge is attempted.
- Expected outcome: `Inbox.md` holds A's content everywhere. B's file becomes `Inbox (conflict desktop 2026-10-20 0915).md`, keeping B's file id. If the two contents were byte-identical, B adopts A's file id and nothing is created.

### Scenario 7: attachment replaced on both devices

- Setup: `img/diagram.png` is replaced by A with export v2 and by B with export v3. A first.
- Process: binary, no merge.
- Expected outcome: B is the resolver, so `img/diagram.png` holds B's v3 bytes on both devices and `img/diagram (conflict laptop 2026-10-20 1030).png` holds A's v2. Embeds keep pointing at the original path, which now shows v3.

### Scenario 8: settings changed on both devices

- Setup: A changes `.obsidian/app.json` key `vimMode` to true. B changes `showLineNumber` to true and `vimMode` to false. A first.
- Process: key-level three-way merge on parsed JSON.
- Expected outcome: `showLineNumber` is true. `vimMode` takes B's false, because B resolves. A's value `vimMode: true` is recorded in the sync log as overridden. Key order and unknown keys round-trip unchanged. `workspace.json` is never synced, so pane layouts stay per device.

### Scenario 9: tags added in frontmatter on both devices

- Setup: frontmatter `tags: [work]`. A adds `urgent`, B adds `q4`. B first.
- Process: A resolves. Flat frontmatter, same list key changed on both sides, so take the union in order.
- Expected outcome: `tags: [work, q4, urgent]` on both devices (base, then the committed head's additions, then the resolver's additions). Body changes are merged as usual. No conflict copy.

### Scenario 10: one device's clock is three hours behind

- Setup: B's clock reads 09:00 while the real time is 12:00. A edits `Log.md` at 12:00 and pushes. B edits a different line at real 12:05 (shown as 09:05) and pushes second.
- Process: ordering is seq 11 (A), then seq 12 (B), regardless of timestamps. An mtime-ordered design would have treated B's newer edit as older.
- Expected outcome: both edits are present on both devices. Only the timestamps in B's history entries are wrong. No data decision depends on them.

### Scenario 11: folder deleted on one device, file added inside it on the other

- Setup: A deletes folder `Archive/` (three files, each delete with its base). B adds `Archive/New.md`. A first.
- Process: B applies the three deletes, none of which touches its new file id, then pushes the create.
- Expected outcome: `Archive/` exists on both devices and contains only `New.md`.

## Self-hosted server

### Smart enough, not smarter

- A dumb blob store (WebDAV, S3) cannot do an atomic "append if head is still N" across a multi-file change. Remotely Save documents the races this causes.
- A smart server that understands notes would need the plaintext, which breaks end-to-end encryption.
- So the server does exactly three things:
  1. Stores opaque encrypted blobs by id.
  2. Keeps one append-only log per vault with compare-and-swap on the head.
  3. Checks that a request is signed by an enrolled device.

### Shape

- One Rust binary, `opensidian-syncd`, in a new `sync-server/` crate in the same repository, under GPL-3.0-or-later. It is built static (musl target) for a single-file download, and as a small OCI image (`FROM scratch` plus the binary) for Docker or Podman users.
- HTTPS on one port (default 8443). Plain HTTP only with `--insecure-http` for use behind a reverse proxy.
- TLS without a CA. On first start the server makes a self-signed certificate and prints its SPKI SHA-256 fingerprint. The pairing payload carries that fingerprint, and clients pin it. Users who prefer Caddy, Traefik or Tailscale certificates pass `--tls-cert/--tls-key` and turn pinning off.
- Dependencies: prefer what is already in `Cargo.lock` (tokio 1.53.1, hyper 1.11.1, sha2 0.10.9, serde_json). TLS needs one new crate family; which one, and its age, is an open question resolved in phase 2.

### API (version 1)

- `POST /v1/enroll`: one-time enrollment token plus device public key returns a device id.
- `GET /v1/vaults/{vault}/log?after={seq}&wait=30`: long-poll for new commit envelopes.
- `POST /v1/vaults/{vault}/log`: body `{parent_seq, parent_hash, envelope}`. Returns 201 with the new seq, or 409 with the current head.
- `PUT /v1/vaults/{vault}/blobs/{id}` and `GET` on the same path. PUT is idempotent; an existing id is a no-op.
- `POST /v1/vaults/{vault}/gc`: list of blob ids that the clients have agreed to drop (see backups and GC).
- `POST/GET /v1/pair/{mailbox}`: a short-lived relay for the pairing handshake. Messages are opaque and expire after 10 minutes.
- Every request carries `Authorization: OSync device=<id>, ts=<unix>, nonce=<rand>, sig=<ed25519 over method, path, body-hash, ts, nonce>`. The server keeps nonces for 5 minutes and rejects any request outside a 5-minute clock window. This window is used only for auth freshness, never for ordering data.

### Storage format

Plain files under `--data` (default `/var/lib/opensidian-syncd`), so standard tools can back it up:

- `server.toml`: config, written once at init.
- `tls/`: the self-signed certificate and key, mode 0600.
- `vaults/<vault-id>/devices.json`: device id, public key, display name (ciphertext), enrolled-at, revoked flag.
- `vaults/<vault-id>/log/<seq>.env`: one immutable file per commit envelope, written via temp file, fsync and rename.
- `vaults/<vault-id>/HEAD`: `seq hash`, replaced atomically under a per-vault mutex. Compare-and-swap is this mutex plus the check.
- `vaults/<vault-id>/blobs/<id[0..2]>/<id>`: immutable encrypted blobs.

No database is needed. A crash leaves at most an orphan temp file or an unreferenced blob, both harmless.

### Multiple devices and vaults

- One server hosts many vaults. A vault has many devices, and each device key is enrolled per vault.
- Revoking a device marks it revoked in `devices.json`. Its signatures stop being accepted, and the remaining devices rotate the vault key (see the threat model).
- Optional per-vault quota (`--quota 10G`) and per-device rate limits.

### Backups and garbage collection

- Backups: stop nothing. Copy or `restic backup` the data directory. Files are immutable apart from `HEAD`, and a backup taken mid-write at worst lags one commit behind. Restoring an old backup is, to clients, a rollback, and they will refuse it loudly (see threat model) until the user confirms "server restored from backup" on one device. That device then re-pushes what the server lost.
- Version history: commits older than 30 days (configurable) are folded into a checkpoint commit that holds the full encrypted manifest. New devices start at the latest checkpoint instead of replaying the whole log.
- GC: the server cannot read manifests, so it cannot know which blobs are live. A client with full history computes "blobs referenced by nothing since the oldest retained checkpoint" and sends that list. The server deletes them only if the list is signed by a device and older than a grace period of 7 days.

## Easy setup

### From zero to two synced devices

1. **Start the server.** On any box reachable by both devices, run `opensidian-syncd init --data /srv/osync && opensidian-syncd serve --data /srv/osync`, or the one-line `docker run`. It prints the server URL, the certificate fingerprint and a one-time **setup link** shown as text and as a terminal QR code.
2. **Connect the first device.** In Opensidian on device A: Settings, then Sync, then "Set up sync". Paste the setup link (or scan it on a device with a camera). The app enrolls, generates the vault key and the device key, stores both in the OS keychain, and uploads the vault. A progress line shows files and bytes.
3. **Add a device.** On device A, click "Add device". The app shows a QR code and the same thing as a short code, for example `4-orbit-lantern-sable`, valid for 10 minutes and for one use.
4. **Join on the second device.** On device B: Settings, then Sync, then "Join a synced vault". Scan the QR code or type the short code, then choose an empty folder (or an existing one, which gets a merge preview first).
5. **Done.** Both devices show "Synced" in the status bar. Device names default to the hostname and can be renamed later.

What the user types in total: one shell command, one paste, one short code. No passwords, no port mapping beyond the one HTTPS port, no certificate authority.

### Pairing, concretely

- The short code is `<mailbox>-<word>-<word>-<word>`. The words come from a 2048-word list, giving 33 bits of secret. Both devices run SPAKE2 (RFC 9382, <https://www.rfc-editor.org/rfc/rfc9382>; crate `spake2` 0.4.0, published 2026-01-25, MIT OR Apache-2.0) over the server mailbox. This is the magic-wormhole pattern (<https://magic-wormhole.readthedocs.io/>).
- An online guess gets one try per code, because the mailbox dies after one failed exchange. An offline guess gets nothing, because the PAKE transcript does not reveal the code.
- Over the resulting channel, A sends B the server URL, the certificate fingerprint, the vault id, a one-time enrollment token and the vault key. The QR code carries the same short code plus the URL and fingerprint, so scanning skips typing.

### Failure messages (plain language, each with one next step)

- "Can't reach the sync server at <url>. Check that it is running and that this device can open that address." (connect or timeout)
- "The server's certificate changed. If you didn't reinstall the server, stop and check — someone may be in the middle." (pin mismatch; no "continue anyway" button on first pairing)
- "That code expired or was already used. Make a new one on the other device." (mailbox gone)
- "The code didn't match. Check the words and make a new code." (PAKE failure; the code is now burned)
- "This device was removed from the vault. Ask another device to add it again." (revoked)
- "The server is missing changes this device has already seen. Sync is paused to protect your notes." (rollback detected; see threat model)
- "About to delete 214 files on all devices. Was that you?" (mass-delete guard; buttons "Yes, sync deletions" and "No, restore them")
- "Server storage is full." (quota or disk full; sync pauses and local edits keep queueing)

### Mobile considerations

Opensidian is Linux-desktop-first, and mobile is not part of this plan. The design keeps it possible:

- Pairing by QR code is the mobile path.
- The client engine is plain Rust behind a small interface, so a Tauri mobile build can reuse it.
- Background execution on iOS and Android is limited, so mobile syncs on app foreground and on save, with long-poll only while open. The server design needs no push service.
- Large attachments may get a "download on demand" flag later. The manifest already separates metadata from blobs.

## Security and threat model

### Assets and actors

- Assets: note contents, attachment contents, filenames and folder structure, settings, and the vault key.
- Actors:
  - an honest-but-curious server operator, which could be a VPS host;
  - an actively malicious server, or someone who took over the server;
  - a network attacker;
  - a lost or stolen device;
  - a malicious file arriving through sync from a compromised device.

### End-to-end encryption

- **Vault key** `Kv`: 256 bits from the OS RNG, created once by the first device. Subkeys come from HKDF-SHA256 (`hkdf`, with `sha2` already in the lock): `Kblob` for blob encryption, `Kid` for blob ids, `Kman` for commit envelopes.
- **Blobs**: XChaCha20-Poly1305 (`chacha20poly1305` 0.11.0, published 2026-06-28, Apache-2.0 OR MIT; construction per <https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-xchacha>) with a random 192-bit nonce per blob. Plaintext is padded to the next bucket in a coarse size ladder, which hides exact sizes.
- **Blob id**: keyed BLAKE3 of the plaintext under `Kid` (`blake3` 1.8.7, published 2026-08-20). This gives deduplication within a vault without letting the server confirm "this vault contains this known file", which an unkeyed hash would allow.
- **Commit envelope**: the ops (paths, file ids, blob ids, sizes, device name) are serialized and sealed under `Kman`. The server sees only `seq`, `parent_hash`, device id, ciphertext length and the signature.
- **Result**: the server sees ciphertext only. Filenames, folder names, the number of notes in a folder, and content are all hidden.

### What still leaks (metadata)

- The number of blobs and their padded sizes.
- Commit count and timing, which reveal when you write.
- Which device id wrote each commit.
- The total vault size.
- Client IP addresses.

This is accepted and documented in the setup screen's "What the server can see" link. Padding plus whole-file blobs means editing one note reveals "one blob of about this size class changed", not which note.

### Device keys and signatures

- Each device has an Ed25519 key pair (`ed25519-dalek` 3.0.0, published 2026-07-06, BSD-3-Clause). It signs every request and every commit envelope.
- Commits form a hash chain: `hash(n) = BLAKE3(hash(n-1) || envelope(n))`. The envelope's plaintext also carries the author's signature and the author's view of other devices' latest seq.

### Rollback, fork and replay by a malicious server

- **Replay:** a replayed request fails the nonce or timestamp check. A replayed commit cannot be appended twice, because `parent_hash` must match the head.
- **Rollback:** each client stores the highest `(seq, hash)` it has verified, in its sync state, which is integrity-protected by an HMAC under a key in the keychain. A head with a lower seq, or a different hash at a known seq, is a rollback or a fork. Sync stops with the "server is missing changes" message, and nothing is applied or deleted locally.
- **Fork** (showing A and B different histories): each commit includes the author's last-seen seq and hash for every other device. When B eventually sees a commit from A that references a chain B never received, the fork is detected.
- **Freeze** (withholding new commits forever) cannot be prevented by any client-server design. It is made visible: the status bar shows "last update from <device>: 3 days ago" when a peer device has gone quiet.
- **Tampering:** a modified blob or envelope fails AEAD decryption or signature checks and is rejected. Since blob ids are keyed hashes of the plaintext, a correct decryption with a wrong id is also rejected.

### Keys at rest and recovery

- `Kv`, the device private key and the state-HMAC key live in the OS keychain through the Secret Service API (`keyring` 4.2.0, published 2026-08-29, MIT OR Apache-2.0). This fills the existing "Keychain" settings tab. Nothing secret is written to `~/.opensidian.json` or into the vault.
- If no keychain is available (headless or minimal desktop), sync refuses to store keys in a file. It asks for a passphrase each launch and derives a wrapping key with Argon2id (`argon2` 0.6.0, published 2026-08-27; parameters per RFC 9106, <https://www.rfc-editor.org/rfc/rfc9106>).
- Recovery: at setup, the user may print or save a **recovery key** (`Kv` encoded as 24 words). Without a recovery key or a live device, a lost vault key means the server copy is unrecoverable. The plaintext vault on every device is unaffected.
- A lost or stolen device: revoke it from another device. Revocation rotates `Kv`. New blobs and envelopes use the new key, and old blobs stay readable by the old key, which the stolen device already had. The UI says so honestly: revocation protects future changes, not history.

### Fit with the trusted-vault model

- `vaultfs.rs` treats the vault root as trusted and everything below it as untrusted. Sync-delivered files are untrusted input of the same kind as a `git pull`.
- Every incoming path is validated before it is used:
  - relative, UTF-8, no `..`, no NUL, no leading `/`;
  - no component named `.git`;
  - not on the per-device exclude list;
  - under a length cap;
  - no case-insensitive duplicate on case-insensitive filesystems.
- Incoming files are then written only through `vaultfs` (`openat` with `O_NOFOLLOW`, random temp name, `renameat`). Sync never creates symlinks; a symlink in the vault is synced as neither a link nor its target, and is reported.
- Rendering stays as it is: synced Markdown passes through the same sanitizer as any `.md`. Synced themes and snippets are files like any other, so the existing themefs refusal rules apply.
- The Landlock sandbox (`OPENSIDIAN_LANDLOCK=1`) gets one more allowed path, the sync state directory, plus a network rule if Landlock's network scoping is on.

## Architecture inside Opensidian

### Where code lives

- New module tree `src-tauri/src/sync/`:
  - `model.rs`: file ids, ops, manifest, envelope.
  - `merge.rs`: three-way text merge, JSON key merge, frontmatter merge, conflict copy naming.
  - `crypto.rs`: keys, AEAD, blob ids, signatures, chain.
  - `client.rs`: HTTP client, built on the reqwest/hyper already in the lock.
  - `engine.rs`: state machine, outbox, apply.
  - `pair.rs`: SPAKE2 mailbox flow.
  - `store.rs`: local state on disk.
- `merge.rs` and `model.rs` are pure functions with no I/O, which makes the scenario tests plain unit tests.
- New crate `sync-server/` for `opensidian-syncd`, sharing `model.rs` and `crypto.rs` through a small `sync-proto/` library crate so the wire format cannot drift.
- UI: a Sync settings pane, a status-bar item, a conflict banner with "Compare" (a two-pane read-only diff view), and a sync log view. Plain JS in `ui/main.js`, no framework.

### Engine and the watcher

- The engine runs on its own thread in the vault process. `vaultlock.rs` already guarantees one backend per vault, so there is exactly one sync engine per vault per machine.
- Local changes: `watcher.rs` already emits a per-tick `Change { added, removed, modified, renamed }` after reconciling against the index. The engine subscribes to the same `Change` (a channel next to the `vault-changed` event). In-app operations (write_note, rename, delete, attachment import) call the engine directly with exact intent, so it does not have to infer them.
- Echo suppression: when the engine writes a remote change, it upserts the index under the index lock exactly as `write_note` does. The watcher's reconcile then sees bytes equal to the index and emits nothing, so remote writes never bounce back as local edits.
- Debounce: local changes are gathered for 2 seconds after the last keystroke-save (the editor saves on a 250 ms debounce), then become one commit. Typing does not produce a commit per save.

### Offline queue

- `outbox.jsonl` in the sync state directory (`$XDG_DATA_HOME/opensidian/sync/<vault-id>/`). It is append-only, fsynced, and holds one record per pending op with its base blob id. Blob contents are snapshotted into the local blob cache at queue time, so later edits cannot change what an op means.
- On reconnect, the engine pulls, rebases the outbox onto the new head (merging per file), pushes with compare-and-swap, and repeats on 409. The outbox is truncated only after a 201.
- A crash at any point restarts from the outbox plus the last verified head. Applying a commit is idempotent: a file already at the target blob is skipped.

### A remote change under an open file

- The editor is the R17 model in `ui/editor.js`, not CodeMirror. The existing R11 machinery already handles external edits under an open tab, and sync reuses it:
  1. Before applying a remote change to path P, the engine emits `sync-before-apply {path}`. If P is open in a dirty tab, the UI flushes it through `saveBuf` (the single write seam) and acknowledges. The timeout is 2 seconds; if it fires, the engine defers P to the next round.
  2. The engine three-way merges (base, disk = ours, remote = theirs) and writes the result through vaultfs.
  3. The watcher path delivers the change. A clean tab gets `reloadInPlace`, which keeps caret line, column and scroll. A tab that became dirty again in the meantime goes through R11.3 at the next save, which today only folds external appends. That rule is extended to a real three-way merge with `t.base` as the base, so typing during a sync round is never lost.
- Caret mapping: instead of keeping the raw line and column, the caret is mapped through the merge's hunks, using the same range-mapping logic `editor.js` already uses for edits. Text inserted above the caret no longer shifts the user's place.
- Remote rename of an open file: the engine emits an explicit rename, so the tab retitles and keeps its buffer instead of the current "external-rename" drop-and-park path.
- Remote delete of an open file: the existing R11.4 path applies (tab dropped, unsaved bytes parked in undo-close). If the tab was dirty, scenario 3's rule already re-created the file through the edit-wins policy before the delete was applied.

### Index, search and graph

These need no change. Remote writes go through the same index upsert as local writes.

## Implementation plan

Six phases, each sized as one goal with its own worktree. Line counts are rough estimates of Rust plus JS, excluding tests. Every phase ends with acceptance checks a reviewer runs. Two-instance checks run on the build box like every other gate.

1. **Phase 1: sync-proto and merge core (no network, no UI).**
   - Scope: `sync-proto/` crate with model, ops, manifest, path validation, conflict-copy naming, and `merge.rs` (line three-way merge via `diffy` pinned with `=`, JSON key merge preserving order with serde_json `preserve_order`, flat-frontmatter merge, binary detection). About 1,500 LOC.
   - Acceptance check: `cargo test --locked -p sync-proto` passes, and includes one test per worked scenario in this RFC (1 to 11, named `scenario_01` to `scenario_11`) asserting the stated expected outcome. A property test merges random non-overlapping edits and asserts that both edits survive.
2. **Phase 2: opensidian-syncd server.**
   - Scope: storage layout, compare-and-swap log, blobs, enrollment tokens, signed-request auth with nonce cache, long-poll, quota, `init` and `serve`, static musl build, OCI image. Pick the TLS crate (age-checked and pinned). About 1,400 LOC.
   - Acceptance check: `cargo test --locked -p opensidian-syncd` passes, including concurrent pushes against one parent (exactly one 201, the rest 409), crash-safety (an aborted write leaves HEAD valid) and restart persistence. On the box, `file target/x86_64-unknown-linux-musl/release/opensidian-syncd` reports a static binary, and the container answers `GET /v1/health` with 200.
3. **Phase 3: crypto, keys and pairing.**
   - Scope: vault key and HKDF subkeys, XChaCha20-Poly1305 blobs and envelopes, keyed BLAKE3 blob ids, size padding, Ed25519 device keys, hash chain plus rollback and fork detection, keychain storage with the Argon2id fallback, SPAKE2 pairing over the mailbox, recovery key. About 1,100 LOC.
   - Acceptance check: `cargo test --locked` covers seal and open round-trip, tamper rejection, and rollback detection (the test restores an older server data directory and asserts the "server is missing changes" error). After a test sync, `grep -rl 'MARKER-7f3a' <server-data-dir>` finds nothing, where `MARKER-7f3a` is a string planted in a note and a filename. A pairing test with a wrong code fails and burns the mailbox.
4. **Phase 4: client engine in src-tauri (headless).**
   - Scope: `sync/engine.rs` with watcher subscription, in-app intent hooks, outbox, pull, rebase and push loop, apply via vaultfs, echo suppression, mass-delete guard, exclude list, path validation on ingress. A headless test mode (`--sync-only`) for two-instance tests. About 1,600 LOC.
   - Acceptance check: on the box, a two-instance sync smoke runs: one server and two headless instances with separate vault copies. It drives scripted edits that reproduce scenarios 1 to 11 through the filesystem and asserts both vault trees hash identically afterwards (`find vault -type f -not -path '*/.obsidian/workspace*' | sort | xargs sha256sum` equal on both). A crash-recovery test (the harness stops one instance mid-push via its own child handle, never pkill) shows that no op is lost.
5. **Phase 5: UI, setup flow and open-file behaviour.**
   - Scope: Sync settings pane and Keychain tab content, the five-step setup flow, QR rendering (our own small encoder or an age-checked pinned crate), status-bar item, conflict banner and Compare view, sync log, failure messages from the setup section, `sync-before-apply` and caret mapping in the editor, explicit rename events. About 1,200 LOC.
   - Acceptance check: a new gate phase `syncsmoke` on two displays. It pairs A and B through the UI with xdotool, types a line in A, and requires it in B's DOM within 5 seconds with B's caret line unchanged. A conflicting edit while offline must produce the conflict copy plus banner. Census tokens and screenshots are pulled into the artifacts directory. The existing T1 typing phases stay green.
6. **Phase 6: history, GC, backups and hardening.**
   - Scope: checkpoints and retention, version-history view with restore, client-driven GC with grace period, rate limits, the "restored from backup" recovery flow, device revocation with key rotation, the operator doc for running and backing up the server. About 900 LOC.
   - Acceptance check: tests show GC never deletes a blob referenced at or after the oldest retained checkpoint. A restic backup-and-restore of the server data directory is followed by the recovery flow, after which both instances converge. After revocation, the revoked device's signed requests get 403 and new blobs do not decrypt under the old key.

Total is about 7,700 LOC across six goals. Phases 1 to 3 have no UI and can run in parallel after phase 1's crate exists. Phase 4 needs 1 to 3; phase 5 needs 4; phase 6 needs 4.

## Open questions and non-goals

### Open questions

- **TLS crate.** `Cargo.lock` has no TLS stack today. The choice between rustls with an age-checked pinned crypto provider and the system OpenSSL through native-tls affects static linking and supply chain. Decide in phase 2 with a written dependency review.
- **Chunking.** Whole-file blobs are simple and leak less. Chunking (as LiveSync and Syncthing do) saves bandwidth for large attachments that change in place, which is rare in vaults. Measure on real vaults before deciding.
- **Merge granularity.** Line merge treats two edits to one long paragraph line as a conflict. A word-level second pass inside conflicting hunks would merge more, at the risk of surprising text. Ship line-level first and collect conflict-copy rates.
- **Conflict copy policy for settings.** Is the sync log enough for a lost `.obsidian` key, or do users want a visible notice?
- **Sync while the app is closed.** A user-level systemd service running `opensidian --sync-only` would keep devices fresh. That needs care with the one-backend-per-vault lock when the app starts.
- **Multiple users.** Shared vaults between people need per-user keys and access control. The design leaves room (devices are per vault), but sharing UX is not designed.
- **Dependency timing.** `automerge` and `yrs` were too young to pin on 2026-10-20. If a later goal wants character-level merge, re-evaluate them then, as a pass inside conflicting hunks rather than as the storage model.

### Non-goals

- Real-time collaborative editing with live cursors.
- A hosted service run by the project.
- Syncing through third-party storage (S3, WebDAV, Dropbox) in v1. The blob layer is abstract enough to add an S3 backend later, but compare-and-swap would still need our server.
- Syncing plugins. Opensidian runs none.
- Selective per-folder sync in v1. Only the exclude list is supported.
- Mobile apps. The design keeps them possible but does not build them.
- Compatibility with Obsidian Sync's protocol or servers. Clean room, and none is documented.
- Replacing git or Syncthing for people who already use them. The watcher keeps supporting both.

## References

- Syncthing BEP v1: <https://docs.syncthing.net/specs/bep-v1.html>
- Syncthing ignore patterns (model for `.opensidian-sync-ignore`): <https://docs.syncthing.net/users/ignoring.html>
- LiveSync: <https://github.com/vrtmrz/obsidian-livesync>
- Remotely Save sync algorithm v3: <https://github.com/remotely-save/remotely-save/blob/master/docs/sync_algorithm/v3/intro.md>
- git merge-file: <https://git-scm.com/docs/git-merge-file>
- Khanna, Kunal, Pierce, "A Formal Investigation of Diff3": <https://www.cis.upenn.edu/~bcpierce/papers/diff3-short.pdf>
- Automerge: <https://automerge.org/docs/hello/>
- Yjs: <https://docs.yjs.dev/>
- Local-first software: <https://www.inkandswitch.com/local-first/>
- restic repository format: <https://restic.readthedocs.io/en/stable/100_references.html>
- age/rage (file encryption design reference): <https://github.com/str4d/rage>
- RFC 9106 Argon2: <https://www.rfc-editor.org/rfc/rfc9106>
- RFC 9382 SPAKE2: <https://www.rfc-editor.org/rfc/rfc9382>
- XChaCha20-Poly1305 draft: <https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-xchacha>
- Magic wormhole: <https://magic-wormhole.readthedocs.io/>
- WebDAV RFC 4918: <https://www.rfc-editor.org/rfc/rfc4918>
- rsync technical report: <https://rsync.samba.org/tech_report/>
