// SPDX-License-Identifier: GPL-3.0-or-later
/* EDITOR — CodeMirror 6 is the editor (CM6 EDITOR CORE, below the literal).
   This literal holds what CM6 does not own:

   TOKENIZER  Ed.row/Ed.block/Ed.inline render ONE source line into DOM with
              the source-mode class vocabulary (mk h bq hr tbl task li code wl
              lt url tag b i bi s hl fence — mirrors src-tauri/src/srcmode.rs
              marker for marker). Live Preview (Ed.lpExt) reads that DOM back
              as CM6 decorations; Ed.nodes maps a rendered row's text nodes to
              source columns (search marks), Ed.tok/tokAt the per-token map
              that drives the per-token marker reveal.
   EDIT OPS   list/task/indent/renumber helpers (Ed.enter, Ed.del, Ed.indent,
              Ed.toggleList, ...) mutate the Ed.lines(g) mirror; Ed.commit turns
              the difference into one CM6 transaction.
   EMBEDS     image / PDF embed elements and the resolve sets for wikilinks.
   CENSUS     [cm:] [sel:] [rv:] [crt:] ... producers for the gate probes.

   The former model/view engine (one contenteditable row per line, beforeinput
   -> model -> dirty-row patching, its own undo stack, lazy rows) is gone:
   CM6 owns input, selection, undo, IME, virtualisation and painting. */
