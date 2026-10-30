// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* editor-api (recon T-c): Obsidian's Editor / MarkdownView / registerEditorExtension /
   EditorSuggest over the CM6 EditorView each note tab already owns (ui/editor.js Ed.mount).
   Loaded by index.html AFTER editor.js and BEFORE api.js. It defines ONE installer,
   window.__osEditorApi(ctx), which api.js calls once while it builds the API object and
   then deletes — no plugin ever sees it. Everything here works on the shared
   window.__osModules instances, so a plugin's ViewPlugin / StateField / Decoration is
   the same class the editor runs.

   Positions: Obsidian's EditorPosition is {line, ch}, both 0-based, ch in UTF-16 code
   units (CM6's own unit), lines split on \n, \r\n and \r alike (CM6 normalises; getValue
   joins with \n). Offsets are clamped to the document like CM6 does.

   Extensions: every view carries ONE Compartment (Ed.PX) whose content is
   Ed.pxExt(view): nothing until a plugin is loaded (restricted mode and an empty
   plugin set pay nothing per keystroke), then the info fields + the editor-change /
   paste / drop / suggest hooks + every registered extension. register/unregister
   reconfigures every open view; a view made later (new tab, setText's fresh state)
   reads Ed.pxExt when it builds its state. */
(function () {
  "use strict";
  const M = window.__osModules || {};
  const S = M["@codemirror/state"], V = M["@codemirror/view"], C = M["@codemirror/commands"];
  const L = M["@codemirror/language"], LM = M["@codemirror/lang-markdown"];

  function install(ctx) {
    const EdG = typeof Ed !== "undefined" ? Ed : null;
    if (!S || !V || !EdG) return null;
    // ED-3: every doc change a plugin makes through the Editor API is marked, so editor.js can group its undo steps as stock does
    const PE = () => EdG.PLUGIN_EDIT ? [EdG.PLUGIN_EDIT.of(true)] : [];
    const app = () => ctx.app(), ws = () => ctx.app().workspace;

    /* ---------------------------------------------------------------- Editor */
    const clamp = (x, a, b) => Math.max(a, Math.min(b, x));
    class Editor {
      constructor(v) { Object.defineProperty(this, "_v", { value: v }); }
      get cm() { return this._v.cm; }
      get _d() { return this._v.cm.state.doc; }
      getDoc() { return this; }
      refresh() { this.cm.requestMeasure(); }
      getValue() { return this._d.toString(); }
      setValue(s) { this.cm.dispatch({ changes: { from: 0, to: this._d.length, insert: String(s == null ? "" : s) } }); }
      getLine(n) { const d = this._d; return n >= 0 && n < d.lines ? d.line(n + 1).text : ""; }
      setLine(n, text) { const d = this._d; if (n < 0 || n >= d.lines) return; const l = d.line(n + 1); this.cm.dispatch({ changes: { from: l.from, to: l.to, insert: String(text) } }); }
      lineCount() { return this._d.lines; }
      firstLine() { return 0; }
      lastLine() { return this._d.lines - 1; }
      posToOffset(p) {
        const d = this._d; if (!p) return 0;
        if (p.line < 0) return 0;
        if (p.line + 1 > d.lines) return d.length;
        const l = d.line((p.line | 0) + 1);
        return l.from + clamp(p.ch | 0, 0, l.length);
      }
      offsetToPos(o) { const d = this._d, x = clamp(o | 0, 0, d.length), l = d.lineAt(x); return { line: l.number - 1, ch: x - l.from }; }
      getCursor(which) {
        const r = this.cm.state.selection.main;
        const o = which === "from" ? r.from : which === "to" ? r.to : which === "anchor" ? r.anchor : r.head;
        return this.offsetToPos(o);
      }
      setCursor(pos, ch) {
        const p = typeof pos === "number" ? { line: pos, ch: ch == null ? 0 : ch } : pos;
        this.cm.dispatch({ selection: { anchor: this.posToOffset(p) }, scrollIntoView: true });
      }
      getSelection() { const r = this.cm.state.selection.main; return this.cm.state.sliceDoc(r.from, r.to); }
      setSelection(anchor, head) {
        const a = this.posToOffset(anchor), h = head ? this.posToOffset(head) : a;
        this.cm.dispatch({ selection: { anchor: a, head: h }, scrollIntoView: true });
      }
      listSelections() { return this.cm.state.selection.ranges.map(r => ({ anchor: this.offsetToPos(r.anchor), head: this.offsetToPos(r.head) })); }
      setSelections(ranges, main) {
        if (!ranges || !ranges.length) return;
        const rs = ranges.map(r => S.EditorSelection.range(this.posToOffset(r.anchor), this.posToOffset(r.head || r.anchor)));
        this.cm.dispatch({ selection: S.EditorSelection.create(rs, clamp(main | 0, 0, rs.length - 1)) });
      }
      somethingSelected() { return this.cm.state.selection.ranges.some(r => !r.empty); }
      replaceSelection(text, origin) {
        const tr = this.cm.state.replaceSelection(String(text == null ? "" : text));
        this.cm.dispatch(tr, { userEvent: origin ? String(origin) : "input", annotations: PE() });
      }
      replaceRange(text, from, to, origin) {
        const a = this.posToOffset(from), b = to ? this.posToOffset(to) : a;
        this.cm.dispatch({ changes: { from: Math.min(a, b), to: Math.max(a, b), insert: String(text == null ? "" : text) }, userEvent: origin ? String(origin) : undefined, annotations: PE() });
      }
      getRange(from, to) { const a = this.posToOffset(from), b = this.posToOffset(to); return this.cm.state.sliceDoc(Math.min(a, b), Math.max(a, b)); }
      wordAt(pos) { const w = this.cm.state.wordAt(this.posToOffset(pos)); return w ? { from: this.offsetToPos(w.from), to: this.offsetToPos(w.to) } : null; }
      focus() { this.cm.focus(); }
      blur() { this.cm.contentDOM.blur(); }
      hasFocus() { return this.cm.hasFocus; }
      getScrollInfo() { const s = this._v.lp || this.cm.scrollDOM; return { top: s.scrollTop, left: s.scrollLeft, height: s.scrollHeight, width: s.scrollWidth, clientHeight: s.clientHeight, clientWidth: s.clientWidth }; }
      scrollTo(x, y) { const s = this._v.lp || this.cm.scrollDOM; if (x != null) s.scrollLeft = x; if (y != null) s.scrollTop = y; }
      scrollIntoView(range, center) {
        const r = range && range.from ? range : { from: this.getCursor("from"), to: this.getCursor("to") };
        const a = this.posToOffset(r.from), b = this.posToOffset(r.to || r.from);
        this.cm.dispatch({ effects: V.EditorView.scrollIntoView(S.EditorSelection.range(a, b), { y: center ? "center" : "nearest" }) });
      }
      /* EditorTransaction: {replaceSelection?, changes?: EditorChange[], selection?, selections?} -> ONE dispatch */
      transaction(tx, origin) {
        if (!tx) return;
        const ue = origin ? String(origin) : undefined;
        const sels = tx.selections || (tx.selection ? [tx.selection] : null);
        // selections are read in the coordinates of the doc they apply to: AFTER the changes
        const selOf = () => S.EditorSelection.create(sels.map(s => S.EditorSelection.range(this.posToOffset(s.from), this.posToOffset(s.to || s.from))), 0);
        if (tx.changes && tx.changes.length) {
          const ch = tx.changes.map(c => { const a = this.posToOffset(c.from), b = c.to ? this.posToOffset(c.to) : a; return { from: Math.min(a, b), to: Math.max(a, b), insert: String(c.text == null ? "" : c.text) }; });
          this.cm.dispatch({ changes: ch, userEvent: ue, annotations: PE() });
        }
        if (sels && sels.length) this.cm.dispatch({ selection: selOf() });
        if (tx.replaceSelection != null) this.replaceSelection(tx.replaceSelection, origin);   // replaces the selections just set (or the current ones)
      }
      undo() { if (C) C.undo(this.cm); }
      redo() { if (C) C.redo(this.cm); }
      exec(cmd) {
        const map = {
          goUp: "cursorLineUp", goDown: "cursorLineDown", goLeft: "cursorCharLeft", goRight: "cursorCharRight",
          goStart: "cursorDocStart", goEnd: "cursorDocEnd", goWordLeft: "cursorGroupLeft", goWordRight: "cursorGroupRight",
          indentMore: "indentMore", indentLess: "indentLess", newlineAndIndent: "insertNewlineAndIndent",
          swapLineUp: "moveLineUp", swapLineDown: "moveLineDown", deleteLine: "deleteLine",
          undo: "undo", redo: "redo", selectAll: "selectAll",
        };
        const f = C && C[map[cmd]];
        if (typeof f === "function") f(this.cm);
      }
      /* the lines the selections touch (each line once, top to bottom; a caret is its own line):
         read(line, text) -> state, then write(line, text, state) -> EditorChange | void, all
         changes in ONE dispatch against the unchanged doc (oracle: stock 1.13.7 visits only the
         selected line, not the whole document) */
      processLines(read, write, ignoreEmpty) {
        const d = this._d, ch = [], seen = new Set();
        for (const r of this.cm.state.selection.ranges) for (let n = d.lineAt(r.from).number; n <= d.lineAt(r.to).number; n++) seen.add(n);
        for (const n of [...seen].sort((a, b) => a - b)) {
          const i = n - 1, l = d.line(n); if (ignoreEmpty && !l.text) continue;
          const st = read(i, l.text), r = write(i, l.text, st);
          if (r) ch.push({ from: this.posToOffset(r.from), to: r.to ? this.posToOffset(r.to) : this.posToOffset(r.from), insert: String(r.text) });
        }
        if (ch.length) this.cm.dispatch({ changes: ch });
      }
    }
    const edOf = new WeakMap();   // tab view -> Editor
    const editorFor = v => { if (!v || !v.cm) return null; let e = edOf.get(v); if (!e) { e = new Editor(v); edOf.set(v, e); } return e; };

    /* a field declared on the PROTOTYPE (so `name in Cls.prototype` holds, as obsidian.d.ts
       declares it) that becomes an ordinary own data property the first time it is assigned;
       until then reads give `def` */
    const protoField = (C, name, def) => Object.defineProperty(C.prototype, name, {
      configurable: true, enumerable: false,
      get() { return def; },
      set(x) { Object.defineProperty(this, name, { value: x, writable: true, enumerable: true, configurable: true }); },
    });
    const tabOf = v => (v && v.tab && !v.tab.kind ? v.tab : null);
    const scrollerOf = v => (v && (v.lp || (v.cm && v.cm.scrollDOM))) || null;
    const isActiveView = v => !!(v && v.g && v.g.view === v);

    /* ------------------------------------------------------ MarkdownEditView
       The edit side of one MarkdownView (view.editMode): text in and out of that view's
       CM6 editor. Scroll positions are in LINES (fractional), the unit Obsidian's ephemeral
       state uses: 0 = the first line at the top. */
    class MarkdownEditView {
      constructor(view) {
        Object.defineProperty(this, "view", { value: view, enumerable: false });
        this.app = (view && view.app) || ctx.app();
      }
      get file() { return this.view ? this.view.file : null; }
      get editor() { return this.view ? this.view.editor : null; }
      get sourceMode() { const v = this.view && this.view._tv; return !!(v && v.lp && v.lp.classList.contains("src")); }
      get() { const e = this.editor; return e ? e.getValue() : ""; }
      set(data, clear) { const v = this.view && this.view._tv; setText(v, data, clear); }
      clear() { const v = this.view && this.view._tv; setText(v, "", true); }
      getSelection() { const e = this.editor; return e ? e.getSelection() : ""; }
      getScroll() {
        const v = this.view && this.view._tv, s = scrollerOf(v), cm = v && v.cm;
        if (!s || !cm || s.scrollTop <= 0) return 0;
        const top = cm.documentTop - s.getBoundingClientRect().top + s.scrollTop, h = s.scrollTop - top;
        if (h <= 0) return 0;
        const b = cm.lineBlockAtHeight(h), n = cm.state.doc.lineAt(b.from).number - 1;
        return Math.round((n + (b.height > 0 ? Math.min(1, (h - b.top) / b.height) : 0)) * 1000) / 1000;
      }
      applyScroll(scroll) {
        const v = this.view && this.view._tv, s = scrollerOf(v), cm = v && v.cm;
        const y = Number(scroll); if (!s || !cm || !Number.isFinite(y)) return;
        const d = cm.state.doc, n = clamp(Math.floor(y), 0, d.lines - 1);
        const b = cm.lineBlockAt(d.line(n + 1).from);
        const top = cm.documentTop - s.getBoundingClientRect().top + s.scrollTop;
        s.scrollTop = y <= 0 ? 0 : top + b.top + (y - n) * b.height;
      }
      getFoldInfo() { return foldInfo(this.view && this.view._tv); }
      applyFoldInfo(info) { applyFolds(this.view && this.view._tv, info); }
    }
    protoField(MarkdownEditView, "app", null);
    protoField(MarkdownEditView, "hoverPopover", undefined);   // oracle: undefined until a hover popover is shown
    /* FoldInfo {folds:[{from,to}], lines}: the CM6 folded ranges of the note's editor as 0-based
       line numbers (Templater's jump-to-cursor reads it, drops the folds over the jump target and
       hands the rest back). The note editor folds nothing through CM6 today, so folds is [] — the
       real state, not a stand-in; applyFoldInfo re-folds exactly the lines it is given. */
    function foldInfo(v) {
      const cm = v && v.cm; if (!cm) return null;
      const d = cm.state.doc, folds = [];
      if (L && typeof L.foldedRanges === "function")
        L.foldedRanges(cm.state).between(0, d.length, (from, to) => { folds.push({ from: d.lineAt(from).number - 1, to: d.lineAt(to).number - 1 }); });
      return { folds, lines: d.lines };
    }
    function applyFolds(v, info) {
      const cm = v && v.cm; if (!cm || !info || !Array.isArray(info.folds) || !L || !L.foldEffect) return;
      const d = cm.state.doc, fx = [];
      for (const f of info.folds) {
        const a = Number(f && f.from), b = Number(f && f.to);
        if (!Number.isInteger(a) || !Number.isInteger(b) || a < 0 || b < a || b >= d.lines) continue;
        const from = d.line(a + 1).to, to = d.line(b + 1).to;
        if (to > from) fx.push(L.foldEffect.of({ from, to }));
      }
      if (!fx.length) return;
      if (cm.state.field(L.foldState, false) === undefined && typeof L.codeFolding === "function") fx.push(S.StateEffect.appendConfig.of(L.codeFolding()));
      cm.dispatch({ effects: fx });
    }
    // load / replace the text of one tab view: the shown view takes a fresh state (Ed.setText:
    // new history, rebuilds the plugin compartment) when `clear`, otherwise an ordinary edit
    function setText(v, data, clear) {
      if (!v || !v.cm) return;
      const s = String(data == null ? "" : data);
      if (clear && isActiveView(v)) Ed.setText(v.g, s);
      else { const e = editorFor(v); if (e) e.setValue(s); }
    }

    /* --------------------------------------------------- MarkdownPreviewView
       The reading side (view.previewMode): containerEl is that tab's reading pane, which
       main.js preview() renders from the editor text; set() goes through the editor so the
       two never disagree; rerender() re-renders a shown reading pane, otherwise it drops
       the retained DOM so the next show renders fresh. getScroll() is null until the pane
       has been rendered (oracle). */
    const MR = ctx.MarkdownRenderer || class { constructor(el) { this.containerEl = el; } };
    class MarkdownPreviewView extends MR {
      constructor(view) {
        super(null);
        Object.defineProperty(this, "view", { value: view, enumerable: false });
        const tv = () => (view ? view._tv : null);
        Object.defineProperty(this, "containerEl", { configurable: true, enumerable: true,
          get() { const v = tv(); return v ? v.preview || (v.g && v.g.preview) || null : null; }, set() {} });
        this.app = (view && view.app) || ctx.app();
        this.hoverPopover = null;
      }
      get file() { return this.view ? this.view.file : null; }
      get() { const e = this.view && this.view.editor; return e ? e.getValue() : ""; }
      set(data, clear) { setText(this.view && this.view._tv, data, clear); this.rerender(false); }
      clear() { const P = this.containerEl; if (!P) return; P._pvk = null; P.textContent = ""; }
      rerender(full) {
        const v = this.view && this.view._tv, P = this.containerEl, t = tabOf(v);
        if (!P) return;
        if (full) P._pvk = null;
        if (t && t.mode === "reading" && isActiveView(v) && typeof preview === "function") {
          try { const r = preview(v.g); if (r && typeof r.catch === "function") r.catch(e => console.error("[plugin] rerender:", e)); } catch (e) { console.error("[plugin] rerender:", e); }
        } else P._pvk = null;
      }
      getScroll() { const P = this.containerEl; return P && P._pvk ? P.scrollTop : null; }
      applyScroll(y) { const P = this.containerEl, n = Number(y); if (P && Number.isFinite(n)) P.scrollTop = n; }
      getFoldInfo() { return foldInfo(this.view && this.view._tv); }
      applyFoldInfo(info) { applyFolds(this.view && this.view._tv, info); }
    }

    /* ---------------------------------------------------------- MarkdownView
       One per tab view (v): .editor drives v.cm, .file is that tab's note, .editMode /
       .previewMode are its two (stable) sub-views. Extends api.js's TextFileView. */
    const Base = ctx.TextFileView || ctx.FileView || class { constructor(leaf) { this.leaf = leaf; } };
    class MarkdownView extends Base {
      constructor(leaf, v) {
        super(leaf);
        Object.defineProperty(this, "_tv", { value: v || null, writable: true });
        if (!this.app) this.app = ctx.app();
        if (v) {
          const self = this;
          Object.defineProperty(this, "file", { configurable: true, enumerable: true, get() { const t = tabOf(self._tv); return t ? ctx.fileOf(t.name) : null; }, set() {} });
          Object.defineProperty(this, "containerEl", { configurable: true, enumerable: true, get() { const g = self._tv.g; return (g && g.pane) || self._tv.lp; }, set() {} });
          Object.defineProperty(this, "contentEl", { configurable: true, enumerable: true, get() { const g = self._tv.g; return (g && g.content) || self._tv.lp; }, set() {} });
          Object.defineProperty(this, "leaf", { configurable: true, enumerable: true, get() { return ctx.leafFor(self._tv.g) || leaf; }, set() {} });
        }
        this.editMode = new MarkdownEditView(this);
        this.previewMode = new MarkdownPreviewView(this);
      }
      getViewType() { return "markdown"; }
      getDisplayText() { const f = this.file; return f ? f.basename : ""; }
      getIcon() { return "document"; }
      get editor() { return editorFor(this._tv); }
      get sourceMode() { return { cmEditor: this.editor }; }
      get currentMode() { return this.getMode() === "preview" ? this.previewMode : this.editMode; }
      getMode() { const t = tabOf(this._tv); return t && t.mode === "reading" ? "preview" : "source"; }
      getViewData() { const e = this.editor; return e ? e.getValue() : ""; }
      setViewData(data, clear) { setText(this._tv, data, clear); }
      clear() {}
      getState() { const t = tabOf(this._tv), f = this.file; return { mode: this.getMode(), source: !!(t && t.mode === "source"), file: f ? f.path : null }; }
      getEphemeralState() { const e = this.editor; return e ? { cursor: { from: e.getCursor("from"), to: e.getCursor("to") } } : {}; }
      setEphemeralState(st) { const e = this.editor; if (e && st && st.cursor) e.setSelection(st.cursor.from, st.cursor.to); }
      showSearch() {}
      requestSave() { const g = this._tv && this._tv.g; if (g && typeof scheduleSave === "function") try { scheduleSave(g); } catch (_) {} }
      save() { this.requestSave(); return Promise.resolve(); }
    }
    protoField(MarkdownView, "hoverPopover", undefined);   // oracle: undefined until a hover popover is shown
    const mvOf = new WeakMap();   // tab view -> MarkdownView
    // the MarkdownView of ANY note editor view, tab or not yet (a scratch view is adopted by a
    // tab later; .file follows the tab): what editorInfoField holds from the state's creation on
    function infoFor(v) {
      if (!v) return null;
      let mv = mvOf.get(v); if (!mv) { mv = new MarkdownView(ctx.leafFor(v.g) || null, v); mvOf.set(v, mv); }
      return mv;
    }
    function viewFor(v, leaf) {
      if (!v || !tabOf(v)) return null;
      let mv = mvOf.get(v); if (!mv) { mv = new MarkdownView(leaf || ctx.leafFor(v.g), v); mvOf.set(v, mv); }
      return mv;
    }
    function allViews() {
      const out = new Set();
      for (const g of ctx.groups()) { if (g.view && g.view.cm) out.add(g.view); for (const t of g.tabs || []) if (t.view && t.view.cm) out.add(t.view); }
      return [...out];
    }
    // plug-val: the view must hold the ACTIVE tab: a graph tab (any non-note tab) in front of a note leaves g.view on the
    // note, and stock then has no active editor (editor-only commands drop out of the palette)
    const activeTabView = () => { const g = ctx.focused(); const t = g && g.active >= 0 ? tabOf(g.view) : null; return t && (!Array.isArray(g.tabs) || g.tabs[g.active] === t) ? g.view : null; };

    /* ------------------------------------------------- the info StateFields */
    const infoField = S.StateField.define({ create: () => null, update: v => v });
    const lpFx = S.StateEffect.define();
    const lpField = S.StateField.define({ create: () => true, update(v, tr) { for (const e of tr.effects) if (e.is(lpFx)) v = e.value; return v; } });
    const edFx = S.StateEffect.define();
    const edField = S.StateField.define({ create: () => null, update(v, tr) { for (const e of tr.effects) if (e.is(edFx)) v = e.value; return v; } });
    // a view mounted after the first plugin loaded builds its state BEFORE the EditorView exists
    // (Ed.mount): editorEditorField is filled in right after construction
    const edFill = V.ViewPlugin.fromClass(class { constructor(view) { if (view.state.field(edField, false) === null) queueMicrotask(() => { try { if (view.state.field(edField, false) === null) view.dispatch({ effects: edFx.of(view) }); } catch (_) {} }); } });
    /* livePreviewState: the ViewPlugin whose value tells a plugin whether a mouse button went
       down in this editor and is not yet released ({mousedown}), the way live-preview widgets
       defer a re-render while the user drags a selection */
    const livePreviewState = V.ViewPlugin.fromClass(class {
      constructor(view) { this.mousedown = false; this._doc = view.dom.ownerDocument; this._up = () => { this.mousedown = false; }; this._doc.addEventListener("mouseup", this._up, true); }
      destroy() { this._doc.removeEventListener("mouseup", this._up, true); }
    }, { eventHandlers: { mousedown(e) { if (e.button === 0) this.mousedown = true; } } });
    const isLp = v => !(v.lp && v.lp.classList.contains("src"));

    /* ---------------------------------------------------- registered exts */
    const exts = [];          // {ext, owner}
    const suggests = [];      // EditorSuggest instances
    let active = false;       // a plugin has loaded: views carry the base hooks
    const UEV = ["input", "delete", "os", "move"];   // FZ-E30.2: the user-edit families that run onTrigger
    const hasEv = n => { const w = ws(), a = w && w._ev && w._ev.get(n); return !!(a && a.length); };
    function hooks(v) {
      return [
        V.EditorView.updateListener.of(u => {
          if (u.docChanged && hasEv("editor-change")) { const mv = viewFor(v); if (mv) queueMicrotask(() => ws().trigger("editor-change", mv.editor, mv)); }
          // FZ-E30.2: only an edit that carries a userEvent (typing, delete, the editor's own os.* edits, a
          // plugin write whose origin is one of those) runs the suggests' onTrigger, as in stock. A bare editor-API write (Tasks "Toggle
          // task done" -> setLine) has none: it is treated like a caret move and opens nothing. tr.isUserEvent, not
          // tr.annotation(S.Transaction.userEvent): the editor's state module is not the plugins' copy (S), so the
          // annotation type differs and the lookup always missed (dv6 r1: typing opened nothing)
          const typed = u.docChanged && u.transactions.some(tr => UEV.some(k => tr.isUserEvent(k)));
          if (suggests.length && (u.docChanged || u.selectionSet)) queueMicrotask(() => suggestCheck(v, typed));
        }),
        V.EditorView.domEventHandlers({
          paste: e => { const mv = viewFor(v); if (mv && hasEv("editor-paste")) ws().trigger("editor-paste", e, mv.editor, mv); return e.defaultPrevented; },
          drop: e => { const mv = viewFor(v); if (mv && hasEv("editor-drop")) ws().trigger("editor-drop", e, mv.editor, mv); return e.defaultPrevented; },
        }),
        S.Prec.highest(V.EditorView.domEventHandlers({ keydown: e => suggestKey(v, e) })),
      ];
    }
    /* a plugin StreamLanguage as a highlight overlay. In Obsidian the note's language is itself a
       StreamLanguage over the CM5 "hypermd" mode, so a plugin that redefines that mode as an overlay
       (Templater 2.25.0: customOverlayMode(getMode("hypermd"), templater) at Prec.high) only adds
       token classes on top of markdown. Here the note keeps its lezer markdown tree (mdLang, which
       wins the language facet), so such a StreamLanguage would never run: it is bridged instead to
       a view plugin that runs its stream parser over the lines up to the viewport end and marks
       every token with the CM5 class names (style "a b" -> cm-a cm-b on the token, "line-x" /
       "line-background-x" -> class x on the line), under a .cm-s-obsidian editor as in Obsidian.
       Obsidian combines the two modes without combineTokens, so where the plugin's mode styles a
       token the markdown style is dropped there: no code span, emphasis or link inside a Templater
       command (a `string` in <% %> stays one JS string). The base "hypermd" is the null mode here,
       so every styled token is the plugin's: its ranges are published per line (Ed.lpOpaque) and
       Live Preview leaves its markdown decorations out of them. */
    const streamOv = new WeakMap();
    const OV_LINES = 20000;      // past this many lines before the viewport: start fresh at the viewport
    function streamOverlay(sl) {
      let ov = streamOv.get(sl);
      if (ov) return ov;
      const p = sl.streamParser, SS = window.CodeMirror && window.CodeMirror.StringStream;
      if (!p || typeof p.token !== "function" || !SS) { streamOv.set(sl, []); return []; }
      const skip = { null: 1, undefined: 1, "": 1 };
      function style(s, lineCls, from, to, marks) {
        if (!s) return;
        const cls = [];
        for (const t of String(s).split(/\s+/)) {
          if (skip[t]) continue;
          const m = /^line-(?:background-)?(.+)$/.exec(t);
          if (m) lineCls.add(m[1]); else cls.push("cm-" + t);
        }
        if (cls.length && to > from) marks.push(V.Decoration.mark({ class: cls.join(" "), attributes: { "data-os-ov": "" } }).range(from, to));
      }
      const views = new WeakMap();   // view -> {doc, vf, vt, deco, opq}: one tokenizer pass shared by the overlay and LP
      function get(view) {
        const doc = view.state.doc, vp = view.viewport;
        let c = views.get(view);
        if (!c || c.doc !== doc || c.vf !== vp.from || c.vt !== vp.to) { c = Object.assign({ doc, vf: vp.from, vt: vp.to }, build(view)); views.set(view, c); }
        return c;
      }
      function build(view) {
        const doc = view.state.doc, vp = view.viewport, out = [], opq = new Map();
        const first = doc.lineAt(vp.from).number, last = doc.lineAt(vp.to).number;
        let n = first > OV_LINES ? first : 1, st;
        try { st = p.startState((L.indentUnit && view.state.facet(L.indentUnit).length) || 4); } catch (e) { console.error("[plugin] stream overlay startState:", e); return { deco: V.Decoration.none, opq }; }
        for (; n <= last; n++) {
          const line = doc.line(n), lineCls = new Set(), marks = [], vis = n >= first, rs = [];
          try {
            if (!line.length) { if (p.blankLine) style(p.blankLine(st, 4), lineCls, 0, 0, marks); }
            else {
              const ss = new SS(line.text, view.state.tabSize);
              let stuck = 0;
              while (!ss.eol()) {
                ss.start = ss.pos;
                const s = p.token(ss, st);
                if (ss.pos <= ss.start) { if (++stuck > 3) break; continue; }
                stuck = 0;
                if (vis) {
                  const k = marks.length;
                  style(s, lineCls, line.from + ss.start, line.from + ss.pos, marks);
                  if (marks.length > k) { const l = rs[rs.length - 1]; if (l && l[1] === ss.start) l[1] = ss.pos; else rs.push([ss.start, ss.pos]); }
                }
              }
            }
          } catch (e) { console.error("[plugin] stream overlay token:", e); return { deco: V.Decoration.none, opq: new Map() }; }
          if (!vis) continue;
          if (rs.length) opq.set(n, rs);
          for (const c of lineCls) out.push(V.Decoration.line({ class: c, attributes: { "data-os-ovl": "" } }).range(line.from));
          for (const m of marks) out.push(m);
        }
        return { deco: V.Decoration.set(out, true), opq };
      }
      ov = [V.EditorView.editorAttributes.of({ class: "cm-s-obsidian" }), V.ViewPlugin.fromClass(class {
        constructor(view) { this.decorations = get(view).deco; }
        update(u) { if (u.docChanged || u.viewportChanged) this.decorations = get(u.view).deco; }
      }, { decorations: x => x.decorations })];
      ov._opq = view => get(view).opq;
      streamOv.set(sl, ov);
      return ov;
    }
    let liveOv = new Set();      // the overlays the current plugin extensions bridge (rebuilt by pxExt)
    /* Ed.lpOpaque(view): Map line number -> [[from, to] line-relative, ...] of plugin-styled tokens, or null */
    EdG.lpOpaque = function (view) {
      if (!liveOv.size) return null;
      let m = null;
      for (const f of liveOv) for (const [n, rs] of f(view)) { m = m || new Map(); m.set(n, (m.get(n) || []).concat(rs)); }
      return m;
    };
    function bridge(e, d) {
      if (!e || typeof e !== "object" || d > 8) return e;
      if (Array.isArray(e)) { let ch = false; const r = e.map(x => { const y = bridge(x, d + 1); if (y !== x) ch = true; return y; }); return ch ? r : e; }
      if (L && L.StreamLanguage && e instanceof L.StreamLanguage) { const o = streamOverlay(e); if (o._opq) liveOv.add(o._opq); return o; }
      if ("inner" in e && "prec" in e) { const y = bridge(e.inner, d + 1); return y === e.inner ? e : y; }   // Prec.*(x)
      return e;
    }
    EdG.pxExt = function (v) {
      if (!active) return [];
      const out = [infoField.init(() => infoFor(v)), lpField.init(() => isLp(v)), edField.init(() => v.cm || null), edFill, livePreviewState, hooks(v), mdLang()];
      liveOv = new Set();
      for (const x of exts) out.push(bridge(x.ext, 0));
      return out;
    };
    /* the note's syntax tree, for plugins that walk it (syntaxTree(state)): while a plugin is
       loaded the editor carries a Markdown language (the vendored @lezer/markdown, GFM) — tree
       only: no keymap, no highlighting, no completion. Inline code parses the way plugins expect
       from Obsidian's tree: each backtick run is its own node and the code between them is one
       node, tagged under tokenClassNodeProp ("formatting formatting-code inline-code" /
       "inline-code") — dataview finds its Live Preview inline queries by that prop and replaces
       [from-1, to+1] of the code node. */
    let mdLangExt = null;
    function mdLang() {
      if (mdLangExt) return mdLangExt;
      const prop = typeof ctx.tokenClassNodeProp === "function" ? ctx.tokenClassNodeProp() : undefined;
      if (!L || typeof L.Language !== "function" || !LM || !LM.markdownLanguage || !prop) return (mdLangExt = []);
      const BT = 96, code = {
        defineNodes: [{ name: "OsInlineCode" }, { name: "OsInlineCodeMark" }, { name: "OsInlineCodeText" }],
        props: [prop.add({ OsInlineCodeMark: "formatting formatting-code inline-code", OsInlineCodeText: "inline-code" })],
        parseInline: [{ name: "OsInlineCode", before: "InlineCode", parse(cx, next, pos) {
          if (next !== BT || (pos > cx.offset && cx.char(pos - 1) === BT)) return -1;
          let s = pos + 1; while (s < cx.end && cx.char(s) === BT) s++;
          const n = s - pos;
          for (let i = s; i < cx.end; i++) {
            if (cx.char(i) !== BT) continue;
            let j = i + 1; while (j < cx.end && cx.char(j) === BT) j++;
            if (j - i === n) {
              const kids = [cx.elt("OsInlineCodeMark", pos, s)];
              if (i > s) kids.push(cx.elt("OsInlineCodeText", s, i));
              kids.push(cx.elt("OsInlineCodeMark", i, j));
              return cx.addElement(cx.elt("OsInlineCode", pos, j, kids));
            }
            i = j - 1;
          }
          return -1;
        } }],
      };
      // Prec.highest: the language facet keeps its FIRST value, and Templater 2.25.0 registers a
      // Prec.high StreamLanguage (its CM5 "templater" overlay; with no CM5 javascript mode here it
      // is the null mode = a flat, nodeless tree) that would otherwise displace this tree for every
      // plugin that reads it (plug-top5: dataview inline queries never rendered in Live Preview)
      try { mdLangExt = S.Prec.highest(new L.Language(L.defineLanguageFacet(), LM.markdownLanguage.parser.configure([code]), [], "markdown").extension); }
      catch (e) { console.error("[plugin] markdown syntax tree:", e); mdLangExt = []; }
      return mdLangExt;
    }
    let bad = 0;
    function reconfigure(v) {
      if (!v.cm || !EdG.PX) return;
      try { v.cm.dispatch({ effects: EdG.PX.reconfigure(EdG.pxExt(v)) }); }
      catch (e) {
        // one bad extension (not an Extension, a throwing facet) must not take the editor down:
        // drop the newest one and retry once
        bad++; console.error("[plugin] editor extension rejected:", e);
        const x = exts.pop(); if (x && x.owner) console.error("[plugin] " + x.owner + ": registerEditorExtension dropped");
        try { v.cm.dispatch({ effects: EdG.PX.reconfigure(EdG.pxExt(v)) }); } catch (_) {}
      }
    }
    const reconfigureAll = () => { for (const v of allViews()) reconfigure(v); ctx.touch(); };
    function activate() { if (active) return; active = true; reconfigureAll(); }
    function addExt(owner, ext) {
      const rec = { ext, owner };
      exts.push(rec); activate(); reconfigureAll();
      return () => { const i = exts.indexOf(rec); if (i >= 0) { exts.splice(i, 1); reconfigureAll(); } };
    }

    /* ------------------------------------------------------- EditorSuggest
       onTrigger(cursor, editor, file) -> {start, end, query} | null after a doc change;
       getSuggestions(ctx) (sync or Promise) -> renderSuggestion(item, el) into a popup
       drawn with the wikilink autocomplete's .ac styling; keys go to the suggest's own scope
       (arrows / PageUp / PageDown / Home / End / Ctrl+P / Ctrl+N move, Enter chooses, Escape
       closes; keydown at Prec.highest, ahead of Ed.onKey). */
    class PopoverSuggest {
      // scope: the suggest's own key scope, a child of the one passed in (or of app.scope), never
      // the caller's. It is BORN with the popover's navigation keys (oracle suggest.scopeKeys, stock
      // 1.13.7: ArrowUp, ArrowDown, PageUp, PageDown, Home, End, Ctrl+P, Ctrl+N, Enter, Escape, in
      // that order), so a subclass may read / unregister / rebind them right after super(). They are
      // the ONLY key path of an open EditorSuggest: an unregistered key is not handled by the popover.
      constructor(app_, scope) {
        this.app = app_; this.scope = ctx.Scope ? new ctx.Scope(scope || (app_ && app_.scope) || undefined) : null;
        this.isOpen = false; this.suggestEl = document.createElement("div"); this.suggestEl.className = "ac os-esug suggestion-container"; this.suggestEl.hidden = true; this._items = []; this._sel = 0;
        const sc = this.scope;
        if (sc && typeof sc.register === "function") {
          const n = () => (this.isOpen && this._items && this._items.length) | 0;
          const step = d => () => { const c = n(); if (!c) return; this._sel = (((this._sel | 0) + d) % c + c) % c; this._draw(); return false; };
          const jump = f => () => { const c = n(); if (!c) return; this._sel = Math.max(0, Math.min(c - 1, f(c, this._sel | 0))); this._draw(); return false; };
          const page = () => { const el = this.suggestEl, it = el && el.firstElementChild, h = it ? it.offsetHeight : 0; return h > 0 ? Math.max(1, Math.floor(el.clientHeight / h)) : 10; };
          sc.register([], "ArrowUp", step(-1));
          sc.register([], "ArrowDown", step(1));
          sc.register([], "PageUp", jump((c, i) => i - page()));
          sc.register([], "PageDown", jump((c, i) => i + page()));
          sc.register([], "Home", jump(() => 0));
          sc.register([], "End", jump(c => c - 1));
          sc.register(["Ctrl"], "P", step(-1));
          sc.register(["Ctrl"], "N", step(1));
          sc.register([], "Enter", e => { if (!n()) return; this._choose(this._sel | 0, e); return false; });
          sc.register([], "Escape", () => { if (!this.isOpen) return; this.close(); if (openS === this) openS = null; return false; });
        }
      }
      open() { this.isOpen = true; this.suggestEl.hidden = false; }
      close() { this.isOpen = false; this.suggestEl.hidden = true; this.suggestEl.textContent = ""; this._items = []; ctx.touch(); }
      onHistoryBack() { if (this.isOpen) this.close(); }   // history back (mouse back button) dismisses an open popover
      renderSuggestion() {} selectSuggestion() {}
      _draw() { if (this._v) draw(this, this._v); }   // redraw where it was last drawn (AbstractInputSuggest has its own)
      _choose(i, e) { choose(this, i, e); }
    }
    protoField(PopoverSuggest, "app", null);
    protoField(PopoverSuggest, "scope", null);
    class EditorSuggest extends PopoverSuggest {
      constructor(app_) { super(app_); this.context = null; this.instructions = []; }
      setInstructions(i) { this.instructions = i || []; }
      onTrigger() { return null; }
      getSuggestions() { return []; }
      close() { super.close(); this.context = null; }
    }
    protoField(EditorSuggest, "context", null);
    protoField(EditorSuggest, "limit", 100);   // oracle: 100 until a subclass sets its own
    let openS = null, reqN = 0;
    function draw(s, v) {
      s._v = v;
      const el = s.suggestEl, g = v.g;
      el.textContent = "";
      s._items.forEach((it, i) => {
        const d = document.createElement("div");
        d.className = "suggestion-item" + (i === s._sel ? " sel is-selected" : "");
        try { s.renderSuggestion(it, d); } catch (e) { console.error("[plugin] renderSuggestion:", e); }
        d.addEventListener("mousedown", e => { e.preventDefault(); choose(s, i, e); });
        el.appendChild(d);
      });
      if (el.parentNode !== g.content) g.content.appendChild(el);
      const xy = EdG.caretXY(g);
      el.style.left = Math.max(0, Math.min(xy[0], g.content.clientWidth - 200)) + "px";
      el.style.top = Math.min(xy[1], Math.max(0, g.content.clientHeight - 60)) + "px";
      s.open(); ctx.touch();
    }
    function choose(s, i, e) {
      const it = s._items[i]; if (it === undefined) return;
      try { s.selectSuggestion(it, e || new KeyboardEvent("keydown")); } catch (er) { console.error("[plugin] selectSuggestion:", er); }
      s.close(); if (openS === s) openS = null;
    }
    function suggestCheck(v, docChanged) {
      const g = v.g;
      if (!g || g.view !== v || !v.cm || !v.cm.hasFocus) { if (openS) { openS.close(); openS = null; } return; }
      const ed = editorFor(v), mv = viewFor(v), cur = ed.getCursor("head"), file = mv ? mv.file : null;
      if (!docChanged) {   // a caret move outside the trigger range closes it
        if (openS && openS.context) { const c = openS.context, o = ed.posToOffset(cur); if (o < ed.posToOffset(c.start) || o > ed.posToOffset(c.end)) { openS.close(); openS = null; } }
        return;
      }
      for (const s of suggests.slice()) {
        if (!s || typeof s.onTrigger !== "function") continue;
        let t = null;
        try { t = s.onTrigger(cur, ed, file); } catch (e) { console.error("[plugin] onTrigger:", e); }
        if (!t) continue;
        if (openS && openS !== s) openS.close();
        openS = s; s.context = Object.assign({}, t, { editor: ed, file });
        const n = ++reqN;
        Promise.resolve().then(() => s.getSuggestions(s.context)).then(items => {
          if (n !== reqN || openS !== s) return;
          { const lim = Number(s.limit); s._items = (items || []).slice(0, Number.isFinite(lim) && lim > 0 ? lim : 100); } s._sel = 0;
          if (!s._items.length) { s.close(); openS = null; return; }
          draw(s, v);
        }).catch(e => console.error("[plugin] getSuggestions:", e));
        return;
      }
      if (openS) { openS.close(); openS = null; }
    }
    function suggestKey(v, e) {
      const s = openS; if (!s || !s.isOpen || !s._items.length) return false;
      // the suggest's own scope is its whole key map (born with the navigation keys, see PopoverSuggest):
      // a handler returning false consumed the key. Only ITS handlers run here — the parent chain
      // (app.scope) is not consulted from inside the editor's keydown.
      const sc = s.scope; if (!sc || typeof sc.handle !== "function") return false;
      let r; const par = sc.parent;
      try { sc.parent = undefined; r = sc.handle(e); } catch (er) { console.error("[plugin] suggest scope:", er); }
      finally { sc.parent = par; }
      if (r !== false) return false;
      e.preventDefault(); e.stopPropagation(); return true;
    }
    /* the one live EditorSuggest list: workspace.editorSuggest.suggests IS this array (views.js), so a
       plugin.registerEditorSuggest and an editorSuggest.addSuggest land in the same place. addSuggest
       appends as given (twice = twice); removeSuggest drops every occurrence, closing it when open. */
    const closeS = s => { if (s && typeof s.close === "function") try { s.close(); } catch (e) { console.error("[plugin] suggest close:", e); } };
    function removeSuggest(s) {
      let hit = false;
      for (let i; (i = suggests.indexOf(s)) >= 0;) { suggests.splice(i, 1); hit = true; }
      if (s && openS === s) { openS = null; closeS(s); }
      if (hit && s && s.suggestEl && typeof s.suggestEl.remove === "function") s.suggestEl.remove();
    }
    function addSuggest(s) {
      suggests.push(s); activate();
      return () => removeSuggest(s);
    }
    const currentSuggest = () => openS || null;
    // FZ-E30.2: a leaf leaving its edit mode (setMode -> reading) closes an open suggest, as stock does
    EdG.closeSuggest = () => { const s = openS; openS = null; reqN++; if (s) closeS(s); };
    function setCurrentSuggest(s) { const c = openS; openS = s || null; if (c && c !== openS) closeS(c); }

    /* ------------------------------------------- live-preview flag follows the mode */
    function tick() {
      if (!active) return;
      const v = activeTabView(); if (!v || !v.cm) return;
      const f = v.cm.state.field(lpField, false);
      if (f !== undefined && f !== isLp(v)) v.cm.dispatch({ effects: lpFx.of(isLp(v)) });
    }
    // [pext:x<exts>,s<suggests>,v<views carrying the base hooks>,d=<value><count>+... of the [data-os-probe]
    // nodes drawn in the focused editor (a gate probe tags its decorations with it; "0" = none),o<suggest items open>,
    // ov<stream overlay token spans>/<overlay-classed lines> in the focused editor (only while some are drawn)]
    function tok() {
      if (!active) return "";
      const v = activeTabView(), m = {}; let nv = 0;
      for (const w of allViews()) if (w.cm.state.field(infoField, false) !== undefined) nv++;
      if (v && v.cm) for (const n of v.cm.contentDOM.querySelectorAll("[data-os-probe]")) { const k = String(n.getAttribute("data-os-probe")).replace(/[^A-Za-z0-9]/g, "").slice(0, 8); m[k] = (m[k] || 0) + 1; }
      const d = Object.keys(m).sort().map(k => k + m[k]).join("+") || "0";
      const ovs = v && v.cm ? v.cm.contentDOM.querySelectorAll("[data-os-ov]").length : 0, ovl = v && v.cm ? v.cm.contentDOM.querySelectorAll("[data-os-ovl]").length : 0;
      // om<N>: markdown LP marks (code/em/strong/link) nested with an overlay token span — 0 as in Obsidian
      const om = ovs && v && v.cm ? v.cm.contentDOM.querySelectorAll("[data-os-ov] :is(code,em,strong,a,.mk), :is(code,em,strong,a,.mk) [data-os-ov]").length : 0;
      return " [pext:x" + exts.length + ",s" + suggests.length + ",v" + nv + ",d=" + d + ",o" + (openS && openS.isOpen ? openS._items.length : 0) + (bad ? ",bad" + bad : "") + (ovs || ovl ? ",ov" + ovs + "/" + ovl + ",om" + om : "") + "]";
    }

    return {
      Editor, MarkdownView, MarkdownEditView, MarkdownPreviewView, EditorSuggest, PopoverSuggest, livePreviewState,
      editorInfoField: infoField, editorViewField: infoField, editorLivePreviewField: lpField, editorEditorField: edField,
      viewFor, editorFor, activeTabView, addExt, addSuggest, activate, reconfigureAll, tick, tok,
      suggests, currentSuggest, setCurrentSuggest, removeSuggest,   // SEAM S1: workspace.editorSuggest is built over these
      activeEditor() { const v = activeTabView(), mv = v && viewFor(v); return mv ? mv : null; },
    };
  }
  Object.defineProperty(window, "__osEditorApi", { value: install, configurable: true, writable: false, enumerable: false });

  /* ------------------------------------------------- window.CodeMirror (CM5 modes)
     Obsidian keeps CodeMirror 5's mode registry on window.CodeMirror so plugins can
     define legacy stream modes and hand them to CM6's StreamLanguage. Templater 2.25.0
     runs `(function(e){e(window.CodeMirror)})(function(e){e.defineMode(...)})` at
     module scope and failed to load with "undefined is not an object (evaluating
     'e.defineMode')". Bounded: the registry only (defineMode / defineMIME / getMode /
     resolveMode / startState / copyState / innerMode / registerHelper / Pass /
     StringStream) — no CM5 editor, no DOM. Plugins may add properties (Templater sets
     customOverlayMode), as in Obsidian. A mode never runs unless a plugin asks
     StreamLanguage for it. */
  if (!window.CodeMirror) {
    const modes = Object.create(null), mimeModes = Object.create(null), helpers = Object.create(null);
    const own = (o, k) => Object.prototype.hasOwnProperty.call(o, k);
    const nullMode = { name: "null", token(stream) { stream.skipToEnd(); return null; } };
    class StringStream {
      constructor(string, tabSize, lineOracle) { this.pos = this.start = 0; this.string = String(string); this.tabSize = tabSize || 8; this.lastColumnPos = this.lastColumnValue = 0; this.lineStart = 0; this.lineOracle = lineOracle; }
      eol() { return this.pos >= this.string.length; }
      sol() { return this.pos == this.lineStart; }
      peek() { return this.string.charAt(this.pos) || undefined; }
      next() { if (this.pos < this.string.length) return this.string.charAt(this.pos++); }
      eat(m) { const ch = this.string.charAt(this.pos); const ok = typeof m == "string" ? ch == m : ch && (m.test ? m.test(ch) : m(ch)); if (ok) { ++this.pos; return ch; } }
      eatWhile(m) { const s = this.pos; while (this.eat(m)) {} return this.pos > s; }
      eatSpace() { const s = this.pos; while (/[\s ]/.test(this.string.charAt(this.pos))) ++this.pos; return this.pos > s; }
      skipToEnd() { this.pos = this.string.length; }
      skipTo(ch) { const f = this.string.indexOf(ch, this.pos); if (f > -1) { this.pos = f; return true; } }
      backUp(n) { this.pos -= n; }
      column() { return this.start - this.lineStart; }
      indentation() { const m = /^\s*/.exec(this.string.slice(this.lineStart)); return m ? m[0].length : 0; }
      match(pattern, consume, caseInsensitive) {
        if (typeof pattern == "string") {
          const cased = s => caseInsensitive ? s.toLowerCase() : s;
          if (cased(this.string.substr(this.pos, pattern.length)) == cased(pattern)) { if (consume !== false) this.pos += pattern.length; return true; }
          return null;
        }
        const m = this.string.slice(this.pos).match(pattern);
        if (m && m.index > 0) return null;
        if (m && consume !== false) this.pos += m[0].length;
        return m;
      }
      current() { return this.string.slice(this.start, this.pos); }
      hideFirstChars(n, inner) { this.lineStart += n; try { return inner(); } finally { this.lineStart -= n; } }
      lookAhead(n) { const o = this.lineOracle; return o && o.lookAhead ? o.lookAhead(n) : undefined; }
      baseToken() {}
    }
    const CM = {
      Pass: { toString() { return "CodeMirror.Pass"; } },
      StringStream, modes, mimeModes,
      defineMode(name, factory) { if (arguments.length > 2) factory.dependencies = Array.prototype.slice.call(arguments, 2); modes[String(name)] = factory; },
      defineMIME(mime, spec) { mimeModes[String(mime)] = spec; },
      resolveMode(spec) {
        if (typeof spec == "string" && own(mimeModes, spec)) spec = mimeModes[spec];
        else if (spec && typeof spec.name == "string" && own(mimeModes, spec.name)) { const found = mimeModes[spec.name]; spec = Object.assign({}, typeof found == "string" ? { name: found } : found, spec, { name: typeof found == "string" ? found : found.name }); }
        if (typeof spec == "string") return { name: spec };
        return spec || { name: "null" };
      },
      getMode(options, spec) {
        spec = CM.resolveMode(spec);
        const f = own(modes, spec.name) ? modes[spec.name] : null;
        if (!f) return Object.assign({}, nullMode);
        const m = f(options || {}, spec);
        m.name = spec.name;
        if (spec.helperType) m.helperType = spec.helperType;
        return m;
      },
      startState(mode, a1, a2) { return mode.startState ? mode.startState(a1, a2) : true; },
      copyState(mode, state) {
        if (state === true) return state;
        if (mode.copyState) return mode.copyState(state);
        const n = {};
        for (const k in state) { let v = state[k]; if (Array.isArray(v)) v = v.concat([]); n[k] = v; }
        return n;
      },
      innerMode(mode, state) {
        let info;
        while (mode.innerMode) { info = mode.innerMode(state); if (!info || info.mode == mode) break; state = info.state; mode = info.mode; }
        return info || { mode, state };
      },
      registerHelper(type, name, value) { (helpers[type] || (helpers[type] = Object.create(null)))[name] = value; },
      helpers,
    };
    Object.defineProperty(window, "CodeMirror", { value: CM, configurable: true, writable: true, enumerable: false });
  }

  /* ------------------------------------------ window.CodeMirrorAdapter (census C4)
     Obsidian exposes its vim layer's CM5-compatible adapter class on window, and its
     static `commands` table is where the vim Ex commands look up named editor commands
     (`:w` runs commands.save). obsidian-linter 1.32.0 assigns
     `window.CodeMirrorAdapter.commands.save = () => executeCommandById("editor:save-file")`
     in onload and failed to load with "undefined is not an object (evaluating
     'window.CodeMirrorAdapter.commands')". Bounded to that public shape: the class and a
     plain, writable command table that plugins may add to. opensidian has no vim mode,
     so nothing dispatches from the table — an entry is stored, never run, and reports
     nothing — and an adapter instance cannot be made (the constructor throws: there is no
     vim editor to adapt). */
  if (!window.CodeMirrorAdapter) {
    class CodeMirrorAdapter {
      constructor() { throw new Error("CodeMirrorAdapter: unsupported in opensidian: no vim mode"); }
    }
    Object.defineProperty(CodeMirrorAdapter, "commands", { value: {}, configurable: true, writable: true, enumerable: true });
    Object.defineProperty(window, "CodeMirrorAdapter", { value: CodeMirrorAdapter, configurable: true, writable: true, enumerable: false });
  }
})();

/* the "obsidian" module, part EDITOR (api-parity stage 0: moved here from api.js,
   behaviour-neutral): MarkdownRenderChild, markdown post / code-block processors,
   MarkdownRenderer, and the editor-api install (EA, MarkdownView). A separate closure
   from the one above (its S / V are CM6 names): it sees only what api.js hands it in K.
   Segment contract: see api-core.js. */
(function () {
  "use strict";
  const parts = window.__osApiParts || (window.__osApiParts = Object.create(null));
  const reg = (id, fn) => { if (!(id in parts)) parts[id] = fn; };
  reg("editor:render", function (K) {
    const { host, be } = K;
    const { Component, P, touch, views } = K;
    class MarkdownRenderChild extends Component { constructor(el) { super(); this.containerEl = el; } }

    /* ------------------- markdown post / code-block processors (plugin-views item 7)
       The Rust renderer (render / render_view) emits a fenced block as
       <pre><code class="language-L">. A registered code-block processor for L
       replaces that <pre> with div.block-language-L and gets (source, el, ctx);
       post processors then run, in sortOrder, on every top-level block element.
       Both run over the reading view (main.js preview() asks PPOST after its
       innerHTML) and over MarkdownRenderer.render. ctx.addChild ties a
       MarkdownRenderChild to that render: the next render of the same pane (or
       the caller's component) unloads it. PPOST is set only once a plugin
       registers one, so a vault without such plugins pays nothing; gen bumps on
       every (un)register so the reading view's retained DOM is re-rendered. */
    const MDP = { post: [], code: new Map(), gen: 0, used: false };
    function mdpChanged() { MDP.gen++; MDP.used = true; try { PPOST = (MDP.post.length || MDP.code.size || embPluginExts()) ? PP : null; } catch (_) {} cbHookSet(); touch(); }
    // plug-top5: the Live Preview side of the code-block processors (editor.js Ed.cbField): a fence
    // of a registered language renders as one block widget; each widget owns its render's children
    let cbT = 0;
    function cbHookSet() {
      try {
        if (typeof Ed === "undefined" || !Ed) return;
        if (!Ed.cbHook) Ed.cbHook = {
          any: () => MDP.code.size > 0,
          has: lang => MDP.code.has(lang),
          gen: () => MDP.gen,
          render(lang, src, el, sourcePath) {
            const p = MDP.code.get(lang);
            if (!p) return null;
            const h = new Component(); h.load();
            runCode(lang, p, src, el, mdCtx(sourcePath, h));
            return () => h.unload();
          },
        };
        if (Ed.cbRefresh) { clearTimeout(cbT); cbT = setTimeout(() => { try { Ed.cbRefresh(); } catch (e) { console.error(e); } }, 0); }   // a plugin registers several in a row: one rebuild
      } catch (e) { console.error(e); }
    }
    // main.js tabGone: a closed tab unloads its reading embed children, even once no processor or plugin
    // embed is left (PPOST null by then). Armed by embHolder, the only maker of tab-owned children:
    // this part runs from api.js before main.js has declared PTABGONE, so it cannot be set here.
    const armTabGone = () => { try { if (PTABGONE !== embRelease) PTABGONE = embRelease; } catch (_) {} };
    const pathOf = n => (n ? n + ".md" : "");   // the app names notes without ".md"
    function mdCtx(sourcePath, holder, sec) {
      let fm; try { const c = K.metadataCache.getCache(sourcePath); fm = c ? c.frontmatter : undefined; } catch (_) {}
      return {
        docId: sourcePath, sourcePath, frontmatter: fm,
        addChild: ch => { holder.addChild(ch); return ch; },
        getSectionInfo: el => (sec ? sec(el) : null),
      };
    }
    function runCode(lang, p, src, el, ctx) {
      try { const r = p(src, el, ctx); if (r && typeof r.catch === "function") r.catch(e => console.error("[plugin] code block " + lang + ":", e)); }
      catch (e) { console.error("[plugin] code block " + lang + ":", e); }
    }
    // the fenced blocks of language `lang` (or any registered one when lang is null) inside
    // `scope` (an element, or a list of them; a <pre> itself counts) -> [{pre, code, lang}]
    function fences(scope, lang) {
      const out = [], els = Array.isArray(scope) ? scope : [scope];
      for (const r of els) {
        if (!r || r.nodeType !== 1) continue;
        const cs = r.matches && r.matches("pre") ? [...r.querySelectorAll(":scope > code[class*='language-']")] : [...r.querySelectorAll("pre > code[class*='language-']")];
        for (const code of cs) {
          const m = /(?:^|\s)language-(\S+)/.exec(code.className), l = m ? m[1] : "";
          if (lang == null || l === lang) out.push({ pre: code.parentElement, code, lang: l });
        }
      }
      return out;
    }
    // one fence -> div.block-language-L, then the handler gets (source, div, ctx)
    function codeBlock(f, p, ctx) {
      const { pre, code, lang } = f;
      if (!pre || !pre.parentNode) return;
      const el = document.createElement("div"); el.className = "block-language-" + lang;
      pre.replaceWith(el);
      runCode(lang, p, code.textContent.replace(/\n$/, ""), el, ctx);
    }
    async function runPost(p, el, ctx) {
      try { const r = p(el, ctx); if (r && typeof r.then === "function") await r.catch(e => console.error("[plugin] post processor:", e)); }
      catch (e) { console.error("[plugin] post processor:", e); }
    }
    /* SEAM S3 — plugin embeds (app.embedRegistry). The Rust renderer emits an internal embed
       ![[f.ext#sub]] it does not render itself (not an image / pdf) as the text "!" followed by
       <a class="wiki" data-note="f.ext" data-anchor="sub">. When f resolves to a TFile whose
       extension a PLUGIN registered (the built-in md entry is left to the app's own rendering),
       the "!" + link is replaced by span.internal-embed[alt][src] = the linktext, the creator gets
       (ctx, file, subpath) with ctx {app, containerEl, depth: 1, displayMode, linktext, showInline:
       true, sourcePath} (displayMode true from MarkdownRenderer.render, present but undefined in the
       reading view — as stock 1.13.7), the child is added to `holder` (so unloading it unloads the
       child) and loadFile() is called once with no arguments; then the span gets is-loaded. A
       creator or loadFile that throws leaves an error embed in that span and the rest renders. */
    const embOwn = (o, k) => Object.prototype.hasOwnProperty.call(o, k);
    function embPluginExts() {
      const r = K.app && K.app.embedRegistry, m = r && r.embedByExtension;
      if (!m) return false;
      for (const k in m) if (embOwn(m, k) && k !== "md") return true;
      return false;
    }
    function embError(span, linktext, e) {
      console.error("[plugin] embed " + linktext + ":", e);
      span.classList.add("embed-error");
      const d = document.createElement("div"); d.className = "embed-error-message";
      d.textContent = "Unable to render embed “" + linktext + "”: " + ((e && e.message) || String(e));
      span.appendChild(d);
    }
    function embedPass(scope, sourcePath, holder, displayMode) {
      const r = K.app && K.app.embedRegistry, mc = K.app && K.app.metadataCache;
      if (!r || !mc || !scope || (!embPluginExts() && !MDP.post.length)) return [];
      const pend = [], links = [];
      for (const n of Array.isArray(scope) ? scope : [scope]) {
        if (!n || n.nodeType !== 1) continue;
        if (n.matches("a.wiki")) links.push(n);
        links.push(...n.querySelectorAll("a.wiki"));
      }
      for (const a of links) {
        const bang = a.previousSibling;
        if (!bang || bang.nodeType !== 3 || !bang.data.endsWith("!")) continue;
        const note = a.dataset.note;
        if (!note || a.closest(".internal-embed")) continue;
        let file = null; try { file = mc.getFirstLinkpathDest(note, sourcePath); } catch (_) {}
        const creator = file && file.extension !== "md" && embOwn(r.embedByExtension, file.extension) ? r.getEmbedCreator(file) : null;
        if (typeof creator !== "function") {
          /* a note embed the app shows as its link (no transclusion, req 410): still the embed's
             element for the post processors — span.internal-embed[src][alt] = the linktext around
             the unchanged "!" + link (Excalidraw draws a drawing embed there). Unresolved ones too,
             as stock; nothing is wrapped while no plugin registered a post processor. */
          if (!MDP.post.length) continue;
          const lt = note + (a.dataset.anchor ? "#" + a.dataset.anchor : "");
          const w = document.createElement("span");
          w.setAttribute("alt", lt); w.setAttribute("src", lt); w.className = "internal-embed";
          a.replaceWith(w);
          if (bang.data.length > 1) { bang.data = bang.data.slice(0, -1); w.append("!", a); } else w.append(bang, a);
          continue;
        }
        const an = a.dataset.anchor, subpath = an ? "#" + an : "", linktext = note + subpath;
        const span = document.createElement("span");
        span.setAttribute("alt", linktext); span.setAttribute("src", linktext); span.className = "internal-embed";
        if (bang.data.length > 1) bang.data = bang.data.slice(0, -1); else bang.remove();
        a.replaceWith(span);
        const ctx = { app: K.app, containerEl: span, depth: 1, displayMode, linktext, showInline: true, sourcePath };
        let child;
        try {
          child = creator(ctx, file, subpath);
          if (child instanceof Component) holder.addChild(child);
          const p = child && typeof child.loadFile === "function" ? child.loadFile() : undefined;
          if (p && typeof p.then === "function")
            pend.push(Promise.resolve(p).then(() => span.classList.add("is-loaded"), e => embError(span, linktext, e)));
          else span.classList.add("is-loaded");
        } catch (e) { embError(span, linktext, e); }
      }
      return pend;
    }
    /* whole = MarkdownRenderer.render: the post processors run ONCE on the target element
       (oracle: stock 1.13.7 hands them the element passed to render, not one call per block);
       the reading view (PP.run) hands them every top-level block. `nodes` limits the code
       block scan to what this render added. */
    async function mdProcess(root, sourcePath, holder, whole, nodes, embHold, sec) {
      const pend = embedPass(nodes || [root], sourcePath, embHold || holder, whole ? true : undefined);
      if (pend.length) await Promise.all(pend);
      if (!MDP.code.size && !MDP.post.length) return;
      const ctx = mdCtx(sourcePath, holder, sec);
      if (MDP.code.size) for (const f of fences(nodes || root, null)) { const p = MDP.code.get(f.lang); if (p) codeBlock(f, p, ctx); }
      if (!MDP.post.length) return;
      const posts = MDP.post.slice().sort((a, b) => (a.sortOrder || 0) - (b.sortOrder || 0));
      if (whole) { for (const p of posts) await runPost(p, root, ctx); return; }
      for (const el of [...root.children]) for (const p of posts) await runPost(p, el, ctx);
    }
    /* the reading view's embed children belong to the TAB (stock: the view), not to one render:
       a re-render keeps the previous render's children (stock 1.13.7: rerender(true) unloads
       nothing) and unloads the ones from the render before that, so a pane re-rendered many
       times holds at most two renders' worth; a different note in the tab, or the tab closing
       (main.js tabGone -> embRelease), unloads them all. */
    function embRelease(t) {
      const e = t && t._emb;
      if (!e) return;
      for (const h of [e.prev, e.cur]) if (h) try { h.unload(); } catch (err) { console.error(err); }
      e.prev = e.cur = null;
    }
    function embHolder(t, note) {
      if (!t || typeof t !== "object") { const h = new Component(); h.load(); return h; }
      armTabGone();
      if (!t._emb) Object.defineProperty(t, "_emb", { value: { prev: null, cur: null, note }, configurable: true });
      const e = t._emb;
      if (e.note !== note) { embRelease(t); e.note = note; }
      if (e.prev) try { e.prev.unload(); } catch (err) { console.error(err); }
      e.prev = e.cur; e.cur = new Component(); e.cur.load();
      return e.cur;
    }
    /* plug-top5: ctx.getSectionInfo(el) in the reading view = {text, lineStart, lineEnd} of the top-level
       block el is (or sits in): text = the whole note source, lines 0-based, lineEnd = the block's last
       non-blank line. From the renderer's own block -> 1-based start line map (main.js preview: pvLines,
       one entry per top-level element), taken when the html went in, so a block a plugin inserts or
       replaces later does not shift the others (a replaced block falls back to its position while the
       pane still has one element per entry). Obsidian Tasks reads it to locate the tasks it re-renders
       in the reading view (its checkbox then writes the done date into that source line). */
    function secMap(P, src, lines) {
      if (typeof src !== "string" || !Array.isArray(lines) || !P || P.children.length !== lines.length) return null;
      const L = src.split("\n"), at = new Map();
      [...P.children].forEach((c, k) => at.set(c, k));
      const span = k => {
        const s = lines[k] - 1;
        let e = (k + 1 < lines.length ? lines[k + 1] - 1 : L.length) - 1;
        while (e > s && !/\S/.test(L[e] || "")) e--;
        return { text: src, lineStart: s, lineEnd: e };
      };
      return el => {
        let n = el;
        while (n && n.parentElement && n.parentElement !== P) n = n.parentElement;
        if (!n || n.parentElement !== P) return null;
        let k = at.get(n);
        if (k === undefined && P.children.length === lines.length) k = [...P.children].indexOf(n);
        return k === undefined || k < 0 ? null : span(k);
      };
    }
    const PP = {   // main.js preview(): PPOST.run(pane, note, tab, src, lines) after the reading html is in
      get gen() { return MDP.gen; },
      run(P, note, tab, src, lines) {
        if (P._ppHold) try { P._ppHold.unload(); } catch (e) { console.error(e); }
        const h = P._ppHold = new Component(); h.load();
        return mdProcess(P, pathOf(note), h, false, null, embPluginExts() ? embHolder(tab, note) : null, secMap(P, src, lines));
      },
      release: embRelease,
    };
    // [pmd:p<post processors>|c<code langs>|r<processed blocks in reading panes>] — only once a plugin used it
    function mdpCensus() {
      if (!MDP.used) return "";
      const r = document.querySelectorAll(".preview [class^='block-language-'], .preview [class*=' block-language-']").length;
      if (!MDP.post.length && !MDP.code.size && !r) return "";
      return " [pmd:p" + MDP.post.length + "|c" + [...MDP.code.keys()].join("+") + "|r" + r + "]" + (typeof Ed !== "undefined" && Ed.cbCensus ? Ed.cbCensus() : "");
    }
    const prop = (C, name, def) => Object.defineProperty(C.prototype, name, {   // see protoField in the editor-api closure
      configurable: true, enumerable: false, get() { return typeof def === "function" ? def() : def; },
      set(x) { Object.defineProperty(this, name, { value: x, writable: true, enumerable: true, configurable: true }); },
    });

    /* MarkdownRenderer: the abstract base of a rendered-markdown child (MarkdownPreviewView
       extends it); the statics render markdown into an element through the app's renderer
       (Rust render), then the registered code-block and post processors. */
    class MarkdownRenderer extends MarkdownRenderChild {
      static async render(app_, markdown, el, sourcePath, component) {
        const html = await K.inv("render", { content: String(markdown == null ? "" : markdown) });
        const t = document.createElement("div"); t.innerHTML = html;
        // plug-val: only the blocks go in, not the newlines between them: stock hands back "happy" as ONE
        // <p> child, and dataview unwraps a lone <p> (childNodes.length == 1) to render an inline field
        // inside a Live Preview line; a trailing "\n" text node left the <p> block and broke the line
        for (const n of [...t.childNodes]) if (n.nodeType === 3 && !/\S/.test(n.data)) n.remove();
        // deep-dataview FD11: a wiki link a plugin renders is a.internal-link[data-href = linktext] as in stock
        // 1.13.7 (dataview result links; plugins select and click them by that class), is-unresolved when the
        // target is missing; a.wiki stays for the app's own link wiring
        for (const a of t.querySelectorAll("a.wiki")) {
          const lt = (a.dataset.note || "") + (a.dataset.anchor ? "#" + a.dataset.anchor : "");
          a.classList.add("internal-link");
          if (a.classList.contains("wiki-unresolved")) a.classList.add("is-unresolved");
          // FD9: an un-aliased heading/block link reads "Note > Heading" as in stock (dataview file.outlinks)
          const nb = (a.dataset.note || "").replace(/\.md$/i, "");
          if (nb && a.dataset.anchor && (a.textContent === lt || a.textContent === nb + "#" + a.dataset.anchor)) a.textContent = nb + " > " + a.dataset.anchor;
          a.setAttribute("data-href", lt);   // href stays "#": a relative href would navigate the webview from a
                                             // plugin view nobody wired (stock routes it through its own handler)
        }
        const added = [...t.childNodes];
        while (t.firstChild) el.appendChild(t.firstChild);
        await mdProcess(el, sourcePath || "", component instanceof Component ? component : new Component(), true, added);
      }
      static renderMarkdown(markdown, el, sourcePath, component) { return MarkdownRenderer.render(K.app, markdown, el, sourcePath, component); }
    }
    prop(MarkdownRenderer, "app", () => K.app);
    prop(MarkdownRenderer, "hoverPopover", null);
    // exported as a value (api.js's fixed list), so it gets the same ES5-callable wrapper api.js gives every
    // exported class here: `_super.call(this, ...)` from a TypeScript-to-ES5 subclass constructs it too
    const MarkdownRendererX = (() => {
      const F = function (...args) { const nt = new.target || (this && this.constructor) || F; return Reflect.construct(MarkdownRenderer, args, nt === F ? MarkdownRenderer : nt); };
      Object.defineProperty(F, "name", { value: "MarkdownRenderer" });
      F.prototype = MarkdownRenderer.prototype; Object.setPrototypeOf(F, MarkdownRenderer);
      return F;
    })();

    /* MarkdownPreviewRenderer: the global post-processor registry (the same one
       Plugin.registerMarkdownPostProcessor feeds; these statics are NOT tied to a plugin's
       lifetime) and the code-block post processor factory. */
    class MarkdownPreviewRenderer {
      static registerPostProcessor(postProcessor, sortOrder) {
        if (typeof postProcessor !== "function") return;
        if (sortOrder !== undefined) postProcessor.sortOrder = sortOrder;
        if (!MDP.post.includes(postProcessor)) { MDP.post.push(postProcessor); mdpChanged(); }
      }
      static unregisterPostProcessor(postProcessor) {
        const i = MDP.post.indexOf(postProcessor);
        if (i >= 0) { MDP.post.splice(i, 1); mdpChanged(); }
      }
      static createCodeBlockPostProcessor(language, handler) {
        const lang = String(language);
        return (el, ctx) => { for (const f of fences(el, lang)) codeBlock(f, handler, ctx || mdCtx("", new Component())); };
      }
    }
    K.exportClasses.MarkdownPreviewRenderer = MarkdownPreviewRenderer;
    Object.assign(K, { MDP, MarkdownRenderChild, MarkdownRenderer: MarkdownRendererX, mdpCensus, mdpChanged });
  });
  reg("editor:editor", function (K) {
    const { host, be } = K;
    const { FileView, TextFileView, touch, MarkdownRenderer, Scope } = K;
    /* editor-api (ui/plugins/editor-api.js): Editor / MarkdownView / EditorSuggest + the editor
       extension compartment, installed ONCE here; the installer is then deleted from window */
    const EA = window.__osEditorApi ? window.__osEditorApi({
      TextFileView, FileView, MarkdownRenderer, Scope,
      app: () => K.app, groups: () => K.allGroups(), focused: () => K.focusedGroup(), leafFor: g => K.leafFor(g), touch: () => touch(),
      fileOf: name => { const f = K.st.modelOn ? K.files.get(name + ".md") : null; return f instanceof K.TFile ? f : null; },
      tokenClassNodeProp: () => (host && typeof host.tokenClassNodeProp === "function" ? host.tokenClassNodeProp() : undefined),   // loader.js: the ONE prop require("@codemirror/language") hands out
    }) : null;
    try { delete window.__osEditorApi; } catch (_) {}
    const MarkdownView = EA ? EA.MarkdownView : class extends TextFileView { getViewType() { return "markdown"; } };
    if (EA) {   // api-parity stage 1 (api-editor): the sub-view classes + livePreviewState, appended after api.js's fixed list
      K.exportClasses.MarkdownEditView = EA.MarkdownEditView;
      K.exportClasses.MarkdownPreviewView = EA.MarkdownPreviewView;
      K.exportValues.livePreviewState = EA.livePreviewState;
    }

    Object.assign(K, { EA, MarkdownView });
  });
})();
