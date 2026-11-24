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

// ---------------------------------------------------------------- #329
#[test]
fn i329_tag_rows_read_nav_item_color() {
    let c = css();
    let r = rule(&c, ".tagrow");
    assert!(r.contains("color: var(--nav-item-color)"), "#329: .tagrow ink is not --nav-item-color: {r}");
    assert!(!r.contains("--text-chrome)"), "#329: .tagrow still paints body ink: {r}");
    // the default is unchanged: --nav-item-color = --text-muted (stock default tag ink = muted)
    assert_eq!(decl_in(&c, "\nbody {", "--nav-item-color"), "var(--text-muted)");
}
#[test]
fn i329_negative_count_and_tree_rows_keep_their_own_ink() {
    let c = css();
    assert!(!rule(&c, ".tagrow .scount").contains("--nav-item-color"), "#329: the tag COUNT is faint ink (#352), not nav ink");
    assert!(rule(&c, ".trow").contains("color: var(--nav-item-color)"), "#329: the file tree must keep --nav-item-color");
    assert!(!rule(&c, ".tagrow:hover").contains("--nav-item-color"), "#329: hover changes the fill only");
}

// ---------------------------------------------------------------- #349
#[test]
fn i349_hovered_tree_row_defaults_to_body_ink() {
    let c = css();
    assert_eq!(decl_in(&c, "\nbody {", "--nav-item-color-hover"), "var(--text-normal)", "#349: no stock default for --nav-item-color-hover");
    assert!(rule(&c, ".trow:hover").contains("color: var(--nav-item-color-hover"), "#349: .trow:hover does not read --nav-item-color-hover");
}
#[test]
fn i349_negative_resting_row_stays_muted_and_light_mode_does_not_override() {
    let c = css();
    let r = rule(&c, ".trow");
    assert!(r.contains("color: var(--nav-item-color)") && !r.contains("--nav-item-color-hover"), "#349: the RESTING row must keep the muted nav ink: {r}");
    let light = &c[c.find("\nbody:where(.theme-light) {").unwrap()..];
    let light = &light[..light.find("\n}").unwrap()];
    assert!(!light.contains("--nav-item-color-hover:"), "#349: light mode must inherit the same hover default");
}

// ---------------------------------------------------------------- #351
#[test]
fn i351_outline_current_heading_reads_the_active_nav_item_colours() {
    let r = rule(&css(), ".tocrow.active");
    assert!(r.contains("color: var(--nav-item-color-active)"), "#351: current heading ink is not --nav-item-color-active: {r}");
    assert!(r.contains("background: var(--nav-item-background-active)"), "#351: current heading fill is not --nav-item-background-active: {r}");
    assert!(!r.contains("--bg-surface") && !r.contains("--text-chrome"), "#351: the old literal surface/ink is still read: {r}");
}
#[test]
fn i351_negative_other_outline_rows_are_unchanged() {
    let c = css();
    let r = rule(&c, ".tocrow");
    assert!(r.contains("color: var(--text-chrome-muted)") && !r.contains("--nav-item-color-active"), "#351: resting outline rows changed: {r}");
    assert!(!rule(&c, ".tocrow:hover").contains("--nav-item-background-active"), "#351: hover is not the active fill");
}

// ---------------------------------------------------------------- #322
#[test]
fn i322_active_tab_is_outlined_in_tab_outline_color() {
    let c = css();
    let r = rule(&c, ".tab.active");
    assert!(r.contains("box-shadow: 0 0 0 var(--tab-outline-width, 1px) var(--tab-outline-color)"), "#322: the active tab does not draw --tab-outline-color: {r}");
    assert_eq!(decl_in(&c, "\nbody {", "--tab-outline-color"), "var(--background-modifier-border)", "#322: no stock default for --tab-outline-color");
}
#[test]
fn i322_negative_inactive_tabs_and_the_tab_strip_stay_unoutlined() {
    let c = css();
    assert!(!rule(&c, ".tab").contains("box-shadow"), "#322: inactive tabs must not be outlined");
    let bar = rule(&c, ".tabbar");
    assert!(bar.contains("border-bottom-width: var(--tab-outline-width, 0)"), "#322: the strip's bottom border must stay 0 by default: {bar}");
    let light = &c[c.find("\nbody:where(.theme-light) {").unwrap()..];
    let light = &light[..light.find("\n}").unwrap()];
    assert!(!light.contains("--tab-outline-color:"), "#322: light mode must inherit the same wiring (the border colour differs, not the var)");
}

