/* vaultbleed B — ONE BACKEND PER VAULT. A second process on a vault that is
   already open in another window must not run a second index/watcher on it.

   Mechanism: an advisory flock(2) (std File::try_lock = flock LOCK_EX|LOCK_NB
   on Linux) on a lock file keyed by the CANONICAL vault path, taken inside
   bind_vault BEFORE the root is claimed and held for the life of the process
   (the fd lives in Vault.lock; the kernel drops the lock when the process dies,
   crash included — no stale-lock cleanup, no pid files). std opens with
   O_CLOEXEC, so a window we spawn never inherits our lock.

   Behaviour on contention, chosen: REFUSE. The argv/last-vault boot prints
   "already open in another window" and exits with DUP_EXIT; the boot picker
   and open_vault_window return the same message as an Err and the window stays
   as it was. Raising the existing window was rejected: it needs a cross-process
   channel (a socket per vault + an X11/Wayland activation token), i.e. a second
   piece of IPC that every sandbox (landlock, flatpak) must allow — more surface
   than the bug is worth. The user sees the message and the other window is
   still there.

   Where the lock lives (lock_dir):
     * flatpak: $XDG_RUNTIME_DIR/app/$FLATPAK_ID/locks. Inside a sandbox
       $XDG_RUNTIME_DIR (/run/user/<uid>) is a PRIVATE tmpfs per instance, so a
       lock there would never be seen by the window flatpak-spawn starts (a new
       sandbox). $XDG_RUNTIME_DIR/app/$FLATPAK_ID is the one runtime dir flatpak
       bind-mounts from the host into EVERY instance of the app (same inode), and
       flock is per inode, so two sandboxes contend correctly.
     * otherwise: $XDG_RUNTIME_DIR/opensidian/locks (tmpfs, per user, wiped at
       logout) when XDG_RUNTIME_DIR is an absolute dir; else
       ~/.local/share/dev.koto.opensidian/locks. Both are inside landlock's write
       roots (/run, ~/.local/share/dev.koto.opensidian — sandbox.rs), so a
       confined process can take its lock.
     slim / portable / AppImage all take the non-flatpak path; a flatpak and a
     non-flatpak build on the same vault do NOT see each other (different dirs).
     Known limit, stated: a vault reached through the document portal
     (/run/flatpak/doc/...) canonicalizes to a different path than the host
     path, so it keys a different lock.

   Key: FNV-1a-64 of the canonical path bytes, hex, plus the canonical path
   written into the file for humans (`cat` tells you which vault it is). */
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// exit status of a boot that found its vault already open elsewhere
pub const DUP_EXIT: i32 = 3;
pub const DUP_MSG: &str = "already open in another window";

/// the held lock; dropping it (process exit) releases the flock
#[derive(Debug)]
pub struct VaultLock {
    f: File,
    pub path: PathBuf,
}

#[derive(Debug)]
pub enum LockErr {
    /// another process holds this vault
    Busy(PathBuf),
    Io(String),
}

impl std::fmt::Display for LockErr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            LockErr::Busy(v) => write!(f, "{} is {DUP_MSG}", v.display()),
            LockErr::Io(e) => write!(f, "vault lock: {e}"),
        }
    }
}


impl VaultLock {
    /// item 14 (spawn.rs, the switch handshake): append `ready <token>` to the
    /// lock file through the fd this process already holds. Only the holder
    /// writes here, and acquire_in truncates, so a line from an earlier holder
    /// never survives into a new one.
    pub fn mark_ready(&self, token: &str) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom};
        let mut f = &self.f;
        f.seek(SeekFrom::End(0))?;
        f.write_all(format!("ready {token}\n").as_bytes())?;
        f.flush()
    }
}

/// has the holder of `canon`'s lock marked `token` ready? (the old window's
/// probe; a missing file / no line = not yet)
pub fn ready_in(dir: &Path, canon: &Path, token: &str) -> bool {
    let want = format!("ready {token}");
    fs::read_to_string(lock_file(dir, canon)).map(|s| s.lines().any(|l| l == want)).unwrap_or(false)
}

