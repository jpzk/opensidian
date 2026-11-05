// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* cp-browser: the community plugin store behind Settings > Community plugins > Browse.

   Every byte the browser shows or installs is fetched HERE, by Rust; the page
   only ever sees the parsed result. The page has no network capability of its
   own for this (CSP connect-src stays as it is).

   Egress rules (all in `check_url`, applied to EVERY hop, redirects included):
     - https only, default port only, no userinfo, a DNS name (never an IP);
     - the host is one of ALLOWED_HOSTS, compared exactly (no suffix match);
     - redirects are followed by hand (libsoup's own follower is OFF), at most
       MAX_REDIRECTS, each Location re-checked before it is requested;
     - a per-I/O timeout (IO_TIMEOUT_S) and a whole-request deadline;
     - a byte cap per kind of file, enforced while reading (a lying or absent
       Content-Length does not get past it);
     - no token, no cookie jar, no auth: the requests are anonymous.

   Gate determinism: OPENSIDIAN_PLUGIN_STORE_BASE=http://127.0.0.1:<port> (a
   loopback origin only; anything else is ignored with a stderr line) maps every
   ALREADY-CHECKED logical URL https://<host>/<path> to <base>/<host>/<path>, so
   a static fixture mirror answers. The allowlist still runs on the logical URL,
   which is what makes the negative cases testable against the mirror.

   Install never trusts or enables anything: it writes main.js, manifest.json
   and (when the release has one) styles.css into <vault>/.obsidian/plugins/<id>/
   and a record of what it wrote (SIDECAR, sha256 per file). Restricted mode and
   the enable toggle stay the user's own acts (plugins.rs / plugins_set). */

use crate::plugins;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "api.github.com",
    "raw.githubusercontent.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];
pub const BASE_ENV: &str = "OPENSIDIAN_PLUGIN_STORE_BASE";
const LIST_URL: &str = "https://raw.githubusercontent.com/obsidianmd/obsidian-releases/HEAD/community-plugins.json";
const STATS_URL: &str = "https://raw.githubusercontent.com/obsidianmd/obsidian-releases/HEAD/community-plugin-stats.json";
/// the same literal as a macro, so a compile-time label (settings.rs API_LABEL) can
/// concat! it without a second copy
#[macro_export]
macro_rules! compat_app_version {
    () => {
        "1.14.4"
    };
}
/// the Obsidian plugin API version this build implements -- the ONE source of truth.
/// ui/plugins/api-core.js API_VERSION and ui/plugins/node-desktop.js appVersion carry the
/// same literal; compat_version_matches_the_js_api fails T0 on drift. Shown in Settings >
/// General ("Plugin API version"). A manifest asking for more is refused.
pub const COMPAT_APP_VERSION: &str = compat_app_version!();
/// what install writes next to the plugin: where each file came from + its sha256
pub const SIDECAR: &str = "opensidian-install.json";
const MAX_REDIRECTS: usize = 5;
const IO_TIMEOUT_S: u32 = 20;
const DEADLINE: Duration = Duration::from_secs(180);
const MAX_LIST: u64 = 32 * 1024 * 1024;
const MAX_API_JSON: u64 = 2 * 1024 * 1024;
const MAX_README: u64 = 1024 * 1024;
const MAX_MANIFEST: u64 = 256 * 1024;
const CACHE_FOR: Duration = Duration::from_secs(600);

/* ---------------------------------------------------------------- egress */

/// the one egress check: Ok(url) only for an https URL on an allowlisted host
pub fn check_url(u: &str) -> Result<url::Url, String> {
    let p = url::Url::parse(u).map_err(|_| format!("refused: not a URL: {u:?}"))?;
    if p.scheme() != "https" {
        return Err(format!("refused: not https: {p}"));
    }
    if !p.username().is_empty() || p.password().is_some() {
        return Err(format!("refused: credentials in URL for host {}", p.host_str().unwrap_or("?")));
    }
    if p.port().is_some() {
        return Err(format!("refused: non-default port in {p}"));
    }
    let host = match p.host() {
        Some(url::Host::Domain(h)) => h.to_string(),
        _ => return Err(format!("refused: no DNS host in {p}")),
    };
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        return Err(format!("refused: host {host} is not on the plugin store allowlist"));
    }
    Ok(p)
}

/// the loopback fixture mirror, if the env names a valid one
fn mirror_base() -> Option<url::Url> {
    let raw = std::env::var(BASE_ENV).ok().filter(|s| !s.is_empty())?;
    match parse_mirror(&raw) {
        Some(u) => Some(u),
        None => {
            eprintln!("[plugin-store] {BASE_ENV}={raw:?} ignored: only an http(s) loopback origin is accepted");
            None
        }
    }
}

pub fn parse_mirror(raw: &str) -> Option<url::Url> {
    let u = url::Url::parse(raw).ok()?;
    let loopback = match u.host() {
        Some(url::Host::Ipv4(a)) => a.is_loopback(),
        Some(url::Host::Ipv6(a)) => a.is_loopback(),
        Some(url::Host::Domain(d)) => d == "localhost",
        None => false,
    };
    (matches!(u.scheme(), "http" | "https") && loopback && u.username().is_empty() && u.password().is_none()).then_some(u)
}

