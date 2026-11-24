// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS-only glue: the few AppKit / WebKit / Security calls the Linux build makes through
//! GTK, WebKitGTK, libsoup and the Secret Service. Everything goes through `objc2` (already in
//! Cargo.lock as tauri's own dependency), sent dynamically by selector name, so no binding crate
//! and no feature flag is added. Each caller documents which Linux path it stands in for.
#![allow(clippy::missing_safety_doc)]

use objc2::encode::{Encode, Encoding, RefEncode};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{class, msg_send};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}
unsafe impl Encode for Point {
    const ENCODING: Encoding = Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl Encode for Size {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl Encode for Rect {
    const ENCODING: Encoding = Encoding::Struct("CGRect", &[Point::ENCODING, Size::ENCODING]);
}
unsafe impl RefEncode for Rect {
    const ENCODING_REF: Encoding = Encoding::Pointer(&<Self as Encode>::ENCODING);
}

pub fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { origin: Point { x, y }, size: Size { w, h } }
}

/// an NSString holding `s` (interior NULs are kept: initWithBytes, not a C string)
pub fn ns_string(s: &str) -> Retained<AnyObject> {
    // SAFETY: NSString alloc + initWithBytes:length:encoding: with a valid UTF-8 buffer
    unsafe {
        let a: Allocated<AnyObject> = msg_send![class!(NSString), alloc];
        let r: Option<Retained<AnyObject>> =
            msg_send![a, initWithBytes: s.as_ptr().cast::<std::ffi::c_void>(), length: s.len(), encoding: 4usize /* NSUTF8StringEncoding */];
        r.unwrap_or_else(|| msg_send![class!(NSString), string])
    }
}

/// the Rust copy of an NSString (None for nil)
pub fn rust_string(s: Option<&AnyObject>) -> Option<String> {
    let s = s?;
    // SAFETY: UTF8String of an NSString is a NUL-terminated buffer owned by the string
    unsafe {
        let p: *const std::ffi::c_char = msg_send![s, UTF8String];
        (!p.is_null()).then(|| std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
    }
}

pub fn has_class(name: &str) -> Option<&'static AnyClass> {
    AnyClass::get(&std::ffi::CString::new(name).ok()?)
}

/// does `obj` answer `sel` (a private WebKit preference may be gone in a later macOS)
pub fn responds(obj: &AnyObject, sel: objc2::runtime::Sel) -> bool {
    // SAFETY: respondsToSelector: is NSObject protocol
    unsafe { msg_send![obj, respondsToSelector: sel] }
}

/* ---------------------------------------------------------------- consent (NSAlert) */

/// The consent prompt as an NSAlert sheet on `ns_window` — the macOS twin of consent.rs's
/// gtk::MessageDialog: `head` is the alert's message (ours), `body` the page-supplied text in a
/// read-only, scrolling NSTextView (plain text, never markup, nothing cut: fixed 640x320 view),
/// `note` a fixed line under it. Buttons in order: the FIRST one is the default (Return), so it
/// must be the refusing answer. `done(i)` gets the index of the button pressed.
pub fn consent_alert(ns_window: *mut std::ffi::c_void, head: &str, body: &str, note: Option<&str>, buttons: &[&str], done: impl Fn(usize) + 'static) {
    // SAFETY: AppKit calls on the main thread (consent::ask_blocking -> run_on_main_thread)
    unsafe {
        let alert: Retained<AnyObject> = msg_send![class!(NSAlert), new];
        let _: () = msg_send![&*alert, setAlertStyle: 0usize /* NSAlertStyleWarning */];
        let _: () = msg_send![&*alert, setMessageText: &*ns_string(head)];
        let _: () = msg_send![&*alert, setInformativeText: &*ns_string(note.unwrap_or(""))];
        let frame = rect(0.0, 0.0, 640.0, 320.0);
        let sa: Allocated<AnyObject> = msg_send![class!(NSScrollView), alloc];
        let sv: Retained<AnyObject> = msg_send![sa, initWithFrame: frame];
        let _: () = msg_send![&*sv, setHasVerticalScroller: true];
        let _: () = msg_send![&*sv, setBorderType: 2usize /* NSBezelBorder */];
        let ta: Allocated<AnyObject> = msg_send![class!(NSTextView), alloc];
        let tv: Retained<AnyObject> = msg_send![ta, initWithFrame: frame];
        let _: () = msg_send![&*tv, setEditable: false];
        let _: () = msg_send![&*tv, setRichText: false];
        let _: () = msg_send![&*tv, setSelectable: true];
        let _: () = msg_send![&*tv, setString: &*ns_string(body)];
        let _: () = msg_send![&*tv, setAccessibilityIdentifier: &*ns_string("consent-body")];
        let _: () = msg_send![&*sv, setDocumentView: &*tv];
        let _: () = msg_send![&*alert, setAccessoryView: &*sv];
        let mut first: *mut AnyObject = std::ptr::null_mut();
        for (i, b) in buttons.iter().enumerate() {
            let btn: *mut AnyObject = msg_send![&*alert, addButtonWithTitle: &*ns_string(b)];
            // Return = the FIRST (refusing) button only, Escape too; no other button has a key
            let _: () = msg_send![btn, setKeyEquivalent: &*ns_string(if i == 0 { "\r" } else { "" })];
            if i == 0 {
                first = btn;
            }
        }
        let block = block2::RcBlock::new(move |resp: isize| {
            // NSAlertFirstButtonReturn = 1000, then 1001, 1002 ...
            done(usize::try_from(resp - 1000).unwrap_or(0));
        });
        // the read-only body must not hold the keyboard (it would eat Return): the refusing button
        // does, as the Deny button grabs focus in the GTK dialog
        let aw: *mut AnyObject = msg_send![&*alert, window];
        if !first.is_null() {
            let _: () = msg_send![aw, setInitialFirstResponder: first];
        }
        let win = ns_window as *mut AnyObject;
        if win.is_null() {
            let r: isize = msg_send![&*alert, runModal];
            block.call((r,));
        } else {
            let _: () = msg_send![&*alert, beginSheetModalForWindow: win, completionHandler: &*block];
            if !first.is_null() {
                let _: bool = msg_send![aw, makeFirstResponder: first];
            }
        }
    }
}

/* ---------------------------------------------------------------- secrets (Keychain) */

/// app.secretStorage on the login Keychain — the macOS twin of secrets.rs's Secret Service store:
/// one generic-password item per (vault, id): service "opensidian:<canonical vault>", account
/// = the id, label = secrets::label. Values only ever live in the Keychain; nothing on disk from us.
pub mod keychain {
    use super::*;
    use std::ffi::c_void;

