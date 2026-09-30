// SPDX-License-Identifier: GPL-3.0-or-later
//! otel (R18): screen-free lag telemetry in the OpenTelemetry data model.
//! Zero deps beyond serde/serde_json.
//!
//! When OPENSIDIAN_OTEL (alias: OPENSIDIAN_PERF) names a file, every span is
//! appended as ONE OTLP/JSON line — an ExportTraceServiceRequest, i.e. what the
//! otel-collector `otlpjsonfile` receiver reads:
//!   {"resourceSpans":[{"resource":{"attributes":[service.name, service.version]},
//!     "scopeSpans":[{"scope":{"name":"opensidian"},"spans":[{traceId, spanId,
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
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// env value -> target file; None (no-op) when unset or empty
pub fn resolve(env: Option<OsString>) -> Option<PathBuf> {
    env.filter(|s| !s.is_empty()).map(PathBuf::from)
}

fn target() -> Option<&'static PathBuf> {
    static T: OnceLock<Option<PathBuf>> = OnceLock::new();
    T.get_or_init(|| {
        resolve(std::env::var_os("OPENSIDIAN_OTEL")).or_else(|| resolve(std::env::var_os("OPENSIDIAN_PERF")))
    })
    .as_ref()
}

/// true when OPENSIDIAN_OTEL / OPENSIDIAN_PERF is set (checked once per process)
pub fn enabled() -> bool {
    target().is_some()
}

// ===== SLOW-OP CONSOLE WARNINGS =============================================
// THE THRESHOLD LIVES HERE AND NOWHERE ELSE.
//
// WHAT "UNUSUALLY LONG" MEANS: a FIXED CEILING of 100ms. Not a baseline.
// Why fixed, stated so a reviewer can disagree with the reasoning rather than
// with a feeling:
//   * it is REVIEWABLE — one integer in one file, diffable, and it cannot be
//     argued into a green by an environment variable (see effective_slow_ms).
//   * it is ALREADY the project's hard rule: scripts/lag-budgets.env pins
//     MAX_INTERACTION_MS=100 as the p95 no user interaction may exceed.
//     scripts/perf-coverage.sh FAILS if these two numbers ever diverge, so the
//     duplication is a checked equality, not drift.
//   * a baseline-relative rule (warn at k x the op's own p95) normalises a slow
//     regression: if everything degrades together the baseline follows it down
//     and the warning never fires. It also has no answer on the first run.
//
// WHAT THIS CEILING CANNOT CATCH — every threshold has a blind spot; one with
// no stated blind spot is one nobody thought about:
//   1. SUB-CEILING REGRESSIONS. key_to_paint has an 8ms budget. Degrading from
//      8ms to 95ms is ~12x worse and this console stays SILENT. The p95 budgets
//      in scripts/lag-budgets.env (enforced by scripts/lag-gate.sh in the bench)
//      are what catch that class; the console is for the breach you can feel.
//   2. HARDWARE. On a machine slow enough that ordinary ops exceed 100ms the
//      console becomes noisy and stops being a signal. OPENSIDIAN_SLOW_MS can
//      only make it stricter, so there is deliberately NO escape hatch for a
//      slow box: the honest reading is "this machine is over the budget".
//   3. DEATH BY A THOUSAND CUTS. 40 ops of 90ms in a row never warn even though
//      the second of work they add up to is plainly felt.
//   4. UNINSTRUMENTED WORK. Anything that emits no span cannot be timed; that is
//      why coverage is measured (scripts/perf-coverage.sh), not assumed.
//   5. THE WARM-UP WINDOW (WARMUP_MS below). An op that STARTS in the first
//      WARMUP_MS of the process does not warn, so a slow operation that only ever
//      happens at startup is invisible HERE. That window belongs to `boot`, which
//      is out of scope above and is measured by the bench, not by this console.
//      Nothing is discarded: the span is still traced, and the ms is reported as
//      `cold=<ms>` on that op's next warning.
/// "Unusually long", in milliseconds. THE single definition.
pub const SLOW_MS_CEIL: u32 = 100;
/// the only env var that touches the ceiling; it may TIGHTEN it, never loosen it
pub const SLOW_ENV: &str = "OPENSIDIAN_SLOW_MS";

/// THE WARM-UP WINDOW. A span that STARTS within this many ms of process start is
/// COLD and does not warn — it is recorded and reported as `cold=<ms>` on that
/// op's next warning, so the number is deferred, never destroyed.
///
/// WHY THIS EXISTS, measured rather than assumed. Five note opens in the CLEAN
/// half of phase `perfslow`, the gate's own run at 3c4a80a on :82 — the full
/// capture, its provenance and the command that reproduces it are committed in
/// docs/perf-console/cold-ramp.log (offsets from the start of the `boot` span):
///   +125ms note_open 169ms   <- STARTS INSIDE the 255ms `boot` span
///   +973ms note_open  77ms
///  +1440ms note_open  10ms
///  +1928ms note_open  11ms
///  +2441ms note_open  11ms
/// Steady state is 10-11ms; the first open is 15x that and it begins BEFORE the
/// app has finished booting. Warning on it would put a line in the console on
/// EVERY launch, at the same point, forever — output at a constant rate, which is
/// the exact noise this feature exists to avoid and the fastest way to train a
/// developer to ignore the console. The cost is real, and it is boot's.
///
/// PINNED, like the ceiling: no env var reads it, so nobody can widen the window
/// to hide a regression (proved by warmup_window_is_pinned_and_envless).
/// The ramp is over by +1440ms in that run; 2000 is 1.4x that, and it is the
/// largest number that still leaves the interactive part of a scenario judged —
/// the 5th open at +2441ms IS judged, and is silent because it is fast (11ms),
/// not because it is cold.
pub const WARMUP_MS: u64 = 2000;

/// unix ms at process start, stamped by main() before anything is measured.
static PROC_START: OnceLock<u64> = OnceLock::new();
/// call ONCE, first thing in main(): the warm-up window is measured from here.
pub fn mark_start() {
    let _ = PROC_START.set(unix_ms());
}
/// is a span that started at `start_ms` inside the warm-up window?
/// Unknown process start (mark_start never called) -> NOTHING is cold: the
/// failure direction is a console that warns too much, never one that hides.
pub fn is_cold(start_ms: u64) -> bool {
    match PROC_START.get() {
        Some(t0) => start_ms < t0.saturating_add(WARMUP_MS),
        None => false,
    }
}

