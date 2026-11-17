// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* #136 the core Audio recorder's capture page (recorder.html, hidden "rec-<window>" webview).
   The app window's microphone gate denies EVERY capture request (a plugin shares that page and WebKit
   hands a page's cached grant to any later request of the same frame without asking the gate), so the
   recording runs HERE, in a page of its own that only Rust creates and only this script runs in:
     1. audio_rec_go       — returns once the native gate of this webview is connected (and armed)
     2. getUserMedia({audio}) + MediaRecorder(audio/webm;codecs=opus), 1 s chunks
     3. audio_rec_started  — reports ok / "<ErrorName>: <message>" to the app window's start; on ok it
                             returns only when the app window asks to stop
     4. audio_rec_data     — the whole recording (base64 webm, "" = nothing recorded); Rust saves it
                             (audio_save rules) and closes this webview. */
(async () => {
  const T = window.__TAURI__, inv = (c, a) => T.core.invoke(c, a);
  const MIME = "audio/webm;codecs=opus";
  const fail = (n, m) => inv("audio_rec_started", { ok: false, err: n + ": " + m }).catch(() => {});
  const log = m => inv("audio_rec_log", { m }).catch(() => {});
  log("load: vis=" + document.visibilityState + " md=" + !!navigator.mediaDevices);
  try { await inv("audio_rec_go"); } catch (e) { return fail("NotReadableError", String((e && e.message) || e)); }
  const md = navigator.mediaDevices, MR = window.MediaRecorder;
  log("go: vis=" + document.visibilityState + " md=" + !!md + " mr=" + (typeof MR));
  // media capture is turned on by the gate (audiorec::install) and may come after this page's first load:
  // a page without navigator.mediaDevices reloads ONCE (the session survives a reload)
  if (!md && !sessionStorage.getItem("rec.reloaded")) { sessionStorage.setItem("rec.reloaded", "1"); return location.reload(); }
  if (!md || typeof md.getUserMedia !== "function" || typeof MR !== "function") return fail("NotSupportedError", "this window has no media capture.");
  if (!(typeof MR.isTypeSupported === "function" && MR.isTypeSupported(MIME))) return fail("NotSupportedError", "GStreamer opusenc/webmmux missing.");
  let stream, r;
  log("getUserMedia");
  try { stream = await md.getUserMedia({ audio: true, video: false }); }
  catch (e) { return fail((e && e.name) || "Error", (e && e.message) || String(e)); }
  const end = () => { for (const t of stream.getTracks()) t.stop(); };
  if (!stream.getAudioTracks().length || stream.getVideoTracks().length) { end(); return fail("NotFoundError", "no microphone track"); }   // never anything but a mic
  try { r = new MR(stream, { mimeType: MIME }); }
  catch (e) { end(); return fail("NotSupportedError", (e && e.message) || String(e)); }
  const chunks = [];
  r.addEventListener("dataavailable", ev => { if (ev.data && ev.data.size) chunks.push(ev.data); });
  r.start(1000);
  log("recording: " + r.state);
  try { await inv("audio_rec_started", { ok: true, err: "" }); } catch (_) {}   // returns when the app window stops the recording
  // an already-stopped recorder (a track ended) fired its stop before: do not wait for it; 5 s cap either way
  const done = r.state === "inactive" ? Promise.resolve() : new Promise(res => { r.addEventListener("stop", res, { once: true }); setTimeout(res, 5000); });
  try { if (r.state !== "inactive") r.stop(); } catch (_) {}
  await done;
  end();
  const u8 = new Uint8Array(await new Blob(chunks, { type: "audio/webm" }).arrayBuffer());
  let s = "";
  for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
  await inv("audio_rec_data", { data: u8.length ? btoa(s) : "" }).catch(() => {});
})();
