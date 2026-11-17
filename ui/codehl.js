// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* CODE HIGHLIGHTING (#186) — fenced code blocks, coloured by the vendored Prism (ui/vendor/prism,
   1.30.0 + every language component of that release, VENDOR.md).
   READING VIEW  CodeHL.reading(root, quiet): every `pre > code.language-L` the Rust renderer emitted
                 (escaped text only) gets pre.language-L + code.is-loaded (as Obsidian marks a fence),
                 then, once Prism is in, its text is replaced by Prism's token spans.
   LIVE PREVIEW  Ed.cbHl(v, G): a decoration layer marking the body of every closed ```L fence with
                 the same token classes (span.token.<type>, the colours style.css already maps to
                 --code-*), in source mode too. A fence a plugin code-block processor renders is left
                 to that processor (Ed.cbHook), as in the reading view.
   Prism loads lazily: only a note that holds a fence with a language asks for it, through the plugin
   API's loader (window.__osLibs.loadPrism, api-functions.js) so app and plugins share ONE instance and
   any gate on that loader (Settings › Security) also covers this renderer; a refused load leaves
   the blocks plain.
   Security: the source is NEVER parsed as HTML. Prism's token stream is turned into DOM here
   (text nodes + span.token.*, class names restricted to [\w-]), no innerHTML, no Prism.highlight
   string. Prism stays in manual mode (no highlightAll), its worker handler off.
   Cost: a block over MAX characters stays plain, and the reading view yields to the event loop
   between blocks, so a 5 MB fence never freezes the UI. */
(function () {
  "use strict";
  const MAX = 256 * 1024;
  const langOf = el => { const m = /(?:^|\s)language-(\S+)/.exec((el && el.className) || ""); return m ? m[1] : ""; };
  const libs = () => window.__osLibs || null;
  const prismNow = () => { const P = window.Prism; return P && typeof P.tokenize === "function" && P.languages && P.hooks ? P : null; };
  function load() {
    const P = prismNow(); if (P && P.languages.rust) return Promise.resolve(P);
    const L = libs();
    if (!L || typeof L.loadPrism !== "function") return Promise.reject(new Error("Prism loader unavailable"));
    return L.loadPrism();
  }
  const grammarOf = (P, lang) => { const g = P.languages[lang] || P.languages[String(lang).toLowerCase()]; return g && typeof g === "object" ? g : null; };
  // Prism's tokenize with the language hooks (markup-templating: php, django, ... rely on them); null = unknown language
  function tokens(P, code, lang) {
    const grammar = grammarOf(P, lang);
    if (!grammar) return null;
    const env = { code, grammar, language: String(lang).toLowerCase() };
    P.hooks.run("before-tokenize", env);
    if (!env.grammar) return null;
    env.tokens = P.tokenize(env.code, env.grammar);
    P.hooks.run("after-tokenize", env);
    return env.tokens;
  }
  const CLS = /^[\w-]+$/;
  const clsOf = t => ["token", t.type].concat(t.alias ? (Array.isArray(t.alias) ? t.alias : [t.alias]) : [])
    .filter(c => typeof c === "string" && CLS.test(c)).join(" ");
  function dom(parent, t) {
    if (typeof t === "string") { parent.appendChild(document.createTextNode(t)); return; }
    if (Array.isArray(t)) { for (const x of t) dom(parent, x); return; }
    if (!t || typeof t !== "object") return;
    const s = document.createElement("span");
    s.className = clsOf(t);
    dom(s, t.content);
    parent.appendChild(s);
  }
  const plain = t => typeof t === "string" ? t : Array.isArray(t) ? t.map(plain).join("") : t && typeof t === "object" ? plain(t.content) : "";
  const procOwns = lang => { const h = typeof Ed !== "undefined" && Ed && Ed.cbHook; try { return !!(h && h.has(lang)); } catch (_) { return false; } };
  const tick = () => new Promise(r => setTimeout(r, 0));

  /* reading view. quiet(fn): the caller's wrapper for DOM writes (main.js keeps its retained-DOM
     key valid across them); resolves with the number of blocks highlighted. */
  async function reading(root, quiet) {
    const run = typeof quiet === "function" ? quiet : fn => fn();
    const todo = [];
    for (const code of root.querySelectorAll("pre > code")) {
      const lang = langOf(code);
      if (!lang) continue;
      const pre = code.parentElement;
      if (!pre.classList.contains("language-" + lang)) pre.classList.add("language-" + lang);
      code.classList.add("is-loaded");
      if (!code.firstElementChild && !procOwns(lang)) todo.push({ code, lang });
    }
    if (!todo.length) return 0;
    let P;
    try { P = await load(); } catch (e) { console.warn("[codehl] " + (e && e.message || e)); return 0; }
    let n = 0, budget = 0;
    for (const { code, lang } of todo) {
      if (!code.isConnected || code.firstElementChild) continue;
      const src = code.textContent;
      if (src.length > MAX) continue;
      let toks;
      try { toks = tokens(P, src, lang); } catch (e) { console.warn("[codehl] " + lang + ": " + (e && e.message || e)); continue; }
      if (!toks || plain(toks) !== src) continue;   // a hook that changed the text: keep the source as it is
      const frag = document.createDocumentFragment();
      dom(frag, toks);
      run(() => code.replaceChildren(frag));
      n++;
      budget += src.length;
      if (budget > 32 * 1024) { budget = 0; await tick(); }
    }
    if (n) pub();
    return n;
  }

  /* live preview / source mode: a mark layer over fence bodies (Ed.cbFences). */
  function lpLayer(v, G) {
    if (typeof CMv === "undefined" || typeof CMs === "undefined" || !CMv.ViewPlugin) return [];
    const { Decoration, ViewPlugin } = CMv;
    const fx = CMs.StateEffect.define();
    const marks = new Map();
    const mk = c => { let d = marks.get(c); if (!d) { d = Decoration.mark({ class: c }); marks.set(c, d); } return d; };
    /* lang \0 body -> [[s, e, cls]]. Only what the LAST build painted survives: each build starts a
       fresh map and carries over the entries it reuses, so an edited-away body is dropped by the next
       build and the cache never holds more than the visible fences (each <= MAX). */
    let cache = new Map(), next = new Map();
    const spans = (P, lang, body) => {
      const k = lang + "\0" + body;
      let r = cache.get(k);
      if (r) { next.set(k, r); return r; }
      r = [];
      const toks = tokens(P, body, lang);
      if (toks && plain(toks) === body) {
        const walk = (t, at) => {
          if (typeof t === "string") return at + t.length;
          if (Array.isArray(t)) { for (const x of t) at = walk(x, at); return at; }
          if (!t || typeof t !== "object") return at;
          const s = at, e = walk(t.content, at);
          if (e > s) r.push([s, e, "cm-hmd-codeblock " + clsOf(t)]);
          return e;
        };
        walk(toks, 0);
      }
      next.set(k, r);
      return r;
    };
    let want = false;
    const build = view => {
      next = new Map();
      try { return build0(view); } finally { cache = next; next = new Map(); }
    };
    const build0 = view => {
      const doc = view.state.doc, blocks = typeof Ed !== "undefined" && Ed.cbFences ? Ed.cbFences(doc) : [];
      if (!blocks.length) return Decoration.none;
      const P = prismNow(), out = [], vr = view.viewport;
      for (const b of blocks) {
        if (b.to < vr.from || b.from > vr.to) continue;
        if (procOwns(b.lang)) continue;   // its widget (Ed.cbField) shows instead; with the caret in it, plain source
        const l0 = doc.lineAt(b.from), l1 = doc.lineAt(b.to);
        if (l1.number - l0.number < 2) continue;
        const from = doc.line(l0.number + 1).from, to = doc.line(l1.number - 1).to;
        if (to - from > MAX) continue;
        if (!P) { want = true; continue; }
        if (!grammarOf(P, b.lang)) { if (!P.languages.rust) want = true; continue; }   // a plugin's own Prism without our languages: ask the loader once
        let sp;
        try { sp = spans(P, b.lang, doc.sliceString(from, to)); } catch (e) { continue; }
        for (const [s, e, c] of sp) out.push(mk(c).range(from + s, from + e));
      }
      return out.length ? Decoration.set(out, true) : Decoration.none;
    };
    return ViewPlugin.fromClass(class {
      constructor(view) { this.view = view; this.decorations = this.safe(view); }
      safe(view) {
        want = false;
        let d;
        try { d = build(view); } catch (e) { console.error(e); d = Decoration.none; }
        if (want && !this.asked) {
          this.asked = true;
          load().then(() => { if (!this.gone) try { this.view.dispatch({ effects: fx.of(null) }); } catch (_) {} pub(); }, e => console.warn("[codehl] " + (e && e.message || e)));
        }
        return d;
      }
      update(u) {
        if (u.docChanged || u.viewportChanged || u.transactions.some(t => t.effects.some(e => e.is(fx) || (Ed.cbFx && e.is(Ed.cbFx)))))
          this.decorations = this.safe(u.view);
      }
      destroy() { this.gone = true; }
    }, { decorations: p => p.decorations });
  }

  /* census (gate phase codehl, CENSUS_ON only): the focused pane's PAINTED code, off the live DOM.
     [chl:<rv|ed>|P<n>|<blocks>] n = grammars loaded (0 = Prism never loaded); rv blocks =
     <lang>/<pre has language-L><code is-loaded>/<token spans>,... ("-" = a fence without a language);
     ed blocks = t<token spans in the editor>. [chlkw:r.g.b] the computed colour of the first painted
     .token.keyword, [chlnorm:r.g.b] of the first block's plain code text; [chlxss:n] elements in any
     code block that the source text asked for (img/script/iframe/object/embed or an on* attribute);
     [chlrc:x,y,w,h] the first painted keyword's rect. */
  const vis = e => !!e && !e.closest(".vkeep") && e.getClientRects().length > 0 && e.getBoundingClientRect().height > 0;
  const rgb = c => { const m = /rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(c || ""); return m ? m[1] + "." + m[2] + "." + m[3] : "-"; };
  const xssIn = els => { let n = 0; for (const r of els) for (const e of r.querySelectorAll("*")) { if (/^(img|script|iframe|object|embed)$/i.test(e.tagName)) { n++; continue; } for (const a of e.attributes) if (/^on/i.test(a.name)) { n++; break; } } return n; };
  function tok() {
    const Pn = document.querySelector("#main .pane.focused") || document.querySelector("#main .pane");
    if (!Pn) return "";
    const P = prismNow(), np = P ? Object.keys(P.languages).filter(k => P.languages[k] && typeof P.languages[k] === "object").length : 0;
    const rect = e => { const r = e.getBoundingClientRect(); return [r.x, r.y, r.width, r.height].map(Math.round).join(","); };
    const pres = [...Pn.querySelectorAll(".preview pre")].filter(p => vis(p) && p.querySelector(":scope > code"));
    let out, roots, kw;
    if (pres.length) {
      out = "rv|P" + np + "|" + pres.slice(0, 16).map(p => {
        const c = p.querySelector(":scope > code"), l = langOf(c);
        return (l ? l.replace(/[^\w+#-]/g, "_").slice(0, 16) : "-") + "/" + (l && p.classList.contains("language-" + l) ? 1 : 0) +
               (c.classList.contains("is-loaded") ? 1 : 0) + "/" + c.querySelectorAll(".token").length;
      }).join(",");
      roots = pres;
      kw = pres.map(p => p.querySelector(".token.keyword")).find(vis);
    } else {
      const ed = [...Pn.querySelectorAll(".cm-editor")].find(vis);
      if (!ed) return " [chl:-|P" + np + "|]";
      roots = [...ed.querySelectorAll(".cm-content")];
      out = "ed|P" + np + "|t" + ed.querySelectorAll(".cm-content .token").length;
      kw = [...ed.querySelectorAll(".cm-content .token.keyword")].find(vis);
    }
    const c0 = pres.length ? pres[0].querySelector(":scope > code") : null;
    return " [chl:" + out + "] [chlxss:" + xssIn(roots) + "]" +
           (kw ? " [chlkw:" + rgb(getComputedStyle(kw).color) + "] [chlrc:" + rect(kw) + "]" : "") +
           (c0 ? " [chlnorm:" + rgb(getComputedStyle(c0).color) + "]" : "");
  }
  const pub = () => { try { if (typeof updateTitle === "function") updateTitle(); } catch (_) {} };

  window.CodeHL = Object.freeze({ reading, MAX, tok, pub });
  if (typeof Ed !== "undefined" && Ed) Ed.cbHl = lpLayer;
})();
