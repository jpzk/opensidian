// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! Backup words S (docs/sync/key-tree.md §1): 11 random words from the EFF
//! large wordlist + 1 checksum word.
//!
//! - list: `wordlists/eff_large_wordlist.txt`, the unmodified EFF file
//!   (7776 lines `ddddd<TAB>word`), embedded at build time; its SHA-256 is
//!   pinned in [`WORDLIST_SHA256`] and checked by the tests. Licence
//!   CC-BY 3.0 US, Electronic Frontier Foundation (THIRD-PARTY.md).
//! - entropy: 11 × log2(7776) = 142.2 bits, from the module DRBG with
//!   rejection sampling ([`Phrase::generate`]) or from 55 d6 rolls
//!   ([`Phrase::from_dice`]).
//! - canonical bytes `S_bytes` = 11 × u16 big-endian index (22 bytes); words
//!   are never hashed as strings.
//! - checksum word = `u16be(SHA-256("osy words v1" || S_bytes)[0..2]) mod
//!   7776`. UX only (typo vs. "vault gone", version detection); it also
//!   rejects BIP39 wallet seeds: most BIP39 words are not EFF words, and a
//!   phrase made only of shared words still fails the checksum with
//!   p = 7775/7776.
//!
//! Errors never contain a word, only its 1-based position.
use std::fmt;
use std::sync::OnceLock;

use zeroize::{Zeroize, Zeroizing};

use crate::CryptoProvider;

/// Words in the EFF large list (6⁵).
pub const LIST_LEN: usize = 7776;
/// Random words per phrase.
pub const ENTROPY_WORDS: usize = 11;
/// Words per phrase including the checksum word.
pub const PHRASE_WORDS: usize = 12;
/// Length of `S_bytes`.
pub const S_BYTES_LEN: usize = 2 * ENTROPY_WORDS;
/// d6 rolls per word.
pub const ROLLS_PER_WORD: usize = 5;
/// Checksum domain label (v2 changes it → version detection).
pub const CHECKSUM_LABEL: &[u8] = b"osy words v1";
/// SHA-256 of `wordlists/eff_large_wordlist.txt` as published at
/// https://www.eff.org/files/2016/07/18/eff_large_wordlist.txt.
pub const WORDLIST_SHA256: &str =
    "addd35536511597a02fa0a9ff1e5284677b8883b83e986e43f15a3db996b903e";

/// The embedded list file, byte for byte.
pub const WORDLIST_FILE: &str = include_str!("../wordlists/eff_large_wordlist.txt");

/// Rejection bound for u16 draws: largest multiple of 7776 ≤ 65536.
const REJECT_AT: u16 = (LIST_LEN * 8) as u16; // 62208

/// Word-phrase failure. Positions are 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordsError {
    /// Not 12 words.
    WordCount { got: usize },
    /// The word at this position is not in the EFF large list.
    UnknownWord { pos: usize },
    /// All words are known but the checksum word does not match (typo,
    /// different phrase version, or a BIP39 wallet seed).
    Checksum,
    /// Dice input: not 55 digits in 1..=6 (whitespace ignored).
    Dice,
    /// Index out of range in `S_bytes`.
    Invalid,
    /// The DRBG failed.
    Rng,
}

impl fmt::Display for WordsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WordsError::WordCount { got } => {
                write!(f, "expected {PHRASE_WORDS} backup words, got {got}")
            }
            WordsError::UnknownWord { pos } => write!(f, "word {pos} is not a backup word"),
            WordsError::Checksum => f.write_str(
                "backup words do not check out (typo, or not an Opensidian phrase, e.g. a wallet seed)",
            ),
            WordsError::Dice => write!(
                f,
                "dice input must be {} rolls of 1-6",
                ENTROPY_WORDS * ROLLS_PER_WORD
            ),
            WordsError::Invalid => f.write_str("invalid backup word bytes"),
            WordsError::Rng => f.write_str("random generator failure"),
        }
    }
}

impl std::error::Error for WordsError {}

/// Parsed list: word by index. Panics only if the embedded file is
/// malformed, which the tests rule out (it is a compile-time constant).
fn list() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut v = Vec::with_capacity(LIST_LEN);
        for (i, line) in WORDLIST_FILE.lines().enumerate() {
            let (code, word) = line.split_once('\t').expect("eff list: tab");
            assert_eq!(code, dice_code(i), "eff list: dice code order");
            v.push(word);
        }
        assert_eq!(v.len(), LIST_LEN, "eff list: length");
        v
    })
}

/// Word for `index` (0..7776).
pub fn word(index: u16) -> Option<&'static str> {
    list().get(index as usize).copied()
}

/// Index of `w` (exact, lowercase; four words contain `-`). The list is
/// byte-sorted, so binary search.
pub fn index_of(w: &str) -> Option<u16> {
    list().binary_search(&w).ok().map(|i| i as u16)
}

/// EFF dice code ("11111".."66666") of an index.
pub fn dice_code(index: usize) -> String {
    let mut d = [0u8; ROLLS_PER_WORD];
    let mut x = index;
    for slot in d.iter_mut().rev() {
        *slot = b'1' + (x % 6) as u8;
        x /= 6;
    }
    String::from_utf8(d.to_vec()).expect("ascii")
}

/// Backup words S: the 11 entropy indices. Zeroized on drop.
pub struct Phrase {
    idx: [u16; ENTROPY_WORDS],
}

impl Drop for Phrase {
    fn drop(&mut self) {
        self.idx.zeroize();
    }
}

impl fmt::Debug for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Phrase(<redacted>)")
    }
}

impl PartialEq for Phrase {
    fn eq(&self, other: &Self) -> bool {
        crate::ct_eq(&self.s_bytes()[..], &other.s_bytes()[..])
    }
}

