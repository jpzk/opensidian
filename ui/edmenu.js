// SPDX-License-Identifier: GPL-3.0-or-later
/* EDMENU — the editor right-click menu (issue #37, C3-3). Live Preview and
   Source mode share it, as in stock (RECON §A1: "Source mode uses the same
   editor menu as LP").

   THE SPEC is stock 1.13.7's lists, measured (goal recon37 dom/stock/lp-*.json):
     no selection: Add link | Add external link | --- | Format > | Paragraph > |
                   Insert > | --- | Cut | Copy | Paste | Paste as plain text | Select all
     selection:    the same, with Search for “…” | Extract current selection...
                   before Format
   Format / Paragraph / Insert are submenus (OMenu, ui/menu.js). Cut and Copy are
   disabled without a selection, Paste and Paste as plain text without text on
   the clipboard (clip_has_text, src-tauri/src/clip.rs — asked when the menu
   opens; the TEXT only crosses when Paste is picked).
   C3-4: a right-click ON a link or an image embed gets stock's target lists
   instead (lp-link-internal / lp-link-external / lp-image), and the reading
   view gets its link menus plus a one-row "Copy" for a selection
   (openReading; no target = no menu, as in stock). Rows we cannot back yet are
   shown DISABLED with the missing subsystem as the hover title.
   Plugin "editor-menu" rows (ui/plugins/views.js PM.editorMenu) follow the core
   rows after a rule — stock's order.

   Every row edits through the SAME paths the palette/hotkeys use (main.js
   edWrap / edList / edEdit / Ed.replace), so an edit is one undo step and the
   save/census plumbing is the usual one. Loaded before main.js: main.js names
   are used at run time only. */
