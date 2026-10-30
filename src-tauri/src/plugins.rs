// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* plugin-loader (recon M1): the opensidian-plugin:// scheme.

   A community plugin's main.js is ONE CommonJS bundle. The page CSP has no
   'unsafe-eval' and no blob:, so the bundle cannot be eval'd from a string;
   instead Rust serves it on its own scheme, wrapped in a CJS function that
   hands the host's module/exports/require to it, and ONLY this scheme is
   added to script-src.

   URL shape:  opensidian-plugin://localhost/<id>/main.js[?n=<reload>]
   (the id rides in the PATH, not the host: a host is case-normalised by some
   URL parsers, a path segment is not.)

   What the scheme serves, and nothing else (every other answer is 403):
     1. the path is exactly "/<id>/main.js" — no other file, no other depth,
        no percent-escapes, so there is no path to "parse" into an escape;
     2. <id> is a plain name ([A-Za-z0-9._-], no leading dot, <= 128 bytes);
     3. plugins are TRUSTED for this vault (restricted mode OFF — that flag
        lives in ~/.opensidian.json, outside the vault, so a vault cannot ship
        itself pre-trusted) AND <id> is listed in the vault's
        .obsidian/community-plugins.json (Obsidian's enabled list);
     4. <vault>/.obsidian/plugins/<id> canonicalises to exactly
        canon(<vault>)/.obsidian/plugins/<id> — a symlinked plugin dir, or a
        symlinked .obsidian / plugins dir pointing out of the vault, refuses;
     5. main.js is a regular file, not a symlink, <= MAX_MAIN_JS bytes.
   A missing main.js in an otherwise allowed plugin is 404. */

use std::fs;
use std::path::{Path, PathBuf};

pub const SCHEME: &str = "opensidian-plugin";
/// the biggest real bundles (excalidraw) are ~8 MB; anything past this is not a plugin
pub const MAX_MAIN_JS: u64 = 32 * 1024 * 1024;
/// the JS host function the wrapper calls (ui/plugins/loader.js defines it)
pub const DEFINE_FN: &str = "__osPluginDefine";

#[derive(Debug, PartialEq, Eq)]
pub enum Served {
    Js(Vec<u8>),
    Refused(&'static str),
    Missing,
}

/// rule 2: a plugin id is a plain directory name
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// rule 1: "/<id>/main.js" -> Some(id); anything else -> None
pub fn parse_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix('/')?;
    let (id, file) = rest.split_once('/')?;
    (file == "main.js" && valid_id(id)).then_some(id)
}

/// the vault's enabled list: .obsidian/community-plugins.json, a JSON array of ids
/// (Obsidian's format). Unreadable / malformed = nothing enabled.
pub fn enabled_ids(root: &Path) -> Vec<String> {
    let p = root.join(".obsidian").join("community-plugins.json");
    let Ok(m) = fs::symlink_metadata(&p) else { return Vec::new() };
    if !m.is_file() || m.len() > 1024 * 1024 {
        return Vec::new();
    }
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
        .map(|v| v.iter().filter_map(|x| x.as_str()).filter(|s| valid_id(s)).map(str::to_string).collect())
        .unwrap_or_default()
}

/// rule 4: the plugin dir, canonical and confined; None = refuse
pub fn plugin_dir_in(root: &Path, id: &str) -> Option<PathBuf> {
    if !valid_id(id) {
        return None;
    }
    let croot = root.canonicalize().ok()?;
    let want = croot.join(".obsidian").join("plugins").join(id);
    let got = root.join(".obsidian").join("plugins").join(id).canonicalize().ok()?;
    (got == want && got.starts_with(&croot) && got.is_dir()).then_some(got)
}

/// rule 5: a file inside a confined plugin dir. Ok(None) = absent, Err = refuse
pub fn plugin_file_in(root: &Path, id: &str, file: &str, cap: u64) -> Result<Option<PathBuf>, &'static str> {
    let dir = plugin_dir_in(root, id).ok_or("plugin dir not confined")?;
    if file.is_empty() || file.contains('/') || file.starts_with('.') {
        return Err("bad file name");
    }
    let p = dir.join(file);
    match fs::symlink_metadata(&p) {
        Err(_) => Ok(None),
        Ok(m) if m.is_symlink() => Err("symlinked file"),
        Ok(m) if !m.is_file() => Err("not a file"),
        Ok(m) if m.len() > cap => Err("too big"),
        Ok(_) => Ok(Some(p)),
    }
}

