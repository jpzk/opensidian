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
use std::path::Path;

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
}
