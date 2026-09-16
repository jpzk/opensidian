// rustidian, an Obsidian-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
// This program comes with ABSOLUTELY NO WARRANTY. It is free software, and you
// are welcome to redistribute it under the terms of the GNU GPL version 3 or
// (at your option) any later version. See LICENSE, or <https://www.gnu.org/licenses/>.
/* otel (R18): every invoke carries the innermost open UI action span as the
   `otel` arg, so backend spans nest under it (commands without the param
   ignore it). ui/otel.js owns ids, buffering and the 250ms batched IPC. */
const inv = (c, a) => { const x = otel.ctx(); return window.__TAURI__.core.invoke(c, x ? Object.assign({ otel: x }, a) : a); };
/* perf-spans shim over otel: mark(name, t0, extra) = a span that started at
   t0 and ends now; push(name, ms, extra) = an already-measured span (graph
   frames); both no-ops once the backend said telemetry is off. */
const perf = {
  now: () => performance.now(),
  mark(name, t0, extra = {}) { otel.span(name, extra, performance.now() - t0); },
  push(name, ms, extra = {}) { otel.span(name, extra, ms); },
  flush() { otel.flush(); },
};
/* R18 action span: act(name, attrs, fn) runs fn (sync or async) inside an
   open span and ends it at PAINT (otel.paint: double rAF). Backend calls made
   by fn nest under it (inv attaches the ctx). Returns fn's result. */
async function act(name, attrs, fn) {
  const sp = otel.begin(name, attrs);
  try { return await fn(sp); } finally { otel.paint(sp); }
}
const $ = id => document.getElementById(id);
/* R20: an uncaught error / rejected action left the UI mid-mutation and the
   census silently STALE (the title only moves in updateTitle) — smoke then
   reports the symptom, never the cause. Surface it as [jserr:...]. */
let jsErr = "";
function noteErr(m) {
  if (jsErr) return;                       // first error wins (the rest are fallout)
  jsErr = String(m || "err").replace(/[\[\]]/g, "").slice(0, 60);
  try { updateTitle(); } catch (_) {}
}
window.addEventListener("error", e => noteErr(e.message));
window.addEventListener("unhandledrejection", e => noteErr(e.reason && e.reason.message || e.reason));
let vaultPath = null, pmode = null, bpath = null;

/* ---------- pane model (M6 / R6.1): split tree, leaves = tab groups ----------
   Layout = Split | Group
   Split  = { dir: "row"|"col", children: [Layout...], fractions: [f...] }
   Group  = { id, tabs: [{name,mode,hist,hpos}], active, pane DOM refs,
              per-group graph / autosave / autocomplete state }
   state.focused is THE focused group (R6.3): explorer clicks, Ctrl+N,
   keymap and the graph button all target it; clicking a pane focuses it. */
let state = null;                 // { root: Split, focused: Group }
let gidSeq = 1;

function leaves(node, out = []) {
  if (node.children) node.children.forEach(c => leaves(c, out));
  else out.push(node);
  return out;
}
const groups = () => (state ? leaves(state.root) : []);
const fg = () => state.focused;
const curOf = g => (g.active >= 0 ? g.tabs[g.active].name : null);
const cur = () => (state && fg() ? curOf(fg()) : null);
// R18: is this group's active tab showing the READING pane? (the only pane the
// Rust renderer feeds — lp/source render in JS, see ui/editor.js)
const isReading = g => { const t = g.active >= 0 ? g.tabs[g.active] : null; return !!t && !t.kind && t.mode === "reading"; };
// R19: per-tab history = [{n: note, s: scrollTop}], hpos = cursor; a nav pushes at
// hpos+1 and drops the forward slice (browser semantics, docs/requirements.md R19)
// R12.4 / feedback #16: a leaf carries TWO ORTHOGONAL BITS, exactly as stock
// persists them in .obsidian/workspace.json ({"mode":"source|preview","source":
// true|false}):  tab.read = READING vs EDITING, tab.src = Source vs Live Preview.
// The view-header icon and Ctrl+E flip the FIRST one only (two states); the tab
// menu radio and the "Toggle Live Preview/Source mode" command flip the second.
// Because they are independent, the sub-mode SURVIVES a trip through reading
// view: source -> reading -> source. `tab.mode` stays as the derived 3-value
// string the rest of the app (and the [mode:] census token) reads — one name for
// a view, not a third state: setting it "reading" never touches tab.src.
function modeBits(t) {
  const m = t.mode;                       // seed from a plain literal (graph tabs carry mode: "source")
  t.read = !!t.read || m === "reading";
  t.src  = !!t.src  || m === "source";
  delete t.mode;
  Object.defineProperty(t, "mode", {
    enumerable: true, configurable: true,
    get: () => t.read ? "reading" : t.src ? "source" : "livepreview",
    set: v => { if (v === "reading") { t.read = true; return; } t.read = false; t.src = v === "source"; },
  });
  return t;
}
const mkTab = name => modeBits({ name, read: false, src: false, hist: [{ n: name, s: 0 }], hpos: 0 });  // R8.8: LP default
const scrollOf = g => { const t = g.active >= 0 ? g.tabs[g.active] : null; return !t || t.kind ? 0 : t.mode === "reading" ? g.preview.scrollTop : isLp(t.mode) ? g.lp.scrollTop : g.editor.scrollTop; };
function histPush(g, tab, name) {     // record where the CURRENT entry was scrolled to, then push the new note
  if (tab.hist[tab.hpos]) tab.hist[tab.hpos].s = scrollOf(g);
  tab.hist = tab.hist.slice(0, tab.hpos + 1); tab.hist.push({ n: name, s: 0 }); tab.hpos++;
}

/* R13: manual linked tabs ('Link with tab...'). tab.link = link-group id
   shared by every member; tab objects move by reference (drag / split), so a
   link survives moves. Layered on the R7.3 group-level auto-link of local
   graph tabs (tab.linkId = group id), which stays as is. */
let linkSeq = 0;
function linkMembers(t) {           // every OTHER member of t's link group
  const out = [];
  if (t.link == null) return out;
  for (const h of groups()) for (const x of h.tabs) if (x !== t && x.link === t.link) out.push({ g: h, t: x });
  return out;
}
function linkTabs(a, b) {           // link two tabs; joining a member joins its group
  if (a === b) return;
  if (a.link != null && b.link != null) {
    const old = b.link;
    for (const h of groups()) for (const x of h.tabs) if (x.link === old) x.link = a.link;
  } else if (a.link != null) b.link = a.link;
  else if (b.link != null) a.link = b.link;
  else a.link = b.link = ++linkSeq;
  for (const h of groups()) renderTabs(h);
}
function isLinked(g, t, i) {        // manual member, lg auto-link, or the auto-linked group's active tab
  if (t.link != null || (t.kind === "lg" && t.linkId != null)) return true;
  return i === g.active && groups().some(h => h.tabs.some(x => x.kind === "lg" && x.linkId === g.id));
}
function unlinkTab(g, t, closing) { // R13.4: drop t from its group; a group of 1 dissolves
  if (t.link != null) {
    const rest = linkMembers(t);
    delete t.link;
    if (rest.length < 2) for (const m of rest) delete m.t.link;
  }
  if (t.kind === "lg") delete t.linkId;
  else if (!closing)                // menu on the auto-linked group's tab: detach its local graphs
    for (const h of groups()) for (const x of h.tabs) if (x.kind === "lg" && x.linkId === g.id) delete x.linkId;
  for (const h of groups()) renderTabs(h);
}
async function linkSync(src, name) { // R13.3: a member opening a note reaches every member
  for (const { g: h, t } of linkMembers(src)) {
    const act = h.tabs[h.active] === t;
    if (t.kind === "lg") {          // graph members re-center + re-title
      if (t.center === name) continue;
      t.center = name; t.name = "Graph of " + name.split("/").pop();
      h.graphSettled = false;
      renderTabs(h);
      if (act && h.graphRefresh) await h.graphRefresh();
      continue;
    }
    if (t.kind || t.name === name) continue;
    if (act) await flushSave(h);
    histPush(h, t, name);           // editors navigate in place, own mode kept
    t.name = name;
    if (act) await loadActive(h); else renderTabs(h);
  }
  updateTitle();
}

/* R9.2: left sidebar pane state — Files / Search / Bookmarks (census [pane:]) */
let sidePane = "files";
/* R9.5/R9.6: sidebar visibility — census [side:lXrX] (r wired in R9.6) */
let sideOpen = true, rightOpen = false;
function cmdToggleSide() {
  return act("pane_toggle_left", { open: !sideOpen, tabs: groups().reduce((a, g) => a + g.tabs.length, 0) }, () => {
    sideOpen = !sideOpen;
    $("side").hidden = $("ldiv").hidden = !sideOpen;
    $("collapsebtn").title = sideOpen ? "Collapse sidebar" : "Expand sidebar";
    updateTitle();
  });
}

/* R20 (#4): the right sidebar holds the list panes only (Backlinks | Outgoing
   links | Tags | Outline), like stock; the local graph is a main-area tab view
   (palette 'Open local graph', Ctrl+Shift+G, ribbon). rNote() = the focused
   group's active NOTE (graph tabs -> null: the panes keep their last note). */
function rNote() {
  const g = fg(), t = g && g.active >= 0 ? g.tabs[g.active] : null;
  return t && !t.kind ? t.name : null;
}
async function rgFollow() {                // active note changed -> panes follow
  if (!rightOpen) return;
  rPanesRefresh();
}
/* R20 (#3): #rtoggle is an in-flow flex item, never position:fixed — in #rtabs
   while the right sidebar is open (stock: the toggle lives in the sidebar's
   header), else at the END of the top-right pane's tabbar after .modebtn, so
   the two can never overlap at any width. Called after every layout change.
   [rt:x0-x1|mb:x0-x1] census (rects, read 60ms after paint = outside every
   action span) is the headless overlap probe. */
let rtInfo = "", rtT = null, rtBtn = null;
function topRight(node) { return node.children ? topRight(node.children[node.dir === "row" ? node.children.length - 1 : 0]) : node; }
function placeRToggle() {
  // The button is CACHED, never re-looked-up: while the right sidebar is closed it
  // lives in the top-right pane's tabbar, so collapseGroup's g.pane.remove() takes
  // it out of the document with its host. getElementById would then return null and
  // the b.parentNode below threw — aborting pane_close/pane_split mid-action and
  // leaving the toggle gone for good. A detached node re-mounts fine on appendChild.
  const b = rtBtn || (rtBtn = $("rtoggle"));
  if (!b) return;
  const host = rightOpen ? $("rtabs") : (state ? topRight(state.root).pane.querySelector(".tabbar") : null);
  if (host && b.parentNode !== host) host.appendChild(b);
  /* R33: the window controls are pinned to the window's TOP-RIGHT corner, which is
     the right end of exactly this row — the same end #rtoggle and .modebtn sit at.
     The row reserves their width instead of being covered by them: an invisible
     #rtoggle is a control the user has lost. */
  for (const el of document.querySelectorAll(".wfinset")) if (el !== host) el.classList.remove("wfinset");
  if (host) host.classList.add("wfinset");
  clearTimeout(rtT);
  rtT = setTimeout(() => {
    rtT = null;
    const r = b.getBoundingClientRect(), mb = host && host.classList.contains("tabbar") ? host.querySelector(".modebtn") : null;
    const m = mb ? mb.getBoundingClientRect() : null;
    const f = x => Math.round(x);
    rtInfo = "rt:" + f(r.left) + "-" + f(r.right) + (m ? "|mb:" + f(m.left) + "-" + f(m.right) : "");
    updateTitle();
  }, 60);
}
async function cmdToggleRight() {
  if (!state) return;
  await act("pane_toggle_right", { open: !rightOpen, rtab: rTab, note: cur() || "" }, () => {
    rightOpen = !rightOpen;
    $("rside").hidden = $("rdiv").hidden = !rightOpen;
    $("rtoggle").title = rightOpen ? "Collapse right sidebar" : "Expand right sidebar";
    placeRToggle();
    if (rightOpen) setRTab(rTab, false);   // pane content fills in AFTER the toggle paints (not awaited)
    updateTitle();
  });
}
$("rtoggle").onclick = cmdToggleRight;

/* rsidebar (R9.L1): icon strip Backlinks | Outgoing links | Tags | Outline.
   Active tab persisted as rside_tab in ~/.rustidian.json; census
   [side:l1r1:<tab>]. All panes follow the focused group's active note
   (rgFollow) and refresh 200ms after a save lands (rSchedule). */
const RPANES = { bl: "rpane-bl", out: "rpane-out", tags: "rpane-tags", toc: "rpane-toc" };
let rTab = "bl", rT = null, rpInfo = "";   // rpInfo -> census [rp:...]
/* F1 (lostwrite-ui) repaint probe -> census [rpn:<n>]: how many times the
   backlinks pane has STARTED a paint. The pane's CONTENT cannot prove the
   optimistic repaint was skipped after a refused write — the backend index is
   only upserted after the bytes land, so a repaint that DID run reads back the
   same bl/ul numbers and looks exactly like no repaint at all. The counter is
   the difference, and it is monotone, so a probe compares it across an action
   instead of trusting a snapshot. */
let rbN = 0;
async function setRTab(t, persist = true) {
  if (!RPANES[t]) t = "bl";
  rTab = t;
  for (const [k, id] of Object.entries(RPANES)) {
    $(id).hidden = k !== t;
    $("rtab-" + k).classList.toggle("active", k === t);
  }
  if (persist) inv("set_rside_tab", { tab: t }).catch(() => {});
  if (!rightOpen) return;
  await rPanesRefresh();
  updateTitle();
}
for (const k of Object.keys(RPANES)) $("rtab-" + k).onclick = () => setRTab(k);
function rSchedule() {                     // debounced refresh after edits
  clearTimeout(rT);
  rT = setTimeout(() => { rT = null; rPanesRefresh(); }, 200);
}
function rEmpty(box, msg) {
  const d = document.createElement("div");
  d.className = "rempty"; d.textContent = msg; box.appendChild(d);
}
const CHEV = '<svg viewBox="0 0 10 10" fill="currentColor"><path d="M3 1l4 4-4 4z"/></svg>';
async function rPanesRefresh() {
  if (!rightOpen) return;
  const n = rNote();
  if (rTab === "bl") await rBacklinks(n);
  else if (rTab === "out") await rOutgoing(n);
  else if (rTab === "toc") await rOutline(n);
  else if (rTab === "tags") await rTags();
  updateTitle();
}
// Backlinks: "Linked mentions N" + one expandable row per linking note; the
// matching lines show with the [[link]] highlighted; click opens the note
async function rBacklinks(n) {
  rbN++;                                  // [rpn:] — counted at ENTRY: the bug is calling it at all
  const box = $("bllist"), head = $("blhead");
  box.textContent = ""; head.textContent = "";
  const bl = n ? await inv("backlinks_ctx", { name: n }) : [];
  head.textContent = "Linked mentions"; rpInfo = "bl:" + bl.length;
  const c = document.createElement("span");
  c.className = "scount"; c.textContent = bl.length; head.appendChild(c);
  if (!bl.length) rEmpty(box, "No backlinks found.");
  for (const b of bl) {
    const row = document.createElement("div");
    row.className = "blnote open";
    row.innerHTML = '<span class="tc">' + CHEV + '</span><span class="bln"></span><span class="scount"></span>';
    row.querySelector(".bln").textContent = b.note;
    row.querySelector(".scount").textContent = b.lines.length;
    const kids = document.createElement("div");
    for (const [ln, text] of b.lines) {
      const d = document.createElement("div");
      d.className = "blline";
      d.title = "line " + (ln + 1);
      text.split(/(\[\[[^\]]*\]\])/).forEach((part, i) => {
        const el = i % 2 ? document.createElement("mark") : document.createTextNode(part);
        if (i % 2) el.textContent = part;
        d.appendChild(el);
      });
      d.onclick = () => navigate(fg(), b.note);
      kids.appendChild(d);
    }
    row.querySelector(".tc").onclick = e => {
      e.stopPropagation();
      row.classList.toggle("open"); kids.hidden = !row.classList.contains("open");
    };
    row.onclick = () => navigate(fg(), b.note);
    box.appendChild(row); box.appendChild(kids);
  }
  // R10.5 Unlinked mentions: plain-text hits of this note's name in other
  // notes (collapsed like stock); each row = the line with the hit marked +
  // a Link button that wraps it in [[ ]] on disk (the hit then migrates up
  // to Linked mentions on the pane's next refresh)
  const ul = n ? await inv("unlinked_mentions", { name: n }).catch(() => []) : [];
  rpInfo = "bl:" + bl.length + "|ul:" + ul.length;
  if (!n) return;
  const uh = document.createElement("div");
  uh.className = "rhead ulhead" + (ulOpen ? " open" : "");
  uh.innerHTML = '<span class="tc">' + CHEV + '</span>Unlinked mentions<span class="scount"></span>';
  uh.querySelector(".scount").textContent = ul.length;
  const ub = document.createElement("div");
  ub.hidden = !ulOpen;
  uh.onclick = () => { ulOpen = !ulOpen; uh.classList.toggle("open", ulOpen); ub.hidden = !ulOpen; };
  box.appendChild(uh); box.appendChild(ub);
  if (!ul.length) return rEmpty(ub, "No unlinked mentions found.");
  for (const m of ul) {
    const row = document.createElement("div");
    row.className = "blnote ulnote";
    row.innerHTML = '<span class="bln"></span><button class="ullink">Link</button>';
    row.querySelector(".bln").textContent = m.note;
    row.title = "line " + (m.line + 1);
    const d = document.createElement("div");
    d.className = "blline";
    const t = m.text, i = t.toLowerCase().indexOf(n.split("/").pop().toLowerCase());
    if (i >= 0) {
      d.appendChild(document.createTextNode(t.slice(0, i)));
      const mk = document.createElement("mark"); mk.textContent = t.slice(i, i + m.len); d.appendChild(mk);
      d.appendChild(document.createTextNode(t.slice(i + m.len)));
    } else d.textContent = t;
    d.onclick = () => navigate(fg(), m.note);
    row.onclick = () => navigate(fg(), m.note);
    row.querySelector(".ullink").onclick = async e => {
      e.stopPropagation();
      /* F1 (lostwrite-ui): Link REWRITES A NOTE ON DISK, so it is a write seam and
         obeys the same rule as saveBuf — a write that did not land is VISIBLE and
         nothing repaints as if it had. The catch here used to log the Err to the
         devtools console — which nobody is looking at — and then the three lines
         below ran anyway: rowSrc dropped (so an open copy
         of the note re-reads and shows the OLD text as if refreshed), the backlinks
         pane repainted the mention as LINKED, and the title republished. On a
         refused write (EROFS/ENOSPC/EACCES) that painted the link the user asked
         for and never got. saveFailed() is the existing F1 surface (#saveerr banner
         + [saveerr:<note>] census token) — reused, not re-invented. */
      try {
        await inv("link_mention", { note: m.note, target: n, line: m.line, col: m.col, len: m.len });
      } catch (err) { saveFailed(m.note, err); return; }   // no optimistic repaint on the failure path
      saveCleared();                                       // this write DID land: retire an older banner
      for (const gg of groups()) if (gg.view) gg.view.rowSrc = null;  // the linked note may be open elsewhere
      await rBacklinks(n); updateTitle();
    };
    ub.appendChild(row); ub.appendChild(d);
  }
}
// Outgoing links: resolved rows navigate, unresolved rows are greyed
async function rOutgoing(n) {
  const box = $("outlist"), head = $("outhead");
  box.textContent = ""; head.textContent = "";
  const out = n ? await inv("outgoing", { name: n }) : [];
  head.textContent = "Links"; rpInfo = "out:" + out.filter(o => o.target).length + "/" + out.filter(o => !o.target).length;
  const c = document.createElement("span");
  c.className = "scount"; c.textContent = out.length; head.appendChild(c);
  if (!out.length) return rEmpty(box, "No outgoing links.");
  for (const o of out) {
    const d = document.createElement("div");
    d.className = "outrow" + (o.target ? "" : " unresolved");
    d.textContent = o.text;
    if (o.target) d.onclick = () => navigate(fg(), o.target);
    box.appendChild(d);
  }
}
// Tags: every vault tag sorted by count desc then name; nested a/b tags fold
// into a collapsible tree (parent row count = own + kids); click a row ->
// search pane prefilled "tag:<name>"
async function rTags() {
  const box = $("taglist"), head = $("tagshead");
  box.textContent = ""; head.textContent = "";
  const counts = await inv("tag_counts").catch(() => ({}));
  const names = Object.keys(counts);
  head.textContent = "Tags"; rpInfo = "tags:" + names.length;
  const c = document.createElement("span");
  c.className = "scount"; c.textContent = names.length; head.appendChild(c);
  if (!names.length) return rEmpty(box, "No tags.");
  const root = { own: 0, kids: new Map() };
  for (const n of names) {
    let cur = root, path = "";
    for (const seg of n.split("/")) {
      path = path ? path + "/" + seg : seg;
      if (!cur.kids.has(seg)) cur.kids.set(seg, { name: path, own: 0, kids: new Map() });
      cur = cur.kids.get(seg);
    }
    cur.own = counts[n];
  }
  const total = nd => { nd.total = nd.own; for (const k of nd.kids.values()) nd.total += total(k); return nd.total; };
  total(root);
  const build = (parent, nd, depth) => {
    const list = [...nd.kids.values()].sort((a, b) => b.total - a.total || a.name.localeCompare(b.name));
    for (const k of list) {
      const row = document.createElement("div");
      row.className = "tagrow open"; row.dataset.tag = k.name; row.title = "#" + k.name;
      row.style.paddingLeft = (6 + depth * 12) + "px";
      row.innerHTML = '<span class="tc"></span><span class="tn"></span><span class="scount"></span>';
      row.querySelector(".tn").textContent = "#" + k.name.split("/").pop();
      row.querySelector(".scount").textContent = k.total;
      const kids = document.createElement("div");
      kids.className = "tockids";
      parent.appendChild(row); parent.appendChild(kids);
      build(kids, k, depth + 1);
      if (kids.childElementCount) {
        row.querySelector(".tc").innerHTML = CHEV;
        row.querySelector(".tc").onclick = e => {
          e.stopPropagation();
          row.classList.toggle("open"); kids.hidden = !row.classList.contains("open");
        };
      }
      row.onclick = () => tagSearch(k.name);
    }
  };
  build(box, root, 0);
}
function tagSearch(tag) {                  // pane row / inline pill -> search "tag:<name>"
  if (!sideOpen) cmdToggleSide();
  setPane("search");
  $("sinput").value = "tag:" + tag;
  clearTimeout(searchT); runSearch();
}
// Outline: nested collapsible heading tree; click scrolls the focused
// group's view to that heading; the heading at the viewport top is .active
let tocHeads = [], ulOpen = false;
async function rOutline(n) {
  const box = $("toclist");
  box.textContent = "";
  tocHeads = n ? await inv("outline", { name: n }) : [];
  rpInfo = "toc:" + tocHeads.length;
  if (!tocHeads.length) return rEmpty(box, "No headings.");
  // build tree: a heading's children = following headings with a deeper level
  const build = (parent, i, minLv) => {
    while (i < tocHeads.length && tocHeads[i].level > minLv) {
      const h = tocHeads[i], lv = h.level;
      const row = document.createElement("div");
      row.className = "tocrow open"; row.dataset.line = h.line;
      row.style.paddingLeft = (6 + (lv - 1) * 12) + "px";
      row.innerHTML = '<span class="tc"></span><span class="tn"></span>';
      row.querySelector(".tn").textContent = h.text || "(untitled)";
      const kids = document.createElement("div");
      kids.className = "tockids";
      parent.appendChild(row); parent.appendChild(kids);
      i = build(kids, i + 1, lv);
      if (kids.childElementCount) {
        row.querySelector(".tc").innerHTML = CHEV;
        row.querySelector(".tc").onclick = e => {
          e.stopPropagation();
          row.classList.toggle("open"); kids.hidden = !row.classList.contains("open");
        };
      }
      row.onclick = () => tocGo(h.line);
    }
    return i;
  };
  build(box, 0, 0);
  tocSync();
}
async function tocGo(line) {               // scroll + focus the heading at `line`
  const g = fg(), t = g && g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind) return;
  if (isLp(t.mode)) {                     // R12: source mode = lp with reveal
    await lpMove(g, line, 0, "heading");   // raw row = the heading, caret on it
    const row = g.lp.children[line];                // R17: one row per source line
    if (row) g.lp.scrollTop = row.offsetTop - g.lp.offsetTop;
  } else {
    const k = tocHeads.findIndex(h => h.line === line);
    const el = g.preview.querySelectorAll("h1,h2,h3,h4,h5,h6")[k];
    if (el) { g.preview.scrollTop = el.offsetTop - g.preview.offsetTop; el.setAttribute("tabindex", "-1"); el.focus(); }
  }
  tocSync();
}
function tocSync() {                       // highlight the heading at the viewport top
  if (!rightOpen || rTab !== "toc" || !tocHeads.length) return;
  const g = fg(), t = g && g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind) return;
  let top = 0;                             // first visible source line
  if (isLp(t.mode)) {
    const st = g.lp.scrollTop + 2;
    let _i = 0;
    for (const r of g.lp.children) { if (r.offsetTop - g.lp.offsetTop <= st) top = _i; _i++; }
  } else {
    const st = g.preview.scrollTop + 2, hs = g.preview.querySelectorAll("h1,h2,h3,h4,h5,h6");
    let k = -1;
    hs.forEach((el, i) => { if (el.offsetTop - g.preview.offsetTop <= st) k = i; });
    top = k >= 0 && tocHeads[k] ? tocHeads[k].line : 0;
  }
  let cur = null;
  for (const h of tocHeads) if (h.line <= top) cur = h.line; else break;
  if (cur === null) cur = tocHeads[0].line;
  for (const r of $("toclist").querySelectorAll(".tocrow"))
    r.classList.toggle("active", +r.dataset.line === cur);
  const info = "toc:" + tocHeads.length + "@" + cur;
  if (info !== rpInfo) { rpInfo = info; updateTitle(); }
}
$("main").addEventListener("scroll", tocSync, true);   // scroll doesn't bubble: capture
/* ux-4: left sidebar drag-resize (clamped 150-600, ribbon is 44px);
   width persisted as sidebar_w in ~/.rustidian.json on mouseup */
$("ldiv").onmousedown = e => {
  e.preventDefault();
  let w = 0;
  const move = ev => {
    w = Math.max(150, Math.min(600, ev.clientX - 44 + 3));
    $("side").style.width = w + "px";
  };
  const up = () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    if (w) inv("set_sidebar_w", { w }).catch(() => {});
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
};
$("rdiv").onmousedown = e => {             // resizable divider (clamped 140-600)
  e.preventDefault();
  const move = ev => {
    const w = Math.max(140, Math.min(600, window.innerWidth - ev.clientX - 3));
    $("rside").style.width = w + "px";
  };
  const up = () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
};
const SPANES = { files: "pane-files", search: "pane-search", bm: "pane-bm" };
function setPane(p) {
  sidePane = p;
  for (const [k, id] of Object.entries(SPANES)) {
    $(id).hidden = k !== p;
    $("stab-" + k).classList.toggle("active", k === p);
  }
  if (p === "search") $("sinput").focus();
  if (p === "bm") refreshBm();               // re-read: disk is the truth
  updateTitle();
}

/* R9.3 search pane: debounced rust search(query), grouped by note.
   census [sr:N] (total hits) while the search pane is showing a query. */
let searchCount = -1;                       // -1 = no query -> no [sr:] flag
let searchT = null, searchSeq = 0, searchT0 = -1, searchSp = null;   // searchSp: R18 search_type span of the latest keystroke
async function runSearch() {
  const q = $("sinput").value.trim();
  const seq = ++searchSeq;                  // stale-response guard
  const box = $("sresults");
  if (!q) {
    searchCount = -1; box.textContent = ""; updateTitle();
    otel.paint(searchSp, { hits: 0 }); searchSp = null; return;
  }
  const st0 = searchT0 >= 0 ? searchT0 : perf.now(); searchT0 = -1;
  const hits = await inv("search", { query: q });
  if (seq !== searchSeq) { otel.cancel(searchSp); return; }
  searchCount = hits.length;
  box.textContent = "";
  if (!hits.length) {
    const d = document.createElement("div");
    d.className = "sempty"; d.textContent = "No results.";
    box.appendChild(d);
  }
  const ql = q.toLowerCase();
  let curNote = null;
  for (const h of hits) {                   // hits arrive ordered by note
    if (h.note !== curNote) {
      curNote = h.note;
      const n = curNote, grp = document.createElement("div");
      grp.className = "sgroup"; grp.textContent = n;
      const c = document.createElement("span");
      c.className = "scount";
      c.textContent = "(" + hits.filter(x => x.note === n).length + ")";
      grp.appendChild(c);
      grp.onclick = () => openInTab(n);
      box.appendChild(grp);
    }
    const note = h.note, row = document.createElement("div");
    row.className = "shit"; row.title = h.snippet;
    const at = h.snippet.toLowerCase().indexOf(ql);  // highlight first hit
    if (at >= 0) {
      row.append(h.snippet.slice(0, at));
      const m = document.createElement("mark");
      m.textContent = h.snippet.slice(at, at + q.length);
      row.appendChild(m);
      row.append(h.snippet.slice(at + q.length));
    } else row.textContent = h.snippet;     // name-hit snippet may differ in case
    row.onclick = () => openInTab(note);    // LATER: jump to h.line
    box.appendChild(row);
  }
  updateTitle();
  perf.mark("search", st0, { q, hits: hits.length });
  otel.paint(searchSp, { hits: hits.length }); searchSp = null;   // R18 search_type: keystroke -> results painted
}

/* R9.4 bookmarks: tree-row context menu toggles; rust persists the plain
   list in vault/.rustidian-bookmarks. census [bm:N] while the pane shows —
   N counts the .bmrow nodes actually PAINTED in #bmlist, so an assertion on
   it fails if renderBm() stops repainting even while bmCache is correct. */
let bmCache = [];
const bmRows = () => document.querySelectorAll("#bmlist .bmrow").length;
/* R20.6: the LABELS the user can actually read, taken from the painted rows in
   paint order. A count alone passes a renderBm() that paints the right NUMBER of
   wrong rows, so [bmn:] is what proves the pane tracks disk. '|' and ']' are
   stripped so a note named with a separator cannot forge a census token. */
const bmNames = () => Array.from(document.querySelectorAll("#bmlist .bmrow"))
  .map(r => r.textContent.replace(/[|\]]/g, "")).join("|");
function renderBm() {
  const box = $("bmlist");
  box.textContent = "";
  if (!bmCache.length) {
    const d = document.createElement("div");
    d.className = "sempty"; d.textContent = "No bookmarks.";
    box.appendChild(d);
  }
  for (const nm of bmCache) {              // insertion order, like Obsidian
    const row = document.createElement("div");
    row.className = "bmrow";
    row.title = nm;
    const s = document.createElement("span");
    s.className = "bmstar";                // svg, not ★ — headless fonts lack the glyph
    s.innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M6 3h12v18l-6-4.5L6 21z"/></svg>';
    row.appendChild(s);
    row.append(nm.split("/").pop());
    row.onclick = () => openInTab(nm);
    box.appendChild(row);
  }
  updateTitle();
}
async function refreshBm() { bmCache = await inv("list_bookmarks"); renderBm(); }
async function toggleBm(nm) {
  bmCache = await inv("toggle_bookmark", { name: nm });
  renderBm();
}
function noteMenu(e, nm) {                 // right-click a tree note row
  e.preventDefault();
  e.stopPropagation();
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  const d = document.createElement("div");
  d.textContent = bmCache.includes(nm) ? "Remove bookmark" : "Bookmark";
  d.onmousedown = ev => ev.stopPropagation();
  d.onclick = () => { closeMenu(); toggleBm(nm); };
  m.appendChild(d);
  placeMenu(m, e.clientX, e.clientY);   /* R22: viewport-clamped by measured size */
}

// R23 (feedback #15) built this helper because the four creation paths
// disagreed about the new note's body: cmdNewNote seeded an H1 of the note's
// own name inline, while the three INDIRECT paths (an unresolved wikilink
// clicked in live preview, the same in reading view, a ghost node clicked in
// the graph) wrote "". ONE shared seam was the fix, not four edits — and it is
// why feedback #20 below is a one-line change instead of four.
// SUPERSEDED BY feedback #20: the seeded heading is gone (see the block below);
// what R23 still buys is that every path funnels through here.
/* F4 (dataloss-audit): creation must never replace an existing note with a
   stub. The backend uses create_new(2) — the kernel's atomic exists-check —
   so unlike a JS-side notesCache test there is no window for another writer
   (git checkout, sync client, the 1000ms-stale index) to land a real file
   between check and truncate. "exists" is not an error here: every creation
   path means "take me to Foo", so the caller opens the existing note (stock
   behaviour); nothing is overwritten either way. -> "ok" | "exists" | "err" */
/* feedback #20: creation materializes NOTHING. Stock's brand-new note is a
   ZERO-BYTE file (recon Q1: Ctrl+N -> Untitled.md, wc -c = 0); the big title
   the user sees is the INLINE TITLE — a render of the FILENAME (mkInlineTitle)
   that lives in no file. This is the single seam all four creation paths share,
   so the default body is "" here and nowhere else; the rust command defaults an
   absent `content` to "" too, so neither side can re-mint a heading alone. */
async function createNote(name, content) {
  const body = content != null ? content : "";
  try { await inv("create_note", { name, content: body }); }
  catch (e) {
    if (errStr(e) === "exists") return "exists";
    saveFailed(name, e);
    return "err";
  }
  await afterWrite(name);
  return "ok";
}

async function writeNote(name, content) {   // every save funnels here so graphs live-update
  const t0 = perf.now();
  await inv("write_note", { name, content });   // F1: REJECTS if the bytes did not land
  await afterWrite(name);
  perf.mark("save", t0, { note: name, bytes: content.length });
}
async function afterWrite(name) {           // bookkeeping shared by write + create
  markStale(name);                          // R20: inactive tabs on this note re-read on activation
  if (!notesCache.includes(name)) await refreshTree();   // R20: a NEW note is the only save that changes the tree
  for (const g of groups()) if (g.graphOn && g.graphRefresh) await g.graphRefresh();
  if (rightOpen) rSchedule();               // rsidebar: list panes refresh (200ms)
}

