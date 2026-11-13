// SPDX-License-Identifier: GPL-3.0-or-later
//! issue-48: dragging a tab onto the Bookmarks pane.
//!
//! Runs the REAL `tabBmName` / `tabBmWhy` / `pvNoteName` out of ui/main.js in
//! JavaScriptCore (same engine as the webview, same pinned dev-dep as
//! listtoggle.rs), with `PLG` and `bmCache` stubbed. An Excalidraw board tab must
//! bookmark as its note path (what the menus write since c5d5837), a second drop
//! must be refused as a duplicate, a note tab must still bookmark as itself, and
//! canvas / graph / non-.md views stay refused.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn main_js() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/main.js");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Source of top-level `function NAME(...) { ... }`: brace-matched, skipping
/// braces inside '...', "..." and `...` strings and // comments.
fn func(src: &str, name: &str) -> String {
    let head = format!("\nfunction {name}(");
    let start = src.find(&head).unwrap_or_else(|| panic!("function {name} not in ui/main.js")) + 1;
    let b = src.as_bytes();
    let (mut i, mut depth, mut q, mut seen) = (start, 0i32, 0u8, false);
    while i < b.len() {
        let c = b[i];
        if q != 0 {
            if c == b'\\' { i += 2; continue; }
            if c == q { q = 0; }
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' { i += 1; }
            continue;
        } else if c == b'"' || c == b'\'' || c == b'`' {
            q = c;
        } else if c == b'{' {
            depth += 1; seen = true;
        } else if c == b'}' {
            depth -= 1;
            if seen && depth == 0 { return src[start..=i].to_string(); }
        }
        i += 1;
    }
    panic!("unbalanced function {name}");
}

fn ctx() -> javascriptcore::Context {
    let src = main_js();
    let ctx = javascriptcore::Context::new();
    // PLG stub: one live Excalidraw leaf whose view.file is the board's .md
    ctx.evaluate(
        "var bmCache = [];\
         var BOARD = { kind: 'unk', utype: 'excalidraw', name: 'Drawing 1' };\
         var PLG = { app: { workspace: { getLeavesOfType(t) {\
           return t === 'excalidraw' ? [{ _t: BOARD, view: { file: { path: 'Boards/Drawing 1.excalidraw.md' } } }] : []; } } } };");
    for f in ["pvNoteName", "tabBmName", "tabBmWhy"] {
        ctx.evaluate(&func(&src, f));
        if let Some(e) = ctx.exception() { panic!("{f} did not load: {}", e.to_str()); }
    }
    ctx
}

fn ev(ctx: &javascriptcore::Context, js: &str) -> String {
    let v = ctx.evaluate(js).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("{js}: threw {}", e.to_str()); }
    v
}

const WHY: &str = "(function(t){ var w = tabBmWhy(t); return w ? 'no:' + w.code : 'ok'; })";

#[test]
fn excalidraw_tab_bookmarks_its_note_once() {
    let c = ctx();
    // the board bookmarks as its NOTE path (path without .md), never its title
    assert_eq!(ev(&c, "tabBmName(BOARD)"), "Boards/Drawing 1.excalidraw");
    assert_eq!(ev(&c, &format!("{WHY}(BOARD)")), "ok");
    // a restored, not-yet-mounted board: the persisted state's file is used
    assert_eq!(ev(&c, "tabBmName({kind:'unk', utype:'excalidraw', name:'B', raw:{state:{state:{file:'B.excalidraw.md'}}}})"),
               "B.excalidraw");
    // after the first drop the pane holds exactly that name -> the second drop is a dup
    ev(&c, "bmCache = [tabBmName(BOARD)]");
    assert_eq!(ev(&c, &format!("{WHY}(BOARD)")), "no:dup");
}

#[test]
fn note_tab_still_bookmarks_as_itself() {
    let c = ctx();
    assert_eq!(ev(&c, "tabBmName({name:'Notes/a'})"), "Notes/a");
    assert_eq!(ev(&c, &format!("{WHY}({{name:'Notes/a'}})")), "ok");
    ev(&c, "bmCache = ['Notes/a']");
    assert_eq!(ev(&c, &format!("{WHY}({{name:'Notes/a'}})")), "no:dup");
}

#[test]
fn canvas_graph_and_non_note_views_stay_refused() {
    let c = ctx();
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'unk', utype:'canvas', name:'c'}})")), "no:canvas");
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'gg', name:'Graph'}})")), "no:graph");
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'lg', name:'Graph'}})")), "no:graph");
    // a plugin view whose file is not a vault .md (e.g. a pdf) is not a note
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'unk', utype:'pdfx', name:'p', raw:{{state:{{state:{{file:'a.pdf'}}}}}}}})")), "no:view");
    assert_eq!(ev(&c, &format!("{WHY}(null)")), "no:view");
}

/// The drop writes the SAME name tabBmWhy checked (a board's note path), not the tab title.
#[test]
fn drop_writes_tab_bm_name() {
    let d = func(&main_js(), "tabDragStart");
    assert!(d.contains("bmApply(\"bm_add_at\", { parent: t.d.parent, pos: t.d.pos, name: tabBmName(bmTab) })"),
            "tabDragStart's bm_add_at must store tabBmName(bmTab)");
    assert_eq!(d.matches("\"bm_add_at\"").count(), 1, "the tab drop is ONE bm_add_at");
}
