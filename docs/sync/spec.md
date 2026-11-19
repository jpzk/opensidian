# Opensidian Sync — object, envelope and /v1 API spec (M0, draft v1)

Status: draft for review. Implements plan sections 3 (sync model),
4 (crypto), 5 (recovery & keys) and 6 (sync-server API). Key derivation
labels live in [key-tree.md](key-tree.md); vector file format in
[test-vectors.md](test-vectors.md). Clean room: designed from first
principles + public stock Sync help pages only.

Conventions: all integers big-endian. `id16` = 16 random bytes from the
module DRBG. `h32` = SHA-256 output. `||` = concatenation. Hex in URLs
is lowercase, no prefix. "MUST reject" means: return a parse error,
never best-effort decode.

## 1. Versioning (crypto agility, no silent downgrade)

Four independent version axes:

1. **API path** `/v1/...` — breaking HTTP changes get `/v2`; a server
   MAY serve both. Clients send `Opensidian-Proto: 1`.
2. **Container format byte** `fmt` (section 3.1) — `0x01` in v1.
3. **Suite byte** `suite` — the crypto algorithm set:
   - `0x00` reserved / invalid
   - `0x01` AES-256-GCM, HKDF-SHA256, HMAC-SHA256, SHA-256, Ed25519 —
     **the v1 suite** (OpenSSL FIPS provider 3.5.9, CMVP #5586, lists
     EDDSA ED-25519 as approved — cmvp.md §2, §6)
   - `0x02` same, signatures ECDSA P-256 / SHA-256 (fallback if a
     pinned CMVP cert does not cover EdDSA — see cmvp.md)
   - `0xF0..0xFF` test-only, MUST be rejected by release builds
4. **Record versions** inside plaintext (`commit_version`, `env_version`,
   `cfg_version`) — bump on any layout change.

Rules: unknown fmt / suite / record version → MUST reject (error
`unsupported`), never fall back. A vault records its minimum suite in the
cfg chain (section 5); objects with a lower suite than the vault minimum
are rejected (downgrade protection). Suite and fmt are inside the AAD.

## 2. Canonical encoding primitives

Used by every plaintext record and every signed payload:

- `u8 u16 u32 u64 i64` fixed width, big-endian
- `bytes` = `u32 len || len bytes`
- `str`   = `u16 len || UTF-8` (NFC, no NUL, ≤ 4096 bytes)
- `list<T>` = `u32 count || T*count`
- `opt<T>` = `u8 0` | `u8 1 || T`

Canonical = exactly one encoding per value: trailing bytes, non-minimal
`opt` tags, unsorted lists where sorting is required, or over-limit
lengths MUST be rejected. Signatures are always over canonical bytes.

## 3. Object container (ciphertext as stored on the server)

Every stored thing — batch, chunk, envelope, cfg, pairing — is one
container. The server sees only the container.

### 3.1 Header (56 bytes)

```
off  len  field
0    4    magic  "OSYN" (4f 53 59 4e)
4    1    fmt    0x01
5    1    suite  (section 1)
6    2    flags  u16, MUST be 0 in v1
8    16   obj_id id16 (equals the id in the URL)
24   32   commit key-commitment tag (3.3)
56   ...  segments
```

### 3.2 Segments (streaming AEAD)

Plaintext is cut into segments of exactly `SEG = 65536` bytes; the last
segment is 1..SEG bytes (an empty plaintext is impossible, the inner
header is ≥ 10 bytes). Each segment on the wire:

```
nonce 12  (module-generated, RandomizedNonceKey; never caller-chosen)
ct    n   (n = segment plaintext len)
tag   16  (GCM tag)
```

AAD per segment (39 bytes):

```
vault_id 16 || obj_id 16 || seg_idx u32 || last u8 (0/1) || suite u8 || fmt u8
```

`fmt` is added on top of the plan's AAD list so the header byte is
authenticated too. Truncation is detected by the `last` flag: a stream
whose final segment has `last=0`, or any segment after `last=1`, MUST be
rejected. Reordering is detected by `seg_idx`. Container size:
`56 + Σ(seg_len + 28)`.

### 3.3 Per-object keys + key commitment

```
okm   = HKDF-SHA256(ikm = K_root_for_kind, salt = obj_id,
                    info = "osy obj v1" || suite, L = 64)
K_enc = okm[0..32]      AES-256-GCM key
K_c   = okm[32..64]
commit = HMAC-SHA256(K_c, "osy commit v1" || vault_id || obj_id || suite || fmt)
```

`K_root_for_kind` is `K_obj` for batch/chunk, `K_cfg` for cfg, `K_wrap`
for the envelope, `K_p` for pairing (key-tree.md). Envelope and pairing
objects use `vault_id = 00*16` in AAD and commit (the vault id is not
known before decryption). Readers recompute `commit` and compare in
constant time BEFORE decrypting any segment; mismatch = wrong key or
partitioning attack → reject.

### 3.4 Inner plaintext (inside the AEAD)

