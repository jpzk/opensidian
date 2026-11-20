// SPDX-License-Identifier: GPL-3.0-or-later
// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
/* Sync M6b — the sync/history command GATE and the embedded engine.
   docs/sync/spec.md §7, threat-model.md T5 / R1 (plan risk "pentest ROOT").

   THE BOUNDARY. Community plugins share the app page's JS realm (loader.js
   seal: defence in depth, not a sandbox — __TAURI_INTERNALS__ stays
   reachable), so nothing a page script checks can tell the core UI from a
   plugin. The trust decision is therefore made on something page script
   cannot choose: the LABEL of the webview the IPC message arrived from.
   Every sync/history command (GUARDED) answers only the webview labelled
   SYNC_LABEL, a separate window this module builds from the fixed app page
   SYNC_PAGE (no plugin loader, no community code, same app CSP). The main
   window — the realm plugins run in — gets EPERM before anything is read.
     - a webview's label is set by the Rust side that builds it; navigation
       or an iframe inside "main" stays "main";
     - the capability (capabilities/default.json) grants no
       create-webview / create-webview-window, so page script cannot build
       a webview with the trusted label (test: no_capability_builds_webviews);
     - sync_open (NOT guarded) only shows/focuses that window — a plugin can
       open it, it cannot read or drive it;
     - the gate runs before any vault/state access (test: the mock-runtime
       IPC test below invokes EVERY guarded command from "main" and "sync").

   THE EMBEDDED ENGINE. syncd's LocalEngine in-process (crates/syncd), state
   dir `<app data>/sync/<vault id>/` (0700; vault.json, keystore entries,
   engine objects + per-vault flock shared with a running opensidian-syncd).
   File history of a sync pass goes into the app's ONE history store
   (FILEHIST, through AppHist) — never a second opener of the same journal.
   Network work runs on the blocking pool.

   REVEAL. The backup words leave the keystore only after a native GTK
   question (consent Kind::Reveal — page script cannot answer it) and only
   to the sync window. The verify quiz is allowed for QUIZ_WINDOW after a
   granted reveal (it is an oracle on the words otherwise). Linux has no
   per-item user-presence gate (threat-model R5; open question 6). */
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use sync_core::history::{HistoryError, Store};
use sync_core::keystore::FixedPresence;
use sync_core::merge::ConflictMode;
use syncd::api::{ApiError, ApiResult, SyncApi};
use syncd::local::{HistSink, Join, LocalEngine, Move, Setup};
use tauri::State;

use crate::Vault;

/// the one webview label the sync/history commands answer
pub const SYNC_LABEL: &str = "sync";
/// the app page that webview loads (ui/sync.html)
pub const SYNC_PAGE: &str = "sync.html";

/// every command behind gate(). The tests check that each one is in the app's
/// invoke handler, calls gate first, and is refused from "main".
pub const GUARDED: &[&str] = &[
    "history_list",
    "history_read",
    "history_restore",
    "history_deleted",
    "history_restore_deleted",
    "history_purge",
    "history_diff",
    "sync_status",
    "sync_setup",
    "sync_now",
    "sync_set_mode",
    "sync_reveal",
    "sync_quiz",
    "sync_census",
    "sync_move",
];

/// Ok only for the trusted sync webview; EPERM for every other (the main
/// window = the plugin realm, a recorder webview, anything later)
pub fn gate(label: &str, cmd: &str) -> Result<(), String> {
    if label == SYNC_LABEL {
        return Ok(());
    }
    eprintln!("[sync] refused {cmd} from webview {label:?}");
    Err(format!("EPERM: {cmd}: sync and file history answer only the sync window"))
}

/// how long after a granted reveal the verify quiz may check answers
pub const QUIZ_WINDOW: Duration = Duration::from_secs(600);
static REVEALED: Mutex<Option<Instant>> = Mutex::new(None);

fn api(e: ApiError) -> String {
    e.to_string()
}

/// `<app data>/sync/<vault id>` of the open vault (None = history/sync off)
fn state_dir(root: &Path) -> Option<PathBuf> {
    crate::hist_locate(root).map(|(data, id)| data.join("sync").join(id))
}

