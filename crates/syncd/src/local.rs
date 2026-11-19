// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! [`LocalEngine`]: the sync-core engine in-process for one vault.
//!
//! State dir layout (0700):
//!
//! ```text
//! <state>/vault.json     VaultConfig (no secrets)
//! <state>/keys/          FileKeystore: <vault>.daily (device key + envelope), <vault>.backup (words)
//! <state>/engine/        engine objects + state.json + lock (the per-vault flock)
//! <state>/app/history/   file history (outside the vault folder)
//! ```
//!
//! Every operation opens the engine for its duration only, so the daemon,
//! a direct CLI call and the app's embedded engine serialise on the same
//! flock (`busy` instead of two writers).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sync_core::commit::Edit;
use sync_core::engine::{Engine, EngineError};
use sync_core::history::{self, HistoryError, Store};
use sync_core::keystore::{Entry, FileKeystore, FixedPresence, Keystore, KeystoreError, Presence};
use sync_core::merge::{ConflictMode, Loser};
use sync_core::sync_crypto::fips::FipsProvider;
use sync_core::sync_crypto::words::Phrase;
use sync_core::sync_proto::Id16;
use sync_core::transport::http::{HttpTransport, Roots};
use sync_core::transport::Transport;
use sync_core::vault::{self, Member, PairCode, VaultError};
use zeroize::Zeroizing;

use crate::api::{ApiError, ApiResult, HistoryItem, Moved, Receipt, Status, SyncApi, SyncSummary};
use crate::workdir;

pub const CONFIG_V: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VaultConfig {
    pub v: u32,
    pub vault_id: Id16,
    pub url: String,
    pub vault_dir: PathBuf,
    pub device_name: String,
    #[serde(default)]
    pub conflict_mode: ConflictMode,
    /// PEM bundle for a self-hosted CA (else the system roots).
    #[serde(default)]
    pub ca_file: Option<PathBuf>,
}

pub fn config_path(state: &Path) -> PathBuf {
    state.join("vault.json")
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn now_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Approved-mode gate (cmvp.md §6): establish the OpenSSL FIPS provider
/// (fips + base loaded, default properties fips=yes) or refuse with code
/// `fips`. Every engine (syncd CLI/daemon and the app's sync window, which
/// embeds this engine) passes here before touching state, keys or the
/// network, so sync never starts outside approved mode.
pub fn fips_gate() -> ApiResult<FipsProvider> {
    sync_core::sync_crypto::fips::init().map_err(|e| ApiError::new("fips", format!("refusing to start sync: {e}")))?;
    FipsProvider::new().map_err(|e| ApiError::new("fips", format!("refusing to start sync: {e:?}")))
}

fn io_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::new("io", e.to_string())
}

pub fn vault_err(e: VaultError) -> ApiError {
    match e {
        VaultError::NotMember => ApiError::new("not-member", e.to_string()),
        VaultError::Transport(_) => ApiError::new("server", e.to_string()),
        VaultError::NoVault => ApiError::new("no-vault", e.to_string()),
        _ => ApiError::new("vault", e.to_string()),
    }
}

fn engine_err(e: EngineError) -> ApiError {
    match e {
        EngineError::Busy => ApiError::new("busy", e.to_string()),
        EngineError::Transport(_) => ApiError::new("server", e.to_string()),
        _ => ApiError::new("engine", e.to_string()),
    }
}

fn hist_err(e: HistoryError) -> ApiError {
    ApiError::new("history", e.to_string())
}

fn ks_err(e: KeystoreError) -> ApiError {
    match e {
        KeystoreError::Denied => ApiError::new("denied", e.to_string()),
        _ => ApiError::new("io", e.to_string()),
    }
}

fn item(e: &history::Entry) -> HistoryItem {
    HistoryItem {
        seq: e.seq,
        ts_ms: e.ts_ms,
        reason: e.reason.as_str().to_string(),
        size: e.size,
        path: e.path.clone(),
        sha: e.sha.clone(),
    }
}

/// Pairing poll: worth another try (reply not posted yet, or the network).
fn retryable(e: &sync_core::transport::TransportError) -> bool {
    use sync_core::sync_proto::api::ErrorCode;
    use sync_core::transport::TransportError as T;
    match e {
        T::Api { code, .. } => *code == ErrorCode::NotFound,
        T::Io(_) => true,
        _ => false,
    }
}

/// Write a secret file readable by the owner only (create_new + fsync).
fn write_private(path: &Path, bytes: &[u8]) -> ApiResult<()> {
    use std::io::Write as _;
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        o.mode(0o600);
    }
    let mut f = o.open(path).map_err(io_err)?;
    f.write_all(bytes).and_then(|_| f.sync_all()).map_err(io_err)
}

