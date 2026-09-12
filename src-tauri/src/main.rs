#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use pulldown_cmark::{html, Event, LinkType, Options, Parser, Tag, TagEnd};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use tauri::State;

mod index;
mod outline;
mod perf;
mod sandbox;
mod srcmode;
mod watcher;
use index::{link_parts, links_in, resolve, tag_spans, Graph, Index};

/* perf-index: root + in-memory Index (src/index.rs). The index replaces the
   perf-lp NoteCache: names, contents, links and backlink edges live in RAM,
   built once per vault open and kept == disk by write_note/rename_note.
   search/graph/backlinks/render never touch the filesystem. */
struct Vault {
    root: Mutex<Option<PathBuf>>,
    index: Mutex<Index>,
}

fn cur_vault(v: &State<Vault>) -> Option<PathBuf> {
    v.root.lock().unwrap().clone()
}

/// sorted note list from the index (empty when no vault open)
fn cur_notes(v: &State<Vault>) -> Vec<String> {
    v.index.lock().unwrap().names().to_vec()
}

/// open a vault: swap root + rebuild the index (one walk, one read per note)
fn open_vault(v: &State<Vault>, p: &Path) {
    let ix = span_timed!("index_build", Index::build(p), serde_json::json!({"notes": 0}));
    *v.index.lock().unwrap() = ix;
    *v.root.lock().unwrap() = Some(p.to_path_buf());
}
/// component-wise traversal check: only plain, non-hidden components allowed
fn safe_rel(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in Path::new(name).components() {
        match c {
            Component::Normal(s) if !s.to_string_lossy().starts_with('.') => out.push(s),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// S2 vault confinement: canonical parent must live under the canonical root
/// and the leaf must not be a symlink. `create` makes missing parents (what
/// write/rename need) — but only after the deepest EXISTING ancestor proved
/// to be inside the vault, so mkdir never walks through a symlinked dir.
/// Returns the canonical path; keys stay the relative names, so the
/// index==disk invariant is untouched.
fn note_path_in(root: &Path, name: &str, create: bool) -> Option<PathBuf> {
    let rel = safe_rel(name)?;
    let croot = root.canonicalize().ok()?;
    let dir = root.join(rel.parent().unwrap_or(Path::new("")));
    if create {
        let mut a = dir.as_path();
        while !a.exists() {
            a = a.parent()?;
        }
        if !a.canonicalize().ok()?.starts_with(&croot) {
            return None;
        }
        fs::create_dir_all(&dir).ok()?;
    }
    let cdir = dir.canonicalize().ok()?;
    if !cdir.starts_with(&croot) {
        return None;
    }
    let p = cdir.join(format!("{}.md", rel.file_name()?.to_string_lossy()));
    if fs::symlink_metadata(&p).map(|m| m.is_symlink()).unwrap_or(false) {
        return None;
    }
    Some(p)
}

fn note_path(v: &State<Vault>, name: &str, create: bool) -> Option<PathBuf> {
    note_path_in(&cur_vault(v)?, name, create)
}

/* ---- R29 image embeds: the vault-contained byte server ------------------
   The attacker's input here is a .md file that arrived by sync/clone/share,
   so a rendered document decides which path we read. The containment rule is
   ours (docs/requirements.md R29.5) and lives in `img_path_in` below, the
   sibling of `note_path_in`: same shape, same two-sided canonicalize, so the
   two are read together. Landlock is NOT the guard — sandbox.rs:46 allowlists
   /etc and /usr for reading and sandbox.rs:94 reports it unsupported on this
   kernel; it is defence in depth that is currently absent.                */

/// R29.8: what v1 serves — stock's list minus `svg` (scriptable, and we have
/// no sanitizer), `bmp` and `avif` (scope only). ext -> Content-Type.
const IMG_TYPES: [(&str, &str); 5] = [
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
];

/// the scheme our images are served on; the CSP names it exactly (R29 / S1)
const IMG_SCHEME: &str = "rustidian-img";

/// R29.10: the label the R29.4 banner carries in place of a REMOTE target.
/// Both engines paint the same sentence (ui/editor.js `imgEl` sets it as
/// data-miss, style.css paints it) — R29.7 says the two renderers agree, and
/// that includes this box. A constant, never the URL: see `image_html`.
const REMOTE_IMG_LABEL: &str = "remote image";

/// S5-style cap: an image bigger than this is not served (a synced vault must
/// not be able to make us allocate a multi-GB buffer inside the webview IPC).
const MAX_IMG_BYTES: u64 = 32 * 1024 * 1024;

/// percent-decode a markdown image target (R29.3: `my%20pic.png` -> `my pic.png`).
/// Strict: a `%` not followed by two hex digits is not a valid escape and the
/// whole target is refused rather than half-decoded. Decoding happens BEFORE
/// any containment check — decode-after-validate is exactly how `%2e%2e%2f`
/// becomes the escape.
fn pct_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hi = (*b.get(i + 1)? as char).to_digit(16)?;
            let lo = (*b.get(i + 2)? as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// R29.5 containment: a vault-relative image target -> the canonical file to
/// read, or None (serve nothing, say nothing — stock does not leak whether an
/// out-of-vault file exists, see R29.5).
///
/// Order matters, every step earns its place:
///  1. percent-decode first, so `%2e%2e%2f` is a plain `../` to the check below;
///  2. `safe_rel` refuses `..`, absolute paths, hidden components and (via
///     `Component::Normal`) anything with a NUL, and a scheme'd target like
///     `file:///etc/passwd` never reaches here (the renderer keeps schemes out);
///  3. extension allowlist — R29.8, lowercased;
///  4. canonicalize BOTH sides and compare, because a prefix test on the raw
///     string is not containment. Canonicalizing the ROOT too is what keeps a
///     genuinely symlinked vault (req S2) working;
///  5. the LEAF must not be a symlink — that is the symlink-inside-the-vault-
///     pointing-out case, which canonicalizing the dir alone would not catch.
fn img_path_in(root: &Path, target: &str) -> Option<PathBuf> {
    let dec = pct_decode(target)?;
    if dec.contains('\0') {
        return None;
    }
    let rel = safe_rel(&dec)?;
    let ext = rel.extension()?.to_str()?.to_ascii_lowercase();
    if !IMG_TYPES.iter().any(|(e, _)| *e == ext) {
        return None;
    }
    let croot = root.canonicalize().ok()?;
    let cdir = root.join(rel.parent().unwrap_or(Path::new(""))).canonicalize().ok()?;
    if !cdir.starts_with(&croot) {
        return None;
    }
    let p = cdir.join(rel.file_name()?);
    let m = fs::symlink_metadata(&p).ok()?;
    if m.is_symlink() || !m.is_file() || m.len() > MAX_IMG_BYTES {
        return None;
    }
    Some(p)
}

/// R29: bytes + Content-Type for a contained image, or None. The type comes
/// from OUR allowlist, never from the file, and the response is served
/// `nosniff` so a mislabeled blob cannot be re-interpreted as script.
fn serve_image(root: &Path, target: &str) -> Option<(&'static str, Vec<u8>)> {
    let p = img_path_in(root, target)?;
    let ext = p.extension()?.to_str()?.to_ascii_lowercase();
    let mime = IMG_TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m)?;
    Some((mime, fs::read(&p).ok()?))
}

/// percent-ENCODE a vault-relative path for the `rustidian-img:` URL. Unreserved
/// RFC3986 characters and `/` survive; everything else (space, `#`, `?`, `%`,
/// non-ASCII) becomes %XX, so `pct_decode` on the serving side gets the exact
/// bytes back. Without this a note named `a#b.png` would lose everything after
/// the `#` to URL fragment parsing, and `a?b.png` to the query.
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// R29.1/R29.2: the html for ONE image embed, from the index's image list.
///
/// Resolution goes through `index::resolve` — the SAME resolver `[[note]]`
/// links use — for BOTH syntaxes, so `![[pic.png]]` and `![](sub/pic.png)`
/// cannot disagree about what a name means (two resolvers for one syntax is
/// how this codebase grew its last bug). The `src` is the path the INDEX
/// holds, never the raw target: the index only ever contains real, non-hidden,
/// non-symlinked vault members, so a target that resolves to nothing gets no
/// URL at all — and the byte server re-checks containment independently.
///
/// Unresolved -> R29.4's banner, verbatim wording, target escaped. R29.5 makes
/// an out-of-vault target take this same branch, so an escape is
/// indistinguishable from a typo and leaks nothing about the filesystem.
///
/// R29.10 rides the same branch: a target carrying ANY scheme (`https://…`,
/// `http://…`, `file://…`) is never resolved and never minted — one rule, one
/// banner. The label is the literal `remote image`, NOT the target, so the URL
/// itself never reaches the DOM: an attribute a note controls is a beacon the
/// moment some later CSS/JS reads it.
fn image_html(imgs: &[String], target: &str, alt: &str) -> String {
    if url_scheme(target).is_some() {
        return format!(
            "<span class=\"imgmiss\">\u{201c}{REMOTE_IMG_LABEL}\u{201d} could not be found.</span>"
        );
    }
    let dec = pct_decode(target).unwrap_or_else(|| target.to_string());
    match index::resolve(imgs, &dec).map(|i| &imgs[i]) {
        Some(rel) => format!(
            "<img class=\"vimg\" src=\"{IMG_SCHEME}://localhost/{}\" alt=\"{}\">",
            pct_encode(rel),
            esc(alt)
        ),
        None => format!(
            "<span class=\"imgmiss\">\u{201c}{}\u{201d} could not be found.</span>",
            esc(target)
        ),
    }
}

/// is this wikilink target an image embed (`![[pic.png]]`) rather than a NOTE
/// embed (`![[Second Note]]`, out of scope — requirements.md:410)?
fn is_img_target(target: &str) -> bool {
    target
        .rsplit_once('.')
        .is_some_and(|(_, e)| index::IMG_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// S5: read a note off disk, "" when it is oversized (never part of the vault)
fn read_capped(p: &Path) -> Option<String> {
    let m = fs::symlink_metadata(p).ok()?;
    if m.len() > index::MAX_NOTE_BYTES {
        index::warn_oversized(p);
        return None;
    }
    fs::read_to_string(p).ok()
}

fn walk_dirs(dir: &Path, base: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            if let Ok(rel) = p.strip_prefix(base) {
                out.push(rel.display().to_string());
            }
            walk_dirs(&p, base, out);
        }
    }
}

#[tauri::command]
fn list_folders(v: State<Vault>) -> Vec<String> {
    let Some(root) = cur_vault(&v) else { return vec![] };
    let mut out = Vec::new();
    walk_dirs(&root, &root, &mut out);
    out.sort();
    out
}

#[tauri::command]
fn create_dir(v: State<Vault>, name: String) -> Result<(), String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let rel = safe_rel(&name).ok_or("invalid folder name")?;
    // perf-index: an empty dir holds no notes -> index unchanged.
    // S2: mkdir via note_path_in (create) so it never crosses a symlinked dir
    note_path_in(&root, &format!("{}/x", rel.display()), true)
        .map(|_| ())
        .ok_or_else(|| "outside vault".to_string())
}

#[tauri::command]
fn list_notes(v: State<Vault>, otel: Option<perf::Ctx>) -> Vec<String> {
    span_timed!(otel => "list_notes", cur_notes(&v))
}

/// R29.6/R29.7: the same image list the Rust renderer resolves against, handed
/// to live preview. ONE source of truth for "does this embed resolve?", so the
/// two engines cannot disagree about a name (the `extlink` phase exists because
/// they disagreed once before).
#[tauri::command]
fn list_images(v: State<Vault>, otel: Option<perf::Ctx>) -> Vec<String> {
    span_timed!(otel => "list_images", v.index.lock().unwrap().images().to_vec())
}

#[tauri::command]
fn read_note(v: State<Vault>, name: String, otel: Option<perf::Ctx>) -> String {
    span_timed!(otel =>
        "read_note",
        note_path(&v, &name, false)
            .and_then(|p| read_capped(&p))
            .unwrap_or_default()
    )
}

// F1 (dataloss-audit): the Result is the product. A save that did not land
// MUST reach the UI — a swallowed ENOSPC/EROFS costs every edit of a session.
#[tauri::command]
fn write_note(v: State<Vault>, name: String, content: String, otel: Option<perf::Ctx>) -> Result<(), String> {
    let bytes = content.len();
    span_timed!(otel => "write_note", write_note_inner(&v, &name, &content), serde_json::json!({"bytes": bytes}))
}

fn write_note_inner(v: &State<Vault>, name: &str, content: &str) -> Result<(), String> {
    let root = cur_vault(v).ok_or("no vault open")?;
    // R11: lock BEFORE the write — the watcher reads+compares under this
    // lock, so it never sees our bytes on disk without them in the index
    let mut ix = v.index.lock().unwrap();
    write_note_in(&root, &mut ix, name, content)
}

/* F1/F3 dataloss: the pure core of a note save, so the durability and the
   error path are testable without Tauri. Every failure is RETURNED (F1) and
   the index is upserted only after the bytes are on disk (index == disk). */
fn write_note_in(root: &Path, ix: &mut Index, name: &str, content: &str) -> Result<(), String> {
    let rel = safe_rel(name).ok_or("invalid name")?;
    // S2: parents created + confined inside note_path_in (None = outside vault)
    let p = note_path_in(root, name, true).ok_or("outside vault")?;
    write_atomic(&p, content)?;
    // index == disk: reparse this note, patch its outgoing edges
    // (a NEW key triggers a full in-memory edge rebuild inside upsert)
    ix.upsert(&rel.display().to_string(), content);
    Ok(())
}

/* F3 dataloss: durable replace, std only. A truncating fs::write releases the
   old bytes at open(2) time, so a crash or ENOSPC mid-write leaves a zero-byte
   or half-written note; and without fsync even a returned write is only page
   cache. Write a SIBLING temp (same directory => same filesystem => the rename
   is atomic; ".tmp" is not ".md", so notes_of and the watcher never index it),
   fsync it, then rename over the target. Failure leaves the OLD file intact. */
fn write_atomic(p: &Path, content: &str) -> Result<(), String> {
    use std::io::Write;
    let tmp = p.with_extension("md.tmp");
    let r = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, p)
    })();
    if r.is_err() {
        let _ = fs::remove_file(&tmp); // never leave a stray temp behind
    }
    r.map_err(|e| e.to_string())
}

/// F4 dataloss: the error a create returns when the note is already there.
/// The UI matches on it to OPEN the existing note instead of clobbering it.
const EXISTS: &str = "exists";

/* F4 dataloss: creation must never replace an existing note with a stub.
   create_new(2) is the kernel's atomic exists-check, so unlike a JS-side
   `if (!notesCache.includes(n))` there is no window in which another writer
   (git checkout, sync client, the 1000ms-stale index) can land a real file
   between the check and the truncate. */
#[tauri::command]
fn create_note(v: State<Vault>, name: String, content: Option<String>, otel: Option<perf::Ctx>) -> Result<(), String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let mut ix = v.index.lock().unwrap();
    // feedback #20: an ABSENT content is an EMPTY note, not a seeded one. The
    // default lives here as well as at ui/main.js createNote so that neither
    // side can re-mint "# name" on its own; a new note is zero bytes on disk.
    let content = content.unwrap_or_default();
    span_timed!(otel => "create_note", create_note_in(&root, &mut ix, &name, &content))
}

fn create_note_in(root: &Path, ix: &mut Index, name: &str, content: &str) -> Result<(), String> {
    use std::io::Write;
    let rel = safe_rel(name).ok_or("invalid name")?;
    let p = note_path_in(root, name, true).ok_or("outside vault")?;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists { EXISTS.to_string() } else { e.to_string() }
        })?;
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())?;
    ix.upsert(&rel.display().to_string(), content);
    Ok(())
}


/* m5 F2 rename: fs::rename old.md -> new.md inside root. Parents created,
   overwrite refused. ux-3: wikilinks updated vault-wide after the move —
   perf-index: the rewrite runs over the in-memory index (no vault read);
   only notes whose text changed are written back. Pure-ish core for tests. */
