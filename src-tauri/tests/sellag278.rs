// SPDX-License-Identifier: GPL-3.0-or-later
//! #278 source-mode selection lag, run in JavaScriptCore like tests/listtoggle.rs runs ui/editor.js.
//!
//! The fix keeps the plain marker decoration for selection-touched markers in SOURCE mode (Ed.lpMk):
//! the .rv twin paints nothing there, but swapping it made CM6 re-create every touched marker span.
//! Checked here:
//!   1. Ed.lpMk: live preview still swaps to the .rv twin on a touched marker; source mode and untouched
//!      markers keep the plain deco (both modes).
//!   2. Ed.lpSrc reads .lp.src; no lp / no classList / no view = live preview.
//!   3. Ed.lpSrcSync rebuilds exactly once on a real mode flip, and never when the mode is unchanged,
//!      before the first build, without an editor, or without the lp effect.
//!   4. Ed.nodes: a marker inside .lp.src without .rv is visible text; in live preview it is hidden
//!      unless .rv (unchanged).
//!   5. the build path goes through Ed.lpMk (no direct .rv swap left in build).
//!   6. style.css: every rule revealing `.lp ... .X.rv` carries the `.lp.src ... .X` twin in the same
//!      selector list — the premise that the swap is invisible in source mode. The checker itself is
//!      shown to reject a rule without its twin.
//!   7. Ed.lpRuns: source mode draws adjacent element ranges with the same parent and spec as one span
//!      (Ctrl+A cost); other parents, specs, gaps, empty ranges and nesting never merge; the build uses it
//!      only in source mode and falls back per element where an overlay touches a run.
//!   8. style.css gives no source-mode marker ::before/::after content (a merge would drop one copy).
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..") }

