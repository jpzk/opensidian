// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! M5 onboarding records (docs/sync/spec.md §4.4–4.6): the signed cfg
//! generation chain, the envelope under the locator and the pairing
//! messages. Pure canonical encodings; sealing, signing and verification
//! live in `sync_core::vault`.

use crate::enc::{DecodeError, EncodeError, Reader, Writer};
use crate::Id16;

pub const CFG_VERSION: u8 = 0x01;
pub const ENV_VERSION: u8 = 0x01;
pub const PAIR_VERSION: u8 = 0x01;
/// The slot table always has exactly this many entries (fixed size, so the
/// object size does not leak the device count).
pub const SLOTS: usize = 32;
/// Keyring and server-list caps of an envelope.
pub const MAX_EPOCHS: usize = 256;
pub const MAX_SERVERS: usize = 16;
pub const MAX_PINS: usize = 8;
pub const MAX_TOKEN: usize = 64;
pub const MAX_PUBKEY: usize = 65;
pub const MAX_SIG: usize = 160;
/// Vaults per pairing offer.
pub const MAX_PAIR_VAULTS: usize = 16;
/// Encoded envelope body inside a pairing offer.
pub const MAX_ENV_BODY: usize = 64 * 1024;
/// `S_bytes` (11 × u16 word indices).
pub const S_BYTES_LEN: usize = 22;

fn bad(what: &'static str) -> DecodeError {
    DecodeError::Invalid(what)
}

fn enc_bad(e: DecodeError) -> EncodeError {
    match e {
        DecodeError::Invalid(w) => EncodeError::Invalid(w),
        _ => EncodeError::Invalid("record"),
    }
}

/// Public-key length of a `key_alg` byte (1 Ed25519, 2 P-256).
fn pubkey_len(alg: u8) -> Option<usize> {
    match alg {
        1 => Some(32),
        2 => Some(65),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SlotState {
    Empty = 0,
    Active = 1,
    Revoked = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub state: SlotState,
    pub device_id: Id16,
    pub key_alg: u8,
    pub pubkey: Vec<u8>,
}

impl Slot {
    pub fn empty() -> Slot {
        Slot { state: SlotState::Empty, device_id: Id16::ZERO, key_alg: 0, pubkey: Vec::new() }
    }

    fn validate(&self) -> Result<(), DecodeError> {
        match self.state {
            SlotState::Empty => {
                if self.device_id != Id16::ZERO || self.key_alg != 0 || !self.pubkey.is_empty() {
                    return Err(bad("empty slot must be all zero"));
                }
            }
            SlotState::Active | SlotState::Revoked => {
                if pubkey_len(self.key_alg) != Some(self.pubkey.len()) {
                    return Err(bad("slot key_alg / pubkey length"));
                }
                if self.device_id == Id16::ZERO {
                    return Err(bad("slot device_id zero"));
                }
            }
        }
        Ok(())
    }
}

/// Who signed a cfg generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfgSigner {
    /// An active slot of the PREVIOUS generation.
    Slot(u8),
    /// The phrase-derived recovery key K_rsig (`rsig_pub`).
    Recovery,
}

/// One cfg generation (spec §4.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfgGen {
    pub gen: u32,
    /// SHA-256 of the previous generation's full plaintext, zero at gen 0.
    pub prev: [u8; 32],
    pub min_suite: u8,
    pub vk_epoch: u32,
    /// Public half of K_rsig (32-byte Ed25519 in suite 0x01, 65-byte P-256
    /// point in 0x02). Identical in every generation.
    pub rsig_pub: Vec<u8>,
    /// Exactly [`SLOTS`] entries.
    pub slots: Vec<Slot>,
    pub signer: CfgSigner,
    pub sig: Vec<u8>,
}

