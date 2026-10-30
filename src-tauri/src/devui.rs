// SPDX-License-Identifier: GPL-3.0-or-later
//! goal devui — DEV-ONLY UI hot reload. This module is compiled ONLY with
//! `cargo build --features devui`; `devui` is never in `[features] default`, so a
//! default/release build contains none of it (main.rs gates every call site with
//! `#[cfg(feature = "devui")]`, test `devui_guard` checks that, and the box release
//! guard refuses any binary carrying MARKER).
//!
//! With the feature AND env OPENSIDIAN_DEV_UI=<dir>:
//! - every asset request of the app window (index.html, main.js, plugins/*.js, css,
//!   ...) is answered from <dir> instead of the embedded frontendDist: same origin
//!   (the body of Tauri's own response is swapped in the response hook, BEFORE
//!   page_csp), so the CSP header is exactly the embedded build's;
//! - a request whose decoded path leaves <dir> (`..`, an encoded `%2f`, a symlink
//!   pointing outside) is REFUSED with 403 and logged `[devui] refused`;
//! - a file missing from <dir> falls back to the embedded asset;
//! - a thread polls the mtimes/sizes under <dir> every TICK; a change that is stable
//!   for one tick (a multi-file sync is not caught half-way) bumps the reload
//!   counter r and reloads every app webview (location.reload: boot runs as on a
//!   normal start, so the workspace/tabs restore from the saved layout);
//! - INIT_JS publishes census token ` [devui:r=<r>]` in the window title.
use std::borrow::Cow;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

/// grepped for by the release guard: a binary containing it is a devui build
pub const MARKER: &str = "OPENSIDIAN_DEVUI_BUILD_NOT_FOR_RELEASE";
pub const ENV: &str = "OPENSIDIAN_DEV_UI";
pub const TICK: Duration = Duration::from_millis(250);
/// sessionStorage key carrying the reload counter across location.reload()
const KEY: &str = "__devuiR";

/// registered as the window's initialization script (runs before the page's own
/// scripts on every load): once census.js/main.js ran, append the devui token
pub const INIT_JS: &str = r#"document.addEventListener("DOMContentLoaded",()=>{try{if(typeof CENSUS!=="undefined")CENSUS.push(()=>" [devui:r="+(sessionStorage.getItem("__devuiR")||"0")+"]")}catch(_){}});"#;

static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();

/// the canonical dev UI dir, or None (embedded UI) when the env is unset/not a dir
pub fn root() -> Option<&'static Path> {
    ROOT.get_or_init(|| {
        let d = std::env::var_os(ENV)?;
        match std::fs::canonicalize(&d) {
            Ok(p) if p.is_dir() => {
                eprintln!("[devui] {MARKER}: serving the UI from {}", p.display());
                Some(p)
            }
            _ => {
                eprintln!("[devui] {ENV}={d:?} is not a directory: embedded UI");
                None
            }
        }
    })
    .as_deref()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// percent-decode a URI path; None on a malformed escape or non-UTF-8 result
pub fn pct_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let (h, l) = (hex(*b.get(i + 1)?)?, hex(*b.get(i + 2)?)?);
            out.push(h << 4 | l);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[derive(Debug, PartialEq)]
pub enum Resolve {
    File(PathBuf),
    /// not in the dev dir: keep the embedded asset
    Missing,
    /// would leave the dev dir
    Refused,
}

/// map a request path to a file under `root` (canonical). Lexical check first (a
/// `..` that climbs above root is refused even if the target does not exist), then
/// canonicalize (a symlink pointing outside root is refused).
pub fn resolve(root: &Path, uri_path: &str) -> Resolve {
    let Some(p) = pct_decode(uri_path) else { return Resolve::Refused };
    if p.contains('\0') || p.contains('\\') {
        return Resolve::Refused;
    }
    let rel = p.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let mut depth: i64 = 0;
    for c in Path::new(rel).components() {
        match c {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return Resolve::Refused;
                }
            }
            Component::RootDir | Component::Prefix(_) => return Resolve::Refused,
        }
    }
    match std::fs::canonicalize(root.join(rel)) {
        Ok(c) if !c.starts_with(root) => Resolve::Refused,
        Ok(c) if c.is_file() => Resolve::File(c),
        _ => Resolve::Missing,
    }
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "png" => "image/png",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// the response hook: swap the embedded asset's body for the dev dir's file
pub fn serve(uri_path: &str, res: &mut tauri::http::Response<Cow<'static, [u8]>>) {
    use tauri::http::{header, HeaderValue, StatusCode};
    let Some(root) = root() else { return };
    match resolve(root, uri_path) {
        Resolve::Missing => {}
        Resolve::Refused => {
            eprintln!("[devui] refused {uri_path:?}: outside {}", root.display());
            *res.status_mut() = StatusCode::FORBIDDEN;
            res.headers_mut().remove(header::CONTENT_LENGTH);
            res.headers_mut().remove(header::CONTENT_ENCODING);
            *res.body_mut() = Cow::Borrowed(b"");
        }
        Resolve::File(f) => match std::fs::read(&f) {
            Ok(b) => {
                *res.status_mut() = StatusCode::OK;
                let h = res.headers_mut();
                h.remove(header::CONTENT_LENGTH);
                h.remove(header::CONTENT_ENCODING);
                // the type of the file served, not the embedded response's: a file only the dev dir has gets
                // Tauri's index.html fallback (text/html), and a script served as html is refused
                h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime(&f)));
                h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                *res.body_mut() = Cow::Owned(b);
            }
            Err(e) => eprintln!("[devui] read {}: {e} (embedded asset kept)", f.display()),
        },
    }
}

