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
    closePill();
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
    add.onclick = async () => { closeTabs(); if (typeof cmdNewTab === "function") await cmdNewTab(); sync(); };
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
      if (typeof cmdPalette === "function") cmdPalette();
      const i = $m("minput"); if (i && document.activeElement === i) i.blur();   // no keyboard over the list
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
      if (typeof sPane !== "undefined" && sPane === "interface") {
        const pg = $m("spage"), box = document.createElement("div");
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
