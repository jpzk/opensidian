// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* canvas export (goal cv-export): "Canvas: Export as image" saves the board as a
   PNG next to the canvas file, written from the public help page
   (help.obsidian.md/plugins/canvas, "Export a canvas as an image") — clean room.

   The dialog offers
     Area      the whole board (every card's bounding box + PAD = 100 px on each
               side) or the current view (the board wrapper as it is
               on screen, at the screen's devicePixelRatio)
     Zoom      continuous (slider + typed field, ZMIN..ZMAX, fresh at 1 each run);
               the image size "W × H" is shown before export
     Privacy   every text run is drawn as a solid rounded bar (off on every run)
     Footer    a band under the image with our own mark (on by default); it
               adds bandOf(z) px BELOW the estimated size, the estimate excludes it
   A size with a side over MAX_SIDE or an area over MAX_AREA is refused before
   anything is rasterised (a notice, no file). On an empty board the command
   says so in a notice and opens no dialog.

   RASTERISING. The image is painted from the MODEL on a 2D <canvas>: cards from
   their data (x/y/width/height/text/file/url/label), styled with the colours the
   board shows (computed style of each card), edges from the geometry canvas.js
   already rendered (the SVG path d -> Path2D, the arrow polygons, the labels).
   No DOM snapshot (an SVG foreignObject can taint the canvas in WebKit).
   canvas.toBlob("image/png") -> the core command canvas_attach (src-tauri canvasfs.rs:
   vault-confined, create-new only, not trust-gated, so it works in a restricted
   vault) at the first free name in the VAULT ROOT ("<canvas>.png", "<canvas> 1.png",
   ...), the name shown in the dialog; a notice says where it went. Never a path
   outside the vault, never an overwrite. (No native save dialog: that would need a
   host path outside the vault.)

   Load order (index.html): canvas.js -> ... -> canvas-edges.js -> canvas-export.js
   -> main.js. canvas.js does not call in: this file registers its command itself
   (app.commands.addCommand) once the page has loaded; main.js canvasTok() appends
   census(view). One frozen global, window.CanvasExport. */
(function () {
  "use strict";
  const PAD = 100;                         // margin around the cards on each side of a whole-board image
  const MAX_SIDE = 16384, MAX_AREA = 64 * 1024 * 1024;
  const ZMIN = 0.25, ZMAX = 8, ZSTEP = 0.05;  // the zoom slider: continuous, typed values snap to 0.01
  const BAND = 64;                         // footer band height at zoom 1 (our own mark), under the estimate
  const host = window.__osPluginHost;      // captured now: the loader seals the host before plugins run
  let app = (host && host.app) || null, X = (host && host.api) || null;

  const opt = { mode: "full", z: 1, p: 0, l: 1 };   // area is kept between runs; zoom, privacy and footer start fresh
  let dlg = null;                             // the open dialog
  let last = null, nExp = 0, refused = null, nRef = 0;

  const vis = e => !!e && e.isConnected && e.getClientRects().length > 0;
  const ping = c => { const s = c && c._cvnav; if (s && s.changed) try { s.changed(); } catch (_) {} };
  const dprOf = () => window.devicePixelRatio || 1;
  const fmtSize = s => s.w + " × " + s.h;

  /* ---------------------------------------------- which board */
  function boardOf() {
    let ws = null; try { ws = app && app.workspace; } catch (_) {}
    if (!ws) return null;
    try {
      const l = ws.activeLeaf, v = l && l.view;
      if (v && v.canvas && v.getViewType && v.getViewType() === "canvas" && vis(v.containerEl)) return v.canvas;
    } catch (_) {}
    try {
      const v = ws.getLeavesOfType("canvas").map(l => l.view).find(x => x && x.canvas && vis(x.containerEl));
      return v ? v.canvas : null;
    } catch (_) { return null; }
  }

  /* ---------------------------------------------- size */
  function bboxOf(c) {
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of c.nodes.values()) { const d = n.data; x0 = Math.min(x0, d.x); y0 = Math.min(y0, d.y); x1 = Math.max(x1, d.x + d.width); y1 = Math.max(y1, d.y + d.height); }
    return x0 === Infinity ? null : { x0, y0, x1, y1 };
  }
  function sizeOf(c, mode, z) {
    if (mode === "view") {
      const r = c.wrapperEl.getBoundingClientRect(), k = dprOf() * z;
      return { mode, z, w: Math.round(r.width * k), h: Math.round(r.height * k), k };
    }
    const b = bboxOf(c) || { x0: 0, y0: 0, x1: 1, y1: 1 };
    return { mode, z, w: Math.round((b.x1 - b.x0 + 2 * PAD) * z), h: Math.round((b.y1 - b.y0 + 2 * PAD) * z), b };
  }
  const bandOf = z => Math.max(24, Math.round(BAND * z));   // the footer band: added BELOW the estimated size
  const zClamp = v => Math.min(ZMAX, Math.max(ZMIN, Math.round(v * 100) / 100));
  const tooBig = s => s.w < 1 || s.h < 1 || s.w > MAX_SIDE || s.h > MAX_SIDE || s.w * s.h > MAX_AREA;

  /* ---------------------------------------------- colours */
  let scratch = null;
  function sctx() {
    if (!scratch) { const cv = document.createElement("canvas"); cv.width = cv.height = 1; scratch = cv.getContext("2d", { willReadFrequently: true }); }
    return scratch;
  }
  // a CSS colour the 2D context understands and that is not fully transparent, else null
  function solid(v) {
    if (!v) return null;
    const s = sctx();
    s.fillStyle = "#010203"; s.fillStyle = v;
    if (s.fillStyle === "#010203" && String(v).trim() !== "#010203") return null;   // not a colour the canvas accepts
    s.clearRect(0, 0, 1, 1); s.fillRect(0, 0, 1, 1);
    return s.getImageData(0, 0, 1, 1).data[3] > 0 ? v : null;
  }
  function boardBg(c) {
    for (let e = c.wrapperEl; e && e.nodeType === 1; e = e.parentElement) {
      const v = solid(getComputedStyle(e).backgroundColor); if (v) return v;
    }
    return solid(getComputedStyle(document.body).backgroundColor) || "#1e1e1e";
  }

  /* ---------------------------------------------- painting */
  function rrect(ctx, x, y, w, h, r) {
    r = Math.max(0, Math.min(r, w / 2, h / 2));
    ctx.beginPath();
    ctx.moveTo(x + r, y); ctx.lineTo(x + w - r, y); ctx.arcTo(x + w, y, x + w, y + r, r);
    ctx.lineTo(x + w, y + h - r); ctx.arcTo(x + w, y + h, x + w - r, y + h, r);
    ctx.lineTo(x + r, y + h); ctx.arcTo(x, y + h, x, y + h - r, r);
    ctx.lineTo(x, y + r); ctx.arcTo(x, y, x + r, y, r);
    ctx.closePath();
  }
  // greedy word wrap to maxW; a word longer than the line is broken by characters
  function wrap(ctx, text, maxW) {
    const out = [];
    for (const para of String(text).split(/\r?\n/)) {
      if (!para.trim()) { out.push(""); continue; }
      let line = "";
      for (const word of para.split(/\s+/).filter(Boolean)) {
        const t = line ? line + " " + word : word;
        if (ctx.measureText(t).width <= maxW || !line) {
          if (ctx.measureText(t).width <= maxW) { line = t; continue; }
          let piece = "";                                    // a single over-long word
          for (const ch of word) { if (piece && ctx.measureText(piece + ch).width > maxW) { out.push(piece); piece = ""; } piece += ch; }
          line = piece; continue;
        }
        out.push(line); line = word;
        if (ctx.measureText(line).width > maxW) {
          let piece = "";
          for (const ch of word) { if (piece && ctx.measureText(piece + ch).width > maxW) { out.push(piece); piece = ""; } piece += ch; }
          line = piece;
        }
      }
      out.push(line);
    }
    while (out.length && out[out.length - 1] === "") out.pop();
    return out;
  }
  function painter(ctx, privacy) {
    const st = { glyphs: 0, bars: 0 };
    // one text line at (x, baseline y), left / centre aligned by ctx.textAlign
    st.line = (s, x, y, fs) => {
      if (!s) return;
      if (!privacy) { ctx.fillText(s, x, y); st.glyphs++; return; }
      // a solid rounded bar the width of the run, in the run's own colour (links keep their link colour)
      const w = Math.max(4, ctx.measureText(s).width), h = Math.max(3, fs * 0.72);
      const left = ctx.textAlign === "center" ? x - w / 2 : x;
      ctx.save(); ctx.globalAlpha = 0.85; rrect(ctx, left, y - h, w, h, h / 2); ctx.fill(); ctx.restore();
      st.bars++;
    };
    return st;
  }
  const fontOf = cs => (cs.fontStyle && cs.fontStyle !== "normal" ? cs.fontStyle + " " : "") + (cs.fontWeight && cs.fontWeight !== "400" && cs.fontWeight !== "normal" ? cs.fontWeight + " " : "") +
    (parseFloat(cs.fontSize) || 14) + "px " + (cs.fontFamily || "sans-serif");
  function cardText(n) {
    const d = n.data;
    if (d.type === "text") return String(d.text || "").replace(/^#{1,6}\s+/gm, "");
    if (d.type === "link") return String(d.url || "");
    if (d.type === "file") {                                   // the note as the card shows it, else its path
      const box = n.containerEl && n.containerEl.querySelector(".canvas-node-container");
      const t = box ? String(box.innerText || box.textContent || "").trim() : "";
      return t || String(d.file || "");
    }
    return "";
  }
  function paintNode(ctx, st, n, bg) {
    const d = n.data, el = n.containerEl, box = el && (el.querySelector(".canvas-node-container") || el);
    const cs = box ? getComputedStyle(box) : null;
    const bw = cs ? parseFloat(cs.borderTopWidth) || 0 : 2, rad = cs ? parseFloat(cs.borderTopLeftRadius) || 0 : 8;
    const fill = cs && solid(cs.backgroundColor), stroke = (cs && solid(cs.borderTopColor)) || "#888";
    rrect(ctx, d.x + bw / 2, d.y + bw / 2, d.width - bw, d.height - bw, rad);
    if (fill) { ctx.fillStyle = fill; ctx.fill(); }
    if (bw > 0) { ctx.lineWidth = bw; ctx.strokeStyle = stroke; ctx.stroke(); }
    const ink = (cs && solid(cs.color)) || "#ddd";
    // the label above the card (file name, group label), as canvas.js draws .canvas-node-label
    const lab = n.labelEl && String(n.labelEl.textContent || "").trim();
    if (lab) {
      const lcs = getComputedStyle(n.labelEl), fs = parseFloat(lcs.fontSize) || 12;
      ctx.font = fontOf(lcs); ctx.fillStyle = solid(lcs.color) || ink; ctx.textAlign = "left"; ctx.textBaseline = "alphabetic";
      st.line(lab, d.x, d.y - 4, fs);
    }
    const text = cardText(n);
    if (!text) return;
    const fs = cs ? parseFloat(cs.fontSize) || 14 : 14, lh = Math.round(fs * 1.45), px = 12, py = 8;
    ctx.save();
    rrect(ctx, d.x, d.y, d.width, d.height, rad); ctx.clip();
    ctx.font = cs ? fontOf(cs) : "14px sans-serif";
    ctx.fillStyle = d.type === "link" ? (solid(getComputedStyle(box.querySelector(".canvas-link") || box).color) || ink) : ink;
    ctx.textAlign = "left"; ctx.textBaseline = "alphabetic";
    let y = d.y + py + fs;
    for (const l of wrap(ctx, text, Math.max(10, d.width - 2 * px))) {
      if (y - fs > d.y + d.height) break;
      st.line(l, d.x + px, y, fs);
      y += lh;
    }
    ctx.restore();
  }
  function paintEdges(ctx, st, c, bg) {
    if (!c.edgeEl) return;
    for (const g of c.edgeEl.querySelectorAll(".canvas-edge")) {
      const line = g.querySelector(".canvas-edge-line");
      let col = "#888";
      if (line && line.getAttribute("d")) {
        col = solid(getComputedStyle(line).stroke) || col;
        ctx.save(); ctx.strokeStyle = col; ctx.lineWidth = parseFloat(getComputedStyle(line).strokeWidth) || 2; ctx.lineCap = "round";
        try { ctx.stroke(new Path2D(line.getAttribute("d"))); } catch (_) {}
        ctx.restore();
      }
      for (const p of g.querySelectorAll(".canvas-edge-arrow")) {
        const pts = String(p.getAttribute("points") || "").trim().split(/[\s,]+/).map(Number);
        if (pts.length < 6 || pts.some(v => !isFinite(v))) continue;
        ctx.beginPath(); ctx.moveTo(pts[0], pts[1]);
        for (let i = 2; i + 1 < pts.length; i += 2) ctx.lineTo(pts[i], pts[i + 1]);
        ctx.closePath(); ctx.fillStyle = solid(getComputedStyle(p).fill) || col; ctx.fill();
      }
      const t = g.querySelector(".canvas-edge-label");
      const s = t && String(t.textContent || "").trim();
      if (s) {
        const tcs = getComputedStyle(t), fs = parseFloat(tcs.fontSize) || 14;
        const x = parseFloat(t.getAttribute("x")) || 0, y = parseFloat(t.getAttribute("y")) || 0;
        ctx.font = fontOf(tcs); ctx.textAlign = "center"; ctx.textBaseline = "alphabetic";
        const w = ctx.measureText(s).width;
        ctx.fillStyle = bg; ctx.fillRect(x - w / 2 - 4, y - fs, w + 8, fs * 1.35);   // the label's plate over the line
        ctx.fillStyle = solid(tcs.fill) || solid(tcs.color) || "#ddd";
        st.line(s, x, y, fs);
      }
    }
  }
  function paint(c, ctx, sz, privacy) {
    const bg = boardBg(c), st = painter(ctx, privacy);
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = bg; ctx.fillRect(0, 0, sz.w, sz.h);
    if (sz.mode === "view") { const k = sz.k; ctx.setTransform(k * c.zoom, 0, 0, k * c.zoom, c.tx * k, c.ty * k); }
    else { const z = sz.z; ctx.setTransform(z, 0, 0, z, (PAD - sz.b.x0) * z, (PAD - sz.b.y0) * z); }
    const ns = [...c.nodes.values()];
    for (const n of ns) if (n.data.type === "group") paintNode(ctx, st, n, bg);   // groups under the edges, as on the board
    paintEdges(ctx, st, c, bg);
    for (const n of ns) if (n.data.type !== "group") paintNode(ctx, st, n, bg);
    return { glyphs: st.glyphs, bars: st.bars };
  }

  /* the footer band under the board: a hairline, a small mark (two overlapping
     rounded squares) and the app name, bottom-left, in the board's own colours */
  function paintBand(c, ctx, w, y0, h) {
    const bg = boardBg(c);
    let ink = "#9a9a9a";
    try { ink = solid(getComputedStyle(c.wrapperEl).color) || ink; } catch (_) {}
    ctx.setTransform(1, 0, 0, 1, 0, 0);
    ctx.fillStyle = bg; ctx.fillRect(0, y0, w, h);
    ctx.save();
    ctx.globalAlpha = 0.35; ctx.fillStyle = ink; ctx.fillRect(0, y0, w, Math.max(1, Math.round(h / 64)));
    ctx.restore();
    const s = Math.round(h * 0.4), pad = Math.round(h * 0.3), cy = y0 + h / 2;
    ctx.save(); ctx.fillStyle = ink;
    ctx.globalAlpha = 0.55; rrect(ctx, pad, cy - s / 2, s, s, s * 0.25); ctx.fill();
    ctx.globalAlpha = 0.9; rrect(ctx, pad + s * 0.45, cy - s * 0.3, s * 0.8, s * 0.8, s * 0.2); ctx.fill();
    ctx.globalAlpha = 0.85;
    ctx.font = "600 " + Math.round(h * 0.34) + "px sans-serif"; ctx.textAlign = "left"; ctx.textBaseline = "middle";
    ctx.fillText("opensidian", pad + s * 1.6, cy);
    ctx.restore();
  }

  /* ---------------------------------------------- export */
  const blobOf = cv => new Promise((res, rej) => cv.toBlob(b => (b ? res(b) : rej(new Error("the image could not be encoded"))), "image/png"));
  function bytesOf(b) {
    if (typeof b.arrayBuffer === "function") return b.arrayBuffer().then(a => new Uint8Array(a));
    return new Promise((res, rej) => { const r = new FileReader(); r.onload = () => res(new Uint8Array(r.result)); r.onerror = () => rej(r.error); r.readAsArrayBuffer(b); });
  }
  // is p in use? Our own earlier exports, the vault model, the backend image index (list_images: works in a
  // restricted vault, where the adapter answers EPERM and the model may not have caught up), the adapter.
  const wrote = new Set();
  async function taken(p) {
    const V = app.vault, lp = p.toLowerCase();
    if (wrote.has(lp)) return true;
    try { if (V.getAbstractFileByPathInsensitive(p)) return true; } catch (_) {}
    try { const I = invoke(), l = I && await I("list_images", {}); if (Array.isArray(l) && l.some(x => String(x).toLowerCase() === lp)) return true; } catch (_) {}
    try { if (V.adapter && typeof V.adapter.exists === "function" && await V.adapter.exists(p)) return true; } catch (_) {}
    return false;
  }
  async function freePath(c) {
    const cp = (c.view && c.view.path) || "Canvas.canvas", base = cp.split("/").pop().replace(/\.canvas$/i, "") || "Canvas";
    let p = base + ".png";
    for (let i = 1; await taken(p); i++) p = base + " " + i + ".png";
    return p;
  }
  const invoke = () => { try { if (typeof inv === "function") return inv; } catch (_) {} const T = window.__TAURI__; return T && T.core ? T.core.invoke : null; };   // main.js global lexical
  function b64Of(u8) {
    let s = "";
    for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
    return btoa(s);
  }
  // canvas_attach is create-new only (EEXIST when taken). A name taken between the pick and the write
  // moves on to "<stem> 1.png", "<stem> 2.png", ... without trusting taken(): in a restricted vault
  // the adapter may not see every file.
  async function attach(path, data) {
    const I = invoke();
    if (!I) throw new Error("no core command bridge");
    const stem = path.replace(/( \d+)?\.png$/i, "");
    for (let i = 0; ; i++) {
      try { return await I("canvas_attach", { path, dataB64: data }); }
      catch (e) {
        const m = String((e && e.message) || e);
        if (!/^EEXIST/.test(m) || i >= 200) throw new Error(m);
        path = stem + " " + (i + 1) + ".png";
      }
    }
  }
  const notice = m => { try { if (X && X.Notice) new X.Notice(m); } catch (_) {} };
  function refuse(c, sz) {
    nRef++; refused = { w: sz.w, h: sz.h, n: nRef };
    notice("Image too large to export (" + fmtSize(sz) + "). Choose a smaller zoom or the current view.");
    if (dlg && dlg.msgEl) dlg.msgEl.textContent = "Too large: at most " + MAX_SIDE + " px a side and " + (MAX_AREA / 1048576) + "M pixels.";
    ping(c);
  }
  async function exportImage(c, o) {
    const sz = sizeOf(c, o.mode, o.z);
    if (tooBig(sz)) { refuse(c, sz); return null; }
    const band = o.l ? bandOf(o.z) : 0;
    const cv = document.createElement("canvas"); cv.width = sz.w; cv.height = sz.h + band;
    const ctx = cv.getContext("2d");
    if (!ctx) throw new Error(window.opensidianCanvasOff || "no 2D context");   // #144 toggle 6: names the switch
    const st = paint(c, ctx, sz, !!o.p);
    if (band) paintBand(c, ctx, sz.w, sz.h, band);
    const bytes = await bytesOf(await blobOf(cv));
    let path = o.to;
    if (!path || await taken(path)) path = await freePath(c);   // never an overwrite
    path = await attach(path, b64Of(bytes));
    wrote.add(String(path).toLowerCase());
    nExp++;
    last = { glyphs: st.glyphs, bars: st.bars, w: sz.w, h: sz.h + band, file: path, n: nExp, dpr: dprOf() };
    notice("Canvas exported to " + path);
    ping(c);
    return path;
  }

  /* ---------------------------------------------- dialog */
  function openDialog(c) {
    if (dlg || !X || !X.Modal) return;
    opt.z = 1; opt.p = 0; opt.l = 1;            // zoom, privacy and the footer start fresh on every run
    const m = new X.Modal(c.app || app);
    m.setTitle("Export as image");
    m.modalEl.classList.add("cvx-modal");
    const D = { m, c, b: {}, sizeEl: null, msgEl: null, to: "" };
    const mk = (tag, cls, parent, txt) => { const e = document.createElement(tag); if (cls) e.className = cls; if (txt != null) e.textContent = txt; parent.appendChild(e); return e; };
    const button = (parent, key, txt, fn) => {
      const b = mk("button", "cvx-opt", parent, txt); b.type = "button";
      b.addEventListener("click", e => { e.preventDefault(); e.stopPropagation(); fn(); update(); });
      D.b[key] = b; return b;
    };
    const sw = (parent, key, get, set) => {        // an Off/On switch
      const b = button(parent, key, "", () => set(get() ? 0 : 1)); b.setAttribute("role", "switch"); return b;
    };
    const paintSw = (b, on) => { b.classList.toggle("is-active", !!on); b.setAttribute("aria-checked", on ? "true" : "false"); b.textContent = on ? "On" : "Off"; };
    function update(from) {
      for (const k of ["full", "view"]) D.b[k].classList.toggle("is-active", opt.mode === k);
      if (from !== "range") D.b.zr.value = String(opt.z);
      if (from !== "field") D.b.zin.value = String(opt.z);
      paintSw(D.b.priv, opt.p); paintSw(D.b.logo, opt.l);
      const sz = sizeOf(c, opt.mode, opt.z);
      D.sizeEl.textContent = fmtSize(sz);
      D.bandEl.textContent = opt.l ? "+ " + bandOf(opt.z) + " px footer band" : "";
      D.toEl.textContent = D.to || "…";
      D.msgEl.textContent = tooBig(sz) ? "Too large to export: at most " + MAX_SIDE + " px a side and " + (MAX_AREA / 1048576) + "M pixels." : "";
      ping(c);
    }
    D.update = update;
    m.onOpen = function () {
      const ce = this.contentEl;
      const row = label => { const r = mk("div", "cvx-row", ce); mk("div", "cvx-label", r, label); return mk("div", "cvx-ctl", r); };
      let r = row("Area");
      button(r, "full", "Whole canvas", () => { opt.mode = "full"; });
      button(r, "view", "Current view", () => { opt.mode = "view"; });
      r = row("Zoom");
      const zr = mk("input", "cvx-zoom", r); zr.type = "range"; zr.min = String(ZMIN); zr.max = String(ZMAX); zr.step = String(ZSTEP); D.b.zr = zr;
      zr.addEventListener("input", () => { opt.z = zClamp(parseFloat(zr.value) || 1); update("range"); });
      const zin = mk("input", "cvx-zin", r); zin.type = "text"; zin.inputMode = "decimal"; zin.size = 5; zin.spellcheck = false; D.b.zin = zin;
      const takeField = fin => {
        const v = parseFloat(String(zin.value).replace(",", "."));
        if (isFinite(v) && v > 0) { opt.z = zClamp(v); update(fin ? undefined : "field"); }
        else if (fin) update();
      };
      zin.addEventListener("input", () => takeField(false));
      zin.addEventListener("change", () => takeField(true));
      zin.addEventListener("keydown", e => { if (e.key === "Enter") { e.preventDefault(); e.stopPropagation(); takeField(true); } });
      r = row("Privacy mode (text as bars)");
      sw(r, "priv", () => opt.p, v => { opt.p = v; });
      r = row("Footer band with app mark");
      sw(r, "logo", () => opt.l, v => { opt.l = v; });
      r = row("Image size");
      D.sizeEl = mk("span", "cvx-size", r);
      D.bandEl = mk("span", "cvx-band", r);
      r = row("Saved as");
      D.toEl = mk("span", "cvx-to", r);
      D.msgEl = mk("div", "cvx-warn", ce);
      const foot = mk("div", "cvx-foot", ce);
      const go = mk("button", "mod-cta cvx-go", foot, "Export"); go.type = "button"; D.b.go = go;
      go.addEventListener("click", e => {
        e.preventDefault(); e.stopPropagation();
        takeField(true);
        const o = { mode: opt.mode, z: opt.z, p: opt.p, l: opt.l, to: D.to }, sz = sizeOf(c, o.mode, o.z);
        if (tooBig(sz)) { refuse(c, sz); return; }   // refused before rasterising; the dialog stays for another choice
        m.close();
        exportImage(c, o).catch(err => { console.error("[canvas] export:", err); notice("Canvas not exported — " + ((err && err.message) || err)); ping(c); });
      });
      dlg = D; update();
      freePath(c).then(p => { if (dlg === D) { D.to = p; update(); } }).catch(() => {});
    };
    const esc = e => { if (e.key === "Escape" && dlg === D) { e.preventDefault(); e.stopPropagation(); m.close(); } };
    m.onClose = function () { document.removeEventListener("keydown", esc, true); if (dlg === D) dlg = null; ping(c); };
    document.addEventListener("keydown", esc, true);
    m.open();
  }

  /* ---------------------------------------------- command */
  let done = false;
  function register() {
    if (done) return;
    app = app || (window.__osPluginHost && window.__osPluginHost.app) || null;
    X = X || (window.__osPluginHost && window.__osPluginHost.api) || null;
    const cm = app && app.commands;
    if (!cm || typeof cm.addCommand !== "function") return;
    try {
      cm.addCommand({ id: "canvas:export-image", name: "Canvas: Export as image", checkCallback: chk => {
        const c = boardOf();
        if (!c) return false;                     // listed whenever a board is the active view
        if (chk) return true;
        if (!c.nodes.size) { notice("Nothing to export: this canvas is empty."); return true; }   // no dialog
        openDialog(c);
        return true;
      } });
      done = typeof cm.findCommand !== "function" || !!cm.findCommand("canvas:export-image");
    } catch (e) { console.error("[canvas] export command:", e); }
  }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", register, { once: true });
  else setTimeout(register, 0);
  document.addEventListener("pointerdown", e => { if (!done && e.target && e.target.closest && e.target.closest(".canvas-wrapper")) register(); }, true);

  /* ---------------------------------------------- census (main.js canvasTok) */
  const ctr = e => { const r = e.getBoundingClientRect(); return Math.round(r.x + r.width / 2) + "," + Math.round(r.y + r.height / 2); };
  const esc = s => String(s).replace(/[[\]|@]/g, "_");
  function census(v) {
    let t = "";
    if (dlg && dlg.sizeEl) {
      t += " [cvxdlg:mode=" + opt.mode + "|z=" + opt.z + "|p=" + (opt.p ? 1 : 0) + "|l=" + (opt.l ? 1 : 0) + "|size=" + dlg.sizeEl.textContent +
        "|band=" + (opt.l ? bandOf(opt.z) : 0) + "|to=" + esc(dlg.to) + "|dpr=" + dprOf();
      for (const k of ["full", "view", "zr", "zin", "priv", "logo", "go"]) if (vis(dlg.b[k])) t += "|" + k + "=" + ctr(dlg.b[k]);
      t += "]";
    }
    if (last) t += " [cvexport:glyphs=" + last.glyphs + "|bars=" + last.bars + "|w=" + last.w + "|h=" + last.h + "|file=" + esc(last.file) + "|n=" + last.n + "|dpr=" + last.dpr + "]";
    if (refused) t += " [cvxrefused:" + refused.w + "x" + refused.h + "|n=" + refused.n + "]";
    return t;
  }

  const api = { census, open: c => openDialog(c || boardOf()), size: (c, mode, z) => sizeOf(c, mode, z), exportImage };
  Object.defineProperty(window, "CanvasExport", { configurable: true, enumerable: false, writable: false, value: Object.freeze(api) });
})();
