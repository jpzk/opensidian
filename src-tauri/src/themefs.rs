// SPDX-License-Identifier: GPL-3.0-or-later
/*! themefs — themes and snippets are FILES IN THE VAULT, stock's files.

Design: docs/themefs/DESIGN.md. Recon: docs/recon-themes/README.md (T1–T8);
the oracle is docs/recon-themes/probe-stock-vault.sh. Everything here is pure
functions over paths/bytes — unit-testable without a display; main.rs wraps
them in thin tauri commands (DESIGN §1).

THIS FILE, part 1 (ledger item 2): the `.obsidian/appearance.json` round-trip.

The file is STOCK'S file (T2): one flat JSON object, a sparse deviation
record — absent key = default, absent FILE = every default. Stock rewrites it
in full on every change, pretty-printed with 2-space indent, `": "` separator
and NO trailing newline (the T2 log's own size line: 180 B for the six-key
final dump — docs/fixtures/themefs/README.md does the arithmetic). That is
exactly serde_json `to_string_pretty`'s layout, the same equivalence bmcompat
proved for bookmarks.json (main.rs bm_emit).

Keys WE author: `cssTheme` (directory name, `""` = built-in Default — T2) and
`enabledCssSnippets` (array of pane labels = basename minus `.css`, in enable
order, disable REMOVES the entry — T3 RESULT 3). Every other key rides
through byte-wise: read bytes → parse with preserve_order (Cargo.toml locks
the feature) → mutate ONLY the key we author → pretty-print. Criterion 2.
*/

// Wired into tauri commands by the R5/R3 ledger items (picker + snippet
// toggle); until those land only the tests call this module.
#![allow(dead_code)]

use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const APPEARANCE_FILE: &str = ".obsidian/appearance.json";

/// Read the flat object. ABSENT file is Ok(empty map) — stock treats absence
/// as "every default" and does not create the file until a deviation (T2).
/// A file that EXISTS but does not parse as a JSON object is an Err: the
/// caller must refuse to write over bytes it cannot read (the loud-failure
/// rule — destroying a user's hand-edited file to save our one key is the
/// bookmarks lesson in reverse).
pub fn read_appearance(root: &Path) -> Result<Map<String, Value>, String> {
    let p = root.join(APPEARANCE_FILE);
    if !p.exists() {
        return Ok(Map::new());
    }
    let body = fs::read_to_string(&p).map_err(|e| format!("appearance.json unreadable: {e}"))?;
    match serde_json::from_str::<Value>(&body) {
        Ok(Value::Object(o)) => Ok(o),
        Ok(_) => Err("appearance.json is not a JSON object — refusing to touch it".into()),
        Err(e) => Err(format!("appearance.json does not parse ({e}) — refusing to touch it")),
    }
}

/// The ONE serializer: stock's layout (2-space indent, ": " separator, no
/// trailing newline). Nothing else may turn the map into bytes.
fn write_appearance(root: &Path, map: &Map<String, Value>) -> Result<(), String> {
    let body = serde_json::to_string_pretty(&Value::Object(map.clone()))
        .map_err(|e| e.to_string())?;
    fs::create_dir_all(root.join(".obsidian")).map_err(|e| e.to_string())?;
    fs::write(root.join(APPEARANCE_FILE), body).map_err(|e| e.to_string())
}

/// `cssTheme`: the active theme's DIRECTORY name; `""` or absent = built-in
/// Default (T2 — Default is written as `""`, never as a name).
pub fn css_theme(root: &Path) -> String {
    read_appearance(root)
        .ok()
        .and_then(|m| m.get("cssTheme").and_then(|v| v.as_str().map(String::from)))
        .unwrap_or_default()
}

/// Write `cssTheme`. preserve_order's insert keeps an existing key's position
/// and appends a new one at the end — both are what stock's own rewrites show
/// (T2: keys appear in first-deviation order and stay put).
pub fn set_css_theme(root: &Path, name: &str) -> Result<(), String> {
    let mut m = read_appearance(root)?; // Err = refuse, never overwrite
    m.insert("cssTheme".into(), Value::String(name.into()));
    write_appearance(root, &m)
}

