// opensidian, a vault-compatible markdown notes app.
// Copyright (C) 2026 Jendrik Poloczek
// SPDX-License-Identifier: GPL-3.0-or-later
/* spawn-consent (GHSA-6xfp-789j-9jr3, operator 2026-11-01): two native GTK
   prompts, outside the webview, that page script cannot answer for the user:
     (a) before a vault becomes trusted (plugins_set trusted:true), and
     (b) before EVERY program a plugin starts (desk_proc_spawn), ahead of
         desk_proc::start -> Command::new.
   The dialog text is built HERE from Rust-side data only: the canonical vault
   path; the program (and where PATH resolves it), the args after map_arg, the
   cwd after map_cwd, the env keys the plugin lays over ours. Nothing the page
   sends is shown as anything but the literal value it is (each value is
   Debug-quoted, so a newline or a control char cannot fake a line of the
   dialog). Buttons: Deny/Cancel is the default (Enter, Escape, the window's
   close button, a destroyed dialog, a lost main loop all answer NO); Allow/
   Trust must be pressed. The spawn prompt also offers "Allow for this
   session" (operator STEER 13:32Z): it remembers (vault, resolved program)
   in memory only — see Session below. No timeout, and no env var, cfg flag,
   feature or test hook that answers for the user — tests click the dialog
   (harness phase spawnconsent; scripts/consent-nobypass.sh guards the calls).

   Threading: the commands that ask are async (tauri runs them off the main
   thread). ask() parks a blocking-pool thread on a channel; the dialog is
   built on the main thread via run_on_main_thread and answers through its
   "response" signal — no nested gtk_dialog_run, so the main loop keeps
   serving IPC and painting while the question is open. One question at a
   time: a plugin spawning in a loop queues behind the open dialog instead of
   stacking windows. */
use std::path::{Path, PathBuf};
use std::collections::BTreeSet;
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

/// the longest value shown verbatim; past it the dialog says how much was cut
const SHOW_MAX: usize = 400;
/// the most args listed one per line; past it the dialog says how many more
const ARGS_MAX: usize = 24;

/// one page-supplied value as the dialog shows it: Debug-quoted (escapes \n,
/// \t, \u{..}, quotes) and capped, so it is always one honest line
fn shown(s: &str) -> String {
    let q = format!("{s:?}");
    if q.chars().count() <= SHOW_MAX {
        return q;
    }
    let head: String = q.chars().take(SHOW_MAX).collect();
    format!("{head}… ({} more chars not shown)", q.chars().count() - SHOW_MAX)
}

/// where Command::new(file) will find the program: a name with a '/' is used
/// as given (relative to cwd); a bare name is looked up on PATH — the child's
/// PATH if the plugin sets one, else ours. None = not found (start will fail).
pub fn resolve_program(sp: &Spec) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let exe = |p: &Path| std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false);
    if sp.file.contains('/') {
        let p = Path::new(&sp.file);
        let p = if p.is_absolute() { p.to_path_buf() } else { sp.cwd.join(p) };
        return exe(&p).then_some(p);
    }
    let path = sp.env.iter().rev().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).or_else(|| std::env::var("PATH").ok())?;
    path.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(&sp.file)).find(|p| exe(p))
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

/// the spawn prompt's body, from the Spec desk_proc::start will run
pub fn spawn_text(sp: &Spec) -> String {
    let resolved = match resolve_program(sp) {
        Some(p) if p.as_os_str() != sp.file.as_str() => format!("  (resolves to {})", shown(&p.to_string_lossy())),
        Some(_) => String::new(),
        None => "  (not found)".into(),
    };
    let mut t = format!("A plugin wants to run a program.\n\nProgram: {}{resolved}\n", shown(&sp.file));
    if sp.args.is_empty() {
        t.push_str("Args: (none)\n");
    } else {
        t.push_str(&format!("Args ({}):\n", sp.args.len()));
        for a in sp.args.iter().take(ARGS_MAX) {
            t.push_str(&format!("    {}\n", shown(a)));
        }
        if sp.args.len() > ARGS_MAX {
            t.push_str(&format!("    … {} more not shown\n", sp.args.len() - ARGS_MAX));
        }
    }
    t.push_str(&format!("Cwd: {}\n", shown(&sp.cwd.to_string_lossy())));
    if !sp.env.is_empty() {
        let keys: Vec<String> = sp.env.iter().map(|(k, v)| format!("{}={}", shown(k), shown(v))).collect();
        t.push_str(&format!("Env set by the plugin: {}\n", keys.join(" ")));
    }
    t.push_str("\nIt will run with your user's rights.");
    t
}

