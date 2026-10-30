// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* the "obsidian" module, part CORE (api-parity stage 0: split out of api.js,
   behaviour-neutral). Events, Component, touch(), Platform / PlatformGuarded,
   requireApiVersion, Keymap (class; app.keymap is one), Tasks, Notice,
   Scope + the keydown scope stack, the
   plugin registry and the shared bridge, loadMoment, app.keymap /
   app.commands / app.plugins, commands -> CMDS, Plugin, and the manager
   (enable / disable / refresh / boot / censusTok).

   Every part file registers named segments on window.__osApiParts; api.js
   (the assembler) runs them ONCE, in one fixed order that is the order the
   code ran in when it was a single file, then deletes the registry. A
   segment reads what earlier segments exported from the shared context K
   (never handed to a plugin) and exports its own names into K; a name from
   a LATER segment is read as K.name at call time. Mutable state several
   parts share lives in K.st (trusted, manifests, enabledIds, layoutReady,
   vaultName, modelOn). New obsidian exports go into K.exportClasses /
   K.exportValues; api.js appends them after the fixed list. */
(function () {
  "use strict";
  const parts = window.__osApiParts || (window.__osApiParts = Object.create(null));
  const reg = (id, fn) => { if (!(id in parts)) parts[id] = fn; };
  reg("core:events", function (K) {
    const { host, be } = K;
    /* ------------------------------------------------------------ Events */
    class Events {
      constructor() { Object.defineProperty(this, "_ev", { value: new Map(), writable: true }); }
      on(name, callback, ctx) {
        const ref = { e: this, name, fn: callback, ctx };
        if (!this._ev.has(name)) this._ev.set(name, []);
        this._ev.get(name).push(ref);
        return ref;
      }
      off(name, callback) {
        const a = this._ev.get(name); if (!a) return;
        const i = a.findIndex(r => r.fn === callback); if (i >= 0) a.splice(i, 1);
      }
      offref(ref) {
        const a = ref && this._ev.get(ref.name); if (!a) return;
        const i = a.indexOf(ref); if (i >= 0) a.splice(i, 1);
      }
      trigger(name, ...data) {
        const a = this._ev.get(name); if (!a) return;
        for (const r of a.slice()) this.tryTrigger(r, data);
      }
      tryTrigger(ref, args) {
        try { ref.fn.apply(ref.ctx, args); } catch (e) { console.error("[plugin] event handler " + ref.name + ":", e); }
      }
    }

    /* --------------------------------------------------------- Component */
    class Component {
      constructor() {
        Object.defineProperty(this, "_loaded", { value: false, writable: true });
        Object.defineProperty(this, "_disp", { value: [], writable: true });      // cleanup callbacks
        Object.defineProperty(this, "_children", { value: [], writable: true });
      }
      load() {
        if (this._loaded) return;
        this._loaded = true;
        const r = this.onload();
        for (const c of this._children.slice()) c.load();
        return r;
      }
      onload() {}
      unload() {
        if (!this._loaded) return;
        this._loaded = false;
        for (const c of this._children.splice(0).reverse()) { try { c.unload(); } catch (e) { console.error(e); } }
        for (const f of this._disp.splice(0).reverse()) { try { f(); } catch (e) { console.error("[plugin] cleanup:", e); } }
        try { this.onunload(); } catch (e) { console.error("[plugin] onunload:", e); }
      }
      onunload() {}
      addChild(c) { this._children.push(c); if (this._loaded) c.load(); return c; }
      removeChild(c) { const i = this._children.indexOf(c); if (i >= 0) { this._children.splice(i, 1); c.unload(); } return c; }
      register(cb) { this._disp.push(cb); }
      registerEvent(ref) { this.register(() => ref && ref.e && ref.e.offref(ref)); }
      registerDomEvent(el, type, cb, opts) { el.addEventListener(type, cb, opts); this.register(() => el.removeEventListener(type, cb, opts)); }
      registerScopeEvent(handler) { this.register(() => handler && handler.scope && handler.scope.unregister(handler)); }
      registerInterval(id) { this.register(() => clearInterval(id)); return id; }
    }

    /* ------------------------------------------------------------- Tasks */
    /* a set of promises to wait for (cleanup work, ...). add() starts the task
       at once; promise() resolves once every task added so far has settled,
       fulfilled or rejected, so one failing task never blocks the wait.
       (Stock 1.13.7 declares Tasks in its typings but exports no runtime
       class, measured by the core oracle; this one follows the typings.) */
    const TASKS = new WeakMap();   // Tasks instance -> pending promises
    class Tasks {
      constructor() { TASKS.set(this, []); }
      add(callback) { this.addPromise(callback()); }
      addPromise(promise) {
        const st = TASKS.get(this); if (!st) return;
        const p = Promise.resolve(promise);
        const done = () => { const i = st.indexOf(p); if (i >= 0) st.splice(i, 1); };
        p.then(done, done);
        st.push(p);
      }
      isEmpty() { const st = TASKS.get(this); return !st || st.length === 0; }
      promise() { return Promise.allSettled((TASKS.get(this) || []).slice()).then(() => undefined); }   // a failed task still counts as completed
    }
    K.exportClasses.Tasks = Tasks;

    /* ------------------------------------------------------ the manager */
    let touchT = 0;
    function touch() { if (touchT) return; touchT = requestAnimationFrame(() => { touchT = 0; if (typeof updateTitle === "function") try { updateTitle(); } catch (_) {} }); }
    Object.assign(K, { Component, Events, touch });
  });
  reg("core:platform", function (K) {
    const { host, be } = K;
    const Platform = Object.freeze({
      isDesktop: true, isMobile: false, isDesktopApp: true, isMobileApp: false, isIosApp: false,
      isAndroidApp: false, isPhone: false, isTablet: false, isMacOS: /Mac/.test(navigator.platform),
      isWin: /Win/.test(navigator.platform), isLinux: /Linux/.test(navigator.platform), isSafari: false,
      resourcePathPrefix: "",
    });
    /* guarded view (td-census-fix integration, ruling on recon-bundles.md (a)):
       what a manifest isDesktopOnly:false plugin with no granted desktop capability
       sees. Mobile, so real bundles take their non-Node branch (Templater skips
       its system-command exec, obsidian-git picks isomorphic-git over the adapter
       instead of spawning git). Desktop-only plugins keep the desktop view. */
    const PlatformGuarded = Object.freeze(Object.assign({}, Platform, { isDesktop: false, isMobile: true, isDesktopApp: false, isMobileApp: true }));
    const API_VERSION = "1.14.4";   // = src-tauri plugin_store.rs COMPAT_APP_VERSION (T0 drift test)
    const vcmp = (a, b) => { const x = String(a).split("."), y = String(b).split("."); for (let i = 0; i < 3; i++) { const d = (+x[i] || 0) - (+y[i] || 0); if (d) return d; } return 0; };
    const requireApiVersion = v => vcmp(API_VERSION, v) >= 0;
    /* Keymap: the scope stack manager (app.keymap is the instance) + the
       modifier helpers. "Mod" is Meta on macOS and Ctrl elsewhere. */
    const IS_MAC = /Mac/.test(navigator.platform);
    const STACK = new WeakMap();   // Keymap instance -> its scope stack (never on the object)
    const ROOT = new WeakMap();    // Keymap instance -> its root scope (= app.scope)
    class Keymap {
      constructor(stack, root) { STACK.set(this, Array.isArray(stack) ? stack : []); if (root) ROOT.set(this, root); }
      /* stock: the app's root scope, the same object as app.scope (Excalidraw registers its
         modifier overrides on it and moves them to the front of keys); null when none */
      getRootScope() { return ROOT.get(this) || null; }
      pushScope(scope) {
        const st = STACK.get(this); if (!st || !scope) return;
        const i = st.lastIndexOf(scope); if (i >= 0) st.splice(i, 1);
        st.push(scope);
      }
      popScope(scope) {
        const st = STACK.get(this); if (!st) return;
        const i = st.lastIndexOf(scope); if (i >= 0) st.splice(i, 1);
      }
      static isModifier(evt, modifier) {
        if (!evt) return false;
        switch (modifier) {
          case "Mod": return !!(IS_MAC ? evt.metaKey : evt.ctrlKey);
          case "Ctrl": return !!evt.ctrlKey;
          case "Meta": return !!evt.metaKey;
          case "Shift": return !!evt.shiftKey;
          case "Alt": return !!evt.altKey;
          default: return false;
        }
      }
      static isModEvent(evt) {
        if (!evt) return false;
        const mod = Keymap.isModifier(evt, "Mod");
        if (mod && evt.altKey && evt.shiftKey) return "window";
        if (mod && evt.altKey) return "split";
        if (mod) return "tab";
        if (typeof MouseEvent === "function" && evt instanceof MouseEvent && evt.button === 1) return "tab";
        return false;
      }
    }
    Object.assign(K, { API_VERSION, Keymap, Platform, PlatformGuarded, requireApiVersion });
  });
  reg("core:notice", function (K) {
    const { host, be } = K;
    const { installDom, touch } = K;
    /* ------------------------------------------------------------ Notice */
    let noticeBox = null;
    const notices = [];   // live Notice objects, newest last — [pnotice:] census
    /* containerEl is the toast (click dismisses it); messageEl (= the older
       noticeEl) is the element inside it that holds the message. */
    class Notice {
      constructor(message, duration) {
        installDom();
        if (!noticeBox || !noticeBox.isConnected) { noticeBox = document.createElement("div"); noticeBox.className = "notice-container"; document.body.appendChild(noticeBox); }
        this.containerEl = document.createElement("div");
        this.containerEl.className = "notice";
        this.messageEl = this.containerEl.appendChild(document.createElement("div"));
        this.messageEl.className = "os-notice-message";
        this.noticeEl = this.messageEl;
        this.setMessage(message);
        this.containerEl.addEventListener("click", () => this.hide());
        noticeBox.appendChild(this.containerEl);
        notices.push(this);
        const ms = duration === undefined ? 5000 : duration;
        if (ms > 0) this._t = setTimeout(() => this.hide(), ms);
        touch();
      }
      setMessage(m) { if (m instanceof Node) { this.messageEl.textContent = ""; this.messageEl.appendChild(m); } else this.messageEl.textContent = m == null ? "" : String(m); touch(); return this; }
      hide() { clearTimeout(this._t); this.containerEl.remove(); const i = notices.indexOf(this); if (i >= 0) notices.splice(i, 1); touch(); }
    }
    for (const k of ["containerEl", "messageEl", "noticeEl"])   // declared on the class; every instance sets its own
      Object.defineProperty(Notice.prototype, k, { value: null, writable: true, configurable: true });

    /* ------------------------------------------------------------- Scope */
    /* A handler matches when its modifiers equal the event's exactly (null =
       any) and its key equals event.key ignoring case (null = any). Modifier
       lists are kept as one canonical string: "Mod" resolved for the platform,
       names sorted, joined with ",". A handler returning false ends the event
       (preventDefault); otherwise the next handler, then the parent, is tried. */
    const IS_MAC = /Mac/.test(navigator.platform);
    const MODS = ["Alt", "Ctrl", "Meta", "Shift"];
    const modsOf = e => MODS.filter(m => e[m.toLowerCase() + "Key"]).join(",");
    const VKEY = { 8: "Backspace", 9: "Tab", 13: "Enter", 16: "Shift", 17: "Ctrl", 18: "Alt", 19: "Pause", 20: "CapsLock",
      27: "Escape", 32: " ", 33: "PageUp", 34: "PageDown", 35: "End", 36: "Home", 37: "ArrowLeft", 38: "ArrowUp",
      39: "ArrowRight", 40: "ArrowDown", 45: "Insert", 46: "Delete", 91: "Meta", 106: "*", 107: "+", 109: "-", 110: ".", 111: "/",
      144: "NumLock", 145: "ScrollLock", 186: ";", 187: "=", 188: ",", 189: "-", 190: ".", 191: "/", 192: "`",
      219: "[", 220: "\\", 221: "]", 222: "'" };
    const vkeyOf = e => {   // the layout-independent key, from the key code
      const c = e.keyCode | 0;
      if ((c >= 65 && c <= 90) || (c >= 48 && c <= 57)) return String.fromCharCode(c);
      if (c >= 96 && c <= 105) return "Numpad" + (c - 96);
      if (c >= 112 && c <= 135) return "F" + (c - 111);
      return VKEY[c] || "Key" + c;
    };
    class Scope {
      constructor(parent) { this.parent = parent; this.keys = []; }
      register(modifiers, key, func) {
        const mods = modifiers == null ? null : modifiers.map(m => (m === "Mod" ? (IS_MAC ? "Meta" : "Ctrl") : m)).sort().join(",");
        const h = { scope: this, modifiers: mods, key: key == null ? null : key, func };
        this.keys.push(h); return h;
      }
      unregister(h) { const i = this.keys.indexOf(h); if (i >= 0) this.keys.splice(i, 1); }
      handle(e) {
        const mods = modsOf(e), k = String(e.key).toLowerCase();
        const ctx = { modifiers: mods, key: e.key, vkey: vkeyOf(e) };
        for (const h of this.keys.slice()) {
          if (h.modifiers !== null && h.modifiers !== mods) continue;
          if (h.key !== null && String(h.key).toLowerCase() !== k) continue;
          const r = h.func(e, ctx);
          if (r === false) { e.preventDefault(); return false; }
        }
        return this.parent ? this.parent.handle(e) : true;
      }
    }
    const scopes = [];
    window.addEventListener("keydown", e => {   // a pushed scope (open plugin Modal / keymap.pushScope) sees keys first
      const s = scopes[scopes.length - 1];
      if (!s) return;
      if (s.handle(e) === false) e.stopImmediatePropagation();
    }, true);

    Object.assign(K, { Notice, Scope, notices, scopes });
  });
  reg("core:registry", function (K) {
    const { host, be } = K;
    /* --------------------------------------------- app + plugin registry */
    const registry = new Map();      // id -> {plugin, manifest, cmds:Set, ribbon:Set, status:Set, tabs:Set, styleEl}
    // manifests (id -> installed manifest), enabledIds (.obsidian/community-plugins.json order), trusted, layoutReady: K.st (api.js)



    const bridge = { editor: null };
    const statusBox = () => {
      let b = document.getElementById("os-plugin-statusbar");
      if (!b) { b = document.createElement("div"); b.id = "os-plugin-statusbar"; b.className = "status-bar os-plugin-statusbar"; document.body.appendChild(b); }
      return b;
    };

    Object.assign(K, { bridge, registry, statusBox });
  });
  reg("core:moment", function (K) {
    const { host, be } = K;
    function loadMoment() {
      if (window.moment) return Promise.resolve();
      return new Promise(res => {
        const s = document.createElement("script");
        s.src = "vendor/moment.min.js"; s.onload = s.onerror = () => res();
        document.head.appendChild(s);
      });
    }
    Object.assign(K, { loadMoment });
  });
  reg("core:keymap", function (K) {
    const { host, be } = K;
    const { Keymap, Scope, scopes } = K;
    const keymap = new Keymap(scopes, new Scope());   // app.keymap: drives the keydown scope stack (core:notice); its root scope is app.scope
    Object.assign(K, { keymap });
  });
  reg("core:commands", function (K) {
    const { host, be } = K;
    const { registry } = K;
    const cmdObj = c => c.obs || { id: c.id, name: c.name, callback: c.run };
    const commands = {
      get commands() { const o = {}; if (typeof CMDS !== "undefined") for (const c of CMDS) o[c.id] = cmdObj(c); return o; },
      get editorCommands() { return {}; },
      listCommands() { return typeof CMDS === "undefined" ? [] : CMDS.map(cmdObj); },
      findCommand(id) { const c = typeof CMDS !== "undefined" && CMDS.find(x => x.id === id); return c ? cmdObj(c) : undefined; },
      executeCommandById(id) {
        const c = typeof CMDS !== "undefined" && CMDS.find(x => x.id === id);
        if (!c) return false;
        try { if (c.when && !c.when()) return false; c.run(); return true; } catch (e) { console.error("[plugin] command " + id + ":", e); return false; }
      },
      executeCommand(cmd) { return cmd && commands.executeCommandById(cmd.id); },
      removeCommand(id) { K.removeCmd(id); },
      addCommand(cmd) { return K.addCmd(null, cmd); },
    };
    /* app.plugins.manifests, stock's shape: keyed by the manifest's OWN id (a dir whose
       manifest has no id is not listed; two dirs claiming one id: the later dir in sort
       order wins), each entry the manifest's fields plus dir (vault-relative plugin
       folder) and author ("" when absent/null). The SAME object between loads (a
       plugin's edits stay visible), a NEW one after loadManifests(). K.st.manifests
       (dir -> manifest, what the loader runs by) is untouched. */
    let manView = null, manSrc = null;
    const manifestsView = () => {
      if (manView && manSrc === K.st.manifests) return manView;
      const o = {}, src = K.st.manifests || {};
      for (const dir of Object.keys(src).sort()) {
        const m = src[dir];
        if (!m || typeof m !== "object" || Array.isArray(m) || !m.id) continue;
        const e = Object.assign({}, m, { dir: ".obsidian/plugins/" + dir });
        if (e.author == null) e.author = "";
        o[m.id] = e;
      }
      manSrc = K.st.manifests; manView = o;
      return o;
    };
    const plugins = {
      get manifests() { return manifestsView(); },
      /* re-read every <configDir>/plugins/<dir>/manifest.json (the backend's confined
         listing: dot dirs and names outside [A-Za-z0-9._-] are not plugin dirs here) */
      async loadManifests() { await K.refresh(); manifestsView(); },
      get plugins() { const o = {}; for (const [id, r] of registry) o[id] = r.plugin; return o; },
      get enabledPlugins() { return new Set(registry.keys()); },
      getPlugin(id) { const r = registry.get(id); return r ? r.plugin : null; },
      isEnabled() { return K.st.trusted; },
      enablePlugin: id => K.enable(id), disablePlugin: id => K.disable(id),
      enablePluginAndSave: id => K.saveEnabled(id, true), disablePluginAndSave: id => K.saveEnabled(id, false),
    };
    /* ---------------------------------------------------- app.hotkeyManager
       Stock's hotkey store, over main.js's ONE dispatcher (CMDS[].def +
       hkUser -> keymap, hkRebuild): defaultKeys / customKeys hold the
       Obsidian-shape arrays a plugin hands in (kept as given: same identity,
       null kept); every change is mirrored into the registry as chords so the
       keydown dispatcher, the palette hints and Settings > Hotkeys all see it.
       customKeys follows hkUser when the app changes it (Settings pane edit or
       reset, a reload of the saved map). Built-in defaults are main.js's own
       chord sets, seeded into defaultKeys once. Dispatch is main.js's chordOf:
       Alt on F1..F12 is ignored there (an Xvfb latch workaround), so Alt+F<n>
       cannot be bound — documented divergence. */
    const HK = { def: {}, cust: {}, mine: new Map(), user: null, seeded: new Set(), api: new Map() };   // api: ids set through this API since the last load/save -> value
    const hkHas = (o, k) => Object.prototype.hasOwnProperty.call(o, k);
    const hkFile = ch => { try { return typeof chordToFile === "function" ? chordToFile(ch) : null; } catch (_) { return null; } };
    /* Obsidian shape -> registry chord. main.js's chordOf never reports Alt on F1..F12 (Xvfb latch), so an
       Alt+F<n> binding is registered without Alt: it fires on Alt+F<n> and also on bare F<n> (divergence) */
    const hkToChord = h => { try { return h && typeof h === "object" && typeof fileToChord === "function" ? fileToChord(h).replace(/(^|\+)alt\+((?:shift\+)?f\d+)$/, "$1$2") : null; } catch (_) { return null; } };
    const hkCmd = id => (typeof CMDS !== "undefined" ? CMDS.find(c => c.id === id) : undefined);
    const hkUserObj = () => (typeof hkUser !== "undefined" ? hkUser : null);
    function hkSeed() {   // built-in defaults, once per id (a removed default stays removed)
      if (typeof CMDS === "undefined") return;
      for (const c of CMDS) {
        if (c.obs || HK.seeded.has(c.id)) continue;
        HK.seeded.add(c.id);
        const keys = (c.def || []).map(hkFile).filter(Boolean);
        if (keys.length && !hkHas(HK.def, c.id)) HK.def[c.id] = keys;
      }
    }
    function hkSync() {   // customKeys <- hkUser when the app (not this API) changed it
      const u = hkUserObj(); if (!u) return;
      if (HK.user !== u) {   // hkUser was replaced: the saved map was (re)loaded
        /* (the app reloads it on every plugin enable: an unsaved setHotkeys survives that; load() drops it) */
        HK.user = u; HK.mine.clear();
        for (const k of Object.keys(HK.cust)) delete HK.cust[k];
        for (const [id, chs] of Object.entries(u)) { HK.cust[id] = chs.map(hkFile).filter(Boolean); HK.mine.set(id, chs); }
        for (const [id, ks] of HK.api) { if (ks === undefined) delete HK.cust[id]; else HK.cust[id] = ks; hkApply(id); }
        return;
      }
      for (const id of Object.keys(HK.cust)) {
        if (HK.mine.get(id) === null || hkHas(u, id) && u[id] === HK.mine.get(id)) continue;
        if (hkHas(u, id)) { HK.cust[id] = u[id].map(hkFile).filter(Boolean); HK.mine.set(id, u[id]); }
        else { delete HK.cust[id]; HK.mine.delete(id); }   // reset to default in Settings
      }
      for (const [id, chs] of Object.entries(u)) if (!hkHas(HK.cust, id)) { HK.cust[id] = chs.map(hkFile).filter(Boolean); HK.mine.set(id, chs); }
    }
    let hkMgr = null;
    function hkApply(id) {   // defaultKeys/customKeys[id] -> the registry's chords, then rebuild the keymap
      const c = hkCmd(id);
      if (c) c.def = Array.isArray(HK.def[id]) ? HK.def[id].map(hkToChord).filter(Boolean) : [];
      const u = hkUserObj();
      if (u) {
        if (hkHas(HK.cust, id) && Array.isArray(HK.cust[id])) { const chs = HK.cust[id].map(hkToChord).filter(Boolean); u[id] = chs; HK.mine.set(id, chs); }
        else if (hkHas(HK.cust, id)) { u[id] = []; HK.mine.set(id, u[id]); }   // custom null (or not an array): nothing fires, the default neither
        else { delete u[id]; HK.mine.delete(id); }
        HK.user = u;
      }
      if (hkMgr) hkMgr.baked = false;
      if (typeof hkRebuild === "function") hkRebuild();
    }
    const HK_KEYS = { ArrowLeft: "←", ArrowUp: "↑", ArrowRight: "→", ArrowDown: "↓", PageUp: "Page Up", PageDown: "Page Down" };
    const hkKeyName = k => { const s = k == null ? "" : String(k); return hkHas(HK_KEYS, s) ? HK_KEYS[s] : s.charAt(0).toUpperCase() + s.slice(1); };
    function hkPrint(h) {   // Linux names: Mod and Ctrl both print Ctrl, Meta prints Win; fixed order
      const m = h.modifiers, out = [];
      if (m.includes("Mod")) out.push("Ctrl");
      if (m.includes("Ctrl")) out.push("Ctrl");
      if (m.includes("Meta")) out.push("Win");
      if (m.includes("Alt")) out.push("Alt");
      if (m.includes("Shift")) out.push("Shift");
      out.push(hkKeyName(h.key));
      return out.join(" + ");
    }
    class HotkeyManager {
      constructor() { this.baked = false; this.bakedHotkeys = []; this.bakedIds = []; }
      get defaultKeys() { hkSeed(); return HK.def; }
      get customKeys() { hkSync(); return HK.cust; }
      getDefaultHotkeys(id) { hkSeed(); return HK.def[id]; }
      getHotkeys(id) { hkSync(); return HK.cust[id]; }
      addDefaultHotkeys(id, keys) { hkSeed(); HK.def[id] = keys; hkApply(id); }
      removeDefaultHotkeys(id) { hkSeed(); delete HK.def[id]; hkApply(id); }
      setHotkeys(id, keys) { hkSync(); HK.cust[id] = keys; HK.api.set(id, keys); hkApply(id); }
      removeHotkeys(id) { hkSync(); delete HK.cust[id]; HK.api.set(id, undefined); hkApply(id); }
      /* the FIRST hotkey of (custom ?? default), as the UI prints it; "" for none */
      printHotkeyForCommand(id) {
        hkSeed(); hkSync();
        const ks = (hkHas(HK.cust, id) ? HK.cust[id] : undefined) ?? HK.def[id];
        return ks && ks.length ? hkPrint(ks[0]) : "";
      }
      /* every (command, hotkey) pair in effect, flattened; the keymap is rebuilt from the registry */
      bake() {
        hkSeed(); hkSync();
        this.bakedHotkeys = []; this.bakedIds = [];
        if (typeof CMDS !== "undefined") for (const c of CMDS) {
          const ks = (hkHas(HK.cust, c.id) ? HK.cust[c.id] : undefined) ?? HK.def[c.id];
          if (Array.isArray(ks)) for (const k of ks) { this.bakedHotkeys.push(k); this.bakedIds.push(c.id); }
        }
        if (typeof hkRebuild === "function") hkRebuild();
        this.baked = true;
      }
      /* keydown -> the bound command (enabled ones only); false = handled (scope convention) */
      onTrigger(e) {
        if (!e || typeof chordOf !== "function" || typeof keymap === "undefined") return true;
        const ch = chordOf(e), c = ch && keymap[ch];
        if (!c || (c.when && !c.when())) return true;
        c.run(); return false;
      }
      /* re-read the saved map (the app's config "hotkeys"); unsaved setHotkeys are dropped, like a reload */
      async load() { HK.api.clear(); if (typeof hkLoad === "function") await hkLoad(); hkSync(); this.baked = false; }
      async onConfigFileChange() { await this.load(); }
      async save() {
        hkSync();
        const map = {};
        for (const [id, ks] of Object.entries(HK.cust)) if (Array.isArray(ks))
          map[id] = ks.filter(k => k && typeof k === "object").map(k => ({ modifiers: Array.isArray(k.modifiers) ? k.modifiers.map(String) : [], key: String(k.key == null ? "" : k.key) }));
        HK.api.clear();
        await K.inv("set_hotkeys", { map });
      }
    }
    hkMgr = new HotkeyManager();
    /* a plugin command's own hotkeys are its defaults (the array as given); removing the command drops them */
    const hkCmdAdded = (id, hotkeys) => { hkSeed(); if (Array.isArray(hotkeys)) HK.def[id] = hotkeys; else delete HK.def[id]; hkApply(id); };
    const hkCmdRemoved = id => { delete HK.def[id]; if (hkMgr) hkMgr.baked = false; };
    Object.assign(K, { commands, plugins, hotkeyManager: hkMgr, hkCmdAdded, hkCmdRemoved });
  });
  reg("core:plugin", function (K) {
    const { host, be } = K;
    const { Component, EA, MDP, bridge, commands, installDom, mdpChanged, registry, setIcon, setting, statusBox, touch, views } = K;
    /* ---------------------------------------------------- commands -> CMDS */
    function hkChord(h) { try { return typeof fileToChord === "function" ? fileToChord(h) : null; } catch (_) { return null; } }
    function addCmd(owner, cmd) {
      if (typeof CMDS === "undefined" || !cmd || !cmd.id) return cmd;
      const pid = owner ? owner.manifest.id : "";
      const full = owner && !String(cmd.id).startsWith(pid + ":") ? pid + ":" + cmd.id : String(cmd.id);
      const name = owner && !String(cmd.name).startsWith(owner.manifest.name + ": ") ? owner.manifest.name + ": " + cmd.name : String(cmd.name);
      cmd.id = full; cmd.name = name;
      const safe = (f, ...a) => { try { return f(...a); } catch (e) { console.error("[plugin] command " + full + ":", e); return false; } };
      const ed = () => (bridge.editor ? bridge.editor() : null);   // item 5: {editor, view} of the active note, or null
      let when, run;
      if (cmd.editorCheckCallback) { when = () => { const x = ed(); return !!x && !!safe(cmd.editorCheckCallback, true, x.editor, x.view); }; run = () => { const x = ed(); if (x) safe(cmd.editorCheckCallback, false, x.editor, x.view); }; }
      else if (cmd.editorCallback) { when = () => !!ed(); run = () => { const x = ed(); if (x) safe(cmd.editorCallback, x.editor, x.view); }; }
      else if (cmd.checkCallback) { when = () => !!safe(cmd.checkCallback, true); run = () => safe(cmd.checkCallback, false); }
      else { when = undefined; run = () => cmd.callback && safe(cmd.callback); }
      removeCmd(full);
      const defs = (cmd.hotkeys || []).map(hkChord).filter(Boolean);
      CMDS.push({ id: full, name, def: defs, run, when, obs: cmd, plugin: pid });
      CMDS.sort((a, b) => a.name.localeCompare(b.name));
      K.hkCmdAdded(full, cmd.hotkeys);   // defaultKeys[full] = the hotkeys as given; the registry chords follow
      if (owner) registry.get(pid) && registry.get(pid).cmds.add(full);
      if (typeof hkRebuild === "function") hkRebuild();
      touch();
      return cmd;
    }
    function removeCmd(id) {
      if (typeof CMDS === "undefined") return;
      const i = CMDS.findIndex(c => c.id === id && c.obs);    // never a built-in command
      if (i < 0) return;
      const c = CMDS[i]; CMDS.splice(i, 1);
      K.hkCmdRemoved(id);
      const r = c.plugin && registry.get(c.plugin); if (r) r.cmds.delete(id);
      if (typeof hkRebuild === "function") hkRebuild();
      touch();
    }

    const CLI = new Map();   // registerCliHandler: command id -> {command, description, flags, handler, plugin}

    /* obsidian:// actions: workspace.protocolHandler.handlers is the action -> handler Map that
       registerObsidianProtocolHandler fills (keys exactly as given; a held action throws; the owner's
       unload deletes it). Registration only: nothing here opens or routes an OS-level URL. */
    const PROTO = new Map(), PROTO_OWN = new Map();   // action -> handler / action -> owning registration
    if (K.workspace && !K.workspace.protocolHandler)
      Object.defineProperty(K.workspace, "protocolHandler", { value: { handlers: PROTO }, configurable: true, enumerable: false, writable: false });

    /* ------------------------------------------------------------ Plugin */
    class Plugin extends Component {
      constructor(app_, manifest) { super(); this.app = app_; this.manifest = manifest; }
      _rec() { return registry.get(this.manifest.id); }
      addCommand(cmd) { const r = addCmd(this, cmd); this.register(() => removeCmd(r.id)); return r; }
      removeCommand(id) { removeCmd(String(id).includes(":") ? id : this.manifest.id + ":" + id); }
      addRibbonIcon(icon, title, cb) {
        installDom();
        const b = document.createElement("button");
        b.className = "side-dock-ribbon-action clickable-icon os-plugin-ribbon";
        b.dataset.plugin = this.manifest.id;
        b.title = title; b.setAttribute("aria-label", title);
        setIcon(b, icon);
        b.addEventListener("click", e => { try { cb(e); } catch (er) { console.error("[plugin] ribbon:", er); } });
        const rb = document.getElementById("ribbon");
        (rb || document.body).appendChild(b);
        const r = this._rec(); if (r) r.ribbon.add(b);
        this.register(() => { b.remove(); const r2 = this._rec(); if (r2) r2.ribbon.delete(b); touch(); });
        touch();
        return b;
      }
      addStatusBarItem() {
        installDom();
        const el = statusBox().createDiv({ cls: "status-bar-item plugin-" + this.manifest.id.replace(/[^A-Za-z0-9_-]/g, "") });
        el.dataset.plugin = this.manifest.id;
        const r = this._rec(); if (r) r.status.add(el);
        document.body.classList.add("os-has-plugin-status");
        this.register(() => { el.remove(); const r2 = this._rec(); if (r2) r2.status.delete(el); if (!statusBox().children.length) document.body.classList.remove("os-has-plugin-status"); touch(); });
        touch();
        return el;
      }
      addSettingTab(tab) {
        tab.id = tab.id || this.manifest.id; tab.name = tab.name || this.manifest.name;
        setting.addSettingTab(tab);
        const r = this._rec(); if (r) r.tabs.add(tab);
        this.register(() => { if (setting.activeTab === tab) setting.closeActiveTab(); setting.removeSettingTab(tab); const r2 = this._rec(); if (r2) r2.tabs.delete(tab); touch(); });
        touch();
      }
      async loadData() { return be ? be.io(this.manifest.id, "load") : null; }
      async saveData(data) { if (!be) throw new Error("plugin backend unavailable"); await be.io(this.manifest.id, "save", data === undefined ? null : JSON.parse(JSON.stringify(data))); }
      onExternalSettingsChange() {}
      registerView(type, factory) {   // plugin-views: hosted in the app's tabs / right sidebar (ui/plugins/views.js); unload detaches them
        try { PVHOST = views.PV; } catch (_) {}   // main.js loadActive asks it to mount a plugin-view tab
        views.registerView(this.manifest.id, type, factory);
        this.register(() => views.unregisterView(type));
      }
      /* SEAM S2: through the app's own registries (views.js); unload unregisters what was registered.
         A registry that throws (duplicate extension) throws here, and nothing is registered for unload. */
      registerHoverLinkSource(id, info) {
        const ws = this.app && this.app.workspace;
        ws.registerHoverLinkSource(id, info);
        this.register(() => ws.unregisterHoverLinkSource(id));
      }
      registerExtensions(exts, type) {
        const vr = this.app && this.app.viewRegistry;
        vr.registerExtensions(exts, type);
        this.register(() => vr.unregisterExtensions(exts));
      }
      registerMarkdownPostProcessor(p, sortOrder) {   // item 7: reading view + MarkdownRenderer.render; unload unregisters
        if (typeof p !== "function") return p;
        if (sortOrder !== undefined) p.sortOrder = sortOrder;
        MDP.post.push(p); mdpChanged();
        this.register(() => { const i = MDP.post.indexOf(p); if (i >= 0) { MDP.post.splice(i, 1); mdpChanged(); } });
        return p;
      }
      registerMarkdownCodeBlockProcessor(lang, p, sortOrder) {   // item 7: ```lang fences -> div.block-language-lang
        lang = String(lang);
        if (typeof p !== "function") return p;
        if (MDP.code.has(lang)) { console.warn("[plugin] " + this.manifest.id + ": code block processor for \"" + lang + "\" already registered"); return p; }
        if (sortOrder !== undefined) p.sortOrder = sortOrder;
        MDP.code.set(lang, p); mdpChanged();
        this.register(() => { if (MDP.code.get(lang) === p) { MDP.code.delete(lang); mdpChanged(); } });
        return p;
      }
      registerEditorExtension(ext) {   // editor-api: every open + future note view, removed on unload
        if (!EA) { console.warn("[plugin] " + this.manifest.id + ": registerEditorExtension — editor extensions not supported"); return; }
        this.register(EA.addExt(this.manifest.id, ext));
      }
      registerEditorSuggest(s) {   // SEAM S1: into workspace.editorSuggest (editor-api's live list), removed on unload
        const es = this.app.workspace.editorSuggest;
        es.addSuggest(s);
        this.register(() => es.removeSuggest(s));
      }
      registerObsidianProtocolHandler(action, handler) {
        if (PROTO.has(action)) throw new Error("registerObsidianProtocolHandler: action \"" + String(action) + "\" is already registered");
        const own = { plugin: this.manifest && this.manifest.id };
        PROTO.set(action, handler); PROTO_OWN.set(action, own);
        this.register(() => { if (PROTO_OWN.get(action) === own) { PROTO_OWN.delete(action); PROTO.delete(action); } });
      }
      /* opensidian has no Bases: the documented answer is false ("bases are not enabled") */
      registerBasesView(viewId, registration) { return false; }
      /* CLI handlers: ids are global; a second registration of one id throws. The
         handler is released when the plugin unloads. */
      registerCliHandler(command, description, flags, handler) {
        command = String(command);
        if (!command || /\s/.test(command)) throw new Error("registerCliHandler: invalid command id \"" + command + "\"");
        if (typeof handler !== "function") throw new TypeError("registerCliHandler: handler must be a function");
        if (CLI.has(command)) throw new Error("registerCliHandler: command \"" + command + "\" is already registered");
        const rec = { command, description: String(description == null ? "" : description), flags: flags || null, handler, plugin: this.manifest.id };
        CLI.set(command, rec);
        this.register(() => { if (CLI.get(command) === rec) CLI.delete(command); });
      }
      onUserEnable() {}
    }
    Plugin.prototype.settings = undefined;   // declared by the API (since 1.13.0); a plugin assigns its loaded data here

    Object.assign(K, { Plugin, addCmd, removeCmd });
  });
  reg("core:manager", function (K) {
    const { host, be } = K;
    const { EA, Plugin, app, ensureModel, focusTick, installDom, mdpCensus, notices, openModals, plugins, registry, removeCmd, setting, touch, vault, views, workspace } = K;
    // guarded = manifest isDesktopOnly:false (missing counts as desktop) and no granted desktop capability (none exist yet: td-desktop-caps)
    const isGuarded = m => !!m && m.isDesktopOnly === false;


    async function enable(id) {
      if (registry.has(id)) return { ok: true, id };
      if (!K.st.trusted) return host.fail(id, new Error("restricted mode is on"));
      const manifest = K.st.manifests[id];
      if (!manifest) return host.fail(id, new Error("not installed (no manifest.json)"));
      installDom();
      try { await ensureModel(); } catch (e) { touch(); return host.fail(id, new Error("vault model: " + (e && e.message || e))); }
      const r = await host.evaluate(id, isGuarded(manifest));
      if (!r.ok) { touch(); return r; }
      const ex = r.exports, Cls = ex && (ex.default || ex);
      if (typeof Cls !== "function" || !(Cls.prototype instanceof Plugin)) { touch(); return host.fail(id, new Error("main.js does not export a Plugin subclass")); }
      const rec = { cmds: new Set(), ribbon: new Set(), status: new Set(), tabs: new Set(), styleEl: null, plugin: null };
      registry.set(id, rec);
      try {
        const m = Object.assign({ dir: ".obsidian/plugins/" + id }, manifest);
        rec.plugin = new Cls(app, m);
        const css = await be.io(id, "styles").catch(() => null);
        if (typeof css === "string" && css) { rec.styleEl = document.createElement("style"); rec.styleEl.dataset.plugin = id; rec.styleEl.textContent = css; document.head.appendChild(rec.styleEl); }
        await rec.plugin.load();
      } catch (e) {
        teardown(id);
        touch();
        return host.fail(id, e);
      }
      host.status.set(id, { state: "loaded" });
      if (EA) EA.activate();   // editor-api: from the first loaded plugin on, note views carry editorInfoField + the editor-* hooks
      if (typeof hkLoad === "function") hkLoad().then(() => K.hotkeyManager.customKeys).catch(() => {});   // the user's chords for this plugin's ids (hkLoad drops unknown ids); customKeys re-syncs (keeps unsaved API sets)
      touch();
      return { ok: true, id };
    }
    function teardown(id) {
      const rec = registry.get(id); if (!rec) return;
      try { if (rec.plugin) rec.plugin.unload(); } catch (e) { console.error("[plugin] unload " + id + ":", e); }
      // belt and braces: anything still attributed to the plugin goes too
      for (const c of [...rec.cmds]) removeCmd(c);
      for (const b of rec.ribbon) b.remove();
      for (const s of rec.status) s.remove();
      for (const t of rec.tabs) setting.removeSettingTab(t);
      if (rec.styleEl) rec.styleEl.remove();
      for (const m of openModals.slice()) m.close();      // a modal is not attributable to its plugin; close them all
      registry.delete(id);
    }
    function disable(id) {
      if (!registry.has(id)) return { ok: true, id };
      teardown(id);
      host.status.set(id, { state: "unloaded" });
      touch();
      return { ok: true, id };
    }
    async function refresh() {
      const s = await be.state();
      K.st.trusted = !!s.trusted;
      K.st.enabledIds = (s.enabled || []).slice();
      K.st.manifests = {};
      for (const p of s.installed || []) if (p && p.id && p.manifest) K.st.manifests[p.id] = p.manifest;
      return s;
    }
    /* the user's toggle (settings -> host.setEnabled): a plugin switched on here
       that was not running gets onUserEnable() after its onload. A plugin calling
       app.plugins.enablePluginAndSave goes through saveEnabled (no onUserEnable). */
    async function saveEnabled(id, on) {
      await be.set({ id, on: !!on });
      await refresh();
      if (!K.st.trusted) { touch(); return { ok: true, id }; }   // restricted: the choice is saved, nothing runs
      return on ? enable(id) : disable(id);
    }
    async function setEnabled(id, on) {
      const was = registry.has(id);
      const r = await saveEnabled(id, on);
      const rec = on && !was && r && r.ok && registry.get(id);
      if (rec && rec.plugin && typeof rec.plugin.onUserEnable === "function") {
        try { rec.plugin.onUserEnable(); } catch (e) { console.error("[plugin] onUserEnable " + id + ":", e); }
      }
      return r;
    }
    async function setRestricted(restricted) {
      await be.set({ trusted: !restricted });
      await refresh();
      if (restricted) for (const id of [...registry.keys()]) disable(id);
      else for (const id of K.st.enabledIds) await enable(id);
      touch();
    }
    /* boot: called by main.js once the vault is entered. Restricted mode (the
       default) = one plugins_state call and nothing else: no prototype touched,
       no plugin script requested. */
    async function boot(vaultPathArg) {
      try {
        K.st.vaultName = String(vaultPathArg || "").replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "";
        app.appId = String(vaultPathArg || "").split("").reduce((h, c) => (Math.imul(h, 31) + c.charCodeAt(0)) | 0, 7).toString(16);
        await refresh();
        K.st.layoutReady = true; workspace.layoutReady = true;
        if (K.st.trusted) for (const id of K.st.enabledIds) await enable(id);
        workspace.trigger("layout-ready");
      } catch (e) { console.error("[plugin] boot:", e); }
      touch();
    }
    /* [plg:] census: restricted flag, per plugin state, and what is registered
       right now — cmds / ribbon / status / tabs counts summed over loaded
       plugins, so an unload phase can assert they drop to 0. */
    /* census-gate (issue #30): the two NON-census side effects censusTok carries, for the
       census-OFF title path (ui/main.js utLite) — plugins still see file-open /
       active-leaf-change and editorLivePreviewField still follows the leaf mode. */
    function censusTick() {
      if (K.st.modelOn) focusTick();
      if (EA) EA.tick();
    }
    function censusTok() {
      if (K.st.modelOn) focusTick();   // file-open / active-leaf-change detection (workspace section)
      if (!be || (!K.st.trusted && !host.status.size)) return K.st.trusted ? "" : " [plg:R]";
      let c = 0, rb = 0, st = 0, tb = 0;
      for (const r of registry.values()) { c += r.cmds.size; rb += r.ribbon.size; st += r.status.size; tb += r.tabs.size; }
      // a failed plugin carries its error (first 80 chars, census-safe) so a red phase says WHY
      const ids = [...host.status].map(([id, s]) => id + "=" + s.state + (s.state === "failed" && s.error ? "(" + String(s.error).replace(/[[\]|,=()\s]+/g, "_").slice(0, 80) + ")" : "")).join(",");
      const nt = notices.length ? " [pnotice:" + notices.map(n => n.noticeEl.textContent.replace(/[[\]|]/g, "").slice(0, 60)).join("|") + "]" : "";
      const md = openModals.length ? " [pmodal:" + openModals.length + "]" : "";
      if (EA) EA.tick();   // editor-api: editorLivePreviewField follows the focused tab's mode
      return " [plg:" + (K.st.trusted ? "T" : "R") + "|" + ids + "] [preg:c" + c + ",r" + rb + ",s" + st + ",t" + tb + "]" + nt + md + views.censusTok() + mdpCensus() + (EA ? EA.tok() : "");
    }

    /* ------------------------------------------------------ app.customCss
       Stock's snippet/theme manager, as a view over main.js's own vault CSS
       state (vaultSnips / vaultSnipsOn / vaultThemesScan / vaultTheme) and its
       one injection authority (snipInject/snipRemove, themeInject/themeRemove):
       a plugin toggling a snippet or picking a theme moves the same file
       (appearance.json via the backend) and the same <style> elements the
       settings pane does. Listing stays the backend's (themefs.rs): its theme
       predicate is stock's (manifest.json + theme.css + name === dir); its
       snippet listing is *.css, no dotfiles, no dirs — ".CSS" in another case
       is not listed (documented divergence). The writes are serialized so two
       quick calls land in call order. Sync state first (stock's setters are
       synchronous), then the file, then the DOM; a refused write reverts the
       state and says why. */
    /* main.js names, read at CALL time (main.js loads after this file); its
       top-level const/let bindings are not window properties, so each is named */
    const MJS = {
      say: () => typeof say === "function" && say, renderSnipCtl: () => typeof renderSnipCtl === "function" && renderSnipCtl,
      renderThemeCtl: () => typeof renderThemeCtl === "function" && renderThemeCtl, armCssReload: () => typeof armCssReload === "function" && armCssReload,
      updateTitle: () => typeof updateTitle === "function" && updateTitle, snipInject: () => typeof snipInject === "function" && snipInject,
      snipRemove: () => typeof snipRemove === "function" && snipRemove, themeInject: () => typeof themeInject === "function" && themeInject,
      themeRemove: () => typeof themeRemove === "function" && themeRemove, effTheme: () => typeof effTheme === "function" && effTheme,
    };
    const mjs = n => { try { return MJS[n]() || null; } catch (_) { return null; } };
    const g = {   // main.js top-level `let` bindings live in the global lexical scope, not on window
      snips: () => (typeof vaultSnips !== "undefined" ? vaultSnips : []),
      on: () => (typeof vaultSnipsOn !== "undefined" ? vaultSnipsOn : []),
      scan: () => (typeof vaultThemesScan !== "undefined" ? vaultThemesScan : { listed: [] }),
      theme: () => (typeof vaultTheme !== "undefined" ? vaultTheme : ""),
    };
    const coll = new Intl.Collator(undefined, { numeric: true });
    const cfgDir = () => (K.vault && K.vault.configDir) || ".obsidian";
    let cssQ = Promise.resolve();
    const cssLater = f => { cssQ = cssQ.then(f).catch(e => console.error("[plugin] customCss:", e)); return cssQ; };
    const cssSay = (e, k) => { const s = mjs("say"); if (s) s(String(e && e.message || e), k); };
    const cssUi = () => {
      for (const n of ["renderSnipCtl", "renderThemeCtl", "armCssReload"]) { const f = mjs(n); if (f) try { f(); } catch (_) {} }
      try { if (typeof state !== "undefined" && state) { const u = mjs("updateTitle"); if (u) u(); } } catch (_) {}
    };
    let snipView = null, snipSrc = null, themeView = null, themeSrc = null;
    const enSet = new Set();
    class CustomCss extends K.Component {
      constructor() {
        super();
        this.oldThemes = []; this.extraStyleEls = [];
      }
      get theme() { return g.theme(); }
      get snippets() {
        const src = g.snips();
        if (snipView && snipSrc === src) return snipView;
        snipSrc = src; snipView = src.slice().sort(coll.compare);
        return snipView;
      }
      get enabledSnippets() {
        const on = g.on();
        if (enSet.size !== on.length || on.some((l, i) => [...enSet][i] !== l)) { enSet.clear(); for (const l of on) enSet.add(l); }
        return enSet;
      }
      /* name -> manifest fields + dir, for every LISTED theme. The entries exist
         as soon as the listing does (name === dir is the predicate); the
         manifest's other fields are merged in when it has been read. */
      get themes() {
        const sc = g.scan();
        if (themeView && themeSrc === sc) return themeView;
        themeSrc = sc; themeView = {};
        const view = themeView;
        for (const n of sc.listed || []) view[n] = { name: n, dir: this.getThemeFolder() + "/" + n };
        const ad = K.adapter;
        if (ad) for (const n of Object.keys(view)) {
          ad.read(this.getThemeFolder() + "/" + n + "/manifest.json").then(s => {
            const m = JSON.parse(s);
            if (m && typeof m === "object" && !Array.isArray(m)) Object.assign(view[n], m, { dir: view[n].dir });
          }).catch(() => {});
        }
        return view;
      }
      getSnippetsFolder() { return cfgDir() + "/snippets"; }
      getSnippetPath(name) { return this.getSnippetsFolder() + "/" + name + ".css"; }
      getThemeFolder() { return cfgDir() + "/themes"; }
      getThemePath(name) { return this.getThemeFolder() + "/" + name + "/theme.css"; }
      isThemeInstalled(name) { return (g.scan().listed || []).includes(name); }
      isDarkMode() { return document.body.classList.contains("theme-dark"); }
      /* re-list the snippets folder; an enabled snippet whose file appeared is
         applied, one whose file is gone stops painting but stays enabled */
      async readSnippets() {
        await cssLater(async () => {
          const l = await K.inv("snippets_scan");
          vaultSnips = Array.isArray(l) ? l : [];
          const has = l => typeof snipEls !== "undefined" && snipEls.has(l);
          for (const lb of g.on()) {
            if (vaultSnips.includes(lb) && !has(lb)) { const f = mjs("snipInject"); if (f) await f(lb); }
            else if (!vaultSnips.includes(lb) && has(lb)) { const f = mjs("snipRemove"); if (f) f(lb); }
          }
          cssUi();
        });
      }
      async readThemes() {
        await cssLater(async () => {
          const sc = await K.inv("themes_scan");
          vaultThemesScan = sc && Array.isArray(sc.listed) ? sc : { listed: [], excluded: [] };
          this.themes;   // build the new view (and start its manifest reads) now
          const ad = K.adapter, view = themeView;
          if (ad) await Promise.all(Object.keys(view).map(n => ad.read(this.getThemeFolder() + "/" + n + "/manifest.json")
            .then(s => { const m = JSON.parse(s); if (m && typeof m === "object" && !Array.isArray(m)) Object.assign(view[n], m, { dir: view[n].dir }); })
            .catch(() => {})));
          cssUi();
        });
      }
      requestLoadSnippets() { this.readSnippets(); }
      requestReadThemes() { this.readThemes(); }
      setCssEnabledStatus(name, on) {
        const lb = String(name), want = !!on, was = g.on().includes(lb);
        if (want === was) return;
        vaultSnipsOn = want ? g.on().concat([lb]) : g.on().filter(l => l !== lb);
        cssLater(async () => {
          try { await K.inv("set_snippet_enabled", { label: lb, on: want }); }
          catch (e) {
            if (g.on().includes(lb) === want) vaultSnipsOn = want ? g.on().filter(l => l !== lb) : g.on().concat([lb]);
            cssSay(e, "snip"); cssUi(); return;
          }
          if (want && g.snips().includes(lb)) { const f = mjs("snipInject"); if (f) await f(lb); }
          if (!want) { const f = mjs("snipRemove"); if (f) f(lb); }
          cssUi();
        });
      }
      /* the theme name is taken as given (stock: an uninstalled name is kept;
         null/undefined/"" = no theme). What PAINTS for a name the listing does
         not have is main.js's effTheme rule (the default theme), not stock's
         base sheet — documented divergence. */
      setTheme(name) {
        const n = name == null || name === "" ? "" : String(name), prev = g.theme();
        vaultTheme = n;
        cssLater(async () => {
          try { await K.inv("set_css_theme", { name: n }); }
          catch (e) { if (g.theme() === n) vaultTheme = prev; cssSay(e, "theme"); cssUi(); return; }
          const eff = mjs("effTheme"), t = eff ? eff() : "";
          if (t) { const f = mjs("themeInject"); if (f) await f(t); } else { const f = mjs("themeRemove"); if (f) f(); }
          cssUi();
        });
      }
    }
    Object.defineProperty(app, "customCss", { value: new CustomCss(), enumerable: true, configurable: true, writable: true });
    Object.defineProperty(app, "hotkeyManager", { value: K.hotkeyManager, enumerable: true, configurable: true, writable: true });

    Object.assign(K, { boot, censusTick, censusTok, disable, enable, refresh, saveEnabled, setEnabled, setRestricted });
  });
})();
