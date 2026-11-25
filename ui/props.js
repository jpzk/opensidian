// SPDX-License-Identifier: GPL-3.0-or-later
/* PROPERTIES (issue #119) — the note's frontmatter as stock's Properties block.

   Live Preview: the frontmatter lines ("---" .. "---") are ONE atomic block widget
   (Ed.propsField, in Ed.lpExt): a row per top-level key — type icon (click = the
   key menu: Property type / Rename / Delete), the key (an input: rename) and a value
   widget of the row's type (text, List pills, number, checkbox, date, date & time;
   tags / aliases / cssclasses are lists). "+ Add property" (and the command "Add file
   property", Ctrl+;) appends a row. Keys and values suggest what the vault already
   uses (Rust props_index). A row the model cannot rewrite faithfully (a nested map,
   a comment inside the value, a type this editor does not know, ...) shows its raw
   YAML read-only. A frontmatter the model calls INVALID gets no widget at all: the
   raw YAML stays in the editor, and nothing is written from it.

   EVERY write goes through ui/props-model.js: one CM6 change over the ONE entry it
   edits (the normal save path; undo works), so the bytes of every other key, every
   comment and the note body stay what they were.

   Settings > Editor > "Properties in document" (app.json propertiesInDocument):
   visible = the widget; hidden = the block is folded away entirely; source = the raw
   YAML. Source mode always shows the raw YAML. The reading view renders the same
   rows read-only (Props.readingBlock, main.js preview()) in visible mode only.

   Census (ui/census.js): [props:<where>|<mode>|<n rows>|key=type/e|r,...] the focused
   pane's painted block (where = lp | rv), [propsxy:...] the centres a driver clicks. */