    type OSStatus = i32;
    const NOT_FOUND: OSStatus = -25300;
    const DUPLICATE: OSStatus = -25299;

    #[link(name = "Security", kind = "framework")]
    extern "C" {
        static kSecClass: *const c_void;
        static kSecClassGenericPassword: *const c_void;
        static kSecAttrService: *const c_void;
        static kSecAttrAccount: *const c_void;
        static kSecAttrLabel: *const c_void;
        static kSecValueData: *const c_void;
        static kSecReturnData: *const c_void;
        static kSecReturnAttributes: *const c_void;
        static kSecMatchLimit: *const c_void;
        static kSecMatchLimitAll: *const c_void;
        static kSecMatchLimitOne: *const c_void;
        fn SecItemAdd(attrs: *const c_void, result: *mut *const c_void) -> OSStatus;
        fn SecItemCopyMatching(query: *const c_void, result: *mut *const c_void) -> OSStatus;
        fn SecItemUpdate(query: *const c_void, attrs: *const c_void) -> OSStatus;
        fn SecItemDelete(query: *const c_void) -> OSStatus;
        fn SecCopyErrorMessageString(status: OSStatus, reserved: *mut c_void) -> *const c_void;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    fn key(k: *const c_void) -> &'static AnyObject {
        // SAFETY: the Security constants are immortal CFStrings (toll-free NSStrings)
        unsafe { &*(k as *const AnyObject) }
    }