fn open_vault(v: &Vault) -> Result<(PathBuf, PathBuf), String> {
    let root = crate::cur_vault(v).ok_or("no vault open")?;
    let st = state_dir(&root).ok_or("sync is unavailable (no app data dir)")?;
    Ok((root, st))
}

fn engine(st: &Path, presence: bool) -> Result<LocalEngine, String> {
    // every server connection asks the app offline policy (#144 toggle 2)
    LocalEngine::open(st, Box::new(FixedPresence(presence))).map(|e| e.with_net_gate(crate::net_allowed)).map_err(api)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| format!("sync: {e}"))?
}

/// a sync pass's history goes to the app's store (filehist.rs FILEHIST)
struct AppHist {
    root: PathBuf,
}

impl HistSink for AppHist {
    fn with(&mut self, f: &mut dyn FnMut(&mut Store) -> Result<(), HistoryError>) -> ApiResult<()> {
        match crate::FILEHIST.with(&self.root, &crate::hist_locate, |s| f(s)) {
            Some(r) => r.map_err(|e| ApiError::new("history", e.to_string())),
            None => Ok(()), // history off for this vault: best effort, like every hook
        }
    }
}

#[derive(Serialize)]
pub struct View {
    configured: bool,
    status: Option<syncd::api::Status>,
}

/// configured? + the engine's status (server reachability, devices, head)
#[tauri::command]
pub async fn sync_status<R: tauri::Runtime>(win: tauri::WebviewWindow<R>, v: State<'_, Vault>) -> Result<View, String> {
    gate(win.label(), "sync_status")?;
    let (_, st) = open_vault(&v)?;
    blocking(move || match LocalEngine::open(&st, Box::new(FixedPresence(false))).map(|e| e.with_net_gate(crate::net_allowed)) {
        Err(e) if e.code == "not-configured" => Ok(View { configured: false, status: None }),
        Err(e) => Err(api(e)),
        Ok(e) => Ok(View { configured: true, status: Some(e.status().map_err(api)?) }),
    })
    .await
}

#[derive(Serialize)]
pub struct SetupDone {
    /// Some only for a NEW vault: the words, shown once (they are also in the
    /// backup entry behind Reveal)
    words: Option<String>,
}

/// set this vault up: `join` = "create" (secret = the admin's one-time
/// enrollment token) | "words" (secret = the 12 backup words)
#[tauri::command]
pub async fn sync_setup<R: tauri::Runtime>(
    win: tauri::WebviewWindow<R>,
    v: State<'_, Vault>,
    url: String,
    device: String,
    join: String,
    secret: String,
    ca_file: Option<String>,
) -> Result<SetupDone, String> {
    gate(win.label(), "sync_setup")?;
    crate::net_allowed("sync setup")?;
    let (root, st) = open_vault(&v)?;
    let secret = zeroize::Zeroizing::new(secret);
    blocking(move || {
        let j = match join.as_str() {
            "create" => Join::Create { enrollment_token: secret.trim() },
            "words" => Join::Words { words: secret.as_str() },
            _ => return Err(format!("unknown join {join:?}")),
        };
        let device = if device.trim().is_empty() { "desktop".to_string() } else { device.trim().to_string() };
        let (_, words) = LocalEngine::setup(Setup {
            state: &st,
            url: url.trim(),
            vault_dir: &root,
            device_name: &device,
            ca_file: ca_file.filter(|c| !c.is_empty()).map(PathBuf::from),
            join: j,
        })
        .map_err(api)?;
        let words = if join == "create" { words.map(|w| w.to_string()) } else { None };
        Ok(SetupDone { words })
    })
    .await
}

/// one sync pass now (commit local edits, pull/merge/push, write the head)
#[tauri::command]
pub async fn sync_now<R: tauri::Runtime>(win: tauri::WebviewWindow<R>, v: State<'_, Vault>) -> Result<syncd::api::SyncSummary, String> {
    gate(win.label(), "sync_now")?;
    let (root, st) = open_vault(&v)?;
    blocking(move || engine(&st, false)?.sync_with(&mut AppHist { root }).map_err(api)).await
}