/// what is actually requested for a CHECKED logical URL
pub fn wire_url(logical: &url::Url, base: Option<&url::Url>) -> String {
    match base {
        None => logical.to_string(),
        Some(b) => format!("{}/{}{}", b.as_str().trim_end_matches('/'), logical.host_str().unwrap_or(""), logical.path()),
    }
}

/// a redirect: Location resolved against the LOGICAL url (a mirror's absolute
/// Location under its own base maps back to the logical host), then re-checked
pub fn next_hop(cur: &url::Url, loc: &str, base: Option<&url::Url>) -> Result<url::Url, String> {
    if let Some(b) = base {
        let bs = b.as_str().trim_end_matches('/');
        if let Some(rest) = loc.strip_prefix(bs) {
            return check_url(&format!("https:/{rest}"));
        }
    }
    let next = cur.join(loc).map_err(|_| format!("refused: bad redirect Location {loc:?}"))?;
    check_url(next.as_str())
}

#[cfg(target_os = "linux")]
pub fn http_get(logical: &str, cap: u64, what: &str) -> Result<Vec<u8>, String> {
    use soup::glib::translate::IntoGlib;
    use soup::prelude::*;
    let base = mirror_base();
    let mut cur = check_url(logical)?;
    let sess = soup::Session::new();
    sess.set_timeout(IO_TIMEOUT_S);
    sess.set_idle_timeout(IO_TIMEOUT_S);
    sess.set_user_agent(concat!("opensidian/", env!("CARGO_PKG_VERSION"), " plugin-browser"));
    let deadline = Instant::now() + DEADLINE;
    for _ in 0..=MAX_REDIRECTS {
        let wire = wire_url(&cur, base.as_ref());
        let msg = soup::Message::new("GET", &wire).map_err(|_| format!("{what}: bad request URL {cur}"))?;
        msg.set_flags(soup::MessageFlags::NO_REDIRECT);
        if cur.host_str() == Some("api.github.com") {
            if let Some(h) = msg.request_headers() {
                h.append("Accept", "application/vnd.github+json");
            }
        }
        let stream = sess
            .send(&msg, soup::gio::Cancellable::NONE)
            .map_err(|e| format!("{what}: {} — {e}", cur.host_str().unwrap_or("?")))?;
        let code = msg.status().into_glib() as u32;
        if (300..400).contains(&code) && code != 304 {
            let loc = msg.response_headers().and_then(|h| h.one("Location")).map(|s| s.to_string());
            let _ = stream.close(soup::gio::Cancellable::NONE);
            let loc = loc.ok_or_else(|| format!("{what}: HTTP {code} without a Location"))?;
            cur = next_hop(&cur, &loc, base.as_ref()).map_err(|e| format!("{what}: redirect {e}"))?;
            continue;
        }
        if code != 200 {
            let _ = stream.close(soup::gio::Cancellable::NONE);
            let hint = if (code == 403 || code == 429) && cur.host_str() == Some("api.github.com") {
                " (GitHub's anonymous API rate limit; try again later)"
            } else {
                ""
            };
            return Err(format!("{what}: HTTP {code} from {}{hint}", cur.host_str().unwrap_or("?")));
        }
        let declared = msg.response_headers().map(|h| h.content_length()).unwrap_or(0);
        if declared > 0 && declared as u64 > cap {
            let _ = stream.close(soup::gio::Cancellable::NONE);
            return Err(format!("refused: {what} is {declared} bytes, over the {cap}-byte cap"));
        }
        let mut out = Vec::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            if Instant::now() > deadline {
                return Err(format!("{what}: timed out"));
            }
            let n = stream.read(&mut buf[..], soup::gio::Cancellable::NONE).map_err(|e| format!("{what}: read: {e}"))?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            if out.len() as u64 > cap {
                let _ = stream.close(soup::gio::Cancellable::NONE);
                return Err(format!("refused: {what} is larger than the {cap}-byte cap"));
            }
        }
        return Ok(out);
    }
    Err(format!("{what}: more than {MAX_REDIRECTS} redirects"))
}

