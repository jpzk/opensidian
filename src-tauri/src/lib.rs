// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
/* opensidian_lib — the PURE parse/normalize modules, split out of the bin so
   the cargo-fuzz harness (jpzk/opensidian-harness, fuzz dir) can link them
   without the tauri runtime. No tauri, no
   window, no global app state lives here. main.rs re-imports each module at
   its crate root (`use opensidian_lib::vaultfs;`), so every existing
   `crate::vaultfs::…` path in the bin resolves to the same code it did before:
   the split moves compilation units, not behaviour. Add a module here ONLY if
   it is tauri-free and a fuzz target needs it. */
#![deny(unused_must_use)]

pub mod canvasfs;
pub mod datefmt;
pub mod graphq;
pub mod index;
pub mod outline;
pub mod vaultfs;