const EdMenu = (() => {
  const NO_EXTRACT = "Missing subsystem: note extraction (a new note from the selection, linked back) is not implemented";
  const NO_BASE = "Missing subsystem: bases (.base files) are not implemented";
  const groupAt = t => groups().find(g => { const cm = Ed.cm(g); return cm && cm.dom.contains(t); }) || null;

  // ---- text ops: (value, selStart, selEnd) -> [value, selStart, selEnd] (main.js edEdit) ----
  const perLine = f => ch => ch.split("\n").map(l => (l.trim() ? f(l) : l)).join("\n");
  const heading = n => perLine(l => { const m = /^#{1,6}[ \t]+/.exec(l), body = m ? l.slice(m[0].length) : l; return n ? "#".repeat(n) + " " + body : body; });
  const task = perLine(l => {
    const t = /^(\s*)[-*+] \[.\] /.exec(l);
    if (t) return t[1] + l.slice(t[0].length);                       // a task already: off
    const m = /^(\s*)(?:[-*+]|\d+[.)])[ \t]+/.exec(l), ind = m ? m[1] : /^\s*/.exec(l)[0];
    return ind + "- [ ] " + l.slice(m ? m[0].length : ind.length);
  });
  const quote = ch => {
    const L = ch.split("\n"), on = L.every(l => /^>/.test(l));
    return L.map(l => (on ? l.replace(/^> ?/, "") : "> " + l)).join("\n");
  };
  const clearFmt = (v, a, b) => {
    if (a === b) return null;
    const s = v.slice(a, b).replace(/\*\*|~~|==|%%|[*`$]/g, "");
    return [v.slice(0, a) + s + v.slice(b), a, a + s.length];
  };
  const link = (o, c) => (v, a, b) => { const s = v.slice(a, b); return [v.slice(0, a) + o + s + c + v.slice(b), a + o.length, a + o.length + s.length]; };
  const extLink = (v, a, b) => {      // [sel]() — the caret goes where the next thing to type is: the URL, or the text when there is none
    const s = v.slice(a, b), at = s ? a + s.length + 3 : a + 1;
    return [v.slice(0, a) + "[" + s + "]()" + v.slice(b), at, at];
  };
  /* a BLOCK at the caret's line: an empty line takes it in place, a line with
     text gets it below after a blank line (stock Insert>Table, stock-actions.txt) */
  const block = (text, at) => (v, a, b) => {
    const l0 = v.lastIndexOf("\n", a - 1) + 1;
    let l1 = v.indexOf("\n", b); if (l1 < 0) l1 = v.length;
    if (!v.slice(l0, l1).trim()) return [v.slice(0, l0) + text + v.slice(l1), l0 + at, l0 + at];
    const ins = "\n\n" + text;
    return [v.slice(0, l1) + ins + v.slice(l1), l1 + 2 + at, l1 + 2 + at];
  };
  const fence = f => (v, a, b) => {   // ``` / $$: a selection is wrapped, a caret gets an empty block
    if (a === b) return block(f + "\n\n" + f, f.length + 1)(v, a, b);
    const s = v.slice(a, b), t = f + "\n" + s + "\n" + f;
    return [v.slice(0, a) + t + v.slice(b), a + f.length + 1, a + f.length + 1 + s.length];
  };
  const footnote = (v, a, b) => {      // [^n] at the caret, its definition at the end of the note, the caret there
    let n = 1;
    for (const m of v.matchAll(/\[\^(\d+)\]/g)) n = Math.max(n, +m[1] + 1);
    const ref = "[^" + n + "]", v1 = v.slice(0, b) + ref + v.slice(b);
    const v2 = v1 + (v1.endsWith("\n") || !v1 ? "" : "\n") + "\n" + ref + ": ";
    return [v2, v2.length, v2.length];
  };
  const TABLE = "|     |     |\n| --- | --- |\n|     |     |";

  /* the menu for the editor of group g. sel = the selected text ("" = caret),
     clip = the clipboard holds text. Rows run on g, focused first. */
  function rows(g, sel, clip, hit) {
    const go = fn => () => { closeMenu(); if (fg() !== g) focusGroup(g); Ed.focus(g); fn(); };
    const ed = f => go(() => edEdit(f));
    const line = f => go(() => edLine(f));
    const R = (title, fn, section, why) => ({ title, fn, section, why });
    const FORMAT = [
      R("Bold", go(() => edWrap("**")), "selection.format.basic"),
      R("Italic", go(() => edWrap("*")), "selection.format.basic"),
      R("Strikethrough", go(() => edWrap("~~")), "selection.format.basic"),
      R("Highlight", go(() => edWrap("==")), "selection.format.basic"),
      "---",
      R("Code", go(() => edWrap("`", "code")), "selection.format.advanced"),
      R("Math", go(() => edWrap("$", "math")), "selection.format.advanced"),
      R("Comment", go(() => edWrap("%%", "comment")), "selection.format.advanced"),
      "---",
      R("Clear formatting", ed(clearFmt), "selection.format.danger"),
    ];
    const PARAGRAPH = [
      R("Bullet list", go(() => edList("bullet")), "paragraph.list"),
      R("Numbered list", go(() => edList("numbered")), "paragraph.list"),
      R("Task list", line(task), "paragraph.list"),
      "---",
      ...[1, 2, 3, 4, 5, 6].map(n => R("Heading " + n, line(heading(n)), "paragraph.heading")),
      R("Body", line(heading(0)), "paragraph.heading"),
      "---",
      R("Quote", line(quote), "paragraph.block"),
    ];
    const INSERT = [
      R("Footnote", ed(footnote), "insert.basic"),
      R("Table", ed(block(TABLE, 2)), "insert.basic"),
      R("Callout", ed(block("> [!note]\n> ", 12)), "insert.basic"),
      R("Horizontal rule", ed(block("---", 3)), "insert.basic"),
      "---",
      R("Code block", ed(fence("```")), "insert.advanced"),
      R("Math block", ed(fence("$$")), "insert.advanced"),
      R("New base", null, "insert.advanced", NO_BASE),
    ];
    const none = sel ? undefined : "";      // disabled with no hover reason: stock greys them the same way
    const ADD = [
      R("Add link", ed(link("[[", "]]")), "selection-link"),
      R("Add external link", ed(extLink), "selection-link"),
      "---",
    ];
    const PAR = { title: "Paragraph", sub: PARAGRAPH, section: "" }, INS = { title: "Insert", sub: INSERT, section: "" };
    const CLIP = [
      R("Cut", go(() => clipCut(g)), "clipboard", none),
      R("Copy", go(() => clipCopy(g)), "clipboard", none),
      R("Paste", go(() => clipPaste(g)), "clipboard", clip ? undefined : ""),
      R("Paste as plain text", go(() => clipPaste(g)), "clipboard", clip ? undefined : ""),
      R("Select all", go(() => Ed.selectAll(g)), "clipboard"),
    ];
    /* C3-4: a right-click ON a link / embed (hit = linkHit(), the source span under
       the pointer) gets stock's target menus (RECON §A1 lp-link-internal / -external / lp-image) */
    if (hit && hit.kind === "image") return imageRows(g, hit, go);
    if (hit) {
      const editLink = R("Edit link", go(() => { const cm = Ed.cm(g); if (cm) cm.dispatch({ selection: { anchor: hit.ea, head: hit.eb }, scrollIntoView: true }); }), "selection-link");
      if (hit.kind === "ext") return [
        R("Open link in default browser", go(() => openUrl(hit.url)), "open"), "---",
        ...ADD, editLink, PAR, INS, "---", ...CLIP, "---",
        R("Copy URL", go(() => clipPut(hit.url)), "clipboard"),
      ];
      return [...noteOpenRows(g, hit, go), "---", ...ADD, editLink, PAR, INS, "---", ...CLIP, "---",
        copyPath(hit.path), "---", ...noteFileRows(hit, go), "---", "---", ...noteSysRows(hit, go)];
    }
    const out = [...ADD];
    if (sel) out.push(
      R("Search for “" + sel + "”", go(() => searchFor(sel)), "selection"),
      R("Extract current selection...", null, "selection", NO_EXTRACT));
    out.push({ title: "Format", sub: FORMAT, section: "selection.format" }, PAR, INS, "---", ...CLIP);
    return out;
  }

  // ---- C3-4: link / embed targets ----
  const NO_RENAME = "Missing subsystem: renaming a note from a link — Rename... (cmdRename) renames the active tab's note; open the link first";
  const NO_MOVE = "Missing subsystem: moving a note from a link — the folder list (moveNoteTo) is wired on the tab menu only";
  const NO_FILEOPEN = "Missing subsystem: a file-path opener — open_external takes a URL, not a vault path";
  const NO_SYSEXP = "Missing subsystem: a file-manager handler — no command reveals a path in a file manager";
  const NO_IMGTAB = "Missing subsystem: an image view — a tab here always holds a note, a graph or a canvas";
  const NO_IMGCLIP = "Missing subsystem: an image clipboard write — clip_set_text (src-tauri/src/clip.rs) carries text only";
  const NO_SWAP = "Missing subsystem: an attachment picker (a modal file suggester for the embed's new target)";
  const NO_IMGFILE = "Missing subsystem: attachment file operations — rename/move/delete here work on notes (.md) only";
  const WIN = () => (typeof DIS_WIN === "string" ? DIS_WIN : "Missing subsystem: a second OS window");
  const R = (title, fn, section, why) => ({ title, fn, section, why });

  /* the note name a link target resolves to (notesCache = vault names without .md, main.js refreshTree) */
  function noteName(n) {
    const b = String(n || "").replace(/\.md$/i, "");
    const L = typeof notesCache !== "undefined" ? notesCache : [];
    return L.find(x => x === b) || L.find(x => x.endsWith("/" + b)) || b;
  }
  function imgName(n) {
    const L = typeof imgsCache !== "undefined" && imgsCache ? imgsCache : [];
    return L.find(x => x === n) || L.find(x => x.endsWith("/" + n)) || n;
  }
  /* the stock Copy path submenu: as Obsidian URL | from vault folder | from system root */
  function copyPath(path) {
    const root = String(vaultPath || "").replace(/[\\/]+$/, ""), vn = root.split(/[\\/]/).pop();
    const url = "obsidian://open?vault=" + encodeURIComponent(vn) + "&file=" + encodeURIComponent(path.replace(/\.md$/i, ""));
    const put = t => () => { closeMenu(); clipPut(t); };
    return { title: "Copy path", section: "info", sub: [
      R("as Obsidian URL", put(url), "info"),
      R("from vault folder", put(path), "info"),
      R("from system root", put(root + "/" + path), "info"),
    ] };
  }
  async function openNote(g, n, an, how) {     // the ctrl+click path of wikiClick, or a split to the right
    if (await cvLinkOpen(n, an)) return;
    if (!Ed.resolves(n)) await createNote(n);  // stock: opening an unresolved link creates the note
    await flushSave(g);
    const t = mkTab(n);
    if (how === "right") { await splitWith(g, "row", t); return; }
    g.tabs.push(t);
    g.active = g.tabs.length - 1;
    await loadActive(g);
    if (an) await navAnchor(g, an);
  }
  const noteOpenRows = (g, h, go) => [
    R("Open in new tab", go(() => openNote(g, h.note, h.anchor, "tab")), "open"),
    R("Open to the right", go(() => openNote(g, h.note, h.anchor, "right")), "open"),
    R("Open in new window", null, "open", WIN()),
  ];
  const noteFileRows = (h, go) => [
    R("Rename...", null, "action", NO_RENAME),
    R("Move file to...", null, "action", NO_MOVE),
    R(bmCache.includes(h.name) ? "Remove bookmark" : "Bookmark...", go(() => toggleBm(h.name)), "action"),
  ];
  const noteSysRows = (h, go) => [
    R("Open in default app", null, "system", NO_FILEOPEN),
    R("Show in system explorer", null, "system", NO_SYSEXP),
    R("Reveal file in navigation", go(() => bmReveal(h.name)), "system"),
  ];
  function imageRows(g, h, go) {
    const cut = ed => go(() => {                // the embed's source span, replaced as ONE edit (edEdit)
      const cm = Ed.cm(g); if (!cm) return;
      cm.dispatch({ selection: { anchor: h.from, head: h.to } });
      edEdit((v, a, b) => { const s = ed(v.slice(a, b)); return [v.slice(0, a) + s + v.slice(b), a, a + s.length]; });
    });
    const sized = /\|\s*\d+(?:x\d+)?\s*\]\]$/.test(h.text);
    return [
      R("Copy image", null, "clipboard", NO_IMGCLIP),
      R("Swap file...", null, "action", NO_SWAP),
      R("Remove image", cut(() => ""), "action"),
      R("Reset size", sized ? cut(s => s.replace(/\|\s*\d+(?:x\d+)?\s*\]\]$/, "]]")) : null, "action", sized ? undefined : ""),
      "---",
      R("Open in new tab", null, "open", NO_IMGTAB),
      R("Open to the right", null, "open", NO_IMGTAB),
      R("Open in new window", null, "open", WIN()),
      "---",
      R("Rename...", null, "action", NO_IMGFILE),
      R("Move file to...", null, "action", NO_IMGFILE),
      R("Bookmark...", null, "action", "Missing subsystem: bookmarks of attachments — a bookmark is a note or PDF name (R9.4)"),
      "---",
      copyPath(h.path),
      "---",
      R("Open in default app", null, "system", NO_FILEOPEN),
      R("Show in system explorer", null, "system", NO_SYSEXP),
      R("Reveal file in navigation", go(() => bmReveal(h.path)), "system"),
      "---",
      { title: "Delete image", section: "danger", why: NO_IMGFILE, warn: true },
    ];
  }
  /* classify one source span: [[x]] / ![[x]] / [t](u) / ![a](u) / a bare url */
  function classify(text) {
    let m;
    if ((m = /^(!?)\[\[([^\]|#]*)(?:#([^\]|]*))?(?:\|[^\]]*)?\]\]$/.exec(text))) {
      const t = m[2].trim(), inner = 2 + m[1].length;
      const ea = inner, eb = inner + m[2].length;
      if (m[1] && Ed.isImg(t)) return { kind: "image", target: t, ea, eb };
      return { kind: "note", target: t, anchor: m[3] || "", ea, eb };
    }
    if ((m = /^(!?)\[([^\]]*)\]\(([^)]*)\)$/.exec(text))) {
      const u = m[3].trim(), ea = m[1].length + m[2].length + 3, eb = ea + m[3].length;
      if (/^https?:\/\//i.test(u)) return m[1] ? { kind: "imgremote" } : { kind: "ext", url: u, ea, eb };
      if (/^[a-z][a-z0-9+.-]*:/i.test(u)) return null;
      let p = u; try { p = decodeURIComponent(u); } catch (_) { /* keep raw */ }
      const h = p.indexOf("#"), f = h >= 0 ? p.slice(0, h) : p;
      if (m[1]) return Ed.isImg(f) ? { kind: "image", target: f, ea, eb } : null;
      return { kind: "note", target: f, anchor: h >= 0 ? p.slice(h + 1) : "", ea, eb };
    }
    if (/^https?:\/\//i.test(text)) return { kind: "ext", url: text, ea: 0, eb: text.length };
    return null;
  }
  const SPAN_RE = /!?\[\[[^\]\n]*\]\]|!?\[[^\]\n]*\]\([^)\n]*\)|https?:\/\/[^\s<>()]+/g;
  /* the link / embed under a right-click, BEFORE the caret moves (a moved caret re-renders the LP row
     raw and the element is gone). Column = the text before the element in its .cm-line (an LP row's
     textContent === its source line); Source mode has no link elements, so it falls back to the
     position under the pointer. */
  function linkHit(cm, e) {
    const el = e.target.closest && e.target.closest("a.wl, a.lt, a.ext, a.url, img.vimg, span.imgmiss");
    let line, col;
    const row = el && Ed.rowOf(el);
    if (row) {
      try { line = cm.state.doc.lineAt(cm.posAtDOM(row, 0)); } catch (_) { return null; }
      col = 0;
      const w = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
      for (let n; (n = w.nextNode());) {
        if (el.contains(n) || !(n.compareDocumentPosition(el) & Node.DOCUMENT_POSITION_FOLLOWING)) break;
        col += n.nodeValue.length;
      }
      if (el.tagName === "A") col += 1;      // inside the label, past any opening marker
    } else {
      const p = cm.posAtCoords({ x: e.clientX, y: e.clientY });
      if (p == null) return null;
      line = cm.state.doc.lineAt(p); col = p - line.from;
    }
    for (const m of line.text.matchAll(SPAN_RE)) {
      if (col < m.index || col > m.index + m[0].length) continue;
      const c = classify(m[0]);
      if (!c || c.kind === "imgremote") return null;
      const from = line.from + m.index, h = Object.assign(c, { from, to: from + m[0].length, text: m[0], ea: from + c.ea, eb: from + c.eb });
      if (h.kind === "note") { h.note = h.target || curOf(fg()); h.name = noteName(h.note); h.path = h.name + ".md"; }
      if (h.kind === "image") h.path = imgName(h.target);
      return h;
    }
    return null;
  }
  async function clipPut(t) { await inv("clip_set_text", { text: t }).catch(err => console.warn("clip_set_text:", err)); }
  function openUrl(u) {          // the S1 choke point: open_external re-checks the scheme in Rust
    inv("open_external", { url: u }).catch(err => console.warn("open_external:", err));
  }

  /* reading view (RECON §A1 rv-*): menus on links only; a selection gets one app row, "Copy"
     (Q4: an app .menu, not WebKit's native one). Anywhere else: no menu, as in stock. */
  function openReading(e) {
    const P = e.target.closest(".preview");
    if (!P) return false;
    const g = (typeof gOf === "function" && gOf(P)) || fg(), x = e.clientX, y = e.clientY;
    const go = fn => () => { closeMenu(); fn(); };
    const a = e.target.closest("a.wiki, a.internal-link, a.ext");
    const s = getSelection(), st = s && !s.isCollapsed && P.contains(s.anchorNode) ? s.toString() : "";
    let rows = null, mt = "";
    if (a && a.classList.contains("ext")) {
      const u = a.getAttribute("href");
      mt = "read:ext";
      rows = [R("Open link in default browser", go(() => openUrl(u)), "open"), "---",
        R("Copy", go(() => clipPut(st || a.textContent)), "clipboard"), "---",
        R("Copy URL", go(() => clipPut(u)), "clipboard")];
    } else if (a) {
      const n = wikiTarget(a) || curOf(g), h = { note: n, anchor: wikiAnchor(a), name: noteName(n) };
      h.path = h.name + ".md";
      mt = "read:link";
      rows = [...noteOpenRows(g, h, go), "---", ...noteFileRows(h, go), "---",
        R("Copy", go(() => clipPut(st || a.textContent)), "clipboard"), "---",
        copyPath(h.path), "---", ...noteSysRows(h, go)];
    } else if (st) {
      mt = "read:sel";
      rows = [R("Copy", go(() => clipPut(st)), "clipboard")];
    }
    if (!rows) return true;                  // reading view, no target: no menu (stock)
    closeMenu();
    const B = OMenu.shell();
    B.el.dataset.mt = mt;
    B.add(rows);
    placeMenu(B.el, x, y);
    return true;
  }
  function searchFor(s) {
    if (!sideOpen) cmdToggleSide();
    setPane("search");
    const q = $("sinput");
    q.value = s;
    q.dispatchEvent(new Event("input", { bubbles: true }));
  }
  const selText = g => { const s = Ed.sel(g); return s && !s.empty ? Ed.slice(g, s) : ""; };
  async function clipCopy(g) { const t = selText(g); if (t) await inv("clip_set_text", { text: t }).catch(err => console.warn("clip_set_text:", err)); }
  async function clipCut(g) {
    const s = Ed.sel(g), t = selText(g);
    if (!t) return;
    try { await inv("clip_set_text", { text: t }); } catch (err) { console.warn("clip_set_text:", err); return; }   // never delete what did not reach the clipboard
    if (!edEditable()) return;
    Ed.replace(g, s, "", "cut");
  }
  async function clipPaste(g) {
    let t = null;
    try { t = await inv("clip_text"); } catch (err) { console.warn("clip_text:", err); }
    if (!t || !edEditable()) return;
    Ed.replace(g, Ed.sel(g), t, "paste");
  }

  /* the contextmenu event inside a .cm-editor. A right-click OUTSIDE the
     selection moves the caret there first (stock); inside it keeps it, so the
     selection rows act on it. Returns true when the menu is ours. */
  /* C3-7 (T3): the LP table widget's menu (RECON §A1 lp-table-cell / lp-table-header):
       Row > | Column > | --- | Sort by column (A to Z) | Sort by column (Z to A)
     No generic editor rows, no plugin rows, and the caret does NOT move (the
     widget stays rendered). Every op is a text transform on the table's pipe
     source in Rust (src-tauri/src/mdtable.rs table_op, unit-tested against
     stock's outputs), re-padded like stock, applied as ONE undo step. */
  const tidy = L => L.filter((x, i, a) => !(x === "---" && (i === 0 || a[i - 1] === "---" || i === a.length - 1)));
  async function tableOp(cm, from, src, op, row, col) {
    if (!edEditable()) return;
    let out;
    try { out = await inv("table_op", { src, op, row, col }); } catch (err) { console.warn("table_op:", err); return; }
    const to = from + src.length;
    if (cm.state.sliceDoc(from, to) !== src || out === src) return;      // edited while Rust worked: drop it
    cm.dispatch({ changes: { from, to, insert: out },
                  annotations: [CMc.isolateHistory.of("full"), CMs.Transaction.userEvent.of("os.table")] });
  }
  function openTable(e, g, cm, cell) {
    const w = cell.closest(".cm-table-widget");
    let p0;
    try { p0 = cm.posAtDOM(w, 0); } catch (_) { return; }
    const b = Ed.tblBlocks(cm.state.doc).find(x => x.from === p0);
    if (!b) return;
    if (fg() !== g) focusGroup(g);
    const src = cm.state.sliceDoc(b.from, b.to), r = +cell.dataset.r || 0, c = +cell.dataset.c || 0;
    const nr = b.rows.length, nc = b.cols;
    const R = (title, o, section, why) => ({ title, section, why, fn: () => { closeMenu(); tableOp(cm, b.from, src, o, r, c); } });
    const rowSub = tidy([R("Add row above", "row-above", "table.row.add"), R("Add row below", "row-below", "table.row.add"), "---",
      ...(r > 0 ? [R("Move row up", "row-up", "table.row.move")] : []),
      ...(r < nr - 1 ? [R("Move row down", "row-down", "table.row.move")] : []), "---",
      R("Duplicate row", "row-dup", "table.row.edit"),
      R("Delete row", "row-del", "table.row.edit", nr > 1 ? undefined : "The table has no other row")]);
    const colSub = tidy([R("Add column to the left", "col-left", "table.col.add"), R("Add column to the right", "col-right", "table.col.add"), "---",
      ...(c > 0 ? [R("Move column left", "col-move-left", "table.col.move")] : []),
      ...(c < nc - 1 ? [R("Move column right", "col-move-right", "table.col.move")] : []), "---",
      R("Align left", "align-left", "table.col.align"), R("Align center", "align-center", "table.col.align"), R("Align right", "align-right", "table.col.align"), "---",
      R("Duplicate column", "col-dup", "table.col.edit"),
      R("Delete column", "col-del", "table.col.edit", nc > 1 ? undefined : "The table has no other column")]);
    const x = e.clientX, y = e.clientY;
    closeMenu();
    const B = OMenu.shell();
    B.el.dataset.mt = "editor:lp-table-" + (r === 0 ? "header" : "cell");
    B.add([{ title: "Row", section: "table", sub: rowSub }, { title: "Column", section: "table", sub: colSub }, "---",
      R("Sort by column (A to Z)", "sort-asc", "table-sort", nr > 2 ? undefined : "Nothing to sort"),
      R("Sort by column (Z to A)", "sort-desc", "table-sort", nr > 2 ? undefined : "Nothing to sort")]);
    placeMenu(B.el, x, y);
  }

  function open(e) {
    const g = groupAt(e.target), cm = g && Ed.cm(g);
    const t = g && g.active >= 0 ? g.tabs[g.active] : null;
    if (!cm || !t || t.kind || t.mode === "reading") return false;
    const cell = e.target.closest(".cm-table-widget th, .cm-table-widget td");
    if (cell && cm.dom.contains(cell)) { openTable(e, g, cm, cell); return true; }   // C3-7: before the caret moves into the table
    const p = cm.posAtCoords({ x: e.clientX, y: e.clientY }), r = cm.state.selection.main;
    const inSel = !r.empty && p != null && p >= r.from && p <= r.to;
    const hit = inSel ? null : linkHit(cm, e);            // C3-4: read the target BEFORE the caret re-renders the row
    if (p != null && !inSel) cm.dispatch({ selection: { anchor: p } });
    if (fg() !== g) focusGroup(g);
    // a right-click on a link/embed never reaches CM's mousedown (C3-1), so nothing focused the
    // editor: focus it like a native right-click would, or keys after Escape go nowhere
    if (!cm.hasFocus) cm.focus();
    const sel = selText(g), x = e.clientX, y = e.clientY;
    closeMenu();
    const ask = inv("clip_has_text").catch(() => false);
    ask.then(clip => {
      if (fg() !== g || selText(g) !== sel) return;       // the world moved while we asked: no stale menu
      const B = OMenu.shell();
      B.el.dataset.mt = "editor:" + (t.mode === "source" ? "src" : "lp") + (hit ? "-" + (hit.kind === "note" ? "link" : hit.kind) : sel ? "-sel" : "");
      B.add(rows(g, sel, !!clip, hit));
      if (PMENU && PMENU.editorMenu) PMENU.editorMenu(e, B);   // plugin rows after the core rows (stock order)
      closeMenu();
      placeMenu(B.el, x, y);
    });
    return true;
  }
  return { open, openReading, rows, linkHit, classify, ops: { heading, task, quote, clearFmt, extLink, block, fence, footnote, TABLE } };
})();
