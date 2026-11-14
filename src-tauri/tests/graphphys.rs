// SPDX-License-Identifier: GPL-3.0-or-later
//! Graph physics (ui/graph-phys.js), run in JavaScriptCore like tests/listtoggle.rs runs
//! ui/editor.js. Four checks, three on one seeded 400-node graph:
//!   1. the struct-of-arrays step is bit-identical to the object step main.js ran before it
//!      (copied below as the reference), step by step for 300 steps;
//!   2. a held node (pin) ends every step exactly on its pin with zero velocity;
//!   3. a fresh open settles to rest: it goes quiet within its cap, and the layout it stops in is
//!      clearly closer to force balance than where the old stop (alpha decayed to 0) froze it.
//!   4. an empty graph goes quiet at once (before, alpha never moved and the frame loop ran forever).
#![cfg(target_os = "linux")]

use javascriptcore::{ContextExt, ExceptionExt, ValueExt};
use std::path::PathBuf;

fn root() -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..") }

const CHECKS: &str = r#"(function () {
  "use strict";
  const F = { repel: 10, repelK: 42.03, repelP: 0, dist: 250, center: 0.5187, centerK: 0.0771 }, LINK = 1, LINK_K = 0.5;
  let r = 7; const rnd = () => (r = r * 16807 % 2147483647) / 2147483647;
  const n = 400, E = [];
  for (let i = 1; i < n; i++) E.push([Math.floor(rnd() * i), i]);
  for (let k = 0; k < 120; k++) { const a = Math.floor(rnd() * n), b = Math.floor(rnd() * n); if (a !== b) E.push([a, b]); }
  const seed = []; for (let i = 0; i < n; i++) { const rr = 66 * Math.sqrt(i + rnd()), t = 2 * Math.PI * rnd(); seed.push([rr * Math.cos(t), rr * Math.sin(t)]); }
  const fresh = () => seed.map(([x, y]) => ({ x, y, vx: 0, vy: 0 }));
  // links as main.js rebuild() makes them: unique, strength Link / min(deg), bias by degree
  const adj = seed.map(() => new Set()); for (const [i, j] of E) { adj[i].add(j); adj[j].add(i); }
  const links = []; adj.forEach((s, i) => { for (const j of s) if (i < j) { const di = adj[i].size, dj = adj[j].size; links.push({ a: i, b: j, bias: di / (di + dj), k: LINK * LINK_K / Math.min(di, dj) }); } });
  const LNK = { LA: new Int32Array(links.map(l => l.a)), LB: new Int32Array(links.map(l => l.b)), LBI: new Float64Array(links.map(l => l.bias)), LK: new Float64Array(links.map(l => l.k)) };

  // ---- reference: main.js physStep before graph-phys.js (object nodes, recursive Barnes-Hut), verbatim but for alpha as a parameter
  function refStep(N, alpha) {
    const THETA2 = 0.8 * 0.8, MAXD = 10; const pool = []; let pn = 0;
    const cell = (x0, y0, s) => { let c = pool[pn]; if (!c) c = pool[pn] = {}; pn++; c.x0 = x0; c.y0 = y0; c.s = s; c.n = 0; c.sx = 0; c.sy = 0; c.p = null; c.k = null; return c; };
    const qi = (c, p) => (p.x >= c.x0 + c.s / 2 ? 1 : 0) + (p.y >= c.y0 + c.s / 2 ? 2 : 0);
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const p of N) { if (p.x < x0) x0 = p.x; if (p.x > x1) x1 = p.x; if (p.y < y0) y0 = p.y; if (p.y > y1) y1 = p.y; }
    const rx = Math.floor(x0), ry = Math.floor(y0); let rs = 1; while (rx + rs <= x1 || ry + rs <= y1) rs *= 2;
    const root = cell(rx, ry, rs);
    for (const p of N) { let c = root, depth = 0; for (;;) { c.n++; c.sx += p.x; c.sy += p.y;
      if (c.k) { c = c.k[qi(c, p)]; depth++; continue; } if (c.n === 1) { c.p = p; break; } if (depth >= MAXD) break;
      const h = c.s / 2, q = c.p; c.p = null; c.k = [cell(c.x0, c.y0, h), cell(c.x0 + h, c.y0, h), cell(c.x0, c.y0 + h, h), cell(c.x0 + h, c.y0 + h, h)];
      const cq = c.k[qi(c, q)]; cq.n = 1; cq.sx = q.x; cq.sy = q.y; cq.p = q; c = c.k[qi(c, p)]; depth++; } }
    function bhApply(a, c, k) { if (c.n === 0) return; const dx = a.x - c.sx / c.n, dy = a.y - c.sy / c.n, d2 = dx * dx + dy * dy + 1;
      if (c.k) { if (c.s * c.s > THETA2 * d2) { const K = c.k; bhApply(a, K[0], k); bhApply(a, K[1], k); bhApply(a, K[2], k); bhApply(a, K[3], k); return; } }
      else if (c.p === a || (a.x >= c.x0 && a.x < c.x0 + c.s && a.y >= c.y0 && a.y < c.y0 + c.s)) return;
      a.vx += k * c.n * dx / d2; a.vy += k * c.n * dy / d2; }
    const k = alpha * F.repel * F.repelK * Math.pow(N.length, F.repelP); for (const a of N) bhApply(a, root, k);
    for (const l of links) { const a = N[l.a], b = N[l.b]; let dx = b.x + b.vx - a.x - a.vx, dy = b.y + b.vy - a.y - a.vy;
      const d = Math.sqrt(dx * dx + dy * dy) || 1e-6, kk = (d - F.dist) / d * alpha * l.k; dx *= kk; dy *= kk;
      b.vx -= dx * l.bias; b.vy -= dy * l.bias; a.vx += dx * (1 - l.bias); a.vy += dy * (1 - l.bias); }
    const ck = F.center * F.centerK * alpha; let sx = 0, sy = 0; for (const p of N) { sx += p.x; sy += p.y; }
    sx /= N.length; sy /= N.length; for (const p of N) if (p.fx == null) { p.x -= sx; p.y -= sy; }
    for (const p of N) { p.vx -= p.x * ck; p.vy -= p.y * ck; p.vx *= 0.7; p.vy *= 0.7; p.x += p.vx; p.y += p.vy; if (p.fx != null) { p.x = p.fx; p.y = p.fy; p.vx = 0; p.vy = 0; } }
  }

  // ---- graph-phys.js, driven the way main.js drives it: a copy out, the result written back
  const eng = window.GraphPhys.create();
  function run(N, st, steps, first) {
    const b = { X: new Float64Array(n), Y: new Float64Array(n), VX: new Float64Array(n), VY: new Float64Array(n), PIN: new Uint8Array(n), FX: new Float64Array(n), FY: new Float64Array(n),
                links: LNK, f: F, st, steps, budget: 1e12, trace: false, first };
    N.forEach((p, i) => { b.X[i] = p.x; b.Y[i] = p.y; b.VX[i] = p.vx; b.VY[i] = p.vy; if (p.fx != null) { b.PIN[i] = 1; b.FX[i] = p.fx; b.FY[i] = p.fy; } });
    const res = eng.run(b);
    N.forEach((p, i) => { p.x = b.X[i]; p.y = b.Y[i]; p.vx = b.VX[i]; p.vy = b.VY[i]; });
    return res;
  }
  const open = () => ({ alpha: 1, alphaTarget: 0, seekN: 0, still: 0, calm: 0, kePeak: 0, quiet: false });

  // 1. bit identity, one step at a time: the reference takes the alpha the engine stepped with
  {
    const A = fresh(), B = fresh(), st = open();
    for (let s = 0; s < 300 && !st.quiet; s++) {
      run(A, st, 1, true); refStep(B, st.alpha);
      for (let i = 0; i < n; i++) if (A[i].x !== B[i].x || A[i].y !== B[i].y || A[i].vx !== B[i].vx || A[i].vy !== B[i].vy)
        throw new Error("step " + (s + 1) + " node " + i + ": graph-phys " + [A[i].x, A[i].y, A[i].vx, A[i].vy] + " != reference " + [B[i].x, B[i].y, B[i].vx, B[i].vy]);
    }
  }
  // 2. a held node stays on its pin
  {
    const A = fresh(), st = open(); A[5].fx = 123.25; A[5].fy = -45.5; st.alphaTarget = 0.3;
    for (let s = 0; s < 50; s++) { run(A, st, 1, true); if (A[5].x !== 123.25 || A[5].y !== -45.5 || A[5].vx !== 0 || A[5].vy !== 0) throw new Error("pin moved at step " + (s + 1)); }
  }
  // 3. settle to rest vs the old stop (alpha toward 0, quiet on calm or alpha <= 0.001). The measure is
  //    the force still acting where each run stopped: from rest, the mean move one full-strength step
  //    (alpha 1) gives a node. Kinetic energy would not do: with the forces faded out it is near 0 at
  //    any stop, balanced or not. Measured on this graph: old stop 7.13, settle to rest 3.86; with the
  //    alpha floor removed (ALPHA_REST 0) the stop is back at 7.24, so the 0.75 bound tells them apart.
  {
    const ke = N => N.reduce((s, p) => s + p.vx * p.vx + p.vy * p.vy, 0);
    const unbalance = N => { const C = N.map(p => ({ x: p.x, y: p.y, vx: 0, vy: 0 })); refStep(C, 1); return C.reduce((s, p) => s + Math.hypot(p.vx, p.vy), 0) / n; };
    const O = fresh(); let alpha = 1, peak = 0, calm = 0;
    for (;;) { alpha += (0 - alpha) * 0.028; refStep(O, alpha); const k = ke(O); if (k > peak) peak = k;
      calm = k < Math.max(4e-5 * peak, 1e-6 * n) ? calm + 1 : 0; if (calm >= 10 || alpha <= 0.001) break; }
    const A = fresh(), st = open(); let steps = 0;
    while (!st.quiet && steps < 20000) steps += run(A, st, 200, true).ran;
    if (!st.quiet) throw new Error("no rest after " + steps + " steps");
    if (steps > 75 * 60 + 1000) throw new Error("rest took " + steps + " steps, past the 75 s cap + ease-out");
    const uOld = unbalance(O), uNew = unbalance(A);
    if (!(uNew < uOld * 0.75)) throw new Error("stopped at unbalance " + uNew + ", the old stop left " + uOld);
  }
  // 4. an empty graph (an empty vault, or filters that hide every note) goes quiet at once
  {
    const st = open(), r = eng.run({ X: new Float64Array(0), Y: new Float64Array(0), VX: new Float64Array(0), VY: new Float64Array(0), PIN: new Uint8Array(0),
      FX: new Float64Array(0), FY: new Float64Array(0), links: LNK, f: F, st, steps: 7, budget: 1e12, trace: false, first: true });
    if (!st.quiet || r.quietKe !== 0) throw new Error("an empty graph did not go quiet: " + JSON.stringify(r));
  }
  return "ok";
})()"#;

#[test]
fn graph_physics() {
    let src = std::fs::read_to_string(root().join("ui/graph-phys.js")).unwrap();
    let ctx = javascriptcore::Context::new();
    // graph-phys.js defines window.GraphPhys when `window` exists (else it is a Worker), and
    // reads performance for its batch clock, which a bare context does not have
    ctx.evaluate("var window = {}; var performance = { timeOrigin: 0, now: function () { return Date.now(); } };");
    ctx.evaluate(&src);
    if let Some(e) = ctx.exception() { panic!("graph-phys.js did not load: {}", e.to_str()); }
    let out = ctx.evaluate(CHECKS).map(|v| v.to_str().to_string()).unwrap_or_default();
    if let Some(e) = ctx.exception() { panic!("graph physics check failed: {}", e.to_str()); }
    assert_eq!(out, "ok");
}
