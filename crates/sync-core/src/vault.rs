// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! M5 onboarding (docs/sync/spec.md §4.4–4.6, key-tree.md): create a vault,
//! join from the backup words + URL, pair a device over a one-time
//! symmetric channel.
//!
//! Trust model in one paragraph: the backup words derive K_wrap / loc /
//! K_rsig / K_destroy. The envelope (keyring + servers + tokens) lives on
//! the server under `loc`, sealed under K_wrap, so "words + URL → everything".
//! Membership is the signed cfg chain: gen 0 is signed by K_rsig, every
//! later gen by an active slot of the previous gen or by K_rsig; a slot,
//! once added, never leaves. Commit signatures are checked against the
//! roster of the chain ([`Roster`]); a device not on it is unknown.
//!
//! No revocation (operator decision 2026-11-19): a vault has exactly ONE
//! VK epoch (0) for its whole life — the wire format keeps the `vk_epoch`
//! / keyring fields, but [`verify_gen`] and [`vault_ctx`] refuse anything
//! but epoch 0. If the words or a device are compromised the vault is
//! BURNED: the user moves to a new vault (new words, new namespace) seeded
//! from local data + file history, then destroys the old one with the
//! K_destroy proof (spec §4.9, syncd `move`).

use std::collections::BTreeMap;
use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use sync_crypto::fips::{FipsProvider, FipsSigningKey};
use sync_crypto::keytree::{derive_root, RootKeys};
use sync_crypto::words::{Phrase, S_BYTES_LEN};
use sync_crypto::{CryptoProvider, SigAlg};
use sync_proto::api::{ErrorCode, ObjKind, ReceiptBody};
use sync_proto::container::{Kind, Suite};
use sync_proto::enc::{Reader, Writer};
use sync_proto::vault::{
    CfgGen, CfgSigner, Envelope, PairMsg, PairVault, ServerEntry, Slot, SlotState, VkEntry, SLOTS,
};
use sync_proto::Id16;
use zeroize::Zeroizing;

use crate::commit::{DeviceKeys, Signer, VaultCtx};
use crate::object::{open, seal, Compress, ObjectCtx, ObjectError};
use crate::transport::http::HttpTransport;
use crate::transport::{destroy_vault, Tokens, Transport, TransportError};

/// v1 vaults: suite 0x01, Ed25519 (OpenSSL FIPS provider 3.5.9, CMVP #5586, lists EdDSA; cmvp.md).
pub const SUITE: Suite = Suite::Ed25519;
pub const KEY_ALG: u8 = sync_crypto::V1_SIG_ALG as u8;
/// Pairing objects expire server-side after 15 min; the offer says so too.
pub const PAIR_TTL_SECS: u64 = 15 * 60;
/// Upper bound when probing envelope / cfg generations.
pub const MAX_GENS: u32 = 1 << 16;
pub const PAIR_PREFIX: &str = "osy-pair1:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    Transport(TransportError),
    Object(ObjectError),
    /// Malformed record or provider failure.
    Format(String),
    /// The cfg chain does not verify.
    Chain(String),
    /// No envelope under these words on this server.
    NoVault,
    /// The words open an envelope, but the server no longer knows its
    /// namespace (410 destroyed, tombstone): the vault was destroyed
    /// (burned). The envelope itself outlives a destroy — the server cannot
    /// link `loc` to the namespace — but it only holds keys of purged data and
    /// dead tokens.
    Destroyed,
    /// Someone else wrote this generation first (write-once 409): re-fetch, retry.
    Fork,
    SlotsFull,
    /// This device holds no active slot in the cfg chain (unknown device).
    NotMember,
    Expired,
}

impl fmt::Display for VaultError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VaultError::Transport(e) => write!(f, "vault: {e}"),
            VaultError::Object(e) => write!(f, "vault: {e}"),
            VaultError::Format(e) => write!(f, "vault format: {e}"),
            VaultError::Chain(e) => write!(f, "vault cfg chain: {e}"),
            VaultError::NoVault => f.write_str("no vault for these words on this server"),
            VaultError::Destroyed => f.write_str("the vault of these words was destroyed on this server (burned); nothing to join"),
            VaultError::Fork => f.write_str("vault: concurrent writer won this generation, retry"),
            VaultError::SlotsFull => f.write_str("vault: all 32 device slots are used"),
            VaultError::NotMember => f.write_str("vault: this device is not on the vault's device list"),
            VaultError::Expired => f.write_str("vault: pairing code expired"),
        }
    }
}

impl std::error::Error for VaultError {}

impl From<TransportError> for VaultError {
    fn from(e: TransportError) -> Self {
        VaultError::Transport(e)
    }
}
impl From<ObjectError> for VaultError {
    fn from(e: ObjectError) -> Self {
        VaultError::Object(e)
    }
}

fn fmt_err(e: impl fmt::Display) -> VaultError {
    VaultError::Format(e.to_string())
}

