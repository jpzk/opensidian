// SPDX-License-Identifier: GPL-3.0-or-later
//! issue #315: the right sidebar panes never paint EMPTY between two refreshes.
//!
//! Runs the REAL `rOutgoing` / `rTags` / `rOutline` / `rBacklinks` (+ `rpSwap`,
//! `rpDrop`, `rEmpty`) out of ui/main.js in JavaScriptCore (same engine as the
//! webview, same pinned dev-dep as listtoggle.rs) against a minimal DOM stub. The
//! stub's `inv` records, at the moment the pane is waiting on the backend, how many
//! rows the LIVE box holds: that is what the compositor can paint during the await.
//! Before the fix every pane cleared its box first (0 rows painted = the blank /
//! transient frames measured while typing); stock 1.13.7 shows none.
//!
//! Positive: during a refresh the old rows stay painted; a refresh whose data did
//! not change does not touch the live DOM at all.
//! Negatives: data that DID change is painted by that same refresh (one swap, final
//! rows = the new data only); a note switch shows the new note's rows and none of the
//! old one's; two renders in flight -> the latest wins; a FAILED fetch after a note
//! switch leaves no stale rows of the previous note.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn main_js() -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/main.js");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Source of a top-level `function NAME(` / `async function NAME(`: brace-matched,
/// skipping braces inside '...', "..." and `...` strings, // and /* */ comments.
fn func(src: &str, name: &str) -> Option<String> {
    let start = [format!("\nasync function {name}("), format!("\nfunction {name}(")]
        .iter().filter_map(|h| src.find(h.as_str())).min()? + 1;
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
            if seen && depth == 0 { return Some(src[start..=i].to_string()); }
        }
        i += 1;
    }
    panic!("unbalanced function {name}");
}

