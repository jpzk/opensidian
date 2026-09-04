//! perf-spans: screen-free perf telemetry. Zero deps beyond serde_json.
//!
//! When the env var RUSTIDIAN_PERF names a file, `span()` appends one JSON
//! line `{"t":<unix_ms>,"name":..,"ms":..,..extra}` to it. When unset the
//! target is resolved once (OnceLock) to None and every span is a no-op —
//! `span_timed!` doesn't even take an Instant in that case.
//!
//! The frontend writes to the SAME file through the `log_span` command, so
//! one `jq` pass over the jsonl shows backend + UI spans on one timeline.
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// env value -> target file; None (no-op) when unset or empty
pub fn resolve(env: Option<OsString>) -> Option<PathBuf> {
    env.filter(|s| !s.is_empty()).map(PathBuf::from)
}

fn target() -> Option<&'static PathBuf> {
    static T: OnceLock<Option<PathBuf>> = OnceLock::new();
    T.get_or_init(|| resolve(std::env::var_os("RUSTIDIAN_PERF")))
        .as_ref()
}

/// true when RUSTIDIAN_PERF is set (checked once per process)
pub fn enabled() -> bool {
    target().is_some()
}

pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// build the line: t, name, ms first, then every key of `extra` (object only)
pub fn line(name: &str, ms: f64, extra: Value) -> Value {
    let mut m = Map::new();
    m.insert("t".into(), json!(unix_ms()));
    m.insert("name".into(), json!(name));
    m.insert("ms".into(), json!((ms * 100.0).round() / 100.0));
    if let Value::Object(o) = extra {
        for (k, v) in o {
            if k != "t" && k != "name" && k != "ms" {
                m.insert(k, v);
            }
        }
    }
    Value::Object(m)
}

/// append one span line to `path` (create if missing). Errors are swallowed
/// by callers: telemetry must never break the app.
pub fn emit(path: &Path, name: &str, ms: f64, extra: Value) -> std::io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", line(name, ms, extra))
}

/// public entry: no-op unless RUSTIDIAN_PERF is set
pub fn span(name: &str, ms: f64, extra: Value) {
    if let Some(p) = target() {
        let _ = emit(p, name, ms, extra);
    }
}

/// `span_timed!(name, expr)` / `span_timed!(name, expr, extra_json)` —
/// evaluates `expr`, records its wall time under `name`. `extra_json` is
/// evaluated after `expr` (so it may read locals computed before the call).
#[macro_export]
macro_rules! span_timed {
    ($name:expr, $e:expr) => {
        $crate::span_timed!($name, $e, ::serde_json::json!({}))
    };
    ($name:expr, $e:expr, $extra:expr) => {{
        if $crate::perf::enabled() {
            let __t0 = ::std::time::Instant::now();
            let __r = $e;
            $crate::perf::span($name, __t0.elapsed().as_secs_f64() * 1000.0, $extra);
            __r
        } else {
            $e
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_writes_parseable_line_when_set_noop_when_unset() {
        // unset / empty env -> no target -> span() is a no-op
        assert!(resolve(None).is_none());
        assert!(resolve(Some(OsString::from(""))).is_none());
        assert_eq!(resolve(Some(OsString::from("/tmp/x.jsonl"))), Some(PathBuf::from("/tmp/x.jsonl")));

        // set -> one JSON line per call, t/name/ms + extra keys, extra can't clobber core keys
        let p = std::env::temp_dir().join(format!("rustidian-perf-test-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        emit(&p, "render_blocks", 12.3456, json!({"blocks": 150, "name": "evil"})).unwrap();
        emit(&p, "note_open", 40.0, json!("not-an-object")).unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let a: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(a["name"], "render_blocks");
        assert_eq!(a["ms"], 12.35);
        assert_eq!(a["blocks"], 150);
        assert!(a["t"].as_u64().unwrap() > 1_600_000_000_000);
        let b: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(b["name"], "note_open");
        assert_eq!(b["ms"], 40.0);
        assert_eq!(b.as_object().unwrap().len(), 3);
        let _ = std::fs::remove_file(&p);

        // the process-wide entry point: whatever the env says now is what
        // span() does; with RUSTIDIAN_PERF unset in `cargo test` this is the
        // no-op path and must not create anything.
        if std::env::var_os("RUSTIDIAN_PERF").is_none() {
            assert!(!enabled());
            span("noop", 1.0, json!({}));
        }
    }
}