/* F1 (dataloss-audit): a save that did not land must be VISIBLE and must not
   mark the buffer clean. write_note now rejects on ENOSPC/EROFS/EACCES, and
   saveBuf() — the single write seam for the active note under R11.3 — surfaces
   the failure (banner + census [saveerr:<note>]) and returns false, so the tab
   keeps its base and stays dirty: the next debounce / ctrl+s retries the same
   bytes. Swallowing it costs every edit of a whole session with zero signal. */
let saveErr = "";
/* R31.9 last drop outcome, declared up here with saveErr so updateTitle (below)
   can never read it through a temporal-dead-zone. The drop code itself is in
   the R31 section further down. */
let dropTok = "", dropT = null;
/* F2 test hook: the vault-switch race lives inside the debounce window, so it
   is not mechanically reproducible at 250ms — RUSTIDIAN_SAVE_MS widens it for
   the smoke (backend save_debounce_ms; default 250 in every normal run). */
let SAVE_MS = 250;
const errStr = e => String((e && e.message) || e);
function saveFailed(name, e) {
  saveErr = String(name).replace(/[\[\]|]/g, "").slice(0, 60);
  const b = $("saveerr");
  b.textContent = "Save failed — " + name + ": " + errStr(e).slice(0, 120);
  b.hidden = false;
  updateTitle();
}
/* the banner is persistent by design (F1), so exactly one thing retires it: a
   write to the vault that DID land. Was inline in saveNote; named because the
   Link seam (rBacklinks) needs the same three lines and a second copy of them
   is how the two surfaces drift apart. */
function saveCleared() {
  if (!saveErr) return;
  saveErr = ""; $("saveerr").hidden = true; updateTitle();
}
async function saveNote(name, content) {    // true == the bytes are on disk
  try { await writeNote(name, content); }
  catch (e) { saveFailed(name, e); return false; }
  saveCleared();
  return true;
}

/* F2 (dataloss-audit): a timer armed in vault A fires AFTER the swap and
   writes A's buffer into B's same-named note — two notes damaged by one
   action, and A's own edits never reach A. Every path that leaves a vault
   flushes first and then clears the handle UNCONDITIONALLY: losing a 250ms
   burst is strictly better than writing it into the wrong vault. */
async function leaveVault() {
  if (!state) return;
  for (const h of groups()) {
    try { await flushSave(h); }
    finally { clearTimeout(h.saveT); h.saveT = null; }
  }
}

async function flushSave(g) {               // write g's pending edits NOW
  if (!g) return;
  lpCommit(g);                              // fold any active lp raw row first
  if (!g.saveT) return;
  clearTimeout(g.saveT); g.saveT = null;
  await saveBuf(g);                         // R11.3 merge-before-write
}

/* ---------- group DOM + layout render ---------- */
function mkGroup() {
  const g = { id: gidSeq++, tabs: [], active: -1,
              graphOn: false, graphRefresh: null, sim: null, saveT: null,
              lpActive: null, view: null };
  const pane = document.createElement("div");
  pane.className = "pane";
  pane._g = g;                                   // R20: gOf(el) — event-time group lookup
  pane.innerHTML =
    '<div class="tabbar"><div class="tabs"></div>' +
    '<button class="modebtn" title="toggle reading view (Ctrl+E)"></button></div>' +
    '<div class="content">' +
      '<canvas class="graph" hidden></canvas>' +
      '<div class="ac" hidden></div>' +
      '<div class="status" hidden><span class="st-bl"></span><span class="st-wc"></span><span class="st-cc"></span></div>' +
      '<button class="lggear" title="local graph settings" hidden>' +
        '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><circle cx="8" cy="8" r="2.2"/><path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4"/></svg></button>' +
      '<div class="lgpop" hidden>' +
        '<label>Depth <span class="lgdv">1</span></label>' +
        '<input class="lgdepth" type="range" min="1" max="5" step="1" value="1">' +
        '<label><input class="lginc" type="checkbox" checked> Incoming links</label>' +
        '<label><input class="lgout" type="checkbox" checked> Outgoing links</label>' +
      '</div>' +
    '</div>';
  g.pane = pane;
  const q = s => pane.querySelector(s);
  g.tabsEl = q(".tabs"); g.modebtn = q(".modebtn"); g.content = q(".content");
  g.graph = q(".graph");
  g.acEl = q(".ac"); g.status = q(".status");
  g.stBl = q(".st-bl"); g.stWc = q(".st-wc"); g.stCc = q(".st-cc");
  g.lggear = q(".lggear"); g.lgpop = q(".lgpop"); g.lgDv = q(".lgdv");
  g.lgDepth = q(".lgdepth"); g.lgInc = q(".lginc"); g.lgOut = q(".lgout");
  g.lggear.onclick = () => { g.lgpop.hidden = !g.lgpop.hidden; };
  g.lgDepth.oninput = () => lgSet(g);
  g.lgInc.onchange = () => lgSet(g);
  g.lgOut.onchange = () => lgSet(g);
  pane.addEventListener("mousedown", () => focusGroup(g), true);  // R6.3: click focuses
  g.modebtn.onclick = () => cmdToggleMode(g);
  attachView(g, mkView(g));                      // scratch view until the first tab adopts it
  return g;
}

/* R20 (#d) per-tab VIEW retention (stock keeps every leaf's view alive):
   view = { editor (hidden model textarea), lp, preview, lp* state, loaded }.
   Every note tab owns one; all of a group's views live in .content, the
   active one shown, the rest display:none. Switching tabs = attachView
   (re-point g.editor/g.lp/g.preview + swap display) — no read, no render,
   no caret re-find. A view re-renders only when its file changed (t.stale:
   watcher / another tab saved it).
   Tab objects move by reference between groups; their view nodes follow
   (attachView re-parents) and handlers resolve the group via gOf(). */
function mkView(g) {
  const v = { g, tab: null, loaded: false, scrollTop: 0,
              lines: null, rowSrc: null, undo: [], redo: [], lpActive: null, lpLines: 0 };
  const ed = document.createElement("textarea");
  ed.className = "editor"; ed.spellcheck = false; ed.style.display = "none";
  ed.placeholder = "# write markdown, link with [[Note]]";
  const lp = document.createElement("div"); lp.className = "lp";
  const pv = document.createElement("div"); pv.className = "preview";
  v.editor = ed; v.lp = lp; v.preview = pv;
  v.lines = [""];                                 // R17: the MODEL (source of truth)
  Ed.mount(v);                                    // R17: lp is the contenteditable view
  // R17: click below the last row (empty pane space) = caret at the end of
  // the note, like stock; rows themselves get the native caret placement
  lp.addEventListener("mousedown", e => {
    const g = v.g;
    if (e.target !== lp || g.graphOn) return;
    // R34.15: the ::before is not an event target — a click on the title's ink
    // reports .lp itself, so the title band is identified by GEOMETRY. Inside
    // it the click opens the rename surface (R34.1); anywhere else below the
    // last row keeps R17's "caret at the end of the note".
    if (titleHit(lp, e.clientX, e.clientY)) { e.preventDefault(); openTitleEdit(g, lp, e.clientX, e.clientY); return; }
    e.preventDefault();
    const L = Ed.lines(g);
    Ed.place(g, L.length - 1, L[L.length - 1].length);
  });
  return v;
}
function attachView(g, v) {          // v becomes g's live editor/lp/preview
  if (g.view === v) return;
  const old = g.view;
  if (old && old.g === g) {          // skip when old already moved to another group (tab drop)
    Object.assign(old, { lpActive: g.lpActive, lpLines: g.lpLines, scrollTop: old.lp.scrollTop });
    old.lp.style.display = old.preview.style.display = "none";
  }
  v.g = g;
  if (v.editor.parentNode !== g.content) g.content.prepend(v.editor, v.lp, v.preview);
  g.view = v; g.editor = v.editor; g.lp = v.lp; g.preview = v.preview;
  g.lpActive = v.lpActive; g.lpLines = v.lpLines;
}
function viewOf(g, t) {              // the tab's view; a tab-less scratch view is adopted
  if (t.view) return t.view;
  const v = g.view && !g.view.tab ? g.view : mkView(g);
  v.tab = t; t.view = v;
  return v;
}
function dropView(t) {               // tab closed: its nodes go
  const v = t.view; if (!v) return;
  v.editor.remove(); v.lp.remove(); v.preview.remove();
  if (v.g && v.g.view === v) attachView(v.g, mkView(v.g));
  t.view = null;
}
function markStale(name) {           // a save of `name` -> every INACTIVE retained view of it re-reads on activation
  for (const h of groups()) for (const t of h.tabs)
    if (!t.kind && t.name === name && h.tabs[h.active] !== t && t.view && t.view.loaded) t.stale = true;
}

function layoutEl(node) {         // split tree -> DOM; flex weights from fractions
  if (!node.children) return node.pane;
  const d = document.createElement("div");
  d.className = "split " + node.dir;
  node.el = d;
  node.children.forEach((c, i) => {
    if (i > 0) d.appendChild(divider(node));
    d.appendChild(layoutEl(c));
  });
  applyFlex(node);
  return d;
}
const elOf = node => (node.children ? node.el : node.pane);
function applyFlex(node) {        // node.fractions -> child flex weights (in place, no rebuild)
  node.children.forEach((c, i) => { elOf(c).style.flex = ((node.fractions && node.fractions[i]) || 1) + " 1 0"; });
}

