// SPDX-License-Identifier: GPL-3.0-or-later
//! #272 hanging indent: the Live Preview list row is split into span.lpre (indent + marker
//! [+ checkbox + "[ ] "]) and span.lbody (the item text), and ui/style.css lays a row that
//! OPENS with lpre directly followed by lbody out side by side, so a wrapped continuation
//! line starts under the item text (stock Obsidian 1.13.7, goal confirm-097: text x=403,
//! continuation x=401-402).
//!
//! Runs the REAL `Ed.row` out of ui/editor.js in JavaScriptCore (as tests/listtoggle.rs does;
//! no new crate) over a minimal DOM stand-in — the row renderer only builds elements and text
//! nodes — and checks the row SHAPE the CSS keys on, plus that the split changes neither the
//! row's text nor its R17.6 token ranges. Negative rows: no split where the CSS must not apply.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..") }

/// Just enough DOM for Ed.row: element / text nodes, appendChild (moves), className +
/// classList, textContent, siblings. Unknown queries answer "nothing" (no leaf matching).
const DOM: &str = r#"
var window = {}; var CENSUS_ON = false;
function TNode(s) { this.nodeType = 3; this.nodeValue = s; this.parentNode = null; this.childNodes = []; }
function ENode(tag) { this.nodeType = 1; this.nodeName = tag.toUpperCase(); this.tagName = this.nodeName;
  this.className = ""; this.childNodes = []; this.parentNode = null; this.dataset = {}; this.attributes = []; this.style = {}; }
var P = { appendChild(c) { if (c.parentNode) { var a = c.parentNode.childNodes; a.splice(a.indexOf(c), 1); }
    c.parentNode = this; this.childNodes.push(c); return c; },
  get firstChild() { return this.childNodes[0] || null; },
  get textContent() { return this.nodeType === 3 ? this.nodeValue : this.childNodes.map(function (c) { return c.textContent; }).join(""); },
  get previousSibling() { if (!this.parentNode) return null; var a = this.parentNode.childNodes, i = a.indexOf(this); return i > 0 ? a[i - 1] : null; },
  get nextSibling() { if (!this.parentNode) return null; var a = this.parentNode.childNodes, i = a.indexOf(this); return a[i + 1] || null; } };
Object.defineProperties(TNode.prototype, Object.getOwnPropertyDescriptors(P));
Object.defineProperties(ENode.prototype, Object.getOwnPropertyDescriptors(P));
Object.defineProperty(ENode.prototype, "classList", { get() { var e = this; return {
  add(c) { var l = e.className ? e.className.split(" ") : []; if (l.indexOf(c) < 0) l.push(c); e.className = l.join(" "); },
  remove(c) { e.className = e.className.split(" ").filter(function (x) { return x !== c; }).join(" "); },
  contains(c) { return e.className.split(" ").indexOf(c) >= 0; } }; } });
Object.defineProperty(ENode.prototype, "children", { get() { return this.childNodes.filter(function (c) { return c.nodeType === 1; }); } });
ENode.prototype.addEventListener = function () {};
ENode.prototype.setAttribute = function (k, v) { this.attributes.push({ name: k, value: String(v) }); };
ENode.prototype.matches = function () { return false; };
ENode.prototype.querySelector = function () { return null; };
ENode.prototype.querySelectorAll = function () { return []; };
var document = { addEventListener() {}, createElement(t) { return new ENode(t); }, createTextNode(s) { return new TNode(s); } };
"#;

/// A row as a compact string: each child of the row (or of `sel` inside it) as
/// "tag.classes[text]" joined by " | ", text nodes as "#text[...]".
const SHAPE: &str = r#"
function shape(n) { return n.childNodes.map(function (c) {
  return c.nodeType === 3 ? "#text[" + c.nodeValue + "]" : c.nodeName.toLowerCase() + "." + c.className.split(" ").join(".") + "[" + c.textContent + "]"; }).join(" | "); }
function find(n, cls) { if (n.nodeType === 1 && n.className.split(" ").indexOf(cls) >= 0) return n;
  for (var i = 0; i < n.childNodes.length; i++) { var f = find(n.childNodes[i], cls); if (f) return f; } return null; }
