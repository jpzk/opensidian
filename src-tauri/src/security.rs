// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* #144 Settings › Security — the ONE reader of the "security" key
   (docs/security-section.md §0).

   - Stored: ~/.opensidian.json "security" {...}, app-wide. Nothing under a vault
     is ever read here, so a foreign vault cannot ship a weaker value.
   - Written: only by cfgstore Op::SecuritySet (one validated boolean field per
     write, merged under the cfg lock); a generic write naming the key is refused
     in cfgstore::apply, like the spawn grants.
   - Read: `effective()` = stored values, then the lockdown overlay, then the
     precedence rules (3 offline overrides 1 and 2; 6 canvas off forces 5 WebGL
     off). Lockdown is an OVERLAY: the stored per-toggle values are never
     rewritten by it, so turning it off restores what the user had.
   - Restart-bound fields (jit, lockdown, lockdownClipboard) are LATCHED at
     process start by `boot()`; the running process keeps the latched value.
   - Missing / malformed key or field = the default; unknown fields are ignored.
     The defaults reproduce today's behaviour (issue rule "Defaults = today"). */
use serde::Serialize;
use serde_json::Value;
use std::sync::OnceLock;

pub const KEY: &str = "security";

/// (stored field, default, strict value, user-visible name). The name is the
/// one every refusal and placeholder carries: "(Settings › Security › <name>)".
pub const FIELDS: &[(&str, bool, bool, &str)] = &[
    ("remoteMedia", false, false, "Load external resources"),
    ("pluginNet", true, false, "Cut off network for all community plugins"),
    ("offline", false, true, "Cut off all network (offline mode)"),
    ("vaultMedia", true, false, "Render media"),
    ("webgl", true, false, "WebGL"),
    ("canvas", true, false, "HTML canvas"),
    ("jit", true, false, "JavaScript JIT"),
    ("pluginClipboard", true, false, "Clipboard for community plugins"),
    ("appClipboard", true, false, "App clipboard"),
    ("pluginSpawn", true, false, "Community plugins may start programs"),
    ("mathjax", true, false, "MathJax"),
    ("prism", true, false, "Prism"),
    ("mermaid", true, false, "Mermaid"),
    ("lockdown", false, true, "Lockdown mode"),
    ("lockdownClipboard", false, true, "Lockdown mode › also turn off the clipboard"),
];

/// fields that are read once at process start (docs §0 "Restart-bound values")
pub const RESTART_BOUND: &[&str] = &["jit", "lockdown", "lockdownClipboard"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sec {
    pub remote_media: bool,
    pub plugin_net: bool,
    pub offline: bool,
    pub vault_media: bool,
    pub webgl: bool,
    pub canvas: bool,
    pub jit: bool,
    pub plugin_clipboard: bool,
    pub app_clipboard: bool,
    pub plugin_spawn: bool,
    pub mathjax: bool,
    pub prism: bool,
    pub mermaid: bool,
    pub lockdown: bool,
    pub lockdown_clipboard: bool,
}

impl Default for Sec {
    fn default() -> Self {
        let mut s = Sec::all(false);
        for (f, d, _, _) in FIELDS {
            *s.field_mut(f).expect("known field") = *d;
        }
        s
    }
}

impl Sec {
    fn all(b: bool) -> Self {
        Sec {
            remote_media: b,
            plugin_net: b,
            offline: b,
            vault_media: b,
            webgl: b,
            canvas: b,
            jit: b,
            plugin_clipboard: b,
            app_clipboard: b,
            plugin_spawn: b,
            mathjax: b,
            prism: b,
            mermaid: b,
            lockdown: b,
            lockdown_clipboard: b,
        }
    }
    pub fn field_mut(&mut self, f: &str) -> Option<&mut bool> {
        Some(match f {
            "remoteMedia" => &mut self.remote_media,
            "pluginNet" => &mut self.plugin_net,
            "offline" => &mut self.offline,
            "vaultMedia" => &mut self.vault_media,
            "webgl" => &mut self.webgl,
            "canvas" => &mut self.canvas,
            "jit" => &mut self.jit,
            "pluginClipboard" => &mut self.plugin_clipboard,
            "appClipboard" => &mut self.app_clipboard,
            "pluginSpawn" => &mut self.plugin_spawn,
            "mathjax" => &mut self.mathjax,
            "prism" => &mut self.prism,
            "mermaid" => &mut self.mermaid,
            "lockdown" => &mut self.lockdown,
            "lockdownClipboard" => &mut self.lockdown_clipboard,
            _ => return None,
        })
    }
    pub fn field(&self, f: &str) -> Option<bool> {
        let mut c = *self;
        c.field_mut(f).map(|b| *b)
    }
    /// the stored values out of the WHOLE config: field by field, so one malformed
    /// field falls back to its own default instead of resetting the others
    pub fn from_cfg(cfg: &Value) -> Self {
        let mut s = Sec::default();
        let o = &cfg[KEY];
        for (f, _, _, _) in FIELDS {
            if let Some(b) = o.get(*f).and_then(Value::as_bool) {
                *s.field_mut(f).expect("known field") = b;
            }
        }
        s
    }
}

/// what the boundaries ask: `effective()`
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Eff {
    pub sec: Sec,
    /// lockdown as latched at start (restart-bound)
    pub lockdown: bool,
}

/// the boot latch: (jit, lockdown, lockdownClipboard) as read once in main()
static BOOT: OnceLock<Sec> = OnceLock::new();

/// latch the restart-bound fields; called once in main() before any window exists.
/// A second call keeps the first latch.
pub fn boot(cfg: &Value) -> Sec {
    *BOOT.get_or_init(|| Sec::from_cfg(cfg))
}

pub fn booted() -> Option<Sec> {
    BOOT.get().copied()
}

/// stored -> effective, pure (tests): `latched` = the boot latch (None = not
/// latched yet: the stored values stand in for it)
pub fn effective_with(stored: Sec, latched: Option<Sec>) -> Eff {
    let mut s = stored;
    if let Some(b) = latched {
        s.jit = b.jit;
        s.lockdown = b.lockdown;
        s.lockdown_clipboard = b.lockdown_clipboard;
    }
    if s.lockdown {
        // issue §12: everything strict, except 9 unless "also turn off the clipboard"
        let app_clip = s.app_clipboard && !s.lockdown_clipboard;
        for (f, _, strict, _) in FIELDS {
            if !matches!(*f, "lockdown" | "lockdownClipboard" | "appClipboard") {
                *s.field_mut(f).expect("known field") = *strict;
            }
        }
        s.app_clipboard = app_clip;
    }
    // precedence (docs §1): offline forces 1 off and 2 to deny; canvas off forces WebGL off
    if s.offline {
        s.remote_media = false;
        s.plugin_net = false;
    }
    if !s.canvas {
        s.webgl = false;
    }
    Eff { sec: s, lockdown: s.lockdown }
}

pub fn effective(cfg: &Value) -> Eff {
    effective_with(Sec::from_cfg(cfg), booted())
}

/// the user-visible name of a field, as refusals carry it
pub fn name(f: &str) -> &'static str {
    FIELDS.iter().find(|x| x.0 == f).map(|x| x.3).unwrap_or("?")
}