// R6.6: draggable divider between split siblings — drag re-weights
// node.fractions (each side floored at 15% of the split), flex updated live.
// R20: siblings are resolved at mousedown from the DOM (the split's children
// are inserted/removed in place now, so a captured index would go stale).
function divider(node) {
  const h = document.createElement("div");
  h.className = "divider " + node.dir;
  h.addEventListener("mousedown", e => {
    e.preventDefault();
    e.stopPropagation();               // don't let mousedown-to-focus swallow it
    const box = node.el, row = node.dir === "row";
    const i = [...box.children].filter(c => c.classList.contains("divider")).indexOf(h);
    const els = [elOf(node.children[i]), elOf(node.children[i + 1])];
    if (!node.fractions) node.fractions = node.children.map(() => 1);
    const total = node.fractions.reduce((a, b) => a + b, 0);
    const r = box.getBoundingClientRect();
    const size = row ? r.width : r.height;
    const f0 = node.fractions[i], f1 = node.fractions[i + 1];
    const p0 = row ? e.clientX : e.clientY;
    const min = 0.15 * total;
    const move = ev => {
      if (f0 + f1 < 2 * min || size <= 0) return;
      const df = ((row ? ev.clientX : ev.clientY) - p0) / size * total;
      const a = Math.min(Math.max(f0 + df, min), f0 + f1 - min);
      node.fractions[i] = a;
      node.fractions[i + 1] = f0 + f1 - a;
      els[0].style.flex = a + " 1 0";
      els[1].style.flex = (f0 + f1 - a) + " 1 0";
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
      updateTitle();                   // republish [fx:...] census for probes
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  });
  return h;
}

function renderLayout() {         // boot / vault switch only — every later change edits the DOM in place (R20)
  const main = $("main");
  main.innerHTML = "";
  main.appendChild(layoutEl(state.root));
  placeRToggle();
  updateTitle();
}

/* ---------- R22 layout census: [ovf:<dw>,<dh>,<n>] ----------
   R22 (no scrollbars, ever): at EVERY window size the app chrome fits
   exactly. dw/dh = documentElement.scrollWidth-clientWidth /
   scrollHeight-clientHeight — the WINDOW must never scroll, so both are 0.
   n = elements in the app FRAME that spill: content wider/taller than their
   own box while that box neither clips nor scrolls it, or a box that reaches
   past the viewport edge. OVF_SCROLL lists the containers that scroll BY
   DESIGN (note body, file tree, result/link lists, picker + modal lists):
   they are not counted, and the walk does not descend into them — their
   contents are content, not chrome, and cost is bounded to the frame.
   When n > 0 the census also carries [ovfe:<el>@<why>|...] so a fuzz failure
   names the element instead of just a number. Only LEAF offenders are named:
   an overflowing child makes every ancestor's scrollWidth overflow too, and
   reporting `#main` when the real culprit is a button in the tab strip costs
   an iteration every time. */
const OVF_SCROLL = "#tree,#sresults,#bmlist,.rlist,.lp,.preview,.editor,.ac," +
                   "#mlist,#p-dirs,#p-recent,#sbody,#snav,#spage,#hklist";
function ovfName(el) {              // short, stable selector for a failure message
  const c = (el.className && el.className.baseVal !== undefined ? el.className.baseVal : el.className) || "";
  const k = String(c).trim().split(/\s+/).filter(Boolean).slice(0, 2).map(x => "." + x).join("");
  return el.tagName.toLowerCase() + (el.id ? "#" + el.id : "") + k;
}
function ovfCause(el, why) {         // "<child" = the child that makes el overflow (leaf case only)
  const horiz = why[1] === "w" || why[0] === "r" || why[0] === "l";
  const box = el.getBoundingClientRect();
  let worst = null, worstBy = 1;
  for (const c of el.children) {
    if (c.hidden) continue;
    const r = c.getBoundingClientRect();
    if (!r.width && !r.height) continue;
    const by = horiz ? Math.max(r.right - box.right, box.left - r.left)
                     : Math.max(r.bottom - box.bottom, box.top - r.top);
    if (by > worstBy) { worstBy = by; worst = c; }
  }
  return worst ? "<" + ovfName(worst) + "+" + Math.round(worstBy) : "";
}
/* R33.13 — the window-control strip reserves 140px at the right end of the row
   it covers (.wfinset), and the four rside tabs are `flex: 1`, so EVERY ONE OF
   THEM MOVED when the strip shipped (measured at 1100x700, 260px sidebar:
   868/926/981/1036 -> the numbers this token now reports). A test that keeps
   clicking the old x hits whatever is there now — the chrome phase's 1082 landed
   on the new CLOSE button and shut the app down mid-phase, which the harness
   reported as "APP DIED ... OOM-kill is the usual cause". So the app publishes
   where they actually are and smoke clicks THAT: re-measured, not suppressed,
   and it cannot rot the next time something shifts the row. */
function stabCentres() {
  return [...document.querySelectorAll("#rtabs .stab")]
    .map(b => { const r = b.getBoundingClientRect(); return Math.round(r.left + r.width / 2); })
    .join(",");
}
function ovfScan() {
  const de = document.documentElement;
  const vw = de.clientWidth, vh = de.clientHeight;
  const bad = [];
  let n = 0;
  const walk = el => {                 // returns how many offenders live BELOW el
    let deep = 0;
    for (const c of el.children) {
      if (c.hidden || c.tagName === "SCRIPT" || c.tagName === "STYLE") continue;
      const r = c.getBoundingClientRect();
      if (!r.width && !r.height) continue;                 // display:none / not laid out
      if (c.matches(OVF_SCROLL)) continue;                 // allowed scroll container: skip it AND its subtree
      const st = getComputedStyle(c);
      const sw = c.scrollWidth - c.clientWidth, sh = c.scrollHeight - c.clientHeight;
      let why = "";
      if (sw > 1 && st.overflowX === "visible") why = "sw+" + sw;
      else if (sh > 1 && st.overflowY === "visible") why = "sh+" + sh;
      else if (r.right > vw + 1) why = "r" + Math.round(r.right) + ">" + vw;
      else if (r.bottom > vh + 1) why = "b" + Math.round(r.bottom) + ">" + vh;
      else if (r.left < -1) why = "l" + Math.round(r.left);
      else if (r.top < -1) why = "t" + Math.round(r.top);
      const sub = walk(c);
      if (why) {
        n++;
        // a LEAF offender is overflowed by a child the walk does not check
        // (an allowed scroll container, or an absolutely-positioned box):
        // name the widest/tallest such child so the cause is not a mystery
        if (!sub && bad.length < 4) bad.push(ovfName(c) + "@" + why + ovfCause(c, why));
      }
      deep += sub + (why ? 1 : 0);
    }
    return deep;
  };
  walk(document.body);
  return { dw: de.scrollWidth - vw, dh: de.scrollHeight - vh, n, bad };
}
let ovfT = 0;
addEventListener("resize", () => {                 // the census must follow the window, not only UI state
  requestAnimationFrame(updateTitle);
  clearTimeout(ovfT); ovfT = setTimeout(updateTitle, 150);   // after the relayout settles
});

function updateTitle() {          // pane/focus census in the window title (headless probe)
  const ps = [...document.querySelectorAll("#main .pane")];
  const nf = document.querySelectorAll("#main .pane.focused").length;
  const fx = (state.root.fractions || []).map(f => f.toFixed(2)).join(",");
  let lg = "";                    // M8: first localgraph tab -> [lg:<center>@<depth>]
  // graph-parity: settled node screen coords (<= 32 nodes; the fast-mode smoke vault is 17 incl. ghosts) -> [lgpos:A@x,y|B@x,y]
  // for the first localgraph group (+ [lgt:<tab title>]), [ggpos:...] for a
  // focused global graph;
  // [lgl:<note>] = the lg tab's LINKED group's active note; [chain:N] = tabs
  // carrying the link glyph. Headless probe for the graphnav smoke.
  const posTok = h => {
    if (!h.graphOn || !h.graphSettled || !h.graphNodes) return "";
    const ns = h.graphNodes();
    return ns.length > 32 ? "" : ns.map(p => p.n + "@" + p.x + "," + p.y).join("|");
  };
  for (const h of groups()) {
    const t = h.tabs.find(t => t.kind === "lg");
    if (t) {
      lg = " [lg:" + t.center + "@" + t.depth + "] [lgt:" + t.name + "]";
      const lk = groups().find(x => x.id === t.linkId);
      if (lk && curOf(lk)) lg += " [lgl:" + curOf(lk) + "]";
      const pt = h.tabs[h.active] === t ? posTok(h) : "";
      if (pt) lg += " [lgpos:" + pt + "]";
      // C5: the last re-centre's continuity record (see rcRecord) — first-frame evidence that
      // the survivors kept their place and their motion, that the new node was seeded beside
      // a placed neighbour, that the restart was warm and the neighbourhood was prefetched.
      const rc = h.tabs[h.active] === t && h.graphRc ? h.graphRc() : "";
      if (rc) lg += " [lgrc:" + rc + "]";
      break;
    }
  }
  {
    let chain = 0; const lk = [];  // R13: [lk:a|b] = names of manually linked tabs
    for (const h of groups()) h.tabs.forEach((t, i) => {
      if (isLinked(h, t, i)) chain++;
      if (t.link != null) lk.push(t.name.split("/").pop());
    });
    if (chain) lg += " [chain:" + chain + "]";
    if (lk.length) lg += " [lk:" + lk.join("|") + "]";
  }
  // R8.10: focused tab's view mode -> [mode:lp|src|read]; when the lp raw
  // row is active, [mode:lp:<l0>] exposes its block start line (headless probe)
  const ft = fg() && fg().active >= 0 ? fg().tabs[fg().active] : null;
  let md = ft && !ft.kind ? " [mode:" + (MODE_ABBR[ft.mode] || "?") : "";
  if (md && isLp(ft.mode) && fg().lpActive) md += ":" + fg().lpActive.l0;
  if (md) md += "]";
  if (lpMs >= 0) md += " [lp:" + lpMs + "]";     // perf: last lpRender ms
  if (md) md += mswTok();                        // R35 perf: the mode switch's own cost (see mswEnd)
  /* R34.15 title probe -> [te:<text in the box>/<g.lp.children.length>]. Two
     facts in one token, and the second one is the R32.4 acceptance condition
     made observable: while the title is being edited the scroller's child list
     must be EXACTLY what it is when it is not (the caret surface is a sibling,
     ui/style.css .titlewrap). A phase compares the row count in this token with
     the count from Ed.geom before the click; a mechanism that prepended a node
     would read one higher here and break every positional index silently. */
  if (titling) md += " [te:" + titling.el.textContent.replace(/[[\]|/]/g, "") +
                     "/" + (titling.g.lp ? titling.g.lp.children.length : -1) + "]";
  /* R34.18 — the two surfaces, MEASURED, not claimed (see tpType above).
     [tty:<rendered>~<editing>], each side
       font-size/weight/letter-spacing/line-height/colour/family-hash/family-head
     read with getComputedStyle from the live DOM: the ::before that paints the
     rendered title, and the .titleedit that stands in for it while renaming.
     [lpc:] is the scroller's child-list signature (R34.15), published open AND
     closed so the phase can diff the two instead of trusting the comment.
     [tpf:] is --font-text-size as the document actually resolves it, which is
     what makes the size-independence re-run readable in the log. */
  if (typeProbe && md) {
    if (titling) md += " [tty:" + tpType(getComputedStyle(titling.host, "::before")) +
                       "~" + tpType(getComputedStyle(titling.el)) + "]";
    if (fg() && fg().lp) md += " [lpc:" + tpKids(fg().lp) + "]";
    md += " [tpf:" + getComputedStyle(document.documentElement).getPropertyValue("--font-text-size").trim() +
          "/" + (tpPerturb || "-") + "]";
  }
  // R17: renderer/token-map self test. A failure names its FIRST bad case
  // (Ed.edtWhy) so the smoke log says what broke, not just how many.
  if (edtBad >= 0) md += " [edt:" + (edtBad ? "fail" + edtBad + ":" + (Ed.edtWhy || "?") : "ok") + "]";
  // R17 editor probe: the MODEL selection range [sel:l.c-l.c], the R17.6/R17.7
  // REVEAL SET [rv:<l>.<s>-<e>:<kind>,...] (which tokens are showing their raw
  // markers — "-" for none, "src" in source mode) and the measured row geometry
  // [edx:] [ery:] of the focused pane — the `edit` smoke asserts selections,
  // reveal scope and clicks on source lines instead of guessed pixels.
  if (md && ft && !ft.kind && isLp(ft.mode) && typeof Ed !== "undefined" && fg() && fg().lp)
    md += Ed.selTok(fg()) + Ed.rvTok(fg()) + Ed.cmTok() + Ed.geom(fg());
  // R17.7 link probe: the FIRST rendered link of the focused note view -> [xl:<dest>|<visible line>].
  // BOTH renderers publish it (live preview/source from g.lp, reading from g.preview, i.e. Rust's
  // pulldown-cmark HTML), so the smoke can assert they AGREE on a destination containing balanced
  // parens — and that no leftover ')' is sitting there as text — from the DOM instead of OCR-ing a
  // paren. dest = the href routing would use for an a.ext, else the parsed destination. innerText,
  // not textContent: lp hides the link markup off the caret row and hidden markup is not rendered text.
  if (md && ft && !ft.kind && fg()) {
    const rt = ft.mode === "reading" ? fg().preview : fg().lp;
    const la = rt && rt.querySelector("a.ext, a.lt");
    if (la) {
      const host = la.closest(".lprow, p, li, h1, h2, h3, h4, h5, h6") || la.parentNode;
      const xs = s => String(s == null ? "" : s).replace(/[[\]|]/g, "").trim().slice(0, 120);
      md += " [xl:" + xs(la.classList.contains("ext") ? la.getAttribute("href") : la.dataset.url) +
            "|" + xs(host.innerText) + "]";
    }
  }
  // R29 image probe: the focused note view's images -> [xi:<n>/<loaded>/<miss>|<src>|<src>].
  // TRAP: an <img> in the DOM is NOT evidence that any byte arrived — a CSP-blocked,
  // out-of-vault or 404 image is still an element with the src you asked for. `loaded`
  // counts naturalWidth > 0, which ONLY a decoded image has, so the census can tell
  // "rendered" from "requested". <miss> is the R29.4 banner count (span.imgmiss), which is
  // the defined answer for a target that resolves to nothing — it must never be an <img>.
  // BOTH renderers publish this from their OWN DOM (reading = Rust's pulldown-cmark HTML,
  // lp = the R17 JS engine), so the smoke asserts they agree on the RESOLVED src byte for
  // byte (getAttribute, not .src: the property is absolutised and would hide a disagreement).
  if (md && ft && !ft.kind && fg()) {
    const rv = ft.mode === "reading" ? fg().preview : fg().lp;
    const ims = rv ? [...rv.querySelectorAll("img")] : [];
    const nmiss = rv ? rv.querySelectorAll(".imgmiss").length : 0;
    if (ims.length || nmiss) {
      // the title only MOVES in updateTitle (see noteErr's note), and an image decodes
      // asynchronously — so at render time every naturalWidth is 0 and the census would
      // sit there saying "0 loaded" forever, which a poll cannot tell from "blocked".
      // Re-publish once per element when its bytes land (or fail): one-shot, so this can
      // never become a title loop.
      for (const i of ims) {
        if (i.dataset.xiw || i.complete) continue;
        i.dataset.xiw = "1";
        const again = () => requestAnimationFrame(updateTitle);
        i.addEventListener("load", again, { once: true });
        i.addEventListener("error", again, { once: true });
      }
      const xi = s => String(s == null ? "" : s).replace(/[[\]|]/g, "").slice(0, 100);
      md += " [xi:" + ims.length + "/" + ims.filter(i => i.naturalWidth > 0).length + "/" + nmiss +
            ims.slice(0, 3).map(i => "|" + xi(i.getAttribute("src"))).join("") + "]";
    }
  }
  // R31.9 drop probe: the LAST drop's outcome -> [drop:<copied>/<refused>].
  // Deliberately not derived from the banner (which times out): "no drop yet"
  // and "a drop whose banner faded" must not look the same to a probe.
  if (dropTok) md += " [drop:" + dropTok.replace(/[[\]|]/g, "") + "]";
  md += " [zoom:" + zoomTok + "]";   // R36: always present — a probe must be able to read "still at 100%"
  // R15.2 font probe: bundled @font-face entries that actually LOADED (lazy: a face loads when text first uses it) -> [fonts:SourceCodePro/400/normal|...]
  { const fl = document.fonts ? [...document.fonts].filter(f => f.status === "loaded").map(f => f.family.replace(/[" ]/g, "") + "/" + f.weight + "/" + f.style) : [];
    if (fl.length) md += " [fonts:" + fl.join("|") + "]"; }
  let gg = ft && ft.kind === "gg" ? " [gg]" : "";  // R9.7: global graph tab focused
  if (gg) { const pt = posTok(fg()); if (pt) gg += " [ggpos:" + pt + "]"; }
  const tokq = s => String(s == null ? "" : s).replace(/[[\]|]/g, "").slice(0, 80);
  const modal = modalKind ? " [modal:" + modalKind + "]" +
                            (mdNew ? " [mdnew:" + tokq(mdNew) + "]" : "")   // C4: the create-this-note affordance is on screen
    : ($("rnbox") && !$("rnbox").hidden ? " [modal:rn]" : "")  // m5 fuzzy modal / rename prompt
    + ($("anew") && !$("anew").hidden ? " [modal:att]" : "")   // R31.7 Insert attachment prompt
    + (settingsOpen ? " [modal:settings]" + setTok() + hkInfo : "")   // R14 hotkeys + R30 settings probe
    + (ulPending ? " [modal:ul]" + ulTok() : "")                // R34.6 Update links prompt
    + (noticeTxt ? " [notice:" + tokq(noticeTxt) + "]" : "")    // R34.12/13 the last refusal — does NOT expire with the banner
    + (menuEl ? " [menu:1]" : "");                             // R22: a context menu is open (fuzz probe)
  // [note:<name>] = the FOCUSED group's active note (null for a graph tab).
  // Which note is active was previously only observable by mutating it (type a
  // marker, grep the disk) or by renaming it — both destroy the thing under
  // test, which is useless for a data-loss assertion.
  const anote = state && fg() ? curOf(fg()) : null;
  const noteTok = anote ? " [note:" + tokq(anote) + "]" : "";
  // THEME census: the mode read back OFF THE DOM, not off themeMode — a probe
  // must not be able to pass because a variable says "light" while the root
  // attribute (the thing that actually paints) was never written. Pixels remain
  // the assertion of record in the phase; this token is how it knows WHICH
  // theme the pixels it just sampled are supposed to be.
  const themeTok = " [theme:" + (document.documentElement.getAttribute("data-theme") || "unset") + "]";
  // PALETTE census, read off the DOM for the same reason: "default" here means
  // the attribute is ABSENT, which is the state the default palette IS. A probe
  // cannot pass by setting a variable.
  const palTok = " [palette:" + (document.documentElement.getAttribute("data-palette") || "default") + "]";
  let t = "rustidian [panes:" + ps.length + " focused:" + nf +
            "@" + (ps.indexOf(fg() && fg().pane) + 1) + "] [fx:" + fx + "]" +
            " [tabs:" + groups().map(g => g.tabs.length).join(",") + "]" + noteTok + themeTok + palTok + lg + md + gg + modal +
            " [side:l" + (sideOpen ? 1 : 0) + "r" + (rightOpen ? 1 : 0) +
            (rightOpen ? ":" + rTab : "") + "]" +
            (rightOpen && rpInfo ? " [rp:" + rpInfo + "]" : "") +
            " [rpn:" + rbN + "]" +                              // F1: backlink repaints ENTERED, so a skipped one is observable
            (rtInfo ? " [" + rtInfo + "]" : "") +
            (rightOpen ? " [stx:" + stabCentres() + "]" : "") +   // R33.13: the strip MOVED these — smoke reads them, never guesses
            (jsErr ? " [jserr:" + jsErr + "]" : "") +
            (saveErr ? " [saveerr:" + saveErr + "]" : "") +             // F1: a save that did not land
            " [armed:" + groups().filter(h => h.saveT).length + "]" +   // F2: groups holding a live save timer
            menuTok() +
            (navInfo ? " [" + navInfo + "]" : "") +
            (acItems.length ? " [ac:" + acKind + ":" + acItems.length + "]" : "") +
            " [pane:" + sidePane + "]" +
            (sidePane === "search" && searchCount >= 0 ? " [sr:" + searchCount + "]" : "") +
            (sidePane === "bm" ? " [bm:" + bmRows() + "]" +          // RENDERED rows, not bmCache.length:
              " [bmn:" + bmNames() + "]" +                          // and their painted LABELS, in paint order
              (bmRows() === bmCache.length ? "" :                    // the smoke assertion must prove the PANE
               " [bmdesync:" + bmCache.length + "/" + bmRows() + "]") : "");   // repainted, not just the model

  // R17: ONE model->text join per title publish, shared by [buf:] and F1's
  // [dirty:] — the token must not put a second full join on the typing path.
  const fb = fg() && fg().active >= 0 && !fg().tabs[fg().active].kind ? bufOf(fg()) : null;
  const t2 = (fb !== null ? " [buf:" + fb.length + "]" + dirtyTok(fb) : "") +
             " [tree:" + notesCache.length + "] [vc:" + vcCount + "]" +   // R11 probes
             (extCount ? " [ext:" + extCount + "]" : "");                 // S1: external-link clicks routed to open_external
  t += t2;
  const ov = ovfScan();            // R22: layout overflow census (window + frame)
  t += " [vp:" + innerWidth + "x" + innerHeight + "]" +   // resize-completed signal for the fuzz harness
       " [ovf:" + ov.dw + "," + ov.dh + "," + ov.n + "]" +
       (ov.bad.length ? " [ovfe:" + ov.bad.join("|").slice(0, 180) + "]" : "");
  t += wfTok();                    // R33: the window's own frame (controls, grips, maximised, keyboard focus)
  document.title = t;
  // publish to the native title: ONE call in flight, last-write-wins, 500ms
  // timeout guard — a hung/rejected setTitle IPC can neither reorder titles
  // nor starve later updates (the old promise-chain stalled forever on one)
  pushTitle(t);
}
let tSending = false, tWant = "";
function pushTitle(t) {
  tWant = t;
  if (tSending) return;
  tSending = true;
  const cur = tWant;
  let done = false;
  const fin = () => {
    if (done) return; done = true;
    tSending = false;
    if (tWant !== cur) pushTitle(tWant);
  };
  try {
    window.__TAURI__.window.getCurrentWindow().setTitle(cur).catch(() => {}).then(fin);
  } catch (e) { fin(); }
  setTimeout(fin, 500);
}

function focusGroup(g) {
  const prev = state.focused;
  if (prev === g) return;
  state.focused = g;
  for (const x of groups()) x.pane.classList.toggle("focused", x === g);
  rgFollow();                     // R9.6: right panel follows focus (fire+forget)
  updateTitle();
  if (prev) treeHighlight();      // explorer active-note highlight follows focus
}

/* ---------- split verbs (M7 / R6.2): tab context menu + tree mutation ---------- */
function findParent(node, target, parent = null) {
  if (node === target) return parent;
  if (!node.children) return null;
  for (const c of node.children) {
    const p = findParent(c, target, node);
    if (p) return p;
  }
  return null;
}

async function splitGroup(g, dir, ti) {  // duplicate g's tab ti into a new sibling group
  const src = g.tabs[ti];
  const t = src ? Object.assign(mkTab(src.name), { src: !!src.src, mode: src.mode }) : null;   // #16: BOTH bits ride along (sub-mode survives a split of a reading tab)
  await splitWith(g, dir, t);
}

async function splitWith(g, dir, tab) {  // insert a new sibling group carrying `tab`
  const parent = findParent(state.root, g);
  if (!parent) return;
  await act("pane_split", { dir, groups: groups().length + 1, note: tab ? tab.name : "" }, async () => {
  const ng = mkGroup();
  if (tab) { ng.tabs.push(tab); ng.active = 0; }
  if (parent.children.length === 1) {    // lone child: re-aim the split
    parent.dir = dir;
    parent.el.className = "split " + dir;
    parent.el.querySelectorAll(":scope > .divider").forEach(d => { d.className = "divider " + dir; });
  }
  const idx = parent.children.indexOf(g);
  if (!parent.fractions) parent.fractions = parent.children.map(() => 1);
  if (parent.dir === dir) {              // same axis: insert sibling, halve g's share (DOM: divider + pane after g)
    const f = parent.fractions[idx] || 1;
    parent.children.splice(idx + 1, 0, ng);
    parent.fractions.splice(idx, 1, f / 2, f / 2);
    g.pane.after(divider(parent), ng.pane);
    applyFlex(parent);
  } else {                               // cross axis: wrap g in a nested split (DOM: new .split takes g's slot)
    const node = { dir, children: [g, ng], fractions: [0.5, 0.5] };
    parent.children[idx] = node;
    const s = document.createElement("div");
    s.className = "split " + dir;
    node.el = s;
    s.style.flex = g.pane.style.flex;
    g.pane.replaceWith(s);
    s.append(g.pane, divider(node), ng.pane);
    applyFlex(node);
  }
  placeRToggle();
  updateTitle();
  focusGroup(ng);
  if (ng.active >= 0) await loadActive(ng);
  });
}

async function collapseGroup(g) {  // R6.5: closing the last tab removes the group
  const parent = findParent(state.root, g);
  if (!parent) return;                        // lone root group: caller keeps it
  if (g.sim) cancelAnimationFrame(g.sim);     // stop the removed group's machinery
  if (g.saveT) clearTimeout(g.saveT);
  const idx = parent.children.indexOf(g);
  const heir = parent.children[idx + 1] || parent.children[idx - 1];
  parent.children.splice(idx, 1);
  const f = (parent.fractions || []).splice(idx, 1)[0] || 0;
  const hi = parent.children.indexOf(heir);   // nearest sibling absorbs the space
  if (hi >= 0 && parent.fractions[hi] != null) parent.fractions[hi] += f;
  // DOM: drop the pane and one adjacent divider (R20: siblings stay mounted, no re-render)
  const dv = g.pane.previousElementSibling || g.pane.nextElementSibling;
  if (dv && dv.classList.contains("divider")) dv.remove();
  g.pane.remove();
  applyFlex(parent);
  if (parent.children.length === 1) {         // simplify single-child splits
    const child = parent.children[0];
    const gp = findParent(state.root, parent);
    if (gp) {                                 // unwrap: child takes the split's slot + flex share
      gp.children[gp.children.indexOf(parent)] = child;
      elOf(child).style.flex = parent.el.style.flex;
      parent.el.replaceWith(elOf(child));
    } else if (child.children) {
      state.root = child;
      parent.el.replaceWith(child.el);
    }
    // lone Group at root keeps the boot wrapper { dir, children:[g] } —
    // splitGroup depends on every group having a findParent hit
  }
  if (state.focused === g) state.focused = null;
  placeRToggle();
  updateTitle();
  focusGroup(leaves(heir)[0]);                // focus nearest surviving group
  for (const h of groups()) renderTabs(h);    // drop stale chain glyphs (M8)
  treeHighlight();
}

let menuEl = null;
// the context menu carries no census, so smoke had to OCR it (flaky under llvmpipe).
// placeMenu()/closeMenu() publish the live item labels as [menu:a|b|c] instead.
function menuTok() {   // [menu:<labels>] + [mg:<left>,<first row centre y>,<row pitch>] — MEASURED, so a
  if (!menuEl) return "";                      // driver clicks item i at (left+20, centre + pitch*i) with no
  const k = menuEl.children, lbl = [...k].map(d => d.textContent).join("|");   // hardcoded padding/line-height guess
  // [mt:<kind>:<label>] — WHICH tab this menu belongs to (R20.8). Without it an assertion like
  // "a graph tab offers no Bookmark" rests on the driver's guess that x=380 hit the gg tab: hit
  // the wrong tab and the claim is about a tab nobody named. tabMenu stamps the target it was
  // handed by the browser's hit test, so the census reports the tab that was ACTUALLY clicked.
  const mt = menuEl.dataset.mt ? " [mt:" + menuEl.dataset.mt + "]" : "";
  if (!k.length) return " [menu:" + lbl + "]" + mt;
  const a = k[0].getBoundingClientRect();
  const pitch = k.length > 1 ? k[1].getBoundingClientRect().top - a.top : a.height;
  return " [menu:" + lbl + "] [mg:" + Math.round(menuEl.getBoundingClientRect().left) + "," +
         Math.round(a.top + a.height / 2) + "," + Math.round(pitch) + "]" + mt;
}
function closeMenu() { if (menuEl) { menuEl.remove(); menuEl = null; updateTitle(); } }
/* R22: a context menu is position:fixed — clamp it to the viewport by its
   MEASURED box. The old `innerWidth - 150 / innerHeight - 60` guesses spilled
   whenever the menu was wider/taller than the guess or the window was small.
   placeMenu appends first (so offsetWidth/Height are real), then places;
   clampMenu re-runs it after the menu's contents change (R13.1 pick list). */
function placeMenu(m, x, y) {
  m.style.left = "0px"; m.style.top = "0px";
  document.body.appendChild(m);
  m.dataset.x = x; m.dataset.y = y;
  menuEl = m;
  clampMenu(m);
  updateTitle();                  // census [menu:1]
}
function clampMenu(m) {
  m.style.left = Math.max(0, Math.min(+m.dataset.x, innerWidth - m.offsetWidth)) + "px";
  m.style.top = Math.max(0, Math.min(+m.dataset.y, innerHeight - m.offsetHeight)) + "px";
}
document.addEventListener("contextmenu", e => e.preventDefault()); // app-like: native menu never
document.addEventListener("mousedown", e => {
  if (menuEl && !menuEl.contains(e.target)) closeMenu();
}, true);
// S1 (docs/security-review.md): external links (a.ext from render) NEVER
// navigate the webview. One capture-phase gate for both mousedown (the lp raw
// row opens on mousedown) and click (the default navigation): swallow, then
// hand the href to open_external, which re-checks the scheme in Rust and
// spawns xdg-open. [ext:N] census counts the attempts (headless probe).
let extCount = 0;
for (const ev of ["mousedown", "click"])
  document.addEventListener(ev, e => {
    const a = e.target && e.target.closest && e.target.closest("a.ext");
    if (!a) return;
    e.preventDefault(); e.stopPropagation();
    if (ev !== "click") return;
    extCount++; updateTitle();
    inv("open_external", { url: a.getAttribute("href") }).catch(err => console.warn("open_external:", err));
  }, true);

function tabMenu(e, g, i) {              // right-click a tab -> Split right / Split down / Link with tab... (R13.1)
  e.preventDefault();
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  const tab = g.tabs[i];
  // stamp the menu with the tab the hit test actually handed us -> census [mt:kind:label].
  // gg/lg tabs carry no note, so they publish their kind and an empty label. Separators and ':'
  // are stripped so a note named 'a:b]' cannot forge a token.
  m.dataset.mt = (tab.kind || "note") + ":" +
    (tab.kind ? "" : String(tab.name).split("/").pop().replace(/[|\]:]/g, ""));
  const item = (label, fn) => {
    const d = document.createElement("div");
    d.textContent = label;
    d.onmousedown = ev => ev.stopPropagation();  // don't let the closer eat the click
    d.onclick = () => { if (fn) fn(); else closeMenu(); };
    m.appendChild(d);
  };
  const pick = () => {                   // R13.1 pick list: every other open tab, in layout order
    m.innerHTML = "";
    setTimeout(() => clampMenu(m), 0);   // R22: the pick list resizes the menu — re-clamp it
    let any = false;
    for (const h of groups()) for (const t of h.tabs) {
      if (t === tab) continue;
      any = true;
      item((t.kind === "gg" ? "Graph" : t.name.split("/").pop()), () => { closeMenu(); linkTabs(tab, t); });
    }
    if (!any) item("(no other tabs)");
    updateTitle();                       // the menu was rebuilt in place -> refresh [menu:]
  };
  item("Split right", () => { closeMenu(); splitGroup(g, "row", i); });
  item("Split down",  () => { closeMenu(); splitGroup(g, "col", i); });
  if (isLinked(g, tab, i)) item("Unlink tab", () => { closeMenu(); unlinkTab(g, tab); });
  else item("Link with tab...", pick);
  if (!tab.kind) {   // R12.4 / R20 (#3): the source-vs-LP RADIO lives here (stock), not in a chrome icon — ✓ on the active one, per tab
    item((tab.src ? "" : "✓ ") + "Live preview", () => { closeMenu(); setMode(g, "livepreview"); });
    item((tab.src ? "✓ " : "") + "Source mode",  () => { closeMenu(); setMode(g, "source"); });
  }
  if (!tab.kind) item(bmCache.includes(tab.name) ? "Remove bookmark" : "Bookmark",  // R20.4 (#12, bookmarks pane = R9.5): same toggle as the tree row menu; graph tabs (gg/lg) have no note to bookmark
    () => { closeMenu(); toggleBm(tab.name); });                                    // toggleBm re-renders the bookmarks pane
  placeMenu(m, e.clientX, e.clientY);   /* R22: viewport-clamped by MEASURED size — supersedes the old innerWidth-150 guess and the #12 post-append top clamp */
}

/* ---------- view modes (R8.8: livepreview / source / reading per tab) ---------- */
const ICON_BOOK = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>';
const ICON_PEN = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z"/></svg>';
const MODE_ABBR = { livepreview: "lp", source: "src", reading: "read" };
const isLp = m => m === "livepreview" || m === "source";  // R12: both render in g.lp
function caretLC(g) { return Ed.caretLC(g); }   // [line, col] of the caret in the lp view

/* ---------- R35 SCROLL SYNC: the position that survives Ctrl+E is CONTENT ----------
   /workspace/notes/brief-scroll-sync.txt, docs/requirements.md §34 (R35).
   The two views are TWO INDEPENDENT SCROLLERS (g.lp and g.preview) over the
   SAME note at DIFFERENT pixel heights: reading collapses markdown syntax and
   re-flows, measured at 0.868x the edit document on the 320-line fixture, and
   the compaction is NOT uniform along the note (1.00 across plain prose, 0.90
   across headings and lists), so `preview.scrollTop = lp.scrollTop` has no
   correct scale factor — it is 9-20 source lines wrong mid-note (negative
   control N2, docs/scroll-sync-controls/).
   What is preserved instead is the SOURCE LINE at the top of the viewport,
   fractional so a position inside a long block survives too:
     edit side    row i of g.lp IS source line i (Ed.render, one row per line)
     reading side block k of g.preview starts at source line pvLines[k]
                  (block_lines, from the renderer's own parser)
   Both directions map line -> pixels through the destination's OWN geometry,
   so neither view's height is ever assumed from the other's. */
function ssTopOf(cont, kids) {   // -> [index, fraction] of the first child not scrolled off the top
  const ct = cont.getBoundingClientRect().top;
  for (let i = 0; i < kids.length; i++) {
    const r = kids[i].getBoundingClientRect();
    if (r.bottom > ct + 1) {
      const f = r.height > 0 ? Math.min(1, Math.max(0, (ct - r.top) / r.height)) : 0;
      return [i, f];
    }
  }
  return [Math.max(0, kids.length - 1), 0];
}
function ssScrollTo(cont, el, frac) {   // put `frac` into `el` at the top of `cont`, clamped
  const d = el.getBoundingClientRect().top - cont.getBoundingClientRect().top
          + frac * el.getBoundingClientRect().height;
  const max = Math.max(0, cont.scrollHeight - cont.clientHeight);
  cont.scrollTop = Math.min(max, Math.max(0, cont.scrollTop + d));
}
function ssAnchor(g) {   // the (fractional, 0-based) SOURCE LINE at the top of the CURRENT view, or null
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind || g.graphOn) return null;
  if (t.mode === "reading") {
    const ch = g.preview.children, bl = g.pvLines;
    if (!ch.length || !bl || bl.length !== ch.length) return null;   // map out of step: no anchor beats a wrong one
    const [i, f] = ssTopOf(g.preview, ch);
    const a = bl[i] - 1, b = i + 1 < bl.length ? bl[i + 1] - 1 : a + 1;
    return a + f * Math.max(1, b - a);
  }
  const rows = g.lp.children;
  if (!rows.length) return null;
  const [i, f] = ssTopOf(g.lp, rows);
  return i + f;
}
function ssRestore(g, line) {   // scroll the CURRENT view so `line` is at the top
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind || line == null || g.graphOn) return;
  if (t.mode === "reading") {
    const ch = g.preview.children, bl = g.pvLines;
    if (!ch.length || !bl || bl.length !== ch.length) return;
    let i = 0;
    while (i + 1 < bl.length && bl[i + 1] - 1 <= line) i++;
    const a = bl[i] - 1, b = i + 1 < bl.length ? bl[i + 1] - 1 : a + 1;
    ssScrollTo(g.preview, ch[i], Math.min(1, Math.max(0, (line - a) / Math.max(1, b - a))));
    return;
  }
  const rows = g.lp.children;
  if (!rows.length) return;
  const i = Math.min(rows.length - 1, Math.max(0, Math.floor(line)));
  ssScrollTo(g.lp, rows[i], Math.min(1, Math.max(0, line - i)));
}

/* ---------- R35 PERF: the mode switch against the 100 ms ceiling ----------
   Three numbers, because they fail for different reasons and one of them is
   the only one this feature OWNS:
     [msw:<last>/<max>/<avg>/<n>] command -> FIRST PAINT (rAF -> task, the same
       pattern as Ed.cmEnd/[cm:] and [sfp:], so the numbers are comparable);
     [mswk:<last>/<max>]          the synchronous work before the frame —
       flushSave + the full re-render of the destination view + the restore;
     [mswss:<last>/<max>]         ssAnchor + ssRestore ALONE, i.e. the cost the
       scroll-sync feature ADDED on top of a mode switch that already had to
       re-render. A regression in the anchoring (say an O(n^2) walk over the
       block map) moves this number and nothing else, and it cannot hide behind
       the re-render or behind a stalled compositor frame.
   t0 is taken at the top of setMode — the command, before any DOM work — not
   at the keystroke: Ctrl+E, the view-header icon and the tab-menu radio all
   funnel through here, so one probe covers every way in. */
let mswMs = -1, mswMax = 0, mswN = 0, mswSum = 0, mswW = -1, mswWMax = 0, mswSs = -1, mswSsMax = 0;
const mswR = x => Math.round(x * 100) / 100;
function mswEnd(t0, ss) {
  mswSs = mswR(ss); if (mswSs > mswSsMax) mswSsMax = mswSs;
  mswW = mswR(performance.now() - t0); if (mswW > mswWMax) mswWMax = mswW;
  requestAnimationFrame(() => setTimeout(() => {
    const ms = mswR(performance.now() - t0);
    mswMs = ms; mswN++; mswSum += ms;
    if (ms > mswMax) mswMax = ms;
    if (typeof otel !== "undefined" && otel.span) otel.span("mode_switch", { ss_ms: mswSs }, ms);
    updateTitle();
  }, 0));
}
function mswTok() {
  return (mswMs >= 0 ? " [msw:" + mswMs + "/" + mswMax + "/" + mswR(mswSum / mswN) + "/" + mswN + "]" : "") +
         (mswW >= 0 ? " [mswk:" + mswW + "/" + mswWMax + "]" : "") +
         (mswSs >= 0 ? " [mswss:" + mswSs + "/" + mswSsMax + "]" : "");
}

function updateModeBtn(g) {
  const tb = g.active >= 0 ? g.tabs[g.active] : null;
  const m = tb ? tb.mode : "livepreview";
  g.modebtn.innerHTML = m === "reading" ? ICON_PEN : ICON_BOOK;   // R20 (#3)/#16: stock shows pen/book only — TWO states; source vs LP lives in the tab menu radio + its own command
}

function applyMode(g) {  // exactly ONE of lp / preview fills the pane
  const m = g.active >= 0 ? g.tabs[g.active].mode : "livepreview";
  g.editor.style.display = "none";          // R12: the model textarea never shows; source = lp + reveal
  g.lp.style.display = isLp(m) ? "" : "none";
  g.lp.classList.toggle("src", m === "source");
  g.preview.style.display = m === "reading" ? "" : "none";
  updateModeBtn(g);
}

async function cmdToggleMode(g) {  // #16: Ctrl+E / the view-header icon = EDIT <-> READING, two states, nothing else
  g = g || fg();
  if (!g || g.active < 0 || g.graphOn) return;
  const tab = g.tabs[g.active];
  if (tab.kind) return;             // graph tabs (lg/gg) have no view mode
  // leaving reading lands in the sub-mode you left from (tab.src is untouched by
  // the reading flag) -> source -> reading -> source, like stock
  await setMode(g, tab.read ? (tab.src ? "source" : "livepreview") : "reading");
}
async function cmdToggleSource(g) {  // stock "Toggle Live Preview/Source mode": flips the OTHER bit
  g = g || fg();
  if (!g || g.active < 0 || g.graphOn) return;
  const tab = g.tabs[g.active];
  if (tab.kind) return;
  await setMode(g, tab.src ? "livepreview" : "source");   // an editing choice: it also leaves reading view
}
async function setMode(g, mode) {   // R20 (#3): one target mode — tab menu radio / palette / the Ctrl+E edit<->reading toggle
  const tab = g.tabs[g.active];
  const mswT0 = performance.now();               // R35 perf: the command, before any DOM work
  const keep = g.lpActive ? caretLC(g) : null;   // R12.4: caret survives lp<->src
  const mswA0 = performance.now();
  const anchor = ssAnchor(g);                    // R35: read the top SOURCE LINE from the OLD view, before anything flips
  let mswSsMs = performance.now() - mswA0;
  await flushSave(g);
  tab.mode = mode;
  hideAc();
  applyMode(g);
  if (tab.mode === "reading") await preview(g);
  if (tab.mode === "livepreview") await lpRender(g, -1, 0, true);  // mode switch: full rebuild
  if (tab.mode === "source") {      // caret where it was, else end of note
    const L = Ed.lines(g);
    await lpRender(g, keep ? keep[0] : L.length - 1, keep ? keep[1] : L[L.length - 1].length, true);
  }
  // R35: the destination is rendered — place the SAME source line at the top of
  // it, through its own geometry. AFTER the caret work above on purpose: source
  // mode's Ed.place() scrolls the caret into view, and the scroll the USER chose
  // outranks the caret they cannot see (the brief's "caret off-screen" case).
  const mswR0 = performance.now();
  ssRestore(g, anchor);
  mswSsMs += performance.now() - mswR0;
  mswEnd(mswT0, mswSsMs);
  updateTitle();
}

/* ---------- tabs (per group) ---------- */
function renderTabs(g) {
  g.tabsEl.innerHTML = "";
  // R7.3 / R13.2: chain glyph on every linked tab (lg auto-link + manual links)
  g.tabs.forEach((tab, i) => {
    const d = document.createElement("div");
    d.className = "tab" + (i === g.active ? " active" : "");
    const ttl = document.createElement("span");
    ttl.className = "t";
    const chain = isLinked(g, tab, i);
    ttl.textContent = (chain ? "\u{1F517} " : "") + tab.name.split("/").pop();
    const x = document.createElement("span");
    x.className = "x";
    // ux-1: inline svg cross — the ✕ glyph tofu'd under webkit2gtk
    x.innerHTML = '<svg viewBox="0 0 10 10"><path d="M1 1l8 8M9 1l-8 8" ' +
      'stroke="currentColor" stroke-width="1.4" stroke-linecap="round" fill="none"/></svg>';
    x.onclick = e => { e.stopPropagation(); closeTab(g, i); };
    d.append(ttl, x);
    d.onclick = () => switchTab(g, i);
    d.oncontextmenu = e => tabMenu(e, g, i);   // R6.2: split verbs
    d.addEventListener("mousedown", e => tabDragStart(e, g, i));  // ux-5: tab drag
    g.tabsEl.appendChild(d);
  });
  updateModeBtn(g);
  updateTitle();                    // keep [tabs:] census fresh on tab changes
}

/* ux-5 (R6.7): tab drag — ghost chip follows the cursor past a 6px threshold.
   Drop targets, resolved per mousemove against every pane:
     - another group's tab strip (cursor inside that pane, above the strip's
       bottom edge)            -> tab MOVES there (appended, becomes active)
     - the right EDGE zone of any pane (last 40px, below the strip)
                               -> new row-split carrying the tab (splitWith)
   Source group emptied by the move collapses (same rule as closeTab). */
const TAB_EDGE = 40;
function tabDragStart(e, g, i) {
  if (e.button !== 0 || e.target.closest(".x")) return;
  const sx = e.clientX, sy = e.clientY;
  let ghost = null, target = null, hl = null, zones = null, raf = 0, last = null;
  const clearHl = () => {
    if (hl) { hl.classList.remove("drop-strip", "drop-edge"); hl = null; }
  };
  const sameT = (a, b) => (a ? a.kind + a.g.id : "") === (b ? b.kind + b.g.id : "");
  const step = () => {                       // R20: one span per FRAME; rects cached at drag start, classes only on target change
    raf = 0;
    const ev = last, mT0 = performance.now();
    if (!ghost) {
      if (Math.abs(ev.clientX - sx) + Math.abs(ev.clientY - sy) < 6) return;
      ghost = document.createElement("div");
      ghost.id = "tabghost";
      ghost.textContent = g.tabs[i] ? g.tabs[i].name.split("/").pop() : "";
      document.body.appendChild(ghost);
      zones = groups().map(h => ({ g: h, pr: h.pane.getBoundingClientRect(), tb: h.tabsEl.getBoundingClientRect().bottom }));
    }
    ghost.style.transform = "translate3d(" + (ev.clientX + 10) + "px," + (ev.clientY + 12) + "px,0)";
    let nt = null;
    for (const z of zones) {
      const pr = z.pr;
      if (ev.clientX < pr.left || ev.clientX > pr.right ||
          ev.clientY < pr.top || ev.clientY > pr.bottom) continue;
      if (ev.clientY <= z.tb) {
        if (z.g !== g) nt = { kind: "strip", g: z.g };   // own strip: reorder unsupported, no-op
      } else if (ev.clientX > pr.right - TAB_EDGE) nt = { kind: "edge", g: z.g };
      break;
    }
    if (!sameT(nt, target)) {
      clearHl();
      if (nt) { hl = nt.kind === "strip" ? nt.g.tabsEl : nt.g.pane; hl.classList.add(nt.kind === "strip" ? "drop-strip" : "drop-edge"); }
    }
    target = nt;
    otel.span("tab_drag_move", { groups: zones.length, target: target ? target.kind : "" }, performance.now() - mT0);
  };
  const move = ev => { last = ev; if (!raf) raf = requestAnimationFrame(step); };
  const up = async () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    if (raf) { cancelAnimationFrame(raf); raf = 0; if (last) step(); }
    const t = target;
    if (ghost) ghost.remove();
    clearHl();
    if (!ghost || !t) return;                // plain click, or dropped nowhere
    await act("tab_drop", { kind: t.kind, groups: groups().length, note: g.tabs[i] ? g.tabs[i].name : "" }, async () => {   // R20: pane/tab nodes MOVE, no layout rebuild
    await flushSave(g);
    const tab = g.tabs.splice(i, 1)[0];
    if (!tab) return;
    if (g.active >= g.tabs.length) g.active = g.tabs.length - 1;
    else if (i < g.active) g.active--;
    if (t.kind === "strip") {
      t.g.tabs.push(tab);
      t.g.active = t.g.tabs.length - 1;
      if (!g.tabs.length && groups().length > 1) await collapseGroup(g);
      else { await loadActive(g); renderTabs(g); }
      focusGroup(t.g);
      await loadActive(t.g);
      renderTabs(t.g);
    } else {                                 // edge: split, then collapse an
      await splitWith(t.g, "row", tab);      // emptied source (lone-tab drag to
      if (!g.tabs.length) await collapseGroup(g);  // own edge nets a plain move)
      else { await loadActive(g); renderTabs(g); }
    }
    });
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}

/* #20 (R32): the big title a note shows is its FILENAME, rendered — never
   bytes in the file (stock calls it the inline title; a new note is zero
   bytes). It is published as data-title on the two SCROLLERS and drawn by a
   ::before in ui/style.css, deliberately NOT as a DOM child: ui/editor.js
   indexes model rows positionally (g.lp.children[l], editor.js:491), so a
   prepended element would shift every line index by one and break the caret.
   Inside the scroller = it scrolls with the content, like stock, and adds no
   scroll container (R22.2's allowlist is unchanged). No name -> attribute
   REMOVED, so an empty pane keeps its y-origin. */
const titleOf = name => (name ? name.split("/").pop().replace(/\.md$/i, "") : "");
function setInlineTitle(v, name) {
  const t = titleOf(name);
  for (const el of [v.lp, v.preview]) {
    if (!el) continue;
    if (t) el.dataset.title = t; else delete el.dataset.title;
  }
}

/* R34.15 — THE CARET PROBLEM, SOLVED OUTSIDE THE ROW LIST.
   R34 makes the inline title a rename surface (reversing half of R32.5), and
   the hard part is not the rename: it is that ui/editor.js addresses model
   rows POSITIONALLY (`rowAt(g,l) = g.lp.children[l]`, editor.js:491). Anything
   editable placed INSIDE the scroller — prepended, appended, or absolutely
   positioned — becomes a member of `g.lp.children` and either shifts every
   line index by one or lengthens a list that `Ed.pos`/`Ed.paint`/`lpRender`
   read as "one child per source line". R15's measured offsets and phase
   `typo`'s SKIP=1 sit on top of that.
   So the caret surface is a SIBLING of the scroller, in `.content` (which is
   position:relative), positioned over the ::before's measured box; the
   ::before itself stays in place and merely goes `visibility: hidden`
   (ui/style.css .lp.titling::before), so the LAYOUT is untouched — the title
   box still reserves its 44px and row 0 does not move.
   Consequence, and it is the point: `g.lp.children` is IDENTICAL whether the
   title is being edited or not. updateTitle publishes [te:<text>/<rows>] so a
   smoke phase can assert that number instead of trusting this comment. */
let titling = null;          // {g, host, wrap, el, name, orig} while the title is being edited
const titleEditing = () => !!titling;

/* ---------- R34.18 TYPE PROBE (test-only, RUSTIDIAN_TYPEPROBE=1) ------------
   The operator's report was "renaming the h1 changes the font-size/decoration",
   and ui/style.css answered it with a COMMENT ("repeats the ::before's type
   declarations verbatim"). A comment is what produced the bug. So the two
   surfaces are made MEASURABLE instead: this probe reads getComputedStyle off
   the LIVE DOM — the ::before that draws the rendered title, and the .titleedit
   that replaces it — and publishes both in the census, so a phase compares the
   two surfaces to EACH OTHER and never to a pixel literal.
   Everything here is inert unless the backend hook says the env var is set:
   no pre-existing phase sees a changed census, and a shipped build has no
   perturbation chords. */
let typeProbe = false;
inv("type_probe").then(v => { typeProbe = !!v; if (typeProbe) tpInstall(); }).catch(() => {});
let tpPerturb = "";          // the negative control's forced font-size on .titleedit ("" = none)
const tpHash = s => {        // FNV-1a/32 — font stacks are 250+ chars; the census compares
  let h = 2166136261;        // the HASH and prints the head, so "same family" is a measurement
  for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619); }
  return (h >>> 0).toString(16).padStart(8, "0");
};
const tpFam = s => s.split(",")[0].replace(/["']/g, "").trim().slice(0, 16).replace(/[[\]|/~ ]/g, "_");
const tpType = cs => [cs.fontSize, cs.fontWeight, cs.letterSpacing, cs.lineHeight,
                      cs.color.replace(/\s+/g, ""), tpHash(cs.fontFamily), tpFam(cs.fontFamily)].join("/");
/* the scroller's child list as a STRING, so R34.15 is proved byte-for-byte
   instead of by a count: one "<tag>.<class>" per child, plus the length. A
   caret surface that became a row changes this token; a count alone would not
   notice a swap. Published whenever a live-preview scroller exists, so the
   phase can read it with the editor CLOSED and OPEN and diff the two. */
const tpKids = lp => {
  if (!lp) return "-";
  const sig = [...lp.children].map(c => c.tagName.toLowerCase() + "." +
                (c.className || "-").toString().replace(/[[\]|/~ ]/g, "_")).join(",");
  return lp.children.length + ":" + tpHash(sig) + ":" + sig.slice(0, 90);
};
function tpInstall() {       // the three chords, capture-phase so a focused contenteditable cannot eat them
  window.addEventListener("keydown", e => {
    if (!(e.ctrlKey && e.altKey && e.shiftKey)) return;
    if (e.key === "1" || e.code === "Digit1") {          // PERTURB the caret surface's type
      tpPerturb = "31px";
      if (titling) titling.el.style.fontSize = tpPerturb;
    } else if (e.key === "2" || e.code === "Digit2") {   // RESTORE it
      tpPerturb = "";
      if (titling) titling.el.style.fontSize = "";
    } else if (e.key === "3" || e.code === "Digit3") {   // move --font-text-size off its default
      const r = document.documentElement;
      r.style.setProperty("--font-text-size", r.style.getPropertyValue("--font-text-size") ? "" : "22px");
    } else return;
    e.preventDefault(); e.stopPropagation();
    updateTitle();
  }, true);
}

/* The ::before's box in CLIENT coordinates (a pseudo-element has no node, so it
   cannot be measured with getBoundingClientRect — it is derived instead):
   top    = the scroller's content-box top, minus how far it has scrolled;
   height = the distance to row 0, less the ::before's own margin-bottom;
   width  = the content box (the text column inside it is re-centred by CSS,
            exactly as `.lp > *` and the ::before itself are). */
function titleBox(host) {
  if (!host || !host.dataset.title) return null;
  const cs = getComputedStyle(host), pre = getComputedStyle(host, "::before");
  const r = host.getBoundingClientRect();
  const padT = parseFloat(cs.paddingTop) || 0;
  const padL = parseFloat(cs.paddingLeft) || 0, padR = parseFloat(cs.paddingRight) || 0;
  const top = r.top + padT - host.scrollTop;
  const first = host.firstElementChild;
  const mb = parseFloat(pre.marginBottom) || 0;
  const h = first ? first.getBoundingClientRect().top - top - mb
                  : (parseFloat(pre.height) || parseFloat(pre.lineHeight) || 0);
  return { top, left: r.left + padL, width: Math.max(0, host.clientWidth - padL - padR), height: h };
}
const titleHit = (host, x, y) => {          // is this click on the title's own band?
  const b = titleBox(host);
  return !!b && b.height > 0 && y >= b.top && y < b.top + b.height && x >= b.left && x < b.left + b.width;
};

function openTitleEdit(g, host, x, y) {
  if (titling) closeTitleEdit();
  const name = curOf(g);
  const b = name ? titleBox(host) : null;
  if (!b || b.height <= 0) return false;
  const wrap = document.createElement("div");
  wrap.className = "titlewrap";
  const el = document.createElement("div");
  el.className = "titleedit";
  el.contentEditable = "plaintext-only";     // one line of text, no markup, no paste-in HTML
  el.spellcheck = false;
  if (tpPerturb) el.style.fontSize = tpPerturb;   // R34.18 negative control, armed before this open
  el.textContent = titleOf(name);
  wrap.appendChild(el);
  wrap.style.top = (b.top - g.content.getBoundingClientRect().top) + "px";
  wrap.style.left = (b.left - g.content.getBoundingClientRect().left) + "px";
  wrap.style.width = b.width + "px";
  host.classList.add("titling");             // hide the INK, keep the BOX
  g.content.appendChild(wrap);               // sibling of the scroller — never g.lp.children
  titling = { g, host, wrap, el, name, orig: titleOf(name) };
  el.focus();
  const rng = typeof x === "number" && document.caretRangeFromPoint ? document.caretRangeFromPoint(x, y) : null;
  const sel = window.getSelection();
  if (sel) {
    const r = rng && el.contains(rng.startContainer) ? rng : document.createRange();
    if (!(rng && el.contains(rng.startContainer))) r.selectNodeContents(el), r.collapse(false);
    sel.removeAllRanges(); sel.addRange(r);
  }
  el.addEventListener("keydown", onTitleKey);
  el.addEventListener("input", onTitleInput);   // R34.12/R34.13: refuse WHILE typing, as stock does
  el.addEventListener("blur", () => closeTitleEdit());
  host.addEventListener("scroll", onTitleScroll);
  updateTitle();
  return true;
}

/* Scrolling the note away from under an absolutely-positioned overlay would
   paint a title over the tab row (.content does not clip). Editing a filename
   while scrolling the body is not an interaction anyone performs, so a scroll
   REVERTS — which loses nothing, because a revert never renames. */
function onTitleScroll() { closeTitleEdit(); }

function closeTitleEdit() {
  if (!titling) return;
  const t = titling;
  titling = null;                            // first: el.remove() fires blur, which re-enters
  t.host.removeEventListener("scroll", onTitleScroll);
  t.wrap.remove();
  t.host.classList.remove("titling");
  if (t.host === t.g.lp && t.g.lp.isConnected) t.g.lp.focus({ preventScroll: true });
  updateTitle();
}

function onTitleKey(e) {
  if (e.key === "Escape") {                  // R34.10: Escape REVERTS, measured past blur
    e.preventDefault(); e.stopPropagation();
    closeTitleEdit();
    return;
  }
  if (e.key === "Enter") {
    e.preventDefault(); e.stopPropagation();
    commitTitleEdit();
    return;
  }
  e.stopPropagation();                       // a filename contains characters the R14 keymap binds
}

/* R34.1 — ENTER RENAMES THE FILE. The order is stock's, measured, and it is
   not negotiable: the file moves FIRST (move_note), the links are still stale
   at that instant (R34.2), and only THEN is the question asked. The refusals
   come before any of it and produce a visible notice instead of a rename.

   Every branch here is a measurement:
     empty title      -> revert, no rename            (R34.14, E8/E9)
     unchanged title  -> nothing at all
     illegal chars    -> notice, BOX STAYS OPEN       (R34.13, E6/E7) — and a
                         "/" is a rejected character, never a move into a folder
     existing name    -> notice, BOX STAYS OPEN       (R34.12, E4/E5)
     otherwise        -> move now, ask after          (R34.1/R34.2)
   The UI's collision check is a COURTESY (stock shows its notice before Enter);
   the one that counts is create_new/O_EXCL in move_note_in, and a move_note
   error is surfaced, never swallowed.

   NOTE NAMES CARRY NO EXTENSION. Every name on the IPC surface and in
   notesCache is vault-relative and `.md`-less ("sub/Nested"); the extension is
   appended at the filesystem boundary alone (`note_path_in` in
   src-tauri/src/main.rs: `cdir.join(format!("{}.md", …))`), which is why F2's
   rename box shows "Ideas" and not "Ideas.md". The first cut of this function
   built `t + ".md"` and produced `ZR Target X.md.md` on disk plus a
   notesCache collision check that could never match — invisible to every unit
   test (the rust side is asserted with `.md`-less keys) and to phase `title`
   (which never presses Enter). The `rename` phase caught it on the first run;
   that is what an end-to-end phase is FOR. */
const TITLE_ILLEGAL = /[\\/:*?"<>|]/;        // stock's own set (R34.13, read at 250%)
async function commitTitleEdit() {
  if (!titling) return;
  const { g, el, name, orig } = titling;
  const t = (el.textContent || "").replace(/[\r\n]+/g, " ").trim();
  if (!t || t === orig) { closeTitleEdit(); return; }     // R34.14 / no-op
  if (!titleCheck()) return;                 // illegal chars or a name already taken:
                                             // the notice is up, the box stays open, NOTHING moved
  const dir = name.includes("/") ? name.slice(0, name.lastIndexOf("/") + 1) : "";
  const nn = dir + t;                        // NAMES CARRY NO EXTENSION (see NOTE NAMES below)
  closeTitleEdit();
  await flushSave(g);                        // the body's own bytes land before the file moves
  let blast;
  try { blast = await inv("move_note", { old: name, new: nn }); }
  catch (err) { say(String(err && err.message || err)); updateTitle(); return; }
  await applyRename(name, nn);
  const links = blast && blast.links || 0, files = blast && blast.files || 0;
  if (!files) { updateTitle(); return; }     // R34.3: nothing links in -> NO modal, ever
  let consent = false;
  try { consent = await inv("link_consent"); } catch (err) { consent = false; }
  if (consent) { await runUpdateLinks(name, nn); return; }   // R34.8: already answered, in the vault
  openUpdateLinks(name, nn, links, files);
}

/* ---------- R34.4-R34.8 the "Update links" prompt --------------------------
   This modal is the difference between R34 and the DELETED ux-3 feature, which
   rewrote a whole vault's links with no question asked. So it is written as a
   gate, not as a notification: `update_links` is called from exactly two
   places, both of them on the far side of a recorded answer (this modal, or
   R34.8 consent already in the vault). */
let ulPending = null, noticeTxt = "", noticeT = 0, noticeSrc = "";

function say(msg, src) {                     // R34.12/R34.13: a refusal is VISIBLE
  const b = $("notice");
  b.textContent = msg;
  b.hidden = !msg;
  noticeTxt = msg || "";
  noticeSrc = msg ? (src || "") : "";
  clearTimeout(noticeT);
  if (msg) noticeT = setTimeout(() => { $("notice").hidden = true; }, 4000);
  updateTitle();
}

/* R34.12/R34.13 — the refusal is measured on stock BEFORE Enter: the notice
   "There's already a file with the same name" is on screen while the title box
   still holds `Dup`, and Enter then does nothing (E4/E5). So the check runs on
   every keystroke, and it RETRACTS when the name becomes legal again — a notice
   that outlived the condition it describes is worse than none. It is a courtesy
   check, though: what actually refuses a collision is create_new/O_EXCL in the
   backend, which is the only thing that holds against another writer. */
function titleCheck() {
  if (!titling) return true;
  const t = (titling.el.textContent || "").replace(/[\r\n]+/g, " ").trim();
  const name = titling.name;
  const dir = name.includes("/") ? name.slice(0, name.lastIndexOf("/") + 1) : "";
  let msg = "";
  if (t && TITLE_ILLEGAL.test(t)) msg = 'File name cannot contain any of these characters: \\ / : * ? " < > |';
  else if (t && t !== titling.orig && notesCache.includes(dir + t)) msg = "There's already a file with the same name";
  if (msg) { if (msg !== noticeTxt) say(msg, "title"); return false; }
  if (noticeSrc === "title") say("", "title");
  return true;
}
function onTitleInput() { titleCheck(); updateTitle(); }

/* R34.5 — the counted sentence, rendered from the radius move_note counted
   BEFORE the move. Stock's fixture was plural on both numbers ("4 links in 1
   file"); the SINGULAR form was never observed, so the n!==1 pluralisation is
   OURS and is flagged as such in docs/requirements.md R34.5. */
const ulSentence = (links, files) =>
  "This will affect " + links + (links === 1 ? " link" : " links") +
  " in " + files + (files === 1 ? " file" : " files") + ".";

function openUpdateLinks(old, nn, links, files) {
  ulPending = { old, nn, links, files };
  $("ulsay").textContent = ulSentence(links, files);
  $("ulbox").hidden = false;
  $("ul-always").focus();                    // R34.6/R34.7: the DEFAULT is the leftmost, measured in pixels
  updateTitle();
}
function closeUpdateLinks() {
  if (!ulPending) return;
  ulPending = null;
  $("ulbox").hidden = true;
  const g = fg();
  if (g && g.lp && g.lp.isConnected) g.lp.focus({ preventScroll: true });
  updateTitle();
}
async function runUpdateLinks(old, nn) {     // the CONSENTED half, and the only caller of update_links
  try { await inv("update_links", { old, new: nn }); }
  catch (err) { say(String(err && err.message || err)); }
  // Bookkeeping ONLY, deliberately: the rewrite changed text in OTHER notes, and
  // reloading them here would discard any unsaved buffer they hold (some group's
  // dirty tab is not this rename's business). F2's rename does the same and no
  // more — a tab showing stale link text is a repaint, a clobbered buffer is a
  // data-loss bug.
  await refreshTree();
  updateTitle();
}
async function ulAnswer(kind) {
  if (!ulPending) return;
  const { old, nn } = ulPending;
  closeUpdateLinks();
  if (kind === "no") return;                 // renamed file, stale links — stock's shape (R34.2)
  if (kind === "always") {
    try { await inv("set_link_consent", { on: true }); }   // R34.8: remembered in the VAULT
    catch (err) { say(String(err && err.message || err)); }
  }
  await runUpdateLinks(old, nn);
}
$("ul-always").onclick = () => ulAnswer("always");
$("ul-once").onclick = () => ulAnswer("once");
$("ul-no").onclick = () => ulAnswer("no");
/* Escape = "Do not update": the conservative answer is the one that writes
   nothing, and dismissing a question is not consent. */
$("ulbox").addEventListener("keydown", e => {
  if (!ulPending) return;
  if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); ulAnswer("no"); return; }
  /* MEASURED, not assumed: under WebKitGTK a shift+Tab arrives as
     `key = "Unidentified"`, `shiftKey = true`, `keyCode = 9` — X11 sends the
     ISO_Left_Tab keysym and GDK has no `key` name for it. A trap written as
     `e.key === "Tab"` therefore catches forward-Tab only and the ring can never
     walk back; the first cut of this handler did exactly that and phase `rename`
     caught it ([ulk:Unidentified,S,9], measured with a throwaway census probe).
     keyCode 9 is the spelling both directions share. */
  if (e.key === "Tab" || e.keyCode === 9) {  // a focus TRAP: three buttons, and the ring never leaves them
    e.preventDefault();
    const b = [$("ul-always"), $("ul-once"), $("ul-no")];
    const i = b.indexOf(document.activeElement);
    b[(i < 0 ? 0 : i + (e.shiftKey ? b.length - 1 : 1)) % b.length].focus();
    updateTitle();
    return;
  }
  e.stopPropagation();                       // Enter activates the FOCUSED button (the browser does that)
});
$("ulbox").addEventListener("mousedown", e => { if (e.target === $("ulbox")) e.preventDefault(); });  // click-off is not an answer

/* R34.5/R34.6 made observable WITHOUT a screenshot, and the geometry published
   so the pixel test does not have to guess where to look (R33.13's idiom):
     [ul:<links>/<files>/<focused button index 0..2>]
     [ulsay:<the counted sentence, verbatim>]
     [ulx:<cx,cy>;<cx,cy>;<cx,cy>]   centres of Always / Just once / Do not update
   The focus index is the DEFAULT-button assertion (R34.6/R34.7) in a form that
   cannot be faked by a screenshot's antialiasing, and [ulx:] is what a
   focus-ring colour-delta probe reads its coordinates from. */
function ulTok() {
  const b = [$("ul-always"), $("ul-once"), $("ul-no")];
  const f = b.indexOf(document.activeElement);
  const xs = b.map(e => {
    const r = e.getBoundingClientRect();
    return Math.round(r.left + r.width / 2) + "," + Math.round(r.top + r.height / 2);
  }).join(";");
  return " [ul:" + ulPending.links + "/" + ulPending.files + "/" + f + "]" +
         " [ulsay:" + $("ulsay").textContent.replace(/[[\]|]/g, "") + "]" +
         " [ulx:" + xs + "]";
}

async function loadActive(g) {
  hideAc();
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (t && t.kind === "lg") {       // R7.1: localgraph tab owns the pane's canvas
    await showLocalGraph(g, t);
    renderTabs(g);
    treeHighlight();
    return;
  }
  if (t && t.kind === "gg") {       // R9.7: global graph as a main tab
    cancelAnimationFrame(g.sim);    // clean restart on tab switches
    await startGraph(g, {
      fetch: () => inv("graph"),
      center: () => null,
      onClick: async n => {         // node click: this tab BECOMES the note
        const tt = g.active >= 0 ? g.tabs[g.active] : null;
        if (tt && tt.kind === "gg") { delete tt.kind; modeBits(tt); }   // #16: it is a NOTE tab now — give it the two mode bits
        await navigate(g, n);
      },
    });
    renderTabs(g);
    treeHighlight();
    return;
  }
  // R20 (#d): the tab's retained view is swapped in; read + render only on
  // first activation or when the file changed underneath (t.stale)
  const v = t ? viewOf(g, t) : g.view;
  attachView(g, v);
  showEditor(g);
  const n = curOf(g);
  setInlineTitle(v, n);              // #20: the rendered filename, before any body render
  if (n) mruTouch(n);                // m5: quick-switcher MRU order
  const m = t ? t.mode : "livepreview";
  if (!t || !v.loaded || t.stale || v.name !== n) {   // v.name: navigate() renames the tab in place
    Ed.setText(g, n ? await inv("read_note", { name: n }) : "");   // R17: load the MODEL
    v.name = n;
    if (t) { t.base = g.editor.value; t.stale = false; v.loaded = true; }  // R11: disk base
    if (m === "reading") await preview(g);
    else if (isLp(m)) await lpRender(g, -1, 0, true);
  } else if (v.scrollTop) g.lp.scrollTop = v.scrollTop;
  renderTabs(g);
  treeHighlight();
  updateStatus(g);                   // word/char count sync, backlinks async (not awaited: outside the action span)
  await lgFollow(g);                 // R7.3: linked localgraphs track this group
}

// R7.3: any localgraph tab linked to `src` re-centers on src's active note
async function lgFollow(src) {
  await rgFollow();                 // R9.6: right panel tracks the active note too
  const n = curOf(src);
  if (!n) return;
  for (const h of groups()) {
    const t = h.active >= 0 ? h.tabs[h.active] : null;
    if (!t || t.kind !== "lg" || t.linkId !== src.id || t.center === n) continue;
    t.center = n;
    t.name = "Graph of " + n.split("/").pop();
    h.graphSettled = false;         // census: old node coords are stale until the re-filtered sim settles
    renderTabs(h);
    if (h.graphRefresh) await h.graphRefresh();
    updateTitle();
  }
}

async function switchTab(g, i) {
  if (i === g.active) return;
  await act("tab_switch", { note: g.tabs[i].name, kind: g.tabs[i].kind || "note", tabs: g.tabs.length }, async () => {
    await flushSave(g);
    g.active = i;
    await loadActive(g);
  });
}

async function openInTab(name, via = "tab") {   // explorer click -> FOCUSED group (R6.3); via:"boot" = auto-open at startup (already inside the boot span)
  const g = fg();
  const lt = g.active >= 0 ? g.tabs[g.active] : null;
  if (lt && !lt.kind && lt.link != null && lt.name !== name) return navigate(g, name);  // R13.3: a linked member navigates in place
  await act("note_open", { note: name, via, tabs: g.tabs.length }, async sp => {
    await flushSave(g);
    const i = g.tabs.findIndex(x => x.name === name);
    if (i >= 0) g.active = i;
    else { g.tabs.push(mkTab(name)); g.active = g.tabs.length - 1; }
    await loadActive(g);
    Object.assign(sp.attrs, { mode: g.tabs[g.active].mode, bytes: g.editor.value.length, lines: g.lpLines || 0 });
  });
}

async function navigate(g, name, anchor) { // wikilink / graph click: replace g's ACTIVE tab, push history
  navInfo = "";
  await act("note_open", { note: name, via: "link", tabs: g.tabs.length }, async sp => {
    await flushSave(g);
    if (g.active < 0) { g.tabs.push(mkTab(name)); g.active = 0; }
    else {
      const tab = g.tabs[g.active];
      histPush(g, tab, name);
      tab.name = name;
    }
    await loadActive(g);
    Object.assign(sp.attrs, { mode: g.tabs[g.active].mode, bytes: g.editor.value.length, lines: g.lpLines || 0 });
  });
  await linkSync(g.tabs[g.active], name);   // R13.3: linked members follow
  if (anchor) await navAnchor(g, anchor);
}

/* R10.2: after opening the note, scroll to the #heading / #^block target and
   flash it ~2 s. LP centers the row (stock), reading/source put it at the
   top. Unresolvable anchor -> note stays at the top. [nav:<anchor>@<line>]
   in the census reports the line that was hit (headless probe). */
let navInfo = "";
function anchorLine(text, anchor) {  // 0-based source line of the target, or -1
  const L = text.split("\n");
  if (anchor.startsWith("^")) {
    const id = anchor.slice(1);
    return L.findIndex(l => l.trimEnd().endsWith(" ^" + id));
  }
  const want = anchor.trim().toLowerCase();
  for (const h of tocHeadsOf(text)) if (h.text.toLowerCase() === want) return h.line;
  return -1;
}
function tocHeadsOf(text) {          // cheap ATX scan (outline.rs is the source of truth for the pane)
  const out = []; let fence = false;
  text.split("\n").forEach((l, i) => {
    if (/^ {0,3}(```|~~~)/.test(l)) { fence = !fence; return; }
    const m = !fence && l.match(/^ {0,3}(#{1,6})\s+(.*?)\s*#*\s*$/);
    if (m) out.push({ level: m[1].length, line: i,
      text: m[2].replace(/\[\[([^\]]*)\]\]/g, (_, s) => s.split("|").pop().split("#")[0])
                 .replace(/[*_`~]/g, "").replace(/ \^[A-Za-z0-9-]+$/, "").trim() });
  });
  return out;
}
function flash(el) {
  if (!el) return;
  el.classList.add("navflash");
  setTimeout(() => el.classList.add("fade"), 400);
  setTimeout(() => el.classList.remove("navflash", "fade"), 2400);
}
async function navAnchor(g, anchor) {
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind) return;
  const line = anchorLine(g.editor.value, anchor);
  navInfo = "nav:" + anchor + "@" + line;
  if (line < 0) return updateTitle();
  if (isLp(t.mode)) {
    const row = g.lp.children[line];                // R17: one row per source line
    if (row) {
      g.lp.scrollTop = row.offsetTop - g.lp.offsetTop - (g.lp.clientHeight - row.offsetHeight) / 2;
      flash(row);
    }
    if (t.mode === "source") await lpEdit(g, line, 0);  // R12: source puts the caret on the line
  } else {
    let el = null;
    if (anchor.startsWith("^")) {
      const s = [...g.preview.querySelectorAll(".blockid")].find(x => x.dataset.bid === anchor.slice(1));
      el = s && s.closest("p,li,h1,h2,h3,h4,h5,h6,blockquote");
    } else {
      const k = tocHeadsOf(g.editor.value).findIndex(h => h.line === line);
      el = g.preview.querySelectorAll("h1,h2,h3,h4,h5,h6")[k];
    }
    if (el) { g.preview.scrollTop = el.offsetTop - g.preview.offsetTop; flash(el); }
  }
  tocSync();
  updateTitle();
}

