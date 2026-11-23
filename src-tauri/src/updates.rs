// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* goal/settings-general: Settings > General > "Check for updates".

   One button press = one check, nothing in the background (Automatic updates
   is deactivated). The check:
     1. GET https://api.github.com/repos/jpzk/opensidian/releases (through
        plugin_store::http_get: https only, exact host allowlist, redirects
        re-checked by hand, byte caps, anonymous);
     2. keeps published, non-draft, non-prerelease releases whose tag is a plain
        numeric version (v0.6.2, v0.4), highest first;
     3. for each, highest first, until one verifies (#294 F-58, at most
        MAX_TRIES downloads of a sums pair): downloads THAT release's SHA256SUMS
        and SHA256SUMS.asc (and nothing else: no installer, no binary);
     4. verifies the detached OpenPGP signature over SHA256SUMS against a key
        EMBEDDED in this binary (data/updates/opensidian-release.pub.asc, and the
        SUCCESSOR slot once the operator fills it, #294 F-84), whose v4
        fingerprint must equal the pinned one — the full 40 hex digits, never
        just the key id — and requires the signature's issuer fingerprint
        subpacket to be that same fingerprint and a creation-time subpacket;
     5. parses the signed SHA256SUMS into name -> sha256 and requires every line
        to name a file of THIS version (opensidian-<ver>-…), so an old release's
        genuinely signed sums re-attached to a newer tag are not a signature for
        that tag;
     6. (#294 F-11) requires EVERY downloadable asset GitHub lists for the
        release to be named in the signed sums, and GitHub's own sha256 `digest`
        of it to equal the signed value. An unlisted extra asset, a missing or
        different digest = "release assets do not match the signed checksums",
        never offered. The signature covers the sums; this step is what makes the
        verdict say something about the files the release link leads to.
   The offered release is the highest that passes 3-6. Every higher one that
   failed is shown as a warning (unsigned / assets do not match). The highest
   version that ever passed is remembered in ~/.opensidian.json (SEEN_KEY, a
   monotonic cfgstore Op, outside any vault); a check whose best verified
   release is below it reports "rollback" and offers nothing (a hidden newer
   release, a frozen list). The verdict names the signer and shows the signed
   sha256 of this platform's release file: the user verifies the download
   against it — the app itself never downloads or installs a release.

   The OpenPGP subset is deliberately tiny and strict: ASCII armor, v4
   signature packets of type 0x00 (binary document), EdDSA (algo 22) over
   SHA-256/SHA-512, v4 Ed25519 public key. Anything else is refused, not
   guessed at. The curve arithmetic is ed25519-compact (pinned, no deps);
   SHA-2 is the already-locked sha2. SHA-1 appears ONLY to compute the v4
   fingerprint of the compile-time embedded key (RFC 4880 12.2 mandates it);
   no downloaded byte is ever hashed with it. The armor CRC-24 is not checked:
   the signature is the integrity check, a CRC adds nothing over it. Key
   rotation and revocation: docs/RELEASING.md "Release key lifecycle". */

use crate::plugin_store::{self, Fetch};
use serde_json::{json, Value};
use sha2::{Digest, Sha256, Sha512};
use std::collections::BTreeMap;

pub const REPO: &str = "jpzk/opensidian";
const RELEASES_URL: &str = "https://api.github.com/repos/jpzk/opensidian/releases?per_page=30";
/// the release-signing key, ed25519, embedded at build time
pub const PINNED_KEY_ASC: &str = include_str!("../data/updates/opensidian-release.pub.asc");
/// its full v4 fingerprint; the embedded key is refused unless it hashes to this
pub const PINNED_FPR: &str = "A6E69ED6BC4779F3281721484CC1AFDE15B64EA3";
/// #294 F-84: the second pinned slot, for the offline SUCCESSOR key: (armored public
/// key, its full v4 fingerprint). EMPTY until the operator creates that key offline
/// (docs/RELEASING.md "Release key lifecycle"); no key material is ever generated
/// here. Filled, a signature by EITHER key verifies, each pinned the same strict way.
pub const SUCCESSOR: Option<(&str, &str)> = None;
/// ~/.opensidian.json key: the highest version whose release fully verified (F-58)
pub const SEEN_KEY: &str = "updateSeen";
const MAX_RELEASES_JSON: u64 = 2 * 1024 * 1024;
const MAX_SUMS: u64 = 64 * 1024;
const MAX_ASC: u64 = 16 * 1024;
/// at most this many releases have their sums pair downloaded per check
const MAX_TRIES: usize = 5;

/// "A6E6…4EA3" — how the UI names the signer
pub fn short_fpr() -> String {
    short_of(PINNED_FPR)
}

/// the short form of any full fingerprint
pub fn short_of(f: &str) -> String {
    match (f.get(..4), f.get(f.len().saturating_sub(4)..)) {
        (Some(a), Some(b)) if f.len() >= 8 => format!("{a}…{b}"),
        _ => f.to_string(),
    }
}

/* ------------------------------------------------------------ armor/base64 */

fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    let body = bytes.strip_suffix(b"==").or_else(|| bytes.strip_suffix(b"=")).unwrap_or(&bytes);
    if bytes.len() % 4 != 0 || body.contains(&b'=') {
        return Err("armor: bad base64 length/padding".into());
    }
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let (mut acc, mut n) = (0u32, 0u32);
    for &c in body {
        acc = (acc << 6) | val(c).ok_or("armor: non-base64 character")?;
        n += 6;
        if n >= 8 {
            n -= 8;
            out.push((acc >> n) as u8);
            acc &= (1 << n) - 1;
        }
    }
    Ok(out)
}

/// the binary inside one "-----BEGIN PGP <kind>-----" block
pub fn dearmor(text: &str, kind: &str) -> Result<Vec<u8>, String> {
    let begin = format!("-----BEGIN PGP {kind}-----");
    let end = format!("-----END PGP {kind}-----");
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    lines.by_ref().find(|l| l.trim() == begin).ok_or_else(|| format!("armor: no {begin} line"))?;
    // header lines ("Comment: …") up to the first blank line
    for l in lines.by_ref() {
        if l.trim().is_empty() {
            break;
        }
        if !l.contains(": ") {
            return Err("armor: malformed header".into());
        }
    }
    let mut b64 = String::new();
    for l in lines {
        let l = l.trim();
        if l == end {
            return b64_decode(&b64);
        }
        if l.starts_with('=') && l.len() == 5 {
            continue; // CRC-24 line (see the header comment)
        }
        b64.push_str(l);
    }
    Err(format!("armor: no {end} line"))
}

/* --------------------------------------------------------------- packets */

/// (tag, body) for every packet; partial / indeterminate lengths are refused
pub fn packets(mut b: &[u8]) -> Result<Vec<(u8, &[u8])>, String> {
    let mut out = Vec::new();
    while !b.is_empty() {
        let ctb = b[0];
        if ctb & 0x80 == 0 {
            return Err("packet: bad header byte".into());
        }
        let (tag, len, hl): (u8, usize, usize) = if ctb & 0x40 != 0 {
            let tag = ctb & 0x3f;
            match *b.get(1).ok_or("packet: truncated")? {
                l @ 0..=191 => (tag, l as usize, 2),
                l @ 192..=223 => {
                    let l2 = *b.get(2).ok_or("packet: truncated")? as usize;
                    (tag, ((l as usize - 192) << 8) + l2 + 192, 3)
                }
                255 => {
                    let l = b.get(2..6).ok_or("packet: truncated")?;
                    (tag, u32::from_be_bytes([l[0], l[1], l[2], l[3]]) as usize, 6)
                }
                _ => return Err("packet: partial body lengths are not accepted".into()),
            }
        } else {
            let tag = (ctb >> 2) & 0x0f;
            match ctb & 3 {
                0 => (tag, *b.get(1).ok_or("packet: truncated")? as usize, 2),
                1 => {
                    let l = b.get(1..3).ok_or("packet: truncated")?;
                    (tag, u16::from_be_bytes([l[0], l[1]]) as usize, 3)
                }
                2 => {
                    let l = b.get(1..5).ok_or("packet: truncated")?;
                    (tag, u32::from_be_bytes([l[0], l[1], l[2], l[3]]) as usize, 5)
                }
                _ => return Err("packet: indeterminate length is not accepted".into()),
            }
        };
        let body = b.get(hl..hl.checked_add(len).ok_or("packet: length overflow")?).ok_or("packet: truncated body")?;
        out.push((tag, body));
        b = &b[hl + len..];
    }
    Ok(out)
}

const ED25519_OID: &[u8] = &[0x2B, 0x06, 0x01, 0x04, 0x01, 0xDA, 0x47, 0x0F, 0x01];

pub struct PinnedKey {
    pub fpr: [u8; 20],
    pub pk: ed25519_compact::PublicKey,
}

/// the embedded key: its primary key packet must be v4 Ed25519 AND hash to PINNED_FPR
pub fn pinned_key() -> Result<PinnedKey, String> {
    load_key(PINNED_KEY_ASC, PINNED_FPR)
}

/// every key a release signature may be by: the current one, plus the successor once
/// its slot is filled. A filled slot that does not load is an error (fail closed).
pub fn trusted_keys() -> Result<Vec<PinnedKey>, String> {
    let mut keys = vec![pinned_key()?];
    if let Some((asc, fpr)) = SUCCESSOR {
        keys.push(load_key(asc, fpr).map_err(|e| format!("successor slot: {e}"))?);
    }
    Ok(keys)
}

/// an armored key whose primary key packet must be v4 Ed25519 AND hash to `want`
pub fn load_key(asc: &str, want: &str) -> Result<PinnedKey, String> {
    let bin = dearmor(asc, "PUBLIC KEY BLOCK")?;
    let pk = packets(&bin)?;
    let body = match pk.first() {
        Some((6, b)) => *b,
        _ => return Err("pinned key: no primary public-key packet".into()),
    };
    let fpr = v4_fingerprint(body);
    if want.len() != 40 || hex(&fpr) != want {
        return Err(format!("pinned key: fingerprint {} is not the pinned {want}", hex(&fpr)));
    }
    // v4 | created(4) | algo 22 | oid len | oid | MPI(263 bits) = 0x40 || key(32)
    if body.len() != 6 + 1 + ED25519_OID.len() + 2 + 33
        || body[0] != 4
        || body[5] != 22
        || body[6] as usize != ED25519_OID.len()
        || &body[7..16] != ED25519_OID
        || body[16..18] != [0x01, 0x07]
        || body[18] != 0x40
    {
        return Err("pinned key: not a v4 Ed25519 key".into());
    }
    let pk = ed25519_compact::PublicKey::from_slice(&body[19..51]).map_err(|e| format!("pinned key: {e:?}"))?;
    Ok(PinnedKey { fpr, pk })
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

/// RFC 4880 12.2: SHA-1(0x99 || u16 len || key packet body)
fn v4_fingerprint(body: &[u8]) -> [u8; 20] {
    let mut m = vec![0x99, (body.len() >> 8) as u8, body.len() as u8];
    m.extend_from_slice(body);
    sha1(&m)
}

/// FIPS 180-4 SHA-1, used ONLY on the compile-time embedded key (see header)
fn sha1(msg: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut m = msg.to_vec();
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&((msg.len() as u64) * 8).to_be_bytes());
    for chunk in m.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// one v4 signature packet body checked over `data` with `key`
pub fn verify_packet(key: &PinnedKey, data: &[u8], sig: &[u8]) -> Result<(), String> {
    let g = |r: std::ops::Range<usize>| sig.get(r).ok_or_else(|| "signature: truncated".to_string());
    let head = g(0..6)?;
    let (ver, typ, palgo, halgo) = (head[0], head[1], head[2], head[3]);
    if ver != 4 {
        return Err(format!("signature: version {ver}, only v4 is accepted"));
    }
    if typ != 0x00 {
        return Err(format!("signature: type 0x{typ:02x}, only a binary-document signature is accepted"));
    }
    if palgo != 22 {
        return Err(format!("signature: public-key algorithm {palgo}, only EdDSA (22) is accepted"));
    }
    let hlen = u16::from_be_bytes([head[4], head[5]]) as usize;
    let hashed = g(6..6 + hlen)?;
    let ul = g(6 + hlen..8 + hlen)?;
    let ulen = u16::from_be_bytes([ul[0], ul[1]]) as usize;
    let mut p = 8 + hlen + ulen;
    let _left16 = g(p..p + 2)?;
    p += 2;
    // two MPIs, R then S, each at most 256 bits, left-padded to 32 bytes
    let mut rs = [0u8; 64];
    for half in 0..2 {
        let bl = g(p..p + 2)?;
        let bits = u16::from_be_bytes([bl[0], bl[1]]) as usize;
        let n = bits.div_ceil(8);
        if n > 32 {
            return Err("signature: EdDSA value over 256 bits".into());
        }
        let v = g(p + 2..p + 2 + n)?;
        rs[half * 32 + 32 - n..half * 32 + 32].copy_from_slice(v);
        p += 2 + n;
    }
    if p != sig.len() {
        return Err("signature: trailing bytes".into());
    }
    // the hashed subpackets: issuer fingerprint REQUIRED and pinned, creation time
    // REQUIRED (#294 F-84: exactly one, 4 bytes); unknown critical = refuse
    let mut issuer_ok = false;
    let mut created: Option<u32> = None;
    let mut s = hashed;
    while !s.is_empty() {
        let (len, hl) = match s[0] {
            l @ 0..=191 => (l as usize, 1),
            l @ 192..=254 => (((l as usize - 192) << 8) + *s.get(1).ok_or("signature: bad subpacket")? as usize + 192, 2),
            255 => {
                let l = s.get(1..5).ok_or("signature: bad subpacket")?;
                (u32::from_be_bytes([l[0], l[1], l[2], l[3]]) as usize, 5)
            }
        };
        let sp = s.get(hl..hl + len).filter(|x| !x.is_empty()).ok_or("signature: bad subpacket")?;
        let (critical, t, d) = (sp[0] & 0x80 != 0, sp[0] & 0x7f, &sp[1..]);
        match t {
            33 => {
                if d.len() != 21 || d[0] != 4 || d[1..] != key.fpr {
                    return Err(format!("signed by another key ({}), not the pinned {}", hex(d.get(1..).unwrap_or(&[])), short_fpr()));
                }
                issuer_ok = true;
            }
            16 if d.len() == 8 && d != &key.fpr[12..] => return Err("signed by another key id, not the pinned key".into()),
            2 => {
                if d.len() != 4 || created.is_some() {
                    return Err("signature: malformed or repeated creation time".into());
                }
                created = Some(u32::from_be_bytes([d[0], d[1], d[2], d[3]]));
            }
            16 => {}
            3 => return Err("signature: carries an expiration time, not accepted".into()),
            _ if critical => return Err(format!("signature: unknown critical subpacket {t}")),
            _ => {}
        }
        s = &s[hl + len..];
    }
    if !issuer_ok {
        return Err("signature: no issuer fingerprint, cannot pin the signer".into());
    }
    if created.is_none() {
        return Err("signature: no creation time subpacket, refused".into());
    }
    // RFC 4880 5.2.4: data || hashed part || 0x04 0xFF || be32(len(hashed part))
    let part = &sig[..6 + hlen];
    let mut trailer = part.to_vec();
    trailer.extend_from_slice(&[0x04, 0xFF]);
    trailer.extend_from_slice(&(part.len() as u32).to_be_bytes());
    let digest: Vec<u8> = match halgo {
        8 => Sha256::new().chain_update(data).chain_update(&trailer).finalize().to_vec(),
        10 => Sha512::new().chain_update(data).chain_update(&trailer).finalize().to_vec(),
        h => return Err(format!("signature: hash algorithm {h}, only SHA-256/SHA-512 are accepted")),
    };
    let sig = ed25519_compact::Signature::from_slice(&rs).map_err(|e| format!("signature: {e:?}"))?;
    key.pk.verify(&digest, &sig).map_err(|_| "signature does not verify against the pinned key".to_string())
}

/// a detached armored signature over `data`, by a trusted key; Ok = the signer's
/// full fingerprint (hex)
pub fn verify_detached(data: &[u8], asc: &[u8]) -> Result<String, String> {
    verify_detached_with(&trusted_keys()?, data, asc).map(|f| hex(&f))
}

/// the same against an explicit key set (the successor-slot tests pass a TEST key)
pub fn verify_detached_with(keys: &[PinnedKey], data: &[u8], asc: &[u8]) -> Result<[u8; 20], String> {
    let text = std::str::from_utf8(asc).map_err(|_| "signature: not ASCII armor")?;
    let bin = dearmor(text, "SIGNATURE")?;
    let pk = packets(&bin)?;
    if pk.is_empty() || pk.iter().any(|(t, _)| *t != 2) {
        return Err("signature: the .asc must hold signature packets only".into());
    }
    if keys.is_empty() {
        return Err("signature: no trusted key".into());
    }
    // the reported reason: the one from a key the signature names, else "another key"
    let mut last = String::new();
    for (_, body) in pk {
        for key in keys {
            match verify_packet(key, data, body) {
                Ok(()) => return Ok(key.fpr),
                Err(e) => {
                    if last.is_empty() || !e.starts_with("signed by another key") {
                        last = e;
                    }
                }
            }
        }
    }
    Err(last)
}

/// the signed SHA256SUMS as file name -> lowercase sha256; every line must name a
/// file of THIS version, no name twice, at least one line
pub fn parse_sums(sums: &[u8], ver: &str) -> Result<BTreeMap<String, String>, String> {
    let t = std::str::from_utf8(sums).map_err(|_| "SHA256SUMS: not text")?;
    let want = format!("opensidian-{ver}-");
    let mut map = BTreeMap::new();
    for l in t.lines().filter(|l| !l.trim().is_empty()) {
        let (h, name) = l.split_once("  ").ok_or("SHA256SUMS: malformed line")?;
        if h.len() != 64 || !h.bytes().all(|c| c.is_ascii_hexdigit()) || !name.starts_with(&want) || name.contains('/') {
            return Err(format!("SHA256SUMS: line {l:?} is not a {ver} release file"));
        }
        if map.insert(name.to_string(), h.to_ascii_lowercase()).is_some() {
            return Err(format!("SHA256SUMS: {name} is listed twice"));
        }
    }
    if map.is_empty() {
        return Err("SHA256SUMS: empty".into());
    }
    Ok(map)
}

/// every line of the signed sums names a file of THIS version
pub fn sums_cover_version(sums: &[u8], ver: &str) -> Result<(), String> {
    parse_sums(sums, ver).map(|_| ())
}

/// #294 F-11: every downloadable asset GitHub lists is in the signed sums, and its
/// GitHub sha256 digest is the signed value. The sums pair itself is not a download.
pub fn assets_match(assets: &[Asset], signed: &BTreeMap<String, String>) -> Result<(), String> {
    let mut n = 0;
    for a in assets.iter().filter(|a| a.name != "SHA256SUMS" && a.name != "SHA256SUMS.asc") {
        let want = signed.get(&a.name).ok_or_else(|| format!("asset {:?} is not listed in the signed SHA256SUMS", a.name))?;
        let got = a.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")).map(str::to_ascii_lowercase);
        match got {
            Some(g) if g == *want => n += 1,
            Some(_) => return Err(format!("asset {:?}: GitHub's sha256 differs from the signed SHA256SUMS", a.name)),
            None => return Err(format!("asset {:?}: GitHub reports no sha256 digest", a.name)),
        }
    }
    if n == 0 {
        return Err("release has no downloadable asset".into());
    }
    Ok(())
}

/* ------------------------------------------------------------- platform */

/// what the running build is, for picking ITS release file out of the signed sums
#[derive(Clone, Debug, Default)]
pub struct Host {
    pub os: String,
    pub arch: String,
    pub flatpak: bool,
    /// $APPIMAGE (the runtime sets it to the image path)
    pub appimage: Option<String>,
    pub debian: bool,
    pub rpm: bool,
}

/// this process. Only picks which signed hash is SHOWN; never part of the verdict.
pub fn host() -> Host {
    let p = |f: &str| std::path::Path::new(f).exists();
    Host {
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        flatpak: std::env::var_os("FLATPAK_ID").is_some() || p("/.flatpak-info"),
        appimage: std::env::var("APPIMAGE").ok().filter(|s| !s.is_empty()),
        debian: p("/etc/debian_version"),
        rpm: p("/etc/redhat-release") || p("/etc/fedora-release") || p("/etc/SuSE-release"),
    }
}

/// the release file names for this host, best match first
pub fn platform_names(ver: &str, h: &Host) -> Vec<String> {
    let a = h.arch.as_str();
    let suf: Vec<String> = match h.os.as_str() {
        "linux" => {
            let mut v = vec![];
            if h.flatpak {
                v.push(format!("{a}.flatpak"));
            }
            if let Some(img) = &h.appimage {
                let kind = if img.contains("portable") { "portable" } else { "slim" };
                v.push(format!("{a}-{kind}.AppImage"));
            }
            if v.is_empty() && h.debian {
                v.push(format!("{a}.deb"));
            }
            if v.is_empty() && h.rpm {
                v.push(format!("{a}.rpm"));
            }
            if v.is_empty() {
                v.push(format!("{a}-slim.AppImage"));
            }
            v
        }
        "macos" => vec!["universal.dmg".into(), format!("macos-{a}-unsigned.app.zip")],
        _ => vec![],
    };
    suf.into_iter().map(|s| format!("opensidian-{ver}-{s}")).collect()
}

/// (file, signed sha256) of this host's release file, if the signed sums list one
pub fn platform_artefact(ver: &str, h: &Host, signed: &BTreeMap<String, String>) -> Option<(String, String)> {
    platform_names(ver, h).into_iter().find_map(|n| signed.get(&n).map(|s| (n.clone(), s.clone())))
}

/* -------------------------------------------------------------- releases */

pub struct Candidate {
    pub tag: String,
    pub version: String,
    pub sums: Option<String>,
    pub asc: Option<String>,
}

/// one asset GitHub lists: its name and GitHub's "sha256:<hex>" digest
pub struct Asset {
    pub name: String,
    pub digest: Option<String>,
}

pub struct Release {
    pub c: Candidate,
    pub assets: Vec<Asset>,
}

/// "v0.6.2" -> "0.6.2"; only plain numeric dotted versions (1-3 parts) count
pub fn tag_version(tag: &str) -> Option<String> {
    let v = tag.strip_prefix('v').unwrap_or(tag);
    let parts: Vec<&str> = v.split('.').collect();
    (!parts.is_empty() && parts.len() <= 3 && parts.iter().all(|p| !p.is_empty() && p.len() <= 6 && p.bytes().all(|c| c.is_ascii_digit())))
        .then(|| v.to_string())
}

/// every published, non-draft, non-prerelease numeric release, highest first
/// (equal versions keep the list's order)
pub fn releases(b: &[u8]) -> Result<Vec<Release>, String> {
    let v: Value = serde_json::from_slice(b).map_err(|e| format!("releases: not JSON ({e})"))?;
    let mut out: Vec<Release> = Vec::new();
    for r in v.as_array().ok_or("releases: not a list")? {
        if r["draft"].as_bool() != Some(false) || r["prerelease"].as_bool() != Some(false) {
            continue;
        }
        if r["published_at"].as_str().map_or(true, str::is_empty) {
            continue;
        }
        let tag = r["tag_name"].as_str().unwrap_or("");
        let Some(version) = tag_version(tag) else { continue };
        let list: Vec<&Value> = r["assets"].as_array().into_iter().flatten().collect();
        let asset = |n: &str| {
            list.iter().find(|a| a["name"].as_str() == Some(n)).and_then(|a| a["browser_download_url"].as_str()).map(str::to_string)
        };
        let c = Candidate { tag: tag.to_string(), version, sums: asset("SHA256SUMS"), asc: asset("SHA256SUMS.asc") };
        let assets = list
            .iter()
            .map(|a| Asset { name: a["name"].as_str().unwrap_or("").to_string(), digest: a["digest"].as_str().map(str::to_string) })
            .collect();
        out.push(Release { c, assets });
    }
    out.sort_by(|a, b| {
        use std::cmp::Ordering::*;
        if plugin_store::version_gt(&b.c.version, &a.c.version) {
            Greater
        } else if plugin_store::version_gt(&a.c.version, &b.c.version) {
            Less
        } else {
            Equal
        }
    });
    Ok(out)
}

/// the highest published, non-draft, non-prerelease release in the list
pub fn pick_latest(b: &[u8]) -> Result<Option<Candidate>, String> {
    Ok(releases(b)?.into_iter().next().map(|r| r.c))
}

/// a release whose SHA256SUMS signature verified
pub struct Signed {
    /// the signer's full fingerprint
    pub fpr: String,
    /// the signed sums, name -> sha256
    pub map: BTreeMap<String, String>,
}

/// the signature status of one release; Err = never offered. A download that
/// failed is Fail::Fetch, not "unsigned": nothing is known about that release.
fn signed(fetch: Fetch, c: &Candidate) -> Result<Signed, Fail> {
    let (Some(su), Some(au)) = (c.sums.as_deref(), c.asc.as_deref()) else {
        return Err(Fail::Unsigned("release has no SHA256SUMS / SHA256SUMS.asc".into()));
    };
    let sums = fetch(su, MAX_SUMS, "SHA256SUMS").map_err(Fail::Fetch)?;
    let asc = fetch(au, MAX_ASC, "SHA256SUMS.asc").map_err(Fail::Fetch)?;
    let fpr = verify_detached(&sums, &asc).map_err(Fail::Unsigned)?;
    Ok(Signed { fpr, map: parse_sums(&sums, &c.version).map_err(Fail::Unsigned)? })
}

/// why a release is not offered
#[derive(Debug)]
pub enum Fail {
    /// no / bad signature over SHA256SUMS, or sums not of this version
    Unsigned(String),
    /// signed sums, but the release's assets do not match them (F-11)
    Mismatch(String),
    /// the sums pair could not be downloaded (network, cap): the release is not
    /// judged at all, and the whole check ends as an error (#300 review)
    Fetch(String),
}

impl Fail {
    fn kind(&self) -> &'static str {
        match self {
            Fail::Unsigned(_) => "unsigned",
            Fail::Mismatch(_) => "mismatch",
            Fail::Fetch(_) => "error",
        }
    }
    fn reason(&self) -> &str {
        match self {
            Fail::Unsigned(e) | Fail::Mismatch(e) | Fail::Fetch(e) => e,
        }
    }
}

/// signature + version + every asset; Ok = this release may be offered
pub fn verify_release(fetch: Fetch, r: &Release) -> Result<Signed, Fail> {
    let s = signed(fetch, &r.c)?;
    assets_match(&r.assets, &s.map).map_err(Fail::Mismatch)?;
    Ok(s)
}

pub fn release_page(tag: &str) -> String {
    format!("https://github.com/{REPO}/releases/tag/{tag}")
}

/// what one check runs with besides the network
pub struct Ctx<'a> {
    /// the language the verdict text renders in (i18n.rs)
    pub lang: &'a str,
    /// SEEN_KEY: the highest version that fully verified in an earlier check
    pub seen: Option<String>,
    pub host: Host,
}

