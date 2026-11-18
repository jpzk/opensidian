# Opensidian Sync — key tree (M0, draft v1)

Binding source: plan sections 4 + 5. All derivations go through the
CMVP-validated module (HKDF-SHA256 / HMAC-SHA256 / DRBG). Every label is
an ASCII byte string, no terminator. `HKDF(ikm, salt, info, L)` =
RFC 5869 extract-then-expand with SHA-256.

## 1. Backup words S

- word list: EFF large wordlist, 7776 words, embedded in the binary,
  SHA-256 of the file pinned in sync-crypto (CC-BY 3.0 attribution in
  the about/licence screen)
- 11 random words + 1 checksum word = 12 words
- entropy: 11 × log2(7776) = 11 × 12.925 = 142.2 bits
- generation: module DRBG, uniform index via rejection sampling
  (draw u16, reject ≥ 7776 × 8 = 62208, take `x mod 7776`), or dice mode
  (5 d6 rolls per word = 6⁵ = 7776, digits only; the app computes word 12)
- canonical bytes `S_bytes` = 11 × u16 big-endian index (22 bytes). Words are never
  hashed as strings → no Unicode/case/whitespace ambiguity.
- checksum word index =
  `u16be(SHA-256("osy words v1" || S_bytes)[0..2]) mod 7776`
  UX only (not security): typo vs. "vault gone", offline, version
  detection (v2 changes the label), rejects BIP39 seeds (a random
  12-word BIP39 phrase passes with p ≈ 1/7776 AND must consist of EFF
  words).
- implementation (M2c): `sync_crypto::words` — `Phrase::{generate,
  from_dice, parse, from_s_bytes, s_bytes, words}`. Parsing: any
  whitespace, ASCII case-insensitive, exact words (no prefix matching);
  errors name a 1-based position, never a word. Only 870 of the 2048
  BIP39 English words are EFF words, so a BIP39 seed is rejected as
  "unknown word" with p ≈ 1 − (870/2048)¹² and otherwise by the checksum.
  Checksum KATs (computed with coreutils `sha256sum`) in `tests/words.rs`.

## 2. Root R and its children

```
R         = HKDF(S_bytes, salt="osy root v1", info="root", 32)
K_wrap    = HKDF(R, salt=00*32, info="osy wrap v1", 32)     envelope key
loc       = HKDF(R, salt=00*32, info="osy loc v1", 16)      server locator
K_rsig    = HKDF(R, salt=00*32, info="osy rsig v1", 32)     cfg recovery signing seed
K_destroy = HKDF(R, salt=00*32, info="osy destroy v1", 32)  destroy signing seed
```

No memory-hard KDF: S has 142 bits of entropy (not a password), so
brute force is out of reach; HKDF keeps everything inside the module.
Signing seeds: Ed25519 seed directly (suite 0x01, reserved); for P-256
(suite 0x02, v1) the scalar follows the arithmetic of FIPS 186-5 A.2.2
(rejection sampling):

```
c_ctr = HKDF(seed, salt=00*32, info="osy p256 v1" || u8 ctr, 32)   ctr = 0, 1, …
first c_ctr with c_ctr ≤ n − 2 wins;  d = c_ctr + 1   (so 1 ≤ d ≤ n − 1)
```

(a candidate is rejected with p ≈ 2⁻³², after 256 rejections the provider
is treated as broken). The scalar enters the module as an RFC 5915
ECPrivateKey; AWS-LC computes Q = d·G and runs its FIPS pairwise-consistency
check. Resolved in M2d (open questions 10 + 12, cmvp.md §8): K_destroy and
K_rsig stay phrase-derived. Implementation: `sync_crypto::keytree`
(`derive_root`, `p256_scalar_from_seed`) + `fips::FipsSigningKey::from_seed`;
tests in `crates/sync-crypto/tests/p256_derive.rs` (d = 1 → G,
d = n − 1 → −G, out-of-range rejected, retry path, FIPS vs soft HKDF chain,
fresh-device re-derivation from words).

## 3. Vault key VK (random, not derived)

- one phrase per vault, VK = 32 bytes from the module DRBG
- keyring: list of `(epoch u32, VK)`; revocation adds epoch+1
- per epoch:

```
K_obj = HKDF(VK, salt=00*32, info="osy obj-root v1" || epoch, 32)
K_bid = HKDF(VK, salt=00*32, info="osy bid v1"      || epoch, 32)
K_cfg = HKDF(VK, salt=00*32, info="osy cfg v1"      || epoch, 32)
```

- K_bid: client-side dedup index ids only, `HMAC(K_bid, content_sha256)`,
  NEVER sent to the server (stock docs say they encrypt file hashes
  deterministically for dedup; we deliberately do not expose that to
  the server)
- per object: `K_o` per spec.md §3.3 (`HKDF(K_obj, salt=obj_id, …)`)
- vault_id: 16 random bytes, stored in the envelope + local config

## 4. Device keys

- device signing key: random per device from the DRBG, private part in
  the OS keystore "daily" entry together with the VK keyring
- public key published in the cfg slot table (spec.md §4.4)
- pairing key K_p: 32 random bytes, one-time, lives only in the QR code

## 5. Storage map

- OS keystore daily entry (no prompt): VK keyring, device signing key
- OS keystore backup entry (user presence): S_bytes
- server `env` object under loc: AEAD(K_wrap, keyring, servers, tokens, pins)
- server: pub_destroy only (enrollment)
- never stored anywhere: R, K_wrap, K_rsig, K_destroy (re-derived from
  S on demand, zeroized after use)

## 6. Rotation / revocation

- revoke device: new VK epoch, new cfg gen with the slot state=2, new
  envelope gen; re-encrypt nothing old (old objects stay readable with
  old epochs, the revoked device already had them)
- revoked device held S → offer a new phrase: new S', new R', new
  envelope under loc'; old envelope DELETEd

## 7. Implementation (M5)

- per-epoch labels: `info = label || u32be(epoch)` (`sync_core::vault::epoch_keys`)
- cfg id `HMAC(K_cfg[epoch 0], "osy cfg id v1" || u32be gen)[0..16]`, envelope id
  `HMAC(loc, "osy env id v1" || u32be gen)[0..16]`, pairing reply id
  `HMAC(K_p, "osy pair reply v1" || u32be 0)[0..16]`
- daily entry (`Member::to_daily`): `u8 1 || id16 device_id || bytes
  PKCS#8 device key || str name || u32 server || bytes envelope body`
  (keyring + servers + tokens) — read without a prompt
- backup entry: the 22 `S_bytes`, read only after `Presence::confirm`
  (OS auth). `sync_core::keystore::FileKeystore` = headless/test backend
  (dir 0700, files 0600, tmp + fsync + rename); OS backends are a
  follow-up (deps.md §M5)
- a revocation is followed by the other devices only through the
  envelope (K_wrap): `NeedWords` → unlock backup entry → re-derive →
  newest envelope. Pairing therefore carries S_bytes by default.
