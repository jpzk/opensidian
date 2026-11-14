// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* canvas menus (goal cv-menus): everything on the board is reachable from a menu —
   the card / selection / board context menus, the pickers and prompts they open,
   the card toolbar at the bottom of the board and drops from the file explorer.
   Written from the public help pages and the JSON Canvas 1.0 spec (clean room).

   Load order (index.html): canvas.js -> canvas-history.js -> canvas-cards.js ->
   canvas-select.js -> canvas-menus.js -> main.js. This file runs NO code at load
   beyond defining window.CanvasMenus; canvas.js calls install(canvas, ctx) once
   per board at the end of wire() (ctx = {X, app, deps, changed, NODE_CLASS}).

   Right-click a card, a selection or the board -> the context menus listed at "the context
   menus" below (goal cvf-menus1: stock lists, order, groups and icons).
   The card toolbar (bottom centre) holds Text / Note / Media: a click adds at the
   centre of the view, a drag adds at the release point (note / media ask the picker).
   A file dragged from the explorer (app.dragManager.draggable) drops as a file card
   centred on the drop point.
   Every edit ends in canvas.requestSave(), so cv-history makes it undoable. */
(function () {
  "use strict";
  const IMG = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif"]);
  const MEDIA = new Set([...IMG, "mp3", "wav", "m4a", "ogg", "3gp", "flac", "mp4", "webm", "ogv", "mov", "mkv", "pdf"]);
  const PAD = 20;                       // group padding around the selection, canvas units
  const STYLES = [["cover", "Cover"], ["ratio", "Fit"], ["repeat", "Repeat"]];
  let open = null;                      // the picker / prompt on screen: {kind, type: "pick"|"prompt", m, input}
  let styled = false;

  const el = (tag, cls, parent) => { const e = document.createElement(tag); if (cls) e.className = cls; if (parent) parent.appendChild(e); return e; };
  const extOf = p => { const n = String(p || "").split("/").pop(); const d = n.lastIndexOf("."); return d > 0 ? n.slice(d + 1).toLowerCase() : ""; };
  const dirOf = p => { const s = String(p || ""); const i = s.lastIndexOf("/"); return i > 0 ? s.slice(0, i) : ""; };
  const kindOf = p => { const x = extOf(p); return x === "md" ? "note" : MEDIA.has(x) ? "media" : x === "canvas" ? "canvas" : "file"; };
  const ro = c => !!(c.readonly || (c.options && c.options.readOnly));
  const C = c => c._cvm;
  const ping = c => { const s = C(c); if (s && typeof s.ctx.changed === "function") try { s.ctx.changed(); } catch (_) {} };
  const later = c => setTimeout(() => ping(c), 0);
  const notify = (c, msg) => { const d = C(c) && C(c).ctx.deps; if (d && typeof d.notify === "function") d.notify(msg); };

  function style() {
    if (styled) return; styled = true;
    const s = document.createElement("style");
    s.dataset.owner = "canvas-menus";
    s.textContent = [
      ".canvas-card-menu { position: absolute; left: 50%; bottom: 12px; transform: translateX(-50%); z-index: 4; display: flex; gap: 2px; padding: 4px; border-radius: 8px; background: var(--background-secondary, #262626); border: 1px solid var(--background-modifier-border, #444); box-shadow: 0 2px 8px rgba(0,0,0,.3); user-select: none }",
      ".canvas-card-menu-item { font: inherit; font-size: 12px; min-width: 54px; padding: 4px 10px; border: 0; border-radius: 5px; background: transparent; color: var(--text-muted, #aaa); cursor: grab }",
      ".canvas-card-menu-item:hover { background: var(--background-modifier-hover, #333); color: var(--text-normal, #ddd) }",
      ".canvas-wrapper.is-card-dragging, .canvas-wrapper.is-card-dragging * { cursor: copy }",
      ".canvas-menus-prompt input { width: 100%; box-sizing: border-box; font: inherit; padding: 6px 8px }",
      ".canvas-menus-prompt .canvas-menus-hint { margin-top: 6px; font-size: 12px; color: var(--text-muted, #aaa) }",
    ].join("\n");
    document.head.appendChild(s);
  }

  /* ---------------------------------------------- mutations (all end in requestSave) */
  function addData(c, d, front) {
    const N = C(c).ctx.NODE_CLASS[d.type];
    const n = c.addNode(new N(c, d));
    if (front) {                        // a new group goes FIRST in nodes[] (JSON Canvas: lower index = below)
      const rest = [...c.nodes].filter(([k]) => k !== n.id);
      c.nodes.clear(); c.nodes.set(n.id, n); for (const [k, v] of rest) c.nodes.set(k, v);
    }
    c.renderEdges(); c.requestSave(); later(c);
    return n;
  }
  // the node's object changed kind (text -> file): a new node class over the SAME object, same id, same place in nodes[] and the layer
  function replaceNode(c, old, d) {
    const N = C(c).ctx.NODE_CLASS[d.type];
    if (old.isEditing) try { old.stopEditing(); } catch (_) {}
    const nn = new N(c, d), sel = c.selection.has(old);
    const layer = d.type === "group" ? c.groupEl : c.nodeEl, next = old.containerEl.parentNode === layer ? old.containerEl.nextSibling : null;
    const order = [...c.nodes];
    c.selection.delete(old); old.detach();
    c.nodes.clear(); for (const [k, v] of order) c.nodes.set(k, k === old.id ? nn : v);
    layer.insertBefore(nn.containerEl, next);
    nn.load(); nn.render();
    if (sel) c.select(nn, false);
    c.renderEdges(); c.requestSave(); later(c);
    return nn;
  }
  function newId(c) {
    const taken = c.ids();
    for (;;) { const b = new Uint8Array(8); crypto.getRandomValues(b); const id = Array.from(b, x => x.toString(16).padStart(2, "0")).join(""); if (!taken.has(id)) return id; }
  }
  // goal cvf-groups (CVR-B-080): the board's Create group is a 400x400 "Untitled group" at the point, selected,
  // its label in edit — the same finish as makeGroup() for a selection
  function groupAt(c, p) {
    const g = addData(c, { id: newId(c), type: "group", x: Math.round(p.x), y: Math.round(p.y), width: 400, height: 400, label: "Untitled group" }, true);
    if (g) { c.select(g, false); editLabel(c, g); }
    return g;
  }
  function groupAround(c, nodes) {
    if (!nodes.length) return null;
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of nodes) { const d = n.data; x0 = Math.min(x0, d.x); y0 = Math.min(y0, d.y); x1 = Math.max(x1, d.x + d.width); y1 = Math.max(y1, d.y + d.height); }
    const g = addData(c, { id: newId(c), type: "group", x: Math.floor(x0 - PAD), y: Math.floor(y0 - PAD), width: Math.ceil(x1 - x0 + 2 * PAD), height: Math.ceil(y1 - y0 + 2 * PAD), label: "Untitled group" }, true);
    c.select(g, false);
    return g;
  }
  function addFile(c, path, pos, center) {
    const w = 400, h = kindOf(path) === "note" ? 400 : 300;
    const n = c.createFileNode({ file: path, pos: center ? { x: pos.x - w / 2, y: pos.y - h / 2 } : pos, size: { width: w, height: h } });
    c.renderEdges(); later(c);
    return n;
  }
  function addLink(c, url, pos) {
    return addData(c, { id: newId(c), type: "link", url, x: Math.round(pos.x), y: Math.round(pos.y), width: 400, height: 400 });
  }
  async function convertToFile(c, n, name) {
    const { app } = C(c).ctx;
    name = String(name || "").trim().replace(/\.md$/i, "").replace(/[\\:*?"<>|]/g, "").replace(/^\/+|\/+$/g, "");
    if (!name) return;
    const dir = dirOf(c.path), path = (dir ? dir + "/" : "") + name + ".md";
    if (app.vault.getAbstractFileByPath(path)) { notify(c, "Canvas: " + path + " already exists — pick another name"); return; }
    if (c.nodes.get(n.id) !== n || n.data.type !== "text") return;
    try { await app.vault.create(path, n.data.text); }
    catch (e) { notify(c, "Canvas: " + path + " was not created — " + String((e && e.message) || e)); return; }
    if (c.nodes.get(n.id) !== n) return;
    const d = n.data;
    delete d.text; d.type = "file"; d.file = path;
    replaceNode(c, n, d);
  }
  function swapFile(c, n, path) { if (c.nodes.get(n.id) !== n) return; n.setFilePath(path, ""); n.renderContent(); c.requestSave(); later(c); }
  function narrow(c, n, sub) { if (c.nodes.get(n.id) !== n) return; n.setFilePath(n.data.file, sub); n.renderContent(); c.requestSave(); later(c); }
  function setBackground(c, g, path, how) {
    const d = g.data;
    if (path === null) { delete d.background; delete d.backgroundStyle; }
    else if (path !== undefined) d.background = path;
    if (how) d.backgroundStyle = how;
    g.renderContent(); c.requestSave(); later(c);
  }

  /* ---------------------------------------------- pickers and prompts */
  function files(c, pred) {
    const { app } = C(c).ctx;
    let fs = [];
    try { fs = app.vault.getFiles(); } catch (_) {}
    return fs.filter(f => f && typeof f.path === "string" && pred(f)).sort((a, b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
  }
  function picker(c, kind, placeholder, items, text, choose) {
    const { X, app } = C(c).ctx;
    if (!items.length) { notify(c, "Canvas: nothing to pick (" + kind + ")"); return null; }
    class Pick extends X.FuzzySuggestModal {
      getItems() { return items; }
      getItemText(i) { return text(i); }
      onChooseItem(i) { setTimeout(() => { try { choose(i); } catch (e) { console.error("[canvas] menu:", e); } }, 0); }
      onClose() { if (open && open.m === this) open = null; later(c); }
    }
    const m = new Pick(app);
    m.setPlaceholder(placeholder);
    open = { kind, type: "pick", m, input: m.inputEl };
    m.inputEl.addEventListener("input", () => later(c));
    m.open(); later(c);
    return m;
  }
  function prompt(c, kind, title, value, hint, submit) {
    const { X, app } = C(c).ctx;
    const m = new X.Modal(app);
    m.modalEl.classList.add("canvas-menus-prompt");
    m.setTitle(title);
    const input = el("input", "", m.contentEl);
    input.type = "text"; input.spellcheck = false; input.value = value || "";
    if (hint) el("div", "canvas-menus-hint", m.contentEl).textContent = hint;
    input.addEventListener("input", () => later(c));
    input.addEventListener("keydown", e => {
      e.stopPropagation();
      if (e.key === "Enter") { e.preventDefault(); const v = input.value; m.close(); setTimeout(() => { try { submit(v); } catch (er) { console.error("[canvas] menu:", er); } }, 0); }
      else if (e.key === "Escape") { e.preventDefault(); m.close(); }
    });
    m.onClose = () => { if (open && open.m === m) open = null; later(c); };
    open = { kind, type: "prompt", m, input };
    m.open();
    // focus the field once the opening click is over; a late focus move (the card re-rendering under
    // the closed menu) must not leave the typing outside the prompt, so check again while it is fresh
    let typed = false;
    input.addEventListener("keydown", () => { typed = true; }, { once: true });
    const grab = sel => {
      if (!m.containerEl.isConnected || typed || document.activeElement === input) return;
      try { input.focus(); if (sel) input.select(); } catch (_) {}
    };
    setTimeout(() => { grab(true); later(c); }, 0);
    setTimeout(() => grab(true), 80);
    setTimeout(() => grab(true), 250);
    return m;
  }
  const pickNote = (c, then) => picker(c, "note", "Add a note from the vault…", files(c, f => f.extension === "md"), f => f.path, f => then(f.path));
  const pickMedia = (c, then) => picker(c, "media", "Add media from the vault…", files(c, f => MEDIA.has(String(f.extension).toLowerCase())), f => f.path, f => then(f.path));
  // the note's headings and ^blocks, read from its text (fenced code skipped)
  async function sections(c, n) {
    const { app } = C(c).ctx;
    const f = n.file || (app.vault.getAbstractFileByPath && app.vault.getAbstractFileByPath(n.data.file));
    if (!f) return [];
    let t = "";
    try { t = await app.vault.cachedRead(f); } catch (_) { return []; }
    const out = []; let fence = false;
    for (const line of t.split("\n")) {
      if (/^\s*(```|~~~)/.test(line)) { fence = !fence; continue; }
      if (fence) continue;
      const h = /^(#{1,6})\s+(.+?)\s*#*\s*$/.exec(line);
      if (h) { out.push({ label: h[2], sub: "#" + h[2] }); continue; }
      const b = /\s\^([A-Za-z0-9-]+)\s*$/.exec(line);
      if (b) out.push({ label: "^" + b[1] + "  " + line.replace(/\s\^[A-Za-z0-9-]+\s*$/, "").trim().slice(0, 40), sub: "#^" + b[1] });
    }
    return out;
  }

  /* ---------------------------------------------- the context menus (goal cvf-menus1)
     Every menu lists what stock lists, in its order and groups (a separator between groups),
     every row with an icon of our own (drawn below, 16x16 strokes):
       board   Add card, Add note from vault, Add media from vault, Add web page, Create group |
               Undo (once the board has history) | Paste | Snap to grid, Snap to objects, Read-only
               (the three settings show their state as a check; a read-only board still opens
               it, its editing rows disabled, so Read-only can be turned off again)
       text    Zoom to selection, Edit, Convert to file | Remove
       note    Zoom to selection, Swap file, Edit, Narrow to heading, Narrow to block |
               Open in new tab, Open in new window | Rename, Move file to, Bookmark, Merge entire
               file with | Open in default app, Show in system explorer, Reveal file in navigation |
               Remove | Copy path
       media   the note menu without Edit, the two Narrow rows, Open in new tab and Merge
       link    Zoom to selection, Change URL, Open external link, Copy URL, Reload page | Remove
               (Copy URL / Change URL come from canvas-files' canvas:node-menu hook; they are
               moved into their place here)
       group   label: Zoom to selection, Edit label | Remove
               body:  the same, the background rows before Remove (cv-menus' Set / Replace /
                      Remove background, Background style)
       several Zoom to selection, Create group | Cut, Copy, Paste | Delete
     Rows the app has no machinery for (one window, no file manager, no merge) are shown
     disabled with the reason as their tooltip. */
  const SVG_NS = "http://www.w3.org/2000/svg";
  const PATHS = {
    zoom: "M7 2.5a4.5 4.5 0 1 0 0 9a4.5 4.5 0 1 0 0-9zM10.3 10.3L14 14",
    edit: "M10.5 2.5l3 3L6 13H3v-3z",
    page: "M4 1.5h5l3 3v10H4zM9 1.5v3h3",
    trash: "M2.5 4h11M6 4V2.5h4V4M4 4l.8 10h6.4L12 4",
    swap: "M2 5h10L9.5 2.5M14 11H4l2.5 2.5",
    heading: "M3.5 3v10M9.5 3v10M3.5 8h6M12.5 9v4",
    block: "M2.5 4.5h11M2.5 8h11M2.5 11.5h6",
    tab: "M2 4h12v9H2zM2 7h12M6 4v3",
    win: "M4.5 5h9v8h-9zM2.5 3h9v2M2.5 3v8h2",
    rename: "M3 13h10M5 10.5L8 3l3 7.5M6 8h4",
    move: "M1.5 4h5L8 5.5h6.5v8h-13zM7 9.5h5M10.5 8L12 9.5L10.5 11",
    mark: "M4 2h8v12l-4-3-4 3z",
    merge: "M4 2v4c0 2 4 3 4 5v3M12 2v4c0 2-4 3-4 5",
    ext: "M9 2.5h4.5V7M13.5 2.5l-6 6M11.5 9.5v4h-9v-9h4",
    folder: "M1.5 4h5L8 5.5h6.5v8h-13z",
    eye: "M2 8s2.5-4 6-4 6 4 6 4-2.5 4-6 4-6-4-6-4zM8 6.5a1.5 1.5 0 1 0 0 3a1.5 1.5 0 1 0 0-3z",
    copy: "M5.5 5.5h8v8h-8zM10.5 5.5v-3h-8v8h3",
    cut: "M4.5 10a2 2 0 1 0 0 4a2 2 0 1 0 0-4zM11.5 10a2 2 0 1 0 0 4a2 2 0 1 0 0-4zM6 10.5L12 2M10 10.5L4 2",
    paste: "M5 3H3.5v11h9V3H11M6 2h4v2.5H6z",
    link: "M7 9a3 3 0 0 0 4.2 0l2-2a3 3 0 0 0-4.2-4.2l-1 1M9 7a3 3 0 0 0-4.2 0l-2 2a3 3 0 0 0 4.2 4.2l1-1",
    reload: "M13 8a5 5 0 1 1-1.5-3.5M13 2.5v3h-3",
    group: "M2.5 5V2.5H5M11 2.5h2.5V5M13.5 11v2.5H11M5 13.5H2.5V11",
    card: "M2 3.5h12v9H2zM4.5 6.5h7M4.5 9.5h4",
    note: "M4 1.5h5l3 3v10H4zM6 8h4M6 11h4",
    media: "M2 3h12v10H2zM2 11l4-4 3 3 2-2 3 3M10.5 5.5h.01",
    web: "M8 2a6 6 0 1 0 0 12A6 6 0 1 0 8 2zM2 8h12M8 2c-2 2-2 10 0 12M8 2c2 2 2 10 0 12",
    undo: "M5 3L2 6l3 3M2 6h7a4 4 0 0 1 0 8H6",
    grid: "M2 2h12v12H2zM6 2v12M10 2v12M2 6h12M2 10h12",
    objects: "M2 2h5v5H2zM9 9h5v5H9zM7 4.5h4.5V9",
    lock: "M4 7h8v7H4zM5.5 7V5a2.5 2.5 0 0 1 5 0v2",
    label: "M2 2.5h6l6 6-5.5 5.5-6-6zM5 5h.01",
  };
  const ICON_OF = {
    "Add card": "card", "Add note from vault": "note", "Add media from vault": "media", "Add web page": "web",
    "Create group": "group", "Undo": "undo", "Paste": "paste", "Snap to grid": "grid", "Snap to objects": "objects",
    "Read-only": "lock", "Zoom to selection": "zoom", "Edit": "edit", "Convert to file": "page", "Remove": "trash",
    "Delete": "trash", "Swap file": "swap", "Narrow to heading": "heading", "Narrow to block": "block",
    "Open in new tab": "tab", "Open in new window": "win", "Rename": "rename", "Move file to": "move",
    "Bookmark": "mark", "Remove bookmark": "mark", "Merge entire file with": "merge", "Open in default app": "ext",
    "Show in system explorer": "folder", "Reveal file in navigation": "eye", "Copy path": "copy", "Cut": "cut",
    "Copy": "copy", "Change URL": "link", "Open external link": "ext", "Copy URL": "copy", "Reload page": "reload",
    "Edit label": "label", "Set background": "media", "Replace background": "media", "Remove background": "trash",
    "Background style: Cover": "media", "Background style: Fit": "media", "Background style: Repeat": "media",
  };
  const titleOf = it => it && it.titleEl ? String(it.titleEl.textContent || "") : null;
  function iconize(it) {
    const d = PATHS[ICON_OF[String(titleOf(it) || "").replace(/(\.\.\.|…)$/, "")]];
    if (!d || !it.iconEl || it.iconEl.querySelector("svg")) return;
    const s = document.createElementNS(SVG_NS, "svg");
    s.setAttribute("viewBox", "0 0 16 16"); s.setAttribute("fill", "none"); s.setAttribute("stroke", "currentColor");
    s.setAttribute("stroke-width", "1.4"); s.setAttribute("stroke-linecap", "round"); s.setAttribute("stroke-linejoin", "round");
    s.setAttribute("aria-hidden", "true");
    const p = document.createElementNS(SVG_NS, "path"); p.setAttribute("d", d); s.appendChild(p);
    it.iconEl.textContent = ""; it.iconEl.appendChild(s);
  }
  let shown = null;                     // the last context menu shown here (census [cvmm:])

  const H = () => window.CanvasHistory || null;
  function zoomSel(c) {
    const nav = window.CanvasNav, s = window.CanvasSelect;
    if (nav && typeof nav.zoomToSelection === "function") nav.zoomToSelection(c);
    else if (s && typeof s.zoomToSelection === "function") s.zoomToSelection(c);
    later(c);
  }
  function setOpt(c, k) {
    c.options = c.options || { snapToGrid: true, snapToObjects: true, readOnly: false };
    c.options[k] = k === "readOnly" ? !c.options.readOnly : c.options[k] === false;
    if (k === "readOnly") {
      c.wrapperEl.classList.toggle("is-readonly", !!c.readonly);
      if (c.readonly) for (const n of c.nodes.values()) if (n.isEditing) try { n.stopEditing(); } catch (_) {}
    }
    later(c);
  }
  // the group label's own in-place editor (canvas-cards opens it on a double click on the label)
  function editLabel(c, g) {
    setTimeout(() => {
      const lab = g && g.labelEl;
      if (!lab || c.nodes.get(g.id) !== g || ro(c)) return;
      lab.dispatchEvent(new MouseEvent("dblclick", { bubbles: true, cancelable: true }));
      later(c);
    }, 0);
  }
  // a note card's in-place editor, focused with the caret at the end (a double click focuses it by landing in it)
  function editNote(c, n) {
    if (ro(c) || c.nodes.get(n.id) !== n) return;
    n.startEditing();
    setTimeout(() => {
      const em = n.child && n.child.editMode, cm = em && em.cm;
      try {
        if (cm) { cm.focus(); cm.dispatch({ selection: { anchor: cm.state.doc.length } }); }
        else { const t = n.containerEl.querySelector(".cm-content, textarea"); if (t) t.focus(); }
      } catch (_) {}
      later(c);
    }, 30);
  }
  function makeGroup(c, nodes) { const g = groupAround(c, nodes); if (g) editLabel(c, g); return g; }
  function copyText(t) {
    const back = document.activeElement, ta = el("textarea", "", document.body);
    ta.value = t; ta.setAttribute("readonly", "");
    ta.style.cssText = "position:fixed;left:-10000px;top:0;width:10px;height:10px;opacity:0";
    const on = e => { try { e.clipboardData.setData("text/plain", t); e.preventDefault(); } catch (_) {} };
    document.addEventListener("copy", on, true);
    try { ta.focus(); ta.select(); document.execCommand("copy"); } catch (_) {}
    document.removeEventListener("copy", on, true);
    ta.remove();
    try { if (back && back.focus) back.focus({ preventScroll: true }); } catch (_) {}
  }
  // the app's external-link gate: main.js hands a click on an a.ext to the OS browser
  function openExt(url) {
    if (!/^https?:\/\//i.test(String(url || ""))) return;
    const a = el("a", "ext", document.body);
    a.href = url; a.hidden = true;
    a.addEventListener("click", e => e.preventDefault());
    try { a.click(); } finally { a.remove(); }
  }
  const G = name => { try { return typeof window[name] === "function" ? window[name] : null; } catch (_) { return null; } };
  const noteName = p => String(p || "").replace(/\.md$/i, "");
  function bookmarked(p) { try { return typeof bmCache !== "undefined" && Array.isArray(bmCache) && bmCache.includes(noteName(p)); } catch (_) { return false; } }   // main.js (global lexical)
  function reveal(c, p) {
    const k = kindOf(p), rv = G("bmReveal");
    if (rv) rv(k === "note" ? noteName(p) : p);
    if (k !== "note") {
      const r = document.querySelector('#tree .trow.file[data-file="' + (window.CSS && CSS.escape ? CSS.escape(p) : p) + '"]');
      if (r) { document.querySelectorAll("#tree .trow.revealed").forEach(x => x.classList.remove("revealed")); r.classList.add("revealed"); r.scrollIntoView({ block: "nearest" }); }
    }
    later(c);
  }
  // a file card follows its file when the menu renamed / moved it
  function relink(c, old, now) {
    if (!now || now === old) return;
    let hit = false;
    for (const x of c.nodes.values()) if (x.data && x.data.type === "file" && x.data.file === old) { x.setFilePath(now, x.data.subpath || ""); x.renderContent(); hit = true; }
    if (hit) { c.requestSave(); later(c); }
  }
  // Rename: the file's name without its extension, selected; Enter renames in place (same folder, same extension)
  function renameFile(c, n) {
    const { app } = C(c).ctx, old = n.data.file, f = app.vault.getAbstractFileByPath(old);
    if (!f) { notify(c, "Canvas: " + old + " is not in the vault"); return; }
    const name = old.split("/").pop(), ext = extOf(old), stem = ext ? name.slice(0, name.length - ext.length - 1) : name;
    prompt(c, "rename", "Rename file", stem, "", async v => {
      const nm = String(v || "").trim();
      if (!nm || nm === stem) return;
      if (/[\\/:]/.test(nm) || nm.startsWith(".")) { notify(c, "Canvas: a file name cannot contain \\ / : or start with a dot"); return; }
      const dir = dirOf(old), to = (dir ? dir + "/" : "") + nm + (ext ? "." + ext : "");
      if (app.vault.getAbstractFileByPath(to)) { notify(c, "Canvas: " + to + " already exists"); return; }
      try {
        try {
          if (app.fileManager && typeof app.fileManager.renameFile === "function") await app.fileManager.renameFile(f, to);
          else await app.vault.rename(f, to);
        } catch (e) {
          // restricted vault (plugin file access off -> EPERM): an audio/video/image card renames through the core's
          // narrow media_rename (vault-confined, same extension, no overwrite); everything else reports as before
          const T = window.__TAURI__, inv = T && T.core && T.core.invoke;
          if (!/EPERM/.test(String((e && e.message) || e)) || !/^(mp3|m4a|ogg|oga|opus|wav|flac|webm|mp4|m4v|mov|ogv|png|jpe?g|gif|webp)$/.test(ext) || !inv) throw e;
          await inv("media_rename", { from: old, to });
        }
      } catch (e) { notify(c, "Canvas: " + old + " was not renamed — " + String((e && e.message) || e)); return; }
      relink(c, old, to);
    });
  }
  function moveFile(c, n) {
    const { app } = C(c).ctx, old = n.data.file, f = app.vault.getAbstractFileByPath(old);
    if (!f) return;
    const here = dirOf(old), dirs = new Set(["/"]);
    for (const x of files(c, () => true)) { let d = dirOf(x.path); while (d) { dirs.add(d); d = dirOf(d); } }
    dirs.delete(here || "/");
    picker(c, "move", "Move " + f.name + " to…", [...dirs].sort(), s => s, async s => {
      const to = (s === "/" ? "" : s + "/") + f.name;
      try { await app.fileManager.renameFile(f, to); } catch (e) { notify(c, "Canvas: " + old + " was not moved — " + String((e && e.message) || e)); return; }
      relink(c, old, f.path);
    });
  }
  // the Enter URL modal: an empty field, Save and Cancel; Enter saves, Escape cancels
  function enterUrl(c, then) {
    const { X, app } = C(c).ctx;
    const m = new X.Modal(app);
    m.modalEl.classList.add("canvas-menus-prompt", "canvas-menus-url");
    m.setTitle("Enter URL");
    const input = el("input", "", m.contentEl);
    input.type = "text"; input.spellcheck = false; input.value = ""; input.placeholder = "https://";
    const row = el("div", "modal-button-container", m.contentEl);
    const save = el("button", "mod-cta", row), cancel = el("button", "mod-cancel", row);
    save.type = "button"; save.textContent = "Save"; cancel.type = "button"; cancel.textContent = "Cancel";
    const ok = () => {
      let u = String(input.value || "").trim();
      if (!u) { input.focus(); return; }
      if (!/^[a-z][a-z0-9+.-]*:/i.test(u)) u = "https://" + u;
      m.close();
      setTimeout(() => { try { then(u); } catch (er) { console.error("[canvas] menu:", er); } }, 0);
    };
    input.addEventListener("input", () => later(c));
    input.addEventListener("keydown", e => {
      e.stopPropagation();
      if (e.key === "Enter") { e.preventDefault(); ok(); }
      else if (e.key === "Escape") { e.preventDefault(); m.close(); }
    });
    save.addEventListener("click", ok);
    cancel.addEventListener("click", () => m.close());
    m.onClose = () => { if (open && open.m === m) open = null; later(c); };
    open = { kind: "url", type: "prompt", m, input, title: "Enter URL", btns: [["Save", save], ["Cancel", cancel]] };
    m.open();
    setTimeout(() => { try { input.focus(); } catch (_) {} later(c); }, 0);
    return m;
  }

  function nodeOf(c, t) { const e = t && t.closest ? t.closest(".canvas-node") : null; return e && c.nodes.get(e.dataset.nodeId) || null; }
  function onContext(c, e) {
    const t = e.target;
    if (t && t.closest && t.closest("textarea, input, .markdown-source-view, [contenteditable=true], .canvas-controls, .canvas-card-menu, .canvas-selection-toolbar")) return;
    e.preventDefault();
    const { X, app } = C(c).ctx;
    const n = nodeOf(c, t), lock = ro(c), h = H();
    if (n && lock) return;                // read-only cards: goal cvf-menus2
    const m = new X.Menu();
    const item = (title, fn, o) => m.addItem(i => {
      o = o || {};
      i.setTitle(title);
      if (o.on !== undefined) i.setChecked(o.on);
      if (o.off) { i.setDisabled(true); if (o.why) i.dom.title = o.why; }
      i.onClick(() => {
        if (o.off || !fn) return;
        // the click on the menu row left the focus on <body>: give it back to the board first, so its
        // keys (Ctrl+Z after Paste) work and a prompt the action opens returns the focus there
        const a = document.activeElement;
        if ((!a || a === document.body) && c.wrapperEl && c.wrapperEl.isConnected) try { c.wrapperEl.focus({ preventScroll: true }); } catch (_) {}
        try { const r = fn(); if (r && typeof r.catch === "function") r.catch(er => console.error("[canvas] menu:", er)); } catch (er) { console.error("[canvas] menu:", er); }
      });
    });
    const sep = () => { const l = m.items[m.items.length - 1]; if (l && titleOf(l) !== null) m.addSeparator(); };
    const zoom = () => item("Zoom to selection", () => zoomSel(c));
    const remove = (label) => { sep(); item(label || "Remove", () => c.deleteSelection()); };
    let ev = "canvas:menu", arg = c, fix = null;
    if (n) {
      if (!c.selection.has(n)) c.select(n, false);
      const sel = [...c.selection], d = n.data;
      if (sel.length > 1) {
        zoom();
        item("Create group", () => makeGroup(c, [...c.selection]));
        sep();
        item("Cut", () => { if (h && h.copy(c)) c.deleteSelection(); }, { off: !h });
        item("Copy", () => h && h.copy(c), { off: !h });
        item("Paste", () => h && h.paste(c), { off: !h });
        remove("Delete");
        ev = "canvas:selection-menu";
      } else {
        ev = "canvas:node-menu"; arg = n;
        if (d.type === "text") {
          zoom();
          item("Edit", () => n.startEditing());
          item("Convert to file", () => {
            const first = String(d.text || "").split("\n").map(s => s.replace(/^#+\s*/, "").trim()).find(Boolean) || "Untitled";
            prompt(c, "convert", "Convert to file", first.slice(0, 60), "The card's text becomes a note of this name, in the canvas's folder.", v => convertToFile(c, n, v));
          });
          remove();
        } else if (d.type === "file") {
          const k = kindOf(d.file), note = k === "note";
          zoom();
          item("Swap file", () => picker(c, "swap", "Swap for another " + (k === "file" ? "file" : k) + "…",
            files(c, f => f.path !== d.file && kindOf(f.path) === k), f => f.path, f => swapFile(c, n, f.path)));
          if (note) {
            item("Edit", () => editNote(c, n), { off: !n.isEditable(), why: "the note is not loaded" });
            item("Narrow to heading", () => sections(c, n).then(list => picker(c, "narrow", "Show only a heading…", list.filter(s => !s.sub.startsWith("#^")), s => s.label, s => narrow(c, n, s.sub))));
            item("Narrow to block", () => sections(c, n).then(list => picker(c, "block", "Show only a block…", list.filter(s => s.sub.startsWith("#^")), s => s.label, s => narrow(c, n, s.sub))));
          }
          sep();
          if (note) item("Open in new tab", () => app.workspace.openLinkText(d.file, c.path || "", "tab"));
          item("Open in new window", null, { off: true, why: "opensidian has one window: no pop-out windows" });
          sep();
          item("Rename", () => renameFile(c, n));
          item("Move file to", () => moveFile(c, n));
          if (note) item(bookmarked(d.file) ? "Remove bookmark" : "Bookmark", () => { const b = G("toggleBm"); if (b) return b(noteName(d.file)); });
          else item("Bookmark", null, { off: true, why: "bookmarks hold note names; this card shows a " + k + " file" });
          if (note) item("Merge entire file with", null, { off: true, why: "no note merge in opensidian" });
          sep();
          item("Open in default app", () => app.openWithDefaultApp(d.file));
          item("Show in system explorer", null, { off: true, why: "no file-manager handler reveals a path" });
          item("Reveal file in navigation", () => reveal(c, d.file));
          remove();
          sep();
          item("Copy path", () => copyText(d.file));
        } else if (d.type === "link") {
          zoom();
          item("Open external link", () => openExt(d.url));
          item("Reload page", () => { n.renderContent(); later(c); });
          remove();
          // canvas-files adds Copy URL / Change URL through the hook: put them in their stock place
          fix = () => {
            const at = l => m.items.findIndex(i => titleOf(i) === l);
            const take = l => { const i = m.items.findIndex((x, j) => j >= base && titleOf(x) === l); return i >= 0 ? m.items.splice(i, 1)[0] : null; };
            const chg = take("Change URL"), cpy = take("Copy URL");
            if (chg) m.items.splice(at("Zoom to selection") + 1, 0, chg);
            if (cpy) m.items.splice(at("Open external link") + 1, 0, cpy);
          };
        } else if (d.type === "group") {
          const onLabel = !!(t && t.closest && t.closest(".canvas-node-label"));
          zoom();
          item("Edit label", () => editLabel(c, n));
          if (!onLabel) {                   // the body keeps cv-menus' background rows (cvmenus CV-075)
            sep();
            const bg = typeof d.background === "string" && d.background;
            item(bg ? "Replace background" : "Set background", () => picker(c, "background", "Background image…",
              files(c, f => IMG.has(String(f.extension).toLowerCase())), f => f.path, f => setBackground(c, n, f.path)));
            if (bg) {
              item("Remove background", () => setBackground(c, n, null));
              const cur = d.backgroundStyle || "cover";
              for (const [v, label] of STYLES) item("Background style: " + label, () => setBackground(c, n, undefined, v), { on: cur === v });
            }
          }
          remove();
        } else { zoom(); remove(); }
      }
    } else {
      const p = c.toCanvas(e.clientX, e.clientY), o = c.options || {}, st = h && typeof h.state === "function" ? h.state(c) : null;
      const why = lock ? "the board is read-only" : "";
      item("Add card", () => c.createTextNode({ pos: p, focus: true }), { off: lock, why });
      item("Add note from vault", () => pickNote(c, path => addFile(c, path, p)), { off: lock, why });
      item("Add media from vault", () => pickMedia(c, path => addFile(c, path, p)), { off: lock, why });
      item("Add web page", () => enterUrl(c, u => addLink(c, u, p)), { off: lock, why });
      item("Create group", () => { c.deselectAll(); groupAt(c, p); }, { off: lock, why });
      if (st && st.u > 0) { sep(); item("Undo", () => h.undo(c), { off: lock, why }); }
      sep();
      item("Paste", () => h && h.paste(c), { off: lock || !h, why });
      sep();
      item("Snap to grid", () => setOpt(c, "snapToGrid"), { on: o.snapToGrid !== false });
      item("Snap to objects", () => setOpt(c, "snapToObjects"), { on: o.snapToObjects !== false });
      item("Read-only", () => setOpt(c, "readOnly"), { on: !!o.readOnly });
    }
    const base = m.items.length;
    try { app.workspace.trigger(ev, m, arg); } catch (_) {}
    if (fix) try { fix(); } catch (er) { console.error("[canvas] menu:", er); }
    for (const it of m.items) if (titleOf(it) !== null) iconize(it);
    m.showAtMouseEvent(e);
    if (m.dom) m.dom.classList.add("canvas-card-menu-ctx");
    shown = m;
    // Escape closes the menu and nothing else: the board's own Escape (deselect) must not see it
    const esc = ev => {
      if (ev.key !== "Escape" || !m.dom || !m.dom.isConnected) return;
      ev.preventDefault(); ev.stopImmediatePropagation();
      m.hide(); later(c);
    };
    window.addEventListener("keydown", esc, true);
    m.onHide(() => { window.removeEventListener("keydown", esc, true); later(c); });
    later(c);
  }

  /* ---------------------------------------------- the card toolbar */
  const TOOLS = [["text", "Text", "Drag to add a card"], ["note", "Note", "Drag to add a note from the vault"], ["media", "Media", "Drag to add media from the vault"]];
  function toolbar(c) {
    const bar = el("div", "canvas-card-menu", c.wrapperEl);
    C(c).bar = bar;
    for (const [kind, label, title] of TOOLS) {
      const b = el("button", "canvas-card-menu-item mod-" + kind, bar);
      b.type = "button"; b.textContent = label; b.title = title; b.setAttribute("aria-label", title); b.dataset.tool = kind;
      let down = null;
      b.addEventListener("pointerdown", e => {
        e.stopPropagation(); if (e.button !== 0 || ro(c)) return; e.preventDefault();
        down = { x: e.clientX, y: e.clientY, moved: false };
        try { b.setPointerCapture(e.pointerId); } catch (_) {}
      });
      b.addEventListener("pointermove", e => {
        if (!down) return;
        if (!down.moved && Math.abs(e.clientX - down.x) + Math.abs(e.clientY - down.y) > 5) { down.moved = true; c.wrapperEl.classList.add("is-card-dragging"); }
      });
      const end = (e, cancel) => {
        if (!down) return;
        const d = down; down = null;
        try { b.releasePointerCapture(e.pointerId); } catch (_) {}
        c.wrapperEl.classList.remove("is-card-dragging");
        if (cancel || ro(c)) return;
        const r = c.wrapperEl.getBoundingClientRect(), br = bar.getBoundingClientRect();
        let pos, center = true;
        if (d.moved) {
          center = !(window.CanvasDrop && window.CanvasDrop.TOPLEFT);   // goal cvf-drop (coordinator grant): stock top-left at the drop point behind CV_DROP_TOPLEFT
          const inBoard = e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
          const onBar = e.clientX >= br.left && e.clientX <= br.right && e.clientY >= br.top && e.clientY <= br.bottom;
          if (!inBoard || onBar) return;
          pos = c.toCanvas(e.clientX, e.clientY);
        } else pos = c.toCanvas(r.left + r.width / 2, r.top + r.height / 2);
        use(c, kind, pos, center);
      };
      b.addEventListener("pointerup", e => end(e, false));
      b.addEventListener("pointercancel", e => end(e, true));
      b.addEventListener("click", e => e.stopPropagation());
      b.addEventListener("dblclick", e => e.stopPropagation());
    }
  }
  function use(c, kind, pos, center) {
    if (kind === "text") { c.createTextNode({ pos: center ? { x: pos.x - 125, y: pos.y - 30 } : pos, focus: true }); later(c); return; }
    if (kind === "note") pickNote(c, path => addFile(c, path, pos, center));
    else pickMedia(c, path => addFile(c, path, pos, center));
  }

  /* ---------------------------------------------- drops from the file explorer */
  function dragged(c) {
    const { app, X } = C(c).ctx, dm = app && app.dragManager, d = dm && dm.draggable;
    if (!d) return [];
    const fs = d.type === "file" ? [d.file] : d.type === "files" ? d.files || [] : [];
    return fs.filter(f => f && (!X.TFile || f instanceof X.TFile) && typeof f.path === "string");
  }
  function onDragOver(c, e) {
    if (ro(c) || !dragged(c).length) return;
    e.preventDefault();
    try { e.dataTransfer.dropEffect = "copy"; } catch (_) {}
  }
  function onDrop(c, e) {
    // goal cvf-drop (coordinator grant 2026-10-26): canvas-drop.js lays out every explorer drop (file, selection, folder)
    if (window.CanvasDrop && typeof window.CanvasDrop.drop === "function") { window.CanvasDrop.drop(c, e); return; }
    const fs = dragged(c);
    if (ro(c) || !fs.length) return;
    e.preventDefault(); e.stopPropagation();
    const p = c.toCanvas(e.clientX, e.clientY);
    fs.forEach((f, i) => addFile(c, f.path, { x: p.x + i * 30, y: p.y + i * 30 }, true));
  }

  function install(c, ctx) {
    if (!c || c._cvm || !ctx || !ctx.X || !ctx.NODE_CLASS) return;
    c._cvm = { ctx, bar: null };
    style();
    const w = c.wrapperEl;
    w.addEventListener("contextmenu", e => onContext(c, e));
    // issue #49: a press on the board closes the open context menu. main.js closes menus on
    // mousedown, but the board's pointerdown calls preventDefault(), which suppresses that mousedown
    w.addEventListener("pointerdown", e => {
      if (typeof menuEl !== "undefined" && menuEl && !menuEl.contains(e.target) && typeof closeMenu === "function") closeMenu();
    }, true);
    w.addEventListener("dragover", e => onDragOver(c, e));
    w.addEventListener("drop", e => onDrop(c, e));
    toolbar(c);
  }

  /* ---------------------------------------------- census (main.js canvasTok)
     [cvtool:text@x,y|note@x,y|media@x,y]  the card toolbar's buttons (centres, client px)
     [cvmpick:<kind>|n=<rows>|<first row>]  a picker is open (kind note|media|swap|narrow|block|move|background)
     [cvmprompt:<kind>|<typed value>]       a prompt is open (kind convert|url)
     [cvmurl:<title>|<value>|Save@x,y;Cancel@x,y]  goal cvf-menus1: the Enter URL modal and its buttons (centres)
     [cvmm:n=<rows>|ico=<rows with an icon>|chk=<checked labels, ;-joined>]  goal cvf-menus1: the open canvas context menu
     [cvmrow:<note>@x,y|...]                the explorer's visible note rows (drag sources, at most 12)
     [cvfrow:<file>@x,y|...]                goal cv-journeys: the explorer's visible image / PDF / audio / video rows (at most 12)
     [tsel:<n>|<path>|...]                  goal cv-journeys: the explorer's shift+click selection, in row order
     [cvmodel:files=<n>|gapp=<0|1>]         goal cv-model: how many files the pickers can list
                                            (app.vault.getFiles()) and whether a global window.app exists
                                            (only a trusted vault running a plugin may define it) */
  function census(v) {
    const esc = s => String(s).replace(/[[\]|@]/g, "_");
    const vis = e => !!e && e.isConnected && e.getClientRects().length > 0;
    const ctr = e => { const r = e.getBoundingClientRect(); return Math.round(r.x + r.width / 2) + "," + Math.round(r.y + r.height / 2); };
    let t = "";
    const s = v && v.canvas && C(v.canvas);
    if (s && vis(s.bar)) {
      const bs = [...s.bar.querySelectorAll(".canvas-card-menu-item")].filter(vis);
      if (bs.length) t += " [cvtool:" + bs.map(b => esc(b.dataset.tool) + "@" + ctr(b)).join("|") + "]";
    }
    if (open && open.m && open.m._open !== false && vis(open.input)) {
      if (open.type === "pick") {
        const rows = [...open.m.resultContainerEl.querySelectorAll(".suggestion-item")];
        t += " [cvmpick:" + esc(open.kind) + "|n=" + rows.length + "|" + esc(rows.length ? rows[0].textContent.slice(0, 60) : "") + "]";
      } else t += " [cvmprompt:" + esc(open.kind) + "|" + esc(String(open.input.value).slice(0, 80)) + "]";
      if (open.btns) t += " [cvmurl:" + esc(open.title || "") + "|" + esc(String(open.input.value).slice(0, 80)) + "|" +
        open.btns.filter(([, b]) => vis(b)).map(([k, b]) => k + "@" + ctr(b)).join(";") + "]";
    }
    if (shown && shown.dom && shown.dom.isConnected) {   // goal cvf-menus1: the open context menu's rows, icons, checks
      const rows = shown.items.filter(i => titleOf(i) !== null);
      t += " [cvmm:n=" + rows.length + "|ico=" + rows.filter(i => i.iconEl && i.iconEl.querySelector("svg")).length +
        "|chk=" + rows.filter(i => i.dom.classList.contains("mod-checked")).map(i => esc(titleOf(i)).replace(/;/g, "_")).join(";") + "]";
    }
    const rows = [...document.querySelectorAll("#tree .trow.note")].filter(vis).slice(0, 12);
    if (rows.length) t += " [cvmrow:" + rows.map(r => esc(r.dataset.note) + "@" + ctr(r)).join("|") + "]";
    const frs = [...document.querySelectorAll("#tree .trow.file")].filter(vis).slice(0, 12);   // goal cv-journeys
    if (frs.length) t += " [cvfrow:" + frs.map(r => esc(r.dataset.file) + "@" + ctr(r)).join("|") + "]";
    const sel = [...document.querySelectorAll("#tree .trow.is-selected")];   // goal cv-journeys: the shift+click selection
    if (sel.length) t += " [tsel:" + sel.length + "|" + sel.map(r => esc(r.dataset.selp || "")).join("|") + "]";
    if (s) {
      let n = -1;
      try { n = s.ctx.app.vault.getFiles().length; } catch (_) {}
      t += " [cvmodel:files=" + n + "|gapp=" + ("app" in window ? 1 : 0) + "]";
    }
    return t;
  }

  const api = { install, census, groupAround, groupAt, convertToFile, pickNote };
  Object.defineProperty(window, "CanvasMenus", { configurable: true, enumerable: false, writable: false, value: Object.freeze(api) });
})();
