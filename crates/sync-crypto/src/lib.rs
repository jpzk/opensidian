// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! sync-crypto — the cipher suite and the `CryptoProvider` trait
//! (docs/sync/spec.md, key-tree.md). Callers depend on the trait only, so the
//! FIPS backend (aws-lc-rs, not added yet — docs/sync/cmvp.md) can be swapped
//! in without touching sync-core or sync-server.
#![forbid(unsafe_code)]