const CHECKS: &str = r#"(function () {
  "use strict";
  const fail = m => { throw new Error(m); };
  // minimal elements: class list + closest() for the ".a.b" / ".a" selectors Ed.nodes and Ed.lpSrc use
  const el = (cls, parent) => {
    const set = new Set(cls.split(" ").filter(Boolean));
    const e = { parent, classList: { contains: c => set.has(c) } };
    e.closest = sel => { const want = sel.split(".").filter(Boolean);
      for (let p = e; p; p = p.parent) if (want.every(c => p.classList.contains(c))) return p; return null; };
    return e;
  };
  // 1. Ed.lpMk
  const x = { d: "plain", rv: "reveal" };
  if (Ed.lpMk(x, true, false) !== "reveal") fail("lpMk: live preview touched marker must use .rv");
  if (Ed.lpMk(x, false, false) !== "plain") fail("lpMk: live preview untouched marker must stay plain");
  if (Ed.lpMk(x, true, true) !== "plain") fail("lpMk: source mode touched marker must stay plain (#278)");
  if (Ed.lpMk(x, false, true) !== "plain") fail("lpMk: source mode untouched marker must stay plain");
  const y = { d: "same", rv: "same" };   // a non-marker element: d === rv (Ed.lpParse)
  if (Ed.lpMk(y, true, false) !== "same" || Ed.lpMk(y, true, true) !== "same") fail("lpMk: non-marker");
  // 2. Ed.lpSrc
  if (Ed.lpSrc({ lp: el("lp src") }) !== true) fail("lpSrc: .lp.src is source mode");
  if (Ed.lpSrc({ lp: el("lp") }) !== false) fail("lpSrc: .lp is live preview");
  if (Ed.lpSrc({}) !== false || Ed.lpSrc(null) !== false || Ed.lpSrc({ lp: {} }) !== false) fail("lpSrc: missing parts");
  // 3. Ed.lpSrcSync
  const mk = (src, built) => { const calls = [];
    const v = { lp: el(src ? "lp src" : "lp"), cm: { dispatch: t => calls.push(t) }, _lpFx: { of: a => ({ fx: a }) }, _lpSrc: built };
    return { g: { view: v }, calls, v };
  };
  let t = mk(true, false);
  if (Ed.lpSrcSync(t.g) !== true || t.calls.length !== 1 || !t.calls[0].effects) fail("lpSrcSync: lp -> source must rebuild once");
  t = mk(false, true);
  if (Ed.lpSrcSync(t.g) !== true || t.calls.length !== 1) fail("lpSrcSync: source -> lp must rebuild once");
  t = mk(true, true);
  if (Ed.lpSrcSync(t.g) !== false || t.calls.length) fail("lpSrcSync: source -> source must not dispatch");
  t = mk(false, false);
  if (Ed.lpSrcSync(t.g) !== false || t.calls.length) fail("lpSrcSync: lp -> lp must not dispatch");
  t = mk(true, undefined);
  if (Ed.lpSrcSync(t.g) !== false || t.calls.length) fail("lpSrcSync: before the first build must not dispatch");
  t = mk(true, false); delete t.v._lpFx;
  if (Ed.lpSrcSync(t.g) !== false || t.calls.length) fail("lpSrcSync: no lp effect must not dispatch");
  t = mk(true, false); delete t.v.cm;
  if (Ed.lpSrcSync(t.g) !== false) fail("lpSrcSync: no editor");
  if (Ed.lpSrcSync(null) !== false || Ed.lpSrcSync({}) !== false) fail("lpSrcSync: no pane / no view");
  // 4. Ed.nodes
  const row = (lpCls, mkCls) => { const lp = el(lpCls), r = el("cm-line", lp), m = el(mkCls, r);
    const texts = [{ nodeValue: "**", parentElement: m }, { nodeValue: "bold", parentElement: r }];
    r.texts = texts; return r; };
  document.createTreeWalker = r => { let i = 0; return { nextNode: () => r.texts[i++] || null }; };
  const hid = r => Ed.nodes(r).map(n => n.hid).join(",");
  if (hid(row("lp src", "mk")) !== "false,false") fail("nodes: a source-mode marker is visible text: " + hid(row("lp src", "mk")));
  if (hid(row("lp src", "mk rv")) !== "false,false") fail("nodes: a source-mode .rv marker is visible");
  if (hid(row("lp", "mk")) !== "true,false") fail("nodes: a live-preview marker without .rv stays hidden: " + hid(row("lp", "mk")));
  if (hid(row("lp", "mk rv")) !== "false,false") fail("nodes: a live-preview .rv marker is visible");
  // 5. Ed.lpRuns (source-mode span merge): same parent + same spec + touching -> one run; else separate
  const P1 = {}, P2 = {};
  const runs = ks => JSON.stringify(Ed.lpRuns(ks).map(r => [r.i, r.j, r.s, r.e]));
  const K = (s, e, pe, k) => ({ s, e, pe, k });
  if (runs([]) !== "[]") fail("lpRuns: empty");
  if (runs([K(0, 1, P1, "mk"), K(1, 2, P1, "mk")]) !== "[[0,2,0,2]]") fail("lpRuns: '#'+' ' must merge: " + runs([K(0, 1, P1, "mk"), K(1, 2, P1, "mk")]));
  if (runs([K(0, 1, P1, "mk"), K(1, 2, P1, "mk"), K(2, 5, P1, "mk")]) !== "[[0,3,0,5]]") fail("lpRuns: three in a row");
  if (runs([K(0, 1, P1, "mk"), K(1, 2, P2, "mk")]) !== "[[0,1,0,1],[1,2,1,2]]") fail("lpRuns: other parent must not merge");
  if (runs([K(0, 1, P1, "mk"), K(1, 2, P1, "mk url")]) !== "[[0,1,0,1],[1,2,1,2]]") fail("lpRuns: other spec must not merge");
  if (runs([K(0, 1, P1, "mk"), K(2, 3, P1, "mk")]) !== "[[0,1,0,1],[1,2,2,3]]") fail("lpRuns: a gap must not merge");
  if (runs([K(0, 1, P1, "mk"), K(1, 1, P1, "mk"), K(1, 2, P1, "mk")]) !== "[[0,1,0,1],[1,3,1,2]]") fail("lpRuns: an empty range never extends a run: " + runs([K(0, 1, P1, "mk"), K(1, 1, P1, "mk"), K(1, 2, P1, "mk")]));
  // nested: a child element between two siblings breaks the adjacency (pre-order), nothing merges across it
  if (runs([K(0, 4, P1, "b"), K(0, 2, "B", "mk"), K(2, 4, "B", "mk"), K(4, 6, P1, "b")]) !== "[[0,1,0,4],[1,3,0,4],[3,4,4,6]]") fail("lpRuns: nested");
  return "ok";
})()"#;

