// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* #251 (F-40): community plugin code is bound to what the user allowed.

   invariant: a plugin's main.js is served (and its styles.css applied) only while
              sha256(main.js), sha256(manifest.json) and sha256(styles.css | absent) on disk
              equal a PIN the user created for that vault and plugin id.
   store    = the per-user config ~/.opensidian.json, key "pluginPins" — the file the trust flag
              (plugins_trusted) and the spawn grants (spawnAllow) live in, NEVER the vault:
                { "<vault folder identity>": { "<plugin id>": Pin } }
              The vault is keyed by its folder identity (plugins::dir_identity, inode + birth
              time), not by its path: a different folder put at a trusted path inherits no pin.
              Written ONLY through the cfgstore Ops PluginPinSet / PluginPinDrop (flock + merge +
              atomic rename, 0600); the generic Ops refuse the key (cfgstore::apply). The
              plugin_store sidecar (opensidian-install.json, in the vault) is never read here.
   who pins = exactly two places, never on sight:
              (a) a store install / update the user started (plugin_store::install_pinned: the
                  pin is of the bytes that were fetched and written);
              (b) the Allow answer of the native "allow plugin code?" dialog (main.rs
                  plugins_pin_ack): Rust measures the files, shows them, and on Allow writes
                  exactly what it measured. A plugin that changes again after that is refused.
   fail closed: no pin = "new", other bytes = "changed", a malformed store = "damaged" (nothing
              in that vault loads until an explicit write replaces the damaged part). */
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

/// the config key the pins live under (~/.opensidian.json)
pub const KEY: &str = "pluginPins";
/// the most plugins one acknowledgement dialog lists
pub const MAX_ACK: usize = 64;
/// a manifest.json past this is not a plugin (plugins::installed uses the same cap)
pub const MAX_MANIFEST: u64 = 256 * 1024;

/// what the user allowed for one plugin of one vault
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pin {
    /// sha256 hex of main.js
    pub main: String,
    /// sha256 hex of manifest.json
    pub manifest: String,
    /// sha256 hex of styles.css; None = the plugin had no styles.css
    pub styles: Option<String>,
    /// manifest.json "version" (shown in the dialog and Settings)
    #[serde(default)]
    pub version: String,
    /// "store" (install / update) | "ack" (the native dialog)
    pub by: String,
    /// unix seconds
    #[serde(default)]
    pub at: u64,
}

/// the three files as they are on disk now, read once (the bytes that are hashed are the bytes
/// that are served)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Files {
    pub main: Vec<u8>,
    pub manifest: Vec<u8>,
    pub styles: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// the files match the pin
    Ok,
    /// no pin for this plugin in this vault
    New,
    /// pinned, but these files differ now
    Changed(Vec<&'static str>),
    /// the pin store is malformed: nothing in the vault loads
    Damaged(&'static str),
    /// the files cannot be read (missing main.js / manifest.json, symlink, too big)
    Unreadable(String),
}

impl Status {
    /// the short word plugins_state / the UI use
    pub fn word(&self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::New => "new",
            Status::Changed(_) => "changed",
            Status::Damaged(_) => "damaged",
            Status::Unreadable(_) => "unreadable",
        }
    }
}

