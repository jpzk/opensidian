// SPDX-License-Identifier: GPL-3.0-or-later
//! #37 C3-3: the editor menu's clipboard (Cut / Copy / Paste / Paste as plain text).
//!
//! The webview can only touch the clipboard inside a REAL copy/paste event
//! (ui/editor.js); a menu row is a click, so stock's rows need a programmatic
//! read and write. They go through the toolkit the window already is: GTK's
//! CLIPBOARD selection (gtk/gdk 0.18.2, already pinned for consent.rs — no new
//! crate). Text only: an image paste stays Ctrl+V's (pasteimg.rs).
//!
//! Threading: GTK is main-thread only. A READ is asynchronous in GTK
//! (`request_text`, the owner may be another process), so the async command
//! posts it to the main loop and waits for the callback off the main thread —
//! never a nested `wait_for_text` loop inside the IPC handler. A WRITE is a
//! sync command, which Tauri runs on the main thread.
//!
//! #144 toggles 8 + 9: every command asks `crate::clip_allowed` FIRST (security::clip_check:
//! app clipboard off refuses all, plugin clipboard off refuses once community code entered
//! the page realm) — before any GTK call.

/// the largest text a menu Paste takes (a paste is one editor transaction)
pub const MAX_CLIP_TEXT: usize = 8 << 20;

/// clip_text's verdict on what the clipboard holds: None when there is no text
/// or it exceeds MAX_CLIP_TEXT (a Paste of nothing, never a truncated one)
pub fn accept(t: Option<&str>) -> Option<String> {
    match t {
        Some(s) if !s.is_empty() && s.len() <= MAX_CLIP_TEXT => Some(s.to_string()),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
#[allow(unused_imports)]
use gtk::prelude::*;

#[cfg(target_os = "linux")]
fn board() -> gtk::Clipboard {
    gtk::Clipboard::get(&gdk::Atom::intern("CLIPBOARD"))
}

#[cfg(target_os = "linux")]
async fn read(window: tauri::Window) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    window
        .run_on_main_thread(move || board().request_text(move |_, t| { let _ = tx.send(accept(t)); }))
        .ok()?;
    tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(2)).ok().flatten())
        .await
        .ok()
        .flatten()
}

/// macOS: NSPasteboard's plain text, read on the main thread (AppKit), same verdict (accept)
#[cfg(target_os = "macos")]
async fn read(window: tauri::Window) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel::<Option<String>>();
    window
        .run_on_main_thread(move || {
            let _ = tx.send(accept(crate::macos::clipboard_text().as_deref()));
        })
        .ok()?;
    tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(std::time::Duration::from_secs(2)).ok().flatten())
        .await
        .ok()
        .flatten()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
async fn read(_window: tauri::Window) -> Option<String> {
    None
}

/// the clipboard's text (None = no text: the menu's Paste rows are disabled)
#[tauri::command]
pub async fn clip_text(window: tauri::Window) -> Result<Option<String>, String> {
    crate::clip_allowed("clip_text")?;
    Ok(read(window).await)
}

/// does the clipboard hold text? The menu asks this when it OPENS, so the
/// content itself only crosses to the webview when the user picks Paste.
#[tauri::command]
pub async fn clip_has_text(window: tauri::Window) -> Result<bool, String> {
    crate::clip_allowed("clip_has_text")?;
    Ok(read(window).await.is_some())
}

/// Copy / Cut: put `text` on the clipboard (sync = Tauri's main thread)
#[tauri::command]
pub fn clip_set_text(text: String) -> Result<(), String> {
    crate::clip_allowed("clip_set_text")?;
    if text.len() > MAX_CLIP_TEXT {
        return Err("clip_set_text: text over the 8 MiB menu cap".into());
    }
    #[cfg(target_os = "linux")]
    {
        board().set_text(&text);
    }
    // macOS: the general NSPasteboard (menu Copy / Cut; CodeAnt #248: Cut deleted without copying)
    #[cfg(target_os = "macos")]
    if !crate::macos::clipboard_set_text(&text) {
        return Err("clip_set_text: the pasteboard refused the text".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_takes_text_and_refuses_empty_or_huge() {
        assert_eq!(accept(Some("these words")).as_deref(), Some("these words"));
        assert_eq!(accept(Some("")), None);
        assert_eq!(accept(None), None);
        let big = "x".repeat(MAX_CLIP_TEXT + 1);
        assert_eq!(accept(Some(&big)), None);
    }
}