fn rename_in(root: &Path, ix: &mut Index, old: &str, new: &str) -> Result<(), String> {
    let orel = safe_rel(old).ok_or("invalid name")?;
    let nrel = safe_rel(new).ok_or("invalid name")?;
    // S2: both ends confined to the vault (symlinked source/parent -> refused)
    let op = note_path_in(root, old, false).ok_or("invalid name")?;
    if !op.is_file() {
        return Err("no such note".into());
    }
    let np = note_path_in(root, new, true).ok_or("invalid name")?;
    if np.exists() {
        return Err("target exists".into());
    }
    // rename is rare and rewrites text vault-wide: resync the index from disk
    // FIRST so a note another writer dropped in since boot (smoke seeds one;
    // LATER: file watcher) gets its [[old]] links rewritten too. One walk per
    // rename — the hot paths (search/graph/backlinks) stay disk-free.
    *ix = Index::build(root);
    fs::rename(&op, &np).map_err(|e| e.to_string())?;
    let (okey, nkey) = (orel.display().to_string(), nrel.display().to_string());
    // index==disk invariant: if the key is somehow missing, seed it from the
    // moved file rather than dropping the note
    let fallback = if ix.content(&okey).is_none() { fs::read_to_string(&np).ok() } else { None };
    for (n, c) in ix.rename(&okey, &nkey, fallback) {
        // S2: never write through a symlink swapped in since the walk
        if let Some(p) = note_path_in(root, &n, false) {
            let _ = fs::write(&p, c);
        }
    }
    Ok(())
}

#[tauri::command]
fn rename_note(v: State<Vault>, old: String, new: String, otel: Option<perf::Ctx>) -> Result<(), String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let mut ix = v.index.lock().unwrap();
    span_timed!(otel => "rename_note", rename_in(&root, &mut ix, &old, &new))
}

#[tauri::command]
fn vault_get(v: State<Vault>) -> Option<String> {
    cur_vault(&v).map(|p| p.display().to_string())
}

/* R1.6 vault persistence: ~/.rustidian.json {"last": path, "list": [paths]}.
   Written only on explicit open/create — VAULT_DIR boots (probes) never touch it. */
fn cfg_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())).join(".rustidian.json")
}

/// whole config as a Value — extra keys (sidebar_w, ...) survive rewrites
fn cfg_value() -> serde_json::Value {
    fs::read_to_string(cfg_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}))
}

fn read_cfg() -> (Option<String>, Vec<String>) {
    let v = cfg_value();
    let last = v["last"].as_str().map(String::from);
    let list = v["list"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    (last, list)
}

/// MRU push: dedup, newest first, capped at 8 — pure for testability
fn push_recent(mut list: Vec<String>, path: &str) -> Vec<String> {
    list.retain(|x| x != path);
    list.insert(0, path.to_string());
    list.truncate(8);
    list
}

fn persist_vault(p: &Path) {
    let s = p.display().to_string();
    let (_, list) = read_cfg();
    let list = push_recent(list, &s);
    let mut v = cfg_value(); // keep sidebar_w & future keys
    v["last"] = serde_json::json!(s);
    v["list"] = serde_json::json!(list);
    let _ = fs::write(cfg_path(), v.to_string());
}

/* ux-4: left sidebar width persistence (clamped 150-600, default 200 total
   = 44 ribbon + 156 #side stays when the key is absent) */
#[tauri::command]
fn get_sidebar_w() -> Option<u64> {
    cfg_value()["sidebar_w"].as_u64()
}

#[tauri::command]
fn set_sidebar_w(w: u64) {
    let mut v = cfg_value();
    v["sidebar_w"] = serde_json::json!(w.clamp(150, 600));
    let _ = fs::write(cfg_path(), v.to_string());
}

#[tauri::command]
fn recent_vaults() -> Vec<String> {
    read_cfg().1.into_iter().filter(|p| Path::new(p).is_dir()).collect()
}

#[tauri::command]
fn set_vault(v: State<Vault>, path: String, otel: Option<perf::Ctx>) -> Result<String, String> {
    span_timed!(otel => "set_vault", set_vault_inner(&v, &path))
}

fn set_vault_inner(v: &State<Vault>, path: &str) -> Result<String, String> {
    let p = PathBuf::from(path.trim());
    if !picker_allows(&p) {
        return Err(format!("not allowed as a vault: {}", p.display())); // S4
    }
    if !p.is_dir() {
        return Err(format!("not a directory: {}", p.display()));
    }
    if !sandbox::allows(&p) {
        persist_vault(&p); // next boot confines to this one instead
        return Err(format!("sandboxed to {} — vault saved, restart rustidian to open it", sandbox::confined_to().unwrap().display()));
    }
    persist_vault(&p);
    open_vault(v, &p);
    Ok(p.display().to_string())
}

#[tauri::command]
fn create_vault(v: State<Vault>, parent: String, name: String) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err("invalid vault name".into());
    }
    let p = PathBuf::from(parent.trim()).join(name);
    if !picker_allows(&p) {
        return Err(format!("not allowed as a vault: {}", p.display())); // S4
    }
    if p.exists() {
        return Err(format!("already exists: {}", p.display()));
    }
    if !sandbox::allows(Path::new(parent.trim())) {
        return Err(format!("sandboxed to {} — create the folder outside rustidian, then pick it and restart", sandbox::confined_to().unwrap().display()));
    }
    fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    fs::write(
        p.join("Welcome.md"),
        "# Welcome\n\nThis is your new vault. Notes are plain Markdown files.\nLink them with [[Wiki Links]].\n",
    )
    .map_err(|e| e.to_string())?;
    persist_vault(&p);
    open_vault(&v, &p);
    Ok(p.display().to_string())
}

/// otel (R18): frontend spans (ui/otel.js) arrive in ONE batch per 250ms — [{name, traceId, spanId,
/// parentSpanId, startMs, endMs, attrs}] — and land in the same RUSTIDIAN_OTEL file as backend spans,
/// one OTLP/JSON request line per batch. Returns false when telemetry is off so the UI stops sending.
#[tauri::command]
fn log_spans(spans: Vec<serde_json::Value>) -> bool {
    perf::ui_spans(&spans);
    perf::enabled()
}

/// graph-webgl: hidden hooks for the graph draw path. RUSTIDIAN_GRAPH_RENDERER=gl|2d forces a
/// renderer (tests); RUSTIDIAN_GRAPH_LOSE_CTX=1 makes the UI lose its WebGL context once the sim
/// settled (smoke graphgl: the 2D fallback must keep drawing).
#[tauri::command]
fn graph_renderer_pref() -> serde_json::Value {
    serde_json::json!({
        "renderer": std::env::var("RUSTIDIAN_GRAPH_RENDERER").ok(),
        "lose_ctx": std::env::var_os("RUSTIDIAN_GRAPH_LOSE_CTX").is_some(),
    })
}

/// F2 (dataloss-audit) test hook: the vault-switch race lives INSIDE the save
/// debounce window, so at 250ms it is not mechanically reproducible.
/// RUSTIDIAN_SAVE_MS widens the window for the smoke; every normal run gets
/// the stock 250ms (the env var is absent, and out-of-range values are ignored).
#[tauri::command]
fn save_debounce_ms() -> u64 {
    std::env::var("RUSTIDIAN_SAVE_MS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| (1..=60_000).contains(n))
        .unwrap_or(250)
}

/// S4: the picker commands take arbitrary absolute paths from the webview.
/// Deny system trees and any dot-component (hidden dirs, `..`); /workspace
/// and $HOME stay browsable even when they sit under a denied prefix (e.g.
/// HOME=/root). /, /home, /mnt, /media, /run/media, /tmp remain open.
fn picker_allows(p: &Path) -> bool {
    const DENY: [&str; 8] = ["/proc", "/sys", "/dev", "/etc", "/usr", "/boot", "/root", "/var"];
    if !p.is_absolute() {
        return false;
    }
    if p.components().any(|c| match c {
        Component::Normal(s) => s.to_string_lossy().starts_with('.'),
        Component::RootDir => false,
        _ => true, // `..`, `.`, prefixes
    }) {
        return false;
    }
    if p.starts_with("/workspace") {
        return true;
    }
    if let Ok(h) = std::env::var("HOME") {
        if h.len() > 1 && p.starts_with(&h) {
            return true;
        }
    }
    !DENY.iter().any(|d| p.starts_with(d))
}

#[tauri::command]
fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".into())
}

/// S1: the only way an external URL leaves the app — the webview never
/// navigates (on_navigation + main.js capture). Scheme re-checked here: the
/// DOM is attacker-influenced, the render allowlist is not the trust boundary.
fn check_external(url: &str) -> Result<(), String> {
    if ext_ok(url, false) { Ok(()) } else { Err(format!("blocked scheme: {url}")) }
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    check_external(&url)?;
    // std::process::Command only — no shell, no opener plugin. Missing xdg-open
    // (headless smoke, minimal hosts) is a plain Err, never a fallback.
    std::process::Command::new("xdg-open")
        .arg(&url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("xdg-open: {e}"))
}

#[tauri::command]
fn list_dirs(path: String) -> Vec<String> {
    if !picker_allows(Path::new(&path)) {
        return vec![]; // S4: the picker just shows ".." there
    }
    let mut v: Vec<String> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}


/// html-escape for text content and attribute values (H1 fix)
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// R10.1 link label: alias wins; `[[#H]]` shows "H"; otherwise the raw target
/// (LP keeps the '#', reading view joins with " > " like stock)
fn link_label(note: &str, anchor: &str, alias: &str, reading: bool) -> String {
    if !alias.is_empty() {
        alias.to_string()
    } else if note.is_empty() {
        anchor.trim_start_matches('#').to_string()
    } else if reading {
        format!("{note}{}", anchor.replace('#', " > "))
    } else {
        format!("{note}{anchor}")
    }
}

/// scan coalesced text for [[wikilinks]], emitting escaped anchors (H1 fix).
/// data-note = note part (empty = same note), data-anchor = heading text or
/// ^blockid (no leading '#') so the frontend can scroll after navigating.
fn linkify(buf: &str, notes: &[String], imgs: &[String], reading: bool, evs: &mut Vec<Event>) {
    let mut rest = buf;
    while let Some(i) = rest.find("[[") {
        let Some(j) = rest[i + 2..].find("]]") else { break };
        let l = &rest[i + 2..i + 2 + j];
        let (note, anchor, alias) = link_parts(l);
        // R29.1 `![[pic.png]]`: the bang belongs to the embed, so it must not
        // be emitted as text. Only IMAGE extensions take this branch — a note
        // embed `![[Second Note]]` stays the link it is today (req 410).
        if i > 0 && rest.as_bytes()[i - 1] == b'!' && anchor.is_empty() && is_img_target(note) {
            tagify(&rest[..i - 1], evs);
            let alt = if alias.is_empty() { note } else { alias };
            evs.push(Event::Html(image_html(imgs, note, alt).into()));
            rest = &rest[i + 2 + j + 2..];
            continue;
        }
        tagify(&rest[..i], evs);
        let cls = if note.is_empty() || resolve(notes, l).is_some() {
            "wiki"
        } else {
            "wiki wiki-unresolved"
        };
        evs.push(Event::Html(
            format!(
                "<a href=\"#\" class=\"{cls}\" data-note=\"{}\" data-anchor=\"{}\">{}</a>",
                esc(note),
                esc(anchor.trim_start_matches('#')),
                esc(&link_label(note, anchor, alias, reading))
            )
            .into(),
        ));
        rest = &rest[i + 2 + j + 2..];
    }
    if !rest.is_empty() {
        tagify(rest, evs);
    }
}

/// R10.3 block id: ` ^id` (letters/digits/-) at the very end of a block ->
/// (text without it, id). Stock hides it in reading view, shows a small grey
/// label in LP — both via CSS on span.blockid.
fn split_block_id(s: &str) -> Option<(&str, &str)> {
    let t = s.trim_end();
    let i = t.rfind(" ^")?;
    let id = &t[i + 2..];
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some((&t[..i], id))
}

/// tags: inline #tag -> pill anchor (label + data-tag escaped like wikilinks).
/// Only ever fed text between wikilinks, never code (linkify's callers skip
/// code blocks/spans + frontmatter), so `#tag` in code stays literal.
fn tagify(buf: &str, evs: &mut Vec<Event>) {
    let mut at = 0;
    for (a, b) in tag_spans(buf) {
        evs.push(Event::Text(buf[at..a].to_string().into()));
        let t = esc(&buf[a + 1..b]);
        evs.push(Event::Html(format!("<a href=\"#\" class=\"tag\" data-tag=\"{t}\">#{t}</a>").into()));
        at = b;
    }
    if at < buf.len() {
        evs.push(Event::Text(buf[at..].to_string().into()));
    }
}

fn render_md(content: &str, notes: &[String]) -> String {
    render_with(content, notes, &[], false)
}

/// S1 (docs/security-review.md): scheme of a markdown link/image target,
/// lowercased, or None when there is none (relative / fragment / bare word).
fn url_scheme(url: &str) -> Option<String> {
    let i = url.find(':')?;
    let s = &url[..i];
    let mut cs = s.chars();
    let ok = cs.next().is_some_and(|c| c.is_ascii_alphabetic())
        && cs.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    ok.then(|| s.to_ascii_lowercase())
}

/// S1: only these schemes survive as `<a class="ext">`; everything else
/// (javascript:, data:, file:, vbscript:, relative paths — the webview would
/// navigate the app window itself) is rendered as literal text.
///
/// R29.10: `image` is ALWAYS false-ing. No scheme whatsoever mints an `<img>`
/// — a note must not be able to cause a network fetch, so the renderer refuses
/// to mint a remote src at all and the CSP's `img-src` is only the backstop.
/// Two independent layers: widening one of them must not open the door.
/// A schemed image target takes `image_html`'s banner branch instead (R29.4).
fn ext_ok(url: &str, image: bool) -> bool {
    if image {
        return false;
    }
    matches!(url_scheme(url).as_deref(), Some("http" | "https" | "mailto"))
}

/// R29.10: is this image target REMOTE — i.e. would rendering it fetch bytes
/// over the network? http(s) only; `file:`/`data:`/`javascript:` are S1's
/// business (literal text) and are left exactly where they were.
fn is_remote_img(url: &str) -> bool {
    matches!(url_scheme(url).as_deref(), Some("http" | "https"))
}