```
u8   kind     0x01 batch, 0x02 chunk, 0x03 envelope, 0x04 cfg, 0x05 pairing
u8   comp     0x00 none, 0x01 zstd (zstd lands with M4; v1 writers of
              M1-M3 emit 0x00)
u64  body_len length of the (compressed) body
...  body
...  zero padding up to padme(total)   (3.5)
```

Kind lives inside the ciphertext so the server cannot tell types apart
(except by size class). Padding bytes MUST be zero; nonzero → reject.

### 3.5 Padding

- batch / envelope / cfg / pairing: Padme. With `L` = len of
  `kind..body`, `E = floor(log2 L)`, `S = floor(log2 E) + 1`,
  `z = E - S`, `mask = 2^z - 1`, `padme(L) = (L + mask) & !mask`.
  For `L < 2` padme(L) = L. Overhead ≤ ~12%.
- chunk: ALWAYS padded to exactly `4 MiB + 10` plaintext (uniform size,
  plan "uniform sizes").

## 4. Records

### 4.1 Batch (kind 0x01) — one sync batch = one object

```
u8          commit_version   0x01
id16        commit_id        == container obj_id
id16        device_id
i64         ts_ms            device clock, signed; ONLY for LWW tie rule
list<parent> parents         sorted by commit_id, 0 (root) .. 16
h32         tree_hash        (4.3) of the tree AFTER this commit
list<change> changes         sorted by path, ≤ 1 change per path
list<bytes>  blobs           inline contents (each ≤ 1 MiB)
u8          sig_alg          == suite signature algorithm
bytes       sig              over SHA-256("osy batch v1" || all bytes above)

parent  = id16 commit_id || h32 tree_hash
change  = u8 op || str path || op-specific
  op 0x01 put:    u8 class (1 markdown, 2 other, 3 settings json)
                  u64 size || h32 content_sha256 || i64 mtime_ms
                  u8 where: 0x01 inline u32 blob_idx
                            0x02 chunks list<id16> (in file order)
  op 0x02 delete: (nothing)
  op 0x03 rename: str new_path || h32 content_sha256 (must match old)
```

The signature is by the device signing key listed in the current cfg
generation (4.4). Unknown device / revoked slot → reject the batch (it is
still kept as an object; no data is discarded on the server side).

Implementation notes (M4a, `sync_proto::batch` layout + `sync_core::commit`):

- **Canonical extras** (checked on encode AND decode): parents strictly
  ascending, a commit is never its own parent; changes strictly ascending
  by path bytes; paths are vault-relative (non-empty `/`-separated
  segments, no `.`/`..`/empty segment, no backslash or control char);
  inline blobs are referenced exactly once and in change order
  (`blob_idx` = 0,1,2,… — no unreferenced or shared blob); inline
  `size` = blob length ≤ 1 MiB; `where = chunks` only for size > 1 MiB and
  with exactly `ceil(size / 4 MiB)` ids; rename target ≠ source.
- **Changes are relative to the tree of `parents[0]`** (lowest commit_id;
  empty tree for a root). Applying them: delete/rename need an existing
  source, rename carries the source hash and may not land on an existing
  path. `tree_hash` is the tree AFTER the commit; a receiver rebuilds the
  tree from the manifest and compares.
- **Signature**: `sig = Sign(K_dev, SHA-256("osy batch v1" || bytes from
  commit_version through sig_alg))`; with suite 0x01 the module signs that
  32-byte digest with pure Ed25519 (RFC 8032, 64-byte R||S); suite 0x02
  uses ECDSA P-256/SHA-256 (fixed 64-byte r||s).
- **Verify order**: container (commitment, GCM) → kind = batch → decode +
  canonical checks → commit_id = obj_id → sig_alg of the vault suite →
  device key from cfg (unknown/revoked → reject) → signature → every inline
  blob's SHA-256.
- **Chunk ids** are fresh random id16 (no content-derived ids: equal
  attachments in two commits are not linkable by the server).
- **Write order**: chunks first, then the batch, so a visible batch never
  references a missing chunk; pushes are idempotent (write-once,
  identical re-upload = OK).
- **Size**: the encoded batch is capped at 12 MiB before compression so
  Padme + segment overhead stays under the default 16 MiB object limit;
  the engine splits larger edit sets into several commits.

### 4.2 Chunk (kind 0x02)

Body = raw 4 MiB plaintext slice of an attachment > 1 MiB (last slice
shorter, then padded per 3.5). The chunk carries no path; position comes
from the signed batch. `content_sha256` in the batch covers the whole
file, so a reassembled file is verified end-to-end.

### 4.3 Tree hash (deterministic merge, no ping-pong)

```
tree_hash = SHA-256("osy tree v1" || list<entry>)
entry     = str path || u8 class || h32 content_sha256
```

sorted bytewise by path. A merge whose resulting tree_hash equals a
parent's tree_hash creates NO new commit. Ordering comes from parents
only; `ts_ms` is consulted only for the last-write-wins tie rule
(tie-break: larger commit_id bytes wins).

### 4.4 cfg generation (kind 0x04), namespace `cfg`, id = gen-derived

