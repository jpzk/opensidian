// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
//! test-events: a TEST-ONLY readiness channel for the gate (harness
//! docs/app-events.md). The harness must never sleep or poll, and only the app
//! knows when "the vault is loaded" is true. So the app writes one NDJSON line
//! per event, `{seq,ts,name,payload}`, after the state the event names is
//! committed. OFF unless this process was started with OPENSIDIAN_TEST_EVENTS=1.
//! This is a runtime switch, not a cargo feature: the gate drives the binary
//! that users get. The env is read ONCE.
//!
//! Sink: OPENSIDIAN_TEST_EVENTS_OUT=/abs/path (O_APPEND|O_CREAT; a FIFO also
//! works and blocks until read), else stdout. Never dropped: one mutex covers
//! seq assignment, write_all, and flush, so a slow sink blocks the emitter
//! instead of losing a line. A write error turns the channel off and says so on
//! stderr. The harness then times out loudly rather than passing.
//! OPENSIDIAN_TEST_EVENTS_SKIP=a,b suppresses the named events (negative
//! controls only). A skipped event takes no seq, so the stream stays gapless.

use std::io::Write;
use std::sync::{Mutex, OnceLock};

pub const ENV: &str = "OPENSIDIAN_TEST_EVENTS";
pub const ENV_OUT: &str = "OPENSIDIAN_TEST_EVENTS_OUT";
pub const ENV_SKIP: &str = "OPENSIDIAN_TEST_EVENTS_SKIP";
/// schema of the line format (additive payload fields only)
pub const SCHEMA: u32 = 1;

/// runs before every page script when the channel is on; JS checks it before invoking
pub const INIT_JS: &str = r#"Object.defineProperty(window,"__OS_TEST_EVENTS",{value:true,writable:false,configurable:false,enumerable:false});"#;

struct Chan {
    seq: u64,
    out: Option<Box<dyn Write + Send>>,
    skip: Vec<String>,
}

static CHAN: OnceLock<Option<Mutex<Chan>>> = OnceLock::new();

/// PURE: the on/off decision for an env value. Only the literal 1 turns it on.
pub fn decide(v: Option<&str>) -> bool {
    v == Some("1")
}

/// PURE: the skip list from OPENSIDIAN_TEST_EVENTS_SKIP
pub fn parse_skip(v: Option<&str>) -> Vec<String> {
    v.unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

/// PURE: one NDJSON line, with no trailing newline
pub fn line(seq: u64, ts: u128, name: &str, payload: &serde_json::Value) -> String {
    let mut m = serde_json::Map::new();
    m.insert("seq".into(), seq.into());
    m.insert("ts".into(), serde_json::Value::from(ts as u64));
    m.insert("name".into(), name.into());
    m.insert("payload".into(), payload.clone());
    serde_json::Value::Object(m).to_string()
}

fn chan() -> &'static Option<Mutex<Chan>> {
    CHAN.get_or_init(|| {
        if !decide(std::env::var(ENV).ok().as_deref()) {
            return None;
        }
        let out: Box<dyn Write + Send> = match std::env::var_os(ENV_OUT) {
            Some(p) => match std::fs::OpenOptions::new().append(true).create(true).open(&p) {
                Ok(f) => Box::new(f),
                Err(e) => {
                    eprintln!("[testev] sink error: open {}: {e}; channel off", p.to_string_lossy());
                    return None;
                }
            },
            None => Box::new(std::io::stdout()),
        };
        let skip = parse_skip(std::env::var(ENV_SKIP).ok().as_deref());
        eprintln!("[testev] on (sink {}, skip {:?})", std::env::var(ENV_OUT).unwrap_or_else(|_| "stdout".into()), skip);
        Some(Mutex::new(Chan { seq: 0, out: Some(out), skip }))
    })
}

/// true when this process publishes test events (read once per process)
pub fn on() -> bool {
    chan().is_some()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// emit one event. When the channel is off this does nothing. It blocks on a slow sink and never drops.
pub fn emit(name: &str, payload: serde_json::Value) {
    let Some(m) = chan() else { return };
    let mut c = m.lock().unwrap_or_else(|p| p.into_inner());
    if c.skip.iter().any(|s| s == name) {
        return;
    }
    let Some(out) = c.out.as_mut() else { return };
    let mut l = line(c.seq, now_ms(), name, &payload);
    l.push('\n');
    if let Err(e) = out.write_all(l.as_bytes()).and_then(|_| out.flush()) {
        eprintln!("[testev] sink error: {e}; channel off");
        c.out = None;
        return;
    }
    c.seq += 1;
}

/// the READY handshake: seq 0 of every process. A second `boot` in one stream means RESTART.
pub fn ready() {
    if !on() {
        return;
    }
    let pid = std::process::id();
    emit("ready", serde_json::json!({"schema": SCHEMA, "pid": pid, "boot": format!("{pid}-{}", now_ms()), "version": env!("CARGO_PKG_VERSION")}));
}

/// PURE: the JS-side events the page may emit through `test_event`. Rust-only events are refused.
pub fn js_allowed(name: &str) -> bool {
    matches!(name, "layout-settled" | "render-committed" | "modal-open" | "modal-closed" | "note-switched" | "graph-settled")
}

/// the page's door into the stream. Off is a no-op Ok. Unknown names are Err, so a typo fails loudly.
#[tauri::command]
pub fn test_event(name: String, payload: Option<serde_json::Value>) -> Result<(), String> {
    if !on() {
        return Ok(());
    }
    if !js_allowed(&name) {
        return Err(format!("test_event: unknown event {name:?}"));
    }
    emit(&name, payload.unwrap_or_else(|| serde_json::json!({})));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_unless_literal_1() {
        assert!(!decide(None));
        assert!(!decide(Some("")));
        assert!(!decide(Some("0")));
        assert!(!decide(Some("true")));
        assert!(decide(Some("1")));
    }

    #[test]
    fn skip_list_parses() {
        assert!(parse_skip(None).is_empty());
        assert_eq!(parse_skip(Some(" vault-loaded, ,ready ")), vec!["vault-loaded", "ready"]);
    }

    #[test]
    fn line_is_ndjson_with_fixed_keys() {
        let l = line(3, 1731237561123, "vault-loaded", &serde_json::json!({"notes": 2}));
        assert_eq!(l, r#"{"seq":3,"ts":1731237561123,"name":"vault-loaded","payload":{"notes":2}}"#);
        assert!(!l.contains('\n'));
    }

    #[test]
    fn js_cannot_forge_rust_events() {
        assert!(!js_allowed("ready"));
        assert!(!js_allowed("vault-loaded"));
        assert!(js_allowed("render-committed"));
    }
}
