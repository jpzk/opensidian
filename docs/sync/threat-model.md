# Opensidian Sync — threat model (M0, draft v1)

Status: draft for review. Binding source: the sync plan (Principles,
section 0) — "never silently lose bytes"; "E2EE: client trusted,
everything else hostile"; "server = dumb write-once store, minimal
metadata". Clean room: written from first principles and the PUBLIC stock
Sync help pages only (no Obsidian code was read).

## 1. Assets

- A1 note/attachment plaintext (content, file names, folder structure)
- A2 history: every earlier version (file history, M1) — same sensitivity
  as A1, plus things the user deleted ON PURPOSE (secrets → purge)
- A3 key material: backup words S, root R and its children, vault key VK
  keyring, device signing keys
- A4 metadata: object count, sizes, timing, device count, vault count,
  client IP
- A5 integrity of the vault: no silent loss, no silent rollback, no
  injected content
- A6 availability: being able to read your own notes offline, always

## 2. Trust boundaries

Trusted: the client device while unlocked (opensidian app process,
opensidian-syncd running as the same user), the FIPS-mode AWS-LC crypto
module inside it (pinned 4.2.0: no CMVP certificate yet, in process —
cmvp.md §7), the OS keystore.

Untrusted / hostile:

- T1 sync-server process and its operator (including a self-host admin
  who is not the vault owner), its disk, its backups
- T2 the network path (ISP, Wi-Fi, captive portals, TLS-MITM proxies)
- T3 replicas (fs/NAS/USB targets) — storage only, same as T1
- T4 other local users on the same machine (other uids)
- T5 plugins and themes loaded into the app (see R1 below — partially
  trusted today, MUST be untrusted for sync/history commands)
- T6 a stolen/lost device that is locked (disk at rest)
- T7 a former device after revocation

## 3. Adversaries and what they must NOT achieve

- server (T1): MUST NOT read A1/A2/A3; MUST NOT forge or modify objects
  undetected (AEAD + device signature); MUST NOT link objects to files
  (random 128-bit ids, Padme padding, chunking to uniform 4 MiB). It CAN:
  drop, withhold, replay old cursor state (rollback/freeze — detectable
  only across replicas, R3), observe A4.
- network (T2): same as T1 plus nothing more — TLS is defence in depth,
  confidentiality never relies on it. Never an "ignore certificate" knob.
- local other uid (T4): MUST NOT reach syncd (unix socket 0700 in
  XDG_RUNTIME_DIR + SO_PEERCRED uid check, never TCP), MUST NOT read the
  history store (app data dir 0700, files 0600).
- plugin (T5): MUST NOT call sync/history IPC to exfiltrate plaintext or
  key material; requires the app-command ACL (plan risk "pentest ROOT")
  to land BEFORE sync ships. M1 history commands are exposed only to the
  core UI and are listed in the ACL work.
- stolen locked device (T6): limited to OS full-disk encryption + keystore;
  the history store is plaintext-at-rest like the vault itself (stock
  parity: "Obsidian doesn't encrypt your local vault"). Open question:
  encrypt the local history store at rest.
- revoked device (T7): gets no new VK epoch; old content it already had is
  out of scope (cannot un-know). If it held S (words, or a pairing offer
  that carried them — the default), it can still open the new envelope:
  phrase rotation on revoke is out of suite v1 (R6, spec §4.6 notes; the
  revoke reply warns, cut-off = new vault with new words).
- destroy abuse: nobody without K_destroy can delete a vault on a server;
  soft DELETE is bounded by a server-enforced retention window, so a
  stolen write token cannot erase history immediately. M3b: the destroy
  challenge is single use (burned on a failed proof), bound to ns + server
  nonce + ts (±300 s), tested against wrong key / nonce / ts / vault
  (`crates/sync-server/tests/api.rs` `destroy_*`). A malicious server can
  of course delete anyway (A1); the signed receipt only proves an honest
  server's acknowledgement, under the key pinned at enrollment.

## 4. Data-loss threats (A5/A6) — the "never silently lose bytes" rule

- D1 own save overwrites content → snapshot current bytes first (M1)
- D2 external editor / git checkout / script overwrites → watcher captures
  the pre-image it last saw; window = one watcher tick (documented gap:
  bytes written AND overwritten within one tick by a foreign process are
  not seen by anyone)
- D3 delete (own or external) → snapshot + tombstone, "restore deleted"
- D4 sync write (M4) → same invariant, the losing side of a merge is
  ALWAYS in history and logged
- D5 merge garbling (DMP) and LWW clock skew → loser kept (D4)
- D6 history store itself: write-temp + fsync + rename, content addressed
  (sha256 of bytes = name), verify on read; a corrupted blob is reported,
  never served
- D7 retention: thinning never removes the newest version or a tombstoned
  file's last version before its retention ends; purge is the ONLY explicit
  destructive op and is user-initiated

## 5. Out of scope (v1)

- compromised client while unlocked (malware as the same uid)
- traffic analysis beyond padding/chunking (timing of syncs is visible)
- server availability guarantees (use several replicas)
- Android FIPS (deferred, plan section 4)
- post-quantum signatures (confidentiality is PQ-safe by design: no
  public-key encryption is ever stored on the server)

## 6. Residual risks (tracked)

- R1 no app-command ACL in tauri → CLOSED for sync/history by the webview-label gate (spec §7.1): those commands answer only the `sync` window; other app commands are unchanged (out of scope)
- R2 LWW trusts device clocks (mitigated: loser kept)
- R3 server freeze/rollback only detectable across replicas
- R4 FIPS build needs CMake + Go + cc → reproducible-build impact
- R5 Linux Secret Service has no per-item user-presence gate
- R6 revoking a device that held S does not cut it off (no phrase
  rotation in suite v1: needs a re-rooted vault, i.e. a suite-v2 protocol
  change) — review finding on #238, WONTFIX for v1; `syncd revoke` replies
  with a `warning` saying so; cut-off = new vault with new words
- R7 app save/delete history hooks log a failure and still write (D1/D3
  are best-effort when the history store itself fails) — review finding
  on #238, follow-up
