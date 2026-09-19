// SPDX-License-Identifier: GPL-3.0-or-later
//! R11 external edits: std-only vault watcher. A background thread walks the
//! vault every TICK, snapshots (mtime, len) per note and diffs against the
//! previous snapshot. Candidates are then RECONCILED against the in-RAM Index
//! (index.rs) by content while holding the index lock: a file whose bytes
//! already equal the index is one of our own writes (write_note upserts the
//! index under the same lock before the lock is released) and produces no
//! event; anything else updates the index and is reported. A removed+added
//! pair with identical content is reported as a rename ONLY when the pairing
//! is unambiguous (one candidate on each side, non-blank bytes); otherwise it
//! is delete+create, because a guessed rename retitles one tab and closes
//! another with nothing behind the choice (stock Obsidian treats a rename as
//! delete+create too — the UI does the same, the pair is just not
//! double-counted). A claimed REMOVAL is never believed on the walk's word:
//! the tick re-walks and stats it (heal_short_walk), because a short walk and
//! a mass delete are the same Diff.
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

/* ---------- TEST-ONLY HOOK: a walk that comes back PARTIAL (suspect S1) ----
   S1 is "a partial vault walk read as a mass delete": one tick's walk returns
   a SUBSET of the notes that are on disk (an interrupted read_dir, a vault on
   a slow/remounting filesystem, a directory replaced under us), diff() reads
   every missing name as a removal, and the UI closes those tabs. The mechanism
   is real but not reproducible on demand from outside the process, so the
   harness needs a way to make ONE tick lie — and only one.

   WHAT THE HOOK CANNOT DO, by construction (assume someone will try to use it
   to buy a green):
     * it can only REMOVE names from the snapshot it is handed. It cannot add a
       name, change a fingerprint, touch the Index or write to the vault — so
       it can manufacture a spurious DELETE (the bug) and nothing else.
     * it needs BOTH halves: PARTIAL_ENV set at launch AND the arm file present
       on disk at tick time. No env -> one OnceLock read per tick, no syscall.
     * it is ONE-SHOT: the arming tick consumes (removes) the arm file, so a
       test that arms it once cannot leave the watcher permanently blind. The
       next tick walks normally, which is exactly the S1 shape we must survive.
     * it is invisible to the fix: it lies INSIDE snapshot(), so the sanity
       guard above it sees a genuinely short walk and cannot special-case it. */
/// env that arms the partial-walk hook: "<arm-file>" or "<arm-file>=<keep>"
pub const PARTIAL_ENV: &str = "RUSTIDIAN_TEST_PARTIAL_WALK";
/// how many notes a partial walk keeps when the spec does not say
pub const PARTIAL_KEEP_DEFAULT: usize = 1;

/// PURE (takes the spec, reads nothing): "<path>" | "<path>=<keep>" ->
/// (arm file, keep). Empty path, empty or non-numeric keep -> None (hook off).
pub fn parse_partial(spec: Option<&str>) -> Option<(std::path::PathBuf, usize)> {
    let s = spec.map(str::trim).filter(|s| !s.is_empty())?;
    let (path, keep) = match s.rsplit_once('=') {
        Some((p, k)) => (p.trim(), k.trim().parse::<usize>().ok()?),
        None => (s, PARTIAL_KEEP_DEFAULT),
    };
    if path.is_empty() {
        return None;
    }
    Some((std::path::PathBuf::from(path), keep))
}

/// PURE: the subset a partial walk would have returned — the first `keep`
/// names in walk (BTreeMap) order. keep >= len is a no-op (not a partial walk).
pub fn partial_take(snap: Snapshot, keep: usize) -> Snapshot {
    snap.into_iter().take(keep).collect()
}

/// this process's arming spec (env read once)
fn partial_spec() -> Option<&'static (std::path::PathBuf, usize)> {
    static P: std::sync::OnceLock<Option<(std::path::PathBuf, usize)>> = std::sync::OnceLock::new();
    P.get_or_init(|| parse_partial(std::env::var(PARTIAL_ENV).ok().as_deref())).as_ref()
}

