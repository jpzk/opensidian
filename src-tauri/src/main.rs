#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use pulldown_cmark::{html, Options, Parser};
use std::{fs, path::PathBuf};

fn vault() -> PathBuf {
    std::env::var("VAULT_DIR").map(Into::into).unwrap_or_else(|_| "vault".into())
}
fn safe(n: &str) -> String {
    n.chars().filter(|c| !matches!(c, '/' | '\\' | '.')).collect()
}

#[tauri::command]
fn list_notes() -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(vault())
        .into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().into_string().ok()?.strip_suffix(".md").map(String::from))
        .collect();
    v.sort();
    v
}

#[tauri::command]
fn read_note(name: String) -> String {
    fs::read_to_string(vault().join(format!("{}.md", safe(&name)))).unwrap_or_default()
}

#[tauri::command]
fn write_note(name: String, content: String) {
    let _ = fs::create_dir_all(vault());
    let _ = fs::write(vault().join(format!("{}.md", safe(&name))), content);
}

fn links_in(s: &str) -> Vec<String> {
    let (mut out, mut rest) = (Vec::new(), s);
    while let Some(a) = rest.find("[[") {
        rest = &rest[a + 2..];
        match rest.find("]]") {
            Some(b) => { out.push(rest[..b].to_string()); rest = &rest[b + 2..]; }
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
struct Graph { nodes: Vec<String>, edges: Vec<(usize, usize)> }

#[tauri::command]
fn graph() -> Graph {
    let nodes = list_notes();
    let mut edges = Vec::new();
    for (i, n) in nodes.iter().enumerate() {
        for l in links_in(&read_note(n.clone())) {
            if let Some(j) = nodes.iter().position(|x| *x == l) {
                if i != j { edges.push((i, j)); }
            }
        }
    }
    Graph { nodes, edges }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![list_notes, read_note, write_note, render, graph])
        .run(tauri::generate_context!())
        .expect("tauri run");
}
