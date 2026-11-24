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

#[test]
fn only_the_boot_restore_remembers_and_the_first_write_forgets() {
    let src = main_js();
    let flush = &src[src.find("async function wsFlush(").expect("wsFlush")..];
    let flush = &flush[..flush.find("\n}\n").expect("end of wsFlush")];
    let skip = flush.find("if (wsUnchanged(doc)) { wsLast = s; return; }").expect("wsFlush skips an unchanged layout");
    let write = flush.find("inv(\"write_workspace\"").expect("wsFlush writes");
    assert!(skip < write, "the skip must come before the write");
    assert!(flush.contains("wsLast = s; wsWrites++; wsFile = \"\";"), "a successful write must forget the file");
    let apply = &src[src.find("async function wsApply(").expect("wsApply")..];
    assert!(apply[..600].contains("wsFile = boot ? wsCanon(doc) : \"\";"), "only the launch restore (boot) remembers the file");
    let leave = &src[src.find("async function wsLeave(").expect("wsLeave")..];
    assert!(leave[..400].contains("wsFile = \"\";"), "a vault switch forgets the file");
}
