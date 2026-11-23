// SPDX-License-Identifier: GPL-3.0-or-later
//! #291 document-navigation keys: the REAL `Ed.navKey` / `Ed.navTarget` out of ui/editor.js,
//! run in JavaScriptCore (webkit2gtk's own engine, as tests/listtoggle.rs).
//!
//! Linux rows = stock Obsidian 1.13.7 measured black-box on the box (harness phase navkeysstock,
//! 3200-line note, live preview + source): Ctrl+Home/End = document start/end, Ctrl+Up/Down = one
//! SOURCE line, same column clamped, PageDown/PageUp = one page (CM6 page move, view follows). macOS rows = the platform convention (Cmd+Up/Down = document
//! start/end); macOS cannot run on the box, so this mapping table IS its test. Negative rows: keys
//! that must stay NOT ours (null) on each platform.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn ctx() -> javascriptcore::Context {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let ed = std::fs::read_to_string(root.join("ui/editor.js")).unwrap();
    let ctx = javascriptcore::Context::new();
    ctx.evaluate("var window = {}; var document = { addEventListener() {} }; var CENSUS_ON = false;");
    ctx.evaluate(&ed);
    if let Some(e) = ctx.exception() { panic!("editor.js did not load: {}", e.to_str()); }
    for f in ["navKey", "navTarget", "navRun", "isMac"] {
        assert_eq!(ctx.evaluate(&format!("typeof Ed.{f}")).unwrap().to_str().as_str(), "function", "Ed.{f} missing");
    }
    ctx
}

/// "ctrl+shift+ArrowDown" -> a keydown-shaped object literal
fn ev(spec: &str) -> String {
    let mut parts: Vec<&str> = spec.split('+').collect();
    let key = parts.pop().unwrap();
    let m = |n: &str| parts.contains(&n);
    format!("{{key:{:?},ctrlKey:{},metaKey:{},shiftKey:{},altKey:{},isComposing:false}}",
            key, m("ctrl"), m("meta"), m("shift"), m("alt"))
}

fn nav(c: &javascriptcore::Context, spec: &str, mac: bool) -> String {
    let js = format!("(function(){{var r=Ed.navKey({},{});return r?r.to+(r.extend?'+ext':''):'null'}})()", ev(spec), mac);
    let v = c.evaluate(&js).unwrap().to_str().to_string();
    if let Some(e) = c.exception() { panic!("{spec}: {}", e.to_str()); }
    v
}

#[test]
fn navkey_linux_stock_table() {
    let c = ctx();
    let rows = [
        ("ctrl+Home", "docStart"), ("ctrl+End", "docEnd"),
        ("ctrl+shift+Home", "docStart+ext"), ("ctrl+shift+End", "docEnd+ext"),
        ("ctrl+ArrowDown", "lineDown"), ("ctrl+ArrowUp", "lineUp"),
        ("PageDown", "pageDown"), ("PageUp", "pageUp"), ("shift+PageDown", "pageDown+ext"), ("shift+PageUp", "pageUp+ext"),
        // negatives: not ours on Linux
        ("ArrowDown", "null"), ("ArrowUp", "null"), ("Home", "null"), ("End", "null"),
        ("ctrl+PageDown", "null"), ("ctrl+PageUp", "null"), ("ctrl+shift+PageDown", "null"), ("alt+PageDown", "null"),   // tab switching: not ours
        ("shift+ArrowDown", "null"),
        ("meta+ArrowDown", "null"), ("meta+ArrowUp", "null"),          // Super: the window manager's
        ("ctrl+alt+ArrowDown", "null"), ("alt+ArrowUp", "null"), ("ctrl+alt+End", "null"),
        ("ctrl+shift+ArrowDown", "null"), ("ctrl+meta+ArrowDown", "null"), ("ctrl+ArrowLeft", "null"),
    ];
    let bad: Vec<String> = rows.iter().filter_map(|(k, w)| { let g = nav(&c, k, false); (g != *w).then(|| format!("{k}: got {g}, want {w}")) }).collect();
    assert!(bad.is_empty(), "linux navKey rows differ:\n{}", bad.join("\n"));
}

#[test]
fn navkey_macos_convention_table() {
    let c = ctx();
    let rows = [
        ("meta+ArrowUp", "docStart"), ("meta+ArrowDown", "docEnd"),
        ("meta+shift+ArrowUp", "docStart+ext"), ("meta+shift+ArrowDown", "docEnd+ext"),
        ("meta+Home", "docStart"), ("meta+End", "docEnd"),
        // negatives: not ours on macOS
        ("ctrl+ArrowDown", "null"), ("ctrl+ArrowUp", "null"),           // Mission Control / App windows
        ("alt+ArrowDown", "null"), ("meta+alt+ArrowUp", "null"),        // paragraph moves: native
        ("PageDown", "pageDown"), ("shift+PageUp", "pageUp+ext"),     // same page move on every platform
        ("meta+PageDown", "null"), ("ctrl+PageUp", "null"),
        ("ctrl+meta+ArrowDown", "null"), ("ArrowDown", "null"), ("meta+ArrowLeft", "null"),
    ];
    let bad: Vec<String> = rows.iter().filter_map(|(k, w)| { let g = nav(&c, k, true); (g != *w).then(|| format!("{k}: got {g}, want {w}")) }).collect();
    assert!(bad.is_empty(), "macOS navKey rows differ:\n{}", bad.join("\n"));
    // composing (IME) and a missing event are never navigation
    assert_eq!(c.evaluate("String(Ed.navKey({key:'ArrowDown',metaKey:true,isComposing:true},true))").unwrap().to_str().as_str(), "null");
    assert_eq!(c.evaluate("String(Ed.navKey(null,false))").unwrap().to_str().as_str(), "null");
}