const DOM: &str = r#"
var MUT = 0, SNAP = [], WATCH = null, DATA = {}, FAIL = {}, HOLD = false, PEND = [], DONE = '';
var rpInfo = '', rbN = 0, blGen = 0, outGen = 0, tagGen = 0, tocGen = 0, tocHeads = [], ulOpen = false;
function Txt(t) { this.nodeType = 3; this.data = String(t); this.parentNode = null; this.childNodes = []; }
Object.defineProperty(Txt.prototype, 'textContent', { get: function () { return this.data; } });
function El(tag, frag) {
  this.nodeType = frag ? 11 : 1; this._frag = !!frag; this.tagName = String(tag || '').toUpperCase();
  this.childNodes = []; this.parentNode = null; this.dataset = {}; this.style = {}; this.className = '';
  this.hidden = false; this.title = '';
  var s = this;
  this.classList = {
    toggle: function (c, f) {
      var a = s.className.split(' ').filter(Boolean), has = a.indexOf(c) >= 0, on = f === undefined ? !has : !!f;
      if (on && !has) a.push(c); if (!on && has) a.splice(a.indexOf(c), 1);
      s.className = a.join(' '); return on;
    },
    contains: function (c) { return s.className.split(' ').indexOf(c) >= 0; }
  };
}
El.prototype._touch = function () { if (this._q) return; for (var n = this; n; n = n.parentNode) if (n._live) { MUT++; return; } };
El.prototype._adopt = function (c) {
  if (c.parentNode) { var p = c.parentNode.childNodes; p.splice(p.indexOf(c), 1); }
  c.parentNode = this;
};
El.prototype.appendChild = function (c) {
  if (c._frag) { var k = c.childNodes.slice(); this._q2(function (me) { for (var i = 0; i < k.length; i++) me.appendChild(k[i]); }); this._touch(); return c; }
  this._adopt(c); this.childNodes.push(c); this._touch(); return c;
};
El.prototype._q2 = function (f) { var o = this._q; this._q = true; try { f(this); } finally { this._q = o; } };
El.prototype.replaceChildren = function () {
  var args = arguments;
  this._q2(function (me) {
    for (var i = 0; i < me.childNodes.length; i++) me.childNodes[i].parentNode = null;
    me.childNodes = [];
    for (var j = 0; j < args.length; j++) me.appendChild(args[j]);
  });
  this._touch();
};
Object.defineProperty(El.prototype, 'firstChild', { get: function () { return this.childNodes[0] || null; } });
Object.defineProperty(El.prototype, 'childElementCount', { get: function () { return this.childNodes.filter(function (c) { return c.nodeType === 1; }).length; } });
Object.defineProperty(El.prototype, 'isConnected', { get: function () { for (var n = this; n; n = n.parentNode) if (n._live) return true; return false; } });
Object.defineProperty(El.prototype, 'textContent', {
  get: function () { return this.childNodes.map(function (c) { return c.textContent; }).join(''); },
  set: function (t) { var me = this; me._q2(function () { me.replaceChildren(); if (String(t) !== '') me.appendChild(new Txt(t)); }); me._touch(); }
});
Object.defineProperty(El.prototype, 'innerHTML', {
  set: function (h) {
    var me = this;
    me._q2(function () {
      me.replaceChildren();
      h = String(h).replace(/<svg[\s\S]*?<\/svg>/g, function () { me.appendChild(new El('svg')); return ''; });
      var re = /<(\w+) class="([^"]*)">([^<]*)<\/\1>|([^<]+)/g, m;
      while ((m = re.exec(h))) {
        if (m[4] !== undefined) { me.appendChild(new Txt(m[4])); continue; }
        var s = new El(m[1]); s.className = m[2]; if (m[3]) s.appendChild(new Txt(m[3])); me.appendChild(s);
      }
    });
    me._touch();
  }
});
El.prototype.querySelector = function (sel) {
  var c = sel.replace(/^\./, '');
  for (var i = 0; i < this.childNodes.length; i++) {
    var k = this.childNodes[i];
    if (k.nodeType !== 1) continue;
    if (k.className.split(' ').indexOf(c) >= 0) return k;
    var r = k.querySelector(sel); if (r) return r;
  }
  return null;
};
var document = {
  createElement: function (t) { return new El(t); },
  createTextNode: function (t) { return new Txt(t); },
  createDocumentFragment: function () { return new El('#fragment', true); }
};
var BOX = {};
['outlist', 'outhead', 'taglist', 'tagshead', 'toclist', 'bllist', 'blhead'].forEach(function (id) { var e = new El('div'); e._live = true; e.id = id; BOX[id] = e; });
function $(id) { return BOX[id]; }
function clone(v) { return v === undefined ? v : JSON.parse(JSON.stringify(v)); }
function inv(cmd, a) {
  if (WATCH) SNAP.push(BOX[WATCH].childNodes.length);   // what the compositor can paint during the await
  var key = a && a.name ? cmd + ':' + a.name : cmd;
  if (FAIL[key]) return Promise.reject(new Error('refused ' + key));
  var v = clone(DATA[key] !== undefined ? DATA[key] : DATA[cmd]);
  if (HOLD) return new Promise(function (res) { PEND.push(function () { res(v); }); });
  return Promise.resolve(v);
}
function cvBacklinks(n) { return Promise.resolve([]); }
function cvCanvasBacklinks(n) { return Promise.resolve([]); }
function cvBacklinkRows(box, cvb) {}
function sideRow() {} function tocSync() {} function tocGo() {} function tagSearch() {} function updateTitle() {}
function saveFailed() {} function saveCleared() {} function groups() { return []; }
function run(p) { DONE = 'pending'; p.then(function () { DONE = 'ok'; }, function (e) { DONE = 'err:' + e.message; }); }
"#;

fn ev(ctx: &javascriptcore::Context, js: &str) -> String {
    let v = ctx.evaluate(js).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("{js}: threw {}", e.to_str()); }
    v
}

