// SPDX-License-Identifier: GPL-3.0-or-later
/* R17 EDITOR CORE (v0.6) — the model/view engine behind Live Preview and
   source mode. Replaces the "caret row becomes a textarea" hybrid (R8.2),
   which re-rendered the whole note per keystroke and mangled list markers
   (USER FEEDBACK v0.5 #1 #9).

   MODEL   v.lines[] is the source of truth. The hidden textarea g.editor
           stays ONLY as the persistence bridge: it is written FROM the model
           (Ed.sync) and never read on the hot path.
   VIEW    one contenteditable (.lp) per pane, ONE DOM row per source line.
           A row's textContent is EXACTLY its source line (markers live in
           span.mk, hidden by CSS unless the row holds the caret or the pane
           is in source mode) — so the token map is just "walk the text nodes
           and accumulate lengths", and hidden markers can never shift the
           caret. A keystroke patches at most a handful of rows; the document
           is never innerHTML'd.
   INPUT   beforeinput -> mutate the model -> patch the dirty rows -> restore
           the caret at (line,col). Nothing is applied by the browser itself
           (except IME composition, which is reconciled on compositionend).
   RUST    the Rust renderer is used for reading mode and for block-level
           things on demand — never per keystroke.

   The inline renderer mirrors src-tauri/src/srcmode.rs marker for marker
   (same class vocabulary: mk h bq hr tbl task li code wl lt url tag b i bi
   s hl fence), so source mode is the same engine with the markers revealed. */