const Ed = {
  cur: null,            // {g, l} row currently holding the caret (census/geometry)
  _tk: null,            // R17.6 token map under construction (Ed.row owns it)
  UNDO_IDLE_MS: 2000,   // R17.8: idle gap that CLOSES an undo group
  PLUGIN_JOIN_MS: 500,  // ED-3: a plugin edit this soon after its previous one joins that undo step

  /* ---------- inline renderer: one row per source line ---------- */
  el(tag, cls) { const e = document.createElement(tag); if (cls) e.className = cls; return e; },
  t(s) { return document.createTextNode(s); },
  mk(s, extra) { const e = Ed.el("span", extra ? "mk " + extra : "mk"); e.appendChild(Ed.t(s)); return e; },

  /* ---------- R17.6 per-line TOKEN MAP ----------
     "the per-line token map MUST carry, for each token, its raw range plus its
     rendered form, so that revealing a token is a per-token class flip on ONE
     line" (R17.6, consequence for the implementation).

     A token is declared by the renderer as the DOM nodes it SPANS (`els`, in
     document order) plus the subset that is hideable markup (`mks`) and a kind
     from the class vocabulary. The raw range is NOT threaded through the
     renderer as an offset: the markers of `**bold**` are siblings of the
     <strong>, not its ancestors, so there is no wrapper to measure — instead
     Ed.row resolves every declared node's raw range in ONE post-pass over the
     finished row (Ed.tokRanges). The map is built ONCE per row render and
     cached on the element, so a caret move is a class flip over the tokens of
     at most the touched lines, never a re-render (R18: this runs on EVERY
     caret move). */
  tok(els, mks, kind) { if (Ed._tk) Ed._tk.push({ els, mks, kind, s: 0, e: 0 }); },
  // raw column range of every element in the row, by walking its text in order
  tokRanges(row, list) {
    if (!list.length) return list;
    const off = new Map();
    const walk = (el, c) => {
      for (const n of el.childNodes) {
        if (n.nodeType === 3) { c += n.nodeValue.length; continue; }
        const s = c;
        c = walk(n, c);
        off.set(n, [s, c]);
      }
      return c;
    };
    walk(row, 0);
    for (const t of list) {
      let s = Infinity, e = -1;
      for (const el of t.els) { const r = off.get(el); if (!r) continue; if (r[0] < s) s = r[0]; if (r[1] > e) e = r[1]; }
      t.s = s === Infinity ? 0 : s; t.e = e < 0 ? t.s : e;
    }
    return list;
  },

  row(g, line, i) {
    const row = Ed.el("div", "lprow");
    row.dataset.l = i;
    const tk = Ed._tk = [];
    try { Ed.block(g, row, line); } finally { Ed._tk = null; }
    row._tok = Ed.tokRanges(row, tk);
    if (!row.firstChild) row.appendChild(document.createElement("br"));
    return row;
  },

  // block-level prefix (heading / quote / hr / table / list / task), then inline
  // nesting cap for "> > > ...": deeper markers render as plain text, so a
  // hostile 200k-">" line costs 32 frames, not 200k (audit #13)
  BQ_MAX: 32,
  block(g, row, s, d = 0) {
    const ind = s.match(/^[ \t]*/)[0], t = s.slice(ind.length);
    const h = /^(#{1,6}) /.exec(t);
    if (h && !ind) {
      const box = Ed.el("span", "h h" + h[1].length);
      const m0 = Ed.mk(h[1]), m1 = Ed.mk(" ");
      box.appendChild(m0);
      box.appendChild(m1);
      Ed.tok([m0, m1], [m0, m1], "h");            // "## " is ONE token (R17.6)
      Ed.inline(g, box, t.slice(h[1].length + 1));
      row.appendChild(box);
      return;
    }
    const tt = t.replace(/\s+$/, "");
    if (tt.length >= 3 && /^(-+|\*+|_+)$/.test(tt)) {          // hr
      const box = Ed.el("span", "hr"), m = Ed.mk(s);
      box.appendChild(m); Ed.tok([m], [m], "hr"); row.appendChild(box); return;
    }
    if (t.startsWith(">")) {                                    // blockquote
      const box = Ed.el("span", "bq");
      if (ind) box.appendChild(Ed.t(ind));
      const m0 = Ed.mk(">"); box.appendChild(m0);
      const qm = [m0];
      let rest = t.slice(1);
      if (rest.startsWith(" ")) { const m1 = Ed.mk(" "); box.appendChild(m1); qm.push(m1); rest = rest.slice(1); }
      Ed.tok(qm, qm, "bq");
      if (d + 1 < Ed.BQ_MAX) Ed.block(g, box, rest, d + 1);     // "> - a": list inside the quote
      else Ed.inline(g, box, rest);                             // past the cap: the rest is text
      row.appendChild(box);
      return;
    }
    if (t.startsWith("|")) {                                    // table row (block render is on demand)
      const box = Ed.el("span", "tbl"); box.appendChild(Ed.t(s)); row.appendChild(box); return;
    }
    if (/^(```|~~~)/.test(t)) {                                 // fence delimiter line
      const box = Ed.el("span", "fence"), m = Ed.mk(s);
      box.appendChild(m); Ed.tok([m], [m], "fence"); row.appendChild(box); return;
    }
    const lm = /^([-*+]|\d+\.) /.exec(t);
    if (lm) {
      const rest = t.slice(lm[0].length);
      if (ind) row.appendChild(Ed.t(ind));
      // the marker glyphs and their trailing space are SEPARATE hidden spans:
      // R15.10 gives .lim a fixed marker advance as a box (bullet drawn
      // inside it), and the space must not sit in that box or a wide "10."
      // would push the text right by its width. textContent is unchanged.
      const li0 = Ed.mk(lm[1], "lim"), li1 = Ed.mk(" ", "lisp");
      row.appendChild(li0);
      row.appendChild(li1);
      row.classList.add("li");
      if (/\d/.test(lm[1])) row.classList.add("ord");
      // deep-tasks: any one status character is a task (Obsidian Tasks "[/]" "[-]" ...); only a BULLET
      // "[x]"/"[X]" line is struck as done (tkx), as the reading view does (rvtask Q5)
      // FZ-E18.7: inside a quote (d > 0) "- [ ] " is a plain list item, as in stock Live Preview: no
      // checkbox, so no click can toggle (stock writes nothing there)
      const task = d === 0 ? /^\[([^\]\n])\] /.exec(rest) : null;
      if (task) {
        row.classList.add("tk");
        if (/[xX]/.test(task[1]) && !/\d/.test(lm[1])) row.classList.add("tkx");
        const cb = Ed.el("input", "tbox");
        cb.type = "checkbox"; cb.checked = task[1] !== " ";
        cb.contentEditable = "false";
        Ed.taskBox(cb);
        row.appendChild(cb);
        const tm = Ed.mk("[" + task[1] + "] ", "task");
        row.appendChild(tm);
        // R17.6 lists "- [ ] " as ONE token: the bullet marker and the
        // checkbox are the same widget, so they reveal together.
        Ed.tok([li0, li1, cb, tm], [li0, li1, tm], "task");
        Ed.inline(g, row, rest.slice(4));
      } else {
        Ed.tok([li0, li1], [li0, li1], "li");
        Ed.inline(g, row, rest);
      }
      return;
    }
    if (ind) row.appendChild(Ed.t(ind));
    Ed.inline(g, row, t);
  },

  // R17.7: "[text](dest)" at the START of s, CommonMark-compatible. Returns
  // [whole, text, dest] (exec-shaped) or null. The destination is SCANNED, not
  // regexed: CommonMark allows BALANCED parens in a link destination
  // (…/wiki/Foo_(bar)), so a ')' closes the destination only at depth 0. The old
  // /^\[([^\[\]]*)\]\(([^)\s]*)\)/ truncated such a URL at the first ')' and left
  // the rest as stray text — reading mode (pulldown-cmark) never did.
  // A backslash escapes the next character: it never counts as a paren and is
  // kept in the slice, so a row's textContent stays === its source line.
  // Unchanged from the regex: whitespace anywhere in the destination means "not
  // a link" (fall through to plain text), and an empty destination is a link.
  linkAt(s) {
    const m = /^\[([^\[\]]*)\]\(/.exec(s);
    if (!m) return null;
    let d = 0;
    for (let i = m[0].length; i < s.length; i++) {
      const c = s[i];
      if (c === "\\") { if (/\s/.test(s[i + 1] || " ")) return null; i++; continue; }
      if (/\s/.test(c)) return null;
      if (c === "(") d++;
      else if (c === ")") { if (d === 0) return [s.slice(0, i + 1), m[1], s.slice(m[0].length, i)]; d--; }
    }
    return null;                                                 // unterminated
  },

  // R17.7: bare http(s) URL at the START of s, with the same paren rule (GFM
  // autolink): a ')' that closes no '(' inside the URL belongs to the enclosing
  // prose — "(see https://x.example/a)" must not swallow the closing paren —
  // while "https://en.wikipedia.org/wiki/Foo_(bar)" keeps its own.
  urlAt(s) {
    const m = /^https?:\/\/\S+/.exec(s);
    if (!m) return null;
    let d = 0;
    for (let i = 0; i < m[0].length; i++) {
      const c = m[0][i];
      if (c === "(") d++;
      else if (c === ")") { if (d === 0) return [m[0].slice(0, i)]; d--; }
    }
    return m;
  },

  // inline markers, longest-opener-wins, unmatched markers fall through as text
  // a line past INLINE_MAX chars is decorated up to the cap and the rest is
  // plain text: one 575k-char line of 25k [[links]] built 25k anchors at open
  // (audit #10/#12), the same bound BQ_MAX puts on quote depth (#13)
  INLINE_MAX: 20000,
  inline(g, box, s) {
    if (s.length > Ed.INLINE_MAX) {
      Ed.inline(g, box, s.slice(0, Ed.INLINE_MAX));
      box.appendChild(Ed.t(s.slice(Ed.INLINE_MAX)));
      return;
    }
    let plain = "";
    const flush = () => { if (plain) { box.appendChild(Ed.t(plain)); plain = ""; } };
    // tagSpans is sorted and disjoint and i only moves forward, so one cursor
    // replaces a per-char tags.find (O(chars x tags): a 50k-tag line froze
    // live preview — audit #11). Spans skipped by another construct are passed.
    const tags = Ed.tagSpans(s);
    let ti = 0;
    let i = 0;
    while (i < s.length) {
      const rest = s.slice(i);
      while (ti < tags.length && tags[ti][0] < i) ti++;
      const tg = ti < tags.length && tags[ti][0] === i ? tags[ti] : null;
      if (tg) {                                                  // #tag pill
        flush();
        const a = Ed.el("a", "tag");
        a.href = "#"; a.dataset.tag = s.slice(tg[0] + 1, tg[1]);
        a.appendChild(Ed.t(s.slice(tg[0], tg[1])));
        a.addEventListener("mousedown", e => {
          e.preventDefault(); e.stopPropagation();
          if (typeof tagSearch === "function") tagSearch(a.dataset.tag);
        });
        box.appendChild(a); i = tg[1]; continue;
      }
      if (rest[0] === "`") {                                     // code span
        const j = s.indexOf("`", i + 1);
        if (j > i) {
          flush();
          const c = Ed.el("code", "code");
          const m0 = Ed.mk("`"), m1 = Ed.mk("`");
          c.appendChild(m0);
          c.appendChild(Ed.t(s.slice(i + 1, j)));
          c.appendChild(m1);
          box.appendChild(c); Ed.tok([c], [m0, m1], "code"); i = j + 1; continue;
        }
      }
      const wl = /^(!?)\[\[([^\]]*)\]\]/.exec(rest);             // [[wikilink]] / ![[embed]]
      if (wl) {
        flush();
        const raw = wl[2], bar = raw.indexOf("|");
        const target = bar >= 0 ? raw.slice(0, bar) : raw;
        const label = bar >= 0 ? raw.slice(bar + 1) : raw;
        const hash = target.indexOf("#");
        // R29.1 `![[pic.png]]`: an IMAGE embed, not a link. Mirrors linkify()'s
        // guard exactly — bang present, NO anchor, and the target must have an
        // image extension, so a NOTE embed `![[Second Note]]` (requirements.md
        // :410) and `![[pic.png#x]]` both fall through to the link branch below.
        if (wl[1] === "!" && hash < 0 && Ed.isImg(target)) {
          const em = [], emk = [];
          const eadd = (el, isMk) => { em.push(el); if (isMk) emk.push(el); box.appendChild(el); };
          eadd(Ed.mk("![["), true);
          eadd(Ed.mk(raw, "img"), true);                          // the whole inner text, folded
          eadd(Ed.mk("]]"), true);
          eadd(Ed.imgEl(target, bar >= 0 ? label : target), false);  // alias is the alt (image_html)
          Ed.tok(em, emk, "img");
          i += wl[0].length; continue;
        }
        // pdfembed `![[f.pdf#page=3|a]]`: mirrors linkify()'s pdf branch — bang
        // present, pdf extension on the NOTE part; the anchor is page/height.
        if (wl[1] === "!" && Ed.isPdf(hash >= 0 ? target.slice(0, hash) : target)) {
          const em = [], emk = [];
          const eadd = (el, isMk) => { em.push(el); if (isMk) emk.push(el); box.appendChild(el); };
          eadd(Ed.mk("![["), true);
          eadd(Ed.mk(raw, "img"), true);
          eadd(Ed.mk("]]"), true);
          const note = hash >= 0 ? target.slice(0, hash) : target;
          eadd(Ed.pdfEl(note, hash >= 0 ? target.slice(hash) : "", bar >= 0 ? label : note), false);
          Ed.tok(em, emk, "img");
          i += wl[0].length; continue;
        }
        // cvf-links CVR-A-095 `![[Board.canvas]]`: a canvas preview (shapes only, no text node), painted and
        // wired by main.js cvEmbedPaint; Canvas off / no painter -> the link branch below, as before
        if (wl[1] === "!" && hash < 0 && /\.canvas$/i.test(target) && typeof cvEmbedPaint === "function" &&
            typeof CANVAS !== "undefined" && CANVAS && CANVAS.isRegistered()) {
          const em = [], emk = [];
          const eadd = (el, isMk) => { em.push(el); if (isMk) emk.push(el); box.appendChild(el); };
          eadd(Ed.mk("![["), true);
          eadd(Ed.mk(raw, "img"), true);
          eadd(Ed.mk("]]"), true);
          const cs = Ed.el("span", "cv-embed");
          cs.dataset.canvas = target;
          eadd(cs, false);
          Ed.tok(em, emk, "img");
          cvEmbedPaint(cs, null);
          i += wl[0].length; continue;
        }
        const parts = [], mks = [];
        const add = (el, isMk) => { parts.push(el); if (isMk) mks.push(el); box.appendChild(el); };
        add(Ed.mk(wl[1] + "[["), true);
        if (bar >= 0) add(Ed.mk(target + "|", "wl"), true);
        const a = Ed.el("a", "wiki wl");
        a.href = "#";
        const note = hash >= 0 ? target.slice(0, hash) : target;
        if (note) a.dataset.note = note;
        if (hash >= 0) a.dataset.anchor = target.slice(hash + 1);
        if (!Ed.resolves(note)) a.classList.add("wiki-unresolved");
        a.appendChild(Ed.t(label));
        a.addEventListener("mousedown", e => Ed.linkClick(e, a));
        add(a, false);
        add(Ed.mk("]]"), true);
        Ed.tok(parts, mks, wl[1] ? "embed" : "wl");
        i += wl[0].length; continue;
      }
      // R29.2 `![alt](pic.png)` — the markdown image form. Same destination
      // scanner as a link (balanced parens, whitespace = not a link), so
      // `![](my pic.png)` stays literal text exactly as pulldown-cmark leaves
      // it (R29.3). Scheme split mirrors the two Tag::Image arms in main.rs:
      // no scheme -> our vault image; http(s) -> R29.4's banner and NO <img>
      // at all (R29.10 — the renderer refuses to mint a remote src, the CSP is
      // only the backstop); any other scheme -> text, the fall-through below.
      if (rest[0] === "!") {
        const im = Ed.linkAt(rest.slice(1));
        const rem = im && /^https?:\/\//i.test(im[2]);   // R29.10: scheme, not spelling (url_scheme lowercases)
        if (im && (rem || !/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(im[2]))) {
          flush();
          const parts = [], mks = [];
          const add = (el, isMk) => { parts.push(el); if (isMk) mks.push(el); box.appendChild(el); };
          add(Ed.mk("![", "img"), true);
          if (im[1]) add(Ed.mk(im[1], "img"), true);
          add(Ed.mk("](" + im[2] + ")", "img"), true);
          const ph = im[2].indexOf("#"), pp = ph >= 0 ? im[2].slice(0, ph) : im[2];
          add(!rem && Ed.isPdf(pp) ? Ed.pdfEl(pp, ph >= 0 ? im[2].slice(ph) : "", im[1])   // image_html's pdf split
                                   : Ed.imgEl(im[2], im[1], rem), false);
          Ed.tok(parts, mks, "img");
          i += 1 + im[0].length; continue;
        }
      }
      const lt = Ed.linkAt(rest);                                // [text](url)
      if (lt) {
        flush();
        const parts = [], mks = [];
        const add = (el, isMk) => { parts.push(el); if (isMk) mks.push(el); box.appendChild(el); };
        add(Ed.mk("["), true);
        const ext = /^https?:\/\//.test(lt[2]);
        const a = Ed.el("a", ext ? "ext lt" : "lt");
        a.href = ext ? lt[2] : "#";       // S1: the capture-phase gate hands http(s) to open_external
        a.dataset.url = lt[2];
        a.appendChild(Ed.t(lt[1]));
        a.addEventListener("mousedown", e => Ed.extClick(e, a));
        add(a, false);
        add(Ed.mk("](", "url"), true);
        add(Ed.mk(lt[2], "url"), true);
        add(Ed.mk(")", "url"), true);
        Ed.tok(parts, mks, "lt");
        i += lt[0].length; continue;
      }
      const url = Ed.urlAt(rest);                                // bare url
      if (url) {
        flush();
        const a = Ed.el("a", "ext url");
        a.href = url[0]; a.dataset.url = url[0];
        a.appendChild(Ed.t(url[0]));
        a.addEventListener("mousedown", e => Ed.extClick(e, a));
        box.appendChild(a); i += url[0].length; continue;
      }
      let done = false;
      for (const [open, cls, tag] of [["***", "bi", "strong"], ["**", "b", "strong"],
                                      ["~~", "s", "span"], ["==", "hl", "mark"],
                                      ["*", "i", "em"], ["_", "i", "em"]]) {
        if (!rest.startsWith(open)) continue;
        if (open === "_" && i > 0 && !/\s/.test(s[i - 1])) break;  // intraword _ is plain
        const a0 = i + open.length;
        if (!/\S/.test(s[a0] || "")) break;
        const j = s.indexOf(open, a0);
        if (j > a0 && /\S/.test(s[j - 1])) {
          flush();
          const m0 = Ed.mk(open);
          box.appendChild(m0);
          const e = Ed.el(tag, cls);
          Ed.inline(g, e, s.slice(a0, j));
          box.appendChild(e);
          const m1 = Ed.mk(open);
          box.appendChild(m1);
          Ed.tok([m0, e, m1], [m0, m1], cls);
          i = j + open.length; done = true;
        }
        break;                                                   // longest opener decides
      }
      if (done) continue;
      plain += s[i]; i++;
    }
    flush();
  },

  // index.rs tag rules: whitespace-preceded '#', at least one non-digit
  tagSpans(s) {
    const out = [];
    let prevWs = true;
    for (let i = 0; i < s.length; i++) {
      const c = s[i];
      if (c === "#" && prevWs) {
        let j = i + 1;
        while (j < s.length && /[\w\/-]/.test(s[j])) j++;
        const body = s.slice(i + 1, j);
        if (body && /\D/.test(body)) { out.push([i, j]); i = j - 1; prevWs = false; continue; }
      }
      prevWs = /\s/.test(c);
    }
    return out;
  },
  resolves(n) {
    if (!n) return true;
    if (typeof notesCache === "undefined" || !notesCache.length) return true;
    return Ed.resSet().has(n);
  },
  /* perfhunt P8: resolves() used to scan every vault note per rendered
     wikilink (4 string tests x 50k names; an unresolved link always paid the
     full scan), so a note open on a big vault cost ms per link. The answer is
     a set lookup: n passes the old test iff n is some x, or x's tail after one
     of its '/', or the same of x minus a trailing ".md". Built once per
     notesCache array — refreshTree() is its only writer and always assigns a
     NEW array, so identity is the invalidation. */
  resSet() {
    if (Ed._resFor === notesCache) return Ed._res;
    const s = new Set();
    const tails = y => {
      s.add(y);
      for (let p = y.indexOf("/"); p >= 0; p = y.indexOf("/", p + 1)) s.add(y.slice(p + 1));
    };
    for (const x of notesCache) {
      tails(x);
      if (x.endsWith(".md")) tails(x.slice(0, -3));
    }
    Ed._res = s; Ed._resFor = notesCache;
    return s;
  },

  /* ---------- R29 IMAGE EMBEDS (live preview half) ----------
     R29.7: reading view (Rust/pulldown-cmark) and live preview are separate
     engines, so every decision below mirrors ONE named function in
     src-tauri/src/main.rs and nothing else:
       Ed.isImg     <- is_img_target        (index::IMG_EXTS, 5 extensions)
       Ed.pctDec    <- pct_decode           (strict: %XX or nothing)
       Ed.pctEnc    <- pct_encode           (unreserved + '/' survive)
       Ed.imgRel    <- index::resolve       (exact, else basename suffix)
       Ed.imgEl     <- image_html           (src from the INDEX, never the target)
     The src is built from the path the IMAGE LIST holds, never from the
     target in the note: the list only ever contains real, non-hidden,
     non-symlinked vault members, so a traversal target resolves to nothing
     and gets no URL at all (the byte server re-checks containment anyway).
     An unresolved target is R29.4's banner in both engines. */
  IMG_EXTS: ["png", "jpg", "jpeg", "gif", "webp"],
  IMG_SCHEME: "opensidian-img",
  isImg(t) {                                   // is this target an IMAGE embed, not a note embed?
    const i = t.lastIndexOf(".");
    return i >= 0 && Ed.IMG_EXTS.includes(t.slice(i + 1).toLowerCase());
  },
  pctDec(s) { try { return decodeURIComponent(s); } catch (_) { return null; } },
  pctEnc(s) {
    let out = "";
    for (const b of new TextEncoder().encode(s)) {
      const c = String.fromCharCode(b);
      out += /[A-Za-z0-9\-._~\/]/.test(c) ? c : "%" + b.toString(16).toUpperCase().padStart(2, "0");
    }
    return out;
  },
  imgRel(target) {                             // vault-relative path from the image list, or null
    const dec = Ed.pctDec(target) ?? target;   // image_html's unwrap_or_else(target)
    if (!dec || typeof imgsCache === "undefined") return null;
    return Ed.imgIdx().get(dec) ?? null;
  },
  // audit #10: imgRel used to scan the whole image list per embed (images x
  // embeds). Same answer, one Map: every path is keyed by itself and by each
  // suffix that starts after a '/', the value being the FIRST path in list order
  // that has that key — exactly what find(x => x === dec || x.endsWith("/" + dec))
  // returned. Rebuilt only when refreshTree swaps in a new list (identity check).
  _imgIdxOf: null, _imgIdx: null,
  _res: null, _resFor: null,     // perfhunt P8: resSet() cache, keyed by the notesCache array
  imgIdx() {
    if (Ed._imgIdxOf !== imgsCache) {
      const m = new Map();
      for (const x of imgsCache) {
        if (!m.has(x)) m.set(x, x);
        for (let j = x.indexOf("/"); j >= 0; j = x.indexOf("/", j + 1)) {
          const k = x.slice(j + 1);
          if (k && !m.has(k)) m.set(k, x);
        }
      }
      Ed._imgIdx = m; Ed._imgIdxOf = imgsCache;
    }
    return Ed._imgIdx;
  },
  imgSrc(target) {
    const rel = Ed.imgRel(target);
    return rel === null ? null : Ed.IMG_SCHEME + "://localhost/" + Ed.pctEnc(rel);
  },
  // the ONE widget both syntaxes paint. Carries no TEXT node in either branch:
  // the row's textContent must stay === its source line (the token map walks
  // text nodes), so the R29.4 banner wording is painted by CSS from data-miss.
  // R29.10: `remote` (http(s)) never becomes an <img> — it takes the SAME
  // banner a missing local target takes, labelled with the constant
  // "remote image" (main.rs REMOTE_IMG_LABEL), never the URL. The renderer
  // refusing to mint the src is layer 1; the CSP's img-src is the backstop.
  imgEl(target, alt, remote) {
    const src = remote ? null : Ed.imgSrc(target);
    if (src === null) {
      const s = Ed.el("span", "imgmiss");
      s.dataset.miss = remote ? "remote image" : target;  // CSS: “<label>” could not be found.
      s.contentEditable = "false";
      return s;
    }
    const im = Ed.el("img", "vimg");
    // audit #10: fetch on approach, not at render. Every opensidian-img request is
    // answered on the UI process's main loop, so 20k eager embeds queued 20k
    // round trips there and starved everything else for seconds after the note
    // had rendered (measured: note_open span 362 ms, title seen 3 s later).
    // ORDER MATTERS: setting src starts the fetch with whatever loading mode the
    // element has AT THAT MOMENT, so lazy must be set first (lazy-after-src left
    // all 20000 loaded at open, [xi:20000/20000/0]).
    im.setAttribute("loading", "lazy");
    im.setAttribute("src", src);               // attribute, not property: the
    im.setAttribute("alt", alt || "");         // agreement assertion compares raw attrs
    im.contentEditable = "false";
    im.draggable = false;
    return im;
  },

  /* ---------- pdfembed: PDF EMBEDS (both engines) ----------
     Mirrors of the Rust side, one function each (R29.7):
       Ed.isPdf    <- is_pdf_target   (extension "pdf", any case)
       Ed.pdfFrag  <- pdf_frag        (#page=N&height=N, strict decimal)
       Ed.pdfEl    <- pdf_html        (path from the INDEX list, never the target)
     The element is the SAME span both engines emit; Ed.pdfFill builds the
     frame inside it and is the ONE frame builder (reading view calls it on the
     rendered html, live preview on the element it just made). The frame holds
     NO text node: an LP row's textContent must stay === its source line, so the
     toolbar counter and the broken note are painted by CSS from attributes.
     Pages are PNGs rasterised by the Rust worker (opensidian-img ?page=&w=),
     requested only when they scroll near the frame's view — never PDF bytes,
     never a PDF viewer, so no PDF action can run here (README security design). */
  isPdf(t) {
    const i = t.lastIndexOf(".");
    return i >= 0 && t.slice(i + 1).toLowerCase() === "pdf";
  },
  pdfFrag(frag) {
    let page = 1, h = null, pg = false;
    for (const kv of frag.replace(/^#+/, "").split("&")) {
      const e = kv.indexOf("=");
      if (e < 0) continue;
      const k = kv.slice(0, e), v = kv.slice(e + 1);
      if (!/^[0-9]{1,6}$/.test(v)) continue;
      const n = parseInt(v, 10);
      if (k === "page" && n >= 1) { page = n; pg = true; }
      else if (k === "height" && n >= 1) h = n;
    }
    return { page, h, pg };
  },
  pdfEl(target, frag, alt) {
    const dec = Ed.pctDec(target) ?? target;
    const rel = Ed.imgRel(target);
    if (rel === null || !Ed.isPdf(rel)) {
      const s = Ed.el("span", "imgmiss pdfmiss");
      s.dataset.miss = dec;
      s.contentEditable = "false";
      return s;
    }
    const f = Ed.pdfFrag(frag || "");
    const s = Ed.el("span", "internal-embed pdf-embed");
    s.dataset.pdf = Ed.pctEnc(rel);
    s.dataset.page = String(f.page);
    s.dataset.height = f.h === null ? "" : String(f.h);
    s.dataset.alt = alt || "";
    // live preview only (pdfEl is the LP builder; reading view never sets it):
    // stock LP sizes #page=N&height=H at H flat, no toolbar add (recon v-pagehgt:
    // LP 700x300, RV 700x338; #height=H alone is H + 38 in both)
    if (f.pg && f.h !== null) s.dataset.lpflat = "1";
    s.contentEditable = "false";
    s.tabIndex = -1;
    Ed.pdfFill(s);
    return s;
  },
  PDF_TOOLBAR: 39, PDF_STOCK_AR: 792 / 612,
  // stock (recon README "Default frame size"): frame = w x aspect(page 1) + 20,
  // #height=N -> N + 38 (LP #page=&height=N -> N, stock quirk). Broken -> 800, like stock.
  pdfSize(s) {
    const w = s.clientWidth;
    if (!w) return;
    const h = s.dataset.height ? +s.dataset.height + (s.dataset.lpflat ? 0 : 38)
      : s.classList.contains("mod-broken") ? 800
      : Math.round(w * (s._pdfAr || Ed.PDF_STOCK_AR) + 20);
    if (s.style.height !== h + "px") s.style.height = h + "px";
  },
  pdfFill(s) {
    if (s._pdfFilled) return;
    s._pdfFilled = true;
    const bar = Ed.el("div", "pdf-toolbar");
    const zo = Ed.el("button", "pdf-zoom-out"), zi = Ed.el("button", "pdf-zoom-in");
    zo.type = zi.type = "button";
    zo.setAttribute("aria-label", "Zoom out"); zi.setAttribute("aria-label", "Zoom in");
    const cnt = Ed.el("span", "pdf-counter");
    bar.append(cnt, zo, zi);
    const box = Ed.el("div", "pdf-container");
    s.append(bar, box);
    // an LP row is contenteditable: clicks in the frame must not move the caret
    s.addEventListener("mousedown", e => { if (e.button === 0) e.stopPropagation(); });
    bar.addEventListener("mousedown", e => { e.preventDefault(); e.stopPropagation(); });
    let zoom = 1, pages = [];
    const setZoom = z => {
      zoom = Math.min(4, Math.max(0.25, z));
      s.dataset.zoom = String(zoom);
      for (const im of pages) im.style.width = (zoom * 100) + "%";
    };
    zo.onclick = () => setZoom(zoom / 1.25);
    zi.onclick = () => setZoom(zoom * 1.25);
    const setCounter = (cur, of) => {
      cnt.dataset.counter = cur + " of " + of;
      s.dataset.cur = String(cur); s.dataset.of = String(parseInt(of, 10) || 0);
      if (typeof xiAgain === "function") xiAgain();        // census [pf:] re-publishes (one rAF)
    };
    setCounter(of0(), 0);
    function of0() { return +s.dataset.page || 1; }
    // resize in the NEXT frame: setting the height inside the observer callback
    // is a resize the same delivery cannot report (the "ResizeObserver loop" error)
    new ResizeObserver(() => requestAnimationFrame(() => Ed.pdfSize(s))).observe(s);
    Ed.pdfSize(s);
    if (typeof inv !== "function") return;
    inv("pdf_info", { path: s.dataset.pdf }).then(info => {
      if (!info || !info.pages) {
        s.classList.add("mod-broken");
        setCounter(0, 0);
        Ed.pdfSize(s);
        return;
      }
      const sz = info.sizes || [];
      const ar = sz[0] && sz[0][0] > 0 ? sz[0][1] / sz[0][0] : Ed.PDF_STOCK_AR;
      s._pdfAr = Math.min(10, Math.max(0.1, ar));
      Ed.pdfSize(s);
      const n = sz.length || 0;           // pdf.rs caps the list (MAX_LISTED)
      const want = Math.min(of0(), n);
      const io = new IntersectionObserver(es => {
        for (const e of es) {
          if (!e.isIntersecting) continue;
          const im = e.target;
          io.unobserve(im);
          const px = Math.min(2048, Math.ceil(box.clientWidth * zoom * (window.devicePixelRatio || 1) / 64) * 64 || 640);
          im.onload = () => {
            if (+im.dataset.n !== want) return;
            s.classList.add("is-loaded");
            if (typeof xiAgain === "function") xiAgain();
          };
          im.setAttribute("src", Ed.IMG_SCHEME + "://localhost/" + s.dataset.pdf + "?page=" + im.dataset.n + "&w=" + px);
        }
      }, { root: box, rootMargin: "100% 0px" });
      for (let i = 0; i < n; i++) {
        const im = Ed.el("img", "pdf-page");
        const [pw, ph] = sz[i];
        im.dataset.n = String(i + 1);
        im.style.aspectRatio = (pw > 0 && ph > 0 ? pw + " / " + ph : "612 / 792");
        im.setAttribute("alt", "");
        im.draggable = false;
        box.appendChild(im);
        pages.push(im);
      }
      setZoom(zoom);
      const total = info.pages > n ? info.pages + " (first " + n + " shown)" : String(info.pages);
      setCounter(want, total);
      const go = () => { if (pages[want - 1]) box.scrollTop = pages[want - 1].offsetTop; };
      requestAnimationFrame(() => { go(); for (const im of pages) io.observe(im); });
      // the frame's height settles after attach (LP builds rows detached):
      // re-aim at #page on every resize until the USER moves the frame
      let user = false;
      for (const ev of ["wheel", "pointerdown", "keydown", "touchstart"])
        box.addEventListener(ev, () => { user = true; }, { passive: true });
      new ResizeObserver(() => { if (!user) go(); }).observe(box);
      box.addEventListener("scroll", () => {
        const top = box.scrollTop + box.clientHeight / 2;
        let cur = 1;
        for (const im of pages) { if (im.offsetTop <= top) cur = +im.dataset.n; else break; }
        if (String(cur) !== s.dataset.cur) setCounter(cur, total);
      }, { passive: true });
    });
  },

  /* ---------- token map: source col <-> DOM offset ---------- */
  // The row's text nodes in document order carry the whole source line, in
  // order, so the map is implicit. `hidden` = the node sits inside a span.mk
  // that CSS is not revealing — never a caret target while the token is folded.
  // A marker whose token IS revealed (.rv, R17.7) is ordinary visible text.
  nodes(row) {
    const out = [];
    const w = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
    let c = 0, n;
    while ((n = w.nextNode())) {
      const m = n.parentElement.closest(".mk");
      out.push({ n, c, len: n.nodeValue.length, hid: !!m && !m.classList.contains("rv") });
      c += n.nodeValue.length;
    }
    return out;
  },
  colOf(row, node, off) {                       // DOM position -> source col
    if (node === row) {                         // offset = child index
      let c = 0;
      for (let k = 0; k < off && k < row.childNodes.length; k++) c += (row.childNodes[k].textContent || "").length;
      return c;
    }
    for (const s of Ed.nodes(row)) if (s.n === node) return s.c + Math.min(off, s.len);
    // node is an element inside the row: count the text before it
    const map = Ed.nodes(row);
    for (const s of map) if (node.contains && node.contains(s.n)) return s.c;
    return map.length ? map[map.length - 1].c + map[map.length - 1].len : 0;
  },
  posOf(row, col, revealed) {                   // source col -> [node, offset]
    const map = Ed.nodes(row);
    if (!map.length) return [row, 0];
    let cand = null;
    for (const s of map) {
      if (col >= s.c && col <= s.c + s.len) {
        if (revealed || !s.hid) return [s.n, col - s.c];
        if (!cand) cand = s;
      }
    }
    if (cand) return [cand.n, col - cand.c];
    const last = map[map.length - 1];
    return [last.n, last.len];
  },
  // Home target for a line (R17.5 M64-M68): CONTENT start, i.e. after a list /
  // task / quote marker + its indent. A heading is NOT a marker line — its "# "
  // stays part of the text, so Home is col 0 there (M68).
  homeCol(g, l) {
    const t = Ed.lines(g)[l] || "";
    if (/^\s*#{1,6} /.test(t)) return 0;
    const m = /^(\s*(?:> )*(?:[-*+] \[[ xX]\] |[-*+] |\d+\. ))/.exec(t) || /^(\s*(?:> )+)/.exec(t);
    return m ? m[0].length : 0;
  },
  // one RAW column left/right, crossing the line edge (R17.5 M69-M71). Pure:
  // covered by Ed.selfTest. null = the document edge, leave it to the browser.
  step(g, l, c, back) {
    const L = Ed.lines(g);
    if (back) {
      if (c > 0) return { l, c: c - 1 };
      return l > 0 ? { l: l - 1, c: L[l - 1].length } : null;
    }
    if (c < (L[l] || "").length) return { l, c: c + 1 };
    return l < L.length - 1 ? { l: l + 1, c: 0 } : null;
  },
  /* R17.6 / R17.7 REVEAL — PER TOKEN, not per row.
     "a marker is hidden iff the selection does NOT intersect that token's raw
     range — this is PER TOKEN, not per line" (R17.6). So: walk the tokens of
     the touched lines only, flip `.rv` on the marker spans of the ones the
     selection reaches, and drop `.rv` from last move's set. `.cur` still marks
     the caret ROW, but it no longer reveals anything — the row-wide rule
     survives for SOURCE MODE alone (`.lp.src .mk`, ui/style.css).

     a/b = the model selection ends (a <= b), null to reveal nothing. A
     COLLAPSED caret touches a token when its column is anywhere in the closed
     range [s, e] (M74: col 15 in `*ital*` 13..19 reveals it; M77: col 55 with
     `#tag` at 57..61 reveals nothing). A non-empty range needs real overlap, so
     selecting up to a token's first column does not open it.

     Cost: O(tokens on the touched lines). The map itself is built once per row
     in Ed.row and cached on the element, so nothing is re-rendered or re-parsed
     here — this is the code that runs on every caret move. */
  /* ---------- PERF: caret-move cost to first paint (R18, 100ms ceiling) ----------
     The per-token reveal (R17.6/R17.7) runs on EVERY caret move, which is the
     highest-frequency interaction in the product, so the caret move gets its own
     measurement instead of riding on key_to_paint (that span covers INPUT, and a
     caret move types nothing). t0 = the key, t1 = after the frame carrying the
     new reveal set is committed — the same rAF -> task pattern otel.paint uses,
     so the number is comparable with key_to_paint's wall_ms.
     Published as [cm:<last>/<max>/<n>] in the census; also emitted as a
     `caret_move` otel span when instrumentation is on. */
  cmT0: -1, cmMs: -1, cmMax: 0, cmN: 0, cmSum: 0,
  cmStart() { if (Ed.cmT0 < 0) Ed.cmT0 = performance.now(); },
  cmEnd(g) {
    if (Ed.cmT0 < 0) return;
    const t0 = Ed.cmT0; Ed.cmT0 = -1;
    requestAnimationFrame(() => setTimeout(() => {
      const ms = Math.round((performance.now() - t0) * 100) / 100;
      Ed.cmMs = ms; Ed.cmN++; Ed.cmSum += ms;
      if (ms > Ed.cmMax) Ed.cmMax = ms;
      if (typeof otel !== "undefined" && otel.span)
        otel.span("caret_move", { reveal: ((g && g.lp && g.lp._rvs) || []).length, note_lines: (g && g.lpLines) || 0 }, ms);
      Ed.census();
    }, 0));
  },
  // [cm:<last>/<max>/<avg>/<n>] — ms to first paint, over the run so far
  cmTok() {
    if (Ed.cmMs < 0) return "";
    return " [cm:" + Ed.cmMs + "/" + Ed.cmMax + "/" +
           (Math.round(Ed.cmSum / Ed.cmN * 100) / 100) + "/" + Ed.cmN + "]";
  },
  // PURE: the tokens of ONE line that a selection [lo,hi] on that line touches.
  // A COLLAPSED caret (lo === hi) touches the CLOSED range [s,e] — M74: col 15
  // inside `*ital*` 13..19 opens it, M77: col 55 with `#tag` at 57..61 opens
  // nothing. A non-empty range needs real overlap, so selecting up to a token's
  // first column does not open it. Unit-asserted in Ed.selfTest.
  tokAt(toks, lo, hi) {
    const touch = lo === hi ? (t => lo >= t.s && lo <= t.e) : (t => lo < t.e && hi > t.s);
    return toks.filter(touch);
  },
  mark(g, l) {                                   // census + autocomplete state
    Ed.census();                                 // selection moved even if the LINE did not
    if (g.lpActive && l >= 0 && g.lpActive.l0 === l) return;   // nothing moved: no title churn
    Ed.cur = l >= 0 ? { g, l } : null;
    g.lpActive = l == null || l < 0 ? null : { l0: l };
    if (g.view) g.view.lpActive = g.lpActive;
    Ed.census();
  },
  /* The census carries the caret line, the model selection and the row
     geometry, and a plain Shift+Right moves none of the first — so it must be
     republished on every selection change, not only when the LINE changes.
     Deferred + coalesced (30ms): the title write must never land inside the
     key_to_paint span it would otherwise inflate. */
  census() {
    if (Ed._ct) return;
    Ed._ct = setTimeout(() => {
      Ed._ct = null;
      if (typeof updateTitle === "function") updateTitle();
    }, 30);
  },
  /* ---------- R17.1/R17.2: the structural prefix of a source line ----------
     ind  leading whitespace — the indent string, copied VERBATIM on continue
          (M11: a 2-space file keeps spaces, it never normalises to TAB)
     qt   blockquote prefix ("> ", "> > ", …), "" when none
     mk   list / task marker WITH its trailing space, "" when none
     num  the ordered number, or null
     pre  ind + qt + mk = everything before the item's content */
  info(line) {
    const m = /^([ \t]*)((?:> ?)*)((?:[-*+]|\d+\.) (?:\[[ xX]\] )?)?/.exec(line || "");
    const mk = m[3] || "", n = /^(\d+)\./.exec(mk);
    return {
      ind: m[1], qt: m[2], mk, num: n ? parseInt(n[1], 10) : null,
      task: /\[[ xX]\] $/.test(mk), pre: m[1] + m[2] + mk,
    };
  },
  // the prefix a NEW item after this one carries: same indent + quote, the
  // ordered number stepped (M4), a task always UNCHECKED (M8)
  contOf(it) {
    const mk = it.task ? it.mk.replace(/\[[xX]\]/, "[ ]")
      : it.num !== null ? (it.num + 1) + ". " + it.mk.slice(String(it.num).length + 2)
        : it.mk;
    return it.ind + it.qt + mk;
  },
  // M24: Enter on an item's CONTINUATION line starts a new item of the item
  // that owns it (top level), not another continuation
  ownerCont(L, l, ind) {
    for (let k = l - 1; k >= 0; k--) {
      if (!String(L[k]).trim()) return "";                 // blank line ends the item
      const it = Ed.info(L[k]);
      if (it.mk) return it.ind.length < ind.length ? Ed.contOf(it) : "";
    }
    return "";
  },
  // M4/M5/M6/M12: the ordered run BELOW `from` renumbers on the spot. Deeper
  // lines (nested runs, continuation lines) are skipped, not renumbered — a
  // nested run numbers independently of its parent; anything at this level
  // that is not an ordered item ENDS the run.
  renumber(L, from, ref, next) {
    const pre = ref.ind + ref.qt;
    for (let l = from; l < L.length; l++) {
      const it = Ed.info(L[l]);
      if (it.num !== null && it.ind + it.qt === pre) {
        L[l] = pre + next + ". " + it.mk.slice(String(it.num).length + 2) + L[l].slice(it.pre.length);
        next++;
      } else if (it.ind.length > ref.ind.length) continue;  // nested / continuation
      else break;
    }
  },
  // one indent unit off the END of an indent string (Shift+Tab / M13 outdent)
  outdentStr(ind) { const m = /(\t| {1,4})$/.exec(ind); return m ? ind.slice(0, -m[0].length) : ind; },
  // the number an ordered item at line `l` takes at `ref`'s level: one past the
  // previous sibling of that level, or 1 when it opens the run (M32).
  startNum(L, l, ref) {
    const pre = ref.ind + ref.qt;
    for (let k = l - 1; k >= 0; k--) {
      if (!String(L[k]).trim()) break;                       // a blank line ends the run
      const it = Ed.info(L[k]);
      if (it.ind + it.qt === pre) return it.num !== null ? it.num + 1 : 1;
      if (it.ind.length < ref.ind.length) break;             // left the sub-list
    }
    return 1;
  },
  enter(g, s) {                                  // Enter: split + list continuation
    const L = Ed.lines(g), a = s.a, b = s.b, line = L[a.l] || "";
    const it = Ed.info(line);
    // --- an EMPTY item / quote line does not continue, it unwinds (M13-M19)
    if (s.empty && line === it.pre && a.c >= it.pre.length && (it.mk || it.qt)) {
      Ed.snap(g, "enter");
      if (it.mk && it.ind) {                      // M13: nested -> outdent one level
        L[a.l] = Ed.outdentStr(it.ind) + it.qt + it.mk;
      } else if (it.mk) {                         // M14/M16/M17: drop the marker, keep the line
        L[a.l] = it.ind + it.qt;
      } else {                                    // M19: a quote line drops '> ' AND opens a line
        L[a.l] = "";
        L.splice(a.l + 1, 0, "");
        return Ed.after(g, a.l + 1, 0);
      }
      return Ed.after(g, a.l, L[a.l].length);
    }
    // --- otherwise: split at the caret, the tail inherits the prefix
    let cont = "";
    if (a.c >= it.pre.length) {
      if (it.mk) cont = Ed.contOf(it);            // M1/M7/M10/M20
      else if (it.qt) cont = it.ind + it.qt;      // M18: the quote prefix continues
      else if (it.ind) cont = Ed.ownerCont(L, a.l, it.ind);   // M24
    }
    Ed.snap(g, "enter");
    const head = line.slice(0, a.c), tail = (L[b.l] || "").slice(b.c);
    L.splice(a.l, b.l - a.l + 1, head, cont + tail);
    const ci = Ed.info(cont);
    if (ci.num !== null) Ed.renumber(L, a.l + 2, ci, ci.num + 1);
    Ed.after(g, a.l + 1, cont.length);
  },
  // M26 Shift+Enter: a soft break INSIDE the item — the marker's width in
  // spaces, no marker (tabs in the indent survive as tabs)
  softBreak(g, s) {
    const it = Ed.info(Ed.lines(g)[s.a.l] || "");
    const pad = it.mk && s.a.c >= it.pre.length ? it.pre.replace(/[^\t]/g, " ") : "";
    Ed.replace(g, s, "\n" + pad, "enter");
  },
  del(g, s, forward, word) {                     // Backspace / Delete
    if (!s.empty) return Ed.replace(g, s, "", "del");
    const L = Ed.lines(g), { l, c } = s.a;
    if (!forward) {
      if (c > 0) {
        let k = c - 1;
        if (word) { k = c; while (k > 0 && /\s/.test(L[l][k - 1])) k--; while (k > 0 && !/\s/.test(L[l][k - 1])) k--; }
        return Ed.replace(g, { a: { l, c: k }, b: { l, c }, empty: false }, "", "del");
      }
      if (l === 0) return;
      return Ed.replace(g, { a: { l: l - 1, c: L[l - 1].length }, b: { l, c: 0 }, empty: false }, "", "del");
    }
    if (c < L[l].length) {
      let k = c + 1;
      if (word) { k = c; while (k < L[l].length && /\s/.test(L[l][k])) k++; while (k < L[l].length && !/\s/.test(L[l][k])) k++; }
      return Ed.replace(g, { a: { l, c }, b: { l, c: k }, empty: false }, "", "del");
    }
    if (l >= L.length - 1) return;
    Ed.replace(g, { a: { l, c }, b: { l: l + 1, c: 0 }, empty: false }, "", "del");
  },
  /* R17.2/R17.10: Tab / Shift+Tab move the LINE, never insert a tab at the
     caret (M33/M34). The unit is exactly one TAB (M27, not spaces); Shift+Tab
     strips one leading TAB or up to 4 spaces and is a NO-OP when the line has
     no indent — it never eats the marker (M31/M87).
     A plain caret on a LIST ITEM carries the item's nested children and
     continuation lines with it: indenting a parent past its own children would
     re-parent them to the wrong item. A real selection moves exactly the lines
     it covers (M88/M89), minus a trailing line it only touches at col 0. */
  indent(g, s, out) {
    const L = Ed.lines(g);
    let first = s.a.l, last = s.b.l;
    if (last > first && s.b.c === 0) last--;                 // M88
    const old = Ed.info(L[first] || "");
    if (s.empty && old.mk) {                                 // carry the sub-tree
      for (let l = first + 1; l < L.length; l++) {
        if (!String(L[l]).trim()) break;
        if (Ed.info(L[l]).ind.length <= old.ind.length) break;
        last = l;
      }
    }
    Ed.snap(g, "indent");
    let dc = 0;
    for (let l = first; l <= last; l++) {
      if (out) {
        const m = /^(\t| {1,4})/.exec(L[l]);
        if (m) { L[l] = L[l].slice(m[0].length); if (l === s.b.l) dc = -m[0].length; }
      } else { L[l] = "\t" + L[l]; if (l === s.b.l) dc = 1; }
    }
    // M32: an ordered item that changes level RESTARTS the run it joins, and
    // the run it LEFT closes up over the hole ("3. c" -> "2. c").
    if (s.empty && old.num !== null && L[first] !== undefined) {
      const it = Ed.info(L[first]);
      if (it.num !== null) Ed.renumber(L, first, it, Ed.startNum(L, first, it));
      Ed.renumber(L, last + 1, old, Ed.startNum(L, last + 1, old));
    }
    Ed.after(g, s.b.l, Math.max(0, s.b.c + dc));
  },
  /* R17.4 M46-M51 Ctrl+L "Toggle checkbox status" (in 1.13.7 this is the task
     toggle — Ctrl+Enter is NOT, see M45/M92): a task flips [ ] <-> [x] in
     place, a bullet/ordered item gains a "[ ] " after its marker, a plain
     paragraph gains a whole "- [ ] ". The caret keeps its offset in the TEXT,
     so it shifts by exactly the bytes inserted before it. */
  toggleCheck(g, s) {
    const L = Ed.lines(g), l = s.b.l, line = L[l];
    if (line === undefined) return;
    const it = Ed.info(line);
    Ed.snap(g, "task");
    let dc = 0;
    if (it.task) L[l] = Ed.flipTask(line);
    else if (it.mk) { L[l] = it.pre + "[ ] " + line.slice(it.pre.length); dc = 4; }
    else { L[l] = it.ind + it.qt + "- [ ] " + line.slice(it.pre.length); dc = 6; }
    Ed.after(g, l, s.b.c >= it.pre.length ? s.b.c + dc : s.b.c);
  },
  /* listtoggle R1-R5: "Toggle bullet list" / "Toggle numbered list" over a
     SELECTION (Obsidian 1.13.7 measured, docs/recon-listtoggle/README.md; spec
     §4 frozen). Unlike toggleCheck this acts on EVERY line the selection
     touches — a selection ending at column 0 of line N still includes N [Q10].
     Ed.listToggle is PURE (lines + selection in -> lines + selection out, or
     null for a no-op) so the unit table (src-tauri/tests/listtoggle.{tsv,rs}, cargo test) can run
     it without a DOM; Ed.toggleList is the thin view wrapper (one undo step).
     Own marker parser, NOT Ed.info: Obsidian treats "N)" as numbered [Q4
     q04-offparen] and Ed.info does not, and widening Ed.info would change
     Enter/indent continuation, which nothing here measured. */
  ltParse(line) {
    const m = /^([ \t]*)((?:> ?)*)(?:([-*+]) (\[[ xX]\] )?|(\d+)[.)] (\[[ xX]\] )?)?/.exec(line || "");
    const kind = m[3] ? (m[4] ? "task" : "bullet") : m[5] !== undefined ? "numbered" : "";
    return { ind: m[1], qt: m[2], box: m[1].length + m[2].length, mk: m[0].length - m[1].length - m[2].length,
             kind, num: m[5] !== undefined ? parseInt(m[5], 10) : null };
  },
  /* Q13 scan: the number a numbered item at `l` (indent `ind`, quote `qt`)
     takes = previous numbered item at that level + 1, scanning UP past plain,
     blank and deeper lines; a bullet/task at that level or any shallower line
     (or a different quote container) ends the scan -> 1. */
  ltStart(L, l, ind, qt) {
    for (let k = l - 1; k >= 0; k--) {
      if (!String(L[k]).trim()) continue;
      const p = Ed.ltParse(L[k]);
      if (p.ind.length > ind.length) continue;             // deeper: skipped [q13-stopdeep]
      if (p.ind.length < ind.length || p.qt !== qt) return 1;
      if (p.kind === "numbered") return p.num + 1;
      if (p.kind) return 1;                                // bullet at this level [q13-bulletabove]
    }                                                      // plain at this level: skipped [q13-gap]
    return 1;
  },
  // following numbered items at a level, renumbered by the same scan
  // [q13-follow/offmid/stopb/stopdeep]
  ltFollow(L, from, ind, qt) {
    for (let l = from; l < L.length; l++) {
      if (!String(L[l]).trim()) continue;
      const p = Ed.ltParse(L[l]);
      if (p.ind.length > ind.length) continue;
      if (p.ind.length < ind.length || p.qt !== qt) return;
      if (p.kind === "numbered") {
        const n = Ed.ltStart(L, l, ind, qt), s0 = p.box;
        const old = L[l].slice(s0, s0 + p.mk), mk = n + old.slice(String(p.num).length);
        L[l] = L[l].slice(0, s0) + mk + L[l].slice(s0 + p.mk);
      } else if (p.kind) return;
    }
  },
  listToggle(lines, s, kind) {
    const L = lines.slice(), a = { l: s.a.l, c: s.a.c }, b = { l: s.b.l, c: s.b.c };
    const first = a.l, last = Math.min(b.l, L.length - 1);
    const P = [];
    for (let l = first; l <= last; l++) P[l] = String(L[l]).trim() ? Ed.ltParse(L[l]) : null;
    const live = P.filter(Boolean);
    if (!live.length) return null;                        // all blank: nothing to do (UNMEASURED)
    const off = live.every(p => p.kind === kind);         // [Q4 Q5 q07-offb]
    const shift = { };                                     // l -> [s0, oldLen, newLen]
    for (let l = first; l <= last; l++) {
      const p = P[l];
      if (!p) continue;                                   // blank: left alone [Q7]
      let mk = "";
      if (!off) mk = kind === "bullet" ? "- " : Ed.ltStart(L, l, p.ind, p.qt) + ". ";
      L[l] = L[l].slice(0, p.box) + mk + L[l].slice(p.box + p.mk);
      shift[l] = [p.box, p.mk, mk.length];
    }
    // the numbered runs after the selection, one per level it touched (R2 scopes
    // this to the NUMBERED command; bullet -> following run is UNMEASURED)
    const lv = new Set();
    if (kind === "numbered") for (let l = first; l <= last; l++) if (P[l]) lv.add(P[l].ind + "\u0000" + P[l].qt);
    for (const k of lv) { const [ind, qt] = k.split("\u0000"); Ed.ltFollow(L, last + 1, ind, qt); }
    // positions through the edit (CodeMirror range mapping, measured): the
    // caret and a range's START move right past a marker inserted AT them
    // [q11-*, q12-selb, q12b-caret0]; a non-empty range's END stays before it,
    // so the range never grows over the next line's marker [q12b-clipcol0,
    // q12b-clipmid0]. Inside a removed/replaced marker -> the edge of the new one.
    const empty = a.l === b.l && a.c === b.c;
    const map = (p, end) => {
      const t = shift[p.l];
      if (!t) return p;
      const [s0, o, n] = t;
      if (p.c < s0 || (end && p.c === s0 && o === 0)) return p;
      return { l: p.l, c: p.c >= s0 + o ? p.c - o + n : end ? s0 : s0 + n };
    };
    return { lines: L, a: map(a, false), b: map(b, !empty) };
  },
  toggleList(g, s, kind) {
    const r = Ed.listToggle(Ed.lines(g), s, kind);
    if (!r) return;
    Ed.snap(g, "list");                                   // ONE undo step [q12-undob/undon/undooff]
    const L = Ed.lines(g);
    L.splice(0, L.length, ...r.lines);
    Ed.after(g, r.a.l, r.a.c);
    if (r.a.l !== r.b.l || r.a.c !== r.b.c) Ed.extendTo(g, r.b.l, r.b.c);
  },
  /* rvtask R3 — THE task-status byte op, shared by Ctrl+L (toggleCheck) and the
     reading-view click. Obsidian 1.13.7 (docs/recon-rvtask Q1/Q2): the status char
     of the item's "[?]" flips ' ' -> 'x' and ANYTHING else (x X / - > ?) -> ' '.
     One byte, this line only; indent, quote prefix, list marker and the text are
     returned untouched. null = not a task item (nothing to flip). */
  flipTask(line) {
    const m = /^([ \t]*(?:> ?)*(?:[-*+]|\d+[.)])[ \t]+\[)([^\]\n])\]/.exec(line || "");
    if (!m) return null;
    return m[1] + (m[2] === " " ? "x" : " ") + line.slice(m[1].length + 1);
  },
  /* rvtask R3: a reading-view checkbox click on SOURCE line l (the renderer's
     data-line). Goes through the note's editor model like any edit: one undo
     entry per reading-view SESSION (Obsidian Q6: one Ctrl+Z undoes every toggle
     made since entering reading view), the model -> bridge sync, and the normal
     debounced save (scheduleSave; saveBuf owns the watcher-echo guard). The
     session ends when the pane's mode is applied again (Ed.rvEnd from
     applyMode) — so read -> edit -> read starts a new step. */
  rvToggle(g, l) {
    const L0 = Ed.lines(g), nl = Ed.flipTask(L0[l]);
    if (nl == null) return false;
    const v = g.view, cm = Ed.cm(g);
    if (!cm) { Ed.snap(g, "rvtask"); L0[l] = nl; Ed.commit(g); Ed.sync(g); if (typeof scheduleSave === "function") scheduleSave(g); return true; }
    const T = L0.slice(); T[l] = nl;
    // one undo step per session: while the doc is still the one this session's last toggle
    // left, take that step back (CM6 history) and commit pre-session -> now as ONE change;
    // the new change drops the redo entry the take-back made
    const s = v.rvSess;
    if (s && s.doc === cm.state.doc && CMc.undoDepth(cm.state) > 0) CMc.undo(cm);
    const L = Ed.lines(g);
    L.length = 0; for (const x of T) L.push(x);
    Ed.snap(g, "rvtask");
    Ed.commit(g);
    v.rvSess = { doc: cm.state.doc };
    Ed.sync(g);
    if (typeof scheduleSave === "function") scheduleSave(g);
    return true;
  },
  rvEnd(g) { if (g && g.view) g.view.rvSess = null; },
  // [[ autocomplete: replace [start, caret) with [[name]] through the model
  acInsert(g, start, name) {
    const s = Ed.sel(g);
    if (!s) return;
    const L = Ed.lines(g), l = s.b.l, line = L[l];
    const a = line.lastIndexOf("[[", s.b.c);
    if (a < 0) return;
    Ed.replace(g, { a: { l, c: a }, b: { l, c: s.b.c }, empty: false }, "[[" + name + "]]", "ac");
  },
  linkClick(e, a) {
    if (typeof wikiClick === "function") wikiClick(e, a);
  },
  extClick(e, a) {
    if (typeof extClick === "function") extClick(e, a);
  },

  /* ---------- self test (census [edt:...]) ---------- */
  selfTest() {
    const cases = ["# Heading one", "- [ ] task **bold** here", "plain *it* and `co de` #tag",
                   "\t- nested [[Note|alias]] tail", "> quote with [text](http://x/y)", "", "| a | b |",
                   "1. numbered ~~s~~ ==hl== end", "```js", "text with https://x.example/z end",
                   // corpus inherited from the deleted scripts/edit-unit.js (it drove a pure
                   // API that no longer exists; the invariants it protected live on here)
                   "   ", "###### h6", "## **bold** head", "- [x] done", "  - two spaces", "12) twelve",
                   "> > deep", "> - a", "---", "a **b _c_ d** e", "![[Welcome]]", "a#nottag and #tag/sub",
                   "snake_case_word stays", "`a**b`", "text with <b>&amp;", "unclosed **bold",
                   "- **bold item** with [[Link]]", "\t\t1. deep ordered",
                   // R17.7: destinations with parens, on both link forms
                   "[wiki](https://e.example/wiki/Foo_(bar)) tail", "(see https://e.example/a) end",
                   // R29 image embeds: the widget carries NO text node, so the
                   // row's textContent === its source line only if every one of
                   // these lands entirely in markers (and `![](my pic.png)`,
                   // R29.3, is not an image at all — it is plain text)
                   "![[pic.png]]", "![](pic.png)", "![alt text](sub/dir/pic.png)",
                   "![](my%20pic.png)", "![](my pic.png)", "![[Second Note]]",
                   "![[pic.png|300]]", "![](https://x.example/a.png)", "text ![](pic.png) tail",
                   "![](pic.png", "![]()", "![[pic.png#x]]"];
    let bad = 0;
    Ed.edtWhy = "";
    // the label rides in the window-title census, so keep it token-safe
    const fail = m => { bad++; if (!Ed.edtWhy) Ed.edtWhy = String(m).replace(/[^\w.:<>+-]/g, "_").slice(0, 40); };
    const g = { view: {}, lp: document.createElement("div") };
    for (const c of cases) {
      const row = Ed.row(g, c, 0);
      if (row.textContent !== c) { fail("text:" + c); continue; }
      for (let k = 0; k <= c.length; k++) {          // col -> DOM -> col round trip
        const [n, o] = Ed.posOf(row, k, true);
        if (n === row) continue;
        if (Ed.colOf(row, n, o) !== k) { fail("col" + k + ":" + c); break; }
      }
    }
    // R17.6 rendered form: the VISIBLE text of a row (markers live in .mk and
    // are display:none off the caret row) is the source minus those markers.
    // textContent === source above is the reveal-all/source-mode side of the
    // same invariant, and it is also the escaping proof: a row built only from
    // text nodes can never turn "<b>" into markup.
    const vis = s => Ed.nodes(Ed.row(g, s, 0)).filter(n => !n.hid).map(n => n.n.nodeValue).join("");
    const rf = [["**bold**", "bold"], ["*it*", "it"], ["`code`", "code"], ["~~s~~ ==h==", "s h"],
                ["[[Welcome]]", "Welcome"], ["[[Welcome|alias]]", "alias"], ["[ext](http://x.y/z)", "ext"],
                ["#tag", "#tag"], ["# Head", "Head"], ["- alpha", "alpha"], ["\t- nested", "\tnested"],
                ["- [ ] todo", "todo"], ["> quoted", "quoted"], ["1. one", "one"],
                // R29: an image embed paints a WIDGET, so its visible text is
                // empty in both forms — while a NOTE embed is still a link and
                // keeps its label, and a raw space is still literal text
                ["![[pic.png]]", ""], ["![](pic.png)", ""], ["![alt](sub/pic.png)", ""],
                ["![[Second Note]]", "Second Note"], ["![](my pic.png)", "![](my pic.png)"]];
    for (const [s, w] of rf) if (vis(s) !== w) fail("vis:" + s);
    /* R17.6 TOKEN MAP + R17.7 PER-TOKEN REVEAL, on the map itself (pure: no
       selection, no CSS). `tm` is the M73-M78 fixture line, raw length 61:
       **bold** 0..8, *ital* 13..19, `code` 24..30, [[Welcome|alias]] 35..52,
       #tag 57..61 (never a token — its '#' is part of the pill, R17.6).
       Each case is [caret col, expected reveal set] straight off M73-M77;
       the last two are M78 (whole line) and the negative "one column short". */
    const tm = "**bold** and *ital* and `code` and [[Welcome|alias]] and #tag";
    const tt = Ed.row(g, tm, 0)._tok;
    const setOf = (lo, hi) => Ed.tokAt(tt, lo, hi).map(t => t.s + "-" + t.e + ":" + t.kind).join(",") || "-";
    const tcs = [[10, 10, "-"],                       // M73 inside the plain word "and"
                 [15, 15, "13-19:i"],                 // M74 inside *ital*
                 [26, 26, "24-30:code"],              // M75 inside `code`
                 [40, 40, "35-52:wl"],                // M76 inside the wikilink target
                 [55, 55, "-"],                       // M77 two chars before "#tag"
                 [3, 3, "0-8:b"],                     // inside **bold**, nothing else
                 [0, 61, "0-8:b,13-19:i,24-30:code,35-52:wl"]];   // M78 select the whole line
    for (const [lo, hi, w] of tcs) if (setOf(lo, hi) !== w) fail("rv" + lo + "." + hi + ":" + setOf(lo, hi));
    // the block markers are tokens too, with the ranges R17.6 names
    const btk = [["## Head", "0-3:h"], ["- alpha", "0-2:li"], ["\t- alpha", "1-3:li"],
                 ["- [ ] todo", "0-6:task"], ["> quoted", "0-2:bq"],
                 ["[ext](http://x.y/z)", "0-19:lt"], ["1. one", "0-3:li"]];
    for (const [s, w] of btk) {
      const tk = Ed.row(g, s, 0)._tok.map(t => t.s + "-" + t.e + ":" + t.kind).join(",");
      if (tk !== w) fail("tok:" + s + ":" + tk);
    }
    // R17.7 link destinations: balanced parens survive whole (CommonMark), a
    // backslash escape stays literal, whitespace is still not a link, an empty
    // destination still is, and a bare URL leaves the enclosing prose's ')' out.
    const dest = s => { const a = Ed.row(g, s, 0).querySelector("a.lt, a.ext"); return a ? a.dataset.url : null; };
    const dl = [["[w](https://e.example/wiki/Foo_(bar))", "https://e.example/wiki/Foo_(bar)"],
                ["[w](https://e.example/a)", "https://e.example/a"],
                ["[w]()", ""], ["[w](a\\)b)", "a\\)b"], ["[w](a b)", null], ["[w](a(b)", null],
                ["(see https://e.example/a) x", "https://e.example/a"],
                ["https://e.example/wiki/Foo_(bar) x", "https://e.example/wiki/Foo_(bar)"]];
    for (const [s, w] of dl) if (dest(s) !== w) fail("dest:" + s);
    // Home targets (M64-M68), checked on the model, not the DOM
    const hg = { view: { lines: ["- alpha", "\t- beta", "- [ ] alpha", "# Head", "plain", "> quoted", "1. one"] } };
    const hw = [2, 3, 6, 0, 0, 2, 3];
    for (let i = 0; i < hw.length; i++) if (Ed.homeCol(hg, i) !== hw[i]) fail("home:" + i);
    // horizontal steps walk RAW columns and cross line edges (M69-M71)
    const sg = { view: { lines: ["**b** x", "- y"] } };
    const sw = [[0, 2, true, "0.1"], [0, 0, true, "-"], [1, 0, true, "0.7"],
                [0, 7, false, "1.0"], [1, 3, false, "-"], [0, 0, false, "0.1"]];
    for (const [l, c, back, want] of sw) {
      const n = Ed.step(sg, l, c, back);
      if ((n ? n.l + "." + n.c : "-") !== want) fail("step:" + l + "." + c + (back ? "<" : ">"));
    }
    // R17.1 Enter / Shift+Enter, asserted on the MODEL (no DOM, no view): each
    // case is [before, caret l.c, op, after, caret l.c] straight off the MUST
    // list. `enter` is where list semantics live, so it is checked in-app on
    // every load — the smoke's byte assertions are the same claim end to end.
    const ec = [
      ["- alpha\n- beta", 0, 7, 0, "- alpha\n- \n- beta", 1, 2],                        // M1
      ["- alpha\n- beta", 0, 3, 0, "- a\n- lpha\n- beta", 1, 2],                        // M2
      ["- alpha", 0, 0, 0, "\n- alpha", 1, 0],                                          // M3
      ["1. one\n2. two\n3. three", 0, 6, 0, "1. one\n2. \n3. two\n4. three", 1, 3],     // M4 renumber
      ["1. one\n2. two", 1, 4, 0, "1. one\n2. t\n3. wo", 2, 3],                          // M6
      ["- [x] done", 0, 10, 0, "- [x] done\n- [ ] ", 1, 6],                              // M8 always unchecked
      ["- a\n  - b", 1, 5, 0, "- a\n  - b\n  - ", 2, 4],                                 // M11 indent verbatim
      ["1. a\n\t1. b\n2. c", 1, 5, 0, "1. a\n\t1. b\n\t2. \n2. c", 2, 4],                // M12 nested run
      ["- a\n\t- ", 1, 3, 0, "- a\n- ", 1, 2],                                           // M13 empty nested -> outdent
      ["- a\n- ", 1, 2, 0, "- a\n", 1, 0],                                               // M14 empty -> drop marker
      ["- [ ] ", 0, 6, 0, "", 0, 0],                                                     // M16
      ["> quoted", 0, 8, 0, "> quoted\n> ", 1, 2],                                       // M18
      ["> ", 0, 2, 0, "\n", 1, 0],                                                       // M19 empty quote
      ["> - a", 0, 5, 0, "> - a\n> - ", 1, 4],                                           // M20 quote + list
      ["# Head", 0, 6, 0, "# Head\n", 1, 0],                                             // M21 headings never continue
      ["---", 0, 3, 0, "---\n", 1, 0],                                                   // M23
      ["- a\n  cont", 1, 6, 0, "- a\n  cont\n- ", 2, 2],                                 // M24 continuation line
      ["- alpha", 0, 7, 1, "- alpha\n  ", 1, 2],                                         // M26 soft break
    ];
    // rvtask R3: the shared status-byte op, Obsidian Q1/Q2/Q3 byte for byte
    const ft = [
      ["- [ ] a", "- [x] a"], ["- [x] a", "- [ ] a"], ["- [X] a", "- [ ] a"],
      ["- [/] a", "- [ ] a"], ["- [-] a", "- [ ] a"], ["- [>] a", "- [ ] a"], ["- [?] a", "- [ ] a"],
      ["- [ ] inner [ ] and  two  ", "- [x] inner [ ] and  two  "],
      ["    - [ ] n4", "    - [x] n4"], ["\t\t* [x] t", "\t\t* [ ] t"],
      ["> - [ ] q", "> - [x] q"], ["> > + [ ] qq", "> > + [x] qq"],
      ["1. [ ] one", "1. [x] one"], ["12) [x] t", "12) [ ] t"],
      ["[ ] bare", null], ["- plain [ ] x", null], ["text", null], ["- [] e", null],
    ];
    for (const [src, want] of ft) if (Ed.flipTask(src) !== want) fail("flip:" + src);
    const oa = Ed.after, on = Ed.snap;
    let cr = null;
    Ed.after = (gg, l, c) => { cr = { l, c }; };
    Ed.snap = () => {};
    try {
      for (const [src, l, c, op, want, wl, wc] of ec) {
        const eg = { view: { lines: src.split("\n") }, editor: {} };
        const cc = Math.min(c, eg.view.lines[l].length);
        cr = null;
        const es = { a: { l, c: cc }, b: { l, c: cc }, empty: true };
        if (op) Ed.softBreak(eg, es); else Ed.enter(eg, es);
        if (eg.view.lines.join("\n") !== want || !cr || cr.l !== wl || cr.c !== wc) fail("enter:" + src);
      }
      /* R17.2/R17.3/R17.4 on the MODEL: [before, l, c, op, after, l, c] straight
         off the MUST list. Where a MUST caret column exceeds the raw line length
         it is clamped (Obsidian counts a leading TAB as two columns; this engine
         counts raw characters — same caret, different unit). */
      const kc = [
        ["- a\n- b", 1, 3, "tab", "- a\n\t- b", 1, 4],                                   // M27
        ["- a\n- b", 0, 3, "tab", "\t- a\n- b", 0, 4],                                   // M28 first item too
        ["- a\n\t- b", 1, 4, "tab", "- a\n\t\t- b", 1, 5],                               // M29 unbounded
        ["- a\n\t- b", 1, 5, "stab", "- a\n- b", 1, 3],                                  // M30
        ["- a\n- b", 1, 3, "stab", "- a\n- b", 1, 3],                                    // M31 top level = no-op
        ["1. a\n2. b\n3. c", 1, 4, "tab", "1. a\n\t1. b\n2. c", 1, 5],                   // M32 restart + close up
        ["1. a\n\t1. b\n\t2. c", 1, 5, "stab", "1. a\n2. b\n\t1. c", 1, 4],              // M32 mirrored
        ["- alpha", 0, 2, "tab", "\t- alpha", 0, 3],                                     // M33 never a tab at the caret
        ["plain text", 0, 10, "tab", "\tplain text", 0, 11],                             // M34
        ["plain para", 0, 10, "stab", "plain para", 0, 10],                              // M87 no-op outside a list
        ["- a\n\t- b", 0, 3, "tab", "\t- a\n\t\t- b", 0, 4],                             // the sub-tree follows
        ["- a\n- b", 1, 0, "bs", "- a- b", 0, 3],                                        // M36 col0 JOINS
        ["- a\n\t- b", 1, 0, "bs", "- a\t- b", 0, 3],                                    // M37 nested joins too
        ["- a\n- b", 1, 2, "bs", "- a\n-b", 1, 1],                                       // M39 marker is text
        ["- a\n- ", 1, 2, "bs", "- a\n-", 1, 1],                                         // M40 empty item: no outdent
        ["- [ ] a", 0, 6, "bs", "- [ ]a", 0, 5],                                         // M41
        ["> a\n> b", 1, 0, "bs", "> a> b", 0, 3],                                        // M42
        ["1. one\n2. two", 1, 0, "bs", "1. one2. two", 0, 6],                            // M43 join wins
        ["- a\n- b", 0, 3, "del", "- a- b", 0, 3],                                       // M44 forward join
        ["- plain", 0, 7, "ctrl-l", "- [ ] plain", 0, 11],                               // M46
        ["- [ ] a", 0, 7, "ctrl-l", "- [x] a", 0, 7],                                    // M47
        ["- [x] a", 0, 7, "ctrl-l", "- [ ] a", 0, 7],                                    // M48
        ["para", 0, 4, "ctrl-l", "- [ ] para", 0, 10],                                   // M49
        ["- a\n\t- b", 1, 5, "ctrl-l", "- a\n\t- [ ] b", 1, 8],                          // M50
        ["1. a", 0, 4, "ctrl-l", "1. [ ] a", 0, 8],                                      // M51 ordered marker kept
      ];
      for (const [src, l, c, op, want, wl, wc] of kc) {
        const eg = { view: { lines: src.split("\n") }, editor: {} };
        const cc = Math.min(c, eg.view.lines[l].length);
        cr = null;
        const es = { a: { l, c: cc }, b: { l, c: cc }, empty: true };
        if (op === "tab") Ed.indent(eg, es, false);
        else if (op === "stab") Ed.indent(eg, es, true);
        else if (op === "bs") Ed.del(eg, es, false, false);
        else if (op === "del") Ed.del(eg, es, true, false);
        else Ed.toggleCheck(eg, es);
        if (eg.view.lines.join("\n") !== want || !cr || cr.l !== wl || cr.c !== wc) fail(op + ":" + src);
      }
      // M88/M89: a real SELECTION indents/outdents exactly the lines it covers,
      // and a trailing line touched only at col 0 is left alone.
      const sc = [
        ["- a\n- b\n- c", 0, 0, 2, 0, false, "\t- a\n\t- b\n- c"],                       // M88
        ["- a\n\t- b\n\t- c", 1, 0, 2, 4, true, "- a\n- b\n- c"],                        // M89
      ];
      for (const [src, al, ac, bl, bc, out, want] of sc) {
        const eg = { view: { lines: src.split("\n") }, editor: {} };
        cr = null;
        Ed.indent(eg, { a: { l: al, c: ac }, b: { l: bl, c: bc }, empty: false }, out);
        if (eg.view.lines.join("\n") !== want) fail("sel-tab:" + src);
      }
    } finally { Ed.after = oa; Ed.snap = on; }
    return bad;
  },
};

