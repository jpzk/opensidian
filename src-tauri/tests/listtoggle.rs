// SPDX-License-Identifier: GPL-3.0-or-later
//! listtoggle unit table (spec R4 / acceptance A2).
//!
//! Runs the REAL `Ed.listToggle` out of ui/editor.js in JavaScriptCore — the
//! engine the webview itself runs it in (javascriptcore-rs is webkit2gtk's own
//! transitive dep, pinned `=` to the locked version: no new crate) — against one
//! row per measured stock 1.13.7 case. Expected bytes come from the committed
//! evidence dumps, never from this file, so a row cannot drift from what stock
//! wrote. See tests/listtoggle.tsv for the row format.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..") }

/// cat -A text -> bytes. Only what the fixtures use: "$" = end of line, "^I" = TAB.
/// Anything else cat -A escapes ("^X", "M-") fails loudly instead of decoding wrong.
fn uncat(block: &[&str]) -> String {
    let mut out = String::new();
    for l in block {
        let body = l.strip_suffix('$').unwrap_or_else(|| panic!("cat -A line without $: {l:?}"));
        let body = body.replace("^I", "\t");
        assert!(!body.contains("M-") && !body.contains('^'), "undecoded cat -A escape in {l:?}");
        out.push_str(&body);
        out.push('\n');
    }
    out
}

struct Case { before: String, blocks: Vec<(String, String)>, steps: String, palettes: Vec<String> }

fn load(name: &str) -> Case {
    let p = root().join("docs/recon-listtoggle/evidence").join(format!("{name}.txt"));
    let t = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    let lines: Vec<&str> = t.lines().collect();
    let (mut blocks, mut steps, mut palettes) = (Vec::new(), String::new(), Vec::new());
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if let Some(s) = l.strip_prefix("# steps: ") { steps = s.to_string(); }
        if let Some(s) = l.strip_prefix("> palette: ") { palettes.push(s.to_string()); }
        if let Some(rest) = l.strip_prefix("--- ") {
            if let Some((tag, _)) = rest.split_once(':') {
                let mut j = i + 1;
                while j < lines.len() && lines[j] != "--- end ---" { j += 1; }
                let body = &lines[i + 1..j];
                // the clipboard dump is `xclip -o | cat -A; echo`: its LAST line has no
                // "$" (a selection need not end in a newline) and the echo ends it
                let text = if tag == "clip" {
                    let (last, init) = body.split_last().expect("empty clip block");
                    let last = uncat(&[&format!("{last}$")]);
                    uncat(init) + last.strip_suffix('\n').unwrap()
                } else { uncat(body) };
                blocks.push((tag.to_string(), text));
                i = j;
            }
        }
        i += 1;
    }
    let before = blocks.iter().find(|b| b.0 == "before").expect("no before block").1.clone();
    Case { before, blocks, steps, palettes }
}

type P = (usize, usize);
fn pos(s: &str) -> P {
    let (l, c) = s.split_once('.').unwrap();
    (l.parse().unwrap(), c.parse().unwrap())
}

/// Replays the recorded "k" keystrokes (shift extends the head) to cross-check
/// the table's selection. A VERTICAL move from a column > 0 returns None: Live
/// Preview keeps the goal column in PIXELS of a proportional font ("al|pha" +
/// Down lands "g|amma", q12b-clip*), which a character replay cannot know — such
/// rows are keys=no and their selection is pinned by a clipboard dump instead.
fn replay(text: &str, steps: &str) -> Option<(P, P)> {
    let lines: Vec<&str> = text.split('\n').collect();
    let len = |l: usize| lines[l].chars().count();
    // (pos, column-unknown) for anchor and head
    let (mut a, mut h, mut goal, mut ua, mut uh): (P, P, usize, bool, bool) = ((0, 0), (0, 0), 0, false, false);
    let k = steps.split('|').find(|s| s.starts_with("k ") && !s.contains("ctrl+e"))
        .expect("no keystroke step");
    for key in k[2..].split_whitespace() {
        let shift = key.starts_with("shift+");
        match key.trim_start_matches("shift+") {
            "ctrl+Home" => { h = (0, 0); goal = 0; uh = false; }
            "Down" => { if h.0 + 1 < lines.len() { h = (h.0 + 1, goal.min(len(h.0 + 1))); uh = uh || goal > 0; } }
            "Right" => { h = if h.1 < len(h.0) { (h.0, h.1 + 1) } else { (h.0 + 1, 0) }; goal = h.1; }
            "Left" => { h = if h.1 > 0 { (h.0, h.1 - 1) } else { (h.0 - 1, len(h.0 - 1)) }; goal = h.1; }
            "End" => { h = (h.0, len(h.0)); goal = h.1; uh = false; }
            "Home" => { h = (h.0, 0); goal = 0; uh = false; }
            other => panic!("replay: key {other:?} not modelled"),
        }
        if !shift { a = h; ua = uh; }
    }
    if ua || uh { None } else { Some((a, h)) }
}

