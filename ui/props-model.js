// SPDX-License-Identifier: GPL-3.0-or-later
/* PROPERTIES MODEL (issue #119) — the note's YAML frontmatter as editable rows,
   with one rule above every other: a write changes the bytes of the ONE entry it
   edits and nothing else. Pure (no DOM, no app state): ui/props.js draws it and
   src-tauri/tests/props.rs runs it in JavaScriptCore against byte-exact fixtures.

   The frontmatter is the first line "---" (trailing blanks allowed) up to the next
   "---" or "..." line, the same rule as the Rust index (src-tauri/src/index.rs
   frontmatter). Its lines are cut into ENTRIES: a top-level "key:" line at column 0
   plus its continuation lines (indented lines, or "- item" lines under an empty
   value). Blank and comment lines between entries are GAPS: kept, never shown,
   never rewritten.

   An entry is EDITABLE only when its value is in the subset this model reads
   faithfully: an empty value, a plain or simply quoted scalar, a flow list or a
   block list of such scalars. Anything else (a nested map, a block scalar | >,
   anchors / aliases / tags, a trailing comment, a comment or blank inside the
   entry, a type the vault assigned that this model does not know, a value that
   does not fit its type) is READ-ONLY: shown as its raw text, never rewritten
   (rename and delete still work, they touch only the key text / drop the lines).

   A frontmatter this model cannot cut (a top-level line that is neither a key
   nor a comment, tab indentation, a duplicate key, an unterminated block) is
   INVALID: no rows, the editor keeps showing the raw YAML, and every edit
   function returns null — nothing is ever written from an invalid block. */
