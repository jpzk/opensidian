#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use pulldown_cmark::{html, Event, Options, Parser, Tag, TagEnd};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use tauri::State;

mod perf;

/* perf-lp: sorted note-name list cached per vault. Every render used to
   re-walk + sort the whole vault (notes_of); with ~150 blocks per lp pass
   that was the bulk of the 269ms/caret-move. Cache is a plain Option so
   invalidate() is one store; readers rebuild lazily on the next get(). */
#[derive(Default)]
struct NoteCache(Mutex<Option<Vec<String>>>);

impl NoteCache {
    fn get(&self, root: &Path) -> Vec<String> {
        let mut g = self.0.lock().unwrap();
        if let Some(n) = g.as_ref() {
            return n.clone();
        }
        let n = notes_of(root);
        *g = Some(n.clone());
        n
    }
    fn invalidate(&self) {
        *self.0.lock().unwrap() = None;
    }
}

struct Vault(Mutex<Option<PathBuf>>, NoteCache);

fn cur_vault(v: &State<Vault>) -> Option<PathBuf> {
    v.0.lock().unwrap().clone()
}

/// cached sorted note list for the open vault (empty when none open)
fn cur_notes(v: &State<Vault>) -> Vec<String> {
    cur_vault(v).map(|r| v.1.get(&r)).unwrap_or_default()
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

fn note_path(v: &State<Vault>, name: &str) -> Option<PathBuf> {
    let p = cur_vault(v)?.join(safe_rel(name)?);
    Some(PathBuf::from(format!("{}.md", p.display())))
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            walk(&p, base, out);
        } else if let Some(stem) = name.strip_suffix(".md") {
            let rel = p
                .parent()
                .and_then(|d| d.strip_prefix(base).ok())
                .unwrap_or(Path::new(""));
            out.push(if rel.as_os_str().is_empty() {
                stem.to_string()
            } else {
                format!("{}/{}", rel.display(), stem)
            });
        }
    }
}