impl CfgGen {
    pub fn validate(&self) -> Result<(), DecodeError> {
        if self.slots.len() != SLOTS {
            return Err(bad("cfg must carry exactly 32 slots"));
        }
        // key lengths follow the suite (0x01 Ed25519: 32, 0x02 P-256: 65)
        let klen = pubkey_len(self.min_suite).ok_or_else(|| bad("min_suite"))?;
        if self.rsig_pub.len() != klen {
            return Err(bad("rsig_pub length"));
        }
        if self.gen == 0 && self.prev != [0u8; 32] {
            return Err(bad("genesis prev must be zero"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for s in &self.slots {
            s.validate()?;
            if s.state != SlotState::Empty && !seen.insert(s.device_id) {
                return Err(bad("duplicate device_id in slots"));
            }
        }
        if let CfgSigner::Slot(i) = self.signer {
            if usize::from(i) >= SLOTS {
                return Err(bad("signer_slot out of range"));
            }
        }
        Ok(())
    }

    /// Bytes covered by the signature (everything before `bytes sig`).
    pub fn encode_signed(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate().map_err(enc_bad)?;
        let mut w = Writer::new();
        w.u8(CFG_VERSION).u32(self.gen).fixed(&self.prev).u8(self.min_suite).u32(self.vk_epoch);
        w.bytes(&self.rsig_pub)?;
        w.list_len(self.slots.len())?;
        for s in &self.slots {
            w.u8(s.state as u8).fixed(&s.device_id.0).u8(s.key_alg);
            w.bytes(&s.pubkey)?;
        }
        match self.signer {
            CfgSigner::Slot(i) => w.u8(1).u8(i),
            CfgSigner::Recovery => w.u8(2).u8(0),
        };
        Ok(w.into_bytes())
    }

    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut out = self.encode_signed()?;
        let mut w = Writer::new();
        w.bytes(&self.sig)?;
        out.extend_from_slice(w.as_bytes());
        Ok(out)
    }

    /// Parse + validate; returns the record and the signed-prefix length.
    pub fn decode(buf: &[u8]) -> Result<(CfgGen, usize), DecodeError> {
        let mut r = Reader::new(buf);
        if r.u8()? != CFG_VERSION {
            return Err(bad("unsupported cfg_version"));
        }
        let gen = r.u32()?;
        let prev = r.array::<32>()?;
        let min_suite = r.u8()?;
        let vk_epoch = r.u32()?;
        let rsig_pub = r.bytes(MAX_PUBKEY)?.to_vec();
        let n = r.list_len(SLOTS)?;
        let mut slots = Vec::with_capacity(n);
        for _ in 0..n {
            let state = match r.u8()? {
                0 => SlotState::Empty,
                1 => SlotState::Active,
                2 => SlotState::Revoked,
                _ => return Err(bad("slot state")),
            };
            let device_id = Id16(r.array::<16>()?);
            let key_alg = r.u8()?;
            let pubkey = r.bytes(MAX_PUBKEY)?.to_vec();
            slots.push(Slot { state, device_id, key_alg, pubkey });
        }
        let signer = match (r.u8()?, r.u8()?) {
            (1, i) => CfgSigner::Slot(i),
            (2, 0) => CfgSigner::Recovery,
            _ => return Err(bad("signer_kind / signer_slot")),
        };
        let signed_len = buf.len() - r.remaining();
        let sig = r.bytes(MAX_SIG)?.to_vec();
        r.finish()?;
        let c = CfgGen { gen, prev, min_suite, vk_epoch, rsig_pub, slots, signer, sig };
        c.validate()?;
        Ok((c, signed_len))
    }
}

/// One VK epoch of the keyring.
#[derive(Clone, PartialEq, Eq)]
pub struct VkEntry {
    pub epoch: u32,
    pub vk: [u8; 32],
}

impl std::fmt::Debug for VkEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "VkEntry({}, <secret>)", self.epoch)
    }
}

/// A server the vault syncs with. Tokens are the base64url strings from
/// enrollment; `server_key` is the destroy-receipt key pinned at enrollment.
#[derive(Clone, PartialEq, Eq)]
pub struct ServerEntry {
    pub url: String,
    pub ns: Id16,
    pub read_token: String,
    pub write_token: String,
    pub spki_pins: Vec<[u8; 32]>,
    pub server_key: Vec<u8>,
}