fn chain_err(s: &str) -> VaultError {
    VaultError::Chain(s.to_string())
}

pub type Result<T> = std::result::Result<T, VaultError>;


/// The server's answer for a namespace that is not (or no longer) there
/// (spec §5): 410 = destroyed (tombstone) -> [`VaultError::Destroyed`],
/// 404 = never existed -> [`VaultError::NoVault`]. Neither is retried.
fn vault_gone(e: VaultError) -> VaultError {
    match e {
        VaultError::Transport(TransportError::Api { code: ErrorCode::Gone, .. }) => VaultError::Destroyed,
        VaultError::Transport(TransportError::Api { code: ErrorCode::NotFound, .. }) => VaultError::NoVault,
        e => e,
    }
}
fn is_not_found(e: &TransportError) -> bool {
    matches!(e, TransportError::Api { code: ErrorCode::NotFound, .. })
}

fn put_once(t: &dyn Transport, kind: ObjKind, ns: &Id16, id: &Id16, bytes: &[u8]) -> Result<()> {
    match t.put(kind, ns, id, bytes) {
        Ok(_) => Ok(()),
        Err(TransportError::Api { code: ErrorCode::Conflict, .. }) => Err(VaultError::Fork),
        Err(e) => Err(e.into()),
    }
}

fn random16(p: &FipsProvider) -> Result<Id16> {
    let mut b = [0u8; 16];
    p.random(&mut b).map_err(fmt_err)?;
    Ok(Id16(b))
}

fn hmac_id(p: &FipsProvider, key: &[u8], label: &[u8], n: u32) -> Id16 {
    let mut m = label.to_vec();
    m.extend_from_slice(&n.to_be_bytes());
    let h = p.hmac_sha256(key, &m);
    let mut id = [0u8; 16];
    id.copy_from_slice(&h[..16]);
    Id16(id)
}

// ---------------------------------------------------------------- keys

/// Per-epoch children of VK (key-tree.md §3). Wiped on drop.
pub struct EpochKeys {
    pub epoch: u32,
    pub k_obj: Zeroizing<[u8; 32]>,
    pub k_bid: Zeroizing<[u8; 32]>,
    pub k_cfg: Zeroizing<[u8; 32]>,
}

impl fmt::Debug for EpochKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EpochKeys({}, <secret>)", self.epoch)
    }
}

pub fn epoch_keys(p: &FipsProvider, e: &VkEntry) -> Result<EpochKeys> {
    let child = |label: &[u8]| -> Result<Zeroizing<[u8; 32]>> {
        let mut info = label.to_vec();
        info.extend_from_slice(&e.epoch.to_be_bytes());
        let mut out = Zeroizing::new([0u8; 32]);
        p.hkdf_sha256(&e.vk, &[0u8; 32], &info, &mut *out).map_err(fmt_err)?;
        Ok(out)
    };
    Ok(EpochKeys {
        epoch: e.epoch,
        k_obj: child(b"osy obj-root v1")?,
        k_bid: child(b"osy bid v1")?,
        k_cfg: child(b"osy cfg v1")?,
    })
}

/// Suite v1: the envelope holds exactly one VK, at epoch 0 (no revocation,
/// no rotation — a compromised vault is moved, spec §4.9).
fn one_epoch(env: &Envelope) -> Result<&VkEntry> {
    match env.keyring.as_slice() {
        [g] if g.epoch == 0 => Ok(g),
        _ => Err(VaultError::Format("keyring must hold exactly one key at epoch 0 (suite v1 has no key rotation)".into())),
    }
}

/// The cfg chain is keyed with the vault's K_cfg. Slots hold public keys only.
fn cfg_key(p: &FipsProvider, env: &Envelope) -> Result<EpochKeys> {
    epoch_keys(p, one_epoch(env)?)
}

/// Batch/chunk context: the one epoch's K_obj seals and opens.
pub fn vault_ctx(p: &FipsProvider, env: &Envelope, ns: Id16) -> Result<VaultCtx> {
    let k_obj = *epoch_keys(p, one_epoch(env)?)?.k_obj;
    Ok(VaultCtx { suite: SUITE, vault_id: env.vault_id, ns, k_obj })
}

pub fn cfg_id(p: &FipsProvider, k_cfg: &[u8; 32], gen: u32) -> Id16 {
    hmac_id(p, k_cfg, b"osy cfg id v1", gen)
}

pub fn env_id(p: &FipsProvider, loc: &[u8; 16], gen: u32) -> Id16 {
    hmac_id(p, loc, b"osy env id v1", gen)
}

pub fn cfg_digest(p: &FipsProvider, signed: &[u8]) -> [u8; 32] {
    let mut m = b"osy cfg v1".to_vec();
    m.extend_from_slice(signed);
    p.sha256(&m)
}