/* ---------- textarea-shaped view of the model ----------
   The [[ autocomplete (acContext/acInsert) and the palette's format commands
   (edEdit: bold/italic/highlight...) were written against a <textarea>. They
   keep working through this shim: reads are model reads, writes go back
   through the model + a full patch, so nothing edits the DOM directly. */
Ed.field = function (g) {
  const L = Ed.lines(g), s = Ed.sel(g) || { a: { l: 0, c: 0 }, b: { l: 0, c: 0 } };
  const off = p => { let o = 0; for (let i = 0; i < p.l && i < L.length; i++) o += L[i].length + 1; return o + p.c; };
  const f = {
    _v: Ed.text(g),
    selectionStart: off(s.a), selectionEnd: off(s.b),
    get value() { return f._v; },
    set value(t) { f._v = t; Ed.setAll(g, t); },
    setSelectionRange(a) { Ed.placeOffset(g, a); },
    dispatchEvent() { if (typeof scheduleSave === "function") scheduleSave(g); },
    focus() { g.lp.focus({ preventScroll: true }); },
  };
  return f;
};
/* The FIRST client rect of selectNodeContents(row).getClientRects(), without
   asking for all the others. A range's rects come in document order — a
   top-level child element's border boxes, then the text rects inside it — so
   the first child that paints anything owns rect 0. The whole-row read built
   one rect per pill and per text run: on a 50000-#tag line that is ~100k
   rects on every census, i.e. on every keystroke (audit #11). */
