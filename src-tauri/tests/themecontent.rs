// SPDX-License-Identifier: GPL-3.0-or-later
//! th-content (milestone 097): content variable wiring in ui/style.css follows the
//! stock Obsidian 1.13.7 defaults, so a community theme that only sets the standard
//! variable reaches the same element it reaches in stock. One test per issue, each
//! with a negative (the old wiring is gone / an unrelated surface did not move).
//! Stock evidence = computed styles of the running stock app (issue comments,
//! /workspace/goal/th-content/stock.md); the live check is harness phase theme-th-content.

use std::path::PathBuf;

fn style_css() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/style.css");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Every innermost `prelude { body }` block of a stylesheet, comments stripped,
/// prelude whitespace collapsed. Strings are honoured so a `{` inside url("...")
/// or content:"..." does not open a block.
fn rules(css: &str) -> Vec<(String, String)> {
    let mut s = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        s.push_str(&rest[..i]);
        rest = match rest[i + 2..].find("*/") { Some(j) => &rest[i + 2 + j + 2..], None => "" };
    }
    s.push_str(rest);
    let (mut out, mut stack, mut cur, mut q, mut leaf) = (Vec::new(), Vec::<String>::new(), String::new(), '\0', true);
    for c in s.chars() {
        if q != '\0' { cur.push(c); if c == q { q = '\0'; } continue; }
        match c {
            '"' | '\'' => { q = c; cur.push(c); }
            '{' => { stack.push(std::mem::take(&mut cur)); leaf = true; }
            '}' => {
                let pre = stack.pop().unwrap_or_default();
                if leaf { out.push((pre.split_whitespace().collect::<Vec<_>>().join(" "), std::mem::take(&mut cur))); }
                cur.clear(); leaf = false;
            }
            ';' if stack.is_empty() => cur.clear(),   // a top-level @import / @charset statement
            _ => cur.push(c),
        }
    }
    out
}

/// Declarations of one block: (property, value) with whitespace collapsed.
fn decls(body: &str) -> Vec<(String, String)> {
    let (mut out, mut cur, mut depth, mut q) = (Vec::new(), String::new(), 0i32, '\0');
    let push = |d: &str, out: &mut Vec<(String, String)>| {
        if let Some(i) = d.find(':') {
            let (p, v) = (d[..i].trim(), d[i + 1..].trim());
            if !p.is_empty() { out.push((p.to_string(), v.split_whitespace().collect::<Vec<_>>().join(" "))); }
        }
    };
    for c in body.chars() {
        if q != '\0' { cur.push(c); if c == q { q = '\0'; } continue; }
        match c {
            '"' | '\'' => { q = c; cur.push(c); }
            '(' => { depth += 1; cur.push(c); }
            ')' => { depth -= 1; cur.push(c); }
            ';' if depth == 0 => { push(&cur, &mut out); cur.clear(); }
            _ => cur.push(c),
        }
    }
    push(&cur, &mut out);
    out
}

/// All values `prop` gets in blocks whose prelude is exactly `sel`.
fn vals(css: &str, sel: &str, prop: &str) -> Vec<String> {
    rules(css).into_iter().filter(|(p, _)| p == sel)
        .flat_map(|(_, b)| decls(&b)).filter(|(p, _)| p == prop).map(|(_, v)| v).collect()
}

/// The single value of `prop` in `sel` (panics when absent or ambiguous).
fn val(css: &str, sel: &str, prop: &str) -> String {
    let v = vals(css, sel, prop);
    assert_eq!(v.len(), 1, "ui/style.css: expected exactly one `{prop}` in `{sel} {{}}`, found {v:?}");
    v[0].clone()
}

/// Every (selector, value) that declares `prop` anywhere.
fn all(css: &str, prop: &str) -> Vec<(String, String)> {
    rules(css).into_iter()
        .flat_map(|(p, b)| decls(&b).into_iter().filter(|(k, _)| k == prop).map(move |(_, v)| (p.clone(), v)))
        .collect()
}

const DARK: &str = "body";
const LIGHT: &str = "body:where(.theme-light)";

#[test]
fn helper_parses_the_token_blocks() {
    let css = style_css();
    // sanity: the two token blocks exist and carry the alt background stock paints code on
    assert_eq!(val(&css, DARK, "--background-primary-alt"), "var(--color-base-10, #232323)");
    assert_eq!(val(&css, LIGHT, "--background-primary-alt"), "var(--color-base-10, #fafafa)");
    let r = rules("a{x:1} @media (x){b{y:url(\"{\")}} /* c{z:2} */");
    assert_eq!(r, vec![("a".to_string(), "x:1".to_string()), ("b".to_string(), "y:url(\"{\")".to_string())]);
}