/* "Allow for this session" (operator STEER 2026-11-01 13:32Z): the key is
   (canonical vault, RESOLVED absolute program path after the PATH lookup) —
   args are NOT part of it, and the dialog says so. The set lives ONLY here,
   in process memory: never persisted, no IPC command reads or writes it
   (session_allowed/session_remember are private; session_clear is the one
   pub door, called by plugins_set when trust flips OFF next to
   desk_proc::kill_all). A vault switch is a new process (vaultbleed), so it
   starts empty there too. `epoch` bumps on every clear: an answer given in a
   dialog that was open across a trust-OFF is not remembered. */
struct Session {
    epoch: u64,
    set: BTreeSet<(String, PathBuf)>,
}
static SESSION_ALLOW: Mutex<Session> = Mutex::new(Session { epoch: 0, set: BTreeSet::new() });

fn session() -> std::sync::MutexGuard<'static, Session> {
    SESSION_ALLOW.lock().unwrap_or_else(|e| e.into_inner())
}

/// trust flipped OFF: forget every "Allow for this session"
pub fn session_clear() {
    let mut s = session();
    s.set.clear();
    s.epoch += 1;
    eprintln!("[consent] session allowlist cleared");
}

fn session_allowed(key: &(String, PathBuf)) -> bool {
    session().set.contains(key)
}

/// remember `key`, unless the allowlist was cleared since `epoch` was read
fn session_remember(key: (String, PathBuf), epoch: u64) {
    let mut s = session();
    if s.epoch == epoch {
        s.set.insert(key);
    }
}

/// what ask() puts on screen: the body, and for a spawn whose program
/// resolves to an absolute path, the session key "Allow for this session"
/// would remember (None = that button is not offered)
pub struct Question {
    text: String,
    session: Option<(String, PathBuf)>,
}

pub fn trust_question(canon: &str) -> Question {
    Question { text: trust_text(canon), session: None }
}

/// `vault` = plugins::trust_key(root), the canonical vault path
pub fn spawn_question(vault: &str, sp: &Spec) -> Question {
    let mut text = spawn_text(sp);
    let session = resolve_program(sp).filter(|p| p.is_absolute()).map(|p| (vault.to_string(), p));
    if let Some((_, p)) = &session {
        text.push_str(&session_note(p));
    }
    Question { text, session }
}

/// the line that tells the user what the session button covers
fn session_note(p: &Path) -> String {
    format!(
        "\n\n\"Allow for this session\" allows {} with any arguments until opensidian quits \
         (or community plugins are turned off).",
        shown(&p.to_string_lossy())
    )
}

/// the dialog's answer. Only answer_of() makes Once/Session — from a GTK
/// response the user produced by pressing that button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    No,
    Once,
    Session,
}

/// response ids: Accept = Allow once / Trust, Other(SESSION_RESPONSE) = Allow
/// for this session; everything else (Reject, DeleteEvent, None, ...) = No
const SESSION_RESPONSE: u16 = 7;

#[cfg(target_os = "linux")]
fn answer_of(r: gtk::ResponseType, session_offered: bool) -> Answer {
    match r {
        gtk::ResponseType::Accept => Answer::Once,
        gtk::ResponseType::Other(SESSION_RESPONSE) if session_offered => Answer::Session,
        _ => Answer::No,
    }
}