impl Eq for Phrase {}

impl Phrase {
    /// New random phrase from the module DRBG (rejection sampling: draw u16,
    /// reject ≥ 62208, take x mod 7776 → uniform).
    pub fn generate<P: CryptoProvider + ?Sized>(p: &P) -> Result<Self, WordsError> {
        let mut idx = [0u16; ENTROPY_WORDS];
        let mut n = 0;
        let mut buf = Zeroizing::new([0u8; 2 * ENTROPY_WORDS]);
        while n < ENTROPY_WORDS {
            p.random(&mut buf[..]).map_err(|_| WordsError::Rng)?;
            for pair in buf.chunks_exact(2) {
                let x = u16::from_be_bytes([pair[0], pair[1]]);
                if x < REJECT_AT && n < ENTROPY_WORDS {
                    idx[n] = x % LIST_LEN as u16;
                    n += 1;
                }
            }
        }
        let ph = Phrase { idx };
        idx.zeroize();
        Ok(ph)
    }

    /// Phrase from 55 d6 rolls (digits 1-6, whitespace ignored), 5 per word
    /// in EFF order (first roll = most significant). The checksum word is
    /// computed, not rolled.
    pub fn from_dice(rolls: &str) -> Result<Self, WordsError> {
        let mut idx = [0u16; ENTROPY_WORDS];
        let mut n = 0usize;
        for c in rolls.bytes() {
            if c.is_ascii_whitespace() {
                continue;
            }
            if !(b'1'..=b'6').contains(&c) || n >= ENTROPY_WORDS * ROLLS_PER_WORD {
                idx.zeroize();
                return Err(WordsError::Dice);
            }
            let w = n / ROLLS_PER_WORD;
            idx[w] = idx[w] * 6 + (c - b'1') as u16;
            n += 1;
        }
        if n != ENTROPY_WORDS * ROLLS_PER_WORD {
            idx.zeroize();
            return Err(WordsError::Dice);
        }
        let ph = Phrase { idx };
        idx.zeroize();
        Ok(ph)
    }

    /// From canonical `S_bytes` (keystore restore).
    pub fn from_s_bytes(b: &[u8; S_BYTES_LEN]) -> Result<Self, WordsError> {
        let mut idx = [0u16; ENTROPY_WORDS];
        for (i, pair) in b.chunks_exact(2).enumerate() {
            let x = u16::from_be_bytes([pair[0], pair[1]]);
            if x as usize >= LIST_LEN {
                idx.zeroize();
                return Err(WordsError::Invalid);
            }
            idx[i] = x;
        }
        let ph = Phrase { idx };
        idx.zeroize();
        Ok(ph)
    }

    /// Parse 12 words: ASCII case-insensitive, any whitespace between words.
    /// Checks every word against the list, then the checksum word.
    pub fn parse<P: CryptoProvider + ?Sized>(p: &P, text: &str) -> Result<Self, WordsError> {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() != PHRASE_WORDS {
            return Err(WordsError::WordCount { got: words.len() });
        }
        let mut all = [0u16; PHRASE_WORDS];
        for (i, w) in words.iter().enumerate() {
            let lower = Zeroizing::new(w.to_ascii_lowercase());
            match index_of(&lower) {
                Some(x) => all[i] = x,
                None => {
                    all.zeroize();
                    return Err(WordsError::UnknownWord { pos: i + 1 });
                }
            }
        }
        let mut idx = [0u16; ENTROPY_WORDS];
        idx.copy_from_slice(&all[..ENTROPY_WORDS]);
        let want = all[ENTROPY_WORDS];
        all.zeroize();
        let ph = Phrase { idx };
        idx.zeroize();
        if ph.checksum_index(p) != want {
            return Err(WordsError::Checksum);
        }
        Ok(ph)
    }

    /// Canonical bytes: 11 × u16 big-endian (key-tree.md §1).
    pub fn s_bytes(&self) -> Zeroizing<[u8; S_BYTES_LEN]> {
        let mut out = Zeroizing::new([0u8; S_BYTES_LEN]);
        for (i, x) in self.idx.iter().enumerate() {
            out[2 * i..2 * i + 2].copy_from_slice(&x.to_be_bytes());
        }
        out
    }

    /// The 11 entropy indices.
    pub fn indices(&self) -> &[u16; ENTROPY_WORDS] {
        &self.idx
    }

    /// Checksum word index: `u16be(SHA-256(label || S_bytes)[0..2]) mod 7776`.
    pub fn checksum_index<P: CryptoProvider + ?Sized>(&self, p: &P) -> u16 {
        let s = self.s_bytes();
        let mut msg = Zeroizing::new(Vec::with_capacity(CHECKSUM_LABEL.len() + S_BYTES_LEN));
        msg.extend_from_slice(CHECKSUM_LABEL);
        msg.extend_from_slice(&s[..]);
        let mut h = p.sha256(&msg);
        let x = u16::from_be_bytes([h[0], h[1]]) % LIST_LEN as u16;
        h.zeroize();
        x
    }

    /// The 12 words (11 + checksum).
    pub fn words<P: CryptoProvider + ?Sized>(&self, p: &P) -> [&'static str; PHRASE_WORDS] {
        let mut out = [""; PHRASE_WORDS];
        for (o, x) in out.iter_mut().zip(self.idx.iter()) {
            *o = word(*x).expect("index < 7776 by construction");
        }
        out[ENTROPY_WORDS] = word(self.checksum_index(p)).expect("mod 7776");
        out
    }

    /// The 12 words joined by single spaces (for display; zeroized on drop).
    pub fn to_text<P: CryptoProvider + ?Sized>(&self, p: &P) -> Zeroizing<String> {
        Zeroizing::new(self.words(p).join(" "))
    }
}
