// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* #116 Audio recorder — the native half: the microphone gate and the save.

   Stock 1.13.7's core plugin "audio-recorder" records with getUserMedia +
   MediaRecorder and saves `Recording <YYYYMMDDHHmmss>.webm` as an attachment.
   On WebKitGTK that needs two things the webview does not do by default:
   `enable-media-stream` (off -> navigator.mediaDevices is absent) and an
   answer to the `permission-request` signal (unhandled -> denied). wry
   connects no handler, so before this module NO page script could open a mic.

   THE GATE (security first — the hook must not widen the webview beyond the mic,
   and the mic only while a recording the user started is starting):
     - `enable-media-stream` is turned on, nothing else (WebRTC stays off; no
       camera/screen setting is touched).
     - permission-request: a UserMediaPermissionRequest is ALLOWED only if it is
       for an audio device, NOT for a video device, NOT for a display (screen)
       device, AND this window is ARMED. Everything else that is a user-media
       or device-info request is DENIED explicitly; every other request type
       (geolocation, notifications, clipboard, ...) returns false = WebKit's
       default handler, which denies — i.e. exactly today's behaviour.
     - ARMED = a one-shot deadline (ARM_WINDOW) set by `audio_arm(token)` and
       CONSUMED by the first matching request, cleared by `audio_disarm` and by a
       page load. A plugin's getUserMedia outside that window is denied.
     - token: 128 random bits per window per page load, handed out ONCE by
       `audio_token` — the core recorder (ui/main.js) takes it at boot, before
       the plugin loader's seal and before any plugin script exists, and keeps it
       in a closure. A plugin calling audio_arm / audio_save has no token.
       Residual (documented in progress.md / security notes): plugins share the
       page realm, so a plugin racing its own getUserMedia into the few ms
       between the core's arm and the core's request could take the one-shot
       grant; the core then fails loudly (its own request is denied), the
       recording does not start, and the core calls `audio_end` -> mic_off: the
       raced stream ends at once (codeant C1 on #124). `audio_end` also runs
       when every recording stops, so no page mic outlives a recording.
     - fake microphone for the test box: WebKit's mock capture devices, ONLY in a
       debug build (cfg(debug_assertions)) and ONLY with OPENSIDIAN_MOCK_CAPTURE=1.
       A release build cannot turn it on. A mock CAMERA is never granted either:
       the video rule above is independent of the mock.

   #136 EXCLUSIVE MIC (supersedes the arm-window residual above): while a capture is live, WebKit
   answers a request of the SAME frame + origin from its grant cache WITHOUT emitting
   permission-request, and the request names no frame/origin a gate could tell apart — so inside the
   app page a plugin's getUserMedia during a recording got the recorder's grant. Now the core recorder
   captures in a page of its own: a webview "rec-<window>" (REC_PREFIX; its window hidden, the
   webview itself 1 px and transparent inside the app window, see rec_offscreen) loading
   ui/recorder.html, created by Rust per recording (audio_rec_start, token-checked) and closed when it
   ends. That webview has its own grant cache and its own gate; no plugin loader and no vault content
   run in it. decide() DENIES every user-media request of any other webview: the app window's gate
   never allows, armed or not. The arm (one-shot, ARM_WINDOW) arms the recorder webview only.
   THE SAVE (`audio_save`): the webm bytes (base64 over the JSON IPC), capped,
   EBML magic checked, written through pasteimg::write_into — the same DEST/NO OVER
   rules as a pasted image (safe attachment folder inside the vault, O_NOFOLLOW per
   component, create_new: a taken name or planted symlink is skipped to ` <n>`). */
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// how long an arm waits for the core recorder's getUserMedia to reach the gate
pub const ARM_WINDOW: Duration = Duration::from_secs(10);
/// the recording cap (webm/opus at ~32-64 kbit/s: many hours); checked on the
/// base64 length before decoding and on the bytes
pub const MAX_REC_BYTES: u64 = 128 * 1024 * 1024;
/// the WebM (EBML) magic every recording starts with
pub const WEBM_MAGIC: &[u8] = b"\x1a\x45\xdf\xa3";
/// env switch for WebKit's mock microphone (debug builds only)
pub const MOCK_ENV: &str = "OPENSIDIAN_MOCK_CAPTURE";

#[derive(Default)]
struct Slot {
    /// the per-page-load secret; None after a page load until minted
    token: Option<String>,
    /// handed out already (to the first caller of audio_token)
    taken: bool,
    /// armed until this instant (one-shot)
    armed: Option<Instant>,
}

/// window label -> its slot
static SLOTS: Mutex<Option<HashMap<String, Slot>>> = Mutex::new(None);

fn with_slot<T>(label: &str, f: impl FnOnce(&mut Slot) -> T) -> T {
    let mut g = SLOTS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new).entry(label.to_string()).or_default())
}

