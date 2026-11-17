// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* #144 Settings › Security — the pane (docs/security-section.md §2).
   The page holds NO security state of its own: every row is drawn from
   security_get (src-tauri/src/security.rs, the one reader of the "security" key
   in ~/.opensidian.json) and every press goes through security_set, which asks a
   NATIVE confirm before any change that lowers a protection. Nothing here is an
   enforcement point; the boundaries are in Rust / WebKit settings / the CSP.
   Markup = the settings pane's own rows (.ssec heading, .scard, .srow/.sinfo/
   .slabel/.sdesc, .sctl.toggle), so the tab looks like every other Options tab.
   Census (while the pane is built): [secp:<rows>/<forced>/<pending>] and
   [secv:<field>=<stored><effective>,...] (1/0 pairs), read by the T3 phases.
   Plugins can ask: window.opensidianSecurity = a frozen copy of the EFFECTIVE values
   (informational, refreshed after every change; plugins degrade instead of throwing). */
(() => {
  "use strict";
  let R = null;            // last security_get answer
  let busy = false;
  const T = window.__TAURI__;   // captured at load: plugins/loader.js deletes window.__TAURI__ before any plugin runs
  const inv = (c, a) => T.core.invoke(c, a);
  const el = (tag, cls, txt) => { const d = document.createElement(tag); if (cls) d.className = cls; if (txt != null) d.textContent = txt; return d; };
  /* [field, invert, description]. invert = the switch reads ON when the stored field is false
     (toggle 2's switch is "Cut off ..."); offline / lockdown store true = on. */
  const GROUPS = [
    [null, [["lockdown", false, "Sets every toggle below to its strictest value: community plugins do not run in any vault, no network, no media, no WebGL / canvas, no JIT, no math / code / diagram rendering. Your own values are kept and come back when you turn it off. Needs a restart."]]],
    ["Network", [
      ["offline", false, "Nothing in the app reaches the network: no plugin or theme store, no update check, no plugin requests or listeners. Links still open in your browser after a confirm."],
      ["remoteMedia", false, "Show images, audio and video from https:// addresses in notes. Off: a placeholder is shown and the address never reaches the page."],
      ["pluginNet", true, "Community plugins cannot make requests or open listeners."],
    ]],
    ["Media and rendering", [
      ["vaultMedia", false, "Render images, PDFs, audio and video from your vaults. Off: a placeholder is shown and no file reaches a decoder."],
      ["webgl", false, "Off: the graph switches to its 2D renderer at once; plugin code gets no new WebGL context (one made before keeps running until a reload)."],
      ["canvas", false, "Off: <canvas> drawing is replaced before any page code runs (graph, image export and paste re-encoding stop). Limit: hardening only; a trusted plugin with native access can still reach drawing through other routes."],
      ["mathjax", false, "Render $math$ with MathJax. Off: math shows as plain text."],
      ["prism", false, "Highlight code blocks with Prism. Off: code shows as plain text."],
      ["mermaid", false, "Render mermaid diagrams. Off: diagrams show as plain text."],
    ]],
    ["Plugins and clipboard", [
      ["pluginSpawn", false, "Community plugins may start programs (each run is still asked about). Off: refused before any dialog, also opening files or links with the system opener."],
      ["pluginClipboard", false, "Community plugin code may read and write the clipboard. Off: once a plugin has loaded on the page, programmatic clipboard reads and writes are refused for everyone on that page; Ctrl+C / Ctrl+X / Ctrl+V and the menu Copy / Cut keep working, the menu Paste rows are disabled (use Ctrl+V). A plugin still sees a paste you make yourself."],
      ["appClipboard", false, "The app's own copy, cut and paste. Off: Ctrl+C / Ctrl+X / Ctrl+V, Shift+Insert, middle-click paste and the menu clipboard rows stop working, for the app and plugins alike. Applies at once."],
    ]],
    ["Engine", [
      ["jit", false, "The JavaScript JIT compiler. Off: slower, a smaller attack surface. Needs a restart."],
    ]],
  ];
  const WHY = { lockdown: "Lockdown mode", offline: "Cut off all network (offline mode)", canvas: "HTML canvas", restart: "a restart" };
  const shown = (f, inv0) => { const v = !!R.stored[f]; return inv0 ? !v : v; };
  function publish() {
    if (!R) return;
    try { window.opensidianSecurity = Object.freeze(Object.assign({ lockdown: !!R.lockdown }, R.effective)); } catch (_) {}
    cspAgree();
  }
  /* toggle 1 layer 2: the page CSP is fixed at load (main.rs page_csp). When the effective
     "Load external resources" and this page's header disagree, reload once (after the save
     debounce, so a pending note write lands) — turning it OFF removes https: at once.
     Two reloads in a row that still disagree stop with a console warning, never a loop.
     Toggle 4 (Render media) rides the same rule: its CSP layer (opensidian-img: in img-src,
     blob: in media-src) is fixed at load too (security_get pageMedia). */
  let reloading = false;
  function cspAgree() {
    const KEY = "osSecReload";
    if (!R || typeof R.pageRemote !== "boolean" || typeof R.pageMedia !== "boolean" || !R.effective) return;
    const agree = !!R.effective.remoteMedia === R.pageRemote && !!R.effective.vaultMedia === R.pageMedia;
    if (agree) { try { sessionStorage.removeItem(KEY); } catch (_) {} return; }
    let n = 0;
    try { n = +sessionStorage.getItem(KEY) || 0; } catch (_) {}
    if (reloading) return;
    if (n >= 2) { console.warn("[sec] page CSP still disagrees with Load external resources / Render media after 2 reloads"); return; }
    reloading = true;
    try { sessionStorage.setItem(KEY, String(n + 1)); } catch (_) {}
    console.warn("[sec] reloading the page into the CSP of Load external resources=" + !!R.effective.remoteMedia + " Render media=" + !!R.effective.vaultMedia);
    Promise.resolve(inv("save_debounce_ms")).catch(() => 0)
      .then(ms => setTimeout(() => location.reload(), (+ms || 0) + 300));
  }
  async function refresh() { R = await inv("security_get"); publish(); return R; }
  async function set(f, on) {
    if (busy) return; busy = true;
    try { R = await inv("security_set", { field: f, on }); publish(); }
    catch (e) { console.error("[sec] set " + f + ":", e); if (typeof say === "function") say("Security: " + String(e && e.message || e)); }
    busy = false;
    if (typeof settingsOpen !== "undefined" && settingsOpen && sPane === "security") build(document.getElementById("spage"));
  }
  function toggle(on, onclick, dis) {
    const t = el("div", "sctl toggle" + (dis ? "" : " live") + (on ? " on" : ""));
    t.appendChild(el("i"));
    t.setAttribute("role", "switch"); t.setAttribute("aria-checked", on ? "true" : "false");
    if (dis) { t.setAttribute("aria-disabled", "true"); return t; }
    t.tabIndex = 0;
    t.onclick = ev => { ev.stopPropagation(); onclick(); };
    t.onkeydown = ev => { if (ev.key === " " || ev.key === "Enter") { ev.preventDefault(); onclick(); } };
    return t;
  }
  let nRows = 0, nForced = 0;
  function build(pg) {
    if (!pg) return;
    pg.innerHTML = ""; pg.className = "rows";
    if (!R) {
      pg.appendChild(el("div", "sempty", "Loading…"));
      refresh().then(() => { if (settingsOpen && sPane === "security") build(document.getElementById("spage")); },
                     e => { pg.innerHTML = ""; pg.appendChild(el("div", "sempty", "Security settings could not be read: " + String(e && e.message || e))); });
      return;
    }
    nRows = 0; nForced = 0;
    for (const [sec, rows] of GROUPS) {
      if (sec) pg.appendChild(el("div", "ssec", sec));
      const card = el("div", "scard"); pg.appendChild(card);
      for (const [f, invert, desc] of rows) {
        const forced = R.forced[f];
        const lock = forced && forced !== "restart";
        const r = el("div", "srow" + (lock ? " dis" : ""));
        r.dataset.sec = f;
        if (lock) r.setAttribute("aria-disabled", "true");
        const info = el("div", "sinfo");
        info.appendChild(el("div", "slabel", R.names[f] || f));
        info.appendChild(el("div", "sdesc", desc));
        if (lock) info.appendChild(el("div", "sdesc ssecforced", "Forced " + ((invert ? !R.effective[f] : R.effective[f]) ? "on" : "off") + " by " + (WHY[forced] || forced) + "."));
        if ((R.restartPending || []).includes(f)) info.appendChild(el("div", "sdesc ssecrestart", "Applies after restart."));
        if (f === "lockdown" && R.stored.lockdown) {
          const cb = el("label", "sdesc ssecclip");
          const box = el("input"); box.type = "checkbox"; box.checked = !!R.stored.lockdownClipboard;
          box.onchange = () => set("lockdownClipboard", box.checked);
          cb.append(box, document.createTextNode(" Also turn off the clipboard"));
          info.appendChild(cb);
        }
        r.appendChild(info);
        r.appendChild(toggle(shown(f, invert), () => set(f, invert ? shown(f, invert) : !shown(f, invert)), lock));
        card.appendChild(r);
        nRows++; if (lock) nForced++;
      }
    }
    if (typeof updateTitle === "function") updateTitle();
  }
  function tok() {
    if (!R || typeof settingsOpen === "undefined" || !settingsOpen || sPane !== "security") return "";
    const v = Object.keys(R.stored).map(f => f + "=" + (R.stored[f] ? 1 : 0) + (R.effective[f] ? 1 : 0)).join(",");
    return " [secp:" + nRows + "/" + nForced + "/" + (R.restartPending || []).length + "] [secv:" + v + "]" + geo();
  }
  /* [secg:nav=<h>/<font>/<padT>/<padL>/<radius>/<same 1|0>,row=<padT>/<padB>,lbl=<font>,desc=<font>,tg=<w>x<h>]
     the painted chrome of the Security tab, read with getComputedStyle — the T3 phase compares it to
     stock 1.13.7's settings tab (goal/sec144 logs it2d) and to the neighbouring nav entry (same=1:
     the Security entry paints exactly like Community plugins). Instrument only. */
  function geo() {
    const pg = document.getElementById("spage"), nv = document.getElementById("snav");
    if (!pg || !nv) return "";
    const px = v => String(v || "").replace(/px$/, "");
    const items = [...nv.querySelectorAll(".snavi")];
    const me = items.find(e => e.dataset.pane === "security"), nb = items.find(e => e.dataset.pane === "community");
    const nav = e => { if (!e) return "-"; const c = getComputedStyle(e); return [Math.round(e.getBoundingClientRect().height), px(c.fontSize), px(c.paddingTop), px(c.paddingLeft), px(c.borderTopLeftRadius)].join("/"); };
    const row = pg.querySelector(".srow"); if (!row) return "";
    const rc = getComputedStyle(row), lb = row.querySelector(".slabel"), ds = row.querySelector(".sdesc"), tg = row.querySelector(".sctl.toggle");
    const tb = tg ? tg.getBoundingClientRect() : { width: 0, height: 0 };
    return " [secg:nav=" + nav(me) + "/" + (nav(me) === nav(nb) ? 1 : 0) + ",row=" + px(rc.paddingTop) + "/" + px(rc.paddingBottom) +
           ",lbl=" + (lb ? px(getComputedStyle(lb).fontSize) : "-") + ",desc=" + (ds ? px(getComputedStyle(ds).fontSize) : "-") +
           ",tg=" + Math.round(tb.width) + "x" + Math.round(tb.height) + "]";
  }
  window.__osSecurity = { build, refresh, tok, get: () => R };
  refresh().catch(() => {});   // boot: the plugin-visible snapshot exists before any plugin loads
})();
