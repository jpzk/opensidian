// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* spawn-consent (GHSA-6xfp-789j-9jr3, operator 2026-11-01): two native GTK
   prompts, outside the webview, that page script cannot answer for the user:
     (a) before a vault becomes trusted (plugins_set trusted:true), and
     (b) before EVERY program a plugin starts (desk_proc_spawn), ahead of
         desk_proc::start -> Command::new.
   The dialog text is built HERE from Rust-side data only: the canonical vault
   path; the ABSOLUTE program path resolve() found — the very path that is
   exec'd (PR #41 F1) — the args after map_arg, the cwd after map_cwd, the env
   the plugin lays over ours. Every value is Debug-quoted (a newline or a
   control char cannot fake a line of the dialog) and shown IN FULL in a
   scrolled, read-only, plain-text view of bounded size (F2); the env is capped
   before anything is asked (F6). Buttons: Deny/Cancel is the default (Enter,
   Escape, the window's close button, a destroyed dialog, a lost main loop all
   answer NO); Allow once / Trust must be pressed, for every question. #74
   (operator 2026-11-13, overriding PR #41 N2/N3 for plugin spawns ONLY): a
   plugin's spawn question may carry a third button, "Always allow this program",
   with a visible note that the grant covers ANY arguments (ADDENDUM 2). It is
   offered only when the question carries a `Remember` (spawn_question_always,
   built by desk_proc_spawn from spawn_allow::Want: vault, plugin id + the
   plugin's on-disk fingerprint, program + its sha256). The Trust prompt and the
   open prompt never carry one: no Always button, nothing remembered for them.
   This module holds no state and writes nothing: `Remember.granted` says whether
   a stored grant already answers, `Remember.grant` stores one (spawn_allow.rs,
   cfgstore Op::SpawnAllowAdd) and runs only after the user pressed Always.
   The app log gets the body with env VALUES masked (N1).
   No timeout, and no env var, cfg flag, feature or test hook that answers for the user — tests click the
   dialog (harness phase spawnconsent; scripts/consent-nobypass.sh guards the
   calls).

   Threading: the commands that ask are async (tauri runs them off the main
   thread). ask() parks a blocking-pool thread on a channel; the dialog is
   built on the main thread via run_on_main_thread and answers through its
   "response" signal — no nested gtk_dialog_run, so the main loop keeps
   serving IPC and painting while the question is open. One question at a
   time: a plugin spawning in a loop queues behind the open dialog instead of
   stacking windows. R2 (PR #42, CodeAnt): every queued question parks a
   blocking-pool thread, so the queue is bounded — at most MAX_IN_FLIGHT
   questions (the one on screen + those behind it); one more is refused at once
   with EBUSY, nothing asked, no thread taken (admit()). */
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::desk_proc::Spec;

/// stable window titles: what the harness finds the dialogs by (xdotool --name)
pub const TRUST_TITLE: &str = "opensidian: trust vault?";
pub const SPAWN_TITLE: &str = "opensidian: run program?";
/// #74: the third button of a plugin's spawn question (never on Trust / open)
pub const ALWAYS_LABEL: &str = "Al_ways allow this program";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Trust,
    Spawn,
}

/// F6: the most variables, and bytes (key + value + 2 each), a plugin may lay
/// over our environment. Over either = the spawn is refused, nothing is asked.
pub const ENV_MAX_VARS: usize = 64;
pub const ENV_MAX_BYTES: usize = 16 * 1024;
/// Q4: the most bytes program + args + cwd (each + 1) a spawn or an open may carry
pub const RUN_MAX_BYTES: usize = 64 * 1024;

/// one page-supplied value as the dialog shows it: Debug-quoted (escapes \n,
/// \t, \u{..}, quotes) and NEVER cut (F2) — the user approves what they saw
fn shown(s: &str) -> String {
    format!("{s:?}")
}

/// F6: refuse an env over the cap before any dialog is built.
/// Q4 (PR #42 round 4, CodeAnt): the same for the run itself — program + args + cwd
/// (each + 1) over RUN_MAX_BYTES is E2BIG before anything is resolved, built or asked:
/// the dialog body (and its log copy) is built from these values in full (never cut,
/// F2) before ask() admits a slot, so an unbounded request would be copied, quoted and
/// laid out several times just to be refused with EBUSY or shown in a giant view.
pub fn check_env(sp: &Spec) -> Result<(), String> {
    check_run(sp)?;
    let bytes: usize = sp.env.iter().map(|(k, v)| k.len() + v.len() + 2).sum();
    if sp.env.len() > ENV_MAX_VARS || bytes > ENV_MAX_BYTES {
        return Err(format!(
            "E2BIG: spawn {}: the plugin's environment ({} variables, {} bytes) is over the cap ({} / {}); refused, nothing asked or started",
            sp.file,
            sp.env.len(),
            bytes,
            ENV_MAX_VARS,
            ENV_MAX_BYTES
        ));
    }
    Ok(())
}

/// Q4: program + args + cwd (each + 1) over RUN_MAX_BYTES = E2BIG.
/// V2 (PR #42 round 5, Qodo + CodeAnt): check_env runs it on the request as the plugin
/// sent it (cheap, before any lookup); resolve runs it AGAIN on the Spec it returns — the
/// one the dialog shows and exec runs — since resolving a bare name to its absolute path
/// (and argv0 keeping the name) makes the run longer than what was checked first
fn check_run(sp: &Spec) -> Result<(), String> {
    let run: usize = sp.file.len()
        + 1
        + sp.argv0.as_ref().map_or(0, |a| a.len() + 1)
        + sp.args.iter().map(|a| a.len() + 1).sum::<usize>()
        + sp.cwd.as_os_str().len()
        + 1;
    if run > RUN_MAX_BYTES {
        return Err(format!(
            "E2BIG: spawn: the program, its {} args and the cwd ({} bytes) are over the cap ({} bytes); refused, nothing asked or started",
            sp.args.len(),
            run,
            RUN_MAX_BYTES
        ));
    }
    Ok(())
}

/// F1: where the program is, found ONCE. A name with a '/' is taken relative
/// to the CHILD's cwd (what exec would do). A bare name is looked up on PATH —
/// the child's PATH if the plugin sets one, else ours — on ABSOLUTE entries
/// only: an empty entry, "." or any relative entry would be resolved by exec
/// against the child's cwd (inside the vault, plugin-writable), so it is
/// skipped. The result is absolute and is what desk_proc execs (resolve), so
/// the dialog and the exec cannot disagree. None = not found.
/* X1 (PR #42 round 7, Strix): like a PATH entry (V1), the PROGRAM path is resolved PER
   PROCESS when it runs through a magic link: "<dir>/link/helper" with link ->
   /proc/self/cwd (or /proc/thread-self/..., /proc/<pid>/{cwd,root,fd/N}, /dev/fd/N) names
   <app cwd>/helper here, but the child chdirs to the cwd first and then execs the same
   string — <child cwd = vault>/helper. So the program found here is CANONICALIZED here (no
   symlink, no "..", no magic link left: the child execs exactly the file we checked and
   the dialog names), and a canonical form still under /proc or /dev/fd is refused. A
   candidate that does not canonicalize is not a candidate. The cwd is canonicalized the
   same way by resolve() before the program is looked up against it. */
pub fn resolve_program(sp: &Spec) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let exe = |p: &Path| std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false);
    let canon = |p: PathBuf| p.canonicalize().ok().filter(|c| c.is_absolute() && exe(c) && !magic(c));
    if sp.file.is_empty() {
        return None;
    }
    if sp.file.contains('/') {
        let p = Path::new(&sp.file);
        let p = if p.is_absolute() { p.to_path_buf() } else { sp.cwd.join(p) };
        return (p.is_absolute() && exe(&p)).then_some(p).and_then(canon);
    }
    let path = sp.env.iter().rev().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).or_else(|| std::env::var("PATH").ok())?;
    path.split(':').map(Path::new).filter(|d| d.is_absolute()).map(|d| d.join(&sp.file)).filter(|p| exe(p)).find_map(canon)
}

