const inv = (c, a) => window.__TAURI__.core.invoke(c, a);
const $ = id => document.getElementById(id);
let graphOn = false, sim = null;
let vaultPath = null, pmode = null, bpath = null;

/* ---------- tabs ---------- */
let tabs = [], active = -1;   // tabs: [{ name, mode: "source"|"reading", hist, hpos }]
const cur = () => (active >= 0 ? tabs[active].name : null);
const mkTab = name => ({ name, mode: "source", hist: [name], hpos: 0 });

let t = null;                               // autosave debounce timer
async function flushSave() {                // write pending edits NOW
  if (!t) return;
  clearTimeout(t); t = null;
  const n = cur();
  if (n) await inv("write_note", { name: n, content: $("editor").value });
}

/* ---------- view modes (R3.2: source / reading per tab) ---------- */
const ICON_BOOK = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M2 3h6a4 4 0 0 1 4 4v14a3 3 0 0 0-3-3H2z"/><path d="M22 3h-6a4 4 0 0 0-4 4v14a3 3 0 0 1 3-3h7z"/></svg>';
const ICON_PEN = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M17 3a2.8 2.8 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z"/></svg>';

function updateModeBtn() {
  const t = active >= 0 ? tabs[active] : null;
  $("modebtn").innerHTML = t && t.mode === "reading" ? ICON_PEN : ICON_BOOK;
}

function applyMode() {  // reading = preview fills the pane, editor hidden
  const m = active >= 0 ? tabs[active].mode : "source";
  $("editor").style.display = m === "reading" ? "none" : "";
  $("preview").style.display = "";
  updateModeBtn();
}

async function cmdToggleMode() {  // Ctrl+E / mode button
  if (active < 0 || graphOn) return;
  const tab = tabs[active];
  if (tab.mode === "source") await flushSave();
  tab.mode = tab.mode === "source" ? "reading" : "source";
  hideAc();
  applyMode();
  if (tab.mode === "source") $("editor").focus();
}

function renderTabs() {
  const bar = $("tabs");
  bar.innerHTML = "";
  tabs.forEach((tab, i) => {
    const d = document.createElement("div");
    d.className = "tab" + (i === active ? " active" : "");
    const ttl = document.createElement("span");
    ttl.className = "t";
    ttl.textContent = tab.name.split("/").pop();
    const x = document.createElement("span");
    x.className = "x";
    x.textContent = "✕";
    x.onclick = e => { e.stopPropagation(); closeTab(i); };
    d.append(ttl, x);
    d.onclick = () => switchTab(i);
    bar.appendChild(d);
  });
  updateModeBtn();
}

async function loadActive() {
  hideAc();
  showEditor();
  const n = cur();
  $("editor").value = n ? await inv("read_note", { name: n }) : "";
  if (n && tabs[active].mode === "source") $("editor").focus();
  await preview();
  renderTabs();
  await refreshTree();
  await updateStatus();
}

async function switchTab(i) {
  if (i === active) return;
  await flushSave();
  active = i;
  await loadActive();
}

async function openInTab(name) {   // explorer click: focus existing tab or open new
  await flushSave();
  const i = tabs.findIndex(x => x.name === name);
  if (i >= 0) active = i;
  else { tabs.push(mkTab(name)); active = tabs.length - 1; }
  await loadActive();
}

async function navigate(name) {    // wikilink / graph click: replace ACTIVE tab, push history
  await flushSave();
  if (active < 0) { tabs.push(mkTab(name)); active = 0; }
  else {
    const tab = tabs[active];
    tab.name = name;
    tab.hist = tab.hist.slice(0, tab.hpos + 1);
    tab.hist.push(name);
    tab.hpos++;
  }
  await loadActive();
}

async function histGo(d) {         // per-tab back/forward (Alt+Left / Alt+Right)
  if (active < 0) return;
  const tab = tabs[active];
  const p = tab.hpos + d;
  if (p < 0 || p >= tab.hist.length) return;
  await flushSave();
  tab.hpos = p;
  tab.name = tab.hist[p];
  await loadActive();
}

