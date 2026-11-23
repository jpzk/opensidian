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
   [secv:<field>=<stored><effective>,...] (1/0 pairs), [secf:<field>=<why>/<disabled>,...] (greyed rows)
   and [secn:<cx>,<cy>] (the Allow network switch), read by the T3 phases.
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
     (toggle 3 "Allow network" reads ON while stored offline is false); lockdown stores true = on.
     A row forced by another switch (lockdown, Allow network off, canvas) shows the EFFECTIVE value. */
  const GROUPS = [
    [null, [["lockdown", false, "Sets every toggle below to its strictest value: community plugins do not run in any vault, no network, no media, no WebGL / canvas, no JIT, no math / code / diagram rendering. Your own values are kept and come back when you turn it off. Needs a restart."]]],
    ["Network", [
      ["offline", true, "The app may reach the network: the plugin and theme store, the update check, plugin requests and listeners. Off = offline mode: nothing in the app reaches the network (community plugins included: their switch below is forced off); links still open in your browser after a confirm."],
      ["remoteMedia", false, "Show images, audio and video from https:// addresses in notes. Off: a placeholder is shown and the address never reaches the page."],
      ["pluginNet", false, "Community plugins may make requests and open listeners. Off: their requests and listeners are refused, naming this switch. Needs Allow network: while that is off (offline mode) this switch is forced off; your value comes back when the network is allowed again."],
    ]],
    ["Media and rendering", [
      ["vaultMedia", false, "Render images, PDFs, audio and video from your vaults. Off: a placeholder is shown and no file reaches a decoder."],
      ["webgl", false, "Off: the graph switches to its 2D renderer at once; plugin code gets no new WebGL context (one made before keeps running until a reload)."],
      ["canvas", false, "Off: <canvas> drawing and pixel reads are refused in every frame before any page code runs (graph view, canvas image export and image paste re-encoding stop and say so). The page reloads to apply it. Limit: hardening, not a wall: WebKit has no canvas switch, so a trusted community plugin (it already has file and program access) can still draw through a same-origin worker, a frame that is still loading, or native code."],
      ["mathjax", false, "The MathJax TeX parser (loaded on demand, e.g. by a plugin calling loadMathJax). Off: its files are refused, the parser is never in memory and loadMathJax fails naming this switch; math stays plain text."],
      ["prism", false, "The Prism code tokenizer (loaded on demand, e.g. by loadPrism). Off: its files are refused, the tokenizer is never in memory and loadPrism fails naming this switch; code stays plain text."],
      ["mermaid", false, "The Mermaid diagram engine (loaded on demand: a note with a mermaid code block, or a plugin calling loadMermaid). Off: its files are refused, the engine is never in memory and loadMermaid fails naming this switch; diagrams stay plain text."],
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
  const WHY = { lockdown: "Lockdown mode", offline: "Allow network (off)", canvas: "HTML canvas", restart: "a restart" };
  const shown = (f, inv0) => { const v = !!R.stored[f]; return inv0 ? !v : v; };
  const snap = () => { try { window.opensidianSecurity = Object.freeze(Object.assign({ lockdown: !!R.lockdown }, R.effective)); } catch (_) {} };
  function publish() {
    if (!R) return;
    snap();
    marker();
    cspAgree();
  }
  /* toggle 1 layer 2: the page CSP is fixed at load (main.rs page_csp). When the effective
     "Load external resources" and this page's header disagree, reload once (after the save
     debounce, so a pending note write lands) — turning it OFF removes https: at once.
     Two reloads in a row that still disagree stop with a console warning, never a loop.
     Toggle 4 (Render media) rides the same rule: its CSP layer (opensidian-img: in img-src,
     blob: in media-src) is fixed at load too (security_get pageMedia). */
  /* toggle 11: "off" means the parser is not in memory. The 403 stops every NEW load; an
     engine a plugin loaded before the switch went off is still in this realm, so the page is
     reloaded into a fresh one (same once-per-change rule as the CSP above). */
  const ENGINE_LOADED = {
    mathjax: () => !!(window.MathJax && window.MathJax.startup && typeof window.MathJax.tex2chtml === "function"),
    prism: () => !!(window.Prism && typeof window.Prism.highlight === "function"),
    mermaid: () => !!(window.mermaid && typeof window.mermaid.render === "function"),
  };
  const engineStale = () => Object.keys(ENGINE_LOADED).filter(f => R.effective[f] === false && ENGINE_LOADED[f]());
  let reloading = false;
  function cspAgree() {
    const KEY = "osSecReload";
    if (!R || typeof R.pageRemote !== "boolean" || typeof R.pageMedia !== "boolean" || !R.effective) return;
    const stale = engineStale();
    // toggle 6: the canvas stubs are fixed at document start (security_get pageCanvas); a window built
    // with them needs a restart instead (restartPending "canvas"), a reload would not help
    const cvAgree = typeof R.pageCanvas !== "boolean" || (R.restartPending || []).includes("canvas") || !!R.effective.canvas === R.pageCanvas;
    const agree = !!R.effective.remoteMedia === R.pageRemote && !!R.effective.vaultMedia === R.pageMedia && cvAgree && !stale.length;
    if (agree) { try { sessionStorage.removeItem(KEY); } catch (_) {} return; }
    let n = 0;
    try { n = +sessionStorage.getItem(KEY) || 0; } catch (_) {}
    if (reloading) return;
    if (n >= 2) { console.warn("[sec] page CSP still disagrees with Load external resources / Render media / HTML canvas (or an engine that is off is still loaded: " + stale.join(",") + ") after 2 reloads"); return; }
    reloading = true;
    try { sessionStorage.setItem(KEY, String(n + 1)); } catch (_) {}
    console.warn("[sec] reloading the page into the CSP of Load external resources=" + !!R.effective.remoteMedia + " Render media=" + !!R.effective.vaultMedia + " HTML canvas=" + !!R.effective.canvas + (stale.length ? " (unloading " + stale.join(",") + ")" : ""));
    Promise.resolve(inv("save_debounce_ms")).catch(() => 0)
      .then(ms => setTimeout(() => location.reload(), (+ms || 0) + 300));
  }
  /* toggle 12 — Lockdown mode. Turning it ON raises every protection (security_set writes it at
     once), so this page dialog is the issue's confirm: what breaks, "also turn off the clipboard",
     restart needed + "Restart now". Turning it OFF lowers them: security_set asks the NATIVE
     confirm, then the row offers "Restart now". Restart = security_restart after leaveVault()
     flushed every buffer (the same flush a vault switch does). Census: [secl:<latched 1|0>/
     <stored 1|0>/<dialog open 1|0>]. */
  const LD_BREAKS = "Breaks on purpose until you turn it off: community plugins (in every vault), the graph, PDFs, images, audio and video embeds, math, code highlighting, Mermaid diagrams, the plugin / theme store and the update check. Notes, links, search, backlinks, canvas boards (text cards) and editing keep working. Your own Security values are kept and come back when lockdown is off.";
  let ldOpen = false;
  async function restartNow() {
    try {
      if (typeof leaveVault === "function") {
        const lost = await leaveVault();
        if (lost && lost.length) { if (typeof say === "function") say("Not restarting: " + lost.join(", ") + " could not be saved."); return; }
      }
      document.body.inert = true;
      await inv("security_restart");
    } catch (e) {
      document.body.inert = false;
      console.error("[sec] restart:", e);
      if (typeof say === "function") say("Restart: " + String(e && e.message || e));
    }
  }
  function lockdownDialog() {
    if (ldOpen) return;
    ldOpen = true;
    const box = el("div"); box.id = "ldbox"; box.className = "modal-container";
    const card = el("div", "modal"); card.id = "ldcard"; box.appendChild(card);
    card.appendChild(el("div", "ldtitle", "Turn on Lockdown mode?"));
    card.appendChild(el("div", "ldq", LD_BREAKS));
    const lab = el("label", "ldclip"); const cb = el("input"); cb.type = "checkbox"; cb.id = "ld-clip";
    cb.checked = !!(R && R.stored.lockdownClipboard);
    lab.append(cb, document.createTextNode(" Also turn off the clipboard (copy, cut and paste stop working in the app too)"));
    card.appendChild(lab);
    card.appendChild(el("div", "ldsay", "Lockdown applies after a restart: the JavaScript JIT is read only when the app starts."));
    const btns = el("div", "ldbtns"); card.appendChild(btns);
    const no = el("button", "mod-cancel", "Cancel"); no.id = "ld-no";
    const later = el("button", "", "Turn on, restart later"); later.id = "ld-later";
    const now = el("button", "mod-cta", "Turn on and restart now"); now.id = "ld-now";
    btns.append(no, later, now);
    /* PR #193 review: the dialog owns the keyboard while it is up. A WINDOW capture listener
       runs before every document handler — the settings modal's own (hkKey: Escape = close
       settings) used to take Escape whenever focus was not inside this box, leaving the dialog
       up. Escape = Cancel; Tab / Shift+Tab cycle inside the card; a key arriving with focus
       outside the card is pulled back to Cancel first. */
    const ctl = () => [cb, no, later, now];
    const keys = ev => {
      if (ev.key === "Escape") { ev.preventDefault(); ev.stopImmediatePropagation(); close(); return; }
      const c = ctl(), at = c.indexOf(document.activeElement);
      if (ev.key === "Tab") {
        ev.preventDefault(); ev.stopImmediatePropagation();
        c[at < 0 ? 1 : (at + (ev.shiftKey ? c.length - 1 : 1)) % c.length].focus();
        return;
      }
      if (at < 0) { ev.preventDefault(); ev.stopImmediatePropagation(); no.focus(); }
    };
    const close = () => {
      ldOpen = false; window.removeEventListener("keydown", keys, true); box.remove();
      if (typeof updateTitle === "function") updateTitle();
    };
    const go = async restart => {
      close();
      if (cb.checked !== !!R.stored.lockdownClipboard) await set("lockdownClipboard", cb.checked);
      await set("lockdown", true);
      if (restart && R && R.stored.lockdown) restartNow();
    };
    no.onclick = close; later.onclick = () => go(false); now.onclick = () => go(true);
    box.onmousedown = ev => { if (ev.target === box) ev.preventDefault(); };   // a scrim click keeps focus in the card
    window.addEventListener("keydown", keys, true);
    document.body.appendChild(box);
    // PR #193 review: the default (Enter) is the SAFE choice — Cancel, never "restart now"
    no.focus();
    if (typeof updateTitle === "function") updateTitle();
  }
  /* the visible marker while the process RUNS in lockdown (latched): a status-bar item + the
     window title (main.js lockTitle) */
  function marker() {
    if (!R) return;
    window.__osLockdown = !!R.lockdown;
    let m = document.getElementById("seclock");
    if (R.lockdown && !m) {
      m = el("div", "status-bar-item", "Lockdown"); m.id = "seclock";
      m.title = "Lockdown mode is on (Settings › Security › Lockdown mode)";
      // PR #193 review: a fixed pill painted over the vault name. It is its own row of the left
      // sidebar now, right above the vault bar, so the vault name keeps its full width (inside
      // the bar it squeezed the name to an ellipsis); body (fixed) only if the bar does not exist.
      const vb = document.getElementById("vbar");
      if (vb && vb.parentNode) vb.parentNode.insertBefore(m, vb);
      else document.body.appendChild(m);
    }
    if (m) m.hidden = !R.lockdown;
    try { if (typeof pushTitle === "function" && tWant) pushTitle(tWant); } catch (_) {}
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
        // forced = security.rs forced_by: lockdown, canvas, and Allow network off (offline) for
        // pluginNet even when its stored value is already off — the same greyed, disabled row
        const forced = R.forced[f] || (f === "pluginNet" && R.effective.offline ? (R.lockdown ? "lockdown" : "offline") : undefined);
        const lock = forced && forced !== "restart";
        const r = el("div", "srow" + (lock ? " dis" : ""));
        r.dataset.sec = f;
        if (lock) r.setAttribute("aria-disabled", "true");
        const info = el("div", "sinfo");
        info.appendChild(el("div", "slabel", R.names[f] || f));
        info.appendChild(el("div", "sdesc", desc));
        if (lock) info.appendChild(el("div", "sdesc ssecforced", "Forced " + ((invert ? !R.effective[f] : R.effective[f]) ? "on" : "off") + " by " + (WHY[forced] || forced) + "."));
        if ((R.restartPending || []).includes(f)) {
          const rp = el("div", "sdesc ssecrestart", "Applies after restart. ");
          const rb = el("button", "mod-cta ssecrestartbtn", "Restart now"); rb.dataset.sec = f;
          rb.onclick = ev => { ev.stopPropagation(); restartNow(); };
          rp.appendChild(rb);
          info.appendChild(rp);
        }
        if (f === "lockdown" && R.stored.lockdown) {
          const cb = el("label", "sdesc ssecclip");
          const box = el("input"); box.type = "checkbox"; box.checked = !!R.stored.lockdownClipboard;
          box.onchange = () => set("lockdownClipboard", box.checked);
          cb.append(box, document.createTextNode(" Also turn off the clipboard"));
          info.appendChild(cb);
        }
        r.appendChild(info);
        // toggle 12: turning lockdown ON goes through its confirm (what breaks, clipboard, restart)
        const press = f === "lockdown" && !R.stored.lockdown ? lockdownDialog : () => set(f, invert ? shown(f, invert) : !shown(f, invert));
        // a forced row shows what is in force (lockdown turns "Allow network" OFF), the stored value comes back with it
        r.appendChild(toggle(lock ? (invert ? !R.effective[f] : !!R.effective[f]) : shown(f, invert), press, lock));
        card.appendChild(r);
        nRows++; if (lock) nForced++;
      }
    }
    micRow(pg);
    if (typeof updateTitle === "function") updateTitle();
  }
  /* #295 (audit F-8) Microphone: "ask" (default) | "always" (#295 follow-up) | "off". Not one of the
     boolean switches above (security.rs keeps it apart; [secp:]/[secv:]/[secf:] stay theirs): its
     own row (data-secmic, not data-sec), a three-way choice, its own command security_mic_set. The
     page only ASKS for a value: a change that lowers protection (off -> ask, to always allow) is
     confirmed in the NATIVE "lower protection?" window before Rust writes it; a revoke (back to ask
     / off) applies at once. The start decision itself is native too (audio_rec_start).
     Census: [secmic:<stored>/<effective>/<forced 1|0>@<cx>,<cy>] — @ = the button the old switch
     press led to (Off, or Ask while off), so the T1 secmic cases keep their clicks — and
     [secmicv:ask@x,y,always@x,y,off@x,y] (every button's centre). Strings: locales secmic-*. */
  const I18 = window.I18N;   // captured at load, before any plugin script (a later global cannot relabel the row)
  const tr = (id, args, en) => { try { const s = I18 && I18.t(id, args); if (s && s !== id) return s; } catch (_) {} return en; };
  const MIC_VALUES = [["ask", "secmic-ask", "Ask"], ["always", "secmic-always", "Always allow"], ["off", "secmic-off", "Off"]];
  async function setMic(v) {
    if (busy) return; busy = true;
    try { R = await inv("security_mic_set", { value: v }); publish(); }
    catch (e) { console.error("[sec] set microphone:", e); if (typeof say === "function") say("Security: " + String(e && e.message || e)); }
    busy = false;
    if (typeof settingsOpen !== "undefined" && settingsOpen && sPane === "security") build(document.getElementById("spage"));
  }
  function micRow(pg) {
    const M = R.microphone;
    if (!M) return;
    // devaccess: the Microphone opens Settings › Security › Device access (it was "Privacy" in #302)
    pg.appendChild(el("div", "ssec", tr("secdev-section", null, "Device access")));
    const card = el("div", "scard"); pg.appendChild(card);
    const lock = !!M.forced;
    const r = el("div", "srow" + (lock ? " dis" : ""));
    r.dataset.secmic = "1";
    if (lock) r.setAttribute("aria-disabled", "true");
    const info = el("div", "sinfo");
    info.appendChild(el("div", "slabel", tr("secmic-label", null, M.name || "Microphone")));
    info.appendChild(el("div", "sdesc", tr("secmic-desc", null, "Ask: the Audio recorder asks in a separate system window before every recording. Always allow: recordings start without that window (you confirm this choice once, in a system window). Off: every recording is refused and no window is shown. While the microphone is on, the window title starts with \"● Recording\".")));
    if (lock) info.appendChild(el("div", "sdesc ssecforced", tr("secmic-forced", { name: WHY[M.forced] || M.forced }, "Forced off by " + (WHY[M.forced] || M.forced) + ".")));
    r.appendChild(info);
    const cur = lock ? M.effective : M.stored;
    const seg = el("div", "sctl ssecmicseg");
    seg.id = "ssecmic";
    seg.setAttribute("role", "radiogroup");
    seg.setAttribute("aria-label", tr("secmic-label", null, "Microphone"));
    for (const [v, id, en] of MIC_VALUES) {
      const b = el("button", "sbtn" + (v === cur ? " acc" : ""), tr(id, null, en));
      b.dataset.secmicv = v;
      b.setAttribute("role", "radio"); b.setAttribute("aria-checked", v === cur ? "true" : "false");
      if (lock) { b.disabled = true; b.setAttribute("aria-disabled", "true"); }
      else b.onclick = ev => { ev.stopPropagation(); if (v !== M.stored) setMic(v); };
      seg.appendChild(b);
    }
    r.appendChild(seg);
    card.appendChild(r);
    devRows(card);
  }
  /* devaccess: Settings › Security › Device access = the Microphone row above plus one row per
     device surface stock 1.13.7 offers (security.rs DEV_ROWS: camera, location, notifications,
     pointerLock), each ask | always | off through security_dev_set (Rust validates the row and the
     value; a weakening change is confirmed in the NATIVE "lower protection?" window; a revoke applies
     at once). The decision on every request is native (devperm.rs decide); this pane only asks for
     a value. Then the "not available" list (never granted). Census:
     [secdev:<field>=<stored>/<effective>/<forced 1|0>@ask:x,y|always:x,y|off:x,y;...] and
     [secdevna:<n>]. Strings: locales secdev-*. */
  const DEV_TXT = {
    camera: ["secdev-camera", "Camera", "secdev-camera-desc", "Camera access for the app and community plugins. While the camera is on, the window title starts with \"● Camera\"."],
    location: ["secdev-location", "Location", "secdev-location-desc", "Your location, for the app and community plugins."],
    notifications: ["secdev-notifications", "Notifications", "secdev-notifications-desc", "Desktop notifications from the app and community plugins."],
    pointerLock: ["secdev-pointerlock", "Pointer lock", "secdev-pointerlock-desc", "The app or a community plugin may hide and capture the mouse pointer after you click (3D views, games); Esc releases it."],
  };
  async function setDev(f, v) {
    if (busy) return; busy = true;
    try { R = await inv("security_dev_set", { field: f, value: v }); publish(); }
    catch (e) { console.error("[sec] set " + f + ":", e); if (typeof say === "function") say("Security: " + String(e && e.message || e)); }
    busy = false;
    if (typeof settingsOpen !== "undefined" && settingsOpen && sPane === "security") build(document.getElementById("spage"));
  }
  function devRows(card) {
    const D = R.device;
    if (!D || !Array.isArray(D.rows)) return;
    for (const M of D.rows) {
      const t = DEV_TXT[M.field];
      if (!t) continue;   // a row this pane does not know is not drawn (Rust still decides it)
      const lock = !!M.forced;
      const r = el("div", "srow" + (lock ? " dis" : ""));
      r.dataset.secdev = M.field;
      if (lock) r.setAttribute("aria-disabled", "true");
      const info = el("div", "sinfo");
      info.appendChild(el("div", "slabel", tr(t[0], null, t[1])));
      info.appendChild(el("div", "sdesc", tr(t[2], null, t[3]) + " " + tr("secdev-values", null, "Ask: every request is asked in a separate system window. Always allow: granted without that window (you confirm this choice once, in a system window). Off: every request is refused and no window is shown.")));
      if (lock) info.appendChild(el("div", "sdesc ssecforced", tr("secmic-forced", { name: WHY[M.forced] || M.forced }, "Forced off by " + (WHY[M.forced] || M.forced) + ".")));
      r.appendChild(info);
      const cur = lock ? M.effective : M.stored;
      const seg = el("div", "sctl ssecmicseg");
      seg.id = "ssecdev-" + M.field;
      seg.setAttribute("role", "radiogroup");
      seg.setAttribute("aria-label", tr(t[0], null, t[1]));
      for (const [v, id, en] of MIC_VALUES) {
        const b = el("button", "sbtn" + (v === cur ? " acc" : ""), tr(id, null, en));
        b.dataset.secdevv = v;
        b.setAttribute("role", "radio"); b.setAttribute("aria-checked", v === cur ? "true" : "false");
        if (lock) { b.disabled = true; b.setAttribute("aria-disabled", "true"); }
        else b.onclick = ev => { ev.stopPropagation(); if (v !== M.stored) setDev(M.field, v); };
        seg.appendChild(b);
      }
      r.appendChild(seg);
      card.appendChild(r);
    }
    const na = el("div", "srow");
    na.dataset.secdevna = String((D.notAvailable || []).length);
    const ni = el("div", "sinfo");
    ni.appendChild(el("div", "slabel", tr("secdev-na", null, "Not available")));
    ni.appendChild(el("div", "sdesc", tr("secdev-na-desc", null, "Never granted to the app or to plugins, and no window is shown: screen capture, device names, protected media (EME), website data access, WebXR, MIDI.")));
    na.appendChild(ni);
    card.appendChild(na);
  }
  function devTok() {
    const D = R && R.device;
    if (!D || !Array.isArray(D.rows) || !document.getElementById("ssecmic")) return "";
    const rows = D.rows.map(M => {
      const seg = document.getElementById("ssecdev-" + M.field);
      const c = v => { const b = seg && seg.querySelector('[data-secdevv="' + v + '"]'); if (!b) return "-"; const q = b.getBoundingClientRect(); return Math.round(q.x + q.width / 2) + "," + Math.round(q.y + q.height / 2); };
      return M.field + "=" + M.stored + "/" + M.effective + "/" + (M.forced ? 1 : 0) + "@" + MIC_VALUES.map(([v]) => v + ":" + c(v)).join("|");
    });
    const na = document.querySelector("#spage [data-secdevna]");
    return " [secdev:" + rows.join(";") + "] [secdevna:" + (na ? na.dataset.secdevna : "-") + "]";
  }
  function micTok() {
    const M = R && R.microphone, seg = document.getElementById("ssecmic");
    if (!M || !seg) return "";
    const c = v => { const b = seg.querySelector('[data-secmicv="' + v + '"]'); if (!b) return null; const q = b.getBoundingClientRect(); return Math.round(q.x + q.width / 2) + "," + Math.round(q.y + q.height / 2); };
    const legacy = c(M.stored === "off" ? "ask" : "off");
    return " [secmic:" + M.stored + "/" + M.effective + "/" + (M.forced ? 1 : 0) + "@" + legacy + "]" +
      " [secmicv:" + MIC_VALUES.map(([v]) => v + "@" + c(v)).join(",") + "]";
  }
  function tok() {
    if (!R) return "";
    let ld = " [secl:" + (R.lockdown ? 1 : 0) + "/" + (R.stored.lockdown ? 1 : 0) + "/" + (ldOpen ? 1 : 0) + "]";
    // [secd:<id of the focused control>] while the confirm is up (default focus = ld-no)
    if (ldOpen) { const a = document.activeElement; ld += " [secd:" + (a && a.id ? a.id : "-") + "]"; }
    // [secm:<marker rect>|<vault-name rect>] x,y,w,h while the marker shows: the harness intersects them
    const m = document.getElementById("seclock"), vs = document.getElementById("vswitch");
    if (R.lockdown && m && !m.hidden) {
      const r = e => { if (!e) return "-"; const b = e.getBoundingClientRect(); return [b.x, b.y, b.width, b.height].map(Math.round).join(","); };
      ld += " [secm:" + r(m) + "|" + r(vs && vs.getClientRects().length ? vs : null) + "]";
    }
    if (typeof settingsOpen === "undefined" || !settingsOpen || sPane !== "security") return R.lockdown || ldOpen ? ld : "";
    const v = Object.keys(R.stored).map(f => f + "=" + (R.stored[f] ? 1 : 0) + (R.effective[f] ? 1 : 0)).join(",");
    // [secf:<field>=<why>/<switch disabled 1|0>,...] every greyed row as drawn (pluginNet=offline/1 while
    // Allow network is off, PR #193 review); [secn:<cx>,<cy>] the centre of the Allow network switch
    const fr = [...document.querySelectorAll('#spage .srow.dis[data-sec]')].map(r => {
      const t = r.querySelector(".sctl.toggle");
      return r.dataset.sec + "=" + (R.forced[r.dataset.sec] || "-") + "/" + (t && t.getAttribute("aria-disabled") === "true" && !t.classList.contains("live") && !t.onclick ? 1 : 0);
    }).join(",");
    const nt = document.querySelector('#spage .srow[data-sec="offline"] .sctl.toggle'), nb = nt && nt.getBoundingClientRect();
    const sn = nb ? " [secn:" + Math.round(nb.x + nb.width / 2) + "," + Math.round(nb.y + nb.height / 2) + "]" : "";
    return " [secp:" + nRows + "/" + nForced + "/" + (R.restartPending || []).length + "] [secv:" + v + "] [secf:" + fr + "]" + sn + geo() + micTok() + devTok() + ld;
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
  /* security_set emits "security-changed" whoever called it (this pane, or a script invoking the
     command): the plugin-visible snapshot follows, and an engine that just went off while it is
     in the page is unloaded (toggle 11). Toggles 1 / 4 keep their reload to this pane's own
     presses; a script's change to them applies at the next load, as before. */
  try {
    T.event.listen("security-changed", () => inv("security_get").then(r => {
      R = r; snap(); marker();
      if (engineStale().length) cspAgree();
    }, () => {}));
  } catch (_) {}
  refresh().catch(() => {});   // boot: the plugin-visible snapshot exists before any plugin loads
})();
