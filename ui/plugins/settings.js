// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* plugin-loader (recon M1), ledger item 6: Settings > Community plugins.
   goal/noplugins removed the pane; M1 brings back the COMMUNITY half only
   (Core plugins stays removed, operator 2026-10-01):
     - Options > "Community plugins" (pane id "community"): a Restricted mode
       toggle (default ON = no plugin code runs; the flag lives outside the vault,
       ~/.opensidian.json plugins_trusted[<vault>], so a vault cannot ship itself
       trusted) and the installed list (<vault>/.obsidian/plugins/<id>/manifest.json)
       with one enable toggle per plugin (.obsidian/community-plugins.json,
       Obsidian's format). cp-browser: a Browse button opens the community plugin
       browser (ui/plugins/browser.js) on this pane; copying a folder in still works.
     - a "Community plugins" GROUP under it with one entry per setting tab a
       LOADED plugin registered (pane id "plg:<id>"); the group exists only while
       it has entries (no empty heading — noplugins' structural rule).
   main.js calls in at four points (buildSettingsNav, showSettingsPage, setTok,
   closeSettings); everything else lives here so cm6-editor's main.js edits do
   not collide with it. Census (only while settings is open):
     [cpp:<R|T>,<installed>,<enabled>]   on the community pane
     [cpr:<x>,<y>,<1 restricted|0>]      the restricted toggle's centre
     [cpbb:<x>,<y>]                      the Browse button; [cpb...] browser.js tok() while browsing
     [cpa:<grants>,<outdated>,<seq>]    #74 Allowed programs list answered; [cpar:<x>,<y>] its first Revoke
     [cpt=<id>:<x>,<y>,<on>,<loaded>,<gear x|0>]   one per installed plugin
     [cpn=<pane id>:<x>,<y>]             every community nav entry's centre
     [ptab:<id>,<setting-items>,<elements>]  a plugin tab is on screen (display() ran) */
"use strict";
(function () {
  /* gh-icons #20: monochrome inline svg icons (drawn for this app, currentColor) in place of
     text pictographs / emoji; class "gi" sizes them (style.css / plugins.css) */
  const I = d => '<svg class="gi" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">' + d + "</svg>";
  const GI = {
    gear: I('<circle cx="8" cy="8" r="2"/><circle cx="8" cy="8" r="4.4"/><path d="M8 1.6v2M8 12.4v2M1.6 8h2M12.4 8h2M3.5 3.5l1.4 1.4M11.1 11.1l1.4 1.4M3.5 12.5l1.4-1.4M11.1 4.9l1.4-1.4"/>'),
    check: I('<path d="M3.5 8.5l3 3 6-7"/>'),
  };
  const host = window.__osPluginHost;
  if (!host || !host.app) return;
  const setting = host.app.setting;
  let busy = false;           // one toggle round-trip at a time (a double click is one intent)
  /* cp-browser: the community plugin browser (ui/plugins/browser.js) is written into the page right
     after this script, while index.html is still parsing (before the loader's seal), and hands itself
     back through adoptBrowser() once. browsing = it owns #spage on the community pane. */
  let browser = null, browsing = false;
  if (document.readyState === "loading") document.write('<script src="plugins/browser.js"><\/script>');
  function adoptBrowser(b) {
    // only while index.html is parsing: no plugin code has run yet (plugins load after the vault opens)
    if (browser || document.readyState !== "loading" || !b || typeof b.open !== "function") return false;
    browser = b;
    return true;
  }
  const browserCtx = Object.freeze({
    host,
    page: () => (typeof settingsOpen !== "undefined" && settingsOpen && sPane === "community" && browsing ? document.getElementById("spage") : null),
    back: () => { stopBrowsing(); rebuild(); },
    title: () => updateTitle(),
  });
  function stopBrowsing() { if (browsing && browser) browser.close(); browsing = false; }
  function startBrowsing() {
    if (!browser) return;
    browsing = true;
    const pg = document.getElementById("spage"); pg.innerHTML = "";
    browser.open(browserCtx, true);
  }

  const el = (tag, cls, txt) => { const d = document.createElement(tag); if (cls) d.className = cls; if (txt != null) d.textContent = txt; return d; };
  const tabOf = id => setting.pluginTabs.find(t => t && t.id === id) || null;

  function toggle(on, onclick) {
    const t = el("div", "sctl toggle live" + (on ? " on" : ""));
    t.appendChild(el("i"));
    t.tabIndex = 0; t.setAttribute("role", "switch"); t.setAttribute("aria-checked", on ? "true" : "false");
    t.onclick = ev => { ev.stopPropagation(); onclick(); };
    t.onkeydown = ev => { if (ev.key === " " || ev.key === "Enter") { ev.preventDefault(); onclick(); } };
    return t;
  }
  function row(card, label, desc, ctls, dis) {
    const r = el("div", "srow" + (dis ? " dis" : ""));
    if (dis) r.setAttribute("aria-disabled", "true");
    const info = el("div", "sinfo");
    info.appendChild(el("div", "slabel", label));
    for (const d of [].concat(desc || [])) if (d) info.appendChild(el("div", "sdesc", d));
    r.appendChild(info);
    for (const c of ctls || []) r.appendChild(c);
    card.appendChild(r);
    return r;
  }

  /* ---- nav: Options > Community plugins, then the per-plugin group ---- */
  function nav(n) {
    for (const d of [...n.querySelectorAll("[data-cp]")]) d.remove();
    const entry = (pane, text, icon) => {
      const d = el("div", "snavi", text);
      /* top5-func: a plugin tab that set SettingTab.icon shows it before its name (data-icon = the name) */
      if (icon != null && icon !== "" && host.api && typeof host.api.setIcon === "function") {
        const i = el("span", "snavi-icon"); host.api.setIcon(i, String(icon)); d.insertBefore(i, d.firstChild);
      }
      d.dataset.pane = pane; d.dataset.cp = "1"; d.tabIndex = 0;
      d.onclick = () => showSettingsPage(pane);
      d.onkeydown = ev => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); showSettingsPage(pane); } };
      if (typeof sPane !== "undefined" && sPane === pane) d.classList.add("sel");
      n.appendChild(d);
    };
    // af-fix 2b (operator rule 2026-10-01, noplugins NP_WANT): no Core plugins entry, no CORE PLUGINS group,
    // no Canvas entry — the Canvas settings pane opens from the board's own settings control (canvas-nav.js)
    entry("community", "Community plugins");
    const tabs = setting.pluginTabs.filter(t => t && t.id);
    if (!tabs.length) return;
    const h = el("div", "snavh", "Community plugins"); h.dataset.cp = "1";
    n.appendChild(h);
    for (const t of tabs.slice().sort((a, b) => String(a.name).localeCompare(String(b.name))))
      entry("plg:" + t.id, String(t.name || t.id), t.icon);
  }
  /* a plugin added/removed a setting tab: the group follows (nav is otherwise built once). A tab
     removed while on screen was hidden by removeSettingTab (activeTab null); like stock, the page
     stays where it is (the next open() goes to General, a nav click anywhere) */
  function tabsChanged() {
    const n = document.getElementById("snav");
    if (n && n.dataset.built === "1") nav(n);
  }

  function leaveTab() { if (setting.activeTab) setting.closeActiveTab(); }
  /* closeSettings: the active tab is hidden and a plugin tab's containerEl leaves the page */
  function closeUi() {
    const t = setting.activeTab;
    leaveTab();
    if (t && t._pane === undefined && t.containerEl && t.containerEl.parentNode === document.getElementById("spage")) t.containerEl.remove();
  }

  /* ---- panes ----
     Every pane is a tab of app.setting (api-ui.js ui:setting): the app's own panes are its built-in
     tabs (settingTabs, pane id in _pane), a loaded plugin's tab is "plg:<id>". A nav click or
     showSettingsPage goes through app.setting.openTab, so activeTab / lastTabId / hide() / display()
     are the same whether the user or a plugin switched the pane. */
  let inBuiltin = false;      // main.js is building one of its own panes for a built-in tab
  const builtinOf = pane => setting.settingTabs.find(t => t._pane === pane) || null;
  const paneOf = t => t._pane !== undefined ? t._pane : "plg:" + t.id;
  function selNav(id) { for (const d of document.getElementById("snav").querySelectorAll(".snavi")) d.classList.toggle("sel", d.dataset.pane === id); }
  function clearPage(id, cls) {
    stopBrowsing();
    fontPage = null; sPane = id; selNav(id);
    const pg = document.getElementById("spage"); pg.innerHTML = ""; pg.className = cls;
    sRowsShown = 0; sEnabledShown = 0;
    return pg;
  }
  /* showSettingsPage(id) asks here first: true = handled */
  function show(id) {
    if (inBuiltin) return false;
    if (id === "core-canvas" && canvasPlugin()) return showCorePane(id);   // goal cvf-board; af-fix 2b: no nav entry, opened by the board gear
    closeSpMenu();
    let t;
    if (id === "community") t = builtinOf("community");
    else if (String(id).startsWith("plg:")) t = tabOf(id.slice(4)) || builtinOf("community");   // its plugin unloaded since
    else {
      const e = typeof SMODEL !== "undefined" && SMODEL ? SMODEL.nav.find(n => n.id === id) || SMODEL.nav.find(n => n.entry === id) : null;
      t = builtinOf(e ? e.id : "hotkeys");     // main.js's own fallback pane
    }
    if (!t) { leaveTab(); return false; }
    try { setting.openTab(t); } catch (e) {
      console.error("[plugin] " + t.id + " settings display:", e);
      const pg = document.getElementById("spage");
      if (pg && t._pane === undefined) pg.appendChild(el("div", "sempty", "This plugin's settings failed to render: " + (e && e.message || e)));
    }
    if (t._pane === undefined || t._pane === "community") updateTitle();   // main.js's panes update the census themselves
    return true;
  }
  /* app.setting.open(): the pane the modal opens on */
  function pane(t) { sPane = paneOf(t); }
  /* app.setting.openTab(t), before t.display(): a plugin tab goes on the page when the modal is open.
     Its containerEl goes on as is: stock never empties it around open/display/hide, so children the
     tab's constructor built there (a header + content pair display() fills) stay; display() clears itself */
  function mount(t) {
    pane(t);
    if (t._pane !== undefined || typeof settingsOpen === "undefined" || !settingsOpen) return;
    const pg = clearPage(sPane, "rows os-plugin-tab");
    pg.appendChild(t.containerEl);
    // api-settings: a declarative tab (getSettingDefinitions) refreshes its settingItems on every open
    try { if (typeof t.update === "function") t.update(); } catch (e) { console.error("[plugin] " + t.id + " settings update:", e); }
  }
  /* a built-in tab's display(): its pane, built by main.js (or the community pane, built here) */
  function builtin(t) {
    if (typeof settingsOpen === "undefined" || !settingsOpen) return;
    if (t._pane === "community") {
      const pg = clearPage("community", "rows");
      buildCommunity(pg);
      host.refresh().then(() => { if (settingsOpen && sPane === "community") rebuild(); }, () => {});
      return;
    }
    inBuiltin = true;
    try { showSettingsPage(t._pane); } finally { inBuiltin = false; }
  }
  function rebuild() {
    const pg = document.getElementById("spage"); if (!pg) return;
    if (browsing && browser) { browser.open(browserCtx, false); return; }
    pg.innerHTML = ""; pg.className = "rows"; buildCommunity(pg); updateTitle();
  }
  function buildCommunity(pg) {
    const s = host.state();
    const restricted = !s.trusted;
    const c1 = el("div", "scard"); pg.appendChild(c1);
    const rt = toggle(restricted, () => act(() => host.setRestricted(!restricted)));
    rt.id = "cprestrict";
    row(c1, "Restricted mode", [
      restricted ? "On: no community plugin code runs in this vault." : "Off: enabled community plugins run with this app's access to your vault.",
      "Plugins are third-party code. Turn restricted mode off only for plugins you trust."], [rt]);
    if (browser) {
      const b = el("button", "cpb-btn cpb-cta", "Browse");
      b.id = "cpbrowse"; b.type = "button";
      b.onclick = ev => { ev.stopPropagation(); startBrowsing(); };
      row(c1, "Community plugin list", "Find plugins in the public community list, then install, update or remove them here.", [b]);
    }
    pg.appendChild(el("div", "ssec", "Installed plugins"));
    const c2 = el("div", "scard"); pg.appendChild(c2);
    const ids = Object.keys(s.installed).sort((a, b) => String(s.installed[a].name || a).localeCompare(String(s.installed[b].name || b)));
    if (!ids.length) {
      row(c2, "No community plugins installed", "Copy a plugin folder (main.js + manifest.json) into .obsidian/plugins/ in this vault.", [], true);
    }
    for (const id of ids) {
      const m = s.installed[id] || {};
      const on = s.enabled.includes(id), loaded = s.loaded.includes(id);
      const st = host.status.get(id);
      const ctls = [];
      if (loaded && tabOf(id)) {
        const g = el("div", "sctl cpgear"); g.innerHTML = GI.gear; g.title = "Options"; g.tabIndex = 0;
        g.onclick = ev => { ev.stopPropagation(); showSettingsPage("plg:" + id); };
        ctls.push(g);
      }
      const t = toggle(on, () => act(() => host.setEnabled(id, !on)));
      t.dataset.plugin = id;
      ctls.push(t);
      const desc = [
        "Version " + (m.version || "?") + (m.author ? " · By " + m.author : ""),
        m.description || "",
        st && st.state === "failed" ? "Failed to load: " + String(st.error || "") : (restricted && on ? "Enabled, not running: restricted mode is on." : ""),
      ];
      const r = row(c2, String(m.name || id), desc, ctls, restricted);
      r.dataset.plugin = id;
    }
    /* goal canvas-core: the "Core plugins" section (the Core plugins nav entry stays removed,
       goal/noplugins): one row per core plugin this app implements — today Canvas. Its toggle is
       the plugin's enable()/disable(): the canvas view type + .canvas extension come and go with it,
       and the choice persists app-wide. Not gated by restricted mode: it is app code, not a plugin. */
    const core = coreList();
    if (core.length) {
      pg.appendChild(el("div", "ssec", "Core plugins"));
      const c3 = el("div", "scard"); pg.appendChild(c3);
      for (const [cid, p, name, desc] of core) {
        const on = !!p.enabled;
        const t = toggle(on, () => act(() => (cid === "canvas" ? setCanvasOn(!on) : on ? p.disable() : p.enable())));
        t.dataset.core = cid;
        const r = row(c3, name, desc, [t]);
        r.dataset.core = cid;
      }
    }
    buildAllowed(pg);
    sRowsShown = 1 + (browser ? 1 : 0) + ids.length + core.length;
    sEnabledShown = (restricted ? 0 : ids.length) + 1 + (browser ? 1 : 0) + core.length;
  }
  /* #74 "Allowed programs": every "Always allow this program" grant (all vaults), listed from
     the backend's spawn_allow_list — vault, plugin + version, program, date, and "outdated" when
     the plugin's main.js / manifest.json or the program file no longer match (an outdated grant
     is never used; the next spawn asks again). Revoke = spawn_allow_revoke. There is NO add:
     a grant is made only by the native spawn dialog's Always button, never from this page.
     The backend's ALWAYS_NOTE (arguments are not checked again) is shown above the list.
     Its rows are not settings rows: sRowsShown / sEnabledShown stay what they were.
     Census: [cpa:<grants>,<outdated>,<seq>] on the community pane once the list answered, and
     [cpar:<x>,<y>] = the centre of the first Revoke button (the harness clicks it). */
  let saList = null, saSeq = 0, saBusy = false;
  function buildAllowed(pg) {
    if (typeof inv !== "function") return;
    const sec = el("div", "ssec", "Allowed programs"); sec.id = "cpallowh";
    const note = el("div", "sdesc cpa-note"); note.id = "cpallownote";
    const card = el("div", "scard"); card.id = "cpallow";
    pg.append(sec, note, card);
    const paint = r => {
      if (!card.isConnected) return;
      note.textContent = String(r && r.note || "");
      card.innerHTML = "";
      const gs = Array.isArray(r && r.grants) ? r.grants : [];
      if (!gs.length) row(card, "No programs are always allowed", "Answer “Always allow this program” in a plugin's run dialog to add one here.", [], true);
      for (const g of gs) {
        const dt = new Date(Number(g.added) * 1000); // a hand-edited u64 past the JS date range = Invalid Date, not a throw
        const day = g.added && !isNaN(dt.getTime()) ? dt.toISOString().slice(0, 10) : "?";
        const st = g.status === "outdated" ? "Outdated: the plugin or the program changed, this grant is not used" : g.status === "unknown" ? "Cannot be checked from this window" : "";
        const b = el("button", "cpb-btn cpa-revoke", "Revoke"); b.type = "button";
        b.onclick = ev => { ev.stopPropagation(); revokeAllowed(g); };
        const r2 = row(card, String(g.program || "?"), [
          "Plugin " + String(g.plugin || "?") + (g.pluginVersion ? " (version " + g.pluginVersion + ")" : "") + " · Vault " + String(g.vault || "?"),
          "Allowed " + day, st], [b]);
        r2.dataset.grant = String(g.plugin) + "|" + String(g.program);
        r2.dataset.status = String(g.status || "");
      }
      saList = gs; saSeq++; updateTitle();
    };
    inv("spawn_allow_list").then(paint, e => { note.textContent = "Allowed programs: " + String(e && e.message || e); });
  }
  async function revokeAllowed(g) {
    if (saBusy) return; saBusy = true;
    try { await inv("spawn_allow_revoke", { vault: g.vault, plugin: g.plugin, program: g.program }); }
    catch (e) { console.error("[plugin] revoke:", e); }
    saBusy = false;
    if (settingsOpen && sPane === "community") rebuild();
  }
  const CORE_DESC = { canvas: ["Canvas", "Open and edit .canvas files (JSON Canvas): text, note, link and group cards on a board."] };
  function coreList() {
    const ip = host.app.internalPlugins, ps = ip && ip.plugins || {};
    return Object.keys(CORE_DESC).filter(id => ps[id] && typeof ps[id].enable === "function")
      .map(id => [id, ps[id], CORE_DESC[id][0], CORE_DESC[id][1]]);
  }
  async function act(f) {
    if (busy) return; busy = true;
    try { await f(); } catch (e) { console.error("[plugin] settings:", e); }
    busy = false;
    if (settingsOpen && sPane === "community") rebuild();
    else if (settingsOpen && sPane === "core-canvas") showCorePane(sPane);   // goal cvf-board
    if (typeof refreshTree === "function") refreshTree().catch(() => {});   // canvas on/off: .canvas rows follow
    const n = document.getElementById("snav"); if (n && n.dataset.built === "1") nav(n);
  }
  /* ---- goal cvf-board: the Canvas core plugin's vault config + its settings pane ----
     <vault>/.obsidian/canvas.json (the Canvas page below, the board's snap toggles) and
     <vault>/.obsidian/core-plugins.json ({"canvas": bool}: honoured at launch, written by the Community plugins pane switch), stock's files and
     key names, read once when the vault is known and merged through the backend (settings.rs VaultCfg,
     reached via settings_model's vcfg argument; works in a restricted vault — this is app code).
     window.__osCanvasCfg is the one reader the canvas plugins use (canvas.js wire(), canvas-nav.js):
       get(key) -> the value or its default; set(patch) -> merge + write; on(fn) -> fn(changedKeys).
     Pane "core-canvas" (af-fix 2b: no Settings nav entry; the board settings control opens it via
     openCanvasSettings). Census while settings is open:
       [cvsp=<ctl>:x,y,<value>]   loc locf wheel cdrag names grid obj zth zthreset
       [cvspopt=<value>:x,y]      the open dropdown's options */
  /* goal cvf-create CVR-A-010/011 (ruling 03:00Z): stock leaves newFileLocation unset = vault root. Off while phase
     cvvault CV-107 asserts same-folder (operator go list); flipping it is this one line. */
  const CV_NEWLOC_STOCK = true;
  const CV_DEF = Object.freeze({
    newFileLocation: CV_NEWLOC_STOCK ? "root" : "current", newFileFolderPath: "", defaultWheelBehavior: "pan", defaultModDragBehavior: "menu",
    showCardNames: "always", zoomThreshold: 100, snapToGrid: true, snapToObjects: true,
  });
  const cvc = { canvas: {}, core: {}, loaded: false, subs: [] };
  const vinv = (file, patch) => {
    if (typeof inv !== "function") return Promise.reject(new Error("no backend"));
    return inv("settings_model", { vcfg: patch === undefined ? { file } : { file, patch } });
  };
  const say_ = m => { try { if (typeof say === "function") say(m, "canvas"); } catch (_) {} };
  // goal cvf-create (ruling 03:00Z): same-folder is "current" (stock's name); "same" (written today, see below) reads as "current"
  const cvRaw = k => (Object.prototype.hasOwnProperty.call(cvc.canvas, k) && cvc.canvas[k] != null ? cvc.canvas[k] : CV_DEF[k]);
  const cvNorm = (k, v) => (k === "newFileLocation" && v === "same" ? "current" : v);
  const cvGet = k => cvNorm(k, cvRaw(k));
  const cvEmit = keys => { for (const f of cvc.subs) { try { f(keys); } catch (e) { console.error("[canvas] cfg listener:", e); } } };
  // writes go out in order, one at a time (a slider drag is many patches; the file holds the last)
  let cvQ = Promise.resolve();
  function cvSet(patch) {
    const keys = Object.keys(patch || {}).filter(k => k in CV_DEF);
    if (!keys.length) return Promise.resolve();
    const p = {}; for (const k of keys) p[k] = patch[k];
    Object.assign(cvc.canvas, p);
    cvEmit(keys);
    cvQ = cvQ.then(() => vinv("canvas", p)).then(v => { if (v && typeof v === "object") cvc.canvas = v; },
      e => say_("Canvas settings: " + String(e && e.message || e)));
    return cvQ;
  }
  Object.defineProperty(window, "__osCanvasCfg", { value: Object.freeze({
    get: cvGet, set: cvSet, loaded: () => cvc.loaded,
    on(f) { if (typeof f === "function") cvc.subs.push(f); },
  }), writable: false, configurable: false });

  const canvasPlugin = () => { const ip = host.app.internalPlugins; return ip && ip.plugins && ip.plugins.canvas || null; };
  /* the Canvas switch: the plugin's enable()/disable() (view type, .canvas rows, ribbon follow) and the
     vault's core-plugins.json, written at once like stock */
  async function setCanvasOn(on) {
    const p = canvasPlugin(); if (!p) return;
    await (on ? p.enable() : p.disable());
    cvc.core.canvas = !!on;
    try { const v = await vinv("core-plugins", { canvas: !!on }); if (v && typeof v === "object") cvc.core = v; }
    catch (e) { say_("Core plugins: " + String(e && e.message || e)); }
  }
  /* boot: once main.js knows the vault (vaultPath, a bare global), read both files; a core-plugins.json
     that names canvas decides on/off for THIS vault (stock: per vault), else the app-wide choice stays */
  (function waitVault() {
    const vp = typeof vaultPath !== "undefined" ? vaultPath : null;
    if (!vp) { setTimeout(waitVault, 150); return; }
    Promise.all([vinv("canvas").catch(() => ({})), vinv("core-plugins").catch(() => ({}))]).then(async ([cv, core]) => {
      cvc.canvas = cv && typeof cv === "object" ? cv : {};
      cvc.core = core && typeof core === "object" ? core : {};
      cvc.loaded = true;
      cvEmit(Object.keys(CV_DEF));
      const p = canvasPlugin();
      if (p && typeof cvc.core.canvas === "boolean" && !!p.enabled !== cvc.core.canvas) {
        try { await (cvc.core.canvas ? p.enable() : p.disable()); } catch (e) { console.error("[canvas] core-plugins.json:", e); }
        if (typeof refreshTree === "function") refreshTree().catch(() => {});
      }
      if (typeof updateTitle === "function") updateTitle();
    });
  })();

  const SVG = (paths, extra) => '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"' + (extra || "") + ">" + paths + "</svg>";
  function iconBtn(cls, svg, title, onclick) {
    const b = el("div", "sctl " + cls); b.innerHTML = svg; b.title = title; b.tabIndex = 0;
    b.setAttribute("role", "button"); b.setAttribute("aria-label", title);
    b.style.cssText = "cursor:pointer;width:24px;height:24px;justify-content:center;border-radius:4px";
    b.onclick = ev => { ev.stopPropagation(); onclick(); };
    b.onkeydown = ev => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); onclick(); } };
    return b;
  }

  /* the dropdown: our own small menu under the control (no native popup — the census sees its options) */
  let spMenu = null;
  function closeSpMenu() { if (spMenu) { spMenu.remove(); spMenu = null; document.removeEventListener("mousedown", spAway, true); } }
  function spAway(ev) { if (spMenu && !spMenu.contains(ev.target)) closeSpMenu(); }
  function dropdown(ctl, key, opts) {
    const d = el("div", "sctl dropdown live");
    // an option may carry a stored value apart from its id: [id, label, stored] (cvf-create: loc "same" is written "current")
    const sv = o => (o.length > 2 ? o[2] : o[0]);
    const curV = String(cvGet(key));
    const cur = (opts.find(o => cvNorm(key, sv(o)) === curV) || [curV])[0];
    const lab = (opts.find(o => o[0] === cur) || opts[0])[1];
    d.textContent = lab; d.title = lab; d.style.width = "220px";
    d.dataset.cvsp = ctl; d.dataset.val = cur; d.tabIndex = 0;
    d.setAttribute("role", "button"); d.setAttribute("aria-haspopup", "listbox");
    const open = ev => {
      ev.preventDefault(); ev.stopPropagation();
      if (spMenu) { closeSpMenu(); return; }
      const b = d.getBoundingClientRect();
      const m = el("div", "cvsp-menu");
      m.setAttribute("role", "listbox");
      m.style.cssText = "position:fixed;z-index:200;left:" + Math.round(b.left) + "px;top:" + Math.round(b.bottom + 2) + "px;min-width:" +
        Math.round(b.width) + "px;background:var(--bg-base);border:1px solid var(--border-strong);border-radius:6px;padding:4px;box-shadow:0 4px 14px var(--shadow-popover);font-size:13px;color:var(--text-chrome)";
      for (const o of opts) {
        const [v, t] = o;
        const it = el("div", "cvsp-opt", t);
        if (v === cur) { const k = el("span", "cvsp-chk"); k.innerHTML = GI.check; it.prepend(k); }   // gh-icons: svg check mark, not a text glyph
        it.dataset.v = v; it.setAttribute("role", "option");
        it.style.cssText = "padding:4px 10px;border-radius:4px;cursor:pointer;white-space:nowrap";
        it.onmouseenter = () => { it.style.background = "var(--bg-surface)"; };
        it.onmouseleave = () => { it.style.background = ""; };
        it.onmousedown = e => e.stopPropagation();
        it.onclick = e => { e.stopPropagation(); closeSpMenu(); cvSet({ [key]: sv(o) }); rebuildCanvasPage(); };
        m.appendChild(it);
      }
      document.body.appendChild(m); spMenu = m;
      document.addEventListener("mousedown", spAway, true);
      if (typeof updateTitle === "function") updateTitle();
    };
    d.onmousedown = ev => ev.stopPropagation();
    d.onclick = open;
    d.onkeydown = ev => { if (ev.key === "Enter" || ev.key === " ") open(ev); };
    return d;
  }
  function rebuildCanvasPage() {
    if (typeof settingsOpen === "undefined" || !settingsOpen || sPane !== "core-canvas") return;
    const pg = document.getElementById("spage"); if (!pg) return;
    const st = pg.scrollTop; pg.innerHTML = ""; buildCanvasPage(pg); pg.scrollTop = st;
    if (typeof updateTitle === "function") updateTitle();
  }
  function buildCanvasPage(pg) {
    const c = el("div", "scard"); pg.appendChild(c);
    let n = 0;
    const r = (label, desc, ctls) => { n++; return row(c, label, desc, ctls); };
    r("Default location for new canvas files", "Where a new canvas goes when you create one from the palette or the ribbon.",
      [dropdown("loc", "newFileLocation", [["same", "Same folder as current file", "current"], ["root", "Vault folder"], ["folder", "In the folder specified below"]])]);
    if (cvGet("newFileLocation") === "folder") {
      const f = el("input", "sctl text live");
      f.type = "text"; f.spellcheck = false; f.autocomplete = "off"; f.placeholder = "Example: folder/sub";
      f.value = String(cvGet("newFileFolderPath") || ""); f.dataset.cvsp = "locf";
      f.style.cssText = "width:220px;color:var(--text-chrome);background:var(--bg-surface)";
      f.onmousedown = ev => ev.stopPropagation();
      f.onkeydown = ev => { if (ev.key === "Escape") { f.blur(); return; } ev.stopPropagation(); if (ev.key === "Enter") f.blur(); };   // Escape still closes Settings
      f.onchange = () => { cvSet({ newFileFolderPath: f.value.trim().replace(/^\/+|\/+$/g, "") }); if (typeof updateTitle === "function") updateTitle(); };
      f.oninput = () => { if (typeof updateTitle === "function") updateTitle(); };
      r("Folder for new canvas files", "New canvas files are created in this folder.", [f]);
    }
    r("Default mouse wheel behavior", "What a plain mouse wheel does on a board. Hold Ctrl for the other one.",
      [dropdown("wheel", "defaultWheelBehavior", [["pan", "Pan the board"], ["zoom", "Zoom the board"]])]);
    r("Default Ctrl + Drag behavior", "What a Ctrl + drag on an empty part of the board does.",
      [dropdown("cdrag", "defaultModDragBehavior", [["menu", "Show the add menu"], ["text", "Add a text card"], ["note", "Add a note from the vault"],
        ["media", "Add media from the vault"], ["web", "Add a web page"], ["group", "Create a group"]])]);
    r("Show card names", "When to show a file card's name above it.",
      [dropdown("names", "showCardNames", [["always", "Always"], ["hover", "On hover"], ["never", "Never"]])]);
    const tg = (ctl, key) => { const t = toggle(!!cvGet(key), () => { cvSet({ [key]: !cvGet(key) }); rebuildCanvasPage(); }); t.dataset.cvsp = ctl; return t; };
    r("Snap to grid", "Moved and resized cards line up with the board's grid.", [tg("grid", "snapToGrid")]);
    r("Snap to objects", "Moved and resized cards line up with the edges and centres of nearby cards.", [tg("obj", "snapToObjects")]);
    const s = el("input", "cvsp-range"); s.type = "range"; s.min = "0"; s.max = "400"; s.step = "10";
    s.value = String(cvGet("zoomThreshold")); s.dataset.cvsp = "zth"; s.title = s.value + "%";
    s.style.cssText = "width:160px;accent-color:var(--accent-mauve, currentColor);cursor:pointer";
    const val = el("span", "cvsp-val", s.value + "%"); val.style.cssText = "min-width:42px;text-align:right;font-variant-numeric:tabular-nums";
    s.onmousedown = ev => ev.stopPropagation();
    s.onkeydown = ev => { if (ev.key !== "Escape") ev.stopPropagation(); };   // arrows/Home/End stay on the slider; Escape closes Settings
    s.oninput = () => { val.textContent = s.value + "%"; s.title = s.value + "%"; cvSet({ zoomThreshold: Number(s.value) }); if (typeof updateTitle === "function") updateTitle(); };
    const rs = iconBtn("cvsp-reset", SVG('<path d="M3 12a9 9 0 1 0 3-6.7"/><path d="M3 4v5h5"/>'), "Restore default", () => {
      cvSet({ zoomThreshold: CV_DEF.zoomThreshold }); rebuildCanvasPage();
    });
    rs.dataset.cvsp = "zthreset";
    const box = el("div", "sctl"); box.append(val, s, rs);
    r("Zoom threshold for hiding card content", "Below this zoom level cards show only their outline (100% is the default).", [box]);
    sRowsShown = n; sEnabledShown = n;
  }
  function showCorePane(id) {
    leaveTab(); closeSpMenu();
    const pg = clearPage(id, "rows");
    // af-fix 2b: no extra heading — the page keeps develop's layout so its last row (zoom threshold) stays on screen
    buildCanvasPage(pg);
    if (typeof updateTitle === "function") updateTitle();
    return true;
  }
  /* af-fix 2b: the board's settings control (canvas-nav.js) opens Settings on the Canvas pane; closing
     Settings puts back the pane it had before, so ctrl+, never lands on a pane the nav does not list */
  let cvPrevPane = null;
  async function openCanvasSettings() {
    if (!canvasPlugin() || typeof openSettings !== "function") return false;
    if (typeof settingsOpen !== "undefined" && settingsOpen) { if (sPane !== "core-canvas") cvPrevPane = sPane; showCorePane("core-canvas"); return true; }
    if (sPane !== "core-canvas") cvPrevPane = sPane;
    sPane = "core-canvas";
    await openSettings();
    return true;
  }
  function leaveCanvasPane() { if (sPane === "core-canvas") { sPane = cvPrevPane || "general"; cvPrevPane = null; } }
  function coreTok() {
    let t = "";
    if (sPane === "core-canvas") {
      for (const d of document.querySelectorAll("#spage [data-cvsp]")) {
        const k = d.dataset.cvsp;
        const v = k === "grid" ? (cvGet("snapToGrid") ? 1 : 0) : k === "obj" ? (cvGet("snapToObjects") ? 1 : 0)
          : k === "zth" ? d.value : k === "locf" ? d.value : k === "zthreset" ? "" : d.dataset.val;
        t += " [cvsp=" + k + ":" + ctr(d) + "," + String(v).replace(/[\]|]/g, "") + "]";
      }
      if (spMenu && spMenu.isConnected) for (const it of spMenu.querySelectorAll(".cvsp-opt")) t += " [cvspopt=" + it.dataset.v + ":" + ctr(it) + "]";
    }
    return t;
  }
  /* ---- census ---- */
  const ctr = d => { const b = d.getBoundingClientRect(); return Math.round(b.left + b.width / 2) + "," + Math.round(b.top + b.height / 2); };
  function tok() {
    if (typeof settingsOpen === "undefined" || !settingsOpen) return "";
    let t = "";
    const n = document.getElementById("snav");
    if (n) for (const d of n.querySelectorAll(".snavi[data-cp]")) t += " [cpn=" + d.dataset.pane + ":" + ctr(d) + "]";
    if (sPane === "community") {
      const s = host.state();
      t += " [cpp:" + (s.trusted ? "T" : "R") + "," + Object.keys(s.installed).length + "," + s.enabled.length + "]";
      const r = document.getElementById("cprestrict");
      if (r) t += " [cpr:" + ctr(r) + "," + (r.classList.contains("on") ? 1 : 0) + "]";
      const bb = document.getElementById("cpbrowse");
      if (bb) t += " [cpbb:" + ctr(bb) + "]";
      if (browsing && browser) t += browser.tok();
      const rv = document.querySelector("#cpallow .cpa-revoke");
      if (saList && rv) t += " [cpar:" + ctr(rv) + "]";
      if (saList && document.getElementById("cpallow")) t += " [cpa:" + saList.length + "," + saList.filter(g => g.status === "outdated").length + "," + saSeq + "]";
      for (const row of document.querySelectorAll("#spage .srow[data-plugin]")) {
        const id = row.dataset.plugin, tg = row.querySelector(".toggle"), g = row.querySelector(".cpgear");
        t += " [cpt=" + id + ":" + ctr(tg) + "," + (tg.classList.contains("on") ? 1 : 0) + "," +
             (s.loaded.includes(id) ? 1 : 0) + "," + (g ? ctr(g).split(",")[0] : 0) + "]";
      }
      for (const tg of document.querySelectorAll("#spage .toggle[data-core]")) {
        const p = (host.app.internalPlugins.plugins || {})[tg.dataset.core];
        t += " [cpc=" + tg.dataset.core + ":" + ctr(tg) + "," + (tg.classList.contains("on") ? 1 : 0) + "," + (p && p._loaded ? 1 : 0) + "]";
      }
    } else if (sPane === "core-canvas") {
      t += coreTok();
    } else if (String(sPane).startsWith("plg:") && setting.activeTab && setting.activeTab.containerEl.isConnected) {
      const ce = setting.activeTab.containerEl;
      t += " [ptab:" + setting.activeTab.id + "," + ce.querySelectorAll(".setting-item").length + "," + ce.querySelectorAll("*").length + "]";
    }
    return t;
  }

  /* kept for older callers: app.setting.openTabById */
  const openTab = id => setting.openTabById(id);

  const api = Object.freeze({ nav, show, tok, tabsChanged, close: () => { stopBrowsing(); closeSpMenu(); closeUi(); leaveCanvasPane(); }, openTab, openCanvasSettings, pane, mount, builtin, adoptBrowser });
  Object.defineProperty(window, "__osPluginSettings", { value: api, writable: false, configurable: false });
  Object.defineProperty(window, "openPluginSettingTab", { value: openTab, writable: false, configurable: false });
})();