function probe(line) { var r = Ed.row(null, line, 0), pre = find(r, "lpre"), body = find(r, "lbody");
  return JSON.stringify({ cls: r.className, text: r.textContent, row: shape(r), pre: pre ? shape(pre) : null,
    // the CSS condition, structurally: lpre is the row's FIRST element child and lbody its NEXT sibling
    flex: !!(pre && body && pre.parentNode.children[0] === pre && pre.nextSibling === body && body.parentNode === pre.parentNode),
    // lpre directly under the row (not inside a quote's span.bq)
    top: !!(pre && pre.parentNode === r),
    tok: (r._tok || []).map(function (t) { return t.s + "-" + t.e + ":" + t.kind; }).join(",") }); }
"#;

#[derive(serde::Deserialize, Debug)]
struct Probe { cls: String, text: String, row: String, pre: Option<String>, flex: bool, top: bool, tok: String }

fn ctx() -> javascriptcore::Context {
    let ed = std::fs::read_to_string(root().join("ui/editor.js")).unwrap();
    let ctx = javascriptcore::Context::new();
    ctx.evaluate(DOM);
    ctx.evaluate(&ed);
    if let Some(e) = ctx.exception() { panic!("editor.js did not load: {}", e.to_str()); }
    ctx.evaluate(SHAPE);
    assert_eq!(ctx.evaluate("typeof Ed.row").unwrap().to_str().as_str(), "function", "Ed.row missing from ui/editor.js");
    ctx
}

fn probe(ctx: &javascriptcore::Context, line: &str) -> Probe {
    let js = format!("probe({})", serde_json::to_string(line).unwrap());
    let out = ctx.evaluate(&js).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("{line:?}: JS threw {}", e.to_str()); }
    let p: Probe = serde_json::from_str(&out).unwrap_or_else(|e| panic!("{line:?}: bad probe {out:?}: {e}"));
    // the split is a pure regrouping: the row still reads as its source line (caret mapping, census)
    assert_eq!(p.text, line, "{line:?}: row text changed");
    p
}

#[test]
fn bullet_numbered_task_rows_split_into_marker_and_body() {
    let c = ctx();
    // (line, prefix box contents, body text, token map)
    let rows: &[(&str, &str, &str, &str)] = &[
        ("- alpha beta gamma", "span.mk.lim[-] | span.mk.lisp[ ]", "alpha beta gamma", "0-2:li"),
        ("* star item", "span.mk.lim[*] | span.mk.lisp[ ]", "star item", "0-2:li"),
        ("+ plus item", "span.mk.lim[+] | span.mk.lisp[ ]", "plus item", "0-2:li"),
        ("1. first", "span.mk.lim[1.] | span.mk.lisp[ ]", "first", "0-3:li"),
        ("10. tenth", "span.mk.lim[10.] | span.mk.lisp[ ]", "tenth", "0-4:li"),
        // nested levels: the literal indent is IN the prefix box, so a nested item hangs under its own text
        ("\t- level two", "#text[\t] | span.mk.lim[-] | span.mk.lisp[ ]", "level two", "1-3:li"),
        ("\t\t- level three", "#text[\t\t] | span.mk.lim[-] | span.mk.lisp[ ]", "level three", "2-4:li"),
        ("    - four spaces", "#text[    ] | span.mk.lim[-] | span.mk.lisp[ ]", "four spaces", "4-6:li"),
        ("\t1. nested numbered", "#text[\t] | span.mk.lim[1.] | span.mk.lisp[ ]", "nested numbered", "1-4:li"),
        // tasks: checkbox + "[ ] " sit in the prefix box; the struck span IS the body box
        ("- [ ] open task", "span.mk.lim[-] | span.mk.lisp[ ] | input.tbox[] | span.mk.task[[ ] ]", "open task", "0-6:task"),
        ("- [x] done task", "span.mk.lim[-] | span.mk.lisp[ ] | input.tbox[] | span.mk.task[[x] ]", "done task", "0-6:task"),
        ("\t- [/] nested custom status", "#text[\t] | span.mk.lim[-] | span.mk.lisp[ ] | input.tbox[] | span.mk.task[[/] ]", "nested custom status", "1-7:task"),
        ("1. [ ] numbered task", "span.mk.lim[1.] | span.mk.lisp[ ] | input.tbox[] | span.mk.task[[ ] ]", "numbered task", "0-7:task"),
    ];
    for (line, pre, body, tok) in rows {
        let p = probe(&c, line);
        assert!(p.cls.split(' ').any(|k| k == "li"), "{line:?}: row lost class li: {:?}", p.cls);
        assert_eq!(p.pre.as_deref(), Some(*pre), "{line:?}: prefix box");
        assert!(p.flex && p.top, "{line:?}: row must OPEN with lpre followed by lbody: {}", p.row);
        let want_body = if line.contains("] ") { format!("span.tkt.lbody[{body}]") } else { format!("span.lbody[{body}]") };
        assert!(p.row.ends_with(&want_body), "{line:?}: body box {want_body:?} not last in {}", p.row);
        assert_eq!(p.row.matches(" | ").count(), 1, "{line:?}: row must be exactly [lpre][lbody]: {}", p.row);
        assert!(p.tok.starts_with(tok), "{line:?}: token map {:?} must start {tok:?}", p.tok);
    }
    // inline markup stays inside the body box (one flex item), tokens keep their raw columns
    let p = probe(&c, "- some **bold** and `code` text");
    assert!(p.flex, "markup row not split: {}", p.row);
    assert!(p.tok.contains("7-15:") && p.tok.contains("20-26:code"), "inline tokens moved: {}", p.tok);
    // done bullet task keeps the strike class on the row and the strike span on the text only
    let p = probe(&c, "- [x] done");
    assert!(p.cls.split(' ').any(|k| k == "tkx"), "{:?}", p.cls);
    assert!(!p.pre.unwrap().contains("tkt"), "the strike must not reach the prefix (gh #45)");
}