fn mint() -> Option<String> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom").ok()?.read_exact(&mut b).ok()?;
    Some(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// #136: the label prefix of the core recorder's own (hidden) webview
pub const REC_PREFIX: &str = "rec-";
/// the recorder webview of app window `parent`
pub fn rec_label(parent: &str) -> String {
    format!("{REC_PREFIX}{parent}")
}
/// is `label` a recorder webview (the only kind the gate may allow)
pub fn is_rec(label: &str) -> bool {
    label.starts_with(REC_PREFIX)
}

/// a page (re)load started in `label`: the old page's token and arm are void
/// (an app window's reload also voids the arm of its recorder webview)
pub fn page_started(label: &str) {
    with_slot(label, |s| *s = Slot::default());
    if !is_rec(label) {
        with_slot(&rec_label(label), |s| s.armed = None);
        grant_drop(label); // #295: an Allow given to the old page never starts the new one
    }
}

/// the token, to the FIRST caller per page load only
pub fn token(label: &str) -> Result<String, String> {
    with_slot(label, |s| {
        if s.taken {
            return Err("audio recorder: token already taken".into());
        }
        let t = mint().ok_or("audio recorder: no randomness")?;
        s.token = Some(t.clone());
        s.taken = true;
        Ok(t)
    })
}

fn check(s: &Slot, tok: &str) -> Result<(), String> {
    match &s.token {
        // constant-time enough for a 32-hex secret compared in-process: length first, then every byte
        Some(t) if t.len() == tok.len() && t.bytes().zip(tok.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0 => Ok(()),
        _ => Err("audio recorder: not allowed".into()),
    }
}

/// arm the next audio-only request of app window `label`'s RECORDER webview
/// (rec_label) for ARM_WINDOW; the token is the app window's
pub fn arm(label: &str, tok: &str, now: Instant) -> Result<(), String> {
    with_slot(label, |s| check(s, tok))?;
    with_slot(&rec_label(label), |s| s.armed = Some(now + ARM_WINDOW));
    Ok(())
}

pub fn disarm(label: &str, tok: &str) -> Result<(), String> {
    with_slot(label, |s| check(s, tok))?;
    with_slot(&rec_label(label), |s| s.armed = None);
    Ok(())
}

/* #136 the recording session of an app window (keyed by its label): the hand-offs between the app
   window's audio_rec_start / audio_rec_stop and the recorder page's audio_rec_go / _started / _data. */
pub struct Sess {
    /// the recorder webview's gate is connected (install's callback)
    pub gated: Option<std::sync::mpsc::Receiver<()>>,
    /// the gate was connected (a reloaded recorder page need not wait again)
    pub ready: bool,
    /// the recorder page's start outcome -> audio_rec_start
    pub started: Option<std::sync::mpsc::Sender<Result<(), String>>>,
    /// audio_rec_stop -> the recorder page (its audio_rec_started returns)
    pub stop_tx: Option<std::sync::mpsc::Sender<()>>,
    pub stop_rx: Option<std::sync::mpsc::Receiver<()>>,
    /// the recorder page's recording (base64) -> audio_rec_stop
    pub data: Option<std::sync::mpsc::Sender<String>>,
}
static SESS: Mutex<Option<HashMap<String, Sess>>> = Mutex::new(None);
/// run `f` on the session of app window `parent` (None: no recording session)
pub fn with_sess<T>(parent: &str, f: impl FnOnce(Option<&mut Sess>) -> T) -> T {
    let mut g = SESS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(HashMap::new).get_mut(parent))
}
pub fn sess_put(parent: &str, s: Sess) {
    SESS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(parent.to_string(), s);
}
pub fn sess_drop(parent: &str) {
    if let Some(m) = SESS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(parent);
    }
}
/// the app window a recorder webview belongs to (None: `label` is not a recorder webview)
pub fn parent_of(label: &str) -> Option<&str> {
    label.strip_prefix(REC_PREFIX).filter(|p| !p.is_empty())
}

/// is this a token the window handed out (audio_save)
pub fn authorized(label: &str, tok: &str) -> Result<(), String> {
    with_slot(label, |s| check(s, tok))
}

/* #295 (audit F-8) THE START DECISION LIVES OUTSIDE THE PAGE. The token only proves "the core
   recorder of this page load asks"; plugins share that page realm and can make the core ask
   (its ribbon / command / any page function). So a token is no longer enough to open the mic:
   every recording start needs a fresh GRANT, and a grant exists only after the user pressed
   Record in the NATIVE consent (consent::Kind::Mic, asked by main.rs audio_rec_start; Settings ›
   Security › Microphone "off" refuses before any dialog, lockdown forces "off").
     - one grant per app window, minted by `grant` right after the native Allow, valid GRANT_TTL;
     - `start_check` (the gate audio_rec_start passes before it creates the recorder webview):
       valid token AND the Microphone setting not off AND a fresh grant, which it CONSUMES — so a
       grant opens exactly one start; no grant, an expired or a used one = NotAllowedError, even
       with a valid token (fail closed);
     - a page (re)load voids the window's grant (page_started).
   No env var, cfg flag or test hook mints a grant: `grant` is called from audio_rec_start only,
   after consent::ask returned Ok(true) (main.rs T0 pins the order). */