/// K_rsig / K_destroy as module signing keys (phrase-derived, same on every device).
pub fn rsig_key(p: &FipsProvider, root: &RootKeys) -> Result<FipsSigningKey> {
    FipsSigningKey::from_seed(p, &root.k_rsig).map_err(fmt_err)
}

pub fn destroy_key(p: &FipsProvider, root: &RootKeys) -> Result<FipsSigningKey> {
    FipsSigningKey::from_seed(p, &root.k_destroy).map_err(fmt_err)
}

pub fn root_of(p: &FipsProvider, phrase: &Phrase) -> Result<RootKeys> {
    derive_root(p, &phrase.s_bytes()).map_err(fmt_err)
}

// ---------------------------------------------------------------- device

/// This device: random id + module-generated Ed25519 signing key.
pub struct Device {
    pub id: Id16,
    pub key: FipsSigningKey,
    pub pubkey: Vec<u8>,
    pub name: String,
}

impl fmt::Debug for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Device").field("id", &self.id).field("name", &self.name).finish()
    }
}

impl Device {
    pub fn generate(p: &FipsProvider, name: &str) -> Result<Device> {
        let (key, pubkey) = p.generate_signing_key(sync_crypto::V1_SIG_ALG).map_err(fmt_err)?;
        Ok(Device { id: random16(p)?, key, pubkey, name: name.to_string() })
    }

    pub fn signer(&self) -> Signer<'_, FipsSigningKey> {
        Signer { device_id: self.id, key: &self.key }
    }

    fn slot(&self) -> Slot {
        Slot { state: SlotState::Active, device_id: self.id, key_alg: KEY_ALG, pubkey: self.pubkey.clone() }
    }
}

// ---------------------------------------------------------------- cfg chain

/// Verified cfg generations, gen 0 first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    pub gens: Vec<CfgGen>,
    /// SHA-256 of each generation's full plaintext (the next `prev`).
    hashes: Vec<[u8; 32]>,
}

impl Chain {
    pub fn head(&self) -> &CfgGen {
        &self.gens[self.gens.len() - 1]
    }

    /// Commit-signature keys: the ACTIVE slots of the head generation. Slots
    /// never leave the table ([`verify_gen`]), so this is every device that
    /// ever joined; anything else is an unknown device.
    pub fn roster(&self) -> Roster {
        let mut r = Roster::default();
        for s in self.head().slots.iter().filter(|s| s.state == SlotState::Active) {
            if let Ok(alg) = SigAlg::from_byte(s.key_alg) {
                r.keys.insert(s.device_id, (alg, s.pubkey.clone()));
            }
        }
        r
    }

    /// Slot index of an active device in the head generation.
    pub fn active_slot(&self, device: &Id16) -> Option<u8> {
        self.head()
            .slots
            .iter()
            .position(|s| s.state == SlotState::Active && s.device_id == *device)
            .and_then(|i| u8::try_from(i).ok())
    }

    /// True if `self` is `old` plus zero or more generations: every
    /// generation `old` accepted is still there, byte for byte (same
    /// plaintext hash). A shorter or diverging chain is a rollback.
    pub fn extends(&self, old: &Chain) -> bool {
        self.hashes.len() >= old.hashes.len() && self.hashes[..old.hashes.len()] == old.hashes[..]
    }

    /// Unsigned successor of the head (gen+1, prev = head hash).
    fn successor(&self) -> CfgGen {
        let mut n = self.head().clone();
        n.gen += 1;
        n.prev = self.hashes[self.hashes.len() - 1];
        n.sig.clear();
        n
    }
}

/// Commit-signature keys from the cfg chain (active slots of the head).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roster {
    pub keys: BTreeMap<Id16, (SigAlg, Vec<u8>)>,
}

impl DeviceKeys for Roster {
    fn key(&self, device_id: &Id16) -> Option<(SigAlg, Vec<u8>)> {
        self.keys.get(device_id).cloned()
    }
}

/// Check `c` (signed prefix `signed`) as the successor of `prev`.
pub fn verify_gen(p: &FipsProvider, prev: Option<(&CfgGen, &[u8; 32])>, c: &CfgGen, signed: &[u8]) -> Result<()> {
    if c.min_suite != SUITE.byte() {
        return Err(chain_err("unsupported min_suite"));
    }
    // suite v1: one VK epoch per vault, no revocation / rotation (spec §4.9)
    if c.vk_epoch != 0 {
        return Err(chain_err("vk_epoch must be 0 (suite v1 has no key rotation)"));
    }
    let key: &[u8] = match prev {
        None => {
            if c.gen != 0 || c.signer != CfgSigner::Recovery {
                return Err(chain_err("genesis must be gen 0 signed by K_rsig"));
            }
            c.rsig_pub.as_slice()
        }
        Some((pg, ph)) => {
            if c.gen != pg.gen + 1 || c.prev != *ph {
                return Err(chain_err("gen / prev hash does not follow"));
            }
            if c.rsig_pub != pg.rsig_pub {
                return Err(chain_err("rsig_pub changed"));
            }
            for old in pg.slots.iter().filter(|s| s.state != SlotState::Empty) {
                let now = c.slots.iter().find(|s| s.device_id == old.device_id && s.state != SlotState::Empty);
                match now {
                    None => return Err(chain_err("a device disappeared from the slot table")),
                    Some(n) if n.key_alg != old.key_alg || n.pubkey != old.pubkey => {
                        return Err(chain_err("a device key changed"))
                    }
                    Some(_) => {}
                }
            }
            match c.signer {
                CfgSigner::Recovery => pg.rsig_pub.as_slice(),
                CfgSigner::Slot(i) => {
                    let s = &pg.slots[usize::from(i)];
                    if s.state != SlotState::Active {
                        return Err(chain_err("signer slot not active in the previous gen"));
                    }
                    s.pubkey.as_slice()
                }
            }
        }
    };
    p.verify(sync_crypto::V1_SIG_ALG, key, &cfg_digest(p, signed), &c.sig)
        .map_err(|_| chain_err("bad cfg signature"))
}