pub const DAMAGED_STORE: &str = "the record of allowed plugins (pluginPins in ~/.opensidian.json) is damaged";
pub const NO_IDENTITY: &str = "this vault folder has no stable identity";

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_hex(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid(p: &Pin) -> bool {
    is_hash(&p.main) && is_hash(&p.manifest) && p.styles.as_deref().is_none_or(is_hash)
}

/// read main.js, manifest.json and styles.css of <vault>/.obsidian/plugins/<id> through the
/// confinement of plugins::plugin_file_in (no symlinked dir or file, no escape, size caps)
pub fn read_files(root: &Path, id: &str) -> Result<Files, String> {
    read_files_with(root, id, &|_| {})
}

/// read_files with `between` run after plugin_file_in's check and before the read
/// (tests: a concurrent writer replacing the file in that window, #288 review)
pub(crate) fn read_files_with(root: &Path, id: &str, between: &dyn Fn(&Path)) -> Result<Files, String> {
    let read = |f: &str, cap: u64| -> Result<Option<Vec<u8>>, String> {
        match crate::plugins::plugin_file_in(root, id, f, cap).map_err(|e| format!("{id}/{f}: {e}"))? {
            None => Ok(None),
            Some(p) => {
                between(&p);
                crate::plugins::read_checked(root, &p, cap).map(Some).map_err(|e| format!("{id}/{f}: {e}"))
            }
        }
    };
    let main = read("main.js", crate::plugins::MAX_MAIN_JS)?.ok_or_else(|| format!("{id}/main.js: missing"))?;
    let manifest = read("manifest.json", MAX_MANIFEST)?.ok_or_else(|| format!("{id}/manifest.json: missing"))?;
    let styles = read("styles.css", crate::plugins::MAX_STYLES)?;
    Ok(Files { main, manifest, styles })
}

fn version_of(manifest: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(manifest).ok().and_then(|m| m["version"].as_str().map(String::from)).unwrap_or_default()
}

/// the pin of these bytes
pub fn pin_of(f: &Files, by: &str, at: u64) -> Pin {
    Pin {
        main: sha256_hex(&f.main),
        manifest: sha256_hex(&f.manifest),
        styles: f.styles.as_deref().map(sha256_hex),
        version: version_of(&f.manifest),
        by: by.into(),
        at,
    }
}

/// which of the three files differ from the pin (empty = match)
pub fn diff(p: &Pin, f: &Files) -> Vec<&'static str> {
    let now = pin_of(f, "", 0);
    let mut d = Vec::new();
    if now.main != p.main {
        d.push("main.js");
    }
    if now.manifest != p.manifest {
        d.push("manifest.json");
    }
    if now.styles != p.styles {
        d.push("styles.css");
    }
    d
}

/// the pins of one vault. Err = the store is damaged (fail closed); Ok(empty) = none yet.
/// `vault` None (a folder without identity) has no pins.
pub fn table(cfg: &serde_json::Value, vault: Option<&str>) -> Result<Vec<(String, Pin)>, &'static str> {
    let all = &cfg[KEY];
    if all.is_null() {
        return Ok(Vec::new());
    }
    let Some(all) = all.as_object() else { return Err(DAMAGED_STORE) };
    let Some(vault) = vault else { return Ok(Vec::new()) };
    let Some(v) = all.get(vault) else { return Ok(Vec::new()) };
    let Some(v) = v.as_object() else { return Err(DAMAGED_STORE) };
    v.iter()
        .map(|(id, x)| match serde_json::from_value::<Pin>(x.clone()) {
            Ok(p) if valid(&p) && crate::plugins::valid_id(id) => Ok((id.clone(), p)),
            _ => Err(DAMAGED_STORE),
        })
        .collect()
}

/// the pin of one plugin (Ok(None) = not pinned)
pub fn pin(cfg: &serde_json::Value, vault: Option<&str>, id: &str) -> Result<Option<Pin>, &'static str> {
    Ok(table(cfg, vault)?.into_iter().find(|(i, _)| i == id).map(|(_, p)| p))
}

/// the status of plugin `id` against the files `f` read from disk
pub fn status_of(cfg: &serde_json::Value, vault: Option<&str>, id: &str, f: &Files) -> Status {
    match pin(cfg, vault, id) {
        Err(e) => Status::Damaged(e),
        Ok(None) => Status::New,
        Ok(Some(p)) => {
            let d = diff(&p, f);
            if d.is_empty() {
                Status::Ok
            } else {
                Status::Changed(d)
            }
        }
    }
}

/// the status of plugin `id` as it is on disk now
pub fn status(cfg: &serde_json::Value, vault: Option<&str>, root: &Path, id: &str) -> Status {
    // a damaged store is reported as such even for a plugin whose files cannot be read
    if let Err(e) = table(cfg, vault) {
        return Status::Damaged(e);
    }
    match read_files(root, id) {
        Err(e) => Status::Unreadable(e),
        Ok(f) => status_of(cfg, vault, id, &f),
    }
}