/// #320: stock's --code-background defaults to --background-primary-alt (default theme
/// stock inline code 250,250,250 light / 35,35,35 dark = our #fafafa / #232323), so a
/// theme that sets only the alt background (Minimal --bg2) paints inline code with it.
#[test]
fn i320_code_background_chains_to_background_primary_alt() {
    let css = style_css();
    let v = all(&css, "--code-background");
    assert_eq!(v, vec![(DARK.to_string(), "var(--background-primary-alt)".to_string())],
        "#320: --code-background must be declared once, on body, as var(--background-primary-alt)");
    // negative: the old wiring (our surface colour) is gone, and the surface token itself did not move
    assert!(!v.iter().any(|(_, x)| x.contains("--bg-surface")), "#320: --code-background still chains to --bg-surface");
    assert_eq!(val(&css, DARK, "--bg-surface"), "var(--background-secondary-alt)");
    // inline code (LP + reading) still paints --code-background
    assert_eq!(val(&css, ".lp code, .preview code", "background-color"), "var(--code-background)");
    assert_eq!(val(&css, ".lp .code", "background"), "var(--code-background)");
}

/// #325: stock bold = 600 on every theme that leaves the weight alone (default, Minimal,
/// AnuPpuccin, Gruvbox; strong, callout title, both modes): --font-weight 400 + modifier 200.
#[test]
fn i325_bold_modifier_gives_600() {
    let css = style_css();
    assert_eq!(all(&css, "--bold-modifier"), vec![(":root".to_string(), "200".to_string())],
        "#325: --bold-modifier must be declared once, on :root, as 200 (400 + 200 = stock 600)");
    assert_eq!(val(&css, ":root", "--font-weight"), "var(--font-normal, 400)");
    // the two consumers still derive from it (so a theme's --bold-modifier/--bold-weight reaches them)
    assert_eq!(val(&css, ":root", "--bold-weight"), "min(900, calc(var(--font-weight) + var(--bold-modifier)))");
    assert!(all(&css, "--callout-title-weight").iter().any(|(_, v)| v == "calc(var(--font-weight) + var(--bold-modifier))"));
    // negative: the old 250 (-> 650) is gone and no rule hard-codes a 650 weight
    assert!(!all(&css, "--bold-modifier").iter().any(|(_, v)| v == "250"), "#325: old --bold-modifier 250 still present");
    assert!(!all(&css, "font-weight").iter().any(|(_, v)| v == "650"), "#325: a rule hard-codes font-weight 650");
    assert_eq!(val(&css, ".lp b, .lp strong, .preview b, .preview strong", "font-weight"), "var(--bold-weight)");
}

/// #331: stock paints the live-preview/source fenced-code rows with --code-background
/// (AnuPpuccin painted 220,224,232 L / 17,17,27 D; Solarized rgb(238,232,213) / rgb(7,54,66)),
/// the same variable reading-view `pre` already uses.
#[test]
fn i331_lp_code_rows_use_code_background() {
    let css = style_css();
    assert_eq!(val(&css, ".lp .fence", "background"), "var(--code-background)");
    // negative: the editor rows no longer paint our surface colour, and nothing else on .fence does
    let fence: Vec<_> = rules(&css).into_iter().filter(|(p, _)| p.contains(".fence")).collect();
    assert_eq!(fence.len(), 1, "#331: expected one .fence rule, got {fence:?}");
    assert!(!fence[0].1.contains("--bg-surface"), "#331: .lp .fence still paints --bg-surface");
    assert_eq!(val(&css, ".lp pre, .preview pre", "background-color"), "var(--code-background)");
    // the fence BODY rows too (stock: the painted ancestor of span.cm-hmd-codeblock is the
    // HyperMD-codeblock line, default L 250,250,250 / D 35,35,35; ours painted only the ``` rows)
    assert_eq!(val(&css, ".lp .lprow.HyperMD-codeblock", "background-color"), "var(--code-background)");
    let js = editor_js();
    assert!(js.contains("Ed.lpFenceRows = function (doc)"), "#331: fence row scan gone");
    assert!(js.contains("(frs.has(ln.number) ? \" HyperMD-codeblock\" : \"\")"), "#331: LP rows no longer carry HyperMD-codeblock");
    // negative: an unclosed fence marks no rows (rows are added only when the closing fence is found)
    let scan = &js[js.find("Ed.lpFenceRows = function").unwrap()..];
    let scan = &scan[..scan.find("\n};").unwrap()];
    assert!(scan.contains("for (let k = open.l0; k <= i; k++) out.add(k)") && !scan.contains("out.add(i)"), "#331: rows added before the fence closes");
}

