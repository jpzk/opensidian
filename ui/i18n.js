// SPDX-License-Identifier: GPL-3.0-or-later
/* I18N — UI translations (issue #220), Project Fluent.
   Loaded after vendor/fluent/fluent-bundle.js (window.FluentBundle) and BEFORE every
   other UI script, so any of them can call t() while it builds DOM.

   The strings: repo-root locales/<lang>/main.ftl, compiled into the binary by
   src-tauri/src/i18n.rs (the SAME bytes Rust formats native text from). The window's
   initialization script (main.rs page_window) puts the English source and the
   active language on window.__OS_I18N before any page script runs, so the first
   paint is already in the right language — no async gap, no flash of ids.

   t(id, args): active language -> English -> the id itself. useIsolating is OFF:
   no U+2068/U+2069 marks around placeables, so English output is byte-identical
   to the literals it replaced.

   Setting: ~/.opensidian.json "language" = "auto" | a code. Auto = the Rust side's
   LC_ALL > LC_MESSAGES > LANG answer when it names a language other than
   English, else the first of navigator.languages we have a table for, else
   English. The env goes first because WebKitGTK derives navigator.languages
   from setlocale(): with LANG=ja_JP.UTF-8 but no ja_JP locale generated it
   reports "en-US", which would hide the language the user actually set.

   LIVE SWITCH: set(code) persists, swaps the bundle and calls every onChange
   subscriber; static nodes marked data-i18n / data-i18n-<attr> are re-rendered
   here. Menus, the palette and modals build their DOM when opened, so they pick
   the new language up on their next open with no subscriber at all. */