    fn dict() -> Retained<AnyObject> {
        // SAFETY: NSMutableDictionary new
        unsafe { msg_send![class!(NSMutableDictionary), new] }
    }

    fn put(d: &AnyObject, k: *const c_void, v: &AnyObject) {
        // SAFETY: setObject:forKey: with non-nil object and key
        unsafe {
            let _: () = msg_send![d, setObject: v, forKey: key(k)];
        }
    }

    fn yes() -> Retained<AnyObject> {
        // SAFETY: NSNumber numberWithBool: (bridged to kCFBooleanTrue)
        unsafe { msg_send![class!(NSNumber), numberWithBool: true] }
    }

    fn data(b: &[u8]) -> Retained<AnyObject> {
        // SAFETY: NSData copies the bytes
        unsafe { msg_send![class!(NSData), dataWithBytes: b.as_ptr().cast::<c_void>(), length: b.len()] }
    }

    fn service(vault: &str) -> String {
        format!("{}:{vault}", crate::secrets::APP)
    }

    fn base(vault: &str) -> Retained<AnyObject> {
        let d = dict();
        put(&d, unsafe { kSecClass }, key(unsafe { kSecClassGenericPassword }));
        put(&d, unsafe { kSecAttrService }, &ns_string(&service(vault)));
        d
    }

    fn err(what: &str, st: OSStatus) -> String {
        // SAFETY: SecCopyErrorMessageString returns a +1 CFString or NULL
        let msg = unsafe {
            let s = SecCopyErrorMessageString(st, std::ptr::null_mut());
            let r = rust_string((s as *const AnyObject).as_ref());
            if !s.is_null() {
                CFRelease(s);
            }
            r
        };
        format!("secret storage: keychain {what}: OSStatus {st}{}", msg.map(|m| format!(" ({m})")).unwrap_or_default())
    }

    /// the object SecItemCopyMatching stored in `out` with errSecSuccess. Security documents a +1
    /// object whenever a kSecReturn* key was set, but nothing enforces that: a NULL fails closed here
    /// instead of being dereferenced (CodeQL 1722/1723, rust/access-invalid-pointer)
    pub(super) fn copied<'a>(out: *const c_void, what: &str) -> Result<&'a AnyObject, String> {
        // SAFETY: out is NULL or the live +1 CF object SecItemCopyMatching stored; as_ref checks NULL
        unsafe { (out as *const AnyObject).as_ref() }.ok_or_else(|| format!("secret storage: keychain {what}: success without a result"))
    }

    /// the ids stored for this vault, sorted
    pub fn ids(vault: &str) -> Result<Vec<String>, String> {
        let q = base(vault);
        put(&q, unsafe { kSecReturnAttributes }, &yes());
        put(&q, unsafe { kSecMatchLimit }, key(unsafe { kSecMatchLimitAll }));
        let mut out: *const c_void = std::ptr::null();
        // SAFETY: query is a valid dictionary; out receives a +1 CFArray<CFDictionary>
        let st = unsafe { SecItemCopyMatching(Retained::as_ptr(&q).cast(), &mut out) };
        if st == NOT_FOUND {
            return Ok(vec![]);
        }
        if st != 0 {
            return Err(err("list", st));
        }
        let arr = copied(out, "list")?;
        let mut ids = vec![];
        // SAFETY: arr (= out) is an NSArray of NSDictionary (released below)
        unsafe {
            let n: usize = msg_send![arr, count];
            for i in 0..n {
                let item: *const AnyObject = msg_send![arr, objectAtIndex: i];
                let acct: *const AnyObject = msg_send![item, objectForKey: key(kSecAttrAccount)];
                if let Some(s) = rust_string(acct.as_ref()) {
                    ids.push(s);
                }
            }
            CFRelease(out);
        }
        ids.sort();
        ids.dedup();
        Ok(ids)
    }