/// #332: stock resolved + external links follow --text-accent when a theme sets no
/// --link-color (Gruvbox: rgb(231,138,78) = its accent, L+D; ours painted the graph-node grey
/// through --accent-blue). Default theme unchanged: --accent-blue falls back to --text-accent.
#[test]
fn i332_link_colors_follow_text_accent() {
    let css = style_css();
    assert_eq!(all(&css, "--link-color"), vec![(DARK.to_string(), "var(--text-accent)".to_string())]);
    assert_eq!(all(&css, "--link-external-color"), vec![(DARK.to_string(), "var(--text-accent)".to_string())]);
    // negative: neither default chains to --accent-blue (= --graph-node first) any more
    for p in ["--link-color", "--link-external-color"] {
        assert!(!all(&css, p).iter().any(|(_, v)| v.contains("--accent-blue")), "#332: {p} still chains to --accent-blue");
    }
    // the default theme look is kept: --accent-blue itself still resolves to --text-accent without a graph-node
    assert_eq!(val(&css, DARK, "--accent-blue"), "var(--graph-node, var(--text-accent))");
    // consumers read the variables
    assert!(val(&css, ".lp a.wiki", "color").starts_with("var(--link-color"));
    assert!(val(&css, ".lp a.ext, .preview a.ext", "color").starts_with("var(--link-external-color"));
}

/// #321: stock unresolved links take the link colour (default L rgb(138,92,245) / D
/// rgb(166,138,249) = accent; Minimal rgb(106,134,149); Gruvbox rgb(231,138,78)), are dimmed
/// to opacity .7 and upright, in every view. Themes that set --link-unresolved-color /
/// --link-unresolved-opacity (Solarized: red, `unset` -> 1) still win.
#[test]
fn i321_unresolved_links_derive_from_link_color() {
    let css = style_css();
    assert_eq!(all(&css, "--link-unresolved-color"), vec![(DARK.to_string(), "var(--link-color)".to_string())]);
    assert_eq!(all(&css, "--link-unresolved-opacity"), vec![(DARK.to_string(), "0.7".to_string())]);
    for sel in [".lp a.wiki-unresolved", ".preview a.wiki-unresolved"] {
        assert!(vals(&css, sel, "color").iter().all(|v| v.starts_with("var(--link-unresolved-color")), "#321: {sel} colour");
        let fs = vals(&css, sel, "font-style");
        assert!(!fs.is_empty() && fs.iter().all(|v| v == "normal"), "#321: {sel} font-style must be normal, got {fs:?}");
        assert!(vals(&css, sel, "opacity").iter().any(|v| v == "var(--link-unresolved-opacity, 1)"), "#321: {sel} opacity");
    }
    // negative: no unresolved-link rule is italic any more, and the colour no longer chains to our own purple
    let italic: Vec<_> = rules(&css).into_iter().filter(|(p, b)| p.contains("wiki-unresolved") && b.contains("italic")).collect();
    assert!(italic.is_empty(), "#321: italic unresolved rules remain: {italic:?}");
    assert!(!all(&css, "--link-unresolved-color").iter().any(|(_, v)| v.contains("var(--link-unresolved)")));
    // resolved links did not move
    assert!(val(&css, ".lp a.wiki", "color").starts_with("var(--link-color"));
}

