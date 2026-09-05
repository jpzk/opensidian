//! otel (R18): screen-free lag telemetry in the OpenTelemetry data model.
//! Zero deps beyond serde/serde_json.
//!
//! When RUSTIDIAN_OTEL (alias: RUSTIDIAN_PERF) names a file, every span is
//! appended as ONE OTLP/JSON line — an ExportTraceServiceRequest, i.e. what the
//! otel-collector `otlpjsonfile` receiver reads:
//!   {"resourceSpans":[{"resource":{"attributes":[service.name, service.version]},
//!     "scopeSpans":[{"scope":{"name":"rustidian"},"spans":[{traceId, spanId,
//!     parentSpanId, name, kind, startTimeUnixNano, endTimeUnixNano, attributes}]}]}]}
//! When unset the target resolves once (OnceLock) to None and every span is a
//! no-op — `span_timed!` doesn't even take an Instant then.
//!
//! Frontend spans (ui/otel.js: action -> paint) arrive batched through the
//! `log_spans` command and land in the same file. A UI action span is the ROOT
//! of its trace; backend commands it invokes receive `{traceId, spanId}` as the
//! `otel` invoke arg and become its children (`span_timed!(ctx => ...)`).
//! Ids are hand-rolled: a process counter mixed with the clock + pid.
//! scripts/otel-flat.sh flattens the lines back to `{t,name,ms,<attrs>}` for jq.
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// env value -> target file; None (no-op) when unset or empty
pub fn resolve(env: Option<OsString>) -> Option<PathBuf> {
    env.filter(|s| !s.is_empty()).map(PathBuf::from)
}

fn target() -> Option<&'static PathBuf> {
    static T: OnceLock<Option<PathBuf>> = OnceLock::new();
    T.get_or_init(|| {
        resolve(std::env::var_os("RUSTIDIAN_OTEL")).or_else(|| resolve(std::env::var_os("RUSTIDIAN_PERF")))
    })
    .as_ref()
}

/// true when RUSTIDIAN_OTEL / RUSTIDIAN_PERF is set (checked once per process)
pub fn enabled() -> bool {
    target().is_some()
}

/// trace context handed in by the frontend (invoke arg `otel`): the UI action
/// span that triggered this command. Backend spans become its children.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Ctx {
    #[serde(rename = "traceId", default)]
    pub trace_id: String,
    #[serde(rename = "spanId", default)]
    pub span_id: String,
}

pub fn unix_nanos() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}
pub fn unix_ms() -> u64 {
    (unix_nanos() / 1_000_000) as u64
}

/// 64 random-looking bits: counter x golden ratio, xor clock, xor pid; never 0 (0 = invalid id in OTLP)
fn id64() -> u64 {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let v = n.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (unix_nanos() as u64) ^ ((std::process::id() as u64) << 40);
    if v == 0 { 0x5EED } else { v }
}
pub fn new_trace_id() -> String {
    format!("{:016x}{:016x}", id64(), id64())
}
pub fn new_span_id() -> String {
    format!("{:016x}", id64())
}

/// one span, ready to serialise
#[derive(Debug, Clone)]
pub struct Span {
    pub name: String,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String, // "" = root
    pub start_ns: u128,
    pub end_ns: u128,
    pub attrs: Value, // object; non-objects are ignored
}

/// serde_json value -> OTLP AnyValue
pub fn any_value(v: &Value) -> Value {
    match v {
        Value::String(s) => json!({ "stringValue": s }),
        Value::Bool(b) => json!({ "boolValue": b }),
        Value::Number(n) if n.is_i64() || n.is_u64() => json!({ "intValue": n.to_string() }), // OTLP/JSON: int64 as string
        Value::Number(n) => json!({ "doubleValue": n }),
        Value::Null => json!({ "stringValue": "" }),
        other => json!({ "stringValue": other.to_string() }),
    }
}
/// object -> OTLP KeyValue list (non-objects -> empty). Core span fields can't be clobbered: they aren't attributes.
pub fn attributes(extra: &Value) -> Vec<Value> {
    match extra {
        Value::Object(o) => o.iter().map(|(k, v)| json!({ "key": k, "value": any_value(v) })).collect(),
        _ => vec![],
    }
}
pub fn span_json(s: &Span) -> Value {
    let mut m = Map::new();
    m.insert("traceId".into(), json!(s.trace_id));
    m.insert("spanId".into(), json!(s.span_id));
    m.insert("parentSpanId".into(), json!(s.parent_span_id));
    m.insert("name".into(), json!(s.name));
    m.insert("kind".into(), json!(1)); // SPAN_KIND_INTERNAL
    m.insert("startTimeUnixNano".into(), json!(s.start_ns.to_string()));
    m.insert("endTimeUnixNano".into(), json!(s.end_ns.to_string()));
    m.insert("attributes".into(), Value::Array(attributes(&s.attrs)));
    Value::Object(m)
}
/// ExportTraceServiceRequest carrying `spans` (one line of the file)
pub fn request(spans: &[Span]) -> Value {
    json!({ "resourceSpans": [{
        "resource": { "attributes": [
            { "key": "service.name", "value": { "stringValue": "rustidian" } },
            { "key": "service.version", "value": { "stringValue": env!("CARGO_PKG_VERSION") } } ] },
        "scopeSpans": [{ "scope": { "name": "rustidian" }, "spans": spans.iter().map(span_json).collect::<Vec<_>>() }] }] })
}

