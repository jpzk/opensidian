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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