    /// one value (None = no such id)
    pub fn get(vault: &str, id: &str) -> Result<Option<String>, String> {
        let q = base(vault);
        put(&q, unsafe { kSecAttrAccount }, &ns_string(id));
        put(&q, unsafe { kSecReturnData }, &yes());
        put(&q, unsafe { kSecMatchLimit }, key(unsafe { kSecMatchLimitOne }));
        let mut out: *const c_void = std::ptr::null();
        // SAFETY: as in ids(); out is a +1 CFData
        let st = unsafe { SecItemCopyMatching(Retained::as_ptr(&q).cast(), &mut out) };
        if st == NOT_FOUND {
            return Ok(None);
        }
        if st != 0 {
            return Err(err("read", st));
        }
        let d = copied(out, "read")?;
        // SAFETY: d (= out) is an NSData (released below)
        let v = unsafe {
            let len: usize = msg_send![d, length];
            let p: *const u8 = msg_send![d, bytes];
            let v = if len == 0 { Vec::new() } else { std::slice::from_raw_parts(p, len).to_vec() };
            CFRelease(out);
            v
        };
        String::from_utf8(v).map(Some).map_err(|_| "secret storage: keychain read: the stored value is not UTF-8".to_string())
    }

    pub fn load(vault: &str) -> Result<std::collections::BTreeMap<String, String>, String> {
        let mut m = std::collections::BTreeMap::new();
        for id in ids(vault)? {
            if let Some(v) = get(vault, &id)? {
                m.insert(id, v);
            }
        }
        Ok(m)
    }

    pub fn set(vault: &str, id: &str, value: &str) -> Result<(), String> {
        let q = base(vault);
        put(&q, unsafe { kSecAttrAccount }, &ns_string(id));
        let upd = dict();
        put(&upd, unsafe { kSecValueData }, &data(value.as_bytes()));
        // SAFETY: valid dictionaries
        let st = unsafe { SecItemUpdate(Retained::as_ptr(&q).cast(), Retained::as_ptr(&upd).cast()) };
        if st == 0 {
            return Ok(());
        }
        if st != NOT_FOUND {
            return Err(err("update", st));
        }
        put(&q, unsafe { kSecAttrLabel }, &ns_string(&crate::secrets::label(vault, id)));
        put(&q, unsafe { kSecValueData }, &data(value.as_bytes()));
        // SAFETY: valid dictionary, no result wanted
        let st = unsafe { SecItemAdd(Retained::as_ptr(&q).cast(), std::ptr::null_mut()) };
        if st == DUPLICATE {
            // a concurrent set added the item between our update and add: update it now
            let q = base(vault);
            put(&q, unsafe { kSecAttrAccount }, &ns_string(id));
            // SAFETY: valid dictionaries
            let st = unsafe { SecItemUpdate(Retained::as_ptr(&q).cast(), Retained::as_ptr(&upd).cast()) };
            return if st == 0 { Ok(()) } else { Err(err("update", st)) };
        }
        if st != 0 {
            return Err(err("add", st));
        }
        Ok(())
    }