/// What the vault folder held, per path, when this device last synced it:
/// the bytes it committed from disk or wrote to disk. A path whose disk
/// bytes still match is NOT a local edit even if the head moved on — it is
/// a pending update the materialise step has not applied yet (skipped,
/// or the pass died part-way). Without it, the next pass would commit the
/// stale disk bytes back over the remote change.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DiskIndex {
    v: u32,
    files: std::collections::BTreeMap<String, String>,
}

fn hex32(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl DiskIndex {
    fn path(state: &Path) -> PathBuf {
        state.join("disk-index.json")
    }

    /// `None` = no index yet (older state dir): the head tree stands in.
    fn load(state: &Path) -> ApiResult<Option<DiskIndex>> {
        match fs::read(Self::path(state)) {
            Ok(b) => {
                let i: DiskIndex = serde_json::from_slice(&b).map_err(|e| ApiError::new("io", format!("disk index: {e}")))?;
                Ok((i.v == 1).then_some(i))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(e)),
        }
    }

    fn save(&self, state: &Path) -> ApiResult<()> {
        let tmp = state.join("disk-index.json.tmp");
        let bytes = serde_json::to_vec(self).map_err(io_err)?;
        (|| -> std::io::Result<()> {
            use std::io::Write as _;
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            fs::rename(&tmp, Self::path(state))
        })()
        .map_err(io_err)
    }

    fn holds(&self, path: &str, sha: &[u8; 32]) -> bool {
        self.files.get(path).is_some_and(|h| *h == hex32(sha))
    }
}

/// Max changed inline bytes per local commit: a vault with a large edit
/// set commits in several batches instead of one batch object over the
/// object limit (which would fail every pass).
const COMMIT_BATCH_BYTES: usize = 4 << 20;

/// Split `edits` into commit-sized groups (a single edit over the limit
/// goes alone; the engine chunks large files itself).
fn batches(edits: Vec<Edit>) -> Vec<Vec<Edit>> {
    let mut out: Vec<Vec<Edit>> = Vec::new();
    let mut cur: Vec<Edit> = Vec::new();
    let mut bytes = 0usize;
    for ed in edits {
        let n = match &ed {
            Edit::Put { content, .. } => content.len(),
            _ => 0,
        };
        if !cur.is_empty() && bytes + n > COMMIT_BATCH_BYTES {
            out.push(std::mem::take(&mut cur));
            bytes = 0;
        }
        bytes += n;
        cur.push(ed);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn mkdir_private(d: &Path) -> ApiResult<()> {
    fs::create_dir_all(d).map_err(io_err)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(d, fs::Permissions::from_mode(0o700)).map_err(io_err)?;
    }
    Ok(())
}

fn roots(ca: &Option<PathBuf>) -> ApiResult<Roots> {
    match ca {
        Some(f) => Ok(Roots::Pem(fs::read(f).map_err(io_err)?)),
        None => Ok(Roots::System),
    }
}

/// How a new device joins / a vault is created (`opensidian-syncd setup`).
pub enum Join<'a> {
    /// New vault on the server with the admin's one-time enrollment token.
    Create { enrollment_token: &'a str },
    /// Existing vault from the backup words + URL only.
    Words { words: &'a str },
    /// Existing vault from a pairing code shown by a member device; waits
    /// up to `wait` for the inviter to approve.
    Pair { code: &'a str, wait: Duration },
}

pub struct Setup<'a> {
    pub state: &'a Path,
    pub url: &'a str,
    pub vault_dir: &'a Path,
    pub device_name: &'a str,
    pub ca_file: Option<PathBuf>,
    pub join: Join<'a>,
}

/// Burn-and-move (`opensidian-syncd move`, the app's "Move to a new vault";
/// spec §4.8). A compromised vault is not repaired (no revocation): this
/// device creates a NEW vault (new words, new server namespace, new keys)
/// seeded from its vault folder as it is now + its file history, then
/// destroys the old vault with the old words' K_destroy proof.
pub struct Move<'a> {
    /// the CURRENT vault's backup words — the destroy proof; checked against
    /// this vault before anything is created
    pub old_words: &'a str,
    /// one-time enrollment token for the new vault (`opensidian-sync-server vault create`)
    pub enrollment_token: &'a str,
    /// server of the new vault; None = the current one
    pub url: Option<&'a str>,
    /// PEM bundle for that server; None = the current setting
    pub ca_file: Option<PathBuf>,
}

/// `<state>` + suffix as a sibling path (`/a/state` -> `/a/state.move`).
fn sibling(state: &Path, suffix: &str) -> ApiResult<PathBuf> {
    let name = state.file_name().ok_or_else(|| ApiError::new("io", format!("state dir {} has no name", state.display())))?;
    let mut n = name.to_os_string();
    n.push(suffix);
    Ok(state.with_file_name(n))
}

/// Asked before every server connection (the desktop app passes its offline
/// policy, net_allowed); Err(reason) = do not connect.
pub type NetGate = fn(&str) -> Result<(), String>;

fn net_open(_: &str) -> Result<(), String> {
    Ok(())
}

pub struct LocalEngine {
    p: FipsProvider,
    state: PathBuf,
    cfg: VaultConfig,
    ks: FileKeystore,
    net: NetGate,
}

impl std::fmt::Debug for LocalEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEngine").field("state", &self.state).field("cfg", &self.cfg).finish_non_exhaustive()
    }
}