/// reading=true: heading links join with " > " (R10.1). Block ids are emitted
/// as span.blockid in both modes; CSS decides visibility.
fn render_with(content: &str, notes: &[String], imgs: &[String], reading: bool) -> String {
    // Security (docs/security-review.md H1): .md files are untrusted, so raw
    // HTML events are demoted to text (push_html escapes Text). Wikilinks are
    // linkified at the EVENT level — label and data-note attr escaped — so our
    // anchors are the only HTML that survives. Code blocks/spans untouched.
    // NB: pulldown emits "[" "[" "Ideas" "]" "]" as SEPARATE Text events, so
    // consecutive text is coalesced in `buf` before the wikilink scan.
    let mut evs: Vec<Event> = Vec::new();
    let mut buf = String::new();
    let mut in_code = false;
    let mut demoted: Vec<Option<String>> = Vec::new(); // S1: per open link/image, Some(text tail) when rendered as text
    // R29.2: Some((target, alt so far)) while inside a vault-relative image
    let mut img_alt: Option<(String, String)> = None;
    // pulldown's own ENABLE_WIKILINKS would consume [[..]] before our pass;
    // SMART_PUNCTUATION would curl quotes/apostrophes inside link targets,
    // breaking [[name]] -> filename fidelity
    let mut opts = Options::all();
    opts.remove(Options::ENABLE_WIKILINKS);
    opts.remove(Options::ENABLE_SMART_PUNCTUATION);
    for ev in Parser::new_ext(content, opts) {
        // inside `![alt](rel.png)`: swallow the inner events, keep their text as
        // the alt attribute. Nothing here reaches linkify — an alt is not a place
        // where a wikilink or a tag renders, in either engine.
        if let Some((_, alt)) = img_alt.as_mut() {
            match ev {
                Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                Event::End(TagEnd::Image) => {
                    let (target, alt) = img_alt.take().expect("img_alt set");
                    evs.push(Event::Html(image_html(imgs, &target, &alt).into()));
                }
                _ => {}
            }
            continue;
        }
        match ev {
            // demoted raw html + plain text both join the scan buffer
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) if !in_code => {
                buf.push_str(&t)
            }
            other => {
                if !buf.is_empty() {
                    // R10.3: ` ^id` closing a paragraph / list item / heading
                    let block_end = matches!(
                        other,
                        Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::Heading(_))
                    );
                    match if block_end { split_block_id(&buf) } else { None } {
                        Some((text, id)) => {
                            linkify(text, notes, imgs, reading, &mut evs);
                            let id = esc(id);
                            evs.push(Event::Html(
                                format!("<span class=\"blockid\" data-bid=\"{id}\">^{id}</span>").into(),
                            ));
                        }
                        None => linkify(&buf, notes, imgs, reading, &mut evs),
                    }
                    buf.clear();
                }
                match other {
                    Event::Start(Tag::CodeBlock(_) | Tag::MetadataBlock(_)) => in_code = true,
                    Event::End(TagEnd::CodeBlock | TagEnd::MetadataBlock(_)) => in_code = false,
                    // S1: standard links / images — scheme allowlist. Allowed link ->
                    // our own anchor (class ext, rel noopener; main.js routes clicks to
                    // open_external). Allowed image -> pulldown's own <img> (src escaped
                    // by push_html). Anything else -> the literal `[label](url)` as TEXT.
                    Event::Start(Tag::Link { link_type, dest_url, title, .. }) => {
                        if ext_ok(&dest_url, false) {
                            let t = if title.is_empty() { String::new() } else { format!(" title=\"{}\"", esc(&title)) };
                            evs.push(Event::Html(format!("<a href=\"{}\" class=\"ext\" rel=\"noopener\"{t}>", esc(&dest_url)).into()));
                            demoted.push(None);
                        } else {
                            let auto = matches!(link_type, LinkType::Autolink | LinkType::Email);
                            if !auto { evs.push(Event::Text("[".into())); }
                            demoted.push(Some(if auto { String::new() } else { format!("]({dest_url})") }));
                        }
                        continue;
                    }
                    Event::End(TagEnd::Link) => {
                        match demoted.pop().flatten() {
                            Some(tail) => { if !tail.is_empty() { evs.push(Event::Text(tail.into())); } }
                            None => evs.push(Event::Html("</a>".into())),
                        }
                        continue;
                    }
                    // R29.2: a target with NO scheme is a vault-relative image.
                    // Its alt text arrives as the events BETWEEN Start and End,
                    // so capture them (img_alt) instead of letting them reach the
                    // wikilink scan, and emit our own <img> at the End.
                    Event::Start(Tag::Image { ref dest_url, .. })
                        if url_scheme(dest_url).is_none() =>
                    {
                        img_alt = Some((dest_url.to_string(), String::new()));
                        continue;
                    }
                    Event::Start(Tag::Image { ref dest_url, .. }) => {
                        if ext_ok(dest_url, true) {
                            // DEAD in a correct build: `ext_ok(_, true)` is false for
                            // every scheme (R29.10). This is the branch that would mint
                            // pulldown's own `<img src="https://…">` — it is what
                            // docs/negctl-remote-img/ restores, and the moment it lives
                            // the note is a beacon. Kept, and only kept, so the control
                            // has something to turn red.
                            demoted.push(None);
                        } else if is_remote_img(dest_url) {
                            // R29.10: http(s) image -> R29.4's banner, same as a missing
                            // local target. No <img>, and the URL never reaches the DOM.
                            img_alt = Some((dest_url.to_string(), String::new()));
                            continue;
                        } else {
                            // S1 unchanged: any OTHER scheme (file:, data:, javascript:)
                            // stays literal text — `imgsec`'s ZJ-File asserts exactly that.
                            evs.push(Event::Text("![".into()));
                            demoted.push(Some(format!("]({dest_url})")));
                            continue;
                        }
                    }
                    Event::End(TagEnd::Image) => {
                        if let Some(tail) = demoted.pop().flatten() {
                            evs.push(Event::Text(tail.into()));
                            continue;
                        }
                    }
                    _ => {}
                }
                evs.push(other);
            }
        }
    }
    if !buf.is_empty() {
        linkify(&buf, notes, imgs, reading, &mut evs);
    }
    let mut out = String::new();
    html::push_html(&mut out, evs.into_iter());
    // R15.11: pulldown-cmark emits "<input .../>\n" for task markers; that newline is a rendered space (~4px) between the
    // custom checkbox box and the text, which stock does not have (box->text = 16px box + margin only).
    out.replace("type=\"checkbox\"/>\n", "type=\"checkbox\"/>").replace("checked=\"\"/>\n", "checked=\"\"/>")
}

#[tauri::command]
fn render(v: State<Vault>, content: String, otel: Option<perf::Ctx>) -> String {
    // ONE lock for both lists: two `v.index.lock()` calls in one expression is
    // a deadlock on a non-reentrant Mutex, not a style question.
    let ix = v.index.lock().unwrap();
    span_timed!(otel => "render", render_with(&content, ix.names(), ix.images(), true), serde_json::json!({"bytes": content.len()}))
}

/// pure core of render_blocks: every block rendered against the same note list
fn render_blocks_with(blocks: &[String], notes: &[String], imgs: &[String]) -> Vec<String> {
    blocks.iter().map(|b| render_with(b, notes, imgs, false)).collect()
}

/* perf-lp: live preview renders every block of a note per caret move. One
   IPC round-trip + one note-list lookup for the whole batch instead of
   ~150 render calls each re-walking the vault. */
#[tauri::command]
fn render_blocks(v: State<Vault>, blocks: Vec<String>, otel: Option<perf::Ctx>) -> Vec<String> {
    let n = blocks.len();
    let ix = v.index.lock().unwrap();
    span_timed!(otel => "render_blocks", render_blocks_with(&blocks, ix.names(), ix.images()), serde_json::json!({"blocks": n}))
}

/// R12 source mode: lp rows with every marker revealed (src/srcmode.rs)
#[tauri::command]
fn highlight_blocks(blocks: Vec<String>, otel: Option<perf::Ctx>) -> Vec<String> {
    let n = blocks.len();
    span_timed!(otel => "highlight_blocks", blocks.iter().map(|b| srcmode::highlight_block(b)).collect(), serde_json::json!({"blocks": n}))
}

/// tags: per-note tag list and vault-wide tag -> note count (BTreeMap keeps
/// the JSON object sorted by tag; the UI re-sorts by count)
#[tauri::command]
fn tags(v: State<Vault>, name: String) -> Vec<String> {
    v.index.lock().unwrap().tags(&name).to_vec()
}

#[tauri::command]
fn tag_counts(v: State<Vault>) -> std::collections::BTreeMap<String, usize> {
    v.index.lock().unwrap().tag_counts()
}

#[tauri::command]
fn backlinks(v: State<Vault>, name: String, otel: Option<perf::Ctx>) -> Vec<String> {
    span_timed!(otel => "backlinks", backlinks_inner(&v, &name))
}

fn backlinks_inner(v: &State<Vault>, name: &str) -> Vec<String> {
    // perf-index: inverted edges are maintained in the index — O(1) lookup,
    // no vault read (was: re-read every note per call, 89ms @500 files)
    v.index.lock().unwrap().backlinks(name)
}

/* rsidebar: right sidebar panes served from the index (zero disk reads) */
#[tauri::command]
fn outline(v: State<Vault>, name: String, otel: Option<perf::Ctx>) -> Vec<outline::Heading> {
    span_timed!(otel => "outline", outline::parse(v.index.lock().unwrap().content(&name).unwrap_or("")))
}

#[derive(serde::Serialize)]
struct OutLink {
    text: String,
    /// resolved note name, None = unresolved (ghost)
    target: Option<String>,
}

/// Outgoing links pane: the note's [[links]] in source order, deduped
#[tauri::command]
fn outgoing(v: State<Vault>, name: String) -> Vec<OutLink> {
    let ix = v.index.lock().unwrap();
    let names = ix.names();
    let mut out: Vec<OutLink> = Vec::new();
    for l in ix.links(&name) {
        if out.iter().any(|o| o.text == *l) {
            continue;
        }
        out.push(OutLink { text: l.clone(), target: resolve(names, l).map(|j| names[j].clone()) });
    }
    out
}

#[derive(serde::Serialize)]
struct BacklinkCtx {
    note: String,
    /// (0-based line, trimmed line text) for every line linking to the note
    lines: Vec<(u32, String)>,
}

/// Backlinks pane: linking notes + the lines that carry the link
#[tauri::command]
fn backlinks_ctx(v: State<Vault>, name: String) -> Vec<BacklinkCtx> {
    let ix = v.index.lock().unwrap();
    let names = ix.names();
    ix.backlinks(&name)
        .into_iter()
        .map(|src| {
            let lines = ix
                .content(&src)
                .unwrap_or("")
                .lines()
                .enumerate()
                .filter(|(_, l)| links_in(l).iter().any(|k| resolve(names, k) == resolve(names, &name)))
                .map(|(i, l)| (i as u32, l.trim().chars().take(200).collect()))
                .collect();
            BacklinkCtx { note: src, lines }
        })
        .collect()
}

#[derive(serde::Serialize)]
struct Mention {
    note: String,
    line: u32,
    col: u32,
    len: u32,
    /// the mention's line, trimmed to 200 chars for the pane
    text: String,
}

/// R10.5 Backlinks pane "Unlinked mentions": every other note's plain-text
/// occurrences of this note's basename (case-insensitive), from the index
#[tauri::command]
fn unlinked_mentions(v: State<Vault>, name: String) -> Vec<Mention> {
    let ix = v.index.lock().unwrap();
    let base = name.rsplit('/').next().unwrap_or(&name);
    let mut out = Vec::new();
    for (n, c, _) in ix.docs() {
        if n == name {
            continue;
        }
        for (line, col, len) in index::mentions_in(c, base) {
            let text = c.lines().nth(line as usize).unwrap_or("").trim().chars().take(200).collect();
            out.push(Mention { note: n.to_string(), line, col, len, text });
        }
    }
    out
}

/// Link button: wrap the matched text in [[ ]] in `note` and save it
#[tauri::command]
fn link_mention(v: State<Vault>, note: String, target: String, line: u32, col: u32, len: u32) -> Result<(), String> {
    let base = target.rsplit('/').next().unwrap_or(&target);
    let nc = {
        let ix = v.index.lock().unwrap();
        let c = ix.content(&note).ok_or("no such note")?;
        index::link_mention(c, base, line, col, len).ok_or("mention moved — refresh the pane")?
    };
    write_note_inner(&v, &note, &nc);
    Ok(())
}

/// active right-sidebar tab, persisted as rside_tab in ~/.rustidian.json
#[tauri::command]
fn get_rside_tab() -> Option<String> {
    cfg_value()["rside_tab"].as_str().map(str::to_string)
}

#[tauri::command]
fn set_rside_tab(tab: String) {
    let mut v = cfg_value();
    v["rside_tab"] = serde_json::json!(tab);
    let _ = fs::write(cfg_path(), v.to_string());
}

/* R14: custom hotkeys, persisted as "hotkeys" in ~/.rustidian.json in the stock
   Obsidian shape {"<cmd id>":[{"modifiers":["Mod","Shift"],"key":"G"}]}:
   [] = default removed, absent id = stock default. The frontend registry
   (ui/main.js CMDS) is the single source of truth for ids + defaults. */
#[tauri::command]
fn get_hotkeys() -> serde_json::Value {
    hotkeys_clean(cfg_value()["hotkeys"].clone())
}

#[tauri::command]
fn set_hotkeys(map: serde_json::Value) {
    let mut v = cfg_value();
    v["hotkeys"] = hotkeys_clean(map);
    let _ = fs::write(cfg_path(), v.to_string());
}

/// keep only well-formed entries (id -> [{modifiers:[str], key:str}]) so a
/// hand-edited config can never wedge the dispatcher; [] survives (= removed)
fn hotkeys_clean(map: serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    if let Some(o) = map.as_object() {
        for (id, chords) in o {
            let Some(arr) = chords.as_array() else { continue };
            let ok: Vec<serde_json::Value> = arr
                .iter()
                .filter(|c| {
                    c["key"].as_str().map_or(false, |k| !k.is_empty())
                        && c["modifiers"].as_array().map_or(false, |m| m.iter().all(|x| x.is_string()))
                })
                .cloned()
                .collect();
            if ok.len() == arr.len() {
                out.insert(id.clone(), serde_json::Value::Array(ok));
            }
        }
    }
    serde_json::Value::Object(out)
}

#[tauri::command]
fn graph(v: State<Vault>, otel: Option<perf::Ctx>) -> Graph {
    span_timed!(otel => "graph", graph_inner(&v), serde_json::json!({}))
}

fn graph_inner(v: &State<Vault>) -> Graph {
    // R19: served from the index graph cache (built once per edge change)
    v.index.lock().unwrap().graph().graph.clone()
}

/// R19 (feedback #5): local-graph neighbourhood cut in Rust from the cached
/// adjacency — the UI no longer pulls the whole vault graph per recentre.
#[tauri::command]
fn graph_local(v: State<Vault>, center: String, depth: usize, inc: bool, out: bool, otel: Option<perf::Ctx>) -> Graph {
    span_timed!(otel => "graph_fetch",
        v.index.lock().unwrap().graph().local(&center, depth.min(5), inc, out),
        serde_json::json!({ "center": center, "depth": depth }))
}

#[derive(serde::Serialize, Debug, PartialEq)]
struct SearchHit {
    note: String,
    line: u32, // 0-based source line; 0 with snippet==note means a NAME match
    snippet: String,
}

/// R9.4: case-insensitive substring over note names + bodies. Hits ordered by
/// note (docs arrive sorted) then line; snippet = the matching line trimmed to
/// ~200 chars around the first hit; capped at 500 hits total.
/// query grammar: whitespace tokens; `tag:foo` / `tag:#foo` tokens restrict
/// hits to notes carrying that tag (nested match by prefix: tag:a hits a/b;
/// case-insensitive); the remaining tokens are the substring query. A
/// tag-only query yields the lines holding the #tag, else one name hit.
fn split_query(query: &str) -> (Vec<String>, String) {
    let (mut tags, mut text) = (Vec::new(), Vec::new());
    for tok in query.split_whitespace() {
        match tok.strip_prefix("tag:") {
            Some(t) => {
                let t = t.trim_start_matches('#').trim_end_matches('/').to_lowercase();
                if !t.is_empty() {
                    tags.push(t);
                }
            }
            None => text.push(tok),
        }
    }
    (tags, text.join(" "))
}

fn has_tag(tags: &[String], want: &str) -> bool {
    tags.iter().any(|t| {
        let t = t.to_lowercase();
        t == want || (t.starts_with(want) && t[want.len()..].starts_with('/'))
    })
}