(function (root) {
  "use strict";
  const KNOWN = ["text", "multitext", "number", "checkbox", "date", "datetime", "tags", "aliases"];
  const LISTY = { multitext: 1, tags: 1, aliases: 1 };
  const RESERVED = { tags: "tags", aliases: "aliases", cssclasses: "multitext" };
  const KEY_RE = /^([^\s#\-?:,\[\]{}&*!|>'"%@`][^:#]*?)[ \t]*:(?:[ \t]+(.*?))?[ \t]*$/;
  const QKEY_RE = /^("(?:[^"\\]|\\.)*"|'(?:[^']|'')*')[ \t]*:(?:[ \t]+(.*?))?[ \t]*$/;
  const ITEM_RE = /^( *)-(?:[ \t]+(.*?))?[ \t]*$/;
  const DATE_RE = /^\d{4}-\d{2}-\d{2}$/, DT_RE = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2})?$/;
  // the shape AND the calendar: 2024-99-99 / 2023-02-29 / 25:00 are not dates (a typed date bound to them
  // would render blank and a blur would clear the stored value) -> such a value is a read-only type mismatch
  function calOk(s) {
    const y = +s.slice(0, 4), m = +s.slice(5, 7), d = +s.slice(8, 10);
    if (m < 1 || m > 12 || d < 1 || d > new Date(Date.UTC(y, m, 0)).getUTCDate()) return false;
    if (s.length > 10 && (+s.slice(11, 13) > 23 || +s.slice(14, 16) > 59 || (s.length > 16 && +s.slice(17, 19) > 59))) return false;
    return true;
  }
  const isDate = s => DATE_RE.test(s) && calOk(s), isDT = s => DT_RE.test(s) && calOk(s);

  // the lines of text with their offsets: {s, e (end of text, eol excluded), n (next line start), t}
  function lines(text) {
    const out = [];
    let s = 0;
    while (s < text.length) {
      let n = text.indexOf("\n", s);
      if (n < 0) { out.push({ s, e: text.length, n: text.length, t: text.slice(s) }); break; }
      const e = n > s && text[n - 1] === "\r" ? n - 1 : n;
      out.push({ s, e, n: n + 1, t: text.slice(s, e) });
      s = n + 1;
    }
    return out;
  }

  /* one scalar of the faithful subset -> {v} | null. Plain: YAML core typing for
     true/false/null and plain decimal numbers; every other number-ish spelling
     (0x1F, 1e3, .inf, 1_000, 012) is not read here -> null (read-only). */
  function scalar(s) {
    if (s === "") return { v: null };
    if (s[0] === '"') {
      if (s.length < 2 || s[s.length - 1] !== '"') return null;
      try { const v = JSON.parse(s); return typeof v === "string" ? { v } : null; } catch (_) { return null; }
    }
    if (s[0] === "'") {
      if (s.length < 2 || s[s.length - 1] !== "'") return null;
      const inner = s.slice(1, -1);
      if (/'(?!')/.test(inner.replace(/''/g, ""))) return null;
      return { v: inner.replace(/''/g, "'") };
    }
    if (/^[\[\]{}#&*!|>%@`,?:]/.test(s) || /^-(\s|$)/.test(s) || /: |:$| #|\t/.test(s)) return null;
    if (/^(true|True|TRUE)$/.test(s)) return { v: true };
    if (/^(false|False|FALSE)$/.test(s)) return { v: false };
    if (/^(null|Null|NULL|~)$/.test(s)) return { v: null };
    if (/^-?(0|[1-9]\d*)(\.\d+)?$/.test(s)) return { v: Number(s) };
    if (/^[-+]?(\.\d|\d)[\d_]*(\.[\d_]*)?([eE][-+]?\d+)?$/.test(s) || /^[-+]?0[xXoObB]/.test(s) ||
        /^[-+]?\.(inf|Inf|INF|nan|NaN|NAN)$/.test(s) || /^[-+]?\d+(:[0-5]?\d)+(\.\d*)?$/.test(s)) return null;   // other number spellings (YAML 1.1 too)
    return { v: s };
  }
  // "[a, 'b', "c"]" -> {v: [...]} | null (no nesting, no empty items)
  function flowList(s) {
    if (s[s.length - 1] !== "]") return null;
    const inner = s.slice(1, -1).trim();
    if (inner === "") return { v: [] };
    const items = [];
    let i = 0;
    while (i <= inner.length) {
      let j = i, q = null;
      for (; j < inner.length; j++) {
        const c = inner[j];
        if (q) { if (c === "\\" && q === '"') { j++; continue; } if (c === q) q = null; continue; }
        if (c === '"' || c === "'") q = c;
        else if (c === ",") break;
        else if (c === "[" || c === "]" || c === "{" || c === "}") return null;
      }
      if (q) return null;
      const one = scalar(inner.slice(i, j).trim());
      if (!one || one.v === null || typeof one.v === "object") return null;
      items.push(one.v);
      i = j + 1;
      if (j >= inner.length) break;
    }
    return { v: items };
  }
  function keyName(raw) {
    if (raw[0] === '"') { try { return JSON.parse(raw); } catch (_) { return null; } }
    if (raw[0] === "'") return raw.slice(1, -1).replace(/''/g, "'");
    return raw;
  }

  /* parse(text) -> null (no frontmatter) | {invalid: reason} | the block:
     {from, to: offset after the closing line, body: same, eol, open, close: line objects,
      entries: [{key, keyRaw, from, to (line range incl. eol), first, lines, value, ok, why, itemIndent}],
      gaps: n} */
  function parse(text) {
    const L = lines(text);
    if (!L.length || L[0].t.replace(/[ \t]+$/, "") !== "---") return null;
    let close = -1;
    for (let i = 1; i < L.length; i++) { const t = L[i].t.replace(/[ \t]+$/, ""); if (t === "---" || t === "...") { close = i; break; } }
    if (close < 0) return { invalid: "the frontmatter is not closed" };
    const eol = text.slice(L[0].e, L[0].n) || "\n";
    const entries = [], seen = new Set();
    let cur = null, gap = [];
    for (let i = 1; i < close; i++) {
      const l = L[i], t = l.t;
      if (!t.trim() || /^[ \t]*#/.test(t)) { gap.push(l); continue; }
      if (/^\t/.test(t) || /^ *\t/.test(t)) return { invalid: "tab indentation (line " + (i + 1) + ")" };
      const top = !/^[ \t]/.test(t);
      if (top && !(cur && cur.inline === "" && ITEM_RE.test(t) && !cur.indented)) {
        const m = t.match(KEY_RE) || t.match(QKEY_RE);
        if (!m) return { invalid: "not a key: line " + (i + 1) };
        const key = keyName(m[1]);
        if (key == null || key === "") return { invalid: "bad key: line " + (i + 1) };
        if (seen.has(key)) return { invalid: "duplicate key " + key };
        seen.add(key);
        gap = [];
        cur = { key, keyRaw: m[1], inline: m[2] || "", first: l, lines: [l], indented: false, gapIn: false };
        entries.push(cur);
        continue;
      }
      if (!cur) return { invalid: "indented line before the first key: line " + (i + 1) };
      if (gap.length) { cur.gapIn = true; for (const g of gap) cur.lines.push(g); gap = []; }
      if (!top) cur.indented = true;
      cur.lines.push(l);
    }
    // case-insensitive twins (Tags + tags): legal YAML, but one property to the app -> both read-only
    const lc = {};
    for (const e of entries) lc[e.key.toLowerCase()] = (lc[e.key.toLowerCase()] || 0) + 1;
    for (const e of entries) {
      const last = e.lines[e.lines.length - 1];
      e.from = e.first.s; e.to = last.n;
      read(e);
      if (lc[e.key.toLowerCase()] > 1) { e.ok = false; e.why = "two keys differ only in case"; }
    }
    return { from: 0, to: L[close].n, body: L[close].n, eol, open: L[0], close: L[close], entries,
             raw: text.slice(0, L[close].n) };
  }
  // the entry's value, when it is in the faithful subset
  function read(e) {
    e.ok = false; e.value = undefined; e.why = "";
    const cont = e.lines.slice(1);
    if (e.gapIn) { e.why = "a comment or blank line inside the value"; return; }
    if (!cont.length) {
      const v = e.inline;
      const r = v[0] === "[" ? flowList(v) : scalar(v);
      if (!r) { e.why = "a value this editor does not rewrite faithfully"; return; }
      e.value = r.v; e.ok = true; e.flow = v[0] === "[";
      return;
    }
    if (e.inline !== "") { e.why = "a multi-line value"; return; }
    let ind = null;
    const items = [];
    for (const l of cont) {
      const m = l.t.match(ITEM_RE);
      if (!m) { e.why = "a nested value"; return; }
      if (ind === null) ind = m[1];
      else if (m[1] !== ind) { e.why = "a nested list"; return; }
      const r = scalar(m[2] || "");
      if (!r || r.v === null || typeof r.v === "object") { e.why = "a list item this editor does not rewrite faithfully"; return; }
      items.push(r.v);
    }
    e.value = items; e.ok = true; e.itemIndent = ind;
  }

  // types: {lowercase name: type}. -> the row's type (a known one, or the unknown name)
  function typeOf(key, value, types) {
    const k = String(key).toLowerCase(), a = types && Object.prototype.hasOwnProperty.call(types, k) ? types[k] : null;
    if (a) return a;
    if (RESERVED[k]) return RESERVED[k];
    return infer(value);
  }
  function infer(v) {
    if (typeof v === "number") return "number";
    if (typeof v === "boolean") return "checkbox";
    if (Array.isArray(v)) return "multitext";
    if (typeof v === "string" && isDate(v)) return "date";
    if (typeof v === "string" && isDT(v)) return "datetime";
    return "text";
  }
  function fits(type, v) {
    if (v === null) return true;
    if (LISTY[type]) return Array.isArray(v) ? v.every(x => typeof x === "string") : typeof v === "string";
    if (type === "text") return typeof v === "string";
    if (type === "number") return typeof v === "number";
    if (type === "checkbox") return typeof v === "boolean";
    if (type === "date") return typeof v === "string" && isDate(v);
    if (type === "datetime") return typeof v === "string" && isDT(v);
    return false;
  }
  /* rows(text, types) -> null | {invalid} | {rows: [{key, type, value, ok, why, raw}]}
     ok=false = read-only. A known type whose value does not fit = read-only "type mismatch". */
  function rows(text, types) {
    const p = parse(text);
    if (!p || p.invalid) return p;
    return { rows: p.entries.map(e => {
      const type = typeOf(e.key, e.value, types), raw = text.slice(e.from, e.to).replace(/\r?\n$/, "");
      let ok = e.ok, why = e.why;
      if (ok && KNOWN.indexOf(type) < 0) { ok = false; why = "the type " + type + " is not editable here"; }
      if (ok && !fits(type, e.value)) { ok = false; why = "type mismatch: the value is not a " + type; }
      let value = e.value;
      if (ok && LISTY[type] && typeof value === "string") value = [value];
      return { key: e.key, type, value: ok ? value : undefined, ok, why, raw };
    }) };
  }

  // ---- emitting ----
  const PLAIN_KEY = /^[^\s#\-?:,\[\]{}&*!|>'"%@`][^:#]*$/;
  function emitKey(name) {
    const s = String(name);
    return PLAIN_KEY.test(s) && s === s.trim() && !/[\r\n\t]/.test(s) ? s : JSON.stringify(s);
  }
  // a string as a scalar that reads back as that same STRING
  function emitStr(s) {
    s = String(s);
    const r = scalar(s);
    return r && r.v === s && s === s.trim() && !/[\r\n]/.test(s) ? s : JSON.stringify(s);
  }
  function emitVal(type, v) {
    if (v === null || v === undefined || v === "") return null;
    if (type === "number") return Number.isFinite(v) ? String(v) : null;
    if (type === "checkbox") return v ? "true" : "false";
    return emitStr(v);
  }
  // the full text of an entry (all its lines, each with eol)
  function emitEntry(keyRaw, type, value, eol, itemIndent) {
    if (LISTY[type]) {
      const items = (Array.isArray(value) ? value : value == null || value === "" ? [] : [value]).map(String).filter(x => x !== "");
      if (!items.length) return keyRaw + ":" + eol;
      const ind = itemIndent == null ? "  " : itemIndent;
      return keyRaw + ":" + eol + items.map(x => ind + "- " + emitStr(x) + eol).join("");
    }
    const s = emitVal(type, value);
    return keyRaw + ":" + (s == null ? "" : " " + s) + eol;
  }
  function find(p, key) { for (const e of p.entries) if (e.key === key) return e; return null; }
  function entryEol(text, e, p) { const l = e.lines[e.lines.length - 1]; return text.slice(l.e, l.n) || p.eol; }

  /* ---- edits: (text, ...) -> {from, to, insert} (a change of the ORIGINAL text) | null (refused) ---- */
  // the value of an editable entry. type = the row's type (the value is written in that type's shape)
  function setValue(text, key, type, value, types) {
    const p = parse(text);
    if (!p || p.invalid || KNOWN.indexOf(type) < 0) return null;
    const e = find(p, key);
    if (!e) return null;
    const R = rows(text, types).rows.find(r => r.key === key);
    if (!R || !R.ok) return null;
    if (type === "number" && value !== null && value !== "" && (typeof value !== "number" || !Number.isFinite(value))) return null;
    const insert = emitEntry(e.keyRaw, type, value, entryEol(text, e, p), e.itemIndent);
    if (insert === text.slice(e.from, e.to)) return { from: e.from, to: e.from, insert: "" };
    return { from: e.from, to: e.to, insert };
  }
  // rename: only the key text of the first line changes (any entry, read-only ones too)
  function rename(text, key, name) {
    const p = parse(text);
    name = String(name == null ? "" : name).trim();
    if (!p || p.invalid || !name) return null;
    const e = find(p, key);
    if (!e) return null;
    if (name === key) return { from: e.from, to: e.from, insert: "" };
    if (p.entries.some(x => x !== e && x.key.toLowerCase() === name.toLowerCase())) return null;
    return { from: e.from, to: e.from + e.keyRaw.length, insert: emitKey(name) };
  }
  // delete: the entry's lines go; an emptied block (no entries, no comments) goes with them
  function remove(text, key) {
    const p = parse(text);
    if (!p || p.invalid) return null;
    const e = find(p, key);
    if (!e) return null;
    if (p.entries.length === 1) {
      const rest = text.slice(p.open.n, e.from) + text.slice(e.to, p.close.s);
      if (!rest.trim()) return { from: 0, to: p.to, insert: "" };
    }
    return { from: e.from, to: e.to, insert: "" };
  }
  // add an empty property at the end of the block (a new block at the top when there is none)
  function add(text, key, type, value) {
    const name = String(key == null ? "" : key).trim();
    if (!name) return null;
    const p = parse(text);
    if (p && p.invalid) return null;
    const t = KNOWN.indexOf(type) >= 0 ? type : "text";
    if (!p) {
      const eol = /\r\n/.test(text.slice(0, (text.indexOf("\n") + 1) || text.length)) ? "\r\n" : "\n";
      return { from: 0, to: 0, insert: "---" + eol + emitEntry(emitKey(name), t, value, eol) + "---" + eol };
    }
    if (p.entries.some(x => x.key.toLowerCase() === name.toLowerCase())) return null;
    // after the last line before the closing fence; a last line without eol cannot exist (the fence follows it)
    return { from: p.close.s, to: p.close.s, insert: emitEntry(emitKey(name), t, value, p.eol) };
  }
  function apply(text, ch) { return ch ? text.slice(0, ch.from) + ch.insert + text.slice(ch.to) : text; }

  // a value typed into a widget -> the model value of that type (null = cleared; undefined = not valid)
  function coerce(type, s) {
    if (LISTY[type]) return Array.isArray(s) ? s : s == null || s === "" ? null : [String(s)];
    if (s === null || s === undefined || s === "") return null;
    if (type === "number") { const t = String(s).trim(); return /^-?(0|[1-9]\d*)(\.\d+)?$/.test(t) ? Number(t) : undefined; }
    if (type === "checkbox") return s === true || s === "true" ? true : s === false || s === "false" ? false : undefined;
    if (type === "date") return isDate(String(s)) ? String(s) : undefined;
    if (type === "datetime") { const t = String(s); return isDT(t) ? t : undefined; }
    return String(s);
  }

  root.PropsModel = { KNOWN, LISTY, RESERVED, parse, rows, typeOf, infer, fits, isDate, isDT, emitKey, emitStr, emitEntry,
                      setValue, rename, remove, add, apply, coerce, scalar };
})(typeof window !== "undefined" ? window : this);