#[test]
fn sellag278_js() {
    let ed = std::fs::read_to_string(root().join("ui/editor.js")).unwrap();
    let ctx = javascriptcore::Context::new();
    // as tests/listtoggle.rs: editor.js only needs these at load (CENSUS_ON off, as in a shipped build)
    ctx.evaluate("var window = {}; var document = { addEventListener() {} }; var CENSUS_ON = false; var NodeFilter = { SHOW_TEXT: 4 };");
    ctx.evaluate(&ed);
    if let Some(e) = ctx.exception() { panic!("editor.js did not load: {}", e.to_str()); }
    let out = ctx.evaluate(CHECKS).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("#278 check failed: {}", e.to_str()); }
    assert_eq!(out, "ok");
}

#[test]
fn sellag278_build_uses_lpmk() {
    let ed = std::fs::read_to_string(root().join("ui/editor.js")).unwrap();
    let start = ed.find("Ed.lpExt = function").expect("Ed.lpExt missing");
    let body = &ed[start..];
    let end = body.find("\nEd.").unwrap_or(body.len());
    let body = &body[..end];
    assert!(body.contains("Ed.lpMk(x, "), "lpExt build no longer picks the marker deco through Ed.lpMk (#278)");
    assert!(!body.contains("? x.rv : x.d"), "lpExt build swaps x.rv directly again (#278: source-mode DOM churn)");
    assert!(body.contains("v._lpSrc = src"), "lpExt build no longer records the mode it built for (Ed.lpSrcSync)");
    let main = std::fs::read_to_string(root().join("ui/main.js")).unwrap();
    let am = &main[main.find("function applyMode(").expect("applyMode missing")..];
    let am = &am[..am.find("\n}\n").unwrap()];
    assert!(am.contains("Ed.lpSrcSync(g)"), "applyMode no longer rebuilds the lp decorations on a source/lp flip (#278)");
}