Ed.firstRect = function (row, r) {
  for (const n of row.childNodes) {
    let rc = null;
    if (n.nodeType === 1) rc = n.getClientRects();
    if (!rc || !rc.length) { r.selectNodeContents(n); rc = r.getClientRects(); }   // text / display:contents
    if (rc.length) return rc[0];
  }
  return null;
};
Ed.selTok = function (g) {                // [sel:<l>.<c>-<l>.<c>] model range
  const s = Ed.sel(g);
  return s ? " [sel:" + s.a.l + "." + s.a.c + "-" + s.b.l + "." + s.b.c + "]" : "";
};
/* rvtask R3/R4: reading-view task checkbox click. ONE delegated listener: the
   renderer (src-tauri main.rs emit) gives each reading-view box data-line = its
   0-based FILE line, so duplicate / nested / quoted / numbered items map
   exactly (spec R2). The native toggle is cancelled — the MODEL decides the
   state (Ed.rvToggle), then the pane re-renders from it at once, with the
   scroll put back (R4, Obsidian Q5 keeps it). Keyboard (Obsidian Q9: Tab to the box,
   Space) arrives here too: Space on a focused checkbox IS a click event. */
document.addEventListener("click", e => {
  const cb = e.target;
  if (!cb || cb.tagName !== "INPUT" || cb.type !== "checkbox" || !cb.hasAttribute("data-line")) return;
  const pv = cb.closest(".preview");
  if (!pv) return;
  e.preventDefault();
  const pane = pv.closest(".pane"), g = pane ? pane._g : null;
  if (!g || g.preview !== pv || !Ed.rvToggle(g, parseInt(cb.dataset.line, 10))) return;
  const st = pv.scrollTop, had = document.activeElement === cb, idx = [...pv.querySelectorAll("input[type=checkbox][data-line]")].indexOf(cb);
  if (typeof preview !== "function") return;
  Promise.resolve(preview(g)).then(() => {
    if (g.preview !== pv) return;
    pv.scrollTop = st;
    const back = pv.querySelectorAll("input[type=checkbox][data-line]")[idx];
    if (back && had) back.focus({ preventScroll: true });
    if (typeof updateTitle === "function") updateTitle();
  });
});