/// the whole check (English, nothing remembered, this host); Err only for "could not ask GitHub at all"
pub fn check(fetch: Fetch, current: &str) -> Result<Value, String> {
    check_with(fetch, current, &Ctx { lang: "en", seen: None, host: host() })
}

/// the whole check. Result "status": available | current | unsigned | mismatch |
/// rollback; "url" ONLY with available; "seen" = the SEEN_KEY value to remember.
pub fn check_with(fetch: Fetch, current: &str, ctx: &Ctx) -> Result<Value, String> {
    let t = |id: &str, args: &[(&str, &str)]| crate::i18n::tr(ctx.lang, id, args);
    let list = fetch(RELEASES_URL, MAX_RELEASES_JSON, "releases")?;
    let rels = releases(&list)?;
    if rels.is_empty() {
        // #300 review: a remembered newer signed release that the list no longer
        // shows at all is the frozen/emptied-list case of rollback, never "up to date"
        if let Some(sn) = ctx.seen.as_deref().filter(|sn| plugin_store::version_gt(sn, current)) {
            return Ok(json!({"status": "rollback", "current": current, "seen": sn, "text": t("update-rollback-none", &[("version", sn)]),
                             "warnings": Vec::<String>::new(), "failed": Vec::<Value>::new()}));
        }
        return Ok(json!({"status": "current", "current": current, "seen": ctx.seen, "text": t("update-none", &[("v", current)])}));
    }
    let mut fails: Vec<(&Release, Fail)> = Vec::new();
    let mut best: Option<(&Release, Signed)> = None;
    let mut tries = 0;
    for r in &rels {
        let fetches = r.c.sums.is_some() && r.c.asc.is_some();
        if fetches && tries >= MAX_TRIES {
            break;
        }
        tries += fetches as usize;
        match verify_release(fetch, r) {
            Ok(s) => {
                best = Some((r, s));
                break;
            }
            // #300 review: a failed download says nothing about the signature;
            // never fall through to an older release with an "unsigned" warning
            Err(Fail::Fetch(e)) => return Err(format!("{}: {e}", r.c.tag)),
            Err(f) => {
                eprintln!("[updates] {} rejected ({}): {}", r.c.tag, f.kind(), f.reason());
                fails.push((r, f));
            }
        }
    }
    // every failed release is above the offered one (highest first)
    let failed: Vec<Value> = fails.iter().map(|(r, f)| json!({"version": r.c.version, "kind": f.kind(), "reason": f.reason()})).collect();
    let warn = || -> Vec<String> {
        fails
            .iter()
            .map(|(r, f)| match f {
                Fail::Unsigned(_) | Fail::Fetch(_) => t("update-warn-unsigned", &[("version", &r.c.version)]),
                Fail::Mismatch(_) => t("update-warn-mismatch", &[("version", &r.c.version)]),
            })
            .collect()
    };
    let Some((r, s)) = best else {
        let (top, f) = &fails[0];
        let id = match f {
            Fail::Unsigned(_) | Fail::Fetch(_) => "update-unsigned",
            Fail::Mismatch(_) => "update-mismatch",
        };
        return Ok(json!({"status": f.kind(), "current": current, "latest": top.c.version, "reason": f.reason(),
                         "text": t(id, &[("version", &top.c.version)]), "warnings": Vec::<String>::new(), "failed": failed, "seen": ctx.seen}));
    };
    let v = r.c.version.as_str();
    let signer = short_of(&s.fpr);
    let seen = match ctx.seen.as_deref() {
        Some(sn) if !plugin_store::version_gt(v, sn) => sn.to_string(),
        _ => v.to_string(),
    };
    // #300 review: the hash shown is that of a file the release link actually
    // offers. A platform file that is signed but not attached (deleted after
    // signing) gets no hash, only a warning naming it.
    let art = platform_artefact(v, &ctx.host, &s.map);
    let (art, unattached) = match art {
        Some((f, _)) if !r.assets.iter().any(|a| a.name == f) => {
            let attached: BTreeMap<String, String> = s.map.iter().filter(|(n, _)| r.assets.iter().any(|a| &a.name == *n)).map(|(n, h)| (n.clone(), h.clone())).collect();
            (platform_artefact(v, &ctx.host, &attached), Some(f))
        }
        a => (a, None),
    };
    let mut warnings = warn();
    let (file, sha) = match &art {
        Some((f, h)) => (json!(f), json!(h)),
        None => (Value::Null, Value::Null),
    };
    let mut out = json!({"current": current, "latest": v, "signer": signer, "fingerprint": s.fpr,
                         "file": file, "sha256": sha, "failed": failed, "seen": seen});
    if plugin_store::version_gt(&seen, v) && plugin_store::version_gt(&seen, current) {
        // never downgrade the offer: a newer release verified before and is gone now
        out["status"] = json!("rollback");
        out["text"] = json!(t("update-rollback", &[("version", &seen), ("v", v)]));
    } else if plugin_store::version_gt(v, current) {
        match (&art, &unattached) {
            (None, Some(f)) => warnings.push(t("update-artefact-missing", &[("file", f)])),
            (None, None) => warnings.push(t("update-no-artefact", &[])),
            _ => {}
        }
        out["status"] = json!("available");
        out["url"] = json!(release_page(&r.c.tag));
        out["text"] = json!(t("update-available", &[("version", v), ("key", &signer)]));
    } else {
        out["status"] = json!("current");
        out["text"] = json!(t("update-current", &[("v", current), ("version", v), ("key", &signer)]));
    }
    out["warnings"] = json!(warnings);
    Ok(out)
}

