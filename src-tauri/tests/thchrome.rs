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
    assert_eq!(decl_in(&c, "\nbody {", "--prompt-border-color"), "var(--color-base-40, var(--background-modifier-border-focus))", "#326: --prompt-border-color must default to the base-40 ramp (Solarized stock = base-40, not its accent focus border)");
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
#[test]
fn i326_negative_prompt_border_is_not_the_focus_accent() {
    // Solarized sets --background-modifier-border-focus = its orange accent, yet stock frames the
    // prompt in base00/base0 (= its --color-base-40): the default must not read the focus var first.
    let c = css();
    let d = decl_in(&c, "\nbody {", "--prompt-border-color");
    assert!(d.starts_with("var(--color-base-40"), "#326: prompt border reads the focus var first: {d}");
    assert!(!c.contains("\n  --color-base-40:"), "#326: our default theme defines no base-40 ramp (the fallback carries the stock default)");
}

// ---------------------------------------------------------------- #337
#[test]
fn i337_focused_prompt_input_reads_border_focus_and_dropdowns_interactive_normal() {
    let c = css();
    let f = rule(&c, "#minput:focus");
    assert!(f.contains("border-color: var(--background-modifier-border-focus)"), "#337: focused prompt input ignores --background-modifier-border-focus: {f}");
    assert_eq!(decl_in(&c, "\nbody {", "--dropdown-background"), "var(--interactive-normal)", "#337: dropdowns do not read --interactive-normal");
    let d = rule(&c, ".sctl.dropdown");
    assert!(d.contains("background: var(--dropdown-background") && d.contains("border: 0;"), "#337: dropdown fill/border-width not stock: {d}");
    assert!(d.contains("box-shadow: var(--input-shadow, inset 0 0 0 1px var(--border))"), "#337: dropdown frame must come from --input-shadow: {d}");
}
#[test]
fn i337_negative_live_dropdown_and_separator_unchanged() {
    let c = css();
    let live = rule(&c, ".sctl.dropdown.live");
    assert!(!live.contains("background"), "#337: the Themes dropdown must not repaint a different fill than the others: {live}");
    let f = rule(&c, "#minput:focus");
    assert!(f.contains("border-bottom-color: var(--border)"), "#337: the palette separator keeps its own colour on focus: {f}");
    assert!(rule(&c, "#minput").contains("outline: none"), "#337: no extra outline ring on the prompt input");
    // focus rings on buttons are a separate affordance: --border-focus-ring is untouched here
    assert_eq!(decl_in(&c, "\nbody {", "--border-focus-ring"), "#bdbdbd", "#337: button focus rings are out of scope");
}

// ---------------------------------------------------------------- #352
#[test]
fn i352_faint_chrome_ink_derives_from_text_faint_and_prompts_have_placeholders() {
    let c = css();
    assert_eq!(decl_in(&c, "\nbody {", "--text-chrome-faint"), "var(--text-faint)", "#352: --text-chrome-faint is a literal, not the theme --text-faint");
    assert_eq!(decl_in(&c, "\nbody {", "--text-chrome-dim"), "var(--text-faint)", "#352: --text-chrome-dim is a literal, not the theme --text-faint");
    let p = rule(&c, "#minput::placeholder, #rninput::placeholder");
    assert!(p.contains("color: var(--input-placeholder-color, var(--text-faint))"), "#352: prompt placeholders fall back to WebKit grey: {p}");
    for sel in [".tagrow .scount", ".rhead .scount", ".blnote .scount"] {
        assert!(rule(&c, sel).contains("var(--text-chrome-dim)"), "#352: {sel} no longer reads the derived faint ink");
    }
}
#[test]
fn i352_negative_no_mode_block_reintroduces_a_faint_literal() {
    let c = css();
    let n_faint = c.matches("\n  --text-chrome-faint:").count();
    let n_dim = c.matches("\n  --text-chrome-dim:").count();
    assert_eq!((n_faint, n_dim), (1, 1), "#352: a mode block re-declares the faint chrome ink as a literal");
    let light = &c[c.find("\nbody:where(.theme-light) {").unwrap()..];
    let light = &light[..light.find("\n}").unwrap()];
    assert!(light.contains("--text-faint: var(--color-base-50, #ababab)"), "#352: light keeps the stock default faint #ababab via --text-faint");
    // muted ink is NOT faint: the two must stay separate variables
    assert_ne!(decl_in(&c, "\nbody {", "--text-chrome-muted"), "var(--text-faint)", "#352: muted chrome ink collapsed onto faint");
}

