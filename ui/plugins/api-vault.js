// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* the "obsidian" module, part META (api-parity stage 0: split out of api.js,
   behaviour-neutral): TAbstractFile / TFile / TFolder, the vault model fed by
   the Rust watcher, the confined VFS core (host.vfs) + DataAdapter /
   FileSystemAdapter, Vault, the metadata cache, the `app` object + App, and
   the guarded view's FileSystemAdapter. Segment contract: see api-core.js. */
(function () {
  "use strict";
  const parts = window.__osApiParts || (window.__osApiParts = Object.create(null));
  const reg = (id, fn) => { if (!(id in parts)) parts[id] = fn; };
  reg("vault:model", function (K) {
    const { host, be } = K;
    const { Events, P, getLinkpath, loadMoment, normalizePath, parseYaml, touch } = K;
    class TAbstractFile { constructor() { this.vault = null; this.path = ""; this.name = ""; this.parent = null; } }
    class TFile extends TAbstractFile { constructor() { super(); this.basename = ""; this.extension = ""; this.stat = { ctime: 0, mtime: 0, size: 0 }; } }
    class TFolder extends TAbstractFile { constructor() { super(); this.children = []; } isRoot() { return this.path === "/" || this.path === ""; } }
    /* ------------------------------------- vault model (item 5)
       TFile/TFolder objects keyed by Obsidian path ("dir/Note.md", "dir",
       "img.png"); identity survives rename. The app names notes WITHOUT ".md"
       (index.rs notes_of), so every backend call strips it. Built only when a
       plugin is about to load (trusted + enabled): restricted mode never lists,
       reads or parses anything for plugins. Mutations made through the API go to
       the app's own commands (create_note / write_note / delete_note -> .trash /
       move_note / create_dir) and are then fed to main.js onVaultChanged, the
       same path an external edit takes, so open tabs and the tree stay right. */
    const files = new Map();                      // path -> TFile | TFolder (root not included)
    const vroot = new TFolder(); vroot.path = "/"; vroot.name = "";
    let modelP = null, filesP = null, unlisten = null;   // vaultName, modelOn, filesOn: K.st (api.js)
    const noteOf = p => (p.endsWith(".md") ? p.slice(0, -3) : null);
    const isCanvas = p => typeof p === "string" && p.endsWith(".canvas");   // JSON Canvas: app-owned canvas_io, not the note commands
    const inv = (c, a) => be.invoke(c, a);
    const cio = (op, path, data) => inv("canvas_io", data == null ? { op, path } : { op, path, data });
    function attach(f, path) {
      const i = path.lastIndexOf("/"), dir = i < 0 ? "" : path.slice(0, i);
      f.vault = vault; f.path = path; f.name = path.slice(i + 1);
      if (f instanceof TFile) { const d = f.name.lastIndexOf("."); f.basename = d > 0 ? f.name.slice(0, d) : f.name; f.extension = d > 0 ? f.name.slice(d + 1).toLowerCase() : ""; }
      const par = dir ? ensureFolder(dir) : vroot;
      f.parent = par; if (!par.children.includes(f)) par.children.push(f);
      files.set(path, f);
    }
    function detachF(f) {
      if (f.parent) { const i = f.parent.children.indexOf(f); if (i >= 0) f.parent.children.splice(i, 1); }
      if (files.get(f.path) === f) files.delete(f.path);
    }
    function ensureFolder(path) {
      const g = files.get(path); if (g instanceof TFolder) return g;
      const f = new TFolder(); attach(f, path); return f;
    }
    function ensureFile(path) {   // -> [file, created]
      const g = files.get(path); if (g instanceof TFile) return [g, false];
      const f = new TFile(); const now = Date.now(); f.stat = { ctime: now, mtime: now, size: 0 }; attach(f, path); return [f, true];
    }
    function removeTree(f) {
      if (f instanceof TFolder) for (const c of f.children.slice()) removeTree(c);
      detachF(f); caches.delete(f.path);
    }
    function movePath(f, np) {
      const op = f.path; detachF(f); caches.delete(op); attach(f, np);
      if (f instanceof TFolder) for (const c of f.children.slice()) movePath(c, np + c.path.slice(op.length));
    }
    async function buildModel() {
      const [notes, folders, imgs, cvs, media] = await Promise.all([inv("list_notes"), inv("list_folders"), inv("list_images").catch(() => []), cio("list").catch(() => []), cio("media").catch(() => [])]);
      const keep = new Set();
      for (const d of folders || []) { ensureFolder(d); keep.add(d); }
      for (const n of notes || []) { ensureFile(n + ".md"); keep.add(n + ".md"); }
      for (const p of imgs || []) { ensureFile(p); keep.add(p); }
      for (const p of cvs || []) { ensureFile(p); keep.add(p); }
      for (const p of media || []) { ensureFile(p); keep.add(p); }   // goal cv-journeys: audio / video (canvasfs media) are files a card plays
      /* plug-val: every other non-hidden file (a .js user script, .json, .txt) is a TFile too, as in stock: one
         confined-fs listing per folder (templater tp.user: Scripts/hello.js must be a child of its TFolder) */
      try {
        const lists = await Promise.all(["", ...(folders || [])].map(d => core.list(d).then(r => [d, r], () => [d, []])));
        for (const [d, r] of lists) for (const [n, k] of r || []) {
          if (k === "folder" || n.startsWith(".")) continue;
          const p = joinRel(d, n);
          if (!keep.has(p)) { ensureFile(p); keep.add(p); }
        }
      } catch (_) {}
      for (const [p, f] of [...files]) if (!keep.has(p) && files.get(p) === f) removeTree(f);
    }
    /* goal cv-journeys: the image / PDF / audio / video files only (notes are the watcher's, canvases the Canvas view's):
       a new one becomes a TFile and fires "create". ADD only: a file a plugin wrote (createBinary) is in no listing and
       must not vanish from the model, and a removal is the watcher's / the deleting verb's business. */
    async function syncMedia() {
      const [imgs, media] = await Promise.all([inv("list_images"), cio("media")]);
      const want = new Set([...(imgs || []), ...(media || [])]);
      for (const p of want) { if (files.has(p)) continue; const [f, made] = ensureFile(p); if (made) vault.trigger("create", f); }
    }
    async function syncFolders() {
      let ds; try { ds = await inv("list_folders"); } catch (_) { return; }
      const want = new Set(ds || []);
      for (const d of want) if (!files.has(d)) { const f = ensureFolder(d); vault.trigger("create", f); }
      for (const [p, f] of [...files]) if (f instanceof TFolder && !want.has(p) && !f.children.length && files.get(p) === f) { removeTree(f); vault.trigger("delete", f); }
    }
    /* one watcher batch (notes only, names without .md) -> Vault events, in
       Obsidian's order: rename, create, modify, delete; then the cache. */
    function applyChange(c) {
      if (!K.st.filesOn || !c) return;   // the file model is up (ensureFiles); indexFile parses only once the plugin runtime is (modelOn)
      for (const [o, n] of c.renamed || []) {
        const f = files.get(o + ".md");
        if (f) { movePath(f, n + ".md"); vault.trigger("rename", f, o + ".md"); indexFile(f); }
        else { const [g, made] = ensureFile(n + ".md"); if (made) vault.trigger("create", g); indexFile(g); }
      }
      for (const n of c.added || []) { const [f, made] = ensureFile(n + ".md"); if (made) vault.trigger("create", f); indexFile(f); }
      for (const n of c.modified || []) { const f = files.get(n + ".md"); if (f instanceof TFile) { f.stat.mtime = Date.now(); vault.trigger("modify", f); indexFile(f); } }
      for (const n of c.removed || []) { const f = files.get(n + ".md"); if (f) { removeTree(f); metadataCache.trigger("deleted", f, null); vault.trigger("delete", f); } }
      syncFolders();
    }
    /* the app's OWN note save (main.js afterWrite: editor autosave, ctrl+s, a note the app created). The
       watcher reports only external changes, so without this a plugin never saw its vault's "modify" for a
       note typed in the editor and the metadata cache kept the text from before the edit (Advanced Tables:
       acceptsTableEdit found no cached section covering the line just typed, so its Tab declined). Like stock:
       "create" for a new note, else "modify", then the file is re-indexed. No folder sync, no events before
       the file model is on (ensureFiles); re-indexed only once the plugin runtime is (indexFile). main.js captures host before the seal; plugins never see it. */
    host.noteWritten = (name, fresh) => {
      if (!K.st.filesOn || typeof name !== "string" || !name) return;
      if (fresh) { const [f, made] = ensureFile(name + ".md"); if (made) vault.trigger("create", f); indexFile(f); return; }
      const f = files.get(name + ".md");
      if (f instanceof TFile) { f.stat.mtime = Date.now(); vault.trigger("modify", f); indexFile(f); }
    };
    // an API mutation: tell main.js first (tabs, tree, bookmarks), then the plugins
    async function changed(c) {
      const full = Object.assign({ added: [], removed: [], modified: [], renamed: [] }, c);
      if (typeof onVaultChanged === "function") { try { await onVaultChanged(full); } catch (e) { console.error("[plugin] onVaultChanged:", e); } }
      applyChange(full);
    }
    // a vault file that is neither a note nor a canvas (an exported .svg, a .json, ...): text I/O over the confined VFS
    const plainFile = f => f instanceof TFile && noteOf(f.path) == null && !isCanvas(f.path);
    function mdOnly(f, what) {
      const n = f instanceof TFile ? noteOf(f.path) : null;
      if (n == null) throw new Error(what + ": only markdown notes are supported in opensidian M1 (" + (f && f.path) + ")");
      return n;
    }
    /* goal cv-model: the FILE model alone — the files Map (buildModel) kept current by the vault-changed
       subscription and the app's own saves (noteWritten). No moment, no window.app, no config, no metadata
       parse: what an app-owned consumer (Canvas: pickers, missing-file cards) needs in ANY vault, restricted
       mode included, with no community plugin enabled. main.js takes it before the seal (host.vaultModel)
       and hands it to Canvas, which calls it when a board opens. Idempotent; a failed build is retried. */
    function ensureFiles() {
      if (filesP) return filesP;
      filesP = (async () => {
        await buildModel();
        K.st.filesOn = true;
        if (!unlisten && be.listen) unlisten = await be.listen("vault-changed", e => applyChange(e.payload));
      })().catch(e => { filesP = null; throw e; });
      return filesP;
    }
    host.vaultModel = ensureFiles;
    /* goal cv-journeys: the app re-listed its explorer (main.js refreshTree) — re-list the file model too, so a file the
       watcher never reports (it reports notes only: an image pasted on or dropped onto a board) is a TFile before the
       card that shows it paints. A no-op until ensureFiles is up. */
    host.vaultSync = async () => { if (!K.st.filesOn) return; try { await syncMedia(); } catch (e) { console.error("[plugin] vaultSync:", e); } };
    /* goal cv-journeys: the app's OWN rename / move of a note (main.js applyRename: F2, the title rename, a drag-move). The
       watcher reconciles against the index and reports none of them, so the model kept the old path: a canvas file card
       relinked to the new name painted "not found". A Vault "rename", as for a plugin's renameFile. */
    host.noteMoved = (o, n) => { if (!K.st.filesOn || typeof o !== "string" || typeof n !== "string" || o === n) return; applyChange({ renamed: [[o, n]] }); };
    /* the runtime a plugin needs before its main.js is evaluated: moment (vendored,
       pinned — ui/vendor/moment.min.js), the file model (ensureFiles), the vault
       config and the metadata sweep. Idempotent; first enable() pays it. */
    function ensureModel() {
      if (modelP) return modelP;
      modelP = (async () => {
        await loadMoment();
        // Obsidian exposes the App as the global `app`, and bundled helper libs read
        // it bare (periodic-notes' obsidian-daily-notes-interface: "Can't find
        // variable: app"). Same object a plugin gets as this.app; set only here,
        // i.e. never in restricted mode.
        if (!("app" in window)) Object.defineProperty(window, "app", { value: K.app, configurable: true, enumerable: false, writable: false });
        await ensureFiles();
        await loadVaultConfig();
        await metadataTypeManager.load();
        K.st.modelOn = true;
        sweep();
      })().catch(e => { modelP = null; throw e; });
      return modelP;
    }

    /* td-vault-io: DataAdapter / FileSystemAdapter over the confined Rust
       backend (src-tauri/src/vaultfs.rs plugin_op, command plugin_vfs). The
       plugin sees ONE virtual root, VROOT = "/vault": basePath is that string,
       never the host path, and "/vault/x" and "x" name the same file. This file
       only maps the virtual root away; every other path is passed to Rust
       VERBATIM (no normalizePath: it would turn "\\host\share" or "/etc" into a
       legal-looking vault path), and Rust is the guard — plugins share the page
       realm, so the JS layer cannot be. ui/plugins/node-modules.js builds the
       `fs` / `fs/promises` facade on the same core (host.vfs, handed over before
       the seal). */
    const VROOT = "/vault";
    function vrel(p) {
      let s = String(p == null ? "" : p);
      if (s === VROOT || s === VROOT + "/") return "";
      if (s.startsWith(VROOT + "/")) s = s.slice(VROOT.length + 1);
      while (s.startsWith("./")) s = s.slice(2);
      return s === "." ? "" : s;
    }
    function vfsError(msg, path) {
      const s = String(msg && msg.message || msg), m = s.match(/^([A-Z]+): /);
      const e = new Error(s);
      e.code = m ? m[1] : "EIO"; e.path = String(path);
      if (/^EPERM: vault-escape refused/.test(s)) e.vaultEscape = true;
      return e;
    }
    const te = new TextEncoder(), td = new TextDecoder("utf-8");
    function toBytes(d) {
      if (d instanceof Uint8Array) return d;
      if (d instanceof ArrayBuffer) return new Uint8Array(d);
      if (ArrayBuffer.isView(d)) return new Uint8Array(d.buffer, d.byteOffset, d.byteLength);
      return te.encode(String(d == null ? "" : d));
    }
    function b64e(u8) {
      let s = "";
      for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
      return btoa(s);
    }
    function b64d(s) { const b = atob(String(s)), u = new Uint8Array(b.length); for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i); return u; }
    async function vio(op, path, a) {
      if (!be) throw vfsError("EPERM: plugin file access is not available", path);
      const args = Object.assign({ op, path: vrel(path) }, a || {});
      if (args.to != null) args.to = vrel(args.to);
      try { return await be.invoke("plugin_vfs", args); } catch (e) { throw vfsError(e, path); }
    }
    const core = Object.freeze({
      VROOT, vrel, vfsError,
      stat: p => vio("stat", p),
      exists: p => vio("exists", p),
      list: p => vio("list", p),
      read: async p => b64d(await vio("read", p)),
      write: (p, d) => vio("write", p, { data: b64e(toBytes(d)) }),
      append: (p, d) => vio("append", p, { data: b64e(toBytes(d)) }),
      mkdir: (p, recursive) => vio("mkdir", p, { recursive: !!recursive }),
      unlink: p => vio("unlink", p),
      rmdir: p => vio("rmdir", p),
      rename: (a, b) => vio("rename", a, { to: String(b) }),
      copy: (a, b) => vio("copy", a, { to: String(b) }),
      utf8: u8 => td.decode(u8), toBytes,
    });
    host.vfs = core;                 // for node-modules.js only; the seal hides host from plugins

    const joinRel = (dir, n) => (dir ? dir + "/" : "") + n;
    /* plug-val G2: a DataAdapter path is vault-relative in stock (getFullPath = basePath + "/" + p),
       so "/.git" names <vault>/.git (obsidian-git asks adapter.exists("/.git")). Only the adapter
       strips the leading "/": "/vault/..." keeps its meaning and the fs facade (core) stays verbatim,
       so require("fs") still has "/etc" refused by Rust. */
    const ap = p => { const s = String(p == null ? "" : p); return s === VROOT || s.startsWith(VROOT + "/") ? s : s.replace(/^\/+/, ""); };
    /* deep-templater ED-4: in stock a DataAdapter write to a new vault path puts the file in the vault model before the
       promise resolves and fires "create" (tp.app.vault.getAllLoadedFiles() lists EX/w1.txt right after
       `await adapter.write("EX/w1.txt", ...)`; templater startup templates write files that way). The watcher reports
       none of these writes. A note goes through changed() so the app's own tree and the metadata cache see it too;
       hidden paths (.obsidian/...) stay out of the model, as in stock. Existing files are left to their own events. */
    async function adapterMade(p, size, folder) {
      if (!K.st.filesOn) return;
      const r = vrel(ap(p)).replace(/\/+$/, "");
      if (!r || r.split("/").some(s => s.startsWith(".")) || files.has(r)) return;
      if (folder) { const f = ensureFolder(r); vault.trigger("create", f); return; }
      const n = noteOf(r);
      if (n != null) { await changed({ added: [n] }); return; }
      const [f, made] = ensureFile(r); f.stat.size = size || 0;
      if (made) vault.trigger("create", f);
    }
    /* PR #33 review: the other half of adapterMade. A file the adapter removes or renames leaves / moves in the vault
       model at once too (getFileByPath / getFiles no longer report a removed tmp.txt), with stock's "delete" /
       "rename"; a note goes through changed() so the app's tabs and tree follow. A path not in the model: nothing to
       drop, and a rename onto a visible path is a new file there (adapterMade). */
    const adapterRel = p => vrel(ap(p)).replace(/\/+$/, "");
    const notesIn = f => { const out = []; const go = x => { if (x instanceof TFolder) x.children.forEach(go); else { const n = noteOf(x.path); if (n != null) out.push(n); } }; go(f); return out; };
    async function adapterGone(p) {
      if (!K.st.filesOn) return;
      const r = adapterRel(p), f = r && files.get(r);
      if (!f) return;
      const notes = notesIn(f), all = [];
      const go = x => { all.push(x); if (x instanceof TFolder) x.children.forEach(go); }; go(f);
      if (f instanceof TFile && notes.length) { await changed({ removed: notes }); return; }
      removeTree(f);
      // PR #33 review: a note in the tree gets metadataCache "deleted" here, as applyChange gives a removed note (the
      // changed() below finds no model entry left to fire it for)
      for (const x of all.reverse()) { if (x instanceof TFile && noteOf(x.path) != null) metadataCache.trigger("deleted", x, null); vault.trigger("delete", x); }
      if (notes.length) await changed({ removed: notes });
    }
    async function adapterMoved(a, b) {
      if (!K.st.filesOn) return;
      const ra = adapterRel(a), rb = adapterRel(b), f = ra && files.get(ra);
      const hidden = !rb || rb.split("/").some(s => s.startsWith("."));
      const kindKept = f && (f instanceof TFolder || (noteOf(ra) != null) === (noteOf(rb) != null));
      if (!f || hidden || !kindKept) {   // out of the model, into a hidden path, or a note <-> non-note: a delete + a create
        if (f) await adapterGone(a);
        if (!hidden) { const st = await core.stat(ap(b)).catch(() => null); await adapterMade(b, st && st.size, st && st.type === "folder"); }
        return;
      }
      // PR #33 review: a rename onto a path in the model replaced that file on disk; it leaves the model (with its
      // "delete") first, so the moved entry is the only one there (getFiles reported both)
      const g = files.get(rb); if (g && g !== f) await adapterGone(b);
      if (f instanceof TFile && noteOf(ra) != null) { await changed({ renamed: [[noteOf(ra), noteOf(rb)]] }); return; }
      const before = notesIn(f), moved = [];
      const go = x => { moved.push([x, x.path]); if (x instanceof TFolder) x.children.forEach(go); }; go(f);
      movePath(f, rb);
      for (const [x, o] of moved) vault.trigger("rename", x, o);
      if (before.length) await changed({ renamed: before.map(o => [o, rb + o.slice(ra.length)]) });
    }
    class DataAdapter {
      getName() { return K.st.vaultName; }
      async exists(p) { return core.exists(ap(p)); }
      async stat(p) {
        try { return await core.stat(ap(p)); } catch (e) { if (e.code === "ENOENT") return null; throw e; }
      }
      async list(p) {
        p = ap(p);
        const d = vrel(p).replace(/\/+$/, ""), out = { files: [], folders: [] };
        for (const [n, k] of await core.list(p)) (k === "folder" ? out.folders : out.files).push(joinRel(d, n));
        return out;
      }
      async read(p) { return td.decode(await core.read(ap(p))); }
      async readBinary(p) { const u = await core.read(ap(p)); return u.buffer.slice(u.byteOffset, u.byteOffset + u.byteLength); }
      /* deep-git + deep-templater: a file the adapter writes is in the vault model at once, as in stock. obsidian-git writes
         its conflict list with adapter.write and opens it with openLinkText right after (before the watcher reports it);
         templater startup templates write files and list them via getAllLoadedFiles right after. adapterMade covers both:
         visible paths only, a file already in the model is left to the watcher, notes go through changed({added}). */
      async write(p, data) { const s = String(data); await core.write(ap(p), s); await adapterMade(p, te.encode(s).byteLength); }
      async writeBinary(p, data) { const u = toBytes(data); await core.write(ap(p), u); await adapterMade(p, u.byteLength); }
      async append(p, data) { const s = String(data); await core.append(ap(p), s); await adapterMade(p, te.encode(s).byteLength); }
      async appendBinary(p, data) { const u = toBytes(data); await core.append(ap(p), u); await adapterMade(p, u.byteLength); }
      async process(p, fn) { const out = fn(await this.read(p)); await this.write(p, out); return out; }
      getResourcePath() { return ""; }
      async mkdir(p) { await core.mkdir(ap(p), true); await adapterMade(p, 0, true); }
      async trashSystem() { return false; }       // no system trash: callers fall back to trashLocal
      async trashLocal(p) {
        p = ap(p);
        await core.mkdir(".trash", true);
        const base = vrel(p).split("/").pop();
        let to = ".trash/" + base;
        for (let i = 1; await core.exists(to); i++) to = ".trash/" + base + "." + i;
        await core.rename(p, to);
      }
      async rmdir(p, recursive) {
        p = ap(p);
        if (recursive) {
          for (const [n, k] of await core.list(p)) {
            const c = joinRel(vrel(p).replace(/\/+$/, ""), n);
            if (k === "folder") await this.rmdir(c, true); else await core.unlink(c);
          }
        }
        await core.rmdir(p);
      }
      async remove(p) { await core.unlink(ap(p)); await adapterGone(p); }
      async rename(a, b) { await core.rename(ap(a), ap(b)); await adapterMoved(a, b); }
      async copy(a, b) { await core.copy(ap(a), ap(b)); const st = await core.stat(ap(b)).catch(() => null); await adapterMade(b, st && st.size); }
    }
    const nodeErr = (Ctor, code, msg) => { const e = new Ctor(msg); e.code = code; return e; };
    const posixResolve = p => {
      const parts = [];
      for (const s of (p.startsWith("/") ? p : VROOT + "/" + p).split("/")) {
        if (!s || s === ".") continue;
        if (s === "..") parts.pop(); else parts.push(s);
      }
      return "/" + parts.join("/");
    };
    const URL_MOD = Object.freeze({
      URL, URLSearchParams,
      pathToFileURL(p) {
        if (typeof p !== "string") throw nodeErr(TypeError, "ERR_INVALID_ARG_TYPE", 'The "path" argument must be of type string. Received ' + (p === null ? "null" : typeof p));
        let r = posixResolve(p);
        if (p.endsWith("/") && r !== "/") r += "/";     // Node keeps a trailing slash
        const u = new URL("file://");
        u.pathname = r.replace(/%/g, "%25").replace(/\\/g, "%5C").replace(/\n/g, "%0A").replace(/\r/g, "%0D").replace(/\t/g, "%09").replace(/#/g, "%23").replace(/\?/g, "%3F");
        return u;
      },
      fileURLToPath(u) {
        let url = u;
        if (typeof u === "string") url = new URL(u);
        else if (!(u instanceof URL)) throw nodeErr(TypeError, "ERR_INVALID_ARG_TYPE", 'The "path" argument must be of type string or an instance of URL. Received ' + (u === null ? "null" : typeof u));
        if (url.protocol !== "file:") throw nodeErr(TypeError, "ERR_INVALID_URL_SCHEME", "The URL must be of scheme file");
        if (url.hostname !== "" && url.hostname !== "localhost") throw nodeErr(TypeError, "ERR_INVALID_FILE_URL_HOST", 'File URL host must be "localhost" or empty on linux');
        if (/%2f/i.test(url.pathname)) throw nodeErr(TypeError, "ERR_INVALID_FILE_URL_PATH", "File URL path must not include encoded / characters");
        return decodeURIComponent(url.pathname);
      },
    });
    host.vfsUrl = URL_MOD;           // node-desktop.js url.fileURLToPath / pathToFileURL (trusted view), before the seal
    class FileSystemAdapter extends DataAdapter {
      get basePath() { return VROOT; }
      getBasePath() { return VROOT; }
      getFullPath(p) { const r = vrel(ap(p)); return r ? VROOT + "/" + r : VROOT; }
      getFilePath(p) { return "file://" + this.getFullPath(p); }
      /* top5-func: stock's desktop adapter carries its fs module (templater reads adapter.fs). Here it
         is the same vault-confined facade require("fs") gives (ui/plugins/node-modules.js): paths are
         "/vault/x" = basePath + "/x", and anything outside the vault is refused by Rust (EPERM).
         A trusted vault only: in restricted mode the adapter has no fs at all. There is no
         adapter.fsPromises (and no realpath): the promise API is fs.promises, the same facade. */
      get fs() { if (!K.st.trusted) return undefined; const m = host.nodeModules && host.nodeModules.resolve("fs"); return m || undefined; }
      /* top5-func: stock's desktop adapter carries Node's url module (Excalidraw: VAULT_BASE_URL =
         url.pathToFileURL(basePath), and a dropped path is "inside the vault" when its file URL starts
         with that). Pure string conversion with Node's rules, over the same "/vault" root basePath
         answers, so the two always agree; it opens nothing and reads nothing. A relative path resolves
         against basePath (the plugin's view of the cwd). */
      get url() { return URL_MOD; }
      static async readLocalFile(p) { const u = await core.read(p); return u.buffer.slice(u.byteOffset, u.byteOffset + u.byteLength); }
      static async mkdir(p) { await core.mkdir(p, true); }
    }
    const adapter = new FileSystemAdapter();

    /* Vault: the typings' class; app.vault is its one instance. recurseChildren
       is the static walk calendar's daily-notes index uses. deep-dataview: stock's own walk, root first,
       then a stack that a folder's children are appended to in order and popped from the end, so the LAST
       child is visited first. getFiles / getMarkdownFiles are this walk in stock, and dataview indexes the
       vault in that order, so result rows that tie keep stock's order. */
    class Vault extends Events {
      static recurseChildren(root, cb) {
        let st = [root];
        while (st.length > 0) { const f = st.pop(); if (f) { cb(f); if (f instanceof TFolder) st = st.concat(f.children); } }
      }
    }
    const vault = new Vault();
    Object.assign(vault, {
      configDir: ".obsidian",
      getName() { return K.st.vaultName; },
      /* top5-func: templater's tp.file.exists. The adapter's check on the confined core: a vault path
         is answered true/false; a path that leaves the vault rejects (EPERM vault-escape), never false.
         `sensitive` is accepted for the typings' shape: the vault's filesystem is case-sensitive. */
      exists(p, sensitive) { return adapter.exists(p, sensitive); },
      getRoot() { return vroot; },
      getAbstractFileByPath(p) { const n = normalizePath(p); return n === "/" ? vroot : files.get(n) || null; },
      getFileByPath(p) { const f = vault.getAbstractFileByPath(p); return f instanceof TFile ? f : null; },
      getFolderByPath(p) { const f = vault.getAbstractFileByPath(p); return f instanceof TFolder ? f : null; },
      getFiles() { const a = []; Vault.recurseChildren(vroot, f => { if (f instanceof TFile) a.push(f); }); return a; },
      getMarkdownFiles() { const a = []; Vault.recurseChildren(vroot, f => { if (f instanceof TFile && f.extension === "md") a.push(f); }); return a; },
      getAllLoadedFiles() { return [vroot, ...files.values()]; },
      getAllFolders(includeRoot) { const a = [...files.values()].filter(f => f instanceof TFolder); return includeRoot ? [vroot, ...a] : a; },
      async read(f) {
        if (f instanceof TFile && isCanvas(f.path)) return cio("read", f.path);
        if (plainFile(f)) return td.decode(await adapter.readBinary(f.path));   // any other vault file: UTF-8 text over the confined VFS
        return inv("read_note", { name: mdOnly(f, "read") });
      },
      async cachedRead(f) { return vault.read(f); },
      async modify(f, data) {
        if (f instanceof TFile && isCanvas(f.path)) {
          const s = String(data); await cio("write", f.path, s);
          f.stat.mtime = Date.now(); f.stat.size = new TextEncoder().encode(s).byteLength; vault.trigger("modify", f); return;
        }
        if (plainFile(f)) return vault.modifyBinary(f, te.encode(String(data)));
        const name = mdOnly(f, "modify");
        await inv("write_note", { name, content: String(data) });
        await changed({ modified: [name] });
      },
      async append(f, data) { const s = await vault.read(f); return vault.modify(f, s + String(data)); },
      async process(f, fn) { const s = await vault.read(f); const out = fn(s); await vault.modify(f, out); return out; },
      async create(path, data) {
        const p = normalizePath(path), name = noteOf(p);
        if (isCanvas(p)) {
          if (files.has(p)) throw new Error("File already exists.");
          const s = String(data == null ? "" : data); await cio("create", p, s);
          const [f, made] = ensureFile(p); f.stat.size = new TextEncoder().encode(s).byteLength;
          if (made) vault.trigger("create", f);
          if (typeof refreshTree === "function") try { await refreshTree(); } catch (_) {}
          return f;
        }
        if (name == null) return vault.createBinary(p, te.encode(String(data == null ? "" : data)));   // other text files: confined VFS
        if (files.has(p)) throw new Error("File already exists.");
        await inv("create_note", { name, content: String(data == null ? "" : data) });
        await changed({ added: [name] });
        return files.get(p) || ensureFile(p)[0];
      },
      async createFolder(path) {
        const p = normalizePath(path);
        if (files.has(p)) throw new Error("Folder already exists.");
        await inv("create_dir", { name: p });
        const f = ensureFolder(p); vault.trigger("create", f);
        if (typeof refreshTree === "function") try { await refreshTree(); } catch (_) {}
        return f;
      },
      async delete(f) { const name = mdOnly(f, "delete"); await inv("delete_note", { name }); await changed({ removed: [name] }); },
      async trash(f) { return vault.delete(f); },    // delete_note already moves to <vault>/.trash
      async rename(f, newPath) {
        const old = mdOnly(f, "rename"), np = normalizePath(newPath), nn = noteOf(np);
        if (nn == null) throw new Error("rename: target must be a .md path (" + np + ")");
        await inv("move_note", { old, new: nn });
        await changed({ renamed: [[old, nn]] });
      },
      async copy(f, newPath) { const s = await vault.read(f); return vault.create(newPath, s); },
      /* binary I/O on any file: a markdown note goes through the note commands
         (UTF-8 text, so tabs/tree/watcher see one change), anything else through
         the confined VFS core, then the model and the Vault events. */
      async readBinary(f) {
        if (!(f instanceof TFile)) throw new Error("readBinary: not a file (" + (f && f.path) + ")");
        return adapter.readBinary(f.path);
      },
      async createBinary(path, data) {
        const p = normalizePath(path), u8 = toBytes(data);
        if (noteOf(p) != null) return vault.create(p, td.decode(u8));
        if (files.has(p) || await core.exists(p)) throw new Error("File already exists.");
        await core.write(p, u8);
        const [f, made] = ensureFile(p); f.stat.size = u8.byteLength;
        if (made) vault.trigger("create", f);
        return f;
      },
      async modifyBinary(f, data) {
        if (!(f instanceof TFile)) throw new Error("modifyBinary: not a file (" + (f && f.path) + ")");
        const u8 = toBytes(data);
        if (noteOf(f.path) != null) return vault.modify(f, td.decode(u8));
        await core.write(f.path, u8);
        f.stat.mtime = Date.now(); f.stat.size = u8.byteLength; vault.trigger("modify", f);
      },
      async appendBinary(f, data) {
        if (!(f instanceof TFile)) throw new Error("appendBinary: not a file (" + (f && f.path) + ")");
        const u8 = toBytes(data);
        if (noteOf(f.path) != null) return vault.append(f, td.decode(u8));
        await core.append(f.path, u8);
        f.stat.mtime = Date.now(); f.stat.size += u8.byteLength; vault.trigger("modify", f);
      },
      getResourcePath() { return ""; },
      getConfig(k) { return ({ useMarkdownLinks: false, newLinkFormat: "shortest", attachmentFolderPath: "/", newFileLocation: "root", newFileFolderPath: "", alwaysUpdateLinks: false, showUnsupportedFiles: false })[k] ?? null; },
      /* vault.config (undocumented, part vint): the vault's own .obsidian/app.json
         as read when the model is built — {} when there is none; no defaults are
         merged in (getConfig answers those). One object for the vault's life. */
      config: {},
      /* lookups the stock app makes WITHOUT normalizePath (oracle part vint):
         "/x", "a//b" and "dir/" find nothing; a non-string throws TypeError. */
      getAbstractFileByPathInsensitive(p) {
        const lc = p.toLowerCase();
        if (p === "/") return vroot;
        const exact = files.get(p); if (exact) return exact;
        for (const [k, f] of files) if (k.toLowerCase() === lc) return f;
        return null;
      },
      // "base.ext", else "base 1.ext", "base 2.ext", ...: the first name no file or folder holds, in any case
      getAvailablePath(base, ext) {
        const b = String(base), e = ext ? "." + ext : "";
        let p = b + e;
        for (let i = 1; vault.getAbstractFileByPathInsensitive(p); i++) p = b + " " + i + e;
        return p;
      },
      /* top10-func: the older attachment-path call (Kanban pastes / drops an image into a card):
         basename + extension (no dot) for the note sourceFile, by the same rule as
         fileManager.getAvailablePathForAttachment (attachment folder setting, "name 1.ext" on a
         collision, the folder made if missing). A non-string basename is a TypeError. */
      getAvailablePathForAttachments(basename, extension, sourceFile) {
        if (typeof basename !== "string" || !basename) return Promise.reject(new TypeError("getAvailablePathForAttachments: basename must be a non-empty string"));
        const ext = extension == null || extension === "" ? "" : "." + String(extension).replace(/^\./, "");
        const src = sourceFile && typeof sourceFile === "object" && typeof sourceFile.path === "string" ? sourceFile.path : sourceFile == null ? null : String(sourceFile);
        return fileManager.getAvailablePathForAttachment(basename + ext, src);
      },
      adapter,
    });

    /* ------------------------------------- metadata cache (item 5)
       getFileCache is SYNCHRONOUS in Obsidian, so the cache is filled ahead:
       a sweep over every note when the model is built (yielding between
       batches, then 'resolved'), and one re-parse per watcher/API change
       ('changed'). A miss returns null and queues that file. Parsed in JS from
       the note text: frontmatter (YAML subset), headings, [[links]], ![[embeds]],
       [md](links), #tags — all with Obsidian-shaped positions. */
    // path -> CachedMetadata; cacheVer counts every change, so derived indexes
    // (metadataTypeManager.properties) rebuild lazily instead of on each event
    let cacheVer = 0;
    const caches = new (class extends Map {
      set(k, v) { cacheVer++; return super.set(k, v); }
      delete(k) { cacheVer++; return super.delete(k); }
    })();
    let sweepDone = false;
    function parseMeta(src) {
      const lines = String(src).split("\n"), out = {};
      const pos = (line, col, offset) => ({ line, col, offset });
      let off = 0, i = 0;
      const lineEnd = n => n.replace(/\r$/, "");
      if (lines.length > 1 && lineEnd(lines[0]) === "---") {
        let j = 1, o = lines[0].length + 1;
        for (; j < lines.length; j++) { if (/^(---|\.\.\.)\s*$/.test(lineEnd(lines[j]))) break; o += lines[j].length + 1; }
        if (j < lines.length) {
          let fm; try { fm = parseYaml(lines.slice(1, j).join("\n")); } catch (_) { fm = undefined; }   // invalid or empty YAML: no frontmatter
          if (fm && typeof fm === "object" && !Array.isArray(fm)) {
            out.frontmatter = fm;
            // an *alias is the SAME object as its anchor, so the frontmatter can be a shared-node graph: each object is walked
            // once (its links are reported under the first key that reaches it), else doubled aliases (aN: [*a(N-1), *a(N-1)])
            // made the walk exponential in the line count
            const fl = [], seen = new Set(), walk = (v, k) => {   // properties whose whole value is a [[wikilink]]
              if (typeof v === "string") {
                const m = v.match(/^\[\[([^\[\]]+)\]\]$/);
                if (m) { const b = m[1].indexOf("|"), link = b < 0 ? m[1] : m[1].slice(0, b); fl.push({ key: k, link, original: v, displayText: b < 0 ? link.split("#").filter(Boolean).join(" > ") : m[1].slice(b + 1) }); }
              } else if (!v || typeof v !== "object" || seen.has(v)) return;
              else if (seen.add(v), Array.isArray(v)) v.forEach((x, n) => walk(x, k + "." + n));
              else for (const s of Object.keys(v)) walk(v[s], k + "." + s);
            };
            for (const k of Object.keys(fm)) walk(fm[k], k);
            if (fl.length) out.frontmatterLinks = fl;
          }
          out.frontmatterPosition = { start: pos(0, 0, 0), end: pos(j, 3, o + 3) };
          out.sections = [{ type: "yaml", position: out.frontmatterPosition }];
          off = o + lines[j].length + 1; i = j + 1;
        }
      }
      let fence = null;
      const bodyLine = i, bodyOff = off;
      const push = (k, v) => { (out[k] || (out[k] = [])).push(v); };
      for (; i < lines.length; off += lines[i].length + 1, i++) {
        const raw = lineEnd(lines[i]);
        const fm = raw.match(/^\s*(`{3,}|~{3,})/);
        if (fm) { if (!fence) fence = fm[1][0]; else if (fm[1][0] === fence) fence = null; continue; }
        if (fence) continue;
        const P = (a, b) => ({ start: pos(i, a, off + a), end: pos(i, b, off + b) });
        const h = raw.match(/^(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/);
        if (h) push("headings", { heading: h[2], level: h[1].length, position: P(0, raw.length) });
        const s = raw.replace(/`[^`]*`/g, m => " ".repeat(m.length));
        let m; const wl = /(!?)\[\[([^\]\n]+?)\]\]/g;
        while ((m = wl.exec(s))) {
          const inner = m[2], bar = inner.indexOf("|"), link = (bar < 0 ? inner : inner.slice(0, bar)).trim();
          push(m[1] ? "embeds" : "links", { link, original: m[0], displayText: bar < 0 ? link : inner.slice(bar + 1), position: P(m.index, m.index + m[0].length) });
        }
        const ml = /(!?)\[([^\]\n]*)\]\(([^)\s]+)\)/g;
        while ((m = ml.exec(s))) {
          if (/^[a-z][a-z0-9+.-]*:/i.test(m[3])) continue;   // external URL: not a vault link
          let link; try { link = decodeURI(m[3]); } catch (_) { link = m[3]; }
          push(m[1] ? "embeds" : "links", { link, original: m[0], displayText: m[2], position: P(m.index, m.index + m[0].length) });
        }
        {   // a heading line's tags count too ("## title #tag": stock getTags, oracle part vint)
          const tg = /(^|\s)#([\p{L}\p{N}_/-]*[\p{L}_/-][\p{L}\p{N}_/-]*)/gu;
          while ((m = tg.exec(s))) { const a = m.index + m[1].length; push("tags", { tag: "#" + m[2], position: P(a, a + 1 + m[2].length) }); }
        }
      }
      parseBlocks(lines, bodyLine, bodyOff, out, pos);
      return out;
    }
    /* block pass over the note body: top-level `sections` (heading, paragraph,
       list, code, blockquote/callout, thematicBreak, table, math, comment) and
       `listItems` — one per list item, its position spanning its own lines
       (not its children's), starting where its container's content starts
       (col 0 at top level, the parent's content column when nested);
       `parent` = the parent item's line, or -(first line of the list) for a
       root item; `task` = the char between the brackets of "[ ]", "[x]", ...
       Shapes as stock 1.13.7 (oracle part cblock). Lists inside a quote are
       indexed too, their root items starting after the "> " prefix. */
    function parseBlocks(lines, i0, off0, out, pos) {
      const sections = out.sections || [], items = [];
      const vw = s => { let w = 0; for (const ch of s) w = ch === "\t" ? w + 4 - (w % 4) : w + 1; return w; };
      const idxAtW = (s, W) => { let w = 0, k = 0; while (k < s.length && w < W && (s[k] === " " || s[k] === "\t")) { w = s[k] === "\t" ? w + 4 - (w % 4) : w + 1; k++; } return k; };
      const ITEM = /^([ \t]*)([-*+]|\d{1,9}[.)])([ \t]+|$)/, HR = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
      const FENCE = /^[ \t]*(`{3,}|~{3,})/, HEAD = /^#{1,6}\s/, QUOTE = /^ {0,3}>/;
      const TDELIM = /^[ \t]*\|?[ \t]*:?-+:?[ \t]*(\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/;
      const blockStart = s => HEAD.test(s) || QUOTE.test(s) || FENCE.test(s) || HR.test(s);
      const L = () => ({ stack: [], start: -1, kind: null, blank: false });
      const span = (i, a, o, len) => ({ start: pos(i, a, o + a), end: pos(i, len, o + len) });
      // a list item at line i (s = text after the container prefix of b chars) -> 0 none, 1 same list, 2 new list
      function item(st, s, b, i, o, len) {
        if (HR.test(s)) return 0;
        const m = s.match(ITEM); if (!m) return 0;
        const w = vw(m[1]);
        while (st.stack.length && w < st.stack[st.stack.length - 1].cw) st.stack.pop();
        const kind = /\d/.test(m[2]) ? "o" + m[2].slice(-1) : m[2];
        let parent, r = 1, sc = b;
        if (st.stack.length) { const top = st.stack[st.stack.length - 1]; parent = top.line; sc = b + idxAtW(s, top.cw); }
        else { if (st.start < 0 || st.kind !== kind) { st.start = i; st.kind = kind; r = 2; } parent = -st.start; }
        let sw = m[3] ? vw(m[1] + m[2] + m[3]) - vw(m[1] + m[2]) : 1; if (sw > 4) sw = 1;
        const it = { parent, position: span(i, sc, o, len) };
        const t = m[3] && s.slice(m[0].length).match(/^\[(.)\](?=[ \t]|$)/u);
        if (t) it.task = t[1];
        items.push(it); st.stack.push({ cw: vw(m[1] + m[2]) + sw, line: i, it }); st.blank = false;
        return r;
      }
      // a non-item, non-blank line inside a list: continuation of an open item?
      function cont(st, s, i, o, len) {
        if (!st.stack.length) return false;
        const w = vw(s.match(/^[ \t]*/)[0]);
        if (st.blank) { while (st.stack.length && w < st.stack[st.stack.length - 1].cw) st.stack.pop(); if (!st.stack.length) return false; }
        else if (w < st.stack[st.stack.length - 1].cw && blockStart(s)) return false;   // lazy continuation only for paragraph text
        st.stack[st.stack.length - 1].it.position.end = pos(i, len, o + len); st.blank = false;
        return true;
      }
      let blk = null, tl = L(), ql = L(), delim = null;
      const close = () => {
        if (blk) sections.push({ type: blk.type, position: { start: pos(blk.sl, 0, blk.so), end: pos(blk.el, blk.ec, blk.eo) } });
        blk = null;
      };
      const open = (type, i, o) => { close(); blk = { type, sl: i, so: o }; };
      const ext = (i, o, len) => { blk.el = i; blk.ec = len; blk.eo = o + len; };
      const quoteLine = (raw, i, o, len) => {
        const b = raw.match(/^(?: {0,3}>[ ]?)+/)[0].length, inner = raw.slice(b);
        if (!inner.trim()) ql.blank = true;
        else if (!item(ql, inner, b, i, o, len) && !cont(ql, inner, i, o, len)) ql = L();
      };
      for (let i = i0, off = off0; i < lines.length; off += lines[i].length + 1, i++) {
        const raw = lines[i].replace(/\r$/, ""), len = raw.length, trim = raw.trim();
        if (delim) {   // inside a fence / $$ / %% block (top level, or inside a list item)
          if (blk) ext(i, off, len);
          const shut = delim.re ? delim.re.test(raw) : trim.endsWith(delim.s) && (trim !== delim.s || !delim.first);
          delim.first = false;
          if (shut) { const own = delim.own; delim = null; if (own) close(); }
          continue;
        }
        if (!trim) {
          if (blk && blk.type === "list") tl.blank = true; else close();
          continue;
        }
        if (blk && blk.type === "list") {
          const r = item(tl, raw, 0, i, off, len);
          if (r === 2) { close(); open("list", i, off); ext(i, off, len); continue; }
          if (r === 1) { ext(i, off, len); continue; }
          if (cont(tl, raw, i, off, len)) {
            ext(i, off, len);
            const fm = raw.match(FENCE);
            if (fm) delim = { re: new RegExp("^[ \\t]*" + fm[1][0].replace("`", "\\`") + "{" + fm[1].length + ",}[ \\t]*$"), own: false };
            continue;
          }
          close(); tl = L();
        }
        if (blk && /^(blockquote|callout)$/.test(blk.type)) {
          if (QUOTE.test(raw)) { quoteLine(raw, i, off, len); ext(i, off, len); continue; }
          if (!blockStart(raw) && !ITEM.test(raw)) { ext(i, off, len); continue; }   // lazy continuation
          close();
        }
        const fm = raw.match(FENCE);
        if (fm) {
          open("code", i, off); ext(i, off, len);
          delim = { re: new RegExp("^[ \\t]*" + fm[1][0].replace("`", "\\`") + "{" + fm[1].length + ",}[ \\t]*$"), own: true };
          continue;
        }
        const dm = trim.startsWith("$$") ? "$$" : trim.startsWith("%%") ? "%%" : null;
        if (dm) {
          open(dm === "$$" ? "math" : "comment", i, off); ext(i, off, len);
          if (trim.length > 4 && trim.endsWith(dm)) close();   // one-line $$x$$ / %%x%%
          else delim = { s: dm, own: true, first: true };
          continue;
        }
        if (HEAD.test(raw)) { open("heading", i, off); ext(i, off, len); close(); continue; }
        if (HR.test(raw)) { open("thematicBreak", i, off); ext(i, off, len); close(); continue; }
        if (QUOTE.test(raw)) {
          open(/^ {0,3}>\s*\[!/.test(raw) ? "callout" : "blockquote", i, off); ql = L();
          quoteLine(raw, i, off, len); ext(i, off, len); continue;
        }
        if (ITEM.test(raw)) { open("list", i, off); tl = L(); item(tl, raw, 0, i, off, len); ext(i, off, len); continue; }
        if (blk && (blk.type === "paragraph" || blk.type === "table")) {
          if (blk.type === "paragraph" && blk.sl === i - 1 && lines[i - 1].includes("|") && raw.includes("-") && TDELIM.test(raw)) blk.type = "table";
          ext(i, off, len); continue;
        }
        open("paragraph", i, off); ext(i, off, len);
      }
      close();
      {   // block ids: a list item, or a paragraph / quote / table / math section, whose last line ends in " ^id" ->
          // blocks[id] = { id, position } (lower-cased, the item / section span); resolveSubpath "#^id" reads it
        const bl = {}, idAt = ln => { const m = String(lines[ln] == null ? "" : lines[ln]).replace(/\r$/, "").match(/(?:^|\s)\^([A-Za-z0-9-]+)[ \t]*$/); return m ? m[1].toLowerCase() : null; };
        for (const it of items) { const id = idAt(it.position.end.line); if (id && !bl[id]) bl[id] = { id, position: it.position }; }
        for (const s of sections) {
          if (!/^(paragraph|blockquote|callout|table|math)$/.test(s.type)) continue;
          const id = idAt(s.position.end.line); if (id && !bl[id]) bl[id] = { id, position: s.position };
        }
        if (Object.keys(bl).length) out.blocks = bl;
      }
      if (sections.length) out.sections = sections;
      if (items.length) out.listItems = items;
    }
    /* link resolution: exact path, path+".md", then the shortest note whose path
       ends with it (case-insensitive) — the app resolves [[x]] by full path OR
       basename (index.rs resolve), and a shortest-path tie-break matches
       Obsidian's "shortest" link format for unique names. */
    function resolveLink(lp, src) {
      let p = getLinkpath(String(lp).split("|")[0]).trim();
      if (!p) return src ? vault.getFileByPath(src) : null;
      p = normalizePath(p);
      for (const c of [p, p + ".md"]) { const f = files.get(c); if (f instanceof TFile) return f; }
      const lc = p.toLowerCase(), lmd = lc + ".md"; let best = null;
      for (const f of files.values()) {
        if (!(f instanceof TFile)) continue;
        const fp = f.path.toLowerCase();
        if (fp === lc || fp === lmd || fp.endsWith("/" + lc) || fp.endsWith("/" + lmd)) if (!best || f.path.length < best.path.length) best = f;
      }
      return best;
    }
    const idxQ = new Set(); let idxT = 0;
    function indexFile(f) {
      if (!K.st.modelOn || !(f instanceof TFile) || f.extension !== "md") return;
      idxQ.add(f); if (!idxT) idxT = setTimeout(drainIdx, 0);
    }
    async function drainIdx() {
      const batch = [...idxQ]; idxQ.clear();
      for (const f of batch) {
        if (files.get(f.path) !== f) continue;
        let txt; try { txt = await inv("read_note", { name: noteOf(f.path) }); } catch (_) { continue; }
        const c = parseMeta(txt); caches.set(f.path, c); f.stat.size = new TextEncoder().encode(txt).byteLength;
        metadataCache.trigger("changed", f, txt, c);
        metadataCache.trigger("resolve", f);   // its links resolve on demand (resolveLink), so they are resolved now: dataview re-indexes a file on "resolve"
      }
      idxT = 0;
      if (idxQ.size) idxT = setTimeout(drainIdx, 0);
      else if (sweepDone) metadataCache.trigger("resolved");
    }
    async function sweep() {
      const all = vault.getMarkdownFiles();
      for (let k = 0; k < all.length; k += 16) {
        await Promise.all(all.slice(k, k + 16).map(async f => {
          if (caches.has(f.path)) return;
          try { const t = await inv("read_note", { name: noteOf(f.path) }); caches.set(f.path, parseMeta(t)); f.stat.size = new TextEncoder().encode(t).byteLength; } catch (_) {}
        }));
        await new Promise(r => setTimeout(r, 0));
      }
      sweepDone = true; metadataCache.resolved = true;
      metadataCache.trigger("resolved");
      touch();
    }
    function linkMap(resolved) {
      const o = {};
      for (const [p, c] of caches) {
        const m = {};
        for (const l of (c.links || []).concat(c.embeds || [])) {
          const f = resolveLink(l.link, p), k = f ? f.path : getLinkpath(l.link);
          if (!!f === resolved) m[k] = (m[k] || 0) + 1;
        }
        o[p] = m;
      }
      return o;
    }
    /* MetadataCache: the typings' class; app.metadataCache is its one instance.
       The methods live on the prototype (a plugin may patch them there) and
       read the closure state above; `new MetadataCache()` elsewhere yields an
       empty cache that knows no file. */
    let theCache = null;
    const mine = mc => mc === theCache;
    /* frontmatter "tags" (key case-insensitive; not "tag"): a list's items, or a
       string as ONE item; a leading "#" dropped; an item that is not a valid tag
       (space, comma, all digits, empty) is not a tag (stock getTags, part vint) */
    function fmTags(fm) {
      if (!fm || typeof fm !== "object") return [];
      const k = Object.keys(fm).find(x => x.toLowerCase() === "tags");
      if (k === undefined || fm[k] == null) return [];
      const out = [];
      for (const v of [].concat(fm[k])) {
        if (typeof v !== "string" && typeof v !== "number") continue;
        const t = String(v).replace(/^#/, "");
        if (/^[\p{L}\p{N}_/-]*[\p{L}_/-][\p{L}\p{N}_/-]*$/u.test(t)) out.push(t);
      }
      return out;
    }
    class MetadataCache extends Events {
      constructor() { super(); this.resolved = false; }
      getFileCache(f) { if (!f || !mine(this)) return null; const c = caches.get(f.path); if (!c) indexFile(f); return c || null; }
      getCache(p) { const f = vault.getFileByPath(p); return f ? this.getFileCache(f) : null; }
      /* the vault paths that have a cache entry, as a fresh array */
      getCachedFiles() { return mine(this) ? [...caches.keys()] : []; }
      getFirstLinkpathDest(lp, src) { return mine(this) ? resolveLink(lp, src) : null; }
      getLinkpathDest(lp, src) { const f = this.getFirstLinkpathDest(lp, src); return f ? [f] : []; }
      fileToLinktext(f, src, omitMd = true) {
        const same = vault.getMarkdownFiles().filter(x => x.basename === f.basename).length;
        const p = same > 1 || f.extension !== "md" ? f.path : f.name;
        return omitMd && f.extension === "md" ? p.replace(/\.md$/, "") : p;
      }
      get resolvedLinks() { return mine(this) ? linkMap(true) : {}; }
      get unresolvedLinks() { return mine(this) ? linkMap(false) : {}; }
      getBacklinksForFile(f) { const d = new Map(); if (mine(this)) for (const [p, c] of caches) { const ls = (c.links || []).filter(l => resolveLink(l.link, p) === f); if (ls.length) d.set(p, ls); } return { data: d, keys: () => [...d.keys()], get: k => d.get(k) }; }
      /* getTags (undocumented, part vint): a fresh {"#tag": count} over the whole
         cache — every body occurrence plus the frontmatter "tags" items, each nested
         tag also counting its parents ("#a/b" -> "#a"); merged case-insensitively
         under the first spelling seen (frontmatter before body within a note). */
      getTags() {
        const out = {}, keyOf = new Map();
        if (!mine(this)) return out;
        const add = t => {
          const segs = t.split("/");
          for (let i = 1; i <= segs.length; i++) {
            if (!segs[i - 1]) break;
            const k = "#" + segs.slice(0, i).join("/"), lc = k.toLowerCase();
            let key = keyOf.get(lc);
            if (key === undefined) { key = k; keyOf.set(lc, k); out[k] = 0; }
            out[key]++;
          }
        };
        for (const c of caches.values()) {
          for (const t of fmTags(c.frontmatter)) add(t);
          for (const t of c.tags || []) add(String(t.tag).slice(1));
        }
        return out;
      }
    }
    /* top5-func: metadataCache.blockCache (undocumented; Excalidraw's heading/block link suggesters,
       its section reader for transclusions and its back-of-note writer). getForFile(ctx, file) parses
       the note's CURRENT text (read now, not the sweep cache, which a just-saved file can be ahead of)
       into one entry per top-level body section: { display: the section's source text, node: { type,
       position, depth (heading), children: [the heading's title as a text node with its own position]
       (heading), id (a block that ends in " ^id") } }. Section types are getFileCache's own (heading,
       paragraph, list, code, blockquote, callout, comment, math, table, thematicBreak); frontmatter is
       not a block. ctx.isCancelled() true before or after the read -> resolves null. A non-markdown
       file has no blocks; something that is not a vault file rejects with a TypeError. */
    class BlockCache {
      async getForFile(ctx, file) {
        if (!(file instanceof TFile) || files.get(file.path) !== file) throw new TypeError("blockCache.getForFile: not a vault file (" + (file && file.path) + ")");
        const cancelled = () => !!(ctx && typeof ctx.isCancelled === "function" && ctx.isCancelled());
        if (cancelled()) return null;
        if (file.extension !== "md") return { blocks: [] };
        const text = await vault.cachedRead(file);
        if (cancelled()) return null;
        const meta = parseMeta(text), blocks = [];
        const at = (line, col, offset) => ({ line, col, offset });
        for (const s of meta.sections || []) {
          if (s.type === "yaml") continue;
          const p = s.position, src = text.slice(p.start.offset, p.end.offset);
          const node = { type: s.type, position: { start: at(p.start.line, p.start.col, p.start.offset), end: at(p.end.line, p.end.col, p.end.offset) } };
          if (s.type === "heading") {
            const m = src.match(/^(#{1,6})([ \t]+)(.*?)(?:[ \t]+#+)?[ \t]*$/);
            node.depth = m ? m[1].length : 1;
            if (m && m[3]) {
              const c = m[1].length + m[2].length, o = p.start.offset + c, l = p.start.line;
              node.children = [{ type: "text", value: m[3], position: { start: at(l, c, o), end: at(l, c + m[3].length, o + m[3].length) } }];
            }
          } else if (s.type !== "code" && s.type !== "comment" && s.type !== "math") {
            const id = src.match(/(?:^|[ \t])\^([A-Za-z0-9-]+)[ \t]*$/);
            if (id) node.id = id[1];
          }
          blocks.push({ display: src, node });
        }
        return { blocks };
      }
    }
    const metadataCache = theCache = new MetadataCache();
    metadataCache.blockCache = new BlockCache();
    K.exportClasses.MetadataCache = MetadataCache;

    /* ------------------------------------- property types (part vint)
       app.metadataTypeManager, undocumented, from the stock oracle (black box,
       scripts/api-probes.d/vint.js): the vault's frontmatter properties
       (lowercase key -> {name: first spelling seen, occurrences: notes holding it,
       widget}), the types the user assigned (.obsidian/types.json) and the
       registered type widgets. A widget's render() is the property editor UI,
       which opensidian does not have: it throws 'unsupported in opensidian'. */
    const TYPES_WHY = "the property editor widgets are not part of opensidian";
    const isDateStr = s => {
      const m = window.moment;
      if (m && m.ISO_8601) return m(s, m.ISO_8601, true).isValid();
      return /^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}(:\d{2}(\.\d+)?)?)?$/.test(s) && !isNaN(Date.parse(s));
    };
    const vDate = v => v == null || v === "" || typeof v === "number" || (typeof v === "string" && isDateStr(v));
    const vStr = v => typeof v === "string", vList = v => typeof v === "string" || Array.isArray(v);
    const widget = (type, icon, label, validate, extra) => Object.assign({
      type, icon, name: () => label, validate,
      render() { throw unsupported(TYPES_WHY); },
    }, extra);
    const UNKNOWN_W = widget("unknown", "lucide-file-question", "Unknown", () => false);
    function inferType(v) {
      if (typeof v === "number") return "number";
      if (typeof v === "boolean") return "checkbox";
      if (typeof v === "string") {
        if (/^\d{4}-\d{2}-\d{2}$/.test(v) && isDateStr(v)) return "date";
        if (/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}(:\d{2})?$/.test(v) && isDateStr(v)) return "datetime";
        return "text";
      }
      if (v == null) return "text";
      if (Array.isArray(v)) return v.every(x => typeof x === "string") ? "multitext" : "unknown";
      return "unknown";
    }
    let typesSaveT = 0;
    const DEFAULT_TYPES = [["aliases", "aliases"], ["cssclasses", "multitext"], ["tags", "tags"]];
    class MetadataTypeManager extends Events {
      constructor() {
        super();
        this.app = null; this._loaded = false; this.lastSave = 0;
        this.properties = {}; this.assignedWidgets = {};
        this.registeredTypeWidgets = {};
        for (const w of [
          widget("aliases", "lucide-forward", "Aliases", vList, { reservedKeys: ["aliases"] }),
          widget("checkbox", "lucide-check-square", "Checkbox", v => typeof v === "boolean"),
          widget("date", "lucide-calendar", "Date", vDate),
          widget("datetime", "lucide-clock", "Date & time", vDate),
          widget("file", "lucide-file", "File", vStr, { reservedKeys: [] }),
          widget("folder", "lucide-folder", "Folder", vStr, { reservedKeys: [] }),
          widget("multitext", "lucide-list", "List", vList),
          widget("number", "lucide-binary", "Number", v => typeof v === "number"),
          widget("property", "lucide-info", "Property", vStr, { reservedKeys: [] }),
          widget("tags", "lucide-tags", "Tags", vList, { reservedKeys: ["tags"] }),
          widget("text", "lucide-text", "Text", vStr),
        ]) this.registeredTypeWidgets[w.type] = w;
        Object.defineProperty(this, "_ver", { value: { built: "", assigned: 0 }, writable: true });
        this.setAssigned(DEFAULT_TYPES);
      }
      setAssigned(pairs) {
        for (const k of Object.keys(this.assignedWidgets)) delete this.assignedWidgets[k];
        for (const [name, w] of pairs) this.assignedWidgets[String(name).toLowerCase()] = { name: String(name), widget: w };
        this._ver.assigned++;
      }
      async load() {
        let pairs = DEFAULT_TYPES;
        try {
          const j = JSON.parse(await adapter.read(vault.configDir + "/types.json"));
          if (j && j.types && typeof j.types === "object") pairs = Object.entries(j.types);
        } catch (_) {}
        this.setAssigned(pairs); this._loaded = true;
      }
      save() {
        clearTimeout(typesSaveT);
        typesSaveT = setTimeout(() => {
          const types = {};
          for (const a of Object.values(this.assignedWidgets)) types[a.name] = a.widget;
          this.lastSave = Date.now();
          adapter.write(vault.configDir + "/types.json", JSON.stringify({ types }, null, 2))
            .then(() => { this._ver.assigned++; })
            .catch(e => console.error("[plugin] metadataTypeManager.save:", e));
        }, 100);
      }
      updatePropertyInfoCache() {
        const stamp = cacheVer + ":" + this._ver.assigned;
        if (this._ver.built === stamp) return;
        this._ver.built = stamp;
        const P = this.properties;
        for (const k of Object.keys(P)) delete P[k];
        if (this === theTypes) for (const c of caches.values()) {
          const fm = c.frontmatter;
          if (!fm || typeof fm !== "object") continue;
          for (const k of Object.keys(fm)) {
            const lc = k.toLowerCase();
            if (!P[lc]) P[lc] = { name: k, occurrences: 0, widget: inferType(fm[k]) };
            P[lc].occurrences++;
          }
        }
        for (const [lc, a] of Object.entries(this.assignedWidgets)) {
          if (a.widget == null) continue;
          if (P[lc]) P[lc].widget = a.widget; else P[lc] = { name: a.name, occurrences: 0, widget: a.widget };
        }
      }
      getAllProperties() { this.updatePropertyInfoCache(); return this.properties; }
      getPropertyInfo(name) {
        this.updatePropertyInfoCache();
        return this.properties[String(name).toLowerCase()] || { name, occurrences: 0, widget: "text" };
      }
      getAssignedWidget(name) { const a = this.assignedWidgets[String(name).toLowerCase()]; return a && a.widget != null ? a.widget : null; }
      getWidget(type) { return Object.prototype.hasOwnProperty.call(this.registeredTypeWidgets, type) ? this.registeredTypeWidgets[type] : UNKNOWN_W; }
      /* {expected, inferred}: expected = the assigned type's widget, else the one the
         value (or, without a value, the property's recorded widget) suggests; with an
         assigned type, inferred stays it while the value validates, else follows the value */
      getTypeInfo(name, value) {
        const as = this.getAssignedWidget(name);
        if (value === undefined) {
          let t = as;
          if (t == null) { const w = this.getPropertyInfo(name).widget; t = Object.prototype.hasOwnProperty.call(this.registeredTypeWidgets, w) ? w : "text"; }
          const w = this.getWidget(t);
          return { expected: w, inferred: w };
        }
        const inf = this.getWidget(inferType(value));
        if (as == null) return { expected: inf, inferred: inf };
        const exp = this.getWidget(as);
        return { expected: exp, inferred: exp.validate(value) ? exp : inf };
      }
      /* setType: the assignment is live at once — an entry already in properties takes
         the new widget, a registered type also lists a name no note uses yet — and the
         whole index is rebuilt once types.json is written (stock: an unregistered type
         shows up in getAllProperties only then). "changed" carries the lowercase name;
         re-assigning updates the existing entry in place (same object, first spelling kept). A properties entry is replaced, not mutated: an info object read earlier keeps its widget. */
      setType(name, type) {
        const lc = String(name).toLowerCase(), had = this.assignedWidgets[lc];
        this.updatePropertyInfoCache();
        if (had) had.widget = type; else this.assignedWidgets[lc] = { name: String(name), widget: type };
        const P = this.properties;
        if (P[lc]) { if (type != null) P[lc] = { name: P[lc].name, occurrences: P[lc].occurrences, widget: type }; }
        else if (type != null && Object.prototype.hasOwnProperty.call(this.registeredTypeWidgets, type)) P[lc] = { name: String(name), occurrences: 0, widget: type };
        this.trigger("changed", lc);
        this.save();
      }
      unsetType(name) {
        const lc = String(name).toLowerCase();
        if (!(lc in this.assignedWidgets)) return;
        delete this.assignedWidgets[lc];
        this._ver.assigned++;
        this.trigger("changed", lc);
        this.save();
      }
    }
    const metadataTypeManager = new MetadataTypeManager();
    const theTypes = metadataTypeManager;
    async function loadVaultConfig() {
      const c = vault.config;
      for (const k of Object.keys(c)) delete c[k];
      try {
        const j = JSON.parse(await adapter.read(vault.configDir + "/app.json"));
        if (j && typeof j === "object" && !Array.isArray(j)) Object.assign(c, j);
      } catch (_) {}
    }
    /* TExternalFile (typings 1.14.4): a file outside the vault, by its real
       host path. The plugin VFS is confined to one virtual root and never
       names a host path (vaultfs.rs), so this is design-gated: exported,
       and both construction and getRealPath throw. */
    const unsupported = why => new Error("unsupported in opensidian: " + why);
    const EXT_WHY = "TExternalFile would expose a host path outside the confined vault";
    class TExternalFile extends TFile {
      constructor() { super(); throw unsupported(EXT_WHY); }
      getRealPath() { throw unsupported(EXT_WHY); }
    }
    K.exportClasses.TExternalFile = TExternalFile;

    /* ------------------------------------- FileManager (api-parity meta)
       Written from the typings and the stock behaviour oracle (black box,
       scripts/api-probes.d/meta.js): links follow the vault's link settings
       (getConfig), attachment paths dedupe as "name 1.ext", "name 2.ext", ...,
       processFrontMatter rewrites the YAML block (block lists, insertion order;
       an emptied block is removed; a throwing callback writes nothing). */
    const dirOf = p => { const s = String(p || ""), i = s.lastIndexOf("/"); return i < 0 ? "" : s.slice(0, i); };
    const joinP = (d, n) => (d && d !== "/" ? d + "/" : "") + n;
    function relPath(from, to) {
      const a = dirOf(from).split("/").filter(Boolean), b = String(to).split("/");
      let i = 0; while (i < a.length && i < b.length - 1 && a[i] === b[i]) i++;
      return "../".repeat(a.length - i) + b.slice(i).join("/");
    }
    function folderFor(dir) { const d = normalizePath(dir || "/"); return d === "/" ? vroot : vault.getFolderByPath(d); }
    function attachDir(sourcePath) {       // attachmentFolderPath: "/" root | "./" next to the note | "./x" below it | "x"
      const c = String(vault.getConfig("attachmentFolderPath") ?? "/");
      if (c === "" || c === "/") return "";
      if (c === "." || c === "./") return dirOf(sourcePath);
      if (c.startsWith("./")) return normalizePath(joinP(dirOf(sourcePath), c.slice(2)));
      return normalizePath(c);
    }
    async function taken(p) { return files.has(p) || await core.exists(p).catch(() => false); }
    // YAML for a frontmatter object: block style, keys in insertion order, plain scalars unless quoting is needed
    function yScalar(v) {
      if (v == null) return "";
      if (typeof v === "number") return Number.isFinite(v) ? String(v) : v !== v ? ".nan" : v > 0 ? ".inf" : "-.inf";
      if (typeof v === "boolean") return String(v);
      if (v instanceof Date) return v.toISOString();
      const s = String(v);
      const plain = s !== "" && s === s.trim() && !/[\n\r\t]/.test(s) && !/^[-?:,\[\]{}#&*!|>'"%@`]/.test(s)
        && !/: |:$| #/.test(s) && !/^(true|false|yes|no|on|off|null|~|[-+]?(\d[\d_]*)?\.?\d+([eE][-+]?\d+)?|0x[0-9a-f]+|\.(inf|nan))$/i.test(s);
      return plain ? s : JSON.stringify(s);
    }
    function yEmit(o, ind) {
      let out = "";
      for (const k of Object.keys(o)) {
        const v = o[k], key = ind + (/^[\w][\w .\/-]*$/.test(k) && k === k.trim() ? k : JSON.stringify(k)) + ":";
        if (v === undefined || typeof v === "function") continue;
        if (Array.isArray(v)) {
          if (!v.length) { out += key + " []\n"; continue; }
          out += key + "\n";
          for (const x of v) out += ind + "  - " + (x !== null && typeof x === "object" && !(x instanceof Date) ? JSON.stringify(x) : yScalar(x)) + "\n";
        } else if (v !== null && typeof v === "object" && !(v instanceof Date)) {
          out += Object.keys(v).length ? key + "\n" + yEmit(v, ind + "  ") : key + " {}\n";
        } else { const s = yScalar(v); out += key + (s === "" ? "" : " " + s) + "\n"; }
      }
      return out;
    }
    /* the frontmatter subset parseYaml reads faithfully: `key: scalar`, `key: [a, b]`,
       `key:` + `  - item` lines. Anything else (nested maps, block scalars, anchors,
       escaped quotes) would be lost on the rewrite, so it is refused, not mangled. */
    function yamlError(msg) { const e = new Error(msg); e.name = "YAMLParseError"; return e; }
    function splitFrontMatter(text) {
      const m = /^---\r?\n/.exec(text);
      if (!m) return { yaml: null, body: text };
      const re = /^(---|\.\.\.)[ \t]*(\r?\n|$)/gm; re.lastIndex = m[0].length;
      let c;
      while ((c = re.exec(text))) if (c.index === m[0].length || text[c.index - 1] === "\n") return { yaml: text.slice(m[0].length, c.index), body: text.slice(c.index + c[0].length) };
      return { yaml: null, body: text };
    }
    function readFrontMatter(yaml) {
      let listKey = false;
      for (const raw of yaml.split("\n")) {
        const l = raw.replace(/\r$/, "");
        if (!l.trim() || /^\s*#/.test(l)) continue;
        if (/^\s*-(\s|$)/.test(l) && listKey) { if (/^\s*-\s*[{|>&*!]/.test(l) || /^\s*-\s*"[^"]*\\/.test(l)) throw yamlError("frontmatter list item not supported: " + l.trim()); continue; }
        const kv = l.match(/^([^\s:#-][^:]*?)\s*:(?:\s+(.*))?$/);
        if (!kv) throw yamlError("frontmatter line not supported: " + l.trim());
        const v = (kv[2] || "").trim();
        if (/^[{|>&*!]/.test(v) || /^"[^"]*\\/.test(v)) throw yamlError("frontmatter value not supported: " + kv[1]);
        listKey = v === "";
      }
      return parseYaml(yaml) || {};
    }
    function prompt(file) {
      return new Promise(resolve => {
        const { Modal, ButtonComponent } = K;
        let done = false;
        const m = new Modal(K.app);
        m.setTitle(file instanceof TFolder ? "Move folder to trash?" : "Move file to trash?");
        m.contentEl.createEl("p", { text: "“" + file.name + "” goes to the vault's .trash folder." });
        const row = m.modalEl.createDiv({ cls: "modal-button-container" });
        new ButtonComponent(row).setButtonText("Cancel").setClass("mod-cancel").onClick(() => m.close());
        new ButtonComponent(row).setButtonText("Move to trash").setClass("mod-cta").setClass("mod-destructive").onClick(async () => {
          done = true; m.close();
          try { await fileManager.trashFile(file); resolve(true); } catch (e) { console.error("[plugin] promptForDeletion:", e); resolve(false); }
        });
        m.onClose = () => { if (!done) { done = true; resolve(false); } };
        m.open();
      });
    }
    function notesUnder(folder) { const out = []; Vault.recurseChildren(folder, f => { const n = f instanceof TFile ? noteOf(f.path) : null; if (n != null) out.push(n); }); return out; }
    class FileManager {
      getNewFileParent(sourcePath, newFilePath) {
        if (newFilePath != null && noteOf(normalizePath(newFilePath)) == null) return folderFor(attachDir(sourcePath)) || vroot;
        const loc = vault.getConfig("newFileLocation");
        if (loc === "current") return folderFor(dirOf(sourcePath)) || vroot;
        if (loc === "folder") return folderFor(vault.getConfig("newFileFolderPath")) || vroot;
        return vroot;
      }
      async getAvailablePathForAttachment(filename, sourcePath) {
        if (sourcePath == null) { const a = K.workspace && K.workspace.getActiveFile && K.workspace.getActiveFile(); sourcePath = a ? a.path : ""; }
        const dir = attachDir(sourcePath);
        if (dir && !vault.getFolderByPath(dir) && !await taken(dir)) await vault.createFolder(dir);
        const name = String(filename), d = name.lastIndexOf(".");
        const base = d > 0 ? name.slice(0, d) : name, ext = d > 0 ? name.slice(d) : "";
        let p = joinP(dir, name);
        for (let i = 1; await taken(p); i++) p = joinP(dir, base + " " + i + ext);
        return p;
      }
      generateMarkdownLink(file, sourcePath, subpath, alias) {
        const fmt = vault.getConfig("newLinkFormat") || "shortest", md = file.extension === "md";
        const sub = subpath ? String(subpath) : "", al = alias ? String(alias) : "";
        let target = fmt === "absolute" ? file.path : fmt === "relative" ? relPath(sourcePath, file.path) : metadataCache.fileToLinktext(file, sourcePath, false);
        if (!vault.getConfig("useMarkdownLinks")) {
          if (md) target = target.replace(/\.md$/, "");
          return "[[" + target + sub + (al ? "|" + al : "") + "]]";
        }
        return "[" + (al || (md ? file.basename : file.name)) + "](" + encodeURI(target) + encodeURI(sub) + ")";
      }
      async processFrontMatter(file, fn) {
        if (!(file instanceof TFile) || file.extension !== "md") throw new Error("processFrontMatter: not a markdown file (" + (file && file.path) + ")");
        const text = await vault.read(file);
        const { yaml, body } = splitFrontMatter(text);
        const fm = yaml == null ? {} : readFrontMatter(yaml);
        fn(fm);
        const out = Object.keys(fm).some(k => fm[k] !== undefined && typeof fm[k] !== "function") ? "---\n" + yEmit(fm, "") + "---\n" + body : body;
        if (out !== text) await vault.modify(file, out);
      }
      async renameFile(file, newPath) {
        const np = normalizePath(newPath), op = file && file.path;
        if (!(file instanceof TAbstractFile) || files.get(op) !== file) throw new Error("renameFile: not a vault file (" + op + ")");
        if (np === op) return;
        if (await taken(np)) throw new Error("Destination file already exists!");
        if (file instanceof TFile && noteOf(op) != null && noteOf(np) != null) return vault.rename(file, np);
        if (file instanceof TFile && (noteOf(op) != null) !== (noteOf(np) != null)) throw new Error("renameFile: a note stays a .md file in opensidian (" + op + " -> " + np + ")");
        const before = file instanceof TFolder ? notesUnder(file) : [];
        const moved = [];
        if (file instanceof TFolder) Vault.recurseChildren(file, f => moved.push([f, f.path]));
        else moved.push([file, op]);
        await core.rename(op, np);
        movePath(file, np);
        for (const [f, o] of moved) vault.trigger("rename", f, o);
        if (before.length) await changed({ renamed: before.map(o => [o, np + o.slice(op.length)]) });
        else if (typeof refreshTree === "function") try { await refreshTree(); } catch (_) {}
      }
      async trashFile(file) {
        if (!(file instanceof TAbstractFile) || files.get(file.path) !== file) throw new Error("trashFile: not a vault file (" + (file && file.path) + ")");
        if (file instanceof TFile && noteOf(file.path) != null) return vault.trash(file);
        const notes = file instanceof TFolder ? notesUnder(file) : [], gone = [];
        Vault.recurseChildren(file, f => gone.push(f));
        await adapter.trashLocal(file.path);
        removeTree(file);
        for (const f of gone.reverse()) vault.trigger("delete", f);
        if (notes.length) await changed({ removed: notes });
        else if (typeof refreshTree === "function") try { await refreshTree(); } catch (_) {}
      }
      promptForDeletion(file) { return prompt(file); }
      /* top10-func: the older name of the same prompt (Calendar's day-note context menu "Delete").
         Same modal, same refusals: a non-vault file is a TypeError, Cancel / Escape trash nothing. */
      promptForFileDeletion(file) {
        if (!(file instanceof TAbstractFile) || files.get(file.path) !== file) return Promise.reject(new TypeError("promptForFileDeletion: not a vault file (" + (file && file.path) + ")"));
        return prompt(file);
      }
      /* top5-func: promptForFileRename (undocumented; Excalidraw asks for a name right after it creates
         a drawing). A modal with the name (its last extension kept aside, selected) and Cancel / Save;
         Enter saves. The rename is renameFile's own, so its refusals hold: a taken name or a name with
         a path separator / reserved character is refused VISIBLY (a Notice; the modal stays open on the
         old name, nothing moves). Resolves once the modal closes (renamed or not). */
      promptForFileRename(file) {
        if (!(file instanceof TAbstractFile) || files.get(file.path) !== file) return Promise.reject(new TypeError("promptForFileRename: not a vault file (" + (file && file.path) + ")"));
        return new Promise(resolve => {
          const { Modal, ButtonComponent, Notice } = K;
          const isF = file instanceof TFile, ext = isF && file.extension ? "." + file.extension : "";
          const stem = isF ? file.name.slice(0, file.name.length - ext.length) : file.name;
          const m = new Modal(K.app);
          m.setTitle(isF ? "Rename file" : "Rename folder");
          m.modalEl.addClass("mod-file-rename");
          const inp = m.contentEl.createEl("input", { type: "text", cls: "rename-textarea" });
          inp.value = stem; inp.spellcheck = false;
          let busy = false;
          const save = async () => {
            if (busy) return;
            const nm = inp.value.trim();
            if (!nm || nm === stem) { m.close(); return; }
            if (/[\\/:]/.test(nm) || nm.startsWith(".")) { new Notice("File name cannot contain any of the following characters: \\ / : (or start with a dot)"); return; }
            const par = file.parent && !file.parent.isRoot() ? file.parent.path + "/" : "";
            busy = true;
            try { await fileManager.renameFile(file, par + nm + ext); m.close(); }
            catch (e) { new Notice("Rename failed: " + String(e && e.message || e)); console.warn("[plugin] promptForFileRename:", e); }
            finally { busy = false; }
          };
          inp.addEventListener("keydown", e => { if (e.key === "Enter" && !e.isComposing) { e.preventDefault(); save(); } });
          const row = m.modalEl.createDiv({ cls: "modal-button-container" });
          new ButtonComponent(row).setButtonText("Cancel").setClass("mod-cancel").onClick(() => m.close());
          new ButtonComponent(row).setButtonText("Save").setCta().onClick(save);
          m.onClose = () => resolve();
          m.open();
          setTimeout(() => { inp.focus(); inp.select(); }, 0);
        });
      }
      /* createNewMarkdownFile (undocumented, part vint): a new note in `folder`
         (null/undefined: the root) named `name` ("Untitled" when absent or blank;
         trimmed; a trailing .md dropped) at the first free "name", "name 1", ...
         (case-insensitive); missing folders on the way ("a/b/c") are created first.
         Not a folder, a non-string name or non-string data: TypeError, like stock. */
      async createNewMarkdownFile(folder, name, data) {
        if (folder == null) folder = vroot;
        if (!(folder instanceof TFolder)) throw new TypeError("createNewMarkdownFile: not a folder (" + (folder && folder.path) + ")");
        if (name != null && typeof name !== "string") throw new TypeError("createNewMarkdownFile: name is not a string");
        if (data != null && typeof data !== "string") throw new TypeError("createNewMarkdownFile: data is not a string");
        const n = String(name == null ? "" : name).trim().replace(/\.md$/i, "") || "Untitled";
        const p = vault.getAvailablePath((folder.isRoot() ? "" : folder.path + "/") + n, "md");
        const segs = p.split("/");
        for (let i = 1; i < segs.length; i++) {
          const d = segs.slice(0, i).join("/");
          if (!vault.getAbstractFileByPath(d)) await vault.createFolder(d);
        }
        return vault.create(p, data == null ? "" : data);
      }
    }
    const fileManager = new FileManager();
    K.exportClasses.FileManager = FileManager;

    Object.assign(K, { DataAdapter, FileSystemAdapter, TAbstractFile, TFile, TFolder, Vault, adapter, changed, ensureModel, files, fileManager, unsupported, inv, metadataCache, metadataTypeManager, noteOf, resolveLink, sweep, td, vault });
  });
  reg("vault:app", function (K) {
    const { host, be } = K;
    const { Events, Scope, commands, fileManager, keymap, metadataCache, metadataTypeManager, plugins, setting, statusBox, unsupported, vault, workspace } = K;
    /* RenderContext (typings 1.10.0): what a rendered value hangs its hover popover
       on; app.renderContext is the app-wide one, no popover until something shows one. */
    class RenderContext { constructor() { this.hoverPopover = null; } }
    RenderContext.prototype.hoverPopover = null;
    /* SecretStorage (typings 1.11.4) over the OS keyring — goal cp-secrets. The store is the
       freedesktop Secret Service, reached only through the backend command secrets_op
       (src-tauri/src/secrets.rs), which answers only a TRUSTED vault: nothing here or in Rust
       writes a secret to the vault, to ~/.opensidian.json or to localStorage.
       The typings API is synchronous and IPC is not, so the vault's ids and values are read
       ONCE, when the plugin runtime starts (ensureModel, below — the same point moment and the
       vault model are loaded, before any plugin's main.js runs), and kept in this closure,
       never on the instance. setSecret updates that copy at once and writes through to the
       keyring in call order; a write the keyring refuses restores the previous value and is
       logged. No Secret Service (or no session bus) = the load fails, and every method throws
       the unsupported error naming the cause: there is no fallback store. Ids are app-wide
       strings (1-64 of a-z, 0-9 and "-"), as stock; any plugin in the
       vault can read any id, which is what SecretComponent's picker relies on. */
    /* app.internalPlugins: the core plugins plugins reach. plugins[id] is the wrapper ({enabled,
       _loaded, instance, load(), views}); config is the id -> enabled map over the core plugins that
       are EXPOSED. A core plugin is exposed by putting its id in config (main.js: canvas); the
       app's own hooks on plugins (graph, switcher, page-preview) stay unexposed, getPluginById null.
       stock keeps config[k] === !!getEnabledPluginById(k). */
    function internalPlugins() {
      const plugins = {}, config = {};
      const own = k => typeof k === "string" && Object.prototype.hasOwnProperty.call(config, k) && Object.prototype.hasOwnProperty.call(plugins, k);
      return {
        plugins, config,
        getPluginById(id) { return own(id) ? plugins[id] : null; },
        getEnabledPluginById(id) { return own(id) && plugins[id].enabled ? plugins[id].instance : null; },
      };
    }
    const ssTok = {}, SS_ID = /^[a-z0-9-]{1,64}$/;   // stock 1.13.7, black-box: apioracle cpsecret.ids / .idlen
    let ssVals = null, ssWhy = "secret storage is not loaded (no plugin has started)", ssP = null, ssQ = Promise.resolve();
    function ssLoad() {
      if (ssP) return ssP;   // loaded, or loading; a failed load is retried by the next start
      ssP = be.invoke("secrets_op", { op: "load" }).then(r => {
        ssVals = new Map(Object.entries((r && r.secrets) || {}).filter(([, v]) => typeof v === "string"));
        ssWhy = null;
      }, e => {
        ssVals = null;
        ssWhy = String((e && e.message) || e);
        ssP = null;
        console.warn("[plugin] secretStorage: " + ssWhy);
      });
      return ssP;
    }
    function ssOpen(what) {
      if (!K.st.trusted) throw unsupported("secretStorage." + what + ": restricted mode is on");
      if (ssVals) return ssVals;
      const why = ssWhy || "secret storage is not loaded";
      throw /^unsupported: /.test(why) ? unsupported(why.slice(13)) : new Error("secretStorage." + what + ": " + why);
    }
    function ssCheckId(id, what) {
      // null / undefined is a TypeError, any other non-id an Error (stock, apioracle cpsecret.ids)
      if (id == null) throw new TypeError("secretStorage." + what + ": the secret id is " + id);
      if (typeof id !== "string" || !SS_ID.test(id)) throw new Error("secretStorage." + what + ": invalid secret id " + JSON.stringify(id) + " (1-64 lowercase letters, digits or dashes)");
    }
    class SecretStorage extends Events {
      constructor(tok) { super(); if (tok !== ssTok) throw new TypeError("SecretStorage: use app.secretStorage"); }
      setSecret(id, secret) {
        ssCheckId(id, "setSecret");
        // this session reads back the value as given (stock: setSecret(id, 7) -> getSecret 7, apioracle
        // cpsecret.roundtrip); the keyring stores its string form, which is what a later start reads
        const vals = ssOpen("setSecret"), v = secret, had = vals.has(id), prev = vals.get(id);
        vals.set(id, v);
        ssQ = ssQ.then(() => be.invoke("secrets_op", { op: "set", id, value: String(v) })).catch(e => {
          if (vals.get(id) === v) { if (had) vals.set(id, prev); else vals.delete(id); }
          console.error("[plugin] secretStorage.setSecret(" + JSON.stringify(id) + "): the keyring refused the write: " + ((e && e.message) || e));
        });
      }
      getSecret(id) {
        const vals = ssOpen("getSecret");
        const v = typeof id === "string" ? vals.get(id) : undefined;
        return v === undefined ? null : v;
      }
      listSecrets() { return Array.from(ssOpen("listSecrets").keys()); }
    }
    {
      // the load rides on the plugin runtime's start (core:manager, a later part, awaits
      // K.ensureModel before evaluating a plugin); restricted mode never gets here
      const ensureModel0 = K.ensureModel;
      K.ensureModel = () => ensureModel0().then(() => ssLoad());
      // goal keychain-pane: Settings ▸ Keychain wrote to the SAME keyring (add / edit / delete);
      // re-read once the queued writes drained, so a plugin never reads a value the user replaced
      // or deleted. Only a runtime that already loaded (or tried) re-reads: restricted mode never did.
      window.addEventListener("os-secrets-changed", () => {
        if (!ssP && !ssVals && ssWhy === "secret storage is not loaded (no plugin has started)") return;
        ssQ = ssQ.then(() => { ssP = null; return ssLoad(); });
      });
    }
    K.exportClasses.RenderContext = RenderContext;
    K.exportClasses.SecretStorage = SecretStorage;
    const app = {
      appId: "",
      vault, workspace, metadataCache, metadataTypeManager, fileManager, keymap, commands, plugins, setting,
      renderContext: new RenderContext(), secretStorage: new SecretStorage(ssTok),
      scope: keymap.getRootScope() || new Scope(),   // stock: app.scope === app.keymap.getRootScope()
      internalPlugins: internalPlugins(),
      statusBar: { get containerEl() { return statusBox(); } },
      dom: { get appContainerEl() { return document.body; } },
      lastEvent: null,
      isMobile: false,
      // no fold persistence: calendar's note creation saves the template's folds (none) here
      foldManager: { load() { return null; }, save() {} },
      loadLocalStorage(k) { try { const v = localStorage.getItem(app.appId + "-" + k); return v == null ? null : JSON.parse(v); } catch (_) { return null; } },
      saveLocalStorage(k, v) { try { if (v == null) localStorage.removeItem(app.appId + "-" + k); else localStorage.setItem(app.appId + "-" + k, JSON.stringify(v)); } catch (_) {} },
      /* top5-func: a VAULT file or folder in the OS default app (obsidian-git "Open in default app",
         templater's scripts-folder button). Rust desk_shell op "vault" confines the path to the vault
         (no "..", no other absolute path, a symlink out of the vault is refused) and refuses in
         restricted mode. Like electron's shell.openPath it resolves "" on success and the error text
         otherwise; a refusal is never silent: it raises a Notice and a console warning. */
      openWithDefaultApp(p) {
        const why = e => String(e && e.message || e);
        const shown = e => { const m = why(e); console.warn("[plugin] openWithDefaultApp(" + JSON.stringify(String(p)) + "): " + m); new K.Notice("Open in default app: " + m); return m; };
        if (!K.st.trusted) return Promise.resolve(shown("EPERM: restricted mode is on"));
        if (!be) return Promise.resolve(shown("EPERM: no desktop backend"));
        return be.invoke("desk_shell", { op: "vault", target: String(p == null ? "" : p) }).then(() => "", shown);
      },
      /* top5-func: getAccentColor (undocumented; Excalidraw derives its element palette from it). The
         accent the theme paints with RIGHT NOW (--interactive-accent, resolved by the engine, so a
         theme or light/dark switch is followed), as "#rrggbb". */
      getAccentColor() {
        const el = document.createElement("span");
        el.style.cssText = "position:absolute;visibility:hidden;color:var(--interactive-accent)";
        (document.body || document.documentElement).appendChild(el);
        const c = getComputedStyle(el).color; el.remove();
        const m = String(c).match(/^rgba?\(\s*([\d.]+)[ ,]+([\d.]+)[ ,]+([\d.]+)/);
        if (!m) return c;
        return "#" + [m[1], m[2], m[3]].map(v => Math.max(0, Math.min(255, Math.round(+v))).toString(16).padStart(2, "0")).join("");
      },
      /* top5-func: getObsidianUrl(file) (undocumented; Excalidraw rewrites a drawing's internal links to it
         on export). The same obsidian://open URL the drag payloads carry (views.js), so a link a plugin
         builds and one the explorer drags are one string. Not a vault file -> TypeError. */
      getObsidianUrl(f) {
        if (!(f instanceof K.TFile) || vault.getAbstractFileByPath(f.path) !== f) throw new TypeError("getObsidianUrl: not a vault file (" + (f && f.path) + ")");
        return K.obsidianUrl(f);
      },
    };
    class App {     // typings expose App as a class; plugins only ever receive `app`
      isDarkMode() { return document.body.classList.contains("theme-dark"); }
      // the base color scheme's name, the same value a vault's .obsidian/appearance.json "theme" holds:
      // "obsidian" (dark) or "moonstone" (light). obsidian-git's
      // line author gutter calls window.app.getTheme() while it paints each marker; without it the
      // gutter's view plugin crashed and no gutter was drawn (deep-git G3)
      getTheme() { return this.isDarkMode() ? "obsidian" : "moonstone"; }
    }
    Object.setPrototypeOf(app, App.prototype);
    metadataTypeManager.app = app;
    // K.appMembers: app-level members contributed by a segment that runs BEFORE this one
    // (name -> property descriptor, e.g. views.js). Empty or absent = no change.
    for (const [k, d] of Object.entries(K.appMembers || {})) {
      if (Object.prototype.hasOwnProperty.call(app, k)) throw new Error("K.appMembers: app." + k + " is already defined");
      Object.defineProperty(app, k, Object.assign({ enumerable: true, configurable: true }, d));
    }

    Object.assign(K, { App, app });
  });
  reg("vault:guarded", function (K) {
    const { host, be } = K;
    const { DataAdapter, FileSystemAdapter, Platform, PlatformGuarded, app } = K;
    /* the guarded "obsidian" (loader.js makeRequire(id, true)): the same API, but
       Platform is PlatformGuarded and FileSystemAdapter is a separate class nothing
       is an instance of, so `app.vault.adapter instanceof FileSystemAdapter` is
       false in this view (DataAdapter only: no basePath for a shell cwd). The
       moment getter is copied as a getter, not a snapshot. */
    class GuardedFileSystemAdapter extends DataAdapter {}
    Object.defineProperty(GuardedFileSystemAdapter, "name", { value: "FileSystemAdapter" });
    Object.assign(K, { GuardedFileSystemAdapter });
  });
})();