/// "auto-merge" (default) | "conflict-file" — this device only
#[tauri::command]
pub fn sync_set_mode<R: tauri::Runtime>(win: tauri::WebviewWindow<R>, v: State<'_, Vault>, mode: String) -> Result<(), String> {
    gate(win.label(), "sync_set_mode")?;
    let (_, st) = open_vault(&v)?;
    let m = match mode.as_str() {
        "auto-merge" => ConflictMode::AutoMerge,
        "conflict-file" => ConflictMode::ConflictFile,
        _ => return Err(format!("unknown conflict mode {mode:?}")),
    };
    engine(&st, false)?.set_conflict_mode(m).map_err(api)
}

/// the native question in front of a reveal; Wry only (the mock runtime of
/// the gate test has no dialog: refused)
async fn ask_reveal<R: tauri::Runtime>(win: &tauri::WebviewWindow<R>) -> Result<bool, String> {
    let w = native_window(win).ok_or("reveal needs the native dialog")?;
    let slot = crate::consent::admit()?;
    crate::consent::ask(slot, w, crate::consent::Kind::Reveal, crate::consent::reveal_question()).await
}

fn native_window<R: tauri::Runtime>(win: &tauri::WebviewWindow<R>) -> Option<tauri::Window> {
    let any: &dyn std::any::Any = win;
    any.downcast_ref::<tauri::WebviewWindow<tauri::Wry>>().map(|w| w.as_ref().window())
}

/// the backup words, after the native question. The page shows them without
/// a copy button and hides them again (ui/sync.js).
#[tauri::command]
pub async fn sync_reveal<R: tauri::Runtime>(win: tauri::WebviewWindow<R>, v: State<'_, Vault>) -> Result<String, String> {
    gate(win.label(), "sync_reveal")?;
    let (_, st) = open_vault(&v)?;
    if !ask_reveal(&win).await? {
        return Err("denied: the backup words stay hidden".into());
    }
    let words = blocking(move || engine(&st, true)?.reveal_words().map(|w| w.to_string()).map_err(api)).await?;
    *REVEALED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    Ok(words)
}


#[derive(Serialize)]
pub struct MoveDone {
    /// the NEW vault's backup words, shown once (also in its backup entry)
    words: String,
    moved: syncd::api::Moved,
}

/// "Vault compromised -> move to a new vault" (burn and move, no revocation;
/// spec §4.8). After the native question: a new vault on the same server from
/// the admin's one-time enrollment token, seeded from this folder (history
/// stays in the app's per-folder store), then the old vault is destroyed with
/// its words' K_destroy proof. The old words come from this device's backup
/// entry; `words` only if the device does not hold them (joined by pairing).
#[tauri::command]
pub async fn sync_move<R: tauri::Runtime>(
    win: tauri::WebviewWindow<R>,
    v: State<'_, Vault>,
    token: String,
    words: Option<String>,
) -> Result<MoveDone, String> {
    gate(win.label(), "sync_move")?;
    crate::net_allowed("sync move")?;
    let (root, st) = open_vault(&v)?;
    let token = zeroize::Zeroizing::new(token);
    let typed = words.map(zeroize::Zeroizing::new);
    if token.trim().is_empty() {
        return Err("an enrollment token for the new vault is needed (opensidian-sync-server vault create)".into());
    }
    let w = native_window(&win).ok_or("move needs the native dialog")?;
    // the vault the user is asked about: the worker refuses a different one
    // (the state dir re-pointed while the dialog was up)
    let approved = {
        let st = st.clone();
        blocking(move || Ok(engine(&st, true)?.config().vault_id)).await?
    };
    let slot = crate::consent::admit()?;
    if !crate::consent::ask(slot, w, crate::consent::Kind::Move, crate::consent::move_question()).await? {
        return Err("denied: the vault was not moved".into());
    }
    let done = blocking(move || {
        let e = engine(&st, true)?;
        if e.config().vault_id != approved {
            return Err("denied: the sync vault changed while the move was being approved; nothing was moved".into());
        }
        let old = match typed.filter(|t| !t.trim().is_empty()) {
            Some(t) => t,
            None => e.reveal_words().map_err(api)?,
        };
        let (_, moved, nw) = e
            .move_vault(
                Move { old_words: old.as_str(), enrollment_token: token.trim(), url: None, ca_file: None },
                Some(&mut AppHist { root }),
            )
            .map_err(api)?;
        Ok(MoveDone { words: nw.to_string(), moved })
    })
    .await?;
    // the new words were just shown: the verify quiz may check them
    *REVEALED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    Ok(done)
}
/// census-gate (#30): the TEST probe of the sync window. ui/sync.js publishes
/// its page state (pane, configured, mode, word COUNT, quiz positions, history
/// list/diff counts, control rects — never a word or a secret) as `[k:v]`
/// tokens, and this puts them in the window's native title, where the GUI
/// harness reads every window (xdotool). Answers only the sync webview, and
/// only when the process runs with the census on (OPENSIDIAN_CENSUS=1 / devui).
#[tauri::command]
pub fn sync_census<R: tauri::Runtime>(win: tauri::WebviewWindow<R>, tokens: String) -> Result<(), String> {
    gate(win.label(), "sync_census")?;
    if !crate::census::on() {
        return Err("census is off".into());
    }
    win.set_title(&census_title(&tokens)).map_err(|e| e.to_string())
}

