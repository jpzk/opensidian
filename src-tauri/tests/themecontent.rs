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