/* R19 per-tab history (feedback #6/#6b): Back/Forward = mouse buttons 4/5,
   Alt+Left/Right, palette "Navigate back/forward". The target group is the
   one under the pointer (mouse) or the focused one (keys/palette); a local
   graph tab delegates to its LINKED group, whose history the graph clicks
   pushed (#6b) — the graph then re-centres through lgFollow like any nav.
   Restores the note + its scroll. Span history_nav: action -> note painted,
   or -> graph settled when a linked local graph had to re-centre. */
async function histGo(d, from) {
  let g = from || fg();
  if (!g || g.active < 0) return;
  let tab = g.tabs[g.active];
  if (tab.kind === "lg" && tab.linkId != null) {
    g = groups().find(x => x.id === tab.linkId);
    if (!g || g.active < 0) return;
    tab = g.tabs[g.active];
  }
  if (tab.kind) return;
  const p = tab.hpos + d;
  if (p < 0 || p >= tab.hist.length) return;
  const sp = otel.begin("history_nav", { dir: d < 0 ? "back" : "forward", from: tab.name, note: tab.hist[p].n, pos: p, len: tab.hist.length });
  await flushSave(g);
  tab.hist[tab.hpos].s = scrollOf(g);
  tab.hpos = p;
  tab.name = tab.hist[p].n;
  // R19 (HARD RULE 100ms): history_nav always ends at PAINT. A linked local graph
  // re-centres in loadActive -> lgFollow -> graphRefresh; its settling is animation and
  // is measured by that group's own graph_settle span, never by history_nav.
  const lgh = groups().find(h => { const t = h.active >= 0 ? h.tabs[h.active] : null; return t && t.kind === "lg" && t.linkId === g.id && t.center !== tab.name; });
  if (lgh) { otel.cancel(lgh.settleSp); lgh.settleSp = otel.begin("graph_settle", { node: tab.name, from: "history", nodes: 0 }); }
  await loadActive(g);
  const s = tab.hist[p].s;
  if (s) { if (tab.mode === "reading") g.preview.scrollTop = s; else if (isLp(tab.mode)) g.lp.scrollTop = s; else g.editor.scrollTop = s; }
  otel.paint(sp);
}
// mouse buttons 4/5 (X11 8/9 -> DOM button 3/4): nav fires on mousedown (capture) with
// preventDefault + stopPropagation so WebKit never turns them into webview history and no
// row/pane handler sees them; the matching auxclick/mouseup are swallowed the same way
function histBtn(e) { return e.button === 3 ? -1 : e.button === 4 ? 1 : 0; }
for (const ev of ["mousedown", "mouseup", "auxclick", "click"])
  document.addEventListener(ev, e => {
    const d = histBtn(e); if (!d) return;
    e.preventDefault(); e.stopPropagation();
    if (ev !== "mousedown") return;
    const pane = e.target && e.target.closest ? e.target.closest("#main .pane") : null;
    histGo(d, pane ? groups().find(h => h.pane === pane) : null);
  }, true);

async function closeTab(g, i) {
  const rm = g.tabs.length === 1 && groups().length > 1;      // R6.5: last tab -> the pane goes too
  await act("pane_close", { note: g.tabs[i].name, kind: g.tabs[i].kind || "note", pane_removed: rm, groups: groups().length, tabs: g.tabs.length - 1 }, async () => {
  if (i === g.active) await flushSave(g);
  if (!g.tabs[i].kind) closedTabs.push(g.tabs[i].name);   // R14 undo close tab
  unlinkTab(g, g.tabs[i], true);    // R13.4: closing a member unlinks it
  dropView(g.tabs[i]);              // R20: retained view goes with the tab
  g.tabs.splice(i, 1);
  if (!g.tabs.length && groups().length > 1)  // R6.5: empty group leaves the tree
    return collapseGroup(g);
  if (g.active >= g.tabs.length) g.active = g.tabs.length - 1;
  else if (i < g.active) g.active--;
  await loadActive(g);
  });
}

/* ---------- explorer tree ---------- */
let collapsed = new Set();

function buildTree(folders, notes) {
  const root = { dirs: new Map(), notes: [] };
  const dirAt = path => {
    let n = root;
    for (const part of path.split("/")) {
      if (!n.dirs.has(part)) n.dirs.set(part, { dirs: new Map(), notes: [] });
      n = n.dirs.get(part);
    }
    return n;
  };
  for (const f of folders) dirAt(f);
  for (const nm of notes) {
    const i = nm.lastIndexOf("/");
    (i < 0 ? root : dirAt(nm.slice(0, i))).notes.push(nm);
  }
  return root;
}

/* ux-2 obsidian parity: chevron rotates via .open, folder icon, indent-guide
   spans (.tg) instead of padding math. Row height fixed 31px in CSS — the
   full smoke suite clicks tree rows at pitch 31 (sub=82), do not change it. */
const TREE_CHEV =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"' +
  ' stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6"/></svg>';
const TREE_FOLDER =
  '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"' +
  ' stroke-linecap="round" stroke-linejoin="round">' +
  '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>';
const treeGuides = d => '<span class="tg"></span>'.repeat(d);

function renderNode(node, prefix, depth, out) {
  for (const d of [...node.dirs.keys()].sort()) {
    const full = prefix ? prefix + "/" + d : d;
    const open = !collapsed.has(full);
    const row = document.createElement("div");
    row.className = "trow folder" + (open ? " open" : "");
    row.innerHTML = treeGuides(depth) +
      '<span class="tc">' + TREE_CHEV + '</span>' +
      '<span class="tfi">' + TREE_FOLDER + '</span>' +
      '<span class="tn"></span>';
    row.querySelector(".tn").textContent = d;   // names never hit innerHTML
    // R20 (#7): children live in a .tkids box rendered ONCE; a click only flips
    // .collapsed on the box (+ .open on the row) — zero IPC, no tree rebuild.
    // The tree DOM is rebuilt only when the note/folder list changes (refreshTree).
    const kids = document.createElement("div");
    kids.className = "tkids" + (open ? "" : " collapsed");
    row.onclick = () => act("folder_toggle", { folder: full, open: collapsed.has(full), notes: node.dirs.get(d).notes.length, rows: kids.childElementCount }, () => {
      const o = collapsed.has(full);
      o ? collapsed.delete(full) : collapsed.add(full);
      row.classList.toggle("open", o);
      kids.classList.toggle("collapsed", !o);
    });
    out.appendChild(row);
    renderNode(node.dirs.get(d), full, depth + 1, kids);
    out.appendChild(kids);
  }
  for (const nm of [...node.notes].sort()) {
    const row = document.createElement("div");
    row.className = "trow note";                // .active: treeHighlight()
    row.innerHTML = treeGuides(depth) + '<span class="tc"></span>' +
      '<span class="tn"></span>';
    row.querySelector(".tn").textContent = nm.split("/").pop();
    row.onclick = () => openInTab(nm);
    row.oncontextmenu = e => noteMenu(e, nm);   // R9.4: bookmark toggle
    treeRows.set(nm, row);
    out.appendChild(row);
  }
}

// perf-index: with backlinks/search/graph served from RAM, rebuilding the
// 500-row explorer DOM on EVERY note open / tab switch became the largest
// remaining slice of note_open. The row set only changes when the folder or
// note list changes -> memoize on that signature and otherwise just move the
// .active highlight. R20: treeHighlight() is the zero-IPC half (tab switch /
// focus change); refreshTree() re-lists the vault (watcher, create, rename).
let treeSig = "", treeRows = new Map(), treeActive = null;
function treeHighlight() {
  const r = treeRows.get(cur()) || null;
  if (r === treeActive) return;
  if (treeActive) treeActive.classList.remove("active");
  if (r) r.classList.add("active");
  treeActive = r;
}
async function refreshTree() {
  const [folders, notes, imgs] =
    await Promise.all([inv("list_folders"), inv("list_notes"), inv("list_images")]);
  notesCache = notes;
  imgsCache = imgs;                                     // R29: LP resolves image embeds against this
  const tree = $("tree");
  const sig = folders.join("\n") + "\0" + notes.join("\n");
  if (sig === treeSig && tree.childElementCount) { treeHighlight(); return; }
  treeSig = sig; treeRows = new Map(); treeActive = null;
  tree.innerHTML = "";
  renderNode(buildTree(folders, notes), "", 0, tree);
  treeHighlight();
}

/* ---------- editor + preview (per group) ---------- */
/* R17 EDITOR CORE: the model/view engine lives in ui/editor.js (Ed).
   v.lines[] is the model, .lp is ONE contenteditable with one row per source
   line, a keystroke patches the dirty rows only, and the hidden textarea
   g.editor is written FROM the model (Ed.sync) purely as the save bridge.
   The functions below are the thin main.js seam the rest of the app calls. */

// lp pane render. `activeL` >= 0 puts the caret at (activeL, col) and reveals
// that row's markers; `full` rebuilds every row (mode switch / note switch).
let lpMs = -1;                 // last completed render duration (census probe)
let edtBad = -1;               // R17 self test: 0 = renderer + token map round-trip clean
async function lpRender(g, activeL = -1, col = 0, full = false) {
  const t0 = performance.now();
  Ed.render(g, activeL, col, full || (g.view && g.view.note !== curOf(g)));
  lpMs = Math.round(performance.now() - t0);
  updateTitle();
}
function lpCommit(g) { if (g && g.view && g.view.lines) Ed.sync(g); }   // model -> save bridge
async function lpMove(g, line, col, why) {     // R18 span kept: caret move -> paint
  return act("lp_commit", { why, from: g.lpActive ? g.lpActive.l0 : -1, to: line,
                            note_lines: g.lpLines || 0, note: curOf(g) || "" },
             () => { Ed.place(g, line, col); return Promise.resolve(); });
}
async function lpEdit(g, line, col) { await lpMove(g, line, col, "click"); }

// R20 (#8/#d): handlers resolve their group AT EVENT TIME from the DOM
// (pane._g) — a rendered view can move to another group (tab drag / split).
const gOf = el => { const p = el.closest(".pane"); return p ? p._g : null; };

/* R8.7 wikilink click: navigate in place, ctrl+click opens a new tab.
   Mousedown level + stopPropagation so the caret is not moved into the row. */
async function wikiClick(e, a) {
  e.preventDefault(); e.stopPropagation();
  const g = gOf(a);
  if (!g) return;
  const n = a.dataset.note || curOf(g), an = a.dataset.anchor;   // R10: [[#H]] = this note
  if (a.classList.contains("wiki-unresolved")) await createNote(n);
  if (e.ctrlKey) {                            // R6.4: new tab, same group
    await flushSave(g);
    g.tabs.push(mkTab(n));
    g.active = g.tabs.length - 1;
    await loadActive(g);
    if (an) await navAnchor(g, an);
  } else navigate(g, n, an);
}
// S1 external link, live-preview side: SWALLOW only. The mousedown must not
// open the raw row under the link, and the webview must never navigate.
// Routing to the desktop browser has exactly ONE choke point — the capture-phase
// a.ext gate above, which preventDefaults both mousedown and click, counts
// [ext:N] and hands the HREF (only ever http(s) here; every other scheme renders
// with href="#") to open_external, where Rust check_external re-checks the
// scheme. This handler deliberately does NOT route: a second path would open the
// browser TWICE for one click, and it could not restore safety anyway (without
// the gate the anchor's own default navigation is what fires).
// It used to end with `if (a.dataset.url) openExt(a.dataset.url)` and openExt was
// DEFINED NOWHERE (introduced by 7cc7cb5, shipped in v0.6): a ReferenceError on a
// path that only ever ran for non-ext links, and only because the capture gate
// stops a.ext events before they reach here. Deleted rather than defined — a
// dangerous scheme (javascript:) is now unreachable by construction, not by
// listener ordering.
function extClick(e, a) {   // `a` unused: kept for the Ed.extClick(e, a) signature
  e.preventDefault(); e.stopPropagation();
}
async function preview(g) {
  const src = g.editor.value;
  g.preview.innerHTML = await inv("render", { content: src });
  // R35: the block -> source line map for THIS html, from the renderer's own
  // parser. Stored next to the html it describes and re-read on every render:
  // a stale map would scroll the reading view to the wrong block, which is
  // exactly the silent wrongness this feature exists to avoid (ssAnchor /
  // ssRestore refuse to act when its length does not match #preview's).
  g.pvLines = await inv("block_lines", { content: src });
  for (const a of g.preview.querySelectorAll("a.tag"))
    a.onclick = e => { e.preventDefault(); tagSearch(a.dataset.tag); };
  for (const a of g.preview.querySelectorAll("a.wiki"))
    a.onclick = async e => {
      e.preventDefault();
      const g = gOf(a);                              // R20: event-time group (retained views move)
      const n = a.dataset.note || curOf(g), an = a.dataset.anchor;  // R10: [[#H]] = this note
      if (a.classList.contains("wiki-unresolved"))   // R3.5: click creates the note
        await createNote(n);
      if (e.ctrlKey) {                               // R6.4: open in NEW TAB, same group
        await flushSave(g);
        g.tabs.push(mkTab(n));
        g.active = g.tabs.length - 1;
        await loadActive(g);
        if (an) await navAnchor(g, an);
      } else navigate(g, n, an);
    };
}

function scheduleSave(g) {
  clearTimeout(g.saveT);
  g.saveT = setTimeout(async () => {
    g.saveT = null;
    await saveBuf(g);                       // R11.3: merge an external append instead of clobbering it
    // R18: only the READING pane needs the Rust renderer. This used to call
    // preview() on every debounced save, i.e. one full-note render IPC per
    // typing burst, into a #preview that is display:none in lp/source mode.
    // Entering reading mode renders it anyway (setMode / openNote / restore),
    // so the hidden refresh bought nothing and put Rust on the typing path.
    if (isReading(g)) preview(g);
    updateStatus(g);
  }, SAVE_MS);
}

/* ---------- status bar (R2.7, per group) ---------- */
async function updateStatus(g) {
  const n = curOf(g);
  if (!n || g.graphOn) { g.status.hidden = true; return; }
  const v = g.editor.value;
  const w = (v.match(/\S+/g) || []).length;
  g.stWc.textContent = w + (w === 1 ? " word" : " words");
  g.stCc.textContent = v.length + (v.length === 1 ? " char" : " chars");
  const bl = await inv("backlinks", { name: n });
  g.stBl.textContent = bl.length + (bl.length === 1 ? " backlink" : " backlinks");
  g.status.hidden = false;
}

/* ---------- [[ autocomplete (R3.4) ---------- */
let notesCache = [], acItems = [], acSel = 0, acStart = -1, acKind = "n";
/* R29: the live-preview engine's image list — the SAME list the Rust renderer
   resolves against (`list_images` hands out `Index::images()` verbatim), so the
   two engines cannot disagree about what `![[pic.png]]` names (R29.7). Loaded
   with notesCache in refreshTree(); R29.11 (an image added while the vault is
   open) is a known gap on BOTH sides, not a JS one. */
let imgsCache = [];

function hideAc() {
  const was = acItems.length;
  for (const g of groups()) g.acEl.hidden = true;
  acItems = []; acStart = -1;
  if (was) updateTitle();
}

function fuzzy(q, s) {  // lower score = better; -1 = no match
  q = q.toLowerCase();
  const l = s.toLowerCase();
  if (!q) return 0;
  const i = l.indexOf(q);
  if (i >= 0) return i;                  // substring: rank by position
  let pos = 0;                           // subsequence fallback
  for (const c of q) {
    pos = l.indexOf(c, pos);
    if (pos < 0) return -1;
    pos++;
  }
  return 1000;
}

// R12: the field being typed in — the lp raw row when one is open (source +
// live preview both edit there now), else the hidden model textarea
const acField = g => (g.view && g.view.lines ? Ed.field(g) : g.editor);   // R17: a textarea-shaped view of the model
function acContext(g) {  // caret inside an unclosed [[ on one line?
  const ed = acField(g);
  const upto = ed.value.slice(0, ed.selectionStart);
  const a = upto.lastIndexOf("[[");
  if (a < 0) return null;
  const frag = upto.slice(a + 2);
  if (frag.includes("]]") || frag.includes("\n")) return null;
  return { start: a, q: frag };
}

function caretXY(g) {    // R17: the real caret rect, relative to the pane
  return Ed.caretXY(g);
}

async function showAc(g) {
  const ctx = acContext(g);
  if (!ctx) return hideAc();
  const hi = ctx.q.indexOf("#");
  if (hi >= 0) {
    // R10.4: [[note#  -> that note's headings (level badge); [[note#^ -> its
    // existing block ids. [[#  = the current note. Id generation for id-less
    // blocks is LATER.
    const note = ctx.q.slice(0, hi), frag = ctx.q.slice(hi + 1);
    const target = note ? notesCache.find(n => n === note || n.endsWith("/" + note)) : curOf(g);
    if (!target) return hideAc();
    const text = target === curOf(g) ? g.editor.value : await inv("read_note", { name: target });
    const now = acContext(g);                 // keystrokes raced the read: stale, let the next call draw
    if (!now || now.q !== ctx.q) return;
    if (frag.startsWith("^")) {
      acKind = "b";
      acItems = text.split("\n")
        .map(l => (l.trimEnd().match(/ \^([A-Za-z0-9-]+)$/) || [])[1]).filter(Boolean)
        .filter(id => fuzzy(frag.slice(1), id) >= 0)
        .map(id => ({ ins: note + "#^" + id, label: "^" + id, badge: "" }));
    } else {
      acKind = "h";
      acItems = tocHeadsOf(text).filter(h => fuzzy(frag, h.text) >= 0)
        .map(h => ({ ins: note + "#" + h.text, label: h.text, badge: "H" + h.level }));
    }
    acItems = acItems.slice(0, 8);
  } else {
    acKind = "n";
    acItems = notesCache
      .map(n => [fuzzy(ctx.q, n), n])
      .filter(([s]) => s >= 0)
      .sort((a, b) => a[0] - b[0] || a[1].localeCompare(b[1]))
      .slice(0, 8)
      .map(([, n]) => ({ ins: n, label: n, badge: "" }));
  }
  acStart = ctx.start;
  if (!acItems.length) return hideAc();
  acSel = 0;
  const box = g.acEl;
  box.innerHTML = "";
  acItems.forEach((it, i) => {
    const d = document.createElement("div");
    d.textContent = it.label;
    if (it.badge) { const b = document.createElement("span"); b.className = "acb"; b.textContent = it.badge; d.appendChild(b); }
    if (i === acSel) d.className = "sel";
    d.onmousedown = e => { e.preventDefault(); acInsert(g, it.ins); };
    box.appendChild(d);
  });
  const [x, y] = caretXY(g);
  box.style.left = Math.max(0, Math.min(x, g.content.clientWidth - 200)) + "px";
  box.style.top = Math.min(y, g.content.clientHeight - 60) + "px";
  box.hidden = false;
  updateTitle();
}

function acInsert(g, name) {   // R17: the [[ completion goes through the model
  Ed.acInsert(g, acStart, name);
  hideAc();
}

function acKeydown(g, e) {
  if (g.acEl.hidden) return;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault(); e.stopPropagation();
    acSel = (acSel + (e.key === "ArrowDown" ? 1 : acItems.length - 1)) % acItems.length;
    [...g.acEl.children].forEach((d, i) => d.className = i === acSel ? "sel" : "");
  } else if (e.key === "Enter" || e.key === "Tab") {
    e.preventDefault(); e.stopPropagation();
    acInsert(g, acItems[acSel].ins);
  } else if (e.key === "Escape") {
    e.stopPropagation();
    hideAc();
  }
}