/// the sync window title with the census tokens: control chars dropped, capped
pub fn census_title(tokens: &str) -> String {
    let t: String = tokens.chars().filter(|c| !c.is_control()).take(4096).collect();
    format!("opensidian — Sync {t}")
}

/// verify quiz: `picks` = (1-based position, word) — true when every one matches
#[tauri::command]
pub async fn sync_quiz<R: tauri::Runtime>(win: tauri::WebviewWindow<R>, v: State<'_, Vault>, picks: Vec<(usize, String)>) -> Result<bool, String> {
    gate(win.label(), "sync_quiz")?;
    let (_, st) = open_vault(&v)?;
    let fresh = REVEALED.lock().unwrap_or_else(|e| e.into_inner()).is_some_and(|t| t.elapsed() < QUIZ_WINDOW);
    if !fresh {
        return Err("reveal the words first (the quiz checks them for 10 minutes after a reveal)".into());
    }
    if picks.is_empty() || picks.len() > 12 {
        return Err("1 to 12 picks".into());
    }
    blocking(move || {
        let w = engine(&st, true)?.reveal_words().map_err(api)?;
        let words: Vec<&str> = w.split_whitespace().collect();
        Ok(quiz_ok(&words, &picks))
    })
    .await
}

/// every pick names a position 1..=len and its word (case/space-insensitive)
pub fn quiz_ok(words: &[&str], picks: &[(usize, String)]) -> bool {
    picks.iter().all(|(i, w)| *i >= 1 && words.get(i - 1).is_some_and(|x| x.eq_ignore_ascii_case(w.trim())))
}

/// one run of a version diff ("eq" | "del" | "ins")
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DiffRun {
    op: &'static str,
    text: String,
}

/// char diff a -> b (sync-core's diff-match-patch); a diff over the edit cap
/// falls back to "all of a deleted, all of b inserted"
pub fn diff_runs(a: &str, b: &str) -> Vec<DiffRun> {
    use sync_core::merge::dmp::{diff, Op};
    let (ac, bc): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    match diff(&ac, &bc) {
        Ok(runs) => runs
            .into_iter()
            .map(|r| DiffRun {
                op: match r.op {
                    Op::Equal => "eq",
                    Op::Delete => "del",
                    Op::Insert => "ins",
                },
                text: r.text.into_iter().collect(),
            })
            .collect(),
        Err(_) => [("del", a), ("ins", b)]
            .into_iter()
            .filter(|(_, t)| !t.is_empty())
            .map(|(op, t)| DiffRun { op, text: t.to_string() })
            .collect(),
    }
}