fn notes_of(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
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
    v.1.invalidate();
    fs::create_dir_all(root.join(rel)).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_notes(v: State<Vault>) -> Vec<String> {
    span_timed!("list_notes", cur_notes(&v))
}

#[tauri::command]
fn read_note(v: State<Vault>, name: String) -> String {
    span_timed!(
        "read_note",
        note_path(&v, &name)
            .and_then(|p| fs::read_to_string(p).ok())
            .unwrap_or_default()
    )
}

#[tauri::command]
fn write_note(v: State<Vault>, name: String, content: String) {
    let bytes = content.len();
    span_timed!("write_note", write_note_inner(&v, &name, &content), serde_json::json!({"bytes": bytes}))
}

fn write_note_inner(v: &State<Vault>, name: &str, content: &str) {
    if let Some(p) = note_path(&v, &name) {
        // new file changes the note set; per-keystroke saves of an existing
        // note must not evict the cache (that is the hot lp path)
        if !p.exists() {
            v.1.invalidate();
        }
        if let Some(d) = p.parent() {
            let _ = fs::create_dir_all(d);
        }
        let _ = fs::write(p, content);
    }
}

/* ux-3: vault-wide wikilink rewrite on rename. [[Old]] -> [[New]],
   [[Old|alias]] keeps alias, [[Old#h]] keeps anchor. Basename-style links
   ([[A]] for sub/A) stay basename-style; full-path links get the full new
   path. bn_ok=false disables basename matching (caller found ANOTHER note
   with the same basename — those links now resolve elsewhere, leave them). */
fn rewrite_links(s: &str, old: &str, new: &str, bn_ok: bool) -> (String, bool) {
    let ob = old.rsplit('/').next().unwrap_or(old);
    let nb = new.rsplit('/').next().unwrap_or(new);
    let (mut out, mut changed) = (String::new(), false);
    let mut rest = s;
    while let Some(a) = rest.find("[[") {
        out.push_str(&rest[..a + 2]);
        rest = &rest[a + 2..];
        let Some(b) = rest.find("]]") else { break };
        let inner = &rest[..b];
        rest = &rest[b + 2..];
        let (tgt, alias) = match inner.find('|') {
            Some(i) => (&inner[..i], &inner[i..]),
            None => (inner, ""),
        };
        let (base, anchor) = match tgt.find('#') {
            Some(i) => (&tgt[..i], &tgt[i..]),
            None => (tgt, ""),
        };
        let rep = if base == old {
            Some(new)
        } else if bn_ok && base == ob {
            Some(nb)
        } else {
            None
        };
        match rep {
            Some(r) => {
                changed = true;
                out.push_str(r);
                out.push_str(anchor);
                out.push_str(alias);
            }
            None => out.push_str(inner),
        }
        out.push_str("]]");
    }
    out.push_str(rest);
    (out, changed)
}

/* m5 F2 rename: fs::rename old.md -> new.md inside root. Parents created,
   overwrite refused. ux-3: wikilinks updated vault-wide after the move.
   Pure-ish core for unit tests. */
fn rename_in(root: &Path, old: &str, new: &str) -> Result<(), String> {
    let op = root.join(safe_rel(old).ok_or("invalid name")?);
    let np = root.join(safe_rel(new).ok_or("invalid name")?);
    let op = PathBuf::from(format!("{}.md", op.display()));
    let np = PathBuf::from(format!("{}.md", np.display()));
    if !op.is_file() {
        return Err("no such note".into());
    }
    if np.exists() {
        return Err("target exists".into());
    }
    if let Some(d) = np.parent() {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    fs::rename(op, np).map_err(|e| e.to_string())?;
    // ux-3: after the move, rewrite [[links]] in every note. Basename links
    // only rewrite when unambiguous BOTH ways: no other note carries old's
    // basename (those links resolve elsewhere) and none carries new's
    // basename besides the renamed note itself (rewrite would capture it).
    let ob = old.rsplit('/').next().unwrap_or(old);
    let nrel = safe_rel(new).unwrap_or_default().display().to_string();
    let nb = new.rsplit('/').next().unwrap_or(new);
    let notes = notes_of(root);
    let bn_ok = !notes.iter().any(|n| {
        let b = n.rsplit('/').next().unwrap_or(n);
        b == ob || (b == nb && *n != nrel)
    });
    for n in &notes {
        let p = PathBuf::from(format!("{}.md", root.join(n).display()));
        if let Ok(c) = fs::read_to_string(&p) {
            let (nc, changed) = rewrite_links(&c, old, new, bn_ok);
            if changed {
                let _ = fs::write(&p, nc);
            }
        }
    }
    Ok(())
}

#[tauri::command]
fn rename_note(v: State<Vault>, old: String, new: String) -> Result<(), String> {
    let root = cur_vault(&v).ok_or("no vault open")?;
    v.1.invalidate();
    span_timed!("rename_note", rename_in(&root, &old, &new))
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
fn set_vault(v: State<Vault>, path: String) -> Result<String, String> {
    span_timed!("set_vault", set_vault_inner(&v, &path))
}

fn set_vault_inner(v: &State<Vault>, path: &str) -> Result<String, String> {
    let p = PathBuf::from(path.trim());
    if !p.is_dir() {
        return Err(format!("not a directory: {}", p.display()));
    }
    persist_vault(&p);
    v.1.invalidate();
    *v.0.lock().unwrap() = Some(p.clone());
    Ok(p.display().to_string())
}

#[tauri::command]
fn create_vault(v: State<Vault>, parent: String, name: String) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err("invalid vault name".into());
    }
    let p = PathBuf::from(parent.trim()).join(name);
    if p.exists() {
        return Err(format!("already exists: {}", p.display()));
    }
    fs::create_dir_all(&p).map_err(|e| e.to_string())?;
    fs::write(
        p.join("Welcome.md"),
        "# Welcome\n\nThis is your new vault. Notes are plain Markdown files.\nLink them with [[Wiki Links]].\n",
    )
    .map_err(|e| e.to_string())?;
    persist_vault(&p);
    v.1.invalidate();
    *v.0.lock().unwrap() = Some(p.clone());
    Ok(p.display().to_string())
}

/// perf-spans: frontend spans land in the same RUSTIDIAN_PERF jsonl as backend ones
#[tauri::command]
fn log_span(name: String, ms: f64, extra: serde_json::Value) -> bool {
    perf::span(&name, ms, extra);
    perf::enabled() // false lets the UI stop sending spans at all
}

#[tauri::command]
fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".into())
}

