// SPDX-License-Identifier: GPL-3.0-or-later
//! Graph node drag (#279 "the graphs are sometimes erratic"): the pure drag steps in ui/graph-phys.js
//! (GraphPhys.dragStep, GraphPhys.releaseHeat) that ui/main.js startGraph drives from its mouse
//! handlers, run in JavaScriptCore with the real physics engine, the way tests/graphphys.rs does.
//! A 20-note graph (each note links two others, like the neutral repro vault) is settled to rest,
//! then the gestures are replayed exactly as main.js applies them (pin on the first real move,
//! alphaTarget 0.3 while held, releaseHeat at the end):
//!   1. QUICK drag (press, one pointer jump, release before any physics step ran): the node is at the
//!      release point when the hold ends, the sim re-flows (steps run, the node and its neighbours move)
//!      and goes quiet again within the cap;
//!   2. root cause, as a control: with the release state develop left (no alpha floor) the same quick
//!      drag runs no step at all and the sim goes quiet with the node frozen where it was dropped;
//!   3. negative: a click (no move past the 4 px slop) pins nothing, moves nothing, reheats nothing;
//!      a release that moved no node (a pan) does not raise alpha;
//!   4. negative: a long stepped drag (alpha already past the floor when released) keeps its alpha —
//!      releaseHeat only floors, it never lowers or replaces the heat a real drag built up;
//!   5. dragStep slop edges: 4 px is still a click, past it the first sample carries the whole offset,
//!      after that every sample is applied (also 1 px ones) and the last applied point follows.
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..") }

