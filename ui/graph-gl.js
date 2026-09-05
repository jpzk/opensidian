/* graph-gl: WebGL draw path for the graph view (global + local). DRAW LAYER ONLY —
   physics, hit-testing, labels and the view transform live in main.js startGraph.
   Nodes = one instanced quad per node (radius + colour + ring per instance,
   antialiased circle edge in the fragment shader, px units, no derivatives ext);
   edges = one gl.LINES buffer (1px, R16 link thickness 1). GLSL ES 1.00 for both
   webgl2 (native instancing) and webgl (ANGLE_instanced_arrays). No context ->
   create() returns null and main.js keeps its Canvas 2D path. */
(function () {
  const VS = `
    attribute vec2 a_corner;                 // quad corner in {-1,1}
    attribute vec2 a_pos;                    // node centre, world
    attribute float a_r;                     // node radius, world
    attribute float a_ring;                  // 0 = filled disc, else ring width (px)
    attribute vec4 a_col;
    uniform vec2 u_res; uniform float u_scale; uniform vec2 u_t;
    varying vec2 v_uv; varying float v_rpx; varying float v_ring; varying vec4 v_col;
    void main() {
      float rpx = a_r * u_scale;
      vec2 c = a_corner * (rpx + 1.5);       // +1.5px apron for the AA edge
      vec2 s = a_pos * u_scale + u_t + c;    // screen px (y down)
      gl_Position = vec4(s / u_res * 2.0 - 1.0, 0.0, 1.0); gl_Position.y = -gl_Position.y;
      v_uv = c; v_rpx = rpx; v_ring = a_ring; v_col = a_col;
    }`;
  const FS = `
    precision mediump float;
    varying vec2 v_uv; varying float v_rpx; varying float v_ring; varying vec4 v_col;
    void main() {
      float d = length(v_uv);
      float cov = clamp(v_rpx - d + 0.5, 0.0, 1.0);                       // outer edge, 1px AA
      if (v_ring > 0.0) cov *= clamp(d - (v_rpx - v_ring) + 0.5, 0.0, 1.0);  // hollow = unresolved
      gl_FragColor = vec4(v_col.rgb, v_col.a * cov);
    }`;
  const EVS = `
    attribute vec2 a_pos; attribute vec4 a_col;
    uniform vec2 u_res; uniform float u_scale; uniform vec2 u_t;
    varying vec4 v_col;
    void main() {
      vec2 s = a_pos * u_scale + u_t;
      gl_Position = vec4(s / u_res * 2.0 - 1.0, 0.0, 1.0); gl_Position.y = -gl_Position.y;
      v_col = a_col;
    }`;
  const EFS = `precision mediump float; varying vec4 v_col; void main() { gl_FragColor = v_col; }`;
  function prog(gl, vs, fs) {
    const sh = (t, src) => { const s = gl.createShader(t); gl.shaderSource(s, src); gl.compileShader(s);
      if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error("shader: " + gl.getShaderInfoLog(s)); return s; };
    const p = gl.createProgram(); gl.attachShader(p, sh(gl.VERTEX_SHADER, vs)); gl.attachShader(p, sh(gl.FRAGMENT_SHADER, fs)); gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error("link: " + gl.getProgramInfoLog(p));
    return p;
  }
  // create(canvas, onLost) -> renderer | null. onLost fires once on webglcontextlost (main.js swaps to 2d).
  function create(cv, onLost) {
    const attrs = { antialias: true, powerPreference: "high-performance", preserveDrawingBuffer: false, alpha: false };
    let gl = null, ver = 0;
    try { gl = cv.getContext("webgl2", attrs); if (gl) ver = 2; } catch (e) { gl = null; }
    if (!gl) { try { gl = cv.getContext("webgl", attrs) || cv.getContext("experimental-webgl", attrs); if (gl) ver = 1; } catch (e) { gl = null; } }
    if (!gl) return null;
    let inst = null;                          // instancing: native on webgl2, ANGLE ext on webgl1
    if (ver === 2) inst = { div: (i, d) => gl.vertexAttribDivisor(i, d), draw: (m, f, c, n) => gl.drawArraysInstanced(m, f, c, n) };
    else { const ext = gl.getExtension("ANGLE_instanced_arrays");
      if (!ext) return null;
      inst = { div: (i, d) => ext.vertexAttribDivisorANGLE(i, d), draw: (m, f, c, n) => ext.drawArraysInstancedANGLE(m, f, c, n) }; }
    const dbg = gl.getExtension("WEBGL_debug_renderer_info");
    const info = { webgl: ver,
      vendor: dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : gl.getParameter(gl.VENDOR),
      renderer: dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : gl.getParameter(gl.RENDERER) };
    let np, ep;
    try { np = prog(gl, VS, FS); ep = prog(gl, EVS, EFS); } catch (e) { return null; }
    const loc = (p, n) => gl.getAttribLocation(p, n), uni = (p, n) => gl.getUniformLocation(p, n);
    const N = { corner: loc(np, "a_corner"), pos: loc(np, "a_pos"), r: loc(np, "a_r"), ring: loc(np, "a_ring"), col: loc(np, "a_col"),
                res: uni(np, "u_res"), scale: uni(np, "u_scale"), t: uni(np, "u_t") };
    const E = { pos: loc(ep, "a_pos"), col: loc(ep, "a_col"), res: uni(ep, "u_res"), scale: uni(ep, "u_scale"), t: uni(ep, "u_t") };
    const quad = gl.createBuffer(); gl.bindBuffer(gl.ARRAY_BUFFER, quad);
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]), gl.STATIC_DRAW);
    const nbuf = gl.createBuffer(), ebuf = gl.createBuffer();
    gl.disable(gl.DEPTH_TEST); gl.enable(gl.BLEND); gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
    gl.clearColor(0x11 / 255, 0x11 / 255, 0x1b / 255, 1);
    let lost = false;
    cv.addEventListener("webglcontextlost", e => { e.preventDefault(); if (lost) return; lost = true; onLost && onLost(); }, false);
    const R = {
      kind: "gl", info, get lost() { return lost; },
      // draw(w, h, view, nodes: Float32Array [x y r ring rgba]*n, nCount, edges: Float32Array [x y rgba]*2e, eCount)
      draw(w, h, view, nodes, nCount, edges, eCount) {
        if (lost) return false;
        if (cv.width !== w || cv.height !== h) { cv.width = w; cv.height = h; }
        gl.viewport(0, 0, w, h);
        gl.clear(gl.COLOR_BUFFER_BIT);
        if (eCount) {                          // 6 floats per vertex, 2 vertices per edge
          gl.useProgram(ep);
          gl.uniform2f(E.res, w, h); gl.uniform1f(E.scale, view.scale); gl.uniform2f(E.t, view.tx, view.ty);
          gl.bindBuffer(gl.ARRAY_BUFFER, ebuf); gl.bufferData(gl.ARRAY_BUFFER, edges.subarray(0, eCount * 12), gl.DYNAMIC_DRAW);
          gl.enableVertexAttribArray(E.pos); gl.vertexAttribPointer(E.pos, 2, gl.FLOAT, false, 24, 0);
          gl.enableVertexAttribArray(E.col); gl.vertexAttribPointer(E.col, 4, gl.FLOAT, false, 24, 8);
          gl.lineWidth(1);
          gl.drawArrays(gl.LINES, 0, eCount * 2);
          gl.disableVertexAttribArray(E.pos); gl.disableVertexAttribArray(E.col);
        }
        if (nCount) {                          // 8 floats per instance
          gl.useProgram(np);
          gl.uniform2f(N.res, w, h); gl.uniform1f(N.scale, view.scale); gl.uniform2f(N.t, view.tx, view.ty);
          gl.bindBuffer(gl.ARRAY_BUFFER, quad);
          gl.enableVertexAttribArray(N.corner); gl.vertexAttribPointer(N.corner, 2, gl.FLOAT, false, 0, 0); inst.div(N.corner, 0);
          gl.bindBuffer(gl.ARRAY_BUFFER, nbuf); gl.bufferData(gl.ARRAY_BUFFER, nodes.subarray(0, nCount * 8), gl.DYNAMIC_DRAW);
          for (const [a, sz, off] of [[N.pos, 2, 0], [N.r, 1, 8], [N.ring, 1, 12], [N.col, 4, 16]]) {
            gl.enableVertexAttribArray(a); gl.vertexAttribPointer(a, sz, gl.FLOAT, false, 32, off); inst.div(a, 1);
          }
          inst.draw(gl.TRIANGLES, 0, 6, nCount);
          for (const a of [N.corner, N.pos, N.r, N.ring, N.col]) { inst.div(a, 0); gl.disableVertexAttribArray(a); }
        }
        return true;
      },
      loseContext() {                          // test hook (smoke graphgl): simulate a lost context
        const ext = gl.getExtension("WEBGL_lose_context"); if (ext) ext.loseContext(); else { lost = true; onLost && onLost(); }
      },
      destroy() { lost = true; try { gl.deleteBuffer(quad); gl.deleteBuffer(nbuf); gl.deleteBuffer(ebuf); gl.deleteProgram(np); gl.deleteProgram(ep); } catch (e) {} },
    };
    return R;
  }
  window.GraphGL = { create };
})();