/// Ops that never warn, and the reason each one is out of scope. These are spans
/// that exceed the ceiling BY DESIGN, so warning on them would produce output at
/// a constant rate — which carries no information about whether anything is wrong.
/// They are still traced; they are only excluded from the console warning.
pub const WARN_EXCLUDE: &[(&str, &str)] = &[
    ("graph_settle", "post-paint force-sim convergence, budgeted informationally at 500ms (GRAPH_SETTLE_INFO_MS) - exceeds 100ms on every graph open by design"),
    ("graph_open_settle", "same family as graph_settle: convergence measured AFTER the paint the user waited for"),
    ("graph_frame", "emitted per animation frame; a per-frame warning is output at a constant rate, i.e. zero information"),
    ("graph_draw", "per-frame renderer span, same reason as graph_frame"),
    ("boot", "process start -> first note painted: webview init + vault scan, over 100ms by construction and not an interaction"),
    ("compositor_floor_build", "a bench fixture that builds DOM on purpose to measure the compositor floor, not a user operation"),
];

/// is this span name out of scope for the console warning?
pub fn warn_excluded(name: &str) -> bool {
    WARN_EXCLUDE.iter().any(|(n, _)| *n == name)
}

/// where the effective ceiling came from
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlowSrc {
    /// no env override: the pinned ceiling
    Ceiling,
    /// env asked for a STRICTER ceiling and got it
    Tightened,
    /// env asked to LOOSEN the ceiling: refused, pinned value kept
    RefusedLoosen,
    /// env was not a usable number: refused, pinned value kept
    RefusedJunk,
}

/// PURE (takes the env value, reads nothing) so a test can prove the rule
/// without racing the process environment: the env may only TIGHTEN.
/// Anything >= the pinned ceiling, zero, or unparseable is REFUSED and the
/// pinned ceiling is returned unchanged.
pub fn effective_slow_ms(env: Option<&str>) -> (u32, SlowSrc) {
    match env.map(str::trim).filter(|s| !s.is_empty()) {
        None => (SLOW_MS_CEIL, SlowSrc::Ceiling),
        Some(s) => match s.parse::<u32>() {
            Ok(v) if v > 0 && v < SLOW_MS_CEIL => (v, SlowSrc::Tightened),
            Ok(_) => (SLOW_MS_CEIL, SlowSrc::RefusedLoosen),
            Err(_) => (SLOW_MS_CEIL, SlowSrc::RefusedJunk),
        },
    }
}

/// the effective ceiling for THIS process (env read once)
pub fn slow_ms() -> u32 {
    static M: OnceLock<u32> = OnceLock::new();
    *M.get_or_init(|| effective_slow_ms(std::env::var(SLOW_ENV).ok().as_deref()).0)
}

/// THE ONE LINE this feature prints when nothing is slow. It states the ceiling,
/// where it is defined, what the env did (including a refusal), whether telemetry
/// is writing at all (`otel=on|off`, straight from [`enabled`]), and which ops are
/// out of scope — so "unusually long" is a number on the screen, not a claim in a
/// comment. Exactly one line, always, breach or not.
///
/// `otel=` exists because the warning must reach the console in an ORDINARY run,
/// the one where a user actually feels the lag and no trace file is open. Without
/// it, a log cannot be told apart from a traced one, and a smoke direction that
/// claims to be untraced is asserting on its own launcher's intent instead of on
/// what the process did.
pub fn slow_banner() -> String {
    let raw = std::env::var(SLOW_ENV).ok();
    let (ms, src) = effective_slow_ms(raw.as_deref());
    let env = match src {
        SlowSrc::Ceiling => "unset".to_string(),
        SlowSrc::Tightened => format!("{}={} TIGHTENED", SLOW_ENV, raw.unwrap_or_default()),
        SlowSrc::RefusedLoosen => format!("{}={} REFUSED(may only tighten)", SLOW_ENV, raw.unwrap_or_default()),
        SlowSrc::RefusedJunk => format!("{}={} REFUSED(not a number)", SLOW_ENV, raw.unwrap_or_default()),
    };
    let ex: Vec<&str> = WARN_EXCLUDE.iter().map(|(n, _)| *n).collect();
    format!(
        "[perf] slow_ms={ms} ceiling={SLOW_MS_CEIL} src=perf.rs:SLOW_MS_CEIL env={env} otel={} warmup={WARMUP_MS}ms cooldown={WARN_COOLDOWN_MS}ms inject={} budget={} excluded={} ({})",
        if enabled() { "on" } else { "off" },
        inject_desc(),
        WARN_LINE_BUDGET,
        ex.join(","),
        ex.len()
    )
}

// ===== THE CONSOLE SURFACE ==================================================
// A breach prints ONE line on stderr. THE TRAP this guards against: "log every
// long operation" degrading into "log every operation". Output arriving at a
// constant rate carries no information, and a console that always talks makes a
// bottleneck HARDER to find, not easier. Two bounds, both measured, neither a
// promise in a comment:
//   * PER-OP COOLDOWN: one line per op per WARN_COOLDOWN_MS. Breaches inside
//     the window are COUNTED, not dropped, and reported on the next line for
//     that op as `suppressed=N`. A storm of 500 slow note_opens is 1 line every
//     2s saying how many it stands for.
//   * PER-PROCESS CAP: WARN_MAX_LINES lines, then ONE final "budget exhausted"
//     line and silence. Without it a permanently slow machine turns the console
//     into a log file.
// So the console output of this feature is bounded, for the whole process, by
//   1 (banner) + WARN_MAX_LINES + 1 (exhausted) = 102 lines
// and on a healthy run it is EXACTLY 1 (the banner) with 0 warnings.
/// one line per op per this many ms; the rest are counted and summarised
pub const WARN_COOLDOWN_MS: u64 = 2000;
/// hard ceiling on warning lines for the life of the process
pub const WARN_MAX_LINES: u32 = 100;
/// the stated bound: banner + warnings + the exhausted notice
pub const WARN_LINE_BUDGET: u32 = 1 + WARN_MAX_LINES + 1;

