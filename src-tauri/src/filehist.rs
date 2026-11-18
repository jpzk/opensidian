// SPDX-License-Identifier: GPL-3.0-or-later
//! Sync M1 — local file history (sync-core `history::Store`) wired into the
//! app. docs/sync/spec.md §6.
//!
//! ONE store per process (the vault root is a OnceLock, a switch is refused),
//! opened lazily on the first hook, kept under
//! `<app data>/history/<vault id>/` — OUTSIDE the vault, so it is never
//! indexed, synced or committed with it.
//!
//! BEST EFFORT, ON PURPOSE. History is a safety net under the save path, not a
//! gate in front of it: a full or read-only app-data disk must not turn every
//! autosave into an error (that would lose the user's edits to protect older
//! copies of them). Every hook failure is logged with `[history]` and swallowed;
//! a store that fails to open is switched OFF for the session (one log line,
//! not one per keystroke).
//!
//! LOCK ORDER: the index mutex (main.rs Vault.index) BEFORE this one. Hooks
//! run inside the note writers, which hold the index lock; the watcher calls
//! in only after it has released it. Nothing here ever takes the index lock.
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use sync_core::history::{Config, HistoryError, Outcome, Retention, Store};

/// Where the store of `vault` lives: (app data dir, vault id). `None` =
/// history disabled for this vault.
pub type Locate<'a> = &'a dyn Fn(&Path) -> Option<(PathBuf, String)>;

enum State {
    Closed,
    Open { vault: PathBuf, store: Store },
    /// open failed / disabled for this vault: do not retry every save
    Off { vault: PathBuf },
}

pub struct FileHist {
    st: Mutex<State>,
}

