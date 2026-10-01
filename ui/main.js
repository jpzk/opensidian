// opensidian, an Obsidian-compatible markdown notes app.
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
/* W6: actN counts user actions still between begin and their PAINT. The layout
   writer (wsFlush) yields while it is non-zero, so a note_open span can never
   contain the workspace serialisation or its IPC. The count is released only
   after the paint promise settles, which is exactly where the span ends. */
let actN = 0;
async function act(name, attrs, fn) {
  const sp = otel.begin(name, attrs);
  actN++;
  try { return await fn(sp); }
  finally { Promise.resolve(otel.paint(sp)).finally(() => { actN--; }); }
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
    if (act) await loadActive(h);   // loadActive -> lgFollow -> rgFollow refreshes the panes
    else { renderTabs(h); if (rLeaf && rLeaf.t === t) rgFollow(); }   // lgpanes: an unfocused rLeaf moved
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
   (palette 'Open local graph', Ctrl+Shift+G, ribbon).
   lgpanes (recon docs/recon-lgpanes, stock 1.13.7): the panes render the
   note of the most recently focused NOTE LEAF (rLeaf, a tab object, so it
   follows that leaf when it navigates) — not the graph's t.center:
   - a note tab focused          -> that note (and it becomes rLeaf);
   - a LOCAL graph tab focused   -> rLeaf's current note, null if none yet
     (shots 04, 17: stock keeps the last note; a node click navigates the
     linked leaf, so the panes then show the clicked note, shot 08);
   - GLOBAL graph / other views  -> null, the panes empty (shot 16, Q4).
   rTrack() runs from updateTitle too, so rLeaf is current while the right
   sidebar is closed. Census [rpnote:<note>|-] = the note last rendered. */
let rLeaf = null, rpNote = "-";
function rTrack() {
  if (!state) return null;
  const g = fg(), t = g && g.active >= 0 ? g.tabs[g.active] : null;
  if (t && !t.kind) rLeaf = { g, t };
  if (rLeaf && !(groups().includes(rLeaf.g) && rLeaf.g.tabs.includes(rLeaf.t))) rLeaf = null;  // leaf closed
  return t;
}
function rNote() {
  const t = rTrack();
  if (t && !t.kind) return t.name;
  if (t && t.kind === "lg" && rLeaf) return rLeaf.t.name;
  return null;
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
   Active tab persisted as rside_tab in ~/.opensidian.json; census
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
  rpNote = n || "-";                       // census [rpnote:] — the note the panes render
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
/* tocjump: the outline belongs to rLeaf (the note leaf rNote() rendered it for),
   NOT to the focused group — with the linked local graph focused, fg() is the
   graph tab (t.kind) and the old fg()-based lookup returned without jumping.
   tocLeaf() = that note leaf while it is its group's active tab, else null. */
function tocLeaf() {
  rTrack();
  if (!rLeaf) return null;
  const { g, t } = rLeaf;
  return g.tabs[g.active] === t && !t.kind ? { g, t } : null;
}
async function tocGo(line) {               // scroll + focus the heading at `line`
  const L = tocLeaf();
  if (!L) return;
  const { g, t } = L;
  if (fg() !== g) focusGroup(g);           // stock 1.13.7: the active leaf moves to the note (docs/tocjump/stock/OBSERVED.md 06,16)
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
  const L = tocLeaf();                     // tocjump: the outline's own note, whatever is focused
  if (!L) return;
  const { g, t } = L;
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
/* tocjump census [tj:<note>|top:<l>|car:<l>|st:<px>|ae:<where>] — the outline's
   OWN note leaf (rLeaf, the leaf rNote() renders), read whatever group is
   focused: top = first source line visible in that leaf's view (lp) or the line
   of the heading at its top (reading), car = its lp caret line (-1 none),
   st = scrollTop, ae = where DOM focus is (lp|rv = that leaf's view, g<i>:<kind>
   = another pane's group i, side|rside|body|<tag>). [tj:<note>|bg] = the leaf
   is a background tab. Only while the outline tab is showing. */
function tjTok() {
  if (!rightOpen || rTab !== "toc" || !rLeaf) return "";
  const { g, t } = rLeaf, nm = String(t.name).replace(/[[\]|]/g, "");
  if (g.tabs[g.active] !== t) return " [tj:" + nm + "|bg]";
  let top = 0, st = 0;
  if (isLp(t.mode)) {
    st = g.lp.scrollTop; let i = 0;
    for (const r of g.lp.children) { if (r.offsetTop - g.lp.offsetTop <= st + 2) top = i; i++; }
  } else {
    st = g.preview.scrollTop; let k = -1;
    g.preview.querySelectorAll("h1,h2,h3,h4,h5,h6").forEach((el, i) => { if (el.offsetTop - g.preview.offsetTop <= st + 2) k = i; });
    top = k >= 0 && tocHeads[k] ? tocHeads[k].line : 0;
  }
  const car = g.lpActive ? g.lpActive.l0 : -1, a = document.activeElement;
  let ae = a ? a.tagName.toLowerCase() : "-";
  if (a && (a === g.lp || g.lp.contains(a))) ae = "lp";
  else if (a && g.preview.contains(a)) ae = "rv";
  else if (a && a.closest) {
    const p = a.closest(".pane"), h = p && p._g;
    if (h) { const x = h.tabs[h.active]; ae = "g" + (groups().indexOf(h) + 1) + ":" + (x ? x.kind || "note" : "-"); }
    else if (a.closest("#side")) ae = "side";
    else if (a.closest("#rside")) ae = "rside";
  }
  return " [tj:" + nm + "|top:" + top + "|car:" + car + "|st:" + Math.round(st) + "|ae:" + ae + "]";
}
/* tocjump geometry [tjxy:toc=<line>@x,y;...|p<i>=tabx,taby/cx,cy;...|tree=x,y|-]
   — PAINTED points the phase clicks (no literal x,y in a phase): each visible
   outline row's text centre by source line; per pane (census order) a point in the left
   third of its ACTIVE tab (a narrow tab's centre is its close button) and a point 14px inside the bottom-left of its content;
   a blank point in the file explorer: below its last row, else the header's
   reserved blank slot (- if neither is painted). */
// census refresh for [tj:ae]: a click on a non-focusable surface (the explorer) blurs
// the editor without any handler that repaints the title — the token would go stale.
document.addEventListener("focusout", () => { if (rightOpen && rTab === "toc") setTimeout(updateTitle, 0); }, true);
function tjxyTok() {
  if (!rightOpen || rTab !== "toc") return "";
  const c = r => Math.round(r.left + r.width / 2) + "," + Math.round(r.top + r.height / 2);
  const rows = [...document.querySelectorAll("#toclist .tocrow")].filter(r => r.getClientRects().length)
    .map(r => r.dataset.line + "@" + c(r.querySelector(".tn").getBoundingClientRect()));
  const ps = [...document.querySelectorAll("#main .pane")].map((p, i) => {
    const a = p.querySelector(".tabs .tab.active"), r = p._g && p._g.content ? p._g.content.getBoundingClientRect() : null;
    const ar = a ? a.getBoundingClientRect() : null;   // tab point = left third: the centre of a narrow tab is its close button
    return "p" + (i + 1) + "=" + (ar ? Math.round(ar.left + Math.min(18, ar.width / 3)) + "," + Math.round(ar.top + ar.height / 2) : "-") + "/" + (r ? Math.round(r.left + 14) + "," + Math.round(r.bottom - 14) : "-");
  });
  const tr = $("tree"), T = tr.getBoundingClientRect();
  let lb = T.top;
  for (const e of tr.querySelectorAll("*")) { const r = e.getBoundingClientRect(); if (r.height && r.bottom > lb) lb = r.bottom; }
  const hs = document.querySelector("#pane-files .hslot"), H = hs ? hs.getBoundingClientRect() : null;   // a full tree has no blank: the explorer header's reserved blank slot, a no-op click inside the pane
  const tree = T.bottom - lb > 24 ? Math.round(T.left + T.width / 2) + "," + Math.round((lb + T.bottom) / 2)
    : H && H.width ? c(H) : "-";
  return " [tjxy:toc=" + rows.join(";") + "|" + ps.join(";") + "|tree=" + tree + "]";
}
/* ux-4: left sidebar drag-resize (clamped 150-600, ribbon is 44px);
   width persisted as sidebar_w in ~/.opensidian.json on mouseup */
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
  if (revealInfo) {                        // bmmenu: the reveal ring is a pointer, not a selection — a pane switch drops it
    revealInfo = "";
    document.querySelectorAll("#tree .trow.revealed").forEach(r => r.classList.remove("revealed"));
  }
  for (const [k, id] of Object.entries(SPANES)) {
    $(id).hidden = k !== p;
    $("stab-" + k).classList.toggle("active", k === p);
  }
  if (p === "search") $("sinput").focus();
  if (p === "bm") refreshBm();               // re-read: disk is the truth
  updateTitle();
}

/* ============ R25.13a..m — CLICKING A SEARCH RESULT ============
   Every rule below was MEASURED against stock 1.13.7 first; the measurements,
   with a shot each, are docs/recon-srclick/README.md (C1-C12) and the rules
   they produced are R25.13a-m in docs/requirements.md. Nothing here is a guess:
   what stock does that we do not yet do is listed LATER in that README, not
   approximated.

   The one non-obvious fact, and the reason the payload grew an `offset`
   (src-tauri/src/main.rs, search_hits_carry_absolute_utf16_offsets): stock
   records a hit as an ABSOLUTE CHARACTER OFFSET into the file as indexed, not
   as a line number and not as a string to re-find. Delete five lines above a
   match and stock jumps to the same offset, which is now other text (C12,
   shots 58-61). So the frontend NEVER re-searches the buffer — a second search
   that can disagree with the first is exactly the bug the brief forbids — it
   maps the backend's offset into the current text and lands wherever it lands.

   The decoration (R25.13d) is stored as offsets on the VIEW, not as a class in
   the DOM and not behind a timer (R25.13f: it survives blur, arrow keys,
   typing, undo and tab switches, measured in shots 19-25). The view is the
   per-tab object, so "the editor instance going away" — the tab takes another
   file, the tab is closed — drops it exactly as measured, with no bookkeeping. */
const SC_MARK = "is-flashing";
let scInfo = "";                           // census [sc:...] — the last jump, as it happened
function scView(g) { return g && g.view ? g.view : null; }
function scHits(g) {                       // the live decoration set, or [] once the view holds another note
  const v = scView(g);
  return v && v.scHl && v.scNote === curOf(g) ? v.scHl : [];
}
// the cheap guard editor.js asks on every keystroke before paying for offOf
function scLive(g) { const v = scView(g); return !!(v && v.scHl && v.scHl.length); }
function scSet(g, list) {                  // R25.13e: the next result click REPLACES the set, never accumulates
  const v = scView(g);
  if (!v) return;
  v.scHl = (list || []).filter(h => h && h.len > 0);
  v.scNote = curOf(g);
  scMarks(g);
}
function scClear(g) {                      // R25.13f: a pointer click in that editor, or the next result click
  const v = scView(g);
  if (!v || !(v.scHl && v.scHl.length)) return;
  v.scHl = [];
  scMarks(g);
  updateTitle();
}
/* R25.13d "a range that follows subsequent edits": the offsets are mapped
   through the splice at Ed.replace — the single edit choke point — the way a
   cm6 range is mapped, so typing above the match moves it and typing inside it
   grows it. Called from editor.js by name, the same way it calls updateTitle. */
function scShift(g, from, to, ins) {
  const v = scView(g);
  if (!v || !(v.scHl && v.scHl.length)) return;
  const d = ins - (to - from);
  const map = (p, start) => (p <= from ? p : p >= to ? p + d : start ? from : from + ins);
  v.scHl = v.scHl.map(h => {
    const a = map(h.off, true), b = map(h.off + h.len, false);
    return { off: a, len: Math.max(0, b - a) };
  }).filter(h => h.len > 0);
}
/* absolute UTF-16 offset -> {l,c}, CLAMPED to the document end (R25.13m: an
   offset past the end lands at the end, with no highlight and no crash). */
function scLC(g, off) {
  const L = Ed.lines(g);
  let o = Math.max(0, off);
  for (let l = 0; l < L.length; l++) {
    if (o <= L[l].length) return { l, c: o };
    o -= L[l].length + 1;                  // + the newline this line ends with
  }
  const last = Math.max(0, L.length - 1);
  return { l: last, c: (L[last] || "").length };
}
/* R25.13f/m: this runs on EVERY keystroke while a decoration is live (from
   lpRender and from scheduleSave), and what it does is DOM SURGERY ON THE ROW
   THE CARET IS IN: the unwrap below calls p.normalize(), which MERGES the text
   nodes the DOM Selection is anchored in, and a merged anchor is a moved
   caret. Measured on the box (12:54 and 13:2x): typing `ZCDIRTY` at the start
   of the line-42 match put `Z` at column 0 and the remaining six characters at
   column 10 — the END of the match — because the first keystroke's scMarks
   relocated the caret and every later one landed where it had been left. The
   model position is the truth, so read it BEFORE the surgery and put it back
   after. Ed.sel() returns null when the caret is not in this pane's lp, so a
   blurred editor (R25.13f's query-input case) is left exactly as it was. */
function scMarks(g) {
  const s0 = g && g.lp ? Ed.sel(g) : null;
  scPaint(g);
  if (!s0) return;
  if (s0.empty) Ed.place(g, s0.b.l, s0.b.c, false);
  else { Ed.place(g, s0.a.l, s0.a.c, false); Ed.extendTo(g, s0.b.l, s0.b.c); }
}
/* paint the set with the SAME per-text-node right-to-left walk the find bar
   uses (fWrap): a match that straddles a rendered <strong> becomes two spans,
   and no offset is invalidated mid-walk. Only the lp surface is painted —
   R25.13k measured that stock paints NO highlight in the reading renderer. */
function scPaint(g) {
  for (const sc of [g.lp, g.preview]) {
    if (!sc) continue;
    for (const m of [...sc.querySelectorAll("span." + SC_MARK)]) {
      const p = m.parentNode;
      if (!p) continue;
      while (m.firstChild) p.insertBefore(m.firstChild, m);
      p.removeChild(m);
      p.normalize();
    }
  }
  const hs = scHits(g);
  if (!hs.length || !g.lp) return;
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind || !isLp(t.mode)) return;
  const byRow = new Map();
  hs.forEach((h, i) => {
    const a = scLC(g, h.off), b = scLC(g, h.off + h.len);
    for (let l = a.l; l <= b.l; l++) {
      const c0 = l === a.l ? a.c : 0;
      const c1 = l === b.l ? b.c : (Ed.lines(g)[l] || "").length;
      if (c1 <= c0) continue;
      if (!byRow.has(l)) byRow.set(l, []);
      byRow.get(l).push({ c: c0, end: c1, i });
    }
  });
  for (const [l, rs] of byRow) {
    const row = Ed.rowAt(g, l);
    if (!row) continue;
    const map = Ed.nodes(row), jobs = new Map();
    for (const h of rs) for (const s of map) {
      const a = Math.max(h.c, s.c), b = Math.min(h.end, s.c + s.len);
      if (b <= a) continue;
      if (!jobs.has(s.n)) jobs.set(s.n, []);
      jobs.get(s.n).push({ a: a - s.c, b: b - s.c, i: h.i, cur: false });
    }
    fWrap(jobs, SC_MARK);
  }
}
/* R25.13c: the match is CENTRED — measured at a 349 px offset in a 718 px
   viewport, independent of note length — and the scroll is a SINGLE-FRAME JUMP
   (a 60 Hz frame sampler saw exactly one scrollTop change, shots 15-18). The
   clamp is the scroller's own range: no overscroll is invented. */
function scCenter(sc, top, h) {
  if (!sc) return;
  sc.scrollTop = Math.max(0, Math.min(top - (sc.clientHeight - h) / 2,
                                      Math.max(0, sc.scrollHeight - sc.clientHeight)));
}
/* the jump itself. `hits` = the occurrences to decorate (one for a hit row,
   every occurrence in the note for a group header, R25.13b); hits[0] is the
   one jumped to. */
async function scJump(g, hits) {
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind || !hits.length) return;
  const h0 = hits[0], p = scLC(g, h0.offset);
  if (t.mode === "reading") {
    // R25.13k: a hit click NEVER changes the view mode. In reading mode stock
    // scrolls the PREVIEW renderer to centre the occurrence and does nothing
    // else — no highlight, no caret (shots 51-53). The preview's text offsets
    // are the rendered ones, so the occurrence is located with the find bar's
    // own reading-view segment map (fRSegs) rather than with a source offset.
    scSet(g, []);
    const el = scPreviewEl(g, h0);
    if (el) scCenter(g.preview, el.offsetTop - g.preview.offsetTop, el.offsetHeight);
    scInfo = "sc:" + curOf(g) + "@read|hl:0|top:" + Math.round(g.preview.scrollTop);
    updateTitle();
    return;
  }
  scSet(g, hits.map(h => ({ off: h.offset, len: h.len })));
  // R25.13l: ONE COLLAPSED caret at the first character of the match — the
  // match is never selected, and a selection that was live is replaced by it.
  await lpMove(g, p.l, p.c, "search");
  scMarks(g);                              // the caret move re-rendered the touched rows
  const row = g.lp.children[p.l];
  // The four numbers R25.13c's rule is MADE of, published beside the result it
  // produced: the smoke phase recomputes
  //   clamp(rowTop - (clientHeight - rowHeight)/2, 0, scrollHeight - clientHeight)
  // and compares it to the scrollTop that was actually set, so "the match is
  // CENTRED, clamped to the scroller's own range" is an arithmetic assertion
  // rather than a screenshot — and the two clamped ends (a match above the
  // first half-viewport, a match on the last line) are distinguishable from a
  // jump that simply did not scroll. Row heights differ per line, so they are
  // MEASURED here and never assumed by the phase.
  const rt = row ? row.offsetTop - g.lp.offsetTop : -1;
  const rh = row ? row.offsetHeight : -1;
  if (row) scCenter(g.lp, rt, rh);
  scInfo = "sc:" + curOf(g) + "@" + p.l + "." + p.c + "|hl:" + scHits(g).length +
           "|top:" + Math.round(g.lp.scrollTop) +
           "|vp:" + Math.round(g.lp.clientHeight) + "x" + Math.round(g.lp.scrollHeight) +
           "|row:" + Math.round(rt) + "x" + Math.round(rh);
  updateTitle();
}
/* the reading-view block holding a hit: the rendered text is not the source
   text, so the occurrence is found by its INDEX among the note's hits (the
   n-th match in document order), never by a second search of the buffer. */
function scPreviewEl(g, h) {
  if (!g.preview) return null;
  const { segs } = fRSegs(g.preview);
  let k = h.nth || 0;
  const q = (h.text || "").toLowerCase();
  if (!q) return g.preview.firstElementChild;
  for (const s of segs) {
    const low = (s.n.nodeValue || "").toLowerCase();
    let i = low.indexOf(q);
    while (i >= 0) {
      if (k === 0) {
        const e = s.n.parentElement;
        return e ? (e.closest("p,li,h1,h2,h3,h4,h5,h6,blockquote,pre,td") || e) : null;
      }
      k--;
      i = low.indexOf(q, i + 1);
    }
  }
  return null;
}
/* R25.13a: a hit click loads the file into the ACTIVE tab, replacing whatever
   it held. No tab is created and a tab that already holds that file elsewhere
   is NEITHER reused NOR focused (shots 04-10). R25.13i: Ctrl+click and MIDDLE
   click open a NEW tab in the active group and activate it; Shift+click and
   Alt+click are plain clicks; every variant performs the full jump.
   Ctrl+Alt+click (stock: a new split pane) is LATER — see the README. */
async function scOpen(g, note, ev) {
  const newTab = !!(ev && (ev.ctrlKey || ev.metaKey || ev.button === 1)) && !(ev && ev.altKey);
  if (newTab) {
    await flushSave(g);
    g.tabs.push(mkTab(note));
    g.active = g.tabs.length - 1;
    await loadActive(g);
    return;
  }
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (t && !t.kind && curOf(g) === note) { await flushSave(g); return; }  // already here: R25.13m still flushes
  await navigate(g, note);                 // replaces the ACTIVE tab, appends when there is none
}
/* the handler the rows and the group headers share. `hits` are this note's
   hits in document order; `one` = the clicked occurrence, or null for the
   group header, which jumps to the FIRST hit and decorates EVERY occurrence
   in that note (R25.13b, shots 11-13). */
async function scClick(ev, note, hits, one) {
  const g = fg();
  if (!g) return;
  const set = one ? [one] : hits;
  await scOpen(g, note, ev);
  await scJump(g, set);
}
/* the index of a hit among the hits of ITS note, in document order. The
   reading renderer has no source offsets, so this ordinal is the only honest
   way to point at the same occurrence there (R25.13k). */
function scNth(hits, h) {
  let k = 0;
  for (const x of hits) {
    if (x === h) return k;
    if (x.note === h.note && x.len > 0) k++;
  }
  return 0;
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
      // R25.13b: the note-name row opens the note AND jumps to its FIRST hit,
      // with EVERY occurrence in that note decorated.
      const nh = hits.filter(x => x.note === n)
                     .map(x => ({ offset: x.offset, len: x.len, nth: scNth(hits, x), text: q }));
      grp.onclick = ev => scClick(ev, n, nh, null);
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
    // R25.13a/d/e: the clicked occurrence, and ONLY it, is decorated. The row
    // carries the backend's absolute offset (R25.13m) — the frontend never
    // re-finds the text. `nth` is its index among this note's hits, which is
    // how the reading renderer locates the same occurrence (R25.13k).
    const one = { offset: h.offset, len: h.len, nth: scNth(hits, h), text: q };
    row.onclick = ev => scClick(ev, note, [one], one);
    // R25.13i: MIDDLE click opens a new tab. mousedown prevents the paste-on-
    // middle-click default; the open runs on auxclick, where button === 1.
    row.onmousedown = ev => { if (ev.button === 1) ev.preventDefault(); };
    row.onauxclick = ev => { if (ev.button === 1) { ev.preventDefault(); scClick(ev, note, [one], one); } };
    box.appendChild(row);
  }
  updateTitle();
  perf.mark("search", st0, { q, hits: hits.length });
  otel.paint(searchSp, { hits: hits.length }); searchSp = null;   // R18 search_type: keystroke -> results painted
}
/* [srg:<centre x>|Q<y>|G<y>|H<y>|H<y>…] — the PAINTED geometry of the search
   pane, in paint order: `Q` the query input, `G` a note-name group row, `H` a
   hit row, each y the element's centre in window coordinates. Same idea as
   [bmg:] for the bookmark rows and [mgy:] for menu rows, and for the same
   reason: the srclick phase clicks the row it MEASURED, never a y computed
   from a padding it read off a stylesheet. A row scrolled out of the #sresults
   viewport publishes y = -1 rather than a coordinate a click would miss, so
   "the row is reachable" is a census fact and not an assumption.
   ONE x for all of them: #sinput and the rows are the same full-width column
   of the pane, so the results box's centre is inside every one of them — and
   the phase needs the input's coordinate too, because "the decoration survives
   BLUR" (R25.13f) is a pointer click OUTSIDE that editor, which has to land
   somewhere that is not another result row.
   Emitted only while the search pane shows a query ([sr:] is showing). */
function srGeom() {
  const box = $("sresults");
  if (!box) return "";
  const rows = box.querySelectorAll(".sgroup, .shit");
  if (!rows.length) return "";
  const b = box.getBoundingClientRect();
  const parts = [];
  const qi = $("sinput");
  if (qi) {
    const a = qi.getBoundingClientRect();
    parts.push("Q" + Math.round(a.top + a.height / 2));
  }
  for (const r of rows) {
    const a = r.getBoundingClientRect();
    const cy = Math.round(a.top + a.height / 2);
    const vis = a.top >= b.top - 1 && a.bottom <= b.bottom + 1;
    parts.push((r.className === "sgroup" ? "G" : "H") + (vis ? cy : -1));
  }
  return " [srg:" + Math.round(b.left + b.width / 2) + "|" + parts.join("|") + "]";
}

/* R9.4 bookmarks: tree-row context menu toggles; rust persists the tree in
   vault/.obsidian/bookmarks.json (stock's own file, R4X.10). census [bm:N] while the pane shows —
   N counts the .bmrow nodes actually PAINTED in #bmlist, so an assertion on
   it fails if renderBm() stops repainting even while the model is correct.

   bmfolder (R4X.*, docs/bookmark-groups.md): the list is a TREE now. The pane
   paints `bookmark_rows()` — a PRE-ORDER vector of {kind,depth,name,label}, one
   entry per painted row, in the same order as the file on disk — so the UI never
   walks a tree and cannot invent an order the file does not have. Every
   structural command is addressed by the row's INDEX into that vector, never by
   title: two sibling groups may carry the same title (measured on stock,
   docs/recon-bmfolder/05-nest.png), so a title is not a key.
   bmCache stays the FLAT name list every other caller asks `includes()` of
   (tab menu, note menu, R9.6 rename) — derived from bmTree, never fetched. */
let bmCache = [];                         // the `f` rows' names, pre-order (== list_bookmarks)
let bmTree = [];                          // the PAINTED rows: [{kind:"f"|"g", depth, name, label}]
let bmRenaming = null;                    // row index whose label is an inline editor (stock's Rename, 07-nest-named.png)
/* collapseall R1/R6: the COLLAPSED bookmark groups, keyed by the group's title
   path from the top level ("Work\u001fInner"). IN MEMORY ONLY, like the explorer's
   `collapsed` Set: stock keeps its folds in Electron localStorage
   ("<vaultId>-bookmarks-folds"), never in bookmarks.json (recon-bmcollapse Q1-Q5),
   and R2 forbids a fold from writing that file — so nothing here calls inv().
   Persisting the folds is todo id:bmcollapsebuild (spec R6). A group not in the
   set paints EXPANDED (M5: an unrecorded group is expanded). */
let bmFolds = new Set();
let bmEdit = null;                        // the open Edit bookmark modal: {ix, name, opts}
let revealInfo = "";                      // bmmenu: [bmrv:<name>] after "Reveal file in navigation" (bmReveal), cleared by setPane
const BM_INDENT = 17;                     // px per depth level — MEASURED on stock (14-saved.png), icon and label both shift
const BM_PAD = 12;                        // .bmrow's own left padding (style.css), depth 0
const bmRows = () => document.querySelectorAll("#bmlist .bmrow").length;
/* collapseall: a row inside a collapsed group stays IN the DOM (class .bmhide,
   display:none) so row i is still bmTree[i] for the drag machine and [bm:]/[bmn:]/
   [bmt:] keep describing the whole rendered model ([bmdesync:] stays meaningful).
   What the user can SEE is [bmvis:<n>]; geometry tokens read visible rows only. */
const bmVisRows = () => Array.from(document.querySelectorAll("#bmlist .bmrow:not(.bmhide)"));
/* R20.6: the LABELS the user can actually read, taken from the painted rows in
   paint order. A count alone passes a renderBm() that paints the right NUMBER of
   wrong rows, so [bmn:] is what proves the pane tracks disk. '|' and ']' are
   stripped so a note named with a separator cannot forge a census token.
   Group rows are painted rows, so they count in [bm:] and name themselves in
   [bmn:] — R20.6/R20.7 keep their meanings exactly, there are simply more rows. */
const bmNames = () => Array.from(document.querySelectorAll("#bmlist .bmrow"))
  .map(r => r.textContent.replace(/[|\]]/g, "")).join("|");
/* R4X.5 [bmt:<kind><depth>|…] — the painted TREE SHAPE, one field per row in
   paint order, positionally parallel to [bmn:]: a driver reads shape and labels
   off the same index. Read off the DOM (the class and the row's own data-bmd),
   not off bmTree, for the reason [bm:] is: a model that is right while the pane
   paints something else is the bug this token exists to catch. */
const bmShape = () => Array.from(document.querySelectorAll("#bmlist .bmrow"))
  .map(r => (r.classList.contains("bmgrp") ? "g" : "f") + (r.dataset.bmd || "0")).join("|");
/* R4X.6 [bmi:<px>] — the painted INDENT STEP, measured between the SHALLOWEST
   and DEEPEST painted row's content (the icon, which stock shifts too) and
   divided by the depth difference. A class that is applied but paints no offset
   passes [bmt:] and fails this. Empty when fewer than two depths are painted. */
function bmIndentTok() {
  const k = bmVisRows();
  let lo = null, hi = null;
  for (const r of k) {
    const ic = r.querySelector(".bmic");
    if (!ic) continue;
    const e = { d: Number(r.dataset.bmd || 0), x: ic.getBoundingClientRect().left };
    if (lo === null || e.d < lo.d) lo = e;
    if (hi === null || e.d > hi.d) hi = e;
  }
  if (!lo || !hi || hi.d === lo.d) return " [bmi:]";
  return " [bmi:" + Math.round((hi.x - lo.x) / (hi.d - lo.d)) + "]";
}
/* R4X.8 [bmren:<text in the inline editor>] — Rename opens an editor IN the row
   and the model is untouched until Return (stock: the dump still says "Untitled
   group" while the box reads "Inner", 07-nest-named.png -> 08-nest-commit.png).
   Without this token a phase cannot tell "the editor is open" from "the rename
   already committed", which is exactly the difference stock draws. */
const bmRenTok = () => (bmRenaming === null ? "" :
  " [bmren:" + String(($("bmren") && $("bmren").value) || "").replace(/[|\]]/g, "") + "]");
function bmSync() {                        // the flat view every non-pane caller uses
  bmCache = bmTree.filter(r => r.kind === "f").map(r => r.name);
}
const BM_ICON_FILE = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M6 3h12v18l-6-4.5L6 21z"/></svg>';
// a group row carries a CHEVRON where a file row carries its bookmark glyph, at
// the same slot and the same depth (14-saved.png). It points DOWN when expanded;
// .bmfold on the row rotates it to point RIGHT (collapseall R1, style.css).
const BM_ICON_GROUP = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M6 9l6 6 6-6"/></svg>';
function renderBm() {
  const box = $("bmlist");
  box.textContent = "";
  box.oncontextmenu = e => { if (e.target === box) bmPaneMenu(e); };   // the pane BACKGROUND: stock's one-item "New group" (02-emptymenu.png)
  if (!bmTree.length) {
    const d = document.createElement("div");
    d.className = "sempty"; d.textContent = "No bookmarks.";
    d.oncontextmenu = e => bmEmptyMenu(e);   // bmmenu: stock's one-item "New group" menu, disabled on THIS line — see bmEmptyMenu
    box.appendChild(d);
  }
  const anc = [];                          // collapseall: the open ancestor chain {depth, key, folded}
  bmTree.forEach((r, ix) => {              // pre-order == file order == paint order
    const row = document.createElement("div");
    const grp = r.kind === "g";
    while (anc.length && anc[anc.length - 1].depth >= r.depth) anc.pop();
    const hide = anc.some(a => a.folded);  // inside ANY collapsed ancestor: in the DOM, not painted
    const key = grp ? anc.map(a => a.name).concat(r.name).join("\u001f") : null;
    const folded = grp && bmFolds.has(key);
    if (grp) anc.push({ depth: r.depth, name: r.name, folded });
    row.className = "bmrow" + (grp ? " bmgrp" : "") + (folded ? " bmfold" : "") + (hide ? " bmhide" : "");
    if (grp) row.dataset.bmk = key;
    row.dataset.bmd = String(r.depth);
    row.style.paddingLeft = (BM_PAD + r.depth * BM_INDENT) + "px";   // content shifts, background still spans the pane (14-saved.png)
    row.title = r.name;
    const s = document.createElement("span");
    s.className = "bmic " + (grp ? "bmchev" : "bmstar");             // svg, not ★/▾ — headless fonts lack the glyphs
    s.innerHTML = grp ? BM_ICON_GROUP : BM_ICON_FILE;
    row.appendChild(s);
    if (grp && ix === bmRenaming) {        // Rename: an editor INSIDE the row's ring, model untouched until Return
      const inp = document.createElement("input");
      inp.id = "bmren"; inp.className = "bmren"; inp.value = r.name;
      inp.spellcheck = false; inp.autocomplete = "off";
      inp.onclick = e => e.stopPropagation();
      inp.oninput = () => updateTitle();
      inp.onblur = () => { if (bmRenaming === ix) { bmRenaming = null; renderBm(); } };   // blur is not a commit: only Return is (08-nest-commit.png)
      inp.onkeydown = e => {
        e.stopPropagation();               // a group title may contain any chord the global keymap owns
        if (e.key === "Enter") { e.preventDefault(); bmGroupRenameCommit(ix, inp.value); }
        else if (e.key === "Escape") { e.preventDefault(); bmRenaming = null; renderBm(); }
      };
      row.appendChild(inp);
    } else {
      // R4X.17: the backend computes the painted label by stock's measured rule
      // (title when typed, else basename — recon-bmcompat 30-afterinject.png);
      // r.name stays the full extensionless name the click opens by.
      row.append(r.label ?? (grp ? r.name : r.name.split("/").pop()));
    }
    // a GROUP row opens nothing — a click on its chevron, label or row background
    // TOGGLES its fold (collapseall R1, M5 / recon-bmcollapse). A FILE row opens.
    row.onclick = grp ? () => { if (bmClickEaten()) return; bmFoldToggle(key); }
                      : e => { if (bmClickEaten()) return; rowOpen(r.name, e); };   // bmdrag: a drop's click opens nothing; opentab REQ-2/3/5/7
    if (!grp) row.onauxclick = e => { if (e.button === 1) { e.preventDefault(); rowOpen(r.name, e); } };   // opentab REQ-6
    row.oncontextmenu = e => (grp ? bmGroupMenu(e, ix, r.name) : bmRowMenu(e, r.name, ix));
    row.addEventListener("mousedown", e => bmDragStart(e, ix));   // bmdrag: every row drags, groups included (recon case 5)
    box.appendChild(row);
  });
  if (bmRenaming !== null) { const i = $("bmren"); if (i) { i.focus(); i.select(); } }
  updateTitle();
}
/* sidefont: [sfont:t<px>b<px>r<px>] — the COMPUTED font-size of the first
   explorer .trow, bookmark .bmrow and right-strip .rlist row in the DOM ("-"
   when that kind has no row rendered). Read off getComputedStyle, not off the CSS
   text, so a theme or a stray rule that overrides --sidebar-font-size shows up
   here. The gate phase `sfont` asserts t==b==13 (stock Q1) and r==t. */
function sfontTok() {
  const fs = sel => {            // first row IN THE DOM: Files and Bookmarks share one slot, so
    const el = document.querySelector(sel);   // one of the two is always [hidden] — computed style still resolves there
    return el ? Math.round(parseFloat(getComputedStyle(el).fontSize)) : "-";
  };
  return " [sfont:t" + fs("#tree .trow") + "b" + fs("#bmlist .bmrow") + "r" + fs(".rlist > :not(.rempty)") + "]";
}
/* collapseall R1/R2: flip ONE group's fold. Pure view state — no inv(), so the
   bookmarks.json bytes cannot move (the phase sha256s the file around it). */
function bmFoldToggle(key) {
  if (key == null || bmRenaming !== null) return;
  return act("bm_fold", { group: key.split("\u001f").join("/"), open: bmFolds.has(key) }, () => {
    bmFolds.has(key) ? bmFolds.delete(key) : bmFolds.add(key);
    renderBm();
  });
}
/* [bmg:<row centre x>,<first row centre y>,<row pitch>] — the PAINTED geometry of the
   bookmark rows, so a driver right-clicks a row it measured, not a y it guessed
   (the same idea as [mg:] for menus). Emitted only while the pane shows rows. */
function bmGeom() {
  const k = bmVisRows();                   // a folded row paints nothing: it has no centre to click
  if (!k.length) return "";
  const a = k[0].getBoundingClientRect();
  const pitch = k.length > 1 ? k[1].getBoundingClientRect().top - a.top : a.height;
  return " [bmg:" + Math.round(a.left + a.width / 2) + "," + Math.round(a.top + a.height / 2) + "," + Math.round(pitch) + "]";
}
/* ---------- bmmenu: the bookmark-row context menu, stock 1.13.7's list ----------
   THE SPEC is docs/recon-bmmenu/README.md — seven items, three separators, measured
   on the box against /srv/reference/obsidian.AppImage (sha256 e0d8e0a6…72663). Not
   one label below is from memory; the README's WIRED / NOT WIRED section names the
   function behind each row. Rows stock has that this tree has no backing for are
   shown DISABLED the way the settings rows are (R30 note below SDIS_TITLE: .dis +
   aria-disabled + pointer-events:none, a stated reason in the hover title) — stock
   itself has no disabled style to copy, and inventing a second one would be worse.
   Separators are real children (div.sep) so the census reads the list EXACTLY as
   the README writes it: [menu:…|Open in new window|---|Rename|…]. */