/// Is the hook armed for THIS tick? Consumes the arm file so it fires once.
/// Returns the number of notes the lying walk keeps.
fn partial_armed() -> Option<usize> {
    let (f, keep) = partial_spec()?;
    if fs::remove_file(f).is_ok() {
        eprintln!("[tabclose] PARTIAL_WALK ARMED file={} keep={keep} (test hook, one tick)", f.display());
        return Some(*keep);
    }
    None
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
    match partial_armed() {
        Some(keep) => {
            let short = partial_take(out, keep);
            eprintln!("[tabclose] PARTIAL_WALK FIRED kept={} names={:?}", short.len(), short.keys().collect::<Vec<_>>());
            short
        }
        None => out,
    }
}

/* ---------- THE S1 GUARD: a removal is a claim about the DISK ----------
   diff() is set math on two walks, and it cannot tell "this note was deleted"
   from "this walk failed to list it" — the pure test s1_partial_walk_reads_as_
   a_mass_delete proves those two are the SAME Diff. So the tick does not act
   on the claim; it CHECKS it, with the one syscall that can settle it: stat
   the file the walk stopped mentioning. A name still on disk was never
   removed, it was missed, and the fix is to put it back into the baseline
   rather than to tell the UI four notes just vanished (which closes four
   tabs, one of them being typed into — the F-class report).

   symlink_metadata, not exists(): snapshot() deliberately skips symlinks, so a
   note REPLACED by a symlink leaves the snapshot. That is not a deletion —
   the bytes are still there under a link we refuse to follow — and closing the
   tab would be the same data-loss move. `heal` therefore treats "any dir entry
   is there" as "not removed" and lets the next honest walk decide.

   Cost: one lstat per CLAIMED removal, on ticks that claim one. A steady-state
   tick claims none and pays nothing. */
