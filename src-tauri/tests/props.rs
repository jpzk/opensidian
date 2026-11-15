// SPDX-License-Identifier: GPL-3.0-or-later
//! Issue #119: the Properties model (ui/props-model.js), run in JavaScriptCore like
//! tests/graphphys.rs runs ui/graph-phys.js. The data-safety contract, byte for byte:
//!   1. a fixture with every type reads as the rows the editor shows (type, value, read-only);
//!   2. an edit of one entry rewrites that entry's lines and NOTHING else (prefix and suffix of
//!      the file identical), for every editable type;
//!   3. comments, nested maps, block scalars, anchors and unknown types survive any edit and are
//!      read-only; a value edit on them is refused;
//!   4. invalid frontmatter (unclosed, a non-key line, tabs, duplicate keys) has no rows and every
//!      edit function refuses (null) — nothing is ever written from it;
//!   5. CRLF notes keep CRLF; rename touches only the key text; delete drops only the entry
//!      (an emptied block goes); add appends before the fence or creates the block.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..") }

const CHECKS: &str = r#"(function () {
  "use strict";
  const M = window.PropsModel;
  const eq = (a, b, what) => { if (a !== b) throw new Error(what + ":\n got  " + JSON.stringify(a) + "\n want " + JSON.stringify(b)); };
  const J = JSON.stringify;
  // only the bytes inside [from, to) may differ between before and after
  const onlyIn = (before, after, from, to, what) => {
    eq(after.slice(0, from), before.slice(0, from), what + " (prefix)");
    const tail = before.length - to;
    eq(after.slice(after.length - tail), before.slice(to), what + " (suffix)");
  };
  const ALL = [
    "---",
    "title: Hello world",
    "# a comment the editor never shows",
    "count: 42",
    "ratio: 1.5",
    "done: false",
    "day: 2024-03-01",
    "at: 2024-03-01T10:30",
    "list:",
    "  - one",
    "  - \"two words\"",
    "flow: [a, 'b c']",
    "tags:",
    "  - x/y",
    "aliases: Nick",
    "cssclasses:",
    "- wide",
    "nested:",
    "  inner: 1",
    "  deep:",
    "    - z",
    "block: |",
    "  line one",
    "  line two",
    "anchor: &a hi",
    "trailing: v # note",
    "hexy: 0x1F",
    "empty:",
    "custom: thing",
    "---",
    "# Body",
    "text after",
    "",
  ].join("\n");
  const types = { custom: "mything", ratio: "number" };
  const R = M.rows(ALL, types);
  if (!R || !R.rows) throw new Error("ALL did not parse: " + J(R));
  const shape = R.rows.map(r => r.key + "=" + r.type + (r.ok ? ":" + J(r.value) : ":RO")).join(" ");
  eq(shape, 'title=text:"Hello world" count=number:42 ratio=number:1.5 done=checkbox:false day=date:"2024-03-01" ' +
    'at=datetime:"2024-03-01T10:30" list=multitext:["one","two words"] flow=multitext:["a","b c"] tags=tags:["x/y"] ' +
    'aliases=aliases:["Nick"] cssclasses=multitext:["wide"] nested=text:RO block=text:RO anchor=text:RO trailing=text:RO ' +
    'hexy=text:RO empty=text:null custom=mything:RO', "rows of the every-type fixture");
  // 2. one-entry edits, every editable type
  const edits = [
    ["title", "text", "New: title"], ["count", "number", 7], ["ratio", "number", 2.25], ["done", "checkbox", true],
    ["day", "date", "2025-12-31"], ["at", "datetime", "2025-12-31T08:00"], ["list", "multitext", ["one", "three"]],
    ["flow", "multitext", ["a"]], ["tags", "tags", ["x/y", "new"]], ["aliases", "aliases", ["Nick", "N"]],
    ["cssclasses", "multitext", []], ["empty", "text", "filled"], ["title", "text", "123"], ["title", "text", ""],
  ];
  const want = {
    title: "title: \"New: title\"\n", count: "count: 7\n", ratio: "ratio: 2.25\n", done: "done: true\n",
    day: "day: 2025-12-31\n", at: "at: 2025-12-31T08:00\n", list: "list:\n  - one\n  - three\n", flow: "flow:\n  - a\n",
    tags: "tags:\n  - x/y\n  - new\n", aliases: "aliases:\n  - Nick\n  - N\n", cssclasses: "cssclasses:\n", empty: "empty: filled\n",
  };
  for (const [k, t, v] of edits) {
    const ch = M.setValue(ALL, k, t, v, types);
    if (!ch) throw new Error("setValue refused " + k);
    const out = M.apply(ALL, ch);
    onlyIn(ALL, out, ch.from, ch.to, "edit " + k);
    const w = k === "title" ? (v === "123" ? "title: \"123\"\n" : v === "" ? "title:\n" : want.title) : want[k];
    eq(ch.insert, w, "emitted " + k + "=" + J(v));
    // the edited value reads back as written; every other row unchanged
    const back = M.rows(out, types).rows;
    for (const r of back) {
      const o = R.rows.find(x => x.key === r.key);
      if (r.key === k) { const exp = v === "" || (Array.isArray(v) && !v.length) ? null : v; eq(J(r.value), J(exp), "read back " + k); }
      else eq(J(r), J(o), "untouched row " + r.key + " after editing " + k);
    }
  }
  // 3. read-only rows: value edits refused, the raw text kept
  for (const k of ["nested", "block", "anchor", "trailing", "hexy", "custom"]) {
    if (M.setValue(ALL, k, "text", "x", types) !== null) throw new Error("setValue on read-only " + k + " was not refused");
  }
  eq(R.rows.find(r => r.key === "nested").raw, "nested:\n  inner: 1\n  deep:\n    - z", "nested raw");
  // a comment between two entries survives an edit of either neighbour
  const t2 = M.apply(ALL, M.setValue(ALL, "count", "number", 9, types));
  if (t2.indexOf("# a comment the editor never shows\ncount: 9\n") < 0) throw new Error("comment lost: " + t2);
  // number edits refuse non-numbers
  if (M.setValue(ALL, "count", "number", NaN, types) !== null) throw new Error("NaN accepted");
  // 4. invalid frontmatter: no rows, every edit refused
  const BAD = [
    "---\ntitle: a\nno closing fence\n",
    "---\ntitle: a\njust words\n---\nbody\n",
    "---\ntitle: a\n\tx: 1\n---\n",
    "---\ntitle: a\ntitle: b\n---\n",
    "---\n  indented: first\n---\n",
  ];
  for (const b of BAD) {
    const r = M.rows(b, {});
    if (!r || !r.invalid) throw new Error("invalid fixture read as rows: " + J(b) + " -> " + J(r));
    for (const ch of [M.setValue(b, "title", "text", "z", {}), M.rename(b, "title", "z"), M.remove(b, "title"), M.add(b, "z", "text", null)])
      if (ch !== null) throw new Error("an edit of invalid frontmatter was not refused: " + J(b) + " -> " + J(ch));
  }
  // no frontmatter: rows null; add creates the block, body untouched
  const NONE = "# Just a body\n";
  eq(M.rows(NONE, {}), null, "no frontmatter");
  eq(M.apply(NONE, M.add(NONE, "status", "text", null)), "---\nstatus:\n---\n# Just a body\n", "add to a note without frontmatter");
  // 5. CRLF
  const CR = "---\r\na: 1\r\nb: two\r\n---\r\nbody\r\n";
  const crc = M.setValue(CR, "b", "text", "three", {});
  eq(M.apply(CR, crc), "---\r\na: 1\r\nb: three\r\n---\r\nbody\r\n", "CRLF kept");
  eq(M.apply(CR, M.add(CR, "c", "checkbox", true)), "---\r\na: 1\r\nb: two\r\nc: true\r\n---\r\nbody\r\n", "CRLF add");
  // rename: the key text only (a read-only nested map too), refused onto an existing key
  const rn = M.rename(ALL, "nested", "renamed");
  onlyIn(ALL, M.apply(ALL, rn), rn.from, rn.to, "rename nested");
  if (M.apply(ALL, rn).indexOf("renamed:\n  inner: 1\n  deep:\n    - z\n") < 0) throw new Error("rename lost the nested value");
  eq(M.rename(ALL, "title", "COUNT"), null, "rename onto an existing key (case-insensitive) refused");
  eq(M.apply("---\na: 1\n---\n", M.rename("---\na: 1\n---\n", "a", "my key: x")), "---\n\"my key: x\": 1\n---\n", "rename quotes a key that needs it");
  // delete: only the entry; the last entry takes the empty block with it, unless a comment remains
  const dl = M.remove(ALL, "list");
  onlyIn(ALL, M.apply(ALL, dl), dl.from, dl.to, "delete list");
  eq(M.apply(ALL, dl).indexOf("list:"), -1, "list deleted");
  eq(M.apply("---\na: 1\n---\nbody\n", M.remove("---\na: 1\n---\nbody\n", "a")), "body\n", "last entry takes the block");
  eq(M.apply("---\n# keep\na: 1\n---\nbody\n", M.remove("---\n# keep\na: 1\n---\nbody\n", "a")), "---\n# keep\n---\nbody\n", "a comment keeps the block");
  // add: before the fence; duplicate refused
  eq(M.apply("---\na: 1\n---\n", M.add("---\na: 1\n---\n", "b", "multitext", ["x"])), "---\na: 1\nb:\n  - x\n---\n", "add a list");
  eq(M.add(ALL, "Title", "text", null), null, "add a duplicate (case-insensitive) refused");
  // coerce
  eq(J([M.coerce("number", "12"), M.coerce("number", "1e3"), M.coerce("date", "2024-02-03"), M.coerce("date", "x"), M.coerce("text", "")]),
     J([12, undefined, "2024-02-03", undefined, null]), "coerce");
  return "ok";
})()"#;

#[test]
fn properties_model_round_trip() {
    let src = std::fs::read_to_string(root().join("ui/props-model.js")).unwrap();
    let ctx = javascriptcore::Context::new();
    ctx.evaluate("var window = {};");
    ctx.evaluate(&src);
    if let Some(e) = ctx.exception() { panic!("props-model.js did not load: {}", e.to_str()); }
    let out = ctx.evaluate(CHECKS).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("properties model check failed: {}", e.to_str()); }
    assert_eq!(out, "ok");
}