fn ctx() -> javascriptcore::Context {
    let src = main_js();
    let ctx = javascriptcore::Context::new();
    ctx.evaluate(DOM);
    if let Some(e) = ctx.exception() { panic!("DOM stub did not load: {}", e.to_str()); }
    let chev = src.lines().find(|l| l.starts_with("const CHEV = ")).expect("CHEV in ui/main.js");
    ctx.evaluate(&chev.replacen("const ", "var ", 1));
    // rpSwap / rpDrop are the fix's helpers: absent before it (the behavioural asserts then fail)
    for f in ["rEmpty", "rpSwap", "rpDrop", "rOutgoing", "rTags", "rOutline", "rBacklinks"] {
        let Some(body) = func(&src, f) else {
            assert!(f == "rpSwap" || f == "rpDrop", "{f} not in ui/main.js");
            continue;
        };
        ctx.evaluate(&body);
        if let Some(e) = ctx.exception() { panic!("{f} did not load: {}", e.to_str()); }
    }
    // the microtask queue drains between evaluations (every async case below relies on it)
    ev(&ctx, "var Z = 0; Promise.resolve().then(function () { Z = 1; })");
    assert_eq!(ev(&ctx, "Z"), "1", "JSC must drain promise jobs between evaluate calls");
    ctx
}

/// one refresh of `call` with WATCH on `box`: returns (rows painted during the await, live mutations, box text)
fn refresh(c: &javascriptcore::Context, call: &str, bx: &str) -> (String, String, String) {
    ev(c, &format!("SNAP = []; MUT = 0; WATCH = '{bx}'; run({call}); WATCH = null"));
    assert_eq!(ev(c, "DONE"), "ok", "{call}");
    (ev(c, "SNAP.join(',')"), ev(c, "MUT"), ev(c, &format!("BOX['{bx}'].textContent")))
}

fn nonzero(snap: &str) -> bool { !snap.is_empty() && snap.split(',').all(|v| v != "0") }

#[test]
fn outgoing_never_paints_empty_and_skips_unchanged() {
    let c = ctx();
    ev(&c, "DATA['outgoing:N1'] = [{text:'Alpha', target:'Alpha'}, {text:'ghost', target:null}]");
    let (_, _, t) = refresh(&c, "rOutgoing('N1')", "outlist");
    assert_eq!(t, "Alphaghost");
    assert_eq!(ev(&c, "BOX.outhead.textContent"), "Links2");
    assert_eq!(ev(&c, "rpInfo"), "out:1/1");
    // a keystroke in plain text: same links -> the old rows stay painted, the live DOM is untouched
    let (snap, mutn, t) = refresh(&c, "rOutgoing('N1')", "outlist");
    assert!(nonzero(&snap), "outgoing pane painted EMPTY during the refresh (#315): rows during await = {snap}");
    assert_eq!(mutn, "0", "unchanged outgoing links must not touch the DOM");
    assert_eq!(t, "Alphaghost");
    // NEGATIVE: a link was added -> this same refresh paints it, in ONE swap per live box
    ev(&c, "DATA['outgoing:N1'] = [{text:'Alpha', target:'Alpha'}, {text:'ghost', target:null}, {text:'Beta', target:'Beta'}]");
    let (snap, mutn, t) = refresh(&c, "rOutgoing('N1')", "outlist");
    assert!(nonzero(&snap), "outgoing pane painted EMPTY before the new rows: {snap}");
    assert_eq!(t, "AlphaghostBeta", "changed links must update within one refresh");
    assert_eq!(mutn, "2", "one swap of the rows + one of the header");
    assert_eq!(ev(&c, "BOX.outhead.textContent"), "Links3");
    // NEGATIVE: all links removed -> the empty-state row, not the old rows
    ev(&c, "DATA['outgoing:N1'] = []");
    let (_, _, t) = refresh(&c, "rOutgoing('N1')", "outlist");
    assert_eq!(t, "No outgoing links.");
}

