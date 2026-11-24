// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* canvas selection (goal cv-select): selecting, moving and resizing cards on the
   canvas board (ui/plugins/canvas.js), written from the public help pages and the
   JSON Canvas 1.0 spec (clean room).

   Load order (index.html): canvas.js -> canvas-history.js -> canvas-cards.js ->
   canvas-select.js -> main.js. This file runs NO code at load beyond defining
   window.CanvasSelect; canvas.js calls in:
     install(canvas, changed)   once per board, from wire(): canvas.options, overlay
     down/move/up(canvas, ...)  the pointer branches of wire()
     onKey(canvas, e)           Ctrl/Cmd+A, arrow nudge (Ctrl/Cmd+X / V: a capture listener from install)
     selected(canvas)           after every select() / deselectAll()

   What a pointer press does:
     on a card's border (7px inside it) or its corner handle -> resize those sides;
       Shift keeps the aspect ratio
     on a card                -> move the selection (a plain press on an unselected card
       selects it alone and brings it to the front: last in nodes[], last painted);
       Shift on an unselected card adds it, Shift-click on a selected one takes it out;
       Alt duplicates the selection and moves the copies; Shift locks the move to one axis;
       a moved group carries every card that lies wholly inside it
     on the empty board       -> pan, as its hand cursor and the help say (a click without
       a drag clears the selection); Shift -> a selection rectangle (cards wholly inside
       it), added to the selection
     middle button, or Space held at the press -> pan
   A card dragged to within PANZ px of the board pane's edge (or past it) pans the view
   that way while it is held there, faster the closer it is, and the card keeps riding
   under the pointer (goal cvf-keys, CVR-A-151 / C-032).
   Moves and resizes snap to the grid (canvas.options.snapToGrid, step 20) and to the
   edges / centres of nearby cards (snapToObjects; wins over the grid); holding Space
   while dragging suspends snapping. canvas.options = {snapToGrid, snapToObjects, readOnly}
   is the contract shared with cv-nav (created only when absent).

   The selection toolbar floats above the selection's bounding box: remove, colour (a
   palette of the six preset colours + clear), zoom to selection, create group and align
   (a multi-selection: one group around it / a palette of six edge+centre aligns), edit
   (one editable card). */
