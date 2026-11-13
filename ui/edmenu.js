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
  function rows(g, sel, clip) {
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
    const out = [
      R("Add link", ed(link("[[", "]]")), "selection-link"),
      R("Add external link", ed(extLink), "selection-link"),
      "---",
    ];
    if (sel) out.push(
      R("Search for “" + sel + "”", go(() => searchFor(sel)), "selection"),
      R("Extract current selection...", null, "selection", NO_EXTRACT));
    out.push(
      { title: "Format", sub: FORMAT, section: "selection.format" },
      { title: "Paragraph", sub: PARAGRAPH, section: "" },
      { title: "Insert", sub: INSERT, section: "" },
      "---",
      R("Cut", go(() => clipCut(g)), "clipboard", none),
      R("Copy", go(() => clipCopy(g)), "clipboard", none),
      R("Paste", go(() => clipPaste(g)), "clipboard", clip ? undefined : ""),
      R("Paste as plain text", go(() => clipPaste(g)), "clipboard", clip ? undefined : ""),
      R("Select all", go(() => Ed.selectAll(g)), "clipboard"));
    return out;
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
  function open(e) {
    const g = groupAt(e.target), cm = g && Ed.cm(g);
    const t = g && g.active >= 0 ? g.tabs[g.active] : null;
    if (!cm || !t || t.kind || t.mode === "reading") return false;
    const p = cm.posAtCoords({ x: e.clientX, y: e.clientY }), r = cm.state.selection.main;
    if (p != null && (r.empty || p < r.from || p > r.to)) cm.dispatch({ selection: { anchor: p } });
    if (fg() !== g) focusGroup(g);
    const sel = selText(g), x = e.clientX, y = e.clientY;
    closeMenu();
    const ask = inv("clip_has_text").catch(() => false);
    ask.then(clip => {
      if (fg() !== g || selText(g) !== sel) return;       // the world moved while we asked: no stale menu
      const B = OMenu.shell();
      B.el.dataset.mt = "editor:" + (t.mode === "source" ? "src" : "lp") + (sel ? "-sel" : "");
      B.add(rows(g, sel, !!clip));
      if (PMENU && PMENU.editorMenu) PMENU.editorMenu(e, B);   // plugin rows after the core rows (stock order)
      closeMenu();
      placeMenu(B.el, x, y);
    });
    return true;
  }
  return { open, rows, ops: { heading, task, quote, clearFmt, extLink, block, fence, footnote, TABLE } };
})();