/* ---------- commands + keymap (target the focused group) ---------- */
async function cmdNewNote() {                // new note in a NEW tab of the focused group
  if (!state) return;
  const g = fg();
  await flushSave(g);
  // F4: "Untitled-" + Date.now() % 10000 repeats exactly every 10s, so the old
  // create could silently eat a note from ten seconds ago. create_note REFUSES;
  // on a collision we pick another name instead of clobbering.
  let name = "Untitled-" + Date.now() % 10000;
  let r = await createNote(name);
  for (let i = 2; r === "exists" && i < 100; i++) {
    name = "Untitled-" + Date.now() % 10000 + "-" + i;
    r = await createNote(name);
  }
  if (r !== "ok") return;                    // no tab for a note that is not on disk
  g.tabs.push(mkTab(name));
  g.active = g.tabs.length - 1;
  await loadActive(g);
}
async function cmdSave() {                   // force save, no debounce
  if (!state) return;
  const g = fg();
  lpCommit(g);
  const n = curOf(g);
  if (!n) return;
  clearTimeout(g.saveT); g.saveT = null;
  await saveBuf(g);
  await preview(g);
  await updateStatus(g);
}
async function cmdCloseTab() {
  if (state && fg().active >= 0) await closeTab(fg(), fg().active);
}
async function cmdNewTab() {       // R5.3 ctrl+t — v1: new tab on the current note
  if (!state) return;              // (no empty-tab state yet; graph tabs no-op)
  const g = fg();
  const n = curOf(g);
  if (!n) return;
  await flushSave(g);
  g.tabs.push(mkTab(n));
  g.active = g.tabs.length - 1;
  await loadActive(g);
}
async function cmdCycleTab(d) {    // R5.3 ctrl+(shift+)tab — cycle in focused group
  const g = fg();
  const len = g.tabs.length;
  if (len < 2) return;
  await switchTab(g, (g.active + d + len) % len);
}
async function cmdJumpTab(n) {     // R5.3 ctrl+1..9 — tab n; 9 = last
  const g = fg();
  const len = g.tabs.length;
  if (!len) return;
  const i = n === 9 ? len - 1 : n - 1;
  if (i < len) await switchTab(g, i);
}

/* ---------- m5: shared fuzzy modal — quick switcher (Ctrl+O) + command
   palette (Ctrl+P). One component; item source decides the kind.
   census [modal:qs|cp] while open. ---------- */
let modalKind = null, mdItems = [], mdSel = 0, mdSrc = null;
let mruList = [];                    // per-session note-use order, newest first
function mruTouch(name) {
  const i = mruList.indexOf(name);
  if (i >= 0) mruList.splice(i, 1);
  mruList.unshift(name);
}
function qsItems() {                 // switcher source: MRU first, rest a-z
  const rest = notesCache.filter(n => !mruList.includes(n)).sort();
  return [...mruList.filter(n => notesCache.includes(n)), ...rest].map(n => ({
    label: n, hint: "",
    run: async ev => {               // Enter = focused group; Ctrl+Enter = new tab
      if (ev && ev.ctrlKey) {
        const g = fg();
        g.tabs.push(mkTab(n)); g.active = g.tabs.length - 1;
        await loadActive(g);
      } else await openInTab(n);
    },
  }));
}
function openModal(kind, src) {
  modalKind = kind; mdSrc = src;
  $("minput").value = "";
  $("minput").placeholder = kind === "qs" ? "Open note..." : "Run command...";
  $("modal").hidden = false;
  mdFilter();
  $("minput").focus();
  updateTitle();
}
function closeModal() {
  if (!modalKind) return;
  modalKind = null;
  mdNew = "";
  $("modal").hidden = true;
  updateTitle();
}
function mdFilter() {
  const q = $("minput").value.trim();
  mdItems = mdSrc().map((it, i) => [fuzzy(q, it.label), i, it])
    .filter(([s]) => s >= 0)
    .sort((a, b) => a[0] - b[0] || a[1] - b[1])  // ties keep source (MRU) order
    .slice(0, 10).map(([, , it]) => it);
  mdSel = 0;
  renderModal();
}
/* C4 (R5.1): the switcher's dead end. A query that matches no note used to
   paint the single dead word "No matches" — the only move left was Escape and
   Ctrl+N with the name retyped. Stock creates the note on Shift+Enter and opens
   it, so offer that action here.
   GATED ON kind "qs": renderModal() is SHARED with the COMMAND PALETTE
   ([modal:cp], mdSrc = cmdItems), where "create a note" is not an answer to an
   unknown COMMAND. mdNew is the affordance's name ("" = not offered) and is
   published as [mdnew:<name>] so the smoke asserts the affordance itself, not
   an OCR of its label. */
let mdNew = "";
/* Creation from the switcher goes through the SHARED createNote helper (whose
   body is now EMPTY — feedback #20: a new note is a zero-byte file and the big
   title is the filename, rendered) — there is deliberately no second creation
   path. It CANNOT overwrite: the backend uses create_new(2),
   the kernel's atomic exists-check, and createNote maps that to "exists" and
   returns without writing a byte.
   The real hazard here is CASE. fuzzy() lowercases both sides, so "ideas"
   matches "Ideas" and the affordance never appears — but on a case-sensitive
   filesystem createNote("ideas") would cheerfully lay a SECOND file down next
   to Ideas.md and open the empty one, which reads as "my note lost its
   contents". So resolve the typed name against the tree case-insensitively
   FIRST and open the note that is already there. */
async function qsCreateNote(name) {
  const hit = notesCache.find(n => n.toLowerCase() === name.toLowerCase());
  if (hit) return await openInTab(hit);
  const r = await createNote(name);
  if (r === "err") return;              // no tab for a note that is not on disk
  await openInTab(name);                // "exists" (racing writer) -> open it, untouched
}
function renderModal() {
  const box = $("mlist");
  box.innerHTML = "";
  const nq = modalKind === "qs" ? $("minput").value.trim() : "";
  mdNew = !mdItems.length && nq ? nq : "";
  if (!mdItems.length) {
    const d = document.createElement("div");
    if (mdNew) {
      d.className = "mrow sel";
      const l = document.createElement("span"); l.textContent = 'Create "' + mdNew + '"';
      const h = document.createElement("span"); h.className = "mhint"; h.textContent = "Shift+Enter";
      d.append(l, h);
      d.onmousedown = async e => { e.preventDefault(); const n = mdNew; closeModal(); await qsCreateNote(n); };
    } else { d.className = "mempty"; d.textContent = "No matches"; }
    box.appendChild(d);
    return updateTitle();
  }
  mdItems.forEach((it, i) => {
    const d = document.createElement("div");
    d.className = "mrow" + (i === mdSel ? " sel" : "");
    const l = document.createElement("span"); l.textContent = it.label;
    const h = document.createElement("span"); h.className = "mhint";
    h.textContent = it.hint || "";
    d.append(l, h);
    d.onmousedown = async e => { e.preventDefault(); closeModal(); await it.run(e); };
    box.appendChild(d);
  });
  updateTitle();   // C4: mdFilter() runs on every keystroke — republish [mdnew:]
}
$("minput").oninput = mdFilter;
$("minput").onkeydown = async e => {
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    if (!mdItems.length) return;
    mdSel = (mdSel + (e.key === "ArrowDown" ? 1 : mdItems.length - 1)) % mdItems.length;
    renderModal();
  } else if (e.key === "Enter") {
    e.preventDefault();
    const it = mdItems[mdSel];
    if (it) { closeModal(); return await it.run(e); }
    // C4: zero matches + a non-empty switcher query -> Shift+Enter creates it.
    // Only when there is nothing to pick: with a match present (incl. a
    // case-differing one) Shift+Enter must never become a creation.
    if (e.shiftKey && mdNew) { const n = mdNew; closeModal(); await qsCreateNote(n); }
  }
};
$("modal").onmousedown = e => { if (e.target === $("modal")) closeModal(); };
function cmdQuickSwitch() {
  modalKind === "qs" ? closeModal() : openModal("qs", qsItems);
}
/* ---------- R14: command registry = the ONE source of truth for the palette
   (Ctrl+P), the keymap dispatcher and Settings ▸ Hotkeys. Chords are
   normalised "ctrl+alt+shift+<key>" (e.key lowercased, Mod = Ctrl on Linux).
   User overrides live in hkUser {id: [chords]} ([] = default removed) and are
   persisted to ~/.rustidian.json "hotkeys" in the stock Obsidian shape. ---------- */
const edField = () => (state && fg() ? acField(fg()) : null);
function edEdit(fn) {   // fn(value, selStart, selEnd) -> [value, selStart, selEnd]; fires input
  const ta = edField();
  const _t = fg() && fg().active >= 0 ? fg().tabs[fg().active] : null;
  if (!ta || !_t || _t.kind || _t.mode === "reading") return;
  const r = fn(ta.value, ta.selectionStart, ta.selectionEnd);
  if (!r) return;
  ta.value = r[0]; ta.setSelectionRange(r[1], r[2]);
  ta.dispatchEvent(new Event("input", { bubbles: true }));
  ta.focus();
}
const edWrap = (m, ph = "text") => edEdit((v, a, b) => {      // toggle **x** / *x* / %%x%%
  const s = v.slice(a, b);
  if (s.startsWith(m) && s.endsWith(m) && s.length >= 2 * m.length)
    return [v.slice(0, a) + s.slice(m.length, -m.length) + v.slice(b), a, b - 2 * m.length];
  if (v.slice(a - m.length, a) === m && v.slice(b, b + m.length) === m)
    return [v.slice(0, a - m.length) + s + v.slice(b + m.length), a - m.length, b - m.length];
  const t = s || ph;
  return [v.slice(0, a) + m + t + m + v.slice(b), a + m.length, a + m.length + t.length];
});
const edLine = fn => edEdit((v, a, b) => {  // fn(line) -> line | null (= delete line)
  const l0 = v.lastIndexOf("\n", a - 1) + 1;
  let l1 = v.indexOf("\n", b); if (l1 < 0) l1 = v.length;
  const nl = fn(v.slice(l0, l1));
  if (nl == null) { const cut = l1 < v.length ? l1 + 1 : Math.max(0, l0 - 1); return [v.slice(0, l0) + v.slice(l1 + 1), Math.min(l0, cut), Math.min(l0, cut)]; }
  return [v.slice(0, l0) + nl + v.slice(l1), l0 + Math.min(nl.length, a - l0 + nl.length - (l1 - l0)), l0 + Math.min(nl.length, b - l0 + nl.length - (l1 - l0))];
});
// R17.4 M46-M51 "Toggle checkbox status": a MODEL op (one row patched, stock
// caret rule), not a whole-document rewrite through the textarea shim.
function edTask() {
  const g = state && fg();
  const t = g && g.active >= 0 ? g.tabs[g.active] : null;
  if (!g || !t || t.kind || t.mode === "reading") return;
  const s = Ed.sel(g);
  if (s) Ed.toggleCheck(g, s);
}
function linkAtCaret() {   // [[target]] spanning the caret of the edited field, note part only
  const ta = edField();
  if (!ta) return null;
  const v = ta.value, c = ta.selectionStart;
  const a = v.lastIndexOf("[[", c);
  if (a < 0) return null;
  const b = v.indexOf("]]", a);
  if (b < 0 || c > b + 1) return null;
  const inner = v.slice(a + 2, b);
  return inner.split("|")[0].split("#")[0].trim() || null;
}
/* ---------- R31 drop-to-attach (feedback #18) ---------------------------
   A real OS drop NEVER reaches the DOM: wry takes the XDND at the GTK layer
   and Tauri hands it to Rust as WindowEvent::DragDrop, which re-emits the
   paths as `drop-files` (main.rs R31.1). So this listener is the drop, and a
   `dragover`/`drop` handler on the editor would be decoration.
   This half decides WHERE text goes — the caret of the focused editor — and
   nothing else: the vault write, the file name, the link text and every
   refusal sentence come back from attach_files -> attach_drop. Anything that
   looks like policy down here would be a second copy of the rules.  */
function dropSay(msg) {           // R31.5 a refusal is VISIBLE or it is a bug report
  const b = $("dropmsg");
  b.textContent = msg;
  b.hidden = !msg;
  clearTimeout(dropT);
  // the banner is transient, the census token is NOT: it is rewritten by the
  // next drop and never expires, so a probe cannot read "no drop happened"
  // off a banner that merely timed out.
  if (msg) dropT = setTimeout(() => { $("dropmsg").hidden = true; }, 4000);
}
/* R31.6 READING VIEW HAS NO CARET, so a drop there cannot "insert at the
   cursor". Stock's behaviour is UNVERIFIED (recon could not drive a drop, and
   the paste oracle is edit-only), and the brief's rule for an unverified
   answer is: refuse, visibly. Copying the file in anyway and inserting it
   somewhere we guessed is the worse failure — it writes to the vault for an
   action the user cannot see the result of. */
async function attachDrop(paths) {
  if (!state || !fg()) return dropSay("open a note first");
  const g = fg(), t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind) return dropSay("open a note first");
  if (t.mode === "reading") { dropTok = "ro"; dropSay("switch to editing (Ctrl+E) to attach a file"); return updateTitle(); }
  let a;
  try { a = await inv("attach_files", { note: t.name, paths }); }
  catch (e) { dropTok = "err"; dropSay(errStr(e)); return updateTitle(); }
  // R29.11: both renderers resolve ![[x.png]] against the index list, and the
  // LP engine reads its own copy (imgsCache) — a file copied but not re-listed
  // paints as a MISSING image. Refresh BEFORE the insert so the first render
  // of the new link already resolves.
  if (a.copied.length) await refreshTree();
  if (a.text) edEdit((v, s, e2) => [v.slice(0, s) + a.text + v.slice(e2), s + a.text.length, s + a.text.length]);
  dropTok = a.copied.length + "/" + a.refused.length;
  dropSay(a.refused.join("\n"));
  updateTitle();
}
/* R31.7 "Insert attachment" — the keyboard road to the SAME attach a drop
   takes. It exists for two reasons, in this order: an OS drop is unreachable
   without a pointer (and unreachable to any automated probe — xdotool has no
   XDND, brief §5), and the smoke must drive the real path, not a JS fake.
   It adds NO policy: it collects a path string and hands it to attachDrop,
   which is the same entry point the `drop-files` event calls. */
function cmdAttach() {
  const box = $("anew");
  box.hidden = !box.hidden;
  if (!box.hidden) { $("apath").value = ""; $("apath").focus(); }
  updateTitle();
}
function closeAttach() { $("anew").hidden = true; updateTitle(); }
/* ---------- THEME: ONE attribute, on ONE root ---------------------------
   Every colour in the app is a token defined in the block at the top of
   ui/style.css; `light` is that same token block re-stated under
   :root[data-theme="light"]. So switching the theme is exactly one DOM write:
   the attribute on document.documentElement. That is deliberate and it is the
   whole design —
     - panes, modals, the graph canvas and the settings modal all inherit their
       colours from the same :root custom properties, so none of them needs to
       know a theme exists. A per-pane or per-container attribute would be a
       second source of truth and would leave whichever container was forgotten
       painting the old theme (negative control N2 drives exactly that).
     - the graph reads its colours out of getComputedStyle(documentElement)
       (commit 2a0f7ba), so it follows the same attribute with no extra wiring.
   themeMode is the in-memory copy; persistence and the system default are a
   separate concern and land on top of applyTheme(), not inside it. */
const MQ_DARK = matchMedia("(prefers-color-scheme: dark)");
/* THE SYSTEM SIGNAL. Nine measured runs (notes/theme/detect-recon.md): the tauri
   2.5 Window::theme() answers Light on this platform no matter what the desktop
   says (a stub), and its JS half throws; prefers-color-scheme is the only reading
   that actually MOVED when the desktop moved, in both directions. Caveat recorded
   there and true here: WebKitGTK never reports no-preference, so "light" also
   means "nothing is configured" — which is why the STORED choice, not the media
   query, is what a user's decision is kept in. */
const systemTheme = () => (MQ_DARK.matches ? "dark" : "light");
let themeMode = "dark";                    // "dark" | "light" — replaced at boot
let themeStored = false;                   // a user chose: the system stops deciding
function applyTheme(t) {
  themeMode = t === "light" ? "light" : "dark";
  document.documentElement.setAttribute("data-theme", themeMode);
  if (state) updateTitle();                // census [theme:<mode>] follows the DOM
                                           // (before a vault is open there is no
                                           // title census to refresh — updateTitle
                                           // reads state.root)
}
/* a USER choice: apply it, and remember it. Persisted through the EXISTING
   settings store (~/.rustidian.json, key "theme" — main.rs set_theme), the same
   file sidebar_w / rside_tab / hotkeys already live in. Fire-and-forget: a failed
   write must not undo the theme the user is looking at. */
function chooseTheme(t) {
  applyTheme(t);
  themeStored = true;
  inv("set_theme", { theme: themeMode }).catch(() => {});
}
function cmdToggleTheme() { chooseTheme(themeMode === "dark" ? "light" : "dark"); }
/* LIVE system change, documented behaviour: the desktop flipping light<->dark
   moves the app ONLY while the user has made no choice of their own. Once they
   have chosen, their choice outranks the desktop until they change it — the
   stored value is never silently overwritten by a system event. */
if (MQ_DARK.addEventListener) {
  MQ_DARK.addEventListener("change", () => { if (!themeStored) applyTheme(systemTheme()); });
} else if (MQ_DARK.addListener) {          // older WebKit spelling
  MQ_DARK.addListener(() => { if (!themeStored) applyTheme(systemTheme()); });
}
/* BOOT ORDER, and why it is this way: the system signal is SYNCHRONOUS, the
   stored choice is an IPC round trip. So paint the system default first (the root
   attribute exists before webkit's first frame — no flash of the wrong theme on
   the common path where the two agree), then let the stored choice, if there is
   one, replace it as the first await of the boot sequence, before any vault
   content is on screen. */
async function bootTheme() {
  applyTheme(systemTheme());
  const st = await inv("get_theme").catch(() => null);
  if (st === "dark" || st === "light") { themeStored = true; applyTheme(st); }
}
/* ---------- PALETTE: the SECOND axis, on the SAME root -------------------
   Mode (above) answers "light or dark". Palette answers "light or dark OF
   WHAT". They are independent and they compose:

       <html data-palette="1984" data-theme="light">

   and ui/style.css has a block per (palette, mode) pair, so flipping either
   attribute re-resolves every token in the app through the same one mechanism
   applyTheme already uses. Nothing else in the app learns that palettes exist.

   THE DEFAULT IS THE ABSENCE OF THE ATTRIBUTE, not a value of it. A user who
   has chosen no palette has no data-palette on the root, so no palette
   selector can match and the pixels are byte-for-byte the ones this app
   painted before the axis existed. "default" as an attribute VALUE would mean
   a second definition of the default, free to drift from the first.

   WHY NOT REUSE THE "theme" KEY. Because "theme" absent means "the user has
   chosen no MODE, let prefers-color-scheme decide". Writing a palette name
   there would destroy that state, and the system default — the thing a user
   who never opens settings relies on — would break the moment anyone picked a
   palette. Two axes, two keys. src-tauri/src/palette.rs is the table both
   sides agree on; a rust test pins THIS list to it. */
const PALETTES = [["default", "Default"], ["1984", "1984"]];
const DEFAULT_PALETTE = PALETTES[0][0];
const paletteKnown = p => PALETTES.some(([id]) => id === p);
/* an unknown name is not an error to surface, it is the default to paint —
   the same fallback main.rs::get_palette applies on the way out of the file */
const resolvePalette = p => (paletteKnown(p) ? p : DEFAULT_PALETTE);
let themePalette = DEFAULT_PALETTE;
function applyPalette(p) {
  themePalette = resolvePalette(p);
  const root = document.documentElement;
  if (themePalette === DEFAULT_PALETTE) root.removeAttribute("data-palette");
  else root.setAttribute("data-palette", themePalette);
  if (settingsOpen) renderPaletteCtl();     // the settings dropdown follows the state
  if (state) updateTitle();                 // census [palette:<id>] follows the DOM
}
/* a USER choice. Refused names never reach applyPalette's DOM write as
   themselves and are never sent to the backend either — which matters because
   the backend refusing silently and the frontend painting it anyway would look
   to a user exactly like a palette that works until you restart. */
function choosePalette(p) {
  const want = resolvePalette(p);
  applyPalette(want);
  inv("set_palette", { palette: want }).catch(() => {});
}
/* BOOT: same order and same reasoning as bootTheme — nothing synchronous to
   paint first (no palette IS the default), then the stored choice replaces it
   before any vault content is on screen. get_palette answers null for absent,
   junk, or an explicitly-stored default, so all three land on the default. */
async function bootPalette() {
  const st = await inv("get_palette").catch(() => null);
  if (st && paletteKnown(st)) applyPalette(st);
}
function cmdSetPalette(id) { return () => choosePalette(id); }
/* ---------- R36 interface zoom (Ctrl+= / Ctrl+- / Ctrl+0) ---------------
   There is deliberately NO CSS in this function. The scale is applied by
   webkit_web_view_set_zoom_level through the `zoom` command (main.rs R36.1),
   which changes what a CSS pixel IS — so the sidebar's 200px, the ribbon's
   44px, the tab strip's height, every icon and the text all scale by the one
   factor, together. Scaling `font-size`/rem here instead would move the text
   and leave the chrome behind: that is the text-only result the operator
   rejected, it is committed as negative control N1 (docs/negctl-zoom/), and
   `scripts/smoke.sh fast zoom` measures three non-text boundaries so it goes
   red.
   The step, the clamps and the level->factor math are NOT duplicated here:
   this function knows three verbs and nothing else, so the palette row and
   the hotkey cannot drift from each other or from the backend. */
let zoomTok = "1.0000@0";                  // [zoom:<factor>@<level>] census — the app's own belief
async function cmdZoom(action) {
  try {
    const z = await inv("zoom", { action });
    // the CLAMP is part of the census: pressing into the ceiling and the key
    // never arriving look identical in the factor alone, and telling those two
    // apart is exactly what the clamp assertions in the smoke phase need.
    zoomTok = z.factor.toFixed(4) + "@" + z.level + (z.clamped ? "!" : "");
  } catch (e) {
    zoomTok = "err:" + errStr(e).replace(/[[\]|]/g, "").slice(0, 40);
  }
  updateTitle();
}
/* R36.4 the census must not LIE after a restore. The persisted level is applied
   in Rust, inside setup(), BEFORE the first paint (main.rs R36.4) — so the
   webview can come up at 1.7280 while this file's optimistic default still
   reads "1.0000@0", and a probe would call a restored zoom "still at 100%".
   Ask the backend once at startup instead of assuming. `zoom_get` returns the
   SAME shape as `zoom`, so the base and the step are still in exactly one file.
   Deliberately NOT on the awaited boot path (enterVault): it is one IPC that
   nothing on screen waits for, and the census is republished by the first real
   updateTitle anyway. The restart assertion in `scripts/smoke.sh fast zoom` is
   what notices if this line is ever deleted. */
inv("zoom_get").then(z => {
  zoomTok = z.factor.toFixed(4) + "@" + z.level;
  if (typeof state !== "undefined" && state) updateTitle();
}, () => { /* a backend that cannot answer is not a reason to blank the census */ });
let closedTabs = [];                       // R14 undo close tab (names, newest last)
const CMDS = [
  ["app:open-settings",        "Open settings",                       ["ctrl+,"],               () => cmdSettings()],
  // R14: the theme switch is a registry entry like any other — no bespoke
  // keystroke, no menu item of its own. It ships with NO default chord on
  // purpose: the palette is the road, so the smoke phase has to drive the real
  // Ctrl+P path (and a user can still bind a chord in Settings ▸ Hotkeys,
  // which works for free because this is in the one registry).
  ["theme:switch",             "Toggle light/dark mode",              [],                       cmdToggleTheme],
  /* THE PALETTE AXIS IN THE PALETTE (goal/theme-1984). One registry entry per
     shipped palette rather than one cycling command, for two reasons:
       - a cycle is not addressable. "Use theme: 1984" lands on 1984 from any
         starting state; a cycler lands somewhere that depends on where you
         were, which is exactly what a user searching a command palette is
         trying not to think about. It is also why the smoke phase can assert
         a RESULT instead of a sequence.
       - it is what the list is for. Adding a palette to PALETTES adds its
         command here for free, with no third place to forget.
     No default chord, same reasoning as theme:switch above: Ctrl+P IS the road,
     and a chord can still be bound in Settings ▸ Hotkeys because these are
     ordinary registry entries. The names are distinct prefixes of each other's
     complement, so the palette's fuzzy match resolves each one uniquely. */
  ...PALETTES.map(([id, label]) => [
    "theme:palette:" + id,
    "Use theme: " + label,
    [],
    cmdSetPalette(id),
  ]),
  ["workspace:close",          "Close current tab",                   ["ctrl+w"],               cmdCloseTab],
  ["window:close",             "Close window",                        ["ctrl+shift+w"],         () => window.__TAURI__.window.getCurrentWindow().close()],
  ["command-palette:open",     "Open command palette",                ["ctrl+p"],               () => cmdPalette()],
  ["file-explorer:new-file",   "Create new note",                     ["ctrl+n"],               cmdNewNote],
  ["file-explorer:new-file-in-new-pane", "Create note to the right",  ["ctrl+shift+n"],         async () => { await splitWith(fg(), "row", null); await cmdNewNote(); }],
  ["file-explorer:new-folder", "Create new folder",                   [],                       () => { $("fnew").hidden = false; $("fname").value = ""; $("fname").focus(); }],
  ["editor:delete-paragraph",  "Delete paragraph",                    ["ctrl+d"],               () => edLine(() => null)],
  ["editor:insert-attachment", "Insert attachment",                   [],                       cmdAttach],
  ["editor:follow-link",       "Follow link under cursor",            ["alt+enter"],            () => { const n = linkAtCaret(); if (n) navigate(fg(), n); }],
  ["workspace:goto-last-tab",  "Go to last tab",                      ["ctrl+9"],               () => cmdJumpTab(9)],
  ["workspace:next-tab",       "Go to next tab",                      ["ctrl+tab", "ctrl+pagedown"],       () => cmdCycleTab(1)],
  ["workspace:previous-tab",   "Go to previous tab",                  ["ctrl+shift+tab", "ctrl+pageup"],   () => cmdCycleTab(-1)],
  ...[1, 2, 3, 4, 5, 6, 7, 8].map(n => ["workspace:goto-tab-" + n, "Go to tab #" + n, ["ctrl+" + n], () => cmdJumpTab(n)]),
  ["graph:open",               "Open graph view",                     ["ctrl+g"],               cmdGlobalGraph],
  ["graph:open-local",         "Open local graph",                    ["ctrl+shift+g"],         () => cmdLocalGraph()],
  ["editor:insert-link",       "Insert Markdown link",                ["ctrl+k"],               () => edEdit((v, a, b) => { const s = v.slice(a, b) || "link"; return [v.slice(0, a) + "[" + s + "]()" + v.slice(b), a + s.length + 3, a + s.length + 3]; })],
  ["editor:insert-wikilink",   "Add internal link",                   [],                       () => edEdit((v, a, b) => { const s = v.slice(a, b); return [v.slice(0, a) + "[[" + s + "]]" + v.slice(b), a + 2, a + 2 + s.length]; })],
  ["editor:insert-tag",        "Add tag",                             [],                       () => edEdit((v, a, b) => [v.slice(0, a) + "#" + v.slice(b), a + 1, a + 1])],
  ["app:go-back",              "Navigate back",                       ["ctrl+alt+arrowleft", "alt+arrowleft"],   () => histGo(-1)],
  ["app:go-forward",           "Navigate forward",                    ["ctrl+alt+arrowright", "alt+arrowright"], () => histGo(1)],
  ["workspace:new-tab",        "New tab",                             ["ctrl+t"],               cmdNewTab],
  ["app:open-help",            "Open help",                           ["f1"],                   () => window.open("https://github.com/jpzk/rustidian#readme")],
  ["editor:open-link-in-new-leaf", "Open link under cursor in new tab", ["ctrl+enter"],         async () => { const n = linkAtCaret(); if (!n) return; const g = fg(); await flushSave(g); g.tabs.push(mkTab(n)); g.active = g.tabs.length - 1; await loadActive(g); }],
  ["editor:open-link-in-new-split", "Open link under cursor to the right", ["ctrl+alt+enter"], async () => { const n = linkAtCaret(); if (n) await splitWith(fg(), "row", mkTab(n)); }],
  ["switcher:open",            "Open quick switcher",                 ["ctrl+o"],               () => cmdQuickSwitch()],
  ["workspace:edit-file-title","Rename file",                         ["f2"],                   cmdRename],
  ["editor:save-file",         "Save current file",                   ["ctrl+s"],               cmdSave],
  ["global-search:open",       "Search in all files",                 ["ctrl+shift+f"],         () => { if (!sideOpen) cmdToggleSide(); setPane("search"); }],
  ["editor:toggle-bold",       "Toggle bold",                         ["ctrl+b"],               () => edWrap("**")],
  ["editor:toggle-checklist-status", "Toggle checkbox status",        ["ctrl+l"],               () => edTask()],
  ["editor:toggle-comments",   "Toggle comment",                      ["ctrl+/"],               () => edWrap("%%", "comment")],
  ["editor:toggle-italics",    "Toggle italic",                       ["ctrl+i"],               () => edWrap("*")],
  ["markdown:toggle-preview",  "Toggle reading view",                 ["ctrl+e"],               () => cmdToggleMode()],
  ["editor:toggle-source",     "Toggle Live Preview/Source mode",     [],                       () => cmdToggleSource()],
  ["workspace:undo-close-pane","Undo close tab",                      ["ctrl+shift+t"],         async () => { const n = closedTabs.pop(); if (n) await openInTab(n); }],
  ["workspace:split-vertical", "Split right",                         [],                       () => splitGroup(fg(), "row", fg().active)],
  ["workspace:split-horizontal","Split down",                         [],                       () => splitGroup(fg(), "col", fg().active)],
  ["app:toggle-left-sidebar",  "Toggle left sidebar",                 [],                       cmdToggleSide],
  ["app:toggle-right-sidebar", "Toggle right sidebar",                [],                       () => cmdToggleRight()],
  ["app:switch-vault",         "Switch vault",                        [],                       showPicker],
  // R36: stock lists exactly these three, with NO hotkey text beside them
  // (recon-zoom shot 01) — built-in bindings it does not surface as rebindable
  // rows. Ours ARE in the one registry, so the palette, the keymap and
  // Settings ▸ Hotkeys all read the same line. The chord is the PLAIN '=' key:
  // Ctrl+plus, Ctrl+Shift+= and Ctrl+KP_Add were each measured as no-ops on
  // stock (notes/recon-zoom.txt), and binding "plus" is negative control N2.
  ["window:zoom-in",           "Zoom in",                             ["ctrl+="],               () => cmdZoom("in")],
  ["window:zoom-out",          "Zoom out",                            ["ctrl+-"],               () => cmdZoom("out")],
  ["window:reset-zoom",        "Reset zoom",                          ["ctrl+0"],               () => cmdZoom("reset")],
].map(([id, name, def, run]) => ({ id, name, def, run }))
 .sort((a, b) => a.name.localeCompare(b.name));
let hkUser = {};                            // id -> [chords] overrides ([] = removed default)
let keymap = {};                            // chord -> cmd (rebuilt from CMDS + hkUser)
const hkChords = c => hkUser[c.id] || c.def;
function hkConflicts() {                    // chord -> [cmd ids] with 2+ owners
  const by = {};
  for (const c of CMDS) for (const ch of hkChords(c)) (by[ch] = by[ch] || []).push(c.id);
  return Object.fromEntries(Object.entries(by).filter(([, v]) => v.length > 1));
}
function hkRebuild() {                      // last-defined wins on conflicts (stock behaviour)
  keymap = {};
  for (const c of CMDS) for (const ch of hkChords(c)) keymap[ch] = c;
}
hkRebuild();
// stock shape <-> chord strings. e.key names round-trip through KEYNAMES.
const KEYNAMES = { arrowleft: "ArrowLeft", arrowright: "ArrowRight", arrowup: "ArrowUp", arrowdown: "ArrowDown",
  pageup: "PageUp", pagedown: "PageDown", tab: "Tab", enter: "Enter", escape: "Escape", backspace: "Backspace",
  delete: "Delete", home: "Home", end: "End", insert: "Insert", " ": "Space" };
function chordToStock(ch) {
  const p = ch.split("+"); const k = p.pop();
  const mods = p.map(m => ({ ctrl: "Mod", alt: "Alt", shift: "Shift" }[m])).filter(Boolean);
  return { modifiers: mods, key: k.length === 1 || /^f\d+$/.test(k) ? k.toUpperCase() : KEYNAMES[k] || k };
}
function stockToChord(o) {
  const m = new Set((o.modifiers || []).map(x => x.toLowerCase()));
  const k = (o.key === "Space" ? " " : String(o.key || "")).toLowerCase();
  return ((m.has("mod") || m.has("ctrl")) ? "ctrl+" : "") + (m.has("alt") ? "alt+" : "") + (m.has("shift") ? "shift+" : "") + k;
}
async function hkLoad() {
  const m = await inv("get_hotkeys").catch(() => ({}));
  hkUser = {};
  const known = new Set(CMDS.map(c => c.id));
  for (const [id, arr] of Object.entries(m || {})) if (known.has(id)) hkUser[id] = arr.map(stockToChord);
  hkRebuild();
}
function hkSave() {
  const map = Object.fromEntries(Object.entries(hkUser).map(([id, chs]) => [id, chs.map(chordToStock)]));
  inv("set_hotkeys", { map }).catch(() => {});
  hkRebuild();
}
function hkSet(c, chords) {                 // set + collapse to "default" when equal
  const same = chords.length === c.def.length && chords.every((x, i) => x === c.def[i]);
  if (same) delete hkUser[c.id]; else hkUser[c.id] = chords;
  hkSave();
}
const chordLabel = ch => ch.split("+").map(k => k === "ctrl" ? "Ctrl" : k === "alt" ? "Alt" : k === "shift" ? "Shift"
  : k.length === 1 ? k.toUpperCase() : (KEYNAMES[k] || k).replace(/^Arrow/, "").replace(/^./, x => x.toUpperCase())).join(" + ");