/// how long a native Allow stays good for the start it was asked for
pub const GRANT_TTL: Duration = Duration::from_secs(10);
/// the refusal a start without a fresh native grant gets (the page maps NotAllowedError to
/// "Microphone access was denied.")
pub const NO_GRANT: &str = "NotAllowedError: audio recorder: the recording was not allowed in the native consent";

/// app window label -> the deadline of its one-shot grant
static GRANTS: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

/// the user pressed Record in the native consent for app window `parent` (main.rs audio_rec_start only)
pub fn grant(parent: &str, now: Instant) {
    GRANTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(parent.to_string(), now + GRANT_TTL);
}

/// take (consume) the grant of `parent`: Ok only for one that exists and has not expired;
/// an expired one is consumed too
pub fn take_grant(parent: &str, now: Instant) -> Result<(), String> {
    match GRANTS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).remove(parent) {
        Some(t) if now <= t => Ok(()),
        _ => Err(NO_GRANT.into()),
    }
}

/// a page (re)load: the old page's grant is void
fn grant_drop(parent: &str) {
    if let Some(m) = GRANTS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.remove(parent);
    }
}

/// THE start gate: the fresh native grant (taken = consumed FIRST, so every start attempt burns
/// it, refused or not), the page's token, the effective Microphone setting (`mic` =
/// security::mic_check of it). Ok only when all three hold; every Err is a refusal (nothing is
/// created).
pub fn start_check(parent: &str, tok: &str, mic: Result<(), String>, now: Instant) -> Result<(), String> {
    let g = take_grant(parent, now);
    authorized(parent, tok)?;
    mic?;
    g
}

/* #295 the recording indicator the page cannot hide: while app window <label> records, its NATIVE
   window title starts with REC_MARK. main.rs rec_indicator keeps it there (Linux: a GTK
   notify::title handler re-applies it on every title change, so the page's own setTitle cannot
   drop it; macOS: re-applied every REC_MARK_TICK, next to the system's own microphone-in-use
   indicator). At the START because a long title is cut at its end. */
pub const REC_MARK: &str = "● Recording — ";
/// macOS / other: how often the title mark is re-applied while recording
#[cfg_attr(target_os = "linux", allow(dead_code))] // the Linux mark rides a GTK notify::title handler instead
pub const REC_MARK_TICK: Duration = Duration::from_millis(250);

/// the title the window must carry: Some(new) when `cur` is not already it. On = exactly one
/// REC_MARK, first; off = no REC_MARK anywhere (a page cannot leave a fake one either)
pub fn indicator_title(cur: &str, on: bool) -> Option<String> {
    let bare = cur.replace(REC_MARK, "");
    let want = if on { format!("{REC_MARK}{bare}") } else { bare };
    (want != cur).then_some(want)
}

/// app window label -> the generation of its current recording indicator (absent = off)
static INDICATOR: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
static INDICATOR_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// turn the indicator state of `parent` on (Some(generation)) or off (None)
pub fn indicator_set(parent: &str, on: bool) -> Option<u64> {
    let mut g = INDICATOR.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    if on {
        let n = INDICATOR_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        m.insert(parent.to_string(), n);
        Some(n)
    } else {
        m.remove(parent);
        None
    }
}

/// is the indicator generation `generation` of `parent` still the live one
#[cfg_attr(target_os = "linux", allow(dead_code))] // the macOS / other ticker only
pub fn indicator_live(parent: &str, generation: u64) -> bool {
    INDICATOR.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(parent)).copied() == Some(generation)
}

/// is app window `parent` recording (its indicator on)
pub fn indicator_on(parent: &str) -> bool {
    INDICATOR.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.contains_key(parent))
}

/// what the gate answers a user-media request with
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Verdict {
    Allow,
    Deny,
}

/// THE rule. a recorder webview && audio && !video && !display && armed (consumed) -> Allow; else Deny.
/// #136: an app window (plugins share its page) is NEVER allowed, armed or not.
pub fn decide(label: &str, audio: bool, video: bool, display: bool, now: Instant) -> Verdict {
    if !is_rec(label) || !audio || video || display {
        return Verdict::Deny; // never consumes the arm: a camera/screen probe cannot burn it either
    }
    with_slot(label, |s| match s.armed.take() {
        Some(t) if now <= t => Verdict::Allow,
        _ => Verdict::Deny,
    })
}

/// the mock microphone is on: debug build AND OPENSIDIAN_MOCK_CAPTURE=1
pub fn mock_on(env: Option<&str>) -> bool {
    cfg!(debug_assertions) && env == Some("1")
}

/// install the gate on the window's webview (Linux / WebKitGTK)
#[cfg(target_os = "linux")]
pub fn install<R: tauri::Runtime>(w: &tauri::WebviewWindow<R>) {
    install_then(w, || {});
}

