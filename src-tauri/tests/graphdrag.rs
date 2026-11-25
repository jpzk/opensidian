// SPDX-License-Identifier: GPL-3.0-or-later
//! Graph node drag (#279 "the graphs are sometimes erratic"): the pure drag steps in ui/graph-phys.js
//! (GraphPhys.dragStep, holdQueue/holdTake, releaseHeld, releaseHeat) that ui/main.js startGraph drives from its mouse
//! handlers and its animation frame, run in JavaScriptCore with the real physics engine, the way
//! tests/graphphys.rs does. A 20-note graph (each note links two others, like the neutral repro vault)
//! is settled to rest, then the gestures are replayed exactly as main.js applies them (a sample past the
//! slop is queued, the next frame applies it: pin on the first applied move, alphaTarget 0.3 while held;
//! release drops what no frame took, then releaseHeat):
//!   1. QUICK drag inside one frame (press, one pointer jump, release before the next frame), stock's
//!      behaviour (measured black-box on 1.13.7): no pin, no travel applied, the node and its label stay
//!      together at the press point, and the grab re-flows the graph (releaseHeld; develop drew the label
//!      at the drop point, the disc at the start, and never re-flowed);
//!   2. ONE-FRAME drag (the frame applied the jump, the release came before any physics step): the node
//!      is at the release point when the hold ends, the sim re-flows and goes quiet within the cap;
//!   3. root cause, as a control: with the release state develop left (no alpha floor) the same
//!      one-frame drag runs no step at all and the sim goes quiet with the node frozen where it was dropped;
//!   4. negative: a click (no move past the 4 px slop) queues, pins, moves and reheats nothing (releaseHeld
//!      false for a click, a pan and no press at all);
//!      a release that moved no node (a pan) does not raise alpha;
//!   5. negative: a long stepped drag (alpha already past the floor when released) keeps its alpha —
//!      releaseHeat only floors, it never lowers or replaces the heat a real drag built up;
//!   6. dragStep slop edges and the queue: 4 px is still a click, past it the first sample carries the
//!      whole offset, every later sample counts (also 1 px ones); samples between two frames are summed
//!      and taken once, an empty queue takes null.
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
  // main.js holdMove for a NODE press: a sample past the slop is queued for the next frame
  const press = (p) => ({ x: p.x, y: p.y, node: p, moved: false });
  const sample = (d, x, y) => {
    const m = G.dragStep(d, d.moved, x, y); if (!m) return false;
    d.moved = true; G.holdQueue(d, m.dx, m.dy);
    return true;
  };
  const hold = st => { st.alphaTarget = 0.3; st.calm = 0; st.kePeak = 0; st.seekN = 0; st.still = 0; st.quiet = false; };
  // main.js holdApply, the top of the next animation frame (view.scale 1): pin on the first applied
  // move, alphaTarget 0.3 (the ev "hold"); false when nothing was queued
  const frame = (d, st) => {
    const m = G.holdTake(d); if (!m) return false;
    const p = d.node;
    if (p.fx == null) { p.fx = p.x; p.fy = p.y; }
    p.fx += m.dx; p.fy += m.dy; p.x = p.fx; p.y = p.fy; p.vx = 0; p.vy = 0;
    if (st.alphaTarget !== 0.3 || st.quiet) hold(st);
    return true;
  };
  // a sample and the frame after it (a drag a frame kept up with)
  const move = (d, st, x, y) => sample(d, x, y) && frame(d, st);
  // main.js release: clear every pin, held = a pin was set
  const unpin = N => { let held = false; for (const q of N) if (q.fx != null) { q.fx = q.fy = null; held = true; } return held; };
  // develop's release body (before #279): target 0, seek rest, alpha untouched
  const oldRelease = st => { st.alphaTarget = 0; st.calm = 0; st.kePeak = 0; st.seekN = 0; st.still = 0; st.quiet = false; return st; };
  const snap = N => N.map(p => [p.x, p.y]);
  const maxMove = (N, S, skip) => N.reduce((m, p, i) => i === skip ? m : Math.max(m, Math.hypot(p.x - S[i][0], p.y - S[i][1])), 0);

  // 1. quick drag inside one frame, stock's behaviour: the node stays at the press point (no pin, no
  //    travel applied, its disc and label together), but the grab re-flows the graph
  {
    const { N, st } = rested(), K = 5, p = N[K], x0 = p.x, y0 = p.y;
    const rx = p.x + 140, ry = p.y - 100, d = press(p);
    if (!sample(d, rx, ry)) fail("quick drag: a 172 px jump was taken for a click");
    if (!(d.pdx === 140 && d.pdy === -100)) fail("quick drag: the jump was not queued: " + JSON.stringify(d));
    if (p.fx != null || p.x === rx) fail("quick drag: the sample moved the node before any frame");
    if (st.alphaTarget !== 0) fail("quick drag: alphaTarget " + st.alphaTarget + " without a frame");
    // release before the next frame: the queue goes with the hold, no pin, but the grab counts
    const pinned = unpin(N); if (pinned) fail("quick drag: a pin was set without a frame");
    const held = G.releaseHeld(d, d.moved, pinned);
    if (held !== true) fail("quick drag: a node grab past the slop is not a re-flowing release");
    if (p.x !== x0 || p.y !== y0) fail("quick drag: node at " + [p.x, p.y] + " when the hold ended, press point " + [x0, y0]);
    G.releaseHeat(st, held);
    if (!(st.alpha >= G.ALPHA_REST)) fail("quick drag: alpha " + st.alpha + " after release, want >= " + G.ALPHA_REST);
    const S = snap(N), ran = toQuiet(N, st, false, 2000);
    if (!(ran > 0)) fail("quick drag: no physics step ran after the release (frozen)");
    if (!st.quiet) fail("quick drag: the re-flow never settled (" + ran + " steps)");
    if (!(maxMove(N, S, -1) > 0)) fail("quick drag: no node moved: the graph did not re-flow");
    if (Math.hypot(p.x - rx, p.y - ry) < Math.hypot(x0 - rx, y0 - ry) / 2) fail("quick drag: the node re-flowed toward the release point it never reached");
  }
  // 2. one-frame drag re-flows
  {
    const { N, st } = rested(), K = 5, p = N[K];
    const rx = p.x + 140, ry = p.y - 100;
    const d = press(p);
    if (!move(d, st, rx, ry)) fail("one-frame drag: the frame applied nothing");
    if (st.alphaTarget !== 0.3) fail("one-frame drag: the frame did not hold (alphaTarget " + st.alphaTarget + ")");
    if (frame(d, st)) fail("one-frame drag: a second frame applied travel again");
    const held = unpin(N); if (!held) fail("one-frame drag: no pin was set by the frame");
    G.releaseHeat(st, held);                    // no physics step between the frame and the release
    if (p.x !== rx || p.y !== ry) fail("one-frame drag: node at " + [p.x, p.y] + " when the hold ended, release point " + [rx, ry]);
    if (!(st.alpha >= G.ALPHA_REST)) fail("one-frame drag: alpha " + st.alpha + " after release, want >= " + G.ALPHA_REST);
    if (st.alphaTarget !== 0 || st.quiet !== false || st.seekN !== 0) fail("one-frame drag: release state " + JSON.stringify(st));
    const S = snap(N), ran = toQuiet(N, st, false, 2000);
    if (!st.quiet) fail("one-frame drag: the re-flow never settled (" + ran + " steps)");
    if (!(ran > 0)) fail("one-frame drag: no physics step ran after the release");
    const self = Math.hypot(p.x - rx, p.y - ry), others = maxMove(N, S, K);
    if (!(self > 5)) fail("one-frame drag: the dropped node moved only " + self + " after the release (frozen)");
    if (!(others > 1)) fail("one-frame drag: no other node moved (max " + others + "): the graph did not re-flow");
    if (ran > 30 * 60 + 2000) fail("one-frame drag: settle took " + ran + " steps, past the 30 s cap + ease-out");
  }
  // 3. control: the same one-frame drag with develop's release freezes (the root cause, pinned down)
  {
    const { N, st } = rested(), K = 5, p = N[K];
    if (!(st.alpha <= 0.001)) fail("control: the rested sim stopped at alpha " + st.alpha + " (> ALPHA_MIN), the frozen-release mechanism does not hold here");
    const rx = p.x + 140, ry = p.y - 100, d = press(p);
    move(d, st, rx, ry); unpin(N); oldRelease(st);
    const S = snap(N), ran = toQuiet(N, st, false, 2000);
    if (ran !== 0) fail("control: develop's release ran " + ran + " steps, expected none");
    if (p.x !== rx || p.y !== ry || maxMove(N, S, -1) !== 0) fail("control: develop's release moved nodes");
  }
  // 4. a click and a pan: nothing queued, pinned or reheated
  {
    const { N, st } = rested(), p = N[3], a0 = st.alpha, x0 = p.x, y0 = p.y, d = press(p);
    if (move(d, st, x0 + 2, y0 + 3) || move(d, st, x0 - 4, y0)) fail("click: a move inside the 4 px slop started a drag");
    if (d.pdx || d.pdy || G.holdTake(d) !== null) fail("click: a slop sample was queued");
    if (p.fx != null || p.x !== x0 || p.y !== y0) fail("click: the node was pinned or moved");
    const held = unpin(N); if (held) fail("click: a pin was reported");
    if (G.releaseHeld(d, d.moved, held) !== false) fail("click: releaseHeld re-flows a click");
    if (G.releaseHeld({ x: 0, y: 0, node: null }, true, false) !== false) fail("pan: releaseHeld re-flows a pan");
    if (G.releaseHeld(null, true, false) !== false) fail("no press: releaseHeld re-flows a second release");
    if (G.releaseHeld(null, false, true) !== true) fail("pinned: releaseHeld ignores a pin");
    G.releaseHeat(st, held);
    if (st.alpha !== a0) fail("click: release changed alpha " + a0 + " -> " + st.alpha);
    const S = snap(N); toQuiet(N, st, false, 2000);
    if (maxMove(N, S, -1) !== 0) fail("click: nodes moved after a click");
    const st2 = { alpha: 0.0004, alphaTarget: 0.3, seekN: 0, still: 0, calm: 0, kePeak: 0, quiet: false };
    G.releaseHeat(st2, false);
    if (st2.alpha !== 0.0004 || st2.alphaTarget !== 0) fail("pan: release raised alpha / kept the target: " + JSON.stringify(st2));
  }
  // 5. a long stepped drag keeps the heat it built up
  {
    const { N, st } = rested(), p = N[8], d = press(p), x0 = p.x, y0 = p.y;
    for (let k = 1; k <= 40; k++) { if (!move(d, st, d.x + 4, d.y - 3)) fail("stepped drag: frame " + k + " applied nothing"); run(N, st, 1, false); }
    if (Math.abs(p.x - x0 - 160) > 1e-9 || Math.abs(p.y - y0 + 120) > 1e-9) fail("stepped drag: node at " + [p.x - x0, p.y - y0] + " from the press, want 160,-120");
    const a = st.alpha; if (!(a > G.ALPHA_REST)) fail("stepped drag: alpha only " + a + " after 40 held steps");
    G.releaseHeat(st, unpin(N));
    if (st.alpha !== a) fail("stepped drag: release changed alpha " + a + " -> " + st.alpha);
    const st3 = { alpha: 0.25, alphaTarget: 0.3, seekN: 0, still: 0, calm: 0, kePeak: 0, quiet: false };
    if (G.releaseHeat(st3, true).alpha !== 0.25) fail("releaseHeat lowered a 0.25 alpha");
    toQuiet(N, st, false, 2000); if (!st.quiet) fail("stepped drag: never settled after release");
  }
  // 6. dragStep slop edges, the queue
  {
    const d = { x: 100, y: 100 };
    if (G.dragStep(d, false, 104, 100) !== null) fail("slop: exactly 4 px started a drag");
    if (G.dragStep(d, false, 100, 96) !== null || d.x !== 100 || d.y !== 100) fail("slop: a click sample moved the last point");
    const m = G.dragStep(d, false, 103, 104);  // 5 px
    if (!m || m.dx !== 3 || m.dy !== 4 || d.x !== 103 || d.y !== 104) fail("slop: 5 px sample " + JSON.stringify(m) + " d " + JSON.stringify(d));
    const m2 = G.dragStep(d, true, 104, 104);
    if (!m2 || m2.dx !== 1 || m2.dy !== 0 || d.x !== 104) fail("moved: a 1 px sample was dropped");
    const m3 = G.dragStep(d, true, 104, 104);
    if (!m3 || m3.dx !== 0 || m3.dy !== 0) fail("moved: a same-point sample is not a zero step");
    if (G.DRAG_SLOP2 !== 16) fail("slop constant changed");
    const q = { x: 0, y: 0 };
    if (G.holdTake(q) !== null) fail("queue: an empty queue took travel");
    G.holdQueue(q, 3, 4); G.holdQueue(q, 10, -2); G.holdQueue(q, 0, 0);
    const t = G.holdTake(q);
    if (!t || t.dx !== 13 || t.dy !== 2) fail("queue: two samples between frames took " + JSON.stringify(t) + ", want 13,2");
    if (G.holdTake(q) !== null) fail("queue: the same travel was taken twice");
    G.holdQueue(q, 5, 0); G.holdQueue(q, -5, 0);
    if (G.holdTake(q) !== null) fail("queue: travel that cancels out moved the node");
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

// Review follow-up (CodeAnt, PR #292 ui/graph-phys.js:170): a physics batch is in flight (the worker
// copied the nodes out while the dragged node was not pinned yet), the next frame applies the held
// travel (holdPlace pins and moves it), the release clears the pin, and only then does the batch land.
// The node must stay where it was dropped: the batch's copy is from before the move. Replayed with
// main.js's own node half of a batch (GraphPhys.nodesOut / landNodes) and the real engine.
//   1. released while the batch was out: the dropped node keeps the drop point, at rest; nodes the
//      main thread did not touch take the batch's result;
//   2. still held when the batch lands: the node sits at its pin;
//   3. pinned when the batch went out, released before it landed: it stays where it was dropped;
//   4. the next batch (posted after the landing, no move since) re-flows the dropped node again.
const IN_FLIGHT: &str = r#"(function () {
  "use strict";
  const G = window.GraphPhys, eng = G.create();
  const F = { repel: 10, repelK: 42.03, repelP: 0, dist: 250, center: 0.5187, centerK: 0.0771 }, LINK_K = 0.5;
  const fail = m => { throw new Error(m); };
  const n = 20, E = []; for (let i = 0; i < n; i++) { E.push([i, (i + 1) % n]); E.push([i, (i + 7) % n]); }
  const adj = Array.from({ length: n }, () => new Set()); for (const [i, j] of E) { adj[i].add(j); adj[j].add(i); }
  const links = []; adj.forEach((s, i) => { for (const j of s) if (i < j) { const di = adj[i].size, dj = adj[j].size; links.push({ a: i, b: j, bias: di / (di + dj), k: LINK_K / Math.min(di, dj) }); } });
  const LNK = { LA: new Int32Array(links.map(l => l.a)), LB: new Int32Array(links.map(l => l.b)), LBI: new Float64Array(links.map(l => l.bias)), LK: new Float64Array(links.map(l => l.k)) };
  // main.js batch(): the node half copied out NOW; the worker runs it later (out)
  const post = (N, st) => ({ ...G.nodesOut(N), links: LNK, f: F, st: Object.assign({}, st), steps: 8, budget: 1e12, trace: false, first: false });
  const work = b => { eng.run(b); return b; };
  const land = (N, b) => G.landNodes(N, b, b.X, b.Y, b.VX, b.VY);
  const unpin = N => { let held = false; for (const q of N) if (q.fx != null) { q.fx = q.fy = null; held = true; } return held; };
  // a hot sim (alpha 0.3): every batch moves every free node
  const hot = () => {
    let r = 11; const rnd = () => (r = r * 16807 % 2147483647) / 2147483647;
    const N = []; for (let i = 0; i < n; i++) { const rr = 66 * Math.sqrt(i + rnd()), t = 2 * Math.PI * rnd(); N.push({ x: rr * Math.cos(t), y: rr * Math.sin(t), vx: 0, vy: 0, fx: null, fy: null }); }
    return { N, st: { alpha: 0.3, alphaTarget: 0.3, seekN: -1, still: 0, calm: 0, kePeak: 0, quiet: false } };
  };
  const frame = (d, p) => { const m = G.holdTake(d); if (!m) fail("frame: nothing queued"); G.holdPlace(p, m.dx, m.dy); };
  // 1. released while the batch was out
  {
    const { N, st } = hot(), K = 5, p = N[K], x0 = p.x, y0 = p.y, d = { x: p.x, y: p.y, node: p };
    if (!G.dragStep(d, false, p.x + 140, p.y - 100)) fail("in-flight: the jump was taken for a click");
    G.holdQueue(d, 140, -100);
    const b = post(N, st);                        // the batch goes out: the node is not pinned yet
    if (b.PIN[K] !== 0) fail("in-flight: the node was pinned before any frame");
    frame(d, p);                                  // the next frame applies the travel while the batch is out
    const rx = x0 + 140, ry = y0 - 100;
    if (p.x !== rx || p.y !== ry || p.fx !== rx) fail("in-flight: holdPlace put the node at " + [p.x, p.y] + ", want " + [rx, ry]);
    if (!unpin(N)) fail("in-flight: the frame set no pin");   // release before the batch lands
    work(b);
    if (b.X[K] === rx && b.Y[K] === ry) fail("in-flight: the batch result already holds the drop point, the race is not exercised");
    land(N, b);
    if (p.x !== rx || p.y !== ry) fail("in-flight: the batch landed the stale position " + [p.x, p.y] + " over the drop point " + [rx, ry]);
    if (p.vx !== 0 || p.vy !== 0) fail("in-flight: the dropped node landed with velocity " + [p.vx, p.vy]);
    for (let i = 0; i < n; i++) if (i !== K && (N[i].x !== b.X[i] || N[i].y !== b.Y[i] || N[i].vx !== b.VX[i])) fail("in-flight: free node " + i + " did not take the batch result");
    // 4. the next batch, posted after the landing with no move since, re-flows it
    const b2 = work(post(N, st)); land(N, b2);
    if (p.x !== b2.X[K] || p.y !== b2.Y[K]) fail("in-flight: the next batch did not re-flow the dropped node (stuck at " + [p.x, p.y] + ")");
    if (p.x === rx && p.y === ry) fail("in-flight: the dropped node never moved again in a hot sim");
  }
  // 2. still held when the batch lands: at the pin
  {
    const { N, st } = hot(), K = 3, p = N[K], d = { x: p.x, y: p.y, node: p };
    G.holdQueue(d, 60, 20); const b = post(N, st); frame(d, p);
    const fx = p.fx, fy = p.fy; work(b); land(N, b);
    if (p.x !== fx || p.y !== fy || p.vx !== 0) fail("held: the landing moved a held node off its pin " + [p.x, p.y] + " pin " + [fx, fy]);
  }
  // 3. pinned when the batch went out, released before it landed: stays where it was dropped
  {
    const { N, st } = hot(), K = 9, p = N[K], d = { x: p.x, y: p.y, node: p };
    G.holdQueue(d, -30, 45); frame(d, p);
    const b = post(N, st); if (b.PIN[K] !== 1) fail("pinned: the batch did not carry the pin");
    const dx = p.x, dy = p.y; unpin(N); work(b); land(N, b);
    if (p.x !== dx || p.y !== dy || p.vx !== 0 || p.vy !== 0) fail("pinned: node at " + [p.x, p.y, p.vx, p.vy] + ", dropped at " + [dx, dy]);
  }
  return "ok";
})()"#;

#[test]
fn graph_drag_release_in_flight() {
    let src = std::fs::read_to_string(root().join("ui/graph-phys.js")).unwrap();
    let ctx = javascriptcore::Context::new();
    ctx.evaluate("var window = {}; var performance = { timeOrigin: 0, now: function () { return Date.now(); } };");
    ctx.evaluate(&src);
    if let Some(e) = ctx.exception() { panic!("graph-phys.js did not load: {}", e.to_str()); }
    let out = ctx.evaluate(IN_FLIGHT).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("graph drag in-flight check failed: {}", e.to_str()); }
    assert_eq!(out, "ok");
}
