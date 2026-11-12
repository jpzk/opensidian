// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* cp-unrestricted (operator ruling 2026-10-18, stock parity): the Node surface
   EVERY plugin of a trusted vault gets. Plugin code only ever runs in a trusted
   vault (Rust serves no main.js otherwise, api-core enable() refuses), so the
   flip in Settings is the one grant: no per-plugin consent, no owner list.

   Loaded by index.html after node-modules.js and BEFORE api.js (which freezes
   the host); it registers host.desktopNode.view(id). loader.js makeRequire asks
   view.resolve(name) after the pure leaves (path) and the fs facade, and fills
   the factory's free variables (process, global, Buffer, Error, __filename,
   __dirname, SharedArrayBuffer; plugins.rs wrap_cjs) from the same view.

   The pure leaves below are ported from goal/td-desktop-caps ui/plugins/desktop.js
   (our own code): events, string_decoder, readline, util, url, http data,
   constants, Buffer, crypto (sha1/sha256/HMAC + CSPRNG), os, async_hooks,
   module.createRequire, tty, the process shim, a per-plugin global and Error
   with V8 stack-trace hooks. No per-owner branches: one view shape for every
   plugin. Function is the page's own: a trusted page has 'unsafe-eval'.
   A builtin with no member here yet (child_process, electron, net, http
   servers, ...) binds the inert stub of node-modules.js, which throws on use;
   ledger item 4 backs those with Rust commands gated on vault trust only. */
