/* otel (R18): frontend lag spans in the OpenTelemetry data model. Loaded
   before main.js; exposes window.otel. Zero deps.

   A UI action span measures action -> PAINT: otel.paint(sp) ends the span
   once the next frame is committed (requestAnimationFrame, then the task
   after its style/layout/paint). R20: the idle wait for that frame tick is
   recorded as vsync_ms and subtracted by scripts/otel-flat.sh (ms = app
   latency; wall_ms keeps the raw number). Spans are
   buffered and shipped to Rust in ONE log_spans IPC every 250ms (not per
   span); the backend writes them as OTLP/JSON lines into RUSTIDIAN_OTEL.
   The first reply says whether telemetry is on; when it is not, every later
   call is a pure no-op (no buffer, no IPC).

   Trace propagation: while an action span is open (begin..end) it is the
   CURRENT ctx; main.js's inv() attaches {traceId, spanId} as the `otel`
   invoke arg, so backend spans (read_note, render_blocks, ...) become its
   children in the same trace. Ids are hand-rolled hex (clock + counter +
   Math.random), no crypto needed for telemetry.

   API:  const sp = otel.begin(name, attrs)     -> span handle (t0 = now)
         otel.end(sp, moreAttrs)               -> record now as the end
         otel.paint(sp, moreAttrs)             -> end at the next frame commit (returns a Promise)
         otel.span(name, attrs, ms)            -> record an already-measured span (ms long, ending now)
         otel.ctx()                            -> {traceId, spanId} of the innermost open span, or null
         otel.flush()                          -> ship the buffer now
         otel.now()                            -> performance.now() */
(function () {
  const hex = (n, w) => Math.floor(n).toString(16).padStart(w, "0").slice(-w);
  let seq = 0;
  const rnd = () => hex(Math.random() * 0x100000000, 8);
  const traceId = () => hex(Date.now(), 12) + hex(++seq, 4) + rnd() + rnd();  // 32 hex = 16 bytes
  const spanId = () => hex(++seq, 6) + hex(Date.now() & 0xff, 2) + rnd();       // 16 hex = 8 bytes
  const stack = [];           // open action spans, innermost last
  const buf = [];
  let on = null, timer = null, flushing = false;
  const otel = {
    on: () => on,
    now: () => performance.now(),
    ctx() {
      const s = stack[stack.length - 1];
      return s ? { traceId: s.traceId, spanId: s.spanId } : null;
    },
    begin(name, attrs = {}) {
      const parent = stack[stack.length - 1];
      const sp = { name, attrs, t0: performance.now(), t1: -1,
                   traceId: parent ? parent.traceId : traceId(), spanId: spanId(),
                   parentSpanId: parent ? parent.spanId : "" };
      if (on !== false) stack.push(sp);
      return sp;
    },
    end(sp, more) {
      if (!sp || sp.t1 >= 0) return;
      sp.t1 = performance.now();
      const i = stack.lastIndexOf(sp); if (i >= 0) stack.splice(i, 1);
      if (on === false) return;
      if (more) Object.assign(sp.attrs, more);
      push(sp);
    },
    cancel(sp) {                // drop an open span unrecorded (superseded by a newer action)
      if (!sp || sp.t1 >= 0) return;
      sp.t1 = 0; const i = stack.lastIndexOf(sp); if (i >= 0) stack.splice(i, 1);
    },
    paint(sp, more) {
      // R20: the span ends when frame 1 is COMMITTED (rAF -> style/layout/paint -> the next task).
      // The old second rAF only added one dead frame period. js_ms = the action's own JS + awaited
      // IPC, vsync_ms = idle wait for the next frame tick (display cadence, not app cost),
      // layout_ms = frame 1 style/layout/paint. scripts/otel-flat.sh reports ms = wall - vsync_ms
      // (the app-attributable latency the lag budgets govern) and keeps wall_ms.
      const tj = performance.now();
      if (sp && sp.t1 < 0) sp.attrs.js_ms = Math.round((tj - sp.t0) * 100) / 100;
      return new Promise(res => requestAnimationFrame(() => {
        const tr = performance.now();
        if (sp) sp.attrs.vsync_ms = Math.round((tr - tj) * 100) / 100;
        setTimeout(() => {
          if (sp) { const tl = performance.now(); sp.attrs.layout_ms = Math.round((tl - tr) * 100) / 100; sp.attrs.wall_ms = Math.round((tl - sp.t0) * 100) / 100; }
          otel.end(sp, more); res();
        }, 0);
      }));
    },
    span(name, attrs = {}, ms = 0) {
      if (on === false) return;
      const t1 = performance.now(), parent = stack[stack.length - 1];
      push({ name, attrs, t0: t1 - ms, t1, traceId: parent ? parent.traceId : traceId(), spanId: spanId(),
             parentSpanId: parent ? parent.spanId : "" });
    },
    flush() {
      if (timer) { clearTimeout(timer); timer = null; }
      if (!buf.length || on === false || flushing || !window.__TAURI__) return;
      const spans = buf.splice(0, buf.length);
      flushing = true;
      window.__TAURI__.core.invoke("log_spans", { spans })
        .then(en => { on = !!en; if (!on) buf.length = 0; })
        .catch(() => {})
        .finally(() => { flushing = false; if (buf.length) arm(); });
    },
  };
  const origin = performance.timeOrigin || (Date.now() - performance.now());
  function push(sp) {
    // unix ms with the sub-ms fraction of performance.now(); Rust turns them into UnixNano
    buf.push({ name: sp.name, traceId: sp.traceId, spanId: sp.spanId, parentSpanId: sp.parentSpanId,
               startMs: origin + sp.t0, endMs: origin + sp.t1, attrs: sp.attrs });
    if (buf.length >= 256) otel.flush(); else arm();
  }
  function arm() { if (!timer) timer = setTimeout(otel.flush, 250); }
  window.addEventListener("pagehide", otel.flush);
  window.otel = otel;
})();