/// THE load gate: the files of `id`, read once, iff they match the user's pin. The caller serves
/// these bytes (never re-reads the file). Err = refuse, with the reason.
pub fn verify(cfg: &serde_json::Value, vault: Option<&str>, root: &Path, id: &str) -> Result<Files, &'static str> {
    if vault.is_none() {
        return Err("not allowed: the vault folder has no identity");
    }
    let f = read_files(root, id).map_err(|_| "not allowed: plugin files unreadable")?;
    match status_of(cfg, vault, id, &f) {
        Status::Ok => Ok(f),
        Status::New => Err("not allowed: new plugin, not acknowledged"),
        Status::Changed(_) => Err("not allowed: changed on disk since it was allowed"),
        Status::Damaged(_) => Err("not allowed: plugin pin store damaged"),
        Status::Unreadable(_) => Err("not allowed: plugin files unreadable"),
    }
}

/// the vault's pin map, made an object again where it was damaged (a damaged table is dropped:
/// what it held was not trustworthy, so every plugin in it is "new" again — still fail closed)
fn vault_map<'a>(v: &'a mut serde_json::Value, vault: &str) -> &'a mut serde_json::Map<String, serde_json::Value> {
    if !v[KEY].is_object() {
        if !v[KEY].is_null() {
            eprintln!("[plugin-pins] {DAMAGED_STORE}: replaced");
        }
        v[KEY] = serde_json::json!({});
    }
    let ok = v[KEY][vault].as_object().is_some_and(|m| {
        m.iter().all(|(id, x)| crate::plugins::valid_id(id) && serde_json::from_value::<Pin>(x.clone()).is_ok_and(|p| valid(&p)))
    });
    if !ok {
        if !v[KEY][vault].is_null() {
            eprintln!("[plugin-pins] {DAMAGED_STORE} for one vault: replaced");
        }
        v[KEY][vault] = serde_json::json!({});
    }
    v[KEY][vault].as_object_mut().expect("object")
}

/// cfgstore::apply for Op::PluginPinSet
pub fn apply_set(v: &mut serde_json::Value, vault: &str, plugin: &str, p: &Pin) {
    if vault.is_empty() || !crate::plugins::valid_id(plugin) || !valid(p) {
        eprintln!("[plugin-pins] refused a malformed pin for {plugin:?}");
        return;
    }
    let m = vault_map(v, vault);
    m.insert(plugin.into(), serde_json::to_value(p).unwrap_or(serde_json::Value::Null));
}

/// cfgstore::apply for Op::PluginPinDrop
pub fn apply_drop(v: &mut serde_json::Value, vault: &str, plugin: &str) {
    if let Some(m) = v[KEY].get_mut(vault).and_then(|x| x.as_object_mut()) {
        m.remove(plugin);
    }
}

/// one line per plugin the acknowledgement dialog lists
pub struct AckItem {
    pub id: String,
    pub pin: Pin,
    pub was: Status,
    pub old_version: Option<String>,
}

fn short(h: &str) -> &str {
    &h[..h.len().min(16)]
}