#[cfg(any(windows, target_os = "macos"))]
pub fn http_get(logical: &str, cap: u64, what: &str) -> Result<Vec<u8>, String> {
    let base = mirror_base();
    let mut cur = check_url(logical)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(IO_TIMEOUT_S as u64))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("opensidian/", env!("CARGO_PKG_VERSION"), " plugin-browser"))
        .build()
        .map_err(|e| format!("{what}: http client: {e}"))?;
    let deadline = Instant::now() + DEADLINE;
    for _ in 0..=MAX_REDIRECTS {
        if Instant::now() > deadline {
            return Err(format!("{what}: timed out"));
        }
        let wire = wire_url(&cur, base.as_ref());
        let mut req = client.get(&wire);
        if cur.host_str() == Some("api.github.com") {
            req = req.header("Accept", "application/vnd.github+json");
        }
        let resp = req.send().map_err(|e| format!("{what}: {} — {e}", cur.host_str().unwrap_or("?")))?;
        let code = resp.status().as_u16();
        if (300..400).contains(&code) && code != 304 {
            let loc = resp
                .headers()
                .get("Location")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| format!("{what}: HTTP {code} without a Location"))?
                .to_string();
            cur = next_hop(&cur, &loc, base.as_ref()).map_err(|e| format!("{what}: redirect {e}"))?;
            continue;
        }
        if code != 200 {
            let hint = if (code == 403 || code == 429) && cur.host_str() == Some("api.github.com") {
                " (GitHub's anonymous API rate limit; try again later)"
            } else {
                ""
            };
            return Err(format!("{what}: HTTP {code} from {}{hint}", cur.host_str().unwrap_or("?")));
        }
        if let Some(cl) = resp.content_length() {
            if cl > cap {
                return Err(format!("refused: {what} is {cl} bytes, over the {cap}-byte cap"));
            }
        }
        let mut out = Vec::new();
        let mut stream = resp;
        let mut buf = [0u8; 64 * 1024];
        use std::io::Read;
        loop {
            if Instant::now() > deadline {
                return Err(format!("{what}: timed out"));
            }
            let n = stream.read(&mut buf).map_err(|e| format!("{what}: read: {e}"))?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            if out.len() as u64 > cap {
                return Err(format!("refused: {what} is larger than the {cap}-byte cap"));
            }
        }
        return Ok(out);
    }
    Err(format!("{what}: more than {MAX_REDIRECTS} redirects"))
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub fn http_get(logical: &str, _cap: u64, what: &str) -> Result<Vec<u8>, String> {
    check_url(logical)?;
    Err(format!("{what}: the plugin browser has no network client on this platform"))
}

/// the fetcher every function below takes, so the logic is testable offline
pub type Fetch<'a> = &'a dyn Fn(&str, u64, &str) -> Result<Vec<u8>, String>;

/* ------------------------------------------------------------- the list */

