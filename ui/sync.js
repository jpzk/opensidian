// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* Sync M6c — the sync window (ui/sync.html): sync settings pane + file history
   restore/diff panel (docs/sync/spec.md §6.6, §7). Runs ONLY in the "sync"
   webview; every sync_* / history_* command refuses any other webview
   (src-tauri/src/syncui.rs gate), so nothing here is reachable from plugins.
   Backup words: shown after the native question (sync_reveal), no copy
   button, not selectable, copy/cut/drag refused, hidden after 60 s and
   dropped from the DOM; the quiz checks 3 random positions in the backend. */
(function () {
  "use strict";
  const T = window.__TAURI__;
  const inv = (c, a) => T.core.invoke(c, a || {});
  const $ = id => document.getElementById(id);
  const q = new URLSearchParams(location.search);
  const REVEAL_S = 60;

  const err = (id, e) => { $(id).textContent = e ? String(e && e.message || e) : ""; };
  const el = (tag, cls, text) => { const n = document.createElement(tag); if (cls) n.className = cls; if (text != null) n.textContent = text; return n; };
  const when = ms => new Date(Number(ms)).toLocaleString();
  const size = b => b < 1024 ? b + " B" : b < 1048576 ? (b / 1024).toFixed(1) + " KB" : (b / 1048576).toFixed(1) + " MB";

  /* ---- tabs ---- */
  function show(pane) {
    for (const p of ["sync", "history"]) {
      $("pane-" + p).hidden = p !== pane;
      $("tab-" + p).setAttribute("aria-selected", String(p === pane));
    }
    if (pane === "sync") refresh();
  }
  for (const b of document.querySelectorAll("#tabs button")) b.onclick = () => show(b.dataset.pane);

  /* ---- sync pane ---- */
  const MODE_TEXT = { "auto-merge": "Automatically merge", "conflict-file": "Create conflict file" };
  async function refresh() {
    err("s-err");
    let v;
    try { v = await inv("sync_status"); } catch (e) { return err("s-err", e); }
    $("sync-off").hidden = v.configured;
    $("sync-on").hidden = !v.configured;
    if (!v.configured) return;
    const s = v.status, dl = $("s-status");
    dl.innerHTML = "";
    const rows = [
      ["Server", s.url],
      ["Connection", s.server_ok ? "connected" : "offline — " + (s.server_error || "unreachable")],
      ["This device", s.device_name + (s.device_id ? " (" + s.device_id.slice(0, 8) + ")" : "")],
      ["Devices", s.active_devices == null ? "—" : String(s.active_devices)],
      ["Key epoch", s.vk_epoch == null ? "—" : String(s.vk_epoch)],
      ["Files synced", String(s.files)],
      ["Last commit", s.head ? s.head.slice(0, 12) : "none yet"],
    ];
    for (const [k, val] of rows) { dl.appendChild(el("dt", null, k)); dl.appendChild(el("dd", null, val)); }
    $("s-mode").value = s.conflict_mode;
  }
  function joinMode() { return document.querySelector("input[name=s-join]:checked").value; }
  for (const r of document.querySelectorAll("input[name=s-join]")) r.onchange = () => {
    $("s-secret-l").firstChild.textContent = joinMode() === "create" ? "Enrollment token " : "Backup words (12) ";
  };
  $("s-setup").onclick = async () => {
    err("s-err");
    const b = $("s-setup"); b.disabled = true;
    try {
      const r = await inv("sync_setup", { url: $("s-url").value, device: $("s-device").value, join: joinMode(), secret: $("s-secret").value, caFile: $("s-ca").value || null });
      $("s-secret").value = "";
      if (r.words) showWords(r.words);
      await refresh();
    } catch (e) { err("s-err", e); } finally { b.disabled = false; }
  };
  $("s-now").onclick = async () => {
    err("s-err");
    const b = $("s-now"); b.disabled = true; $("s-result").textContent = "syncing…";
    try {
      const r = await inv("sync_now");
      const parts = [r.pulled + " pulled", r.pushed + " pushed", r.written + " written", r.removed + " removed"];
      if (r.losers) parts.push(r.losers + " kept in history");
      if (r.skipped) parts.push(r.skipped + " changed meanwhile (next pass)");
      if (r.replica_errors.length) parts.push("errors: " + r.replica_errors.join("; "));
      $("s-result").textContent = parts.join(", ");
      await refresh();
    } catch (e) { $("s-result").textContent = ""; err("s-err", e); } finally { b.disabled = false; }
  };
  $("s-mode").onchange = async () => {
    err("s-err");
    try { await inv("sync_set_mode", { mode: $("s-mode").value }); } catch (e) { err("s-err", e); refresh(); }
  };

  /* ---- backup words: no copy, auto-hide, quiz ---- */
  let hideTimer = null, quizPos = [];
  const box = $("words-box");
  for (const ev of ["copy", "cut", "dragstart", "contextmenu", "selectstart"]) box.addEventListener(ev, e => e.preventDefault());
  function hideWords() {
    clearInterval(hideTimer); hideTimer = null;
    $("words").innerHTML = "";          // the words leave the DOM, not just the screen
    $("words").hidden = true;
    $("words-left").parentNode.hidden = true;
  }
  function showWords(text) {
    const ws = String(text).split(/\s+/).filter(Boolean);
    text = null;
    const ol = $("words"); ol.innerHTML = ""; ol.hidden = false;
    for (const w of ws) ol.appendChild(el("li", null, w));
    box.hidden = false;
    $("words-left").parentNode.hidden = false;
    let left = REVEAL_S; $("words-left").textContent = String(left);
    clearInterval(hideTimer);
    hideTimer = setInterval(() => { left -= 1; $("words-left").textContent = String(left); if (left <= 0) hideWords(); }, 1000);
    // 3 distinct random positions of ws.length
    const pool = ws.map((_, i) => i + 1), r = new Uint32Array(3);
    crypto.getRandomValues(r);
    quizPos = [0, 1, 2].map(k => pool.splice(r[k] % pool.length, 1)[0]).sort((a, b) => a - b);
    const f = $("quiz-fields"); f.innerHTML = "";
    for (const p of quizPos) {
      const l = el("label", "row", "Word #" + p + " ");
      const i = el("input"); i.type = "text"; i.autocomplete = "off"; i.spellcheck = false; i.dataset.pos = String(p);
      l.appendChild(i); f.appendChild(l);
    }
    $("quiz-result").textContent = "";
  }
  $("words-hide").onclick = hideWords;
  $("s-reveal").onclick = async () => {
    err("s-err");
    try { showWords(await inv("sync_reveal")); } catch (e) { err("s-err", e); }
  };
  $("quiz-check").onclick = async () => {
    const picks = [...$("quiz-fields").querySelectorAll("input")].map(i => [Number(i.dataset.pos), i.value]);
    try {
      const ok = await inv("sync_quiz", { picks });
      $("quiz-result").textContent = ok ? "✓ correct — the words are written down right" : "✗ not matching — reveal them again and re-check";
    } catch (e) { $("quiz-result").textContent = String(e); }
  };

  /* ---- file history pane ---- */
  let sel = null;
  const REASON = { "own-save": "saved", "pre-write": "before overwrite", external: "external edit", delete: "deleted", rename: "renamed", idle: "snapshot", restore: "restored", "sync-loser": "kept by sync" };
  function rowFor(e, onpick) {
    const li = el("li", "item");
    li.tabIndex = 0;
    li.appendChild(el("span", "when", when(e.ts_ms)));
    li.appendChild(el("span", "why", REASON[e.reason] || e.reason));
    li.appendChild(el("span", "size", e.sha ? size(e.size) : "—"));
    if (e.from) li.appendChild(el("span", "hint", "from " + e.from));
    li.onclick = () => onpick(li);
    li.onkeydown = ev => { if (ev.key === "Enter" || ev.key === " ") { ev.preventDefault(); onpick(li); } };
    return li;
  }
  // every view change bumps viewGen; an awaited reply for an older view is
  // dropped, so a slow history_list/diff never paints over a newer one
  let viewGen = 0;
  function clearView(head) {
    viewGen++;
    sel = null; $("h-diff").innerHTML = ""; $("h-restore").disabled = true;
    $("h-restore").onclick = null; delete $("h-restore").dataset.armed; $("h-restore").textContent = "Restore this version";
    $("h-head").textContent = head || "Select a version to compare it with the note on disk.";
  }
  async function listVersions() {
    err("h-err"); clearView(); const g = viewGen;
    const name = $("h-note").value.trim().replace(/\.md$/, "");
    const ul = $("h-list"); ul.innerHTML = "";
    if (!name) return;
    let vs;
    try { vs = await inv("history_list", { name }); } catch (e) { return g === viewGen && err("h-err", e); }
    if (g !== viewGen) return;
    if (!vs.length) { ul.appendChild(el("li", "hint", "no versions recorded for " + name)); return; }
    for (const e of vs.slice().reverse()) {   // newest first
      ul.appendChild(rowFor(e, async li => {
        for (const x of ul.children) x.classList.toggle("sel", x === li);
        await pick(e, name);
      }));
    }
  }
  async function pick(e, name) {
    err("h-err"); clearView(when(e.ts_ms) + " · " + (REASON[e.reason] || e.reason) + " — compared with the note on disk");
    const g = viewGen;
    sel = e;
    if (!e.sha) { $("h-head").textContent += " (no bytes stored for this record)"; return; }
    let runs;
    try { runs = await inv("history_diff", { seq: e.seq }); } catch (er) { return g === viewGen && err("h-err", er); }
    if (g !== viewGen) return;
    const pre = $("h-diff");
    for (const r of runs) pre.appendChild(el("span", r.op, r.text));
    const b = $("h-restore");
    b.disabled = e.reason === "delete" && !e.sha;
    b.textContent = "Restore this version";
    b.onclick = async () => {
      // two-step in-page confirm (no window.confirm in the webview)
      if (b.dataset.armed !== String(e.seq)) {
        b.dataset.armed = String(e.seq);
        b.textContent = "Click again to restore — the current text is kept in history first";
        return;
      }
      delete b.dataset.armed; b.textContent = "Restore this version";
      try { await inv("history_restore", { seq: e.seq }); await listVersions(); $("h-head").textContent = "restored — the current text is the version you picked"; } catch (er) { err("h-err", er); }
    };
  }
  async function listDeleted() {
    err("h-err"); clearView("Recently deleted notes — restore brings back the newest stored version."); const g = viewGen;
    const ul = $("h-list"); ul.innerHTML = "";
    let ds;
    try { ds = await inv("history_deleted"); } catch (e) { return g === viewGen && err("h-err", e); }
    if (g !== viewGen) return;
    if (!ds.length) { ul.appendChild(el("li", "hint", "nothing deleted")); return; }
    for (const e of ds) {
      const name = e.path.replace(/\.md$/, "");
      const li = rowFor(e, () => {});
      li.insertBefore(el("span", "path", name), li.firstChild);
      const b = el("button", null, "Restore");
      b.onclick = async ev => {
        ev.stopPropagation();
        try { await inv("history_restore_deleted", { name }); await listDeleted(); } catch (er) { err("h-err", er); }
      };
      li.appendChild(b); ul.appendChild(li);
    }
  }
  $("h-show").onclick = listVersions;
  $("h-note").onkeydown = e => { if (e.key === "Enter") listVersions(); };
  $("h-deleted").onclick = listDeleted;

  /* ---- boot ---- */
  if (q.get("note")) $("h-note").value = q.get("note");
  const start = q.get("pane") === "history" ? "history" : "sync";
  show(start);
  if (start === "history" && q.get("note")) listVersions();
})();