const CHECKS: &str = r#"(function () {
  "use strict";
  const G = window.GraphPhys, eng = G.create();
  const F = { repel: 10, repelK: 42.03, repelP: 0, dist: 250, center: 0.5187, centerK: 0.0771 }, LINK_K = 0.5;
  const fail = m => { throw new Error(m); };
  // 20 notes, note i links i+1 and i+7 (mod 20): a connected ring with chords
  const n = 20, E = []; for (let i = 0; i < n; i++) { E.push([i, (i + 1) % n]); E.push([i, (i + 7) % n]); }
  const adj = Array.from({ length: n }, () => new Set()); for (const [i, j] of E) { adj[i].add(j); adj[j].add(i); }
  const links = []; adj.forEach((s, i) => { for (const j of s) if (i < j) { const di = adj[i].size, dj = adj[j].size; links.push({ a: i, b: j, bias: di / (di + dj), k: LINK_K / Math.min(di, dj) }); } });
  const LNK = { LA: new Int32Array(links.map(l => l.a)), LB: new Int32Array(links.map(l => l.b)), LBI: new Float64Array(links.map(l => l.bias)), LK: new Float64Array(links.map(l => l.k)) };
  // main.js batch() + land(): a copy out (pins from fx/fy), the result written back
  function run(N, st, steps, first) {
    const b = { X: new Float64Array(n), Y: new Float64Array(n), VX: new Float64Array(n), VY: new Float64Array(n), PIN: new Uint8Array(n), FX: new Float64Array(n), FY: new Float64Array(n),
                links: LNK, f: F, st, steps, budget: 1e12, trace: false, first };
    N.forEach((p, i) => { b.X[i] = p.x; b.Y[i] = p.y; b.VX[i] = p.vx; b.VY[i] = p.vy; if (p.fx != null) { b.PIN[i] = 1; b.FX[i] = p.fx; b.FY[i] = p.fy; } });
    const r = eng.run(b);
    N.forEach((p, i) => { if (p.fx != null) { p.x = p.fx; p.y = p.fy; p.vx = 0; p.vy = 0; } else { p.x = b.X[i]; p.y = b.Y[i]; p.vx = b.VX[i]; p.vy = b.VY[i]; } });
    return r;
  }
  const toQuiet = (N, st, first, cap) => { let ran = 0, k = 0; while (!st.quiet && k++ < cap) ran += run(N, st, 60, first).ran; return ran; };
  // a graph settled to rest after its open (the first settle), at scale 1 (px == world units)
  const rested = () => {
    let r = 11; const rnd = () => (r = r * 16807 % 2147483647) / 2147483647;
    const N = []; for (let i = 0; i < n; i++) { const rr = 66 * Math.sqrt(i + rnd()), t = 2 * Math.PI * rnd(); N.push({ x: rr * Math.cos(t), y: rr * Math.sin(t), vx: 0, vy: 0, fx: null, fy: null }); }
    const st = { alpha: 1, alphaTarget: 0, seekN: 0, still: 0, calm: 0, kePeak: 0, quiet: false };
    toQuiet(N, st, true, 1000); if (!st.quiet) fail("the open never settled");
    return { N, st };
  };
  // main.js holdMove for a NODE press (view.scale 1): pin on the first real move, alphaTarget 0.3 (the ev "hold")
  const press = (p) => ({ x: p.x, y: p.y, node: p, moved: false });
  const move = (d, x, y) => {
    const m = G.dragStep(d, d.moved, x, y); if (!m) return false;
    d.moved = true; const p = d.node;
    if (p.fx == null) { p.fx = p.x; p.fy = p.y; }
    p.fx += m.dx; p.fy += m.dy; p.x = p.fx; p.y = p.fy; p.vx = 0; p.vy = 0;
    return true;
  };
  const hold = st => { st.alphaTarget = 0.3; st.calm = 0; st.kePeak = 0; st.seekN = 0; st.still = 0; st.quiet = false; };
  // main.js release: clear every pin, held = a pin was set
  const unpin = N => { let held = false; for (const q of N) if (q.fx != null) { q.fx = q.fy = null; held = true; } return held; };
  // develop's release body (before #279): target 0, seek rest, alpha untouched
  const oldRelease = st => { st.alphaTarget = 0; st.calm = 0; st.kePeak = 0; st.seekN = 0; st.still = 0; st.quiet = false; return st; };
  const snap = N => N.map(p => [p.x, p.y]);
  const maxMove = (N, S, skip) => N.reduce((m, p, i) => i === skip ? m : Math.max(m, Math.hypot(p.x - S[i][0], p.y - S[i][1])), 0);

  // 1. quick drag re-flows
  {
    const { N, st } = rested(), K = 5, p = N[K];
    const x0 = p.x, y0 = p.y, rx = x0 + 140, ry = y0 - 100;
    const d = press(p);
    if (!move(d, rx, ry)) fail("quick drag: a 172 px jump was taken for a click");
    hold(st);                                   // no physics step between the jump and the release
    const held = unpin(N); if (!held) fail("quick drag: no pin was set by the jump");
    G.releaseHeat(st, held);
    if (p.x !== rx || p.y !== ry) fail("quick drag: node at " + [p.x, p.y] + " when the hold ended, release point " + [rx, ry]);
    if (!(st.alpha >= G.ALPHA_REST)) fail("quick drag: alpha " + st.alpha + " after release, want >= " + G.ALPHA_REST);
    if (st.alphaTarget !== 0 || st.quiet !== false || st.seekN !== 0) fail("quick drag: release state " + JSON.stringify(st));
    const S = snap(N), ran = toQuiet(N, st, false, 2000);
    if (!st.quiet) fail("quick drag: the re-flow never settled (" + ran + " steps)");
    if (!(ran > 0)) fail("quick drag: no physics step ran after the release");
    const self = Math.hypot(p.x - rx, p.y - ry), others = maxMove(N, S, K);
    if (!(self > 5)) fail("quick drag: the dropped node moved only " + self + " after the release (frozen)");
    if (!(others > 1)) fail("quick drag: no other node moved (max " + others + "): the graph did not re-flow");
    if (ran > 30 * 60 + 2000) fail("quick drag: settle took " + ran + " steps, past the 30 s cap + ease-out");
  }
  // 2. control: the same quick drag with develop's release freezes (the root cause, pinned down)
  {
    const { N, st } = rested(), K = 5, p = N[K];
    if (!(st.alpha <= 0.001)) fail("control: the rested sim stopped at alpha " + st.alpha + " (> ALPHA_MIN), the frozen-release mechanism does not hold here");
    const rx = p.x + 140, ry = p.y - 100, d = press(p);
    move(d, rx, ry); hold(st); unpin(N); oldRelease(st);
    const S = snap(N), ran = toQuiet(N, st, false, 2000);
    if (ran !== 0) fail("control: develop's release ran " + ran + " steps, expected none");
    if (p.x !== rx || p.y !== ry || maxMove(N, S, -1) !== 0) fail("control: develop's release moved nodes");
  }
  // 3. a click and a pan: nothing pinned, nothing reheated
  {
    const { N, st } = rested(), p = N[3], a0 = st.alpha, x0 = p.x, y0 = p.y, d = press(p);
    if (move(d, x0 + 2, y0 + 3) || move(d, x0 - 4, y0)) fail("click: a move inside the 4 px slop started a drag");
    if (p.fx != null || p.x !== x0 || p.y !== y0) fail("click: the node was pinned or moved");
    const held = unpin(N); if (held) fail("click: a pin was reported");
    G.releaseHeat(st, held);
    if (st.alpha !== a0) fail("click: release changed alpha " + a0 + " -> " + st.alpha);
    const S = snap(N); toQuiet(N, st, false, 2000);
    if (maxMove(N, S, -1) !== 0) fail("click: nodes moved after a click");
    const st2 = { alpha: 0.0004, alphaTarget: 0.3, seekN: 0, still: 0, calm: 0, kePeak: 0, quiet: false };
    G.releaseHeat(st2, false);
    if (st2.alpha !== 0.0004 || st2.alphaTarget !== 0) fail("pan: release raised alpha / kept the target: " + JSON.stringify(st2));
  }
  // 4. a long stepped drag keeps the heat it built up
  {
    const { N, st } = rested(), p = N[8], d = press(p);
    hold(st);
    for (let k = 1; k <= 40; k++) { move(d, d.x + 4, d.y - 3); run(N, st, 1, false); }
    const a = st.alpha; if (!(a > G.ALPHA_REST)) fail("stepped drag: alpha only " + a + " after 40 held steps");
    G.releaseHeat(st, unpin(N));
    if (st.alpha !== a) fail("stepped drag: release changed alpha " + a + " -> " + st.alpha);
    const st3 = { alpha: 0.25, alphaTarget: 0.3, seekN: 0, still: 0, calm: 0, kePeak: 0, quiet: false };
    if (G.releaseHeat(st3, true).alpha !== 0.25) fail("releaseHeat lowered a 0.25 alpha");
    toQuiet(N, st, false, 2000); if (!st.quiet) fail("stepped drag: never settled after release");
  }
  // 5. dragStep slop edges
  {
    const d = { x: 100, y: 100 };
    if (G.dragStep(d, false, 104, 100) !== null) fail("slop: exactly 4 px started a drag");
    if (G.dragStep(d, false, 100, 96) !== null || d.x !== 100 || d.y !== 100) fail("slop: a click sample moved the last point");
    const m = G.dragStep(d, false, 103, 104);  // 5 px
    if (!m || m.dx !== 3 || m.dy !== 4 || d.x !== 103 || d.y !== 104) fail("slop: 5 px sample " + JSON.stringify(m) + " d " + JSON.stringify(d));
    const m2 = G.dragStep(d, true, 104, 104);
    if (!m2 || m2.dx !== 1 || m2.dy !== 0 || d.x !== 104) fail("moved: a 1 px sample was dropped");
    const m3 = G.dragStep(d, true, 104, 104);
    if (!m3 || m3.dx !== 0 || m3.dy !== 0) fail("moved: a same-point sample (the release point) is not a zero step");
    if (G.DRAG_SLOP2 !== 16) fail("slop constant changed");
  }
  return "ok";
})()"#;

#[test]
fn graph_drag() {
    let src = std::fs::read_to_string(root().join("ui/graph-phys.js")).unwrap();
    let ctx = javascriptcore::Context::new();
    ctx.evaluate("var window = {}; var performance = { timeOrigin: 0, now: function () { return Date.now(); } };");
    ctx.evaluate(&src);
    if let Some(e) = ctx.exception() { panic!("graph-phys.js did not load: {}", e.to_str()); }
    let out = ctx.evaluate(CHECKS).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("graph drag check failed: {}", e.to_str()); }
    assert_eq!(out, "ok");
}