/// Walk `cfg` generations 0,1,2,… on `t` until the first missing one.
/// `expect_rsig`: K_rsig public key derived from the words, if known —
/// then the genesis must carry exactly it.
pub fn fetch_chain(
    p: &FipsProvider,
    t: &dyn Transport,
    env: &Envelope,
    ns: &Id16,
    expect_rsig: Option<&[u8]>,
) -> Result<Chain> {
    let kc = cfg_key(p, env)?;
    let mut ch = Chain { gens: Vec::new(), hashes: Vec::new() };
    for gen in 0..MAX_GENS {
        let id = cfg_id(p, &kc.k_cfg, gen);
        let bytes = match t.get(ObjKind::Cfg, ns, &id, None) {
            Ok(f) => f.bytes,
            Err(e) if is_not_found(&e) => break,
            Err(e) => return Err(e.into()),
        };
        let o = open(p, &kc.k_cfg, &ObjectCtx { suite: SUITE, vault_id: env.vault_id, obj_id: id }, &bytes)?;
        if o.kind != Kind::Cfg {
            return Err(chain_err("cfg slot holds another kind"));
        }
        let (c, n) = CfgGen::decode(&o.body).map_err(fmt_err)?;
        let prev = ch.gens.last().zip(ch.hashes.last());
        verify_gen(p, prev, &c, &o.body[..n])?;
        if gen == 0 {
            if let Some(r) = expect_rsig {
                if c.rsig_pub != r {
                    return Err(chain_err("genesis rsig_pub differs from the words"));
                }
            }
        }
        ch.hashes.push(p.sha256(&o.body));
        ch.gens.push(c);
    }
    if ch.gens.is_empty() {
        return Err(chain_err("no genesis cfg"));
    }
    Ok(ch)
}

/// Sign `next` (as `signer` with `key`), verify it against the chain head,
/// seal it under K_cfg and PUT it write-once. 409 → [`VaultError::Fork`].
#[allow(clippy::too_many_arguments)]
fn append(
    p: &FipsProvider,
    t: &dyn Transport,
    env: &Envelope,
    ns: &Id16,
    chain: &mut Chain,
    mut next: CfgGen,
    signer: CfgSigner,
    key: &FipsSigningKey,
) -> Result<()> {
    next.signer = signer;
    next.sig.clear();
    let signed = next.encode_signed().map_err(fmt_err)?;
    next.sig = p.sign(key, &cfg_digest(p, &signed)).map_err(fmt_err)?;
    verify_gen(p, chain.gens.last().zip(chain.hashes.last()), &next, &signed)?;
    let body = next.encode().map_err(fmt_err)?;
    let kc = cfg_key(p, env)?;
    let id = cfg_id(p, &kc.k_cfg, next.gen);
    let c = seal(p, &kc.k_cfg, &ObjectCtx { suite: SUITE, vault_id: env.vault_id, obj_id: id }, Kind::Cfg, &body, Compress::Never)?;
    put_once(t, ObjKind::Cfg, ns, &id, &c)?;
    chain.hashes.push(p.sha256(&body));
    chain.gens.push(next);
    Ok(())
}

// ---------------------------------------------------------------- envelope

fn env_ctx(id: Id16) -> ObjectCtx {
    ObjectCtx { suite: SUITE, vault_id: Id16::ZERO, obj_id: id }
}

pub fn put_envelope(p: &FipsProvider, t: &dyn Transport, root: &RootKeys, env: &Envelope) -> Result<()> {
    let id = env_id(p, &root.loc, env.env_gen);
    let body = Zeroizing::new(env.encode().map_err(fmt_err)?);
    let c = seal(p, &root.k_wrap, &env_ctx(id), Kind::Envelope, &body, Compress::Never)?;
    put_once(t, ObjKind::Env, &Id16::ZERO, &id, &c)
}

