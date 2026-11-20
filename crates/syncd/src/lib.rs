// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! opensidian-syncd: the client side of sync around sync-core (plan M6) —
//! [`api::SyncApi`] with an in-process [`local::LocalEngine`] and an
//! [`ipc::IpcClient`] for the user-service daemon, plus the vault-folder
//! mapping ([`workdir`]). The Tauri backend embeds `LocalEngine`.
#![forbid(unsafe_code)]

pub mod api;
#[cfg(unix)]
pub mod ipc;
pub mod local;
pub mod workdir;
