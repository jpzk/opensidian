// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Storage: a SQLite index (vaults, hashed tokens, object rows, the per-namespace
//! list cursor) plus one file per object under `<data>/objects`. The server
//! stores ciphertext containers verbatim and never sees a key.
//!
//! Every method is blocking; the HTTP layer calls them from `spawn_blocking`.
//! One connection behind a mutex: the index is tiny and every write is a short
//! transaction, so a pool buys nothing here.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use sync_proto::api::{ErrorCode, ListItem, ObjKind};
use sync_proto::Id16;

/// Namespace used for `env` objects: they are addressed by the secret `loc`
/// alone (spec §5), so a fresh device that only knows the words can find them.
pub const ENV_NS: Id16 = Id16::ZERO;

/// A failed store operation, already mapped to the wire error code.
#[derive(Debug)]
pub struct StoreError {
    pub code: ErrorCode,
    pub detail: &'static str,
}

impl StoreError {
    pub fn new(code: ErrorCode, detail: &'static str) -> Self {
        StoreError { code, detail }
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(_: rusqlite::Error) -> Self {
        StoreError::new(ErrorCode::Internal, "index error")
    }
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::StorageFull || e.kind() == io::ErrorKind::QuotaExceeded {
            StoreError::new(ErrorCode::Quota, "server storage full")
        } else {
            StoreError::new(ErrorCode::Internal, "storage error")
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Result of a write-once PUT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    Created,
    Identical,
}

/// One stored vault (namespace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vault {
    pub ns: Id16,
    pub pub_destroy: Vec<u8>,
    pub key_alg: u8,
    pub quota: Option<u64>,
    pub used: u64,
}

/// Server-side state of a namespace (see [`Store::vault_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultState {
    Live,
    Destroyed,
    Unknown,
}

/// Token scope; write implies read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scope {
    Read,
    Write,
}

/// Object row as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjMeta {
    pub size: u64,
    pub sha256: [u8; 32],
    pub deleted: Option<i64>,
    pub expires: Option<i64>,
}

pub struct Store {
    conn: Mutex<Connection>,
    root: PathBuf,
}

fn kind_code(k: ObjKind) -> i64 {
    match k {
        ObjKind::Obj => 0,
        ObjKind::Cfg => 1,
        ObjKind::Env => 2,
        ObjKind::Pair => 3,
    }
}

fn kind_from_code(c: i64) -> Option<ObjKind> {
    match c {
        0 => Some(ObjKind::Obj),
        1 => Some(ObjKind::Cfg),
        2 => Some(ObjKind::Env),
        3 => Some(ObjKind::Pair),
        _ => None,
    }
}

