// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! census-gate (issue #30): the window-title census is a TEST/DEV probe, not a
//! user feature. It is built only when this process was started with
//! OPENSIDIAN_CENSUS=1, or by a `devui` build (on unless OPENSIDIAN_CENSUS=0).
//! RUNTIME, not a cargo feature: the gate and the smokes drive the very binary
//! users get, with the flag set. The env is read ONCE (first call, before the
//! window exists) and handed to the page as a frozen, non-configurable
//! `window.__OS_CENSUS === true` by an initialization script, which runs before
//! any page script — ui/census.js turns it into `CENSUS_ON` before main.js.
//! Off: no script is injected, the property is absent, `CENSUS_ON` is false.

use std::sync::OnceLock;

/// the env that turns the census on (`1`), or off in a devui build (`0`)
pub const ENV: &str = "OPENSIDIAN_CENSUS";

/// runs before every page script, on every load of the window
pub const INIT_JS: &str = r#"Object.defineProperty(window,"__OS_CENSUS",{value:true,writable:false,configurable:false,enumerable:false});"#;

static ON: OnceLock<bool> = OnceLock::new();

/// PURE: the decision for an env value and the build kind
pub fn decide(v: Option<&str>, devui: bool) -> bool {
    match v {
        Some("1") => true,
        Some("0") => false,
        _ => devui,
    }
}

/// true when this process publishes the census (read once per process)
pub fn on() -> bool {
    *ON.get_or_init(|| {
        let v = std::env::var(ENV).ok();
        let on = decide(v.as_deref(), cfg!(feature = "devui"));
        if on {
            eprintln!("[census] on ({ENV}={})", v.as_deref().unwrap_or("<unset>, devui build"));
        }
        on
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn census_off_unless_opted_in() {
        assert!(!decide(None, false));
        assert!(!decide(Some(""), false));
        assert!(!decide(Some("0"), false));
        assert!(!decide(Some("true"), false), "only the literal 1 opts in");
        assert!(decide(Some("1"), false));
    }

    #[test]
    fn census_devui_default_on_but_can_be_turned_off() {
        assert!(decide(None, true));
        assert!(decide(Some("1"), true));
        assert!(!decide(Some("0"), true));
    }

    #[test]
    fn census_init_js_freezes_the_flag() {
        assert!(INIT_JS.contains("__OS_CENSUS"));
        assert!(INIT_JS.contains("writable:false") && INIT_JS.contains("configurable:false"));
    }
}