/// install, then `done` once the handler is connected (on the main thread; #136: the recorder page
/// waits for it before its getUserMedia, so the request can never reach WebKit's default deny)
#[cfg(target_os = "linux")]
pub fn install_then<R: tauri::Runtime>(w: &tauri::WebviewWindow<R>, done: impl FnOnce() + Send + 'static) {
    let label = w.label().to_string();
    let mock = mock_on(std::env::var(MOCK_ENV).ok().as_deref());
    let r = w.with_webview(move |pw| {
        use webkit2gtk::{PermissionRequestExt, SettingsExt, UserMediaPermissionRequestExt, WebViewExt};
        use gtk::glib::{translate::ToGlibPtr, Cast, ObjectExt};
        let wv = pw.inner();
        if let Some(st) = WebViewExt::settings(&wv) {
            st.set_enable_media_stream(true);
            if mock {
                st.set_enable_mock_capture_devices(true);
                eprintln!("[audiorec] mock capture devices ON ({MOCK_ENV}=1, debug build)");
            }
        }
        wv.connect_permission_request(move |_, req| {
            if let Some(um) = req.downcast_ref::<webkit2gtk::UserMediaPermissionRequest>() {
                let raw: *mut webkit2gtk_sys::WebKitUserMediaPermissionRequest = um.to_glib_none().0;
                // SAFETY: `raw` is the live request object, borrowed for the duration of the signal handler
                let display = unsafe {
                    webkit2gtk_sys::webkit_user_media_permission_is_for_display_device(raw) != 0
                };
                let (a, v) = (um.is_for_audio_device(), um.is_for_video_device());
                let verdict = decide(&label, a, v, display, Instant::now());
                eprintln!("[audiorec] user-media request audio={a} video={v} display={display} -> {verdict:?}");
                match verdict {
                    Verdict::Allow => req.allow(),
                    Verdict::Deny => req.deny(),
                }
                return true;
            }
            if req.is::<webkit2gtk::DeviceInfoPermissionRequest>() {
                // enumerateDevices labels: not needed to record, never given
                req.deny();
                return true;
            }
            false // every other request: WebKit's default handler (deny), unchanged
        });
        done();
    });
    if let Err(e) = r {
        eprintln!("[audiorec] gate not installed: {e}");
    }
}

/* #136 the recorder webview must be IN an on-screen window for WebKit: a page that is not in a window
   (hidden window, or a GtkOffscreenWindow, which WebKitGTK does not count as on screen) never gets
   media started — its getUserMedia waits forever (WebPage canStartMedia). So the recorder's webview is
   moved into the APP window it records for: a 1 x 1 px, fully transparent strip at the bottom of the
   window's box, for the length of the recording. Its own window stays hidden (no taskbar entry, no
   focus). It is still its own page: its own capture grant and its own permission-request signal.
   rec_offscreen_drop takes it out again and destroys it with the recording. Main thread only (GTK),
   hence the thread_local; both hand-offs run on the GTK main loop in the order they were queued. */
#[cfg(target_os = "linux")]
thread_local! {
    static EMBED: std::cell::RefCell<HashMap<String, gtk::Widget>> = std::cell::RefCell::new(HashMap::new());
}

#[cfg(target_os = "linux")]
fn embed_drop(label: &str) {
    use gtk::prelude::{ContainerExt, WidgetExt, WidgetExtManual};
    use gtk::glib::Cast;
    if let Some(w) = EMBED.with(|o| o.borrow_mut().remove(label)) {
        if let Some(p) = w.parent() {
            if let Ok(c) = p.downcast::<gtk::Container>() {
                c.remove(&w);
            }
        }
        // SAFETY: GTK main thread (with_webview); the recorder webview is ours and its recording is over
        unsafe { w.destroy() }
    }
}

#[cfg(target_os = "linux")]
pub fn rec_offscreen<R: tauri::Runtime>(w: &tauri::WebviewWindow<R>, app_win: &tauri::WebviewWindow<R>) {
    let label = w.label().to_string();
    let l2 = label.clone();
    let r = w.with_webview(move |pw| {
        use gtk::prelude::{ContainerExt, WidgetExt};
        use gtk::glib::Cast;
        let widget: gtk::Widget = pw.inner().upcast();
        if let Some(p) = widget.parent() {
            if let Ok(c) = p.downcast::<gtk::Container>() {
                c.remove(&widget);
            }
        }
        embed_drop(&label); // a stale recorder of the same label
        EMBED.with(|o| o.borrow_mut().insert(label, widget));
    });
    if let Err(e) = r {
        eprintln!("[audiorec] recorder webview not detached: {e}");
        return;
    }
    let r = app_win.with_webview(move |pw| {
        use gtk::prelude::{BoxExt, ContainerExt, WidgetExt};
        use gtk::glib::Cast;
        let Some(widget) = EMBED.with(|o| o.borrow().get(&l2).cloned()) else { return };
        let main: gtk::Widget = pw.inner().upcast();
        let Some(host) = main.parent() else {
            eprintln!("[audiorec] {l2}: app webview has no parent box");
            return;
        };
        widget.set_size_request(1, 1);
        widget.set_opacity(0.0);
        widget.set_can_focus(false);
        match host.clone().downcast::<gtk::Box>() {
            Ok(b) => b.pack_end(&widget, false, false, 0),
            Err(h) => {
                if let Ok(c) = h.downcast::<gtk::Container>() {
                    c.add(&widget)
                }
            }
        }
        widget.show_all();
        eprintln!("[audiorec] {l2}: capture page in the app window (mapped={})", widget.is_mapped());
    });
    if let Err(e) = r {
        eprintln!("[audiorec] recorder webview not embedded: {e}");
    }
}