impl std::fmt::Debug for ServerEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerEntry")
            .field("url", &self.url)
            .field("ns", &self.ns)
            .field("tokens", &"<redacted>")
            .field("spki_pins", &self.spki_pins.len())
            .finish()
    }
}

/// Envelope body (spec §4.5), sealed under K_wrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub env_gen: u32,
    pub vault_id: Id16,
    /// Strictly ascending epochs, at least one.
    pub keyring: Vec<VkEntry>,
    pub servers: Vec<ServerEntry>,
}

fn token_str(s: &str) -> Result<(), DecodeError> {
    if s.is_empty() || s.len() > MAX_TOKEN || !s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(bad("token must be base64url"));
    }
    Ok(())
}

impl Envelope {
    pub fn validate(&self) -> Result<(), DecodeError> {
        if self.keyring.is_empty() || self.keyring.len() > MAX_EPOCHS {
            return Err(bad("keyring size"));
        }
        if self.keyring.windows(2).any(|w| w[0].epoch >= w[1].epoch) {
            return Err(bad("keyring epochs must be strictly ascending"));
        }
        if self.servers.len() > MAX_SERVERS {
            return Err(bad("too many servers"));
        }
        for s in &self.servers {
            token_str(&s.read_token)?;
            token_str(&s.write_token)?;
            if s.spki_pins.len() > MAX_PINS {
                return Err(bad("too many pins"));
            }
            // empty (not pinned yet), Ed25519 (32, suite v1) or a P-256 point (65)
            if !matches!(s.server_key.len(), 0 | 32 | 65) {
                return Err(bad("server_key length"));
            }
        }
        Ok(())
    }

    /// Highest epoch (the one new objects are written in).
    pub fn current(&self) -> &VkEntry {
        // validate() guarantees a non-empty keyring; encode/decode enforce it
        &self.keyring[self.keyring.len() - 1]
    }

    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        self.validate().map_err(enc_bad)?;
        let mut w = Writer::new();
        w.u8(ENV_VERSION).u32(self.env_gen).fixed(&self.vault_id.0);
        w.list_len(self.keyring.len())?;
        for k in &self.keyring {
            w.u32(k.epoch).fixed(&k.vk);
        }
        w.list_len(self.servers.len())?;
        for s in &self.servers {
            w.str(&s.url)?.fixed(&s.ns.0);
            w.bytes(s.read_token.as_bytes())?;
            w.bytes(s.write_token.as_bytes())?;
            w.list_len(s.spki_pins.len())?;
            for p in &s.spki_pins {
                w.fixed(p);
            }
            w.bytes(&s.server_key)?;
        }
        Ok(w.into_bytes())
    }

    pub fn decode(buf: &[u8]) -> Result<Envelope, DecodeError> {
        let mut r = Reader::new(buf);
        if r.u8()? != ENV_VERSION {
            return Err(bad("unsupported env_version"));
        }
        let env_gen = r.u32()?;
        let vault_id = Id16(r.array::<16>()?);
        let n = r.list_len(MAX_EPOCHS)?;
        let mut keyring = Vec::with_capacity(n);
        for _ in 0..n {
            let epoch = r.u32()?;
            let vk = r.array::<32>()?;
            keyring.push(VkEntry { epoch, vk });
        }
        let n = r.list_len(MAX_SERVERS)?;
        let mut servers = Vec::with_capacity(n);
        for _ in 0..n {
            let url = r.str()?.to_string();
            let ns = Id16(r.array::<16>()?);
            let tok = |r: &mut Reader<'_>| -> Result<String, DecodeError> {
                let b = r.bytes(MAX_TOKEN)?;
                String::from_utf8(b.to_vec()).map_err(|_| DecodeError::BadUtf8)
            };
            let read_token = tok(&mut r)?;
            let write_token = tok(&mut r)?;
            let np = r.list_len(MAX_PINS)?;
            let mut spki_pins = Vec::with_capacity(np);
            for _ in 0..np {
                spki_pins.push(r.array::<32>()?);
            }
            let server_key = r.bytes(MAX_PUBKEY)?.to_vec();
            servers.push(ServerEntry { url, ns, read_token, write_token, spki_pins, server_key });
        }
        r.finish()?;
        let e = Envelope { env_gen, vault_id, keyring, servers };
        e.validate()?;
        Ok(e)
    }
}

