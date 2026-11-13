// SPDX-License-Identifier: GPL-3.0-or-later
/* OMENU — the Obsidian-shaped context menu (issue #37, C2 "one menu component").
   Loaded BEFORE main.js; it calls main.js's placeMenu/closeMenu/updateTitle only
   at run time, so the load order is the only coupling.

   THE DOM IS STOCK 1.13.7's (docs: goal recon37 RECON.md §A5), so a community
   theme that styles `.menu` styles ours:
     div.menu[data-om]
       div.menu-grabber
       div.menu-scroll
         div.menu-group
           div.menu-item.tappable[data-section]   (+ .is-disabled / .is-warning / .has-submenu / .selected)
             div.menu-item-icon                   (only when the row has an icon or a check gutter)
             div.menu-item-title
             div.menu-item-icon.mod-submenu       (the chevron of a submenu row)
         div.menu-separator
   A SUBMENU is a second div.menu, a SIBLING in <body> (stock), never a child:
   the parent stays open beside it. root._sub points at it; closeMenu() removes it.
   NOT .ctxmenu: style.css's `.ctxmenu div` would paint every inner div as a row.

   Rows: { title, fn, why, icon, section, chk, warn, sub }
     fn   — called on click/Enter (the row closes the menu itself, as the .ctxmenu rows did)
     why  — set (a string) = the row is DISABLED, `why` is its hover title
     chk  — undefined = no radio gutter; false/true = gutter, true = the check mark
     sub  — array of rows (or a function returning one): the row opens a submenu
   "---" is a separator. Census (main.js menuTok): [menu:] reads .menu-item-title
   and .menu-separator, [mdis:] .is-disabled, [menu2:]/[mg2:]/[mgy2:] the submenu,
   [msel:] the keyboard-selected row. */