#[test]
fn navtarget_long_note_is_the_document_not_the_viewport() {
    let c = ctx();
    // 3200 lines + the trailing empty line, like the harness fixture: the target is computed on the
    // whole document, so nothing depends on which lines CM6 has rendered
    c.evaluate("var L=[]; for (var i=0;i<3200;i++) L.push(i%7==3?'':'L'+i+' some text'); L.push('');");
    let t = |nav: &str, l: u32, col: u32| -> String {
        let js = format!("(function(){{var t=Ed.navTarget({{to:{nav:?}}},{{l:{l},c:{col}}},L);return t.l+':'+t.c}})()");
        c.evaluate(&js).unwrap().to_str().to_string()
    };
    assert_eq!(t("docEnd", 0, 0), "3200:0");
    assert_eq!(t("docStart", 3200, 0), "0:0");
    assert_eq!(t("docStart", 1700, 5), "0:0");
    assert_eq!(t("lineDown", 0, 0), "1:0");
    assert_eq!(t("lineUp", 3200, 0), "3199:0");
    assert_eq!(t("lineDown", 1, 4), "2:4");          // same column
    assert_eq!(t("lineDown", 2, 9), "3:0");          // clamped to an empty line
    assert_eq!(t("lineUp", 4, 9), "3:0");
    // negatives: the first / last line keep the caret where it is
    assert_eq!(t("lineUp", 0, 3), "0:3");
    assert_eq!(t("lineDown", 3200, 0), "3200:0");
}

#[test]
fn navtarget_properties_floor_and_goal_column() {
    let c = ctx();
    // the harness fixture's head: frontmatter L1-L4 (Properties block in live preview), a heading,
    // long lines, an empty line, a short heading
    c.evaluate("var P=['---','title: t','tags: [a]','---','# L0005 head','L0006 '+'x'.repeat(300),'L0007 '+'y'.repeat(300),'','### L0009 sub','L0010 tail'];");
    let t = |nav: &str, l: u32, col: u32, lo: &str, goal: &str| -> String {
        let js = format!("(function(){{var t=Ed.navTarget({{to:{nav:?}}},{{l:{l},c:{col}}},P,{lo},{goal});return t.l+':'+t.c}})()");
        let v = c.evaluate(&js).unwrap().to_str().to_string();
        if let Some(e) = c.exception() { panic!("{nav}: {}", e.to_str()); }
        v
    };
    // live preview, Properties block over lines 0-3: Ctrl+Home lands on the body's first line (stock L5)
    assert_eq!(t("docStart", 9, 4, "4", "undefined"), "4:0");
    assert_eq!(t("docStart", 9, 4, "4", "77"), "4:0");          // a goal column never applies to docStart
    assert_eq!(t("lineUp", 5, 3, "4", "undefined"), "4:3");
    assert_eq!(t("lineUp", 4, 3, "4", "undefined"), "4:3");     // negative: never up INTO the block
    assert_eq!(t("docEnd", 0, 0, "4", "undefined"), "9:10");
    // source mode (no block): the frontmatter is text, document start is line 0
    assert_eq!(t("docStart", 9, 4, "0", "undefined"), "0:0");
    assert_eq!(t("lineUp", 4, 2, "0", "undefined"), "3:2");
    // negative: a floor past the end is clamped, a missing floor is 0
    assert_eq!(t("docStart", 3, 0, "99", "undefined"), "9:0");
    assert_eq!(t("docStart", 3, 0, "undefined", "undefined"), "0:0");
    // a caret already above the floor (the block just appeared) is not dragged down by lineUp
    assert_eq!(t("lineUp", 2, 1, "4", "undefined"), "2:1");
    // goal column (stock cdowng: L6 col 79 -> empty L8 -> L9 col 20 = the heading's length)
    assert_eq!(t("lineDown", 6, 0, "0", "79"), "7:0");           // empty line: clamped to 0
    assert_eq!(t("lineDown", 7, 0, "0", "79"), "8:13");          // goal kept, clamped to the line
    assert_eq!(t("lineDown", 8, 13, "0", "5"), "9:5");
    // negative: no goal = the caret's own column
    assert_eq!(t("lineDown", 5, 12, "0", "undefined"), "6:12");
}
