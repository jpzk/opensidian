// SPDX-License-Identifier: GPL-3.0-or-later
//! The launch restore must not write back the layout it just read.
//!
//! Runs the REAL `wsCanon` / `wsUnchanged` out of ui/main.js in JavaScriptCore
//! (same engine and pinned dev-dep as tabbm.rs): a layout equal to the restored
//! file, key order aside, is "unchanged"; any other layout, or any layout once
//! `wsFile` is forgotten, is not. The wiring around them (wsFlush skips, the
//! first write forgets, only the boot restore remembers) is pinned on the source.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn main_js() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/main.js");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Source of top-level `function NAME(...) { ... }`: brace-matched, skipping
/// braces inside '...', "..." and `...` strings and // comments (as tabbm.rs).
fn func(src: &str, name: &str) -> String {
    let start = [format!("\nfunction {name}("), format!("\nasync function {name}(")]
        .iter()
        .find_map(|h| src.find(h.as_str()))
        .unwrap_or_else(|| panic!("function {name} not in ui/main.js")) + 1;
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

fn ev(ctx: &javascriptcore::Context, js: &str) -> String {
    let v = ctx.evaluate(js).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("{js}: threw {}", e.to_str()); }
    v
}

#[test]
fn the_restored_layout_is_unchanged_until_something_is_written() {
    let src = main_js();
    let c = javascriptcore::Context::new();
    ev(&c, "var wsFile = '';");
    for f in ["wsCanon", "wsUnchanged"] {
        ev(&c, &func(&src, f));
    }
    // the file as serde wrote it (keys sorted) vs the same layout as wsDoc builds it
    ev(&c, "var FILE = {active:'a1', main:{children:[{id:'g', type:'tabs'}], direction:'vertical', id:'m'}, lastOpenFiles:['x.md','y.md']};");
    ev(&c, "var SAME = {main:{id:'m', direction:'vertical', children:[{type:'tabs', id:'g'}]}, active:'a1', lastOpenFiles:['x.md','y.md']};");
    ev(&c, "var MOVED = {main:{id:'m', direction:'vertical', children:[{type:'tabs', id:'g'}]}, active:'a1', lastOpenFiles:['y.md','x.md']};");
    // nothing restored: never "unchanged", every layout is written as before
    assert_eq!(ev(&c, "wsUnchanged(SAME)"), "false");
    ev(&c, "wsFile = wsCanon(FILE);");
    assert_eq!(ev(&c, "wsUnchanged(SAME)"), "true", "key order must not count");
    assert_eq!(ev(&c, "wsUnchanged(MOVED)"), "false", "array order is content");
    assert_eq!(ev(&c, "wsUnchanged({})"), "false");
    // the first real write forgets the file: going back to its layout writes again
    ev(&c, "wsFile = '';");
    assert_eq!(ev(&c, "wsUnchanged(SAME)"), "false");
}

/// A main split with one child comes back as that child; wrapped again it must keep
/// the file's id (R28.4) and direction, or wsNode writes a fresh id every launch.
#[test]
fn a_one_child_main_split_keeps_its_id_and_direction() {
    let src = main_js();
    let c = javascriptcore::Context::new();
    ev(&c, &func(&src, "wsMainWrap"));
    ev(&c, "var LEAF = { tabs: [], active: 0 };");
    assert_eq!(ev(&c, "(function(){ var w = wsMainWrap({id:'eb3c8a0239e5357f', type:'split', direction:'vertical'}, LEAF); return w.wid + ' ' + w.dir + ' ' + (w.children[0] === LEAF); })()"),
               "eb3c8a0239e5357f row true");
    assert_eq!(ev(&c, "wsMainWrap({id:'h1', direction:'horizontal'}, LEAF).dir"), "col");
    // no id on file: none invented here (wsIdOf draws one at the write, as before)
    assert_eq!(ev(&c, "String(wsMainWrap({type:'split'}, LEAF).wid)"), "undefined");
    // a real split is the file's own node already: returned as is
    assert_eq!(ev(&c, "(function(){ var r = {children:[1,2], wid:'x'}; return wsMainWrap({id:'y'}, r) === r; })()"), "true");
    assert!(src.contains("root = wsMainWrap(doc.main, root);"), "wsApply must wrap through wsMainWrap");
}

/// A top-level `const NAME = ...;` one-liner of ui/main.js, loaded as a `var`.
fn const_line(src: &str, name: &str) -> String {
    let head = format!("const {name} = ");
    let l = src.lines().find(|l| l.starts_with(&head)).unwrap_or_else(|| panic!("{head} not in ui/main.js"));
    l.replacen("const ", "var ", 1)
}

