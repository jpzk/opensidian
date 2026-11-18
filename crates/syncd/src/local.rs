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

use crate::api::{ApiError, ApiResult, HistoryItem, Receipt, Status, SyncApi, SyncSummary};
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

fn io_err(e: impl std::fmt::Display) -> ApiError {
    ApiError::new("io", e.to_string())
}

pub fn vault_err(e: VaultError) -> ApiError {
    match e {
        VaultError::Revoked => ApiError::new("revoked", e.to_string()),
        VaultError::NeedWords => ApiError::new("need-words", e.to_string()),
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

pub struct LocalEngine {
    p: FipsProvider,
    state: PathBuf,
    cfg: VaultConfig,
    ks: FileKeystore,
}

impl std::fmt::Debug for LocalEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEngine").field("state", &self.state).field("cfg", &self.cfg).finish_non_exhaustive()
    }
}

impl LocalEngine {
    /// Open a configured state dir. `presence` gates reading the backup words.
    pub fn open(state: &Path, presence: Box<dyn Presence>) -> ApiResult<LocalEngine> {
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
        let p = FipsProvider::new().map_err(io_err)?;
        let ks = FileKeystore::open(state.join("keys"), presence).map_err(ks_err)?;
        Ok(LocalEngine { p, state: state.to_path_buf(), cfg, ks })
    }