```
u8   cfg_version 0x01
u32  gen                          0 = genesis
h32  prev                         SHA-256 of previous cfg plaintext, 00*32 at gen 0
u8   min_suite
u32  vk_epoch                     current VK epoch
bytes rsig_pub                    public K_rsig (32 Ed25519 / 65 P-256, per min_suite), same in every gen
list<slot> slots                  ALWAYS 32 slots (padding: state 0)
u8   signer_kind                  1 = device slot, 2 = K_rsig
u8   signer_slot
bytes sig                         over SHA-256("osy cfg v1" || all bytes above)

slot = u8 state (0 empty, 1 active, 2 revoked) || id16 device_id
       || u8 key_alg || bytes pubkey (32 Ed25519 / 65 P-256 uncompressed)
```

Gen N+1 MUST be signed by an active slot of gen N, or by K_rsig (recovery
from words). The cfg object id is
`HMAC-SHA256(K_cfg, "osy cfg id v1" || gen)[0..16]` so clients find gen
N+1 without listing; write-once makes forks visible (two writers → 409).

### 4.5 Envelope (kind 0x03), namespace `env`, id = loc

```
u8    env_version 0x01
u32   env_gen
id16  vault_id
list<vk>   keyring       vk = u32 epoch || 32 bytes VK
list<srv>  servers       srv = str url || id16 ns || bytes read_token
                         || bytes write_token || list<h32> spki_pins
```

Encrypted under `K_wrap` (from the words only), so "words → everything"
recovery works with nothing else. No signature: only holders of S can
produce K_wrap. Envelope object id for gen g:
`HMAC-SHA256(loc, "osy env id v1" || g)[0..16]`; the client probes
g = 0,1,2,… (write-once, newest wins).

### 4.6 Pairing (kind 0x05), namespace `pair`

Body: see the M5 implementation notes below (normative: `u8 version 1 ||
u8 msg`, msg 1 = offer `u64 expires_unix || list<bytes envelope body ||
opt<22 S_bytes>>`, msg 2 = reply). The offer may carry several
vaults per QR. Key K_p is 32 random bytes in the QR together with the
server url + ns + object id; the object is DELETEd by the receiver and
expires server-side after 15 min.

### 4.4–4.6 implementation notes (M5, `sync_proto::vault` + `sync_core::vault`)

- **cfg** gains `bytes rsig_pub` (public K_rsig, 32-byte Ed25519 in suite 0x01) after
  `vk_epoch`; identical in every gen. Gen 0 MUST be signed by K_rsig
  (`signer_kind 2`, `signer_slot 0`). Gen N+1: gen = N+1, `prev` =
  SHA-256 of gen N's full plaintext, same rsig_pub, `vk_epoch` and
  `min_suite` never decrease, every non-empty slot of gen N is still there
  with the same key, a revoked slot never becomes active again, signer =
  an ACTIVE slot of gen N or K_rsig. Empty slots are all-zero, device ids
  unique. Signature: `Sign(SHA-256("osy cfg v1" || signed bytes))`.
- The cfg chain is sealed and addressed with the **genesis epoch's K_cfg**
  (keyring[0]), not the current one: any keyring holder — including a
  device that has not followed a rotation yet — can walk the chain and
  learn that it must. Slots hold public keys only.
- **Roster** (commit signature keys) = every device that ever held a
  slot; revoked = not active in the head gen. A revoked device's key still
  verifies, but the engine keeps its commits only when *vouched for* (§4.8).
- **Envelope** server entry gains `bytes server_key` (the destroy-receipt
  key pinned at enrollment, 0 or 65 bytes); tokens are the base64url
  strings (≤ 64, alphabet checked). Keyring epochs strictly ascending,
  keyring[0] = epoch 0. A new envelope gen is written for every keyring
  change; readers probe gen 0,1,2,… and take the newest.
- **Pairing**: `u8 version 1 || u8 msg || …`. msg 1 = offer:
  `u64 expires_unix || list<bytes envelope body || opt<22 S_bytes>>`
  (the words are included by default so the new device can follow a key
  rotation; the inviter may omit them). msg 2 = reply:
  `id16 device_id || u8 key_alg || bytes pubkey || str name`, sealed under
  the SAME K_p at id `HMAC(K_p, "osy pair reply v1" || u32 0)[0..16]` —
  the channel is symmetric. Flow: inviter `pair_offer` (PUT offer) →
  new device `pair_accept` (GET + DELETE offer, PUT reply) → inviter
  `pair_complete` (GET + DELETE reply, cfg gen+1 adding the slot, retried
  on 409 against a re-fetched chain) → new device `pair_join` (chain must
  list it active). Pairing objects use `vault_id = 00*16` in AAD.
  QR text: `osy-pair1:` + base64url(`u8 1 || str url || id16 ns ||
  id16 offer_id || 32 K_p`).
