// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! opensidian-sync-server — self-host storage server (plan M3). Stores
//! ciphertext only, holds no keys. Stub: no network code yet.
#![forbid(unsafe_code)]

fn main() {
    eprintln!(
        "opensidian-sync-server: not implemented yet (API {})",
        sync_proto::API_VERSION
    );
    std::process::exit(2);
}