/// where SEEN_KEY lives: ~/.opensidian.json through cfgstore in the app
pub trait SeenStore {
    /// the stored value, read fresh
    fn get(&self) -> Option<String>;
    /// raise it (cfgstore Op::UpdateSeen: monotonic, a lower value is ignored)
    fn raise(&self, ver: &str);
}

struct CfgSeen;

impl SeenStore for CfgSeen {
    fn get(&self) -> Option<String> {
        crate::cfg_value()[SEEN_KEY].as_str().and_then(tag_version)
    }
    fn raise(&self, ver: &str) {
        crate::cfg_update(&[crate::cfgstore::Op::UpdateSeen(ver.to_string())]);
    }
}

/// one check plus remembering its result. #300 review: the stored value is read
/// AGAIN after this check raised it. Another window's check may have stored a
/// higher verified version while this one was on the network; this result then
/// must not offer (or call "current") anything below that version.
pub fn check_remembered(fetch: Fetch, current: &str, lang: &str, h: Host, store: &dyn SeenStore) -> Result<Value, String> {
    let ctx = Ctx { lang, seen: store.get(), host: h };
    let mut r = check_with(fetch, current, &ctx)?;
    if let Some(sn) = r["seen"].as_str().filter(|sn| ctx.seen.as_deref() != Some(*sn)) {
        store.raise(sn);
    }
    if let Some(now) = store.get() {
        hold_back(&mut r, &now, current, lang);
    }
    Ok(r)
}