/// #347: stock draws the blockquote bar 2px in --interactive-accent (default theme stock
/// BQ_BORDER_W=2, harness typo gate); ours defaulted to 3px --text-accent, so every theme whose
/// text accent differs from its interactive accent (Gruvbox: orange vs teal, accent-h 166) got
/// a thick wrong-hue bar. A theme that sets the variables (Minimal: 1px) still wins.
#[test]
fn i347_blockquote_bar_stock_defaults() {
    let css = style_css();
    assert_eq!(all(&css, "--blockquote-border-color"), vec![(DARK.to_string(), "var(--interactive-accent)".to_string())]);
    assert_eq!(all(&css, "--blockquote-border-thickness"), vec![(":root".to_string(), "2px".to_string())]);
    // negative: the text-accent / 3px defaults are gone
    assert!(!all(&css, "--blockquote-border-color").iter().any(|(_, v)| v.contains("--text-accent")), "#347: bar still --text-accent");
    assert!(!all(&css, "--blockquote-border-thickness").iter().any(|(_, v)| v.contains("0.75")), "#347: bar still 3px");
    // both consumers draw the bar from the variables
    let bar = "var(--blockquote-border-thickness) solid var(--blockquote-border-color)";
    assert_eq!(val(&css, ".lp blockquote, .preview blockquote", "border-inline-start"), bar);
    assert_eq!(val(&css, ".lp .bq", "border-inline-start"), bar);
    // LP text offset stays at stock +14 from the bar (2px bar + 12px padding; gate LP_BQ_TEXT_X=14)
    assert_eq!(val(&css, ".lp .bq", "padding-inline-start"), "12px");
    assert_eq!(val(&css, ".lp blockquote", "padding-inline-start"), "12px");
    // negative: reading-view padding unchanged
    assert_eq!(val(&css, ".lp blockquote, .preview blockquote", "padding"), "0 0 0 var(--size-4-6)");
}

/// #330: stock paints --blockquote-background-color (default transparent) behind blockquotes;
/// AnuPpuccin sets rgba(crust, .5) -> stock painted L 230,233,239 / D 24,24,37, ours showed the
/// editor background because no rule of ours consumed the variable.
#[test]
fn i330_blockquote_background_consumed() {
    let css = style_css();
    assert_eq!(all(&css, "--blockquote-background-color"), vec![(DARK.to_string(), "transparent".to_string())]);
    let bg = "var(--blockquote-background-color)";
    assert_eq!(val(&css, ".lp blockquote, .preview blockquote", "background-color"), bg);
    assert_eq!(val(&css, ".lp .bq", "background-color"), bg);
    // negative: the hard transparent background is gone from the blockquote rule
    assert!(vals(&css, ".lp blockquote, .preview blockquote", "background").is_empty(), "#330: blockquote still hard-codes background");
    // negative: a nested LP quote does not stack the translucent paint a second time
    assert_eq!(val(&css, ".lp .bq .bq", "background-color"), "transparent");
}

fn main_js() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/main.js");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// #323: stock paints a search / backlink match as `.search-result-file-matched-text` =
/// --text-highlight-bg behind --text-normal ink (default L 255,236,153 painted ink 34,34,34;
/// Minimal rgba(255,225,0,.5); AnuPpuccin rgba(yellow,.2)); themes style that class directly
/// (Solarized ink --text-on-accent, Gruvbox --text-highlight-fg). Ours hard-coded
/// `background: none; color: --accent-yellow` on `.shit mark` / `.blline mark`.
#[test]
fn i323_search_matches_use_text_highlight_bg() {
    let css = style_css();
    let sel = ".search-result-file-matched-text";
    assert_eq!(val(&css, sel, "background-color"), "var(--text-highlight-bg)");
    assert_eq!(val(&css, sel, "color"), "var(--text-normal)");
    // negative: the hard-coded rules are gone, and nothing more specific than the bare class
    // styles a match (a theme's own `.search-result-file-matched-text` rule must win by load order)
    for old in [".shit mark", ".blline mark"] {
        assert!(rules(&css).iter().all(|(p, _)| p != old), "#323: `{old}` still styles the match");
    }
    let over: Vec<_> = rules(&css).into_iter().map(|(p, _)| p)
        .filter(|p| p != sel && (p.contains("matched-text") || p.ends_with(" mark") && (p.contains("shit") || p.contains("blline")))).collect();
    assert!(over.is_empty(), "#323: rules outrank the stock match class: {over:?}");
    // every <mark> the search / backlink / unlinked-mention builders create carries the class
    let js = main_js();
    let marks = js.matches("createElement(\"mark\")").count();
    let classed = js.lines().filter(|l| l.contains("search-result-file-matched-text") && l.contains("className")).count();
    assert_eq!(marks, 5, "#323: mark builders in ui/main.js changed ({marks}); re-check each carries the class");
    assert_eq!(classed, marks, "#323: {classed}/{marks} search/backlink marks carry .search-result-file-matched-text");
}

