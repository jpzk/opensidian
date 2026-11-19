// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Directory replica (spec §5 semantics on a plain filesystem): a mounted
//! drive, NAS share or folder another tool copies around. Layout:
//!
//! ```text
//! <root>/osy-replica-v1            marker
//! <root>/<ns>/<kind>/<id>          object bytes (write-once)
//! <root>/<ns>/del/<kind>-<id>      tombstone (soft delete; id never reusable)
//! <root>/<ns>/log                  append-only list cursor, 25-byte records
//! <root>/<ns>/lock                 exclusive lock for put/delete
//! <root>/<ns>/tmp/                 staging
//! ```
//!
//! Every path component is a fixed kind word or 32 lowercase hex digits, so
//! no caller value reaches the filesystem unparsed. Writes are staged,
//! fsynced and hard-linked into place (link fails if the name exists, so a
//! racing writer cannot replace an object). The log record is appended
//! under the per-namespace lock, so `seq` is unique across processes.
//! Differences from the server: no tokens, no enrollment, no destroy (a
//! dir replica has no authority to attest a purge), no retention GC.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sync_proto::api::{
    DestroyChallenge, DestroyConfirm, DestroyReceipt, EnrollResponse, ErrorCode, Health, ListItem, ListQuery,
    ListResponse, ObjKind, DEFAULT_MAX_OBJECT, MAX_MISSING_IDS, PROTO_VERSIONS,
};
use sync_proto::container::Header;
use sync_proto::Id16;

use super::{Fetched, PutOutcome, Range, Result, Transport, TransportError};

/// Marker file name at the replica root.
pub const MARKER: &str = "osy-replica-v1";
/// Bytes per log record: u8 kind || id16 || u64 size (BE).
pub const RECORD: usize = 1 + 16 + 8;
/// Poll interval of a long-poll `list` (filesystems have no portable notify).
pub const POLL: Duration = Duration::from_millis(50);

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct FsTransport {
    root: PathBuf,
    max_object: u64,
}

fn io(e: std::io::Error) -> TransportError {
    TransportError::Io(e.to_string())
}

fn kind_byte(k: ObjKind) -> u8 {
    match k {
        ObjKind::Obj => 0,
        ObjKind::Cfg => 1,
        ObjKind::Env => 2,
        ObjKind::Pair => 3,
    }
}

fn byte_kind(b: u8) -> Option<ObjKind> {
    match b {
        0 => Some(ObjKind::Obj),
        1 => Some(ObjKind::Cfg),
        2 => Some(ObjKind::Env),
        3 => Some(ObjKind::Pair),
        _ => None,
    }
}

/// Exclusive per-namespace lock, released on drop.
struct NsLock(File);

