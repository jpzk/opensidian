// SPDX-License-Identifier: GPL-3.0-or-later
//! Theme parity bundle th-chrome (milestone 097): app chrome variable wiring.
//! One test group per issue (#322 #326 #327 #328 #329 #337 #341 #344 #345 #346 #349
//! #350 #351 #352), each with a NEGATIVE that pins what must NOT change. Source tests
//! read ui/style.css / ui/main.js; the pure helpers run in JavaScriptCore (the engine
//! the webview runs). The computed values per theme x mode are the harness phase
//! themethchrome's job (stock 1.13.7 values, getComputedStyle).
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn ui(f: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui").join(f);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}
fn css() -> String { ui("style.css") }

/// The declaration block of the FIRST rule whose selector text (at a line start) is exactly `sel`.
fn rule(css: &str, sel: &str) -> String {
    let head = format!("\n{sel} {{");
    let at = css.find(&head).unwrap_or_else(|| panic!("rule `{sel}` not in ui/style.css")) + head.len();
    let end = css[at..].find('}').unwrap_or_else(|| panic!("rule `{sel}` unterminated")) + at;
    css[at..end].to_string()
}
#[allow(dead_code)]
fn has_rule(css: &str, sel: &str) -> bool { css.contains(&format!("\n{sel} {{")) }
/// The value of custom property `name` in the FIRST block that declares it after `scope` (a line-start marker).
#[allow(dead_code)]
fn decl_in(css: &str, scope: &str, name: &str) -> String {
    let s = css.find(scope).unwrap_or_else(|| panic!("scope `{scope}` not in ui/style.css"));
    let key = format!("\n  {name}:");
    let at = css[s..].find(&key).unwrap_or_else(|| panic!("{name} not declared after `{scope}`")) + s + key.len();
    let end = css[at..].find(';').unwrap() + at;
    css[at..end].trim().to_string()
}

/// Source of top-level `function NAME(...) { ... }`: brace-matched, skipping strings, // and /* */ comments.
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
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            // block comment (prose with apostrophes: "stock's")
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') { i += 1; }
            i += 2;
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

#[allow(dead_code)]
fn ev(ctx: &javascriptcore::Context, js: &str) -> String {
    let v = ctx.evaluate(js).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("{js}: threw {}", e.to_str()); }
    v
}
#[allow(dead_code)]
fn jsc(funcs: &[&str], prelude: &str) -> javascriptcore::Context {
    let src = ui("main.js");
    let ctx = javascriptcore::Context::new();
    ctx.evaluate(prelude);
    if let Some(e) = ctx.exception() { panic!("prelude: {}", e.to_str()); }
    for f in funcs {
        ctx.evaluate(&func(&src, f));
        if let Some(e) = ctx.exception() { panic!("{f} did not load: {}", e.to_str()); }
    }
    ctx
}

// ---------------------------------------------------------------- #345
#[test]
fn i345_explorer_delete_is_a_warning_row() {
    let nm = func(&ui("main.js"), "noteMenu");
    assert!(nm.contains("const mkItem = (label, fn, warn) =>") && nm.contains("if (warn) d.className = \"is-warning\";"),
        "#345: noteMenu's mkItem cannot mark a row .is-warning");
    assert!(nm.contains("mkItem(\"Delete\", () => askDelete(nm), true)"), "#345: the explorer Delete row is not .is-warning");
    let c = css();
    let r = rule(&c, ".ctxmenu div.is-warning");
    assert!(r.contains("color: var(--text-error"), "#345: .ctxmenu .is-warning is not --text-error ink: {r}");
}
#[test]
fn i345_negative_only_delete_warns_and_plain_rows_keep_their_ink() {
    let nm = func(&ui("main.js"), "noteMenu");
    // exactly ONE warn row in the note menu: Bookmark / Rename... stay plain
    assert_eq!(nm.matches("), true);").count(), 1, "#345: a non-destructive explorer row was marked .is-warning");
    let base = rule(&css(), ".ctxmenu div");
    assert!(!base.contains("--text-error"), "#345: the plain menu row must not be warning ink: {base}");
}

// ---------------------------------------------------------------- #346
const DOC_STUB: &str = "var document = { createTextNode: function(t){ return { t: t }; },\
    createElement: function(){ return { className: '', textContent: '' }; } };\
    function El(){ this.k = []; } El.prototype.appendChild = function(n){ this.k.push(n); return n; };\
    function dump(e){ return e.k.map(function(n){ return n.className === 'suggestion-highlight' ? '[' + n.textContent + ']' : n.t; }).join(''); }\
    function lab(q, s){ var e = new El(); fuzzyLabel(e, q, s); return dump(e); }";

#[test]
fn i346_matched_characters_are_suggestion_highlight_spans() {
    let c = jsc(&["fuzzy", "fuzzySpans", "fuzzyLabel"], DOC_STUB);
    assert_eq!(ev(&c, "lab('toggle', 'Toggle ribbon')"), "[Toggle] ribbon");          // substring: one run
    assert_eq!(ev(&c, "lab('rib', 'Toggle ribbon')"), "Toggle [rib]bon");
    assert_eq!(ev(&c, "lab('tgr', 'Toggle ribbon')"), "[T]o[g]gle [r]ibbon");         // subsequence: per char
    assert_eq!(ev(&c, "lab('tog', 'Outgoing links')"), "Ou[t]g[o]in[g] links");       // greedy subsequence, like fuzzy()
    assert_eq!(ev(&c, "lab('ou', 'Outgoing links')"), "[Ou]tgoing links");
    // the run set agrees with fuzzy(): a match <=> at least one run
    assert_eq!(ev(&c, "fuzzy('tgr','Toggle ribbon') >= 0 && fuzzySpans('tgr','Toggle ribbon').length > 0"), "true");
    let r = rule(&css(), ".suggestion-highlight");
    assert!(r.contains("font-weight: 700"), "#346: .suggestion-highlight is not bold: {r}");
    let rm = func(&ui("main.js"), "renderModal");
    assert!(rm.contains("fuzzyLabel(l, $(\"minput\").value.trim(), it.label)"), "#346: renderModal rows are not highlighted");
}
#[test]
fn i346_negative_no_match_or_empty_query_marks_nothing_and_text_is_unchanged() {
    let c = jsc(&["fuzzy", "fuzzySpans", "fuzzyLabel"], DOC_STUB);
    assert_eq!(ev(&c, "lab('', 'Toggle ribbon')"), "Toggle ribbon");
    assert_eq!(ev(&c, "lab('zzz', 'Toggle ribbon')"), "Toggle ribbon");
    assert_eq!(ev(&c, "JSON.stringify(fuzzySpans('zzz', 'Toggle ribbon'))"), "[]");
    assert_eq!(ev(&c, "fuzzy('zzz', 'Toggle ribbon')"), "-1");
    // the label text survives intact (the census reads textContent)
    assert_eq!(ev(&c, "lab('tgr', 'Toggle ribbon').replace(/[\\[\\]]/g, '')"), "Toggle ribbon");
    // the highlight is a single-class rule: a theme's .suggestion-highlight can override it
    assert!(!css().contains("#mlist .mrow .suggestion-highlight"), "#346: highlight rule must stay single-class");
}