fn search_docs<'a>(docs: impl IntoIterator<Item = (&'a str, &'a str, &'a [String])>, query: &str) -> Vec<SearchHit> {
    let (want, text) = split_query(query);
    let q = text.to_lowercase();
    let mut out = Vec::new();
    if q.is_empty() && want.is_empty() {
        return out;
    }
    'docs: for (name, content, tags) in docs {
        if !want.iter().all(|w| has_tag(tags, w)) {
            continue;
        }
        if q.is_empty() {
            // tag-only: show the lines carrying the tag inline (frontmatter-
            // only notes get a name hit so they still show up)
            let n0 = out.len();
            for (i, l) in content.lines().enumerate() {
                let lower = l.to_lowercase();
                if tag_spans(&lower).iter().any(|&(a, b)| want.iter().any(|w| has_tag(&[lower[a + 1..b].to_string()], w))) {
                    out.push(SearchHit { note: name.to_string(), line: i as u32, snippet: l.trim().chars().take(200).collect() });
                }
            }
            if out.len() == n0 {
                out.push(SearchHit { note: name.to_string(), line: 0, snippet: name.to_string() });
            }
            if out.len() >= 500 {
                break;
            }
            continue;
        }
        if name.to_lowercase().contains(&q) {
            out.push(SearchHit { note: name.to_string(), line: 0, snippet: name.to_string() });
        }
        for (i, l) in content.lines().enumerate() {
            let lower = l.to_lowercase();
            let Some(bpos) = lower.find(&q) else { continue };
            let t = l.trim();
            let snippet = if t.len() <= 200 {
                t.to_string()
            } else {
                // char-safe ~200-char window around the first hit
                let cpos = lower[..bpos].chars().count();
                let chars: Vec<char> = l.chars().collect();
                let start = cpos.saturating_sub(80).min(chars.len());
                let end = (cpos + 120).min(chars.len());
                chars[start..end].iter().collect()
            };
            out.push(SearchHit { note: name.to_string(), line: i as u32, snippet });
            if out.len() >= 500 {
                break 'docs;
            }
        }
        if out.len() >= 500 {
            break;
        }
    }
    out
}

#[tauri::command]
fn search(v: State<Vault>, query: String, otel: Option<perf::Ctx>) -> Vec<SearchHit> {
    span_timed!(otel => "search", search_inner(&v, &query))
}

fn search_inner(v: &State<Vault>, query: &str) -> Vec<SearchHit> {
    // perf-index: contents are resident; no per-call vault read
    search_docs(v.index.lock().unwrap().docs(), query)
}

/* R9.4 bookmarks: plain newline list in vault/.rustidian-bookmarks —
   dotfile, so walk()/notes_of never see it. Order = insertion order. */
const BM_FILE: &str = ".rustidian-bookmarks";

fn read_bookmarks(root: &Path) -> Vec<String> {
    fs::read_to_string(root.join(BM_FILE))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn toggle_in(mut list: Vec<String>, name: &str) -> Vec<String> {
    match list.iter().position(|b| b == name) {
        Some(i) => { list.remove(i); }
        None => list.push(name.to_string()),
    }
    list
}

#[tauri::command]
fn list_bookmarks(v: State<Vault>) -> Vec<String> {
    cur_vault(&v).map(|r| read_bookmarks(&r)).unwrap_or_default()
}

#[tauri::command]
fn toggle_bookmark(v: State<Vault>, name: String) -> Result<Vec<String>, String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    let list = toggle_in(read_bookmarks(&root), &name);
    let mut body = list.join("\n");
    if !body.is_empty() {
        body.push('\n');
    }
    fs::write(root.join(BM_FILE), body).map_err(|e| e.to_string())?;
    Ok(list)
}

/* R11 watcher thread: every TICK_MS walk the vault (stat only), diff against
   the last snapshot, reconcile candidates with the Index under its lock
   (src/watcher.rs), emit `vault-changed` when anything external happened.
   A root switch (set_vault/create_vault) just reseeds the snapshot silently.
   Idle cost = one read_dir walk + one stat per note per tick, no reads. */
fn spawn_watcher(app: tauri::AppHandle) {
    use tauri::{Emitter, Manager};
    std::thread::spawn(move || {
        let mut prev: Option<(PathBuf, watcher::Snapshot)> = None;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(watcher::TICK_MS));
            let v = app.state::<Vault>();
            let Some(root) = cur_vault(&v) else { prev = None; continue };
            let cur = watcher::snapshot(&root);
            let change = match &prev {
                Some((r, s)) if *r == root => {
                    let d = watcher::diff(s, &cur);
                    if d.added.is_empty() && d.removed.is_empty() && d.modified.is_empty() {
                        watcher::Change::default()
                    } else {
                        let mut ix = v.index.lock().unwrap();
                        span_timed!("watcher_reconcile", watcher::reconcile(&root, &d, &mut ix))
                    }
                }
                _ => watcher::Change::default(),
            };
            prev = Some((root, cur));
            if !change.is_empty() {
                let _ = app.emit("vault-changed", &change);
            }
        }
    });
}

