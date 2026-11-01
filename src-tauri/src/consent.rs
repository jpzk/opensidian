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
   answer NO); Allow once / Trust must be pressed, for every question: there is
   no session-wide answer and nothing is remembered between two runs
   (PR #41 N2/N3). The app log gets the body with env VALUES masked (N1).
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
    let run: usize = sp.file.len() + 1 + sp.args.iter().map(|a| a.len() + 1).sum::<usize>() + sp.cwd.as_os_str().len() + 1;
    if run > RUN_MAX_BYTES {
        return Err(format!(
            "E2BIG: spawn: the program, its {} args and the cwd ({} bytes) are over the cap ({} bytes); refused, nothing asked or started",
            sp.args.len(),
            run,
            RUN_MAX_BYTES
        ));
    }
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

/// F1: where the program is, found ONCE. A name with a '/' is taken relative
/// to the CHILD's cwd (what exec would do). A bare name is looked up on PATH —
/// the child's PATH if the plugin sets one, else ours — on ABSOLUTE entries
/// only: an empty entry, "." or any relative entry would be resolved by exec
/// against the child's cwd (inside the vault, plugin-writable), so it is
/// skipped. The result is absolute and is what desk_proc execs (resolve), so
/// the dialog and the exec cannot disagree. None = not found.
pub fn resolve_program(sp: &Spec) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let exe = |p: &Path| std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false);
    if sp.file.is_empty() {
        return None;
    }
    if sp.file.contains('/') {
        let p = Path::new(&sp.file);
        let p = if p.is_absolute() { p.to_path_buf() } else { sp.cwd.join(p) };
        return (p.is_absolute() && exe(&p)).then_some(p);
    }
    let path = sp.env.iter().rev().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).or_else(|| std::env::var("PATH").ok())?;
    path.split(':').map(Path::new).filter(|d| d.is_absolute()).map(|d| d.join(&sp.file)).find(|p| exe(p))
}

/// F1: the Spec that will run — file = the absolute path resolve_program found
/// (the one the dialog names), argv0 = the name the plugin asked for. Not found
/// = ENOENT, before anything is asked.
pub fn resolve(sp: Spec) -> Result<Spec, String> {
    let p = resolve_program(&sp)
        .ok_or_else(|| format!("ENOENT: spawn {} not found (bare names are looked up on absolute PATH entries only)", sp.file))?;
    let file = p.into_os_string().into_string().map_err(|_| format!("ENOENT: spawn {}: resolved path is not UTF-8", sp.file))?;
    let argv0 = if file != sp.file { Some(sp.file.clone()) } else { sp.argv0.clone() };
    Ok(Spec { file, argv0, ..sp })
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
   when the very same run was allowed a moment ago. There is no allowlist, no
   key, nothing remembered between two questions — so nothing a plugin can
   rewrite between runs (a program in the vault, a git hook, a script an
   interpreter reads) can ride on an earlier answer. */

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
}

const TRUST_HEAD: &str = "Turn on community plugins for this vault?";
const SPAWN_HEAD: &str = "A plugin wants to run a program. Read all of it (scroll) before you answer.";
const OPEN_HEAD: &str = "Open this with the default application? It may run it. Read all of it (scroll) before you answer.";

pub fn trust_question(canon: &str) -> Question {
    let text = trust_text(canon);
    Question { head: TRUST_HEAD.into(), log: text.clone(), text }
}

/// `sp` = the Spec after resolve() (absolute file)
pub fn spawn_question(sp: &Spec) -> Question {
    Question { head: SPAWN_HEAD.into(), text: spawn_text(sp), log: spawn_log(sp) }
}

/// R5: an open through the default opener. `sp` = the resolved opener Spec (no env)
pub fn open_question(sp: &Spec, target: &str) -> Question {
    let text = open_text(sp, target);
    Question { head: OPEN_HEAD.into(), log: text.clone(), text }
}

/// the dialog's answer. Only answer_of() makes Once — from a GTK response the
/// user produced by pressing Allow once / Trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    No,
    Once,
}

