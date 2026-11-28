// SPDX-License-Identifier: GPL-3.0-or-later
//! Theme parity bundle th-default (#340 #342 #343): the DEFAULT theme's tokens
//! must equal stock Obsidian 1.13.7's computed defaults (black-box devtools
//! measurements, goal theme-default), and they must stay TOKENS, so a community
//! theme that sets the same variable still wins. Source-level checks on
//! ui/style.css; the computed-style proof is harness phase theme-th-default.

use std::path::PathBuf;

fn css(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    let s = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    strip_comments(&s)
}

fn strip_comments(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        o.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") { Some(j) => rest = &rest[i + 2 + j + 2..], None => { rest = ""; } }
    }
    o.push_str(rest);
    o
}

/// Body of the FIRST top-level rule whose selector text is exactly `sel`.
fn rule(src: &str, sel: &str) -> String {
    let head = format!("\n{sel} {{");
    let i = src.find(&head).unwrap_or_else(|| panic!("rule `{sel}` not found in ui/style.css")) + head.len();
    let j = src[i..].find('}').expect("unterminated rule") + i;
    src[i..j].to_string()
}

/// Value of declaration `prop` inside a rule body (None if absent).
fn decl(body: &str, prop: &str) -> Option<String> {
    for d in body.split(';') {
        if let Some((p, v)) = d.split_once(':') {
            if p.trim() == prop { return Some(v.trim().to_string()); }
        }
    }
    None
}

/// Every value given to custom property `name` anywhere in the file.
fn token_values(src: &str, name: &str) -> Vec<String> {
    let pat = format!("{name}:");
    let mut out = vec![];
    let mut at = 0;
    while let Some(k) = src[at..].find(&pat) {
        let s = at + k;
        let ok_before = s == 0 || !matches!(src.as_bytes()[s - 1], b'-' | b'a'..=b'z' | b'0'..=b'9');
        let vs = s + pat.len();
        let ve = src[vs..].find(|c| c == ';' || c == '}').map(|e| vs + e).unwrap_or(src.len());
        if ok_before { out.push(src[vs..ve].trim().to_string()); }
        at = ve;
    }
    out
}

fn style() -> String { css("../ui/style.css") }

// ---------------- #342 heading scale + weights ----------------

#[test]
fn h342_heading_sizes_are_stock_scale() {
    let c = style();
    for (n, want) in [(1, "1.618em"), (2, "1.462em"), (3, "1.318em"), (4, "1.188em"), (5, "1.076em"), (6, "1em")] {
        let v = token_values(&c, &format!("--h{n}-size"));
        assert_eq!(v, vec![want.to_string()], "--h{n}-size must be declared once, = stock {want} (#342)");
    }
}

#[test]
fn h342_negative_old_inflated_scale_is_gone() {
    let c = style();
    for old in ["1.974em", "1.762em", "1.574em", "1.405em", "1.254em", "1.12em"] {
        assert!(!c.contains(old), "the pre-#342 heading scale value {old} is back in ui/style.css");
    }
}

#[test]
fn h342_live_preview_headings_use_per_level_weight_token() {
    let c = style();
    for n in 1..=6 {
        let b = rule(&c, &format!(".lp .h{n}"));
        assert_eq!(decl(&b, "font-size").as_deref(), Some(format!("var(--h{n}-size)").as_str()), ".lp .h{n} size token");
        assert_eq!(decl(&b, "font-weight").as_deref(), Some(format!("var(--h{n}-weight)").as_str()),
            ".lp .h{n} must take --h{n}-weight (stock LP h2 680 / h3 660), not the shared .lp .h 700 (#342)");
    }
}

#[test]
fn h342_negative_reading_view_and_tokens_keep_graded_weights() {
    let c = style();
    for n in 1..=6 {
        let b = rule(&c, &format!(".lp h{n}, .preview h{n}"));
        assert_eq!(decl(&b, "font-weight").as_deref(), Some(format!("var(--h{n}-weight)").as_str()));
    }
    let graded = token_values(&c, "--h2-weight");
    assert!(graded.iter().any(|v| v == "680"), "stock graded h2 weight 680 lost: {graded:?}");
    assert!(token_values(&c, "--h3-weight").iter().any(|v| v == "660"));
}

// ---------------- #343 folder weight, tag pill, inline code radius ----------------

#[test]
fn h343_folder_rows_take_nav_item_weight_not_500() {
    let c = style();
    let f = rule(&c, ".trow.folder");
    assert_eq!(decl(&f, "font-weight"), None, ".trow.folder must not pin a weight (stock folder rows 400 = --nav-item-weight) (#343)");
    let t = rule(&c, ".trow");
    assert_eq!(decl(&t, "font-weight").as_deref(), Some("var(--nav-item-weight, inherit)"), "rows keep the theme's --nav-item-weight");
}