/* rvtask R5 census -> " [rvtask:<boxes>/<checked>] [rvtaskxy:<line>:<x>,<y>|...] [rvtaskst:<scrollTop>]"
   for the FOCUSED group's reading view (both absent otherwise). rvtask counts
   every reading-view task box of the note; rvtaskxy is the painted CENTRE of
   each box that is inside the pane's viewport, keyed by its data-line (source
   line), in document order, first 40 — so a phase clicks published geometry
   for exactly the line it means (duplicates differ by line), never a literal.
   Viewport pixels, same frame as [rvlb:]. rvtaskst = the reading pane scrollTop, so
   a phase proves a toggle did not move the view (R4, +-2px). Registered by ONE ui/census.js line. */
Ed.rvTaskTok = () => {
  const g = typeof fg === "function" ? fg() : null;
  if (!g || typeof isReading !== "function") return "";
  if (!isReading(g)) {
    /* [rvtaskex:x,y] = a click point INSIDE the editor on the first visible
       row that has painted text and no task box (a click there moves the caret,
       never a byte). Ed.geom's [erx:]/[ery:] stop at 40 rows; the rvtask
       fixture is 230 lines, so the Q6 undo step needs its own point. */
    const rows = g.lp ? Ed.rows(g) : null;
    if (!rows) return "";
    const lb = g.lp.getBoundingClientRect(), r = document.createRange();
    for (let i = 0; i < rows.length && i < 400; i++) {
      const q = rows[i].getBoundingClientRect();
      // a row CM6 left half under the scroller's top edge (the scroll restored from
      // reading view lands mid-row) has its centre outside the editor — on the
      // view header — so the point must be wholly inside g.lp AND hit-test to the row
      if (q.top < lb.top) continue;
      if (q.bottom > lb.bottom) break;
      if (rows[i].querySelector("input")) continue;
      const r0 = Ed.firstRect(rows[i], r);
      if (!r0 || !r0.width) continue;
      const x = Math.round(r0.left + 2), y = Math.round(q.top + q.height / 2), hit = document.elementFromPoint(x, y);
      if (!hit || !rows[i].contains(hit)) continue;
      return " [rvtaskex:" + x + "," + y + "]";
    }
    return "";
  }
  if (!g.preview) return "";
  const bs = [...g.preview.querySelectorAll("input[type=checkbox][data-line]")];
  const pr = g.preview.getBoundingClientRect(), xy = [];
  for (const b of bs) {
    const r = b.getBoundingClientRect();
    if (!r.width || r.top < pr.top || r.bottom > pr.bottom) continue;
    xy.push(b.dataset.line + ":" + Math.round(r.left + r.width / 2) + "," + Math.round(r.top + r.height / 2));
    if (xy.length >= 40) break;
  }
  return " [rvtask:" + bs.length + "/" + bs.filter(b => b.checked).length + "] [rvtaskxy:" + (xy.join("|") || "-") + "] [rvtaskst:" + Math.round(g.preview.scrollTop) + "]";
};
/* rvtask R5: [rvtaskxy:]/[rvtaskst:] are geometry, so the census must follow a
   READING-VIEW scroll (nothing else refreshes the title on one — the same
   reason the context menu refreshes it on its own scroll). Debounced: one
   title write per settled scroll, not per wheel tick. */
let rvTaskScT = null;
if (CENSUS_ON) document.addEventListener("scroll", e => {   // census-gate: census-only listener, not registered when OFF
  const t = e.target;
  if (!t || !t.classList || !t.classList.contains("preview")) return;
  clearTimeout(rvTaskScT);
  rvTaskScT = setTimeout(() => { rvTaskScT = null; if (typeof updateTitle === "function") updateTitle(); }, 60);
}, true);

/* ======================================================================
   CM6 EDITOR CORE — CodeMirror 6 is the view and the model.
   One EditorView per tab view (mkView -> Ed.mount). Its state.doc is the
   source of truth; Ed.lines(g) is a lazily rebuilt string[] mirror so the
   model-level helpers above (enter/indent/toggles/renumber, replace-all,
   insdate) keep mutating an array: they call Ed.snap, mutate, then
   Ed.after/Ed.render, and Ed.commit turns the difference into ONE change
   dispatched as its own undo step. g.lp stays the pane scroller (div.lp: display,
   vkeep, scrollTop, the title band, every `.lp ...` selector) and view.dom is
   its only child: CM6 runs auto-height and virtualises against that scroller. The CM6 module objects are the vendored registry's
   (window.__osModules), the same instances plugins will require.
   ====================================================================== */
const CM6 = window.__osModules || {};
const CMs = CM6["@codemirror/state"], CMv = CM6["@codemirror/view"], CMc = CM6["@codemirror/commands"];
/* ED-3: a plugin Editor-API edit carries Ed.PLUGIN_EDIT; Ed.EDIT_KIND remembers the kind + time of the last
   doc change so the undo grouping below can join a plugin's follow-up edit to its own previous one */