impl Default for FileHist {
    fn default() -> Self {
        FileHist::new()
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Vault-relative path as the store keys it (`/` separated). `None` for
/// anything that is not a plain relative path.
pub fn key(rel: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?),
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl FileHist {
    pub const fn new() -> Self {
        FileHist { st: Mutex::new(State::Closed) }
    }

    /// Run `f` on the store of `vault`, opening it on first use (and thinning
    /// it once with the default retention). `None` = history unavailable.
    pub fn with<R>(&self, vault: &Path, locate: Locate, f: impl FnOnce(&mut Store) -> R) -> Option<R> {
        let mut g = self.st.lock().unwrap_or_else(|p| p.into_inner());
        let same = match &*g {
            State::Open { vault: v, .. } | State::Off { vault: v } => v == vault,
            State::Closed => false,
        };
        if !same {
            *g = match locate(vault) {
                None => State::Off { vault: vault.to_path_buf() },
                Some((data, id)) => match Store::open(&data, &id, vault, Config::default()) {
                    Ok(mut store) => {
                        if let Err(e) = store.thin(now_ms(), &Retention::default()) {
                            eprintln!("[history] thin on open: {e}");
                        }
                        State::Open { vault: vault.to_path_buf(), store }
                    }
                    Err(e) => {
                        eprintln!("[history] store unavailable, history OFF for this session: {e}");
                        State::Off { vault: vault.to_path_buf() }
                    }
                },
            };
        }
        match &mut *g {
            State::Open { store, .. } => Some(f(store)),
            _ => None,
        }
    }

    /// A hook on one vault-relative file: logs and swallows every error.
    pub fn hook(
        &self,
        vault: &Path,
        locate: Locate,
        what: &str,
        rel: &Path,
        f: impl FnOnce(&mut Store, &str, u64) -> Result<Outcome, HistoryError>,
    ) {
        let Some(k) = key(rel) else { return };
        if let Some(Err(e)) = self.with(vault, locate, |s| f(s, &k, now_ms())) {
            eprintln!("[history] {what} {k:?}: {e}");
        }
    }

    pub fn pre_write(&self, vault: &Path, locate: Locate, rel: &Path) {
        self.hook(vault, locate, "pre-write", rel, |s, k, t| s.snapshot_before_overwrite(k, t));
    }

    pub fn own_save(&self, vault: &Path, locate: Locate, rel: &Path) {
        self.hook(vault, locate, "own-save", rel, |s, k, t| s.record_own_save(k, t));
    }

    pub fn pre_delete(&self, vault: &Path, locate: Locate, rel: &Path) {
        self.hook(vault, locate, "pre-delete", rel, |s, k, t| s.snapshot_before_delete(k, t));
    }

    pub fn external(&self, vault: &Path, locate: Locate, rel: &Path) {
        self.hook(vault, locate, "external", rel, |s, k, t| s.record_external(k, t));
    }

    /// Call AFTER the move is a fact on disk.
    pub fn renamed(&self, vault: &Path, locate: Locate, from: &Path, to: &Path) {
        let Some(fk) = key(from) else { return };
        self.hook(vault, locate, "rename", to, |s, k, t| s.record_rename(&fk, k, t));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use sync_core::history::{Head, Reason};

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("osy-filehist-{tag}-{}-{}", std::process::id(), now_ms()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("vault")).unwrap();
        fs::create_dir_all(d.join("data")).unwrap();
        d
    }

    fn reasons(h: &FileHist, v: &Path, loc: Locate, k: &str) -> Vec<Reason> {
        h.with(v, loc, |s| s.versions(k).iter().map(|e| e.reason).collect()).unwrap()
    }

    #[test]
    fn save_snapshots_previous_bytes_then_records_the_new_ones() {
        let d = tmp("save");
        let (v, data) = (d.join("vault"), d.join("data"));
        let loc = |_: &Path| Some((data.clone(), "v1".to_string()));
        let h = FileHist::new();
        let rel = Path::new("sub/N.md");
        fs::create_dir_all(v.join("sub")).unwrap();
        fs::write(v.join(rel), "old").unwrap();
        h.pre_write(&v, &loc, rel);
        fs::write(v.join(rel), "new").unwrap();
        h.own_save(&v, &loc, rel);
        // versions() is newest first
        assert_eq!(reasons(&h, &v, &loc, "sub/N.md"), vec![Reason::OwnSave, Reason::PreWrite]);
        let old = h.with(&v, &loc, |s| s.read_version(s.versions("sub/N.md")[1].seq).unwrap()).unwrap();
        assert_eq!(old, b"old");
        // the store lives under <data>/history/<id>, not in the vault
        assert!(data.join("history/v1/index.log").is_file());
        assert!(!v.join("history").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn delete_rename_and_external_edit_are_journaled() {
        let d = tmp("ops");
        let (v, data) = (d.join("vault"), d.join("data"));
        let loc = |_: &Path| Some((data.clone(), "v2".to_string()));
        let h = FileHist::new();
        fs::write(v.join("A.md"), "a").unwrap();
        h.external(&v, &loc, Path::new("A.md"));
        fs::write(v.join("A.md"), "a2").unwrap();
        h.external(&v, &loc, Path::new("A.md"));
        fs::rename(v.join("A.md"), v.join("B.md")).unwrap();
        h.renamed(&v, &loc, Path::new("A.md"), Path::new("B.md"));
        h.pre_delete(&v, &loc, Path::new("B.md"));
        fs::remove_file(v.join("B.md")).unwrap();
        assert_eq!(reasons(&h, &v, &loc, "A.md"), vec![Reason::External, Reason::External]);
        let deleted = h.with(&v, &loc, |s| s.deleted()).unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].path, "B.md");
        assert!(matches!(h.with(&v, &loc, |s| s.head("B.md").cloned()).unwrap(), Some(Head::Deleted { .. })));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn disabled_or_unopenable_store_never_fails_a_hook() {
        let d = tmp("off");
        let v = d.join("vault");
        let h = FileHist::new();
        fs::write(v.join("N.md"), "x").unwrap();
        // disabled
        let none = |_: &Path| None;
        h.pre_write(&v, &none, Path::new("N.md"));
        assert!(h.with(&v, &none, |_| ()).is_none());
        // a store root inside the vault is refused -> OFF, hook is a no-op
        let h2 = FileHist::new();
        let inside = |_: &Path| Some((v.clone(), "v3".to_string()));
        h2.own_save(&v, &inside, Path::new("N.md"));
        assert!(h2.with(&v, &inside, |_| ()).is_none());
        assert!(!v.join("history").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn key_is_slash_joined_and_rejects_non_relative() {
        assert_eq!(key(Path::new("a/b.md")).as_deref(), Some("a/b.md"));
        assert_eq!(key(Path::new("/abs.md")), None);
        assert_eq!(key(Path::new("../x.md")), None);
        assert_eq!(key(Path::new("")), None);
    }
}