#[cfg(target_os = "linux")]
pub fn rec_offscreen_drop<R: tauri::Runtime>(w: &tauri::WebviewWindow<R>) {
    let label = w.label().to_string();
    let _ = w.with_webview(move |_| embed_drop(&label)); // reason: a webview that is already gone has nothing left to drop
}

#[cfg(not(target_os = "linux"))]
pub fn rec_offscreen<R: tauri::Runtime>(_: &tauri::WebviewWindow<R>, _: &tauri::WebviewWindow<R>) {}
#[cfg(target_os = "macos")]
pub fn rec_offscreen_drop<R: tauri::Runtime>(w: &tauri::WebviewWindow<R>) {
    let _ = w.with_webview(|pw| crate::macos::media_gate_forget(pw.inner()));
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn rec_offscreen_drop<R: tauri::Runtime>(_: &tauri::WebviewWindow<R>) {}

/// the kill switch (codeant C1 on #124): every microphone track of the window's
/// page stops (WebKit's microphone capture state -> NONE). The core calls it
/// through `audio_end` when a recording stops and when its own request was
/// denied after it armed (= another script of the page took the one-shot grant):
/// a stream a plugin raced out of the arm window dies at once, and no mic
/// outlives a recording.
#[cfg(target_os = "linux")]
pub fn mic_off<R: tauri::Runtime>(w: &tauri::WebviewWindow<R>) -> Result<(), String> {
    w.with_webview(|pw| {
        use webkit2gtk::{MediaCaptureState, WebViewExt};
        let wv = pw.inner();
        if wv.microphone_capture_state() != MediaCaptureState::None {
            wv.set_microphone_capture_state(MediaCaptureState::None);
            eprintln!("[audiorec] microphone capture of the page stopped");
        }
    })
    .map_err(|e| format!("audio recorder: {e}"))
}

/// macOS: the same gate on wry's WKUIDelegate (macos::media_gate) — registered for this window's
/// webview; the app window is registered too, so its requests meet the rule (#136: always Deny)
#[cfg(target_os = "macos")]
pub fn install<R: tauri::Runtime>(w: &tauri::WebviewWindow<R>) {
    install_then(w, || {});
}
#[cfg(target_os = "macos")]
pub fn install_then<R: tauri::Runtime>(w: &tauri::WebviewWindow<R>, done: impl FnOnce() + Send + 'static) {
    let label = w.label().to_string();
    let r = w.with_webview(move |pw| {
        crate::macos::media_gate(pw.inner(), &label, |l, a, v| decide(l, a, v, false, Instant::now()) == Verdict::Allow);
        done();
    });
    if let Err(e) = r {
        eprintln!("[audiorec] gate not installed: {e}");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn install<R: tauri::Runtime>(_: &tauri::WebviewWindow<R>) {}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn install_then<R: tauri::Runtime>(_: &tauri::WebviewWindow<R>, done: impl FnOnce() + Send + 'static) {
    done();
}

#[cfg(not(target_os = "linux"))]
pub fn mic_off<R: tauri::Runtime>(_: &tauri::WebviewWindow<R>) -> Result<(), String> {
    Ok(())
}

/// why a save was refused (beyond write_into's own)
pub fn check_bytes(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("the recording is empty".into());
    }
    if bytes.len() as u64 > MAX_REC_BYTES {
        return Err(format!("the recording ({} MB) is over the {} MB limit", bytes.len() as u64 / 1048576, MAX_REC_BYTES / 1048576));
    }
    if !bytes.starts_with(WEBM_MAGIC) {
        return Err("the recording is not a WebM file".into());
    }
    Ok(())
}

/// the recording -> `Recording <stamp>.webm` in the attachment folder of `note`
/// (None/"" = no note open: the folder's "./" forms resolve against the root)
#[cfg(test)]
pub fn save(
    root: &std::path::Path,
    note: Option<&str>,
    bytes: &[u8],
    stamp: &str,
    attach_cfg: &str,
    md_links: bool,
    taken: &dyn Fn(&str) -> bool,
) -> Result<crate::pasteimg::Pasted, String> {
    save_owned(root, note, bytes.to_vec(), stamp, attach_cfg, md_links, taken)
}

/// [`save`] on the decoded upload itself: the #197 Duration is written into `bytes`
/// in place, no second full-size copy next to it (codeant Major on #226)
pub fn save_owned(
    root: &std::path::Path,
    note: Option<&str>,
    mut bytes: Vec<u8>,
    stamp: &str,
    attach_cfg: &str,
    md_links: bool,
    taken: &dyn Fn(&str) -> bool,
) -> Result<crate::pasteimg::Pasted, String> {
    check_bytes(&bytes)?;
    if stamp.len() != 14 || !stamp.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid clock".into());
    }
    let note = note.filter(|n| !n.is_empty());
    // #197: MediaRecorder's live WebM has no Duration; write it now that the whole file is known
    crate::webmdur::add_duration(&mut bytes);
    crate::pasteimg::write_into(root, note, &format!("Recording {stamp}"), "webm", &bytes, attach_cfg, md_links, taken).map_err(|e| match e {
        crate::pasteimg::PasteErr::NoNote => "the note the recording belongs to is gone".into(),
        crate::pasteimg::PasteErr::Io(m) => format!("the recording could not be saved ({m})"),
        e => e.say(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    const WEBM: &[u8] = b"\x1a\x45\xdf\xa3\x9f\x42\x86\x81\x01webm-test";

    fn vault(tag: &str) -> PathBuf {
        let r = std::env::temp_dir().join(format!("opensidian-audiorec-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&r); // reason: a leftover from an earlier run, absent is fine
        fs::create_dir_all(r.join("dir")).unwrap();
        fs::write(r.join("N.md"), "# N\n").unwrap();
        fs::write(r.join("dir/M.md"), "# M\n").unwrap();
        r
    }
    fn none(_: &str) -> bool {
        false
    }

    #[test]
    fn audiorec_gate_needs_token_arm_and_audio_only() {
        let w = "t-gate";
        let now = Instant::now();
        page_started(w);
        // not armed: an audio request (a plugin's getUserMedia) is denied
        assert_eq!(decide(&rec_label(w), true, false, false, now), Verdict::Deny);
        // no token yet / wrong token: arm refused
        assert!(arm(w, "", now).is_err());
        let t = token(w).unwrap();
        assert_eq!(t.len(), 32);
        // the token goes to the FIRST caller only
        assert!(token(w).is_err());
        assert!(arm(w, "00000000000000000000000000000000", now).is_err());
        assert!(arm(w, &t[..31], now).is_err());
        arm(w, &t, now).unwrap();
        // camera, camera+mic, screen: denied even while armed, and they do NOT burn the arm
        assert_eq!(decide(&rec_label(w), false, true, false, now), Verdict::Deny);
        assert_eq!(decide(&rec_label(w), true, true, false, now), Verdict::Deny);
        assert_eq!(decide(&rec_label(w), true, false, true, now), Verdict::Deny);
        assert_eq!(decide(&rec_label(w), false, false, false, now), Verdict::Deny);
        // the audio-only request while armed: allowed ONCE
        assert_eq!(decide(&rec_label(w), true, false, false, now), Verdict::Allow);
        assert_eq!(decide(&rec_label(w), true, false, false, now), Verdict::Deny, "the arm is one-shot");
        // disarm needs the token too
        arm(w, &t, now).unwrap();
        assert!(disarm(w, "nope").is_err());
        disarm(w, &t).unwrap();
        assert_eq!(decide(&rec_label(w), true, false, false, now), Verdict::Deny);
    }

    // #136: the app window (plugins share its page) is never granted the mic, armed or not; only its
    // recorder webview is, and arming one window's recorder never arms another's
    #[test]
    fn audiorec_app_window_never_allowed_only_its_recorder() {
        let w = "t-excl";
        let now = Instant::now();
        page_started(w);
        let t = token(w).unwrap();
        arm(w, &t, now).unwrap();
        assert_eq!(decide(w, true, false, false, now), Verdict::Deny, "the app window itself is never allowed");
        assert_eq!(decide("rec-", true, false, false, now), Verdict::Deny);
        assert_eq!(decide(&rec_label("t-excl2"), true, false, false, now), Verdict::Deny, "another window's recorder is not armed");
        assert_eq!(decide(&rec_label(w), true, false, false, now), Verdict::Allow, "the app window's checks did not burn its recorder's arm");
        assert_eq!(decide(&rec_label(w), true, false, false, now), Verdict::Deny);
        assert!(is_rec("rec-main") && !is_rec("main") && !is_rec("xrec-main"));
        assert_eq!(parent_of("rec-main"), Some("main"));
        assert_eq!(parent_of("rec-"), None);
        assert_eq!(parent_of("main"), None);
        // a reload of the app window voids its recorder's arm
        arm(w, &t, now).unwrap();
        page_started(w);
        assert_eq!(decide(&rec_label(w), true, false, false, now), Verdict::Deny);
    }

    #[test]
    fn audiorec_arm_expires_and_page_load_voids() {
        let w = "t-exp";
        let now = Instant::now();
        page_started(w);
        let t = token(w).unwrap();
        arm(w, &t, now).unwrap();
        assert_eq!(decide(&rec_label(w), true, false, false, now + ARM_WINDOW + Duration::from_millis(1)), Verdict::Deny);
        // a page load: old token and arm void, a new token can be taken once
        arm(w, &t, now).unwrap();
        page_started(w);
        assert_eq!(decide(&rec_label(w), true, false, false, now), Verdict::Deny);
        assert!(arm(w, &t, now).is_err(), "the old page's token is void");
        assert!(authorized(w, &t).is_err());
        let t2 = token(w).unwrap();
        assert_ne!(t, t2);
        authorized(w, &t2).unwrap();
        // windows are separate: another window's token does not arm this one
        page_started("t-other");
        let o = token("t-other").unwrap();
        assert!(arm(w, &o, now).is_err());
    }

    #[test]
    fn audiorec_mock_only_debug_and_env() {
        assert_eq!(mock_on(Some("1")), cfg!(debug_assertions));
        assert!(!mock_on(None));
        assert!(!mock_on(Some("0")));
        assert!(!mock_on(Some("true")));
    }

    #[test]
    fn audiorec_save_stock_name_folder_and_embed() {
        let r = vault("save");
        let p = save(&r, Some("N"), WEBM, "20261115073000", "", false, &none).unwrap();
        assert_eq!(p.path, "Recording 20261115073000.webm");
        assert_eq!(p.text, "![[Recording 20261115073000.webm]]");
        assert_eq!(fs::read(r.join(&p.path)).unwrap(), WEBM);
        // same second: ` 1`, the first file untouched
        let q = save(&r, Some("N"), WEBM, "20261115073000", "", false, &none).unwrap();
        assert_eq!(q.path, "Recording 20261115073000 1.webm");
        // attachment folder forms; no note open -> "./" is the vault root
        assert_eq!(save(&r, Some("dir/M"), WEBM, "20261115073001", "./att", false, &none).unwrap().path, "dir/att/Recording 20261115073001.webm");
        assert_eq!(save(&r, None, WEBM, "20261115073002", "./att", false, &none).unwrap().path, "att/Recording 20261115073002.webm");
        assert_eq!(save(&r, Some(""), WEBM, "20261115073003", "", false, &none).unwrap().path, "Recording 20261115073003.webm");
        let m = save(&r, Some("N"), WEBM, "20261115073004", "", true, &none).unwrap();
        assert_eq!(m.text, "![](Recording%2020261115073004.webm)");
        let _ = fs::remove_dir_all(&r); // reason: test cleanup
    }

    #[test]
    fn audiorec_save_refusals_write_nothing() {
        let r = vault("refuse");
        let count = |r: &Path| fs::read_dir(r).unwrap().count();
        let before = count(&r);
        assert!(save(&r, Some("N"), b"", "20261115073000", "", false, &none).is_err());
        assert!(save(&r, Some("N"), b"RIFF....WAVE", "20261115073000", "", false, &none).is_err(), "not webm");
        assert!(save(&r, Some("N"), WEBM, "../../x/000000", "", false, &none).is_err());
        assert!(save(&r, Some("N"), WEBM, "20261115073000", "../out", false, &none).is_err());
        assert!(save(&r, Some("../N"), WEBM, "20261115073000", "", false, &none).is_err());
        assert!(save(&r, Some("Gone"), WEBM, "20261115073000", "", false, &none).is_err());
        let mut big = WEBM.to_vec();
        big.resize(MAX_REC_BYTES as usize + 1, 0);
        assert!(save(&r, Some("N"), &big, "20261115073000", "", false, &none).is_err());
        assert_eq!(count(&r), before, "a refused save wrote nothing");
        assert!(!r.parent().unwrap().join("out").exists());
        // a symlink planted at the stock name is skipped, never written through
        let out = std::env::temp_dir().join(format!("opensidian-audiorec-out-{}", std::process::id()));
        let _ = fs::remove_dir_all(&out); // reason: leftover
        fs::create_dir_all(&out).unwrap();
        std::os::unix::fs::symlink(out.join("victim.webm"), r.join("Recording 20261115073000.webm")).unwrap();
        assert_eq!(save(&r, Some("N"), WEBM, "20261115073000", "", false, &none).unwrap().path, "Recording 20261115073000 1.webm");
        assert!(!out.join("victim.webm").exists());
        let _ = fs::remove_dir_all(&r); // reason: test cleanup
        let _ = fs::remove_dir_all(&out); // reason: test cleanup
    }

    /* #295 (audit F-8): the start gate. A valid page token is NOT enough: a fresh native grant is needed too */

    /// a valid token without a grant is refused with NotAllowedError (the core recorder driven by a
    /// page script, no native Record pressed)
    #[test]
    fn mic295_valid_token_without_grant_is_not_allowed() {
        let w = "t295-nogrant";
        let now = Instant::now();
        page_started(w);
        let t = token(w).unwrap();
        let e = start_check(w, &t, Ok(()), now).unwrap_err();
        assert!(e.starts_with("NotAllowedError: "), "{e}");
        assert_eq!(e, NO_GRANT);
        // a wrong token is refused too (and never reaches the grant question)
        grant(w, now);
        assert!(start_check(w, "00000000000000000000000000000000", Ok(()), now).is_err());
        // ... and that refused attempt BURNED the grant: the right token now finds none
        assert_eq!(start_check(w, &t, Ok(()), now).unwrap_err(), NO_GRANT, "a refused start must consume the grant");
    }

    /// one native Record = one start: the grant is consumed by the first start_check
    #[test]
    fn mic295_grant_is_single_use() {
        let w = "t295-once";
        let now = Instant::now();
        page_started(w);
        let t = token(w).unwrap();
        grant(w, now);
        start_check(w, &t, Ok(()), now).unwrap();
        assert_eq!(start_check(w, &t, Ok(()), now).unwrap_err(), NO_GRANT, "second start on one grant");
        assert_eq!(take_grant(w, now).unwrap_err(), NO_GRANT);
        // a grant belongs to its window: window A's Record never starts window B
        let o = "t295-once-other";
        page_started(o);
        let to = token(o).unwrap();
        grant(w, now);
        assert_eq!(start_check(o, &to, Ok(()), now).unwrap_err(), NO_GRANT, "a grant crossed windows");
        start_check(w, &t, Ok(()), now).unwrap();
    }

    /// a grant older than GRANT_TTL is refused (and consumed: it cannot be used later either)
    #[test]
    fn mic295_expired_grant_is_refused() {
        let w = "t295-ttl";
        let now = Instant::now();
        page_started(w);
        let t = token(w).unwrap();
        grant(w, now);
        start_check(w, &t, Ok(()), now + GRANT_TTL).expect("at the deadline: still good");
        grant(w, now);
        let late = now + GRANT_TTL + Duration::from_millis(1);
        assert_eq!(start_check(w, &t, Ok(()), late).unwrap_err(), NO_GRANT, "an expired grant opened the mic");
        assert_eq!(start_check(w, &t, Ok(()), now).unwrap_err(), NO_GRANT, "an expired grant was kept");
        assert!(GRANT_TTL <= Duration::from_secs(30), "the grant must be short-lived");
    }

    /// Settings › Security › Microphone off (security::mic_check(Off)) refuses even with a fresh
    /// grant and a valid token, and the grant is consumed
    #[test]
    fn mic295_microphone_off_refuses_even_with_grant() {
        let w = "t295-off";
        let now = Instant::now();
        page_started(w);
        let t = token(w).unwrap();
        grant(w, now);
        let e = start_check(w, &t, crate::security::mic_check(crate::security::Mic::Off), now).unwrap_err();
        assert!(e.starts_with("NotAllowedError: ") && e.contains("(Settings › Security › Microphone)"), "{e}");
        assert_eq!(start_check(w, &t, Ok(()), now).unwrap_err(), NO_GRANT, "the off refusal left the grant usable");
        // lockdown: the effective value is off -> the same refusal
        let mut l = crate::security::Sec::default();
        l.lockdown = true;
        let eff = crate::security::effective_with(l, Some(l));
        grant(w, now);
        let m = crate::security::mic_effective_with(crate::security::Mic::Ask, &eff);
        assert!(start_check(w, &t, crate::security::mic_check(m), now).unwrap_err().contains("the microphone is off"));
    }

    /// a page (re)load voids the window's grant (an Allow given to the old page starts nothing)
    #[test]
    fn mic295_page_load_voids_the_grant() {
        let w = "t295-reload";
        let now = Instant::now();
        page_started(w);
        grant(w, now);
        page_started(w);
        let t = token(w).unwrap();
        assert_eq!(start_check(w, &t, Ok(()), now).unwrap_err(), NO_GRANT);
        // a recorder webview's own load does not touch the app window's grant
        grant(w, now);
        page_started(&rec_label(w));
        start_check(w, &t, Ok(()), now).unwrap();
    }

    /// the native title mark: exactly one, first, while on; none at all while off; no write when
    /// the title already is what it must be
    #[test]
    fn mic295_indicator_title_mark() {
        let t = "Note - Vault - Opensidian [panes:1]";
        let on = indicator_title(t, true).unwrap();
        assert_eq!(on, format!("{REC_MARK}{t}"));
        assert_eq!(indicator_title(&on, true), None, "already marked: no write (no notify loop)");
        assert_eq!(indicator_title(&on, false).as_deref(), Some(t));
        assert_eq!(indicator_title(t, false), None);
        // a page that moves the mark to the end (where a long title is cut), doubles it or fakes it
        let moved = format!("{t}{REC_MARK}");
        assert_eq!(indicator_title(&moved, true).as_deref(), Some(on.as_str()));
        let twice = format!("{REC_MARK}{REC_MARK}{t}");
        assert_eq!(indicator_title(&twice, true).as_deref(), Some(on.as_str()));
        assert_eq!(indicator_title(&moved, false).as_deref(), Some(t), "a fake mark survives the off");
        assert_eq!(indicator_title("", true).as_deref(), Some(REC_MARK));
    }

    /// the indicator generation: on = a new live generation, off = none live, a later on
    /// supersedes an earlier ticker
    #[test]
    fn mic295_indicator_generations() {
        let w = "t295-ind";
        assert!(!indicator_on(w));
        let g1 = indicator_set(w, true).unwrap();
        assert!(indicator_on(w) && indicator_live(w, g1));
        let g2 = indicator_set(w, true).unwrap();
        assert!(!indicator_live(w, g1) && indicator_live(w, g2), "the old ticker must stop");
        assert_eq!(indicator_set(w, false), None);
        assert!(!indicator_on(w) && !indicator_live(w, g2));
    }
}