static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// ask the user; true only when Allow/Trust was pressed now, or "Allow for
/// this session" was pressed earlier for this exact (vault, program). BLOCKS
/// until the answer: call it from a worker thread, never the main thread.
fn ask_blocking(win: &tauri::Window, kind: Kind, q: Question) -> bool {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // checked under ONE_AT_A_TIME: a spawn queued behind the dialog that
    // just allowed its program for the session does not ask again
    if let Some(key) = &q.session {
        if session_allowed(key) {
            eprintln!("[consent] spawn {:?}: allowed for this session, not asked", key.1);
            return true;
        }
    }
    let epoch = session().epoch;
    let (tx, rx) = std::sync::mpsc::channel::<Answer>();
    let w2 = win.clone();
    let offered = q.session.is_some();
    let text = q.text;
    if win.run_on_main_thread(move || show_dialog(&w2, kind, &text, offered, tx)).is_err() {
        return false;
    }
    // a dialog torn down without a response drops tx: recv errs -> No
    match rx.recv().unwrap_or(Answer::No) {
        Answer::No => false,
        Answer::Once => true,
        Answer::Session => {
            if let Some(key) = q.session {
                eprintln!("[consent] spawn {:?}: allowed for this session", key.1);
                session_remember(key, epoch);
            }
            true
        }
    }
}

/// the async front door the commands await. Any failure on the way = NO.
pub async fn ask(win: tauri::Window, kind: Kind, q: Question) -> bool {
    let what = match kind {
        Kind::Trust => "trust",
        Kind::Spawn => "spawn",
    };
    eprintln!("[consent] {what}: asking");
    let yes = tauri::async_runtime::spawn_blocking(move || ask_blocking(&win, kind, q)).await.unwrap_or(false);
    eprintln!("[consent] {what}: {}", if yes { "allowed" } else { "denied" });
    yes
}

#[cfg(target_os = "linux")]
fn show_dialog(win: &tauri::Window, kind: Kind, text: &str, session_offered: bool, tx: std::sync::mpsc::Sender<Answer>) {
    use gtk::prelude::*;
    let parent = win.gtk_window().ok();
    let (title, no, yes) = match kind {
        Kind::Trust => (TRUST_TITLE, "_Cancel", "_Trust"),
        Kind::Spawn => (SPAWN_TITLE, "_Deny", "Allow _once"),
    };
    // the message is plain text (gtk_message_dialog_new formats it with "%s"),
    // never markup: a page value cannot style or hide part of the prompt
    let d = gtk::MessageDialog::new(
        parent.as_ref(),
        gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
        gtk::MessageType::Warning,
        gtk::ButtonsType::None,
        text,
    );
    d.set_title(title);
    let _ = d.add_button(no, gtk::ResponseType::Reject);
    let _ = d.add_button(yes, gtk::ResponseType::Accept);
    if session_offered {
        let _ = d.add_button("Allow for this _session", gtk::ResponseType::Other(SESSION_RESPONSE));
    }
    d.set_default_response(gtk::ResponseType::Reject);
    d.set_keep_above(true);
    let tx = std::cell::RefCell::new(Some(tx));
    d.connect_response(move |d, r| {
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(answer_of(r, session_offered));
        }
        // SAFETY: the dialog is ours alone (no other Rust handle outlives this
        // handler); destroying a toplevel from its own response handler is the
        // documented GtkDialog pattern.
        unsafe { d.destroy() };
    });
    d.show_all();
    d.present();
}