/// diff version `seq` against version `against`, or against the note on disk
#[tauri::command]
pub fn history_diff<R: tauri::Runtime>(
    win: tauri::WebviewWindow<R>,
    v: State<'_, Vault>,
    seq: u64,
    against: Option<u64>,
) -> Result<Vec<DiffRun>, String> {
    gate(win.label(), "history_diff")?;
    let (path, old) = crate::hist_run(&v, |s| -> Result<_, String> {
        let e = s.entry(seq).cloned().ok_or_else(|| format!("no version {seq}"))?;
        Ok((e.path, s.read_version(seq).map_err(|e| e.to_string())?))
    })??;
    let new = match against {
        Some(q) => crate::hist_run(&v, |s| s.read_version(q).map_err(|e| e.to_string()))??,
        None => {
            let name = crate::hist_note_name(&path)?.to_string();
            let p = crate::note_path(&v, &name, false).ok_or("the note is not on disk")?;
            crate::read_capped(&p).ok_or("the note is not readable")?.into_bytes()
        }
    };
    Ok(diff_runs(&String::from_utf8_lossy(&old), &String::from_utf8_lossy(&new)))
}

/// show (build or focus) the sync window. NOT guarded: opening it grants the
/// caller nothing — its page is ours and only the user drives it.
/// `pane` = "sync" | "history"; `note` = the note the history pane starts on
/// (a hint for the page: it lists that note's versions in the sync window,
/// where only the user sees them).
#[tauri::command]
pub fn sync_open(app: tauri::AppHandle, pane: Option<String>, note: Option<String>) -> Result<(), String> {
    use tauri::Manager;
    let url = page_url(pane.as_deref(), note.as_deref())?;
    if let Some(w) = app.get_webview_window(SYNC_LABEL) {
        w.navigate(url).map_err(|e| e.to_string())?;
        return w.set_focus().map_err(|e| e.to_string());
    }
    let b = tauri::WebviewWindowBuilder::new(&app, SYNC_LABEL, tauri::WebviewUrl::CustomProtocol(url))
        .title("opensidian — Sync")
        .inner_size(760.0, 640.0)
        .min_inner_size(420.0, 360.0);
    // census-gate (#30): the page publishes its state only when the census is on (sync_census)
    let b = if crate::census::on() { b.initialization_script(crate::census::INIT_JS) } else { b };
    b.build()
        .map(|_| ())
        .map_err(|e| format!("the sync window could not open: {e}"))
}

