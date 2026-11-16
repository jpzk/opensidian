// SPDX-License-Identifier: GPL-3.0-or-later
/* ======================================================================
   ui/scrollpos.js — ONE scroll-position model for every note surface
   (goal scroll-model; root cause: /workspace/goal/scroll-model/ROOTCAUSE.md)

   THE POSITION is content, never pixels:
     { line }  = 0-based SOURCE LINE at the scroller's top, fractional:
                 floor(line) = the top line, line - floor(line) = how far into
                 that line (or into the block widget holding it) the top edge is.
   Every surface maps it through its OWN geometry:
     live preview / source  ONE CM6 EditorView, auto-height, inside the pane
                            scroller g.lp (an EXTERNAL scrollParent for CM6)
     reading                g.preview, block k starts at source line lines[k]

   WHY CM6 MUST LAND IT. CM6 re-anchors its scroller inside its measure loop:
   it takes an anchor at loop start, measures (estimates and, after the .lp.src
   restyle, stale heights become real), runs measure-request writes, and then
   corrects `scrollTop += newAnchorTop - oldAnchorTop` — but, with an external
   scrollParent, only while the editor has focus or a wheel/touch is < 100 ms
   old. A scrollTop computed by us from the height map is therefore right,
   counted twice or counted zero times depending on focus and timing (longpos
   off 19, Properties off 15, scrollsync off 14). So on the CM6 surface this
   module NEVER writes the scroller from outside CM6: it dispatches a
   scrollIntoView effect and lands in CM6's scrollHandler, i.e. inside the
   measure loop, after the target is drawn and measured; CM6 then re-takes its
   anchor from the landed position (`o = -1; continue`). Heights that settle
   later (an image decoding, a widget measured) re-land through the same
   effect while the position is PINNED; any user input on the scroller, or
   any other scrollIntoView (caret, find), unpins it.

   API (global ScrollPos):
     capture(g, mode?)            -> { line } | null   (mode: the view's own tab mode)
     restore(g, pos|line, opts?)  land the CURRENT view on it (opts.settle ms, default 5000)
     centre(g, line, opts?)       centre source line `line` (block middle); opts.done() after the first landing
     ext()                        the CM6 extension (Ed.mount)
     wire(lp)                     user-intent listeners on a pane scroller (Ed.mount)
     cancel(g)                    drop any pending/pinned landing
   ====================================================================== */