pub type Snap = Vec<(PathBuf, Option<SystemTime>, u64)>;

/// (path, mtime, size) of every regular file under root, sorted; symlinked dirs not followed
pub fn snapshot(root: &Path) -> Snap {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                out.push((e.path(), m.modified().ok(), m.len()));
            }
        }
    }
    out.sort();
    out
}

/// files added, removed or changed between two snapshots
pub fn changed(a: &Snap, b: &Snap) -> usize {
    let mut n = a.iter().filter(|x| !b.contains(x)).count();
    n += b.iter().filter(|x| !a.iter().any(|y| y.0 == x.0)).count();
    n
}

fn reload(app: &tauri::AppHandle, r: u64) {
    use tauri::Manager;
    let js = format!("try{{sessionStorage.setItem('{KEY}','{r}')}}catch(_){{}};location.reload()");
    for (label, w) in app.webview_windows() {
        if let Err(e) = w.eval(js.as_str()) {
            eprintln!("[devui] reload {label}: {e}");
        }
    }
}

/// start the watcher (no-op without OPENSIDIAN_DEV_UI)
pub fn start(app: tauri::AppHandle) {
    let Some(root) = root() else { return };
    eprintln!("[devui] {MARKER}: watching {} every {}ms", root.display(), TICK.as_millis());
    std::thread::spawn(move || {
        let (mut base, mut r) = (snapshot(root), 0u64);
        let mut last = base.clone();
        loop {
            std::thread::sleep(TICK);
            let cur = snapshot(root);
            if cur != base && cur == last {
                r += 1;
                eprintln!("[devui] {} file(s) changed -> reload r={r}", changed(&base, &cur));
                reload(&app, r);
                base = cur.clone();
            }
            last = cur;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("devui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("ui/plugins")).unwrap();
        std::fs::write(d.join("ui/index.html"), "<html>").unwrap();
        std::fs::write(d.join("ui/main.js"), "1").unwrap();
        std::fs::write(d.join("ui/plugins/a.js"), "2").unwrap();
        std::fs::write(d.join("secret.txt"), "s").unwrap();
        std::fs::canonicalize(&d).unwrap()
    }

    #[test]
    fn devui_resolve_serves_files_under_root() {
        let d = tmp("ok");
        let r = d.join("ui");
        assert_eq!(resolve(&r, "/"), Resolve::File(r.join("index.html")));
        assert_eq!(resolve(&r, "/main.js"), Resolve::File(r.join("main.js")));
        assert_eq!(resolve(&r, "/plugins/a.js"), Resolve::File(r.join("plugins/a.js")));
        assert_eq!(resolve(&r, "/plugins/../main.js"), Resolve::File(r.join("main.js")));
        assert_eq!(resolve(&r, "/nope.js"), Resolve::Missing);
        assert_eq!(resolve(&r, "/plugins"), Resolve::Missing);
    }

    #[test]
    fn devui_resolve_refuses_traversal() {
        let d = tmp("trav");
        let r = d.join("ui");
        for p in ["/../secret.txt", "/..%2fsecret.txt", "/%2e%2e/secret.txt", "/plugins/../../secret.txt",
                  "/..%2f..%2f..%2fetc%2fpasswd", "/../nonexistent", "/a%00b", "/%zz", "/..%5csecret.txt"] {
            assert_eq!(resolve(&r, p), Resolve::Refused, "{p}");
        }
        // an encoded leading slash is still relative to root: never the host /etc, the embedded asset is kept
        assert_eq!(resolve(&r, "/%2fetc%2fpasswd"), Resolve::Missing);
        std::os::unix::fs::symlink(d.join("secret.txt"), r.join("link.txt")).unwrap();
        assert_eq!(resolve(&r, "/link.txt"), Resolve::Refused);
    }

    #[test]
    fn devui_pct_decode() {
        assert_eq!(pct_decode("/a%20b%2Fc").as_deref(), Some("/a b/c"));
        assert_eq!(pct_decode("/%4"), None);
        assert_eq!(pct_decode("/%ff"), None);
    }

    #[test]
    fn devui_snapshot_sees_edit_add_remove() {
        let d = tmp("snap");
        let r = d.join("ui");
        let a = snapshot(&r);
        assert_eq!(a.len(), 3);
        std::fs::write(r.join("main.js"), "12").unwrap();
        let b = snapshot(&r);
        assert_ne!(a, b);
        assert_eq!(changed(&a, &b), 1);
        std::fs::write(r.join("plugins/n.js"), "").unwrap();
        std::fs::remove_file(r.join("plugins/a.js")).unwrap();
        let c = snapshot(&r);
        assert_eq!(changed(&b, &c), 2);
    }

    #[test]
    fn devui_marker_is_in_this_build() {
        assert!(cfg!(feature = "devui"));
        assert!(!MARKER.is_empty() && INIT_JS.contains("[devui:r="));
    }
}