#[test]
fn h343_tag_pill_and_inline_code_radius_are_stock() {
    let c = style();
    assert_eq!(token_values(&c, "--tag-radius"), vec!["2em".to_string()], "stock tag corner 28px on a 14px tag (#343)");
    assert_eq!(token_values(&c, "--tag-size"), vec![".875em".to_string()], "stock tag font 14px at 16px");
    assert_eq!(token_values(&c, "--code-radius"), vec!["var(--radius-s, 4px)".to_string()], "stock inline code corner 4px (#343)");
    assert!(token_values(&c, "--radius-s").iter().any(|v| v == "4px"));
}

#[test]
fn h343_negative_rules_read_the_tokens_so_themes_still_win() {
    let c = style();
    assert!(!c.contains("var(--tag-radius, 999px)"), "a 999px tag pill fallback is back (#343)");
    let lt = rule(&c, ".lp .tag");
    assert_eq!(decl(&lt, "border-radius").as_deref(), Some("var(--tag-radius, 2em)"));
    let lc = rule(&c, ".lp .code");
    assert_eq!(decl(&lc, "border-radius").as_deref(), Some("var(--code-radius)"));
}

fn plugins_css() -> String { css("../ui/plugins/plugins.css") }

/// Omnisearch's modal removes the `modal` class and keeps `prompt`. The window must still
/// be a box ABOVE `.modal-bg` (which closes the modal on click) and not see-through, so the
/// rule is on `.prompt` itself, not only on `.modal.prompt`.
#[test]
fn a_prompt_without_the_modal_class_is_still_the_window() {
    let p = plugins_css();
    let b = rule(&p, ".os-plugin-modal > .prompt");
    assert_eq!(decl(&b, "position").as_deref(), Some("relative"), "not positioned = under .modal-bg: every click closes it");
    assert_eq!(decl(&b, "background").as_deref(), Some("var(--bg-elevated)"), "no background = see-through");
    assert_eq!(decl(&b, "overflow").as_deref(), Some("auto"));
    // the backdrop is the positioned layer the box must sit on top of
    let bg = rule(&p, ".os-plugin-modal .modal-bg");
    assert_eq!(decl(&bg, "position").as_deref(), Some("absolute"));
}

#[test]
fn h343_coloured_canvas_card_tint_is_stock_7pct() {
    // stock 1.13.7 colour-4 text card, painted: light 238,250,243 = 7% #08b94e over #fff
    // (docs/thdefault/stock-default-light.vals cv.card.ppbg in the harness)
    let p = plugins_css();
    let b = rule(&p, ".canvas-node.is-themed > .canvas-node-container");
    assert_eq!(decl(&b, "background").as_deref(),
        Some("color-mix(in srgb, var(--canvas-color) 7%, var(--background-primary, #1e1e1e))"),
        "coloured canvas card tint must be the stock 7% mix onto --background-primary (#343)");
}

#[test]
fn h343_negative_canvas_tint_not_12pct_and_uncoloured_card_untinted() {
    let p = plugins_css();
    assert!(!p.contains("var(--canvas-color) 12%"), "the 12% canvas card tint (2x too saturated vs stock) is back (#343)");
    let b = rule(&p, ".canvas-node > .canvas-node-container");
    assert_eq!(decl(&b, "background").as_deref(), Some("var(--background-primary, #1e1e1e)"),
        "an uncoloured card stays on the plain surface");
}

// ---------------- #340 dark default palette: stock neutral greys, no Catppuccin ----------------

/// Catppuccin Mocha literals the dark default used to ship (issue #340 evidence).
const CTP: [&str; 9] = ["#313244", "#45475a", "#585b70", "#6c7086", "#7f849c", "#bac2de", "#cdd6f5", "#cdd6f4", "#31324499"];

