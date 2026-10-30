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
   - #mtabs minimal tab switcher
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

  // ---- drawer (M4.1 M4.3 M9.2)
  let paneBefore = null;               // the sidebar pane to restore when a Search-opened drawer closes
  const drawerOpen = () => document.body.classList.contains("m-drawer");
  const openDrawer = pane => {
    closeTabs();
    if (typeof closeMenu === "function") closeMenu();
    if (pane && typeof setPane === "function" && sidePane !== pane) { paneBefore = sidePane; setPane(pane); }
    document.body.classList.add("m-drawer");
  };
  const closeDrawer = () => {
    if (!drawerOpen()) return;
    document.body.classList.remove("m-drawer");
    const a = document.activeElement;
    if (a && $m("side").contains(a)) a.blur();
    if (paneBefore && typeof setPane === "function") { setPane(paneBefore); paneBefore = null; }
  };
  expand.onclick = () => drawerOpen() ? closeDrawer() : openDrawer();
  scrim.onclick = closeDrawer;
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
         !(e.target.closest && e.target.closest("#modal, #settings, .ctxmenu, #mtabs, #mnav, #mhdr, canvas"))
      ? { x: t.clientX, y: t.clientY, lx: t.clientX, ly: t.clientY } : null;
  }, { passive: true, capture: true });
  document.addEventListener("touchmove", e => {
    if (t0) { const t = e.touches[0]; t0.lx = t.clientX; t0.ly = t.clientY; }
  }, { passive: true, capture: true });
  const swipeEnd = () => {
    if (!t0) return;
    const dx = t0.lx - t0.x, dy = t0.ly - t0.y; t0 = null;
    if (Math.abs(dx) < 60 || Math.abs(dy) > Math.abs(dx) * 0.6) return;
    if (dx > 0 && !drawerOpen() && !(typeof modalKind !== "undefined" && modalKind)) openDrawer();
    else if (dx < 0 && drawerOpen()) closeDrawer();
  };
  document.addEventListener("touchend", swipeEnd, { passive: true, capture: true });
  document.addEventListener("touchcancel", swipeEnd, { passive: true, capture: true });

  // ---- tab switcher
  const allTabs = () => (typeof groups === "function" ? groups() : []).flatMap(g => g.tabs.map((t, i) => ({ g, i, t })));
  function closeTabs() { tabsBox.hidden = true; nTabs.classList.remove("on"); }
  const openTabs = () => {
    closeDrawer();
    tabsBox.textContent = "";
    for (const { g, i, t } of allTabs()) {
      const d = document.createElement("div");
      d.className = "m-tab" + (g === live() && i === g.active ? " active" : "");
      d.dataset.tab = g.id + ":" + i;
      d.textContent = t.kind ? (({ gg: "Graph view", lg: "Local graph" })[t.kind] || t.kind) : String(t.name).split("/").pop();
      d.onclick = async () => {
        closeTabs();
        if (typeof focusGroup === "function") focusGroup(g);
        if (typeof switchTab === "function") await switchTab(g, i);
        sync();
      };
      tabsBox.append(d);
    }
    tabsBox.hidden = false; nTabs.classList.add("on");
  };

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
  nMenu.onclick = () => {
    closeTabs(); closeDrawer();
    if (typeof cmdPalette === "function") cmdPalette();
    const i = $m("minput"); if (i && document.activeElement === i) i.blur();   // no keyboard over the list; back closes it
  };

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
    if (!tabsBox.hidden) { closeTabs(); return "tabs"; }
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