Ed.PLUGIN_EDIT = CMs && CMs.Annotation.define();
Ed.HOME_SEL = CMs && CMs.Annotation.define();   // goal mt-fix: Ed.home selection (no raw row)
Ed.EDIT_KIND = CMs && CMs.StateField.define({
  create: () => ({ k: "", t: 0 }),
  update: (v, tr) => !tr.docChanged ? v : { k: tr.annotation(Ed.PLUGIN_EDIT) ? "plugin" : tr.isUserEvent("input.type") ? "type" : "other", t: tr.annotation(CMs.Transaction.time) || Date.now() },
});
Object.assign(Ed, {
  cm(g) { return g && g.view && g.view.cm || null; },
  view(g) { return Ed.cm(g); },                        // the Obsidian Editor shim's accessor (ui/plugins/editor-api.js)
  /* editor-api: ONE Compartment per view holds every plugin editor extension; its content is
     Ed.pxExt(v) (editor-api.js replaces this [] once a plugin loads), read whenever a state is
     built (mount, setText) and pushed to open views by PX.reconfigure */
  PX: CMs && CMs.Compartment ? new CMs.Compartment() : null,
  pxExt() { return []; },
  /* ---------- model ---------- */
  lines(g) {
    const v = g.view, cm = v && v.cm;
    if (!cm) { if (!v.lines) v.lines = [""]; return v.lines; }
    const d = cm.state.doc;
    if (v._Ld !== d) { v.lines = d.toJSON(); v._Ld = d; }
    return v.lines;
  },
  text(g) { const cm = Ed.cm(g); return cm ? cm.state.doc.toString() : Ed.lines(g).join("\n"); },
  sync(g) { g.editor.value = Ed.text(g); },            // persistence bridge only
  off(g, l, c) {                                       // (line,col) -> doc offset, clamped
    if (!Ed.cm(g)) { const L = Ed.lines(g), n = Math.max(0, Math.min(l, L.length - 1)); let o = 0;   // a cm-less model (selfTest fixtures)
      for (let i = 0; i < n; i++) o += L[i].length + 1; return o + Math.max(0, Math.min(c, L[n].length)); }
    const d = g.view.cm.state.doc, n = Math.max(1, Math.min(l + 1, d.lines)), ln = d.line(n);
    return ln.from + Math.max(0, Math.min(c, ln.length));
  },
  lc(g, o) {
    if (!Ed.cm(g)) { const L = Ed.lines(g); let l = 0; while (l < L.length - 1 && o > L[l].length) { o -= L[l].length + 1; l++; } return { l, c: Math.min(o, L[l].length) }; }
    const ln = g.view.cm.state.doc.lineAt(o); return { l: ln.number - 1, c: o - ln.from }; },
  offOf(g, l, c) { return Ed.off(g, l, c); },
  offsetOf(g, p) { return Ed.off(g, p.l, p.c); },
  placeOffset(g, o) { const p = Ed.lc(g, Math.max(0, Math.min(o, g.view.cm.state.doc.length))); Ed.place(g, p.l, p.c); },
  /* a model-level caller mutated the Ed.lines array in place: dispatch the
     difference as ONE minimal change (common prefix/suffix), its own undo step */
  commit(g) {
    const v = g.view, cm = v.cm, L = v.lines;
    if (!cm || !L) return false;
    // the mirror must be OF the current doc: after setText (or typing since Ed.lines()) the doc moved
    // on, and diffing a stale mirror dispatched a revert (reloadInPlace -> lpRender undid the reload)
    if (v._Ld !== cm.state.doc) { v._snap = null; return false; }
    const oldT = cm.state.doc.toString(), newT = L.join("\n");
    if (oldT === newT) { v._Ld = cm.state.doc; return false; }
    const m = Math.min(oldT.length, newT.length);
    let p = 0, s = 0;
    while (p < m && oldT.charCodeAt(p) === newT.charCodeAt(p)) p++;
    while (s < m - p && oldT.charCodeAt(oldT.length - 1 - s) === newT.charCodeAt(newT.length - 1 - s)) s++;
    const kind = v._snap || "edit"; v._snap = null;
    cm.dispatch({ changes: { from: p, to: oldT.length - s, insert: newT.slice(p, newT.length - s) },
                  annotations: [CMc.isolateHistory.of("full"), CMs.Transaction.userEvent.of("os." + kind)] });
    v.lines = L; v._Ld = cm.state.doc;                 // the caller's array IS the doc now
    return true;
  },
  setText(g, t, load) {                                // load / external replace: fresh history
    const v = g.view, s = String(t == null ? "" : t);
    if (!v.cm) { v.lines = s.split("\n"); Ed.sync(g); return; }
    // note_open: the fresh state's LP plugin builds its decorations in setState, so the full
    // render that follows a load would re-parse and redraw every visible row a second time.
    // A load busts the parse cache HERE (links/images re-resolve in that one build) and
    // marks the state; render(full) skips its bust while the doc is still that state.
    if (load) Ed.lpBust();
    if (v._pxI != null) v._ext[v._pxI] = Ed.PX.of(Ed.pxExt(v));   // editor-api: the fresh state carries the CURRENT plugin extensions
    v.cm.setState(CMs.EditorState.create({ doc: s, extensions: v._ext }));
    v._fresh = load ? v.cm.state : null;
    v._Ld = null; v._snap = null;
    Ed.sync(g);
  },
  setAll(g, text) { Ed.snap(g, "cmd"); const v = g.view; v.lines = String(text).split("\n"); if (v.cm) v._Ld = v.cm.state.doc; Ed.render(g, -1, 0); },
  snap(g, kind) { g.view._snap = kind; },              // the next commit is its own undo step
  after(g, l, c) {                                     // model changed -> doc + caret
    Ed.commit(g);
    Ed.place(g, l, c);
  },
  render(g, caret, col, full) {
    const t0 = performance.now(), v = g.view;
    if (full && v._fresh !== v.cm.state) { Ed.lpBust(); if (v._lpFx) v.cm.dispatch({ effects: v._lpFx.of(null) }); }   // vault changed: links/images re-resolve
    const did = Ed.commit(g);
    g.lpLines = v.cm.state.doc.lines;
    v.note = typeof curOf === "function" ? curOf(g) : null;
    if (caret != null && caret >= 0) Ed.place(g, caret, col || 0);
    if (!did && typeof perf !== "undefined" && perf.mark)       // a change marks ed_patch in onUpdate
      perf.mark("ed_patch", t0, { lines: g.lpLines, patched: 0, full: false });
    return Promise.resolve();
  },
  patchRow() {},                                       // CM6 redraws what changed
  replace(g, s, text, kind) {                          // splice [a,b) with `text`
    if (!Ed.cm(g)) {                                   // a cm-less model (selfTest fixtures): splice the array
      const L = Ed.lines(g), A = s.a, B = s.b, ins = String(text == null ? "" : text).replace(/\r\n?/g, "\n").split("\n");
      const head = L[A.l].slice(0, A.c), tail = (L[B.l] || "").slice(B.c);
      L.splice(A.l, B.l - A.l + 1, ...(ins.length === 1 ? [head + ins[0] + tail] : [head + ins[0], ...ins.slice(1, -1), ins[ins.length - 1] + tail]));
      Ed.snap(g, kind || "type");
      return Ed.after(g, A.l + ins.length - 1, ins.length === 1 ? A.c + ins[0].length : ins[ins.length - 1].length);
    }
    const cm = g.view.cm, a = Ed.off(g, s.a.l, s.a.c), b = Ed.off(g, s.b.l, s.b.c);
    const ins = String(text == null ? "" : text).replace(/\r\n?/g, "\n");
    kind = kind || "type"; g.view._snap = null;
    cm.dispatch({ changes: { from: a, to: b, insert: ins }, selection: { anchor: a + ins.length },
                  scrollIntoView: true,
                  annotations: kind === "type" ? [CMs.Transaction.userEvent.of("input.type")]
                    : [CMc.isolateHistory.of("full"), CMs.Transaction.userEvent.of("os." + kind)] });
  },
  slice(g, s) { if (!Ed.cm(g)) return Ed.text(g).slice(Ed.off(g, s.a.l, s.a.c), Ed.off(g, s.b.l, s.b.c)); return g.view.cm.state.sliceDoc(Ed.off(g, s.a.l, s.a.c), Ed.off(g, s.b.l, s.b.c)); },
  /* ---------- selection, in model coords {l, c} ---------- */
  sel(g) {
    const cm = Ed.cm(g); if (!cm) return null;
    const r = cm.state.selection.main, a = Ed.lc(g, r.from), b = Ed.lc(g, r.to);
    return { a, b, empty: r.empty };
  },
  caret(g) { const s = Ed.sel(g); return s ? s.b : null; },
  caretLC(g) { const c = Ed.caret(g) || { l: 0, c: 0 }; return [c.l, c.c]; },
  focusPos(g) { const cm = Ed.cm(g); return cm ? Ed.lc(g, cm.state.selection.main.head) : null; },
  place(g, l, c, focus) {                              // caret to (line,col); never scrolls (R17)
    const cm = Ed.cm(g); if (!cm) return;
    cm.dispatch({ selection: { anchor: Ed.off(g, l, c) } });
    if (focus !== false && !cm.hasFocus) cm.focus();
    Ed.mark(g, Ed.focusPos(g).l);
  },
  /* goal mt-fix: the caret at (line,col) WITHOUT opening the raw row there — a freshly opened note (and reading ->
     edit) puts the caret at 0:0, focused, as in stock, so typing lands at the top and plugins see the selection
     change (dataview inline queries, templater cursor); the row only opens once the user moves or types. */
  home(g, l, c, focus) {
    const cm = Ed.cm(g); if (!cm) return;
    cm.dispatch({ selection: { anchor: Ed.off(g, l, c) }, annotations: Ed.HOME_SEL ? [Ed.HOME_SEL.of(true)] : [] });
    if (focus !== false && !cm.hasFocus) cm.focus();
    Ed.mark(g, -1);
  },
  extendTo(g, l, c) {
    const cm = Ed.cm(g); if (!cm) return;
    cm.dispatch({ selection: { anchor: cm.state.selection.main.anchor, head: Ed.off(g, l, c) } });
    Ed.mark(g, l);
  },
  selectAll(g) {
    const cm = Ed.cm(g); if (!cm) return;
    cm.dispatch({ selection: { anchor: 0, head: cm.state.doc.length } });
    Ed.census();
  },
  // R17 undo/redo re-rendered the caret into the note, which took the keyboard there
  // (find bar Ctrl+Z hands focus back to the text, R26.20) — CM6 history does not focus
  undo(g) { const cm = Ed.cm(g); if (cm) { CMc.undo(cm); if (!cm.hasFocus) cm.focus(); } },
  redo(g) { const cm = Ed.cm(g); if (cm) { CMc.redo(cm); if (!cm.hasFocus) cm.focus(); } },
  focus(g) { const cm = Ed.cm(g); if (cm) cm.focus(); },
  pos(g, node, off) {                                  // DOM position -> {l, c}
    const cm = Ed.cm(g); if (!cm || !node || !cm.contentDOM.contains(node)) return null;
    try { return Ed.lc(g, cm.posAtDOM(node, off)); } catch (_) { return null; }
  },
  rowOf(node) {
    const e = node && (node.nodeType === 3 ? node.parentElement : node);
    return e && e.closest ? e.closest(".cm-line") : null;
  },
  rowAt(g, l) {                                        // the rendered line l (null off the viewport)
    const cm = Ed.cm(g); if (!cm) return null;
    const d = cm.state.doc; if (l < 0 || l >= d.lines) return null;
    const from = d.line(l + 1).from, vp = cm.viewport;
    if (from < vp.from || from > vp.to) return null;
    try { const { node } = cm.domAtPos(from); return Ed.rowOf(node); } catch (_) { return null; }
  },
  rows(g) { const cm = Ed.cm(g); return cm ? cm.contentDOM.querySelectorAll(".cm-line") : []; },
  wake(g, l) { return Ed.rowAt(g, l); },
  wakeView() {},                                       // CM6 renders its own viewport
  reveal() {},                                         // LP reveal = decorations (selection-driven)
  /* ---------- census readers on CM6 geometry ---------- */
  rvTok(g) {
    if (!g.lp) return "";
    if (g.lp.classList.contains("src")) return " [rv:src]";
    return " [rv:" + ((g.lp._rvs && g.lp._rvs.length) ? g.lp._rvs.join(",") : "-") + "]";
  },
  /* ---------- wiring ---------- */
  G(v) { const p = v.cm && v.cm.dom.closest(".pane"); return p && p._g ? p._g : v.g; },
  /* perfslow (note_open ceiling 100 ms): the first note that shows a heading size, a
     fence or inline code paid WebKit's font setup for each new face/size inside its
     open — style recalc resolves a font per (family, weight, style, size) and the
     first use of each one builds it (box, perfslow fixture: an 81-line note with
     h2/h3 + a fence spent 40 ms in style for 58 elements; the same rows without those
     faces 2-3 ms). Done once after boot instead, in idle time, ONE face per idle
     callback, at most 50 ms apart (~13 ms each on the box: no step holds the main
     thread long, all 12 land well inside the first second after boot): an
     off-screen, hidden .lp row in the pane with that face, styled + laid out, removed
     in the same task (no frame and no other script sees it). A face it does not cover,
     or one an open reaches first, simply costs what it cost before. */
  FACES: ["h h1", "h h2", "h h3", "h h4", "h h5", "h h6", "b", "i", "bi", "code", "fence", "task"],
  fontWarm(g) {
    if (Ed._fw || !g || !g.lp) return;
    Ed._fw = { i: 0, ms: 0 };
    const idle = typeof requestIdleCallback === "function"
      ? f => requestIdleCallback(f, { timeout: 50 }) : f => setTimeout(f, 16);
    const step = () => {
      const c = Ed.FACES[Ed._fw.i++], host = g.lp && g.lp.parentNode;
      if (!c || !host || !host.isConnected) return;
      const t0 = performance.now(), w = Ed.el("div", "lp"), row = Ed.el("div", "cm-line lprow"), s = Ed.el("span", c);
      w.setAttribute("aria-hidden", "true");
      w.style.cssText = "position:absolute;left:-10000px;top:0;width:600px;visibility:hidden;pointer-events:none";
      s.textContent = "Warm up the quick fox 0123456789 *_-[]()";
      row.appendChild(s); w.appendChild(row); host.appendChild(w);
      void w.offsetHeight;
      w.remove();
      Ed._fw.ms += performance.now() - t0;
      idle(step);
    };
    idle(step);
  },
  /* perfslow (note_open ceiling 100 ms): CM6 mounts its style modules on EVERY new
     EditorView and EVERY setState (view.mountStyles -> style-mod 4.1.3 StyleSet.mount).
     With the document as root, style-mod keeps one <style> tag and rewrites its
     textContent on every mount, even when the module list is unchanged: the sheet is
     replaced and the whole document restyles (box, perfslow fixture: 13-17 ms per
     rewrite; a note open into a new view paid it twice, mkView + setText). A mount
     that would leave the set exactly as it is — style-mod's own walk: every module
     already present, in order — is skipped; a new or reordered module still takes
     style-mod's own path. Hence also ONE shared view theme (Ed.viewTheme): a theme
     built per mount is a NEW module each time and would always rewrite. */
  styleOnce() {
    if (Ed._smOnce) return;
    for (const s of Object.getOwnPropertySymbols(document)) {
      const set = document[s];
      if (!set || typeof set.mount !== "function" || !Array.isArray(set.modules) || !set.styleTag) continue;
      const mount = set.mount;
      set.mount = function (mods, root) {
        const m = this.modules; let j = 0, same = this.styleTag.parentNode === (root.head || root);
        for (let k = 0; same && k < mods.length; k++) { const i = m.indexOf(mods[k]); if (i < j) same = false; else j = i + 1; }
        return same ? undefined : mount.call(this, mods, root);
      };
      Ed._smOnce = true;
      return;
    }
  },
  mount(v) {
    const G = () => Ed.G(v);
    v._ext = [
      CMc.history({ newGroupDelay: Ed.UNDO_IDLE_MS }),
      // R17.8: only an uninterrupted TYPING burst coalesces; every other edit
      // kind (CM6's own paste/drop/cut included) is its own undo step
      // ED-3: a plugin's Editor-API edit opens its own step, and its follow-up edit within
      // PLUGIN_JOIN_MS (Obsidian's CM6 newGroupDelay) joins it, as in stock: templater's
      // insert + cursor jump undo as ONE step; typing after a plugin edit starts a new one
      Ed.EDIT_KIND,
      CMs.EditorState.transactionExtender.of(tr => {
        if (!tr.docChanged || tr.isUserEvent("undo") || tr.isUserEvent("redo") || tr.annotation(CMc.isolateHistory) != null) return null;
        const last = tr.startState.field(Ed.EDIT_KIND, false), plug = !!tr.annotation(Ed.PLUGIN_EDIT);
        if (plug) return last && last.k === "plugin" && (tr.annotation(CMs.Transaction.time) || Date.now()) - last.t < Ed.PLUGIN_JOIN_MS
          ? null : { annotations: CMc.isolateHistory.of("before") };
        if (tr.isUserEvent("input.type")) return last && last.k === "plugin" ? { annotations: CMc.isolateHistory.of("before") } : null;
        return { annotations: CMc.isolateHistory.of("full") };
      }),
      // the editor's own keys (Ed.onKey) are a keymap `any` binding, not a keydown
      // handler: CM6 merges every keymap's commands for a key into one run list in
      // precedence order, so a plugin binding at a higher precedence
      // (registerEditorExtension(Prec.highest(keymap.of([{key:"Tab", run}])))) runs
      // first and, when its command returns false, ours still runs. A keydown
      // handler here ran ahead of the keymap handler and swallowed Tab/Enter/... for
      // every plugin keymap (Advanced Tables' Tab only indented).
      CMv.keymap.of([{ any: (view, e) => { Ed.onKey(G(), e); return e.defaultPrevented; } }]),
      CMv.EditorView.domEventHandlers({
        beforeinput: (e) => {
          const g = G();
          if (g && !v._sp && !e.isComposing && typeof otel !== "undefined" && otel.begin)
            v._sp = otel.begin("key_to_paint", { key: e.inputType, note_lines: g.lpLines || 0 });
          return false;
        },
        blur: () => { if (typeof hideAc === "function") setTimeout(hideAc, 100); return false; },
        // gh-paste (#19): a PNG on the clipboard (a screenshot) is saved into the vault and
        // embedded, like stock (ui/main.js attachPaste); true = claimed, CM6 does not paste text
        paste: (e) => typeof attachPaste === "function" ? attachPaste(G(), e) : false,
      }),
      CMv.EditorView.lineWrapping,
      CMv.EditorView.contentAttributes.of({ spellcheck: "false", autocorrect: "off", autocapitalize: "off" }),
      Ed.viewTheme || (Ed.viewTheme = CMv.EditorView.theme({ "&.cm-focused": { outline: "none" }, ".cm-scroller": { fontFamily: "inherit", lineHeight: "inherit", overflow: "visible" },
                             ".cm-content": { padding: "0" }, ".cm-line": { padding: "0" } })),
      CMv.EditorView.updateListener.of(u => Ed.onUpdate(G(), v, u)),
      Ed.lpExt ? Ed.lpExt(v, G) : [],
      Ed.hlExt(v),
    ];
    if (Ed.PX) { v._pxI = v._ext.length; v._ext.push(Ed.PX.of(Ed.pxExt(v))); }   // editor-api: plugin extensions (empty until a plugin loads)
    const cm = new CMv.EditorView({ state: CMs.EditorState.create({ doc: (v.lines || [""]).join("\n"), extensions: v._ext }) });
    Ed.styleOnce();
    const lp = v.lp;
    v.cm = cm; v._Ld = null;
    lp.textContent = ""; lp.appendChild(cm.dom); lp._cm = cm;
    // g.lp.focus() (tab switch, palette close, title edit) = focus the editor
    lp.focus = () => { if (!cm.hasFocus) cm.focus(); };
    Ed.wheelOwn(lp);
    return lp;
  },
  onUpdate(g, v, u) {
    Ed.kroMap(u);
    if (!g || g.view !== v) { if (u.docChanged && v.editor) v.editor.value = u.state.doc.toString(); return; }
    if (u.docChanged) {
      const t0 = performance.now(), d = u.state.doc;
      g.lpLines = u.state.doc.lines;
      // R25.13d: the search-jump offsets follow EVERY doc change (typed keys go through
      // CM6 input, not Ed.replace); last change first, so earlier offsets stay in A coords
      if (typeof scLive === "function" && scLive(g)) {
        const ch = [];
        u.changes.iterChanges((fa, ta, fb, tb, ins) => ch.push([fa, ta, ins.length]));
        for (let i = ch.length - 1; i >= 0; i--) scShift(g, ch[i][0], ch[i][1], ch[i][2]);
      }
      Ed.sync(g);
      if (typeof scheduleSave === "function") scheduleSave(g);
      if (typeof showAc === "function") showAc(g);
      if (v._sp) { const sp = v._sp; v._sp = null; if (otel.paint) otel.paint(sp); }
      if (typeof perf !== "undefined" && perf.mark) {
        // R18 ed_patch = the JS work after ONE doc change (CM6 redraws only the
        // lines it touched); "patched" = lines the change spans in the new doc.
        let n = 0;
        u.changes.iterChangedRanges((fa, ta, fb, tb) => { n += d.lineAt(tb).number - d.lineAt(fb).number + 1; });
        perf.mark("ed_patch", t0, { lines: d.lines, patched: n, full: n >= d.lines && d.lines > 3 });
      }
    }
    // goal mt-fix: the caret Ed.home placed (note open, reading -> edit) opens no raw row: [mode:lp] stays bare
    if (u.selectionSet || u.docChanged) {
      if (!u.docChanged && Ed.HOME_SEL && u.transactions.some(t => t.annotation(Ed.HOME_SEL))) Ed.census();
      else { const p = Ed.focusPos(g); Ed.mark(g, p ? p.l : -1); }
    }
  },
  /* keys CM6 does not own: Enter (list continuation), Backspace/Delete (model
     deletes), Tab, Home/End, raw-column arrows, undo/redo, select-all — the
     R17 semantics, now on the CM6 selection */
  onKey(g, e) {
    if (!g || e.isComposing) return;
    if (typeof acKeydown === "function") { acKeydown(g, e); if (e.defaultPrevented) return; }
    const s = Ed.sel(g); if (!s) return;
    const mod = e.ctrlKey || e.metaKey;
    if (e.key === "Enter" && !mod && !e.altKey) {
      e.preventDefault();
      if (e.shiftKey) Ed.softBreak(g, s); else Ed.enter(g, s);
      return;
    }
    if ((e.key === "Backspace" || e.key === "Delete") && !e.shiftKey && !e.altKey) {
      e.preventDefault();
      return Ed.del(g, s, e.key === "Delete", mod);
    }
    if (e.key === "Tab" || e.code === "Tab") { e.preventDefault(); return Ed.indent(g, s, e.shiftKey); }
    if (e.key === "Enter" && mod) { e.preventDefault(); return; }
    const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    if (k === "a" && mod) { e.preventDefault(); return Ed.selectAll(g); }
    if (k === "z" && mod) { e.preventDefault(); return e.shiftKey ? Ed.redo(g) : Ed.undo(g); }
    if (k === "y" && mod) { e.preventDefault(); return Ed.redo(g); }
    if (e.key === "Home" || e.key === "End" || e.key === "ArrowLeft" || e.key === "ArrowRight") Ed.cmStart();
    if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      // Ctrl/Cmd+Home/End: document start / end; bare Home/End: the line's (smart) home / end
      const f = Ed.focusPos(g) || s.b, L = Ed.lines(g), end = e.key === "End";
      const l = mod ? (end ? L.length - 1 : 0) : f.l;
      const c = mod ? (end ? L[l].length : 0) : end ? L[l].length : (f.c === Ed.homeCol(g, l) ? 0 : Ed.homeCol(g, l));
      if (e.shiftKey) Ed.extendTo(g, l, c); else Ed.place(g, l, c);
      return Ed.cmEnd(g);
    }
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      if (mod || e.altKey) { Ed.cmT0 = -1; return; }
      const back = e.key === "ArrowLeft";
      if (!e.shiftKey && !s.empty) { e.preventDefault(); const p = back ? s.a : s.b; Ed.place(g, p.l, p.c); return Ed.cmEnd(g); }
      const f = (e.shiftKey && Ed.focusPos(g)) || (back ? s.a : s.b);
      const n = Ed.step(g, f.l, f.c, back);
      if (!n) { Ed.cmT0 = -1; return; }
      e.preventDefault();
      if (e.shiftKey) Ed.extendTo(g, n.l, n.c); else Ed.place(g, n.l, n.c);
      return Ed.cmEnd(g);
    }
    Ed.cmT0 = -1;
  },
});
/* ---------- census producers on CM6 geometry (same token grammar) ---------- */
// the client box of source line l: the rendered .cm-line, or the height map's
// block for a line outside the viewport
Ed.lineBox = function (g, l) {
  const cm = Ed.cm(g), row = Ed.rowAt(g, l);
  if (row) return row.getBoundingClientRect();
  const b = cm.lineBlockAt(cm.state.doc.line(l + 1).from), c = cm.contentDOM.getBoundingClientRect();
  const top = cm.documentTop + b.top;
  return { top, bottom: top + b.height, height: b.height, left: c.left, right: c.right, width: c.width };
};
Ed.geom = function (g) {
  const cm = Ed.cm(g);
  if (!cm || !g.lp || !g.lp.isConnected) return "";
  const n = cm.state.doc.lines;
  if (n > 40) return "";
  const b = cm.contentDOM.getBoundingClientRect();   // the text column
  let ys = "", xs = "", cb = "";
  const r = document.createRange();
  for (let i = 0; i < n; i++) {
    const q = Ed.lineBox(g, i), row = Ed.rowAt(g, i);
    ys += (i ? "," : "") + Math.round(q.top + q.height / 2);
    const box = row && row.querySelector("input.tbox");
    if (box) {
      const bb = box.getBoundingClientRect();
      if (bb.width) cb += (cb ? "," : "") + i + "@" + Math.round(bb.left + bb.width / 2);
    }
    const r0 = row ? Ed.firstRect(row, r) : null;
    xs += (i ? "," : "") + Math.round(r0 ? r0.left : q.left);
  }
  return " [edx:" + Math.round(b.left) + "] [erx:" + xs + "] [ery:" + ys + "]"
    + (cb ? " [ecb:" + cb + "]" : "");
};
/* ---------- KEY BEFORE CLICK (live-preview caret, goal top10-func) ----------
   WebKitGTK hands key and mouse events to the page through separate queues: a
   key still waiting on a busy page is overtaken by a LATER click, so the click
   moves the caret first and the key (typed before it) lands at the click point
   ('**bold** XXand' + a stray space on the clicked line). event.timeStamp keeps
   the input's own time, so the inversion is visible here: a printable keydown
   stamped BEFORE the last mousedown arrived after it. It is typed where the
   caret was when that mousedown came (mapped through every change since),
   never at the click point; consecutive overtaken keys follow each other.
   The recorded position belongs to ONE document: a note load swaps the state
   (setState, no transaction, so onUpdate never sees it) and the position is
   then an offset into a different, maybe shorter doc. mapPos past a changeset's
   length throws a RangeError, and thrown from the top of onUpdate it skipped
   the model sync of EVERY later edit (srclick R25.13m: a search-hit click
   recorded ZC-Click's caret, the hit loaded ZC-Second, and 18 of 19
   BackSpaces were lost). So the record carries its doc and is dropped the
   moment the editor's doc is not that doc any more.
   Census [kro:<keys re-placed>/<ms the last keydown waited>/<stale records dropped>]. */