/// the sentence every refusal / placeholder ends with
pub fn named(f: &str) -> String {
    format!("(Settings › Security › {})", name(f))
}

/// why the effective value of `f` differs from the stored one (None = it does not)
pub fn forced_by(stored: Sec, eff: &Eff, f: &str) -> Option<&'static str> {
    let (sv, ev) = (stored.field(f)?, eff.sec.field(f)?);
    if sv == ev {
        return None;
    }
    if RESTART_BOUND.contains(&f) {
        return Some("restart");
    }
    if eff.lockdown {
        return Some("lockdown");
    }
    match f {
        "remoteMedia" | "pluginNet" if eff.sec.offline => Some("offline"),
        "webgl" if !eff.sec.canvas => Some("canvas"),
        _ => None,
    }
}

/* toggle 3 — THE NETWORK CHOKE POINT. Every outbound path the app has calls
   `net_check` first (plugin_store::http_get = store / builtins / update check,
   desk_http_request, desk_net_listen, desk_shell's URL opens), and
   main.rs `every_outbound_path_goes_through_the_choke_point` fails T0 when a
   network constructor appears anywhere else or a caller skips the check. */
pub const OFFLINE_IS_ON: &str = "offline mode is on";

/// Err while offline mode is on: "<what>: offline mode is on (Settings › Security › ...)"
pub fn net_check(eff: &Eff, what: &str) -> Result<(), String> {
    if eff.sec.offline {
        return Err(format!("{what}: {OFFLINE_IS_ON} {}", named("offline")));
    }
    Ok(())
}

/// the confirm before the system browser opens a link while offline (operator decision:
/// allowed, with a confirm). `url` passed main.rs check_external (http/https/mailto).
pub fn offline_open_text(url: &str) -> String {
    format!(
        "Offline mode is on {}.\n\nOpen this address in your web browser? The browser uses \
         the network; the app stays offline.\n\nAddress: {url:?}",
        named("offline")
    )
}

/* toggle 2 — community-plugin network. The plugin network commands (main.rs
   desk_http_request = requestUrl / request / http.request, desk_net_listen =
   net/http.createServer) ask `plugin_net_check`, which asks the offline choke
   point FIRST (offline wins, and its refusal is the one shown), then this toggle.
   fetch / XHR / WebSocket to a remote host are refused by the page CSP whatever
   the toggle says (connect-src has no remote origin; T0 pins that in main.rs).
   `plugin` is a LABEL for the message (all plugins share one realm, the id is a
   claim): the refusal never depends on it. */
pub const PLUGIN_NET_OFF: &str = "network is off for community plugins";

/// a plugin id for a message: [A-Za-z0-9._-], at most 64 chars, "" when nothing is left
pub fn plugin_label(p: &str) -> String {
    p.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')).take(64).collect()
}

/// Err when offline mode is on (net_check) or the plugin network toggle cuts it:
/// "EPERM: <what>: network is off for community plugins (Settings › Security › ...) — plugin <id>"
pub fn plugin_net_check(eff: &Eff, what: &str, plugin: Option<&str>) -> Result<(), String> {
    net_check(eff, what)?;
    if !eff.sec.plugin_net {
        let who = plugin.map(plugin_label).filter(|p| !p.is_empty()).map(|p| format!(" — plugin {p}")).unwrap_or_default();
        return Err(format!("EPERM: {what}: {PLUGIN_NET_OFF} {}{who}", named("pluginNet")));
    }
    Ok(())
}


/* toggle 10 — community plugins may start programs. The two plugin commands that
   start a program (main.rs desk_proc_spawn = child_process.spawn/exec/execFile,
   desk_shell = electron shell openExternal/openPath/showItemInFolder and
   app.openWithDefaultApp, all of which exec xdg-open) ask `spawn_check` right after
   the restricted-mode check and BEFORE consent::admit: when off no question is
   queued and no native dialog is created. `plugin` is a label only (one realm). */
pub const SPAWN_OFF: &str = "community plugins may not start programs";

/// Err when toggle 10 is off (or lockdown forces it off):
/// "EPERM: <what>: community plugins may not start programs (Settings › Security › ...) — plugin <id>"
pub fn spawn_check(eff: &Eff, what: &str, plugin: Option<&str>) -> Result<(), String> {
    if !eff.sec.plugin_spawn {
        let who = plugin.map(plugin_label).filter(|p| !p.is_empty()).map(|p| format!(" — plugin {p}")).unwrap_or_default();
        return Err(format!("EPERM: {what}: {SPAWN_OFF} {}{who}", named("pluginSpawn")));
    }
    Ok(())
}
/// a requested change, validated: (field, value). Unknown field = refused.
pub fn validate(field: &str, on: bool) -> Result<(&'static str, bool), String> {
    FIELDS
        .iter()
        .find(|x| x.0 == field)
        .map(|x| (x.0, on))
        .ok_or_else(|| format!("unknown security setting {field:?}"))
}

/// a change that LOWERS protection (moves a field away from its strict value):
/// these need the native confirm (consent Kind::Security), so plugin code in the
/// page cannot quietly switch a protection off through the same command
pub fn weakens(stored: Sec, field: &str, on: bool) -> bool {
    match (stored.field(field), FIELDS.iter().find(|x| x.0 == field)) {
        (Some(cur), Some((_, _, strict, _))) => cur != on && on != *strict,
        _ => false,
    }
}