/// an "available" / "current" verdict for a release below `seen` becomes
/// "rollback", exactly as check_with decides it when `seen` is known up front
fn hold_back(r: &mut Value, seen: &str, current: &str, lang: &str) {
    let st = r["status"].as_str().unwrap_or("");
    let Some(latest) = r["latest"].as_str().map(String::from) else { return };
    if (st == "available" || st == "current") && plugin_store::version_gt(seen, &latest) && plugin_store::version_gt(seen, current) {
        r["status"] = json!("rollback");
        r["text"] = json!(crate::i18n::tr(lang, "update-rollback", &[("version", seen), ("v", &latest)]));
        r["seen"] = json!(seen);
        if let Some(o) = r.as_object_mut() {
            o.remove("url");
        }
    }
}

#[tauri::command]
pub async fn update_check() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| check_remembered(&plugin_store::http_get, env!("CARGO_PKG_VERSION"), crate::ui_lang(), host(), &CfgSeen))
        .await
        .map_err(|e| format!("update check: {e}"))?
}

/// cfgstore::Op::UpdateSeen: raise SEEN_KEY to `ver`, never lower it; a value that
/// is not a plain numeric version is refused
pub fn apply_seen(v: &mut Value, ver: &str) {
    if tag_version(ver).as_deref() != Some(ver) || ver.starts_with('v') {
        eprintln!("[cfg] refused {SEEN_KEY} {ver:?}");
        return;
    }
    let cur = v[SEEN_KEY].as_str().and_then(tag_version);
    if cur.as_deref().map_or(true, |c| plugin_store::version_gt(ver, c)) {
        v[SEEN_KEY] = json!(ver);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUMS: &[u8] = include_bytes!("../data/updates/v0.6.1-SHA256SUMS");
    const ASC: &[u8] = include_bytes!("../data/updates/v0.6.1-SHA256SUMS.asc");
    const WRONG: &[u8] = include_bytes!("../data/updates/v0.6.1-SHA256SUMS.wrongkey.asc");

    #[test]
    fn sha1_vectors_and_the_pinned_fingerprint() {
        assert_eq!(hex(&sha1(b"abc")), "A9993E364706816ABA3E25717850C26C9CD0D89D");
        assert_eq!(hex(&sha1(b"")), "DA39A3EE5E6B4B0D3255BFEF95601890AFD80709");
        assert_eq!(hex(&sha1(&[b'a'; 1000])), "291E9A6C66994949B57BA5E650361E98FC36B1BA");
        let k = pinned_key().expect("embedded key");
        assert_eq!(hex(&k.fpr), PINNED_FPR);
        assert_eq!(short_fpr(), "A6E6…4EA3");
    }

    #[test]
    fn valid_release_signature_verifies() {
        verify_detached(SUMS, ASC).expect("v0.6.1 SHA256SUMS.asc is a good signature by the pinned key");
        sums_cover_version(SUMS, "0.6.1").unwrap();
        assert!(sums_cover_version(SUMS, "9.9.9").is_err(), "old sums replayed under a newer tag");
    }

    #[test]
    fn tampered_sums_rejected() {
        let mut t = SUMS.to_vec();
        t[0] ^= 1;
        assert!(verify_detached(&t, ASC).unwrap_err().contains("does not verify"));
        let mut t = SUMS.to_vec();
        t.extend_from_slice(b"00  evil\n");
        assert!(verify_detached(&t, ASC).is_err());
    }

    #[test]
    fn tampered_signature_rejected() {
        let bin = dearmor(std::str::from_utf8(ASC).unwrap(), "SIGNATURE").unwrap();
        let (_, body) = packets(&bin).unwrap()[0];
        let mut b = body.to_vec();
        let n = b.len();
        b[n - 1] ^= 0x40; // a bit of S
        assert!(verify_packet(&pinned_key().unwrap(), SUMS, &b).is_err());
    }

    #[test]
    fn wrong_key_rejected() {
        assert!(verify_detached(SUMS, WRONG).unwrap_err().contains("another key"));
        // the wrong key's signature with its issuer rewritten to the pinned fingerprint:
        // the issuer check passes, the curve check must still refuse it
        let bin = dearmor(std::str::from_utf8(WRONG).unwrap(), "SIGNATURE").unwrap();
        let (_, body) = packets(&bin).unwrap()[0];
        let mut b = body.to_vec();
        let at = b.windows(3).position(|w| w == [22, 33, 4]).expect("issuer fpr subpacket") + 3;
        b[at..at + 20].copy_from_slice(&pinned_key().unwrap().fpr);
        assert!(verify_packet(&pinned_key().unwrap(), SUMS, &b).unwrap_err().contains("does not verify"));
    }

    #[test]
    fn missing_or_garbage_signature_rejected() {
        assert!(verify_detached(SUMS, b"").is_err());
        assert!(verify_detached(SUMS, b"-----BEGIN PGP SIGNATURE-----\n\n-----END PGP SIGNATURE-----\n").is_err());
        assert!(verify_detached(SUMS, PINNED_KEY_ASC.as_bytes()).is_err(), "a public key is not a signature");
        let c = Candidate { tag: "v0.9.0".into(), version: "0.9.0".into(), sums: Some("https://github.com/x/SHA256SUMS".into()), asc: None };
        let f = |_: &str, _: u64, _: &str| -> Result<Vec<u8>, String> { panic!("nothing is fetched without both files") };
        assert!(signed(&f, &c).is_err());
    }

    fn rel(tag: &str, draft: bool, pre: bool, sums: bool, asc: bool) -> Value {
        let mut assets = vec![json!({"name": format!("opensidian-{}-x86_64.deb", tag.trim_start_matches('v')), "browser_download_url": "https://github.com/x/deb"})];
        // #294: the deb carries GitHub's digest; for v0.6.1 it is the signed value
        if let Some(d) = signed_digest(assets[0]["name"].as_str().unwrap()) {
            assets[0]["digest"] = json!(d);
        }
        if sums {
            assets.push(json!({"name": "SHA256SUMS", "browser_download_url": format!("https://github.com/jpzk/opensidian/releases/download/{tag}/SHA256SUMS")}));
        }
        if asc {
            assets.push(json!({"name": "SHA256SUMS.asc", "browser_download_url": format!("https://github.com/jpzk/opensidian/releases/download/{tag}/SHA256SUMS.asc")}));
        }
        json!({"tag_name": tag, "draft": draft, "prerelease": pre, "published_at": "2026-10-09T00:00:00Z", "assets": assets})
    }

    #[test]
    fn picks_highest_published_stable_numeric_tag() {
        let l = json!([rel("v0.4", false, false, true, true), rel("v0.6.1", false, false, true, true), rel("v0.9.0", true, false, true, true),
                       rel("v0.8.0", false, true, true, true), rel("v1.0.0-rc1", false, false, true, true), rel("v0.6.2", false, false, true, true)]);
        let c = pick_latest(l.to_string().as_bytes()).unwrap().unwrap();
        assert_eq!((c.tag.as_str(), c.version.as_str()), ("v0.6.2", "0.6.2"));
        assert_eq!(tag_version("v0.4").as_deref(), Some("0.4"));
        assert_eq!(tag_version("v1.2.3.4"), None);
        assert_eq!(tag_version("../x"), None);
    }

    /// the whole check against an offline GitHub: v0.6.1's real files as the newest release
    fn gh(list: Value, asc: &'static [u8], sums: &'static [u8]) -> impl Fn(&str, u64, &str) -> Result<Vec<u8>, String> {
        move |u: &str, cap: u64, _: &str| {
            plugin_store::check_url(u)?;
            let b: Vec<u8> = if u == RELEASES_URL {
                list.to_string().into_bytes()
            } else if u.ends_with("/SHA256SUMS") {
                sums.to_vec()
            } else if u.ends_with("/SHA256SUMS.asc") {
                asc.to_vec()
            } else {
                return Err(format!("unexpected fetch {u}"));
            };
            assert!(b.len() as u64 <= cap);
            Ok(b)
        }
    }

    #[test]
    fn check_reports_available_current_and_unsigned() {
        let l = json!([rel("v0.6.1", false, false, true, true), rel("v0.4", false, false, true, true)]);
        let r = check(&gh(l.clone(), ASC, SUMS), "0.6.0").unwrap();
        assert_eq!(r["status"], "available");
        // #294: the verdict names what is signed (the checksums) and asks the user to verify
        assert_eq!(r["text"], "Opensidian 0.6.1 is available — checksums signed by A6E6…4EA3, verify your download against this hash.");
        assert_eq!(r["url"], "https://github.com/jpzk/opensidian/releases/tag/v0.6.1");
        let r = check(&gh(l.clone(), ASC, SUMS), "0.7.0").unwrap();
        assert_eq!(r["status"], "current");
        assert!(r["text"].as_str().unwrap().starts_with("Opensidian is up to date (0.7.0)"));
        for (asc, sums) in [(WRONG, SUMS), (ASC, b"00  opensidian-0.6.1-x\n" as &[u8])] {
            let r = check(&gh(l.clone(), asc, sums), "0.6.0").unwrap();
            assert_eq!(r["status"], "unsigned", "{r}");
            assert_eq!(r["text"], "Latest release 0.6.1 is NOT signed (or signature invalid) — ignored.");
            assert!(r.get("url").is_none(), "an unsigned release is never offered");
        }
        let nosig = json!([rel("v0.6.1", false, false, true, false)]);
        assert_eq!(check(&gh(nosig, ASC, SUMS), "0.6.0").unwrap()["status"], "unsigned");
        let down = |_: &str, _: u64, _: &str| -> Result<Vec<u8>, String> { Err("releases: api.github.com — no route".into()) };
        assert!(check(&down, "0.7.0").unwrap_err().contains("no route"));
    }

    /* ---------------------------------------------------------- #294 tests */

    /// the signed sha256 of one v0.6.1 file, as GitHub's API spells a digest
    fn signed_digest(name: &str) -> Option<String> {
        std::str::from_utf8(SUMS).unwrap().lines().find_map(|l| l.split_once("  ").filter(|(_, n)| *n == name).map(|(h, _)| format!("sha256:{h}")))
    }

    const DEB: &str = "9a76781b7f48398bb70550308634510a2079e2ac957a228cf5477959877c118f";

    /// v0.6.1 as GitHub lists it: all five signed files with their digests + the sums pair
    fn genuine(tag: &str) -> Value {
        let mut assets: Vec<Value> = std::str::from_utf8(SUMS)
            .unwrap()
            .lines()
            .map(|l| {
                let (h, n) = l.split_once("  ").unwrap();
                json!({"name": n, "digest": format!("sha256:{h}"), "browser_download_url": format!("https://github.com/jpzk/opensidian/releases/download/{tag}/{n}")})
            })
            .collect();
        for n in ["SHA256SUMS", "SHA256SUMS.asc"] {
            assets.push(json!({"name": n, "browser_download_url": format!("https://github.com/jpzk/opensidian/releases/download/{tag}/{n}")}));
        }
        json!({"tag_name": tag, "draft": false, "prerelease": false, "published_at": "2026-10-09T00:00:00Z", "assets": assets})
    }

    fn deb_host() -> Host {
        Host { os: "linux".into(), arch: "x86_64".into(), debian: true, ..Host::default() }
    }

    fn ctx(seen: Option<&str>) -> Ctx<'static> {
        Ctx { lang: "en", seen: seen.map(String::from), host: deb_host() }
    }

    /// gh() that also records every URL fetched: the check never downloads a release file
    fn recorded(list: Value) -> (impl Fn(&str, u64, &str) -> Result<Vec<u8>, String>, std::rc::Rc<std::cell::RefCell<Vec<String>>>) {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let (l2, inner) = (log.clone(), gh(list, ASC, SUMS));
        (move |u: &str, cap: u64, w: &str| { l2.borrow_mut().push(u.to_string()); inner(u, cap, w) }, log)
    }

    #[test]
    fn genuine_release_passes_and_shows_the_signed_platform_hash() {
        let (f, log) = recorded(json!([genuine("v0.6.1")]));
        let r = check_with(&f, "0.6.0", &ctx(None)).unwrap();
        assert_eq!(r["status"], "available", "{r}");
        assert_eq!(r["text"], "Opensidian 0.6.1 is available — checksums signed by A6E6…4EA3, verify your download against this hash.");
        assert_eq!(r["file"], "opensidian-0.6.1-x86_64.deb");
        assert_eq!(r["sha256"], DEB);
        assert_eq!(r["fingerprint"], PINNED_FPR);
        assert_eq!(r["url"], "https://github.com/jpzk/opensidian/releases/tag/v0.6.1");
        assert_eq!(r["warnings"], json!([]));
        assert_eq!(r["seen"], "0.6.1");
        // nothing but the list and the sums pair is ever fetched
        for u in log.borrow().iter() {
            assert!(u == RELEASES_URL || u.ends_with("/SHA256SUMS") || u.ends_with("/SHA256SUMS.asc"), "fetched {u}");
        }
        assert_eq!(log.borrow().len(), 3);
        // the digest GitHub reports may be upper case; the value is what counts
        let mut g = genuine("v0.6.1");
        g["assets"][0]["digest"] = json!(g["assets"][0]["digest"].as_str().unwrap().to_ascii_uppercase().replace("SHA256:", "sha256:"));
        assert_eq!(check_with(&gh(json!([g]), ASC, SUMS), "0.6.0", &ctx(None)).unwrap()["status"], "available");
    }

    fn assert_mismatch(r: &Value) {
        assert_eq!(r["status"], "mismatch", "{r}");
        assert_eq!(r["text"], "Latest release 0.6.1: release assets do not match the signed checksums — ignored.");
        assert!(r.get("url").is_none(), "a mismatching release is never offered");
        assert!(r.get("sha256").is_none() && r.get("signer").is_none(), "no signed verdict: {r}");
    }

    #[test]
    fn unlisted_extra_asset_is_not_available() {
        for extra in ["opensidian-0.6.1-x86_64-extra.AppImage", "install.sh", "SHA256SUMS.sig", ""] {
            let mut g = genuine("v0.6.1");
            g["assets"].as_array_mut().unwrap().push(json!({"name": extra, "digest": format!("sha256:{DEB}"), "browser_download_url": "https://github.com/x/e"}));
            let r = check_with(&gh(json!([g]), ASC, SUMS), "0.6.0", &ctx(None)).unwrap();
            assert_mismatch(&r);
            assert!(r["reason"].as_str().unwrap().contains("not listed"), "{r}");
        }
        // a release with nothing to download offers nothing either
        let mut g = genuine("v0.6.1");
        g["assets"].as_array_mut().unwrap().retain(|a| a["name"].as_str().unwrap().starts_with("SHA256SUMS"));
        assert_mismatch(&check_with(&gh(json!([g]), ASC, SUMS), "0.6.0", &ctx(None)).unwrap());
    }

    #[test]
    fn listed_asset_with_another_digest_is_not_available() {
        for i in 0..5 {
            for d in [Some(format!("sha256:{}", "0".repeat(64))), Some(format!("sha512:{DEB}")), Some(DEB.to_string()), None] {
                let mut g = genuine("v0.6.1");
                match &d {
                    Some(d) => g["assets"][i]["digest"] = json!(d),
                    None => {
                        g["assets"][i].as_object_mut().unwrap().remove("digest");
                    }
                }
                assert_mismatch(&check_with(&gh(json!([g]), ASC, SUMS), "0.6.0", &ctx(None)).unwrap());
            }
        }
        // the old (pre-#294) verdict would have said "available — signed": the signature is fine
        let mut g = genuine("v0.6.1");
        g["assets"][4]["digest"] = json!(format!("sha256:{}", "ab".repeat(32)));
        let r = check_with(&gh(json!([g]), ASC, SUMS), "0.6.0", &ctx(None)).unwrap();
        assert_eq!(r["failed"][0]["kind"], "mismatch");
        assert!(r["reason"].as_str().unwrap().contains("differs"), "{r}");
    }

    #[test]
    fn unsigned_higher_tag_offers_the_signed_one_with_a_warning() {
        // v9.9.9: no .asc at all; v9.0.0: a sums pair that does not verify for it
        let l = json!([rel("v9.9.9", false, false, true, false), rel("v9.0.0", false, false, true, true), genuine("v0.6.1")]);
        let r = check_with(&gh(l, ASC, SUMS), "0.6.0", &ctx(None)).unwrap();
        assert_eq!(r["status"], "available", "{r}");
        assert_eq!(r["latest"], "0.6.1");
        assert_eq!(r["url"], "https://github.com/jpzk/opensidian/releases/tag/v0.6.1");
        assert_eq!(
            r["warnings"],
            json!(["Newer release 9.9.9 is NOT signed (or signature invalid) — ignored.", "Newer release 9.0.0 is NOT signed (or signature invalid) — ignored."])
        );
        // a higher release whose assets were swapped: warned as a mismatch, not offered
        let mut bad = genuine("v0.6.1");
        bad["assets"][1]["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
        let older = json!({"tag_name": "v0.4", "draft": false, "prerelease": false, "published_at": "x", "assets": []});
        let r = check_with(&gh(json!([bad, older]), ASC, SUMS), "0.3", &ctx(None)).unwrap();
        assert_eq!(r["status"], "mismatch", "{r}");
        assert_eq!(r["warnings"], json!([]), "nothing verified: the lower failures are in \"failed\", not warned as newer");
        assert_eq!(r["failed"].as_array().unwrap().len(), 2);
        // the current release verifying is "current", with the higher unsigned tag warned
        let l = json!([rel("v9.9.9", false, false, true, false), genuine("v0.6.1")]);
        let r = check_with(&gh(l, ASC, SUMS), "0.6.1", &ctx(None)).unwrap();
        assert_eq!(r["status"], "current", "{r}");
        assert_eq!(r["text"], "Opensidian is up to date (0.6.1). Latest release 0.6.1: checksums signed by A6E6…4EA3.");
        assert_eq!(r["warnings"].as_array().unwrap().len(), 1);
        assert!(r.get("url").is_none());
    }

    #[test]
    fn at_most_max_tries_sums_pairs_are_fetched() {
        let mut l: Vec<Value> = (0..12).map(|i| rel(&format!("v9.{i}"), false, false, true, true)).collect();
        l.push(genuine("v0.6.1"));
        let (f, log) = recorded(json!(l));
        let r = check_with(&f, "0.6.0", &ctx(None)).unwrap();
        assert_eq!(r["status"], "unsigned", "the window is bounded; nothing verifies inside it: {r}");
        assert!(r.get("url").is_none());
        assert_eq!(log.borrow().len(), 1 + 2 * MAX_TRIES);
    }

    #[test]
    fn highest_signed_version_seen_is_never_downgraded() {
        let l = json!([genuine("v0.6.1")]);
        // seen 0.7.0 earlier, now only 0.6.1 verifies: no offer, rollback
        let r = check_with(&gh(l.clone(), ASC, SUMS), "0.6.0", &ctx(Some("0.7.0"))).unwrap();
        assert_eq!(r["status"], "rollback", "{r}");
        assert!(r.get("url").is_none(), "an older release than one seen is never offered");
        assert_eq!(r["seen"], "0.7.0");
        assert_eq!(r["text"], "A signed release 0.7.0 was seen before, but the newest release that verifies now is 0.6.1 — an older release is not offered.");
        // seen lower or equal: offered, remembered value raised
        for s in ["0.5", "0.6.1"] {
            let r = check_with(&gh(l.clone(), ASC, SUMS), "0.6.0", &ctx(Some(s))).unwrap();
            assert_eq!((r["status"].as_str(), r["seen"].as_str()), (Some("available"), Some("0.6.1")), "{r}");
        }
        // running the seen version itself is just current
        let r = check_with(&gh(l.clone(), ASC, SUMS), "0.7.0", &ctx(Some("0.7.0"))).unwrap();
        assert_eq!(r["status"], "current", "{r}");
        // nothing verifies: the remembered value is kept as it was
        let r = check_with(&gh(l, WRONG, SUMS), "0.6.0", &ctx(Some("0.7.0"))).unwrap();
        assert_eq!((r["status"].as_str(), r["seen"].as_str()), (Some("unsigned"), Some("0.7.0")));
    }

    #[test]
    fn seen_is_stored_monotonically_outside_the_vault() {
        use crate::cfgstore::{apply, Op};
        let mut v = json!({"last": "/v"});
        apply(&mut v, &[Op::UpdateSeen("0.9.6".into())]);
        assert_eq!(v[SEEN_KEY], "0.9.6");
        for low in ["0.9.5", "0.9", "0.9.6", "v1.0", "../x", "1.0.0-rc1", ""] {
            apply(&mut v, &[Op::UpdateSeen(low.into())]);
            assert_eq!(v[SEEN_KEY], "0.9.6", "{low:?} must not replace it");
        }
        apply(&mut v, &[Op::UpdateSeen("0.10.0".into())]);
        assert_eq!(v[SEEN_KEY], "0.10.0");
        // no generic write can lower or wipe it
        apply(&mut v, &[Op::Set(SEEN_KEY.into(), json!("0.1")), Op::SetIn(SEEN_KEY.into(), "x".into(), json!(1))]);
        assert_eq!(v[SEEN_KEY], "0.10.0");
        assert_eq!(v["last"], "/v");
    }

    #[test]
    fn platform_file_follows_the_package_the_app_runs_from() {
        let n = |h: Host| platform_names("1.2.3", &h).first().cloned().unwrap_or_default();
        let lx = |f: fn(&mut Host)| {
            let mut h = Host { os: "linux".into(), arch: "x86_64".into(), ..Host::default() };
            f(&mut h);
            h
        };
        assert_eq!(n(lx(|h| h.flatpak = true)), "opensidian-1.2.3-x86_64.flatpak");
        assert_eq!(n(lx(|h| h.appimage = Some("/home/u/opensidian-1.2.3-x86_64-portable.AppImage".into()))), "opensidian-1.2.3-x86_64-portable.AppImage");
        assert_eq!(n(lx(|h| h.appimage = Some("/opt/Opensidian.AppImage".into()))), "opensidian-1.2.3-x86_64-slim.AppImage");
        assert_eq!(n(lx(|h| h.debian = true)), "opensidian-1.2.3-x86_64.deb");
        assert_eq!(n(lx(|h| h.rpm = true)), "opensidian-1.2.3-x86_64.rpm");
        assert_eq!(n(lx(|h| { h.rpm = true; h.arch = "aarch64".into() })), "opensidian-1.2.3-aarch64.rpm");
        assert_eq!(n(Host { os: "macos".into(), arch: "aarch64".into(), ..Host::default() }), "opensidian-1.2.3-universal.dmg");
        assert!(platform_names("1.2.3", &Host { os: "windows".into(), ..Host::default() }).is_empty());
        // no file for this platform in the signed sums: still offered, with a warning, no hash
        let h = Host { os: "windows".into(), arch: "x86_64".into(), ..Host::default() };
        let r = check_with(&gh(json!([genuine("v0.6.1")]), ASC, SUMS), "0.6.0", &Ctx { lang: "en", seen: None, host: h }).unwrap();
        assert_eq!(r["status"], "available");
        assert!(r["sha256"].is_null());
        assert_eq!(r["warnings"], json!(["No release file for this platform is listed in the signed checksums."]));
    }

    #[test]
    fn verdict_is_translated() {
        for lang in ["es", "ja", "ko", "ru", "zh-CN"] {
            let c = Ctx { lang, seen: None, host: deb_host() };
            let r = check_with(&gh(json!([genuine("v0.6.1")]), ASC, SUMS), "0.6.0", &c).unwrap();
            let t = r["text"].as_str().unwrap();
            assert!(t.contains("0.6.1") && t.contains("A6E6…4EA3") && !t.contains("checksums signed"), "{lang}: {t}");
            let mut g = genuine("v0.6.1");
            g["assets"][0]["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
            let r = check_with(&gh(json!([g]), ASC, SUMS), "0.6.0", &c).unwrap();
            let t = r["text"].as_str().unwrap();
            assert!(t.contains("0.6.1") && !t.contains("do not match"), "{lang}: {t}");
        }
    }

    /* F-84. TEST-ONLY key material: an Ed25519 key built HERE from a fixed public
       seed, never written to disk, never pinned outside #[cfg(test)]. It stands in
       for the successor key so the second slot is exercised without any real key. */
    const TEST_ONLY_SEED: [u8; 32] = *b"opensidian-TEST-ONLY-not-a-key!!";

    fn b64(b: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::new();
        for c in b.chunks(3) {
            let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
            for i in 0..4 {
                s.push(if i <= c.len() { A[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
            }
        }
        s
    }

    fn armor(kind: &str, bin: &[u8]) -> String {
        format!("-----BEGIN PGP {kind}-----\n\n{}\n-----END PGP {kind}-----\n", b64(bin))
    }

    /// (key pair, armored public key, full fingerprint hex)
    fn test_only_key() -> (ed25519_compact::KeyPair, String, String) {
        let kp = ed25519_compact::KeyPair::from_seed(ed25519_compact::Seed::new(TEST_ONLY_SEED));
        let mut body = vec![4u8, 0x65, 0, 0, 0, 22, ED25519_OID.len() as u8];
        body.extend_from_slice(ED25519_OID);
        body.extend_from_slice(&[0x01, 0x07, 0x40]);
        body.extend_from_slice(&kp.pk[..]);
        let fpr = hex(&v4_fingerprint(&body));
        let mut pkt = vec![0xC6, body.len() as u8];
        pkt.extend_from_slice(&body);
        (kp, armor("PUBLIC KEY BLOCK", &pkt), fpr)
    }

    /// a v4 EdDSA binary-document signature over `data` by the test key
    fn test_only_sig(kp: &ed25519_compact::KeyPair, fpr_hex: &str, data: &[u8], ctime: bool) -> String {
        let fpr: Vec<u8> = (0..20).map(|i| u8::from_str_radix(&fpr_hex[2 * i..2 * i + 2], 16).unwrap()).collect();
        let mut hashed = vec![];
        if ctime {
            hashed.extend_from_slice(&[5, 2, 0x65, 0x10, 0, 0]);
        }
        hashed.extend_from_slice(&[22, 33, 4]);
        hashed.extend_from_slice(&fpr);
        let mut part = vec![4u8, 0x00, 22, 8];
        part.extend_from_slice(&(hashed.len() as u16).to_be_bytes());
        part.extend_from_slice(&hashed);
        let mut trailer = part.clone();
        trailer.extend_from_slice(&[0x04, 0xFF]);
        trailer.extend_from_slice(&(part.len() as u32).to_be_bytes());
        let digest = Sha256::new().chain_update(data).chain_update(&trailer).finalize();
        let sig = kp.sk.sign(&digest[..], None);
        let mut b = part;
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&digest[..2]);
        for half in sig[..].chunks(32) {
            b.extend_from_slice(&[0x01, 0x00]);
            b.extend_from_slice(half);
        }
        let mut pkt = vec![0xC2, b.len() as u8];
        pkt.extend_from_slice(&b);
        armor("SIGNATURE", &pkt)
    }

    #[test]
    fn successor_slot_signature_verifies() {
        let keys = trusted_keys().expect("the pinned slots load");
        assert_eq!(keys.len(), 1 + SUCCESSOR.is_some() as usize);
        assert_eq!(hex(&keys[0].fpr), PINNED_FPR);
        let (kp, asc, fpr) = test_only_key();
        assert_ne!(fpr, PINNED_FPR);
        let succ = load_key(&asc, &fpr).expect("test key loads into a slot");
        assert!(load_key(&asc, PINNED_FPR).is_err(), "a slot pins the full fingerprint");
        assert!(load_key(&asc, &fpr[..16]).is_err(), "never a key id");
        let sig = test_only_sig(&kp, &fpr, SUMS, true);
        // current + successor: the successor's signature verifies and names its signer
        let both = [pinned_key().unwrap(), succ];
        assert_eq!(hex(&verify_detached_with(&both, SUMS, sig.as_bytes()).unwrap()), fpr);
        // ... and the current key's real signature still verifies with both slots set
        assert_eq!(hex(&verify_detached_with(&both, SUMS, ASC).unwrap()), PINNED_FPR);
        // a tampered file under the successor's signature: refused for the right reason
        let mut t = SUMS.to_vec();
        t[0] ^= 1;
        assert!(verify_detached_with(&both, &t, sig.as_bytes()).unwrap_err().contains("does not verify"));
        // without the successor slot that signature is just another key
        assert!(verify_detached(SUMS, sig.as_bytes()).unwrap_err().contains("another key"));
        assert!(verify_detached_with(&[], SUMS, ASC).is_err());
    }

    /// the hashed area of a signature packet body with every subpacket of type `drop` removed
    fn strip_subpacket(body: &[u8], drop: u8) -> Vec<u8> {
        let hlen = u16::from_be_bytes([body[4], body[5]]) as usize;
        let (mut s, mut kept) = (&body[6..6 + hlen], vec![]);
        while !s.is_empty() {
            assert!(s[0] < 192, "short subpackets only in the fixtures");
            let len = s[0] as usize;
            if s[1] & 0x7f != drop {
                kept.extend_from_slice(&s[..1 + len]);
            }
            s = &s[1 + len..];
        }
        let mut out = body[..4].to_vec();
        out.extend_from_slice(&(kept.len() as u16).to_be_bytes());
        out.extend_from_slice(&kept);
        out.extend_from_slice(&body[6 + hlen..]);
        out
    }

    #[test]
    fn signature_without_creation_time_is_refused() {
        // the real release signature with its creation-time subpacket cut out
        let bin = dearmor(std::str::from_utf8(ASC).unwrap(), "SIGNATURE").unwrap();
        let (_, body) = packets(&bin).unwrap()[0];
        verify_packet(&pinned_key().unwrap(), SUMS, body).expect("the fixture has one");
        let cut = strip_subpacket(body, 2);
        assert!(cut.len() < body.len());
        assert!(verify_packet(&pinned_key().unwrap(), SUMS, &cut).unwrap_err().contains("creation time"));
        // a cryptographically VALID signature that simply has none: still refused
        let (kp, asc, fpr) = test_only_key();
        let k = [load_key(&asc, &fpr).unwrap()];
        verify_detached_with(&k, SUMS, test_only_sig(&kp, &fpr, SUMS, true).as_bytes()).expect("with a creation time it verifies");
        let e = verify_detached_with(&k, SUMS, test_only_sig(&kp, &fpr, SUMS, false).as_bytes()).unwrap_err();
        assert!(e.contains("creation time"), "{e}");
    }

    /* real later releases: v0.9.6 (24 files, SHA-512 signature) and v0.9.4, as
       published (the sums pair verbatim; digests = what GitHub's API reports) */
    const S096: &[u8] = include_bytes!("../data/updates/v0.9.6-SHA256SUMS");
    const A096: &[u8] = include_bytes!("../data/updates/v0.9.6-SHA256SUMS.asc");
    const S094: &[u8] = include_bytes!("../data/updates/v0.9.4-SHA256SUMS");
    const A094: &[u8] = include_bytes!("../data/updates/v0.9.4-SHA256SUMS.asc");

    fn real(tag: &str, sums: &[u8]) -> Value {
        let mut g = genuine(tag);
        let mut assets: Vec<Value> = std::str::from_utf8(sums)
            .unwrap()
            .lines()
            .map(|l| {
                let (h, n) = l.split_once("  ").unwrap();
                json!({"name": n, "digest": format!("sha256:{h}"), "browser_download_url": format!("https://github.com/jpzk/opensidian/releases/download/{tag}/{n}")})
            })
            .collect();
        assets.extend(g["assets"].as_array().unwrap().iter().filter(|a| a["name"].as_str().unwrap().starts_with("SHA256SUMS")).cloned());
        g["assets"] = json!(assets);
        g
    }

    /// an offline GitHub serving each tag its own sums pair
    fn gh_tags(list: Value) -> impl Fn(&str, u64, &str) -> Result<Vec<u8>, String> {
        move |u: &str, _: u64, _: &str| {
            plugin_store::check_url(u)?;
            if u == RELEASES_URL {
                return Ok(list.to_string().into_bytes());
            }
            for (tag, s, a) in [("v0.9.6", S096, A096), ("v0.9.4", S094, A094), ("v0.6.1", SUMS, ASC)] {
                if let Some(f) = u.strip_prefix(&format!("https://github.com/jpzk/opensidian/releases/download/{tag}/")) {
                    return match f {
                        "SHA256SUMS" => Ok(s.to_vec()),
                        "SHA256SUMS.asc" => Ok(a.to_vec()),
                        _ => Err(format!("unexpected fetch {u}")),
                    };
                }
            }
            Err(format!("unexpected fetch {u}"))
        }
    }

    #[test]
    fn real_releases_verify_and_a_tampered_newer_one_is_warned() {
        let l = json!([real("v0.9.6", S096), real("v0.9.4", S094), genuine("v0.6.1")]);
        let r = check_with(&gh_tags(l.clone()), "0.9.0", &ctx(None)).unwrap();
        assert_eq!((r["status"].as_str(), r["latest"].as_str()), (Some("available"), Some("0.9.6")), "{r}");
        assert_eq!(r["sha256"], "6ee6c401d47dba46e5faa39264d2ef885af5f70a649e534d0de5f8d8c3f22d20");
        let r = check_with(&gh_tags(l.clone()), "0.9.6", &ctx(None)).unwrap();
        assert_eq!(r["status"], "current", "{r}");
        assert_eq!(r["warnings"], json!([]));
        // one swapped binary in v0.9.6: v0.9.4 is offered, v0.9.6 warned as a mismatch
        let mut t = l.clone();
        t[0]["assets"][4]["digest"] = json!(format!("sha256:{}", "1".repeat(64)));
        let r = check_with(&gh_tags(t.clone()), "0.9.0", &ctx(None)).unwrap();
        assert_eq!((r["status"].as_str(), r["latest"].as_str()), (Some("available"), Some("0.9.4")), "{r}");
        assert_eq!(r["warnings"], json!(["Newer release 0.9.6: release assets do not match the signed checksums — ignored."]));
        // ... and if 0.9.6 had verified before, nothing older is offered
        let r = check_with(&gh_tags(t.clone()), "0.9.0", &ctx(Some("0.9.6"))).unwrap();
        assert_eq!(r["status"], "rollback", "{r}");
        assert!(r.get("url").is_none());
        // running 0.9.6 with its own release tampered: never "current — signed" for it
        let r = check_with(&gh_tags(t), "0.9.6", &ctx(None)).unwrap();
        assert_eq!(r["status"], "current");
        assert_eq!(r["latest"], "0.9.4");
        assert_eq!(r["warnings"].as_array().unwrap().len(), 1, "{r}");
        // a v0.9.6 tag carrying v0.9.4's genuinely signed sums is not signed for 0.9.6
        let mut swap = l.clone();
        for a in swap[0]["assets"].as_array_mut().unwrap() {
            if let Some(u) = a["browser_download_url"].as_str().map(|u| u.replace("/v0.9.6/SHA256SUMS", "/v0.9.4/SHA256SUMS")) {
                a["browser_download_url"] = json!(u);
            }
        }
        let r = check_with(&gh_tags(swap), "0.9.0", &ctx(None)).unwrap();
        assert_eq!(r["latest"], "0.9.4", "{r}");
        assert_eq!(r["failed"][0]["kind"], "unsigned");
    }

    /* ------------------------------------------- #300 review follow-up tests */

    const RELEASING: &str = include_str!("../../docs/RELEASING.md");

    /// RELEASING.md describes the bounded window instead of "offers the highest that passes"
    #[test]
    fn releasing_doc_states_the_max_tries_window() {
        assert_eq!(MAX_TRIES, 5, "RELEASING.md names the window as 5: update it with the constant");
        let flat = RELEASING.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("at most the 5 highest that carry a sums pair (`MAX_TRIES`"), "RELEASING.md must name the MAX_TRIES window");
        assert!(flat.contains("If none of those 5 passes, nothing is offered"), "RELEASING.md must say what happens past the window");
    }

    /// the creation time is signer-controlled: the doc must not promise a date cutoff
    #[test]
    fn releasing_doc_does_not_promise_a_creation_time_cutoff() {
        let flat = RELEASING.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!flat.contains("refuse the old key's signatures dated after the compromise"), "a backdated signature defeats any date cutoff");
        assert!(flat.contains("whoever holds the key can backdate it"), "RELEASING.md must say the creation time is not evidence");
    }

    /// a signed platform file that is not attached gets no hash; another attached one still can
    #[test]
    fn signed_but_unattached_platform_file_shows_no_hash() {
        let mut g = genuine("v0.6.1");
        g["assets"].as_array_mut().unwrap().retain(|a| a["name"] != "opensidian-0.6.1-x86_64.deb");
        let r = check_with(&gh(json!([g]), ASC, SUMS), "0.6.0", &ctx(None)).unwrap();
        assert_eq!(r["status"], "available", "the attached files still verify: {r}");
        assert!(r["sha256"].is_null() && r["file"].is_null(), "no hash for a file the release does not offer: {r}");
        assert_eq!(r["warnings"], json!(["The signed release file for this platform (opensidian-0.6.1-x86_64.deb) is not attached to the release."]));
        // translated like every other warning
        let c = Ctx { lang: "ru", seen: None, host: deb_host() };
        let mut g = genuine("v0.6.1");
        g["assets"].as_array_mut().unwrap().retain(|a| a["name"] != "opensidian-0.6.1-x86_64.deb");
        let w = check_with(&gh(json!([g]), ASC, SUMS), "0.6.0", &c).unwrap()["warnings"][0].as_str().unwrap().to_string();
        assert!(w.contains("opensidian-0.6.1-x86_64.deb") && !w.contains("not attached"), "{w}");
        // still attached: unchanged
        let r = check_with(&gh(json!([genuine("v0.6.1")]), ASC, SUMS), "0.6.0", &ctx(None)).unwrap();
        assert_eq!((r["sha256"].as_str(), r["warnings"].clone()), (Some(DEB), json!([])));
    }

    /// a sums download that fails is an error, never "unsigned" with an older offer
    #[test]
    fn failed_sums_download_is_an_error_not_unsigned() {
        let l = json!([real("v0.9.6", S096), real("v0.9.4", S094)]);
        let inner = gh_tags(l.clone());
        for broken in ["/v0.9.6/SHA256SUMS", "/v0.9.6/SHA256SUMS.asc"] {
            let f = |u: &str, cap: u64, w: &str| -> Result<Vec<u8>, String> {
                if u.ends_with(broken) {
                    return Err(format!("{w}: github.com — timed out"));
                }
                inner(u, cap, w)
            };
            let e = check_with(&f, "0.9.0", &ctx(None)).expect_err("a failed download ends the check");
            assert!(e.contains("v0.9.6") && e.contains("timed out"), "{e}");
        }
        // a release that genuinely does not verify still falls through, warned as unsigned
        let mut t = l.clone();
        t[0]["assets"].as_array_mut().unwrap().retain(|a| a["name"] != "SHA256SUMS.asc");
        let r = check_with(&gh_tags(t), "0.9.0", &ctx(None)).unwrap();
        assert_eq!((r["status"].as_str(), r["latest"].as_str()), (Some("available"), Some("0.9.4")), "{r}");
        assert_eq!(r["failed"][0]["kind"], "unsigned");
    }

    /// an emptied list with a remembered newer release is rollback, not "up to date"
    #[test]
    fn empty_list_below_a_seen_release_is_rollback() {
        let r = check_with(&gh(json!([]), ASC, SUMS), "0.6.0", &ctx(Some("0.7.0"))).unwrap();
        assert_eq!(r["status"], "rollback", "{r}");
        assert_eq!(r["seen"], "0.7.0");
        assert!(r.get("url").is_none());
        assert_eq!(r["text"], "A signed release 0.7.0 was seen before, but GitHub lists no release now — nothing is offered.");
        // nothing seen, or seen not above the running version: plain "current"
        for s in [None, Some("0.6.0"), Some("0.5")] {
            assert_eq!(check_with(&gh(json!([]), ASC, SUMS), "0.6.0", &ctx(s)).unwrap()["status"], "current", "{s:?}");
        }
    }

    /// a SeenStore another window writes to while this check is on the network
    struct RacingSeen {
        v: std::cell::RefCell<Option<String>>,
        /// what the other window stores during this check
        other: Option<&'static str>,
    }

    impl SeenStore for RacingSeen {
        fn get(&self) -> Option<String> {
            self.v.borrow().clone()
        }
        fn raise(&self, ver: &str) {
            let mut v = self.v.borrow_mut();
            for c in [Some(ver), self.other].into_iter().flatten() {
                if v.as_deref().map_or(true, |cur| plugin_store::version_gt(c, cur)) {
                    *v = Some(c.to_string());
                }
            }
        }
    }

    /// a higher version stored by a concurrent check is honoured by this one
    #[test]
    fn concurrently_seen_higher_release_is_not_offered_lower() {
        let l = json!([genuine("v0.6.1")]);
        let s = RacingSeen { v: std::cell::RefCell::new(None), other: Some("0.7.0") };
        let r = check_remembered(&gh(l.clone(), ASC, SUMS), "0.6.0", "en", deb_host(), &s).unwrap();
        assert_eq!(r["status"], "rollback", "{r}");
        assert!(r.get("url").is_none(), "nothing below the stored 0.7.0 is offered");
        assert_eq!(r["seen"], "0.7.0");
        assert_eq!(s.get().as_deref(), Some("0.7.0"));
        // no concurrent writer: offered and remembered as before
        let s = RacingSeen { v: std::cell::RefCell::new(None), other: None };
        let r = check_remembered(&gh(l.clone(), ASC, SUMS), "0.6.0", "en", deb_host(), &s).unwrap();
        assert_eq!((r["status"].as_str(), s.get().as_deref()), (Some("available"), Some("0.6.1")), "{r}");
        // a concurrent writer storing a LOWER value changes nothing
        let s = RacingSeen { v: std::cell::RefCell::new(None), other: Some("0.5") };
        let r = check_remembered(&gh(l, ASC, SUMS), "0.6.0", "en", deb_host(), &s).unwrap();
        assert_eq!((r["status"].as_str(), s.get().as_deref()), (Some("available"), Some("0.6.1")), "{r}");
    }
}