const Ed = {
  cur: null,            // {g, l} row currently holding the caret (census/geometry)
  _tk: null,            // R17.6 token map under construction (Ed.row owns it)
  // the REVEAL SET lives on the pane's .lp element (g.lp._rv = the marker spans
  // carrying .rv, g.lp._rvs = their "<l>.<s>-<e>:<kind>" labels for the census),
  // so two panes never clear each other's reveal.
  composing: false,
  UNDO_IDLE_MS: 2000,   // R17.8: idle gap that CLOSES an undo group

  /* ---------- model ---------- */
  lines(g) { const v = g.view; if (!v.lines) v.lines = [""]; return v.lines; },
  text(g) { return Ed.lines(g).join("\n"); },
  sync(g) { g.editor.value = Ed.text(g); },          // persistence bridge only
  setText(g, t) {                                     // load / external replace
    const v = g.view;
    v.lines = String(t == null ? "" : t).split("\n");
    v.undo = []; v.redo = [];
    Ed.sync(g);
  },

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
      const task = /^\[([ xX])\] /.exec(rest);
      if (task) {
        row.classList.add("tk");
        const cb = Ed.el("input", "tbox");
        cb.type = "checkbox"; cb.checked = task[1] !== " ";
        cb.contentEditable = "false";
        cb.addEventListener("mousedown", e => Ed.toggleTask(e, row));
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
          add(Ed.imgEl(im[2], im[1], rem), false);
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
    return notesCache.some(x => x === n || x.endsWith("/" + n) ||
                                x === n + ".md" || x.endsWith("/" + n + ".md"));
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

  /* ---------- caret + selection ----------
     THE POSITIONAL CONTRACT, and what R34 had to do to keep it: model line `l`
     IS `g.lp.children[l]`, index for index, with nothing else in that list.
     R32 kept the inline title out of it by drawing it as a ::before; R34 makes
     that title a rename surface (ui/main.js openTitleEdit) WITHOUT joining the
     list — the caret box is appended to `.content`, a sibling of the scroller,
     and the ::before stays exactly where it is (visibility:hidden) so the box
     it reserves, and therefore row 0's y, never moves. Anything editable put
     inside .lp — prepended, appended or absolutely positioned — is a child,
     and rowAt() then returns the wrong node for every line after it.
     That is not a comment you have to trust: the census publishes
     [te:<text>/<children>] while the title is open and [ery:] gives one entry
     per child, phase `title` section F asserts both are unchanged, and
     docs/negctl-title-rename/README.md (control T1) is the committed run where
     making the box a child reads `[te:…/5]` on a 4-line note and goes red. */
  rowAt(g, l) { const r = g.lp.children[l] || null; return r && r._lzy ? Ed.wake(g, l) : r; },
  /* LAZY ROWS (audit #10/#12). A 75k-line note used to build, lay out and census
     75k rows at open (~3.4 s on the box). Past LAZY_AT lines a row that is not
     near the top or the caret is a placeholder: an empty div.lprow.lzy one line
     tall (style.css). It is still the child at its index, so the positional
     contract above holds and rowAt() is the one door: it builds the real row
     the first time anything addresses that line. wakeView() builds what a
     scroll brings into view. The model (Ed.lines) is the truth either way —
     copy, save and select-all read the model, never the placeholder. */
  LAZY_AT: 2000, LAZY_HEAD: 300, LAZY_NEAR: 100,
  lzy() { const r = document.createElement("div"); r.className = "lprow lzy"; r._lzy = true; return r; },
  wake(g, l) {
    const r = g.lp.children[l];
    if (!r || !r._lzy) return r || null;
    const f = Ed.row(g, Ed.lines(g)[l], l);
    r.replaceWith(f);
    return f;
  },
  wakeView(g) {                                  // build the placeholders in (and near) the viewport
    const lp = g && g.lp, rows = lp && lp.children;
    if (!rows || !rows.length || !lp._lzy) return;
    for (let pass = 0; pass < 3; pass++) {
      const base = rows[0].offsetTop, top = lp.scrollTop, bot = top + 2 * (lp.clientHeight || 800);
      let lo = 0, hi = rows.length - 1;
      while (lo < hi) { const m = (lo + hi + 1) >> 1; if (rows[m].offsetTop - base <= top) lo = m; else hi = m - 1; }
      const todo = [];
      for (let i = Math.max(0, lo - 40); i < rows.length && rows[i].offsetTop - base <= bot; i++)
        if (rows[i]._lzy) todo.push(i);
      if (!todo.length) return;
      for (const i of todo) Ed.wake(g, i);       // one batch, then re-measure: built rows may wrap taller
      if (typeof scHits === "function" && scHits(g).length) scMarks(g);   // the search flash covers built rows too
    }
  },
  indexOf(row) { return row && row.parentNode ? Array.prototype.indexOf.call(row.parentNode.children, row) : -1; },
  rowOf(node) {
    if (!node) return null;
    const e = node.nodeType === 3 ? node.parentElement : node;
    return e && e.closest ? e.closest(".lprow") : null;
  },
  pos(g, node, off) {                            // DOM position -> {l, c}
    let row = Ed.rowOf(node);
    if (!row && node === g.lp) {
      row = g.lp.children[Math.min(off, g.lp.children.length - 1)];
      return row ? { l: Ed.indexOf(row), c: 0 } : null;
    }
    if (!row || !g.lp.contains(row)) return null;
    return { l: Ed.indexOf(row), c: Ed.colOf(row, node, off) };
  },
  sel(g) {                                       // model selection, a <= b
    const s = window.getSelection();
    if (!s || !s.rangeCount) return null;
    const r = s.getRangeAt(0);
    if (!g.lp.contains(r.startContainer)) return null;
    const a = Ed.pos(g, r.startContainer, r.startOffset);
    if (!a) return null;
    const b = r.collapsed ? a : (Ed.pos(g, r.endContainer, r.endOffset) || a);
    return { a, b, empty: a.l === b.l && a.c === b.c };
  },
  caret(g) { const s = Ed.sel(g); return s ? s.b : null; },
  caretLC(g) { const c = Ed.caret(g) || { l: 0, c: 0 }; return [c.l, c.c]; },
  focusPos(g) {                                  // the MOVING end of the selection
    const s = window.getSelection();
    if (!s || !s.focusNode || !g.lp.contains(s.focusNode)) return null;
    return Ed.pos(g, s.focusNode, s.focusOffset);
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
  extendTo(g, l, c) {                            // Shift+Home/End: keep the anchor, move the focus
    const row = Ed.rowAt(g, l);
    if (!row) return;
    const [node, off] = Ed.posOf(row, c, true);
    const s = window.getSelection();
    if (!s.rangeCount) return Ed.place(g, l, c);
    try { s.extend(node, off); } catch (_) { return Ed.place(g, l, c); }
    Ed.mark(g, l);
  },
  place(g, l, c, focus) {                        // caret to (line,col)
    const L = Ed.lines(g);
    l = Math.max(0, Math.min(l, L.length - 1));
    c = Math.max(0, Math.min(c, L[l].length));
    const row = Ed.rowAt(g, l);
    if (!row) return;
    Ed.reveal(g, { l, c }, { l, c });             // markers first: offsets stay valid
    const [node, off] = Ed.posOf(row, c, true);
    const s = window.getSelection();
    const r = document.createRange();
    try { r.setStart(node, off); } catch (_) { r.selectNodeContents(row); r.collapse(true); }
    r.collapse(true);
    s.removeAllRanges(); s.addRange(r);
    if (focus !== false && document.activeElement !== g.lp) g.lp.focus({ preventScroll: true });
    Ed.mark(g, l);
  },
  // Ctrl+A over the MODEL, not over the painted text: the browser's own
  // select-all starts at the first VISIBLE character, so on a note that opens
  // with "# Title" it silently drops the hidden "# " and a copy would come
  // back without the heading marker. The range is set on the raw text nodes,
  // hidden or not.
  selectAll(g) {
    const L = Ed.lines(g), first = Ed.rowAt(g, 0), last = Ed.rowAt(g, L.length - 1);
    if (!first || !last) return;
    const [n0, o0] = Ed.posOf(first, 0, true);
    const [n1, o1] = Ed.posOf(last, L[L.length - 1].length, true);
    const r = document.createRange();
    try { r.setStart(n0, o0); r.setEnd(n1, o1); } catch (_) { return; }
    const s = window.getSelection();
    s.removeAllRanges(); s.addRange(r);
    Ed.census();
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
  reveal(g, a, b) {
    const lp = g.lp;
    const prev = lp.querySelector(".lprow.cur");
    const row = a ? Ed.rowAt(g, a.l) : null;
    if (prev !== row) { if (prev) prev.classList.remove("cur"); if (row) row.classList.add("cur"); }
    const mks = [], labels = [];
    if (a) {
      if (!b) b = a;
      if (b.l < a.l || (b.l === a.l && b.c < a.c)) { const t = a; a = b; b = t; }
      const L = Ed.lines(g);
      for (let l = a.l; l <= b.l && l < lp.children.length; l++) {
        const toks = lp.children[l] && lp.children[l]._tok;
        if (!toks || !toks.length) continue;
        const len = (L[l] || "").length;
        for (const t of Ed.tokAt(toks, l === a.l ? a.c : 0, l === b.l ? b.c : len)) {
          for (const m of t.mks) mks.push(m);
          labels.push(l + "." + t.s + "-" + t.e + ":" + t.kind);
        }
      }
    }
    /* Idempotence is load-bearing, not an optimisation: making a marker visible
       relayouts the row, WebKit can re-fire selectionchange off that, and a
       reveal that always remove-then-adds the class would then chase its own
       tail. Same set => not one DOM write. (Ed.render clears the signature: new
       rows carry no .rv, so the set is stale even when its labels match.) */
    const sig = labels.join(",");
    if (lp._rvsig === sig) return;
    for (const el of (lp._rv || [])) el.classList.remove("rv");
    for (const m of mks) m.classList.add("rv");
    lp._rv = mks; lp._rvs = labels; lp._rvsig = sig;
  },
  /* R17.6/R17.7 census: the REVEAL SET — which tokens are showing their raw
     markers right now, as "<line>.<start>-<end>:<kind>", "-" for none, "src" in
     source mode. Without this the smoke could only see the caret ROW, which is
     exactly how the reveal scope drifted from per-token to per-row unnoticed.

     It reads the PAINTED state (computed `display` of one marker per token),
     NOT `lp._rvs`: the scope lives half in JS and half in ONE CSS rule, and a
     revert of that rule alone would leave the bookkeeping perfectly correct
     while the whole row reveals on screen. Probing the rendered style is what
     makes the negative control (docs/negctl-lp-reveal/) bite.

     The probe marker is the first NON-`.lim` one: `.mk.lim` is the list bullet
     box, which is `display: inline-block` with transparent ink even while
     FOLDED (R15.10), so it can never answer "is this marker's text showing?".
     Its sibling `.lisp` (the marker's trailing space) can.

     Rows scanned = the ones the selection touches plus the caret row. Census
     runs deferred and coalesced (30ms), off the keystroke and off the
     caret-move measurement — but a getComputedStyle per token is still a
     layout read, so it stays bounded to the rows R17.6/R17.7 govern. */
  rvRows(g) {
    const out = [], add = l => { if (l >= 0 && l < g.lp.children.length && out.indexOf(l) < 0) out.push(l); };
    const s = Ed.sel(g);
    if (s) for (let l = s.a.l; l <= s.b.l && l - s.a.l < 64; l++) add(l);
    const cur = g.lp.querySelector(".lprow.cur");
    if (cur) add(Ed.indexOf(cur));
    return out.sort((x, y) => x - y);
  },
  rvTok(g) {
    if (!g.lp) return "";
    if (g.lp.classList.contains("src")) return " [rv:src]";
    const out = [];
    for (const l of Ed.rvRows(g)) {
      const toks = g.lp.children[l] && g.lp.children[l]._tok;
      if (!toks) continue;
      for (const t of toks) {
        const probe = t.mks.filter(m => !m.classList.contains("lim"))[0] || t.mks[0];
        if (!probe) continue;
        if (getComputedStyle(probe).display !== "none") out.push(l + "." + t.s + "-" + t.e + ":" + t.kind);
      }
    }
    return " [rv:" + (out.length ? out.join(",") : "-") + "]";
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

  /* ---------- view: dirty-row patching ---------- */
  render(g, caret, col, full) {
    const v = g.view, lp = g.lp, L = Ed.lines(g), old = v.rowSrc;
    g.lpLines = L.length;
    lp._rv = []; lp._rvsig = null;      // patched rows carry no .rv: the reveal set is stale by construction
    const t0 = performance.now();
    let touched = 0;
    if (full || !old || lp.children.length !== old.length) {
      const frag = document.createDocumentFragment();
      // audit #10/#12: past LAZY_AT lines only the head and the caret's
      // neighbourhood are built; the rest are one-line placeholders (Ed.lzy)
      // that rowAt() and a scroll build on demand
      const lazy = L.length > Ed.LAZY_AT, near = caret != null && caret >= 0 ? caret : 0;
      for (let i = 0; i < L.length; i++)
        frag.appendChild(!lazy || i < Ed.LAZY_HEAD || Math.abs(i - near) < Ed.LAZY_NEAR ? Ed.row(g, L[i], i) : Ed.lzy());
      lp.textContent = "";
      lp.appendChild(frag);
      lp._lzy = lazy;
      if (lazy) requestAnimationFrame(() => Ed.wakeView(g));
      touched = L.length;
    } else {
      let p = 0, s = 0;
      const on = old.length, nn = L.length;
      while (p < on && p < nn && old[p] === L[p]) p++;
      while (s < on - p && s < nn - p && old[on - 1 - s] === L[nn - 1 - s]) s++;
      for (let k = on - s - p; k > 0; k--) lp.children[p].remove();
      if (nn - s > p) {
        const frag = document.createDocumentFragment();
        for (let i = p; i < nn - s; i++) frag.appendChild(Ed.row(g, L[i], i));
        lp.insertBefore(frag, lp.children[p] || null);
        touched = nn - s - p;
      }
    }
    v.rowSrc = L.slice();
    v.note = typeof curOf === "function" ? curOf(g) : null;
    if (caret != null && caret >= 0) Ed.place(g, caret, col || 0);
    else Ed.reveal(g, null);
    if (typeof perf !== "undefined" && perf.mark)
      // R18: this is the JS row patch, NOT the old per-keystroke Rust render.
      // "lp_render" stays reserved for the IPC span (docs/perf.md '## EDITOR':
      // its count must be 0 while typing) — reusing the name here would make
      // that check unfalsifiable.
      perf.mark("ed_patch", t0, { lines: L.length, patched: touched, full: !!full || !old });
    return Promise.resolve();
  },
  // re-render exactly one row (IME reconcile / self-heal)
  patchRow(g, l) {
    const row = Ed.rowAt(g, l);
    if (!row) return;
    const fresh = Ed.row(g, Ed.lines(g)[l], l);
    if (row.classList.contains("cur")) fresh.classList.add("cur");
    row.replaceWith(fresh);
    g.view.rowSrc[l] = Ed.lines(g)[l];
  },

  /* ---------- edits ---------- */
  snap(g, kind) {                                // undo entry, coalesced
    const v = g.view, now = Date.now();
    if (!v.undo) { v.undo = []; v.redo = []; }
    v.redo = [];
    const last = v.undo[v.undo.length - 1];
    // R17.8 M79/M80: an uninterrupted typing burst is ONE step (spaces do not
    // split it); the group is closed by an IDLE TIMER (gap since the last
    // keystroke, refreshed per key), never by a char count. Enter/del/paste/
    // indent each stay their own step.
    if (last && last.kind === kind && kind === "type" && now - last.t < Ed.UNDO_IDLE_MS) { last.t = now; return; }
    v.undo.push({ lines: Ed.lines(g).slice(), caret: Ed.caret(g), kind, t: now });
    if (v.undo.length > 300) v.undo.shift();
  },
  after(g, l, c) {                               // model changed -> view + bridge
    Ed.sync(g);
    Ed.render(g, l, c);
    if (typeof scheduleSave === "function") scheduleSave(g);
    if (typeof showAc === "function") showAc(g);
  },
  replace(g, s, text, kind) {                    // splice [a,b) with `text`
    const L = Ed.lines(g);
    Ed.snap(g, kind || "type");
    const a = s.a, b = s.b;
    const head = L[a.l].slice(0, a.c), tail = (L[b.l] || "").slice(b.c);
    const ins = String(text == null ? "" : text).replace(/\r\n?/g, "\n").split("\n");
    /* R25.13d: the search flash is "a range that FOLLOWS subsequent edits", so
       its absolute offsets are mapped through the splice HERE — the one edit
       choke point — before the model changes under them. Called by name, the
       same way this file calls updateTitle: editor.js owns the text, main.js
       owns the decoration, and neither imports the other.
       GUARDED on there being a flash at all: Ed.offOf walks the line array, and
       this runs on EVERY keystroke (R18). With no flash — the whole measured
       typing path — it costs one typeof and one property read. */
    if (typeof scLive === "function" && scLive(g))
      scShift(g, Ed.offOf(g, a.l, a.c), Ed.offOf(g, b.l, b.c), ins.join("\n").length);
    const mid = ins.length === 1 ? [head + ins[0] + tail]
      : [head + ins[0]].concat(ins.slice(1, -1), [ins[ins.length - 1] + tail]);
    L.splice(a.l, b.l - a.l + 1, ...mid);
    const cl = a.l + ins.length - 1;
    const cc = ins.length === 1 ? a.c + ins[0].length : ins[ins.length - 1].length;
    Ed.after(g, cl, cc);
  },
  // (line, col) -> absolute UTF-16 offset into the model, counting the newline
  // each line ends with. The inverse of main.js scLC; the unit is the one the
  // backend's search payload uses (src-tauri/src/main.rs lines_with_offsets).
  offOf(g, l, c) {
    const L = Ed.lines(g);
    let o = 0;
    for (let i = 0; i < l && i < L.length; i++) o += L[i].length + 1;
    return o + c;
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
    const L = Ed.lines(g), nl = Ed.flipTask(L[l]);
    if (nl == null) return false;
    const v = g.view, last = v.undo && v.undo[v.undo.length - 1];
    if (v.rvSess && last && last.kind === "rvtask" && last.rvs === v.rvSess) v.redo = [];
    else {
      Ed.snap(g, "rvtask");
      v.rvSess = v.rvSess || {};
      v.undo[v.undo.length - 1].rvs = v.rvSess;
    }
    L[l] = nl;
    Ed.sync(g);
    Ed.render(g, null);                         // keep the hidden lp rows in step with the model
    if (typeof scheduleSave === "function") scheduleSave(g);
    return true;
  },
  rvEnd(g) { if (g && g.view) g.view.rvSess = null; },
  undo(g) {
    const v = g.view;
    if (!v.undo || !v.undo.length) return;
    const e = v.undo.pop();
    v.redo.push({ lines: Ed.lines(g).slice(), caret: Ed.caret(g) });
    v.lines = e.lines.slice();
    const c = e.caret || { l: 0, c: 0 };
    Ed.sync(g); Ed.render(g, c.l, c.c);
    if (typeof scheduleSave === "function") scheduleSave(g);
  },
  redo(g) {
    const v = g.view;
    if (!v.redo || !v.redo.length) return;
    const e = v.redo.pop();
    v.undo.push({ lines: Ed.lines(g).slice(), caret: Ed.caret(g), kind: "redo", t: Date.now() });
    v.lines = e.lines.slice();
    const c = e.caret || { l: 0, c: 0 };
    Ed.sync(g); Ed.render(g, c.l, c.c);
    if (typeof scheduleSave === "function") scheduleSave(g);
  },
  // clipboard/selection helper: the SOURCE text of a selection (hidden
  // markers included — Selection.toString() would drop them)
  slice(g, s) {
    const L = Ed.lines(g);
    if (s.a.l === s.b.l) return (L[s.a.l] || "").slice(s.a.c, s.b.c);
    const out = [L[s.a.l].slice(s.a.c)];
    for (let l = s.a.l + 1; l < s.b.l; l++) out.push(L[l]);
    out.push((L[s.b.l] || "").slice(0, s.b.c));
    return out.join("\n");
  },
  // [[ autocomplete: replace [start, caret) with [[name]] through the model
  acInsert(g, start, name) {
    const s = Ed.sel(g);
    if (!s) return;
    const L = Ed.lines(g), l = s.b.l, line = L[l];
    const a = line.lastIndexOf("[[", s.b.c);
    if (a < 0) return;
    Ed.replace(g, { a: { l, c: a }, b: { l, c: s.b.c }, empty: false }, "[[" + name + "]]", "ac");
  },
  toggleTask(e, row) {
    e.preventDefault(); e.stopPropagation();
    const g = row.closest(".pane") ? row.closest(".pane")._g : null;
    if (!g) return;
    const l = Ed.indexOf(row), L = Ed.lines(g);
    Ed.snap(g, "task");
    L[l] = L[l].replace(/^(\s*(?:[-*+]|\d+\.) )\[( |[xX])\]/, (_, p, ch) => p + (ch === " " ? "[x]" : "[ ]"));
    Ed.sync(g);
    Ed.patchRow(g, l);
    if (typeof scheduleSave === "function") scheduleSave(g);
  },
  linkClick(e, a) {
    if (typeof wikiClick === "function") wikiClick(e, a);
  },
  extClick(e, a) {
    if (typeof extClick === "function") extClick(e, a);
  },

  /* ---------- wiring ---------- */
  mount(v) {
    const lp = v.lp;
    lp.contentEditable = "true";
    lp.spellcheck = false;
    lp.setAttribute("autocorrect", "off");
    const G = () => (lp.closest(".pane") ? lp.closest(".pane")._g : v.g);
    lp.addEventListener("beforeinput", e => Ed.onInput(G(), e));
    lp.addEventListener("keydown", e => Ed.onKey(G(), e));
    lp.addEventListener("paste", e => {
      const g = G(), s = Ed.sel(g);
      e.preventDefault();
      if (!s) return;
      const txt = (e.clipboardData && e.clipboardData.getData("text/plain")) || "";
      if (txt) Ed.replace(g, s, txt, "paste");
    });
    lp.addEventListener("copy", e => Ed.onCopy(G(), e, false));
    lp.addEventListener("cut", e => Ed.onCopy(G(), e, true));
    lp.addEventListener("compositionstart", () => { Ed.composing = true; });
    lp.addEventListener("compositionend", () => {
      Ed.composing = false;
      Ed.heal(G());
    });
    lp.addEventListener("input", e => {          // self-heal: nothing may diverge
      if (Ed.composing || e.inputType === "insertCompositionText") return;
      Ed.heal(G());
    });
    lp.addEventListener("blur", () => { if (typeof hideAc === "function") setTimeout(hideAc, 100); });
    lp.addEventListener("scroll", () => {       // lazy rows: build what scrolls into view, once per frame
      if (!lp._lzy || lp._lzyRaf) return;
      lp._lzyRaf = requestAnimationFrame(() => { lp._lzyRaf = 0; Ed.wakeView(G()); });
    }, { passive: true });
  },
  // a row the browser edited behind our back -> take its text as the truth
  heal(g) {
    if (!g || !g.view || !g.view.rowSrc) return;
    const L = Ed.lines(g), rows = g.lp.children;
    if (rows.length !== L.length) { Ed.render(g, -1, 0, true); return; }
    const s = Ed.sel(g);
    const l = s ? s.b.l : -1;
    if (l < 0 || l >= L.length) return;
    const txt = rows[l].textContent;
    if (txt === L[l]) return;
    L[l] = txt; g.view.rowSrc[l] = txt;
    Ed.sync(g);
    if (typeof scheduleSave === "function") scheduleSave(g);
  },
  onCopy(g, e, cut) {
    const s = Ed.sel(g);
    if (!s || s.empty) return;
    e.preventDefault();
    e.clipboardData.setData("text/plain", Ed.slice(g, s));
    if (cut) Ed.replace(g, s, "", "del");
  },
  onKey(g, e) {
    if (!g) return;
    if (typeof acKeydown === "function") {
      acKeydown(g, e);
      if (e.defaultPrevented) return;
    }
    const s = Ed.sel(g);
    if (!s) return;
    // X11 sends shift+Tab as ISO_Left_Tab, so e.key is NOT "Tab" for the
    // outdent half of R17.2 — the physical key (e.code) is what identifies it
    // (chordOf() normalises the same way for the global keymap).
    if (e.key === "Tab" || e.code === "Tab") { e.preventDefault(); return Ed.indent(g, s, e.shiftKey); }
    // R17.4 M45 / R17.10 M92: in 1.13.7 Ctrl+Enter is "follow link under
    // cursor", NOT the task toggle — it must leave the bytes alone. Without
    // this guard the webview turns it into an insertParagraph.
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); return; }
    // Shift+key arrives UPPERCASE (e.key is the produced character): Ctrl+Shift+Z
    // is "Z", so a lowercase-only compare silently loses redo (R17.8 M83).
    const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    if (k === "a" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); return Ed.selectAll(g); }
    if (k === "z" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); return e.shiftKey ? Ed.redo(g) : Ed.undo(g); }
    if (k === "y" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); return Ed.redo(g); }
    // Ctrl+L (M46-M51) is NOT bound here: it is the palette command
    // editor:toggle-checklist-status, dispatched on document keydown. Binding
    // it in both places would toggle twice and cancel itself out.
    // PERF: Home/End/arrows are pure CARET MOVES — the interaction the per-token
    // reveal runs on. Time them to first paint (census [cm:], R18 100ms ceiling).
    if (e.key === "Home" || e.key === "End" || e.key === "ArrowLeft" || e.key === "ArrowRight") Ed.cmStart();
    if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      const f = Ed.focusPos(g) || s.b;                      // shift-extend moves the FOCUS, not s.b
      const h = Ed.homeCol(g, f.l);
      const c = e.key === "End" ? Ed.lines(g)[f.l].length : (f.c === h ? 0 : h);
      if (e.shiftKey) Ed.extendTo(g, f.l, c); else Ed.place(g, f.l, c);
      return Ed.cmEnd(g);
    }
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      /* R17.5 M69-M71: horizontal motion is a MODEL move — one RAW column at a
         time (a hidden marker is a caret stop, not a jump), and at a line edge
         it crosses to the RAW end/start of the neighbour row. Native motion
         cannot do the crossing: the row it lands on is still folded, so the
         browser stops at the last VISIBLE character and the caret lands before
         the hidden markers instead of after them. */
      if (e.ctrlKey || e.metaKey || e.altKey) { Ed.cmT0 = -1; return; }   // word/doc moves stay native
      const back = e.key === "ArrowLeft";
      if (!e.shiftKey && !s.empty) {                          // a plain arrow collapses to the edge
        e.preventDefault();
        const p = back ? s.a : s.b;
        Ed.place(g, p.l, p.c);
        return Ed.cmEnd(g);
      }
      const f = (e.shiftKey && Ed.focusPos(g)) || (back ? s.a : s.b);
      const n = Ed.step(g, f.l, f.c, back);
      if (!n) { Ed.cmT0 = -1; return; }                       // document edge: nothing to do
      e.preventDefault();
      if (e.shiftKey) Ed.extendTo(g, n.l, n.c); else Ed.place(g, n.l, n.c);
      return Ed.cmEnd(g);
    }
    Ed.cmT0 = -1;                                            // not a caret move after all
  },
  onInput(g, e) {
    if (!g) return;
    if (Ed.composing || e.inputType === "insertCompositionText") return;   // IME: native, reconciled on end
    const s = Ed.sel(g);
    if (!s) return;
    const sp = typeof otel !== "undefined" && otel.begin
      ? otel.begin("key_to_paint", { key: e.inputType, note_lines: g.lpLines || 0 })
      : null;
    e.preventDefault();
    switch (e.inputType) {
      case "insertText": Ed.replace(g, s, e.data || "", "type"); break;
      case "insertParagraph": Ed.enter(g, s); break;
      case "insertLineBreak": Ed.softBreak(g, s); break;
      case "deleteContentBackward": Ed.del(g, s, false, false); break;
      case "deleteContentForward": Ed.del(g, s, true, false); break;
      case "deleteWordBackward": Ed.del(g, s, false, true); break;
      case "deleteWordForward": Ed.del(g, s, true, true); break;
      case "deleteByCut": if (!s.empty) Ed.replace(g, s, "", "del"); break;
      case "insertFromPaste": {
        const txt = (e.dataTransfer && e.dataTransfer.getData("text/plain")) || "";
        if (txt) Ed.replace(g, s, txt, "paste");
        break;
      }
      case "historyUndo": Ed.undo(g); break;
      case "historyRedo": Ed.redo(g); break;
      default:
        if (e.inputType.startsWith("delete")) Ed.del(g, s, /Forward/.test(e.inputType), false);
        else if (e.data) Ed.replace(g, s, e.data, "type");
    }
    if (sp && otel.paint) otel.paint(sp);
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
Ed.setAll = function (g, text) {          // whole-document replace (undoable)
  Ed.snap(g, "cmd");
  g.view.lines = String(text).split("\n");
  Ed.sync(g);
  Ed.render(g, -1, 0);
};
Ed.offsetOf = function (g, p) {           // (line,col) -> document char offset
  const L = Ed.lines(g);
  let o = 0;
  for (let i = 0; i < p.l && i < L.length; i++) o += L[i].length + 1;
  return o + p.c;
};
Ed.placeOffset = function (g, o) {        // document char offset -> caret
  const L = Ed.lines(g);
  let l = 0;
  while (l < L.length - 1 && o > L[l].length) { o -= L[l].length + 1; l++; }
  Ed.place(g, l, o);
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
/* Headless probe (R17 smoke): the geometry the `edit` phase clicks with.
   [edx:<left>] + [ery:<centre y per row>] let a test address a SOURCE LINE
   instead of guessing a pixel — row heights differ per line (headings, list
   items), so a fixed pitch goes stale on any type change. Capped at 40 rows:
   the 500-line perf note never pays the layout read, so this stays off the
   measured keystroke path. */
Ed.geom = function (g) {
  const rows = g.lp ? g.lp.children : null;
  if (!rows || !rows.length || rows.length > 40) return "";
  const b = rows[0].getBoundingClientRect();   // the ROW box (= the text column), not the padded container
  let ys = "", xs = "", cb = "";
  const r = document.createRange();
  for (let i = 0; i < rows.length; i++) {
    const q = rows[i].getBoundingClientRect();
    ys += (i ? "," : "") + Math.round(q.top + q.height / 2);
    /* [ecb:<row>@<x>] = centre of the rendered task checkbox on that row. It
       is an <input>, so it is invisible to the range rects above, and its x
       moves with the hidden "- [ ] " run — the lp phase used to click a
       constant 315 for it. */
    const box = rows[i].querySelector("input.tbox");
    if (box) {
      const bb = box.getBoundingClientRect();
      if (bb.width) cb += (cb ? "," : "") + i + "@" + Math.round(bb.left + bb.width / 2);
    }
    /* [erx:] = where the row's first PAINTED character starts, which is NOT
       the row box: a folded list row hides its "- " and paints a ::before
       bullet instead, so the row box left is inside that bullet and a click
       there lands on the pseudo-element, not on column 2. Range client rects
       skip display:none text and generated content, so this is the only
       honest "visual start of this source line". */
    const r0 = Ed.firstRect(rows[i], r);
    xs += (i ? "," : "") + Math.round(r0 ? r0.left : q.left);
  }
  return " [edx:" + Math.round(b.left) + "] [erx:" + xs + "] [ery:" + ys + "]"
    + (cb ? " [ecb:" + cb + "]" : "");
};
Ed.selTok = function (g) {                // [sel:<l>.<c>-<l>.<c>] model range
  const s = Ed.sel(g);
  return s ? " [sel:" + s.a.l + "." + s.a.c + "-" + s.b.l + "." + s.b.c + "]" : "";
};
Ed.caretXY = function (g) {               // caret rect relative to the pane box
  const s = window.getSelection();
  const pr = g.pane.getBoundingClientRect();
  if (!s || !s.rangeCount || !g.lp.contains(s.anchorNode)) return [16, 16];
  let r = s.getRangeAt(0).getBoundingClientRect();
  if (!r.width && !r.height) {            // collapsed range in an empty node
    const row = Ed.rowOf(s.anchorNode) || g.lp;
    r = row.getBoundingClientRect();
  }
  return [r.left - pr.left, r.bottom - pr.top];
};

/* R17.6/R17.7: reveal follows the SELECTION, per token. Selection moves the
   browser handles natively (arrows, clicks, drag-select) land here — no
   re-render, just the `.rv` class flip + the [mode:lp:<line>] [rv:] census.

   The old "same line? nothing to do" short-circuit is GONE on purpose: with a
   per-token reveal a move WITHIN a line changes the reveal set (col 10 -> col
   15 opens `*ital*`), so bailing on the line number is exactly the bug this
   replaces. Ed.mark still guards the title churn on the line number. */
document.addEventListener("selectionchange", () => {
  const s = window.getSelection();
  if (!s || !s.anchorNode) return;
  const row = Ed.rowOf(s.anchorNode);
  const lp = row ? row.parentElement : null;
  if (!lp || !lp.classList.contains("lp")) return;
  const pane = lp.closest(".pane"), g = pane ? pane._g : null;
  if (!g || g.lp !== lp) return;
  const l = Ed.indexOf(row);
  const ms = Ed.sel(g);
  Ed.reveal(g, ms && ms.a, ms && ms.b);
  /* THE CACHE MAY NOT OUTLIVE THE STATE IT CACHES (2026-09-19, tabclose).
     Ed.cur is a module-level memo of "the caret is already on this row of this
     pane", and the ONLY writer that keeps it in step with g.lpActive is
     Ed.mark. Anything that nulls g.lpActive behind mark's back — dropTab()
     (main.js: the watcher removing the active tab) and the per-tab view
     restore (main.js: g.lpActive = v.lpActive on a tab switch) — leaves Ed.cur
     pointing at a row of a note that is gone, and this short-circuit then
     swallows the very click that should have re-armed the raw row: the caret
     lands (the selection moves, [sel:0.16-0.16]) and [mode:lp] stays without a
     line FOREVER, because every later click on the same row number is
     short-circuited too. Measured on :123 at 738a530: after an external delete
     closed a tab, five clicks on the next note's line 0 produced no raw row.
     So the memo is only trusted when g.lpActive AGREES with it — one condition
     here instead of an invalidation call every caller must remember. */
  if (Ed.cur && Ed.cur.g === g && Ed.cur.l === l && g.lpActive && g.lpActive.l0 === l) { Ed.census(); return; }
  Ed.cur = { g, l };
  Ed.mark(g, l);
});

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
    const rows = g.lp ? g.lp.children : null;
    if (!rows) return "";
    const lb = g.lp.getBoundingClientRect(), r = document.createRange();
    for (let i = 0; i < rows.length && i < 400; i++) {
      const q = rows[i].getBoundingClientRect();
      if (q.bottom <= lb.top) continue;
      if (q.top >= lb.bottom) break;
      if (rows[i].querySelector("input")) continue;
      const r0 = Ed.firstRect(rows[i], r);
      if (!r0 || !r0.width) continue;
      return " [rvtaskex:" + Math.round(r0.left + 2) + "," + Math.round(q.top + q.height / 2) + "]";
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
document.addEventListener("scroll", e => {
  const t = e.target;
  if (!t || !t.classList || !t.classList.contains("preview")) return;
  clearTimeout(rvTaskScT);
  rvTaskScT = setTimeout(() => { rvTaskScT = null; if (typeof updateTitle === "function") updateTitle(); }, 60);
}, true);
