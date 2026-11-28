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
#[cfg(target_os = "linux")]
use gtk::prelude::*;
#[cfg(target_os = "linux")]
use std::cell::RefCell;
#[cfg(unix)]
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
#[cfg(target_os = "linux")]
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

/// the %XX-encoded path of a LOCAL file URI: `file:///p`, `file://localhost/p`, `file:/p`
/// -> `/p`. A file URI naming another host (`file://server/share/x`) is NOT local: None,
/// never a host-less relative path that could resolve against the working directory
fn local_part(u: &str) -> Option<&str> {
    let u = u.trim();
    if let Some(r) = strip_ci(u, "file://") {
        let r = strip_ci(r, "localhost").unwrap_or(r);
        r.starts_with('/').then_some(r)
    } else {
        strip_ci(u, "file:").filter(|x| x.starts_with('/'))
    }
}

/// is this uri-list entry a LOCAL file (`file:` scheme, no foreign host)?
pub(crate) fn is_file_uri(u: &str) -> bool {
    local_part(u).is_some()
}

/// a uri-list entry as the path attach_drop gets: a local file URI -> its absolute path
/// (%XX decoded); anything else (`https://…`, `file://server/…`) stays the text it was, so
/// attach_drop refuses it as Refused::Remote (its "://" check) instead of guessing a path
pub(crate) fn uri_path(u: &str) -> PathBuf {
    match local_part(u) {
        #[cfg(unix)]
        Some(p) => PathBuf::from(OsString::from_vec(pct_decode(p))),
        // Windows: "file:///C:/x" names "/C:/x"; the drive form is the path
        #[cfg(windows)]
        Some(p) => {
            let s = String::from_utf8_lossy(&pct_decode(p)).into_owned();
            PathBuf::from(match s.as_bytes() {
                [b'/', d, b':', ..] if d.is_ascii_alphabetic() => s[1..].to_string(),
                _ => s,
            })
        }
        None => PathBuf::from(u.trim()),
    }
}

/// #280: targets only a BROWSER (or a web page inside one) offers for a drag. A file
/// manager offers text/uri-list (+ plain text); a page dragging an element offers these
/// next to a uri-list it controls — which may name a `file:` URI of the page's choosing.
/// Such a drag is never claimed as a file drop: WebKit performs it (a DOM drop, the
/// editor's text/html road), so nothing is moved or copied on a web page's say-so.
pub(crate) const BROWSER_TARGETS: [&str; 4] = ["text/x-moz-url", "text/html", "_NETSCAPE_URL", "chromium/x-renderer-taint"];

/// does the drag offer a browser-only target? (MIME type names compare case-insensitively)
pub(crate) fn from_browser(targets: &[String]) -> bool {
    targets.iter().any(|t| {
        let t = t.trim();
        let t = t.split(';').next().unwrap_or(t).trim();
        BROWSER_TARGETS.iter().any(|b| t.eq_ignore_ascii_case(b))
    })
}

/// THE decision: Some(paths) = a file drop this app takes (drop-files -> attach_drop);
/// None = not ours, WebKit performs the drop (text, html, a remote link, #280: any drag that
/// also offers a browser target). `targets` = every target the drag source offers.
pub(crate) fn claim(targets: &[String], uris: &[String]) -> Option<Vec<PathBuf>> {
    if from_browser(targets) {
        return None;
    }
    let uris: Vec<&str> = uris.iter().map(|u| u.trim()).filter(|u| !u.is_empty() && !u.starts_with('#')).collect();
    if uris.iter().any(|u| is_file_uri(u)) {
        Some(uris.iter().map(|u| uri_path(u)).collect())
    } else {
        None
    }
}

/// #280 what the page receives for a claimed drop: a one-shot ticket (dropticket.rs) and
/// the display names, never the paths. attach_files takes the ticket.
#[derive(serde::Serialize, Clone, Debug, PartialEq)]
pub(crate) struct Offer {
    pub ticket: String,
    pub names: Vec<String>,
}