/// Newest envelope under `loc` (probes gen 0,1,2,…), `None` if there is none.
pub fn fetch_envelope(p: &FipsProvider, t: &dyn Transport, root: &RootKeys) -> Result<Option<Envelope>> {
    let mut newest: Option<Envelope> = None;
    for g in 0..MAX_GENS {
        let id = env_id(p, &root.loc, g);
        let bytes = match t.get(ObjKind::Env, &Id16::ZERO, &id, None) {
            Ok(f) => f.bytes,
            Err(e) if is_not_found(&e) => break,
            Err(e) => return Err(e.into()),
        };
        let o = open(p, &root.k_wrap, &env_ctx(id), &bytes)?;
        let body = Zeroizing::new(o.body);
        if o.kind != Kind::Envelope {
            return Err(VaultError::Format("env slot holds another kind".into()));
        }
        let e = Envelope::decode(&body).map_err(fmt_err)?;
        if e.env_gen != g || newest.as_ref().is_some_and(|n| n.vault_id != e.vault_id) {
            return Err(VaultError::Format("envelope gen / vault mismatch".into()));
        }
        newest = Some(e);
    }
    Ok(newest)
}

// ---------------------------------------------------------------- member

/// Everything a member device holds for one vault (daily keystore entry +
/// the verified cfg chain). `server` indexes `env.servers`.
#[derive(Debug)]
pub struct Member {
    pub env: Envelope,
    pub device: Device,
    pub chain: Chain,
    pub server: usize,
}

impl Member {
    pub fn ns(&self) -> Id16 {
        self.env.servers[self.server].ns
    }

    pub fn ctx(&self, p: &FipsProvider) -> Result<VaultCtx> {
        vault_ctx(p, &self.env, self.ns())
    }

    pub fn roster(&self) -> Roster {
        self.chain.roster()
    }

    pub fn tokens(&self) -> Tokens {
        let s = &self.env.servers[self.server];
        Tokens { read: Some(s.read_token.clone()), write: Some(s.write_token.clone()) }
    }

    fn own_slot(&self) -> Result<u8> {
        self.chain.active_slot(&self.device.id).ok_or(VaultError::NotMember)
    }

    /// Re-read the cfg chain. Errors with `NotMember` if this device holds no
    /// active slot in it.
    /// A chain that does not extend the accepted one (a server serving an
    /// older or forked cfg chain) is refused and the accepted chain kept.
    pub fn refresh_chain(&mut self, p: &FipsProvider, t: &dyn Transport) -> Result<()> {
        let chain = fetch_chain(p, t, &self.env, &self.ns(), None)?;
        if !chain.extends(&self.chain) {
            return Err(chain_err("cfg chain rolled back or forked"));
        }
        self.chain = chain;
        self.own_slot()?;
        Ok(())
    }

    /// Daily keystore entry: `u8 1 || id16 device || bytes pkcs8 || str name
    /// || u32 server || bytes envelope body`.
    pub fn to_daily(&self) -> Result<Zeroizing<Vec<u8>>> {
        let pk8 = self.device.key.to_pkcs8().map_err(fmt_err)?;
        let env = Zeroizing::new(self.env.encode().map_err(fmt_err)?);
        let mut w = Writer::new();
        w.u8(1).fixed(&self.device.id.0);
        w.bytes(&pk8).map_err(fmt_err)?.str(&self.device.name).map_err(fmt_err)?;
        w.u32(u32::try_from(self.server).map_err(fmt_err)?);
        w.bytes(&env).map_err(fmt_err)?;
        Ok(Zeroizing::new(w.into_bytes()))
    }

    /// The server tokens inside a daily entry (set them on the transport
    /// before [`Member::from_daily`] fetches the chain).
    pub fn daily_tokens(daily: &[u8]) -> Result<Tokens> {
        let (_, _, _, server, env) = Self::parse_daily(daily)?;
        let s = &env.servers[server];
        Ok(Tokens { read: Some(s.read_token.clone()), write: Some(s.write_token.clone()) })
    }

    fn parse_daily(daily: &[u8]) -> Result<(Id16, FipsSigningKey, String, usize, Envelope)> {
        let mut r = Reader::new(daily);
        if r.u8().map_err(fmt_err)? != 1 {
            return Err(VaultError::Format("daily entry version".into()));
        }
        let id = Id16(r.array::<16>().map_err(fmt_err)?);
        let key = FipsSigningKey::from_pkcs8(r.bytes(4096).map_err(fmt_err)?).map_err(fmt_err)?;
        let name = r.str().map_err(fmt_err)?.to_string();
        let server = r.u32().map_err(fmt_err)? as usize;
        let env = Envelope::decode(r.bytes(1 << 20).map_err(fmt_err)?).map_err(fmt_err)?;
        r.finish().map_err(fmt_err)?;
        if server >= env.servers.len() {
            return Err(VaultError::Format("daily entry server index".into()));
        }
        Ok((id, key, name, server, env))
    }