Ed.kro = { md: null, n: 0, lag: 0, st: 0 };
Ed.kroView = function () {
  const a = document.activeElement, h = a && a.closest ? a.closest(".cm-editor") : null;
  return h && CMv && CMv.EditorView.findFromDOM ? CMv.EditorView.findFromDOM(h) : null;
};
Ed.kroMap = function (u) {                  // keep the pre-click position on the text it pointed at
  const m = Ed.kro.md; if (!m || m.cm !== u.view || !u.docChanged) return;
  if (u.startState.doc !== m.doc || m.pos > u.startState.doc.length) { Ed.kro.md = null; Ed.kro.st++; return; }   // another doc: stale
  m.pos = u.changes.mapPos(m.pos, 1); m.doc = u.state.doc;
};
if (typeof window !== "undefined" && typeof window.addEventListener === "function") {   // a headless loader (tests/listtoggle.rs) has no event target
window.addEventListener("mousedown", e => {
  const cm = e.button === 0 ? Ed.kroView() : null, r = cm && cm.state.selection.main;
  Ed.kro.md = r && r.empty && typeof e.timeStamp === "number" && e.timeStamp > 0 ? { t: e.timeStamp, cm, pos: r.head, doc: cm.state.doc } : null;
}, true);
window.addEventListener("keydown", e => {
  const now = performance.now(), m = Ed.kro.md;
  if (typeof e.timeStamp === "number" && e.timeStamp > 0 && e.timeStamp <= now + 1) Ed.kro.lag = Math.round(now - e.timeStamp);
  if (!m || !(e.timeStamp > 0 && e.timeStamp < m.t) || e.isComposing || e.ctrlKey || e.metaKey || e.altKey
      || typeof e.key !== "string" || [...e.key].length !== 1 || !m.cm.dom.isConnected) return;
  if (m.cm.state.doc !== m.doc) { Ed.kro.md = null; Ed.kro.st++; return; }   // the doc was swapped (a note load) since that mousedown
  e.preventDefault(); e.stopImmediatePropagation();
  const p = Math.min(m.pos, m.cm.state.doc.length);
  m.cm.dispatch({ changes: { from: p, insert: e.key }, annotations: [CMs.Transaction.userEvent.of("input.type")] });
  m.pos = p + e.key.length; m.doc = m.cm.state.doc; Ed.kro.n++;
}, true);
}
Ed.kroTok = function () { return " [kro:" + Ed.kro.n + "/" + Ed.kro.lag + "/" + Ed.kro.st + "]"; };
Ed.caretXY = function (g) {               // caret rect relative to the pane box
  const cm = Ed.cm(g), pr = g.pane.getBoundingClientRect();
  const c = cm && cm.coordsAtPos(cm.state.selection.main.head);
  return c ? [c.left - pr.left, c.bottom - pr.top] : [16, 16];
};
Ed.crtTok = function () {
  const g = typeof fg === "function" ? fg() : null, lp = g && g.lp, cm = Ed.cm(g);
  if (!lp || !cm || !lp.isConnected || !lp.getClientRects().length) return "";
  const R = Math.round, s = window.getSelection(), cs = getComputedStyle(lp), b = lp.getBoundingClientRect();
  const ox = R(b.left + lp.clientLeft + parseFloat(cs.paddingLeft)), oy = R(b.top + lp.clientTop + parseFloat(cs.paddingTop) - lp.scrollTop);
  let k = "none", x = 0, y = 0, h = 0, lc = "-";
  if (s && s.rangeCount && s.focusNode) {
    const n = s.focusNode;
    if (!lp.contains(n)) k = "out";
    else {
      k = n === lp || n === cm.contentDOM || n === cm.dom ? "lp"
        : n.nodeType === 3 ? (n.parentElement && n.parentElement.getClientRects().length ? "txt" : "hid")
        : n.nodeName === "BR" ? "br" : n.classList && n.classList.contains("cm-line") ? "row" : "el";
      const r = document.createRange();
      try { r.setStart(n, s.focusOffset); r.collapse(true); } catch (_) { /* a stale offset: no rect */ }
      const q = r.getClientRects(), c = q.length ? q[0] : r.getBoundingClientRect();
      x = R(c.left); y = R(c.top); h = R(c.height);
      const p = Ed.pos(g, n, s.focusOffset);
      if (p) lc = p.l + "." + p.c;
    }
  }
  const r0 = Ed.rowAt(g, 0), cb = r0 ? r0.getBoundingClientRect() : null;
  const t0 = r0 ? Ed.firstRect(r0, document.createRange()) : null;
  return " [crt:" + k + "|" + x + "," + y + "," + h + "|lp=" + ox + "," + oy
    + "|col=" + (cb ? R(cb.left) + "," + R(cb.right) : "-")
    + "|t0=" + (t0 ? R(t0.left) + "," + R(t0.top) : cb ? R(cb.left) + "," + R(cb.top) : "-")
    + "|ae=" + (cm.hasFocus ? 1 : 0) + "|" + lc + "]";
};
/* ---------- source line <-> scroller pixels, on CM6's height map ----------
   R17 walked g.lp.children (one row per line); CM6 renders only its viewport,
   so line geometry comes from lineBlockAt*, which covers unrendered lines too. */
Ed.lineAtY = function (g, y) {            // client y -> {l, f}: the line under y, f = fraction into it
  const cm = Ed.cm(g); if (!cm) return null;
  const b = cm.lineBlockAtHeight(y - cm.documentTop), d = cm.state.doc;
  const f = b.height > 0 ? Math.min(1, Math.max(0, (y - cm.documentTop - b.top) / b.height)) : 0;
  return { l: d.lineAt(b.from).number - 1, f };
};
/* the line at the scroller's top edge (+slack px), fractional. DOM FIRST: the
   drawn rows are the truth the user sees. The height map is only as fresh as
   CM6's last measure, which runs in a rAF — a Ctrl+E keydown handled after a
   wheel walk but before that frame read a stale map (gate: lp showed L137,
   reading landed on L150). The map answers only when y falls in a gap. */
Ed.topLine = function (g, slack) {
  const cm = Ed.cm(g); if (!g.lp || !cm) return null;
  const y = g.lp.getBoundingClientRect().top + (slack == null ? 1 : slack), d = cm.state.doc;
  for (const el of cm.contentDOM.children) {
    const r = el.getBoundingClientRect();
    if (r.bottom <= y) continue;
    if (r.top > y + 0.5 || r.height <= 0 || el.classList.contains("cm-gap")) break;   // y sits in a gap
    let a, b;
    try { a = d.lineAt(cm.posAtDOM(el, 0)).number - 1; b = d.lineAt(cm.posAtDOM(el, el.childNodes.length)).number - 1; }
    catch (e) { break; }
    // one element can span several source lines (a block widget): spread y over them
    const f = Math.min(1, Math.max(0, (y - r.top) / r.height)) * (b - a + 1);
    return a + Math.min(b - a + 1 - 1e-6, f);
  }
  const p = Ed.lineAtY(g, y);
  return p ? p.l + p.f : null;
};
Ed.scrollToLine = function (g, line) {    // put the fractional source line at the scroller's top, clamped
  const cm = Ed.cm(g), lp = g.lp; if (!cm || line == null) return;
  const d = cm.state.doc, i = Math.min(d.lines - 1, Math.max(0, Math.floor(line))), f = Math.min(1, Math.max(0, line - i));
  const go = () => {
    const b = cm.lineBlockAt(d.line(i + 1).from);
    const dy = cm.documentTop + b.top + f * b.height - lp.getBoundingClientRect().top;
    const max = Math.max(0, lp.scrollHeight - lp.clientHeight);
    lp.scrollTop = Math.min(max, Math.max(0, lp.scrollTop + dy));
  };
  go();
  // the height map holds ESTIMATES for lines never drawn: once CM6 has measured
  // the lines this scroll brought in, land on the real geometry
  cm.requestMeasure({ key: "osScrollLine", read: () => null, write: () => { if (Ed.cm(g) === cm) go(); } });
};
/* the editor scroller OWNS its plain vertical wheel. CM6 virtualises: lines it has
   not drawn carry ESTIMATED (or, after the .lp.src restyle, stale) heights, and when
   it measures the lines a scroll brings in it keeps the view still by correcting the
   scroller (`scrollTop += diff` in its measure loop, applied while a wheel event is
   < 100 ms old). WebKitGTK runs a wheel burst as ONE smooth-scroll animation and a
   scrollTop write CANCELS it, so scrolling UP into unmeasured lines lost most of
   every burst (scrollsync: 80 notches up from L91 in source mode landed on L45, then
   L20; Chromium/Obsidian composes the two). Here the burst is accumulated and walked
   in RELATIVE steps per frame, so CM6's corrections add to it instead of killing it.
   Left to the browser: Ctrl (fontwheel), Shift / horizontal intent, and any nested
   scroller (pdf frame, wide code/table) that can still move in the wheel's direction. */
Ed.wheelOwn = function (lp) {
  if (!lp || lp._osWheel) return;
  lp._osWheel = { pend: 0, raf: 0 };
  const W = lp._osWheel;
  const nestedTakes = (t, dy) => {
    for (let el = t && t.nodeType === 1 ? t : t && t.parentElement; el && el !== lp; el = el.parentElement) {
      if (el.scrollHeight <= el.clientHeight + 1) continue;
      const oy = getComputedStyle(el).overflowY;
      if (oy !== "auto" && oy !== "scroll") continue;
      if (dy < 0 ? el.scrollTop > 0 : el.scrollTop + el.clientHeight < el.scrollHeight - 1) return true;
    }
    return false;
  };
  const step = () => {
    W.raf = 0;
    const max = Math.max(0, lp.scrollHeight - lp.clientHeight), st = lp.scrollTop;
    // at an edge the rest of the burst has nowhere to go: drop it (no stored push)
    if ((W.pend < 0 && st <= 0) || (W.pend > 0 && st >= max)) { W.pend = 0; return; }
    let d = W.pend / 3;
    if (Math.abs(d) < 8) d = Math.abs(W.pend) < 8 ? W.pend : Math.sign(W.pend) * 8;
    W.pend -= d;
    lp.scrollTop = Math.min(max, Math.max(0, st + d));   // RELATIVE to wherever CM6 left it
    if (Math.abs(W.pend) >= 0.5) W.raf = requestAnimationFrame(step); else W.pend = 0;
  };
  lp.addEventListener("wheel", e => {
    if (e.ctrlKey || e.shiftKey || e.defaultPrevented) return;
    if (!lp._cm || Math.abs(e.deltaX) > Math.abs(e.deltaY) || !e.deltaY) return;
    if (nestedTakes(e.target, e.deltaY)) return;
    const px = e.deltaMode === 1 ? e.deltaY * (parseFloat(getComputedStyle(lp).lineHeight) || 24)
             : e.deltaMode === 2 ? e.deltaY * lp.clientHeight : e.deltaY;
    e.preventDefault();
    if (Math.sign(px) !== Math.sign(W.pend)) W.pend = 0;   // a reversal starts fresh
    W.pend += px;
    if (!W.raf) W.raf = requestAnimationFrame(step);
  }, { passive: false });
};
/* centre source position (l, c) in the scroller through CM6's own scrollIntoView
   effect: CM6 brings the line in, measures it and scrolls INSIDE its measure loop,
   after its redraw — a scrollTop written outside that loop was seen landing back at
   0 when the redraw of a bottom-clamped scroll ran (find reveal, [frv:..|t0]).
   No selection change, no focus. */
Ed.revealAt = function (g, l, c) {
  const cm = Ed.cm(g); if (!cm || l == null) return false;
  const d = cm.state.doc, i = Math.min(d.lines, Math.max(1, l + 1)), ln = d.line(i);
  cm.dispatch({ effects: CMv.EditorView.scrollIntoView(Math.min(ln.to, ln.from + Math.max(0, c || 0)), { y: "center" }) });
  return true;
};
/* put source line l at the scroller's top through CM6's own scrollIntoView effect
   (y=start): like revealAt, the scroll happens inside CM6's measure loop, so the
   redraw cannot put a bottom-clamped scrollTop back to 0 (tj [top:0|st:0]). */
Ed.revealTop = function (g, l) {
  const cm = Ed.cm(g); if (!cm || l == null) return false;
  const d = cm.state.doc, ln = d.line(Math.min(d.lines, Math.max(1, Math.floor(l) + 1)));
  cm.dispatch({ effects: CMv.EditorView.scrollIntoView(ln.from, { y: "start" }) });
  return true;
};
/* flash source line l: a LINE DECORATION, because CM6 owns every .cm-line's class
   attribute and wipes a class written on the element at its next redraw. */
Ed.flashLine = function (g, l) {
  const v = g && g.view; if (!v || !v.cm || !v._lpFx) return false;
  const me = g._fl = { l, fade: false }, bump = () => { if (Ed.cm(g) === v.cm) v.cm.dispatch({ effects: v._lpFx.of(null) }); };
  bump();
  setTimeout(() => { if (g._fl === me) { me.fade = true; bump(); } }, 400);
  setTimeout(() => { if (g._fl === me) { g._fl = null; bump(); } }, 2400);
  return true;
};
/* ---------- LIVE PREVIEW = CM6 decorations ----------
   The R17 per-line renderer (Ed.row -> Ed.block/Ed.inline) stays the ONE
   markdown->class-vocabulary tokenizer; its detached row is read back as
   ranges: every text-bearing element becomes a mark decoration (same tag,
   classes, data-*), every text-less leaf (checkbox, image, pdf frame) a
   widget, the row's own classes (lprow li ord tk) a line decoration. A line's
   textContent stays === its source line, so caret mapping and the census
   token grammar are unchanged. Markers (.mk) are hidden by the existing CSS
   unless their TOKEN is touched by the selection (.rv — R17.6 per token), or
   the pane is in source mode (.lp.src). Parsed lines are cached by text. */
