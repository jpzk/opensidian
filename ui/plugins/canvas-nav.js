// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* canvas navigation (goal cv-nav): moving around a big board, written from the
   public help pages (help.obsidian.md/plugins/canvas) — clean room.

     Shift + wheel        pans sideways (tx only)
     Space + wheel        zooms about the pointer
     Space + drag         pans, even when the drag starts on a card (the card stays put)
     Shift + 1            zoom to fit every card
     Shift + 2            zoom to the selection (a control does the same)
     Reset zoom           a control: zoom 1 about the board centre
     Jump to group        command "Canvas: Jump to group" -> a chooser of the board's groups
     Help                 a control: a modal listing the pan / zoom / select / move shortcuts
     Grid                 the board's dot grid (plugins.css) scales and moves with the viewport
     Settings             a control: snap to grid, snap to objects, read-only
                          (canvas.options {snapToGrid, snapToObjects, readOnly}, the contract cv-select shares);
                          read-only routes through canvas.readonly, which every edit path already checks.

   Load order (index.html): canvas.js -> ... -> canvas-nav.js -> main.js. This file runs no
   code at load beyond one stylesheet and one frozen global, window.CanvasNav: canvas.js
   calls attach(canvas, X, changed) from the Canvas constructor and onWheel(canvas, e) from
   its wheel listener; main.js canvasTok() appends census(view). */