/// X1: a canonical path that still names a per-process object (it could mean another
/// file in the child): anything under /proc or /dev/fd
fn magic(c: &Path) -> bool {
    c.starts_with("/proc") || c.starts_with("/dev/fd")
}

/// X1: the cwd the child runs in, canonical (what the dialog shows); a cwd that does not
/// canonicalize, is not a directory, or is still under /proc or /dev/fd = ENOENT
fn resolve_cwd(cwd: &Path) -> Result<PathBuf, String> {
    cwd.canonicalize()
        .ok()
        .filter(|c| c.is_absolute() && c.is_dir() && !magic(c))
        .ok_or_else(|| format!("ENOENT: spawn: cwd {} is not a directory (resolved without symlinks, outside /proc and /dev/fd)", shown(&cwd.to_string_lossy())))
}

/// F1: the Spec that will run — file = the absolute path resolve_program found
/// (the one the dialog names), argv0 = the name the plugin asked for. Not found
/// = ENOENT, before anything is asked. X1: cwd = its canonical form, file canonical.
pub fn resolve(sp: Spec) -> Result<Spec, String> {
    #[cfg(test)]
    crate::desk_proc::fs_work(); // BB2 probe
    let sp = Spec { cwd: resolve_cwd(&sp.cwd)?, ..sp };
    let p = resolve_program(&sp)
        .ok_or_else(|| format!("ENOENT: spawn {} not found (bare names are looked up on absolute PATH entries only)", sp.file))?;
    let file = p.into_os_string().into_string().map_err(|_| format!("ENOENT: spawn {}: resolved path is not UTF-8", sp.file))?;
    let argv0 = if file != sp.file { Some(sp.file.clone()) } else { sp.argv0.clone() };
    let sp = Spec { file, argv0, ..sp };
    check_run(&sp)?; // V2: the cap holds for what is shown and run, not only what was sent
    // Y1 (PR #42 round 8): the program is held open from HERE — before the question is built —
    // to the exec, which runs this open file (desk_proc::pin, desk_proc::build), never the name again
    let prog = Some(crate::desk_proc::pin(&sp.file)?);
    // AA2 (PR #42 round 10): the cwd too — held open from here, entered by fchdir on that fd
    let dir = Some(crate::desk_proc::pin_dir(&sp.cwd)?);
    Ok(Spec { prog, dir, ..sp })
}

/// the trust prompt's body. `canon` = plugins::trust_key (canonical vault path)
pub fn trust_text(canon: &str) -> String {
    format!(
        "Turn on community plugins for this vault?\n\n\
         Vault: {}\n\n\
         Plugin code in this vault will run with your user's rights. Every \
         program a plugin starts is still asked about, one by one.",
        shown(canon)
    )
}

/// the spawn prompt's body, from the resolved Spec desk_proc::start will run
pub fn spawn_text(sp: &Spec) -> String {
    spawn_body(sp, EnvValues::Shown)
}

/// N1: what the app log gets for that prompt: the same body, every env value
/// replaced by a marker (names are kept: they say what was set, not what it holds)
pub fn spawn_log(sp: &Spec) -> String {
    spawn_body(sp, EnvValues::Hidden)
}

const ENV_VALUE_HIDDEN: &str = "(value not logged)";

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnvValues {
    Shown,
    Hidden,
}

fn spawn_body(sp: &Spec, env: EnvValues) -> String {
    let mut t = format!("A plugin wants to run a program.\n\nProgram: {}\n", shown(&sp.file));
    if let Some(a0) = &sp.argv0 {
        t.push_str(&format!("    (asked for {}; the file above is exactly what runs)\n", shown(a0)));
    }
    if sp.args.is_empty() {
        t.push_str("Args: (none)\n");
    } else {
        t.push_str(&format!("Args ({}):\n", sp.args.len()));
        for a in &sp.args {
            t.push_str(&format!("    {}\n", shown(a)));
        }
    }
    t.push_str(&format!("Cwd: {}\n", shown(&sp.cwd.to_string_lossy())));
    if !sp.env.is_empty() {
        t.push_str(&format!("Env set by the plugin ({}):\n", sp.env.len()));
        for (k, v) in &sp.env {
            let v = if env == EnvValues::Shown { shown(v) } else { ENV_VALUE_HIDDEN.to_string() };
            t.push_str(&format!("    {}={}\n", shown(k), v));
        }
    }
    t.push_str("\nIt will run with your user's rights.");
    t
}

/* No session-wide answer (PR #41 round 2, N2/N3, coordinator default
   2026-11-01 19:20Z): every run a plugin asks for is asked, one by one, also
   when the very same run was allowed a moment ago — UNLESS the user pressed
   "Always allow this program" for it (#74, operator 2026-11-13). That grant is
   the only thing remembered, and only for a plugin spawn: key = (vault, plugin,
   absolute program), valid only while the plugin's main.js + manifest.json and
   the program's bytes still hash to what they were when the user pressed it
   (spawn_allow.rs). A program in the vault the plugin rewrites, or a new plugin
   version, asks again. What a grant does NOT cover: arguments, cwd, env, and
   what an interpreter (sh, python, node, git hooks) reads — the dialog says so
   (ALWAYS_NOTE). Trust and open questions are asked every time, as before. */

/// ADDENDUM 2: shown directly above the Always button (and in Settings, and in the docs)
pub const ALWAYS_NOTE: &str = "Always allow lets this plugin run this program again with ANY arguments, without asking. \
Arguments can be malicious (for example options that make the program run other commands, \
or a script an interpreter reads), and they are not checked again. Only the program file and \
the plugin's main.js / manifest.json are checked: if either changes, you are asked again.";

/// #74: what a spawn question needs to offer "Always allow this program". Built by
/// desk_proc_spawn from spawn_allow::Want; consent.rs only calls it. `granted` = a stored
/// grant matches this exact Want (read now); `grant` = store one (cfgstore
/// Op::SpawnAllowAdd) — run only after the user pressed Always, under ONE_AT_A_TIME.
pub struct Remember {
    pub granted: Box<dyn Fn() -> bool + Send>,
    pub grant: Box<dyn FnOnce() + Send>,
}

/* R5 (PR #42, Qodo): an open (electron shell openPath / showItemInFolder /
   openExternal of a local or custom-scheme target, app.openWithDefaultApp) is not
   "a plugin wants to run a program": the app's own "open in default app" goes the
   same way, and a page script cannot be told apart from the app. So its question
   says nothing about who asks, names the target, and says what opening it may do:
   a program, script or .desktop file is RUN by the default opener. The opener
   (the resolved xdg-open that runs) and its cwd stay in the body in full. */

/// the open prompt's body. `sp` = the resolved opener Spec (file = absolute
/// xdg-open, args = [target]), `target` = what the opener gets (the final path)
pub fn open_text(sp: &Spec, target: &str) -> String {
    format!(
        "Open {t} with the default application?\n\n\
         If it is a program, a script or a .desktop file, opening it can run it, \
         with your user's rights.\n\n\
         Target: {t}\n\
         Program: {p}\n\
         Cwd: {c}\n",
        t = shown(target),
        p = shown(&sp.file),
        c = shown(&sp.cwd.to_string_lossy())
    )
}