/// append one OTLP/JSON line to `path` (create if missing). Errors are swallowed
/// by callers: telemetry must never break the app.
pub fn emit(path: &Path, spans: &[Span]) -> std::io::Result<()> {
    if spans.is_empty() {
        return Ok(());
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", request(spans))
}

/// a backend span that just ended: `ms` long, ending now; child of `ctx` when given
pub fn ended(ctx: Option<&Ctx>, name: &str, ms: f64, extra: Value) -> Span {
    let end = unix_nanos();
    let start = end.saturating_sub((ms.max(0.0) * 1e6) as u128);
    let (trace_id, parent) = match ctx {
        Some(c) if c.trace_id.len() == 32 && c.span_id.len() == 16 => (c.trace_id.clone(), c.span_id.clone()),
        _ => (new_trace_id(), String::new()),
    };
    Span { name: name.into(), trace_id, span_id: new_span_id(), parent_span_id: parent, start_ns: start, end_ns: end, attrs: extra }
}

/// public entry: no-op unless enabled
pub fn span(name: &str, ms: f64, extra: Value) {
    span_ctx(None, name, ms, extra)
}
pub fn span_ctx(ctx: Option<&Ctx>, name: &str, ms: f64, extra: Value) {
    if let Some(p) = target() {
        let _ = emit(p, &[ended(ctx, name, ms, extra)]);
    }
}

/// frontend batch (ui/otel.js): [{name, traceId, spanId, parentSpanId, startMs, endMs, attrs}]
/// startMs/endMs are unix ms (f64, sub-ms fraction from performance.now()) -> nanos here.
pub fn ui_span(v: &Value) -> Span {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let ms = |k: &str| v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0);
    let ns = |m: f64| if m > 0.0 { (m * 1e6) as u128 } else { unix_nanos() };
    let end_ns = ns(ms("endMs"));
    let start_ns = if ms("startMs") > 0.0 { ns(ms("startMs")).min(end_ns) } else { end_ns.saturating_sub((ms("ms") * 1e6) as u128) };
    let mut trace_id = s("traceId");
    if trace_id.len() != 32 { trace_id = new_trace_id(); }
    let mut span_id = s("spanId");
    if span_id.len() != 16 { span_id = new_span_id(); }
    let attrs = v.get("attrs").or_else(|| v.get("extra")).cloned().unwrap_or(Value::Null);
    Span { name: s("name"), trace_id, span_id, parent_span_id: s("parentSpanId"), start_ns, end_ns, attrs }
}
pub fn ui_spans(list: &[Value]) {
    if let Some(p) = target() {
        let spans: Vec<Span> = list.iter().map(ui_span).collect();
        let _ = emit(p, &spans);
    }
}

