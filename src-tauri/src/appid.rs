// SPDX-License-Identifier: GPL-3.0-or-later
//! wmclass: the window identity the desktop shell groups us by.
//!
//! GNOME (and every XDG dock) maps a running window to its launcher by the
//! Wayland xdg_toplevel app_id / X11 WM_CLASS, compared with the desktop-file
//! id or its StartupWMClass=. GTK3 derives both from the GLib prgname (app_id
//! = prgname; WM_CLASS = prgname, program class) and, unset, the prgname is
//! argv[0]'s basename: "opensidian". Measured on main f915d786
//! (goal/wmclass evidence/measure-before.txt): app_id "opensidian", WM_CLASS
//! "opensidian","Opensidian" != dev.koto.opensidian.desktop -> GNOME tracked
//! every deb/rpm/AppImage window as "window:N" with the generic icon.
//!
//! Fix, in two steps because GTK owns the timing:
//! 1. `apply()`, before GTK starts: prgname = APP_ID -> Wayland app_id and the
//!    X11 WM_CLASS instance; application name stays "opensidian" (GLib would
//!    otherwise fall back to the prgname for the name users read).
//! 2. `set_x11_class()`, after gtk_init and before the first window: X11
//!    WM_CLASS class = APP_ID. It cannot go in step 1: GTK 3's gdk_pre_parse()
//!    (run by gtk_init) overwrites the program class with the capitalised
//!    prgname. Measured on ec78b73f, which set it pre-init: xprop WM_CLASS =
//!    "dev.koto.opensidian", "Dev.koto.opensidian" (evidence/raw/after-ec78b73f).
//!    main() registers it as a tauri plugin: tauri builds the runtime (gtk
//!    init) first, then initialises plugins, then creates the config windows.
//! Result: app_id == WM_CLASS instance == class == the desktop-file id == the
//! StartupWMClass every shipped desktop file carries. Deliberately NOT tauri's
//! enableGTKAppId: that registers a unique GApplication on the session bus and
//! changes single-instance behaviour.

/// tauri.conf.json `identifier`, the desktop-file id, the flatpak app id.
pub const APP_ID: &str = "dev.koto.opensidian";
/// What the user reads (g_get_application_name).
pub const APP_NAME: &str = "opensidian";

/// Call before tauri/GTK starts. Idempotent (Once: GLib warns on a second
/// g_set_application_name).
#[cfg(target_os = "linux")]
pub fn apply() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        gdk::glib::set_prgname(Some(APP_ID));
        gdk::glib::set_application_name(APP_NAME);
    });
}

/// Call after gtk_init, before the first window (see step 2 above). Returns
/// whether it applied: false = GDK has no default display yet (gtk_init not
/// run: unit tests, a misordered call) — nothing set, nothing panics.
/// FFI, not `gdk::set_program_class`: the safe wrapper asserts gtk-rs's own
/// init flag; whether GDK is up is a C-side fact, so ask C.
#[cfg(target_os = "linux")]
pub fn set_x11_class() -> bool {
    // SAFETY: plain GDK getters/setters on the thread tauri runs GTK on;
    // gdk_set_program_class g_strdup()s its argument, `class` outlives the call.
    unsafe {
        if gdk::ffi::gdk_display_get_default().is_null() {
            return false;
        }
        let class = std::ffi::CString::new(APP_ID).expect("APP_ID has no NUL");
        gdk::ffi::gdk_set_program_class(class.as_ptr());
    }
    true
}

/// tauri plugin running `set_x11_class` at plugin init (after the runtime
/// initialised GTK, before tauri creates the config windows).
pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("wmclass")
        .setup(|_app, _api| {
            if !set_x11_class() {
                eprintln!("[wmclass] GTK not initialised at plugin setup: X11 WM_CLASS class left to GTK");
            }
            Ok(())
        })
        .build()
}

/// The X11 program class GDK will put in WM_CLASS (tests read it).
#[cfg(target_os = "linux")]
#[cfg_attr(not(test), allow(dead_code))]
pub fn program_class() -> Option<String> {
    // SAFETY: returns GDK's own static string (or NULL), never freed by us.
    let p = unsafe { gdk::ffi::gdk_get_program_class() };
    (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
pub fn apply() {}

#[cfg(not(target_os = "linux"))]
pub fn set_x11_class() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONF: &str = include_str!("../tauri.conf.json");
    const DESKTOP: &str = include_str!("../../packaging/flatpak/dev.koto.opensidian.desktop");

    #[test]
    fn app_id_is_the_tauri_identifier() {
        let v: serde_json::Value = serde_json::from_str(CONF).unwrap();
        assert_eq!(v["identifier"].as_str(), Some(APP_ID));
    }

    #[test]
    fn desktop_file_startupwmclass_is_app_id() {
        let lines: Vec<&str> = DESKTOP.lines().map(str::trim).collect();
        assert!(lines.contains(&"[Desktop Entry]"));
        let wm: Vec<&str> = lines
            .iter()
            .filter_map(|l| l.strip_prefix("StartupWMClass="))
            .collect();
        assert_eq!(wm, vec![APP_ID], "exactly one StartupWMClass == APP_ID");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn apply_sets_prgname_and_keeps_display_name() {
        apply();
        apply(); // idempotent
        assert_eq!(gdk::glib::prgname().as_deref(), Some(APP_ID));
        assert_eq!(gdk::glib::application_name().as_deref(), Some(APP_NAME));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn x11_class_refuses_before_gtk_init() {
        // test threads never run gtk_init: must neither panic nor pretend
        assert!(!set_x11_class());
        assert_ne!(program_class().as_deref(), Some(APP_ID));
    }
}