fn main() {
    // VAULT_DIR (probes/tests) wins; else last persisted vault if still a dir (R1.6)
    let init = std::env::var("VAULT_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| read_cfg().0.map(PathBuf::from).filter(|p| p.is_dir()));
    // landlock: confine the whole process tree to the vault before webkit spawns
    if let (Some(p), true) = (&init, std::env::var_os("RUSTIDIAN_NO_LANDLOCK").is_none()) {
        match sandbox::enforce(p, &cfg_path()) {
            Ok(s) => eprintln!("landlock: {s:?}"),
            Err(e) => eprintln!("landlock: off ({e})"),
        }
    }
    // perf-index: one walk + read now, so the first note_open is already warm
    let index = init.as_deref().map(Index::build).unwrap_or_default();
    tauri::Builder::default()
        .manage(Vault { index: Mutex::new(index), root: Mutex::new(init) })
        .setup(|app| {
            spawn_watcher(app.handle().clone());
            Ok(())
        })
        // S1: the window is the app, never a browser — every navigation off the
        // app origin (remote http(s), javascript:, file:, ...) is denied here.
        // Config-created windows have no builder hook; an inline plugin's
        // on_navigation applies to every webview of the app.
        .plugin(
            tauri::plugin::Builder::<tauri::Wry, ()>::new("navguard")
                .on_navigation(|_, u| u.scheme() == "tauri" || u.host_str() == Some("tauri.localhost"))
                .build(),
        )
        // R29: image bytes reach the webview on our own scheme, so the
        // containment rule is a function we can unit-test (img_path_in) rather
        // than a runtime scope config. Out-of-vault targets get a bodiless 404 —
        // the renderer paints R29.4's "could not be found" banner either way, so
        // an escape is indistinguishable from a typo (R29.5).
        .register_uri_scheme_protocol(IMG_SCHEME, |ctx, req| {
            use tauri::Manager;
            let root = ctx.app_handle().state::<Vault>().root.lock().unwrap().clone();
            let target = req.uri().path().trim_start_matches('/').to_string();
            match root.as_deref().and_then(|r| serve_image(r, &target)) {
                Some((mime, body)) => tauri::http::Response::builder()
                    .status(200)
                    .header("Content-Type", mime)
                    .header("X-Content-Type-Options", "nosniff")
                    .header("Cache-Control", "no-store")
                    .body(body),
                None => tauri::http::Response::builder().status(404).body(Vec::new()),
            }
            .expect("img response")
        })
        .invoke_handler(tauri::generate_handler![
            list_notes, list_images, read_note, write_note, create_note, render, render_blocks, highlight_blocks, graph, graph_local, vault_get, set_vault,
            create_vault, home_dir, list_dirs, list_folders, create_dir, backlinks, search,
            list_bookmarks, toggle_bookmark, recent_vaults, rename_note, tags, tag_counts,
            get_sidebar_w, set_sidebar_w, log_spans, graph_renderer_pref,
            outline, outgoing, backlinks_ctx, unlinked_mentions, link_mention, get_rside_tab, set_rside_tab,
            get_hotkeys, set_hotkeys, open_external, save_debounce_ms
        ])
        .run(tauri::generate_context!())
        .expect("tauri run");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys_clean_keeps_shape_drops_garbage() {
        let v = serde_json::json!({
            "graph:open-local": [{"modifiers": ["Mod"], "key": "G"}],
            "editor:delete-paragraph": [],
            "bad-chord": [{"modifiers": "Mod", "key": "X"}],
            "no-key": [{"modifiers": []}],
            "not-array": "ctrl+x"
        });
        let c = hotkeys_clean(v);
        let o = c.as_object().unwrap();
        assert_eq!(o.len(), 2);
        assert_eq!(o["graph:open-local"][0]["key"], "G");
        assert_eq!(o["editor:delete-paragraph"].as_array().unwrap().len(), 0);
        assert_eq!(hotkeys_clean(serde_json::json!(null)), serde_json::json!({}));
    }

    /// (name, content) docs -> the (name, content, tags) stream search wants;
    /// tags empty here — tag queries are tested through a real Index
    fn docs_ref(d: &[(String, String)]) -> Vec<(&str, &str, &[String])> {
        d.iter().map(|(n, c)| (n.as_str(), c.as_str(), &[][..])).collect()
    }

    /// graph from (name, content) docs — what the index feeds build_graph
    fn graph_of(d: &[(String, String)]) -> Graph {
        let names: Vec<String> = d.iter().map(|(n, _)| n.clone()).collect();
        let links: Vec<Vec<String>> = d.iter().map(|(_, c)| links_in(c)).collect();
        let refs: Vec<&[String]> = links.iter().map(|l| l.as_slice()).collect();
        index::build_graph(&names, &refs)
    }

    fn tmp_vault(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rustidian-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        root
    }

    fn ix_graph(ix: &Index) -> Graph {
        index::build_graph(ix.names(), &ix.link_lists())
    }

    fn edge_names(g: &Graph) -> Vec<(String, String)> {
        let mut e: Vec<(String, String)> = g
            .edges
            .iter()
            .map(|&(a, b)| (g.nodes[a].name.clone(), g.nodes[b].name.clone()))
            .collect();
        e.sort();
        e
    }

    #[test]
    fn link_parts_split_note_anchor_alias() {
        use index::link_parts;
        assert_eq!(link_parts("Note"), ("Note", "", ""));
        assert_eq!(link_parts("Note#Heading"), ("Note", "#Heading", ""));
        assert_eq!(link_parts("Note#^blk1"), ("Note", "#^blk1", ""));
        assert_eq!(link_parts("Note|nick"), ("Note", "", "nick"));
        assert_eq!(link_parts("Note#H|nick"), ("Note", "#H", "nick"));
        assert_eq!(link_parts("#Local"), ("", "#Local", ""));
        assert_eq!(link_parts("sub/N#a|b|c"), ("sub/N", "#a", "b|c"));
        let notes = ["A".to_string(), "sub/B".to_string()];
        assert_eq!(resolve(&notes, "A#h"), Some(0));
        assert_eq!(resolve(&notes, "B#^id|alias"), Some(1));
        assert_eq!(resolve(&notes, "A|alias"), Some(0));
        assert_eq!(resolve(&notes, "#h"), None);
        assert_eq!(resolve(&notes, "Ghost#h"), None);
    }

    #[test]
    fn backlinks_and_graph_resolve_through_suffixes() {
        let root = tmp_vault("sfx");
        fs::write(root.join("T.md"), "# H\ntext ^blk").unwrap();
        fs::write(root.join("A.md"), "[[T#H]] [[T#^blk|nick]] [[#Own]]").unwrap();
        fs::write(root.join("sub/B.md"), "[[T|alias]] [[Ghost#H]] [[Ghost|g]]").unwrap();
        let ix = Index::build(&root);
        assert_eq!(ix.backlinks("T"), ["A", "sub/B"]);
        let g = ix_graph(&ix);
        // one ghost node "Ghost" (note part), no node for [[#Own]]
        assert_eq!(g.nodes.len(), 4);
        assert!(!g.nodes[3].resolved && g.nodes[3].name == "Ghost");
        let e = edge_names(&g);
        assert!(e.contains(&("A".to_string(), "T".to_string())));
        assert_eq!(e.iter().filter(|(s, _)| s == "A").count(), 1);
        assert_eq!(e.iter().filter(|(s, _)| s == "sub/B").count(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_rewrites_anchor_and_alias_links() {
        use index::rewrite_links;
        assert_eq!(
            rewrite_links("[[Old#h|alias]] [[Old#^b]] [[Old|a|b]] [[Older]]", "Old", "New", true),
            ("[[New#h|alias]] [[New#^b]] [[New|a|b]] [[Older]]".to_string(), true)
        );
        let root = tmp_vault("ra");
        fs::write(root.join("Old.md"), "# h").unwrap();
        fs::write(root.join("L.md"), "x [[Old#h|alias]] y").unwrap();
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        assert_eq!(fs::read_to_string(root.join("L.md")).unwrap(), "x [[New#h|alias]] y");
        assert_eq!(ix.backlinks("New"), ["L"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn index_builds_from_temp_vault() {
        let root = tmp_vault("ix");
        fs::write(root.join("A.md"), "[[B]] and [[Nested]] and [[Ghost]]").unwrap();
        fs::write(root.join("B.md"), "back to [[A]] self [[B]]").unwrap();
        fs::write(root.join("sub/Nested.md"), "[[A]] twice [[A]]").unwrap();
        fs::write(root.join(".hidden.md"), "[[A]]").unwrap(); // dotfiles never indexed
        let ix = Index::build(&root);
        assert_eq!(ix.names(), ["A", "B", "sub/Nested"]);
        assert_eq!(ix.content("B"), Some("back to [[A]] self [[B]]"));
        assert_eq!(ix.links("A"), ["B", "Nested", "Ghost"]);
        // raw tokens are kept in links(); resolve() strips #anchor / |alias (R10)
        assert_eq!(ix.backlinks("A"), ["B", "sub/Nested"]);
        assert_eq!(ix.backlinks("B"), ["A"]); // B's self-link excluded
        assert_eq!(ix.backlinks("sub/Nested"), ["A"]); // basename resolve
        assert!(ix.backlinks("Ghost").is_empty());
        // graph: unresolved [[Ghost]] stays a ghost node
        let g = ix_graph(&ix);
        assert_eq!(g.nodes.len(), 4);
        assert!(!g.nodes[3].resolved && g.nodes[3].name == "Ghost");
        // search sees every body without touching disk: mutate disk behind it
        fs::write(root.join("B.md"), "changed on disk").unwrap();
        let hits = search_docs(ix.docs(), "back to");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].note, "B");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn graph_cache_invalidates_and_local_bfs() {  // R19
        let root = tmp_vault("gc");
        fs::write(root.join("A.md"), "[[B]] [[Ghost]]").unwrap();
        fs::write(root.join("B.md"), "[[C]]").unwrap();
        fs::write(root.join("C.md"), "[[A]]").unwrap();
        fs::write(root.join("D.md"), "[[C]]").unwrap();
        let mut ix = Index::build(&root);
        // cached graph == the from-scratch build
        let fresh = ix_graph(&ix);
        assert_eq!(ix.graph().graph, fresh);
        assert_eq!(ix.graph().out[0], [1, 4]); // A -> B, Ghost
        assert_eq!(ix.graph().inc[2], [1, 3]); // B, D -> C
        // local: depth 1 out+in around B = B, C, A; edges among them keep every direction
        let l = ix.graph().local("B", 1, true, true);
        assert_eq!(l.nodes.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), ["B", "C", "A"]);
        assert_eq!(edge_names(&l).len(), 3); // A->B, B->C, C->A
        let l = ix.graph().local("B", 1, false, true); // outgoing only
        assert_eq!(l.nodes.len(), 2);
        let l = ix.graph().local("B", 2, true, true); // 2 hops reach D + Ghost
        assert_eq!(l.nodes.len(), 5);
        assert!(ix.graph().local("Nope", 1, true, true).nodes.is_empty());
        // plain save (links unchanged) keeps the cache; a link edit / new note / remove rebuilds it
        ix.upsert("D", "[[C]] more text");
        assert!(ix.graph.is_some());
        ix.upsert("D", "[[A]]");
        assert!(ix.graph.is_none());
        assert_eq!(ix.graph().inc[0], [2, 3]);
        ix.upsert("Ghost", "");
        assert!(ix.graph().graph.nodes.iter().all(|n| n.resolved));
        ix.remove("Ghost");
        assert!(!ix.graph().graph.nodes.iter().all(|n| n.resolved));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn write_updates_search_backlinks_graph() {
        let root = tmp_vault("ixw");
        fs::write(root.join("A.md"), "[[B]] [[Ghost]]").unwrap();
        fs::write(root.join("B.md"), "plain").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(ix.backlinks("B"), ["A"]);
        // edit A: drop the B link, add a C link (C absent -> unresolved)
        ix.upsert("A", "now [[C]] only, needle here");
        assert!(ix.backlinks("B").is_empty());
        assert_eq!(search_docs(ix.docs(), "needle").len(), 1);
        assert!(search_docs(ix.docs(), "ghost").is_empty());
        let g = ix_graph(&ix);
        assert_eq!(edge_names(&g), [("A".to_string(), "C".to_string())]);
        assert!(g.nodes.iter().any(|n| n.name == "C" && !n.resolved));
        // new note C appears: A's dangling link now resolves, ghost node gone
        ix.upsert("C", "[[A]]");
        assert_eq!(ix.names(), ["A", "B", "C"]);
        assert_eq!(ix.backlinks("C"), ["A"]);
        assert_eq!(ix.backlinks("A"), ["C"]);
        let g = ix_graph(&ix);
        assert!(g.nodes.iter().all(|n| n.resolved));
        assert_eq!(
            edge_names(&g),
            [("A".to_string(), "C".to_string()), ("C".to_string(), "A".to_string())]
        );
        // B links to C too: edge list stays sorted + deduped
        ix.upsert("B", "[[C]] [[C]]");
        assert_eq!(ix.backlinks("C"), ["A", "B"]);
        ix.upsert("B", "");
        assert_eq!(ix.backlinks("C"), ["A"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_moves_keys_edges_and_targets() {
        let root = tmp_vault("ixr");
        fs::write(root.join("Old.md"), "[[B]]").unwrap();
        fs::write(root.join("B.md"), "see [[Old]] and [[Old|nick]]").unwrap();
        fs::write(root.join("sub/C.md"), "[[Old]] [[Ghost]]").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(ix.backlinks("Old"), ["B", "sub/C"]);
        rename_in(&root, &mut ix, "Old", "sub/New").unwrap();
        // keys moved
        assert_eq!(ix.names(), ["B", "sub/C", "sub/New"]);
        assert!(ix.content("Old").is_none());
        assert_eq!(ix.content("sub/New"), Some("[[B]]"));
        // link targets rewritten in memory AND on disk (exact-path links get
        // the full new path — same rewrite_links rule as before)
        assert_eq!(ix.content("B"), Some("see [[sub/New]] and [[sub/New|nick]]"));
        assert_eq!(fs::read_to_string(root.join("B.md")).unwrap(), "see [[sub/New]] and [[sub/New|nick]]");
        assert_eq!(ix.content("sub/C"), Some("[[sub/New]] [[Ghost]]"));
        assert_eq!(ix.links("sub/C"), ["sub/New", "Ghost"]);
        // edges follow the new key
        assert!(ix.backlinks("Old").is_empty());
        assert_eq!(ix.backlinks("sub/New"), ["B", "sub/C"]);
        assert_eq!(ix.backlinks("B"), ["sub/New"]);
        // unresolved stays unresolved in the graph
        let g = ix_graph(&ix);
        assert!(g.nodes.iter().any(|n| n.name == "Ghost" && !n.resolved));
        assert!(!g.nodes.iter().any(|n| n.name == "Old"));
        // index == disk after the whole dance
        let fresh = Index::build(&root);
        assert_eq!(fresh.names(), ix.names());
        for n in ix.names() {
            assert_eq!(fresh.content(n), ix.content(n), "{n}");
            assert_eq!(fresh.backlinks(n), ix.backlinks(n), "{n}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_resyncs_externally_added_notes() {
        // smoke.sh fast ux seeds a linker note ON DISK after boot and expects
        // the rename to rewrite it: rename resyncs the index from disk first
        let root = tmp_vault("ixx");
        fs::write(root.join("Old.md"), "# Old").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(ix.names(), ["Old"]);
        fs::write(root.join("Linker.md"), "see [[Old]] and [[Old|nick]] here").unwrap();
        assert!(ix.content("Linker").is_none()); // invisible until the resync
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        assert_eq!(ix.names(), ["Linker", "New"]);
        assert_eq!(
            fs::read_to_string(root.join("Linker.md")).unwrap(),
            "see [[New]] and [[New|nick]] here"
        );
        assert_eq!(ix.backlinks("New"), ["Linker"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_link_forms_and_block_ids() {
        let notes = ["T".to_string()];
        // LP: raw target keeps '#', alias-only, [[#H]] drops '#'; data-note = note part
        let h = render_md("[[T#Gamma]] [[T#^b1|nick]] [[#Local]] [[Ghost#H]]", &notes);
        assert!(h.contains(r#"class="wiki" data-note="T" data-anchor="Gamma">T#Gamma</a>"#), "{h}");
        assert!(h.contains(r#"class="wiki" data-note="T" data-anchor="^b1">nick</a>"#), "{h}");
        assert!(h.contains(r#"class="wiki" data-note="" data-anchor="Local">Local</a>"#), "{h}");
        assert!(h.contains(r#"class="wiki wiki-unresolved" data-note="Ghost" data-anchor="H">Ghost#H</a>"#), "{h}");
        // reading: " > " separator, alias unchanged
        let r = render_with("[[T#Gamma]] [[T#^b1|nick]] [[#Local]]", &notes, &[], true);
        assert!(r.contains(">T &gt; Gamma</a>"), "{r}");
        assert!(r.contains(">nick</a>") && r.contains(">Local</a>"), "{r}");
        // block ids: paragraph / list item / heading tails become span.blockid;
        // mid-line ^x and code stay literal
        let b = render_md("para text ^blk1\n\n- item ^i-2\n\n# Head ^h3\n\nnot ^mid here\n\n`code ^c`", &notes);
        assert!(b.contains(r#"para text<span class="blockid" data-bid="blk1">^blk1</span></p>"#), "{b}");
        assert!(b.contains(r#"item<span class="blockid" data-bid="i-2">^i-2</span></li>"#), "{b}");
        assert!(b.contains(r#"Head<span class="blockid" data-bid="h3">^h3</span></h1>"#), "{b}");
        assert!(b.contains("not ^mid here</p>") && b.contains("code ^c</code>"), "{b}");
        assert_eq!(split_block_id("x ^bad id"), None);
        assert_eq!(split_block_id("x ^ok-1  "), Some(("x", "ok-1")));
    }

    #[test]
    fn unlinked_mentions_detected_and_linked() {
        use index::{link_mention, mentions_in};
        let c = "Link Target once, link target twice\nalready [[Link Target]] here but Link Target too\n```\nLink Target in code\n```\nno hit\n";
        assert_eq!(
            mentions_in(c, "Link Target"),
            [(0, 0, 11), (0, 18, 11), (1, 33, 11)]
        );
        assert!(mentions_in(c, "").is_empty());
        // Link button: wrap exactly the matched text, case preserved
        let nc = link_mention(c, "Link Target", 0, 18, 11).unwrap();
        assert!(nc.starts_with("Link Target once, [[link target]] twice\n"), "{nc}");
        assert_eq!(mentions_in(&nc, "Link Target").len(), 2);
        assert!(link_mention(c, "Link Target", 0, 5, 11).is_none()); // stale offsets
        // through the index (basename of a nested note)
        let root = tmp_vault("um");
        fs::write(root.join("sub/Deep Note.md"), "# t").unwrap();
        fs::write(root.join("A.md"), "see deep note and [[Deep Note]]").unwrap();
        let ix = Index::build(&root);
        let hits: Vec<_> = ix.docs().flat_map(|(n, c, _)| mentions_in(c, "Deep Note").into_iter().map(move |h| (n.to_string(), h))).collect();
        assert_eq!(hits, [("A".to_string(), (0, 4, 9))]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_marks_unresolved() {
        let notes = vec!["Ideas".to_string(), "sub/Nested".to_string()];
        let h = render_md("[[Ideas]] [[Nested]] [[Nope]]", &notes);
        assert!(h.contains(r#"class="wiki" data-note="Ideas""#));
        assert!(h.contains(r#"class="wiki" data-note="Nested""#)); // basename resolve
        assert!(h.contains(r#"class="wiki wiki-unresolved" data-note="Nope""#));
    }

    #[test]
    fn render_neutralizes_raw_html() {
        // H1 (docs/security-review.md): raw/inline HTML must render inert
        let h = render_md("hi <img src=x onerror=alert(1)> there", &[]);
        assert!(!h.contains("<img"), "raw inline html leaked: {h}");
        assert!(h.contains("&lt;img"));
        let h = render_md("<script>alert(1)</script>", &[]);
        assert!(!h.contains("<script"), "html block leaked: {h}");
        // ...but code blocks still render their (escaped) content normally
        let h = render_md("```\n<b>code</b>\n```", &[]);
        assert!(h.contains("<pre><code>") && h.contains("&lt;b&gt;"));
    }

    #[test]
    fn render_keeps_http_mailto_anchors() {
        // S1: allowed schemes -> our own anchor: class ext, rel noopener
        let h = render_md("[ext](https://example.com/a?b=1&c=2) [m](mailto:a@b.c) [h](HTTP://x.y)", &[]);
        assert!(h.contains(r#"<a href="https://example.com/a?b=1&amp;c=2" class="ext" rel="noopener">ext</a>"#), "{h}");
        assert!(h.contains(r#"<a href="mailto:a@b.c" class="ext" rel="noopener">m</a>"#), "{h}");
        assert!(h.contains(r#"<a href="HTTP://x.y" class="ext" rel="noopener">h</a>"#), "{h}");
        // autolink + title survive too
        let h = render_md(r#"<https://a.b> [t](https://c.d "Ti")"#, &[]);
        assert!(h.contains(r#"class="ext" rel="noopener">https://a.b</a>"#), "{h}");
        assert!(h.contains(r#"rel="noopener" title="Ti">t</a>"#), "{h}");
        // reading mode identical
        assert!(render_with("[x](https://q)", &[], &[], true).contains(r#"class="ext""#));
        // R29.10 (operator decision, 2026-09-12): a REMOTE image mints NOTHING.
        // The same URL as a LINK still works — that is the point of scoping the
        // refusal to images, and extlink asserts it end to end. The <img> that
        // used to be asserted here is now the mutation of
        // docs/negctl-remote-img/; the rule lives in
        // remote_images_never_mint_an_img_element.
        let h = render_md("![pic](https://img.x/a.png)", &[]);
        assert!(!h.contains("<img"), "a remote image minted an element: {h}");
        assert!(!h.contains("img.x"), "a remote URL reached the DOM: {h}");
        assert!(!h.contains(IMG_SCHEME), "remote image must not ride our scheme: {h}");
    }

    /// R29.6 — what the index calls a vault image. This list is what BOTH
    /// renderers resolve against, so anything wrongly in it is a URL the
    /// webview will ask for: hidden dirs, symlinks and non-image extensions
    /// must stay out, and the extension set must match the byte server's.
    #[test]
    fn index_lists_vault_images_only() {
        use std::os::unix::fs::symlink;
        // the renderer's allowlist and the byte server's table cannot drift:
        // a name the index resolves but serve_image refuses is a broken image
        let served: Vec<&str> = IMG_TYPES.iter().map(|(e, _)| *e).collect();
        assert_eq!(served, index::IMG_EXTS.to_vec(), "IMG_TYPES vs index::IMG_EXTS");
        let root = tmp_vault("imgidx");
        let outside = std::env::temp_dir().join(format!("rustidian-imgout-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.png"), b"out").unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        for (p, b) in [
            ("A.md", "note"),
            ("pic.png", "x"),
            ("UP.JPG", "x"),
            ("sub/nested.webp", "x"),
            (".hidden/h.png", "x"),
            ("doc.svg", "x"),
            ("notes.txt", "x"),
        ] {
            fs::write(root.join(p), b).unwrap();
        }
        symlink(outside.join("secret.png"), root.join("link.png")).unwrap();
        let ix = Index::build(&root);
        assert_eq!(ix.images(), ["UP.JPG", "pic.png", "sub/nested.webp"], "{:?}", ix.images());
        assert_eq!(ix.names(), ["A"], "images must never become notes");
        let _ = fs::remove_dir_all(&outside);
    }

    /// the two halves must MEET: whatever url the renderer emits, the byte
    /// server must serve — and only that. A pct_encode/pct_decode mismatch
    /// would show up here as a broken image in the app and nowhere else.
    #[test]
    fn rendered_img_url_round_trips_through_the_byte_server() {
        let root = tmp_vault("imgtrip");
        fs::create_dir_all(root.join("sub")).unwrap();
        let png = b"\x89PNG\r\n\x1a\n-solid";
        for p in ["pic.png", "my pic.png", "sub/nested.webp"] {
            fs::write(root.join(p), png).unwrap();
        }
        let ix = Index::build(&root);
        for md in ["![](pic.png)", "![[pic.png]]", "![](my%20pic.png)", "![[my pic.png]]", "![[nested.webp]]"] {
            let h = render_with(md, &[], ix.images(), true);
            let i = h.find("src=\"").unwrap_or_else(|| panic!("no img for {md}: {h}")) + 5;
            let url = h[i..].split('"').next().unwrap();
            let target = url.strip_prefix(&format!("{IMG_SCHEME}://localhost/")).expect(url);
            // exactly the path the protocol handler passes to serve_image
            let (mime, bytes) = serve_image(&root, target).unwrap_or_else(|| panic!("{md} -> {url} served nothing"));
            assert_eq!(bytes, png, "{md}");
            assert!(mime.starts_with("image/"), "{md}: {mime}");
        }
    }

    /// R29.1/R29.2/R29.3/R29.4/R29.5 — the READING-MODE renderer. Both syntaxes
    /// go through ONE resolver (index::resolve over the index's image list), so
    /// what they cannot resolve they cannot emit a URL for.
    #[test]
    fn render_images_resolve_through_the_index() {
        let imgs: Vec<String> = ["pic.png", "sub/nested.png", "my pic.png", "q\"t.png", "up.JPG"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let src = |h: &str| -> String {
            let i = h.find("src=\"").expect("no src") + 5;
            h[i..].split('"').next().unwrap().to_string()
        };
        // images resolve against the INDEX IMAGE LIST, not the note list
        let rmd = |c: &str| render_with(c, &[], &imgs, false);
        // R29.2 relative path, alt preserved and escaped
        let h = rmd("![a pic](pic.png)");
        assert_eq!(src(&h), format!("{IMG_SCHEME}://localhost/pic.png"), "{h}");
        assert!(h.contains(r#"alt="a pic""#), "{h}");
        // subfolder: full path AND bare basename resolve to the same file
        let full = rmd("![](sub/nested.png)");
        let bare = rmd("![](nested.png)");
        assert_eq!(src(&full), format!("{IMG_SCHEME}://localhost/sub/nested.png"), "{full}");
        assert_eq!(src(&bare), src(&full), "basename must resolve like the path");
        // R29.1: the wikilink embed form agrees with the markdown form, exactly
        assert_eq!(src(&rmd("![[pic.png]]")), src(&rmd("![](pic.png)")));
        assert_eq!(src(&rmd("![[nested.png]]")), src(&full));
        assert!(!rmd("![[pic.png]]").contains('!'), "bang leaked as text");
        // R29.3: %20 resolves and comes back percent-ENCODED in the url
        let h = rmd("![](my%20pic.png)");
        assert_eq!(src(&h), format!("{IMG_SCHEME}://localhost/my%20pic.png"), "{h}");
        // R29.3 second half: a RAW space is not an image at all (pulldown), text
        let h = rmd("![](my pic.png)");
        assert!(!h.contains("<img") && h.contains("![](my pic.png)"), "{h}");
        // case-insensitive extension is still an embed target
        assert!(rmd("![[up.JPG]]").contains("<img"), "ext case");
        // a quote in a vault filename cannot break out of the src attribute
        assert!(src(&rmd("![[q\"t.png]]")).ends_with("q%22t.png"));
        // R29.5: an ESCAPE is indistinguishable from a typo, and serves no url
        for t in [
            "../../../etc/passwd",
            "../outside.png",
            "/etc/passwd",
            "..%2f..%2fpic.png",
            "%2e%2e%2fpic.png",
            ".hidden/pic.png",
            "sub/../../pic.png",
        ] {
            for h in [rmd(&format!("![]({t})")), rmd(&format!("![[{t}]]"))] {
                assert!(!h.contains("<img"), "{t} produced an image: {h}");
                assert!(!h.contains(IMG_SCHEME), "{t} produced a url: {h}");
            }
        }
        // R29.4: missing -> stock's banner, verbatim, and NO element to load
        for m in ["![](nope.png)", "![[nope.png]]"] {
            let h = rmd(m);
            assert!(h.contains("\u{201c}nope.png\u{201d} could not be found."), "{m}: {h}");
            assert!(!h.contains("<img") && !h.contains(IMG_SCHEME), "{m}: {h}");
        }
        // the banner escapes the target it echoes back
        let h = rmd(r#"![[x" onerror="alert(1).png]]"#);
        assert!(!h.contains("onerror=\"alert") && h.contains("&quot;"), "{h}");
        // note embeds are NOT image embeds (requirements.md:410): still a link
        let h = render_md("![[Second Note]]", &["Second Note".to_string()]);
        assert!(h.contains(r#"class="wiki""#) && !h.contains("<img"), "{h}");
        // both Rust modes (reading view, live-preview block batch) agree
        let note = "![[pic.png]] ![](sub/nested.png) ![](nope.png)";
        assert_eq!(
            render_with(note, &[], &imgs, true),
            render_with(note, &[], &imgs, false),
            "reading view and block render disagree"
        );
    }

    #[test]
    fn render_drops_bad_schemes_to_text() {
        // S1: javascript:/data:/file:/vbscript:/relative -> literal text, no anchor at all
        for src in [
            "[js](javascript:alert(1))",
            "[d](data:text/html,<b>x</b>)",
            "[f](file:///etc/passwd)",
            "[v](vbscript:msgbox)",
            "[r](../../etc/passwd)",
            "[frag](#h)",
            "[sp]( javascript:alert(1))",
            "[tab](java\tscript:alert(1))",
            "<javascript:alert(1)>",
        ] {
            let h = render_md(src, &[]);
            assert!(!h.contains("<a "), "anchor leaked for {src}: {h}");
            assert!(!h.contains("href"), "href leaked for {src}: {h}");
        }
        let h = render_md("[js](javascript:alert(1))", &[]);
        assert!(h.contains("[js](javascript:alert(1))"), "literal expected: {h}");
        let h = render_md("![x](file:///etc/hostname) ![y](javascript:alert(1)) ![z](data:image/png;base64,AAAA)", &[]);
        assert!(!h.contains("<img"), "img leaked: {h}");
        assert!(h.contains("![x](file:///etc/hostname)"), "{h}");
        // label text still escaped (push_html Text), url text too
        let h = render_md("[<b>](javascript:'<s>')", &[]);
        assert!(!h.contains("<b>") && !h.contains("<s>"), "{h}");
        // wikilinks / tags unaffected
        let h = render_md("[[Ideas]] #tag [x](javascript:1)", &["Ideas".into()]);
        assert!(h.contains(r#"class="wiki""#) && h.contains(r#"class="tag""#), "{h}");
    }

    #[test]
    fn open_external_rejects_non_http() {
        for u in ["javascript:alert(1)", "file:///etc/passwd", "data:text/html,x", "vbscript:x", "../x", "#h", "", "ftp://a.b"] {
            assert!(check_external(u).is_err(), "accepted {u}");
            assert!(open_external(u.to_string()).is_err(), "command accepted {u}");
        }
        for u in ["https://example.com", "http://a.b/c", "mailto:x@y.z", "HTTPS://A.B"] {
            assert!(check_external(u).is_ok(), "rejected {u}");
        }
        assert_eq!(url_scheme("javascript:x").as_deref(), Some("javascript"));
        assert_eq!(url_scheme("no-scheme/path"), None);
        assert_eq!(url_scheme("1http://x"), None);
        assert!(!ext_ok("mailto:a@b", true)); // images: no scheme at all (R29.10)
        for u in ["https://example.com/x.png", "http://example.com/y.png", "HTTPS://A.B/z.png"] {
            assert!(!ext_ok(u, true), "an image was allowed a remote src: {u}");
            assert!(ext_ok(u, false), "the same url must still open as a LINK: {u}");
        }
    }

    /// R29.10 layer 1 (the renderer). The CSP is layer 2 and is pinned by
    /// csp_is_exactly_this_string_and_no_remote_image_origin — neither test
    /// knows about the other, which is the whole point of two layers.
    ///
    /// MUTATE TO CHECK: restore the remote branch in `ext_ok`
    /// (`matches!(url_scheme(url).as_deref(), Some("http" | "https")) || …`)
    /// and this goes red on `<img src="https://example.com/x.png">`.
    #[test]
    fn remote_images_never_mint_an_img_element() {
        let md = "![a](https://example.com/x.png)\n\n![b](http://example.com/y.png)\n\n\
                  ![c](HTTPS://EXAMPLE.COM/z.png)\n";
        for reading in [true, false] {
            // the vault DOES hold an x.png: a remote target must not be able to
            // borrow a local file's bytes either (resolve is never reached).
            let h = render_with(md, &[], &["x.png".to_string()], reading);
            let lo = h.to_ascii_lowercase(); // the third target is UPPERCASE on purpose
            assert!(!h.contains("<img"), "reading={reading}: a remote target minted an <img>: {h}");
            assert!(!lo.contains("http"), "reading={reading}: a remote URL reached the DOM: {h}");
            assert!(!lo.contains("example.com"), "reading={reading}: the host leaked: {h}");
            assert_eq!(
                h.matches("class=\"imgmiss\"").count(),
                3,
                "reading={reading}: want R29.4's banner for each remote image: {h}"
            );
            assert!(
                h.contains("\u{201c}remote image\u{201d} could not be found."),
                "reading={reading}: the banner is not R29.4's wording: {h}"
            );
            // S4/extlink is NOT affected: the same url is still a working link.
            let l = render_with("[text](https://example.com/x.png)", &[], &[], reading);
            assert!(
                l.contains("<a href=\"https://example.com/x.png\" class=\"ext\""),
                "reading={reading}: a non-image external link stopped working: {l}"
            );
        }
        // S1 unchanged: any other scheme is still literal text, not a banner
        // (smoke `imgsec`/ZJ-File asserts the census publishes no [xi:] token).
        let f = render_with("![d](file:///etc/passwd)", &[], &[], true);
        assert!(!f.contains("imgmiss") && !f.contains("<img"), "file:// image changed shape: {f}");
        assert!(f.contains("![d](file:///etc/passwd)"), "file:// image is no longer literal text: {f}");
    }

    #[test]
    fn render_escapes_wikilink_attr() {
        // attribute breakout: [[x" onmouseover=...]] must stay inside data-note
        let h = render_md(r#"[[x" onmouseover="alert(1)]]"#, &[]);
        assert!(!h.contains(r#"" onmouseover="#), "attr breakout: {h}");
        assert!(h.contains("data-note=\"x&quot; onmouseover=&quot;alert(1)\""));
        // wikilinks inside code blocks are NOT linkified
        let h = render_md("```\n[[Ideas]]\n```", &["Ideas".to_string()]);
        assert!(!h.contains("class=\"wiki\""));
    }

    #[test]
    fn render_tags_become_pills() {
        let h = render_md("see #alpha and #beta/gamma here", &[]);
        assert!(h.contains(r##"<a href="#" class="tag" data-tag="alpha">#alpha</a>"##), "{h}");
        assert!(h.contains(r##"<a href="#" class="tag" data-tag="beta/gamma">#beta/gamma</a>"##), "{h}");
        // tag next to a wikilink: both anchors, text between preserved
        let h = render_md("[[Ideas]] #x", &["Ideas".to_string()]);
        assert!(h.contains("class=\"wiki\"") && h.contains("data-tag=\"x\""), "{h}");
        // never inside fenced blocks, code spans or frontmatter
        let h = render_md("```\n#code\n```\n`#span`\n\n---\ntags: [fm]\n---\n", &[]);
        assert!(!h.contains("class=\"tag\""), "{h}");
        let h = render_md("---\ntags: #fm\n---\nbody", &[]);
        assert!(!h.contains("class=\"tag\""), "{h}");
        // URL fragments and headings are not tags
        let h = render_md("https://x/y#frag and <https://x/y#frag>\n\n# Heading", &[]);
        assert!(!h.contains("class=\"tag\""), "{h}");
        // no raw html leak via tags: '#' followed by '<' is not a tag, and
        // the '<img>' is escaped by push_html like any text
        let h = render_md("#<img src=x onerror=alert(1)> #a<img>", &[]);
        assert!(!h.contains("<img"), "raw html leaked: {h}");
        assert!(h.contains("data-tag=\"a\">#a</a>&lt;img&gt;"), "{h}");
    }

    #[test]
    fn tags_extraction_edge_cases() {
        use index::tags_in;
        assert_eq!(tags_in("plain #alpha text"), ["alpha"]);
        assert_eq!(tags_in("#nested/tag and #nested/tag again"), ["nested/tag"]);
        assert_eq!(tags_in("#a #b\n#a"), ["a", "b"]);
        assert_eq!(tags_in("#_under #dash-ed #digits2 #ünï"), ["_under", "dash-ed", "digits2", "ünï"]);
        // must start with a letter or '_'; punctuation/headings/bare '#'
        assert!(tags_in("#1 #123 # Heading\n# H1\n## H2 #").is_empty());
        // trailing '/' trimmed, trailing punctuation not part of the tag
        assert_eq!(tags_in("#a/ (#b), #c."), ["a", "c"]);
        // fenced code (both fences) and inline code spans
        assert_eq!(tags_in("```\n#no\n```\n#yes\n~~~\n#no2\n~~~\n  ```rust\n  #no3\n  ```\n"), ["yes"]);
        assert_eq!(tags_in("`#no` #yes `x #no2 y`"), ["yes"]);
        // URL fragments / mid-word '#' / wikilink anchors
        assert!(tags_in("https://x/y#frag foo#bar [[Note#head]] [[#head]]").is_empty());
        // frontmatter: YAML list, comma string, inline list, block list, tag: key
        assert_eq!(tags_in("---\ntags: [one, \"two\", '#three']\n---\n"), ["one", "three", "two"]);
        assert_eq!(tags_in("---\ntags: one, two/sub\n---\nbody"), ["one", "two/sub"]);
        assert_eq!(tags_in("---\ntitle: x\ntags:\n  - one\n  - two\nother: y\n---\n#body"), ["body", "one", "two"]);
        assert_eq!(tags_in("---\ntag: solo\n---\n"), ["solo"]);
        // frontmatter must open on line 1 and close; unclosed = plain body
        assert!(tags_in("text\n---\ntags: [x]\n---\n").is_empty());
        assert!(tags_in("---\ntags: [x]\n").is_empty());
    }

    #[test]
    fn index_tags_counts_search_and_sync() {
        let root = tmp_vault("ixt");
        fs::write(root.join("A.md"), "---\ntags: [fm, shared]\n---\n#alpha #beta/gamma").unwrap();
        fs::write(root.join("B.md"), "#shared and #beta/delta").unwrap();
        fs::write(root.join("sub/C.md"), "no tags").unwrap();
        let mut ix = Index::build(&root);
        assert_eq!(ix.tags("A"), ["alpha", "beta/gamma", "fm", "shared"]);
        assert_eq!(ix.tags("B"), ["beta/delta", "shared"]);
        assert!(ix.tags("sub/C").is_empty() && ix.tags("Nope").is_empty());
        let tc = ix.tag_counts();
        let got: Vec<(&str, usize)> = tc.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        assert_eq!(got, [("alpha", 1), ("beta/delta", 1), ("beta/gamma", 1), ("fm", 1), ("shared", 2)]);
        // search: tag: filter, with/without '#', case-insensitive, nested prefix
        let notes = |ix: &Index, q: &str| {
            let mut v: Vec<String> = search_docs(ix.docs(), q).into_iter().map(|h| h.note).collect();
            v.dedup();
            v
        };
        assert_eq!(notes(&ix, "tag:shared"), ["A", "B"]);
        assert_eq!(notes(&ix, "tag:#alpha"), ["A"]);
        assert_eq!(notes(&ix, "tag:ALPHA"), ["A"]);
        assert_eq!(notes(&ix, "tag:beta"), ["A", "B"]);          // prefix hits both nested tags
        assert_eq!(notes(&ix, "tag:beta/gamma"), ["A"]);
        assert!(notes(&ix, "tag:bet").is_empty());               // prefix is per segment
        assert_eq!(notes(&ix, "tag:fm"), ["A"]);                 // frontmatter-only -> name hit
        assert_eq!(notes(&ix, "tag:shared delta"), ["B"]);       // tag filter + text
        let h = search_docs(ix.docs(), "tag:alpha");
        assert_eq!((h[0].line, h[0].snippet.as_str()), (3, "#alpha #beta/gamma"));
        // upsert keeps tags + counts in sync (existing key and new key)
        ix.upsert("B", "#shared only");
        assert_eq!(ix.tags("B"), ["shared"]);
        assert!(!ix.tag_counts().contains_key("beta/delta"));
        ix.upsert("D", "#fresh");
        assert_eq!(ix.tag_counts()["fresh"], 1);
        assert_eq!(notes(&ix, "tag:fresh"), ["D"]);
        // rename moves the tags with the key
        ix.rename("A", "sub/Z", None);
        assert!(ix.tags("A").is_empty());
        assert_eq!(ix.tags("sub/Z"), ["alpha", "beta/gamma", "fm", "shared"]);
        assert_eq!(ix.tag_counts()["shared"], 2);
        assert_eq!(notes(&ix, "tag:alpha"), ["sub/Z"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn graph_emits_unresolved_nodes() {
        let docs = vec![
            ("A".to_string(), "[[B]] [[Ghost]] [[Ghost]]".to_string()),
            ("B".to_string(), "[[Ghost]] [[sub/C]]".to_string()),
            ("sub/C".to_string(), String::new()),
        ];
        let g = graph_of(&docs);
        // 3 real notes + 1 deduped unresolved
        assert_eq!(g.nodes.len(), 4);
        assert!(g.nodes[..3].iter().all(|n| n.resolved));
        assert_eq!(g.nodes[3].name, "Ghost");
        assert!(!g.nodes[3].resolved);
        // A->B, A->Ghost (deduped), B->Ghost, B->sub/C
        assert_eq!(g.edges, vec![(0, 1), (0, 3), (1, 3), (1, 2)]);
    }

    #[test]
    fn search_finds_case_insensitive_hits() {
        let docs = vec![
            ("Alpha".to_string(), "first LINE here\nsecond alpha line".to_string()),
            ("Beta".to_string(), "nothing\nAlPhA again".to_string()),
        ];
        let hits = search_docs(docs_ref(&docs), "alpha");
        // name hit (Alpha@0) + body hit in Alpha line 1 + body hit in Beta line 1
        assert_eq!(hits.len(), 3);
        assert_eq!((hits[0].note.as_str(), hits[0].line, hits[0].snippet.as_str()),
                   ("Alpha", 0, "Alpha"));
        assert_eq!((hits[1].note.as_str(), hits[1].line, hits[1].snippet.as_str()),
                   ("Alpha", 1, "second alpha line"));
        assert_eq!((hits[2].note.as_str(), hits[2].line, hits[2].snippet.as_str()),
                   ("Beta", 1, "AlPhA again"));
        assert!(search_docs(docs_ref(&docs), "").is_empty());
        assert!(search_docs(docs_ref(&docs), "zzz").is_empty());
        // long line trims to a window around the hit
        let long = ("L".to_string(), format!("{}needle{}", "x".repeat(300), "y".repeat(300)));
        let h = search_docs(docs_ref(&[long]), "needle");
        assert_eq!(h.len(), 1);
        assert!(h[0].snippet.contains("needle") && h[0].snippet.len() <= 210);
    }

    #[test]
    fn bookmark_toggle_adds_then_removes() {
        let l = toggle_in(vec![], "A");
        assert_eq!(l, vec!["A"]);
        let l = toggle_in(l, "sub/B");           // append keeps insertion order
        assert_eq!(l, vec!["A", "sub/B"]);
        let l = toggle_in(l, "A");               // second toggle removes
        assert_eq!(l, vec!["sub/B"]);
        assert!(toggle_in(l, "sub/B").is_empty());
    }

    #[test]
    fn rename_updates_links_vault_wide() {
        let root = std::env::temp_dir().join(format!("rustidian-rl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("Old.md"), "self [[Old]]").unwrap();
        fs::write(
            root.join("B.md"),
            "see [[Old]] and [[Old|nick]] plus [[Old#h2]] and [[Other]]",
        )
        .unwrap();
        fs::write(root.join("sub/C.md"), "[[Old|x]] deep").unwrap();
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "Old", "New").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("B.md")).unwrap(),
            "see [[New]] and [[New|nick]] plus [[New#h2]] and [[Other]]"
        );
        assert_eq!(fs::read_to_string(root.join("sub/C.md")).unwrap(), "[[New|x]] deep");
        assert_eq!(fs::read_to_string(root.join("New.md")).unwrap(), "self [[New]]");
        // full-path links track a move into a folder; basename links keep basename
        fs::write(root.join("D.md"), "[[New]] and [[sub/C]]").unwrap();
        ix.upsert("D", "[[New]] and [[sub/C]]");
        rename_in(&root, &mut ix, "sub/C", "sub/C2").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("D.md")).unwrap(),
            "[[New]] and [[sub/C2]]"
        );
        // ambiguity guard: renaming sub/C2 -> sub/C while a top-level C.md
        // exists — basename rewrite would make [[C2]] capture the decoy, so
        // only the full-path link updates
        fs::write(root.join("C.md"), "decoy").unwrap();
        fs::write(root.join("E.md"), "[[C2]] and [[sub/C2]]").unwrap();
        ix.upsert("C", "decoy");
        ix.upsert("E", "[[C2]] and [[sub/C2]]");
        rename_in(&root, &mut ix, "sub/C2", "sub/C").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("E.md")).unwrap(),
            "[[C2]] and [[sub/C]]"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_moves_refuses_overwrite() {
        let root = std::env::temp_dir().join(format!("rustidian-rn-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("A.md"), "body").unwrap();
        fs::write(root.join("B.md"), "other").unwrap();
        let mut ix = Index::build(&root);
        rename_in(&root, &mut ix, "A", "sub/A2").unwrap(); // parents created
        assert!(!root.join("A.md").exists());
        assert_eq!(fs::read_to_string(root.join("sub/A2.md")).unwrap(), "body");
        assert_eq!(ix.names(), ["B", "sub/A2"]); // index key moved with the file
        assert!(rename_in(&root, &mut ix, "sub/A2", "B").is_err()); // refuse overwrite
        assert!(rename_in(&root, &mut ix, "Ghost", "X").is_err()); // missing source
        assert!(rename_in(&root, &mut ix, "B", "../esc").is_err()); // traversal blocked
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn render_blocks_matches_per_block_render() {
        let notes = vec!["Ideas".to_string(), "sub/Nested".to_string()];
        let blocks: Vec<String> = [
            "# Title [[Ideas]]",
            "plain para with [[Nested]] and [[Nope]]",
            "- a\n- b [[Ideas|alias]]",
            "```\n[[Ideas]]\n```",
            "",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let batch = render_blocks_with(&blocks, &notes, &[]);
        assert_eq!(batch.len(), blocks.len());
        for (b, out) in blocks.iter().zip(&batch) {
            assert_eq!(*out, render_md(b, &notes), "block {b:?} diverged");
        }
        assert!(render_blocks_with(&[], &notes, &[]).is_empty());
    }

    #[test]
    fn recent_list_dedups_newest_first_capped() {
        let l = push_recent(vec![], "/a");
        let l = push_recent(l, "/b");
        assert_eq!(l, vec!["/b", "/a"]); // newest first
        let l = push_recent(l, "/a");    // re-open dedups + promotes
        assert_eq!(l, vec!["/a", "/b"]);
        let mut l = l;
        for i in 0..10 { l = push_recent(l, &format!("/v{i}")); }
        assert_eq!(l.len(), 8);          // capped
        assert_eq!(l[0], "/v9");
    }

    /// S2: symlinked dir + file inside the vault are invisible: not indexed,
    /// not snapshotted, not readable, not writable, not creatable-through
    #[test]
    fn symlinks_are_not_part_of_the_vault() {
        use std::os::unix::fs::symlink;
        let root = tmp_vault("sym");
        let outside = std::env::temp_dir().join(format!("rustidian-sym-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("Secret.md"), "secret").unwrap();
        fs::write(root.join("A.md"), "[[L]] [[link/Secret]]").unwrap();
        symlink(&outside, root.join("link")).unwrap(); // dir link
        symlink(outside.join("Secret.md"), root.join("L.md")).unwrap(); // file link
        // not listed (Index::build + watcher::snapshot share the walk)
        let ix = Index::build(&root);
        assert_eq!(ix.names(), ["A"]);
        assert_eq!(watcher::snapshot(&root).keys().cloned().collect::<Vec<_>>(), ["A"]);
        // not readable
        assert_eq!(note_path_in(&root, "L", false), None);
        assert_eq!(note_path_in(&root, "link/Secret", false), None);
        assert_eq!(note_path_in(&root, "A", false), Some(root.canonicalize().unwrap().join("A.md")));
        // not writable: create=true must not mkdir through the link either
        assert_eq!(note_path_in(&root, "L", true), None);
        assert_eq!(note_path_in(&root, "link/New", true), None);
        assert_eq!(note_path_in(&root, "link/deep/er/New", true), None);
        assert!(!outside.join("New.md").exists());
        assert!(!outside.join("deep").exists());
        assert_eq!(fs::read_to_string(outside.join("Secret.md")).unwrap(), "secret");
        // rename refuses both directions
        let mut ix = Index::build(&root);
        assert!(rename_in(&root, &mut ix, "A", "link/A").is_err());
        assert!(rename_in(&root, &mut ix, "L", "X").is_err());
        assert!(root.join("A.md").is_file());
        assert!(!outside.join("A.md").exists());
        // a plain note still round-trips through create=true (parents made)
        let p = note_path_in(&root, "sub/deep/N", true).unwrap();
        assert!(p.parent().unwrap().is_dir());
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    /// R29 images, POSITIVE half: the shapes that MUST serve bytes. Kept next
    /// to the traversal test because a containment check that refuses
    /// everything also passes the negative one.
    #[test]
    fn img_serves_contained_vault_images_only() {
        let root = tmp_vault("img-ok");
        let px = b"\x89PNG\r\n\x1a\nfake-but-bytes";
        for f in ["pic.png", "my pic.png", "UPPER.PNG", "sub/nested.jpeg", "doc.md", "vec.svg"] {
            fs::write(root.join(f), px).unwrap();
        }
        let croot = root.canonicalize().unwrap();
        // R29.2 relative + subfolder; R29.3 percent-decoded space
        assert_eq!(img_path_in(&root, "pic.png"), Some(croot.join("pic.png")));
        assert_eq!(img_path_in(&root, "sub/nested.jpeg"), Some(croot.join("sub/nested.jpeg")));
        assert_eq!(img_path_in(&root, "my%20pic.png"), Some(croot.join("my pic.png")));
        assert_eq!(img_path_in(&root, "my pic.png"), Some(croot.join("my pic.png")));
        // extension match is case-insensitive, the Content-Type comes from OUR table
        assert_eq!(serve_image(&root, "UPPER.PNG").unwrap().0, "image/png");
        assert_eq!(serve_image(&root, "sub/nested.jpeg").unwrap().0, "image/jpeg");
        assert_eq!(serve_image(&root, "pic.png").unwrap().1, px.to_vec());
        // R29.8: svg is NOT in v1 (scriptable, no sanitizer here); non-images never
        assert_eq!(img_path_in(&root, "vec.svg"), None);
        assert_eq!(img_path_in(&root, "doc.md"), None);
        assert_eq!(img_path_in(&root, "pic"), None);
        // R29.4: a missing file serves nothing — same None as an escape, no leak
        assert_eq!(img_path_in(&root, "nope.png"), None);
        // S5-style cap: an oversized image is not part of the vault either
        let big = root.join("big.png");
        fs::write(&big, px).unwrap();
        assert!(img_path_in(&root, "big.png").is_some());
        let f = fs::OpenOptions::new().write(true).open(&big).unwrap();
        f.set_len(MAX_IMG_BYTES + 1).unwrap();
        assert_eq!(img_path_in(&root, "big.png"), None);
        let _ = fs::remove_dir_all(&root);
    }

    /// R29.5 + S2: every escape serves NO bytes, in both directions —
    /// a target that walks out, and a symlink inside the vault that points out.
    /// The positive control (a genuinely SYMLINKED VAULT still serves) is in
    /// the same test on purpose: canonicalizing only one side passes one half
    /// and fails the other, which is the likely failure mode.
    #[test]
    fn img_traversal_and_symlink_escapes_serve_no_bytes() {
        use std::os::unix::fs::symlink;
        let root = tmp_vault("img-esc");
        let outside = std::env::temp_dir().join(format!("rustidian-img-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        let px = b"\x89PNG\r\n\x1a\nsecret-bytes";
        fs::write(outside.join("outside.png"), px).unwrap();
        fs::write(root.join("pic.png"), b"\x89PNG\r\n\x1a\nin-vault").unwrap();
        // a HIDDEN component is not a vault member (same rule as notes, safe_rel).
        // The file is created on purpose: without it the `.hidden/pic.png` case
        // below would be refused by a failed canonicalize instead of by the
        // check, and deleting safe_rel would not turn this test red.
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::write(root.join(".hidden/pic.png"), px).unwrap();
        // the out-of-vault file EXISTS — refusing it must not depend on a stat miss
        assert!(outside.join("outside.png").is_file());
        for t in [
            "../../../etc/passwd",
            "../outside.png",
            "sub/../../outside.png",
            "..%2foutside.png",
            "%2e%2e%2foutside.png",
            "%2e%2e/outside.png",
            "/etc/passwd",
            "/tmp/rustidian-img-out.png",
            "file:///etc/passwd",
            "pic.png\0.txt",
            "pic.png%00.txt",
            ".hidden/pic.png",
            "%zz.png",
            "%2",
        ] {
            assert_eq!(img_path_in(&root, t), None, "escaped the vault: {t:?}");
            assert!(serve_image(&root, t).is_none(), "served bytes for {t:?}");
        }
        // a symlink INSIDE the vault pointing OUT of it: leaf link and dir link
        symlink(outside.join("outside.png"), root.join("link.png")).unwrap();
        symlink(&outside, root.join("out")).unwrap();
        assert_eq!(img_path_in(&root, "link.png"), None);
        assert_eq!(img_path_in(&root, "out/outside.png"), None);
        assert!(serve_image(&root, "link.png").is_none());
        assert!(serve_image(&root, "out/outside.png").is_none());
        // POSITIVE CONTROL (req S2): a vault reached THROUGH a symlink still
        // serves its own images — this is what canonicalizing the root buys.
        let vlink = std::env::temp_dir().join(format!("rustidian-img-vlink-{}", std::process::id()));
        let _ = fs::remove_file(&vlink);
        symlink(&root, &vlink).unwrap();
        assert_eq!(serve_image(&vlink, "pic.png").unwrap().1, b"\x89PNG\r\n\x1a\nin-vault".to_vec());
        assert_eq!(img_path_in(&vlink, "link.png"), None); // still refused through the link
        let _ = fs::remove_file(&vlink);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    /// R29 + S1: the CSP is a security boundary, so it is asserted as an EXACT
    /// string. A future widening (`img-src *`, an http origin, a stray
    /// 'unsafe-inline' in script-src) then fails a test instead of shipping
    /// silently. tauri.conf.json IS a cargo input, so editing it rebuilds this.
    #[test]
    fn csp_is_exactly_this_string_and_no_remote_image_origin() {
        const CONF: &str = include_str!("../tauri.conf.json");
        let csp = serde_json::from_str::<serde_json::Value>(CONF).unwrap()["app"]["security"]["csp"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            csp,
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
             img-src 'self' data: rustidian-img:; font-src 'self'; \
             connect-src ipc: http://ipc.localhost",
            "the CSP changed — if that was deliberate, say why in the commit message"
        );
        // R29: the image widening is ONE scheme token, and it is ours
        let img = csp.split("; ").find(|d| d.starts_with("img-src ")).unwrap();
        assert_eq!(img, format!("img-src 'self' data: {IMG_SCHEME}:"));
        // R29.10: remote images stay closed pending an operator decision —
        // no wildcard, no http(s) origin anywhere in the image directive
        assert!(!img.contains('*'), "img-src wildcard re-enables remote beacons: {img}");
        assert!(!img.contains("http"), "img-src names a remote origin: {img}");
    }

    /// S4: picker commands refuse system trees and dot-components, keep the
    /// roots the folder picker offers
    #[test]
    fn picker_denies_system_paths_allows_user_roots() {
        for p in ["/etc", "/proc", "/etc/ssh", "/proc/self", "/sys", "/dev", "/usr/bin", "/boot", "/root", "/var/log",
                  "/home/u/.ssh", "/tmp/../etc", "/tmp/.hidden", "tmp", ""] {
            assert!(!picker_allows(Path::new(p)), "{p} must be denied");
        }
        for p in ["/", "/workspace", "/workspace/vault", "/tmp", "/tmp/rustidian-smoke97/vault", "/mnt", "/media", "/run/media", "/home", "/home/u/notes"] {
            assert!(picker_allows(Path::new(p)), "{p} must be allowed");
        }
        assert_eq!(list_dirs("/etc".into()), Vec::<String>::new());
        assert_eq!(list_dirs("/proc".into()), Vec::<String>::new());
        assert!(list_dirs("/".into()).iter().any(|d| d == "tmp"));
    }

    /// S5: a 33 MiB sparse note is skipped by the walk and unreadable
    #[test]
    fn oversized_note_is_skipped() {
        let root = tmp_vault("big");
        fs::write(root.join("A.md"), "small").unwrap();
        let big = root.join("Big.md");
        fs::File::create(&big).unwrap().set_len(33 * 1024 * 1024).unwrap();
        assert_eq!(Index::build(&root).names(), ["A"]);
        assert_eq!(watcher::snapshot(&root).keys().cloned().collect::<Vec<_>>(), ["A"]);
        assert_eq!(read_capped(&big), None); // read_note -> ""
        assert_eq!(read_capped(&root.join("A.md")).as_deref(), Some("small"));
        let _ = fs::remove_dir_all(&root);
    }

    /* ---------- dataloss-audit F1/F3/F4 regression tests ----------
       These bugs are invisible in normal operation: the product reports
       success while the bytes are gone. Each test fails without its fix. */

    /// F1 [audit src-tauri/src/main.rs:162]: a write that cannot land must
    /// RETURN the error and must NOT upsert the index — the old code dropped
    /// fs::write's Result inside an `if ... .is_ok()`, so the UI marked the
    /// buffer clean and every edit of the session died in RAM.
    #[test]
    fn f1_failed_write_returns_err_and_leaves_the_index_untouched() {
        let root = tmp_vault("f1");
        fs::write(root.join("Note.md"), "old body").unwrap();
        let mut ix = Index::build(&root);
        // EISDIR as a portable stand-in for the audit's ENOSPC/EROFS/EDQUOT:
        // a directory sitting exactly where the note file must be created.
        fs::create_dir(root.join("Blocked.md")).unwrap();
        let e = write_note_in(&root, &mut ix, "Blocked", "new body").unwrap_err();
        assert!(!e.is_empty(), "a failed save must carry a message to the UI");
        assert!(ix.content("Blocked").is_none(), "index upserted for bytes that never landed");
        // the happy path still succeeds, lands, and indexes
        assert_eq!(write_note_in(&root, &mut ix, "Note", "new body"), Ok(()));
        assert_eq!(fs::read_to_string(root.join("Note.md")).unwrap(), "new body");
        assert_eq!(ix.content("Note"), Some("new body"));
        let _ = fs::remove_dir_all(&root);
    }

    /// F3 [audit src-tauri/src/main.rs:162]: the note write must be
    /// sibling-temp + fsync + rename, not a truncating open. rename(2)
    /// installs a NEW inode over the name; a truncating write keeps the old
    /// one — that identity IS the window where a crash leaves a short file.
    #[test]
    fn f3_note_write_is_sibling_temp_rename_not_a_truncating_open() {
        use std::os::unix::fs::MetadataExt;
        let root = tmp_vault("f3");
        let p = root.join("Note.md");
        fs::write(&p, "OLD").unwrap();
        let ino0 = fs::metadata(&p).unwrap().ino();
        let mut ix = Index::build(&root);
        write_note_in(&root, &mut ix, "Note", "NEW").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "NEW");
        assert_ne!(
            fs::metadata(&p).unwrap().ino(),
            ino0,
            "note was written in place (O_TRUNC) — no rename barrier"
        );
        // no stray temp survives a successful save
        let stray: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(stray.is_empty(), "temp left behind: {stray:?}");
        // the temp is a SIBLING (same dir => same filesystem => atomic rename):
        // block that exact path and the save fails with the OLD note intact,
        // where a truncating write would already have destroyed it.
        fs::write(root.join("Keep.md"), "KEEP").unwrap();
        fs::create_dir(root.join("Keep.md.tmp")).unwrap();
        let mut ix2 = Index::build(&root);
        assert!(write_note_in(&root, &mut ix2, "Keep", "LOST").is_err());
        assert_eq!(fs::read_to_string(root.join("Keep.md")).unwrap(), "KEEP");
        assert_eq!(ix2.content("Keep"), Some("KEEP"));
        let _ = fs::remove_dir_all(&root);
    }

    /// F4 [audit ui/main.js:606]: creation must refuse to clobber. create_new
    /// is the kernel's atomic exists-check, so the stale in-RAM index cannot
    /// turn a real note into a two-line stub between the check and the write.
    #[test]
    fn f4_create_note_refuses_to_clobber_an_existing_note() {
        let root = tmp_vault("f4");
        fs::write(root.join("Ghost.md"), "valuable\n").unwrap();
        let mut ix = Index::build(&root);
        let e = create_note_in(&root, &mut ix, "Ghost", "# Ghost\n\n").unwrap_err();
        assert_eq!(e, EXISTS, "the UI opens the existing note on this exact error");
        assert_eq!(fs::read_to_string(root.join("Ghost.md")).unwrap(), "valuable\n");
        assert_eq!(ix.content("Ghost"), Some("valuable\n"), "even the RAM copy must survive");
        // a genuinely new note is still created, on disk and in the index
        create_note_in(&root, &mut ix, "sub/New", "# New\n\n").unwrap();
        assert_eq!(fs::read_to_string(root.join("sub/New.md")).unwrap(), "# New\n\n");
        assert_eq!(ix.content("sub/New"), Some("# New\n\n"));
        let _ = fs::remove_dir_all(&root);
    }

    /* feedback #20 [acceptance 1]: a brand-new note materializes NOTHING.
       Recon Q1 against stock 1.13.7: Ctrl+N produces `Untitled.md` whose
       `wc -c` is 0, and a second one `Untitled 1.md`, also 0. The big title
       the user sees is the INLINE TITLE — the FILENAME, rendered — so the
       bytes on disk are empty. Asserted on disk (metadata len) AND in the
       in-RAM index, which the UI reads back for search. */
    #[test]
    fn f20_a_new_note_is_zero_bytes_on_disk() {
        let root = tmp_vault("f20zero");
        let mut ix = Index::build(&root);
        create_note_in(&root, &mut ix, "Untitled", "").unwrap();
        let p = root.join("Untitled.md");
        assert_eq!(fs::metadata(&p).unwrap().len(), 0, "stock's new note is ZERO bytes");
        assert_eq!(fs::read_to_string(&p).unwrap(), "");
        assert_eq!(ix.content("Untitled"), Some(""), "the index copy is empty too");
        // the path-qualified path is where the old code seeded the BASENAME
        create_note_in(&root, &mut ix, "sub/Deep Note", "").unwrap();
        assert_eq!(fs::metadata(root.join("sub/Deep Note.md")).unwrap().len(), 0);
        // an explicit body is still honoured — this is a DEFAULT, not a filter
        create_note_in(&root, &mut ix, "Seeded", "given\n").unwrap();
        assert_eq!(fs::read_to_string(root.join("Seeded.md")).unwrap(), "given\n");
        let _ = fs::remove_dir_all(&root);
    }

    /* feedback #20 [acceptance 1]: ...and NO code path may re-mint a heading.
       The fix is at the two seams every creation path funnels through —
       ui/main.js `createNote` (the shared helper for all four JS paths) and
       the rust `create_note` command's default — so this test reads the JS
       source and pins BOTH: the helper's default body is the empty string,
       and the `"# " + name` construction exists nowhere in main.js. A string
       strip in one caller would not satisfy either half. */
    #[test]
    fn f20_no_code_path_materializes_a_heading_at_creation() {
        const UI: &str = include_str!("../../ui/main.js");
        assert!(
            UI.contains(r#"  const body = content != null ? content : "";"#),
            "createNote's default body must be EMPTY — that is the shared seam"
        );
        assert!(
            !UI.contains(r##""# " + name"##),
            "no creation path may mint a heading from the note name"
        );
        // exactly one call site invokes the backend command, and it passes the
        // helper's body — so the default above is the only default there is.
        assert_eq!(
            UI.matches(r#"inv("create_note""#).count(),
            1,
            "creation must funnel through the single createNote helper"
        );
        assert!(UI.contains(r#"inv("create_note", { name, content: body })"#));
        // and the backend must not synthesise one from a name either. The
        // needle is ASSEMBLED AT RUNTIME on purpose: include_str!("main.rs")
        // contains THIS test, so a literal would match itself and the
        // assertion would fail no matter what the production code does (it
        // did, on the first run — the failure was the test, not the fix).
        const RS: &str = include_str!("main.rs");
        let minted = ["format!(\"#", " {}"].concat();
        assert!(
            !RS.contains(&minted),
            "the backend must not synthesise a heading from a name either"
        );
        // the rust command's own default is empty, mirroring the JS seam.
        assert!(RS.contains("let content = content.unwrap_or_default();"));
    }

    /* feedback #20 [acceptance 2]: an EXISTING note is NOT ours to tidy.
       Now that the big title is the filename, a note whose body still starts
       with `# Foo` shows the title twice — exactly what stock does (recon Q6),
       and exactly what we must leave alone. The rule of order puts data loss
       above every fidelity argument: a "migration" that strips a redundant
       heading rewrites bytes the user typed, so none may exist.

       The cycle below is open -> render -> close at the PURE CORES the Tauri
       commands delegate to (read_capped for read_note, render_with for
       render, render_blocks_with for live preview, srcmode::highlight_block
       for source mode), plus Index::build for opening the vault itself — the
       only places that could plausibly rewrite a file. The oracle is the
       whole vault's bytes AND mtimes, so a rewrite that happened to produce
       identical content elsewhere would still be caught. */
    #[test]
    fn f20_an_existing_body_heading_survives_open_render_close_byte_for_byte() {
        let root = tmp_vault("f20keep");
        // written BEHIND the app's back: these are pre-existing user files
        let foo = "# Foo\n\nfirst paragraph\n\n# Foo\ntwo headings, both stay\n";
        let same = "# Same Name\n\nbody\n"; // Q6: the heading EQUALS the filename
        let odd = "# Keep\r\n\r\nCRLF, no trailing newline, trailing spaces   ";
        fs::write(root.join("Foo.md"), foo).unwrap();
        fs::write(root.join("Same Name.md"), same).unwrap();
        fs::write(root.join("Keep.md"), odd).unwrap();
        let files = ["Foo", "Same Name", "Keep"];
        let snap = |root: &Path| -> Vec<(Vec<u8>, std::time::SystemTime)> {
            files
                .iter()
                .map(|n| {
                    let p = root.join(format!("{n}.md"));
                    let m = fs::metadata(&p).unwrap();
                    (fs::read(&p).unwrap(), m.modified().unwrap())
                })
                .collect()
        };
        let before = snap(&root);

        // OPEN the vault, then per note: read, render (reading + LP + source)
        let ix = Index::build(&root);
        for n in files {
            let p = root.join(format!("{n}.md"));
            let text = read_capped(&p).expect("read_note's core must read it");
            let html = render_with(&text, ix.names(), ix.images(), true);
            let blocks: Vec<String> = text.split("\n\n").map(str::to_string).collect();
            let _ = render_blocks_with(&blocks, ix.names(), ix.images());
            let _: Vec<String> = blocks.iter().map(|b| srcmode::highlight_block(b)).collect();
            // the index must hold the file's bytes, not a cleaned-up copy
            assert_eq!(ix.content(n), Some(text.as_str()), "{n}: index != disk");
            if n == "Foo" {
                assert_eq!(
                    html.matches("<h1").count(),
                    2,
                    "both body headings must render — nothing is stripped: {html}"
                );
            }
            if n == "Same Name" {
                assert!(
                    html.contains("Same Name"),
                    "a heading equal to the filename is still the user's text (Q6): {html}"
                );
            }
        }
        drop(ix); // CLOSE the vault

        assert_eq!(before, snap(&root), "open+render+close rewrote a user file");
        // and re-opening the vault is not a second chance to rewrite anything
        let ix2 = Index::build(&root);
        assert_eq!(ix2.content("Foo"), Some(foo));
        drop(ix2);
        assert_eq!(before, snap(&root), "re-opening the vault rewrote a user file");

        // source level: no migration may exist, in either language. The note
        // OPEN path in the UI must not write — `setInlineTitle` publishes a
        // dataset attribute, it does not touch bytes.
        const UI: &str = include_str!("../../ui/main.js");
        let i = UI.find("async function loadActive(g) {").expect("loadActive moved");
        let end = UI[i..].find("\nasync function ").unwrap_or(UI.len() - i);
        let open_path = &UI[i..i + end];
        assert!(
            !open_path.contains("write_note"),
            "opening a note must never write it back"
        );
        assert!(open_path.contains("setInlineTitle"), "the open path draws the title");
        const RS2: &str = include_str!("main.rs");
        // needles ASSEMBLED AT RUNTIME: include_str!("main.rs") contains THIS
        // test, so a literal needle matches itself and the assertion fails no
        // matter what the production code does (it did, first run).
        for needle in [["fn mig", "rate"].concat(), ["strip_", "heading"].concat(), ["strip", "Title"].concat()] {
            assert!(!RS2.contains(&needle), "no migration may exist ({needle})");
            assert!(!UI.contains(&needle), "no migration may exist ({needle})");
        }
        let _ = fs::remove_dir_all(&root);
    }

    /* feedback #20 [acceptance 3]: the title left the bytes — FINDING must not
       have left with it. Before this goal the `# <name>` heading meant every
       note's own title was searchable as BODY TEXT; a zero-byte note has no
       body at all, so both find-by-title paths now rest on the FILENAME:

         search        -> search_docs' name arm (`name.contains(q)`), fed by
                          Index::docs(), which yields a name for every note
                          however empty its content is.
         quick switcher-> ui/main.js qsItems(), whose labels are notesCache =
                          the `list_notes` command = Index::names(); mdFilter
                          scores `it.label`, never a body.

       So the Rust half drives the REAL matcher over notes whose bodies cannot
       help (one zero-byte, one whose text never mentions its name), and the
       switcher half pins its data source + the field it matches on. */
    #[test]
    fn f20_a_note_is_findable_by_its_title_now_that_the_title_is_not_in_the_body() {
        let root = tmp_vault("f20find");
        let mut ix = Index::build(&root);
        // created the way the app creates them now: NOTHING on disk
        create_note_in(&root, &mut ix, "Project Ideas", "").unwrap();
        create_note_in(&root, &mut ix, "sub/Deep Thought", "").unwrap();
        // a note whose body never mentions its own name
        fs::write(root.join("Meeting Notes.md"), "agenda\ndiscussed the budget\n").unwrap();
        let ix = Index::build(&root);
        assert_eq!(fs::metadata(root.join("Project Ideas.md")).unwrap().len(), 0);

        // SEARCH: the query is title text and nothing else on disk carries it
        let hits = search_docs(ix.docs(), "project ideas");
        assert_eq!(hits.len(), 1, "a zero-byte note must still be findable by its title");
        assert_eq!(
            (hits[0].note.as_str(), hits[0].line, hits[0].snippet.as_str()),
            ("Project Ideas", 0, "Project Ideas"),
            "the hit is the NAME arm: line 0, snippet = the title itself"
        );
        // partial + case-insensitive + a nested note matched by its basename
        assert_eq!(search_docs(ix.docs(), "IDEAS")[0].note, "Project Ideas");
        assert_eq!(search_docs(ix.docs(), "thought")[0].note, "sub/Deep Thought");
        // ...and the body-less note is findable by title while its neighbour
        // is findable by body — the two arms are independent
        let m = search_docs(ix.docs(), "meeting");
        assert_eq!(m.len(), 1, "title hit only; the body never says 'meeting'");
        assert_eq!((m[0].note.as_str(), m[0].line), ("Meeting Notes", 0));
        let b = search_docs(ix.docs(), "budget");
        assert_eq!((b.len(), b[0].note.as_str(), b[0].line), (1, "Meeting Notes", 1));

        // and the name hit does NOT come from content: prove it on a doc stream
        // whose content is empty by construction.
        let docs = vec![("Only A Name".to_string(), String::new())];
        let h = search_docs(docs_ref(&docs), "only a name");
        assert_eq!((h.len(), h[0].snippet.as_str()), (1, "Only A Name"));

        // QUICK SWITCHER: its source is Index::names() via `list_notes`, which
        // lists a zero-byte note exactly like any other.
        assert_eq!(ix.names(), ["Meeting Notes", "Project Ideas", "sub/Deep Thought"]);
        assert_eq!(index::notes_of(&root), ix.names(), "the walk and the index agree");
        assert!(ix.content("Project Ideas").unwrap().is_empty(), "found by name, not by bytes");

        // the JS half: the switcher's items are FILENAMES and the filter scores
        // that label. A switcher that had matched on body text would need a
        // second source here; there is none.
        const UI: &str = include_str!("../../ui/main.js");
        let qs = {
            let i = UI.find("function qsItems() {").expect("qsItems moved");
            let end = UI[i..].find("\nfunction ").unwrap_or(UI.len() - i);
            &UI[i..i + end]
        };
        assert!(qs.contains("notesCache"), "the switcher lists the vault's NOTE NAMES");
        assert!(qs.contains("label: n,"), "each item's label IS the note name");
        assert!(!qs.contains("content") && !qs.contains("read_note"), "it never reads bodies");
        assert!(
            UI.contains(r#"mdItems = mdSrc().map((it, i) => [fuzzy(q, it.label), i, it])"#),
            "the filter scores the label"
        );
        assert!(
            UI.contains(r#"await Promise.all([inv("list_folders"), inv("list_notes"), inv("list_images")]);"#)
                && UI.contains("notesCache = notes;"),
            "notesCache is the list_notes command = Index::names()"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
