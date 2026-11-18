// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! sync-core — the sync engine library. M1 scope: local file history
//! (content-addressed store OUTSIDE the vault, docs/sync/spec.md §6);
//! M2b: object containers — seal/open with zstd ([`object`], spec §3);
//! M3c: replica transports — HTTP(S) client + dir replica ([`transport`], spec §5).
#![forbid(unsafe_code)]

pub use sync_crypto;
pub use sync_proto;

pub mod history;
pub mod object;
pub mod transport;
