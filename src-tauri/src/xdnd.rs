// SPDX-License-Identifier: GPL-3.0-or-later
//! #51 "094 still eats links": the app window's OS drop transport (R31 osdrop), owned here
//! instead of by Tauri's built-in drag-drop handler.
//!
//! WHY. Tauri's handler (wry-0.55.1 src/webkitgtk/drag_drop.rs:77-113 +
//! tauri-runtime-wry-2.11.4 src/lib.rs:4862-4895) treats EVERY drag that offers
//! `text/uri-list` as a file drop and always claims it at the GTK layer, so WebKit never
//! performs it and the DOM never sees a drop. A browser LINK drag offers exactly that
//! (Firefox: text/x-moz-url + text/uri-list + text/html + text/plain for any anchor or image
//! drag, nsContentAreaDragDrop.cpp:706-724), so the link reached attach_drop as a "path"
//! `https://…`, was refused as Refused::Remote and nothing was inserted: the link was
//! eaten. Stock 1.13.7 converts the drop's text/html to Markdown (`[title](url)`), the same
//! as a selection drop (harness docs/issue-51/STOCK.md).
//!
//! WHAT. The same state machine as wry's (data-received -> leave -> drop), with ONE
//! difference: a drop is claimed only when its uri-list names at least one LOCAL file
//! (`file:` URI). Those keep the exact old path: `drop-files` with `[paths, x, y]`
//! (main.rs DROP_EVENT) -> ui/main.js attachDrop -> attach_files -> attach_drop, every rule
//! still there (remote entries in a mixed list are still refused by attach_drop). A
//! uri-list of only remote URLs is not claimed: WebKit's own drop runs (its handlers are
//! connected AFTER, DropTargetGtk3.cpp g_signal_connect_after), the DOM drop reaches the
//! editor and Ed.htmlDrop (ui/editor.js) inserts the Markdown of its text/html. A drop off
//! the editor that WebKit would turn into a navigation is still denied by navguard.
//!
//! The decision is the pure fn `claim` (unit-tested below); the GTK wiring is the gate
//! phases' (osdrop: a file drag still attaches; droplink: a link drag lands as Markdown).
//! No new crate: gtk/gdk 0.18.2 are already pinned (consent.rs), the webview's
//! webkit2gtk::WebView is used only as the gtk::Widget it already is.
use gtk::prelude::*;
use std::cell::RefCell;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::rc::Rc;

/// `pre` stripped off the front of `s`, ASCII case-insensitively (URI schemes are)
fn strip_ci<'a>(s: &'a str, pre: &str) -> Option<&'a str> {
    let head = s.get(..pre.len())?;
    if head.eq_ignore_ascii_case(pre) {
        s.get(pre.len()..)
    } else {
        None
    }
}

/// RFC 3986 %XX decoding to raw bytes (a file name need not be UTF-8); a malformed escape
/// stays literal, as wry's percent_decode leaves it
fn pct_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 3 <= b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// is this uri-list entry a local file (`file:` scheme)?
pub(crate) fn is_file_uri(u: &str) -> bool {
    strip_ci(u.trim(), "file:").is_some()
}

/// a uri-list entry as the path attach_drop gets: `file:///p`, `file://localhost/p` and
/// `file:/p` -> `/p` (%XX decoded); anything else stays the text it was, so attach_drop
/// can still say WHY it is refused (Refused::Remote for `https://…`)
pub(crate) fn uri_path(u: &str) -> PathBuf {
    let u = u.trim();
    let p = if let Some(r) = strip_ci(u, "file://") {
        strip_ci(r, "localhost").filter(|x| x.starts_with('/')).unwrap_or(r)
    } else if let Some(r) = strip_ci(u, "file:").filter(|x| x.starts_with('/')) {
        r
    } else {
        return PathBuf::from(u);
    };
    PathBuf::from(OsString::from_vec(pct_decode(p)))
}