fn id_from_blob(b: Vec<u8>) -> Result<Id16> {
    let a: [u8; 16] = b
        .try_into()
        .map_err(|_| StoreError::new(ErrorCode::Internal, "corrupt id"))?;
    Ok(Id16(a))
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS enroll(
  hash BLOB PRIMARY KEY, quota INTEGER, expires INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS vault(
  ns BLOB PRIMARY KEY, read_hash BLOB NOT NULL UNIQUE, write_hash BLOB NOT NULL UNIQUE,
  pub_destroy BLOB NOT NULL, key_alg INTEGER NOT NULL, quota INTEGER,
  used INTEGER NOT NULL DEFAULT 0, next_seq INTEGER NOT NULL DEFAULT 1,
  created INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS object(
  ns BLOB NOT NULL, kind INTEGER NOT NULL, id BLOB NOT NULL, seq INTEGER NOT NULL,
  size INTEGER NOT NULL, sha BLOB NOT NULL, deleted INTEGER, expires INTEGER,
  PRIMARY KEY(ns, kind, id));
CREATE INDEX IF NOT EXISTS object_seq ON object(ns, seq);
CREATE TABLE IF NOT EXISTS destroy_chal(
  hash BLOB PRIMARY KEY, ns BLOB NOT NULL, nonce BLOB NOT NULL, expires INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS destroy_chal_ns ON destroy_chal(ns);
CREATE TABLE IF NOT EXISTS destroyed(ns BLOB PRIMARY KEY, at INTEGER NOT NULL, receipt BLOB, chal BLOB);
";

/// Outstanding destroy challenges per vault (spam bound on a tokenless path).
pub const MAX_DESTROY_CHALLENGES: i64 = 16;

/// What a destroy removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Purged {
    pub objects: u64,
    pub bytes: u64,
}

impl Store {
    /// Open (or create) the store under `root` (mode 0700).
    pub fn open(root: &Path) -> Result<Store> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new().recursive(true).mode(0o700).create(root)?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        for sub in ["objects", "tmp"] {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(root.join(sub))?;
        }
        let conn = Connection::open(root.join("index.sqlite"))?;
        conn.execute_batch(SCHEMA)?;
        // Older indexes predate the stored receipt; the columns are additive.
        for col in ["receipt", "chal"] {
            let has: bool = conn
                .prepare("SELECT 1 FROM pragma_table_info('destroyed') WHERE name = ?1")?
                .exists([col])?;
            if !has {
                conn.execute_batch(&format!("ALTER TABLE destroyed ADD COLUMN {col} BLOB"))?;
            }
        }
        // A crash between the rename and the commit of a destroy leaves a
        // staged `destroy-<ns>` tree: finish the purge only if the tombstone
        // committed, otherwise the vault still exists and gets its files back.
        for e in fs::read_dir(root.join("tmp"))? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(hex) = name.strip_prefix("destroy-") else { continue };
            let committed = match Id16::from_hex(hex) {
                Ok(ns) => is_destroyed(&conn, &ns)?,
                Err(_) => true, // not ours to restore: no vault has this name
            };
            if committed {
                fs::remove_dir_all(e.path())?;
            } else {
                let dir = root.join("objects").join(hex);
                if dir.exists() {
                    // never overwrite: leave the staged tree for the operator
                    continue;
                }
                fs::rename(e.path(), &dir)?;
                sync_dir(&root.join("objects"))?;
            }
        }
        Ok(Store {
            conn: Mutex::new(conn),
            root: root.to_path_buf(),
        })
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        // A panic while holding the lock leaves SQLite consistent (transactions
        // roll back on drop), so a poisoned mutex is safe to keep using.
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn obj_path(&self, ns: &Id16, kind: ObjKind, id: &Id16) -> PathBuf {
        let h = id.to_hex();
        self.root
            .join("objects")
            .join(ns.to_hex())
            .join(kind.path_segment())
            .join(&h[..2])
            .join(h)
    }

    // ---- enrollment / vaults -------------------------------------------------

    /// Admin: register a one-time enrollment token (by hash).
    pub fn add_enrollment(&self, token_hash: &[u8; 32], quota: Option<u64>, expires: i64) -> Result<()> {
        self.conn().execute(
            "INSERT INTO enroll(hash, quota, expires) VALUES (?1, ?2, ?3)",
            params![&token_hash[..], quota.map(|q| q as i64), expires],
        )?;
        Ok(())
    }

    /// Consume an enrollment token and create the vault, atomically.
    #[allow(clippy::too_many_arguments)]
    pub fn enroll(
        &self,
        token_hash: &[u8; 32],
        now: i64,
        ns: &Id16,
        read_hash: &[u8; 32],
        write_hash: &[u8; 32],
        pub_destroy: &[u8],
        key_alg: u8,
    ) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let row: Option<(Option<i64>, i64)> = tx
            .query_row(
                "SELECT quota, expires FROM enroll WHERE hash = ?1",
                params![&token_hash[..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((quota, expires)) = row else {
            return Err(StoreError::new(ErrorCode::Unauthorized, "unknown enrollment token"));
        };
        tx.execute("DELETE FROM enroll WHERE hash = ?1", params![&token_hash[..]])?;
        if expires < now {
            tx.commit()?;
            return Err(StoreError::new(ErrorCode::Unauthorized, "enrollment token expired"));
        }
        tx.execute(
            "INSERT INTO vault(ns, read_hash, write_hash, pub_destroy, key_alg, quota, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![&ns.0[..], &read_hash[..], &write_hash[..], pub_destroy, key_alg as i64, quota, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Resolve a bearer token hash to its scope on `ns`, if any.
    pub fn token_scope(&self, ns: &Id16, token_hash: &[u8; 32]) -> Result<Option<Scope>> {
        let row: Option<(Vec<u8>, Vec<u8>)> = self
            .conn()
            .query_row(
                "SELECT read_hash, write_hash FROM vault WHERE ns = ?1",
                params![&ns.0[..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((rh, wh)) = row else { return Ok(None) };
        if sync_crypto::ct_eq(&wh, token_hash) {
            Ok(Some(Scope::Write))
        } else if sync_crypto::ct_eq(&rh, token_hash) {
            Ok(Some(Scope::Read))
        } else {
            Ok(None)
        }
    }

    pub fn vault(&self, ns: &Id16) -> Result<Option<Vault>> {
        let row = self
            .conn()
            .query_row(
                "SELECT pub_destroy, key_alg, quota, used FROM vault WHERE ns = ?1",
                params![&ns.0[..]],
                |r| {
                    Ok(Vault {
                        ns: *ns,
                        pub_destroy: r.get(0)?,
                        key_alg: r.get::<_, i64>(1)? as u8,
                        quota: r.get::<_, Option<i64>>(2)?.map(|q| q as u64),
                        used: r.get::<_, i64>(3)? as u64,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    // ---- objects --------------------------------------------------------------

    /// Write-once PUT. `sha256` is the digest of `body`. `expires` is set for
    /// short-lived kinds (pairing). Quota applies to vault namespaces; the
    /// tokenless `env` namespace has no owner, so it is bounded as a whole by
    /// `env_total` bytes instead.
    #[allow(clippy::too_many_arguments)]
    pub fn put(
        &self,
        ns: &Id16,
        kind: ObjKind,
        id: &Id16,
        body: &[u8],
        sha256: &[u8; 32],
        expires: Option<i64>,
        env_total: u64,
    ) -> Result<PutOutcome> {
        // Stage the bytes outside the lock (fsync'd), then publish under it.
        // Each PUT gets its own staging file (process-wide counter, created
        // exclusively): two concurrent PUTs of the same id must not truncate
        // or remove each other's bytes before publish renames them.
        static STAGE_SEQ: AtomicU64 = AtomicU64::new(0);
        let tmp = self.root.join("tmp").join(format!(
            "{}-{}-{}-{}-{}.part",
            ns.to_hex(),
            kind.path_segment(),
            id.to_hex(),
            std::process::id(),
            STAGE_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let staged = (|| -> io::Result<()> {
            let mut f = fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(body)?;
            f.sync_all()
        })();
        if let Err(e) = staged {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        let r = self.publish(ns, kind, id, &tmp, body.len() as u64, sha256, expires, env_total);
        let _ = fs::remove_file(&tmp);
        r
    }

    #[allow(clippy::too_many_arguments)]
    fn publish(
        &self,
        ns: &Id16,
        kind: ObjKind,
        id: &Id16,
        tmp: &Path,
        size: u64,
        sha256: &[u8; 32],
        expires: Option<i64>,
        env_total: u64,
    ) -> Result<PutOutcome> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let existing: Option<(Vec<u8>, Option<i64>)> = tx
            .query_row(
                "SELECT sha, deleted FROM object WHERE ns = ?1 AND kind = ?2 AND id = ?3",
                params![&ns.0[..], kind_code(kind), &id.0[..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((sha, deleted)) = existing {
            if deleted.is_some() {
                return Err(StoreError::new(ErrorCode::Gone, "object id was deleted"));
            }
            return if sync_crypto::ct_eq(&sha, sha256) {
                Ok(PutOutcome::Identical)
            } else {
                Err(StoreError::new(ErrorCode::Conflict, "different bytes under this id"))
            };
        }
        let seq: i64 = if *ns == ENV_NS {
            // Env objects are never deleted by clients, so the live sum is
            // the namespace's footprint; refuse past the server-wide cap.
            let used: i64 = tx.query_row(
                "SELECT COALESCE(SUM(size), 0) FROM object WHERE ns = ?1",
                params![&ns.0[..]],
                |r| r.get(0),
            )?;
            if (used.max(0) as u64).saturating_add(size) > env_total {
                return Err(StoreError::new(ErrorCode::Quota, "envelope storage full"));
            }
            0
        } else {
            let (quota, used, next): (Option<i64>, i64, i64) = tx
                .query_row(
                    "SELECT quota, used, next_seq FROM vault WHERE ns = ?1",
                    params![&ns.0[..]],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?
                .ok_or(StoreError::new(ErrorCode::NotFound, "unknown namespace"))?;
            if let Some(q) = quota {
                if used as u64 + size > q as u64 {
                    return Err(StoreError::new(ErrorCode::Quota, "vault quota exceeded"));
                }
            }
            tx.execute(
                "UPDATE vault SET used = used + ?2, next_seq = next_seq + 1 WHERE ns = ?1",
                params![&ns.0[..], size as i64],
            )?;
            next
        };
        tx.execute(
            "INSERT INTO object(ns, kind, id, seq, size, sha, expires) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![&ns.0[..], kind_code(kind), &id.0[..], seq, size as i64, &sha256[..], expires],
        )?;
        let dst = self.obj_path(ns, kind, id);
        let parent = dst
            .parent()
            .ok_or(StoreError::new(ErrorCode::Internal, "object path"))?;
        // The index must never name a file a crash can still lose: the rename
        // and every shard directory entry up to objects/ are fsync'd before
        // the commit — always, not only for dirs this call created (a failed
        // earlier PUT may have left them in place without their dir sync).
        let objects = self.root.join("objects");
        fs::create_dir_all(parent)?;
        fs::rename(tmp, &dst)?;
        sync_dir(parent)?;
        let mut d = parent;
        while d != objects {
            d = d.parent().ok_or(StoreError::new(ErrorCode::Internal, "object path"))?;
            sync_dir(d)?;
        }
        tx.commit()?; // on any error above the transaction rolls back
        Ok(PutOutcome::Created)
    }

    pub fn meta(&self, ns: &Id16, kind: ObjKind, id: &Id16) -> Result<Option<ObjMeta>> {
        let row = self
            .conn()
            .query_row(
                "SELECT size, sha, deleted, expires FROM object WHERE ns = ?1 AND kind = ?2 AND id = ?3",
                params![&ns.0[..], kind_code(kind), &id.0[..]],
                |r| {
                    let sha: Vec<u8> = r.get(1)?;
                    let mut a = [0u8; 32];
                    if sha.len() == 32 {
                        a.copy_from_slice(&sha);
                    }
                    Ok(ObjMeta {
                        size: r.get::<_, i64>(0)? as u64,
                        sha256: a,
                        deleted: r.get(2)?,
                        expires: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Read `len` bytes at `offset` of a stored object.
    pub fn read(&self, ns: &Id16, kind: ObjKind, id: &Id16, offset: u64, len: u64) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = fs::File::open(self.obj_path(ns, kind, id))?;
        f.seek(SeekFrom::Start(offset))?;
        let mut out = Vec::with_capacity(len as usize);
        f.take(len).read_to_end(&mut out)?;
        if out.len() as u64 != len {
            return Err(StoreError::new(ErrorCode::Internal, "short read"));
        }
        Ok(out)
    }

    /// Which of `ids` are not stored (live or soft-deleted) under `ns`.
    pub fn missing(&self, ns: &Id16, ids: &[Id16]) -> Result<Vec<Id16>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT 1 FROM object WHERE ns = ?1 AND kind = 0 AND id = ?2",
        )?;
        let mut out = Vec::new();
        for id in ids {
            let hit: Option<i64> = st
                .query_row(params![&ns.0[..], &id.0[..]], |r| r.get(0))
                .optional()?;
            if hit.is_none() {
                out.push(*id);
            }
        }
        Ok(out)
    }

    /// List live `obj`/`cfg` rows with `seq > after`, ascending, at most `limit`.
    pub fn list(&self, ns: &Id16, after: u64, limit: u32) -> Result<Vec<ListItem>> {
        let c = self.conn();
        let mut st = c.prepare(
            "SELECT seq, kind, id, size FROM object
             WHERE ns = ?1 AND seq > ?2 AND deleted IS NULL AND kind IN (0, 1)
             ORDER BY seq LIMIT ?3",
        )?;
        let rows = st.query_map(params![&ns.0[..], after as i64, limit as i64], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, kind, id, size) = row?;
            out.push(ListItem {
                seq: seq as u64,
                kind: kind_from_code(kind)
                    .ok_or(StoreError::new(ErrorCode::Internal, "corrupt kind"))?,
                id: id_from_blob(id)?,
                size: size as u64,
            });
        }
        Ok(out)
    }

    /// Soft delete: the row stays (readable with `?deleted=1`) until `gc`
    /// runs after `purge_at`. Returns false if there was no live object.
    pub fn soft_delete(&self, ns: &Id16, kind: ObjKind, id: &Id16, now: i64, purge_at: i64) -> Result<bool> {
        let n = self.conn().execute(
            "UPDATE object SET deleted = ?4, expires = ?5
             WHERE ns = ?1 AND kind = ?2 AND id = ?3 AND deleted IS NULL",
            params![&ns.0[..], kind_code(kind), &id.0[..], now, purge_at],
        )?;
        Ok(n == 1)
    }

    // ---- destroy (spec §5, 2-step) --------------------------------------------

    /// Step 1: record a challenge (`token_hash` = SHA-256 of the confirm token).
    /// Unknown vault -> 404, destroyed -> 410 (spec §5; a namespace is 128
    /// random bits, so the distinction is no practical oracle).
    pub fn add_destroy_challenge(
        &self,
        ns: &Id16,
        token_hash: &[u8; 32],
        nonce: &[u8; 32],
        now: i64,
        expires: i64,
    ) -> Result<()> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        if is_destroyed(&tx, ns)? {
            return Err(StoreError::new(ErrorCode::Gone, "vault destroyed"));
        }
        let known: Option<i64> = tx
            .query_row("SELECT 1 FROM vault WHERE ns = ?1", params![&ns.0[..]], |r| r.get(0))
            .optional()?;
        if known.is_none() || *ns == ENV_NS {
            return Err(StoreError::new(ErrorCode::NotFound, "unknown vault"));
        }
        tx.execute("DELETE FROM destroy_chal WHERE expires < ?1", params![now])?;
        let open: i64 = tx.query_row(
            "SELECT COUNT(*) FROM destroy_chal WHERE ns = ?1",
            params![&ns.0[..]],
            |r| r.get(0),
        )?;
        if open >= MAX_DESTROY_CHALLENGES {
            return Err(StoreError::new(ErrorCode::RateLimited, "too many open destroy challenges"));
        }
        tx.execute(
            "INSERT INTO destroy_chal(hash, ns, nonce, expires) VALUES (?1, ?2, ?3, ?4)",
            params![&token_hash[..], &ns.0[..], &nonce[..], expires],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Step 2a: consume a challenge (single use: removed whether or not the
    /// signature then verifies, so a token cannot be used to grind). Returns
    /// the server nonce and the vault's `pub_destroy`.
    pub fn take_destroy_challenge(&self, ns: &Id16, token_hash: &[u8; 32], now: i64) -> Result<([u8; 32], Vec<u8>)> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        if is_destroyed(&tx, ns)? {
            return Err(StoreError::new(ErrorCode::Gone, "vault destroyed"));
        }
        let row: Option<(Vec<u8>, i64, Vec<u8>)> = tx
            .query_row(
                "SELECT c.nonce, c.expires, v.pub_destroy FROM destroy_chal c
                 JOIN vault v ON v.ns = c.ns WHERE c.hash = ?1 AND c.ns = ?2",
                params![&token_hash[..], &ns.0[..]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((nonce, expires, pubk)) = row else {
            return Err(StoreError::new(ErrorCode::Unauthorized, "unknown confirm token"));
        };
        tx.execute("DELETE FROM destroy_chal WHERE hash = ?1", params![&token_hash[..]])?;
        tx.commit()?;
        if expires < now {
            return Err(StoreError::new(ErrorCode::Unauthorized, "destroy challenge expired"));
        }
        let nonce: [u8; 32] = nonce
            .try_into()
            .map_err(|_| StoreError::new(ErrorCode::Internal, "corrupt nonce"))?;
        Ok((nonce, pubk))
    }

    /// Step 2b (after the proof verified): purge every object, token and
    /// challenge of `ns` and leave a tombstone. Under the connection lock, so
    /// no PUT can publish into the namespace concurrently; the object tree is
    /// renamed away inside the transaction and deleted after the commit
    /// (`open` finishes the deletion if the process dies in between, or puts
    /// the tree back if the commit never happened).
    ///
    /// `sign` turns the purge counts into the signed wire receipt; it runs
    /// before the commit, so a signing failure purges nothing, and the
    /// receipt is stored with the tombstone under `chal` (the hash of the
    /// confirm token): a client that lost the response replays its confirm
    /// and gets the same receipt back ([`Store::destroy_receipt`]).
    pub fn destroy_vault(
        &self,
        ns: &Id16,
        now: i64,
        chal: &[u8; 32],
        sign: impl FnOnce(Purged) -> Option<Vec<u8>>,
    ) -> Result<(Purged, Vec<u8>)> {
        let mut c = self.conn();
        let tx = c.transaction()?;
        let (objects, bytes): (i64, i64) = tx.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM object WHERE ns = ?1",
            params![&ns.0[..]],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let purged = Purged {
            objects: objects as u64,
            bytes: bytes as u64,
        };
        tx.execute("DELETE FROM object WHERE ns = ?1", params![&ns.0[..]])?;
        tx.execute("DELETE FROM destroy_chal WHERE ns = ?1", params![&ns.0[..]])?;
        let n = tx.execute("DELETE FROM vault WHERE ns = ?1", params![&ns.0[..]])?;
        if n != 1 {
            return Err(StoreError::new(ErrorCode::Gone, "vault destroyed"));
        }
        let receipt = sign(purged).ok_or(StoreError::new(ErrorCode::Internal, "receipt signing failed"))?;
        tx.execute(
            "INSERT INTO destroyed(ns, at, receipt, chal) VALUES (?1, ?2, ?3, ?4)",
            params![&ns.0[..], now, &receipt, &chal[..]],
        )?;
        let dir = self.root.join("objects").join(ns.to_hex());
        let trash = self.root.join("tmp").join(format!("destroy-{}", ns.to_hex()));
        let moved = match fs::rename(&dir, &trash) {
            Ok(()) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        if moved {
            // the move out of objects/ is durable before the tombstone commits
            // (a crash then leaves the tree in tmp/, which `open` restores
            // unless the tombstone committed); a failed sync rolls back
            if let Err(e) = sync_dir(&self.root.join("objects")).and_then(|()| sync_dir(&self.root.join("tmp"))) {
                let _ = fs::rename(&trash, &dir);
                return Err(e.into());
            }
        }
        if let Err(e) = tx.commit() {
            if moved {
                let _ = fs::rename(&trash, &dir);
            }
            return Err(e.into());
        }
        if moved {
            // The tombstone is committed: a failed cleanup must not turn a
            // completed destroy into an error (`open` retries the removal).
            let _ = fs::remove_dir_all(&trash);
        }
        Ok((purged, receipt))
    }

    /// The receipt stored by a committed destroy of `ns`, if `chal` is the
    /// hash of the confirm token that destroyed it.
    pub fn destroy_receipt(&self, ns: &Id16, chal: &[u8; 32]) -> Result<Option<Vec<u8>>> {
        type Row = (Option<Vec<u8>>, Option<Vec<u8>>);
        let row: Option<Row> = self
            .conn()
            .query_row(
                "SELECT receipt, chal FROM destroyed WHERE ns = ?1",
                params![&ns.0[..]],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((Some(receipt), Some(h))) if sync_crypto::ct_eq(&h, chal) => Some(receipt),
            _ => None,
        })
    }

    /// True once `ns` was destroyed.
    pub fn destroyed(&self, ns: &Id16) -> Result<bool> {
        is_destroyed(&self.conn(), ns)
    }

    /// What the server knows about `ns`: a live vault, a destroyed one
    /// (tombstone) or nothing. The env namespace is never a vault.
    pub fn vault_state(&self, ns: &Id16) -> Result<VaultState> {
        let c = self.conn();
        if is_destroyed(&c, ns)? {
            return Ok(VaultState::Destroyed);
        }
        let known: Option<i64> =
            c.query_row("SELECT 1 FROM vault WHERE ns = ?1", params![&ns.0[..]], |r| r.get(0)).optional()?;
        Ok(if known.is_some() && *ns != ENV_NS { VaultState::Live } else { VaultState::Unknown })
    }

    /// Reclaim every object whose `expires` has passed (soft-deleted objects
    /// past retention, expired pairing objects). Returns the number reclaimed.
    ///
    /// An obj/cfg row stays as a tombstone (`deleted` set, `size` 0, no `expires`;
    /// single-use pairing rows are removed):
    /// a reclaimed id stays known to `missing` and can never be published
    /// again with different bytes. Each object is reclaimed in its own
    /// transaction that re-checks the row, so the quota is debited exactly
    /// once and only for an object this pass actually reclaimed; the file is
    /// unlinked after the commit (a crash in between leaves an unreferenced
    /// file, never a row without its bytes).
    pub fn gc(&self, now: i64) -> Result<usize> {
        let victims: Vec<(Vec<u8>, i64, Vec<u8>)> = {
            let c = self.conn();
            c.execute("DELETE FROM destroy_chal WHERE expires < ?1", params![now])?;
            let mut st = c.prepare(
                "SELECT ns, kind, id FROM object WHERE expires IS NOT NULL AND expires <= ?1",
            )?;
            let rows = st.query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut n = 0;
        for (ns, kind, id) in victims {
            let (ns, id) = (id_from_blob(ns)?, id_from_blob(id)?);
            let kind = kind_from_code(kind).ok_or(StoreError::new(ErrorCode::Internal, "corrupt kind"))?;
            let mut c = self.conn();
            let tx = c.transaction()?;
            let size: Option<i64> = tx
                .query_row(
                    "SELECT size FROM object WHERE ns = ?1 AND kind = ?2 AND id = ?3
                     AND expires IS NOT NULL AND expires <= ?4",
                    params![&ns.0[..], kind_code(kind), &id.0[..], now],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(size) = size else { continue }; // reclaimed by a concurrent pass
            if kind == ObjKind::Pair {
                // single-use pairing drops: purged entirely (spec §5), the
                // joiner only ever sees "not there"
                tx.execute(
                    "DELETE FROM object WHERE ns = ?1 AND kind = ?2 AND id = ?3",
                    params![&ns.0[..], kind_code(kind), &id.0[..]],
                )?;
            } else {
                tx.execute(
                    "UPDATE object SET deleted = COALESCE(deleted, ?4), expires = NULL, size = 0
                     WHERE ns = ?1 AND kind = ?2 AND id = ?3",
                    params![&ns.0[..], kind_code(kind), &id.0[..], now],
                )?;
            }
            tx.execute(
                "UPDATE vault SET used = MAX(used - ?2, 0) WHERE ns = ?1",
                params![&ns.0[..], size],
            )?;
            tx.commit()?;
            drop(c);
            match fs::remove_file(self.obj_path(&ns, kind, &id)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            n += 1;
        }
        Ok(n)
    }
}

fn is_destroyed(c: &Connection, ns: &Id16) -> Result<bool> {
    let r: Option<i64> = c
        .query_row("SELECT 1 FROM destroyed WHERE ns = ?1", params![&ns.0[..]], |r| r.get(0))
        .optional()?;
    Ok(r.is_some())
}

/// fsync a directory so a rename or create inside it survives a crash.
fn sync_dir(d: &Path) -> io::Result<()> {
    fs::File::open(d)?.sync_all()
}
