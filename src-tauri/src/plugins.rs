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
    // #250: the vaultfs fd read (no symlink at any component, regular file by fstat, 1 MiB on the fd)
    crate::vaultfs::read_string(root, Path::new(ENABLED_FILE), 1024 * 1024)
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

/// #288 review: read a file plugin_file_in accepted, checking the OPEN file again:
/// O_NOFOLLOW (a symlink swapped in after the check is refused), fstat on the fd
/// (regular file, <= cap) and a read bounded by cap. A file replaced or grown
/// between plugin_file_in's stat and this read can never make it allocate more
/// than cap + 1 bytes, nor block on a FIFO / device.
/// #250 (merged with #251): the open and the read go through the vaultfs fd walk from the vault
/// root (O_NOFOLLOW on every component, not just the leaf; regular file by fstat; cap on the fd),
/// so a plugin folder swapped for a symlink after the check is refused as well.
pub fn read_checked(root: &Path, p: &Path, cap: u64) -> Result<Vec<u8>, &'static str> {
    let rel = crate::vaultfs::rel_of(root, p).map_err(|_| "unreadable")?;
    crate::vaultfs::read(root, &rel, cap).map_err(|e| {
        // the walk refuses a symlinked component itself (PermissionDenied "refused: symlink in vault path") or the kernel does (ELOOP)
        if e.raw_os_error() == Some(libc::ELOOP) || e.to_string().contains("symlink") {
            "symlinked file"
        } else if e.kind() == std::io::ErrorKind::FileTooLarge {
            "too big"
        } else {
            "unreadable"
        }
    })
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

/// #250 (audit F-47): rule 5 AND the read. `plugin_file_in` decides (same
/// refusals as ever), then the bytes come from `vaultfs::read`: the openat
/// walk from the vault fd (O_NOFOLLOW per component and on the leaf,
/// O_NONBLOCK, regular file by fstat, `cap` on the fd). A file or folder
/// swapped for a symlink after the check is refused, never followed.
/// Ok(None) = absent.
pub fn plugin_read_in(root: &Path, id: &str, file: &str, cap: u64) -> Result<Option<Vec<u8>>, &'static str> {
    if plugin_file_in(root, id, file, cap)?.is_none() {
        return Ok(None);
    }
    let rel = Path::new(".obsidian").join("plugins").join(id).join(file);
    match crate::vaultfs::read(root, &rel, cap) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::FileTooLarge => Err("too big"),
        Err(_) => Err("not a regular file in the vault"),
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

/// #251: what the scheme handler serves — rules 1-5 (serve), THEN rule 6: the plugin's main.js,
/// manifest.json and styles.css match the pin the user created for this vault folder
/// (plugin_pins.rs, outside the vault). The bytes sent are the bytes that were hashed (read once).
/// `vault` = the vault folder identity (plugins::TrustKey.id); `cfg` = ~/.opensidian.json.
pub fn serve_pinned(root: Option<&Path>, path: &str, trusted: bool, cfg: &serde_json::Value, vault: Option<&str>) -> Served {
    // rules 1-5 first; main.js is then read ONCE, by verify, and the bytes it hashed are served
    let (root, id) = match gate(root, path, trusted) {
        Err(s) => return s,
        Ok((r, i, _)) => (r, i),
    };
    match crate::plugin_pins::verify(cfg, vault, root, id) {
        Ok(f) => Served::Js(wrap_cjs(id, &String::from_utf8_lossy(&f.main)).into_bytes()),
        Err(why) => Served::Refused(why),
    }
}

/// #251: a plugin's styles.css through the snippet sanitizer, only while the plugin matches its pin
/// (the css applied is the css that was hashed). None = no styles.css, or css the sanitizer refuses;
/// Err = the pin refuses the plugin (the load stops, api-core.js enable).
pub fn read_styles_pinned(root: &Path, id: &str, cfg: &serde_json::Value, vault: Option<&str>) -> Result<Option<String>, String> {
    if !valid_id(id) {
        return Err("bad plugin id".into());
    }
    let f = crate::plugin_pins::verify(cfg, vault, root, id).map_err(|e| format!("{id}/styles.css: {e}"))?;
    match f.styles {
        None => Ok(None),
        Some(b) => {
            // #288 review: an error here is the PIN's (it stops the load in api-core.js, fail
            // closed); css the sanitizer refuses is not applied and the plugin still runs, as before
            let css = String::from_utf8(b).map_err(|e| e.to_string()).and_then(|src| crate::themefs::sanitize_css(&src).map(|s| s.css));
            match css {
                Ok(c) => Ok(Some(c)),
                Err(e) => {
                    eprintln!("[plugin] {id}/styles.css not applied: {e}");
                    Ok(None)
                }
            }
        }
    }
}

