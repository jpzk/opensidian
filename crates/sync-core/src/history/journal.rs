// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! `index.log` grammar (version 1). Text, one record per line, append-only.
//!
//! ```text
//! file    = header *record
//! header  = "osy-history 1" LF
//! record  = seq TAB ts_ms TAB reason TAB sha TAB size TAB path TAB from LF
//! seq     = 1*DIGIT            ; strictly increasing, starts at 1
//! ts_ms   = 1*DIGIT            ; unix milliseconds, caller clock
//! reason  = "own-save" / "pre-write" / "external" / "delete" / "rename"
//!         / "idle" / "restore"
//! sha     = 64LOWHEX / ""      ; "" only for a tombstone of never-stored bytes
//! size    = 1*DIGIT            ; byte length of the blob (0 if sha = "")
//! path    = esc                ; vault-relative, "/" separated, never empty
//! from    = esc / ""           ; old path for "rename", else ""
//! esc     = 1*( %x21-24 / %x26-7E / %x80-FF / "%" 2UPHEX / " " )
//! ```
//!
//! Escaping: `%`, every byte < 0x20 and 0x7f are written as `%XX` (upper
//! hex); everything else is raw UTF-8. A torn final line (no LF) is a
//! crash artefact and is truncated on open; any other malformed line is
//! corruption and opening fails (never silently drop history).

use super::{Entry, HistoryError, Reason};

pub(crate) const HEADER: &str = "osy-history 1\n";

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let c = ch as u32;
        if ch == '%' || c < 0x20 || c == 0x7f {
            out.push_str(&format!("%{:02X}", c));
        } else {
            out.push(ch);
        }
    }
    out
}

pub(crate) fn unescape(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if i + 2 >= b.len() {
                return Err("truncated escape".into());
            }
            let hi = hexval(b[i + 1]).ok_or("bad escape")?;
            let lo = hexval(b[i + 2]).ok_or("bad escape")?;
            let v = hi * 16 + lo;
            if !(v == b'%' || v < 0x20 || v == 0x7f) {
                return Err("non-canonical escape".into());
            }
            out.push(v);
            i += 3;
        } else {
            if b[i] < 0x20 || b[i] == 0x7f {
                return Err("raw control byte".into());
            }
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "invalid utf-8".to_string())
}

fn hexval(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn is_sha_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

pub(crate) fn encode(e: &Entry) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        e.seq,
        e.ts_ms,
        e.reason.as_str(),
        e.sha.as_deref().unwrap_or(""),
        e.size,
        escape(&e.path),
        e.from.as_deref().map(escape).unwrap_or_default()
    )
}

/// Decode one line WITHOUT its trailing LF.
pub(crate) fn decode(line: &str) -> Result<Entry, String> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() != 7 {
        return Err(format!("expected 7 fields, got {}", f.len()));
    }
    let seq: u64 = f[0].parse().map_err(|_| "bad seq")?;
    let ts_ms: u64 = f[1].parse().map_err(|_| "bad ts")?;
    let reason = Reason::parse(f[2]).ok_or("bad reason")?;
    let sha = match f[3] {
        "" => None,
        s if is_sha_hex(s) => Some(s.to_string()),
        _ => return Err("bad sha".into()),
    };
    let size: u64 = f[4].parse().map_err(|_| "bad size")?;
    let path = unescape(f[5])?;
    if path.is_empty() {
        return Err("empty path".into());
    }
    let from = if f[6].is_empty() { None } else { Some(unescape(f[6])?) };
    if sha.is_none() && (reason != Reason::Delete || size != 0) {
        return Err("missing sha".into());
    }
    if from.is_some() != (reason == Reason::Rename) {
        return Err("from only on rename".into());
    }
    Ok(Entry { seq, ts_ms, reason, sha, size, path, from })
}

/// Parse a whole journal. Returns the entries and the byte length of the
/// valid prefix (shorter than `text` iff the last line is torn).
pub(crate) fn parse(text: &str) -> Result<(Vec<Entry>, usize), HistoryError> {
    if text.is_empty() {
        return Ok((Vec::new(), 0));
    }
    if !text.starts_with(HEADER) {
        if HEADER.starts_with(text) {
            // torn header write: treat as empty journal
            return Ok((Vec::new(), 0));
        }
        return Err(HistoryError::Corrupt { line: 1, msg: "bad header".into() });
    }
    let mut entries: Vec<Entry> = Vec::new();
    let mut pos = HEADER.len();
    let mut lineno = 1usize;
    while pos < text.len() {
        lineno += 1;
        let rest = &text[pos..];
        let Some(nl) = rest.find('\n') else {
            break; // torn final line
        };
        let e = decode(&rest[..nl]).map_err(|msg| HistoryError::Corrupt { line: lineno, msg })?;
        if let Some(prev) = entries.last() {
            if e.seq <= prev.seq {
                return Err(HistoryError::Corrupt { line: lineno, msg: "seq not increasing".into() });
            }
        }
        entries.push(e);
        pos += nl + 1;
    }
    Ok((entries, pos))
}