/// what ask() puts on screen (`text`) and what the app log gets (`log`): the
/// same body, except that the log never holds an env VALUE (N1: a plugin's env
/// can carry tokens; names are logged, values are not). `head` is the dialog's
/// bold first line (ours, never page data).
pub struct Question {
    head: String,
    text: String,
    log: String,
    /// #74: Some only from spawn_question_always; ask_with takes it out before presenting
    remember: Option<Remember>,
    /// #74: the dialog shows the Always button (+ ALWAYS_NOTE); set by ask_with only
    always: bool,
}

const TRUST_HEAD: &str = "Turn on community plugins for this vault?";
const SPAWN_HEAD: &str = "A plugin wants to run a program. Read all of it (scroll) before you answer.";
const OPEN_HEAD: &str = "Open this with the default application? It may run it. Read all of it (scroll) before you answer.";

pub fn trust_question(canon: &str) -> Question {
    let text = trust_text(canon);
    Question { head: TRUST_HEAD.into(), log: text.clone(), text, remember: None, always: false }
}

/// `sp` = the Spec after resolve() (absolute file). Allow once / Deny only.
pub fn spawn_question(sp: &Spec) -> Question {
    Question { head: SPAWN_HEAD.into(), text: spawn_text(sp), log: spawn_log(sp), remember: None, always: false }
}

/// #74: a plugin's spawn question that ALSO offers "Always allow this program". `plugin` /
/// `version` = the plugin id and its manifest.json version as spawn_allow::plugin_fingerprint
/// read them from disk. The body names the plugin and carries ALWAYS_NOTE (ADDENDUM 2).
pub fn spawn_question_always(sp: &Spec, plugin: &str, version: &str, remember: Remember) -> Question {
    let tail = format!(
        "\n\nPlugin: {} (version {})\n\n{}",
        shown(plugin),
        shown(version),
        ALWAYS_NOTE
    );
    Question {
        head: SPAWN_HEAD.into(),
        text: spawn_text(sp) + &tail,
        log: spawn_log(sp) + &tail,
        remember: Some(remember),
        always: false,
    }
}

/// R5: an open through the default opener. `sp` = the resolved opener Spec (no env)
pub fn open_question(sp: &Spec, target: &str) -> Question {
    let text = open_text(sp, target);
    Question { head: OPEN_HEAD.into(), log: text.clone(), text, remember: None, always: false }
}

/// the dialog's answer. Only answer_of() makes Once / Always — from a GTK response
/// the user produced by pressing Allow once / Trust / Always allow this program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    No,
    Once,
    Always,
}

/// Accept = Allow once / Trust; Apply = Always allow this program (a button only a
/// question with a Remember has); everything else (Reject, DeleteEvent, None, ...) = No
#[cfg(target_os = "linux")]
fn answer_of(r: gtk::ResponseType) -> Answer {
    match r {
        gtk::ResponseType::Accept => Answer::Once,
        gtk::ResponseType::Apply => Answer::Always,
        _ => Answer::No,
    }
}

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// one question, one at a time: `present` puts it in front of the user and hands
/// the answer to the sender (false = it could not be presented = NO). Every call
/// presents, unless the question carries a Remember whose stored grant answers it (#74):
/// checked before the lock (no wait behind an open dialog) and AGAIN under it, so the
/// identical questions queued behind a dialog answered "Always" are released, not asked.
/// Always stores the grant UNDER the lock (before the next queued one looks) and only for
/// a question that carries a Remember; an Always without one is NO.
fn ask_with(kind: Kind, mut q: Question, present: impl FnOnce(Kind, Question, std::sync::mpsc::Sender<Answer>) -> bool) -> bool {
    let remember = q.remember.take().filter(|_| kind == Kind::Spawn);
    if remember.as_ref().is_some_and(|r| (r.granted)()) {
        return true;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    if remember.as_ref().is_some_and(|r| (r.granted)()) {
        return true; // queued behind the dialog whose Always stored this grant
    }
    q.always = remember.is_some();
    let (tx, rx) = std::sync::mpsc::channel::<Answer>();
    if !present(kind, q, tx) {
        return false;
    }
    // a dialog torn down without a response drops tx: recv errs -> No
    match rx.recv().unwrap_or(Answer::No) {
        Answer::Once => true,
        Answer::Always => match remember {
            Some(r) => {
                (r.grant)();
                true
            }
            None => false,
        },
        Answer::No => false,
    }
}

/// ask the user; true only when Allow once / Trust was pressed for THIS
/// question. BLOCKS until the answer: call it from a worker thread, never the
/// main thread.
fn ask_blocking(win: &tauri::Window, kind: Kind, q: Question) -> bool {
    ask_with(kind, q, |kind, q, tx| {
        let w2 = win.clone();
        win.run_on_main_thread(move || show_dialog(&w2, kind, &q, tx)).is_ok()
    })
}

/// R2: the most questions in flight at once — the one on screen and the ones
/// queued behind it (each holds a blocking-pool thread while it waits)
pub const MAX_IN_FLIGHT: usize = 5;

static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

/// R2: a place in the bounded queue; dropping it frees the place. BB2 (PR #42 round 11): the
/// commands take it FIRST (admit), before any lookup or pin, and hand it to ask, which demands it
pub struct Slot(());

impl Drop for Slot {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// R2: a place in the queue, or EBUSY at once (nothing asked, no thread taken)
/// when MAX_IN_FLIGHT questions are already open or queued. BB2: called before resolve / pin /
/// shell_target, so a refused request has done no fs work and opened no fd
pub fn admit() -> Result<Slot, String> {
    IN_FLIGHT
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < MAX_IN_FLIGHT).then_some(n + 1))
        .map(|_| Slot(()))
        .map_err(|n| format!("EBUSY: {n} consent questions are already open or waiting (at most {MAX_IN_FLIGHT}); refused, nothing asked"))
        .inspect_err(|e| eprintln!("[consent] {e}"))
}

/// the async front door the commands await: Ok(true) only for Allow once /
/// Trust; Ok(false) for every NO (any failure on the way included). BB2: `slot` = the place
/// admit() gave the request before its resolve / pin / question building (R2's EBUSY is now
/// admit's, at the top of the command: ask no longer admits itself)
pub async fn ask(slot: Slot, win: tauri::Window, kind: Kind, q: Question) -> Result<bool, String> {
    let what = match kind {
        Kind::Trust => "trust",
        Kind::Spawn => "spawn",
    };
    eprintln!("[consent] {what}: asking");
    let yes = tauri::async_runtime::spawn_blocking(move || {
        let _slot = slot;
        ask_blocking(&win, kind, q)
    })
    .await
    .unwrap_or(false);
    eprintln!("[consent] {what}: {}", if yes { "allowed" } else { "denied" });
    Ok(yes)
}

/// the scrolled body's fixed size (F2/F6): the dialog never outgrows the screen
#[cfg(target_os = "linux")]
const BODY_W: i32 = 640;
#[cfg(target_os = "linux")]
const BODY_H: i32 = 320;

