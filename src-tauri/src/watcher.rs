// SPDX-License-Identifier: GPL-3.0-or-later
//! R11 external edits: std-only vault watcher. A background thread walks the
//! vault every TICK, snapshots (mtime, len) per note and diffs against the
//! previous snapshot. Candidates are then RECONCILED against the in-RAM Index
//! (index.rs) by content while holding the index lock: a file whose bytes
//! already equal the index is one of our own writes (write_note upserts the
//! index under the same lock before the lock is released) and produces no
//! event; anything else updates the index and is reported. A removed+added
//! pair with identical content is reported as a rename (stock Obsidian treats
//! it as delete+create — the UI does too, the pair is just not double-counted).
//! Frontend receives one tauri event `vault-changed` per tick with changes.
use crate::index::{notes_of, Index};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

pub const TICK_MS: u64 = 1000;

/// (mtime, len) — cheap change fingerprint; content decides in reconcile
pub type Meta = (SystemTime, u64);
pub type Snapshot = BTreeMap<String, Meta>;

#[derive(Debug, Default, PartialEq, Clone, Serialize)]
pub struct Diff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
}

#[derive(Debug, Default, PartialEq, Clone, Serialize)]
pub struct Change {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub modified: Vec<String>,
    /// (old, new) pairs — excluded from added/removed
    pub renamed: Vec<(String, String)>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty() && self.renamed.is_empty()
    }
}

pub fn note_file(root: &Path, name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.md", root.join(name).display()))
}

/// one walk + one stat per note (no reads). S2: lstat — walk already skipped
/// symlinks, a link swapped in between walk and stat must not be followed
pub fn snapshot(root: &Path) -> Snapshot {
    let mut out = Snapshot::new();
    for n in notes_of(root) {
        if let Ok(m) = fs::symlink_metadata(note_file(root, &n)) {
            if m.is_symlink() {
                continue;
            }
            let t = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.insert(n, (t, m.len()));
        }
    }
    out
}

/// pure set diff on fingerprints: names sorted (BTreeMap order)
pub fn diff(prev: &Snapshot, cur: &Snapshot) -> Diff {
    let mut d = Diff::default();
    for (n, m) in cur {
        match prev.get(n) {
            None => d.added.push(n.clone()),
            Some(pm) if pm != m => d.modified.push(n.clone()),
            _ => {}
        }
    }
    for n in prev.keys() {
        if !cur.contains_key(n) {
            d.removed.push(n.clone());
        }
    }
    d
}

