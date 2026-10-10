// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* goal qscreate: where the quick switcher creates the note it was asked for.
   Stock 1.13.7 black-box recon (/workspace/goal/qscreate/notes/stock.md, runs
   r1..r5), a drop-in contract, so every rule below is an OBSERVATION:
   - the typed text is cut at the first '#' (link-subpath semantics; '|' kept)
     and trimmed; nothing left = nothing created.
   - '\' or ':' anywhere -> refused with stock's notice, no file.
   - a query with '/' is a VAULT-ROOT-relative path (folders are created),
     whatever newFileLocation says (r3/u02, r2/f02).
   - otherwise the folder comes from <vault>/.obsidian/app.json:
       newFileLocation absent | "root"   -> vault root          (r5, r4)
       "current"                          -> folder of the ACTIVE note (r3)
       "folder" + newFileFolderPath       -> that folder if it EXISTS,
                                             else the vault root, never mkdir'd (r2z, r2)
   app.json is READ only: stock left it byte-identical (unknown keys
   included) in every run, and so do we. */
use std::path::Path;

pub const NOTICE_ILLEGAL: &str = "File name cannot contain any of the following characters: \\ / :";

#[derive(Debug, PartialEq, Eq)]
pub enum QsName {
    /// vault-relative note name, no `.md` (`Projects/Foo`)
    Create(String),
    /// refused, with the notice stock shows
    Refused(&'static str),
    /// nothing left to create after the '#' cut
    Empty,
}

/// the stock rules over an already-parsed app.json and a folder probe.
pub fn resolve(cfg: &serde_json::Value, query: &str, active: Option<&str>, dir_exists: &dyn Fn(&str) -> bool) -> QsName {
    let q = query.split('#').next().unwrap_or("").trim();
    if q.is_empty() {
        return QsName::Empty;
    }
    if q.contains('\\') || q.contains(':') {
        return QsName::Refused(NOTICE_ILLEGAL);
    }
    if q.contains('/') {
        let parts: Vec<&str> = q.split('/').map(str::trim).collect();
        if parts.iter().any(|p| p.is_empty()) {
            return QsName::Refused(NOTICE_ILLEGAL);   // "a//b", "/a", "a/": no empty path segment
        }
        return QsName::Create(parts.join("/"));
    }
    let dir = match cfg.get("newFileLocation").and_then(|v| v.as_str()) {
        Some("current") => active
            .and_then(|a| a.rsplit_once('/').map(|(d, _)| d.to_string()))
            .unwrap_or_default(),
        Some("folder") => {
            let f = cfg.get("newFileFolderPath").and_then(|v| v.as_str()).unwrap_or("").trim().trim_matches('/');
            if !f.is_empty() && dir_exists(f) { f.to_string() } else { String::new() }
        }
        _ => String::new(),
    };
    QsName::Create(if dir.is_empty() { q.to_string() } else { format!("{dir}/{q}") })
}

/// resolve against a vault on disk: app.json read (never written), folders probed
/// without following a symlink out of the vault.
pub fn resolve_in(root: &Path, query: &str, active: Option<&str>) -> QsName {
    let cfg = std::fs::read_to_string(root.join(".obsidian/app.json")).unwrap_or_default();
    let cfg = serde_json::from_str::<serde_json::Value>(&cfg).unwrap_or(serde_json::Value::Null);
    let croot = root.canonicalize().ok();
    let probe = |f: &str| {
        if f.split('/').any(|c| c.is_empty() || c.starts_with('.')) {
            return false;
        }
        let p = root.join(f);
        match (p.canonicalize(), &croot) {
            (Ok(c), Some(r)) => c.starts_with(r) && c.is_dir(),
            _ => false,
        }
    };
    resolve(&cfg, query, active, &probe)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn r(cfg: serde_json::Value, q: &str, a: Option<&str>) -> QsName {
        resolve(&cfg, q, a, &|f| f == "Zettel" || f == "deep/box")
    }
    fn c(s: &str) -> QsName { QsName::Create(s.into()) }

    #[test]
    fn default_and_root_create_at_the_vault_root() {   // r5 v01/v03, r4 t01
        assert_eq!(r(json!({}), "Zqx Def", Some("Projects/Rustidian")), c("Zqx Def"));
        assert_eq!(r(serde_json::Value::Null, "Zqx Def", Some("Projects/Rustidian")), c("Zqx Def"));
        assert_eq!(r(json!({"newFileLocation":"root"}), "Zqx Root", Some("Projects/Rustidian")), c("Zqx Root"));
        assert_eq!(r(json!({"newFileLocation":"bogus"}), "N", Some("P/A")), c("N"));
    }
    #[test]
    fn current_uses_the_active_notes_folder() {         // r3 u01/u04
        let cur = json!({"newFileLocation":"current"});
        assert_eq!(r(cur.clone(), "Zqx Cur", Some("Projects/Rustidian")), c("Projects/Zqx Cur"));
        assert_eq!(r(cur.clone(), "Zqx CurRoot", Some("Welcome")), c("Zqx CurRoot"));
        assert_eq!(r(cur.clone(), "N", Some("a/b/C")), c("a/b/N"));
        assert_eq!(r(cur, "N", None), c("N"));
    }
    #[test]
    fn folder_mode_needs_an_existing_folder() {          // r2z z01, r2 f01
        assert_eq!(r(json!({"newFileLocation":"folder","newFileFolderPath":"Zettel"}), "Zqx Zet", None), c("Zettel/Zqx Zet"));
        assert_eq!(r(json!({"newFileLocation":"folder","newFileFolderPath":"/deep/box/"}), "N", None), c("deep/box/N"));
        assert_eq!(r(json!({"newFileLocation":"folder","newFileFolderPath":"Inbox"}), "Zqx Folder", None), c("Zqx Folder"));
        assert_eq!(r(json!({"newFileLocation":"folder"}), "N", None), c("N"));
    }
    #[test]
    fn a_slash_query_is_vault_root_relative() {          // r3 u02, r2 f02, r1 c10
        assert_eq!(r(json!({"newFileLocation":"current"}), "zqc/Zqx CPath", Some("Projects/Rustidian")), c("zqc/Zqx CPath"));
        assert_eq!(r(json!({"newFileLocation":"folder","newFileFolderPath":"Zettel"}), "zqp/Zqx FPath", None), c("zqp/Zqx FPath"));
        assert_eq!(r(json!({}), "zqdir/sub/Zqx Path", None), c("zqdir/sub/Zqx Path"));
        assert_eq!(r(json!({}), "/lead", None), QsName::Refused(NOTICE_ILLEGAL));
        assert_eq!(r(json!({}), "a//b", None), QsName::Refused(NOTICE_ILLEGAL));
    }
    #[test]
    fn hash_cut_and_illegal_chars() {                     // r1 c11/c12
        assert_eq!(r(json!({}), "Zqx pipe|hash#x", None), c("Zqx pipe|hash"));
        assert_eq!(r(json!({}), "Zqx bad:na*me?", None), QsName::Refused(NOTICE_ILLEGAL));
        assert_eq!(r(json!({}), "back\\slash", None), QsName::Refused(NOTICE_ILLEGAL));
        assert_eq!(r(json!({}), "#only", None), QsName::Empty);
        assert_eq!(r(json!({}), "   ", None), QsName::Empty);
        assert_eq!(r(json!({}), "  Trim me  ", None), c("Trim me"));
    }
    #[test]
    fn on_disk_app_json_is_read_and_left_byte_identical() {
        let root = std::env::temp_dir().join(format!("qscreate-ut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::create_dir_all(root.join("Zettel")).unwrap();
        let body = r#"{"newFileLocation":"folder","newFileFolderPath":"Zettel","zqKeep":1}"#;
        std::fs::write(root.join(".obsidian/app.json"), body).unwrap();
        assert_eq!(resolve_in(&root, "N", None), c("Zettel/N"));
        std::fs::write(root.join(".obsidian/app.json"), r#"{"newFileLocation":"folder","newFileFolderPath":"Inbox","zqKeep":1}"#).unwrap();
        assert_eq!(resolve_in(&root, "N", None), c("N"));
        assert!(!root.join("Inbox").exists(), "a missing folder is never created");
        std::fs::write(root.join(".obsidian/app.json"), r#"{"newFileLocation":"folder","newFileFolderPath":".obsidian"}"#).unwrap();
        assert_eq!(resolve_in(&root, "N", None), c("N"), "a dot folder is not a note folder");
        std::fs::write(root.join(".obsidian/app.json"), body).unwrap();
        let _ = resolve_in(&root, "M", Some("Zettel/X"));
        assert_eq!(std::fs::read_to_string(root.join(".obsidian/app.json")).unwrap(), body, "app.json untouched");
        let _ = std::fs::remove_dir_all(&root);
    }
}