/// no native prompt on this platform = NO (nothing is trusted or started)
#[cfg(not(target_os = "linux"))]
fn show_dialog(_win: &tauri::Window, _kind: Kind, _text: &str, _session_offered: bool, tx: std::sync::mpsc::Sender<Answer>) {
    let _ = tx.send(Answer::No);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(file: &str, args: &[&str], cwd: &str) -> Spec {
        Spec { file: file.into(), args: args.iter().map(|s| s.to_string()).collect(), cwd: cwd.into(), env: vec![] }
    }

    #[test]
    fn spawn_text_names_program_args_and_cwd() {
        let t = spawn_text(&sp("touch", &["/v/marker"], "/v"));
        assert!(t.contains("Program: \"touch\""), "{t}");
        assert!(t.contains("    \"/v/marker\"\n"), "{t}");
        assert!(t.contains("Cwd: \"/v\""), "{t}");
    }

    #[test]
    fn a_newline_in_an_arg_cannot_fake_a_dialog_line() {
        let t = spawn_text(&sp("sh", &["-c", "rm -rf ~\nCwd: /safe"], "/v"));
        assert_eq!(t.matches("\nCwd: ").count(), 1, "{t}");
        assert!(t.contains("\\nCwd: /safe"), "{t}");
    }

    #[test]
    fn long_values_and_many_args_say_what_was_cut() {
        let long = "x".repeat(5000);
        let many: Vec<&str> = (0..40).map(|_| "a").collect();
        let t = spawn_text(&sp(&long, &many, "/v"));
        assert!(t.contains("more chars not shown"), "{t}");
        assert!(t.contains("… 16 more not shown"), "{t}");
    }

    #[test]
    fn env_the_plugin_sets_is_shown() {
        let mut s = sp("git", &[], "/v");
        s.env.push(("LD_PRELOAD".into(), "/tmp/x.so".into()));
        assert!(spawn_text(&s).contains("\"LD_PRELOAD\"=\"/tmp/x.so\""));
    }

    #[test]
    fn bare_names_resolve_on_the_childs_path() {
        let d = std::env::temp_dir().join(format!("consent-path-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("fakeprog");
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut s = sp("fakeprog", &[], "/");
        s.env.push(("PATH".into(), d.display().to_string()));
        assert_eq!(resolve_program(&s), Some(f.clone()));
        assert!(spawn_text(&s).contains("resolves to"));
        s.env.clear();
        s.env.push(("PATH".into(), "/nonexistent-consent".into()));
        assert_eq!(resolve_program(&s), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn trust_text_shows_the_canonical_path() {
        assert!(trust_text("/home/u/My Vault").contains("Vault: \"/home/u/My Vault\""));
    }

    #[test]
    fn the_session_key_is_vault_and_resolved_path_not_args() {
        let a = spawn_question("/v", &sp("/bin/sh", &["-c", "a"], "/"));
        let b = spawn_question("/v", &sp("/bin/sh", &["-c", "b"], "/"));
        assert_eq!(a.session, Some(("/v".to_string(), PathBuf::from("/bin/sh"))));
        assert_eq!(a.session, b.session);
        assert_ne!(a.session, spawn_question("/w", &sp("/bin/sh", &[], "/")).session);
        assert!(a.text.contains("allows \"/bin/sh\" with any arguments until opensidian quits"), "{}", a.text);
    }

    #[test]
    fn no_session_button_for_a_program_that_does_not_resolve() {
        let q = spawn_question("/v", &sp("/nonexistent-consent/prog", &[], "/"));
        assert_eq!(q.session, None);
        assert!(!q.text.contains("Allow for this session"));
        assert_eq!(trust_question("/v").session, None);
    }

    #[test]
    fn a_clear_forgets_and_a_stale_epoch_is_not_remembered() {
        let k = ("/consent-test-vault".to_string(), PathBuf::from(format!("/consent-test/{}", std::process::id())));
        let e = session().epoch;
        session_remember(k.clone(), e);
        assert!(session_allowed(&k));
        session_clear();
        assert!(!session_allowed(&k));
        // an answer from a dialog opened before the clear
        session_remember(k.clone(), e);
        assert!(!session_allowed(&k));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_the_session_button_answers_session_and_only_when_offered() {
        use gtk::ResponseType as R;
        assert_eq!(answer_of(R::Accept, false), Answer::Once);
        assert_eq!(answer_of(R::Other(SESSION_RESPONSE), true), Answer::Session);
        assert_eq!(answer_of(R::Other(SESSION_RESPONSE), false), Answer::No);
        for r in [R::Reject, R::DeleteEvent, R::None, R::Cancel, R::Close, R::Ok, R::Yes, R::Other(8)] {
            assert_eq!(answer_of(r, true), Answer::No, "{r:?}");
        }
    }
}
