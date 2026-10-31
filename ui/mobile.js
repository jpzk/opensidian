// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
// This program comes with ABSOLUTELY NO WARRANTY. It is free software, and you
// are welcome to redistribute it under the terms of the GNU GPL version 3 or
// (at your option) any later version. See LICENSE, or <https://www.gnu.org/licenses/>.
/* android-p0: the phone UI mode (docs/mobile rubric rows M2.1 M2.2 M3.1 M4.1 M4.3 M9.2 M10.1 M10.2).
   Runs ONLY on an Android WebView user agent; on the desktop this file returns at the first line, so the desktop
   DOM, CSS (every rule is scoped under body.is-mobile) and behaviour are unchanged.
   - #mhdr  one header row: Expand (left drawer) | the active view's title | the focused pane's own .modebtn and
            .morebtn (the real nodes, moved here, so their handlers, titles and menu placement stay main.js's)
   - #mnav  bottom pill: Navigate back, Navigate forward, Search, New note, Tabs (open-tab count), Menu (palette)
   - #side  becomes a slide-in overlay drawer over #mscrim; a mid-screen swipe right opens it, left closes it;
            touches starting within EDGE css px of either screen edge are left to the OS (its back gesture)
   - #side .m-pill  (android-p1a M4.2) one drawer view at a time: Files, Search, Tags, All properties, Bookmarks
   - #rside (M4.6) a mid-screen swipe left opens main.js's right sidebar as a right overlay drawer: Backlinks,
            Outgoing links, Outline behind a pill + a footer with the note's word and character counts
   - #msheet (M5.1) Menu opens the ribbon actions as a bottom sheet; Settings > Interface configures them
   - #vswitch (M1.1) the vault name opens a vault list (.m-vaultmenu) + "Manage vaults..." -> full-screen #picker
   - landscape (M3.3) the header and the nav float over the note (CSS only)
   - #mtabs (M6.1) full-screen tab switcher: cards (title, close x, active outlined) + a bar: +, count, Done
   - window.__mobileBack: called by MainActivity's OnBackPressedCallback; closes the innermost layer
            (menu > settings/modal/prompt > tab switcher > drawer > note history) and never exits the app
   - window.__mobileSafe(t, r, b, l): MainActivity pushes the system-bar + cutout insets (css px) -> --safe-* */