/// `enabledCssSnippets`: pane labels (basename minus `.css`), enable order.
/// Absent key or absent file = nothing enabled (T3 RESULT 3).
pub fn enabled_snippets(root: &Path) -> Vec<String> {
    read_appearance(root)
        .ok()
        .and_then(|m| m.get("enabledCssSnippets").cloned())
        .and_then(|v| match v {
            Value::Array(a) => Some(
                a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// Toggle one snippet label. Measured semantics (T3 RESULT 3): enabling
/// APPENDS (array is enable order), disabling REMOVES the entry — disabled is
/// not recorded as `false`, it is simply not listed. Enabling twice is one
/// entry. When the last entry is disabled the KEY stays as `[]`: stock was
/// only measured removing ENTRIES, never observed on the last-entry case, and
/// keeping a present key is the smaller mutation (re-measure against
/// /srv/reference/obsidian.AppImage if a gate ever makes this matter).
pub fn set_snippet_enabled(root: &Path, label: &str, on: bool) -> Result<(), String> {
    let mut m = read_appearance(root)?; // Err = refuse, never overwrite
    let mut list: Vec<Value> = match m.get("enabledCssSnippets") {
        Some(Value::Array(a)) => a.clone(),
        _ => Vec::new(),
    };
    let present = list.iter().any(|v| v.as_str() == Some(label));
    if on && !present {
        list.push(Value::String(label.into()));
    } else if !on {
        list.retain(|v| v.as_str() != Some(label));
    } else {
        return Ok(()); // already enabled: nothing to write
    }
    m.insert("enabledCssSnippets".into(), Value::Array(list));
    write_appearance(root, &m)
}

// ---------------------------------------------------------------------------
// Part 3 (ledger item 4): the snippet listing + loader — R3.
//
// The oracle (docs/recon-themes/probe-stock-vault.sh §4, T3 RESULT 2):
// top-level `<vault>/.obsidian/snippets/*.css` ONLY. `*.css` is a sh glob, so
// the rule it encodes is: no dotfiles (the glob never matches a leading `.`),
// exact-case `.css` suffix, subdirectories NEVER recursed (a directory —
// even one NAMED `x.css` — is not a snippet), non-.css files ignored. The
// label is the basename minus `.css`. Absent snippets/ directory is SILENT
// NORMAL (T0, T3 RESULT 1): empty list, no message, nothing created.

/// List snippet labels for a vault, sorted bytewise (deterministic — stock's
/// on-screen order was not measured; re-measure if a gate ever compares it).
pub fn list_snippets(root: &Path) -> Vec<String> {
    let dir = root.join(".obsidian").join("snippets");
    let rd = match fs::read_dir(&dir) {
        Ok(r) => r,
        Err(_) => return Vec::new(), // absent = silent normal, never created here
    };
    let mut out: Vec<String> = rd
        .filter_map(|e| e.ok())
        // path().is_file() follows symlinks, as the oracle's `[ -d "$f" ]` does
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .filter_map(|n| n.strip_suffix(".css").map(str::to_string))
        .collect();
    out.sort();
    out
}

/// The one path a label may name: `<vault>/.obsidian/snippets/<label>.css`.
/// A label is a LISTING result, never a path — anything that could traverse
/// (separators, a leading dot, emptiness) is refused before the filesystem
/// sees it, loudly, because a silent skip here would read as "snippet off".
pub fn snippet_path(root: &Path, label: &str) -> Result<PathBuf, String> {
    if label.is_empty()
        || label.starts_with('.')
        || label.contains('/')
        || label.contains('\\')
        || label.contains('\0')
    {
        return Err(format!(
            "snippet label {label:?} refused: not a top-level snippet name"
        ));
    }
    Ok(root
        .join(".obsidian")
        .join("snippets")
        .join(format!("{label}.css")))
}

/// Read + sanitize ONE snippet for injection. Ok = (css, Some(loud message)
/// when mask declarations were stripped — R4X.4). Err = the R6-loud refusal,
/// always naming the FILE and the reason (bad label, unreadable, sanitizer
/// refusal), because stock silently excludes and this loader must not.
pub fn load_snippet(root: &Path, label: &str) -> Result<(String, Option<String>), String> {
    let p = snippet_path(root, label)?;
    let origin = format!("snippet {label}.css");
    let src =
        fs::read_to_string(&p).map_err(|e| format!("{origin}: cannot read: {e}"))?;
    let s = sanitize_css(&src).map_err(|e| format!("{origin}: {e}"))?;
    let msg = (s.stripped > 0).then(|| mask_strip_message(&origin, s.stripped));
    Ok((s.css, msg))
}

// ---------------------------------------------------------------------------
// Part 2 (ledger item 3): the CSS sanitizer — the mask policy, R4X.4.
//
// The brick finding (docs/themecsp/README.md): an unloadable
// `-webkit-mask-image` makes ZERO network requests but silently composites
// the masked subtree to blank — the CSP cannot see it, so the LOADER must.
// Policy (DESIGN §6): any declaration whose property name, after comment
// removal and CSS-escape decoding, case-insensitively is `mask`, starts
// `mask-`, or starts `-webkit-mask`, is STRIPPED and counted; the caller
// surfaces mask_strip_message(). If the scanner cannot tokenize confidently
// (unclosed comment, unterminated string, unbalanced braces/parens, dangling
// escape, mask-shaped prelude) the FILE is REFUSED with Err("cannot parse
// safely: ...") — a parse we are not sure of is a strip we cannot promise.
//
// Scanner notes, each a deliberate decision:
// - comments become ONE SPACE (spec: a comment is a token boundary), so
//   `/*x*/mask-image` is still caught while `ma/**/sk-image` becomes the
//   invalid `ma sk-image` — which no browser applies either.
// - `;` inside parentheses does NOT end a declaration (unquoted
//   `url(data:image/svg+xml;base64,...)` is legal); a brace inside
//   parentheses is refused.
// - strings are opaque: `content: "}; mask: none"` strips nothing.
// - bytes ≥ 0x80 (multibyte UTF-8) never equal an ASCII structural byte, so
//   byte-wise scanning cannot split a code point; segments are cut only at
//   ASCII bytes, so every emitted slice is valid UTF-8.

/// Sanitizer output: the CSS to inject and how many declarations were cut.
#[derive(Debug)]
pub struct Sanitized {
    pub css: String,
    pub stripped: usize,
}

/// DESIGN §6's loud message, in ONE place. `origin` names the file for the
/// user, e.g. `theme Minimal` or `snippet custom-checkbox.css`.
pub fn mask_strip_message(origin: &str, n: usize) -> String {
    format!(
        "{origin}: stripped {n} mask declaration(s) — mask can blank the window (docs/themecsp R4X.4)"
    )
}

fn refuse(reason: &str) -> String {
    format!("cannot parse safely: {reason}")
}

/// The deny predicate, on a DECODED lower-cased property name.
fn is_mask_prop(p: &str) -> bool {
    p == "mask" || p.starts_with("mask-") || p.starts_with("-webkit-mask")
}

/// Decode CSS escapes in a property-name slice (`\6d ask` → `mask`,
/// `m\61 sk` → `mask`). None = an escape we cannot decode confidently
/// (dangling backslash, bad hex, invalid code point) — the caller refuses.
/// Bytes ≥ 0x80 map through as-is; they can never match the ASCII predicate.
fn decode_ident(raw: &[u8]) -> Option<String> {
    let n = raw.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        let c = raw[i];
        if c == b'\\' {
            i += 1;
            if i >= n {
                return None; // dangling escape
            }
            let start = i;
            while i < n && i - start < 6 && raw[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i > start {
                let hex = std::str::from_utf8(&raw[start..i]).ok()?;
                let cp = u32::from_str_radix(hex, 16).ok()?;
                out.push(char::from_u32(cp)?);
                // one whitespace terminates a hex escape and is consumed
                if i < n && matches!(raw[i], b' ' | b'\t' | b'\n' | b'\r') {
                    i += 1;
                }
            } else {
                out.push(raw[i] as char); // literal escape: \m → m
                i += 1;
            }
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    Some(out)
}

/// Property name of a candidate declaration segment: bytes before the first
/// `:`, trimmed, escape-decoded, lower-cased. None if the segment has no `:`.
fn segment_prop(seg: &[u8]) -> Result<Option<String>, String> {
    let ci = match seg.iter().position(|&c| c == b':') {
        Some(ci) => ci,
        None => return Ok(None),
    };
    let mut a = 0;
    let mut b = ci;
    while a < b && seg[a].is_ascii_whitespace() {
        a += 1;
    }
    while b > a && seg[b - 1].is_ascii_whitespace() {
        b -= 1;
    }
    let decoded = decode_ident(&seg[a..b])
        .ok_or_else(|| refuse("undecodable escape in property name"))?;
    Ok(Some(decoded.to_ascii_lowercase()))
}

/// Sanitize one CSS file (theme.css or a snippet). Ok = the bytes to inject
/// plus the strip count (caller raises the loud message when > 0);
/// Err = REFUSE the whole file, loudly, reason inside (DESIGN §6 fail-closed).
pub fn sanitize_css(src: &str) -> Result<Sanitized, String> {
    let b = src.as_bytes();
    let n = b.len();
    let mut out: Vec<u8> = Vec::with_capacity(n);
    let mut seg: Vec<u8> = Vec::new(); // since the last { } or ;
    let mut depth: u32 = 0; // brace depth
    let mut paren: u32 = 0;
    let mut stripped = 0usize;

    // flush a candidate declaration: strip it (count) or emit it verbatim
    let flush = |seg: &mut Vec<u8>,
                 out: &mut Vec<u8>,
                 depth: u32,
                 stripped: &mut usize|
     -> Result<(), String> {
        if depth >= 1 {
            if let Some(p) = segment_prop(seg)? {
                if is_mask_prop(&p) {
                    *stripped += 1;
                    seg.clear();
                    return Ok(());
                }
            }
        }
        out.append(seg);
        Ok(())
    };

    let mut i = 0;
    while i < n {
        let c = b[i];
        match c {
            b'/' if i + 1 < n && b[i + 1] == b'*' => {
                let mut j = i + 2;
                let mut closed = false;
                while j + 1 < n {
                    if b[j] == b'*' && b[j + 1] == b'/' {
                        closed = true;
                        break;
                    }
                    j += 1;
                }
                if !closed {
                    return Err(refuse("unclosed comment"));
                }
                seg.push(b' '); // a comment is a token boundary
                i = j + 2;
            }
            b'"' | b'\'' => {
                seg.push(c);
                let mut j = i + 1;
                loop {
                    if j >= n {
                        return Err(refuse("unterminated string"));
                    }
                    let d = b[j];
                    if d == b'\\' {
                        if j + 1 >= n {
                            return Err(refuse("dangling escape in string"));
                        }
                        seg.push(d);
                        seg.push(b[j + 1]);
                        j += 2;
                        continue;
                    }
                    if d == b'\n' {
                        return Err(refuse("newline inside string"));
                    }
                    seg.push(d);
                    j += 1;
                    if d == c {
                        break;
                    }
                }
                i = j;
            }
            b'(' => {
                paren += 1;
                seg.push(c);
                i += 1;
            }
            b')' => {
                if paren == 0 {
                    return Err(refuse("unbalanced parentheses: stray ')'"));
                }
                paren -= 1;
                seg.push(c);
                i += 1;
            }
            b'{' | b'}' | b';' if paren > 0 => {
                // `;` legally appears in unquoted data: urls; braces do not.
                if c == b';' {
                    seg.push(c);
                    i += 1;
                } else {
                    return Err(refuse("brace inside parentheses"));
                }
            }
            b'{' => {
                // prelude flush. FAIL CLOSED on a mask-shaped prelude
                // (`mask: {`) — nesting ambiguity we will not guess at.
                if let Some(p) = segment_prop(&seg)? {
                    if is_mask_prop(&p) {
                        return Err(refuse("ambiguous 'mask' before '{'"));
                    }
                }
                out.append(&mut seg);
                out.push(b'{');
                depth += 1;
                i += 1;
            }
            b'}' => {
                if depth == 0 {
                    return Err(refuse("unbalanced braces: stray '}'"));
                }
                flush(&mut seg, &mut out, depth, &mut stripped)?; // last decl may lack ';'
                out.push(b'}');
                depth -= 1;
                i += 1;
            }
            b';' => {
                seg.push(b';');
                flush(&mut seg, &mut out, depth, &mut stripped)?;
                i += 1;
            }
            b'\\' => {
                if i + 1 >= n {
                    return Err(refuse("dangling escape"));
                }
                seg.push(c);
                seg.push(b[i + 1]);
                i += 2;
            }
            _ => {
                seg.push(c);
                i += 1;
            }
        }
    }
    if depth != 0 {
        return Err(refuse("unbalanced braces at EOF"));
    }
    if paren != 0 {
        return Err(refuse("unbalanced parentheses at EOF"));
    }
    out.append(&mut seg); // depth-0 tail: whitespace/junk, applies to nothing
    Ok(Sanitized {
        css: String::from_utf8(out).map_err(|_| refuse("output not UTF-8"))?,
        stripped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// the committed stock-written fixture — docs/fixtures/themefs/README.md
    const STOCK: &str = include_str!("../../docs/fixtures/themefs/stock-1.13.7.appearance.json");

    fn tmp_vault(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("rustidian-themefs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        root
    }

    fn bytes(root: &Path) -> String {
        fs::read_to_string(root.join(APPEARANCE_FILE)).unwrap()
    }

    /// the fixture is the log's file: 180 bytes (the T2 log's own size line),
    /// last byte `}` — no trailing newline. Guards fixture corruption; every
    /// byte-wise assertion below stands on this one.
    #[test]
    fn themefs_fixture_matches_the_t2_log() {
        assert_eq!(STOCK.len(), 180, "t2-appearance.log: '--- appearance.json  180 B'");
        assert_eq!(STOCK.as_bytes().last(), Some(&b'}'), "no trailing newline");
        assert!(STOCK.contains("\"cssTheme\": \"T1noauthor\""));
    }

    /// an edit-free read -> write round trip must not change one byte:
    /// layout included (2-space indent, ": " separator, no trailing newline,
    /// key order as stock wrote it).
    #[test]
    fn themefs_edit_free_round_trip_is_byte_identical() {
        let root = tmp_vault("rt");
        fs::write(root.join(APPEARANCE_FILE), STOCK).unwrap();
        let m = read_appearance(&root).unwrap();
        write_appearance(&root, &m).unwrap();
        assert_eq!(bytes(&root), STOCK, "no-edit round trip changed bytes");
        let _ = fs::remove_dir_all(&root);
    }

    /// criterion 2, byte-wise (the r4x_criterion4 pattern): ONE set_css_theme
    /// = the fixture with exactly the cssTheme line replaced. Every key we do
    /// not author — theme, baseFontSize, baseFontSizeAction,
    /// interfaceFontFamily, accentColor — is proved unchanged by byte
    /// equality, not by a checklist.
    #[test]
    fn themefs_criterion2_one_edit_changes_one_line_byte_wise() {
        let root = tmp_vault("c2");
        fs::write(root.join(APPEARANCE_FILE), STOCK).unwrap();
        set_css_theme(&root, "Minimal").unwrap();
        assert_eq!(STOCK.matches("\"cssTheme\": \"T1noauthor\",").count(), 1);
        let want = STOCK.replacen(
            "\"cssTheme\": \"T1noauthor\",",
            "\"cssTheme\": \"Minimal\",",
            1,
        );
        assert_eq!(bytes(&root), want, "one edit changes one line and nothing else");
        // the same fact spelled key by key, so a failure names the loss:
        let v: Value = serde_json::from_str(&bytes(&root)).unwrap();
        let o: Value = serde_json::from_str(STOCK).unwrap();
        for k in ["theme", "baseFontSize", "baseFontSizeAction", "interfaceFontFamily", "accentColor"] {
            assert_eq!(v[k], o[k], "unauthored key {k} must ride through");
        }
        assert_eq!(v["cssTheme"], "Minimal");
        let _ = fs::remove_dir_all(&root);
    }

    /// a TOP-LEVEL key we have never heard of survives our write (the
    /// strictly-more-conservative bmcompat decision, applied here too).
    #[test]
    fn themefs_top_level_unknown_key_survives_a_write() {
        let root = tmp_vault("zz");
        fs::write(
            root.join(APPEARANCE_FILE),
            r#"{"cssTheme": "X", "zzFutureKey": {"v": [1, 2]}}"#,
        )
        .unwrap();
        set_snippet_enabled(&root, "snipAlpha", true).unwrap();
        let v: Value = serde_json::from_str(&bytes(&root)).unwrap();
        assert_eq!(v["zzFutureKey"], serde_json::json!({"v": [1, 2]}), "unknown kept");
        assert_eq!(v["cssTheme"], "X");
        assert_eq!(v["enabledCssSnippets"], serde_json::json!(["snipAlpha"]));
        let _ = fs::remove_dir_all(&root);
    }

    /// ABSENT file = every default, and reads never materialise it (T2: stock
    /// writes it on deviation; T0's `{}` at vault creation is stock's act in
    /// stock's vault, not a duty of ours — DESIGN §2).
    #[test]
    fn themefs_absent_file_reads_default_and_is_not_created() {
        let root = tmp_vault("absent");
        assert_eq!(css_theme(&root), "");
        assert!(enabled_snippets(&root).is_empty());
        assert!(read_appearance(&root).unwrap().is_empty());
        assert!(!root.join(APPEARANCE_FILE).exists(), "a read must not create the file");
        let _ = fs::remove_dir_all(&root);
    }

    /// the first deviation creates the file carrying only that deviation —
    /// the sparse record T2 describes.
    #[test]
    fn themefs_first_deviation_creates_the_sparse_record() {
        let root = tmp_vault("first");
        set_css_theme(&root, "Minimal").unwrap();
        assert_eq!(bytes(&root), "{\n  \"cssTheme\": \"Minimal\"\n}");
        let _ = fs::remove_dir_all(&root);
    }

    /// T3 RESULT 3 verbatim: enable appends (enable order), disable removes
    /// the entry, double-enable is one entry, last disable leaves `[]`.
    #[test]
    fn themefs_snippet_toggle_is_an_ordered_array_of_labels() {
        let root = tmp_vault("snip");
        set_snippet_enabled(&root, "snipAlpha", true).unwrap();
        set_snippet_enabled(&root, "snipBeta", true).unwrap();
        assert_eq!(enabled_snippets(&root), vec!["snipAlpha", "snipBeta"], "enable order");
        set_snippet_enabled(&root, "snipAlpha", true).unwrap();
        assert_eq!(enabled_snippets(&root), vec!["snipAlpha", "snipBeta"], "no dup");
        set_snippet_enabled(&root, "snipAlpha", false).unwrap();
        assert_eq!(enabled_snippets(&root), vec!["snipBeta"], "disable removes the entry");
        set_snippet_enabled(&root, "snipBeta", false).unwrap();
        assert!(enabled_snippets(&root).is_empty());
        let v: Value = serde_json::from_str(&bytes(&root)).unwrap();
        assert_eq!(v["enabledCssSnippets"], serde_json::json!([]), "key kept as []");
        let _ = fs::remove_dir_all(&root);
    }

    /// a file we cannot parse is REFUSED, loudly, and its bytes survive us —
    /// never overwrite what we cannot read.
    #[test]
    fn themefs_unparseable_file_refuses_the_write_and_keeps_the_bytes() {
        let root = tmp_vault("bad");
        fs::write(root.join(APPEARANCE_FILE), "{not json").unwrap();
        let e = set_css_theme(&root, "X").unwrap_err();
        assert!(e.contains("does not parse"), "message names the reason: {e}");
        assert!(set_snippet_enabled(&root, "s", true).is_err());
        assert_eq!(bytes(&root), "{not json", "refusal must not touch the file");
        // an ARRAY file is also not ours to rewrite
        fs::write(root.join(APPEARANCE_FILE), "[1,2]").unwrap();
        assert!(set_css_theme(&root, "X").unwrap_err().contains("not a JSON object"));
        assert_eq!(bytes(&root), "[1,2]");
        let _ = fs::remove_dir_all(&root);
    }

    // ---- part 2: the sanitizer (R4X.4) ------------------------------------

    fn ok(src: &str) -> Sanitized {
        sanitize_css(src).expect("expected Ok")
    }
    fn err(src: &str) -> String {
        sanitize_css(src).expect_err("expected refusal")
    }

    #[test]
    fn themefs_sanitize_clean_css_is_untouched_and_idempotent() {
        let src = ".a { color: red; background: url(\"x.png\") }\n.b:hover { border: 1px }\n";
        let s = ok(src);
        assert_eq!(s.stripped, 0);
        assert_eq!(s.css, src, "no mask, no comments: byte-identical");
        let again = ok(&s.css);
        assert_eq!(again.stripped, 0);
        assert_eq!(again.css, s.css, "sanitizer must be idempotent");
    }

    #[test]
    fn themefs_sanitize_strips_the_deny_family() {
        // shorthand, longhand, -webkit- prefixed shorthand + longhand
        let s = ok(".a { color: red; mask: url(x); mask-image: url(x); -webkit-mask: url(x); -webkit-mask-image: url(x); font-size: 1em }");
        assert_eq!(s.stripped, 4);
        assert!(!s.css.to_ascii_lowercase().contains("mask"));
        assert!(s.css.contains("color: red;") && s.css.contains("font-size: 1em"));
        // the mask- prefix family (the brick needs no image longhand)
        let s = ok(".a { mask-position: 0 0; mask-composite: add; -webkit-mask-box-image: url(x) }");
        assert_eq!(s.stripped, 3);
        assert!(!s.css.to_ascii_lowercase().contains("mask"));
    }

    #[test]
    fn themefs_sanitize_evasion_case_and_whitespace() {
        let s = ok(".a { MASK-IMAGE: url(x); -WebKit-Mask: url(x); color: red }");
        assert_eq!(s.stripped, 2);
        let s = ok(".a { mask \n : url(x); color: red }");
        assert_eq!(s.stripped, 1);
        assert!(s.css.contains("color: red"));
    }

    #[test]
    fn themefs_sanitize_evasion_css_escapes_are_decoded() {
        // \6d = 'm', \61 = 'a' — WebKit decodes these in property names
        let s = ok(".a { \\6d ask-image: url(x); color: red }");
        assert_eq!(s.stripped, 1, "hex-escaped first letter");
        let s = ok(".a { m\\61 sk: url(x); color: red }");
        assert_eq!(s.stripped, 1, "hex escape mid-name");
        let s = ok(".a { \\4D ASK: url(x) }"); // uppercase hex, uppercase rest
        assert_eq!(s.stripped, 1);
        let s = ok(".a { m\\ask: url(x) }"); // literal escape \a? no: \a is hex
        // `\a` IS a hex digit escape (LF) → 'm' + LF + "sk" — not mask, and no
        // browser applies it either. The decode must not panic; strip count 0.
        assert_eq!(s.stripped, 0);
        let s = ok(".a { \\6D\\61\\73\\6B: url(x) }"); // fully hex-escaped "mask"
        assert_eq!(s.stripped, 1);
    }

    #[test]
    fn themefs_sanitize_evasion_comments_in_and_before_the_name() {
        // comment BEFORE the name: browsers apply it → we must catch it
        let s = ok(".a { /*x*/mask-image: url(x); color: red }");
        assert_eq!(s.stripped, 1);
        // comment INSIDE the name: a token boundary — browsers refuse the
        // declaration, we emit the equally-inert `ma sk-image`
        let s = ok(".a { ma/**/sk-image: url(x); color: red }");
        assert!(!s.css.to_ascii_lowercase().contains("mask"));
        assert!(s.css.contains("color: red"));
    }

    #[test]
    fn themefs_sanitize_semicolon_in_unquoted_url_does_not_split() {
        let s = ok(".a { background: url(data:image/svg+xml;base64,AA); mask: none; color: red }");
        assert_eq!(s.stripped, 1);
        assert!(s.css.contains("url(data:image/svg+xml;base64,AA)"), "data: url intact");
        assert!(s.css.contains("color: red"));
    }

    #[test]
    fn themefs_sanitize_strings_are_opaque() {
        let s = ok(".a { content: \"}; mask: none\"; color: red }");
        assert_eq!(s.stripped, 0, "mask inside a string is data, not a declaration");
        assert!(s.css.contains("\"}; mask: none\""));
        let s = ok(".a { content: \"/*\"; mask: none; color: red }");
        assert_eq!(s.stripped, 1, "quote does not open a comment");
        assert!(s.css.contains("\"/*\""));
    }

    #[test]
    fn themefs_sanitize_near_names_are_kept() {
        // custom properties and lookalikes are NOT the deny family
        let s = ok(".a { --mask-color: red; masking: 1; unmask: 2; --webkit-mask: x }");
        assert_eq!(s.stripped, 0);
        assert!(s.css.contains("--mask-color: red"));
    }

    #[test]
    fn themefs_sanitize_nested_blocks_and_media() {
        let s = ok("@media (max-width: 100px) { .a { mask: url(x); color: red } }");
        assert_eq!(s.stripped, 1);
        assert!(s.css.contains("@media (max-width: 100px)"));
        assert!(s.css.contains("color: red"));
    }

    #[test]
    fn themefs_sanitize_refuses_what_it_cannot_tokenize() {
        // DESIGN §6 fail-closed: each broken shape is a refusal, not a guess
        assert!(err(".a { /* x").contains("unclosed comment"));
        assert!(err(".a { content: \"x }").contains("unterminated string"));
        assert!(err("} .a { color: red }").contains("stray '}'"));
        assert!(err(".a { color: red;").contains("unbalanced braces at EOF"));
        assert!(err(".a { background: url(x }").contains("brace inside parentheses"));
        assert!(err(".a { color: red\\").contains("dangling escape"));
        assert!(err(".x { mask: { } }").contains("ambiguous 'mask' before '{'"));
        // every refusal wears the DESIGN §6 prefix the caller shows the user
        for s in [".a { /* x", "} x", ".a { color: red;"] {
            assert!(err(s).starts_with("cannot parse safely: "), "prefix on {s:?}");
        }
    }

    #[test]
    fn themefs_sanitize_mask_message_is_the_design_string() {
        assert_eq!(
            mask_strip_message("theme Minimal", 2),
            "theme Minimal: stripped 2 mask declaration(s) — mask can blank the window (docs/themecsp R4X.4)"
        );
    }

    // ---- Part 3: snippet listing + loader (item 4, T3 semantics) ----------

    #[test]
    fn themefs_snips_top_level_exact_case_css_files_only() {
        let root = tmp_vault("sniplist");
        let sd = root.join(".obsidian").join("snippets");
        fs::create_dir_all(sd.join("sub")).unwrap(); // subdirectory: never recursed
        fs::create_dir_all(sd.join("dir.css")).unwrap(); // a DIRECTORY named x.css is not a snippet
        fs::write(sd.join("zeta.css"), ".z{}").unwrap();
        fs::write(sd.join("alpha.css"), ".a{}").unwrap();
        fs::write(sd.join("note.txt"), "not css").unwrap(); // non-.css ignored
        fs::write(sd.join(".hidden.css"), ".h{}").unwrap(); // dotfile: the oracle's glob skips it
        fs::write(sd.join("Upper.CSS"), ".u{}").unwrap(); // exact-case suffix only (sh glob)
        fs::write(sd.join("sub").join("nested.css"), ".n{}").unwrap(); // T3 RESULT 2: ignored
        assert_eq!(list_snippets(&root), vec!["alpha".to_string(), "zeta".to_string()]);
    }

    #[test]
    fn themefs_snips_absent_dir_is_silent_normal() {
        let root = tmp_vault("snipabsent");
        assert!(list_snippets(&root).is_empty());
        // T0: listing must not CREATE the directory stock never creates
        assert!(!root.join(".obsidian").join("snippets").exists());
    }

    #[test]
    fn themefs_snippet_path_refuses_anything_that_is_not_a_listing_label() {
        let root = tmp_vault("snippath");
        for bad in ["", "..", ".hidden", "a/b", "a\\b", "../evil", "a\0b"] {
            let e = snippet_path(&root, bad).unwrap_err();
            assert!(e.contains("refused"), "label {bad:?} must be refused, got {e:?}");
        }
        let p = snippet_path(&root, "custom-checkbox").unwrap();
        assert!(p.ends_with(".obsidian/snippets/custom-checkbox.css"));
    }

    #[test]
    fn themefs_load_snippet_strips_mask_loudly() {
        let root = tmp_vault("snipload");
        let sd = root.join(".obsidian").join("snippets");
        fs::create_dir_all(&sd).unwrap();
        fs::write(sd.join("m.css"), ".a { color: red; -webkit-mask-image: url(x) }").unwrap();
        let (css, msg) = load_snippet(&root, "m").unwrap();
        assert!(css.contains("color: red"));
        assert!(!css.to_lowercase().contains("mask"));
        assert_eq!(msg, Some(mask_strip_message("snippet m.css", 1)));
    }

    #[test]
    fn themefs_load_snippet_refusals_name_the_file() {
        let root = tmp_vault("sniprefuse");
        let sd = root.join(".obsidian").join("snippets");
        fs::create_dir_all(&sd).unwrap();
        fs::write(sd.join("broken.css"), ".a { /* unclosed").unwrap();
        let e = load_snippet(&root, "broken").unwrap_err();
        assert!(e.starts_with("snippet broken.css: cannot parse safely:"), "got {e:?}");
        let e2 = load_snippet(&root, "ghost").unwrap_err();
        assert!(e2.starts_with("snippet ghost.css: cannot read:"), "got {e2:?}");
        // a clean file carries NO message — silence is the no-mask signal
        fs::write(sd.join("ok.css"), ".a { color: blue }").unwrap();
        assert_eq!(load_snippet(&root, "ok").unwrap().1, None);
    }
}