/// CodeAnt on #361: wsMainWrap alone proves nothing if wsNode does not write the wid it
/// keeps. The REAL wsNode + wsIdOf + emptyLeaf serialize the wrapped root (an empty
/// group, so no tab machinery is needed): the file's main id is the id written, and a
/// file without one still gets an id drawn at the write.
#[test]
fn wsnode_writes_the_main_id_the_wrap_kept() {
    let src = main_js();
    let c = javascriptcore::Context::new();
    ev(&c, "var leafBoot = true; var drawn = 0; var wsId = function () { return 'NEW' + (++drawn); };");
    for k in ["emptyLeaf", "wsIdOf"] {
        ev(&c, &const_line(&src, k));
    }
    for f in ["wsMainWrap", "wsNode"] {
        ev(&c, &func(&src, f));
    }
    assert_eq!(
        ev(&c, "(function(){ var o = wsNode(wsMainWrap({id:'eb3c8a0239e5357f', type:'split', direction:'vertical'}, {tabs:[], active:-1}));\
                 return [o.id, o.type, o.direction, o.children.length, o.children[0].type].join(' '); })()"),
        "eb3c8a0239e5357f split vertical 1 tabs"
    );
    assert_eq!(ev(&c, "wsNode(wsMainWrap({id:'h1', type:'split', direction:'horizontal'}, {tabs:[], active:-1})).direction"), "horizontal");
    assert!(ev(&c, "wsNode(wsMainWrap({type:'split'}, {tabs:[], active:-1})).id").starts_with("NEW"), "no id on file: one is drawn");
}

/// CodeAnt on #361: a guard that is merely BEFORE the write proves nothing if it does
/// not return. The REAL wsFlush runs with its I/O stubbed (inv records, setTimeout
/// records): while a restore runs it writes nothing and re-arms; the restored file is
/// skipped; a changed layout is written; a forced flush (wsLeave) is not held back.
#[test]
fn wsflush_writes_nothing_while_a_restore_runs() {
    let src = main_js();
    let c = javascriptcore::Context::new();
    ev(&c, "var state = {}, vaultPath = 'V', actN = 0, wsYields = 0, WS_YIELD_MS = 50, WS_YIELD_MAX = 40;\
            var wsT = null, wsLast = '', wsFile = '', wsWrites = 0, wsInFlight = null, wsApplying = 0;\
            var timers = [], calls = [], DOC = {a: 1};\
            function setTimeout(f, ms) { timers.push(ms); return 7; }\
            function inv(cmd, a) { calls.push(cmd); return new Promise(function () {}); }\
            function updateTitle() {}\
            function wsDoc() { return DOC; }");
    for f in ["wsCanon", "wsUnchanged", "wsFlush"] {
        ev(&c, &func(&src, f));
    }
    // a restore is running: no write, the flush is re-armed
    ev(&c, "wsApplying = 1; wsFlush();");
    assert_eq!(ev(&c, "calls.length + ' ' + timers.join(',') + ' ' + wsT"), "0 50 7");
    // restore done, layout = the file read at launch: skipped, and remembered as written
    ev(&c, "wsApplying = 0; wsFile = wsCanon(DOC); wsFlush();");
    assert_eq!(ev(&c, "calls.length + ' ' + (wsLast === JSON.stringify(DOC))"), "0 true");
    // a real change is written
    ev(&c, "DOC = {a: 2}; wsFlush();");
    assert_eq!(ev(&c, "calls.join(',')"), "write_workspace");
    // a forced flush (vault switch) goes even during a restore
    ev(&c, "wsInFlight = null; wsApplying = 1; DOC = {a: 3}; wsFlush(true);");
    assert_eq!(ev(&c, "calls.length"), "2");
}

#[test]
fn only_the_boot_restore_remembers_and_the_first_write_forgets() {
    let src = main_js();
    let flush = &src[src.find("async function wsFlush(").expect("wsFlush")..];
    let flush = &flush[..flush.find("\n}\n").expect("end of wsFlush")];
    let skip = flush.find("if (wsUnchanged(doc)) { wsLast = s; return; }").expect("wsFlush skips an unchanged layout");
    let write = flush.find("inv(\"write_workspace\"").expect("wsFlush writes");
    assert!(skip < write, "the skip must come before the write");
    assert!(flush.contains("wsLast = s; wsWrites++; wsFile = \"\";"), "a successful write must forget the file");
    let apply = &src[src.find("async function wsApplyIn(").expect("wsApplyIn")..];
    assert!(apply[..600].contains("wsFile = boot ? wsCanon(doc) : \"\";"), "only the launch restore (boot) remembers the file");
    // no flush while a restore is running: it would write a half-restored layout
    let wrap = &src[src.find("async function wsApply(").expect("wsApply")..];
    assert!(wrap[..200].contains("wsApplying++;") && wrap[..200].contains("finally { wsApplying--; }"), "wsApply counts itself");
    let wait = flush.find("if (!force && wsApplying > 0)").expect("wsFlush waits for a restore");
    assert!(wait < skip, "the wait must come before the skip and the write");
    let leave = &src[src.find("async function wsLeave(").expect("wsLeave")..];
    assert!(leave[..400].contains("wsFile = \"\";"), "a vault switch forgets the file");
}