function bmMenuItems(m) {
  const item = (label, fn, why) => {       // why != null -> disabled, with the reason as the tooltip
    const d = document.createElement("div");
    d.textContent = label;
    if (why != null) { d.className = "dis"; d.setAttribute("aria-disabled", "true"); d.title = why; }
    else { d.onmousedown = ev => ev.stopPropagation(); d.onclick = () => { closeMenu(); fn(); }; }
    m.appendChild(d);
  };
  const sep = () => { const d = document.createElement("div"); d.className = "sep"; m.appendChild(d); };
  return { item, sep };
}
function bmRowMenu(e, nm, ix) {             // right-click a FILE .bmrow -> stock's file-bookmark menu
  e.preventDefault();
  e.stopPropagation();
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  const { item, sep } = bmMenuItems(m);
  /* Edit... is the route that MOVES a bookmark between groups (recon: 11-edit.png
     -> 14-saved.png; it is not a "Move to group" item). It stays DISABLED while
     the pane holds no group: the modal's only wired field is the group chooser,
     and a chooser whose every option is "(top level)" is a dialog that cannot
     change anything. That is also why phase_bmmenu — which runs on a FLAT list
     and asserts [mdis:3|5|6] — keeps passing unedited (criterion 8). */
  const hasGrp = bmTree.some(r => r.kind === "g");
  item("Open in new tab",   () => openNewTab(nm));                         // WIRED: openNewTab — right of the active tab, focused, never deduped (opentab REQ-8/9/14, stock c20)
  item("Open to the right", () => splitWith(fg(), "row", mkTab(nm)));      // WIRED: splitWith — the verb behind the tab menu's "Split right" (M7/R6.2), carrying a fresh tab of this note
  item("Open in new window", null, "Single-window app: there is no second window to open into");   // NOT WIRED
  sep();
  item("Rename",  null, "Bookmarks are note names on disk (.obsidian/bookmarks.json); there is no per-bookmark title to rename");   // NOT WIRED
  item("Edit...", hasGrp ? () => openBmEdit(ix) : null,                    // WIRED (R4X.7) once a group exists: the move route, in and out
       hasGrp ? null : "Edit bookmark chooses a GROUP and this pane has none — right-click the pane background to create one");
  sep();
  item("Reveal file in navigation", () => bmReveal(nm));                   // WIRED: bmReveal — setPane("files") + the explorer row's focus ring (R9.3 pane switch, treeRows)
  sep();
  item("Remove", () => { if (bmCache.includes(nm)) toggleBm(nm); });       // WIRED: toggleBm — the same toggle the explorer-row / tab menus use (R9.4 / R20.4); guarded so it can only REMOVE
  placeMenu(m, e.clientX, e.clientY);      // R22: viewport-clamped by measured size — the same seam as noteMenu/tabMenu
}
/* ---------- bmfolder: the GROUP-row menu, stock 1.13.7's list ----------
   SEVEN items, TWO separators, verbatim and in order off the pixels of
   docs/recon-bmfolder/04-groupmenu.png (re-measured on 23-del-menu.png for a
   group that HAS children: the same seven, no confirmation item). It differs
   from the file-row menu exactly as the recon says: no Edit..., no Reveal file
   in navigation, and it gains "Bookmark the active tab..." and "New group". */
function bmGroupMenu(e, ix, title) {
  e.preventDefault();
  e.stopPropagation();
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  const { item, sep } = bmMenuItems(m);
  const OPEN_WHY = "A group holds NAMES, not a note (R9.4) — there is nothing behind this row to open; stock's own behaviour here is UNMEASURED (docs/recon-bmfolder/README.md)";
  item("Open in new tab",    null, OPEN_WHY);                              // NOT WIRED
  item("Open to the right",  null, OPEN_WHY);                              // NOT WIRED
  item("Open in new window", null, "Single-window app: there is no second window to open into");   // NOT WIRED
  sep();
  item("Rename", () => bmGroupRenameStart(ix));                            // WIRED: inline editor in the row (07-nest-named.png -> 08-nest-commit.png)
  item("Bookmark the active tab...", () => openBmAdd(ix));                // WIRED (bmactive): stock's "Add bookmark" modal, docs/recon-bmactive F1/F2
  item("New group", () => bmGroupNew(ix));                                 // WIRED: nests INSIDE this group (04-groupmenu.png -> 05-nest.png)
  sep();
  item("Remove", () => bmGroupDelete(ix));                                 // WIRED: takes the SUBTREE, no confirmation (23-del-menu.png -> 24-deleted.png)
  // [mt:bmgroup:<title>] — WHICH row the browser's hit test handed us (R20.8's
  // lesson): "Remove" on a group drops its whole subtree, so an OCR/geometry
  // miss followed by Remove would delete a group nobody named and still look green.
  m.dataset.mt = "bmgroup:" + String(title).replace(/[|\]]/g, "");
  placeMenu(m, e.clientX, e.clientY);
}
function bmEmptyMenu(e) {                  // right-click the "No bookmarks." LINE -> stock's one-item menu
  e.preventDefault();
  e.stopPropagation();
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  /* DELIBERATE DELTA, and the doc says so: stock enables New group here
     (02-emptymenu.png). phase_bmmenu asserts [menu:New group] [mdis:1] on this
     exact line and criterion 8 forbids editing it, so THIS line keeps its
     committed answer. The affordance is not lost: the same right-click anywhere
     else in the pane background creates a group (bmPaneMenu), empty list or not. */
  bmMenuItems(m).item("New group", null, "Not from the empty-state line — right-click the pane background below it to create a group");
  placeMenu(m, e.clientX, e.clientY);
}
function bmPaneMenu(e) {                   // right-click the pane BACKGROUND -> stock's one-item "New group" (02-emptymenu.png)
  e.preventDefault();
  e.stopPropagation();
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  bmMenuItems(m).item("New group", () => bmGroupNew(null));                // WIRED: appends at the END of the top level, titled "Untitled group" (03-newgroup.png)
  m.dataset.mt = "bmpane:";
  placeMenu(m, e.clientX, e.clientY);
}
/* Reveal file in navigation: stock switches the left sidebar to Files and puts the
   focus ring on that note's row — no tab opens, the editor is untouched (recon
   10-reveal.png). The ring is .revealed on the explorer row (one at a time); the
   census publishes [bmrv:<name>] so a driver asserts the HANDLER ran, not merely
   that the Files pane is showing (it was showing before the click too). Cleared
   by the next pane switch (setPane) — the ring is a pointer, not a selection. */

function bmReveal(nm) {
  setPane("files");
  document.querySelectorAll("#tree .trow.revealed").forEach(r => r.classList.remove("revealed"));
  const r = treeRows.get(nm);
  if (r) { r.classList.add("revealed"); r.scrollIntoView({ block: "nearest" }); }
  revealInfo = r ? nm.replace(/[|\]:]/g, "") : "";
  updateTitle();
}
async function refreshBm() { bmTree = await inv("bookmark_rows"); bmSync(); renderBm(); }
async function toggleBm(nm) {
  await inv("toggle_bookmark", { name: nm });   // by NAME (R9.4/R20.4): ON appends at the top level, OFF removes at any depth
  await refreshBm();                            // the TREE is the model the pane paints — never patch bmCache behind it
}
/* ---------- bmfolder: the four structural commands ----------
   Every one is addressed by the ROW INDEX the pane just painted, and every one
   returns the new row vector, so the pane repaints what rust actually wrote
   instead of a prediction of it. NONE of them takes a vault path: a group
   operation moves NAMES and never touches a .md (R4X.2, criterion 6). */
async function bmApply(cmd, args) {
  try { bmTree = await inv(cmd, args); }
  catch (err) { say(String(err && err.message || err)); return; }   // a refused move (into its own child) is stated, not swallowed
  bmSync();
  renderBm();
}
const bmGroupNew = parent => bmApply("bm_group_new", { parent });   // parent = a group's row index, or null for the top level
const bmGroupDelete = ix => bmApply("bm_group_delete", { ix });     // the SUBTREE goes with it, and nothing asks (R4X.3)
function bmGroupRenameStart(ix) { bmRenaming = ix; renderBm(); }    // renderBm focuses+selects the editor it just painted
function bmGroupRenameCommit(ix, title) { bmRenaming = null; return bmApply("bm_group_rename", { ix, title }); }
/* ---------- bmdrag: DRAG A BOOKMARK ROW (R4X.n placeholders) ----------
   R24.6's mouse machine, not a second convention (and HTML5 dragstart/
   dataTransfer stays used NOWHERE — the R24.6 comment is binding here too):
   mousedown + window mousemove/mouseup, a 6px Manhattan threshold, a
   #tabghost chip, targets resolved once per rAF frame against rects cached at
   drag start, the census published only on a DECISION change, the drag's
   click eaten, the commit through bmApply.
   THE DECISION is [bmdrop:...] ([wfp:] pattern, criterion 3) — the app's own
   computed drop target, so a phase asserts what the app DECIDED:
     gap@<parent>.<pos> — insert between siblings; <parent> = the parent
       group's flat row ix, '-' for the top level; <pos> = painted slot among
       its children. The pane background below the last row SNAPS to the END
       of the top level at root depth (recon-bmdrag case 3).
     grp@<ix>          — onto group row <ix>: the drop PREPENDS as its first
       child (recon case 2, row fill, no line).
     none              — drag live, no legal target: over the source row or
       its own descendants NOTHING paints, and mouseup calls NOTHING — the
       refusal never even reaches the backend (recon case 6; the file is
       asserted on BYTES because stock rewrites identical bytes there).
   Groups drag exactly like files, the whole subtree moves intact (case 5).
   Spring-load (case 4: a COLLAPSED group under a held hover opens mid-drag,
   takes the drop as a prepend, stays open). collapseall R5 makes groups
   collapsible, so it ships here: a grp@<ix> decision on a .bmfold row arms a
   BM_SPRING_MS timer (0.85s, spec R5 / M6); ANY decision change or the mouseup
   disarms it. On fire the group leaves bmFolds (in memory — no inv(), R2),
   renderBm() repaints, and the zone cache is REBUILT, because the rects cached
   at drag start do not contain the rows that just started painting. */
const BM_SPRING_MS = 850;
let bmDropTok = "", bmEat = 0, bmDrop = null;
function bmClickEaten() {                  // a completed drag must not also open the note under the cursor
  const t = bmEat;
  bmEat = 0;
  return !!t && performance.now() - t < 400;
}
function bmSubEnd(ix) {                    // one past the last painted row of ix's subtree
  const d = bmTree[ix].depth;
  let j = ix + 1;
  while (j < bmTree.length && bmTree[j].depth > d) j++;
  return j;
}
function bmDragStart(e, ix) {
  if (e.button !== 0 || bmRenaming !== null) return;
  const src = bmTree[ix];
  if (!src) return;
  const sx = e.clientX, sy = e.clientY;
  const label = src.label ?? (src.kind === "g" ? src.name : src.name.split("/").pop());
  let ghost = null, line = null, hl = null, srcEl = null, zones = null, raf = 0, last = null;
  let spring = 0;                          // collapseall R5: the armed spring-load timer
  bmDrop = null;
  const clearFb = () => {
    if (line) { line.remove(); line = null; }
    if (hl) { hl.classList.remove("bmdrop-into"); hl = null; }
  };
  const disarm = () => { if (spring) { clearTimeout(spring); spring = 0; } };
  // rects cached ONCE per paint (R20, like treeDragStart), plus the parent/slot
  // tables, so a gap names its exact slot without a per-frame model walk. Rebuilt
  // only when a spring-load repaints the pane mid-drag.
  const cacheZones = () => {
    const rows = [...document.querySelectorAll("#bmlist .bmrow")];
    const par = bmTree.map((_, i) => bmParentOf(i));
    const cix = bmTree.map((_, i) => { let n = 0; for (let k = 0; k < i; k++) if (par[k] === par[i]) n++; return n; });
    zones = {
      // collapseall: a folded row (.bmhide) keeps its slot so rows[i] stays bmTree[i],
      // but it is not a target — vis=false, and hit tests skip it
      rows: rows.map((el, i) => ({ el, i, r: el.getBoundingClientRect(), vis: !el.classList.contains("bmhide") })),
      box: $("bmlist").getBoundingClientRect(),
      par, cix,
      rootLen: par.filter(p => p === null).length,
      exFrom: ix, exTo: bmSubEnd(ix),      // the dragged row and its descendants are not targets
    };
    if (srcEl) srcEl.classList.remove("bmdrop-src");
    srcEl = rows[ix];
    if (srcEl) srcEl.classList.add("bmdrop-src");   // the dragged row keeps its fill (recon case 1)
  };
  const springFire = key => {
    spring = 0;
    if (!ghost || !bmFolds.has(key)) return;
    act("bm_spring", { group: key.split("\u001f").join("/") }, () => {
      bmFolds.delete(key);                 // in memory only — the file is never written (R2)
      clearFb();
      renderBm();                          // repaints: every cached rect/element is now stale
      cacheZones();
      bmDropTok = "";                      // force the next frame to re-publish + re-highlight
      if (last && !raf) raf = requestAnimationFrame(step);
    });
  };
  const step = () => {
    raf = 0;
    const ev = last;
    if (!ghost) {
      if (Math.abs(ev.clientX - sx) + Math.abs(ev.clientY - sy) < 6) return;
      ghost = document.createElement("div");
      ghost.id = "tabghost";
      ghost.textContent = label;
      document.body.appendChild(ghost);
      cacheZones();
      bmDropTok = "none";
      updateTitle();
    }
    ghost.style.transform = "translate3d(" + (ev.clientX + 10) + "px," + (ev.clientY + 12) + "px,0)";
    const z = zones;
    let nd = null;                         // {grp, parent, pos, y}
    let lastVis = null;                    // the last PAINTED row (a folded tail has no bottom)
    for (const w of z.rows) if (w.vis) lastVis = w;
    const gapAt = j => j >= z.rows.length
      ? { parent: null, pos: z.rootLen, y: lastVis ? lastVis.r.bottom : z.box.top }
      : { parent: z.par[j], pos: z.cix[j], y: z.rows[j].r.top };
    if (ev.clientX >= z.box.left && ev.clientX <= z.box.right &&
        ev.clientY >= z.box.top && ev.clientY <= z.box.bottom) {
      let hit = null;
      for (const w of z.rows) if (w.vis && ev.clientY >= w.r.top && ev.clientY < w.r.bottom) { hit = w; break; }
      if (!hit) {
        // pane background: the line SNAPS below the last row, root depth (case 3)
        if (!lastVis || ev.clientY >= lastVis.r.bottom) nd = gapAt(z.rows.length);
      } else if (hit.i >= z.exFrom && hit.i < z.exTo) {
        nd = null;                         // self or own descendant: NOTHING paints (case 6)
      } else {
        const band = ev.clientY - hit.r.top;
        const grp = bmTree[hit.i] && bmTree[hit.i].kind === "g";
        if (band < 6) nd = gapAt(hit.i);                       // top band: the gap above
        else if (grp && band < hit.r.height - 6) nd = { grp: hit.i, parent: hit.i, pos: 0, y: 0 };   // group middle: PREPEND (case 2)
        // bottom band / file middle: the gap below. Below a COLLAPSED group that is
        // after its whole (unpainted) subtree, never inside it (collapseall)
        else nd = gapAt(hit.el.classList.contains("bmfold") ? bmSubEnd(hit.i) : hit.i + 1);
      }
      // the slot the row already occupies is not a move — treeDragStart's
      // "the folder it is ALREADY in is not a destination", gap edition
      if (nd && nd.grp == null && nd.parent === z.par[ix] && (nd.pos === z.cix[ix] || nd.pos === z.cix[ix] + 1)) nd = null;
      // a first-child slot reached via a gap can still name a dragged group's
      // descendant as parent — same refusal as the row-band test above
      if (nd && nd.parent !== null && nd.parent >= z.exFrom && nd.parent < z.exTo) nd = null;
    }
    const tok = nd == null ? "none" : (nd.grp != null ? "grp@" + nd.grp : "gap@" + (nd.parent === null ? "-" : nd.parent) + "." + nd.pos);
    if (tok !== bmDropTok) {
      clearFb();
      disarm();                            // R5: a decision change restarts the hover clock
      if (nd && nd.grp != null && z.rows[nd.grp].el.classList.contains("bmfold")) {
        const key = z.rows[nd.grp].el.dataset.bmk;
        spring = setTimeout(() => springFire(key), BM_SPRING_MS);
      }
      if (nd) {
        if (nd.grp != null) {
          hl = z.rows[nd.grp].el;
          hl.classList.add("bmdrop-into"); // measured row fill, no line (case 2)
        } else {
          line = document.createElement("div");
          line.id = "bmdropline";          // 3px accent line spanning the pane (cases 1/3)
          line.style.left = z.box.left + "px";
          line.style.width = z.box.width + "px";
          line.style.top = (nd.y - 1) + "px";
          document.body.appendChild(line);
        }
      }
      bmDrop = nd;
      bmDropTok = tok;
      ghost.textContent = label + (nd ? " -> " + tok : "");   // the chip names the decision
      updateTitle();                       // ONLY on a decision change — never per frame
    }
  };
  const move = ev => { last = ev; if (!raf) raf = requestAnimationFrame(step); };
  const up = async () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    if (raf) { cancelAnimationFrame(raf); raf = 0; if (last) step(); }
    disarm();                              // R5: a release before 0.85s opens nothing
    const d = bmDrop, dragged = !!ghost;
    if (ghost) { ghost.remove(); ghost = null; }
    clearFb();
    if (srcEl) srcEl.classList.remove("bmdrop-src");
    bmDropTok = "";
    bmDrop = null;
    if (!dragged) return;                  // below the threshold: a plain click, let it through
    bmEat = performance.now();
    if (!d) { updateTitle(); return; }     // no legal target: NOTHING is called — byte-level refusal
    await bmApply("bm_drag", { ix, parent: d.parent, pos: d.pos });
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}
/* ---------- R4X.7 the Edit bookmark modal — the MOVE route ----------
   Stock's `Edit...` opens a modal whose `Bookmark group` dropdown lists the
   existing groups by TITLE with nesting shown by indentation (12-groupdd.png),
   and picking one + Save relocates the bookmark, first among that group's
   children, without touching the note on disk (14-saved.png).
   THE ONE DELIBERATE DELTA: option 0 is a TOP-LEVEL entry. Stock's chooser has
   no such option — measured three ways (16-dd2.png, 18-dd-up-shot.png,
   19-crop.png) — so in stock this route cannot move a bookmark back OUT and
   only drag-and-drop can. Drag is goal bmdrag; criterion 2 needs both
   directions through the menu, so opensidian ships the superset and records it. */
const BM_TOP = "(top level)";
function bmGroupOpts() {                   // option 0 = the top level, then every group row in pre-order
  const out = [{ ix: null, title: BM_TOP, depth: 0 }];
  bmTree.forEach((r, i) => { if (r.kind === "g") out.push({ ix: i, title: r.name, depth: r.depth }); });
  return out;
}
function bmParentOf(ix) {                  // the row index of the group this row sits in, or null at the top level
  const d = bmTree[ix] ? bmTree[ix].depth : 0;
  for (let i = ix - 1; i >= 0; i--) if (bmTree[i].depth < d) return bmTree[i].kind === "g" ? i : null;
  return null;
}
function openBmEdit(ix) {
  const r = bmTree[ix];
  if (!r || r.kind !== "f") return;        // a group row has no Edit... in stock's menu either
  const opts = bmGroupOpts();
  bmEdit = { ix, name: r.name, opts };
  const sel = $("bme-grp");
  sel.textContent = "";
  opts.forEach((o, i) => {
    const op = document.createElement("option");
    op.value = String(i);
    op.textContent = "  ".repeat(o.depth) + o.title;   // nesting shown by indentation, like stock's chooser
    sel.appendChild(op);
  });
  const p = bmParentOf(ix);
  const cur = opts.findIndex(o => o.ix === p);
  sel.value = String(cur < 0 ? 0 : cur);
  $("bme-path").value = r.name;            // read-only: a bookmark IS the vault-relative name (R9.4), there is no title to edit
  $("bmebox").hidden = false;
  sel.focus();
  updateTitle();
}
function closeBmEdit() {
  if (!bmEdit) return;
  bmEdit = null;
  $("bmebox").hidden = true;
  updateTitle();
}
async function bmEditSave() {
  if (!bmEdit) return;
  const o = bmEdit.opts[Number($("bme-grp").value)] || bmEdit.opts[0];
  const ix = bmEdit.ix;
  closeBmEdit();
  await bmApply("bm_move", { ix, into: o.ix });    // into: null == back to the top level (the delta above)
}
/* [modal:bmedit] + [bmedit:<name>|<selected>|<option>|<option>…] (R4X.7) +
   [bmex:<cx,cy>;…] for the chooser and the two buttons, the [delx:] pattern:
   a driver clicks what it measured. The option list is published by TITLE with
   the indentation stripped — the indent is presentation, and a shell assertion
   should not have to spell non-breaking spaces. */
function bmEditTok() {
  const q = s => String(s == null ? "" : s).replace(/[|\]]/g, "");
  const sel = $("bme-grp");
  const cur = bmEdit.opts[Number(sel.value)] || bmEdit.opts[0];
  const xs = [sel, $("bme-no"), $("bme-yes")].map(e => {
    const r = e.getBoundingClientRect();
    return Math.round(r.left + r.width / 2) + "," + Math.round(r.top + r.height / 2);
  }).join(";");
  return " [bmedit:" + q(bmEdit.name) + "|" + q(cur.title) + "|" +
         bmEdit.opts.map(o => q(o.title)).join("|") + "] [bmex:" + xs + "]";
}
/* ---------- bmactive: "Bookmark the active tab..." — the SAME card, Add mode ----------
   THE SPEC is docs/recon-bmactive/README.md (stock 1.13.7, black box):
   F1 the modal is "Add bookmark": Path (read-only basename), Title (EMPTY, the
      basename as placeholder, FOCUSED), Bookmark group (preset to the group
      right-clicked), Cancel / Save, an X. So it is #bmebox with the Title row
      and the X shown — one modal geometry, not a second card.
   F2 Save APPENDS a file item as the LAST child of the chosen group; `title` is
      written only when typed (bm_add_in, the one serializer does the bytes).
   F3 an already-bookmarked note is DUPLICATED, never moved: no lookup here.
   F4/F7 the active tab is the last-focused MAIN leaf: rTrack() (lgpanes), not
      rNote() — rNote maps a local graph to its note, and stock NO-OPS on a local
      graph and on an empty tab (our zero-tab pane). A GLOBAL graph is the one
      deliberate delta (spec B2): stock writes a type:"graph" item this pane
      cannot paint, so we refuse VISIBLY and write nothing.
   F6 Cancel / Escape / X write nothing: closing never calls inv().
   bmAdd is its own state (not bmEdit) so R4X.7's [modal:bmedit] census stays
   exactly what it was; the add census is one ui/census.js line (bmAddTok). */
let bmAdd = null;                          // the open Add bookmark modal: {name, opts}
function openBmAdd(gix) {
  const t = rTrack();
  if (!t) return;                          // a pane with no tab = stock's empty tab: no-op (108-emptyitem)
  if (t.kind === "gg") { say("Bookmarking a graph view is not supported: the bookmarks pane cannot show a graph bookmark"); return; }
  if (t.kind) return;                      // local graph (and any non-note view): no-op (102-lgitem)
  const opts = bmGroupOpts();
  const cur = opts.findIndex(o => o.ix === gix);
  bmAdd = { name: t.name, opts };
  const sel = $("bme-grp");
  sel.textContent = "";
  opts.forEach((o, i) => {
    const op = document.createElement("option");
    op.value = String(i);
    op.textContent = "  ".repeat(o.depth) + o.title;
    sel.appendChild(op);
  });
  sel.value = String(cur < 0 ? 0 : cur);
  const base = t.name.split("/").pop();
  $("bmetitle").textContent = "Add bookmark";
  $("bme-path").value = base;
  $("bme-title").value = "";
  $("bme-title").placeholder = base;
  $("bme-trow").hidden = false;
  $("bme-x").hidden = false;
  $("bmebox").hidden = false;
  $("bme-title").focus();
  updateTitle();
}
function closeBmAdd() {
  if (!bmAdd) return;
  bmAdd = null;
  $("bmebox").hidden = true;
  $("bme-trow").hidden = true;             // back to the Edit card's shape
  $("bme-x").hidden = true;
  $("bmetitle").textContent = "Edit bookmark";
  updateTitle();
}
async function bmAddSave() {
  if (!bmAdd) return;
  const o = bmAdd.opts[Number($("bme-grp").value)] || bmAdd.opts[0];
  const name = bmAdd.name, title = $("bme-title").value;
  closeBmAdd();
  await bmApply("bm_add", { into: o.ix, name, title: title || null });
}
/* [modal:bmadd] [bmadd:<note>|<selected group>|<typed title>|<option>…]
   [bmax:<title>;<chooser>;<cancel>;<save>;<x>] — centres a driver clicks
   (the [bmex:] pattern), plus [bmafocus:title|-]: F1 says the Title input
   has the focus on open. */
function bmAddTok() {
  if (!bmAdd) return "";
  const q = s => String(s == null ? "" : s).replace(/[|\]]/g, "");
  const sel = $("bme-grp");
  const cur = bmAdd.opts[Number(sel.value)] || bmAdd.opts[0];
  const xs = [$("bme-title"), sel, $("bme-no"), $("bme-yes"), $("bme-x")].map(e => {
    const r = e.getBoundingClientRect();
    return Math.round(r.left + r.width / 2) + "," + Math.round(r.top + r.height / 2);
  }).join(";");
  return " [modal:bmadd] [bmadd:" + q(bmAdd.name) + "|" + q(cur.title) + "|" + q($("bme-title").value) + "|" +
         bmAdd.opts.map(o => q(o.title)).join("|") + "] [bmax:" + xs + "] [bmafocus:" +
         (document.activeElement === $("bme-title") ? "title" : "-") + "]";
}
$("bme-no").onclick = () => (bmAdd ? closeBmAdd() : closeBmEdit());   // Cancel writes nothing
$("bme-yes").onclick = () => (bmAdd ? bmAddSave() : bmEditSave());
$("bme-x").onclick = () => closeBmAdd();             // the X exists only in Add mode
$("bme-title").oninput = () => updateTitle();        // the census carries the typed title
$("bme-grp").onchange = () => updateTitle();        // the census follows the selection, keyboard or mouse
$("bmebox").addEventListener("keydown", e => {
  if (bmAdd) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); closeBmAdd(); return; }
    if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); bmAddSave(); return; }
    e.stopPropagation();                            // typing belongs to the Title input, not the global keymap
    setTimeout(updateTitle, 0);
    return;
  }
  if (!bmEdit) return;
  if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); closeBmEdit(); return; }
  if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); bmEditSave(); return; }
  e.stopPropagation();                              // arrows belong to the chooser, not to the global keymap
  setTimeout(updateTitle, 0);                       // a <select> changed by arrow keys publishes its new value
});
$("bmebox").addEventListener("mousedown", e => { if (e.target === $("bmebox")) e.preventDefault(); });   // click-off is not Save
function noteMenu(e, nm) {                 // right-click a tree note row
  e.preventDefault();
  e.stopPropagation();
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  const mkItem = (label, fn) => {
    const d = document.createElement("div");
    d.textContent = label;
    d.onmousedown = ev => ev.stopPropagation();
    d.onclick = () => { closeMenu(); fn(); };
    m.appendChild(d);
  };
  /* opentab REQ-10/11/12/16: stock 1.13.7's file menu opens with this group (stock/c07-b.json) */
  { const { item, sep } = bmMenuItems(m);
    item("Open in new tab",    () => openNewTab(nm));                     // WIRED: REQ-11
    item("Open to the right",  () => splitWith(fg(), "row", mkTab(nm)));  // WIRED: REQ-12, the bookmark menu's verb
    item("Open in new window", null, "Single-window app: there is no second window to open into");   // REQ-16 EXCEPTION
    sep(); }
  mkItem(bmCache.includes(nm) ? "Remove bookmark" : "Bookmark", () => toggleBm(nm));
  /* R24.7: Delete lives HERE, on the explorer row, because that is where the
     user is when they decide a note is finished — and it opens a question, not
     a deletion. askDelete does no I/O of its own beyond counting the inbound
     links the confirmation has to state. */
  mkItem("Delete", () => askDelete(nm));
  /* [mt:note:<name>] — WHICH ROW the browser's hit test actually handed us. The
     explorer row is found by OCR in the harness, and R20.8's lesson applies
     harder here than it did for tabs: an OCR row miss followed by "Delete" would
     delete a note nobody named and still look green. */
  m.dataset.mt = "note:" + nm;
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
/* R24.6 explorer drag-to-move, declared here for the same reason as dropTok:
   updateTitle() reads both and runs from load-time code ABOVE the explorer
   section, where a `let` in its own section would be a temporal-dead-zone
   throw rather than an empty token. [dragt:] is live (a drag is up, this is
   the destination it resolved to), [mv:] is the last completed move. */
let dragTok = "", mvTok = "";
/* F2 test hook: the vault-switch race lives inside the debounce window, so it
   is not mechanically reproducible at 250ms — OPENSIDIAN_SAVE_MS widens it for
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
    try { await flushSave(h); h.flushedAt = tgSeq; }   // tabclose: witness for the session-replace record
    finally { clearTimeout(h.saveT); h.saveT = null; }
  }
  // R28: the LAYOUT timer is process-wide, so the loop above cannot reach it.
  // Same rule as the buffers — flush into the vault we are leaving, then make
  // sure nothing from it can still fire into the next one.
  try { await wsLeave(); } catch (e) { /* a layout is never worth blocking a switch */ }
}

/* -> true iff bytes reached DISK in this call (false = nothing needed writing,
   or the write failed and the banner is up).

   tabclose: this used to `return` on `!g.saveT` — "no pending timer, nothing
   to write". That is true for every buffer the EDITOR filled (typing arms the
   debounce) and false for the one buffer that matters here: undoCloseTab
   restores rescued keystrokes with reloadInPlace, which arms no timer, so the
   tab read DIRTY and flushed NOTHING, and the next close dropped the rescue on
   the floor (phase_tabrepro RP4 at cdb768f: seq=5 dirty=1 flushed=1 with the
   bytes nowhere on disk). The timer is not the question — the BUFFER is. */
async function flushSave(g) {               // write g's pending edits NOW
  if (!g) return false;
  lpCommit(g);                              // fold any active lp raw row first
  const armed = !!g.saveT;
  clearTimeout(g.saveT); g.saveT = null;
  if (!armed && !bufDirty(g)) return false; // genuinely nothing at risk
  return await saveBuf(g);                  // R11.3 merge-before-write
}
/* "does the active tab hold bytes the disk does not have?" — the one test
   behind both the [dirty:] census and every flush decision. A non-note tab
   (kind) and a tab with no base have no buffer of their own. */