/// what security_get hands the UI (and, read-only, plugins)
pub fn report(cfg: &Value) -> Value {
    let stored = Sec::from_cfg(cfg);
    let eff = effective(cfg);
    let mut forced = serde_json::Map::new();
    for (f, _, _, _) in FIELDS {
        if let Some(why) = forced_by(stored, &eff, f) {
            forced.insert((*f).into(), Value::from(why));
        }
    }
    let pending: Vec<&str> = RESTART_BOUND
        .iter()
        .copied()
        .filter(|f| booted().is_some_and(|b| b.field(f) != stored.field(f)))
        .collect();
    let names: serde_json::Map<String, Value> = FIELDS.iter().map(|x| (x.0.to_string(), Value::from(x.3))).collect();
    serde_json::json!({
        "stored": stored,
        "effective": eff.sec,
        "lockdown": eff.lockdown,
        "forced": forced,
        "restartPending": pending,
        "names": names,
        // toggle 1 layer 2: did THIS page get the https: img/media CSP at load?
        "pageRemote": PAGE_REMOTE.load(std::sync::atomic::Ordering::SeqCst),
        // toggle 4 layer 2: does THIS page's CSP still carry opensidian-img: / blob:?
        "pageMedia": PAGE_MEDIA.load(std::sync::atomic::Ordering::SeqCst),
        // toggle 5: what the page webview's enable-webgl is right now (main.rs sec_webgl)
        "pageWebgl": PAGE_WEBGL.load(std::sync::atomic::Ordering::SeqCst),
        // toggles 8 + 9: has community code entered THIS page; what the native key gate enforces
        "realmPlugin": REALM_PLUGIN.load(std::sync::atomic::Ordering::SeqCst),
        "pageAppClipboard": PAGE_APPCLIP.load(std::sync::atomic::Ordering::SeqCst),
    })
}

/// the confirm text for a weakening change (built from Rust data only)
pub fn weaken_text(field: &str, on: bool) -> String {
    format!(
        "Setting: Settings › Security › {}\nNew value: {}\n\n\
         This lowers a protection. Confirm only if you changed it yourself in Settings: \
         page or plugin code cannot answer this window.",
        name(field),
        if on { "on" } else { "off" }
    )
}

/* ---- toggle 1: load external resources (docs §1 "1 — Load external resources").
   Two layers that must BOTH agree before a note fetches a remote byte:
     layer 1 the renderer: main.rs image_html mints an https <img>/<audio>/<video>
       only while `remote_on()` (a per-call flag the render commands set from the
       effective value); off = R29.4's banner + REMOTE_OFF naming this toggle.
     layer 2 the page CSP: `csp_remote` adds `https:` to img-src and media-src
       ONLY (script/worker/connect/frame/font untouched) while the effective value
       is on at page load. PAGE_REMOTE records which header THIS page got; the
       pane reloads the page when it disagrees with the effective value
       (ui/security.js), so turning the toggle off removes the origin at once. */
thread_local! {
    static REMOTE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
pub static PAGE_REMOTE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// layer 1: may the renderer on THIS thread mint a remote (https) element right now?
pub fn remote_on() -> bool {
    REMOTE.with(|c| c.get())
}

/// run `f` (a render) with layer 1 set to `on`; restores the previous value
pub fn with_remote<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let prev = REMOTE.with(|c| c.replace(on));
    let r = f();
    REMOTE.with(|c| c.set(prev));
    r
}

/// the reason a remote image / audio / video shows the placeholder while toggle 1 is off
pub fn remote_off_text() -> String {
    format!("(Settings › Security › {} is off)", name("remoteMedia"))
}

/// the reason an http:// (not https) target shows the placeholder while toggle 1 is on
pub const REMOTE_HTTP_ONLY_HTTPS: &str = "(Settings › Security › Load external resources: only https:// addresses load)";

/// layer 2: the page CSP with toggle 1 applied. Off = byte-identical input.
/// On = `https:` appended to img-src and media-src (added as `'self' https:` /
/// `'self' blob: https:` if absent); every other directive untouched.
pub fn csp_remote(csp: &str, on: bool) -> String {
    if !on {
        return csp.to_string();
    }
    let (mut img, mut media) = (false, false);
    let mut out: Vec<String> = Vec::new();
    for d in csp.split(';').map(str::trim).filter(|d| !d.is_empty()) {
        match d.split_whitespace().next().unwrap_or("") {
            "img-src" => {
                img = true;
                out.push(format!("{d} https:"));
            }
            "media-src" => {
                media = true;
                out.push(format!("{d} https:"));
            }
            _ => out.push(d.to_string()),
        }
    }
    if !img {
        out.push("img-src 'self' https:".into());
    }
    if !media {
        out.push("media-src 'self' blob: https:".into());
    }
    out.join("; ")
}

/* ---- toggle 4: render vault media (docs §1 "4 — Render vault media").
   The boundary is the BACKEND, asked per request with the effective value:
     main.rs IMG_SCHEME handler (vault images + PDF page PNGs) answers 403,
     media_read (audio / video bytes for embeds, views and canvas cards) and
     pdf_info refuse with `media_check`'s message — so no vault byte reaches
     WebKit's image decoder, GStreamer or the PDF worker (pdf.rs is never queued).
   Layer 2, the page CSP (`csp_media`): off drops `opensidian-img:` from img-src
   and `blob:` from media-src, recorded in PAGE_MEDIA; the pane reloads the page
   when it disagrees with the effective value (ui/security.js), like toggle 1.
   The renderer follows the page (thread-local MEDIA, set per render call) and
   paints the placeholder `<name> — media rendering is off (Settings › Security ›
   Render media)` instead of an element. Vault media only (operator decision):
   theme / snippet CSS images ride 'self'/data: and are not covered. The audio
   RECORDER (mic -> audio_save) decodes nothing from the vault and is not cut. */
pub const MEDIA_OFF: &str = "media rendering is off";

thread_local! {
    static MEDIA: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
}
/// which CSP THIS page got (true = today's: opensidian-img: + blob: present)
pub static PAGE_MEDIA: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// may the renderer on THIS thread mint a vault image / PDF / audio element right now?
pub fn media_on() -> bool {
    MEDIA.with(|c| c.get())
}