fn js_str(s: &str) -> String { serde_json::to_string(s).unwrap() }

#[test]
fn listtoggle_table() {
    let ed = std::fs::read_to_string(root().join("ui/editor.js")).unwrap();
    let ctx = javascriptcore::Context::new();
    // editor.js's only load-time DOM touch is one selectionchange listener
    ctx.evaluate("var window = {}; var document = { addEventListener() {} };");
    ctx.evaluate(&ed);
    if let Some(e) = ctx.exception() { panic!("editor.js did not load: {}", e.to_str()); }
    assert_eq!(ctx.evaluate("typeof Ed.listToggle").unwrap().to_str().as_str(), "function",
               "Ed.listToggle missing from ui/editor.js");

    let table = std::fs::read_to_string(root().join("src-tauri/tests/listtoggle.tsv")).unwrap();
    let (mut rows, mut fails, mut seen) = (0, Vec::new(), std::collections::BTreeSet::new());
    for line in table.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        assert_eq!(f.len(), 4, "bad row {line:?}");
        let (name, kind, sel, keys) = (f[0], f[1], f[2], f[3]);
        seen.insert(name.to_string());
        rows += 1;
        let nfail = fails.len();
        let c = load(name);
        let want_cmd = if kind == "bullet" { "Toggle bullet list" } else { "Toggle numbered list" };
        assert!(c.palettes.iter().any(|p| p == want_cmd), "{name}: evidence ran {:?}, row says {want_cmd}", c.palettes);
        let (sa, sb) = sel.split_once('-').unwrap();
        let (a, b) = (pos(sa), pos(sb));
        if keys == "yes" {
            let r = replay(&c.before, &c.steps);
            assert_eq!(r, Some((a, b)), "{name}: table selection {sel} != replayed keys {:?}", c.steps);
        }
        let js = format!(
            "(function(){{ var r = Ed.listToggle({t}.split('\\n'), {{a:{{l:{},c:{}}}, b:{{l:{},c:{}}}}}, {k});\
             if (!r) return 'NOOP';\
             var L = r.lines.slice(), s = r.a, e = r.b;\
             var z = L[s.l].slice(0, s.c) + 'Z' + L[e.l].slice(e.c);\
             L.splice(s.l, e.l - s.l + 1, z);\
             var C = r.lines, t = s.l === e.l ? C[s.l].slice(s.c, e.c)\
               : [C[s.l].slice(s.c)].concat(C.slice(s.l + 1, e.l), [C[e.l].slice(0, e.c)]).join('\\n');\
             return JSON.stringify([r.lines.join('\\n'), L.join('\\n'), t]); }})()",
            a.0, a.1, b.0, b.1, t = js_str(&c.before), k = js_str(kind));
        let out = ctx.evaluate(&js).map(|v| v.to_str().to_string()).unwrap_or_default();
        if let Some(e) = ctx.exception() { fails.push(format!("{name}: JS threw {}", e.to_str())); ctx.clear_exception(); continue; }
        let (after, typed, clip): (String, String, String) = match serde_json::from_str::<(String, String, String)>(&out) {
            Ok(v) => v, Err(_) => { fails.push(format!("{name}: op returned {out:?}")); continue; }
        };
        for (tag, want) in &c.blocks {
            let got = match tag.as_str() {
                "before" => continue,
                "after" => &after,
                "sel" | "caret" => &typed,
                "clip" => &clip,        // stock's Ctrl+C right after the command = the selection itself
                "undo" => &c.before,   // R3's Ctrl+Z is the gate's job; here: the dump is the fixture
                other => panic!("{name}: unknown dump {other:?}"),
            };
            if got != want { fails.push(format!("{name} [{tag}]\n   want {want:?}\n    got {got:?}")); }
        }
        println!("row {name:<16} {kind:<9} {sel:<9} {}", if fails.len() == nfail { "ok" } else { "FAIL" });
    }
    // completeness: every evidence case is a row or a named exclusion
    let ev = std::fs::read_dir(root().join("docs/recon-listtoggle/evidence")).unwrap();
    for e in ev {
        let n = e.unwrap().file_name().to_string_lossy().trim_end_matches(".txt").to_string();
        let excluded = n == "q01-ids" || n == "q14-reading" || n.starts_with("q16-");
        assert!(excluded || seen.contains(&n), "evidence case {n} has no table row");
    }
    assert!(fails.is_empty(), "{} of {rows} rows FAILED:\n{}", fails.len(), fails.join("\n"));
    println!("listtoggle table: {rows}/{rows} rows pass");
    assert_eq!(rows, 53);
}