#[cfg(target_os = "linux")]
fn show_dialog(win: &tauri::Window, kind: Kind, q: &Question, tx: std::sync::mpsc::Sender<Answer>) {
    use gtk::prelude::*;
    let parent = win.gtk_window().ok();
    let (title, no, yes) = match kind {
        Kind::Trust => (TRUST_TITLE, "_Cancel", "_Trust"),
        Kind::Spawn => (SPAWN_TITLE, "_Deny", "Allow _once"),
    };
    let head = q.head.as_str();
    // the head is ours; the body (page values) goes into a read-only TextView
    // as plain text, never markup: a page value cannot style or hide part of
    // the prompt, and nothing is cut — the view scrolls (F2)
    let d = gtk::MessageDialog::new(
        parent.as_ref(),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Warning,
        gtk::ButtonsType::None,
        head,
    );
    d.set_title(title);
    let buf = gtk::TextBuffer::new(None::<&gtk::TextTagTable>);
    buf.set_text(&q.text);
    let tv = gtk::TextView::with_buffer(&buf);
    tv.set_editable(false);
    tv.set_cursor_visible(false);
    tv.set_wrap_mode(gtk::WrapMode::Char);
    tv.set_left_margin(6);
    tv.set_right_margin(6);
    let sw = gtk::ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    sw.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sw.set_shadow_type(gtk::ShadowType::In);
    sw.set_size_request(BODY_W, BODY_H);
    sw.add(&tv);
    d.content_area().add(&sw);
    // #74 ADDENDUM 2: the any-arguments note, plain text, directly above the buttons, only
    // where the Always button is offered (q.always: a question with a Remember, spawn only)
    if q.always {
        let note = gtk::Label::new(Some(ALWAYS_NOTE));
        note.set_line_wrap(true);
        note.set_max_width_chars(80);
        note.set_xalign(0.0);
        note.set_selectable(false);
        d.content_area().add(&note);
    }
    // N1: the log gets the body with every env VALUE masked (names, program, args and
    // cwd stay: the harness asserts on them). The view itself holds q.text, in full.
    eprintln!("[consent] dialog text: {:?}", q.log);
    let _ = d.add_button(no, gtk::ResponseType::Reject);
    let _ = d.add_button(yes, gtk::ResponseType::Accept);
    if q.always {
        let _ = d.add_button(ALWAYS_LABEL, gtk::ResponseType::Apply);
    }
    d.set_default_response(gtk::ResponseType::Reject);
    d.set_keep_above(true);
    let tx = std::cell::RefCell::new(Some(tx));
    d.connect_response(move |d, r| {
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(answer_of(r));
        }
        // SAFETY: the dialog is ours alone (no other Rust handle outlives this
        // handler); destroying a toplevel from its own response handler is the
        // documented GtkDialog pattern.
        unsafe { d.destroy() };
    });
    d.show_all();
    // Enter answers Deny/Cancel even though the text view can take focus
    if let Some(b) = d.widget_for_response(gtk::ResponseType::Reject) {
        b.grab_focus();
    }
    d.present();
}