/// run `f` (a render) with the media flag set to `on`; restores the previous value
pub fn with_media<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let prev = MEDIA.with(|c| c.replace(on));
    let r = f();
    MEDIA.with(|c| c.set(prev));
    r
}

/// the placeholder sentence after a file name: "media rendering is off (Settings › Security › Render media)"
pub fn media_off_text() -> String {
    format!("{MEDIA_OFF} {}", named("vaultMedia"))
}

/// Err while toggle 4 is off: "EPERM: <what>: media rendering is off (Settings › Security › Render media)"
pub fn media_check(eff: &Eff, what: &str) -> Result<(), String> {
    if !eff.sec.vault_media {
        return Err(format!("EPERM: {what}: {}", media_off_text()));
    }
    Ok(())
}

/// layer 2: the page CSP with toggle 4 applied. On = byte-identical input.
/// Off = `opensidian-img:` removed from img-src and `blob:` from media-src; every
/// other directive (and every other source in those two) untouched.
pub fn csp_media(csp: &str, on: bool) -> String {
    if on {
        return csp.to_string();
    }
    csp.split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| {
            let drop = match d.split_whitespace().next().unwrap_or("") {
                "img-src" => "opensidian-img:",
                "media-src" => "blob:",
                _ => return d.to_string(),
            };
            d.split_whitespace().filter(|t| *t != drop).collect::<Vec<_>>().join(" ")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/* ---- toggle 5: WebGL (docs §1 "5 — WebGL"). Boundary: native — WebKitSettings
   enable-webgl on the page webview (main.rs sec_webgl), set at window creation from the
   effective value and again from security_set when the effective value moves (lockdown
   and HTML canvas off force it off in effective_with). With it off every
   getContext("webgl" | "webgl2" | "experimental-webgl") returns null for app and plugin
   code alike; a context made before the change keeps running until its page reloads.
   PAGE_WEBGL = what the webview's setting is right now (report pageWebgl). */
pub static PAGE_WEBGL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// the reason the graph names when it draws in 2D because of this toggle
pub const WEBGL_OFF: &str = "WebGL is off";

/// "WebGL is off (Settings › Security › WebGL)"
pub fn webgl_off_text() -> String {
    format!("{WEBGL_OFF} {}", named("webgl"))
}

/// must the webview's enable-webgl change? Some(new value) when the effective value and
/// the applied one disagree, None when nothing is to do
pub fn webgl_change(eff: &Eff, applied: bool) -> Option<bool> {
    (eff.sec.webgl != applied).then_some(eff.sec.webgl)
}

/* ---- toggles 8 + 9: clipboard (docs §1 "8 + 9"). Boundary: the BACKEND reads the page
   realm's state, not an app-held capability (same-realm code can exercise any capability the
   app holds in the page). REALM_PLUGIN = community code has entered THIS page: the
   opensidian-plugin: scheme handler sets it when it serves a plugin main.js (plugins::serve ->
   Served::Js), page_csp clears it for each new HTML page. clip_text / clip_has_text /
   clip_set_text (clip.rs) ask clip_check first:
     9 off -> refused for every caller; natively main.rs sec_clip_gate swallows the clipboard
              keys + middle-click paste in a GTK handler that runs before WebKit (page script
              cannot reach it) and sec_clip sets WebKitSettings javascript-can-access-clipboard
              false (execCommand copy/cut/paste refuse).
     8 off -> refused while the realm is tainted; a realm with no community code (restricted
              vault, no plugin enabled) keeps the app's menu rows. Ctrl+C/X/V stay (native,
              trusted events), the menu Copy/Cut switch to execCommand inside the click.
   PAGE_APPCLIP = what the native gate enforces right now (report pageAppClipboard). */
pub static REALM_PLUGIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static PAGE_APPCLIP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub const CLIP_PLUGIN_OFF: &str = "the clipboard is off for community plugins";
pub const CLIP_APP_OFF: &str = "the app clipboard is off";

/// Err when the clipboard command `what` may not run: 9 off always, 8 off in a tainted realm
/// "EPERM: <what>: the app clipboard is off (Settings › Security › App clipboard)"
pub fn clip_check(eff: &Eff, tainted: bool, what: &str) -> Result<(), String> {
    if !eff.sec.app_clipboard {
        return Err(format!("EPERM: {what}: {CLIP_APP_OFF} {}", named("appClipboard")));
    }
    if tainted && !eff.sec.plugin_clipboard {
        return Err(format!("EPERM: {what}: {CLIP_PLUGIN_OFF} {}", named("pluginClipboard")));
    }
    Ok(())
}

/// the notice the native gate raises when it swallows a clipboard key / middle-click
pub fn clip_key_text() -> String {
    format!("Copy and paste are off {}", named("appClipboard"))
}

/// is this key press a clipboard binding (GTK keysym, modifiers)? Ctrl+C/X/V (Ctrl+Shift+V too),
/// Ctrl+Insert, Shift+Insert, Shift+Delete, the keypad twins and the XF86 Copy/Cut/Paste keys
pub fn clip_key(keyval: u32, ctrl: bool, shift: bool) -> bool {
    const INSERT: [u32; 2] = [0xff63, 0xff9e];
    const DELETE: [u32; 2] = [0xffff, 0xff9f];
    match keyval {
        0x63 | 0x43 | 0x78 | 0x58 | 0x76 | 0x56 => ctrl,
        k if INSERT.contains(&k) => ctrl || shift,
        k if DELETE.contains(&k) => shift && !ctrl,
        0x1008ff57 | 0x1008ff58 | 0x1008ff6d => true,
        _ => false,
    }
}


/* ---- toggle 11: MathJax / Prism / Mermaid (docs §1 "11"). Boundary: the app scheme's
   resource hook (main.rs page_window -> on_web_resource_request) answers 403 for every
   request under /vendor/<engine>/ while that engine's switch is off, whoever asks
   (loadScript, a <script src>, import(), fetch, a worker): the parser bytes never reach the
   page. The loaders in ui/plugins/api-functions.js reject first with the same sentence (a
   clear message only; the 403 is the boundary). The served 200 carries no-store so a page
   reloaded after the switch went off cannot be fed the engine from WebKit's memory cache.
   Mermaid is not vendored on develop and the app's own rendering is #185-#187: the rule
   already covers /vendor/mermaid/ and any later engine loaded from there. */
pub const ENGINES: &[(&str, &str)] = &[("mathjax", "mathjax"), ("prism", "prism"), ("mermaid", "mermaid")];

/// the request path as the asset resolver will see it: %XX decoded, ASCII lowercased,
/// empty / "." segments dropped, ".." resolved — so "/vendor/%50rism/./x.js" or
/// "//VENDOR/a/../prism/x.js" still match (lowercasing only ever makes the gate stricter)
pub fn norm_path(p: &str) -> String {
    let b = p.as_bytes();
    let mut d = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                d.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        d.push(b[i]);
        i += 1;
    }
    let s = String::from_utf8_lossy(&d).to_ascii_lowercase().replace('\\', "/");
    let mut out: Vec<&str> = Vec::new();
    for seg in s.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            x => out.push(x),
        }
    }
    format!("/{}", out.join("/"))
}