// ---------------------------------------------------------------- #326
#[test]
fn i326_palette_and_switcher_border_reads_prompt_border_color() {
    let c = css();
    let r = rule(&c, "#mbox");
    assert!(r.contains("border: 1px solid var(--prompt-border-color)"), "#326: #mbox border does not read --prompt-border-color: {r}");
    assert!(!r.contains("var(--border)"), "#326: #mbox still draws the generic --border: {r}");
    assert_eq!(decl_in(&c, "\nbody {", "--prompt-border-color"), "var(--background-modifier-border-focus)", "#326: no stock default for --prompt-border-color");
}
#[test]
fn i326_negative_geometry_and_other_modals_are_unchanged() {
    let c = css();
    let r = rule(&c, "#mbox");
    assert!(r.contains("border-radius: 8px") && r.contains("width: 520px"), "#326: prompt geometry is out of scope: {r}");
    let light = &c[c.find("\nbody:where(.theme-light) {").unwrap()..];
    let light = &light[..light.find("\n}").unwrap()];
    assert!(!light.contains("--prompt-border-color:") && light.contains("--background-modifier-border-focus: #bdbdbd"), "#326: light must inherit the wiring via its own focus border");
}

// ---------------------------------------------------------------- #327
#[test]
fn i327_menus_and_graph_controls_chain_to_secondary_and_border_hover() {
    let c = css();
    assert_eq!(decl_in(&c, "\nbody {", "--menu-background"), "var(--background-secondary)", "#327: menus do not sit on --background-secondary");
    assert_eq!(decl_in(&c, "\nbody {", "--menu-border-color"), "var(--background-modifier-border-hover, var(--border))", "#327: menu frame does not read --background-modifier-border-hover");
    let m = rule(&c, ".ctxmenu");
    assert!(m.contains("var(--menu-background") && m.contains("var(--menu-border-color"), "#327: .ctxmenu lost the --menu-* wiring: {m}");
    let g = rule(&c, ".graph-controls");
    assert!(g.contains("background: var(--background-secondary)"), "#327: graph controls bg is not --background-secondary: {g}");
    assert!(g.contains("border: 1px solid var(--background-modifier-border-hover, var(--border))"), "#327: graph controls border is not --background-modifier-border-hover: {g}");
}
#[test]
fn i327_negative_no_elevated_surface_and_geometry_unchanged() {
    let c = css();
    let g = rule(&c, ".graph-controls");
    assert!(!g.contains("var(--bg-elevated)"), "#327: graph controls still on the primary/elevated surface: {g}");
    assert!(g.contains("border-radius: 8px") && g.contains("width: var(--graph-controls-width, 240px)"), "#327: graph controls geometry is out of scope: {g}");
    assert_eq!(decl_in(&c, "\nbody {", "--menu-radius"), "6px", "#327: menu radius is not this issue (AnuPpuccin 8 vs 6 left out)");
    // light/dark differ by --background-secondary only: no mode block redefines the menu wiring
    let light = &c[c.find("\nbody:where(.theme-light) {").unwrap()..];
    let light = &light[..light.find("\n}").unwrap()];
    assert!(!light.contains("--menu-background:") && light.contains("--background-secondary: #f6f6f6"), "#327: light must inherit the wiring via its own secondary (stock 246)");
}