/// the whole decision for one request (rules 1-5), unit-testable without tauri
pub fn serve(root: Option<&Path>, path: &str, trusted: bool) -> Served {
    match gate(root, path, trusted) {
        Err(s) => s,
        Ok((root, id, p)) => match read_checked(root, &p, MAX_MAIN_JS) {
            Ok(b) => Served::Js(wrap_cjs(id, &String::from_utf8_lossy(&b)).into_bytes()),
            Err("unreadable") => Served::Missing,
            Err(why) => Served::Refused(why),
        },
    }
}

/// rules 1-5 without reading main.js: Ok = (vault root, plugin id, main.js path)
fn gate<'a>(root: Option<&'a Path>, path: &'a str, trusted: bool) -> Result<(&'a Path, &'a str, PathBuf), Served> {
    let Some(id) = parse_path(path) else { return Err(Served::Refused("bad path")) };
    let Some(root) = root else { return Err(Served::Refused("no vault")) };
    if !trusted {
        return Err(Served::Refused("restricted mode"));
    }
    if !enabled_ids(root).iter().any(|e| e == id) {
        return Err(Served::Refused("not enabled"));
    }
    match plugin_file_in(root, id, "main.js", MAX_MAIN_JS) {
        Err(why) => Err(Served::Refused(why)),
        Ok(None) => Err(Served::Missing),
        Ok(Some(p)) => Ok((root, id, p)),
    }
}

/// trust is keyed by the CANONICAL vault path in ~/.opensidian.json
/// ("plugins_trusted": { "<canon path>": true }); absent = restricted.
/// #127: the entry is BOUND to the directory it was given for — "plugins_trust_id":
/// { "<canon path>": "<inode>:<birth time ns>" } of the canonical root, recorded with the flag (plugins_set)
/// or, for an entry from before the binding, on the first boot that finds it (plugins_state, bind_legacy).
/// A directory removed and replaced at a trusted path (another vault cloned / unpacked / moved there) has
/// another inode and another birth time: the entry does not apply, the vault opens restricted and turning
/// trust on asks again. Never read from the vault (a vault file cannot forge its own inode or birth time).
/// `id` is None when the root cannot be stat'ed or its filesystem keeps no birth time: such a root is
/// never trusted (a recorded id never matches, a legacy entry is not honoured, ON is refused).
#[derive(Clone, PartialEq, Eq)]
pub struct TrustKey {
    pub path: String,
    pub id: Option<String>,
}

impl std::ops::Deref for TrustKey {
    type Target = str;
    fn deref(&self) -> &str {
        &self.path
    }
}

impl std::fmt::Display for TrustKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.path)
    }
}

impl std::fmt::Debug for TrustKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.path, f)
    }
}

/// the ~/.opensidian.json key holding { "<canon vault path>": "<identity>" } (#127)
pub const TRUST_ID_KEY: &str = "plugins_trust_id";

/// the identity of the directory at `canon` (#127): inode + birth time (statx) in ns. Birth time
/// separates a reused inode number (rm -rf + mkdir often gets the freed one back); a filesystem
/// without birth time gives NO identity (review #129: the inode alone is reused). The device number is left out on purpose: it is not
/// stable across remounts of removable / network disks and the path already names the place.
#[cfg(unix)]
pub fn dir_identity(canon: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let m = fs::metadata(canon).ok()?;
    if !m.is_dir() {
        return None;
    }
    identity_of(m.ino(), m.created().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()))
}

/// Windows: the same identity from the open directory — file index (what an inode is here) plus the
/// creation time (what birth time is): a rm -rf + mkdir gets a new creation time even if NTFS reuses the
/// index. The volume serial is left out for the reason the device number is.
#[cfg(windows)]
pub fn dir_identity(canon: &Path) -> Option<String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Info {
        attributes: u32,
        created: [u32; 2],
        accessed: [u32; 2],
        written: [u32; 2],
        volume: u32,
        size_high: u32,
        size_low: u32,
        links: u32,
        index_high: u32,
        index_low: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandle(h: *mut core::ffi::c_void, info: *mut Info) -> i32;
    }
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
    let f = fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(canon).ok()?;
    let mut info = std::mem::MaybeUninit::<Info>::zeroed();
    // SAFETY: the handle is open; the struct is read only when the call succeeded
    if unsafe { GetFileInformationByHandle(f.as_raw_handle() as *mut core::ffi::c_void, info.as_mut_ptr()) } == 0 {
        return None;
    }
    let info = unsafe { info.assume_init() };
    if info.attributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return None;
    }
    let index = (info.index_high as u64) << 32 | info.index_low as u64;
    identity_of(index, f.metadata().ok()?.created().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()))
}

