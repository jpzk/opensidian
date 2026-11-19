# Opensidian Sync — design docs

End-to-end encrypted, self-hostable sync. Clean room: behaviour taken
only from public stock Sync documentation; no stock code was read.
Status: draft, milestone M0 (spec) + M1 (local file history).

- [threat-model.md](threat-model.md) — assets, trust boundaries,
  adversaries, data-loss cases, residual risks
- [spec.md](spec.md) — versioning, canonical encoding, container /
  envelope byte layouts, records, `/v1` server API, local history store
- [key-tree.md](key-tree.md) — recovery phrase S, root R, vault key VK,
  device keys, rotation and revocation
- [test-vectors.md](test-vectors.md) — `osy-vec-1` vector file format
- [cmvp.md](cmvp.md) — the OpenSSL 3.5.9 FIPS provider, CMVP certificate
  #5586, box build, operational rules, packaging; unknowns marked
- [deps.md](deps.md) — every crate the sync work uses, pins, dates,
  checksums
- [open-questions.md](open-questions.md) — decisions still open

Code: `crates/sync-proto` (wire types), `crates/sync-crypto`
(CryptoProvider trait), `crates/sync-core` (file history, later the
engine), `crates/syncd`, `crates/sync-server` (stubs until M3/M6).

TODO (not in M1): restore/diff UI for file history. Backend commands
`history_*` exist and are tested; the planned panel is specified in
[spec.md §6.6](spec.md).