/// cp-unrestricted: the CommonJS / Electron-renderer free variables every plugin
/// bundle may read at module scope, bound as factory parameters after
/// module,exports,require. loader.js fills them from ui/plugins/node-desktop.js
/// (one view per plugin), so nothing is assigned onto window.
pub const FREE_VARS: &str = "process,global,Buffer,Error,__filename,__dirname,SharedArrayBuffer";

/// the CJS wrapper: module/exports/require come from the host's define fn.
/// The plugin source starts the function body, so its own "use strict" stays a
/// directive. The trailing newline before "})" survives a final // comment.
pub fn wrap_cjs(id: &str, src: &str) -> String {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    format!(
        ";(function(){{var d=window.{DEFINE_FN};if(typeof d!==\"function\")return;d({id:?},function(module,exports,require,{FREE_VARS}){{{src}\n}});}})();\n//# sourceURL={SCHEME}://localhost/{id}/main.js\n"
    )
}

/// the whole decision for one request (rules 1-5), unit-testable without tauri
pub fn serve(root: Option<&Path>, path: &str, trusted: bool) -> Served {
    let Some(id) = parse_path(path) else { return Served::Refused("bad path") };
    let Some(root) = root else { return Served::Refused("no vault") };
    if !trusted {
        return Served::Refused("restricted mode");
    }
    if !enabled_ids(root).iter().any(|e| e == id) {
        return Served::Refused("not enabled");
    }
    match plugin_file_in(root, id, "main.js", MAX_MAIN_JS) {
        Err(why) => Served::Refused(why),
        Ok(None) => Served::Missing,
        Ok(Some(p)) => match fs::read(&p) {
            Ok(b) => Served::Js(wrap_cjs(id, &String::from_utf8_lossy(&b)).into_bytes()),
            Err(_) => Served::Missing,
        },
    }
}

/// trust is keyed by the CANONICAL vault path in ~/.opensidian.json
/// ("plugins_trusted": { "<canon path>": true }); absent = restricted.
pub fn trust_key(root: &Path) -> String {
    fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()).display().to_string()
}

pub fn is_trusted(cfg: &serde_json::Value, root: &Path) -> bool {
    cfg["plugins_trusted"][trust_key(root).as_str()].as_bool() == Some(true)
}

// ---------------------------------------------------------------------------
// ledger item 3: what ui/plugins/loader.js needs from the backend. Three thin
// commands in main.rs (plugins_state / plugins_set / plugin_io) over these.
// Every path below goes through plugin_dir_in (rule 4) and every write is a
// vault-dir-fd relative atomic replace (vaultfs::write_atomic: a symlinked
// .obsidian is refused, the replace is O_EXCL temp + renameat).

/// the ~/.opensidian.json key holding { "<canon vault path>": bool }
pub const TRUST_KEY: &str = "plugins_trusted";
pub const ENABLED_FILE: &str = ".obsidian/community-plugins.json";
/// a plugin's data.json past this is refused, read or write
pub const MAX_DATA_JSON: u64 = 16 * 1024 * 1024;
pub const MAX_STYLES: u64 = 4 * 1024 * 1024;
const MAX_MANIFEST: u64 = 256 * 1024;