/// "<inode>:<birth ns>"; no birth time = no identity (an inode number alone is handed out again)
pub fn identity_of(ino: u64, birth: Option<std::time::Duration>) -> Option<String> {
    birth.map(|b| format!("{ino}:{}", b.as_nanos()))
}

/// a path lookup (canonicalize + stat): callers compute it BEFORE the lifecycle lock (CC1)
pub fn trust_key(root: &Path) -> TrustKey {
    let canon = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let id = dir_identity(&canon);
    TrustKey { path: canon.display().to_string(), id }
}

pub fn is_trusted(cfg: &serde_json::Value, root: &Path) -> bool {
    is_trusted_key(cfg, &trust_key(root))
}

/// BB1 (PR #42 round 11): the trust check for a key computed earlier (trust_key canonicalizes the
/// vault root: a path lookup, done before the lifecycle lock by desk_shell). #127: the flag must be
/// true AND the recorded identity (if any) must be the directory's — a mismatch is restricted.
/// No recorded identity = an entry from before the binding: honoured only for a root WITH an
/// identity (bind_legacy records it on that boot); a root without one is restricted (review #129).
pub fn is_trusted_key(cfg: &serde_json::Value, key: &TrustKey) -> bool {
    // #144 toggle 12: under the latched lockdown every vault is restricted; the trust
    // entries are not touched and apply again after lockdown is off and the app restarted
    if crate::security::trust_blocked() {
        return false;
    }
    if cfg[TRUST_KEY][&key.path].as_bool() != Some(true) {
        return false;
    }
    match &cfg[TRUST_ID_KEY][&key.path] {
        serde_json::Value::Null => key.id.is_some(),
        rec => key.id.as_deref().is_some_and(|id| rec.as_str() == Some(id)),
    }
}

/// #127: the identity a trust flip of `key` records next to the flag (plugins_set writes both, one
/// update, the flag line kept in plugins_set where consent-nobypass S3/S6 pin it): ON = the directory's
/// identity, OFF = none. ON without an identity is refused (review #129): a flag written alone would
/// read as a legacy entry and carry over to whatever directory is put at the path later.
pub fn trust_bind(key: &TrustKey, on: bool) -> Result<Option<String>, String> {
    if !on {
        return Ok(None);
    }
    key.id.clone().map(Some).ok_or_else(|| NO_IDENTITY.to_string())
}

pub const NO_IDENTITY: &str =
    "this vault folder cannot be trusted: its filesystem gives no stable folder identity (birth time); plugins stay restricted";

