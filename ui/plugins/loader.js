// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* plugin-loader (recon M1), ledger item 3: the community plugin LOADER and the
   require() shim. Loaded by index.html BEFORE main.js and before any plugin.

   How a plugin's code gets in (the page CSP has no 'unsafe-eval', no blob:):
     1. load(id) appends <script src="opensidian-plugin://localhost/<id>/main.js?n=K">;
        Rust (src-tauri/src/plugins.rs) serves it only when the vault is trusted,
        the id is enabled and the file is confined — wrapped as
          ;(function(){ __osPluginDefine("<id>", function(module,exports,require){ ...main.js... }) })();
     2. __osPluginDefine hands the factory to the load that is PENDING for that
        id; a define nobody asked for (a plugin defining a second id, a stray
        script) is ignored.
     3. the factory runs with a fresh module/exports and makeRequire(id). Any
        throw — a require() we refuse, a top-level bug — fails THAT plugin:
        status "failed" + the error, the app keeps running.

   require(name):
     "obsidian"                 -> the API object (ui/plugins/api.js, via host.api)
     "@codemirror/*", "@lezer/*" -> window.__osModules[name] (ui/vendor/codemirror/cm6.js:
                                   the SAME instances ui/editor.js builds its
                                   EditorView from); a name cm6.js does not ship
                                   -> Error "editor extensions not supported yet"
     "path", "node:path",        -> window.__osNodeModules (ui/plugins/node-leaves.js): pure
     "path/posix" (+ node:)        leaves, one frozen POSIX path object, no I/O
     "fs", "fs/promises" (+ node:) -> ui/plugins/node-modules.js (host.nodeModules):
                                   the vault-confined fs facade over Rust plugin_vfs
     events, util, os, crypto,   -> ui/plugins/node-desktop.js (host.desktopNode): the
     buffer, process, url, ...     trusted-vault Node surface, one view per plugin
     any other Node/Electron     -> an inert stub that throws on use (node-modules.js)
     builtin (child_process, ...)
     anything else               -> throws "unknown module"

   Privileges: the Tauri invoke handle is captured HERE, at boot, into this
   closure. Only host.grant() (one call, made by api.js before any plugin can
   load) receives it. Before the first plugin script is requested, seal() deletes
   window.__TAURI__ and narrows window.__osPluginHost to {censusTok}; see
   /workspace/goal/plugin-loader/security.md. */