/// one installed plugin: its dir is confined and manifest.json parses as an
/// object. The id is the DIRECTORY name (what the scheme serves); the manifest's
/// own "id" rides along in `manifest` and a mismatch is reported, not fatal.
pub fn installed(root: &Path) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(root.join(".obsidian").join("plugins")) else { return out };
    let mut names: Vec<String> =
        rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).filter(|n| valid_id(n)).collect();
    names.sort();
    for id in names {
        let Ok(Some(mp)) = plugin_file_in(root, &id, "manifest.json", MAX_MANIFEST) else { continue };
        let Some(manifest) = fs::read_to_string(&mp)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .filter(|v| v.is_object())
        else {
            continue;
        };
        let has = |f: &str, cap| matches!(plugin_file_in(root, &id, f, cap), Ok(Some(_)));
        out.push(serde_json::json!({
            "id": id,
            "manifest": manifest,
            "hasMain": has("main.js", MAX_MAIN_JS),
            "hasStyles": has("styles.css", MAX_STYLES),
        }));
    }
    out
}

/// enable/disable one id in community-plugins.json (Obsidian's array). Order is
/// kept, other entries (even ones we cannot parse as ids) round-trip. A file
/// that exists but is not a JSON array is refused, never overwritten.
pub fn set_enabled(root: &Path, id: &str, on: bool) -> Result<Vec<String>, String> {
    if !valid_id(id) {
        return Err("bad plugin id".into());
    }
    let p = root.join(ENABLED_FILE);
    let mut list: Vec<serde_json::Value> = match fs::symlink_metadata(&p) {
        Err(_) => Vec::new(),
        Ok(m) if !m.is_file() || m.len() > 1024 * 1024 => return Err(format!("{ENABLED_FILE}: not a plain file")),
        Ok(_) => {
            let s = fs::read_to_string(&p).map_err(|e| format!("{ENABLED_FILE}: {e}"))?;
            serde_json::from_str(&s).map_err(|_| format!("{ENABLED_FILE}: not a JSON array, left untouched"))?
        }
    };
    let present = list.iter().any(|v| v.as_str() == Some(id));
    if on && !present {
        list.push(serde_json::Value::String(id.into()));
    } else if !on && present {
        list.retain(|v| v.as_str() != Some(id));
    } else {
        return Ok(enabled_ids(root));
    }
    let body = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    crate::vaultfs::write_atomic(root, Path::new(ENABLED_FILE), body.as_bytes())
        .map_err(|e| format!("{ENABLED_FILE}: {e}"))?;
    Ok(enabled_ids(root))
}

/// the vault-relative path of a confined plugin file, for vaultfs writes
fn rel_in(root: &Path, id: &str, file: &str) -> Result<PathBuf, String> {
    plugin_dir_in(root, id).ok_or("plugin dir not confined")?;
    Ok(Path::new(".obsidian").join("plugins").join(id).join(file))
}

/// Plugin.loadData(): data.json parsed, or Null when absent or not valid JSON (Obsidian resolves null for both, so a
/// plugin falls back to its defaults; the file is left as is)
pub fn read_data(root: &Path, id: &str) -> Result<serde_json::Value, String> {
    match plugin_file_in(root, id, "data.json", MAX_DATA_JSON)? {
        None => Ok(serde_json::Value::Null),
        Some(p) => {
            // bytes, not a String: a file cut inside a multi-byte character is invalid JSON (null), not a read error
            let b = fs::read(&p).map_err(|e| format!("{id}/data.json: {e}"))?;
            Ok(serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null))
        }
    }
}

