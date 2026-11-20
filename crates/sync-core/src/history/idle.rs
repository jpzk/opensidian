// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Debouncer for idle snapshots. Pure state machine over caller-supplied
//! millisecond timestamps (no threads, no clock): the backend calls
//! [`Debouncer::touch`] on every edit / watcher event and polls
//! [`Debouncer::due`] from a timer; each due path then gets
//! [`super::Store::snapshot_idle`]. Editors (ours on autosave, external ones
//! writing truncate+write bursts) settle before the snapshot is taken.

use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Debouncer {
    quiet_ms: u64,
    pending: BTreeMap<String, u64>,
}

impl Debouncer {
    /// `quiet_ms`: how long a path must see no activity before it is due.
    pub fn new(quiet_ms: u64) -> Self {
        Debouncer { quiet_ms, pending: BTreeMap::new() }
    }

    /// Record activity on `path` at `now_ms` (restarts its quiet window).
    pub fn touch(&mut self, path: &str, now_ms: u64) {
        self.pending.insert(path.to_string(), now_ms);
    }

    /// Forget a path (deleted / renamed away before it settled).
    pub fn cancel(&mut self, path: &str) {
        self.pending.remove(path);
    }

    /// Remove and return every path quiet for at least `quiet_ms`, sorted.
    pub fn due(&mut self, now_ms: u64) -> Vec<String> {
        let q = self.quiet_ms;
        let ready: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, t)| now_ms.saturating_sub(**t) >= q)
            .map(|(p, _)| p.clone())
            .collect();
        for p in &ready {
            self.pending.remove(p);
        }
        ready
    }

    /// Earliest time at which something becomes due (for timer scheduling).
    pub fn next_due(&self) -> Option<u64> {
        self.pending.values().min().map(|t| t.saturating_add(self.quiet_ms))
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}