(function () {
  "use strict";
  if (typeof window === "undefined" || typeof Ed === "undefined") return;   // (tests/props.rs loads this file bare: syntax only)
  const PM = () => window.PropsModel;
  const Props = window.Props = {
    mode: "visible",          // app.json propertiesInDocument (Props.load)
    types: {},                // .obsidian/types.json, lowercase name -> type
    gen: 0,                   // bumped on every mode / types change: every widget rebuilds
    idx: null, idxT: 0,       // props_index (the suggester), refreshed when older than 3 s
    fx: null,                 // the rebuild StateEffect (one for every editor)
    draft: null,              // {cm} the editor whose widget opens an "add property" row on its next build
    focus: null,              // {cm, key, part} the input to focus after the next build
  };
  const TYPE_LABEL = { text: "Text", multitext: "List", number: "Number", checkbox: "Checkbox", date: "Date",
                       datetime: "Date & time", tags: "Tags", aliases: "Aliases" };
  // the Property type menu: stock offers tags / aliases only on their reserved keys
  const MENU_TYPES = ["text", "multitext", "number", "checkbox", "date", "datetime"];
  const ICON = { text: "≡", multitext: "☰", number: "#", checkbox: "☑", date: "\u{1F4C5}",
                 datetime: "\u{1F552}", tags: "\u{1F3F7}", aliases: "↪" };
  const inv0 = (c, a) => (typeof inv === "function" ? inv(c, a) : Promise.reject(new Error("no backend")));
  const notice = m => { if (typeof say === "function") say(m); else console.warn(m); };
  const el = (tag, cls, txt) => { const e = document.createElement(tag); if (cls) e.className = cls; if (txt != null) e.textContent = txt; return e; };

  Props.load = async function () {
    let mode = "visible", types = {};
    try { mode = String(await inv0("props_mode")) || "visible"; } catch (_) {}
    try { types = (await inv0("props_types")) || {}; } catch (_) {}
    Props.mode = mode; Props.types = types;
    Props.refresh();
  };
  Props.setMode = async function (mode) {
    await inv0("set_props_mode", { mode });
    Props.mode = mode;
    Props.refresh();
  };
  Props.setType = async function (name, ty) {
    await inv0("set_prop_type", { name, ty });
    const t = Object.assign({}, Props.types);
    for (const k of Object.keys(t)) if (k === String(name).toLowerCase()) delete t[k];
    if (ty) t[String(name).toLowerCase()] = ty;
    Props.types = t;
    Props.refresh();
  };
  // every editor rebuilds its block (mode / types changed); reading views re-render
  Props.refresh = function () {
    Props.gen++;
    if (Props.fx && typeof CMv !== "undefined" && CMv && CMv.EditorView)
      for (const d of document.querySelectorAll(".cm-editor")) {
        const cm = CMv.EditorView.findFromDOM(d);
        if (cm) try { cm.dispatch({ effects: Props.fx.of(null) }); } catch (e) { console.error(e); }
      }
    if (typeof Props.onRefresh === "function") try { Props.onRefresh(); } catch (e) { console.error(e); }
    if (typeof updateTitle === "function") updateTitle();
  };
  async function index() {
    if (!Props.idx || Date.now() - Props.idxT > 3000) {
      try { Props.idx = await inv0("props_index"); } catch (_) { Props.idx = Props.idx || []; }
      Props.idxT = Date.now();
    }
    return Props.idx || [];
  }
  const statOf = (idx, key) => idx.find(p => p.name.toLowerCase() === String(key).toLowerCase());

  // ---- the edit path: a model change of the CURRENT doc, dispatched as one transaction ----
  function write(cm, ch, why) {
    if (!ch) { notice("Properties: " + why + " — the note was not changed"); return false; }
    if (ch.from === ch.to && !ch.insert) return true;
    cm.dispatch({ changes: ch, userEvent: "input.props" });
    return true;
  }
  const docText = cm => cm.state.doc.toString();
  function setValue(cm, key, type, value) {
    const M = PM();
    return write(cm, M.setValue(docText(cm), key, type, value, Props.types), "the value of " + key + " cannot be written here");
  }
  function rename(cm, key, name) {
    if (!String(name).trim() || name === key) return false;
    Props.focus = { cm, key: String(name).trim(), part: "value" };   // BEFORE the dispatch: CM6 redraws the widget synchronously in it
    const ok = write(cm, PM().rename(docText(cm), key, name), "a property named " + JSON.stringify(String(name).trim()) + " already exists");
    if (!ok) Props.focus = null;
    return ok;
  }
  function remove(cm, key) { return write(cm, PM().remove(docText(cm), key), "the property " + key + " cannot be deleted"); }
  function add(cm, key) {
    const k = String(key || "").trim();
    if (!k) return false;
    const st = Props.types[k.toLowerCase()], M = PM();
    const type = st && M.KNOWN.indexOf(st) >= 0 ? st : M.RESERVED[k.toLowerCase()] || "text";
    Props.focus = { cm, key: k, part: "value" };   // BEFORE the dispatch (see rename)
    const ok = write(cm, M.add(docText(cm), k, type, null), "a property named " + JSON.stringify(k) + " already exists");
    if (!ok) Props.focus = null;
    return ok;
  }
  // a retype: the value is converted when the conversion is exact, then types.json takes the type
  async function retype(cm, row, ty) {
    const M = PM();
    if (row.ok) {
      let v = row.value, nv;
      if (M.LISTY[ty]) nv = v == null ? null : Array.isArray(v) ? v.map(String) : [String(v)];
      else if (Array.isArray(v)) nv = v.length === 0 ? null : v.length === 1 ? M.coerce(ty, v[0]) : undefined;
      else nv = v == null ? null : M.coerce(ty, typeof v === "string" ? v : String(v));
      if (nv !== undefined && M.fits(ty, nv)) setValue(cm, row.key, ty, nv);
    }
    try { await Props.setType(row.key, ty); } catch (e) { notice("Properties: " + String(e && e.message || e)); }
  }

  // ---- suggester: a .suggestion-container under the input, own DOM (no native popup) ----
  let sugEl = null;
  function closeSug() { if (sugEl) { sugEl.remove(); sugEl = null; if (typeof updateTitle === "function") updateTitle(); } }
  function suggest(input, list, pick) {
    closeSug();
    const q = input.value.trim().toLowerCase();
    const items = list.filter(s => s && (!q || String(s).toLowerCase().includes(q)) && String(s) !== input.value).slice(0, 10);
    if (!items.length) return;
    const box = el("div", "suggestion-container props-suggest");
    items.forEach((s, i) => {
      const it = el("div", "suggestion-item" + (i === 0 ? " is-selected" : ""), String(s));
      it.addEventListener("mousedown", e => { e.preventDefault(); e.stopPropagation(); closeSug(); pick(String(s)); });
      box.appendChild(it);
    });
    const r = input.getBoundingClientRect();
    box.style.left = Math.round(r.left) + "px"; box.style.top = Math.round(r.bottom + 2) + "px";
    document.body.appendChild(box);
    sugEl = box; sugEl._input = input;
    if (typeof updateTitle === "function") updateTitle();
  }
  // keys in an input with an open suggester: arrows move, Enter/Tab picks (true = handled)
  function sugKey(e, input, pick) {
    if (!sugEl || sugEl._input !== input) return false;
    const its = [...sugEl.children], i = its.findIndex(x => x.classList.contains("is-selected"));
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const j = (i + (e.key === "ArrowDown" ? 1 : its.length - 1)) % its.length;
      its.forEach((x, k) => x.classList.toggle("is-selected", k === j));
      return true;
    }
    if ((e.key === "Enter" || e.key === "Tab") && i >= 0 && sugEl._nav) {
      e.preventDefault(); const s = its[i].textContent; closeSug(); pick(s); return true;
    }
    if (e.key === "Escape") { e.preventDefault(); closeSug(); return true; }
    return false;
  }
  // the app's global keymap must not see keys typed into a property input (Ctrl+A/C/V/X/Z stay the input's)
  function guardKeys(input) {
    input.addEventListener("keydown", e => {
      if (!(e.ctrlKey || e.metaKey) || /^[acvxyz]$/i.test(e.key)) e.stopPropagation();
      if (sugEl && sugEl._input === input && (e.key === "ArrowDown" || e.key === "ArrowUp")) sugEl._nav = true;
    });
    input.addEventListener("mousedown", e => e.stopPropagation());
  }

  // ---- the key menu (the type icon): Property type rows, Rename, Delete ----
  function keyMenu(cm, row, anchor, wrap) {
    if (typeof closeMenu === "function") closeMenu();
    const m = el("div", "ctxmenu props-menu"); m.dataset.menu = "props";
    const item = (txt, on) => { const it = el("div", null, txt); it.onmousedown = e => e.stopPropagation(); it.onclick = e => { e.stopPropagation(); if (typeof closeMenu === "function") closeMenu(); on(); }; m.appendChild(it); };
    const sep = () => m.appendChild(el("div", "sep"));
    const M = PM(), lc = row.key.toLowerCase();
    const types = M.RESERVED[lc] && M.RESERVED[lc] !== "multitext" ? [M.RESERVED[lc]] : MENU_TYPES;
    for (const t of types) item((t === row.type ? "✓ " : "") + TYPE_LABEL[t], () => { if (t !== row.type) retype(cm, row, t); });
    sep();
    item(I18N.m("Rename"), () => { const k = wrap.querySelector(".metadata-property-key-input"); if (k) { k.focus(); k.select(); } });
    item(I18N.m("Delete"), () => remove(cm, row.key));
    const r = anchor.getBoundingClientRect();
    if (typeof placeMenu === "function") placeMenu(m, Math.round(r.left), Math.round(r.bottom + 4));
    else { m.style.left = r.left + "px"; m.style.top = r.bottom + "px"; document.body.appendChild(m); }
  }

  // ---- value widgets ----
  function valueWidget(cm, row, ro, onSet) {
    // onSet(nv): the value sink (metadataTypeManager render: ctx.onChange); default = this note's entry
    const set = onSet || (nv => setValue(cm, row.key, row.type, nv));
    const M = PM(), box = el("div", "metadata-property-value");
    box.dataset.type = row.type;
    if (!row.ok) {
      box.classList.add("is-readonly");
      const raw = row.raw.slice(row.raw.indexOf(":") + 1).replace(/^[ \t]+/, "");
      box.appendChild(el("code", "props-raw", raw || "(empty)"));
      box.title = "Read-only: " + row.why + ". Edit it in Source mode.";
      return box;
    }
    const v = row.value;
    if (M.LISTY[row.type]) {
      const ms = el("div", "multi-select-container");
      for (const [i, x] of (v || []).entries()) {
        const pill = el("div", "multi-select-pill");
        pill.appendChild(el("span", "multi-select-pill-content", row.type === "tags" ? String(x).replace(/^#?/, "#") : String(x)));
        if (!ro) {
          const rm = el("span", "multi-select-pill-remove-button", "×");
          rm.addEventListener("mousedown", e => { e.preventDefault(); e.stopPropagation(); });
          rm.addEventListener("click", e => { e.stopPropagation(); const nv = v.slice(); nv.splice(i, 1); Props.focus = { cm, key: row.key, part: "value" }; set(nv.length ? nv : null); });
          pill.appendChild(rm);
        }
        ms.appendChild(pill);
      }
      if (!ro) {
        const inp = el("input", "multi-select-input");
        inp.type = "text"; inp.placeholder = (v || []).length ? "" : "Empty";
        guardKeys(inp);
        const push = s => {
          const t = String(s).trim().replace(row.type === "tags" ? /^#/ : /^$/, "");
          if (!t) return;
          const nv = (v || []).slice(); if (nv.indexOf(t) < 0) nv.push(t);
          Props.focus = { cm, key: row.key, part: "value" };
          set(nv);
        };
        const sugNow = async () => { const st = statOf(await index(), row.key); suggest(inp, st ? st.values.filter(x => (v || []).indexOf(x) < 0) : [], push); };
        inp.addEventListener("focus", sugNow);
        inp.addEventListener("input", sugNow);
        inp.addEventListener("keydown", e => {
          if (sugKey(e, inp, push)) return;
          if (e.key === "Enter" || e.key === ",") { e.preventDefault(); const s = inp.value; inp.value = ""; closeSug(); push(s); }
          else if (e.key === "Backspace" && !inp.value && (v || []).length) { e.preventDefault(); Props.focus = { cm, key: row.key, part: "value" }; const nv = v.slice(0, -1); set(nv.length ? nv : null); }
          else if (e.key === "Escape") { inp.value = ""; inp.blur(); }
        });
        inp.addEventListener("blur", () => { setTimeout(closeSug, 0); if (inp.value.trim()) { const s = inp.value; inp.value = ""; push(s); } });
        ms.appendChild(inp);
      }
      box.appendChild(ms);
      return box;
    }
    if (row.type === "checkbox") {
      const cb = el("input", "metadata-input-checkbox");
      cb.type = "checkbox"; cb.checked = v === true; cb.disabled = !!ro;
      cb.addEventListener("mousedown", e => e.stopPropagation());
      cb.addEventListener("change", () => set(cb.checked));
      box.appendChild(cb);
      return box;
    }
    const inp = el("input", "metadata-input metadata-input-" + (row.type === "number" ? "number" : row.type === "date" || row.type === "datetime" ? row.type : "text"));
    inp.type = row.type === "number" ? "number" : row.type === "date" ? "date" : row.type === "datetime" ? "datetime-local" : "text";
    inp.value = v == null ? "" : String(v);
    if (row.type === "text") inp.placeholder = "Empty";
    inp.readOnly = !!ro;
    if (ro) return box.appendChild(inp), box;
    guardKeys(inp);
    const commit = () => {
      const was = v == null ? "" : String(v);
      if (inp.value === was) return;
      const nv = M.coerce(row.type, inp.value);
      if (nv === undefined) { notice("Properties: " + JSON.stringify(inp.value) + " is not a " + TYPE_LABEL[row.type].toLowerCase()); inp.value = was; return; }
      set(nv);
    };
    if (row.type === "text") {
      const pick = s => { inp.value = s; commit(); };
      const sugNow = async () => { const st = statOf(await index(), row.key); suggest(inp, st ? st.values : [], pick); };
      inp.addEventListener("focus", sugNow);
      inp.addEventListener("input", sugNow);
      inp.addEventListener("keydown", e => { if (sugKey(e, inp, pick)) return; if (e.key === "Enter") { e.preventDefault(); closeSug(); commit(); inp.blur(); } else if (e.key === "Escape") { inp.value = v == null ? "" : String(v); inp.blur(); } });
    } else inp.addEventListener("keydown", e => { if (e.key === "Enter") { e.preventDefault(); commit(); inp.blur(); } else if (e.key === "Escape") { inp.value = v == null ? "" : String(v); inp.blur(); } });
    inp.addEventListener("change", commit);
    inp.addEventListener("blur", () => { setTimeout(closeSug, 0); commit(); });
    box.appendChild(inp);
    return box;
  }

  // one row: icon + key + value
  function rowEl(cm, row, ro) {
    const w = el("div", "metadata-property" + (row.ok ? "" : " is-readonly"));
    w.dataset.propertyKey = row.key; w.dataset.propertyType = row.type;
    const kc = el("div", "metadata-property-key");
    const ic = el("span", "metadata-property-icon", ICON[row.type] || "?");
    ic.title = TYPE_LABEL[row.type] || ("Unknown type: " + row.type);
    kc.appendChild(ic);
    const ki = el("input", "metadata-property-key-input");
    ki.type = "text"; ki.value = row.key; ki.readOnly = !!ro;
    kc.appendChild(ki);
    w.appendChild(kc);
    w.appendChild(valueWidget(cm, row, ro));
    if (ro) return w;
    ic.addEventListener("mousedown", e => { e.preventDefault(); e.stopPropagation(); });
    ic.addEventListener("click", e => { e.stopPropagation(); keyMenu(cm, row, ic, w); });
    guardKeys(ki);
    const done = () => { closeSug(); const n = ki.value.trim(); if (!n) { ki.value = row.key; return; } if (n !== row.key && !rename(cm, row.key, n)) ki.value = row.key; };
    ki.addEventListener("keydown", e => { if (e.key === "Enter") { e.preventDefault(); ki.blur(); } else if (e.key === "Escape") { ki.value = row.key; ki.blur(); } });
    ki.addEventListener("blur", done);
    return w;
  }

  // the draft "add property" row: a key input; Enter / blur with a name adds it, Escape drops it
  function draftRow(cm, list, rows) {
    const w = el("div", "metadata-property is-draft"), kc = el("div", "metadata-property-key");
    kc.appendChild(el("span", "metadata-property-icon", ICON.text));
    const ki = el("input", "metadata-property-key-input");
    ki.type = "text"; ki.placeholder = "Property name";
    kc.appendChild(ki); w.appendChild(kc); w.appendChild(el("div", "metadata-property-value"));
    guardKeys(ki);
    let gone = false;
    const drop = () => { if (gone) return; gone = true; closeSug(); w.remove(); if (typeof updateTitle === "function") updateTitle(); };
    const take = s => { if (gone) return; gone = true; closeSug(); if (!add(cm, s)) { gone = false; ki.select(); } };
    const have = new Set(rows.map(r => r.key.toLowerCase()));
    const sugNow = async () => suggest(ki, (await index()).map(p => p.name).filter(n => !have.has(n.toLowerCase())), take);
    ki.addEventListener("focus", sugNow);
    ki.addEventListener("input", sugNow);
    ki.addEventListener("keydown", e => {
      if (sugKey(e, ki, take)) return;
      if (e.key === "Enter") { e.preventDefault(); ki.value.trim() ? take(ki.value) : drop(); }
      else if (e.key === "Escape") { e.preventDefault(); drop(); }
    });
    ki.addEventListener("blur", () => setTimeout(() => { if (gone) return; ki.value.trim() ? take(ki.value) : drop(); }, 0));
    list.appendChild(w);
    setTimeout(() => { ki.focus(); if (typeof updateTitle === "function") updateTitle(); }, 0);
    if (typeof updateTitle === "function") updateTitle();
  }

  // the whole block. ro = the reading view (no inputs act, no add row)
  Props.block = function (cm, rows, ro) {
    const c = el("div", "metadata-container" + (ro ? " is-readonly" : ""));
    c.setAttribute("contenteditable", "false");
    c.appendChild(el("div", "metadata-properties-heading", "Properties"));
    const list = el("div", "metadata-properties");
    for (const r of rows) list.appendChild(rowEl(cm, r, ro));
    const mc = el("div", "metadata-content");
    mc.appendChild(list);
    if (!ro) {
      const ab = el("div", "metadata-add-button", "+ Add property");
      ab.addEventListener("mousedown", e => { e.preventDefault(); e.stopPropagation(); });
      ab.addEventListener("click", e => { e.stopPropagation(); if (!list.querySelector(".is-draft")) draftRow(cm, list, rows); });
      mc.appendChild(ab);
    }
    c.appendChild(mc);
    return c;
  };

  // ---- the CM6 field (Live Preview) ----
  Ed.propsField = function (v, G, bust) {
    const { Decoration, WidgetType, EditorView } = CMv;
    if (!Props.fx) Props.fx = CMs.StateEffect.define();
    class PB extends WidgetType {
      constructor(raw, gen, hidden) { super(); this.raw = raw; this.gen = gen; this.hidden = hidden; }
      eq(o) { return o.raw === this.raw && o.gen === this.gen && o.hidden === this.hidden; }
      get estimatedHeight() { return this.hidden ? 0 : 30 * (2 + this.raw.split("\n").length); }
      toDOM(view) {
        if (this.hidden) { const h = el("div", "cm-props-hidden"); h.style.height = "0"; return h; }
        const R = PM().rows(this.raw, Props.types);
        const w = el("div", "cm-embed-block cm-props-block");
        w.appendChild(Props.block(view, R && R.rows ? R.rows : [], false));
        const f = Props.focus, d = Props.draft;
        if (d && d.cm === view) { Props.draft = null; const mc = w.querySelector(".metadata-properties"); draftRow(view, mc, R && R.rows ? R.rows : []); }
        else if (f && f.cm === view) {
          Props.focus = null;
          setTimeout(() => {
            const r = [...w.querySelectorAll(".metadata-property")].find(x => x.dataset.propertyKey === f.key);
            const t = r && (f.part === "key" ? r.querySelector(".metadata-property-key-input") : r.querySelector(".metadata-property-value input"));
            if (t && !t.readOnly && !t.disabled) t.focus();
            if (typeof updateTitle === "function") updateTitle();   // the census ([propsxy:focus=]) sees the new focus
          }, 0);
        }
        return w;
      }
      ignoreEvent() { return true; }
    }
    const build = st => {
      const g = G();
      const tb = g && g.view === v && g.active >= 0 ? g.tabs[g.active] : null;
      if (!g || (v.lp && v.lp.classList.contains("src")) || (tb && tb.mode === "reading") || Props.mode === "source") return Decoration.none;
      const text = st.doc.toString(), p = PM().parse(text);
      if (!p || p.invalid) return Decoration.none;
      const end = st.doc.lineAt(p.close.s).to;
      return Decoration.set([Decoration.replace({ widget: new PB(text.slice(0, p.to), Props.gen, Props.mode === "hidden"), block: true }).range(0, end)]);
    };
    const field = CMs.StateField.define({
      create: st => { try { return build(st); } catch (e) { console.error(e); return Decoration.none; } },
      update(d, tr) {
        const fx = tr.effects.some(e => e.is(bust) || e.is(Props.fx));
        if (!fx && tr.docChanged) {
          // a body edit cannot change the block: map it instead of re-reading the whole note per keystroke.
          // Re-read when the change touches the block's range, or (no block yet) the first line / a doc whose
          // first line opens a block (a typed closing "---" further down makes one).
          let end = -1; d.between(0, 0, (f, t) => { end = t; });
          const st = tr.state, first = st.doc.line(1).text.trim();
          const body = end >= 0 ? !tr.changes.touchesRange(0, end) : !tr.changes.touchesRange(0, tr.startState.doc.line(1).to) && first !== "---";
          if (body) return d.map(tr.changes);
        }
        if (tr.docChanged || fx)
          try { return build(tr.state); } catch (e) { console.error(e); return Decoration.none; }
        return d;
      },
      provide: f => [EditorView.decorations.from(f), EditorView.atomicRanges.of(view => view.state.field(f))],
    });
    v._propsF = field;   // Ed.scrollToLine asks whether a Properties block sits above its destination
    return [field, Props.rawRows(v, bust)];
  };

  // #341: when the frontmatter is shown RAW (source mode, or Properties in document = source) its lines
  // carry the line class "fmrow" (style.css: muted ink, 14px at a 16px editor, like stock's raw frontmatter).
  // Re-read only when an edit touches the block (or the first line, which can open one) or a transaction
  // carries the lp bust / Props.fx (properties-mode change), or the raw mode flipped — never per keystroke in the body.
  Props.rawRows = function (v, bust) {
    const { Decoration, ViewPlugin } = CMv;
    if (!ViewPlugin) return [];
    const line = Decoration.line({ class: "fmrow" });
    const raw = () => { return !!(v.lp && v.lp.classList.contains("src")) || Props.mode === "source"; };
    const build = st => {
      const r = raw();
      if (!r) return { d: Decoration.none, end: -1, r };
      if (st.doc.line(1).text.trim() !== "---") return { d: Decoration.none, end: -1, r };
      const p = PM().parse(st.doc.toString());
      if (!p || p.invalid) return { d: Decoration.none, end: -1, r };
      const last = st.doc.lineAt(p.close.s).number, out = [];
      for (let n = 1; n <= last; n++) out.push(line.range(st.doc.line(n).from));
      return { d: Decoration.set(out), end: st.doc.line(last).to, r };
    };
    return ViewPlugin.fromClass(class {
      constructor(view) { this.s = build(view.state); this.decorations = this.s.d; }
      update(u) {
        const fx = u.transactions.some(t => t.effects.some(e => e.is(bust) || e.is(Props.fx)));
        const touch = u.docChanged && u.changes.touchesRange(0, Math.max(this.s.end, u.startState.doc.line(1).to));
        if (fx || touch || raw() !== this.s.r) this.s = build(u.state);   // also the first update after a mode flip
        else if (u.docChanged) this.s = { d: this.s.d.map(u.changes), end: this.s.end < 0 ? -1 : u.changes.mapPos(this.s.end), r: this.s.r };
        this.decorations = this.s.d;
      }
    }, { decorations: p => p.decorations });
  };

  // ---- "Add file property" (Ctrl+;, the tab menu row) ----
  Props.addCommand = function (g) {
    const cm = typeof Ed !== "undefined" ? Ed.cm(g) : null;
    if (!cm) { notice("Add file property: no note is open in an editor"); return; }
    const text = docText(cm), M = PM(), p = M.parse(text);
    if (p && p.invalid) { notice("Add file property: the properties of this note are not valid YAML (" + p.invalid + ") — fix them in Source mode"); return; }
    const src = g.view && g.view.lp && g.view.lp.classList.contains("src");
    if (src || Props.mode !== "visible") {
      // raw YAML: an empty line inside the block (a new block when there is none), caret on it
      const eol = p ? p.eol : "\n";
      const ch = p ? { from: p.close.s, to: p.close.s, insert: eol } : { from: 0, to: 0, insert: "---" + eol + eol + "---" + eol };
      cm.dispatch({ changes: ch, selection: { anchor: p ? p.close.s : 3 + eol.length }, userEvent: "input.props" });
      cm.focus();
      return;
    }
    Props.draft = { cm };
    if (!p) cm.dispatch({ changes: { from: 0, to: 0, insert: "---\n---\n" }, userEvent: "input.props" });
    else { Props.gen++; cm.dispatch({ effects: Props.fx.of(null) }); }   // a new gen: the widget is drawn again (eq() false), with the draft row
  };

  // ---- the reading view: the same rows, read-only, above the rendered body ----
  Props.readingBlock = function (src) {
    if (Props.mode !== "visible") return null;
    const R = PM().rows(src, Props.types);
    if (!R || R.invalid || !R.rows.length) return null;
    const w = el("div", "props-reading");
    w.appendChild(Props.block(null, R.rows, true));
    return w;
  };

  // ---- census ----
  const safe = s => String(s == null ? "" : s).replace(/[\[\]|,=\s:@]+/g, "_").slice(0, 24);
  const ctr = e => { const r = e.getBoundingClientRect(); return Math.round(r.left + r.width / 2) + "," + Math.round(r.top + r.height / 2); };
  Props.tok = function () {
    const P = document.querySelector("#main .pane.focused") || document.querySelector("#main .pane");
    if (!P) return "";
    const vis = e => !!e && !e.closest(".vkeep") && e.getClientRects().length > 0;
    const c = [...P.querySelectorAll(".metadata-container")].find(vis);
    let out = " [propsmode:" + Props.mode + "]";
    // [propsfm:widget|hidden|raw|none] how the focused editor paints its frontmatter; [spd:<label>,x,y] the
    // Settings > Editor "Properties in document" dropdown (its centre), while the settings pane shows it
    const ed = [...P.querySelectorAll(".cm-editor")].find(vis);
    if (ed) { const l0 = ed.querySelector(".cm-content > .cm-line"); out += " [propsfm:" + (ed.querySelector(".cm-props-block") ? "widget" : ed.querySelector(".cm-props-hidden") ? "hidden" : l0 && l0.textContent === "---" ? "raw" : "none") + "]"; }
    const sd = document.getElementById("spropsdoc");
    if (sd && sd.getClientRects().length) out += " [spd:" + safe(sd.textContent) + "," + ctr(sd) + "]";
    if (sugEl) out += " [propsug:" + [...sugEl.children].map(x => safe(x.textContent) + (x.classList.contains("is-selected") ? "*" : "")).join("|") + "]";
    if (!c) return out;
    const where = c.closest(".cm-editor") ? "lp" : "rv";
    const rows = [...c.querySelectorAll(".metadata-property:not(.is-draft)")];
    out += " [props:" + where + "|" + rows.length + "|" + rows.map(r => {
      const b = r.querySelector(".metadata-property-value");
      let val = "";
      const cb = b && b.querySelector("input[type=checkbox]"), pills = b ? [...b.querySelectorAll(".multi-select-pill-content")] : [];
      if (cb) val = cb.checked ? "true" : "false";
      else if (pills.length) val = pills.map(x => safe(x.textContent)).join("+");
      else if (b && b.querySelector("input")) val = safe(b.querySelector("input").value);
      else if (b && b.querySelector("code")) val = "raw";
      return safe(r.dataset.propertyKey) + "=" + r.dataset.propertyType + "/" + (r.classList.contains("is-readonly") ? "r" : "e") + "/" + val;
    }).join(",") + "]";
    const xy = [];
    const ab = c.querySelector(".metadata-add-button");
    if (ab && vis(ab)) xy.push("add=" + ctr(ab));
    for (const r of rows) {
      const k = safe(r.dataset.propertyKey), ic = r.querySelector(".metadata-property-icon");
      const vi = r.querySelector(".metadata-property-value input, .metadata-property-value code");
      if (ic && vis(ic)) xy.push("i:" + k + "=" + ctr(ic));
      if (vi && vis(vi)) xy.push("v:" + k + "=" + ctr(vi));
    }
    const d = c.querySelector(".is-draft .metadata-property-key-input");
    if (d && vis(d)) xy.push("draft=" + ctr(d) + (document.activeElement === d ? "*" : ""));
    const a = document.activeElement;
    if (a && c.contains(a)) { const r = a.closest(".metadata-property"); xy.push("focus=" + (r ? safe(r.dataset.propertyKey || "draft") : "-") + "/" + (a.classList.contains("metadata-property-key-input") ? "key" : "value")); }
    return out + " [propsxy:" + xy.join("|") + "]";
  };
  // ---- app.metadataTypeManager widgets: render(containerEl, value, ctx) builds the value widget
  // of the in-document block; edits go to ctx.onChange(value) (the plugin owns the write) ----
  const API_TYPE = { file: "text", folder: "text", property: "text" };
  Props.renderWidget = function (type, host, value, ctx) {
    const M = PM(), c = ctx || {};
    const ty = M.KNOWN.indexOf(type) >= 0 ? type : API_TYPE[type] || null;
    let cur = value, node = null;
    const draw = () => {
      if (node) node.remove();
      const ok = ty != null && (cur == null || M.fits(ty, M.LISTY[ty] && typeof cur === "string" ? [cur] : cur));
      const v = ok && M.LISTY[ty] && typeof cur === "string" ? [cur] : cur;
      const row = { key: String(c.key == null ? "" : c.key), type: ty || "unknown", ok, value: ok ? v : undefined,
                    why: ty == null ? "unknown type " + type : "the value is not a " + ty,
                    raw: ": " + (cur === undefined ? "" : JSON.stringify(cur)) };
      node = valueWidget(null, row, false, nv => {
        if (Props.focus && Props.focus.cm == null) Props.focus = null;
        cur = nv; draw();
        if (typeof c.onChange === "function") c.onChange(nv);
      });
      host.appendChild(node);
    };
    draw();
    return { type, focus() { const i = node && node.querySelector("input"); if (i) i.focus(); } };
  };

  // ---- the Properties view (core plugin, right sidebar): every vault property + its note count ----
  Props.viewSort = "name";         // name | freq (the header toggle)
  Props.viewInfo = "";             // census [rp:props:<n>|<sort>]
  const guessType = p => {
    const lc = p.name.toLowerCase(), M = PM();
    if (Props.types[lc]) return Props.types[lc];
    if (M.RESERVED[lc]) return M.RESERVED[lc];
    const vs = p.values || [];
    if (!vs.length) return "text";
    if (vs.every(x => /^-?\d+(\.\d+)?$/.test(x))) return "number";
    if (vs.every(x => /^(true|false)$/i.test(x))) return "checkbox";
    if (vs.every(x => M.isDate(x))) return "date";
    if (vs.every(x => M.isDT(x))) return "datetime";
    return "text";
  };
  const qKey = k => (/^[\w-]+$/.test(k) ? k : JSON.stringify(k));
  // hooks: {search(query), renameAll(old, new) -> Promise, refresh()}
  Props.renderView = async function (box, head, hooks) {
    let idx = [];
    try { idx = (await inv0("props_index")) || []; Props.idx = idx; Props.idxT = Date.now(); } catch (_) {}
    box.textContent = ""; head.textContent = "";
    const byName = (a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase());
    const list = idx.slice().sort(Props.viewSort === "freq" ? (a, b) => b.count - a.count || byName(a, b) : byName);
    head.appendChild(document.createTextNode("All properties"));
    head.appendChild(el("span", "scount", String(list.length)));
    const sb = el("button", "props-sort", Props.viewSort === "freq" ? "Frequency" : "Name");
    sb.id = "propsort"; sb.title = "Sort by name / frequency";
    sb.onclick = () => { Props.viewSort = Props.viewSort === "freq" ? "name" : "freq"; hooks.refresh(); };
    head.appendChild(sb);
    Props.viewInfo = "props:" + list.length + "|" + Props.viewSort;
    if (!list.length) { const d = el("div", "rempty", "No properties found."); box.appendChild(d); return; }
    for (const p of list) {
      const ty = guessType(p);
      const row = el("div", "props-vrow");
      row.dataset.key = p.name; row.dataset.type = ty; row.title = p.name;
      const ic = el("span", "metadata-property-icon", ICON[ty] || "?"); ic.title = TYPE_LABEL[ty] || ty;
      row.appendChild(ic);
      const nm = el("span", "props-vname", p.name);
      row.appendChild(nm);
      row.appendChild(el("span", "scount", String(p.count)));
      row.onclick = () => { if (!row.querySelector("input")) hooks.search("[" + qKey(p.name) + ":]"); };
      row.oncontextmenu = e => { e.preventDefault(); e.stopPropagation(); viewMenu(p, ty, row, nm, e, hooks); };
      box.appendChild(row);
    }
  };
  function viewMenu(p, ty, row, nm, ev, hooks) {
    if (typeof closeMenu === "function") closeMenu();
    const m = el("div", "ctxmenu props-menu"); m.dataset.menu = "propsview";
    const item = (txt, on) => { const it = el("div", null, txt); it.onmousedown = e => e.stopPropagation(); it.onclick = e => { e.stopPropagation(); if (typeof closeMenu === "function") closeMenu(); on(); }; m.appendChild(it); };
    const M = PM(), lc = p.name.toLowerCase();
    const types = M.RESERVED[lc] && M.RESERVED[lc] !== "multitext" ? [M.RESERVED[lc]] : MENU_TYPES;
    for (const t of types) item((t === ty ? "✓ " : "") + TYPE_LABEL[t], async () => {
      if (t === ty) return;
      try { await Props.setType(p.name, t); } catch (e) { notice("Properties: " + String(e && e.message || e)); }
      hooks.refresh();
    });
    m.appendChild(el("div", "sep"));
    item(I18N.m("Rename"), () => {
      const ki = el("input", "props-vrename"); ki.type = "text"; ki.value = p.name;
      nm.replaceWith(ki); ki.focus(); ki.select();
      let gone = false;
      const end = async ok => {
        if (gone) return; gone = true;
        const n = ki.value.trim();
        ki.replaceWith(nm);
        if (ok && n && n !== p.name) { await hooks.renameAll(p.name, n); hooks.refresh(); }
      };
      ki.addEventListener("keydown", e => { e.stopPropagation(); if (e.key === "Enter") { e.preventDefault(); end(true); } else if (e.key === "Escape") { e.preventDefault(); end(false); } });
      ki.addEventListener("blur", () => end(true));
      ki.addEventListener("mousedown", e => e.stopPropagation());
      ki.addEventListener("click", e => e.stopPropagation());
    });
    if (typeof placeMenu === "function") placeMenu(m, ev.clientX, ev.clientY);
    else { m.style.left = ev.clientX + "px"; m.style.top = ev.clientY + "px"; document.body.appendChild(m); }
  }
  // census [pview:<key>=<type>/<count>@x,y|...] the rows of a showing Properties view
  Props.viewTok = function () {
    const b = document.getElementById("propslist");
    if (!b || !b.getClientRects().length) return "";
    const rows = [...b.querySelectorAll(".props-vrow")];
    const sb = document.getElementById("propsort");
    return " [pview:" + (sb ? "sort=" + ctr(sb) + "|" : "") + rows.map(r => safe(r.dataset.key) + "=" + r.dataset.type + "/" + r.querySelector(".scount").textContent + "@" + ctr(r)).join("|") + "]";
  };

})();
