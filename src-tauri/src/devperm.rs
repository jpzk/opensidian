// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* #295 follow-up (Settings › Security › Device access): THE one decision for every permission
   request a webview raises — WebKitGTK's `permission-request` signal (audiorec::install_then) and
   the macOS WKUIDelegate media-capture method (macos::gate_imp via audiorec::install_then).

   Every request is classified into a `Kind` by its GObject type name (`kind_of`), then `decide`
   answers it from the Device access rows and lockdown. A type this function does not know is
   `Kind::Unknown` and is DENIED. The GTK handler calls allow() or deny() itself and returns true for
   EVERY request, so WebKit's class default (which ALLOWS a PointerLockPermissionRequest) is never
   reached (source-shape test: devperm_handler_decides_every_request in this file).

   Rows (stock 1.13.7 black box, docs/RESULT of goal devaccess §2): Microphone, Camera, Location,
   Notifications, Pointer lock. Everything else (screen capture, device names, EME, website-data
   access, clipboard permission, WebXR, unknown) has no row and is denied: "not available".
   The microphone keeps its #136 rule (audiorec::decide): only the core recorder's own webview,
   armed by a recording start that already passed the Microphone setting, gets audio; an app
   window never does. The recorder webview gets nothing else. */
use crate::security::Mic as Access;
use std::time::Instant;

/// what a permission request asks for
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Kind {
    /// getUserMedia: audio / video / display (screen) flags as WebKit reports them
    UserMedia { audio: bool, video: bool, display: bool },
    DeviceInfo,
    Geolocation,
    Notification,
    PointerLock,
    Clipboard,
    MediaKeySystem,
    WebsiteData,
    Xr,
    Unknown,
}

/// the GObject type name of a WebKitGTK permission request -> its kind (user media gets its
/// device flags filled in by the caller). Anything not listed: Unknown (denied).
pub fn kind_of(type_name: &str) -> Kind {
    match type_name {
        "WebKitUserMediaPermissionRequest" => Kind::UserMedia { audio: false, video: false, display: false },
        "WebKitDeviceInfoPermissionRequest" => Kind::DeviceInfo,
        "WebKitGeolocationPermissionRequest" => Kind::Geolocation,
        "WebKitNotificationPermissionRequest" => Kind::Notification,
        "WebKitPointerLockPermissionRequest" => Kind::PointerLock,
        "WebKitClipboardPermissionRequest" => Kind::Clipboard,
        "WebKitMediaKeySystemPermissionRequest" => Kind::MediaKeySystem,
        "WebKitWebsiteDataAccessPermissionRequest" => Kind::WebsiteData,
        "WebKitXRPermissionRequest" => Kind::Xr,
        _ => Kind::Unknown,
    }
}

/// the Device access rows other than the microphone (whose setting is checked at recording start)
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Rows {
    pub camera: Access,
    pub location: Access,
    pub notifications: Access,
    pub pointer_lock: Access,
}

/// the defaults = today's behaviour on develop: camera / location / notifications denied
/// (WebKit's default), pointer lock granted (WebKit's class default for an unhandled request)
pub const DEFAULT_ROWS: Rows = Rows {
    camera: Access::Off,
    location: Access::Off,
    notifications: Access::Off,
    pointer_lock: Access::Always,
};

/// the answer to one request
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Decision {
    Allow,
    /// a native consent decides (default Cancel); until it answers the request stays pending
    Ask,
    Deny,
}

fn row(a: Access, lockdown: bool) -> Decision {
    if lockdown {
        return Decision::Deny; // lockdown forces every row off, over always allow too
    }
    match a {
        Access::Always => Decision::Allow,
        Access::Ask => Decision::Ask,
        Access::Off => Decision::Deny,
    }
}