/// `span_timed!(name, expr)` / `span_timed!(name, expr, extra_json)` records the wall time of
/// `expr` under `name` as a root span; `span_timed!(ctx => name, expr[, extra])` makes it a child
/// of `ctx: Option<perf::Ctx>` (the frontend's action span). `extra_json` is evaluated after
/// `expr` (so it may read locals computed before the call).
#[macro_export]
macro_rules! span_timed {
    ($name:expr, $e:expr) => {
        $crate::span_timed!(::std::option::Option::<$crate::perf::Ctx>::None => $name, $e, ::serde_json::json!({}))
    };
    ($name:expr, $e:expr, $extra:expr) => {
        $crate::span_timed!(::std::option::Option::<$crate::perf::Ctx>::None => $name, $e, $extra)
    };
    ($ctx:expr => $name:expr, $e:expr) => {
        $crate::span_timed!($ctx => $name, $e, ::serde_json::json!({}))
    };
    ($ctx:expr => $name:expr, $e:expr, $extra:expr) => {{
        if $crate::perf::enabled() {
            let __t0 = ::std::time::Instant::now();
            let __r = $e;
            $crate::perf::span_ctx($ctx.as_ref(), $name, __t0.elapsed().as_secs_f64() * 1000.0, $extra);
            __r
        } else {
            $e
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_span(line: &str) -> Value {
        let v: Value = serde_json::from_str(line).unwrap();
        let rs = &v["resourceSpans"][0];
        assert_eq!(rs["resource"]["attributes"][0]["value"]["stringValue"], "rustidian");
        assert_eq!(rs["resource"]["attributes"][1]["key"], "service.version");
        rs["scopeSpans"][0]["spans"][0].clone()
    }

    #[test]
    fn otlp_lines_parse_ids_nest_noop_when_unset() {
        assert!(resolve(None).is_none());
        assert!(resolve(Some(OsString::from(""))).is_none());
        assert_eq!(resolve(Some(OsString::from("/tmp/x.jsonl"))), Some(PathBuf::from("/tmp/x.jsonl")));
        // ids: right width, hex, unique
        let t = new_trace_id();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(new_span_id(), new_span_id());
        assert_eq!(new_span_id().len(), 16);

        let p = std::env::temp_dir().join(format!("rustidian-otel-test-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        // root backend span with typed attrs
        emit(&p, &[ended(None, "render_blocks", 12.3456, json!({"blocks": 150, "name": "evil", "f": 1.5, "b": true}))]).unwrap();
        // child of a frontend ctx
        let ctx = Ctx { trace_id: "0123456789abcdef0123456789abcdef".into(), span_id: "0123456789abcdef".into() };
        emit(&p, &[ended(Some(&ctx), "read_note", 40.0, json!("not-an-object"))]).unwrap();
        // frontend batch: two spans in ONE request line; second is a child of the first
        let now_ms = unix_ms() as f64;
        let ui = vec![
            json!({"name":"note_open","traceId":ctx.trace_id,"spanId":ctx.span_id,"parentSpanId":"","startMs":now_ms-30.5,"endMs":now_ms,"attrs":{"note":"a.md","via":"tree"}}),
            json!({"name":"paint","traceId":ctx.trace_id,"spanId":"fedcba9876543210","parentSpanId":ctx.span_id,"startMs":now_ms-2.0,"endMs":now_ms,"attrs":{}}),
            json!({"name":"legacy","ms":3.0,"extra":{"k":1}}),   // old perf.push shape: ids minted here
        ];
        let spans: Vec<Span> = ui.iter().map(ui_span).collect();
        emit(&p, &spans).unwrap();
        emit(&p, &[]).unwrap(); // empty batch writes nothing

        let body = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 3);
        let a = first_span(lines[0]);
        assert_eq!(a["name"], "render_blocks");
        assert_eq!(a["traceId"].as_str().unwrap().len(), 32);
        assert_eq!(a["spanId"].as_str().unwrap().len(), 16);
        assert_eq!(a["parentSpanId"], "");
        let st: u128 = a["startTimeUnixNano"].as_str().unwrap().parse().unwrap();
        let en: u128 = a["endTimeUnixNano"].as_str().unwrap().parse().unwrap();
        assert!(en > 1_600_000_000_000_000_000);
        assert!((en - st) as f64 / 1e6 - 12.3456 < 0.01);
        let attrs = a["attributes"].as_array().unwrap();
        let find = |k: &str| attrs.iter().find(|x| x["key"] == k).map(|x| x["value"].clone()).unwrap();
        assert_eq!(find("blocks"), json!({"intValue": "150"}));
        assert_eq!(find("name"), json!({"stringValue": "evil"})); // an attribute, not the span name
        assert_eq!(find("f"), json!({"doubleValue": 1.5}));
        assert_eq!(find("b"), json!({"boolValue": true}));
        let b = first_span(lines[1]);
        assert_eq!(b["traceId"], ctx.trace_id);
        assert_eq!(b["parentSpanId"], ctx.span_id);
        assert_ne!(b["spanId"], ctx.span_id);
        assert_eq!(b["attributes"].as_array().unwrap().len(), 0);
        let v: Value = serde_json::from_str(lines[2]).unwrap();
        let sp = v["resourceSpans"][0]["scopeSpans"][0]["spans"].as_array().unwrap().clone();
        assert_eq!(sp.len(), 3);
        assert_eq!(sp[0]["spanId"], ctx.span_id);
        assert_eq!(sp[1]["parentSpanId"], ctx.span_id);
        let s0: u128 = sp[0]["startTimeUnixNano"].as_str().unwrap().parse().unwrap();
        let e0: u128 = sp[0]["endTimeUnixNano"].as_str().unwrap().parse().unwrap();
        assert!(((e0 - s0) as f64 / 1e6 - 30.5).abs() < 0.01);
        assert_eq!(sp[2]["traceId"].as_str().unwrap().len(), 32);
        let s2: u128 = sp[2]["startTimeUnixNano"].as_str().unwrap().parse().unwrap();
        let e2: u128 = sp[2]["endTimeUnixNano"].as_str().unwrap().parse().unwrap();
        assert!(((e2 - s2) as f64 / 1e6 - 3.0).abs() < 0.01);
        let _ = std::fs::remove_file(&p);

        // the process-wide entry point: whatever the env says now is what
        // span() does; with both vars unset in `cargo test` this is the
        // no-op path and must not create anything.
        if std::env::var_os("RUSTIDIAN_OTEL").is_none() && std::env::var_os("RUSTIDIAN_PERF").is_none() {
            assert!(!enabled());
            span("noop", 1.0, json!({}));
            ui_spans(&[json!({"name":"noop"})]);
        }
    }
}