(() => {
  if (!/Android/.test(navigator.userAgent)) return;
  const EDGE = 24, $m = id => document.getElementById(id);
  document.body.classList.add("is-mobile", "is-phone");

  // insets: the native push sets them; a cached value per orientation covers the first frames of a cold start
  const okey = () => "os.safe." + (innerWidth > innerHeight ? "land" : "port");
  const setSafe = (t, r, b, l) => {
    const s = document.documentElement.style;
    [["top", t], ["right", r], ["bottom", b], ["left", l]].forEach(([k, v]) => s.setProperty("--safe-" + k, (+v || 0) + "px"));
  };
  window.__mobileSafe = (t, r, b, l) => {
    setSafe(t, r, b, l);
    try { localStorage.setItem(okey(), JSON.stringify([t, r, b, l])); } catch (e) { /* storage off: native push only */ }
  };
  try { const c = JSON.parse(localStorage.getItem(okey()) || "null"); if (Array.isArray(c)) setSafe(...c); } catch (e) { /* none cached */ }

  // ---- M7.2 / M7.3 / M8.1: the soft keyboard's height (css px) -> --m-kb. The IME does not resize this WebView
  // (innerHeight stays), only visualViewport shrinks to the keyboard's top edge, so bottom-docked UI pads by it.
  const vv = window.visualViewport;
  const kbPx = () => vv ? Math.max(0, Math.round(innerHeight - vv.height)) : 0;
  const setKb = () => {
    const k = kbPx();
    document.documentElement.style.setProperty("--m-kb", k + "px");
    document.body.classList.toggle("m-kb", k > 80);
    // the WebView also PANS the visual viewport (offsetTop > 0) to reveal a focused field, and that pan is not
    // undoable from JS: overlays that must sit on the keyboard follow the visual viewport itself (--m-vvt/--m-vvh)
    document.documentElement.style.setProperty("--m-vvt", (vv ? vv.offsetTop : 0) + "px");
    document.documentElement.style.setProperty("--m-vvh", (vv ? vv.height : innerHeight) + "px");
  };
  if (vv) { vv.addEventListener("resize", setKb); vv.addEventListener("scroll", setKb); }
  addEventListener("resize", setKb);
  setKb();

  const svg = d => '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">' + d + "</svg>";
  const btn = (cls, label, icon) => {
    const b = document.createElement("button");
    if (cls) b.className = cls;
    b.setAttribute("aria-label", label); b.title = label; b.innerHTML = icon;
    return b;
  };
  const app = document.querySelector(".app-container") || document.body;

  // ---- M2.1 / M2.2 header
  const hdr = document.createElement("div");
  hdr.id = "mhdr";
  const expand = btn("m-expand", "Expand", svg('<path d="M4 6h16M4 12h16M4 18h16"/>'));
  const title = document.createElement("div");
  title.className = "m-title";
  const tools = document.createElement("div");
  tools.className = "m-tools";
  hdr.append(expand, title, tools);
  app.prepend(hdr);

  // ---- M3.1 bottom navigation
  const nav = document.createElement("div");
  nav.id = "mnav";
  const nBack = btn("m-back", "Navigate back", svg('<path d="M15 5l-7 7 7 7"/>'));
  const nFwd = btn("m-fwd", "Navigate forward", svg('<path d="M9 5l7 7-7 7"/>'));
  const nSearch = btn("m-search", "Search", svg('<circle cx="11" cy="11" r="7"/><path d="M21 21l-4.3-4.3"/>'));
  const nNew = btn("m-new", "New note", svg('<path d="M12 5v14M5 12h14"/>'));
  const nTabs = btn("m-tabs", "Tabs", "");
  const nMenu = btn("m-menu", "Menu", svg('<path d="M4 6h16M4 12h16M4 18h16"/>'));
  nav.append(nBack, nFwd, nSearch, nNew, nTabs, nMenu);
  app.append(nav);

  const scrim = document.createElement("div");
  scrim.id = "mscrim";
  document.body.append(scrim);
  const tabsBox = document.createElement("div");
  tabsBox.id = "mtabs";
  tabsBox.hidden = true;
  document.body.append(tabsBox);

  const live = () => { try { return typeof state !== "undefined" && state && fg(); } catch (e) { return null; } };


  // ---- M4.2 drawer view switcher: ONE view at a time behind a pill at the drawer's foot (no #stabs strip on a phone).
  // Files / Search / Bookmarks are main.js's own panes (setPane); Tags (the right sidebar's #rpane-tags, moved here,
  // painted by rTags) and All properties (property_counts) are phone-only views that never enter sidePane, so the
  // persisted workspace.json pane stays one desktop knows.
  const VIEWS = [["files", "Files"], ["search", "Search"], ["tags", "Tags"], ["props", "All properties"], ["bm", "Bookmarks"]];
  const side = $m("side");
  const vTags = document.createElement("div");
  vTags.id = "m-vtags"; vTags.className = "spane m-view"; vTags.hidden = true;
  const rTagsPane = $m("rpane-tags");
  if (rTagsPane) vTags.append(rTagsPane);
  const vProps = document.createElement("div");
  vProps.id = "m-vprops"; vProps.className = "spane m-view"; vProps.hidden = true;
  vProps.innerHTML = '<div class="rhead">All properties<span class="scount"></span></div><div class="rlist m-proplist"></div>';
  // a view switcher pill + its list (left drawer here, right drawer M4.6): pick(k) switches, cur() names the view
  const pills = [];
  const makePill = (views, pick, cur) => {
    const bar = document.createElement("div");
    bar.className = "m-pillbar";
    const b = document.createElement("button");
    b.className = "m-pill"; b.setAttribute("aria-label", "Switch view");
    const txt = document.createElement("span");
    b.append(txt);
    b.insertAdjacentHTML("beforeend", svg('<path d="M6 15l6-6 6 6"/>'));
    const menu = document.createElement("div");
    menu.className = "m-pillmenu"; menu.hidden = true;
    const P = { bar, txt, isOpen: () => !menu.hidden, close: () => { menu.hidden = true; b.classList.remove("on"); },
      label: k => (views.find(v => v[0] === k) || views[0])[1], set: k => { txt.textContent = P.label(k); } };
    for (const [k, label] of views) {
      const it = document.createElement("div");
      it.className = "m-pillitem"; it.dataset.view = k; it.textContent = label;
      it.onclick = e => { e.stopPropagation(); P.close(); pick(k); };
      menu.append(it);
    }
    b.onclick = e => {
      e.stopPropagation();
      if (P.isOpen()) return P.close();
      for (const it of menu.children) it.classList.toggle("checked", it.dataset.view === cur());
      menu.hidden = false; b.classList.add("on");
    };
    bar.append(menu, b);
    pills.push(P);
    return P;
  };
  document.addEventListener("click", e => { for (const P of pills) if (P.isOpen() && !P.bar.contains(e.target)) P.close(); }, true);
  const lp = makePill(VIEWS, k => setView(k), () => mview);
  const pillBar = lp.bar;
  const vbar = $m("vbar");
  for (const n of [vTags, vProps]) side.insertBefore(n, vbar);
  side.insertBefore(pillBar, vbar);
  for (const id of ["pane-files", "pane-search", "pane-bm"]) { const p = $m(id); if (p) p.classList.add("m-view"); }
  let mview = typeof sidePane !== "undefined" ? sidePane : "files";
  const closePill = lp.close;
  const own = k => k === "tags" || k === "props";
  async function paintProps() {
    const counts = await (typeof inv === "function" ? inv("property_counts") : Promise.resolve({})).catch(() => ({}));
    const list = vProps.querySelector(".m-proplist"), names = Object.keys(counts);
    vProps.querySelector(".scount").textContent = names.length;
    list.textContent = "";
    if (!names.length) { const d = document.createElement("div"); d.className = "rempty"; d.textContent = "No properties."; list.append(d); return; }
    names.sort((a, b) => counts[b] - counts[a] || a.localeCompare(b));
    for (const n of names) {
      const r = document.createElement("div");
      r.className = "tagrow m-proprow"; r.dataset.prop = n;
      r.innerHTML = '<span class="tn"></span><span class="scount"></span>';
      r.firstChild.textContent = n; r.lastChild.textContent = counts[n];
      list.append(r);
    }
  }
  const paintOwn = k => {
    if (k === "tags" && typeof rTags === "function") { if (rTagsPane) rTagsPane.hidden = false; rTags(); }
    if (k === "props") paintProps();
  };
  function setView(k) {
    if (!VIEWS.some(v => v[0] === k)) k = "files";
    if (own(k)) {
      for (const id of ["pane-files", "pane-search", "pane-bm"]) { const p = $m(id); if (p) p.hidden = true; }
      mview = k;
      vTags.hidden = k !== "tags"; vProps.hidden = k !== "props";
      paintOwn(k);
    } else {
      mview = k;                        // before setPane: the wrapper below reads it
      if (typeof setPane === "function") setPane(k);
    }
    lp.set(mview);
  }
  // any main.js setPane (tagSearch, a command) is the drawer's view too: the phone views step aside
  if (typeof window.setPane === "function") {
    const origPane = window.setPane;
    window.setPane = function (p) {
      const r = origPane.apply(this, arguments);
      mview = p; vTags.hidden = true; vProps.hidden = true; lp.set(p);
      return r;
    };
  }
  lp.set(mview);

  // ---- drawer (M4.1 M4.3 M9.2)
  let paneBefore = null;               // the drawer view to restore when a Search-opened drawer closes
  const drawerOpen = () => document.body.classList.contains("m-drawer");
  const openDrawer = pane => {
    closeTabs(); closeRight();
    if (typeof closeMenu === "function") closeMenu();
    if (pane && mview !== pane) { paneBefore = mview; setView(pane); }
    else if (own(mview)) paintOwn(mview);   // tags / properties: repaint from the index on every open
    document.body.classList.add("m-drawer");
  };
  const closeDrawer = () => {
    if (!drawerOpen()) return;
    closePill(); closeVmenu();
    document.body.classList.remove("m-drawer");
    if (paneBefore) { setView(paneBefore); paneBefore = null; }
    const a = document.activeElement;
    if (a && $m("side").contains(a)) a.blur();
  };
  expand.onclick = () => drawerOpen() ? closeDrawer() : openDrawer();
  scrim.onclick = closeDrawer;

  // ---- M4.6 right drawer: a mid-screen swipe LEFT (no drawer open) slides #rside in from the right as an overlay.
  // It IS main.js's right sidebar (cmdToggleRight: rightOpen, the panes, their refresh on note change); the phone
  // hides its icon strip and shows ONE view behind a pill (Backlinks / Outgoing links / Outline) and a footer with the
  // active note's name and its word + character counts (the desktop status-bar rule: \S+ runs, editor text length).
  const RVIEWS = [["bl", "Backlinks"], ["out", "Outgoing links"], ["toc", "Outline"]];
  const rside = $m("rside");
  const rcur = () => (typeof rTab !== "undefined" && RVIEWS.some(v => v[0] === rTab)) ? rTab : "bl";
  const rp = makePill(RVIEWS, async k => { if (typeof setRTab === "function") await setRTab(k); rp.set(k); rFoot(); }, rcur);
  const rfoot = document.createElement("div");
  rfoot.className = "m-rfoot";
  rfoot.innerHTML = '<div class="m-rname"></div><div class="m-rcount"></div>';
  for (const k of ["bl", "out", "toc"]) { const p = $m("rpane-" + k); if (p) p.classList.add("m-view"); }
  rside.append(rp.bar, rfoot);
  function rFoot() {
    const g = live(), n = g && typeof curOf === "function" ? curOf(g) : null;
    const v = g && g.editor && n ? g.editor.value : "", w = (v.match(/\S+/g) || []).length;
    rfoot.firstChild.textContent = n ? String(n).split("/").pop().replace(/\.md$/, "") : "";
    rfoot.lastChild.textContent = n ? w + (w === 1 ? " word, " : " words, ") + v.length + (v.length === 1 ? " character" : " characters") : "";
  }
  const rOpenNow = () => document.body.classList.contains("m-rdrawer");
  const openRight = async () => {
    closeTabs(); closeDrawer();
    if (typeof closeMenu === "function") closeMenu();
    if (typeof rightOpen !== "undefined" && !rightOpen && typeof cmdToggleRight === "function") await cmdToggleRight();
    if (typeof setRTab === "function") await setRTab(rcur(), false);
    rp.set(rcur()); rFoot();
    document.body.classList.add("m-rdrawer");
  };
  const closeRight = () => {
    if (!rOpenNow()) return;
    rp.close();
    document.body.classList.remove("m-rdrawer");
    const a = document.activeElement; if (a && rside.contains(a)) a.blur();
    if (typeof rightOpen !== "undefined" && rightOpen && typeof cmdToggleRight === "function") cmdToggleRight();
  };
  scrim.addEventListener("click", closeRight);

  // ---- M1.1 the vault name in the drawer opens a vault list (known vaults, the current one ticked, then "Manage
  // vaults...") above the vault bar; "Manage vaults..." opens the vault manager (main.js's #picker) as a full-screen
  // page over everything. The manager's known-vaults list always carries the current vault.
  const vmenu = document.createElement("div");
  vmenu.className = "m-vaultmenu"; vmenu.hidden = true;
  side.insertBefore(vmenu, vbar);
  const vmenuOpen = () => !vmenu.hidden;
  function closeVmenu() { vmenu.hidden = true; }
  const vname = p => (typeof base === "function" ? base(p) : String(p).split(/[\\/]/).filter(Boolean).pop() || String(p));
  const known = async () => {
    let ps = [];
    try { ps = typeof inv === "function" ? await inv("recent_vaults") : []; } catch (e) { ps = []; }
    const cur = typeof vaultPath !== "undefined" ? vaultPath : null;
    if (cur && !ps.includes(cur)) ps = [cur, ...ps];
    return { ps, cur };
  };
  const openManager = () => {
    closeVmenu(); closeDrawer();
    if (typeof showPicker === "function") showPicker();
  };
  const vswitch = $m("vswitch");
  if (vswitch) vswitch.onclick = async e => {
    e.stopPropagation();
    if (vmenuOpen()) return closeVmenu();
    const { ps, cur } = await known();
    vmenu.textContent = "";
    for (const p of ps) {
      const it = document.createElement("div");
      it.className = "m-vaultitem" + (p === cur ? " checked" : ""); it.title = p;
      it.textContent = vname(p);
      it.onclick = ev => { ev.stopPropagation(); closeVmenu(); if (p !== cur && typeof openVault === "function") openVault(p, true); };
      vmenu.append(it);
    }
    const m = document.createElement("div");
    m.className = "m-vaultitem m-vaultmanage"; m.textContent = "Manage vaults...";
    m.onclick = ev => { ev.stopPropagation(); openManager(); };
    vmenu.append(m);
    vmenu.hidden = false;
  };
  document.addEventListener("click", e => { if (vmenuOpen() && !vmenu.contains(e.target) && e.target !== vswitch) closeVmenu(); }, true);
  if (typeof window.loadRecent === "function") {
    const origRecent = window.loadRecent;
    window.loadRecent = async function () {
      const r = await origRecent.apply(this, arguments);
      const ul = $m("p-recent"), cur = typeof vaultPath !== "undefined" ? vaultPath : null;
      if (ul && cur && ![...ul.children].some(li => li.querySelector("span") && li.querySelector("span").textContent === cur)) {
        const li = document.createElement("li");
        li.className = "m-curvault";
        li.innerHTML = "<b></b><span></span>";
        li.querySelector("b").textContent = vname(cur);
        li.querySelector("span").textContent = cur;
        li.onclick = () => { const pk = $m("picker"); if (pk) pk.hidden = true; };   // the open vault: just go back to it
        ul.prepend(li);
      }
      return r;
    };
  }
  // M1.2: in the vault manager a whole action row is the button. "Open folder as vault" = the SYSTEM folder picker
  // (MainActivity bridge, Storage Access Framework): the grant is persisted and the picked folder opens as the vault
  // (a vault switch = the app relaunches on it). Reading a folder outside the app needs "All files access": without
  // it the row says so and opens the system setting; picking again after the grant opens the folder.
  const droid = window.__osAndroid;
  window.__mobileTrees = () => { try { return droid ? JSON.parse(droid.trees()) : []; } catch (e) { return []; } };
  const prowOf = id => { const b = $m(id); return b && b.closest(".prow"); };
  const perr = t => { const e = $m("p-err"); if (e) e.textContent = t; };
  const openRow = prowOf("p-open"), createRow = prowOf("p-create");
  if (createRow) createRow.addEventListener("click", e => { if (e.target.id !== "p-create") $m("p-create").click(); });
  if (openRow) openRow.addEventListener("click", e => {
    if (!droid) { if (e.target.id !== "p-open") $m("p-open").click(); return; }
    e.stopPropagation(); e.preventDefault(); perr(""); droid.pickTree();
  }, true);
  window.__mobileTreePicked = (uri, path) => {
    if (!uri) return;                                    // picker backed out: the manager stays as it was
    if (!path) return perr("That folder has no file path on this phone; pick a folder on the device storage.");
    if (!droid.allFiles()) {
      perr("Allow \"All files access\" for Opensidian, then open the folder again.");
      return droid.requestAllFiles();
    }
    const cur = typeof vaultPath !== "undefined" ? vaultPath : null;
    if (path === cur) { const pk = $m("picker"); if (pk) pk.hidden = true; return; }
    if (typeof openVault === "function") openVault(path, true);
  };
  // a row tapped in the right drawer opens its note: the drawer gets out of the way
  rside.addEventListener("click", e => {
    if (e.target.closest && e.target.closest(".blline, .outrow, .tocrow")) { quietUntil = Date.now() + 3000; setTimeout(closeRight, 0); }
  });
  // M4.3: a file opened from the drawer opens in the current tab (rowOpen, main.js) and the drawer closes
  $m("side").addEventListener("click", e => {
    const r = e.target && e.target.closest && e.target.closest("#tree .trow");
    if (r && !r.classList.contains("folder")) setTimeout(closeDrawer, 0);
  });
  /* opening a note from the drawer must not raise the keyboard (main.js focuses the editor once the file loads):
     focus landing in #main is dropped until the user touches #main themselves, or 3 s pass */
  let quietUntil = 0;
  const quiet = () => { quietUntil = Date.now() + 3000; const a = document.activeElement; if (a && $m("main").contains(a)) a.blur(); };
  const main = $m("main");
  $m("side").addEventListener("click", e => {
    const r = e.target && e.target.closest && e.target.closest("#tree .trow");
    if (!r || r.classList.contains("folder")) return;
    quietUntil = Date.now() + 3000;
    const a = document.activeElement; if (a && main.contains(a)) a.blur();
  }, true);
  main.addEventListener("pointerdown", () => { quietUntil = 0; }, true);
  main.addEventListener("touchstart", () => { quietUntil = 0; }, { capture: true, passive: true });
  main.addEventListener("focusin", e => {
    if (Date.now() < quietUntil && e.target && e.target.blur) e.target.blur();
  });
  // swipes: mid-screen right opens, left closes; a touch that starts in the edge zone belongs to the OS
  let t0 = null;
  document.addEventListener("touchstart", e => {
    const t = e.touches[0];
    t0 = e.touches.length === 1 && t.clientX > EDGE && t.clientX < innerWidth - EDGE &&
         !(e.target.closest && e.target.closest("#modal, #settings, .ctxmenu, #mtabs, #msheet, #mnav, #mhdr, canvas"))
      ? { x: t.clientX, y: t.clientY, lx: t.clientX, ly: t.clientY } : null;
  }, { passive: true, capture: true });
  document.addEventListener("touchmove", e => {
    if (t0) { const t = e.touches[0]; t0.lx = t.clientX; t0.ly = t.clientY; }
  }, { passive: true, capture: true });
  const swipeEnd = () => {
    if (!t0) return;
    const dx = t0.lx - t0.x, dy = t0.ly - t0.y; t0 = null;
    if (Math.abs(dx) < 60 || Math.abs(dy) > Math.abs(dx) * 0.6) return;
    const modal = typeof modalKind !== "undefined" && modalKind;
    if (dx > 0) { if (rOpenNow()) closeRight(); else if (!drawerOpen() && !modal) openDrawer(); }
    else if (drawerOpen()) closeDrawer();
    else if (!rOpenNow() && !modal) openRight();
  };
  document.addEventListener("touchend", swipeEnd, { passive: true, capture: true });
  document.addEventListener("touchcancel", swipeEnd, { passive: true, capture: true });

  // ---- M6.1 full-screen tab switcher: one card per open tab (title + close x, the active one outlined) and a bottom
  // bar: + (a new tab), the tab count, Done. Cards keep [data-tab] = "<group id>:<index>".
  const allTabs = () => (typeof groups === "function" ? groups() : []).flatMap(g => g.tabs.map((t, i) => ({ g, i, t })));
  function closeTabs() { tabsBox.hidden = true; nTabs.classList.remove("on"); }
  const tabTitle = (g, i, t) => {
    const el = g.tabsEl && g.tabsEl.querySelectorAll(".tab")[i], s = el ? el.textContent.trim() : "";
    return s || (t.kind ? (({ gg: "Graph view", lg: "Local graph" })[t.kind] || t.kind) : String(t.name).split("/").pop().replace(/\.md$/, ""));
  };
  const paintTabs = () => {
    tabsBox.textContent = "";
    const cards = document.createElement("div");
    cards.className = "m-cards";
    const list = allTabs();
    for (const { g, i, t } of list) {
      const d = document.createElement("div");
      d.className = "m-card" + (g === live() && i === g.active ? " active" : "");
      d.dataset.tab = g.id + ":" + i;
      d.innerHTML = '<div class="m-card-head"><div class="m-card-title"></div><button class="m-card-x" aria-label="Close tab">' +
        svg('<path d="M6 6l12 12M18 6L6 18"/>') + '</button></div><div class="m-card-sub"></div>';
      d.querySelector(".m-card-title").textContent = tabTitle(g, i, t);
      const dir = !t.kind && t.name ? String(t.name).split("/").slice(0, -1).join("/") : "";
      d.querySelector(".m-card-sub").textContent = dir || (t.kind ? "" : "/");
      d.onclick = async () => {
        quiet();                               // the tab's load must not raise the keyboard
        closeTabs();
        if (typeof focusGroup === "function") focusGroup(g);
        if (typeof switchTab === "function") await switchTab(g, i);
        sync();
      };
      d.querySelector(".m-card-x").onclick = async e => {
        e.stopPropagation();
        if (typeof closeTab === "function") await closeTab(g, i);
        sync();
        if (!tabsBox.hidden) paintTabs();      // the switcher stays, one card fewer
      };
      cards.append(d);
    }
    const bar = document.createElement("div");
    bar.className = "m-tbar";
    const add = btn("m-tnew", "New tab", svg('<path d="M12 5v14M5 12h14"/>'));
    add.onclick = async () => { closeTabs(); quiet(); if (typeof cmdNewTab === "function") await cmdNewTab(); sync(); };
    const cnt = document.createElement("div");
    cnt.className = "m-tcount"; cnt.textContent = list.length + (list.length === 1 ? " tab" : " tabs");
    const done = btn("m-tdone", "Done", "Done");
    done.onclick = closeTabs;
    bar.append(add, cnt, done);
    tabsBox.append(cards, bar);
    const a = cards.querySelector(".m-card.active"); if (a) a.scrollIntoView({ block: "nearest" });
  };
  const openTabs = () => {
    closeDrawer(); closeRight(); closeSheet();
    paintTabs();
    tabsBox.hidden = false; nTabs.classList.add("on");
  };


  // ---- M5.1 Menu -> the ribbon as a bottom sheet (#msheet): the ribbon's actions, the ones ticked in Settings >
  // Interface > Ribbon menu configuration (phone-only, localStorage). Stock has 7; daily note + template have no core
  // plugin here and Bases is out of scope, so ours are the 4 our ribbon + palette carry.
  const RIBBON = [
    ["Open quick switcher", () => typeof cmdQuickSwitch === "function" && cmdQuickSwitch(),
      '<path d="M5 12h14M13 6l6 6-6 6"/>'],
    ["Open graph view", () => typeof cmdGlobalGraph === "function" && cmdGlobalGraph(),
      '<circle cx="6" cy="6" r="2.5"/><circle cx="18" cy="8" r="2.5"/><circle cx="10" cy="18" r="2.5"/><path d="M8 7l8 1M7.5 8.5l2 7M16.5 10l-5 6"/>'],
    ["Create new canvas", () => typeof cmdNewCanvas === "function" && cmdNewCanvas(typeof cvNewCanvasFolder === "function" ? cvNewCanvasFolder() : undefined),
      '<rect x="3" y="3" width="7" height="6" rx="1"/><rect x="14" y="13" width="7" height="8" rx="1"/><path d="M10 6h4a3 3 0 0 1 3 3v4"/>'],
    ["Open command palette", () => {
      if (typeof cmdPalette === "function") cmdPalette();   // M7.3: docked above the keyboard, which stays up
    }, '<path d="M4 17l6-5-6-5M12 19h8"/>'],
  ];
  const RKEY = "os.mobile.ribbonOff";
  const ribOff = () => { try { return new Set(JSON.parse(localStorage.getItem(RKEY) || "[]")); } catch (e) { return new Set(); } };
  const setRibOff = s => { try { localStorage.setItem(RKEY, JSON.stringify([...s])); } catch (e) { /* storage off: this session only */ } };
  const sheet = document.createElement("div");
  sheet.id = "msheet"; sheet.hidden = true;
  document.body.append(sheet);
  const sheetOpen = () => !sheet.hidden;
  function closeSheet() { sheet.hidden = true; document.body.classList.remove("m-sheet"); }
  const openSheet = () => {
    closeTabs(); closeDrawer(); closeRight();
    if (typeof closeMenu === "function") closeMenu();
    sheet.textContent = "";
    const off = ribOff();
    for (const [label, run, icon] of RIBBON) {
      if (off.has(label)) continue;
      const it = document.createElement("div");
      it.className = "m-sheetitem"; it.dataset.act = label;
      it.innerHTML = svg(icon) + "<span></span>";
      it.lastChild.textContent = label;
      it.onclick = async () => { closeSheet(); await run(); sync(); };
      sheet.append(it);
    }
    if (!sheet.children.length) {
      const d = document.createElement("div"); d.className = "m-sheetempty";
      d.textContent = "No ribbon actions. Settings > Interface > Ribbon menu configuration.";
      sheet.append(d);
    }
    sheet.hidden = false; document.body.classList.add("m-sheet");
  };
  scrim.addEventListener("click", closeSheet);
  // Settings > Interface: "Ribbon menu configuration", one checkbox row per action (appended after main.js's rows)
  if (typeof window.showSettingsPage === "function") {
    const origPage = window.showSettingsPage;
    window.showSettingsPage = function () {
      const r = origPage.apply(this, arguments);
      // re-entrant: a plugin settings tab (plugins/settings.js) calls showSettingsPage from inside the outer call,
      // so the outer return finds the inner call's box already there — one box per page
      const pg0 = $m("spage");
      if (typeof sPane !== "undefined" && sPane === "interface" && !(pg0 && pg0.querySelector(".m-ribcfg"))) {
        const pg = pg0, box = document.createElement("div");
        box.className = "m-ribcfg";
        box.innerHTML = '<div class="m-ribh">Ribbon menu configuration</div><div class="m-ribd">The actions the bottom-bar Menu opens. Untick one to leave it out.</div>';
        const off = ribOff();
        for (const [label] of RIBBON) {
          const row = document.createElement("label");
          row.className = "m-ribrow"; row.dataset.act = label;
          const cb = document.createElement("input");
          cb.type = "checkbox"; cb.checked = !off.has(label);
          cb.onchange = () => { const s = ribOff(); cb.checked ? s.delete(label) : s.add(label); setRibOff(s); };
          const t = document.createElement("span"); t.textContent = label;
          row.append(t, cb);
          box.append(row);
        }
        if (pg) pg.append(box);
      }
      return r;
    };
  }

  // ---- bottom nav actions
  nBack.onclick = () => { if (typeof histGo === "function") histGo(-1); };
  nFwd.onclick = () => { if (typeof histGo === "function") histGo(1); };
  nSearch.onclick = () => {
    openDrawer("search");
    const i = $m("sinput"); if (i) i.focus();
  };
  nNew.onclick = async () => {
    closeTabs(); closeDrawer();
    if (typeof cmdNewNote === "function") await cmdNewNote();
    // the keyboard comes up when the user taps into the note, not over the bottom bar on creation
    const a = document.activeElement; if (a && a !== document.body) a.blur();
    sync();
  };
  nTabs.onclick = () => tabsBox.hidden ? openTabs() : closeTabs();
  nMenu.onclick = () => sheetOpen() ? closeSheet() : openSheet();

  // a second tap on ⋮ closes its menu: main.js's document-capture mousedown would close it first and the click would
  // then reopen it, so the press on the open menu's own button stops at the window and the click toggles
  window.addEventListener("mousedown", e => {
    if (typeof menuEl !== "undefined" && menuEl && e.target.closest && e.target.closest("#mhdr .morebtn")) e.stopPropagation();
  }, true);

  // ---- M7.2: a quick switcher row REPLACES the active note on a phone (the explorer-tap rule, rowOpen: pinned / a
  // special tab still get a new tab), instead of desktop's push of a new tab; no new tab piles up behind the switcher.
  const origQsItems = window.qsItems;
  if (typeof origQsItems === "function" && typeof rowOpen === "function") window.qsItems = function () {
    return origQsItems.apply(this, arguments).map(it =>
      typeof notesCache !== "undefined" && notesCache.includes(it.label)
        ? Object.assign({}, it, { run: ev => rowOpen(it.label, ev) }) : it);
  };

  // ---- M7.1 header More options -> a bottom sheet: main.js builds the note menu, the phone drops its window / split /
  // OS-shell rows (no such surfaces on a phone), puts Close (the active tab) first and docks it to the screen bottom
  const DESK_ROW = /^(Split right|Split down|Move to new window|Open in new window|Show in system explorer|Open in default app|Copy screenshot)$/;
  const origHdrMenu = window.hdrMenu;
  if (typeof origHdrMenu === "function") window.hdrMenu = function (e, g) {
    const r = origHdrMenu.apply(this, arguments);
    const m = typeof menuEl !== "undefined" ? menuEl : null;
    if (!m || !m.dataset.hdr) return r;
    m.classList.add("m-bsheet");
    for (const d of [...m.children]) if (!d.classList.contains("sep") && DESK_ROW.test(d.textContent.trim())) d.remove();
    if (g && g.active >= 0 && g.tabs[g.active] && typeof closeTab === "function") {
      const sp = document.createElement("div"); sp.className = "sep"; m.prepend(sp);
      const c = document.createElement("div"); c.textContent = "Close";
      c.onmousedown = ev => ev.stopPropagation();
      c.onclick = () => { const i = g.active; closeMenu(); closeTab(g, i); };
      m.prepend(c);
    }
    let prev = true;                     // drop leading / doubled / trailing separators the removals left behind
    for (const d of [...m.children]) { const s = d.classList.contains("sep"); if (s && prev) d.remove(); else prev = s; }
    if (m.lastElementChild && m.lastElementChild.classList.contains("sep")) m.lastElementChild.remove();
    m.scrollTop = 0;
    if (typeof updateTitle === "function") updateTitle();      // census [menu:] spells the phone rows
    return r;
  };

  // ---- M8.1 the editing toolbar: a full-width, sideways-scrolling row pinned to the soft keyboard's top edge while
  // the note editor has focus (body.m-edkb). Its buttons run the registry's own commands or one CM6 transaction; a
  // press never takes focus from the editor, so the keyboard stays up.
  const tbar = document.createElement("div");
  tbar.id = "mtoolbar"; tbar.setAttribute("role", "toolbar"); tbar.hidden = true;
  const cmdRun = id => { const c = typeof CMDS !== "undefined" && CMDS.find(x => x.id === id); if (c && (!c.when || c.when())) c.run(); };
  const tg = () => (typeof edEditable === "function" && edEditable() ? fg() : null);
  const tbWrap = m => {                // a collapsed caret gets the empty pair with the caret inside (stock), else toggle
    const g = tg(); if (!g) return;
    const s = Ed.sel(g);
    if (s && s.empty) { Ed.replace(g, s, m + m, "wrap"); Ed.place(g, s.a.l, s.a.c + m.length); }
    else edWrap(m);
  };
  const tbLine = (re, next) => { if (tg()) edLine(l => { const m = re.exec(l), p = m ? m[0] : ""; return next(p) + l.slice(p.length); }); };
  const TB = [
    ["Undo", '<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11"/>', () => { const g = tg(); if (g) Ed.undo(g); }],
    ["Redo", '<path d="m15 14 5-5-5-5"/><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13"/>', () => { const g = tg(); if (g) Ed.redo(g); }],
    ["Bold", '<path d="M6 12h9a4 4 0 0 1 0 8H6V4h8a4 4 0 0 1 0 8"/>', () => tbWrap("**")],
    ["Italic", '<path d="M19 4h-9M14 20H5M15 4 9 20"/>', () => tbWrap("*")],
    ["Strikethrough", '<path d="M16 4H9a3 3 0 0 0-2.83 4M14 12a4 4 0 0 1 0 8H6M4 12h16"/>', () => tbWrap("~~")],
    ["Highlight", '<path d="m9 11-6 6v3h9l3-3"/><path d="m22 12-4.6 4.6a2 2 0 0 1-2.8 0l-5.2-5.2a2 2 0 0 1 0-2.8L14 4"/>', () => tbWrap("==")],
    ["Code", '<path d="m16 18 6-6-6-6M8 6l-6 6 6 6"/>', () => tbWrap("`")],
    ["Heading", '<path d="M6 12h12M6 20V4M18 20V4"/>', () => tbLine(/^#{1,6} /, p => p.length >= 7 ? "" : "#".repeat(p ? p.length : 1) + " ")],
    ["Insert internal link", '<path d="M10 13a5 5 0 0 0 7.5.5l3-3a5 5 0 0 0-7-7l-1.7 1.7"/><path d="M14 11a5 5 0 0 0-7.5-.5l-3 3a5 5 0 0 0 7 7l1.7-1.7"/>', () => cmdRun("editor:insert-wikilink")],
    ["Insert Markdown link", '<path d="M15 3h6v6M10 14 21 3M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6"/>', () => cmdRun("editor:insert-link")],
    ["Add tag", '<path d="M4 9h16M4 15h16M10 3 8 21M16 3l-2 18"/>', () => cmdRun("editor:insert-tag")],
    ["Checklist", '<rect x="3" y="5" width="6" height="6" rx="1"/><path d="m3 17 2 2 4-4M13 6h8M13 12h8M13 18h8"/>', () => cmdRun("editor:toggle-checklist-status")],
    ["Bullet list", '<path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/>', () => cmdRun("editor:toggle-bullet-list")],
    ["Numbered list", '<path d="M10 6h11M10 12h11M10 18h11M4 6h1v4M4 10h2M6 18H4c0-1 2-2 2-3s-1-1.5-2-1"/>', () => cmdRun("editor:toggle-numbered-list")],
    ["Quote", '<path d="M3 21c3 0 7-1 7-8V5H3v7h4M14 21c3 0 7-1 7-8V5h-7v7h4"/>', () => tbLine(/^> /, p => p ? "" : "> ")],
    ["Indent", '<path d="m3 8 4 4-4 4M21 12H11M21 6H11M21 18H11"/>', () => { const g = tg(), s = g && Ed.sel(g); if (s) Ed.indent(g, s, false); }],
    ["Unindent", '<path d="m7 8-4 4 4 4M21 12H11M21 6H11M21 18H11"/>', () => { const g = tg(), s = g && Ed.sel(g); if (s) Ed.indent(g, s, true); }],
    ["Insert current date", '<rect x="3" y="4" width="18" height="18" rx="2"/><path d="M16 2v4M8 2v4M3 10h18"/>', () => cmdRun("templates:insert-current-date")],
    ["Open command palette", '<path d="m4 17 6-6-6-6M12 19h8"/>', () => cmdRun("command-palette:open")],
    ["Hide keyboard", '<rect x="2" y="4" width="20" height="12" rx="2"/><path d="M6 8h.01M10 8h.01M14 8h.01M18 8h.01M7 12h10M9 20l3 2 3-2"/>', () => { const a = document.activeElement; if (a) a.blur(); }],
    ["Settings", '<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-2.9 1.2V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-2.9-1.2l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1A1.7 1.7 0 0 0 3 14H3a2 2 0 1 1 0-4h.1A1.7 1.7 0 0 0 4.6 8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1A1.7 1.7 0 0 0 10 3.1V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 2.9 1.2l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1A1.7 1.7 0 0 0 21 10h.1a2 2 0 1 1 0 4H21a1.7 1.7 0 0 0-1.6 1z"/>',
      () => { if (typeof sPane !== "undefined") sPane = "interface"; if (typeof settingsOpen !== "undefined" && !settingsOpen) cmdSettings(); }],
  ];
  for (const [label, d, run] of TB) {
    const b = btn("m-tbtn", label, svg(d));
    for (const ev of ["pointerdown", "mousedown"]) b.addEventListener(ev, e => e.preventDefault());   // the editor keeps focus
    b.addEventListener("click", e => { e.preventDefault(); run(); later(); });
    tbar.append(b);
  }
  document.body.append(tbar);
  const edFocused = () => { const a = document.activeElement; return !!(a && a.closest && a.closest("#main .cm-content")); };
  const tbSync = () => {
    const on = document.body.classList.contains("m-kb") && edFocused();
    document.body.classList.toggle("m-edkb", on);
    if (tbar.hidden === on) tbar.hidden = !on;
  };
  document.addEventListener("focusin", tbSync); document.addEventListener("focusout", () => setTimeout(tbSync, 0));
  if (vv) vv.addEventListener("resize", tbSync);
  addEventListener("resize", tbSync);

  // ---- M10.1 / M10.2: the OS back (key or edge gesture) unwinds the innermost layer; at the root it is a no-op
  window.__mobileBack = () => {
    if (typeof menuEl !== "undefined" && menuEl) { closeMenu(); return "menu"; }
    if (typeof settingsOpen !== "undefined" && settingsOpen) { closeSettings(); return "settings"; }
    if (typeof modalKind !== "undefined" && modalKind) { closeModal(); return "modal"; }
    const box = ["rnbox", "ulbox", "delbox", "bmebox", "picker"].map($m).find(b => b && !b.hidden);
    if (box) {
      const t = box.contains(document.activeElement) ? document.activeElement : box;
      t.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", code: "Escape", bubbles: true, cancelable: true }));
      return "prompt";
    }
    if (vmenuOpen()) { closeVmenu(); return "vaults"; }
    const op = pills.find(P => P.isOpen());
    if (op) { op.close(); return "pill"; }
    if (sheetOpen()) { closeSheet(); return "sheet"; }
    if (!tabsBox.hidden) { closeTabs(); return "tabs"; }
    if (rOpenNow()) { closeRight(); return "rdrawer"; }
    if (drawerOpen()) { closeDrawer(); return "drawer"; }
    const g = live();
    if (g && typeof navState === "function" && navState(g).b) {
      // the note back-navigation loads must not raise the keyboard either (same rule as a drawer tap)
      quietUntil = Date.now() + 3000;
      const a = document.activeElement; if (a && main.contains(a)) a.blur();
      histGo(-1); return "history";
    }
    return "root";
  };

  // ---- keep the header + nav in step with the focused pane
  let parked = null;                   // the pane whose .modebtn/.morebtn currently sit in #mhdr
  function sync() {
    const g = live();
    if (!g) return;
    if (parked !== g || g.modebtn.parentNode !== tools || g.morebtn.parentNode !== tools) {
      if (parked && parked !== g && parked.pane) {
        const bar = parked.pane.querySelector(":scope > .tabbar");
        if (bar) bar.append(parked.modebtn, parked.morebtn);   // back to their desktop slot: after .tabs, ⋮ last
      }
      tools.textContent = "";
      tools.append(g.modebtn, g.morebtn);
      parked = g;
    }
    const a = g.tabsEl && g.tabsEl.querySelector(".tab.active");
    const tt = a ? a.textContent.trim() : "";
    if (title.textContent !== tt) title.textContent = tt;
    const s = typeof navState === "function" ? navState(g) : { b: false, f: false };
    nBack.disabled = !s.b; nFwd.disabled = !s.f;
    const n = String(document.querySelectorAll("#main .tabs .tab").length);
    if (nTabs.textContent !== n) nTabs.textContent = n;
  }
  let due = 0;
  const later = () => { if (!due) due = requestAnimationFrame(() => { due = 0; sync(); }); };
  for (const f of ["updateTitle", "updateTitleNow", "updateModeBtn", "renderTabs"]) {
    const orig = window[f];
    if (typeof orig !== "function") continue;
    window[f] = function () { const r = orig.apply(this, arguments); later(); return r; };
  }
  for (const ev of ["click", "touchend", "keyup"]) document.addEventListener(ev, () => setTimeout(later, 30), true);
  new MutationObserver(later).observe($m("main"), { childList: true });   // a pane added / removed
  later();
})();