/// register a claimed drop for window `win`: Some(offer) to emit, None = not delivered
/// (no randomness: fail closed, nothing the page could import)
pub(crate) fn offer(win: &str, paths: &[PathBuf]) -> Option<Offer> {
    match crate::dropticket::mint(win, paths) {
        Ok(ticket) => Some(Offer { ticket, names: crate::dropticket::display_names(paths) }),
        Err(e) => {
            eprintln!("[osdrop] drop not delivered: {}", e.say());
            None
        }
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
/// WindowEvent::DragDrop pass-through emitted, except that #280 replaced the paths with an
/// Offer (a one-shot ticket + display names): (offer, x, y), x/y the GTK drop point.
/// macOS: the same rule on AppKit's drop (macos::file_drop_hook): local files -> drop-files with
/// (offer, x, y); anything else is WebKit's DOM drop
#[cfg(target_os = "macos")]
pub(crate) fn attach(w: &tauri::WebviewWindow) {
    let win = w.clone();
    let r = w.with_webview(move |pw| {
        crate::macos::file_drop_hook(pw.inner(), move |paths, x, y| {
            use tauri::Emitter;
            let Some(o) = offer(win.label(), &paths) else { return };
            let _ = win.emit(crate::DROP_EVENT, (o, x, y));
        });
    });
    if let Err(e) = r {
        eprintln!("[xdnd] macOS drop transport not installed: {e}");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn attach(_w: &tauri::WebviewWindow) {}

#[cfg(target_os = "linux")]
pub(crate) fn attach(w: &tauri::WebviewWindow) {
    let win = w.clone();
    let r = w.with_webview(move |pw| {
        let wv: gtk::Widget = pw.inner().upcast();
        let c = Rc::new(RefCell::new(Ctl { st: St::Left, paths: None }));
        {
            let c = c.clone();
            wv.connect_drag_data_received(move |_, ctx, _, _, data, _, _| {
                if data.target().name().as_str() != "text/uri-list" {
                    return;
                }
                let uris: Vec<String> = data.uris().iter().map(|u| u.to_string()).collect();
                // #280: EVERY target the source offers, so a browser drag is told apart
                let targets: Vec<String> = ctx.list_targets().iter().map(|a| a.name().to_string()).collect();
                let mut b = c.borrow_mut();
                match claim(&targets, &uris) {
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
            // #280: the page gets a one-shot ticket for these paths, never the paths
            let Some(o) = offer(win.label(), &paths) else {
                ctx.drop_finish(false, time);
                return true; // claimed and refused: no ticket, nothing the page could import
            };
            ctx.drop_finish(true, time);
            use tauri::Emitter;
            let _ = win.emit(crate::DROP_EVENT, (o, x as f64, y as f64));
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
    /// the targets a file manager's drag offers (Nautilus/Thunar: the uri-list + plain text)
    fn fm() -> Vec<String> {
        s(&["text/uri-list", "text/plain", "UTF8_STRING"])
    }

    /// #280: a drag that ALSO offers a browser target is a web page's drag, never a file
    /// drop, even when its uri-list names a local file (the page chose that uri-list)
    #[test]
    fn a_drag_offering_a_browser_target_is_not_claimed() {
        let file = s(&["file:///home/u/Pictures/a.png"]);
        let moz = s(&["text/x-moz-url", "text/uri-list", "text/html", "text/plain"]);
        assert_eq!(claim(&moz, &file), None, "text/x-moz-url (Firefox) must not be claimed");
        assert_eq!(claim(&s(&["text/uri-list", "text/x-moz-url"]), &file), None);
        assert_eq!(claim(&s(&["text/uri-list", "text/html"]), &file), None, "text/html (Chromium)");
        assert_eq!(claim(&s(&["text/uri-list", "text/html;charset=utf-8"]), &file), None);
        assert_eq!(claim(&s(&["TEXT/X-MOZ-URL", "text/uri-list"]), &file), None, "MIME names are case-insensitive");
        assert_eq!(claim(&s(&["_NETSCAPE_URL", "text/uri-list"]), &file), None);
        assert_eq!(claim(&s(&["chromium/x-renderer-taint", "text/uri-list"]), &file), None);
        // and the same uri-list from a file manager is still a file drop (R31 unchanged)
        assert_eq!(claim(&fm(), &file), Some(vec![PathBuf::from("/home/u/Pictures/a.png")]));
        assert_eq!(claim(&s(&["text/uri-list"]), &file), Some(vec![PathBuf::from("/home/u/Pictures/a.png")]));
    }

    /// #280: what the page receives of a claimed drop names no directory
    #[test]
    fn an_offer_carries_a_ticket_and_names_only() {
        let o = offer("main", &[PathBuf::from("/home/u/Pictures/a b.png"), PathBuf::from("/x/y/z.pdf")]).expect("urandom");
        assert_eq!(o.names, vec!["a b.png", "z.pdf"]);
        assert_eq!(o.ticket.len(), 32);
        let j = serde_json::to_string(&o).unwrap();
        assert!(!j.contains("/home") && !j.contains("Pictures") && !j.contains("/x/y"), "{j}");
        assert_eq!(crate::dropticket::take("main", &o.ticket).unwrap().len(), 2);
    }

    /// #51: a browser link drag (only remote URIs) is NOT a file drop — WebKit gets it, so
    /// the editor's text/html conversion runs instead of attach_drop refusing it
    #[test]
    fn a_remote_only_uri_list_is_left_to_webkit() {
        assert_eq!(claim(&fm(), &s(&["https://example.com/x"])), None);
        assert_eq!(claim(&fm(), &s(&["https://a.example/1", "http://b.example/2"])), None);
        assert_eq!(claim(&fm(), &s(&["data:text/plain,hi"])), None);
        assert_eq!(claim(&fm(), &s(&[])), None);
        assert_eq!(claim(&fm(), &s(&["# a comment line", ""])), None);
    }

    /// R31 unchanged: a file manager's drag is claimed, paths exactly as wry handed them
    #[test]
    fn a_file_uri_list_is_claimed_with_decoded_paths() {
        assert_eq!(claim(&fm(), &s(&["file:///tmp/a%20b.png"])), Some(vec![PathBuf::from("/tmp/a b.png")]));
        assert_eq!(
            claim(&fm(), &s(&["file:///x/1.png", "file:///x/%C3%A9.pdf"])),
            Some(vec![PathBuf::from("/x/1.png"), PathBuf::from("/x/\u{e9}.pdf")])
        );
        assert_eq!(claim(&fm(), &s(&["FILE:///x.png"])), Some(vec![PathBuf::from("/x.png")]));
        assert_eq!(claim(&fm(), &s(&["file://localhost/x.png"])), Some(vec![PathBuf::from("/x.png")]));
        assert_eq!(claim(&fm(), &s(&["file:/x.png"])), Some(vec![PathBuf::from("/x.png")]));
        assert_eq!(claim(&fm(), &s(&["  file:///x.png\r"])), Some(vec![PathBuf::from("/x.png")]));
    }

    /// a mixed list is still a file drop; the remote entry reaches attach_drop as text, which
    /// refuses it as Remote (R31.8) — the old behaviour for that entry
    #[test]
    fn a_mixed_list_is_claimed_and_keeps_the_remote_entry_as_text() {
        assert_eq!(
            claim(&fm(), &s(&["file:///x.png", "https://e.example/y.png"])),
            Some(vec![PathBuf::from("/x.png"), PathBuf::from("https://e.example/y.png")])
        );
    }

    /// codeant #109: a file URI naming ANOTHER host is not local — never claimed on its own,
    /// and in a mixed list it stays the URI text (attach_drop: Refused::Remote via "://"),
    /// never the relative path `server/share/x.png` resolved against the working directory
    #[test]
    fn a_foreign_host_file_uri_is_not_local() {
        assert!(!is_file_uri("file://server/share/item.png"));
        assert!(!is_file_uri("file://localhostx/item.png"));
        assert!(!is_file_uri("file:relative.png"));
        assert_eq!(claim(&fm(), &s(&["file://server/share/item.png"])), None);
        assert_eq!(uri_path("file://server/share/item.png"), PathBuf::from("file://server/share/item.png"));
        assert_eq!(
            claim(&fm(), &s(&["file:///x.png", "file://server/share/item.png"])),
            Some(vec![PathBuf::from("/x.png"), PathBuf::from("file://server/share/item.png")])
        );
        assert!(uri_path("file://server/share/item.png").to_string_lossy().contains("://"));
    }

    /// a non-UTF-8 file name survives as bytes; a broken escape stays literal
    #[cfg(unix)]
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