/// THE decision: Some(paths) = a file drop this app takes (drop-files -> attach_drop);
/// None = not ours, WebKit performs the drop (text, html, a remote link)
pub(crate) fn claim(uris: &[String]) -> Option<Vec<PathBuf>> {
    let uris: Vec<&str> = uris.iter().map(|u| u.trim()).filter(|u| !u.is_empty() && !u.starts_with('#')).collect();
    if uris.iter().any(|u| is_file_uri(u)) {
        Some(uris.iter().map(|u| uri_path(u)).collect())
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    Left,
    Entered,
    Leaving,
}

struct Ctl {
    st: St,
    paths: Option<Vec<PathBuf>>,
}

/// wire the app window's webview: call once, after build, with Tauri's own handler disabled
/// (page_window: disable_drag_drop_handler). The emitted payload is the one the old
/// WindowEvent::DragDrop pass-through emitted: (paths, x, y), x/y the GTK drop point.
pub(crate) fn attach(w: &tauri::WebviewWindow) {
    let win = w.clone();
    let r = w.with_webview(move |pw| {
        let wv: gtk::Widget = pw.inner().upcast();
        let c = Rc::new(RefCell::new(Ctl { st: St::Left, paths: None }));
        {
            let c = c.clone();
            wv.connect_drag_data_received(move |_, _, _, _, data, _, _| {
                if data.target().name().as_str() != "text/uri-list" {
                    return;
                }
                let uris: Vec<String> = data.uris().iter().map(|u| u.to_string()).collect();
                let mut b = c.borrow_mut();
                match claim(&uris) {
                    Some(p) => {
                        b.st = St::Entered;
                        b.paths = Some(p);
                    }
                    None => {
                        b.st = St::Left;
                        b.paths = None;
                    }
                }
            });
        }
        {
            let c = c.clone();
            // GTK emits drag-leave BEFORE drag-drop; a leave that no drop follows ends the drag
            wv.connect_drag_leave(move |_, _, _| {
                let mut b = c.borrow_mut();
                if b.st == St::Left {
                    return;
                }
                b.st = St::Leaving;
                drop(b);
                let c = c.clone();
                gtk::glib::idle_add_local_once(move || {
                    let mut b = c.borrow_mut();
                    if b.st == St::Leaving {
                        b.st = St::Left;
                        b.paths = None;
                    }
                });
            });
        }
        wv.connect_drag_drop(move |_, ctx, x, y, time| {
            let mut b = c.borrow_mut();
            if b.st != St::Leaving {
                return false; // not a file drop: WebKit's own drop handler runs next
            }
            let Some(paths) = b.paths.take() else { return false };
            b.st = St::Left;
            drop(b);
            ctx.drop_finish(true, time);
            use tauri::Emitter;
            let _ = win.emit(crate::DROP_EVENT, (paths, x as f64, y as f64));
            true // claimed: WebKit never performs a file drop (R31: the DOM never sees the files)
        });
    });
    if let Err(e) = r {
        eprintln!("[osdrop] could not wire the drop handler: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    /// #51: a browser link drag (only remote URIs) is NOT a file drop — WebKit gets it, so
    /// the editor's text/html conversion runs instead of attach_drop refusing it
    #[test]
    fn a_remote_only_uri_list_is_left_to_webkit() {
        assert_eq!(claim(&s(&["https://example.com/x"])), None);
        assert_eq!(claim(&s(&["https://a.example/1", "http://b.example/2"])), None);
        assert_eq!(claim(&s(&["data:text/plain,hi"])), None);
        assert_eq!(claim(&s(&[])), None);
        assert_eq!(claim(&s(&["# a comment line", ""])), None);
    }

    /// R31 unchanged: a file manager's drag is claimed, paths exactly as wry handed them
    #[test]
    fn a_file_uri_list_is_claimed_with_decoded_paths() {
        assert_eq!(claim(&s(&["file:///tmp/a%20b.png"])), Some(vec![PathBuf::from("/tmp/a b.png")]));
        assert_eq!(
            claim(&s(&["file:///x/1.png", "file:///x/%C3%A9.pdf"])),
            Some(vec![PathBuf::from("/x/1.png"), PathBuf::from("/x/\u{e9}.pdf")])
        );
        assert_eq!(claim(&s(&["FILE:///x.png"])), Some(vec![PathBuf::from("/x.png")]));
        assert_eq!(claim(&s(&["file://localhost/x.png"])), Some(vec![PathBuf::from("/x.png")]));
        assert_eq!(claim(&s(&["file:/x.png"])), Some(vec![PathBuf::from("/x.png")]));
        assert_eq!(claim(&s(&["  file:///x.png\r"])), Some(vec![PathBuf::from("/x.png")]));
    }

    /// a mixed list is still a file drop; the remote entry reaches attach_drop as text, which
    /// refuses it as Remote (R31.8) — the old behaviour for that entry
    #[test]
    fn a_mixed_list_is_claimed_and_keeps_the_remote_entry_as_text() {
        assert_eq!(
            claim(&s(&["file:///x.png", "https://e.example/y.png"])),
            Some(vec![PathBuf::from("/x.png"), PathBuf::from("https://e.example/y.png")])
        );
    }

    /// a non-UTF-8 file name survives as bytes; a broken escape stays literal
    #[test]
    fn decoding_is_byte_exact() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(uri_path("file:///t/%FF.png").as_os_str().as_bytes(), b"/t/\xFF.png");
        assert_eq!(uri_path("file:///t/100%.png"), PathBuf::from("/t/100%.png"));
        assert_eq!(uri_path("file:///t/%zz"), PathBuf::from("/t/%zz"));
        assert!(!is_file_uri("https://file.example/"));
        assert!(!is_file_uri("fil"));
    }
}