impl Drop for NsLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl FsTransport {
    /// Open (or initialise) a replica at `root`. An existing non-empty
    /// directory without the marker is refused, so a typo cannot scatter
    /// objects into an unrelated folder.
    pub fn open(root: impl Into<PathBuf>) -> Result<FsTransport> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(io)?;
        let marker = root.join(MARKER);
        if !marker.exists() {
            let empty = fs::read_dir(&root).map_err(io)?.next().is_none();
            if !empty {
                return Err(TransportError::Config(format!(
                    "{} is not empty and not a replica (no {MARKER})",
                    root.display()
                )));
            }
            fs::write(&marker, b"opensidian sync dir replica v1\n").map_err(io)?;
        }
        Ok(FsTransport {
            root,
            max_object: DEFAULT_MAX_OBJECT,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn ns_dir(&self, ns: &Id16) -> PathBuf {
        self.root.join(ns.to_hex())
    }

    fn obj_path(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> PathBuf {
        self.ns_dir(ns).join(kind.path_segment()).join(id.to_hex())
    }

    fn tomb_path(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> PathBuf {
        self.ns_dir(ns).join("del").join(format!("{}-{}", kind.path_segment(), id.to_hex()))
    }

    fn lock(&self, ns: &Id16) -> Result<NsLock> {
        let d = self.ns_dir(ns);
        for sub in ["obj", "cfg", "env", "pair", "del", "tmp"] {
            fs::create_dir_all(d.join(sub)).map_err(io)?;
        }
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(d.join("lock"))
            .map_err(io)?;
        f.lock().map_err(io)?;
        Ok(NsLock(f))
    }

    fn check_kind_ns(kind: ObjKind, ns: &Id16) -> Result<()> {
        let env_ns = *ns == Id16::ZERO;
        match kind {
            ObjKind::Env if !env_ns => Err(TransportError::api(
                ErrorCode::BadRequest,
                "env objects live in the all-zero namespace",
            )),
            ObjKind::Obj | ObjKind::Cfg | ObjKind::Pair if env_ns => {
                Err(TransportError::api(ErrorCode::BadRequest, "the all-zero namespace holds env objects only"))
            }
            _ => Ok(()),
        }
    }

    fn read_log(&self, ns: &Id16) -> Result<Vec<u8>> {
        match fs::read(self.ns_dir(ns).join("log")) {
            Ok(b) => Ok(b),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(io(e)),
        }
    }

    fn list_now(&self, ns: &Id16, q: &ListQuery) -> Result<Vec<ListItem>> {
        let log = self.read_log(ns)?;
        let mut out = Vec::new();
        // A torn trailing record (crash mid-append) is ignored.
        for (i, rec) in log.as_chunks::<RECORD>().0.iter().enumerate() {
            let seq = i as u64 + 1;
            if seq <= q.after {
                continue;
            }
            let Some(kind) = byte_kind(rec[0]) else { continue };
            let mut id = [0u8; 16];
            id.copy_from_slice(&rec[1..17]);
            let id = Id16(id);
            let mut sz = [0u8; 8];
            sz.copy_from_slice(&rec[17..25]);
            if matches!(kind, ObjKind::Obj | ObjKind::Cfg) && !self.tomb_path(kind, ns, &id).exists() {
                out.push(ListItem {
                    seq,
                    kind,
                    id,
                    size: u64::from_be_bytes(sz),
                });
                if out.len() >= q.limit as usize {
                    break;
                }
            }
        }
        Ok(out)
    }
}

fn same_bytes(path: &Path, bytes: &[u8]) -> Result<bool> {
    let mut f = File::open(path).map_err(io)?;
    if f.metadata().map_err(io)?.len() != bytes.len() as u64 {
        return Ok(false);
    }
    let mut have = Vec::with_capacity(bytes.len());
    f.read_to_end(&mut have).map_err(io)?;
    Ok(sync_crypto::ct_eq(&have, bytes))
}

impl Transport for FsTransport {
    fn health(&self) -> Result<Health> {
        if !self.root.join(MARKER).exists() {
            return Err(TransportError::Io(format!("replica marker missing in {}", self.root.display())));
        }
        Ok(Health {
            ok: true,
            proto: PROTO_VERSIONS.to_vec(),
        })
    }

    fn enroll(&self, _token: &str, _pub_destroy: &[u8]) -> Result<EnrollResponse> {
        Err(TransportError::Unsupported("enrollment (a dir replica has no tokens)"))
    }

    fn put(&self, kind: ObjKind, ns: &Id16, id: &Id16, bytes: &[u8]) -> Result<PutOutcome> {
        Self::check_kind_ns(kind, ns)?;
        if bytes.len() as u64 > self.max_object {
            return Err(TransportError::api(ErrorCode::TooLarge, "body over the size limit"));
        }
        let (h, _) = Header::parse(bytes).map_err(|_| TransportError::api(ErrorCode::BadRequest, "bad container header"))?;
        if h.obj_id != *id {
            return Err(TransportError::api(ErrorCode::BadRequest, "id differs from header obj_id"));
        }
        let _g = self.lock(ns)?;
        let dst = self.obj_path(kind, ns, id);
        if self.tomb_path(kind, ns, id).exists() {
            return Err(TransportError::api(ErrorCode::Gone, "object deleted"));
        }
        if dst.exists() {
            return if same_bytes(&dst, bytes)? {
                Ok(PutOutcome::Identical)
            } else {
                Err(TransportError::api(ErrorCode::Conflict, "id exists with different bytes"))
            };
        }
        let tmp = self.ns_dir(ns).join("tmp").join(format!(
            "{}-{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed),
            id.to_hex()
        ));
        let staged = (|| -> std::io::Result<()> {
            let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            fs::hard_link(&tmp, &dst)
        })();
        let _ = fs::remove_file(&tmp);
        match staged {
            Ok(()) => {}
            // Another writer outside our lock (foreign tool) beat us.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                return if same_bytes(&dst, bytes)? {
                    Ok(PutOutcome::Identical)
                } else {
                    Err(TransportError::api(ErrorCode::Conflict, "id exists with different bytes"))
                };
            }
            Err(e) => return Err(io(e)),
        }
        if matches!(kind, ObjKind::Obj | ObjKind::Cfg) {
            let mut rec = [0u8; RECORD];
            rec[0] = kind_byte(kind);
            rec[1..17].copy_from_slice(&id.0);
            rec[17..].copy_from_slice(&(bytes.len() as u64).to_be_bytes());
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .read(true)
                .open(self.ns_dir(ns).join("log"))
                .map_err(io)?;
            // Drop a torn tail first so records stay aligned.
            let len = log.seek(SeekFrom::End(0)).map_err(io)?;
            if len % RECORD as u64 != 0 {
                log.set_len(len - len % RECORD as u64).map_err(io)?;
            }
            log.write_all(&rec).and_then(|_| log.sync_data()).map_err(io)?;
        }
        if let Some(parent) = dst.parent() {
            if let Ok(d) = File::open(parent) {
                let _ = d.sync_all();
            }
        }
        Ok(PutOutcome::Created)
    }

    fn get(&self, kind: ObjKind, ns: &Id16, id: &Id16, range: Option<Range>) -> Result<Fetched> {
        Self::check_kind_ns(kind, ns)?;
        if self.tomb_path(kind, ns, id).exists() {
            return Err(TransportError::api(ErrorCode::Gone, "object deleted"));
        }
        let mut f = match File::open(self.obj_path(kind, ns, id)) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                return Err(TransportError::api(ErrorCode::NotFound, "no such object"))
            }
            Err(e) => return Err(io(e)),
        };
        let size = f.metadata().map_err(io)?.len();
        let (start, len) = match range {
            None => (0, size),
            Some(r) => {
                let (s, e) = r.resolve(size)?;
                (s, e - s + 1)
            }
        };
        f.seek(SeekFrom::Start(start)).map_err(io)?;
        let mut bytes = Vec::with_capacity(len as usize);
        f.take(len).read_to_end(&mut bytes).map_err(io)?;
        Ok(Fetched { bytes, total: size })
    }