#[tauri::command]
fn list_dirs(path: String) -> Vec<String> {
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

fn links_in(s: &str) -> Vec<String> {
    let (mut out, mut rest) = (Vec::new(), s);
    while let Some(a) = rest.find("[[") {
        rest = &rest[a + 2..];
        match rest.find("]]") {
            Some(b) => {
                out.push(rest[..b].to_string());
                rest = &rest[b + 2..];
            }
            None => break,
        }
    }
    out
}

/// wikilinks resolve by full relative path or basename
fn resolve(notes: &[String], l: &str) -> Option<usize> {
    notes
        .iter()
        .position(|x| *x == l || x.ends_with(&format!("/{l}")))
}

/// html-escape for text content and attribute values (H1 fix)
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// scan coalesced text for [[wikilinks]], emitting escaped anchors (H1 fix)
fn linkify(buf: &str, notes: &[String], evs: &mut Vec<Event>) {
    let mut rest = buf;
    while let Some(i) = rest.find("[[") {
        let Some(j) = rest[i + 2..].find("]]") else { break };
        let l = &rest[i + 2..i + 2 + j];
        evs.push(Event::Text(rest[..i].to_string().into()));
        let cls = if resolve(notes, l).is_some() {
            "wiki"
        } else {
            "wiki wiki-unresolved"
        };
        evs.push(Event::Html(
            format!(
                "<a href=\"#\" class=\"{cls}\" data-note=\"{}\">{}</a>",
                esc(l),
                esc(l)
            )
            .into(),
        ));
        rest = &rest[i + 2 + j + 2..];
    }
    if !rest.is_empty() {
        evs.push(Event::Text(rest.to_string().into()));
    }
}

fn render_md(content: &str, notes: &[String]) -> String {
    // Security (docs/security-review.md H1): .md files are untrusted, so raw
    // HTML events are demoted to text (push_html escapes Text). Wikilinks are
    // linkified at the EVENT level — label and data-note attr escaped — so our
    // anchors are the only HTML that survives. Code blocks/spans untouched.
    // NB: pulldown emits "[" "[" "Ideas" "]" "]" as SEPARATE Text events, so
    // consecutive text is coalesced in `buf` before the wikilink scan.
    let mut evs: Vec<Event> = Vec::new();
    let mut buf = String::new();
    let mut in_code = false;
    // pulldown's own ENABLE_WIKILINKS would consume [[..]] before our pass;
    // SMART_PUNCTUATION would curl quotes/apostrophes inside link targets,
    // breaking [[name]] -> filename fidelity
    let mut opts = Options::all();
    opts.remove(Options::ENABLE_WIKILINKS);
    opts.remove(Options::ENABLE_SMART_PUNCTUATION);
    for ev in Parser::new_ext(content, opts) {
        match ev {
            // demoted raw html + plain text both join the scan buffer
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) if !in_code => {
                buf.push_str(&t)
            }
            other => {
                if !buf.is_empty() {
                    linkify(&buf, notes, &mut evs);
                    buf.clear();
                }
                match other {
                    Event::Start(Tag::CodeBlock(_)) => in_code = true,
                    Event::End(TagEnd::CodeBlock) => in_code = false,
                    _ => {}
                }
                evs.push(other);
            }
        }
    }
    if !buf.is_empty() {
        linkify(&buf, notes, &mut evs);
    }
    let mut out = String::new();
    html::push_html(&mut out, evs.into_iter());
    out
}

#[tauri::command]
fn render(v: State<Vault>, content: String) -> String {
    span_timed!("render", render_md(&content, &cur_notes(&v)), serde_json::json!({"bytes": content.len()}))
}

/// pure core of render_blocks: every block rendered against the same note list
fn render_blocks_with(blocks: &[String], notes: &[String]) -> Vec<String> {
    blocks.iter().map(|b| render_md(b, notes)).collect()
}

/* perf-lp: live preview renders every block of a note per caret move. One
   IPC round-trip + one note-list lookup for the whole batch instead of
   ~150 render calls each re-walking the vault. */
#[tauri::command]
fn render_blocks(v: State<Vault>, blocks: Vec<String>) -> Vec<String> {
    let n = blocks.len();
    span_timed!("render_blocks", render_blocks_with(&blocks, &cur_notes(&v)), serde_json::json!({"blocks": n}))
}

#[tauri::command]
fn backlinks(v: State<Vault>, name: String) -> Vec<String> {
    span_timed!("backlinks", backlinks_inner(&v, &name))
}