fn editor_js() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/editor.js");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// #335: stock's live-preview ==highlight== is `span.cm-highlight` (--text-highlight-bg behind
/// --text-normal); themes restyle it by that class (Solarized `.cm-s-obsidian span.cm-highlight`
/// ink --text-on-accent L rgb(253,246,227) / D rgb(0,43,54) + padding 2px 0; Gruvbox `.cm-highlight`
/// --text-highlight-fg). Ours was `<mark class="hl">` with `.lp .hl` hard-coding the ink.
#[test]
fn i335_lp_highlight_is_span_cm_highlight() {
    let css = style_css();
    assert_eq!(val(&css, ".cm-highlight", "background-color"), "var(--text-highlight-bg)");
    assert_eq!(val(&css, ".cm-highlight", "color"), "var(--text-normal)");
    // negative: no rule more specific than the bare class sets the highlight ink or padding
    assert!(rules(&css).iter().all(|(p, _)| p != ".lp .hl"), "#335: `.lp .hl` still hard-codes the ink");
    let over: Vec<_> = rules(&css).into_iter().map(|(p, _)| p).filter(|p| p != ".cm-highlight" && p.contains("cm-highlight")).collect();
    assert!(over.is_empty(), "#335: rules outrank the stock highlight class: {over:?}");
    // the inline renderer emits a span carrying both our token class and the stock class
    let js = editor_js();
    assert!(js.contains("[\"==\", \"hl\", \"span\"]"), "#335: == must render as a span (stock span.cm-highlight)");
    assert!(!js.contains("[\"==\", \"hl\", \"mark\"]"), "#335: == still renders as <mark>");
    assert!(js.contains("if (cls === \"hl\") e.classList.add(\"cm-highlight\")"), "#335: highlight span lacks .cm-highlight");
    // negative: the other inline tokens did not move
    for t in ["[\"**\", \"b\", \"strong\"]", "[\"~~\", \"s\", \"span\"]", "[\"*\", \"i\", \"em\"]"] {
        assert!(js.contains(t), "#335: inline token table changed: {t}");
    }
}

/// #336: stock defines the base variables --interactive-accent-hsl (= --color-accent-hsl, the
/// accent h,s,l triple, both modes) and --font-bold (700). Themes build on them (Solarized
/// --tag-background hsla(var(--interactive-accent-hsl), .1/.2) -> stock rgba(203,75,22,.1 L/.2 D);
/// --table-header-weight var(--font-bold) -> stock 700); undefined, they went invalid at
/// computed-value time (tag background transparent, th weight our 600 fallback).
#[test]
fn i336_accent_hsl_and_font_bold_defined() {
    let css = style_css();
    assert_eq!(all(&css, "--color-accent-hsl"), vec![(DARK.to_string(), "var(--accent-h), var(--accent-s), var(--accent-l)".to_string())]);
    assert_eq!(all(&css, "--interactive-accent-hsl"), vec![(DARK.to_string(), "var(--color-accent-hsl)".to_string())]);
    assert_eq!(all(&css, "--font-bold"), vec![(DARK.to_string(), "700".to_string())]);
    // negative: the light block does not re-point the triple at a shifted accent step
    assert!(vals(&css, LIGHT, "--interactive-accent-hsl").is_empty() && vals(&css, LIGHT, "--color-accent-hsl").is_empty());
    // negative: the default look is unchanged (accent colour, tag tint and header weight keep their own rows)
    assert_eq!(val(&css, DARK, "--color-accent"), "hsl(var(--accent-h), var(--accent-s), var(--accent-l))");
    assert_eq!(val(&css, DARK, "--font-semibold"), "600");
    assert_eq!(val(&css, DARK, "--tag-background"), "hsla(var(--accent-h), var(--accent-s), var(--accent-l), .1)");
}

fn props_js() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/props.js");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// #334: stock paints the pills of a tags property with the tag variables (Gruvbox L color
/// rgb(76,122,93) bg rgb(235,219,178), D color rgb(137,180,130)); ours gave every pill
/// `background: var(--border)` and inherited ink.
#[test]
fn i334_tag_pills_use_tag_variables() {
    let css = style_css();
    let sel = ".metadata-property[data-property-type=\"tags\"] .multi-select-pill";
    assert_eq!(val(&css, sel, "color"), "var(--tag-color)");
    assert_eq!(val(&css, sel, "background-color"), "var(--tag-background)");
    // negative: the generic (aliases / list) pill is unchanged
    assert_eq!(val(&css, ".multi-select-pill", "background"), "var(--border)");
    assert!(vals(&css, ".multi-select-pill", "color").is_empty());
    // the DOM carries what the selector needs: the row's data-property-type and the pill class
    let js = props_js();
    assert!(js.contains("w.dataset.propertyType = row.type"), "#334: property row lost data-property-type");
    assert!(js.contains("el(\"div\", \"multi-select-pill\")"), "#334: pill class changed");
}

