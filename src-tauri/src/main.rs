#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
// This program comes with ABSOLUTELY NO WARRANTY. It is free software, and you
// are welcome to redistribute it under the terms of the GNU GPL version 3 or
// (at your option) any later version. See LICENSE, or <https://www.gnu.org/licenses/>.
// Thin desktop entry: the app is the opensidian_lib crate (src/lib.rs); Android
// enters the same run() through tauri's mobile entry point.
fn main() {
    opensidian_lib::run()
}
