// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* MATH (#185) — $inline$ and $$block$$ TeX, typeset by the vendored MathJax (ui/vendor/mathjax,
   3.2.2 tex-chtml-full, VENDOR.md), as Obsidian shows it (stock 1.13.7, black box):
   READING VIEW  MathView.reading(root, quiet): the Rust renderer emits span.math.math-inline /
                 span.math.math-display holding the TeX as escaped TEXT (src-tauri main.rs
                 render_with_br; "$ a$", "\$5" and code stay text). Synchronously a display span
                 becomes div.math.math-block (replacing its paragraph when it is alone in it); once
                 MathJax is in, each element gets its mjx-container and the class is-loaded.
   LIVE PREVIEW  Ed.mathField(v, G, bust): $..$ on a line -> an inline widget span.math, a $$ .. $$
                 block -> a block widget div.math.math-block.cm-embed-block. A selection touching
                 the math reveals its source (as stock); source mode and the reading view's laid-out
                 editor render none; fences, inline code, escapes and frontmatter are skipped.
   MathJax loads lazily: only a note that holds math asks for it, through the plugin API's loader
   (window.__osMath.loadMathJax, api-functions.js) so app and plugins share ONE instance and any gate
   on that loader (Settings > Security) also covers this renderer; a refused load leaves the TeX as
   text.
   Security: the TeX is never parsed as HTML (textContent in, MathJax's own DOM out). MathJax's TeX
   html extension can still put attributes on its output (\href, \cssId, \class, \style, \data), so
   every typeset node is cleaned: no id, no href/src/on* attribute, only MathJax's own class names
   (mjx-*, MJX-*, TEX-*, MathJax), no data-* but data-mjx-*, and no style property that could lift
   the formula out of its box (position, inset, z-index, transform, url() ...). \require is off
   (loader config). Cost: a formula over MAX characters stays text, at most MAXN per render, and the
   reading view yields between batches. */
(function () {
  "use strict";
  const MAX = 64 * 1024, MAXN = 5000;
  const mjNow = () => { const M = window.MathJax; return M && M.startup && M.startup.document && typeof M.tex2chtml === "function" ? M : null; };
  let loading = null;
  function load() {
    const M = mjNow(); if (M) return Promise.resolve(M);
    const L = window.__osMath;
    if (!L || typeof L.loadMathJax !== "function") return Promise.reject(new Error("MathJax loader unavailable"));
    if (!loading) loading = L.loadMathJax().finally(() => { loading = null; });
    return loading;
  }
  const OKCLS = /^(mjx-|MJX-|TEX-|MathJax$)/;
  const BADCSS = /^(position|top|left|right|bottom|inset.*|z-index|transform.*|translate|rotate|scale|perspective.*|filter|backdrop-filter|clip-path|mask.*|content|cursor|pointer-events|behavior|-webkit-.*)$/i;
  const DROP = /^(id|href|xlink:href|src|srcset|srcdoc|action|formaction|on.*)$/i;
  function clean(root) {
    // \href: MathJax wraps the formula part in a link; keep its content, drop the link
    for (const a of [...root.querySelectorAll("a")]) a.replaceWith(...a.childNodes);
    for (const e of [root, ...root.querySelectorAll("*")]) {
      for (const a of [...e.attributes]) {
        const n = a.name;
        if (DROP.test(n)) { e.removeAttribute(n); continue; }
        if (/^data-/i.test(n) && !/^data-mjx-/i.test(n)) { e.removeAttribute(n); continue; }
        if (n === "class") {
          const k = a.value.split(/\s+/).filter(c => c && OKCLS.test(c));
          if (k.length) e.setAttribute("class", k.join(" ")); else e.removeAttribute("class");
        }
      }
      const s = e.style;
      if (s && s.length) for (const p of [...s]) if (BADCSS.test(p) || /url\(|expression\(|image-set\(/i.test(s.getPropertyValue(p))) s.removeProperty(p);
    }
    return root;
  }
  // one formula -> a cleaned mjx-container, or null (MathJax threw: the TeX stays text)
  function typeset(M, tex, display) {
    if (tex.length > MAX) return null;
    try { return clean(M.tex2chtml(tex, { display: !!display })); } catch (e) { console.warn("[math] " + (e && e.message || e)); return null; }
  }
  // the CHTML stylesheet for what was typeset so far (fonts/sizes it now uses), batched per tick
  let flushT = 0;
  function flush() {
    if (flushT) return;
    flushT = setTimeout(() => {
      flushT = 0;
      const M = mjNow(); if (!M) return;
      try { const d = M.startup.document; d.clear(); d.updateDocument(); } catch (e) { console.warn("[math] stylesheet: " + (e && e.message || e)); }
    }, 0);
  }
  const tick = () => new Promise(r => setTimeout(r, 0));

  /* reading view. quiet(fn): the caller's wrapper for DOM writes (main.js keeps its retained-DOM key
     valid across them); resolves with the number of formulas typeset. */
  async function reading(root, quiet) {
    const run = typeof quiet === "function" ? quiet : fn => fn();
    const todo = [];
    for (const s of root.querySelectorAll("span.math.math-inline, span.math.math-display")) {
      if (s.closest("pre, code")) continue;
      if (s.classList.contains("math-inline")) { todo.push({ el: s, display: false }); continue; }
      const d = document.createElement("div");
      d.className = "math math-block";
      d.textContent = s.textContent;
      const p = s.parentElement;
      if (p && p.tagName === "P" && [...p.childNodes].every(n => n === s || n.nodeName === "BR" || (n.nodeType === 3 && !n.data.trim()))) {
        for (const a of p.attributes) if (a.name !== "class") d.setAttribute(a.name, a.value);   // data-line & co (scroll sync)
        p.replaceWith(d);
      } else s.replaceWith(d);
      todo.push({ el: d, display: true });
    }
    if (!todo.length) return 0;
    let M;
    try { M = await load(); } catch (e) { console.warn("[math] " + (e && e.message || e)); return 0; }
    let n = 0;
    for (const { el, display } of todo.slice(0, MAXN)) {
      if (!el.isConnected || el.classList.contains("is-loaded")) continue;
      const node = typeset(M, el.textContent, display);
      if (!node) continue;
      run(() => { el.replaceChildren(node); el.classList.add("is-loaded"); });
      if (++n % 64 === 0) { flush(); await tick(); }
    }
    if (n) { flush(); pub(); }
    return n;
  }

  /* the math of a doc, for Live Preview: blocks [{from, to, tex}] ($$ lines) and inline
     [{from, to, tex, display}] ($..$, $$..$$ inside a line). Pure on the line texts. */
  function scanLines(lines) {
    const blocks = [], inl = [];
    let fence = null, mb = null, fm = lines.length > 0 && /^---\s*$/.test(lines[0]);
    for (let i = 0; i < lines.length; i++) {
      const t = lines[i];
      if (fm) { if (i > 0 && /^(---|\.\.\.)\s*$/.test(t)) fm = false; continue; }
      if (mb) {
        const e = t.trimEnd();
        if (e.endsWith("$$")) { mb.tex.push(e.slice(0, -2)); blocks.push({ l0: mb.l0, l1: i, tex: mb.tex.join("\n").trim() }); mb = null; }
        else mb.tex.push(t);
        continue;
      }
      const f = /^ {0,3}(`{3,}|~{3,})/.exec(t);
      if (fence) { if (f && f[1][0] === fence[0] && f[1].length >= fence.length && !t.slice(f[0].length).trim()) fence = null; continue; }
      if (f) { fence = f[1]; continue; }
      if (/^ {4,}|^\t/.test(t)) continue;   // indented code
      const b = /^ {0,3}\$\$/.exec(t);
      if (b) {
        const rest = t.slice(b[0].length), e = rest.trimEnd();
        if (e.endsWith("$$") && e.length >= 2) { const tex = e.slice(0, -2).trim(); if (tex) blocks.push({ l0: i, l1: i, tex }); continue; }
        mb = { l0: i, tex: [rest] };
        continue;
      }
      inlineIn(t, i, inl);
    }
    return { blocks, inl };
  }
  const WS = c => c === undefined || /\s/.test(c);
  function inlineIn(t, li, out) {
    for (let i = 0; i < t.length; i++) {
      const c = t[i];
      if (c === "\\") { i++; continue; }
      if (c === "`") {
        let n = 1; while (t[i + n] === "`") n++;
        let j = i + n, hit = -1;
        while (j < t.length) { if (t[j] === "`") { let m = 1; while (t[j + m] === "`") m++; if (m === n) { hit = j + m; break; } j += m; } else j++; }
        i = hit < 0 ? i + n - 1 : hit - 1;
        continue;
      }
      if (c !== "$") continue;
      if (t[i + 1] === "$") {   // $$..$$ inside a line: display math
        const j = t.indexOf("$$", i + 2);
        if (j > i + 2) { out.push({ li, a: i, b: j + 2, tex: t.slice(i + 2, j), display: true }); i = j + 1; } else i++;
        continue;
      }
      if (WS(t[i + 1])) continue;   // "$ a$": not an opener
      let k = i + 1, hit = -1;
      for (; k < t.length; k++) {
        if (t[k] === "\\") { k++; continue; }
        if (t[k] === "$") { if (!WS(t[k - 1])) { hit = k; break; } }
      }
      if (hit < 0) continue;
      out.push({ li, a: i, b: hit + 1, tex: t.slice(i + 1, hit), display: false });
      i = hit;
    }
  }
  let scDoc = null, scOut = null;
  function scan(doc) {
    if (scDoc === doc) return scOut;
    const lines = [];
    for (let i = 1; i <= doc.lines; i++) lines.push(doc.line(i).text);
    const r = scanLines(lines);
    const out = {
      blocks: r.blocks.map(b => ({ from: doc.line(b.l0 + 1).from, to: doc.line(b.l1 + 1).to, tex: b.tex })),
      inl: r.inl.map(m => { const f = doc.line(m.li + 1).from; return { from: f + m.a, to: f + m.b, tex: m.tex, display: m.display }; }),
    };
    scDoc = doc; scOut = out;
    return out;
  }

  let fx = null;
  // MathJax arrived: every editor rebuilds its math widgets
  function refresh() {
    pub();
    if (!fx || typeof CMv === "undefined" || !CMv.EditorView) return;
    for (const el of document.querySelectorAll(".cm-editor")) {
      const cm = CMv.EditorView.findFromDOM(el);
      if (cm) try { cm.dispatch({ effects: fx.of(null) }); } catch (e) { console.error(e); }
    }
  }
  let asked = false;
  function want() {
    if (asked || mjNow()) return;
    asked = true;
    load().then(() => setTimeout(refresh, 0), e => { asked = false; console.warn("[math] " + (e && e.message || e)); });
  }
  function mathField(v, G, bust) {
    if (typeof CMv === "undefined" || typeof CMs === "undefined") return [];
    const { Decoration, WidgetType, EditorView } = CMv;
    if (!fx) fx = CMs.StateEffect.define();
    class MW extends WidgetType {
      constructor(tex, block, gen) { super(); this.tex = tex; this.block = block; this.gen = gen; }
      eq(o) { return o.tex === this.tex && o.block === this.block && o.gen === this.gen; }
      get estimatedHeight() { return this.block ? 48 : -1; }
      toDOM(view) {
        const w = document.createElement(this.block ? "div" : "span");
        w.className = this.block ? "math math-block cm-embed-block" : "math";
        w.setAttribute("contenteditable", "false");
        const M = mjNow(), node = M ? typeset(M, this.tex, this.block) : null;
        if (node) { w.appendChild(node); w.classList.add("is-loaded"); flush(); }
        else w.textContent = this.block ? "$$" + this.tex + "$$" : "$" + this.tex + "$";
        // the widget ignores editor events: a click puts the caret into the math (its source shows)
        w.addEventListener("mousedown", e => {
          if (e.button !== 0) return;
          e.preventDefault();
          try { const p = view.posAtDOM(w, 0); view.dispatch({ selection: { anchor: this.block ? p : p + 1 } }); view.focus(); } catch (_) {}
        });
        return w;
      }
      ignoreEvent() { return true; }
    }
    const build = st => {
      const g = G();
      const tb = g && g.view === v && g.active >= 0 ? g.tabs[g.active] : null;
      if (!g || (v.lp && v.lp.classList.contains("src")) || (tb && tb.mode === "reading")) return Decoration.none;
      const { blocks, inl } = scan(st.doc);
      if (!blocks.length && !inl.length) return Decoration.none;
      if (!mjNow()) want();
      const gen = mjNow() ? 1 : 0, out = [], sel = st.selection.ranges;
      const touched = (a, b) => sel.some(r => r.from <= b && r.to >= a);
      for (const b of blocks) if (!touched(b.from, b.to) && b.tex.length <= MAX)
        out.push(Decoration.replace({ widget: new MW(b.tex, true, gen), block: true }).range(b.from, b.to));
      for (const m of inl) if (!touched(m.from, m.to) && m.tex.length <= MAX)
        out.push(Decoration.replace({ widget: new MW(m.tex, m.display, gen) }).range(m.from, m.to));
      return out.length ? Decoration.set(out, true) : Decoration.none;
    };
    return CMs.StateField.define({
      create: st => { try { return build(st); } catch (e) { console.error(e); return Decoration.none; } },
      update(d, tr) {
        if (tr.docChanged || tr.selection || tr.effects.some(e => e.is(bust) || e.is(fx)))
          try { return build(tr.state); } catch (e) { console.error(e); return Decoration.none; }
        return d;
      },
      provide: f => EditorView.decorations.from(f),
    });
  }

  /* census (gate phase mathjax, CENSUS_ON only): the focused pane's PAINTED math, off the live DOM.
     [mth:<rv|ed>|M<1 MathJax loaded, else 0>|i<inline>/b<block>/l<with an mjx-container>|d<1: the
     first block's container is display="true">] [mthxss:n] elements inside math that carry what TeX
     must never produce (id, href/src/on*, a foreign class, data-*, position/transform style, or an
     img/script/iframe/object/embed/a element); [mthrc:x,y,w,h] the first typeset block's rect. */
  const vis = e => !!e && !e.closest(".vkeep") && e.getClientRects().length > 0;
  function bad(e) {
    if (/^(img|script|iframe|object|embed|a|foreignobject)$/i.test(e.tagName)) return true;
    for (const a of e.attributes) {
      if (DROP.test(a.name) || (/^data-/i.test(a.name) && !/^data-mjx-/i.test(a.name))) return true;
      if (a.name === "class" && a.value.split(/\s+/).some(c => c && !OKCLS.test(c) && !/^(math|math-inline|math-block|math-display|is-loaded|cm-embed-block)$/.test(c))) return true;
    }
    const s = e.style;
    if (s && s.length) for (const p of s) if (BADCSS.test(p)) return true;
    return false;
  }
  function tok() {
    const Pn = document.querySelector("#main .pane.focused") || document.querySelector("#main .pane");
    if (!Pn) return "";
    const M = mjNow() ? 1 : 0;
    const pv = [...Pn.querySelectorAll(".preview")].find(vis);
    let kind, els;
    if (pv) { kind = "rv"; els = [...pv.querySelectorAll(".math")].filter(vis); }
    else {
      const ed = [...Pn.querySelectorAll(".cm-editor")].find(vis);
      if (!ed) return " [mth:-|M" + M + "|]";
      kind = "ed"; els = [...ed.querySelectorAll(".cm-content .math")].filter(vis);
    }
    const blk = els.filter(e => e.classList.contains("math-block")), inl = els.filter(e => !e.classList.contains("math-block"));
    const ld = els.filter(e => e.querySelector(":scope > mjx-container")).length;
    let x = 0;
    for (const e of els) for (const d of [e, ...e.querySelectorAll("*")]) if (d !== e && bad(d)) x++;
    const b0 = blk.find(e => e.querySelector(":scope > mjx-container"));
    const c0 = b0 && b0.querySelector(":scope > mjx-container");
    const rc = b0 ? (r => [r.x, r.y, r.width, r.height].map(Math.round).join(","))(b0.getBoundingClientRect()) : "";
    return " [mth:" + kind + "|M" + M + "|i" + inl.length + "/b" + blk.length + "/l" + ld + "|d" + (c0 && c0.getAttribute("display") === "true" ? 1 : 0) + "]" +
           " [mthxss:" + x + "]" + (rc ? " [mthrc:" + rc + "]" : "");
  }
  const pub = () => { try { if (typeof updateTitle === "function") updateTitle(); } catch (_) {} };

  window.MathView = Object.freeze({ reading, tok, scanLines, MAX });
  if (typeof Ed !== "undefined" && Ed) Ed.mathField = mathField;
})();