/// "owner/name", each a plain GitHub name, so it can only ever be a path prefix
pub fn valid_repo(r: &str) -> bool {
    let ok = |s: &str| {
        !s.is_empty() && s != "." && s != ".." && s.len() <= 100
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    matches!(r.split_once('/'), Some((o, n)) if ok(o) && ok(n) && !n.contains('/'))
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

/// community-plugins.json + community-plugin-stats.json -> the browser's rows
pub fn merge_list(list: &[u8], stats: Option<&[u8]>) -> Result<Vec<Value>, String> {
    let arr: Vec<Value> = serde_json::from_slice(list).map_err(|e| format!("plugin list: not a JSON array ({e})"))?;
    let stats: Value = stats.and_then(|b| serde_json::from_slice(b).ok()).unwrap_or(Value::Null);
    let mut out = Vec::with_capacity(arr.len());
    for e in &arr {
        let (id, repo) = (s(e, "id"), s(e, "repo"));
        if !plugins::valid_id(&id) || !valid_repo(&repo) {
            continue;
        }
        let st = &stats[id.as_str()];
        out.push(json!({
            "id": id, "name": s(e, "name"), "author": s(e, "author"),
            "description": s(e, "description"), "repo": repo,
            "downloads": st["downloads"].as_u64().unwrap_or(0),
            "updated": st["updated"].as_u64().unwrap_or(0),
        }));
    }
    Ok(out)
}

struct Cached {
    at: Instant,
    rows: Vec<Value>,
}
static LIST: Mutex<Option<Cached>> = Mutex::new(None);
static RELEASES: Mutex<Option<HashMap<String, (Instant, Release)>>> = Mutex::new(None);

pub fn list(fetch: Fetch, refresh: bool) -> Result<Vec<Value>, String> {
    if !refresh {
        if let Some(c) = LIST.lock().unwrap().as_ref().filter(|c| c.at.elapsed() < CACHE_FOR) {
            return Ok(c.rows.clone());
        }
    }
    let l = fetch(LIST_URL, MAX_LIST, "plugin list")?;
    // download counts are a nicety: the list works without them
    let st = fetch(STATS_URL, MAX_LIST, "download stats").map_err(|e| eprintln!("[plugin-store] {e}")).ok();
    let rows = merge_list(&l, st.as_deref())?;
    *LIST.lock().unwrap() = Some(Cached { at: Instant::now(), rows: rows.clone() });
    if refresh {
        *RELEASES.lock().unwrap() = None;
    }
    Ok(rows)
}

fn entry(fetch: Fetch, id: &str) -> Result<Value, String> {
    if !plugins::valid_id(id) {
        return Err("bad plugin id".into());
    }
    list(fetch, false)?
        .into_iter()
        .find(|r| r["id"] == id)
        .ok_or_else(|| format!("{id} is not in the community plugin list"))
}

/* ---------------------------------------------------------- the release */

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub published: String,
    /// asset name -> (download URL, declared size)
    pub assets: HashMap<String, (String, u64)>,
}

pub fn parse_release(b: &[u8]) -> Result<Release, String> {
    let v: Value = serde_json::from_slice(b).map_err(|e| format!("release: not JSON ({e})"))?;
    let mut assets = HashMap::new();
    for a in v["assets"].as_array().into_iter().flatten() {
        let (n, u) = (s(a, "name"), s(a, "browser_download_url"));
        if matches!(n.as_str(), "main.js" | "manifest.json" | "styles.css") && !u.is_empty() {
            assets.insert(n, (u, a["size"].as_u64().unwrap_or(0)));
        }
    }
    Ok(Release { tag: s(&v, "tag_name"), published: s(&v, "published_at"), assets })
}

fn release(fetch: Fetch, repo: &str) -> Result<Release, String> {
    if let Some((at, r)) = RELEASES.lock().unwrap().as_ref().and_then(|m| m.get(repo)) {
        if at.elapsed() < CACHE_FOR {
            return Ok(r.clone());
        }
    }
    // the store version is the one the repo's default-branch manifest.json names, served by the
    // release tagged with it -- "latest" can be a pre-release under another id (calendar's 2.0.0
    // beta is "calendar-beta"). No readable HEAD manifest, or no such release: the latest release.
    let r = match head_version(fetch, repo).and_then(|v| tagged_release(fetch, repo, &v)) {
        Some(r) => r,
        None => parse_release(&fetch(&format!("https://api.github.com/repos/{repo}/releases/latest"), MAX_API_JSON, "latest release")?)?,
    };
    RELEASES.lock().unwrap().get_or_insert_with(HashMap::new).insert(repo.to_string(), (Instant::now(), r.clone()));
    Ok(r)
}

/// a version string safe to put in a release-tag URL path
fn valid_tag(v: &str) -> bool {
    !v.is_empty() && v.len() <= 64 && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-+_".contains(c))
}

/// the version in the repo's default-branch manifest.json
fn head_version(fetch: Fetch, repo: &str) -> Option<String> {
    let b = fetch(&format!("https://raw.githubusercontent.com/{repo}/HEAD/manifest.json"), MAX_MANIFEST, "manifest.json").ok()?;
    let v = s(&serde_json::from_slice::<Value>(&b).ok()?, "version");
    valid_tag(&v).then_some(v)
}

/// the release tagged <v> (or v<v>), if it carries a main.js and a manifest.json
fn tagged_release(fetch: Fetch, repo: &str, v: &str) -> Option<Release> {
    [v.to_string(), format!("v{v}")].iter().find_map(|t| {
        let b = fetch(&format!("https://api.github.com/repos/{repo}/releases/tags/{t}"), MAX_API_JSON, "release").ok()?;
        parse_release(&b).ok().filter(|r| r.assets.contains_key("main.js") && r.assets.contains_key("manifest.json"))
    })
}

/// "1.10.2" > "1.9" — numeric dotted compare; a non-numeric tail is ignored
pub fn version_gt(a: &str, b: &str) -> bool {
    let p = |v: &str| -> Vec<u64> {
        v.trim().trim_start_matches('v').split('.')
            .map(|x| x.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (p(a), p(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

/// a downloaded manifest.json, validated against the list entry
pub fn check_manifest(bytes: &[u8], list_id: &str) -> Result<Value, String> {
    let m: Value = serde_json::from_slice(bytes).map_err(|e| format!("refused: manifest.json is not JSON ({e})"))?;
    if !m.is_object() {
        return Err("refused: manifest.json is not an object".into());
    }
    let mid = s(&m, "id");
    if mid != list_id {
        return Err(format!("refused: manifest id {mid:?} does not match the list id {list_id:?}"));
    }
    if s(&m, "version").is_empty() {
        return Err("refused: manifest.json has no version".into());
    }
    let min = s(&m, "minAppVersion");
    if !min.is_empty() && version_gt(&min, COMPAT_APP_VERSION) {
        return Err(format!("refused: needs app version {min}, this app provides the plugin API of {COMPAT_APP_VERSION}"));
    }
    Ok(m)
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/* ------------------------------------------------- install / uninstall */

fn installed_manifest(root: &Path, id: &str) -> Option<Value> {
    let p = plugins::plugin_file_in(root, id, "manifest.json", MAX_MANIFEST).ok().flatten()?;
    serde_json::from_slice::<Value>(&fs::read(p).ok()?).ok().filter(Value::is_object)
}

/// the sidecar, plus whether each recorded file still hashes to its record
fn audit(root: &Path, id: &str) -> Value {
    let Some(dir) = plugins::plugin_dir_in(root, id) else { return Value::Null };
    let p = dir.join(SIDECAR);
    match fs::symlink_metadata(&p) {
        Ok(m) if m.is_file() && m.len() <= MAX_MANIFEST => {}
        _ => return Value::Null,
    }
    let Some(mut rec) = fs::read(&p).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { return Value::Null };
    let mut intact = true;
    if let Some(files) = rec["files"].as_object() {
        for (name, f) in files {
            let cap = if name == "main.js" { plugins::MAX_MAIN_JS } else { plugins::MAX_STYLES.max(MAX_MANIFEST) };
            let now = plugins::plugin_file_in(root, id, name, cap).ok().flatten().and_then(|p| fs::read(p).ok()).map(|b| sha256_hex(&b));
            intact &= now.as_deref() == f["sha256"].as_str();
        }
    }
    rec["intact"] = json!(intact);
    rec
}

pub fn detail(fetch: Fetch, root: Option<&Path>, id: &str) -> Result<Value, String> {
    let e = entry(fetch, id)?;
    let repo = s(&e, "repo");
    let readme = fetch(&format!("https://raw.githubusercontent.com/{repo}/HEAD/README.md"), MAX_README, "README")
        .map(|b| crate::render_md(&String::from_utf8_lossy(&b), &[]))
        .unwrap_or_else(|err| format!("<p class=\"cpb-noreadme\">{}</p>", html_esc(&format!("No README: {err}"))));
    let (latest, rel_err) = match release(fetch, &repo) {
        Ok(r) => {
            // the release's own manifest.json is what Update compares against (the tag is only a label)
            let ver = r.assets.get("manifest.json")
                .and_then(|(u, _)| fetch(u, MAX_MANIFEST, "manifest.json").ok())
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .map(|m| s(&m, "version"))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| r.tag.trim_start_matches('v').to_string());
            (json!({"version": ver, "tag": r.tag, "published": r.published}), Value::Null)
        }
        Err(err) => (Value::Null, json!(err)),
    };
    let inst = root.and_then(|r| installed_manifest(r, id));
    let iv = inst.as_ref().map(|m| s(m, "version"));
    let update = matches!((&iv, latest["version"].as_str()), (Some(i), Some(l)) if version_gt(l, i));
    Ok(json!({
        "entry": e, "readme": readme, "latest": latest, "releaseError": rel_err,
        "installed": iv, "update": update,
        "audit": root.map(|r| audit(r, id)).unwrap_or(Value::Null),
    }))
}

fn html_esc(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// install or update <id> from its store release (see release()). Nothing is written
/// until every asset URL passed the allowlist and every file passed its check.
pub fn install(fetch: Fetch, root: &Path, id: &str) -> Result<Value, String> {
    let e = entry(fetch, id)?;
    let repo = s(&e, "repo");
    let r = release(fetch, &repo)?;
    let need = |n: &str| r.assets.get(n).cloned().ok_or_else(|| format!("refused: the latest release of {repo} has no {n}"));
    let (main_u, main_n) = need("main.js")?;
    let (man_u, _) = need("manifest.json")?;
    let css = r.assets.get("styles.css").cloned();
    // every URL first, before a single byte is fetched
    for u in [&main_u, &man_u].into_iter().chain(css.as_ref().map(|c| &c.0)) {
        check_url(u)?;
    }
    if main_n > plugins::MAX_MAIN_JS {
        return Err(format!("refused: main.js is {main_n} bytes, over the {}-byte cap", plugins::MAX_MAIN_JS));
    }
    let man = fetch(&man_u, MAX_MANIFEST, "manifest.json")?;
    let m = check_manifest(&man, id)?;
    let main = fetch(&main_u, plugins::MAX_MAIN_JS, "main.js")?;
    let styles = match &css {
        Some((u, _)) => Some(fetch(u, plugins::MAX_STYLES, "styles.css")?),
        None => None,
    };
    let rel = Path::new(".obsidian").join("plugins").join(id);
    crate::vaultfs::mkdir_p(root, &rel).map_err(|e| format!("{id}: create plugin folder: {e}"))?;
    plugins::plugin_dir_in(root, id).ok_or("refused: plugin folder is not confined to the vault")?;
    let mut files = serde_json::Map::new();
    let mut put = |name: &str, b: &[u8], url: &str| -> Result<(), String> {
        crate::vaultfs::write_atomic(root, &rel.join(name), b).map_err(|e| format!("{id}/{name}: {e}"))?;
        files.insert(name.into(), json!({"sha256": sha256_hex(b), "bytes": b.len(), "url": url}));
        Ok(())
    };
    put("manifest.json", &man, &man_u)?;
    put("main.js", &main, &main_u)?;
    match (&styles, &css) {
        (Some(b), Some((u, _))) => put("styles.css", b, u)?,
        _ => {
            // an update whose release dropped styles.css must not keep the old one
            if let Ok(Some(p)) = plugins::plugin_file_in(root, id, "styles.css", u64::MAX) {
                fs::remove_file(p).map_err(|e| format!("{id}/styles.css: {e}"))?;
            }
        }
    }
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let rec = json!({
        "by": "opensidian plugin browser", "id": id, "repo": repo, "version": s(&m, "version"),
        "tag": r.tag, "installedAt": at, "files": files,
    });
    let body = serde_json::to_string_pretty(&rec).map_err(|e| e.to_string())?;
    crate::vaultfs::write_atomic(root, &rel.join(SIDECAR), body.as_bytes()).map_err(|e| format!("{id}/{SIDECAR}: {e}"))?;
    eprintln!("[plugin-store] installed {id} {} from {repo}", s(&m, "version"));
    Ok(rec)
}

/// remove <id>: off the enabled list first, then its (confined) folder
pub fn uninstall(root: &Path, id: &str) -> Result<(), String> {
    let dir = plugins::plugin_dir_in(root, id).ok_or_else(|| format!("{id}: not installed, or its folder is not confined"))?;
    plugins::set_enabled(root, id, false)?;
    fs::remove_dir_all(&dir).map_err(|e| format!("{id}: remove: {e}"))?;
    eprintln!("[plugin-store] uninstalled {id}");
    Ok(())
}

/* ------------------------------------------------------------- commands
   async + spawn_blocking: a fetch never runs on the main (GTK) thread. */

fn live(u: &str, cap: u64, what: &str) -> Result<Vec<u8>, String> {
    http_get(u, cap, what)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| format!("plugin store: {e}"))?
}

#[tauri::command]
pub async fn store_list(refresh: Option<bool>) -> Result<Vec<Value>, String> {
    blocking(move || list(&live, refresh.unwrap_or(false))).await
}

#[tauri::command]
pub(crate) async fn store_detail(v: tauri::State<'_, crate::Vault>, id: String) -> Result<Value, String> {
    let root = crate::cur_vault(&v);
    blocking(move || detail(&live, root.as_deref(), &id)).await
}

#[tauri::command]
pub(crate) async fn store_install(v: tauri::State<'_, crate::Vault>, id: String) -> Result<Value, String> {
    let root = crate::cur_vault(&v).ok_or("no vault open")?;
    blocking(move || install(&live, &root, &id)).await
}

#[tauri::command]
pub(crate) async fn store_uninstall(v: tauri::State<'_, crate::Vault>, id: String) -> Result<(), String> {
    let root = crate::cur_vault(&v).ok_or("no vault open")?;
    crate::span_timed!("store_uninstall", blocking(move || uninstall(&root, &id)).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn allowlist_is_exact_and_https_only() {
        for ok in ["https://github.com/a/b/releases/download/1/main.js", "https://api.github.com/repos/a/b/releases/latest",
                   "https://raw.githubusercontent.com/a/b/HEAD/README.md", "https://objects.githubusercontent.com/x",
                   "https://release-assets.githubusercontent.com/x", "https://GitHub.com/a"] {
            assert!(check_url(ok).is_ok(), "{ok}");
        }
        for bad in ["http://github.com/a", "https://evil.example.net/main.js", "https://github.com.evil.net/a",
                    "https://evilgithub.com/a", "https://user:pw@github.com/a", "https://github.com:8443/a",
                    "https://140.82.112.3/a", "file:///etc/passwd", "ftp://github.com/a", "notaurl", "https://gist.github.com/a"] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
        assert!(check_url("https://evil.example.net/main.js").unwrap_err().contains("not on the plugin store allowlist"));
    }

    #[test]
    fn redirects_are_rechecked() {
        let cur = check_url("https://github.com/o/r/releases/download/1/main.js").unwrap();
        assert!(next_hop(&cur, "https://release-assets.githubusercontent.com/abc", None).is_ok());
        assert!(next_hop(&cur, "/o/r/other", None).unwrap().as_str().starts_with("https://github.com/o/r/other"));
        assert!(next_hop(&cur, "https://evil.example.net/x", None).is_err());
        assert!(next_hop(&cur, "http://github.com/x", None).is_err());
        let b = parse_mirror("http://127.0.0.1:18267").unwrap();
        assert_eq!(next_hop(&cur, "http://127.0.0.1:18267/objects.githubusercontent.com/z", Some(&b)).unwrap().as_str(),
                   "https://objects.githubusercontent.com/z");
        assert!(next_hop(&cur, "http://127.0.0.1:18267/evil.example.net/z", Some(&b)).is_err());
        assert_eq!(wire_url(&cur, Some(&b)), "http://127.0.0.1:18267/github.com/o/r/releases/download/1/main.js");
        assert_eq!(wire_url(&cur, None), cur.as_str());
    }

    #[test]
    fn mirror_must_be_loopback() {
        assert!(parse_mirror("http://127.0.0.1:1/").is_some());
        assert!(parse_mirror("http://localhost:1").is_some());
        assert!(parse_mirror("http://[::1]:1").is_some());
        assert!(parse_mirror("http://10.0.0.1:1").is_none());
        assert!(parse_mirror("https://example.com").is_none());
        assert!(parse_mirror("file:///tmp").is_none());
    }

    #[test]
    fn repos_and_versions() {
        assert!(valid_repo("blacksmithgu/obsidian-dataview"));
        for bad in ["a", "a/b/c", "../b", "a/..", "a/b?x", "a /b", "/b", "a/"] {
            assert!(!valid_repo(bad), "{bad}");
        }
        assert!(version_gt("1.10.0", "1.9.9"));
        assert!(version_gt("v2.0", "1.99"));
        assert!(!version_gt("1.0.0", "1.0"));
        assert!(!version_gt("0.13.11", COMPAT_APP_VERSION));
        assert!(!version_gt("1.13.0", COMPAT_APP_VERSION));
        assert!(!version_gt("1.14.4", COMPAT_APP_VERSION));
        assert!(version_gt("1.14.5", COMPAT_APP_VERSION));
        assert!(version_gt("1.15.0", COMPAT_APP_VERSION));
    }

    #[test]
    fn compat_version_matches_the_js_api() {
        assert_eq!(COMPAT_APP_VERSION, "1.14.4");
        let js = include_str!("../../ui/plugins/api-core.js");
        assert!(js.contains(&format!("const API_VERSION = \"{COMPAT_APP_VERSION}\";")), "api-core.js API_VERSION drifted from COMPAT_APP_VERSION");
        let nd = include_str!("../../ui/plugins/node-desktop.js");
        assert!(nd.contains(&format!("const appVersion = () => \"{COMPAT_APP_VERSION}\";")), "node-desktop.js appVersion drifted from COMPAT_APP_VERSION");
    }

    #[test]
    fn list_merges_stats_and_drops_bad_rows() {
        let l = br#"[{"id":"a-plug","name":"A","author":"x","description":"d","repo":"o/a"},
                     {"id":"../evil","name":"E","author":"x","description":"d","repo":"o/e"},
                     {"id":"b","name":"B","author":"y","description":"d","repo":"o/b/../../c"}]"#;
        let st = br#"{"a-plug":{"downloads":42,"updated":1700000000000,"1.0.0":40}}"#;
        let rows = merge_list(l, Some(st)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["downloads"], 42);
        assert_eq!(rows[0]["updated"], 1700000000000u64);
        assert_eq!(merge_list(l, None).unwrap()[0]["downloads"], 0);
    }

    #[test]
    fn manifest_checks() {
        assert!(check_manifest(br#"{"id":"p","version":"1.0.0","minAppVersion":"0.15.0"}"#, "p").is_ok());
        let e = check_manifest(br#"{"id":"other","version":"1.0.0"}"#, "p").unwrap_err();
        assert!(e.contains("does not match the list id"), "{e}");
        // Templater-style manifest: minAppVersion 1.13.0 is within the 1.14.4 API
        assert!(check_manifest(br#"{"id":"templater-obsidian","name":"Templater","version":"2.16.0","minAppVersion":"1.13.0","isDesktopOnly":false}"#, "templater-obsidian").is_ok());
        assert!(check_manifest(br#"{"id":"p","version":"1","minAppVersion":"9.0.0"}"#, "p").unwrap_err().contains("needs app version"));
        assert!(check_manifest(br#"{"id":"p"}"#, "p").is_err());
        assert!(check_manifest(b"[]", "p").is_err());
    }

    // ---- install / uninstall against an in-memory "network" ----
    fn vault(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("opensidian-store-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".obsidian")).unwrap();
        root
    }

    fn net(id: &'static str, man_id: &'static str, main_size: u64, asset_host: &'static str) -> impl Fn(&str, u64, &str) -> Result<Vec<u8>, String> {
        move |u: &str, cap: u64, what: &str| {
            check_url(u)?;
            let body: Vec<u8> = if u.ends_with("community-plugins.json") {
                format!(r#"[{{"id":"{id}","name":"P","author":"me","description":"d","repo":"o/{id}"}}]"#).into_bytes()
            } else if u.ends_with("community-plugin-stats.json") {
                b"{}".to_vec()
            } else if u.ends_with("/releases/latest") {
                format!(r#"{{"tag_name":"1.0.0","published_at":"2026-01-01T00:00:00Z","assets":[
                    {{"name":"main.js","size":{main_size},"browser_download_url":"https://{asset_host}/o/{id}/releases/download/1.0.0/main.js"}},
                    {{"name":"manifest.json","size":50,"browser_download_url":"https://github.com/o/{id}/releases/download/1.0.0/manifest.json"}}]}}"#).into_bytes()
            } else if u.ends_with("manifest.json") {
                format!(r#"{{"id":"{man_id}","name":"P","version":"1.0.0","minAppVersion":"0.15.0"}}"#).into_bytes()
            } else if u.ends_with("main.js") {
                b"module.exports=class{};".to_vec()
            } else {
                return Err(format!("{what}: 404"));
            };
            if body.len() as u64 > cap { return Err(format!("refused: {what} too big")); }
            Ok(body)
        }
    }

    // the store caches are process-global: tests that use them take this lock
    static STORE_TEST: Mutex<()> = Mutex::new(());

    fn reset() {
        *LIST.lock().unwrap() = None;
        *RELEASES.lock().unwrap() = None;
    }

    // the caches are process-global: the install cases run in ONE test, in order
    #[test]
    fn install_update_uninstall_and_refusals() {
        let _g = STORE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let root = vault("inst");
        reset();
        let f = net("good-plug", "good-plug", 23, "github.com");
        let rec = install(&f, &root, "good-plug").unwrap();
        let dir = root.join(".obsidian/plugins/good-plug");
        assert_eq!(fs::read(dir.join("main.js")).unwrap(), b"module.exports=class{};");
        assert_eq!(rec["files"]["main.js"]["sha256"], sha256_hex(b"module.exports=class{};"));
        assert!(dir.join(SIDECAR).is_file());
        assert_eq!(audit(&root, "good-plug")["intact"], true);
        fs::write(dir.join("main.js"), "tampered").unwrap();
        assert_eq!(audit(&root, "good-plug")["intact"], false);
        // nothing is enabled by an install
        assert!(plugins::enabled_ids(&root).is_empty());
        plugins::set_enabled(&root, "good-plug", true).unwrap();
        uninstall(&root, "good-plug").unwrap();
        assert!(!dir.exists());
        assert!(plugins::enabled_ids(&root).is_empty());

        reset();
        let f = net("idm", "someone-else", 23, "github.com");
        assert!(install(&f, &root, "idm").unwrap_err().contains("does not match the list id"));
        assert!(!root.join(".obsidian/plugins/idm").exists());

        reset();
        let f = net("far", "far", 23, "evil.example.net");
        assert!(install(&f, &root, "far").unwrap_err().contains("not on the plugin store allowlist"));
        assert!(!root.join(".obsidian/plugins/far").exists());

        reset();
        let f = net("big", "big", plugins::MAX_MAIN_JS + 1, "github.com");
        assert!(install(&f, &root, "big").unwrap_err().contains("over the"));
        assert!(!root.join(".obsidian/plugins/big").exists());
        reset();
        let _ = fs::remove_dir_all(&root);
    }

    // a repo whose "latest" release is a beta under another id, while its HEAD manifest names
    // the store version: install + detail follow the HEAD version's tagged release
    fn beta_net(head: bool) -> impl Fn(&str, u64, &str) -> Result<Vec<u8>, String> {
        move |u: &str, _cap: u64, what: &str| {
            check_url(u)?;
            let rel = |tag: &str| format!(r#"{{"tag_name":"{tag}","published_at":"2026-01-01T00:00:00Z","assets":[
                {{"name":"main.js","size":23,"browser_download_url":"https://github.com/o/cal/releases/download/{tag}/main.js"}},
                {{"name":"manifest.json","size":50,"browser_download_url":"https://github.com/o/cal/releases/download/{tag}/manifest.json"}}]}}"#);
            let man = |id: &str, v: &str| format!(r#"{{"id":"{id}","name":"P","version":"{v}","minAppVersion":"0.15.0"}}"#);
            Ok(match u {
                _ if u.ends_with("community-plugins.json") => r#"[{"id":"cal","name":"P","author":"me","description":"d","repo":"o/cal"}]"#.into(),
                _ if u.ends_with("community-plugin-stats.json") => "{}".into(),
                "https://raw.githubusercontent.com/o/cal/HEAD/manifest.json" if head => man("cal", "1.5.10"),
                "https://api.github.com/repos/o/cal/releases/latest" => rel("2.0.0-beta"),
                "https://api.github.com/repos/o/cal/releases/tags/1.5.10" => rel("1.5.10"),
                "https://github.com/o/cal/releases/download/1.5.10/manifest.json" => man("cal", "1.5.10"),
                "https://github.com/o/cal/releases/download/2.0.0-beta/manifest.json" => man("cal-beta", "2.0.0"),
                _ if u.ends_with("/main.js") => "module.exports=class{};".into(),
                _ => return Err(format!("{what}: 404")),
            }.into_bytes())
        }
    }

    #[test]
    fn install_follows_the_head_manifest_version_not_latest() {
        let _g = STORE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let root = vault("headver");
        reset();
        let f = beta_net(true);
        let rec = install(&f, &root, "cal").unwrap();
        assert_eq!(rec["version"], "1.5.10");
        assert_eq!(rec["tag"], "1.5.10");
        assert_eq!(detail(&f, Some(&root), "cal").unwrap()["latest"]["version"], "1.5.10");
        // no HEAD manifest: the latest release, still checked against the list id
        reset();
        let f = beta_net(false);
        assert!(install(&f, &root, "cal").unwrap_err().contains("does not match the list id"));
        assert!(!valid_tag("1.0/../x") && !valid_tag("") && valid_tag("1.5.10") && valid_tag("v2.0.0-beta+1"));
        reset();
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_refuses_a_symlinked_plugin_dir() {
        let root = vault("unsym");
        let outside = root.with_extension("outside");
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "x").unwrap();
        fs::create_dir_all(root.join(".obsidian/plugins")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".obsidian/plugins/sneaky")).unwrap();
        assert!(uninstall(&root, "sneaky").is_err());
        assert!(outside.join("keep.txt").is_file());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }
}