(function () {
  "use strict";
  const T = window.__TAURI__;               // captured at load: the plugin loader removes the global later
  const FB = window.FluentBundle;
  const boot = window.__OS_I18N || {};
  const LANGS = Array.isArray(boot.langs) && boot.langs.length ? boot.langs : [["en", "English"]];
  const subs = [];
  const cache = Object.create(null);         // code -> ftl source we were handed
  if (typeof boot.en === "string") cache.en = boot.en;
  if (boot.lang && typeof boot.ftl === "string") cache[boot.lang] = boot.ftl;
  let setting = boot.setting || "auto", system = boot.system || "en", lang = "en", active = null, en = null;

  function mk(code, src) {
    if (!FB || typeof src !== "string") return null;
    const b = new FB.FluentBundle(code, { useIsolating: false });
    const errs = b.addResource(new FB.FluentResource(src));
    if (errs.length) console.warn("[i18n] " + code + ": " + errs.length + " ftl error(s): " + errs[0]);
    return b;
  }
  function fmt(b, id, args) {
    if (!b) return null;
    const m = b.getMessage(id);
    if (!m || m.value == null) return null;
    const errs = [];
    const s = b.formatPattern(m.value, args || null, errs);
    return errs.length ? null : s;
  }
  /* the message `id` in the active language, else English, else the id
     (a missing message shows its id: visible in a screenshot, never blank) */
  function t(id, args) {
    const s = fmt(active, id, args);
    if (s != null) return s;
    const e = fmt(en, id, args);
    return e != null ? e : id;
  }
  /* EXTRACTED LITERALS (#220 item 5). A surface whose English text lives in
     data (the settings table, the command registry) or in a menu call site
     keeps that English as the fallback and asks for a DERIVED id:
       slug("Open in new tab") = "open-in-new-tab"  (ASCII a-z0-9, runs -> "-";
       "..." -> "dots", so "Rename..." and "Rename" stay two messages)
     src-tauri/src/i18n.rs carries the same slug() and its cargo tests check that
     every derived en message equals the literal it replaces, so English stays
     byte-identical. tk(): active -> English -> `fallback` (never the id: a
     derived id is not something a user should ever see). */
  function slug(s) {
    return String(s == null ? "" : s).replace(/\.\.\./g, " dots ").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  }
  function tk(id, fallback, args) {
    const s = fmt(active, id, args);
    if (s != null) return s;
    const e = fmt(en, id, args);
    return e != null ? e : fallback;
  }
  // a menu row's literal English title -> "menu-<slug>" (call sites with a LITERAL only:
  // a note or folder name must never go through here)
  const m = s => { const k = slug(s); return k ? tk("menu-" + k, s) : s; };
  // "ja-JP" / "zh-Hans-CN" / "ru" -> a code we ship, or null (mirrors i18n.rs match_tag)
  function match(tag) {
    const p = String(tag || "").split(/[.@]/)[0].replace(/_/g, "-").split("-")[0].toLowerCase();
    const m = { en: "en", zh: "zh-CN", ko: "ko", ru: "ru", es: "es", ja: "ja" }[p];
    return m && LANGS.some(l => l[0] === m) ? m : null;
  }
  function autoLang() {
    const env = match(system);
    if (env && env !== "en") return env;
    const nav = (navigator.languages && navigator.languages.length) ? navigator.languages : [navigator.language];
    for (const l of nav) { const m = match(l); if (m) return m; }
    return match(system) || "en";
  }
  const resolve = s => (s === "auto" ? autoLang() : (LANGS.some(l => l[0] === s) ? s : "en"));

  const ATTRS = ["title", "placeholder", "aria-label"];
  /* re-render the static nodes under `root`: data-i18n="id" -> textContent,
     data-i18n-title / -placeholder / -aria-label="id" -> that attribute. A node
     whose message is missing everywhere keeps its literal text. */
  function apply(root) {
    const r = root || document;
    r.querySelectorAll("[data-i18n]").forEach(n => {
      const id = n.getAttribute("data-i18n"), s = fmt(active, id) ?? fmt(en, id);
      if (s != null && n.textContent !== s) n.textContent = s;
    });
    for (const a of ATTRS) r.querySelectorAll("[data-i18n-" + a + "]").forEach(n => {
      const id = n.getAttribute("data-i18n-" + a), s = fmt(active, id) ?? fmt(en, id);
      if (s != null && n.getAttribute(a) !== s) n.setAttribute(a, s);
    });
  }

  async function source(code) {
    if (cache[code] == null && T) {
      const r = await T.core.invoke("i18n_load", { lang: code });
      cache[code] = r.ftl;
      if (cache.en == null) cache.en = r.en;
    }
    return cache[code];
  }
  function use(code) {
    if (!en) en = mk("en", cache.en);
    lang = code;
    active = code === "en" ? en : mk(code, cache[code]);
    // lang= picks the right CJK glyphs (Han unification); English keeps the page as it was (no lang)
    if (code === "en") document.documentElement.removeAttribute("lang"); else document.documentElement.setAttribute("lang", code);
  }
  function emit() {
    apply(document);
    for (const f of subs.slice()) { try { f(lang); } catch (e) { console.error("[i18n] onChange", e); } }
  }
  /* switch now (live) and persist; resolves to the language in use.
     Each switch takes a ticket: a slow load from an earlier switch (or the first-paint
     load) that finishes after a newer one must not put the older language back (#220 review) */
  let ticket = 0;
  async function set(s) {
    if (s !== "auto" && !LANGS.some(l => l[0] === s)) throw new Error("unknown language " + s);
    const my = ++ticket;
    if (T) await T.core.invoke("i18n_set", { setting: s });
    if (my !== ticket) return lang;
    setting = s;
    const code = resolve(s);
    await source(code);
    if (my !== ticket) return lang;
    if (code !== lang) { use(code); emit(); }
    return lang;
  }

  // first paint: the language the boot data carries if it is the one we resolve to
  // (it is unless navigator.languages disagrees with the env); else switch once loaded
  const want = resolve(setting);
  use(cache[want] != null ? want : (cache[boot.lang] != null ? boot.lang : "en"));
  if (lang !== want) { const my = ticket; source(want).then(() => { if (my === ticket) { use(want); emit(); } }).catch(e => console.error("[i18n] load", e)); }
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", () => apply(document), { once: true });
  else apply(document);

  window.I18N = Object.freeze({
    t, tk, m, slug, set, apply, match,
    lang: () => lang,
    setting: () => setting,
    system: () => system,
    langs: () => LANGS.map(l => l.slice()),
    onChange(f) { if (typeof f === "function") subs.push(f); },
  });
})();