async function closeTab(i) {
  if (i === active) await flushSave();
  tabs.splice(i, 1);
  if (active >= tabs.length) active = tabs.length - 1;
  else if (i < active) active--;
  await loadActive();
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

/* ---------- editor + preview ---------- */
async function preview() {
  $("preview").innerHTML = await inv("render", { content: $("editor").value });
  for (const a of $("preview").querySelectorAll("a.wiki"))
    a.onclick = async e => {
      e.preventDefault();
      const n = a.dataset.note;
      if (a.classList.contains("wiki-unresolved"))   // R3.5: click creates the note
        await inv("write_note", { name: n, content: "" });
      navigate(n);
    };
}

function scheduleSave() {
  clearTimeout(t);
  t = setTimeout(async () => {
    t = null;
    const n = cur();
    if (n) await inv("write_note", { name: n, content: $("editor").value });
    preview();
    updateStatus();
  }, 250);
}
$("editor").addEventListener("input", () => { scheduleSave(); showAc(); });

/* ---------- status bar (R2.7) ---------- */
async function updateStatus() {
  const n = cur();
  if (!n || graphOn) { $("status").hidden = true; return; }
  const v = $("editor").value;
  const w = (v.match(/\S+/g) || []).length;
  $("st-wc").textContent = w + (w === 1 ? " word" : " words");
  $("st-cc").textContent = v.length + (v.length === 1 ? " char" : " chars");
  const bl = await inv("backlinks", { name: n });
  $("st-bl").textContent = bl.length + (bl.length === 1 ? " backlink" : " backlinks");
  $("status").hidden = false;
}

/* ---------- [[ autocomplete (R3.4) ---------- */
let notesCache = [], acItems = [], acSel = 0, acStart = -1;

function hideAc() { $("ac").hidden = true; acItems = []; acStart = -1; }

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

function acContext() {  // caret inside an unclosed [[ on one line?
  const ed = $("editor");
  const upto = ed.value.slice(0, ed.selectionStart);
  const a = upto.lastIndexOf("[[");
  if (a < 0) return null;
  const frag = upto.slice(a + 2);
  if (frag.includes("]]") || frag.includes("\n")) return null;
  return { start: a, q: frag };
}

function caretXY() {    // approximate caret position within #content
  const ed = $("editor");
  const lines = ed.value.slice(0, ed.selectionStart).split("\n");
  const col = lines[lines.length - 1].length;
  const x = ed.offsetLeft + 16 + Math.min(col * 7.8, ed.clientWidth - 40);
  const y = ed.offsetTop + 16 + lines.length * 20.8 - ed.scrollTop;
  return [x, y];
}

function showAc() {
  const ctx = acContext();
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
  const box = $("ac");
  box.innerHTML = "";
  acItems.forEach((n, i) => {
    const d = document.createElement("div");
    d.textContent = n;
    if (i === acSel) d.className = "sel";
    d.onmousedown = e => { e.preventDefault(); acInsert(n); };
    box.appendChild(d);
  });
  const [x, y] = caretXY();
  box.style.left = Math.max(0, Math.min(x, $("content").clientWidth - 200)) + "px";
  box.style.top = Math.min(y, $("content").clientHeight - 60) + "px";
  box.hidden = false;
}

function acInsert(name) {
  const ed = $("editor");
  const end = ed.selectionStart;
  ed.value = ed.value.slice(0, acStart) + "[[" + name + "]]" + ed.value.slice(end);
  const p = acStart + name.length + 4;
  ed.setSelectionRange(p, p);
  hideAc();
  ed.focus();
  scheduleSave();
}

$("editor").addEventListener("keydown", e => {
  if ($("ac").hidden) return;
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault(); e.stopPropagation();
    acSel = (acSel + (e.key === "ArrowDown" ? 1 : acItems.length - 1)) % acItems.length;
    [...$("ac").children].forEach((d, i) => d.className = i === acSel ? "sel" : "");
  } else if (e.key === "Enter" || e.key === "Tab") {
    e.preventDefault(); e.stopPropagation();
    acInsert(acItems[acSel]);
  } else if (e.key === "Escape") {
    e.stopPropagation();
    hideAc();
  }
});
$("editor").addEventListener("blur", () => setTimeout(hideAc, 100));