#[test]
fn outgoing_note_switch_race_and_failure_leave_no_stale_rows() {
    let c = ctx();
    ev(&c, "DATA['outgoing:N1'] = [{text:'OneLink', target:'X'}]; DATA['outgoing:N2'] = [{text:'TwoLink', target:'Y'}]");
    refresh(&c, "rOutgoing('N1')", "outlist");
    let (_, _, t) = refresh(&c, "rOutgoing('N2')", "outlist");
    assert_eq!(t, "TwoLink", "after a note switch only the new note's links");
    // two renders in flight (fast switches): N1 answers LAST, N2 must still own the pane
    ev(&c, "HOLD = true; PEND = []; rOutgoing('N1'); rOutgoing('N2'); HOLD = false");
    ev(&c, "PEND[1](); PEND[0]()");
    assert_eq!(ev(&c, "BOX.outlist.textContent"), "TwoLink", "an older render must not overwrite the latest");
    // a switch whose fetch FAILS: no stale TwoLink rows for the new note
    ev(&c, "FAIL['outgoing:N3'] = true");
    ev(&c, "run(rOutgoing('N3'))");
    assert!(ev(&c, "DONE").starts_with("err:"), "the failure still propagates");
    assert_eq!(ev(&c, "BOX.outlist.textContent"), "", "a failed fetch must not leave the previous note's rows");
    assert_eq!(ev(&c, "BOX.outhead.textContent"), "");
    // and the next good refresh rebuilds it
    ev(&c, "FAIL = {}; DATA['outgoing:N3'] = [{text:'Three', target:'Z'}]");
    let (_, _, t) = refresh(&c, "rOutgoing('N3')", "outlist");
    assert_eq!(t, "Three");
}

#[test]
fn tags_never_paint_empty_and_skip_unchanged() {
    let c = ctx();
    ev(&c, "DATA['tag_counts'] = {a: 3, 'a/b': 1, c: 2}");
    let (_, _, t) = refresh(&c, "rTags()", "taglist");
    assert_eq!(t, "#a4#b1#c2", "count desc, nested folded (parent = own + kids)");
    assert_eq!(ev(&c, "BOX.tagshead.textContent"), "Tags3");
    let (snap, mutn, t) = refresh(&c, "rTags()", "taglist");
    assert!(nonzero(&snap), "tags pane painted EMPTY during the refresh (#315): {snap}");
    assert_eq!(mutn, "0", "unchanged tags must not touch the DOM");
    assert_eq!(t, "#a4#b1#c2");
    // NEGATIVE: a new tag typed -> updated in this refresh
    ev(&c, "DATA['tag_counts'] = {a: 3, 'a/b': 1, c: 2, zed: 9}");
    let (snap, mutn, t) = refresh(&c, "rTags()", "taglist");
    assert!(nonzero(&snap), "{snap}");
    assert_eq!(t, "#zed9#a4#b1#c2");
    assert_eq!(mutn, "2");
    // two in flight: the latest wins, rows are not appended twice
    ev(&c, "HOLD = true; PEND = []; DATA['tag_counts'] = {old: 1}; rTags(); DATA['tag_counts'] = {neu: 1}; rTags(); HOLD = false");
    ev(&c, "PEND[1](); PEND[0]()");
    assert_eq!(ev(&c, "BOX.taglist.textContent"), "#neu1");
}

#[test]
fn outline_never_paints_empty_skips_unchanged_and_switches() {
    let c = ctx();
    ev(&c, "DATA['outline:N1'] = [{level:1, text:'Top', line:0}, {level:2, text:'Sub', line:3}]; DATA['outline:N2'] = [{level:1, text:'Other', line:0}]");
    let (_, _, t) = refresh(&c, "rOutline('N1')", "toclist");
    assert_eq!(t, "TopSub");
    let (snap, mutn, _) = refresh(&c, "rOutline('N1')", "toclist");
    assert!(nonzero(&snap), "outline pane painted EMPTY during the refresh (#315): {snap}");
    assert_eq!(mutn, "0");
    assert_eq!(ev(&c, "tocHeads.length"), "2");
    // NEGATIVE: a heading renamed -> updated in this refresh
    ev(&c, "DATA['outline:N1'] = [{level:1, text:'Top', line:0}, {level:2, text:'Renamed', line:3}]");
    let (snap, mutn, t) = refresh(&c, "rOutline('N1')", "toclist");
    assert!(nonzero(&snap), "{snap}");
    assert_eq!((t.as_str(), mutn.as_str()), ("TopRenamed", "1"));
    // note switch: only the new note's headings; failed switch: no stale rows
    let (_, _, t) = refresh(&c, "rOutline('N2')", "toclist");
    assert_eq!(t, "Other");
    assert_eq!(ev(&c, "rpInfo"), "toc:1");
    ev(&c, "FAIL['outline:N3'] = true; run(rOutline('N3'))");
    assert!(ev(&c, "DONE").starts_with("err:"));
    assert_eq!(ev(&c, "BOX.toclist.textContent"), "");
    // no headings -> the empty-state row
    ev(&c, "FAIL = {}; DATA['outline:N3'] = []");
    let (_, _, t) = refresh(&c, "rOutline('N3')", "toclist");
    assert_eq!(t, "No headings.");
}

