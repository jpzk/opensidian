/* R17 EDITOR CORE — pure line model (no DOM, no IPC).
   Feedback #9: the caret-row-textarea design must die. This module is the
   half of the new engine that can be unit-tested without a browser: it turns
   ONE source line into (a) a piece list carrying raw source ranges and
   (b) HTML, plus the col<->offset maps the caret and the selection ride on.

   Vocabulary (R17.6): a LINE is a list of PIECES.
     piece = { k:"t"|"m", s, e, cls, tok }
       k="t" TEXT   — rendered verbatim, source[s,e) maps 1:1 to output
       k="m" MARKER — rendered only when its TOKEN is revealed
       tok          — index into tokens[]; a marker is revealed iff the
                      selection intersects tokens[tok] raw range (R17.6,
                      per TOKEN, never per line)
     tokens[i] = { s, e, type }   raw range of the whole construct
   Every piece is identity-mapped: out === src.slice(s,e) or "" — that
   invariant is what makes (line,col) <-> DOM offset exact, which is what the
   old rendered-vs-source two-pointer guess in lpCol() could never be. */
(function (root) {
"use strict";

const esc = s => s.replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

// ---- line prefix (block markers): heading / quote / list / task / hr ----
// Returns { i, cls, kind, marks:[[s,e]], meta } — i = col where inline starts.
function scanPrefix(src, P) {
  let i = 0, cls = [], kind = "p", marks = [], meta = {};
  // blockquote, possibly nested: "> " repeated
  let q = 0;
  for (;;) {
    const m = /^[ \t]{0,3}>[ ]?/.exec(src.slice(i));
    if (!m) break;
    marks.push([i, i + m[0].length]); i += m[0].length; q++;
  }
  if (q) { cls.push("q"); kind = "quote"; meta.quote = q; }
  const rest = src.slice(i);
  let m;
  if ((m = /^(#{1,6})([ \t]+)/.exec(rest))) {           // heading
    marks.push([i, i + m[0].length]);
    cls.push("h" + m[1].length); kind = "h"; meta.level = m[1].length;
    i += m[0].length;
  } else if ((m = /^([ \t]*)([-*+]|\d{1,9}[.)])([ \t]+)(\[([ xX])\][ \t]+)?/.exec(rest))) {
    const ind = m[1];
    marks.push([i + ind.length, i + m[0].length]);      // indent is NOT a marker
    kind = m[4] ? "task" : (/^\d/.test(m[2]) ? "ol" : "ul");
    cls.push("li", kind);
    meta.indent = ind; meta.bullet = m[2]; meta.mark = m[0].slice(ind.length);
    if (m[4]) meta.checked = m[5] !== " ";
    if (meta.checked) cls.push("done");
    i += m[0].length;
    meta.contentCol = i;
  } else if (/^(\*\s*\*\s*\*|-\s*-\s*-|_\s*_\s*_)[\s]*$/.test(rest) && rest.trim()) {
    marks.push([i, src.length]); cls.push("hr"); kind = "hr"; i = src.length;
  }
  if (kind === "h" || kind === "quote") meta.contentCol = i;
  P.prefixCols = i;
  return { i, cls, kind, marks, meta };
}

// ---- inline scan ----------------------------------------------------
// sticky patterns, tried in precedence order at each position
const PATS = [
  ["code",  /(`+)([^`\n]*?)\1/y],
  ["embed", /!\[\[([^\[\]\n]*)\]\]/y],
  ["wiki",  /\[\[([^\[\]\n|]*)(\|[^\[\]\n]*)?\]\]/y],
  ["link",  /\[([^\[\]\n]*)\]\(([^()\s\n]*)\)/y],
  ["bold",  /(\*\*|__)(?=\S)([\s\S]*?\S)\1/y],
  ["ital",  /(\*|_)(?=\S)((?:[^\s*_]|[^\s][^*_]*?[^\s])?)\1/y],
  ["strike",/~~(?=\S)([\s\S]*?\S)~~/y],
  ["hl",    /==(?=\S)([\s\S]*?\S)==/y],
  ["tag",   /#[A-Za-z0-9_\/-]*[A-Za-z_\/][A-Za-z0-9_\/-]*/y],
];

function scanInline(src, from, to, cls, P) {
  let run = from;
  const flush = at => { if (at > run) P.pieces.push({ k: "t", s: run, e: at, cls }); };
  let i = from;
  while (i < to) {
    let hit = null;
    for (const [name, re] of PATS) {
      re.lastIndex = i;
      const m = re.exec(src);
      if (!m || m.index !== i || i + m[0].length > to) continue;
      if (name === "tag" && i > 0 && !/[\s(\[]/.test(src[i - 1])) continue;
      if (name === "ital" && /[A-Za-z0-9]/.test(src[i - 1] || "")) continue;  // intra_word
      hit = { name, m }; break;
    }
    if (!hit) { i++; continue; }
    flush(i);
    const { name, m } = hit, len = m[0].length, tok = P.tokens.length;
    P.tokens.push({ s: i, e: i + len, type: name });
    const mk = (s, e, c) => { if (e > s) P.pieces.push({ k: "m", s, e, cls: c || "", tok }); };
    const tx = (s, e, c) => { if (e > s) P.pieces.push({ k: "t", s, e, cls: c, tok }); };
    if (name === "code") {
      const d = m[1].length;
      mk(i, i + d, "code"); tx(i + d, i + len - d, cls + " code"); mk(i + len - d, i + len, "code");
    } else if (name === "wiki" || name === "embed") {
      const o = name === "embed" ? 3 : 2;                 // "![[" | "[["
      const tgt = m[1] || "", al = m[2] || "";
      const c = cls + " wiki";
      if (al) {                                            // [[Target|Alias]] -> Alias
        mk(i, i + o + tgt.length + 1, "wikib");
        tx(i + o + tgt.length + 1, i + len - 2, c);
      } else {
        mk(i, i + o, "wikib"); tx(i + o, i + len - 2, c);
      }
      mk(i + len - 2, i + len, "wikib");
    } else if (name === "link") {
      const t = m[1];
      mk(i, i + 1, "linkb");
      scanInline(src, i + 1, i + 1 + t.length, cls + " ext", P);
      mk(i + 1 + t.length, i + len, "linkb");
    } else if (name === "tag") {
      tx(i, i + len, cls + " tag");                       // R17.6: '#' never hidden
    } else {
      const d = name === "bold" ? m[1].length : name === "ital" ? 1 : 2;
      const c = cls + " " + name;
      mk(i, i + d, name);
      scanInline(src, i + d, i + len - d, c, P);
      mk(i + len - d, i + len, name);
    }
    run = i = i + len;
  }
  flush(to);
}

/* parseLine(src) -> { pieces, tokens, cls, kind, meta, contentCol } */
function parseLine(src) {
  const P = { pieces: [], tokens: [] };
  const pre = scanPrefix(src, P);
  for (const [s, e] of pre.marks) {
    const tok = P.tokens.length;
    P.tokens.push({ s, e, type: "block" });
    if (s > 0 || e > 0) P.pieces.push({ k: "m", s, e, cls: "bm", tok });
  }
  if (pre.i > 0) {                                        // indent of a nested item is TEXT
    const ind = /^[ \t]*/.exec(src)[0].length;
    if (ind && P.pieces.length && P.pieces[0].s === ind)
      P.pieces.unshift({ k: "t", s: 0, e: ind, cls: "ind" });
  }
  if (pre.kind !== "hr") scanInline(src, pre.i, src.length, "", P);
  P.pieces.sort((a, b) => a.s - b.s || a.e - b.e);
  return { pieces: P.pieces, tokens: P.tokens, cls: pre.cls, kind: pre.kind,
           meta: pre.meta, contentCol: pre.meta.contentCol == null ? 0 : pre.meta.contentCol };
}

/* revealed(tokens, sel) -> bool[] . sel = null | {s,e} raw cols of the
   selection ON THIS LINE (caret = s===e). R17.6: a token reveals iff the
   selection intersects its raw range; a collapsed caret at either edge
   counts as touching. */
function revealSet(tokens, sel, all) {
  const r = new Array(tokens.length).fill(!!all);
  if (all || !sel) return r;
  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i];
    r[i] = sel.s <= t.e && sel.e >= t.s;
  }
  return r;
}

/* visible(pieces, rev) -> the pieces actually rendered, in order */
function visible(pieces, rev) {
  return pieces.filter(p => p.k === "t" || (p.tok == null ? true : rev[p.tok]));
}

/* col (raw source col) -> rendered offset, and back. Both clamp. */
function colToOff(pieces, rev, col) {
  let off = 0;
  for (const p of visible(pieces, rev)) {
    if (col <= p.s) return off;
    if (col < p.e) return off + (col - p.s);
    off += p.e - p.s;
  }
  return off;
}
function offToCol(pieces, rev, off, srcLen) {
  let o = 0, last = 0;
  for (const p of visible(pieces, rev)) {
    const n = p.e - p.s;
    if (off <= o + n) return p.s + (off - o);
    o += n; last = p.e;
  }
  return srcLen == null ? last : srcLen;
}

/* lineHTML(src, sel, opts) -> html for ONE row's inner content.
   Every emitted span carries data-s (raw start col) so the DOM->model
   direction is a lookup, never a guess. */
function lineHTML(src, sel, opts) {
  opts = opts || {};
  const L = parseLine(src);
  const rev = revealSet(L.tokens, sel, opts.all);
  let h = "";
  for (const p of visible(L.pieces, rev)) {
    const t = src.slice(p.s, p.e);
    const cl = ((p.k === "m" ? "mk " : "") + (p.cls || "")).trim();
    h += '<span data-s="' + p.s + '"' + (cl ? ' class="' + esc(cl) + '"' : "") + ">" + esc(t) + "</span>";
  }
  if (!h) h = '<span data-s="0" class="pad"><br></span>';
  return { html: h, line: L, rev: rev };
}

root.EDT = { esc, parseLine, revealSet, visible, colToOff, offToCol, lineHTML };
})(typeof globalThis !== "undefined" ? globalThis : this);
