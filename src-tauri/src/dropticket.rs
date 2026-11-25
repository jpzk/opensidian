// SPDX-License-Identifier: GPL-3.0-or-later
//! #280: a native drop is a CAPABILITY, not a list of paths the page may repeat.
//!
//! WHY. `attach_files` used to take the source paths from the page. Rust kept no record of
//! what the user had actually dropped, so any script in the page (a community plugin in a
//! trusted vault) could name any image or PDF outside the vault and have it moved (images)
//! or copied (PDFs) into the vault, with no dialog. `attach_one` checks the FILE (type,
//! size, regular, no symlink, magic); nothing checked that a drop had happened.
//!
//! WHAT. When the native drop transport (xdnd.rs: GTK on Linux, AppKit on macOS) accepts a
//! file drop, it hands the paths to `mint`, which stores them under a random one-shot id and
//! returns only that id; the page receives the id and the display names, never a path it
//! could reuse. `attach_files` takes the id and `take` consumes it: a ticket is good for ONE
//! import, in the window that received the drop, for TTL. Anything else — a raw path, an id
//! that was never minted, one already used, one expired, one from another window — is
//! refused before the vault or the source is touched (fail closed).
//!
//! The table is bounded (MAX): a page cannot grow it (only a native drop mints), and a burst
//! of drops evicts the oldest ticket instead of growing. Pure apart from /dev/urandom, so the
//! whole rule set is unit-tested below; the GTK/AppKit wiring is the gate phases' (osdrop,
//! dropticket).
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// how long a drop's ticket is good for: the page calls attach_files right after the drop
/// event (milliseconds); a minute covers a busy event loop and nothing more
pub const TTL: Duration = Duration::from_secs(60);
/// the most live tickets at once (one per drop not yet imported)
pub const MAX: usize = 16;
/// id entropy: 16 bytes = 128 bit, hex-encoded (32 chars)
const ID_BYTES: usize = 16;

/// why a ticket was refused. The page gets `say()`; nothing names a path.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TicketErr {
    /// not even the shape of an id (a raw path, a URL, empty, wrong length or alphabet)
    BadId,
    /// never minted, already used, or evicted
    Unknown,
    /// minted, but older than TTL (consumed now all the same)
    Expired,
    /// minted for another window's drop (consumed now all the same)
    OtherWindow,
    /// no randomness: no ticket is minted, the drop is not delivered
    NoRandom,
}

impl TicketErr {
    pub fn say(self) -> String {
        match self {
            TicketErr::BadId => "refused: attach_files takes the ticket of a real drop, not a path",
            TicketErr::Unknown => "refused: this drop ticket is unknown or was already used",
            TicketErr::Expired => "refused: this drop ticket expired — drop the file again",
            TicketErr::OtherWindow => "refused: this drop ticket belongs to another window",
            TicketErr::NoRandom => "the drop could not be registered (no randomness)",
        }
        .into()
    }
}

struct Entry {
    id: String,
    win: String,
    paths: Vec<PathBuf>,
    at: Instant,
}

/// the ticket table. One process-wide instance (TICKETS); tests make their own.
pub struct Table {
    m: Mutex<Vec<Entry>>,
}

impl Table {
    pub const fn new() -> Self {
        Table { m: Mutex::new(Vec::new()) }
    }

    /// store `paths` (as `canonical_source` made them) for window `win` under a fresh id
    pub fn mint_at(&self, win: &str, paths: Vec<PathBuf>, id: String, now: Instant) -> String {
        let mut t = self.m.lock().unwrap_or_else(|e| e.into_inner());
        // full: expired tickets go first, then the oldest live one
        if t.len() >= MAX {
            t.retain(|e| now.saturating_duration_since(e.at) < TTL);
        }
        while t.len() >= MAX {
            t.remove(0); // oldest first: entries are pushed in mint order
        }
        t.push(Entry { id: id.clone(), win: win.to_string(), paths, at: now });
        id
    }