Ed._lpc = new Map();
Ed.lpBust = function () { Ed._lpc.clear(); };
Ed.lpParse = function (g, text) {
  let P = Ed._lpc.get(text);
  if (P) return P;
  const row = Ed.row(g, text, 0), els = [], wid = [], at = new Map();
  const walk = (el, c) => {
    for (const n of el.childNodes) {
      if (n.nodeType === 3) { c += n.nodeValue.length; continue; }
      if (n.nodeType !== 1) continue;
      if (!n.textContent.length) { if (n.nodeName !== "BR") wid.push({ pos: c, k: wid.length, el: n }); continue; }
      const x = { s: c, e: c, el: n }; at.set(n, els.length); els.push(x);
      c = walk(n, c); x.e = c;
    }
    return c;
  };
  walk(row, 0);
  const toks = (row._tok || []).map(t => ({ s: t.s, e: t.e, kind: t.kind,
    mk: t.mks.map(m => at.get(m)).filter(i => i != null) }));
  const spec = n => {
    const attributes = {};
    for (const a of n.attributes) if (a.name !== "class" && a.name !== "contenteditable") attributes[a.name] = a.value;
    return { tagName: n.nodeName.toLowerCase(), class: n.className, attributes };
  };
  // a row holding a BLOCK leaf (image, missing-image banner, pdf frame) is tagged .blk: CM6 puts an
  // inline caret buffer (img.cm-widgetBuffer, 1em) before an inline widget, and before a display:block
  // leaf that buffer alone opens an empty line box — 24 px above every embed that R17 never drew
  // (osdrop: six image rows pushed the pdf frame to y=737, below a 700 px window). style.css hides it.
  const BLK = ".vimg, .imgmiss, .pdf-embed";
  const blk = wid.some(w => w.el.matches && (w.el.matches(BLK) || !!w.el.querySelector(BLK)));
  P = { cls: row.className + (blk ? " blk" : ""), toks, wid, els: els.map(x => {
    const sp = spec(x.el), d = CMv.Decoration.mark(sp);
    const isMk = x.el.classList.contains("mk");
    return { s: x.s, e: x.e, d, rv: isMk ? CMv.Decoration.mark(Object.assign({}, sp, { class: sp.class + " rv" })) : d };
  }) };
  if (Ed._lpc.size > 4000) Ed._lpc.clear();
  Ed._lpc.set(text, P);
  return P;
};
Ed.lpLeaves = function (row) {            // the text-less leaves of a rendered row, in order
  const out = [];
  const walk = el => { for (const n of el.childNodes) if (n.nodeType === 1) { if (!n.textContent.length) { if (n.nodeName !== "BR") out.push(n); } else walk(n); } };
  walk(row);
  return out;
};
Ed.lpExt = function (v, G) {
  const { Decoration, ViewPlugin, WidgetType, EditorView } = CMv;
  const bust = CMs.StateEffect.define();
  v._lpFx = bust;
  class Leaf extends WidgetType {
    constructor(t, k, el) { super(); this.t = t; this.k = k; this.el = el; }
    eq(o) { return o.t === this.t && o.k === this.k && o.el === this.el; }
    // the parsed leaf itself (cloned): re-rendering the row here could disagree with the parse
    // except a pdf frame: its loader (Ed.pdfFill) is closures + observers on the ORIGINAL
    // element, so a clone never loads a page (osdrop [pf:1/0|..p1/0-]) — render it fresh
    toDOM() {
      if (this.el && this.el.dataset && this.el.dataset.pdf != null) {
        const f = Ed.lpLeaves(Ed.row(G(), this.t, 0))[this.k];
        if (f) return f;
      }
      if (!this.el) return document.createElement("span");
      // cloneNode drops listeners, and ignoreEvent() keeps CM6's domEventHandlers off
      // widget events — so the task checkbox clone gets its own toggle (R8.6)
      const c = this.el.cloneNode(true);
      const cb = c.matches && c.matches("input.tbox") ? c : (c.querySelector ? c.querySelector("input.tbox") : null);
      if (cb) Ed.taskBox(cb);
      return c;
    }
    ignoreEvent() { return true; }
  }
  const lineDeco = new Map();
  const ld = c => { let d = lineDeco.get(c); if (!d) { d = Decoration.line({ class: c }); lineDeco.set(c, d); } return d; };
  const build = view => {
    const g = G(), st = view.state, doc = st.doc, sel = st.selection.main, out = [], labels = [];
    const cur = doc.lineAt(sel.head).number;
    // plugin overlay tokens (a Templater command) take no markdown decoration, as in Obsidian
    const opq = Ed.lpOpaque ? Ed.lpOpaque(view) : null;
    const hit = (rs, s, e) => { for (const r of rs) if (s < r[1] && e > r[0]) return true; return false; };
    let last = -1;
    for (const { from, to } of view.visibleRanges) {
      let pos = from;
      while (pos <= to) {
        const ln = doc.lineAt(pos);
        pos = ln.to + 1;
        if (ln.number <= last) continue;
        last = ln.number;
        const P = Ed.lpParse(g, ln.text), ors = opq && opq.get(ln.number);
        let rv = null;
        if (sel.from <= ln.to && sel.to >= ln.from) {
          const lo = Math.max(sel.from, ln.from) - ln.from, hi = Math.min(sel.to, ln.to) - ln.from;
          rv = new Set();
          for (const t of Ed.tokAt(P.toks, lo, hi)) {
            for (const i of t.mk) rv.add(i);
            labels.push((ln.number - 1) + "." + t.s + "-" + t.e + ":" + t.kind);
          }
        }
        const fl = g && g._fl && g._fl.l === ln.number - 1 ? (g._fl.fade ? " navflash fade" : " navflash") : "";
        const whole = ors && ln.length > 0 && ors.some(r => r[0] <= 0 && r[1] >= ln.length);
        out.push(ld((whole ? Ed.lpParse(g, "x").cls : P.cls) + (ln.number === cur ? " cur" : "") + fl).range(ln.from));
        for (let i = 0; i < P.els.length; i++) {
          const x = P.els[i];
          if (x.e > x.s && !(ors && hit(ors, x.s, x.e))) out.push((rv && rv.has(i) ? x.rv : x.d).range(ln.from + x.s, ln.from + x.e));
        }
        for (const w of P.wid) if (!(ors && ors.some(r => r[0] < w.pos && w.pos < r[1]))) out.push(Decoration.widget({ widget: new Leaf(ln.text, w.k, w.el), side: 1 }).range(ln.from + w.pos));
      }
    }
    if (g && g.view === v && g.lp) {
      const sig = labels.join(",");
      if (g.lp._rvsig !== sig) { g.lp._rvs = labels; g.lp._rvsig = sig; }
    }
    return Decoration.set(out, true);
  };
  const plugin = ViewPlugin.fromClass(class {
    constructor(view) { this.decorations = build(view); }
    update(u) {
      if (u.docChanged || u.selectionSet || u.viewportChanged || u.transactions.some(t => t.reconfigured || t.effects.some(e => e.is(bust))))
        this.decorations = build(u.view);
    }
  }, { decorations: p => p.decorations });
  const clicks = EditorView.domEventHandlers({
    mousedown: (e) => {
      const a = e.target && e.target.closest ? e.target.closest("a.tag, a.wl, a.lt, a.url, a.ext") : null;
      if (!a) return false;
      if (a.classList.contains("tag")) {
        e.preventDefault(); e.stopPropagation();
        if (typeof tagSearch === "function") tagSearch(a.dataset.tag);
        return true;
      }
      if (a.classList.contains("wl")) Ed.linkClick(e, a); else Ed.extClick(e, a);
      return e.defaultPrevented;
    },
  });
  return [plugin, clicks, Ed.cbField(v, G, bust)];
};
/* ---------- plugin CODE BLOCKS in Live Preview (plug-top5) ----------
   A fenced block whose language a plugin registered a code-block processor for
   (registerMarkdownCodeBlockProcessor: dataview, tasks, ...) shows in Live Preview
   as the processor's output, not as source: the whole fence (opening line ..
   closing line) is one block widget, div.block-language-L handed to the processor
   with (source, el, ctx) the way the reading view does it. A selection touching
   the fence reveals the source; source mode never renders. Ed.cbHook is set by
   ui/plugins/editor-api.js (null = no plugin loaded: nothing is scanned). Block
   decorations must come from a StateField (CM6 refuses them from a ViewPlugin). */
Ed.cbHook = null;
Ed.cbFx = null;
Ed.CB_FENCE = /^ {0,3}(`{3,}|~{3,})[ \t]*([^\s`{]*)(.*)$/;
// every closed fence of doc -> [{from, to, lang, src}] (cached on the doc: a caret move re-scans nothing)
Ed.cbFences = function (doc) {
  if (Ed._cbDoc === doc) return Ed._cbOut;
  const out = [];
  let open = null, li = -1;   // li: content column of the list item the following lines may belong to (-1: none)
  const LI = /^([ \t]*)([-*+]|\d{1,9}[.)])([ \t]+)\S/;
  for (let i = 1; i <= doc.lines; i++) {
    const ln = doc.line(i), t = ln.text;
    if (!open) {
      // deep-dataview FD14: a fence indented under a list item is that item's code block (stock Live Preview
      // renders it; one in a blockquote stays source, as stock). Indent stripped from the fence and its lines.
      const lm = LI.exec(t), ws = /^[ \t]*/.exec(t)[0].length;
      if (lm) { li = lm[1].length + lm[2].length + lm[3].length; continue; }
      if (t.trim() && li >= 0 && ws < li) li = -1;
      const ind = li >= 0 && ws >= li && ws < li + 4 ? li : 0, m = Ed.CB_FENCE.exec(ind ? t.slice(ind) : t);
      if (m && !(m[1][0] === "`" && m[3].includes("`"))) open = { ch: m[1][0], n: m[1].length, lang: m[2], from: ln.from, l0: i, ind };
      continue;
    }
    const m = Ed.CB_FENCE.exec(open.ind ? t.replace(new RegExp("^[ \\t]{0," + open.ind + "}"), "") : t);
    if (m && m[1][0] === open.ch && m[1].length >= open.n && !m[2] && !m[3].trim()) {
      let src = i - 1 > open.l0 ? doc.sliceString(doc.line(open.l0 + 1).from, doc.line(i - 1).to) : "";
      if (open.ind) src = src.split("\n").map(s => s.replace(new RegExp("^[ \\t]{0," + open.ind + "}"), "")).join("\n");
      if (open.lang) out.push({ from: open.from, to: ln.to, lang: open.lang, src });
      open = null;
    }
  }
  Ed._cbDoc = doc; Ed._cbOut = out;
  return out;
};
// a processor was (un)registered: every editor rebuilds its code-block widgets
Ed.cbRefresh = function () {
  if (!Ed.cbFx || !CMv || !CMv.EditorView) return;
  for (const el of document.querySelectorAll(".cm-editor")) {
    const cm = CMv.EditorView.findFromDOM(el);
    if (cm) try { cm.dispatch({ effects: Ed.cbFx.of(null) }); } catch (e) { console.error(e); }
  }
};
// leaf.rebuildView on a note in Live Preview: that editor's code-block widgets render again from
// scratch (stock rebuilds the whole view, so a dataview TABLE re-runs its query with refresh off)
Ed.cbRebuild = function (v) {
  if (!v || !v.cm || !Ed.cbFx) return;
  v._cbRb = (v._cbRb || 0) + 1;
  try { v.cm.dispatch({ effects: Ed.cbFx.of(null) }); } catch (e) { console.error(e); }
};
Ed.cbField = function (v, G, bust) {
  const { Decoration, WidgetType, EditorView } = CMv;
  if (!Ed.cbFx) Ed.cbFx = CMs.StateEffect.define();
  class CB extends WidgetType {
    constructor(lang, src, path, gen) { super(); this.lang = lang; this.src = src; this.path = path; this.gen = gen; }
    eq(o) { return o.lang === this.lang && o.src === this.src && o.path === this.path && o.gen === this.gen; }
    toDOM() {
      const w = Ed.el("div", "cm-preview-code-block cm-embed-block markdown-rendered cm-lang-" + this.lang);
      const el = Ed.el("div", "block-language-" + this.lang);
      w.appendChild(el);
      // the widget ignores editor events (ignoreEvent): its wiki links follow here (dataview file links)
      w.addEventListener("click", e => {
        const a = e.target && e.target.closest && e.target.closest("a.wiki, a.internal-link");
        if (a && w.contains(a) && typeof wikiClick === "function") wikiClick(e, a);
      });
      const h = Ed.cbHook;
      if (h) try { w._cbOff = h.render(this.lang, this.src, el, this.path); } catch (e) { console.error("[plugin] code block " + this.lang + ":", e); }
      return w;
    }
    destroy(dom) { if (dom && dom._cbOff) try { dom._cbOff(); } catch (e) { console.error(e); } }
    ignoreEvent() { return true; }
  }
  const build = st => {
    const h = Ed.cbHook;
    if (!h || !h.any()) return Decoration.none;
    const g = G();
    const tb = g && g.view === v && g.active >= 0 ? g.tabs[g.active] : null;   // goal mt-fix: the lp a reading view keeps laid out renders none
    if (!g || (v.lp && v.lp.classList.contains("src")) || (tb && tb.mode === "reading")) return Decoration.none;
    const blocks = Ed.cbFences(st.doc);
    if (!blocks.length) return Decoration.none;
    const n = typeof curOf === "function" && g.view === v ? curOf(g) : v.note, path = n ? n + ".md" : "", gen = h.gen() + "/" + (v._cbRb || 0), out = [];
    for (const b of blocks) {
      if (!h.has(b.lang)) continue;
      if (st.selection.ranges.some(r => r.from <= b.to && r.to >= b.from)) continue;   // caret in the fence: its source shows
      out.push(Decoration.replace({ widget: new CB(b.lang, b.src, path, gen), block: true }).range(b.from, b.to));
    }
    return Decoration.set(out, true);
  };
  const field = CMs.StateField.define({
    create: st => { try { return build(st); } catch (e) { console.error(e); return Decoration.none; } },
    update(d, tr) {
      if (tr.docChanged || tr.selection || tr.effects.some(e => e.is(bust) || e.is(Ed.cbFx)))
        try { return build(tr.state); } catch (e) { console.error(e); return Decoration.none; }
      return d;
    },
    provide: f => EditorView.decorations.from(f),
  });
  return field;
};
Ed.cbCensus = function () {   // [lpcb:N] rendered plugin code blocks in live-preview editors (only once one exists)
  const n = document.querySelectorAll(".cm-editor .cm-preview-code-block").length;
  return n ? " [lpcb:" + n + "]" : "";
};
/* ---------- MARKS painted by main.js (find bar "fd", search jump "sc") ----------
   A named DecorationSet per view, replaced whole by Ed.hl and mapped through
   every doc change in between. CM6 owns its contentDOM: wrapping its text nodes
   in spans (the R17 way) is a DOM mutation its observer reads back as an edit.
   Each range renders as <span class="cls" data-fh="i">, the census counts
   them in the live DOM as before (only lines CM6 has drawn carry one). */
Ed.HL_KEYS = ["fd", "sc"];
Ed.hlExt = function (v) {
  const fx = CMs.StateEffect.define();
  v._hlFx = fx;
  const field = CMs.StateField.define({
    create: () => ({}),
    update(m, tr) {
      let o = m;
      if (tr.docChanged) { o = {}; for (const k in m) o[k] = m[k].map(tr.changes); }
      for (const e of tr.effects) if (e.is(fx)) { o = Object.assign({}, o); o[e.value.k] = e.value.d; }
      return o;
    },
    provide: f => Ed.HL_KEYS.map(k => CMv.EditorView.decorations.from(f, m => m[k] || CMv.Decoration.none)),
  });
  v._hlField = field;
  return field;
};
// the data-fh ids of set k whose range lies OUTSIDE the lines CM6 has drawn (cm.viewport):
// those are in the set and paint when their line is drawn, but no DOM span exists yet
Ed.hlUndrawn = function (g, k) {
  const cm = Ed.cm(g), v = g && g.view, out = new Set();
  if (!cm || !v || !v._hlField) return out;
  const m = cm.state.field(v._hlField, false), d = m && m[k], vp = cm.viewport;
  if (!d) return out;
  d.between(0, cm.state.doc.length, (a, b, val) => {
    const fh = val.spec && val.spec.attributes && val.spec.attributes["data-fh"];
    if (fh != null && (b < vp.from || a > vp.to)) out.add(fh);
  });
  return out;
};
// replace mark set k with ranges [{from, to, cls, fh}] (absolute doc offsets, clamped)
Ed.hl = function (g, k, ranges) {
  const cm = Ed.cm(g), v = g && g.view; if (!cm || !v || !v._hlFx) return;
  const n = cm.state.doc.length, out = [];
  for (const r of ranges || []) {
    const a = Math.max(0, Math.min(n, r.from)), b = Math.max(0, Math.min(n, r.to));
    if (b <= a) continue;
    out.push(CMv.Decoration.mark({ class: r.cls, attributes: r.fh == null ? undefined : { "data-fh": String(r.fh) } }).range(a, b));
  }
  cm.dispatch({ effects: v._hlFx.of({ k, d: CMv.Decoration.set(out, true) }) });
};
// a task checkbox (an LP widget): mousedown keeps the caret off the line (the raw "[ ]" must not
// reveal under the pointer); the toggle runs on click, once the event has bubbled through the
// editor, so a plugin editor extension's own click handler (Obsidian Tasks: done date, recurrence)
// takes it with preventDefault as in stock; untaken, Ed.toggleTask flips the line itself
Ed.taskBox = function (cb) {
  cb.addEventListener("mousedown", e => { e.preventDefault(); e.stopPropagation(); });
  cb.addEventListener("click", e => setTimeout(() => { if (!e.defaultPrevented) Ed.toggleTask(e); }, 0));
};
// toggle "[ ]" <-> "[x]" on the checkbox's line, one undo step
Ed.toggleTask = function (e) {
  const box = e.target, pane = box && box.closest ? box.closest(".pane") : null, g = pane ? pane._g : null, cm = Ed.cm(g);
  if (!cm) return;
  let l;
  try { l = cm.state.doc.lineAt(cm.posAtDOM(box, 0)).number - 1; } catch (_) { return; }
  const L = Ed.lines(g);
  Ed.snap(g, "task");
  L[l] = L[l].replace(/^(\s*(?:[-*+]|\d+\.) )\[([^\]\n])\]/, (_, p, ch) => p + (ch === " " ? "[x]" : "[ ]"));
  Ed.commit(g);
};