/// Plugin.saveData(): pretty JSON (Obsidian writes 2-space), atomic, confined. A data.json that is not valid JSON
/// (loadData gave the plugin null, so it is saving its defaults) is copied to data.json.invalid first, so the save
/// does not destroy the settings the user's broken edit still holds
pub fn write_data(root: &Path, id: &str, data: &serde_json::Value) -> Result<(), String> {
    // a symlinked data.json refuses
    if let Some(p) = plugin_file_in(root, id, "data.json", MAX_DATA_JSON)? {
        let old = fs::read(&p).map_err(|e| format!("{id}/data.json: {e}"))?;
        if serde_json::from_slice::<serde_json::Value>(&old).is_err() {
            crate::vaultfs::write_atomic(root, &rel_in(root, id, "data.json.invalid")?, &old)
                .map_err(|e| format!("{id}/data.json.invalid: {e}"))?;
            eprintln!("[plugins] {id}/data.json was not valid JSON: kept as data.json.invalid before the save");
        }
    }
    let body = serde_json::to_string_pretty(data).map_err(|e| e.to_string())?;
    if body.len() as u64 > MAX_DATA_JSON {
        return Err(format!("{id}/data.json: too big"));
    }
    crate::vaultfs::write_atomic(root, &rel_in(root, id, "data.json")?, body.as_bytes())
        .map_err(|e| format!("{id}/data.json: {e}"))
}