/// no native prompt on this platform = NO (nothing is trusted or started)
#[cfg(not(target_os = "linux"))]
fn show_dialog(_win: &tauri::Window, _kind: Kind, _q: &Question, tx: std::sync::mpsc::Sender<Answer>) {
    let _ = tx.send(Answer::No);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(file: &str, args: &[&str], cwd: &str) -> Spec {
        Spec { file: file.into(), args: args.iter().map(|s| s.to_string()).collect(), cwd: cwd.into(), root: cwd.into(), env: vec![], argv0: None, path: None, prog: None, dir: None }
    }

    /// base/vault (the plugin's cwd, with its own `fakeprog`), base/bin (the real
    /// `fakeprog`), base/link -> vault. Each script prints where it lives.
    fn tree(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("consent-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for (dir, say) in [("vault", "VAULT"), ("bin", "BIN"), ("vault/rel", "REL")] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
            let f = base.join(dir).join("fakeprog");
            crate::testutil::write_exe(&f, format!("#!/bin/sh\necho {say}\n")).unwrap();
        }
        std::os::unix::fs::symlink(base.join("vault"), base.join("link")).unwrap();
        base
    }

    fn with_path(file: &str, cwd: &Path, path: &str) -> Spec {
        let mut s = sp(file, &[], &cwd.to_string_lossy());
        s.env.push(("PATH".into(), path.into()));
        s
    }

    #[test]
    fn spawn_text_names_program_args_and_cwd() {
        let mut s = sp("/usr/bin/touch", &["/v/marker"], "/v");
        s.argv0 = Some("touch".into());
        let t = spawn_text(&s);
        assert!(t.contains("Program: \"/usr/bin/touch\"\n    (asked for \"touch\""), "{t}");
        assert!(t.contains("    \"/v/marker\"\n"), "{t}");
        assert!(t.contains("Cwd: \"/v\""), "{t}");
    }

    #[test]
    fn a_newline_in_an_arg_cannot_fake_a_dialog_line() {
        let t = spawn_text(&sp("/bin/sh", &["-c", "rm -rf ~\nCwd: /safe"], "/v"));
        assert_eq!(t.matches("\nCwd: ").count(), 1, "{t}");
        assert!(t.contains("\\nCwd: /safe"), "{t}");
    }

    // F2: nothing is cut — a 5000-char value and the 40th arg are in the text whole
    #[test]
    fn f2_long_values_and_many_args_are_shown_whole() {
        let long = format!("{}END", "x".repeat(5000));
        let mut many: Vec<String> = (0..40).map(|i| format!("arg{i}")).collect();
        many.push(long.clone());
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let t = spawn_text(&sp("/bin/echo", &refs, "/v"));
        assert!(t.contains(&format!("    \"{long}\"\n")), "a long arg was cut");
        assert!(t.contains("    \"arg39\"\n"), "{t}");
        assert!(t.contains("Args (41):"), "{t}");
        assert!(!t.contains("not shown"), "{t}");
    }

    #[test]
    fn env_the_plugin_sets_is_shown() {
        let mut s = sp("/usr/bin/git", &[], "/v");
        s.env.push(("LD_PRELOAD".into(), "/tmp/x.so".into()));
        assert!(spawn_text(&s).contains("    \"LD_PRELOAD\"=\"/tmp/x.so\"\n"));
    }

    // F1: an empty PATH entry, "." or a relative entry would be resolved by exec against
    // the child's cwd (the vault): skipped, so vault/fakeprog is never what resolves
    #[test]
    fn f1_relative_path_entries_never_resolve_into_the_cwd() {
        let b = tree("f1");
        let (v, bin) = (b.join("vault"), b.join("bin"));
        let real = bin.join("fakeprog").canonicalize().unwrap(); // X1: the result is canonical
        for path in [format!(":{}", bin.display()), format!(".:{}", bin.display()), format!("rel:{}", bin.display()), format!("{}::", bin.display())] {
            assert_eq!(resolve_program(&with_path("fakeprog", &v, &path)), Some(real.clone()), "PATH={path:?}");
        }
        for path in ["", ":", ".", "rel", "./", "::."] {
            assert_eq!(resolve_program(&with_path("fakeprog", &v, path)), None, "PATH={path:?}");
            assert!(resolve(with_path("fakeprog", &v, path)).err().expect("fakeprog resolved").starts_with("ENOENT: spawn fakeprog"), "PATH={path:?}");
        }
        // a name with a '/' is the child's cwd + name, made absolute
        assert_eq!(resolve_program(&with_path("./fakeprog", &v, "")).map(|p| p.canonicalize().unwrap()), Some(v.join("fakeprog").canonicalize().unwrap()));
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn trust_text_shows_the_canonical_path() {
        assert!(trust_text("/home/u/My Vault").contains("Vault: \"/home/u/My Vault\""));
    }

    // F6: an env over the cap is refused before anything is asked
    #[test]
    fn f6_env_over_the_cap_is_refused() {
        let mut s = sp("/bin/true", &[], "/");
        s.env = (0..ENV_MAX_VARS).map(|i| (format!("K{i}"), "v".into())).collect();
        assert!(check_env(&s).is_ok());
        s.env.push(("ONE_MORE".into(), "v".into()));
        assert!(check_env(&s).unwrap_err().starts_with("E2BIG: spawn /bin/true"), "count");
        s.env = vec![("BIG".into(), "y".repeat(ENV_MAX_BYTES))];
        assert!(check_env(&s).unwrap_err().starts_with("E2BIG"), "bytes");
        s.env = vec![("OK".into(), "y".repeat(ENV_MAX_BYTES - 10))];
        assert!(check_env(&s).is_ok());
    }

    /* Q4 (PR #42 round 4, CodeAnt): program + args + cwd over RUN_MAX_BYTES is refused by
       check_env — which desk_proc_spawn and desk_shell call before resolve and before the
       question body is built or ask() is reached (consent-nobypass S6: check_env < ask) —
       many small args or one huge one alike, an open's target too. At the cap it passes:
       nothing a dialog shows is ever cut, the request is either shown in full or refused. */
    #[test]
    fn q4_a_run_over_the_size_cap_is_refused_before_anything_is_asked() {
        let fixed = "/bin/true".len() + 1 + "/".len() + 1;
        let mut s = sp("/bin/true", &[], "/");
        s.args = vec!["a".repeat(RUN_MAX_BYTES - fixed - 1)];
        assert!(check_env(&s).is_ok(), "a run exactly at the cap was refused");
        s.args[0].push('a');
        let e = check_env(&s).unwrap_err();
        assert!(e.starts_with("E2BIG: spawn:") && e.contains("nothing asked"), "one arg over the cap: {e}");
        s.args = vec!["x".into(); RUN_MAX_BYTES / 2];
        assert!(check_env(&s).unwrap_err().starts_with("E2BIG"), "many small args over the cap");
        // an open: the opener Spec desk_shell builds, the target as its one arg
        let mut o = sp("xdg-open", &[], "/");
        o.args = vec![format!("steam://{}", "r".repeat(RUN_MAX_BYTES))];
        assert!(check_env(&o).unwrap_err().starts_with("E2BIG"), "an open over the cap");
    }

    /* V2 (PR #42 round 5, Qodo + CodeAnt): check_env measures the request as sent, with
       the bare program name; resolve then replaces it with its absolute path (and keeps the
       name as argv0), so a request just under the cap became a run over it — shown and
       exec'd. resolve now checks the cap on the Spec it returns: E2BIG, before the
       question is built or asked (desk_proc_spawn / desk_shell: resolve < ask). */
    #[test]
    fn v2_a_run_pushed_over_the_size_cap_by_resolve_is_refused_before_anything_is_asked() {
        let d = std::env::temp_dir().join(format!("consent-v2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let prog = d.join("v2prog");
        crate::testutil::write_exe(&prog, "#!/bin/sh\nexit 0\n").unwrap();
        let mut s = sp("v2prog", &[], "/");
        s.env = vec![("PATH".into(), d.display().to_string())];
        // control: a short run resolves to the absolute path
        let mut c = sp("v2prog", &["a"], "/");
        c.env = s.env.clone();
        let r = resolve(c).unwrap();
        assert_eq!(Path::new(&r.file).canonicalize().unwrap(), prog.canonicalize().unwrap());
        // exactly at the cap as sent: check_env lets it through ...
        let fixed = "v2prog".len() + 1 + "/".len() + 1;
        s.args = vec!["a".repeat(RUN_MAX_BYTES - fixed - 1)];
        assert!(check_env(&s).is_ok(), "the request as sent is at the cap");
        // ... and the resolved run (absolute path + argv0) is over it: refused
        let e = resolve(s).map(|r| r.file).unwrap_err();
        assert!(e.starts_with("E2BIG: spawn:") && e.contains("nothing asked"), "resolve let a run over the cap through: {e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    // N1: the app log holds the env NAMES the plugin set, never their values; the
    // dialog itself shows them in full
    #[test]
    fn n1_env_values_are_not_logged() {
        let secret = "s3cr3t-token-n1-do-not-log";
        let mut s = sp("/usr/bin/git", &["push", "origin"], "/v");
        s.env.push(("GITHUB_TOKEN".into(), secret.into()));
        s.env.push(("LD_PRELOAD".into(), "/tmp/x.so".into()));
        let q = spawn_question(&s);
        assert!(q.text.contains(&format!("    \"GITHUB_TOKEN\"=\"{secret}\"\n")), "{}", q.text);
        assert!(!q.log.contains(secret), "an env value reached the log: {}", q.log);
        assert!(!q.log.contains("/tmp/x.so"), "an env value reached the log: {}", q.log);
        assert!(q.log.contains(&format!("    \"GITHUB_TOKEN\"={ENV_VALUE_HIDDEN}\n")), "{}", q.log);
        assert!(q.log.contains("    \"LD_PRELOAD\"="), "{}", q.log);
        // program, args and cwd are still logged (the harness reads them)
        for x in ["Program: \"/usr/bin/git\"", "    \"push\"\n", "    \"origin\"\n", "Cwd: \"/v\""] {
            assert!(q.log.contains(x), "{x} not logged: {}", q.log);
        }
        assert_eq!(trust_question("/v").log, trust_question("/v").text, "the trust body holds no env");
    }

    /* N2/N3 + #74: no session grant. A plain spawn question (no Remember: what
       desk_proc_spawn builds when the plugin cannot be fingerprinted) is presented every
       time, even after Allow once, and has no Always button; nothing mentions a session. */
    #[test]
    fn n2_74_allow_once_asks_again_every_time() {
        let s = sp("/bin/true", &["x"], "/");
        let mut presented = 0;
        for _ in 0..3 {
            let q = spawn_question(&s);
            assert!(!q.text.to_lowercase().contains("session"), "{}", q.text);
            let yes = ask_with(Kind::Spawn, q, |_, q, tx| {
                presented += 1;
                assert!(!q.always, "a plain spawn question offered Always");
                tx.send(Answer::Once).is_ok()
            });
            assert!(yes);
        }
        assert_eq!(presented, 3, "an identical run was allowed without being asked");
        // a question that cannot be presented, or is torn down unanswered, is NO
        assert!(!ask_with(Kind::Spawn, spawn_question(&s), |_, _, _| false));
        assert!(!ask_with(Kind::Spawn, spawn_question(&s), |_, _, tx| {
            drop(tx);
            true
        }));
        assert!(!ask_with(Kind::Spawn, spawn_question(&s), |_, _, tx| tx.send(Answer::No).is_ok()));
        // Once on an Always-capable question stores nothing: asked again
        let (cfg, w) = (store(), want74("/bin/true"));
        for _ in 0..2 {
            let mut shown = false;
            assert!(ask_with(Kind::Spawn, q74(&s, &cfg, &w), |_, q, tx| {
                shown = true;
                assert!(q.always, "a Remember question did not offer Always");
                tx.send(Answer::Once).is_ok()
            }));
            assert!(shown, "Allow once was remembered");
        }
        assert!(crate::spawn_allow::grants(&cfg.lock().unwrap()).is_empty(), "Allow once stored a grant");
    }

    type Store = std::sync::Arc<Mutex<serde_json::Value>>;
    fn store() -> Store {
        std::sync::Arc::new(Mutex::new(serde_json::json!({})))
    }
    fn want74(program: &str) -> crate::spawn_allow::Want {
        crate::spawn_allow::Want { vault: "/v".into(), plugin: "obsidian-git".into(), plugin_sha256: "p1".into(), plugin_version: "2.0".into(), program: program.into(), program_sha256: "x1".into() }
    }
    /// what desk_proc_spawn builds, over an in-memory config instead of ~/.opensidian.json
    fn remember74(cfg: &Store, w: &crate::spawn_allow::Want) -> Remember {
        let (c1, c2, w1, w2) = (cfg.clone(), cfg.clone(), w.clone(), w.clone());
        Remember {
            granted: Box::new(move || crate::spawn_allow::granted(&c1.lock().unwrap(), &w1)),
            grant: Box::new(move || crate::spawn_allow::apply_add(&mut c2.lock().unwrap(), &w2.grant(1))),
        }
    }
    fn q74(s: &Spec, cfg: &Store, w: &crate::spawn_allow::Want) -> Question {
        spawn_question_always(s, &w.plugin, &w.plugin_version, remember74(cfg, w))
    }

    /* #74: Always on a question with a Remember stores the grant; the next identical
       question is answered from it WITHOUT being presented. A changed plugin or program
       fingerprint, or another vault / plugin / program, is presented again. */
    #[test]
    fn a74_always_asks_once_then_the_grant_answers() {
        let s = sp("/usr/bin/git", &["status"], "/v");
        let (cfg, w) = (store(), want74("/usr/bin/git"));
        let mut shown = 0;
        assert!(ask_with(Kind::Spawn, q74(&s, &cfg, &w), |_, q, tx| {
            shown += 1;
            assert!(q.always);
            tx.send(Answer::Always).is_ok()
        }));
        assert_eq!(crate::spawn_allow::grants(&cfg.lock().unwrap()).len(), 1, "Always stored no grant");
        for _ in 0..3 {
            assert!(ask_with(Kind::Spawn, q74(&s, &cfg, &w), |_, _, tx| {
                shown += 1;
                tx.send(Answer::No).is_ok()
            }));
        }
        assert_eq!(shown, 1, "a granted run was presented again");
        use crate::spawn_allow::Want;
        for other in [
            Want { plugin_sha256: "p2".into(), ..w.clone() }, // main.js / manifest.json changed
            Want { program_sha256: "x2".into(), ..w.clone() }, // program file changed
            Want { vault: "/other".into(), ..w.clone() },
            Want { plugin: "other-plugin".into(), ..w.clone() },
            want74("/usr/bin/ssh"),
        ] {
            let mut asked = false;
            assert!(!ask_with(Kind::Spawn, q74(&s, &cfg, &other), |_, _, tx| {
                asked = true;
                tx.send(Answer::No).is_ok()
            }));
            assert!(asked, "{other:?} rode the grant for {w:?}");
        }
        // after revoke it asks again
        crate::spawn_allow::apply_revoke(&mut cfg.lock().unwrap(), "/v", "obsidian-git", "/usr/bin/git");
        let mut asked = false;
        assert!(!ask_with(Kind::Spawn, q74(&s, &cfg, &w), |_, _, tx| {
            asked = true;
            tx.send(Answer::No).is_ok()
        }));
        assert!(asked, "a revoked grant still answered");
    }

    /* #74: an Always answer for a question with no Remember (trust, open, plain spawn) is
       NO and stores nothing; a Remember on a Trust question is ignored (never offered). */
    #[test]
    fn a74_always_without_a_remember_is_no() {
        let s = sp("/bin/true", &[], "/");
        for q in [spawn_question(&s), trust_question("/v"), open_question(&s, "/v/x")] {
            assert!(!ask_with(Kind::Spawn, q, |_, q, tx| {
                assert!(!q.always);
                tx.send(Answer::Always).is_ok()
            }));
        }
        let (cfg, w) = (store(), want74("/bin/true"));
        assert!(!ask_with(Kind::Trust, q74(&s, &cfg, &w), |_, q, tx| {
            assert!(!q.always, "a Trust question offered Always");
            tx.send(Answer::Always).is_ok()
        }));
        assert!(crate::spawn_allow::grants(&cfg.lock().unwrap()).is_empty(), "a Trust answer stored a spawn grant");
    }

    /* #74: ALWAYS_NOTE and the plugin line are in the Always question (dialog text AND log),
       never in the trust, open or plain spawn question. */
    #[test]
    fn a74_the_always_note_is_only_on_the_always_question() {
        let s = sp("/usr/bin/git", &[], "/v");
        let (cfg, w) = (store(), want74("/usr/bin/git"));
        let q = q74(&s, &cfg, &w);
        for t in [&q.text, &q.log] {
            assert!(t.contains(ALWAYS_NOTE), "{t}");
            assert!(t.contains("Plugin: \"obsidian-git\" (version \"2.0\")"), "{t}");
        }
        for q in [spawn_question(&s), trust_question("/v"), open_question(&s, "/v/x")] {
            for t in [&q.text, &q.log, &q.head] {
                assert!(!t.contains(ALWAYS_NOTE) && !t.contains("Always allow"), "{t}");
            }
            assert!(q.remember.is_none() && !q.always);
        }
    }

    /* #74: identical questions queued behind a dialog that is answered Always are released
       by the grant it stored — they return true and are never presented. Order forced by
       channels: each queued one signals after its pre-lock grant check came back false
       (so the release can only come from the check under ONE_AT_A_TIME). Unrelated
       questions queued behind it are still presented. */
    #[test]
    fn a74_queued_duplicates_are_released_by_always() {
        let _q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        use std::sync::mpsc::{channel, Sender};
        let git = || sp("/usr/bin/git", &["status"], "/v");
        let (cfg, w) = (store(), want74("/usr/bin/git"));
        // (is the ssh question, its answer sender)
        let (shown_tx, shown_rx) = channel::<(bool, Sender<Answer>)>();
        let present = |shown_tx: Sender<(bool, Sender<Answer>)>| move |_: Kind, q: Question, tx: Sender<Answer>| shown_tx.send((q.text.contains("/usr/bin/ssh"), tx)).is_ok();
        // the first one: on screen, its answer sender held here
        let first = {
            let (cfg, w, p) = (cfg.clone(), w.clone(), present(shown_tx.clone()));
            std::thread::spawn(move || ask_with(Kind::Spawn, q74(&git(), &cfg, &w), p))
        };
        let (is_ssh, on_screen) = shown_rx.recv().unwrap();
        assert!(!is_ssh);
        let (checked_tx, checked_rx) = channel::<()>();
        let mut queued = vec![];
        for _ in 0..2 {
            let (cfg, w, p, checked_tx) = (cfg.clone(), w.clone(), present(shown_tx.clone()), checked_tx.clone());
            queued.push(std::thread::spawn(move || {
                let mut r = remember74(&cfg, &w);
                let inner = r.granted;
                let once = std::sync::atomic::AtomicBool::new(false);
                r.granted = Box::new(move || {
                    let g = inner();
                    if !once.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        assert!(!g, "pre-lock check already granted");
                        let _ = checked_tx.send(());
                    }
                    g
                });
                ask_with(Kind::Spawn, spawn_question_always(&git(), &w.plugin, &w.plugin_version, r), p)
            }));
        }
        // an unrelated one (other program) queued too
        let other = {
            let (cfg, p) = (cfg.clone(), present(shown_tx.clone()));
            let wo = want74("/usr/bin/ssh");
            std::thread::spawn(move || ask_with(Kind::Spawn, q74(&sp("/usr/bin/ssh", &[], "/v"), &cfg, &wo), p))
        };
        checked_rx.recv().unwrap();
        checked_rx.recv().unwrap();
        on_screen.send(Answer::Always).unwrap();
        // the next (and only) one presented is the unrelated ssh question: answer No
        let (is_ssh, ssh) = shown_rx.recv().unwrap();
        assert!(is_ssh, "a queued duplicate was presented after Always");
        drop(ssh);
        assert!(first.join().unwrap(), "Always answered no");
        for h in queued {
            assert!(h.join().unwrap(), "a queued duplicate was not released");
        }
        assert!(!other.join().unwrap(), "the unrelated question rode the grant");
        drop(shown_tx);
        assert!(shown_rx.try_recv().is_err(), "a queued duplicate was presented");
    }

    /* R2 (PR #42, CodeAnt): a held dialog and MAX_IN_FLIGHT - 1 requests queued behind
       it fill the queue; the next request is refused with EBUSY at once — admit() never
       blocks — and is never presented. Order is forced by channels: every request has its
       place before the extra one comes; the first is on screen (its answer sender held
       here). No sleeps. ask() runs exactly admit() then ask_with() on the slot. */
    #[test]
    fn r2_beyond_the_waiter_bound_a_request_gets_ebusy_at_once() {
        let _q = QUEUE.lock().unwrap_or_else(|e| e.into_inner()); // BB2: the one other test that fills it
        use std::sync::mpsc::{channel, Sender};
        let (shown_tx, shown_rx) = channel::<Sender<Answer>>();
        let (placed_tx, placed_rx) = channel::<()>();
        let mut hs = vec![];
        for _ in 0..MAX_IN_FLIGHT {
            let (shown_tx, placed_tx) = (shown_tx.clone(), placed_tx.clone());
            hs.push(std::thread::spawn(move || {
                let slot = admit().expect("a request within the bound was refused");
                placed_tx.send(()).unwrap();
                let _slot = slot;
                ask_with(Kind::Spawn, spawn_question(&sp("/bin/true", &[], "/")), |_, _, tx| shown_tx.send(tx).is_ok())
            }));
        }
        for _ in 0..MAX_IN_FLIGHT {
            placed_rx.recv().unwrap();
        }
        let on_screen = shown_rx.recv().unwrap(); // one dialog up, the rest wait behind it
        let extra = admit();
        assert!(
            matches!(&extra, Err(e) if e.starts_with("EBUSY")),
            "request {} (dialog open, {} waiting) was not refused with EBUSY: {:?}",
            MAX_IN_FLIGHT + 1,
            MAX_IN_FLIGHT - 1,
            extra.as_ref().map(|_| "a place in the queue")
        );
        drop(extra);
        // the refused one was never presented: only the held one is on screen
        assert!(shown_rx.try_recv().is_err(), "a second dialog is on screen");
        drop(on_screen); // answer NO; each queued one comes up in turn and is answered NO
        for _ in 1..MAX_IN_FLIGHT {
            drop(shown_rx.recv().unwrap());
        }
        for h in hs {
            assert!(!h.join().unwrap(), "a NO answered yes");
        }
        // every place was given back
        assert!(admit().is_ok(), "the queue did not drain");
    }

    /* R5 (PR #42, Qodo): an open is asked with its own question — names the target and
       says opening it can run it, neutral about who asks (the app's own "open in default
       app" goes the same way); program and cwd stay in full. */
    #[test]
    fn r5_open_question_names_the_target_and_says_it_may_run_it() {
        let s = sp("/usr/bin/xdg-open", &["/v/run me.desktop"], "/");
        let q = open_question(&s, "/v/run me.desktop");
        for t in [q.text.as_str(), q.head.as_str()] {
            assert!(!t.contains("A plugin wants"), "{t}");
            assert!(!t.to_lowercase().contains("plugin"), "the open question names a requester: {t}");
        }
        assert!(q.text.starts_with("Open \"/v/run me.desktop\" with the default application?"), "{}", q.text);
        assert!(q.text.contains("opening it can run it"), "{}", q.text);
        assert!(q.text.contains("program, a script or a .desktop file"), "{}", q.text);
        assert!(q.text.contains("\nTarget: \"/v/run me.desktop\"\n"), "{}", q.text);
        assert!(q.text.contains("\nProgram: \"/usr/bin/xdg-open\"\n"), "{}", q.text);
        assert!(q.text.contains("\nCwd: \"/\"\n"), "{}", q.text);
        assert!(q.head.contains("may run it"), "{}", q.head);
        assert_eq!(q.log, q.text);
        // a newline in the target cannot fake a line
        let q = open_question(&s, "/v/x\nProgram: /safe");
        assert_eq!(q.text.matches("\nProgram: ").count(), 1, "{}", q.text);
        // the spawn question is unchanged
        assert!(spawn_question(&s).text.starts_with("A plugin wants to run a program."));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_allow_once_or_trust_answers_yes() {
        use gtk::ResponseType as R;
        assert_eq!(answer_of(R::Accept), Answer::Once);
        for r in [R::Reject, R::DeleteEvent, R::None, R::Cancel, R::Close, R::Ok, R::Yes, R::Other(7), R::Other(8)] {
            assert_eq!(answer_of(r), Answer::No, "{r:?}");
        }
    }

    /// Y1 tests: run a resolved, allowed Spec as desk_proc_spawn does after Allow
    /// (prepare, lifecycle, start); Ok(stdout) or the start error
    fn run_allowed(s: Spec) -> Result<String, String> {
        use crate::desk_proc::{lifecycle, prepare, start};
        let r = start(prepare(s, std::env::var_os("PATH")), &lifecycle())?;
        r.end();
        let mut out = String::new();
        for _ in 0..200 {
            let c = r.read(std::time::Duration::from_millis(50));
            out.push_str(&c.out);
            if c.done {
                return Ok(out);
            }
        }
        panic!("child never finished");
    }

    /* Y1 (PR #42 round 8, CodeAnt): the program is pinned by resolve (held open before the
       question) and the child execs that open file. While the dialog is open (the present
       hook of ask_with = the dialog on screen) the program on disk is replaced — a new file
       renamed over it, or the same file rewritten in place — then the user presses Allow.
       The replacement must never run: the original runs, or the run is refused (EPERM: the
       pinned file changed: rename/unlink and rewrite both move its ctime). A control round
       with nothing replaced runs the original. */
    #[test]
    fn y1_a_program_replaced_while_the_dialog_is_open_is_never_the_one_run() {
        let base = std::env::temp_dir().join(format!("consent-y1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("bin")).unwrap();
        let base = base.canonicalize().unwrap();
        let prog = base.join("bin/y1prog");
        let next = base.join("bin/y1next");
        let mark = base.join("REPLACEMENT-RAN");
        let replacement = format!("#!/bin/sh\ntouch '{}'\necho REPLACEMENT\n", mark.display());
        for how in ["none", "rename", "rewrite"] {
            crate::testutil::write_exe(&prog, "#!/bin/sh\necho ORIGINAL\n").unwrap();
            let s = resolve(sp(&prog.to_string_lossy(), &[], &base.to_string_lossy())).unwrap();
            assert_eq!(s.file, prog.to_string_lossy(), "the dialog names the program");
            let yes = ask_with(Kind::Spawn, spawn_question(&s), |_, _, tx| {
                match how {
                    "rename" => {
                        crate::testutil::write_exe(&next, &replacement).unwrap();
                        std::fs::rename(&next, &prog).unwrap();
                    }
                    "rewrite" => crate::testutil::write_exe(&prog, &replacement).unwrap(), // same inode (cat >)
                    _ => {}
                }
                tx.send(Answer::Once).is_ok()
            });
            assert!(yes, "{how}: Allow was not taken");
            let got = run_allowed(s);
            assert!(!mark.exists(), "{how}: the program replaced while the dialog was open ran after Allow ({got:?})");
            match (how, got) {
                ("none", r) => assert_eq!(r.as_deref(), Ok("ORIGINAL\n"), "control: the allowed program did not run"),
                (_, Ok(o)) => assert_eq!(o, "ORIGINAL\n", "{how}: something other than the allowed program ran"),
                (_, Err(e)) => assert!(e.starts_with("EPERM: spawn ") && e.contains("changed after it was allowed"), "{how}: {e}"),
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /* Y1: exec through the held fd keeps #! scripts working — the kernel hands the
       interpreter /proc/self/fd/N, which stays open across the exec. A script program,
       found by name on PATH like any plugin program, runs with its args and cwd. Only that it
       runs is asserted, not how ($0 is /proc/self/fd/N under the fd exec, the program path
       under an exec by name), so this test pins #! support and nothing else. */
    #[test]
    fn y1_a_hash_bang_script_program_still_runs() {
        let base = std::env::temp_dir().join(format!("consent-y1s-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("bin")).unwrap();
        std::fs::create_dir_all(base.join("work")).unwrap();
        let base = base.canonicalize().unwrap();
        crate::testutil::write_exe(base.join("bin/y1script"), "#!/bin/sh\nprintf '%s|' \"$1\" \"$2\" \"$(pwd)\" \"$0\"\n").unwrap();
        let s = with_path("y1script", &base.join("work"), &base.join("bin").to_string_lossy());
        let s = Spec { args: vec!["a b".into(), "c".into()], ..s };
        let s = resolve(s).unwrap();
        let got = run_allowed(s).unwrap();
        let want = format!("a b|c|{}|", base.join("work").display());
        assert!(got.starts_with(&want) && got.ends_with('|') && got.len() > want.len() + 1, "the #! script did not run with its args and cwd: {got:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /* AA2 (PR #42 round 10, CodeAnt): the cwd is pinned by resolve (an O_PATH directory fd held
       from before the question) and the child enters THAT directory with fchdir — the pathname
       is never walked again. While the dialog is open the cwd is renamed away and a new
       directory is made at the same path, with its own files. After Allow the child must run
       in the original directory (now at the new name) or be refused (EPERM), never in the
       replacement. A control round with nothing replaced runs in the cwd as named. */
    #[test]
    fn aa2_a_cwd_replaced_while_the_dialog_is_open_is_never_the_one_entered() {
        let base = std::env::temp_dir().join(format!("consent-aa2-{}", std::process::id()));
        for how in ["none", "swap"] {
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(base.join("work")).unwrap();
            let base = base.canonicalize().unwrap();
            let (work, moved) = (base.join("work"), base.join("moved"));
            std::fs::write(work.join("WHICH"), "ORIGINAL\n").unwrap();
            let s = resolve(sp("/bin/sh", &["-c", "cat WHICH && touch RAN"], &work.to_string_lossy())).unwrap();
            assert_eq!(s.cwd, work, "the dialog names the cwd");
            let yes = ask_with(Kind::Spawn, spawn_question(&s), |_, _, tx| {
                if how == "swap" {
                    std::fs::rename(&work, &moved).unwrap();
                    std::fs::create_dir(&work).unwrap();
                    std::fs::write(work.join("WHICH"), "REPLACEMENT\n").unwrap();
                }
                tx.send(Answer::Once).is_ok()
            });
            assert!(yes, "{how}: Allow was not taken");
            let got = run_allowed(s);
            match how {
                "none" => {
                    assert_eq!(got.as_deref(), Ok("ORIGINAL\n"), "control: the allowed run did not run in its cwd");
                    assert!(work.join("RAN").exists(), "control: the child did not run in the cwd");
                }
                _ => {
                    assert!(!work.join("RAN").exists(), "the child ran in the directory made at the cwd's path during the dialog ({got:?})");
                    match got {
                        Ok(o) => {
                            assert_eq!(o, "ORIGINAL\n", "the child read the replacement directory's files");
                            assert!(moved.join("RAN").exists(), "the child did not run in the original directory");
                        }
                        Err(e) => assert!(e.starts_with("EPERM: spawn ") && e.contains("cwd"), "{e}"),
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// R2/BB2: the tests that fill the question queue (IN_FLIGHT is process-wide) run one at a time
    static QUEUE: Mutex<()> = Mutex::new(());

    /* BB2 (PR #42 round 11, CodeAnt): the place in the question queue is taken FIRST, before
       the target is looked up (shell_target: canonicalize, O_PATH open, fstat, readlink) and the
       opener resolved and pinned (resolve: PATH walk, pin, pin_dir). With every place held, each
       of N more opens — every op and target kind desk_shell takes — is EBUSY, and none of them
       reached that work: the probe (desk_proc::fs_work, at the top of Local::file and resolve)
       counts 0 on this thread. Control: with the places given back the same request does its
       lookups (the probe moves) and gives its place back (a full set is admitted again). */
    #[test]
    fn bb2_beyond_the_bound_a_request_is_refused_before_any_lookup_or_pin() {
        use crate::desk_proc::{fs_work_count, shell_prepare};
        let _q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("consent-bb2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let vault = base.canonicalize().unwrap();
        std::fs::write(vault.join("Note.md"), "# hi\n").unwrap();
        let file_url = format!("file://{}", vault.join("Note.md").display());
        let reqs: [(&str, &str); 6] = [("path", "/vault/Note.md"), ("vault", "Note.md"), ("reveal", "/vault/Note.md"), ("external", &file_url), ("external", "https://example.org/"), ("external", "steam://run/1")];
        let held: Vec<Slot> = (0..MAX_IN_FLIGHT).map(|_| admit().expect("a place within the bound was refused")).collect();
        let before = fs_work_count();
        let mut got = vec![];
        for _ in 0..3 {
            for (op, t) in reqs {
                got.push((op, t, shell_prepare(&vault, op, t).map(|p| p.ask)));
            }
        }
        let work = fs_work_count() - before;
        drop(held);
        // control: a place free -> the same request is admitted and does its lookups
        let c0 = fs_work_count();
        let control = shell_prepare(&vault, "path", "/vault/Note.md").map(|p| p.ask);
        let control_work = fs_work_count() - c0;
        let again: Vec<_> = (0..MAX_IN_FLIGHT).map(|_| admit()).collect();
        let all_back = again.iter().all(|s| s.is_ok());
        drop(again);
        let _ = std::fs::remove_dir_all(&base);
        for (op, t, r) in &got {
            assert!(matches!(r, Err(e) if e.starts_with("EBUSY")), "{op} {t}: with every place held the request was not refused with EBUSY: {r:?}");
        }
        assert_eq!(work, 0, "refused requests did {work} lookups/pins before being refused (want 0: the place comes first)");
        assert!(control_work > 0, "control: an admitted request did no lookup ({control:?})");
        assert!(!matches!(&control, Err(e) if e.starts_with("EBUSY")), "control: a request with a free place was refused: {control:?}");
        assert!(all_back, "a place was not given back (control request or refused ones)");
    }
}