#[test]
fn rows_that_must_keep_the_inline_flow() {
    let c = ctx();
    // no body -> no lbody -> the CSS flex rule does not match (blank items cannot wrap;
    // gh #75's blank-task caret anchor span.tcar stays a plain inline sibling)
    for line in ["- ", "1. ", "\t- "] {
        let p = probe(&c, line);
        assert!(!p.flex && !p.row.contains("lbody"), "{line:?}: {}", p.row);
    }
    let p = probe(&c, "- [ ] ");
    assert!(!p.flex && p.row.ends_with("span.tcar[]"), "blank task: {}", p.row);
    // a list inside a quote: lpre exists but is NOT the first element of its box (">" marker
    // precedes it), so the quote keeps its exact layout
    for line in ["> - quoted item", "> > - deep quoted", "> 1. quoted numbered"] {
        let p = probe(&c, line);
        assert!(!p.top && !p.flex, "{line:?}: quote list must not match the flex rule: {}", p.row);
    }
    // not list items at all
    for line in ["plain paragraph", "-not a list", "1.not a list", "# heading", "---", "| a | b |", "```js", "#tag - x", ""] {
        let p = probe(&c, line);
        assert!(p.pre.is_none() && !p.row.contains("lbody"), "{line:?}: {}", p.row);
    }
}

#[test]
fn style_rules_key_on_the_split_and_spare_source_mode() {
    let css = std::fs::read_to_string(root().join("ui/style.css")).unwrap();
    for rule in [
        ".lp:not(.src) .li:has(> .lpre:first-child + .lbody) { display: flex; align-items: baseline }",
        ".lp:not(.src) .li > .lpre:first-child:has(+ .lbody) { flex: none; white-space: pre }",
        ".lp:not(.src) .li > .lpre:first-child + .lbody { flex: 1 1 0; min-width: 0 }",
    ] {
        assert!(css.contains(rule), "ui/style.css lost the #272 rule {rule:?}");
    }
    // the marker-box rules (R15.10) must follow the marker into lpre, or the bullet vanishes
    assert!(!css.contains(".li > .mk.lim") && !css.contains(".tk > .mk.lim") && !css.contains("ord) > .mk.lim"),
            "a marker rule still targets .mk.lim as a direct row child");
    assert!(css.matches("> .lpre > .mk.lim").count() >= 10, "marker-box rules not retargeted through .lpre");
    // never in source mode (raw lines) and never on a non-list row
    for l in css.lines().filter(|l| l.contains(".lbody") && l.contains('{')) {
        assert!(l.starts_with(".lp:not(.src) .li"), "#272 rule escapes LP list rows: {l}");
    }
}