(function () {
  "use strict";
  const host = window.__osPluginHost;
  if (!host) return;
  const bare = n => (String(n).startsWith("node:") ? String(n).slice(5) : String(n));
  const later = f => Promise.resolve().then(f);
  const errOf = e => (e instanceof Error ? e : new Error(String(e && e.message || e)));
  const den = e => { const x = errOf(e); if (!x.code) x.code = "EACCES"; return x; };

  /* ------------------------------------------------ a pure EventEmitter */
  /* Listener maps live in a WeakMap keyed by the emitter and are created on
     first use, like Node's lazy _events: bundled express mixes the prototype
     into a plain function (app) and Node-style subclasses call
     EventEmitter.call(this), so the constructor may never run on the object. */
  const evs = new WeakMap();
  const evOf = o => { let m = evs.get(o); if (!m) { m = new Map(); evs.set(o, m); } return m; };
  class EmitterBase {
    on(n, f) { if (typeof f !== "function") return this; const e = evOf(this), l = e.get(n) || []; l.push({ f, once: false }); e.set(n, l); return this; }
    addListener(n, f) { return this.on(n, f); }
    prependListener(n, f) { if (typeof f !== "function") return this; const e = evOf(this), l = e.get(n) || []; l.unshift({ f, once: false }); e.set(n, l); return this; }
    once(n, f) { if (typeof f !== "function") return this; const e = evOf(this), l = e.get(n) || []; l.push({ f, once: true }); e.set(n, l); return this; }
    prependOnceListener(n, f) { if (typeof f !== "function") return this; const e = evOf(this), l = e.get(n) || []; l.unshift({ f, once: true }); e.set(n, l); return this; }
    off(n, f) { const l = evOf(this).get(n); if (l) { const i = l.findIndex(x => x.f === f); if (i >= 0) l.splice(i, 1); } return this; }
    removeListener(n, f) { return this.off(n, f); }
    removeAllListeners(n) { const e = evOf(this); if (n === undefined) e.clear(); else e.delete(n); return this; }
    listenerCount(n) { return (evOf(this).get(n) || []).length; }
    listeners(n) { return (evOf(this).get(n) || []).map(x => x.f); }
    rawListeners(n) { return this.listeners(n); }
    eventNames() { return [...evOf(this).keys()]; }
    setMaxListeners() { return this; }
    getMaxListeners() { return Infinity; }
    emit(n, ...a) {
      const l = evOf(this).get(n);
      if (!l || !l.length) { if (n === "error") throw errOf(a[0]); return false; }
      for (const x of [...l]) { if (x.once) this.off(n, x.f); try { x.f.apply(this, a); } catch (e) { console.error("[desktop] listener " + String(n) + ":", e); } }
      return true;
    }
  }
  /* callable like Node's: EventEmitter.call(this) initialises nothing (the map
     is lazy); a bare EventEmitter() call returns a new emitter */
  function Emitter() { if (this == null || (typeof this !== "object" && typeof this !== "function")) return new Emitter(); }
  Emitter.prototype = EmitterBase.prototype;
  Object.defineProperty(EmitterBase.prototype, "constructor", { value: Emitter, writable: true, configurable: true });
  Emitter.EventEmitter = Emitter;
  Emitter.defaultMaxListeners = 10;
  Emitter.once = (em, n) => new Promise((res, rej) => { em.once(n, (...a) => res(a)); if (n !== "error") em.once("error", rej); });
  /* Node's module-level helpers. realclaudian reads events.setMaxListeners at
     module scope (it wraps it once, guarded by a marker on the function) and
     the SDK calls it as setMaxListeners(n, signal); listeners are unbounded
     here, so it only accepts its arguments. */
  Emitter.setMaxListeners = function setMaxListeners() {};
  Emitter.getMaxListeners = () => Infinity;
  Emitter.listenerCount = (em, n) => (em && typeof em.listenerCount === "function" ? em.listenerCount(n) : 0);
  const eventsMod = Object.freeze(Emitter);

  /* string_decoder / readline: what the Claude SDK transport wraps the child's
     pipes in. Chunks here are already strings (the broker returns UTF-8 text). */
  class StringDecoder { constructor(enc) { this.encoding = enc || "utf8"; } write(c) { return c == null ? "" : String(c); } end(c) { return c == null ? "" : String(c); } }
  const stringDecoderMod = Object.freeze({ StringDecoder });

  class Lines extends Emitter {
    constructor(input) {
      super();
      this.closed = false; this.terminal = false; this.input = input;
      let buf = "";
      const q = [], waits = [];
      const push = line => { this.emit("line", line); if (waits.length) waits.shift()({ value: line, done: false }); else q.push(line); };
      const fin = () => { if (this.closed) return; if (buf) { const b = buf; buf = ""; push(b); } this.close(); };
      this._q = q; this._waits = waits;
      if (input && typeof input.on === "function") {
        input.on("data", c => { buf += String(c); let i; while ((i = buf.indexOf("\n")) >= 0) { let l = buf.slice(0, i); buf = buf.slice(i + 1); if (l.endsWith("\r")) l = l.slice(0, -1); push(l); } });
        input.on("end", fin); input.on("close", fin);
      }
    }
    close() { if (this.closed) return; this.closed = true; while (this._waits.length) this._waits.shift()({ value: undefined, done: true }); this.emit("close"); }
    pause() { return this; }
    resume() { return this; }
    setPrompt() {}
    prompt() {}
    write() {}
    [Symbol.asyncIterator]() {
      return {
        next: () => (this._q.length ? Promise.resolve({ value: this._q.shift(), done: false }) : this.closed ? Promise.resolve({ value: undefined, done: true }) : new Promise(r => this._waits.push(r))),
        return: () => { this.close(); return Promise.resolve({ value: undefined, done: true }); },
        [Symbol.asyncIterator]() { return this; },
      };
    }
  }
  const readlineMod = Object.freeze({ createInterface: o => new Lines(o && typeof o === "object" && "input" in o ? o.input : o), Interface: Lines });

  /* util: the members the pinned zotero bundle's bundled libraries (execa, get-stream,
     readable-stream, fstream/unzip) touch, some at module scope (promisify(stream.pipeline),
     debuglog("stream"), debuglog("execa").enabled). Pure leaf: string formatting and
     function wrapping only, no I/O; promisify wraps without calling, so promisify(<stub>)
     still denies on use. Any other member is the inert deny stub (mod()). */
  const inspectCustom = Symbol.for("nodejs.util.inspect.custom");
  function inspect(v) {
    if (typeof v === "string") return "'" + v + "'";
    if (typeof v === "function") return "[Function: " + (v.name || "(anonymous)") + "]";
    if (v === null || typeof v !== "object") return String(v);
    if (v instanceof Error) return v.stack || String(v);
    try { return JSON.stringify(v); } catch (_) { return Object.prototype.toString.call(v); }
  }
  inspect.custom = inspectCustom;
  function format(f, ...a) {
    if (typeof f !== "string") return [f, ...a].map(x => (typeof x === "string" ? x : inspect(x))).join(" ");
    let i = 0;
    let s = f.replace(/%[sdifjoO%]/g, m => {
      if (m === "%%") return "%";
      if (i >= a.length) return m;
      const x = a[i++];
      if (m === "%s") return typeof x === "string" ? x : inspect(x);
      if (m === "%d" || m === "%i") return String(m === "%i" ? parseInt(x, 10) : Number(x));
      if (m === "%f") return String(parseFloat(x));
      if (m === "%j") { try { return JSON.stringify(x); } catch (_) { return "[Circular]"; } }
      return inspect(x);
    });
    for (; i < a.length; i++) s += " " + (typeof a[i] === "string" ? a[i] : inspect(a[i]));
    return s;
  }
  const PCUSTOM = Symbol.for("nodejs.util.promisify.custom");
  function promisify(fn) {
    if (typeof fn !== "function") throw new TypeError('The "original" argument must be of type function');
    if (typeof fn[PCUSTOM] === "function") return fn[PCUSTOM];   // exec/execFile resolve {stdout, stderr}
    return function (...a) { return new Promise((res, rej) => { fn.call(this, ...a, (e, ...r) => (e ? rej(e) : res(r.length > 1 ? r : r[0]))); }); };
  }
  promisify.custom = PCUSTOM;
  function debuglog() { const f = () => {}; f.enabled = false; return f; }
  function inherits(ctor, sup) {
    if (typeof ctor !== "function" || typeof sup !== "function") throw new TypeError("inherits: ctor and superCtor must be functions");
    Object.defineProperty(ctor, "super_", { value: sup, writable: true, configurable: true });
    Object.setPrototypeOf(ctor.prototype, sup.prototype);
  }
  const utilOwn = { promisify, debuglog, debug: debuglog, format, inspect, inherits };
  // advanced slides: its bundled express/readable-stream/http-errors call util.inherits, debuglog and
  // format at module scope; callbackify wraps a promise-returning function (pure, never calls it early)
  function callbackify(fn) {
    if (typeof fn !== "function") throw new TypeError('The "original" argument must be of type function');
    return function (...a) { const cb = a.pop(); Promise.resolve().then(() => fn.apply(this, a)).then(v => cb(null, v), e => cb(e || new Error("callbackify: falsy rejection"))); };
  }
  const slidesUtilOwn = { promisify, callbackify, debuglog, debug: debuglog, format, inspect, inherits };

  /* url (slides only): its bundled express (parseurl, serve-static, send) parses the
     request URL of the in-process handler with the legacy url.parse/Url/format/resolve.
     Pure string work over WHATWG URL: no fileURLToPath/pathToFileURL (no host paths),
     no DNS, no I/O. A path-only string is split by hand (no dot-segment rewrite), as
     the legacy parser does, so send's own traversal check still sees the raw path. */
  function LegacyUrl() {
    this.protocol = null; this.slashes = null; this.auth = null; this.host = null; this.port = null; this.hostname = null;
    this.hash = null; this.search = null; this.query = null; this.pathname = null; this.path = null; this.href = null;
  }
  const qsObj = s => { const o = {}; for (const [k, v] of new URLSearchParams(s)) { if (Object.prototype.hasOwnProperty.call(o, k)) o[k] = [].concat(o[k], v); else o[k] = v; } return o; };
  function urlParse(s, parseQuery) {
    s = String(s).trim();
    const u = new LegacyUrl();
    let rest = s;
    if (/^[A-Za-z][A-Za-z0-9+.-]*:/.test(s)) {
      let w = null;
      try { w = new URL(s); } catch (_) { w = null; }
      if (w) {
        u.protocol = w.protocol; u.slashes = /^[A-Za-z][A-Za-z0-9+.-]*:\/\//.test(s) ? true : null;
        u.auth = w.username ? decodeURIComponent(w.username) + (w.password ? ":" + decodeURIComponent(w.password) : "") : null;
        u.host = w.host || null; u.hostname = w.hostname || null; u.port = w.port || null;
        rest = w.pathname + w.search + w.hash;
      }
    }
    const h = rest.indexOf("#"); if (h >= 0) { u.hash = rest.slice(h); rest = rest.slice(0, h); }
    const q = rest.indexOf("?");
    if (q >= 0) { u.search = rest.slice(q); u.query = rest.slice(q + 1); rest = rest.slice(0, q); }
    else if (parseQuery) { u.search = ""; u.query = {}; }
    if (parseQuery && typeof u.query === "string") u.query = qsObj(u.query);
    u.pathname = rest || (u.slashes ? "/" : null);
    u.path = (u.pathname || "") + (u.search || "") || null;
    u.href = urlFormat(u);
    return u;
  }
  function urlFormat(o) {
    if (typeof o === "string") o = urlParse(o);
    if (o instanceof URL) return o.href;
    if (!o || typeof o !== "object") return "";
    const proto = o.protocol ? (String(o.protocol).endsWith(":") ? String(o.protocol) : o.protocol + ":") : "";
    let host = "";
    if (o.host) host = String(o.host);
    else if (o.hostname) host = String(o.hostname) + (o.port ? ":" + o.port : "");
    /* Node's legacy url.format appends auth ("the username and password separated by ':'")
       with EVERY ':' kept literal (its auth escape set = encodeURIComponent's unreserved set
       plus ':'), so the pieces between colons are encoded and the colons never are:
       {auth: "u:p:q"} -> "u:p:q@", {auth: "a@b"} -> "a%40b@". */
    const auth = o.auth ? String(o.auth).split(":").map(encodeURIComponent).join(":") + "@" : "";
    const slashes = o.slashes || /^(https?|ftp|file|wss?):$/.test(proto) ? "//" : "";
    let search = o.search != null ? String(o.search) : "";
    if (!search && o.query && typeof o.query === "object") { const p = new URLSearchParams(); for (const k of Object.keys(o.query)) for (const v of [].concat(o.query[k])) p.append(k, String(v)); const t = p.toString(); search = t ? "?" + t : ""; }
    if (search && search[0] !== "?") search = "?" + search;
    const hash = o.hash ? (String(o.hash)[0] === "#" ? String(o.hash) : "#" + o.hash) : "";
    return proto + (host || proto === "file:" ? slashes + auth + host : "") + (o.pathname != null ? String(o.pathname) : "") + search + hash;
  }
  function urlResolve(from, to) {
    const B = "http://opensidian.invalid";
    let r;
    try { r = new URL(String(to), new URL(String(from), B + "/")); } catch (_) { return String(to); }
    return r.origin === B ? r.pathname + r.search + r.hash : r.href;
  }
  // fileURLToPath / pathToFileURL are api-vault.js URL_MOD (adapter.url), looked up at call time
  const vaultUrl = n => { const m = host.vfsUrl; if (!m) { const e = new Error("url." + n + ": the vault url module is not loaded"); e.code = "ERR_INVALID_STATE"; throw e; } return m; };
  const urlMod = Object.freeze({ Url: LegacyUrl, parse: urlParse, format: urlFormat, resolve: urlResolve, URL, URLSearchParams,
    fileURLToPath: u => vaultUrl("fileURLToPath").fileURLToPath(u), pathToFileURL: p => vaultUrl("pathToFileURL").pathToFileURL(p) });

  /* http data members (Node 18 values, data only). Bundled express builds app[method] for
     every entry of http.METHODS at module scope, then app.del = depd.function(app.delete):
     with a 3-entry list app.delete was undefined and the slides load failed with "argument fn
     must be a function". No function, no reach: the request path is still the facade server. */
  const HTTP_METHODS = Object.freeze(["ACL", "BIND", "CHECKOUT", "CONNECT", "COPY", "DELETE", "GET", "HEAD", "LINK", "LOCK", "M-SEARCH", "MERGE",
    "MKACTIVITY", "MKCALENDAR", "MKCOL", "MOVE", "NOTIFY", "OPTIONS", "PATCH", "POST", "PROPFIND", "PROPPATCH", "PURGE", "PUT", "REBIND",
    "REPORT", "SEARCH", "SOURCE", "SUBSCRIBE", "TRACE", "UNBIND", "UNLINK", "UNLOCK", "UNSUBSCRIBE"]);
  const HTTP_STATUS = Object.freeze({ 100: "Continue", 101: "Switching Protocols", 102: "Processing", 103: "Early Hints", 200: "OK", 201: "Created",
    202: "Accepted", 203: "Non-Authoritative Information", 204: "No Content", 205: "Reset Content", 206: "Partial Content", 207: "Multi-Status",
    208: "Already Reported", 226: "IM Used", 300: "Multiple Choices", 301: "Moved Permanently", 302: "Found", 303: "See Other", 304: "Not Modified",
    305: "Use Proxy", 307: "Temporary Redirect", 308: "Permanent Redirect", 400: "Bad Request", 401: "Unauthorized", 402: "Payment Required",
    403: "Forbidden", 404: "Not Found", 405: "Method Not Allowed", 406: "Not Acceptable", 407: "Proxy Authentication Required", 408: "Request Timeout",
    409: "Conflict", 410: "Gone", 411: "Length Required", 412: "Precondition Failed", 413: "Payload Too Large", 414: "URI Too Long",
    415: "Unsupported Media Type", 416: "Range Not Satisfiable", 417: "Expectation Failed", 418: "I'm a Teapot", 421: "Misdirected Request",
    422: "Unprocessable Entity", 423: "Locked", 424: "Failed Dependency", 425: "Too Early", 426: "Upgrade Required", 428: "Precondition Required",
    429: "Too Many Requests", 431: "Request Header Fields Too Large", 451: "Unavailable For Legal Reasons", 500: "Internal Server Error",
    501: "Not Implemented", 502: "Bad Gateway", 503: "Service Unavailable", 504: "Gateway Timeout", 505: "HTTP Version Not Supported",
    506: "Variant Also Negotiates", 507: "Insufficient Storage", 508: "Loop Detected", 509: "Bandwidth Limit Exceeded", 510: "Not Extended",
    511: "Network Authentication Required" });

  /* `Error` for an owner (factory parameter, plugins.rs wrap_cjs_ctx). Bundled depd (express,
     body-parser) calls Error.captureStackTrace with a V8 prepareStackTrace and reads CallSite
     objects (getFileName, getLineNumber, ...) at module scope; WebKit ignores
     prepareStackTrace and leaves a string, so local-rest failed with "callSite.getFileName is
     not a function". This is the realm's Error behind a proxy: construct, call, instanceof,
     extends and prototype are the real ones; captureStackTrace / prepareStackTrace /
     stackTraceLimit are per owner (a plugin setting them never touches the page's Error) and
     CallSites are parsed from the engine's own stack string. Pure: no I/O, no new reach. */
  function ownError() {
    const E = Error;
    let prep, limit = 10;
    const extra = new Map();   // other writes land here, per owner
    const site = (fn, file, line, col) => Object.freeze({
      getFileName: () => file, getScriptNameOrSourceURL: () => file, getLineNumber: () => line, getColumnNumber: () => col,
      getFunctionName: () => fn || null, getMethodName: () => null, getTypeName: () => null, getThis: () => undefined,
      getFunction: () => undefined, getEvalOrigin: () => undefined, isEval: () => false, isNative: () => file === "[native code]",
      isToplevel: () => !fn, isConstructor: () => false, isAsync: () => false, isPromiseAll: () => false, getPromiseIndex: () => null,
      toString: () => { const at = file + (line != null ? ":" + line + (col != null ? ":" + col : "") : ""); return fn ? fn + " (" + at + ")" : at; },
    });
    const frames = raw => String(raw || "").split("\n").filter(Boolean).map(l => {
      const at = l.lastIndexOf("@"), fn = at < 0 ? "" : l.slice(0, at), loc = at < 0 ? l : l.slice(at + 1);
      const m = /^(.*?)(?::(\d+))?(?::(\d+))?$/.exec(loc);
      return site(fn, m[1] || null, m[2] != null ? +m[2] : null, m[3] != null ? +m[3] : null);
    });
    function captureStackTrace(obj) {
      if (obj === null || (typeof obj !== "object" && typeof obj !== "function")) throw new TypeError("Error.captureStackTrace: the target must be an object");
      const n = Math.max(0, Math.floor(Number(limit)) || 0);
      const cs = frames(new E().stack).slice(1, 1 + n);   // [0] is this function
      let v;
      if (typeof prep === "function") v = prep(obj, cs);
      else { let h; try { h = E.prototype.toString.call(obj); } catch (_) { h = "Error"; } v = h + cs.map(c => "\n    at " + c.toString()).join(""); }
      Object.defineProperty(obj, "stack", { value: v, writable: true, configurable: true, enumerable: false });
    }
    return new Proxy(E, {
      get(o, k, r) {
        if (k === "captureStackTrace") return captureStackTrace;
        if (k === "prepareStackTrace") return prep;
        if (k === "stackTraceLimit") return limit;
        if (extra.has(k)) return extra.get(k);
        return Reflect.get(o, k, o);
      },
      set(o, k, v) {
        if (k === "prepareStackTrace") { prep = v; return true; }
        if (k === "stackTraceLimit") { limit = v; return true; }
        if (k === "prototype" || k === "length") return false;
        extra.set(k, v); return true;   // never the page's Error
      },
      defineProperty() { return false; },
      deleteProperty(o, k) { if (k === "prepareStackTrace") { prep = undefined; return true; } return false; },
    });
  }

  /* constants: graceful-fs (zotero, slides) reads require("constants") at module scope
     (hasOwnProperty("O_SYMLINK"), O_WRONLY) and zotero's bundled tar reads the S_IF* mode
     bits. Pure data, Linux values, exactly the members those bundles touch. O_SYMLINK is
     absent (as on Linux), so graceful-fs never installs its lchmod/lutimes polyfills.
     A plain frozen object: graceful-fs calls the inherited hasOwnProperty. */
  const constantsMod = Object.freeze({
    O_WRONLY: 1,
    S_IFMT: 0o170000, S_IFIFO: 0o010000, S_IFCHR: 0o020000, S_IFDIR: 0o040000,
    S_IFBLK: 0o060000, S_IFLNK: 0o120000,
  });

  /* Buffer: the free variable (and require("buffer").Buffer) the owners' bundles read —
     zotero: safe-buffer/execa/get-stream/tar (Buffer.from, isBuffer, concat, byteLength,
     alloc, isEncoding at module scope), and the other three via their bundled streams.
     plugins.rs wrap_cjs_ctx binds it as the owners' 6th factory parameter. Pure: a Uint8Array
     subclass plus encode/decode of utf8, latin1/binary, ascii, hex, base64(url), ucs2. No I/O,
     no host state, no pooling (allocUnsafe is zero-filled), one frozen class for all owners. */
  const ENC = new Set(["utf8", "utf-8", "latin1", "binary", "ascii", "hex", "base64", "base64url", "ucs2", "ucs-2", "utf16le", "utf-16le"]);
  const encName = e => { const n = e == null ? "utf8" : String(e).toLowerCase(); if (!ENC.has(n)) throw new TypeError("Unknown encoding: " + e); return n; };
  const latin1Bytes = s => { const b = new Uint8Array(s.length); for (let i = 0; i < s.length; i++) b[i] = s.charCodeAt(i) & 255; return b; };
  const latin1Str = (b, mask) => { let s = ""; for (let i = 0; i < b.length; i += 8192) s += String.fromCharCode.apply(null, mask ? Array.from(b.subarray(i, i + 8192), x => x & mask) : b.subarray(i, i + 8192)); return s; };
  function encode(s, e) {
    s = String(s);
    switch (encName(e)) {
      case "latin1": case "binary": case "ascii": return latin1Bytes(s);
      case "hex": { const n = s.length >> 1, b = new Uint8Array(n); let i = 0; for (; i < n; i++) { const v = parseInt(s.substr(i * 2, 2), 16); if (Number.isNaN(v)) break; b[i] = v; } return b.subarray(0, i); }
      case "base64": case "base64url": { const t = s.replace(/-/g, "+").replace(/_/g, "/").replace(/[^A-Za-z0-9+/]/g, ""); try { return latin1Bytes(atob(t + "===".slice((t.length + 3) % 4))); } catch (_) { return new Uint8Array(0); } }
      case "ucs2": case "ucs-2": case "utf16le": case "utf-16le": { const b = new Uint8Array(s.length * 2); for (let i = 0; i < s.length; i++) { const c = s.charCodeAt(i); b[i * 2] = c & 255; b[i * 2 + 1] = c >> 8; } return b; }
      default: return new TextEncoder().encode(s);
    }
  }
  function decode(b, e) {
    switch (encName(e)) {
      case "latin1": case "binary": return latin1Str(b, 0);
      case "ascii": return latin1Str(b, 127);
      case "hex": { let s = ""; for (const x of b) s += (x < 16 ? "0" : "") + x.toString(16); return s; }
      case "base64": return btoa(latin1Str(b, 0));
      case "base64url": return btoa(latin1Str(b, 0)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
      case "ucs2": case "ucs-2": case "utf16le": case "utf-16le": { let s = ""; for (let i = 0; i + 1 < b.length; i += 2) s += String.fromCharCode(b[i] | (b[i + 1] << 8)); return s; }
      default: return new TextDecoder().decode(b);
    }
  }
  const BufferClass = class Buffer extends Uint8Array {
    // legacy new Buffer(string, enc) / new Buffer(array) as well as (n) and (arrayBuffer, off, len)
    constructor(a, b, c) {
      if (typeof a === "string") super(encode(a, b));
      else if (Array.isArray(a)) super(a);
      else if (a instanceof ArrayBuffer) super(a, b || 0, c === undefined ? a.byteLength - (b || 0) : c);
      else if (ArrayBuffer.isView(a)) super(a);
      else super(a === undefined ? 0 : a);
    }
    static from(v, b, c) {
      if (typeof v === "string") return new Buffer(encode(v, b));
      if (v instanceof ArrayBuffer) return new Buffer(v, b, c);
      if (ArrayBuffer.isView(v)) { const u = new Uint8Array(v.buffer, v.byteOffset, v.byteLength), b = new Buffer(u.length); b.set(u); return b; }
      if (Array.isArray(v)) return new Buffer(v);
      if (v && typeof v === "object" && v.type === "Buffer" && Array.isArray(v.data)) return new Buffer(v.data);
      throw new TypeError("The first argument must be of type string, Buffer, ArrayBuffer or Array");
    }
    static alloc(n, fill, enc) { const b = new Buffer(n); if (fill !== undefined && fill !== 0) b.fill(fill, 0, b.length, enc); return b; }
    static allocUnsafe(n) { return new Buffer(n); }
    static allocUnsafeSlow(n) { return new Buffer(n); }
    static isBuffer(x) { return x instanceof Buffer; }
    static isEncoding(e) { return typeof e === "string" && ENC.has(e.toLowerCase()); }
    static byteLength(v, e) { return typeof v === "string" ? encode(v, e).length : v.byteLength; }
    static concat(list, total) {
      if (total === undefined) total = list.reduce((n, x) => n + x.length, 0);
      const out = new Buffer(total);
      let o = 0;
      for (const x of list) { if (o >= total) break; const s = x.length > total - o ? x.subarray(0, total - o) : x; out.set(s, o); o += s.length; }
      return out;
    }
    static compare(a, b) { return a.compare(b); }
    toString(e, s, t) { return decode(this.subarray(s || 0, t === undefined ? this.length : t), e); }
    toJSON() { return { type: "Buffer", data: Array.from(this) }; }
    slice(s, t) { return this.subarray(s, t); }
    equals(o) { return this.compare(o) === 0; }
    compare(o) { const n = Math.min(this.length, o.length); for (let i = 0; i < n; i++) if (this[i] !== o[i]) return this[i] < o[i] ? -1 : 1; return this.length === o.length ? 0 : this.length < o.length ? -1 : 1; }
    copy(t, ts, ss, se) { const s = this.subarray(ss || 0, se === undefined ? this.length : se).subarray(0, t.length - (ts || 0)); t.set(s, ts || 0); return s.length; }
    write(str, off, len, enc) {
      if (typeof off === "string") { enc = off; off = 0; len = undefined; } else if (typeof len === "string") { enc = len; len = undefined; }
      off = off || 0; const b = encode(str, enc).subarray(0, Math.min(len === undefined ? Infinity : len, this.length - off)); this.set(b, off); return b.length;
    }
    fill(v, s, t, enc) {
      if (typeof s === "string") { enc = s; s = 0; t = this.length; }
      if (typeof v === "string") { const b = encode(v, enc); if (!b.length) return super.fill(0, s, t); for (let i = s || 0, j = 0, end = t === undefined ? this.length : t; i < end; i++, j = (j + 1) % b.length) this[i] = b[j]; return this; }
      return super.fill(v, s, t);
    }
  };
  /* Node's Buffer is callable without new (the deprecated Buffer(x) form, and
     Buffer.call(this, ...) from util.inherits subclasses); a class constructor throws there,
     which failed the advanced-slides load ("Cannot call a class constructor Buffer without
     new", gate xXzyJg41). The owners' Buffer is a plain function over the class: called or
     constructed it returns a class instance, shares its prototype (instanceof holds) and its
     statics. The class keeps its own constructor, so subarray/map species stay Buffers. */
  function Buffer(a, b, c) { return new BufferClass(a, b, c); }
  Buffer.prototype = BufferClass.prototype;
  for (const k of Object.getOwnPropertyNames(BufferClass)) {
    if (k !== "length" && k !== "name" && k !== "prototype") Object.defineProperty(Buffer, k, Object.getOwnPropertyDescriptor(BufferClass, k));
  }
  Object.setPrototypeOf(Buffer, Uint8Array);
  Object.freeze(BufferClass.prototype); Object.freeze(BufferClass); Object.freeze(Buffer);

  const bufferMod = Object.freeze({ Buffer, kMaxLength: 2 ** 31 - 1, constants: Object.freeze({ MAX_LENGTH: 2 ** 31 - 1 }) });

  /* crypto, the pure part only (realclaudian + local REST): local REST's bundled forge seeds
     its PRNG from crypto.randomBytes and MCP code calls randomUUID at load; its express
     computes ETags with createHash("sha1") per response; realclaudian hashes and mints ids.
     All of it is computation on bytes the plugin already holds plus the page's CSPRNG
     (crypto.getRandomValues): no I/O, no key store, no host state. sha1/sha256 (+ HMAC) are
     implemented here; every other digest throws ERR_OSSL_EVP_UNSUPPORTED. Signing, keys,
     certificates, ciphers and pbkdf2 are NOT here: they stay the deny stub, or for local
     REST read undefined so forge takes its own pure-JS path (it feature-tests them). */
  const rot = (x, n) => (x << n) | (x >>> (32 - n));
  function mdPad(m) {
    const n = m.length, t = ((n + 8) >> 6) + 1, b = new Uint8Array(t * 64);
    b.set(m); b[n] = 0x80;
    const dv = new DataView(b.buffer), bits = n * 8;
    dv.setUint32(b.length - 8, Math.floor(bits / 2 ** 32)); dv.setUint32(b.length - 4, bits >>> 0);
    return dv;
  }
  function sha1(m) {
    const dv = mdPad(m), w = new Int32Array(80);
    let h0 = 0x67452301, h1 = 0xefcdab89, h2 = 0x98badcfe, h3 = 0x10325476, h4 = 0xc3d2e1f0;
    for (let o = 0; o < dv.byteLength; o += 64) {
      for (let i = 0; i < 16; i++) w[i] = dv.getInt32(o + i * 4);
      for (let i = 16; i < 80; i++) w[i] = rot(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
      let a = h0, b = h1, c = h2, d = h3, e = h4;
      for (let i = 0; i < 80; i++) {
        const f = i < 20 ? (b & c) | (~b & d) : i < 40 ? b ^ c ^ d : i < 60 ? (b & c) | (b & d) | (c & d) : b ^ c ^ d;
        const k = i < 20 ? 0x5a827999 : i < 40 ? 0x6ed9eba1 : i < 60 ? 0x8f1bbcdc : 0xca62c1d6;
        const t = (rot(a, 5) + f + e + k + w[i]) | 0;
        e = d; d = c; c = rot(b, 30); b = a; a = t;
      }
      h0 = (h0 + a) | 0; h1 = (h1 + b) | 0; h2 = (h2 + c) | 0; h3 = (h3 + d) | 0; h4 = (h4 + e) | 0;
    }
    const out = new DataView(new ArrayBuffer(20));
    [h0, h1, h2, h3, h4].forEach((h, i) => out.setInt32(i * 4, h));
    return new Uint8Array(out.buffer);
  }
  const K256 = new Int32Array([0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb,
    0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f,
    0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2]);
  function sha256(m) {
    const dv = mdPad(m), w = new Int32Array(64);
    const h = new Int32Array([0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]);
    const ror = (x, n) => (x >>> n) | (x << (32 - n));
    for (let o = 0; o < dv.byteLength; o += 64) {
      for (let i = 0; i < 16; i++) w[i] = dv.getInt32(o + i * 4);
      for (let i = 16; i < 64; i++) {
        const s0 = ror(w[i - 15], 7) ^ ror(w[i - 15], 18) ^ (w[i - 15] >>> 3), s1 = ror(w[i - 2], 17) ^ ror(w[i - 2], 19) ^ (w[i - 2] >>> 10);
        w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0;
      }
      let [a, b, c, d, e, f, g, hh] = h;
      for (let i = 0; i < 64; i++) {
        const t1 = (hh + (ror(e, 6) ^ ror(e, 11) ^ ror(e, 25)) + ((e & f) ^ (~e & g)) + K256[i] + w[i]) | 0;
        const t2 = ((ror(a, 2) ^ ror(a, 13) ^ ror(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) | 0;
        hh = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0;
      }
      h[0] += a; h[1] += b; h[2] += c; h[3] += d; h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
    }
    const out = new DataView(new ArrayBuffer(32));
    h.forEach((x, i) => out.setInt32(i * 4, x));
    return new Uint8Array(out.buffer);
  }
  const DIGESTS = { sha1, sha256 };
  // known-answer self-test (FIPS 180-2 example vectors, one and two blocks): a wrong digest must never reach
  // a plugin, so on any mismatch every hash and HMAC throws instead (fail closed)
  const KAT = (() => {
    try {
      const h = (f, s) => decode(f(new TextEncoder().encode(s)), "hex"), q = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
      return h(sha1, "abc") === "a9993e364706816aba3e25717850c26c9cd0d89d" && h(sha1, q) === "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        && h(sha256, "abc") === "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        && h(sha256, q) === "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1";
    } catch (_) { return false; }
  })();
  function digestFn(alg) {
    if (!KAT) { const e = new Error("crypto: the sha1/sha256 self-test failed; hashing is disabled"); e.code = "ERR_OSSL_EVP_UNSUPPORTED"; throw e; }
    const f = DIGESTS[String(alg).toLowerCase().replace(/-/g, "")];
    if (!f) { const e = new Error("Digest method not supported: " + alg + " (opensidian: sha1/sha256 only)"); e.code = "ERR_OSSL_EVP_UNSUPPORTED"; throw e; }
    return f;
  }
  function hashBytes(d, enc) {
    if (typeof d === "string") return encode(d, enc);
    if (ArrayBuffer.isView(d)) return new Uint8Array(d.buffer, d.byteOffset, d.byteLength);
    if (d instanceof ArrayBuffer) return new Uint8Array(d);
    throw new TypeError('The "data" argument must be of type string or an instance of Buffer, TypedArray, or DataView');
  }
  const joinBytes = parts => { let n = 0; for (const p of parts) n += p.length; const b = new Uint8Array(n); let o = 0; for (const p of parts) { b.set(p, o); o += p.length; } return b; };
  function hashObject(compute) {
    const parts = [];
    let done = false;
    const h = {
      update(d, enc) { if (done) throw Object.assign(new Error("Digest already called"), { code: "ERR_CRYPTO_HASH_FINALIZED" }); parts.push(hashBytes(d, enc).slice()); return h; },
      digest(enc) {
        if (done) throw Object.assign(new Error("Digest already called"), { code: "ERR_CRYPTO_HASH_FINALIZED" });
        done = true;
        const out = Buffer.from(compute(joinBytes(parts)));
        return enc === undefined || enc === "buffer" ? out : out.toString(enc);
      },
      copy() { const c = hashObject(compute); for (const p of parts) c.update(p); return c; },
    };
    return h;
  }
  const createHash = alg => { const f = digestFn(alg); return hashObject(f); };
  function createHmac(alg, key) {
    const f = digestFn(alg);
    let k = key && typeof key === "object" && !ArrayBuffer.isView(key) && typeof key.export === "function" ? key.export() : hashBytes(key);
    if (k.length > 64) k = f(k);
    const ki = new Uint8Array(64), ko = new Uint8Array(64);
    for (let i = 0; i < 64; i++) { const v = i < k.length ? k[i] : 0; ki[i] = v ^ 0x36; ko[i] = v ^ 0x5c; }
    return hashObject(m => f(joinBytes([ko, f(joinBytes([ki, m]))])));
  }
  const rng = a => { for (let i = 0; i < a.byteLength; i += 65536) crypto.getRandomValues(new Uint8Array(a.buffer, a.byteOffset + i, Math.min(65536, a.byteLength - i))); return a; };
  function randomBytes(n, cb) {
    if (!Number.isInteger(n) || n < 0 || n > 2 ** 31 - 1) throw new RangeError('The "size" argument must be an integer in [0, 2147483647]');
    const b = rng(Buffer.alloc(n));
    if (typeof cb === "function") { queueMicrotask(() => cb(null, b)); return undefined; }
    return b;
  }
  function randomUUID() {
    if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
    const b = rng(new Uint8Array(16)); b[6] = (b[6] & 15) | 64; b[8] = (b[8] & 63) | 128;
    const x = decode(b, "hex"); return x.slice(0, 8) + "-" + x.slice(8, 12) + "-" + x.slice(12, 16) + "-" + x.slice(16, 20) + "-" + x.slice(20);
  }
  function timingSafeEqual(a, b) {
    a = hashBytes(a); b = hashBytes(b);
    if (a.length !== b.length) throw Object.assign(new RangeError("Input buffers must have the same byte length"), { code: "ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH" });
    let d = 0; for (let i = 0; i < a.length; i++) d |= a[i] ^ b[i]; return d === 0;
  }
  const cryptoOwn = Object.freeze({ createHash, createHmac, randomBytes, randomUUID, timingSafeEqual,
    getRandomValues: a => crypto.getRandomValues(a), getHashes: () => ["sha1", "sha256"] });
  /* realclaudian, the rest of its module-scope surface in one pass (gate QPmEkmwP:
     require("os") stub called; the static dump of every os/module/async_hooks read in
     its bundle is homedir/tmpdir/release/userInfo/networkInterfaces, createRequire,
     AsyncLocalStorage). All three are constants or pure computation:
       - os: fixed values that describe no real host (no username, no NIC, no kernel);
         networkInterfaces() is empty because the page has no interface to report.
         The paths it builds from homedir() land outside /vault, where the confined fs
         answers them as missing; the claude process itself only starts through the broker.
       - module.createRequire(): the owner's OWN require (the loader binds it), so its
         bundled CJS deps get exactly the facades and stubs a top-level require gets; no
         new reach. resolve() has no module paths and throws MODULE_NOT_FOUND.
       - async_hooks.AsyncLocalStorage: the store is visible inside run()'s synchronous
         extent only (no async-context propagation in the page); getStore() is undefined
         elsewhere, which callers already handle (?? default). */
  const OS_HOME = "/home/opensidian";
  const osOwn = Object.freeze({
    homedir: () => OS_HOME, tmpdir: () => "/tmp", hostname: () => "localhost",
    platform: () => "linux", type: () => "Linux", arch: () => "x64", machine: () => "x86_64",
    release: () => "0.0.0", version: () => "", endianness: () => "LE", EOL: "\n", devNull: "/dev/null",
    userInfo: () => ({ uid: -1, gid: -1, username: "opensidian", homedir: OS_HOME, shell: null }),
    networkInterfaces: () => ({}), cpus: () => [], availableParallelism: () => 1,
    totalmem: () => 0, freemem: () => 0, uptime: () => 0, loadavg: () => [0, 0, 0],
  });
  class AsyncLocalStorage {
    #on = false; #has = false; #store = undefined;
    getStore() { return this.#on && this.#has ? this.#store : undefined; }
    run(store, fn, ...args) {
      const ph = this.#has, ps = this.#store, po = this.#on;
      this.#on = true; this.#has = true; this.#store = store;
      try { return fn(...args); } finally { this.#on = po; this.#has = ph; this.#store = ps; }
    }
    exit(fn, ...args) {
      const ph = this.#has, ps = this.#store;
      this.#has = false; this.#store = undefined;
      try { return fn(...args); } finally { this.#has = ph; this.#store = ps; }
    }
    enterWith(store) { this.#on = true; this.#has = true; this.#store = store; }
    disable() { this.#on = false; this.#has = false; this.#store = undefined; }
    static bind(fn) { return fn; }
    static snapshot() { return (fn, ...args) => fn(...args); }
  }
  const asyncHooksOwn = Object.freeze({ AsyncLocalStorage });

  /* the free variable `process` an owner's bundle reads (Electron-renderer code: module-scope
     process.umask(), semver checks on process.version, process.platform, process.env.NODE_DEBUG).
     plugins.rs wrap_cjs_ctx binds it as the owners' 4th factory parameter and loader.js fills
     it from view.process. I/O-free: constants and no-ops only. No exit/kill/stdout/stdin/
     binding/getuid/chdir, no electron version, an empty env. chdir is absent, not a throwing
     member: graceful-fs (zotero, slides) wraps a present chdir and setPrototypeOf(wrapper, chdir)s it;
     on this frozen shim the wrap assignment is dropped, so that became setPrototypeOf(chdir, chdir)
     ("Cyclic __proto__ value") and failed the load. execa / which check chdir !== undefined first. */
  function stdioShim(fd) {
    const s = Object.create(null);
    const noop = () => s;
    return Object.freeze(Object.assign(s, { fd, isTTY: false, columns: 80, rows: 24, writable: true,
      write: (c, e, cb) => { const f = typeof e === "function" ? e : cb; if (typeof f === "function") queueMicrotask(() => f()); return true; },
      end: noop, on: noop, once: noop, off: noop, addListener: noop, removeListener: noop, emit: () => false }));
  }
  function processShim() {
    const self = Object.create(null);
    const noop = () => self;
    return Object.freeze(Object.assign(self, {
      platform: "linux", arch: "x64", browser: false, pid: 0,
      env: Object.freeze(Object.create(null)),
      version: "v18.0.0",
      versions: Object.freeze(Object.assign(Object.create(null), { node: "18.0.0" })),
      execArgv: Object.freeze([]), argv: Object.freeze([]), execPath: "",
      cwd: () => "/",
      umask: () => 0o022,
      nextTick: (fn, ...a) => { queueMicrotask(() => fn(...a)); },
      emitWarning: () => {},   // deprecation warnings: dropped (no stderr)
      // stdout/stderr: no terminal (isTTY false, as in a GUI renderer); writes are dropped, like
      // emitWarning: the page has no stdio. realclaudian's SDK reads process.stdout.isTTY on send.
      stdout: stdioShim(1), stderr: stdioShim(2),
      on: noop, once: noop, off: noop, addListener: noop, removeListener: noop, removeAllListeners: noop, emit: () => false,
    }));
  }

  /* `global`: Node's name for the global object. Bundled libraries read it (lodash root
     check global.Object === Object, readable-stream, global.process) and graceful-fs WRITES a
     queue symbol onto it. Plugins already share the page realm, so reads fall through to
     window (no new reach); writes and defines land on the owner's own object and never touch
     window. global.process is the owner's shim. */
  function ownGlobal(proc) {
    const own = Object.create(null);
    own.process = proc;
    own.Buffer = Buffer;
    // window methods (setTimeout, fetch, ...) are bound so a call through global keeps
    // this === window; constructors (Object, Uint8Array, ...) pass through unbound so
    // identity checks like global.Object === Object hold
    const bound = new Map();
    const pass = k => {
      const v = window[k];
      if (typeof v !== "function" || typeof k !== "string" || !/^[a-z]/.test(k)) return v;
      let b = bound.get(k);
      if (!b || b.f !== v) { b = { f: v, b: v.bind(window) }; bound.set(k, b); }
      return b.b;
    };
    const g = new Proxy(own, {
      get(o, k) { if (k === "global" || k === "globalThis" || k === "self" || k === "window") return k === "window" ? window : g; return Object.prototype.hasOwnProperty.call(o, k) ? o[k] : pass(k); },
      has(o, k) { return Object.prototype.hasOwnProperty.call(o, k) || k in window; },
    });
    return g;
  }
  /* SharedArrayBuffer: WebKit gives a page that is not cross-origin isolated none, and
     realclaudian's bundled SDK allocates new Int32Array(new SharedArrayBuffer(4)) at chunk
     scope (a sleep cell for Atomics.wait on EBUSY retries) -> ReferenceError on send. The
     owners' binding is a plain ArrayBuffer subclass: nothing is shared with any other agent,
     so it is no shared-memory timer; Atomics.wait on it throws TypeError, as the main
     thread's Atomics.wait does anyway. */
  const OwnSAB = class SharedArrayBuffer extends ArrayBuffer {};
  Object.freeze(OwnSAB.prototype); Object.freeze(OwnSAB);

  /* ------------------------------------------- child_process (real, Rust desk_proc.rs)
     spawn / exec / execFile run real programs with the user's rights, as stock does once
     community plugins are on. Rust checks the vault's trust flip on every spawn; the page
     only ever sees an id. cwd "/vault[/x]" (getBasePath) is the real vault dir there.
     env: process.env here is an empty shim, so Rust lays the given keys over the app's real
     environment (what {...process.env, X} gives a stock child). Output is polled
     (desk_proc_read waits up to 250 ms per call) and arrives as Buffers, or strings after
     setEncoding. No *Sync variants and no fork(): those stay the inert stub. */
  const desk = typeof host.desk === "function" ? host.desk : null;
  const deskErr = (e, file, args) => {
    const m = String(e && e.message || e);
    const x = new Error(m.replace(/^(E[A-Z0-9]+): (spawn \S+) .*/, "$2 $1"));
    const c = /^([A-Z][A-Z0-9_]*):/.exec(m); x.code = c ? c[1] : "EIO"; x.errno = -1;
    x.syscall = "spawn " + file; x.path = file; x.spawnargs = args;
    return x;
  };
  class CpReadable extends EmitterBase {
    constructor() { super(); this._enc = null; this._dec = null; this.readable = true; this.destroyed = false; this.readableEnded = false; }
    setEncoding(e) { this._enc = String(e || "utf8").toLowerCase(); this._dec = /^utf-?8$/.test(this._enc) ? new TextDecoder("utf-8") : null; return this; }
    _push(s) {
      if (!s) return;
      const b = Buffer.from(latin1Bytes(s));
      this.emit("data", !this._enc ? b : this._dec ? this._dec.decode(b, { stream: true }) : b.toString(this._enc));
    }
    _end() {
      if (this._dec) { const t = this._dec.decode(); if (t) this.emit("data", t); }
      this.readableEnded = true; this.readable = false;
      this.emit("end"); this.emit("close");
    }
    pipe(d) { this.on("data", c => d.write(c)); this.on("end", () => { if (d && typeof d.end === "function") d.end(); }); return d; }
    pause() { return this; } resume() { return this; } unpipe() { return this; }
    destroy() { this.destroyed = true; return this; }
    ref() { return this; } unref() { return this; }
  }
  class CpWritable extends EmitterBase {
    constructor(cp) { super(); this._cp = cp; this.writable = true; this.destroyed = false; this.writableEnded = false; }
    write(c, enc, cb) {
      if (typeof enc === "function") { cb = enc; enc = undefined; }
      const b = typeof c === "string" ? Buffer.from(c, enc || "utf8") : Buffer.from(c);
      const s = latin1Str(b, 0);
      this._cp._q = this._cp._q.then(id => (id == null ? id : desk("desk_proc_write", { id, data: s }).then(() => id)))
        .then(id => { if (cb) cb(null); return id; }, e => { if (cb) cb(errOf(e)); else this.emit("error", errOf(e)); return null; });
      return true;
    }
    end(c, enc, cb) {
      if (typeof c === "function") { cb = c; c = undefined; } else if (typeof enc === "function") { cb = enc; enc = undefined; }
      if (c != null) this.write(c, enc);
      if (this.writableEnded) return this;
      this.writableEnded = true; this.writable = false;
      this._cp._q = this._cp._q.then(id => (id == null ? id : desk("desk_proc_end", { id }).then(() => id, () => id)))
        .then(id => { if (cb) cb(); this.emit("finish"); this.emit("close"); return id; });
      return this;
    }
    destroy() { this.end(); this.destroyed = true; return this; }
    cork() {} uncork() {}
    setDefaultEncoding() { return this; }
  }
  class ChildProcess extends EmitterBase {
    constructor() {
      super();
      this.pid = undefined; this.exitCode = null; this.signalCode = null; this.killed = false; this.connected = false;
      this.stdin = new CpWritable(this); this.stdout = new CpReadable(); this.stderr = new CpReadable();
      this.stdio = [this.stdin, this.stdout, this.stderr];
      this._q = null;   // resolves to the Rust id (null if the spawn failed)
    }
    kill(sig) {
      if (this.exitCode !== null || this.signalCode !== null) return false;
      const signal = typeof sig === "number" ? ({ 1: "SIGHUP", 2: "SIGINT", 9: "SIGKILL", 15: "SIGTERM" }[sig] || "SIGTERM") : String(sig || "SIGTERM");
      this.killed = true;
      this._q.then(id => (id == null ? null : desk("desk_proc_kill", { id, signal }))).catch(() => {});
      return true;
    }
    ref() { return this; } unref() { return this; }
    disconnect() {}
  }
  function spawnArgs(args, opts) {
    if (!Array.isArray(args)) { opts = args; args = []; }
    return [args.map(String), opts && typeof opts === "object" ? opts : {}];
  }
  function cpSpawn(file, args0, opts0) {
    let [args, opts] = spawnArgs(args0, opts0);
    file = String(file);
    if (opts.shell) { const cmd = [file, ...args].join(" "); file = typeof opts.shell === "string" ? opts.shell : "/bin/sh"; args = ["-c", cmd]; }
    const cp = new ChildProcess();
    cp.spawnfile = file; cp.spawnargs = [file, ...args];
    const st = typeof opts.stdio === "string" ? [opts.stdio, opts.stdio, opts.stdio] : Array.isArray(opts.stdio) ? opts.stdio : [];
    if (st[0] != null && st[0] !== "pipe") cp.stdin = null;
    if (st[1] != null && st[1] !== "pipe") cp.stdout = null;
    if (st[2] != null && st[2] !== "pipe") cp.stderr = null;
    cp.stdio = [cp.stdin, cp.stdout, cp.stderr];
    const out = cp.stdout || new CpReadable(), err = cp.stderr || new CpReadable();
    let env;
    if (opts.env && typeof opts.env === "object") { env = {}; for (const k of Object.keys(opts.env)) if (opts.env[k] != null) env[k] = String(opts.env[k]); }
    const cwd = opts.cwd == null ? undefined : String(opts.cwd instanceof URL ? opts.cwd.pathname : opts.cwd);
    cp._q = (desk ? desk("desk_proc_spawn", { file, args, cwd, env }) : Promise.reject(new Error("EPERM: no desktop backend")))
      .then(r => { cp.pid = r.pid; later(() => cp.emit("spawn")); return r.id; },
        e => { later(() => { cp.emit("error", deskErr(e, file, cp.spawnargs)); cp.emit("close", -2, null); }); return null; });
    let timer = null;
    if (opts.timeout > 0) timer = setTimeout(() => cp.kill(opts.killSignal || "SIGTERM"), Number(opts.timeout));
    if (opts.signal && typeof opts.signal.addEventListener === "function") opts.signal.addEventListener("abort", () => cp.kill(opts.killSignal || "SIGTERM"), { once: true });
    cp._q.then(async id => {
      if (id == null) return;
      for (;;) {
        let c;
        try { c = await desk("desk_proc_read", { id, waitMs: 250 }); } catch (_) { c = { out: "", err: "", exit: null, signal: "SIGKILL", done: true }; }
        out._push(c.out); err._push(c.err);
        if (c.done) { cp.exitCode = c.exit == null ? null : c.exit; cp.signalCode = c.signal || null; break; }
      }
      if (timer) clearTimeout(timer);
      cp.emit("exit", cp.exitCode, cp.signalCode);
      out._end(); err._end();
      cp.emit("close", cp.exitCode, cp.signalCode);
    });
    return cp;
  }
  // execFile/exec: collect both pipes, cb(err, stdout, stderr); err.code = exit code
  function cpRun(file, args, opts, cb, shownCmd) {
    const enc = opts.encoding === undefined ? "utf8" : opts.encoding;
    const cp = cpSpawn(file, args, opts);
    const o = [], e = [];
    if (cp.stdout) cp.stdout.on("data", b => o.push(b));
    if (cp.stderr) cp.stderr.on("data", b => e.push(b));
    const fin = buf => { const b = Buffer.concat(buf); return enc === "buffer" || enc === null ? b : b.toString(enc); };
    let called = false;
    const done = (er, so, se) => { if (called) return; called = true; if (cb) cb(er, so, se); };
    cp.on("error", x => done(x, fin(o), fin(e)));
    cp.on("close", (code, signal) => {
      if (code === -2) return;   // spawn failed: "error" already answered
      const so = fin(o), se = fin(e);
      if (code === 0 && !signal) return done(null, so, se);
      const x = new Error("Command failed: " + shownCmd + (se ? "\n" + se : ""));
      x.code = code; x.killed = cp.killed; x.signal = signal; x.cmd = shownCmd; x.stdout = so; x.stderr = se;
      done(x, so, se);
    });
    return cp;
  }
  function execFile(file, args, opts, cb) {
    if (typeof args === "function") { cb = args; args = []; opts = {}; }
    else if (!Array.isArray(args)) { cb = opts; opts = args; args = []; }
    if (typeof opts === "function") { cb = opts; opts = {}; }
    opts = opts || {};
    return cpRun(String(file), (args || []).map(String), opts, cb, [file, ...(args || [])].join(" "));
  }
  function exec(cmd, opts, cb) {
    if (typeof opts === "function") { cb = opts; opts = {}; }
    opts = opts || {};
    const sh = typeof opts.shell === "string" ? opts.shell : "/bin/sh";
    return cpRun(sh, ["-c", String(cmd)], Object.assign({}, opts, { shell: false }), cb, String(cmd));
  }
  const asPromise = run => function (...a) {
    let cp;
    const p = new Promise((res, rej) => { cp = run(...a, (er, stdout, stderr) => (er ? rej(Object.assign(er, { stdout, stderr })) : res({ stdout, stderr }))); });
    p.child = cp;
    return p;
  };
  Object.defineProperty(exec, PCUSTOM, { value: asPromise(exec) });
  Object.defineProperty(execFile, PCUSTOM, { value: asPromise(execFile) });
  const cpOwn = { spawn: cpSpawn, exec, execFile, ChildProcess };

  /* ------------------------------------------- net / http servers (real, Rust desk_net.rs)
     net.createServer / http.createServer listen on a real TCP port with the user's rights,
     as stock does once community plugins are on (advanced-slides serves its preview, the
     local REST API its endpoints). Rust checks the vault's trust flip on every listen.
     Rust is TCP only; HTTP/1.1 is parsed and written here. One request per connection:
     every response says "Connection: close" and the socket ends after it. No TLS server
     (https.createServer listens nowhere and reports ENOTSUP) and no client sockets yet. */
  const netErr = (e, extra) => {
    const m = String(e && e.message || e), x = new Error(m), c = /^([A-Z_]+):/.exec(m);
    x.code = c ? c[1] : "EIO"; x.errno = -1;
    return Object.assign(x, extra || {});
  };
  const toBuf = (c, enc) => (typeof c === "string" ? Buffer.from(c, enc || "utf8") : Buffer.from(c));
  const argsCb = (c, enc, cb) => (typeof c === "function" ? [undefined, undefined, c] : typeof enc === "function" ? [c, undefined, enc] : [c, enc, cb]);
  class NetSocket extends EmitterBase {
    constructor(id, info) {
      super();
      this._id = id; this._q = Promise.resolve(); this._enc = null; this._dec = null;
      this.remoteAddress = info && info.remoteAddress; this.remotePort = info && info.remotePort; this.remoteFamily = "IPv4";
      this.readable = true; this.writable = true; this.destroyed = false; this.readableEnded = false; this.writableEnded = false;
      this.bytesWritten = 0; this.bytesRead = 0; this.timeout = 0; this.allowHalfOpen = false; this.connecting = false;
    }
    _start() {
      (async () => {
        for (;;) {
          if (this.destroyed) return;
          let d;
          try { d = await desk("desk_sock_read", { id: this._id, waitMs: 250 }); } catch (_) { d = { data: "", eof: true }; }
          if (d.data) {
            const b = Buffer.from(latin1Bytes(d.data)); this.bytesRead += b.length;
            this.emit("data", !this._enc ? b : this._dec ? this._dec.decode(b, { stream: true }) : b.toString(this._enc));
          }
          if (d.eof) break;
        }
        if (this.destroyed) return;
        this.readable = false; this.readableEnded = true; this.emit("end");
        if (!this.allowHalfOpen) this.end();
        this._maybeClose();
      })();
    }
    setEncoding(e) { this._enc = String(e || "utf8").toLowerCase(); this._dec = /^utf-?8$/.test(this._enc) ? new TextDecoder("utf-8") : null; return this; }
    write(c, enc, cb) {
      [c, enc, cb] = argsCb(c, enc, cb);
      if (this.writableEnded || this.destroyed) { const x = Object.assign(new Error("This socket has been ended by the other party"), { code: "EPIPE" }); if (cb) later(() => cb(x)); return false; }
      const b = toBuf(c, enc); this.bytesWritten += b.length;
      const s = latin1Str(b, 0);
      this._q = this._q.then(() => desk("desk_sock_write", { id: this._id, data: s }))
        .then(() => { if (cb) cb(null); }, e => { const x = netErr(e); if (cb) cb(x); else if (this.listenerCount("error")) this.emit("error", x); });
      return true;
    }
    end(c, enc, cb) {
      [c, enc, cb] = argsCb(c, enc, cb);
      if (c != null) this.write(c, enc);
      if (this.writableEnded) { if (cb) later(cb); return this; }
      this.writableEnded = true; this.writable = false;
      this._q = this._q.then(() => desk("desk_sock_end", { id: this._id })).catch(() => {})
        .then(() => { if (cb) cb(); this.emit("finish"); this._maybeClose(); });
      return this;
    }
    _maybeClose() { if (this.readableEnded && this.writableEnded) this._q.then(() => this._close(false)); }
    _close(hadError) {
      if (this._closed) return; this._closed = true;
      this.destroyed = true; this.readable = false; this.writable = false;
      desk("desk_sock_destroy", { id: this._id }).catch(() => {});
      later(() => this.emit("close", !!hadError));
    }
    destroy(err) {
      if (this.destroyed) return this;
      this.writableEnded = true; this.destroyed = true;
      if (err && this.listenerCount("error")) later(() => this.emit("error", errOf(err)));
      this._q.then(() => { this._closed = false; this._close(!!err); });
      return this;
    }
    destroySoon() { this.end(); return this; }
    resetAndDestroy() { return this.destroy(); }
    address() { return { address: "127.0.0.1", family: "IPv4", port: 0 }; }
    setTimeout(ms, cb) { this.timeout = Number(ms) || 0; if (cb) this.once("timeout", cb); return this; }
    setNoDelay() { return this; } setKeepAlive() { return this; }
    pause() { return this; } resume() { return this; } ref() { return this; } unref() { return this; }
    cork() {} uncork() {}
    pipe(d) { this.on("data", c => d.write(c)); this.on("end", () => { if (d && typeof d.end === "function") d.end(); }); return d; }
  }
  class NetServer extends EmitterBase {
    constructor(opts, onConn) {
      super();
      if (typeof opts === "function") { onConn = opts; opts = {}; }
      if (onConn) this.on("connection", onConn);
      this._id = null; this._addr = null; this._socks = new Set(); this._tls = false;
      this.listening = false; this.maxConnections = undefined;
      this.timeout = 0; this.keepAliveTimeout = 5000; this.headersTimeout = 60000; this.requestTimeout = 300000; this.maxHeadersCount = null;
    }
    listen(...a) {
      const cb = typeof a[a.length - 1] === "function" ? a.pop() : null;
      let port = 0, host;
      if (a[0] && typeof a[0] === "object") { port = a[0].port; host = a[0].host; } else { port = a[0]; if (typeof a[1] === "string") host = a[1]; }
      port = Number(port) || 0;
      if (cb) this.once("listening", cb);
      const fail = e => { const x = netErr(e, { syscall: "listen", address: host, port }); if (this.listenerCount("error")) this.emit("error", x); else console.error("[desktop] server listen " + (host || "") + ":" + port + ": " + x.message); };
      if (this._tls) { later(() => fail(new Error("ENOTSUP: TLS servers are not available in opensidian (https.createServer)"))); return this; }
      (desk ? desk("desk_net_listen", { host: host == null ? null : String(host), port }) : Promise.reject(new Error("EPERM: no desktop backend")))
        .then(r => { this._id = r.id; this._addr = { address: r.address, family: "IPv4", port: r.port }; this.listening = true; this.emit("listening"); this._loop(); }, fail);
      return this;
    }
    async _loop() {
      const id = this._id;
      while (this.listening && this._id === id) {
        let b;
        try { b = await desk("desk_net_accept", { id, waitMs: 1000 }); } catch (_) { b = { conns: [], closed: true }; }
        for (const c of b.conns) {
          if (!this.listening) { desk("desk_sock_destroy", { id: c.id }).catch(() => {}); continue; }
          const s = new NetSocket(c.id, c); s.server = this;
          this._socks.add(s); s.once("close", () => this._socks.delete(s));
          this.emit("connection", s); s._start();
        }
        if (b.closed) break;
      }
    }
    close(cb) {
      const was = this.listening; this.listening = false;
      if (this._id != null) desk("desk_net_close", { id: this._id }).catch(() => {});
      this._id = null;
      later(() => {
        if (!was) { const x = Object.assign(new Error("Server is not running."), { code: "ERR_SERVER_NOT_RUNNING" }); if (cb) cb(x); return; }
        if (cb) cb(); this.emit("close");
      });
      return this;
    }
    closeAllConnections() { for (const s of [...this._socks]) s.destroy(); }
    closeIdleConnections() {}
    address() { return this._addr; }
    getConnections(cb) { later(() => cb(null, this._socks.size)); return this; }
    setTimeout(ms, cb) { this.timeout = Number(ms) || 0; if (cb) this.on("timeout", cb); return this; }
    ref() { return this; } unref() { return this; }
  }
  const isIPv4 = s => /^(25[0-5]|2[0-4]\d|1?\d?\d)(\.(25[0-5]|2[0-4]\d|1?\d?\d)){3}$/.test(String(s));
  const isIPv6 = s => { s = String(s); return s.includes(":") && /^[0-9a-fA-F:.]+$/.test(s) && (s.match(/::/g) || []).length <= 1; };
  const netOwn = {
    Server: NetServer, Socket: NetSocket,
    createServer: (o, f) => new NetServer(o, f),
    isIP: s => (isIPv4(s) ? 4 : isIPv6(s) ? 6 : 0), isIPv4, isIPv6,
  };

  // http: a request whose body is buffered until a reader shows up (on("data") / resume / pipe / read)
  class IncomingMessage extends EmitterBase {
    constructor(socket) {
      super();
      this.socket = socket; this.connection = socket;
      this.headers = {}; this.rawHeaders = []; this.trailers = {}; this.rawTrailers = [];
      this.method = null; this.url = ""; this.statusCode = null; this.statusMessage = null;
      this.httpVersion = "1.1"; this.httpVersionMajor = 1; this.httpVersionMinor = 1;
      this.complete = false; this.aborted = false; this.readable = true; this.readableEnded = false; this.destroyed = false;
      this._chunks = []; this._ended = false; this._flowing = false; this._enc = null;
    }
    _push(b) { if (b && b.length) this._chunks.push(b); this._flush(); }
    _end() { this._ended = true; this.complete = true; this._flush(); }
    _flush() {
      if (!this._flowing) { if (this._chunks.length && this.listenerCount("readable")) later(() => this.emit("readable")); return; }
      later(() => {
        while (this._flowing && this._chunks.length) { const b = this._chunks.shift(); this.emit("data", this._enc ? b.toString(this._enc) : b); }
        if (this._ended && !this._chunks.length && !this.readableEnded) { this.readableEnded = true; this.readable = false; this.emit("end"); this.emit("close"); }
      });
    }
    on(n, f) { super.on(n, f); if (n === "data") this.resume(); else if (n === "readable" || n === "end") this._flush(); return this; }
    addListener(n, f) { return this.on(n, f); }
    once(n, f) { super.once(n, f); if (n === "data") this.resume(); return this; }
    read() { if (!this._chunks.length) { if (this._ended && !this.readableEnded) { this.readableEnded = true; later(() => { this.emit("end"); this.emit("close"); }); } return null; } const b = Buffer.concat(this._chunks); this._chunks = []; return this._enc ? b.toString(this._enc) : b; }
    resume() { this._flowing = true; this._flush(); return this; }
    pause() { this._flowing = false; return this; }
    isPaused() { return !this._flowing; }
    setEncoding(e) { this._enc = String(e || "utf8"); return this; }
    pipe(d) { this.on("data", c => d.write(c)); this.on("end", () => { if (d && typeof d.end === "function") d.end(); }); return d; }
    unpipe() { return this; }
    unshift(b) { this._chunks.unshift(toBuf(b)); }
    destroy(err) { this.destroyed = true; if (this.socket) this.socket.destroy(err); return this; }
    setTimeout(ms, cb) { if (this.socket) this.socket.setTimeout(ms, cb); return this; }
    [Symbol.asyncIterator]() {
      const q = [], w = []; let done = false;
      this.on("data", c => (w.length ? w.shift()({ value: c, done: false }) : q.push(c)));
      this.on("end", () => { done = true; while (w.length) w.shift()({ value: undefined, done: true }); });
      return { next: () => (q.length ? Promise.resolve({ value: q.shift(), done: false }) : done ? Promise.resolve({ value: undefined, done: true }) : new Promise(r => w.push(r))),
        return: () => Promise.resolve({ value: undefined, done: true }), [Symbol.asyncIterator]() { return this; } };
    }
  }
  const NOBODY = new Set([204, 304]);
  class ServerResponse extends EmitterBase {
    constructor(req) {
      super();
      this.req = req; this.socket = req.socket; this.connection = req.socket;
      this.statusCode = 200; this.statusMessage = undefined; this.sendDate = true; this.strictContentLength = false;
      this.headersSent = false; this.finished = false; this.writableEnded = false; this.writableFinished = false; this.writable = true; this.destroyed = false;
      this._h = new Map();
    }
    _chk() { if (this.headersSent) throw Object.assign(new Error("Cannot set headers after they are sent to the client"), { code: "ERR_HTTP_HEADERS_SENT" }); }
    setHeader(n, v) { this._chk(); this._h.set(String(n).toLowerCase(), [String(n), v]); return this; }
    appendHeader(n, v) { const k = String(n).toLowerCase(), e = this._h.get(k); return this.setHeader(e ? e[0] : n, e ? [].concat(e[1], v) : v); }
    getHeader(n) { const e = this._h.get(String(n).toLowerCase()); return e ? e[1] : undefined; }
    getHeaders() { const o = Object.create(null); for (const [k, e] of this._h) o[k] = e[1]; return o; }
    getHeaderNames() { return [...this._h.keys()]; }
    getRawHeaderNames() { return [...this._h.values()].map(e => e[0]); }
    hasHeader(n) { return this._h.has(String(n).toLowerCase()); }
    removeHeader(n) { this._chk(); this._h.delete(String(n).toLowerCase()); }
    writeHead(code, msg, hdrs) {
      if (typeof msg !== "string") { hdrs = msg; msg = undefined; }
      this.statusCode = Number(code); if (msg) this.statusMessage = msg;
      if (Array.isArray(hdrs)) { for (let i = 0; i + 1 < hdrs.length; i += 2) this.setHeader(hdrs[i], hdrs[i + 1]); }
      else if (hdrs) for (const k of Object.keys(hdrs)) this.setHeader(k, hdrs[k]);
      this._head();
      return this;
    }
    _head() {
      if (this.headersSent) return;
      if (this.sendDate && !this._h.has("date")) this._h.set("date", ["Date", new Date().toUTCString()]);
      this._h.set("connection", ["Connection", "close"]);
      const clean = v => String(v).replace(/[\r\n]+/g, " ");
      let s = "HTTP/1.1 " + this.statusCode + " " + clean(this.statusMessage || HTTP_STATUS[this.statusCode] || "unknown") + "\r\n";
      for (const [, [n, v]] of this._h) for (const x of [].concat(v)) s += clean(n) + ": " + clean(x) + "\r\n";
      this.headersSent = true;
      this.socket.write(s + "\r\n", "utf8");
    }
    flushHeaders() { this._head(); }
    _body() { return this.req.method !== "HEAD" && !NOBODY.has(this.statusCode); }
    write(c, enc, cb) {
      [c, enc, cb] = argsCb(c, enc, cb);
      if (this.writableEnded) { const x = Object.assign(new Error("write after end"), { code: "ERR_STREAM_WRITE_AFTER_END" }); if (cb) later(() => cb(x)); else later(() => this.emit("error", x)); return false; }
      this._head();
      if (c != null && this._body()) this.socket.write(toBuf(c, enc), undefined, cb); else if (cb) later(cb);
      return true;
    }
    end(c, enc, cb) {
      [c, enc, cb] = argsCb(c, enc, cb);
      if (this.writableEnded) { if (cb) later(cb); return this; }
      if (c != null && !this.headersSent && !this._h.has("content-length") && !this._h.has("transfer-encoding")) this.setHeader("Content-Length", toBuf(c, enc).length);
      if (c != null) this.write(c, enc); else this._head();
      this.writableEnded = true; this.finished = true; this.writable = false;
      this.socket.end(() => { this.writableFinished = true; if (cb) cb(); this.emit("finish"); this.emit("close"); });
      return this;
    }
    writeContinue() {} writeProcessing() {} writeEarlyHints() {}
    addTrailers() {}
    cork() {} uncork() {}
    setTimeout(ms, cb) { this.socket.setTimeout(ms, cb); return this; }
    destroy(err) { this.destroyed = true; this.socket.destroy(err); return this; }
    assignSocket() {} detachSocket() {}
  }
  // the HTTP/1.1 request head + body (Content-Length or chunked) off one connection
  function httpParse(server, sock) {
    let buf = "", req = null, need = -1, chunked = false, body = "";
    const bad = () => { sock.end("HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"); };
    const dechunk = () => {   // consume complete chunks of body; true once the last chunk arrived
      for (;;) {
        const i = body.indexOf("\r\n"); if (i < 0) return false;
        const n = parseInt(body.slice(0, i), 16); if (!(n >= 0)) { bad(); return false; }
        if (n === 0) return true;
        if (body.length < i + 2 + n + 2) return false;
        req._push(Buffer.from(latin1Bytes(body.slice(i + 2, i + 2 + n)))); body = body.slice(i + 2 + n + 2);
      }
    };
    sock.on("data", b => {
      if (req && req.complete) return;   // one request per connection
      buf += latin1Str(b, 0);
      if (!req) {
        const h = buf.indexOf("\r\n\r\n");
        if (h < 0) { if (buf.length > 65536) bad(); return; }
        const lines = buf.slice(0, h).split("\r\n"), m = /^([A-Z]+) (\S+) HTTP\/(\d)\.(\d)$/.exec(lines[0]);
        if (!m) return bad();
        req = new server._Req(sock);
        req.method = m[1]; req.url = m[2]; req.httpVersionMajor = +m[3]; req.httpVersionMinor = +m[4]; req.httpVersion = m[3] + "." + m[4];
        for (const l of lines.slice(1)) {
          const c = l.indexOf(":"); if (c <= 0) continue;
          const n = l.slice(0, c).trim(), v = new TextDecoder("utf-8").decode(latin1Bytes(l.slice(c + 1).trim())), k = n.toLowerCase();
          req.rawHeaders.push(n, v);
          if (k === "set-cookie") (req.headers[k] = req.headers[k] || []).push(v);
          else req.headers[k] = k in req.headers ? req.headers[k] + ", " + v : v;
        }
        chunked = /chunked/i.test(req.headers["transfer-encoding"] || "");
        need = chunked ? -1 : Number(req.headers["content-length"] || 0) || 0;
        buf = buf.slice(h + 4);
        const res = new server._Res(req);
        later(() => server.emit("request", req, res));
      }
      if (chunked) { body += buf; buf = ""; if (dechunk()) req._end(); return; }
      if (buf) { const take = buf.slice(0, need); need -= take.length; buf = buf.slice(take.length); req._push(Buffer.from(latin1Bytes(take))); }
      if (need <= 0) req._end();
    });
    sock.on("end", () => { if (req && !req.complete) { req.aborted = true; req._end(); } });
  }
  class HttpServer extends NetServer {
    constructor(opts, onReq) {
      super();
      if (typeof opts === "function") { onReq = opts; opts = {}; }
      opts = opts || {};
      this._Req = typeof opts.IncomingMessage === "function" ? opts.IncomingMessage : IncomingMessage;
      this._Res = typeof opts.ServerResponse === "function" ? opts.ServerResponse : ServerResponse;
      if (onReq) this.on("request", onReq);
      this.on("connection", s => httpParse(this, s));
    }
  }
  class HttpsServer extends HttpServer {
    constructor(opts, onReq) { super(opts, onReq); this._tls = true; }
  }
  const httpOwn = {
    METHODS: HTTP_METHODS, STATUS_CODES: HTTP_STATUS,
    Server: HttpServer, IncomingMessage, ServerResponse,
    createServer: (o, f) => new HttpServer(o, f),
    // the client (top10-func): ClientRequest over desk_http, made at call time (defined further down)
    request: (a, b, c) => clientRequest("http")(a, b, c), get: (a, b, c) => clientGet("http")(a, b, c),
    maxHeaderSize: 16384,
  };
  const httpsOwn = {
    METHODS: HTTP_METHODS, STATUS_CODES: HTTP_STATUS,
    Server: HttpsServer,
    createServer: (o, f) => new HttpsServer(o, f),
    request: (a, b, c) => clientRequest("https")(a, b, c), get: (a, b, c) => clientGet("https")(a, b, c),
  };

  /* ------------------------------------------------------ the view */
  // a frozen module object whose named members are real; any other member is the inert
  // deny stub (node-modules.js stub(), same message as the loader's). Members are OWN keys:
  // bundled __toESM(require("x")) copies own keys only.
  function mod(name, deny, own) {
    const s = host.nodeModules && host.nodeModules.stub(name, deny);
    const t = Object.create(null);
    for (const k of Object.keys(own)) Object.defineProperty(t, k, { value: own[k], enumerable: true, writable: false, configurable: false });
    Object.freeze(t);
    return new Proxy(t, {
      get(o, k) { if (Object.prototype.hasOwnProperty.call(o, k)) return o[k]; if (typeof k === "symbol" || k === "then") return undefined; return s ? s[k] : undefined; },
      has(o, k) { return Object.prototype.hasOwnProperty.call(o, k); },
    });
  }
  const utilAll = Object.freeze(Object.assign({ TextDecoder, TextEncoder,
    types: Object.freeze({ isPromise: v => v instanceof Promise, isDate: v => v instanceof Date, isRegExp: v => v instanceof RegExp, isUint8Array: v => v instanceof Uint8Array }),
    isDeepStrictEqual: (a, b) => { try { return JSON.stringify(a) === JSON.stringify(b); } catch (_) { return a === b; } } }, slidesUtilOwn));
  // forge (and others) feature-test keygen / pbkdf2 with typeof === "function": undefined
  // sends them down their own pure-JS path instead of calling a deny stub
  const cryptoAll = Object.freeze(Object.assign({ generateKeyPair: undefined, generateKeyPairSync: undefined, pbkdf2: undefined, pbkdf2Sync: undefined }, cryptoOwn));
  const qsMod = Object.freeze({
    parse: s => qsObj(String(s == null ? "" : s)),
    stringify: o => { const p = new URLSearchParams(); for (const k of Object.keys(o || {})) for (const v of [].concat(o[k])) p.append(k, String(v)); return p.toString(); },
    escape: s => encodeURIComponent(String(s)), unescape: s => decodeURIComponent(String(s)),
  });
  const timersMod = Object.freeze({ setTimeout: (...a) => window.setTimeout(...a), clearTimeout: t => window.clearTimeout(t),
    setInterval: (...a) => window.setInterval(...a), clearInterval: t => window.clearInterval(t),
    setImmediate: (f, ...a) => window.setTimeout(f, 0, ...a), clearImmediate: t => window.clearTimeout(t) });

  /* ------------------------------------------- obsidian request / requestUrl (Rust desk_http.rs)
     Stock gives plugins an HTTP client with no CORS: any host, redirects followed, the
     response whatever its status (requestUrl throws on >= 400 unless throw: false).
     A trusted vault's plugins get that here; the shared API's placeholder stays for the
     rest. http:// only for now: https:// fails with ENOTSUP (no TLS client in the app).
     Bodies cross as latin1 strings (one char per byte). */
  const toBytes = b => {
    if (b == null) return null;
    if (typeof b === "string") return new TextEncoder().encode(b);
    if (b instanceof ArrayBuffer) return new Uint8Array(b);
    if (ArrayBuffer.isView(b)) return new Uint8Array(b.buffer, b.byteOffset, b.byteLength);
    return new TextEncoder().encode(String(b));
  };
  function httpSend(p) {
    const o = typeof p === "string" ? { url: p } : (p && typeof p === "object" ? p : {});
    const headers = [];
    if (o.headers && typeof o.headers === "object") for (const k of Object.keys(o.headers)) if (o.headers[k] != null) headers.push([String(k), String(o.headers[k])]);
    if (o.contentType && !headers.some(([k]) => k.toLowerCase() === "content-type")) headers.push(["Content-Type", String(o.contentType)]);
    const bytes = toBytes(o.body);
    const req = { url: String(o.url || ""), method: String(o.method || "GET").toUpperCase(), headers };
    if (bytes) req.body = latin1Str(bytes, 0);
    if (!desk) return Promise.reject(new Error("request: no desktop bridge"));
    return desk("desk_http_request", { req }).then(r => {
      const body = latin1Bytes(r.body || "");
      const hs = {};
      for (const [k, v] of r.headers || []) hs[k] = Object.prototype.hasOwnProperty.call(hs, k) ? hs[k] + ", " + v : v;
      return { status: r.status, headers: hs, body };
    }, e => { throw new Error("net::" + String(e && e.message || e)); });
  }
  const httpFail = r => Object.assign(new Error("Request failed, status " + r.status), { status: r.status, headers: r.headers });
  function requestUrl(p) {
    const thr = !(p && typeof p === "object" && p.throw === false);
    const pr = httpSend(p).then(r => {
      if (thr && r.status >= 400) throw httpFail(r);
      let text = null, json;
      const buf = r.body.buffer.slice(r.body.byteOffset, r.body.byteOffset + r.body.byteLength);
      return {
        status: r.status, headers: r.headers, arrayBuffer: buf,
        get text() { if (text === null) text = new TextDecoder("utf-8").decode(r.body); return text; },
        get json() { if (json === undefined) json = JSON.parse(this.text); return json; },
      };
    });
    // .arrayBuffer / .json / .text are made on first read: an eager pr.then() per member
    // is a second promise nobody holds, and when the request fails it rejects unhandled
    // even though the caller awaited pr inside a try (Copilot's update check)
    for (const k of ["arrayBuffer", "json", "text"]) {
      let d = null;
      Object.defineProperty(pr, k, { enumerable: true, configurable: false, get() { if (!d) d = pr.then(x => x[k]); return d; } });
    }
    return pr;
  }
  const request = p => httpSend(p).then(r => { if (r.status >= 400) throw httpFail(r); return new TextDecoder("utf-8").decode(r.body); });
  // the plugin's "obsidian": the shared API with stock's request / requestUrl laid over
  const apis = new WeakMap();
  function obsidianApi(base) {
    if (!base || typeof base !== "object" || !desk) return base;
    let a = apis.get(base);
    if (!a) {
      const d = Object.getOwnPropertyDescriptors(base);
      d.request = { value: request, enumerable: true, writable: false, configurable: false };
      d.requestUrl = { value: requestUrl, enumerable: true, writable: false, configurable: false };
      a = Object.defineProperties(Object.create(Object.getPrototypeOf(base)), d);
      if (Object.isFrozen(base)) Object.freeze(a);
      apis.set(base, a);
    }
    return a;
  }

  /* ------------------------------------------- fs *Sync (stock parity: copilot, slides)
     Node's synchronous fs over the same confined core as the async facade
     (vaultfs plugin_op). An invoke is always async, so each call is one
     same-origin synchronous XHR to the app's own scheme, answered in Rust by
     main.rs vfs_sync (trusted vault only; connect-src 'self' exists only in a
     trusted page's CSP). Paths are the facade's: "/vault/x" and "x" are one file,
     anything outside the vault is refused EPERM by Rust. */
  const VFS_SYNC = "/__os_vfs_sync";
  const sysErr = (msg, p) => {
    const V = host.vfs;
    if (V && V.vfsError) return V.vfsError(msg, p);
    const e = new Error(String(msg)); e.code = "EIO"; e.path = String(p); return e;
  };
  function vioSync(op, p, a) {
    const V = host.vfs;
    if (!V) throw sysErr("EPERM: plugin file access is not available", p);
    const body = Object.assign({ op, path: V.vrel(p) }, a || {});
    if (body.to != null) body.to = V.vrel(body.to);
    const x = new XMLHttpRequest();
    x.open("POST", location.origin + VFS_SYNC, false);
    x.setRequestHeader("Content-Type", "text/plain;charset=UTF-8");
    try { x.send(JSON.stringify(body)); } catch (e) { throw sysErr("EIO: synchronous fs request failed (" + String(e && e.message || e) + ")", p); }
    let r = null;
    try { r = JSON.parse(x.responseText); } catch (_) { /* below */ }
    if (!r) throw sysErr("EIO: synchronous fs answered " + x.status, p);
    if ("err" in r) throw sysErr(r.err, p);
    return r.ok;
  }
  const b64bytes = s => { const b = atob(String(s)), u = new Uint8Array(b.length); for (let i = 0; i < b.length; i++) u[i] = b.charCodeAt(i); return u; };
  const bytesB64 = d => {
    const u = typeof d === "string" ? new TextEncoder().encode(d) : ArrayBuffer.isView(d) ? new Uint8Array(d.buffer, d.byteOffset, d.byteLength) : d instanceof ArrayBuffer ? new Uint8Array(d) : new TextEncoder().encode(String(d == null ? "" : d));
    let s = "";
    for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode.apply(null, u.subarray(i, i + 0x8000));
    return btoa(s);
  };
  const encArg = o => (typeof o === "string" ? o : o && typeof o === "object" ? o.encoding || null : null);
  class SyncStats {
    constructor(s) {
      const dir = s.type === "folder";
      // no inode, owner or mode bits exist behind the vault core: fixed, plausible values
      Object.assign(this, { dev: 0, ino: 0, mode: dir ? 0o40755 : 0o100644, nlink: 1, uid: 0, gid: 0, rdev: 0, size: s.size, blksize: 4096, blocks: Math.ceil(s.size / 512),
        atimeMs: s.mtime, mtimeMs: s.mtime, ctimeMs: s.ctime, birthtimeMs: s.ctime,
        atime: new Date(s.mtime), mtime: new Date(s.mtime), ctime: new Date(s.ctime), birthtime: new Date(s.ctime) });
      Object.defineProperty(this, "_dir", { value: dir });
    }
    isFile() { return !this._dir; }
    isDirectory() { return this._dir; }
    isSymbolicLink() { return false; }
    isBlockDevice() { return false; }
    isCharacterDevice() { return false; }
    isFIFO() { return false; }
    isSocket() { return false; }
  }
  class SyncDirent {
    constructor(name, kind, parent) { this.name = name; this.parentPath = this.path = parent; Object.defineProperty(this, "_k", { value: kind }); }
    isFile() { return this._k === "file"; }
    isDirectory() { return this._k === "folder"; }
    isSymbolicLink() { return false; }
    isBlockDevice() { return false; }
    isCharacterDevice() { return false; }
    isFIFO() { return false; }
    isSocket() { return false; }
  }
  const noent = p => { const e = new Error("ENOENT: no such file or directory, '" + p + "'"); e.code = "ENOENT"; e.path = String(p); return e; };
  function statS(p, o) {
    try { return new SyncStats(vioSync("stat", p)); } catch (e) { if (e.code === "ENOENT" && o && o.throwIfNoEntry === false) return undefined; throw e; }
  }
  function rmTreeS(p) {
    for (const [n, k] of vioSync("list", p)) {
      const c = String(p).replace(/\/+$/, "") + "/" + n;
      if (k === "folder") { rmTreeS(c); vioSync("rmdir", c); } else vioSync("unlink", c);
    }
  }
  /* git-chmod: fs.chmod / chmodSync / promises.chmod (obsidian-git marks its askpass helper
     executable). Trusted vault only: it rides the sync route, and Rust (vaultfs plugin_chmod)
     confines the path, refuses a symlink and masks the mode to 0o777. fchmod / lchmod stay
     denied (no descriptors, and a link's own mode is never ours to set). Mode as in Node: an
     integer or an octal string; anything else is ERR_INVALID_ARG_TYPE / ERR_INVALID_ARG_VALUE. */
  function modeArg(m) {
    if (typeof m === "string" && /^[0-7]+$/.test(m)) m = parseInt(m, 8);
    if (typeof m !== "number") { const e = new TypeError('The "mode" argument must be a 32-bit unsigned integer or an octal string'); e.code = "ERR_INVALID_ARG_TYPE"; throw e; }
    if (!Number.isInteger(m) || m < 0 || m > 0xFFFFFFFF) { const e = new RangeError('The value of "mode" is out of range. Received ' + String(m)); e.code = "ERR_INVALID_ARG_VALUE"; throw e; }
    return m;
  }
  const chmodS = (p, m) => { vioSync("chmod", p, { mode: modeArg(m) }); };
  const chmodP = (p, m) => { try { return Promise.resolve(chmodS(p, m)); } catch (e) { return Promise.reject(e); } };
  const chmodCb = function chmod(p, m, cb) {
    if (typeof cb !== "function") throw new TypeError('The "cb" argument must be of type function');
    chmodP(p, m).then(() => cb(null), e => cb(e));
  };
  const fsSync = Object.freeze({
    existsSync(p) { try { return p != null && vioSync("exists", p) === true; } catch (_) { return false; } },
    chmodSync: chmodS,
    statSync: statS,
    lstatSync: statS,
    accessSync(p) { if (!vioSync("exists", p)) throw noent(p); },
    readFileSync(p, o) {
      const u = b64bytes(vioSync("read", p)), enc = encArg(o);
      return enc ? Buffer.from(u).toString(enc) : Buffer.from(u);
    },
    writeFileSync(p, d, o) { const enc = encArg(o); vioSync("write", p, { data: bytesB64(typeof d === "string" && enc && !/^utf-?8$/i.test(enc) ? Buffer.from(d, enc) : d) }); },
    appendFileSync(p, d) { vioSync("append", p, { data: bytesB64(d) }); },
    readdirSync(p, o) {
      const l = vioSync("list", p);
      return o && typeof o === "object" && o.withFileTypes ? l.map(([n, k]) => new SyncDirent(n, k, String(p))) : l.map(([n]) => n);
    },
    mkdirSync(p, o) {
      const rec = !!(o && typeof o === "object" ? o.recursive : false);
      if (rec && vioSync("exists", p)) return undefined;
      vioSync("mkdir", p, { recursive: rec });
      return rec ? "/vault/" + host.vfs.vrel(p) : undefined;
    },
    renameSync(a, b) { vioSync("rename", a, { to: String(b) }); },
    copyFileSync(a, b, mode) {
      if ((mode | 0) & 1 && vioSync("exists", b)) { const e = new Error("EEXIST: file already exists, '" + b + "'"); e.code = "EEXIST"; e.path = String(b); throw e; }
      vioSync("copy", a, { to: String(b) });
    },
    unlinkSync(p) { vioSync("unlink", p); },
    rmdirSync(p) { vioSync("rmdir", p); },
    rmSync(p, o) {
      o = o || {};
      let s;
      try { s = vioSync("stat", p); } catch (e) { if (e.code === "ENOENT" && o.force) return; throw e; }
      if (s.type === "folder") {
        if (!o.recursive) { const e = new Error("ERR_FS_EISDIR: Path is a directory: rm returned EISDIR, '" + p + "'"); e.code = "ERR_FS_EISDIR"; throw e; }
        rmTreeS(p); vioSync("rmdir", p);
      } else vioSync("unlink", p);
    },
  });
  // the fs facade of node-modules.js (not fs/promises): it has readFile AND a promises member
  const isFsFacade = m => m && typeof m === "object" && typeof m.readFile === "function" && m.promises && typeof m.promises === "object";
  // fs/promises of node-modules.js: readFile and the chmod stub, no promises member and no *Sync
  const isFsPromises = m => m && typeof m === "object" && typeof m.readFile === "function" && typeof m.chmod === "function" && !("promises" in m) && !("readFileSync" in m);


  /* ------------------------------------------- top10-func (Copilot's rows, coverage.json)
     fs descriptors (openSync / readSync / writeSync / closeSync / fstatSync, their callback forms and
     fs.promises.open's FileHandle), realpath, readlink, read / write streams, the stream module, zlib
     over the page's CompressionStream / DecompressionStream, and worker_threads' main-thread view.
     All over the SAME confined core as the facade: a descriptor is a vault path + flags + a position
     held here (no host descriptor exists), every read / write is one vault op that Rust confines and
     refuses on a symlink. Link creation (symlink / link) is REFUSED BY POLICY (operator ruling: fs
     access vault-confined and symlink-safe): a symlink's target is a host path the vault cannot
     confine, and a hard link aliases a vault file's inode, so a write through the vault name lands
     in a file whose other name may be (or later move) outside the vault. */
  const sysE = (code, msg, p, syscall) => { const e = new Error(code + ": " + msg + (p != null ? ", " + (syscall ? syscall + " " : "") + "'" + p + "'" : "")); e.code = code; if (p != null) e.path = String(p); if (syscall) e.syscall = syscall; e.errno = -1; return e; };
  const pathArg = p => {
    if (p instanceof URL) { if (!host.vfsUrl) throw sysE("EINVAL", "no vault url module", String(p)); return host.vfsUrl.fileURLToPath(p); }
    if (ArrayBuffer.isView(p)) return new TextDecoder().decode(p);
    if (typeof p !== "string") { const e = new TypeError('The "path" argument must be of type string or an instance of Buffer or URL. Received ' + (p === null ? "null" : typeof p)); e.code = "ERR_INVALID_ARG_TYPE"; throw e; }
    return p;
  };
  const policyLink = n => function () {
    const e = new Error("EPERM: fs." + n + " is refused by policy in opensidian: plugins may not create links in a symlink-safe, vault-confined fs");
    e.code = "EPERM"; e.syscall = n.replace(/Sync$/, ""); e.errno = -1;
    if (/Sync$/.test(n)) throw e;
    const cb = arguments[arguments.length - 1];
    if (typeof cb === "function") { later(() => cb(e)); return undefined; }
    return Promise.reject(e);
  };
  // the canonical vault path: "/vault" + the vault-relative path, no "." / empty segments
  function realS(p) {
    p = pathArg(p);
    vioSync("stat", p);                     // ENOENT when missing, EPERM outside the vault or on a symlink
    const r = host.vfs.vrel(p).split("/").filter(s => s && s !== ".").join("/");
    return r ? host.vfs.VROOT + "/" + r : host.vfs.VROOT;
  }
  function realpathSync(p) { return realS(p); }
  realpathSync.native = realpathSync;
  // nothing reachable through the confined fs is a link (Rust refuses a symlink path EPERM): an
  // existing vault path is "not a symbolic link" -> EINVAL, as Node says for a regular file
  function readlinkS(p) { p = pathArg(p); vioSync("stat", p); throw sysE("EINVAL", "invalid argument", p, "readlink"); }
  const cbOf = a => { const cb = a[a.length - 1]; if (typeof cb !== "function") throw new TypeError('The "cb" argument must be of type function'); return cb; };
  const asCb = f => function (...a) { const cb = cbOf(a); a.pop(); let v; try { v = f(...a); } catch (e) { later(() => cb(e)); return; } later(() => cb(null, v)); };
  const asP = f => (...a) => { try { return Promise.resolve(f(...a)); } catch (e) { return Promise.reject(e); } };

  /* descriptors: flags as Node's ("r" "r+" "w" "wx" "w+" "a" "ax" "a+" ..., or O_* bits) */
  const fds = new Map();
  let fdNext = 20;
  function flagsOf(f) {
    if (f == null) f = "r";
    if (typeof f === "number") {
      const acc = f & 3;
      return { rd: acc !== 1, wr: acc !== 0, create: !!(f & 64), excl: !!(f & 128), trunc: !!(f & 512), append: !!(f & 1024) };
    }
    const m = /^(r|rs|sr|w|a)(x?)(\+?)(x?)$/.exec(String(f).replace(/s(?=\+)/, ""));
    if (!m) { const e = new TypeError("The value \"" + String(f) + "\" is invalid for option \"flags\""); e.code = "ERR_INVALID_ARG_VALUE"; throw e; }
    const k = m[1][0] === "s" ? "r" : m[1][0], plus = !!m[3], x = !!(m[2] || m[4]);
    if (k === "r") return { rd: true, wr: plus, create: false, excl: false, trunc: false, append: false };
    if (k === "w") return { rd: plus, wr: true, create: true, excl: x, trunc: true, append: false };
    return { rd: plus, wr: true, create: true, excl: x, trunc: false, append: true };
  }
  function openS(p, flags, mode) {
    p = pathArg(p);
    const fl = flagsOf(flags);
    const there = vioSync("exists", p) === true;
    if (there && fl.excl) throw sysE("EEXIST", "file already exists", p, "open");
    if (there) {
      const s = vioSync("stat", p);
      if (s.type === "folder" && fl.wr) throw sysE("EISDIR", "illegal operation on a directory", p, "open");
      if (fl.trunc) vioSync("write", p, { data: "" });
    } else {
      if (!fl.create) vioSync("stat", p);   // throws ENOENT (or EPERM outside the vault)
      vioSync("write", p, { data: "" });
      if (mode != null) vioSync("chmod", p, { mode: modeArg(mode) & 0o777 });
    }
    const fd = fdNext++;
    fds.set(fd, { path: p, fl, pos: 0 });
    return fd;
  }
  const fdOf = (fd, syscall) => { const d = fds.get(fd); if (!d) throw sysE("EBADF", "bad file descriptor", null, syscall); return d; };
  const fileBytes = p => b64bytes(vioSync("read", p));
  function readS(fd, buf, off, len, pos) {
    const d = fdOf(fd, "read");
    if (!d.fl.rd) throw sysE("EBADF", "bad file descriptor", null, "read");
    if (!ArrayBuffer.isView(buf)) { const e = new TypeError('The "buffer" argument must be an instance of Buffer, TypedArray, or DataView'); e.code = "ERR_INVALID_ARG_TYPE"; throw e; }
    if (off && typeof off === "object") ({ offset: off, length: len, position: pos } = off);
    off = off == null ? 0 : off | 0;
    const view = new Uint8Array(buf.buffer, buf.byteOffset, buf.byteLength);
    len = len == null ? view.length - off : len | 0;
    if (off < 0 || len < 0 || off + len > view.length) { const e = new RangeError('The value of "length" is out of range'); e.code = "ERR_OUT_OF_RANGE"; throw e; }
    const at = pos == null || pos === -1 || pos === -1n ? d.pos : Number(pos);
    const all = fileBytes(d.path);
    const part = all.subarray(Math.min(at, all.length), Math.min(at + len, all.length));
    view.set(part, off);
    if (pos == null || pos === -1) d.pos = at + part.length;
    return part.length;
  }
  function writeS(fd, data, a, b, c) {
    const d = fdOf(fd, "write");
    if (!d.fl.wr) throw sysE("EBADF", "bad file descriptor", null, "write");
    let bytes, pos;
    if (typeof data === "string") { bytes = Buffer.from(data, typeof b === "string" ? b : "utf8"); pos = a; }
    else if (ArrayBuffer.isView(data)) {
      const u = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
      if (a && typeof a === "object") ({ offset: a, length: b, position: c } = a);
      const off = a == null ? 0 : a | 0, len = b == null ? u.length - off : b | 0;
      bytes = u.subarray(off, off + len); pos = c;
    } else { const e = new TypeError('The "buffer" argument must be of type string or an instance of Buffer, TypedArray, or DataView'); e.code = "ERR_INVALID_ARG_TYPE"; throw e; }
    if (d.fl.append) { vioSync("append", d.path, { data: bytesB64(bytes) }); return bytes.length; }
    const at = pos == null ? d.pos : Number(pos), cur = fileBytes(d.path);
    const out = new Uint8Array(Math.max(cur.length, at + bytes.length));
    out.set(cur); out.set(bytes, at);
    vioSync("write", d.path, { data: bytesB64(out) });
    if (pos == null) d.pos = at + bytes.length;
    return bytes.length;
  }
  function closeS(fd) { fdOf(fd, "close"); fds.delete(fd); }
  function fstatS(fd) { return statS(fdOf(fd, "fstat").path); }
  // readFileSync / writeFileSync / appendFileSync accept a descriptor as well as a path
  const fdOrPath = (pf, ff) => function (p, ...a) { return typeof p === "number" ? ff(p, ...a) : pf(p, ...a); };
  const readFileFd = (fd, o) => { const d = fdOf(fd, "read"), u = fileBytes(d.path).subarray(d.pos); d.pos += u.length; const enc = encArg(o), b = Buffer.from(u); return enc ? b.toString(enc) : b; };
  const writeFileFd = (fd, data, o) => { writeS(fd, typeof data === "string" ? Buffer.from(data, encArg(o) || "utf8") : data); };
  class FileHandle extends EmitterBase {
    constructor(fd) { super(); this.fd = fd; }
    async read(buf, off, len, pos) { if (buf == null) buf = Buffer.alloc(16384); return { bytesRead: readS(this.fd, buf, off, len, pos), buffer: buf }; }
    async write(data, a, b, c) { return { bytesWritten: writeS(this.fd, data, a, b, c), buffer: data }; }
    async readFile(o) { return readFileFd(this.fd, o); }
    async writeFile(data, o) { writeFileFd(this.fd, data, o); }
    async appendFile(data, o) { writeFileFd(this.fd, data, o); }
    async stat() { return fstatS(this.fd); }
    async truncate(n) { const d = fdOf(this.fd, "ftruncate"), u = fileBytes(d.path), out = new Uint8Array(n | 0); out.set(u.subarray(0, n | 0)); vioSync("write", d.path, { data: bytesB64(out) }); }
    async sync() { fdOf(this.fd, "fsync"); }
    async close() { if (fds.has(this.fd)) { closeS(this.fd); this.emit("close"); } }
  }

  /* ------------------------------------------- stream (Readable / Writable / Duplex / Transform /
     PassThrough, pipe, pipeline, finished, async iteration). Push-driven and in-page; the
     buffer is unbounded (write() reports backpressure past highWaterMark, a Readable is not
     throttled by its consumer's pace). */
  const tick = f => Promise.resolve().then(f);
  const sErr = (code, msg) => { const e = new Error(msg); e.code = code; return e; };
  const abortErr = () => { const e = new Error("The operation was aborted"); e.name = "AbortError"; e.code = "ABORT_ERR"; return e; };
  const chunkLen = (s, c) => (s.objectMode ? 1 : c && c.length != null ? c.length : 1);
  function destroyS(self, err, cb) {
    if (self.destroyed) { if (cb) cb(err); return self; }
    self.destroyed = true;
    const fin = e => { tick(() => { if (e) { self.errored = e; self.emit("error", e); } self.closed = true; self.emit("close"); if (cb) cb(e); }); };
    if (typeof self._destroy === "function") { try { self._destroy(err || null, e => fin(e || err)); } catch (e) { fin(e); } } else fin(err);
    return self;
  }
  function initR(self, o) {
    self._rs = { buf: [], flowing: null, ended: false, endEmitted: false, sched: false, reading: false,
      objectMode: !!(o.objectMode || o.readableObjectMode), enc: o.encoding || null };
    if (typeof o.read === "function") self._read = o.read;
    if (typeof o.destroy === "function") self._destroy = o.destroy;
    self.readable = true; self.readableEnded = false;
  }
  function initW(self, o) {
    self._ws = { q: [], writing: false, ending: false, finishing: false, len: 0, needDrain: false,
      objectMode: !!(o.objectMode || o.writableObjectMode), decode: o.decodeStrings !== false, hwm: o.highWaterMark != null ? o.highWaterMark : (o.objectMode ? 16 : 16384) };
    if (typeof o.write === "function") self._write = o.write;
    if (typeof o.final === "function") self._final = o.final;
    if (typeof o.destroy === "function") self._destroy = o.destroy;
    self.writable = true; self.writableEnded = false; self.writableFinished = false;
  }
  function abortOn(self, sig) { if (!sig) return; const f = () => destroyS(self, abortErr()); if (sig.aborted) tick(f); else sig.addEventListener("abort", f, { once: true }); }
  class Stream extends EmitterBase {
    constructor() { super(); this.destroyed = false; }
  }
  class Readable extends Stream {
    constructor(o) { super(); o = o || {}; initR(this, o); abortOn(this, o.signal); }
    static from(it, o) {
      const r = new Readable(Object.assign({ objectMode: true }, o || {}));
      if (typeof it === "string" || ArrayBuffer.isView(it)) { r.push(it); r.push(null); return r; }
      (async () => { for await (const x of it) { if (r.destroyed) return; r.push(x); } r.push(null); })().catch(e => destroyS(r, e));
      return r;
    }
    _read() {}
    push(c, enc) {
      const s = this._rs;
      if (c === null) { s.ended = true; this._kick(); return false; }
      if (this.destroyed || s.ended) return false;
      if (!s.objectMode && typeof c === "string") c = Buffer.from(c, enc || "utf8");
      else if (!s.objectMode && ArrayBuffer.isView(c) && !(c instanceof Buffer)) c = Buffer.from(c);
      if (s.enc && !s.objectMode) c = Buffer.from(c).toString(s.enc);
      s.buf.push(c); s.reading = false;
      this._kick();
      return true;
    }
    unshift(c) { this._rs.buf.unshift(c); this._kick(); }
    _kick() { const s = this._rs; if (s.sched) return; s.sched = true; tick(() => { s.sched = false; this._drain(); }); }
    _drain() {
      const s = this._rs;
      if (this.destroyed) return;
      while (s.flowing && s.buf.length && !this.destroyed) this.emit("data", s.buf.shift());
      if (!s.buf.length && s.ended && !s.endEmitted) {
        s.endEmitted = true; this.readableEnded = true; this.readable = false;
        this.emit("end");
        if (!this._ws && this._autoClose !== false) tick(() => { if (!this.destroyed) { this.destroyed = true; this.closed = true; this.emit("close"); } });
        return;
      }
      if (s.flowing && !s.buf.length && !s.ended && !s.reading) { s.reading = true; try { this._read(16384); } catch (e) { destroyS(this, e); } }
    }
    on(n, f) { super.on(n, f); if (n === "data" && this._rs.flowing !== false) this.resume(); return this; }
    addListener(n, f) { return this.on(n, f); }
    resume() { this._rs.flowing = true; this._rs.reading = false; this._kick(); return this; }
    pause() { this._rs.flowing = false; return this; }
    isPaused() { return this._rs.flowing === false; }
    setEncoding(e) { this._rs.enc = e; return this; }
    read() {
      const s = this._rs;
      if (s.buf.length) return s.buf.shift();
      if (!s.ended && !s.reading) { s.reading = true; try { this._read(16384); } catch (e) { destroyS(this, e); } }
      if (s.ended) this._kick();
      return null;
    }
    pipe(dest, o) {
      this.on("data", c => { if (dest.write(c) === false) { this.pause(); dest.once("drain", () => this.resume()); } });
      if (!o || o.end !== false) this.once("end", () => dest.end());
      if (typeof dest.emit === "function") dest.emit("pipe", this);
      return dest;
    }
    unpipe() { this.removeAllListeners("data"); this.pause(); return this; }
    destroy(err, cb) { return destroyS(this, err, cb); }
    [Symbol.asyncIterator]() {
      const self = this, q = [], waiters = [];
      let done = false, err = null;
      const settle = () => {
        while (waiters.length && (q.length || done || err)) {
          const w = waiters.shift();
          if (q.length) w.res({ value: q.shift(), done: false });
          else if (err) w.rej(err);
          else w.res({ value: undefined, done: true });
        }
        if (waiters.length && !q.length && !done && !err) self.resume();
      };
      this.on("data", c => { q.push(c); self.pause(); settle(); });
      this.once("end", () => { done = true; settle(); });
      this.once("error", e => { err = e; settle(); });
      this.once("close", () => { done = true; settle(); });
      this.pause();
      return {
        next() { return new Promise((res, rej) => { waiters.push({ res, rej }); settle(); }); },
        return() { destroyS(self); return Promise.resolve({ value: undefined, done: true }); },
        [Symbol.asyncIterator]() { return this; },
      };
    }
  }
  const W = {
    write(c, enc, cb) {
      const s = this._ws;
      if (typeof enc === "function") { cb = enc; enc = null; }
      if (s.ending || this.destroyed) {
        const e = sErr("ERR_STREAM_WRITE_AFTER_END", "write after end");
        tick(() => { if (cb) cb(e); this.emit("error", e); });
        return false;
      }
      if (!s.objectMode && typeof c === "string" && s.decode) { c = Buffer.from(c, enc || "utf8"); enc = "buffer"; }
      s.q.push({ c, enc: enc || (typeof c === "string" ? "utf8" : "buffer"), cb });
      s.len += chunkLen(s, c);
      this._pump();
      if (s.len >= s.hwm) { s.needDrain = true; return false; }
      return true;
    },
    _pump() {
      const s = this._ws;
      if (s.writing || this.destroyed) return;
      const it = s.q.shift();
      if (!it) {
        if (s.needDrain) { s.needDrain = false; tick(() => this.emit("drain")); }
        if (s.ending && !s.finishing) this._finishW();
        return;
      }
      s.writing = true;
      let sync = true;
      const done = e => {
        const go = () => {
          s.writing = false; s.len -= chunkLen(s, it.c);
          if (it.cb) it.cb(e || null);
          if (e) { destroyS(this, e); return; }
          this._pump();
        };
        if (sync) tick(go); else go();
      };
      try { this._write(it.c, it.enc, done); } catch (e) { done(e); }
      sync = false;
    },
    _write(c, enc, cb) { cb(sErr("ERR_METHOD_NOT_IMPLEMENTED", "The _write() method is not implemented")); },
    end(c, enc, cb) {
      if (typeof c === "function") { cb = c; c = null; } else if (typeof enc === "function") { cb = enc; enc = null; }
      const s = this._ws;
      if (c != null) this.write(c, enc);
      if (cb) { this.once("finish", () => cb()); this.once("error", e => cb(e)); }
      if (s.ending) return this;
      s.ending = true; this.writableEnded = true;
      this._pump();
      return this;
    },
    _finishW() {
      const s = this._ws;
      s.finishing = true;
      const fin = e => {
        if (e) { destroyS(this, e); return; }
        this.writableFinished = true; this.writable = false;
        this.emit("finish");
        if (!this._rs && this._autoClose !== false) tick(() => { if (!this.destroyed) { this.destroyed = true; this.closed = true; this.emit("close"); } });
      };
      if (typeof this._final === "function") { try { this._final(e => tick(() => fin(e))); } catch (e) { fin(e); } } else tick(() => fin());
    },
    cork() {}, uncork() {},
    setDefaultEncoding() { return this; },
    destroy(err, cb) { return destroyS(this, err, cb); },
  };
  class Writable extends Stream {
    constructor(o) { super(); o = o || {}; initW(this, o); abortOn(this, o.signal); }
  }
  for (const k of Object.keys(W)) Object.defineProperty(Writable.prototype, k, { value: W[k], writable: true, configurable: true });
  class Duplex extends Readable {
    constructor(o) { super(o); initW(this, o || {}); }
  }
  for (const k of Object.keys(W)) if (k !== "destroy") Object.defineProperty(Duplex.prototype, k, { value: W[k], writable: true, configurable: true });
  class Transform extends Duplex {
    constructor(o) {
      super(o); o = o || {};
      if (typeof o.transform === "function") this._transform = o.transform;
      if (typeof o.flush === "function") this._flush = o.flush;
    }
    _transform(c, enc, cb) { cb(sErr("ERR_METHOD_NOT_IMPLEMENTED", "The _transform() method is not implemented")); }
    _write(c, enc, cb) { this._transform(c, enc, (e, data) => { if (e) { cb(e); return; } if (data != null) this.push(data); cb(); }); }
    _final(cb) {
      const end = (e, data) => { if (e) { cb(e); return; } if (data != null) this.push(data); this.push(null); cb(); };
      if (typeof this._flush === "function") this._flush(end); else end();
    }
  }
  class PassThrough extends Transform { _transform(c, enc, cb) { cb(null, c); } }
  const isWritable = s => s && typeof s.write === "function";
  function finished(s, o, cb) {
    if (typeof o === "function") { cb = o; o = {}; }
    let done = false;
    const fin = e => { if (!done) { done = true; cb(e || undefined); } };
    s.once("error", fin);
    s.once(isWritable(s) && !s._rs ? "finish" : "end", () => fin());
    s.once("close", () => fin(s.readableEnded || s.writableFinished ? undefined : sErr("ERR_STREAM_PREMATURE_CLOSE", "Premature close")));
    return () => { done = true; };
  }
  function pipelineP(list, o) {
    return new Promise((res, rej) => {
      if (list.length < 2) { rej(sErr("ERR_MISSING_ARGS", 'The "streams" argument must be specified')); return; }
      let done = false;
      const fin = e => {
        if (done) return; done = true;
        if (e) { for (const s of list) if (s && typeof s.destroy === "function" && !s.destroyed) s.destroy(); rej(e); } else res();
      };
      const sig = o && o.signal;
      if (sig) { if (sig.aborted) { fin(abortErr()); return; } sig.addEventListener("abort", () => fin(abortErr()), { once: true }); }
      for (const s of list) s.on("error", fin);
      for (let i = 0; i < list.length - 1; i++) list[i].pipe(list[i + 1]);
      const last = list[list.length - 1];
      finished(last, e => fin(e));
    });
  }
  const splitPipe = a => {
    let o = null;
    const t = a[a.length - 1];
    if (t && typeof t === "object" && typeof t.pipe !== "function" && typeof t.write !== "function" && typeof t.on !== "function") { o = t; a = a.slice(0, -1); }
    return [Array.isArray(a[0]) && a.length === 1 ? a[0] : a, o];
  };
  function pipeline(...a) {
    const cb = typeof a[a.length - 1] === "function" ? a.pop() : null;
    const [list, o] = splitPipe(a);
    const p = pipelineP(list, o);
    if (cb) p.then(() => cb(), e => cb(e)); else p.catch(() => {});
    return list[list.length - 1];
  }
  const streamPromises = Object.freeze({ pipeline: (...a) => { const [list, o] = splitPipe(a); return pipelineP(list, o); }, finished: (s, o) => new Promise((res, rej) => finished(s, o || {}, e => (e ? rej(e) : res()))) });
  const streamOwn = { Stream, Readable, Writable, Duplex, Transform, PassThrough, pipeline, finished, promises: streamPromises,
    addAbortSignal: (sig, s) => { abortOn(s, sig); return s; } };

  /* fs read / write streams: one vault read (the bytes are sliced into highWaterMark chunks) /
     a vault write that opens (creates, truncates, refuses for "wx") at construction and appends per chunk */
  class ReadStream extends Readable {
    constructor(p, o) {
      if (typeof o === "string") o = { encoding: o };
      o = o || {};
      super({ encoding: o.encoding, signal: o.signal });
      this.path = pathArg(p); this.bytesRead = 0; this.pending = true;
      this._hwm = o.highWaterMark > 0 ? o.highWaterMark : 65536;
      this._pos = o.start > 0 ? o.start | 0 : 0; this._end = o.end != null ? Number(o.end) : Infinity;
      this._data = null;
      // opened at construction, as Node's: a missing / refused path is an "error" even if nobody reads
      this._got = host.vfs.read(this.path).then(u => {
        if (this.destroyed) return;
        this._data = u; this.pending = false; this.emit("open", 0); this.emit("ready");
      }, e => { destroyS(this, e); });
    }
    _read() {
      this._got.then(() => {
        const u = this._data;
        if (!u || this.destroyed) return;
        const end = Math.min(u.length, this._end + 1);
        if (this._pos >= end) { this.push(null); return; }
        const n = Math.min(this._hwm, end - this._pos), c = Buffer.from(u.subarray(this._pos, this._pos + n));
        this._pos += n; this.bytesRead += n;
        this.push(c);
      });
    }
    close(cb) { destroyS(this, null, cb); }
  }
  class WriteStream extends Writable {
    constructor(p, o) {
      if (typeof o === "string") o = { encoding: o };
      o = o || {};
      super({ signal: o.signal, decodeStrings: true });
      this.path = pathArg(p); this.bytesWritten = 0; this.pending = true;
      const fl = flagsOf(o.flags || "w"), V = host.vfs;
      this._open = (async () => {
        const there = await V.exists(this.path);
        if (there && fl.excl) throw sysE("EEXIST", "file already exists", this.path, "open");
        if (!there || fl.trunc) await V.write(this.path, new Uint8Array(0));
        this.pending = false; this.emit("open", 0); this.emit("ready");
      })();
      this._open.catch(e => destroyS(this, e));
    }
    _write(c, enc, cb) {
      const b = typeof c === "string" ? Buffer.from(c, enc) : c;
      this._open.then(() => host.vfs.append(this.path, b)).then(() => { this.bytesWritten += b.length; cb(); }, cb);
    }
    _final(cb) { this._open.then(() => cb(), cb); }
    close(cb) { if (cb) this.once("close", () => cb()); this.end(); }
  }

  /* git-watch: fs.watch / fs.promises.watch (obsidian-git waits on its askpass helper's answer file with
     `for await (const e of fsp.watch(dir, {signal}))`). No inotify reaches the page: the watcher polls
     the confined fs (list + stat of the watched dir, or the one watched file) every WATCH_MS and reports
     Node's events: "rename" for a name that appeared or went away, "change" for a size/mtime change;
     filename is the entry's base name. recursive watches the top level only. An abort signal ends the
     iterator with AbortError (fs.watch: emits "close"). Paths are the facade's ("/vault/x" or "x"). */
  const WATCH_MS = 250;
  /* PR #33 review: the FIRST snapshot keeps a stat / list failure of the watched path (ENOENT, EPERM from the VFS), so
     the watch reports it as Node's does (fs.promises.watch: the iterator throws; fs.watch: "error") instead of polling
     an empty snapshot forever. A later poll still reads a vanished path as empty ("rename" for what went away). */
  async function watchSnap(p, first) {
    const m = new Map(), sig = s => s.type + ":" + s.size + ":" + s.mtime, keep = e => { throw e; };
    const st = await host.vfs.stat(p).catch(first ? keep : () => null);
    if (!st) return m;
    if (st.type !== "folder") { m.set(String(p).replace(/\/+$/, "").split("/").pop(), sig(st)); return m; }
    for (const [n] of await host.vfs.list(p).catch(first ? keep : () => [])) {
      const s = await host.vfs.stat(String(p).replace(/\/+$/, "") + "/" + n).catch(() => null);
      if (s) m.set(n, sig(s));
    }
    return m;
  }
  const watchDiff = (a, b) => {
    const ev = [];
    for (const [n, s] of b) if (!a.has(n)) ev.push({ eventType: "rename", filename: n }); else if (a.get(n) !== s) ev.push({ eventType: "change", filename: n });
    for (const n of a.keys()) if (!b.has(n)) ev.push({ eventType: "rename", filename: n });
    return ev;
  };
  function watchP(p, o) {
    p = pathArg(p); o = o && typeof o === "object" ? o : {};
    const sig = o.signal, q = [];
    let prev = null, wake = null, done = false, err = null, timer = 0;
    const stop = e => { if (done) return; done = true; err = e || null; clearTimeout(timer); if (wake) { const w = wake; wake = null; w(); } };
    const poll = async () => {
      if (done) return;
      let cur;
      try { cur = await watchSnap(p, !prev); } catch (e) { stop(e); return; }
      if (prev) for (const e of watchDiff(prev, cur)) q.push(e);
      prev = cur;
      if (q.length && wake) { const w = wake; wake = null; w(); }
      if (!done) timer = setTimeout(poll, WATCH_MS);
    };
    if (sig) { if (sig.aborted) stop(abortErr()); else sig.addEventListener("abort", () => stop(abortErr()), { once: true }); }
    if (!done) poll();
    const it = {
      async next() {
        for (;;) {
          if (q.length) return { value: q.shift(), done: false };
          if (done) { if (err) { const e = err; err = null; throw e; } return { value: undefined, done: true }; }
          await new Promise(r => { wake = r; });
        }
      },
      async return() { stop(null); return { value: undefined, done: true }; },
      async throw(e) { stop(null); throw e; },
      [Symbol.asyncIterator]() { return this; },
    };
    return it;
  }
  class FSWatcher extends EmitterBase {
    constructor(p, o, l) {
      super();
      this._ac = new AbortController();
      const sig = o && o.signal;
      if (sig) { if (sig.aborted) this._ac.abort(); else sig.addEventListener("abort", () => this.close(), { once: true }); }
      if (typeof l === "function") this.on("change", l);
      const it = watchP(p, Object.assign({}, o, { signal: this._ac.signal }));
      (async () => { try { for (;;) { const r = await it.next(); if (r.done) break; this.emit("change", r.value.eventType, r.value.filename); } } catch (e) {
        // closed (AbortError), or the watched path could not be read at the start: Node's watcher reports that as "error"
        if (e && e.name !== "AbortError" && !this._closed) { if (this.listenerCount("error")) this.emit("error", e); else console.error("[desktop] fs.watch:", e); }
      } })();
    }
    close() { if (this._closed) return; this._closed = true; this._ac.abort(); later(() => this.emit("close")); }
    ref() { return this; } unref() { return this; }
  }
  const watchCb = function watch(p, o, l) { if (typeof o === "function") { l = o; o = {}; } if (typeof o === "string") o = { encoding: o }; return new FSWatcher(p, o, l); };

  const fsMore = Object.freeze({
    realpathSync, realpath: Object.assign(asCb(realS), { native: asCb(realS) }),
    readlinkSync: readlinkS, readlink: asCb(readlinkS),
    openSync: openS, open: asCb(openS),
    readSync: readS, read: function (fd, buf, ...a) { const cb = cbOf(a); a.pop(); let n; try { n = readS(fd, buf, ...a); } catch (e) { later(() => cb(e)); return; } later(() => cb(null, n, buf)); },
    writeSync: writeS, write: function (fd, data, ...a) { const cb = cbOf(a); a.pop(); let n; try { n = writeS(fd, data, ...a); } catch (e) { later(() => cb(e)); return; } later(() => cb(null, n, data)); },
    closeSync: closeS, close: function (fd, cb) { let e = null; try { closeS(fd); } catch (x) { e = x; } if (typeof cb === "function") later(() => cb(e)); },
    fstatSync: fstatS, fstat: asCb(fstatS),
    readFileSync: fdOrPath(fsSync.readFileSync, readFileFd),
    writeFileSync: fdOrPath(fsSync.writeFileSync, writeFileFd),
    appendFileSync: fdOrPath(fsSync.appendFileSync, writeFileFd),
    createReadStream: (p, o) => new ReadStream(p, o),
    createWriteStream: (p, o) => new WriteStream(p, o),
    ReadStream, WriteStream,
    symlinkSync: policyLink("symlinkSync"), symlink: policyLink("symlink"),
    watch: watchCb, FSWatcher,
    linkSync: policyLink("linkSync"), link: policyLink("link"),
  });
  const fsPromisesMore = Object.freeze({
    watch: watchP,
    realpath: asP(realS), readlink: asP(readlinkS),
    open: (p, f, m) => { try { return Promise.resolve(new FileHandle(openS(p, f, m))); } catch (e) { return Promise.reject(e); } },
    symlink: policyLink("symlink"), link: policyLink("link"),
  });

  /* zlib: gzip / deflate (zlib) / deflate-raw streams over the page's CompressionStream and
     DecompressionStream (the browser's own zlib). Corrupt input is an "error" with Node's code
     Z_DATA_ERROR; the one-shot callback forms collect a stream. */
  const ZFMT = { gzip: "gzip", deflate: "deflate", raw: "deflate-raw" };
  function webZ(inflate, fmt) {
    const C = inflate ? window.DecompressionStream : window.CompressionStream;
    if (typeof C !== "function") throw sErr("ERR_ZLIB_INITIALIZATION_FAILED", "Initialization failed: this page has no " + (inflate ? "DecompressionStream" : "CompressionStream"));
    const cs = new C(fmt), w = cs.writable.getWriter(), r = cs.readable.getReader();
    const zerr = e => { if (e && e.code) return e; const x = new Error(inflate ? "incorrect header check" : String(e && e.message || e)); x.code = inflate ? "Z_DATA_ERROR" : "Z_STREAM_ERROR"; x.errno = inflate ? -3 : -2; return x; };
    for (const p of [w.closed, w.ready, r.closed]) p.catch(() => {});
    let failed = null;
    const t = new Transform({
      transform(c, enc, cb) { if (failed) { cb(failed); return; } w.write(new Uint8Array(c.buffer, c.byteOffset, c.byteLength).slice()).then(() => cb(), e => cb(failed || zerr(e))); },
      flush(cb) { if (failed) { cb(failed); return; } w.close().then(() => pump).then(() => cb(), e => cb(failed || zerr(e))); },
    });
    const pump = (async () => { for (;;) { const { value, done } = await r.read(); if (done) return; t.push(Buffer.from(value)); } })();
    pump.catch(e => { failed = zerr(e); destroyS(t, failed); });
    t.bytesWritten = 0;
    return t;
  }
  const zOne = (inflate, fmt) => (buf, o, cb) => {
    if (typeof o === "function") cb = o;
    const out = [];
    let z;
    try { z = webZ(inflate, fmt); } catch (e) { later(() => cb(e)); return; }
    z.on("data", c => out.push(c));
    z.once("error", e => cb(e));
    z.once("end", () => cb(null, Buffer.concat(out)));
    z.end(typeof buf === "string" ? Buffer.from(buf) : buf);
  };
  const zlibOwn = {
    createGunzip: () => webZ(true, ZFMT.gzip), createGzip: () => webZ(false, ZFMT.gzip),
    createInflate: () => webZ(true, ZFMT.deflate), createDeflate: () => webZ(false, ZFMT.deflate),
    createInflateRaw: () => webZ(true, ZFMT.raw), createDeflateRaw: () => webZ(false, ZFMT.raw),
    gunzip: zOne(true, ZFMT.gzip), gzip: zOne(false, ZFMT.gzip), inflate: zOne(true, ZFMT.deflate), deflate: zOne(false, ZFMT.deflate),
    inflateRaw: zOne(true, ZFMT.raw), deflateRaw: zOne(false, ZFMT.raw),
    constants: Object.freeze({ Z_NO_FLUSH: 0, Z_FINISH: 4, Z_OK: 0, Z_STREAM_END: 1, Z_DATA_ERROR: -3, Z_DEFAULT_COMPRESSION: -1 }),
  };


  /* http(s).get / http(s).request: a Node client request over the SAME Rust desk_http path as
     requestUrl (no new network surface: same backend, same refusals — an https:// URL fails there
     until the app has a TLS client, and the failure reaches the request's "error"). The response is
     a Readable (statusCode, statusMessage, headers lower-cased) holding the whole body; a redirect is
     returned as is (Node does not follow one). setTimeout / destroy / abort / options.signal work. */
  class ClientRequest extends Writable {
    constructor(scheme, a, b, c) {
      super({});
      let url = null, o = {}, cb = null;
      for (const x of [a, b, c]) {
        if (typeof x === "function") cb = x;
        else if (typeof x === "string" || x instanceof URL) url = String(x);
        else if (x && typeof x === "object") o = x;
      }
      if (!url) {
        const host_ = o.hostname || o.host || "localhost", port = o.port ? ":" + o.port : "";
        url = (o.protocol || scheme + ":") + "//" + host_ + port + (o.path || "/");
      }
      let u;
      try { u = new URL(url); } catch (_) { const e = new TypeError("Invalid URL: " + url); e.code = "ERR_INVALID_URL"; throw e; }
      if (u.protocol !== scheme + ":") { const e = new TypeError('Protocol "' + u.protocol + '" not supported. Expected "' + scheme + ':"'); e.code = "ERR_INVALID_PROTOCOL"; throw e; }
      this.method = String(o.method || "GET").toUpperCase(); this.path = u.pathname + u.search; this.host = u.hostname; this.protocol = u.protocol;
      this.aborted = false; this.reusedSocket = false; this._autoClose = false;   // "close" comes after the response (or the error), not at end()
      this._url = u.href; this._hdr = Object.assign({}, o.headers || {}); this._body = []; this._tm = null; this._sent = false;
      if (cb) this.once("response", cb);
      const sig = o.signal;
      if (sig) { if (sig.aborted) tick(() => this.destroy(abortErr())); else sig.addEventListener("abort", () => this.destroy(abortErr()), { once: true }); }
    }
    setHeader(k, v) { this._hdr[k] = v; return this; }
    getHeader(k) { const f = Object.keys(this._hdr).find(x => x.toLowerCase() === String(k).toLowerCase()); return f ? this._hdr[f] : undefined; }
    removeHeader(k) { for (const x of Object.keys(this._hdr)) if (x.toLowerCase() === String(k).toLowerCase()) delete this._hdr[x]; }
    _write(c, enc, cb) { this._body.push(typeof c === "string" ? Buffer.from(c, enc) : Buffer.from(c)); cb(); }
    _final(cb) { cb(); this._send(); }
    setTimeout(ms, f) {
      if (f) this.once("timeout", f);
      window.clearTimeout(this._tm);
      if (ms > 0) this._tm = window.setTimeout(() => { if (!this.destroyed && !this._res) this.emit("timeout"); }, ms);
      return this;
    }
    abort() { if (!this.aborted) { this.aborted = true; this.emit("abort"); this.destroy(); } }
    destroy(err, cb) { window.clearTimeout(this._tm); if (this._res && !this._res.destroyed) destroyS(this._res, err || null); return destroyS(this, err, cb); }
    _send() {
      if (this._sent || this.destroyed) return;
      this._sent = true;
      const body = this._body.length ? Buffer.concat(this._body) : null;
      httpSend({ url: this._url, method: this.method, headers: this._hdr, body }).then(r => {
        window.clearTimeout(this._tm);
        if (this.destroyed) return;
        const res = new Readable({});
        res.statusCode = r.status; res.statusMessage = HTTP_STATUS[r.status] || "";
        res.headers = {}; res.rawHeaders = [];
        for (const k of Object.keys(r.headers)) { res.headers[k.toLowerCase()] = r.headers[k]; res.rawHeaders.push(k, r.headers[k]); }
        res.httpVersion = "1.1"; res.complete = false; res.req = this;
        res.once("end", () => { res.complete = true; tick(() => { if (!this.destroyed) { this.destroyed = true; this.closed = true; this.emit("close"); } }); });
        this._res = res;
        if (r.body.length) res.push(Buffer.from(r.body));
        res.push(null);
        if (this.listenerCount("response")) this.emit("response", res); else res.resume();
      }, e => { window.clearTimeout(this._tm); if (!this.destroyed) destroyS(this, Object.assign(e, { code: e.code || "ECONNREFUSED" })); });
    }
  }
  const clientRequest = scheme => (a, b, c) => new ClientRequest(scheme, a, b, c);
  const clientGet = scheme => (a, b, c) => { const r = new ClientRequest(scheme, a, b, c); r.end(); return r; };
  /* worker_threads: a plugin runs on the page's main thread. isMainThread, no parentPort (null in
     Node's main thread too), no workerData; a Worker (a Node thread) stays the deny stub. */
  const workerOwn = { isMainThread: true, parentPort: null, workerData: null, threadId: 0, resourceLimits: Object.freeze({}),
    SHARE_ENV: Symbol.for("nodejs.worker_threads.SHARE_ENV") };

  /* ------------------------------------------- electron (shell real, Rust desk_shell)
     shell.openExternal / openPath / showItemInFolder go to the OS opener with the user's
     rights, as stock's do. remote / ipcRenderer: just enough for bundles that look up the
     focused window at load or show a window; there is no second BrowserWindow. */
  const shellCall = (op, target) => desk ? desk("desk_shell", { op, target: String(target) }) : Promise.reject(new Error("shell: no desktop bridge"));
  const shellOwn = Object.freeze({
    openExternal: url => shellCall("external", url).then(() => undefined),
    openPath: p => shellCall("path", p).then(() => "", e => String(e && e.message || e)),
    showItemInFolder: p => { shellCall("reveal", p).catch(e => console.warn("[desktop] showItemInFolder: " + String(e && e.message || e))); },
    beep() {},
  });
  const noWindow = Object.freeze({ show() {}, focus() {}, isFocused: () => document.hasFocus(), isDestroyed: () => false, webContents: Object.freeze({ id: 1 }) });
  /* remote.app: what plugins read at load (pdf-plus: getVersion). The version is the
     Obsidian API level this app implements, not a browser or engine version. */
  const appVersion = () => "1.14.4";   // = api-core.js API_VERSION = plugin_store.rs COMPAT_APP_VERSION (T0 drift test)
  const remoteApp = Object.freeze({
    getVersion: appVersion,
    getName: () => "opensidian",
    getLocale: () => navigator.language || "en-US",
    getPath: n => (n === "userData" || n === "appData" ? "/vault/.obsidian" : n === "home" ? OS_HOME : n === "temp" ? "/tmp" : "/vault"),
    isPackaged: true,
  });
  const electronOwn = {
    shell: shellOwn,
    remote: Object.freeze({ app: remoteApp, shell: shellOwn, BrowserWindow: Object.freeze({ getFocusedWindow: () => noWindow, getAllWindows: () => [noWindow] }), getCurrentWindow: () => noWindow }),
    ipcRenderer: Object.freeze({ send() {}, on() { return this; }, once() { return this; }, removeListener() { return this; }, removeAllListeners() { return this; }, sendSync: () => undefined, invoke: () => Promise.resolve(undefined) }),
  };

  const PLUGIN_DIR = id => "/vault/.obsidian/plugins/" + id;
  const views = new Map();   // id -> view (one per plugin id for the page's life)
  function view(id) {
    id = String(id);
    if (views.has(id)) return views.get(id);
    const made = new Map();   // one module object per builtin name (require twice -> same object)
    let boundReq = null;      // the loader's require for this plugin, for module.createRequire
    const ownRequire = function require(n) {
      if (!boundReq) { const e = new Error("Cannot find module '" + String(n) + "'"); e.code = "MODULE_NOT_FOUND"; throw e; }
      return boundReq(n);
    };
    ownRequire.resolve = n => { const e = new Error("Cannot find module '" + String(n) + "' (no module paths in opensidian)"); e.code = "MODULE_NOT_FOUND"; throw e; };
    ownRequire.cache = Object.create(null);
    const proc = processShim();
    const mods = new Map([
      ["buffer", () => bufferMod],
      ["process", () => proc],
      ["events", () => eventsMod],
      ["string_decoder", () => stringDecoderMod],
      ["readline", () => readlineMod],
      ["url", () => urlMod],
      ["constants", () => constantsMod],
      ["querystring", () => qsMod],
      ["timers", () => timersMod],
      ["os", () => osOwn],
      ["async_hooks", () => asyncHooksOwn],
      ["module", () => Object.freeze({ createRequire: () => ownRequire, builtinModules: Object.freeze([]) })],
      ["util", (deny, n) => mod(n, deny, utilAll)],
      ["crypto", (deny, n) => mod(n, deny, cryptoAll)],
      ["tty", (deny, n) => mod(n, deny, { isatty: () => false })],
      ["child_process", (deny, n) => mod(n, deny, cpOwn)],
      ["net", (deny, n) => mod(n, deny, netOwn)],
      ["http", (deny, n) => mod(n, deny, httpOwn)],
      ["https", (deny, n) => mod(n, deny, httpsOwn)],
      ["electron", (deny, n) => mod(n, deny, electronOwn)],
      ["stream", (deny, n) => mod(n, deny, streamOwn)],
      ["stream/promises", () => streamPromises],
      ["zlib", (deny, n) => mod(n, deny, zlibOwn)],
      ["worker_threads", (deny, n) => mod(n, deny, workerOwn)],
    ]);
    /* the vault fs facade (node-modules.js) is one frozen object shared by every plugin;
       graceful-fs defines a queue symbol on require("fs") and reassigns close/closeSync at
       load. A plugin gets its own extensible copy of the same functions, so its writes land
       on its copy, never on another plugin's fs. */
    const owned = new Map();
    const own = m => {
      if (!m || typeof m !== "object") return m;
      let c = owned.get(m);
      if (!c) {
        const d = Object.getOwnPropertyDescriptors(m);
        for (const k of Reflect.ownKeys(d)) { d[k].configurable = true; if ("value" in d[k]) d[k].writable = true; }
        // a trusted vault's fs has Node's *Sync calls (the facade's ENOSYS stubs are replaced)
        if (isFsFacade(m)) for (const k of Object.keys(fsSync)) d[k] = { value: fsSync[k], enumerable: true, writable: true, configurable: true };
        if (isFsFacade(m)) d.chmod = { value: chmodCb, enumerable: true, writable: true, configurable: true };
        // top10-func: descriptors, realpath / readlink, read / write streams, link refusal
        if (isFsFacade(m)) for (const k of Object.keys(fsMore)) d[k] = { value: fsMore[k], enumerable: true, writable: true, configurable: true };
        if (isFsPromises(m)) d.chmod = { value: chmodP, enumerable: true, writable: true, configurable: true };
        if (isFsPromises(m)) for (const k of Object.keys(fsPromisesMore)) d[k] = { value: fsPromisesMore[k], enumerable: true, writable: true, configurable: true };
        // as in Node, require("fs").promises is the very object require("fs/promises") returns
        if (isFsFacade(m)) d.promises.value = own(m.promises);
        c = Object.defineProperties(Object.create(Object.getPrototypeOf(m)), d);
        owned.set(m, c);
      }
      return c;
    };
    const v = Object.freeze({
      own,
      // a builtin this view answers; deny is the loader's message for the stub members
      resolve(name, deny) { const b = bare(name), f = mods.get(b); if (!f) return undefined; if (!made.has(b)) made.set(b, f(String(deny), b)); return made.get(b); },
      bindRequire(r) { if (typeof r === "function" && !boundReq) boundReq = r; },
      api: obsidianApi,
      process: proc,
      global: ownGlobal(proc),
      Buffer,
      Error: ownError(),
      SharedArrayBuffer: OwnSAB,
      filename: PLUGIN_DIR(id) + "/main.js",
      dirname: PLUGIN_DIR(id),
    });
    views.set(id, v);
    return v;
  }
  host.desktopNode = Object.freeze({ view });
})();