/// what the warner remembers between breaches. Held behind one Mutex; passed
/// explicitly to `decide_warn` so the policy is testable without a clock.
#[derive(Default)]
pub struct OpState {
    /// unix ms of the last line printed for this op
    last_ms: u64,
    /// breaches counted but not printed since that line (cooldown)
    suppressed: u32,
    /// a COLD breach we deliberately did not print, in ms: deferred, not destroyed
    cold_ms: f64,
}

#[derive(Default)]
pub struct WarnState {
    /// per-op memory: cooldown, suppression count, deferred cold breach
    last: std::collections::HashMap<String, OpState>,
    /// warning lines printed so far (the banner is not one of them)
    pub emitted: u32,
    /// has the "budget exhausted" line been printed?
    exhausted: bool,
}

impl WarnState {
    pub fn new() -> Self {
        Self::default()
    }
    /// breaches counted but not printed, across all ops
    pub fn suppressed_total(&self) -> u32 {
        self.last.values().map(|o| o.suppressed).sum()
    }
    /// cold breaches held back by the warm-up window (for tests / assertions)
    pub fn cold_held(&self) -> usize {
        self.last.values().filter(|o| o.cold_ms > 0.0).count()
    }
}

/// THE POLICY, as a pure function of (state, name, ms, now, ceiling, cold) so a
/// test can drive a whole storm deterministically — no sleeps, no process env, no
/// wall clock. Returns the line to print, or None for silence.
/// `ms` is the op's measured duration; `now_ms` a unix-ms clock; `cold` says the
/// span STARTED inside the warm-up window (see WARMUP_MS).
pub fn decide_warn(st: &mut WarnState, name: &str, ms: f64, now_ms: u64, ceil_ms: u32, cold: bool) -> Option<String> {
    if !(ms > ceil_ms as f64) || warn_excluded(name) || name.is_empty() {
        return None;
    }
    if st.exhausted {
        return None;
    }
    let e = st.last.entry(name.to_string()).or_default();
    // COLD: the app was still starting. Remember the number, print nothing; it
    // rides out on this op's next warning as cold=<ms>.
    if cold {
        if ms > e.cold_ms {
            e.cold_ms = ms;
        }
        return None;
    }
    if st.emitted >= WARN_MAX_LINES {
        st.exhausted = true;
        let sup = st.suppressed_total();
        return Some(format!(
            "[perf][SLOW] warning budget exhausted: {} lines printed, {} further breaches counted and now silent (bound: perf.rs:WARN_MAX_LINES)",
            st.emitted, sup
        ));
    }
    let e = st.last.entry(name.to_string()).or_default();
    // cooldown: seen this op recently -> count it, say nothing
    if e.last_ms != 0 && now_ms.saturating_sub(e.last_ms) < WARN_COOLDOWN_MS {
        e.suppressed = e.suppressed.saturating_add(1);
        return None;
    }
    let sup = e.suppressed;
    let cold_ms = e.cold_ms;
    e.last_ms = now_ms;
    e.suppressed = 0;
    e.cold_ms = 0.0;
    st.emitted += 1;
    let over = ms - ceil_ms as f64;
    let mut line = format!(
        "[perf][SLOW] op={name} ms={ms:.1} ceiling={ceil_ms} over=+{over:.1} x{:.1}",
        ms / ceil_ms as f64
    );
    if cold_ms > 0.0 {
        line.push_str(&format!(" cold={cold_ms:.1}"));
    }
    if sup > 0 {
        line.push_str(&format!(" suppressed={sup}"));
    }
    Some(line)
}

// ===== THE NEGATIVE CONTROL: DELIBERATE SLOWNESS ============================
// A warning that never fired is not evidence. This hook makes ONE named span
// genuinely slow — it SLEEPS inside the timed region, so the ms the console
// reports is MEASURED by the same timer as every other span, not fabricated for
// the test. Removing the env removes the sleep and nothing else, which is what
// makes the two runs the SAME scenario.
//
// WHAT THE HOOK CANNOT DO, by construction — assume someone will try to use it
// to buy a green:
//   * it cannot touch SLOW_MS_CEIL or slow_ms(): different env var, different
//     parser, no path between them (proved by inject_cannot_loosen_the_ceiling).
//   * it only ever moves one direction — SLOWER. It can make the console louder,
//     never quieter, so it cannot silence a real breach.
//   * it is CLAMPED at INJECT_MAX_MS, so a fat-fingered 9999999 cannot wedge a
//     gate phase into a timeout.
/// env that injects a real delay into one named span: "<span_name>=<ms>"
pub const INJECT_ENV: &str = "OPENSIDIAN_SLOW_INJECT";
/// the most an injection may add to one span, in ms (values above are clamped)
pub const INJECT_MAX_MS: u64 = 5000;

/// PURE (takes the spec, reads nothing): "<span_name>=<ms>" -> (name, ms).
/// Empty name, non-numeric or zero ms, or a missing '=' -> None (no injection).
pub fn parse_inject(spec: Option<&str>) -> Option<(String, u64)> {
    let s = spec.map(str::trim).filter(|s| !s.is_empty())?;
    let (name, ms) = s.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let ms: u64 = ms.trim().parse().ok()?;
    if ms == 0 {
        return None;
    }
    Some((name.to_string(), ms.min(INJECT_MAX_MS)))
}

/// this process's injection (env read once)
fn inject() -> Option<&'static (String, u64)> {
    static I: OnceLock<Option<(String, u64)>> = OnceLock::new();
    I.get_or_init(|| parse_inject(std::env::var(INJECT_ENV).ok().as_deref())).as_ref()
}

/// how the banner states the injection: "none", or "<name>=<ms>ms"
pub fn inject_desc() -> String {
    match inject() {
        None => "none".to_string(),
        Some((n, ms)) => format!("{n}={ms}ms"),
    }
}

/// Called at the start of a timed span (inside the measured region). Sleeps only
/// for the one span named by INJECT_ENV; every other span pays one string compare.
pub fn inject_delay(name: &str) {
    if let Some((n, ms)) = inject() {
        if n == name {
            std::thread::sleep(std::time::Duration::from_millis(*ms));
        }
    }
}

fn warn_state() -> &'static Mutex<WarnState> {
    static S: OnceLock<Mutex<WarnState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(WarnState::new()))
}

