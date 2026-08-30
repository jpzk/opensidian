const inv = (c, a) => window.__TAURI__.core.invoke(c, a);
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
const mkTab = name => ({ name, mode: "source", hist: [name], hpos: 0 });

async function writeNote(name, content) {   // every save funnels here so graphs live-update
  await inv("write_note", { name, content });
  for (const g of groups()) if (g.graphOn && g.graphRefresh) await g.graphRefresh();
}

async function flushSave(g) {               // write g's pending edits NOW
  if (!g || !g.saveT) return;
  clearTimeout(g.saveT); g.saveT = null;
  const n = curOf(g);
  if (n) await writeNote(n, g.editor.value);
}

/* ---------- group DOM + layout render ---------- */
function mkGroup() {
  const g = { id: gidSeq++, tabs: [], active: -1,
              graphOn: false, graphRefresh: null, sim: null, saveT: null };
  const pane = document.createElement("div");
  pane.className = "pane";
  pane.innerHTML =
    '<div class="tabbar"><div class="tabs"></div>' +
    '<button class="modebtn" title="toggle reading view (Ctrl+E)"></button></div>' +
    '<div class="content">' +
      '<textarea class="editor" spellcheck="false" placeholder="# write markdown, link with [[Note]]"></textarea>' +
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
  g.editor = q(".editor"); g.preview = q(".preview"); g.graph = q(".graph");
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
  const t = "rustidian [panes:" + ps.length + " focused:" + nf +
            "@" + (ps.indexOf(fg() && fg().pane) + 1) + "] [fx:" + fx + "]" +
            " [tabs:" + groups().map(g => g.tabs.length).join(",") + "]" + lg;
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

/* ---------- view modes (R3.2: source / reading per tab) ---------- */
const ICON_BOOK = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>';
const ICON_PEN = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z"/></svg>';

function updateModeBtn(g) {
  const tb = g.active >= 0 ? g.tabs[g.active] : null;
  g.modebtn.innerHTML = tb && tb.mode === "reading" ? ICON_PEN : ICON_BOOK;
}

function applyMode(g) {  // reading = preview fills the pane, editor hidden
  const m = g.active >= 0 ? g.tabs[g.active].mode : "source";
  g.editor.style.display = m === "reading" ? "none" : "";
  g.preview.style.display = "";
  updateModeBtn(g);
}

async function cmdToggleMode(g) {  // Ctrl+E / mode button
  g = g || fg();
  if (!g || g.active < 0 || g.graphOn) return;
  const tab = g.tabs[g.active];
  if (tab.mode === "source") await flushSave(g);
  tab.mode = tab.mode === "source" ? "reading" : "source";
  hideAc();
  applyMode(g);
  if (tab.mode === "source") g.editor.focus();
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
    x.textContent = "✕";
    x.onclick = e => { e.stopPropagation(); closeTab(g, i); };
    d.append(ttl, x);
    d.onclick = () => switchTab(g, i);
    d.oncontextmenu = e => tabMenu(e, g, i);   // R6.2: split verbs
    g.tabsEl.appendChild(d);
  });
  updateModeBtn(g);
  updateTitle();                    // keep [tabs:] census fresh on tab changes
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
  showEditor(g);
  const n = curOf(g);
  g.editor.value = n ? await inv("read_note", { name: n }) : "";
  if (n && g.tabs[g.active].mode === "source") g.editor.focus();
  await preview(g);
  renderTabs(g);
  await refreshTree();
  await updateStatus(g);
  await lgFollow(g);                // R7.3: linked localgraphs track this group
}

// R7.3: any localgraph tab linked to `src` re-centers on src's active note
async function lgFollow(src) {
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
  await flushSave(g);
  g.active = i;
  await loadActive(g);
}

async function openInTab(name) {   // explorer click -> FOCUSED group (R6.3)
  const g = fg();
  await flushSave(g);
  const i = g.tabs.findIndex(x => x.name === name);
  if (i >= 0) g.active = i;
  else { g.tabs.push(mkTab(name)); g.active = g.tabs.length - 1; }
  await loadActive(g);
}

async function navigate(g, name) { // wikilink / graph click: replace g's ACTIVE tab, push history
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

function renderNode(node, prefix, depth, out) {
  for (const d of [...node.dirs.keys()].sort()) {
    const full = prefix ? prefix + "/" + d : d;
    const row = document.createElement("div");
    row.className = "trow folder";
    row.style.paddingLeft = 12 + depth * 14 + "px";
    row.textContent = (collapsed.has(full) ? "▸ " : "▾ ") + d;
    row.onclick = () => {
      collapsed.has(full) ? collapsed.delete(full) : collapsed.add(full);
      refreshTree();
    };
    out.appendChild(row);
    if (!collapsed.has(full)) renderNode(node.dirs.get(d), full, depth + 1, out);
  }
  for (const nm of [...node.notes].sort()) {
    const row = document.createElement("div");
    row.className = "trow note" + (nm === cur() ? " active" : "");
    row.style.paddingLeft = 12 + depth * 14 + "px";
    row.textContent = nm.split("/").pop();
    row.onclick = () => openInTab(nm);
    out.appendChild(row);
  }
}

async function refreshTree() {
  const [folders, notes] =
    await Promise.all([inv("list_folders"), inv("list_notes")]);
  notesCache = notes;
  const tree = $("tree");
  tree.innerHTML = "";
  renderNode(buildTree(folders, notes), "", 0, tree);
}

/* ---------- editor + preview (per group) ---------- */
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

const keymap = {
  "ctrl+n": cmdNewNote,
  "ctrl+s": cmdSave,
  "ctrl+w": cmdCloseTab,
  "ctrl+e": () => cmdToggleMode(),
  "ctrl+shift+g": () => cmdLocalGraph(),
  "alt+arrowleft": () => histGo(-1),
  "alt+arrowright": () => histGo(1),
};
document.addEventListener("keydown", e => {
  if (e.key === "Escape") {
    closeMenu();
    if (vaultPath && !$("picker").hidden) $("picker").hidden = true;
    if (!$("fnew").hidden) $("fnew").hidden = true;
    return;
  }
  const combo = (e.ctrlKey ? "ctrl+" : "") + (e.altKey ? "alt+" : "")
    + (e.shiftKey ? "shift+" : "") + e.key.toLowerCase();
  const fn = keymap[combo];
  if (fn) { e.preventDefault(); fn(); }
});

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
$("graphbtn").onclick = () => {
  if (!state) return;
  const g = fg(), t = g.active >= 0 ? g.tabs[g.active] : null;
  if (t && t.kind === "lg") return;   // localgraph tabs own their canvas
  if (g.graphOn) return showEditor(g);
  toggleGraph(g);
};
async function toggleGraph(g) {  // global graph (R4.x): whole vault, click navigates own group
  await startGraph(g, {
    fetch: () => inv("graph"),
    center: () => null,
    onClick: n => navigate(g, n),
  });
}
async function startGraph(g, cfg) {
  g.graphOn = true;
  hideAc();
  g.status.hidden = true;
  g.editor.style.display = "none"; g.preview.style.display = "none";
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
  function step() {
    const now = performance.now();
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
  const names = await inv("list_notes");
  if (names.length) await openInTab(names[0]);
  else renderTabs(g);
}
$("vswitch").onclick = showPicker;

(async () => {
  vaultPath = await inv("vault_get");
  if (vaultPath) await enterVault(); else showPicker();
})();

