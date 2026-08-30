#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use pulldown_cmark::{html, Options, Parser};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use tauri::State;

struct Vault(Mutex<Option<PathBuf>>);

fn cur_vault(v: &State<Vault>) -> Option<PathBuf> {
    v.0.lock().unwrap().clone()
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
    fs::create_dir_all(root.join(rel)).map_err(|e| e.to_string())
}

#[tauri::command]
fn list_notes(v: State<Vault>) -> Vec<String> {
    cur_vault(&v).map(|r| notes_of(&r)).unwrap_or_default()
}

#[tauri::command]
fn read_note(v: State<Vault>, name: String) -> String {
    note_path(&v, &name)
        .and_then(|p| fs::read_to_string(p).ok())
        .unwrap_or_default()
}

#[tauri::command]
fn write_note(v: State<Vault>, name: String, content: String) {
    if let Some(p) = note_path(&v, &name) {
        if let Some(d) = p.parent() {
            let _ = fs::create_dir_all(d);
        }
        let _ = fs::write(p, content);
    }
}

#[tauri::command]
fn vault_get(v: State<Vault>) -> Option<String> {
    cur_vault(&v).map(|p| p.display().to_string())
}

#[tauri::command]
fn set_vault(v: State<Vault>, path: String) -> Result<String, String> {
    let p = PathBuf::from(path.trim());
    if !p.is_dir() {
        return Err(format!("not a directory: {}", p.display()));
    }
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
    *v.0.lock().unwrap() = Some(p.clone());
    Ok(p.display().to_string())
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

fn render_md(content: &str, notes: &[String]) -> String {
    // [[X]] -> inline html anchor (unresolved targets marked), then markdown
    let mut md = content.to_string();
    for l in links_in(content) {
        let cls = if resolve(notes, &l).is_some() {
            "wiki"
        } else {
            "wiki wiki-unresolved"
        };
        md = md.replace(
            &format!("[[{l}]]"),
            &format!("<a href=\"#\" class=\"{cls}\" data-note=\"{l}\">{l}</a>"),
        );
    }
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(&md, Options::all()));
    out
}

#[tauri::command]
fn render(v: State<Vault>, content: String) -> String {
    let notes = cur_vault(&v).map(|r| notes_of(&r)).unwrap_or_default();
    render_md(&content, &notes)
}

#[tauri::command]
fn backlinks(v: State<Vault>, name: String) -> Vec<String> {
    // invert the graph edges: which notes link to `name`?
    let Some(root) = cur_vault(&v) else { return vec![] };
    let notes = notes_of(&root);
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
    let Some(root) = cur_vault(&v) else {
        return Graph { nodes: vec![], edges: vec![] };
    };
    let docs: Vec<(String, String)> = notes_of(&root)
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
    let Some(root) = cur_vault(&v) else { return vec![] };
    let docs: Vec<(String, String)> = notes_of(&root)
        .into_iter()
        .map(|n| {
            let c = fs::read_to_string(format!("{}.md", root.join(&n).display()))
                .unwrap_or_default();
            (n, c)
        })
        .collect();
    search_docs(&docs, &query)
}

fn main() {
    let init = std::env::var("VAULT_DIR").ok().map(PathBuf::from);
    tauri::Builder::default()
        .manage(Vault(Mutex::new(init)))
        .invoke_handler(tauri::generate_handler![
            list_notes, read_note, write_note, render, graph, vault_get, set_vault,
            create_vault, home_dir, list_dirs, list_folders, create_dir, backlinks, search
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
}
