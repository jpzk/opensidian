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
  cur: null,            // {g, l} row currently holding the caret (reveal state)
  composing: false,

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

  row(g, line, i) {
    const row = Ed.el("div", "lprow");
    row.dataset.l = i;
    Ed.block(g, row, line);
    if (!row.firstChild) row.appendChild(document.createElement("br"));
    return row;
  },

  // block-level prefix (heading / quote / hr / table / list / task), then inline
  block(g, row, s) {
    const ind = s.match(/^[ \t]*/)[0], t = s.slice(ind.length);
    const h = /^(#{1,6}) /.exec(t);
    if (h && !ind) {
      const box = Ed.el("span", "h h" + h[1].length);
      box.appendChild(Ed.mk(h[1]));
      box.appendChild(Ed.mk(" "));
      Ed.inline(g, box, t.slice(h[1].length + 1));
      row.appendChild(box);
      return;
    }
    const tt = t.replace(/\s+$/, "");
    if (tt.length >= 3 && /^(-+|\*+|_+)$/.test(tt)) {          // hr
      const box = Ed.el("span", "hr"); box.appendChild(Ed.mk(s)); row.appendChild(box); return;
    }
    if (t.startsWith(">")) {                                    // blockquote
      const box = Ed.el("span", "bq");
      if (ind) box.appendChild(Ed.t(ind));
      box.appendChild(Ed.mk(">"));
      let rest = t.slice(1);
      if (rest.startsWith(" ")) { box.appendChild(Ed.mk(" ")); rest = rest.slice(1); }
      Ed.block(g, box, rest);                                   // "> - a": list inside the quote
      row.appendChild(box);
      return;
    }
    if (t.startsWith("|")) {                                    // table row (block render is on demand)
      const box = Ed.el("span", "tbl"); box.appendChild(Ed.t(s)); row.appendChild(box); return;
    }
    if (/^(```|~~~)/.test(t)) {                                 // fence delimiter line
      const box = Ed.el("span", "fence"); box.appendChild(Ed.mk(s)); row.appendChild(box); return;
    }
    const lm = /^([-*+]|\d+\.) /.exec(t);
    if (lm) {
      const rest = t.slice(lm[0].length);
      if (ind) row.appendChild(Ed.t(ind));
      row.appendChild(Ed.mk(lm[1] + " ", "lim"));
      row.classList.add("li");
      const task = /^\[([ xX])\] /.exec(rest);
      if (task) {
        row.classList.add("tk");
        const cb = Ed.el("input", "tbox");
        cb.type = "checkbox"; cb.checked = task[1] !== " ";
        cb.contentEditable = "false";
        cb.addEventListener("mousedown", e => Ed.toggleTask(e, row));
        row.appendChild(cb);
        row.appendChild(Ed.mk("[" + task[1] + "] ", "task"));
        Ed.inline(g, row, rest.slice(4));
      } else {
        Ed.inline(g, row, rest);
      }
      return;
    }
    if (ind) row.appendChild(Ed.t(ind));
    Ed.inline(g, row, t);
  },

  // inline markers, longest-opener-wins, unmatched markers fall through as text
  inline(g, box, s) {
    let plain = "";
    const flush = () => { if (plain) { box.appendChild(Ed.t(plain)); plain = ""; } };
    const tags = Ed.tagSpans(s);
    let i = 0;
    while (i < s.length) {
      const rest = s.slice(i);
      const tg = tags.find(x => x[0] === i);
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
          c.appendChild(Ed.mk("`"));
          c.appendChild(Ed.t(s.slice(i + 1, j)));
          c.appendChild(Ed.mk("`"));
          box.appendChild(c); i = j + 1; continue;
        }
      }
      const wl = /^(!?)\[\[([^\]]*)\]\]/.exec(rest);             // [[wikilink]] / ![[embed]]
      if (wl) {
        flush();
        const raw = wl[2], bar = raw.indexOf("|");
        const target = bar >= 0 ? raw.slice(0, bar) : raw;
        const label = bar >= 0 ? raw.slice(bar + 1) : raw;
        const hash = target.indexOf("#");
        box.appendChild(Ed.mk(wl[1] + "[["));
        if (bar >= 0) box.appendChild(Ed.mk(target + "|", "wl"));
        const a = Ed.el("a", "wiki wl");
        a.href = "#";
        const note = hash >= 0 ? target.slice(0, hash) : target;
        if (note) a.dataset.note = note;
        if (hash >= 0) a.dataset.anchor = target.slice(hash + 1);
        if (!Ed.resolves(note)) a.classList.add("wiki-unresolved");
        a.appendChild(Ed.t(label));
        a.addEventListener("mousedown", e => Ed.linkClick(e, a));
        box.appendChild(a);
        box.appendChild(Ed.mk("]]"));
        i += wl[0].length; continue;
      }
      const lt = /^\[([^\[\]]*)\]\(([^)\s]*)\)/.exec(rest);      // [text](url)
      if (lt) {
        flush();
        box.appendChild(Ed.mk("["));
        const ext = /^https?:\/\//.test(lt[2]);
        const a = Ed.el("a", ext ? "ext lt" : "lt");
        a.href = ext ? lt[2] : "#";       // S1: the capture-phase gate hands http(s) to open_external
        a.dataset.url = lt[2];
        a.appendChild(Ed.t(lt[1]));
        a.addEventListener("mousedown", e => Ed.extClick(e, a));
        box.appendChild(a);
        box.appendChild(Ed.mk("](", "url"));
        box.appendChild(Ed.mk(lt[2], "url"));
        box.appendChild(Ed.mk(")", "url"));
        i += lt[0].length; continue;
      }
      const url = /^https?:\/\/\S+/.exec(rest);                  // bare url
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
          box.appendChild(Ed.mk(open));
          const e = Ed.el(tag, cls);
          Ed.inline(g, e, s.slice(a0, j));
          box.appendChild(e);
          box.appendChild(Ed.mk(open));
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

  /* ---------- token map: source col <-> DOM offset ---------- */
  // The row's text nodes in document order carry the whole source line, in
  // order, so the map is implicit. `hidden` = the node sits inside a span.mk
  // that CSS is not revealing — never a caret target while the row is folded.
  nodes(row) {
    const out = [];
    const w = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
    let c = 0, n;
    while ((n = w.nextNode())) {
      out.push({ n, c, len: n.nodeValue.length, hid: !!n.parentElement.closest(".mk") });
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

  /* ---------- caret + selection ---------- */
  rowAt(g, l) { return g.lp.children[l] || null; },
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
    Ed.reveal(g, l);                             // markers first: offsets stay valid
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
  // the caret row is the ONLY row with its markers revealed (stock behaviour)
  reveal(g, l) {
    const prev = g.lp.querySelector(".lprow.cur");
    const row = Ed.rowAt(g, l);
    if (prev === row) return;
    if (prev) prev.classList.remove("cur");
    if (row) row.classList.add("cur");
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
    const t0 = performance.now();
    let touched = 0;
    if (full || !old || lp.children.length !== old.length) {
      const frag = document.createDocumentFragment();
      for (let i = 0; i < L.length; i++) frag.appendChild(Ed.row(g, L[i], i));
      lp.textContent = "";
      lp.appendChild(frag);
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
    else Ed.reveal(g, -1);
    if (typeof perf !== "undefined" && perf.mark)
      perf.mark("lp_render", t0, { lines: L.length, patched: touched, full: !!full || !old });
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
    if (last && last.kind === kind && kind === "type" && now - last.t < 700) { last.t = now; return; }
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
    const mid = ins.length === 1 ? [head + ins[0] + tail]
      : [head + ins[0]].concat(ins.slice(1, -1), [ins[ins.length - 1] + tail]);
    L.splice(a.l, b.l - a.l + 1, ...mid);
    const cl = a.l + ins.length - 1;
    const cc = ins.length === 1 ? a.c + ins[0].length : ins[ins.length - 1].length;
    Ed.after(g, cl, cc);
  },
  enter(g, s) {                                  // Enter: split + list continuation
    const L = Ed.lines(g), line = L[s.a.l] || "";
    const m = /^(\s*)([-*+] \[[ xX]\] |[-*+] |\d+\. )/.exec(line);
    if (m && s.empty && line === m[0]) {          // empty item: clear the marker
      Ed.snap(g, "enter");
      L[s.a.l] = "";
      return Ed.after(g, s.a.l, 0);
    }
    const cont = m && s.a.c >= m[0].length
      ? m[1] + (m[2].includes("[") ? m[2].replace(/\[[xX]\]/, "[ ]")
        : /^\d/.test(m[2]) ? (parseInt(m[2], 10) + 1) + ". " : m[2])
      : "";
    Ed.replace(g, s, "\n" + cont, "enter");
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
  indent(g, s, out) {                            // Tab / Shift+Tab on the line
    const L = Ed.lines(g);
    Ed.snap(g, "indent");
    let dc = 0;
    for (let l = s.a.l; l <= s.b.l; l++) {
      if (out) {
        const m = /^(\t| {1,4})/.exec(L[l]);
        if (m) { L[l] = L[l].slice(m[0].length); if (l === s.b.l) dc = -m[0].length; }
      } else { L[l] = "\t" + L[l]; if (l === s.b.l) dc = 1; }
    }
    Ed.after(g, s.b.l, Math.max(0, s.b.c + dc));
  },
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
    if (e.key === "Tab") { e.preventDefault(); return Ed.indent(g, s, e.shiftKey); }
    // Shift+key arrives UPPERCASE (e.key is the produced character): Ctrl+Shift+Z
    // is "Z", so a lowercase-only compare silently loses redo (R17.8 M83).
    const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    if (k === "a" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); return Ed.selectAll(g); }
    if (k === "z" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); return e.shiftKey ? Ed.redo(g) : Ed.undo(g); }
    if (k === "y" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); return Ed.redo(g); }
    if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      const f = Ed.focusPos(g) || s.b;                      // shift-extend moves the FOCUS, not s.b
      const h = Ed.homeCol(g, f.l);
      const c = e.key === "End" ? Ed.lines(g)[f.l].length : (f.c === h ? 0 : h);
      return e.shiftKey ? Ed.extendTo(g, f.l, c) : Ed.place(g, f.l, c);
    }
  },
  onInput(g, e) {
    if (!g) return;
    if (Ed.composing || e.inputType === "insertCompositionText") return;   // IME: native, reconciled on end
    const s = Ed.sel(g);
    if (!s) return;
    const sp = typeof otel !== "undefined" && otel.begin
      ? otel.begin("key_to_paint", { key: e.inputType, note_lines: g.lpLines || 0, note: (typeof curOf === "function" && curOf(g)) || "" })
      : null;
    e.preventDefault();
    switch (e.inputType) {
      case "insertText": Ed.replace(g, s, e.data || "", "type"); break;
      case "insertParagraph": Ed.enter(g, s); break;
      case "insertLineBreak": Ed.replace(g, s, "\n", "type"); break;
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
                   "1. numbered ~~s~~ ==hl== end", "```js", "text with https://x.example/z end"];
    let bad = 0;
    const g = { view: {}, lp: document.createElement("div") };
    for (const c of cases) {
      const row = Ed.row(g, c, 0);
      if (row.textContent !== c) { bad++; continue; }
      for (let k = 0; k <= c.length; k++) {          // col -> DOM -> col round trip
        const [n, o] = Ed.posOf(row, k, true);
        if (n === row) continue;
        if (Ed.colOf(row, n, o) !== k) { bad++; break; }
      }
    }
    // Home targets (M64-M68), checked on the model, not the DOM
    const hg = { view: { lines: ["- alpha", "\t- beta", "- [ ] alpha", "# Head", "plain", "> quoted", "1. one"] } };
    const hw = [2, 3, 6, 0, 0, 2, 3];
    for (let i = 0; i < hw.length; i++) if (Ed.homeCol(hg, i) !== hw[i]) bad++;
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
    r.selectNodeContents(rows[i]);
    const rc = r.getClientRects();
    xs += (i ? "," : "") + Math.round(rc.length ? rc[0].left : q.left);
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

/* The caret row is the only revealed row (stock). Selection moves that the
   browser handles natively (arrows, clicks, drag-select) land here — no
   re-render, just the .cur class + the [mode:lp:<line>] census. */
document.addEventListener("selectionchange", () => {
  const s = window.getSelection();
  if (!s || !s.anchorNode) return;
  const row = Ed.rowOf(s.anchorNode);
  const lp = row ? row.parentElement : null;
  if (!lp || !lp.classList.contains("lp")) return;
  const pane = lp.closest(".pane"), g = pane ? pane._g : null;
  if (!g || g.lp !== lp) return;
  const l = Ed.indexOf(row);
  if (Ed.cur && Ed.cur.g === g && Ed.cur.l === l) { Ed.census(); return; }
  Ed.cur = { g, l };
  Ed.reveal(g, l);
  Ed.mark(g, l);
});