/// EVERY span that ends passes through here — frontend (ui_spans) and backend
/// (span_ctx) alike — whether or not OPENSIDIAN_OTEL is set. Telemetry writing
/// to a file is optional; the console warning is not.
/// `start_ms` is when the span STARTED (unix ms): the warm-up window is judged on
/// the start, not the end, because a cold op that takes 144ms ENDS outside a
/// window it plainly began inside.
pub fn check_slow_at(name: &str, ms: f64, start_ms: u64) {
    let ceil = slow_ms();
    let cold = is_cold(start_ms);
    let line = {
        let mut st = warn_state().lock().unwrap_or_else(|e| e.into_inner());
        decide_warn(&mut st, name, ms, unix_ms(), ceil, cold)
    };
    if let Some(l) = line {
        eprintln!("{l}");
    }
}

/// backend spans: the start is derived from the measurement that just ended
pub fn check_slow(name: &str, ms: f64) {
    let now = unix_ms();
    check_slow_at(name, ms, now.saturating_sub(ms.max(0.0) as u64));
}

/* R18.1 THE SINK IS AN OPEN FD, NOT A PATH RE-OPENED PER SPAN.
   Landlock filters PATH LOOKUPS; it does not revoke a descriptor that is
   already open. opensidian confines itself to `sandbox::write_roots()` before
   the webview starts, so on a landlock kernel every later `open(OPENSIDIAN_OTEL)`
   is EACCES whenever the target sits outside vault/cfg/~.cache//tmp//run//dev//var/tmp.
   `emit`'s error is swallowed by every caller ("telemetry must never break the
   app"), so R18.1 — "with OPENSIDIAN_OTEL=<file> set, the app appends one
   OTLP/JSON line per span" — became SILENTLY FALSE the day the app first ran on
   a kernel that enforces the sandbox: app ran, spans were built, file never
   existed, nothing said so. Measured on the Hetzner box (kernel 6.8, landlock
   FullyEnforced, gate $OUT=/srv/out/<goal>): 0 bytes written, 0 diagnostics.
   Opening HERE, before `sandbox::enforce`, makes the sink work wherever it
   points — without widening the ruleset by one path. */
static SINK: OnceLock<Mutex<File>> = OnceLock::new();

/// Open the OPENSIDIAN_OTEL / OPENSIDIAN_PERF target and keep the fd for the life
/// of the process. MUST be called BEFORE `sandbox::enforce()` — after it, a
/// target outside the write roots can no longer be opened at all.
/// No-op when the env is unset (R18.3: unset = zero cost, no file, no syscall).
/// It prints EITHER WAY: a telemetry run that writes nothing must say so, since
/// the downstream reader (`otel-flat.sh`, the smoke's span windows) can only
/// see an empty file and cannot tell "no spans" from "no permission".
pub fn open_sink() {
    let Some(p) = target() else { return };
    match OpenOptions::new().create(true).append(true).open(p) {
        Ok(f) => {
            let _ = SINK.set(Mutex::new(f));
            eprintln!("otel: sink {} open (fd held across the sandbox)", p.display());
        }
        Err(e) => eprintln!("otel: sink {} UNWRITABLE ({e}) — R18.1 telemetry is OFF for this run", p.display()),
    }
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
            { "key": "service.name", "value": { "stringValue": "opensidian" } },
            { "key": "service.version", "value": { "stringValue": env!("CARGO_PKG_VERSION") } } ] },
        "scopeSpans": [{ "scope": { "name": "opensidian" }, "spans": spans.iter().map(span_json).collect::<Vec<_>>() }] }] })
}