/// a plugin's styles.css through the snippet sanitizer; None = no styles.css
pub fn read_styles(root: &Path, id: &str) -> Result<Option<String>, String> {
    match plugin_file_in(root, id, "styles.css", MAX_STYLES)? {
        None => Ok(None),
        Some(p) => {
            let src = fs::read_to_string(&p).map_err(|e| format!("{id}/styles.css: {e}"))?;
            crate::themefs::sanitize_css(&src).map(|s| Some(s.css)).map_err(|e| format!("{id}/styles.css: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("opensidian-plug-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".obsidian/plugins/good")).unwrap();
        fs::write(root.join(".obsidian/plugins/good/main.js"), "\"use strict\";module.exports=1;//end").unwrap();
        fs::write(root.join(".obsidian/community-plugins.json"), r#"["good","gone","sneaky","linkdir"]"#).unwrap();
        fs::write(root.join("secret.md"), "SECRET").unwrap();
        root
    }

    fn js(s: Served) -> String {
        match s {
            Served::Js(b) => String::from_utf8(b).unwrap(),
            o => panic!("expected js, got {o:?}"),
        }
    }

    #[test]
    fn serves_enabled_main_js_wrapped() {
        let r = vault("ok");
        let out = js(serve(Some(&r), "/good/main.js", true));
        assert!(out.starts_with(";(function(){var d=window.__osPluginDefine;"), "{out}");
        assert!(out.contains("d(\"good\",function(module,exports,require,process,global,Buffer,Error,__filename,__dirname,SharedArrayBuffer){\"use strict\";module.exports=1;//end\n});"), "{out}");
        assert!(out.ends_with("//# sourceURL=opensidian-plugin://localhost/good/main.js\n"), "{out}");
    }

    #[test]
    fn restricted_mode_serves_nothing() {
        let r = vault("restr");
        assert_eq!(serve(Some(&r), "/good/main.js", false), Served::Refused("restricted mode"));
    }

    #[test]
    fn not_enabled_is_refused_and_missing_is_404() {
        let r = vault("en");
        fs::create_dir_all(r.join(".obsidian/plugins/off")).unwrap();
        fs::write(r.join(".obsidian/plugins/off/main.js"), "x").unwrap();
        assert_eq!(serve(Some(&r), "/off/main.js", true), Served::Refused("not enabled"));
        fs::create_dir_all(r.join(".obsidian/plugins/gone")).unwrap();
        assert_eq!(serve(Some(&r), "/gone/main.js", true), Served::Missing);
        assert_eq!(serve(None, "/good/main.js", true), Served::Refused("no vault"));
    }

    #[test]
    fn path_escapes_are_refused() {
        let r = vault("esc");
        for p in [
            "/../secret.md", "/good/../../../secret.md", "/good/../main.js", "/../good/main.js",
            "/good/manifest.json", "/good/data.json", "/good/sub/main.js", "/good//main.js",
            "/%2e%2e/main.js", "/good%2fmain.js", "/.obsidian/main.js", "/./main.js", "/../../etc/passwd",
            "//etc/passwd", "/etc/passwd", "good/main.js", "", "/", "/good/", "/good/main.js/",
            "/good/main.js%00", "/go od/main.js", "/good\\..\\x/main.js",
        ] {
            match serve(Some(&r), p, true) {
                Served::Refused(_) => {}
                o => panic!("{p:?} must be refused, got {o:?}"),
            }
        }
        assert_eq!(parse_path("/good/main.js"), Some("good"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_out_are_refused() {
        use std::os::unix::fs::symlink;
        let r = vault("sym");
        let outside = r.with_extension("outside");
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("main.js"), "OUTSIDE").unwrap();
        // a plugin dir that is a symlink out of the vault
        symlink(&outside, r.join(".obsidian/plugins/linkdir")).unwrap();
        assert!(matches!(serve(Some(&r), "/linkdir/main.js", true), Served::Refused(_)));
        // a main.js that is a symlink (to a vault file, even)
        fs::create_dir_all(r.join(".obsidian/plugins/sneaky")).unwrap();
        symlink(r.join("secret.md"), r.join(".obsidian/plugins/sneaky/main.js")).unwrap();
        assert!(matches!(serve(Some(&r), "/sneaky/main.js", true), Served::Refused(_)));
        // a whole .obsidian/plugins that points out of the vault
        let r2 = vault("sym2");
        fs::remove_dir_all(r2.join(".obsidian/plugins")).unwrap();
        fs::create_dir_all(outside.join("good")).unwrap();
        fs::write(outside.join("good/main.js"), "OUTSIDE").unwrap();
        symlink(&outside, r2.join(".obsidian/plugins")).unwrap();
        assert!(matches!(serve(Some(&r2), "/good/main.js", true), Served::Refused(_)));
    }

    #[test]
    fn oversize_and_non_file_main_js_refused() {
        let r = vault("big");
        fs::create_dir_all(r.join(".obsidian/plugins/sneaky/main.js")).unwrap(); // a dir named main.js
        assert!(matches!(serve(Some(&r), "/sneaky/main.js", true), Served::Refused(_)));
        assert!(matches!(plugin_file_in(&r, "good", "main.js", 3), Err("too big")));
    }

    #[test]
    fn ids_and_enabled_list() {
        for ok in ["sample-plugin", "periodic-notes", "cmdr", "obsidian_x.y", "A1"] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", ".", "..", ".hidden", "a/b", "a\\b", "a b", "a%2e", "é", &"x".repeat(129)] {
            assert!(!valid_id(bad), "{bad}");
        }
        let r = vault("list");
        fs::write(r.join(".obsidian/community-plugins.json"), r#"["good", "../x", 3, "ok-2"]"#).unwrap();
        assert_eq!(enabled_ids(&r), vec!["good".to_string(), "ok-2".to_string()]);
        fs::write(r.join(".obsidian/community-plugins.json"), "not json").unwrap();
        assert!(enabled_ids(&r).is_empty());
    }

    #[test]
    fn trust_lives_outside_the_vault_and_defaults_off() {
        let r = vault("trust");
        assert!(!is_trusted(&serde_json::json!({}), &r));
        let cfg = serde_json::json!({"plugins_trusted": {trust_key(&r): true}});
        assert!(is_trusted(&cfg, &r));
        assert!(!is_trusted(&serde_json::json!({"plugins_trusted": {trust_key(&r): "yes"}}), &r));
    }

    #[test]
    fn installed_lists_confined_plugins_with_manifests() {
        let r = vault("inst");
        fs::write(r.join(".obsidian/plugins/good/manifest.json"), r#"{"id":"good","name":"Good","version":"1.0.0"}"#).unwrap();
        fs::write(r.join(".obsidian/plugins/good/styles.css"), ".x{color:red}").unwrap();
        fs::create_dir_all(r.join(".obsidian/plugins/nomani")).unwrap();
        fs::create_dir_all(r.join(".obsidian/plugins/badmani")).unwrap();
        fs::write(r.join(".obsidian/plugins/badmani/manifest.json"), "[1]").unwrap();
        let l = installed(&r);
        assert_eq!(l.len(), 1, "{l:?}");
        assert_eq!(l[0]["id"], "good");
        assert_eq!(l[0]["manifest"]["name"], "Good");
        assert_eq!(l[0]["hasMain"], true);
        assert_eq!(l[0]["hasStyles"], true);
        assert_eq!(read_styles(&r, "good").unwrap().as_deref(), Some(".x{color:red}"));
        assert_eq!(read_styles(&r, "nomani").unwrap(), None);
    }

    #[test]
    fn set_enabled_round_trips_and_refuses_garbage() {
        let r = vault("seten");
        assert_eq!(set_enabled(&r, "new-one", true).unwrap(), vec!["good", "gone", "sneaky", "linkdir", "new-one"]);
        assert_eq!(set_enabled(&r, "gone", false).unwrap(), vec!["good", "sneaky", "linkdir", "new-one"]);
        assert!(set_enabled(&r, "../x", true).is_err());
        fs::write(r.join(ENABLED_FILE), "{\"not\":\"array\"}").unwrap();
        assert!(set_enabled(&r, "good", true).is_err());
        assert_eq!(fs::read_to_string(r.join(ENABLED_FILE)).unwrap(), "{\"not\":\"array\"}");
        fs::remove_file(r.join(ENABLED_FILE)).unwrap();
        assert_eq!(set_enabled(&r, "good", true).unwrap(), vec!["good"]);
    }

    #[test]
    fn data_json_is_confined() {
        let r = vault("data");
        assert_eq!(read_data(&r, "good").unwrap(), serde_json::Value::Null);
        write_data(&r, "good", &serde_json::json!({"a": 1})).unwrap();
        assert_eq!(read_data(&r, "good").unwrap(), serde_json::json!({"a": 1}));
        assert!(fs::read_to_string(r.join(".obsidian/plugins/good/data.json")).unwrap().contains("\"a\": 1"));
        fs::write(r.join(".obsidian/plugins/good/data.json"), "{\n  \"globalFilter\": \"#task\",\n  \"globalQuery\": \"not do").unwrap();
        assert_eq!(read_data(&r, "good").unwrap(), serde_json::Value::Null);
        // cut inside a multi-byte character: invalid JSON too, not a load error
        fs::write(r.join(".obsidian/plugins/good/data.json"), b"{\"k\": \"\xc3").unwrap();
        assert_eq!(read_data(&r, "good").unwrap(), serde_json::Value::Null);
        // saving over the invalid file keeps its bytes as data.json.invalid; a valid one is replaced as is
        write_data(&r, "good", &serde_json::json!({"d": 2})).unwrap();
        assert_eq!(fs::read(r.join(".obsidian/plugins/good/data.json.invalid")).unwrap(), b"{\"k\": \"\xc3");
        assert_eq!(read_data(&r, "good").unwrap(), serde_json::json!({"d": 2}));
        write_data(&r, "good", &serde_json::json!({"d": 3})).unwrap();
        assert_eq!(fs::read(r.join(".obsidian/plugins/good/data.json.invalid")).unwrap(), b"{\"k\": \"\xc3");
        for bad in ["..", "../good", "a/b", ".obsidian", "nosuch"] {
            assert!(write_data(&r, bad, &serde_json::json!(1)).is_err(), "{bad}");
            assert!(read_data(&r, bad).is_err(), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn data_json_symlinks_refused() {
        use std::os::unix::fs::symlink;
        let r = vault("datasym");
        symlink(r.join("secret.md"), r.join(".obsidian/plugins/good/data.json")).unwrap();
        assert!(read_data(&r, "good").is_err());
        assert!(write_data(&r, "good", &serde_json::json!({"x": 1})).is_err());
        assert_eq!(fs::read_to_string(r.join("secret.md")).unwrap(), "SECRET");
        let outside = r.with_extension("dataout");
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, r.join(".obsidian/plugins/linkdir")).unwrap();
        assert!(write_data(&r, "linkdir", &serde_json::json!({"x": 1})).is_err());
        assert!(!outside.join("data.json").exists());
    }
}