const ScrollPos = (() => {
  const SP = {};
  const SETTLE_MS = 5000, MAX_RELAND = 40, VERIFY_FRAMES = 30;
  const tabOf = g => (g && g.active >= 0 ? g.tabs[g.active] : null);
  const modeOf = g => { const t = tabOf(g); return !t || t.kind || g.graphOn ? null : t.mode; };
  const clamp = (x, a, b) => Math.min(b, Math.max(a, x));
  const lineOf = p => (p == null ? null : typeof p === "number" ? p : p.line);

  /* ---------- reading view: block -> source line map ---------- */
  const pvMap = g => {
    const P = g.preview; if (!P) return null;
    const ch = P.children, bl = (P._pvk && P._pvk.lines && P._pvk.lines.length === ch.length) ? P._pvk.lines : g.pvLines;
    return ch.length && bl && bl.length === ch.length ? [ch, bl] : null;   // map out of step: no position beats a wrong one
  };
  function readTop(g) {
    const m = pvMap(g); if (!m) return null;
    const [ch, bl] = m, ct = g.preview.getBoundingClientRect().top;
    const P = g.preview, at = P._spAt;
    // a reading view still where the module LANDED it holds that position exactly: the
    // reading view has coarser geometry than the editor (one block per element, no
    // element at all for the frontmatter), and re-deriving the line from it on the way
    // back would quantise an edit -> reading -> edit round trip to a block start
    // (fm lp->read->lp: L2 under the Properties came back as L5, the first rendered block)
    if (at && at.first === ch[0] && at.n === ch.length && Math.abs(P.scrollTop - at.st) < 1) return at.line;
    // the reading view at its very top IS the top of the note: blocks the reading
    // view does not render (frontmatter -> Properties above block 0) must not move it
    // to the first rendered block (fm lp->read->lp landed on L5 under the Properties)
    if (P.scrollTop < 1) return 0;
    let i = ch.length - 1, f = 0;
    for (let k = 0; k < ch.length; k++) {
      const r = ch[k].getBoundingClientRect();
      if (r.bottom > ct + 1) { i = k; f = r.height > 0 ? clamp((ct - r.top) / r.height, 0, 1) : 0; break; }
      if (k === ch.length - 1) f = 0;
    }
    const a = bl[i] - 1, b = i + 1 < bl.length ? bl[i + 1] - 1 : a + 1;
    return a + f * Math.max(1, b - a);
  }
  function readLand(g, line) {   // reading view is not CM6: the module writes it directly
    const m = pvMap(g); if (!m) return false;
    const [ch, bl] = m, P = g.preview;
    let i = 0;
    while (i + 1 < bl.length && bl[i + 1] - 1 <= line) i++;
    const a = bl[i] - 1, b = i + 1 < bl.length ? bl[i + 1] - 1 : a + 1;
    const el = ch[i], fr = clamp((line - a) / Math.max(1, b - a), 0, 1);
    const d = el.getBoundingClientRect().top - P.getBoundingClientRect().top + fr * el.getBoundingClientRect().height;
    P.scrollTop = clamp(P.scrollTop + d, 0, Math.max(0, P.scrollHeight - P.clientHeight));
    P._spAt = { line, st: P.scrollTop, first: ch[0], n: ch.length };   // readTop: unmoved since = this position
    return true;
  }
  // an <img> still decoding when the reading view lands is 0 px tall and pushes the
  // anchored block down when its bytes arrive: re-land on each late load while this
  // landing is the latest, the tab still reads, and nobody scrolled since (<= settle ms)
  function readSettle(g, line, ms) {
    const P = g.preview, t = tabOf(g);
    const ims = [...P.querySelectorAll("img")].filter(i => !i.complete);
    const seq = P._spSeq = (P._spSeq || 0) + 1;
    if (!ims.length) return;
    const t0 = performance.now(); let at = P.scrollTop;
    const again = () => {
      if (P._spSeq !== seq || performance.now() - t0 > ms) return;
      if (tabOf(g) !== t || !t || t.mode !== "reading" || g.preview !== P) return;
      if (Math.abs(P.scrollTop - at) >= 1) { P._spSeq++; return; }   // the user scrolled
      readLand(g, line); at = P.scrollTop;
    };
    for (const i of ims) { i.addEventListener("load", again, { once: true }); i.addEventListener("error", again, { once: true }); }
  }

  /* ---------- CM6 surface ---------- */
  const scrollerOf = view => view && view.dom && view.dom.parentElement;   // g.lp: view.dom is its only child
  const shown = sc => !!sc && sc.isConnected && sc.clientHeight > 0;
  // the DRAWN element holding doc position `pos` (a child of contentDOM), or null when
  // the line is not drawn. Its layout box is the truth Ed.topLine captured from; the
  // height map can still carry a stale (pre-restyle) or estimated height for it.
  function rowEl(view, pos) {
    if (pos < view.viewport.from || pos > view.viewport.to) return null;
    let n, o; try { ({ node: n, offset: o } = view.domAtPos(pos)); } catch (e) { return null; }
    const c = view.contentDOM; if (n === c) n = c.childNodes[o] || null;
    while (n && n.parentNode !== c) n = n.parentNode;
    if (!n || n.nodeType !== 1 || n.classList.contains("cm-gap")) return null;
    try { if (view.posAtDOM(n, 0) > pos) return null; } catch (e) { return null; }   // a neighbour, not the row
    return n;
  }
  // the target y, in px from the scroller's top edge: the drawn row's layout box when
  // the line is drawn (same oracle as the capture), else the CURRENT height map
  function targetDy(view, p) {
    const sc = scrollerOf(view), d = view.state.doc;
    const i = clamp(Math.floor(p.line), 0, d.lines - 1), f = clamp(p.line - i, 0, 1);
    const b = view.lineBlockAt(d.line(i + 1).from);
    // a block widget can hold several source lines: the position is spread over them
    const a = d.lineAt(b.from).number - 1, z = d.lineAt(Math.max(b.from, b.to)).number - 1;
    const fr = clamp((i - a + f) / (z - a + 1), 0, 1);
    const st = sc.getBoundingClientRect().top, el = rowEl(view, b.from), r = el && el.getBoundingClientRect();
    const dom = r && r.height > 0;
    const top = dom ? r.top - st : view.documentTop + b.top - st;   // block top vs the scroller's top edge
    const h = dom ? r.height : b.height;
    if (p.kind === "centre") return top - sc.clientTop - (sc.clientHeight - h) / 2;
    return top + fr * h;
  }
  // THE landing: called by CM6 itself (scrollHandler, inside its measure loop)
  function land(view, p) {
    const sc = scrollerOf(view); if (!shown(sc)) return;
    const max = Math.max(0, sc.scrollHeight - sc.clientHeight);
    sc.scrollTop = clamp(sc.scrollTop + targetDy(view, p), 0, max);
    p.st = sc.scrollTop; p.n++;
    if (p.done) { const d = p.done; p.done = null; try { d(); } catch (e) { console.error(e); } }
    verify(view, p);
  }
  // layout can move the landed row WITHOUT a CM6 geometry update (the .lp.src restyle
  // re-flows drawn rows CM6 has not re-measured yet): check the drawn row's own box for
  // a few frames after each landing and re-land through CM6 when it moved. Bounded
  // (VERIFY_FRAMES per landing, MAX_RELAND total); a read, never a write.
  function verify(view, p) {
    if (p.vf) return; p.vf = true; let k = 0;
    const tick = () => {
      if (view._osPin !== p || performance.now() > p.until) { p.vf = false; return; }
      if (p.armed) { requestAnimationFrame(tick); return; }
      const sc = scrollerOf(view);
      if (shown(sc)) {
        const dy = targetDy(view, p), max = Math.max(0, sc.scrollHeight - sc.clientHeight);
        if (Math.abs(dy) >= 1 && !(dy > 0 && sc.scrollTop >= max - 0.5) && !(dy < 0 && sc.scrollTop <= 0.5)) {
          if (p.n > MAX_RELAND) { view._osPin = null; p.vf = false; return; }
          k = 0; view.requestMeasure(); dispatchPin(view, p);
        }
      }
      if (++k > VERIFY_FRAMES) { p.vf = false; return; }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }
  function dispatchPin(view, p) {
    const d = view.state.doc, i = clamp(Math.floor(p.line), 0, d.lines - 1);
    p.pos = d.line(i + 1).from; p.armed = true;
    view.dispatch({ effects: CMv.EditorView.scrollIntoView(p.pos, { y: "start" }) });
  }
  function editLand(g, line, kind, ms, done) {
    const view = Ed.cm(g); if (!view || line == null || !isFinite(line)) return false;
    const p = { line: Math.max(0, line), kind, armed: false, pos: 0, st: -1, n: 0, until: performance.now() + ms, done: done || null };
    view._osPin = p;
    dispatchPin(view, p);
    return true;
  }
  SP.ext = function () {
    return [
      CMv.EditorView.scrollHandler.of((view, range) => {
        const p = view._osPin;
        if (!p) return false;
        if (!p.armed || range.head !== p.pos || !range.empty) { view._osPin = null; return false; }   // someone else scrolls (caret, find): they win
        p.armed = false; land(view, p);
        return true;
      }),
      // late height changes (image decoded, widget measured, lines drawn above the
      // landing) while pinned: re-land through CM6 once the update is over
      CMv.EditorView.updateListener.of(u => {
        const view = u.view, p = view._osPin;
        if (!p || p.armed || !u.geometryChanged) return;
        if (performance.now() > p.until || p.n > MAX_RELAND) { view._osPin = null; return; }
        const sc = scrollerOf(view); if (!shown(sc)) return;
        queueMicrotask(() => {
          if (view._osPin !== p || p.armed || !shown(sc)) return;
          const dy = targetDy(view, p), max = Math.max(0, sc.scrollHeight - sc.clientHeight);
          if (Math.abs(dy) < 1 || (dy > 0 && sc.scrollTop >= max - 0.5) || (dy < 0 && sc.scrollTop <= 0.5)) return;   // still right (or clamped)
          dispatchPin(view, p);
        });
      }),
    ];
  };
  SP.wire = function (lp) {   // user intent on the scroller unpins: the user's scroll outranks a restore
    if (!lp || lp._spWired) return;
    lp._spWired = true;
    const off = () => { const v = lp._cm; if (v && v._osPin) v._osPin = null; };
    for (const ev of ["wheel", "pointerdown", "touchstart", "keydown"]) lp.addEventListener(ev, off, { capture: true, passive: true });
  };
  SP.cancel = function (g) {
    const v = g && Ed.cm(g); if (v) v._osPin = null;
    if (g && g.preview) { g.preview._spSeq = (g.preview._spSeq || 0) + 1; g.preview._spAt = null; }
  };

  /* ---------- the API ---------- */
  SP.capture = function (g, mode) {
    const m = mode || modeOf(g); if (!m || !g) return null;
    if (!shown(m === "reading" ? g.preview : g.lp)) return null;   // a hidden surface has no geometry: no position beats a wrong one
    const line = m === "reading" ? readTop(g) : Ed.topLine(g, 1);   // Ed.topLine: drawn rows first, CM6 height map in gaps
    return line == null || !isFinite(line) ? null : { line };
  };
  SP.restore = function (g, pos, opts) {
    const line = lineOf(pos), m = modeOf(g), ms = opts && opts.settle != null ? opts.settle : SETTLE_MS;
    if (!m || line == null || !isFinite(line)) return false;
    if (m === "reading") { const ok = readLand(g, line); if (ok) readSettle(g, line, ms); return ok; }
    return editLand(g, line, "top", ms, opts && opts.done);
  };
  SP.centre = function (g, line, opts) {
    const m = modeOf(g), ms = opts && opts.settle != null ? opts.settle : SETTLE_MS;
    if (!m || m === "reading" || line == null) return false;
    return editLand(g, Math.floor(line), "centre", ms, opts && opts.done);
  };
  // census only (goal scroll-model): the editor's OWN idea of its top line, beside the
  // scrollpos phase's pixel read: [sptop:<DOM rows>/<CM6 height map>/<scrollTop>/<pin>]
  // (1-based lines). A pixel read and the DOM disagreeing is a paint/oracle question;
  // the DOM and the height map disagreeing is a stale-map question.
  SP.diag = function (g) {
    const view = g && Ed.cm(g); if (!view || !shown(g.lp)) return "";
    const y = g.lp.getBoundingClientRect().top + 1, t = Ed.topLine(g, 1), h = Ed.lineAtY(g, y), p = view._osPin;
    const f = x => (x == null || !isFinite(x) ? "-" : (x + 1).toFixed(2));
    return " [sptop:" + f(t) + "/" + f(h ? h.l + h.f : null) + "/" + Math.round(g.lp.scrollTop) + "/" + (p ? (p.armed ? "a" : "p") + p.n : "0") + "]";
  };
  SP.line = pos => lineOf(pos);
  // reading view: a block element at the top (outline / anchor jump to a heading or ^block)
  SP.toElement = function (g, el) {
    const P = g && g.preview; if (!P || !el || !P.contains(el)) return false;
    P._spSeq = (P._spSeq || 0) + 1; P._spAt = null;   // supersedes a pending image re-land and the landed position
    P.scrollTop = el.offsetTop - P.offsetTop;
    return true;
  };
  // reading view: centre a block (search hit), clamped to the scroller's own range (R25.13c)
  SP.centreElement = function (g, el) {
    const P = g && g.preview; if (!P || !el) return false;
    P._spSeq = (P._spSeq || 0) + 1; P._spAt = null;
    const top = el.offsetTop - P.offsetTop, h = el.offsetHeight;
    P.scrollTop = Math.max(0, Math.min(top - (P.clientHeight - h) / 2, Math.max(0, P.scrollHeight - P.clientHeight)));
    return true;
  };
  return SP;
})();