fn backlinks_inner(v: &State<Vault>, name: &str) -> Vec<String> {
    // invert the graph edges: which notes link to `name`?
    let Some(root) = cur_vault(&v) else { return vec![] };
    let notes = v.1.get(&root);
    let mut out = Vec::new();
    for n in &notes {
        if *n == name {
            continue;
        }
        let content =
            fs::read_to_string(format!("{}.md", root.join(n).display())).unwrap_or_default();
        if links_in(&content)
            .iter()
            .any(|l| resolve(&notes, l).map(|j| notes[j] == name).unwrap_or(false))
        {
            out.push(n.clone());
        }
    }
    out
}

#[derive(serde::Serialize)]
struct GNode {
    name: String,
    resolved: bool,
}

#[derive(serde::Serialize)]
struct Graph {
    nodes: Vec<GNode>,
    edges: Vec<(usize, usize)>,
}

/// R4.2: notes = resolved nodes; wikilinks to nonexistent notes become
/// unresolved nodes (deduped by link text), so the graph shows ghost targets.
fn build_graph(docs: &[(String, String)]) -> Graph {
    let notes: Vec<String> = docs.iter().map(|(n, _)| n.clone()).collect();
    let mut nodes: Vec<GNode> = notes
        .iter()
        .map(|n| GNode { name: n.clone(), resolved: true })
        .collect();
    let mut edges = Vec::new();
    for (i, (_, content)) in docs.iter().enumerate() {
        for l in links_in(content) {
            let j = match resolve(&notes, &l) {
                Some(j) => j,
                None => nodes
                    .iter()
                    .position(|x| !x.resolved && x.name == l)
                    .unwrap_or_else(|| {
                        nodes.push(GNode { name: l.clone(), resolved: false });
                        nodes.len() - 1
                    }),
            };
            if i != j && !edges.contains(&(i, j)) {
                edges.push((i, j));
            }
        }
    }
    Graph { nodes, edges }
}

#[tauri::command]
fn graph(v: State<Vault>) -> Graph {
    span_timed!("graph", graph_inner(&v), serde_json::json!({}))
}