function chordOf(e) {                       // keydown -> normalised chord (null for bare modifiers)
  if (["Control", "Alt", "Shift", "Meta"].includes(e.key)) return null;
  let k = e.key.toLowerCase();
  if (e.code === "Tab") k = "tab";   // X11 shift+tab arrives as ISO_Left_Tab
  // Xvfb/xdotool synthesize every F-key with a spurious Mod1 latch (alt:true
  // on F1..F12, clean on letters) — drop alt for function keys so F2 binds.
  const alt = e.altKey && !/^F\d+$/.test(e.code);
  return (e.ctrlKey ? "ctrl+" : "") + (alt ? "alt+" : "") + (e.shiftKey ? "shift+" : "") + k;
}
function cpItems() {                 // palette source: the registry w/ current hotkey hints
  return CMDS.map(c => ({ label: c.name, hint: hkChords(c).map(chordLabel).join(", "), run: c.run }));
}
function cmdPalette() {
  modalKind === "cp" ? closeModal() : openModal("cp", cpItems);
}

/* m5 F2 rename: inline prompt over the focused note tab; disk rename via
   rename_note (the rust side rewrites inbound wikilinks vault-wide), then
   tabs/hist/mru follow the name */
async function applyRename(old, nn) {   // post-rename bookkeeping (F2 / cmdRename is the only caller)
  for (const h of groups()) for (const tb of h.tabs) {
    if (tb.kind) continue;
    if (tb.name === old) { tb.name = nn; if (tb.view) setInlineTitle(tb.view, nn); }   // #20: the rendered title follows the FILE, in every retained view
    if (tb.hist) for (const e of tb.hist) if (e.n === old) e.n = nn;
  }
  const mi = mruList.indexOf(old);
  if (mi >= 0) mruList[mi] = nn;
  for (const h of groups()) renderTabs(h);
  await refreshTree();
  /* R9.8: the note's name changed, so the rust side rewrote its entry in
     .rustidian-bookmarks (move_note_in). bmCache is a COPY of that file taken
     at the last refresh, and the bookmarks pane paints from the cache — so
     without this line the file is right on disk and the pane still shows the
     old name until the user switches panes (the pane's own `if (p === "bm")
     refreshBm()`). Here, not in the two callers: applyRename is the UI's
     single post-rename choke point, the mirror of move_note_in on the rust
     side, and both the F2 rename and the R34 title rename (a MOVE) reach it.
     Re-read rather than patch the cache locally: disk is the truth, and the
     rewrite rules (index kept, duplicate collapsed) live in one place. */
  await refreshBm();
  updateTitle();
}

/* ux-3 is GONE (#20 / R32.5). It used to rename the note — and rewrite every
   inbound [[link]] vault-wide — whenever a committed edit changed the note's
   first-line body H1. Recon on stock 1.13.7 (inline-title/09, progress.md Q4)
   measured the opposite: typing into a note's first-line `# Rename Me` grew
   the file 40 -> 49 bytes and the FILENAME NEVER MOVED. Stock renames from the
   inline TITLE, and then raises an "Update links" modal (Always update / Just
   once / Do not update) — it never silently rewrites a vault. So a body-H1
   edit that mutates the user's filename and every note that links to it was
   rustidian's own invention, not parity, and it is the riskiest write in the
   app. It is deleted: editing a body H1 now does what stock does — edits text.
   The rename path is F2 / "Rename file" -> cmdRename below (phase `m5` asserts
   it lands on disk); phase `ux` step 2 asserts the negative half (a body-H1
   edit leaves the filename and every inbound link alone, and the bytes land). */
function cmdRename() {
  const g = fg();
  const t = g && g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind) return;                 // notes only, no graph tabs
  const inp = $("rninput");
  $("rnbox").hidden = false;
  inp.value = t.name;
  inp.focus();
  inp.select();
  updateTitle();
}
$("rninput").onkeydown = async e => {
  if (e.key === "Escape") { $("rnbox").hidden = true; updateTitle(); return; }
  if (e.key !== "Enter") return;
  const g = fg();
  const t = g && g.active >= 0 ? g.tabs[g.active] : null;
  const nn = $("rninput").value.trim();
  $("rnbox").hidden = true;
  if (!t || t.kind || !nn || nn === t.name) { updateTitle(); return; }
  const old = t.name;
  await flushSave(g);                       // old content lands before the move
  try { await inv("rename_note", { old, new: nn }); }
  catch (err) { updateTitle(); return; }    // exists/invalid -> keep old name
  await applyRename(old, nn);
};

if (document.fonts) document.fonts.addEventListener("loadingdone", () => updateTitle());   // R15.2: republish [fonts:] once a lazy @font-face lands

/* perf (INTEGRATE): compositor FLOOR probe — N no-op 1px repaints driven through
   the SAME act() -> otel.paint() path every interaction uses, so each op can be
   reported as a multiple of what one frame costs on this host (Xvfb/llvmpipe is
   ~an order slower than a GPU). A 1.2x-floor op is the framebuffer; a 3x-floor
   op is our code. Bench-only: Ctrl+Alt+Shift+F, no menu/palette entry. */
async function floorProbe(n = 30) {
  let px = $("floorpx");
  if (!px) { px = document.createElement("div"); px.id = "floorpx"; document.body.appendChild(px); }
  // INTEGRATE: three floors, cheapest damage first. compositor_floor = 1px;
  // compositor_floor_full = every painted pixel of the content tree re-rastered;
  // compositor_floor_relayout = both note columns reflow + that raster, i.e. what a
  // pane split costs the compositor before any of our code is blamed for it. `rows`
  // records how much live-preview DOM was on screen while the floor was measured.
  for (const [name, cls, k] of [["compositor_floor", "floorprobe", n],
                                ["compositor_floor_full", "floorfull", 20],
                                ["compositor_floor_relayout", "floorrelayout", 20]]) {
    for (let i = 0; i < k; i++) {
      const rows = document.querySelectorAll(".lprow").length;
      await act(name, { i, rows }, () => { document.body.classList.toggle(cls); });
      await new Promise(r => setTimeout(r, 30));   // act() resolves before paint lands; let the span close
    }
    document.body.classList.remove(cls);
  }
  // INTEGRATE floor #4: compositor_floor_build. The three probes above all re-damage
  // DOM that is ALREADY laid out, so they price a re-paint, not a first paint — and the
  // bench says every op over the ceiling (pane_split, tab_drop, note_open) is one that
  // BUILDS a note column from nothing. This probe prices exactly that and nothing else:
  // clone the live note's rows (cloneNode = no markdown parse, no invoke, no our-code),
  // append them as a second .lp column inside #main, and measure to paint. What is left
  // is the engine's style+layout+raster of N fresh subtrees, i.e. the real floor a pane
  // split cannot go below while it shows the same note twice.
  const srcLp = document.querySelector(".lp");
  if (srcLp) {
    for (let i = 0; i < 20; i++) {
      const rows = srcLp.querySelectorAll(".lprow").length;
      const col = document.createElement("div");
      col.className = srcLp.className;
      col.style.cssText = "flex:1 1 0; min-width:0";
      for (const r of srcLp.children) col.appendChild(r.cloneNode(true));
      await act("compositor_floor_build", { i, rows }, () => { $("main").appendChild(col); });
      await new Promise(r => setTimeout(r, 30));
      col.remove();
      await new Promise(r => setTimeout(r, 30));   // let the removal paint before the next sample
    }
  }
}
document.addEventListener("keydown", e => {
  if (e.ctrlKey && e.altKey && e.shiftKey && (e.key === "F" || e.key === "f")) { e.preventDefault(); floorProbe(); return; }
  if (settingsOpen) return hkKey(e);       // R14: settings modal owns the keyboard (chord capture)
  if (ulPending) return;                   // R34.6: the Update links prompt owns the keyboard — its own handler answers it
  if (titleEditing()) return;              // R34.1: so does the title box (a filename contains chords)
  if (e.key === "Escape") {
    if (modalKind) { closeModal(); return; }
    if (!$("rnbox").hidden) { $("rnbox").hidden = true; updateTitle(); return; }
    closeMenu();
    if (vaultPath && !$("picker").hidden) $("picker").hidden = true;
    if (!$("fnew").hidden) $("fnew").hidden = true;
    if (!$("anew").hidden) closeAttach();       // R31.7
    return;
  }
  const combo = chordOf(e);
  const c = combo && keymap[combo];
  if (modalKind && c && c.id !== "switcher:open" && c.id !== "command-palette:open") return;  // modal traps the keymap
  if (!$("rnbox").hidden) return;    // rename prompt traps the keymap too
  if (!$("anew").hidden) return;     // R31.7: so does the attachment prompt — a path contains characters that are chords
  if (c) { e.preventDefault(); c.run(); }
});

$("stab-files").onclick = () => setPane("files");
$("collapsebtn").onclick = cmdToggleSide;
$("stab-search").onclick = () => setPane("search");
$("stab-bm").onclick = () => setPane("bm");
$("sinput").oninput = () => {              // debounce 60ms (was 150: sized for the disk-walking search; the index answers in ~1ms, the DOM for ~400 hits in a few ms)
  if (searchT0 < 0) searchT0 = perf.now();  // perf: first keystroke of this query
  otel.cancel(searchSp);                    // R18: a keystroke before the last one painted supersedes it
  searchSp = otel.begin("search_type", { q_len: $("sinput").value.length, notes: notesCache.length });
  clearTimeout(searchT); searchT = setTimeout(runSearch, 60);
};
$("sclear").onclick = () => {
  $("sinput").value = ""; clearTimeout(searchT); runSearch(); $("sinput").focus();
};
$("newbtn").onclick = cmdNewNote;
$("newfolderbtn").onclick = () => {
  const box = $("fnew");
  box.hidden = !box.hidden;
  if (!box.hidden) { $("fname").value = ""; $("fname").focus(); }
};
$("fname").onkeydown = async e => {
  if (e.key !== "Enter") return;
  const name = $("fname").value.trim();
  if (!name) return;
  try { await inv("create_dir", { name }); }
  catch (err) { return; }
  $("fnew").hidden = true;
  await refreshTree();
};
// R31.7: Enter attaches the typed path through attachDrop (the drop's own entry
// point — no second copy of the rules down here), Escape just closes.
$("apath").onkeydown = async e => {
  if (e.key === "Escape") return closeAttach();
  if (e.key !== "Enter") return;
  const p = $("apath").value.trim();
  closeAttach();
  if (p) await attachDrop([p]);
};

/* ---------- graph (per group: one sim instance per group) ----------
   startGraph(g, cfg) is the shared canvas sim (M4): hover/zoom/pan/unresolved.
   cfg = { fetch: async () -> {nodes, edges},   node/edge supplier
           center: () -> name|null,             drawn larger + accent (M8 localgraph)
           onClick: async name -> void }        navigation target on node click */
let simGen = 0;                    // perf-graph: sim generation counter (see startGraph)
// graph-webgl: draw-path preference. env RUSTIDIAN_GRAPH_RENDERER=gl|2d (backend) beats the
// hidden localStorage setting rustidian.graphRenderer; RUSTIDIAN_GRAPH_LOSE_CTX=1 is the smoke
// hook that loses the GL context once the sim has settled (fallback must keep drawing).
let graphPrefP = null;
const graphRendererPref = () => graphPrefP || (graphPrefP = inv("graph_renderer_pref").catch(() => null).then(p => {
  const r = (p && p.renderer) || localStorage.getItem("rustidian.graphRenderer");
  return { renderer: r === "gl" || r === "2d" ? r : null, loseCtx: !!(p && p.lose_ctx) };
}));
function showEditor(g) {
  g.graphOn = false; g.graphRefresh = null; g.graphRc = null; cancelAnimationFrame(g.sim);
  if (g.ro) { g.ro.disconnect(); g.ro = null; }
  perf.flush();                    // ship buffered graph_frame samples of the closed sim
  g.graph.hidden = true; if (g.glcv) g.glcv.hidden = true;
  g.lggear.hidden = true; g.lgpop.hidden = true;
  applyMode(g);
}
$("graphbtn").onclick = cmdGlobalGraph;
async function cmdGlobalGraph() {  // R9.7: ribbon icon opens GLOBAL graph as a main tab
  if (!state) return;
  const g = fg();
  g.perfT0 = perf.now();           // perf: graph_open = click -> first sim frame
  await flushSave(g);
  const i = g.tabs.findIndex(t => t.kind === "gg");
  if (i >= 0) g.active = i;        // one graph-view tab per group, refocus it
  else {
    g.tabs.push({ kind: "gg", name: "Graph view", mode: "source",
                  hist: [], hpos: -1 });
    g.active = g.tabs.length - 1;
  }
  await loadActive(g);
}
async function startGraph(g, cfg) {
  g.graphOn = true; g.graphSettled = false;
  const openT0 = g.perfT0 || perf.now();   // perf: graph_open_settle = open -> kinetic energy below eps
  hideAc();
  g.status.hidden = true;
  g.editor.style.display = "none"; g.preview.style.display = "none";
  g.lp.style.display = "none";
  const cv = g.graph; cv.hidden = false;
  cv.width = cv.clientWidth; cv.height = cv.clientHeight;
  const fetchP = cfg.fetch(), prefP = graphRendererPref();   // backend works while the renderer comes up
  const ctx = cv.getContext("2d");
  // graph-webgl: DEFAULT draw path is WebGL (ui/graph-gl.js) on a .graphgl canvas BEHIND cv
  // (nodes + edges); cv stays on top for events, labels (Canvas 2D text) and the 2D fallback.
  // Fallback to the full 2D path when: forced (env/setting), no webgl context, or the
  // context is lost mid-session (webglcontextlost -> g.glLost). One renderer per canvas is
  // cached on the group (g.glr); a lost context gets a fresh canvas on the next open.
  // Created BEFORE the fetch resolves so context + shader setup never lands in the first sim frame.
  const pref = await prefP, rT0 = perf.now();
  let glr = null, reason = "default";
  if (pref.renderer === "2d") reason = "forced:2d";
  else if (!window.GraphGL) reason = "no-module";
  else {
    if (!g.glr || g.glr.lost) {
      if (g.glcv) g.glcv.remove();
      g.glcv = document.createElement("canvas"); g.glcv.className = "graphgl";
      cv.parentNode.insertBefore(g.glcv, cv);
      g.glr = GraphGL.create(g.glcv, () => { if (g.glLost) g.glLost(); });
    }
    glr = g.glr;
    if (!glr) reason = "no-webgl";
  }
  const gpu = glr ? { gpu_vendor: glr.info.vendor, gpu_renderer: glr.info.renderer } : { gpu_vendor: "", gpu_renderer: "" };
  cv.classList.toggle("gl-on", !!glr); if (g.glcv) g.glcv.hidden = !glr;
  perf.mark("graph_renderer", rT0, { renderer: glr ? "gl" : "2d", reason, webgl: glr ? glr.info.webgl : 0, ...gpu });
  const gr = await fetchP;
  // R16 GRAPH FIT (stock-faithful, docs/requirements.md R16): the sim runs in
  // UNBOUNDED world coords with the origin at the canvas centre; camera opens
  // at scale 1 (1 world unit = 1 px) centred on the origin — no fit-to-view,
  // no viewport clamp: a big vault overflows the canvas and the user pans/zooms
  // (R16.4). screen = world*scale + t.
  const view = { scale: 1, tx: cv.width / 2, ty: cv.height / 2, notch: 0 };
  // stock force defaults (R16.1). REPEL_K/REPEL_P: per-node many-body strength
  // = repel * REPEL_K * N^REPEL_P, calibrated offline (goal/graphfit/sim) so
  // the 12-node vault settles ~35-45% of the canvas wide and the 500-note star
  // to a ~2300-unit disc with ~80 nodes inside a 1080x764 view (stock: 79).
  const F = { center: 0.52, repel: 10, link: 1, dist: 250 }, REPEL_K = 7.5, REPEL_P = 0.36;
  // d3-force style phyllotaxis seed (deterministic: smoke coords repeat)
  const seed = i => { const r = 10 * Math.sqrt(i + 0.5), t = i * 2.399963; return [r * Math.cos(t), r * Math.sin(t)]; };
  const N = gr.nodes.map((nd, i) => {
    const [x, y] = seed(i);
    return { n: nd.name, resolved: nd.resolved, x, y, vx: 0, vy: 0, deg: 0, r: 6.5 };
  });
  const toWorld = (sx, sy) =>
    [(sx - view.tx) / view.scale, (sy - view.ty) / view.scale];
  // adjacency (hover) + undirected unique link list for the spring force;
  // node radius (R16.2, fit on stock deg-1 7.5px / deg-499 29.5px): 6.5 + sqrt(deg)
  const adj = N.map(() => new Set());
  let links = [];                       // [{a, b, bias, k}] d3 link semantics
  const rebuild = () => {
    adj.length = 0; for (const _ of N) adj.push(new Set());
    for (const [i, j] of gr.edges) { adj[i].add(j); adj[j].add(i); }
    N.forEach((p, i) => { p.deg = adj[i].size; p.r = 6.5 + Math.sqrt(p.deg); });
    links = [];
    adj.forEach((s, i) => { for (const j of s) if (i < j) {
      const di = N[i].deg, dj = N[j].deg;
      // strength = Link force / min(deg) (d3 default: hubs are not yanked by
      // every leaf), bias = share of the correction the far end takes
      links.push({ a: i, b: j, bias: di / (di + dj), k: F.link / Math.min(di, dj) });
    } });
  };
  rebuild();
  let hov = -1;
  const hitTest = (x, y) => N.findIndex(p => (p.x - x) ** 2 + (p.y - y) ** 2 < (p.r + 4) ** 2);
  // C5 (B26-B29, R19.2/R19.3): the re-centre CONTINUITY record. graphRefresh snapshots the
  // real pre-refresh node objects; the first frame after the swap (rcPre) and its first PAINT
  // (rcRecord) measure the LIVE N[] against that snapshot and publish it once as [lgrc:].
  // A shell cannot sample a frame, so the frame reports itself. It is a probe, not a metric:
  // nothing is recomputed on later frames, so a settled graph pays two null tests per frame.
  let rcSnap = null, rcTok = "", rcSeq = 0;
  g.graphRc = () => rcTok;
  // live refresh (R4.3): re-fetch on save, keep surviving positions,
  // seed new nodes near their first neighbor
  g.graphRefresh = async () => {
    // R19: a node click prefetches the next neighbourhood in parallel with the
    // note open (g.prefetch = {n, p}); use it when it is for the current centre
    const pf = g.prefetch; g.prefetch = null;
    // C5: the branch B26 guards. pfHit is the condition the code ALREADY took; naming it
    // changes nothing but lets the first paint report which side it ran, and `waited` is the
    // wall time this refresh spent blocked on the fetch — ~0 when the request really did run
    // in parallel with the note open, a full round trip when it did not.
    const pfHit = !!(pf && pf.n === cfg.center()), wT0 = perf.now();
    const g2 = await (pfHit ? pf.p : cfg.fetch());
    const waited = perf.now() - wT0;
    // graph-webgl: a refresh that changes nothing (the race-closing refetch below, a save that
    // touched no link) must not reheat — the layout stays a pure function of the vault, so two
    // opens land on identical positions (smoke graphgl compares gl vs 2d frames pixel-wise)
    const same = g2.nodes.length === N.length && g2.edges.length === gr.edges.length &&
      g2.nodes.every((nd, i) => nd.name === N[i].n && nd.resolved === N[i].resolved) &&
      g2.edges.every((e, i) => e[0] === gr.edges[i][0] && e[1] === gr.edges[i][1]);
    if (same) return;
    const old = new Map(N.map(p => [p.n, p]));
    // C5: plain-value copy of the REAL pre-refresh nodes, taken before the swap. These
    // objects leave N below and are never stepped again, but copy anyway so the frames that
    // read the record compare against numbers that cannot have moved under them.
    const prev = new Map();
    for (const [nm, o] of old) prev.set(nm, { x: o.x, y: o.y, vx: o.vx, vy: o.vy, pin: o.fx != null });
    const N2 = g2.nodes.map(nd => {
      const o = old.get(nd.name);
      // C3: a survivor keeps its PIN too (fx/fy) — a save must not unstick a node
      // the user dropped somewhere on purpose.
      return o ? { n: nd.name, resolved: nd.resolved, x: o.x, y: o.y, vx: o.vx, vy: o.vy, fx: o.fx, fy: o.fy, deg: 0, r: 6.5 }
               : { n: nd.name, resolved: nd.resolved, x: null, y: null, vx: 0, vy: 0, deg: 0, r: 6.5 };
    });
    // R19 warm start (feedback #5): survivors keep position + velocity; a NEW
    // node is seeded one link length (F.dist) from its first surviving
    // neighbour, on the ray from the old centroid through that neighbour
    // (outward, where the spring wants it), fanned by index so siblings do
    // not stack; a node with no placed neighbour takes the phyllotaxis slot.
    let cx = 0, cy = 0, nOld = 0;
    for (const p of N2) if (p.x !== null) { cx += p.x; cy += p.y; nOld++; }
    if (nOld) { cx /= nOld; cy /= nOld; }
    N2.forEach((p, i) => {
      if (p.x !== null) return;
      const e = g2.edges.find(([a, b]) => (a === i && N2[b].x !== null) || (b === i && N2[a].x !== null));
      if (!e) { [p.x, p.y] = seed(i); return; }
      const nb = N2[e[0] === i ? e[1] : e[0]];
      let ang = Math.atan2(nb.y - cy, nb.x - cx);
      if (!Number.isFinite(ang) || (nb.x === cx && nb.y === cy)) ang = i * 2.399963;
      ang += (i % 2 ? 1 : -1) * 0.35 * ((i >> 1) % 3);
      p.x = nb.x + F.dist * Math.cos(ang); p.y = nb.y + F.dist * Math.sin(ang);
    });
    N.length = 0; N.push(...N2);
    gr.edges = g2.edges;
    rebuild();
    hov = -1;
    // C5: a0 is the sim's REAL alpha before the restart — a graph can go quiet with alpha
    // still around 0.5 (kinetic calm wins the race against the alpha floor), and reheat only
    // ever RAISES alpha, so "the restart was warm" is `alpha at the first paint <= max(a0,
    // 0.3)`, never `<= 0.3`.
    rcSnap = { prev, pf: pfHit, w: waited, a0: alpha, c: cfg.center() };
    g.reheat(0.3);   // R19: d3 restart semantics — alpha 0.3, not 1: settle the new nodes without scattering the old ones
  };
  // sim heat (d3-force shaped): forces scale by alpha, which decays per PHYSICS
  // STEP toward 0 (alpha += -alpha*ALPHA_DECAY; 0.001 after 300 steps) and
  // physics freezes below 0.001. Physics steps are wall-clock-locked at PH_HZ/s
  // (substepped inside rAF): a throttled/headless rAF must not stretch settle.
  // Catch-up is bounded TWICE, and the bound is the frame-time budget: at most
  // PH_STEP_CAP steps of sim time may be owed at the top of a frame
  // (PH_STEP_CAP/PH_HZ s), and the substep loop also stops once it has spent
  // PH_BUDGET_MS of WALL time inside one frame, whichever binds first. The old
  // bound was sim time only (PH_CAP = 1 s = 120 substeps/frame) and was argued
  // for, never measured: "every gap beyond the cap is sim time LOST, which
  // stretches settle". Measured at N=3000 (docs/graph-perf, item 8) that trade
  // is real but one-sided — a step costs 6.13 ms here, so an unbounded catch-up
  // converts every scheduler gap the host hands rAF into MORE steps in the next
  // frame: hot_frames x steps/frame = 300.0 in every run at every load (the
  // alpha floor fixes the TOTAL work), and load only repackages that constant
  // into fewer, fatter frames — 11.54 steps = 78 ms, 18.75 steps = 129 ms.
  // The frame is the thing the user feels; the settle is the thing the cap
  // costs. Prediction and arithmetic committed before this change (item 8):
  // S=7 => frame 50.2 ms (-35.7%), settle ~3.29 s (+15%), steps/frame pinned at
  // the cap, per-step physics unchanged. Sim time beyond the cap is still lost;
  // that is now a measured 15% on settle, not an unmeasured 1 s of debt.
  // A hidden tab returning after minutes bursts at most 300 steps (alpha floor)
  // anyway, and now drains them at PH_STEP_CAP per frame instead of 120.
  // perf-graph: the rAF loop is NOT unconditional — it runs while physics is
  // hot (alpha > ALPHA_MIN and kinetic energy above eps) and stops otherwise
  // (CPU 0); wake() restarts it on refresh (reheat), pan, zoom, hover, resize, close.
  const PH_HZ = 120, PH_STEP_CAP = 7, PH_BUDGET_MS = 40, ALPHA_MIN = 0.001, ALPHA_DECAY = 1 - Math.pow(0.001, 1 / 300);
  let alpha = 1, phAcc = 0, phLast = performance.now();
  // settled = total kinetic energy (sum v^2) under 0.0025 px^2/step per node
  // (mean speed < 0.05 px/step, invisible) for 10 consecutive steps, or physics frozen
  let calm = 0, quiet = false;          // quiet: physics halted until the next reheat
  let settledMark = false;              // graph_open_settle mark fires once per open
  const kinetic = () => { let k = 0; for (const p of N) k += p.vx * p.vx + p.vy * p.vy; return k; };
  // Barnes-Hut quadtree (theta 0.8) for the many-body repulsion (d3 shape:
  // dv = REPEL*alpha*dx/d^2, i.e. |dv| ~ 1/d). The force is long-range, so a
  // cutoff grid would change the layout; instead every far cell
  // (side/dist < theta) acts as one body of its mass at its centroid.
  // O(N log N) per step instead of the all-pairs O(N^2). Cells come from a
  // pool reused across steps (no per-step allocation churn). The root square
  // is sized from the CURRENT node bbox each step (world is unbounded); depth
  // is capped at MAXD (leaf >= side/1024) so a coincident pair cannot split
  // 24 levels. Points sharing a leaf do not repel each other.
  const THETA2 = 0.8 * 0.8, MAXD = 10;
  const pool = []; let pn = 0;
  const cell = (x0, y0, s) => {
    let c = pool[pn]; if (!c) c = pool[pn] = {};
    pn++; c.x0 = x0; c.y0 = y0; c.s = s; c.n = 0; c.sx = 0; c.sy = 0; c.p = null; c.k = null;
    return c;
  };
  const qi = (c, p) => (p.x >= c.x0 + c.s / 2 ? 1 : 0) + (p.y >= c.y0 + c.s / 2 ? 2 : 0);
  function bhBuild() {
    pn = 0;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const p of N) { if (p.x < x0) x0 = p.x; if (p.x > x1) x1 = p.x; if (p.y < y0) y0 = p.y; if (p.y > y1) y1 = p.y; }
    const root = cell(x0, y0, Math.max(x1 - x0, y1 - y0) + 1);
    for (const p of N) {
      let c = root, depth = 0;
      for (;;) {
        c.n++; c.sx += p.x; c.sy += p.y;
        if (c.k) { c = c.k[qi(c, p)]; depth++; continue; }
        if (c.n === 1) { c.p = p; break; }         // empty leaf takes p
        if (depth >= MAXD) break;                    // shared leaf: mass only
        const h = c.s / 2, q = c.p; c.p = null;      // occupied leaf: split, push old point down
        c.k = [cell(c.x0, c.y0, h), cell(c.x0 + h, c.y0, h), cell(c.x0, c.y0 + h, h), cell(c.x0 + h, c.y0 + h, h)];
        const cq = c.k[qi(c, q)]; cq.n = 1; cq.sx = q.x; cq.sy = q.y; cq.p = q;
        c = c.k[qi(c, p)]; depth++;
      }
    }
    return root;
  }
  function bhApply(a, c, k) {
    if (c.n === 0) return;
    const dx = a.x - c.sx / c.n, dy = a.y - c.sy / c.n, d2 = dx * dx + dy * dy + 1;
    if (c.k) {
      if (c.s * c.s > THETA2 * d2) { const K = c.k; bhApply(a, K[0], k); bhApply(a, K[1], k); bhApply(a, K[2], k); bhApply(a, K[3], k); return; }
    } else if (c.p === a || (a.x >= c.x0 && a.x < c.x0 + c.s && a.y >= c.y0 && a.y < c.y0 + c.s)) return;  // own leaf
    a.vx += k * c.n * dx / d2; a.vy += k * c.n * dy / d2;
  }
  function physStep() {
    if (!N.length) return;
    // link: spring to F.dist (R16.1: 250), d3 semantics (strength/bias per link)
    for (const l of links) {
      const a = N[l.a], b = N[l.b];
      let dx = b.x + b.vx - a.x - a.vx, dy = b.y + b.vy - a.y - a.vy;
      const d = Math.sqrt(dx * dx + dy * dy) || 1e-6, k = (d - F.dist) / d * alpha * l.k;
      dx *= k; dy *= k;
      b.vx -= dx * l.bias; b.vy -= dy * l.bias; a.vx += dx * (1 - l.bias); a.vy += dy * (1 - l.bias);
    }
    // many-body repulsion. Per-node strength = REPEL_K * Repel force * N^0.25:
    // calibrated on two stock layouts (docs/requirements.md R16 — N=12 cloud
    // ~550px wide, N=500 uniform disc ~2300 world units across, 79 visible)
    const root = bhBuild(), k = alpha * F.repel * REPEL_K * Math.pow(N.length, REPEL_P);
    for (const a of N) bhApply(a, root, k);
    // center force (R16.1: 0.52): centroid pulled toward the origin (d3 forceCenter shape)
    let sx = 0, sy = 0;
    for (const p of N) { sx += p.x; sy += p.y; }
    sx = sx / N.length * F.center; sy = sy / N.length * F.center;
    for (const p of N) {
      p.x -= sx; p.y -= sy;
      p.vx *= 0.6; p.vy *= 0.6; p.x += p.vx; p.y += p.vy;   // velocity decay 0.4 (d3 default)
      // C3 PIN (d3 forceSimulation semantics): a node the user dropped carries
      // fx/fy and is CLAMPED back onto it after every force — including the
      // centre force's whole-cloud translation above, which is what would
      // otherwise drift it away from the drop point. The layout reflows
      // AROUND it; it does not move again until it is dragged again.
      if (p.fx != null) { p.x = p.fx; p.y = p.fy; p.vx = 0; p.vy = 0; }
    }
    alpha += -alpha * ALPHA_DECAY;
  }
  // C5 (B26-B29, R19.2/R19.3): the two halves of the re-centre continuity record. Every field
  // is a measurement of the node objects the renderer is about to draw — never a variable the
  // refresh set to describe itself.
  //
  // rcPre runs at the top of the FIRST frame after the swap, BEFORE that frame's physics: the
  // only instant at which a carried velocity is still literally the velocity the node had.
  //   vraw = % of unpinned survivors still holding their own (vx,vy). 100 = every one of them
  //          kept its motion, 0 = they were all restarted from rest. Vacuous when the graph
  //          was already still, which is why vold is published next to it.
  function rcPre() {
    const s = rcSnap; let n = 0, k = 0;
    for (const p of N) {
      const o = s.prev.get(p.n);
      if (!o || o.pin || p.fx != null) continue;
      n++; if (p.vx === o.vx && p.vy === o.vy) k++;
    }
    s.vraw = n ? Math.round(100 * k / n) : -1;
  }
  // rcRecord runs at that frame's PAINT — the frame the user actually sees.
  //   a/a0  = the sim's real alpha now, and what it was before the restart (see above).
  //   d     = the survivor displacement the EYE reads. The centre force translates the whole
  //           cloud whenever the node set changes (a node 250 away moves the centroid, and
  //           every node with it), so the common translation is subtracted first: d is the
  //           RELATIVE motion, i.e. "did the layout come apart".
  //   vold  = rms speed the survivors had before the re-centre; dv = d / vold, how far they
  //           coasted in this frame per unit of the motion they had (a node that kept its
  //           velocity glides on, one restarted from rest barely moves).
  //   vkeep = sum(v_now . v_before) / sum(|v_before|^2) in %.
  //   nd/no = a NEW node's distance to the nearest node that already had a place, and to the
  //           layout origin (the phyllotaxis fallback sits within ~30px of it).
  // C3: pinned nodes are excluded throughout — fx/fy clamps them, so they are immobile by
  // construction and would make any displacement assertion vacuously true.
  function rcRecord() {
    const s = rcSnap; rcSnap = null;
    const sur = [];
    for (const p of N) { const o = s.prev.get(p.n); if (o && !o.pin && p.fx == null) sur.push([p, o]); }
    let mx = 0, my = 0;
    for (const [p, o] of sur) { mx += p.x - o.x; my += p.y - o.y; }
    if (sur.length) { mx /= sur.length; my /= sur.length; }
    let d = 0, dot = 0, v2 = 0;
    for (const [p, o] of sur) {
      d = Math.max(d, Math.hypot(p.x - o.x - mx, p.y - o.y - my));
      dot += p.vx * o.vx + p.vy * o.vy; v2 += o.vx * o.vx + o.vy * o.vy;
    }
    let nn = 0, nd = -1, no = -1;
    for (const p of N) {
      if (s.prev.has(p.n)) continue;
      nn++;
      if (nn > 64) continue;           // O(new x survivors) bound: this is a probe, not a metric
      let best = Infinity;
      for (const [q] of sur) { const b = Math.hypot(p.x - q.x, p.y - q.y); if (b < best) best = b; }
      if (best < Infinity && (nd < 0 || best < nd)) { nd = Math.round(best); no = Math.round(Math.hypot(p.x, p.y)); }
    }
    const r2 = v => Math.round(v * 100) / 100, vold = Math.sqrt(v2 / (sur.length || 1));
    rcTok = "s=" + (++rcSeq) + ",a=" + r2(alpha) + ",a0=" + r2(s.a0) + ",pf=" + (s.pf ? 1 : 0) +
            ",w=" + r2(s.w) + ",sur=" + sur.length + ",d=" + r2(d) + ",vold=" + r2(vold) +
            ",dv=" + (vold > 1e-6 ? r2(d / vold) : -1) + ",vraw=" + (s.vraw === undefined ? -1 : s.vraw) +
            ",vkeep=" + (v2 > 1e-9 ? Math.round(100 * dot / v2) : -1) +
            ",new=" + nn + ",nd=" + nd + ",no=" + no + ",c=" + s.c;
  }
  // draw: batched paths — edges in 2 strokes (lit / dim), nodes grouped by
  // (color, alpha, resolved) into one fill/stroke each, labels per group
  // graph-webgl: with glr the SAME per-node style decisions feed instance arrays
  // (x y r ring rgba) and an edge instance array (x0 y0 x1 y1 rgba) for graph-gl.js; cv then
  // carries only the labels. Arrays grow on demand and are reused across frames.
  // #rrggbb / #rgb -> [r,g,b] in 0..1 for the GL instance arrays. The 2D path wants
  // the string itself, so both come from the ONE value read out of the token block.
  const hex = h => {
    const s = h.trim(), x = s.length < 7 ? "#" + s[1] + s[1] + s[2] + s[2] + s[3] + s[3] : s;
    return [parseInt(x.slice(1, 3), 16) / 255, parseInt(x.slice(3, 5), 16) / 255, parseInt(x.slice(5, 7), 16) / 255];
  };
  // GRAPH PALETTE. The graph is a <canvas>: it cannot inherit a colour the way every
  // other surface does, it has to ASK for one. It used to hold four hex literals — a
  // second palette that no theme could reach, so a light theme would have left the
  // graph painting dark-theme blue on white. These read the SAME tokens the stylesheet
  // defines, off documentElement, so there is exactly one definition of each colour.
  //
  // CACHED PER THEME, not per node: getComputedStyle forces a style resolution and
  // draw() runs at up to 60 Hz over every node, so the lookup happens once per draw()
  // and only re-reads when the theme attribute actually changes (R13 graph_draw budget).
  // RGB is keyed by the colour STRING, so it is rebuilt with the palette — a stale key
  // would hand the GL path `undefined` and paint nothing.
  const PAL_VAR = { hi: "--accent-yellow", ctr: "--accent-green", node: "--accent-blue", edge: "--border" };
  let pal = null, palKey = null, RGB = {};
  const palette = () => {
    const key = document.documentElement.dataset.theme || "";
    if (pal && palKey === key) return pal;
    const cs = getComputedStyle(document.documentElement), p = {};
    RGB = {};
    for (const k in PAL_VAR) { const v = cs.getPropertyValue(PAL_VAR[k]).trim(); p[k] = v; RGB[v] = hex(v); }
    palKey = key; pal = p;
    return p;
  };
  let nArr = new Float32Array(0), eArr = new Float32Array(0);
  function draw() {
    const dT0 = perf.now();
    const P = palette();               // one token read per frame, none per node
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, cv.width, cv.height);
    ctx.setTransform(view.scale, 0, 0, view.scale, view.tx, view.ty);
    // hover: hovered node + its edges/neighbors lit accent, rest faded
    const litE = ([i, j]) => hov < 0 || i === hov || j === hov;
    const litN = i => hov < 0 || i === hov || adj[hov].has(i);
    ctx.lineWidth = 1;
    let ec = 0;
    const edgePass = (lit, col, a) => {
      if (glr) {                       // gl: dim pass first, lit pass on top (same order as the 2D strokes)
        const c = RGB[col];
        for (const ed of gr.edges) {
          if (litE(ed) !== lit) continue;
          const A = N[ed[0]], B = N[ed[1]], o = ec * 8;
          eArr[o] = A.x; eArr[o + 1] = A.y; eArr[o + 2] = B.x; eArr[o + 3] = B.y;
          eArr[o + 4] = c[0]; eArr[o + 5] = c[1]; eArr[o + 6] = c[2]; eArr[o + 7] = a;
          ec++;
        }
        return;
      }
      ctx.globalAlpha = a; ctx.strokeStyle = col; ctx.beginPath();
      let any = false;
      for (const ed of gr.edges) {
        if (litE(ed) !== lit) continue;
        ctx.moveTo(N[ed[0]].x, N[ed[0]].y); ctx.lineTo(N[ed[1]].x, N[ed[1]].y); any = true;
      }
      if (any) ctx.stroke();
    };
    if (glr && eArr.length < gr.edges.length * 8) eArr = new Float32Array(gr.edges.length * 8 + 800);
    if (hov >= 0) { edgePass(false, P.edge, 0.12); edgePass(true, P.hi, 1); }
    else edgePass(true, P.edge, 1);
    ctx.textAlign = "center"; ctx.font = "12px sans-serif";
    const cn = cfg.center();          // M8: center node larger + accent (R7.1)
    const groups = new Map();         // key -> { col, a, res, dr, idx: [] }
    // world-space cull: nodes outside the viewport are not drawn (R16.4 overflow)
    const [wx0, wy0] = toWorld(0, 0), [wx1, wy1] = toWorld(cv.width, cv.height), pad = 40 / view.scale;
    for (let i = 0; i < N.length; i++) {
      const p = N[i], isC = cn !== null && p.n === cn;
      if (p.x < wx0 - pad || p.x > wx1 + pad || p.y < wy0 - pad || p.y > wy1 + pad) continue;
      const a = litN(i) ? (p.resolved ? 1 : 0.55) : 0.12;
      const col = i === hov ? P.hi : isC ? P.ctr : P.node;
      const key = col + a + (p.resolved ? "r" : "u") + (isC ? "c" : "");
      let gp = groups.get(key);
      if (!gp) groups.set(key, gp = { col, a, res: p.resolved, dr: isC ? 4 : 0, idx: [] });
      gp.idx.push(i);
    }
    const labels = view.scale > 0.73;   // R16.5: labels hidden at scale <= 0.73 (Text fade 0)
    if (glr) {
      if (nArr.length < N.length * 8) nArr = new Float32Array(N.length * 8 + 800);
      let nc = 0;
      for (const gp of groups.values()) {
        const c = RGB[gp.col], ring = gp.res ? 0 : 1.5;
        for (const i of gp.idx) {
          const p = N[i], o = nc * 8;
          nArr[o] = p.x; nArr[o + 1] = p.y; nArr[o + 2] = p.r + gp.dr + ring / 2; nArr[o + 3] = ring;   // 2D strokes straddle the radius
          nArr[o + 4] = c[0]; nArr[o + 5] = c[1]; nArr[o + 6] = c[2]; nArr[o + 7] = gp.a; nc++;
        }
        if (labels) { ctx.globalAlpha = gp.a; ctx.fillStyle = gp.col; for (const i of gp.idx) { const p = N[i]; ctx.fillText(p.n, p.x, p.y - p.r - gp.dr - 4); } }
      }
      glr.draw(cv.width, cv.height, view, nArr, nc, eArr, ec);
    } else for (const gp of groups.values()) {
      ctx.globalAlpha = gp.a; ctx.beginPath();
      for (const i of gp.idx) { const p = N[i], r = p.r + gp.dr; ctx.moveTo(p.x + r, p.y); ctx.arc(p.x, p.y, r, 0, 7); }
      if (gp.res) { ctx.fillStyle = gp.col; ctx.fill(); }
      else { ctx.lineWidth = 1.5; ctx.strokeStyle = gp.col; ctx.stroke(); ctx.lineWidth = 1; } // hollow = unresolved
      if (labels) { ctx.fillStyle = gp.col; for (const i of gp.idx) { const p = N[i]; ctx.fillText(p.n, p.x, p.y - p.r - gp.dr - 4); } }
    }
    ctx.globalAlpha = 1;
    if (rcSnap) { rcRecord(); updateTitle(); }   // C5: once, on the first paint after a re-centre
    perf.push("graph_draw", perf.now() - dT0, { renderer: glr ? "gl" : "2d", nodes: N.length, edges: gr.edges.length, ...gpu });
  }
  let firstFrame = true, running = false, dirty = true;
  const gen = ++simGen; g.simGen = gen;   // a newer startGraph on this canvas retires this sim
  function step() {
    if (g.simGen !== gen) return;
    const now = performance.now();
    if (firstFrame) {
      firstFrame = false;
      if (g.perfT0) { perf.mark("graph_open", g.perfT0, { nodes: N.length, edges: gr.edges.length }); g.perfT0 = null; }
    }
    const fT0 = perf.now(); let steps = 0, ke = -1;
    if (rcSnap && rcSnap.vraw === undefined) rcPre();   // C5: before this frame's physics
    phAcc = Math.min(phAcc + (now - phLast) / 1000, PH_STEP_CAP / PH_HZ); phLast = now;
    const phT0 = perf.now();
    while (phAcc >= 1 / PH_HZ) {
      phAcc -= 1 / PH_HZ;
      if (!quiet && alpha > ALPHA_MIN) { physStep(); steps++; }
      if (steps && perf.now() - phT0 >= PH_BUDGET_MS) { phAcc = 0; break; }   // wall-clock backstop: a costlier step (bigger N) must not lengthen the frame
    }
    const fT1 = perf.now();
    if (!quiet && (steps || alpha <= ALPHA_MIN)) {
      ke = kinetic();
      calm = ke < 0.0025 * N.length ? calm + 1 : 0;
      if (calm >= 10 || alpha <= ALPHA_MIN) {
        quiet = true;
        // R19 (HARD RULE 100ms): graph_recenter ends at the first PAINT of the new centre
        // (see cv.onclick); what happens after that is ANIMATION and is measured here as its
        // own span graph_settle = click -> kinetic energy below eps.
        if (g.settleSp) { otel.end(g.settleSp, { nodes: N.length, edges: gr.edges.length, ke: +ke.toFixed(3), alpha: +alpha.toFixed(3) }); g.settleSp = null; }
        if (!settledMark) {
          settledMark = true;
          perf.mark("graph_open_settle", openT0, { nodes: N.length, ke: +ke.toFixed(3), alpha: +alpha.toFixed(3) });
          if (pref.loseCtx && glr) setTimeout(() => { if (glr && g.simGen === gen) glr.loseContext(); }, 300);   // smoke hook: WEBGL_lose_context after settle
        }
      }
    }
    const drew = steps || dirty;
    if (drew) { draw(); dirty = false; }
    if (steps) perf.push("graph_frame", perf.now() - fT0, { nodes: N.length, steps, phys: +(fT1 - fT0).toFixed(1), ke: +ke.toFixed(2), alpha: +alpha.toFixed(3) });
    if (quiet) {                       // settled: loop ends, CPU -> 0; publish node coords to the census
      running = false; perf.flush();
      if (!g.graphSettled || drew) { g.graphSettled = true; updateTitle(); }  // pan/zoom moves screen coords
      return;
    }
    g.sim = requestAnimationFrame(step);
  }
  const wake = () => {                  // (re)start the loop; a stopped loop draws once and exits
    if (running) return;
    running = true; phLast = performance.now(); phAcc = 0;
    g.sim = requestAnimationFrame(step);
  };
  const redraw = () => { dirty = true; wake(); };
  // graph-webgl: context lost -> this sim swaps to the 2D path for good (a later open gets a fresh gl canvas)
  g.glLost = () => {
    if (g.simGen !== gen || !glr) return;
    glr = null; cv.classList.remove("gl-on"); if (g.glcv) g.glcv.hidden = true;
    perf.mark("graph_renderer", perf.now(), { renderer: "2d", reason: "contextlost", webgl: 0, ...gpu });
    redraw();
  };
  g.reheat = (a = 0.5) => { calm = 0; quiet = false; g.graphSettled = false; alpha = Math.max(alpha, a); wake(); };
  g.graphNodes = () => {              // census: node screen coords (window px) for the graphnav smoke
    const r = cv.getBoundingClientRect();
    return N.map(p => ({ n: p.n, x: Math.round(r.left + p.x * view.scale + view.tx), y: Math.round(r.top + p.y * view.scale + view.ty) }));
  };
  running = true; step();
  // canvas resized (pane split / window): keep the bitmap crisp, redraw (world unchanged)
  if (g.ro) g.ro.disconnect();
  if (window.ResizeObserver) {
    const ro = g.ro = new ResizeObserver(() => {
      if (cv.hidden || !g.graphOn || g.simGen !== gen) { ro.disconnect(); return; }
      if (cv.width === cv.clientWidth && cv.height === cv.clientHeight) return;
      cv.width = cv.clientWidth; cv.height = cv.clientHeight; redraw();
    });
    ro.observe(cv);
  }
  // wheel: cursor-anchored zoom, stock 0.9 per notch out / 1/0.9 in (R16.5);
  // scale is derived from a notch counter so 3 out + 3 in is EXACTLY 1.00
  cv.onwheel = e => {
    e.preventDefault();
    const r = cv.getBoundingClientRect();
    const mx = e.clientX - r.left, my = e.clientY - r.top;
    const [wx, wy] = toWorld(mx, my);
    view.notch = Math.max(-15, Math.min(15, view.notch + (e.deltaY < 0 ? -1 : 1)));   // 0.9^15 ~ 0.2 .. 0.9^-15 ~ 4.9
    const s = view.notch === 0 ? 1 : Math.pow(0.9, view.notch);
    view.tx = mx - wx * s; view.ty = my - wy * s; view.scale = s;
    redraw();
  };
  // C3: mousedown HIT TESTS. On a node the drag moves the NODE (pinned via
  // fx/fy, so releasing keeps the drop position); on empty canvas it pans the
  // camera exactly as it always did. Click w/o movement still navigates.
  let drag = null, moved = false;
  cv.onmousedown = e => {
    const r = cv.getBoundingClientRect();
    const [wx, wy] = toWorld(e.clientX - r.left, e.clientY - r.top);
    const i = hitTest(wx, wy);
    drag = { x: e.clientX, y: e.clientY, node: i >= 0 ? N[i] : null };
    moved = false;
  };
  cv.onmousemove = e => {
    const r = cv.getBoundingClientRect();
    if (drag) {
      const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
      if (moved || dx * dx + dy * dy > 16) {
        moved = true;
        if (drag.node) {
          // NODE drag. Pin on the first real movement, not on mousedown: a bare
          // click must not stick a node it never moved. Pointer px -> world
          // units via the camera scale, so a zoomed-out drag still tracks.
          const p = drag.node;
          if (p.fx == null) { p.fx = p.x; p.fy = p.y; }
          p.fx += dx / view.scale; p.fy += dy / view.scale;
          p.x = p.fx; p.y = p.fy; p.vx = 0; p.vy = 0;
          g.reheat(0.3);                 // d3 alphaTarget: the neighbours follow the node out
        } else {
          view.tx += dx; view.ty += dy;  // EMPTY canvas: camera pan, unchanged
        }
        drag.x = e.clientX; drag.y = e.clientY;
        redraw();
      }
      return;
    }
    const [x, y] = toWorld(e.clientX - r.left, e.clientY - r.top);
    const h = hitTest(x, y);
    if (h !== hov) { hov = h; redraw(); }
    cv.style.cursor = hov >= 0 ? "pointer" : "";
  };
  cv.onmouseup = () => { drag = null; };
  cv.onmouseleave = () => { drag = null; if (hov >= 0) { hov = -1; redraw(); } cv.style.cursor = ""; };
  cv.onclick = async e => {
    if (moved) { moved = false; return; }               // was a pan, not a click
    const r = cv.getBoundingClientRect();
    const [x, y] = toWorld(e.clientX - r.left, e.clientY - r.top);
    const hit = N[hitTest(x, y)];
    if (!hit) return;
    if (!hit.resolved)                                  // ghost node: create then open (M3 path)
      await createNote(hit.n);
    // R19 (HARD RULE 100ms): the CLICK is the interaction — graph_recenter measures it to the
    // first paint of the response (note painted + graph redrawn at the new centre). The sim
    // settling afterwards is animation: it gets its own informational span graph_settle.
    // Global graph clicks turn the tab into the note (no re-centre): note_open only.
    const rsp = cfg.center() ? otel.begin("graph_recenter", { node: hit.n, from: cfg.center(), nodes: N.length, edges: gr.edges.length }) : null;
    if (rsp) { otel.cancel(g.settleSp); g.settleSp = otel.begin("graph_settle", { node: hit.n, from: cfg.center(), nodes: N.length }); }
    await cfg.onClick(hit.n);
    if (rsp) otel.paint(rsp, { nodes: N.length, edges: gr.edges.length });
    if (g.settleSp && quiet) { otel.end(g.settleSp, { nodes: N.length, edges: gr.edges.length, reheat: false }); g.settleSp = null; }   // centre unchanged / nothing to settle
  };
  // a save can land while the initial fetch is in flight (writeNote sees
  // graphRefresh still null and skips) — refresh once now to close the race
  await g.graphRefresh();
}