// ---------------------------------------------------------------- #350
#[test]
fn i350_settings_desc_muted_heading_weight_and_card_surface() {
    let c = css();
    let d = rule(&c, ".sdesc");
    assert!(d.contains("color: var(--text-chrome-muted)") && !d.contains("faint"), "#350: setting descriptions are not muted ink: {d}");
    let h = rule(&c, ".snavh");
    assert!(h.contains("font-weight: 600"), "#350: nav group heading weight is not 600: {h}");
    let k = rule(&c, ".scard");
    assert!(k.contains("background: var(--background-primary-alt)"), "#350: setting cards are not on --background-primary-alt: {k}");
}
#[test]
fn i350_negative_heading_stays_faint_and_card_frame_unchanged() {
    let c = css();
    assert!(rule(&c, ".snavh").contains("color: var(--text-chrome-faint)"), "#350: the group heading ink is faint in stock (only its weight was off)");
    let k = rule(&c, ".scard");
    assert!(k.contains("border: 1px solid var(--bg-surface)") && k.contains("border-radius: 6px") && !k.contains("var(--bg-base)"), "#350: card frame out of scope / still page bg: {k}");
    assert_eq!(decl_in(&c, "\nbody {", "--text-chrome-muted"), "var(--text-muted)", "#350: muted chrome ink must be the theme --text-muted");
}

// ---------------------------------------------------------------- #341
#[test]
fn i341_ribbon_backlink_titles_done_tasks_and_raw_frontmatter_read_stock_ink() {
    let c = css();
    assert!(rule(&c, "#ribbon").contains("color: var(--text-muted)"), "#341: the ribbon container is not --text-muted");
    assert!(rule(&c, ".blnote").contains("color: var(--nav-item-color-active)"), "#341: backlink file titles do not read --nav-item-color-active");
    assert_eq!(decl_in(&c, "\nbody {", "--checklist-done-color"), "var(--text-muted)", "#341: stock done-task ink is --text-muted");
    let s = rule(&c, ".lp.src .tkx, .lp.src .tkx .tkt");
    assert!(s.contains("text-decoration: none") && s.contains("color: inherit"), "#341: source mode still dims/strikes a done task: {s}");
    let f = rule(&c, ".lp .lprow.fmrow");
    assert!(f.contains("color: var(--text-muted)") && f.contains("font-size: 0.875em"), "#341: raw frontmatter rows: {f}");
    let p = ui("props.js");
    assert!(p.contains("Decoration.line({ class: \"fmrow\" })"), "#341: props.js does not class raw frontmatter lines");
    assert!(p.contains("return [field, Props.rawRows(v, bust)];"), "#341: the raw-row plugin is not installed with the properties field");
}
#[test]
fn i341_negative_body_ink_icons_live_preview_strike_and_widget_unchanged() {
    let c = css();
    // the editor and other chrome still read --text-chrome = body ink: the fix is per rule, not the variable
    assert_eq!(decl_in(&c, "\nbody {", "--text-chrome"), "var(--text-normal)", "#341: --text-chrome must stay body ink (the editor reads it)");
    assert!(rule(&c, ".outrow").contains("color: var(--text-chrome)"), "#341: outgoing-link rows have no stock evidence, keep them");
    assert!(rule(&c, ".tocrow").contains("color: var(--text-chrome-muted)"), "#341: outline rows were already muted");
    assert!(rule(&c, "#ribbon button").contains("var(--icon-color, var(--text-chrome-muted))"), "#341: ribbon icons keep --icon-color");
    assert!(rule(&c, ".lp .tkx .tkt").contains("text-decoration: var(--checklist-done-decoration)"), "#341: LP done tasks must stay struck");
    assert!(!c.contains("--checklist-done-color: var(--text-chrome-faint)"), "#341: faint done-task ink is back");
    let p = ui("props.js");
    // raw mode only: the LP widget path still bails out in source mode, the row plugin bails out outside it
    assert!(p.contains("if (!r) return { d: Decoration.none, end: -1, r };"), "#341: fmrow must not apply in Live Preview (the widget covers it)");
    assert!(p.contains("v.lp.classList.contains(\"src\")) || (tb && tb.mode === \"reading\") || Props.mode === \"source\") return Decoration.none;"), "#341: the Properties widget must still yield to raw mode");
}