    fn get_deleted(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> Result<Vec<u8>> {
        Self::check_kind_ns(kind, ns)?;
        match fs::read(self.obj_path(kind, ns, id)) {
            Ok(b) => Ok(b),
            Err(e) if e.kind() == ErrorKind::NotFound => Err(TransportError::api(ErrorCode::NotFound, "no such object")),
            Err(e) => Err(io(e)),
        }
    }

    fn missing(&self, ns: &Id16, ids: &[Id16]) -> Result<Vec<Id16>> {
        if ids.len() > MAX_MISSING_IDS {
            return Err(TransportError::api(ErrorCode::TooLarge, "too many ids"));
        }
        Ok(ids
            .iter()
            .filter(|id| !self.obj_path(ObjKind::Obj, ns, id).exists())
            .copied()
            .collect())
    }

    fn list(&self, ns: &Id16, q: ListQuery) -> Result<ListResponse> {
        q.validate()
            .map_err(|_| TransportError::api(ErrorCode::BadRequest, "wait <= 60, 1 <= limit <= 1000"))?;
        let deadline = Instant::now() + Duration::from_secs(q.wait as u64);
        loop {
            let items = self.list_now(ns, &q)?;
            if !items.is_empty() || Instant::now() >= deadline {
                let next = items.last().map(|i| i.seq).unwrap_or(q.after);
                return Ok(ListResponse { items, next });
            }
            std::thread::sleep(POLL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }

    fn delete(&self, kind: ObjKind, ns: &Id16, id: &Id16) -> Result<()> {
        Self::check_kind_ns(kind, ns)?;
        if kind == ObjKind::Env {
            return Err(TransportError::api(ErrorCode::Forbidden, "env objects are not deletable"));
        }
        let _g = self.lock(ns)?;
        let obj = self.obj_path(kind, ns, id);
        let tomb = self.tomb_path(kind, ns, id);
        if !obj.exists() || tomb.exists() {
            return Err(TransportError::api(ErrorCode::NotFound, "no live object"));
        }
        if kind == ObjKind::Pair {
            // Pairing objects are single-use: purged at once, no tombstone
            // (the server's GC drops the row the same way).
            return fs::remove_file(&obj).map_err(io);
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tomb)
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        Ok(())
    }

    fn destroy_request(&self, _ns: &Id16) -> Result<DestroyChallenge> {
        Err(TransportError::Unsupported("destroy (a dir replica cannot attest a purge; delete the folder)"))
    }

    fn destroy_confirm(&self, _ns: &Id16, _c: &DestroyConfirm) -> Result<DestroyReceipt> {
        Err(TransportError::Unsupported("destroy (a dir replica cannot attest a purge; delete the folder)"))
    }
}