#[test]
fn backlinks_never_paint_empty_skip_unchanged_and_switch() {
    let c = ctx();
    ev(&c, "DATA['backlinks_ctx:N1'] = [{note:'P', lines:[[2, 'see [[N1]] here']]}];\
            DATA['unlinked_mentions:N1'] = [{note:'Q', line:1, text:'N1 plain', mcol:0, mlen:2, col:0, len:2}];\
            DATA['backlinks_ctx:N2'] = [];\
            DATA['unlinked_mentions:N2'] = []");
    let (_, _, t) = refresh(&c, "rBacklinks('N1')", "bllist");
    assert_eq!(t, "P1see [[N1]] hereUnlinked mentions1QLinkN1 plain");
    assert_eq!(ev(&c, "BOX.blhead.textContent"), "Linked mentions1");
    assert_eq!(ev(&c, "rpInfo"), "bl:1|ul:1");
    let (snap, mutn, _) = refresh(&c, "rBacklinks('N1')", "bllist");
    assert!(nonzero(&snap), "backlinks pane painted EMPTY during the refresh (#315): {snap}");
    assert_eq!(mutn, "0");
    assert_eq!(ev(&c, "rbN"), "2", "[rpn:] still counts every render started");
    // NEGATIVE: the unlinked mention became a link -> moved up within this refresh
    ev(&c, "DATA['backlinks_ctx:N1'] = [{note:'P', lines:[[2, 'see [[N1]] here']]}, {note:'Q', lines:[[1, '[[N1]] plain']]}];\
            DATA['unlinked_mentions:N1'] = []");
    let (snap, _, t) = refresh(&c, "rBacklinks('N1')", "bllist");
    assert!(nonzero(&snap), "{snap}");
    assert_eq!(t, "P1see [[N1]] hereQ1[[N1]] plainUnlinked mentions0No unlinked mentions found.");
    assert_eq!(ev(&c, "rpInfo"), "bl:2|ul:0");
    // note switch -> no stale rows; failed switch -> empty
    let (_, _, t) = refresh(&c, "rBacklinks('N2')", "bllist");
    assert_eq!(t, "No backlinks found.Unlinked mentions0No unlinked mentions found.");
    ev(&c, "FAIL['backlinks_ctx:N3'] = true; run(rBacklinks('N3'))");
    assert!(ev(&c, "DONE").starts_with("err:"));
    assert_eq!(ev(&c, "BOX.bllist.textContent + '|' + BOX.blhead.textContent"), "|");
    // no note -> linked header + empty row, no unlinked section (as before)
    ev(&c, "FAIL = {}");
    let (_, _, t) = refresh(&c, "rBacklinks(null)", "bllist");
    assert_eq!(t, "No backlinks found.");
    assert_eq!(ev(&c, "rpInfo"), "bl:0|ul:0");
}

/// source guard: no right pane clears its live box before awaiting the backend again
#[test]
fn no_pane_clears_before_its_await() {
    let src = main_js();
    for f in ["rOutgoing", "rTags", "rOutline", "rBacklinks"] {
        let body = func(&src, f).unwrap();
        let first_await = body.find("await ").unwrap_or_else(|| panic!("{f} awaits"));
        let pre = &body[..first_await];
        assert!(!pre.contains("textContent = \"\"") && !pre.contains(".replaceChildren("),
            "{f} clears its pane before the await (#315 blank frame)");
    }
}
