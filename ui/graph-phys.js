// SPDX-License-Identifier: GPL-3.0-or-later
// Graph physics + settle test, one batch of steps at a time (graphworker).
//
// Loaded twice: as a Worker script (the graph sim posts a batch, this runs it off the main thread
// and posts the result back), and as a page script defining window.GraphPhys, which the sim runs
// synchronously when a Worker cannot be started. Either way the main thread owns the node objects
// and the sim state: a batch carries a copy of both in, the result carries them out, so this file
// keeps nothing between batches but its scratch arrays and the last link set.
//
// The step is the struct-of-arrays step that ui/main.js ran in-line (same operations, same order,
// so positions are bit-identical to it), and the settle test is main.js's settleTest minus its
// side effects: the result reports what happened (steps run, kinetic energy, the step that went
// quiet, the trace rows) and main.js does the spans, marks and classes.
(function () {
  "use strict";
  const PH_HZ = 60, ALPHA_MIN = 0.001, ALPHA_DECAY = 0.028, CALM_KE = 4e-5;
  // settle to rest (see main.js startGraph): alpha decays toward ALPHA_REST, not 0, until every node
  // is slower than V_REST world units/step for 10 steps or REST_CAP steps have passed
  // OPEN_CAP: the first settle of an open runs at main.js OPEN_SPEED, so it may seek rest for 75 s of
  // sim time (under 4 s of wall time) and get closer to the balance; a later one runs at 1x and keeps 30 s.
  const ALPHA_REST = 0.1, V_REST = 0.3, REST_CAP = 30 * PH_HZ, OPEN_CAP = 75 * PH_HZ;
  // Barnes-Hut quadtree (theta 0.8) for the many-body repulsion (d3 shape: dv = REPEL*alpha*dx/d^2).
  // Every far cell (side/dist < theta) acts as one body at its centroid; depth capped at MAXD so a
  // coincident pair cannot split 24 levels; points sharing a leaf do not repel each other.
  const THETA2 = 0.8 * 0.8, MAXD = 10;
  const now = () => performance.timeOrigin + performance.now();   // epoch ms: comparable across threads

  function create() {
    let ccap = 0, CX, CY, CS, CSX, CSY, CN, CP, CK, pn = 0;
    let LA, LB, LBI, LK;
    const STK = new Int32Array(4 * MAXD + 64);
    const cgrow = need => {
      const c2 = Math.max(need, ccap * 2), g = (A, T) => { const B = new T(c2); if (A) B.set(A); return B; };
      CX = g(CX, Float64Array); CY = g(CY, Float64Array); CS = g(CS, Float64Array); CSX = g(CSX, Float64Array); CSY = g(CSY, Float64Array);
      CN = g(CN, Int32Array); CP = g(CP, Int32Array); CK = g(CK, Int32Array); ccap = c2;
    };
    const cell = (x0, y0, s) => {
      if (pn >= ccap) cgrow(pn + 1);
      const c = pn++; CX[c] = x0; CY[c] = y0; CS[c] = s; CN[c] = 0; CSX[c] = 0; CSY[c] = 0; CP[c] = -1; CK[c] = -1;
      return c;
    };
    function bhBuild(X, Y, n) {
      pn = 0;
      let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
      for (let i = 0; i < n; i++) { const x = X[i], y = Y[i]; if (x < x0) x0 = x; if (x > x1) x1 = x; if (y < y0) y0 = y; if (y > y1) y1 = y; }
      // d3 quadtree cover(): root anchored at floor(min), side the smallest power of two that holds the extent
      const rx = Math.floor(x0), ry = Math.floor(y0); let rs = 1;
      while (rx + rs <= x1 || ry + rs <= y1) rs *= 2;
      cell(rx, ry, rs);
      for (let i = 0; i < n; i++) {
        const px = X[i], py = Y[i]; let c = 0, depth = 0;
        for (;;) {
          CN[c]++; CSX[c] += px; CSY[c] += py;
          if (CK[c] >= 0) { c = CK[c] + (px >= CX[c] + CS[c] / 2 ? 1 : 0) + (py >= CY[c] + CS[c] / 2 ? 2 : 0); depth++; continue; }
          if (CN[c] === 1) { CP[c] = i; break; }        // empty leaf takes i
          if (depth >= MAXD) break;                     // shared leaf: mass only
          const h = CS[c] / 2, q = CP[c], cx0 = CX[c], cy0 = CY[c]; CP[c] = -1;   // occupied leaf: split, push old point down
          const k0 = cell(cx0, cy0, h); cell(cx0 + h, cy0, h); cell(cx0, cy0 + h, h); cell(cx0 + h, cy0 + h, h); CK[c] = k0;
          const cq = k0 + (X[q] >= cx0 + CS[c] / 2 ? 1 : 0) + (Y[q] >= cy0 + CS[c] / 2 ? 2 : 0);
          CN[cq] = 1; CSX[cq] = X[q]; CSY[cq] = Y[q]; CP[cq] = q;
          c = k0 + (px >= cx0 + CS[c] / 2 ? 1 : 0) + (py >= cy0 + CS[c] / 2 ? 2 : 0); depth++;
        }
      }
    }
    // depth-first in child order 0..3, exactly the recursion main.js had (the sum order is the result)
    function bhApply(X, Y, VX, VY, a, k) {
      const ax = X[a], ay = Y[a]; let vx = VX[a], vy = VY[a], sp = 0;
      STK[sp++] = 0;
      while (sp) {
        const c = STK[--sp], cn = CN[c]; if (cn === 0) continue;
        const dx = ax - CSX[c] / cn, dy = ay - CSY[c] / cn, d2 = dx * dx + dy * dy + 1, ch = CK[c];
        if (ch >= 0) {
          const s = CS[c];
          if (s * s > THETA2 * d2) { STK[sp++] = ch + 3; STK[sp++] = ch + 2; STK[sp++] = ch + 1; STK[sp++] = ch; continue; }
        } else if (CP[c] === a || (ax >= CX[c] && ax < CX[c] + CS[c] && ay >= CY[c] && ay < CY[c] + CS[c])) continue;  // own leaf
        vx += k * cn * dx / d2; vy += k * cn * dy / d2;
      }
      VX[a] = vx; VY[a] = vy;
    }
    function physStep(b, st) {
      const X = b.X, Y = b.Y, VX = b.VX, VY = b.VY, PIN = b.PIN, n = X.length, f = b.f;
      st.alpha += ((st.alphaTarget || (st.seekN >= 0 ? ALPHA_REST : 0)) - st.alpha) * ALPHA_DECAY;   // top of the step; floor while seeking rest
      const alpha = st.alpha;
      // many-body repulsion FIRST (the link term reads p+v after it)
      if (ccap < 4 * n + 64) cgrow(4 * n + 64);
      bhBuild(X, Y, n); const k = alpha * f.repel * f.repelK * Math.pow(n, f.repelP);
      for (let a = 0; a < n; a++) bhApply(X, Y, VX, VY, a, k);
      // link: spring to f.dist, d3 semantics (strength/bias per link)
      for (let i = 0, L = LA.length; i < L; i++) {
        const a = LA[i], b2 = LB[i];
        let dx = X[b2] + VX[b2] - X[a] - VX[a], dy = Y[b2] + VY[b2] - Y[a] - VY[a];
        const d = Math.sqrt(dx * dx + dy * dy) || 1e-6, kk = (d - f.dist) / d * alpha * LK[i], bi = LBI[i];
        dx *= kk; dy *= kk;
        VX[b2] -= dx * bi; VY[b2] -= dy * bi; VX[a] += dx * (1 - bi); VY[a] += dy * (1 - bi);
      }
      // centre: per-node alpha-scaled pull to the origin, plus the d3 forceCenter translation
      // (a held node counts in the centroid but is not moved: fx snaps it back)
      const ck = f.center * f.centerK * alpha;
      let sx = 0, sy = 0;
      for (let i = 0; i < n; i++) { sx += X[i]; sy += Y[i]; }
      sx /= n; sy /= n; for (let i = 0; i < n; i++) if (!PIN[i]) { X[i] -= sx; Y[i] -= sy; }
      for (let i = 0; i < n; i++) {
        let vx = VX[i] - X[i] * ck, vy = VY[i] - Y[i] * ck;
        vx *= 0.7; vy *= 0.7; let x = X[i] + vx, y = Y[i] + vy;   // velocity decay 0.3 (measured)
        if (PIN[i]) { x = b.FX[i]; y = b.FY[i]; vx = 0; vy = 0; }  // C3 pin, only while held
        X[i] = x; Y[i] = y; VX[i] = vx; VY[i] = vy;
      }
    }
    // main.js settleTest without its side effects: may set st.quiet; returns the kinetic energy
    function settle(b, st, tr) {
      const VX = b.VX, VY = b.VY, n = VX.length;
      let ke = 0, vm2 = 0;
      for (let i = 0; i < n; i++) { const v2 = VX[i] * VX[i] + VY[i] * VY[i]; ke += v2; if (v2 > vm2) vm2 = v2; }
      if (st.seekN >= 0 && st.alphaTarget === 0) {
        st.still = vm2 < V_REST * V_REST ? st.still + 1 : 0;
        // at rest (or out of patience): drop the floor, and measure the ease-out against its own peak
        if (st.still >= 10 || ++st.seekN >= (b.first ? OPEN_CAP : REST_CAP)) { st.seekN = -1; st.still = 0; st.calm = 0; st.kePeak = 0; }
        else return ke;
      }
      if (ke > st.kePeak) st.kePeak = ke;
      st.calm = ke < Math.max(CALM_KE * st.kePeak, 1e-6 * n) ? st.calm + 1 : 0;
      if (tr) { tr.a.push(String(st.alpha)); tr.k.push(ke.toPrecision(6)); tr.p.push(st.kePeak.toPrecision(6)); tr.c.push(st.calm); tr.t.push(now()); }
      // never freeze while a node is HELD (alphaTarget > 0)
      if (st.alphaTarget === 0 && (st.calm >= 10 || st.alpha <= ALPHA_MIN)) st.quiet = true;
      return ke;
    }
    // b: { X, Y, VX, VY, PIN, FX, FY, links, f, st, steps, budget, trace, first }. Mutates the arrays and
    // b.st in place and returns { ran, ke, quietKe, ms, tr }: at most b.steps steps, stopping at quiet
    // or once b.budget ms of wall time are spent, the settle test after every step.
    function run(b) {
      LA = b.links.LA; LB = b.links.LB; LBI = b.links.LBI; LK = b.links.LK;
      const st = b.st, t0 = now(), tr = b.trace ? { a: [], k: [], p: [], c: [], t: [] } : null;
      let ran = 0, ke = -1, quietKe = -1;
      if (b.X.length) {
        for (let s = 0; s < b.steps && !st.quiet; s++) {
          if (st.alpha > ALPHA_MIN || st.alphaTarget > 0) { physStep(b, st); ran++; ke = settle(b, st, tr); }
          if (st.quiet) { quietKe = ke; break; }
          if (ran && now() - t0 >= b.budget) break;    // wall-clock backstop: a costlier step must not lengthen the batch
        }
        if (!st.quiet && !ran && st.alpha <= ALPHA_MIN) { ke = settle(b, st, tr); if (st.quiet) quietKe = ke; }
      }
      return { ran, ke, quietKe, ms: now() - t0, tr };
    }
    return { run };
  }

  if (typeof window === "undefined") {             // Worker: one engine, batches in, results out (arrays transferred both ways)
    const eng = create();
    self.onmessage = e => {
      const b = e.data, r = eng.run(b);
      self.postMessage({ id: b.id, r, st: b.st, X: b.X, Y: b.Y, VX: b.VX, VY: b.VY }, [b.X.buffer, b.Y.buffer, b.VX.buffer, b.VY.buffer]);
    };
  } else window.GraphPhys = { create };
})();