/* ---------- M8 local graph (R7.1-R7.5) ---------- */
// R19: the depth-N neighbourhood cut (formerly lgFilter here) lives in
// index.rs GraphCache::local — served over graph_local from the cached adjacency.

async function showLocalGraph(g, t) {  // t = the localgraph tab (kind:"lg")
  cancelAnimationFrame(g.sim);         // clean restart on tab switches
  await startGraph(g, {
    fetch: () => inv("graph_local", { center: t.center, depth: t.depth, inc: t.inc, out: t.out }),   // R19: served from the index adjacency cache
    center: () => t.center,
    onClick: async n => {              // R7.4: navigate the LINKED group; lgFollow re-centers
      const lk = groups().find(x => x.id === t.linkId);
      // R19: the next neighbourhood is fetched IN PARALLEL with the note (graphRefresh picks it up)
      g.prefetch = { n, p: inv("graph_local", { center: n, depth: t.depth, inc: t.inc, out: t.out }) };
      if (lk) await navigate(lk, n);
      await linkSync(t, n);          // R13.3: manual members follow too
    },
  });
  g.lgDepth.value = t.depth; g.lgDv.textContent = t.depth;
  g.lgInc.checked = t.inc; g.lgOut.checked = t.out;
  g.lggear.hidden = false;             // R7.5: settings popover entry
}

async function lgSet(g) {              // R7.5: popover changed -> re-filter live
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind !== "lg") return;
  t.depth = +g.lgDepth.value; t.inc = g.lgInc.checked; t.out = g.lgOut.checked;
  g.lgDv.textContent = t.depth;
  if (g.graphRefresh) await g.graphRefresh();
  updateTitle();                       // republish [lg:center@depth]
}

async function cmdLocalGraph() {       // R7.2: local graph of focused note -> new right split
  if (!state) return;
  const g = fg();
  const t0 = g.active >= 0 ? g.tabs[g.active] : null;
  const center = curOf(g);
  if (!center || (t0 && t0.kind === "lg")) return;
  await flushSave(g);
  const t = { kind: "lg", name: "Graph of " + center.split("/").pop(),
              center, depth: 1, inc: true, out: true, linkId: g.id,
              mode: "source", hist: [], hpos: 0 };
  await splitWith(g, "row", t);        // R7.3: auto-linked to g from birth
  for (const h of groups()) renderTabs(h);  // chain glyph lands on the source tab too
}
$("lgbtn").onclick = cmdLocalGraph;

/* ---------- vault picker ---------- */
function base(p) { return p.replace(/\/+$/, "").split("/").pop() || p; }

function showPicker() {
  pmode = null;
  $("picker").hidden = false;
  $("p-actions").style.display = "";
  $("p-sub").hidden = true;
  $("p-err").textContent = "";
  $("p-close").hidden = !vaultPath;
  loadRecent();
}
/* R1.6: recent-vault rows under the create/open actions — click reopens */
async function loadRecent() {
  const ul = $("p-recent");
  ul.innerHTML = "";
  for (const p of await inv("recent_vaults")) {
    const li = document.createElement("li");
    li.innerHTML = "<b></b><span></span>";
    li.querySelector("b").textContent = base(p);
    li.querySelector("span").textContent = p;
    li.onclick = async () => {
      await leaveVault();                    // F2: flush + disarm BEFORE the root swaps
      try { vaultPath = await inv("set_vault", { path: p }); }
      catch (err) { $("p-err").textContent = String(err); return; }
      $("picker").hidden = true;
      await enterVault();
    };
    ul.appendChild(li);
  }
}
async function browseTo(p) {
  const dirs = await inv("list_dirs", { path: p });
  bpath = p;
  $("p-path").value = p;
  const ul = $("p-dirs");
  ul.innerHTML = "";
  const up = document.createElement("li");
  up.textContent = "..";
  up.onclick = () => browseTo(bpath.replace(/\/[^/]+\/?$/, "") || "/");
  ul.appendChild(up);
  for (const d of dirs) {
    const li = document.createElement("li");
    li.textContent = d + "/";
    li.onclick = () => browseTo((bpath === "/" ? "" : bpath) + "/" + d);
    ul.appendChild(li);
  }
}
async function enterMode(m) {
  pmode = m;
  $("p-actions").style.display = "none";
  $("p-sub").hidden = false;
  $("p-name").hidden = m !== "create";
  $("p-go").textContent = m === "create" ? "Create vault" : "Open this folder";
  $("p-err").textContent = "";
  await browseTo(await inv("home_dir"));
  (m === "create" ? $("p-name") : $("p-path")).focus();
}
$("p-create").onclick = () => enterMode("create");
$("p-open").onclick = () => enterMode("open");
$("p-back").onclick = showPicker;
$("p-close").onclick = () => { $("picker").hidden = true; };
$("p-path").onkeydown = e => { if (e.key === "Enter") browseTo($("p-path").value.trim()); };
$("p-name").onkeydown = e => { if (e.key === "Enter") $("p-go").click(); };
$("p-go").onclick = async () => {
  await leaveVault();                        // F2: flush + disarm BEFORE the root swaps
  try {
    vaultPath = pmode === "create"
      ? await inv("create_vault", { parent: bpath, name: $("p-name").value })
      : await inv("set_vault", { path: bpath });
  } catch (err) { $("p-err").textContent = String(err); return; }
  $("picker").hidden = true;
  await enterVault();
};
async function enterVault() {
  // F2 backstop: whatever route got us here, no timer from the old vault may
  // survive into this one (leaveVault flushes; this only guarantees disarm).
  if (state) for (const h of groups()) { clearTimeout(h.saveT); h.saveT = null; }
  $("vswitch").innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>' + base(vaultPath);
  collapsed = new Set();
  const g = mkGroup();               // M6: one group, wrapped in a one-leaf split tree
  state = { root: { dir: "row", children: [g], fractions: [1] }, focused: null };
  renderLayout();
  focusGroup(g);
  hideAc();
  edtBad = Ed.selfTest();            // R17: renderer + token map invariants -> [edt:] census
  await refreshTree();
  await refreshBm();                 // R9.4: menu label needs the cache early
  const names = await inv("list_notes");
  if (names.length) await openInTab(names[0], "boot");
  else renderTabs(g);
  perf.mark("boot", 0, { notes: names.length });   // perf: page start -> vault ready (first note rendered)
}
/* ---------- R11 external edits (backend watcher -> `vault-changed`) ---------- */
// tab.base = the bytes we last loaded from / saved to disk. bufOf(g) = the
// editor model with an open lp raw row folded in (no side effects), so
// dirty == bufOf(g) !== base even while the raw row is still being typed in.
function setBase(g) {
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (t && !t.kind) t.base = bufOf(g);
}
/* F1 probe: the focused tab holds bytes that are NOT on disk. A failed save
   must leave this token standing (setBase is skipped) — that is the whole
   difference between "retried next keystroke" and "silently discarded". */
function dirtyTok(buf) {
  const g = fg(), t = g && g.active >= 0 ? g.tabs[g.active] : null;
  return t && !t.kind && t.base !== undefined && buf !== t.base ? " [dirty:1]" : "";
}
function bufOf(g) {          // R17: the model IS the buffer (g.editor is its mirror)
  return g.view && g.view.lines ? Ed.text(g) : g.editor.value;
}
// R11.3 AT SAVE TIME. Every write of the active note goes through here.
// Under R17 the model saves on a 250ms debounce while you type (the old design
// only wrote when the caret LEFT the raw row), so our write can now land inside
// the watcher's 1s tick and silently clobber an external append that reached
// disk first. Compare disk against the tab's base and apply the SAME merge rule
// as onVaultChanged before writing: an external append on top of our base is
// folded in (buf + tail), anything else keeps the buffer. One read per debounced
// save — never on the keystroke path.
async function saveBuf(g) {
  const n = curOf(g);
  if (!n) return false;
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  let buf = bufOf(g);
  if (t && !t.kind && t.base != null) {
    const disk = await inv("read_note", { name: n });
    if (disk !== t.base && disk !== buf) {
      const merged = disk.startsWith(t.base) ? buf + disk.slice(t.base.length) : buf;
      // F1 x R11.3: reloadInPlace sets base = the text we are ABOUT to write.
      // If the write then fails, base == buf and the tab reads CLEAN — the
      // exact loss F1 is about, reintroduced through the merge path. base
      // means "the bytes we last saw on disk", so put the DISK bytes there:
      // correct on failure (dirty, and the next merge still sees the append
      // as an append), and overwritten by setBase below on success.
      if (merged !== buf) { await reloadInPlace(g, merged); buf = merged; t.base = disk; }
    }
  }
  if (!await saveNote(n, buf)) return false;   // F1: a failed save leaves the tab DIRTY, banner up
  setBase(g);
  return true;
}
// R11.2: replace the ACTIVE tab's text in place — caret line/col + scroll kept
async function reloadInPlace(g, text) {
  const t = g.tabs[g.active];
  const c = g.lpActive ? Ed.caret(g) : null;   // R17: caret survives the reload
  Ed.setText(g, text); t.base = text;
  if (t.mode === "reading") await preview(g);
  else await lpRender(g, c ? c.l : -1, c ? c.c : 0);   // dirty rows only
  updateStatus(g);
}
// remove a tab WITHOUT flushing (R11.4: the file is gone; a flush would resurrect it)
async function dropTab(g, i) {
  if (i === g.active) { clearTimeout(g.saveT); g.saveT = null; g.lpActive = null; }
  unlinkTab(g, g.tabs[i], true);    // R13.4
  dropView(g.tabs[i]);
  g.tabs.splice(i, 1);
  if (!g.tabs.length && groups().length > 1) return collapseGroup(g);
  if (g.active >= g.tabs.length) g.active = g.tabs.length - 1;
  else if (i < g.active) g.active--;
  await loadActive(g);
}
let vcCount = 0;                               // census [vc:N] — events handled
async function onVaultChanged(c) {
  vcCount++;
  const gone = new Set([...c.removed, ...c.renamed.map(r => r[0])]);
  const mod = new Set(c.modified);
  for (const g of groups()) {
    for (let i = g.tabs.length - 1; i >= 0; i--) {   // R11.4: delete/rename closes the tab
      const t = g.tabs[i];
      if (!t.kind && gone.has(t.name)) await dropTab(g, i);
    }
    for (const x of g.tabs) if (!x.kind && x !== g.tabs[g.active] && mod.has(x.name)) x.stale = true;  // R20: retained views re-read on switch
    const t = g.active >= 0 ? g.tabs[g.active] : null;
    if (!t || t.kind || !mod.has(t.name)) continue;
    const ext = await inv("read_note", { name: t.name });
    const buf = bufOf(g);
    if (buf === ext) { t.base = ext; continue; }
    if (buf === t.base) { await reloadInPlace(g, ext); continue; }   // clean: take disk
    // R11.3 dirty: never lose typed text. External append on top of our base
    // merges (buf + tail); anything else keeps the buffer. Either way re-save.
    const merged = ext.startsWith(t.base) ? buf + ext.slice(t.base.length) : buf;
    if (merged !== buf) await reloadInPlace(g, merged);
    t.base = ext; scheduleSave(g);
  }
  await refreshTree();
  await refreshBm();
  for (const g of groups()) if (g.graphOn && g.graphRefresh) await g.graphRefresh();
  if (rightOpen) rSchedule();
  updateTitle();
}
window.__TAURI__.event.listen("vault-changed", e => onVaultChanged(e.payload));
// R31.1: a real OS drop arrives here, from Rust, never from a DOM drop event.
window.__TAURI__.event.listen("drop-files", e => attachDrop(e.payload || []));

$("vswitch").onclick = showPicker;

(async () => {
  await bootTheme();             // the root attribute is set synchronously inside
                                 // (system default), then the STORED choice replaces
                                 // it — one call site decides the boot theme.
  await bootPalette();           // the OTHER axis, after the mode and before any
                                 // vault content: a stored palette must be on the
                                 // root before webkit's first frame, or the user
                                 // sees the default flash past on every start.
  SAVE_MS = await inv("save_debounce_ms").catch(() => 250);   // F2 smoke hook
  const sw = await inv("get_sidebar_w").catch(() => null);   // ux-4
  if (sw >= 150) $("side").style.width = Math.min(600, sw) + "px";
  const rt = await inv("get_rside_tab").catch(() => null);   // rsidebar
  await hkLoad();                                             // R14 custom hotkeys
  await setRTab(RPANES[rt] ? rt : "bl", false);
  vaultPath = await inv("vault_get");
  if (vaultPath) await enterVault(); else showPicker();
  // R30 / T2: the table AND the nav DOM are warmed off the open path — the nav is
  // 19 entries that never change, so building it at boot into the hidden modal
  // takes the only unavoidable DOM work out of the open keystroke. The pane stays
  // lazy (105 rows are never all built), which is the half that actually scales.
  smodelPrefetch().then(buildSettingsNav, () => {});
})();


/* ---------- R14 Settings modal (Ctrl+,) — Hotkeys page. Layout is fixed-size
   (900x600 at 190,100 on the 1280x800 smoke screen, 32px rows) so the smoke
   can click chips by coordinate. census [modal:settings] [hk:<rows>]
   [hkrec:<id>] while recording, [hkc:N] conflicting commands. ---------- */
let settingsOpen = false, hkChip = "all", hkRec = null, hkInfo = "";
/* R30 (feedback #19): the nav tree and the row lists are a DATA TABLE in Rust
   (src-tauri/src/settings.rs = the black-box recon transcript of stock 1.13.7).
   The frontend authors NO structure: it fetches `settings_model` once and
   renders it. Prefetched at boot, so opening the modal does synchronous work
   only — T2's 100 ms first-paint ceiling has no round trip inside it.
   NAV IS EAGER (19 entries, built once per session), PANE IS LAZY: only the
   selected pane exists in the DOM, so 105 rows are never all built at once. */
let SMODEL = null, SMODELP = null, sPane = "hotkeys";
function smodelPrefetch() {
  if (!SMODELP) SMODELP = inv("settings_model").then(m => (SMODEL = m), e => { SMODELP = null; throw e; });
  return SMODELP;
}
function cmdSettings() {
  if (!settingsOpen) sfpT0 = performance.now();   // R30.12: t0 is the OPEN request, not the paint
  settingsOpen ? closeSettings() : openSettings();
}
/* ---------- R30.12 PERF: open-keystroke -> first paint of the modal ----------
   T2's ceiling is 100 ms and the brief says measure it, do not assume it. Same
   rAF -> task pattern as Ed.cmEnd (ui/editor.js:614) so the number is
   comparable with [cm:] and otel's key_to_paint wall_ms: t0 is taken in
   cmdSettings (the command the keystroke dispatches, before any DOM work), t1
   inside a task queued from the frame that carries the modal — i.e. after the
   frame is committed, not merely after the DOM is mutated.
   Published as [sfp:<last>/<max>/<avg>/<n>] while the modal is open, so the
   `settings` smoke phase reads it out of the window title instead of trusting
   a claim in a doc. [sfpw:<last>/<max>] is the same span WITHOUT the frame
   wait — the synchronous build cost, which is the part this code owns. The
   smoke host is a 4-vCPU VM running three goals with software GL, where the
   compositor alone can stall a frame for half a second; separating the two
   numbers is what keeps a host stall from reading as a slow modal, and what
   makes the slow modal (were it ever slow) impossible to hide behind one. */
let sfpT0 = -1, sfpMs = -1, sfpMax = 0, sfpN = 0, sfpSum = 0, sfpW = -1, sfpWMax = 0;
function sfpEnd() {
  if (sfpT0 < 0) return;
  const t0 = sfpT0; sfpT0 = -1;
  sfpW = Math.round((performance.now() - t0) * 100) / 100;   // work only: DOM built, frame not yet committed
  if (sfpW > sfpWMax) sfpWMax = sfpW;
  requestAnimationFrame(() => setTimeout(() => {
    const ms = Math.round((performance.now() - t0) * 100) / 100;
    sfpMs = ms; sfpN++; sfpSum += ms;
    if (ms > sfpMax) sfpMax = ms;
    if (typeof otel !== "undefined" && otel.span)
      otel.span("settings_open", { tabs: SMODEL ? SMODEL.nav.length : 0, rows_built: sRowsShown }, ms);
    updateTitle();
  }, 0));
}
/* [set:<tabs>/<rows>/<enabled>] — the DATA TABLE's own counts (R30.14), so a
   row silently added, dropped or flipped to enabled moves a number the gate
   asserts. [spane:<id>/<rows>/<enabled>] is the pane currently BUILT, which is
   how the phase proves a nav click actually swapped the pane (OCR alone cannot
   distinguish "clicked" from "painted the same pane again"). */
/* [spal:<centre x>,<centre y>,<label>] — the Appearance ▸ Themes control's OWN
   measured rect and the text it is currently showing, published for the same
   reason [mg:] publishes the context menu's geometry: the settings-UI route is
   proven by a phase that must CLICK this control, and a hardcoded coordinate
   would be a guess that goes stale the moment a row above it gains a line of
   description. The label is in the token too, so "the control shows the active
   palette" is assertable without OCR. Absent when the pane holding it is not
   built — which is itself the assertion that the control is only on Appearance. */
function spalTok() {
  const d = document.getElementById("spalette");
  if (!d) return "";
  const b = d.getBoundingClientRect();
  // the same sanitising updateTitle's local tokq does (brackets and | would
  // break the census grammar); inline because tokq is scoped to updateTitle.
  const lbl = String(d.textContent || "").replace(/[[\]|]/g, "").slice(0, 40);
  return " [spal:" + Math.round(b.left + b.width / 2) + "," +
         Math.round(b.top + b.height / 2) + "," + lbl + "]";
}
function setTok() {
  if (!SMODEL) return "";
  const e = SMODEL.rows.reduce((n, r) => n + (r.enabled ? 1 : 0), 0);
  return " [set:" + SMODEL.nav.length + "/" + SMODEL.rows.length + "/" + e + "]" +
         " [spane:" + sPane + "/" + sRowsShown + "/" + sEnabledShown + "]" +
         spalTok() +
         (sfpMs >= 0 ? " [sfp:" + sfpMs + "/" + sfpMax + "/" +
                       (Math.round(sfpSum / sfpN * 100) / 100) + "/" + sfpN + "]" : "") +
         (sfpW >= 0 ? " [sfpw:" + sfpW + "/" + sfpWMax + "]" : "");
}
async function openSettings() {
  if (!SMODEL) await smodelPrefetch();   // cold open only (prefetched at boot)
  settingsOpen = true; hkRec = null;
  closeModal();
  $("settings").hidden = false;
  buildSettingsNav();
  showSettingsPage(sPane);
  sfpEnd();        // BEFORE the census: updateTitle walks the whole document for
  updateTitle();   // the R22 overflow probe, and that is the instrument's cost, not the modal's
}
function closeSettings() {
  settingsOpen = false; hkRec = null;
  /* THE PANE'S OWN MENU GOES WITH IT (goal/theme-1984). Appearance ▸ Themes
     opens a .ctxmenu anchored to a control INSIDE this modal; .ctxmenu is
     position:fixed at z-index 60, so a menu left open when the modal is hidden
     goes on painting over the note, anchored to a control that is no longer on
     screen. Measured: 7005 px of a stray Default/1984 card still floating after
     Esc, which is how the smoke phase found it. */
  closeMenu();
  $("settings").hidden = true;
  updateTitle();
}
/* the left nav, 1:1 with stock's order and grouping (Options 1-9, then the
   Core plugins group) — built ONCE from the model, never re-created on a tab
   switch: selection is a class toggle, so clicking a nav entry costs one pane
   build and nothing else. */
