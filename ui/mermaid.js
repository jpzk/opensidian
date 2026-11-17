/* #187 Mermaid diagrams: ```mermaid fences in the reading view and Live Preview.
   Stock 1.13.7 (black box, goal b96-render d187 captures) shows a trust guard first:
     div.mermaid-wrapper.is-guarded > div.mermaid-guard-header >
       div.mermaid-guard-text > .mermaid-guard-title "Display Mermaid diagrams in this vault?"
                              + .mermaid-guard-description "Only allow if you trust this vault's contents."
       + button "Allow"
     followed by the diagram source as code.
   "Allow" is per vault and remembered (here: localStorage "os-mermaid-allow:<vault>").
   Mermaid itself (ui/vendor/mermaid, ~3.5 MB) is loaded ONLY when an allowed vault
   shows a mermaid fence, through the plugin API's loader (window.__osLibs, ui/plugins/
   api-functions.js) so the app and plugins share one locked instance and one gate.
   Rendered SVG is parsed in an inert XML document, stripped of anything that could
   run or fetch (script, foreignObject, on* attributes, non-fragment hrefs, external
   url(...) / @import in styles) and only then imported. Errors render inline: the
   message plus the source. A light/dark switch re-renders every live diagram. */
(function () {
  "use strict";
  const live = new Set();             // wrappers currently showing (or about to show) a diagram
  let queue = Promise.resolve();      // one render at a time: a hostile diagram cannot stack up work
  let busy = 0, pubT = 0;             // renders queued or running; title-census refresh timer
  const el = (tag, cls, text) => { const e = document.createElement(tag); if (cls) e.className = cls; if (text != null) e.textContent = text; return e; };
  const vaultKey = () => "os-mermaid-allow:" + (typeof vaultPath !== "undefined" && vaultPath ? vaultPath : "");
  function allowed() { try { return localStorage.getItem(vaultKey()) === "1"; } catch (_) { return false; } }
  function allow() {
    try { localStorage.setItem(vaultKey(), "1"); } catch (_) {}
    for (const w of document.querySelectorAll(".mermaid-wrapper.is-guarded")) if (w._mmSrc != null) show(w);
  }

  const SVGNS = "http://www.w3.org/2000/svg";
  const BAD_EL = new Set(["script", "foreignobject", "iframe", "object", "embed", "link", "meta", "image", "animate", "set", "animatemotion", "animatetransform", "handler", "listener"]);
  const EXT_URL = /url\s*\(\s*(?!['"]?\s*#)[^)]*\)/gi;   // url(...) that is not a same-document fragment
  function cleanStyle(t) { return String(t).replace(/@import[^;]*;?/gi, "").replace(EXT_URL, "none").replace(/expression\s*\(|javascript:/gi, ""); }
  function scrub(n) {
    for (const c of Array.from(n.children)) {
      if (BAD_EL.has(c.localName.toLowerCase()) || (c.namespaceURI !== SVGNS && c.localName.toLowerCase() !== "style")) { c.remove(); continue; }
      for (const a of Array.from(c.attributes)) {
        const k = a.name.toLowerCase(), v = a.value;
        if (k.startsWith("on")) c.removeAttribute(a.name);
        else if ((k === "href" || k === "xlink:href" || k.endsWith(":href") || k === "src" || k === "action" || k === "formaction") && !/^#/.test(v.trim())) c.removeAttribute(a.name);
        else if (k === "style" || /url\s*\(/i.test(v)) c.setAttribute(a.name, cleanStyle(v));
      }
      if (c.localName.toLowerCase() === "style") c.textContent = cleanStyle(c.textContent);
      else scrub(c);
    }
    return n;
  }
  function toSvg(text) {
    const doc = new DOMParser().parseFromString(String(text || ""), "image/svg+xml");
    const r = doc.documentElement;
    if (!r || r.localName !== "svg" || r.namespaceURI !== SVGNS || doc.querySelector("parsererror")) throw new Error("Mermaid produced no SVG");
    for (const a of Array.from(r.attributes)) { const k = a.name.toLowerCase(); if (k.startsWith("on") || k.endsWith("href")) r.removeAttribute(a.name); else if (k === "style") r.setAttribute(a.name, cleanStyle(a.value)); }
    return document.importNode(scrub(r), true);
  }

  function sourceBlock(src) {
    const pre = el("pre", "language-mermaid"), code = el("code", "language-mermaid", src);
    pre.appendChild(code);
    return pre;
  }
  function guard(w) {
    w.className = "mermaid-wrapper is-guarded";
    w.replaceChildren();
    const h = el("div", "mermaid-guard-header"), t = el("div", "mermaid-guard-text");
    t.append(el("div", "mermaid-guard-title", "Display Mermaid diagrams in this vault?"),
             el("div", "mermaid-guard-description", "Only allow if you trust this vault's contents."));
    const b = el("button", "mermaid-guard-allow mod-cta", "Allow");
    b.type = "button";
    b.addEventListener("mousedown", e => e.stopPropagation());
    b.addEventListener("click", e => { e.preventDefault(); e.stopPropagation(); allow(); });
    h.append(t, b);
    w.append(h, sourceBlock(w._mmSrc));
  }
  function fail(w, err) {
    w.className = "mermaid-wrapper is-error";
    const msg = (err && (err.message || err.str)) || String(err || "Mermaid error");
    const box = el("div", "mermaid-error");
    box.append(el("div", "mermaid-error-title", "Mermaid: " + String(msg).split("\n")[0].slice(0, 300)));
    if (String(msg).includes("\n")) box.append(el("pre", "mermaid-error-detail", String(msg).slice(0, 2000)));
    w.replaceChildren(box, sourceBlock(w._mmSrc));
  }
  function render(w) {
    const libs = window.__osLibs, gen = (w._mmGen = (w._mmGen || 0) + 1);
    live.add(w);
    if (!libs || typeof libs.renderMermaid !== "function") { fail(w, new Error("Mermaid is not available")); return Promise.resolve(); }
    if (!w.firstChild) w.append(sourceBlock(w._mmSrc));
    w.classList.add("is-loading");
    busy++;
    const job = queue.then(() => {
      if (!w.isConnected && w._mmLive) { live.delete(w); return; }
      return libs.renderMermaid(w._mmSrc).then(r => {
        if (gen !== w._mmGen) return;
        const m = el("div", "mermaid");
        m.appendChild(toSvg(r.svg));
        w.className = "mermaid-wrapper";
        w.replaceChildren(m);
      }, e => { if (gen === w._mmGen) fail(w, e); });
    }).catch(e => { if (gen === w._mmGen) fail(w, e); }).then(() => { w._mmLive = true; busy--; if (window.Merm) Merm.rendered++; pub(); });
    queue = job;
    return job;
  }
  function show(w) { if (allowed()) render(w); else { guard(w); pub(); } }
  // a wrapper for one fence's source: guard or diagram
  function block(src) {
    const w = el("div", "mermaid-wrapper");
    w._mmSrc = String(src || "").replace(/\n$/, "");
    show(w);
    return w;
  }
  // reading view: every pre > code.language-mermaid the renderer emitted
  function reading(root) {
    if (!root) return 0;
    let n = 0;
    for (const code of root.querySelectorAll("pre > code.language-mermaid")) {
      const pre = code.parentNode;
      if (pre.closest(".mermaid-wrapper")) continue;
      pre.replaceWith(block(code.textContent));
      n++;
    }
    return n;
  }
  // light/dark (body.theme-dark/-light) or the data-theme attribute changed: re-render live diagrams
  let thm = "";
  const theme = () => (document.body && document.body.classList.contains("theme-dark") ? "d" : "l") + document.documentElement.getAttribute("data-theme");
  function onTheme() {
    const t = theme();
    if (t === thm) return;
    thm = t;
    for (const w of Array.from(live)) {
      if (!w.isConnected) { live.delete(w); continue; }
      if (!w.classList.contains("is-guarded")) render(w);
    }
  }
  function watch() {
    thm = theme();
    const mo = new MutationObserver(onTheme);
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class"] });
    if (document.body) mo.observe(document.body, { attributes: true, attributeFilter: ["class"] });
  }
  if (document.body) watch(); else document.addEventListener("DOMContentLoaded", watch);
  // the title census reads the DOM; refresh it once the async renders settle
  function pub() { if (pubT) return; pubT = setTimeout(() => { pubT = 0; try { if (typeof updateTitle === "function") updateTitle(); } catch (_) {} }, 30); }
  function tok() {
    const q = s => Array.from(document.querySelectorAll(s)).filter(e => e.getClientRects().length > 0);
    const kinds = q(".mermaid-wrapper .mermaid > svg").map(s => s.getAttribute("aria-roledescription") || "?");
    const b = q(".mermaid-wrapper.is-guarded .mermaid-guard-header button")[0], r = b && b.getBoundingClientRect();
    return " [mmd:lib=" + (window.mermaid ? 1 : 0) + "|allow=" + (allowed() ? 1 : 0) + "|g=" + q(".mermaid-wrapper.is-guarded").length +
      "|ok=" + kinds.length + "|err=" + q(".mermaid-wrapper.is-error").length + "|busy=" + busy + "|kinds=" + kinds.join(",") +
      "|th=" + (document.body && document.body.classList.contains("theme-dark") ? "d" : "l") + "|pwn=" + (window.__mmPwn ? 1 : 0) +
      (r ? "|btn=" + Math.round(r.x + r.width / 2) + "," + Math.round(r.y + r.height / 2) : "") + "]";
  }
  window.Merm = { block, reading, allowed, allow, rendered: 0, live: () => live.size, tok };
})();