/// THE rule for every permission request of webview `label`
pub fn decide(label: &str, kind: Kind, rows: Rows, lockdown: bool, now: Instant) -> Decision {
    let rec = crate::audiorec::is_rec(label);
    match kind {
        Kind::UserMedia { audio: true, video, display } => {
            // the microphone: #136 rule, unchanged (recorder webview, audio only, armed)
            match crate::audiorec::decide(label, true, video, display, now) {
                crate::audiorec::Verdict::Allow if !lockdown => Decision::Allow,
                _ => Decision::Deny,
            }
        }
        Kind::UserMedia { audio: false, video: true, display: false } if !rec => row(rows.camera, lockdown),
        Kind::Geolocation if !rec => row(rows.location, lockdown),
        Kind::Notification if !rec => row(rows.notifications, lockdown),
        Kind::PointerLock if !rec => row(rows.pointer_lock, lockdown),
        // screen capture, device names, EME, website data, clipboard, XR, unknown, and anything the
        // recorder webview asks besides its microphone: not available
        _ => Decision::Deny,
    }
}

/// the rows in force now: the stored rows (Settings › Security › Device access, read fresh from the
/// config so a change applies to the next request) and the lockdown latch. No latch (cannot happen
/// after main()) = lockdown: fail closed.
pub fn current() -> (Rows, bool) {
    let lockdown = crate::security::booted().map(|s| s.lockdown).unwrap_or(true);
    (crate::security::dev_rows(&crate::cfg_value()), lockdown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::Mic as A;

    const ALL: [A; 3] = [A::Ask, A::Always, A::Off];
    const APP: &str = "main";

    fn rows_all() -> Vec<Rows> {
        let mut v = Vec::new();
        for c in ALL {
            for l in ALL {
                for n in ALL {
                    for p in ALL {
                        v.push(Rows { camera: c, location: l, notifications: n, pointer_lock: p });
                    }
                }
            }
        }
        v
    }

    fn not_available() -> Vec<Kind> {
        let mut v = vec![Kind::DeviceInfo, Kind::Clipboard, Kind::MediaKeySystem, Kind::WebsiteData, Kind::Xr, Kind::Unknown];
        for a in [false, true] {
            for vd in [false, true] {
                v.push(Kind::UserMedia { audio: a, video: vd, display: true });
            }
        }
        v.push(Kind::UserMedia { audio: false, video: false, display: false });
        v
    }

    #[test]
    fn devperm_kind_of_names_and_unknown() {
        assert_eq!(kind_of("WebKitPointerLockPermissionRequest"), Kind::PointerLock);
        assert_eq!(kind_of("WebKitGeolocationPermissionRequest"), Kind::Geolocation);
        assert_eq!(kind_of("WebKitNotificationPermissionRequest"), Kind::Notification);
        assert_eq!(kind_of("WebKitXRPermissionRequest"), Kind::Xr);
        assert_eq!(kind_of("WebKitClipboardPermissionRequest"), Kind::Clipboard);
        for n in ["", "WebKitFutureThingPermissionRequest", "webkitgeolocationpermissionrequest", "WebKitUserMediaPermissionRequestX"] {
            assert_eq!(kind_of(n), Kind::Unknown, "{n}");
        }
    }

    /// no row value, lockdown or webview turns a not-available kind into anything but Deny
    #[test]
    fn devperm_not_available_always_denied() {
        let now = Instant::now();
        for r in rows_all() {
            for lk in [false, true] {
                for label in [APP, "rec-main"] {
                    for k in not_available() {
                        assert_eq!(decide(label, k, r, lk, now), Decision::Deny, "{label} {k:?} {r:?} lockdown={lk}");
                    }
                }
            }
        }
    }

    #[test]
    fn devperm_rows_ask_always_off_and_lockdown() {
        let now = Instant::now();
        let cam = Kind::UserMedia { audio: false, video: true, display: false };
        for (k, set) in [
            (cam, (|r: &mut Rows, a| r.camera = a) as fn(&mut Rows, A)),
            (Kind::Geolocation, |r, a| r.location = a),
            (Kind::Notification, |r, a| r.notifications = a),
            (Kind::PointerLock, |r, a| r.pointer_lock = a),
        ] {
            for (a, want) in [(A::Ask, Decision::Ask), (A::Always, Decision::Allow), (A::Off, Decision::Deny)] {
                let mut r = DEFAULT_ROWS;
                set(&mut r, a);
                assert_eq!(decide(APP, k, r, false, now), want, "{k:?} {a:?}");
                assert_eq!(decide(APP, k, r, true, now), Decision::Deny, "lockdown {k:?} {a:?}");
                assert_eq!(decide("rec-main", k, r, false, now), Decision::Deny, "recorder webview {k:?} {a:?}");
            }
        }
    }

    /// the defaults are today's behaviour: only pointer lock (WebKit's class default) is allowed
    #[test]
    fn devperm_defaults_are_todays_behaviour() {
        let now = Instant::now();
        let d = |k| decide(APP, k, DEFAULT_ROWS, false, now);
        assert_eq!(d(Kind::UserMedia { audio: false, video: true, display: false }), Decision::Deny);
        assert_eq!(d(Kind::Geolocation), Decision::Deny);
        assert_eq!(d(Kind::Notification), Decision::Deny);
        assert_eq!(d(Kind::PointerLock), Decision::Allow);
        assert_eq!(decide(APP, Kind::PointerLock, DEFAULT_ROWS, true, now), Decision::Deny);
    }

    /// the microphone stays the #136 rule: an app window never, camera rows never widen it
    #[test]
    fn devperm_mic_app_window_never_even_with_camera_always() {
        let now = Instant::now();
        let mut r = DEFAULT_ROWS;
        r.camera = A::Always;
        for v in [false, true] {
            assert_eq!(decide(APP, Kind::UserMedia { audio: true, video: v, display: false }, r, false, now), Decision::Deny);
        }
        // the recorder webview, unarmed: denied; its camera: denied with camera = always
        assert_eq!(decide("rec-main", Kind::UserMedia { audio: true, video: false, display: false }, r, false, now), Decision::Deny);
    }

    /// SOURCE SHAPE: the GTK handler classifies EVERY request with kind_of and answers it through
    /// devperm::decide with allow()/deny(), returns true on every path (never WebKit's default,
    /// which allows pointer lock), and the macOS media gate goes through the same decide.
    #[test]
    fn devperm_handler_decides_every_request() {
        let src = include_str!("audiorec.rs");
        let start = src.find("wv.connect_permission_request(").expect("the GTK handler");
        let body = &src[start..];
        let end = body.find("\n        });").expect("handler end");
        let body = &body[..end];
        assert!(body.contains("devperm::kind_of("), "classified by kind_of");
        assert!(body.contains("devperm::decide("), "answered by devperm::decide");
        assert!(body.contains("req.allow()") && body.contains("req.deny()"));
        for bad in ["false", "return false", "PermissionRequestExt::", "_ =>"] {
            let code: String = body.lines().map(|l| l.split("//").next().unwrap_or("")).collect::<Vec<_>>().join("\n");
            assert!(!code.contains(bad), "handler must not contain {bad:?}");
        }
        assert!(body.trim_end().ends_with("true"), "every path returns true");
        assert_eq!(src.matches("connect_permission_request(").count(), 1, "one handler");
        for f in ["main.rs", "macos.rs", "consent.rs", "security.rs"] {
            let s = match f {
                "main.rs" => include_str!("main.rs"),
                "macos.rs" => include_str!("macos.rs"),
                "consent.rs" => include_str!("consent.rs"),
                _ => include_str!("security.rs"),
            };
            assert!(!s.contains(concat!("connect_permission", "_request(")), "{f}: a second handler");
        }
        // macOS: the media gate registered by install_then asks devperm::decide
        let mac = src.find("crate::macos::media_gate(").expect("macOS gate");
        let line = &src[mac..mac + src[mac..].find('\n').unwrap()];
        assert!(line.contains("devperm::decide("), "macOS through devperm::decide: {line}");
    }
}