/* ---------- commands + keymap ---------- */
async function cmdNewNote() {                // new note in a NEW tab
  await flushSave();
  const name = "Untitled-" + Date.now() % 10000;
  await inv("write_note", { name, content: "# " + name + "\n" });
  tabs.push(mkTab(name));
  active = tabs.length - 1;
  await loadActive();
}
async function cmdSave() {                   // force save, no debounce
  const n = cur();
  if (!n) return;
  clearTimeout(t); t = null;
  await inv("write_note", { name: n, content: $("editor").value });
  await preview();
  await updateStatus();
}
async function cmdCloseTab() {
  if (active >= 0) await closeTab(active);
}

const keymap = {
  "ctrl+n": cmdNewNote,
  "ctrl+s": cmdSave,
  "ctrl+w": cmdCloseTab,
  "ctrl+e": cmdToggleMode,
  "alt+arrowleft": () => histGo(-1),
  "alt+arrowright": () => histGo(1),
};
document.addEventListener("keydown", e => {
  if (e.key === "Escape") {
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
$("modebtn").onclick = cmdToggleMode;
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

/* ---------- graph ---------- */
function showEditor() {
  graphOn = false; cancelAnimationFrame(sim);
  $("graph").hidden = true;
  applyMode();
}
$("graphbtn").onclick = async () => {
  if (graphOn) return showEditor();
  graphOn = true;
  hideAc();
  $("status").hidden = true;
  $("editor").style.display = "none"; $("preview").style.display = "none";
  const cv = $("graph"); cv.hidden = false;
  cv.width = cv.clientWidth; cv.height = cv.clientHeight;
  const g = await inv("graph");
  const N = g.nodes.map((nd, i) => ({
    n: nd.name, resolved: nd.resolved,
    x: cv.width / 2 + 120 * Math.cos(i), y: cv.height / 2 + 120 * Math.sin(i),
    vx: 0, vy: 0
  }));
  const ctx = cv.getContext("2d");
  function step() {
    for (const a of N) for (const b of N) {
      if (a === b) continue;
      const dx = a.x - b.x, dy = a.y - b.y, d2 = dx * dx + dy * dy + 0.01;
      a.vx += 800 * dx / d2; a.vy += 800 * dy / d2;          // repulsion
    }
    for (const [i, j] of g.edges) {
      const a = N[i], b = N[j], dx = b.x - a.x, dy = b.y - a.y;
      a.vx += dx * 0.005; a.vy += dy * 0.005;                 // spring
      b.vx -= dx * 0.005; b.vy -= dy * 0.005;
    }
    for (const p of N) {
      p.vx += (cv.width / 2 - p.x) * 0.01; p.vy += (cv.height / 2 - p.y) * 0.01;
      p.vx *= 0.85; p.vy *= 0.85; p.x += p.vx; p.y += p.vy;
      const m = 30;   // keep nodes (and labels) inside the viewport
      p.x = Math.max(m, Math.min(cv.width - m, p.x));
      p.y = Math.max(m, Math.min(cv.height - m, p.y));
    }
    ctx.clearRect(0, 0, cv.width, cv.height);
    ctx.strokeStyle = "#45475a";
    for (const [i, j] of g.edges) {
      ctx.beginPath(); ctx.moveTo(N[i].x, N[i].y); ctx.lineTo(N[j].x, N[j].y); ctx.stroke();
    }
    ctx.fillStyle = "#89b4fa"; ctx.textAlign = "center"; ctx.font = "12px sans-serif";
    for (const p of N) {
      ctx.beginPath(); ctx.arc(p.x, p.y, 6, 0, 7); ctx.fill();
      ctx.fillText(p.n, p.x, p.y - 10);
    }
    sim = requestAnimationFrame(step);
  }
  step();
  cv.onclick = e => {
    const r = cv.getBoundingClientRect();
    const x = e.clientX - r.left, y = e.clientY - r.top;
    const hit = N.find(p => (p.x - x) ** 2 + (p.y - y) ** 2 < 144);
    if (hit) navigate(hit.n);
  };
};

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
  tabs = []; active = -1; collapsed = new Set();
  hideAc();
  $("status").hidden = true;
  showEditor();
  $("editor").value = ""; $("preview").innerHTML = "";
  await refreshTree();
  const names = await inv("list_notes");
  if (names.length) await openInTab(names[0]);
  else renderTabs();
}
$("vswitch").onclick = showPicker;

(async () => {
  vaultPath = await inv("vault_get");
  if (vaultPath) await enterVault(); else showPicker();
})();
