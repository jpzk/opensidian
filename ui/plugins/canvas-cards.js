// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* canvas cards (goal cv-cards): what a card on a canvas board shows, written from
   the public help pages (help.obsidian.md/plugins/canvas) and the JSON Canvas 1.0
   spec (jsoncanvas.org/spec/1.0, MIT) — clean room.

     text card   its Markdown through the app's renderer (X.MarkdownRenderer, the same
                 sanitizing Rust render notes use); an internal [[link]] in it opens the
                 note in a new tab on click
     file card   image -> <img>, audio -> <audio controls>, video -> <video controls>,
                 any other non-note file -> its name + extension, opened with the system
                 default app on double-click (a note card stays canvas.js's md embed)
     link card   Ctrl/Cmd-click hands the URL to the app's ONE external-link gate
                 (main.js a.ext capture -> open_external, Rust re-checks the scheme)
     group       background image (JSON Canvas "background" + "backgroundStyle"
                 cover | ratio | repeat); double-click its label to rename, Enter saves
     every card  the wheel scrolls a card's own content only while it is selected;
                 unselected, the wheel keeps panning the board

   Load order (index.html): ... canvas.js -> canvas-cards.js -> main.js. This file runs
   no code at load beyond one stylesheet and one frozen global, window.__osCanvasCards,
   whose hooks canvas.js's node render() bodies call, and whose census(view) main.js
   canvasTok() appends ([cvcard:] [cvext:]).
   Image bytes ride the app's image scheme (opensidian-img://localhost/<path>); audio/video
   bytes come from main.rs media_read (vault-confined, whitelisted, no-follow, 256 MiB cap) as
   a blob: URL, read only once the card is on screen. */
(function () {
  "use strict";
  /* gh-icons #20: monochrome inline svg icons (drawn for this app, currentColor) in place of
     text pictographs / emoji; class "gi" sizes them (style.css / plugins.css) */
  const I = d => '<svg class="gi" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">' + d + "</svg>";
  const GI = {
    align: I('<path d="M2.8 2.5v11"/><rect x="5" y="4" width="8.2" height="3" rx=".8"/><rect x="5" y="9" width="5" height="3" rx=".8"/>'),
    edit: I('<path d="M10.6 2.7a1.6 1.6 0 0 1 2.3 2.3l-7.6 7.6-3 .9.9-3z"/>'),
    image: I('<rect x="2.5" y="3.5" width="11" height="9" rx="1.5"/><circle cx="6" cy="6.8" r="1"/><path d="M13.5 10.5l-3-3-6.5 5"/>'),
  };
  const IMG = new Set(["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "avif"]);
  const AUD = new Set(["mp3", "wav", "m4a", "ogg", "3gp", "flac"]);
  const VID = new Set(["mp4", "webm", "ogv", "mov", "mkv"]);
  const BG = { cover: 1, ratio: 1, repeat: 1 };
  const ST = { ext: "", extN: 0 };      // the last URL a link card handed to open_external
  const LINK_SEL = "a.wiki, a.internal-link";

  const extOf = p => { const n = String(p || "").split("/").pop(); const d = n.lastIndexOf("."); return d > 0 ? n.slice(d + 1).toLowerCase() : ""; };
  const nameOf = p => String(p || "").split("/").pop();
  const mediaUrl = p => "opensidian-img://localhost/" + String(p).split("/").map(encodeURIComponent).join("/");
  // goal cvf-mediasrv: WebKitGTK's media player reads http(s) and blob: only (no handler for the
  // image scheme), so an audio/video card gets a Blob built from bounded media_read slices.
  // Lazy: the bytes are read only once the card has been on screen for a moment (or on play),
  // never for every media card when a board opens. A file over MEDIA_MAX (main.rs MEDIA_BLOB_MAX)
  // is refused by the first call, before any byte is read, and the card says it cannot play.
  // Every object URL is revoked when its card re-renders, leaves the board or the board closes
  // (MEDIA.live: url -> { m, cc, arm }; an element no longer in the document is swept, and one that
  // is no longer rendered (its tab hidden or closed) drops its bytes and re-arms the lazy read, so a
  // closed tab whose view lingers in the DOM holds no blob).
  const MIME = { mp3: "audio/mpeg", m4a: "audio/mp4", ogg: "audio/ogg", wav: "audio/wav", flac: "audio/flac", webm: "video/webm", mp4: "video/mp4", ogv: "video/ogg", mov: "video/quicktime" };
  const MEDIA_MAX = 256 * 1024 * 1024, MEDIA_DWELL = 150;
  const MEDIA = { live: new Map(), reads: 0, timer: 0, csp: 0 };
  // af-fix 1a: every CSP refusal on the page is counted ([cvmcsp:n]): a restricted media card must cause none
  window.addEventListener("securitypolicyviolation", () => { MEDIA.csp++; }, true);
  function mediaSweep() {
    for (const [u, r] of MEDIA.live) {
      if (r.m.isConnected && r.m.getClientRects().length) continue;
      URL.revokeObjectURL(u); MEDIA.live.delete(u);
      if (!r.m.isConnected || r.cc.blob !== u) continue;
      try { r.m.pause(); r.m.removeAttribute("src"); r.m.load(); } catch (_) {}
      r.cc.blob = ""; r.cc.state = ""; r.arm();
    }
    if (!MEDIA.live.size && MEDIA.timer) { clearInterval(MEDIA.timer); MEDIA.timer = 0; }
  }
  function mediaRevoke(cc) {
    if (cc) clearTimeout(cc.t);
    if (cc && cc.io) { cc.io.disconnect(); cc.io = null; }
    if (cc && cc.blob) { URL.revokeObjectURL(cc.blob); MEDIA.live.delete(cc.blob); cc.blob = ""; }
  }
  // af-fix 1a / #196: pageEval says which CSP header THIS page got (fixed at load). Both headers carry
  // media-src 'self' blob: since #139 (main.rs csp_by_trust), so a media card plays on either page; what the
  // trust changes is WHEN its bytes are read: trusted = lazily once on screen (or on play), restricted =
  // CLICK-TO-PLAY like the audio embeds (#139, editor.js audioFill): nothing is read or demuxed before the press
  let pageTrust = null;
  const mediaTrusted = () => pageTrust || (pageTrust = Promise.resolve().then(() => {
    const T = window.__TAURI__, inv = T && T.core && T.core.invoke;
    return inv ? inv("plugins_state") : null;
  }).then(s => !!(s && s.pageEval), () => false));
  async function mediaBlob(p) {
    const T = window.__TAURI__, inv = T && T.core && T.core.invoke;
    if (!inv) throw new Error("no ipc");
    const parts = []; let off = 0, size = 0;
    do {
      MEDIA.reads++;
      const buf = await inv("media_read", { path: p, offset: off });
      size = Number(new DataView(buf, 0, 8).getBigUint64(0));
      if (size > MEDIA_MAX) throw new Error("too large");
      const c = buf.slice(8);
      if (!c.byteLength && off < size) throw new Error("short read");
      parts.push(c); off += c.byteLength;
    } while (off < size);
    return URL.createObjectURL(new Blob(parts, { type: MIME[extOf(p)] || "" }));
  }
  // wire one card's player: trusted = load on dwell-in-view or on play; restricted (#196) = load on the
  // card's Play button (or the player's own play) only; cc is the card's _cc
  function mediaLazy(node, ctx, p, m, cc) {
    const go = play => {
      if (cc.state) return;
      cc.state = "loading";
      if (cc.io) { cc.io.disconnect(); cc.io = null; }
      clearTimeout(cc.t);
      mediaBlob(p).then(u => {
        if (node._cc !== cc || !m.isConnected) { URL.revokeObjectURL(u); return; }
        cc.blob = u; cc.state = "ready"; MEDIA.live.set(u, { m, cc, arm });
        if (!MEDIA.timer) MEDIA.timer = setInterval(mediaSweep, 2000);
        if (cc.c2p) { cc.c2p.hidden = true; m.style.display = ""; }
        if (m.preload !== "metadata") m.preload = "metadata";   // restricted: only now, after the press
        m.src = u;
        if (play) { const q = m.play(); if (q && q.catch) q.catch(() => {}); }   // the user pressed play: start it
        ping(ctx);
      }, e => {
        if (node._cc !== cc) return;
        const why = String(e && e.message || e), big = /too large/.test(why);
        cc.state = big ? "big" : "error";
        if (big) {
          m.style.display = "none";
          if (cc.c2p) cc.c2p.hidden = true;
          const w = mk("div", "canvas-file-media-error", node.contentEl);
          w.textContent = "Cannot play " + nameOf(p) + ": the file is larger than 256 MB.";
          cc.err = w;
        } else console.error("[canvas] media card " + p + ":", e);
        ping(ctx);
      });
    };
    function arm() {
      if (cc.c2p) {   // restricted (#196): back to the Play button, never a reload on screen
        cc.c2p.hidden = false; m.style.display = "none"; m.preload = "none";
        return;
      }
      if (typeof IntersectionObserver !== "function") return;
      if (cc.io) cc.io.disconnect();
      cc.io = new IntersectionObserver(en => {
        const e = en[en.length - 1];
        clearTimeout(cc.t);
        if (e.isIntersecting && m.getClientRects().length) cc.t = setTimeout(go, MEDIA_DWELL);
      });
      cc.io.observe(m);
    }
    mediaTrusted().then(ok => {
      if (node._cc !== cc || !m.isConnected) return;
      if (ok) {
        m.addEventListener("play", () => go(false));
        if (typeof IntersectionObserver !== "function") { go(false); return; }
        arm();
        return;
      }
      m.preload = "none"; m.style.display = "none";
      const b = mk("button", "canvas-file-media-c2p");
      node.contentEl.insertBefore(b, m);
      b.type = "button";
      b.title = "Play (restricted vault: the file is read when you press play)";
      b.setAttribute("aria-label", "Play " + nameOf(p));
      b.addEventListener("pointerdown", ev => ev.stopPropagation());   // a press on the button is not a card drag
      b.addEventListener("click", ev => { ev.preventDefault(); ev.stopPropagation(); go(true); });
      m.addEventListener("play", () => { if (!cc.state) { m.pause(); go(true); } });
      cc.c2p = b;
      ping(ctx);
    });
  }
  const mk = (tag, cls, parent) => { const e = document.createElement(tag); if (cls) e.className = cls; if (parent) parent.appendChild(e); return e; };
  const ping = ctx => { if (ctx && ctx.deps && typeof ctx.deps.changed === "function") try { ctx.deps.changed(); } catch (_) {} };
  const later = (ctx, ms) => setTimeout(() => ping(ctx), ms || 30);
  const selected = node => !!(node.canvas && node.canvas.selection && node.canvas.selection.has(node));
  const srcPath = node => (node.canvas && node.canvas.path) || "";

  // content rules only (.canvas-node-*): the rendered markdown is block HTML, not the
  // pre-wrap plain text .canvas-text was styled for
  const css = mk("style");
  css.textContent =
    ".canvas-node .canvas-text.markdown-rendered { white-space: normal }\n" +
    ".canvas-node .canvas-text.markdown-rendered > :first-child { margin-top: 0 }\n" +
    ".canvas-node .canvas-text.markdown-rendered > :last-child { margin-bottom: 0 }\n" +
    ".canvas-node-container > .canvas-file-img { display: block; width: 100%; height: 100%; object-fit: contain }\n" +
    ".canvas-node-container > .canvas-file-media { display: block; width: 100%; max-height: 100%; box-sizing: border-box; padding: 8px }\n" +
    ".canvas-node.is-selected.cc-media > .canvas-node-content-blocker { display: none }\n" +
    ".canvas-node-container > .canvas-file-media-error { padding: 8px 12px; color: var(--text-muted, #999); font-size: var(--font-ui-small, 13px) }\n" +
    // #196 restricted click-to-play (the embed's .audio-c2p look, style.css): one button, label from CSS
    ".canvas-node-container > .canvas-file-media-c2p { display: flex; align-items: center; gap: 8px; margin: 8px; padding: 6px 12px; width: calc(100% - 16px); box-sizing: border-box;" +
    " font: inherit; font-size: var(--font-ui-small, 13px); color: var(--text-normal, #ddd); background: var(--interactive-normal, #363636); border: 1px solid var(--background-modifier-border, #444); border-radius: var(--radius-s, 4px); cursor: pointer }\n" +
    ".canvas-node-container > .canvas-file-media-c2p[hidden] { display: none }\n" +
    ".canvas-node-container > .canvas-file-media-c2p::before { content: \"\\25B6\"; font-size: 12px }\n" +
    ".canvas-node-container > .canvas-file-media-c2p::after { content: \"Play (restricted vault: read on click)\" }\n" +
    ".canvas-file-other { padding: 8px 12px; display: flex; flex-direction: column; gap: 4px }\n" +
    ".canvas-file-other-ext { color: var(--text-muted, #999); font-size: 12px; text-transform: uppercase }\n" +
    ".canvas-node-label > .canvas-group-label-input { font: inherit; color: var(--text-normal, #ddd); min-width: 160px }\n" +
    // goal cvf-groups: the group label is a filled tab in the group colour, its toolbar buttons, the custom-colour picker
    ".canvas-node-group > .canvas-node-label { bottom: calc(100% + 4px); padding: 3px 10px; border-radius: 6px; line-height: 1.25;" +
    " background: var(--canvas-color); color: var(--text-on-accent, #fff) }\n" +
    ".canvas-node-group > .canvas-node-label > .canvas-group-label-input { background: transparent; border: 0; outline: none; padding: 0; color: inherit }\n" +
    ".canvas-selection-palette > button.mod-color-custom { background: conic-gradient(#e93147, #e0ac00, #08b94e, #00bfbc, #3366cc, #7852ee, #e93147) }\n" +
    ".canvas-group-picker { position: absolute; z-index: 4; display: flex; align-items: center; gap: 6px; padding: 6px; border-radius: 6px;" +
    " background: var(--background-secondary, #262626); border: 1px solid var(--background-modifier-border, #444); box-shadow: 0 2px 8px rgba(0,0,0,.3) }\n" +
    ".canvas-group-picker-preview { width: 22px; height: 22px; border-radius: 50%; border: 1px solid var(--background-modifier-border, #555) }\n" +
    ".canvas-group-picker-hex { width: 90px; font: 13px var(--font-monospace, monospace); color: var(--text-normal, #ddd);" +
    " background: var(--background-primary, #1e1e1e); border: 1px solid var(--background-modifier-border, #555); border-radius: 4px; padding: 2px 6px }\n" +
    ".canvas-group-picker-hex.is-invalid { border-color: var(--text-error, #e93147) }\n" +
    // goal cvf-textedit: the card editor in the text font, live-preview heading sizes, the link suggester
    ".canvas-node.cc-lp .cvf-text-editor .cm-scroller, .canvas-node.cc-lp .cvf-text-editor .cm-content { font-family: var(--font-text, system-ui, sans-serif) }\n" +
    ".canvas-node.cc-lp .cm-line.cc-lp-h { font-weight: 700; line-height: 1.3 }\n" +
    ".canvas-node.cc-lp .cm-line.cc-lp-h1 { font-size: 1.8em } .canvas-node.cc-lp .cm-line.cc-lp-h2 { font-size: 1.5em }\n" +
    ".canvas-node.cc-lp .cm-line.cc-lp-h3 { font-size: 1.3em } .canvas-node.cc-lp .cm-line.cc-lp-h4 { font-size: 1.15em }\n" +
    ".canvas-node.cc-lp .cc-lp-strong { font-weight: 700 }\n" +
    ".canvas-node.cc-lp .cc-lp-link { color: var(--text-accent, #a78bfa); text-decoration: underline }\n" +
    ".canvas-link-suggest { position: fixed; z-index: 60; min-width: 220px; max-width: 360px; background: var(--background-primary, #1e1e1e); color: var(--text-normal, #ddd);" +
    " border: 1px solid var(--background-modifier-border, #444); border-radius: 6px; box-shadow: 0 4px 14px rgba(0,0,0,.35); font-size: 13px; overflow: hidden }\n" +
    ".canvas-link-suggest-list { max-height: 260px; overflow-y: auto; padding: 4px }\n" +
    ".canvas-link-suggest-item { padding: 4px 8px; border-radius: 4px; cursor: pointer }\n" +
    ".canvas-link-suggest-item.is-selected { background: var(--background-modifier-hover, #333) }\n" +
    ".canvas-link-suggest-path { color: var(--text-muted, #999); font-size: 11px }\n" +
    ".canvas-link-suggest-foot { padding: 4px 8px; color: var(--text-faint, #888); font-size: 11px; border-top: 1px solid var(--background-modifier-border, #444) }\n";
  (document.head || document.documentElement).appendChild(css);

  /* the app's external-link gate: main.js swallows any click on an a.ext in the capture
     phase and hands its href to open_external ([ext:N] counts it). A transient anchor
     goes through exactly that path; its own listener cancels the default navigation in
     case no gate is installed (a canvas never navigates the webview). */
  function openExternal(url, ctx) {
    if (!/^https?:\/\//i.test(url)) return false;
    const a = mk("a", "ext");
    a.href = url; a.hidden = true;
    a.addEventListener("click", e => e.preventDefault());
    document.body.appendChild(a);
    try { a.click(); } finally { a.remove(); }
    ST.ext = url; ST.extN++;
    ping(ctx);
    return true;
  }

  /* one set of listeners per node element (render() runs again on every edit) */
  function wire(node, ctx) {
    node._ccCtx = ctx;
    const cv = node.canvas;
    if (cv && !cv._ccWired && cv.wrapperEl) {      // the census follows selection + pan, which canvas.js does not announce
      cv._ccWired = true;
      cv.wrapperEl.addEventListener("pointerdown", () => later(node._ccCtx, 40));
      cv.wrapperEl.addEventListener("wheel", () => requestAnimationFrame(() => ping(node._ccCtx)), { passive: true });
    }
    if (node._ccWired) return;
    node._ccWired = true;
    const box = node.containerEl;
    box.addEventListener("pointerdown", e => {
      const c = node._ccCtx, s = node._cc || {};
      if (node.isEditing || e.button !== 0) return;
      if (s.kind === "text") {                       // an internal link under the pointer (the blocker sits over it)
        const hit = document.elementsFromPoint(e.clientX, e.clientY).map(x => x.closest && x.closest(LINK_SEL)).find(a => a && node.contentEl.contains(a));
        if (hit) {
          e.stopPropagation(); e.preventDefault();
          const lt = (hit.dataset.note || hit.getAttribute("data-href") || hit.textContent || "") + (hit.dataset.anchor ? "#" + hit.dataset.anchor : "");
          // a selected card's link opens in place (Back returns to the board); Ctrl/Cmd, or a card not yet selected: a new tab
          const how = e.ctrlKey || e.metaKey || !selected(node) ? "tab" : false;
          if (lt && c.app && c.app.workspace) c.app.workspace.openLinkText(lt, srcPath(node), how).catch(err => console.error("[canvas] open link:", err));
          return;
        }
      }
      if (s.kind === "link" && (e.ctrlKey || e.metaKey) && !e.target.classList.contains("canvas-node-resizer")) {
        e.stopPropagation(); e.preventDefault();
        openExternal(String(node.data.url || ""), c);
        return;
      }
      if ((s.kind === "audio" || s.kind === "video") && selected(node) && e.target === s.media) e.stopPropagation();   // its native controls take the press
    });
    box.addEventListener("wheel", e => {
      if (node.isEditing || e.ctrlKey || e.metaKey || !selected(node)) return;   // unselected: the board pans (canvas.js)
      const sc = node.contentEl;
      if (!(sc.scrollHeight > sc.clientHeight + 1)) return;
      const k = e.deltaMode === 1 ? 40 : e.deltaMode === 2 ? sc.clientHeight : 1;
      e.preventDefault(); e.stopPropagation();
      sc.scrollTop += e.deltaY * k;
      requestAnimationFrame(() => ping(node._ccCtx));
    }, { passive: false });
    /* An other-kind file opens on a double press. The board takes pointer capture on every press
       (canvas.js), so the second click and the dblclick land on the wrapper, not on this card:
       detect the double press here, before the wrapper sees it. dblclick stays as a fallback for
       a press that did reach the card, deduped against the press that already opened. */
    let lastP = 0, px = 0, py = 0, opened = 0;
    const openOther = () => {
      const c = node._ccCtx, s = node._cc || {};
      opened = Date.now();
      s.open = (s.open || 0) + 1;
      const fn = c.app && typeof c.app.openWithDefaultApp === "function" ? c.app.openWithDefaultApp : null;
      if (fn) Promise.resolve(fn.call(c.app, node.data.file)).catch(err => console.warn("[canvas] open file:", err));
      ping(c);
    };
    box.addEventListener("pointerdown", e => {
      const s = node._cc || {};
      if (s.kind !== "other" || node.isEditing || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey) return;
      if (e.target.classList.contains("canvas-node-resizer")) return;
      const now = Date.now(), dbl = now - lastP < 500 && Math.abs(e.clientX - px) + Math.abs(e.clientY - py) < 8;
      lastP = now; px = e.clientX; py = e.clientY;
      if (!dbl) return;                              // a single press still selects / drags the card (canvas.js)
      lastP = 0;
      e.stopPropagation(); e.preventDefault();
      openOther();
    });
    box.addEventListener("dblclick", e => {
      const s = node._cc || {};
      if (s.kind !== "other" || node.isEditing) return;
      e.stopPropagation();
      if (Date.now() - opened > 600) openOther();
    });
  }

  /* ------------------------------------------------ text card editor (goal cvf-textedit)
     canvas-files.js mounts the CM6 note editor in a text card; this layer, hooked on the node
     instance once, adds what the card editor does on top of a plain CM6 view:
       - entering edit selects the card alone (the node toolbar shows over it while editing);
         leaving edit keeps it selected, a further Escape on the board clears the selection
       - live preview: a heading line paints large, '#' / '**' / '[[ ]]' markup is hidden on
         every line the caret is not on; proportional text font
       - '[[' adds its closing ']]'; inside '[[...' a link suggester lists the vault's notes:
         Up/Down move, Enter/Tab insert, Escape closes it (and only it) */
  const LP = { v: null };
  function lpExt(Vw) {
    if (LP.v) return LP.v;
    const D = Vw.Decoration, hide = D.replace({}), strong = D.mark({ class: "cc-lp-strong" }), lnk = D.mark({ class: "cc-lp-link" });
    const head = [1, 2, 3, 4, 5, 6].map(n => D.line({ class: "cc-lp-h cc-lp-h" + n }));
    const build = v => {
      const s = v.state, doc = s.doc, out = [], caret = new Set();
      for (const r of s.selection.ranges) for (let i = doc.lineAt(r.from).number, b = doc.lineAt(r.to).number; i <= b; i++) caret.add(i);
      for (const vr of v.visibleRanges) {
        for (let p = vr.from; p <= vr.to;) {
          const ln = doc.lineAt(p), t = ln.text, off = !caret.has(ln.number);
          const h = /^(#{1,6})[ \t]+/.exec(t);
          if (h) { out.push(head[h[1].length - 1].range(ln.from)); if (off) out.push(hide.range(ln.from, ln.from + h[0].length)); }
          const re = /\*\*(?=\S)([^*\n]*?\S)\*\*|\[\[([^\]|\n]+)(\|[^\]\n]*)?\]\]/g;
          let m;
          while ((m = re.exec(t))) {
            const a = ln.from + m.index, b = a + m[0].length;
            const in0 = m[2] === undefined ? a + 2 : m[3] ? a + 2 + m[2].length + 1 : a + 2;
            if (in0 < b - 2) out.push((m[2] === undefined ? strong : lnk).range(in0, b - 2));
            if (off) { out.push(hide.range(a, in0)); out.push(hide.range(b - 2, b)); }
          }
          p = ln.to + 1;
        }
      }
      return D.set(out, true);
    };
    const plugin = Vw.ViewPlugin.fromClass(class {
      constructor(v) { this.decorations = build(v); }
      update(u) { if (u.docChanged || u.viewportChanged || u.selectionSet) this.decorations = build(u.view); }
    }, { decorations: p => p.decorations });
    // the second '[' of '[[' brings its ']]' (not when they are already there)
    const close = Vw.EditorView.inputHandler.of((view, from, to, ins) => {
      const d = view.state.doc;
      if (ins !== "[" || from < 1 || d.sliceString(from - 1, from) !== "[" || d.sliceString(to, to + 2) === "]]") return false;
      view.dispatch({ changes: { from, to, insert: "[]]" }, selection: { anchor: from + 1 }, userEvent: "input.type" });
      return true;
    });
    // the note editor's keys (caret to doc start/end, word moves, undo/redo): the bare embed editor has none
    const C = (window.__osModules || {})["@codemirror/commands"], keys = [];
    if (C && Array.isArray(C.defaultKeymap)) {
      if (typeof C.history === "function") keys.push(C.history());
      keys.push(Vw.keymap.of([].concat(C.defaultKeymap, Array.isArray(C.historyKeymap) ? C.historyKeymap : [])));
    }
    return (LP.v = [plugin, close].concat(keys));
  }

  // '[[query' before the caret on its line, not yet closed: { start (after '[['), q } or null
  function sugAt(view) {
    const s = view.state, r = s.selection.main;
    if (!r.empty) return null;
    const ln = s.doc.lineAt(r.head), before = ln.text.slice(0, r.head - ln.from), i = before.lastIndexOf("[[");
    if (i < 0) return null;
    const q = before.slice(i + 2);
    return /[\]|#^\n]/.test(q) ? null : { start: ln.from + i + 2, q };
  }
  function sugFiles(app, q) {
    let all = [];
    try { all = app && app.vault ? app.vault.getMarkdownFiles() : []; } catch (_) { all = []; }
    const lq = q.toLowerCase(), hits = [];
    for (const f of all) {
      const b = String(f.basename || nameOf(f.path).replace(/\.md$/i, "")).toLowerCase(), p = String(f.path).toLowerCase();
      const sc = !lq ? 0 : b.startsWith(lq) ? 0 : b.includes(lq) ? 1 : p.includes(lq) ? 2 : -1;
      if (sc >= 0) hits.push({ f, sc, b });
    }
    hits.sort((x, y) => x.sc - y.sc || x.b.localeCompare(y.b) || String(x.f.path).localeCompare(String(y.f.path)));
    return hits.slice(0, 10).map(h => h.f);
  }
  function sugClose(node) {
    const g = node._ccSug;
    if (!g || !g.el) return;
    g.el.remove(); g.el = null; g.files = [];
    ping(node._ccCtx);
  }
  function sugPaint(g) {
    g.list.textContent = "";
    g.files.forEach((f, i) => {
      const it = mk("div", "canvas-link-suggest-item" + (i === g.idx ? " is-selected" : ""), g.list);
      mk("div", "canvas-link-suggest-title", it).textContent = f.basename || nameOf(f.path).replace(/\.md$/i, "");
      const dir = String(f.path).includes("/") ? String(f.path).slice(0, String(f.path).lastIndexOf("/")) : "";
      if (dir) mk("div", "canvas-link-suggest-path", it).textContent = dir;
      it.addEventListener("pointerdown", e => { e.preventDefault(); e.stopPropagation(); g.idx = i; sugPick(g.node); });
    });
  }
  function sugUpdate(node, view) {
    const g = node._ccSug || (node._ccSug = { node, el: null, files: [], idx: 0, start: -1, dis: -1 });
    const at = view.hasFocus ? sugAt(view) : null;
    if (!at) { g.dis = -1; sugClose(node); return; }
    if (at.start === g.dis) { sugClose(node); return; }      // Escape closed it for this '[['
    g.start = at.start; g.view = view;
    const files = sugFiles(node._ccCtx && node._ccCtx.app, at.q);
    if (!files.length) { sugClose(node); return; }
    if (!g.el) {
      g.el = mk("div", "canvas-link-suggest", document.body);
      g.list = mk("div", "canvas-link-suggest-list", g.el);
      mk("div", "canvas-link-suggest-foot", g.el).textContent = "↑↓ choose · ↵ insert · esc dismiss";
      g.el.addEventListener("pointerdown", e => { e.preventDefault(); e.stopPropagation(); });
      g.el.addEventListener("wheel", e => e.stopPropagation(), { passive: true });
    }
    const same = g.files.length === files.length && g.files.every((f, i) => f === files[i]);
    if (!same) { g.files = files; g.idx = 0; }
    sugPaint(g);
    const c = view.coordsAtPos(Math.max(0, at.start - 2));
    if (c) { g.el.style.left = Math.round(c.left) + "px"; g.el.style.top = Math.round(c.bottom + 4) + "px"; }
    ping(node._ccCtx);
  }
  function sugPick(node) {
    const g = node._ccSug, view = g && g.view;
    if (!g || !g.el || !view || !g.files[g.idx]) return;
    const f = g.files[g.idx], base = f.basename || nameOf(f.path).replace(/\.md$/i, "");
    let lt = base;
    try { if (g.files.length && node._ccCtx.app.vault.getMarkdownFiles().filter(x => x.basename === base).length > 1) lt = String(f.path).replace(/\.md$/i, ""); } catch (_) {}
    const head = view.state.selection.main.head, doc = view.state.doc;
    const closed = doc.sliceString(head, head + 2) === "]]";
    view.dispatch({ changes: { from: g.start, to: head, insert: lt + (closed ? "" : "]]") }, selection: { anchor: g.start + lt.length + 2 }, userEvent: "input.complete" });
    sugClose(node);
  }
  function sugKey(node, e) {
    const g = node._ccSug;
    if (!g || !g.el) return;
    const n = g.files.length;
    if (e.key === "Escape") { g.dis = g.start; sugClose(node); }
    else if (e.key === "ArrowDown") { g.idx = (g.idx + 1) % n; sugPaint(g); ping(node._ccCtx); }
    else if (e.key === "ArrowUp") { g.idx = (g.idx + n - 1) % n; sugPaint(g); ping(node._ccCtx); }
    else if (e.key === "Enter" || e.key === "Tab") sugPick(node);
    else return;
    e.preventDefault(); e.stopPropagation();
  }
  function edExtend(node) {
    const st = node._cvfEd, M = window.__osModules || {}, Vw = M["@codemirror/view"], Sx = M["@codemirror/state"];
    if (!st || !st.em || !st.em.cm || !Vw || !Sx || st._cc) return;
    st._cc = true;
    node.containerEl.classList.add("cc-lp");
    try {
      st.em.cm.dispatch({ effects: Sx.StateEffect.appendConfig.of([lpExt(Vw),
        Vw.EditorView.updateListener.of(u => {
          if (!(u.docChanged || u.selectionSet || u.focusChanged)) return;
          sugUpdate(node, u.view);
          // the census (caret line, painted lines) follows the caret: one refresh per frame
          if (!node._ccRaf) node._ccRaf = requestAnimationFrame(() => { node._ccRaf = 0; ping(node._ccCtx); });
        })]) });
    } catch (e) { console.error("[canvas] text card editor extensions:", e); }
    st.host.addEventListener("keydown", e => sugKey(node, e), true);   // before the editor and the card's own Escape
  }
  function edHook(node) {
    if (node._ccEd) return;
    node._ccEd = true;
    const start = node.startEditing, stop = node.stopEditing;
    node.startEditing = function () {
      const r = start.apply(this, arguments);
      if (this.isEditing && !this._gone) {
        const c = this.canvas;
        if (c && typeof c.select === "function" && !(c.selection.size === 1 && c.selection.has(this))) c.select(this, false);
        edExtend(this);
        ping(this._ccCtx);
      }
      return r;
    };
    node.stopEditing = function () {
      sugClose(this);
      if (this._ccSug) this._ccSug.dis = -1;
      return stop.apply(this, arguments);
    };
  }

  function text(node, ctx) {
    wire(node, ctx);
    edHook(node);
    const seq = (node._ccSeq = (node._ccSeq || 0) + 1);
    node._cc = { kind: "text", md: 0, tags: "" };
    const src = String(node.data.text == null ? "" : node.data.text);
    const MR = ctx.X && ctx.X.MarkdownRenderer;
    if (!MR || typeof MR.render !== "function") return;
    const out = mk("div", "canvas-text markdown-rendered");
    Promise.resolve(MR.render(ctx.app, src, out, srcPath(node), node)).then(() => {
      if (node._ccSeq !== seq || node.isEditing || node._gone) return;
      node.contentEl.textContent = "";
      node.contentEl.appendChild(out);
      const tags = [];
      for (const x of out.querySelectorAll("*")) { const t = x.tagName.toLowerCase(); if (!tags.includes(t)) tags.push(t); }
      node._cc.md = 1; node._cc.tags = tags.slice(0, 12).join(".");
      ping(ctx);
    }, e => console.error("[canvas] text card render:", e));
  }

  // a non-note file card; false = not ours (a note, a canvas, a missing file: canvas.js paints it)
  function file(node, ctx) {
    wire(node, ctx);
    const p = String(node.data.file || ""), ext = ctx.file && ctx.file.extension ? String(ctx.file.extension).toLowerCase() : extOf(p);
    node.containerEl.classList.remove("cc-media");
    mediaRevoke(node._cc);
    // the vault model lists notes, canvases and images only: no TFile for an .mp3/.zip says nothing
    const tracked = ext === "md" || ext === "canvas" || IMG.has(ext);
    if (ext === "md" || ext === "canvas" || (ctx.missing && tracked) || !p) { node._cc = { kind: ext === "md" && !ctx.missing ? "note" : "missing" }; return false; }   // goal cv-model: a note not in the vault model is missing, not a note
    const host = node.contentEl;
    if (IMG.has(ext)) {
      const img = mk("img", "canvas-file-img", host);
      img.alt = nameOf(p); img.draggable = false;
      img.addEventListener("load", () => ping(ctx), { once: true });
      img.addEventListener("error", () => ping(ctx), { once: true });
      img.src = mediaUrl(p);
      node._cc = { kind: "image", img };
      return true;
    }
    if (AUD.has(ext) || VID.has(ext)) {
      const kind = AUD.has(ext) ? "audio" : "video";
      const m = mk(kind, "canvas-file-media", host);
      m.controls = true; m.preload = "metadata";
      node.containerEl.classList.add("cc-media");
      const cc = node._cc = { kind, media: m, blob: "", state: "", io: null, t: 0, err: null };
      mediaLazy(node, ctx, p, m, cc);
      return true;
    }
    const box = mk("div", "canvas-file-other", host);
    mk("div", "canvas-file-other-name", box).textContent = nameOf(p);
    mk("div", "canvas-file-other-ext", box).textContent = ext || "file";
    node._cc = { kind: "other", open: (node._cc && node._cc.open) || 0 };
    return true;
  }

  function link(node, ctx) {
    wire(node, ctx);
    node._cc = { kind: "link" };
  }

  function group(node, ctx) {
    wire(node, ctx);
    const d = node.data, c = node.contentEl, s = c.style;
    const bg = typeof d.background === "string" ? d.background : "";
    const how = BG[d.backgroundStyle] ? d.backgroundStyle : "cover";
    if (bg) {
      s.backgroundImage = 'url("' + mediaUrl(bg) + '")';
      s.backgroundPosition = "center";
      s.backgroundSize = how === "cover" ? "cover" : how === "ratio" ? "contain" : "auto";
      s.backgroundRepeat = how === "repeat" ? "repeat" : "no-repeat";
    } else { s.backgroundImage = ""; s.backgroundPosition = ""; s.backgroundSize = ""; s.backgroundRepeat = ""; }
    node._cc = { kind: "group", bg: bg ? how : "-", ren: node._cc && node._cc.ren ? 1 : 0 };
    gboard(node.canvas, ctx);
    const lab = node.labelEl;
    if (lab._ccWired) return;
    lab._ccWired = true;
    let last = 0, lx = 0, ly = 0;
    lab.addEventListener("pointerdown", e => {
      if (e.target !== lab || e.button !== 0) return;
      const now = Date.now(), dbl = now - last < 500 && Math.abs(e.clientX - lx) + Math.abs(e.clientY - ly) < 8;
      last = now; lx = e.clientX; ly = e.clientY;
      if (!dbl) return;                              // a single press still selects / drags the group (canvas.js)
      e.stopPropagation(); e.preventDefault();
      last = 0;
      rename(node);
    });
    lab.addEventListener("dblclick", e => { if (e.target === lab) { e.stopPropagation(); rename(node); } });
  }

  function rename(node) {
    const ctx = node._ccCtx, lab = node.labelEl, d = node.data;
    if (node._ccRen || (node.canvas && node.canvas.readonly)) return;
    node._ccRen = true; if (node._cc) node._cc.ren = 1;
    lab.textContent = "";
    const inp = mk("input", "canvas-group-label-input", lab);
    inp.type = "text"; inp.value = d.label || ""; inp.spellcheck = false;
    for (const ev of ["pointerdown", "dblclick", "wheel", "keyup"]) inp.addEventListener(ev, e => e.stopPropagation());
    let done = false;
    const finish = keep => {
      if (done) return; done = true;
      const v = inp.value.trim();
      node._ccRen = false; if (node._cc) node._cc.ren = 0;
      if (keep && v !== (d.label || "")) { if (v) d.label = v; else delete d.label; node.canvas.requestSave(); }
      lab.textContent = d.label || "";
      try { node.canvas.wrapperEl.focus({ preventScroll: true }); } catch (_) {}
      ping(ctx);
    };
    inp.addEventListener("keydown", e => {
      e.stopPropagation();
      if (e.key === "Enter") { e.preventDefault(); finish(true); }
      else if (e.key === "Escape") { e.preventDefault(); finish(false); }
    });
    inp.addEventListener("blur", () => finish(true));
    setTimeout(() => { try { inp.focus(); inp.select(); } catch (_) {} }, 0);
    ping(ctx);
  }

  /* goal cvf-groups: a lone selected group adds arrange / edit label / background to the
     selection toolbar (canvas-select.js owns the toolbar and its remove / colour / zoom), and
     the colour palette gains a custom swatch whose small picker takes a #rrggbb value.
     Installed once per board, from the first group render, once canvas-select has built
     the toolbar. */
  const GBTN = [["arrange", "Align and arrange", GI.align], ["label", "Edit label", GI.edit], ["background", "Set background", GI.image]];
  const groupOf = c => { const n = c.selection.size === 1 ? [...c.selection][0] : null; return n && n.data && n.data.type === "group" && c.nodes.get(n.id) === n ? n : null; };
  const roOf = c => !!(c.readonly || (c.options && c.options.readOnly));
  const gping = c => ping(c._ccGX);
  const toBoard = c => { try { c.wrapperEl.focus({ preventScroll: true }); } catch (_) {} };

  function gboard(c, ctx) {
    if (!c) return;
    c._ccGX = ctx;
    const s = c._cvs;
    if (!s || !s.tbEl || !s.palEl) {                 // canvas-select installs its overlay when the board is wired
      if (!c._ccGT) c._ccGT = setTimeout(() => { c._ccGT = 0; gboard(c, c._ccGX); }, 60);
      return;
    }
    if (c._ccG) return;
    const g = c._ccG = { btn: {}, on: null, custom: null, pick: null, hex: null, ok: null };
    const mkb = (parent, name, label, txt, fn) => {
      const b = mk("button", "canvas-selection-btn mod-" + name, parent);
      b.type = "button"; if (txt.startsWith("<svg")) b.innerHTML = txt; else b.textContent = txt; b.title = label; b.setAttribute("aria-label", label); b.dataset.action = name;
      b.addEventListener("click", e => { e.stopPropagation(); fn(e); });
      return b;
    };
    g.btn.arrange = mkb(s.tbEl, "arrange", GBTN[0][1], GBTN[0][2], e => arrangeMenu(c, e.currentTarget));
    g.btn.label = mkb(s.tbEl, "label", GBTN[1][1], GBTN[1][2], () => { const n = groupOf(c); if (n && !roOf(c)) rename(n); });
    g.btn.background = mkb(s.tbEl, "background", GBTN[2][1], GBTN[2][2], () => { const n = groupOf(c); if (n && !roOf(c)) pickBackground(c, n); });
    for (const b of Object.values(g.btn)) b.hidden = true;
    g.custom = mkb(s.palEl, "color-custom", "Custom color", "", () => openPick(c));
    const sync = () => gsync(c);
    const sel = c.select;                            // the selection changes through select() (deselectAll included)
    c.select = function () { const r = sel.apply(this, arguments); sync(); return r; };
    new MutationObserver(sync).observe(s.tbEl, { attributes: true, attributeFilter: ["hidden", "style"] });
    new MutationObserver(sync).observe(s.palEl, { attributes: true, attributeFilter: ["hidden"] });
    sync();
  }

  function gsync(c) {
    const g = c._ccG, s = c._cvs; if (!g || !s) return;
    const on = !!groupOf(c) && !roOf(c);
    if (on !== g.on) {
      g.on = on;
      for (const b of Object.values(g.btn)) b.hidden = !on;
      if (window.CanvasSelect) try { window.CanvasSelect.refresh(c); } catch (_) {}   // re-centre: the toolbar changed width
      gping(c);
    }
    if (g.pick && s.palEl.hidden) closePick(c);
  }

  // the arrange operations (canvas-arrange.js) applied to the cards inside the group
  function arrangeMenu(c, at) {
    const n = groupOf(c), X = c._ccGX && c._ccGX.X, A = window.CanvasArrange;
    if (!n || !X || !X.Menu || !A || roOf(c)) return;
    const d = n.data;
    const kids = [...c.nodes.values()].filter(k => k !== n && k.data.type !== "group" &&
      k.data.x >= d.x && k.data.y >= d.y && k.data.x + k.data.width <= d.x + d.width && k.data.y + k.data.height <= d.y + d.height);
    const NAMES = { "align-left": "Align left", "align-centre": "Align centre", "align-right": "Align right", "align-top": "Align top",
      "align-middle": "Align middle", "align-bottom": "Align bottom", "distribute-horizontal": "Distribute horizontally",
      "distribute-vertical": "Distribute vertically", "arrange-row": "Arrange in a row", "arrange-column": "Arrange in a column",
      "arrange-grid": "Arrange in a grid" };
    const m = new X.Menu();
    for (const key of A.ops()) {
      if (!NAMES[key]) continue;
      const need = key.startsWith("distribute") ? 3 : 2;
      m.addItem(i => {
        i.setTitle(NAMES[key]);
        if (kids.length < need) i.setDisabled(true);
        i.onClick(() => {
          if (kids.length < need) return;
          toBoard(c);
          c.deselectAll(); for (const k of kids) c.select(k, true);
          try { A.run(c, key); } finally { c.deselectAll(); if (c.nodes.get(n.id) === n) c.select(n); }
          gping(c);
        });
      });
    }
    const r = at.getBoundingClientRect();
    m.showAtPosition({ x: Math.round(r.left), y: Math.round(r.bottom + 4) });
  }

  // a vault image as the group background (JSON Canvas "background"), or none
  function pickBackground(c, n) {
    const ctx = c._ccGX, X = ctx && ctx.X, app = ctx && ctx.app;
    if (!X || !X.FuzzySuggestModal || !app || !app.vault) return;
    const NONE = { none: true, path: "No background" };
    let files = [];
    try { files = app.vault.getFiles().filter(f => IMG.has(String(f.extension || extOf(f.path)).toLowerCase())).map(f => ({ path: f.path })); } catch (_) {}
    if (typeof n.data.background === "string" && n.data.background) files.unshift(NONE);
    class Pick extends X.FuzzySuggestModal {
      getItems() { return files; }
      getItemText(f) { return f.path; }
      onChooseItem(f) {
        if (c.nodes.get(n.id) !== n || roOf(c)) return;
        const d = n.data, was = d.background || "";
        if (f.none) delete d.background; else d.background = f.path;
        if ((d.background || "") === was) return;
        n.render(); c.requestSave(); toBoard(c); gping(c);
      }
    }
    const p = new Pick(app);
    if (typeof p.setPlaceholder === "function") p.setPlaceholder("Choose a background image");
    p.open();
  }

  function closePick(c) {
    const g = c._ccG; if (!g || !g.pick) return;
    g.pick.remove(); g.pick = g.hex = g.ok = null;
    gping(c);
  }
  const normHex = v => {
    v = String(v || "").trim().toLowerCase();
    if (v && v[0] !== "#") v = "#" + v;
    if (/^#[0-9a-f]{3}$/.test(v)) v = "#" + v[1] + v[1] + v[2] + v[2] + v[3] + v[3];
    return /^#[0-9a-f]{6}$/.test(v) ? v : "";
  };
  function openPick(c) {
    const g = c._ccG, s = c._cvs; if (!g || !s || roOf(c)) return;
    closePick(c);
    const p = g.pick = mk("div", "canvas-group-picker", c.wrapperEl);
    for (const ev of ["pointerdown", "dblclick", "wheel", "keyup", "contextmenu"]) p.addEventListener(ev, e => e.stopPropagation());
    const pre = mk("div", "canvas-group-picker-preview", p);
    const inp = g.hex = mk("input", "canvas-group-picker-hex", p);
    inp.type = "text"; inp.spellcheck = false; inp.maxLength = 7; inp.placeholder = "#rrggbb"; inp.setAttribute("aria-label", "Hex color");
    const ok = g.ok = mk("button", "canvas-group-picker-ok", p);
    ok.type = "button"; ok.textContent = "OK";
    const first = [...c.selection][0], cur = first && first.data ? normHex(first.data.color) : "";
    inp.value = cur;
    const show = () => { const v = normHex(inp.value); pre.style.background = v || "transparent"; inp.classList.toggle("is-invalid", !!inp.value.trim() && !v); };
    show();
    const apply = () => {
      const v = normHex(inp.value);
      if (!v) { inp.classList.add("is-invalid"); return; }
      if (!roOf(c)) for (const n of [...c.selection]) if ((n.data.color || "") !== v) n.setColor(v);
      s.palette = false;
      closePick(c);
      if (window.CanvasSelect) try { window.CanvasSelect.refresh(c); } catch (_) {}
      toBoard(c); gping(c);
    };
    inp.addEventListener("input", show);
    inp.addEventListener("keydown", e => {
      e.stopPropagation();
      if (e.key === "Enter") { e.preventDefault(); apply(); }
      else if (e.key === "Escape") { e.preventDefault(); closePick(c); toBoard(c); }
    });
    ok.addEventListener("click", e => { e.stopPropagation(); apply(); });
    // under the palette, or above it when the board has no room below
    const pl = s.palEl.offsetLeft, pt = s.palEl.offsetTop, ph = s.palEl.offsetHeight || 32, H = c.wrapperEl.clientHeight || 0;
    p.style.left = pl + "px";
    p.style.top = (pt + ph + 4 + 40 > H && pt > 44 ? pt - 40 : pt + ph + 4) + "px";
    setTimeout(() => { try { inp.focus(); inp.select(); } catch (_) {} }, 0);
    gping(c);
  }

  /* census: one token per painted card of the visible canvas view, the board's pan,
     the last external open. Read off the DOM, not off node state, where it can be. */
  function census(v) {
    if (!v || !v.canvas) return "";
    const esc = x => String(x == null ? "" : x).replace(/[[\]|@]/g, "_").replace(/\s+/g, " ").trim();
    const vis = e => !!e && e.getClientRects().length > 0;
    const ctr = e => { const r = e.getBoundingClientRect(); return Math.round(r.x + r.width / 2) + "," + Math.round(r.y + r.height / 2); };
    let t = "", k = 0;
    for (const n of v.canvas.nodes.values()) {
      if (k >= 16 || !vis(n.containerEl)) continue;
      k++;
      const s = n._cc || {}, id = esc(n.id), ty = n.data.type;
      if (ty === "text") {
        const a = n.contentEl.querySelector(LINK_SEL);
        t += " [cvcard:" + id + "|md=" + (s.md && n.contentEl.querySelector(".markdown-rendered") ? 1 : 0) + "|kind=text|tags=" + esc(s.tags || "") + (vis(a) ? "|lnk=" + ctr(a) : "") + "]";
      } else if (ty === "file") {
        if (s.kind === "image") t += " [cvcard:" + id + "|img=" + (s.img && s.img.complete && s.img.naturalWidth > 0 ? 1 : 0) + "|kind=image]";
        else if (s.kind === "audio" || s.kind === "video") t += " [cvcard:" + id + "|kind=" + s.kind + "|ctl=" + (s.media && s.media.controls && n.contentEl.contains(s.media) ? 1 : 0) + "]";
        else if (s.kind === "other") t += " [cvcard:" + id + "|kind=other|name=" + esc(nameOf(n.data.file)) + "|open=" + (s.open || 0) + "]";
        else t += " [cvcard:" + id + "|kind=" + (s.kind || "note") + "|sel=" + (v.canvas.selection.has(n) ? 1 : 0) + "|st=" + Math.round(n.contentEl.scrollTop) +
                  "|txt=" + esc(n.contentEl.textContent).slice(0, 48) + "]";
      } else if (ty === "link") {
        const a = n.contentEl.querySelector(".canvas-link");
        t += " [cvcard:" + id + "|kind=link" + (vis(a) ? "|at=" + ctr(a) : "") + "]";
      } else if (ty === "group") {
        const cs = getComputedStyle(n.contentEl), bg = n.data.background;
        t += " [cvcard:" + id + "|kind=group" + (vis(n.labelEl) ? "|lab=" + ctr(n.labelEl) : "") + "|ren=" + (n._ccRen ? 1 : 0) + "|bg=" + esc(s.bg || "-") +
             "|rep=" + esc(cs.backgroundRepeat) + "|url=" + (bg && cs.backgroundImage.includes(mediaUrl(bg)) ? 1 : 0) + "]";
      }
    }
    // goal cvf-mediasrv: [cvmblob:<live object URLs>|vis=<media cards on screen>|rd=<media_read calls>] [cvmbig:<id>@x,y]
    // #196: [cvmc2p:<id>@x,y] a restricted page's media card not yet pressed (its Play button), [cvmcsp:<CSP refusals on the page>]
    mediaSweep();
    const wr = v.canvas.wrapperEl ? v.canvas.wrapperEl.getBoundingClientRect() : null;
    let mv = 0;
    for (const n of v.canvas.nodes.values()) {
      const s = n._cc;
      if (!s || (s.kind !== "audio" && s.kind !== "video") || !n.containerEl.isConnected) continue;
      const r = n.containerEl.getBoundingClientRect();
      if (wr && r.width && r.height && r.right > wr.left && r.left < wr.right && r.bottom > wr.top && r.top < wr.bottom) mv++;
      if (s.state === "big" && vis(s.err)) t += " [cvmbig:" + esc(n.id) + "@" + ctr(s.err) + "]";
      if (!s.state && s.c2p && vis(s.c2p)) t += " [cvmc2p:" + esc(n.id) + "@" + ctr(s.c2p) + "]";   // #196
    }
    t += " [cvmblob:" + MEDIA.live.size + "|vis=" + mv + "|rd=" + MEDIA.reads + "]";
    t += " [cvmcsp:" + MEDIA.csp + "]";   // af-fix 1a
    t += " [cvcard:@pan|" + Math.round(v.canvas.tx) + "," + Math.round(v.canvas.ty) + "]";
    // goal cvf-groups: [cvgtb:arrange@x,y|label@x,y|background@x,y] [cvgpal:custom@x,y] [cvgpick:hex@x,y|ok@x,y]
    const g = v.canvas._ccG, gv = e => !!e && !e.hidden && vis(e);
    if (g) {
      const bs = Object.keys(g.btn).filter(k => gv(g.btn[k]));
      if (bs.length) t += " [cvgtb:" + bs.map(k => k + "@" + ctr(g.btn[k])).join("|") + "]";
      if (gv(g.custom)) t += " [cvgpal:custom@" + ctr(g.custom) + "]";
      if (g.pick && gv(g.hex)) t += " [cvgpick:hex@" + ctr(g.hex) + "|ok@" + ctr(g.ok) + "]";
    }
    if (ST.ext) t += " [cvext:" + esc(ST.ext) + "]";
    return t;
  }

  Object.defineProperty(window, "__osCanvasCards", {
    configurable: false, enumerable: false, writable: false,
    value: Object.freeze({ text, file, link, group, census }),
  });
})();
