const inv = (c, a) => window.__TAURI__.core.invoke(c, a);
/* perf-spans: UI spans land in the SAME RUSTIDIAN_PERF jsonl as the backend
   via log_span. mark(name, t0, extra) is fire-and-forget; the first reply
   tells us whether telemetry is on at all — when it is not, every later
   mark() is a pure no-op (no IPC). */
const perf = {
  on: null,                                  // null = unknown yet
  now: () => performance.now(),
  mark(name, t0, extra = {}) {
    if (perf.on === false) return;
    const ms = performance.now() - t0;
    inv("log_span", { name, ms, extra }).then(en => { perf.on = !!en; }).catch(() => {});
  },
};
const $ = id => document.getElementById(id);
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
const mkTab = name => ({ name, mode: "livepreview", hist: [name], hpos: 0 });  // R8.8: LP default

/* R9.2: left sidebar pane state — Files / Search / Bookmarks (census [pane:]) */
let sidePane = "files";
/* R9.5/R9.6: sidebar visibility — census [side:lXrX] (r wired in R9.6) */
let sideOpen = true, rightOpen = false;
function cmdToggleSide() {
  sideOpen = !sideOpen;
  $("side").hidden = $("ldiv").hidden = !sideOpen;
  $("collapsebtn").title = sideOpen ? "Collapse sidebar" : "Expand sidebar";
  updateTitle();
}

/* R9.6 right sidebar: localgraph of the ACTIVE note (depth 1, in+out),
   reusing startGraph via a stub "group" whose only real element is the
   canvas. fetch reads the OUTER rgCenter so graphRefresh re-filters on
   follow without restarting the sim. census [rg:<center>] while open. */
let rg = null, rgCenter = null;
// active tab's NOTE (kind tabs like gg/lg have no note -> null; the panel
// then keeps its previous center, like Obsidian keeps the last file)
function rgNote() {
  const g = fg(), t = g && g.active >= 0 ? g.tabs[g.active] : null;
  return t && !t.kind ? t.name : null;
}
function mkRg() {
  const stub = () => ({ style: {}, hidden: true });
  return { graph: $("rgraph"), editor: stub(), preview: stub(), lp: stub(),
           status: stub(), lggear: stub(), lgpop: stub(),
           sim: 0, graphRefresh: null, graphOn: false };
}
async function rgStart() {                 // (re)build canvas + sim at current size
  if (!rg) rg = mkRg();
  cancelAnimationFrame(rg.sim);
  rgCenter = rgNote() || rgCenter;
  if (!rgCenter) return;
  await startGraph(rg, {
    fetch: async () => lgFilter(await inv("graph"), rgCenter, 1, true, true),
    center: () => rgCenter,
    onClick: async n => { await navigate(fg(), n); },  // opens in focused group
  });
}
async function rgFollow() {                // active note changed -> re-center
  if (!rightOpen || !rg || !rg.graphRefresh) return;
  const n = rgNote();
  if (!n || n === rgCenter) return;
  rgCenter = n;
  await rg.graphRefresh();
  updateTitle();
}
async function cmdToggleRight() {
  if (!state) return;
  rightOpen = !rightOpen;
  $("rside").hidden = $("rdiv").hidden = !rightOpen;
  $("rtoggle").title = rightOpen ? "Collapse right sidebar" : "Expand right sidebar";
  if (rightOpen) await rgStart();
  else if (rg) { cancelAnimationFrame(rg.sim); rg.graphRefresh = null; rg.graphOn = false; }
  updateTitle();
}
$("rtoggle").onclick = cmdToggleRight;
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
    if (rightOpen) rgStart();              // re-fit canvas world to new width
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
let searchT = null, searchSeq = 0, searchT0 = -1;
async function runSearch() {
  const q = $("sinput").value.trim();
  const seq = ++searchSeq;                  // stale-response guard
  const box = $("sresults");
  if (!q) {
    searchCount = -1; box.textContent = ""; updateTitle(); return;
  }
  const st0 = searchT0 >= 0 ? searchT0 : perf.now(); searchT0 = -1;
  const hits = await inv("search", { query: q });
  if (seq !== searchSeq) return;
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
}

/* R9.4 bookmarks: tree-row context menu toggles; rust persists the plain
   list in vault/.rustidian-bookmarks. census [bm:N] while the pane shows. */
let bmCache = [];
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
  m.style.left = Math.min(e.clientX, window.innerWidth - 150) + "px";
  m.style.top = Math.min(e.clientY, window.innerHeight - 60) + "px";
  document.body.appendChild(m);
  menuEl = m;
}

async function writeNote(name, content) {   // every save funnels here so graphs live-update
  const t0 = perf.now();
  await inv("write_note", { name, content });
  for (const g of groups()) if (g.graphOn && g.graphRefresh) await g.graphRefresh();
  if (rightOpen && rg && rg.graphRefresh) await rg.graphRefresh();  // R9.6 live too
  perf.mark("save", t0, { note: name, bytes: content.length });
}

async function flushSave(g) {               // write g's pending edits NOW
  if (!g) return;
  lpCommit(g);                              // fold any active lp raw row first
  if (!g.saveT) return;
  clearTimeout(g.saveT); g.saveT = null;
  const n = curOf(g);
  if (n) await writeNote(n, g.editor.value);
  await maybeH1Rename(g);                   // ux-3: H1 edit commits a rename
}

/* ---------- group DOM + layout render ---------- */
function mkGroup() {
  const g = { id: gidSeq++, tabs: [], active: -1,
              graphOn: false, graphRefresh: null, sim: null, saveT: null,
              lpActive: null };
  const pane = document.createElement("div");
  pane.className = "pane";
  pane.innerHTML =
    '<div class="tabbar"><div class="tabs"></div>' +
    '<button class="modebtn" title="toggle reading view (Ctrl+E)"></button></div>' +
    '<div class="content">' +
      '<textarea class="editor" spellcheck="false" placeholder="# write markdown, link with [[Note]]"></textarea>' +
      '<div class="lp"></div>' +
      '<div class="preview"></div>' +
      '<canvas class="graph" hidden></canvas>' +
      '<div class="ac" hidden></div>' +
      '<div class="status" hidden><span class="st-bl"></span><span class="st-wc"></span><span class="st-cc"></span></div>' +
      '<button class="lggear" title="local graph settings" hidden>&#9881;</button>' +
      '<div class="lgpop" hidden>' +
        '<label>Depth <span class="lgdv">1</span></label>' +
        '<input class="lgdepth" type="range" min="1" max="3" step="1" value="1">' +
        '<label><input class="lginc" type="checkbox" checked> Incoming links</label>' +
        '<label><input class="lgout" type="checkbox" checked> Outgoing links</label>' +
      '</div>' +
    '</div>';
  g.pane = pane;
  const q = s => pane.querySelector(s);
  g.tabsEl = q(".tabs"); g.modebtn = q(".modebtn"); g.content = q(".content");
  g.editor = q(".editor"); g.lp = q(".lp"); g.preview = q(".preview"); g.graph = q(".graph");
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
  g.editor.addEventListener("input", () => { scheduleSave(g); showAc(g); });
  g.editor.addEventListener("keydown", e => acKeydown(g, e));
  g.editor.addEventListener("blur", () => setTimeout(hideAc, 100));
  return g;
}