/// #127 migration: an entry trusted before the binding (flag true, no identity) is bound to the
/// directory found at its path now (the first boot of it after the upgrade). None = nothing to write.
/// The caller writes it with cfgstore::Op::InsertIn (only if still absent, under the config lock):
/// a stale snapshot never overwrites an identity a concurrent trust-ON recorded (review #129).
pub fn bind_legacy(cfg: &serde_json::Value, key: &TrustKey) -> Option<(String, String, serde_json::Value)> {
    let id = key.id.as_ref()?;
    (cfg[TRUST_KEY][&key.path].as_bool() == Some(true) && cfg[TRUST_ID_KEY][&key.path].is_null())
        .then(|| (TRUST_ID_KEY.to_string(), key.path.clone(), serde_json::json!(id)))
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
    // #250: listed from the fd the no-follow walk opened (a symlinked
    // .obsidian or plugins dir lists nothing; symlinked entries are not listed)
    let Ok(rd) = crate::vaultfs::Vault::open(root).and_then(|v| v.p_list(Path::new(".obsidian/plugins"))) else { return out };
    let mut names: Vec<String> = rd.into_iter().filter(|(n, k)| *k == "folder" && valid_id(n)).map(|(n, _)| n).collect();
    names.sort();
    for id in names {
        let Ok(Some(mb)) = plugin_read_in(root, &id, "manifest.json", MAX_MANIFEST) else { continue };
        let Some(manifest) = String::from_utf8(mb)
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
    // #250: read by the vaultfs fd walk; absent = empty, anything not a plain
    // in-vault file (symlink at any component, FIFO, over 1 MiB) is refused
    let mut list: Vec<serde_json::Value> = match crate::vaultfs::read_string(root, Path::new(ENABLED_FILE), 1024 * 1024) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Err(format!("{ENABLED_FILE}: {e}")),
        Err(_) => return Err(format!("{ENABLED_FILE}: not a plain file")),
        Ok(s) => serde_json::from_str(&s).map_err(|_| format!("{ENABLED_FILE}: not a JSON array, left untouched"))?,
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
    match plugin_read_in(root, id, "data.json", MAX_DATA_JSON)? {
        None => Ok(serde_json::Value::Null),
        // bytes, not a String: a file cut inside a multi-byte character is invalid JSON (null), not a read error
        Some(b) => Ok(serde_json::from_slice(&b).unwrap_or(serde_json::Value::Null)),
    }
}

/// Plugin.saveData(): pretty JSON (Obsidian writes 2-space), atomic, confined. A data.json that is not valid JSON
/// (loadData gave the plugin null, so it is saving its defaults) is copied to data.json.invalid first, so the save
/// does not destroy the settings the user's broken edit still holds
pub fn write_data(root: &Path, id: &str, data: &serde_json::Value) -> Result<(), String> {
    // a symlinked data.json refuses
    if let Some(old) = plugin_read_in(root, id, "data.json", MAX_DATA_JSON)? {
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
    match plugin_read_in(root, id, "styles.css", MAX_STYLES)? {
        None => Ok(None),
        Some(b) => {
            let src = String::from_utf8(b).map_err(|e| format!("{id}/styles.css: {e}"))?;
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

    /// what main.rs plugins_set writes for a trust flip (flag, + the identity on ON), applied to `cfg`
    fn flip(cfg: &mut serde_json::Value, key: &TrustKey, on: bool) -> Result<(), String> {
        let bind = trust_bind(key, on)?;
        cfg[TRUST_KEY][&key.path] = serde_json::json!(on);
        if let Some(id) = bind {
            cfg[TRUST_ID_KEY][&key.path] = serde_json::json!(id);
        }
        Ok(())
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
        let cfg = serde_json::json!({"plugins_trusted": {trust_key(&r).path: true}});
        assert!(is_trusted(&cfg, &r));
        assert!(!is_trusted(&serde_json::json!({"plugins_trusted": {trust_key(&r).path: "yes"}}), &r));
    }

    /* #127 case c: a trusted path whose directory is removed and replaced by another vault (cloned,
       unpacked, moved there) does not inherit the entry: the identity recorded with the flag is the
       old directory's. The vault's own files never enter the decision. */
    #[test]
    fn trust_is_bound_to_the_directory_not_only_the_path() {
        let r = vault("bound");
        let k = trust_key(&r);
        assert!(k.id.is_some(), "a vault dir has an identity");
        let mut cfg = serde_json::json!({});
        flip(&mut cfg, &k, true).unwrap();
        assert_eq!(cfg[TRUST_ID_KEY][&k.path].as_str(), k.id.as_deref());
        assert!(is_trusted(&cfg, &r), "the directory it was given for");
        assert!(is_trusted_key(&cfg, &trust_key(&r)), "a recomputed key of the same directory");
        // a new file inside the vault changes nothing (mtime/ctime are not part of the identity)
        fs::write(r.join("new.md"), "x").unwrap();
        assert!(is_trusted(&cfg, &r));
        // the directory replaced at the same path: restricted, even though the path entry is still true
        fs::remove_dir_all(&r).unwrap();
        fs::create_dir_all(r.join(".obsidian/plugins/good")).unwrap();
        fs::write(r.join(".obsidian/community-plugins.json"), r#"["good"]"#).unwrap();
        assert_eq!(cfg[TRUST_KEY][&k.path].as_bool(), Some(true));
        assert_ne!(trust_key(&r).id, k.id, "a replaced directory has another identity");
        assert!(!is_trusted(&cfg, &r), "the replaced directory inherited trust");
        // a directory moved onto the path (rename keeps the inode of the moved dir) is just as foreign
        let other = vault("bound-other");
        fs::remove_dir_all(&r).unwrap();
        fs::rename(&other, &r).unwrap();
        assert!(!is_trusted(&cfg, &r));
        // trusting again (the asked flip) binds the new directory
        flip(&mut cfg, &trust_key(&r), true).unwrap();
        assert!(is_trusted(&cfg, &r));
        // OFF writes only the flag
        let mut off = serde_json::json!({});
        flip(&mut off, &trust_key(&r), false).unwrap();
        assert_eq!(off, serde_json::json!({"plugins_trusted": {trust_key(&r).path: false}}));
        // a root that cannot be stat'ed never matches a recorded identity
        let gone = TrustKey { path: k.path.clone(), id: None };
        assert!(!is_trusted_key(&cfg, &gone));
        // a forged identity of the wrong type is no match either
        cfg[TRUST_ID_KEY][&k.path] = serde_json::json!(true);
        assert!(!is_trusted(&cfg, &r));
    }

    #[test]
    fn a_legacy_entry_is_honoured_once_then_bound() {
        let r = vault("legacy");
        let k = trust_key(&r);
        let mut cfg = serde_json::json!({"plugins_trusted": {k.path.clone(): true}});
        assert!(is_trusted(&cfg, &r), "an entry from before the binding still applies");
        let (p, key, v) = bind_legacy(&cfg, &k).expect("the first boot binds it");
        cfg[p][key] = v;
        assert!(bind_legacy(&cfg, &k).is_none(), "bound once");
        assert!(is_trusted(&cfg, &r));
        fs::remove_dir_all(&r).unwrap();
        fs::create_dir_all(r.join(".obsidian")).unwrap();
        assert!(!is_trusted(&cfg, &r), "after the binding a replaced directory is restricted");
        // nothing to bind for an untrusted or OFF entry
        assert!(bind_legacy(&serde_json::json!({}), &k).is_none());
        assert!(bind_legacy(&serde_json::json!({"plugins_trusted": {k.path.clone(): false}}), &k).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_root_shares_the_identity_of_its_target() {
        let r = vault("symid");
        // the link lives inside the test's own vault dir (no fixed name in the shared temp dir)
        let l = r.join("self-link");
        std::os::unix::fs::symlink(&r, &l).unwrap();
        assert_eq!(trust_key(&l), trust_key(&r));
        let _ = fs::remove_file(&l);
    }

    /* review #129: a root without an identity (no birth time, or not stat-able) is never trusted:
       the inode alone is reused, and a flag written without an identity would read as legacy. */
    #[test]
    fn no_identity_no_trust() {
        assert_eq!(identity_of(7, None), None, "the inode alone is no identity");
        assert_eq!(identity_of(7, Some(std::time::Duration::from_nanos(42))).as_deref(), Some("7:42"));
        let r = vault("noid");
        let bare = TrustKey { path: trust_key(&r).path, id: None };
        assert_eq!(trust_bind(&bare, true), Err(NO_IDENTITY.to_string()), "ON without an identity is refused");
        assert_eq!(trust_bind(&bare, false), Ok(None), "OFF is always possible");
        let legacy = serde_json::json!({"plugins_trusted": {bare.path.clone(): true}});
        assert!(!is_trusted_key(&legacy, &bare), "a legacy entry is not honoured where it cannot be bound");
        assert!(bind_legacy(&legacy, &bare).is_none());
    }

    /* review #129: the boot's late binding uses InsertIn — a trust-ON that recorded the identity of a
       new directory in the meantime is never overwritten by the stale snapshot's identity */
    #[test]
    fn a_late_legacy_binding_never_overwrites_a_newer_identity() {
        let r = vault("race");
        let k = trust_key(&r);
        let stale = serde_json::json!({"plugins_trusted": {k.path.clone(): true}});
        let old = TrustKey { path: k.path.clone(), id: Some("1:1".into()) };
        let (p, key, id) = bind_legacy(&stale, &old).expect("legacy");
        let mut disk = serde_json::json!({});
        flip(&mut disk, &k, true).unwrap();
        crate::cfgstore::apply(&mut disk, &[crate::cfgstore::Op::InsertIn(p, key, id)]);
        assert_eq!(disk[TRUST_ID_KEY][&k.path].as_str(), k.id.as_deref(), "the newer identity stays");
        assert!(is_trusted_key(&disk, &k));
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

    /* #251 rule 6: in a trusted vault, main.js is served only while the plugin matches the pin the
       user created (store install / native acknowledgement), and the styles only then too */
    #[test]
    fn serve_pinned_serves_only_what_was_allowed() {
        let r = vault("pin");
        fs::write(r.join(".obsidian/plugins/good/manifest.json"), r#"{"id":"good","version":"1.0.0"}"#).unwrap();
        fs::write(r.join(".obsidian/plugins/good/styles.css"), ".x{color:red}").unwrap();
        let vid = trust_key(&r).id;
        let v = vid.as_deref();
        let none = serde_json::json!({});
        // not pinned: refused, even though rules 1-5 allow it
        assert_eq!(serve(Some(&r), "/good/main.js", true), js_ok(&r));
        assert_eq!(serve_pinned(Some(&r), "/good/main.js", true, &none, v), Served::Refused("not allowed: new plugin, not acknowledged"));
        assert!(read_styles_pinned(&r, "good", &none, v).is_err());
        // pinned: served, byte-identical to the unpinned answer; styles applied
        let mut cfg = serde_json::json!({});
        let p = crate::plugin_pins::pin_of(&crate::plugin_pins::read_files(&r, "good").unwrap(), "ack", 1);
        crate::plugin_pins::apply_set(&mut cfg, v.unwrap(), "good", &p);
        assert_eq!(serve_pinned(Some(&r), "/good/main.js", true, &cfg, v), js_ok(&r));
        assert_eq!(read_styles_pinned(&r, "good", &cfg, v).unwrap().as_deref(), Some(".x{color:red}"));
        // rules 1-5 still come first: restricted mode / not enabled / bad path are refused as before
        assert_eq!(serve_pinned(Some(&r), "/good/main.js", false, &cfg, v), Served::Refused("restricted mode"));
        assert_eq!(serve_pinned(Some(&r), "/good/manifest.json", true, &cfg, v), Served::Refused("bad path"));
        assert_eq!(serve_pinned(None, "/good/main.js", true, &cfg, v), Served::Refused("no vault"));
        // main.js rewritten on disk (sync, git, another plugin): refused, styles too
        fs::write(r.join(".obsidian/plugins/good/main.js"), "\"use strict\";module.exports=2;//end").unwrap();
        assert_eq!(serve_pinned(Some(&r), "/good/main.js", true, &cfg, v), Served::Refused("not allowed: changed on disk since it was allowed"));
        assert!(read_styles_pinned(&r, "good", &cfg, v).is_err());
        // another folder identity (a vault replaced at the path) has no pins
        assert!(matches!(serve_pinned(Some(&r), "/good/main.js", true, &cfg, Some("1:1")), Served::Refused(_)));
        assert!(matches!(serve_pinned(Some(&r), "/good/main.js", true, &cfg, None), Served::Refused(_)));
        // a damaged store: nothing
        assert_eq!(serve_pinned(Some(&r), "/good/main.js", true, &serde_json::json!({"pluginPins": 1}), v), Served::Refused("not allowed: plugin pin store damaged"));
    }

    /* #288 review (CodeAnt nitpick api-core.js:677): the styles answer is Err ONLY when the pin
       refuses (api-core.js then stops the load); css the sanitizer refuses is not applied and is
       not a reason to stop a plugin that matches its pin. */
    #[test]
    fn styles_pinned_err_means_the_pin_refused() {
        let r = vault("pincss");
        fs::write(r.join(".obsidian/plugins/good/manifest.json"), r#"{"id":"good","version":"1.0.0"}"#).unwrap();
        fs::write(r.join(".obsidian/plugins/good/styles.css"), ".x{color:red} /* unclosed").unwrap();
        let vid = trust_key(&r).id;
        let v = vid.as_deref();
        let mut cfg = serde_json::json!({});
        let p = crate::plugin_pins::pin_of(&crate::plugin_pins::read_files(&r, "good").unwrap(), "ack", 1);
        crate::plugin_pins::apply_set(&mut cfg, v.unwrap(), "good", &p);
        assert_eq!(read_styles_pinned(&r, "good", &cfg, v), Ok(None), "refused css: not applied, not an error");
        fs::write(r.join(".obsidian/plugins/good/styles.css"), ".x{color:blue}").unwrap();
        assert!(read_styles_pinned(&r, "good", &cfg, v).is_err(), "changed css: the pin refuses");
        let _ = fs::remove_dir_all(&r);
    }

    fn js_ok(r: &Path) -> Served {
        Served::Js(wrap_cjs("good", &fs::read_to_string(r.join(".obsidian/plugins/good/main.js")).unwrap()).into_bytes())
    }
}
