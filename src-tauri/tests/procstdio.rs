// SPDX-License-Identifier: GPL-3.0-or-later
//! copilot-tty: the process.stdout / process.stderr a plugin gets (ui/plugins/node-desktop.js).
//!
//! Runs the REAL `stdioShim` / `processShim` out of node-desktop.js in JavaScriptCore (same
//! engine as the webview, same pinned dev-dep as listtoggle.rs) and asserts the surface equals
//! what stock 1.13.7 shows a plugin, recorded black box (devtools console values only:
//! typeof / property values / `in` / write return values) on 2026-11-19:
//!   stdout and stderr are two distinct objects, fd 1 and 2; NO isTTY, columns or rows
//!   property ("isTTY" in stdout is false: stock's renderer stdio is not a terminal stream);
//!   writable true; write/end/on/once/off/addListener/removeListener/emit are functions;
//!   none of the tty.WriteStream methods (getWindowSize, hasColors, getColorDepth, cursorTo,
//!   clearLine, moveCursor, clearScreenDown); write("") returns true, write("", cb) returns
//!   true and calls cb exactly once.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn node_desktop_js() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/plugins/node-desktop.js");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Source of the 2-space-indented `function NAME(...) { ... }` inside the IIFE: brace-matched,
/// skipping braces inside '...', "..." and `...` strings and // comments.
fn func(src: &str, name: &str) -> String {
    let head = format!("\n  function {name}(");
    let start = src.find(&head).unwrap_or_else(|| panic!("function {name} not in node-desktop.js")) + 3;
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
    let src = node_desktop_js();
    let ctx = javascriptcore::Context::new();
    // the shim defers write callbacks with queueMicrotask; JSC's bare context may lack it: a
    // synchronous queue the test drains by hand (FLUSH), so "called once" is observable
    // WIN: the IIFE's host test (node-desktop.js), evaluated there; processShim alone needs it
    ctx.evaluate("var WIN = false; var Q = []; var queueMicrotask = function (f) { Q.push(f); };\
                  function FLUSH() { while (Q.length) Q.shift()(); }");
    for f in ["stdioShim", "processShim"] {
        ctx.evaluate(&func(&src, f));
        if let Some(e) = ctx.exception() { panic!("{f} did not load: {}", e.to_str()); }
    }
    ctx.evaluate("var P = processShim();");
    if let Some(e) = ctx.exception() { panic!("processShim(): {}", e.to_str()); }
    ctx
}

/// process.platform names the host the IIFE detected (WIN): "linux" here, "win32" on Windows,
/// so a plugin picks the commands of the system it really runs on
#[test]
fn process_platform_names_the_host() {
    let c = ctx();
    assert_eq!(ev(&c, "P.platform"), "linux");
    c.evaluate("WIN = true; var PW = processShim();");
    if let Some(e) = c.exception() { panic!("processShim() on Windows: {}", e.to_str()); }
    assert_eq!(ev(&c, "PW.platform"), "win32");
}

fn ev(ctx: &javascriptcore::Context, js: &str) -> String {
    let v = ctx.evaluate(js).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("{js}: threw {}", e.to_str()); }
    v
}

const STOCK_FN: [&str; 8] = ["write", "end", "on", "once", "off", "addListener", "removeListener", "emit"];
const STOCK_NO_TTY: [&str; 7] = ["getWindowSize", "hasColors", "getColorDepth", "cursorTo", "clearLine", "moveCursor", "clearScreenDown"];

fn stream_matches_stock(c: &javascriptcore::Context, s: &str, fd: &str) {
    assert_eq!(ev(c, &format!("typeof P.{s}")), "object", "{s}");
    // stock 1.13.7: "isTTY" in process.stdout === false (undefined, not false)
    assert_eq!(ev(c, &format!("String('isTTY' in P.{s})")), "false", "{s}.isTTY must be absent like stock");
    assert_eq!(ev(c, &format!("String(P.{s}.isTTY)")), "undefined", "{s}.isTTY");
    assert_eq!(ev(c, &format!("String('columns' in P.{s}) + ',' + String('rows' in P.{s})")), "false,false", "{s} columns/rows must be absent like stock");
    assert_eq!(ev(c, &format!("typeof P.{s}.fd + ':' + P.{s}.fd")), format!("number:{fd}"), "{s}.fd");
    assert_eq!(ev(c, &format!("String(P.{s}.writable)")), "true", "{s}.writable");
    for f in STOCK_FN {
        assert_eq!(ev(c, &format!("typeof P.{s}.{f}")), "function", "{s}.{f}");
    }
    for f in STOCK_NO_TTY {
        assert_eq!(ev(c, &format!("typeof P.{s}.{f}")), "undefined", "{s}.{f} is a tty method stock does not have");
    }
    assert_eq!(ev(c, &format!("String(P.{s}.write(''))")), "true", "{s}.write('')");
    assert_eq!(ev(c, &format!("var N = 0; var R = P.{s}.write('', function () {{ N++; }}); FLUSH(); R + ':' + N")), "true:1", "{s}.write('', cb)");
}

#[test]
fn process_stdout_matches_stock() {
    let c = ctx();
    stream_matches_stock(&c, "stdout", "1");
}

#[test]
fn process_stderr_matches_stock() {
    let c = ctx();
    stream_matches_stock(&c, "stderr", "2");
    assert_eq!(ev(&c, "String(P.stdout !== P.stderr)"), "true");
}

/// stock: a plugin's process IS window.process. Ours installs one page-global shim with the
/// same stdio once the first plugin view is made; String() of it must not throw (page code
/// sniffing for Node does process + ""), a plugin may reassign it, an existing one is kept.
#[test]
fn page_global_process_has_the_stock_stdio() {
    let c = ctx();
    let src = node_desktop_js();
    ev(&c, "var window = {}; var pageProcess = false;");
    c.evaluate(&func(&src, "installPageProcess"));
    if let Some(e) = c.exception() { panic!("installPageProcess did not load: {}", e.to_str()); }
    ev(&c, "installPageProcess(); var P = window.process;");
    stream_matches_stock(&c, "stdout", "1");
    stream_matches_stock(&c, "stderr", "2");
    assert_eq!(ev(&c, "String(window.process)"), "[object Object]");
    assert_eq!(ev(&c, "Object.keys(window).length"), "0", "non-enumerable, like a plain global");
    assert_eq!(ev(&c, "installPageProcess(); String(window.process === P)"), "true", "installed once");
    assert_eq!(ev(&c, "window.process = 7; String(window.process)"), "7", "writable");
    // a page that already has a process keeps it
    assert_eq!(ev(&c, "window = { process: 'own' }; pageProcess = false; installPageProcess(); window.process"), "own");
}