/// One vault inside a pairing offer: its envelope body and, optionally, the
/// backup words (so the new device can later follow a VK rotation, which
/// is only published under K_wrap — key-tree.md §6).
#[derive(Clone, PartialEq, Eq)]
pub struct PairVault {
    pub envelope: Envelope,
    pub s_bytes: Option<[u8; S_BYTES_LEN]>,
}

impl std::fmt::Debug for PairVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairVault")
            .field("vault_id", &self.envelope.vault_id)
            .field("s_bytes", &self.s_bytes.map(|_| "<secret>"))
            .finish()
    }
}

/// Pairing messages (spec §4.6), both sealed under the one-time K_p — the
/// channel is symmetric: the offer goes inviter → new device, the reply
/// (the new device's public key) goes back the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairMsg {
    Offer { expires_unix: u64, vaults: Vec<PairVault> },
    Reply { device_id: Id16, key_alg: u8, pubkey: Vec<u8>, name: String },
}

impl PairMsg {
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = Writer::new();
        w.u8(PAIR_VERSION);
        match self {
            PairMsg::Offer { expires_unix, vaults } => {
                if vaults.is_empty() || vaults.len() > MAX_PAIR_VAULTS {
                    return Err(EncodeError::Invalid("pair vault count"));
                }
                w.u8(1).u64(*expires_unix).list_len(vaults.len())?;
                for v in vaults {
                    w.bytes(&v.envelope.encode()?)?;
                    w.opt_tag(v.s_bytes.is_some());
                    if let Some(s) = &v.s_bytes {
                        w.fixed(s);
                    }
                }
            }
            PairMsg::Reply { device_id, key_alg, pubkey, name } => {
                if pubkey_len(*key_alg) != Some(pubkey.len()) || *device_id == Id16::ZERO {
                    return Err(EncodeError::Invalid("pair reply key"));
                }
                w.u8(2).fixed(&device_id.0).u8(*key_alg);
                w.bytes(pubkey)?.str(name)?;
            }
        }
        Ok(w.into_bytes())
    }

    pub fn decode(buf: &[u8]) -> Result<PairMsg, DecodeError> {
        let mut r = Reader::new(buf);
        if r.u8()? != PAIR_VERSION {
            return Err(bad("unsupported pairing version"));
        }
        let m = match r.u8()? {
            1 => {
                let expires_unix = r.u64()?;
                let n = r.list_len(MAX_PAIR_VAULTS)?;
                if n == 0 {
                    return Err(bad("pair vault count"));
                }
                let mut vaults = Vec::with_capacity(n);
                for _ in 0..n {
                    let envelope = Envelope::decode(r.bytes(MAX_ENV_BODY)?)?;
                    let s_bytes = if r.opt_tag()? { Some(r.array::<S_BYTES_LEN>()?) } else { None };
                    vaults.push(PairVault { envelope, s_bytes });
                }
                PairMsg::Offer { expires_unix, vaults }
            }
            2 => {
                let device_id = Id16(r.array::<16>()?);
                let key_alg = r.u8()?;
                let pubkey = r.bytes(MAX_PUBKEY)?.to_vec();
                let name = r.str()?.to_string();
                if pubkey_len(key_alg) != Some(pubkey.len()) || device_id == Id16::ZERO {
                    return Err(bad("pair reply key"));
                }
                PairMsg::Reply { device_id, key_alg, pubkey, name }
            }
            _ => return Err(bad("pairing message type")),
        };
        r.finish()?;
        Ok(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CfgGen {
        let mut slots = vec![Slot::empty(); SLOTS];
        slots[0] = Slot { state: SlotState::Active, device_id: Id16([7; 16]), key_alg: 2, pubkey: vec![4; 65] };
        CfgGen {
            gen: 0,
            prev: [0; 32],
            min_suite: 2,
            vk_epoch: 0,
            rsig_pub: vec![4; 65],
            slots,
            signer: CfgSigner::Recovery,
            sig: vec![1; 64],
        }
    }

    #[test]
    fn cfg_key_lengths_follow_the_suite() {
        // suite 0x01 (v1, Ed25519): 32-byte K_rsig public key
        let mut c = cfg();
        c.min_suite = 1;
        c.slots[0] = Slot { state: SlotState::Active, device_id: Id16([7; 16]), key_alg: 1, pubkey: vec![9; 32] };
        assert!(c.encode().is_err(), "65-byte rsig_pub under suite 1");
        c.rsig_pub = vec![9; 32];
        let (d, _) = CfgGen::decode(&c.encode().unwrap()).unwrap();
        assert_eq!(d, c);
        let mut x = cfg();
        x.rsig_pub = vec![9; 32];
        assert!(x.encode().is_err(), "32-byte rsig_pub under suite 2");
        let mut x = c.clone();
        x.min_suite = 3;
        assert!(x.encode().is_err(), "unknown suite");
    }

    #[test]
    fn cfg_round_trip_and_rules() {
        let c = cfg();
        let b = c.encode().unwrap();
        let (d, n) = CfgGen::decode(&b).unwrap();
        assert_eq!(d, c);
        assert_eq!(&b[..n], &c.encode_signed().unwrap()[..]);
        let mut x = c.clone();
        x.slots.pop();
        assert!(x.encode().is_err(), "31 slots");
        let mut x = c.clone();
        x.slots[1] = x.slots[0].clone();
        assert!(x.encode().is_err(), "duplicate device");
        let mut x = c.clone();
        x.slots[2].key_alg = 2;
        assert!(x.encode().is_err(), "non-zero empty slot");
        let mut x = c.clone();
        x.gen = 0;
        x.prev = [1; 32];
        assert!(x.encode().is_err(), "genesis prev");
        let mut t = b.clone();
        t.push(0);
        assert!(CfgGen::decode(&t).is_err(), "trailing");
    }

    fn env() -> Envelope {
        Envelope {
            env_gen: 1,
            vault_id: Id16([3; 16]),
            keyring: vec![VkEntry { epoch: 0, vk: [1; 32] }, VkEntry { epoch: 1, vk: [2; 32] }],
            servers: vec![ServerEntry {
                url: "https://sync.example:8443".into(),
                ns: Id16([9; 16]),
                read_token: "rrrr".into(),
                write_token: "w-w_".into(),
                spki_pins: vec![[5; 32]],
                server_key: vec![4; 65],
            }],
        }
    }

    #[test]
    fn envelope_round_trip_and_rules() {
        let e = env();
        assert_eq!(Envelope::decode(&e.encode().unwrap()).unwrap(), e);
        assert_eq!(e.current().epoch, 1);
        let mut x = e.clone();
        x.keyring.reverse();
        assert!(x.encode().is_err(), "descending epochs");
        let mut x = e.clone();
        x.keyring.clear();
        assert!(x.encode().is_err(), "empty keyring");
        let mut x = e.clone();
        x.servers[0].read_token = "a b".into();
        assert!(x.encode().is_err(), "token alphabet");
        assert!(!format!("{e:?}").contains("rrrr"), "debug redacts tokens");
    }

    #[test]
    fn pairing_round_trip() {
        let o = PairMsg::Offer { expires_unix: 99, vaults: vec![PairVault { envelope: env(), s_bytes: Some([8; 22]) }] };
        assert_eq!(PairMsg::decode(&o.encode().unwrap()).unwrap(), o);
        let r = PairMsg::Reply { device_id: Id16([1; 16]), key_alg: 2, pubkey: vec![4; 65], name: "laptop".into() };
        assert_eq!(PairMsg::decode(&r.encode().unwrap()).unwrap(), r);
        let bad = PairMsg::Reply { device_id: Id16([1; 16]), key_alg: 2, pubkey: vec![4; 32], name: String::new() };
        assert!(bad.encode().is_err());
        assert!(PairMsg::Offer { expires_unix: 1, vaults: vec![] }.encode().is_err());
    }
}