(function () {
  "use strict";
  /* gh-icons #20: monochrome inline svg icons (drawn for this app, currentColor) in place of
     text pictographs / emoji; class "gi" sizes them (style.css / plugins.css) */
  const I = d => '<svg class="gi" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">' + d + "</svg>";
  const GI = {
    trash: I('<path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.7 8.5h5.6l.7-8.5"/>'),
    color: I('<circle cx="8" cy="8" r="5.5"/><path d="M8 2.5a5.5 5.5 0 0 1 0 11z" fill="currentColor"/>'),
    zoom: I('<path d="M9.5 2.5h4v4M6.5 13.5h-4v-4M13.5 2.5L9 7M2.5 13.5L7 9"/>'),
    edit: I('<path d="M10.6 2.7a1.6 1.6 0 0 1 2.3 2.3l-7.6 7.6-3 .9.9-3z"/>'),
    x: I('<path d="M4 4l8 8M12 4l-8 8"/>'),
    group: I('<rect x="2.5" y="2.5" width="11" height="11" rx="1.5"/><rect x="5.5" y="5.5" width="5" height="5" rx=".8"/>'),
    align: I('<path d="M2.5 2v12M5 4.5h8M5 8h5M5 11.5h7"/>'),
  };
  const GRID = 20;            // grid step, canvas units
  const EDGE = 7;             // border hit zone, client px (inside the card)
  const SNAP = 8;             // snap distance, client px
  const SNAPR = 10;           // resize: an edge within this many client px (inclusive) of another card's edge lands on it (CVR-B-106)
  const MINW = 50, MINH = 50; // a resize never makes a card smaller than this (CVR-B-109)
  const PANZ = 40;            // auto-pan zone at the pane edge, client px
  const PANV = 800;           // auto-pan speed at (or past) the edge, client px per second
  const COLORS = [["1", "Red", "var(--color-red, #e93147)"], ["2", "Orange", "var(--color-orange, #ec7500)"],
                  ["3", "Yellow", "var(--color-yellow, #e0ac00)"], ["4", "Green", "var(--color-green, #08b94e)"],
                  ["5", "Cyan", "var(--color-cyan, #00bfbc)"], ["6", "Purple", "var(--color-purple, #7852ee)"]];
  const ALIGN = [["left", "Align left edges", "⇤"], ["center", "Align horizontal centres", "↔"], ["right", "Align right edges", "⇥"],
                 ["top", "Align top edges", "⤒"], ["middle", "Align vertical centres", "↕"], ["bottom", "Align bottom edges", "⤓"]];
  let space = false, styled = false;

  const S = c => c._cvs;
  const el = (tag, cls, parent) => { const e = document.createElement(tag); if (cls) e.className = cls; if (parent) parent.appendChild(e); return e; };
  const notify = c => { const s = S(c); if (s && s.changed) try { s.changed(); } catch (_) {} };
  const isGroup = n => n && n.data && n.data.type === "group";
  const box = d => ({ x: d.x, y: d.y, w: d.width, h: d.height });
  const inside = (a, g) => a.x >= g.x && a.y >= g.y && a.x + a.w <= g.x + g.w && a.y + a.h <= g.y + g.h;
  const ro = c => !!(c.readonly || (c.options && c.options.readOnly));

  function style() {
    if (styled) return; styled = true;
    const s = document.createElement("style");
    s.dataset.owner = "canvas-select";
    s.textContent = [
      ".canvas-selection { position: absolute; box-sizing: border-box; pointer-events: none; border: 1px dashed var(--interactive-accent, #7f6df2); border-radius: 4px }",
      ".canvas-selection[hidden], .canvas-selection-toolbar[hidden], .canvas-selection-palette[hidden], .canvas-selection-marquee[hidden] { display: none }",
      ".canvas-wrapper.is-board-panning { cursor: grabbing }",
      ".canvas-selection-marquee { position: absolute; box-sizing: border-box; pointer-events: none; z-index: 3; border: 1px solid var(--interactive-accent, #7f6df2); background: color-mix(in srgb, var(--interactive-accent, #7f6df2) 15%, transparent) }",
      ".canvas-selection-toolbar { position: absolute; z-index: 3; display: flex; gap: 2px; padding: 3px; border-radius: 6px; background: var(--background-secondary, #262626); border: 1px solid var(--background-modifier-border, #444); box-shadow: 0 2px 8px rgba(0,0,0,.3) }",
      ".canvas-selection-toolbar > button { min-width: 28px; height: 26px; padding: 0 6px; cursor: pointer }",
      ".canvas-selection-palette { position: absolute; z-index: 3; display: flex; gap: 4px; padding: 4px; border-radius: 6px; background: var(--background-secondary, #262626); border: 1px solid var(--background-modifier-border, #444) }",
      ".canvas-selection-palette > button { width: 22px; height: 22px; padding: 0; border-radius: 50%; cursor: pointer; border: 1px solid var(--background-modifier-border, #555) }",
      // selected look (CVR-B-032): the card's own 2px border turns the accent colour; no extra ring, no painted
      // corner handle (the corner and every border band stay resize zones)
      ".canvas-node.canvas-node.is-selected > .canvas-node-container { border-color: var(--interactive-accent, #7f6df2); box-shadow: none }",
    ].join("\n");
    document.head.appendChild(s);
    window.addEventListener("keydown", e => { if (e.key === " " || e.code === "Space") space = true; }, true);
    window.addEventListener("keyup", e => { if (e.key === " " || e.code === "Space") space = false; }, true);
    window.addEventListener("blur", () => { space = false; });
  }

  /* ---------------------------------------------- install (from wire(), once per board) */
  function install(c, changed) {
    if (c._cvs) { if (typeof changed === "function") c._cvs.changed = changed; return c._cvs; }
    style();
    c.options = c.options || { snapToGrid: true, snapToObjects: true, readOnly: false };   // shared contract (cv-nav reads it)
    const s = c._cvs = { changed: typeof changed === "function" ? changed : null, raf: 0, palette: false };
    s.boxEl = el("div", "canvas-selection", c.canvasEl); s.boxEl.hidden = true;
    s.marqEl = el("div", "canvas-selection-marquee", c.wrapperEl); s.marqEl.hidden = true;
    s.tbEl = el("div", "canvas-selection-toolbar", c.wrapperEl); s.tbEl.hidden = true;
    s.palEl = el("div", "canvas-selection-palette", c.wrapperEl); s.palEl.hidden = true;
    const stop = e => e.stopPropagation();
    c.wrapperEl.addEventListener("keydown", e => clipKey(c, e), true);   // Ctrl+X / Ctrl+V, ahead of the history's keys
    for (const x of [s.tbEl, s.palEl]) { x.addEventListener("pointerdown", stop); x.addEventListener("dblclick", stop); x.addEventListener("wheel", stop); }
    const btn = (parent, name, label, txt, fn) => {
      const b = el("button", "canvas-selection-btn mod-" + name, parent);
      b.type = "button"; if (txt.startsWith("<svg")) b.innerHTML = txt; else b.textContent = txt; b.title = label; b.setAttribute("aria-label", label); b.dataset.action = name;
      b.addEventListener("click", e => { e.stopPropagation(); fn(); });
      return b;
    };
    s.btn = {
      remove: btn(s.tbEl, "remove", "Remove", GI.trash, () => { s.palette = false; if (!ro(c)) c.deleteSelection(); refresh(c); notify(c); }),
      color: btn(s.tbEl, "color", "Set color", GI.color, () => { s.palette = !s.palette; s.align = false; refresh(c); notify(c); }),
      zoom: btn(s.tbEl, "zoom", "Zoom to selection", GI.zoom, () => { zoomToSelection(c); }),
      group: btn(s.tbEl, "group", "Create group", GI.group, () => { s.palette = false; s.align = false; if (!ro(c) && c.selection.size > 1 && window.CanvasMenus) window.CanvasMenus.groupAround(c, [...c.selection]); refresh(c); notify(c); }),
      align: btn(s.tbEl, "align", "Align", GI.align, () => { s.align = !s.align; s.palette = false; refresh(c); notify(c); }),
      edit: btn(s.tbEl, "edit", "Edit", GI.edit, () => { const n = only(c); s.palette = false; if (n && n.isEditable() && !ro(c)) n.startEditing(); refresh(c); notify(c); }),
    };
    s.pal = {};
    for (const [k, name, css] of COLORS) { const b = btn(s.palEl, "color-" + k, name, "", () => setColor(c, k)); b.style.background = css; s.pal[k] = b; }
    s.pal.clear = btn(s.palEl, "color-clear", "No color", GI.x, () => setColor(c, ""));
    s.alEl = el("div", "canvas-selection-palette canvas-selection-align", c.wrapperEl); s.alEl.hidden = true;
    for (const x of ["pointerdown", "dblclick", "wheel"]) s.alEl.addEventListener(x, stop);
    s.al = {};
    for (const [k, name, txt] of ALIGN) s.al[k] = btn(s.alEl, "align-" + k, name, txt, () => align(c, k));
    // the overlay follows the view (pan / zoom) and the document (remove, undo rebuild)
    const at = c.applyTransform, rs = c.requestSave;
    c.applyTransform = function () { const r = at.apply(this, arguments); refresh(c); return r; };
    c.requestSave = function () { const r = rs.apply(this, arguments); refresh(c); return r; };
    return s;
  }
  const only = c => c.selection.size === 1 ? [...c.selection][0] : null;

  function setColor(c, k) {
    const s = S(c); s.palette = false;
    if (!ro(c)) for (const n of c.selection) if ((n.data.color || "") !== k) n.setColor(k);
    refresh(c); notify(c);
  }
  // align: every selected card to one edge / centre of the selection box. One write, one undo step; the
  // moved cards go last in nodes[] (raise), like a drag. Fewer than two cards: nothing to align.
  function align(c, k) {
    const s = S(c); s.align = false;
    const sel = [...c.selection], b = bboxOf(sel);
    if (ro(c) || sel.length < 2 || !b) { refresh(c); notify(c); return; }
    const moved = [];
    for (const n of sel) {
      const d = n.data; let x = d.x, y = d.y;
      if (k === "left") x = b.x; else if (k === "right") x = b.x + b.w - d.width; else if (k === "center") x = Math.round(b.x + b.w / 2 - d.width / 2);
      else if (k === "top") y = b.y; else if (k === "bottom") y = b.y + b.h - d.height; else if (k === "middle") y = Math.round(b.y + b.h / 2 - d.height / 2);
      if (x !== d.x || y !== d.y) { n.moveTo({ x, y }); moved.push(n); }
    }
    if (moved.length) { c.renderEdges(); raise(c, moved); c.requestSave(); }
    refresh(c); notify(c);
  }
  function bboxOf(nodes) {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of nodes) { const d = n.data; x0 = Math.min(x0, d.x); y0 = Math.min(y0, d.y); x1 = Math.max(x1, d.x + d.width); y1 = Math.max(y1, d.y + d.height); }
    return x0 === Infinity ? null : { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
  }
  function zoomToSelection(c) {
    const b = bboxOf(c.selection), r = c.wrapperEl.getBoundingClientRect();
    if (!b || !r.width || !r.height) return;
    const pad = 60;
    c.zoom = Math.max(0.1, Math.min(2, (r.width - 2 * pad) / Math.max(1, b.w), (r.height - 2 * pad) / Math.max(1, b.h)));
    c.tx = r.width / 2 - (b.x + b.w / 2) * c.zoom; c.ty = r.height / 2 - (b.y + b.h / 2) * c.zoom;
    c.applyTransform(); notify(c);
  }

  /* ---------------------------------------------- the overlay: bounding box + toolbar + palette */
  function refresh(c) {
    const s = S(c); if (!s) return;
    for (const n of [...c.selection]) if (c.nodes.get(n.id) !== n) c.selection.delete(n);   // gone (removed / rebuilt)
    const b = bboxOf(c.selection);
    if (!b) { s.boxEl.hidden = true; s.tbEl.hidden = true; s.palEl.hidden = true; s.alEl.hidden = true; s.palette = false; s.align = false; return; }
    const P = 6;
    Object.assign(s.boxEl.style, { left: (b.x - P) + "px", top: (b.y - P) + "px", width: (b.w + 2 * P) + "px", height: (b.h + 2 * P) + "px" });
    s.boxEl.hidden = false;
    const one = only(c);
    s.btn.edit.hidden = !(one && one.isEditable());
    const many = c.selection.size > 1;                       // group / align: a multi-selection only
    s.btn.group.hidden = !many; s.btn.align.hidden = !many; if (!many) s.align = false;
    s.tbEl.hidden = !!ro(c);
    if (s.tbEl.hidden) { s.palEl.hidden = true; s.alEl.hidden = true; return; }
    // client -> wrapper-local: the toolbar sits centred above the box, inside the board
    const z = c.zoom, lx = c.tx + (b.x - P) * z, ly = c.ty + (b.y - P) * z, lw = (b.w + 2 * P) * z;
    const W = c.wrapperEl.clientWidth || 0, tw = s.tbEl.offsetWidth || 140, th = s.tbEl.offsetHeight || 32;
    const left = Math.max(4, Math.min(Math.max(4, W - tw - 4), lx + lw / 2 - tw / 2));
    // no room above the box: below it instead, so the toolbar never covers the selected card's top border (CVR-B-108)
    const lh = (b.h + 2 * P) * z, H = c.wrapperEl.clientHeight || 0;
    const top = ly - th - 8 >= 4 || ly + lh + 8 + th > H - 4 ? Math.max(4, ly - th - 8) : ly + lh + 8;
    s.tbEl.style.left = left + "px"; s.tbEl.style.top = top + "px";
    s.palEl.hidden = !s.palette;
    if (s.palette) { s.palEl.style.left = left + "px"; s.palEl.style.top = (top - (s.palEl.offsetHeight || 32) - 4 < 4 ? top + th + 4 : top - (s.palEl.offsetHeight || 32) - 4) + "px"; }
    s.alEl.hidden = !s.align;
    if (s.align) { const ah = s.alEl.offsetHeight || 32; s.alEl.style.left = left + "px"; s.alEl.style.top = (top - ah - 4 < 4 ? top + th + 4 : top - ah - 4) + "px"; }
  }

  /* ---------------------------------------------- selection */
  function selected(c) { const s = S(c); if (!s) return; s.palette = false; s.align = false; refresh(c); notify(c); }
  // z-raise policy (goal cvf-keys, CVR-A-111/A-148/A-038). A click only PAINTS the card on top (last in
  // the card layer): no write, no undo step, nodes[] untouched. The file's paint order (nodes[]) follows
  // only a real mutation: raise() puts the touched cards last in nodes[] right before that edit's save,
  // so the edit and the reorder are one write and one undo step. Groups keep their own layer and their
  // place in nodes[].
  function toFront(c, n) {
    if (!n || isGroup(n) || ro(c)) return;
    if (c.nodeEl.lastElementChild !== n.containerEl) c.nodeEl.appendChild(n.containerEl);
  }
  function raise(c, nodes) {
    const ns = [...nodes].filter(n => n && !isGroup(n) && c.nodes.get(n.id) === n);
    if (!ns.length) return;
    const pos = new Map([...c.nodeEl.children].map((x, i) => [x, i]));
    ns.sort((a, b) => (pos.has(a.containerEl) ? pos.get(a.containerEl) : -1) - (pos.has(b.containerEl) ? pos.get(b.containerEl) : -1));
    for (const n of ns) { c.nodes.delete(n.id); c.nodes.set(n.id, n); }
  }
  function selectAll(c) {
    c.deselectAll();
    for (const n of c.nodes.values()) c.select(n, true);
  }

  /* ---------------------------------------------- pointer */
  function sidesAt(n, e) {
    if (e.target && e.target.classList && e.target.classList.contains("canvas-node-resizer")) return { r: 1, b: 1 };
    if (e.target && e.target.closest && e.target.closest(".canvas-node-label")) return null;
    const r = n.containerEl.getBoundingClientRect();
    const px = e.clientX - r.left, py = e.clientY - r.top;
    if (px < 0 || py < 0 || px > r.width || py > r.height) return null;
    const ez = Math.min(EDGE, r.width / 4, r.height / 4);
    const s = { l: px < ez ? 1 : 0, r: px > r.width - ez ? 1 : 0, t: py < ez ? 1 : 0, b: py > r.height - ez ? 1 : 0 };
    return s.l || s.r || s.t || s.b ? s : null;
  }
  function freshId(taken) {
    for (;;) {
      let s = "";
      for (let i = 0; i < 16; i++) s += "0123456789abcdef"[Math.floor(Math.random() * 16)];
      if (!taken.has(s)) { taken.add(s); return s; }
    }
  }
  // Alt-drag: copies of the selection (new ids, the edges between them re-pointed) become the selection
  function duplicateForDrag(c) {
    const src = [...c.selection]; if (!src.length) return;
    const taken = c.ids(), map = new Map(), made = [];
    for (const n of src) {
      const d = JSON.parse(JSON.stringify(n.data));
      d.id = freshId(taken); map.set(n.id, d.id);
      const Cls = Object.getPrototypeOf(n).constructor;
      made.push(c.addNode(new Cls(c, d)));
    }
    for (const e of [...c.edges.values()]) {
      if (!map.has(e.fromNode) || !map.has(e.toNode)) continue;
      const d = JSON.parse(JSON.stringify(e));
      d.id = freshId(taken); d.fromNode = map.get(e.fromNode); d.toNode = map.get(e.toNode);
      c.edges.set(d.id, d);
    }
    c.deselectAll();
    for (const n of made) c.select(n, true);
    c.renderEdges();
  }

  // returns the drag state canvas.js keeps for move/up, or null (nothing to track)
  function down(c, e, n) {
    const s = S(c) || install(c);
    s.palette = false;
    const base = { sel: true, x: e.clientX, y: e.clientY, moved: false, tx: c.tx, ty: c.ty, node: n };
    if (e.button === 1 || (e.button === 0 && space)) { refresh(c); return Object.assign(base, { mode: "pan" }); }
    if (!n) {
      if (e.button === 0 && !e.shiftKey) {   // the board's hand: a drag pans; up() clears the selection on a click
        c.wrapperEl.classList.add("is-board-panning");
        return Object.assign(base, { mode: "pan", board: true });
      }
      refresh(c);
      return Object.assign(base, { mode: "marquee", keep: new Set(c.selection), rect: c.wrapperEl.getBoundingClientRect() });
    }
    if (ro(c)) return null;
    const sides = sidesAt(n, e);
    let toggleOff = false, collapse = false;
    if (!c.selection.has(n)) { c.select(n, e.shiftKey); if (!e.shiftKey) toFront(c, n); }
    else if (e.shiftKey && !sides) toggleOff = true;          // decided on release: a shift-DRAG keeps it
    else if (!e.shiftKey && !sides) { toFront(c, n); collapse = c.selection.size > 1; }   // a plain CLICK (no drag) selects it alone
    if (sides) {
      if (!c.selection.has(n)) c.select(n, false);
      return Object.assign(base, { mode: "resize", sides, start: box(n.data), ratio: n.data.width / Math.max(1, n.data.height) });
    }
    return Object.assign(base, { mode: "move", toggleOff, collapse, alt: !!e.altKey, start: null });
  }
  function beginMove(c, d) {
    if (d.alt && !ro(c)) duplicateForDrag(c);
    const moving = new Set(c.selection);
    for (const g of [...moving]) if (isGroup(g)) {           // a group carries every card wholly inside it
      const gb = box(g.data);
      for (const m of c.nodes.values()) if (!moving.has(m) && inside(box(m.data), gb)) moving.add(m);
    }
    d.start = [...moving].map(m => ({ m, x: m.data.x, y: m.data.y }));
    const prim = d.node && moving.has(d.node) ? d.node : [...c.selection][0] || (d.alt ? [...c.selection][0] : null);
    d.prim = prim;
    d.pb = prim ? box(prim.data) : null;
    d.others = [...c.nodes.values()].filter(m => !moving.has(m)).map(m => box(m.data));
  }
  // snap one axis: lo = the moving box's low edge, len its length, lines = the other boxes' edges on that axis
  function snapAxis(c, lo, len, lines) {
    const o = c.options || {};
    if (o.snapToObjects !== false && lines.length) {
      const T = SNAP / (c.zoom || 1);
      let best = null;
      for (const L of lines) for (const off of [0, len / 2, len]) { const dd = L - (lo + off); if (Math.abs(dd) <= T && (!best || Math.abs(dd) < Math.abs(best))) best = dd; }
      if (best !== null) return lo + best;
    }
    if (o.snapToGrid !== false) return gridOf(lo);
    return lo;
  }
  const xLines = bs => bs.flatMap(b => [b.x, b.x + b.w / 2, b.x + b.w]);
  const yLines = bs => bs.flatMap(b => [b.y, b.y + b.h / 2, b.y + b.h]);
  // the nearest grid line; a half step rounds away from zero (-30 -> -40, 310 -> 320), the same both ways
  const gridOf = v => Math.sign(v) * Math.round(Math.abs(v) / GRID) * GRID;
  function snapEdge(c, v, lines) {     // one edge (resize): to a nearby card edge, else to the grid
    const o = c.options || {};
    if (o.snapToObjects !== false) {
      const T = SNAPR / (c.zoom || 1) + 1e-6; let best = null;
      for (const L of lines) { const dd = L - v; if (Math.abs(dd) <= T && (best === null || Math.abs(dd) < Math.abs(best))) best = dd; }
      if (best !== null) return v + best;
    }
    return o.snapToGrid !== false ? gridOf(v) : v;
  }

  function move(c, d, e) {
    const dxp = e.clientX - d.x, dyp = e.clientY - d.y;
    if (!d.moved && Math.abs(dxp) + Math.abs(dyp) < 3) return;
    const first = !d.moved; d.moved = true;
    const s = S(c);
    if (d.mode === "pan") { c.tx = d.tx + dxp; c.ty = d.ty + dyp; c.applyTransform(); return; }
    if (d.mode === "marquee") {
      const r = d.rect, x0 = Math.min(d.x, e.clientX), y0 = Math.min(d.y, e.clientY), x1 = Math.max(d.x, e.clientX), y1 = Math.max(d.y, e.clientY);
      Object.assign(s.marqEl.style, { left: (x0 - r.left) + "px", top: (y0 - r.top) + "px", width: (x1 - x0) + "px", height: (y1 - y0) + "px" });
      s.marqEl.hidden = false;
      const a = c.toCanvas(x0, y0), b = c.toCanvas(x1, y1), q = { x: a.x, y: a.y, w: b.x - a.x, h: b.y - a.y };
      const want = new Set(d.keep || []);
      for (const n of c.nodes.values()) if (inside(box(n.data), q)) want.add(n);
      const same = want.size === c.selection.size && [...want].every(n => c.selection.has(n));
      if (!same) {
        for (const n of c.selection) if (!want.has(n)) n.setSelected(false);
        c.selection.clear();
        for (const n of c.nodes.values()) if (want.has(n)) { c.selection.add(n); n.setSelected(true); }
        selected(c);
      }
      return;
    }
    const z = c.zoom || 1, snapOn = !space;
    if (d.mode === "move") {
      if (first) beginMove(c, d);
      let dx = dxp / z, dy = dyp / z;
      let lockX = false, lockY = false;
      if (e.shiftKey) { if (Math.abs(dxp) >= Math.abs(dyp)) { dy = 0; lockY = true; } else { dx = 0; lockX = true; } }
      // an axis the pointer has not travelled along stays put: a purely horizontal drag never shifts y (CVR-B-115)
      if (dxp === 0) { dx = 0; lockX = true; } if (dyp === 0) { dy = 0; lockY = true; }
      if (snapOn && d.pb) {
        const p = d.pb;
        if (!lockX) dx = snapAxis(c, p.x + dx, p.w, xLines(d.others)) - p.x;
        if (!lockY) dy = snapAxis(c, p.y + dy, p.h, yLines(d.others)) - p.y;
      }
      dx = Math.round(dx); dy = Math.round(dy);
      for (const o of d.start) o.m.moveTo({ x: o.x + dx, y: o.y + dy });
      edgePan(c, d, e);
    } else if (d.mode === "resize") {
      const n = d.node, st = d.start, sd = d.sides;
      const dx = dxp / z, dy = dyp / z;
      let x0 = st.x, y0 = st.y, x1 = st.x + st.w, y1 = st.y + st.h;
      if (sd.l) x0 += dx; if (sd.r) x1 += dx; if (sd.t) y0 += dy; if (sd.b) y1 += dy;
      if (e.shiftKey) {                                       // keep the aspect ratio
        let w = x1 - x0, h = y1 - y0; const R = d.ratio;
        const horiz = sd.l || sd.r, vert = sd.t || sd.b;
        if (horiz && !vert) h = w / R;
        else if (vert && !horiz) w = h * R;
        else if (Math.abs(w / st.w - 1) >= Math.abs(h / st.h - 1)) h = w / R; else w = h * R;
        if (w < MINW) { w = MINW; h = w / R; } if (h < MINH) { h = MINH; w = h * R; }
        if (sd.l) x0 = x1 - w; else x1 = x0 + w;
        if (sd.t) y0 = y1 - h; else y1 = y0 + h;
      } else {
        if (snapOn) {
          const others = [...c.nodes.values()].filter(m => m !== n && !(isGroup(n) && inside(box(m.data), st))).map(m => box(m.data));
          const xl = others.flatMap(b => [b.x, b.x + b.w]), yl = others.flatMap(b => [b.y, b.y + b.h]);
          if (sd.l) x0 = snapEdge(c, x0, xl); if (sd.r) x1 = snapEdge(c, x1, xl);
          if (sd.t) y0 = snapEdge(c, y0, yl); if (sd.b) y1 = snapEdge(c, y1, yl);
        }
        if (x1 - x0 < MINW) { if (sd.l) x0 = x1 - MINW; else x1 = x0 + MINW; }
        if (y1 - y0 < MINH) { if (sd.t) y0 = y1 - MINH; else y1 = y0 + MINH; }
      }
      const X0 = Math.round(x0), Y0 = Math.round(y0);
      n.data.x = X0; n.data.y = Y0; n.data.width = Math.round(x1) - X0; n.data.height = Math.round(y1) - Y0;
      n.place();
    }
    c.renderEdges();
    refresh(c);
  }
  // auto-pan (CVR-A-151 / C-032): the pan velocity for a pointer at p, client px/s, or null outside the zone
  function panVel(c, p) {
    const w = c.wrapperEl; if (!w || !w.isConnected) return null;
    const r = w.getBoundingClientRect();
    const sp = dist => dist >= PANZ ? 0 : PANV * Math.min(1, (PANZ - dist) / PANZ);
    const vx = sp(p.clientX - r.left) - sp(r.right - p.clientX), vy = sp(p.clientY - r.top) - sp(r.bottom - p.clientY);
    return vx || vy ? { x: vx, y: vy } : null;
  }
  // while the held card sits in the zone the view pans (tx/ty) and the press anchor shifts with it, so move()
  // re-places the card under the pointer: the card rides along onto the newly shown board
  function edgePan(c, d, e) {
    d.last = { clientX: e.clientX, clientY: e.clientY, shiftKey: e.shiftKey, altKey: e.altKey };
    if (d.pan || d.done || ro(c) || !panVel(c, d.last)) return;
    d.pan = { t: performance.now() };
    const tick = () => {
      const v = !d.done && panVel(c, d.last);
      if (!v) { d.pan = null; return; }
      const now = performance.now(), dt = Math.min(0.1, (now - d.pan.t) / 1000); d.pan.t = now;
      const sx = v.x * dt, sy = v.y * dt;
      c.tx += sx; c.ty += sy; d.x += sx; d.y += sy;
      c.applyTransform();
      move(c, d, d.last);
      d.pan.timer = setTimeout(tick, 16);
    };
    d.pan.timer = setTimeout(tick, 16);
  }
  function up(c, d) {
    const s = S(c);
    d.done = true;
    if (d.pan) { clearTimeout(d.pan.timer); d.pan = null; }
    if (d.mode === "marquee") { s.marqEl.hidden = true; refresh(c); notify(c); return; }
    if (d.mode === "pan") {
      if (!d.board) return;
      c.wrapperEl.classList.remove("is-board-panning");
      if (!d.moved) { c.deselectAll(); refresh(c); notify(c); }
      return;
    }
    if (d.mode === "move" && !d.moved && d.toggleOff && d.node) {   // shift-click on a selected card: out of the selection
      c.selection.delete(d.node); d.node.setSelected(false); selected(c); return;
    }
    if (d.mode === "move" && !d.moved && d.collapse && d.node) { c.deselectAll(); c.select(d.node, false); return; }
    if (d.moved) {
      if (d.mode === "move" && d.start) raise(c, d.start.map(o => o.m));
      else if (d.mode === "resize") raise(c, [d.node]);
      c.requestSave();
    }
    refresh(c); notify(c);
  }

  /* ---------------------------------------------- keys */
  // cut / paste (goal cvf-keys, CVR-A-119/C-055/C-056), a capture listener on the board so it runs before the
  // history's own Ctrl+V. Ctrl+X = copy (the history clipboard) + remove the cards and their edges: one write,
  // one undo step. Ctrl+V = the history's paste (new ids, inner edges re-pointed), then the copies are moved,
  // in the same task, so their box is centred on the centre of the view wherever the pointer is: the step the
  // history commits (a microtask later) and the debounced write both see the final place.
  function clipKey(c, e) {
    if (e.defaultPrevented || inEditor(e.target) || !(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey || ro(c)) return;
    const H = window.CanvasHistory, k = String(e.key).toLowerCase();
    if (!H || (k !== "x" && k !== "v")) return;
    if (k === "x") {
      if (!c.selection.size || !H.copy(c)) return;
      c.deleteSelection();
    } else {
      const before = new Set(c.nodes.keys()), D = window.CanvasDrop;
      // the OS clipboard wins (cvf-drop, C-057/058): the history's Ctrl+V asks CanvasDrop.pasteKey, whose card paste
      // centres through centrePaste; only a copy the OS clipboard refused is pasted here
      if (D && typeof D.census === "function" && !/\[cvpaste:copy[_|]internal-only\]/.test(D.census())) return;
      if (!H.paste(c)) return;
      centrePaste(c, before);
    }
    e.preventDefault(); e.stopImmediatePropagation();
    refresh(c); notify(c);
  }
  function centrePaste(c, before) {
    const fresh = [...c.nodes.values()].filter(n => !before.has(n.id)), b = bboxOf(fresh), r = c.wrapperEl.getBoundingClientRect();
    if (b && r.width && r.height) {
      const m = c.toCanvas(r.left + r.width / 2, r.top + r.height / 2);
      const dx = Math.round(m.x - (b.x + b.w / 2)), dy = Math.round(m.y - (b.y + b.h / 2));
      if (dx || dy) { for (const n of fresh) n.moveTo({ x: n.data.x + dx, y: n.data.y + dy }); c.renderEdges(); c.requestSave(); }
    }
  }
  const inEditor = t => !!(t && t.closest && t.closest("textarea, input, .markdown-source-view, [contenteditable=true]"));
  function onKey(c, e) {
    if (inEditor(e.target)) return false;
    if ((e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && String(e.key).toLowerCase() === "a") {
      e.preventDefault(); e.stopPropagation(); selectAll(c); return true;
    }
    if (e.key === " " && !e.ctrlKey && !e.metaKey) { e.preventDefault(); return false; }   // Space: pan / no-snap modifier, never a scroll
    if (NUDGE[e.key] && !e.ctrlKey && !e.metaKey && !e.altKey) return nudge(c, e);
    return false;
  }
  // arrow nudge (goal cvf-keys, CVR-A-109/A-110): an arrow moves the selection one grid step, Shift+arrow
  // five; a moved group carries the cards wholly inside it (as a drag). Each press is one write and one
  // undo step; the touched cards go last in nodes[] in that same save. Nothing selected: nothing happens.
  const NUDGE = { ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1] };
  function nudge(c, e) {
    if (!c.selection.size || ro(c)) return false;
    e.preventDefault(); e.stopPropagation();
    const k = (e.shiftKey ? 5 : 1) * GRID, [ux, uy] = NUDGE[e.key];
    const moving = new Set(c.selection);
    for (const g of [...moving]) if (isGroup(g)) {
      const gb = box(g.data);
      for (const m of c.nodes.values()) if (!moving.has(m) && inside(box(m.data), gb)) moving.add(m);
    }
    for (const m of moving) m.moveTo({ x: m.data.x + ux * k, y: m.data.y + uy * k });
    c.renderEdges();
    raise(c, [...moving]);
    c.requestSave();
    refresh(c); notify(c);
    return true;
  }

  /* ---------------------------------------------- census (main.js canvasTok) */
  function census(v) {
    const c = v && v.canvas, s = c && S(c); if (!c) return "";
    const esc = x => String(x).replace(/[[\]|@,]/g, "_");
    const vis = x => !!x && !x.hidden && x.getClientRects().length > 0;
    const ctr = x => { const r = x.getBoundingClientRect(); return Math.round(r.x + r.width / 2) + "," + Math.round(r.y + r.height / 2); };
    const rc = x => { const r = x.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].map(Math.round).join(","); };
    let t = " [cvsel:n=" + c.selection.size + "]";
    t += " [cvz:" + [...c.nodeEl.children].map(x => x.dataset && x.dataset.nodeId).filter(Boolean).slice(0, 24).map(esc).join(",") + "]";
    const o = c.options || {};
    t += " [cvopt:grid=" + (o.snapToGrid !== false ? 1 : 0) + "|obj=" + (o.snapToObjects !== false ? 1 : 0) + "|step=" + GRID + "]";
    if (!s) return t;
    if (vis(s.boxEl)) t += " [cvbox:" + rc(s.boxEl) + "]";
    if (vis(s.tbEl)) {
      const bs = Object.keys(s.btn).filter(k => vis(s.btn[k]));
      t += " [cvstb:" + bs.join("|") + "@" + ctr(s.tbEl) + "]";
      t += " [cvstbb:" + bs.map(k => k + "@" + ctr(s.btn[k])).join("|") + "]";
    }
    if (vis(s.palEl)) t += " [cvpal:" + Object.keys(s.pal).filter(k => vis(s.pal[k])).map(k => k + "@" + ctr(s.pal[k])).join("|") + "]";
    if (vis(s.alEl)) t += " [cvalign:" + Object.keys(s.al).map(k => k + "@" + ctr(s.al[k])).join("|") + "]";
    t += look(c);
    return t;
  }
  // [cvklook:<id>|bw=<px>|hdl=<n>|acc=r,g,b] (goal cvf-keys, CVR-B-032): the first selected card's painted border
  // width, how many handle elements are visible on it (painted corner resizer + connection handles) and the
  // resolved accent colour, so a check can compare it with the screen pixels
  function look(c) {
    const n = [...c.selection][0]; if (!n || !n.containerEl || !n.containerEl.isConnected) return "";
    const box = n.containerEl.querySelector(":scope > .canvas-node-container") || n.containerEl;
    const cs = getComputedStyle(box);
    const rgb = v => (String(v).match(/[\d.]+/g) || []).slice(0, 3).map(x => Math.round(+x)).join(",");
    const s = S(c); if (!s || !s.boxEl) return "";
    const acc = rgb(getComputedStyle(s.boxEl).borderTopColor);   // the selection box's border is drawn in the accent colour
    const painted = x => { const st = getComputedStyle(x), a = String(st.backgroundColor).match(/[\d.]+/g) || [];
      return st.display !== "none" && st.visibility !== "hidden" && x.getClientRects().length > 0 && !(a.length > 3 && +a[3] === 0) && a.length >= 3; };
    let hdl = 0;
    for (const r of n.containerEl.querySelectorAll(".canvas-node-resizer")) if (painted(r)) hdl++;
    for (const h of c.wrapperEl.querySelectorAll(".canvas-edge-handle")) if (h.dataset.node === n.id && h.getClientRects().length) hdl++;
    const bw = Math.round(parseFloat(cs.borderTopWidth) || 0), solid = cs.borderTopStyle === "solid" && rgb(cs.borderTopColor) === acc;
    return " [cvklook:" + String(n.id).replace(/[[\]|@,]/g, "_") + "|bw=" + (solid ? bw : 0) + "|hdl=" + hdl + "|acc=" + acc + "]";
  }

  const api = { install, down, move, up, onKey, selected, selectAll, zoomToSelection, refresh, census, GRID, centrePaste };
  Object.defineProperty(window, "CanvasSelect", { configurable: true, enumerable: false, writable: false, value: Object.freeze(api) });
})();
