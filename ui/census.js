// SPDX-License-Identifier: GPL-3.0-or-later
// CENSUS TOKEN REGISTRY (goal gatetrain, G2). Loaded BEFORE main.js.
// Every rebase used to conflict on the ONE long census expression in
// updateTitle(): two branches each appending a token to the same line.
// A NEW token registers here instead — ONE self-contained line per token:
//   CENSUS.push(() => myTok());          // <feature>: what it proves
// The function is looked up when updateTitle() runs (never at load), so it may
// name anything main.js defines. Return "" for "absent", else " [name:...]".
// .gitattributes sets `merge=union` on THIS file: two branches that each add a
// line merge/rebase to BOTH lines, never a conflict. The price of union: edit
// or delete a line on two branches at once and both versions survive — so
// lines here are append-only; change a token by changing the function it calls.
// Tokens are appended AFTER every inline token (end of the title), in this
// file's line order; a token that throws is published as [cerr:<index>].
var CENSUS = [];
CENSUS.push(() => { const L = CMDS.filter(c => c.id === "editor:toggle-bullet-list" || c.id === "editor:toggle-numbered-list"); return " [lt:" + L.length + "/" + L.filter(c => !c.when || c.when()).length + "]"; });   // listtoggle: both list commands registered / offered by the palette (editing view only)
CENSUS.push(() => sgeoTok());   // setcenter R3: settings modal geometry [sbox:] [snav:] [srow1:] [shkf:] [shkc:] [shk1/2:] (only while settings is open)
CENSUS.push(() => Ed.rvTaskTok());   // rvtask R5: reading-view task boxes [rvtask:n/checked] + painted centres per source line [rvtaskxy:]
CENSUS.push(() => foldTok());   // collapseall R7: [fold:fe<c>/<n>:<C|E>,bm<c>/<n>:<C|E>] fold state of both trees, read off the DOM
CENSUS.push(() => caSync());   // collapseall R3/R4: [fecab:x0-x1,y][bmcab:x0-x1,y] painted rects of the two header toggles
CENSUS.push(() => sidePane === "bm" ? " [bmvis:" + bmVisRows().length + "]" : "");   // collapseall: bookmark rows NOT folded away (what is seen)
CENSUS.push(() => { const q = s => document.querySelector(s), b = e => e ? getComputedStyle(e).backgroundColor.replace(/\s+/g, "") : "-", vis = e => !!e && e.getClientRects().length > 0, o = ["mbox=" + b(q("#mbox")), "lgpop=" + b(q("#main .lgpop:not([hidden])") || q("#main .lgpop")), "sbox=" + b(q("#sbox")), "ac=" + b(q("#main .ac:not([hidden])") || q("#main .ac")), "rnbox=" + b(q("#rnbox"))]; for (const [n, s] of [["pcard", "#pcard"], ["ulcard", "#ulcard"], ["delcard", "#delcard"], ["bmecard", "#bmecard"], ["findbar", ".findbar"]]) { const e = [...document.querySelectorAll(s)].find(vis); if (e) o.push(n + "=" + b(e)); } return " [ovl:" + o.join("|") + "]"; });   // overlaytheme R4: resolved background of every --bg-elevated floating surface (mbox=palette+switcher, lgpop=graph options, sbox=settings, ac, rnbox always; other cards when open) — proves Layer 3 repaints under a vault theme
