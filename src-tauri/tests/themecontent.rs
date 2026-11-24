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
