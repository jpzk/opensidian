// SPDX-License-Identifier: GPL-3.0-or-later
//! th-editor theme bundle (milestone 097): source tests for the editor/renderer DOM that
//! theme selectors key on. Stock 1.13.7 evidence (computed styles + DOM class strings read
//! with getComputedStyle / className in the running app, issue bodies #333 #319 #324 #348).
//! The live DOM is asserted in-app by `Ed.themeDom` (ui/editor.js, [edt:] census) and on the
//! box by the harness phase theme-th-editor; these tests pin the source side, negatives included.
use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// CSS text without /* comments */ (a selector named in a comment is not a rule)
fn css_rules(rel: &str) -> String {
    let s = read(rel);
    let mut out = String::new();
    let mut rest = s.as_str();
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") { Some(j) => rest = &rest[i + 2 + j + 2..], None => { rest = ""; } }
    }
    out.push_str(rest);
    out
}

const CSS: [&str; 3] = ["ui/style.css", "ui/menu.css", "ui/tables.css"];

// ---------- #333 (dup #338): the caret line carries stock's `cm-active` ----------
#[test]
fn i333_caret_line_decoration_carries_cm_active() {
    let ed = read("ui/editor.js");
    assert!(ed.contains("Ed.ACTIVE_LINE = \"cur cm-active\";"), "caret-line classes must be `cur cm-active`");
    // the ONE line decoration of the LP/source build puts it on the caret line and nowhere else
    assert!(ed.contains("(ln.number === cur ? \" \" + Ed.ACTIVE_LINE : \"\")"), "only the caret line (ln.number === cur) gets Ed.ACTIVE_LINE");
    assert_eq!(ed.matches("Ed.ACTIVE_LINE").count(), 2, "ACTIVE_LINE: one definition + one use (no second writer)");
    // the in-app DOM case runs in selfTest -> [edt:]
    assert!(ed.contains("const thd = Ed.themeDom();"), "selfTest must run Ed.themeDom");
    assert!(ed.contains("if (act() !== \"1\") return \"act1:\""), "themeDom asserts the caret line is the only .cm-active");
}

#[test]
fn i333_negative_our_css_never_paints_cm_active() {
    // default look unchanged + disable-active-line-highlight honoured: the band is the THEME's rule only.
    // Our own sheets must not select .cm-active, and our code must not set/clear the body class themes test.
    for f in CSS {
        let c = css_rules(f);
        assert!(!c.contains("cm-active"), "{f}: our CSS must not style .cm-active (stock default paints no band)");
    }
    for f in ["ui/main.js", "ui/editor.js"] {
        assert!(!read(f).contains("disable-active-line-highlight"), "{f}: the body class belongs to the theme / its settings");
    }
}

// ---------- #319: heading lines carry stock's HyperMD-header line classes ----------
#[test]
fn i319_heading_line_carries_hypermd_header_classes() {
    let ed = read("ui/editor.js");
    // the block renderer tags the LINE (d === 0) of a top-level ATX heading, with its level
    assert!(ed.contains("if (d === 0) row.classList.add(\"HyperMD-header\", \"HyperMD-header-\" + h[1].length);"),
        "Ed.block must put HyperMD-header + HyperMD-header-N on a top-level heading line");
    // the editor root carries stock's .cm-s-obsidian (the ancestor of Minimal's .cm-line.HyperMD-header rule)
    assert!(ed.contains("Ed.EDITOR_ROOT = \"cm-s-obsidian\";"), "editor root class must be cm-s-obsidian");
    assert!(ed.contains("return [EditorView.editorAttributes.of({ class: Ed.EDITOR_ROOT }), plugin,"),
        "lpExt must put Ed.EDITOR_ROOT on the editor root");
    // the in-app DOM case covers it (positive + negatives) -> [edt:]
    assert!(ed.contains("if (hd !== \"2:HyperMD-header-1\") return \"hdr:\""), "themeDom asserts only the heading line is HyperMD-header-1");
    assert!(ed.contains("[\"> # quoted\", \"  # indented\", \"#nospace\", \"####### seven\", \"plain\"]"),
        "themeDom negatives: quoted / indented / no-space / 7 hashes / plain are not heading lines");
}

#[test]
fn i319_negative_our_css_never_styles_hypermd_or_root() {
    // default look unchanged: the classes exist for THEME selectors; our own sheets must not key on them
    for f in CSS {
        let c = css_rules(f);
        assert!(!c.contains("HyperMD-header"), "{f}: our CSS must not style .HyperMD-header");
        assert!(!c.contains("cm-s-obsidian"), "{f}: our CSS must not style .cm-s-obsidian");
    }
    // the heading class is added in ONE place (no second writer drifting from the parse)
    assert_eq!(read("ui/editor.js").matches("classList.add(\"HyperMD-header\"").count(), 1, "one writer of HyperMD-header in editor.js");
}
