// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* canvas files (goal cv-files): every kind of file sits on the board. Written from the
   public help pages (help.obsidian.md/plugins/canvas) and the JSON Canvas 1.0 spec
   (jsoncanvas.org/spec/1.0, MIT) — clean room.

     PDF file card      the app's ONE pdf frame builder (editor.js Ed.pdfFill) inside the card;
                        a PDF pdf_info refuses (over pdf.rs MAX_PDF_BYTES, broken) stays the
                        "other" card canvas-cards.js paints
     canvas file card   the shapes-only preview cv-vault paints for ![[X.canvas]] (main.js
                        cvEmbedPaint: one rect per node, one path per edge) — read from the file,
                        so a card on its own canvas is depth 1, never a recursion; a double press
                        opens the canvas in a canvas tab
     note file card     (goal cvf-filecards) the note painted as the reading view (X.MarkdownRenderer), a
                        heading / block subpath narrowing it; the label is the note name (a subpath a
                        breadcrumb), any other file its full name; cards of a note repaint when it changes
     text card editing  the note editor (app.embedRegistry md embed -> showEditor(), a CM6
                        EmbedEditor), created on edit and destroyed on stop: one editor at a time
     link card menu     "Copy URL" / "Change URL" added to cv-menus' card menu through its
                        canvas:node-menu event (no second contextmenu listener)
     refused canvas     a file refused ONLY for edges naming a missing card offers "Open read-only":
                        the valid part paints with the board read-only and the view still broken,
                        so nothing is ever written (canvas.js save() refuses a broken view)

   Load order (index.html): ... canvas.js -> canvas-*.js -> canvas-files.js -> main.js. This file
   runs no code at load beyond one stylesheet, one capture pointerdown listener (a double press on a
   text card opens its editor) and one frozen global, window.__osCanvasFiles, whose
   hooks canvas.js calls (FileNode.renderContent, TextNode.startEditing/stopEditing, the parse-refusal
   path of CanvasView.loadPath) and whose census(view) main.js canvasTok() appends. */