/// append one OTLP/JSON line to `path` (create if missing). Errors are swallowed
/// by callers: telemetry must never break the app.
///
/// ONE `write_all` of the whole line, under a process-wide lock. The obvious
/// `writeln!(f, "{}", request(spans))` is NOT one write: `write_fmt` hands the
/// formatter's fragments to the file one by one, so two threads appending at the
/// same instant (a `log_spans` batch from the webview while a command thread
/// records its own span) interleaved CHARACTER BY CHARACTER and produced a line
/// no JSON parser accepts. That corruption is silent and total downstream:
/// scripts/otel-flat.sh is a single `jq`, jq aborts at the bad line, and every
/// span AFTER it disappears — the smoke's R18 window then measured 0 ed_patch
/// spans for 16 keystrokes and read it as "telemetry not live".
pub fn emit(path: &Path, spans: &[Span]) -> std::io::Result<()> {
    if spans.is_empty() {
        return Ok(());
    }
    let mut line = request(spans).to_string();
    line.push('\n');
    // the process sink, when this is the process's own target: ONE write_all
    // through the fd opened before the sandbox closed (see SINK above).
    if let (Some(m), Some(t)) = (SINK.get(), target()) {
        if t == path {
            let mut f = m.lock().unwrap_or_else(|e| e.into_inner());
            return f.write_all(line.as_bytes());
        }
    }
    static W: Mutex<()> = Mutex::new(());
    let _g = W.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(line.as_bytes())
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
    check_slow(name, ms);
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
/// APP-ATTRIBUTABLE duration of a frontend span, in ms — the number the warning
/// judges. Wall time MINUS vsync_ms, exactly as scripts/otel-flat.sh reports it
/// and as the lag budgets are written: vsync_ms is the idle wait for the next
/// frame tick (display cadence — 16.7ms at 60Hz, more under Xvfb), which the app
/// cannot make shorter. Warning on raw wall time would blame the display for the
/// app's latency and fire on healthy runs, which is the noise this feature exists
/// to avoid.
pub fn ui_span_ms(s: &Span) -> f64 {
    let wall = s.end_ns.saturating_sub(s.start_ns) as f64 / 1e6;
    let vsync = s.attrs.get("vsync_ms").and_then(|v| v.as_f64()).unwrap_or(0.0);
    (wall - vsync.max(0.0)).max(0.0)
}

pub fn ui_spans(list: &[Value]) {
    let spans: Vec<Span> = list.iter().map(ui_span).collect();
    // the console warning runs whether or not telemetry is writing to a file
    for s in &spans {
        check_slow_at(&s.name, ui_span_ms(s), (s.start_ns / 1_000_000) as u64);
    }
    if let Some(p) = target() {
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
        // ALWAYS timed. The old form measured only when OPENSIDIAN_OTEL was set,
        // which would have made the slow-op console warning a traced-runs-only
        // feature — invisible in exactly the ordinary run where a user notices
        // the lag. `$extra` (a json! literal) is still built ONLY when telemetry
        // is on, so an untraced run pays one Instant and no allocation.
        let __t0 = ::std::time::Instant::now();
        // INSIDE the measured region on purpose: the negative control's delay is
        // part of what the timer sees, so the ms the console prints is measured
        // the same way as every other span's. Without an injection this is one
        // string compare against None.
        $crate::perf::inject_delay($name);
        let __r = $e;
        let __ms = __t0.elapsed().as_secs_f64() * 1000.0;
        if $crate::perf::enabled() {
            $crate::perf::span_ctx($ctx.as_ref(), $name, __ms, $extra);
        } else {
            $crate::perf::check_slow($name, __ms);
        }
        __r
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE PIN. The env may make the ceiling stricter; every attempt to make it
    /// looser returns the pinned number. Written against the PURE function so it
    /// proves the rule itself, not one process's environment — and so it cannot
    /// race another test that sets env vars.
    #[test]
    fn env_can_tighten_the_slow_ceiling_but_never_loosen_it() {
        assert_eq!(SLOW_MS_CEIL, 100, "the ceiling moved: update lag-budgets.env MAX_INTERACTION_MS and docs, or put it back");
        // no override -> the pinned ceiling
        assert_eq!(effective_slow_ms(None), (100, SlowSrc::Ceiling));
        assert_eq!(effective_slow_ms(Some("")), (100, SlowSrc::Ceiling));
        assert_eq!(effective_slow_ms(Some("   ")), (100, SlowSrc::Ceiling));
        // STRICTER is honoured
        assert_eq!(effective_slow_ms(Some("25")), (25, SlowSrc::Tightened));
        assert_eq!(effective_slow_ms(Some(" 99 ")), (99, SlowSrc::Tightened));
        // LOOSER is refused, at every scale, including the absurd
        for v in ["100", "101", "250", "5000", "999999", "4294967295"] {
            assert_eq!(effective_slow_ms(Some(v)), (100, SlowSrc::RefusedLoosen), "{v} loosened the ceiling");
        }
        // junk, negatives, floats and overflow are refused too (never "disabled")
        for v in ["0", "-1", "-999", "1e9", "100.5", "abc", "99999999999999999999", "inf", "null"] {
            let (ms, src) = effective_slow_ms(Some(v));
            assert_eq!(ms, 100, "{v} moved the ceiling");
            assert!(matches!(src, SlowSrc::RefusedJunk | SlowSrc::RefusedLoosen), "{v} -> {src:?}");
        }
        // the banner states the effective number and says the override was refused
        let b = slow_banner();
        assert!(b.starts_with("[perf] slow_ms="), "{b}");
        assert!(!b.contains('\n'), "the banner is ONE line: {b}");
    }

    /// THE NEGATIVE CONTROL'S HOOK, pinned to what it is allowed to do: add time
    /// to ONE named span. Parsing is pure, so these are facts about the rule.
    #[test]
    fn inject_spec_parses_only_name_equals_positive_ms() {
        assert_eq!(parse_inject(Some("read_note=250")), Some(("read_note".into(), 250)));
        assert_eq!(parse_inject(Some("  read_note = 250 ")), Some(("read_note".into(), 250)));
        // clamped, so a typo cannot hang a phase into its timeout
        assert_eq!(parse_inject(Some("read_note=9999999")), Some(("read_note".into(), INJECT_MAX_MS)));
        assert_eq!(INJECT_MAX_MS, 5000);
        // no injection: unset, empty, no '=', empty name, zero, junk, negative, float
        for s in [None, Some(""), Some("   "), Some("read_note"), Some("=250"), Some("read_note=0"), Some("read_note=abc"), Some("read_note=-5"), Some("read_note=2.5")] {
            assert_eq!(parse_inject(s), None, "{s:?} produced an injection");
        }
    }

    /// THE HOOK MAY NOT BUY A GREEN. It shares no code and no env var with the
    /// ceiling, so no spec — however hostile — can move slow_ms or silence a
    /// breach. The only direction it moves anything is SLOWER.
    #[test]
    fn inject_cannot_loosen_the_ceiling() {
        for hostile in [
            "OPENSIDIAN_SLOW_MS=99999",
            "ceiling=99999",
            "SLOW_MS_CEIL=99999",
            "*=0",
            "read_note=99999999",
        ] {
            // the ceiling is computed from SLOW_ENV alone and never sees this string
            assert_eq!(effective_slow_ms(None), (SLOW_MS_CEIL, SlowSrc::Ceiling), "{hostile}");
            assert_eq!(effective_slow_ms(Some(hostile)).0, SLOW_MS_CEIL, "{hostile} moved the ceiling");
            // and a breach still warns while it is set
            assert!(
                decide_warn(&mut WarnState::new(), "read_note", (SLOW_MS_CEIL + 1) as f64, 1, SLOW_MS_CEIL, false).is_some(),
                "{hostile} silenced a breach"
            );
            // whatever it parses to, it is a DELAY in ms, never a threshold
            if let Some((_, ms)) = parse_inject(Some(hostile)) {
                assert!(ms <= INJECT_MAX_MS, "{hostile} -> {ms}ms is above the clamp");
            }
        }
    }

    /// the banner says what is injected, in the same one line: a reader of the log
    /// never has to guess whether the run they are looking at was rigged.
    #[test]
    fn banner_states_the_injection_in_one_line() {
        let b = slow_banner();
        assert!(b.contains(" inject="), "banner hides the injection: {b}");
        assert!(!b.contains('\n'), "the banner is ONE line: {b}");
        // unset in the test process -> "none"
        assert_eq!(inject_desc(), "none", "a test process must not be rigged");
        assert!(b.contains(&format!(" budget={WARN_LINE_BUDGET}")), "banner hides the stated bound: {b}");
    }

    /// THE UNTRACED RUN IS A DIRECTION, so the log must say which one it is. The
    /// smoke phase launches the app three times — injected+traced, clean+traced,
    /// injected+UNTRACED — and only the third proves the warning is not
    /// traced-runs-only. Nothing in a log distinguished the third from the first,
    /// so the phase had to trust its own launcher. The banner now reports
    /// [`enabled`] verbatim, and it agrees with it in both states.
    #[test]
    fn banner_reports_whether_telemetry_is_writing() {
        let b = slow_banner();
        let want = if enabled() { " otel=on " } else { " otel=off " };
        assert!(b.contains(want), "banner does not state the telemetry state as {want:?}: {b}");
        // exactly one of the two, never both, and still one line
        assert_ne!(b.contains(" otel=on "), b.contains(" otel=off "), "ambiguous otel= field: {b}");
        assert!(!b.contains('\n'), "the banner is ONE line: {b}");
    }

    /// THE WARM-UP WINDOW, both directions. A breach that STARTED cold is silent
    /// but REMEMBERED: the next warm breach of that op carries cold=<ms>, so the
    /// number the console declined to shout is still in the console.
    #[test]
    fn a_cold_breach_is_silent_and_then_reported_as_cold() {
        let mut st = WarnState::new();
        // the measured cold ramp: 144ms starting inside boot -> not a word
        assert_eq!(decide_warn(&mut st, "note_open", 144.0, 1_000, 100, true), None, "a cold breach warned");
        assert_eq!(st.emitted, 0);
        assert_eq!(st.cold_held(), 1, "the cold breach was thrown away instead of remembered");
        // the same op, warm, breaching -> warns AND surfaces the cold number
        let l = decide_warn(&mut st, "note_open", 250.0, 9_000, 100, false).expect("a warm breach must warn");
        assert!(l.contains("op=note_open ms=250.0"), "{l}");
        assert!(l.contains("cold=144.0"), "the deferred cold measurement was dropped: {l}");
        assert_eq!(st.cold_held(), 0, "cold is reported once, not on every line");
        let l2 = decide_warn(&mut st, "note_open", 300.0, 9_000 + WARN_COOLDOWN_MS + 1, 100, false).unwrap();
        assert!(!l2.contains("cold="), "cold repeated on a later line: {l2}");
        // and a cold span UNDER the ceiling is not a breach at all
        assert_eq!(decide_warn(&mut WarnState::new(), "note_open", 76.0, 1, 100, true), None);
    }

    /// THE WINDOW IS PINNED AND ENVLESS. The ceiling has one env var that may only
    /// tighten; the warm-up has NONE, so there is no string anyone can set to widen
    /// the blind spot. The source is the proof: no env read mentions it.
    #[test]
    fn warmup_window_is_pinned_and_envless() {
        assert_eq!(WARMUP_MS, 2000, "the warm-up window moved: update the measurement in its doc comment, or put it back");
        let src = include_str!("perf.rs");
        let envs: Vec<&str> = src.lines().filter(|l| l.contains("var_os(") || l.contains("var(")).collect();
        for l in &envs {
            assert!(!l.contains("WARMUP"), "an env var reaches the warm-up window: {l}");
        }
        // is_cold with no process start stamped (this test binary never calls
        // mark_start) must judge NOTHING cold — fail loud, never silent
        assert!(!is_cold(0), "an unstamped process treated a span as cold");
        assert!(!is_cold(u64::MAX), "an unstamped process treated a span as cold");
        // the banner states the window as a number
        assert!(slow_banner().contains(&format!(" warmup={WARMUP_MS}ms")), "{}", slow_banner());
    }

    /// THE THIRD SILENCE CHANNEL. The ceiling is pinned and the warm-up window is
    /// pinned, but the per-op COOLDOWN silences breaches too, and until this test
    /// existed every assertion about it was written as `t0 + WARN_COOLDOWN_MS + 1`
    /// — self-referential, so raising the constant to an hour kept the whole suite
    /// green while the console went quiet for every repeat breach. A test that
    /// moves with the number it guards is not a pin. These numbers are LITERALS.
    #[test]
    fn cooldown_and_cap_are_pinned_literals_and_envless() {
        assert_eq!(WARN_COOLDOWN_MS, 2000, "the per-op cooldown moved: it is the third way to silence a warning, so it is pinned like the ceiling");
        assert_eq!(WARN_MAX_LINES, 100, "the per-process cap moved; 0 would silence the feature entirely while reading as noise reduction");
        assert_eq!(WARN_LINE_BUDGET, 102, "the stated bound is 1 + 100 + 1");

        // BEHAVIOUR, in literal milliseconds: 1999ms after a warning is silence,
        // 2000ms is a line. Both directions, so a wider OR narrower window fails.
        let mut st = WarnState::new();
        let t0: u64 = 1_000_000;
        assert!(decide_warn(&mut st, "note_open", 300.0, t0, 100, false).is_some(), "the first breach must warn");
        assert!(decide_warn(&mut st, "note_open", 300.0, t0 + 1999, 100, false).is_none(), "1999ms after a warning must still be inside the cooldown");
        let l = decide_warn(&mut st, "note_open", 300.0, t0 + 2000, 100, false)
            .expect("2000ms after a warning the cooldown is over — a longer window is a loosened rule");
        assert!(l.contains("suppressed=1"), "the breach held back at 1999ms must be counted, not dropped: {l}");

        // no env var may reach the cooldown, the cap or the budget
        let src = include_str!("perf.rs");
        for l in src.lines().filter(|l| l.contains("var_os(") || l.contains("var(")) {
            for knob in ["COOLDOWN", "MAX_LINES", "LINE_BUDGET"] {
                assert!(!l.contains(knob), "an env var reaches {knob}: {l}");
            }
        }
        // and the console STATES it: an undeclared blind spot is one nobody thought about
        assert!(slow_banner().contains(" cooldown=2000ms"), "the banner does not state the cooldown: {}", slow_banner());
    }

    /// the out-of-scope table is data a reviewer reads: every entry needs a name
    /// and a REASON, and no entry may be duplicated.
    #[test]
    fn warn_exclusions_are_named_reasoned_and_unique() {
        assert!(!WARN_EXCLUDE.is_empty());
        for (n, why) in WARN_EXCLUDE {
            assert!(!n.is_empty() && !n.contains(' '), "bad op name {n:?}");
            assert!(why.len() > 30, "op {n} has no real reason: {why:?}");
            assert!(warn_excluded(n));
        }
        let mut names: Vec<&str> = WARN_EXCLUDE.iter().map(|(n, _)| *n).collect();
        let n0 = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), n0, "duplicate entry in WARN_EXCLUDE");
        assert!(!warn_excluded("key_to_paint"), "an interaction op must NOT be excluded");
        assert!(!warn_excluded("note_open"));
    }

    /// THE WARNING POLICY, both directions. A warning that never fires is not
    /// evidence; one that always fires is noise. Driven through the pure
    /// `decide_warn` with an explicit clock, so the storm below is deterministic.
    #[test]
    fn warning_fires_over_the_ceiling_and_is_silent_under_it() {
        let mut st = WarnState::new();
        let t = 1_000_000u64;
        // UNDER and EXACTLY AT the ceiling: silence. 100ms is not "over 100ms".
        assert_eq!(decide_warn(&mut st, "note_open", 0.4, t, 100, false), None);
        assert_eq!(decide_warn(&mut st, "note_open", 99.9, t, 100, false), None);
        assert_eq!(decide_warn(&mut st, "note_open", 100.0, t, 100, false), None);
        assert_eq!(st.emitted, 0, "a healthy run must print ZERO warnings");
        // OVER: one line, naming the op and the measured ms
        let l = decide_warn(&mut st, "note_open", 214.68, t, 100, false).expect("breach must warn");
        assert!(l.contains("op=note_open"), "{l}");
        assert!(l.contains("ms=214.7"), "the measured number must be IN the line: {l}");
        assert!(l.contains("ceiling=100"), "{l}");
        assert!(l.contains("over=+114.7"), "{l}");
        assert!(l.starts_with("[perf][SLOW] "), "{l}");
        assert!(!l.contains("suppressed"), "first line has nothing to summarise: {l}");
        assert_eq!(st.emitted, 1);
        // an EXCLUDED op is over the ceiling by design and stays silent
        for (name, _) in WARN_EXCLUDE {
            assert_eq!(decide_warn(&mut st, name, 9_999.0, t, 100, false), None, "{name} warned");
        }
        assert_eq!(st.emitted, 1);
        // a tightened ceiling warns where the pinned one would not
        let mut st2 = WarnState::new();
        assert!(decide_warn(&mut st2, "key_to_paint", 40.0, t, 25, false).is_some());
    }

    /// THE NOISE BOUND. 500 breaches of one op inside the cooldown produce ONE
    /// line, and the next line after the window says how many it stood for.
    #[test]
    fn a_storm_is_summarised_not_streamed() {
        let mut st = WarnState::new();
        let t0 = 5_000_000u64;
        let mut lines = 0;
        for i in 0..500u64 {
            // 500 breaches, 1ms apart = 500ms, well inside the 2000ms cooldown
            if decide_warn(&mut st, "tab_switch", 300.0, t0 + i, 100, false).is_some() {
                lines += 1;
            }
        }
        assert_eq!(lines, 1, "500 breaches in one cooldown window must print ONE line");
        assert_eq!(st.suppressed_total(), 499);
        // after the window: one line, carrying the count of what it stands for
        let l = decide_warn(&mut st, "tab_switch", 300.0, t0 + WARN_COOLDOWN_MS + 1, 100, false).unwrap();
        assert!(l.contains("suppressed=499"), "{l}");
        assert_eq!(st.suppressed_total(), 0, "the counter resets once it has been reported");
        // the cooldown is PER OP: a different op is not silenced by tab_switch
        assert!(decide_warn(&mut st, "pane_split", 300.0, t0 + WARN_COOLDOWN_MS + 1, 100, false).is_some());
    }

    /// THE STATED BOUND. However bad the machine gets, this feature cannot print
    /// more than WARN_LINE_BUDGET lines for the life of the process.
    #[test]
    fn console_output_is_bounded_by_a_number() {
        assert_eq!(WARN_LINE_BUDGET, 102, "the documented bound is 1 banner + 100 warnings + 1 notice");
        let mut st = WarnState::new();
        let mut lines = 0u32;
        let mut exhausted = 0u32;
        // 400 distinct ops, each far over the ceiling, each outside every cooldown
        for i in 0..400u32 {
            if let Some(l) = decide_warn(&mut st, &format!("op_{i}"), 5_000.0, 9_000_000 + i as u64 * 10_000, 100, false) {
                lines += 1;
                if l.contains("budget exhausted") {
                    exhausted += 1;
                    assert!(l.contains("100 lines printed"), "{l}");
                }
            }
        }
        assert_eq!(st.emitted, WARN_MAX_LINES, "the cap is the cap");
        assert_eq!(exhausted, 1, "the budget notice is printed exactly ONCE, then silence");
        assert_eq!(lines, WARN_LINE_BUDGET - 1, "warnings + notice = the bound minus the banner");
    }

    /// vsync is the display's cadence, not the app's latency: warning on raw
    /// wall time would fire on a healthy run under a slow Xvfb.
    #[test]
    fn ui_duration_subtracts_the_frame_wait() {
        let mk = |wall: f64, attrs: Value| Span {
            name: "note_open".into(),
            trace_id: new_trace_id(),
            span_id: new_span_id(),
            parent_span_id: String::new(),
            start_ns: 1_000_000_000_000,
            end_ns: 1_000_000_000_000 + (wall * 1e6) as u128,
            attrs,
        };
        assert!((ui_span_ms(&mk(120.0, json!({}))) - 120.0).abs() < 0.01);
        // 120ms wall of which 45ms was waiting for the next frame -> 75ms of app
        assert!((ui_span_ms(&mk(120.0, json!({"vsync_ms": 45.0}))) - 75.0).abs() < 0.01);
        assert!(decide_warn(&mut WarnState::new(), "note_open", ui_span_ms(&mk(120.0, json!({"vsync_ms": 45.0}))), 1, 100, false).is_none());
        assert!(decide_warn(&mut WarnState::new(), "note_open", ui_span_ms(&mk(120.0, json!({}))), 1, 100, false).is_some());
        // junk vsync cannot make a duration negative or inflate it
        assert!((ui_span_ms(&mk(50.0, json!({"vsync_ms": -9.0}))) - 50.0).abs() < 0.01);
        assert_eq!(ui_span_ms(&mk(50.0, json!({"vsync_ms": 900.0}))), 0.0);
    }

    fn first_span(line: &str) -> Value {
        let v: Value = serde_json::from_str(line).unwrap();
        let rs = &v["resourceSpans"][0];
        assert_eq!(rs["resource"]["attributes"][0]["value"]["stringValue"], "opensidian");
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

        let p = std::env::temp_dir().join(format!("opensidian-otel-test-{}.jsonl", std::process::id()));
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
        if std::env::var_os("OPENSIDIAN_OTEL").is_none() && std::env::var_os("OPENSIDIAN_PERF").is_none() {
            assert!(!enabled());
            span("noop", 1.0, json!({}));
            ui_spans(&[json!({"name":"noop"})]);
        }
    }

    /// R18 regression: EVERY line must be whole JSON, even when threads append at
    /// the same instant. The old `writeln!(f, "{}", ...)` split one line into many
    /// small writes, so a `log_spans` batch and a command's own span interleaved
    /// mid-line — and one unparseable line silently truncates every consumer
    /// (scripts/otel-flat.sh is a single jq: it aborts there and every span after
    /// it vanishes, which the smoke reads as "telemetry not live").
    #[test]
    fn concurrent_emit_never_interleaves_a_line() {
        let p = std::env::temp_dir().join(format!("opensidian-otel-race-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let p = p.clone();
                std::thread::spawn(move || {
                    for i in 0..64 {
                        // a batch, so the line is long enough to need several writes
                        let spans: Vec<Span> = (0..4)
                            .map(|k| ended(None, "ed_patch", 1.5, json!({"thread": t, "i": i, "k": k, "pad": "x".repeat(64)})))
                            .collect();
                        emit(&p, &spans).unwrap();
                    }
                })
            })
            .collect();
        for h in threads {
            h.join().unwrap();
        }
        let body = std::fs::read_to_string(&p).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 8 * 64, "one line per emit, none split or merged");
        for (n, l) in lines.iter().enumerate() {
            let v: Value = serde_json::from_str(l).unwrap_or_else(|e| panic!("line {n} is not JSON: {e}"));
            assert_eq!(v["resourceSpans"][0]["scopeSpans"][0]["spans"].as_array().unwrap().len(), 4);
        }
        let _ = std::fs::remove_file(&p);
    }

    /* R18.1 UNDER AN ENFORCED SANDBOX — the regression this file's SINK exists
       for. The old `emit` re-opened OPENSIDIAN_OTEL for every span; on a kernel
       that enforces landlock that open is EACCES for any target outside the
       write roots, and the error is swallowed, so telemetry died in silence.
       The test asserts BOTH halves on the same enforced thread:
         by_path == false   the old behaviour would write nothing (the bug)
         by_fd   == true    a descriptor opened BEFORE enforce still writes
       $HOME itself is the "outside" path: sandbox grants ReadDir on it and
       write on ~/.cache only, which is exactly the shape of the gate's
       /srv/out/<goal>. On a kernel with no landlock the test SKIPS LOUDLY —
       it prints what the kernel lacks, it never passes vacuously. */
    #[test]
    fn otel_sink_survives_the_sandbox_that_kills_a_path_open() {
        use crate::sandbox;
        use std::io::Write as _;
        // TEST ISOLATION, AND WHY IT IS A CHILD PROCESS.
        // `sandbox::enforce()` publishes its vault into `sandbox::CONFINED`, a
        // process-global `OnceLock<PathBuf>` — FIRST WRITER WINS, permanently.
        // cargo runs every #[test] as a thread in ONE process, so two tests that
        // both call enforce() race for that slot: whoever loses still sees the
        // winner's vault through `sandbox::allows()`. The pre-existing
        // `sandbox::tests::confines_reads_to_vault` asserts
        // `allows(&its_own_vault) == true`, so when THIS test won the race that
        // test failed with `left: (false,false,true,false,false)` vs
        // `right: (..,true,..)` — a real, order-dependent regression this test
        // introduced (gate log 0ae56ee: "112 passed; 1 failed"; reproduced on the
        // box, where running as plain root also exposes the capsh-dependent
        // lw_link_mention test). Serialising the two would not fix it: a OnceLock
        // cannot be reset, so the loser is poisoned whatever the order.
        // So this test re-execs the test binary and does its work in a FRESH
        // process, where it is the only caller of enforce() and CONFINED is
        // unset. The product's real enforce() stays in the assertion path and no
        // pre-existing assertion moves.
        const GUARD: &str = "OPENSIDIAN_OTELSINK_CHILD";
        if std::env::var_os(GUARD).is_none() {
            let exe = std::env::current_exe().expect("current_exe for the isolated re-exec");
            let st = std::process::Command::new(exe)
                .args([
                    "--exact",
                    "perf::tests::otel_sink_survives_the_sandbox_that_kills_a_path_open",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(GUARD, "1")
                .status()
                .expect("re-exec the test binary for sandbox isolation");
            assert!(
                st.success(),
                "the ISOLATED (child-process) run of this test failed — see its output above; \
                 the assertions live in the child so that sandbox::CONFINED is unset there"
            );
            return;
        }
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            eprintln!("otel sink test SKIPPED: no HOME in the environment");
            return;
        };
        let base = std::env::temp_dir().join(format!("opensidian-otelsink-{}", std::process::id()));
        let (vault, cfg) = (base.join("vault"), base.join("cfg.json"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&vault).unwrap();
        // outside every write root, like the gate's $OUT
        let sink_path = home.join(format!(".opensidian-otel-sink-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&sink_path);
        let pre = OpenOptions::new().create(true).append(true).open(&sink_path).expect("pre-open (before enforce)");
        let (sp, v2, c2) = (sink_path.clone(), vault.clone(), cfg.clone());
        // restrict_self is per-thread: enforce in a child, keep the parent free to clean up
        let res = std::thread::spawn(move || {
            match sandbox::enforce(&v2, &c2) {
                Err(e) => {
                    eprintln!("otel sink test SKIPPED: this kernel cannot enforce landlock ({e})");
                    None
                }
                Ok(landlock::RulesetStatus::NotEnforced) => {
                    eprintln!("otel sink test SKIPPED: landlock reported NotEnforced on this kernel");
                    None
                }
                Ok(_) => {
                    let by_path = OpenOptions::new().create(true).append(true).open(&sp).is_ok();
                    let mut f = pre;
                    let by_fd = f.write_all(b"{\"resourceSpans\":[]}\n").and_then(|_| f.flush()).is_ok();
                    Some((by_path, by_fd))
                }
            }
        })
        .join()
        .unwrap();
        let bytes = std::fs::metadata(&sink_path).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_file(&sink_path);
        let _ = std::fs::remove_dir_all(&base);
        if let Some(r) = res {
            assert_eq!(r, (false, true), "(open-by-path, write-through-held-fd) under an enforced ruleset");
            assert_eq!(bytes, 21, "the held fd really wrote the line ({bytes} bytes)");
        }
    }
}