/// Accept = Allow once / Trust; everything else (Reject, DeleteEvent, None, ...) = No
#[cfg(target_os = "linux")]
fn answer_of(r: gtk::ResponseType) -> Answer {
    match r {
        gtk::ResponseType::Accept => Answer::Once,
        _ => Answer::No,
    }
}

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// one question, one at a time: `present` puts it in front of the user and hands
/// the answer to the sender (false = it could not be presented = NO). Every call
/// presents: there is no earlier answer that stands in for this one.
fn ask_with(kind: Kind, q: Question, present: impl FnOnce(Kind, Question, std::sync::mpsc::Sender<Answer>) -> bool) -> bool {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (tx, rx) = std::sync::mpsc::channel::<Answer>();
    if !present(kind, q, tx) {
        return false;
    }
    // a dialog torn down without a response drops tx: recv errs -> No
    rx.recv().unwrap_or(Answer::No) == Answer::Once
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

/// R2: a place in the bounded queue; dropping it frees the place
struct Slot(());

impl Drop for Slot {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// R2: a place in the queue, or EBUSY at once (nothing asked, no thread taken)
/// when MAX_IN_FLIGHT questions are already open or queued
fn admit() -> Result<Slot, String> {
    IN_FLIGHT
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < MAX_IN_FLIGHT).then_some(n + 1))
        .map(|_| Slot(()))
        .map_err(|n| format!("EBUSY: {n} consent questions are already open or waiting (at most {MAX_IN_FLIGHT}); refused, nothing asked"))
}

/// the async front door the commands await: Ok(true) only for Allow once /
/// Trust; Ok(false) for every NO (any failure on the way included); Err(EBUSY)
/// when the queue is full (R2) — then nothing was asked.
pub async fn ask(win: tauri::Window, kind: Kind, q: Question) -> Result<bool, String> {
    let what = match kind {
        Kind::Trust => "trust",
        Kind::Spawn => "spawn",
    };
    let slot = admit().inspect_err(|e| eprintln!("[consent] {what}: {e}"))?;
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
    // N1: the log gets the body with every env VALUE masked (names, program, args and
    // cwd stay: the harness asserts on them). The view itself holds q.text, in full.
    eprintln!("[consent] dialog text: {:?}", q.log);
    let _ = d.add_button(no, gtk::ResponseType::Reject);
    let _ = d.add_button(yes, gtk::ResponseType::Accept);
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
    use std::os::unix::fs::PermissionsExt;

    fn sp(file: &str, args: &[&str], cwd: &str) -> Spec {
        Spec { file: file.into(), args: args.iter().map(|s| s.to_string()).collect(), cwd: cwd.into(), root: cwd.into(), env: vec![], argv0: None, path: None }
    }

    /// base/vault (the plugin's cwd, with its own `fakeprog`), base/bin (the real
    /// `fakeprog`), base/link -> vault. Each script prints where it lives.
    fn tree(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("consent-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for (dir, say) in [("vault", "VAULT"), ("bin", "BIN"), ("vault/rel", "REL")] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
            let f = base.join(dir).join("fakeprog");
            std::fs::write(&f, format!("#!/bin/sh\necho {say}\n")).unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
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
        let real = bin.join("fakeprog");
        for path in [format!(":{}", bin.display()), format!(".:{}", bin.display()), format!("rel:{}", bin.display()), format!("{}::", bin.display())] {
            assert_eq!(resolve_program(&with_path("fakeprog", &v, &path)), Some(real.clone()), "PATH={path:?}");
        }
        for path in ["", ":", ".", "rel", "./", "::."] {
            assert_eq!(resolve_program(&with_path("fakeprog", &v, path)), None, "PATH={path:?}");
            assert!(resolve(with_path("fakeprog", &v, path)).err().expect("fakeprog resolved").starts_with("ENOENT: spawn fakeprog"), "PATH={path:?}");
        }
        // a name with a '/' is the child's cwd + name, made absolute
        assert_eq!(resolve_program(&with_path("./fakeprog", &v, "")), Some(v.join("./fakeprog")));
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

    // N2/N3: no session grant. The same run, asked twice and allowed the first time,
    // is presented to the user again; nothing in the question mentions a session.
    #[test]
    fn n2_an_identical_second_spawn_asks_again() {
        let s = sp("/bin/true", &["x"], "/");
        let mut presented = 0;
        for _ in 0..3 {
            let q = spawn_question(&s);
            assert!(!q.text.to_lowercase().contains("session"), "{}", q.text);
            let yes = ask_with(Kind::Spawn, q, |_, _, tx| {
                presented += 1;
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
    }

    /* R2 (PR #42, CodeAnt): a held dialog and MAX_IN_FLIGHT - 1 requests queued behind
       it fill the queue; the next request is refused with EBUSY at once — admit() never
       blocks — and is never presented. Order is forced by channels: every request has its
       place before the extra one comes; the first is on screen (its answer sender held
       here). No sleeps. ask() runs exactly admit() then ask_with() on the slot. */
    #[test]
    fn r2_beyond_the_waiter_bound_a_request_gets_ebusy_at_once() {
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
}