(function () {
  "use strict";
  const ST = { app: null, X: null, prompt: null, copyN: 0, copyOk: 0, hooked: null, vhooked: null };
  const mk = (tag, cls, parent) => { const e = document.createElement(tag); if (cls) e.className = cls; if (parent) parent.appendChild(e); return e; };
  const isObj = v => v !== null && typeof v === "object" && !Array.isArray(v);
  const extOf = p => { const n = String(p || "").split("/").pop(); const d = n.lastIndexOf("."); return d > 0 ? n.slice(d + 1).toLowerCase() : ""; };
  const ping = ctx => { const d = ctx && ctx.deps; if (d && typeof d.changed === "function") try { d.changed(); } catch (_) {} };
  const G = name => { try { return typeof window[name] === "function" ? window[name] : null; } catch (_) { return null; } };
  const EdG = () => { try { return typeof Ed !== "undefined" ? Ed : null; } catch (_) { return null; } };      // editor.js (global lexical)
  const invG = () => { try { return typeof inv === "function" ? inv : null; } catch (_) { return null; } };   // main.js (global lexical)

  const css = mk("style");
  css.textContent =
    ".canvas-node-container > .canvas-file-pdf { display: flex; flex-direction: column; width: 100%; box-sizing: border-box; overflow: hidden;" +
    " user-select: none; white-space: normal }\n" +
    ".canvas-node-container > .canvas-file-canvas { display: block; width: 100%; height: 100%; box-sizing: border-box; padding: 6px }\n" +
    ".canvas-node-container > .cvf-text-editor { width: 100%; height: 100%; box-sizing: border-box; padding: 4px 8px }\n" +
    ".cvf-text-editor .cm-editor { height: 100%; outline: none }\n" +
    ".canvas-file-embed > .cvf-note { padding: 0 12px; white-space: normal; word-break: break-word }\n" +
    ".canvas-error .cvf-ro-btn { margin-top: 10px; cursor: pointer }\n" +
    ".canvas-error.cvf-ro { position: absolute; left: 8px; top: 8px; margin: 0; padding: 6px 10px; z-index: 3; max-width: 60%;" +
    " background: var(--background-primary, #1e1e1e) }\n" +
    ".canvas-error.cvf-ro .canvas-error-message { font-size: 12px }\n";
  (document.head || document.documentElement).appendChild(css);

  function remember(ctx) {
    if (ctx && ctx.app) ST.app = ctx.app;
    if (ctx && ctx.X) ST.X = ctx.X;
    hookMenus();
    hookVault();
  }

  // a note changed on disk (an inline edit in another card, the note's own tab): every card of it repaints
  function hookVault() {
    const app = ST.app;
    if (!app || !app.vault || typeof app.vault.on !== "function" || ST.vhooked === app) return;
    ST.vhooked = app;
    app.vault.on("modify", f => {
      const path = f && f.path;
      if (!path) return;
      let leaves = [];
      try { leaves = app.workspace.getLeavesOfType("canvas"); } catch (_) { leaves = []; }
      for (const l of leaves) {
        const c = l.view && l.view.canvas;
        if (c) for (const n of c.nodes.values()) if (n.data.type === "file" && n.data.file === path && n._cvf && n._cvf.kind === "note" && !n.isEditing) try { n.renderContent(); } catch (e) { console.error(e); }
      }
    });
    /* a card's file vanished or moved away (an external rm -rf of its folder, a delete, a rename): every card
       naming that path, or a path under that folder, repaints, as in stock — the card turns into the missing-file
       card without reopening the board. Not on "create": the missing card's own Create button repaints its node
       (canvas-media.js create), and a second repaint mid-flow would cancel its editor focus. A card the user is
       editing is left alone. */
    const repaintUnder = p => {
      if (!p) return;
      const pre = p + "/";
      let leaves = [];
      try { leaves = app.workspace.getLeavesOfType("canvas"); } catch (_) { leaves = []; }
      for (const l of leaves) {
        const c = l.view && l.view.canvas;
        if (c) for (const n of c.nodes.values()) {
          const fp = n.data.type === "file" ? String(n.data.file || "") : "";
          if (fp && (fp === p || fp.startsWith(pre)) && !n.isEditing) try { n.renderContent(); } catch (e) { console.error(e); }
        }
      }
    };
    app.vault.on("delete", f => repaintUnder(f && f.path));
    // PR #33 review: a rename also repaints the cards at its destination, so a missing-file card naming that path
    // turns into the file (the model has the new path when "rename" fires). No Create-button flow renames
    app.vault.on("rename", (f, old) => { repaintUnder(typeof old === "string" ? old : ""); if (f && f.path !== old) repaintUnder(f.path); });
  }

  /* ------------------------------------------------ file cards */
  // true = painted here; false = not ours (canvas-cards.js / canvas.js paint it)
  function file(node, ctx) {
    remember(ctx);
    if (window.CanvasMedia && !ctx._cvm && window.CanvasMedia.file(node, ctx, file)) return true;   // goal cvf-media: media / missing / canvas / PDF toolbar (canvas-media.js)
    const p = String(node.data.file || ""), ext = ctx.file && ctx.file.extension ? String(ctx.file.extension).toLowerCase() : extOf(p);
    node._cvfSeq = (node._cvfSeq || 0) + 1;
    node._cvf = null; node._cvfWhy = ""; node._cvfErr = "";
    if (p) label(node, p, ext);
    const MR = ctx.X && ctx.X.MarkdownRenderer;
    if (ext === "md" && ctx.file && ctx.app && ctx.app.vault && MR && typeof MR.render === "function") return note(node, ctx, p);
    if (ext === "pdf" && EdG() && invG()) return pdf(node, ctx, p);
    if (ext === "canvas" && ctx.file && G("cvEmbedPaint")) return nested(node, ctx, p);
    if (ext === "pdf" || ext === "canvas") node._cvfWhy = (ext === "pdf" ? (EdG() ? "" : "ed") + (invG() ? "" : "inv") : (ctx.file ? "" : "file") + (G("cvEmbedPaint") ? "" : "paint")) || "?";   // census: why the card fell through
    return false;
  }


  /* ------------------------------------------------ note file cards (goal cvf-filecards)
     A note card paints the note the way the reading view does (X.MarkdownRenderer, the renderer text
     cards use), not its source. A subpath narrows it first: "#H" (or "#H#Sub") keeps that heading's
     section up to the next heading of the same or a higher level; "#^id" keeps the block that carries
     the id (its paragraph, or the list item / line), the marker itself dropped. */
  const reEsc = s => String(s).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const HD = /^ {0,3}(#{1,6})[ \t]+(.*?)[ \t#]*$/;
  function section(src, sub) {
    const parts = String(sub || "").split("#").map(s => s.trim()).filter(Boolean);
    if (!parts.length) return src;
    const lines = String(src).split("\n");
    if (parts[0].startsWith("^")) {
      const id = parts[0].slice(1), re = new RegExp("(^|\\s)\\^" + reEsc(id) + "\\s*$");
      const i = lines.findIndex(l => re.test(l));
      if (i < 0) return null;
      if (lines[i].trim() === "^" + id) {            // the id on its own line names the block above it
        let b = i - 1; while (b >= 0 && !lines[b].trim()) b--;
        if (b < 0) return null;
        let a = b; while (a > 0 && lines[a - 1].trim() && !HD.test(lines[a - 1])) a--;
        return lines.slice(a, b + 1).join("\n");
      }
      if (/^\s*([-*+]|\d+[.)])\s/.test(lines[i]) || HD.test(lines[i])) return lines[i].replace(re, "");
      let a = i, b = i;
      while (a > 0 && lines[a - 1].trim() && !HD.test(lines[a - 1])) a--;
      while (b < lines.length - 1 && lines[b + 1].trim() && !HD.test(lines[b + 1])) b++;
      const blk = lines.slice(a, b + 1); blk[i - a] = blk[i - a].replace(re, "");
      return blk.join("\n");
    }
    let lo = 0, hi = lines.length, fence = false;
    const heads = [];
    lines.forEach((l, k) => { if (/^\s*(```|~~~)/.test(l)) fence = !fence; else if (!fence) { const m = HD.exec(l); if (m) heads.push({ k, lv: m[1].length, t: m[2].trim() }); } });
    const norm = s => s.replace(/\s+/g, " ").toLowerCase();
    for (const p of parts) {
      const h = heads.find(x => x.k >= lo && x.k < hi && norm(x.t) === norm(p));
      if (!h) return null;
      const nx = heads.find(x => x.k > h.k && x.lv <= h.lv);
      lo = h.k; hi = Math.min(hi, nx ? nx.k : lines.length);
    }
    return lines.slice(lo, hi).join("\n").replace(/\s+$/, "");
  }
  const stripFm = t => /^---\r?\n/.test(t) ? t.replace(/^---\r?\n[\s\S]*?\r?\n---[ \t]*(\r?\n|$)/, "") : t;

  // the label above a file card: a note is its name without .md (a subpath a breadcrumb), any other file its full name
  function label(node, p, ext) {
    const nm = p.split("/").pop();
    let t = ext === "md" ? nm.replace(/\.md$/i, "") : nm;
    const parts = String(node.data.subpath || "").split("#").map(s => s.trim()).filter(Boolean);
    if (parts.length) t += " › " + parts.join(" › ");
    if (node.labelEl.textContent !== t) node.labelEl.textContent = t;
  }

  /* The card keeps the note embed (app.embedRegistry md, node.child) that the board API and the
     inline editor work through (canvas.js FileNode: isEditable / startEditing / writeBack read
     child.text and child.data); only its painter is ours: loadFile reads the note, narrows it to the
     subpath, sets child.data / child.text the way the embed does, and paints the reading view. */
  function note(node, ctx, p) {
    const seq = node._cvfSeq, app = ctx.app, f = ctx.file, MR = ctx.X.MarkdownRenderer;
    const cc = window.__osCanvasCards;               // its wiring (wheel scroll of a selected card, census refresh); false for a note
    if (cc) try { cc.file(node, ctx); } catch (_) {}
    node._cc = { kind: "note" };
    node._cvf = { kind: "note", out: null, md: "none" };
    noteWire(node, ctx);
    const reg = app.embedRegistry;
    const mdc = reg && reg.embedByExtension && Object.prototype.hasOwnProperty.call(reg.embedByExtension, "md") ? reg.embedByExtension.md : null;
    const host = mk("div", "canvas-file-embed", node.contentEl);
    let child = null;
    if (typeof mdc === "function") {
      try { child = mdc({ app, containerEl: host, linktext: p, sourcePath: (node.canvas && node.canvas.path) || "", depth: 1, showInline: true }, f, node.data.subpath || ""); }
      catch (e) { console.error("[canvas] note card embed:", e); child = null; }
    }
    const paint = () => Promise.resolve().then(() => app.vault.cachedRead(f)).then(txt => {
      if (node._cvfSeq !== seq || node._gone) return;
      txt = String(txt);
      const sub = node.data.subpath || "";
      const src = sub ? section(txt, sub) : stripFm(txt);
      if (child) { child.data = txt; child.text = sub ? (src == null ? "" : src) : txt; if ("subpathNotFound" in child) child.subpathNotFound = src == null; }
      if (node.isEditing) return;
      for (const o of [...host.querySelectorAll(":scope > .markdown-embed-content, :scope > .canvas-node-placeholder, :scope > .markdown-embed-link")]) o.remove();
      if (src == null) {
        mk("div", "canvas-node-placeholder", host).textContent = "Unable to find “" + sub.replace(/^#/, "") + "” in " + p.split("/").pop().replace(/\.md$/i, "");
        node._cvf.out = null; node._cvf.md = "none"; ping(ctx); return;
      }
      const out = mk("div", "markdown-embed-content markdown-rendered cvf-note");
      return Promise.resolve(MR.render(app, src, out, p, child || node)).then(() => {
        if (node._cvfSeq !== seq || node._gone || node.isEditing) return;
        for (const o of [...host.querySelectorAll(":scope > .markdown-embed-content")]) o.remove();
        host.appendChild(out);
        node._cvf.out = out; node._cvf.md = "rendered";
        ping(ctx);
      });
    }).catch(e => {
      console.error("[canvas] note card:", e);
      if (node._cvfSeq !== seq || node._gone || node.isEditing) return;
      host.textContent = ""; mk("div", "canvas-node-placeholder", host).textContent = p + " — could not be read";
      node._cvf.md = "none"; node._cvfErr = String(e && e.message || e).slice(0, 60); ping(ctx);
    });
    if (child) {
      Object.defineProperty(child, "editor", { configurable: true, get() { return typeof node.editorApi === "function" ? node.editorApi() : null; } });
      child.showPreview = () => node.stopEditing();
      child.loadFile = paint;                        // canvas.js reloads the embed after an edit: it repaints here
      node.child = child;
      node.addChild(child);
    }
    paint();
    return true;
  }

  /* CVR-B-037: while exactly one note card is selected its note is the active file — the explorer highlights it and the
     status bar counts its backlinks (main.js setCanvasActiveFile); any other selection hands the active file back to
     the board. Settled once per task: select() clears, then adds. */
  let actT = null, actOn = false;
  function actSoon(c) {
    if (actT) return;
    actT = Promise.resolve().then(() => {
      actT = null;
      const set = G("setCanvasActiveFile");
      if (!set) return;
      const s = c && c.selection ? [...c.selection] : [];
      const n = s.length === 1 ? s[0] : null;
      const p = n && !n._gone && n._cvf && n._cvf.kind === "note" && n.data && typeof n.data.file === "string" ? n.data.file : null;
      if (!p && !actOn) return;
      actOn = !!p;
      try { set(p); } catch (e) { console.error("[canvas] active file:", e); }
    });
  }

  // once per note card: the inline editor takes the focus (caret at the double-click point), a link in the
  // selected card opens its target, the label opens the note (double click: this tab, Ctrl/Cmd-click: a new tab)
  function noteWire(node, ctx) {
    node._cvfCtx = ctx;
    if (node._cvfNW) return;
    node._cvfNW = true;
    const sel = () => !!(node.canvas && node.canvas.selection && node.canvas.selection.has(node));
    const isNote = () => node._cvf && node._cvf.kind === "note";
    const src = () => (node.canvas && node.canvas.path) || "";
    const lt = () => String(node.data.file || "") + (node.data.subpath ? (String(node.data.subpath).startsWith("#") ? "" : "#") + node.data.subpath : "");
    const open = how => { const app = node._cvfCtx && node._cvfCtx.app; if (app && app.workspace) app.workspace.openLinkText(lt(), src(), how).catch(e => console.error("[canvas] open note:", e)); };
    node._cvfOpen = open;                            // the board-level double press (below) opens the note from the label
    if (typeof node.setSelected === "function") {    // CVR-B-037: a lone selected note card is the active file (main.js seam)
      const bs = node.setSelected;
      node.setSelected = function () { const r = bs.apply(this, arguments); actSoon(this.canvas); return r; };
      const bd = node.detach;                        // a removed card leaves the selection without setSelected
      if (typeof bd === "function") node.detach = function () { const c = this.canvas, r = bd.apply(this, arguments); actSoon(c); return r; };
    }
    if (typeof node.startEditing === "function") {
      const base = node.startEditing;
      node.startEditing = function () {
        const was = this.isEditing;
        const r = base.apply(this, arguments);
        if (!isNote() || was || !this.isEditing) return r;
        const at = node._cvfDbl; node._cvfDbl = null;
        setTimeout(() => {
          const em = this.child && this.child.editMode, cm = em && em.cm;
          if (!cm || !this.isEditing) return;
          try {
            cm.focus();
            let pos = at && typeof cm.posAtCoords === "function" ? cm.posAtCoords({ x: at[0], y: at[1] }) : null;
            if (pos == null) pos = cm.state.doc.length;
            cm.dispatch({ selection: { anchor: pos } });
          } catch (_) {}
          ping(node._cvfCtx);
        }, 0);
        return r;
      };
    }
    const box = node.containerEl;
    box.addEventListener("focusout", e => {          // the focus left the card (the board, another card): the edit ends
      if (!isNote() || !node.isEditing || (e.relatedTarget && box.contains(e.relatedTarget))) return;
      setTimeout(() => { if (node.isEditing && !box.contains(document.activeElement)) try { node.stopEditing(); } catch (err) { console.error(err); } }, 0);
    });
    box.addEventListener("keydown", e => { if (isNote() && node.isEditing && e.key === "Escape") { e.preventDefault(); e.stopPropagation(); node.stopEditing(); try { node.canvas.wrapperEl.focus({ preventScroll: true }); } catch (_) {} } });
    box.addEventListener("dblclick", e => { if (isNote() && !node.isEditing && node.contentEl.contains(e.target) || e.target === node.blockerEl) node._cvfDbl = [e.clientX, e.clientY]; }, true);
    box.addEventListener("pointerdown", e => {
      if (!isNote() || node.isEditing || e.button !== 0 || !sel()) return;
      const o = node._cvf.out;
      const hit = o && document.elementsFromPoint(e.clientX, e.clientY).map(x => x.closest && x.closest("a.wiki, a.internal-link")).find(a => a && o.contains(a));
      if (!hit) return;
      e.stopPropagation(); e.preventDefault();
      const t = (hit.dataset.note || hit.getAttribute("data-href") || hit.textContent || "") + (hit.dataset.anchor ? "#" + hit.dataset.anchor : "");
      const app = node._cvfCtx && node._cvfCtx.app;
      if (t && app && app.workspace) app.workspace.openLinkText(t, String(node.data.file || ""), e.ctrlKey || e.metaKey ? "tab" : false).catch(err => console.error("[canvas] open link:", err));
    });
    const lab = node.labelEl;
    lab.addEventListener("pointerdown", e => {
      if (!isNote() || e.button !== 0 || !(e.ctrlKey || e.metaKey)) return;
      e.stopPropagation(); e.preventDefault();
      open("tab");
    });
    lab.addEventListener("dblclick", e => {
      if (!isNote()) return;
      e.stopPropagation(); e.preventDefault();
      open(false);
    });
  }

  function pdf(node, ctx, p) {
    const E = EdG(), seq = node._cvfSeq, host = node.contentEl;
    const enc = typeof E.pctEnc === "function" ? E.pctEnc(p) : p;
    node._cc = { kind: "pdf" };
    node._cvf = { kind: "pdf", s: null };
    Promise.resolve().then(() => invG()("pdf_info", { path: enc })).catch(e => { node._cvfErr = String(e && e.message || e).slice(0, 60); return null; }).then(info => {
      if (node._cvfSeq !== seq || node._gone) return;
      host.textContent = "";
      if (!info || !info.pages) {                // over the cap / not a PDF: the "other" card (open with the default app)
        node._cvfWhy = info ? "pages" : "info";
        node._cvf = { kind: "other" };
        const cc = window.__osCanvasCards;
        if (cc) cc.file(node, ctx); else mk("div", "canvas-node-placeholder", host).textContent = p;
        ping(ctx);
        return;
      }
      const s = mk("span", "internal-embed pdf-embed canvas-file-pdf", host);
      s.dataset.pdf = enc; s.dataset.page = "1"; s.dataset.height = ""; s.dataset.alt = p.split("/").pop();
      s.contentEditable = "false";
      node._cvf.s = s;
      new MutationObserver(() => ping(ctx)).observe(s, { attributes: true, attributeFilter: ["class", "data-of"] });
      E.pdfFill(s);
      ping(ctx);
    });
    return true;
  }

  function nested(node, ctx, p) {
    const host = node.contentEl, seq = node._cvfSeq;
    node._cc = { kind: "canvas" };
    const s = mk("span", "cv-embed canvas-file-canvas", host);
    s.dataset.canvas = p; s.title = p;
    const link = mk("a"); link.textContent = p;    // cvEmbedPaint's fallback (an unreadable canvas keeps a plain link)
    node._cvf = { kind: "canvas", s };
    Promise.resolve(G("cvEmbedPaint")(s, link)).catch(e => console.error("[canvas] canvas card:", e)).then(() => {
      if (node._cvfSeq !== seq || node._gone) return;
      if (!s.isConnected) { host.textContent = ""; mk("div", "canvas-node-placeholder", host).textContent = p + " — could not be read"; }
      ping(ctx);
    });
    wireOpen(node, ctx);
    return true;
  }

  // a double press on a canvas card opens it (the board captures the pointer: detect it on the card's own press)
  function wireOpen(node, ctx) {
    node._cvfCtx = ctx;
    if (node._cvfOpenWired) return;
    node._cvfOpenWired = true;
    let last = 0, px = 0, py = 0;
    const open = () => {
      const p = String(node.data.file || ""), oc = G("openCanvas");
      if (oc) Promise.resolve(oc(p)).catch(e => console.error("[canvas] open canvas:", e));
      else if (ST.app) ST.app.workspace.getLeaf("tab").setViewState({ type: "canvas", state: { file: p }, active: true }).catch(e => console.error(e));
    };
    node.containerEl.addEventListener("pointerdown", e => {
      if (!node._cvf || node._cvf.kind !== "canvas" || node.isEditing || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey) return;
      if (e.target.classList && e.target.classList.contains("canvas-node-resizer")) return;
      const now = Date.now(), dbl = now - last < 500 && Math.abs(e.clientX - px) + Math.abs(e.clientY - py) < 8;
      last = now; px = e.clientX; py = e.clientY;
      if (!dbl) return;                             // a single press still selects / drags the card
      last = 0;
      e.stopPropagation(); e.preventDefault();
      open();
    });
  }

  /* ------------------------------------------------ text card editing (CM6) */
  function startEditing(node, ctx) {
    remember(ctx);
    const app = ctx.app, reg = app && app.embedRegistry;
    const mdc = reg && reg.embedByExtension && Object.prototype.hasOwnProperty.call(reg.embedByExtension, "md") ? reg.embedByExtension.md : null;
    if (typeof mdc !== "function" || !node.canvas || node.canvas.readonly) return false;
    // one editor at a time on the board
    for (const n of node.canvas.nodes.values()) if (n !== node && n.isEditing) try { n.stopEditing(); } catch (_) {}
    const host = mk("div", "cvf-text-editor");
    let child, em;
    try {
      child = mdc({ app, containerEl: host, linktext: "", sourcePath: node.canvas.path || "", depth: 1 }, null, "");
      node.addChild(child);
      em = child.showEditor();
      if (!em || !em.cm) throw new Error("no editor");
    } catch (e) {
      console.error("[canvas] text card editor:", e);
      if (child) try { node.removeChild(child); } catch (_) {}
      return false;                                 // canvas.js falls back to its textarea
    }
    node.isEditing = true;
    node.containerEl.classList.add("is-editing");
    node.contentEl.textContent = "";
    node.contentEl.appendChild(host);
    em.set(node.data.text, true);
    node._cvfEd = { child, em, host };
    em.onUpdate = (u, docChanged) => {
      if (!docChanged || node._cvfEd === null) return;
      const v = em.get();
      if (v !== node.data.text) { node.data.text = v; node.canvas.requestSave(); }
    };
    host.addEventListener("keydown", e => {
      if (e.key === "Escape") { e.preventDefault(); node.stopEditing(); }
      e.stopPropagation();
    });
    for (const ev of ["dblclick", "wheel", "keyup"]) host.addEventListener(ev, e => e.stopPropagation(), { passive: true });
    host.addEventListener("focusout", e => {
      if (e.relatedTarget && host.contains(e.relatedTarget)) return;
      setTimeout(() => { const st = node._cvfEd; if (st && st.host === host && !host.contains(document.activeElement)) node.stopEditing(); }, 0);
    });
    setTimeout(() => {
      try { em.cm.focus(); const L = em.cm.state.doc.length; em.cm.dispatch({ selection: { anchor: L } }); } catch (_) {}
      ping(ctx);
    }, 0);
    ping(ctx);
    return true;
  }

  function stopEditing(node, ctx) {
    const st = node._cvfEd;
    if (!st) return false;
    node._cvfEd = null;
    let v = node.data.text;
    try { v = st.em.get(); } catch (_) {}
    const hadFocus = st.host.contains(document.activeElement);
    node.isEditing = false;
    node.containerEl.classList.remove("is-editing");
    try { node.removeChild(st.child); } catch (e) { console.error(e); }   // the embed unloads: its CM6 view is destroyed
    st.host.remove();
    const ch = v !== node.data.text;
    node.data.text = v;
    node.renderContent();
    if (ch) node.canvas.requestSave();
    if (hadFocus && node.canvas && node.canvas.wrapperEl) try { node.canvas.wrapperEl.focus({ preventScroll: true }); } catch (_) {}
    ping(ctx);
    return true;
  }

  /* The board takes pointer capture on every press (canvas.js), so the dblclick of a double press
     lands on the wrapper, where nodeOf() finds no card and a NEW text card is made (canvas-cards.js
     documents the same for other-kind files). A double press on a text card is therefore detected
     on its second pointerdown, before the board sees it, and opens that card's editor. */
  /* The same holds for a NOTE file card: a double press on its body edits the note inline (caret at
     the press point), a double press on its label opens the note in this tab. The dblclick that
     follows such a press is kept from the board, which would otherwise make a text card. */
  let dpId = "", dpT = 0, dpX = 0, dpY = 0, dpLab = false, dpEat = 0;
  const nodeFor = (card, id) => {
    const app = ST.app || (window.__osPluginHost && window.__osPluginHost.app) || null;
    try {
      for (const l of app ? app.workspace.getLeavesOfType("canvas") : []) {
        const v = l.view;
        if (v && v.canvas && v.containerEl && v.containerEl.contains(card)) return v.canvas.nodes.get(id) || null;
      }
    } catch (_) {}
    return null;
  };
  document.addEventListener("pointerdown", e => {
    const t = e.target, card = t && t.closest ? t.closest(".canvas-node[data-node-id]") : null;
    const text = !!(card && card.classList.contains("canvas-node-text")), lab = !!(card && t.closest(".canvas-node-label"));
    if (!card || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) { dpId = ""; return; }
    if (t.closest("textarea, input, .markdown-source-view, [contenteditable=true], .canvas-node-resizer") || (text && lab)) { dpId = ""; return; }
    const id = card.dataset.nodeId, now = Date.now();
    const dbl = id === dpId && lab === dpLab && now - dpT < 500 && Math.abs(e.clientX - dpX) + Math.abs(e.clientY - dpY) < 8;
    dpId = id; dpT = now; dpX = e.clientX; dpY = e.clientY; dpLab = lab;
    if (!dbl) return;                               // the first press still selects / drags the card
    dpId = "";
    if (!text && !card.classList.contains("canvas-node-file")) return;
    const node = nodeFor(card, id);
    if (!node || node.isEditing || !node.canvas) return;
    if (!text) {                                    // a file card: only a note card answers here (other kinds: canvas-cards.js)
      if (!node._cvf || node._cvf.kind !== "note") return;
      const o = node._cvf.out;                      // a second press on a link of the (now selected) card opens the link
      if (!lab && o && document.elementsFromPoint(e.clientX, e.clientY).some(x => x.closest && o.contains(x) && x.closest("a.wiki, a.internal-link"))) return;
      e.stopPropagation(); e.preventDefault();
      dpEat = now;
      if (lab) { if (typeof node._cvfOpen === "function") node._cvfOpen(false); return; }
      if (node.canvas.readonly || typeof node.startEditing !== "function" || !node.isEditable || !node.isEditable()) return;
      node._cvfDbl = [e.clientX, e.clientY];
      node.startEditing();
      return;
    }
    if (node.canvas.readonly || typeof node.startEditing !== "function") return;
    e.stopPropagation(); e.preventDefault();
    node.startEditing();
  }, true);
  document.addEventListener("dblclick", e => {
    if (!dpEat || Date.now() - dpEat > 700) return;
    dpEat = 0;
    if (e.target && e.target.closest && e.target.closest(".canvas-wrapper, .canvas-node, .canvas-node-container")) e.stopPropagation();
  }, true);

  /* ------------------------------------------------ link card menu */
  function hookMenus() {
    const app = ST.app;
    if (!app || !app.workspace || typeof app.workspace.on !== "function" || ST.hooked === app) return;
    ST.hooked = app;
    app.workspace.on("canvas:node-menu", (menu, node) => {
      if (!menu || !node || !node.data || node.data.type !== "link" || (node.canvas && node.canvas.readonly)) return;
      menu.addItem(i => i.setTitle("Copy URL").onClick(() => { copyText(String(node.data.url || "")); refocus(node); }));
      menu.addItem(i => i.setTitle("Change URL").onClick(() => changeUrl(node)));
    });
  }

  // no programmatic clipboard in this tree: a copy command inside the menu click, its copy event carries the URL
  function copyText(t) {
    ST.copyN++;
    let ok = false;
    const back = document.activeElement;           // the copy textarea borrows focus; give it back (Escape, keys keep working)
    const ta = mk("textarea");
    ta.value = t; ta.setAttribute("readonly", "");
    ta.style.cssText = "position:fixed;left:-10000px;top:0;width:10px;height:10px;opacity:0";
    document.body.appendChild(ta);
    const on = e => { try { e.clipboardData.setData("text/plain", t); e.preventDefault(); ok = true; } catch (_) {} };
    document.addEventListener("copy", on, true);
    try { ta.focus(); ta.select(); ok = document.execCommand("copy") || ok; } catch (_) {}
    document.removeEventListener("copy", on, true);
    ta.remove();
    if (back && back !== document.body && back.isConnected && typeof back.focus === "function") try { back.focus({ preventScroll: true }); } catch (_) {}
    if (!ok && navigator.clipboard && typeof navigator.clipboard.writeText === "function") navigator.clipboard.writeText(t).then(() => { ST.copyOk = 1; }, () => {});
    ST.copyOk = ok ? 1 : 0;
  }

  function changeUrl(node) {
    const X = ST.X, app = ST.app;
    if (!X || !app || typeof X.Modal !== "function") return;
    const m = new X.Modal(app);
    m.modalEl.classList.add("cvf-url-prompt");
    m.setTitle("Change URL");
    const input = mk("input", "", m.contentEl);
    input.type = "text"; input.spellcheck = false; input.value = String(node.data.url || "");
    const later = () => setTimeout(() => ping(node._ccCtx || node._cvfCtx || null) || bump(), 0);
    input.addEventListener("input", later);
    input.addEventListener("keydown", e => {
      e.stopPropagation();
      if (e.key === "Enter") {
        e.preventDefault();
        const v = String(input.value || "").trim();
        m.close();
        if (!/^https?:\/\/[^\s/]+/i.test(v)) { notify("Canvas: not a web address — " + v); return; }
        if (v !== node.data.url && node.canvas && node.canvas.nodes.get(node.id) === node && !node.canvas.readonly) {
          node.data.url = v; node.renderContent(); node.canvas.requestSave();
        }
      } else if (e.key === "Escape") { e.preventDefault(); m.close(); }
    });
    m.onClose = () => { if (ST.prompt && ST.prompt.m === m) ST.prompt = null; refocus(node); later(); };
    ST.prompt = { kind: "url", m, input };
    m.open();
    setTimeout(() => { try { input.focus(); input.select(); } catch (_) {} later(); }, 0);
  }
  // the board keeps the keyboard after a menu action / prompt (Escape, Delete, undo keys reach it)
  const refocus = node => { const w = node && node.canvas && node.canvas.wrapperEl; if (w) setTimeout(() => { if (w.isConnected && (!document.activeElement || document.activeElement === document.body)) try { w.focus({ preventScroll: true }); } catch (_) {} }, 0); };
  const bump = () => { const u = G("updateTitle"); if (u) try { u(); } catch (_) {} };
  const notify = msg => { try { if (ST.X && ST.X.Notice) new ST.X.Notice(msg); } catch (_) {} };

  /* ------------------------------------------------ refused canvas: read-only open */
  // called by canvas.js right after setError() for a file that did not parse; offers the button
  // only when the sole problem is edges naming a missing card
  function refused(view, text, ctx) {
    remember(ctx);
    view._cvfRO = null;
    let doc;
    try { doc = JSON.parse(text); } catch (_) { return false; }
    if (!isObj(doc) || !Array.isArray(doc.nodes) || ("edges" in doc && !Array.isArray(doc.edges))) return false;
    const ids = new Set(doc.nodes.filter(isObj).map(n => n.id));
    const edges = doc.edges || [];
    const keep = edges.filter(e => isObj(e) && ids.has(e.fromNode) && ids.has(e.toNode));
    const skip = edges.length - keep.length;
    if (!skip) return false;
    const clean = Object.assign({}, doc, { edges: keep });
    try { ctx.model.validateCanvas(clean); } catch (_) { return false; }
    const box = view.errorEl, p = view.path;
    if (!box) return false;
    const b = mk("button", "cvf-ro-btn", box);
    b.type = "button"; b.textContent = "Open read-only";
    b.title = "Show the valid part of this canvas. Nothing is written to the file.";
    b.addEventListener("click", e => { e.preventDefault(); e.stopPropagation(); openReadOnly(view, clean, skip, p, ctx); });
    ping(ctx);
    return true;
  }

  function openReadOnly(view, doc, skip, p, ctx) {
    if (view.path !== p || !view.broken || !view.errorEl) return;
    const c = view.canvas;
    view.dirty = false;
    c.readonly = true;                               // no edit takes; view.broken stays set: save() never writes
    c.wrapperEl.hidden = false;
    c.importData(doc);
    view._cvfRO = { path: p, skip };
    const box = view.errorEl;
    box.classList.add("cvf-ro");
    const b = box.querySelector(".cvf-ro-btn"); if (b) b.remove();
    const msg = box.querySelector(".canvas-error-message");
    if (msg) msg.textContent = "Read-only: " + skip + " edge" + (skip === 1 ? "" : "s") + " naming a missing card " + (skip === 1 ? "is" : "are") + " not shown. The file is never written.";
    c.fitted = false;
    requestAnimationFrame(() => { if (view.path === p && c.wrapperEl.getBoundingClientRect().width) { c.fitted = true; c.zoomToFit(); } ping(ctx); });
    ping(ctx);
  }

  /* ------------------------------------------------ rename: file cards follow the note (CV-143)
     main.js runUpdateLinks0 (the CONSENTED half of R34) calls this once. Note names carry no
     extension there; a file card names the vault path. Open canvas views save first (a pending
     edit is not lost), the Rust op rewrites only the matching "file" tokens, and every open view
     on a rewritten canvas reloads from disk (a stale view would otherwise save the old path back). */
  async function relink(old, nn, env) {
    const inv = env && env.inv, app = (env && env.app) || ST.app;
    if (typeof inv !== "function" || !old || !nn) return [];
    const md = s => (/\.[^/.]+$/.test(s) ? s : s + ".md");
    const from = md(String(old)), to = md(String(nn));
    if (from === to) return [];
    let leaves = [];
    try { leaves = app && app.workspace ? app.workspace.getLeavesOfType("canvas") : []; } catch (_) { leaves = []; }
    for (const l of leaves) if (l.view && typeof l.view.flush === "function") try { await l.view.flush(); } catch (e) { console.error("[canvas] relink flush:", e); }
    const wrote = await inv("canvas_io", { op: "relink", path: "", data: JSON.stringify({ from, to }) });
    const set = new Set(Array.isArray(wrote) ? wrote : []);
    for (const l of leaves) {
      const v = l.view;
      if (v && v.path && set.has(v.path) && !v.broken && typeof v.loadPath === "function") try { await v.loadPath(v.path); } catch (e) { console.error("[canvas] relink reload:", e); }
    }
    return [...set];
  }

  /* ------------------------------------------------ census (main.js canvasTok)
     [cvcard:<id>|kind=pdf|pages=<n>|frame=0|1|loaded=0|1]  [cvfile:<id>|rects=<n>|paths=<n>]  [cvfile:<id>|ext=<text>]
     [cvfile:cm=<CM6 editors on the board>|ta=<textareas>|ed=<editing card id or ->]  [cvfile:robtn@x,y]
     [cvfile:ro|n=|e=|skip=]  [cvfprompt:url|<value>]  [cvfcopy:n=|ok=]  [cvfcover:x,y] */
  function census(v) {
    if (!v || !v.canvas) return "";
    if (v.app && !ST.app) { ST.app = v.app; hookMenus(); }
    const esc = x => String(x == null ? "" : x).replace(/[[\]|@]/g, "_").replace(/\s+/g, " ").trim();
    const vis = e => !!e && e.isConnected && e.getClientRects().length > 0;
    const ctr = e => { const r = e.getBoundingClientRect(); return Math.round(r.x + r.width / 2) + "," + Math.round(r.y + r.height / 2); };
    let t = "", k = 0, ed = "-";
    const lbl = [];
    for (const n of v.canvas.nodes.values()) {
      if (n.data.type === "text" && n.isEditing && ed === "-") ed = esc(n.id);
      if (n.data.type === "file" && lbl.length < 24 && vis(n.containerEl) && vis(n.labelEl)) lbl.push(esc(n.id) + "=" + esc(n.labelEl.textContent) + "@" + ctr(n.labelEl));
      if (n.data.type !== "file" || k >= 16 || !vis(n.containerEl)) continue;
      const f = n._cvf || {}, id = esc(n.id);
      if (f.kind === "note") {                       // goal cvf-filecards: [cvfn:<id>|md=|h=|src=|ed=|cm=|fo=|lk=|lt=|tx=]
        k++;
        const o = f.out && n.contentEl.contains(f.out) ? f.out : null;
        const hs = o ? [...o.querySelectorAll("h1,h2,h3,h4,h5,h6")].map(h => esc(h.textContent).replace(/,/g, " ")).join(",") : "";
        let src = 0;
        if (o) { const w = document.createTreeWalker(o, NodeFilter.SHOW_TEXT); for (let x = w.nextNode(); x; x = w.nextNode()) if (/(^|\n)\s*#{1,6} |\[\[|\]\]/.test(x.nodeValue)) src++; }
        const a = o && o.querySelector("a.wiki, a.internal-link");
        const lt = a ? (a.dataset.note || a.getAttribute("data-href") || a.textContent || "") + (a.dataset.anchor ? "#" + a.dataset.anchor : "") : "";
        t += " [cvfn:" + id + "|md=" + (o ? f.md : f.md === "rendered" ? "none" : f.md) + "|h=" + hs + "|src=" + src + "|ed=" + (n.isEditing ? 1 : 0) +
             "|cm=" + (n.contentEl.querySelector(".cm-editor") ? 1 : 0) + "|fo=" + (n.contentEl.contains(document.activeElement) ? 1 : 0) +
             "|lk=" + (vis(a) ? ctr(a) : "-") + "|lt=" + (a ? esc(lt) || "-" : "-") + "|tx=" + esc((o || n.contentEl).textContent).slice(0, 160) + "]";
        continue;
      }
      if (n._cvfWhy) t += " [cvfwhy:" + id + "=" + esc(n._cvfWhy + (n._cvfErr ? ":" + n._cvfErr : "")) + "]";   // a PDF / canvas card that fell back, and why
      if (f.kind === "pdf") {
        k++;
        const s = f.s;
        t += " [cvcard:" + id + "|kind=pdf|pages=" + (s ? (+s.dataset.of || 0) : 0) + "|frame=" + (s && s.querySelector(".pdf-container") ? 1 : 0) +
             "|loaded=" + (s && s.classList.contains("is-loaded") ? 1 : 0) + "]";
      } else if (f.kind === "canvas") {
        k++;
        const s = f.s, ok = s && s.isConnected;
        t += " [cvfile:" + id + "|rects=" + (ok ? s.querySelectorAll("svg rect").length : 0) + "|paths=" + (ok ? s.querySelectorAll("svg path").length : 0) + "]";
      } else if (n._cc && n._cc.kind === "other") {
        const x = n.contentEl.querySelector(".canvas-file-other-ext");
        if (x) { k++; t += " [cvfile:" + id + "|ext=" + esc(x.textContent) + "]"; }
      }
    }
    const root = v.canvas.nodeEl || v.containerEl;
    if (lbl.length) t += " [cvfl:" + lbl.join("|") + "]";
    t += " [cvfile:cm=" + root.querySelectorAll(".canvas-node-text .cm-editor").length + "|ta=" + root.querySelectorAll("textarea.canvas-node-textarea").length + "|ed=" + ed + "]";
    const b = v.errorEl && v.errorEl.querySelector(".cvf-ro-btn");
    if (vis(b)) t += " [cvfile:robtn@" + ctr(b) + "]";
    const ro = v._cvfRO;
    if (ro && ro.path === v.path && v.broken) t += " [cvfile:ro|n=" + v.canvas.nodes.size + "|e=" + v.canvas.edges.size + "|skip=" + ro.skip + "]";
    if (ST.prompt && vis(ST.prompt.input)) t += " [cvfprompt:" + ST.prompt.kind + "|" + esc(String(ST.prompt.input.value).slice(0, 80)) + "]";
    if (ST.copyN) t += " [cvfcopy:n=" + ST.copyN + "|ok=" + ST.copyOk + "]";
    { // a toast lying over the undo control (it takes the click): its centre, so a driver dismisses it like a user
      const u = v.canvas.controlsEl && v.canvas.controlsEl.querySelector(".mod-undo");
      if (vis(u)) {
        const r = u.getBoundingClientRect(), hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
        const nt = hit && hit.closest ? hit.closest(".notice") : null;
        if (nt) t += " [cvfcover:" + ctr(nt) + "]";
      }
    }
    return t;
  }

  Object.defineProperty(window, "__osCanvasFiles", {
    configurable: false, enumerable: false, writable: false,
    value: Object.freeze({ file, startEditing, stopEditing, refused, relink, census }),
  });
})();