/// which engine field a request path belongs to (None = not an engine's bytes)
pub fn engine_of(path: &str) -> Option<&'static str> {
    let n = norm_path(path);
    let mut segs = n.trim_start_matches('/').split('/');
    if segs.next() != Some("vendor") {
        return None;
    }
    let dir = segs.next()?;
    ENGINES.iter().find(|(d, _)| *d == dir).map(|(_, f)| *f)
}

/// "MathJax is off (Settings › Security › MathJax)"
pub fn engine_off_text(field: &str) -> String {
    format!("{} is off {}", name(field), named(field))
}

/// Some(refusal) when `path` is an engine's bytes and that engine is off (effective value)
pub fn engine_refusal(eff: &Eff, path: &str) -> Option<String> {
    let f = engine_of(path)?;
    (!eff.sec.field(f)?).then(|| engine_off_text(f))
}

/* #144 toggle 7 — JavaScript JIT. Boundary: the PROCESS ENVIRONMENT before the first
   webview exists. JavaScriptCore reads its options from JSC_<option> env vars when the
   WebKit web process starts, and the web process inherits this process's env. Off sets
   every JIT tier off (useJIT is the master switch on current JSC; the tier flags cover
   builds that read only those) plus the RegExp JIT, which is a JIT too. Restart-bound:
   main() applies the LATCHED effective value once (lockdown forces it off). This module
   reads no env (nothing_but_the_config_value_is_read): it only plans; main.rs applies.
   JIT_MARK: set next to the vars when WE set them. A window started from this process
   (switch / open vault / spawner) inherits the env; if JIT is on again by then, the
   child removes exactly the vars it finds marked, never a JSC_ var the user set. */
pub const JIT_OFF_ENV: &[(&str, &str)] = &[
    ("JSC_useJIT", "0"),
    ("JSC_useBaselineJIT", "0"),
    ("JSC_useDFGJIT", "0"),
    ("JSC_useFTLJIT", "0"),
    ("JSC_useRegExpJIT", "0"),
];
pub const JIT_MARK: &str = "OPENSIDIAN_JIT_OFF";

/// what main() does to the env: (set, remove). `jit` = the effective latched value,
/// `marked` = JIT_MARK was inherited (a parent opensidian set the vars)
pub fn jit_plan(jit: bool, marked: bool) -> (Vec<(&'static str, &'static str)>, Vec<&'static str>) {
    if !jit {
        let mut set = JIT_OFF_ENV.to_vec();
        set.push((JIT_MARK, "1"));
        return (set, Vec::new());
    }
    if marked {
        let mut rm: Vec<&str> = JIT_OFF_ENV.iter().map(|(k, _)| *k).collect();
        rm.push(JIT_MARK);
        return (Vec::new(), rm);
    }
    (Vec::new(), Vec::new())
}