function layoutEl(node) {         // split tree -> DOM; flex weights from fractions
  if (!node.children) return node.pane;
  const d = document.createElement("div");
  d.className = "split " + node.dir;
  const els = node.children.map(layoutEl);
  els.forEach((el, i) => {
    el.style.flex = ((node.fractions && node.fractions[i]) || 1) + " 1 0";
    if (i > 0) d.appendChild(divider(node, i - 1, els, d));
    d.appendChild(el);
  });
  return d;
}

// R6.6: draggable divider between split siblings i and i+1 — drag re-weights
// node.fractions (each side floored at 15% of the split), flex updated live
function divider(node, i, els, box) {
  const h = document.createElement("div");
  h.className = "divider " + node.dir;
  h.addEventListener("mousedown", e => {
    e.preventDefault();
    e.stopPropagation();               // don't let mousedown-to-focus swallow it
    const row = node.dir === "row";
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
      els[i].style.flex = a + " 1 0";
      els[i + 1].style.flex = (f0 + f1 - a) + " 1 0";
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

function renderLayout() {
  const main = $("main");
  main.innerHTML = "";
  main.appendChild(layoutEl(state.root));
  updateTitle();
}

function updateTitle() {          // pane/focus census in the window title (headless probe)
  const ps = [...document.querySelectorAll("#main .pane")];
  const nf = document.querySelectorAll("#main .pane.focused").length;
  const fx = (state.root.fractions || []).map(f => f.toFixed(2)).join(",");
  let lg = "";                    // M8: first localgraph tab -> [lg:<center>@<depth>]
  for (const h of groups()) {
    const t = h.tabs.find(t => t.kind === "lg");
    if (t) { lg = " [lg:" + t.center + "@" + t.depth + "]"; break; }
  }
  // R8.10: focused tab's view mode -> [mode:lp|src|read]; when the lp raw
  // row is active, [mode:lp:<l0>] exposes its block start line (headless probe)
  const ft = fg() && fg().active >= 0 ? fg().tabs[fg().active] : null;
  let md = ft && !ft.kind ? " [mode:" + (MODE_ABBR[ft.mode] || "?") : "";
  if (md && ft.mode === "livepreview" && fg().lpActive) md += ":" + fg().lpActive.l0;
  if (md) md += "]";
  if (lpMs >= 0) md += " [lp:" + lpMs + "]";     // perf: last lpRender ms
  const gg = ft && ft.kind === "gg" ? " [gg]" : "";  // R9.7: global graph tab focused
  const modal = modalKind ? " [modal:" + modalKind + "]"
    : ($("rnbox") && !$("rnbox").hidden ? " [modal:rn]" : "");  // m5 fuzzy modal / rename prompt
  const t = "rustidian [panes:" + ps.length + " focused:" + nf +
            "@" + (ps.indexOf(fg() && fg().pane) + 1) + "] [fx:" + fx + "]" +
            " [tabs:" + groups().map(g => g.tabs.length).join(",") + "]" + lg + md + gg + modal +
            " [side:l" + (sideOpen ? 1 : 0) + "r" + (rightOpen ? 1 : 0) + "]" +
            (rightOpen && rgCenter ? " [rg:" + rgCenter + "]" : "") +
            " [pane:" + sidePane + "]" +
            (sidePane === "search" && searchCount >= 0 ? " [sr:" + searchCount + "]" : "") +
            (sidePane === "bm" ? " [bm:" + bmCache.length + "]" : "");
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
  if (prev) refreshTree();        // explorer active-note highlight follows focus
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
  const t = src ? Object.assign(mkTab(src.name), { mode: src.mode }) : null;
  await splitWith(g, dir, t);
}

async function splitWith(g, dir, tab) {  // insert a new sibling group carrying `tab`
  const parent = findParent(state.root, g);
  if (!parent) return;
  const ng = mkGroup();
  if (tab) { ng.tabs.push(tab); ng.active = 0; }
  if (parent.children.length === 1) parent.dir = dir;   // lone child: re-aim the split
  const idx = parent.children.indexOf(g);
  if (parent.dir === dir) {              // same axis: insert sibling, halve g's share
    const f = (parent.fractions && parent.fractions[idx]) || 1;
    parent.children.splice(idx + 1, 0, ng);
    parent.fractions.splice(idx, 1, f / 2, f / 2);
  } else {                               // cross axis: wrap g in a nested split
    parent.children[idx] = { dir, children: [g, ng], fractions: [0.5, 0.5] };
  }
  renderLayout();
  focusGroup(ng);
  if (ng.active >= 0) await loadActive(ng);
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
  if (parent.children.length === 1) {         // simplify single-child splits
    const child = parent.children[0];
    const gp = findParent(state.root, parent);
    if (gp) gp.children[gp.children.indexOf(parent)] = child;
    else if (child.children) state.root = child;
    // lone Group at root keeps the boot wrapper { dir, children:[g] } —
    // splitGroup depends on every group having a findParent hit
  }
  if (state.focused === g) state.focused = null;
  renderLayout();
  focusGroup(leaves(heir)[0]);                // focus nearest surviving group
  for (const h of groups()) renderTabs(h);    // drop stale chain glyphs (M8)
  await refreshTree();
}

let menuEl = null;
function closeMenu() { if (menuEl) { menuEl.remove(); menuEl = null; } }
document.addEventListener("contextmenu", e => e.preventDefault()); // app-like: native menu never
document.addEventListener("mousedown", e => {
  if (menuEl && !menuEl.contains(e.target)) closeMenu();
}, true);

function tabMenu(e, g, i) {              // right-click a tab -> Split right / Split down
  e.preventDefault();
  closeMenu();
  const m = document.createElement("div");
  m.className = "ctxmenu";
  for (const [label, fn] of [
    ["Split right", () => splitGroup(g, "row", i)],
    ["Split down",  () => splitGroup(g, "col", i)],
  ]) {
    const d = document.createElement("div");
    d.textContent = label;
    d.onmousedown = ev => ev.stopPropagation();  // don't let the closer eat the click
    d.onclick = () => { closeMenu(); fn(); };
    m.appendChild(d);
  }
  m.style.left = Math.min(e.clientX, window.innerWidth - 150) + "px";
  m.style.top = Math.min(e.clientY, window.innerHeight - 80) + "px";
  document.body.appendChild(m);
  menuEl = m;
}

/* ---------- view modes (R8.8: livepreview / source / reading per tab) ---------- */
const ICON_BOOK = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>';
const ICON_PEN = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z"/></svg>';
const ICON_SRC = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M8 6l-6 6 6 6M16 6l6 6-6 6"/></svg>';
const MODE_ABBR = { livepreview: "lp", source: "src", reading: "read" };
const MODE_NEXT = { livepreview: "source", source: "reading", reading: "livepreview" };

function updateModeBtn(g) {
  const tb = g.active >= 0 ? g.tabs[g.active] : null;
  const m = tb ? tb.mode : "livepreview";
  g.modebtn.innerHTML =
    m === "reading" ? ICON_PEN : m === "source" ? ICON_SRC : ICON_BOOK;
}

function applyMode(g) {  // exactly ONE of editor / lp / preview fills the pane
  const m = g.active >= 0 ? g.tabs[g.active].mode : "livepreview";
  g.editor.style.display = m === "source" ? "" : "none";
  g.lp.style.display = m === "livepreview" ? "" : "none";
  g.preview.style.display = m === "reading" ? "" : "none";
  updateModeBtn(g);
}

async function cmdToggleMode(g) {  // Ctrl+E / mode button: lp -> src -> read -> lp
  g = g || fg();
  if (!g || g.active < 0 || g.graphOn) return;
  const tab = g.tabs[g.active];
  if (tab.kind) return;             // graph tabs (lg/gg) have no view mode
  await flushSave(g);
  tab.mode = MODE_NEXT[tab.mode] || "livepreview";
  hideAc();
  applyMode(g);
  if (tab.mode === "reading") await preview(g);
  if (tab.mode === "livepreview") await lpRender(g, -1, 0, true);  // mode switch: full rebuild
  if (tab.mode === "source") g.editor.focus();
  updateTitle();
}

/* ---------- tabs (per group) ---------- */
function renderTabs(g) {
  g.tabsEl.innerHTML = "";
  // R7.3: chain glyph on the localgraph tab AND its linked group's active tab
  const linked = new Set();
  for (const h of groups()) for (const t of h.tabs) if (t.kind === "lg") linked.add(t.linkId);
  g.tabs.forEach((tab, i) => {
    const d = document.createElement("div");
    d.className = "tab" + (i === g.active ? " active" : "");
    const ttl = document.createElement("span");
    ttl.className = "t";
    const chain = tab.kind === "lg" || (i === g.active && linked.has(g.id));
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
  let ghost = null, target = null, hl = null;
  const clearHl = () => {
    if (hl) { hl.classList.remove("drop-strip", "drop-edge"); hl = null; }
  };
  const move = ev => {
    if (!ghost) {
      if (Math.abs(ev.clientX - sx) + Math.abs(ev.clientY - sy) < 6) return;
      ghost = document.createElement("div");
      ghost.id = "tabghost";
      ghost.textContent = g.tabs[i] ? g.tabs[i].name.split("/").pop() : "";
      document.body.appendChild(ghost);
    }
    ghost.style.left = (ev.clientX + 10) + "px";
    ghost.style.top = (ev.clientY + 12) + "px";
    target = null; clearHl();
    for (const h of groups()) {
      const pr = h.pane.getBoundingClientRect();
      if (ev.clientX < pr.left || ev.clientX > pr.right ||
          ev.clientY < pr.top || ev.clientY > pr.bottom) continue;
      const tr = h.tabsEl.getBoundingClientRect();
      if (ev.clientY <= tr.bottom) {
        if (h !== g) {                       // own strip: reorder unsupported, no-op
          target = { kind: "strip", g: h };
          hl = h.tabsEl; hl.classList.add("drop-strip");
        }
      } else if (ev.clientX > pr.right - TAB_EDGE) {
        target = { kind: "edge", g: h };
        hl = h.pane; hl.classList.add("drop-edge");
      }
      break;
    }
  };
  const up = async () => {
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
    const t = target;
    if (ghost) ghost.remove();
    clearHl();
    if (!ghost || !t) return;                // plain click, or dropped nowhere
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
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}

async function loadActive(g) {
  hideAc();
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (t && t.kind === "lg") {       // R7.1: localgraph tab owns the pane's canvas
    await showLocalGraph(g, t);
    renderTabs(g);
    await refreshTree();
    return;
  }
  if (t && t.kind === "gg") {       // R9.7: global graph as a main tab
    cancelAnimationFrame(g.sim);    // clean restart on tab switches
    await startGraph(g, {
      fetch: () => inv("graph"),
      center: () => null,
      onClick: async n => {         // node click: this tab BECOMES the note
        const tt = g.active >= 0 ? g.tabs[g.active] : null;
        if (tt && tt.kind === "gg") delete tt.kind;
        await navigate(g, n);
      },
    });
    renderTabs(g);
    await refreshTree();
    return;
  }
  showEditor(g);
  const n = curOf(g);
  if (n) mruTouch(n);                // m5: quick-switcher MRU order
  g.editor.value = n ? await inv("read_note", { name: n }) : "";
  const tb0 = g.tabs[g.active];
  if (tb0 && !tb0.kind) tb0.h1 = h1Of(g.editor.value);  // ux-3: H1 snapshot
  const m = g.tabs[g.active] ? g.tabs[g.active].mode : "livepreview";
  if (n && m === "source") g.editor.focus();
  if (m === "reading") await preview(g);
  else if (m === "livepreview") await lpRender(g);
  renderTabs(g);
  await refreshTree();
  await updateStatus(g);
  await lgFollow(g);                // R7.3: linked localgraphs track this group
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
    renderTabs(h);
    if (h.graphRefresh) await h.graphRefresh();
    updateTitle();
  }
}

async function switchTab(g, i) {
  if (i === g.active) return;
  const t0 = perf.now();
  await flushSave(g);
  g.active = i;
  await loadActive(g);
  perf.mark("tab_switch", t0, { note: curOf(g), kind: g.tabs[i].kind || "note" });
}

async function openInTab(name, via = "tab") {   // explorer click -> FOCUSED group (R6.3); via:"boot" = auto-open at startup (already inside the boot span)
  const g = fg();
  const t0 = perf.now();
  await flushSave(g);
  const i = g.tabs.findIndex(x => x.name === name);
  if (i >= 0) g.active = i;
  else { g.tabs.push(mkTab(name)); g.active = g.tabs.length - 1; }
  await loadActive(g);
  perf.mark("note_open", t0, { note: name, mode: g.tabs[g.active].mode, via });
}

async function navigate(g, name) { // wikilink / graph click: replace g's ACTIVE tab, push history
  const t0 = perf.now();
  await flushSave(g);
  if (g.active < 0) { g.tabs.push(mkTab(name)); g.active = 0; }
  else {
    const tab = g.tabs[g.active];
    tab.name = name;
    tab.hist = tab.hist.slice(0, tab.hpos + 1);
    tab.hist.push(name);
    tab.hpos++;
  }
  await loadActive(g);
  perf.mark("note_open", t0, { note: name, mode: g.tabs[g.active].mode, via: "link" });
}

async function histGo(d) {         // per-tab back/forward in the focused group
  const g = fg();
  if (!g || g.active < 0) return;
  const tab = g.tabs[g.active];
  const p = tab.hpos + d;
  if (p < 0 || p >= tab.hist.length) return;
  await flushSave(g);
  tab.hpos = p;
  tab.name = tab.hist[p];
  await loadActive(g);
}

async function closeTab(g, i) {
  if (i === g.active) await flushSave(g);
  g.tabs.splice(i, 1);
  if (!g.tabs.length && groups().length > 1)  // R6.5: empty group leaves the tree
    return collapseGroup(g);
  if (g.active >= g.tabs.length) g.active = g.tabs.length - 1;
  else if (i < g.active) g.active--;
  await loadActive(g);
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
    row.onclick = () => {
      collapsed.has(full) ? collapsed.delete(full) : collapsed.add(full);
      refreshTree();
    };
    out.appendChild(row);
    if (open) renderNode(node.dirs.get(d), full, depth + 1, out);
  }
  for (const nm of [...node.notes].sort()) {
    const row = document.createElement("div");
    row.className = "trow note" + (nm === cur() ? " active" : "");
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
// note list (or the collapsed set) changes -> memoize on that signature and
// otherwise just move the .active highlight.
let treeSig = "", treeRows = new Map();
async function refreshTree() {
  const [folders, notes] =
    await Promise.all([inv("list_folders"), inv("list_notes")]);
  notesCache = notes;
  const tree = $("tree");
  const sig = folders.join("\n") + "\0" + notes.join("\n") + "\0" +
    [...collapsed].sort().join("\n");
  if (sig === treeSig && tree.childElementCount) {
    for (const r of tree.querySelectorAll(".trow.note.active")) r.classList.remove("active");
    const r = treeRows.get(cur());
    if (r) r.classList.add("active");
    return;
  }
  treeSig = sig; treeRows = new Map();
  tree.innerHTML = "";
  renderNode(buildTree(folders, notes), "", 0, tree);
}

/* ---------- editor + preview (per group) ---------- */
/* R8.1+R8.2 live preview: per-line hybrid (doc B.3). The hidden textarea
   g.editor.value IS the model; lp shows one rendered row per source line
   (fenced code grouped into one block row). Exactly one raw region: the
   caret row is a textarea with the block's source; leaving it commits back
   into the model (normal scheduleSave path = R8.9) and re-renders. */
function lpBlocks(text) {  // -> [{l0,l1}] inclusive line ranges; fences grouped
  const L = text.split("\n"), out = [];
  let i = 0;
  while (i < L.length) {
    if (/^(```|~~~)/.test(L[i])) {
      let j = i + 1;
      while (j < L.length && !/^(```|~~~)\s*$/.test(L[j])) j++;
      const end = Math.min(j, L.length - 1);
      out.push({ l0: i, l1: end });
      i = end + 1;
    } else { out.push({ l0: i, l1: i }); i++; }
  }
  if (!out.length) out.push({ l0: 0, l1: 0 });
  return out;
}

function lpCommit(g) {  // fold the active raw row back into the model
  const a = g.lpActive;
  if (!a) return;
  g.lpActive = null;
  const L = g.editor.value.split("\n");
  L.splice(a.l0, a.l1 - a.l0 + 1, ...a.ta.value.split("\n"));
  const nv = L.join("\n");
  if (nv !== g.editor.value) { g.editor.value = nv; scheduleSave(g); }
}

// lp pane render. perf-dom: INCREMENTAL. g.lpCache = {note, texts[], htmls[]}
// mirrors g.lp.children one row per block. A pass re-splits blocks, diffs the
// new block texts against the cache by common prefix/suffix, renders (one
// render_blocks IPC) only blocks whose html is unknown — changed/new blocks
// and a raw row that never had html — and patches rows in place: middle
// region replaced by index, kept rows only swapped when their raw/rendered
// state flips. A caret move inside unchanged text is therefore zero IPC and
// two row swaps; scroll/selection/hover elsewhere survive. Full rebuild
// (the only g.lp.innerHTML = "") when: no cache, note switched, `full`
// (mode switch), or the DOM row count disagrees with the cache.
// activeL >= 0 makes that line's block the raw row, caret at (activeL, col)
let lpMs = -1;                 // last completed lpRender duration (census probe)
async function lpRender(g, activeL = -1, col = 0, full = false) {
  const lpT0 = performance.now();
  const seq = g.lpSeq = (g.lpSeq || 0) + 1;      // stale-render guard
  const src = g.editor.value, L = src.split("\n");
  const blocks = lpBlocks(src);
  const texts = blocks.map(b => L.slice(b.l0, b.l1 + 1).join("\n"));
  const activeBi = activeL < 0 ? -1
    : blocks.findIndex(b => b.l0 <= activeL && activeL <= b.l1);
  const note = curOf(g);
  let c = g.lpCache;
  if (full || !c || c.note !== note || g.lp.children.length !== c.texts.length)
    c = null;
  // diff by block text: common prefix p / suffix s; [p, nn-s) is the edit
  const on = c ? c.texts.length : 0, nn = texts.length;
  let p = 0, s = 0;
  if (c) {
    while (p < on && p < nn && c.texts[p] === texts[p]) p++;
    while (s < on - p && s < nn - p && c.texts[on - 1 - s] === texts[nn - 1 - s]) s++;
  }
  const htmls = new Array(nn).fill(null);
  if (c) {
    for (let i = 0; i < p; i++) htmls[i] = c.htmls[i];
    for (let i = 0; i < s; i++) htmls[nn - 1 - i] = c.htmls[on - 1 - i];
  }
  const need = [];
  for (let i = 0; i < nn; i++) if (i !== activeBi && htmls[i] == null) need.push(i);
  if (need.length) {                            // ONE IPC for everything unknown
    const rendered = await inv("render_blocks", { blocks: need.map(i => texts[i]) });
    if (seq !== g.lpSeq) return;                 // a newer render superseded us
    need.forEach((i, k) => { htmls[i] = rendered[k]; });
  }
  const st = g.lp.scrollTop;
  const mk = i => i === activeBi ? lpRawRow(g, blocks[i], texts[i])
                                 : lpRow(g, blocks[i], htmls[i]);
  let touched = 0;
  const rows = g.lp.children;
  if (!c) {                                      // FULL rebuild
    g.lp.innerHTML = "";
    const frag = document.createDocumentFragment();
    for (let i = 0; i < nn; i++) frag.appendChild(mk(i));
    g.lp.appendChild(frag);
    touched = nn;
  } else {
    // middle: old rows [p, on-s) -> new rows [p, nn-s)
    for (let k = on - s - p; k > 0; k--) rows[p].remove();
    if (nn - s > p) {
      const frag = document.createDocumentFragment();
      for (let i = p; i < nn - s; i++) frag.appendChild(mk(i));
      g.lp.insertBefore(frag, rows[p] || null);
      touched += nn - s - p;
    }
    // kept rows: swap only when raw/rendered state flips; suffix line numbers shift
    const kept = i => {
      const row = rows[i], raw = row.classList.contains("raw"), want = i === activeBi;
      if (raw !== want) { row.replaceWith(mk(i)); touched++; return; }
      const b = blocks[i];
      if (+row.dataset.l0 !== b.l0) row.dataset.l0 = b.l0;
      if (+row.dataset.l1 !== b.l1) row.dataset.l1 = b.l1;
      if (raw) {                                 // same block text, keep the textarea
        const ta = row.firstChild;               // ...but a split inside it (Enter) left
        if (ta.value !== texts[i]) {             // extra lines in ta.value: resync
          ta.value = texts[i]; ta.rows = b.l1 - b.l0 + 1;
        }
        g.lpActive = { l0: b.l0, l1: b.l1, ta };
      }
    };
    for (let i = 0; i < p; i++) kept(i);
    for (let i = nn - s; i < nn; i++) kept(i);
  }
  g.lpCache = { note, texts, htmls };
  g.lp.scrollTop = st;
  if (activeBi >= 0 && g.lpActive) {             // caret into the raw row
    const b = blocks[activeBi], ta = g.lpActive.ta;
    const off = L.slice(b.l0, activeL).reduce((a, x) => a + x.length + 1, 0)
              + Math.min(col, L[activeL].length);
    ta.focus(); ta.setSelectionRange(off, off);
  }
  lpMs = Math.round(performance.now() - lpT0);   // perf: census [lp:<ms>]
  updateTitle();                                 // republish [mode:lp:<l0>] census
  perf.mark("lp_render", lpT0, { blocks: nn, lines: L.length, active: activeL,
                                 rendered: need.length, patched: touched, full: !c });
}

// the ONE raw region (R8.2): textarea with the block's source; sets g.lpActive
function lpRawRow(g, b, text) {
  const row = document.createElement("div");
  row.className = "lprow raw";
  row.dataset.l0 = b.l0; row.dataset.l1 = b.l1;
  const ta = document.createElement("textarea");
  ta.className = "lpraw";
  ta.value = text;
  ta.rows = b.l1 - b.l0 + 1;
  ta.spellcheck = false;
  ta.addEventListener("input", () => {           // grow with typed newlines
    ta.rows = ta.value.split("\n").length;
  });
  ta.addEventListener("keydown", ev => lpKey(g, ev));  // R8.4 traversal
  ta.addEventListener("blur", () => setTimeout(() => {
    if (g.lpActive && g.lpActive.ta === ta) { lpCommit(g); lpRender(g); }
  }, 60));
  row.appendChild(ta);
  g.lpActive = { l0: b.l0, l1: b.l1, ta };
  return row;
}

// a rendered row. Handlers read l0/l1 from row.dataset and the model from
// g.editor.value AT EVENT TIME: rows are kept across passes, so closures
// over line numbers would go stale when lines above are inserted/removed.
function lpRow(g, b, h) {
  const row = document.createElement("div");
  row.className = "lprow";
  row.dataset.l0 = b.l0; row.dataset.l1 = b.l1;
  row.innerHTML = h && h.trim() ? h : "&nbsp;";  // blank line stays clickable
  const cur = () => ({ l0: +row.dataset.l0, l1: +row.dataset.l1 });
  // R8.6: rendered checkbox click toggles [ ]/[x] on the source line via
  // the normal save path. pulldown-cmark emits the input disabled (WebKit
  // eats clicks on disabled controls) so re-enable, and stopPropagation
  // keeps lpEdit from opening the raw row.
  row.querySelectorAll("input[type=checkbox]").forEach(cb => {
    cb.disabled = false;
    cb.addEventListener("mousedown", e => {
      e.preventDefault(); e.stopPropagation();
      lpCommit(g);                             // fold any active raw row first
      const l0 = cur().l0, M = g.editor.value.split("\n");
      M[l0] = M[l0].replace(/^(\s*(?:[-*+]|\d+\.) )\[( |[xX])\]/,
        (_, pfx, ch) => pfx + (ch === " " ? "[x]" : "[ ]"));
      g.editor.value = M.join("\n");
      scheduleSave(g);
      lpRender(g);
    });
  });
  // R8.7: wikilink click navigates, ctrl+click opens a new tab. Mousedown
  // level (the row's edit handler is mousedown too) + stopPropagation so
  // the raw row never opens; navigate()/flushSave fold the raw row.
  row.querySelectorAll("a.wiki").forEach(a => {
    a.onclick = e => e.preventDefault();       // href="#": no hash churn
    a.addEventListener("mousedown", async e => {
      e.preventDefault(); e.stopPropagation();
      const n = a.dataset.note;
      if (a.classList.contains("wiki-unresolved")) {
        await writeNote(n, "");
        for (const gg of groups()) gg.lpCache = null;  // cached html says unresolved
      }
      if (e.ctrlKey) {                         // new tab, same group
        await flushSave(g);
        g.tabs.push(mkTab(n));
        g.active = g.tabs.length - 1;
        await loadActive(g);
      } else navigate(g, n);
    });
  });
  row.addEventListener("mousedown", e => {
    e.preventDefault();                        // keep browser from part-selecting
    const bb = cur(), L = g.editor.value.split("\n");
    lpEdit(g, bb.l0, lpCol(e, row, bb, L));    // R8.3 column mapping
  });
  return row;
}

/* R8.3 click -> source column. caretRangeFromPoint gives the caret offset in
   RENDERED text; greedy two-pointer alignment maps it back to the source line
   (marker chars — **, [[, #, "- [ ] " — exist only source-side and are
   consumed there alone), so error <= enclosing marker width (spec tolerance).
   Multi-line (fence) blocks and misses fall back to end/start of line. */
function lpCol(e, row, b, L) {
  const s = L[b.l0];
  if (b.l1 !== b.l0) return 0;                   // fence block: caret at start
  const cr = document.caretRangeFromPoint
    ? document.caretRangeFromPoint(e.clientX, e.clientY) : null;
  if (!cr || !row.contains(cr.startContainer)) return s.length;
  const r = document.createRange();              // rendered chars before caret
  r.selectNodeContents(row);
  r.setEnd(cr.startContainer, cr.startOffset);
  const k = r.toString().length, rt = row.textContent;
  let i = 0, j = 0;
  while (i < s.length && j < k)
    if (s[i] === rt[j]) { i++; j++; } else i++;  // skip source-only marker char
  return i;
}

async function lpEdit(g, line, col) {  // move the raw region to `line`
  lpCommit(g);
  await lpRender(g, line, col);
}

/* R8.4 keyboard traversal. Up/Down at the raw row's first/last line move the
   row to the adjacent block (caret column preserved); Enter on a single-line
   block splits immediately (raw row follows to the new line; fences keep
   native newlines); Backspace at col 0 joins with the previous source line,
   caret at the join point. Home/End stay native inside the textarea. */
function lpKey(g, ev) {
  const a = g.lpActive;
  if (!a) return;
  const ta = a.ta, v = ta.value;
  const pre = v.slice(0, ta.selectionStart).split("\n");
  const tl = pre.length - 1, tc = pre[tl].length;   // caret line/col inside ta
  const nl = v.split("\n").length;
  if (ev.key === "ArrowUp" && tl === 0 && a.l0 > 0) {
    ev.preventDefault();
    lpCommit(g); lpRender(g, a.l0 - 1, tc);
  } else if (ev.key === "ArrowDown" && tl === nl - 1) {
    const target = a.l0 + nl;                       // first line after commit
    const total = g.editor.value.split("\n").length - (a.l1 - a.l0 + 1) + nl;
    if (target >= total) return;                    // nothing below: native
    ev.preventDefault();
    lpCommit(g); lpRender(g, target, tc);
  } else if (ev.key === "Enter" && a.l1 === a.l0) { // split single-line block
    ev.preventDefault();
    // R8.5: list/checkbox auto-continuation — marker = indent + bullet/number
    const m = v.match(/^(\s*)([-*+] \[[ xX]\] |[-*+] |\d+\. )/);
    if (m && v === m[0]) {                          // empty item: clear it
      ta.value = "";
      lpCommit(g); lpRender(g, a.l0, 0);
      return;
    }
    const cont = !m ? "" : m[1] +
      (m[2].includes("[") ? m[2].replace(/\[[xX]\]/, "[ ]")   // checkbox -> unchecked
       : /^\d/.test(m[2]) ? (parseInt(m[2], 10) + 1) + ". "   // numbered increments
       : m[2]);
    ta.value = v.slice(0, ta.selectionStart) + "\n" + cont + v.slice(ta.selectionEnd);
    lpCommit(g); lpRender(g, a.l0 + pre.length, cont.length);
  } else if (ev.key === "Backspace" && ta.selectionStart === 0 &&
             ta.selectionEnd === 0 && a.l0 > 0) {   // join with previous line
    ev.preventDefault();
    lpCommit(g);
    const L = g.editor.value.split("\n"), p = a.l0 - 1, c = L[p].length;
    L[p] += L[p + 1]; L.splice(p + 1, 1);
    g.editor.value = L.join("\n"); scheduleSave(g);
    lpRender(g, p, c);
  }
}

async function preview(g) {
  g.preview.innerHTML = await inv("render", { content: g.editor.value });
  for (const a of g.preview.querySelectorAll("a.wiki"))
    a.onclick = async e => {
      e.preventDefault();
      const n = a.dataset.note;
      if (a.classList.contains("wiki-unresolved"))   // R3.5: click creates the note
        await writeNote(n, "");
      if (e.ctrlKey) {                               // R6.4: open in NEW TAB, same group
        await flushSave(g);
        g.tabs.push(mkTab(n));
        g.active = g.tabs.length - 1;
        await loadActive(g);
      } else navigate(g, n);
    };
}

function scheduleSave(g) {
  clearTimeout(g.saveT);
  g.saveT = setTimeout(async () => {
    g.saveT = null;
    const n = curOf(g);
    if (n) await writeNote(n, g.editor.value);
    await maybeH1Rename(g);                 // ux-3: H1 edit commits a rename
    preview(g);
    updateStatus(g);
  }, 250);
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
let notesCache = [], acItems = [], acSel = 0, acStart = -1;

function hideAc() {
  for (const g of groups()) g.acEl.hidden = true;
  acItems = []; acStart = -1;
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

function acContext(g) {  // caret inside an unclosed [[ on one line?
  const ed = g.editor;
  const upto = ed.value.slice(0, ed.selectionStart);
  const a = upto.lastIndexOf("[[");
  if (a < 0) return null;
  const frag = upto.slice(a + 2);
  if (frag.includes("]]") || frag.includes("\n")) return null;
  return { start: a, q: frag };
}

function caretXY(g) {    // approximate caret position within g's content box
  const ed = g.editor;
  const lines = ed.value.slice(0, ed.selectionStart).split("\n");
  const col = lines[lines.length - 1].length;
  const x = ed.offsetLeft + 16 + Math.min(col * 7.8, ed.clientWidth - 40);
  const y = ed.offsetTop + 16 + lines.length * 20.8 - ed.scrollTop;
  return [x, y];
}

function showAc(g) {
  const ctx = acContext(g);
  if (!ctx) return hideAc();
  acStart = ctx.start;
  acItems = notesCache
    .map(n => [fuzzy(ctx.q, n), n])
    .filter(([s]) => s >= 0)
    .sort((a, b) => a[0] - b[0] || a[1].localeCompare(b[1]))
    .slice(0, 8)
    .map(([, n]) => n);
  if (!acItems.length) return hideAc();
  acSel = 0;
  const box = g.acEl;
  box.innerHTML = "";
  acItems.forEach((n, i) => {
    const d = document.createElement("div");
    d.textContent = n;
    if (i === acSel) d.className = "sel";
    d.onmousedown = e => { e.preventDefault(); acInsert(g, n); };
    box.appendChild(d);
  });
  const [x, y] = caretXY(g);
  box.style.left = Math.max(0, Math.min(x, g.content.clientWidth - 200)) + "px";
  box.style.top = Math.min(y, g.content.clientHeight - 60) + "px";
  box.hidden = false;
}

function acInsert(g, name) {
  const ed = g.editor;
  const end = ed.selectionStart;
  ed.value = ed.value.slice(0, acStart) + "[[" + name + "]]" + ed.value.slice(end);
  const p = acStart + name.length + 4;
  ed.setSelectionRange(p, p);
  hideAc();
  ed.focus();
  scheduleSave(g);
}

function acKeydown(g, e) {
  if (g.acEl.hidden) return;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault(); e.stopPropagation();
    acSel = (acSel + (e.key === "ArrowDown" ? 1 : acItems.length - 1)) % acItems.length;
    [...g.acEl.children].forEach((d, i) => d.className = i === acSel ? "sel" : "");
  } else if (e.key === "Enter" || e.key === "Tab") {
    e.preventDefault(); e.stopPropagation();
    acInsert(g, acItems[acSel]);
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
  const name = "Untitled-" + Date.now() % 10000;
  await writeNote(name, "# " + name + "\n");
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
  await writeNote(n, g.editor.value);
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
function renderModal() {
  const box = $("mlist");
  box.innerHTML = "";
  if (!mdItems.length) {
    const d = document.createElement("div");
    d.className = "mempty"; d.textContent = "No matches";
    return box.appendChild(d);
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
    if (it) { closeModal(); await it.run(e); }
  }
};
$("modal").onmousedown = e => { if (e.target === $("modal")) closeModal(); };
function cmdQuickSwitch() {
  modalKind === "qs" ? closeModal() : openModal("qs", qsItems);
}
function cpItems() {                 // palette source: command registry w/ hotkey hints
  return [
    { label: "New note",             hint: "Ctrl+N",       run: cmdNewNote },
    { label: "New folder",           hint: "",             run: () => {
        $("fnew").hidden = false; $("fname").value = ""; $("fname").focus(); } },
    { label: "Toggle edit mode",     hint: "Ctrl+E",       run: () => cmdToggleMode() },
    { label: "Split right",          hint: "",             run: () => splitGroup(fg(), "row", fg().active) },
    { label: "Split down",           hint: "",             run: () => splitGroup(fg(), "col", fg().active) },
    { label: "Open graph view",      hint: "Ctrl+G",       run: cmdGlobalGraph },
    { label: "Open local graph",     hint: "Ctrl+Shift+G", run: () => cmdLocalGraph() },
    { label: "Toggle left sidebar",  hint: "",             run: cmdToggleSide },
    { label: "Toggle right sidebar", hint: "",             run: () => cmdToggleRight() },
    { label: "Switch vault",         hint: "",             run: showPicker },
    { label: "Rename note",          hint: "F2",           run: cmdRename },
    { label: "Close tab",            hint: "Ctrl+W",       run: cmdCloseTab },
    { label: "Save",                 hint: "Ctrl+S",       run: cmdSave },
  ];
}
function cmdPalette() {
  modalKind === "cp" ? closeModal() : openModal("cp", cpItems);
}

/* m5 F2 rename: inline prompt over the focused note tab; disk rename via
   rename_note (ux-3: wikilinks rewritten vault-wide by the rust side), then
   tabs/hist/mru follow the name */
async function applyRename(old, nn) {   // post-rename bookkeeping (F2 + H1 paths)
  for (const h of groups()) for (const tb of h.tabs) {
    if (tb.kind) continue;
    if (tb.name === old) tb.name = nn;
    if (tb.hist) tb.hist = tb.hist.map(n => (n === old ? nn : n));
  }
  const mi = mruList.indexOf(old);
  if (mi >= 0) mruList[mi] = nn;
  for (const h of groups()) renderTabs(h);
  await refreshTree();
  updateTitle();
}

/* ux-3: committing an edit to the first-line H1 renames the note (Obsidian
   inline-title behavior). Fires only when the note HAD an H1 and its text
   changed since load/last commit; collision/invalid -> rust refuses, name
   kept (content keeps the new H1, like Obsidian on conflict). */
const h1Of = s => {
  const m = /^#[ \t]+(.+?)\s*$/.exec((s || "").split("\n", 1)[0]);
  return m ? m[1] : null;
};
async function maybeH1Rename(g) {
  const t = g.active >= 0 ? g.tabs[g.active] : null;
  if (!t || t.kind) return;
  const h1 = h1Of(g.editor.value);
  const prev = t.h1;
  t.h1 = h1;
  if (!h1 || prev == null || h1 === prev) return;
  const old = t.name;
  const dir = old.includes("/") ? old.slice(0, old.lastIndexOf("/") + 1) : "";
  const nn = dir + h1.replace(/[\\/]/g, "-");
  if (nn === old) return;
  try { await inv("rename_note", { old, new: nn }); }
  catch (err) { return; }                   // exists/invalid -> keep old name
  await applyRename(old, nn);
}

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

const keymap = {
  "ctrl+n": cmdNewNote,
  "ctrl+s": cmdSave,
  "ctrl+w": cmdCloseTab,
  "ctrl+e": () => cmdToggleMode(),
  "ctrl+o": cmdQuickSwitch,
  "ctrl+p": cmdPalette,
  "ctrl+g": cmdGlobalGraph,
  "f2": cmdRename,
  "ctrl+t": cmdNewTab,
  "ctrl+tab": () => cmdCycleTab(1),
  "ctrl+shift+tab": () => cmdCycleTab(-1),
  "ctrl+shift+g": () => cmdLocalGraph(),
  "alt+arrowleft": () => histGo(-1),
  "alt+arrowright": () => histGo(1),
  "ctrl+alt+arrowleft": () => histGo(-1),
  "ctrl+alt+arrowright": () => histGo(1),
};
for (let n = 1; n <= 9; n++) keymap["ctrl+" + n] = () => cmdJumpTab(n);
document.addEventListener("keydown", e => {
  if (e.key === "Escape") {
    if (modalKind) { closeModal(); return; }
    if (!$("rnbox").hidden) { $("rnbox").hidden = true; updateTitle(); return; }
    closeMenu();
    if (vaultPath && !$("picker").hidden) $("picker").hidden = true;
    if (!$("fnew").hidden) $("fnew").hidden = true;
    return;
  }
  let k = e.key.toLowerCase();
  if (e.code === "Tab") k = "tab";   // X11 shift+tab arrives as ISO_Left_Tab
  // Xvfb/xdotool synthesize every F-key with a spurious Mod1 latch (alt:true
  // on F1..F12, clean on letters) — drop alt for function keys so F2 binds.
  // No alt+Fn combo is in the keymap, so nothing real is masked.
  const alt = e.altKey && !/^F\d+$/.test(e.code);
  const combo = (e.ctrlKey ? "ctrl+" : "") + (alt ? "alt+" : "")
    + (e.shiftKey ? "shift+" : "") + k;
  const fn = keymap[combo];
  if (modalKind && fn !== cmdQuickSwitch && fn !== cmdPalette) return;  // modal traps the keymap
  if (!$("rnbox").hidden) return;    // rename prompt traps the keymap too
  if (fn) { e.preventDefault(); fn(); }
});

$("stab-files").onclick = () => setPane("files");
$("collapsebtn").onclick = cmdToggleSide;
$("stab-search").onclick = () => setPane("search");
$("stab-bm").onclick = () => setPane("bm");
$("sinput").oninput = () => {              // debounce 60ms (was 150: sized for the disk-walking search; the index answers in ~1ms, the DOM for ~400 hits in a few ms)
  if (searchT0 < 0) searchT0 = perf.now();  // perf: first keystroke of this query
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

/* ---------- graph (per group: one sim instance per group) ----------
   startGraph(g, cfg) is the shared canvas sim (M4): hover/zoom/pan/unresolved.
   cfg = { fetch: async () -> {nodes, edges},   node/edge supplier
           center: () -> name|null,             drawn larger + accent (M8 localgraph)
           onClick: async name -> void }        navigation target on node click */
function showEditor(g) {
  g.graphOn = false; g.graphRefresh = null; cancelAnimationFrame(g.sim);
  g.graph.hidden = true;
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
  g.graphOn = true;
  hideAc();
  g.status.hidden = true;
  g.editor.style.display = "none"; g.preview.style.display = "none";
  g.lp.style.display = "none";
  const cv = g.graph; cv.hidden = false;
  cv.width = cv.clientWidth; cv.height = cv.clientHeight;
  const gr = await cfg.fetch();
  // sim runs in WORLD coords (world = initial canvas rect); screen = world*scale + t
  const view = { scale: 1, tx: 0, ty: 0 };
  const W = cv.width, H = cv.height;                          // world bounds
  const N = gr.nodes.map((nd, i) => ({
    n: nd.name, resolved: nd.resolved,
    x: W / 2 + 120 * Math.cos(i), y: H / 2 + 120 * Math.sin(i),
    vx: 0, vy: 0
  }));
  const ctx = cv.getContext("2d");
  const toWorld = (sx, sy) =>
    [(sx - view.tx) / view.scale, (sy - view.ty) / view.scale];
  // hover: adjacency + hovered node index (-1 = none)
  const adj = N.map(() => new Set());
  for (const [i, j] of gr.edges) { adj[i].add(j); adj[j].add(i); }
  let hov = -1;
  const hitTest = (x, y) => N.findIndex(p => (p.x - x) ** 2 + (p.y - y) ** 2 < 144);
  // live refresh (R4.3): re-fetch on save, keep surviving positions,
  // seed new nodes near their first neighbor
  g.graphRefresh = async () => {
    const g2 = await cfg.fetch();
    const old = new Map(N.map(p => [p.n, p]));
    const N2 = g2.nodes.map(nd => {
      const o = old.get(nd.name);
      return o ? { n: nd.name, resolved: nd.resolved, x: o.x, y: o.y, vx: o.vx, vy: o.vy }
               : { n: nd.name, resolved: nd.resolved, x: null, y: null, vx: 0, vy: 0 };
    });
    N2.forEach((p, i) => {
      if (p.x !== null) return;
      const e = g2.edges.find(([a, b]) => a === i || b === i);
      const nb = e ? N2[e[0] === i ? e[1] : e[0]] : null;
      p.x = (nb && nb.x !== null ? nb.x : W / 2) + 30 * (Math.random() - 0.5);
      p.y = (nb && nb.y !== null ? nb.y : H / 2) + 30 * (Math.random() - 0.5);
    });
    N.length = 0; N.push(...N2);
    gr.edges = g2.edges;
    adj.length = 0; for (const _ of N) adj.push(new Set());
    for (const [i, j] of gr.edges) { adj[i].add(j); adj[j].add(i); }
    hov = -1;
    alpha = Math.max(alpha, 0.5);   // partial reheat: settle new nodes without scattering old ones
  };
  // sim heat: forces scale by alpha, which decays per PHYSICS STEP; below
  // 0.02 physics freezes (render loop keeps running for hover/zoom/pan).
  // Physics steps are wall-clock-locked at 60/s (substepped inside rAF):
  // a throttled/headless rAF must not stretch the ~3.2s settle time.
  let alpha = 1, phAcc = 0, phLast = performance.now();
  function physStep() {
    for (const a of N) for (const b of N) {
      if (a === b) continue;
      const dx = a.x - b.x, dy = a.y - b.y, d2 = dx * dx + dy * dy + 0.01;
      a.vx += alpha * 800 * dx / d2; a.vy += alpha * 800 * dy / d2;  // repulsion
    }
    for (const [i, j] of gr.edges) {
      const a = N[i], b = N[j], dx = b.x - a.x, dy = b.y - a.y;
      a.vx += dx * 0.005 * alpha; a.vy += dy * 0.005 * alpha;        // spring
      b.vx -= dx * 0.005 * alpha; b.vy -= dy * 0.005 * alpha;
    }
    for (const p of N) {
      p.vx += (W / 2 - p.x) * 0.01 * alpha; p.vy += (H / 2 - p.y) * 0.01 * alpha;
      p.vx *= 0.85; p.vy *= 0.85; p.x += p.vx; p.y += p.vy;
      const m = 30;   // clamp in WORLD coords (labels stay near world bounds)
      p.x = Math.max(m, Math.min(W - m, p.x));
      p.y = Math.max(m, Math.min(H - m, p.y));
    }
    alpha *= 0.98;
  }
  let firstFrame = true;
  function step() {
    const now = performance.now();
    if (firstFrame) {
      firstFrame = false;
      if (g.perfT0) { perf.mark("graph_open", g.perfT0, { nodes: N.length, edges: gr.edges.length }); g.perfT0 = null; }
    }
    phAcc = Math.min(phAcc + (now - phLast) / 1000, 0.25); phLast = now;
    while (phAcc >= 1 / 60) {
      phAcc -= 1 / 60;
      if (alpha > 0.02) physStep();
    }
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.clearRect(0, 0, cv.width, cv.height);
    ctx.setTransform(view.scale, 0, 0, view.scale, view.tx, view.ty);
    // hover: hovered node + its edges/neighbors lit accent, rest faded
    const litE = ([i, j]) => hov < 0 || i === hov || j === hov;
    const litN = i => hov < 0 || i === hov || adj[hov].has(i);
    ctx.lineWidth = 1;
    for (const ed of gr.edges) {
      const lit = litE(ed);
      ctx.globalAlpha = lit ? 1 : 0.12;
      ctx.strokeStyle = hov >= 0 && lit ? "#f9e2af" : "#45475a";
      ctx.beginPath(); ctx.moveTo(N[ed[0]].x, N[ed[0]].y);
      ctx.lineTo(N[ed[1]].x, N[ed[1]].y); ctx.stroke();
    }
    ctx.textAlign = "center"; ctx.font = "12px sans-serif";
    const cn = cfg.center();          // M8: center node larger + accent (R7.1)
    for (let i = 0; i < N.length; i++) {
      const p = N[i];
      const isC = cn !== null && p.n === cn;
      ctx.globalAlpha = litN(i) ? (p.resolved ? 1 : 0.55) : 0.12;
      const col = i === hov ? "#f9e2af" : isC ? "#a6e3a1" : "#89b4fa";
      ctx.beginPath(); ctx.arc(p.x, p.y, isC ? 10 : 6, 0, 7);
      if (p.resolved) { ctx.fillStyle = col; ctx.fill(); }
      else { ctx.lineWidth = 1.5; ctx.strokeStyle = col; ctx.stroke(); ctx.lineWidth = 1; } // hollow = unresolved
      if (view.scale >= 0.5) { ctx.fillStyle = col; ctx.fillText(p.n, p.x, p.y - (isC ? 14 : 10)); }
    }
    ctx.globalAlpha = 1;
    g.sim = requestAnimationFrame(step);
  }
  step();
  // wheel: cursor-anchored zoom, 0.2x-5x
  cv.onwheel = e => {
    e.preventDefault();
    const r = cv.getBoundingClientRect();
    const mx = e.clientX - r.left, my = e.clientY - r.top;
    const [wx, wy] = toWorld(mx, my);
    const s = Math.max(0.2, Math.min(5, view.scale * (e.deltaY < 0 ? 1.1 : 1 / 1.1)));
    view.tx = mx - wx * s; view.ty = my - wy * s; view.scale = s;
  };
  // drag anywhere pans (incl. on nodes — simpler); click w/o movement navigates
  let drag = null, moved = false;
  cv.onmousedown = e => { drag = { x: e.clientX, y: e.clientY }; moved = false; };
  cv.onmousemove = e => {
    const r = cv.getBoundingClientRect();
    if (drag) {
      const dx = e.clientX - drag.x, dy = e.clientY - drag.y;
      if (moved || dx * dx + dy * dy > 16) {
        moved = true;
        view.tx += dx; view.ty += dy;
        drag = { x: e.clientX, y: e.clientY };
      }
      return;
    }
    const [x, y] = toWorld(e.clientX - r.left, e.clientY - r.top);
    hov = hitTest(x, y);
    cv.style.cursor = hov >= 0 ? "pointer" : "";
  };
  cv.onmouseup = () => { drag = null; };
  cv.onmouseleave = () => { drag = null; hov = -1; cv.style.cursor = ""; };
  cv.onclick = async e => {
    if (moved) { moved = false; return; }               // was a pan, not a click
    const r = cv.getBoundingClientRect();
    const [x, y] = toWorld(e.clientX - r.left, e.clientY - r.top);
    const hit = N[hitTest(x, y)];
    if (!hit) return;
    if (!hit.resolved)                                  // ghost node: create then open (M3 path)
      await writeNote(hit.n, "");
    cfg.onClick(hit.n);
  };
  // a save can land while the initial fetch is in flight (writeNote sees
  // graphRefresh still null and skips) — refresh once now to close the race
  await g.graphRefresh();
}

/* ---------- M8 local graph (R7.1-R7.5) ---------- */
// BFS neighborhood of `center` over the full edge set; frontier expands via
// outgoing edges when `out`, incoming when `inc`; keeps ALL edges among the
// surviving node set (Obsidian's neighbor-links default), remaps indices
function lgFilter(gr, center, depth, inc, out) {
  const idx = new Map(gr.nodes.map((nd, i) => [nd.name, i]));
  const ci = idx.get(center);
  if (ci == null) return { nodes: [], edges: [] };
  const keep = new Set([ci]);
  let frontier = [ci];
  for (let d = 0; d < depth && frontier.length; d++) {
    const next = [];
    for (const [a, b] of gr.edges) for (const f of frontier) {
      if (out && a === f && !keep.has(b)) { keep.add(b); next.push(b); }
      if (inc && b === f && !keep.has(a)) { keep.add(a); next.push(a); }
    }
    frontier = next;
  }
  const order = [...keep];
  const rmap = new Map(order.map((o, ni) => [o, ni]));
  return {
    nodes: order.map(i => gr.nodes[i]),
    edges: gr.edges.filter(([a, b]) => keep.has(a) && keep.has(b))
                   .map(([a, b]) => [rmap.get(a), rmap.get(b)]),
  };
}

async function showLocalGraph(g, t) {  // t = the localgraph tab (kind:"lg")
  cancelAnimationFrame(g.sim);         // clean restart on tab switches
  await startGraph(g, {
    fetch: async () => lgFilter(await inv("graph"), t.center, t.depth, t.inc, t.out),
    center: () => t.center,
    onClick: async n => {              // R7.4: navigate the LINKED group; lgFollow re-centers
      const lk = groups().find(x => x.id === t.linkId);
      if (lk) await navigate(lk, n);
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
  try {
    vaultPath = pmode === "create"
      ? await inv("create_vault", { parent: bpath, name: $("p-name").value })
      : await inv("set_vault", { path: bpath });
  } catch (err) { $("p-err").textContent = String(err); return; }
  $("picker").hidden = true;
  await enterVault();
};
async function enterVault() {
  $("vswitch").innerHTML = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>' + base(vaultPath);
  collapsed = new Set();
  const g = mkGroup();               // M6: one group, wrapped in a one-leaf split tree
  state = { root: { dir: "row", children: [g], fractions: [1] }, focused: null };
  renderLayout();
  focusGroup(g);
  hideAc();
  await refreshTree();
  await refreshBm();                 // R9.4: menu label needs the cache early
  const names = await inv("list_notes");
  if (names.length) await openInTab(names[0], "boot");
  else renderTabs(g);
  perf.mark("boot", 0, { notes: names.length });   // perf: page start -> vault ready (first note rendered)
}
$("vswitch").onclick = showPicker;

(async () => {
  const sw = await inv("get_sidebar_w").catch(() => null);   // ux-4
  if (sw >= 150) $("side").style.width = Math.min(600, sw) + "px";
  vaultPath = await inv("vault_get");
  if (vaultPath) await enterVault(); else showPicker();
})();

