// SPDX-License-Identifier: GPL-3.0-or-later
//! M2c backup words (docs/sync/key-tree.md §1): embedded EFF list pin,
//! round trip, checksum KATs, dice input, BIP39 wallet-seed rejection.
//! Checksum KATs were computed independently with coreutils sha256sum:
//! `{ printf 'osy words v1'; printf '<S_bytes as \x escapes>'; } | sha256sum`.
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};

use sync_crypto::fips::FipsProvider;
use sync_crypto::soft::{hex, SoftProvider};
use sync_crypto::words::{
    dice_code, index_of, word, Phrase, WordsError, LIST_LEN, PHRASE_WORDS, WORDLIST_FILE,
    WORDLIST_SHA256,
};
use sync_crypto::{content_sha256, CryptoError, CryptoProvider, Sealed, SigAlg};

fn fips() -> FipsProvider {
    FipsProvider::new().expect("FIPS provider")
}

#[test]
fn embedded_list_matches_pinned_sha256() {
    assert_eq!(hex(&content_sha256(WORDLIST_FILE.as_bytes())), WORDLIST_SHA256);
    let file = std::fs::read(format!(
        "{}/wordlists/eff_large_wordlist.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert_eq!(hex(&content_sha256(&file)), WORDLIST_SHA256);
}

#[test]
fn list_shape() {
    assert_eq!(WORDLIST_FILE.lines().count(), LIST_LEN);
    let mut prev = "";
    for i in 0..LIST_LEN {
        let w = word(i as u16).unwrap();
        assert!(w.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'), "{i}");
        assert!(w > prev, "sorted + unique at {i}");
        assert_eq!(index_of(w), Some(i as u16));
        prev = w;
    }
    assert_eq!(word(LIST_LEN as u16), None);
    assert_eq!(word(0), Some("abacus"));
    assert_eq!(word(7775), Some("zoom"));
    assert_eq!(dice_code(0), "11111");
    assert_eq!(dice_code(7775), "66666");
    assert_eq!(index_of("Zoom"), None, "index_of is exact; parse lowercases");
    // four hyphenated words exist (drop-down, felt-tip, t-shirt, yo-yo)
    let hy: Vec<&str> = (0..LIST_LEN as u16).map(|i| word(i).unwrap()).filter(|w| w.contains('-')).collect();
    assert_eq!(hy, ["drop-down", "felt-tip", "t-shirt", "yo-yo"]);
}

/// (S_bytes hex, 11 words, checksum word) — sha256sum-computed.
const KATS: &[(&str, &str, &str)] = &[
    (
        "00000000000000000000000000000000000000000000",
        "abacus abacus abacus abacus abacus abacus abacus abacus abacus abacus abacus",
        "authentic",
    ),
    (
        "1e5f1e5f1e5f1e5f1e5f1e5f1e5f1e5f1e5f1e5f1e5f",
        "zoom zoom zoom zoom zoom zoom zoom zoom zoom zoom zoom",
        "unstylish",
    ),
    (
        "000100020003000400050006000700080009000a000b",
        "abdomen abdominal abide abiding ability ablaze able abnormal abrasion abrasive abreast",
        "harmonica",
    ),
];

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn checksum_kats_both_providers() {
    let f = fips();
    for (sb, w11, ck) in KATS {
        let mut b = [0u8; 22];
        b.copy_from_slice(&unhex(sb));
        let ph = Phrase::from_s_bytes(&b).unwrap();
        let want = format!("{w11} {ck}");
        assert_eq!(&*ph.to_text(&f), &want);
        assert_eq!(&*ph.to_text(&SoftProvider), &want);
        let back = Phrase::parse(&f, &want).unwrap();
        assert_eq!(hex(&back.s_bytes()[..]), *sb);
    }
}

#[test]
fn dice_matches_eff_codes() {
    // every word's own dice code maps to its index
    for i in 0..LIST_LEN {
        let rolls = dice_code(i).repeat(11);
        let ph = Phrase::from_dice(&rolls).unwrap();
        assert!(ph.indices().iter().all(|&x| x as usize == i), "{i}");
    }
    // KAT 3 from dice, with whitespace
    let rolls = "11112 11113 11114 11115 11116 11121\n11122 11123 11124 11125 11126";
    let ph = Phrase::from_dice(rolls).unwrap();
    assert_eq!(ph.words(&fips())[11], "harmonica");
    assert_eq!(ph.words(&fips())[0], "abdomen");
}

#[test]
fn dice_rejects_bad_input() {
    let ok = "1".repeat(55);
    assert!(Phrase::from_dice(&ok).is_ok());
    for bad in [
        "1".repeat(54),
        "1".repeat(56),
        format!("{}7", "1".repeat(54)),
        format!("{}0", "1".repeat(54)),
        format!("{}a", "1".repeat(54)),
        String::new(),
    ] {
        assert_eq!(Phrase::from_dice(&bad).err(), Some(WordsError::Dice), "{bad:?}");
    }
}

#[test]
fn generated_round_trip() {
    let f = fips();
    let mut seen = HashSet::new();
    for _ in 0..500 {
        let ph = Phrase::generate(&f).unwrap();
        assert!(ph.indices().iter().all(|&x| (x as usize) < LIST_LEN));
        let text = ph.to_text(&f);
        assert_eq!(text.split(' ').count(), PHRASE_WORDS);
        let back = Phrase::parse(&f, &text).unwrap();
        assert!(back == ph);
        let sb = ph.s_bytes();
        assert!(Phrase::from_s_bytes(&sb).unwrap() == ph);
        assert!(seen.insert(sb.to_vec()), "142-bit phrases must not repeat");
    }
}

#[test]
fn parse_is_case_and_whitespace_tolerant() {
    let f = fips();
    let (_, w11, ck) = KATS[2];
    let messy = format!("  {}\n\t{}  ", w11.to_uppercase().replace(' ', "\t "), ck);
    let ph = Phrase::parse(&f, &messy).unwrap();
    assert_eq!(hex(&ph.s_bytes()[..]), KATS[2].0);
}

#[test]
fn parse_errors() {
    let f = fips();
    let (_, w11, ck) = KATS[2];
    assert_eq!(Phrase::parse(&f, w11).err(), Some(WordsError::WordCount { got: 11 }));
    assert_eq!(
        Phrase::parse(&f, &format!("{w11} {ck} {ck}")).err(),
        Some(WordsError::WordCount { got: 13 })
    );
    let typo = format!("{w11} harmonicx");
    assert_eq!(Phrase::parse(&f, &typo).err(), Some(WordsError::UnknownWord { pos: 12 }));
    let e = Phrase::parse(&f, &typo).unwrap_err().to_string();
    assert!(!e.contains("harmonic"), "error must not echo words: {e}");
    // wrong checksum word
    assert_eq!(
        Phrase::parse(&f, &format!("{w11} zoom")).err(),
        Some(WordsError::Checksum)
    );
}

#[test]
fn checksum_catches_substitution_and_swap() {
    let f = fips();
    let (_, w11, ck) = KATS[2];
    let good: Vec<&str> = format!("{w11} {ck}").leak().split(' ').collect();
    for pos in 0..PHRASE_WORDS {
        let mut w = good.clone();
        let i = index_of(w[pos]).unwrap();
        w[pos] = word((i + 1) % LIST_LEN as u16).unwrap();
        assert_eq!(
            Phrase::parse(&f, &w.join(" ")).err(),
            Some(WordsError::Checksum),
            "substitution at {pos}"
        );
    }
    for pos in 0..PHRASE_WORDS - 2 {
        let mut w = good.clone();
        w.swap(pos, pos + 1);
        assert_eq!(
            Phrase::parse(&f, &w.join(" ")).err(),
            Some(WordsError::Checksum),
            "swap at {pos}"
        );
    }
}

#[test]
fn bip39_test_vectors_rejected() {
    // 12-word mnemonics from the BIP-0039 reference test vectors
    // (trezor/python-mnemonic vectors.json, english).
    let f = fips();
    for m in [
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        "legal winner thank year wave sausage worth useful legal winner thank yellow",
        "letter advice cage absurd amount doctor acoustic avoid letter advice cage above",
        "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong",
        "scheme spot photo card baby mountain device kick cradle pact join borrow",
        "jelly better achieve collect unaware mountain thought cargo oxygen act hood bridge",
        "hamster diagram private dutch cause delay private meat slide toddler razor book",
        "cat swing flag economy stadium alone churn speed unique patch report train",
    ] {
        let e = Phrase::parse(&f, m).unwrap_err();
        assert!(
            matches!(e, WordsError::UnknownWord { .. } | WordsError::Checksum),
            "{m}: {e:?}"
        );
    }
    // words shared by both lists: an all-shared phrase still needs the
    // checksum (p = 1/7776)
    let shared = ["legal", "winner", "thank", "letter", "cage", "absurd", "amount", "avoid"];
    for w in shared {
        assert!(index_of(w).is_some(), "{w}");
    }
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut passed = 0;
    for _ in 0..2000 {
        let m: Vec<&str> = (0..12)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                shared[(x % 8) as usize]
            })
            .collect();
        match Phrase::parse(&f, &m.join(" ")) {
            Ok(_) => passed += 1,
            Err(e) => assert_eq!(e, WordsError::Checksum),
        }
    }
    assert!(passed <= 3, "{passed} of 2000 random shared-word phrases passed");
}

#[test]
fn random_eff_phrases_fail_checksum_at_expected_rate() {
    // 20000 uniformly random 12-word EFF phrases: expected passes 20000/7776 ≈ 2.6.
    let f = fips();
    let mut passed = 0;
    let mut b = [0u8; 24];
    for _ in 0..20000 {
        f.random(&mut b).unwrap();
        let m: Vec<&str> = b
            .as_chunks::<2>().0.iter()
            .map(|c| word(u16::from_be_bytes(*c) % LIST_LEN as u16).unwrap())
            .collect();
        if Phrase::parse(&f, &m.join(" ")).is_ok() {
            passed += 1;
        }
    }
    // P(X ≥ 15 | Poisson 2.57) < 1e-7
    assert!(passed < 15, "{passed}");
}

#[test]
fn debug_is_redacted() {
    let ph = Phrase::from_dice(&"1".repeat(55)).unwrap();
    assert_eq!(format!("{ph:?}"), "Phrase(<redacted>)");
}

/// Provider whose DRBG replays scripted bytes (rejection-sampling boundary).
struct Scripted(RefCell<VecDeque<u8>>);

impl CryptoProvider for Scripted {
    type SigningKey = ();
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn fips_mode(&self) -> bool {
        false
    }
    fn sha256(&self, d: &[u8]) -> [u8; 32] {
        SoftProvider.sha256(d)
    }
    fn hmac_sha256(&self, k: &[u8], m: &[u8]) -> [u8; 32] {
        SoftProvider.hmac_sha256(k, m)
    }
    fn hkdf_sha256(&self, i: &[u8], s: &[u8], n: &[u8], o: &mut [u8]) -> Result<(), CryptoError> {
        SoftProvider.hkdf_sha256(i, s, n, o)
    }
    fn random(&self, out: &mut [u8]) -> Result<(), CryptoError> {
        let mut q = self.0.borrow_mut();
        for b in out.iter_mut() {
            *b = q.pop_front().ok_or(CryptoError::Module)?;
        }
        Ok(())
    }
    fn aead_seal(&self, _: &[u8; 32], _: &[u8], _: &[u8]) -> Result<Sealed, CryptoError> {
        Err(CryptoError::Unsupported)
    }
    fn aead_open(&self, _: &[u8; 32], _: &[u8; 12], _: &[u8], _: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Err(CryptoError::Unsupported)
    }
    fn generate_signing_key(&self, _: SigAlg) -> Result<((), Vec<u8>), CryptoError> {
        Err(CryptoError::Unsupported)
    }
    fn sign(&self, _: &(), _: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Err(CryptoError::Unsupported)
    }
    fn verify(&self, _: SigAlg, _: &[u8], _: &[u8], _: &[u8]) -> Result<(), CryptoError> {
        Err(CryptoError::Unsupported)
    }
}

#[test]
fn rejection_sampling_boundary() {
    // first draw: 11 × 0xF300 (= 62208, rejected) ; second: 0xF2FF (62207 →
    // 7775) then 0x0000.., so the 11 accepted values are 7775 + 10 × 0.
    let mut bytes = Vec::new();
    for _ in 0..11 {
        bytes.extend_from_slice(&[0xF3, 0x00]);
    }
    bytes.extend_from_slice(&[0xF2, 0xFF]);
    bytes.extend_from_slice(&[0u8; 20]);
    let p = Scripted(RefCell::new(bytes.into()));
    let ph = Phrase::generate(&p).unwrap();
    let mut want = [0u16; 11];
    want[0] = 7775;
    assert_eq!(ph.indices(), &want);
    // DRBG failure surfaces as Rng
    let empty = Scripted(RefCell::new(VecDeque::new()));
    assert_eq!(Phrase::generate(&empty).err(), Some(WordsError::Rng));
}
