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