/// disk -> index: apply the fingerprint diff by content. Caller holds the
/// index lock for the whole call, so a concurrent write_note (which writes +
/// upserts under that same lock) is either fully before or fully after us.
pub fn reconcile(root: &Path, d: &Diff, ix: &mut Index) -> Change {
    let mut c = Change::default();
    for n in &d.modified {
        let Ok(s) = fs::read_to_string(note_file(root, n)) else { continue };
        if ix.content(n) == Some(s.as_str()) {
            continue; // own write (or a touch): index already has these bytes
        }
        ix.upsert(n, &s);
        c.modified.push(n.clone());
    }
    let mut removed: Vec<String> = d.removed.iter().filter(|n| ix.content(n).is_some()).cloned().collect();
    for n in &d.added {
        let Ok(s) = fs::read_to_string(note_file(root, n)) else { continue };
        if ix.content(n) == Some(s.as_str()) {
            continue; // our own create/rename target
        }
        // rename: some vanished note carried exactly these bytes
        if let Some(i) = removed.iter().position(|r| ix.content(r) == Some(s.as_str())) {
            let old = removed.remove(i);
            ix.remove(&old);
            ix.upsert(n, &s);
            c.renamed.push((old, n.clone()));
            continue;
        }
        ix.upsert(n, &s);
        c.added.push(n.clone());
    }
    for n in &removed {
        ix.remove(n);
    }
    c.removed = removed;
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn meta(t: u64, len: u64) -> Meta {
        (SystemTime::UNIX_EPOCH + Duration::from_secs(t), len)
    }
    fn snap(v: &[(&str, u64, u64)]) -> Snapshot {
        v.iter().map(|(n, t, l)| (n.to_string(), meta(*t, *l))).collect()
    }
    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("rustidian-watch-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(p.join("sub")).unwrap();
        p
    }
    fn w(root: &Path, n: &str, s: &str) {
        fs::write(note_file(root, n), s).unwrap();
    }

    #[test]
    fn diff_added_removed_modified_unchanged() {
        let prev = snap(&[("A", 1, 10), ("B", 1, 5), ("C", 2, 7), ("sub/D", 3, 1)]);
        let cur = snap(&[("A", 1, 10), ("B", 4, 5), ("C", 2, 9), ("E", 5, 2)]);
        let d = diff(&prev, &cur);
        assert_eq!(d.added, vec!["E"]);
        assert_eq!(d.removed, vec!["sub/D"]);
        assert_eq!(d.modified, vec!["B", "C"]); // mtime-only and len-only both count
        assert_eq!(diff(&cur, &cur), Diff::default());
    }

    #[test]
    fn own_write_ignored_external_edit_reported() {
        let root = tmp("own");
        w(&root, "A", "alpha\n");
        w(&root, "B", "beta [[A]]\n");
        let mut ix = Index::build(&root);
        let s0 = snapshot(&root);
        // our own write path: index upsert + disk write (same bytes)
        std::thread::sleep(Duration::from_millis(20));
        w(&root, "A", "alpha own\n");
        ix.upsert("A", "alpha own\n");
        let s1 = snapshot(&root);
        let d = diff(&s0, &s1);
        assert_eq!(d.modified, vec!["A"]); // fingerprint moved...
        assert!(reconcile(&root, &d, &mut ix).is_empty()); // ...but content == index: silent
        // external edit: bytes differ from the index -> modified + index updated
        std::thread::sleep(Duration::from_millis(20));
        w(&root, "A", "alpha ext [[B]]\n");
        let s2 = snapshot(&root);
        let c = reconcile(&root, &diff(&s1, &s2), &mut ix);
        assert_eq!(c.modified, vec!["A"]);
        assert_eq!(ix.content("A"), Some("alpha ext [[B]]\n"));
        assert_eq!(ix.backlinks("B"), vec!["A"]); // edges follow the external edit
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn external_create_delete_rename() {
        let root = tmp("cdr");
        w(&root, "A", "alpha\n");
        let mut ix = Index::build(&root);
        let s0 = snapshot(&root);
        // create
        w(&root, "sub/New", "fresh [[A]]\n");
        let s1 = snapshot(&root);
        let c = reconcile(&root, &diff(&s0, &s1), &mut ix);
        assert_eq!(c.added, vec!["sub/New"]);
        assert_eq!(ix.names(), ["A", "sub/New"]);
        assert_eq!(ix.backlinks("A"), vec!["sub/New"]);
        // rename (mv keeps bytes): one renamed pair, nothing in added/removed
        fs::rename(note_file(&root, "sub/New"), note_file(&root, "Moved")).unwrap();
        let s2 = snapshot(&root);
        let c = reconcile(&root, &diff(&s1, &s2), &mut ix);
        assert_eq!(c.renamed, vec![("sub/New".to_string(), "Moved".to_string())]);
        assert!(c.added.is_empty() && c.removed.is_empty());
        assert_eq!(ix.names(), ["A", "Moved"]);
        assert_eq!(ix.backlinks("A"), vec!["Moved"]);
        // delete
        fs::remove_file(note_file(&root, "Moved")).unwrap();
        let s3 = snapshot(&root);
        let c = reconcile(&root, &diff(&s2, &s3), &mut ix);
        assert_eq!(c.removed, vec!["Moved"]);
        assert_eq!(ix.names(), ["A"]);
        assert!(ix.backlinks("A").is_empty());
        // own delete-equivalent: a name the index never had vanishing is silent
        let _ = fs::remove_dir_all(&root);
    }
}