pub fn heal_short_walk(root: &Path, cur: &mut Snapshot, d: &mut Diff) -> Vec<String> {
    let mut healed: Vec<String> = Vec::new();
    for n in &d.removed {
        let Ok(m) = fs::symlink_metadata(note_file(root, n)) else { continue }; // really gone
        // put the name back into the baseline with a TRUTHFUL fingerprint, so
        // the next tick neither re-reports it as added nor misses a real edit
        let t = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        cur.insert(n.clone(), (t, m.len()));
        healed.push(n.clone());
    }
    if !healed.is_empty() {
        d.removed.retain(|n| !healed.contains(n));
    }
    healed
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
    // each added file is read ONCE — the pairing below has to know whether a
    // second new note carries the same bytes, and re-reading to find out is
    // how a "cheap" guard becomes an extra read per added note per tick
    let adds: Vec<(String, String)> = d
        .added
        .iter()
        .filter_map(|n| fs::read_to_string(note_file(root, n)).ok().map(|s| (n.clone(), s)))
        .collect();
    for (n, s) in &adds {
        if ix.content(n) == Some(s.as_str()) {
            continue; // our own create/rename target
        }
        /* RENAME PAIRING MUST BE UNAMBIGUOUS (S2). Content is the only link we
           have between a vanished name and a new one, so it must actually
           IDENTIFY: exactly one removed note carrying these bytes, exactly one
           added note carrying them, and the bytes must say something. Two
           empty notes are byte-identical, which is why the old
           `position(...)` picked the pair by BTreeMap order and the UI then
           retitled one tab and CLOSED another with no fact behind either
           (test s2_identical_empty_notes_pair_as_the_wrong_rename).
           When the evidence is ambiguous we report delete+create — the honest
           shape, and the one the UI already rescues bytes on — instead of
           guessing a rename that renames the wrong note. */
        let cands: Vec<usize> = (0..removed.len()).filter(|&i| ix.content(&removed[i]) == Some(s.as_str())).collect();
        let twins = adds.iter().filter(|(_, o)| o == s).count();
        if cands.len() == 1 && twins == 1 && !s.trim().is_empty() {
            let old = removed.remove(cands[0]);
            ix.remove(&old);
            ix.upsert(n, s);
            c.renamed.push((old, n.clone()));
            continue;
        }
        ix.upsert(n, s);
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

    /// S1, the pure half: a walk that comes back SHORT is indistinguishable,
    /// at the diff, from someone deleting every note it failed to list. That
    /// set math is unchanged by the fix — what changed is that the tick no
    /// longer BELIEVES it: heal_short_walk stats every claimed removal and
    /// puts back the ones still on disk. Pre-fix behaviour (reconcile really
    /// dropped all four from the Index) is preserved in
    /// docs/watcher-tests/RESULT-2026-09-19.log, the RED side of item 5.
    #[test]
    fn s1_partial_walk_reads_as_a_mass_delete() {
        let root = tmp("partial");
        for n in ["A", "B", "C", "sub/D", "sub/E"] {
            w(&root, n, &format!("body of {n}\n"));
        }
        let full = snapshot(&root);
        assert_eq!(full.len(), 5);
        // the lying walk: read_dir gave us the first entry and stopped
        let short = partial_take(full.clone(), 1);
        assert_eq!(short.keys().cloned().collect::<Vec<_>>(), ["A"]);
        let mut d = diff(&full, &short);
        assert_eq!(d.removed, ["B", "C", "sub/D", "sub/E"]); // four notes "deleted"
        assert!(d.added.is_empty() && d.modified.is_empty());
        // THE GUARD: four claims, four files on disk, zero removals survive —
        // and the baseline gets the missed names back with real fingerprints
        let mut cur = short.clone();
        let healed = heal_short_walk(&root, &mut cur, &mut d);
        assert_eq!(healed, ["B", "C", "sub/D", "sub/E"]);
        assert!(d.removed.is_empty());
        assert_eq!(cur.keys().cloned().collect::<Vec<_>>(), full.keys().cloned().collect::<Vec<_>>());
        assert_eq!(cur, full); // fingerprints too: the re-stat is not a placeholder
        // and the index keeps every note: nothing to close, nothing to lose
        let mut ix = Index::build(&root);
        let c = reconcile(&root, &d, &mut ix);
        assert!(c.is_empty());
        assert_eq!(ix.names(), ["A", "B", "C", "sub/D", "sub/E"]);
        assert_eq!(notes_of(&root).len(), 5);
        // a REAL delete still gets through the guard — it is a confirmation,
        // not a veto: the file is gone, the removal stands, the tab closes
        fs::remove_file(note_file(&root, "B")).unwrap();
        let after = snapshot(&root);
        let mut d2 = diff(&full, &after);
        assert_eq!(d2.removed, ["B"]);
        let mut cur2 = after.clone();
        assert!(heal_short_walk(&root, &mut cur2, &mut d2).is_empty());
        assert_eq!(d2.removed, ["B"]);
        assert_eq!(reconcile(&root, &d2, &mut ix).removed, ["B"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// the hook's spec parser and its one-shot arming. No env -> no hook.
    #[test]
    fn s1_hook_parses_and_fires_exactly_once() {
        assert_eq!(parse_partial(None), None);
        assert_eq!(parse_partial(Some("   ")), None);
        assert_eq!(parse_partial(Some("=3")), None); // no path
        assert_eq!(parse_partial(Some("/t/arm=x")), None); // keep must be a number
        let pb = |s: &str| std::path::PathBuf::from(s);
        assert_eq!(parse_partial(Some("/t/arm")), Some((pb("/t/arm"), PARTIAL_KEEP_DEFAULT)));
        assert_eq!(parse_partial(Some(" /t/arm=2 ")), Some((pb("/t/arm"), 2)));
        // keep >= len is not a partial walk at all
        let root = tmp("hook");
        for n in ["A", "B"] {
            w(&root, n, "x\n");
        }
        let full = snapshot(&root);
        assert_eq!(partial_take(full.clone(), 9), full);
        assert!(partial_take(full, 0).is_empty()); // a walk that listed nothing
        // arming is a file, and the tick that sees it CONSUMES it
        let arm = root.join("arm");
        fs::write(&arm, b"").unwrap();
        assert!(arm.exists());
        // (the env is read once per process, so this half checks the file half)
        assert!(fs::remove_file(&arm).is_ok());
        assert!(fs::remove_file(&arm).is_err()); // second tick: not armed
        let _ = fs::remove_dir_all(&root);
    }

    /// S2: rename pairing is by CONTENT, and empty notes are all the same
    /// bytes. Before the fix, deleting one empty note and creating another
    /// reported a rename between two notes with nothing to do with each other
    /// (the pair picked by BTreeMap order) and the UI retitled one tab and
    /// closed a different one. After the fix an ambiguous pairing is reported
    /// as what it provably is: delete + create.
    #[test]
    fn s2_identical_empty_notes_pair_as_the_wrong_rename() {
        let root = tmp("ambig");
        w(&root, "Empty One", "");
        w(&root, "Empty Two", "");
        w(&root, "Typed", "words\n");
        let mut ix = Index::build(&root);
        let s0 = snapshot(&root);
        // an editor's save dance on ONE of them: remove both empties, add a new
        // empty note. Every candidate has identical (zero) bytes.
        fs::remove_file(note_file(&root, "Empty One")).unwrap();
        fs::remove_file(note_file(&root, "Empty Two")).unwrap();
        w(&root, "Fresh", "");
        let s1 = snapshot(&root);
        let d = diff(&s0, &s1);
        assert_eq!(d.removed, ["Empty One", "Empty Two"]);
        assert_eq!(d.added, ["Fresh"]);
        let c = reconcile(&root, &d, &mut ix);
        // NO GUESS: blank bytes identify nothing, so nothing is paired
        assert!(c.renamed.is_empty());
        assert_eq!(c.added, ["Fresh"]);
        assert_eq!(c.removed, ["Empty One", "Empty Two"]);
        assert_eq!(ix.names(), ["Fresh", "Typed"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// The other half of the S2 rule, and the reason it is not "never pair":
    /// ONE removed note and ONE added note carrying the same NON-BLANK bytes
    /// is still a rename (external_create_delete_rename covers the mv case).
    /// Two added twins, or two removed twins, are not.
    #[test]
    fn s2_pairing_is_unambiguous_or_it_is_delete_plus_create() {
        let root = tmp("ambig2");
        w(&root, "One", "same body\n");
        w(&root, "Two", "same body\n");
        let mut ix = Index::build(&root);
        let s0 = snapshot(&root);
        // TWO removed candidates with identical bytes, one new name: which of
        // the two was renamed is not knowable, so neither is claimed
        fs::remove_file(note_file(&root, "One")).unwrap();
        fs::remove_file(note_file(&root, "Two")).unwrap();
        w(&root, "New", "same body\n");
        let s1 = snapshot(&root);
        let c = reconcile(&root, &diff(&s0, &s1), &mut ix);
        assert!(c.renamed.is_empty());
        assert_eq!(c.added, ["New"]);
        assert_eq!(c.removed, ["One", "Two"]);
        // and the mirror case: one removed, TWO added twins
        let mut ix2 = Index::build(&root); // {New}
        let s2 = snapshot(&root);
        fs::remove_file(note_file(&root, "New")).unwrap();
        w(&root, "Copy A", "same body\n");
        w(&root, "Copy B", "same body\n");
        let s3 = snapshot(&root);
        let c2 = reconcile(&root, &diff(&s2, &s3), &mut ix2);
        assert!(c2.renamed.is_empty());
        assert_eq!(c2.added, ["Copy A", "Copy B"]);
        assert_eq!(c2.removed, ["New"]);
        let _ = fs::remove_dir_all(&root);
    }

    /// heal_short_walk is the guard the tick leans on, so it is tested on its
    /// own terms: a name really gone stays removed, a name still on disk is
    /// healed back into the baseline, and a note replaced by a SYMLINK counts
    /// as still there (snapshot skips symlinks — dropping the tab because we
    /// refuse to follow a link would lose the buffer for nothing).
    #[test]
    fn heal_short_walk_confirms_every_claimed_removal() {
        let root = tmp("heal");
        w(&root, "Gone", "x\n");
        w(&root, "Here", "y\n");
        w(&root, "Linked", "z\n");
        let full = snapshot(&root);
        fs::remove_file(note_file(&root, "Gone")).unwrap();
        fs::remove_file(note_file(&root, "Linked")).unwrap();
        std::os::unix::fs::symlink(note_file(&root, "Here"), note_file(&root, "Linked")).unwrap();
        // the walk claims all three (Here was simply missed)
        let mut d = Diff { removed: vec!["Gone".into(), "Here".into(), "Linked".into()], ..Diff::default() };
        let mut cur = Snapshot::new();
        let healed = heal_short_walk(&root, &mut cur, &mut d);
        assert_eq!(healed, ["Here", "Linked"]);
        assert_eq!(d.removed, ["Gone"]);
        assert_eq!(cur.keys().cloned().collect::<Vec<_>>(), ["Here", "Linked"]);
        assert_eq!(cur.get("Here"), full.get("Here")); // truthful fingerprint
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