    /// consume ticket `id` for window `win`: the paths, or why not. A ticket that is found
    /// is REMOVED whatever the outcome, so no id is ever honoured twice.
    pub fn take_at(&self, win: &str, id: &str, now: Instant) -> Result<Vec<PathBuf>, TicketErr> {
        if !is_id(id) {
            return Err(TicketErr::BadId);
        }
        let mut t = self.m.lock().unwrap_or_else(|e| e.into_inner());
        let i = t.iter().position(|e| ct_eq(e.id.as_bytes(), id.as_bytes())).ok_or(TicketErr::Unknown)?;
        let e = t.remove(i);
        if now.saturating_duration_since(e.at) >= TTL {
            return Err(TicketErr::Expired);
        }
        if e.win != win {
            return Err(TicketErr::OtherWindow);
        }
        Ok(e.paths)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.m.lock().unwrap().len()
    }
}

static TICKETS: Table = Table::new();

/// a fresh 128-bit id from the kernel, or None (then nothing is minted: fail closed)
fn fresh_id() -> Option<String> {
    use std::io::Read;
    let mut b = [0u8; ID_BYTES];
    std::fs::File::open("/dev/urandom").ok()?.read_exact(&mut b).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// exactly the shape fresh_id produces: 32 lowercase hex chars
fn is_id(s: &str) -> bool {
    s.len() == ID_BYTES * 2 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// constant-time byte comparison (an id is a secret the page holds)
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// the path stored for one dropped entry: an absolute local path gets its PARENT
/// canonicalized (so `..` and symlinked directories are resolved now, at drop time) while
/// the last component is kept as dropped — attach_one must still SEE a symlinked file to
/// refuse it (S1). Anything else (a remote entry of a mixed list, a parent that cannot be
/// resolved) is kept verbatim: attach_one refuses it with its own sentence, as before.
pub fn canonical_source(p: &Path) -> PathBuf {
    if !p.is_absolute() {
        return p.to_path_buf();
    }
    match (p.parent(), p.file_name()) {
        (Some(d), Some(n)) => d.canonicalize().map(|d| d.join(n)).unwrap_or_else(|_| p.to_path_buf()),
        _ => p.to_path_buf(),
    }
}

/// what the page may see of a drop: the display name of each entry, never its directory
pub fn display_names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string()))
        .collect()
}

/// called by the native drop transport ONLY: register a real drop for window `win`
pub fn mint(win: &str, paths: &[PathBuf]) -> Result<String, TicketErr> {
    let id = fresh_id().ok_or(TicketErr::NoRandom)?;
    let stored = paths.iter().map(|p| canonical_source(p)).collect();
    Ok(TICKETS.mint_at(win, stored, id, Instant::now()))
}

/// called by attach_files: consume the ticket the page was handed
pub fn take(win: &str, id: &str) -> Result<Vec<PathBuf>, TicketErr> {
    TICKETS.take_at(win, id, Instant::now())
}

