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
       grant; the core then fails loudly (its own request is denied) and the
       recording does not start.
     - fake microphone for the test box: WebKit's mock capture devices, ONLY in a
       debug build (cfg(debug_assertions)) and ONLY with OPENSIDIAN_MOCK_CAPTURE=1.
       A release build cannot turn it on. A mock CAMERA is never granted either:
       the video rule above is independent of the mock.

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

/// a page (re)load started in `label`: the old page's token and arm are void
pub fn page_started(label: &str) {
    with_slot(label, |s| *s = Slot::default());
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

/// arm the next audio-only request of `label` for ARM_WINDOW
pub fn arm(label: &str, tok: &str, now: Instant) -> Result<(), String> {
    with_slot(label, |s| {
        check(s, tok)?;
        s.armed = Some(now + ARM_WINDOW);
        Ok(())
    })
}

pub fn disarm(label: &str, tok: &str) -> Result<(), String> {
    with_slot(label, |s| {
        check(s, tok)?;
        s.armed = None;
        Ok(())
    })
}

/// is this a token the window handed out (audio_save)
pub fn authorized(label: &str, tok: &str) -> Result<(), String> {
    with_slot(label, |s| check(s, tok))
}

/// what the gate answers a user-media request with
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Verdict {
    Allow,
    Deny,
}

/// THE rule. audio && !video && !display && armed (consumed) -> Allow; else Deny.
pub fn decide(label: &str, audio: bool, video: bool, display: bool, now: Instant) -> Verdict {
    if !audio || video || display {
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
    });
    if let Err(e) = r {
        eprintln!("[audiorec] gate not installed: {e}");
    }
}

#[cfg(not(target_os = "linux"))]
pub fn install<R: tauri::Runtime>(_: &tauri::WebviewWindow<R>) {}

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
pub fn save(
    root: &std::path::Path,
    note: Option<&str>,
    bytes: &[u8],
    stamp: &str,
    attach_cfg: &str,
    md_links: bool,
    taken: &dyn Fn(&str) -> bool,
) -> Result<crate::pasteimg::Pasted, String> {
    check_bytes(bytes)?;
    if stamp.len() != 14 || !stamp.bytes().all(|b| b.is_ascii_digit()) {
        return Err("invalid clock".into());
    }
    let note = note.filter(|n| !n.is_empty());
    crate::pasteimg::write_into(root, note, &format!("Recording {stamp}"), "webm", bytes, attach_cfg, md_links, taken).map_err(|e| match e {
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
        assert_eq!(decide(w, true, false, false, now), Verdict::Deny);
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
        assert_eq!(decide(w, false, true, false, now), Verdict::Deny);
        assert_eq!(decide(w, true, true, false, now), Verdict::Deny);
        assert_eq!(decide(w, true, false, true, now), Verdict::Deny);
        assert_eq!(decide(w, false, false, false, now), Verdict::Deny);
        // the audio-only request while armed: allowed ONCE
        assert_eq!(decide(w, true, false, false, now), Verdict::Allow);
        assert_eq!(decide(w, true, false, false, now), Verdict::Deny, "the arm is one-shot");
        // disarm needs the token too
        arm(w, &t, now).unwrap();
        assert!(disarm(w, "nope").is_err());
        disarm(w, &t).unwrap();
        assert_eq!(decide(w, true, false, false, now), Verdict::Deny);
    }

    #[test]
    fn audiorec_arm_expires_and_page_load_voids() {
        let w = "t-exp";
        let now = Instant::now();
        page_started(w);
        let t = token(w).unwrap();
        arm(w, &t, now).unwrap();
        assert_eq!(decide(w, true, false, false, now + ARM_WINDOW + Duration::from_millis(1)), Verdict::Deny);
        // a page load: old token and arm void, a new token can be taken once
        arm(w, &t, now).unwrap();
        page_started(w);
        assert_eq!(decide(w, true, false, false, now), Verdict::Deny);
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
}