/// tauri://localhost/sync.html?pane=..&note=.. (the app origin, main.rs
/// nav_allowed); pane is one of two fixed words, note is query-encoded
pub fn page_url(pane: Option<&str>, note: Option<&str>) -> Result<url::Url, String> {
    let mut u = url::Url::parse(&format!("tauri://localhost/{SYNC_PAGE}")).map_err(|e| e.to_string())?;
    {
        let mut q = u.query_pairs_mut();
        q.append_pair("pane", if pane == Some("history") { "history" } else { "sync" });
        if let Some(n) = note.filter(|n| !n.is_empty() && n.len() <= 4096) {
            q.append_pair("note", n);
        }
    }
    Ok(u)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const MAIN_RS: &str = include_str!("main.rs");
    const SYNCUI_RS: &str = include_str!("syncui.rs");
    const CAPS: &str = include_str!("../capabilities/default.json");

    #[test]
    fn gate_answers_only_the_sync_label() {
        assert!(gate(SYNC_LABEL, "sync_now").is_ok());
        for l in ["main", "rec-main", "", "Sync", "sync ", "sync-2", "main/sync"] {
            let e = gate(l, "history_read").unwrap_err();
            assert!(e.starts_with("EPERM: history_read: "), "{l:?}: {e}");
        }
    }

    /// the body of `fn <name>` (generic or not) in `src`, from its opening brace
    fn body<'a>(src: &'a str, name: &str) -> Option<&'a str> {
        let at = [format!("fn {name}<"), format!("fn {name}(")].iter().find_map(|p| src.find(p.as_str()))?;
        let rest = &src[at..];
        Some(&rest[rest.find('{')? + 1..])
    }

    /// every guarded command is registered, and its FIRST statement is the gate
    /// with its own name; every #[tauri::command] history_*/sync_* is guarded
    /// (sync_open, which only shows the window, is the one exception)
    #[test]
    fn every_sync_and_history_command_calls_the_gate_first() {
        let handler = &MAIN_RS[MAIN_RS.find("tauri::generate_handler![").expect("handler")..];
        let handler = &handler[..handler.find(']').expect("handler end")];
        for name in GUARDED {
            let reg = handler.contains(&format!(" {name},")) || handler.contains(&format!("syncui::{name},"));
            assert!(reg, "{name} not in the invoke handler");
            let b = body(MAIN_RS, name).or_else(|| body(SYNCUI_RS, name)).unwrap_or_else(|| panic!("no fn {name}"));
            let first = b.trim_start().lines().next().unwrap_or("");
            let want = format!("gate(win.label(), \"{name}\")?;");
            assert!(first == want || first == format!("syncui::{want}"), "{name}: first statement is {first:?}");
        }
        for src in [MAIN_RS, SYNCUI_RS] {
            for (i, _) in src.match_indices("#[tauri::command]") {
                let after = &src[i..];
                let f = after.find("fn ").expect("fn after attribute");
                let name: String = after[f + 3..].chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if name.starts_with("history_") || name.starts_with("sync_") {
                    assert!(GUARDED.contains(&name.as_str()) || name == "sync_open", "{name} is not guarded");
                }
            }
        }
    }

    /// page script cannot build a webview (it could pick the trusted label)
    #[test]
    fn no_capability_builds_webviews() {
        assert!(!CAPS.contains("create-webview"), "{CAPS}");
        assert!(!CAPS.contains("allow-create"), "{CAPS}");
        let v: serde_json::Value = serde_json::from_str(CAPS).expect("json");
        assert_eq!(v["windows"], json!(["main"]));
    }

    #[test]
    fn page_url_is_the_app_origin() {
        let u = page_url(Some("history"), Some("a b/&c")).unwrap();
        assert_eq!(u.as_str(), "tauri://localhost/sync.html?pane=history&note=a+b%2F%26c");
        assert_eq!(page_url(Some("x"), None).unwrap().as_str(), "tauri://localhost/sync.html?pane=sync");
        assert_eq!(page_url(None, Some("")).unwrap().as_str(), "tauri://localhost/sync.html?pane=sync");
    }

    /// the census title keeps the fixed prefix, drops control chars, is capped;
    /// the page's census block publishes the word COUNT, never the words
    #[test]
    fn census_title_and_page_probe() {
        assert_eq!(census_title("[spane:sync]\n[x:1]\u{7}"), "opensidian — Sync [spane:sync][x:1]");
        assert_eq!(census_title(&"a".repeat(9000)).chars().count(), "opensidian — Sync ".chars().count() + 4096);
        let js = include_str!("../../ui/sync.js");
        let probe = &js[js.find("census-gate (#30)").expect("census block")..js.find("/* ---- boot ---- */").expect("boot")];
        assert!(probe.contains("if (window.__OS_CENSUS === true)"), "census block not behind the switch");
        assert!(probe.contains("ws.children.length"), "word count");
        for leak in ["ws.textContent", "ws.innerText", "li.textContent", "sync_reveal", "s-secret\").value"] {
            assert!(!probe.contains(leak), "census block reads {leak}");
        }
    }

    #[test]
    fn quiz_and_diff() {
        let w = ["alpha", "bravo", "charlie"];
        assert!(quiz_ok(&w, &[(1, "alpha".into()), (3, " Charlie ".into())]));
        assert!(!quiz_ok(&w, &[(2, "alpha".into())]));
        assert!(!quiz_ok(&w, &[(0, "alpha".into())]));
        assert!(!quiz_ok(&w, &[(4, "x".into())]));
        let d = diff_runs("one two three", "one 2 three");
        let back_a: String = d.iter().filter(|r| r.op != "ins").map(|r| r.text.as_str()).collect();
        let back_b: String = d.iter().filter(|r| r.op != "del").map(|r| r.text.as_str()).collect();
        assert_eq!((back_a.as_str(), back_b.as_str()), ("one two three", "one 2 three"));
        assert!(d.iter().any(|r| r.op == "del" && r.text.contains("two")));
        assert_eq!(diff_runs("", "x"), vec![DiffRun { op: "ins", text: "x".into() }]);
    }

    /// REAL IPC (tauri's mock runtime): every guarded command invoked with valid
    /// arguments from the "main" webview — the realm community plugins run in —
    /// is refused with EPERM; the same invoke from the "sync" webview passes the
    /// gate (and then fails on "no vault open", i.e. it reached the body).
    #[test]
    fn plugin_realm_invoke_is_refused() {
        use std::sync::{Arc, Mutex, OnceLock};
        use tauri::test::{get_ipc_response, mock_builder, mock_context, noop_assets, INVOKE_KEY};
        let app = mock_builder()
            .manage(crate::Vault {
                root: OnceLock::new(),
                index: Arc::new(Mutex::new(crate::Index::default())),
                lock: OnceLock::new(),
                seed: Arc::new(Mutex::new(None)),
            })
            .invoke_handler(tauri::generate_handler![
                crate::history_list,
                crate::history_read,
                crate::history_restore,
                crate::history_deleted,
                crate::history_restore_deleted,
                crate::history_purge,
                history_diff,
                sync_status,
                sync_setup,
                sync_now,
                sync_set_mode,
                sync_reveal,
                sync_quiz,
                sync_census,
                sync_move,
            ])
            .build(mock_context(noop_assets()))
            .expect("mock app");
        let main = tauri::WebviewWindowBuilder::new(&app, "main", Default::default()).build().expect("main");
        let sync = tauri::WebviewWindowBuilder::new(&app, SYNC_LABEL, Default::default()).build().expect("sync");
        let args: &[(&str, serde_json::Value)] = &[
            ("history_list", json!({"name": "Note"})),
            ("history_read", json!({"seq": 1})),
            ("history_restore", json!({"seq": 1})),
            ("history_deleted", json!({})),
            ("history_restore_deleted", json!({"name": "Note"})),
            ("history_purge", json!({"name": "Note"})),
            ("history_diff", json!({"seq": 1})),
            ("sync_status", json!({})),
            ("sync_setup", json!({"url": "http://127.0.0.1:1", "device": "d", "join": "words", "secret": "x"})),
            ("sync_now", json!({})),
            ("sync_set_mode", json!({"mode": "auto-merge"})),
            ("sync_reveal", json!({})),
            ("sync_quiz", json!({"picks": [[1, "w"]]})),
            ("sync_census", json!({"tokens": "[spane:sync]"})),
            ("sync_move", json!({"token": "t"})),
        ];
        assert_eq!(args.len(), GUARDED.len());
        let call = |w: &tauri::WebviewWindow<tauri::test::MockRuntime>, cmd: &str, a: &serde_json::Value| {
            get_ipc_response(
                w,
                tauri::webview::InvokeRequest {
                    cmd: cmd.into(),
                    callback: tauri::ipc::CallbackFn(0),
                    error: tauri::ipc::CallbackFn(1),
                    url: "tauri://localhost".parse().expect("url"),
                    body: tauri::ipc::InvokeBody::Json(a.clone()),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.to_string(),
                },
            )
        };
        for (cmd, a) in args {
            assert!(GUARDED.contains(cmd), "{cmd}");
            let e = call(&main, cmd, a).expect_err(cmd);
            let s = e.as_str().unwrap_or_default().to_string();
            assert!(s.starts_with(&format!("EPERM: {cmd}: ")), "{cmd} from main: {e}");
            let e = call(&sync, cmd, a).expect_err(cmd);
            let s = e.as_str().unwrap_or_default().to_string();
            // sync_census needs no vault: from "sync" it reaches its body and stops at the census switch (off in tests)
            let past_gate = if *cmd == "sync_census" { s.contains("census is off") } else { s.contains("no vault open") };
            assert!(!s.contains("EPERM") && past_gate, "{cmd} from sync: {e}");
        }
    }
}