    /// Load from the daily entry and re-verify the chain from `t`.
    pub fn from_daily(p: &FipsProvider, t: &dyn Transport, daily: &[u8]) -> Result<Member> {
        let (id, key, name, server, env) = Self::parse_daily(daily)?;
        let pubkey = key.public_key();
        let device = Device { id, key, pubkey, name };
        let ns = env.servers[server].ns;
        let chain = fetch_chain(p, t, &env, &ns, None).map_err(vault_gone)?;
        let mut m = Member { env, device, chain, server };
        m.own_slot()?;
        m.server = server;
        Ok(m)
    }
}

fn server_index(env: &Envelope, url: &str) -> Result<usize> {
    if env.servers.is_empty() {
        return Err(VaultError::Format("envelope lists no server".into()));
    }
    Ok(env.servers.iter().position(|s| s.url == url).unwrap_or(0))
}

/// New vault on `url`: enroll with the one-time token (pub_destroy from the
/// words), random vault_id + VK epoch 0, cfg gen 0 (this device in slot 0,
/// signed by K_rsig), envelope gen 0 under loc. Sets the tokens on `t`.
pub fn create_vault(
    p: &FipsProvider,
    t: &HttpTransport,
    url: &str,
    enrollment_token: &str,
    phrase: &Phrase,
    device_name: &str,
) -> Result<Member> {
    let root = root_of(p, phrase)?;
    let kd = destroy_key(p, &root)?;
    let er = t.enroll(enrollment_token, &kd.public_key())?;
    t.set_tokens(Tokens::from(&er));
    let server_key = URL_SAFE_NO_PAD.decode(er.server_key.as_bytes()).map_err(fmt_err)?;
    let mut vk = [0u8; 32];
    p.random(&mut vk).map_err(fmt_err)?;
    let env = Envelope {
        env_gen: 0,
        vault_id: random16(p)?,
        keyring: vec![VkEntry { epoch: 0, vk }],
        servers: vec![ServerEntry {
            url: url.to_string(),
            ns: er.ns,
            read_token: er.read_token.clone(),
            write_token: er.write_token.clone(),
            spki_pins: Vec::new(),
            server_key,
        }],
    };
    vk.fill(0);
    let device = Device::generate(p, device_name)?;
    let rk = rsig_key(p, &root)?;
    let mut slots = vec![Slot::empty(); SLOTS];
    slots[0] = device.slot();
    let genesis = CfgGen {
        gen: 0,
        prev: [0; 32],
        min_suite: SUITE.byte(),
        vk_epoch: 0,
        rsig_pub: rk.public_key(),
        slots,
        signer: CfgSigner::Recovery,
        sig: Vec::new(),
    };
    let mut chain = Chain { gens: Vec::new(), hashes: Vec::new() };
    append(p, t, &env, &er.ns, &mut chain, genesis, CfgSigner::Recovery, &rk)?;
    put_envelope(p, t, &root, &env)?;
    Ok(Member { env, device, chain, server: 0 })
}

/// Fresh device from the words + URL only: newest envelope under loc →
/// tokens + keyring → verified cfg chain (genesis must carry the words'
/// K_rsig) → a new slot for this device signed by K_rsig.
pub fn bootstrap(p: &FipsProvider, t: &HttpTransport, url: &str, phrase: &Phrase, device_name: &str) -> Result<Member> {
    let root = root_of(p, phrase)?;
    let env = fetch_envelope(p, t, &root)?.ok_or(VaultError::NoVault)?;
    let server = server_index(&env, url)?;
    let ns = env.servers[server].ns;
    let s = &env.servers[server];
    t.set_tokens(Tokens { read: Some(s.read_token.clone()), write: Some(s.write_token.clone()) });
    let rk = rsig_key(p, &root)?;
    let mut chain = fetch_chain(p, t, &env, &ns, Some(rk.public_key().as_slice())).map_err(vault_gone)?;
    let device = Device::generate(p, device_name)?;
    add_slot(p, t, &env, &ns, &mut chain, &device, CfgSigner::Recovery, &rk)?;
    Ok(Member { env, device, chain, server })
}

#[allow(clippy::too_many_arguments)]
fn add_slot(
    p: &FipsProvider,
    t: &dyn Transport,
    env: &Envelope,
    ns: &Id16,
    chain: &mut Chain,
    device: &Device,
    signer: CfgSigner,
    key: &FipsSigningKey,
) -> Result<()> {
    let mut next = chain.successor();
    if next.slots.iter().any(|s| s.device_id == device.id && s.state != SlotState::Empty) {
        return Err(chain_err("device id already enrolled"));
    }
    let free = next.slots.iter().position(|s| s.state == SlotState::Empty).ok_or(VaultError::SlotsFull)?;
    next.slots[free] = device.slot();
    append(p, t, env, ns, chain, next, signer, key)
}