function buildSettingsNav() {
  const nav = $("snav");
  if (nav.dataset.built === "1") return;
  nav.innerHTML = "";
  let group = null;
  for (const e of SMODEL.nav) {
    if (e.group !== group) {
      group = e.group;
      const h = document.createElement("div"); h.className = "snavh"; h.textContent = group;
      nav.appendChild(h);
    }
    const d = document.createElement("div");
    d.className = "snavi"; d.textContent = e.entry;
    d.dataset.pane = e.id; d.tabIndex = 0;
    d.onclick = () => showSettingsPage(e.id);
    d.onkeydown = ev => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); showSettingsPage(e.id); } };
    nav.appendChild(d);
  }
  nav.dataset.built = "1";
}
/* `id` is a pane id from the model ("general", "hotkeys", "cp-dailynotes", ...);
   a stock ENTRY NAME is accepted too, so older call sites keep working. */
function showSettingsPage(id) {
  const e = SMODEL.nav.find(n => n.id === id) || SMODEL.nav.find(n => n.entry === id);
  const pane = e ? e.id : "hotkeys";
  sPane = pane;
  for (const d of $("snav").querySelectorAll(".snavi")) d.classList.toggle("sel", d.dataset.pane === pane);
  const pg = $("spage"); pg.innerHTML = ""; pg.className = "";
  const rows = SMODEL.rows.filter(r => r.tab === pane);   // what this pane owes the census
  sRowsShown = rows.length;
  sEnabledShown = rows.filter(r => r.enabled).length;
  if (pane !== "hotkeys") {
    if (!rows.length) {                     // per-core-plugin panes: nav entry + empty pane (brief §2 OUT)
      const d = document.createElement("div"); d.className = "sempty";
      d.textContent = (e ? e.entry : pane) + " — nothing to configure yet.";
      pg.appendChild(d);
    } else buildSettingsRows(pg, rows, pane);
    return updateTitle();                   // [spane:] follows the pane that is actually built
  }
  const bar = document.createElement("div"); bar.id = "hkbar";
  const inp = document.createElement("input");
  inp.id = "hkfilter"; inp.placeholder = "Filter..."; inp.spellcheck = false; inp.autocomplete = "off";
  inp.oninput = renderHk;
  const clr = document.createElement("span"); clr.id = "hkclear"; clr.textContent = "⊗"; clr.title = "Clear";
  clr.onclick = () => { inp.value = ""; renderHk(); inp.focus(); };
  bar.append(inp, clr);
  const chips = document.createElement("div"); chips.id = "hkchips";
  const list = document.createElement("div"); list.id = "hklist";
  pg.append(bar, chips, list);
  renderHk();
  inp.focus();
}
/* R30 ROWS — the Options-tab panes, built from the data table, never from HTML.
   Stock has no disabled style to copy (recon Q4: where a control does not apply
   stock REMOVES it), so ours is invented ONCE, here, and applied to every row
   the table does not back with a real config key:
     - .dis + aria-disabled="true": it reads as disabled, it is not merely grey
     - the control is a DIV, never a form element — there is no tab stop to take
       away, no default activation to suppress, and no handler is attached
     - pointer-events:none (style.css) so click and hover do nothing either
     - ONE hover string for all of them (SDIS_TITLE), no per-row "coming soon"
   Geometry comes from docs/stock-settings-recon/measurements.txt: 76 px row with
   a one-line description, +16 px per extra line, 17 px card inset, 1 px
   separator, controls at the card's right edge. The palette stays rustidian's
   dark theme — that delta is recorded in R30. */
const SDIS_TITLE = "Not implemented yet";
/* ---- Appearance ▸ Themes: the settings-UI route onto the PALETTE axis ----
   Two routes reach this feature and each is proven separately: Ctrl+P (the
   "Use theme: <name>" registry entries) and this control. They share
   choosePalette(), so neither can drift into a second definition of what
   selecting a palette means. */
const paletteLabel = id => (PALETTES.find(([p]) => p === id) || PALETTES[0])[1];
function paletteCtl() {
  const d = document.createElement("div");
  d.className = "sctl dropdown live";
  d.id = "spalette";
  d.tabIndex = 0;                             // an ENABLED row is a tab stop, unlike the disabled ones
  d.setAttribute("role", "button");
  d.setAttribute("aria-haspopup", "menu");
  d.textContent = paletteLabel(themePalette);
  const open = ev => { ev.preventDefault(); ev.stopPropagation(); openPaletteMenu(d); };
  d.onmousedown = ev => ev.stopPropagation();  // the document-level closer must not eat this
  d.onclick = open;
  d.onkeydown = ev => { if (ev.key === "Enter" || ev.key === " ") open(ev); };
  return d;
}
/* the control shows the LIVE state, not the state it was built in: selecting a
   palette from Ctrl+P while the pane is open must move this text too, or the
   two routes disagree on screen about which palette is active. */
function renderPaletteCtl() {
  const d = document.getElementById("spalette");
  if (d) d.textContent = paletteLabel(themePalette);
}
function openPaletteMenu(anchor) {
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  for (const [id, label] of PALETTES) {
    const it = document.createElement("div");
    // ✓ on the active one, the same radio idiom the tab menu uses — so the
    // census [menu:] carries which palette is active as well as the choices.
    it.textContent = (id === themePalette ? "✓ " : "") + label;
    it.onmousedown = ev => ev.stopPropagation();
    it.onclick = () => { closeMenu(); choosePalette(id); };
    m.appendChild(it);
  }
  const b = anchor.getBoundingClientRect();
  // .ctxmenu is z-index 60 and #settings is 55, so the menu is above the modal;
  // placeMenu clamps it to the viewport by its measured box (R22).
  placeMenu(m, Math.round(b.left), Math.round(b.bottom + 4));
}
let sRowsShown = 0, sEnabledShown = 0;
function sctl(r) {                            // the control cell for one row, or null
  const v = r.default_shown === "-" ? "" : r.default_shown;
  /* THE ONE LIVE DROPDOWN (goal/theme-1984). Everything else in this pane is a
     transcription of stock's pixels with no handler; Appearance ▸ Themes is
     backed by the "palette" config key (settings.rs BACKED), so it is rendered
     as a control that actually does something. It deliberately does NOT use a
     native <select>: a native popup is an OS-level window, invisible to the
     screenshot-and-census rig, so the ONE settings control that changes the
     app's appearance would be the one no phase could prove. It opens the app's
     own .ctxmenu instead — the same widget the tab menu uses, published in the
     census as [menu:<labels>] with measured geometry, so the settings-UI route
     is drivable and assertable exactly like every other menu in the app. */
  if (r.key === "palette") return paletteCtl();
  const d = document.createElement("div");
  d.className = "sctl " + r.control;
  const parts = (t, cls) => t.split(" / ").forEach(p => {
    const s = document.createElement("span"); s.className = cls; s.textContent = p; d.appendChild(s);
  });
  switch (r.control) {
    case "none": return null;                 // stock shows label + description and nothing else
    case "toggle":
      if (/^on\b/.test(v)) d.classList.add("on");       // "on", "on + gear + plus", ...
      d.appendChild(document.createElement("i"));       // the knob
      break;
    case "dropdown": case "text": case "list":
      d.textContent = v.length > 48 ? v.slice(0, 47) + "…" : v;
      break;
    case "color": {
      const sw = document.createElement("span"); sw.className = "sw";
      d.appendChild(sw); d.appendChild(document.createTextNode(v.replace(" + reset", "")));
      break;
    }
    case "slider": {
      const t = document.createElement("span"); t.className = "trk";
      const n = document.createElement("span"); n.className = "val"; n.textContent = v.replace(" + reset", "");
      d.append(n, t);
      break;
    }
    case "button": case "buttons":            // "(accent-filled)" = stock's one filled button
      parts(v.replace(" (accent-filled)", ""), "sbtn" + (v.includes("(accent-filled)") ? " acc" : ""));
      break;
    case "nav":
      if (v) { const s = document.createElement("span"); s.className = "nv"; s.textContent = v; d.appendChild(s); }
      d.appendChild(document.createTextNode("›"));
      break;
    default:
      d.textContent = v;
  }
  return d;
}
/* stock's plugin rows carry a gear ("options") and/or a plus ("add to sidebar")
   glyph LEFT of the toggle — transcribed in default_shown as "on + gear + plus".
   The gear is DRAWN (inline SVG, built with createElementNS): the bundled fonts
   have no U+2699, so a text gear renders as nothing at all — which is how the
   first pass of this pane shipped a row that silently lost its icon.
   Same rule as every other control: no handler, no tab stop, aria-hidden. */
function svgel(n, at) {
  const e = document.createElementNS("http://www.w3.org/2000/svg", n);
  for (const k in at) e.setAttribute(k, at[k]);
  return e;
}
function gearSvg() {
  const s = svgel("svg", { width: 14, height: 14, viewBox: "0 0 16 16", fill: "none",
                           stroke: "currentColor", "stroke-width": 1.3, "aria-hidden": "true" });
  s.appendChild(svgel("circle", { cx: 8, cy: 8, r: 2.4 }));
  for (let i = 0; i < 8; i++) {                 // 8 teeth, radial strokes
    const a = i * Math.PI / 4, c = Math.cos(a), n = Math.sin(a);
    s.appendChild(svgel("line", { x1: (8 + c * 4.4).toFixed(2), y1: (8 + n * 4.4).toFixed(2),
                                  x2: (8 + c * 6.8).toFixed(2), y2: (8 + n * 6.8).toFixed(2) }));
  }
  return s;
}
function sicons(v) {
  const want = ["gear", "plus"].filter(n => v.includes(n));
  if (!want.length) return null;
  const d = document.createElement("div"); d.className = "sicons";
  for (const n of want) {
    const s = document.createElement("span"); s.className = "sico " + n;
    s.appendChild(n === "gear" ? gearSvg() : document.createTextNode("+"));
    d.appendChild(s);
  }
  return d;
}
/* CHROME rows: the transcript marks a pane's non-setting furniture with a
   parenthesised label — "(search field)" at the top of Core plugins, the
   "(security blurb)" card on Community plugins. Stock draws them as part of the
   pane, not as a label/description row, so they get their own shape here.
   They are keyless, therefore disabled, therefore inert like everything else. */
function schrome(r) {
  if (r.label === "(search field)") {
    const d = document.createElement("div"); d.className = "ssearch dis";
    d.setAttribute("aria-disabled", "true"); d.title = SDIS_TITLE;
    d.textContent = r.desc.replace(/ placeholder$/, "");
    return d;
  }
  if (r.label === "(security blurb)") {
    const wrap = document.createElement("div"); wrap.className = "sblurbwrap";
    const m = r.desc.match(/^(.*?)\s*\+ \d+ cards \((.*)\)$/);
    const p = document.createElement("div"); p.className = "sblurb";
    p.textContent = m ? m[1] : r.desc;
    wrap.appendChild(p);
    if (m) {                                  // stock's 2x2 grid of security cells
      const g = document.createElement("div"); g.className = "sgrid";
      for (const t of m[2].split(" / ")) {
        const c = document.createElement("div"); c.className = "scell"; c.textContent = t;
        g.appendChild(c);
      }
      wrap.appendChild(g);
    }
    return wrap;
  }
  return null;
}
function buildSettingsRows(pg, rows, pane) {
  /* Core plugins is stock's LIST pane, not a settings-card pane: denser rows
     (52 px for a one-line description, measurements.txt LIST PANE) and a search
     field at the top instead of a section heading. */
  pg.className = "rows" + (pane === "coreplugins" ? " list" : "");
  let section = null, card = null;   // null !== "" so the first row always opens a card
  for (const r of rows) {
    const sec = r.section || "";
    if (sec !== section) {                    // a new section = its own heading + card, like stock
      section = sec;
      if (sec) { const h = document.createElement("div"); h.className = "ssec"; h.textContent = sec; pg.appendChild(h); }
      card = document.createElement("div"); card.className = "scard"; pg.appendChild(card);
    }
    const ch = schrome(r);                    // furniture, not a setting row
    if (ch) { card.appendChild(ch); continue; }
    const row = document.createElement("div");
    row.className = "srow" + (r.enabled ? "" : " dis");
    if (!r.enabled) { row.setAttribute("aria-disabled", "true"); row.title = SDIS_TITLE; }
    const info = document.createElement("div"); info.className = "sinfo";
    const lb = document.createElement("div"); lb.className = "slabel"; lb.textContent = r.label;
    const link = r.desc === "(external link row)";   // stock's bare link line, no description
    if (link) { lb.classList.add("slink"); row.classList.add("linkrow"); }
    info.appendChild(lb);
    if (!link && r.desc && r.desc !== "-") {
      const ds = document.createElement("div"); ds.className = "sdesc";
      r.desc.split(" | ").forEach((p, i) => {   // " | " marks stock's inline link tail
        const s = document.createElement("span");
        if (i) s.className = "slink";
        s.textContent = (i ? " " : "") + p;
        ds.appendChild(s);
      });
      info.appendChild(ds);
    }
    row.appendChild(info);
    const ic = sicons(r.default_shown || "");
    if (ic) row.appendChild(ic);
    const c = sctl(r);
    if (c) row.appendChild(c);
    card.appendChild(row);
  }
}
const HKCHIPS = [["all", "All"], ["assigned", "Assigned"], ["mine", "Assigned by me"], ["unassigned", "Unassigned"]];
function hkRows() {                          // fuzzy filter AND active chip
  const q = ($("hkfilter") ? $("hkfilter").value : "").trim();
  const conf = hkConflicts();
  const confIds = new Set(Object.values(conf).flat());
  return CMDS.filter(c => {
    if (fuzzy(q, c.name) < 0) return false;
    const chs = hkChords(c);
    switch (hkChip) {
      case "assigned":   return chs.length > 0;
      case "mine":       return c.id in hkUser && chs.length > 0;
      case "unassigned": return chs.length === 0;
      case "conflicts":  return confIds.has(c.id);
      default:           return true;
    }
  });
}
function renderHk() {
  if (!settingsOpen || !$("hklist")) return;
  const conf = hkConflicts();
  const confIds = new Set(Object.values(conf).flat());
  if (hkChip === "conflicts" && !confIds.size) hkChip = "all";
  const chips = $("hkchips"); chips.innerHTML = "";
  if (confIds.size) {
    const d = document.createElement("span"); d.className = "hkchip conf" + (hkChip === "conflicts" ? " sel" : "");
    d.textContent = "Conflicts " + confIds.size;
    d.onclick = () => { hkChip = "conflicts"; $("hkfilter").value = ""; renderHk(); };
    chips.appendChild(d);
  }
  for (const [k, l] of HKCHIPS) {
    const d = document.createElement("span"); d.className = "hkchip" + (hkChip === k ? " sel" : "");
    d.textContent = l;
    d.onclick = () => { hkChip = k; renderHk(); };
    chips.appendChild(d);
  }
  const list = $("hklist"); list.innerHTML = "";
  const rows = hkRows();
  for (const c of rows) {
    const chs = hkChords(c);
    const row = document.createElement("div"); row.className = "hkrow";
    const nm = document.createElement("span"); nm.className = "hkname"; nm.textContent = c.name;
    const keys = document.createElement("span"); keys.className = "hkkeys";
    if (hkRec === c.id) {
      const k = document.createElement("span"); k.className = "hkkey rec"; k.textContent = "Press hotkey...";
      keys.appendChild(k);
    } else if (!chs.length) {
      const k = document.createElement("span"); k.className = "hkkey blank"; k.textContent = "Blank";
      keys.appendChild(k);
    } else for (const ch of chs) {
      const k = document.createElement("span"); k.className = "hkkey" + (conf[ch] ? " conf" : "");
      k.textContent = chordLabel(ch);
      const x = document.createElement("span"); x.className = "hkx"; x.innerHTML = '<svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M3 3l10 10M13 3L3 13"/></svg>'; x.title = "Remove";
      x.onclick = () => { hkSet(c, chs.filter(y => y !== ch)); renderHk(); };
      k.appendChild(x);
      keys.appendChild(k);
    }
    const rs = document.createElement("button"); rs.className = "hkbtn hkrestore"; rs.innerHTML = '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8a5 5 0 1 0 1.5-3.6"/><path d="M3 2.5v3h3"/></svg>'; rs.title = "Restore default";
    rs.style.visibility = c.id in hkUser ? "visible" : "hidden";
    rs.onclick = () => { delete hkUser[c.id]; hkSave(); renderHk(); };
    const add = document.createElement("button"); add.className = "hkbtn hkadd"; add.innerHTML = '<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><circle cx="8" cy="8" r="6"/><path d="M8 5v6M5 8h6"/></svg>'; add.title = "Customize this command";
    add.onclick = () => { hkRec = hkRec === c.id ? null : c.id; renderHk(); };
    row.append(nm, keys, rs, add);
    list.appendChild(row);
  }
  hkInfo = " [hk:" + rows.length + "]" + (hkRec ? " [hkrec:" + hkRec + "]" : "") + (confIds.size ? " [hkc:" + confIds.size + "]" : "");
  updateTitle();
}
function hkKey(e) {                          // keyboard while settings is open
  if (hkRec) {                               // recording: next non-modifier chord binds, Esc cancels
    e.preventDefault(); e.stopPropagation();
    if (e.key === "Escape") { hkRec = null; return renderHk(); }
    const ch = chordOf(e);
    if (!ch) return;
    const c = CMDS.find(x => x.id === hkRec);
    hkRec = null;
    if (c && !hkChords(c).includes(ch)) hkSet(c, [...hkChords(c), ch]);
    renderHk();
    return $("hkfilter") && $("hkfilter").focus();
  }
  if (e.key === "Escape") { e.preventDefault(); return closeSettings(); }
  const c = keymap[chordOf(e)];
  if (c && c.id === "app:open-settings") { e.preventDefault(); closeSettings(); }
}
$("sclose").onclick = closeSettings;
$("settings").onmousedown = e => { if (e.target === $("settings")) closeSettings(); };

/* ---------- R33 frameless window: the app IS the title bar ----------
   With `decorations: false` there is no WM frame, so moving, resizing,
   minimising, maximising and closing the window are this file's job.

   WHY THE GESTURES ARE OURS AND NOT THE WM'S: the obvious implementation hands
   the press to the WM (start_dragging / _NET_WM_MOVERESIZE) and lets it move the
   window. That does nothing where there is NO window manager — which is exactly
   the state of every Xvfb display this repo tests on, and an undecorated window
   there would be permanently stuck. So a gesture sends the ANCHOR rect it started
   from plus the cursor delta, and win_gesture applies the resulting geometry
   (absolute at every step: a dropped pointermove cannot make the window drift).
   Cost: the window follows the cursor one frame late. R33 records the divergence.

   THE DRAG REGION is the empty part of ANY pane header (the strip itself and the
   background of a tab bar, never a tab or a button) — stock drags by the same
   empty tab-row space, in every pane, and R37 extends this region from the top
   row to all of them. It USED to end at `top <= 2`, i.e. the top row only: a
   split pane's own tab bar, halfway down the window, did nothing at all. That is
   the case R37 exists to fix, so the test is gone, and with it the only thing
   that made a non-first pane's header dead space.
   WHAT STILL MUST NOT DRAG, and why each is safe without an extra test here:
     - a tab, its close button, .modebtn, #rtoggle: each is its OWN event target,
       and none of them carries the .tabbar/.tabs class this function requires;
     - the .wfinset reservation at the right end of the top row: #wframe is
       `position: fixed; right: 0; width: 140px; z-index: 200` (ui/style.css),
       i.e. it COVERS exactly the reserved strip, so a press there targets the
       frame or one of its buttons and never reaches the tab bar underneath.
       (.wfinset is on at most one row — placeRToggle — and it is always that
       top-right one, so no lower pane reserves space nobody covers.) */
/* `var`, not `let`: updateTitle() runs during this file's own top-level init,
   BEFORE execution reaches this block at the bottom — a `let` would still be in
   its temporal dead zone and reading it would throw, turning the whole census
   into [jserr:] instead of a title. */
var wfMax = false;          // last state win_toggle_max reported -> census [wfm:]
var wfArmed = false;          // the gesture listeners are installed -> census [wf:3d8]
var wfA = null;               // gesture in flight: {dir, sx, sy, r, moved, seq}
/* wfSeq IS THE FIX FOR A ZOMBIE GESTURE, and it was paid for by a whole recon pass
   in which SIX of the eight resize directions read as broken. wfBegin has to AWAIT
   win_rect (the anchor), and a synthetic click releases the button long before that
   IPC resolves: pointerup ran, wfEnd set wfA = null, and THEN the await resumed and
   assigned wfA — a gesture nobody was holding. Every later pointermove, including
   the next drag's, then kept resizing with the DEAD gesture's direction and press
   point: the west drag reported `[wfg:e:...+-1158,0]` (direction e, delta measured
   from the previous drag's press x) and the window walked to its 320px floor.
   So every press takes a ticket, and a release or a newer press invalidates it: an
   anchor that arrives after its ticket expired is dropped instead of becoming state. */
var wfSeq = 0;
var wfPend = null, wfRaf = 0; // rAF-coalesced pointer delta (the LAST one always lands)
var wfLast = "-";          // last gesture, published as [wfg:] for the smoke assertions
var wfRet = null;          // R33.9: where keyboard focus came from before Alt+Space
/* a gesture is a SEQUENCE, and only the sequence says which step went wrong: a press
   with no begin, a begin whose anchor arrived after its ticket expired, a flush after
   the release. [wfg:] carries the last flush only, which is why the zombie gesture
   read as "our maths is wrong" for two iterations. Last 8 steps, oldest first. */
var wfLogA = [];
function wfLog(t) { wfLogA.push(t); if (wfLogA.length > 8) wfLogA.shift(); }
var wfDownT = 0, wfDownX = 0, wfDownY = 0;   // double-press detector (= maximise)

function wfDragRegion(t) {    // is this event target part of the drag region?
  if (!t || !t.classList) return false;
  if (t.id === "wframe") return true;
  // R37: ANY pane's tab-bar background, not just the top row's (see the block above)
  return t.classList.contains("tabbar") || t.classList.contains("tabs");
}
/* R37 census [hdr:<i>@<free>/<tab0>/<modebtn>|...] — one entry per pane, in
   document order, every field an `x,y` CENTRE in CLIENT px (add the window's
   outer origin to get the screen point xdotool needs), `-` when that thing does
   not exist here:
     free    = the middle of the pane header's FREE space, i.e. the R37 drag
               region: between the last tab's right edge and the right end of
               .tabs (which already stops before the .wfinset reservation,
               because that padding is on the .tabbar and .tabs is its content).
     tab0    = the first tab, the thing that must KEEP switching and reordering.
     modebtn = the reading-view toggle, which must keep toggling.
   The smoke must never GUESS these points: a press that lands 1px inside the
   last tab exercises the tab path, not the window drag, and still "passes".
   Same reason [stx:] (R33.13) publishes the right strip's buttons. */
function wfHdrTok() {
  const ctr = el => {
    if (!el) return "-";
    const r = el.getBoundingClientRect();
    return r.width < 1 || r.height < 1 ? "-" : Math.round(r.left + r.width / 2) + "," + Math.round(r.top + r.height / 2);
  };
  const out = [];
  let i = 0;
  for (const el of document.querySelectorAll("#main .pane .tabs")) {
    i++;
    const r = el.getBoundingClientRect();
    let left = r.left;
    for (const t of el.querySelectorAll(".tab")) left = Math.max(left, t.getBoundingClientRect().right);
    const free = r.right - left >= 16 && r.height > 4
      ? Math.round((left + r.right) / 2) + "," + Math.round(r.top + r.height / 2) : "-";
    const bar = el.parentNode;
    out.push(i + "@" + free + "/" + ctr(el.querySelector(".tab")) +
             "/" + ctr(bar && bar.querySelector ? bar.querySelector(".modebtn") : null));
  }
  return out.join("|");
}
async function wfToggleMax() {
  try { wfMax = await inv("win_toggle_max"); } catch (e) { noteErr(e); }
  updateTitle();
}
function wfFlush() {
  wfRaf = 0;
  const a = wfA, p = wfPend;
  if (!a || !p) { wfPend = null; return; }
  if (!a.r) return;             // the anchor has not arrived yet: KEEP the delta pending
  wfPend = null;
  /* the anchor rect is the single thing a gesture can get wrong invisibly: send the
     wrong w/h and the window resizes to a number nobody asked for, which reads as
     "our resize is broken" when it is really "win_rect lied". So the last gesture
     is published in the census — dir, the anchor it used, and the delta — and the
     smoke phase asserts it against the geometry the WM reports. */
  wfLog("f" + a.dir + "#" + a.seq);
  wfLast = a.dir + ":" + a.r.x + "," + a.r.y + "," + a.r.w + "," + a.r.h + "+" + p.dx + "," + p.dy;
  inv("win_gesture", { dir: a.dir, x: a.r.x, y: a.r.y, w: a.r.w, h: a.r.h, dx: p.dx, dy: p.dy })
    .then(() => updateTitle()).catch(noteErr);
}
async function wfBegin(dir, ev, el) {
  if (ev.button !== 0) return;
  if (dir === "move") {       // second press on the strip inside 400ms = maximise/restore
    const now = Date.now(), near = Math.abs(ev.screenX - wfDownX) + Math.abs(ev.screenY - wfDownY) < 8;
    const dbl = now - wfDownT < 400 && near;
    wfDownT = now; wfDownX = ev.screenX; wfDownY = ev.screenY;
    if (dbl) { wfDownT = 0; return wfToggleMax(); }
  }
  ev.preventDefault();
  const seq = ++wfSeq;
  // ACTIVE IMMEDIATELY, anchor later: the gesture exists from the press, so a
  // pointerup that lands during the win_rect round trip can cancel it (wfEnd bumps
  // wfSeq), and moves that arrive during it are buffered, not lost.
  wfLog("b" + dir + "#" + seq);
  wfA = { dir, sx: ev.screenX, sy: ev.screenY, r: null, moved: false, seq, el, pid: ev.pointerId };
  try { el.setPointerCapture(ev.pointerId); } catch (e) { /* capture is an optimisation, not the mechanism */ }
  let r; try { r = await inv("win_rect"); } catch (e) { if (wfA && wfA.seq === seq) wfA = null; return noteErr(e); }
  if (!wfA || wfA.seq !== seq || wfSeq !== seq) { wfLog("x" + dir + "#" + seq); return; }   // released or superseded while we waited
  wfA.r = r;
  if (wfPend && !wfRaf) wfRaf = requestAnimationFrame(wfFlush);   // the buffered delta lands now
}
function wfDrag(ev) {
  const a = wfA;
  if (!a) return;
  /* A LOST POINTERUP IS THE SECOND WAY A GESTURE BECOMES A ZOMBIE, and it is the one
     that poisoned six of eight resize directions after wfSeq fixed the first. The
     window is moving under the cursor, so a release can land where the DOM never sees
     it (outside the window the resize has not caught up with yet, on the frame of a
     window that just shrank away from the pointer). The gesture then stays armed AND
     the grip keeps pointer capture — so the NEXT press is retargeted to the OLD grip,
     no new gesture begins, and the dead gesture keeps resizing with its own direction
     and press point: the west drag reported `[wfg:e:...+-1158,0]` and the window walked
     to its 320px floor. `buttons === 0` on a move means the button is up whatever we
     were told, so end the gesture there — this is the only signal that survives a
     dropped release. */
  if (ev.buttons === 0) { wfLog("z"); return wfEnd(); }
  const dx = ev.screenX - a.sx, dy = ev.screenY - a.sy;
  if (!a.moved && Math.abs(dx) + Math.abs(dy) < 3) return;   // a click is not a drag
  a.moved = true;
  wfPend = { dx, dy };
  if (!wfRaf) wfRaf = requestAnimationFrame(wfFlush);
}
function wfEnd() {
  if (wfA) wfLog("e#" + wfA.seq);
  if (wfPend) wfFlush();      // the last delta decides the final geometry
  const a = wfA;
  // hand the capture back, or the next press is retargeted to this grip and the
  // control it actually landed on never hears about it
  if (a && a.el && a.el.hasPointerCapture && a.el.hasPointerCapture(a.pid))
    try { a.el.releasePointerCapture(a.pid); } catch (e) { /* already gone */ }
  wfA = null; wfPend = null;
  wfSeq++;                    // invalidate any anchor still in flight (see wfSeq)
}
function wfArm() {
  // spelled out, one call per control: the cargo test greps for inv("win_...")
  // in this file, which is the only thing that notices when a command is renamed
  // in Rust and the button silently becomes a no-op (ui/ is not a cargo input)
  $("wf-min").onclick = () => inv("win_minimize").catch(noteErr);
  $("wf-close").onclick = () => inv("win_close").catch(noteErr);
  $("wf-max").onclick = wfToggleMax;
  for (const g of document.querySelectorAll("#wrz i"))
    g.addEventListener("pointerdown", ev => wfBegin(g.dataset.d, ev, g));
  // capture phase: the press must be claimed before a tab bar handler sees it,
  // and ONLY when it landed on the background (a tab is its own target)
  addEventListener("pointerdown", ev => { if (wfDragRegion(ev.target)) wfBegin("move", ev, ev.target); }, true);
  addEventListener("pointermove", wfDrag, true);
  addEventListener("pointerup", wfEnd, true);
  addEventListener("pointercancel", wfEnd, true);
  // the census must be able to say WHICH control the keyboard is on (R33.4)
  for (const b of document.querySelectorAll("#wframe button")) {
    b.addEventListener("focus", updateTitle);
    b.addEventListener("blur", updateTitle);
  }
  /* R33.9 KEYBOARD — the controls are real <button>s, so Enter and Space already
     activate them; the missing half is REACHING them. Tab is not the answer: focus
     lives in the editor, where Tab is a text edit, and a user who has to tab through
     the whole app to close a window has a mouse-only window with extra steps. So the
     strip gets the classic window-menu chord, Alt+Space, handled in the CAPTURE phase
     so it works from inside the editor and from inside a modal — a window you cannot
     close while a modal is open is a broken interaction. Left/Right rove between the
     three controls, Escape hands focus back to wherever it came from. Handled keys
     are stopped so the R14 keymap cannot also fire on them. */
  addEventListener("keydown", ev => {
    const bs = Array.from(document.querySelectorAll("#wframe button"));
    if (!bs.length) return;
    const at = bs.indexOf(document.activeElement);
    const take = () => { ev.preventDefault(); ev.stopPropagation(); };
    /* TWO chords, and the second one is not belt-and-braces: it is the only one that
       works on a real desktop. Alt+Space is the classic window-menu chord, which is
       exactly why window managers GRAB it — openbox 3.6's stock rc.xml binds
       `A-space` to its client-menu (/etc/xdg/openbox/rc.xml:245), so the key never
       reaches the app when a WM is running, and the smoke phase measured precisely
       that ("Alt+Space did not move keyboard focus onto the frame strip", under
       openbox, while it works on a WM-less display). F10 is the GTK/GNOME menubar
       convention, no WM grabs it, and it is what makes R33.9 true for a user rather
       than only for our headless rig. Divergence recorded in R33.9. */
    const wfChord = (ev.altKey && !ev.ctrlKey && !ev.metaKey && (ev.key === " " || ev.key === "Spacebar")) ||
                    (ev.key === "F10" && !ev.ctrlKey && !ev.metaKey && !ev.shiftKey);
    if (wfChord) {
      take();
      if (at < 0) wfRet = document.activeElement;   // remember, so Escape can give it back
      bs[0].focus();
      return;
    }
    if (at < 0) return;                 // focus is not on the strip: nothing below applies
    if (ev.key === "ArrowRight" || ev.key === "ArrowLeft") {
      take();
      bs[(at + (ev.key === "ArrowRight" ? 1 : bs.length - 1)) % bs.length].focus();
      return;
    }
    if (ev.key === "Escape") {
      take();
      bs[at].blur();
      if (wfRet && typeof wfRet.focus === "function") { try { wfRet.focus(); } catch (e) { /* the node may be gone */ } }
      wfRet = null;
    }
  }, true);
  wfArmed = true;
  /* wfArm() runs at top-level, BEFORE a vault is loaded, so `state` can still be
     null — and updateTitle() reads state.root unguarded. Calling it here threw
     "null is not an object (evaluating 'state.root')" into window.onerror, which
     noteErr latched as [jserr:] for the whole session: a first-error-wins channel
     poisoned before the app even booted, hiding the NEXT real error from every
     smoke phase. The census is republished by the first real updateTitle anyway. */
  if (typeof state !== "undefined" && state) updateTitle();
}
/* census: [wf:<buttons><d|-><grips>] [wfm:<maximised>] [wfk:<focused control>].
   `d` = the gesture listeners are armed; without them the window cannot be
   moved, and the smoke's move assertion is the thing that notices. */
function wfTok() {
  const b = document.querySelectorAll("#wframe button").length;
  const g = document.querySelectorAll("#wrz i").length;
  const a = document.activeElement;
  const k = a && a.id && a.id.indexOf("wf-") === 0 ? a.id : "-";
  return " [wf:" + b + (wfArmed ? "d" : "-") + g + "] [wfm:" + (wfMax ? 1 : 0) + "] [wfk:" + k + "] [wfg:" + wfLast + "] [wfl:" + wfLogA.join(">") + "] [hdr:" + wfHdrTok() + "]";
}
wfArm();