/// the native dialog's body: the vault, then each plugin with its version, why it is asked (new,
/// changed: which files) and the start of the hashes. Ids and versions are Debug-quoted (page /
/// vault data cannot style or hide part of the text).
pub fn ack_text(vault: &str, items: &[AckItem], damaged: bool) -> String {
    let mut t = String::from(
        "These community plugins are new in this vault, or their files changed on disk since you allowed them. \
         Changes made outside the plugin browser (sync, git, another program or plugin) are not reviewed.\n\n",
    );
    t.push_str(&format!("Vault: {vault:?}\n\n"));
    if damaged {
        t.push_str("The record of allowed plugins for this vault was damaged; allowing replaces it.\n\n");
    }
    for it in items {
        let why = match &it.was {
            Status::Changed(d) => format!(
                "CHANGED on disk ({}){}",
                d.join(", "),
                it.old_version.as_deref().map(|v| format!(", allowed version was {v:?}")).unwrap_or_default()
            ),
            _ => "NEW, not allowed before".into(),
        };
        t.push_str(&format!("{:?} version {:?}: {why}\n", it.id, it.pin.version));
        t.push_str(&format!("    main.js {}…  manifest.json {}…  styles.css {}\n", short(&it.pin.main), short(&it.pin.manifest), it.pin.styles.as_deref().map(|s| format!("{}…", short(s))).unwrap_or_else(|| "(none)".into())));
    }
    t.push_str("\nAllow only plugins you expect. Their code runs with your user's rights. Cancel = they stay off; you can review them later in Settings › Community plugins.");
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    const V: &str = "123:456";

    fn vault(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("opensidian-pins-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".obsidian/plugins/good")).unwrap();
        fs::write(root.join(".obsidian/plugins/good/main.js"), "module.exports=1;").unwrap();
        fs::write(root.join(".obsidian/plugins/good/manifest.json"), r#"{"id":"good","version":"1.0.0"}"#).unwrap();
        fs::write(root.join(".obsidian/plugins/good/styles.css"), ".a{}").unwrap();
        fs::write(root.join(".obsidian/community-plugins.json"), r#"["good"]"#).unwrap();
        root
    }

    fn pinned(root: &Path, id: &str) -> serde_json::Value {
        let mut cfg = serde_json::json!({});
        apply_set(&mut cfg, V, id, &pin_of(&read_files(root, id).unwrap(), "ack", 1));
        cfg
    }

    #[test]
    fn a_matching_pin_loads_the_bytes_it_hashed() {
        let r = vault("ok");
        let cfg = pinned(&r, "good");
        assert_eq!(status(&cfg, Some(V), &r, "good"), Status::Ok);
        let f = verify(&cfg, Some(V), &r, "good").unwrap();
        assert_eq!(f.main, b"module.exports=1;");
        assert_eq!(f.styles.as_deref(), Some(&b".a{}"[..]));
        assert_eq!(cfg[KEY][V]["good"]["version"], "1.0.0");
    }

    #[test]
    fn a_changed_main_manifest_or_styles_is_refused() {
        for (file, body) in [("main.js", "module.exports=2;"), ("manifest.json", r#"{"id":"good","version":"1.0.1"}"#), ("styles.css", ".b{}")] {
            let r = vault(&format!("chg-{file}"));
            let cfg = pinned(&r, "good");
            fs::write(r.join(".obsidian/plugins/good").join(file), body).unwrap();
            assert_eq!(status(&cfg, Some(V), &r, "good"), Status::Changed(vec![file]), "{file}");
            assert_eq!(verify(&cfg, Some(V), &r, "good"), Err("not allowed: changed on disk since it was allowed"), "{file}");
        }
        // a styles.css that appears or disappears is a change too
        let r = vault("chg-css-gone");
        let cfg = pinned(&r, "good");
        fs::remove_file(r.join(".obsidian/plugins/good/styles.css")).unwrap();
        assert_eq!(status(&cfg, Some(V), &r, "good"), Status::Changed(vec!["styles.css"]));
        fs::write(r.join(".obsidian/plugins/good/styles.css"), "").unwrap();
        assert_eq!(status(&cfg, Some(V), &r, "good"), Status::Changed(vec!["styles.css"]), "an empty styles.css is not 'no styles.css'");
        // the same bytes written back match again
        fs::write(r.join(".obsidian/plugins/good/styles.css"), ".a{}").unwrap();
        assert_eq!(status(&cfg, Some(V), &r, "good"), Status::Ok);
    }

    #[test]
    fn a_new_plugin_in_a_trusted_vault_is_refused_until_acknowledged() {
        let r = vault("new");
        let mut cfg = pinned(&r, "good");
        fs::create_dir_all(r.join(".obsidian/plugins/evil")).unwrap();
        fs::write(r.join(".obsidian/plugins/evil/main.js"), "steal()").unwrap();
        fs::write(r.join(".obsidian/plugins/evil/manifest.json"), r#"{"id":"evil","version":"6.6.6"}"#).unwrap();
        assert_eq!(status(&cfg, Some(V), &r, "evil"), Status::New);
        assert_eq!(verify(&cfg, Some(V), &r, "evil"), Err("not allowed: new plugin, not acknowledged"));
        // a pin of another vault or another id does not apply
        assert_eq!(verify(&cfg, Some("999:1"), &r, "good"), Err("not allowed: new plugin, not acknowledged"));
        assert!(verify(&cfg, None, &r, "good").is_err(), "no folder identity, no pins");
        // the acknowledgement (what plugins_pin_ack writes after Allow) lets exactly those bytes run
        apply_set(&mut cfg, V, "evil", &pin_of(&read_files(&r, "evil").unwrap(), "ack", 2));
        assert!(verify(&cfg, Some(V), &r, "evil").is_ok());
        assert!(verify(&cfg, Some(V), &r, "good").is_ok(), "the other pin is kept");
        // dropped (store uninstall) = new again
        apply_drop(&mut cfg, V, "evil");
        assert_eq!(status(&cfg, Some(V), &r, "evil"), Status::New);
    }

    /* the pin store lives outside the vault: a record inside the vault — the store's sidecar,
       or anything shaped like a pin store — is never consulted, even when it matches the new bytes */
    #[test]
    fn a_pin_store_inside_the_vault_is_ignored() {
        let r = vault("invault");
        let cfg = pinned(&r, "good");
        fs::write(r.join(".obsidian/plugins/good/main.js"), "evil()").unwrap();
        let forged = pin_of(&read_files(&r, "good").unwrap(), "store", 3);
        let side = serde_json::json!({"files": {"main.js": {"sha256": forged.main}, "manifest.json": {"sha256": forged.manifest}}});
        fs::write(r.join(".obsidian/plugins/good").join(crate::plugin_store::SIDECAR), side.to_string()).unwrap();
        let mut fake = serde_json::json!({});
        apply_set(&mut fake, V, "good", &forged);
        fs::write(r.join(".obsidian/pluginPins.json"), fake.to_string()).unwrap();
        fs::write(r.join(".opensidian.json"), fake.to_string()).unwrap();
        assert!(verify(&cfg, Some(V), &r, "good").is_err());
        assert_eq!(status(&cfg, Some(V), &r, "good"), Status::Changed(vec!["main.js"]));
    }

    #[test]
    fn a_damaged_pin_store_fails_closed() {
        let r = vault("dmg");
        let good = pinned(&r, "good");
        let mut bad_entry = good.clone();
        bad_entry[KEY][V]["good"]["main"] = serde_json::json!("not-a-hash");
        let mut extra = good.clone();
        extra[KEY][V]["other"] = serde_json::json!({"main": 1});
        let mut unknown_field = good.clone();
        unknown_field[KEY][V]["good"]["trusted"] = serde_json::json!(true);
        for cfg in [
            serde_json::json!({KEY: "garbage"}),
            serde_json::json!({KEY: [1, 2]}),
            serde_json::json!({KEY: {V: "x"}}),
            bad_entry,
            extra,
            unknown_field,
        ] {
            assert_eq!(status(&cfg, Some(V), &r, "good"), Status::Damaged(DAMAGED_STORE), "{cfg}");
            assert_eq!(verify(&cfg, Some(V), &r, "good"), Err("not allowed: plugin pin store damaged"), "{cfg}");
            assert_eq!(table(&cfg, Some(V)), Err(DAMAGED_STORE));
        }
        // an explicit write replaces the damaged part; what it held is not carried over
        let mut cfg = serde_json::json!({KEY: {V: {"good": {"main": 1}}, "other:1": {}}});
        apply_set(&mut cfg, V, "new-one", &pin_of(&read_files(&r, "good").unwrap(), "ack", 4));
        assert_eq!(table(&cfg, Some(V)).unwrap().len(), 1);
        assert_eq!(status(&cfg, Some(V), &r, "good"), Status::New, "the damaged pin was dropped, not repaired into a match");
        assert!(cfg[KEY]["other:1"].is_object(), "another vault's pins stay");
        // a malformed pin is never written
        let mut cfg = serde_json::json!({});
        let mut p = pin_of(&read_files(&r, "good").unwrap(), "ack", 5);
        p.main = "zz".into();
        apply_set(&mut cfg, V, "good", &p);
        apply_set(&mut cfg, V, "../x", &pin_of(&read_files(&r, "good").unwrap(), "ack", 5));
        assert_eq!(cfg, serde_json::json!({}));
    }

    #[test]
    fn unreadable_plugins_are_refused() {
        let r = vault("unread");
        let cfg = pinned(&r, "good");
        fs::remove_file(r.join(".obsidian/plugins/good/manifest.json")).unwrap();
        assert!(matches!(status(&cfg, Some(V), &r, "good"), Status::Unreadable(_)));
        assert!(verify(&cfg, Some(V), &r, "good").is_err());
        #[cfg(unix)]
        {
            let r = vault("unread-link");
            let cfg = pinned(&r, "good");
            let out = r.with_extension("out.js");
            fs::write(&out, "module.exports=1;").unwrap();
            fs::remove_file(r.join(".obsidian/plugins/good/main.js")).unwrap();
            std::os::unix::fs::symlink(&out, r.join(".obsidian/plugins/good/main.js")).unwrap();
            assert!(verify(&cfg, Some(V), &r, "good").is_err(), "a symlinked main.js with the pinned bytes");
        }
    }

    #[test]
    fn the_dialog_names_every_plugin_and_why() {
        let r = vault("text");
        let f = read_files(&r, "good").unwrap();
        let items = vec![
            AckItem { id: "good".into(), pin: pin_of(&f, "ack", 0), was: Status::Changed(vec!["main.js"]), old_version: Some("0.9".into()) },
            AckItem { id: "evil\n".into(), pin: pin_of(&f, "ack", 0), was: Status::New, old_version: None },
        ];
        let t = ack_text("/v", &items, false);
        assert!(t.contains("\"good\" version \"1.0.0\": CHANGED on disk (main.js), allowed version was \"0.9\""), "{t}");
        assert!(t.contains("\"evil\\n\" version \"1.0.0\": NEW"), "{t}");
        assert!(t.contains(&format!("main.js {}…", &sha256_hex(b"module.exports=1;")[..16])), "{t}");
        assert!(ack_text("/v", &items, true).contains("damaged"));
    }

    /// #288 review (CodeAnt): plugin_file_in checks the size, then a concurrent vault writer
    /// replaces the file. The read is of the OPEN file, bounded by the cap, never via a symlink.
    #[test]
    #[cfg(unix)] // symlinks, FIFOs or mode bits
    fn file_replaced_after_the_check_is_read_bounded() {
        let r = vault("swap");
        let main = r.join(".obsidian/plugins/good/main.js");
        let cap = crate::plugins::MAX_MAIN_JS;
        // grown past the cap in the window between the check and the read
        let grow = |p: &Path| {
            if p.ends_with("main.js") {
                fs::OpenOptions::new().write(true).open(p).unwrap().set_len(cap + 1).unwrap();
            }
        };
        let e = read_files_with(&r, "good", &grow).err().expect("a main.js grown past the cap is refused");
        assert!(e.contains("main.js") && e.contains("too big"), "{e}");
        // swapped for a symlink to a big file outside the plugin
        fs::write(&main, "module.exports=1;").unwrap();
        let outside = r.join("outside.js");
        fs::OpenOptions::new().create(true).write(true).open(&outside).unwrap().set_len(cap + 1).unwrap();
        let swap = |p: &Path| {
            if p.ends_with("main.js") {
                fs::remove_file(p).unwrap();
                std::os::unix::fs::symlink(&outside, p).unwrap();
            }
        };
        let e = read_files_with(&r, "good", &swap).err().expect("a main.js swapped for a symlink is refused");
        assert!(e.contains("main.js") && e.contains("symlinked"), "{e}");
        // untouched: read as before
        fs::remove_file(&main).unwrap();
        fs::write(&main, "module.exports=1;").unwrap();
        assert_eq!(read_files_with(&r, "good", &|_| {}).unwrap().main, b"module.exports=1;");
        let _ = fs::remove_dir_all(&r);
    }
}