/// selectors of `css` that reveal a marker via `.rv` but lack their `.lp.src` twin in the same rule
fn missing_src_twins(css: &str) -> Vec<String> {
    let mut s = String::new();
    let mut rest = css;
    while let Some(i) = rest.find("/*") {          // strip comments
        s.push_str(&rest[..i]);
        rest = rest[i + 2..].find("*/").map(|j| &rest[i + 2 + j + 2..]).unwrap_or("");
    }
    s.push_str(rest);
    let mut bad = Vec::new();
    for chunk in s.split('}') {
        let Some((sel, _)) = chunk.split_once('{') else { continue };
        let sels: Vec<String> = sel.split(',').map(|x| x.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
        for x in &sels {
            if !x.contains(".rv") { continue; }
            let twin = x.replacen(".lp ", ".lp.src ", 1).replace(".rv", "");
            if !x.starts_with(".lp ") || !sels.contains(&twin) { bad.push(x.clone()); }
        }
    }
    bad
}

#[test]
fn sellag278_css_src_twins() {
    // the checker: a twin-less .rv rule is caught, a twinned one passes, an unscoped one is caught
    assert_eq!(missing_src_twins(".lp .mk.rv { color: red }"), vec![".lp .mk.rv".to_string()]);
    assert!(missing_src_twins(".lp.src .mk, .lp .mk.rv { color: red }").is_empty());
    assert_eq!(missing_src_twins("/* .lp .x.rv */ .lp.src .li > .x, .lp .li > .x.rv {} .mk.rv { a: b }"), vec![".mk.rv".to_string()]);
    for f in ["ui/style.css", "ui/tables.css", "ui/menu.css"] {
        let css = std::fs::read_to_string(root().join(f)).unwrap();
        let bad = missing_src_twins(&css);
        assert!(bad.is_empty(), "{f}: .rv rules without their .lp.src twin (source mode would not show them without .rv, #278): {bad:?}");
    }
    let css = std::fs::read_to_string(root().join("ui/style.css")).unwrap();
    assert!(css.contains(".lp.src .mk, .lp .mk.rv"), "style.css: the base marker reveal rule moved; re-check #278");
}

#[test]
fn sellag278_build_merges_only_in_source() {
    let ed = std::fs::read_to_string(root().join("ui/editor.js")).unwrap();
    let start = ed.find("Ed.lpExt = function").expect("Ed.lpExt missing");
    let body = &ed[start..];
    let body = &body[..body.find("\nEd.").unwrap_or(body.len())];
    assert!(body.contains("if (src && P.runs) for (const r of P.runs)"), "lpExt: the source-mode run merge (Ed.lpRuns) is gone or no longer gated on source mode (#278)");
    assert!(body.contains("else for (let i = 0; i < P.els.length; i++) emit(i);"), "lpExt: live preview must keep one span per element (#278)");
    assert!(body.contains("else for (let i = r.i; i < r.j; i++) emit(i);"), "lpExt: an overlay-touched run must fall back to its elements (#278)");
    let p = &ed[ed.find("Ed.lpParse = function").expect("Ed.lpParse missing")..];
    let p = &p[..p.find("\nEd.").unwrap_or(p.len())];
    assert!(p.contains("P.runs = Ed.lpRuns(keys)"), "lpParse no longer computes the merged runs (#278)");
}

/// marker selectors (`.mk` + ::before/::after) whose generated content would differ between one merged
/// span and two sibling spans in SOURCE mode: an unscoped or `.lp.src` rule with content other than none,
/// or a `.lp ` rule with content whose `.lp.src` twin does not set `content: none`
fn src_marker_content(css: &str) -> Vec<String> {
    let mut s = String::new();
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        s.push_str(&rest[..i]);
        rest = rest[i + 2..].find("*/").map(|j| &rest[i + 2 + j + 2..]).unwrap_or("");
    }
    s.push_str(rest);
    let mut rules: Vec<(Vec<String>, bool)> = Vec::new();   // (selectors, content is none/absent)
    for chunk in s.split('}') {
        let Some((sel, decl)) = chunk.split_once('{') else { continue };
        let none = decl.split(';').filter_map(|d| d.split_once(':')).filter(|(k, _)| k.trim() == "content")
            .all(|(_, v)| v.trim() == "none");
        let sels = sel.split(',').map(|x| x.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|x| !x.is_empty()).collect();
        rules.push((sels, none));
    }
    let none_sels: Vec<&String> = rules.iter().filter(|r| r.1).flat_map(|r| r.0.iter()).collect();
    let mut bad = Vec::new();
    for (sels, none) in &rules {
        if *none { continue; }
        for x in sels {
            if !x.contains(".mk") || !(x.contains("::before") || x.contains("::after") || x.contains(":before") || x.contains(":after")) { continue; }
            if !x.starts_with(".lp ") { bad.push(x.clone()); continue; }
            let twin = x.replacen(".lp ", ".lp.src ", 1);
            if !none_sels.iter().any(|n| **n == twin) { bad.push(x.clone()); }
        }
    }
    bad
}

#[test]
fn sellag278_css_no_src_marker_content() {
    // the checker: content on a source-mode marker is caught, a .lp rule with a none twin passes
    assert_eq!(src_marker_content(".lp .li > .mk.lim::before { content: \"\" }"), vec![".lp .li > .mk.lim::before".to_string()]);
    assert!(src_marker_content(".lp .li > .mk.lim::before { content: \"\" } .lp.src .li > .mk.lim::before { content: none }").is_empty());
    assert_eq!(src_marker_content(".mk::after { content: \"x\" }"), vec![".mk::after".to_string()]);
    assert_eq!(src_marker_content(".lp.src .mk::after { content: \"x\" }"), vec![".lp.src .mk::after".to_string()]);
    assert!(src_marker_content("/* .mk::after { content: \"x\" } */ .lp .mk::after { content: none }").is_empty());
    for f in ["ui/style.css", "ui/tables.css", "ui/menu.css"] {
        let css = std::fs::read_to_string(root().join(f)).unwrap();
        let bad = src_marker_content(&css);
        assert!(bad.is_empty(), "{f}: source-mode marker pseudo content - Ed.lpRuns merges adjacent equal marker spans in source mode, which would drop one copy (#278): {bad:?}");
    }
}
