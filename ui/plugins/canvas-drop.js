// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* canvas drop (goal cv-drop): things dropped on the board become cards.
   Written from the public help page (Canvas: "Add cards from folders", "Add
   cards") and the JSON Canvas 1.0 spec (clean room).

   Load order (index.html): canvas.js -> ... -> canvas-edges.js -> canvas-drop.js
   -> main.js. This file registers two document listeners and defines
   window.CanvasDrop; it never edits canvas.js and needs no per-board install.

   FROM THE FILE EXPLORER (CV-100, goal cvf-drop): a file, a Shift+click selection or a folder (recursively)
   dropped on a board becomes file cards at the drop point, as stock lays them out — see "everything dragged
   from the file explorer" below. One synchronous placement -> ONE cv-history step; the new cards end selected.

   A FILE from another application (CV-101). A real OS drop reaches the page
   only as main.js's `drop-files` event -> attachDrop(paths, at); "Insert
   attachment" (R31.7) is the keyboard road into the same attachDrop. When the
   board under the drop point (or, with no point, the active leaf) is a canvas,
   attachDrop hands the paths here: attach_files copies them into the vault with
   its own caps and refusal sentences (the "note" argument is the board's path:
   the attachment folder is the vault's setting, the note only names the
   requester), and every file that landed becomes one file card at the drop
   point (no point: the last pointer position on that board, else its centre).
   A refused file adds no card; its sentence is shown through the host's drop
   banner. Browser URL drags are refused by attach_files (R31.8) — a URL becomes
   a link card through the board menu (cv-menus "Add web page"), not here.

   Census (main.js canvasTok, appended):
     [cvdrop:<copied>/<refused>]   the last attach on a board
     [cvdropf:<folder>|n=<cards added>|cap=<cap>]   the last folder drop (n=0 when refused)
     [cvdropbm:<kind>|n=<cards added>|ref=<refused>|cap=<cap>]   goal cv-dragin: the last bookmark drop (kind file|heading|block|group|<type>)
     [cvdropmsg:<text>]            the last sentence this file showed (refusal / cap notice)
     [cvpaste:<type:size|none>|types=<clipboard types>|<vault path>]   goal cv-journeys: the last Ctrl+V the board handed to the catcher */
(function () {
  "use strict";
  const CAP = 50;                       // files per folder drop (help: "Add cards from folders")
  // stock lays a dropped card with its TOP-LEFT on the drop point; develop centres it, and 961-cvjourney (J1, J3..J6)
  // asserts the centring. Coordinator ruling 2 (2026-10-26 03:15Z): stock placement waits behind this flag until the
  // operator lets cvjourney change. false = every explorer / toolbar drop is centred on the point (first card of a row).
  const CV_DROP_TOPLEFT = true;
  // stock adds a dropped folder's files recursively, one row at a 445 pitch; develop adds its direct files only, laid out by
  // CanvasArrange's grid with the grid's top-left on the point, and 954-cvdrop CV-100 asserts that. Coordinator ruling 3
  // (2026-10-26 03:30Z): the stock folder drop waits behind this flag. false = develop's folder drop.
  const CV_DROP_FOLDER_STOCK = true;
  const IMG = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif"]);
  const HOST = window.__osPluginHost;   // captured at load: the loader seals the host before plugins run
  const app = HOST && HOST.app ? HOST.app : null, X = HOST && HOST.api ? HOST.api : null;
  let last = { att: "", fold: "", msg: "", paste: "", ex: "", bm: "" };
  let ptr = null;                       // {x, y, w}: the last pointer position over a board wrapper

  const vis = e => !!e && e.isConnected && e.getClientRects().length > 0;
  const ro = c => !!(c.readonly || (c.options && c.options.readOnly));
  const extOf = p => { const n = String(p || "").split("/").pop(); const d = n.lastIndexOf("."); return d > 0 ? n.slice(d + 1).toLowerCase() : ""; };
  const sizeOf = p => extOf(p) === "md" ? { width: 400, height: 400 } : { width: 400, height: IMG.has(extOf(p)) ? 300 : 400 };

  function boards() {
    try { return app.workspace.getLeavesOfType("canvas").map(l => l.view).filter(v => v && v.canvas && vis(v.containerEl)); }
    catch (_) { return []; }
  }
  function boardAt(el) {
    const w = el && el.closest ? el.closest(".canvas-wrapper") : null;
    if (!w) return null;
    const v = boards().find(b => b.canvas.wrapperEl === w);
    return v ? v.canvas : null;
  }
  function activeBoard() {
    try {
      const l = app.workspace.activeLeaf, v = l && l.view;
      if (v && v.canvas && v.getViewType && v.getViewType() === "canvas" && vis(v.containerEl)) return v.canvas;
    } catch (_) {}
    return null;
  }
  // the board an attach goes to, or null (attachDrop then does its note path): a drop point decides by what is
  // under it; no point (Insert attachment) = the active leaf
  function claim(at) {
    if (!app) return null;
    if (at && Number.isFinite(at.x) && Number.isFinite(at.y)) {
      const k = window.devicePixelRatio || 1;
      return boardAt(document.elementFromPoint(at.x / k, at.y / k));
    }
    return activeBoard();
  }
  function pointOf(c, at) {
    if (at && Number.isFinite(at.x) && Number.isFinite(at.y)) { const k = window.devicePixelRatio || 1; return c.toCanvas(at.x / k, at.y / k); }
    const r = c.wrapperEl.getBoundingClientRect();
    if (ptr && ptr.w === c.wrapperEl && ptr.x >= r.left && ptr.x <= r.right && ptr.y >= r.top && ptr.y <= r.bottom) return c.toCanvas(ptr.x, ptr.y);
    return c.toCanvas(r.left + r.width / 2, r.top + r.height / 2);
  }

  /* place: one file card per path, the grid's top-left at canvas point p; ONE history step */
  function place(c, paths, p) {
    const ns = paths.map(f => c.createFileNode({ file: f, pos: { x: p.x, y: p.y }, size: sizeOf(f), save: false }));
    if (ns.length > 1 && window.CanvasArrange) {
      c.deselectAll();
      ns.forEach((n, i) => c.select(n, i > 0));
      window.CanvasArrange.run(c, "arrange-grid");
    } else if (ns.length) c.select(ns[0], false);
    if (typeof c.renderEdges === "function") c.renderEdges();
    c.requestSave();                    // the step (cv-history commits once per turn)
    return ns;
  }

  /* ---------------------------------------------- goal cvf-drop: everything dragged from the file explorer
     The explorer's mouse machine (main.js) offers a drag over a plugin view as dragover/drop whose payload is
     app.dragManager.draggable: {type:"file", file}, {type:"files", files} (a Shift+click selection) or
     {type:"folder", file: TFolder}. This file takes all three over a board: canvas-menus' wrapper drop handler hands every
     drop to drop() below (one handler per drop; coordinator grant 2026-10-26).
     Measured on stock 1.13.7 (docs/recon-cv2/A, black-box):
       one file      -> one card whose TOP-LEFT is the drop point; 400 wide; a note / PDF / anything else 400 high,
                        an image or a video as high as its aspect makes it (320x200 -> 400x250), audio 42 high
       a selection   -> one 400x400 card per item, left to right at a 445 pitch (400 + 45), one y, the first at P
       a folder      -> the same row of 400x400 cards for every file under it, recursively (its own files first,
                        then each subfolder, by name); never a card for the folder itself
     New cards go to the front of nodes[] (right after any groups that lead it: a group stays below what is
     dropped on it) and a group under the drop point is left alone (no auto-grow). One synchronous placement after
     the media sizes are known = one cv-history step. More than CAP files adds nothing and says so. */
  const AUD = new Set(["mp3", "wav", "m4a", "ogg", "3gp", "flac"]);
  const VID = new Set(["mp4", "webm", "ogv", "mov", "mkv"]);
  const PITCH = 445, CARD = 400, AUDIO_H = 42;
  const MP4_SCAN = 1 << 20, MP4_BOXES = 64;   // video header walk: first 1 MB, at most 64 boxes (ruling 5)
  const mediaUrl = p => "opensidian-img://localhost/" + String(p).split("/").map(encodeURIComponent).join("/");
  function payload() {
    const dm = app && app.dragManager, d = dm && dm.draggable;
    if (!d) return null;
    const isFile = f => f && typeof f.path === "string" && !Array.isArray(f.children) && (!X || !X.TFile || f instanceof X.TFile);
    if (d.type === "file" && isFile(d.file)) return { kind: "file", paths: [d.file.path] };
    if (d.type === "files" && Array.isArray(d.files)) { const ps = d.files.filter(isFile).map(f => f.path); return ps.length ? { kind: "files", paths: ps } : null; }
    if (d.type === "folder" && d.file && Array.isArray(d.file.children)) {
      const out = [], walk = f => {
        const kids = [...f.children].sort((a, b) => String(a.name || a.path).localeCompare(String(b.name || b.path)));
        for (const k of kids) if (isFile(k)) out.push(k.path);
        if (CV_DROP_FOLDER_STOCK) for (const k of kids) if (k && Array.isArray(k.children)) walk(k);
      };
      walk(d.file);
      if (!CV_DROP_FOLDER_STOCK) out.sort();          // develop: direct files, by path
      return { kind: "folder", paths: out, name: d.file.path || "/" };
    }
    return null;
  }
  function probe(p, ms) {   // natural size of an image / video in the vault -> {w, h} or null
    const ext = extOf(p);
    return new Promise(res => {
      let t = 0, el = null;
      const done = v => { clearTimeout(t); if (el) { el.onload = el.onerror = el.onloadedmetadata = null; if (el.tagName === "VIDEO") { el.removeAttribute("src"); try { el.load(); } catch (_) {} } } res(v); };
      t = setTimeout(() => done(null), ms);
      if (IMG.has(ext)) {
        el = new Image();
        el.onload = () => done(el.naturalWidth > 0 && el.naturalHeight > 0 ? { w: el.naturalWidth, h: el.naturalHeight } : null);
        el.onerror = () => done(null);
        el.src = mediaUrl(p);
      } else if (VID.has(ext)) {
        el = document.createElement("video"); el.preload = "metadata"; el.muted = true;
        el.onloadedmetadata = () => done(el.videoWidth > 0 && el.videoHeight > 0 ? { w: el.videoWidth, h: el.videoHeight } : null);
        el.onerror = () => done(null);
        el.src = mediaUrl(p);
      } else done(null);
    });
  }
  /* an MP4 / MOV declares its picture size in the first video track header (ISO BMFF moov/trak/tkhd: width and
     height, 16.16 fixed point); read from the vault's bytes, no decode. -> {w, h} or null */
  let vdiag = "";                       // census: how the last video got (or missed) its size
  async function mp4Dims(p) {
    let b;
    try { b = await adapterBytes(p); } catch (e) { vdiag += "|tkhd:read-" + String(e && e.message || e).slice(0, 40); return null; }
    if (!b) { vdiag += "|tkhd:nofile"; return null; }
    if (!(b instanceof ArrayBuffer) && b.buffer instanceof ArrayBuffer) b = b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
    if (!(b instanceof ArrayBuffer)) { vdiag += "|tkhd:notbytes"; return null; }
    // untrusted input (coordinator ruling 5): only the first MP4_SCAN bytes and MP4_BOXES boxes are looked at, every box
    // size and offset is checked against the scanned length before it is read, anything malformed -> null (4:3 card)
    const n = Math.min(b.byteLength, MP4_SCAN), v = new DataView(b, 0, n);
    vdiag += "|tkhd:n=" + b.byteLength;
    let boxes = 0;
    const walk = (s, e, depth) => {
      for (let o = s; o + 8 <= e;) {
        if (++boxes > MP4_BOXES) return null;
        let sz = v.getUint32(o), hd = 8;
        const ty = String.fromCharCode(v.getUint8(o + 4), v.getUint8(o + 5), v.getUint8(o + 6), v.getUint8(o + 7));
        if (sz === 1) { if (o + 16 > e) return null; sz = v.getUint32(o + 8) * 4294967296 + v.getUint32(o + 12); hd = 16; }
        else if (sz === 0) sz = e - o;
        if (!(sz >= hd) || o + sz > e) return null;
        if (ty === "tkhd") {
          if (o + hd + 1 > o + sz) return null;
          const ver = v.getUint8(o + hd), wo = o + hd + (ver === 1 ? 88 : 76);
          if (wo + 8 <= o + sz) {
            const w = v.getUint32(wo) / 65536, h = v.getUint32(wo + 4) / 65536;
            if (w >= 1 && h >= 1 && w <= 16384 && h <= 16384) return { w, h };
          }
        } else if (depth < 4 && (ty === "moov" || ty === "trak")) { const r = walk(o + hd, o + sz, depth + 1); if (r) return r; }
        o += sz;
      }
      return null;
    };
    let r = null;
    try { r = walk(0, n, 0); } catch (e) { vdiag += "|tkhd:bad"; r = null; }
    const top = [];
    for (let o = 0, k = 0; o + 8 <= n && k < 6; k++) { const sz = v.getUint32(o); top.push(String.fromCharCode(v.getUint8(o + 4), v.getUint8(o + 5), v.getUint8(o + 6), v.getUint8(o + 7)) + sz); if (sz < 8) break; o += sz; }
    vdiag += r ? "|tkhd=" + r.w + "x" + r.h : "|tkhd:none:" + top.join(",");
    return r;
  }
  async function adapterBytes(p) {
    const f = app && app.vault && app.vault.getAbstractFileByPath ? app.vault.getAbstractFileByPath(p) : null;
    return f ? await app.vault.readBinary(f) : null;
  }
  async function oneSize(p) {
    const ext = extOf(p);
    if (AUD.has(ext)) return { width: CARD, height: AUDIO_H };
    if (IMG.has(ext) || VID.has(ext)) {
      let s = await probe(p, 2000);
      vdiag = s ? "probe" : "probe:none";
      if (!s && VID.has(ext)) { try { s = await mp4Dims(p); } catch (e) { vdiag += "|tkhd:" + String(e && e.message || e).slice(0, 40); s = null; } }   // no decoder for it (no metadata event): the size the file itself declares
      if (s) return { width: CARD, height: Math.max(1, Math.round(CARD * s.h / s.w)) };
      if (VID.has(ext)) return { width: CARD, height: 300 };   // size unreadable (no decoder; readBinary refused in a restricted vault): develop's 4:3 video card
    }
    return { width: CARD, height: CARD };
  }
  // the new nodes move to the front of the board order, behind only the groups that already lead it
  function toFront(c, ns) {
    const ids = new Set(ns.map(n => n.id)), all = [...c.nodes].filter(([k]) => !ids.has(k));
    let g = 0; while (g < all.length && all[g][1].data && all[g][1].data.type === "group") g++;
    c.nodes.clear();
    for (const [k, v] of all.slice(0, g)) c.nodes.set(k, v);
    for (const n of ns) c.nodes.set(n.id, n);
    for (const [k, v] of all.slice(g)) c.nodes.set(k, v);
  }
  function placeRow(c, paths, p, size) {
    const sz = size || { width: CARD, height: CARD };
    // an item is a vault path, or {file, subpath} (goal cv-dragin: a heading / block bookmark keeps its JSON Canvas subpath)
    const ns = paths.map((f, i) => {
      const it = typeof f === "string" ? { file: f } : f;
      return c.createFileNode({ file: it.file, subpath: it.subpath || undefined, pos: { x: p.x + i * PITCH, y: p.y }, size: sz, save: false });
    });
    toFront(c, ns);
    c.deselectAll();
    ns.forEach((n, i) => c.select(n, i > 0));
    if (typeof c.renderEdges === "function") c.renderEdges();
    c.requestSave();                    // the step (cv-history commits once per turn)
    return ns;
  }
  // the canvas point under the middle of the board's viewport
  function viewMid(c) { const r = c.wrapperEl.getBoundingClientRect(); return c.toCanvas(r.left + r.width / 2, r.top + r.height / 2); }
  // a pasted picture's card size from its own bytes (no URL load: the bytes are not in the vault yet)
  async function bufSize(type, buf) {
    try {
      const bm = await createImageBitmap(new Blob([buf], { type }));
      const w = bm.width, h = bm.height; if (bm.close) bm.close();
      if (w > 0 && h > 0) return { width: CARD, height: Math.max(1, Math.round(CARD * h / w)) };
    } catch (_) {}
    return { width: CARD, height: CARD };
  }
  function onDragOver(e) {
    if (!payload()) return;
    const c = boardAt(e.target); if (!c || ro(c)) return;
    e.preventDefault();
    try { e.dataTransfer.dropEffect = "copy"; } catch (_) {}
  }
  // canvas-menus' wrapper drop handler -> here; true = taken (the rest runs async)
  function drop(c, e) {
    const d = payload(); if (!d || !c || ro(c)) return false;
    e.preventDefault(); e.stopPropagation();
    place2(c, d, c.toCanvas(e.clientX, e.clientY));
    return true;
  }
  // the point the first card's top-left goes to: the drop point itself (stock), or the point less half the card (flag off)
  function anchor(p, sz) { return CV_DROP_TOPLEFT ? p : { x: p.x - sz.width / 2, y: p.y - sz.height / 2 }; }
  async function place2(c, d, p) {
    if (d.kind === "folder") {
      if (d.paths.length > CAP) {
        last.fold = d.name + "|n=0|cap=" + CAP;
        say(c, "Canvas: " + d.name + " has " + d.paths.length + " files, over the limit of " + CAP + " files per folder drop");
        return ping(c);
      }
      const ns = CV_DROP_FOLDER_STOCK ? placeRow(c, d.paths, anchor(p, { width: CARD, height: CARD })) : place(c, d.paths, p);
      last.fold = d.name + "|n=" + ns.length + "|cap=" + CAP;
      return ping(c);
    }
    if (d.kind === "files") { placeRow(c, d.paths, anchor(p, { width: CARD, height: CARD })); last.ex = "files|n=" + d.paths.length; return ping(c); }
    const size = await oneSize(d.paths[0]);
    if (!c.wrapperEl.isConnected || ro(c)) return;
    placeRow(c, d.paths, anchor(p, size), size);
    last.ex = "file|" + size.width + "x" + size.height + (vdiag ? "|" + vdiag : ""); vdiag = "";
    ping(c);
  }
  /* ---------------------------------------------- goal cv-dragin: a bookmark dragged from the bookmarks pane
     The pane's mouse machine (main.js bmDragStart, R24.6 — no HTML5 drag events, no drag payload object) asks boardUnder() every
     frame its pointer is outside the pane; over an editable board its decision is [bmdrop:cv], and the release
     calls dropBookmark() here directly. The pane's rows carry only {kind, depth, name}, so the entry itself is read
     from the vault's own .obsidian/bookmarks.json (READ ONLY: the file's bytes are never written by a drop) and
     the painted row is found by the backend's own paint rule (main.rs bm_node_of: a "file" whose path ends in .md
     and a "group" with a string title and an items array are painted, in pre-order; anything else is carried
     unpainted). Per bookmark:
       file             -> one file card, placed exactly as an explorer file drop (anchor(), CV_DROP_TOPLEFT)
       heading / block  -> the same card with JSON Canvas "subpath" (stock keeps "#Heading" / "#^block" on the
                           bookmark; a "heading" / "block" typed entry is read the same way)
       group            -> one card per file bookmark inside it, recursively in pane order, one row at the 445
                           pitch like a folder drop; more than CAP files adds nothing and says so
       search / url / anything else -> refused: no card (inside a group: skipped and counted)
     One synchronous placeRow = one cv-history step; the new cards end selected.
     Census: [cvdropbm:<kind>|n=<cards added>|ref=<refused entries>|cap=<cap>] */
  function boardUnder(x, y) {
    if (!app || !Number.isFinite(x) || !Number.isFinite(y)) return null;
    const c = boardAt(document.elementFromPoint(x, y));
    return c && !ro(c) ? c : null;
  }
  const isPathNode = o => !!o && typeof o === "object" && typeof o.path === "string" && o.path !== "" &&
    (o.type === "file" || o.type === "heading" || o.type === "block");
  // issue #44 (PR #57 review): main.rs bm_node_of also paints a "file" whose path is a PDF (len > 4, ext pdf any case)
  // as a row — it must count here too, or every row index after a PDF bookmark points one entry too far
  const isPdfPath = p => typeof p === "string" && p.length > 4 && /\.pdf$/i.test(p);
  const painted = o => !!o && typeof o === "object" && (
    (o.type === "file" && typeof o.path === "string" && ((o.path.endsWith(".md") && o.path.length > 3) || isPdfPath(o.path))) ||
    (o.type === "group" && typeof o.title === "string" && Array.isArray(o.items)));
  function rawAt(items, ix) {           // the raw entry painted as row ix (pre-order over painted entries only)
    let seen = 0, hit = null;
    const walk = arr => {
      for (const o of arr) {
        if (!painted(o)) continue;
        if (seen++ === ix) { hit = o; return true; }
        if (o.type === "group" && walk(o.items)) return true;
      }
      return false;
    };
    walk(items);
    return hit;
  }
  const subOf = o => (typeof o.subpath === "string" && o.subpath ? o.subpath : "");
  const itemOf = o => { const s = subOf(o); return s ? { file: o.path, subpath: s } : { file: o.path }; };
  async function rawBookmarks() {
    try {
      const a = app && app.vault && app.vault.adapter, dir = (app.vault.configDir || ".obsidian");
      if (!a || typeof a.read !== "function") return null;
      const j = JSON.parse(await a.read(dir + "/bookmarks.json"));
      return j && Array.isArray(j.items) ? j.items : null;
    } catch (_) { return null; }
  }
  // row = the painted row {kind, name} (main.js bmTree[ix]); without a readable file the row alone decides (no subpath)
  async function dropBookmark(c, ix, row, x, y) {
    if (!c || ro(c) || !row) return false;
    const p = c.toCanvas(x, y);          // the point is taken at the release, before any await
    const items = await rawBookmarks();
    let o = items ? rawAt(items, ix) : null;
    const rk = row.kind === "g" ? "group" : "file";
    const rp = row.pdf ? row.name : row.name + ".md";   // issue #44: a PDF row's name IS its vault path; row.files are vault paths
    if (o && (o.type !== rk || (rk === "file" ? o.path !== rp : o.title !== row.name))) o = null;   // the file moved under us: trust the row
    if (!o) o = rk === "group" ? { type: "group", title: row.name, items: (row.files || []).map(p => ({ type: "file", path: p })) } : { type: "file", path: rp };
    const label = String(o.title || o.path || row.name);
    let kind, out = [], ref = 0;
    if (o.type === "group") {
      kind = "group";
      const walk = arr => { for (const k of arr) { if (k && k.type === "group" && Array.isArray(k.items)) walk(k.items); else if (isPathNode(k)) out.push(itemOf(k)); else ref++; } };
      walk(o.items);
    } else if (isPathNode(o)) {
      const s = subOf(o);
      kind = !s ? "file" : s.startsWith("#^") ? "block" : "heading";
      out = [itemOf(o)];
    } else { kind = String(o.type || "unknown").replace(/[^A-Za-z0-9_-]/g, "_").slice(0, 24); ref = 1; }
    const tag = kind + "|n=";
    if (!c.wrapperEl.isConnected || ro(c)) { last.bm = tag + "0|ref=" + ref + "|cap=" + CAP; return ping(c); }
    if (!out.length) {
      last.bm = tag + "0|ref=" + ref + "|cap=" + CAP;
      say(c, "Canvas: the bookmark " + label + " has no file to add as a card");
      return ping(c);
    }
    if (out.length > CAP) {
      last.bm = tag + "0|ref=" + ref + "|cap=" + CAP;
      say(c, "Canvas: " + label + " has " + out.length + " files, over the limit of " + CAP + " files per bookmark group drop");
      return ping(c);
    }
    if (kind === "group") placeRow(c, out, anchor(p, { width: CARD, height: CARD }));
    else {
      const size = await oneSize(out[0].file);
      if (!c.wrapperEl.isConnected || ro(c)) { last.bm = tag + "0|ref=" + ref + "|cap=" + CAP; return ping(c); }
      placeRow(c, out, anchor(p, size), size);
    }
    last.bm = tag + out.length + "|ref=" + ref + "|cap=" + CAP;
    ping(c);
    return true;
  }
  function say(c, msg) {
    last.msg = msg;
    try { if (X && X.Notice) new X.Notice(msg); } catch (_) {}
  }

  /* ---------------------------------------------- CV-101: files from other applications */
  // host = {inv, refresh, say, update}: main.js's attach_files invoke, tree refresh, drop banner, census repaint
  // attach_files names a requester (it only checks the name stays inside the vault): the board's own path
  function noteOf(c) { const v = boards().find(b => b.canvas === c); return (v && v.path) || c.path || "canvas"; }
  async function attach(c, paths, at, host) {
    const p = pointOf(c, at);
    let a;
    try { a = await host.inv("attach_files", { note: noteOf(c), paths }); }
    catch (e) { last.att = "err"; last.msg = String((e && e.message) || e); host.say(last.msg); return host.update(); }
    if (a.copied.length) await host.refresh();
    if (a.copied.length && !ro(c)) place(c, a.copied, p);
    last.att = a.copied.length + "/" + a.refused.length;
    const msg = a.refused.concat(a.kept || []).join("\n");
    if (msg) last.msg = msg;
    host.say(msg);
    host.update();
  }

  /* ---------------------------------------------- goal cv-journeys J10: an image pasted on the board (Ctrl+V)
     WebKit runs the Paste editing command only into an editable selection, and a board is not editable: a Ctrl+V the
     board did not take (cv-history pastes its own copied cards and cancels the key) moves focus, for the duration of
     that one key, into a hidden contenteditable catcher. The paste lands there as a ClipboardEvent; an image file in
     its clipboardData is written NEW into the attachment folder (core canvas_attach: "Pasted image <stamp>.<ext>", the
     folder the file manager names) and becomes one file card at the last pointer position on the board (else its centre).
     Anything that is not an image is left alone (the catcher is emptied); focus goes back where it was. */
  const PASTE_EXT = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp" };
  const inEd = t => !!(t && t.closest && t.closest("textarea, input, .markdown-source-view, [contenteditable=true], [contenteditable=\"\"]"));
  let catcher = null, pasteFor = null;
  function catcherEl() {
    if (catcher && catcher.isConnected) return catcher;
    catcher = document.createElement("div");
    catcher.className = "cvdrop-paste-catcher";
    catcher.contentEditable = "true";
    catcher.setAttribute("aria-hidden", "true");
    catcher.tabIndex = -1;
    catcher.style.cssText = "position:fixed;left:-10000px;top:0;width:1px;height:1px;overflow:hidden;opacity:0";
    catcher.addEventListener("paste", onPaste);
    document.body.appendChild(catcher);
    return catcher;
  }
  function giveBack() {
    const p = pasteFor; pasteFor = null;
    if (catcher) catcher.textContent = "";
    if (p && p.back && p.back.isConnected && document.activeElement === catcher) try { p.back.focus({ preventScroll: true }); } catch (_) {}
  }
  /* goal cvf-drop: the OS clipboard wins (stock, CVR-A-035/036, C-057/058/059). Ctrl+V on a board: cv-history's key entry asks
     pasteKey() first (coordinator grant 2026-10-26): text -> a 250x60 text card centred on the view, opened for editing (the literal
     text: "[[Beta]]" or a vault path stays text); an http(s) URL alone -> a 400x400 link card centred on the view; an
     image -> "Pasted image <stamp>.png" (below). A card copy (Ctrl+C on the board, cv-history) is ALSO written to the
     OS clipboard as its JSON, so a paste of exactly that text is our own copy and goes back to cv-history's paste;
     a copy the OS clipboard refused leaves Ctrl+V to cv-history as before. */
  let copied = null, copyFailed = false;
  function mirror(c) {   // cv-history's Ctrl+C entry calls this after its own copy (coordinator grant)
    if (!app || !c || !c.selection || !c.selection.size) return;
    const ids = new Set([...c.selection].map(n => n.id));
    const txt = JSON.stringify({ nodes: [...c.selection].map(n => n.data), edges: [...c.edges.values()].filter(x => ids.has(x.fromNode) && ids.has(x.toNode)) });
    const k = catcherEl(), back = document.activeElement;
    let ok = false;
    try {
      k.textContent = txt; k.focus({ preventScroll: true });
      const r = document.createRange(); r.selectNodeContents(k); const s = getSelection(); s.removeAllRanges(); s.addRange(r);
      ok = document.execCommand("copy");
      s.removeAllRanges();
    } catch (_) { ok = false; }
    k.textContent = "";
    if (back && back !== k && back.isConnected) try { back.focus({ preventScroll: true }); } catch (_) {}
    copied = ok ? txt : null; copyFailed = !ok;
    last.paste = "copy|" + (ok ? "os" : "internal-only");
  }
  function internalPaste(c) {   // our own card copy: cv-history pastes it, CanvasSelect centres it on the view in the same task (one undo step, CVR-A-119/C-055)
    try {
      const before = new Set(c.nodes.keys()), S = window.CanvasSelect;
      if (!(window.CanvasHistory && window.CanvasHistory.paste(c))) return false;
      if (S && typeof S.centrePaste === "function") S.centrePaste(c, before);
      return true;
    } catch (_) { return false; }
  }
  function linkClass(c) {   // canvas.js exposes its node classes read-only on the board (coordinator grant 2026-10-26)
    return (c && c.NODE_CLASS && c.NODE_CLASS.link) || null;
  }
  function textPaste(c, tx) {
    if (copied != null && (tx === copied || tx.trim() === copied)) { last.paste += "|internal"; internalPaste(c); return ping(c); }
    const m = viewMid(c), u = tx.trim();
    if (/^https?:\/\/\S+$/i.test(u)) {
      const L = linkClass(c);
      if (L) {
        const d = { id: newIdOf(c), type: "link", url: u, x: Math.round(m.x - CARD / 2), y: Math.round(m.y - CARD / 2), width: CARD, height: CARD };
        const n = c.addNode(new L(c, d));
        c.deselectAll(); c.select(n, false);
        if (typeof c.renderEdges === "function") c.renderEdges();
        c.requestSave();
        last.paste += "|link";
        return ping(c);
      }
      last.paste += "|link=no-class";
    }
    c.createTextNode({ text: tx, pos: { x: m.x - 125, y: m.y - 30 }, focus: true });
    last.paste += "|text:" + tx.length;
    ping(c);
  }
  function newIdOf(c) {
    const taken = c.ids();
    for (;;) { const b = new Uint8Array(8); crypto.getRandomValues(b); const id = Array.from(b, x => x.toString(16).padStart(2, "0")).join(""); if (!taken.has(id)) return id; }
  }
  /* Ctrl+V on board c, from cv-history's key entry (the wrapper or a card has focus) or from onPasteKey below (body
     has focus: a click on the empty board focuses nothing). true = armed: the key's default action must run (no
     preventDefault), the paste lands in the catcher. false = the last card copy never reached the OS clipboard:
     cv-history pastes it (old road). */
  function pasteKey(c) {
    if (!app || !c || ro(c) || copyFailed) return false;
    pasteFor = { c, back: document.activeElement };
    const k = catcherEl();
    k.textContent = "";
    k.focus({ preventScroll: true });
    try { const r = document.createRange(); r.selectNodeContents(k); const s = getSelection(); s.removeAllRanges(); s.addRange(r); } catch (_) {}
    last.paste = "key|armed";
    setTimeout(() => {   // the paste (if any) ran inside this key's default action
      if (pasteFor && pasteFor.wantImg) return imgPaste(pasteFor);   // the engine put the image INTO the catcher (below)
      if (pasteFor && last.paste === "key|armed") { last.paste = "key|no-paste-event"; internalPaste(c); ping(c); }
      giveBack();
    }, 0);
    return true;
  }
  function onPasteKey(e) {   // only a key cv-history cannot see: focus on body, a board up
    if (e.defaultPrevented || !(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey || String(e.key).toLowerCase() !== "v") return;
    if (!app) { last.paste = "key|no-app"; return; }
    if (e.target !== document.body && e.target !== document.documentElement) return;
    const c = activeBoard();
    if (!c || ro(c)) { last.paste = "key|no-board"; return ping(c); }
    if (!pasteKey(c)) { internalPaste(c); ping(c); }
  }
  function b64(buf) {
    const u = new Uint8Array(buf); let s = "";
    for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode.apply(null, u.subarray(i, i + 0x8000));
    return btoa(s);
  }
  const stampNow = () => { const d = new Date(), z = n => String(n).padStart(2, "0"); return d.getFullYear() + z(d.getMonth() + 1) + z(d.getDate()) + z(d.getHours()) + z(d.getMinutes()) + z(d.getSeconds()); };
  /* WebKitGTK hands a clipboard IMAGE to the page with an empty clipboardData (no types, no items) and pastes it as
     an <img> (blob: / data: URL) into the editable target instead. So a catcher paste with no file in clipboardData
     is let through, and the image the engine inserted is read back out of the catcher (bytes from a data: URL, a
     blob: fetch, else re-encoded through a 2D canvas as PNG). Census [cvpaste:...|img=<scheme or 0>]. */
  function catcherImgs() { return catcher ? [...catcher.querySelectorAll("img")].map(i => i) : []; }
  function waitImg(ms) {
    return new Promise(res => {
      const k = catcher; if (!k) return res([]);
      const got = () => !!(k.querySelector("img") || (k.textContent || "").length);   // goal cvf-drop: pasted TEXT lands here too (WebKitGTK hands the page an empty clipboardData)
      if (got()) return res(catcherImgs());
      let t = 0;
      const mo = new MutationObserver(() => { if (got()) done(); });
      const done = () => { mo.disconnect(); clearTimeout(t); res(catcherImgs()); };
      mo.observe(k, { childList: true, subtree: true, characterData: true });
      t = setTimeout(done, ms);
    });
  }
  function dataBytes(src) {   // data:<type>;base64,<b64> -> { type, buf }
    const m = /^data:([^;,]*)(;base64)?,(.*)$/s.exec(src); if (!m || !m[2]) return null;
    const s = atob(m[3]), u = new Uint8Array(s.length);
    for (let i = 0; i < s.length; i++) u[i] = s.charCodeAt(i);
    return { type: m[1], buf: u.buffer };
  }
  async function imgBytes(img) {
    const src = String(img.currentSrc || img.src || "");
    if (src.startsWith("data:")) { const d = dataBytes(src); if (d && PASTE_EXT[d.type]) return d; }
    if (src.startsWith("blob:")) {
      try { const b = await (await fetch(src)).blob(); if (PASTE_EXT[b.type]) return { type: b.type, buf: await b.arrayBuffer() }; } catch (_) {}
    }
    try {   // any loadable image: re-encode (the pixels are what was copied)
      if (!img.complete || !img.naturalWidth) await new Promise((ok, no) => { img.onload = ok; img.onerror = no; setTimeout(no, 1500); });
      const cv = document.createElement("canvas"); cv.width = img.naturalWidth; cv.height = img.naturalHeight;
      cv.getContext("2d").drawImage(img, 0, 0);
      const b = await new Promise(ok => cv.toBlob(ok, "image/png"));
      if (b) return { type: "image/png", buf: await b.arrayBuffer() };
    } catch (_) {}
    return null;
  }
  /* the async Clipboard API, asked while the paste gesture is live: an image ClipboardItem is a Blob, no URL load (the
     CSP has no blob: in img-src / connect-src, so the engine's blob: <img> neither loads nor fetches).
     -> { type, buf }, else a reason string for the census */
  function acbRead() {
    let p;
    try { p = navigator.clipboard && typeof navigator.clipboard.read === "function" ? navigator.clipboard.read() : null; } catch (e) { return Promise.resolve("throw:" + (e && e.name)); }
    if (!p) return Promise.resolve("none");
    return p.then(async items => {
      for (const it of items || []) for (const t of it.types || []) if (PASTE_EXT[t]) { const b = await it.getType(t); return { type: t, buf: await b.arrayBuffer() }; }
      return "noimg:" + (items || []).map(it => (it.types || []).join("+")).join(",");
    }, e => "err:" + (e && e.name));
  }
  function acbText() {
    try { return navigator.clipboard && typeof navigator.clipboard.readText === "function" ? navigator.clipboard.readText().then(t => String(t || ""), () => "") : null; } catch (_) { return null; }
  }
  async function imgPaste(pf) {
    let a = null;
    try { a = await Promise.race([pf.acb || Promise.resolve("none"), new Promise(r => setTimeout(() => r("timeout"), 2000))]); } catch (e) { a = "err:" + (e && e.name); }
    if (a && typeof a === "object") {
      giveBack();
      last.paste += "|acb|" + a.type + ":" + a.buf.byteLength;
      const c0 = pf && pf.c;
      if (!c0 || ro(c0)) return ping(c0);
      return savePaste(c0, a.type, a.buf);
    }
    last.paste += "|acb=" + String(a).slice(0, 40);
    const imgs = await waitImg(1500);
    let ktx = !imgs.length && catcher ? String(catcher.innerText || catcher.textContent || "") : "";
    if (!imgs.length && !ktx && pf.acbt) {   // the async Clipboard API's text, asked inside the gesture (onPaste)
      let t = ""; try { t = await Promise.race([pf.acbt, new Promise(r => setTimeout(() => r(""), 1000))]); } catch (_) {}
      if (t) { ktx = t; last.paste += "|acbt"; }
    }
    if (ktx) { giveBack(); const c1 = pf && pf.c; last.paste += "|ktext"; if (!c1 || ro(c1)) return ping(c1); return textPaste(c1, ktx); }
    const src = imgs.length ? String(imgs[0].currentSrc || imgs[0].src || "") : "";
    last.paste += "|img=" + (imgs.length ? (src.split(":")[0] || "?") : "0");
    let d = null;
    try { if (imgs.length) d = await imgBytes(imgs[0]); } catch (_) {}
    giveBack();
    const c = pf && pf.c;
    if (!d && !imgs.length && c && !ro(c) && internalPaste(c)) return ping(c);   // nothing on the OS clipboard: our own card copy, if any
    if (!d || !c || ro(c)) { if (imgs.length && !d) last.paste += "|unreadable"; return ping(c); }
    last.paste += "|" + d.type + ":" + d.buf.byteLength;
    return savePaste(c, d.type, d.buf);
  }
  async function onPaste(e) {
    const pf = pasteFor;
    const onCatcher = !!(catcher && e.target && (e.target === catcher || catcher.contains(e.target)));
    e.stopPropagation();
    const cd = e.clipboardData;
    const types = cd ? [...(cd.types || [])] : [];
    let file = null;
    try {
      for (const it of cd ? [...(cd.items || [])] : []) if (it.kind === "file" && PASTE_EXT[it.type]) { file = it.getAsFile(); if (file) break; }
      if (!file) for (const f of cd ? [...(cd.files || [])] : []) if (PASTE_EXT[f.type]) { file = f; break; }
    } catch (_) {}
    last.paste = (file ? file.type + ":" + file.size : "none") + "|types=" + types.join(",") + "|t=" + (onCatcher ? "catcher" : "doc");
    // WebKitGTK may list no types for an X selection offered only as UTF8_STRING, yet still answer getData("text/plain")
    let tx0 = ""; if (!file) try { tx0 = cd ? String(cd.getData("text/plain") || "") : ""; } catch (_) {}
    if (tx0) last.paste += "|gd:" + tx0.length;
    if (!file && !tx0 && onCatcher && pf && types.every(t => t !== "text/plain" && t !== "text/html")) { pf.wantImg = true; pf.acb = acbRead(); pf.acbt = acbText(); return; }   // the engine inserts it (imgPaste); the async Clipboard API is asked inside the gesture
    e.preventDefault();
    giveBack();
    const c = pf && pf.c;
    if (!file && c && !ro(c) && tx0) return textPaste(c, tx0);
    if (!file || !c || ro(c)) return ping(c);
    return savePaste(c, file.type, await file.arrayBuffer());
  }
  /* the write is the core command canvas_attach (goal cvf-export: vault-confined, create-new only, EEXIST when taken):
     "Pasted image <stamp>.<ext>" in the attachment folder the file manager names (vault root by default), then
     "<stem> 1.<ext>", "<stem> 2.<ext>" ... on EEXIST without trusting the model (a restricted vault may not see it) */
  async function attachPath(c, name) {
    try { const fm = app && app.fileManager; if (fm && typeof fm.getAvailablePathForAttachment === "function") { const p = await fm.getAvailablePathForAttachment(name, noteOf(c)); if (typeof p === "string" && p) return p; } } catch (_) {}
    return name;
  }
  async function savePaste(c, type, buf) {
    const inv = invG(); if (!inv) { last.paste += "|err=inv"; return ping(c); }
    const ext = PASTE_EXT[type], data = b64(buf);
    const sz = await bufSize(type, buf);   // goal cvf-drop: 400 wide, as high as the picture's aspect (320x200 -> 400x250)
    let path = await attachPath(c, "Pasted image " + stampNow() + "." + ext), rel = null;
    const stem = path.slice(0, -(ext.length + 1));
    for (let i = 0; rel == null; i++) {
      try { rel = await inv("canvas_attach", { path, dataB64: data }); }
      catch (err) {
        const m = String((err && err.message) || err);
        if (!/^EEXIST/.test(m) || i >= 200) { last.paste += "|err"; say(c, "Canvas: the pasted image was not saved — " + m); return ping(c); }
        path = stem + " " + (i + 1) + "." + ext;
      }
    }
    if (typeof rel !== "string" || !rel) rel = path;
    try { if (typeof refreshTree === "function") await refreshTree(); } catch (_) {}   // main.js: the explorer row + the file model (vaultSync)
    if (!ro(c)) { const m = viewMid(c); placeRow(c, [rel], { x: m.x - sz.width / 2, y: m.y - sz.height / 2 }, sz); }   // centred on the VIEW (stock), not the pointer
    last.paste += "|" + rel;
    ping(c);
  }
  const invG = () => { try { return typeof inv === "function" ? inv : null; } catch (_) { return null; } };   // main.js (global lexical)
  function ping(c) { try { if (typeof updateTitle === "function") updateTitle(); } catch (_) {} }
  document.addEventListener("keydown", onPasteKey);
  // the paste lands where the engine puts it: the catcher when the focus move took, else the old focus (body / the
  // board wrapper) — a paste outside any editor while a board is up is the board's
  document.addEventListener("paste", e => {
    if (!app || e.defaultPrevented || (catcher && e.target === catcher) || inEd(e.target)) return;
    const top = e.target === document.body || e.target === document.documentElement;
    const c = (pasteFor && pasteFor.c) || boardAt(e.target) || (top ? activeBoard() : null);
    if (!c) return;
    if (!pasteFor) pasteFor = { c, back: null };
    onPaste(e);
  });

  document.addEventListener("dragover", onDragOver);   // the drop itself: canvas-menus' wrapper onDrop hands it to drop() (coordinator grant)
  document.addEventListener("pointermove", e => {
    const w = e.target && e.target.closest ? e.target.closest(".canvas-wrapper") : null;
    if (w) ptr = { x: e.clientX, y: e.clientY, w };
  }, true);

  function census() {
    const esc = s => String(s).replace(/[[\]|@\n]/g, "_");
    let t = "";
    if (last.att) t += " [cvdrop:" + last.att + "]";
    if (last.ex) t += " [cvdropx:" + esc(last.ex) + "]";
    if (last.fold) t += " [cvdropf:" + last.fold.replace(/[[\]@\n]/g, "_") + "]";
    if (last.bm) t += " [cvdropbm:" + last.bm + "]";
    if (last.paste) t += " [cvpaste:" + esc(last.paste.slice(0, 120)) + "]";
    if (last.msg) t += " [cvdropmsg:" + esc(last.msg.slice(0, 120)) + "]";
    return t;
  }

  const api = { claim, attach, census, CAP, drop, pasteKey, mirror, boardUnder, dropBookmark, TOPLEFT: CV_DROP_TOPLEFT, FOLDER_STOCK: CV_DROP_FOLDER_STOCK };
  Object.defineProperty(window, "CanvasDrop", { configurable: true, enumerable: false, writable: false, value: Object.freeze(api) });
})();
