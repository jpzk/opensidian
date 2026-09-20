// SPDX-License-Identifier: GPL-3.0-or-later
/* graph-gl: WebGL draw path for the graph view (global + local). DRAW LAYER ONLY —
   physics, hit-testing, labels and the view transform live in main.js startGraph.
   Nodes = one instanced quad per node (radius + colour + ring per instance);
   edges = one instanced quad per segment (R16 link thickness 1). Both are
   antialiased in the fragment shader in PIXEL units (coverage = distance-based,
   no derivatives extension, no MSAA — 4x MSAA quadruples fragment work on a
   software rasterizer for no gain over shader coverage). GLSL ES 1.00 for both
   webgl2 (native instancing) and webgl (ANGLE_instanced_arrays). No context ->
   create() returns null and main.js keeps its Canvas 2D path. */
(function () {
  const PROJ = `
    uniform vec2 u_res; uniform float u_scale; uniform vec2 u_t;
    vec4 proj(vec2 s) { vec4 p = vec4(s / u_res * 2.0 - 1.0, 0.0, 1.0); p.y = -p.y; return p; }`;
  const NVS = PROJ + `
    attribute vec2 a_corner;                 // quad corner in {-1,1}
    attribute vec2 a_pos;                    // node centre, world
    attribute float a_r;                     // node radius, world
    attribute float a_ring;                  // 0 = filled disc, else ring width (px)
    attribute vec4 a_col;
    varying vec2 v_uv; varying float v_rpx; varying float v_ring; varying vec4 v_col;
    void main() {
      float rpx = a_r * u_scale;
      vec2 c = a_corner * (rpx + 1.5);       // +1.5px apron for the AA edge
      gl_Position = proj(a_pos * u_scale + u_t + c);
      v_uv = c; v_rpx = rpx; v_ring = a_ring; v_col = a_col;
    }`;
  const NFS = `
    precision mediump float;
    varying vec2 v_uv; varying float v_rpx; varying float v_ring; varying vec4 v_col;
    void main() {
      float d = length(v_uv);
      float cov = clamp(v_rpx - d + 0.5, 0.0, 1.0);                          // outer edge, 1px AA
      if (v_ring > 0.0) cov *= clamp(d - (v_rpx - v_ring) + 0.5, 0.0, 1.0);  // hollow = unresolved
      gl_FragColor = vec4(v_col.rgb, v_col.a * cov);
    }`;
  const EVS = PROJ + `
    attribute vec2 a_corner;                 // x: along the segment (-1 = p0 end), y: across
    attribute vec2 a_p0; attribute vec2 a_p1; attribute vec4 a_col;
    varying float v_d; varying float v_l; varying float v_len; varying vec4 v_col;
    void main() {
      vec2 s0 = a_p0 * u_scale + u_t, s1 = a_p1 * u_scale + u_t;
      vec2 dv = s1 - s0; float len = length(dv);
      vec2 dir = len > 1e-3 ? dv / len : vec2(1.0, 0.0), nrm = vec2(-dir.y, dir.x);
      float along = mix(-1.5, len + 1.5, a_corner.x * 0.5 + 0.5);   // 1.5px apron past both caps
      gl_Position = proj(s0 + dir * along + nrm * a_corner.y * 1.5);
      v_d = a_corner.y * 1.5; v_l = along; v_len = len; v_col = a_col;
    }`;
  const EFS = `
    precision mediump float;
    varying float v_d; varying float v_l; varying float v_len; varying vec4 v_col;
    void main() {
      float cov = clamp(1.0 - abs(v_d), 0.0, 1.0)                           // 1px core, tent falloff
                * clamp(v_l + 0.5, 0.0, 1.0) * clamp(v_len - v_l + 0.5, 0.0, 1.0);   // butt caps
      gl_FragColor = vec4(v_col.rgb, v_col.a * cov);
    }`;
  function prog(gl, vs, fs) {
    const sh = (t, src) => { const s = gl.createShader(t); gl.shaderSource(s, src); gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error("shader: " + gl.getShaderInfoLog(s)); return s; };
    const p = gl.createProgram(); gl.attachShader(p, sh(gl.VERTEX_SHADER, vs)); gl.attachShader(p, sh(gl.FRAGMENT_SHADER, fs)); gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error("link: " + gl.getProgramInfoLog(p));
    return p;
  }
  // create(canvas, onLost, bg) -> renderer | null. onLost fires once on webglcontextlost (main.js swaps to 2d).
  // bg = [r,g,b] in 0..1: the clear colour for the warm-up frame. THIS FILE HOLDS NO COLOUR OF ITS OWN —
  // every rgb here arrives from main.js, which reads it out of the stylesheet token block (--graph-bg for
  // the clear). It used to clear to a literal dark, which was one palette's crust behind every other
  // palette's light mode (goal graphtheme, D2).
  function create(cv, onLost, bg) {
    const attrs = { antialias: false, powerPreference: "high-performance", preserveDrawingBuffer: false, alpha: false, depth: false, stencil: false };
    let gl = null, ver = 0;
    try { gl = cv.getContext("webgl2", attrs); if (gl) ver = 2; } catch (e) { gl = null; }
    if (!gl) { try { gl = cv.getContext("webgl", attrs) || cv.getContext("experimental-webgl", attrs); if (gl) ver = 1; } catch (e) { gl = null; } }
    if (!gl) return null;
    let inst = null;                          // instancing: native on webgl2, ANGLE ext on webgl1
    if (ver === 2) inst = { div: (i, d) => gl.vertexAttribDivisor(i, d), draw: (n) => gl.drawArraysInstanced(gl.TRIANGLES, 0, 6, n) };
    else { const ext = gl.getExtension("ANGLE_instanced_arrays");
      if (!ext) return null;
      inst = { div: (i, d) => ext.vertexAttribDivisorANGLE(i, d), draw: (n) => ext.drawArraysInstancedANGLE(gl.TRIANGLES, 0, 6, n) }; }
    const dbg = gl.getExtension("WEBGL_debug_renderer_info");
    const info = { webgl: ver,
      vendor: dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : gl.getParameter(gl.VENDOR),
      renderer: dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER) };
    let np, ep;
    try { np = prog(gl, NVS, NFS); ep = prog(gl, EVS, EFS); } catch (e) { return null; }
    const loc = (p, n) => gl.getAttribLocation(p, n), uni = (p, n) => gl.getUniformLocation(p, n);
    const N = { corner: loc(np, "a_corner"), res: uni(np, "u_res"), scale: uni(np, "u_scale"), t: uni(np, "u_t"),
                attrs: [[loc(np, "a_pos"), 2, 0], [loc(np, "a_r"), 1, 8], [loc(np, "a_ring"), 1, 12], [loc(np, "a_col"), 4, 16]] };
    const E = { corner: loc(ep, "a_corner"), res: uni(ep, "u_res"), scale: uni(ep, "u_scale"), t: uni(ep, "u_t"),
                attrs: [[loc(ep, "a_p0"), 2, 0], [loc(ep, "a_p1"), 2, 8], [loc(ep, "a_col"), 4, 16]] };
    const quad = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, quad);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]), gl.STATIC_DRAW);
    const nbuf = gl.createBuffer(), ebuf = gl.createBuffer();
    gl.disable(gl.DEPTH_TEST); gl.enable(gl.BLEND); gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    // one instanced pass: P = program table, buf = instance buffer, data = Float32Array (8 floats / instance)
    const pass = (P, prg, buf, data, n, view, w, h) => {
      gl.useProgram(prg);
      gl.uniform2f(P.res, w, h); gl.uniform1f(P.scale, view.scale); gl.uniform2f(P.t, view.tx, view.ty);
      gl.bindBuffer(gl.ARRAY_BUFFER, quad);
      gl.enableVertexAttribArray(P.corner); gl.vertexAttribPointer(P.corner, 2, gl.FLOAT, false, 0, 0); inst.div(P.corner, 0);
      gl.bindBuffer(gl.ARRAY_BUFFER, buf); gl.bufferData(gl.ARRAY_BUFFER, data.subarray(0, n * 8), gl.DYNAMIC_DRAW);
      for (const [a, sz, off] of P.attrs) { gl.enableVertexAttribArray(a); gl.vertexAttribPointer(a, sz, gl.FLOAT, false, 32, off); inst.div(a, 1); }
      inst.draw(n);
      for (const [a] of P.attrs) { inst.div(a, 0); gl.disableVertexAttribArray(a); }
      gl.disableVertexAttribArray(P.corner);
    };
    let lost = false;
    cv.addEventListener("webglcontextlost", e => { e.preventDefault(); if (lost) return; lost = true; onLost && onLost(); }, false);
    const R = {
      kind: "gl", info, get lost() { return lost; },
      // draw(w, h, view, nodes: Float32Array [x y r ring r g b a]*n, nCount, edges: Float32Array [x0 y0 x1 y1 r g b a]*e, eCount, bg: [r g b])
      draw(w, h, view, nodes, nCount, edges, eCount, bg) {
        if (lost) return false;
        if (cv.width !== w || cv.height !== h) { cv.width = w; cv.height = h; }
        gl.viewport(0, 0, w, h);
        // bg is the resolved [r,g,b] main.js got from the browser. If resolution failed for
        // BOTH the theme's text and our own :root fallback it hands null — keep the previous
        // clear colour rather than clearing to a silent black: a channel this file cannot read
        // is a colour it must not invent (a NaN channel clamps to 0, which is how a themed
        // graph rendered black under a fixture typing a notation the old parser did not know).
        if (bg && bg.length >= 3) { gl.clearColor(bg[0], bg[1], bg[2], 1); }   // per frame: the token can change between frames (palette / mode switch), the context does not
        gl.clear(gl.COLOR_BUFFER_BIT);
        if (eCount) pass(E, ep, ebuf, edges, eCount, view, w, h);
        if (nCount) pass(N, np, nbuf, nodes, nCount, view, w, h);
        return true;
      },
      loseContext() {                          // test hook (smoke graphgl): simulate a lost context
        const ext = gl.getExtension("WEBGL_lose_context"); if (ext) ext.loseContext(); else { lost = true; onLost && onLost(); }
      },
      destroy() { lost = true; try { gl.deleteBuffer(quad); gl.deleteBuffer(nbuf); gl.deleteBuffer(ebuf); gl.deleteProgram(np); gl.deleteProgram(ep); } catch (e) {} },
    };
    // warm-up: the driver builds its pipelines on the FIRST draw of each program (llvmpipe: ~300ms of
    // shader JIT) — pay that here, at renderer creation, not inside the first sim frame
    const one = new Float32Array([0, 0, 1, 0, 0, 0, 0, 0]);
    R.draw(Math.max(cv.width, 1), Math.max(cv.height, 1), { scale: 1, tx: 0, ty: 0 }, one, 1, one, 1, bg);
    return R;
  }
  window.GraphGL = { create };
})();