function bufDirty(g) {
  if (!g || g.active < 0) return false;
  const t = g.tabs[g.active];
  return !!(t && !t.kind && t.base !== undefined && bufOf(g) !== t.base);
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
    // R4X.1 (navbtn): Back/Forward live at the LEFT edge of the view header,
    // before the tabs — stock 1.13.7 measured placement (docs/recon-navbtn
    // §1: back then forward, thin chevrons, ~28px apart, left of the title).
    // Stock's titlebar variant is NOT copied: our titlebar is a gated surface
    // (phases hdrdragwm/wmframe) — divergence recorded in the recon README.
    '<div class="tabbar"><div class="navbtns">' +
      '<button class="navbtn navback" title="Navigate back" disabled>' +
        '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M15 5l-7 7 7 7"/></svg></button>' +
      '<button class="navbtn navfwd" title="Navigate forward" disabled>' +
        '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 5l7 7-7 7"/></svg></button>' +
    '</div><div class="tabs"></div>' +
    '<button class="modebtn" title="toggle reading view (Ctrl+E)"></button></div>' +
    '<div class="content">' +
      '<canvas class="graph" hidden></canvas>' +
      '<div class="ac" hidden></div>' +
      '<div class="status status-bar" hidden><span class="st-bl"></span><span class="st-wc"></span><span class="st-cc"></span></div>' +
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
  g.navback = q(".navback"); g.navfwd = q(".navfwd");
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
  // R4X.2 (navbtn): the buttons walk THIS pane's history through the R19
  // model (histGo — same entry point as Alt+Left/Right, mouse 8/9, palette).
  // A disabled button never fires (native disabled), matching stock's
  // measured no-op (docs/recon-navbtn §2); histGo's own range guard backs
  // that up even if the disabled paint were ever stale.
  g.navback.onclick = () => histGo(-1, g);
  g.navfwd.onclick = () => histGo(1, g);
  // R4X.5: plain right-click drops the history menu (navHistMenu — stock's
  // measured gesture, docs/recon-navbtn §4); a disabled button opens nothing.
  g.navback.oncontextmenu = e => navHistMenu(e, g, -1, g.navback);
  g.navfwd.oncontextmenu = e => navHistMenu(e, g, 1, g.navfwd);
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
    /* R25.13f: a POINTER CLICK inside this editor is one of the three things
       that remove the search flash (the others are the next result click and
       the editor instance going away). Keystrokes, blur, undo and tab
       switches do NOT — that was measured, shots 19-25, and the flash is
       stored on the view precisely so nothing else has to remember it. */
    if (g) scClear(g);
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
  // R26: the find bar belongs to the NOTE it was opened on. Its hits are model
  // positions in THAT text, and its marks are spans in THAT view's rows — both
  // are meaningless one tab later, and the marks would ride back into the old
  // view when it is re-attached. Closing here (without moving the caret: this
  // is a tab switch, not an Escape) is the one choke point every swap passes.
  if (g.find && g.find.open) fClose(g, false);
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

/* ---------- R28 WORKSPACE PERSISTENCE: quit, relaunch, your layout is there ---
   The file is the vault's own `.obsidian/workspace.json`, in STOCK's shape
   (R28.1/R28.16) — an opensidian vault stays openable by stock Obsidian and back.
   Shape, as measured off a stock vault:
     { main:  { id, type:"split", direction:"vertical"|"horizontal",
                children: [ { id, type:"tabs", currentTab?, dimension?,
                              children: [ { id, type:"leaf",
                                            state:{ type:"markdown",
                                                    state:{ file, mode, source },
                                                    icon, title } } ] } ] },
       left / right: the sidebars, same node grammar, `collapsed` on the split,
       active: <the focused LEAF's id>,
       lastOpenFiles: [names, newest first] }
   Two shape details that are not decoration:
     R28.5  `currentTab` is OMITTED when it is 0 — stock writes it only when a
            tab other than the first is active, and a file that carries it
            anyway is a file stock did not write.
     R28.6  a split child's size is a PERCENTAGE (`dimension`), never pixels, so
            a layout restored into a differently sized window keeps its
            PROPORTIONS instead of overflowing or leaving a gap.
   `direction` is stock's, and it is the OPPOSITE word to ours: stock's
   "vertical" split stands its children side by side (our dir:"row"), its
   "horizontal" stacks them (our dir:"col"). Translated in exactly two places
   (wsNode and wsNodeIn) so the confusion cannot spread.
   R28.4 leaf ids are STABLE across restarts: an id is minted once per tab and
   then carried through the file, so a restore is a restore (the ids the next
   save writes are the ids the last one wrote) rather than a rebuild. */
const WS_VIEW_L = { files: "file-explorer", search: "search", bm: "bookmarks" };
const WS_VIEW_R = { bl: "backlink", out: "outgoing-link", tags: "tag", toc: "outline" };
let wsWrites = 0;        // successful write_workspace calls THIS PROCESS (census [ws:])
let wsRestored = 0;      // leaves rebuilt from the file on this launch
let wsDropped = 0;       // R28.17: leaves whose file was gone, dropped instead of failing
let wsT = null, wsLast = "", wsIds = {}, wsInFlight = null;
let wsGrp = {};          // W3: link-group key ("L<link>" | "G<group id>") -> the 16-hex `group` value on file
/* W4 (F6, frozen): what this app does not understand it hands back unchanged.
     wsExtra   top-level keys other than WS_TOP, in file order (stock drops
               them; keeping them is the documented safe divergence)
     wsSideRaw the left / right sidebar subtree AS READ. Stock keeps several
               leaves per sidebar (file-explorer+search+bookmarks; backlink+
               outgoing+localgraph+tag+all-properties+outline; `width`) and
               opensidian shows one pane at a time: writing only that one leaf
               would delete the rest from a stock vault opened here once
     wsSide0   the pane each sidebar showed right after the restore. While it
               is unchanged the file's currentTab stands — it may name a leaf
               there is no pane for here (a stock sidebar local graph) */
let wsExtra = null, wsSideRaw = {}, wsSide0 = {};
const WS_TOP = ["main", "left", "right", "active", "lastOpenFiles"];
const WS_KNOWN = new Set(["markdown", "graph", "localgraph", "empty"]);
const wsClone = o => JSON.parse(JSON.stringify(o));
const wsId = () => {     // stock's ids are 16 hex chars; the VALUE is opaque, only stability matters
  let s = "";
  for (let i = 0; i < 16; i++) s += ((Math.random() * 16) | 0).toString(16);
  return s;
};
const wsIdOf = (o, k) => (o[k] || (o[k] = wsId()));
// Stock stores VAULT PATHS WITH THE EXTENSION ("file": "sub/A.md", lastOpenFiles
// ["sub/A.md"]); a tab here is named without it. Convert at the file boundary
// only — a bare name in a leaf is a file stock would not open (gate 1247154).
const wsPathOut = n => n + ".md";
const wsPathIn = f => (typeof f === "string" && f.endsWith(".md")) ? f.slice(0, -3) : f;
// W2 (F1/F2): the two graph views are leaves too — global graph and local graph
// persist with stock's type/state; any other kind (none today) is not written.
const wsPersistable = t => !!t && typeof t.name === "string" &&
  (!t.kind || t.kind === "gg" || (t.kind === "unk" && !!t.raw) || (t.kind === "lg" && typeof t.center === "string" && !!t.center));
const WS_GICON = "lucide-git-fork";
// F2: `options` is 23 keys of stock graph settings. Only three mean anything
// here (depth, incoming, outgoing); the rest ride along OPAQUE in t.opts so a
// stock file loses nothing by passing through (W2 frozen: round-tripped as-is).
function wsLgOpts(t) {
  const o = Object.assign({}, t.opts && typeof t.opts === "object" ? t.opts : {});
  o.localJumps = t.depth; o.localBacklinks = !!t.inc; o.localForelinks = !!t.out;
  return o;
}
function wsLeaf(t) {
  if (t.kind === "unk") {                    // W4: the leaf as read, id and all; `group` is re-decided by wsNode
    const l = wsClone(t.raw);
    delete l.group;
    return l;
  }
  if (t.kind === "gg")                       // F1: state is EMPTY in stock
    return { id: wsIdOf(t, "lid"), type: "leaf",
             state: { type: "graph", state: {}, icon: WS_GICON, title: "Graph view" } };
  if (t.kind === "lg")                       // F2: main-area lg is bound to its stored file
    return { id: wsIdOf(t, "lid"), type: "leaf",
             state: { type: "localgraph", state: { file: wsPathOut(t.center), options: wsLgOpts(t) },
                      icon: WS_GICON, title: "Graph of " + titleOf(t.center) } };
  // R28.9: the per-tab view mode, in stock's two orthogonal bits (see modeBits).
  // Without it every tab that was READING comes back as an editor.
  const st = t.read
    ? { file: wsPathOut(t.name), mode: "preview", source: !!t.src }
    : { file: wsPathOut(t.name), mode: "source", source: !!t.src };
  return { id: wsIdOf(t, "lid"), type: "leaf",
           state: { type: "markdown", state: st, icon: "lucide-file", title: titleOf(t.name) } };
}
/* W3 (F3, stock 1.13.7): LINKED LEAVES CARRY A SHARED `group` (16 hex, opaque)
   on every member, and stock restores them linked. Two link models live here:
   manual links (t.link, R13 — a tab-level set) and the lg auto-link (t.linkId =
   a PANE id, R7.3 — the graph follows whatever tab is active in that pane). On
   file both become one `group` value:
     t.link L                        -> key "L<L>"
     lg with linkId P                -> the active tab of pane P is its partner:
                                        that partner's "L<L>" if it has one, else "G<P>"
     the active note of a pane some lg follows -> "G<pane id>"
   An lg whose partner is not a note (a graph is focused there) writes no group:
   a group of one links nothing. The hex is minted once per key and remembered
   (wsGrp), and wsLinksIn seeds it from the file, so a restore writes back the
   bytes it read (stock: byte-identical across relaunch). */
function wsLkPartner(pid) {
  const h = groups().find(x => x.id === pid);
  const pt = h && h.active >= 0 ? h.tabs[h.active] : null;
  return pt && !pt.kind && wsPersistable(pt) ? pt : null;
}
function wsGroupKey(t, g) {
  if (t.link != null) return "L" + t.link;
  if (t.kind === "lg") {
    if (t.linkId == null) return null;
    const pt = wsLkPartner(t.linkId);
    return pt ? (pt.link != null ? "L" + pt.link : "G" + t.linkId) : null;
  }
  if (t.kind || g.tabs[g.active] !== t) return null;
  return groups().some(h => h.tabs.some(x => x.kind === "lg" && x.linkId === g.id && wsPersistable(x)))
    ? "G" + g.id : null;
}
function wsNode(node) {
  if (node.children) {
    const kids = node.children.map(wsNode);
    const fr = node.fractions || [];
    const tot = fr.reduce((a, b) => a + (b || 0), 0) || kids.length;
    kids.forEach((k, i) => { if (k) k.dimension = Math.round(((fr[i] != null ? fr[i] : 1) / tot) * 1e4) / 1e2; });
    return { id: wsIdOf(node, "wid"), type: "split",
             children: kids.filter(Boolean),
             direction: node.dir === "row" ? "vertical" : "horizontal" };
  }
  const tabs = node.tabs.filter(wsPersistable);
  const act = node.active >= 0 ? tabs.indexOf(node.tabs[node.active]) : -1;
  const o = { id: wsIdOf(node, "wid"), type: "tabs", children: tabs.map(t => {
    const l = wsLeaf(t), k = wsGroupKey(t, node);
    if (k) l.group = wsIdOf(wsGrp, k);      // F3: stock writes `group` after `state`, on EVERY member
    return l;
  }) };
  if (act > 0) o.currentTab = act;          // R28.5: 0 is written by its ABSENCE
  return o;
}
function wsSide(which) {
  const open = which === "left" ? sideOpen : rightOpen;
  const view = which === "left" ? (WS_VIEW_L[sidePane] || "file-explorer") : (WS_VIEW_R[rTab] || "backlink");
  // R28.11: a sidebar leaf carries its OWN view state, not just which pane is
  // showing. Stock's search leaf comes back still holding its query, so the
  // query travels in the leaf's state where stock puts it.
  const lst = which === "left" && sidePane === "search" ? { query: $("sinput").value } : {};
  if (wsSideRaw[which]) return wsSideKeep(which, view, open, lst);
  const o = { id: wsIdOf(wsIds, which), type: "split", direction: "horizontal",
              children: [{ id: wsIdOf(wsIds, which + "tabs"), type: "tabs",
                           children: [{ id: wsIdOf(wsIds, which + "leaf"), type: "leaf",
                                        state: { type: view, state: lst } }] }] };
  // R28.10: the key is ABSENT when the sidebar is open, not `false` — stock
  // writes `collapsed` only for a collapsed sidebar, and a file carrying
  // `collapsed: false` is a file stock did not write.
  if (!open) o.collapsed = true;
  return o;
}
/* W4: a sidebar that came from the file is written back AS READ, with only the
   three facts this app owns laid over it: collapsed, which leaf is showing
   (only once the user has changed pane — until then the file's currentTab
   stands), and the search leaf's query. Every other leaf, id, key and `width`
   rides through untouched. */
function wsSideTabs(o) {
  const ts = [];
  (function walk(n) {
    if (!n || typeof n !== "object" || !Array.isArray(n.children)) return;
    if (n.type === "tabs") ts.push(n); else n.children.forEach(walk);
  })(o);
  return ts;
}
function wsSideKeep(which, view, open, lst) {
  const cur = which === "left" ? sidePane : rTab;
  const moved = cur !== wsSide0[which];
  const has = tn => tn.children.some(l => l && l.state && l.state.type === view);
  const rts = wsSideTabs(wsSideRaw[which]);
  if (moved && rts.length && !rts.some(has))    // a pane the file had no leaf for: ADD one (never replace),
    rts[0].children.push({ id: wsId(), type: "leaf", state: { type: view, state: {} } });   // kept in raw so its id is stable
  const o = wsClone(wsSideRaw[which]);
  let hit = null;
  for (const tn of wsSideTabs(o)) {
    const i = tn.children.findIndex(l => l && l.state && l.state.type === view);
    if (i >= 0) { hit = { tn, i }; break; }
  }
  if (moved && hit) { if (hit.i > 0) hit.tn.currentTab = hit.i; else delete hit.tn.currentTab; }
  if (hit && lst.query !== undefined) {
    const ls = hit.tn.children[hit.i].state;
    ls.state = Object.assign({}, ls.state && typeof ls.state === "object" ? ls.state : {}, lst);
  }
  if (open) delete o.collapsed; else o.collapsed = true;
  return o;
}
function wsDoc() {
  const ft = fg() && fg().active >= 0 ? fg().tabs[fg().active] : null;
  const d = {
    main: wsNode(state.root),
    left: wsSide("left"),
    right: wsSide("right"),
  };
  // W4: unknown top-level keys, where stock puts its own extra key (left-ribbon)
  if (wsExtra) for (const k of Object.keys(wsExtra)) d[k] = wsClone(wsExtra[k]);
  return Object.assign(d, {
    active: wsPersistable(ft) ? wsIdOf(ft, "lid") : "",   // R28.12: focus lands on the NAMED leaf
    // R28.15: `lastOpenFiles` is the note-use order, newest first. It is the
    // SAME list the quick switcher already keeps (mruList) rather than a second
    // one that could disagree with it — nothing reads the key back yet beyond
    // seeding that list on restore, which is exactly what the row says: written
    // for a recent-files affordance that does not exist.
    lastOpenFiles: mruList.slice(0, 20).map(wsPathOut),
  });
}
/* R28.3 — WRITTEN WHILE RUNNING, NOT AT EXIT. This is the data-loss row of the
   section and the reason there is no beforeunload/window-close handler anywhere
   in this feature: a handler that runs on a clean quit is exactly what a
   `kill -9`, an OOM kill or a power cut skips, and the layout would be lost in
   precisely the cases the user did not choose. wsTouch() is called from
   updateTitle(), which every mutation in this app already funnels through, so
   "something changed" needs no second list of call sites to fall out of date.
   THROTTLE, not debounce: the timer is armed by the first change and NOT reset
   by later ones, so continuous typing cannot starve the write forever — at most
   WS_MS of layout change is ever unwritten. The write is skipped when the
   serialized layout is byte-identical to the last one, so a typing burst costs
   one stringify and no IPC. */
const WS_MS = 400;
function wsTouch() {
  if (!state || !vaultPath || wsT) return;
  wsT = setTimeout(() => wsFlush(), WS_MS);
}
/* W6 — OFF THE note_open PATH. The throttle timer can fall due in the middle of
   an action (between two awaits of openInTab, or between its last IPC and the
   paint), and a flush there would put wsDoc + stringify + the write IPC inside
   the user-visible span. So while an action is open the flush YIELDS and
   re-arms in WS_YIELD_MS steps. The yield is CAPPED (WS_YIELD_MAX): a paint
   that never comes (hidden window, throttled rAF) must not starve R28.3, so
   after the cap the write goes anyway — worst case 400 + 2000ms unwritten,
   still inside the 5s the wspace phase asserts. `force` (wsLeave) never yields:
   a vault switch is a hard boundary. */
const WS_YIELD_MS = 50, WS_YIELD_MAX = 40;
let wsYields = 0;
async function wsFlush(force) {
  wsT = null;
  if (!state || !vaultPath) return;
  if (!force && actN > 0 && wsYields < WS_YIELD_MAX) { wsYields++; wsT = setTimeout(() => wsFlush(), WS_YIELD_MS); return; }
  wsYields = 0;
  let doc, s;
  try { doc = wsDoc(); s = JSON.stringify(doc); } catch (e) { return; }   // never let a census update throw
  if (s === wsLast) return;
  // The vault is captured HERE, with the bytes it describes, and travels with
  // them: everything below this line is asynchronous, and `vaultPath` is not a
  // constant across an await (see wsLeave). The backend refuses a write whose
  // `vault` is not the one it has open, so a layout serialized in A can never
  // land in B no matter how the timers fall.
  const v = vaultPath;
  wsInFlight = (async () => {
    try {
      await inv("write_workspace", { vault: v, layout: doc });
      if (v !== vaultPath) return;     // switched under us: the backend dropped it, so must our bookkeeping
      wsLast = s; wsWrites++;
      updateTitle();                 // republish [ws:] — the write is observable, not asserted by faith
    } catch (e) { /* R28.13/R28.17: persistence must never be able to break the session */ }
  })();
  await wsInFlight;
  wsInFlight = null;
}
/* A VAULT SWITCH IS A HARD BOUNDARY FOR THE LAYOUT WRITER, and it needs its own
   function because leaveVault's existing loop disarms `saveT` per group — it
   knows nothing about the single process-wide layout timer. Three things have
   to happen, in this order, BEFORE `set_vault` swaps the root:
     1. flush what is pending, so leaving a vault persists the layout you had
        (R28.3 is "while running", and a switch is not an exit);
     2. await any write already in flight, because clearTimeout cannot recall an
        IPC that has left;
     3. forget `wsLast` and `wsIds`. wsLast is a CONTENT dedupe: carried across
        the switch, a vault B whose layout happened to serialize identically to
        A's would have its first write skipped and keep a stale file forever.
        wsIds are the ids the FILE named (R28.4) and belong to A's file alone. */
async function wsLeave() {
  if (wsT) { clearTimeout(wsT); wsT = null; await wsFlush(true); }
  try { await wsInFlight; } catch (e) { /* its own catch already ate it */ }
  wsInFlight = null; wsLast = ""; wsIds = {}; wsGrp = {};
  wsExtra = null; wsSideRaw = {}; wsSide0 = {};          // W4: A's unknowns belong to A's file alone
}

/* ---------- R28 GROUP 2: READ IT BACK ----------------------------------------
   The inverse of wsDoc(), and deliberately written as a SEPARATE pair of
   functions rather than a generic walker: the two directions disagree about
   what is authoritative. Writing trusts the live model; READING trusts nothing
   — the file may have been written by stock, by an older opensidian, by a half
   finished sync, or by a text editor. Every branch below therefore has a "this
   is not what I expected" exit that returns null, and a null anywhere means
   DROP THAT SUBTREE, never "fail the restore" (R28.17).

   THE DEGRADE RULE, stated once so the three call sites can be read against it:
     leaf  whose file is gone / has no file  -> dropped, wsDropped++  (R28.17/R28.14)
     tabs  node left with zero tabs          -> dropped, the group does not exist
     split node left with zero children      -> dropped
     split node left with ONE child          -> collapses to that child; a split
                                                with one side is not a split, and
                                                leaving it would show the user a
                                                divider they cannot remove
     nothing restorable at all               -> false, and the caller takes the
                                                ordinary first-launch path (R28.13)
   R28.8 IS AN EXPLICIT NON-GOAL: stock does not restore scroll position and
   neither does this. Cloning the absence is the requirement — no hpos/scroll
   value is read here, and none is written by wsLeaf(). */
function wsGraphIn(leaf, have) {
  const ls = leaf.state, st = ls.state && typeof ls.state === "object" ? ls.state : {};
  let t;
  if (ls.type === "graph") {                         // F1: nothing to bind, nothing to check
    t = { kind: "gg", name: "Graph view", mode: "source", hist: [], hpos: -1 };
  } else {
    const c = wsPathIn(st.file);
    // a local graph of a note that is gone is dropped like that note's tab
    // (R28.17): a graph centred on nothing is not a view the user can use
    if (typeof c !== "string" || !c || !have.has(c)) { wsDropped++; return null; }
    const o = st.options && typeof st.options === "object" && !Array.isArray(st.options) ? st.options : {};
    const d = Number.isInteger(o.localJumps) ? Math.min(5, Math.max(1, o.localJumps)) : 1;
    t = { kind: "lg", name: "Graph of " + c.split("/").pop(), center: c, depth: d,
          inc: o.localBacklinks !== false, out: o.localForelinks !== false,
          opts: o, mode: "source", hist: [], hpos: 0 };
  }
  if (typeof leaf.id === "string" && leaf.id) t.lid = leaf.id;   // R28.4
  wsRestored++;
  return t;
}
function wsTabIn(leaf, have) {
  if (leaf && leaf.state && (leaf.state.type === "graph" || leaf.state.type === "localgraph"))
    return wsGraphIn(leaf, have);
  // W4 (F6): a leaf type this app has no view for (canvas, pdf, a plugin's view)
  // is KEPT as a placeholder tab carrying the leaf verbatim — id, slot, type,
  // state — the way stock keeps a disabled plugin's leaf ("Plugin no longer
  // active"). Nothing about it is checked: its file is not ours to judge.
  const ty = leaf && leaf.state && leaf.state.type;
  if (typeof ty === "string" && ty && !WS_KNOWN.has(ty) && typeof leaf.id === "string" && leaf.id) {
    const ti = leaf.state.title;
    wsRestored++;
    return { kind: "unk", name: typeof ti === "string" && ti ? ti : ty, utype: ty, raw: wsClone(leaf),
             lid: leaf.id, mode: "source", hist: [], hpos: -1 };
  }
  const st = leaf && leaf.state && leaf.state.state;
  const f = wsPathIn(st && st.file);
  if (typeof f !== "string" || !f) return null;      // stock's `empty` leaf carries no file
  if (!have.has(f)) { wsDropped++; return null; }     // R28.17 / R28.14
  const t = mkTab(f);
  // R28.9: the two orthogonal bits back out of stock's two keys — the exact
  // inverse of wsLeaf(). Assigned to t.read/t.src rather than through the
  // t.mode setter, because that setter is deliberately not symmetric (setting
  // "reading" leaves the source bit alone) and would silently lose one bit.
  t.read = st.mode === "preview";
  t.src = st.source === true;
  if (typeof leaf.id === "string" && leaf.id) t.lid = leaf.id;   // R28.4: the id the file names is the id we keep
  wsRestored++;
  return t;
}
/* W3 inverse. Members of one `group`, after the degrade drops (a member whose
   note is gone is simply absent; a group left with ONE member links nothing):
     >= 2 non-graph members         -> manual link (t.link) between them
     only local graphs (>= 2)       -> manual link between them (linkSync handles lg members)
     every lg member                -> follows the pane holding the group's note
                                       (the one ACTIVE in its pane, else the first),
                                       never its own pane
   and the key the next write computes for this set is mapped to the hex on file. */
function wsLinksIn(gs) {
  const by = new Map();
  for (const g of gs) for (const t of g.tabs) {
    if (typeof t.wsg !== "string") continue;
    if (!by.has(t.wsg)) by.set(t.wsg, []);
    by.get(t.wsg).push({ g, t });
    delete t.wsg;
  }
  for (const [hex, ms] of by) {
    if (ms.length < 2) continue;
    const lgs = ms.filter(m => m.t.kind === "lg"), rest = ms.filter(m => m.t.kind !== "lg");
    const notes = rest.filter(m => !m.t.kind);
    const anchor = notes.find(m => m.g.tabs[m.g.active] === m.t) || notes[0] || null;
    let key = null;
    const man = rest.length >= 2 ? rest : (!rest.length && lgs.length >= 2 ? lgs : null);
    if (man) { const L = ++linkSeq; for (const m of man) m.t.link = L; key = "L" + L; }
    if (anchor) for (const m of lgs) if (m.g !== anchor.g) m.t.linkId = anchor.g.id;
    if (!key && anchor && lgs.some(m => m.t.linkId === anchor.g.id)) key = "G" + anchor.g.id;
    if (key) wsGrp[key] = hex;
  }
}
function wsNodeIn(node, have) {
  if (!node || typeof node !== "object") return null;
  if (node.type === "split" && Array.isArray(node.children)) {
    const kids = [], dims = [];
    for (const c of node.children) {
      const k = wsNodeIn(c, have);
      if (!k) continue;                                 // a dropped child costs its share, not the split
      kids.push(k);
      // R28.6: the size on the file is a PERCENTAGE, so a layout restored into
      // a window of a different size keeps its PROPORTIONS. They are
      // renormalised over the SURVIVORS below, which is what makes a dropped
      // child give its space back instead of leaving a gap.
      dims.push(typeof c.dimension === "number" && c.dimension > 0 ? c.dimension : 1);
    }
    if (!kids.length) return null;
    if (kids.length === 1) return kids[0];
    const tot = dims.reduce((a, b) => a + b, 0) || kids.length;
    const out = { dir: node.direction === "vertical" ? "row" : "col",   // stock's word is the OPPOSITE of ours
                  children: kids, fractions: dims.map(d => d / tot) };
    if (typeof node.id === "string" && node.id) out.wid = node.id;      // R28.4
    return out;
  }
  if (node.type === "tabs" && Array.isArray(node.children)) {
    const want = Number.isInteger(node.currentTab) ? node.currentTab : 0;   // R28.5: absent means 0
    const g = mkGroup();
    let act = 0;
    node.children.forEach((leaf, i) => {
      const t = wsTabIn(leaf, have);
      if (!t) return;
      if (typeof leaf.group === "string" && leaf.group) t.wsg = leaf.group;   // W3: resolved by wsLinksIn once every group exists
      // the active index must survive the drops BEFORE it: if the tab that was
      // active is itself gone, focus falls back to the nearest survivor to its
      // left rather than to a tab the user was not looking at.
      if (i <= want) act = g.tabs.length;
      g.tabs.push(t);
    });
    if (!g.tabs.length) return null;
    if (typeof node.id === "string" && node.id) g.wid = node.id;
    g.active = Math.min(act, g.tabs.length - 1);
    return g;
  }
  return null;
}
/* Read the file. Separated from wsApply so enterVault can do it BEFORE the
   first renderLayout(): renderLayout calls updateTitle, updateTitle arms
   wsTouch, and a 400ms timer that fires before we have read would overwrite the
   very file we came to restore with the empty boot layout. */
async function wsRead() {
  try { return await inv("read_workspace"); } catch (e) { return null; }
}
async function wsApply(doc, names) {
  if (!doc || typeof doc !== "object") return false;     // R28.13: missing/corrupt == first launch
  // W4: captured BEFORE anything can bail — even a file whose main area is
  // unusable must not lose its unknown keys or its sidebars on the next write
  wsExtra = {};
  for (const k of Object.keys(doc)) if (!WS_TOP.includes(k)) wsExtra[k] = wsClone(doc[k]);
  for (const k of ["left", "right"])
    if (doc[k] && typeof doc[k] === "object" && !Array.isArray(doc[k]) && wsSideTabs(doc[k]).length)
      wsSideRaw[k] = wsClone(doc[k]);
  const have = new Set(names);
  let root = null;
  try { root = wsNodeIn(doc.main, have); } catch (e) { root = null; }
  if (!root) { wsSide0 = { left: sidePane, right: rTab }; return false; }
  if (!root.children) root = { dir: "row", children: [root], fractions: [1] };
  state.root = root;
  state.focused = null;
  renderLayout();
  const gs = groups();
  // R28.12: focus lands on the leaf NAMED by `active`, not on "the first group"
  let target = gs[0];
  if (typeof doc.active === "string" && doc.active) {
    for (const g of gs) {
      const i = g.tabs.findIndex(t => t.lid === doc.active);
      if (i >= 0) { g.active = i; target = g; break; }
    }
  }
  try { wsLinksIn(gs); } catch (e) { /* a link is not worth the layout */ }
  focusGroup(target);
  for (const g of gs) if (g !== target) await loadActive(g);
  await loadActive(target);                              // the focused pane renders LAST, so it owns the caret
  // R28.10 / R28.11: the sidebars, each independently, with their own view state
  try { wsSidesIn(doc); } catch (e) { /* a sidebar is not worth the layout */ }
  wsSide0 = { left: sidePane, right: rTab };             // W4: the file's currentTab stands until the user moves
  // R28.15: seed the quick switcher's recency from the file, filtered to notes
  // that still exist — the list is written for a recent-files affordance and a
  // dead name in it would offer the user a note they cannot open.
  if (Array.isArray(doc.lastOpenFiles)) {
    mruList = doc.lastOpenFiles.map(wsPathIn).filter(n => typeof n === "string" && have.has(n)).slice(0, 20);
  }
  // the ids the file used are the ids the next save writes (R28.4)
  for (const k of ["left", "right"]) {
    const s = doc[k];
    if (!s || typeof s.id !== "string") continue;
    wsIds[k] = s.id;
    const tabs = s.children && s.children[0];
    if (tabs && typeof tabs.id === "string") wsIds[k + "tabs"] = tabs.id;
    const leaf = tabs && tabs.children && tabs.children[0];
    if (leaf && typeof leaf.id === "string") wsIds[k + "leaf"] = leaf.id;
  }
  updateTitle();
  return true;
}
const WS_VIEW_L_IN = { "file-explorer": "files", search: "search", bookmarks: "bm" };
const WS_VIEW_R_IN = { backlink: "bl", "outgoing-link": "out", tag: "tags", outline: "toc" };
function wsSideLeaf(s) {               // the SHOWING leaf of a sidebar split (currentTab, absent = 0), or null
  const tabs = s && Array.isArray(s.children) ? s.children[0] : null;
  const ci = tabs && Number.isInteger(tabs.currentTab) ? tabs.currentTab : 0;
  const leaf = tabs && Array.isArray(tabs.children) ? tabs.children[ci] : null;
  return leaf && leaf.state ? leaf.state : null;
}
function wsSidesIn(doc) {
  // R28.10: `collapsed` is ABSENT when the sidebar is open, so "open" is the
  // absence of the key and not `collapsed === false`.
  const lOpen = !(doc.left && doc.left.collapsed === true);
  const rOpen = !!(doc.right && doc.right.collapsed !== true);
  const ls = wsSideLeaf(doc.left), rs = wsSideLeaf(doc.right);
  if (ls) {
    const p = WS_VIEW_L_IN[ls.type];
    if (p && p !== sidePane) setPane(p);
    // R28.11: the search leaf comes back holding its query, and the results
    // that query produced — the pane is restored, not merely selected.
    if (ls.state && typeof ls.state.query === "string" && ls.state.query) {
      $("sinput").value = ls.state.query;
      runSearch();                     // fire+forget: results fill in, boot is not blocked on them
    }
  }
  if (rs) {
    const t = WS_VIEW_R_IN[rs.type];
    if (t && t !== rTab) setRTab(t, false);
  }
  if (lOpen !== sideOpen) {
    sideOpen = lOpen;
    $("side").hidden = $("ldiv").hidden = !sideOpen;
    $("collapsebtn").title = sideOpen ? "Collapse sidebar" : "Expand sidebar";
  }
  if (rOpen !== rightOpen) {
    rightOpen = rOpen;
    $("rside").hidden = $("rdiv").hidden = !rightOpen;
    $("rtoggle").title = rightOpen ? "Collapse right sidebar" : "Expand right sidebar";
    placeRToggle();
    if (rightOpen) setRTab(rTab, false);
  }
}
/* R28.2 — THE WINDOW RECTANGLE GOES SOMEWHERE ELSE. Not in the vault: a vault
   synced between a laptop and a desktop would otherwise carry one machine's
   window size to the other and the two would overwrite each other on every
   launch. It is written to ~/.opensidian.json ("win") beside sidebar_w / theme /
   zoom, which are machine facts for the same reason. Throttled on resize, and
   deliberately NOT applied at startup — WHERE the geometry lives is the
   requirement; restoring it is a row nobody has written. */
let wgT = null;
function wsGeomTouch() {
  if (wgT) return;
  wgT = setTimeout(async () => {
    wgT = null;
    try {
      const r = await inv("win_rect");
      await inv("set_win_geom", { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.w), h: Math.round(r.h) });
    } catch (e) { /* geometry is a convenience; it never breaks a session */ }
  }, 700);
}
window.addEventListener("resize", wsGeomTouch);

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

/* rvcursor (criterion 3): a MOUSE drag over READING VIEW runs no application
   code at all — WebKit does the selection natively — so the [rvsel:] census
   would stay STALE from before the drag and a phase could only ever read "no
   selection". This listener is the republish, and it is deliberately narrow:
   the guard returns on the very first branch unless the selection's anchor is
   inside a `.preview`, so the EDITOR's keystroke path (every caret move fires
   selectionchange too) does exactly what it did before this goal — no title
   write, no layout read. When it does fire it is deferred+coalesced 30ms, the
   same shape Ed.census() uses, so the title write can never land inside a
   key_to_paint span. */
let rvSelT = null;
document.addEventListener("selectionchange", () => {
  const s = window.getSelection(), n = s && s.anchorNode;
  if (!n) return;
  const e = n.nodeType === 1 ? n : n.parentNode;
  if (!e || !e.closest || !e.closest(".preview")) return;
  if (rvSelT) return;
  rvSelT = setTimeout(() => { rvSelT = null; updateTitle(); }, 30);
});

/* R28 census (goal wsrestore) — registered in ui/census.js, not inline.
   [wstabs:<note>:<mode>,<note>:<mode>|<note>:<mode>] — EVERY tab of EVERY
   group, in paint order, with its own view mode; groups separated by "|".
   [mode:] publishes the FOCUSED tab only, so without this token "each tab's
   mode came back" is unobservable and a restore that flattened every reading
   tab into an editor would pass.
   [ws:<writes>,<restored>,<dropped>] — writes = write_workspace calls that
   RETURNED this process (R28.3 is a claim about the running app, so the
   census counts the act, not the intent); restored = leaves rebuilt from the
   file on this launch; dropped = leaves whose file was gone and which were
   dropped instead of failing the restore (R28.17). A degrade that silently
   did nothing and a degrade that dropped a leaf are different numbers. */
function wsTok() {
  return " [wstabs:" + groups().map(h => h.tabs.map(x =>
        (x.kind ? x.kind : titleOf(x.name)) + ":" + (x.kind ? "-" : (MODE_ABBR[x.mode] || "?"))).join(",")).join("|") + "]" +
       " [ws:" + wsWrites + "," + wsRestored + "," + wsDropped + "]" + wsUnkTok();
}
// W4: the kept-but-unviewable leaves, by stock type, in layout order; the
// right-sidebar leaf types as they will be written (only while a file supplied them)
function wsUnkTok() {
  const u = [];
  for (const h of groups()) for (const x of h.tabs) if (x.kind === "unk") u.push(x.utype);
  let s = u.length ? " [wsunk:" + u.join(",") + "]" : "";
  if (wsSideRaw.right) s += " [wsside:" + wsSideTabs(wsSideRaw.right).map(tn =>
    tn.children.map(l => (l && l.state && l.state.type) || "?").join(",")).join("|") + "]";
  return s;
}

// G2 (goal gatetrain): the registered census tokens from ui/census.js, in
// registration order. typeof guard: a page that failed to load census.js must
// still publish every inline token. A throwing token is named, not swallowed.
function censusToks() {
  if (typeof CENSUS === "undefined") return "";
  let s = "";
  for (let i = 0; i < CENSUS.length; i++) {
    try { s += CENSUS[i]() || ""; } catch (_) { s += " [cerr:" + i + "]"; }
  }
  return s;
}
/* collapseall R7 [fold:fe<collapsed>/<folders>:<C|E>,bm<collapsed>/<groups>:<C|E>]
   read off the DOM of BOTH panes (hidden panes keep their DOM), so a phase asserts
   fold state without pixels. C|E = what that pane's header toggle would do NOW
   (R4, M3): Collapse while anything is expanded, else Expand. A pane with NOTHING
   to fold has no state to derive it from, and stock then flips the label as a
   FLAG on every click (recon-collapseall Q5, 42/46/47): the explorer's flag
   starts on Expand (stock's folders start collapsed, 40-q5-fe-flat.png), the
   bookmarks' on Collapse (43-q5-bm-flat.png). Reset per vault (enterVault). */
let feCaFlag = "E", bmCaFlag = "C";
const feFoldRows = () => document.querySelectorAll("#tree .trow.folder");
const bmGroupRows = () => document.querySelectorAll("#bmlist .bmrow.bmgrp");
/* caperf C2b: the fold counts are EVENT-driven, not re-queried per updateTitle.
   caFold caches {fc,fn,bc,bn}; a MutationObserver on #tree and #bmlist (static
   elements, index.html) drops it on a child-list change or on a class flip that
   moves a row in/out of folder|open (explorer) or bmgrp|bmfold (bookmarks) —
   hover/selection class churn does not. Readers drain takeRecords() first, so a
   fold followed by updateTitle() in the SAME task (feCollapseAll) is never stale. */
let caFold = null, caObs = null;
const caCls = (s, a, b) => { const t = " " + (s || "") + " "; return (t.includes(" " + a + " ") ? 1 : 0) + (t.includes(" " + b + " ") ? 2 : 0); };
function caRel(m) {
  if (m.type === "childList") return true;
  if (m.attributeName !== "class") return false;
  const now = m.target.getAttribute("class"), inTree = !!m.target.closest("#tree");
  return inTree ? caCls(m.oldValue, "folder", "open") !== caCls(now, "folder", "open")
                : caCls(m.oldValue, "bmgrp", "bmfold") !== caCls(now, "bmgrp", "bmfold");
}
function caCounts() {
  if (!caObs) {
    caObs = new MutationObserver(ms => { if (caFold && ms.some(caRel)) caFold = null; });
    const o = { childList: true, subtree: true, attributes: true, attributeFilter: ["class"], attributeOldValue: true };
    for (const id of ["tree", "bmlist"]) { const el = $(id); if (el) caObs.observe(el, o); }
  } else if (caFold && caObs.takeRecords().some(caRel)) caFold = null;
  else caObs.takeRecords();
  if (!caFold) caFold = {
    fn: feFoldRows().length, fc: document.querySelectorAll("#tree .trow.folder:not(.open)").length,
    bn: bmGroupRows().length, bc: document.querySelectorAll("#bmlist .bmrow.bmgrp.bmfold").length,
  };
  return caFold;
}
function feCaState() {
  const k = caCounts(), n = k.fn, c = k.fc;
  return { c, n, lab: n ? (c === n ? "E" : "C") : feCaFlag };
}
function bmCaState() {
  const k = caCounts(), n = k.bn, c = k.bc;
  return { c, n, lab: n ? (c === n ? "E" : "C") : bmCaFlag };
}
function foldTok() {
  const f = feCaState(), b = bmCaState();
  return " [fold:fe" + f.c + "/" + f.n + ":" + f.lab + ",bm" + b.c + "/" + b.n + ":" + b.lab + "]";
}
/* collapseall R4: the header toggle's face. Title = the NEXT action (M3); glyph
   chevrons-IN (pointing at each other) when the next action is Collapse, chevrons-
   OUT when it is Expand (recon-collapseall Q3: a glyph flip, NO state highlight —
   the hover background every #bar button has is all stock paints). Written only
   when it changes: updateTitle runs this on every census tick. */
const CA_IN = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 20l5-5 5 5M7 4l5 5 5-5"/></svg>';
const CA_OUT = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M7 15l5 5 5-5M7 9l5-5 5 5"/></svg>';
function caFace(btn, lab) {
  if (!btn || btn.dataset.ca === lab) return;
  btn.dataset.ca = lab;
  btn.title = lab === "C" ? "Collapse all" : "Expand all";
  btn.innerHTML = lab === "C" ? CA_IN : CA_OUT;
}
/* [fecab:x0-x1,y] / [bmcab:x0-x1,y] — the toggle's PAINTED rect (x span, centre y),
   so a phase clicks what it measured (lessons/a-phase-must-not-own-a-layout.md).
   Only while the button paints: a hidden pane's button has a zero rect. */
function cabTok(id, name) {
  const b = $(id);
  if (!b) return "";
  const r = b.getBoundingClientRect();
  if (!r.width || !r.height) return "";
  return " [" + name + ":" + Math.round(r.left) + "-" + Math.round(r.right) + "," + Math.round(r.top + r.height / 2) + "]";
}
/* collapseall R4, explorer: any folder expanded -> collapse ALL (nested included:
   every folder path goes into `collapsed`, so re-opening a parent shows its
   children still shut, recon Q2 05-fe-open-projects.png); else expand ALL. Same
   zero-IPC DOM flip as a single folder click (renderNode), no tree rebuild. */
/* updateTitle hook: repaint both toggle faces from the live state, then publish
   their rects (after the face is set, so the rect is the one a click will hit). */
function caSync() {
  caFace($("fecabtn"), feCaState().lab);
  caFace($("bmcabtn"), bmCaState().lab);
  return cabTok("fecabtn", "fecab") + cabTok("bmcabtn", "bmcab");
}
function feCollapseAll() {
  const s = feCaState();
  return act("fe_foldall", { action: s.lab, folders: s.n, collapsed: s.c }, () => {
    if (!s.n) { feCaFlag = feCaFlag === "C" ? "E" : "C"; updateTitle(); return; }
    const shut = s.lab === "C";
    for (const row of feFoldRows()) {
      const full = row.dataset.folder;
      shut ? collapsed.add(full) : collapsed.delete(full);
      row.classList.toggle("open", !shut);
      const kids = row.nextElementSibling;
      if (kids && kids.classList.contains("tkids")) kids.classList.toggle("collapsed", shut);
    }
    updateTitle();
  });
}
/* collapseall R4 + R2, bookmarks: the same rule over the groups. View state only:
   no inv(), so bookmarks.json cannot move (the phase sha256s it around this). */
