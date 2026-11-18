// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! sync-proto — the `/v1` wire types and the canonical binary encodings of
//! docs/sync/spec.md. Pure data: no I/O, no crypto, no network code.
#![forbid(unsafe_code)]

/// API path version this crate speaks (spec.md §1, "path" axis).
pub const API_VERSION: &str = "v1";