/// 2-step destroy with the words (K_destroy proof), receipt checked against
/// the server key pinned at enrollment (envelope).
pub fn destroy(p: &FipsProvider, t: &dyn Transport, env: &Envelope, server: usize, root: &RootKeys, ts: i64) -> Result<ReceiptBody> {
    let s = env.servers.get(server).ok_or_else(|| VaultError::Format("server index".into()))?;
    let kd = destroy_key(p, root)?;
    Ok(destroy_vault(t, p, &kd, &s.ns, &s.server_key, ts)?)
}

// ---------------------------------------------------------------- pairing

/// The QR / pairing code: server url + ns + offer id + one-time K_p.
/// Text form `osy-pair1:<base64url>`. Debug is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct PairCode {
    pub url: String,
    pub ns: Id16,
    pub offer: Id16,
    pub k_p: [u8; 32],
}

impl fmt::Debug for PairCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairCode").field("url", &self.url).field("k_p", &"<secret>").finish()
    }
}

impl Drop for PairCode {
    fn drop(&mut self) {
        self.k_p.fill(0);
    }
}

impl PairCode {
    pub fn to_text(&self) -> Result<Zeroizing<String>> {
        let mut w = Writer::new();
        w.u8(1).str(&self.url).map_err(fmt_err)?;
        w.fixed(&self.ns.0).fixed(&self.offer.0).fixed(&self.k_p);
        let raw = Zeroizing::new(w.into_bytes());
        Ok(Zeroizing::new(format!("{PAIR_PREFIX}{}", URL_SAFE_NO_PAD.encode(&raw[..]))))
    }

    pub fn parse(text: &str) -> Result<PairCode> {
        let b64 = text.trim().strip_prefix(PAIR_PREFIX).ok_or_else(|| VaultError::Format("not a pairing code".into()))?;
        let raw = Zeroizing::new(URL_SAFE_NO_PAD.decode(b64.as_bytes()).map_err(fmt_err)?);
        let mut r = Reader::new(&raw);
        if r.u8().map_err(fmt_err)? != 1 {
            return Err(VaultError::Format("pairing code version".into()));
        }
        let url = r.str().map_err(fmt_err)?.to_string();
        let ns = Id16(r.array::<16>().map_err(fmt_err)?);
        let offer = Id16(r.array::<16>().map_err(fmt_err)?);
        let k_p = r.array::<32>().map_err(fmt_err)?;
        r.finish().map_err(fmt_err)?;
        Ok(PairCode { url, ns, offer, k_p })
    }

    fn reply_id(&self, p: &FipsProvider) -> Id16 {
        hmac_id(p, &self.k_p, b"osy pair reply v1", 0)
    }
}

fn seal_pair(p: &FipsProvider, k_p: &[u8; 32], id: Id16, m: &PairMsg) -> Result<Vec<u8>> {
    let body = Zeroizing::new(m.encode().map_err(fmt_err)?);
    Ok(seal(p, k_p, &env_ctx(id), Kind::Pairing, &body, Compress::Never)?)
}

fn take_pair(p: &FipsProvider, t: &dyn Transport, code: &PairCode, id: Id16) -> Result<PairMsg> {
    let m = read_pair(p, t, code, id)?;
    // single use: the reader purges it (server: DELETE on pair = purge)
    t.delete(ObjKind::Pair, &code.ns, &id)?;
    Ok(m)
}

fn read_pair(p: &FipsProvider, t: &dyn Transport, code: &PairCode, id: Id16) -> Result<PairMsg> {
    let f = t.get(ObjKind::Pair, &code.ns, &id, None)?;
    let o = open(p, &code.k_p, &env_ctx(id), &f.bytes)?;
    let body = Zeroizing::new(o.body);
    if o.kind != Kind::Pairing {
        return Err(VaultError::Format("pairing slot holds another kind".into()));
    }
    PairMsg::decode(&body).map_err(fmt_err)
}

/// Inviter, step 1: PUT an offer (envelope body + optionally the words) under
/// a fresh K_p and return the code to show as QR.
pub fn pair_offer(p: &FipsProvider, t: &dyn Transport, m: &Member, words: Option<&Phrase>, now_unix: u64) -> Result<PairCode> {
    m.own_slot()?;
    let mut k_p = [0u8; 32];
    p.random(&mut k_p).map_err(fmt_err)?;
    let code = PairCode { url: m.env.servers[m.server].url.clone(), ns: m.ns(), offer: random16(p)?, k_p };
    k_p.fill(0);
    let s_bytes: Option<[u8; S_BYTES_LEN]> = words.map(|w| *w.s_bytes());
    let msg = PairMsg::Offer {
        expires_unix: now_unix + PAIR_TTL_SECS,
        vaults: vec![PairVault { envelope: m.env.clone(), s_bytes }],
    };
    let c = seal_pair(p, &code.k_p, code.offer, &msg)?;
    put_once(t, ObjKind::Pair, &code.ns, &code.offer, &c)?;
    Ok(code)
}