const OMenu = (() => {
  const CHEVRON = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"' +
    ' stroke-linecap="round" stroke-linejoin="round"><path d="M6 4l4 4-4 4"/></svg>';
  const CHECK = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2"' +
    ' stroke-linecap="round" stroke-linejoin="round"><path d="M3 8.5l3.5 3.5L13 5"/></svg>';
  const div = cls => { const d = document.createElement("div"); d.className = cls; return d; };
  const refresh = () => { if (typeof updateTitle === "function") updateTitle(); };

  // a menu SHELL: root + grabber + scroll; b.item()/b.sep() fill it in order
  function shell() {
    const el = div("menu");
    el.dataset.om = "1";
    // a press on the menu must not take focus from the editor (rows are not focusable, so the
    // default action would blur it to <body> and keys after the menu closes would go nowhere).
    // CAPTURE: a row's own mousedown stops propagation, so a bubbling listener never saw row presses.
    el.addEventListener("mousedown", ev => ev.preventDefault(), true);
    el.appendChild(div("menu-grabber"));
    const scroll = el.appendChild(div("menu-scroll"));
    let grp = null;
    const b = {
      el,
      item(r) {
        if (!grp) grp = scroll.appendChild(div("menu-group"));
        const d = div("menu-item tappable");
        if (r.section) d.dataset.section = r.section;
        if (r.icon || r.chk !== undefined) {
          const ic = d.appendChild(div("menu-item-icon"));
          if (r.icon) ic.innerHTML = r.icon;
          else if (r.chk) { ic.innerHTML = CHECK; d.classList.add("mod-checked"); }
        }
        d.appendChild(div("menu-item-title")).textContent = r.title;
        if (r.warn) d.classList.add("is-warning");
        if (r.sub) { d.classList.add("has-submenu"); d.appendChild(div("menu-item-icon mod-submenu")).innerHTML = CHEVRON; d._sub = r.sub; }
        if (r.why != null) {
          d.classList.add("is-disabled"); d.setAttribute("aria-disabled", "true");
          if (r.why) d.title = r.why;
        } else {
          d.addEventListener("mousedown", ev => ev.stopPropagation());
          d.addEventListener("click", ev => { ev.stopPropagation(); activate(el, d); });
        }
        d._fn = r.fn;
        d.addEventListener("mouseenter", () => hover(el, d));
        grp.appendChild(d);
        return d;
      },
      sep() { grp = null; scroll.appendChild(div("menu-separator")); },
      clear() { closeSub(el); scroll.innerHTML = ""; grp = null; },
      add(rows) { for (const r of rows || []) r === "---" ? b.sep() : b.item(r); return b; },
      /* a plugin hook's rows (ui/plugins/api-ui.js MenuItem: div.menu-item + div.sep, filled into the
         scratch m) join this menu after a rule, in their own order. The item keeps its own click
         (api-ui runs the callback); the row closes this menu, and Enter is that same click. */
      plugin(m) {
        const ds = [...m.children];
        if (!ds.length) return b;
        if (scroll.childElementCount) b.sep();
        for (const d of ds) {
          if (d.classList.contains("sep") || d.classList.contains("menu-separator")) { b.sep(); continue; }
          if (!grp) grp = scroll.appendChild(div("menu-group"));
          d.classList.add("tappable");
          if (d.classList.contains("dis")) { d.classList.add("is-disabled"); d.setAttribute("aria-disabled", "true"); }
          d._fn = () => d.click();
          d.addEventListener("click", () => { if (!d.classList.contains("dis") && typeof menuEl !== "undefined" && menuEl === el) closeMenu(); });
          d.addEventListener("mouseenter", () => hover(el, d));
          grp.appendChild(d);
        }
        return b;
      },
    };
    return b;
  }
  const rowsOf = m => [...m.querySelectorAll(":scope > .menu-scroll .menu-item")];
  const live = m => rowsOf(m).filter(d => !d.classList.contains("is-disabled"));
  function select(m, d, kbd) {   // kbd: a key moved it — scroll it into view (a hover never scrolls: the wheel owns that)
    for (const x of rowsOf(m)) x.classList.toggle("selected", x === d);
    if (d && kbd) d.scrollIntoView({ block: "nearest" });
    refresh();
  }
  function closeSub(m) {
    const s = m._sub;
    if (!s) return;
    closeSub(s);
    s.remove(); m._sub = null;
    for (const x of rowsOf(m)) x.classList.remove("mod-open");
    refresh();
  }
  function openSub(m, d) {
    if (m._sub && m._sub._from === d) return m._sub;
    closeSub(m);
    const rows = typeof d._sub === "function" ? d._sub() : d._sub;
    const b = shell();
    b.add(rows);
    const s = b.el;
    s.classList.add("mod-sub");
    s._from = d; s._parent = m;
    s.style.left = "0px"; s.style.top = "0px";
    document.body.appendChild(s);
    m._sub = s;
    d.classList.add("mod-open");
    // beside the row, flipped to the left when it would leave the window (stock behaviour)
    const r = d.getBoundingClientRect(), pr = m.getBoundingClientRect();
    let x = pr.right - 2;
    if (x + s.offsetWidth > innerWidth) x = Math.max(0, pr.left - s.offsetWidth + 2);
    const y = Math.max(0, Math.min(r.top - 5, innerHeight - s.offsetHeight));
    s.style.left = Math.round(x) + "px"; s.style.top = Math.round(y) + "px";
    s.addEventListener("scroll", refresh);
    refresh();
    return s;
  }
  function hover(m, d) {
    if (d.classList.contains("is-disabled")) { select(m, null); return; }
    select(m, d);
    if (d._sub) openSub(m, d); else closeSub(m);
  }
  function activate(m, d) {
    if (d._sub) { const s = openSub(m, d); select(s, live(s)[0] || null, 1); return; }
    if (d._fn) d._fn(); else if (typeof closeMenu === "function") closeMenu();
  }
  // the deepest open menu of the chain rooted at the open root
  const deepest = m => { while (m && m._sub) m = m._sub; return m; };

  /* keyboard (stock): Up/Down move over the ENABLED rows (wrapping), Right/Enter
     open a submenu on its first row, Left/Escape close the deepest submenu, Escape
     on the root closes the menu. Window CAPTURE, so neither the editor nor the
     global Escape handler sees a key the menu consumed. */
  window.addEventListener("keydown", e => {
    const root = typeof menuEl !== "undefined" ? menuEl : null;
    if (!root || !root.dataset.om) return;
    const m = deepest(root);
    const rows = live(m), cur = rows.findIndex(d => d.classList.contains("selected"));
    const eat = () => { e.preventDefault(); e.stopPropagation(); };
    switch (e.key) {
      case "ArrowDown": eat(); if (rows.length) select(m, rows[(cur + 1) % rows.length], 1); break;
      case "ArrowUp": eat(); if (rows.length) select(m, rows[cur <= 0 ? rows.length - 1 : cur - 1], 1); break;
      case "ArrowRight": eat(); if (cur >= 0 && rows[cur]._sub) activate(m, rows[cur]); break;
      case "ArrowLeft": eat(); if (m._parent) closeSub(m._parent); break;
      case "Enter": eat(); if (cur >= 0) activate(m, rows[cur]); break;
      case "Escape": eat(); if (m._parent) closeSub(m._parent); else if (typeof closeMenu === "function") closeMenu(); break;
      default: break;
    }
  }, true);

  // the whole chain (root + open submenus) as one hit-test: a click in a submenu is IN the menu
  const owns = (root, t) => { for (let m = root; m; m = m._sub) if (m.contains(t)) return true; return false; };
  // census helpers for main.js menuTok
  const census = {
    rows: m => [...m.querySelectorAll(":scope > .menu-scroll .menu-item, :scope > .menu-scroll .menu-separator")],
    label: d => d.classList.contains("menu-separator") ? "---" : (d.querySelector(".menu-item-title") || d).textContent,
  };
  return { shell, owns, closeSub, openSub, select, census, deepest };
})();