#[test]
fn h340_dark_surface_border_and_ink_are_stock_greys() {
    let c = style();
    let d = rule(&c, "body");
    assert_eq!(decl(&d, "--background-secondary-alt").as_deref(), Some("#1c1c1c"), "stock dark tab header 28,28,28 (#340)");
    assert_eq!(decl(&d, "--background-modifier-border").as_deref(), Some("var(--color-base-30, #333333)"), "stock dark border 51,51,51");
    assert_eq!(decl(&d, "--color-base-30").as_deref(), Some("#333333"), "the ramp row feeding the border must be the stock grey too");
    assert_eq!(decl(&d, "--text-chrome-alt").as_deref(), Some("#dadada"));
    // #352 (on develop) derives the faint chrome ink from the theme --text-faint instead of a
    // literal; #340's requirement is the dark value 102,102,102, so check what it resolves to.
    assert_eq!(decl(&d, "--text-chrome-faint").as_deref(), Some("var(--text-faint)"));
    assert_eq!(decl(&d, "--text-chrome-dim").as_deref(), Some("var(--text-faint)"));
    assert_eq!(decl(&d, "--text-faint").as_deref(), Some("var(--color-base-50, #666666)"), "dark faint ink 102,102,102 (#340)");
    assert_eq!(decl(&d, "--color-base-50").as_deref(), Some("#666666"), "dark faint ink 102,102,102 (#340)");
}

#[test]
fn h340_done_task_ink_is_text_muted_in_both_modes() {
    let c = style();
    let d = rule(&c, "body");
    assert_eq!(decl(&d, "--checklist-done-color").as_deref(), Some("var(--text-muted)"),
        "stock done-task ink = --text-muted (dark 179,179,179 / light 92,92,92) (#340)");
    let l = rule(&c, "body:where(.theme-light, :root[data-theme=\"light\"] > :not(.theme-dark))");
    assert_eq!(decl(&l, "--checklist-done-color"), None, "light must inherit the same wiring, not a second literal");
}

#[test]
fn h340_negative_no_catppuccin_literal_in_the_dark_default_block() {
    let c = style();
    let d = rule(&c, "body").to_ascii_lowercase();
    for hex in CTP {
        // a hex followed by more hex digits would be a different colour; require a boundary
        let mut at = 0;
        while let Some(k) = d[at..].find(hex) {
            let e = at + k + hex.len();
            let next = d.as_bytes().get(e).copied().unwrap_or(b';');
            assert!(next.is_ascii_hexdigit(), "Catppuccin literal {hex} is back in the dark default block (#340)");
            at = e;
        }
    }
}

#[test]
fn h340_negative_light_block_untouched() {
    let c = style();
    let l = rule(&c, "body:where(.theme-light, :root[data-theme=\"light\"] > :not(.theme-dark))");
    assert_eq!(decl(&l, "--background-secondary-alt").as_deref(), Some("var(--color-base-05, #e5e5e5)"), "light is not part of #340");
    assert_eq!(decl(&l, "--background-modifier-border").as_deref(), Some("var(--color-base-30, #e4e4e4)"));
}

// #343 (Solarized lp.tag.btrr stock 0px, ours 28px on fix 8a05186): stock's live-preview tag is two spans,
// .cm-hashtag-begin ('#') + .cm-hashtag-end (name), and themes square/style THOSE. Ours drew the pill on one
// a.tag the theme selectors never match.
fn editor_js() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/editor.js");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

#[test]
fn h343_live_preview_tag_is_split_into_stock_hashtag_spans() {
    let j = editor_js();
    assert!(j.contains(r#"Ed.el("span", "cm-hashtag cm-hashtag-begin")"#), "lp tag '#' must be .cm-hashtag-begin (#343)");
    assert!(j.contains(r#"Ed.el("span", "cm-hashtag cm-hashtag-end")"#), "lp tag name must be .cm-hashtag-end (#343)");
    let c = style();
    let e = rule(&c, ":where(.lp a.tag) > .cm-hashtag-end");
    assert_eq!(decl(&e, "border-radius").as_deref(), Some("0 var(--tag-radius, 2em) var(--tag-radius, 2em) 0"), "end span carries the end corners");
    let b = rule(&c, ":where(.lp a.tag) > .cm-hashtag-begin");
    assert_eq!(decl(&b, "border-radius").as_deref(), Some("var(--tag-radius, 2em) 0 0 var(--tag-radius, 2em)"), "begin span carries the start corners");
    let w = rule(&c, ".lp a.tag, .lp a.tag:hover");
    assert_eq!(decl(&w, "background").as_deref(), Some("none"), "the wrapper must not paint a second pill");
    assert_eq!(decl(&w, "border-radius").as_deref(), Some("0"));
}

#[test]
fn h343_negative_hashtag_pill_rules_stay_below_theme_specificity() {
    let c = style();
    // a theme's `.cm-hashtag.cm-hashtag-end {border-radius: unset}` (0,2,0) must beat ours, as it does in stock
    for bad in [".lp a.tag > .cm-hashtag", ".lp .tag > .cm-hashtag", ".lp .cm-hashtag", "a.tag > .cm-hashtag"] {
        assert!(!c.contains(&format!("\n{bad}")), "`{bad}` out-specifies theme .cm-hashtag rules (#343)");
    }
}
