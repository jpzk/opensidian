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