/// directory the lock files live in (pure: the caller reads the environment)
pub fn lock_dir(flatpak_id: Option<OsString>, xdg_runtime: Option<OsString>, home: &Path) -> PathBuf {
    let rt = xdg_runtime.map(PathBuf::from).filter(|p| p.is_absolute());
    match (flatpak_id.filter(|s| !s.is_empty()), rt) {
        (Some(id), Some(rt)) => rt.join("app").join(id).join("locks"),
        (None, Some(rt)) if rt.is_dir() => rt.join("opensidian").join("locks"),
        _ => home.join(".local/share/dev.koto.opensidian/locks"),
    }
}

pub fn env_lock_dir() -> PathBuf {
    let in_flatpak = Path::new("/.flatpak-info").exists();
    let id = if in_flatpak { std::env::var_os("FLATPAK_ID") } else { None };
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()));
    lock_dir(id, std::env::var_os("XDG_RUNTIME_DIR"), &home)
}

fn fnv1a64(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// lock file for a CANONICAL vault path
pub fn lock_file(dir: &Path, canon: &Path) -> PathBuf {
    dir.join(format!("vault-{:016x}.lock", fnv1a64(canon.as_os_str().as_bytes())))
}

fn open(dir: &Path, canon: &Path) -> Result<(File, PathBuf), LockErr> {
    fs::create_dir_all(dir).map_err(|e| LockErr::Io(format!("{}: {e}", dir.display())))?;
    let lf = lock_file(dir, canon);
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lf)
        .map_err(|e| LockErr::Io(format!("{}: {e}", lf.display())))?;
    Ok((f, lf))
}

/// take the vault's lock (non-blocking). `canon` MUST be canonicalized by the
/// caller: two spellings of one directory must key one lock.
pub fn acquire_in(dir: &Path, canon: &Path) -> Result<VaultLock, LockErr> {
    let (mut f, lf) = open(dir, canon)?;
    match f.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Err(LockErr::Busy(canon.to_path_buf())),
        Err(fs::TryLockError::Error(e)) => return Err(LockErr::Io(format!("{}: {e}", lf.display()))),
    }
    // label for humans; we own the lock, so nobody else writes it concurrently
    let _ = f.set_len(0).and_then(|_| f.write_all(canon.as_os_str().as_bytes())).and_then(|_| f.write_all(b"\n"));
    Ok(VaultLock { f, path: lf })
}