    pub fn delete(vault: &str, id: &str) -> Result<(), String> {
        let q = base(vault);
        put(&q, unsafe { kSecAttrAccount }, &ns_string(id));
        // SAFETY: valid dictionary
        let st = unsafe { SecItemDelete(Retained::as_ptr(&q).cast()) };
        if st == 0 || st == NOT_FOUND {
            Ok(())
        } else {
            Err(err("delete", st))
        }
    }
}

/* ---------------------------------------------------------------- clipboard (NSPasteboard) */

/// the general pasteboard's plain text (clip.rs's GTK CLIPBOARD read); main thread only
pub fn clipboard_text() -> Option<String> {
    // SAFETY: NSPasteboard on the main thread (clip.rs runs this through run_on_main_thread)
    unsafe {
        let pb: *mut AnyObject = msg_send![class!(NSPasteboard), generalPasteboard];
        if pb.is_null() {
            return None;
        }
        let s: *mut AnyObject = msg_send![pb, stringForType: &*ns_string("public.utf8-plain-text")];
        rust_string(s.as_ref())
    }
}

/// put `text` on the general pasteboard as plain text (clip.rs's GTK CLIPBOARD set); true = written
pub fn clipboard_set_text(text: &str) -> bool {
    // SAFETY: NSPasteboard; clearContents + setString:forType: as AppKit documents for a write
    unsafe {
        let pb: *mut AnyObject = msg_send![class!(NSPasteboard), generalPasteboard];
        if pb.is_null() {
            return false;
        }
        let _: isize = msg_send![pb, clearContents];
        msg_send![pb, setString: &*ns_string(text), forType: &*ns_string("public.utf8-plain-text")]
    }
}

/* ---------------------------------------------------------------- page webview (WKWebView) */

/// set a boolean WKPreferences setting of the live page webview by setter name (the WebKitSettings
/// calls of the Linux build). Several are WebKit SPI (`_setWebGLEnabled:`): checked with
/// respondsToSelector, Err(why) if this macOS does not have it — never a silent success.
/// Returns the value the getter reported before the change, when there is one.
pub fn webview_pref(wv: *mut std::ffi::c_void, setter: &str, getter: &str, on: bool) -> Result<Option<bool>, String> {
    let wv = wv as *mut AnyObject;
    if wv.is_null() {
        return Err("no page webview".into());
    }
    // SAFETY: WKWebView on the main thread (with_webview); configuration.preferences is the live
    // WKPreferences object of the page (shared, not copied, by the configuration copy)
    unsafe {
        let cfg: *mut AnyObject = msg_send![wv, configuration];
        let prefs: *mut AnyObject = msg_send![cfg, preferences];
        let prefs = prefs.as_ref().ok_or("no WKPreferences")?;
        let set = objc2::runtime::Sel::register(&std::ffi::CString::new(setter).map_err(|e| e.to_string())?);
        if !responds(prefs, set) {
            return Err(format!("WKPreferences has no {setter} on this macOS"));
        }
        let get = objc2::runtime::Sel::register(&std::ffi::CString::new(getter).map_err(|e| e.to_string())?);
        use objc2::runtime::MessageReceiver;
        use objc2::runtime::Bool;
        let before: Option<bool> = if responds(prefs, get) { Some(MessageReceiver::send_message::<(), Bool>(prefs, get, ()).as_bool()) } else { None };
        let _: () = MessageReceiver::send_message(prefs, set, (Bool::new(on),));
        Ok(before)
    }
}

thread_local! {
    /// the canvas-off user script WE added to the page (main thread only, like the GTK twin)
    static OUR_SCRIPT: std::cell::RefCell<Option<Retained<AnyObject>>> = const { std::cell::RefCell::new(None) };
}

/// add (or remove) OUR user script, all frames, document start, to the page's content controller.
/// WKUserContentController can only remove ALL scripts, so a removal re-adds every script that is
/// not ours (tauri's IPC init scripts live there too and must survive)
pub fn user_script(controller: *mut std::ffi::c_void, source: Option<&str>) -> Result<(), String> {
    let ucc = controller as *mut AnyObject;
    if ucc.is_null() {
        return Err("no user content controller".into());
    }
    // SAFETY: WKUserContentController / WKUserScript on the main thread
    unsafe {
        OUR_SCRIPT.with(|o| {
            let mut o = o.borrow_mut();
            if let Some(old) = o.take() {
                let all: Retained<AnyObject> = msg_send![ucc, userScripts];
                let all: Retained<AnyObject> = msg_send![&*all, copy];
                let _: () = msg_send![ucc, removeAllUserScripts];
                let n: usize = msg_send![&*all, count];
                for i in 0..n {
                    let s: *mut AnyObject = msg_send![&*all, objectAtIndex: i];
                    if !std::ptr::eq(s, Retained::as_ptr(&old)) {
                        let _: () = msg_send![ucc, addUserScript: s];
                    }
                }
            }
            if let Some(src) = source {
                let a: Allocated<AnyObject> = msg_send![class!(WKUserScript), alloc];
                let us: Retained<AnyObject> =
                    msg_send![a, initWithSource: &*ns_string(src), injectionTime: 0isize /* AtDocumentStart */, forMainFrameOnly: false];
                let _: () = msg_send![ucc, addUserScript: &*us];
                *o = Some(us);
            }
        });
    }
    Ok(())
}

/// #144 toggle 9 on macOS: a local NSEvent monitor on key-down, installed once. While `off()` a
/// clipboard binding (Cmd+C / Cmd+X / Cmd+V, Shift too) aimed at `ns_window` is swallowed before
/// AppKit's menu or WebKit sees it, and `refused(what)` is told (the page notice). The page cannot
/// reach it. Other windows and keys pass untouched.
pub fn clip_key_monitor(ns_window: usize, off: impl Fn() -> bool + 'static, refused: impl Fn(&str) + 'static) {
    let block = block2::RcBlock::new(move |ev: std::ptr::NonNull<AnyObject>| -> *mut AnyObject {
        // SAFETY: the monitor hands a valid NSEvent on the main thread
        unsafe {
            let e = ev.as_ref();
            if off() {
                let w: *mut AnyObject = msg_send![e, window];
                let flags: usize = msg_send![e, modifierFlags];
                let cmd = flags & (1 << 20) != 0; // NSEventModifierFlagCommand
                let ch: *mut AnyObject = msg_send![e, charactersIgnoringModifiers];
                let k = rust_string(ch.as_ref()).unwrap_or_default().to_lowercase();
                if w as usize == ns_window && cmd && matches!(k.as_str(), "c" | "x" | "v") {
                    refused(&format!("clipboard key Cmd+{}", k.to_uppercase()));
                    return std::ptr::null_mut();
                }
            }
            ev.as_ptr()
        }
    });
    // SAFETY: AppKit on the main thread; the monitor (and the block it retains) lives for the app
    unsafe {
        let m: *mut AnyObject = msg_send![class!(NSEvent), addLocalMonitorForEventsMatchingMask: 1u64 << 10 /* NSEventMaskKeyDown */, handler: &*block];
        std::mem::forget(m);
    }
}

/* ---------------------------------------------------------------- microphone gate (WKUIDelegate) */

/// WKWebView -> its window label, for the media gate (registered by audiorec::install*)
static MEDIA_WEBVIEWS: std::sync::Mutex<Vec<(usize, String)>> = std::sync::Mutex::new(Vec::new());
static MEDIA_DECIDE: std::sync::OnceLock<fn(&str, bool, bool) -> bool> = std::sync::OnceLock::new();

/// The macOS twin of audiorec.rs's WebKitGTK permission-request handler. wry's WKUIDelegate
/// answers EVERY getUserMedia with Grant; this replaces that one method on wry's delegate class
/// (class_replaceMethod, once) with a gate that asks `decide(label, audio, video)` for the webview
/// the request came from — an unregistered webview (the app page, any other) is Deny.
/// Main thread (with_webview).
pub fn media_gate(wv: *mut std::ffi::c_void, label: &str, decide: fn(&str, bool, bool) -> bool) {
    let wv = wv as *mut AnyObject;
    if wv.is_null() {
        return eprintln!("[audiorec] media gate: no webview");
    }
    if let Ok(mut m) = MEDIA_WEBVIEWS.lock() {
        m.retain(|(p, _)| *p != wv as usize);
        m.push((wv as usize, label.to_string()));
    }
    if MEDIA_DECIDE.set(decide).is_err() {
        return; // the method is already ours (one class for every wry webview)
    }
    // SAFETY: Objective-C runtime calls on the delegate's class; the replacement has the exact
    // signature of webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:
    unsafe {
        let del: *mut AnyObject = msg_send![wv, UIDelegate];
        if del.is_null() {
            return eprintln!("[audiorec] media gate: the webview has no UI delegate");
        }
        let cls = objc2::ffi::object_getClass(del);
        let sel = objc2::sel!(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:);
        let imp: unsafe extern "C-unwind" fn(*mut AnyObject, objc2::runtime::Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject, isize, *mut block2::Block<dyn Fn(isize)>) = gate_imp;
        let types = c"v@:@@@q@?";
        objc2::ffi::class_replaceMethod(cls as *mut AnyClass, sel, std::mem::transmute::<_, objc2::runtime::Imp>(imp), types.as_ptr());
        eprintln!("[audiorec] media gate installed on {}", std::ffi::CStr::from_ptr(objc2::ffi::object_getClassName(del)).to_string_lossy());
    }
}

/// the webview is gone: its label must not answer for a later webview at the same address
pub fn media_gate_forget(wv: *mut std::ffi::c_void) {
    if let Ok(mut m) = MEDIA_WEBVIEWS.lock() {
        m.retain(|(p, _)| *p != wv as usize);
    }
}

unsafe extern "C-unwind" fn gate_imp(
    _this: *mut AnyObject,
    _cmd: objc2::runtime::Sel,
    webview: *mut AnyObject,
    _origin: *mut AnyObject,
    _frame: *mut AnyObject,
    kind: isize,
    handler: *mut block2::Block<dyn Fn(isize)>,
) {
    // WKMediaCaptureType: 0 camera, 1 microphone, 2 camera + microphone
    let (audio, video) = (kind == 1 || kind == 2, kind == 0 || kind == 2);
    let label = MEDIA_WEBVIEWS.lock().ok().and_then(|m| m.iter().find(|(p, _)| *p == webview as usize).map(|(_, l)| l.clone()));
    let allow = match (&label, MEDIA_DECIDE.get()) {
        (Some(l), Some(d)) => d(l, audio, video),
        _ => false,
    };
    eprintln!("[audiorec] user-media request {} audio={audio} video={video} display=false -> {}", label.as_deref().unwrap_or("<unknown webview>"), if allow { "Allow" } else { "Deny" });
    // WKPermissionDecision: 1 grant, 2 deny
    if let Some(h) = handler.as_ref() {
        h.call((if allow { 1 } else { 2 },));
    }
}

/* ---------------------------------------------------------------- file drops (NSDraggingDestination) */

type DropSink = Box<dyn Fn(Vec<std::path::PathBuf>, f64, f64) + Send + Sync>;
static DROP_SINK: std::sync::OnceLock<DropSink> = std::sync::OnceLock::new();
static ORIG_PERFORM: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

/// The macOS twin of xdnd.rs (#51): a drop is CLAIMED only when it names local files — those go
/// to `sink(paths, x, y)` (x/y = the drop point in the webview, CSS px, top-left origin) exactly as
/// the GTK transport hands them on. Anything else (a browser link drag) falls through to wry's own
/// performDragOperation, which (tauri's handler being disabled) is WebKit's: the DOM drop reaches
/// the editor. Replaces performDragOperation: on the webview's class once; main thread.
pub fn file_drop_hook(wv: *mut std::ffi::c_void, sink: impl Fn(Vec<std::path::PathBuf>, f64, f64) + Send + Sync + 'static) {
    let wv = wv as *mut AnyObject;
    if wv.is_null() || DROP_SINK.set(Box::new(sink)).is_err() {
        return;
    }
    // SAFETY: Objective-C runtime on the webview's class; the replacement has the exact signature
    // of -[NSView(NSDraggingDestination) performDragOperation:] and calls the original otherwise
    unsafe {
        let cls = objc2::ffi::object_getClass(wv);
        let sel = objc2::sel!(performDragOperation:);
        let imp: unsafe extern "C-unwind" fn(*mut AnyObject, objc2::runtime::Sel, *mut AnyObject) -> objc2::runtime::Bool = drop_imp;
        let old = objc2::ffi::class_replaceMethod(cls as *mut AnyClass, sel, std::mem::transmute::<_, objc2::runtime::Imp>(imp), c"c@:@".as_ptr());
        if let Some(old) = old {
            let _ = ORIG_PERFORM.set(old as usize);
        }
        eprintln!("[xdnd] macOS file-drop transport on {}", std::ffi::CStr::from_ptr(objc2::ffi::object_getClassName(wv)).to_string_lossy());
    }
}

unsafe extern "C-unwind" fn drop_imp(this: *mut AnyObject, cmd: objc2::runtime::Sel, info: *mut AnyObject) -> objc2::runtime::Bool {
    let pb: *mut AnyObject = msg_send![info, draggingPasteboard];
    let mut paths = Vec::new();
    if !pb.is_null() {
        let list: *mut AnyObject = msg_send![pb, propertyListForType: &*ns_string("NSFilenamesPboardType")];
        if !list.is_null() {
            let n: usize = msg_send![list, count];
            for i in 0..n {
                let s: *mut AnyObject = msg_send![list, objectAtIndex: i];
                if let Some(p) = rust_string(s.as_ref()) {
                    paths.push(std::path::PathBuf::from(p));
                }
            }
        }
    }
    if !paths.is_empty() {
        if let Some(sink) = DROP_SINK.get() {
            let at: Point = msg_send![info, draggingLocation];
            let nil: *mut AnyObject = std::ptr::null_mut();
            let v: Point = msg_send![this, convertPoint: at, fromView: nil];
            let flipped: bool = msg_send![this, isFlipped];
            let fr: Rect = msg_send![this, frame];
            let y = if flipped { v.y } else { fr.size.h - v.y };
            eprintln!("[xdnd] drop of {} local file(s) at {:.0},{:.0}: claimed", paths.len(), v.x, y);
            sink(paths, v.x, y);
            return objc2::runtime::Bool::YES;
        }
    }
    match ORIG_PERFORM.get() {
        Some(&old) => {
            let f: unsafe extern "C-unwind" fn(*mut AnyObject, objc2::runtime::Sel, *mut AnyObject) -> objc2::runtime::Bool = std::mem::transmute(old);
            f(this, cmd, info)
        }
        None => objc2::runtime::Bool::NO,
    }
}

#[cfg(test)]
mod tests {
    /// the Keychain store round-trips under a unique vault key and leaves nothing behind
    #[test]
    fn keychain_roundtrip() {
        use super::keychain as kc;
        let v = format!("/tmp/opensidian-kc-test-{}", std::process::id());
        kc::set(&v, "k-one", "value-1").expect("set");
        assert_eq!(kc::get(&v, "k-one").expect("get"), Some("value-1".into()));
        kc::set(&v, "k-one", "value-2").expect("update");
        assert_eq!(kc::get(&v, "k-one").expect("get2"), Some("value-2".into()));
        assert_eq!(kc::ids(&v).expect("ids"), vec!["k-one".to_string()]);
        kc::delete(&v, "k-one").expect("delete");
        assert_eq!(kc::get(&v, "k-one").expect("get3"), None);
        assert!(kc::ids(&v).expect("ids2").is_empty());
    }

    /// CodeQL 1722/1723: a success status with a NULL result fails closed instead of being
    /// dereferenced (the non-NULL path is keychain_roundtrip above)
    #[test]
    fn keychain_success_without_result_is_an_error() {
        use super::keychain as kc;
        for what in ["list", "read"] {
            let e = kc::copied(std::ptr::null(), what).expect_err("NULL result must be an error");
            assert_eq!(e, format!("secret storage: keychain {what}: success without a result"));
        }
    }
}