    /// Create or join a vault; writes the keystore entries + vault.json.
    /// Returns the engine and, for `Create` (and a pairing that carried
    /// them), the words — show once, they are also in the backup entry.
    pub fn setup(s: Setup<'_>) -> ApiResult<(LocalEngine, Option<Zeroizing<String>>)> {
        if config_path(s.state).exists() {
            return Err(ApiError::new("bad-request", format!("{} already holds a vault", s.state.display())));
        }
        let vault_dir = fs::canonicalize(s.vault_dir).map_err(io_err)?;
        mkdir_private(s.state)?;
        let state = fs::canonicalize(s.state).map_err(io_err)?;
        if state.starts_with(&vault_dir) {
            return Err(ApiError::new("bad-request", "the state dir must be outside the vault folder"));
        }
        let p = FipsProvider::new().map_err(io_err)?;
        let t = HttpTransport::new(s.url, roots(&s.ca_file)?).map_err(|e| ApiError::new("server", e.to_string()))?;
        let (m, words) = match s.join {
            Join::Create { enrollment_token } => {
                let w = Phrase::generate(&p).map_err(io_err)?;
                let m = vault::create_vault(&p, &t, s.url, enrollment_token, &w, s.device_name).map_err(vault_err)?;
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
            ca_file: s.ca_file,
        };
        let tmp = state.join("vault.json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&cfg).map_err(io_err)?).map_err(io_err)?;
        fs::rename(&tmp, config_path(&state)).map_err(io_err)?;
        Ok((LocalEngine { p, state, cfg, ks }, text))
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

    fn transport(&self) -> ApiResult<HttpTransport> {
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

    fn save_member(&self, m: &Member) -> ApiResult<()> {
        self.ks.put(&self.cfg.vault_id, Entry::Daily, &m.to_daily().map_err(vault_err)?).map_err(ks_err)
    }

    fn store(&self) -> ApiResult<Store> {
        Store::open(&self.state.join("app"), &self.cfg.vault_id.to_hex(), &self.cfg.vault_dir, history::Config::default())
            .map_err(hist_err)
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
                // no reply yet (404) — the new device has not answered
                Err(VaultError::Transport(e)) => {
                    if Instant::now() >= deadline {
                        return Err(ApiError::new("server", format!("no pairing reply in time ({e})")));
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
                Err(e) => return Err(vault_err(e)),
            }
        }
    }

    /// Revoke another device (needs the words: a new VK epoch is published).
    pub fn revoke(&self, device_hex: &str, words: &str) -> ApiResult<u32> {
        let root = vault::root_of(&self.p, &self.words(words)?).map_err(vault_err)?;
        let id = Id16::from_hex(device_hex).map_err(|e| ApiError::new("bad-request", format!("{e:?}")))?;
        let (t, mut m) = self.member()?;
        let epoch = vault::revoke(&self.p, &t, &mut m, &root, &id).map_err(vault_err)?;
        self.save_member(&m)?;
        Ok(epoch)
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

    /// Local edits: disk vs head tree.
    fn edits(&self, before: &sync_core::commit::Tree, disk: &std::collections::BTreeMap<String, workdir::DiskFile>) -> ApiResult<Vec<Edit>> {
        let mut edits = Vec::new();
        for (path, f) in disk {
            if before.0.get(path).map(|e| e.sha256) == Some(f.sha256) {
                continue;
            }
            let content = fs::read(workdir::abs(&self.cfg.vault_dir, path)).map_err(io_err)?;
            edits.push(Edit::Put { path: path.clone(), class: workdir::class_of(path), mtime_ms: f.mtime_ms, content });
        }
        for path in before.0.keys() {
            if !disk.contains_key(path) {
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
        let (t, m) = self.member()?;
        let mut e = self.engine(&m)?;
        let mut hist = self.store()?;
        let root = self.cfg.vault_dir.clone();
        let mut sum = SyncSummary::default();
        let before = e.head_tree().map_err(engine_err)?;
        let (disk, _) = workdir::scan(&self.p, &root).map_err(io_err)?;
        let edits = self.edits(&before, &disk)?;
        let now = now_ms();
        for ed in &edits {
            if let Edit::Put { path, .. } = ed {
                // the local version enters history before anything can replace it
                let _ = hist.record_external(path, now as u64);
            }
        }
        if !edits.is_empty() {
            let id = e.commit_local(&m.device.signer(), &m.roster(), now, edits).map_err(engine_err)?;
            sum.committed = Some(id.to_hex());
        }
        let mut losers: Vec<(String, Vec<u8>)> = Vec::new();
        let mut hook = |l: &Loser, b: &[u8]| losers.push((l.path.clone(), b.to_vec()));
        let rep = e
            .sync(&[&t as &dyn Transport], &m.device.signer(), &m.roster(), now, &mut hook)
            .map_err(engine_err)?;
        for (path, b) in &losers {
            hist.record_sync_loser(path, b, now as u64).map_err(hist_err)?;
        }
        sum.pulled = rep.pulled;
        sum.pushed = rep.pushed;
        sum.rejected = rep.rejected.len();
        sum.replica_errors = rep.replica_errors.iter().map(|(_, e)| e.clone()).collect();
        sum.losers = rep.losers;
        // materialise the head tree; anything changed on disk meanwhile is left for the next pass
        let after = e.head_tree().map_err(engine_err)?;
        let roster = m.roster();
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
                hist.snapshot_before_overwrite(path, now as u64).map_err(hist_err)?;
            }
            workdir::write_atomic(&self.p, &root, path, &bytes).map_err(io_err)?;
            hist.record_own_save(path, now as u64).map_err(hist_err)?;
            sum.written += 1;
        }
        for (path, f) in &disk {
            if after.0.contains_key(path) {
                continue;
            }
            if workdir::disk_sha(&self.p, &root, path).map_err(io_err)? != Some(f.sha256) {
                sum.skipped += 1;
                continue;
            }
            hist.snapshot_before_delete(path, now as u64).map_err(hist_err)?;
            workdir::remove(&root, path).map_err(io_err)?;
            sum.removed += 1;
        }
        sum.head = e.head().map(|h| h.to_hex());
        Ok(sum)
    }

    fn history(&self, path: &str) -> ApiResult<Vec<HistoryItem>> {
        history::validate_path(path).map_err(hist_err)?;
        Ok(self.store()?.versions(path).iter().map(item).collect())
    }

    fn history_read(&self, seq: u64) -> ApiResult<Vec<u8>> {
        self.store()?.read_version(seq).map_err(hist_err)
    }

    fn restore(&self, seq: u64) -> ApiResult<HistoryItem> {
        let mut s = self.store()?;
        s.restore(seq, now_ms() as u64).map(|e| item(&e)).map_err(hist_err)
    }

    fn destroy(&self, words: &str) -> ApiResult<Receipt> {
        let root = vault::root_of(&self.p, &self.words(words)?).map_err(vault_err)?;
        let (t, m) = self.member()?;
        let r = vault::destroy(&self.p, &t, &m.env, m.server, &root, now_s() as i64).map_err(vault_err)?;
        let vid = self.cfg.vault_id;
        self.ks.delete(&vid, Entry::Daily).map_err(ks_err)?;
        self.ks.delete(&vid, Entry::Backup).map_err(ks_err)?;
        Ok(Receipt { ns: r.ns.to_hex(), destroyed_at: r.destroyed_at, objects: r.objects, bytes: r.bytes })
    }
}
