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
   - #vswitch (M1.1/M1.3) the vault name opens a centred single-choice vault dialog (.m-vdlg) + "Manage vaults..." -> full-screen #picker
   - landscape (M3.3) the header and the nav float over the note (CSS only)
   - #mtabs (M6.1) full-screen tab switcher: cards (title, close x, active outlined) + a bar: +, count, Done
   - window.__mobileBack: called by MainActivity's OnBackPressedCallback; closes the innermost layer
            (menu > settings/modal/prompt > tab switcher > drawer > note history) and never exits the app
   - window.__mobileSafe(t, r, b, l): MainActivity pushes the system-bar + cutout insets (css px) -> --safe-* */
(() => {
  if (!/Android/.test(navigator.userAgent)) return;
  // M14.1: the Quick Action and the two config sections are built further down (toolbar block); used earlier
  let qaGet = () => "command-palette:open", mobCfgBox = null, tbSubOpen = () => false, tbSubClose = () => {};
  let rowRn = null;   // android-p2a M10.4: the live inline row rename { cancel }
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
  // M15.6 once the WebView itself is panned (vv.offsetTop > 0) the visual viewport grows back towards innerHeight while
  // the keyboard is still up (landscape walk past the keyboard: vvh 424 + vvt 24 = innerHeight). The keyboard height
  // seen unpanned is remembered per orientation, and a pan with a field focused keeps the keyboard state on.
  const kbSeen = {};
  const kbKey = () => (innerWidth > innerHeight ? "land" : "port");
  // M15.5: an inline drawer rename (input.m-trn) with the keyboard up stays inside the visual viewport: its scroller
  // gets bottom room for the keyboard and scrolls the row to the top of what is still visible (stock: the field and
  // ~3 rows below it above the keyboard). The room goes when the rename ends.
  function keepRn(a) {
    if (!vv || !a || !a.isConnected) return;
    let s = a.parentElement;
    while (s && s !== document.body) { const o = getComputedStyle(s).overflowY; if (o === "auto" || o === "scroll") break; s = s.parentElement; }
    if (!s || s === document.body) return;
    const top = vv.offsetTop, bot = vv.offsetTop + vv.height, row = a.closest(".trow") || a, r = row.getBoundingClientRect();
    if (r.top >= top + 2 && r.bottom <= bot - 2) return;
    s.style.paddingBottom = Math.max(0, Math.round(innerHeight - bot)) + "px"; s.dataset.mRnPad = "1";
    const g = s.getBoundingClientRect();
    s.scrollTop += row.getBoundingClientRect().top - (Math.max(g.top, top) + 4);
  }
  const unRnPad = () => { for (const s of document.querySelectorAll("[data-m-rn-pad]")) { s.style.paddingBottom = ""; delete s.dataset.mRnPad; } };
  const setKb = () => {
    const k = kbPx(), a = document.activeElement;
    if (vv && vv.offsetTop < 1 && k > 80) kbSeen[kbKey()] = k;
    const typing = !!(a && a !== document.body && (a.isContentEditable || /^(INPUT|TEXTAREA)$/.test(a.tagName)));
    const up = k > 80 || !!(vv && vv.offsetTop >= 1 && typing && kbSeen[kbKey()]);
    document.documentElement.style.setProperty("--m-kb", k + "px");
    document.body.classList.toggle("m-kb", up);
    // the WebView also PANS the visual viewport (offsetTop > 0) to reveal a focused field, and that pan is not
    // undoable from JS: overlays that must sit on the keyboard follow the visual viewport itself (--m-vvt/--m-vvh)
    document.documentElement.style.setProperty("--m-vvt", (vv ? vv.offsetTop : 0) + "px");
    document.documentElement.style.setProperty("--m-vvh", (vv ? vv.height : innerHeight) + "px");
    if (up && a && a.matches && a.matches("input.m-trn")) requestAnimationFrame(() => keepRn(a));
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
  // ---- M13.1 tablet (smallest width >= 600 css px, stock's sw600dp): body.is-tablet instead of is-phone — no bottom
  // nav, Navigate back / forward in the view header's left, the pane's desktop tab strip (titles, the active tab's
  // close x, a "New tab" +) under it; drawers stay overlays. Re-evaluated on every resize (a fold, wm size).
  const hBack = btn("m-hback", "Navigate back", svg('<path d="M15 5l-7 7 7 7"/>'));
  const hFwd = btn("m-hfwd", "Navigate forward", svg('<path d="M9 5l7 7-7 7"/>'));
  hdr.insertBefore(hFwd, title); hdr.insertBefore(hBack, hFwd);
  const tabletSync = () => {
    const t = Math.min(screen.width, screen.height) >= 600 && innerWidth >= 600;
    document.body.classList.toggle("is-tablet", t); document.body.classList.toggle("is-phone", !t);
  };
  tabletSync(); addEventListener("resize", tabletSync);

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
  const lp = makePill(VIEWS, k => { setView(k); wsPick(k); }, () => mview);
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

  // ---- android-p2a M12.2 / M12.3: the phone's layout file is .obsidian/workspace-mobile.json (src-tauri WS_REL on
  // android) in stock mobile's shape. main.js builds its desktop-shaped doc (wsDoc) and hands it to out(), which
  // returns: main = ONE tabs group in a vertical split (no dimension); left / right = "mobile-drawer" holding EVERY
  // drawer view as a leaf + currentTab (the view showing) and no width / collapsed (drawer open/closed is not
  // persisted); right views bound to the active note; left-ribbon.hiddenItems; active = the focused leaf, which is a
  // LEFT DRAWER VIEW right after the user picks one in the pill (stock) and goes back to the note's leaf as soon as
  // the main area changes. sidesIn() is the inverse on restore (view, search query, leaf ids, a drawer-view active).
  const LV = [["files", "file-explorer", "Files", "lucide-folder-closed"], ["search", "search", "Search", "lucide-search"],
    ["tags", "tag", "Tags", "lucide-tags"], ["props", "all-properties", "All properties", "lucide-archive"],
    ["bm", "bookmarks", "Bookmarks", "lucide-bookmark"]];
  const RV = [["bl", "backlink", "Backlinks", "links-coming-in"], ["out", "outgoing-link", "Outgoing links", "links-going-out"],
    ["toc", "outline", "Outline", "lucide-list"]];
  const wsm = { ids: {}, act: null, key: null };
  const wmid = k => wsm.ids[k] || (wsm.ids[k] = typeof wsId === "function" ? wsId() : Math.random().toString(16).slice(2, 18));
  const wsKey = () => {            // what the main area shows: focused leaf id + newest note (a change drops a drawer active)
    const g = typeof fg === "function" && typeof state !== "undefined" && state ? fg() : null;
    const t = g && g.active >= 0 ? g.tabs[g.active] : null;
    const id = t ? (typeof wsIdOf === "function" ? wsIdOf(t, "lid") : t.lid || "") : "";
    return id + "|" + ((typeof mruList !== "undefined" && mruList[0]) || "");
  };
  function wsPick(k) {             // the user picked a left drawer view: it is the active leaf (stock), persisted
    const v = LV.find(x => x[0] === k);
    if (!v) return;
    wsm.act = wmid("l:" + v[1]); wsm.key = wsKey();
    if (typeof wsTouch === "function") wsTouch();
  }
  window.__wsMobile = {
    out(d, af) {
      const tabs = [];
      (function walk(n) {
        if (!n || typeof n !== "object") return;
        if (n.type === "tabs") tabs.push(n); else if (Array.isArray(n.children)) n.children.forEach(walk);
      })(d.main);
      const kids = [].concat(...tabs.map(t => t.children || []));
      const g = { id: tabs.length ? tabs[0].id : wmid("tabs"), type: "tabs", children: kids };
      const ai = kids.findIndex(l => l && l.id === d.active);
      if (ai > 0) g.currentTab = ai;                       // R28.5: 0 by its absence
      const sq = $m("sinput") ? $m("sinput").value : "";
      const left = { id: wmid("left"), type: "mobile-drawer",
        children: LV.map(([, ty, ti, ic]) => ({ id: wmid("l:" + ty), type: "leaf",
          state: { type: ty, state: ty === "search" ? { query: sq } : {}, icon: ic, title: ti } })),
        currentTab: Math.max(0, LV.findIndex(v => v[0] === mview)) };
      const rk = rcur();
      const right = { id: wmid("right"), type: "mobile-drawer",
        children: RV.map(([, ty, ti, ic]) => ({ id: wmid("r:" + ty), type: "leaf",
          state: { type: ty, state: af ? { file: af } : {}, icon: ic, title: ti } })),
        currentTab: Math.max(0, RV.findIndex(v => v[0] === rk)) };
      let active = d.active;
      if (wsm.act && wsm.key === wsKey()) active = wsm.act; else wsm.act = null;
      const o = { main: { id: d.main && d.main.id || wmid("main"), type: "split", direction: "vertical", children: [g] },
        left, right,
        "left-ribbon": d["left-ribbon"] && typeof d["left-ribbon"] === "object" ? d["left-ribbon"] : { hiddenItems: {} },
        active, lastOpenFiles: d.lastOpenFiles };
      if (!o["left-ribbon"].hiddenItems || typeof o["left-ribbon"].hiddenItems !== "object") o["left-ribbon"].hiddenItems = {};
      for (const k of Object.keys(d)) if (!(k in o)) o[k] = d[k];   // W4: unknown top-level keys ride through
      return o;
    },
    sidesIn(doc) {
      const L = doc.left, R = doc.right;
      if (L && L.type === "mobile-drawer" && Array.isArray(L.children)) {
        if (typeof L.id === "string") wsm.ids.left = L.id;
        for (const l of L.children) if (l && l.state && typeof l.id === "string") wsm.ids["l:" + l.state.type] = l.id;
        const cl = L.children[Number.isInteger(L.currentTab) ? L.currentTab : 0];
        const v = cl && cl.state && LV.find(x => x[1] === cl.state.type);
        if (v && v[0] !== mview) setView(v[0]);
        const q = v && v[0] === "search" && cl.state.state && cl.state.state.query;
        if (typeof q === "string" && q && $m("sinput")) { $m("sinput").value = q; if (typeof runSearch === "function") runSearch(); }
        if (typeof doc.active === "string" && L.children.some(l => l && l.id === doc.active)) { wsm.act = doc.active; wsm.key = wsKey(); }
      }
      if (R && R.type === "mobile-drawer" && Array.isArray(R.children)) {
        if (typeof R.id === "string") wsm.ids.right = R.id;
        for (const l of R.children) if (l && l.state && typeof l.id === "string") wsm.ids["r:" + l.state.type] = l.id;
        const cr = R.children[Number.isInteger(R.currentTab) ? R.currentTab : 0];
        const v = cr && cr.state && RV.find(x => x[1] === cr.state.type);
        if (v && typeof setRTab === "function") Promise.resolve(setRTab(v[0], false)).catch(() => {});
      }
    },
  };

  // ---- M1.1 the vault name in the drawer opens a vault list (known vaults, the current one ticked, then "Manage
  // vaults...") above the vault bar; "Manage vaults..." opens the vault manager (main.js's #picker) as a full-screen
  // page over everything. The manager's known-vaults list always carries the current vault.
  // M1.3 (stock: a native single-choice dialog): the list is a centred dialog over a scrim, one radio row per vault,
  // the current one checked, then "Manage vaults..."; back or a tap outside dismisses it and the drawer stays open.
  const vdlg = document.createElement("div");
  vdlg.className = "m-vdlg"; vdlg.hidden = true;
  const vmenu = document.createElement("div");
  vmenu.className = "m-vaultmenu"; vmenu.setAttribute("role", "dialog"); vmenu.setAttribute("aria-modal", "true");
  vmenu.setAttribute("aria-label", "Vaults");
  vdlg.append(vmenu); document.body.append(vdlg);
  const vmenuOpen = () => !vdlg.hidden;
  function closeVmenu() { vdlg.hidden = true; }
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
      it.setAttribute("role", "radio"); it.setAttribute("aria-checked", p === cur ? "true" : "false");
      it.textContent = vname(p);
      it.onclick = ev => { ev.stopPropagation(); closeVmenu(); if (p !== cur && typeof openVault === "function") openVault(p, true); };
      vmenu.append(it);
    }
    const m = document.createElement("div");
    m.className = "m-vaultitem m-vaultmanage"; m.textContent = "Manage vaults...";
    m.onclick = ev => { ev.stopPropagation(); openManager(); };
    vmenu.append(m);
    vdlg.hidden = false;
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
  // M9.1: a touch that starts in a note editor already scrolled to its top may be a pull-down (Quick Action)
  // the note body or its pane blank around it (landscape pads the top of .lp under the floating header)
  const atTop = el => {
    const c = el && el.closest && el.closest("#main .pane .content"); if (!c) return false;
    const s = [...c.querySelectorAll(".cm-scroller")].find(x => x.getClientRects().length);
    const lp = el.closest(".lp");
    return !!s && s.scrollTop <= 0 && (!lp || lp.scrollTop <= 0);
  };
  document.addEventListener("touchstart", e => {
    const t = e.touches[0];
    t0 = e.touches.length === 1 && t.clientX > EDGE && t.clientX < innerWidth - EDGE &&
         !(e.target.closest && e.target.closest("#modal, #settings, .ctxmenu, #mtabs, #msheet, .m-asheet, #mascrim, #mnav, #mhdr, canvas"))
      ? { x: t.clientX, y: t.clientY, lx: t.clientX, ly: t.clientY, top: atTop(e.target) } : null;
  }, { passive: true, capture: true });
  document.addEventListener("touchmove", e => {
    if (!t0) return;
    const t = e.touches[0]; t0.lx = t.clientX; t0.ly = t.clientY;
    // M9.1: a downward pull at the top has nothing to scroll; cancel it so Chromium does not make it a scroll
    // gesture — only then is its touchend a user activation, which the palette field needs to raise the keyboard
    const dy = t0.ly - t0.y;
    if (t0.top && dy > 0 && Math.abs(t0.lx - t0.x) <= dy && e.cancelable) e.preventDefault();
  }, { passive: false, capture: true });
  const swipeEnd = () => {
    if (!t0) return;
    const dx = t0.lx - t0.x, dy = t0.ly - t0.y; const top = t0.top; t0 = null;
    const modal = typeof modalKind !== "undefined" && modalKind;
    // M9.1 pull-down in a note at its top -> the Quick Action (M14.1: Settings > Interface > Mobile Quick Action,
    // default the command palette) with the keyboard up when it opens a field.
    if (top && dy > 120 && Math.abs(dx) < dy / 2) {
      if (!modal && !drawerOpen() && !rOpenNow() && typeof cmdPalette === "function") {
        const qa = qaGet();
        if (qa === "command-palette:open") cmdPalette(); else cmdRun(qa);
        // a focus from a drag's touchend: Android raises the IME only for a tap, so the app asks for it (native
        // MainActivity showIme). Without the bridge the field stays focused and a tap raises the keyboard.
        const f = document.getElementById("minput"), d = window.__osAndroid;
        if (f && d && d.showIme && document.activeElement === f) d.showIme();
      }
      return;
    }
    if (Math.abs(dx) < 60 || Math.abs(dy) > Math.abs(dx) * 0.6) return;
    if (dx > 0) { if (rOpenNow()) closeRight(); else if (!drawerOpen() && !modal) openDrawer(); }
    else if (drawerOpen()) closeDrawer();
    else if (!rOpenNow() && !modal) openRight();
  };
  document.addEventListener("touchend", swipeEnd, { passive: true, capture: true });
  document.addEventListener("touchcancel", swipeEnd, { passive: true, capture: true });

  // ---- M9.4 a two-finger pinch over a note is the quick font size: spreading grows the text, pinching in shrinks it
  // (the vault's appearance.json baseFontSize through qfsSet, the one setter Ctrl+wheel and the slider use; clamped
  // 10..30, only while Quick font size adjustment is on). The page itself never zooms: the gesture is cancelled.
  let pz = null;
  const pdist = ts => Math.hypot(ts[0].clientX - ts[1].clientX, ts[0].clientY - ts[1].clientY);
  document.addEventListener("touchstart", e => {
    if (e.touches.length !== 2) { pz = null; return; }
    const on = el => el && el.closest && el.closest("#main .lp, #main .preview");
    const ok = typeof qfsSet === "function" && qfsAct && !settingsOpen &&
      on(e.touches[0].target) && on(e.touches[1].target);
    const d = pdist(e.touches);
    pz = ok && d > 10 ? { d, s: qfsSize } : null;
  }, { passive: true, capture: true });
  document.addEventListener("touchmove", e => {
    if (!pz || e.touches.length !== 2) return;
    if (e.cancelable) e.preventDefault();
    if (qfsSet(pz.s * pdist(e.touches) / pz.d)) updateTitle();
  }, { passive: false, capture: true });
  const pinchEnd = e => { if (e.touches.length < 2) pz = null; };
  document.addEventListener("touchend", pinchEnd, { passive: true, capture: true });
  document.addEventListener("touchcancel", pinchEnd, { passive: true, capture: true });

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
      d.addEventListener("contextmenu", e => {   // M6.4: a long-press on a card = the tab card sheet (not activated)
        e.preventDefault(); e.stopPropagation();
        noSel(); setTimeout(noSel, 60);
        cardSheet(g, i, t);
      });
      cards.append(d);
    }
    const bar = document.createElement("div");
    bar.className = "m-tbar";
    const add = btn("m-tnew", "New tab", svg('<path d="M12 5v14M5 12h14"/>'));
    add.onclick = async () => { closeTabs(); quiet(); if (typeof cmdNewEmptyTab === "function") await cmdNewEmptyTab(); sync(); };   // M12.3: + opens an EMPTY tab (stock mobile)
    const cnt = document.createElement("button");   // M6.2: the count opens the tab-count sheet
    cnt.className = "m-tcount"; cnt.setAttribute("aria-label", "Tab options");
    cnt.append(list.length + (list.length === 1 ? " tab" : " tabs"));
    cnt.insertAdjacentHTML("beforeend", svg('<path d="M7 10l5 5 5-5"/>'));
    cnt.onclick = e => { e.stopPropagation(); tcountSheet(list.length); };
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
  // ---- M11.2 Settings > Interface, mobile-only rows above the ribbon configuration: Full screen (the header and nav
  // hide while a note scrolls down, come back on a scroll up), Floating navigation (the nav is a pill over the note in
  // portrait too; landscape always floats, M3.3), Sliding sidebars (an open left drawer pushes the note instead of
  // covering it). Stored in localStorage os.m.iface; body classes m-fs / m-float / m-slide. Defaults off, except
  // Full screen: the vault's appearance.json autoFullScreen, absent = on (M12.5).
  const IKEY = "os.m.iface";
  const IFACE = [
    ["fullscreen", "Full screen", "Hide the header and the navigation bar while scrolling down a note.", "m-fs"],
    ["floatnav", "Floating navigation", "Show the navigation bar as a floating pill over the note.", "m-float"],
    ["slide", "Sliding sidebars", "An open sidebar pushes the note aside instead of covering it.", "m-slide"],
  ];
  // android-p2a M12.5: Full screen is the VAULT's .obsidian/appearance.json autoFullScreen (get_autofs / set_autofs;
  // a missing key = ON, stock's default). localStorage only caches it for the first paint before the vault answers.
  let fsV = null;
  const ifRaw = () => { try { return JSON.parse(localStorage.getItem(IKEY) || "{}") || {}; } catch (e) { return {}; } };
  const ifGet = () => {
    const s = ifRaw();
    s.fullscreen = fsV !== null ? fsV : (typeof s.fullscreen === "boolean" ? s.fullscreen : true);
    return s;
  };
  const fsSync = v => {
    fsV = !!v;
    const s = ifRaw(); s.fullscreen = fsV;
    try { localStorage.setItem(IKEY, JSON.stringify(s)); } catch (e) { /* cache only */ }
    const cb = document.querySelector("#spage .m-ifrow[data-k=fullscreen] input");
    if (cb) cb.checked = fsV;
    ifApply();
  };
  const fsLoad = async (n = 0) => {      // the vault opens after this script runs: retry until it answers
    if (typeof inv !== "function") return;
    try { fsSync(await inv("get_autofs")); } catch (e) { if (n < 80) setTimeout(() => fsLoad(n + 1), 250); }
  };
  const ifApply = () => {
    const s = ifGet();
    for (const [k, , , cls] of IFACE) document.body.classList.toggle(cls, !!s[k]);
    if (!s.fullscreen) document.body.classList.remove("m-fshide");
  };
  const ifSet = (k, v) => {
    if (k === "fullscreen") {
      fsV = !!v;
      if (typeof inv === "function") inv("set_autofs", { on: fsV }).catch(() => { /* refused: this session only */ });
    }
    const s = ifRaw(); s[k] = !!v;
    try { localStorage.setItem(IKEY, JSON.stringify(s)); } catch (e) { /* storage off: this session only */ }
    ifApply();
  };
  ifApply();
  fsLoad();
  // Full screen: a note scroller moving down past the first screenful hides the bars, any move up shows them
  const fsLast = new WeakMap();
  document.addEventListener("scroll", e => {
    const el = e.target;
    if (!(el instanceof Element) || !el.matches("#main .cm-scroller, #main .preview, #main .lp")) return;
    const y = el.scrollTop, p = fsLast.has(el) ? fsLast.get(el) : y; fsLast.set(el, y);
    if (!document.body.classList.contains("m-fs")) return;
    if (y < 24 || y - p < -6) document.body.classList.remove("m-fshide");
    else if (y - p > 6) document.body.classList.add("m-fshide");
  }, true);
  const ifRows = () => {
    const box = document.createElement("div");
    box.className = "m-ifcfg";
    const s = ifGet();
    for (const [k, name, desc] of IFACE) {
      const row = document.createElement("label");
      row.className = "m-ifrow"; row.dataset.k = k;
      const t = document.createElement("div"); t.className = "m-iftxt";
      const h = document.createElement("div"); h.className = "m-ifh"; h.textContent = name;
      const d = document.createElement("div"); d.className = "m-ifd"; d.textContent = desc;
      t.append(h, d);
      const cb = document.createElement("input");
      cb.type = "checkbox"; cb.checked = !!s[k];
      cb.onchange = () => ifSet(k, cb.checked);
      row.append(t, cb);
      box.append(row);
    }
    return box;
  };
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
        if (pg) { pg.prepend(ifRows()); pg.append(box); if (mobCfgBox) pg.append(mobCfgBox()); }   // the phone rows first: reachable without a scroll
      }
      return r;
    };
  }

  // ---- bottom nav actions
  nBack.onclick = () => { if (typeof histGo === "function") histGo(-1); };
  nFwd.onclick = () => { if (typeof histGo === "function") histGo(1); };
  hBack.onclick = nBack.onclick; hFwd.onclick = nFwd.onclick;
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
  // M15.1 the Add bookmark card as a bottom sheet on the visual viewport (CSS); the Title field focused with the IME
  // raised by the native bridge (a focus outside a direct tap on the field does not raise it on Android)
  const bmSheet = () => {
    openBmAdd(null);
    const f = $m("bme-title"), d = window.__osAndroid;
    if (!f || $m("bmebox").hidden) return;
    f.focus();
    if (d && d.showIme) d.showIme();
    const keep = () => { if (!$m("bmebox").hidden && document.activeElement === f) f.scrollIntoView({ block: "nearest" }); };
    setTimeout(keep, 350); setTimeout(keep, 800);
  };
  // M15.3 the folder suggester on the #modal machinery (docked to the visual viewport, M7.2): every folder but the
  // note's own, "/" = the vault root; a row runs moveNoteTo. Generic modal kind "mv" -> Enter = the selected row.
  const mvSheet = async nm => {
    let fl = [];
    try { fl = await inv("list_folders"); } catch (e) { fl = []; }
    const here = nm.includes("/") ? nm.slice(0, nm.lastIndexOf("/")) : "";
    const items = [];
    if (here) items.push({ label: "/", hint: "", run: () => moveNoteTo(nm, "") });
    for (const f of fl) if (f && f !== here) items.push({ label: f, hint: "", run: () => moveNoteTo(nm, f) });
    openModal("mv", () => items);
    const f = $m("minput"), d = window.__osAndroid;
    f.placeholder = "Type a folder";
    if (typeof qsFoot === "function") qsFoot(false);
    if (d && d.showIme && document.activeElement === f) d.showIme();
  };
  // M15.2 the rename prompt is a sheet on the keyboard's top edge: a "Note title" heading (stock mobile, M7.4), the one-line field, Cancel / Save.
  // Cancel / Save replay Escape / Enter on the field, so the note and the canvas rename paths stay main.js's; a press
  // keeps the field focused (the keyboard stays up until the sheet closes).
  const rnbox = $m("rnbox"), rnin = $m("rninput");
  if (rnbox && rnin && !rnbox.querySelector(".m-rnhead")) {
    const hd = document.createElement("div"); hd.className = "m-rnhead"; hd.textContent = "Note title";
    const bt = document.createElement("div"); bt.className = "m-rnbtns";
    const mk = (label, key, cls) => {
      const b = document.createElement("button"); b.type = "button"; b.textContent = label; b.className = cls;
      b.addEventListener("mousedown", e => e.preventDefault());
      b.addEventListener("click", e => { e.preventDefault(); e.stopPropagation();
        rnin.dispatchEvent(new KeyboardEvent("keydown", { key, code: key, bubbles: true, cancelable: true })); });
      return b;
    };
    bt.append(mk("Cancel", "Escape", "m-rnno"), mk("Save", "Enter", "m-rnyes"));
    rnbox.prepend(hd); rnbox.append(bt);
    if (typeof window.cmdRename === "function") {
      const origRename = window.cmdRename;
      window.cmdRename = function () {
        const r = origRename.apply(this, arguments);
        const d = window.__osAndroid;
        if (!rnbox.hidden && document.activeElement === rnin && d && d.showIme) d.showIme();
        return r;
      };
    }
  }
  // android-p2a M10.4: Rename from a Files-row long-press edits the name IN the row (stock) instead of the sheet.
  // Enter renames through renameThenAsk (the title road: move, then the link prompt); Escape, blur and system back
  // cancel — the field goes, the row's name is back, nothing moves on disk.
  const origNoteMenu = window.noteMenu;
  if (typeof origNoteMenu === "function") window.noteMenu = function (e, nm) {
    const r = origNoteMenu.apply(this, arguments);
    const row = e && e.target && e.target.closest ? e.target.closest("#tree .trow.note") : null;
    const m = typeof menuEl !== "undefined" ? menuEl : null;
    if (row && m) m.addEventListener("click", ev => {
      const it = [...m.querySelectorAll("*")].find(d => d.onclick && /^Rename/.test(d.textContent.trim()));
      if (!it || !it.contains(ev.target)) return;
      ev.preventDefault(); ev.stopImmediatePropagation();
      closeMenu(); rowRename(row, nm);
    }, true);
    return r;
  };
  function rowRename(row, nm) {
    const tn = row.querySelector(".tn"); if (!tn || rowRn) return;
    const old = nm.split("/").pop(), dir = nm.slice(0, nm.length - old.length);
    const inp = document.createElement("input");
    inp.type = "text"; inp.className = "m-trn"; inp.value = old; inp.spellcheck = false;
    inp.setAttribute("autocapitalize", "off"); inp.setAttribute("enterkeyhint", "done");
    tn.hidden = true; tn.after(inp);
    let done = false;
    const end = async ok => {
      if (done) return; done = true; rowRn = null;
      const nn = inp.value.trim();
      inp.remove(); tn.hidden = false; unRnPad();
      if (!ok || !nn || nn === old || /[\\/]/.test(nn)) return;
      const g = fg(), t = g && g.active >= 0 ? g.tabs[g.active] : null;
      if (t && !t.kind && t.name === nm && typeof flushSave === "function") await flushSave(g);
      await renameThenAsk(nm, dir + nn);
    };
    inp.addEventListener("keydown", ev => {
      ev.stopPropagation();
      if (ev.key === "Enter") { ev.preventDefault(); end(true); }
      else if (ev.key === "Escape") { ev.preventDefault(); end(false); }
    });
    inp.addEventListener("blur", () => end(false));
    for (const k of ["click", "mousedown", "contextmenu", "touchstart"]) inp.addEventListener(k, ev => ev.stopPropagation());
    rowRn = { cancel: () => end(false) };
    inp.focus(); inp.select();
    const d = window.__osAndroid; if (d && d.showIme) d.showIme();
  }
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
    // M15.1: "Bookmark..." opens the Add bookmark sheet (Title focused, keyboard up) instead of a silent toggle
    for (const d of [...m.children]) if (d.textContent.trim() === "Bookmark..." && !d.classList.contains("dis") &&
        typeof openBmAdd === "function") d.onclick = () => { closeMenu(); bmSheet(); };
    // M15.3: "Move file to..." opens a folder suggester (Type a folder) docked above the keyboard instead of the
    // in-place folder list inside the menu; Enter takes the top match, a row tap takes that folder
    const mvTab = g && g.tabs && g.tabs[g.active];
    for (const d of [...m.children]) if (d.textContent.trim() === "Move file to..." && mvTab && !mvTab.kind &&
        typeof moveNoteTo === "function") d.onclick = () => { closeMenu(); mvSheet(mvTab.name); };
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
  const TB_CFG = "Configure mobile toolbar";
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
    // M8.2: the last button is stock's wrench, "Configure mobile toolbar": the keyboard goes down and Settings opens
    // straight on Interface at its Mobile section (back = the Settings tab list, back again = the note). Not offered
    // in Manage toolbar options, so the way back to the toolbar settings cannot be switched off.
    [TB_CFG, '<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>',
      () => { const a = document.activeElement; if (a) a.blur(); const d = window.__osAndroid; if (d && d.hideIme) d.hideIme(); openIfaceAt(".m-mobcfg"); }],
  ];
  const tbBtn = new Map();
  for (const [label, d, run] of TB) {
    const b = btn("m-tbtn", label, svg(d));
    for (const ev of ["pointerdown", "mousedown"]) b.addEventListener(ev, e => e.preventDefault());   // the editor keeps focus
    b.addEventListener("click", e => { e.preventDefault(); run(); later(); });
    tbar.append(b); tbBtn.set(label, b);
  }
  document.body.append(tbar);
  // ---- android-p2a M14.1: the mobile-only palette commands (stock mobile's names) + their two Settings sections
  //  - Configure mobile toolbar / Configure mobile Quick Action -> Settings > Interface, scrolled to its "Mobile"
  //    section (M11.3 below: the Quick Action row, localStorage os.m.qa, default the command palette; the toolbar
  //    options page, one checkbox per toolbar button, localStorage os.m.tboff)
  //  - Show tab overview -> the tab switcher; Share -> the active note's text to the Android share sheet (bridge)
  //  - Toggle keyboard -> the editor's keyboard as it was before the palette opened: up -> hidden (bridge hideIme),
  //    down -> the editor focused with the keyboard shown (bridge showIme)
  // M14.2: the phone has one window, so "Close window" is not offered.
  const TBOFF = "os.m.tboff", QAK = "os.m.qa";
  const tbOff = () => { try { return new Set(JSON.parse(localStorage.getItem(TBOFF) || "[]")); } catch (e) { return new Set(); } };
  const tbApply = () => { const off = tbOff(); for (const [l, b] of tbBtn) b.style.display = off.has(l) ? "none" : ""; };
  tbApply();
  // M11.3 (stock Settings > Interface > Mobile): a "Mobile" heading, then "Configure mobile Quick Action" (its
  // description names the current command; "Configure" opens a command chooser — the palette's list, picking a row
  // stores it instead of running it) and "Configure mobile toolbar" ("Manage toolbar options" swaps the page for the
  // toolbar list, one checkbox per button; back returns to Interface). The Quick Action may be any command.
  const QA0 = "command-palette:open";
  const cmdOf = id => (typeof CMDS !== "undefined" && CMDS.find(c => c.id === id)) || null;
  qaGet = () => { const k = localStorage.getItem(QAK); return k && cmdOf(k) ? k : QA0; };
  const qaName = () => { const c = cmdOf(qaGet()); return c ? c.name : "Open command palette"; };
  const qaDesc = () => "Choose the command a pull-down at the top of a note runs. The current command is set to “" + qaName() + "”.";
  const mRow = (cls, name, desc, btn, run) => {
    const row = document.createElement("div"); row.className = "m-mrow " + cls;
    const t = document.createElement("div"); t.className = "m-iftxt";
    const h = document.createElement("div"); h.className = "m-mh"; h.textContent = name;
    const d = document.createElement("div"); d.className = "m-ifd"; d.textContent = desc;
    t.append(h, d);
    const b = document.createElement("button"); b.type = "button"; b.className = "m-mbtn"; b.textContent = btn;
    b.onclick = e => { e.preventDefault(); e.stopPropagation(); run(row); };
    row.append(t, b); return row;
  };
  const qaChoose = row => {
    if (typeof openModal !== "function") return;
    const items = () => CMDS.filter(c => !c.id.startsWith("mobile:configure-")).map(c => ({ label: c.name, hint: "", run: () => {
      try { localStorage.setItem(QAK, c.id); } catch (e) { /* this session only */ }
      const d = row.querySelector(".m-ifd"); if (d) d.textContent = qaDesc();
    } }));
    document.body.classList.add("m-qachoose");
    openModal("qa", items);
    $m("minput").placeholder = "Select a command...";
  };
  if ($m("modal")) new MutationObserver(() => { if ($m("modal").hidden) document.body.classList.remove("m-qachoose"); })
    .observe($m("modal"), { attributes: true, attributeFilter: ["hidden"] });
  const tbList = () => {
    const box = document.createElement("div");
    box.className = "m-ribcfg m-tbcfg";
    const off = tbOff();
    for (const [label] of TB) {
      if (label === TB_CFG) continue;
      const row = document.createElement("label");
      row.className = "m-ribrow"; row.dataset.tb = label;
      const cb = document.createElement("input");
      cb.type = "checkbox"; cb.checked = !off.has(label);
      cb.onchange = () => {
        const s = tbOff(); cb.checked ? s.delete(label) : s.add(label);
        try { localStorage.setItem(TBOFF, JSON.stringify([...s])); } catch (e) { /* this session only */ }
        tbApply();
      };
      const t = document.createElement("span"); t.textContent = label;
      row.append(t, cb); box.append(row);
    }
    return box;
  };
  tbSubOpen = () => !!document.querySelector("#spage > .m-tbsub");
  tbSubClose = () => {
    const pg = $m("spage"), s = pg && pg.querySelector(":scope > .m-tbsub"); if (!s) return;
    s.remove(); for (const c of pg.children) if (c.dataset.mTbHid) { c.hidden = false; delete c.dataset.mTbHid; }
    const m = pg.querySelector(".m-mobcfg"); if (m) m.scrollIntoView({ block: "start" });
  };
  const tbSub = () => {
    const pg = $m("spage"); if (!pg || tbSubOpen()) return;
    for (const c of pg.children) if (!c.hidden) { c.hidden = true; c.dataset.mTbHid = "1"; }
    const s = document.createElement("div"); s.className = "m-tbsub";
    s.innerHTML = '<div class="m-ribh">Manage toolbar options</div><div class="m-ribd">The buttons above the keyboard while you edit a note. Untick one to leave it out.</div>';
    s.append(tbList()); pg.append(s); pg.scrollTop = 0;
  };
  mobCfgBox = () => {
    const box = document.createElement("div");
    box.className = "m-ribcfg m-mobcfg";
    box.innerHTML = '<div class="m-ribh">Mobile</div>';
    box.append(mRow("m-qacfg", "Configure mobile Quick Action", qaDesc(), "Configure", qaChoose),
      mRow("m-tbrow", "Configure mobile toolbar", "Choose the buttons on the toolbar above the keyboard.", "Manage toolbar options", tbSub));
    return box;
  };
  // openSettings is async and ends on the tab list, so the page is shown only once it has finished opening
  // (shown during the open, the list replaced it again and Settings sat on the list)
  const openIfaceAt = async sel => {
    if (typeof sPane !== "undefined") sPane = "interface";
    if (typeof settingsOpen !== "undefined" && !settingsOpen) {
      if (typeof window.openSettings === "function") await window.openSettings(); else if (typeof cmdSettings === "function") cmdSettings();
    }
    if (typeof showSettingsPage === "function") window.showSettingsPage("interface");
    setTimeout(() => { const e = document.querySelector("#spage " + sel); if (e) e.scrollIntoView({ block: "start" }); }, 120);
  };
  let edKbUp = false;                  // the editor's keyboard, as last seen while the editor had focus
  const edKbSeen = () => { if (edFocused()) edKbUp = document.body.classList.contains("m-kb"); };
  // focus coming back from the palette (or any modal) brings the MODAL's keyboard along: it is not the editor's, so
  // Toggle keyboard (run as the palette closes) must still see the state from before the palette
  // (landscape: the hand-back also resizes/pans the visual viewport, so the viewport listener honours the same window)
  let modalOut = 0;
  const kbSettled = () => Date.now() - modalOut > 800;
  document.addEventListener("focusout", e => { if (e.target && e.target.closest && e.target.closest("#modal")) modalOut = Date.now(); });
  document.addEventListener("focusin", () => { if (kbSettled()) edKbSeen(); });
  if (vv) vv.addEventListener("resize", () => setTimeout(() => { if (kbSettled()) edKbSeen(); }, 50));
  const bridge = () => window.__osAndroid || null;
  const toggleKb = () => setTimeout(() => {   // after the palette has closed and handed focus back
    const d = bridge(), g = typeof fg === "function" ? fg() : null;
    if (edKbUp) {
      edKbUp = false;
      if (d && d.hideIme) d.hideIme(); else { const a = document.activeElement; if (a) a.blur(); }
    } else {
      edKbUp = true;
      if (g && typeof Ed !== "undefined") Ed.focus(g);
      if (d && d.showIme) d.showIme();
    }
  }, 300);
  const noteActive = () => { const g = typeof fg === "function" && typeof state !== "undefined" && state ? fg() : null;
    const t = g && g.active >= 0 ? g.tabs[g.active] : null; return !!(t && !t.kind); };
  const share = () => {
    const d = bridge(), g = fg(), n = curOf(g);
    if (!d || !d.share || !n) return;
    d.share(String(n).split("/").pop(), g.editor ? g.editor.value : "");
  };
  if (typeof CMDS !== "undefined") {
    const add = (id, name, run, when) => CMDS.push({ id, name, def: [], run, when });
    add("mobile:configure-toolbar", "Configure mobile toolbar", () => openIfaceAt(".m-mobcfg"));
    add("mobile:configure-quick-action", "Configure mobile Quick Action", () => openIfaceAt(".m-mobcfg"));
    add("mobile:toggle-keyboard", "Toggle keyboard", toggleKb, noteActive);
    // the overview is a full-screen layer: once the palette has handed focus back to the editor, the keyboard goes
    // down (else the first OS back only hides the keyboard and the overview stays)
    add("workspace:show-tab-overview", "Show tab overview", () => { openTabs(); setTimeout(() => {
      if (tabsBox.hidden) return;
      const a = document.activeElement; if (a && a !== document.body) a.blur();
      const d = bridge(); if (d && d.hideIme) d.hideIme();
    }, 300); });
    add("mobile:share", "Share", share, () => noteActive() && !!(bridge() && bridge().share));
    const wc = CMDS.find(c => c.id === "window:close");
    if (wc) wc.when = () => false;
    CMDS.sort((a, b) => a.name.localeCompare(b.name));
  }
  const edFocused = () => { const a = document.activeElement; return !!(a && a.closest && a.closest("#main .cm-content")); };
  // M8.3 stock hides the mobile toolbar while a property is edited. Ours has no Properties widget: a note's properties
  // are its leading YAML frontmatter (--- ... ---), edited as text, so the toolbar stays down while the caret is in it.
  const caretInFm = () => {
    const c = typeof Ed !== "undefined" && typeof fg === "function" && fg() ? Ed.cm(fg()) : null;
    if (!c || !c.state) return false;
    const d = c.state.doc; if (d.lines < 2 || d.line(1).text.trim() !== "---") return false;
    const h = c.state.selection.main.head, n = Math.min(d.lines, 400);
    for (let i = 2; i <= n; i++) if (d.line(i).text.trim() === "---") return h <= d.line(i).to;
    return false;
  };
  const tbSync = () => {
    const on = document.body.classList.contains("m-kb") && edFocused() && !caretInFm();
    document.body.classList.toggle("m-edkb", on);
    if (tbar.hidden === on) tbar.hidden = !on;
  };
  document.addEventListener("focusin", tbSync); document.addEventListener("focusout", () => setTimeout(tbSync, 0));
  document.addEventListener("selectionchange", () => { if (edFocused()) tbSync(); });
  if (vv) { vv.addEventListener("resize", tbSync); vv.addEventListener("scroll", tbSync); }
  addEventListener("resize", tbSync);
  // M15.6 the caret line stays above the toolbar: the IME does not shrink the layout, so the note's scroller runs on
  // under the keyboard and neither CM6 nor the WebView keeps the caret above the toolbar (the WebView pans just enough
  // to show the caret at the keyboard edge, which also pushes the header off-screen). While the toolbar is up, after a
  // caret move / keystroke / viewport change, scroll the caret's scroller so the caret line ends above the toolbar top
  // (visual viewport bottom - toolbar - 8px) and starts below the header (or the visual viewport top).
  // CM6's own scrollIntoView (run in the same frame as a caret move, before the WebView decides to pan) keeps the caret
  // clear of the keyboard + toolbar through a scrollMargins extension added once per editor view
  const cmV = (window.__osModules || {})["@codemirror/view"], cmS = (window.__osModules || {})["@codemirror/state"];
  const cmDone = new WeakSet();
  const cmMargins = c => {
    if (!c || cmDone.has(c) || !cmV || !cmS || !cmV.EditorView.scrollMargins) return;
    cmDone.add(c);
    c.dispatch({ effects: cmS.StateEffect.appendConfig.of(cmV.EditorView.scrollMargins.of(() => {
      const kh = kbSeen[kbKey()];
      return document.body.classList.contains("m-edkb") && kh ? { bottom: kh + (tbar.offsetHeight || 44) + 8 } : {};
    })) });
  };
  const caretFollow = () => {
    if (!document.body.classList.contains("m-edkb") || typeof Ed === "undefined") return;
    const g = fg(), c = g && Ed.cm(g); if (!c) return;
    const k = c.coordsAtPos(c.state.selection.main.head); if (!k) return;
    let sc = c.contentDOM.parentElement;
    while (sc && sc !== document.body && !(sc.scrollHeight > sc.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(sc).overflowY))) sc = sc.parentElement;
    if (!sc || sc === document.body) return;
    cmMargins(c);
    // the target is the UNPANNED band (header bottom .. keyboard top seen unpanned - toolbar): a caret kept there gives
    // the WebView no reason to pan, and a pan once started only grows (vv reports past the WebView while panned)
    const kh = kbSeen[kbKey()], vb = kh ? innerHeight - kh : (vv ? vv.offsetTop + vv.height : innerHeight);
    const h = document.querySelector("#mhdr"), hb = h && h.getClientRects().length ? h.getBoundingClientRect().bottom : 0;
    // once panned, the top `pan` css px of the layout sit under the status bar; with Full screen hiding #mhdr (M12.5)
    // nothing else keeps the caret below that edge (on screen the WebView shows vv's bottom at the keyboard top)
    const pan = kh && vv ? Math.max(0, vv.offsetTop + vv.height - vb) : 0;
    const top = Math.max(hb, pan) + 4, bot = vb - (tbar.offsetHeight || 44) - 8;
    if (k.bottom > bot) sc.scrollTop += Math.ceil(k.bottom - bot);
    else if (k.top < top && bot - top > k.bottom - k.top) sc.scrollTop -= Math.ceil(top - k.top);
  };
  let cfT = 0;
  const caretSoon = () => { if (cfT) return; cfT = requestAnimationFrame(() => requestAnimationFrame(() => { cfT = 0; caretFollow(); })); };
  document.addEventListener("selectionchange", caretSoon);
  document.addEventListener("input", caretSoon, true);
  document.addEventListener("keyup", caretSoon, true);
  if (vv) { vv.addEventListener("resize", caretSoon); vv.addEventListener("scroll", caretSoon); }
  document.addEventListener("focusin", () => { setTimeout(caretSoon, 300); setTimeout(caretSoon, 700); });

  // M15.12 a rotation keeps the caret: the IME restarts its input on the configuration change and commits the word
  // around the caret, which lands the caret at that word's end — a selection-only change that lands a few ms BEFORE
  // the orientation event. The editor selections are kept with their time; at the flip the newest one older than
  // 250 ms (on the same, textually unchanged document) is put back and held for 1.5 s while the text stays unchanged.
  let rsHist = [], rsGuard = null, rsOri = kbKey();
  const rsCm = () => (typeof Ed !== "undefined" && typeof fg === "function" && fg() ? Ed.cm(fg()) : null);
  const rsFix = () => {
    const c = rsCm(); if (!rsGuard || !c) return;
    if (performance.now() > rsGuard.until || c !== rsGuard.c || !c.state.doc.eq(rsGuard.doc)) { rsGuard = null; return; }
    const s = c.state.selection.main;
    if (s.anchor !== rsGuard.a || s.head !== rsGuard.h) c.dispatch({ selection: { anchor: rsGuard.a, head: rsGuard.h } });
  };
  const rsNote = () => {
    if (rsGuard) { rsFix(); return; }
    const c = rsCm(); if (!c) return;
    const s = c.state.selection.main, l = rsHist[rsHist.length - 1];
    if (l && l.c === c && l.doc === c.state.doc && l.a === s.anchor && l.h === s.head) return;
    rsHist.push({ c, doc: c.state.doc, a: s.anchor, h: s.head, t: performance.now() });
    if (rsHist.length > 8) rsHist.shift();
  };
  document.addEventListener("selectionchange", rsNote);
  document.addEventListener("keyup", rsNote, true);
  const rsFlip = () => {
    const o = kbKey(); if (o === rsOri) return; rsOri = o;
    const c = rsCm(), now = performance.now(); if (!c) return;
    let p = null;
    for (let i = rsHist.length - 1; i >= 0; i--) if (rsHist[i].t < now - 250) { p = rsHist[i]; break; }
    if (!p || p.c !== c || !p.doc.eq(c.state.doc)) return;
    rsGuard = Object.assign({}, p, { until: now + 1500 });
    for (const t of [0, 100, 250, 500, 900, 1400]) setTimeout(rsFix, t);
  };
  addEventListener("resize", rsFlip);
  if (screen.orientation) screen.orientation.addEventListener("change", rsFlip);


  // ---- M10.3 Settings on a phone: one column. Opening shows the tab list alone (body.m-snav); a tab shows its page
  // alone (body.m-spage, #shead names it); back on a page returns to the list, back on the list closes Settings.
  const sMode = page => {
    document.body.classList.toggle("m-spage", !!page); document.body.classList.toggle("m-snav", !page);
    const h = document.querySelector("#shead > span"), s = document.querySelector("#snav .snavi.sel");
    if (h) h.textContent = page && s ? s.textContent.trim() : "Settings";
  };
  let sOpening = false;
  if (typeof window.openSettings === "function") {
    const origOpen = window.openSettings;
    window.openSettings = async function () {
      sOpening = true;
      try { return await origOpen.apply(this, arguments); } finally { sOpening = false; if (settingsOpen) sMode(false); }
    };
  }
  if (typeof window.showSettingsPage === "function") {
    const origShow = window.showSettingsPage;
    window.showSettingsPage = function () {
      const r = origShow.apply(this, arguments);
      if (!sOpening && typeof settingsOpen !== "undefined" && settingsOpen) {
        sMode(true);
        // a tab opens at its top: #spage is one element reused for every tab, and while it was hidden behind the
        // list it kept the previous tab's scroll offset (an Interface page opened scrolled past its own rows)
        const pg = $m("spage"); if (pg) pg.scrollTop = 0;
      }
      return r;
    };
  }
  const sPageOpen = () => document.body.classList.contains("m-spage");
  // ---- M11.1 the phone Settings header carries a back arrow at its left (the desktop X is hidden): on a page it
  // returns to the tab list, on the list it closes Settings — the same steps as the OS back.
  const shead = $m("shead");
  if (shead && !shead.querySelector(".m-sback")) {
    const sb = document.createElement("button");
    sb.className = "m-sback"; sb.type = "button"; sb.setAttribute("aria-label", "Back"); sb.title = "Back";
    sb.innerHTML = '<svg viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 5l-7 7 7 7"/></svg>';
    sb.addEventListener("click", e => { e.preventDefault(); e.stopPropagation(); sPageOpen() ? sMode(false) : closeSettings(); });
    shead.prepend(sb);
  }
  // ---- M11.4 the phone hides the desktop-only settings (stock mobile has none of them): Appearance loses Custom app
  // icon, Interface loses Zoom level / Native menus / Window frame style / Open settings in new window, General loses
  // the updater rows (Automatic updates, Receive early access versions) and the Version row's Check for updates button
  // (the row stays, text only). A section heading left without a row goes with its card. DOM only, after main.js
  // built the page, so settings.rs and the desktop census are untouched.
  const SHIDE = {
    appearance: ["Zoom level", "Window frame style", "Custom app icon"],
    interface: ["Zoom level", "Native menus", "Window frame style", "Open settings in new window"],
    general: ["Automatic updates", "Receive early access versions"],
  };
  const sHide = () => {
    const pg = $m("spage"), hide = typeof sPane !== "undefined" && SHIDE[sPane];
    if (!pg || !hide) return;
    for (const lb of pg.querySelectorAll(".srow .slabel")) if (hide.includes(lb.textContent.trim())) lb.closest(".srow").remove();
    for (const c of pg.querySelectorAll('.sctl[data-act="check_updates"]')) c.remove();
    for (const card of pg.querySelectorAll(".scard")) {
      if (card.querySelector(".srow")) continue;
      const h = card.previousElementSibling;
      if (h && h.classList.contains("ssec")) h.remove();
      card.remove();
    }
  };
  if (typeof window.showSettingsPage === "function") {
    const origHide = window.showSettingsPage;
    window.showSettingsPage = function () { const r = origHide.apply(this, arguments); sHide(); return r; };
  }
  // ---- M14.3 the command palette lists at most 100 commands while the query is empty (stock mobile's cap); every
  // other command is one filter away. The quick switcher keeps main.js's own cap.
  if (typeof window.mdFilter === "function") {
    const origMdf = window.mdFilter;
    window.mdFilter = function () {
      const r = origMdf.apply(this, arguments);
      if (modalKind === "cp" && !$m("minput").value.trim() && mdItems.length > 100) { mdItems = mdItems.slice(0, 100); renderModal(); }
      return r;
    };
  }
  // ---- M10.1 / M10.2: the OS back (key or edge gesture) unwinds the innermost layer; at the root it is a no-op
  window.__mobileBack = () => {
    // android-p2a M10.4: back during an inline rename CANCELS it (the drawer row editor, the inline title): no move
    if (rowRn) { rowRn.cancel(); return "rename"; }
    if (typeof titling !== "undefined" && titling) { closeTitleEdit(); return "title"; }
    if (typeof menuEl !== "undefined" && menuEl) { closeMenu(); return "menu"; }
    if (asOpen()) { closeAS(); return "asheet"; }
    // M11.3 the Quick Action chooser sits over Settings; the toolbar options page goes back to Interface
    if (typeof modalKind !== "undefined" && modalKind === "qa") { closeModal(); return "modal"; }
    if (typeof settingsOpen !== "undefined" && settingsOpen && tbSubOpen()) { tbSubClose(); return "settings-sub"; }
    if (typeof settingsOpen !== "undefined" && settingsOpen && sPageOpen()) { sMode(false); return "settings-page"; }
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

  // ---- M9.3 double-tap in a note = a caret at the tap (no word select); long-press = the WebView's own selection.
  // CM6 leaves touch selection to the platform, so the word select comes from the WebView: the second tap's mousedown
  // is swallowed and, once the gesture has settled, the selection is collapsed to the tapped position.
  const inEd = t => t instanceof Element && !!t.closest("#main .cm-editor");
  const dblCaret = e => {
    const g = live(), c = g && Ed.cm(g);
    if (!c) return;
    const p = c.posAtCoords({ x: e.clientX, y: e.clientY });
    if (p == null) return;
    c.dispatch({ selection: { anchor: p } }); c.focus();
  };
  document.addEventListener("mousedown", e => {
    if (e.detail < 2 || !inEd(e.target)) return;
    e.preventDefault();
    const x = e.clientX, y = e.clientY;
    for (const ms of [0, 120, 350]) setTimeout(() => dblCaret({ clientX: x, clientY: y }), ms);
  }, true);
  document.addEventListener("dblclick", e => { if (inEd(e.target)) { e.preventDefault(); dblCaret(e); } }, true);
  // main.js's document contextmenu listener cancels the event (desktop: no native menu, the plugin editor-menu). In a
  // phone note a long-press keeps the platform selection (handles + copy/paste) and opens no app menu.
  // android-p2b M7.7: a long-press ON an internal link opens its action sheet instead (no selection, the caret stays)
  window.addEventListener("contextmenu", e => { if (!inEd(e.target)) return; e.stopImmediatePropagation(); if (linkSheetAt(e)) e.preventDefault(); }, true);

  // ---- M9.5 background + resume keeps the place in the note: the active note's caret is saved when the app goes to
  // the background, and the first load of that note after a cold start (the OS killed the backgrounded process) puts
  // it back, unfocused (no keyboard on launch). Tabs and the active note come back through workspace.json already.
  const CKEY = "os.m.caret";
  const saveCaret = () => {
    const g = live(), c = g && Ed.cm(g), t = g && g.tabs[g.active];
    if (!c || !t || t.kind) return;
    try { localStorage.setItem(CKEY, JSON.stringify({ v: vaultPath, n: t.name, a: c.state.selection.main.anchor, h: c.state.selection.main.head })); } catch (e) { /* storage off */ }
  };
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "hidden") saveCaret(); });
  window.addEventListener("pagehide", saveCaret);
  let caretBack = null;
  try { caretBack = JSON.parse(localStorage.getItem(CKEY) || "null"); } catch (e) { /* none */ }
  setTimeout(() => { caretBack = null; }, 60000);   // a restore is for the launch, not a later open of that note
  if (caretBack && typeof window.loadActive === "function") {
    const origLoad = window.loadActive;
    window.loadActive = async function (g) {
      const r = await origLoad.apply(this, arguments);
      const s = caretBack, t = g && g.tabs[g.active], c = t && !t.kind && Ed.cm(g);
      if (s && c && s.v === vaultPath && s.n === t.name) {
        caretBack = null;
        const L = c.state.doc.length;
        c.dispatch({ selection: { anchor: Math.min(s.a, L), head: Math.min(s.h, L) }, scrollIntoView: true });
      }
      return r;
    };
  }

  // ---- android-p2b: ONE bottom action sheet (.m-asheet) for the Files sort orders (M4.5), the long-press file /
  // folder / link sheets (M7.5 M7.6 M7.7) and the tab-count sheet (M6.2): a drag handle, an optional title and sub
  // line, then items (label, icon, red, checked) and separators. Back or a tap on its own scrim closes the sheet
  // alone: the drawer or the tab switcher under it stays.
  const asScrim = document.createElement("div");
  asScrim.id = "mascrim"; asScrim.hidden = true;
  const asBox = document.createElement("div");
  asBox.className = "m-asheet"; asBox.hidden = true; asBox.setAttribute("role", "menu");
  document.body.append(asScrim, asBox);
  const asOpen = () => !asBox.hidden;
  function closeAS() { asBox.hidden = true; asScrim.hidden = true; }
  asScrim.addEventListener("click", e => { e.stopPropagation(); closeAS(); });
  function openAS(o) {
    asBox.textContent = ""; asBox.className = "m-asheet" + (o.cls ? " " + o.cls : "");
    const div = (cls, t) => { const d = document.createElement("div"); d.className = cls; if (t != null) d.textContent = t; return d; };
    asBox.append(div("m-ashandle"));
    if (o.title) asBox.append(div("m-astitle", o.title));
    if (o.sub) asBox.append(div("m-assub", o.sub));
    for (const it of o.items) {
      if (it === "sep") { if (asBox.lastChild && !asBox.lastChild.classList.contains("m-assep")) asBox.append(div("m-assep")); continue; }
      const d = div("m-asitem" + (it.red ? " m-red" : ""));
      d.setAttribute("role", it.checked == null ? "menuitem" : "menuitemradio");
      if (it.checked != null) d.setAttribute("aria-checked", it.checked ? "true" : "false");
      d.innerHTML = (it.icon ? svg(it.icon) : "") + "<span></span>" + (it.checked ? svg('<path d="M5 12l5 5L20 7"/>') : "");
      d.querySelector("span").textContent = it.label;
      d.onclick = async e => { e.stopPropagation(); closeAS(); await it.run(); sync(); };
      asBox.append(d);
    }
    if (asBox.lastChild.classList.contains("m-assep")) asBox.lastChild.remove();
    asBox.scrollTop = 0;
    asBox.hidden = false; asScrim.hidden = false;
  }

  // ---- android-p3a M1.4: a vault manager row opens its page (stock): the vault name, "This vault is located at
  // <path>", then Open vault, Rename vault, Forget vault, Delete vault. Rename / Forget / Delete go through m_vault
  // (mobilefs.rs), which refuses the open vault and any folder holding it; Forget only drops the list entry (the
  // files stay), Delete asks first and removes the folder. The open vault's own row offers Open vault alone (= back
  // to it). A long-press on a row does nothing (no menu, no selection). The page sits on the action sheet layer, so
  // back closes it alone and the manager stays.
  const reRecent = () => { try { if (typeof window.loadRecent === "function") window.loadRecent(); } catch (e) {} };
  const vErr = t => { let e = asBox.querySelector(".m-aserr"); if (!e) { e = document.createElement("div"); e.className = "m-assub m-aserr"; asBox.append(e); } e.textContent = t; };
  function vaultRename(p) {
    openAS({ title: "Rename vault", items: [] });
    const inp = document.createElement("input");
    inp.className = "m-asinput"; inp.type = "text"; inp.value = vname(p); inp.setAttribute("aria-label", "Vault name");
    const row = document.createElement("div"); row.className = "m-asbtns";
    const mk = (t, f) => { const b = document.createElement("button"); b.type = "button"; b.textContent = t; b.onclick = e => { e.stopPropagation(); f(); }; return b; };
    const go = async () => {
      const n = inp.value.trim();
      if (!n || n === vname(p)) return closeAS();
      try { await inv("m_vault", { op: "rename", path: p, name: n }); } catch (er) { return vErr(String(er)); }
      closeAS(); reRecent();
    };
    inp.addEventListener("keydown", e => { if (e.key === "Enter") { e.preventDefault(); go(); } });
    row.append(mk("Cancel", closeAS), mk("Rename", go));
    asBox.append(inp, row);
  }
  function vaultPage(p) {
    const cur = typeof vaultPath !== "undefined" ? vaultPath : null, isCur = p === cur;
    const open = { label: "Open vault", icon: '<path d="M4 6h6l2 2h8v10H4z"/>', run: () => {
      if (isCur || typeof openVault !== "function") { const pk = $m("picker"); if (pk) pk.hidden = true; return; }
      openVault(p, true); } };
    if (isCur) return openAS({ cls: "m-vpage", title: vname(p), sub: "This vault is located at " + p, items: [open] });
    openAS({ cls: "m-vpage", title: vname(p), sub: "This vault is located at " + p, items: [open,
      { label: "Rename vault", icon: '<path d="M4 20h4L19 9l-4-4L4 16z"/>', run: () => vaultRename(p) },
      { label: "Forget vault", icon: '<path d="M6 6l12 12M18 6L6 18"/>', run: () => openAS({ title: "Forget vault",
        sub: "Remove \"" + vname(p) + "\" from the vault list without deleting any files. The folder stays at " + p + ".",
        items: [{ label: "Forget", run: async () => { try { await inv("m_vault", { op: "forget", path: p }); } catch (er) { return; } reRecent(); } },
          { label: "Cancel", run: () => {} }] }) },
      { label: "Delete vault", red: true, icon: '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>', run: () => openAS({ title: "Delete vault",
        sub: "Permanently delete the vault \"" + vname(p) + "\" and every file in it? This cannot be undone.",
        items: [{ label: "Delete", red: true, run: async () => { try { await inv("m_vault", { op: "delete", path: p }); } catch (er) { return; } reRecent(); } },
          { label: "Cancel", run: () => {} }] }) }] });
  }
  const precent = $m("p-recent");
  if (precent) {
    precent.addEventListener("contextmenu", e => { e.preventDefault(); e.stopPropagation(); }, true);
    precent.addEventListener("click", e => {
      const li = e.target.closest && e.target.closest("#p-recent li");
      if (!li || e.target.closest(".p-newwin")) return;
      const s = li.querySelector("span"); if (!s) return;
      e.stopPropagation(); e.preventDefault(); vaultPage(s.textContent);
    }, true);
  }

  // the vault's files and folders with their times (m_stats, mobilefs.rs): the drawer counts, the time sort orders
  let stCache = null;
  const mStats = async () => {
    if (!stCache) { try { stCache = typeof inv === "function" ? await inv("m_stats") : null; } catch (e) { stCache = null; } }
    return stCache || [];
  };
  const plural = (n, w) => n + " " + w + (n === 1 ? "" : "s");
  const countLine = (st, dir) => {
    const pre = dir ? dir + "/" : "";
    const sub = st.filter(e => !dir || e.path.startsWith(pre));
    return plural(sub.filter(e => !e.dir).length, "file") + ", " + plural(sub.filter(e => e.dir).length, "folder");
  };

  // ---- M4.4 the drawer's foot: the vault name + a chevron (the vault list, M1.1), "N files, M folders" and the
  // Settings gear. main.js's own #vbar (#vswitch + #sgear, their handlers) moves into it; below the view pill.
  const dfoot = document.createElement("div");
  dfoot.className = "m-dfoot";
  const dcount = document.createElement("div");
  dcount.className = "m-dcount";
  if (vbar) {
    side.insertBefore(dfoot, vbar);
    dfoot.append(vbar, dcount);
    const chev = document.createElement("span");
    chev.className = "m-dchev"; chev.innerHTML = svg('<path d="M7 10l5 5 5-5"/>');
    chev.onclick = e => { e.stopPropagation(); if (vswitch && !vmenuOpen()) vswitch.click(); };
    vbar.insertBefore(chev, $m("sgear"));
  }
  const paintCount = async () => { dcount.textContent = countLine(await mStats(), ""); };

  // ---- M4.5 the Files view's sort orders (stock's six, phone only, localStorage os.m.sort): a sort button in the
  // Files toolbar (the slot main.js reserves for it) opens them as a bottom sheet. Folders stay above the notes; the
  // name orders sort the folders too, the time orders keep them A to Z. Created time = the birth time where the
  // platform has one; on Android it is the modified time (mobilefs.rs).
  const SORTS = [["alphabetical", "File name (A to Z)"], ["alphabeticalReverse", "File name (Z to A)"],
    ["byModifiedTime", "Modified time (new to old)"], ["byModifiedTimeReverse", "Modified time (old to new)"],
    ["byCreatedTime", "Created time (new to old)"], ["byCreatedTimeReverse", "Created time (old to new)"]];
  const SKEY = "os.m.sort";
  const sortKey = () => { try { const k = localStorage.getItem(SKEY); return SORTS.some(s => s[0] === k) ? k : "alphabetical"; } catch (e) { return "alphabetical"; } };
  const up = s => s.replace(/[a-z]/g, c => c.toUpperCase());
  const byName = (a, b) => { const x = up(a), y = up(b); return x < y ? -1 : x > y ? 1 : a < b ? -1 : a > b ? 1 : 0; };
  const rowPath = r => r.dataset.note != null ? r.dataset.note + ".md" : r.dataset.canvas || r.dataset.file || "";
  const rowName = r => { const n = r.querySelector(".tn"); return n ? n.textContent : ""; };
  async function applySort() {
    const tree = $m("tree"), k = sortKey();
    if (!tree) return;
    const time = /Time/.test(k), rev = /Reverse$/.test(k), cre = /Created/.test(k);
    const tm = new Map(time ? (await mStats()).map(e => [e.path, cre ? e.ctime : e.mtime]) : []);
    const nc = (a, b) => byName(rowName(a), rowName(b));
    const sortBox = box => {
      const kids = [...box.children], dirs = [], files = [];
      for (let i = 0; i < kids.length; i++) {
        const r = kids[i];
        if (!r.classList.contains("trow")) return;          // a row being renamed or anything unknown: leave the box
        if (r.classList.contains("folder")) {
          const t = kids[i + 1] && kids[i + 1].classList.contains("tkids") ? kids[++i] : null;
          dirs.push([r, t]);
        } else files.push(r);
      }
      dirs.sort((a, b) => (k === "alphabeticalReverse" ? -1 : 1) * nc(a[0], b[0]));
      files.sort((a, b) => {
        if (!time) return (rev ? -1 : 1) * nc(a, b);
        const d = (tm.get(rowPath(a)) || 0) - (tm.get(rowPath(b)) || 0);
        return d ? (rev ? d : -d) : nc(a, b);
      });
      for (const [r, t] of dirs) { box.append(r); if (t) { box.append(t); sortBox(t); } }
      for (const r of files) box.append(r);
    };
    sortBox(tree);
  }
  const sortB = btn("m-sortbtn", "Change sort order", svg('<path d="M3 6h11M3 12h8M3 18h5M17 4v16M14 17l3 3 3-3"/>'));
  sortB.onclick = e => {
    e.stopPropagation();
    const cur = sortKey();
    const items = [];
    SORTS.forEach(([k, label], i) => {
      if (i && i % 2 === 0) items.push("sep");                // name | modified | created
      items.push({ label, checked: k === cur, run: async () => {
        try { localStorage.setItem(SKEY, k); } catch (err) { /* storage off: this session only */ }
        stCache = null; await applySort();
      } });
    });
    openAS({ items });
  };
  { const fb = $m("bar"), hs = fb && fb.querySelector(".hslot"); if (hs) hs.replaceWith(sortB); else if (fb) fb.append(sortB); }
  if (typeof window.refreshTree === "function") {
    const origTree = window.refreshTree;
    window.refreshTree = async function () {
      const r = await origTree.apply(this, arguments);
      stCache = null;
      await applySort();
      if (drawerOpen()) paintCount();
      return r;
    };
  }
  // every drawer open re-reads the times (a note saved since changes its modified-time place) and the counts
  new MutationObserver(() => {
    const o = drawerOpen();
    if (o && !dfoot.dataset.open) { stCache = null; paintCount(); applySort(); }
    dfoot.dataset.open = o ? "1" : "";
  }).observe(document.body, { attributes: true, attributeFilter: ["class"] });

  // ---- M6.2 the switcher's count opens New tab / Undo close tab / Close N tabs (N > 1). Close N tabs leaves one empty
  // New tab (stock); every closed note goes on the undo-close list.
  function tcountSheet(n) {
    const items = [
      { label: "New tab", icon: '<path d="M12 5v14M5 12h14"/>', run: async () => {
        closeTabs(); quiet(); if (typeof cmdNewEmptyTab === "function") await cmdNewEmptyTab(); } },
      { label: "Undo close tab", icon: '<path d="M9 14L4 9l5-5"/><path d="M4 9h11a5 5 0 0 1 0 10h-3"/>', run: async () => {
        quiet(); if (typeof undoCloseTab === "function") await undoCloseTab(); sync(); if (!tabsBox.hidden) paintTabs(); } },
    ];
    if (n > 1) items.push("sep", { label: "Close " + n + " tabs", red: true, icon: '<path d="M6 6l12 12M18 6L6 18"/>', run: async () => {
      closeTabs(); quiet();
      if (typeof closeAllTabs === "function") for (const g of [...groups()].reverse()) await closeAllTabs(g);
      if (typeof cmdNewEmptyTab === "function") await cmdNewEmptyTab();
    } });
    openAS({ items });
  }

  // ---- M6.4 a long-press on a switcher card: Close, Close others, Close tabs after, Close all | Pin (Unpin) | Link
  // with tab (Unlink tab when linked). The close rows keep desktop's absence rules (R38.4-6: Close others / Close all
  // need a second tab in the group, Close tabs after a tab to the right). Link with tab lists the other open tabs in a
  // second sheet. Back closes the sheet alone; the switcher stays and repaints after every action.
  function cardSheet(g, i, t) {
    const n = g.tabs.length, X = '<path d="M6 6l12 12M18 6L6 18"/>';
    const after = async () => { quiet(); sync(); if (!tabsBox.hidden) paintTabs(); };
    const at = () => g.tabs.indexOf(t);                // the index again at run time (a repaint may have shifted it)
    const items = [{ label: "Close", icon: X, run: async () => { if (at() >= 0) await closeTab(g, at()); await after(); } }];
    if (n > 1) items.push({ label: "Close others", icon: X, run: async () => { if (at() >= 0) await closeOtherTabs(g, at()); await after(); } });
    if (i < n - 1) items.push({ label: "Close tabs after", icon: '<path d="M5 12h14M13 6l6 6-6 6"/>', run: async () => {
      if (at() >= 0) await closeTabsAfter(g, at()); await after(); } });
    if (n > 1) items.push({ label: "Close all", icon: X, red: true, run: async () => { await closeAllTabs(g); await after(); } });
    items.push("sep", { label: t.pinned ? "Unpin" : "Pin", icon: '<path d="M12 17v5M8 3h8l-1 7 3 4H6l3-4z"/>', run: async () => {
      await togglePin(g, t); await after(); } });
    const linked = typeof isLinked === "function" && isLinked(g, t, i);
    items.push(linked
      ? { label: "Unlink tab", icon: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
          run: async () => { unlinkTab(g, t); await after(); } }
      : { label: "Link with tab", icon: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
          run: () => {
            const others = allTabs().filter(o => o.t !== t);
            openAS({ title: "Link with tab", items: others.length
              ? others.map(o => ({ label: tabTitle(o.g, o.i, o.t), run: async () => { linkTabs(t, o.t); await after(); } }))
              : [{ label: "(no other tabs)", run: () => {} }] });
          } });
    openAS({ title: tabTitle(g, i, t), items });
  }

  // ---- M7.5 / M7.6 a Files-row long-press opens a bottom action sheet (stock mobile) instead of the desktop menu at
  // the finger; the long-press leaves no text selection behind. File: "Created at <birth time>" (= modified time on
  // Android, mobilefs.rs), Open in new tab | Make a copy, Move file to..., Bookmark... | Copy path | Share | Rename...
  // (in the row, M10.4), Delete. Folder: titled "<folder>" over "N files, M folders" (recursive, dot entries out),
  // New note, New folder, New canvas | Search in folder | Copy path | Rename... (in the row), Delete (to .trash).
  // Not offered (no backend yet): Merge entire file with..., Open version history, folder Make a copy / Move folder to...
  const noSel = () => { const s = getSelection(); if (s) s.removeAllRanges(); };
  const pad2 = n => String(n).padStart(2, "0");
  const ymdhm = ms => { const d = new Date(ms); return d.getFullYear() + "-" + pad2(d.getMonth() + 1) + "-" + pad2(d.getDate()) + " " + pad2(d.getHours()) + ":" + pad2(d.getMinutes()); };
  async function copyText(s) {
    try { await navigator.clipboard.writeText(s); return; } catch (e) { /* fall back to a copy event */ }
    const t = document.createElement("textarea"); t.value = s; t.style.cssText = "position:fixed;left:-9999px;top:0";
    document.body.append(t); t.select(); try { document.execCommand("copy"); } catch (e) { /* nothing to copy into */ }
    t.remove(); noSel();
  }
  async function copyNote(nm) {
    let body = "";
    try { body = await inv("read_note", { name: nm }); } catch (e) { return; }
    for (let i = 1; i < 100; i++) {
      const r = await createNote(nm + " " + i, body);
      if (r === "exists") continue;
      if (r === "ok" && typeof window.refreshTree === "function") await window.refreshTree();
      return;
    }
  }
  if (typeof window.noteMenu === "function") window.noteMenu = async function (e, nm) {
    if (e) { e.preventDefault(); e.stopPropagation(); }
    if (typeof closeMenu === "function") closeMenu();
    noSel(); setTimeout(noSel, 60);
    const row = e && e.target && e.target.closest ? e.target.closest("#tree .trow") : null;
    const st = (await mStats()).find(x => x.path === nm + ".md");
    const items = [
      { label: "Open in new tab", icon: '<path d="M12 5v14M5 12h14"/>', run: () => openNewTab(nm) }, "sep",
      { label: "Make a copy", icon: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/>', run: () => copyNote(nm) },
      { label: "Move file to...", icon: '<path d="M3 7h6l2 2h10v10H3z"/>', run: () => mvSheet(nm) },
      { label: "Bookmark...", icon: '<path d="M6 4h12v16l-6-4-6 4z"/>', run: async () => { await rowOpen(nm); bmSheet(); } }, "sep",
      { label: "Copy path", icon: '<path d="M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1"/><path d="M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1"/>', run: () => copyText(nm + ".md") }];
    const d = window.__osAndroid;
    if (d && d.share) items.push("sep", { label: "Share", icon: '<path d="M12 3v12M7 8l5-5 5 5"/><path d="M5 14v6h14v-6"/>', run: async () => {
      let t = ""; try { t = await inv("read_note", { name: nm }); } catch (er) { t = ""; }
      d.share(nm.split("/").pop(), t); } });
    items.push("sep",
      { label: "Rename...", icon: '<path d="M4 20h4L19 9l-4-4L4 16z"/>', run: () => { if (row && row.isConnected) rowRename(row, nm); } },
      { label: "Delete", red: true, icon: '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>', run: () => askDelete(nm) });
    openAS({ title: st ? "Created at " + ymdhm(st.ctime) : null, items });
  };
  // a folder row renamed in place, as M10.4 does for a note row: Enter = m_folder rename (one move on disk), the open
  // tabs below it follow; Escape, blur and back cancel
  function folderRename(row, f) {
    const tn = row.querySelector(".tn"); if (!tn || rowRn) return;
    const old = f.split("/").pop(), dir = f.slice(0, f.length - old.length);
    const inp = document.createElement("input");
    inp.type = "text"; inp.className = "m-trn"; inp.value = old; inp.spellcheck = false;
    inp.setAttribute("autocapitalize", "off"); inp.setAttribute("enterkeyhint", "done");
    tn.hidden = true; tn.after(inp);
    let done = false;
    const end = async ok => {
      if (done) return; done = true; rowRn = null;
      const nn = inp.value.trim();
      inp.remove(); tn.hidden = false; unRnPad();
      if (!ok || !nn || nn === old || /[\\/]/.test(nn)) return;
      for (const g of groups()) await flushSave(g);
      try { await inv("m_folder", { op: "rename", path: f, to: dir + nn }); } catch (er) { return; }
      for (const g of groups()) for (const t of g.tabs) if (!t.kind && t.name && t.name.startsWith(f + "/")) t.name = dir + nn + t.name.slice(f.length);
      stCache = null; await window.refreshTree(); if (typeof updateTitle === "function") updateTitle();
    };
    inp.addEventListener("keydown", ev => {
      ev.stopPropagation();
      if (ev.key === "Enter") { ev.preventDefault(); end(true); }
      else if (ev.key === "Escape") { ev.preventDefault(); end(false); }
    });
    inp.addEventListener("blur", () => end(false));
    for (const k of ["click", "mousedown", "contextmenu", "touchstart"]) inp.addEventListener(k, ev => ev.stopPropagation());
    rowRn = { cancel: () => end(false) };
    inp.focus(); inp.select();
    const d = window.__osAndroid; if (d && d.showIme) d.showIme();
  }
  if (typeof window.folderMenu === "function") window.folderMenu = async function (e, f) {
    if (e) { e.preventDefault(); e.stopPropagation(); }
    if (typeof closeMenu === "function") closeMenu();
    noSel(); setTimeout(noSel, 60);
    const row = e && e.target && e.target.closest ? e.target.closest("#tree .trow") : null;
    const below = (await mStats()).filter(x => x.path.startsWith(f + "/"));
    const nf = below.filter(x => !x.dir).length, nd = below.filter(x => x.dir).length;
    const items = [], ico = { "New note": '<path d="M6 3h8l4 4v14H6z"/><path d="M12 11v6M9 14h6"/>',
      "New folder": '<path d="M3 7h6l2 2h10v10H3z"/><path d="M12 12v4M10 14h4"/>', "New canvas": '<rect x="4" y="4" width="16" height="16" rx="2"/><path d="M9 4v16"/>' };
    // main.js's own folder rows (New note / New folder / New canvas), the wired ones only
    if (typeof folderMenuFull === "function") folderMenuFull((label, fn) => { if (fn && ico[label]) items.push({ label, icon: ico[label], run: fn }); }, () => {}, f);
    items.push("sep",
      { label: "Search in folder", icon: '<circle cx="11" cy="11" r="6"/><path d="M20 20l-4-4"/>', run: () => {
        if (typeof sideOpen !== "undefined" && !sideOpen && typeof cmdToggleSide === "function") cmdToggleSide();
        if (typeof setPane === "function") setPane("search");
        const s = $m("sinput"); if (s) { s.value = 'path:"' + f + '" '; s.focus(); if (typeof runSearch === "function") runSearch(); } } },
      "sep", { label: "Copy path", icon: '<path d="M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1"/><path d="M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1"/>', run: () => copyText(f) },
      "sep", { label: "Rename...", icon: '<path d="M4 20h4L19 9l-4-4L4 16z"/>', run: () => { if (row && row.isConnected) folderRename(row, f); } },
      { label: "Delete", red: true, icon: '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>', run: () => openAS({ title: "Delete folder",
        sub: f + " and " + plural(nf, "file") + " in it go to the vault's .trash", items: [
          { label: "Delete", red: true, run: async () => {
            for (const g of groups()) { await flushSave(g); for (let i = g.tabs.length - 1; i >= 0; i--) { const t = g.tabs[i];
              if (!t.kind && t.name && t.name.startsWith(f + "/")) await closeTab(g, i); } }
            try { await inv("m_folder", { op: "trash", path: f }); } catch (er) { return; }
            stCache = null; await window.refreshTree(); } },
          { label: "Cancel", run: () => {} }] }) });
    openAS({ title: f.split("/").pop(), sub: plural(nf, "file") + ", " + plural(nd, "folder"), items });
  };

  // ---- M7.7 a long-press on an internal link [[target]] in the editor: a bottom sheet titled with the target, Open
  // link, Open in new tab | Edit link | Copy path, Rename..., Move file to... | Share. The long-press makes no text
  // selection, leaves the caret where it was and raises no keyboard. Not offered: Bookmark... (it bookmarks the ACTIVE
  // note in this app), splits and windows (no such surfaces on a phone).
  function linkSheetAt(e) {
    const g = typeof fg === "function" ? fg() : null, cm = g && typeof Ed !== "undefined" && Ed.cm ? Ed.cm(g) : null;
    if (!cm) return false;
    const pos = cm.posAtCoords({ x: e.clientX, y: e.clientY });
    if (pos == null) return false;
    const ln = cm.state.doc.lineAt(pos), re = /\[\[([^\]\n]+)\]\]/g;
    let m, hit = null;
    while ((m = re.exec(ln.text))) { const a = ln.from + m.index, b = a + m[0].length; if (pos >= a && pos <= b) { hit = { a, b, inner: m[1] }; break; } }
    if (!hit) return false;
    const n = hit.inner.split("|")[0].split("#")[0].trim();
    if (!n) return false;
    noSel(); setTimeout(noSel, 60);
    const goNew = async () => { await flushSave(g); g.tabs.push(mkTab(n)); g.active = g.tabs.length - 1; await loadActive(g); };
    const items = [
      { label: "Open link", icon: '<path d="M14 4h6v6M20 4l-9 9"/><path d="M18 14v6H4V6h6"/>', run: () => navigate(g, n) },
      { label: "Open in new tab", icon: '<path d="M12 5v14M5 12h14"/>', run: goNew }, "sep",
      { label: "Edit link", icon: '<path d="M4 20h4L19 9l-4-4L4 16z"/>', run: () => {
        const s = hit.a + 2, t = s + hit.inner.split("|")[0].length;
        cm.focus(); cm.dispatch({ selection: { anchor: s, head: t }, scrollIntoView: true });
        const d = window.__osAndroid; if (d && d.showIme) d.showIme(); } }, "sep",
      { label: "Copy path", icon: '<path d="M10 14a5 5 0 0 0 7 0l3-3a5 5 0 0 0-7-7l-1 1"/><path d="M14 10a5 5 0 0 0-7 0l-3 3a5 5 0 0 0 7 7l1-1"/>', run: () => copyText(n + ".md") },
      { label: "Rename...", icon: '<path d="M4 20h4L19 9l-4-4L4 16z"/>', run: async () => { await navigate(g, n); cmdRename(); } },
      { label: "Move file to...", icon: '<path d="M3 7h6l2 2h10v10H3z"/>', run: () => mvSheet(n) }];
    const d = window.__osAndroid;
    if (d && d.share) items.push("sep", { label: "Share", icon: '<path d="M12 3v12M7 8l5-5 5 5"/><path d="M5 14v6h14v-6"/>', run: async () => {
      let t = ""; try { t = await inv("read_note", { name: n }); } catch (er) { t = ""; }
      d.share(n.split("/").pop(), t); } });
    openAS({ title: n.split("/").pop(), items });
    return true;
  }

  // ---- M6.3 / M6.6 the empty New tab (the switcher's +, Close N tabs) shows stock mobile's New tab view: three big
  // buttons, Create new note (a new note IN this tab), Go to file (the quick switcher) and Close (this tab; the previous
  // one becomes active, M6.5). main.js paints its desktop "No file is open" line; the phone swaps in the buttons.
  function newTabView(g) {
    const t = g && g.active >= 0 ? g.tabs[g.active] : null;
    if (!t || !isEmptyTab(t) || !g.unkEl || g.unkEl.hidden || g.unkEl.querySelector(".m-ntview")) return;
    const box = document.createElement("div"); box.className = "m-ntview";
    const mk = (label, run) => { const b = document.createElement("button"); b.type = "button"; b.className = "m-ntbtn";
      b.textContent = label; b.addEventListener("click", e => { e.preventDefault(); e.stopPropagation(); run(); }); box.append(b); };
    mk("Create new note", async () => {
      let name = "Untitled-" + Date.now() % 10000, r = await createNote(name);
      for (let i = 2; r === "exists" && i < 100; i++) { name = "Untitled-" + Date.now() % 10000 + "-" + i; r = await createNote(name); }
      if (r !== "ok") return;
      const i = g.tabs.indexOf(t); if (i < 0) return;
      const nt = mkTab(name); nt.lid = t.lid; g.tabs[i] = nt; g.active = i;
      if (typeof window.refreshTree === "function") await window.refreshTree();
      await loadActive(g);
    });
    mk("Go to file", () => cmdQuickSwitch());
    mk("Close", () => { const i = g.tabs.indexOf(t); if (i >= 0) closeTab(g, i); });
    g.unkEl.textContent = ""; g.unkEl.append(box);
  }
  if (typeof window.loadActive === "function") {
    const origLoad = window.loadActive;
    window.loadActive = async function (g) { const r = await origLoad.apply(this, arguments); newTabView(g); return r; };
  }

  // ---- keep the header + nav in step with the focused pane
  let parked = null;                  // the pane whose .modebtn/.morebtn currently sit in #mhdr
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
    nBack.disabled = !s.b; nFwd.disabled = !s.f; hBack.disabled = !s.b; hFwd.disabled = !s.f;
    const tbr = g.pane && g.pane.querySelector(":scope > .tabbar"), tbs = tbr && tbr.querySelector(":scope > .tabs");
    if (tbs && !tbr.querySelector(":scope > .m-newtab")) {   // M13.1 the tablet strip's + (stock: an empty New tab)
      const nb = btn("m-newtab", "New tab", svg('<path d="M12 5v14M5 12h14"/>'));
      nb.onclick = async () => { if (typeof cmdNewEmptyTab === "function") await cmdNewEmptyTab(); sync(); };
      tbs.after(nb);
    }
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

  // ---- M3.4 accessibility labels (we do better than stock, which leaves Search, +, the tab count and Menu bare):
  // every icon control (its own text is empty or not a word: an svg, a count, a glyph) gets an aria-label from its
  // title, so TalkBack and the OS accessibility tree name it. Labels we set follow later title changes (the reading /
  // editing .modebtn flips its title); a label the code set itself is never touched.
  const A11Y = "button,[role=button],[role=tab],[role=menuitem],[role=radio],[role=checkbox],.clickable-icon";
  const a11y = root => {
    const els = root.querySelectorAll ? [...root.querySelectorAll(A11Y)] : [];
    if (root.matches && root.matches(A11Y)) els.push(root);
    for (const e of els) {
      const own = e.getAttribute("aria-label"), mine = e.hasAttribute("data-m-al");
      if ((own && !mine) || e.hasAttribute("aria-labelledby")) continue;
      if (/[A-Za-z]{2,}/.test((e.textContent || "").replace(/\s+/g, " "))) { if (mine) { e.removeAttribute("aria-label"); e.removeAttribute("data-m-al"); } continue; }
      const t = (e.getAttribute("title") || "").trim();
      if (t && own !== t) { e.setAttribute("aria-label", t); e.setAttribute("data-m-al", ""); }
    }
  };
  let a11yDue = 0;
  const a11yLater = () => { if (!a11yDue) a11yDue = requestAnimationFrame(() => { a11yDue = 0; a11y(document.body); }); };
  // typing churns the editor DOM: those records carry no controls, skip them so a keystroke costs nothing here
  new MutationObserver(rs => { if (rs.some(r => !(r.target.closest && r.target.closest(".cm-content")))) a11yLater(); })
    .observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ["title"] });
  a11yLater();
})();
