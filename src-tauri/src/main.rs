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

#[tauri::command]
fn render(content: String) -> String {
    // [[X]] -> inline html anchor, then markdown
    let mut md = content;
    for l in links_in(&md.clone()) {
        md = md.replace(
            &format!("[[{l}]]"),
            &format!("<a href=\"#\" class=\"wiki\" data-note=\"{l}\">{l}</a>"),
        );
    }
    let mut out = String::new();
    html::push_html(&mut out, Parser::new_ext(&md, Options::all()));
    out
}

#[derive(serde::Serialize)]
struct Graph {
    nodes: Vec<String>,
    edges: Vec<(usize, usize)>,
}

#[tauri::command]
fn graph(v: State<Vault>) -> Graph {
    let Some(root) = cur_vault(&v) else {
        return Graph { nodes: vec![], edges: vec![] };
    };
    let nodes = notes_of(&root);
    let mut edges = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        let content =
            fs::read_to_string(format!("{}.md", root.join(n).display())).unwrap_or_default();
        for l in links_in(&content) {
            // wikilinks resolve by full relative path or basename
            if let Some(j) = nodes
                .iter()
                .position(|x| *x == l || x.ends_with(&format!("/{l}")))
            {
                if i != j {
                    edges.push((i, j));
                }
            }
        }
    }
    Graph { nodes, edges }
}

fn main() {
    let init = std::env::var("VAULT_DIR").ok().map(PathBuf::from);
    tauri::Builder::default()
        .manage(Vault(Mutex::new(init)))
        .invoke_handler(tauri::generate_handler![
            list_notes, read_note, write_note, render, graph, vault_get, set_vault,
            create_vault, home_dir, list_dirs, list_folders, create_dir
        ])
        .run(tauri::generate_context!())
        .expect("tauri run");
}