/// the one log line main() prints (T3 secjit greps it)
pub fn jit_log(jit: bool) -> String {
    if jit {
        "[sec] jit=on".into()
    } else {
        format!("[sec] jit=off ({}=0 + tier flags) {}", JIT_OFF_ENV[0].0, named("jit"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// toggle 1 layer 2: off leaves the CSP byte-identical; on widens img-src and
    /// media-src by `https:` and NOTHING else (no http:, no wildcard, no script/connect change)
    #[test]
    fn remote_csp_widens_only_img_and_media() {
        let base = "default-src 'self'; script-src 'self' opensidian-plugin: 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
                    img-src 'self' data: opensidian-img:; font-src 'self'; connect-src ipc: http://ipc.localhost; media-src 'self' blob:";
        assert_eq!(csp_remote(base, false), base);
        let on = csp_remote(base, true);
        let dirs: Vec<&str> = on.split("; ").collect();
        let b: Vec<&str> = base.split("; ").collect();
        assert_eq!(dirs.len(), b.len(), "{on}");
        for (x, y) in dirs.iter().zip(&b) {
            if y.starts_with("img-src") || y.starts_with("media-src") {
                assert_eq!(*x, format!("{y} https:"));
            } else {
                assert_eq!(x, y, "toggle 1 touched a directive it must not: {x}");
            }
        }
        assert!(!on.contains("http:") || on.matches("http:").count() == base.matches("http:").count(), "{on}");
        assert!(!on.contains('*'), "{on}");
        // a header without the two directives gains them, restricted to 'self' + https:
        let m = csp_remote("default-src 'self'", true);
        assert_eq!(m, "default-src 'self'; img-src 'self' https:; media-src 'self' blob: https:");
    }

    /// toggle 1 layer 1: the per-call flag is scoped to the call and defaults off
    #[test]
    fn remote_flag_is_scoped() {
        assert!(!remote_on());
        assert!(with_remote(true, remote_on));
        assert!(!remote_on());
        assert!(remote_off_text().contains("Load external resources is off"));
    }

    /// toggle 4: default renders; off refuses naming the toggle; the per-call flag
    /// defaults ON (today) and is scoped; the CSP loses exactly opensidian-img: and blob:
    #[test]
    fn vault_media_toggle_refuses_and_strips_only_its_sources() {
        let eff = effective_with(Sec::default(), None);
        assert!(media_check(&eff, "media_read").is_ok(), "default must render (today)");
        let mut s = Sec::default();
        s.vault_media = false;
        let e = media_check(&effective_with(s, None), "media_read").unwrap_err();
        assert_eq!(e, "EPERM: media_read: media rendering is off (Settings › Security › Render media)");
        assert!(media_on());
        assert!(!with_media(false, media_on));
        assert!(media_on());
        let base = "default-src 'self'; script-src 'self' opensidian-plugin: 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
                    img-src 'self' data: opensidian-img:; font-src 'self'; connect-src ipc: http://ipc.localhost; media-src 'self' blob:";
        assert_eq!(csp_media(base, true), base);
        let off = csp_media(base, false);
        let (d, b): (Vec<&str>, Vec<&str>) = (off.split("; ").collect(), base.split("; ").collect());
        assert_eq!(d.len(), b.len(), "{off}");
        for (x, y) in d.iter().zip(&b) {
            match y.split_whitespace().next() {
                Some("img-src") => assert_eq!(*x, "img-src 'self' data:"),
                Some("media-src") => assert_eq!(*x, "media-src 'self'"),
                _ => assert_eq!(x, y, "toggle 4 touched a directive it must not: {x}"),
            }
        }
        // composes with toggle 1 either order without re-adding a vault source
        let both = csp_media(&csp_remote(base, true), false);
        assert!(both.contains("img-src 'self' data: https:") && both.contains("media-src 'self' https:"), "{both}");
        // lockdown turns it off
        let mut l = Sec::default();
        l.lockdown = true;
        assert!(media_check(&effective_with(l, None), "x").is_err());
    }

    /// toggle 2: default allows; off refuses naming the toggle (+ the plugin label, sanitized);
    /// offline wins and its refusal is the one shown; the label never decides
    #[test]
    fn plugin_net_toggle_refuses_naming_it_offline_first() {
        let d = Sec::default();
        assert!(plugin_net_check(&effective_with(d, Some(d)), "requestUrl", Some("x")).is_ok());
        let mut s = d;
        s.plugin_net = false;
        let e = effective_with(s, Some(s));
        let err = plugin_net_check(&e, "requestUrl", Some("evil\n<b>id")).unwrap_err();
        assert_eq!(err, "EPERM: requestUrl: network is off for community plugins (Settings › Security › Cut off network for all community plugins) — plugin evilbid");
        assert!(plugin_net_check(&e, "net.createServer", None).unwrap_err().ends_with("community plugins)"));
        assert!(plugin_net_check(&e, "requestUrl", Some("")).unwrap_err().ends_with("community plugins)"));
        assert_eq!(plugin_label(&"a".repeat(99)).len(), 64);
        // offline on (plugin net stored allow): offline's message
        let mut o = d;
        o.offline = true;
        let err = plugin_net_check(&effective_with(o, Some(o)), "requestUrl", Some("x")).unwrap_err();
        assert!(err.contains(OFFLINE_IS_ON) && !err.contains(PLUGIN_NET_OFF), "{err}");
        // lockdown cuts it too (it also forces offline, whose message wins)
        let mut l = d;
        l.lockdown = true;
        let le = effective_with(l, Some(l));
        assert!(!le.sec.plugin_net && plugin_net_check(&le, "requestUrl", None).unwrap_err().contains(OFFLINE_IS_ON));
    }

    /// toggles 8 + 9: 9 off refuses every caller, 8 off only a realm community code entered;
    /// defaults (both on) refuse nobody; lockdown cuts 8 and leaves 9 unless lockdownClipboard
    #[test]
    fn clipboard_toggles_refuse_by_realm_and_name_themselves() {
        let d = Sec::default();
        let e = effective_with(d, Some(d));
        assert!(clip_check(&e, false, "clip_text").is_ok());
        assert!(clip_check(&e, true, "clip_text").is_ok(), "default = today: plugins may use it");
        let mut p = d;
        p.plugin_clipboard = false;
        let e = effective_with(p, Some(p));
        assert!(clip_check(&e, false, "clip_text").is_ok(), "no community code: the app's rows work");
        assert_eq!(
            clip_check(&e, true, "clip_text").unwrap_err(),
            "EPERM: clip_text: the clipboard is off for community plugins (Settings › Security › Clipboard for community plugins)"
        );
        let mut a = d;
        a.app_clipboard = false;
        let e = effective_with(a, Some(a));
        for t in [false, true] {
            assert_eq!(
                clip_check(&e, t, "clip_set_text").unwrap_err(),
                "EPERM: clip_set_text: the app clipboard is off (Settings › Security › App clipboard)"
            );
        }
        let mut l = d;
        l.lockdown = true;
        let e = effective_with(l, Some(l));
        assert!(clip_check(&e, false, "clip_text").is_ok());
        assert!(clip_check(&e, true, "clip_text").unwrap_err().contains(CLIP_PLUGIN_OFF));
        l.lockdown_clipboard = true;
        assert!(clip_check(&effective_with(l, Some(l)), false, "clip_text").unwrap_err().contains(CLIP_APP_OFF));
        assert_eq!(clip_key_text(), "Copy and paste are off (Settings › Security › App clipboard)");
        let r = report(&json!({}));
        assert_eq!((r["realmPlugin"].clone(), r["pageAppClipboard"].clone()), (json!(false), json!(true)));
    }

    /// toggle 9's native gate: which key presses are clipboard bindings
    #[test]
    fn clip_key_matches_the_clipboard_bindings_only() {
        for (k, c, s) in [(0x63, true, false), (0x43, true, true), (0x78, true, false), (0x76, true, false), (0x56, true, true), (0xff63, true, false), (0xff63, false, true), (0xff9e, false, true), (0xffff, false, true), (0x1008ff6d, false, false)] {
            assert!(clip_key(k, c, s), "{k:#x} ctrl={c} shift={s}");
        }
        for (k, c, s) in [(0x63, false, false), (0x76, false, true), (0x61, true, false), (0xffff, true, true), (0xffff, false, false), (0xff63, false, false), (0x7a, true, false)] {
            assert!(!clip_key(k, c, s), "{k:#x} ctrl={c} shift={s}");
        }
    }
    /// toggle 10: default allows (each spawn still asked); off refuses naming the toggle
    /// (+ sanitized plugin label); lockdown turns it off
    #[test]
    fn spawn_toggle_refuses_naming_it() {
        let d = Sec::default();
        assert!(spawn_check(&effective_with(d, Some(d)), "spawn", Some("x")).is_ok());
        let mut s = d;
        s.plugin_spawn = false;
        let e = effective_with(s, Some(s));
        let err = spawn_check(&e, "spawn true", Some("probe\n-spawn")).unwrap_err();
        assert_eq!(err, "EPERM: spawn true: community plugins may not start programs (Settings › Security › Community plugins may start programs) — plugin probe-spawn");
        assert!(spawn_check(&e, "shell openExternal", None).unwrap_err().ends_with("start programs)"));
        let mut l = d;
        l.lockdown = true;
        assert!(spawn_check(&effective_with(l, Some(l)), "spawn", None).unwrap_err().contains(SPAWN_OFF));
    }
    #[test]
    fn defaults_are_todays_behaviour() {
        let s = Sec::from_cfg(&json!({}));
        assert_eq!(s, Sec::default());
        // today: no remote media, plugins reach the network, online, media renders, WebGL,
        // canvas, JIT, both clipboards, plugin spawns (each asked), all three renderers, no lockdown
        assert!(!s.remote_media && s.plugin_net && !s.offline && s.vault_media && s.webgl && s.canvas && s.jit);
        assert!(s.plugin_clipboard && s.app_clipboard && s.plugin_spawn && s.mathjax && s.prism && s.mermaid);
        assert!(!s.lockdown && !s.lockdown_clipboard);
        let e = effective_with(s, Some(s));
        assert_eq!(e.sec, s, "the defaults pass through effective() unchanged");
        assert!(!e.lockdown);
    }

    /// toggle 7: off sets every JIT tier off + the mark; on with an inherited mark removes
    /// exactly what an opensidian parent set; on without a mark touches nothing (a user's
    /// own JSC_ vars stand); lockdown forces it off through the latch
    #[test]
    fn jit_plan_sets_removes_only_its_own_vars() {
        let (set, rm) = jit_plan(false, false);
        assert!(rm.is_empty());
        for k in ["JSC_useJIT", "JSC_useBaselineJIT", "JSC_useDFGJIT", "JSC_useFTLJIT", "JSC_useRegExpJIT"] {
            assert!(set.contains(&(k, "0")), "{k}=0 missing");
        }
        assert!(set.contains(&(JIT_MARK, "1")));
        assert_eq!(jit_plan(false, true).0, set, "off is off, inherited mark or not");
        let (set, rm) = jit_plan(true, true);
        assert!(set.is_empty());
        assert_eq!(rm.len(), JIT_OFF_ENV.len() + 1);
        assert!(rm.contains(&"JSC_useJIT") && rm.contains(&JIT_MARK));
        assert_eq!(jit_plan(true, false), (Vec::new(), Vec::new()), "default: env untouched");
        let mut s = Sec::default();
        s.lockdown = true;
        assert!(!effective_with(s, Some(s)).sec.jit, "lockdown forces the JIT off");
        let mut s = Sec::default();
        s.jit = false;
        assert!(effective_with(s, Some(Sec::default())).sec.jit, "restart-bound: the latch wins");
        assert!(jit_log(false).contains("[sec] jit=off") && jit_log(false).contains("Settings › Security › JavaScript JIT"));
        assert_eq!(jit_log(true), "[sec] jit=on");
    }

    #[test]
    fn malformed_key_or_field_reads_as_default_unknown_ignored() {
        for bad in [json!({"security": 7}), json!({"security": "x"}), json!({"security": null}), json!({"security": []})] {
            assert_eq!(Sec::from_cfg(&bad), Sec::default());
        }
        let s = Sec::from_cfg(&json!({"security": {"offline": "yes", "webgl": false, "zzz": true}}));
        assert!(!s.offline, "a non-bool field is its default");
        assert!(!s.webgl, "a valid field next to a bad one still applies");
    }

    #[test]
    fn nothing_but_the_config_value_is_read() {
        // the API takes the ~/.opensidian.json Value only: no path, no vault argument exists
        let src = include_str!("security.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
        for bad in ["std::fs", "File::open", ".obsidian", "Path", "env::var"] {
            assert!(!code.contains(bad), "security.rs must read the config Value only, found {bad}");
        }
    }

    #[test]
    fn lockdown_is_an_overlay_and_restores_stored_values() {
        let mut stored = Sec::default();
        stored.remote_media = true;
        stored.webgl = false;
        stored.plugin_spawn = false;
        stored.lockdown = true;
        let e = effective_with(stored, Some(stored));
        assert!(e.lockdown);
        for (f, _, strict, _) in FIELDS {
            if !matches!(*f, "lockdown" | "lockdownClipboard" | "appClipboard") {
                assert_eq!(e.sec.field(f), Some(*strict), "{f} not strict under lockdown");
            }
        }
        assert!(e.sec.app_clipboard, "9 is not forced off without the checkbox");
        stored.lockdown_clipboard = true;
        assert!(!effective_with(stored, Some(stored)).sec.app_clipboard, "9 off with the checkbox");
        // off again: exactly the earlier values, not the defaults
        stored.lockdown = false;
        stored.lockdown_clipboard = false;
        let e = effective_with(stored, Some(stored));
        assert!(e.sec.remote_media && !e.sec.webgl && !e.sec.plugin_spawn);
    }

    #[test]
    fn restart_bound_fields_are_latched() {
        let boot = Sec::default();
        let mut now = Sec::default();
        now.jit = false;
        now.lockdown = true;
        let e = effective_with(now, Some(boot));
        assert!(e.sec.jit && !e.lockdown, "jit / lockdown keep the boot value until restart");
        assert_eq!(forced_by(now, &e, "jit"), Some("restart"));
        assert_eq!(forced_by(now, &e, "lockdown"), Some("restart"));
        // a live field still applies at once
        now.webgl = false;
        assert!(!effective_with(now, Some(boot)).sec.webgl);
    }

    #[test]
    fn precedence_offline_and_canvas() {
        let mut s = Sec::default();
        s.remote_media = true;
        s.offline = true;
        let e = effective_with(s, Some(s));
        assert!(!e.sec.remote_media && !e.sec.plugin_net);
        assert_eq!(forced_by(s, &e, "remoteMedia"), Some("offline"));
        assert_eq!(forced_by(s, &e, "pluginNet"), Some("offline"));
        let mut s = Sec::default();
        s.canvas = false;
        let e = effective_with(s, Some(s));
        assert!(!e.sec.webgl);
        assert_eq!(forced_by(s, &e, "webgl"), Some("canvas"));
    }

    #[test]
    fn validate_and_weakens() {
        assert!(validate("nope", true).is_err());
        assert!(validate("__proto__", true).is_err());
        assert_eq!(validate("offline", true), Ok(("offline", true)));
        let d = Sec::default();
        assert!(!weakens(d, "offline", true), "turning a protection ON never asks");
        assert!(!weakens(d, "webgl", true), "no change never asks");
        assert!(!weakens(d, "webgl", false));
        let mut s = d;
        s.offline = true;
        assert!(weakens(s, "offline", false), "offline off lowers protection");
        s.webgl = false;
        assert!(weakens(s, "webgl", true));
        assert!(weakens(d, "remoteMedia", true), "loading remote media lowers protection");
        let mut l = d;
        l.lockdown = true;
        assert!(weakens(l, "lockdown", false));
    }

    #[test]
    fn every_field_has_a_name_and_named_carries_it() {
        for (f, _, _, n) in FIELDS {
            assert!(Sec::default().field(f).is_some(), "{f} has no struct field");
            assert_eq!(named(f), format!("(Settings › Security › {n})"));
        }
        assert_eq!(FIELDS.len(), 15);
        let r = report(&json!({}));
        assert_eq!(r["stored"]["offline"], json!(false));
        assert_eq!(r["effective"]["pluginNet"], json!(true));
        assert_eq!(r["names"]["webgl"], json!("WebGL"));
    }

    #[test]
    fn offline_check_names_the_toggle() {
        let mut s = Sec::default();
        assert!(net_check(&effective_with(s, Some(s)), "store").is_ok(), "default = online (today)");
        s.offline = true;
        let e = net_check(&effective_with(s, Some(s)), "store").unwrap_err();
        assert_eq!(e, "store: offline mode is on (Settings › Security › Cut off all network (offline mode))");
        let mut l = Sec::default();
        l.lockdown = true;
        assert!(net_check(&effective_with(l, Some(l)), "x").is_err(), "lockdown forces offline");
        assert!(offline_open_text("https://a.b/\n").contains("\"https://a.b/\\n\""), "the address is Debug-quoted");
    }

    /// toggle 5: the webview setting follows the EFFECTIVE value — the stored switch, HTML
    /// canvas off and lockdown all turn it off; nothing changes while they agree
    #[test]
    fn webgl_follows_effective_value() {
        let d = Sec::default();
        assert_eq!(webgl_change(&effective_with(d, None), true), None, "default = today: WebGL on, nothing to do");
        let mut s = d;
        s.webgl = false;
        assert_eq!(webgl_change(&effective_with(s, None), true), Some(false));
        assert_eq!(webgl_change(&effective_with(s, None), false), None);
        let mut c = d;
        c.canvas = false;
        assert_eq!(webgl_change(&effective_with(c, None), true), Some(false), "canvas off forces WebGL off");
        let mut l = d;
        l.lockdown = true;
        assert_eq!(webgl_change(&effective_with(l, Some(l)), true), Some(false), "lockdown forces WebGL off");
        assert_eq!(webgl_change(&effective_with(d, None), false), Some(true), "turning it back on re-enables");
        assert_eq!(webgl_off_text(), "WebGL is off (Settings › Security › WebGL)");
        assert_eq!(report(&json!({}))["pageWebgl"], json!(true));
    }

    /// toggle 11: each engine's switch 403s exactly its own /vendor/<engine>/ bytes, through
    /// percent-encoding / case / dot-segment spellings; defaults (today) refuse nothing;
    /// lockdown refuses all three; other vendored libs (pdfjs, codemirror) are never touched
    #[test]
    fn engine_switches_refuse_only_their_own_vendor_dir() {
        let d = effective_with(Sec::default(), None);
        for p in ["/vendor/mathjax/tex-chtml-full.js", "/vendor/prism/prism.js", "/vendor/mermaid/mermaid.min.js"] {
            assert_eq!(engine_refusal(&d, p), None, "default = today: {p} served");
        }
        for (f, file) in [("mathjax", "tex-chtml-full.js"), ("prism", "prism.js"), ("mermaid", "mermaid.min.js")] {
            let mut s = Sec::default();
            *s.field_mut(f).unwrap() = false;
            let e = effective_with(s, None);
            let want = engine_off_text(f);
            for p in [
                format!("/vendor/{f}/{file}"),
                format!("/vendor/{}/{file}", f.to_uppercase()),
                format!("//vendor/./x/../{f}/{file}"),
                format!("/vendor/%{:02X}{}/{file}", f.as_bytes()[0], &f[1..]),
                format!("/vendor%2F{f}%2F{file}"),
                format!("/vendor/{f}"),
            ] {
                assert_eq!(engine_refusal(&e, &p).as_deref(), Some(want.as_str()), "{f}: {p} not refused");
            }
            for (o, ofile) in [("mathjax", "a.js"), ("prism", "prism.js"), ("mermaid", "m.js"), ("pdfjs", "pdf.min.mjs"), ("codemirror", "cm.js")] {
                if o != f {
                    assert_eq!(engine_refusal(&e, &format!("/vendor/{o}/{ofile}")), None, "{f} off touched {o}");
                }
            }
            assert_eq!(engine_refusal(&e, &format!("/{f}/{file}")), None, "only under /vendor/");
        }
        assert_eq!(engine_off_text("mathjax"), "MathJax is off (Settings › Security › MathJax)");
        let mut l = Sec::default();
        l.lockdown = true;
        let le = effective_with(l, Some(l));
        assert!(["mathjax", "prism", "mermaid"].iter().all(|f| engine_refusal(&le, &format!("/vendor/{f}/x.js")).is_some()), "lockdown turns all three off");
        assert_eq!(norm_path("/a/%zz/../b%2"), "/a/b%2", "malformed escapes stay literal");
    }
}
