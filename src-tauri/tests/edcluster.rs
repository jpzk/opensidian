// SPDX-License-Identifier: GPL-3.0-or-later
//! #274 review (CodeAnt 4654882322): a caret the editor API placed INSIDE a grapheme cluster.
//!
//! Runs the REAL `Ed.cluster` / `Ed.inside` / `Ed.backDel` out of ui/editor.js in JavaScriptCore —
//! the engine the webview runs them in (same setup as tests/listtoggle.rs, no new crate). JSC's
//! `Intl.Segmenter` is not the only oracle here: the rows below are UAX #29 cluster bounds, and an
//! engine that answers `containing(i)` for a HIGH surrogate i with the wrong segment start (the
//! interior-caret case, c - 1 == the high half of a pair) must not make Backspace / Delete remove part
//! of a cluster. Every mismatch is listed with what the engine returned.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..") }

const CHECKS: &str = r#"(function () {
  "use strict";
  if (!Ed.gseg()) return "NO-SEGMENTER";
  const FP = "\u{1F926}‍♂️", FAM = "\u{1F468}‍\u{1F469}‍\u{1F467}", out = [];
  const eq = (tag, got, want) => { if (String(got) !== String(want)) out.push(tag + " got " + got + " want " + want); };
  // Ed.cluster [text, col, back, want]: a column inside a cluster steps to that cluster's edge
  for (const [t, c, back, want] of [
    ["a\u{1F600}b", 2, true, 1], ["a\u{1F600}b", 2, false, 3],          // inside a surrogate pair
    ["a" + FP + "b", 2, true, 1], ["a" + FP + "b", 2, false, 6],        // c - 1 is the high half of U+1F926
    ["a" + FP + "b", 3, true, 1], ["a" + FP + "b", 5, true, 1], ["a" + FP + "b", 5, false, 6],
    ["x" + FAM, 2, true, 1], ["x" + FAM, 5, true, 1], ["x" + FAM, 4, false, 9],
    ["a" + FP + "b", 6, true, 1], ["a" + FP + "b", 1, false, 6], ["a" + FP + "b", 1, true, 0], ["ab", 1, true, 0]])
    eq("cluster(" + c + (back ? "<" : ">") + "," + t.length + ")", Ed.cluster(t, c, back), want);
  // Ed.inside [text, col, want "s,e" | "-"]: only a caret strictly inside a cluster
  for (const [t, c, want] of [
    ["a\u{1F600}b", 2, "1,3"], ["a\u{1F600}b", 1, "-"], ["a\u{1F600}b", 3, "-"],
    ["a" + FP + "b", 2, "1,6"], ["a" + FP + "b", 3, "1,6"], ["a" + FP + "b", 4, "1,6"], ["a" + FP + "b", 5, "1,6"],
    ["a" + FP + "b", 1, "-"], ["a" + FP + "b", 6, "-"], ["x" + FAM, 2, "1,9"], ["x" + FAM, 4, "1,9"], ["x" + FAM, 8, "1,9"],
    ["aéb", 2, "1,3"], ["ab", 1, "-"], ["", 0, "-"]]) {
    const r = Ed.inside(t, c);
    eq("inside(" + c + "," + t.length + ")", r ? r.join(",") : "-", want);
  }
  // Ed.backDel at a cluster EDGE is unchanged (stock oracle rows from Ed.selfTest)
  for (const [t, c, want] of [["a" + FP + "b", 6, 1], ["x" + FAM, 9, 1], ["a\u{1F44D}\u{1F3FD}b", 5, 3], ["a\u{1F600}b", 3, 1]])
    eq("backDel(" + c + "," + t.length + ")", Ed.backDel(t, c), want);
  return out.length ? out.join("; ") : "OK";
})()"#;

#[test]
fn interior_caret_cluster_bounds() {
    let ed = std::fs::read_to_string(root().join("ui/editor.js")).unwrap();
    let ctx = javascriptcore::Context::new();
    // same stubs as tests/listtoggle.rs (CENSUS_ON off, as in a shipped build)
    ctx.evaluate("var window = {}; var document = { addEventListener() {} }; var CENSUS_ON = false;");
    ctx.evaluate(&ed);
    if let Some(e) = ctx.exception() { panic!("editor.js did not load: {}", e.to_str()); }
    let r = ctx.evaluate(CHECKS).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("checks threw: {}", e.to_str()); }
    assert_eq!(r, "OK", "Ed.cluster / Ed.inside interior-caret rows");
}