fn ui_file(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui").join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

const CARD: &str = ":is(.canvas-text, .canvas-file-embed)";

/// #339: canvas card markdown (text card .canvas-text, note card .canvas-file-embed) sits outside
/// .lp/.preview, so no link rule reached it and the UA default (rgb(0,0,238), underline) won.
/// Stock: card links = --link-color with no underline (Solarized rgb(38,139,210) td none, L=D).
#[test]
fn i339_canvas_card_links_use_link_styling() {
    let css = style_css();
    let w = format!("{CARD} a.wiki");
    assert_eq!(val(&css, &w, "color"), "var(--link-color, var(--accent-blue))");
    assert_eq!(val(&css, &w, "text-decoration"), "var(--link-decoration, none)");
    let u = format!("{CARD} a.wiki-unresolved");
    assert_eq!(val(&css, &u, "color"), "var(--link-unresolved-color, var(--link-unresolved))");
    assert_eq!(val(&css, &u, "opacity"), "var(--link-unresolved-opacity, 1)");
    assert_eq!(val(&css, &u, "font-style"), "normal");
    let e = format!("{CARD} a.ext");
    assert_eq!(val(&css, &e, "color"), "var(--link-external-color, var(--accent-blue))");
    // the default link decoration stays none (stock card links are not underlined)
    assert_eq!(val(&css, DARK, "--link-decoration"), "none");
    // the DOM carries the scope classes the selector needs
    assert!(ui_file("plugins/canvas-cards.js").contains("mk(\"div\", \"canvas-text markdown-rendered\")"), "#339: text card class changed");
    assert!(ui_file("plugins/canvas.js").contains("el(\"div\", \"canvas-file-embed\", this.contentEl)"), "#339: note card class changed");
    // negative: the reading-view / LP link rules did not move
    assert_eq!(val(&css, ".preview a.wiki", "color"), "var(--link-color, var(--accent-blue))");
    assert_eq!(val(&css, ".lp a.wiki", "text-decoration"), "var(--link-decoration, none)");
    // negative: nothing in the canvas plugin CSS hard-codes a card link colour over the variables
    for f in ["plugins/plugins.css", "plugins/canvas-cards.js", "plugins/canvas.js"] {
        let s = ui_file(f);
        assert!(!s.contains("canvas-text a") && !s.contains("canvas-file-embed a"), "#339: {f} styles card links itself");
    }
}

/// #353: the heading / bold / italic colour rules were scoped to .lp/.preview, so canvas card
/// markdown painted headings and bold in body ink (Gruvbox L rgb(101,71,53)) where stock uses
/// --h1-color (red) and --bold-color (orange) as in the note view.
#[test]
fn i353_canvas_card_headings_and_bold_use_theme_colours() {
    let css = style_css();
    for i in 1..=6 {
        assert_eq!(val(&css, &format!("{CARD} h{i}"), "color"), format!("var(--h{i}-color, var(--text-heading, currentColor))"), "#353: card h{i}");
        // the card heading colour chains exactly like the reading-view one
        assert!(vals(&css, &format!(".lp h{i}, .preview h{i}"), "color").contains(&format!("var(--h{i}-color, var(--text-heading, currentColor))")));
    }
    assert_eq!(val(&css, &format!("{CARD} :is(b, strong)"), "color"), "var(--bold-color, inherit)");
    assert_eq!(val(&css, &format!("{CARD} em"), "color"), "var(--italic-color, inherit)");
    // negative: the default theme declares no heading/bold colour, so card ink stays the body ink (currentColor / inherit)
    for v in ["--h1-color", "--bold-color", "--italic-color", "--text-heading"] {
        assert!(all(&css, v).is_empty(), "#353: style.css now sets {v}, the default card look would move");
    }
    // negative: colours only - card heading size/weight/spacing (#342, default-theme card typography) untouched here
    for i in 1..=6 {
        for p in ["font-size", "font-weight", "margin", "margin-block", "line-height"] {
            assert!(vals(&css, &format!("{CARD} h{i}"), p).is_empty(), "#353: card h{i} sets {p}");
        }
    }
}
