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
//! Fix: name the process APP_ID before GTK starts, for BOTH the prgname and
//! the X11 program class (else GDK capitalises it to "Dev.koto.opensidian"),
//! so app_id == WM_CLASS instance == class == the desktop-file id == the
//! StartupWMClass every shipped desktop file carries. The human-readable
//! application name stays "opensidian" (GLib would otherwise fall back to the
//! prgname). Deliberately NOT tauri's enableGTKAppId: that registers a unique
//! GApplication on the session bus and changes single-instance behaviour.

/// tauri.conf.json `identifier`, the desktop-file id, the flatpak app id.
pub const APP_ID: &str = "dev.koto.opensidian";
/// What the user reads (g_get_application_name).
pub const APP_NAME: &str = "opensidian";

/// Call before tauri/GTK creates the first window. Idempotent (Once: GLib
/// warns on a second g_set_application_name).
///
/// The program class goes through gdk-sys, not `gdk::set_program_class`: the
/// safe wrapper asserts GTK is already initialised, and the point is to set it
/// BEFORE gdk_init reads it (gdk_set_program_class only stores a string).
#[cfg(target_os = "linux")]
pub fn apply() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        gdk::glib::set_prgname(Some(APP_ID));
        gdk::glib::set_application_name(APP_NAME);
        let class = std::ffi::CString::new(APP_ID).expect("APP_ID has no NUL");
        // SAFETY: GDK g_strdup()s the argument; `class` outlives the call.
        unsafe { gdk::ffi::gdk_set_program_class(class.as_ptr()) };
    });
}

/// The X11 program class GDK will put in WM_CLASS (readable before gtk init).
#[cfg(target_os = "linux")]
pub fn program_class() -> Option<String> {
    // SAFETY: returns GDK's own static string (or NULL), never freed by us.
    let p = unsafe { gdk::ffi::gdk_get_program_class() };
    (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "linux"))]
pub fn apply() {}

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
    fn apply_sets_prgname_class_and_keeps_display_name() {
        apply();
        apply(); // idempotent
        assert_eq!(gdk::glib::prgname().as_deref(), Some(APP_ID));
        assert_eq!(gdk::glib::application_name().as_deref(), Some(APP_NAME));
        assert_eq!(program_class().as_deref(), Some(APP_ID));
    }
}
