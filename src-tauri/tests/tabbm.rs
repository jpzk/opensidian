// SPDX-License-Identifier: GPL-3.0-or-later
//! issue-48 + issue #31: dragging a tab onto the Bookmarks pane.
//!
//! Runs the REAL `tabBmName` / `tabBmWhy` / `pvNoteName` / `cvTabPath` / `bmHasPath`
//! out of ui/main.js in JavaScriptCore (same engine as the webview, same pinned
//! dev-dep as listtoggle.rs), with `PLG`, `bmCache` and `bmTree` stubbed. An
//! Excalidraw board tab must bookmark as its note path (what the menus write since
//! c5d5837), a second drop must be refused as a duplicate, a note tab must still
//! bookmark as itself. Since #31 a canvas, a global graph and a local graph tab are
//! bookmarkable views too (one bm_add_view_at with tabBmItem's item); a canvas
//! already bookmarked is a dup, a graph never is. Views with nothing behind them
//! (a fileless / non-.md plugin view, no tab) stay refused.
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
    // PLG stub: one live Excalidraw leaf whose view.file is the board's .md, one live canvas leaf
    ctx.evaluate(
        "var bmCache = []; var bmTree = [];\
         var BOARD = { kind: 'unk', utype: 'excalidraw', name: 'Drawing 1' };\
         var CANVAS = { kind: 'unk', utype: 'canvas', name: 'Board' };\
         var PLG = { app: { workspace: { getLeavesOfType(t) {\
           return t === 'excalidraw' ? [{ _t: BOARD, view: { file: { path: 'Boards/Drawing 1.excalidraw.md' } } }]\
                : t === 'canvas' ? [{ _t: CANVAS, view: { path: 'Boards/Board.canvas' } }] : []; } } } };");
    // bmHasPath is a top-level `const` arrow, not a function declaration: load its one line
    let hp = src.lines().find(|l| l.starts_with("const bmHasPath = ")).expect("const bmHasPath not in ui/main.js");
    ctx.evaluate(&hp.replacen("const ", "var ", 1));
    if let Some(e) = ctx.exception() { panic!("bmHasPath did not load: {}", e.to_str()); }
    for f in ["pvNoteName", "cvTabPath", "tabBmName", "tabBmWhy"] {
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

/// issue #31: a canvas tab is a view bookmark (a Path item); once its path is in the tree it is a dup.
#[test]
fn canvas_tab_bookmarks_once() {
    let c = ctx();
    assert_eq!(ev(&c, &format!("{WHY}(CANVAS)")), "ok");
    // a restored, not-yet-mounted canvas: the persisted state's file is used
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'unk', utype:'canvas', name:'K', raw:{{state:{{state:{{file:'K.canvas'}}}}}}}})")), "ok");
    ev(&c, "bmTree = [{ path: true, name: 'Boards/Board.canvas' }]");
    assert_eq!(ev(&c, &format!("{WHY}(CANVAS)")), "no:dup");
    // a NOTE bookmark of the same string is not the canvas path item
    ev(&c, "bmTree = [{ name: 'Boards/Board.canvas' }]");
    assert_eq!(ev(&c, &format!("{WHY}(CANVAS)")), "ok");
}

/// issue #31: global and local graph tabs are bookmarkable and never a dup (each drop = a new graph item).
#[test]
fn graph_tabs_are_bookmarkable() {
    let c = ctx();
    ev(&c, "bmCache = ['Graph', 'Graph view']");
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'gg', name:'Graph'}})")), "ok");
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'lg', name:'Graph'}})")), "ok");
}

/// what has nothing behind it stays refused, with no backend call
#[test]
fn non_bookmarkable_views_stay_refused() {
    let c = ctx();
    // a plugin view whose file is not a vault .md (e.g. a pdf) is not a note
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'unk', utype:'pdfx', name:'p', raw:{{state:{{state:{{file:'a.pdf'}}}}}}}})")), "no:view");
    // a fileless plugin view, and a canvas leaf with no path at all
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'unk', utype:'pdfx', name:'p'}})")), "no:view");
    assert_eq!(ev(&c, &format!("{WHY}({{kind:'unk', utype:'canvas', name:'nopath'}})")), "no:view");
    assert_eq!(ev(&c, &format!("{WHY}({{name:''}})")), "no:view");
    assert_eq!(ev(&c, &format!("{WHY}(null)")), "no:view");
}

/// A note tab's drop writes the SAME name tabBmWhy checked, as ONE bm_add_at; a view tab
/// (graph / canvas / board) writes ONE bm_add_view_at with tabBmItem's item (issue #31).
#[test]
fn drop_writes_tab_bm_name() {
    let d = func(&main_js(), "tabDragStart");
    assert!(d.contains("bmApply(\"bm_add_at\", { parent: t.d.parent, pos: t.d.pos, name: tabBmName(bmTab) })"),
            "tabDragStart's bm_add_at must store tabBmName(bmTab)");
    assert_eq!(d.matches("\"bm_add_at\"").count(), 1, "the tab drop is ONE bm_add_at");
    assert!(d.contains("tabBmItem(bmTab).then(item => bmApply(\"bm_add_view_at\", { parent: t.d.parent, pos: t.d.pos, item }))"),
            "a view tab's drop must write tabBmItem(bmTab) via bm_add_view_at");
    assert_eq!(d.matches("\"bm_add_view_at\"").count(), 1, "the view-tab drop is ONE bm_add_view_at");
}
