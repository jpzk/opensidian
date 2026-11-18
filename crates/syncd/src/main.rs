// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! opensidian-syncd — client daemon / CLI shell around sync-core (plan M6).
//! Stub: no IPC, no network code yet.
#![forbid(unsafe_code)]

fn main() {
    eprintln!(
        "opensidian-syncd: not implemented yet (API {})",
        sync_core::sync_proto::API_VERSION
    );
    std::process::exit(2);
}
