// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* plugin-views (recon T-b): Obsidian's View / ItemView / FileView /
   WorkspaceLeaf, written from the public obsidian-api typings (obsidian.d.ts)
   and plugin source only (clean room).

   Load order (index.html): loader.js -> views.js -> api.js -> main.js.
   This file runs NO code at load beyond defining one factory,
   window.__osPluginViews(deps), which api.js calls ONCE while it builds the
   "obsidian" module and then deletes, so no plugin can reach it. Everything
   these classes need from the plugin host (Component, Events, the DOM helpers,
   the vault model, main.js's tab groups) comes in through `deps`, read at
   CALL time — the same rule api.js keeps with main.js. */
(function () {
  "use strict";
  Object.defineProperty(window, "__osPluginViews", {
    configurable: true, enumerable: false, writable: false,
    value: function viewsFactory(d) {
      const { Component, Events, installDom, mk, setIcon } = d;

      class View extends Component {
        constructor(leaf) {
          super(); installDom();
          this.leaf = leaf; this.app = leaf && leaf.app;
          this.containerEl = mk("div", { cls: "workspace-leaf-content" });
          this.navigation = false; this.icon = "";
          this.scope = null;
        }
        getViewType() { return ""; }
        getDisplayText() { return ""; }
        getIcon() { return this.icon; }
        getState() { return {}; }
        setState() { return Promise.resolve(); }
        getEphemeralState() { return {}; }
        setEphemeralState() {}
        onOpen() { return Promise.resolve(); }
        onClose() { return Promise.resolve(); }
        onResize() {}
        onPaneMenu() {}
        onHeaderMenu() {}
      }
      /* Obsidian's leaf DOM: .workspace-leaf-content > .view-header + .view-content.
         The app's tab strip is the header here, so .view-header only shows once
         the view adds an action (addAction) — its buttons have nowhere else to go. */
      class ItemView extends View {
        constructor(leaf) {
          super(leaf);
          this.headerEl = this.containerEl.createDiv({ cls: "view-header" });
          this.headerEl.hidden = true;
          this.titleEl = this.headerEl.createDiv({ cls: "view-header-title" });
          this.actionsEl = this.headerEl.createDiv({ cls: "view-actions" });
          this.contentEl = this.containerEl.createDiv({ cls: "view-content" });
        }
        addAction(icon, title, cb) {
          const b = mk("a", { cls: "clickable-icon view-action", attr: { "aria-label": title, title } });
          setIcon(b, icon);
          this.actionsEl.prepend(b);
          this.headerEl.hidden = false;
          this.registerDomEvent(b, "click", e => { try { cb(e); } catch (er) { console.error("[plugin] view action:", er); } });
          return b;
        }
      }
      class FileView extends ItemView {
        constructor(leaf) { super(leaf); this.file = null; this.allowNoFile = false; this.navigation = true; }
        getDisplayText() { return this.file ? this.file.basename : "No file"; }
        getState() { return this.file ? { file: this.file.path } : {}; }
        async setState(state) {
          const p = state && state.file, f = p && d.vault().getAbstractFileByPath(p);
          if (!(f instanceof d.TFile()) || f === this.file) return;
          try { await this.loadFile(f); } catch (e) { console.error(e); }
        }
        /* loadFile: the old file is unloaded, then the new one loaded — setState goes through it, so a view that
           overrides loadFile and calls super (Kanban drops its board state there) sees every file change */
        async loadFile(file) {
          if (this.file) try { await this.onUnloadFile(this.file); } catch (e) { console.error(e); }
          this.file = file;
          await this.onLoadFile(file);
        }
        onLoadFile() { return Promise.resolve(); }
        onUnloadFile() { return Promise.resolve(); }
        onRename() { return Promise.resolve(); }
        canAcceptExtension(ext) { return ext === "md"; }
      }
      class EditableFileView extends FileView {}
      /* TextFileView (obsidian.d.ts): the file's text lives in this.data. Opening a file reads it and
         hands it to the subclass with setViewData(data, true) (true = a different file, start clean);
         save() writes the subclass's getViewData() back when it differs from the file on disk;
         requestSave() is that save, debounced 2s; switching or closing the file saves it first, then
         the view's clear(). The view supplies getViewData / setViewData / clear. */
      class TextFileView extends EditableFileView {
        constructor(leaf) {
          super(leaf); this.data = "";
          this.requestSave = d.debounce(() => { this.save().catch(e => console.error("[plugin] TextFileView save:", e)); }, 2000);
        }
        /* an outside change to the open file (the watcher's "modify": another program, a sync tool) reaches the
           view as stock does it: the new text, when it differs from this.data, goes to setViewData(data, false)
           (false = same file, keep the view's state). The view's own save sets this.data before it writes, so its
           own "modify" reads back equal and is a no-op. Armed once, on the first file (Kanban's loadFile calls super). */
        async loadFile(file) {
          if (!this._modRef) {
            this._modRef = d.vault().on("modify", f => { if (f && f === this.file) this._osExtModify(f).catch(e => console.error("[plugin] TextFileView reload:", e)); });
            this.registerEvent(this._modRef);
          }
          return super.loadFile(file);
        }
        async _osExtModify(file) {
          const data = await d.vault().read(file);
          if (this.file !== file || data === this.data) return;
          this.data = data;
          this.setViewData(data, false);
        }
        async onLoadFile(file) {
          const data = await d.vault().read(file);
          if (this.file !== file) return;              // another file was opened while this one was read
          this.data = data;
          this.setViewData(data, true);
        }
        async onUnloadFile(file) {
          await this.save(false, file);
          this.clear();
        }
        async save(clear, file) {
          if (this.requestSave && this.requestSave.cancel) this.requestSave.cancel();
          const f = file || this.file;
          if (f instanceof d.TFile()) {
            const data = this.getViewData();
            if (typeof data === "string") {
              this.data = data;
              if (data !== await d.vault().read(f)) await d.vault().modify(f, data);
            }
          }
          if (clear) this.clear();
        }
        getViewData() { return this.data; }
        setViewData(data) { this.data = data; }
        clear() {}
      }


      /* ------------------------------------------------ the view host
         A plugin view in the MAIN area is a tab of kind "unk" (main.js W4: a
         leaf type the app has no view of its own for) whose utype is the view's
         type. main.js asks PVHOST.mount(g, t) when such a tab becomes active;
         the tab's raw leaf (t.raw) is what workspace.json persists, so a plugin
         view survives a restart and comes back live when its plugin registers
         the type again. Each live tab has a record {t, leaf, view, type, pid};
         the view is loaded and opened once (from the registered factory, or the
         instance WorkspaceLeaf.open was handed), its containerEl lives in the
         group's .content and is shown/hidden. */
      const types = new Map();          // view type -> {factory, pid}
      const recs = new Map();           // tab object -> record
      const tabLeaf = new WeakMap();    // tab object -> WorkspaceLeaf (identity: one leaf per tab)
      const M = () => ({                // main.js top-level names, read at call time
        groups: typeof groups === "function" ? groups : () => [],
        fg: typeof fg === "function" ? fg : () => null,
      });
      const hid = (o, k, v) => Object.defineProperty(o, k, { value: v, writable: true, configurable: true });
      const byId = id => document.getElementById(id);
      const activeTab = g => (g && g.active >= 0 ? g.tabs[g.active] : null);
      const groupOfTab = t => { for (const g of M().groups()) if (g.tabs.indexOf(t) >= 0) return g; return null; };
      const isPvTab = t => !!t && t.kind === "unk" && (recs.has(t) || types.has(t.utype));
      const typeOfTab = t => (!t ? "empty" : t.kind === "unk" ? t.utype : !t.kind ? "markdown" : t.kind === "gg" ? "graph" : t.kind === "lg" ? "localgraph" : String(t.kind));
      const newId = () => { try { if (typeof wsId === "function") return wsId(); } catch (_) {} let s = ""; for (let i = 0; i < 16; i++) s += "0123456789abcdef"[Math.random() * 16 | 0]; return s; };
      const safe = (what, f) => { try { const r = f(); if (r && typeof r.catch === "function") r.catch(e => console.error("[plugin] " + what + ":", e)); return r; } catch (e) { console.error("[plugin] " + what + ":", e); } };
      function leafOfTab(t) { let l = tabLeaf.get(t); if (!l) { l = new WorkspaceLeaf(d.app()); l._t = t; tabLeaf.set(t, l); } return l; }
      function bindLeaf(leaf, t) {      // leaf now IS tab t (a pending leaf landed, or the leaf moved to a new tab)
        const old = leaf._t;
        if (old && old !== t && tabLeaf.get(old) === leaf) tabLeaf.delete(old);
        leaf._t = t; leaf._g0 = null; leaf._nt = false; leaf._split = null;
        if (!t.lid) t.lid = leaf._id;   // the leaf keeps its id (the tab persists it)
        tabLeaf.set(t, leaf);
      }
      function syncTab(r) {             // the tab's label + persisted raw leaf follow the view
        const t = r.t, v = r.view;
        let st = {}; try { st = v.getState() || {}; } catch (_) {}
        let title = ""; try { title = String(v.getDisplayText() || ""); } catch (_) {}
        let icon = ""; try { icon = String(v.getIcon() || ""); } catch (_) {}
        if (!t.lid) t.lid = (t.raw && t.raw.id) || newId();
        const raw = { id: t.lid, type: "leaf", state: { type: r.type, state: JSON.parse(JSON.stringify(st)), icon: icon || undefined, title: title || r.type } };
        if (t.raw && t.raw.group) raw.group = t.raw.group;
        t.raw = raw;
        const name = title || r.type;
        if (t.name !== name) { t.name = name; const g = groupOfTab(t); if (g && typeof renderTabs === "function") try { renderTabs(g); } catch (_) {} }
      }
      function makeView(t, type, state, inst) {   // instantiate (or adopt inst) + load + onOpen, once per tab
        const ty = types.get(type);
        const leaf = leafOfTab(t);
        let view = inst || null;
        if (!view) {
          if (!ty) return null;
          try { view = ty.factory(leaf); } catch (e) { console.error("[plugin] view factory " + type + ":", e); return null; }
          if (!view) return null;
        }
        const r = { t, leaf, view, type, pid: ty ? ty.pid : null, opened: null };
        recs.set(t, r);
        // main.js mounts a plugin-view tab only through PVHOST, which registerView arms; a view handed to
        // WorkspaceLeaf.open needs no registered type, so the first record arms it too (else the tab shows
        // "No view for" and the view's containerEl is never attached)
        try { if (typeof PVHOST !== "undefined" && !PVHOST) PVHOST = PV; } catch (_) {}
        view.containerEl.classList.add("os-pv");
        view.containerEl.dataset.type = type;
        view.containerEl.hidden = true;
        safe("view load " + type, () => view.load());
        r.opened = (async () => {
          try { await view.onOpen(); } catch (e) { console.error("[plugin] view onOpen " + type + ":", e); }
          if (state && Object.keys(state).length) try { await view.setState(state, {}); } catch (e) { console.error("[plugin] view setState " + type + ":", e); }
          syncTab(r); d.touch();
        })();
        syncTab(r);
        return r;
      }
      function closeRec(r) {
        recs.delete(r.t);
        r.view.containerEl.remove();
        safe("view onClose " + r.type, () => r.view.onClose());
        safe("view unload " + r.type, () => r.view.unload());
        d.touch();
      }
      let svsProbe = null, svsSkip = 0;   // top5-func: PV.openAs probe / re-entry guard
      const PV = {
        /* top5-func: a NATIVE note open (explorer row, quick switcher, wikilink: main.js navigate /
           openInTab / openNewTab) asks the plugins first, as stock routes every open through
           WorkspaceLeaf.setViewState. Only when a plugin WRAPPED WorkspaceLeaf.prototype.setViewState
           (Excalidraw: a note whose frontmatter has excalidraw-plugin opens in its "excalidraw" view):
           the wrapper chain runs on a detached probe leaf with {type:"markdown", state:{file}}; the
           type that reaches our own setViewState is the one the plugins picked. "markdown" (or a type
           nobody registered) -> false, the app opens the note as always. Another type -> the file opens
           in that plugin view in a new tab of g; how "replace" (a plain open replaces the active tab)
           then closes the note tab it replaced (not a pinned one, not a special tab). Never re-asked
           while a plugin's own setViewState/openFile is already opening the file (svsSkip), so a
           plugin's "open as markdown" stays markdown. */
        async openAs(g, name, how) {
          if (svsSkip > 0 || !g || WorkspaceLeaf.prototype.setViewState === baseSVS) return false;
          const path = name + ".md", f = d.vault().getAbstractFileByPath(path);
          if (!(f instanceof d.TFile())) return false;
          const probe = new WorkspaceLeaf(d.app()); let want = "";
          svsProbe = { leaf: probe, see: vs => { const ty = vs && vs.type; if (ty && (!want || want === "markdown")) want = String(ty); } };
          try { await probe.setViewState({ type: "markdown", state: { file: path } }); }
          catch (e) { console.error("[plugin] setViewState (open " + path + "):", e); return false; }
          finally { svsProbe = null; }
          if (!want || want === "markdown" || !types.has(want)) return false;
          for (const [t, r] of recs)        // already showing in this pane (a re-click on its row): go there
            if (r.type === want && r.view && r.view.file && r.view.file.path === path && g.tabs.indexOf(t) >= 0) {
              if (typeof switchTab === "function" && g.tabs.indexOf(t) !== g.active) await switchTab(g, g.tabs.indexOf(t));
              return true;
            }
          const old = g.active >= 0 ? g.tabs[g.active] : null;
          const leaf = new WorkspaceLeaf(d.app()); leaf._g = g; leaf._nt = true;
          svsSkip++;
          try { await openTyped(leaf, want, { file: path }, true); } finally { svsSkip--; }
          if (how === "replace" && old && !old.kind && !old.pinned && old !== leaf._t && g.tabs.indexOf(old) >= 0 && typeof closeTab === "function")
            try { await closeTab(g, g.tabs.indexOf(old), "plugin-detach", "plugin"); } catch (e) { console.error(e); }
          return true;
        },
        mount(g, t) {                   // main.js loadActive: t (active, kind unk) -> true if a plugin view shows it
          if (!isPvTab(t)) return false;
          let r = recs.get(t);
          if (!r) { const st = t.raw && t.raw.state; r = makeView(t, t.utype, st && st.state); if (!r) return false; }
          const ce = r.view.containerEl;
          if (ce.parentNode !== g.content) g.content.appendChild(ce);
          for (const x of g.content.querySelectorAll(":scope > .os-pv")) x.hidden = x !== ce;
          ce.hidden = false;
          d.touch();
          return true;
        },
        hide(g, t) {                    // main.js loadActive: whatever g shows next, other plugin views go
          if (!g || !g.content || !recs.size) return;
          const keep = recs.get(t);
          for (const x of g.content.querySelectorAll(":scope > .os-pv")) if (!keep || x !== keep.view.containerEl) x.hidden = true;
        },
        /* main.js cmdSave ("Save current file"): when g's active tab is a plugin view with a file and a
           save(), that save runs (a TextFileView writes its getViewData()) -> true; else false and the
           app saves its own note buffer as before. */
        async save(g, t) {
          const r = isPvTab(t) ? recs.get(t) : null, v = r && r.view;
          if (!v || !v.file || typeof v.save !== "function") return false;
          await v.save();
          return true;
        },
        sideHide(side) { sideHide(side); },        // main.js setRTab / setPane: a built-in sidebar tab shows
        sideShowing(side) { return sideShowing(side); },   // main.js cmdToggleRight: a plugin pane is what reopens
      };
      function sweep() {                // a tab closed / collapsed by the app: its view closes
        if (!recs.size) return;
        const live = new Set();
        for (const g of M().groups()) for (const t of g.tabs) live.add(t);
        for (const [t, r] of [...recs]) if (!live.has(t) && !r.side) closeRec(r);
      }
      async function openTabView(g, t, at, active, guard) {   // insert t into g at `at` (no splice: tabclose census)
        if (typeof flushSave === "function") try { await flushSave(g); } catch (_) {}
        // PR #33 review: `guard` runs after the last await before the switch (nothing can type in between); false = nothing inserted
        if (guard && !guard()) return false;
        g.tabs.push(t);
        if (at < g.tabs.length - 1) { g.tabs.copyWithin(at + 1, at); g.tabs[at] = t; }
        if (active !== false || g.active < 0) { g.active = g.tabs.indexOf(t); if (typeof focusGroup === "function") try { focusGroup(g); } catch (_) {} if (typeof loadActive === "function") await loadActive(g); }
        else if (typeof renderTabs === "function") renderTabs(g);
        return true;
      }
      async function noteSaved(g, t0) {   // true only when t0 provably holds no edits the disk lacks
        // bufDirty/flushSave only see the group's ACTIVE buffer: an inactive note tab whose last save failed may still
        // carry edits nothing here can check or write, so it is never treated as saved (the view opens after it instead)
        if (activeTab(g) !== t0) return false;
        if (typeof bufDirty !== "function" || !bufDirty(g)) return true;
        try { return !!(await flushSave(g)); } catch (_) { return false; }
      }
      async function closeTabOf(t) {
        const g = groupOfTab(t); if (!g) { const r = recs.get(t); if (r) closeRec(r); return; }
        const i = g.tabs.indexOf(t);
        if (typeof closeTab === "function") await closeTab(g, i, "plugin-detach", "plugin");
        const r = recs.get(t); if (r) closeRec(r);
      }
      const pvTab = (type, name) => ({ kind: "unk", name: name || type, utype: type, raw: null, lid: newId(), mode: "source", hist: [], hpos: -1 });
      /* CVR-B-027: a canvas tab that opens a note in its own leaf (a link in a selected text card, at rest)
         becomes that note IN PLACE, as stock replaces the leaf's view: the tab count is unchanged and the
         board is the leaf's Back entry ({pv: {type, state}}, as main.js keeps a graph as {gg:1}). Back on
         that entry turns the tab into the canvas again, re-opened from its saved view state; Forward from
         there turns it into the note again. Only the canvas type: other plugin views keep opening a note
         in a new tab of their group (Excalidraw and friends rely on it). */
      const PV_INPLACE = new Set(["canvas"]);
      function pvToNote(leaf, file) {   // leaf's canvas tab -> a note tab with the board as its Back entry; true if done
        const t = leaf._t, g = t && groupOfTab(t);
        if (!g || leaf._side || !PV_INPLACE.has(t.utype) || !file || file.extension !== "md" || typeof modeBits !== "function") return false;
        const r = recs.get(t);
        let st = {}; try { st = (r ? r.view.getState() : t.raw && t.raw.state && t.raw.state.state) || {}; } catch (_) {}
        const e = { n: t.name, s: 0, pv: { type: t.utype, state: JSON.parse(JSON.stringify(st)), title: t.name } };
        t.hist = t.hpos >= 0 ? t.hist.slice(0, t.hpos + 1) : [];
        t.hist.push(e); t.hpos = t.hist.length - 1;
        if (r) closeRec(r);
        pvAsNote(t);
        pvHistInstall();
        return true;
      }
      function pvAsNote(t) {            // the tab is a note tab now: no plugin kind, live preview bits
        delete t.kind; delete t.utype; delete t.raw; delete t.base; delete t.mode;
        t.read = false; t.src = false; modeBits(t);
      }
      function pvAsView(t, e) {         // the tab is the plugin view of history entry e again (mounted by loadActive)
        delete t.base;
        t.kind = "unk"; t.utype = e.pv.type; t.name = e.n; t.read = false; t.src = true;
        t.raw = { id: t.lid, type: "leaf", state: { type: e.pv.type, state: JSON.parse(JSON.stringify(e.pv.state || {})), title: e.pv.title || e.n } };
      }
      /* main.js histGo walks note/graph entries only; a {pv} entry (or a tab standing on one) is walked here,
         every other step goes to histGo unchanged. Installed on the first in-place open, so a session that
         never makes one runs main.js's history exactly as before. */
      let pvHistOn = false;
      function pvHistInstall() {
        if (pvHistOn || typeof histGo !== "function") return;
        pvHistOn = true;
        const base = histGo;
        window.histGo = async function pvHistGo(dd, from) {
          const g = from || M().fg(), t = activeTab(g);
          const at = t && t.hist && t.hpos >= 0 ? t.hist[t.hpos] : null, p = t ? t.hpos + dd : -1;
          const to = t && t.hist && p >= 0 && p < t.hist.length ? t.hist[p] : null;
          if (!to || t.kind === "lg" || (!(to.pv) && !(t.kind === "unk" && at && at.pv))) return base(dd, from);
          if (to.pv) {                  // onto a canvas entry: leave the note (saved), the tab is the canvas again
            if (t.kind === "unk") { const r = recs.get(t); if (r) closeRec(r); }
            else { if (typeof flushSave === "function") await flushSave(g); if (typeof scrollOf === "function" && at) at.s = scrollOf(g); }
            t.hpos = p; pvAsView(t, to);
            if (typeof loadActive === "function") await loadActive(g);
            return;
          }
          const r = recs.get(t); if (r) closeRec(r);   // off a canvas entry onto a note: histGo walks it as a note tab
          pvAsNote(t);
          return base(dd, from);
        };
      }

      /* ------------------------------------------- the layout tree (api-workspace)
         Obsidian's layout classes over main.js's own model, resolved LIVE at
         read time (nothing is mirrored, so nothing goes stale):
           rootSplit   WorkspaceRoot   main.js state.root (whatever node is the
                                       root right now; a new object after
                                       changeLayout, as stock's is)
           split node  WorkspaceSplit  one item per node object of the split tree
           group       WorkspaceTabs   one item per tab group; children = its leaves
           leaf        WorkspaceLeaf   one per TAB (leafOfTab); an empty group has one
           left/right  WorkspaceSidedock  the two sidebars (#side / #rside);
                       collapse/expand/toggle drive the app's own sidebar toggles
         getRoot walks .parent (never calls getRoot again), getContainer is the
         main root for anything outside a container (a sidedock, as in stock). */
      const S = () => { try { return typeof state !== "undefined" && state ? state : null; } catch (_) { return null; } };
      const parentNodeOf = x => { const s = S(); return s && typeof findParent === "function" ? findParent(s.root, x) : null; };
      const inLayout = x => { const s = S(); return !!s && (x === s.root || !!parentNodeOf(x)); };
      class WorkspaceItem extends Events {
        constructor() { super(); hid(this, "_k", null); hid(this, "_n", null); hid(this, "_fp", null); }
        get parent() { return itemParent(this); }
        get containerEl() { return itemEl(this); }
        getRoot() { let x = this; for (let p = x.parent; p; p = p.parent) x = p; return x; }
        getContainer() { const r = rootOf(this); return r instanceof WorkspaceContainer ? r : mainRoot(); }
      }
      class WorkspaceParent extends WorkspaceItem { get children() { return itemChildren(this); } }
      class WorkspaceSplit extends WorkspaceParent {
        /* new WorkspaceSplit(workspace, direction) from a plugin: a DETACHED split (not in the app layout) that
           holds the items a plugin inserts — Excalidraw hosts its canvas leaf in one (CanvasNodeFactory). The app
           builds its own splits with no arguments and binds them to a layout node (itemOfNode). */
        constructor(ws, dir) {
          super();
          if (arguments.length) {
            const el = document.createElement("div"), v = dir === "horizontal" ? "horizontal" : "vertical";
            el.className = "workspace-split mod-" + v;
            this._k = "free"; this._n = { children: [], el, direction: v };
          }
        }
        insertChild(index, child) {
          if (this._k !== "free") throw new Error("WorkspaceSplit.insertChild: the app layout changes through workspace.getLeaf / splitActiveLeaf; only a split built with new WorkspaceSplit() takes inserted items");
          if (!(child instanceof WorkspaceItem)) throw new TypeError("WorkspaceSplit.insertChild: not a workspace item");
          if (child._fp) child._fp.removeChild(child);
          const c = this._n.children, i = Math.max(0, Math.min(c.length, Number.isFinite(+index) ? Math.trunc(+index) : c.length));
          c.splice(i, 0, child); child._fp = this;
        }
        removeChild(child) {
          if (this._k !== "free") throw new Error("WorkspaceSplit.removeChild: only a split built with new WorkspaceSplit() gives up items here");
          const c = this._n.children, i = c.indexOf(child);
          if (i >= 0) { c.splice(i, 1); child._fp = null; }
        }
        get direction() { if (this._k === "free") return this._n.direction; const n = this._k === "split" ? this._n : this._k === "root" ? (S() || {}).root : null; return n && n.dir === "col" ? "horizontal" : "vertical"; }
      }
      class WorkspaceContainer extends WorkspaceSplit {
        get doc() { const el = this.containerEl; return (el && el.ownerDocument) || document; }
        get win() { return this.doc.defaultView || window; }
      }
      class WorkspaceRoot extends WorkspaceContainer {}
      class WorkspaceWindow extends WorkspaceContainer {}      // a popout window's root: never built here (popouts are design-gated)
      class WorkspaceFloating extends WorkspaceParent {}       // the popout windows' parent: never built here
      class WorkspaceTabs extends WorkspaceParent {
        get currentTab() { const g = this._k === "tabs" ? this._n : null; return g ? Math.max(0, g.active) : 0; }
      }
      const sideIsOpen = side => { try { return side === "left" ? !!sideOpen : !!rightOpen; } catch (_) { return false; } };
      function sideToggle(side) {       // the app's own toggles (they flip the flag synchronously, then paint)
        const f = side === "left" ? (typeof cmdToggleSide === "function" ? cmdToggleSide : null) : (typeof cmdToggleRight === "function" ? cmdToggleRight : null);
        if (f) safe("sidedock " + side, () => f());
      }
      class WorkspaceSidedock extends WorkspaceSplit {
        get direction() { return "horizontal"; }
        get collapsed() { return !sideIsOpen(this._n); }
        collapse() {
          if (this.collapsed) return;
          const al = d.activeLeaf();    // focus leaves a sidebar that is going away
          if (al && al.getRoot() === this) { const g = M().fg() || M().groups()[0]; if (g && typeof focusGroup === "function") try { focusGroup(g); } catch (_) {} }
          sideToggle(this._n);
        }
        expand() { if (this.collapsed) sideToggle(this._n); }
        toggle() { if (this.collapsed) this.expand(); else this.collapse(); }
      }
      class WorkspaceRibbon {
        constructor(side) { hid(this, "_side", side === "right" ? "right" : "left"); this.collapsed = false; }
        get ribbonItemsEl() { return this._side === "left" ? byId("ribbon") : null; }
        // read-only view of plugin ribbon icons (cmdr's "Left Ribbon" tab maps it); a copy, so remove() on it can't detach app buttons
        get items() { return this._side !== "left" ? [] : [...document.querySelectorAll("#ribbon .os-plugin-ribbon")].map(b => ({ id: (b.dataset.plugin || "") + ":" + b.title, title: b.title, icon: (b.firstElementChild && b.firstElementChild.dataset.icon) || "", buttonEl: b })); }
      }
      let rootGen = 0, rootItem = null;
      function mainRoot() { if (!rootItem || rootItem._gen !== rootGen) { rootItem = new WorkspaceRoot(); rootItem._k = "root"; hid(rootItem, "_gen", rootGen); } return rootItem; }
      const splitItems = new WeakMap(), tabsItems = new WeakMap(), docks = {}, dockTabs = {}, ribbons = {};
      function tabsOf(g) { let it = tabsItems.get(g); if (!it) { it = new WorkspaceTabs(); it._k = "tabs"; it._n = g; tabsItems.set(g, it); } return it; }
      function itemOfNode(n) {          // a node of main.js's split tree -> its live item
        if (!n) return null;
        const s = S(); if (s && n === s.root) return mainRoot();
        if (!n.children) return tabsOf(n);
        let it = splitItems.get(n); if (!it) { it = new WorkspaceSplit(); it._k = "split"; it._n = n; splitItems.set(n, it); }
        return it;
      }
      function dock(side) { if (!docks[side]) { const it = new WorkspaceSidedock(); it._k = "dock"; it._n = side; docks[side] = it; } return docks[side]; }
      function dockTabsOf(side) { if (!dockTabs[side]) { const it = new WorkspaceTabs(); it._k = "docktabs"; it._n = side; dockTabs[side] = it; } return dockTabs[side]; }
      function ribbon(side) { return ribbons[side] || (ribbons[side] = new WorkspaceRibbon(side)); }
      /* workspace.floatingSplit: the parent of the popout windows (stock: a WorkspaceFloating with no parent,
         getRoot() itself, getContainer() the main root, a detached DIV). Popouts are design-gated, so it never
         gets a child: one stable item, its children one stable empty array. */
      let floatItem = null;
      function floating() { if (!floatItem) { floatItem = new WorkspaceFloating(); floatItem._k = "floating"; const el = document.createElement("div"), h = el.appendChild(document.createElement("hr")); h.className = "workspace-leaf-resize-handle"; floatItem._n = { children: [], el }; hid(floatItem, "resizeHandleEl", h); } return floatItem; }
      function itemParent(it) {
        if (it._fp) return it._fp;      // an item a plugin inserted into its own detached split
        switch (it._k) {
          case "split": case "tabs": { if (!inLayout(it._n)) return null; const p = parentNodeOf(it._n); return p ? itemOfNode(p) : null; }
          case "docktabs": return dock(it._n);
          default: return null;         // the root, a sidedock, an item never placed in the layout
        }
      }
      function itemChildren(it) {
        const s = S();
        switch (it._k) {
          case "root": return s ? s.root.children.map(itemOfNode) : [];
          case "split": return (it._n.children || []).map(itemOfNode);
          case "tabs": { const g = it._n; return g.tabs.length ? g.tabs.map(leafOfTab) : [d.groupLeaf(g)]; }
          case "dock": return [dockTabsOf(it._n)];
          case "docktabs": return sides.filter(r => r.side === it._n).map(r => r.leaf);
          case "floating": return it._n.children;
          case "free": return it._n.children.slice();
          default: return [];
        }
      }
      function itemEl(it) {
        switch (it._k) {
          case "root": return byId("main");
          case "split": return it._n.el || null;
          case "tabs": return it._n.pane || null;
          case "dock": case "docktabs": return byId(it._n === "left" ? "side" : "rside");
          case "floating": return it._n.el;
          case "free": return it._n.el;
          default: return null;
        }
      }
      function rootOf(it) { let x = it; for (let p = x.parent; p; p = p.parent) x = p; return x; }
      function leafParent(l) {
        if (l._fp) return l._fp;
        if (l._side) return sideRec(l) ? dockTabsOf(l._side) : null;
        if (l._t) { const g = groupOfTab(l._t); return g ? tabsOf(g) : null; }
        if (l._split) return null;      // a pending split: its group does not exist yet
        const g = l._g0; return g && inLayout(g) ? tabsOf(g) : null;
      }

      /* leaf groups (setGroup / getGroupLeaves): a tab leaf's group IS the app's
         link group (R13, t.link), so grouped leaves are linked tabs that follow
         each other; the 16-hex string is the same `group` value workspace.json
         carries for that link (wsGrp). A leaf that is not a tab keeps it alone. */
      function leafGroup(l) {
        const t = l._t;
        if (!t) return l._gs;
        if (t.link == null || typeof wsGrp !== "object" || typeof wsIdOf !== "function") return null;
        return wsIdOf(wsGrp, "L" + t.link);
      }
      function setLeafGroup(l, hex) {
        hex = hex == null || hex === "" ? null : String(hex);
        const t = l._t;
        if (!t || typeof wsGrp !== "object" || typeof linkSeq !== "number") { l._gs = hex; return; }
        if (t.link != null) {           // leave the current link group; a group of one dissolves (R13.4)
          const L = t.link; delete t.link;
          const rest = []; for (const g of M().groups()) for (const x of g.tabs) if (x.link === L) rest.push(x);
          if (rest.length < 2) for (const x of rest) delete x.link;
        }
        if (hex) {
          let L = null;
          for (const k of Object.keys(wsGrp)) if (k[0] === "L" && wsGrp[k] === hex) { L = Number(k.slice(1)); break; }
          if (L == null || !Number.isFinite(L)) { linkSeq = linkSeq + 1; L = linkSeq; wsGrp["L" + L] = hex; }
          t.link = L;
        }
        if (typeof renderTabs === "function") for (const g of M().groups()) try { renderTabs(g); } catch (_) {}
        d.touch();
      }

      /* A leaf. Shapes (hidden fields, never enumerable):
           tab leaf    _t    one tab of a group, any kind (identity per tab: leafOfTab);
                             a plugin view's tab has a record in recs
           group leaf  _g0   an EMPTY tab group (no tab to be)
           pending     _g0 + _nt (new tab) | _split ("row"|"col"): getLeaf(true|'tab'|'split')
                       — becomes a tab leaf when something opens in it
           side leaf   _side "right"|"left" (getRightLeaf/getLeftLeaf) */
      class WorkspaceLeaf extends WorkspaceItem {
        constructor(app) {
          super(); this.app = app;
          for (const k of ["_g0", "_v", "_t", "_split", "_side", "_gs"]) hid(this, k, null);
          hid(this, "_nt", false); hid(this, "_id", newId());
          this.hoverPopover = null;
        }
        get _g() { return this._t ? groupOfTab(this._t) : this._g0; }
        set _g(g) { this._g0 = g; }
        get _grp() { return this._g; }
        get id() { const t = this._t; return t ? (t.lid || (t.lid = this._id)) : this._id; }
        get view() {
          if (this._side) { const r = sideRec(this); return r ? r.view : emptyView(this); }
          const t = this._t;
          if (t) { const r = recs.get(t); if (r) return r.view; if (t.kind === "unk") return emptyView(this); }
          else if (this._nt || this._split) return emptyView(this);
          else if (!this._g0) return emptyView(this);   // a bare leaf a plugin built (new WorkspaceLeaf): not a tab, not a pane
          return d.leafView(this);
        }
        get containerEl() { return this.view.containerEl; }
        get parent() { return leafParent(this); }
        get group() { return leafGroup(this); }
        getViewState() {
          const r = this._t ? recs.get(this._t) : sideRec(this);
          if (r) { let st = {}; try { st = r.view.getState() || {}; } catch (_) {} return { type: r.type, state: st }; }
          if (this._t && this._t.kind === "unk") { const s = this._t.raw && this._t.raw.state; return { type: this._t.utype, state: (s && s.state) || {} }; }
          if (this._side || this._nt || this._split) return { type: "empty", state: {} };
          return d.leafState(this);
        }
        async setViewState(vs, eState) {
          if (svsProbe && svsProbe.leaf === this) { svsProbe.see(vs); return; }   // PV.openAs: the type the wrappers picked
          const type = vs && vs.type;
          if (type && types.has(type)) return openTyped(this, type, (vs && vs.state) || {}, vs.active);
          if (this._side && (type === "empty" || !type)) return;
          const p = vs && vs.state && vs.state.file; const f = p && d.vault().getAbstractFileByPath(p);
          if (!(f instanceof d.TFile())) return;
          return this._openFileDirect(f, { state: vs.state, eState, active: vs.active });
        }
        /* openFile goes through setViewState like stock: {type: the view type registered for the file's
           extension (md -> "markdown"), state: {...openState.state, file}}. Plugins that pick the view
           for a file by wrapping WorkspaceLeaf.prototype.setViewState (Excalidraw turns a markdown
           drawing into its "excalidraw" view this way) see every file open; "markdown" (or a type
           nothing registered) falls through to the app's own note open. */
        openFile(file, openState) {
          const os = openState || {};
          if (!(file instanceof d.TFile())) return this._openFileDirect(file, os);
          const reg = this.app && this.app.viewRegistry;
          const type = (reg && typeof reg.getTypeByExtension === "function" && reg.getTypeByExtension(file.extension)) || (file.extension === "md" ? "markdown" : "");
          if (!type) return this._openFileDirect(file, os);
          return this.setViewState({ type, state: Object.assign({}, os.state, { file: file.path }), active: os.active }, os.eState);
        }
        async _openFileDirect(file, openState) { svsSkip++; try { return await this._openFileDirect0(file, openState); } finally { svsSkip--; } }
        _openFileDirect0(file, openState) {
          if (this._t && this._t.kind === "unk" && !this._side && pvToNote(this, file)) return d.openFileIn(this, file, openState);   // CVR-B-027: a canvas leaf shows the note in place
          if ((this._t && this._t.kind === "unk") || this._side) {   // a plugin leaf asked to show a note: a new tab in its group (or the focused one)
            const n = new WorkspaceLeaf(this.app); n._g = this._grp || M().fg(); n._nt = true;
            return d.openFileIn(n, file, openState);
          }
          return d.openFileIn(this, file, openState);
        }
        async open(view) {              // show a View the caller built (for this leaf) in this leaf
          if (!(view instanceof View)) throw new TypeError("WorkspaceLeaf.open: expected a View");
          let type = ""; try { type = String(view.getViewType() || ""); } catch (_) {}
          // the view shows in this leaf, but the active leaf stays where it was (stock): a split leaf's new
          // pane does not take focus, so a view that follows the active note (mind-map) still sees the note
          await openTyped(this, type || "empty", null, true, view, true);
          return view;
        }
        getDisplayText() { const v = this.view; try { const s = v && v.getDisplayText(); if (s) return s; } catch (_) {} return ""; }
        getIcon() { const v = this.view; try { const s = v && v.getIcon(); if (s) return s; } catch (_) {} return "file"; }
        updateHeader() { const r = this._t ? recs.get(this._t) : sideRec(this); if (r) { if (r.t) syncTab(r); else sideSync(r); } }
        setPinned(p) { if (this._t) { this._t.pinned = !!p; const g = this._grp; if (g && typeof renderTabs === "function") renderTabs(g); } }
        togglePinned() { if (this._t) this.setPinned(!this._t.pinned); }
        get pinned() { return !!(this._t && this._t.pinned); }
        setGroup(group) { setLeafGroup(this, group); }
        setGroupMember(other) {
          if (!(other instanceof WorkspaceLeaf) || other === this) return;
          let g = other.group; if (g == null) { g = newId(); other.setGroup(g); }
          this.setGroup(g);
        }
        detach() {
          if (this._fp) { this._fp.removeChild(this); return; }
          if (this._side) return sideDetach(this);
          if (this._t) return closeTabOf(this._t);
          const g = this._g0;               // an empty group's leaf: the empty pane goes (never the last one)
          if (g && !this._nt && !this._split && !g.tabs.length && M().groups().length > 1 && inLayout(g) && typeof collapseGroup === "function") return collapseGroup(g, "plugin-detach");
        }
        getEphemeralState() { const v = this.view; try { return (v && v.getEphemeralState && v.getEphemeralState()) || {}; } catch (_) { return {}; } }
        setEphemeralState(s) { const v = this.view; try { if (v && v.setEphemeralState) v.setEphemeralState(s); } catch (_) {} }
        loadIfDeferred() { return Promise.resolve(); }
        get isDeferred() { return false; }
        onResize() { const v = this.view; try { if (v && v.onResize) v.onResize(); } catch (_) {} }
        /* rebuildView: the leaf's view is built again from its own view state (Dataview's "Rebuild
           current view"). A plugin view is closed (onClose/unload) and a fresh instance of its type
           opened with getState(); a note re-renders its reading pane from scratch (rerender(true))
           and re-applies the editor/scroll ephemeral state. */
        async rebuildView() {
          const es = this.getEphemeralState();
          const r = this._t ? recs.get(this._t) : sideRec(this);
          if (r) {
            let st = {}; try { st = r.view.getState() || {}; } catch (_) {}
            const type = r.type;
            if (this._side) { sideClose(r); const n = sideMount(this, type, st); if (n) { await n.opened; sideShow(n); } }
            else {
              const t = this._t, g = groupOfTab(t);
              closeRec(r); const n = makeView(t, type, st); if (!n) return;
              if (g && activeTab(g) === t && typeof loadActive === "function") await loadActive(g);
              await n.opened;
            }
          } else {
            const v = this.view, pm = v && v.previewMode;
            if (pm && typeof pm.rerender === "function") pm.rerender(true);
            try { if (typeof Ed !== "undefined" && Ed && Ed.cbRebuild) Ed.cbRebuild(v && v._tv); } catch (e) { console.error(e); }
          }
          this.setEphemeralState(es);
        }
      }
      const baseSVS = WorkspaceLeaf.prototype.setViewState;   // PV.openAs: unwrapped -> no plugin routes opens
      const emptyOf = new WeakMap();
      function emptyView(leaf) { let v = emptyOf.get(leaf); if (!v) { v = new View(leaf); v.getViewType = () => (leaf._t ? leaf._t.utype : "empty"); emptyOf.set(leaf, v); } return v; }

      async function openTyped(leaf, type, state, active, inst, keepFocus) {
        if (leaf._side) return sideOpenView(leaf, type, state, active, inst);
        const t0 = leaf._t;
        if (t0 && t0.kind === "unk") {  // a plugin-view tab: same type -> new state; other type / a given view -> a new view in the same tab
          const t = t0, g = groupOfTab(t);
          let r = recs.get(t);
          if (r && r.type === type && !inst) { await r.opened; try { await r.view.setState(state, {}); } catch (e) { console.error(e); } syncTab(r); }
          else {
            if (r) closeRec(r);
            t.utype = type; r = makeView(t, type, state, inst); if (!r) return;
            if (g && activeTab(g) === t && typeof loadActive === "function") await loadActive(g);
            await r.opened;
          }
          if (active && g) await reveal(leaf);
          return;
        }
        const g = leaf._g || M().fg(); if (!g) return;
        const t = pvTab(type);
        let at = g.active >= 0 ? g.active + 1 : g.tabs.length;
        if (t0) { const i = g.tabs.indexOf(t0); if (i >= 0) at = i + 1; }
        const split = leaf._split;
        // the leaf moves to the new tab: a pending leaf lands, a note/graph tab's leaf follows its view
        // (the note tab stays, as before; it gets a leaf of its own on next use); a stale group leaf mirrors
        if (t0 || leaf._nt || split || !g.tabs.length) bindLeaf(leaf, t);
        const r = makeView(t, type, state, inst); if (!r) return;
        if (split && typeof splitWith === "function") {
          const prev = keepFocus ? M().fg() : null;   // splitWith focuses the new pane; WorkspaceLeaf.open gives focus back
          await splitWith(g, split, t);
          if (prev && prev !== M().fg() && M().groups().indexOf(prev) >= 0 && typeof focusGroup === "function") try { focusGroup(prev); } catch (_) {}
        }
        // PR #33 review: a PINNED note leaf is never replaced (as getLeaf and the "replace" open above): the view opens in a
        // new tab after it and the note stays
        // PR #33 review: the note goes with no flush of its own (closeTab noFlush, and it is no longer the active tab), so
        // its edits are written first; a write that fails keeps the note (the view opens after it, as on a pinned note)
        else if (t0 && !t0.kind && !t0.pinned && active && !leaf._nt && (PV_INPLACE.has(type) || (state && state.file))) {
          // cvf-links CVR-A-096: a canvas opened from a NOTE leaf (a [[X.canvas]] link / embed click) replaces the
          // note in the same tab, as stock: the canvas tab goes in right after it, then the note tab goes (count unchanged).
          // Any file view opened on the note's own leaf does the same (Excalidraw "create new drawing - in the current
          // active window" turns the note leaf into its drawing view: stock keeps one tab, not the note plus the drawing).
          // PR #33 review: the save captures the buffer before its I/O, so keys typed while it is in flight are not in it:
          // the note goes only if it is still the shown tab and clean at the switch itself (the guard runs after the last await)
          if (await noteSaved(g, t0) && await openTabView(g, t, at, true, () => activeTab(g) === t0 && !(typeof bufDirty === "function" && bufDirty(g)))) {
            const j = g.tabs.indexOf(t0);
            if (j >= 0 && typeof closeTab === "function") await closeTab(g, j, "plugin-detach", "plugin", true);
          }
          // PR #33 review: not saved (F1 banner up) or edited since: the note stays the ACTIVE tab, dirty where the next
          // save, the dirty census and leaveVault/quit (active tabs only) see it; the view opens after it, not shown.
          // A note that was not the shown tab (noteSaved false, e6aec6f7) keeps it as before: the view opens after it, shown
          else await openTabView(g, t, at, activeTab(g) !== t0);
        }
        else if (t0 && !active && !leaf._nt) {
          // a leaf already showing something changes view in place: the active leaf stays where it is (stock);
          // the leaf's tab stays the one its group shows if it was, without focusing that group
          // (the insert shifts the tabs after `at`: the shown tab is re-found by identity BEFORE anything
          // renders, else a render in between sees the index on a neighbour and makes that the active leaf)
          const cur = activeTab(g), was = !cur || cur === t0;
          if (was && typeof flushSave === "function") try { await flushSave(g); } catch (_) {}
          g.tabs.push(t);
          if (at < g.tabs.length - 1) { g.tabs.copyWithin(at + 1, at); g.tabs[at] = t; }
          g.active = g.tabs.indexOf(was ? t : cur);
          if (was && typeof loadActive === "function") await loadActive(g);
          else if (typeof renderTabs === "function") renderTabs(g);
        }
        else await openTabView(g, t, at, true);
        await r.opened;
        d.touch();
      }
      async function reveal(leaf) {
        if (leaf._side) return sideReveal(leaf);
        const g = leaf._grp; if (!g) return;
        if (typeof focusGroup === "function") try { focusGroup(g); } catch (_) {}
        if (leaf._t) { const i = g.tabs.indexOf(leaf._t); if (i >= 0 && i !== g.active && typeof switchTab === "function") await switchTab(g, i); }
        d.touch();
      }
      function leafOfGroup(g) {         // the group's leaf: its ACTIVE tab's (an empty group has one of its own)
        const t = activeTab(g);
        return t ? leafOfTab(t) : d.groupLeaf(g);
      }
      function mainLeaves() {
        const out = [];
        for (const g of M().groups()) { if (g.tabs.length) for (const t of g.tabs) out.push(leafOfTab(t)); else { const l = d.groupLeaf(g); if (l) out.push(l); } }
        return out;
      }
      function leavesOfType(type) {
        const out = [];
        for (const g of M().groups()) {
          for (const t of g.tabs) if (typeOfTab(t) === type) out.push(leafOfTab(t));
          if (!g.tabs.length && type === "empty") { const l = d.groupLeaf(g); if (l) out.push(l); }
        }
        for (const s of sideRecs()) if (s.type === type) out.push(s.leaf);
        return out;
      }
      function iterateLeaves(cb, withSide) {
        for (const l of mainLeaves()) if (cb(l) === true) return true;
        if (withSide) for (const s of sideRecs().slice()) if (cb(s.leaf) === true) return true;
        return false;
      }
      function getLeaf(newLeaf, dir) {
        const g = M().fg();
        if (!g) return null;
        if (!newLeaf) return null;      // caller (Workspace.getLeaf) answers false with the active leaf
        const n = new WorkspaceLeaf(d.app()); n._g = g;
        if (newLeaf === "split") n._split = dir === "horizontal" ? "col" : "row";   // Obsidian's word is the opposite of ours
        else n._nt = true;               // true | "tab" | "window" (no windows here: a tab)
        return n;
      }
      /* app.viewRegistry: the registry of view types and the file extensions they open (an Events:
         "view-registered"(type), "view-unregistered"(type), "extensions-updated"()). It is the source of
         truth for "is this type registered"; `types` beside it keeps the owning plugin and outlives an
         unregister until that type's leaves are detached. Lookups are case-sensitive. The app
         opens markdown only, so the built-in extension map is {md: "markdown"}; viewByType holds the types
         registered through it (the app's own panes are not creatable views). The two getters are plain
         property reads (as stock: an inherited name answers), isExtensionRegistered and the duplicate checks own keys. */
      const own = (o, k) => k != null && Object.prototype.hasOwnProperty.call(o, k);
      class ViewRegistry extends Events {
        constructor() { super(); this.viewByType = {}; this.typeByExtension = { md: "markdown" }; }
        registerView(type, creator) { return addView(null, type, creator); }
        unregisterView(type) { dropView(type); }
        registerExtensions(exts, type) {
          // all or nothing (stock: a batch with one registered extension registers none of it)
          for (const e of exts) if (own(this.typeByExtension, e)) throw new Error("Attempting to register an existing file extension \"" + e + "\"");
          for (const e of exts) this.typeByExtension[e] = type;
          this.trigger("extensions-updated");
        }
        unregisterExtensions(exts) { for (const e of exts) if (own(this.typeByExtension, e)) delete this.typeByExtension[e]; this.trigger("extensions-updated"); }
        registerViewWithExtensions(exts, type, creator) { this.registerView(type, creator); this.registerExtensions(exts, type); }
        isExtensionRegistered(ext) { return own(this.typeByExtension, ext); }
        getTypeByExtension(ext) { return this.typeByExtension[ext]; }
        getViewCreatorByType(type) { return this.viewByType[type]; }
      }
      const viewRegistry = new ViewRegistry();
      function addView(pid, type, factory) {
        type = String(type);
        if (own(viewRegistry.viewByType, type)) throw new Error("Attempting to register an existing view type \"" + type + "\"");
        viewRegistry.viewByType[type] = factory;
        types.set(type, { factory, pid });
        // main.js mounts a plugin-view tab only through PVHOST: a type registered here (by a plugin or straight
        // on the registry) arms it
        try { if (typeof PVHOST !== "undefined" && !PVHOST) PVHOST = PV; } catch (_) {}
        // tabs restored from workspace.json (kind unk) of this type come alive; a SHOWING one re-mounts now
        for (const g of M().groups()) { const t = activeTab(g); if (t && t.kind === "unk" && t.utype === type && typeof loadActive === "function") safe("remount " + type, () => loadActive(g)); }
        viewRegistry.trigger("view-registered", type);
        d.touch();
      }
      function dropView(type) {         // the registry forgets now; the type's leaves detach after (Obsidian detaches them)
        type = String(type);
        if (!own(viewRegistry.viewByType, type)) return null;
        delete viewRegistry.viewByType[type];
        const ty = types.get(type);
        viewRegistry.trigger("view-unregistered", type);
        return (async () => {
          const leaves = leavesOfType(type);
          for (const l of leaves) { try { await l.detach(); } catch (e) { console.error(e); } }
          if (types.get(type) !== ty) return;   // registered again meanwhile: the new registration stands
          types.delete(type);
          for (const [, r] of [...recs]) if (r.type === type) closeRec(r);
          d.touch();
        })();
      }
      function registerView(pid, type, factory) { addView(pid, type, factory); }
      async function unregisterView(type) { await dropView(type); }   // plugin unload: its views go

      /* sidebars (getRightLeaf / getLeftLeaf): a plugin view gets its own icon
         button in the sidebar's strip (#rtabs before #rtoggle / #stabs) and its
         own .spane in #rside / #side. Showing it hides the built-in panes; a
         built-in tab click (setRTab / setPane) calls PV.sideHide(side) back.
         Side views are NOT persisted in workspace.json: plugins re-create them
         on onLayoutReady (Obsidian plugins all do: ensureSideLeaf / getRightLeaf). */
      const sides = [];                 // side records {leaf, view, type, pid, side, opened, btn, pane}
      const sideRecs = () => sides;
      const sideRec = leaf => sides.find(s => s.leaf === leaf) || null;
      const SIDE = {
        right: { strip: "rtabs", host: "rside", before: "rtoggle", ids: ["rpane-bl", "rpane-out", "rpane-tags", "rpane-toc"], tab: "rtab-" },
        left:  { strip: "stabs", host: "side", before: null, ids: ["pane-files", "pane-search", "pane-bm"], tab: "stab-" },
      };
      function sideSync(r) {            // the strip button's icon + tooltip follow the view
        let title = ""; try { title = String(r.view.getDisplayText() || ""); } catch (_) {}
        let icon = ""; try { icon = String(r.view.getIcon() || ""); } catch (_) {}
        r.btn.title = title || r.type; r.btn.setAttribute("aria-label", r.btn.title);
        if (r.icon !== icon) { r.icon = icon; r.btn.textContent = ""; try { setIcon(r.btn, icon || "box"); } catch (_) {} }
      }
      function sideShowing(side) { return sides.some(s => s.side === side && !s.pane.hidden); }
      function sideHide(side) {         // a built-in tab shows: every plugin pane of that side hides
        if (!sides.length) return;
        for (const s of sides) if (s.side === side) { s.pane.hidden = true; s.btn.classList.remove("active"); }
        d.touch();
      }
      function sideShow(r) {
        const c = SIDE[r.side];
        for (const id of c.ids) { const p = byId(id); if (p) p.hidden = true; }
        for (const b of document.querySelectorAll("#" + c.strip + " > .stab")) if (b !== r.btn) b.classList.remove("active");
        for (const s of sides) if (s.side === r.side && s !== r) s.pane.hidden = true;
        r.pane.hidden = false; r.btn.classList.add("active");
        safe("view onResize " + r.type, () => r.view.onResize());
        if (typeof updateTitle === "function") try { updateTitle(); } catch (_) {}
        d.touch();
      }
      function sideMount(leaf, type, state, inst) {
        const ty = types.get(type);
        const c = SIDE[leaf._side], strip = byId(c.strip), host = byId(c.host);
        if (!strip || !host) return null;
        let view = inst || null;
        if (!view) {
          if (!ty) return null;
          try { view = ty.factory(leaf); } catch (e) { console.error("[plugin] view factory " + type + ":", e); return null; }
          if (!view) return null;
        }
        const btn = mk("button", { cls: "stab os-pv-tab" });
        btn.dataset.type = type;
        const pane = mk("div", { cls: "spane os-pv-pane" });
        pane.dataset.type = type; pane.hidden = true;
        view.containerEl.classList.add("os-pv");
        view.containerEl.dataset.type = type;
        pane.appendChild(view.containerEl);
        const before = c.before ? byId(c.before) : null;
        if (before && before.parentNode === strip) strip.insertBefore(btn, before); else strip.appendChild(btn);
        const vs = byId("vbar") || byId("vswitch");   // settings-gear: the footer is #vbar (switcher + gear); panes go above it
        if (leaf._side === "left" && vs && vs.parentNode === host) host.insertBefore(pane, vs); else host.appendChild(pane);
        const r = { leaf, view, type, pid: ty ? ty.pid : null, side: leaf._side, opened: null, btn, pane, icon: null };
        sides.push(r);
        btn.onclick = () => sideShow(r);
        safe("view load " + type, () => view.load());
        r.opened = (async () => {
          try { await view.onOpen(); } catch (e) { console.error("[plugin] view onOpen " + type + ":", e); }
          if (state && Object.keys(state).length) try { await view.setState(state, {}); } catch (e) { console.error("[plugin] view setState " + type + ":", e); }
          sideSync(r); d.touch();
        })();
        sideSync(r);
        return r;
      }
      function sideClose(r) {
        const i = sides.indexOf(r); if (i < 0) return;
        sides.splice(i, 1);
        const wasShown = !r.pane.hidden;
        r.btn.remove(); r.pane.remove();
        safe("view onClose " + r.type, () => r.view.onClose());
        safe("view unload " + r.type, () => r.view.unload());
        if (wasShown) {                 // the strip falls back to the built-in tab the app had
          if (r.side === "right" && typeof setRTab === "function") safe("rtab", () => setRTab(typeof rTab === "string" ? rTab : "bl", false));
          else if (r.side === "left" && typeof setPane === "function") safe("ltab", () => setPane(typeof sidePane === "string" ? sidePane : "files"));
        }
        if (typeof updateTitle === "function") try { updateTitle(); } catch (_) {}
        d.touch();
      }
      async function sideOpenView(leaf, type, state, active, inst) {
        let r = sideRec(leaf);
        if (r && r.type === type && !inst) { await r.opened; try { await r.view.setState(state, {}); } catch (e) { console.error(e); } sideSync(r); }
        else { if (r) sideClose(r); r = sideMount(leaf, type, state, inst); if (!r) return; await r.opened; }
        if (active) await sideReveal(leaf);   // a background open stays a strip button until revealed
      }
      async function sideReveal(leaf) {
        const r = sideRec(leaf); if (!r) return;
        if (r.side === "right" && !sideIsOpen("right") && typeof cmdToggleRight === "function") await cmdToggleRight();
        if (r.side === "left" && !sideIsOpen("left") && typeof cmdToggleSide === "function") await cmdToggleSide();
        sideShow(r);
      }
      async function sideDetach(leaf) { const r = sideRec(leaf); if (r) sideClose(r); }
      function getSideLeaf(side) { const l = new WorkspaceLeaf(d.app()); l._side = side; return l; }

      /* ------------------------------------------- HoverPopover (api-workspace)
         A popover owned by a HoverParent (parent.hoverPopover). Born Showing and
         detached; after waitTime it is Shown: appended to <body>, placed under
         its target (or at staticPos), and it becomes parent.hoverPopover. The
         pointer leaving target and popover starts Hiding; HIDE_MS later it is
         Hidden, detached, unowned and unloaded (re-entering cancels). hide()
         ends it at once. hoverEl / state are accessors over a private record,
         so a subclass may still assign them. */
      const PopoverState = { Showing: 0, Shown: 1, Hiding: 2, Hidden: 3 };
      for (const k of Object.keys(PopoverState)) PopoverState[PopoverState[k]] = k;
      const pops = new WeakMap(), HIDE_MS = 300;
      class HoverPopover extends Component {
        constructor(parent, targetEl, waitTime, staticPos) {
          super(); installDom();
          const el = mk("div", { cls: "popover hover-popover" });
          const p = { el, state: PopoverState.Showing, parent: parent && typeof parent === "object" ? parent : null,
                      target: targetEl instanceof Element ? targetEl : null, pos: staticPos && typeof staticPos === "object" ? staticPos : null,
                      showT: null, hideT: null, off: [] };
          pops.set(this, p);
          const on = (n, ev, fn) => { n.addEventListener(ev, fn); p.off.push(() => n.removeEventListener(ev, fn)); };
          const leave = e => { const x = e && e.relatedTarget; if (x instanceof Node && (p.el.contains(x) || (p.target && p.target.contains(x)))) return; popLeave(this); };
          const enter = () => popEnter(this);
          if (p.target) { on(p.target, "mouseover", enter); on(p.target, "mouseout", leave); on(p.target, "mouseleave", leave); }
          on(el, "mouseenter", enter); on(el, "mouseleave", leave);
          const ms = Number(waitTime) > 0 ? Number(waitTime) : 0;
          p.showT = setTimeout(() => popShow(this), ms);
        }
        get hoverEl() { const p = pops.get(this); return p ? p.el : undefined; }
        set hoverEl(el) { const p = pops.get(this); if (p && el instanceof HTMLElement) p.el = el; }
        get state() { const p = pops.get(this); return p ? p.state : undefined; }
        set state(s) { const p = pops.get(this); if (p) p.state = s; }
        onShow() {}
        onHide() {}
        hide() { popHide(this); }
      }
      function popShow(hp) {
        const p = pops.get(hp); if (!p) return;
        p.showT = null;
        if (p.state !== PopoverState.Showing) return;
        const prev = p.parent && p.parent.hoverPopover;
        if (prev && prev !== hp && typeof prev.hide === "function") safe("hover popover", () => prev.hide());
        p.state = PopoverState.Shown;
        const el = p.el, s = el.style;
        s.position = "fixed"; s.zIndex = "1000"; s.maxWidth = "min(640px, 90vw)"; s.maxHeight = "min(480px, 80vh)"; s.overflow = "auto";
        if (!s.background) s.background = "var(--background-primary, #fff)";
        if (!s.border) s.border = "1px solid var(--background-modifier-border, #ccc)";
        if (!s.borderRadius) s.borderRadius = "8px";
        document.body.appendChild(el);
        popPlace(p);
        const down = e => { if (!(e.target instanceof Node) || !(el.contains(e.target) || (p.target && p.target.contains(e.target)))) popHide(hp); };
        document.addEventListener("mousedown", down, true);
        p.off.push(() => document.removeEventListener("mousedown", down, true));
        if (p.parent) p.parent.hoverPopover = hp;
        safe("hover popover load", () => hp.load());
        safe("hover popover onShow", () => hp.onShow());
      }
      function popPlace(p) {
        const el = p.el, W = window.innerWidth, H = window.innerHeight;
        let x = 0, y = 0, above = 0;
        if (p.pos) { x = Number(p.pos.x) || 0; y = Number(p.pos.y) || 0; above = y; }
        else if (p.target) { const r = p.target.getBoundingClientRect(); x = r.left; y = r.bottom + 6; above = r.top - 6; }
        const b = el.getBoundingClientRect();
        if (x + b.width > W - 8) x = Math.max(8, W - b.width - 8);
        if (y + b.height > H - 8 && above - b.height >= 8) y = above - b.height;
        el.style.left = Math.round(Math.max(0, x)) + "px"; el.style.top = Math.round(Math.max(0, y)) + "px";
      }
      function popLeave(hp) {
        const p = pops.get(hp); if (!p) return;
        if (p.state === PopoverState.Showing) return popHide(hp);   // left before it showed: it never does
        if (p.state !== PopoverState.Shown) return;
        p.state = PopoverState.Hiding;
        p.hideT = setTimeout(() => { p.hideT = null; if (p.state === PopoverState.Hiding) popHide(hp); }, HIDE_MS);
      }
      function popEnter(hp) {
        const p = pops.get(hp); if (!p || p.state !== PopoverState.Hiding) return;
        clearTimeout(p.hideT); p.hideT = null; p.state = PopoverState.Shown;
      }
      function popHide(hp) {
        const p = pops.get(hp); if (!p) return;
        clearTimeout(p.showT); clearTimeout(p.hideT); p.showT = p.hideT = null;
        if (p.state === PopoverState.Hidden) return;
        const shown = p.state !== PopoverState.Showing;
        p.state = PopoverState.Hidden;
        for (const f of p.off.splice(0)) try { f(); } catch (_) {}
        p.el.remove();
        if (p.parent && p.parent.hoverPopover === hp) p.parent.hoverPopover = null;
        if (shown) safe("hover popover onHide", () => hp.onHide());
        safe("hover popover unload", () => hp.unload());
      }

      // [pview:<n types>|<type>@<where><+ shown|- hidden>,...] — where: t<pane index> main-area tab, r / l sidebar
      // <n types> counts plugin view types: a core plugin's own (factory tagged opensidian.coreView) is not one
      const CORE_VIEW = Symbol.for("opensidian.coreView");
      function censusTok() {
        const nt = [...types.values()].filter(t => !(t.factory && t.factory[CORE_VIEW])).length;
        if (!nt && !recs.size && !sides.length) return "";
        const gs = M().groups(), cs = s => String(s).replace(/[[\]|,@+\s-]+/g, "_").slice(0, 40);
        const one = r => cs(r.type) + "@" + (r.side ? r.side[0] : "t" + gs.indexOf(groupOfTab(r.t))) + (r.view.containerEl.isConnected && !r.view.containerEl.hidden && r.view.containerEl.getClientRects().length ? "+" : "-");
        return " [pview:" + nt + "|" + [...recs.values(), ...sides].map(one).join(",") + "]";
      }
      const host = { PV, sweep, censusTok, registerView, unregisterView, viewRegistry, leavesOfType, iterateLeaves, getLeaf, getSideLeaf,
                     reveal, leafOfGroup, leafOfTab, bindLeaf, activeTab, groupOfTab, typeOfTab, isPvTab, recs, types, sideHide, sideShowing,
                     mainRoot, newRoot() { rootGen++; }, dock, ribbon, floating };
      const layout = { WorkspaceItem, WorkspaceParent, WorkspaceSplit, WorkspaceContainer, WorkspaceRoot, WorkspaceWindow,
                       WorkspaceFloating, WorkspaceTabs, WorkspaceSidedock, WorkspaceRibbon, HoverPopover };
      return { View, ItemView, FileView, EditableFileView, TextFileView, WorkspaceLeaf, layout, PopoverState, host };
    },
  });
})();
/* the "obsidian" module, part WORKSPACE: the views factory is taken ONCE and
   removed from window, then app.workspace (a Workspace) over the app's tab
   groups. A separate closure: it sees only what api.js hands it in K. Segment
   contract: see api-core.js. */
(function () {
  "use strict";
  const parts = window.__osApiParts || (window.__osApiParts = Object.create(null));
  const reg = (id, fn) => { if (!(id in parts)) parts[id] = fn; };
  reg("workspace:views", function (K) {
    const { Component, Events, installDom, mk, setIcon, touch } = K;
    /* --------------------------------------------- views (ui/plugins/views.js)
       View / ItemView / FileView / WorkspaceLeaf and the layout classes live
       in views.js. Its factory is taken ONCE here and removed from window, so
       plugin code cannot reach it; its deps are read at call time. */
    const viewsFactory = window.__osPluginViews;
    try { delete window.__osPluginViews; } catch (_) {}
    if (typeof viewsFactory !== "function") { console.error("[plugin] views.js did not load"); return false; }
    const { View, ItemView, FileView, EditableFileView, TextFileView, WorkspaceLeaf, layout, PopoverState, host: views } = viewsFactory({
      Component, Events, installDom, mk, setIcon, app: () => K.app, touch: () => touch(),
      vault: () => K.vault, TFile: () => K.TFile, debounce: (cb, ms, reset) => K.debounce(cb, ms, reset),
      leafView: l => K.leafView(l), groupFile: g => K.groupFile(g), openFileIn: (l, f, o) => K.openFileIn(l, f, o),
      groupLeaf: g => K.groupLeaf(g), leafState: l => K.leafState(l), activeLeaf: () => (K.workspace ? K.workspace.activeLeaf : null),
    });
    Object.assign(K, { EditableFileView, FileView, ItemView, TextFileView, View, WorkspaceLeaf, views, layout });
    /* app.embedRegistry: file extension -> embed creator(ctx, file, subpath), the component an
       internal embed ![[f.ext]] of that extension renders through (an Events that fires nothing).
       Lookups are case-sensitive; the duplicate check and isExtensionRegistered use own keys. The app's
       own embeds of images / pdf / audio are rendered natively by the editor, so the built-in map is
       {md}: a markdown embed that renders the note (or its #heading / #^block section) with
       MarkdownRenderer. */
    const ownKey = (o, k) => k != null && Object.prototype.hasOwnProperty.call(o, k);
    /* the markdown embed. ctx.displayMode true is the reading embed: the whole note (the subpath is
       not applied), no link row, and it keeps its rendering past unload. Otherwise (the default) it is
       the note embed: data = the file, text = the #heading / #^block section (the whole file without a
       subpath; "##### Unable to find “x” in <basename>" with subpathNotFound when it does not resolve),
       rendered with the frontmatter left out, a .markdown-embed-link row after it; unload empties it. */
    class MarkdownEmbed extends Component {
      constructor(ctx, file, subpath) {
        super();
        ctx = ctx || {};
        this.app = ctx.app || K.app; this.containerEl = ctx.containerEl; this.file = file;
        this.subpath = String(subpath == null ? "" : subpath);
        this.linktext = ctx.linktext; this.sourcePath = ctx.sourcePath; this.depth = ctx.depth || 0;
        Object.defineProperty(this, "_reading", { value: !!ctx.displayMode });
        if (!this._reading) this.subpathNotFound = false;
        this.containerEl.classList.add("markdown-embed");
      }
      getFile() { return this.file; }
      async loadFile() {
        const f = this.file, el = this.containerEl;
        if (!f) throw new TypeError("MarkdownEmbed.loadFile: no file");
        const data = await this.app.vault.cachedRead(f);
        let text = data;
        if (!this._reading) {
          this.data = data;
          if (this.subpath) {
            const r = K.resolveSubpath(this.app.metadataCache.getFileCache(f), this.subpath);
            this.subpathNotFound = !r;
            text = r ? data.slice(r.start.offset, r.end ? r.end.offset : data.length)
                     : "##### Unable to find “" + this.subpath.replace(/^#/, "") + "” in " + f.basename;
          } else this.subpathNotFound = false;
          this.text = text;
        }
        const md = text === data ? data.slice(K.getFrontMatterInfo(data).contentStart) : text;
        el.textContent = "";
        const body = document.createElement("div"); body.className = "markdown-embed-content";
        el.appendChild(body);
        if (!this._reading) {
          const link = document.createElement("div"); link.className = "markdown-embed-link";
          link.addEventListener("click", () => this.app.workspace.openLinkText(this.linktext || f.path, this.sourcePath || "", false));
          el.appendChild(link);
        }
        if (md) await K.MarkdownRenderer.render(this.app, md, body, f.path, this);
      }
      /* editable embed: showEditor() swaps the rendering for an in-place editor (this.editMode), an
         EmbedEditor whose base class is the internal MarkdownEditor; plugins (obsidian-kanban) read that
         base off the prototype chain and subclass it for their own inline editors. */
      showEditor() {
        if (this.editMode) return this.editMode;
        const ed = new EmbedEditor(this.app, this.containerEl, this);
        this.editMode = ed; this.addChild(ed);
        ed.set(this.text != null ? this.text : (this.data || ""), true);
        return ed;
      }
      onunload() {
        if (this.editMode) { this.removeChild(this.editMode); this.editMode = null; }
        if (!this._reading) this.containerEl.textContent = "";
      }
    }
    /* MarkdownEditor: a CodeMirror 6 editor over the editor's own shared modules, mounted in a div
       under containerEl. Subclasses extend buildLocalExtensions() (called once from the ctor) and
       onUpdate(update, docChanged); owner is the object that hosts it (an embed, a plugin's view).
       get/set read and replace the text, editor is the obsidian Editor over this.cm. */
    class MarkdownEditor extends Component {
      constructor(app, containerEl, owner) {
        super();
        this.app = app || K.app; this.containerEl = containerEl; this.owner = owner || null;
        const M = window.__osModules || {}, Vw = M["@codemirror/view"], St = M["@codemirror/state"];
        if (!Vw || !St) throw new Error("unsupported in opensidian: the editor modules are not loaded");
        const el = document.createElement("div"); el.className = "markdown-source-view cm-s-obsidian mod-cm6";
        if (containerEl) containerEl.appendChild(el);
        this.editorEl = el;
        const exts = [].concat(this.buildLocalExtensions() || [],
          Vw.EditorView.updateListener.of(u => { if (u.docChanged || u.selectionSet || u.focusChanged) this.onUpdate(u, u.docChanged); }));
        this.cm = new Vw.EditorView({ state: St.EditorState.create({ doc: "", extensions: exts }), parent: el });
      }
      buildLocalExtensions() {
        const Vw = (window.__osModules || {})["@codemirror/view"];
        return Vw ? [Vw.EditorView.lineWrapping] : [];
      }
      onUpdate(update, changed) {}
      get editor() {
        if (!this._editor) { const EA = K.EA; if (!EA || !EA.Editor) return null; Object.defineProperty(this, "_editor", { value: new EA.Editor(this) }); }
        return this._editor;
      }
      get() { return this.cm.state.doc.toString(); }
      set(text, clear) {
        const d = this.cm.state.doc;
        this.cm.dispatch({ changes: { from: 0, to: d.length, insert: String(text == null ? "" : text) }, selection: clear ? { anchor: 0 } : undefined });
      }
      clear() { this.set("", true); }
      focus() { this.cm.focus(); }
      hasFocus() { return this.cm.hasFocus; }
      destroy() { this.unload(); }
      onunload() { try { this.cm.destroy(); } catch (_) {} if (this.editorEl) this.editorEl.remove(); }
    }
    class EmbedEditor extends MarkdownEditor {}

    // SEAM S3: the renderers (editor-api embedPass) read this map; a change re-arms the reading view
    // hook (PPOST) and bumps its generation so a retained reading DOM is rendered again
    const embChanged = () => { try { if (K.mdpChanged) K.mdpChanged(); } catch (e) { console.error(e); } };
    class EmbedRegistry extends Events {
      constructor() { super(); this.embedByExtension = { md: (ctx, file, subpath) => new MarkdownEmbed(ctx, file, subpath) }; }
      registerExtension(ext, creator) {
        if (ownKey(this.embedByExtension, ext)) throw new Error("Attempting to register an embed for an already registered extension \"" + ext + "\"");
        this.embedByExtension[ext] = creator; embChanged();
      }
      registerExtensions(exts, creator) { for (const e of exts) this.registerExtension(e, creator); }
      unregisterExtension(ext) { if (ownKey(this.embedByExtension, ext)) { delete this.embedByExtension[ext]; embChanged(); } }
      unregisterExtensions(exts) { for (const e of exts) this.unregisterExtension(e); }
      isExtensionRegistered(ext) { return ownKey(this.embedByExtension, ext); }
      getEmbedCreator(file) { const e = file.extension; return ownKey(this.embedByExtension, e) ? this.embedByExtension[e] : null; }
    }
    const embedRegistry = new EmbedRegistry();
    /* app.dragManager: the drag payloads plugins hand around (a plain object). dragFile / dragFiles /
       dragFolder / dragLink build the payload {type, source, title, icon, file(s)|linktext} and write the
       event's dataTransfer: a file (or several) as its obsidian://open URL in text/plain + text/uri-list,
       a folder as text/plain only. onDragStart(e, data) makes data the current draggable until the drag
       that started it ends (dragend on that event's target); setAction(text) labels the drag. */
    const obsUrl = f => "obsidian://open?vault=" + encodeURIComponent(K.vault ? K.vault.getName() : "") +
      "&file=" + encodeURIComponent(f.extension === "md" ? f.path.slice(0, -3) : f.path);
    class DragManager {
      constructor() {
        this.draggable = null; this.actionEl = null; this.dragStart = null; this.ghostEl = null; this.hoverEl = null;
        this.sourceEls = null; this.hoverClass = "is-being-dragged-over"; this.sourceClass = "is-being-dragged";
        this.isDragOverHandled = false; this.shouldHideOverlay = false;
      }
      dragFile(e, file, source) {
        const p = file.path, dt = e.dataTransfer;
        const u = obsUrl(file); dt.setData("text/plain", u); dt.setData("text/uri-list", u);
        return { type: "file", file, source, title: file.basename !== undefined ? file.basename : p, icon: "lucide-file" };
      }
      dragFiles(e, files, source) {
        const n = files.length, dt = e.dataTransfer, u = files.map(obsUrl).join("\n");
        dt.setData("text/plain", u); dt.setData("text/uri-list", u);
        return { type: "files", files, source, title: n + " files", icon: "lucide-files" };
      }
      dragFolder(e, folder, source) {
        const root = folder.isRoot ? folder.isRoot() : folder.path === "/", p = root ? "" : folder.path;
        e.dataTransfer.setData("text/plain", p);   // the vault root: ""
        return { type: "folder", file: folder, source, title: root ? "" : folder.name, icon: "lucide-folder-open" };
      }
      dragLink(e, linktext, sourcePath, title, source) {
        const dt = e.dataTransfer;
        const file = K.vault && K.app && K.app.metadataCache ? K.app.metadataCache.getFirstLinkpathDest(String(linktext).split("#")[0], sourcePath) : null;
        if (file) { const u = obsUrl(file); dt.setData("text/plain", u); dt.setData("text/uri-list", u); }
        return { type: "link", linktext, sourcePath, file, source, title, icon: "lucide-link" };
      }
      onDragStart(e, data) {
        this.draggable = data;
        const t = e && e.target;
        if (t && typeof t.addEventListener === "function") t.addEventListener("dragend", () => this.onDragEnd(), { once: true });
      }
      onDragEnd() { this.draggable = null; if (this.actionEl) this.actionEl.textContent = ""; }
      setAction(text) {
        if (!this.actionEl) { this.actionEl = document.createElement("div"); this.actionEl.className = "drag-ghost-action"; }
        if (text != null) this.actionEl.textContent = String(text);   // null / undefined keep the label, as stock
      }
    }
    // app-level members (vault:app builds app later and defines these on it)
    K.appMembers = Object.assign(K.appMembers || {}, { viewRegistry: { value: views.viewRegistry, writable: true },
                                                       embedRegistry: { value: embedRegistry, writable: true },
                                                       dragManager: { value: new DragManager(), writable: true },
                                                       // the mobile toolbar exists only on mobile: null on desktop, as stock
                                                       mobileToolbar: { value: null, writable: true, enumerable: true } });
    K.obsidianUrl = obsUrl;            // app.getObsidianUrl (api-vault.js vault:app) builds the same URL
    Object.assign(K.exportClasses, layout);
    K.exportValues.PopoverState = PopoverState;
  });
  reg("workspace:workspace", function (K) {
    const { EA, Events, FileView, MarkdownView, Menu, TFile, View, WorkspaceLeaf, bridge, debounce, files, normalizePath, noteOf, parseLinktext, resolveLink, touch, vault, views } = K;
    /* ------------------------------------------- workspace
       Leaves are the app's tabs (one per tab; an empty group has one), the
       layout tree is main.js's split tree (views.js), all read at call time.
       file-open / active-leaf-change are detected by focusTick(), which the
       [plg:] census runs on every updateTitle — the one place main.js already
       reports "something on screen changed" — so this needs no new hook. */
    const leafOf = new WeakMap();       // group -> its EMPTY-group leaf (a tab, once the group has one, is its own leaf)
    const groupLeaf = g => {
      if (!g) return null;
      let l = leafOf.get(g);
      if (!l || l._t || l._nt || l._split) { l = new WorkspaceLeaf(K.app); l._g = g; leafOf.set(g, l); }
      return l;
    };
    const leafFor = g => (g ? views.leafOfGroup(g) : null);
    const allGroups = () => { try { return typeof groups === "function" ? groups() : []; } catch (_) { return []; } };
    const focusedGroup = () => { try { return typeof state !== "undefined" && state ? state.focused : null; } catch (_) { return null; } };
    const noteFile = t => { if (!t || t.kind || !K.st.modelOn) return null; const f = files.get(t.name + ".md"); return f instanceof TFile ? f : null; };
    function groupFile(g) { return noteFile(views.activeTab(g)); }
    function leafView(l) {              // a tab leaf that is not a plugin view: its note / graph / nothing
      const t = l._t, g = l._grp;
      if (!l._v) {
        const mv = new MarkdownView(l);   // no editor-api / a background tab: a file-only stand-in
        Object.defineProperty(mv, "file", { configurable: true, get: () => noteFile(l._t || views.activeTab(l._grp)) });
        const gv = new View(l); gv.getViewType = () => views.typeOfTab(l._t);
        gv.getDisplayText = () => (l._t && l._t.kind === "lg" ? "Graph of " + (l._t.center || "") : "Graph view");
        const ev = new View(l); ev.getViewType = () => "empty"; ev.navigation = true;   // a main-area view, as stock's empty view
        Object.defineProperty(l, "_v", { value: { mv, gv, ev }, writable: true, configurable: true });
      }
      const tt = t || views.activeTab(g);
      if (tt && !tt.kind) {
        if (EA && K.st.modelOn) {
          let tv = null;
          if (g && g.view && g.view.tab === tt) tv = g.view; else if (tt.view) tv = tt.view;
          const mv = tv && EA.viewFor(tv, l); if (mv) return mv;
        }
        return l._v.mv;
      }
      if (tt && (tt.kind === "gg" || tt.kind === "lg")) return l._v.gv;
      return l._v.ev;
    }
    function leafState(l) {             // {type, state} of a tab leaf, in workspace.json's own words
      const t = l._t || views.activeTab(l._grp);
      if (!t) return { type: "empty", state: {} };
      if (typeof wsLeaf === "function") try { const w = wsLeaf(t); if (w && w.state) return { type: w.state.type, state: JSON.parse(JSON.stringify(w.state.state || {})) }; } catch (_) {}
      const f = noteFile(t); return f ? { type: "markdown", state: { file: f.path } } : { type: "empty", state: {} };
    }
    async function openFileIn(leaf, file, openState) {
      if (!(file instanceof TFile) || file.extension !== "md") return;
      const name = noteOf(file.path), g = leaf && leaf._grp;
      if (g && g !== focusedGroup() && typeof focusGroup === "function") try { focusGroup(g); } catch (_) {}
      if (leaf && leaf._t && g) {       // a tab leaf: that tab shows the note
        const t = leaf._t;
        const i = g.tabs.indexOf(t);
        if (i >= 0 && i !== g.active && typeof switchTab === "function") await switchTab(g, i);
        if (!t.kind) { if (t.name !== name && typeof navigate === "function") await navigate(g, name); }
        else if (typeof openNewTab === "function") { await openNewTab(name, g); const n = views.activeTab(g); if (n) views.bindLeaf(leaf, n); }
      } else if (leaf && leaf._split && g && typeof splitWith === "function" && typeof mkTab === "function") {
        const d0 = leaf._split, t = mkTab(name); views.bindLeaf(leaf, t); await splitWith(g, d0, t);
      } else if (leaf && leaf._nt && typeof openNewTab === "function") {
        await openNewTab(name, g || undefined);
        const n = views.activeTab(g || focusedGroup()); if (n) views.bindLeaf(leaf, n);
      } else if (typeof openInTab === "function") {
        const empty = g && !g.tabs.length;
        await openInTab(name);
        const n = views.activeTab(g || focusedGroup()); if (empty && n && leaf) views.bindLeaf(leaf, n);
      }
      const sub = openState && openState.eState && openState.eState.subpath;
      if (sub && typeof navAnchor === "function") try { await navAnchor(focusedGroup(), sub.replace(/^#/, "")); } catch (_) {}
      touch();
    }
    let lastLeaf = null, lastFile;
    /* leaf recency: each leaf that becomes active gets the next number (focusTick, and the active leaf
       again whenever getActiveFileView asks), so "the most recently active leaf" is a max over them. */
    const leafSeq = new WeakMap();
    let seqN = 0;
    const stampLeaf = l => { if (l && leafSeq.get(l) !== seqN) leafSeq.set(l, ++seqN); };
    function focusTick() {
      views.sweep();                       // a plugin-view tab the app closed -> its view closes
      if (!K.st.modelOn || !K.st.layoutReady) return;
      const l = leafFor(focusedGroup()), f = l ? groupFile(l._grp) : null;
      if (l !== lastLeaf) { lastLeaf = l; stampLeaf(l); queueMicrotask(() => workspace.trigger("active-leaf-change", l)); }
      if (f !== lastFile) { lastFile = f; queueMicrotask(() => { workspace.trigger("file-open", f); if (l) workspace.trigger("layout-change"); }); }
    }
    const unsupported = what => { throw new Error("unsupported in opensidian: " + what); };
    /* workspace.editorSuggest: a plain object (not Events) owning {currentSuggest, suggests}. Built over
       editor-api's one live list (SEAM S1): suggests IS that array, so plugin.registerEditorSuggest and
       addSuggest land in the same place and the editor consults it in order; currentSuggest is the suggest
       the editor has open (or set here). addSuggest appends (twice = twice), removeSuggest drops every
       occurrence (closing it if current). The app's own [[ / # completion is not an EditorSuggest, so
       suggests starts empty. Without editor-api the same object keeps its own list. */
    class EditorSuggests {
      constructor() {
        let own = [], cur = null;
        Object.defineProperty(this, "currentSuggest", { enumerable: true, configurable: true,
          get: () => EA ? EA.currentSuggest() : cur,
          set: v => { if (EA) EA.setCurrentSuggest(v); else cur = v || null; } });
        Object.defineProperty(this, "suggests", { enumerable: true, configurable: true, get: () => EA ? EA.suggests : own });
      }
      addSuggest(s) { if (EA) EA.addSuggest(s); else this.suggests.push(s); }
      removeSuggest(s) {
        if (EA) { EA.removeSuggest(s); return; }
        for (let i; (i = this.suggests.indexOf(s)) >= 0;) this.suggests.splice(i, 1);
        if (s && this.currentSuggest === s) this.close();
      }
      setCurrentSuggest(s) {
        const c = this.currentSuggest;
        if (EA) { EA.setCurrentSuggest(s); return; }
        if (c && c !== s && typeof c.close === "function") c.close();
        this.currentSuggest = s || null;
      }
      close() { const c = this.currentSuggest; this.currentSuggest = null; if (!EA && c && typeof c.close === "function") c.close(); }   // with editor-api, setting null closes it
      isShowingSuggestion() { const c = this.currentSuggest; return c && c.isOpen; }   // null when none, as stock
      reposition() { const c = this.currentSuggest; if (c && c.isOpen && typeof c.open === "function") c.open(); }
      /* ask each suggest in order (onTrigger(cursor, editor, file)); the first that answers a trigger
         info becomes current with context {...info, editor, file} and gets its suggestions; none: close.
         openIfClosed false keeps a closed popup closed. */
      trigger(editor, file, openIfClosed) {
        if (!editor || typeof editor.getCursor !== "function") { this.close(); return; }
        if (!openIfClosed && !this.isShowingSuggestion()) return;
        const cur = editor.getCursor();
        for (const s of this.suggests) {
          let info = null;
          try { info = s && typeof s.onTrigger === "function" ? s.onTrigger(cur, editor, file || null) : null; } catch (e) { console.error("[plugin] onTrigger:", e); }
          if (!info) continue;
          const ctx = Object.assign({}, info, { editor, file: file || null });
          s.context = ctx;
          this.setCurrentSuggest(s);
          Promise.resolve().then(() => s.getSuggestions(ctx)).then(items => {
            if (this.currentSuggest !== s || s.context !== ctx) return;
            if (typeof s.showSuggestions === "function") s.showSuggestions(items || []);
            else if (typeof s.open === "function") s.open();
          }).catch(e => console.error("[plugin] getSuggestions:", e));
          return;
        }
        this.close();
      }
    }
    class Workspace extends Events {
      constructor() {
        super();
        this.layoutReady = false;
        this.containerEl = document.body;
        this.leftSplit = views.dock("left");
        this.rightSplit = views.dock("right");
        this.leftRibbon = views.ribbon("left");
        this.rightRibbon = views.ribbon("right");
        this.floatingSplit = views.floating();
        this.editorSuggest = new EditorSuggests();
        this.requestSaveLayout = debounce(() => { if (typeof wsTouch === "function") try { wsTouch(); } catch (_) {} }, 0);
        /* hover link sources: id -> the info object a plugin registered ({display, defaultMod}), stored
           as given (the same object; re-registering an id replaces it, unregistering deletes it). The
           app's own panes have no hover-preview setting, so it starts empty. */
        this.hoverLinkSources = {};
      }
      registerHoverLinkSource(id, info) { this.hoverLinkSources[id] = info; }
      unregisterHoverLinkSource(id) { delete this.hoverLinkSources[id]; }
      /* the view of the most recently active NAVIGATION leaf (view.navigation truthy, read now: a note,
         an empty tab, a plugin view that set it) when that view is a FileView, else null. A non-navigation
         view (a plugin ItemView by default) being active does not count: the last navigation leaf
         answers. Arguments ignored. */
      getActiveFileView() {
        stampLeaf(this.activeLeaf);
        let best = null, bs = -1;
        views.iterateLeaves(l => { const v = l.view, s = leafSeq.get(l) || 0; if (v && v.navigation && s > bs) { best = v; bs = s; } }, true);
        return best instanceof FileView ? best : null;
      }
      /* every leaf under a layout item (a leaf is its own only leaf; a parent: its children, in order),
         either argument order: (root, cb) or (cb, root). cb returning a truthy value stops the walk, which then
         answers true; otherwise false. No root walks nothing (false); a leaf reached without a cb throws
         TypeError (calling it). */
      iterateLeaves(a, b) {
        const [root, cb] = typeof a === "function" ? [b, a] : [a, b];
        const walk = it => {
          if (!it || typeof it !== "object") return false;
          if (it instanceof WorkspaceLeaf) return !!cb(it);
          const ch = it.children;
          if (Array.isArray(ch)) for (const c of ch) if (walk(c)) return true;
          return false;
        };
        return walk(root);
      }
      get rootSplit() { return views.mainRoot(); }
      get activeLeaf() { const g = focusedGroup() || allGroups()[0]; return g ? leafFor(g) : null; }
      set activeLeaf(l) { this.setActiveLeaf(l); }
      // editor-api: the focused note's MarkdownView (a MarkdownFileInfo: .editor, .file), or null
      get activeEditor() { return EA && K.st.modelOn ? EA.activeEditor() : null; }
      on(name, cb, ctx) {               // main.js's PMENU is set lazily: api.js loads before main.js declares it
        if (name === "file-menu" || name === "editor-menu") try { PMENU = PM; } catch (_) {}
        return super.on(name, cb, ctx);
      }
      onLayoutReady(cb) { if (K.st.layoutReady) queueMicrotask(() => { try { cb(); } catch (e) { console.error(e); } }); else this.on("layout-ready", cb); }
      iterateAllLeaves(cb) { views.iterateLeaves(cb, true); }
      iterateRootLeaves(cb) { views.iterateLeaves(cb, false); }
      getLeavesOfType(t) { return views.leavesOfType(String(t)); }
      detachLeavesOfType(t) { for (const l of views.leavesOfType(String(t))) l.detach(); }
      getRightLeaf() { return views.getSideLeaf("right"); }
      getLeftLeaf() { return views.getSideLeaf("left"); }
      async ensureSideLeaf(type, side, o) {
        o = o || {}; let l = views.leavesOfType(type)[0];
        if (!l) { l = views.getSideLeaf(side === "left" ? "left" : "right"); await l.setViewState({ type, state: o.state || {}, active: !!o.active }); }
        if (o.reveal !== false) await views.reveal(l);
        return l;
      }
      getActiveViewOfType(C) { const l = this.activeLeaf, v = l && l.view; return v && C && v instanceof C ? v : null; }
      /* the focused note, else the file of the focused plugin FileView (a Kanban board is a .md the app shows
         in a plugin view: its commands check getActiveFile's frontmatter, so without this they never ran) */
      getActiveFile() {
        const g = focusedGroup(), f = groupFile(g); if (f) return f;
        const l = leafFor(g), v = l && l.view;
        return v instanceof FileView && v.file instanceof TFile ? v.file : null;
      }
      getMostRecentLeaf() { return this.activeLeaf; }
      getUnpinnedLeaf() { const l = this.activeLeaf; return l && l.pinned ? this.getLeaf("tab") : l; }
      getLeaf(newLeaf, dir) {
        if (!newLeaf) { const l = this.activeLeaf; if (!l || !l.pinned) return l; newLeaf = "tab"; }   // a pinned tab is not replaced
        return views.getLeaf(newLeaf, dir) || this.activeLeaf;
      }
      splitActiveLeaf(dir) { return this.getLeaf("split", dir); }
      createLeafBySplit(leaf, dir) { if (leaf && leaf._grp && typeof focusGroup === "function") try { focusGroup(leaf._grp); } catch (_) {} return this.getLeaf("split", dir); }
      createLeafInParent(parent, index) {
        if (parent instanceof K.layout.WorkspaceSplit && parent._k === "free") { const l = new WorkspaceLeaf(K.app); parent.insertChild(index, l); return l; }
        return this.getLeaf("tab");
      }
      setActiveLeaf(l) { if (l && (l._t || l._side)) { views.reveal(l); return; } if (l && l._grp && typeof focusGroup === "function") try { focusGroup(l._grp); } catch (_) {} }
      revealLeaf(l) { if (l && (l._t || l._side)) return views.reveal(l); this.setActiveLeaf(l); return Promise.resolve(); }
      async openLinkText(lt, src, newLeaf, openState) {
        const { path, subpath } = parseLinktext(lt);
        let f = path ? resolveLink(path, src) : vault.getFileByPath(src || "");
        if (!f) f = await vault.create(normalizePath(path.endsWith(".md") ? path : path + ".md"), "");
        const os = Object.assign({}, openState || {}); if (subpath) os.eState = Object.assign({}, os.eState, { subpath });
        return this.getLeaf(newLeaf).openFile(f, os);
      }
      getLastOpenFiles() { try { return typeof mruList !== "undefined" && Array.isArray(mruList) ? mruList.slice(0, 10).map(n => n + ".md") : []; } catch (_) { return []; } }
      updateOptions() { if (EA) EA.reconfigureAll(); }   // editor-api: a plugin mutated its registered extension array
      getLeafById(id) { let hit = null; views.iterateLeaves(l => { if (l.id === id) { hit = l; return true; } }, true); return hit; }
      getGroupLeaves(group) { const out = []; if (group == null) return out; views.iterateLeaves(l => { if (l.group === group) out.push(l); }, true); return out; }
      getLayout() {                     // the layout as workspace.json carries it (lastOpenFiles is the file's, not the layout's)
        if (typeof wsDoc !== "function") return {};
        const d = wsDoc(); delete d.lastOpenFiles;
        if (!d["left-ribbon"]) d["left-ribbon"] = { hiddenItems: {} };
        return JSON.parse(JSON.stringify(d));
      }
      async changeLayout(lay) {         // replace the whole layout: a new root, new groups, new leaves
        if (!lay || typeof lay !== "object" || typeof wsApply !== "function") return;
        const old = allGroups();
        if (typeof flushSave === "function") for (const g of old) { try { await flushSave(g); } catch (_) {} }
        const names = typeof inv === "function" ? await inv("list_notes") : [];
        const ok = await wsApply(lay, names);
        if (!ok) return;
        views.newRoot();
        const live = new Set(allGroups());
        for (const g of old) if (!live.has(g)) { if (g.sim) { try { clearTimeout(g.sim); } catch (_) {} g.sim = null; } if (g.saveT) { clearTimeout(g.saveT); g.saveT = null; } }
        views.sweep();
        touch();
        this.trigger("layout-change");
      }
      async duplicateLeaf(leaf, type, dir) {
        const n = this.getLeaf(type === undefined ? false : type, dir);
        if (n && leaf && n !== leaf) await n.setViewState(leaf.getViewState(), leaf.getEphemeralState());
        return n;
      }
      handleLinkContextMenu(menu, linktext, sourcePath) {   // the link's rows in a context menu
        if (!menu || typeof menu.addItem !== "function") return false;
        const lt = String(linktext || "");
        menu.addItem(i => i.setTitle("Open in new tab").setIcon("lucide-file-plus").onClick(() => this.openLinkText(lt, sourcePath, "tab")));
        menu.addItem(i => i.setTitle("Open to the right").setIcon("lucide-separator-vertical").onClick(() => this.openLinkText(lt, sourcePath, "split")));
        const { path } = parseLinktext(lt);
        const f = path ? resolveLink(path, sourcePath) : null;
        if (f instanceof TFile) this.trigger("file-menu", menu, f, "link-context-menu", null);
        return true;
      }
      /* top5-func: one window per vault, so a popout leaf is what getLeaf("window") already is here — a new
         tab in the focused pane, a real leaf (setViewState/openFile show there, detach closes it). The size and
         position in `data` describe an OS window that does not exist and are not used. A leaf never given a
         view leaves no tab behind (it is pending, like getLeaf("tab")'s). */
      openPopoutLeaf(data) { return this.getLeaf("window"); }
      moveLeafToPopout() { unsupported("popout windows (Workspace.moveLeafToPopout): one window per vault"); }
    }
    const workspace = new Workspace();
    /* workspace "file-menu" / "editor-menu". main.js builds its own .ctxmenu
       (noteMenu: the explorer row) and asks PMENU before placing it; a plugin's
       handler gets a Menu whose rows land in THAT menu after a rule, so they
       share its styling, its close-on-mousedown and its [menu:] census. Nothing
       is added — not even the rule — when no handler adds a row. "editor-menu"
       opens a menu of plugin rows only, and only once bridge.editor yields an
       Editor. */
    /* plug-val E43: MenuItem.setSection(). Stock orders a file/folder menu by section
       (open, action-primary, action, info, view, system, "", danger; a rule between two
       sections) and a plugin row joins its section's group: excalidraw's "New drawing"
       (action-primary) sits under New note..New base, git's rows (action) after Bookmark.
       When every core row of the host menu has a known section, the menu is rebuilt in
       section order; an unknown core row keeps the old append-after-a-rule layout. */
    const SEC_RANK = ["title", "open", "action-primary", "action", "info", "view", "system", "", "danger"];
    const CORE_SEC = {
      "Open in new tab": "open", "Open to the right": "open", "Open in new window": "open",
      "New note": "action-primary", "New folder": "action-primary", "New canvas": "action-primary", "New base": "action-primary",
      "Make a copy": "action", "Move folder to...": "action", "Move file to...": "action", "Search in folder": "action",
      "Bookmark": "action", "Bookmark...": "action", "Remove bookmark": "action", "Merge entire file with...": "action",
      "Copy path": "info", "Show in system explorer": "system", "Open in default app": "system",
      "Rename...": "danger", "Delete": "danger",
    };
    function sectionLayout(m, items, isSep) {
      const core = [...m.children].filter(c => !c.classList.contains("sep"));
      const secOf = c => { const t = (c.textContent || "").trim(); return Object.prototype.hasOwnProperty.call(CORE_SEC, t) ? CORE_SEC[t] : null; };
      if (!core.length || core.some(c => secOf(c) === null)) return false;
      const rows = core.map(c => ({ el: c, sec: secOf(c) }));
      // PR #33 review: a plugin's own separator between two of its rows that land side by side in one section stays
      // a rule there (a section change draws one anyway); it is not dropped with the rest of the separator items
      let sepAfter = null, last = null;
      for (const x of items) {
        if (isSep(x)) { sepAfter = last; continue; }
        if (!x) continue;
        const r = { el: x.dom, sec: String(x.section || ""), afterSep: sepAfter };
        rows.push(r); last = r; sepAfter = null;
      }
      const extra = [];   // plugin sections stock does not name: after "", before danger, first-seen order
      for (const r of rows) if (!SEC_RANK.includes(r.sec) && !extra.includes(r.sec)) extra.push(r.sec);
      const order = [...SEC_RANK.slice(0, -1), ...extra, "danger"];
      const rank = s => order.indexOf(s);
      rows.forEach((r, i) => { r.i = i; });
      rows.sort((a, b) => rank(a.sec) - rank(b.sec) || a.i - b.i);
      m.textContent = "";
      let prev = null, prow = null;
      for (const r of rows) {
        if (prev !== null && (r.sec !== prev || (r.afterSep && r.afterSep === prow))) { const s = document.createElement("div"); s.className = "sep"; m.appendChild(s); }
        m.appendChild(r.el); prev = r.sec; prow = r;
      }
      return true;
    }
    function menuHook(name, m, args) {
      const a = workspace._ev.get(name);
      if (!a || !a.length) return 0;
      const menu = new Menu(); menu.dom = m;
      workspace.trigger(name, menu, ...args);
      // a MenuSeparator row (when that class exists) counts as a rule, like null
      const isSep = x => x === null || (typeof K.MenuSeparator === "function" && x instanceof K.MenuSeparator);
      const it = menu.items.slice();
      while (it.length && isSep(it[0])) it.shift();
      while (it.length && isSep(it[it.length - 1])) it.pop();
      const n = it.filter(x => x && !isSep(x)).length;
      if (!n) return 0;
      if (sectionLayout(m, it, isSep)) return n;
      if (m.childElementCount) { const s = document.createElement("div"); s.className = "sep"; m.appendChild(s); }
      for (const x of it) {
        if (isSep(x)) { const s = document.createElement("div"); s.className = "sep"; m.appendChild(s); }
        else m.appendChild(x.dom);
      }
      return n;
    }
    const PM = {
      fileMenu(m, name, source) {
        const f = files.get(name + ".md");
        return f instanceof TFile ? menuHook("file-menu", m, [f, source || "file-explorer-context-menu", null]) : 0;
      },
      /* an explorer FOLDER row: "file-menu" with its TFolder (templater "Create new note from template") */
      folderMenu(m, path) {
        const f = files.get(normalizePath(String(path || "")));
        return f instanceof K.TFolder ? menuHook("file-menu", m, [f, "file-explorer-context-menu", null]) : 0;
      },
      editorMenu(e) {
        const x = bridge.editor ? bridge.editor() : null;
        if (!x || !x.editor) return false;
        const m = document.createElement("div");
        m.className = "ctxmenu menu os-plugin-menu";
        if (!menuHook("editor-menu", m, [x.editor, x.view])) return false;
        if (typeof closeMenu === "function") closeMenu();
        if (typeof placeMenu === "function") placeMenu(m, e.clientX, e.clientY);
        return true;
      },
    };
    if (EA) bridge.editor = () => { const mv = K.st.modelOn ? EA.activeEditor() : null; return mv && mv.editor ? { editor: mv.editor, view: mv } : null; };
    K.exportClasses.Workspace = Workspace;
    Object.assign(K, { allGroups, focusTick, focusedGroup, groupFile, groupLeaf, leafFor, leafState, leafView, openFileIn, workspace });
  });
})();