(function () {
  "use strict";
  const T = window.__TAURI__;
  const invoke = T && T.core && T.core.invoke ? T.core.invoke.bind(T.core) : null;
  const listen = T && T.event && T.event.listen ? T.event.listen.bind(T.event) : null;   // item 5: vault-changed for the plugin vault model
  const SCHEME = "opensidian-plugin://localhost/";
  const NODE = window.__osNodeModules;   // captured at boot (index.html loads node-leaves.js first)

  const status = new Map();   // id -> {state: "loading"|"loaded"|"failed"|"unloaded", error?}
  const pending = new Map();  // id -> {resolve(factory)}
  let seq = 0, granted = false;

  /* cp-unrestricted (operator ruling 2026-10-18, stock parity): guarded mode is
     retired. Plugin code only runs in a trusted vault (the flip), and there every
     plugin gets the desktop view stock gives it: the desktop "obsidian" (Platform
     desktop, adapter instanceof FileSystemAdapter) and the Node surface of
     ui/plugins/node-desktop.js. A builtin that has no member there yet binds the
     inert stub (throws on use), so a bundle that only binds it at module scope
     still loads. api-core still passes its old guarded flag; it is ignored. */
  /* top5-func: Obsidian's bundled @codemirror/language carries one export upstream 6.12.4 does
     not: tokenClassNodeProp, the NodeProp<string> a syntax node's CSS token classes live under
     (dataview's Live Preview inline queries read node.type.prop(tokenClassNodeProp)). Plugins get
     the vendored module's own members (the editor's instances) plus that prop, a real
     @lezer/common NodeProp, one per page. While a plugin is loaded the note editor carries a
     Markdown syntax tree whose inline-code nodes carry it (editor-api.js mdLang); any other node
     answers undefined, as for an untagged node. */
  let langX = null, tokProp = null;
  function tokenClassNodeProp() {   // one per page: the editor tags its inline-code nodes with this same prop (editor-api.js mdLang)
    if (tokProp === null) { const C = window.__osModules && window.__osModules["@lezer/common"]; tokProp = C && typeof C.NodeProp === "function" ? new C.NodeProp() : undefined; }
    return tokProp;
  }
  function languageX(m) {
    if (langX && langX.base === m) return langX.mod;
    const prop = tokenClassNodeProp();
    const mod = Object.freeze(Object.assign(Object.create(null), m, prop ? { tokenClassNodeProp: prop } : {}));
    langX = { base: m, mod };
    return mod;
  }
  function makeRequire(id) {
    const dv = host.desktopNode ? host.desktopNode.view(id) : null;
    const require = function require(name) {
      name = String(name);
      if (name === "obsidian") {
        const a = host.api;
        if (!a) throw new Error(id + ': require("obsidian"): the plugin API is not available');
        return dv && dv.api ? dv.api(a) : a;   // node-desktop.js lays stock request / requestUrl over it
      }
      if (name.startsWith("@codemirror/") || name.startsWith("@lezer/")) {
        const m = window.__osModules && Object.prototype.hasOwnProperty.call(window.__osModules, name) ? window.__osModules[name] : undefined;
        if (m) return name === "@codemirror/language" ? languageX(m) : m;
        throw new Error(id + ': require("' + name + '"): editor extensions not supported yet');
      }
      const leaf = NODE && NODE.get(name);   // ui/plugins/node-leaves.js: pure leaves (path)
      if (leaf) return leaf;
      const nm = host.nodeModules && host.nodeModules.resolve(name);   // ui/plugins/node-modules.js: fs, fs/promises (vault-confined)
      if (nm) return dv ? dv.own(nm) : nm;
      const deny = id + ': require("' + name + '"): this member is not available in opensidian yet';
      const d = dv && dv.resolve(name, deny);   // ui/plugins/node-desktop.js: the trusted-vault Node surface
      if (d) return d;
      const s = host.nodeModules && host.nodeModules.stub(name, deny);
      if (s) return s;
      throw new Error(id + ': require("' + name + '"): unknown module');
    };
    if (dv) dv.bindRequire(require);
    return require;
  }
  // the CommonJS free variables plugins.rs wrap_cjs binds after module,exports,require
  function freeVars(id) {
    const dv = host.desktopNode ? host.desktopNode.view(id) : null;
    if (!dv) return [undefined, window, undefined, Error, "", "", undefined];
    return [dv.process, dv.global, dv.Buffer, dv.Error, dv.filename, dv.dirname, dv.SharedArrayBuffer];
  }

  // the Rust wrapper calls this; only a define for a PENDING load is honoured
  Object.defineProperty(window, "__osPluginDefine", {
    configurable: false, writable: false, enumerable: false,
    value: function (id, factory) {
      const p = pending.get(id);
      if (!p || typeof factory !== "function") return;
      pending.delete(id);
      p.resolve(factory);
    },
  });

  /* item 7, the seal: runs ONCE, right before the first plugin script is
     requested (evaluate). Everything the app needs was captured at load time
     (main.js TAURI/inv, otel.js, this closure, api.js's grant, settings.js's
     host), so from here on:
       - window.__TAURI__ is deleted (withGlobalTauri assigns it plainly; if a
         future Tauri makes it non-configurable we overwrite it, and the census
         says so),
       - window.__osPluginHost becomes a frozen facade with censusTok only: no
         grant, no setRestricted/setEnabled/enable/evaluate/status for plugin code.
     This is defence in depth, not a sandbox: plugins share the page's realm
     (main.js top-level bindings, __TAURI_INTERNALS__). See security.md. */
  let sealed = false;
  /* top5-func: data: URLs for plugin code, installed once right before the first plugin script
     (seal), so a restricted page never has it. The page CSP is NOT touched (font-src 'self',
     connect-src ipc:/self stay). A data: URL carries its bytes inline: decoding it in-page reads
     nothing from anywhere and grants nothing a plugin cannot already do (atob + new
     FontFace(name, bytes) is allowed under any CSP). Excalidraw ships its fonts as data: URLs —
     new FontFace("Virgil", "url(data:font/woff2;base64,...)") and fetch(dataUrl) to subset them
     into an SVG export — and both were refused ("Failed to load font", "Failed to fetch font
     family"). So fetch(data:) answers a Response of the decoded bytes (GET only; a malformed URL
     rejects with a TypeError like the native one), and a FontFace whose first source is
     url(data:...) is built from those bytes. Every other URL goes to the native fetch / FontFace
     and its CSP, untouched. */
  function dataUrlBytes(u) {
    const m = /^data:([^,]*),([\s\S]*)$/i.exec(String(u));
    if (!m) throw new TypeError("malformed data: URL");
    const b64 = /;\s*base64\s*$/i.test(m[1]);
    const type = (b64 ? m[1].replace(/;\s*base64\s*$/i, "") : m[1]).trim() || "text/plain;charset=US-ASCII";
    let bytes;
    if (b64) {
      const s = atob(m[2].replace(/%([0-9a-f]{2})/gi, (_, h) => String.fromCharCode(parseInt(h, 16))).replace(/\s+/g, ""));
      bytes = new Uint8Array(s.length);
      for (let i = 0; i < s.length; i++) bytes[i] = s.charCodeAt(i);
    } else {
      const out = [], enc = new TextEncoder(), d = m[2];
      for (let i = 0; i < d.length;) {
        if (d[i] === "%" && /^[0-9a-f]{2}$/i.test(d.substr(i + 1, 2))) { out.push(parseInt(d.substr(i + 1, 2), 16)); i += 3; continue; }
        let j = i; while (j < d.length && d[j] !== "%") j++;
        if (j === i) j = i + 1;
        for (const b of enc.encode(d.slice(i, j))) out.push(b);
        i = j;
      }
      bytes = Uint8Array.from(out);
    }
    return { type, bytes };
  }
  function dataUrls() {
    const nf = window.fetch;
    if (typeof nf === "function") {
      const fetch = function fetch(input, init) {
        const u = typeof input === "string" ? input : input instanceof URL ? input.href : input && typeof input.url === "string" ? input.url : String(input);
        if (!/^\s*data:/i.test(u)) return nf.apply(window, arguments);
        const meth = (init && init.method) || (input && typeof input === "object" && input.method) || "GET";
        if (!/^get$/i.test(String(meth))) return Promise.reject(new TypeError("Failed to fetch: a data: URL answers GET only"));
        let d;
        try { d = dataUrlBytes(u.trim()); } catch (e) { return Promise.reject(new TypeError("Failed to fetch: " + (e && e.message || "malformed data: URL"))); }
        return Promise.resolve(new Response(d.bytes, { status: 200, statusText: "OK", headers: { "Content-Type": d.type } }));
      };
      try { window.fetch = fetch; } catch (e) { console.error("[plugin] data: fetch:", e); }
    }
    const NF = window.FontFace;
    if (typeof NF === "function") {
      const FontFace = class FontFace extends NF {
        constructor(family, source, descriptors) {
          let src = source;
          if (typeof source === "string") {
            const m = /^\s*url\(\s*(["']?)(data:[^"')]*)\1\s*\)/i.exec(source);
            if (m) { try { src = dataUrlBytes(m[2]).bytes.buffer; } catch (_) { src = source; } }
          }
          super(family, src, descriptors);
        }
      };
      try { window.FontFace = FontFace; } catch (e) { console.error("[plugin] data: FontFace:", e); }
    }
  }

  function seal() {
    if (sealed) return;
    sealed = true;
    dataUrls();
    try { delete window.__TAURI__; } catch (_) {}
    if ("__TAURI__" in window) { try { Object.defineProperty(window, "__TAURI__", { value: undefined, writable: false, configurable: false }); } catch (_) {} }
    /* census-gate (issue #30): censusTick = censusTok's two non-census side effects, for the
       census-OFF title path. NON-enumerable, so the narrowed facade still has ONE key and
       [pseal:0,1] (phase plgsample) keeps meaning "narrowed to {censusTok}". */
    const facade = Object.freeze(Object.defineProperty({ censusTok: () => (host.censusTok ? host.censusTok() : "") + sealTok() }, "censusTick", { value: () => { if (host.censusTick) host.censusTick(); }, enumerable: false }));
    try { Object.defineProperty(window, "__osPluginHost", { value: facade, configurable: false, writable: false, enumerable: false }); } catch (e) { console.error("[plugin] seal host:", e); }
    /* cp-unrestricted (stock parity): the seal runs only before the first plugin script,
       and plugin scripts are served only for a trusted vault. Stock's page has Node
       integration, so bundles reach builtins through window.require (obsidian-importer:
       window.require("node:crypto")) and the electron module through window.electron
       (pdf-plus: window.electron.remote). Both resolve through the page's own view of
       node-desktop.js, the same surface a plugin's require gets. */
    if (host.desktopNode) {
      try {
        const pageReq = makeRequire("window");
        Object.defineProperty(window, "require", { value: pageReq, configurable: true, writable: true, enumerable: false });
        Object.defineProperty(window, "electron", { get: () => pageReq("electron"), configurable: true, enumerable: false });
      } catch (e) { console.error("[plugin] page require:", e); }
    }
  }
  // [pseal:<__TAURI__ still visible 0|1>,<public host keys>] — after the seal only
  const sealTok = () => " [pseal:" + (window.__TAURI__ ? 1 : 0) + "," + Object.keys(window.__osPluginHost || {}).length + "]";

  function fail(id, err) {
    const msg = (err && err.message) || String(err);
    status.set(id, { state: "failed", error: msg });
    console.error("[plugin] " + id + " failed: " + msg);
    if (evalWanted && evalRefused(err)) cspReloadNow();   // trusted vault on a no-eval page (cspCheck)
    return { ok: false, id, error: msg };
  }

  /* fetch + evaluate one plugin's main.js. Resolves {ok, id, exports, error};
     never rejects — a broken plugin must not take the caller down. */
  function evaluate(id) {   // api-core passes a retired guarded flag (makeRequire)
    if (!/^[A-Za-z0-9_-][A-Za-z0-9._-]{0,127}$/.test(id)) return Promise.resolve(fail(id, new Error("bad plugin id")));
    if (pending.has(id)) return Promise.resolve(fail(id, new Error("already loading")));
    seal();   // item 7: no plugin script is ever requested while __TAURI__ / the full host are public
    status.set(id, { state: "loading" });
    return new Promise(resolve => {
      const s = document.createElement("script");
      let done = false;
      const fin = r => { if (done) return; done = true; pending.delete(id); s.remove(); resolve(r); };
      pending.set(id, {
        resolve: factory => {
          const module = { exports: {} };
          try {
            factory.call(module.exports, module, module.exports, makeRequire(id), ...freeVars(id));
          } catch (e) { fin(fail(id, e)); return; }
          fin({ ok: true, id, exports: module.exports });
        },
      });
      // a script that 403s/404s fires onerror; one that loads but never defines
      // (syntax error, refused wrapper) fires onload with nothing pending resolved
      // fail() WRITES status, so it must only run when nothing settled first: an
      // eager fin(fail(..)) after a factory that threw overwrote the real error
      // (e.g. require("@codemirror/..")) with "did not define" (plgprobe caught it)
      s.onerror = () => { if (!done) fin(fail(id, new Error("main.js refused or missing (restricted mode, not enabled, or not installed)"))); };
      s.onload = () => setTimeout(() => { if (!done) fin(fail(id, new Error("main.js did not define a plugin (syntax error?)"))); }, 0);
      s.src = SCHEME + encodeURIComponent(id) + "/main.js?n=" + (++seq);
      document.head.appendChild(s);
    });
  }

  /* cp-unrestricted: the page CSP is fixed when the page loads — Rust page_csp
     gives a trusted vault's page 'unsafe-eval' + blob: workers, every other page
     the restricted CSP — and plugins_state says which one this page got
     (pageEval). When the trust flag and the page disagree:
       - flag OFF, page has eval (restricted mode was just turned on): reload
         now, so the negative control is the real header, not a leftover one;
       - flag ON, page has no eval (restricted mode was just turned off, or the
         vault was bound after the page loaded): plugins load without a reload
         (most never eval); the FIRST time anything on the page is refused eval
         or a blob: script/worker, or a same-origin connection (the fs *Sync
         route) by the CSP (a securitypolicyviolation, or a plugin that fails
         with an EvalError), the page reloads into the
         trusted CSP and the plugin loads again there.
     Every reload waits the save debounce so a pending note write lands first.
     Two reloads in a row that still disagree stop with a console warning
     instead of looping. */
  let cspReload = false, evalWanted = false;
  function cspReloadNow() {
    const KEY = "osCspReload";
    let n = 0;
    try { n = +sessionStorage.getItem(KEY) || 0; } catch (_) { /* no storage: the in-page flag stops a second one */ }
    if (cspReload) return;
    if (n >= 2) { console.warn("[plugins] page CSP still disagrees with the vault trust flag after 2 reloads"); return; }
    cspReload = true;
    try { sessionStorage.setItem(KEY, String(n + 1)); } catch (_) {}
    console.warn("[plugins] reloading the page into the CSP of the vault's trust state");
    Promise.resolve(invoke ? invoke("save_debounce_ms") : 0).catch(() => 0)
      .then(ms => setTimeout(() => location.reload(), (+ms || 0) + 300));
  }
  const evalRefused = e => e instanceof EvalError
    || /unsafe-eval|Content Security Policy|Refused to evaluate/i.test(String(e && e.message));
  window.addEventListener("securitypolicyviolation", e => {
    if (!evalWanted) return;
    const d = String(e.effectiveDirective || e.violatedDirective || ""), u = String(e.blockedURI || "");
    if ((/^(script-src|worker-src)/.test(d) && (u === "eval" || u.startsWith("blob"))) || /^worker-src/.test(d)
      // node-desktop.js fs *Sync: a same-origin synchronous XHR ('self' is only in the trusted connect-src)
      || (/^connect-src/.test(d) && u.startsWith(location.origin + "/"))) cspReloadNow();
  });
  function cspCheck(s) {
    if (!s || typeof s.pageEval !== "boolean") return s;
    const trusted = !!s.trusted;
    evalWanted = trusted && !s.pageEval;
    if (trusted === s.pageEval) { try { sessionStorage.removeItem("osCspReload"); } catch (_) {} return s; }
    if (!trusted) cspReloadNow();
    return s;
  }

  const host = {
    api: null,
    status,
    makeRequire,
    tokenClassNodeProp,
    evaluate,
    fail,
    /* ONE call, by ui/plugins/api.js at boot (before any plugin can run): hands
       it the narrow backend calls the API needs. A second call (a plugin
       trying to get the handle) gets nothing. */
    grant(fn) {
      if (granted) return false;
      granted = true;
      fn({
        state: () => invoke("plugins_state").then(cspCheck),
        set: a => invoke("plugins_set", a).then(cspCheck),
        io: (id, op, data) => invoke("plugin_io", data === undefined ? { id, op } : { id, op, data }),
        invoke,   // api.js's own vault calls (read/modify/...) — never handed to a plugin
        listen,   // api.js subscribes to vault-changed (Vault create/modify/delete/rename events)
      });
      return true;
    },
    snapshot: () => [...status].map(([id, s]) => id + "=" + s.state).join(","),
    /* cp-unrestricted: node-desktop.js captures this at boot (before the seal) for
       the desktop builtins it backs with Rust (child_process, ...). Only desk_*
       commands pass; Rust gates each spawn on the vault's trust flip. */
    desk: (cmd, args) => (invoke && /^desk_[a-z_]+$/.test(String(cmd)) ? invoke(String(cmd), args) : Promise.reject(new Error("desktop call refused"))),
  };
  // configurable until seal() swaps in the public facade (item 7)
  Object.defineProperty(window, "__osPluginHost", { value: host, configurable: true, writable: false, enumerable: false });
})();