fn graph_inner(v: &State<Vault>) -> Graph {
    let Some(root) = cur_vault(&v) else {
        return Graph { nodes: vec![], edges: vec![] };
    };
    let docs: Vec<(String, String)> = v.1.get(&root)
        .into_iter()
        .map(|n| {
            let c = fs::read_to_string(format!("{}.md", root.join(&n).display()))
                .unwrap_or_default();
            (n, c)
        })
        .collect();
    build_graph(&docs)
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
fn search_docs(docs: &[(String, String)], query: &str) -> Vec<SearchHit> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    if q.is_empty() {
        return out;
    }
    'docs: for (name, content) in docs {
        if name.to_lowercase().contains(&q) {
            out.push(SearchHit { note: name.clone(), line: 0, snippet: name.clone() });
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
            out.push(SearchHit { note: name.clone(), line: i as u32, snippet });
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
fn search(v: State<Vault>, query: String) -> Vec<SearchHit> {
    span_timed!("search", search_inner(&v, &query))
}

fn search_inner(v: &State<Vault>, query: &str) -> Vec<SearchHit> {
    let Some(root) = cur_vault(&v) else { return vec![] };
    let docs: Vec<(String, String)> = v.1.get(&root)
        .into_iter()
        .map(|n| {
            let c = fs::read_to_string(format!("{}.md", root.join(&n).display()))
                .unwrap_or_default();
            (n, c)
        })
        .collect();
    search_docs(&docs, &query)
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

fn main() {
    // VAULT_DIR (probes/tests) wins; else last persisted vault if still a dir (R1.6)
    let init = std::env::var("VAULT_DIR")
        .ok()
        .map(PathBuf::from)
        .or_else(|| read_cfg().0.map(PathBuf::from).filter(|p| p.is_dir()));
    tauri::Builder::default()
        .manage(Vault(Mutex::new(init), NoteCache::default()))
        .invoke_handler(tauri::generate_handler![
            list_notes, read_note, write_note, render, render_blocks, graph, vault_get, set_vault,
            create_vault, home_dir, list_dirs, list_folders, create_dir, backlinks, search,
            list_bookmarks, toggle_bookmark, recent_vaults, rename_note,
            get_sidebar_w, set_sidebar_w, log_span
        ])
        .run(tauri::generate_context!())
        .expect("tauri run");
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn graph_emits_unresolved_nodes() {
        let docs = vec![
            ("A".to_string(), "[[B]] [[Ghost]] [[Ghost]]".to_string()),
            ("B".to_string(), "[[Ghost]] [[sub/C]]".to_string()),
            ("sub/C".to_string(), String::new()),
        ];
        let g = build_graph(&docs);
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
        let hits = search_docs(&docs, "alpha");
        // name hit (Alpha@0) + body hit in Alpha line 1 + body hit in Beta line 1
        assert_eq!(hits.len(), 3);
        assert_eq!((hits[0].note.as_str(), hits[0].line, hits[0].snippet.as_str()),
                   ("Alpha", 0, "Alpha"));
        assert_eq!((hits[1].note.as_str(), hits[1].line, hits[1].snippet.as_str()),
                   ("Alpha", 1, "second alpha line"));
        assert_eq!((hits[2].note.as_str(), hits[2].line, hits[2].snippet.as_str()),
                   ("Beta", 1, "AlPhA again"));
        assert!(search_docs(&docs, "").is_empty());
        assert!(search_docs(&docs, "zzz").is_empty());
        // long line trims to a window around the hit
        let long = ("L".to_string(), format!("{}needle{}", "x".repeat(300), "y".repeat(300)));
        let h = search_docs(&[long], "needle");
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
        rename_in(&root, "Old", "New").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("B.md")).unwrap(),
            "see [[New]] and [[New|nick]] plus [[New#h2]] and [[Other]]"
        );
        assert_eq!(fs::read_to_string(root.join("sub/C.md")).unwrap(), "[[New|x]] deep");
        assert_eq!(fs::read_to_string(root.join("New.md")).unwrap(), "self [[New]]");
        // full-path links track a move into a folder; basename links keep basename
        fs::write(root.join("D.md"), "[[New]] and [[sub/C]]").unwrap();
        rename_in(&root, "sub/C", "sub/C2").unwrap();
        assert_eq!(
            fs::read_to_string(root.join("D.md")).unwrap(),
            "[[New]] and [[sub/C2]]"
        );
        // ambiguity guard: renaming sub/C2 -> sub/C while a top-level C.md
        // exists — basename rewrite would make [[C2]] capture the decoy, so
        // only the full-path link updates
        fs::write(root.join("C.md"), "decoy").unwrap();
        fs::write(root.join("E.md"), "[[C2]] and [[sub/C2]]").unwrap();
        rename_in(&root, "sub/C2", "sub/C").unwrap();
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
        rename_in(&root, "A", "sub/A2").unwrap(); // parents created
        assert!(!root.join("A.md").exists());
        assert_eq!(fs::read_to_string(root.join("sub/A2.md")).unwrap(), "body");
        assert!(rename_in(&root, "sub/A2", "B").is_err()); // refuse overwrite
        assert!(rename_in(&root, "Ghost", "X").is_err()); // missing source
        assert!(rename_in(&root, "B", "../esc").is_err()); // traversal blocked
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
        let batch = render_blocks_with(&blocks, &notes);
        assert_eq!(batch.len(), blocks.len());
        for (b, out) in blocks.iter().zip(&batch) {
            assert_eq!(*out, render_md(b, &notes), "block {b:?} diverged");
        }
        assert!(render_blocks_with(&[], &notes).is_empty());
    }

    #[test]
    fn note_cache_invalidates_after_rename() {
        let root = std::env::temp_dir().join(format!("rustidian-nc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("A.md"), "a").unwrap();
        fs::write(root.join("B.md"), "b").unwrap();
        let c = NoteCache::default();
        assert_eq!(c.get(&root), vec!["A", "B"]);
        // stale-by-design: disk changed but no invalidate -> old list served
        fs::write(root.join("C.md"), "c").unwrap();
        assert_eq!(c.get(&root), vec!["A", "B"]);
        // rename + invalidate -> fresh walk shows the new names
        rename_in(&root, "A", "sub/Z").unwrap();
        c.invalidate();
        assert_eq!(c.get(&root), vec!["B", "C", "sub/Z"]);
        // and the rebuilt list is cached again
        fs::remove_file(root.join("B.md")).unwrap();
        assert_eq!(c.get(&root), vec!["B", "C", "sub/Z"]);
        c.invalidate();
        assert_eq!(c.get(&root), vec!["C", "sub/Z"]);
        let _ = fs::remove_dir_all(&root);
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
}