(function () {
  "use strict";
  /* gh-icons #20: monochrome inline svg icons (drawn for this app, currentColor) in place of
     text pictographs / emoji; class "gi" sizes them (style.css / plugins.css) */
  const I = d => '<svg class="gi" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">' + d + "</svg>";
  const GI = {
    frame: I('<path d="M2.5 5.5v-3h3M10.5 2.5h3v3M13.5 10.5v3h-3M5.5 13.5h-3v-3"/><circle cx="8" cy="8" r="2"/>'),
    gear: I('<circle cx="8" cy="8" r="2"/><circle cx="8" cy="8" r="4.4"/><path d="M8 1.6v2M8 12.4v2M1.6 8h2M12.4 8h2M3.5 3.5l1.4 1.4M11.1 11.1l1.4 1.4M3.5 12.5l1.4-1.4M11.1 4.9l1.4-1.4"/>'),
    check: I('<path d="M3.5 8.5l3 3 6-7"/>'),
  };
  /* CVR-A-073 (operator go 2026-10-26, goal cv-stockflip): stock has no "+ Text" and no zoom-to-selection control;
     text cards come from the bottom bar / double-click, zoom to selection from Shift+2. true = the old stack */
  const DEFER_073 = false;
  const GRID = 20;                        // plugins.css .canvas-wrapper background-size at zoom 1
  /* goal cvf-zoom: zoom feel (CVR-C-067 / 068). One wheel notch (Ctrl, or Space held) zooms x WHEEL_STEP about the
     pointer, the + / - controls x CTL_STEP about the board centre; a trackpad's small pixel deltas zoom smoothly
     at the same rate per NOTCH_PX. Far out (dots under GRID_MIN px apart) the dot grid doubles its spacing until they are GRID_FAR
     px apart (CVR-C-077), so it never turns into a dense dot field. */
  const WHEEL_STEP = 1.32, CTL_STEP = Math.SQRT2, NOTCH_PX = 50, GRID_MIN = 5, GRID_FAR = 8;
  let hover = null;                       // the board under the pointer: Space + wheel works without focusing it
  const boards = new Set();
  let space = false;                      // Space is held (one keyboard: module-wide)
  let active = null;                      // the board the user last touched (commands act on it)
  let help = null, jump = null;           // the open help modal / group chooser
  let X = null, cmdsDone = false;

  const live = c => !!(c && c.view && c.wrapperEl.isConnected && c.wrapperEl.getClientRects().length);
  const inEditor = t => !!(t && t.closest && t.closest("textarea, input, .markdown-source-view, [contenteditable=true]"));
  const ping = c => { const n = c && c._cvnav && c._cvnav.changed; if (n) try { n(); } catch (_) {} };
  const board = () => (live(active) ? active : [...boards].find(live) || null);

  let css = false;
  function style() {
    if (css) return; css = true;
    const s = document.createElement("style");
    s.dataset.owner = "canvas-nav";
    s.textContent =
      ".canvas-wrapper.is-space-held { cursor: grab }\n" +
      ".canvas-wrapper.is-space-panning { cursor: grabbing }\n" +
      ".canvas-wrapper.is-readonly .canvas-control.mod-add-text { opacity: .4; pointer-events: none }\n" +
      // goal cvf-board: the controls are one icon-only column at the right edge (CVR-A-073)
      ".canvas-wrapper .canvas-controls.cvn-stack { flex-direction: column; align-items: center; top: 8px; right: 8px; gap: 2px; padding: 3px;\n" +
      "  border: 1px solid var(--background-modifier-border); border-radius: 6px; background: var(--background-primary) }\n" +
      ".canvas-controls.cvn-stack .canvas-control { width: 28px; height: 28px; padding: 0; display: flex; align-items: center; justify-content: center;\n" +
      "  border: 0; border-radius: 4px; background: transparent; color: var(--text-muted) }\n" +
      ".canvas-controls.cvn-stack .canvas-control:hover { background: var(--background-modifier-hover); color: var(--text-normal) }\n" +
      ".canvas-controls.cvn-stack .canvas-control svg { width: 16px; height: 16px; pointer-events: none }\n" +
      (DEFER_073 ? "" : ".canvas-controls.cvn-stack .canvas-control.mod-add-text, .canvas-controls.cvn-stack .canvas-control.mod-zoom-selection { display: none }\n") +
      // a disabled button swallows pointer events in WebKit: let them reach the column (hover tooltips hit-test by geometry)
      ".canvas-controls.cvn-stack .canvas-control:disabled { pointer-events: none; opacity: .45 }\n" +
      ".canvas-controls.cvn-stack .cvn-sep { width: 20px; height: 1px; margin: 2px 0; background: var(--background-modifier-border) }\n" +
      ".canvas-control-tip { position: fixed; z-index: 60; pointer-events: none; white-space: nowrap; padding: 4px 8px; border-radius: 4px;\n" +
      "  font-size: 13px; line-height: 1.3; color: var(--text-normal); background: var(--background-secondary); border: 1px solid var(--background-modifier-border);\n" +
      "  box-shadow: 0 2px 8px rgba(0, 0, 0, .25) }\n" +
      // read-only: a drag across cards never paints a text selection (CVR-C-099)
      ".canvas-wrapper.is-readonly, .canvas-wrapper.is-readonly * { -webkit-user-select: none; user-select: none }\n" +
      ".canvas-settings-menu { position: absolute; right: 52px; top: 8px; z-index: 3; display: flex; flex-direction: column; min-width: 200px;\n" +
      "  padding: 4px; border: 1px solid var(--background-modifier-border); border-radius: 6px; background: var(--background-secondary) }\n" +
      ".canvas-settings-item { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 4px 8px; cursor: pointer; border-radius: 4px }\n" +
      ".canvas-settings-item:hover { background: var(--background-modifier-hover) }\n" +
      ".canvas-settings-item .canvas-settings-check { min-width: 1.2em; text-align: center }\n" +
      ".canvas-help-row { display: flex; justify-content: space-between; gap: 24px; padding: 3px 0; border-bottom: 1px solid var(--background-modifier-border) }\n" +
      ".canvas-help-keys { font-family: var(--font-monospace); color: var(--text-muted); white-space: nowrap }\n" +
      // goal cvf-board: a file card's name keeps its screen size at any zoom; Show card names (CVR-A-015)
      ".canvas-wrapper .canvas-node-file > .canvas-node-label { font-size: calc(13px / var(--cvn-z, 1)); padding-bottom: calc(3px / var(--cvn-z, 1)) }\n" +
      ".canvas-wrapper.cvn-names-never .canvas-node-file > .canvas-node-label { display: none }\n" +
      ".canvas-wrapper.cvn-names-hover .canvas-node-file:not(:hover) > .canvas-node-label { visibility: hidden }\n" +
      // below the zoom threshold a card's content is not drawn (CVR-A-016)
      ".canvas-wrapper.cvn-hide-content .canvas-node:not(.canvas-node-group):not(.is-editing) > .canvas-node-container { visibility: hidden }\n" +
      // the Ctrl+drag rectangle (CVR-A-014)
      ".cvn-cdrag-box { position: fixed; z-index: 5; pointer-events: none; box-sizing: border-box; border: 1px dashed var(--interactive-accent, #7f6df2);\n" +
      "  background: color-mix(in srgb, var(--interactive-accent, #7f6df2) 10%, transparent) }\n";
    document.head.appendChild(s);
  }

  /* ------------------------------------------------ viewport */
  function paintGrid(c) {
    const w = c.wrapperEl; let s = GRID * c.zoom;
    // CVR-C-077: 20 x zoom down to 25 %; below that (dots under GRID_MIN px) every 2nd, 4th... dot, GRID_FAR..2 x GRID_FAR px apart
    if (s > 0 && s < GRID_MIN) while (s < GRID_FAR) s *= 2;
    w.style.backgroundSize = s + "px " + s + "px";
    w.style.backgroundPosition = c.tx + "px " + c.ty + "px";
  }
  // fit the canvas-coordinate box into the wrapper, centred; zoom at most maxZ
  function zoomToBox(c, x0, y0, x1, y1, maxZ) {
    const r = c.wrapperEl.getBoundingClientRect();
    if (!r.width || !r.height) return;
    const pad = 40, w = Math.max(1, x1 - x0), h = Math.max(1, y1 - y0);
    c.zoom = Math.max(0.05, Math.min(maxZ, (r.width - 2 * pad) / w, (r.height - 2 * pad) / h));
    c.tx = r.width / 2 - (x0 + x1) / 2 * c.zoom; c.ty = r.height / 2 - (y0 + y1) / 2 * c.zoom;
    c.applyTransform();
  }
  const boxOf = ns => {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of ns) { const d = n.data; x0 = Math.min(x0, d.x); y0 = Math.min(y0, d.y); x1 = Math.max(x1, d.x + d.width); y1 = Math.max(y1, d.y + d.height); }
    return x0 === Infinity ? null : [x0, y0, x1, y1];
  };
  function zoomToSelection(c) {
    const b = boxOf(c.selection.size ? c.selection : c.nodes.values());
    if (!b) return c.zoomToFit();
    zoomToBox(c, b[0], b[1], b[2], b[3], 1);
  }
  function resetZoom(c) {
    const r = c.wrapperEl.getBoundingClientRect(), px = r.width / 2, py = r.height / 2;
    c.tx = px - (px - c.tx) / c.zoom; c.ty = py - (py - c.ty) / c.zoom; c.zoom = 1;
    c.applyTransform();
  }
  function jumpTo(c, g) { const d = g.data; zoomToBox(c, d.x, d.y, d.x + d.width, d.y + d.height, 1); }

  /* ------------------------------------------------ input */
  // canvas.js wire() wheel listener -> here first; true = handled
  // one notch (a line / page delta, or a pixel delta of at least NOTCH_PX) = one WHEEL_STEP; smaller deltas scale smoothly
  function wheelFactor(e) {
    const d = e.deltaY || e.deltaX || 0;
    if (!d) return 1;
    if (e.deltaMode !== 0 || Math.abs(d) >= NOTCH_PX) return d < 0 ? WHEEL_STEP : 1 / WHEEL_STEP;
    return Math.pow(WHEEL_STEP, -d / NOTCH_PX);
  }
  function onWheel(c, e) {
    // Space + wheel, and Ctrl + wheel (the plain wheel when Settings > Canvas says Zoom), zoom about the pointer
    const g = cfg(), wz = !!(g && g.get("defaultWheelBehavior") === "zoom");
    if (space || (e.ctrlKey || e.metaKey) !== wz) {
      e.preventDefault();
      c.zoomAt(e.clientX, e.clientY, wheelFactor(e));
      return true;
    }
    if (e.ctrlKey || e.metaKey) return false;               // wheel = Zoom: canvas.js pans on Ctrl + wheel
    if (e.shiftKey) {                                        // WebKitGTK may already turn it into deltaX
      e.preventDefault();
      c.tx -= Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY;
      c.applyTransform();
      return true;
    }
    return false;
  }
  function onKeyDown(c, e) {
    if (inEditor(e.target)) return;
    if (e.key === " " || e.code === "Space") {
      e.preventDefault();
      if (!space) { space = true; c.wrapperEl.classList.add("is-space-held"); }
      return;
    }
    if (!e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
    if (e.code === "Digit1" || e.key === "!") { e.preventDefault(); e.stopPropagation(); c.zoomToFit(); }
    else if (e.code === "Digit2" || e.key === "@" || e.key === "\"") { e.preventDefault(); e.stopPropagation(); zoomToSelection(c); }
  }
  function spaceUp() {
    if (!space) return;
    space = false;
    for (const c of boards) c.wrapperEl.classList.remove("is-space-held");
  }

  /* ------------------------------------------------ settings + read-only */
  /* goal cvf-board CVR-A-079: snap to grid / snap to objects are app settings of the VAULT, like stock:
     a toggle writes <vault>/.obsidian/canvas.json at once (settings.js __osCanvasCfg) and every open board
     follows; a board opens with them. Read-only stays per board. */
  const CFGK = ["snapToGrid", "snapToObjects"];
  const cfg = () => window.__osCanvasCfg || null;
  function cfgSnap(c) {
    const g = cfg(); if (!g || !c.options) return;
    for (const k of CFGK) c.options[k] = !!g.get(k);
  }
  let cfgHooked = false;
  function hookCfg() {
    if (cfgHooked) return; const g = cfg(); if (!g) return; cfgHooked = true;
    g.on(keys => {
      if (keys.some(k => CFGK.includes(k))) for (const c of boards) { cfgSnap(c); ping(c); }
      if (keys.some(k => VIEWK.includes(k))) for (const c of boards) { viewCls(c); ping(c); }
    });
  }
  function setOption(c, k, v) {
    c.options[k] = !!v;
    // stock writes both snap keys on any toggle (CVR-A-079: "keys kept")
    if (CFGK.includes(k) && cfg()) cfg().set({ snapToGrid: c.options.snapToGrid !== false, snapToObjects: c.options.snapToObjects !== false });
    if (k === "readOnly") {
      syncRO(c);
      if (c.readonly) { for (const n of c.nodes.values()) if (n.isEditing) try { n.stopEditing(); } catch (_) {} }
    }
    ping(c);
  }
  // read-only shows on the settings control: a padlock instead of the gear (CVR-A-078)
  function syncRO(c) {
    const s = c._cvnav; if (!s) return;
    c.wrapperEl.classList.toggle("is-readonly", !!c.readonly);
    const b = s.btn.set, want = c.readonly ? "lock" : "gear";
    if (b && b.dataset.icon !== want) { b.dataset.icon = want; b.innerHTML = ICON[want]; }
  }
  const SETTINGS = [["g", "snapToGrid", "Snap to grid"], ["o", "snapToObjects", "Snap to objects"], ["ro", "readOnly", "Read-only"]];
  function closeSettings(c) {
    const s = c._cvnav; if (!s || !s.menu) return;
    s.menu.remove(); s.menu = null;
    document.removeEventListener("keydown", s.menuKey, true);
    document.removeEventListener("pointerdown", s.menuAway, true);
    ping(c);
  }
  function openSettings(c) {
    const s = c._cvnav;
    if (s.menu) return closeSettings(c);
    const m = document.createElement("div");
    m.className = "canvas-settings-menu"; m.setAttribute("role", "menu");
    for (const ev of ["pointerdown", "dblclick", "wheel"]) m.addEventListener(ev, e => e.stopPropagation());
    s.items = {}; s.more = null;
    for (const [tk, k, label] of SETTINGS) {
      const it = document.createElement("div");
      it.className = "canvas-settings-item"; it.setAttribute("role", "menuitemcheckbox");
      it.setAttribute("aria-checked", c.options[k] ? "true" : "false");
      const t = document.createElement("span"); t.textContent = label; it.appendChild(t);
      const ck = document.createElement("span"); ck.className = "canvas-settings-check"; ck.innerHTML = c.options[k] ? GI.check : ""; it.appendChild(ck);
      it.addEventListener("click", e => { e.stopPropagation(); setOption(c, k, !c.options[k]); closeSettings(c); });
      m.appendChild(it); s.items[tk] = it;
    }
    /* af-fix 2b: the Canvas settings pane (default location, wheel, Ctrl+drag, card names, snap, zoom threshold ->
       .obsidian/canvas.json) has no Settings nav entry any more; this item is its door. Not a toggle: [cvset:n=] counts
       the three toggles only, the item reports itself as [cvsetmore=x,y]. */
    if (window.__osPluginSettings && typeof __osPluginSettings.openCanvasSettings === "function") {
      const mo = document.createElement("div");
      mo.className = "canvas-settings-item canvas-settings-more"; mo.setAttribute("role", "menuitem");
      mo.style.cssText = "border-top:1px solid var(--background-modifier-border);margin-top:2px;padding-top:6px";
      const t = document.createElement("span"); t.textContent = "More canvas settings"; mo.appendChild(t);
      mo.addEventListener("click", e => { e.stopPropagation(); closeSettings(c); __osPluginSettings.openCanvasSettings(); });
      m.appendChild(mo); s.more = mo;
    }
    s.menuKey = e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); closeSettings(c); } };
    s.menuAway = e => { if (s.menu && !s.menu.contains(e.target) && !(s.btn.set && s.btn.set.contains(e.target))) closeSettings(c); };
    document.addEventListener("keydown", s.menuKey, true);
    document.addEventListener("pointerdown", s.menuAway, true);
    c.wrapperEl.appendChild(m); s.menu = m;
    ping(c);
  }

  /* ------------------------------------------------ help + jump */
  // goal cvf-board (CVR-A-082): the board shortcuts, written from the public help page
  const HELP = [
    ["Pan the board", "Space+drag or drag empty space"],
    ["Pan sideways", "Shift+wheel"],
    ["Zoom", "Ctrl+wheel or Space+wheel"],
    ["Zoom to fit", "Shift+1"],
    ["Zoom to selection", "Shift+2"],
    ["Select all", "Ctrl+A"],
    ["Add to the selection", "Shift+click"],
    ["Select with a rectangle", "Shift+drag empty space (mouse; a finger always pans)"],
    ["Create a card with specific size", "Ctrl+drag"],
    ["Clone card", "Ctrl+drag"],
    ["Constrain movement to axis", "Shift+drag"],
    ["Disable snapping", "Alt"],
    ["Edit a card", "Double-click"],
    ["Remove card", "Backspace or Delete"],
    ["Undo", "Ctrl+Z"],
    ["Redo", "Ctrl+Y or Ctrl+Shift+Z"],
  ];
  // the control icons (24x24 strokes, drawn here)
  const SVG = d => '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' + d + "</svg>";
  const ICON = {
    gear: SVG(SVG_ICON["lucide-settings"]),   // #130: the one cog source (ui/icons.js)
    lock: SVG('<rect x="5" y="11" width="14" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>'),
    in: SVG('<path d="M12 5v14M5 12h14"/>'),
    out: SVG('<path d="M5 12h14"/>'),
    reset: SVG('<path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/>'),
    fit: SVG('<path d="M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5"/>'),
    undo: SVG('<path d="M9 14L4 9l5-5"/><path d="M4 9h11a5 5 0 0 1 0 10h-3"/>'),
    redo: SVG('<path d="M15 14l5-5-5-5"/><path d="M20 9H9a5 5 0 0 0 0 10h3"/>'),
    help: SVG('<circle cx="12" cy="12" r="9"/><path d="M9.5 9a2.5 2.5 0 0 1 4.9.8c0 1.7-2.4 2.2-2.4 3.7M12 17h.01"/>'),
    add: SVG('<rect x="4" y="5" width="16" height="14" rx="2"/><path d="M9 9h6M12 9v6"/>'),
    sel: SVG('<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="3"/>'),
  };
  function openHelp(c) {
    if (help || !X || !X.Modal) return;
    const m = new X.Modal(c.app);
    m.setTitle("Canvas help");
    m.onOpen = function () {
      for (const [what, keys] of HELP) {
        const r = document.createElement("div"); r.className = "canvas-help-row";
        const a = document.createElement("span"); a.textContent = what; r.appendChild(a);
        const b = document.createElement("span"); b.className = "canvas-help-keys"; b.textContent = keys; r.appendChild(b);
        this.contentEl.appendChild(r);
      }
      help = this; ping(c);
    };
    const esc = e => { if (e.key === "Escape" && help === m) { e.preventDefault(); e.stopPropagation(); m.close(); } };
    m.onClose = function () { document.removeEventListener("keydown", esc, true); if (help === m) help = null; ping(c); };
    document.addEventListener("keydown", esc, true);
    m.open();
  }
  const groupsOf = c => [...c.nodes.values()].filter(n => n.data.type === "group");
  const groupName = g => String(g.data.label || "Untitled group");
  function openJump(c) {
    if (!X || !X.FuzzySuggestModal) return;
    const gs = groupsOf(c);
    const m = new X.FuzzySuggestModal(c.app);
    m.setPlaceholder("Jump to group…");
    m.emptyStateText = "No groups on this canvas.";
    m.getItems = () => gs;
    m.getItemText = g => groupName(g);
    m.onChooseItem = g => { if (live(c) && c.nodes.get(g.id) === g) { jumpTo(c, g); ping(c); } };
    const oo = m.onOpen.bind(m), oc = m.onClose.bind(m);
    m.onOpen = function () { jump = { m, n: gs.length }; oo(); ping(c); };
    m.onClose = function () { oc(); if (jump && jump.m === m) jump = null; ping(c); };
    m.open();
  }
  function registerCommands(app) {
    if (cmdsDone || !app || !app.commands || typeof app.commands.addCommand !== "function") return;
    cmdsDone = true;
    try {
      app.commands.addCommand({ id: "canvas:jump-to-group", name: "Canvas: Jump to group",
        checkCallback: chk => { const c = board(); if (!c) return false; if (!chk) openJump(c); return true; } });
      // CVR-A-007: stock's palette has no zoom / help rows (Shift+1, Shift+2 and the controls cover them)
    } catch (e) { cmdsDone = false; console.error("[canvas] nav commands:", e); }
  }

  /* ------------------------------------------------ attach (Canvas constructor) */
  function attach(c, x, changed, nc) {
    if (!c || c._cvnav) return;
    if (x) X = x;
    if (nc && !NC) NC = nc;
    style();
    const s = c._cvnav = { changed: typeof changed === "function" ? changed : null, btn: {}, menu: null, pan: null, raf: 0 };
    boards.add(c);
    // canvas.options: the contract shared with cv-select (conflicts.md O9); whichever lands first creates it
    c.options = c.options || { snapToGrid: true, snapToObjects: true, readOnly: false };
    hookCfg(); cfgSnap(c);
    // read-only = the file error (setError) OR the user's setting; every edit path already checks c.readonly
    let err = !!c.readonly;
    Object.defineProperty(c, "readonly", { configurable: true, enumerable: true,
      get() { return err || !!(this.options && this.options.readOnly); }, set(v) { err = !!v; } });
    // the grid follows every viewport change; the census republishes once per frame
    const orig = c.applyTransform;
    c.applyTransform = function () {
      const r = orig.apply(this, arguments);
      paintGrid(this); viewCls(this);
      if (!s.raf) s.raf = requestAnimationFrame(() => { s.raf = 0; ping(this); });
      return r;
    };
    paintGrid(c); viewCls(c);
    const w = c.wrapperEl;
    modDrag(c);
    w.addEventListener("keydown", e => onKeyDown(c, e));
    w.addEventListener("keyup", e => { if (e.key === " " || e.code === "Space") { if (!inEditor(e.target)) e.preventDefault(); spaceUp(); } });
    w.addEventListener("blur", spaceUp);
    w.addEventListener("pointerdown", () => { active = c; }, true);
    w.addEventListener("pointerenter", () => { hover = c; });
    w.addEventListener("pointerleave", () => { if (hover === c) hover = null; });
    w.addEventListener("focusin", () => { active = c; });
    // Space + drag: pan from anywhere, a card included; capture phase, so canvas.js's move/select never starts
    w.addEventListener("pointerdown", e => {
      if (!space || e.button !== 0 || inEditor(e.target)) return;
      if (e.target.closest && e.target.closest(".canvas-controls, .canvas-settings-menu")) return;
      e.preventDefault(); e.stopPropagation();
      w.focus({ preventScroll: true });
      s.pan = { x: e.clientX, y: e.clientY, tx: c.tx, ty: c.ty, id: e.pointerId };
      w.classList.add("is-space-panning");
      try { w.setPointerCapture(e.pointerId); } catch (_) {}
    }, true);
    w.addEventListener("pointermove", e => {
      if (!s.pan) return;
      e.stopPropagation();
      c.tx = s.pan.tx + e.clientX - s.pan.x; c.ty = s.pan.ty + e.clientY - s.pan.y; c.applyTransform();
    }, true);
    const end = e => {
      if (!s.pan) return;
      e.stopPropagation();
      try { w.releasePointerCapture(s.pan.id); } catch (_) {}
      s.pan = null; w.classList.remove("is-space-panning");
    };
    w.addEventListener("pointerup", end, true);
    w.addEventListener("pointercancel", end, true);
    // goal cvf-zoom CVR-C-067: the + / - controls step x CTL_STEP (canvas.js's own buttons; capture, so theirs never runs)
    if (c.controlsEl) c.controlsEl.addEventListener("click", e => {
      const b = e.target && e.target.closest ? e.target.closest(".mod-zoom-in, .mod-zoom-out") : null;
      if (!b || b.disabled) return;
      e.stopPropagation(); e.preventDefault();
      c.zoomBy(b.classList.contains("mod-zoom-in") ? CTL_STEP : 1 / CTL_STEP);
    }, true);
    // controls: appended after canvas.js's own (+ Text, zoom in/out, fit)
    if (c.controlsEl) {
      const btn = (cls, label, txt, fn) => {
        const b = document.createElement("button");
        b.className = "canvas-control " + cls; b.type = "button"; if (txt.startsWith("<svg")) b.innerHTML = txt; else b.textContent = txt; b.title = label; b.setAttribute("aria-label", label);
        b.addEventListener("pointerdown", e => e.stopPropagation());
        b.addEventListener("dblclick", e => e.stopPropagation());
        b.addEventListener("click", e => { e.stopPropagation(); fn(); });
        c.controlsEl.appendChild(b);
        return b;
      };
      s.btn.sel = btn("mod-zoom-selection", "Zoom to selection", GI.frame, () => zoomToSelection(c));
      s.btn.reset = btn("mod-zoom-reset", "Reset zoom", "1:1", () => resetZoom(c));
      s.btn.help = btn("mod-help", "Canvas help", "?", () => openHelp(c));
      s.btn.set = btn("mod-settings", "Canvas settings", GI.gear, () => openSettings(c));
      stack(c);
    }
    // read-only also follows the file error (canvas.readonly setter): keep the padlock in step
    const d = Object.getOwnPropertyDescriptor(c, "readonly");
    Object.defineProperty(c, "readonly", { configurable: true, enumerable: true,
      get: d.get, set(v) { d.set.call(this, v); syncRO(this); } });
    // read-only: a drag across cards selects no text (CVR-C-099)
    w.addEventListener("selectstart", e => { if (c.readonly && !inEditor(e.target)) e.preventDefault(); }, true);
    syncRO(c);
    registerCommands(c.app);
  }

  /* ------------------------------------------------ the controls column + tooltips (goal cvf-board)
     top to bottom: settings | zoom in, reset zoom, zoom to fit, zoom out | undo, redo | help.
     undo / redo are canvas-history.js's buttons (it may add them after attach): a MutationObserver
     keeps the order. The tooltip is our own element LEFT of the hovered button; the column
     hit-tests by geometry, so sliding from button to button (a disabled one included) re-targets it. */
  const ORDER = [["set", ".mod-settings"], "|", ["in", ".mod-zoom-in"], ["reset", ".mod-zoom-reset"], ["fit", ".mod-zoom-fit"],
                 ["out", ".mod-zoom-out"], "|", ["undo", ".mod-undo"], ["redo", ".mod-redo"], "|", ["help", ".mod-help"]].concat(DEFER_073 ? ["|", ["add", ".mod-add-text"], ["sel", ".mod-zoom-selection"]] : []);
  const TIP = { set: "Canvas settings", in: "Zoom in", reset: "Reset zoom", fit: "Zoom to fit (Shift + 1)", out: "Zoom out",
                undo: "Undo", redo: "Redo", help: "Canvas help", add: "Add text node", sel: "Zoom to selection" };
  let tip = null;                          // {el, b: the button, c}
  function hideTip() { if (!tip) return; const c = tip.c; tip.el.remove(); tip = null; ping(c); }
  function showTip(c, b) {
    if (tip && tip.b === b) return;
    hideTip();
    const t = document.createElement("div");
    t.className = "canvas-control-tip"; t.textContent = b.dataset.tip;
    document.body.appendChild(t);
    const r = b.getBoundingClientRect(), tr = t.getBoundingClientRect();
    t.style.left = Math.round(r.left - 8 - tr.width) + "px";
    t.style.top = Math.round(r.top + r.height / 2 - tr.height / 2) + "px";
    tip = { el: t, b, c }; ping(c);
  }
  function stack(c) {
    const s = c._cvnav, el = c.controlsEl;
    el.classList.add("cvn-stack");
    const seps = [];
    const arrange = () => {
      const want = [];
      let k = 0;
      for (const it of ORDER) {
        if (it === "|") { want.push(seps[k] || (seps[k] = Object.assign(document.createElement("div"), { className: "cvn-sep" }))); k++; continue; }
        const b = el.querySelector(it[1]); if (!b) continue;
        if (b.dataset.cvn !== it[0]) {
          b.dataset.cvn = it[0]; b.dataset.tip = TIP[it[0]];
          b.removeAttribute("title"); b.setAttribute("aria-label", TIP[it[0]]);
          if (it[0] !== "set") b.innerHTML = ICON[it[0]];
        }
        want.push(b);
      }
      for (const x of el.children) if (!want.includes(x)) want.push(x);   // + Text / zoom to selection (hidden) and anything else
      const cur = [...el.children];
      if (cur.length === want.length && cur.every((x, i) => x === want[i])) return;
      mo.disconnect();
      for (const x of want) el.appendChild(x);
      mo.observe(el, { childList: true });
    };
    const mo = new MutationObserver(arrange);
    arrange();
    mo.observe(el, { childList: true });
    s.arrange = arrange;
    const hit = e => {
      for (const b of el.querySelectorAll(".canvas-control[data-cvn]")) {
        if (!vis(b)) continue;
        const r = b.getBoundingClientRect();
        if (e.clientX >= r.left && e.clientX < r.right && e.clientY >= r.top - 1 && e.clientY < r.bottom + 1) return b;
      }
      return null;
    };
    el.addEventListener("pointermove", e => { const b = hit(e); if (b) showTip(c, b); else hideTip(); });
    el.addEventListener("pointerenter", e => { const b = hit(e); if (b) showTip(c, b); });
    el.addEventListener("pointerleave", hideTip);
    el.addEventListener("pointerdown", hideTip, true);
    el.addEventListener("wheel", hideTip, { passive: true });
  }
  window.addEventListener("keyup", e => { if (e.key === " " || e.code === "Space") spaceUp(); }, true);
  window.addEventListener("blur", spaceUp);
  // goal cvf-zoom CVR-C-068: Space held while the pointer is over a board arms Space + wheel / Space + drag even when
  // the keyboard focus is elsewhere (the file explorer row that opened the board); typing into a field is untouched
  window.addEventListener("keydown", e => {
    if (!(e.key === " " || e.code === "Space") || e.ctrlKey || e.metaKey || e.altKey) return;
    const c = hover;
    if (!live(c) || inEditor(e.target) || (e.target && e.target.closest && e.target.closest(".modal-container, .modal, .menu"))) return;
    e.preventDefault();
    if (!space) { space = true; c.wrapperEl.classList.add("is-space-held"); }
  }, true);

  /* ------------------------------------------------ card names, zoom threshold, Ctrl+drag (goal cvf-board)
     Show card names (always | hover | never) and the zoom threshold (percent; a card's content is not drawn
     while zoom < 0.25 x threshold / 100, so 100 hides it below 25 %) are wrapper classes kept in step with
     the zoom and the vault config. Ctrl+drag on the empty board draws a rectangle; on release the
     "Default Ctrl + Drag behavior" decides: menu (the five add actions) | text | note | media | web | group,
     each made the size of the rectangle. */
  const VIEWK = ["showCardNames", "zoomThreshold"];
  let NC = null;                           // canvas.js NODE_CLASS (attach's 4th argument)
  let cdm = null;                          // the open Ctrl+drag menu: {c, m, items: [[action, MenuItem]]}
  function viewCls(c) {
    const w = c.wrapperEl, g = cfg();
    w.style.setProperty("--cvn-z", String(c.zoom || 1));
    const nm = g ? String(g.get("showCardNames") || "always") : "always";
    for (const k of ["hover", "never"]) w.classList.toggle("cvn-names-" + k, nm === k);
    const th = g ? Number(g.get("zoomThreshold")) : 100;
    w.classList.toggle("cvn-hide-content", isFinite(th) && th > 0 && (c.zoom || 1) < 0.25 * th / 100);
  }
  const CDM = [["card", "Add card"], ["note", "Add note from vault"], ["media", "Add media from vault"], ["web", "Add web page"], ["group", "Create group"]];
  const MEDIA = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif", "mp3", "wav", "m4a", "ogg", "3gp", "flac", "mp4", "webm", "ogv", "mov", "mkv", "pdf"]);
  function freshId(c) {
    const taken = typeof c.ids === "function" ? c.ids() : new Set(c.nodes.keys());
    for (;;) { let s = ""; for (let i = 0; i < 16; i++) s += "0123456789abcdef"[Math.floor(Math.random() * 16)]; if (!taken.has(s)) return s; }
  }
  function addRaw(c, d, front) {
    const N = NC && NC[d.type]; if (!N || !live(c) || c.readonly) return null;
    const n = c.addNode(new N(c, d));
    if (front) {                           // a group goes first in nodes[] (JSON Canvas: lower index = below)
      const rest = [...c.nodes].filter(([k]) => k !== n.id);
      c.nodes.clear(); c.nodes.set(n.id, n); for (const [k, v] of rest) c.nodes.set(k, v);
    }
    c.renderEdges(); c.requestSave(); ping(c);
    return n;
  }
  function pickFile(c, title, pred, then) {
    if (!X || !X.FuzzySuggestModal) return;
    const fs = (c.app && c.app.vault && c.app.vault.getFiles ? c.app.vault.getFiles() : []).filter(pred);
    const m = new X.FuzzySuggestModal(c.app);
    m.setPlaceholder(title);
    m.emptyStateText = "No matching files.";
    m.getItems = () => fs;
    m.getItemText = f => f.path;
    m.onChooseItem = f => { if (live(c)) then(f.path); };
    m.open();
  }
  function askUrl(c, then) {
    if (!X || !X.Modal) return;
    const m = new X.Modal(c.app);
    m.setTitle("Add web page");
    const inp = document.createElement("input");
    inp.type = "text"; inp.value = "https://"; inp.style.width = "100%";
    m.contentEl.appendChild(inp);
    inp.addEventListener("keydown", e => {
      if (e.key !== "Enter") return;
      e.preventDefault();
      const u = inp.value.trim(); m.close();
      if (/^https?:\/\/[^\s/]+/i.test(u) && live(c)) then(u);
    });
    m.open();
    setTimeout(() => { try { inp.focus(); inp.select(); } catch (_) {} }, 0);
  }
  // r = the rectangle in canvas units {x, y, width, height}
  function modAct(c, a, r) {
    if (!live(c) || c.readonly) return;
    const R = { x: Math.round(r.x), y: Math.round(r.y), width: Math.max(1, Math.round(r.width)), height: Math.max(1, Math.round(r.height)) };
    const ext = p => String(p).split(".").pop().toLowerCase();
    if (a === "card" || a === "text") { c.createTextNode({ pos: R, size: R, focus: true }); ping(c); }
    else if (a === "note") pickFile(c, "Add a note from the vault…", f => ext(f.path) === "md",
      p => { c.createFileNode({ file: p, pos: R, size: R }); c.renderEdges(); ping(c); });
    else if (a === "media") pickFile(c, "Add media from the vault…", f => MEDIA.has(ext(f.path)),
      p => { c.createFileNode({ file: p, pos: R, size: R }); c.renderEdges(); ping(c); });
    else if (a === "web") askUrl(c, u => addRaw(c, Object.assign({ id: freshId(c), type: "link", url: u }, R)));
    else if (a === "group") { c.deselectAll(); const g = addRaw(c, Object.assign({ id: freshId(c), type: "group" }, R), true); if (g) c.select(g, false); }
  }
  function modMenu(c, r, e) {
    if (cdm) try { cdm.m.hide(); } catch (_) {}
    if (!X || !X.Menu) return;
    const m = new X.Menu(), items = [];
    for (const [a, label] of CDM) m.addItem(it => { it.setTitle(label).onClick(() => modAct(c, a, r)); items.push([a, it]); });
    const me = { c, m, items }; cdm = me;
    m.onHide(() => { if (cdm === me) cdm = null; ping(c); });
    m.showAtPosition({ x: e.clientX, y: e.clientY });
    ping(c);
  }
  function modDrag(c) {
    const w = c.wrapperEl;
    let d = null;
    const box = () => {
      const x = Math.min(d.x0, d.x1), y = Math.min(d.y0, d.y1);
      Object.assign(d.el.style, { left: x + "px", top: y + "px", width: Math.abs(d.x1 - d.x0) + "px", height: Math.abs(d.y1 - d.y0) + "px" });
    };
    // capture phase: canvas.js / canvas-select.js never start a marquee for it
    w.addEventListener("pointerdown", e => {
      if (space || e.button !== 0 || !(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
      if (e.pointerType && e.pointerType !== "mouse") return;
      if (c.readonly || inEditor(e.target)) return;
      if (e.target.closest && e.target.closest(".canvas-node, .canvas-controls, .canvas-settings-menu, .canvas-card-menu, .canvas-selection-toolbar, .canvas-selection-palette, .canvas-node-label")) return;
      e.preventDefault(); e.stopPropagation();
      w.focus({ preventScroll: true });
      d = { id: e.pointerId, x0: e.clientX, y0: e.clientY, x1: e.clientX, y1: e.clientY, moved: false, el: null };
      try { w.setPointerCapture(e.pointerId); } catch (_) {}
    }, true);
    w.addEventListener("pointermove", e => {
      if (!d || e.pointerId !== d.id) return;
      e.stopPropagation();
      d.x1 = e.clientX; d.y1 = e.clientY;
      if (!d.moved && Math.abs(d.x1 - d.x0) + Math.abs(d.y1 - d.y0) < 4) return;
      if (!d.el) { d.el = document.createElement("div"); d.el.className = "cvn-cdrag-box"; document.body.appendChild(d.el); }
      d.moved = true; box();
    }, true);
    const end = (e, cancel) => {
      if (!d || e.pointerId !== d.id) return;
      e.stopPropagation();
      const g = d; d = null;
      try { w.releasePointerCapture(g.id); } catch (_) {}
      if (g.el) g.el.remove();
      if (cancel || !g.moved) return;
      g.x1 = e.clientX; g.y1 = e.clientY;
      const p0 = c.toCanvas(Math.min(g.x0, g.x1), Math.min(g.y0, g.y1)), p1 = c.toCanvas(Math.max(g.x0, g.x1), Math.max(g.y0, g.y1));
      const r = { x: p0.x, y: p0.y, width: p1.x - p0.x, height: p1.y - p0.y };
      const how = cfg() ? String(cfg().get("defaultModDragBehavior") || "menu") : "menu";
      if (how === "menu") modMenu(c, r, e); else modAct(c, how, r);
    };
    w.addEventListener("pointerup", e => end(e, false), true);
    w.addEventListener("pointercancel", e => end(e, true), true);
  }

  /* ------------------------------------------------ census (main.js canvasTok) */
  const vis = e => !!e && e.getClientRects().length > 0;
  const ctr = e => { const r = e.getBoundingClientRect(); return Math.round(r.x + r.width / 2) + "," + Math.round(r.y + r.height / 2); };
  const b01 = v => (v ? 1 : 0);
  function census(v) {
    const c = v && v.canvas; if (!c || !c._cvnav) return "";
    const s = c._cvnav, w = c.wrapperEl;
    let t = " [cvnz:" + (w.dataset.zoom || "1") + "|" + Math.round(c.tx) + "|" + Math.round(c.ty) + "]";
    // every painted control in DOM (column) order: the stack names, + Text / zoom to selection as add / sel if shown
    const NM = [["mod-add-text", "add"], ["mod-zoom-selection", "sel"]];
    const pts = c.controlsEl ? [...c.controlsEl.querySelectorAll(".canvas-control")].filter(vis)
      .map(b => [b.dataset.cvn || (NM.find(n => b.classList.contains(n[0])) || [0, ""])[1], b]).filter(p => p[0]) : [];
    if (pts.length) {
      t += " [cvctl:" + pts.map(p => p[0] + "=" + ctr(p[1])).join("|") + "]";
      t += " [cvbtn:" + pts.map(p => { const r = p[1].getBoundingClientRect(); return p[0] + "=" + [r.x, r.y, r.width, r.height].map(Math.round).join(","); }).join("|") + "]";
    }
    if (tip && !(tip.b.isConnected && vis(tip.b))) { tip.el.remove(); tip = null; }   // its board went away (no ping: we are the census)
    if (tip && tip.c === c) { const r = tip.el.getBoundingClientRect(); t += " [cvtip:" + tip.el.textContent + "@" + [r.x, r.y, r.width, r.height].map(Math.round).join(",") + "]"; }
    if (s.btn.set) t += " [cvseticon:" + (s.btn.set.dataset.icon || "gear") + "]";
    { const sl = document.getSelection(); t += " [cvdocsel:" + (sl ? String(sl).length : 0) + "]"; }
    const sz = parseFloat(getComputedStyle(w).backgroundSize);
    if (isFinite(sz)) t += " [cvgrid:sz=" + Math.round(sz * 100) / 100 + "|z=" + (w.dataset.zoom || "1") + "]";
    const o = c.options || {};
    t += " [cvnopt:g=" + b01(o.snapToGrid) + "|o=" + b01(o.snapToObjects) + "|ro=" + b01(o.readOnly) + "]";
    if (s.menu && s.menu.isConnected) {
      const its = SETTINGS.filter(([k]) => vis(s.items[k]));
      t += " [cvset:n=" + its.length + its.map(([k]) => "|" + k + "=" + ctr(s.items[k])).join("") + "]";
      if (s.more && vis(s.more)) t += " [cvsetmore=" + ctr(s.more) + "]";   // af-fix 2b
    }
    if (help) {
      const rows = [...help.contentEl.querySelectorAll(".canvas-help-row")];
      t += " [cvhelp:n=" + rows.length + "]";
      t += " [cvhelpk:" + rows.map(r => r.children[0].textContent + "=" + r.children[1].textContent).join("|") + "]";
    }
    if (jump) t += " [cvjump:n=" + jump.n + "]";
    if (cdm && cdm.c === c && cdm.m.dom && cdm.m.dom.isConnected)
      t += " [cvcdm:" + cdm.items.filter(([, it]) => vis(it.dom)).map(([a, it]) => a + "@" + ctr(it.dom)).join("|") + "]";
    return t;
  }

  const api = { attach, onWheel, census, zoomToSelection, resetZoom, jumpTo };
  Object.defineProperty(window, "CanvasNav", { configurable: true, enumerable: false, writable: false, value: Object.freeze(api) });
})();