- **Create**: words → K_destroy → `enroll` → random vault_id + VK epoch 0
  → cfg gen 0 (creator in slot 0, signed by K_rsig) → envelope gen 0.
  **Join from words**: newest envelope under loc → tokens, keyring →
  chain (genesis rsig_pub must equal the words' K_rsig) → cfg gen+1 adding
  the new device, signed by K_rsig.
- **Revoke**: new random VK at epoch+1 → envelope gen+1 (needs the words:
  only K_wrap publishes keys) → cfg gen+1 with the slot `revoked` and the
  new `vk_epoch`, signed by the revoking device. New objects are sealed
  under the new epoch's K_obj; old epochs stay readable (readers pick the
  K_obj whose key-commitment tag matches, `object::select_key`). There is
  no public-key encryption, so another device learns the new VK only from
  the envelope: `refresh_chain` answers `NeedWords` until the device
  unlocks its backup entry (user presence) and calls `refresh_with_words`.
  Residual (follow-up): the revoked device keeps the server write token
  (no token-rotation endpoint yet) — it can still upload ciphertext, which
  every client rejects (§4.8), and soft-delete within the retention window.
  **Residual (open, review finding on #238): a revoked device that holds
  S_bytes (words entered, or received in a pairing offer — the default)
  can still derive K_wrap and read the new envelope, i.e. the new VK.**
  Revocation therefore cuts off ONLY devices that never held S. Required
  follow-up before revocation is advertised as complete: revoking a
  device that held S MUST first rotate the phrase (new S → new loc,
  K_wrap, K_rsig chain hand-over) and publish the new epoch's envelope
  ONLY under the new loc; the old loc's envelope stays at the old epoch.
  Until then the UI/CLI must say so when revoking such a device.

### 4.7 Tip merge (M4b, `sync_core::merge`)

Input: two tips L (local) and R (remote) and their exact base B.

- **Base** = a best common ancestor in the DAG (a common ancestor that is
  not an ancestor of another common ancestor). Criss-cross (several) →
  the smallest commit_id. No common ancestor → empty tree. A commit
  missing locally → error, never a guessed base.
- **Winner** W = the LWW winner of the two tips: larger tip `ts_ms`, tie →
  larger tip commit_id. Loser X = the other tip. All "both changed"
  rules below are phrased in W/X, so both devices compute the same tree
  (except the two by-design local rules: conflict mode, settings JSON).
- **Per path** (equality = class + content SHA-256):
  - L = R → L; L = B → R; R = B → L (covers one-sided add/edit/delete).
  - deleted on one side, changed on the other → the edit (edit beats
    delete).
  - markdown on both sides, mode *Automatically merge* (default): DMP
    three-way — diff(B, X) as context patches applied to W. Exact match
    only (no fuzzy bitap), ambiguous match → fail; diff over D = 2000 per
    level → fail. Fail / no base (add-add) / non-UTF-8 → conflict file.
  - markdown, mode *Create conflict file* → conflict file.
  - conflict file: the path keeps W; X's bytes go to
    `<note> (Conflicted copy <X device> YYYYMMDDHHMM).md` — X's device
    display name (`/\:*?"<>|` and control chars → `-`, ≤ 64 chars,
    empty → `device`), UTC of X's tip `ts_ms`. Name taken → ` 2`, ` 3`…
    inside the parentheses.
  - settings JSON on both sides: L's keys on top of R's (deep for nested
    objects), pretty-printed; non-object / unparsable → L.
  - everything else (other files, class mismatch): W.
- **Losers**: every version that does not survive verbatim (LWW loser,
  both sides of a text merge, conflict-copy source, overridden settings)
  is returned with its tree entry; the engine writes remote losers into
  file history as `sync-loser` records (head untouched, idempotent) and
  snapshots local bytes before overwriting them (threat model D4/D5).
- **Commit**: merged tree hash = L's → nothing (up to date); = R's →
  fast-forward to R, no commit; else one merge commit, parents {L, R},
  changes relative to `parents[0]`, existing chunk objects re-referenced
  (no re-upload). Merging the same tips again yields the same tree hash.
- Renames are path-level here (a rename on one side + an edit on the
  other keeps both paths; nothing is lost).

### 4.8 Sync engine: equal peers, set union (M4c, `sync_core::engine`)

Every replica (sync-server, dir replica, …) is a `Transport` holding
opaque `obj` containers; none is authoritative. The engine owns a state
dir: `lock` (exclusive `flock`, a second engine → `Busy`), `state.json`
(`{v:1, head, absorbed}`, tmp + fsync + rename) and `objects/` — its own
dir replica holding every object it has verified.

One `sync(replicas)` pass:

1. **pull**, per replica: paged `list` (wait 0); each unknown id is
   fetched and verified before it is stored — batch = container auth +
   kind + decode + signature of a device in `devices`; chunk = container
   auth under `K_obj` with kind chunk. A failure is reported in
   `rejected` and the object is neither stored nor pushed anywhere
   (a bad or hostile replica cannot poison the others). 404/410 between
   list and get is skipped.
2. **resolve**: *ready* commits = every ancestor known; commits waiting for
   a parent are `pending` (kept + propagated, never a tip). Tips (ready,
   no ready child) are folded into the head in commit_id order:
   ancestor of the head or absorbed → skip; else `merge_base` →
   `merge_trees` → `FastForward` (head = tip), `Commit` (merge commit
   signed by this device, head = it) or `UpToDate`. `UpToDate` with a
   tip that is not an ancestor (two devices merged the same tips
   concurrently: same tree, different ids) makes NO commit; the tip is
   *absorbed* and becomes an extra parent (≤ 16 total) of the next local
   commit, so tips cannot pile up and there is no ping-pong. Losers go to
   the `on_loser` hook (→ `history::Store::record_sync_loser`).
3. **push**, per replica that listed fine: every local object it lacks,
   chunks first, then batches by generation (parents first), so a reader
   never sees a dangling reference. 410 (deleted id) is skipped.

A failing replica is reported (`replica_errors`) and skipped; the pass
continues with the others. After a pass in which every replica answered,
each holds ⋃(local, R₁..Rₙ) − rejected — so two devices that never share
a replica still converge through any chain of replicas.

`commit_local(edits)` (edits relative to the head tree): parents = head ∪
absorbed; with one parent the edits go in as is, otherwise the target
tree is materialised and diffed against `parents[0]` (renames become
delete + put there; chunks are re-referenced). Re-verification on open
uses the caller's `devices`: a commit whose key is no longer accepted
drops out of the DAG. M5: the roster keeps a revoked device's key, and a
commit signed by a revoked device is kept only when it is **vouched for**
— an ancestor of a commit by a non-revoked device, of this engine's head
or of an absorbed tip. On pull such commits wait in a quarantine until
the whole pass is listed; unvouched ones are reported as rejected
(`engine::NOT_VOUCHED`), not stored, and re-evaluated on the next pass.
So pre-revocation history stays, anything the revoked device signs
afterwards (nobody active built on it) is refused. Not yet:
an index cache (open re-verifies the local replica, O(objects)), GC of
superseded chunks, long-poll (syncd, M6).

## 5. sync-server API (/v1)

Transport: HTTPS (rustls). Impl note (M3a): TLS 1.3 only, ALPN
`http/1.1`, rustls on the OpenSSL FIPS provider (rustls-openssl, TLS 1.3
AES-GCM suites, ECDHE P-256/P-384 only; `tls::fips_provider()`); the
server refuses a non-FIPS config and refuses plain HTTP on any non-loopback
bind (`--tls-cert`/`--tls-key` PEM; handshake bounded at 10 s). TLS is
defence in depth only — every object is already sealed end-to-end.
Bodies of control calls are JSON
(`application/json`, UTF-8, unknown fields rejected); object bodies are
`application/octet-stream` containers. `{ns}` = 32-hex server namespace
id assigned at `vault create` (random, unrelated to vault_id → no
cross-server linkage). `{id}` = 32-hex obj_id. `{kind}` path segment ∈
`obj | cfg | env | pair`.

Auth: `Authorization: Bearer <base64url 32 bytes>` capability token,
per-namespace, scope `read` or `write` (write ⊇ read). Server stores
SHA-256(token) only. `env` GET/PUT is authorized by knowledge of `loc`
(the id is a 128-bit secret) so recovery from words works with no token;
the server still rate-limits it per IP.

Implementation notes (M3a, `crates/sync-server`):

- Tokens on the wire are base64url without padding of exactly 32 bytes;
  the server stores `SHA-256(raw 32 bytes)` only (test
  `tokens_are_stored_hashed` greps the SQLite files). Every other base64
  value of this API (`pub_destroy`, `sig`, `receipt`, `server_key`) uses
  the same alphabet. An unknown `{ns}` and a wrong token both answer 401
  (no namespace oracle); a read token on a write path answers 403.
- `env` objects live in the all-zero namespace
  (`/v1/env/000…0/{id}`): a fresh device knows only the words and the URL,
  not `{ns}`. No token, write-once, per-IP rate limit, not deletable.
- `pair` objects are PUT with the vault's write token; GET and DELETE need
  only the id (the QR holds it). DELETE purges at once; unread objects
  expire `pair_ttl` (15 min) after the PUT.
- `GET /v1/list` returns live `obj` and `cfg` rows only; `POST /v1/missing`
  checks `obj` ids (live or soft-deleted). A soft-deleted id is never
  reusable: PUT answers 410.
- `enroll` accepts `key_alg = 1` (Ed25519, 32-byte `pub_destroy`) only —
  suite v1 (cmvp.md §7); `key_alg = 2` answers `unsupported`.
- `Opensidian-Proto` is optional; if present it must be a listed major.
- Unsatisfiable or multi-part `Range` answers 416 with
  `Content-Range: bytes */<size>`.
- Quota is per vault (set by `vault create --quota`), counted over stored
  bytes including soft-deleted objects until GC; ENOSPC also answers 507.

Endpoints:

- `PUT /v1/{kind}/{ns}/{id}` — write-once, idempotent.
  201 created; 200 if identical bytes already stored (SHA-256 compare);
  409 if different bytes under the same id; 413 if over the size limit;
  507 quota/disk full; 400 if header magic/fmt or URL id ≠ header obj_id.
  The server validates the header only (it cannot read anything else).
- `GET /v1/{kind}/{ns}/{id}` — full object; supports
  `Range: bytes=a-b` → 206 (resume). 404 unknown, 410 soft-deleted.
- `POST /v1/missing/{ns}` — `{"ids":["<hex>",…]}` (≤ 1024) →
  `{"missing":["<hex>",…]}`.
- `GET /v1/list/{ns}?after=<seq>&wait=<0..60>&limit=<1..1000>` —
  long-poll cursor. → `{"items":[{"seq":n,"kind":"obj","id":"<hex>",
  "size":n}],"next":n}`. `seq` is a per-namespace u64 counter; no
  timestamps are returned (minimal metadata).
- `DELETE /v1/{kind}/{ns}/{id}` — soft delete (202); object stays
  readable via `?deleted=1` until the server's retention window
  (config, default 30 d) passes, then GC.
- `POST /v1/destroy/{ns}/request` — `{"scope":"vault"}` →
  `{"server_nonce":"<hex32>","confirm_token":"<b64>","expires":unix}`.
- `POST /v1/destroy/{ns}/confirm` —
  `{"confirm_token":…,"ts":unix,"sig":"<b64>"}` where sig is by
  K_destroy over canonical bytes
  `"osy destroy v1" || u8 action(1=destroy) || u8 scope(1=vault) ||
  id16 ns || 32 server_nonce || i64 ts`. Server verifies against the
  `pub_destroy` it stored at enrollment → signed receipt
  `{"receipt":"<b64>","server_key":"<b64>"}`. No proof → no destroy.
  Implementation notes (M3b):
  - `request` needs no bearer token (the proof is the authority, so a
    device holding only the words can destroy); per-IP rate limit, unknown
    vault → 401, destroyed vault → 410, at most 16 open challenges per vault
    (→ 429). TTL 300 s. `confirm_token` is 32 DRBG bytes, stored hashed.
  - `confirm` consumes the challenge before anything else (single use even
    when the proof then fails, so a token cannot be used to grind), then
    requires `|ts − server now| ≤ 300 s` (400) and a 64-byte Ed25519
    signature (bad/missing/wrong-key/wrong-nonce/wrong-ts →
    403 `forbidden`). A challenge of vault A never confirms vault B (401).
  - Purge is one transaction under the index lock (no concurrent PUT can
    publish into the namespace): object rows, tokens (the vault row) and
    challenges go, a tombstone row stays (`request`/`confirm` → 410, tokens
    → 401). The object tree is renamed into `tmp/` inside the transaction
    and deleted after commit; startup finishes an interrupted deletion.
    `env` objects are not linked to a namespace on the server (minimal
    metadata) and stay; they are ciphertext under the phrase and now point
    at a dead namespace.
  - Receipt = `"osy destroy receipt v1" || u8 scope(1) || id16 ns ||
    32 server_nonce || i64 ts || i64 destroyed_at || u64 objects ||
    u64 bytes` followed by the 64-byte Ed25519 signature of the server receipt
    key (Ed25519 from the module, `<data>/receipt_key.p8`, PKCS#8 v2, 0600,
    created once). `POST /v1/enroll` also returns `server_key` so devices
    pin it at enrollment (TOFU) and compare it with the receipt's.
- `POST /v1/enroll` — `{"enrollment_token":…,"pub_destroy":"<b64>",
  "key_alg":2}` → `{"ns":"<hex>","read_token":…,"write_token":…,
  "server_key":"<b64>"}` (receipt key, see destroy).
  Enrollment tokens are one-time, created by the admin CLI
  `opensidian-sync-server vault create`; no open registration.
- `GET /v1/health` — unauthenticated, `{"ok":true,"proto":[1]}`.

Errors: `{"error":"<code>","detail":"<text>"}`, codes
`bad_request unauthorized forbidden not_found gone conflict too_large
quota unsupported rate_limited internal`.

Limits (server defaults, configurable): object ≤ 16 MiB, control body
≤ 64 KiB, header parse only on the pre-auth path, per-token and per-IP
rate limits, list `wait` ≤ 60 s.

### 5.1 Client transports (M3c, `sync_core::transport`)

One `Transport` trait mirrors the calls above; the engine never branches on
the backend. Errors are the same `ErrorCode`s on every backend.

- **HTTP(S)** (`HttpTransport`): blocking HTTP/1.1 over std `TcpStream`,
  one request per connection. `https://` = TLS 1.3 on the OpenSSL FIPS
  rustls provider, refused unless `ClientConfig::fips()`; roots = explicit
  PEM (self-hosted CA) or the OS bundle (`$SSL_CERT_FILE`, distro paths).
  `http://` only to `localhost`/loopback IPs; URLs with userinfo, query,
  fragment or dot segments are refused. Responses are bounded (header
  16 KiB, body 16 MiB + 64 KiB); chunked encoding is refused (the server
  always sends `Content-Length`). Tokens live in the transport, Debug is
  redacted, a missing token fails before any network I/O.
- **Destroy helper**: `destroy_vault` = request → sign `destroy_payload`
  with K_destroy → confirm → `verify_receipt`: `server_key` must equal the
  key pinned at enrollment, the signature must verify, and the body must
  name the same ns, nonce and ts.
- **Dir replica** (`FsTransport`): `<root>/osy-replica-v1` marker (a
  non-empty folder without it is refused); `<ns>/<kind>/<id>` objects
  staged + fsynced + hard-linked (link fails on an existing name →
  write-once even against a racing writer); tombstones in `<ns>/del/`
  (410, id never reusable, readable as `deleted`); `obj`/`cfg` appended
  to `<ns>/log` (25-byte records, seq = record index + 1) under an
  exclusive per-namespace file lock, a torn tail is ignored and trimmed on
  the next append. Pair objects are purged on delete. Not offered:
  enrollment, tokens, destroy (a folder cannot attest a purge), retention
  GC. Long-poll = 50 ms polling of the log.

## 6. Client-side history store (M1, local only)

Not on the wire; documented here because M1 implements it:

- root `<app_data>/history/<vault_id>/` (dir 0700, files 0600), never
  inside the vault
- `objects/<aa>/<sha256-hex>` content-addressed blobs (SHA-256 of
  plaintext bytes), written via temp file + rename
- `index.log` append-only text journal of versions:
  path, sha256, size, ts, reason (`own-save`, `external`, `delete`,
  `rename`, `idle`, `restore`), plus tombstones for deletes
- invariant (goal): before ANY overwrite/delete/sync write of a vault
  file, the current on-disk bytes are stored in `objects/` and journaled.
  Actual guarantee today: the sync engine (syncd `sync_with`) snapshots
  through the history sink before materialising; the app's own save /
  delete hooks LOG a history-write failure and still proceed with the
  write, so a failing history store (disk full, permissions) can lose the
  pre-image of that one write. Open (review finding on #238): abort the
  write or surface the failure before overwriting.
- retention thinning + purge rewrite the journal atomically and GC
  unreferenced blobs

See docs in crates/sync-core for the exact journal grammar.

### 6.1 Rename detection

- explicit: `record_rename(from, to)` after an own move (or a watcher
  event that carries both paths) → one `rename` record (`path` = new,
  `from` = old); the old path leaves the head map, so it is NOT listed
  as deleted
- heuristic: a delete of A and a create of B with identical, non-empty
  bytes within `rename_window_ms` (default 10 s), in either order, are
  folded into one `rename` record. Empty files are never paired (new
  empty notes are too common to be evidence of a move). Only
  `external`/`own-save` creates of a path that was absent qualify.

### 6.2 Tombstones / restore deleted

- `deleted()` = paths whose last record is a `delete` tombstone
- `restore_deleted(path)` writes back the newest stored bytes (the
  tombstone's own, else the newest earlier version — a too-large file's
  tombstone has none); it snapshots whatever is on disk first, like any
  restore

### 6.3 Retention thinning (`thin(now, Retention)`)

Evaluated per path, newest record first:

1. the newest record of a LIVE path is always kept
2. records older than the max age are dropped — notes (`.md`,
   `.canvas`) default 365 d, attachments 14 d (plan §2 "separate
   attachment rule"); a file deleted longer ago than that disappears
3. else `delete` and `rename` records are kept
4. else every version younger than `keep_all` (24 h) is kept
5. else the newest version per clock hour for `hourly_for` (7 d)
6. else the newest version per UTC day

Guard: the rewrite is refused (nothing written) if replaying the kept
records would change the head of any live path or resurrect a path;
the only allowed head change is a deleted path vanishing.

### 6.4 Purge (secrets)

- `purge_path(p)`: drop every record with `path = p`; a `rename` with
  `from = p` is rewritten as `external` (the new path keeps its bytes,
  the old name is forgotten)
- `purge_version(seq)`: a version record is dropped; a tombstone keeps
  existing without `sha`; a `rename` becomes a bytes-less tombstone of
  its `from` path
- both: index.log rewritten via `index.log.tmp` + fsync + rename (old
  or new journal survives a crash, never a mix), THEN blobs no record
  references are unlinked (a crash in between leaves orphans for the
  next GC, never a dangling reference). Identical bytes still held by
  another record stay. Unlink is not a secure wipe (SSD/CoW/backups —
  see threat model).
- a purged newest record's `seq` may be reused after reopen; UIs must
  re-list after purge.

### 6.5 App wiring (src-tauri/src/filehist.rs)

- one store per process at `~/.local/share/dev.koto.opensidian/history/<vault id>/`
  (inside the Landlock write roots, outside the vault); the vault id is the
  treefold `vault_ids` entry of `~/.opensidian.json`, minted on the first
  history write. `OPENSIDIAN_HISTORY=0` disables history
- hooks, one call site per writer:
  - `write_note_in`, `update_links_in`: `snapshot_before_overwrite` →
    atomic write → `record_own_save`
  - `create_note_in`: `record_own_save`
  - `move_note_in`: `record_rename` after the move succeeds
  - `delete_note_in`: `snapshot_before_delete` before the `.trash` move
    (re-journaled `external` if the move fails)
  - watcher tick: `record_external` for added/modified/removed notes,
    `record_rename` for pairs the watcher matched
- best effort: hook errors are logged (`[history]`) and never fail the
  save; a store that cannot open turns history off for the session
- lock order: index mutex → history mutex; the watcher calls in with no
  index lock held
- commands: `history_list(name)`, `history_read(seq)`,
  `history_restore(seq)`, `history_deleted()`,
  `history_restore_deleted(name)`, `history_purge(name, seq?)`. Restores
  go through the app's hardened writers (`write_note_in` / `create_note_in`,
  fd-relative, O_NOFOLLOW), not `Store::restore`, so the pre-write snapshot
  and index == disk hold; `history_restore_deleted` refuses if the path
  exists again
- M1 limits: notes (`.md`) only — attachments, canvas and `.obsidian/`
  config are not journaled yet; idle snapshots (`Debouncer`) are not
  scheduled by the app (every save is already journaled); thinning runs
  once when the store opens

### 6.6 Restore / diff UI — TODO (not in M1)

No UI ships in M1; the backend commands above are complete and tested,
the panel is deferred. Reason: `ui/main.js` is a ~15k-line vanilla JS
module and this milestone has no JS test path, so a panel could not be
verified to the same bar as the backend. Planned shape (follow-up PR):

- command palette + file menu entry "Open file history" for the active
  note → right-side panel listing `history_list(name)` newest first
  (time, reason, size; tombstones and renames labelled)
- selecting a version: `history_read(seq)` → read-only preview, plus a
  line diff against the current editor buffer (pure JS LCS, no new dep;
  cap at the store's `max_file_bytes`)
- "Restore this version" → `history_restore(seq)`, then reload the note;
  confirm first if the buffer is dirty (the restore itself snapshots the
  on-disk bytes, so it is undoable from the same list)
- "Recently deleted" view: `history_deleted()` →
  `history_restore_deleted(name)`; surface the "path exists" refusal
  as-is
- "Purge…" (per version / whole path) behind a typed confirmation, since
  `history_purge` is irreversible by design (§6.4)
- acceptance: the panel never writes to the vault except through these
  commands; keyboard reachable; works at the 320×240 layout

## 7. App integration (M6b, `src-tauri/src/syncui.rs`)

### 7.1 Command gate (threat-model T5 / R1)

Community plugins run in the app page's JS realm; nothing page script can
check separates core UI from plugin code. The trust decision is made on the
webview LABEL the IPC message arrived from — set by Rust when the webview is
built, not choosable by page script:

- every sync/history command (`syncui::GUARDED`: `history_list`,
  `history_read`, `history_restore`, `history_deleted`,
  `history_restore_deleted`, `history_purge`, `history_diff`, `sync_status`,
  `sync_setup`, `sync_now`, `sync_set_mode`, `sync_reveal`, `sync_quiz`)
  starts with `gate(win.label(), "<cmd>")?` and answers only the webview
  labelled `sync`; any other (the `main` window = the plugin realm) gets
  `EPERM: <cmd>: sync and file history answer only the sync window` before
  any state is touched;
- the `sync` webview is built only by `sync_open` (not guarded: it shows or
  focuses the window, nothing else) from the fixed app page `ui/sync.html`,
  which loads no plugin loader and no community code; navigation is limited
  for every webview by the app's `on_navigation`;
- `capabilities/default.json` grants no `create-webview*` permission, so page
  script cannot build a webview with the trusted label;
- tests (`syncui::tests`): a source check that every guarded command is
  registered and calls the gate FIRST and that no `history_*` / `sync_*`
  command is left unguarded; and a real IPC round trip through tauri's mock
  runtime invoking EVERY guarded command with valid arguments from a `main`
  webview (refused, EPERM) and from a `sync` webview (passes the gate).

### 7.2 Embedded engine

`syncd::local::LocalEngine` in-process; state dir `<app data>/sync/<vault
id>/` (same layout as the daemon's, §M6a). A sync pass writes file history
through `syncd::local::HistSink` into the app's one history store
(`FILEHIST`), never through a second opener of the same journal. Network
work runs on the blocking pool. A running `opensidian-syncd` on the same
state dir serialises on the per-vault flock (`busy`).

### 7.3 Reveal backup words

`sync_reveal` asks a native GTK question first (consent `Kind::Reveal`,
fixed text; page script cannot answer it), then returns the words to the
sync window only. The page shows them without a copy button and hides them
after 60 s. `sync_quiz` (verify the user wrote them down) checks positions
against the backup entry and is allowed for 10 minutes after a granted
reveal only (otherwise an oracle on the words). Linux has no per-item OS
user-presence gate (threat-model R5, open question 6): the native question
is the presence check there.