/// is the vault held by some OTHER open lock right now? (a probe for the UI's
/// "open in new window": refuse before spawning a child that would refuse.
/// Racy by nature; the child's own acquire stays the authority.)
pub fn held_in(dir: &Path, canon: &Path) -> bool {
    match open(dir, canon) {
        Ok((f, _)) => match f.try_lock() {
            Ok(()) => false, // dropping f releases it again
            Err(fs::TryLockError::WouldBlock) => true,
            Err(_) => false,
        },
        Err(_) => false,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// "the lock is free again" checks retry for up to 3 s: a sibling test that
    /// spawns a process forks our WHOLE fd table, and until that child execs
    /// (O_CLOEXEC) it shares the open file description that holds the flock.
    /// Observed in the gate (262 tests in parallel). Busy assertions stay strict.
    pub(crate) fn acquire_eventually(dir: &Path, canon: &Path) -> Result<VaultLock, LockErr> {
        let t0 = std::time::Instant::now();
        loop {
            match acquire_in(dir, canon) {
                Err(LockErr::Busy(_)) if t0.elapsed().as_secs() < 3 => std::thread::sleep(std::time::Duration::from_millis(25)),
                r => return r,
            }
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("vlock-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn vaultlock_second_holder_is_refused_until_the_first_drops() {
        let d = tmp("contend");
        let v = d.join("A");
        fs::create_dir_all(&v).unwrap();
        let canon = v.canonicalize().unwrap();
        let locks = d.join("locks");
        let first = acquire_in(&locks, &canon).expect("first acquire");
        // a second open file description contends exactly like a second process
        match acquire_in(&locks, &canon) {
            Err(LockErr::Busy(p)) => assert_eq!(p, canon),
            other => panic!("second acquire must be Busy, got {other:?}"),
        }
        assert!(held_in(&locks, &canon), "probe sees the held lock");
        assert!(LockErr::Busy(canon.clone()).to_string().contains(DUP_MSG));
        // a different vault is independent
        let b = d.join("B");
        fs::create_dir_all(&b).unwrap();
        let _lb = acquire_in(&locks, &b.canonicalize().unwrap()).expect("other vault is free");
        drop(first);
        let _again = acquire_eventually(&locks, &canon).expect("free again after the holder dropped");
        assert_eq!(fs::read_to_string(lock_file(&locks, &canon)).unwrap().trim_end(), canon.to_str().unwrap());
    }

    #[test]
    fn vaultlock_contention_across_processes() {
        // a real second PROCESS holds the lock: flock(1) from util-linux on the
        // same file, sleeping while we try.
        let d = tmp("proc");
        let v = d.join("A");
        fs::create_dir_all(&v).unwrap();
        let canon = v.canonicalize().unwrap();
        let locks = d.join("locks");
        fs::create_dir_all(&locks).unwrap();
        let lf = lock_file(&locks, &canon);
        let ready = d.join("ready");
        // sh takes the lock on fd 9 and EXECS sleep: the lock holder is the one
        // pid we kill (flock(1) as a parent would leave sh+sleep holding it).
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("exec 9>'{}'; flock -x 9; touch '{}'; exec sleep 30", lf.display(), ready.display()))
            .spawn()
            .expect("flock(1) present");
        let t0 = std::time::Instant::now();
        while !ready.exists() {
            assert!(t0.elapsed().as_secs() < 10, "flock child never got the lock");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let r = acquire_in(&locks, &canon);
        let _ = child.kill();
        let _ = child.wait();
        assert!(matches!(r, Err(LockErr::Busy(_))), "a lock held by another process must refuse, got {r:?}");
        acquire_eventually(&locks, &canon).expect("free once that process is gone");
    }

    #[test]
    fn vaultlock_dir_flatpak_uses_the_shared_per_app_runtime_dir() {
        let home = Path::new("/home/u");
        assert_eq!(
            lock_dir(Some("dev.koto.opensidian".into()), Some("/run/user/1000".into()), home),
            PathBuf::from("/run/user/1000/app/dev.koto.opensidian/locks")
        );
        // no usable runtime dir -> under HOME (also inside landlock's write roots)
        assert_eq!(lock_dir(None, None, home), PathBuf::from("/home/u/.local/share/dev.koto.opensidian/locks"));
        assert_eq!(lock_dir(None, Some("relative".into()), home), PathBuf::from("/home/u/.local/share/dev.koto.opensidian/locks"));
        assert_eq!(lock_dir(Some("".into()), Some("/nonexistent-rt-xyz".into()), home), PathBuf::from("/home/u/.local/share/dev.koto.opensidian/locks"));
        let rt = std::env::temp_dir();
        assert_eq!(lock_dir(None, Some(rt.clone().into()), home), rt.join("opensidian/locks"));
    }

    #[test]
    fn vaultlock_two_spellings_of_one_vault_key_one_lock() {
        let d = tmp("spell");
        let v = d.join("A");
        fs::create_dir_all(&v).unwrap();
        let via_dotdot = d.join("A/../A");
        let c1 = v.canonicalize().unwrap();
        let c2 = via_dotdot.canonicalize().unwrap();
        assert_eq!(lock_file(&d, &c1), lock_file(&d, &c2));
    }

    /// item 14: the holder's `ready <token>` is what the old window waits for;
    /// a new holder starts clean (a stale ready from the previous one never
    /// matches), and only the exact token counts.
    #[test]
    fn vaultlock_ready_marker_is_per_holder_and_per_token() {
        let d = tmp("ready");
        let v = d.join("B");
        fs::create_dir_all(&v).unwrap();
        let canon = v.canonicalize().unwrap();
        let locks = d.join("locks");
        assert!(!ready_in(&locks, &canon, "t1"), "no lock file yet");
        let l = acquire_in(&locks, &canon).unwrap();
        assert!(!ready_in(&locks, &canon, "t1"));
        l.mark_ready("t1").unwrap();
        assert!(ready_in(&locks, &canon, "t1"));
        assert!(!ready_in(&locks, &canon, "t"), "prefix of the token is not the token");
        assert!(!ready_in(&locks, &canon, "t2"));
        drop(l);
        let _l2 = acquire_eventually(&locks, &canon).unwrap();
        assert!(!ready_in(&locks, &canon, "t1"), "a new holder truncates the old ready line");
    }
}