impl LocalEngine {
    /// Open a configured state dir. `presence` gates reading the backup words.
    pub fn open(state: &Path, presence: Box<dyn Presence>) -> ApiResult<LocalEngine> {
        let p = fips_gate()?;
        let b = match fs::read(config_path(state)) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ApiError::new("not-configured", format!("no vault configured in {}", state.display())))
            }
            Err(e) => return Err(io_err(e)),
        };
        let cfg: VaultConfig = serde_json::from_slice(&b).map_err(io_err)?;
        if cfg.v != CONFIG_V {
            return Err(ApiError::new("io", format!("unknown config version {}", cfg.v)));
        }
        let ks = FileKeystore::open(state.join("keys"), presence).map_err(ks_err)?;
        Ok(LocalEngine { p, state: state.to_path_buf(), cfg, ks, net: net_open })
    }

    /// Create or join a vault; writes the keystore entries + vault.json.
    /// Returns the engine and, for `Create` (and a pairing that carried
    /// them), the words — show once, they are also in the backup entry.
    pub fn setup(s: Setup<'_>) -> ApiResult<(LocalEngine, Option<Zeroizing<String>>)> {
        let p = fips_gate()?;
        if config_path(s.state).exists() {
            return Err(ApiError::new("bad-request", format!("{} already holds a vault", s.state.display())));
        }
        let vault_dir = fs::canonicalize(s.vault_dir).map_err(io_err)?;
        mkdir_private(s.state)?;
        let state = fs::canonicalize(s.state).map_err(io_err)?;
        if state.starts_with(&vault_dir) || vault_dir.starts_with(&state) {
            return Err(ApiError::new("bad-request", "the state dir and the vault folder must not contain each other"));
        }
        // persisted in config.json: a relative path would resolve against
        // whatever directory a later run starts in
        let ca_file = s.ca_file.as_deref().map(fs::canonicalize).transpose().map_err(io_err)?;
        let t = HttpTransport::new(s.url, roots(&ca_file)?).map_err(|e| ApiError::new("server", e.to_string()))?;
        let (m, words) = match s.join {
            Join::Create { enrollment_token } => {
                // The words are generated and kept in <state>/setup-words
                // (0600, private state dir) BEFORE the server consumes the
                // one-time token. If anything after create_vault fails, the
                // next setup in the same state dir resumes: the vault on the
                // server is joined from those words (bootstrap) instead of
                // being stranded with a spent token and lost words. Removed
                // once vault.json is written.
                let pending = state.join("setup-words");
                let w = match fs::read(&pending) {
                    Ok(b) => {
                        let b = Zeroizing::new(b);
                        let text = std::str::from_utf8(&b).map_err(|_| ApiError::new("io", "setup-words is not text"))?;
                        Phrase::parse(&p, text).map_err(|e| ApiError::new("io", format!("setup-words: {e}")))?
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        let w = Phrase::generate(&p).map_err(io_err)?;
                        write_private(&pending, w.to_text(&p).as_bytes())?;
                        w
                    }
                    Err(e) => return Err(io_err(e)),
                };
                let m = match vault::bootstrap(&p, &t, s.url, &w, s.device_name) {
                    Ok(m) => m,
                    Err(VaultError::NoVault) => {
                        vault::create_vault(&p, &t, s.url, enrollment_token, &w, s.device_name).map_err(vault_err)?
                    }
                    Err(e) => return Err(vault_err(e)),
                };
                (m, Some(w))
            }
            Join::Words { words } => {
                let w = Phrase::parse(&p, words).map_err(|e| ApiError::new("bad-request", e.to_string()))?;
                let m = vault::bootstrap(&p, &t, s.url, &w, s.device_name).map_err(vault_err)?;
                (m, Some(w))
            }
            Join::Pair { code, wait } => {
                let code = PairCode::parse(code).map_err(vault_err)?;
                let j = vault::pair_accept(&p, &t, &code, s.device_name, now_s()).map_err(vault_err)?;
                let deadline = Instant::now() + wait;
                let (dev, env, srv) = (j.device, j.env, j.server);
                let mut joining = Some((dev, env, srv, j.words));
                loop {
                    let (device, env, server, words) = joining.take().expect("joining");
                    let ns = env.servers[server].ns;
                    let chain = vault::fetch_chain(&p, &t, &env, &ns, None).map_err(vault_err)?;
                    if chain.active_slot(&device.id).is_some() {
                        break (Member { env, device, chain, server }, words);
                    }
                    if Instant::now() >= deadline {
                        return Err(ApiError::new("vault", "pairing not approved in time"));
                    }
                    joining = Some((device, env, server, words));
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        };
        let ks = FileKeystore::open(state.join("keys"), Box::new(FixedPresence(false))).map_err(ks_err)?;
        let vid = m.env.vault_id;
        ks.put(&vid, Entry::Daily, &m.to_daily().map_err(vault_err)?).map_err(ks_err)?;
        let text = match &words {
            Some(w) => {
                ks.put(&vid, Entry::Backup, &w.s_bytes()[..]).map_err(ks_err)?;
                Some(w.to_text(&p))
            }
            None => None,
        };
        let cfg = VaultConfig {
            v: CONFIG_V,
            vault_id: vid,
            url: s.url.to_string(),
            vault_dir,
            device_name: s.device_name.to_string(),
            conflict_mode: ConflictMode::default(),
            ca_file,
        };
        let tmp = state.join("vault.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&cfg).map_err(io_err)?).map_err(io_err)?;
        fs::rename(&tmp, config_path(&state)).map_err(io_err)?;
        // setup finished: the words now live in the keystore Backup entry
        let _ = fs::remove_file(state.join("setup-words"));
        Ok((LocalEngine { p, state, cfg, ks, net: net_open }, text))
    }

    pub fn config(&self) -> &VaultConfig {
        &self.cfg
    }

    pub fn state_dir(&self) -> &Path {
        &self.state
    }

    pub fn set_conflict_mode(&mut self, mode: ConflictMode) -> ApiResult<()> {
        self.cfg.conflict_mode = mode;
        let tmp = self.state.join("vault.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.cfg).map_err(io_err)?).map_err(io_err)?;
        fs::rename(&tmp, config_path(&self.state)).map_err(io_err)
    }

    /// Every server connection of this engine asks `gate` first.
    pub fn with_net_gate(mut self, gate: NetGate) -> Self {
        self.net = gate;
        self
    }

    fn transport(&self) -> ApiResult<HttpTransport> {
        (self.net)("sync server").map_err(|e| ApiError::new("offline", e))?;
        HttpTransport::new(&self.cfg.url, roots(&self.cfg.ca_file)?).map_err(|e| ApiError::new("server", e.to_string()))
    }

    fn daily(&self) -> ApiResult<Zeroizing<Vec<u8>>> {
        self.ks
            .get(&self.cfg.vault_id, Entry::Daily)
            .map_err(ks_err)?
            .ok_or_else(|| ApiError::new("not-configured", "no daily keystore entry for this vault"))
    }

    /// The member (chain re-verified against the server) + its transport.
    pub fn member(&self) -> ApiResult<(HttpTransport, Member)> {
        let daily = self.daily()?;
        let t = self.transport()?;
        t.set_tokens(Member::daily_tokens(&daily).map_err(vault_err)?);
        let m = Member::from_daily(&self.p, &t, &daily).map_err(vault_err)?;
        Ok((t, m))
    }

    fn store(&self) -> ApiResult<Store> {
        Store::open(&self.state.join("app"), &self.cfg.vault_id.to_hex(), &self.cfg.vault_dir, history::Config::default())
            .map_err(hist_err)
    }

    /// Open the store under `<state>/history.lock`. Every daemon/CLI user
    /// of this journal takes it (sync pass, restore, and readers too: open
    /// cuts a torn tail, which must never hit a live writer's append), so
    /// two processes cannot interleave appends or a rewrite (history is
    /// single-writer). A held lock is `busy`, like the engine lock.
    fn store_mut(&self) -> ApiResult<(fs::File, Store)> {
        fs::create_dir_all(&self.state).map_err(io_err)?;
        let lock = fs::File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.state.join("history.lock"))
            .map_err(io_err)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                return Err(ApiError::new("busy", "history is being written by another sync or restore"))
            }
            Err(fs::TryLockError::Error(e)) => return Err(io_err(e)),
        }
        Ok((lock, self.store()?))
    }

    fn engine<'a>(&'a self, m: &Member) -> ApiResult<Engine<'a, FipsProvider>> {
        let mut e = Engine::open(&self.p, m.ctx(&self.p).map_err(vault_err)?, self.state.join("engine"), &m.roster())
            .map_err(engine_err)?;
        e.mode = self.cfg.conflict_mode;
        e.names.insert(m.device.id, m.device.name.clone());
        Ok(e)
    }

    fn words(&self, words: &str) -> ApiResult<Phrase> {
        Phrase::parse(&self.p, words).map_err(|e| ApiError::new("bad-request", e.to_string()))
    }

    /// Inviter side of pairing: show `on_code(code)`, then wait up to
    /// `wait` for the new device's reply and give it a slot.
    pub fn pair(&self, with_words: Option<&str>, wait: Duration, on_code: &mut dyn FnMut(&str)) -> ApiResult<String> {
        let (t, mut m) = self.member()?;
        let w = with_words.map(|w| self.words(w)).transpose()?;
        let code = vault::pair_offer(&self.p, &t, &m, w.as_ref(), now_s()).map_err(vault_err)?;
        on_code(&code.to_text().map_err(vault_err)?);
        let deadline = Instant::now() + wait;
        loop {
            match vault::pair_complete(&self.p, &t, &mut m, &code) {
                Ok((id, name)) => return Ok(format!("{} {name}", id.to_hex())),
                // Retry only while the reply can still be there: not posted
                // yet (404) or a transient network error (pair_complete
                // purges the reply only after the new slot is on the server,
                // and treats an already-landed slot as done). Anything else
                // (410, 403, bad bytes, chain errors) ends the pairing.
                Err(VaultError::Transport(e)) if retryable(&e) => {
                    if Instant::now() >= deadline {
                        return Err(ApiError::new("server", format!("no pairing reply in time ({e})")));
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                Err(e) => return Err(vault_err(e)),
            }
        }
    }

    /// Backup words (behind the presence check).
    pub fn reveal_words(&self) -> ApiResult<Zeroizing<String>> {
        let b = self
            .ks
            .get(&self.cfg.vault_id, Entry::Backup)
            .map_err(ks_err)?
            .ok_or_else(|| ApiError::new("not-configured", "no backup entry on this device"))?;
        let arr: [u8; sync_core::sync_crypto::words::S_BYTES_LEN] =
            b[..].try_into().map_err(|_| ApiError::new("io", "backup entry length"))?;
        let w = Phrase::from_s_bytes(&arr).map_err(io_err)?;
        Ok(w.to_text(&self.p))
    }

    /// Local edits: disk vs head tree. A path is edited only if its disk
    /// bytes differ from the head AND from what this device last synced
    /// there (`idx`); a head path missing on disk is deleted only if this
    /// device had it on disk. Everything else is a pending update the
    /// materialise step still owes the folder, not a local change.
    fn edits(
        &self,
        before: &sync_core::commit::Tree,
        disk: &std::collections::BTreeMap<String, workdir::DiskFile>,
        idx: Option<&DiskIndex>,
    ) -> ApiResult<Vec<Edit>> {
        let mut edits = Vec::new();
        for (path, f) in disk {
            if before.0.get(path).map(|e| e.sha256) == Some(f.sha256) {
                continue;
            }
            if idx.is_some_and(|i| i.holds(path, &f.sha256)) {
                continue;
            }
            // the bytes committed are the bytes the scan hashed (anchored read)
            let content = workdir::read_expect(&self.p, &self.cfg.vault_dir, path, &f.sha256).map_err(io_err)?;
            edits.push(Edit::Put { path: path.clone(), class: workdir::class_of(path), mtime_ms: f.mtime_ms, content });
        }
        for path in before.0.keys() {
            if !disk.contains_key(path) && idx.is_none_or(|i| i.files.contains_key(path)) {
                edits.push(Edit::Delete { path: path.clone() });
            }
        }
        Ok(edits)
    }
}

impl SyncApi for LocalEngine {
    fn status(&self) -> ApiResult<Status> {
        let mut st = Status {
            vault_id: self.cfg.vault_id.to_hex(),
            vault_dir: self.cfg.vault_dir.display().to_string(),
            url: self.cfg.url.clone(),
            device_id: String::new(),
            device_name: self.cfg.device_name.clone(),
            conflict_mode: match self.cfg.conflict_mode {
                ConflictMode::AutoMerge => "auto-merge".into(),
                ConflictMode::ConflictFile => "conflict-file".into(),
            },
            server_ok: false,
            server_error: None,
            cfg_gen: None,
            vk_epoch: None,
            active_devices: None,
            head: None,
            commits: 0,
            tips: 0,
            files: 0,
        };
        let m = match self.member() {
            Ok((_, m)) => m,
            Err(e) => {
                st.server_error = Some(e.to_string());
                return Ok(st);
            }
        };
        st.server_ok = true;
        st.device_id = m.device.id.to_hex();
        let h = m.chain.head();
        st.cfg_gen = Some(h.gen);
        st.vk_epoch = Some(h.vk_epoch);
        st.active_devices =
            Some(h.slots.iter().filter(|s| s.state == sync_core::sync_proto::vault::SlotState::Active).count());
        let mut e = self.engine(&m)?;
        st.head = e.head().map(|h| h.to_hex());
        st.commits = e.commit_count();
        st.tips = e.tips().len();
        st.files = e.head_tree().map_err(engine_err)?.0.len();
        Ok(st)
    }

    fn sync_now(&self) -> ApiResult<SyncSummary> {
        let (_lock, mut s) = self.store_mut()?;
        self.sync_with(&mut s)
    }

    fn history(&self, path: &str) -> ApiResult<Vec<HistoryItem>> {
        history::validate_path(path).map_err(hist_err)?;
        Ok(self.store_mut()?.1.versions(path).iter().map(item).collect())
    }

    fn history_read(&self, seq: u64) -> ApiResult<Vec<u8>> {
        self.store_mut()?.1.read_version(seq).map_err(hist_err)
    }

    fn restore(&self, seq: u64) -> ApiResult<HistoryItem> {
        let (_lock, mut s) = self.store_mut()?;
        s.restore(seq, now_ms() as u64).map(|e| item(&e)).map_err(hist_err)
    }

    fn destroy(&self, words: &str) -> ApiResult<Receipt> {
        let root = vault::root_of(&self.p, &self.words(words)?).map_err(vault_err)?;
        let (t, m) = self.member()?;
        let r = vault::destroy(&self.p, &t, &m.env, m.server, &root, now_s() as i64).map_err(vault_err)?;
        // The purge is done and the receipt verified: that proof goes back to
        // the caller even if the local key cleanup fails (a retry could never
        // fetch it again from a vault that no longer exists).
        let vid = self.cfg.vault_id;
        let cleanup = [Entry::Daily, Entry::Backup]
            .into_iter()
            .filter_map(|en| self.ks.delete(&vid, en).err().map(|e| ks_err(e).to_string()))
            .collect::<Vec<_>>();
        Ok(Receipt {
            ns: r.ns.to_hex(),
            destroyed_at: r.destroyed_at,
            objects: r.objects,
            bytes: r.bytes,
            cleanup_error: (!cleanup.is_empty()).then(|| cleanup.join("; ")),
        })
    }
}

/// Where a sync pass records file history: the CLI / daemon open the store
/// under the state dir; the app hands in its process-wide store (ONE opener
/// per journal — the app's filehist already holds it, src-tauri/src/syncui.rs).
pub trait HistSink {
    /// Run `f` on the store. A sink without a store (history off) returns Ok.
    fn with(&mut self, f: &mut dyn FnMut(&mut Store) -> Result<(), HistoryError>) -> ApiResult<()>;
}

impl HistSink for Store {
    fn with(&mut self, f: &mut dyn FnMut(&mut Store) -> Result<(), HistoryError>) -> ApiResult<()> {
        f(self).map_err(hist_err)
    }
}

impl LocalEngine {
    /// One sync pass (`sync_now`) with file history recorded through `hist`.
    pub fn sync_with(&self, hist: &mut dyn HistSink) -> ApiResult<SyncSummary> {
        let (t, m) = self.member()?;
        let mut e = self.engine(&m)?;
        let root = self.cfg.vault_dir.clone();
        let mut sum = SyncSummary::default();
        let before = e.head_tree().map_err(engine_err)?;
        let (disk, _) = workdir::scan(&self.p, &root).map_err(io_err)?;
        let idx = DiskIndex::load(&self.state)?;
        let edits = self.edits(&before, &disk, idx.as_ref())?;
        let now = now_ms();
        for ed in &edits {
            if let Edit::Put { path, .. } = ed {
                // the local version enters history before anything can
                // replace it; a failed write stops the pass before commit
                hist.with(&mut |s| s.record_external(path, now as u64).map(|_| ()))?;
            }
        }
        for batch in batches(edits) {
            let id = e.commit_local(&m.device.signer(), &m.roster(), now, batch).map_err(engine_err)?;
            sum.committed = Some(id.to_hex());
        }
        // Every path on disk now holds synced bytes: committed just now,
        // equal to the head, or a pending update still at its old bytes.
        let mut idx = DiskIndex { v: 1, files: disk.iter().map(|(p, f)| (p.clone(), hex32(&f.sha256))).collect() };
        idx.save(&self.state)?;
        // Losers go to history inside the pass: a failed write aborts it
        // before the merged head is installed, so the next pass merges
        // again and offers the loser again (nothing is dropped silently).
        let mut hook = |l: &Loser, b: &[u8]| -> Result<(), String> {
            hist.with(&mut |s| s.record_sync_loser(&l.path, b, now as u64).map(|_| ())).map_err(|e| e.to_string())
        };
        let rep = e
            .sync_checked(&[&t as &dyn Transport], &m.device.signer(), &m.roster(), now, &mut hook)
            .map_err(engine_err)?;
        sum.pulled = rep.pulled;
        sum.pushed = rep.pushed;
        sum.rejected = rep.rejected.len() + rep.invalid.len();
        sum.rejected_detail = rep.rejected.iter().map(|(i, id, why)| format!("replica {i}: {} {why}", id.to_hex())).collect();
        sum.rejected_detail.extend(rep.invalid.iter().map(|(id, why)| format!("tip {}: {why}", id.to_hex())));
        sum.replica_errors = rep.replica_errors.iter().map(|(_, e)| e.clone()).collect();
        sum.losers = rep.losers;
        // Materialise the head tree; anything changed on disk meanwhile is
        // left for the next pass. Deletions run first, so a file that turns
        // into a folder (or a folder emptied by them that turns into a file)
        // is out of the way before the write that needs its name. Each change
        // re-checks the disk right before it touches it, after the history
        // snapshot: a concurrent edit is either skipped or already captured.
        let after = e.head_tree().map_err(engine_err)?;
        let roster = m.roster();
        for (path, f) in &disk {
            if after.0.contains_key(path) {
                continue;
            }
            if workdir::disk_sha(&self.p, &root, path).map_err(io_err)? != Some(f.sha256) {
                sum.skipped += 1;
                continue;
            }
            hist.with(&mut |s| s.snapshot_before_delete(path, now as u64).map(|_| ()))?;
            if workdir::disk_sha(&self.p, &root, path).map_err(io_err)? != Some(f.sha256) {
                sum.skipped += 1;
                continue;
            }
            workdir::remove(&root, path).map_err(io_err)?;
            idx.files.remove(path);
            sum.removed += 1;
        }
        for (path, te) in &after.0 {
            let scanned = disk.get(path).map(|f| f.sha256);
            if scanned == Some(te.sha256) {
                continue;
            }
            if workdir::disk_sha(&self.p, &root, path).map_err(io_err)? != scanned {
                sum.skipped += 1;
                continue;
            }
            let bytes = e.read(te, &roster).map_err(engine_err)?;
            if scanned.is_some() {
                hist.with(&mut |s| s.snapshot_before_overwrite(path, now as u64).map(|_| ()))?;
                if workdir::disk_sha(&self.p, &root, path).map_err(io_err)? != scanned {
                    sum.skipped += 1;
                    continue;
                }
            } else if sync_core::vaultfs::remove_empty_dir(&root, path).is_err() {
                // a folder with files still sits where the file goes
                sum.skipped += 1;
                continue;
            }
            match workdir::write_atomic(&self.p, &root, path, &bytes) {
                Ok(()) => {}
                // vaultfs refused the path: a parent is a file whose delete was
                // skipped above (user changed it) or a symlink -> next pass
                Err(err) if err.kind() == std::io::ErrorKind::InvalidInput => {
                    sum.skipped += 1;
                    continue;
                }
                Err(err) => return Err(io_err(err)),
            }
            hist.with(&mut |s| s.record_own_save(path, now as u64).map(|_| ()))?;
            sum.written += 1;
            idx.files.insert(path.clone(), hex32(&te.sha256));
        }
        idx.save(&self.state)?;
        sum.head = e.head().map(|h| h.to_hex());
        Ok(sum)
    }

    /// Burn-and-move, see [`Move`]. Every step is resumable — a rerun after
    /// a failure continues where it stopped, and the old vault is destroyed
    /// only after the new one holds the folder:
    ///  1. the old words must open THIS vault's envelope (else nothing changes);
    ///  2. new vault in the staging dir `<state>.move` (setup `Create` with the
    ///     enrollment token; a staging dir of an interrupted move is reused);
    ///  3. file history copied into the new vault's store
    ///     ([`history::copy_store`]; `hist` = None) — the app passes its own
    ///     per-folder store instead, which the move does not touch;
    ///  4. seed: one sync pass of the new vault commits every file of the
    ///     folder and pushes it. NOTHING is pulled from the old vault first —
    ///     it is compromised; edits other devices never synced to this one
    ///     are not carried over;
    ///  5. 2-step destroy of the old vault with the old words' K_destroy
    ///     proof (receipt checked against the pinned server key), old
    ///     keystore entries deleted;
    ///  6. `<state>` -> `<state>.burned-<old vault id>`, staging -> `<state>`.
    ///
    /// Returns the new vault's engine, the report and the NEW words (show
    /// them once; they are also in the new backup entry).
    pub fn move_vault(self, mv: Move<'_>, hist: Option<&mut dyn HistSink>) -> ApiResult<(LocalEngine, Moved, Zeroizing<String>)> {
        let old_vid = self.cfg.vault_id;
        let old_root = vault::root_of(&self.p, &self.words(mv.old_words)?).map_err(vault_err)?;
        {
            let (t, _m) = self.member()?;
            match vault::fetch_envelope(&self.p, &t, &old_root).map_err(vault_err)? {
                Some(env) if env.vault_id == old_vid => {}
                _ => {
                    return Err(ApiError::new(
                        "bad-request",
                        "these are not the backup words of this vault (nothing was changed)",
                    ))
                }
            }
        }
        let staging = sibling(&self.state, ".move")?;
        let burned = sibling(&self.state, &format!(".burned-{}", old_vid.to_hex()))?;
        if fs::symlink_metadata(&burned).is_ok() {
            return Err(ApiError::new("bad-request", format!("{} already exists", burned.display())));
        }
        let (mut new, words) = if config_path(&staging).exists() {
            // interrupted move: the new vault exists already
            let e = LocalEngine::open(&staging, Box::new(FixedPresence(true)))?.with_net_gate(self.net);
            if e.cfg.vault_dir != self.cfg.vault_dir {
                return Err(ApiError::new(
                    "bad-request",
                    format!("{} holds a move of another folder ({})", staging.display(), e.cfg.vault_dir.display()),
                ));
            }
            let w = e.reveal_words()?;
            (e, w)
        } else {
            (self.net)("sync server").map_err(|e| ApiError::new("offline", e))?;
            let (e, w) = LocalEngine::setup(Setup {
                state: &staging,
                url: mv.url.unwrap_or(&self.cfg.url),
                vault_dir: &self.cfg.vault_dir,
                device_name: &self.cfg.device_name,
                ca_file: mv.ca_file.clone().or_else(|| self.cfg.ca_file.clone()),
                join: Join::Create { enrollment_token: mv.enrollment_token },
            })?;
            let w = w.ok_or_else(|| ApiError::new("vault", "the new vault came without words"))?;
            (e.with_net_gate(self.net), w)
        };
        if new.cfg.vault_id == old_vid {
            return Err(ApiError::new("vault", "the new vault has the old vault id"));
        }
        new.set_conflict_mode(self.cfg.conflict_mode)?;
        let new_hex = new.cfg.vault_id.to_hex();
        let new_app = staging.join("app");
        let hist_root = history::store_root(&new_app, &new_hex).map_err(hist_err)?;
        let history_entries = if fs::symlink_metadata(&hist_root).is_ok() {
            fs::read(hist_root.join("index.log")).map(|b| b.iter().filter(|&&c| c == b'\n').count()).unwrap_or(0)
        } else {
            history::copy_store(&self.state.join("app"), &old_vid.to_hex(), &new_app, &new_hex).map_err(hist_err)?
        };
        let seeded = match hist {
            Some(h) => new.sync_with(h)?,
            None => {
                let (_lock, mut s) = new.store_mut()?;
                new.sync_with(&mut s)?
            }
        };
        if !seeded.replica_errors.is_empty() || seeded.rejected > 0 {
            return Err(ApiError::new(
                "server",
                format!(
                    "seeding the new vault did not complete ({}); the old vault is untouched, run move again",
                    seeded.replica_errors.join("; ")
                ),
            ));
        }
        let files = new.status()?.files;
        let destroyed = self.destroy(mv.old_words).map_err(|e| {
            ApiError::new(
                &e.code,
                format!("{e} — the new vault is ready in {}; run move again to finish", staging.display()),
            )
        })?;
        let (state, net) = (self.state.clone(), self.net);
        drop(new);
        drop(self);
        fs::rename(&state, &burned).map_err(io_err)?;
        fs::rename(&staging, &state).map_err(io_err)?;
        let e = LocalEngine::open(&state, Box::new(FixedPresence(false)))?.with_net_gate(net);
        let moved = Moved {
            old_vault_id: old_vid.to_hex(),
            new_vault_id: new_hex,
            files,
            history_entries,
            destroyed,
            burned_state: burned.display().to_string(),
        };
        // the proof stays with the new vault's state (the receipt cannot be fetched again)
        write_private(
            &state.join(format!("burned-{}.receipt.json", moved.old_vault_id)),
            &serde_json::to_vec_pretty(&moved.destroyed).map_err(io_err)?,
        )?;
        Ok((e, moved, words))
    }
}