function bmCollapseAll() {
  if (bmRenaming !== null) return;
  const s = bmCaState();
  return act("bm_foldall", { action: s.lab, groups: s.n, collapsed: s.c }, () => {
    if (!s.n) { bmCaFlag = bmCaFlag === "C" ? "E" : "C"; updateTitle(); return; }
    if (s.lab === "C") for (const r of bmGroupRows()) bmFolds.add(r.dataset.bmk);
    else bmFolds.clear();
    renderBm();
  });
}
function updateTitle() {          // pane/focus census in the window title (headless probe)
  rTrack();                       // lgpanes: keep rLeaf current even with the right sidebar closed
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
    // R38.7/R38.34: [pin:<names>] = every PINNED tab, in layout order. Pin is
    // the one row in the tab menu whose effect is a tab-bar glyph, and a glyph
    // is not assertable headlessly — this token is, so the gate reads the
    // handler's state instead of OCRing the strip.
    const pn = [];
    for (const h of groups()) for (const t of h.tabs)
      if (t.pinned) pn.push(String(t.name).split("/").pop().replace(/[[\]|]/g, ""));
    if (pn.length) lg += " [pin:" + pn.join("|") + "]";
    // opentab: [tord:a|b*|c] = the FOCUSED group's tab strip left to right, * = active —
    // where a new tab lands (REQ-9) and a duplicate tab (REQ-3/8) become assertable headlessly.
    const tg = fg();
    if (tg && tg.tabs.length) lg += " [tord:" + tg.tabs.map((t, i) =>
      String(t.kind ? t.kind : t.name).split("/").pop().replace(/[[\]|*]/g, "") + (i === tg.active ? "*" : "")).join("|") + "]";
  }
  // R8.10: focused tab's view mode -> [mode:lp|src|read]; when the lp raw
  // row is active, [mode:lp:<l0>] exposes its block start line (headless probe)
  const ft = fg() && fg().active >= 0 ? fg().tabs[fg().active] : null;
  let md = ft && !ft.kind ? " [mode:" + (MODE_ABBR[ft.mode] || "?") : "";
  if (md && isLp(ft.mode) && fg().lpActive) md += ":" + fg().lpActive.l0;
  if (md) md += "]";
  if (lpMs >= 0) md += " [lp:" + lpMs + "]";     // perf: last lpRender ms
  /* R25.13 census — the ONE headless record of a search-result click, read by
     scripts/gate.sh phase srclick:
       [sc:<note>@<line>.<col>|hl:<ranges>|top:<scrollTop>]  (source / lp)
       [sc:<note>@read|hl:0|top:<preview scrollTop>]         (reading, R25.13k)
     plus [scm:<spans>/<ranges>] read from the LIVE DOM of the focused pane, so
     the phase can tell "a range is remembered" from "a range is painted": the
     decoration is what the user sees, and the offsets alone would let a broken
     painter report a green. scm is published whenever the view holds a set,
     which is how the lifetime rules (R25.13f) are asserted after a keystroke,
     an undo or a tab switch. */
  if (scInfo) md += " [" + scInfo + "]";
  if (fg() && scHits(fg()).length) {
    const sc = fg().lp;
    md += " [scm:" + (sc ? sc.querySelectorAll("span." + SC_MARK).length : -1) +
          "/" + scHits(fg()).length + "]";
  }
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
  /* rvcursor census -> [rvc:p=default|h1=default|wiki=pointer|wikiu=pointer|ext=pointer|
     tag=pointer|task=default:dis1|img=default|lprow=text] — the computed cursor
     SHAPE of every surface this goal names, read with getComputedStyle from the
     LIVE DOM of the focused group, never from the stylesheet text and never
     from a pixel. "-" = the surface is not in this note's DOM (so a phase can
     tell "wrong cursor" from "the note never rendered the element", which an
     absent-equals-pass token would hide). The reading-view surfaces come from
     g.preview (Rust's pulldown-cmark HTML) and `lprow` from g.lp, published in
     EVERY mode — the hidden view still computes a cursor, so one shot asserts
     both the fix and criterion 4's "live preview unchanged".
     WHY THIS IS NOT THE WHOLE PROOF: getComputedStyle returns the literal
     "auto" for the UA link fallback, so it cannot tell a hand from an I-beam
     (docs/recon-rvcursor/CLICKABLES.md H1/H3). The shape the X server actually
     draws is measured independently by the -draw_mouse grab (POINTER-GRAB.md).
     `task` also publishes the `disabled` attribute its inert verdict rests on
     (dis1/dis0), so that classification is measured here, not taken from a note. */
  if (md && fg()) {
    const rvg = fg(), rvp = rvg.preview, rvl = rvg.lp;
    const rvc = el => el ? String(getComputedStyle(el).cursor || "?").replace(/[[\]|=]/g, "").slice(0, 24) : "-";
    const rvq = (r, s) => (r ? r.querySelector(s) : null);
    const rvtb = rvq(rvp, "input[type=checkbox]");
    md += " [rvc:p=" + rvc(rvq(rvp, "p")) +
          "|h1=" + rvc(rvq(rvp, "h1")) +
          "|wiki=" + rvc(rvq(rvp, "a.wiki:not(.wiki-unresolved)")) +
          "|wikiu=" + rvc(rvq(rvp, "a.wiki-unresolved")) +
          "|ext=" + rvc(rvq(rvp, "a.ext")) +
          "|tag=" + rvc(rvq(rvp, "a.tag")) +
          "|task=" + rvc(rvtb) + (rvtb ? ":dis" + (rvtb.disabled ? 1 : 0) : "") +
          "|img=" + rvc(rvq(rvp, "img")) +
          "|lprow=" + rvc(rvq(rvl, ".lprow")) + "]";
    /* rvcursor criterion 3 -> [rvsel:<chars>|<x0>,<y>,<x1>] — the SELECTION
       CAPABILITY, which is a different question from the cursor SHAPE: stock
       ships the I-beam over reading-view prose, and the I-beam is the
       affordance that says "this text can be selected", so removing it is
       exactly the change that could quietly take selectability with it.
       <chars> = the length of the live DOM selection when BOTH its ends sit
       inside this pane's .preview (0 = nothing selected there, "-" = no
       preview in this view), read with window.getSelection() — never from
       CSS, which would only re-assert what the stylesheet already says.
       <x0>,<y>,<x1> = the first paragraph's first painted line box (range
       client rect, so generated content and hidden runs cannot move it),
       which is what the phase DRAGS along — a guessed pixel would make a
       failure mean "missed the text", not "cannot select". */
    /* ONE implementation of "where is this element painted", used by [rvsel:]
       and [rvlb:] alike: the FIRST painted line box of the element's contents
       (a range client rect — generated content and hidden runs cannot move it),
       as <x0>,<y-middle>,<x1> in viewport pixels, or "-" when it paints
       nothing. Two copies of this would be two chances to disagree about which
       pixel a driver is aiming at. */
    const rvbox = el => {
      if (!el) return "-";
      const rr = document.createRange(); rr.selectNodeContents(el);
      const rcs = rr.getClientRects(), q = rcs.length ? rcs[0] : el.getBoundingClientRect();
      return q.width ? Math.round(q.left) + "," + Math.round(q.top + q.height / 2) + "," + Math.round(q.right) : "-";
    };
    let rvsl = "-", rvsx = "-";
    if (rvp) {
      const ss = window.getSelection();
      rvsl = (ss && ss.rangeCount && ss.anchorNode && ss.focusNode &&
              rvp.contains(ss.anchorNode) && rvp.contains(ss.focusNode))
             ? String(ss.toString().length) : "0";
      rvsx = rvbox(rvq(rvp, "p"));
    }
    md += " [rvsel:" + rvsl + "|" + rvsx + "]";
    /* rvcursor criterion 2 -> [rvlb:wiki=<x0>,<y>,<x1>|p=<x0>,<y>,<x1>] — WHERE
       the two surfaces of the X-pointer measurement are painted. The computed
       style ([rvc:]) cannot see the shape the X server draws (getComputedStyle
       returns the literal "auto" for the UA link fallback, hand and I-beam
       alike), so the second, INDEPENDENT measurement grabs the screen with the
       pointer drawn over a link and over plain text — and it must warp to a
       pixel that is REALLY inside each of them. A guessed pixel would turn
       "wrong shape" and "missed the element" into the same result. `p` is the
       same box [rvsel:] publishes (same rvbox call), so a driver can read one
       token for both points. */
    md += " [rvlb:wiki=" + (rvp ? rvbox(rvq(rvp, "a.wiki:not(.wiki-unresolved)")) : "-") +
          "|p=" + (rvp ? rvsx : "-") + "]";
    /* goal/linebreak -> [rvab:<yA>,<yB>,<br>,<blk>] — the DOM measurement the
       `linebreak` phase asserts (docs/linebreak/recon.md, the same ruler the
       stock recon used): the painted row (Range client-rect top, px) of the
       FIRST "Alpha" and the first "Beta" in this pane's reading view ("-" =
       absent / not painted), the number of <br> in the view, and the first
       code/table/math block as <tag>:<height>:<br inside> ("-" = none).
       Reading view only, and the text walk is capped at 400 nodes, so a big
       note costs the census nothing it did not already pay. */
    if (rvp && isReading(rvg)) {
      const tw = document.createTreeWalker(rvp, NodeFilter.SHOW_TEXT);
      const ys = { Alpha: "-", Beta: "-" };
      for (let n, k = 0; (n = tw.nextNode()) && k < 400 && (ys.Alpha === "-" || ys.Beta === "-"); k++) {
        for (const w of ["Alpha", "Beta"]) {
          const i = ys[w] === "-" ? n.data.indexOf(w) : -1;
          if (i < 0) continue;
          const rr = document.createRange(); rr.setStart(n, i); rr.setEnd(n, i + w.length);
          // the WIDEST rect: WebKit hands a zero-width box at the END of the previous
          // line first when the word follows a "\n" inside a <pre> (measured: code
          // block, Alpha and Beta both reported on row 1)
          let q = null;
          for (const c of rr.getClientRects()) if (c.height && (!q || c.width > q.width)) q = c;
          if (q && q.width) ys[w] = Math.round(q.top);
        }
      }
      const bk = rvq(rvp, "pre, table, .math-display");
      const bh = bk ? bk.tagName.toLowerCase() + ":" + Math.round(bk.getBoundingClientRect().height) + ":" + bk.querySelectorAll("br").length : "-";
      md += " [rvab:" + ys.Alpha + "," + ys.Beta + "," + rvp.querySelectorAll("br").length + "," + bh + "]";
    }
  }
  // R26: the in-note find bar of the FOCUSED pane (open only) — see fTok.
  if (md && fg()) md += fTok(fg());
  // R31.9 drop probe: the LAST drop's outcome -> [drop:<copied>/<refused>].
  // Deliberately not derived from the banner (which times out): "no drop yet"
  // and "a drop whose banner faded" must not look the same to a probe.
  if (dropTok) md += " [drop:" + dropTok.replace(/[[\]|]/g, "") + "]";
  // R24.6 explorer drag-to-move: [dragt:<dest or - >:<the chip's own text>] while
  // a drag is up, [mv:<old>><new>/<files linking in>] for the last completed move.
  if (dragTok) md += " [dragt:" + dragTok.replace(/[[\]|]/g, "").slice(0, 120) + "]";
  if (mvTok) md += " [mv:" + mvTok.replace(/[[\]|]/g, "").slice(0, 120) + "]";
  md += " [zoom:" + zoomTok + "]" + qfsTok();   // fontwheel [qfs:] beside it. R36: always present — a probe must be able to read "still at 100%"
  // R15.2 font probe: bundled @font-face entries that actually LOADED (lazy: a face loads when text first uses it) -> [fonts:SourceCodePro/400/normal|...]
  { const fl = document.fonts ? [...document.fonts].filter(f => f.status === "loaded").map(f => f.family.replace(/[" ]/g, "") + "/" + f.weight + "/" + f.style) : [];
    if (fl.length) md += " [fonts:" + fl.join("|") + "]"; }
  let gg = ft && ft.kind === "gg" ? " [gg]" : "";  // R9.7: global graph tab focused
  if (gg) { const pt = posTok(fg()); if (pt) gg += " [ggpos:" + pt + "]"; }
  // GRAPH THEME census (goal graphtheme). [graphbg:]/[graphnode:] are a FRESH read of the
  // stylesheet off the root element — deliberately NOT the graph's own cached palette: the
  // defect under test is a cache that outlives a palette switch, and a token read from that
  // cache would agree with the stale pixels and pass. The phase compares the pixels the graph
  // painted against what the stylesheet says it should have painted. [gl:] names the draw path
  // the same shot came from (gl = ui/graph-gl.js, 2d = the Canvas 2D fallback).
  // BOTH GRAPHS, NOT JUST THE GLOBAL ONE (goal themeone item 11, criterion 4). The
  // block above only ever fired for a focused GLOBAL graph, so a phase asserting the
  // LOCAL graph's canvas had no token to compare its pixels against and no way to
  // know which draw path painted them. The tokens are document-level (the stylesheet
  // read) plus the FOCUSED GROUP's renderer, so publishing them for a focused `lg`
  // tab is the same read about the other canvas — nothing here is gg-specific.
  // [gcv:<kind>:<x>,<y>,<w>,<h>] is the canvas's MEASURED rect in window coordinates
  // (the window is undecorated at 0,0, so this is also the screenshot crop): a pixel
  // phase crops what the app says it painted instead of a hand-counted box. That is
  // the navbtn/phase_ux lesson — drive and measure the PUBLISHED body, never a
  // remembered coordinate. The rect is omitted while the canvas is hidden (no graph
  // is painting, so there is nothing to crop); the colour tokens are published
  // whenever a graph tab is focused, exactly as before for `gg`.
  let gpx = "";
  {
    const fkind = ft && ft.kind, fgr = fg();
    if (fkind === "gg" || fkind === "lg") {
      // READ WHERE THE THEME DECLARES (item 5). The colour tokens moved from
      // :root to body — a theme declares on body.theme-dark/.theme-light (T6
      // RESULT 3) and custom-property substitution happens on the element that
      // carries the declaration, so an <html> lookup sees our defaults and can
      // never see a theme's value. That is DESIGN §3 cause (1), arriving with
      // the declarations it follows; item 6 owns causes 2-4.
      const cs = getComputedStyle(document.body), tokv = n => cs.getPropertyValue(n).trim().toLowerCase();
      gpx = " [gl:" + ((fgr && fgr.graphRenderer) || "none") + "] [graphbg:" + tokv("--graph-bg") + "] [graphnode:" + tokv("--accent-blue") + "]";
      const cvEl = fgr && fgr.graph;
      if (cvEl && !cvEl.hidden) {
        const r = cvEl.getBoundingClientRect();
        gpx += " [gcv:" + fkind + ":" + Math.round(r.left) + "," + Math.round(r.top) +
               "," + Math.round(r.width) + "," + Math.round(r.height) + "]";
      }
    }
  }
  const tokq = s => String(s == null ? "" : s).replace(/[[\]|]/g, "").slice(0, 80);
  const modal = modalKind ? " [modal:" + modalKind + "]" +
                            (mdNew ? " [mdnew:" + tokq(mdNew) + "]" : "")   // C4: the create-this-note affordance is on screen
    : ($("rnbox") && !$("rnbox").hidden ? " [modal:rn]" : "")  // m5 fuzzy modal / rename prompt
    + ($("anew") && !$("anew").hidden ? " [modal:att]" : "")   // R31.7 Insert attachment prompt
    + (settingsOpen ? " [modal:settings]" + setTok() + hkInfo : "")   // R14 hotkeys + R30 settings probe
    + (ulPending ? " [modal:ul]" + ulTok() : "")                // R34.6 Update links prompt
    + (delPending ? " [modal:del]" + delTok() : "")             // R24.7 delete confirmation
    + (bmEdit ? " [modal:bmedit]" + bmEditTok() : "")           // R4X.7 Edit bookmark (the group MOVE route)
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
  // R1 (themecsp): stock Obsidian marks the mode as a CLASS on <body>
  // (.theme-dark / .theme-light); T6 measured a stock-shaped rule keys off
  // body.theme-dark, which our :root[data-theme] shadows. We emit BOTH so
  // stock-shaped CSS can match. This token reads the body class back OFF THE
  // DOM (not off themeMode) so a phase can assert the marker stock CSS sees.
  const bodyCls = document.body ? document.body.classList : null;
  const thmTok = " [thm:" + (bodyCls && bodyCls.contains("theme-dark") ? "dark"
                          : bodyCls && bodyCls.contains("theme-light") ? "light" : "unset") + "]";
  // THE [palette:] TOKEN IS GONE (themeone item 7/8). It published
  // documentElement's data-palette attribute — the second theme axis, which
  // this goal deletes. What a phase wants to know now ("which theme is
  // painting") is [vtheme:] below, and it is a STRICTLY better token: it reads
  // the name off the injected <style> element, so it says what is painting
  // rather than what an attribute wishes were painting.
  // themecsp crit 5: with the rig applicator active (OPENSIDIAN_SMOKE_CSS), publish
  // body's COMPUTED background-color so a phase can prove a stock-shaped rule
  // targeting body.theme-dark won a pixel (getComputedStyle before/after). Off
  // (empty) on every normal run, so no pre-existing phase's census changes.
  const thmpxTok = (smokeCssOn && document.body)
    ? " [thmpx:" + getComputedStyle(document.body).backgroundColor.replace(/\s+/g, "") + "]"
    : "";
  // R4X.4 (navbtn) census [nvb:bXfX] — the app's DECIDED button state for the
  // focused pane (1 = enabled), from the same navState the buttons paint from.
  // Published on every title write, so it moves with nav, tab switch and
  // focus change; a phase asserts the DECISION, never pixels.
  navBtnSync();
  const nv = navState(fg());
  const navTok = " [nvb:b" + (nv.b ? 1 : 0) + "f" + (nv.f ? 1 : 0) + "]";
  // R4X.4b [navg:<back cx>,<fwd cx>,<cy>] — the PAINTED centres of the focused
  // pane's nav buttons, same idea as [mg:]/[bmg:]: the phase clicks MEASURED
  // geometry, never a guessed pixel. Empty only if the pair is not painted.
  let navgTok = "";
  const nfg = fg();
  if (nfg && nfg.navback) {
    const nbr = nfg.navback.getBoundingClientRect(), nfr = nfg.navfwd.getBoundingClientRect();
    if (nbr.width) navgTok = " [navg:" + Math.round(nbr.left + nbr.width / 2) + "," +
      Math.round(nfr.left + nfr.width / 2) + "," + Math.round(nbr.top + nbr.height / 2) + "]";
  }
  // themefs R3: how many snippet <style> elements are ACTUALLY in the head —
  // counted off the DOM (snipEls maps label -> live element), not off the
  // enabled array, so an injection that failed loudly is not counted as on.
  const snipTok = " [snips:" + snipEls.size + "]";
  // themefs R5: the active VAULT theme — the name only when its <style> is
  // really in the head (a refused/unlisted cssTheme reads "none": the census
  // reports what is painting, not what the config wishes were)
  const vthemeTok = " [vtheme:" +
    (document.getElementById("vault-theme") && vaultTheme
      ? String(vaultTheme).replace(/[[\]|]/g, "").slice(0, 40) : "none") + "]";
  // themefs item 6: hot reloads APPLIED this session — the phase's latency
  // clock (edit the file, poll the title until this bumps, subtract; the
  // pixels are then proved separately with getComputedStyle/thmpx).
  const creloadTok = " [creload:" + vaultCssReloads + "]";
  // themefs item 8: alias rows the R4 bridge is painting (0 = no #vault-bridge
  // element — no theme, or a theme declaring none of the aliased stock names)
  const vbridgeTok = " [vbridge:" + vaultBridgeAliases + "]";
  // themeone item 6 (R5 causes 3+4): the ROOT ATTRIBUTE the graph's cache key
  // and its MutationObserver both read — "<generation>:<name>", "-" before the
  // first theme decision of the session. Deliberately a SEPARATE token from
  // [vtheme:]: that one reads the <style> element (what is painting), this one
  // reads documentElement.dataset.vtheme (what the graph was TOLD). A phase
  // asserting the repaint trigger needs the second, and the two disagreeing is
  // itself the bug — a theme painting whose generation never reached the root
  // is exactly the silent staleness cause 4 describes.
  const vtgTok = " [vtg:" + (document.documentElement.dataset.vtheme || "-") + "]";
  // themeone item 4 (C3): the boot's seeding decision — w<wrote> k<kept>
  // f<failed>. Always on, like [snips:]/[vtheme:]: a fresh vault must read
  // w3k0f0 and the NEXT boot on the same vault k3, which is legs 1 and 2 of
  // criterion 3 readable off the title.
  const bseedTok = " [bseed:" + vaultSeedTok + "]";
  // themefs item 10, MIGRATED by themeone item 14: the phase's
  // getComputedStyle surface — five COMPUTED chrome backgrounds, in three
  // classes, so one instrument answers both halves of "how far does a vault
  // theme reach":
  //   1-3  body / #side / #wframe — the R4 ALIAS BRIDGE's points
  //        (--bg-base / --bg-sidebar / --titlebar-bg): a stock name the
  //        BRIDGE_ALIASES table names, emitted as a var() row.
  //   4    #bar button (--bg-surface) — the INVERSION's point. No bridge row
  //        aliases it and none ever will: since item 5, style.css declares
  //        `--bg-surface: var(--background-secondary-alt)` on BODY (layer 2),
  //        so a theme declaring that stock name on body.theme-dark wins the
  //        cascade on the same element and reaches our chrome with no table
  //        in the path. This field was "must NOT move" before item 5; it is
  //        "must move" now, which is the whole R4 thesis as a pixel.
  //   5    #mbox (--bg-elevated) — LAYER 3, ours alone: no stock name sits
  //        behind it (style.css ships the literal), so neither route can
  //        carry a theme there. The containment half of the old field-4
  //        assert lives here now. #mbox is static in index.html under
  //        #modal[hidden], and getComputedStyle resolves custom properties
  //        on a display:none element, so it reads without opening the modal.
  // Always on, like [snips:]/[vtheme:]: a probe cannot pass by setting a
  // variable — these are resolved pixels off the live cascade.
  const vpxBg = el => el ? getComputedStyle(el).backgroundColor.replace(/\s+/g, "") : "-";
  const vpxTok = document.body
    ? " [vpx:" + vpxBg(document.body) + "/" + vpxBg(document.getElementById("side")) +
      "/" + vpxBg(document.getElementById("wframe")) + "/" + vpxBg(document.querySelector("#bar button")) +
      "/" + vpxBg(document.getElementById("mbox")) + "]"
    : "";
  let t = "opensidian [panes:" + ps.length + " focused:" + nf +
            "@" + (ps.indexOf(fg() && fg().pane) + 1) + "] [fx:" + fx + "]" +
            " [tabs:" + groups().map(g => g.tabs.length).join(",") + "]" + noteTok + themeTok + thmTok + thmpxTok + snipTok + vthemeTok + creloadTok + vbridgeTok + vtgTok + bseedTok + vpxTok + navTok + navgTok + lg + md + gg + gpx + modal +
            " [side:l" + (sideOpen ? 1 : 0) + "r" + (rightOpen ? 1 : 0) +
            (rightOpen ? ":" + rTab : "") + "]" +
            (rightOpen && rpInfo ? " [rp:" + rpInfo + "]" : "") +
            (rightOpen ? " [rpnote:" + rpNote + "]" : "") +      // lgpanes: the note the right panes render (- = none)
            " [rpn:" + rbN + "]" +                              // F1: backlink repaints ENTERED, so a skipped one is observable
            (rtInfo ? " [" + rtInfo + "]" : "") +
            (rightOpen ? " [stx:" + stabCentres() + "]" : "") +   // R33.13: the strip MOVED these — smoke reads them, never guesses
            (jsErr ? " [jserr:" + jsErr + "]" : "") +
            (saveErr ? " [saveerr:" + saveErr + "]" : "") +             // F1: a save that did not land
            " [armed:" + groups().filter(h => h.saveT).length + "]" +   // F2: groups holding a live save timer
            menuTok() +
            (navInfo ? " [" + navInfo + "]" : "") +
            (revealInfo ? " [bmrv:" + revealInfo + "]" : "") +      // bmmenu: "Reveal file in navigation" ran (bmReveal) — not merely "the Files pane is showing"
            (acItems.length ? " [ac:" + acKind + ":" + acItems.length + "]" : "") +
            " [pane:" + sidePane + "]" +
            sfontTok() +                                        // sidefont: computed sidebar row font sizes (t=trow b=bmrow r=rlist)
            (sidePane === "search" && searchCount >= 0 ? " [sr:" + searchCount + "]" + srGeom() : "") +
            (sidePane === "bm" ? " [bm:" + bmRows() + "]" +          // RENDERED rows, not the model's length:
              " [bmn:" + bmNames() + "]" +                          // and their painted LABELS, in paint order
              bmGeom() +                                            // bmmenu: [bmg:x,y,pitch] of the painted rows — a driver right-clicks what it measured
              " [bmt:" + bmShape() + "]" +                          // R4X.5: the painted TREE SHAPE, parallel to [bmn:]
              bmIndentTok() +                                       // R4X.6: the painted indent STEP in px
              bmRenTok() +                                          // R4X.8: an inline group rename is OPEN and not yet committed
              (bmDropTok ? " [bmdrop:" + bmDropTok.replace(/[[\]|]/g, "") + "]" : "") +   // bmdrag: the app-computed drop target, live only mid-drag
              (bmRows() === bmTree.length ? "" :                     // the smoke assertion must prove the PANE
               " [bmdesync:" + bmTree.length + "/" + bmRows() + "]") : "");   // repainted, not just the model

  // R17: ONE model->text join per title publish, shared by [buf:] and F1's
  // [dirty:] — the token must not put a second full join on the typing path.
  const fb = fg() && fg().active >= 0 && !fg().tabs[fg().active].kind ? bufOf(fg()) : null;
  const t2 = (fb !== null ? " [buf:" + fb.length + "]" + dirtyTok(fb) : "") +
             " [tree:" + notesCache.length + "] [vc:" + vcCount + "]" +   // R11 probes
             tgTok() +                                                    // tabclose: why every tab went
             ucTok() +                                                    // tabclose: the undo-close RESCUE buffer (depth/with-bytes)
             tcxTok() +                                                   // tabclose S4: the close glyph's PAINTED centres
             (extCount ? " [ext:" + extCount + "]" : "");                 // S1: external-link clicks routed to open_external
  t += t2;
  const ov = ovfScan();            // R22: layout overflow census (window + frame)
  t += " [vp:" + innerWidth + "x" + innerHeight + "]" +   // resize-completed signal for the fuzz harness
       " [ovf:" + ov.dw + "," + ov.dh + "," + ov.n + "]" +
       (ov.bad.length ? " [ovfe:" + ov.bad.join("|").slice(0, 180) + "]" : "");
  t += wfTok();                    // R33: the window's own frame (controls, grips, maximised, keyboard focus)
  t += censusToks();               // G2: registered tokens (ui/census.js) — new tokens go THERE, not on the lines above
  document.title = t;
  // publish to the native title: ONE call in flight, last-write-wins, 500ms
  // timeout guard — a hung/rejected setTitle IPC can neither reorder titles
  // nor starve later updates (the old promise-chain stalled forever on one)
  pushTitle(t);
  wsTouch();   // R28.3: every mutation already funnels through here — arm the layout write
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
  if (src && src.kind === "unk") {         // W4: a kept leaf duplicates as itself under a NEW id (two leaves never share one)
    const id = wsId(), raw = wsClone(src.raw);
    raw.id = id; delete raw.group;
    return splitWith(g, dir, { kind: "unk", name: src.name, utype: src.utype, raw, lid: id, mode: "source", hist: [], hpos: -1 });
  }
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

async function collapseGroup(g, via) {  // R6.5: closing the last tab removes the group
  const parent = findParent(state.root, g);
  if (!parent) return;                        // lone root group: caller keeps it
  if (g.sim) cancelAnimationFrame(g.sim);     // stop the removed group's machinery
  // tabclose: the SECOND unflushed path. This clears an armed save exactly like
  // dropTab does, and it is reachable with tabs still in the group (a drag that
  // moves the last tab out, :1837/:1844) — so the record is unconditional and
  // names the pane, not a tab. dirty is stated only when the group still owns
  // its active tab; a group emptied by a move no longer holds the buffer.
  const at = g.active >= 0 && g.tabs[g.active] ? g.tabs[g.active] : null;
  // criterion 4: a pane that still owns a dirty tab FLUSHES before it goes.
  // Unlike dropTab's path the file is still on disk here, so the honest rescue
  // is the write itself — the timer below then clears nothing that mattered.
  // tabclose: dirty is read from the BUFFER (bufDirty), not from `g.saveT`. A
  // pane can hold unsaved bytes with no timer armed (the undo-close restore),
  // and "no timer" said clean while the bytes were still only in RAM.
  const atDirty = !!at && bufDirty(g);
  const risked = atDirty ? bufOf(g) : null;
  let atFlushed = false;
  if (atDirty) { try { atFlushed = await flushSave(g); } catch (_) { atFlushed = false; } }
  // a write that did not land (F1 banner, or a vanished file) still must not
  // cost the keystrokes: park them where Ctrl+Shift+T can return them
  if (atDirty && !atFlushed && !at.kind) ucPush(at.name, risked, "user-close");
  tabGone("pane-collapse", at || ("pane#" + (g.id == null ? "?" : g.id)),
          { dirty: atDirty, flushed: atFlushed, preserved: atDirty && !atFlushed, via: via || "collapseGroup", tabsLeft: g.tabs.length });
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
  // bmmenu: a separator child (div.sep) reads as "---", so the census spells the list the way
  // docs/recon-bmmenu/README.md does; no other menu has one, so their tokens are unchanged.
  const k = menuEl.children, lbl = [...k].map(d => d.classList.contains("sep") ? "---" : d.textContent).join("|");   // hardcoded padding/line-height guess
  // [mt:<kind>:<label>] — WHICH tab this menu belongs to (R20.8). Without it an assertion like
  // "a graph tab offers no Bookmark" rests on the driver's guess that x=380 hit the gg tab: hit
  // the wrong tab and the claim is about a tab nobody named. tabMenu stamps the target it was
  // handed by the browser's hit test, so the census reports the tab that was ACTUALLY clicked.
  const mt = menuEl.dataset.mt ? " [mt:" + menuEl.dataset.mt + "]" : "";
  // bmmenu: [mdis:i|j] = 1-based indices (in [menu:] order) of the rows shown DISABLED —
  // the label list alone cannot tell a wired row from a stated-reason stub.
  const dis = [...k].map((d, i) => d.classList.contains("dis") ? i + 1 : 0).filter(Boolean);
  const md = dis.length ? " [mdis:" + dis.join("|") + "]" : "";
  if (!k.length) return " [menu:" + lbl + "]" + mt + md;
  const a = k[0].getBoundingClientRect();
  const pitch = k.length > 1 ? k[1].getBoundingClientRect().top - a.top : a.height;
  // bmmenu: with separators the rows are NOT one pitch apart, so a menu that has one also
  // publishes every child's centre y — [mgy:y1|y2|…] in [menu:] order (separators included, so
  // the index a driver found in [menu:] is the index it clicks). menu_click prefers it when present.
  const mgy = [...k].some(d => d.classList.contains("sep"))
    ? " [mgy:" + [...k].map(d => { const b = d.getBoundingClientRect(); return Math.round(b.top + b.height / 2); }).join("|") + "]" : "";
  return " [menu:" + lbl + "] [mg:" + Math.round(menuEl.getBoundingClientRect().left) + "," +
         Math.round(a.top + a.height / 2) + "," + Math.round(pitch) + "]" + mt + md + mgy;
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
  /* R38: stock's note-tab menu is 28 rows + 8 separators ≈ 820 px, TALLER than
     the 700 px smoke window, so .ctxmenu's overflow:auto scrolls it and the
     bottom rows (Reveal file in navigation, Delete file) sit below the viewport
     until it does. [mgy:] is measured from the live rects, so it stays true —
     but only if the census is REFRESHED when the menu scrolls, which a scroll
     alone does not do. Without this the geometry a driver clicks is the
     geometry from before its own wheel event. */
  m.addEventListener("scroll", () => updateTitle());
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

const DIS_WIN = "Missing subsystem: a second OS window — single-window tauri app, and no backend command creates one";
/* ---------- R38: THE TAB CONTEXT MENU — stock 1.13.7's list, measured ----------
   THE SPEC is docs/requirements.md §36 (R38) + docs/recon-tabmenu/README.md's
   WIRED / NOT WIRED split: 28 rows in 9 groups on an ACTIVE NOTE tab, 11 on a
   graph tab, and three ABSENCE rules (R38.31) — stock's grammar for an
   inapplicable row is that the row is NOT THERE (R38.2: across 12 measured
   menus not one row was greyed), so our disabled treatment is reserved for the
   rows we cannot WIRE, each carrying its missing subsystem in the hover title
   (the rule bmRowMenu already established). Nothing below is from memory: the
   labels, the group boundaries and every effect were measured on the box
   against obsidian.AppImage sha256 e0d8e0a6…72663, one fresh stock instance per
   performed row (docs/recon-tabmenu/effects/, shots/eff-*).
   SUPERSEDES the six-item menu (Split right first) and, with it, R12.4's
   "Live preview / Source mode" pair: stock has NO Live preview row, so the ✓
   moves out of the label TEXT into a span.chk marker element and the rendered
   label set equals stock's character for character (R38.10). */
const ICON_CHK = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2"' +
  ' stroke-linecap="round" stroke-linejoin="round"><path d="M3 8.5l3.5 3.5L13 5"/></svg>';
const ICON_PIN = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"' +
  ' stroke-linecap="round" stroke-linejoin="round"><path d="M6 2h4l-.6 4.2 2.6 2.4H3.9l2.7-2.4z"/><path d="M8 8.6V14"/></svg>';
/* The close verbs, R38.3-R38.6. EVERY one of them goes through the existing
   flush-then-close path (closeTab -> flushSave), and each also flushes ONCE up
   front because the doomed set may contain the tab whose buffer is pending
   while the right-clicked target is a different one — R38.32, the data-loss
   rule, is the reason these are three functions and not three inline loops. */
async function closeOtherTabs(g, i) {      // Close others: the TARGET survives and takes focus (measured: not the active tab)
  await flushSave(g);
  const keep = g.tabs[i];
  for (let j = g.tabs.length - 1; j >= 0; j--) if (g.tabs[j] !== keep) await closeTab(g, j);
  const k = g.tabs.indexOf(keep);
  if (k >= 0 && g.active !== k) { g.active = k; await loadActive(g); }
}
async function closeTabsAfter(g, i) {      // Close tabs after: target + everything to its LEFT kept (5 -> 2 measured)
  await flushSave(g);
  const keep = g.tabs[i];
  for (let j = g.tabs.length - 1; j > i; j--) await closeTab(g, j);
  const k = g.tabs.indexOf(keep);
  // focus: the target if the active tab was one of the doomed ones (stock's
  // survivor rule). A surviving active tab LEFT of the target keeps focus —
  // stock's behaviour in that case is UNMEASURED and is not guessed here.
  if (k >= 0 && g.active < 0) { g.active = k; await loadActive(g); }
}
async function closeAllTabs(g) {           // Close all: the GROUP SURVIVES (collapseGroup must not run)
  await flushSave(g);
  while (g.tabs.length > 1) await closeTab(g, g.tabs.length - 1);
  if (!g.tabs.length) return;
  /* The last tab cannot go through closeTab: with more than one group that path
     is R6.5's "empty group leaves the tree" and the group would be REMOVED,
     which is exactly what stock does not do (R38.6). Same teardown, no collapse.
     DEVIATION, stated: stock leaves an empty leaf whose tab reads `New tab`; we
     have no empty-tab entity (a tab is a note name), so the group survives with
     ZERO tabs — observably [tabs:0] with the pane still there, one tab row
     fewer than stock. docs/recon-tabmenu/README.md records it. */
  const t = g.tabs[0];
  await act("pane_close", { note: t.name, kind: t.kind || "note", pane_removed: false, groups: groups().length, tabs: 0 }, async () => {
    if (!t.kind) closedTabs.push(t.name);  // R14: undo close tab still works on the last one
    unlinkTab(g, t, true);
    dropView(t);
    g.tabs.length = 0;
    g.active = -1;
    await loadActive(g);
  });
}
function tabMenu(e, g, i) {
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
  /* item(label, fn) wires a row; item(label, null, why) renders it DISABLED with
     `why` (the missing subsystem) as the hover title; chk adds the radio marker
     as an ELEMENT so textContent — and therefore [menu:] — stays stock's label. */
  const item = (label, fn, why, chk) => {
    const d = document.createElement("div");
    if (chk !== undefined) {             // a RADIO row: the gutter is reserved whether or not this one is the active mode
      d.className = "chkrow";
      const s = document.createElement("span");
      s.className = "chk";
      if (chk) s.innerHTML = ICON_CHK;
      d.appendChild(s);
    }
    d.appendChild(document.createTextNode(label));
    if (why != null) { d.className = (d.className ? d.className + " " : "") + "dis"; d.setAttribute("aria-disabled", "true"); d.title = why; }
    else { d.onmousedown = ev => ev.stopPropagation(); d.onclick = () => { if (fn) fn(); else closeMenu(); }; }
    m.appendChild(d);
    return d;
  };
  const sep = () => { const d = document.createElement("div"); d.className = "sep"; m.appendChild(d); };
  const inPlace = fill => {              // the ctxmenu's ONE submenu seam: rebuild in place (R13.1's pick list)
    m.innerHTML = "";
    fill();
    setTimeout(() => clampMenu(m), 0);   // R22: the new list resizes the menu — re-clamp it
    updateTitle();                       // the menu was rebuilt in place -> refresh [menu:]
  };
  const pick = () => inPlace(() => {     // R13.1/R38.8: every other open tab, in layout order
    let any = false;
    for (const h of groups()) for (const t of h.tabs) {
      if (t === tab) continue;
      any = true;
      item((t.kind === "gg" ? "Graph" : t.name.split("/").pop()), () => { closeMenu(); linkTabs(tab, t); });
    }
    if (!any) item("(no other tabs)");
  });
  const pickFolder = async () => {       // R38.16: stock opens a modal folder SUGGESTER; ours is the same in-place list
    let fl = [];
    try { fl = await inv("list_folders"); } catch (err) { fl = []; }
    const here = tab.name.includes("/") ? tab.name.slice(0, tab.name.lastIndexOf("/")) : "";
    inPlace(() => {
      if (here) item("(vault root)", () => { closeMenu(); moveNoteTo(tab.name, ""); });
      for (const f of fl) if (f !== here) item(f, () => { closeMenu(); moveNoteTo(tab.name, f); });
      if (!m.children.length) item("(no other folder)");
    });
  };
  // ---- group 1, the close group. R38.31: the SET is the absence rule ----
  const n = g.tabs.length, rightmost = i === n - 1;
  item("Close", () => { closeMenu(); closeTab(g, i); });                        // WIRED: closeTab — flushes, focus to the NEXT tab (R38.3)
  if (n > 1) {
    item("Close others", () => { closeMenu(); closeOtherTabs(g, i); });         // WIRED (R38.4)
    if (!rightmost) item("Close tabs after", () => { closeMenu(); closeTabsAfter(g, i); });   // WIRED, absent when rightmost (R38.5)
    item("Close all", () => { closeMenu(); closeAllTabs(g); });                 // WIRED (R38.6)
  }
  /* R38.31, the first absence rule: an INACTIVE tab's menu is the close group
     and NOTHING else (M5/M8, 4 rows not 28). Measured twice, and it is why
     every row below may assume the target IS this group's active tab — which is
     what lets Reading view / Find... / Rename... keep taking the group. */
  if (i !== g.active) { placeMenu(m, e.clientX, e.clientY); return; }
  sep();
  const pinRow = () => item(tab.pinned ? "Unpin" : "Pin", () => { closeMenu(); togglePin(g, tab); });   // WIRED (R38.7)
  const linkRow = () => isLinked(g, tab, i)
    ? item("Unlink tab", () => { closeMenu(); unlinkTab(g, tab); })             // WIRED: R6.8, unchanged — stock wording in a stock slot
    : item("Link with tab...", pick);
  if (tab.kind) {
    /* ---- R38.30: a GRAPH tab gets a DIFFERENT menu, assembled from the view
       type — 11 rows with a second tab in the group, 8 alone. It drops every
       row that needs a file behind the tab and adds Copy screenshot. ---- */
    pinRow(); linkRow();
    sep();
    item("Move to new window", null, DIS_WIN);
    item("Split right", () => { closeMenu(); splitGroup(g, "row", i); });
    item("Split down",  () => { closeMenu(); splitGroup(g, "col", i); });
    sep();
    item("Copy screenshot", null, "Missing subsystem: canvas-to-clipboard — no programmatic clipboard write exists in this tree (ui/editor.js writes only inside real copy events) and the graph view has no canvas-to-PNG step");
    item("Bookmark...", null, "Missing subsystem: bookmarks of non-file views — a bookmark here is a note name on disk (R9.4) and a graph tab has no note behind it");
    placeMenu(m, e.clientX, e.clientY);
    return;
  }
  // ---- group 2 (5 rows) ----
  pinRow();
  linkRow();
  item("Backlinks in document", null, "Missing subsystem: an in-document backlinks section — stock appends backlinks to the BOTTOM OF THE NOTE PANE; ours is a right-sidebar pane (R27), a different surface");
  item("Reading view", () => { closeMenu(); setMode(g, "reading"); }, null, tab.mode === "reading");     // WIRED: setMode (R38.10)
  item("Source mode",  () => { closeMenu(); setMode(g, "source"); },  null, tab.mode === "source");      // WIRED: setMode — the ✓ is the R12.4 radio, now a marker element
  // ---- group 3 (4 rows) ----
  sep();
  item("Move to new window", null, DIS_WIN);
  item("Split right", () => { closeMenu(); splitGroup(g, "row", i); });         // WIRED: R6.2 — row 11 now, not row 1 (R38.12)
  item("Split down",  () => { closeMenu(); splitGroup(g, "col", i); });         // WIRED: R6.2 (R38.13)
  item("Open in new window", null, DIS_WIN + " — and this row COPIES the tab where the one above MOVES it (measured), so the two stay distinct");
  // ---- group 4 (6 rows) ----
  sep();
  item("Rename...", () => { closeMenu(); focusGroup(g); cmdRename(); });        // WIRED: R24.2/R34 rename + the prompted link update
  item("Move file to...", pickFolder);                                          // WIRED: moveNoteTo behind the in-place folder list (R38.16)
  item(bmCache.includes(tab.name) ? "Remove bookmark" : "Bookmark...",          // WIRED: toggleBm — stock's wording, our flat-list toggle (R38.17/R20.4)
    () => { closeMenu(); toggleBm(tab.name); });
  item("Merge entire file with...", null, "Missing subsystem: file merge — no command concatenates one note into another, and stock's is a suggester with four modifier behaviours");
  item("Add file property", null, "Missing subsystem: a frontmatter property model — stock writes a Properties block at the top of the note; there is no frontmatter parser or editor here");
  item("Export to PDF...", null, "Missing subsystem: a PDF pipeline — no renderer and no page-size/margin model; PDF export is an explicit project non-goal");
  // ---- group 5 (2 rows) ----
  sep();
  item("Find...",    () => { closeMenu(); fOpen(g); });                         // WIRED: R26 find bar
  item("Replace...", () => { closeMenu(); fOpenRep(g); });                      // WIRED: R26 replace row
  // ---- group 6 (1 row) ----
  sep();
  item("Copy path", null, "Missing subsystems: submenu panels in ctxmenu (stock keeps the parent menu OPEN beside a child panel; ours can only rebuild itself in place) and a programmatic clipboard write (the only clipboard access in the tree is inside real copy/paste events, ui/editor.js)");
  // ---- group 7 (2 rows) ----
  sep();
  item("Open version history", null, "Missing subsystem: file history — nothing here keeps a revision of a note to show changes from or restore");
  item("Open linked view", null, "Missing subsystem: submenu panels in ctxmenu — four of stock's five children already have backends here (R7 local graph, backlinks, outgoing, outline), so this is the highest-value row in this list");
  // ---- group 8 (3 rows) ----
  sep();
  item("Open in default app", null, "Missing subsystem: a file-path opener — open_external takes a URL, not a vault path");
  item("Show in system explorer", null, "Missing subsystem: a file-manager handler — no command reveals a path in a file manager");
  item("Reveal file in navigation", () => { closeMenu(); bmReveal(tab.name); });  // WIRED: bmReveal — the identical verb bmRowMenu wires, publishes [bmrv:] (R38.28)
  // ---- group 9 (1 row) ----
  sep();
  item("Delete file", () => { closeMenu(); askDelete(tab.name); }).className = "del";   // WIRED: askDelete — stock's row is the link-COUNTING confirmation, not an unlink (R38.29)
  placeMenu(m, e.clientX, e.clientY);   /* R22: viewport-clamped by MEASURED size */
}
async function togglePin(g, tab) {      // R38.7: the flag + the tab-bar glyph; Close stays enabled on a pinned tab (measured)
  tab.pinned = !tab.pinned;
  renderTabs(g);
  updateTitle();                        // census [pin:<names>]
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
  if (typeof Ed !== "undefined" && Ed.rvEnd) Ed.rvEnd(g);   // rvtask R3: a mode change closes the reading-view undo step
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
  fModeSwitched(g);   // R26.21/R26.23: an open find bar re-shapes and re-scans for the new surface
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
    // R38.7: a PINNED tab carries a pin glyph in FRONT of the label, the way
    // stock paints it (shots/m3-pinned-tabbar.png). It is a marker element, not
    // text, so [tabs:]/[note:] and every label assertion are unchanged.
    if (tab.pinned) { const p = document.createElement("span"); p.className = "pin"; p.innerHTML = ICON_PIN; d.appendChild(p); }
    ttl.textContent = (chain ? "\u{1F517} " : "") + tab.name.split("/").pop();
    const x = document.createElement("span");
    x.className = "x";
    // ux-1: inline svg cross — the ✕ glyph tofu'd under webkit2gtk
    x.innerHTML = '<svg viewBox="0 0 10 10"><path d="M1 1l8 8M9 1l-8 8" ' +
      'stroke="currentColor" stroke-width="1.4" stroke-linecap="round" fill="none"/></svg>';
    x.onclick = e => { e.stopPropagation(); closeTab(g, i, "user-close", "close-glyph"); };
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
  let ghost = null, target = null, hl = null, zones = null, raf = 0, last = null, rsR = null;
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
      // R20.3 (operator 2026-09-30, wsrestore): the right sidebar is content panes ONLY.
      // No tab is ever accepted there — a drop over #rside (its strip included) is
      // refused, the tab stays where it was. Explicit, not an accident of zones[].
      const rs = $("rside");
      rsR = rs && !rs.hidden ? rs.getBoundingClientRect() : null;
    }
    ghost.style.transform = "translate3d(" + (ev.clientX + 10) + "px," + (ev.clientY + 12) + "px,0)";
    let nt = null;
    const overSide = rsR && ev.clientX >= rsR.left && ev.clientX <= rsR.right &&
                     ev.clientY >= rsR.top && ev.clientY <= rsR.bottom;
    for (const z of overSide ? [] : zones) {
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
  // A tab drag is OUR gesture, never a text selection: WebKit otherwise extends a
  // selection from the press across whatever the pointer crosses, and a drop that
  // moves nothing (refused over #rside, or nowhere) leaves it behind — the NEXT
  // press on a tab then starts a native selection DnD that swallows every
  // mousemove (measured on the box, rside drag positive control, 2026-09-30).
  const noSel = ev => ev.preventDefault();
  window.addEventListener("selectstart", noSel, true);
  const move = ev => { last = ev; if (!raf) raf = requestAnimationFrame(step); };
  const up = async () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    window.removeEventListener("selectstart", noSel, true);
    if (raf) { cancelAnimationFrame(raf); raf = 0; if (last) step(); }
    const t = target;
    if (ghost) ghost.remove();
    clearHl();
    if (ghost) { const s = getSelection(); if (s && !s.isCollapsed) s.removeAllRanges(); }
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
      if (!g.tabs.length && groups().length > 1) await collapseGroup(g, "tabdrag-strip");
      else { await loadActive(g); renderTabs(g); }
      focusGroup(t.g);
      await loadActive(t.g);
      renderTabs(t.g);
    } else {                                 // edge: split, then collapse an
      await splitWith(t.g, "row", tab);      // emptied source (lone-tab drag to
      if (!g.tabs.length) await collapseGroup(g, "tabdrag-edge");  // own edge nets a plain move)
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

/* ---------- R34.18 TYPE PROBE (test-only, OPENSIDIAN_TYPEPROBE=1) ------------
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
/* ---------- themecsp RIG APPLICATOR (test-only, OPENSIDIAN_SMOKE_CSS=<path>) ---
   NOT a CSS loader. The threat model is attacker-controlled CSS living in the
   document; the CHEAPEST injector for that is an inline <style> (style-src
   'unsafe-inline' lets the style EXIST — what the egress proof falsifies is that
   anything inside it can REACH the network). The backend hook returns the file
   only when the operator set the env var; a shipped build never sets it and no
   product feature calls smoke_css. Once injected we republish the census so the
   [thmpx:] computed-style probe reflects whatever rule just won the cascade. */
let smokeCssOn = false;
inv("smoke_css").then(css => {
  if (typeof css === "string" && css.length) {
    const s = document.createElement("style");
    s.id = "opensidian-smoke-css";
    s.textContent = css;                 // inline: allowed to exist, must not egress
    (document.head || document.documentElement).appendChild(s);
    smokeCssOn = true;
    try { updateTitle(); } catch (_) {}  // census now carries [thmpx:]
  }
}).catch(() => {});
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
  await renameThenAsk(name, nn);
}

/* R24.3 — THE RENAME TAIL, AND THE ONLY ONE. "After a rename that has incoming
   links, stock shows the Update links modal ... because `Automatically update
   internal links` is OFF by default" — R24.3 says nothing about WHICH rename
   surface, and the app has two: the inline title (R34) and F2 / "Rename file"
   (cmdRename). Until now only the title road asked; F2 called `rename_note`,
   which is `move_note_in` + `update_links_in` with no question in between — a
   silent vault-wide rewrite, the exact shape ux-3 was deleted for (R32.5).
   Two roads with two different answers to "may I edit your other notes" is one
   road too many, so the tail is extracted here and both call it.

   The ORDER is R34.1's measured order and is not negotiable: the file moves
   FIRST (move_note), the links are stale at that instant (R34.2), and only
   then is the question asked. `update_links` therefore still has exactly two
   callers, both on the far side of a recorded answer (the modal, or R34.8
   consent already on disk in the vault).

   `rename_note` survives as the composite the rust unit tests exercise
   (rename_in); the UI no longer calls it, because nothing the USER does may
   rewrite another note without an answer. */
async function renameThenAsk(old, nn) {
  let blast;
  try { blast = await inv("move_note", { old, new: nn }); }
  catch (err) { say(String(err && err.message || err)); updateTitle(); return false; }
  await applyRename(old, nn);
  const links = blast && blast.links || 0, files = blast && blast.files || 0;
  if (!files) { updateTitle(); return true; }   // R34.3: nothing links in -> NO modal, ever
  let consent = false;
  try { consent = await inv("link_consent"); } catch (err) { consent = false; }
  if (consent) { await runUpdateLinks(old, nn); return true; }   // R34.8: already answered, in the vault
  openUpdateLinks(old, nn, links, files);
  return true;
}

/* ---------- R34.4-R34.8 the "Update links" prompt --------------------------
   This modal is the difference between R34 and the DELETED ux-3 feature, which
   rewrote a whole vault's links with no question asked. So it is written as a
   gate, not as a notification: `update_links` is called from exactly two
   places, both of them on the far side of a recorded answer (this modal, or
   R34.8 consent already in the vault). */
let ulPending = null, noticeTxt = "", noticeT = 0, noticeSrc = "";
// R24.7: declared beside ulPending, not down in its own section, because
// updateTitle() reads it and updateTitle runs from load-time code ABOVE that
// section — a `let` in the temporal dead zone would throw there, not read null.
let delPending = null;

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

/* ---------- R24.7/R24.8 the delete confirmation --------------------------
   The only irreversible act the explorer offers, so it is written as a
   QUESTION with two answers that are both real: Cancel leaves the file exactly
   where it was (nothing is called at all — not even a "dry run" of the
   backend), Delete calls delete_note once.

   WHAT THE DIALOG STATES, and why each part is there:
     - the file, by its vault-relative name (two notes can share a basename);
     - where it goes: `.trash/` inside THIS vault. delete_note_in moves it
       there with one rename(2) and the commit message argues why that, and not
       "your system trash", is the honest sentence under the Landlock ruleset;
     - how many notes link to it. R24.8 forbids editing those notes, and R24.9
       (shipped) renders their now-unresolved links faded. The count is the
       warning the user gets BEFORE the fact; the faded links are what they see
       after. A delete that quietly breaks four links in three notes and says
       nothing is the failure mode this line exists to prevent.
   The count comes from `backlinks_ctx` — the command the backlinks pane
   already uses. No second counter: R24.8 says a delete must not even READ the
   other notes for the purpose of rewriting them, and it does not, but the
   number on screen must still come from the same index everything else reads,
   not from a private walk that can disagree with the pane one panel away. */
const delSentence = (files, lines) =>
  files ? "Its links break in " + files + (files === 1 ? " note" : " notes") +
          " (" + lines + (lines === 1 ? " line" : " lines") +
          "). Those notes are not edited — their links go faded."
        : "No other note links to it.";

async function askDelete(nm) {
  let bl = [];
  try { bl = await inv("backlinks_ctx", { name: nm }); }
  catch (err) { bl = []; }                   // a count we could not take must not block the delete
  const files = bl.length, lines = bl.reduce((a, b) => a + b.lines.length, 0);
  openDelete(nm, files, lines);
}
function openDelete(nm, files, lines) {
  delPending = { name: nm, files, lines };
  $("delq").textContent = "Are you sure you want to delete " + nm + "? It will be moved to .trash in this vault.";
  $("delsay").textContent = delSentence(files, lines);
  $("delbox").hidden = false;
  $("del-no").focus();                       // the DEFAULT is Cancel — see style.css
  updateTitle();
}
function closeDelete() {
  if (!delPending) return;
  delPending = null;
  $("delbox").hidden = true;
  const g = fg();
  if (g && g.lp && g.lp.isConnected) g.lp.focus({ preventScroll: true });
  updateTitle();
}
async function delAnswer(kind) {
  if (!delPending) return;
  const nm = delPending.name;
  closeDelete();
  if (kind !== "yes") return;                // Cancel: the vault is not touched at all
  try { await inv("delete_note", { name: nm }); }
  catch (err) { say(String(err && err.message || err)); return; }
  await afterDelete(nm);
}
/* The note is gone from disk; now the UI stops claiming otherwise. Tabs first,
   because a tab holding an unsaved buffer of a deleted note would RESURRECT it:
   closeTab flushes the pending save, and that write recreates the file the user
   just deleted. So the pending timer is cancelled before the tab is closed —
   the user's answer was "delete", and honouring a 400 ms-old keystroke over it
   is the same data-loss shape in reverse. */
async function afterDelete(nm) {
  for (const g of groups()) {
    for (let i = g.tabs.length - 1; i >= 0; i--) {
      const t = g.tabs[i];
      if (t.kind || t.name !== nm) continue;
      if (i === g.active && g.saveT) { clearTimeout(g.saveT); g.saveT = null; }
      await closeTab(g, i, "user-close", "delete-dialog", true);   // noFlush: never resurrect what the user deleted
    }
  }
  // R14 undo-close would otherwise offer to reopen a note that no longer exists
  for (let i = closedTabs.length - 1; i >= 0; i--) if (closedTabs[i].name === nm) closedTabs.splice(i, 1);
  const mi = mruList.indexOf(nm);
  if (mi >= 0) mruList.splice(mi, 1);
  await refreshTree();                       // the explorer row goes
  await refreshBm();                         // a bookmark of it is now a bookmark with no file (backend leaves it: R24.8)
  updateTitle();
}
$("del-no").onclick = () => delAnswer("no");
$("del-yes").onclick = () => delAnswer("yes");
/* Escape = Cancel: dismissing a question is never consent, and here consent is
   irreversible. Same focus trap as the Update links prompt (keyCode 9 for the
   ISO_Left_Tab spelling WebKitGTK actually sends). */
$("delbox").addEventListener("keydown", e => {
  if (!delPending) return;
  if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); delAnswer("no"); return; }
  if (e.key === "Tab" || e.keyCode === 9) {
    e.preventDefault();
    const b = [$("del-no"), $("del-yes")];
    const i = b.indexOf(document.activeElement);
    b[(i < 0 ? 0 : i + (e.shiftKey ? b.length - 1 : 1)) % b.length].focus();
    updateTitle();
    return;
  }
  e.stopPropagation();
});
$("delbox").addEventListener("mousedown", e => { if (e.target === $("delbox")) e.preventDefault(); });  // click-off is not an answer

/* [modal:del] + [del:<name>/<files>/<lines>/<focused button 0=Cancel,1=Delete>]
   + [delsay:<the stated sentence>] + [delx:<cx,cy>;<cx,cy>].
   The phase asserts the DIALOG, not just the end state: without a token, a
   Cancel test cannot tell "the dialog came up and was dismissed" from "Delete
   did nothing at all", and those are the two things a confirmation is between. */
function delTok() {
  const b = [$("del-no"), $("del-yes")];
  const f = b.indexOf(document.activeElement);
  const xs = b.map(e => {
    const r = e.getBoundingClientRect();
    return Math.round(r.left + r.width / 2) + "," + Math.round(r.top + r.height / 2);
  }).join(";");
  const q = s => String(s == null ? "" : s).replace(/[[\]|]/g, "");
  return " [del:" + q(delPending.name) + "/" + delPending.files + "/" + delPending.lines + "/" + f + "]" +
         " [delsay:" + q($("delq").textContent + " " + $("delsay").textContent).slice(0, 200) + "]" +
         " [delx:" + xs + "]";
}

async function loadActive(g) {
  hideAc();
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (g.unkEl) g.unkEl.hidden = !(t && t.kind === "unk");
  if (t && t.kind === "unk") {      // W4: a kept leaf we have no view for — say so, keep it, touch nothing
    showEditor(g);                  // stops a graph sim this pane may have been running
    g.status.hidden = true;
    g.lp.style.display = g.preview.style.display = g.editor.style.display = "none";
    if (!g.unkEl) {
      g.unkEl = document.createElement("div");
      g.unkEl.className = "unkview";
      g.content.append(g.unkEl);
    }
    g.unkEl.innerHTML = "";
    const h = document.createElement("div"); h.className = "unkt";
    h.textContent = "No view for “" + t.utype + "” here";
    const p = document.createElement("div");
    p.textContent = "This tab is kept in the layout unchanged, so the app that made it finds it again.";
    g.unkEl.append(h, p);
    g.unkEl.hidden = false;
    renderTabs(g);
    treeHighlight();
    rgFollow();
    return;
  }
  if (t && t.kind === "lg") {       // R7.1: localgraph tab owns the pane's canvas
    await showLocalGraph(g, t);
    renderTabs(g);
    treeHighlight();
    rgFollow();                     // lgpanes: lg focused -> panes = rLeaf note (not awaited: outside graph perf spans)
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
    rgFollow();                     // lgpanes (Q4, shot 16): GLOBAL graph focused -> panes empty
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

/* opentab (docs/opentab/requirements.md): stock 1.13.7 opens a note from the
   file explorer and from bookmarks like this (black-box, display :150):
   - plain click REPLACES the active tab's note, even when the note is already
     open in another tab (REQ-1/2/3, no dedup); a PINNED active tab is never
     replaced, the note opens in a new tab (REQ-5); no tab at all -> one tab.
   - middle click, Ctrl/Cmd click and menu "Open in new tab" open a NEW tab
     IMMEDIATELY RIGHT of the active one, focused, never deduped (REQ-6..9,11,14).
   rowOpen is the one seam the two row kinds share. */
async function openNewTab(name) {
  const g = fg();
  await act("note_open", { note: name, via: "newtab", tabs: g.tabs.length }, async sp => {
    await flushSave(g);
    const at = g.active >= 0 ? g.active + 1 : g.tabs.length;
    // an INSERT at `at`, written without splice: the tabclose census (main.rs test
    // tabclose_every_removal_path_names_a_cause) counts every splice on g.tabs as a removal site
    const nt = mkTab(name);
    g.tabs.push(nt); g.tabs.copyWithin(at + 1, at); g.tabs[at] = nt;
    g.active = at;
    await loadActive(g);
    Object.assign(sp.attrs, { mode: g.tabs[g.active].mode, bytes: g.editor.value.length, lines: g.lpLines || 0 });
  });
}
async function rowOpen(name, ev) {
  if (ev && (ev.button === 1 || ev.ctrlKey || ev.metaKey)) return openNewTab(name);
  const g = fg();
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (t && t.pinned) return openNewTab(name);                 // REQ-5
  if (t && t.kind) return openInTab(name);                   // a graph/special tab is not a note slot: keep the old push
  if (t && t.name === name) { await flushSave(g); return; }  // REQ-4: already showing it
  return navigate(g, name);                                  // REQ-1/2/3 (+ R13.3 linked members follow)
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
/* R4X.3 (navbtn): the buttons' TRUTH comes from the R19 model, not pixels.
   navState resolves the group's acting tab exactly the way histGo does — a
   linked local graph delegates to its linked group — and reads hpos against
   the history bounds. navBtnSync repaints every pane's pair from that state;
   it is called from updateTitle, the one census choke point, so the buttons
   can never be fresher or staler than the [nvb:] token a phase asserts. */
function navState(g) {
  if (!g || g.active < 0) return { b: false, f: false };
  let tab = g.tabs[g.active];
  if (tab.kind === "lg" && tab.linkId != null) {
    const h = groups().find(x => x.id === tab.linkId);
    if (!h || h.active < 0) return { b: false, f: false };
    tab = h.tabs[h.active];
  }
  if (tab.kind) return { b: false, f: false };
  return { b: tab.hpos > 0, f: tab.hpos < tab.hist.length - 1 };
}
function navBtnSync() {
  for (const h of groups()) {
    if (!h.navback) continue;
    const s = navState(h);
    h.navback.disabled = !s.b;
    h.navfwd.disabled = !s.f;
  }
}
/* R4X.5 (navbtn): right-click on a nav button drops that stack as a menu
   BELOW the button — stock 1.13.7 measured (docs/recon-navbtn §4): the back
   menu lists the back stack most-recent-first, the forward menu the forward
   stack nearest-first, the CURRENT entry marked by EXCLUSION (never listed,
   no checkmark), a file icon per row. Selecting a row is a move WITHIN the
   stack — histGo with that row's delta — so BOTH stacks survive the jump
   (browser semantics, §4 19-mid-click.png: only a NEW navigation, histPush,
   discards forward). Right-clicking a DISABLED button opens NOTHING, not
   even an empty panel (§4 23-fwd-after.png): native disabled suppresses the
   event, and the stack-empty guard below backs that up if an engine ever
   dispatches it anyway. The menu inherits the shared .ctxmenu plumbing, so
   [menu:]/[mt:]/[mg:] census, outside-click and Escape dismissal all come
   from the same seam every other menu is asserted through. */
function navHistMenu(e, g, dir, btn) {
  e.preventDefault();
  e.stopPropagation();
  closeMenu();
  if (!g || g.active < 0) return;
  let h = g, tab = h.tabs[h.active];
  if (tab.kind === "lg" && tab.linkId != null) {  // lg delegates to its linked group — exactly navState/histGo
    h = groups().find(x => x.id === tab.linkId);
    if (!h || h.active < 0) return;
    tab = h.tabs[h.active];
  }
  if (tab.kind) return;
  const idx = [];
  if (dir < 0) { for (let i = tab.hpos - 1; i >= 0; i--) idx.push(i); }               // back: most-recent-first
  else { for (let i = tab.hpos + 1; i < tab.hist.length; i++) idx.push(i); }          // fwd: nearest-first
  if (!idx.length) return;                        // disabled end: opens nothing
  const m = document.createElement("div");
  m.className = "ctxmenu";
  // [mt:navback|navfwd:<note>] — WHICH button on WHICH note this menu came off
  // (same forge-stripping as tabMenu), so per-tab menu assertions name their tab.
  m.dataset.mt = (dir < 0 ? "navback" : "navfwd") + ":" +
    String(tab.name).split("/").pop().replace(/[|\]:]/g, "");
  for (const i of idx) {
    const d = document.createElement("div");
    const s = document.createElement("span");
    s.className = "mi";                           // file icon per row (recon §4, 18-menu-crop.png)
    s.innerHTML = BM_ICON_FILE;
    d.appendChild(s);
    d.appendChild(document.createTextNode(String(tab.hist[i].n).split("/").pop()));
    d.onmousedown = ev => ev.stopPropagation();
    d.onclick = () => { closeMenu(); histGo(i - tab.hpos, h); };   // delta read at CLICK time
    m.appendChild(d);
  }
  const r = btn.getBoundingClientRect();          // stock: panel below the button, left-aligned (§4)
  placeMenu(m, Math.round(r.left), Math.round(r.bottom) + 2);
}
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

/* ---------- TABCLOSE: why did that tab disappear? ----------
   Operator report 2026-09-19: "when I'm editing a note, it randomly closes."
   The report could not be answered from any artifact, because nothing recorded
   WHY a tab went — and dropTab() below cancels the pending save first, so a
   spurious close is silent data loss of everything typed inside the debounce.

   So: there are exactly FOUR places in this file that remove a tab from a
   group, and every one of them now names its cause here.
     closeTab()      user-close     (close glyph :1763, ctrl+w, delete dialog)
     dropTab()       external-delete / external-rename (the watcher)
     collapseGroup() pane-collapse  (R6.5 — the group follows its last tab)
     enterVault()    session-replace (the whole layout is thrown away)
   The cause set is CLOSED and mirrored in src-tauri/src/main.rs
   (TAB_REMOVAL_CAUSES); an unknown cause is an error on both sides, because
   "something else removed it" is exactly the answer that was missing.

   Two records per removal, deliberately, because each alone lies:
     * a LOG line, via the tab_removed command -> the app's stderr. It survives
       the window, so a reviewer greps a finished run and can say why every tab
       disappeared — `grep '\[tabgone\]' app-*.log`.
     * a CENSUS token in the title -> a driver can assert the cause of a removal
       it just triggered, in the same read as [dirty:] and [vc:].
   `dirty` is the tab's state AT REMOVAL; `flushed` says whether its bytes were
   written to DISK first and `preserved` whether they were parked in the
   undo-close rescue buffer because writing them was not allowed (R11.4: the
   file is gone). dirty=1 flushed=0 preserved=0 is the F-class defect, stated at
   the moment it happens instead of reconstructed from a bug report. */
const TAB_CAUSES = ["user-close", "external-delete", "external-rename", "pane-collapse", "session-replace"];
let tgSeq = 0, tgHist = [];        // census [tgn:<seq>] [tg:<cause>:<d>:<note>|...]
/* A note name is user data and the census is a bracket-delimited string, so the
   same stripping the bookmark census uses applies: a note called "x] [dirty:0"
   must not be able to forge a token (see the [bm:] comment). */
const tgSafe = s => String(s == null ? "" : s).replace(/[\[\]|:]/g, "").slice(0, 40);
/* t may be a tab object, a bare name, or null (collapseGroup records the PANE,
   which has no single tab). dirty is read from the tab's own base, not from the
   focused group, because the tab being removed is often not the focused one. */
function tabGone(cause, t, opts) {
  const o = opts || {};
  const name = typeof t === "string" ? t : (t && (t.kind ? t.kind + ":" + (t.name || "") : t.name)) || "";
  // dirty: the caller may state it (it knows the buffer); otherwise derive it
  // from the tab's base, which is the only per-tab record of the disk bytes.
  const dirty = o.dirty !== undefined ? !!o.dirty
    : !!(t && typeof t === "object" && !t.kind && t.base !== undefined && o.buf !== undefined && o.buf !== t.base);
  const bad = TAB_CAUSES.indexOf(cause) < 0;
  if (bad) noteErr("tabGone bad cause " + cause);     // surfaced as [jserr:], never swallowed
  tgSeq++;
  // The witness letters, in the order a reviewer reads them: d/c = dirty state,
  // then what happened to those bytes — `f` they reached DISK before the tab
  // went, `p` they were PRESERVED in the undo-close buffer because writing them
  // was not allowed (the file is gone: R11.4 forbids the resurrect). A dirty
  // removal with NEITHER letter is the F-class defect, and now says so in one
  // token instead of in a bug report.
  tgHist.push((bad ? "BAD" : cause) + ":" + (dirty ? "d" : "c")
    + (o.flushed ? "f" : "") + (o.preserved ? "p" : "") + ":" + tgSafe(name));
  if (tgHist.length > 8) tgHist.shift();              // the census is a title, not a journal
  try {
    inv("tab_removed", {
      cause: bad ? "BAD-" + cause : cause, note: String(name).slice(0, 120),
      dirty, flushed: !!o.flushed, preserved: !!o.preserved, via: String(o.via || "?"), seq: tgSeq,
      tabsLeft: o.tabsLeft == null ? -1 : o.tabsLeft, groups: groups().length,
    }).catch(() => {});                               // the log is evidence, never a dependency of the close
  } catch (_) {}
  return dirty;
}
function tgTok() {
  return " [tgn:" + tgSeq + "]" + (tgHist.length ? " [tg:" + tgHist.join("|") + "]" : "");
}
/* S4, the close glyph (:1763) under a REFLOWING tab bar. The cross is a 16px
   box at the right edge of a 120px tab that is `flex-shrink: 1` inside a strip
   that also carries the mode button, the right-panel toggle and — with the
   window frame on — a 140px inset (.tabbar.wfinset). A driver that clicks a
   computed 200+120*i+102 therefore hits the TAB BODY once anything reflows,
   which SWITCHES tabs instead of closing one: a spurious "nothing happened",
   or worse a close of the wrong tab. So publish the painted centre of every
   cross, per pane, exactly like [stx:] does for the right-panel strip: the
   phase clicks what it measured. A cross whose centre has been clipped out of
   its own tab (R22: overflow:hidden at narrow widths) is reported as `!x,y` —
   an unclickable close button is itself a finding, not a missing measurement. */
function tcxTok() {
  const panes = [];
  for (const g of groups()) {
    const xs = [];
    const strip = g.tabsEl ? g.tabsEl.getBoundingClientRect() : null;
    for (const el of (g.tabsEl ? g.tabsEl.querySelectorAll(".tab > .x") : [])) {
      const r = el.getBoundingClientRect(), tab = el.parentElement.getBoundingClientRect();
      if (r.width <= 0 || r.height <= 0) { xs.push("-"); continue; }
      const cx = Math.round(r.left + r.width / 2), cy = Math.round(r.top + r.height / 2);
      const clipped = !strip || cx < tab.left || cx > tab.right || cx < strip.left || cx > strip.right;
      xs.push((clipped ? "!" : "") + cx + "," + cy);
    }
    panes.push(xs.join(";"));
  }
  return " [tcx:" + panes.join("|") + "]";
}

/* `noFlush` exists for exactly ONE caller: afterDelete. The user said delete;
   writing a 400 ms-old keystroke back would recreate the file they just
   removed. Before the flushSave fix that was achieved by cancelling the timer
   and relying on flushSave's early return — a guarantee made of a side effect,
   which broke the moment flushSave started trusting the BUFFER instead of the
   timer. It is a parameter now, so the one path that must not write says so. */
async function closeTab(g, i, cause, via, noFlush) {
  const rm = g.tabs.length === 1 && groups().length > 1;      // R6.5: last tab -> the pane goes too
  await act("pane_close", { note: g.tabs[i].name, kind: g.tabs[i].kind || "note", pane_removed: rm, groups: groups().length, tabs: g.tabs.length - 1 }, async () => {
  // tabclose: recorded BEFORE the flush, with the dirty state the user's
  // keystrokes actually left — after flushSave() every tab is clean and the
  // log would claim there was never anything at risk.
  const wasDirty = i === g.active && !g.tabs[i].kind ? bufOf(g) !== g.tabs[i].base : false;
  const risked = wasDirty ? bufOf(g) : null;       // the bytes, read while they still exist
  // `flushed` is now the RESULT of the flush, not the intention to flush. It
  // used to be recorded as `wasDirty`, so a flush that wrote nothing (no timer
  // armed — the undo-close restore path) or failed (F1 banner) logged
  // flushed=1 with the bytes nowhere: the instrument lied on exactly the path
  // criterion 4 is about.
  const flushed = i === g.active && !noFlush ? await flushSave(g) : false;
  // and if the bytes did NOT reach disk, they are not thrown away either:
  // same promise dropTab makes, on the path the user asked for. (A note the
  // user DELETED is the exception: afterDelete purges its rescue entry two
  // lines later anyway, and offering the bytes back is offering a resurrect.)
  const rescued = wasDirty && !flushed && !noFlush ? risked : null;
  tabGone(cause || "user-close", g.tabs[i], { dirty: wasDirty, flushed, preserved: rescued != null, via: via || "closeTab", tabsLeft: g.tabs.length - 1 });
  if (!g.tabs[i].kind) ucPush(g.tabs[i].name, rescued, cause || "user-close");   // R14 undo close tab
  unlinkTab(g, g.tabs[i], true);    // R13.4: closing a member unlinks it
  dropView(g.tabs[i]);              // R20: retained view goes with the tab
  g.tabs.splice(i, 1);
  if (!g.tabs.length && groups().length > 1)  // R6.5: empty group leaves the tree
    return collapseGroup(g, "closeTab");
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
    row.dataset.folder = full;                  // R24.6: the drop target's identity, off the DOM
    const kids = document.createElement("div");
    kids.className = "tkids" + (open ? "" : " collapsed");
    row.onclick = () => act("folder_toggle", { folder: full, open: collapsed.has(full), notes: node.dirs.get(d).notes.length, rows: kids.childElementCount }, () => {
      const o = collapsed.has(full);
      o ? collapsed.delete(full) : collapsed.add(full);
      row.classList.toggle("open", o);
      kids.classList.toggle("collapsed", !o);
      updateTitle();                            // collapseall: [fold:] moves with every folder toggle
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
    row.dataset.note = nm;
    row.onclick = e => { if (treeClickEaten()) return; rowOpen(nm, e); };     // opentab REQ-1/3/5/7: replace in place; ctrl = new tab
    row.onauxclick = e => { if (e.button === 1) { e.preventDefault(); rowOpen(nm, e); } };   // opentab REQ-6: middle = new tab
    row.oncontextmenu = e => noteMenu(e, nm);   // R9.4: bookmark toggle
    row.addEventListener("mousedown", e => treeDragStart(e, nm));   // R24.6: drag onto a folder row = move
    treeRows.set(nm, row);
    out.appendChild(row);
  }
}

/* ---------- R24.6: DRAG A NOTE ONTO A FOLDER ROW TO MOVE IT ----------------
   THE SAME MOUSE MACHINE AS THE TABS, not a second convention: mousedown +
   window mousemove/mouseup, a 6px threshold, a #tabghost chip, drop targets
   resolved once per rAF frame against rects cached at drag start, and the
   commit wrapped in act(). HTML5 `dragstart`/dataTransfer is used NOWHERE in
   this codebase and is not introduced here; R31's drop-to-attach is an
   OS-level XDND delivered as the `drop-files` event and never sees a DOM drag,
   so the two cannot collide.

   NO LINKS ARE REWRITTEN AND NO PROMPT IS SHOWN — that is R24.6 itself
   ("links that still resolve are left untouched and no prompt is shown"), and
   it is true of THIS vault for a mechanical reason worth writing down:
   index.rs:367 resolves a wikilink by full relative path OR basename, so
   `[[Note]]` still resolves after `Note` becomes `sub/Note`. The only spelling
   a move can break is a FULL-PATH link (`[[old/Note]]`), and silently
   rewriting those vault-wide without asking is exactly the deleted ux-3
   behaviour (R32.5). So the move calls move_note — the EXISTING backend, which
   also carries the bookmark (move_note_in) — and stops. `update_links` keeps
   its two consented callers (R34's prompt and R34.8 consent) and gains none;
   there is no second rewriter in this feature, and the blast radius move_note
   returns is published in the census rather than acted on.

   Folder rows are drop TARGETS here, not drag sources: moving a folder is N
   moves with no atomicity and belongs with the folder-rename work (R24.5).  */
let treeEat = 0;                         // mark: a drag just ended, eat the click it generates
function treeClickEaten() {              // a completed drag must not also open/toggle the row
  const t = treeEat;
  treeEat = 0;                           // one click only, and stale marks expire by time:
  return !!t && performance.now() - t < 400;   // a drop on a FOLDER row generates no row click
}
function treeDragStart(e, nm) {
  if (e.button !== 0) return;
  const sx = e.clientX, sy = e.clientY;
  const base = nm.split("/").pop();
  const par = nm.includes("/") ? nm.slice(0, nm.lastIndexOf("/")) : "";
  let ghost = null, target = null, hl = null, zones = null, raf = 0, last = null;
  const clearHl = () => { if (hl) { hl.classList.remove("drop-into"); hl = null; } };
  const step = () => {
    raf = 0;
    const ev = last;
    if (!ghost) {
      if (Math.abs(ev.clientX - sx) + Math.abs(ev.clientY - sy) < 6) return;
      ghost = document.createElement("div");
      ghost.id = "tabghost";
      ghost.textContent = base;
      document.body.appendChild(ghost);
      dragTok = "-:" + base;             // a drag is UP with no destination yet
      updateTitle();
      // rects cached ONCE (R20): a per-frame getBoundingClientRect over every
      // folder row is a layout read per mousemove on a 500-row explorer.
      // The tree box is LAST so a folder row always wins the hit test.
      zones = [...document.querySelectorAll("#tree .trow.folder")]
        .map(el => ({ el, f: el.dataset.folder, r: el.getBoundingClientRect() }))
        .filter(z => z.r.height > 0 && z.f != null);
      zones.push({ el: $("tree"), f: "", r: $("tree").getBoundingClientRect() });   // the vault ROOT
    }
    ghost.style.transform = "translate3d(" + (ev.clientX + 10) + "px," + (ev.clientY + 12) + "px,0)";
    let nt = null;
    for (const z of zones) {
      const r = z.r;
      if (ev.clientX < r.left || ev.clientX > r.right ||
          ev.clientY < r.top || ev.clientY > r.bottom) continue;
      if (z.f !== par) nt = z;            // the folder it is ALREADY in is not a destination
      break;
    }
    if ((nt ? nt.f : null) !== (target ? target.f : null)) {
      clearHl();
      if (nt) { hl = nt.el; hl.classList.add("drop-into"); }   // R24.6: the target is OUTLINED
      target = nt;
      // R24.6: the chip NAMES THE DESTINATION — and the census carries the same
      // sentence, so a phase asserts what the chip says instead of OCR'ing it.
      // ASCII arrow on purpose: this string travels through the window title.
      ghost.textContent = base + (target ? " -> " + (target.f || "vault root") : "");
      dragTok = (target ? (target.f || "/") : "-") + ":" + ghost.textContent;
      updateTitle();                     // ONLY on a target change — never per frame
    }
  };
  const move = ev => { last = ev; if (!raf) raf = requestAnimationFrame(step); };
  const up = async () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    if (raf) { cancelAnimationFrame(raf); raf = 0; if (last) step(); }
    const t = target;
    if (ghost) ghost.remove();
    clearHl();
    dragTok = "";
    if (!ghost || !t) { updateTitle(); return; }   // plain click, or dropped on nothing
    treeEat = 1;                                   // the mouseup's click is not an "open this note"
    await moveNoteTo(nm, t.f);
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}

/* The commit. Refusals are the rename's refusals (say(), nothing moves) and
   the collision check is the same COURTESY the title rename does — what
   actually refuses is create_new/O_EXCL inside move_note_in, which no JS-side
   test of a 1000ms-stale cache can hold against another writer. */
async function moveNoteTo(nm, folder) {
  const base = nm.split("/").pop();
  const nn = folder ? folder + "/" + base : base;
  if (nn === nm) return;
  if (notesCache.includes(nn)) { say("There's already a file with the same name"); return; }
  await act("note_move", { note: nm, to: folder, dest: nn }, async () => {
    // the bytes of any open buffer of THIS note land before the file moves —
    // a debounced save that fires after the rename would write the old path
    for (const g of groups()) if (curOf(g) === nm) await flushSave(g);
    let blast;
    try { blast = await inv("move_note", { old: nm, new: nn }); }
    catch (err) { say(String(err && err.message || err)); updateTitle(); return; }
    await applyRename(nm, nn);        // tabs, history, MRU, bookmarks, tree — the ONE post-move choke point
    // [mv:<old>><new>/<files that link in>] — the radius is REPORTED, never
    // acted on (R24.6). A phase can assert "2 notes link here and were still
    // not touched", which is the whole claim.
    mvTok = nm + ">" + nn + "/" + ((blast && blast.files) || 0);
    updateTitle();
  });
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
  if (scHits(g).length) scMarks(g);        // R25.13f: the flash survives every re-render
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
  // R26: an edit moves every match after it. Ed.after() re-renders the touched
  // rows (dropping their marks) and every edit path lands here, so this is where
  // an open bar re-scans. Guarded on `open` so the closed case — i.e. the whole
  // measured typing path (R18) — costs one property read and nothing else.
  if (g.find && g.find.open) fSync(g);
  // R25.13d/f: the search flash has NO timer and survives typing and undo. The
  // rows it was painted on were just rebuilt, so it is repainted here, at the
  // same choke point, from offsets Ed.replace already mapped through the edit.
  if (scHits(g).length) scMarks(g);
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
  if (state && fg().active >= 0) await closeTab(fg(), fg().active, "user-close", "cmdCloseTab");
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
/* listtoggle A3 (found by the gate phase, not by reading): the palette input
   TAKES the DOM selection, so a palette command that reads the editor
   selection (Ed.sel) found none and was a silent no-op — "Toggle bullet list"
   run from Ctrl+P changed no byte. Stock runs palette commands against the
   selection the editor had when the palette opened (evidence/q02-bullet.txt:
   select 3 lines, palette, all 3 toggled). So snapshot it at open and put it
   back, anchor and focus, just before a command RUNS. Escape is unchanged. */
let mdEdSel = null;
function edSelSnap() {
  if (!edEditable()) return null;
  const g = fg(), s = Ed.sel(g);
  if (!s) return null;
  const f = Ed.focusPos(g);
  const back = f && f.l === s.a.l && f.c === s.a.c && !s.empty;   // selection made upward
  return { g, t: g.tabs[g.active], anc: back ? s.b : s.a, foc: back ? s.a : s.b };
}
function edSelRestore(r) {
  const g = fg();
  if (!r || g !== r.g || g.tabs[g.active] !== r.t || !edEditable()) return;
  Ed.place(g, r.anc.l, r.anc.c);
  if (r.anc.l !== r.foc.l || r.anc.c !== r.foc.c) Ed.extendTo(g, r.foc.l, r.foc.c);
}
async function mdRun(it, e) {
  const r = mdEdSel; mdEdSel = null;
  closeModal();
  edSelRestore(r);
  return await it.run(e);
}
function openModal(kind, src) {
  mdEdSel = kind === "cp" ? edSelSnap() : null;   // BEFORE minput.focus() takes the selection
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
    d.onmousedown = async e => { e.preventDefault(); await mdRun(it, e); };
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
    if (it) return await mdRun(it, e);
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
   persisted to ~/.opensidian.json "hotkeys" in the stock Obsidian shape. ---------- */
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
// listtoggle R1/R3: the note tab in an editing mode (LP or Source — the op is
// text-only, R5 [Q15]); reading view is excluded [Q14]. Both the palette filter
// and the run guard use it, so a user-bound chord is a no-op in reading view.
function edEditable() {
  const g = state && fg();
  const t = g && g.active >= 0 ? g.tabs[g.active] : null;
  return !!(t && !t.kind && t.mode !== "reading");
}
function edList(kind) {        // one Ed.snap inside Ed.toggleList = ONE undo step [Q12 q12-undo*]
  if (!edEditable()) return;
  const g = fg(), s = Ed.sel(g);
  if (s) Ed.toggleList(g, s, kind);
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
     - the graph reads its colours out of getComputedStyle(document.body) — off
       BODY since themeone item 6, because that is where a theme's declarations
       land (DESIGN §3 cause 1); it follows the mode through the same body class
       with no extra wiring, and the ACTIVE THEME through data-vtheme.
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
  // R1 (themecsp): ALSO emit stock Obsidian's mode marker as a class on <body>
  // (.theme-dark / .theme-light), next to our :root[data-theme]. Stock-shaped
  // CSS keys off body.theme-dark (T6), which our root attribute shadows; without
  // this marker no stock CSS can ever apply. body exists (main.js loads at the
  // end of <body>); guard anyway so an early call never throws.
  if (document.body) {
    document.body.classList.toggle("theme-dark", themeMode === "dark");
    document.body.classList.toggle("theme-light", themeMode === "light");
  }
  if (state) updateTitle();                // census [theme:<mode>] follows the DOM
                                           // (before a vault is open there is no
                                           // title census to refresh — updateTitle
                                           // reads state.root)
}
/* a USER choice: apply it, and remember it. Persisted through the EXISTING
   settings store (~/.opensidian.json, key "theme" — main.rs set_theme), the same
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
/* ---------- THE SECOND AXIS IS GONE (themeone items 7/8, R1/R2) ----------
   Fifty lines lived here: PALETTES, applyPalette (the data-palette write),
   choosePalette, bootPalette and cmdSetPalette — a SECOND theme axis beside
   the mode, selecting between colour sets this file's own stylesheet carried.

   The operator asked for one axis. So the three palettes became real THEME
   FILES (src-tauri/themes/, seeded onto the vault's .obsidian/themes/ by
   builtins.rs), and the ONE thing that selects a theme is stock's cssTheme
   picker — `chooseVaultTheme` above. Everything that used to call
   choosePalette now calls that: the Appearance ▸ Themes dropdown, and the
   Ctrl+P "Use theme: <name>" entries (cpItems, which builds one per LISTED
   theme instead of one per hardcoded palette id).

   WHAT IS GAINED, precisely: a third-party theme a user drops in
   .obsidian/themes/ is reachable by the same command as a built-in, because
   there is no longer a private list of names the app will select between. A
   palette table could never have listed it.

   The MODE axis (applyTheme/chooseTheme above, dark|light, "theme" in
   ~/.opensidian.json) is untouched, including "no stored value =
   prefers-color-scheme decides" (R6). Mode and theme compose exactly the way
   mode and palette did: a theme declares body.theme-dark and body.theme-light
   blocks, and the mode class picks which one paints. */
/* ---------- themefs R3: vault CSS snippets (stock's files, T3) ----------
   A snippet is <vault>/.obsidian/snippets/<label>.css, toggled by the
   enabledCssSnippets array in the vault's own appearance.json — the backend
   (src-tauri/src/themefs.rs) owns the listing predicate, the byte-wise config
   round-trip and the R4X.4 mask sanitizer; this side only injects <style>
   elements and keeps them in DESIGN §5's cascade order:
       <link style.css> -> #vault-bridge -> #vault-theme -> snippet styles
   Snippets sit LAST (T3 RESULT 4: they compose on top of the theme), in
   enabledCssSnippets ARRAY order (T3 RESULT 3: the array is enable order).
   Appending to <head> keeps that true; #vault-theme (below) insertBefores the
   first [data-snip] element, and #vault-bridge (item 8) insertBefores
   #vault-theme. */
let vaultSnips = [], vaultSnipsOn = [], snipEls = new Map();
/* ---- themefs R5: the vault THEME (stock's .obsidian/themes/<Name>/) ----
   vaultThemesScan is the backend's oracle-predicate result: {listed:[names],
   excluded:[{dir,file,reason,message}]}. vaultTheme mirrors cssTheme ("" =
   built-in Default). The theme's <style id="vault-theme"> sits BEFORE every
   [data-snip] element (DESIGN §5: snippets compose on top of the theme —
   T3 RESULT 4); the #vault-bridge element (item 8) insertBefores it. */
let vaultThemesScan = { listed: [], excluded: [] }, vaultTheme = "";
let vaultBridgeAliases = 0;   // census [vbridge:<n>] — item 8 alias rows painting
/* themeone item 4 (C3): the boot's SEEDING decision, as the backend reported
   it — wrote / kept / failed counts, census [bseed:w<n>k<n>f<n>]. Read after
   the scan (the write already happened in Rust, before the webview existed),
   so on a fresh vault the same title carries w3 AND the three listed names,
   and on the second boot k3 with the same names. "-" until a vault is open. */
let vaultSeedTok = "-";
/* themeone item 6 (R5 causes 3+4): THE ACTIVE THEME'S IDENTITY, ON THE ROOT.
   The graph caches its five tokens (palette(), below) and repaints on a root
   attribute change. Both were keyed on data-theme/data-palette, and injecting
   or removing <style id="vault-theme"> moves NEITHER — so before this, picking
   a theme with the mode held left the graph holding the previous theme's
   colours AND never redrawing to notice (DESIGN §3 causes 3 and 4).
   ONE mechanism kills both, because both read the same attribute: themeInject
   and themeRemove — already the only authority over the theme element — write
   `data-vtheme` here, and the attribute then enters the cache key (cause 3)
   and the existing attributeFilter (cause 4).
   THE GENERATION COUNTER IS NOT DECORATION. A hot reload rewrites the SAME
   theme's bytes under the SAME name (T4 RESULT 2 measured stock repainting an
   edit to the active theme in 0.14-0.34 s); a name-only attribute would be
   unchanged across that write, the MutationObserver would not fire, and the
   graph would keep painting the pre-edit colours. The counter changes on every
   inject and every remove, so "same name, new bytes" is a different value.
   Removal writes a generation with an EMPTY name rather than deleting the
   attribute: `delete dataset.x` is also an attribute mutation and would fire
   the observer, but it would leave the key equal to the no-theme boot key, so
   a theme applied and removed within one cache lifetime would hand back the
   themed colours. */
let themeGen = 0;
const vthemeMark = name => {
  document.documentElement.dataset.vtheme = (++themeGen) + ":" + (name || "");
};
async function themeInject(name) {
  try {
    const r = await inv("theme_css", { name });
    // swap IN PLACE: the old css keeps painting until the new bytes are
    // ready (item 6 re-injects through here — remove-then-insert would let
    // a frame of Default through on every hot reload), and the element's
    // position (before the first [data-snip], DESIGN §5) never moves.
    let el = document.getElementById("vault-theme");
    if (!el) {
      el = document.createElement("style");
      el.id = "vault-theme";
      document.head.insertBefore(el, document.head.querySelector("style[data-snip]"));
    }
    el.textContent = r.css;
    // item 8, the R4 alias bridge (DESIGN §5/§7): #vault-bridge sits BEFORE
    // #vault-theme so the theme out-cascades its own aliases. GENERATED by
    // the backend from the same sanitized bytes: empty when the theme
    // declares none of the aliased stock names — then NO element exists and
    // the DOM stays byte-identical to the palette baseline. Same in-place
    // swap discipline as the theme element (hot reload comes through here).
    const bcss = r.bridge || "";
    let br = document.getElementById("vault-bridge");
    if (bcss) {
      if (!br) {
        br = document.createElement("style");
        br.id = "vault-bridge";
        document.head.insertBefore(br, el);
      }
      br.textContent = bcss;
    } else if (br) br.remove();
    // goal overlaytheme: the STOCK-DEFAULT sheet (#vault-stockdef) — stock
    // names derived from stock names the theme declared (--modal-background
    // <- --background-primary, docs/recon-overlaytheme Q1), so Layer 3's
    // var(--modal-background, <literal>) follows a theme that only set the
    // source. Its own element, NOT the bridge: it is not an alias row and
    // must not move [vbridge:]. Empty -> no element (R2). Sits first, before
    // the bridge and the theme, so the theme out-cascades it.
    const scss = r.stockdef || "";
    let sd = document.getElementById("vault-stockdef");
    if (scss) {
      if (!sd) {
        sd = document.createElement("style");
        sd.id = "vault-stockdef";
        document.head.insertBefore(sd, document.getElementById("vault-bridge") || el);
      }
      sd.textContent = scss;
    } else if (sd) sd.remove();
    // census [vbridge:<n>] — alias rows actually painting (one var() each)
    vaultBridgeAliases = bcss ? (bcss.match(/var\(/g) || []).length : 0;
    // item 6: the bytes are IN the DOM now — publish the identity that the
    // graph's cache key and its MutationObserver both read. AFTER the write,
    // never before: the observer fires synchronously at the end of this task,
    // and a mark set first would have the graph re-read tokens off a
    // stylesheet that has not been swapped yet.
    vthemeMark(name);
    if (r.message) say(r.message, "theme");  // R4X.4: the strip is LOUD
  } catch (e) {
    themeRemove();                           // stale css must not keep painting
    say(String(e), "theme");                 // R6: the refusal names the file
  }
}
function themeRemove() {
  const el = document.getElementById("vault-theme");
  if (el) el.remove();
  // the bridge lives and dies with the theme: aliases painting while no
  // theme does would hand the chrome to a var() nobody declares (item 8)
  const br = document.getElementById("vault-bridge");
  if (br) br.remove();
  const sd = document.getElementById("vault-stockdef");   // goal overlaytheme
  if (sd) sd.remove();
  vaultBridgeAliases = 0;
  vthemeMark("");   // item 6: "no theme, as of generation n" — a new key, and a repaint
}
/* pick a theme (the settings control's route): file FIRST — a refused write
   (unparseable appearance.json) must not paint a choice that will not
   survive the next boot — then the DOM. "" = Default: remove, inject nothing. */
async function chooseVaultTheme(name) {
  try { await inv("set_css_theme", { name }); }
  catch (e) { say(String(e), "theme"); return; }
  vaultTheme = name;
  if (name) await themeInject(name); else themeRemove();
  renderThemeCtl();
  armCssReload();                        // item 6: the watched set follows what is applied
  if (state) updateTitle();
}
/* the ONE insertion authority for snippet elements: enabledCssSnippets order
   (T3 RESULT 3), independent of WHEN the element is created — at load and on
   toggle-on the element lands last among snips (same as append), but a hot
   reload re-creating a mid-array element (item 6: a refused file fixed by an
   edit) must land BEFORE its later siblings, or the cascade lies about the
   enable order. */
function snipElInsert(label, el) {
  for (const l of vaultSnipsOn.slice(vaultSnipsOn.indexOf(label) + 1)) {
    const nxt = snipEls.get(l);
    if (nxt) { document.head.insertBefore(el, nxt); return; }
  }
  document.head.appendChild(el);
}
async function snipInject(label) {
  if (snipEls.has(label)) return;
  try {
    const r = await inv("snippet_css", { label });
    const el = document.createElement("style");
    el.dataset.snip = label;              // the removal/census handle — the label stays data, never an id fragment
    el.textContent = r.css;
    snipElInsert(label, el);
    snipEls.set(label, el);
    if (r.message) say(r.message, "snip");   // R4X.4: the strip is LOUD, one string, authored in themefs.rs
  } catch (e) {
    say(String(e), "snip");               // R6: the refusal names the file and the reason (backend string)
  }
}
/* item 6, the hot-reload path: re-read -> re-sanitize -> re-inject that ONE
   element. In place when it exists (the old css paints until the new bytes
   are ready); created at its enable-order position when it does not (an edit
   FIXING a previously refused snippet goes live — same loop as the theme).
   A re-read that now refuses removes the element: stale css must not keep
   painting a file the sanitizer no longer accepts. */
async function snipReinject(label) {
  try {
    const r = await inv("snippet_css", { label });
    let el = snipEls.get(label);
    if (!el) {
      el = document.createElement("style");
      el.dataset.snip = label;
      snipElInsert(label, el);
      snipEls.set(label, el);
    }
    el.textContent = r.css;
    if (r.message) say(r.message, "snip");
  } catch (e) {
    snipRemove(label);
    say(String(e), "snip");
  }
}
function snipRemove(label) {
  const el = snipEls.get(label);
  if (el) el.remove();
  snipEls.delete(label);
}
/* vault entry (and vault SWITCH: the old vault's CSS must not survive into
   the new one, so this clears before it loads). Failure to scan is not a
   notice: no vault / no snippets dir is stock's silent normal (T0). */
async function loadVaultCss() {
  for (const l of [...snipEls.keys()]) snipRemove(l);
  themeRemove();
  vaultSnips = []; vaultSnipsOn = [];
  vaultThemesScan = { listed: [], excluded: [] }; vaultTheme = "";
  vaultSeedTok = "-";
  await qfsLoad();                   // fontwheel: the vault's baseFontSize before first paint of a note (REQ-14)
  try {
    vaultThemesScan = await inv("themes_scan");
    // R6: LOUD where stock silently excludes — every broken theme dir says
    // its one message (dir + reason, authored in themefs.rs) at scan time.
    for (const x of vaultThemesScan.excluded) say(x.message, "theme");
    vaultTheme = await inv("get_css_theme");
  } catch { vaultThemesScan = { listed: [], excluded: [] }; vaultTheme = ""; }
  // item 4: the seeding pass that ran before this webview existed. Its own
  // try — a census token is not worth failing the vault entry over, and a
  // backend that could not answer must read as "-" and not as w0k0f0.
  try {
    const s = await inv("theme_seed_report");
    vaultSeedTok = "w" + s.wrote.length + "k" + s.kept.length + "f" + s.failed.length;
    for (const f of s.failed) say("theme seed failed — " + f, "theme");  // R6: loud
  } catch { vaultSeedTok = "-"; }
  // apply ONLY what the predicate lists: cssTheme naming an unlisted theme
  // paints the Default (its dir, if present, already said WHY above; an
  // absent dir is stock's silent normal — the oracle flags "!!" either way)
  if (vaultTheme && vaultThemesScan.listed.includes(vaultTheme)) {
    await themeInject(vaultTheme);
  }
  try {
    vaultSnips = await inv("snippets_scan");
    // a stale enabled entry whose file is gone is skipped silently — the array
    // is stock's own record and may outlive the file (unmeasured; re-measure
    // against /srv/reference/obsidian.AppImage if a gate ever makes it matter)
    vaultSnipsOn = (await inv("snippets_enabled")).filter(l => vaultSnips.includes(l));
  } catch { vaultSnips = []; vaultSnipsOn = []; }
  for (const l of vaultSnipsOn) await snipInject(l);
  armCssReload();
  if (state) updateTitle();
}
/* ---- themefs item 6: hot reload (DESIGN §8, criterion 4) ----------------
   The backend polls AT MOST the applied theme.css + enabled snippet files at
   100 ms (themefs::RELOAD_TICK_MS, alive only while something is applied)
   and emits `vault-css-changed` {kind,name} on an (mtime,len) move; this
   side re-reads through the SAME sanitizing commands and re-injects that ONE
   element. armCssReload is the frontend declaring what is APPLIED — it owns
   the listed/painting decision, so an unlisted cssTheme is declared as ""
   (Default paints, nothing to watch). Nothing here writes: criterion 4
   asserts appearance.json's bytes across an edit. NEW files are not live
   (stock's asymmetry, T4 RESULT 4 / T3 RESULT 5): the watch set moves only
   when a user action lands here, never because a tick discovered a file. */
let vaultCssReloads = 0;                 // census [creload:<n>] — the phase's latency clock
function armCssReload() {
  const theme = (vaultTheme && vaultThemesScan.listed.includes(vaultTheme)) ? vaultTheme : "";
  inv("vault_css_watch", { theme, snippets: vaultSnipsOn.slice() }).catch(() => {});
}
async function onVaultCssChanged(ch) {
  if (!ch) return;
  if (ch.kind === "theme") {
    // stale-event guard: the poller's word is never newer than this side's
    // own state — only the theme that IS painting gets re-injected
    if (ch.name !== vaultTheme || !vaultThemesScan.listed.includes(ch.name)) return;
    await themeInject(ch.name);
  } else if (ch.kind === "snippet") {
    if (!vaultSnipsOn.includes(ch.name)) return;
    await snipReinject(ch.name);
  } else return;
  vaultCssReloads++;
  if (state) updateTitle();
}
/* the toggle WITHOUT restart (criterion 3): one user action moves the file
   AND the DOM — but the file first. If the backend refuses (unparseable
   appearance.json is refused, never overwritten), the DOM stays put: a toggle
   that paints but does not persist would look exactly like one that works,
   until the next boot un-decides it. */
async function toggleSnippet(label) {
  const on = !vaultSnipsOn.includes(label);
  try { await inv("set_snippet_enabled", { label, on }); }
  catch (e) { say(String(e), "snip"); return; }
  if (on) { vaultSnipsOn.push(label); await snipInject(label); }
  else { vaultSnipsOn = vaultSnipsOn.filter(l => l !== label); snipRemove(label); }
  renderSnipCtl();                       // the settings control follows the state, like renderPaletteCtl
  armCssReload();                        // item 6: the watched set follows what is applied
  if (state) updateTitle();
}
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
/* ---------- goal fontwheel: stock "Quick font size adjustment" ----------
   docs/ctrlzoom/recon.md REQ-1..16. Ctrl+wheel over a markdown view (.lp —
   live preview AND source, .lp.src — or .preview) moves the vault's
   appearance.json "baseFontSize" by 1 px per notch, clamped 10..30, while
   "baseFontSizeAction" is on. OPERATOR EXCEPTION (REQ-2): an absent key reads
   ON here (stock: off) — themefs::quickfont owns that rule, not this file.
   This is NOT a second zoom: it moves only --font-text-size (text of the
   markdown views), never webkit zoom (R36 above, REQ-16), and Ctrl+0 stays
   the interface-zoom reset (REQ-6).
   Every Ctrl+wheel is preventDefault'ed, ON or OFF, everywhere (stock never
   scrolls on Ctrl+wheel: REQ-3/8/9/12) — except a <canvas>, whose own wheel
   handler (graph view, cv.onwheel) zooms the graph with or without Ctrl
   (REQ-10). The size goes on body's inline style the way stock's does, so a
   theme's --font-text-size cannot shadow it; at the default 16 the inline
   property is REMOVED, leaving the stylesheet/theme value in charge (and the
   typo phase's root-level probe chord meaningful on a fresh vault). */
const QFS_DEF = 16, QFS_MIN = 10, QFS_MAX = 30;
let qfsSize = QFS_DEF, qfsAct = true, qfsSteps = 0;
let qfsSave = Promise.resolve();          // writes are SERIALISED: a 3-notch burst must land 3 steps, in order (REQ-13)
function qfsApply() {
  const b = document.body;
  if (qfsSize === QFS_DEF) b.style.removeProperty("--font-text-size");
  else b.style.setProperty("--font-text-size", qfsSize + "px");
}
async function qfsLoad() {                // vault entry / switch: the vault's own record, or the defaults
  qfsSize = QFS_DEF; qfsAct = true;
  try { const q = await inv("get_quickfont"); qfsSize = Math.round(q.size); qfsAct = !!q.action; } catch { }
  qfsApply();
}
function qfsPersist(args) {
  qfsSave = qfsSave.then(() => inv("set_quickfont", args)).catch(e => say("font size not saved — " + errStr(e), "theme"));
}
function qfsSetAction(on) {               // the Settings ▸ Appearance toggle
  qfsAct = !!on;
  qfsPersist({ size: null, action: qfsAct });
  updateTitle();
}
window.addEventListener("wheel", e => {
  if (!e.ctrlKey) return;                                   // plain wheel: untouched (REQ-15)
  const t = e.target instanceof Element ? e.target : null;
  if (t && t.closest("canvas")) return;                     // graph keeps its own wheel (REQ-10)
  e.preventDefault();                                       // never scroll on Ctrl+wheel (REQ-3/8/9/12)
  if (!qfsAct || !e.deltaY) return;
  const sc = t && t.closest(".lp, .preview");
  if (!sc || (settingsOpen && t.closest("#settings"))) return;
  const next = Math.max(QFS_MIN, Math.min(QFS_MAX, qfsSize + (e.deltaY < 0 ? 1 : -1)));
  if (next === qfsSize) return;                             // clamped: nothing to write (REQ-5)
  // keep the TOP VISIBLE LINE while the text grows/shrinks (REQ-8): anchor on
  // the element under the scroller's top edge and restore its offset after
  const r = sc.getBoundingClientRect();
  let a = document.elementFromPoint(r.left + Math.min(40, r.width / 2), r.top + 2);
  if (a && (!sc.contains(a) || a === sc)) a = null;
  const before = a ? a.getBoundingClientRect().top : 0;
  qfsSize = next; qfsSteps++;
  qfsApply();
  if (a && a.isConnected) sc.scrollTop += a.getBoundingClientRect().top - before;
  qfsPersist({ size: qfsSize, action: null });
  updateTitle();
}, { passive: false });
/* [qfs:<baseFontSize>,<action 1|0>,<steps>,<computed .lp font-size of the
   focused leaf or ->@<centre x,y of the focused pane's content or ->] — the
   phase asserts the MEASURED size, not our belief, and wheels at the PAINTED
   centre (note body or graph canvas), never a literal */
function qfsTok() {
  const g = typeof fg === "function" ? fg() : null;
  const el = g && (g.lp && g.lp.offsetParent ? g.lp : g.preview && g.preview.offsetParent ? g.preview : null);
  const px = el ? getComputedStyle(el).fontSize : "-";
  const cr = g && g.content ? g.content.getBoundingClientRect() : null;
  const at = cr && cr.width ? Math.round(cr.left + cr.width / 2) + "," + Math.round(cr.top + cr.height / 2) : "-";
  return " [qfs:" + qfsSize + "," + (qfsAct ? 1 : 0) + "," + qfsSteps + "," + px + "@" + at + "]";
}
/* R14 undo close tab — newest last. tabclose: an entry is now an OBJECT
   {name, text, cause}, not a bare name, because the stack is also this app's
   only rescue buffer. A tab removed by the watcher cannot be flushed (the file
   is gone; R11.4 says a write here would resurrect it), so the bytes that were
   in the editor are parked HERE with the name, and Ctrl+Shift+T brings them
   back. text === null means "nothing was at risk, reopen from disk". */
let closedTabs = [];
const ucPush = (name, text, cause) => {
  closedTabs.push({ name: String(name), text: text == null ? null : String(text), cause: cause || "user-close" });
  if (closedTabs.length > 20) closedTabs.shift();   // a rescue buffer, not a journal
};
/* census: how deep the stack is, and how many entries carry rescued BYTES —
   [uc:<depth>/<rescued>]. A driver that triggers a removal asserts the rescue
   in the same read as [tg:], and a reviewer can see at a glance that nothing
   was silently dropped. */
function ucTok() {
  const r = closedTabs.filter(e => e.text != null).length;
  return " [uc:" + closedTabs.length + "/" + r + "]";
}
/* Ctrl+Shift+T. Reopening a tab whose bytes were RESCUED is the other half of
   the promise made at removal time: the keystrokes come back, and they come
   back where the user can see them.
     * the name is FREE (the watcher's delete took the file) -> the rescued
       bytes are written back under the old name, by create_note, whose
       create_new(true) makes "was it free?" and "write it" ONE step. This is a
       USER action, not an autosave: R11.4 forbids a timer resurrecting a
       deleted file behind the user's back, not the user asking for their text
       back.
     * a file IS there and differs -> it is left ALONE and the rescued bytes go
       into the buffer, which then reads dirty against the disk bytes. The user
       decides; nothing of either version is destroyed. */
async function undoCloseTab() {
  const e = closedTabs.pop();
  if (!e) return false;
  /* "is anything on disk under that name?" is NOT a read_note question: a
     missing note reads back as the EMPTY STRING (src-tauri/src/main.rs:652,
     `unwrap_or_default`), so a `disk == null` test is never true and 25fe7a0's
     rescue silently wrote nothing — gate 25fe7a0 died on exactly that
     ("the rescue lost the keystrokes: ZTC-Del2 came back without TC-KEEP-2",
     hz-artifacts/tabclose/phase-25fe7a0-red.log).
     create_note IS the question: it opens with create_new(true) (main.rs
     create_note_in), so it either WRITES the rescued bytes because the name was
     free, or it fails with EXISTS because a different file owns it. One atomic
     call, no TOCTOU window between the look and the write. */
  let wrote = false, disk = null;
  if (e.text != null) {
    /* THROUGH createNote, not around it: f20 pins ONE backend create-note
       invocation in this file — the literal is deliberately NOT spelled here,
       because f20 counts OCCURRENCES OF THE STRING, so a comment that quotes
       it reddens the gate with a call site that does not exist (measured
       2026-09-19: gate rc=1, cargo-test, left 2 right 1, on this very comment)
       (main.rs f20_no_code_path_materializes_a_heading
       _at_creation) so the empty default body has exactly one home. The helper
       already answers the only question the rescue has — "ok" = the name was
       free and the bytes are down, "exists"/"err" = somebody else owns it. */
    wrote = (await createNote(e.name, e.text)) === "ok";
    if (!wrote) { try { disk = await inv("read_note", { name: e.name }); } catch (_2) { disk = null; } }
  }
  await openInTab(e.name);
  const g = fg();
  if (e.text != null && !wrote && disk !== e.text && curOf(g) === e.name && g.active >= 0) {
    await reloadInPlace(g, e.text);
    g.tabs[g.active].base = disk == null ? "" : disk;   // base = the bytes on disk -> the tab reads DIRTY, honestly
  }
  updateTitle();
  return true;
}
const CMDS = [
  ["app:open-settings",        "Open settings",                       ["ctrl+,"],               () => cmdSettings()],
  // R14: the theme switch is a registry entry like any other — no bespoke
  // keystroke, no menu item of its own. It ships with NO default chord on
  // purpose: the palette is the road, so the smoke phase has to drive the real
  // Ctrl+P path (and a user can still bind a chord in Settings ▸ Hotkeys,
  // which works for free because this is in the one registry).
  ["theme:switch",             "Toggle light/dark mode",              [],                       cmdToggleTheme],
  /* THE "Use theme: <name>" ENTRIES ARE NOT HERE ANY MORE — see cpItems().
     They used to be one static registry row per PALETTES id. A theme is a
     directory in the vault now (.obsidian/themes/), discovered at runtime and
     different per vault, so a static row per theme is the one shape this
     cannot have: the registry is fixed at load, and the vault is not. They are
     built where the palette is opened, from the SAME listed set the Appearance
     dropdown offers, and they call the SAME chooseVaultTheme (item 7: one
     selection function, two routes onto it).
     The consequence, stated rather than hidden: a theme command cannot carry a
     user hotkey, because Settings ▸ Hotkeys binds registry ids and there is no
     stable id for "a directory that may not exist in the next vault". The mode
     toggle below IS a registry row and keeps its binding. */
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
  ["app:open-help",            "Open help",                           ["f1"],                   () => window.open("https://github.com/jpzk/opensidian#readme")],
  ["editor:open-link-in-new-leaf", "Open link under cursor in new tab", ["ctrl+enter"],         async () => { const n = linkAtCaret(); if (!n) return; const g = fg(); await flushSave(g); g.tabs.push(mkTab(n)); g.active = g.tabs.length - 1; await loadActive(g); }],
  ["editor:open-link-in-new-split", "Open link under cursor to the right", ["ctrl+alt+enter"], async () => { const n = linkAtCaret(); if (n) await splitWith(fg(), "row", mkTab(n)); }],
  ["switcher:open",            "Open quick switcher",                 ["ctrl+o"],               () => cmdQuickSwitch()],
  ["workspace:edit-file-title","Rename file",                         ["f2"],                   cmdRename],
  ["editor:save-file",         "Save current file",                   ["ctrl+s"],               cmdSave],
  ["global-search:open",       "Search in all files",                 ["ctrl+shift+f"],         () => { if (!sideOpen) cmdToggleSide(); setPane("search"); }],
  // R26.1: find INSIDE the note. Stock's id and name, and stock's chord — the
  // vault-wide search above (R25) is a different requirement in a different
  // pane and keeps Ctrl+Shift+F.
  ["editor:open-search",       "Search current file",                 ["ctrl+f"],               () => fOpen(fg())],
  ["editor:open-search-replace", "Search & replace current file",     ["ctrl+h"],               () => fOpenRep(fg())],
  ["editor:toggle-bold",       "Toggle bold",                         ["ctrl+b"],               () => edWrap("**")],
  ["editor:toggle-checklist-status", "Toggle checkbox status",        ["ctrl+l"],               () => edTask()],
  // listtoggle R1: stock's ids and names, NO default chord (Settings ▸ Hotkeys
  // reads Blank on stock, docs/recon-listtoggle Q1 q01-ids). The 5th field is
  // the palette's visibility test: stock hides both in reading view — palette
  // "Toggle bullet list" -> no command, no write [Q14 q14-reading].
  ["editor:toggle-bullet-list",   "Toggle bullet list",               [],                       () => edList("bullet"),   edEditable],
  ["editor:toggle-numbered-list", "Toggle numbered list",             [],                       () => edList("numbered"), edEditable],
  ["editor:toggle-comments",   "Toggle comment",                      ["ctrl+/"],               () => edWrap("%%", "comment")],
  ["editor:toggle-italics",    "Toggle italic",                       ["ctrl+i"],               () => edWrap("*")],
  ["markdown:toggle-preview",  "Toggle reading view",                 ["ctrl+e"],               () => cmdToggleMode()],
  ["editor:toggle-source",     "Toggle Live Preview/Source mode",     [],                       () => cmdToggleSource()],
  ["workspace:undo-close-pane","Undo close tab",                      ["ctrl+shift+t"],         async () => { await undoCloseTab(); }],
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
].map(([id, name, def, run, when]) => ({ id, name, def, run, when }))
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
/* palette source: the registry w/ current hotkey hints, PLUS one "Use theme:
   <name>" item per theme this vault actually offers (themeone item 7).
   The theme items are built here, at open time, for the reason the comment in
   CMDS gives: the set is the vault's, not the build's. They are appended after
   the registry so every fixed command keeps the position a user has learned,
   and they carry no hint because they carry no chord.
   `(Default)` is offered like any other, and is stock's "" — the absence of a
   theme, not a theme named Default; chooseVaultTheme("") is what the dropdown
   sends for the same row, which is the point: ONE function, and neither route
   can drift into a second definition of what selecting a theme means. */
function cpItems() {
  const themes = [["Default", ""]].concat(vaultThemesScan.listed.map(n => [n, n]));
  return CMDS.filter(c => !c.when || c.when()).map(c => ({ label: c.name, hint: hkChords(c).map(chordLabel).join(", "), run: c.run }))
    .concat(themes.map(([label, name]) => ({
      label: "Use theme: " + label,
      hint: "",
      run: () => chooseVaultTheme(name),
    })));
}
function cmdPalette() {
  modalKind === "cp" ? closeModal() : openModal("cp", cpItems);
}

/* m5 F2 rename: inline prompt over the focused note tab; the disk move and the
   consented link rewrite are renameThenAsk's (R24.3), the same tail the inline
   title uses, then tabs/hist/mru follow the name */
async function applyRename(old, nn) {   // post-rename bookkeeping (F2 / cmdRename, the R34 title rename, and R24.6's drag-move)
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
     .obsidian/bookmarks.json (move_note_in). bmCache is a COPY of that file taken
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
   opensidian's own invention, not parity, and it is the riskiest write in the
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
  // R24.3: the SAME tail as the title road — move, then ask if anything links
  // in. This used to be `rename_note` (move + a silent vault-wide rewrite);
  // a refusal now reaches the notice banner instead of being swallowed, which
  // is R34.12's rule and was always the right one for this road too.
  await renameThenAsk(old, nn);
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
  if (delPending) return;                  // R24.7: so does the delete confirmation (Escape there = Cancel)
  if (bmEdit) return;                      // R4X.7: and the Edit bookmark modal (its own handler: Escape = Cancel, Enter = Save)
  if (bmAdd) return;                       // bmactive: and its Add mode (Escape = Cancel, Enter = Save, typing = the Title input)
  if (titleEditing()) return;              // R34.1: so does the title box (a filename contains chords)
  if (e.key === "Escape") {
    if (menuEl) { closeMenu(); return; }   // bmmenu: stock closes an open context menu on Escape (recon 15-escape.png); the guards above keep settings' own Escape (R14) first
    if (modalKind) { closeModal(); return; }
    if (!$("rnbox").hidden) { $("rnbox").hidden = true; updateTitle(); return; }
    /* R26.14 FROM THE NOTE. The bar's own two inputs close it in their own
       handlers (and put the caret on the match). But focus does not stay in the
       bar: undo, a click in the text, and replace-all itself all hand the
       keyboard back to the editor, and Escape is still the key that closes the
       find bar there — that is the panel this clones, and a bar that can only be
       dismissed by first clicking back into it is a trap.
       The caret is NOT moved on this path (fClose's atMatch=false): the user is
       already somewhere in the text, and dragging them back to a match they have
       since left would be a jump, not navigation. */
    {
      const gf = fg();
      if (gf && gf.find && gf.find.open) { fClose(gf, false); return; }
    }
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
$("fecabtn").onclick = () => feCollapseAll();   // collapseall R3/R4: explorer header, 5th slot
$("bmcabtn").onclick = () => bmCollapseAll();   // collapseall R3/R4: bookmarks header, 3rd of 4
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
// graph-webgl: draw-path preference. env OPENSIDIAN_GRAPH_RENDERER=gl|2d (backend) beats the
// hidden localStorage setting opensidian.graphRenderer; OPENSIDIAN_GRAPH_LOSE_CTX=1 is the smoke
// hook that loses the GL context once the sim has settled (fallback must keep drawing).
let graphPrefP = null;
const graphRendererPref = () => graphPrefP || (graphPrefP = inv("graph_renderer_pref").catch(() => null).then(p => {
  const r = (p && p.renderer) || localStorage.getItem("opensidian.graphRenderer");
  return { renderer: r === "gl" || r === "2d" ? r : null, loseCtx: !!(p && p.lose_ctx) };
}));
function showEditor(g) {
  g.graphOn = false; g.graphRefresh = null; g.graphRc = null; cancelAnimationFrame(g.sim); g.graphRenderer = null;
  if (g.ro) { g.ro.disconnect(); g.ro = null; }
  if (g.attrObs) { g.attrObs.disconnect(); g.attrObs = null; }
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
  // COLOUR RESOLUTION — DESIGN §9. A custom property's computed value is the AUTHOR'S TEXT
  // after var() substitution, not a colour: the browser never parses it, because a custom
  // property has no type. Our own assets type their colours as hex; the committed Solarized
  // fixture types the same colour in the functional notation; a theme downloaded tomorrow may
  // type a named colour, a hue-based notation, or a mix. Every one of those is a legal CSS
  // colour and the theme MEANT it, so the graph must paint it.
  //
  // This used to be a six-line hex parser (`parseInt(x.slice(1,3), 16)`), which is why the
  // criterion-4 pixel phase caught a BLACK canvas under Solarized: a functional-notation token
  // sliced as if it were hex yields NaN per channel, gl.clearColor(NaN, NaN, NaN, 1) clamps to
  // zero, and the graph cleared to black on a theme whose background is dark cyan. The token
  // had moved — the census proved it — and the pixel had not. A hand-written parser can only
  // ever cover the syntaxes its author thought of; the browser already has the whole grammar.
  //
  // So: ASK THE BROWSER. A hidden probe span (in the document — a detached element has no
  // computed style) takes the text as `color`, and getComputedStyle serialises it back in the
  // one legacy notation this file reads, with 0..255 channels. CSS.supports() gates the
  // assignment so an unparseable value is REFUSED BY NAME instead of silently leaving the
  // previous colour behind. The 2D path wants a string, the GL path wants 0..1 floats: both
  // come from this ONE resolution, so the two renderers cannot disagree about a colour.
  const LEGACY = /^rgba?\s*\(/;                    // the serialisation this file reads back
  const probe = document.createElement("span");
  probe.style.cssText = "position:absolute;left:-9999px;top:0;width:0;height:0;visibility:hidden;pointer-events:none";
  const cssColor = raw => {                        // theme text -> the browser's own serialisation, or null
    const s = (raw || "").trim();
    if (!s || !(window.CSS && CSS.supports && CSS.supports("color", s))) return null;
    probe.style.color = "";                        // never let a refused value keep the previous one
    probe.style.color = s;
    const out = getComputedStyle(probe).color.trim();
    return out && LEGACY.test(out) ? out : null;   // a wide-gamut serialisation is not read here
  };
  const chan01 = s => {                            // that serialisation -> [r,g,b] in 0..1 for GL
    const m = String(s).match(/-?\d*\.?\d+/g);
    if (!m || m.length < 3) return null;
    const c = i => Math.min(1, Math.max(0, parseFloat(m[i]) / 255));
    return [c(0), c(1), c(2)];
  };
  // GRAPH PALETTE. The graph is a <canvas>: it cannot inherit a colour the way every
  // other surface does, it has to ASK for one. It used to hold four hex literals — a
  // second palette that no theme could reach, so a light theme would have left the
  // graph painting dark-theme blue on white. These read the SAME tokens the stylesheet
  // defines, off document.body, so there is exactly one definition of each colour.
  //
  // OFF BODY, NOT documentElement — DESIGN §3 cause (1), and the single reason the
  // graph was not themed at all. A theme's declarations land on `body.theme-dark` /
  // `body.theme-light` (T6 measured stock putting the mode class on BODY, and our
  // stylesheet now declares there too); custom properties inherit, so a lookup on
  // <body> sees the theme's value when it exists and our :root-side fallback when it
  // does not, while a lookup on <html> can only ever see ours. T9 confirmed a
  // <canvas> cannot be painted by a custom property at all — stock READS the computed
  // value and fills with it — so the element we read off is the whole contract.
  // bg is the fifth: --graph-bg, the canvas background on BOTH draw paths (the 2D fill
  // and the WebGL clear colour) — ui/graph-gl.js holds no colour of its own.
  //
  // CACHED PER (MODE x ACTIVE THEME), not per node: getComputedStyle forces a style
  // resolution and draw() runs at up to 60 Hz over every node, so the lookup happens
  // once per draw() and only re-reads when a root attribute actually changes (R13
  // graph_draw budget).
  // THE KEY NAMES BOTH AXES — every axis the stylesheet selects a token block on, or
  // the cache IS a second, stale palette. It was data-theme alone (goal graphtheme,
  // D1): a palette switch with the mode held kept the previous colours until the next
  // mode flip, because nothing in the key had changed. themeone deletes the palette
  // axis and replaces it with the vault THEME, whose identity does not live in an
  // attribute of its own accord — injecting <style id="vault-theme"> moves nothing on
  // the root. data-vtheme is written for exactly this (see vthemeMark), and carries a
  // GENERATION so a hot reload of the same theme's bytes is a new key too (cause 3).
  // RGB is keyed by the colour STRING, so it is rebuilt with the palette — a stale key
  // would hand the GL path `undefined` and paint nothing. RGB is ONE object, emptied in
  // place, never reassigned: `RGB[palette().bg]` evaluates the base RGB BEFORE the call,
  // so a reassigning palette() handed the warm-up frame the old, empty map (undefined bg,
  // draw threw, no GL renderer, every graph phase dead — d374a32).
  const PAL_VAR = { hi: "--accent-yellow", ctr: "--accent-green", node: "--accent-blue", edge: "--border", bg: "--graph-bg" };
  const RGB = {};
  let pal = null, palKey = null;
  const palette = () => {
    const ds = document.documentElement.dataset, key = (ds.theme || "") + "|" + (ds.vtheme || "");
    if (pal && palKey === key) return pal;
    const cs = getComputedStyle(document.body), p = {};   // item 5: the tokens live on body now — DESIGN §3 cause (1)
    const csRoot = getComputedStyle(document.documentElement);   // our own token block: the DEFINED fallback
    for (const k in RGB) delete RGB[k];
    document.body.appendChild(probe);                     // in the document only while we resolve
    for (const k in PAL_VAR) {
      // the theme's text first; if the browser refuses it (a syntax it does not know, an
      // empty token, a wide-gamut serialisation), fall back to the SAME token off :root,
      // which is our own block and always plain. Never empty, never transparent (DESIGN §7).
      const themeText = cs.getPropertyValue(PAL_VAR[k]).trim();
      const v = cssColor(themeText) || cssColor(csRoot.getPropertyValue(PAL_VAR[k]).trim()) || themeText;
      p[k] = v; RGB[v] = chan01(v);                       // null only if BOTH were unreadable
    }
    probe.remove();
    palKey = key; pal = p;
    return p;
  };
  let glr = null, reason = "default";
  if (pref.renderer === "2d") reason = "forced:2d";
  else if (!window.GraphGL) reason = "no-module";
  else {
    if (!g.glr || g.glr.lost) {
      if (g.glcv) g.glcv.remove();
      g.glcv = document.createElement("canvas"); g.glcv.className = "graphgl";
      cv.parentNode.insertBefore(g.glcv, cv);
      const P0 = palette();   // read the tokens FIRST, then index RGB — see the note above palette()
      g.glr = GraphGL.create(g.glcv, () => { if (g.glLost) g.glLost(); }, RGB[P0.bg]);   // the warm-up frame clears to the token too
    }
    glr = g.glr;
    if (!glr) reason = "no-webgl";
  }
  const gpu = glr ? { gpu_vendor: glr.info.vendor, gpu_renderer: glr.info.renderer } : { gpu_vendor: "", gpu_renderer: "" };
  cv.classList.toggle("gl-on", !!glr); if (g.glcv) g.glcv.hidden = !glr;
  g.graphRenderer = glr ? "gl" : "2d";   // census [gl:] — which draw path painted the shot
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
  let nArr = new Float32Array(0), eArr = new Float32Array(0);
  function draw() {
    const dT0 = perf.now();
    const P = palette();               // one token read per frame, none per node
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    // background: the 2D path PAINTS --graph-bg (a transparent canvas would show the pane behind it,
    // which happens to be the same token today and is not a contract); over WebGL the 2D canvas only
    // carries labels, so it stays clear and the GL clear colour below is the one that paints.
    if (glr) ctx.clearRect(0, 0, cv.width, cv.height);
    else { ctx.globalAlpha = 1; ctx.fillStyle = P.bg; ctx.fillRect(0, 0, cv.width, cv.height); }
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
      glr.draw(cv.width, cv.height, view, nArr, nc, eArr, ec, RGB[P.bg]);   // bg: the clear colour, from the token like every other colour here
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
  // THEME / PALETTE -> REPAINT (goal graphtheme). The loop stops once the sim is quiet (CPU -> 0),
  // so a settled graph paints nothing until something wakes it — a mode or palette switch used to
  // leave the old colours on screen until the next hover or pan, whatever the cache did. The graph
  // watches the two root attributes its tokens are selected by and redraws ONCE per change: no
  // reheat, the layout is untouched, only the colours are re-read (palette() sees the new key).
  // Torn down with the sim, the way g.ro is: a retired sim must not paint over its successor.
  if (g.attrObs) g.attrObs.disconnect();
  const obs = new MutationObserver(() => {   // `obs`, not g.attrObs: by the time a retired sim's callback runs, g.attrObs is its successor's
    if (g.simGen !== gen || !g.graphOn) { obs.disconnect(); if (g.attrObs === obs) g.attrObs = null; return; }
    redraw();
  });
  obs.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "data-vtheme"] });
  g.attrObs = obs;
  // graph-webgl: context lost -> this sim swaps to the 2D path for good (a later open gets a fresh gl canvas)
  g.glLost = () => {
    if (g.simGen !== gen || !glr) return;
    glr = null; cv.classList.remove("gl-on"); if (g.glcv) g.glcv.hidden = true;
    g.graphRenderer = "2d";
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
  if (vaultPath) { try { updateTitle(); } catch (_) {} }   // brand: publish [precent:] once the rows exist (async); never at the boot picker, whose window must stay untitled by the census
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
  // tabclose: every tab of the OLD layout disappears on the next line, without
  // passing dropTab or closeTab. Unrecorded, this path could account for any
  // number of "it closed by itself" reports, so it names itself too.
  if (state) for (const h of groups()) {
    for (const t of h.tabs) tabGone("session-replace", t, { dirty: false, flushed: h.flushedAt !== undefined, via: "enterVault", tabsLeft: 0 });
    clearTimeout(h.saveT); h.saveT = null;
  }
  // ...including the process-wide layout timer, and the dedupe/id memory that
  // belongs to the file we just stopped looking at. Disarm only — a route that
  // reached here WITHOUT leaveVault has no vault left to flush into safely.
  if (wsT) { clearTimeout(wsT); wsT = null; }
  wsLast = ""; wsIds = {};
  $("vswitch").innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>' + base(vaultPath);
  collapsed = new Set();
  bmFolds = new Set();               // collapseall: folds are per vault and in memory (R6)
  feCaFlag = "E"; bmCaFlag = "C";    // collapseall Q5: the empty-pane label flags start where stock's do
  // R28.7: read the layout BEFORE the first render. renderLayout -> updateTitle
  // -> wsTouch arms a 400ms write, so reading later would race a write of the
  // empty boot layout over the file we came here to restore.
  const wdoc = await wsRead();
  const g = mkGroup();               // M6: one group, wrapped in a one-leaf split tree
  state = { root: { dir: "row", children: [g], fractions: [1] }, focused: null };
  renderLayout();
  focusGroup(g);
  hideAc();
  edtBad = Ed.selfTest();            // R17: renderer + token map invariants -> [edt:] census
  await loadVaultCss();              // themefs R3: the vault's own snippet CSS, before first paint of a note
  await refreshTree();
  await refreshBm();                 // R9.4: menu label needs the cache early
  const names = await inv("list_notes");
  // R28.7 / R28.13: the restore, or — if there is no file, it is corrupt, or
  // nothing in it survived the degrade rule — the ordinary first-launch path.
  // Both land on a usable window; that is the whole point of R28.13.
  if (!await wsApply(wdoc, names)) {
    if (names.length) await openInTab(names[0], "boot");
    else renderTabs(g);
  }
  perf.mark("boot", 0, { notes: names.length });   // perf: page start -> vault ready (first note rendered)
  wsGeomTouch();   // R28.2: the rectangle is recorded once per session OUTSIDE the vault, resize or no resize
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
// — but NOT without the bytes: whatever was typed inside the debounce is parked
// in the undo-close buffer (ucPush) before the timer dies, so "cannot write it
// back" stops meaning "throw it away". Ctrl+Shift+T returns it.
// tabclose: `cause` is external-delete or external-rename — the watcher is the
// ONLY caller, and which of the two it is decides the whole diagnosis (S1 vs
// S2/S3), so it is not inferred here, it is passed in by the caller that knows.
async function dropTab(g, i, cause, via) {
  const t = g.tabs[i];
  // The dirty read must happen BEFORE clearTimeout: this is the exact line
  // where the pending save dies, and the log line is the only record that
  // anything was in flight when it did.
  const wasDirty = i === g.active && !t.kind ? bufOf(g) !== t.base : false;
  // THE F-CLASS FIX (criterion 4). The bytes cannot go to DISK here — the file
  // is gone and writing it back is the resurrect R11.4 exists to forbid — so
  // they go to the undo-close buffer, which is the one place the user can ask
  // for them back (Ctrl+Shift+T). Read the buffer BEFORE the timer dies, for
  // the same reason `wasDirty` is read here: two lines down there is nothing
  // left to read. A CLEAN tab parks its name only; there is nothing at risk.
  const rescued = wasDirty ? bufOf(g) : null;
  if (!t.kind) ucPush(t.name, rescued, cause || "external-delete");
  tabGone(cause || "external-delete", t,
    { dirty: wasDirty, flushed: false, preserved: rescued != null, via: via || "dropTab", tabsLeft: g.tabs.length - 1 });
  if (i === g.active) { clearTimeout(g.saveT); g.saveT = null; g.lpActive = null; }
  unlinkTab(g, g.tabs[i], true);    // R13.4
  dropView(g.tabs[i]);
  g.tabs.splice(i, 1);
  if (!g.tabs.length && groups().length > 1) return collapseGroup(g, "dropTab");
  if (g.active >= g.tabs.length) g.active = g.tabs.length - 1;
  else if (i < g.active) g.active--;
  await loadActive(g);
}
let vcCount = 0;                               // census [vc:N] — events handled
async function onVaultChanged(c) {
  vcCount++;
  // tabclose: `gone` used to be one undifferentiated Set, so the log could not
  // tell a DELETE from a rename the watcher paired by content (S2) — which is
  // the single most important bit in this whole investigation. Keep the causes
  // apart from here down.
  const renamedFrom = new Set(c.renamed.map(r => r[0]));
  const gone = new Set([...c.removed, ...renamedFrom]);
  const mod = new Set(c.modified);
  for (const g of groups()) {
    for (let i = g.tabs.length - 1; i >= 0; i--) {   // R11.4: delete/rename closes the tab
      const t = g.tabs[i];
      if (!t.kind && gone.has(t.name))
        await dropTab(g, i, renamedFrom.has(t.name) ? "external-rename" : "external-delete", "onVaultChanged#" + vcCount);
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
window.__TAURI__.event.listen("vault-css-changed", e => onVaultCssChanged(e.payload));  // themefs item 6: hot reload
// R31.1: a real OS drop arrives here, from Rust, never from a DOM drop event.
window.__TAURI__.event.listen("drop-files", e => attachDrop(e.payload || []));

$("vswitch").onclick = showPicker;

(async () => {
  await bootTheme();             // the root attribute is set synchronously inside
                                 // (system default), then the STORED choice replaces
                                 // it — one call site decides the boot theme.
                                 // (item 8: bootPalette's second axis is gone; the
                                 // vault's own cssTheme is applied by loadVaultCss
                                 // when a vault opens, which is where it belongs —
                                 // a theme is a property of the vault, not the app.)
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
/* THE PALETTE CONTROL'S CENSUS TOKEN IS GONE (themeone item 7 / C1). A
   `spal` token used to publish the PALETTE dropdown's measured rect on
   Appearance > Current community themes. That control is deleted — not
   hidden, not disabled: there is no second theme control to click, so there
   is no geometry to publish and no token to read. The ONE theme control
   publishes svtTok below, and a census carrying exactly one such control
   token is how criterion 1 is asserted without OCR. The old token's name is
   deliberately not written out here in its bracketed form: criterion 1 is
   checked by grepping this tree for it, and a comment that spells it would
   answer that grep with a line about its own absence. */
/* [svt:<centre x>,<centre y>,<label>] — the vault-theme picker's control
   (Appearance ▸ Themes, themefs R5), and since themeone item 7 the ONLY theme
   control the pane publishes: the phase that proves the picker must CLICK its
   measured rect and assert the shown label without OCR. */
function svtTok() {
  const d = document.getElementById("svtheme");
  if (!d) return "";
  const b = d.getBoundingClientRect();
  const lbl = String(d.textContent || "").replace(/[[\]|]/g, "").slice(0, 40);
  return " [svt:" + Math.round(b.left + b.width / 2) + "," +
         Math.round(b.top + b.height / 2) + "," + lbl + "]";
}
/* [ssn:<centre x>,<centre y>,<label>] — the CSS-snippets control (Appearance
   ▸ CSS snippets, themefs R3), published like [svt:] and for the same
   reason: the phase that proves the toggle must CLICK the control's measured
   rect and assert its live "<n> enabled" label without OCR.
   (A third token of this shape used to sit beside them for the palette
   dropdown; it went with the control in themeone item 7, and its name is not
   spelled here on purpose — C1 is checked by grepping this tree for it.) */
function ssnTok() {
  const d = document.getElementById("ssnips");
  if (!d) return "";
  const b = d.getBoundingClientRect();
  const lbl = String(d.textContent || "").replace(/[[\]|]/g, "").slice(0, 40);
  return " [ssn:" + Math.round(b.left + b.width / 2) + "," +
         Math.round(b.top + b.height / 2) + "," + lbl + "]";
}
/* setcenter R3 — the settings modal's PUBLISHED geometry, one census line of
   its own (ui/census.js), emitted only while settings is open. Every phase
   that clicks inside the box reads these instead of a literal pixel, so the
   box can move (centred, viewport-relative, R1) without a phase noticing.
     [sbox:x,y,w,h]   #sbox's rect                    (A2 asserts its margins)
     [snav:x,y,w,h]   #snav, the scrolling nav column (OCR crop, wheel target)
     [srow1:ctlx,cy,bodyx]   rows panes: the first .srow's control-cell centre
                      x, its centre y, and its body (.sinfo) centre x
     [shkf:cx,cy]     Hotkeys: the filter input
     [shkc:cx,cy]     Hotkeys: the Conflicts chip (only while it is shown)
     [shk<N>:x,y,w,h,addx,rsx,xx]   Hotkeys rows 1-2: the row rect, then the
                      centre x of its add ⊕, restore ↺ and LAST chip's ✕
                      (0 = no chip); the centre y is y+h/2
   All rounded getBoundingClientRect, i.e. viewport px == screen px for the
   gate's window at +0+0. */
function sgeoTok() {
  if (!settingsOpen) return "";
  const R = el => el ? el.getBoundingClientRect() : null;
  const cx = b => Math.round(b.left + b.width / 2), cy = b => Math.round(b.top + b.height / 2);
  const rc = b => Math.round(b.left) + "," + Math.round(b.top) + "," + Math.round(b.width) + "," + Math.round(b.height);
  const sb = R($("sbox")), nv = R($("snav"));
  let t = " [sbox:" + rc(sb) + "] [snav:" + rc(nv) + "]";
  const row = document.querySelector("#spage .srow");
  if (row) {
    const rb = R(row), cb = R(row.querySelector(".sctl")) || rb, ib = R(row.querySelector(".sinfo")) || rb;
    t += " [srow1:" + (cb === rb ? Math.round(rb.right - 50) : cx(cb)) + "," + cy(rb) + "," + cx(ib) + "]";
  }
  const f = $("hkfilter");
  if (f && f.isConnected) {
    t += " [shkf:" + cx(R(f)) + "," + cy(R(f)) + "]";
    const cc = document.querySelector("#hkchips .hkchip.conf");
    if (cc) t += " [shkc:" + cx(R(cc)) + "," + cy(R(cc)) + "]";
    const rows = document.querySelectorAll("#hklist .hkrow");
    for (let i = 0; i < 2 && i < rows.length; i++) {
      const xs = rows[i].querySelectorAll(".hkx");
      t += " [shk" + (i + 1) + ":" + rc(R(rows[i])) + "," + cx(R(rows[i].querySelector(".hkadd"))) + "," +
           cx(R(rows[i].querySelector(".hkrestore"))) + "," + (xs.length ? cx(R(xs[xs.length - 1])) : 0) + "]";
    }
  }
  return t;
}
/* [snavl:<child>|<child>|...] (goal/noplugins) — #snav's RENDERED children in
   DOM order, while settings is open: a group heading as "H=<text>", an entry as
   its text, anything else (a separator, a stray node) as "?<className>"; spaces
   become "_". It is read off the DOM, not the model, so the phase sees exactly
   what the nav paints: which entries exist, which headings head them, and
   whether a heading (or a dangling separator) is left with nothing under it. */
function snavlTok() {
  if (!settingsOpen) return "";
  const n = $("snav");
  if (!n) return "";
  const out = [];
  for (const c of n.children) {
    const t = String(c.textContent || "").trim().replace(/[[\]|]/g, "").replace(/\s+/g, "_");
    if (c.classList.contains("snavh")) out.push("H=" + t);
    else if (c.classList.contains("snavi")) out.push(t);
    else out.push("?" + (c.className || c.tagName.toLowerCase()));
  }
  const a = document.activeElement;
  const f = a && a.classList && a.classList.contains("snavi") ? a.dataset.pane :
            a ? (a.tagName.toLowerCase() + (a.id ? "#" + a.id : "")) : "-";
  return " [snavl:" + out.join("|") + "] [snavf:" + f + "]";
}
/* [snavf:] = where keyboard focus sits while settings is open (pane id of a focused
   nav entry, else tag#id). Instrument only: repaint the census on focus moves so a
   phase can assert keyboard navigation of the nav. */
document.addEventListener("focusin", () => { if (settingsOpen) updateTitle(); });
/* [sqf:<centre x>,<centre y>,<on 1|0>] — the Quick font size toggle (Appearance
   ▸ Font, fontwheel REQ-1), published like [ssn:] so the phase clicks its
   measured rect and reads its drawn state. */
function sqfTok() {
  const d = document.getElementById("sqfs");
  if (!d || !d.isConnected) return "";
  const b = d.getBoundingClientRect();
  return " [sqf:" + Math.round(b.left + b.width / 2) + "," + Math.round(b.top + b.height / 2) + "," +
         (d.classList.contains("on") ? 1 : 0) + "]";
}
function setTok() {
  if (!SMODEL) return "";
  const e = SMODEL.rows.reduce((n, r) => n + (r.enabled ? 1 : 0), 0);
  return " [set:" + SMODEL.nav.length + "/" + SMODEL.rows.length + "/" + e + "]" +
         snavlTok() +
         " [spane:" + sPane + "/" + sRowsShown + "/" + sEnabledShown + "]" +
         svtTok() + ssnTok() + slbTok() + sqfTok() +
         (sfpMs >= 0 ? " [sfp:" + sfpMs + "/" + sfpMax + "/" +
                       (Math.round(sfpSum / sfpN * 100) / 100) + "/" + sfpN + "]" : "") +
         (sfpW >= 0 ? " [sfpw:" + sfpW + "/" + sfpWMax + "]" : "");
}
async function openSettings() {
  if (!SMODEL) await smodelPrefetch();   // cold open only (prefetched at boot)
  try { slbOn = !!(await inv("strict_line_breaks")); } catch (err) { slbOn = false; }   // goal/linebreak: disk is the truth
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
/* the left nav, stock's order and grouping (Options 1-7; the Core/Community plugins
   entries and the Core plugins group are gone, goal/noplugins) — built ONCE from the model, never re-created on a tab
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
/* `id` is a pane id from the model ("general", "hotkeys", ...);
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
    if (!rows.length) {                     // fallback only: since goal/noplugins every nav pane has rows
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
   separator, controls at the card's right edge. The palette stays opensidian's
   dark theme — that delta is recorded in R30. */
const SDIS_TITLE = "Not implemented yet";
/* THE PALETTE CONTROL IS DELETED (themeone item 7 / C1 / R1). A second live
   dropdown used to be built here for Appearance > Current community themes:
   paletteCtl / renderPaletteCtl / openPaletteMenu, the settings-UI route onto
   the palette axis. Deleted rather than hidden — settings.rs's BACKED table
   no longer names that row, so it renders the way every other unimplemented
   row does (stock's inert status line), and this pane now publishes EXACTLY
   ONE theme control: themeCtl, on stock's own Themes row.
   The two routes that reach theme selection — Ctrl+P "Use theme: <name>" and
   that dropdown — share chooseVaultTheme(), so neither can drift into a
   second definition of what selecting a theme means. That was the argument
   for sharing choosePalette() when there were two axes; it is the same
   argument, and now there is one thing to share. */
/* ---- Appearance ▸ CSS snippets: the settings-UI route onto the vault's
   snippet toggles (themefs R3). Same construction as themeCtl and for the
   same reason: the ONE menu widget the census can see ([menu:]), not a native
   popup. The text is the live count, the shape stock's own row shows
   ("0 enabled"); toggling goes through toggleSnippet(), the same function the
   injection path uses, so the pane and the <head> cannot disagree. */
const snipCtlLabel = () => vaultSnipsOn.length + " enabled";
function snipCtl() {
  const d = document.createElement("div");
  d.className = "sctl dropdown live";
  d.id = "ssnips";
  d.tabIndex = 0;
  d.setAttribute("role", "button");
  d.setAttribute("aria-haspopup", "menu");
  d.textContent = snipCtlLabel();
  const open = ev => { ev.preventDefault(); ev.stopPropagation(); openSnipMenu(d); };
  d.onmousedown = ev => ev.stopPropagation();
  d.onclick = open;
  d.onkeydown = ev => { if (ev.key === "Enter" || ev.key === " ") open(ev); };
  return d;
}
function renderSnipCtl() {
  const d = document.getElementById("ssnips");
  if (d) d.textContent = snipCtlLabel();
}
/* ---- Editor ▸ Display ▸ Strict line breaks (goal/linebreak REQ-1/REQ-2,
   docs/linebreak/recon.md). Stock's row, stock's key: the VAULT's
   .obsidian/app.json "strictLineBreaks", default off. The disk is the truth —
   Rust's `render` reads it on every reading render — so this control only
   writes it, mirrors it, and re-renders every open reading view at once
   (stock re-renders without a reopen, REQ-2). slbOn is refreshed on every
   settings open, so a hand edit of app.json shows up on the next open. */
let slbOn = false;
function slbCtl() {
  const d = document.createElement("div");
  d.className = "sctl toggle live" + (slbOn ? " on" : "");
  d.id = "sslb";
  d.tabIndex = 0;
  d.setAttribute("role", "switch");
  d.setAttribute("aria-checked", String(slbOn));
  d.appendChild(document.createElement("i"));
  const flip = async ev => {
    ev.preventDefault(); ev.stopPropagation();
    const want = !slbOn;
    try { await inv("set_strict_line_breaks", { on: want }); }
    catch (err) { say("Strict line breaks: " + String(err && err.message || err)); return; }
    slbOn = want;
    d.classList.toggle("on", slbOn); d.setAttribute("aria-checked", String(slbOn));
    for (const g of groups()) {
      if (isReading(g)) await preview(g);
      // a RETAINED reading view in an inactive tab is swapped back in without a
      // render (R20), so it must re-render on activation (measured: the phase's
      // second pass landed on LB01's retained OFF render)
      for (const t of g.tabs)
        if (!t.kind && t !== g.tabs[g.active] && t.mode === "reading" && t.view && t.view.loaded) t.stale = true;
    }
    updateTitle();
  };
  d.onmousedown = ev => ev.stopPropagation();
  d.onclick = flip;
  d.onkeydown = ev => { if (ev.key === "Enter" || ev.key === " ") flip(ev); };
  return d;
}
/* [slb:<0|1>,<cx>,<cy>] — the toggle's state and centre while it is built
   (Editor pane open), so the smoke phase CLICKS its measured rect. */
function slbTok() {
  const d = document.getElementById("sslb");
  if (!d) return "";
  const b = d.getBoundingClientRect();
  return " [slb:" + (slbOn ? 1 : 0) + "," + Math.round(b.left + b.width / 2) + "," + Math.round(b.top + b.height / 2) + "]";
}
/* ---- Appearance ▸ Themes: the settings-UI route onto the VAULT theme
   (themefs R5) — stock's own row, stock's own semantics: a dropdown showing
   the active cssTheme ("Default" when ""), listing (Default) + EXACTLY the
   oracle-predicate set (DESIGN §9), the excluded dirs rendered inert with
   their reason so the pane shows WHY a broken theme is not offered. Same
   ctxmenu construction as snipCtl, same reason ([menu:] census). */
const themeCtlLabel = () => vaultTheme || "Default";
function themeCtl() {
  const d = document.createElement("div");
  d.className = "sctl dropdown live";
  d.id = "svtheme";
  d.tabIndex = 0;
  d.setAttribute("role", "button");
  d.setAttribute("aria-haspopup", "menu");
  d.textContent = themeCtlLabel();
  const open = ev => { ev.preventDefault(); ev.stopPropagation(); openThemeMenu(d); };
  d.onmousedown = ev => ev.stopPropagation();
  d.onclick = open;
  d.onkeydown = ev => { if (ev.key === "Enter" || ev.key === " ") open(ev); };
  return d;
}
function renderThemeCtl() {
  const d = document.getElementById("svtheme");
  if (d) d.textContent = themeCtlLabel();
}
function openThemeMenu(anchor) {
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  const mk = (txt, on) => {
    const it = document.createElement("div");
    it.textContent = txt;
    it.onmousedown = ev => ev.stopPropagation();
    if (on) it.onclick = on;
    return it;
  };
  m.appendChild(mk((vaultTheme === "" ? "✓ " : "") + "(Default)",
    () => { closeMenu(); chooseVaultTheme(""); }));
  for (const name of vaultThemesScan.listed) {
    m.appendChild(mk((name === vaultTheme ? "✓ " : "") + name,
      () => { closeMenu(); chooseVaultTheme(name); }));
  }
  // inert-with-reason (DESIGN §9): visible, not clickable — no handler
  for (const x of vaultThemesScan.excluded) {
    const it = mk("✕ " + x.dir + " — " + x.reason, null);
    it.style.opacity = "0.5";
    m.appendChild(it);
  }
  const b = anchor.getBoundingClientRect();
  placeMenu(m, Math.round(b.left), Math.round(b.bottom + 4));
}
function openSnipMenu(anchor) {
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  if (!vaultSnips.length) {
    // absent snippets/ is stock's silent normal (T0): an empty menu entry,
    // deliberately not a notice and not a directory-creating button (yet)
    const it = document.createElement("div");
    it.textContent = "(no snippets)";
    m.appendChild(it);
  }
  for (const label of vaultSnips) {
    const it = document.createElement("div");
    it.textContent = (vaultSnipsOn.includes(label) ? "✓ " : "") + label;
    it.onmousedown = ev => ev.stopPropagation();
    it.onclick = () => { closeMenu(); toggleSnippet(label); };
    m.appendChild(it);
  }
  const b = anchor.getBoundingClientRect();
  placeMenu(m, Math.round(b.left), Math.round(b.bottom + 4));
}
let sRowsShown = 0, sEnabledShown = 0;
function sctl(r) {                            // the control cell for one row, or null
  const v = r.default_shown === "-" ? "" : r.default_shown;
  /* THE LIVE DROPDOWNS (themefs R5/R3). Everything else in this pane is a
     transcription of stock's pixels with no handler; a row is rendered live
     only when settings.rs BACKED names its real key (cssTheme /
     enabledCssSnippets — the palette key left that table with its axis,
     themeone item 7). None uses a
     native <select>: a native popup is an OS-level window, invisible to the
     screenshot-and-census rig, so the settings controls that change the
     app's appearance would be the ones no phase could prove. Each opens the
     app's own .ctxmenu instead — the same widget the tab menu uses, published
     in the census as [menu:<labels>] with measured geometry, so the
     settings-UI route is drivable and assertable like every other menu. */
  if (r.key === "cssTheme") return themeCtl();            // themefs R5: stock's Themes row, stock's semantics
  if (r.key === "enabledCssSnippets") return snipCtl();   // themefs R3, same live-control rule
  if (r.key === "strictLineBreaks") return slbCtl();      // goal/linebreak REQ-1: stock's Editor toggle
  if (r.key === "baseFontSizeAction") {                   // fontwheel REQ-1: stock's toggle, live
    const d = document.createElement("div");
    d.className = "sctl toggle" + (qfsAct ? " on" : ""); d.id = "sqfs";
    d.appendChild(document.createElement("i"));
    d.onclick = () => { d.classList.toggle("on", !qfsAct); qfsSetAction(!qfsAct); };   // drawn state first: qfsSetAction repaints the census
    return d;
  }
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
function buildSettingsRows(pg, rows, pane) {
  /* goal/noplugins: the one LIST pane (Core plugins: denser rows + a search
     field) and the CHROME rows (its "(search field)", Community plugins'
     "(security blurb)") went with their panes — no remaining pane has either. */
  pg.className = "rows";
  let section = null, card = null;   // null !== "" so the first row always opens a card
  for (const r of rows) {
    const sec = r.section || "";
    if (sec !== section) {                    // a new section = its own heading + card, like stock
      section = sec;
      if (sec) { const h = document.createElement("div"); h.className = "ssec"; h.textContent = sec; pg.appendChild(h); }
      card = document.createElement("div"); card.className = "scard"; pg.appendChild(card);
    }
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
// [sqf:] (and every other settings rect token) is a PAINTED position: a scrolled
// pane moves it, so the census follows the scroll (scroll doesn't bubble: capture)
$("settings").addEventListener("scroll", () => { if (settingsOpen) updateTitle(); }, true);

/* ---------- R33 frameless window: the app IS the title bar ----------
   With `decorations: false` there is no WM frame, so moving, resizing,
   minimising, maximising and closing the window are this file's job.

   WHY THE GESTURES ARE OURS AND NOT THE WM'S — *WHEN THERE IS NO WM*: the
   obvious implementation hands the press to the WM (start_dragging /
   _NET_WM_MOVERESIZE) and lets it move the window. That does nothing where there
   is NO window manager — which is exactly the state of every Xvfb display this
   repo tests on, and an undecorated window there would be permanently stuck. So
   a gesture sends the ANCHOR rect it started from plus the cursor delta, and
   win_gesture applies the resulting geometry (absolute at every step: a dropped
   pointermove cannot make the window drift).
   Cost: the window follows the cursor one frame late. R33 records the divergence.
   AND THE CONVERSE, WHICH R33.6 MISSED AND AN OPERATOR PAID FOR (R33.6b): where
   a compositor owns the position — any Wayland session, including the app
   running as an XWayland client — a client's own position request is DROPPED,
   so that same path moves the window 0 px and nothing about the arithmetic can
   fix it (docs/recon-hdrdrag/README.md has the measurement). The path is
   therefore chosen at runtime from a positive test of the live session
   (win_move_proto -> wfProto, published as [wfp:]), and BOTH paths ship: see
   "THE LINE THAT CHOOSES THE PATH" in wfBegin.

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
/* R33.6b THE TWO PATHS. "none" = this window can position ITSELF, so the app
   moves it (win_rect anchor + cursor delta + win_gesture) — that is every Xvfb
   rig (nobody else could move it) AND every plain X11 desktop, where E1 measured
   our geometry exact under openbox and a handover measurably WORSE (every other
   press swallowed by the WM's pointer grab). "wm" / "wayland" = MEASURED that a
   client's position request does nothing here, or known a priori on native
   Wayland; then the compositor is the only party that can move the window and
   the press is handed to it.
   CACHED, not awaited per press: the WM-less path must keep the timing it has
   today (panedrag is green because of it), so the press path adds zero IPC.
   "none" is the boot value and the value until something is measured, so the
   fallback is always the behaviour this repo has evidence for, never a hang. */
var wfProto = "none";
/* R33.6b DIAGNOSTIC: what the webview actually receives around a handover.
   Published as [wfd:] so a phase can tell "the press never arrived" apart from
   "the press arrived and the handover did nothing" — the two look identical in
   the geometry alone, and telling them apart is the whole debugging step. */
var wfEv = { dn: 0, up: 0, cx: 0, lc: 0, mvb: 0 };

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
  /* ===== THE LINE THAT CHOOSES THE PATH (R33.6b, docs/negctl-hdrdrag) =====
     wfProto is "none" until the Rust side has MEASURED that this session drops
     a client's position request on the floor (or knows it must, because the
     session is native Wayland). "none" therefore covers both X11 cases — no WM,
     and a WM that lets us position ourselves — and both keep today's anchor+delta
     path, unchanged, which is what E1 measured exact under openbox and what the
     gate rigs depend on. When the path IS "wm"/"wayland", the compositor owns the
     position and our arithmetic is a 0 px no-op, so hand the press over and
     return: no anchor, no delta, no win_gesture, nothing for its drag to fight.
     Resizes are NOT handed over — the eight grips are a different protocol edge
     and are out of this goal's scope.
     KNOWN, MEASURED COST of the handover (docs/recon-hdrdrag/B-ALT.log): the WM
     grabs the pointer for its move loop, so the webview never sees that press's
     release and the NEXT press can be swallowed. That is why the handover is
     spent only where the alternative moves the window 0 px every time. */
  if (dir === "move" && wfProto !== "none") {
    wfLog("h" + wfProto + "#" + ++wfSeq);
    wfLast = "handover:" + wfProto;
    inv("win_drag_start").then(p => { wfProto = p; wfLast = "handover:" + p; wfTitleSafe(); }).catch(noteErr);
    return updateTitle();
  }
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
  /* R33.6b: the path can change UNDER US. win_gesture measures, during a real
     move, whether this session honours a client-set position, and the verdict
     can only arrive after the gesture has been running a moment — so the answer
     is re-read HERE, after the drag, never in the press path (which must keep
     the timing the WM-less rigs are green with). On every session this repo
     gates, the answer is the same "none" it booted with. */
  if (a && a.dir === "move" && wfProto === "none")
    inv("win_move_proto").then(p => { if (p !== wfProto) { wfProto = p; wfLog("p" + p); } wfTitleSafe(); }).catch(noteErr);
}
/* R33.6b: THE PATH PROBES ARE ASYNC, AND THE FIRST ONE RESOLVES DURING BOOT.
   wfArm() runs at top level, before any vault is loaded, so `state` can still be
   null — and updateTitle() reads state.root unguarded. Republishing the census
   straight from a probe callback therefore threw "null is not an object
   (evaluating 'state.root')" into window.onerror, and noteErr latches the FIRST
   error as [jserr:] for the whole session: a first-error-wins channel poisoned
   before the app booted, which hides the next real error from every smoke phase.
   MEASURED, not theorised: phase hdrdragwm's "a plain click on free header space
   is a no-op" assertion went red on that boot-time error, on a click that was
   fine (the window had not moved a pixel) — and the same [jserr:] sat in the
   census of every launch on this branch while main's was clean.
   It is the same scar as the guarded updateTitle() at the end of wfArm, so every
   async path callback republishes through THIS, never through updateTitle. */
function wfTitleSafe() { if (typeof state !== "undefined" && state) updateTitle(); }
function wfArm() {
  // R33.6b: ask ONCE who moves this window, then publish it as [wfp:] — a smoke
  // phase must be able to read the path that was taken instead of inferring it
  // from whether the window ended up somewhere.
  inv("win_move_proto").then(p => { wfProto = p; wfTitleSafe(); }).catch(noteErr);
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
  addEventListener("pointerdown", ev => { wfEv.dn++; if (wfDragRegion(ev.target)) wfBegin("move", ev, ev.target); }, true);
  addEventListener("pointermove", ev => { if (ev.buttons) wfEv.mvb++; wfDrag(ev); }, true);
  addEventListener("pointerup", ev => { wfEv.up++; wfEnd(ev); }, true);
  addEventListener("pointercancel", ev => { wfEv.cx++; wfEnd(ev); }, true);
  // grab-broken -> lostpointercapture is the event that would clear the engine's
  // stuck button state for free if WebKit emits it; count it either way.
  addEventListener("lostpointercapture", () => { wfEv.lc++; }, true);
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
/* census: [wf:<buttons><d|-><grips>] [wfm:<maximised>] [wfk:<focused control>]
   [wfp:<none|wm|wayland>] — which mechanism this session moves the window with
   (R33.6b), read from win_move_proto at arm time, so a phase asserts the PATH
   and not just the outcome.
   `d` = the gesture listeners are armed; without them the window cannot be
   moved, and the smoke's move assertion is the thing that notices. */
function wfTok() {
  const b = document.querySelectorAll("#wframe button").length;
  const g = document.querySelectorAll("#wrz i").length;
  const a = document.activeElement;
  const k = a && a.id && a.id.indexOf("wf-") === 0 ? a.id : "-";
  return " [wf:" + b + (wfArmed ? "d" : "-") + g + "] [wfm:" + (wfMax ? 1 : 0) + "] [wfk:" + k + "] [wfg:" + wfLast + "] [wfl:" + wfLogA.join(">") + "] [wfp:" + wfProto + "] [wfd:" + wfEv.dn + "," + wfEv.up + "," + wfEv.cx + "," + wfEv.lc + "," + wfEv.mvb + "] [hdr:" + wfHdrTok() + "]";
}
wfArm();

/* ================= R26: FIND INSIDE A NOTE (Ctrl+F) =================
   Stock's in-note find bar, cloned. What the requirement rows buy, and where
   each one lives in this block:
     R26.1  Ctrl+F opens the bar and FOCUS LANDS IN THE INPUT      -> fOpen
     R26.2  five elements and no more: input, count, prev, next, close -> fBar
     R26.3  the counter appears WITH the query, live as you type    -> fPaint
     R26.4  case-insensitive substring ALWAYS (no toggles, R26.5)   -> fScan
     R26.7  `0 / 0` when nothing matches — not blank, not hidden     -> fPaint
     R26.8  every match is marked in the note                       -> fMarks
     R26.11 Enter advances, Shift+Enter goes back                   -> fGo
     R26.12 advancing SCROLLS the match into view                   -> fReveal
     R26.13 cycling WRAPS past the last match back to the first     -> fGo
     R26.14 Escape closes and leaves the caret AT the match         -> fClose

   WHERE THE MATCHES LIVE. Hits are computed over the MODEL (Ed.lines), never
   over the painted DOM: live preview hides markup, so a DOM-text search would
   silently disagree with the bytes on disk (and with the replace that follows).
   The MARKS, by contrast, are pure DOM: each hit's run is wrapped in a
   <span class="fmk"> by splitting the row's text nodes. That is safe for every
   other part of the editor because Ed.nodes() maps a row's text nodes to source
   columns by CONCATENATION — wrapping a run adds no text and removes none, so
   every caret column, selection range and token range reads exactly as before.
   Nothing in this block writes g.view.lines.

   ONE BAR PER PANE, owned by the group (g.findEl / g.find), because a split is
   two independent notes and a single global bar would search one and mark the
   other. The census reports the FOCUSED group's bar (fTok, in updateTitle). */
const F_MARK = "fmk", F_CUR = "fcur";
function fState(g) {
  if (!g.find) g.find = { open: false, rep: false, q: "", r: "", hits: [], idx: 0 };
  return g.find;
}
/* ---- R26.21–R26.24: THE READING-VIEW QUIRKS, cloned, not improved ----
   In reading view stock's find is a DIFFERENT, smaller thing, and the brief is
   explicit that the quirks are the requirement:
     R26.21 the bar is REDUCED — the two navigation arrows are gone
     R26.23 it searches the RENDERED text, so markup the renderer consumed
            (`**`) is not findable and text the renderer JOINED ("with bold")
            is — the opposite answer from the editor on the same note
     R26.24 it COUNTS but does not navigate: no current match, Enter does
            nothing, nothing scrolls
     R26.22 Ctrl+H is a NO-OP there — you cannot replace what you cannot edit
   Consequences that follow from those rows, not from taste: the counter cannot
   read "i / n" when there is no i, so it is the bare count; hits are offsets
   into the rendered text (no {l,c} exists for a <strong> the source does not
   have); and the marks are painted in g.preview, which is why every surface
   read below goes through fSurf() instead of naming g.lp. */
const fRead = g => !!g && isReading(g);
const fSurf = g => (!g ? null : fRead(g) ? g.preview : g.lp);
/* The rendered text as ONE string, plus the text nodes it was concatenated
   from. Marks never change the text (a <span> wrapping a run adds no
   characters), so these offsets are stable whether or not the previous query's
   marks are still in the DOM. */
function fRSegs(el) {
  const segs = [], parts = [];
  let len = 0;
  if (!el) return { segs, text: "" };
  const w = document.createTreeWalker(el, NodeFilter.SHOW_TEXT, null);
  for (let n = w.nextNode(); n; n = w.nextNode()) {
    const t = n.nodeValue || "";
    if (!t) continue;
    segs.push({ n, start: len, len: t.length });
    parts.push(t);
    len += t.length;
  }
  return { segs, text: parts.join("") };
}
/* R26.21 + R26.22: the bar's SHAPE follows the view. The five children stay in
   the DOM (R26.2 is about what the bar offers, and a hidden control offers
   nothing) — prev/next are hidden, and an open replace row is withdrawn,
   because entering reading view with replace open would leave a Replace all
   button over a note that cannot be edited. */
function fShape(g) {
  const bar = g.findEl;
  if (!bar) return;
  const r = fRead(g);
  bar.classList.toggle("reading", r);
  bar.querySelector(".findprev").hidden = r;
  bar.querySelector(".findnext").hidden = r;
  if (r) { fState(g).rep = false; bar.querySelector(".reprow").hidden = true; }
}
/* The bar survives a mode switch, so the switch must re-shape it and re-scan:
   the same query over the rendered text is a different set of hits. */
function fModeSwitched(g) {
  const st = g && g.find;
  if (!st || !st.open) return;
  fClear(g);
  fShape(g);
  fSync(g, true);
  fReveal(g);
}
function fBar(g) {                     // the bar's DOM, built once per pane
  if (g.findEl) return g.findEl;
  const bar = document.createElement("div");
  bar.className = "findbar";
  bar.hidden = true;
  /* R26.2: EXACTLY these five children of .findrow. R26.5 records that stock
     deliberately offers no case / whole-word / regex toggles — the census
     publishes [fels:] so an added sixth control fails the phase, which is the
     only way "and no more" can be a testable claim rather than a wish. */
  bar.innerHTML =
    '<div class="findrow">' +
      '<input class="findq" type="text" spellcheck="false" autocomplete="off" placeholder="Find">' +
      '<span class="findcount"></span>' +
      '<button class="findprev" title="Previous match (Shift+Enter)">↑</button>' +
      '<button class="findnext" title="Next match (Enter)">↓</button>' +
      '<button class="findclose" title="Close (Escape)">✕</button>' +
    '</div>' +
    /* R26.15: the replace row is the SAME bar with a second row revealed, not a
       second widget — that is what makes R26.16 ("replace inherits the find
       query") structural rather than a copy step that can drift. Three children:
       the replacement box, Replace, Replace all. */
    '<div class="reprow" hidden>' +
      '<input class="repq" type="text" spellcheck="false" autocomplete="off" placeholder="Replace">' +
      '<button class="repone" title="Replace this match (Enter)">Replace</button>' +
      '<button class="repall" title="Replace all matches">Replace all</button>' +
    '</div>';
  const inp = bar.querySelector(".findq");
  inp.addEventListener("input", () => {          // R26.3: the count follows the query, per keystroke
    fState(g).q = inp.value;
    fSync(g, true);
    fReveal(g);
    updateTitle();
  });
  inp.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); fClose(g, true); return; }
    if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); fGo(g, e.shiftKey ? -1 : 1); return; }
    if (fEdit(g, e)) return;
    const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    if ((e.ctrlKey || e.metaKey) && k === "f") { e.preventDefault(); e.stopPropagation(); inp.select(); return; }
    if ((e.ctrlKey || e.metaKey) && k === "h") { e.preventDefault(); e.stopPropagation(); fOpenRep(g); return; }
    /* Everything else belongs to the BOX, not to the app keymap: a query
       contains characters that are chords elsewhere (F2 renames the file, Tab
       indents, Ctrl+W closes the tab). The global keydown listener is on
       `document`, so stopping the bubble here is what keeps them out. */
    e.stopPropagation();
  });
  bar.querySelector(".findprev").onclick = () => { fGo(g, -1); fFocus(g); };
  bar.querySelector(".findnext").onclick = () => { fGo(g, 1); fFocus(g); };
  bar.querySelector(".findclose").onclick = () => fClose(g, true);
  const rin = bar.querySelector(".repq");
  rin.addEventListener("input", () => { fState(g).r = rin.value; updateTitle(); });
  rin.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); fClose(g, true); return; }
    if (e.key === "Enter") {            // R26.17: Enter in the replacement box rewrites THIS match
      e.preventDefault(); e.stopPropagation();
      if (e.shiftKey || e.ctrlKey || e.metaKey) fRepAll(g); else fRepOne(g);
      return;
    }
    if (fEdit(g, e)) return;
    const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    if ((e.ctrlKey || e.metaKey) && k === "f") { e.preventDefault(); e.stopPropagation(); fFocus(g); return; }
    e.stopPropagation();
  });
  bar.querySelector(".repone").onclick = () => { fRepOne(g); fRFocus(g); };
  bar.querySelector(".repall").onclick = () => { fRepAll(g); fRFocus(g); };
  g.content.appendChild(bar);          // .content is position:relative; the bar floats over the note
  g.findEl = bar;
  return bar;
}
function fFocus(g) { const b = g.findEl; if (b) { const i = b.querySelector(".findq"); if (i) i.focus(); } }
function fRFocus(g) { const b = g.findEl; if (b) { const i = b.querySelector(".repq"); if (i) i.focus(); } }
/* THE UNDO BRIDGE. Focus sits in a text input while the bar is open, so the
   global keydown listener never sees Ctrl+Z — and after a replace-all the one
   chord a user reaches for is exactly that one. R26.20 promises a note can be
   put back with ONE undo; a promise reachable only after closing the bar first
   is not the promise. So the two boxes forward undo/redo TO THE NOTE, and only
   those two chords: everything else keeps belonging to the input. Returns true
   when it handled the event. */
function fEdit(g, e) {
  const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  if (!(e.ctrlKey || e.metaKey)) return false;
  if (k !== "z" && k !== "y") return false;
  e.preventDefault(); e.stopPropagation();
  if (k === "y" || e.shiftKey) Ed.redo(g); else Ed.undo(g);
  fSync(g, true);                      // the note changed under the query: re-count, re-mark
  updateTitle();
  return true;
}
/* R26.4: case-insensitive SUBSTRING, always, over the source lines. Matches do
   not overlap (stock's counter counts the same runs its Enter walks). */
function fScan(g) {
  const st = fState(g), out = [];
  const q = st.q.toLowerCase();
  if (!q) return out;
  /* R26.23: in reading view the haystack is what the RENDERER produced, not the
     source — same case rule, different text. A hit is an offset range into that
     flat text; it deliberately has no {l,c}, because the rendered string has no
     source line (a <strong> spans markup the source still carries). */
  if (fRead(g)) {
    const hay = fRSegs(g.preview).text.toLowerCase();
    let i = hay.indexOf(q);
    while (i >= 0) { out.push({ a: i, b: i + q.length }); i = hay.indexOf(q, i + q.length); }
    return out;
  }
  if (!g.lp) return out;
  const L = Ed.lines(g);
  for (let l = 0; l < L.length; l++) {
    const hay = (L[l] || "").toLowerCase();
    let i = hay.indexOf(q);
    while (i >= 0) { out.push({ l, c: i }); i = hay.indexOf(q, i + q.length); }
  }
  return out;
}
function fClear(g) {                   // unwrap every mark, leaving the row byte-identical
  /* BOTH surfaces: a mode switch leaves the marks it painted behind in the view
     the user just left, where nothing else will ever come back to remove them. */
  for (const sc of [g.lp, g.preview]) {
    if (!sc) continue;
    for (const m of [...sc.querySelectorAll("span." + F_MARK)]) {
      const p = m.parentNode;
      if (!p) continue;
      while (m.firstChild) p.insertBefore(m.firstChild, m);
      p.removeChild(m);
      p.normalize();                   // re-join the split text nodes: no fragmentation accumulates
    }
  }
}
/* R26.8: EVERY match is marked, the current one distinguishably (.fcur).
   The wrapping is done per text node, from the LAST offset backwards, so a
   splitText never invalidates an offset that has not been used yet. A hit that
   straddles two text nodes (a match running into a rendered <strong>) becomes
   two spans carrying the SAME data-fh, which is why the census counts DISTINCT
   hit indices rather than spans — "every match is marked" must not be
   satisfiable by marking one match twice. */
function fMarks(g) {
  fClear(g);
  const st = fState(g);
  if (!st.open || !st.hits.length) return;
  /* Reading view: the hits are offsets into the concatenated rendered text, so
     the same right-to-left splitText walk runs over the preview's text nodes.
     A hit that straddles a node boundary ("with bold" = "text with " + a
     <strong>) becomes two spans carrying one data-fh — one match, marked once.
     NOTHING is given .fcur here: R26.24 says this bar counts, it does not
     navigate, and a "current" match is the navigation. */
  if (fRead(g)) {
    const { segs } = fRSegs(g.preview), jobs = new Map();
    for (let i = 0; i < st.hits.length; i++) {
      const h = st.hits[i];
      for (const s of segs) {
        const a = Math.max(h.a, s.start), b = Math.min(h.b, s.start + s.len);
        if (b <= a) continue;
        if (!jobs.has(s.n)) jobs.set(s.n, []);
        jobs.get(s.n).push({ a: a - s.start, b: b - s.start, i, cur: false });
      }
    }
    fWrap(jobs);
    return;
  }
  if (!g.lp) return;
  const qlen = st.q.length, byRow = new Map();
  st.hits.forEach((h, i) => {
    if (!byRow.has(h.l)) byRow.set(h.l, []);
    byRow.get(h.l).push({ c: h.c, i, cur: i === st.idx });
  });
  for (const [l, hs] of byRow) {
    const row = Ed.rowAt(g, l);
    if (!row) continue;                // row not rendered (should not happen: one row per line)
    const map = Ed.nodes(row), jobs = new Map();
    for (const h of hs) for (const s of map) {
      const a = Math.max(h.c, s.c), b = Math.min(h.c + qlen, s.c + s.len);
      if (b <= a) continue;
      if (!jobs.has(s.n)) jobs.set(s.n, []);
      jobs.get(s.n).push({ a: a - s.c, b: b - s.c, i: h.i, cur: h.cur });
    }
    fWrap(jobs);
  }
}
/* The wrap itself, shared by both surfaces: per text node, right to left, so a
   splitText never invalidates an offset that has not been used yet. */
function fWrap(jobs, cls) {
  for (const [node, js] of jobs) {
    js.sort((x, y) => y.a - x.a);      // right to left
    let head = node;
    for (const j of js) {
      head.splitText(j.b);             // tail leaves; head keeps [0, j.b)
      const mid = head.splitText(j.a);
      const sp = document.createElement("span");
      sp.className = (cls || F_MARK) + (j.cur ? " " + F_CUR : "");
      sp.dataset.fh = String(j.i);
      mid.parentNode.insertBefore(sp, mid);
      sp.appendChild(mid);
    }
  }
}
function fPaint(g) {
  const st = fState(g), bar = fBar(g);
  fMarks(g);
  /* R26.3 + R26.7: the counter appears WITH the query (empty box, no counter)
     and a query that matches nothing reads "0 / 0" — a hidden counter and a
     blank one are both indistinguishable from "the search never ran".
     R26.24: in reading view there IS no current match, so there is no "i" to
     print — the counter is the bare number of matches ("3", "0"). Printing
     "1 / 3" there would claim a position the bar refuses to move. */
  bar.querySelector(".findcount").textContent =
    !st.q ? "" : fRead(g) ? String(st.hits.length)
                          : st.hits.length ? (st.idx + 1) + " / " + st.hits.length : "0 / 0";
}
function fSync(g, reset) {             // recompute hits against the current text
  const st = fState(g);
  if (!st.open) return;
  const was = st.hits[st.idx];
  st.hits = fScan(g);
  /* Reading view has no place to keep: there is no current match to hold and
     no edits to hold it across (R26.24). */
  if (reset || !was || fRead(g) || was.l == null) st.idx = 0;
  else {                               // keep the caret's place in the note across a re-scan
    const i = st.hits.findIndex(h => h.l > was.l || (h.l === was.l && h.c >= was.c));
    st.idx = i < 0 ? 0 : i;
  }
  if (st.idx >= st.hits.length) st.idx = 0;
  fPaint(g);
}
/* R26.12: "a find that does not scroll has not found anything". The scroll is
   done on the NOTE SCROLLER only (g.lp), never through scrollIntoView, which
   would also scroll every ancestor — including the window, which R22 says must
   never scroll at all. */
function fReveal(g) {
  /* Nothing to reveal in reading view: no .fcur is ever painted there, so this
     returns without touching g.preview.scrollTop — R26.24's "does not
     navigate" includes not moving the page under the reader. */
  const sc = fSurf(g);
  if (!sc) return;
  const el = sc.querySelector("span." + F_MARK + "." + F_CUR);
  if (!el) return;
  const r = el.getBoundingClientRect(), b = sc.getBoundingClientRect();
  if (r.top >= b.top + 4 && r.bottom <= b.bottom - 4) return;      // already on screen: do not jitter
  sc.scrollTop += (r.top - b.top) - Math.max(0, (sc.clientHeight - r.height) / 2);
}
function fGo(g, d) {                   // R26.11 + R26.13
  const st = fState(g);
  if (!st.open || !st.hits.length) return;
  if (fRead(g)) return;                // R26.24: reading view counts, it does not navigate
  const n = st.hits.length;
  st.idx = (st.idx + d + n) % n;       // past the last match IS the first: the cycle wraps
  fPaint(g);
  fReveal(g);
  updateTitle();
}
/* ---------- R26.15–R26.20: replace ---------- */
/* R26.17: ONE match — the current one — and the cycle moves on to the next.
   This goes through Ed.replace(), so it is an ordinary edit with its own undo
   entry, exactly like typing over a selection. */
function fRepOne(g) {
  const st = fState(g);
  if (!st.open || !st.rep || !st.q || !st.hits.length) return 0;
  const h = st.hits[st.idx], qlen = st.q.length;
  Ed.replace(g, { a: { l: h.l, c: h.c }, b: { l: h.l, c: h.c + qlen } }, fRepText(g), "replace");
  fSync(g, false);                     // keep the place: the next hit at/after where this one was
  fReveal(g);
  fRFocus(g);
  updateTitle();
  return 1;
}
function fRepText(g) {                 // an <input> cannot hold a newline; do not let one in anyway
  const r = fState(g).r;
  return String(r == null ? "" : r).replace(/[\r\n]/g, " ");
}
/* R26.18 + R26.20 — REPLACE-ALL IS ONE UNDO TRANSACTION.
   The whole row exists because of data loss: replace-all is the one command in
   the app that can rewrite a note in dozens of places at once, and the only
   thing between a mistyped query and a lost note is that ONE Ctrl+Z puts every
   byte back.
   That property is not a property of the loop, it is a property of WHERE the
   snapshot is taken. Ed.snap() pushes a FULL copy of lines[], so the correct
   shape is exactly one snap, taken BEFORE the first mutation, and then direct
   splices into the model. Looping over Ed.replace() would read identically and
   be wrong: it snaps per call, so N replacements push N entries and one undo
   restores only the LAST one — the note comes back mangled, and the user, who
   pressed undo once and saw the text move, believes it came back.
   The undo assertion in phase_find is the bytes of the file before and after,
   and docs/negctl-find/ is the proof it is observed failing when this snap is
   moved into the loop. */
function fRepAll(g) {
  const st = fState(g);
  if (!st.open || !st.rep || !st.q || !st.hits.length) return 0;
  const L = Ed.lines(g), qlen = st.q.length, rep = fRepText(g), n = st.hits.length;
  Ed.snap(g, "replace-all");           // ONCE, before anything changes. See above.
  for (let i = n - 1; i >= 0; i--) {   // right to left: an earlier hit's column is never invalidated
    const h = st.hits[i];
    L[h.l] = L[h.l].slice(0, h.c) + rep + L[h.l].slice(h.c + qlen);
  }
  /* The caret lands after the LAST replacement. Its column is the last hit's
     column shifted by every EARLIER hit on that same line (each moved the text
     right of it by rep.length - qlen) — the fixture has one hit per line, so a
     naive `last.c` would pass here and be wrong on any real note. */
  const last = st.hits[n - 1];
  const before = st.hits.filter(h => h.l === last.l && h.c < last.c).length;
  Ed.after(g, last.l, last.c + before * (rep.length - qlen) + rep.length);
  st.idx = 0;
  fSync(g, true);
  fReveal(g);
  fRFocus(g);
  updateTitle();
  return n;
}
function fOpenRep(g) {                 // R26.15
  if (!g) return;
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind) return;            // notes only
  /* R26.22: in reading view Ctrl+H is a NO-OP — not a disabled button, not an
     error: nothing happens at all, because there is no editable text under the
     bar to rewrite. It does not even open the find bar. */
  if (fRead(g)) return;
  const st = fState(g), bar = fBar(g);
  const wasOpen = st.open;
  if (!wasOpen) fOpen(g);              // fOpen() clears rep mode, so it must run FIRST
  st.rep = true;
  bar.querySelector(".reprow").hidden = false;
  const rin = bar.querySelector(".repq");
  rin.value = st.r;
  /* R26.16: Ctrl+H over an open find INHERITS the query — the search you are
     already looking at, with the same case rules, is the one being replaced.
     Focus goes where the new text is typed; on a cold open there is no query
     yet, so it goes to the find box instead. */
  if (wasOpen && st.q) { rin.focus(); rin.select(); } else fFocus(g);
  updateTitle();
}
function fOpen(g) {                    // R26.1
  if (!g) return;
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind) return;            // notes only — a graph tab has no text to search
  const st = fState(g), bar = fBar(g);
  st.open = true;
  bar.hidden = false;
  /* Ctrl+F is FIND. Re-pressing it over an open replace bar drops back to the
     find row (stock: the replace row is a mode you enter deliberately). */
  st.rep = false;
  bar.querySelector(".reprow").hidden = true;
  fShape(g);                           // R26.21: reduced in reading view, full in the editor
  const inp = bar.querySelector(".findq");
  inp.value = st.q;
  fSync(g, true);
  fReveal(g);
  inp.focus();
  inp.select();                        // a re-press replaces the previous query, stock-style
  updateTitle();
}
function fClose(g, atMatch) {          // R26.14
  const st = fState(g);
  const h = st.hits[st.idx], qlen = st.q.length;
  st.open = false;
  if (g.findEl) g.findEl.hidden = true;
  fClear(g);
  st.hits = [];
  /* Escape LEAVES THE CARET AT THE MATCH — this is what turns find into
     navigation instead of a counter. The match is left SELECTED (caret at its
     end), so the next keystroke replaces what was searched for.
     Reading view is excluded by `h.l == null`: its hits are rendered-text
     offsets with no source position, and there is no caret to place (R26.24). */
  if (atMatch && h && h.l != null && !fRead(g) && g.lp) { Ed.place(g, h.l, h.c); Ed.extendTo(g, h.l, h.c + qlen); }
  updateTitle();
}
/* Census (headless probe), published for the FOCUSED group by updateTitle:
     [find:1]              the bar is open
     [fels:<n>]            children of .findrow — R26.2's "five and no more"
     [fvels:<n>]           of those, the ones NOT hidden — 5 in the editor, 3 in
                           reading view (input, count, close): R26.21
     [fbar:full|read]      which bar the view is showing (R26.21/R26.23)
     [ffoc:q|r|lp|-]       where the DOM focus actually is (R26.1, R26.15)
     [fq:<query>]          the text IN THE BOX, not the state variable
     [frep:0|1]            is the replace row revealed (R26.15)
     [rels:<n>]            children of .reprow — replacement box, Replace, Replace all
     [fr:<text>]           the text in the REPLACEMENT box (R26.16 inheritance)
     [fc:<counter text>]   the counter's own textContent ("3 / 7", "0 / 0", "-" = empty)
     [fmk:<marked>/<hits>] DISTINCT hits carrying a painted mark / hits found (R26.8)
     [fcw:<text>]          the painted text of the CURRENT match — a lowercase
                           query selecting "ALPHA" is how R26.4 is observed
     [fsc:<scrollTop>]     the note scroller's own scroll offset (R26.12)
     [fvis:0|1]            is the current match's box inside the scroller's box */
function fTok(g) {
  const st = g && g.find;
  if (!st || !st.open || !g.findEl) return "";
  const bar = g.findEl, inp = bar.querySelector(".findq"), rin = bar.querySelector(".repq");
  const rrow = bar.querySelector(".reprow");
  const surf = fSurf(g);
  const marks = surf ? [...surf.querySelectorAll("span." + F_MARK)] : [];
  const cur = marks.filter(m => m.classList.contains(F_CUR));
  const a = document.activeElement;
  const q = s => String(s == null ? "" : s).replace(/[[\]|]/g, "").slice(0, 60);
  const cnt = bar.querySelector(".findcount").textContent;
  const row = bar.querySelector(".findrow");
  let t = " [find:1] [fels:" + row.children.length + "]" +
          " [fvels:" + [...row.children].filter(c => !c.hidden).length + "]" +
          " [fbar:" + (fRead(g) ? "read" : "full") + "]" +
          " [ffoc:" + (a === inp ? "q" : a === rin ? "r" : a === g.lp ? "lp" : "-") + "]" +
          " [fq:" + (q(inp.value) || "-") + "]" +
          " [frep:" + (st.rep && !rrow.hidden ? 1 : 0) + "]" +
          " [rels:" + rrow.children.length + "]" +
          " [fr:" + (q(rin.value) || "-") + "]" +
          " [fc:" + (q(cnt) || "-") + "]" +
          " [fmk:" + new Set(marks.map(m => m.dataset.fh)).size + "/" + st.hits.length + "]" +
          " [fcw:" + (q(cur.map(m => m.textContent).join("")) || "-") + "]";
  if (surf) {
    t += " [fsc:" + Math.round(surf.scrollTop) + "]";
    if (cur.length) {
      const r = cur[0].getBoundingClientRect(), b = surf.getBoundingClientRect();
      t += " [fvis:" + (r.top >= b.top - 1 && r.bottom <= b.bottom + 1 ? 1 : 0) + "]";
    } else t += " [fvis:-]";
  }
  return t;
}