/// tests elsewhere (main.rs, end to end through attach_ticket): a real ticket minted at `at`
#[cfg(test)]
pub fn mint_at_for_test(win: &str, paths: &[PathBuf], at: Instant) -> String {
    let id = fresh_id().expect("urandom");
    TICKETS.mint_at(win, paths.iter().map(|p| canonical_source(p)).collect(), id, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u8) -> String {
        format!("{:032x}", n as u128 + 0xabc0)
    }

    #[test]
    fn a_ticket_is_good_for_exactly_one_take() {
        let t = Table::new();
        let now = Instant::now();
        let k = t.mint_at("main", vec![PathBuf::from("/d/a.png")], id(1), now);
        assert_eq!(t.take_at("main", &k, now), Ok(vec![PathBuf::from("/d/a.png")]));
        assert_eq!(t.take_at("main", &k, now), Err(TicketErr::Unknown), "a reused ticket must be refused");
        assert_eq!(t.len(), 0);
    }

    #[test]
    fn an_expired_ticket_is_refused_and_gone() {
        let t = Table::new();
        let now = Instant::now();
        let k = t.mint_at("main", vec![PathBuf::from("/d/a.png")], id(2), now);
        assert_eq!(t.take_at("main", &k, now + TTL), Err(TicketErr::Expired));
        assert_eq!(t.take_at("main", &k, now), Err(TicketErr::Unknown), "an expired ticket is consumed too");
        // just inside the TTL is still good
        let k = t.mint_at("main", vec![PathBuf::from("/d/b.png")], id(3), now);
        assert!(t.take_at("main", &k, now + TTL - Duration::from_millis(1)).is_ok());
    }

    #[test]
    fn a_raw_path_or_any_non_id_is_refused_without_a_lookup() {
        let t = Table::new();
        let now = Instant::now();
        let _ = t.mint_at("main", vec![PathBuf::from("/d/a.png")], id(4), now);
        for bad in ["/home/u/Pictures/a.png", "file:///home/u/a.png", "", "../a.png", &id(4).to_uppercase(), &format!("{}0", id(4)), &id(4)[1..]] {
            assert_eq!(t.take_at("main", bad, now), Err(TicketErr::BadId), "{bad:?}");
        }
        assert_eq!(t.len(), 1, "a refused non-id consumes nothing");
        assert_eq!(t.take_at("main", &id(5), now), Err(TicketErr::Unknown), "a well-formed id nobody minted");
    }

    #[test]
    fn a_ticket_works_only_in_the_window_that_got_the_drop() {
        let t = Table::new();
        let now = Instant::now();
        let k = t.mint_at("main", vec![PathBuf::from("/d/a.png")], id(6), now);
        assert_eq!(t.take_at("vault-2", &k, now), Err(TicketErr::OtherWindow));
        assert_eq!(t.take_at("main", &k, now), Err(TicketErr::Unknown), "a wrong-window attempt burns the ticket");
    }

    #[test]
    fn the_table_is_bounded_and_evicts_the_oldest() {
        let t = Table::new();
        let now = Instant::now();
        let ks: Vec<String> = (0..MAX as u8 + 3).map(|i| t.mint_at("main", vec![], id(10 + i), now)).collect();
        assert_eq!(t.len(), MAX);
        assert_eq!(t.take_at("main", &ks[0], now), Err(TicketErr::Unknown), "the oldest was evicted");
        assert!(t.take_at("main", ks.last().unwrap(), now).is_ok());
        // a full table sweeps its expired entries on the next mint
        assert_eq!(t.len(), MAX - 1);
        let _ = t.mint_at("main", vec![], id(199), now);
        assert_eq!(t.len(), MAX);
        let _ = t.mint_at("main", vec![], id(200), now + TTL);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn minted_ids_are_128_bit_random_and_distinct() {
        let a = fresh_id().expect("urandom");
        let b = fresh_id().expect("urandom");
        assert!(is_id(&a) && is_id(&b));
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }

    #[test]
    fn the_stored_path_resolves_the_directory_but_not_the_file() {
        let base = std::env::temp_dir().join(format!("dropticket-canon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("real")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("dirlink")).unwrap();
        std::fs::write(base.join("real/a.png"), b"x").unwrap();
        std::os::unix::fs::symlink(base.join("real/a.png"), base.join("real/l.png")).unwrap();
        let creal = base.join("real").canonicalize().unwrap();
        assert_eq!(canonical_source(&base.join("dirlink/a.png")), creal.join("a.png"));
        assert_eq!(canonical_source(&base.join("real/../real/a.png")), creal.join("a.png"));
        // the symlinked FILE stays a symlink, so attach_one still refuses it
        assert_eq!(canonical_source(&base.join("real/l.png")), creal.join("l.png"));
        assert!(std::fs::symlink_metadata(creal.join("l.png")).unwrap().is_symlink());
        // a remote entry of a mixed list and a relative name pass through unchanged
        assert_eq!(canonical_source(Path::new("https://e.example/y.png")), PathBuf::from("https://e.example/y.png"));
        assert_eq!(display_names(&[creal.join("a.png"), PathBuf::from("https://e.example/y.png")]), vec!["a.png", "y.png"]);
        let _ = std::fs::remove_dir_all(&base);
    }
}