/// New device after [`pair_accept`]: envelope, own device, words (if sent).
#[derive(Debug)]
pub struct Joining {
    pub env: Envelope,
    pub device: Device,
    pub words: Option<Phrase>,
    pub server: usize,
}

/// New device, step 2: read + purge the offer, generate the device key and
/// answer over the same K_p with its public key. Sets the tokens on `t`.
pub fn pair_accept(p: &FipsProvider, t: &HttpTransport, code: &PairCode, device_name: &str, now_unix: u64) -> Result<Joining> {
    let PairMsg::Offer { expires_unix, mut vaults } = take_pair(p, t, code, code.offer)? else {
        return Err(VaultError::Format("expected a pairing offer".into()));
    };
    if now_unix > expires_unix {
        return Err(VaultError::Expired);
    }
    let pv = vaults.remove(0);
    let server = pv
        .envelope
        .servers
        .iter()
        .position(|s| s.ns == code.ns)
        .ok_or_else(|| VaultError::Format("offer does not list the code's server".into()))?;
    let s = &pv.envelope.servers[server];
    t.set_tokens(Tokens { read: Some(s.read_token.clone()), write: Some(s.write_token.clone()) });
    let words = match pv.s_bytes {
        Some(b) => Some(Phrase::from_s_bytes(&b).map_err(fmt_err)?),
        None => None,
    };
    let device = Device::generate(p, device_name)?;
    let reply = PairMsg::Reply { device_id: device.id, key_alg: KEY_ALG, pubkey: device.pubkey.clone(), name: device.name.clone() };
    let c = seal_pair(p, &code.k_p, code.reply_id(p), &reply)?;
    put_once(t, ObjKind::Pair, &code.ns, &code.reply_id(p), &c)?;
    Ok(Joining { env: pv.envelope, device, words, server })
}

/// Inviter, step 3: read the reply, give the new device a slot, then purge it
/// (cfg gen+1 signed by this device). The reply is single use, so a stale
/// local chain (someone else appended meanwhile → 409) is re-fetched and
/// the append retried. Returns the new device id and name.
pub fn pair_complete(p: &FipsProvider, t: &dyn Transport, m: &mut Member, code: &PairCode) -> Result<(Id16, String)> {
    m.own_slot()?;
    // The reply is purged only after the slot is on the server: a failed
    // append leaves it in place, so the inviter can retry (spec §4.6).
    let reply = code.reply_id(p);
    let PairMsg::Reply { device_id, key_alg, pubkey, name } = read_pair(p, t, code, reply)? else {
        return Err(VaultError::Format("expected a pairing reply".into()));
    };
    if key_alg != KEY_ALG {
        return Err(VaultError::Format("pairing reply key_alg".into()));
    }
    let ns = m.ns();
    for _ in 0..8 {
        let own = m.own_slot()?;
        let mut next = m.chain.successor();
        if next.slots.iter().any(|s| s.device_id == device_id && s.state == SlotState::Active && s.pubkey == pubkey) {
            // an earlier attempt's append landed (its reply was lost): done
            let _ = t.delete(ObjKind::Pair, &code.ns, &reply);
            return Ok((device_id, name));
        }
        if next.slots.iter().any(|s| s.device_id == device_id && s.state != SlotState::Empty) {
            return Err(chain_err("device id already enrolled"));
        }
        let free = next.slots.iter().position(|s| s.state == SlotState::Empty).ok_or(VaultError::SlotsFull)?;
        next.slots[free] = Slot { state: SlotState::Active, device_id, key_alg, pubkey: pubkey.clone() };
        match append(p, t, &m.env, &ns, &mut m.chain, next, CfgSigner::Slot(own), &m.device.key) {
            Err(VaultError::Fork) => {
                // a concurrent extension is installed and retried; a shorter
                // or divergent chain (rollback) is refused like refresh_chain
                let chain = fetch_chain(p, t, &m.env, &ns, None)?;
                if !chain.extends(&m.chain) {
                    return Err(chain_err("cfg chain rolled back or forked"));
                }
                m.chain = chain;
            }
            Ok(()) => {
                // best effort: a leftover reply holds only a public key and
                // the server GC drops pair rows; a second complete is refused
                // as "already enrolled"
                let _ = t.delete(ObjKind::Pair, &code.ns, &reply);
                return Ok((device_id, name));
            }
            Err(e) => return Err(e),
        }
    }
    Err(VaultError::Fork)
}

/// New device, step 4: once the inviter completed, fetch the chain and
/// check this device holds an active slot.
pub fn pair_join(p: &FipsProvider, t: &dyn Transport, j: Joining) -> Result<(Member, Option<Phrase>)> {
    let ns = j.env.servers[j.server].ns;
    let chain = fetch_chain(p, t, &j.env, &ns, None)?;
    let m = Member { env: j.env, device: j.device, chain, server: j.server };
    m.own_slot()?;
    Ok((m, j.words))
}
