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
   - `0x01` AES-256-GCM, HKDF-SHA256, HMAC-SHA256, SHA-256, Ed25519
   - `0x02` same, signatures ECDSA P-256 / SHA-256 (fallback if the
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

Body = `u8 version || u64 expires_unix || list<envelope body>` (several
vaults per QR). Key K_p is 32 random bytes in the QR together with the
server url + ns + object id; the object is DELETEd by the receiver and
expires server-side after 15 min.

## 5. sync-server API (/v1)

Transport: HTTPS (rustls). Bodies of control calls are JSON
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
- `POST /v1/enroll` — `{"enrollment_token":…,"pub_destroy":"<b64>",
  "key_alg":1}` → `{"ns":"<hex>","read_token":…,"write_token":…}`.
  Enrollment tokens are one-time, created by the admin CLI
  `opensidian-sync-server vault create`; no open registration.
- `GET /v1/health` — unauthenticated, `{"ok":true,"proto":[1]}`.

Errors: `{"error":"<code>","detail":"<text>"}`, codes
`bad_request unauthorized forbidden not_found gone conflict too_large
quota unsupported rate_limited internal`.

Limits (server defaults, configurable): object ≤ 16 MiB, control body
≤ 64 KiB, header parse only on the pre-auth path, per-token and per-IP
rate limits, list `wait` ≤ 60 s.

## 6. Client-side history store (M1, local only)

Not on the wire; documented here because M1 implements it:

- root `<app_data>/history/<vault_id>/` (dir 0700, files 0600), never
  inside the vault
- `objects/<aa>/<sha256-hex>` content-addressed blobs (SHA-256 of
  plaintext bytes), written via temp file + rename
- `index.log` append-only text journal of versions:
  path, sha256, size, ts, reason (`own-save`, `external`, `delete`,
  `rename`, `idle`, `restore`), plus tombstones for deletes
- invariant: before ANY overwrite/delete/sync write of a vault file,
  the current on-disk bytes are stored in `objects/` and journaled
- retention thinning + purge rewrite the journal atomically and GC
  unreferenced blobs

See docs in crates/sync-core for the exact journal grammar.
