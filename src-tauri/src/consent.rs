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
   answer NO); Allow/Trust must be pressed. The spawn prompt may also offer
   "Allow for this session" (operator STEER 13:32Z), narrowed by F3/F4: it
   remembers the EXACT run (vault, program, argv0, args, cwd, env) in memory
   only, and is never offered for a program inside the vault (the plugin can
   rewrite it) or for a shell / interpreter / git / env-style runner (it runs
   whatever its input says). See Session below. No timeout, and no env var, cfg
   flag, feature or test hook that answers for the user — tests click the
   dialog (harness phase spawnconsent; scripts/consent-nobypass.sh guards the
   calls).

   Threading: the commands that ask are async (tauri runs them off the main
   thread). ask() parks a blocking-pool thread on a channel; the dialog is
   built on the main thread via run_on_main_thread and answers through its
   "response" signal — no nested gtk_dialog_run, so the main loop keeps
   serving IPC and painting while the question is open. One question at a
   time: a plugin spawning in a loop queues behind the open dialog instead of
   stacking windows. */
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
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

/// one page-supplied value as the dialog shows it: Debug-quoted (escapes \n,
/// \t, \u{..}, quotes) and NEVER cut (F2) — the user approves what they saw
fn shown(s: &str) -> String {
    format!("{s:?}")
}

/// F6: refuse an env over the cap before any dialog is built
pub fn check_env(sp: &Spec) -> Result<(), String> {
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
            t.push_str(&format!("    {}={}\n", shown(k), shown(v)));
        }
    }
    t.push_str("\nIt will run with your user's rights.");
    t
}

/* "Allow for this session" (operator STEER 2026-11-01 13:32Z, narrowed by PR #41
   F3/F4): the key is the EXACT run — canonical vault, absolute program, argv0,
   args, cwd, env. Any difference (another arg, another cwd, an added
   LD_PRELOAD) asks again, and the dialog says so. The set lives ONLY here, in
   process memory: never persisted, no IPC command reads or writes it
   (session_allowed/session_remember are private; session_clear is the one pub
   door, called by plugins_set when trust flips OFF next to
   desk_proc::kill_all). A vault switch is a new process (vaultbleed), so it
   starts empty there too. `epoch` bumps on every clear: an answer given in a
   dialog that was open across a trust-OFF is not remembered. */
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SessionKey {
    vault: String,
    file: String,
    argv0: Option<String>,
    args: Vec<String>,
    cwd: PathBuf,
    env: Vec<(String, String)>,
}

struct Session {
    epoch: u64,
    set: BTreeSet<SessionKey>,
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

fn session_allowed(key: &SessionKey) -> bool {
    session().set.contains(key)
}

/// remember `key`, unless the allowlist was cleared since `epoch` was read
fn session_remember(key: SessionKey, epoch: u64) {
    let mut s = session();
    if s.epoch == epoch {
        s.set.insert(key);
    }
}

/// F4: programs whose behaviour is whatever their input says — shells,
/// interpreters, git (hooks, aliases, config), env-style launchers, and the desktop
/// openers (G3: xdg-open runs whatever its target maps to). Exact names,
/// and interpreter stems followed only by a version ("python3.12", "lua5.4").
const RUNNERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "fish", "ksh", "mksh", "csh", "tcsh", "busybox", "git", "env", "xargs", "sudo", "doas",
    "pkexec", "nohup", "nice", "timeout", "setsid", "stdbuf", "flock", "xdg-open", "gio", "kde-open", "kde-open5", "gnome-open",
    "exo-open", "mimeopen", "gtk-launch",
];
const RUNNER_STEMS: &[&str] = &["python", "pypy", "node", "nodejs", "perl", "ruby", "php", "lua", "luajit", "deno", "bun", "tclsh", "wish", "pwsh"];

fn is_runner(name: &str) -> bool {
    RUNNERS.contains(&name)
        || RUNNER_STEMS
            .iter()
            .any(|s| name.strip_prefix(s).is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-')))
}

/// ".." and "." removed without touching the disk
fn lexical(p: &Path) -> PathBuf {
    let mut o = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                o.pop();
            }
            Component::CurDir => {}
            c => o.push(c),
        }
    }
    o
}

/// F3: the program file, or the directory entry naming it, lies inside the
/// vault (lexically, through symlinks, or via a symlinked parent). A path that
/// cannot be resolved counts as inside: when in doubt, no session.
fn inside_vault(vault: &str, p: &Path) -> bool {
    let v = Path::new(vault);
    let vr = v.canonicalize().unwrap_or_else(|_| v.to_path_buf());
    let real = p.canonicalize().ok();
    let entry = p.parent().and_then(|d| d.canonicalize().ok()).zip(p.file_name()).map(|(d, n)| d.join(n));
    [Some(lexical(p)), real, entry].iter().any(|x| x.as_ref().is_none_or(|x| x.starts_with(v) || x.starts_with(&vr)))
}

/// why "Allow for this session" is not offered for this resolved Spec, if not
fn no_session_why(vault: &str, sp: &Spec) -> Option<&'static str> {
    let p = Path::new(&sp.file);
    if !p.is_absolute() {
        return Some("the program path is not absolute");
    }
    if inside_vault(vault, p) {
        return Some("the program is inside the vault, where a plugin can change it between runs");
    }
    let real = p.canonicalize().ok();
    let names = [p.file_name(), real.as_deref().and_then(Path::file_name), sp.argv0.as_deref().map(Path::new).and_then(Path::file_name)];
    if names.iter().flatten().any(|n| is_runner(&n.to_string_lossy())) {
        return Some("it is a shell, an interpreter, git or a launcher, which runs whatever its input says");
    }
    None
}

/// what ask() puts on screen: the body, and for a spawn that qualifies, the
/// exact-run key "Allow for this session" would remember (None = that button
/// is not offered)
pub struct Question {
    text: String,
    session: Option<SessionKey>,
}

pub fn trust_question(canon: &str) -> Question {
    Question { text: trust_text(canon), session: None }
}

/// the line that tells the user what the session button covers
const SESSION_NOTE: &str = "\n\n\"Allow for this session\" lets exactly this run happen again without asking — this program, \
     these arguments, this working directory and this environment, nothing else — until opensidian quits \
     (or community plugins are turned off). Any difference asks again.";

/// `vault` = plugins::trust_key(root), the canonical vault path; `sp` = the
/// Spec after resolve() (absolute file)
pub fn spawn_question(vault: &str, sp: &Spec) -> Question {
    let mut text = spawn_text(sp);
    let session = match no_session_why(vault, sp) {
        None => {
            text.push_str(SESSION_NOTE);
            Some(SessionKey {
                vault: vault.to_string(),
                file: sp.file.clone(),
                argv0: sp.argv0.clone(),
                args: sp.args.clone(),
                cwd: sp.cwd.clone(),
                env: sp.env.clone(),
            })
        }
        Some(why) => {
            text.push_str(&format!("\n\n(No \"Allow for this session\" here: {why}.)"));
            None
        }
    };
    Question { text, session }
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
/// this session" was pressed earlier for this exact run. BLOCKS until the
/// answer: call it from a worker thread, never the main thread.
fn ask_blocking(win: &tauri::Window, kind: Kind, q: Question) -> bool {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // checked under ONE_AT_A_TIME: a spawn queued behind the dialog that
    // just allowed this exact run for the session does not ask again
    if let Some(key) = &q.session {
        if session_allowed(key) {
            eprintln!("[consent] spawn {:?}: allowed for this session, not asked", key.file);
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
                eprintln!("[consent] spawn {:?}: allowed for this session", key.file);
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

/// the scrolled body's fixed size (F2/F6): the dialog never outgrows the screen
#[cfg(target_os = "linux")]
const BODY_W: i32 = 640;
#[cfg(target_os = "linux")]
const BODY_H: i32 = 320;

#[cfg(target_os = "linux")]
fn show_dialog(win: &tauri::Window, kind: Kind, text: &str, session_offered: bool, tx: std::sync::mpsc::Sender<Answer>) {
    use gtk::prelude::*;
    let parent = win.gtk_window().ok();
    let (title, head, no, yes) = match kind {
        Kind::Trust => (TRUST_TITLE, "Turn on community plugins for this vault?", "_Cancel", "_Trust"),
        Kind::Spawn => (SPAWN_TITLE, "A plugin wants to run a program. Read all of it (scroll) before you answer.", "_Deny", "Allow _once"),
    };
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
    buf.set_text(text);
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
    // what the view holds, read back from the widget: the harness asserts on it
    let (s, e) = buf.bounds();
    eprintln!("[consent] dialog text: {:?}", buf.text(&s, &e, false));
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
    // Enter answers Deny/Cancel even though the text view can take focus
    if let Some(b) = d.widget_for_response(gtk::ResponseType::Reject) {
        b.grab_focus();
    }
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
    use std::os::unix::fs::PermissionsExt;

    /// tests that touch the process-wide allowlist (remember/clear) run one at a time
    static SESSION_TESTS: Mutex<()> = Mutex::new(());

    fn sp(file: &str, args: &[&str], cwd: &str) -> Spec {
        Spec { file: file.into(), args: args.iter().map(|s| s.to_string()).collect(), cwd: cwd.into(), env: vec![], argv0: None }
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

    // F3: a program inside the vault (directly, via a symlinked dir, via "..") gets no
    // session button; the same program outside it does
    #[test]
    fn f3_no_session_for_a_program_inside_the_vault() {
        let b = tree("f3");
        let vault = plain(&b.join("vault").canonicalize().unwrap());
        for p in [b.join("vault/fakeprog"), b.join("link/fakeprog"), b.join("bin/../vault/rel/fakeprog")] {
            let q = spawn_question(&vault, &sp(&plain(&p), &[], &vault));
            assert_eq!(q.session, None, "{p:?}");
            assert!(q.text.contains("inside the vault"), "{}", q.text);
            assert!(!q.text.contains("lets exactly this run"), "{}", q.text);
        }
        let q = spawn_question(&vault, &sp(&plain(&b.join("bin/fakeprog")), &[], &vault));
        assert!(q.session.is_some());
        let _ = std::fs::remove_dir_all(&b);
    }

    fn plain(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    // F4: the session key is the exact run: another arg, cwd, argv0 or env var is
    // another key; the dialog says so
    #[test]
    fn f4_the_session_key_is_the_exact_run() {
        let b = tree("f4");
        let prog = plain(&b.join("bin/fakeprog"));
        let base = sp(&prog, &["a"], "/");
        let k = |s: &Spec| spawn_question("/v", s).session.expect("offered");
        let k0 = k(&base);
        assert_eq!(k0, k(&sp(&prog, &["a"], "/")));
        assert_ne!(k0, k(&sp(&prog, &["b"], "/")), "args");
        assert_ne!(k0, k(&sp(&prog, &["a", "b"], "/")), "args");
        assert_ne!(k0, k(&sp(&prog, &["a"], "/tmp")), "cwd");
        let mut e = sp(&prog, &["a"], "/");
        e.env.push(("LD_PRELOAD".into(), "/tmp/x.so".into()));
        assert_ne!(k0, k(&e), "env");
        let mut a0 = sp(&prog, &["a"], "/");
        a0.argv0 = Some("other".into());
        assert_ne!(k0, k(&a0), "argv0");
        assert_ne!(k0, spawn_question("/w", &base).session.unwrap(), "vault");
        assert!(spawn_question("/v", &base).text.contains("this program, these arguments, this working directory and this environment"));
        let _s = SESSION_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        // remembered: the exact run hits, a changed one does not
        let e0 = session().epoch;
        session_remember(k0.clone(), e0);
        assert!(session_allowed(&k0));
        assert!(!session_allowed(&k(&sp(&prog, &["b"], "/"))));
        assert!(!session_allowed(&k(&e)));
        session_clear();
        assert!(!session_allowed(&k0));
        let _ = std::fs::remove_dir_all(&b);
    }

    // F4: shells, interpreters, git and launchers never get the session button —
    // by the path's name, its real name, or the name asked for
    #[test]
    fn f4_no_session_for_shells_interpreters_and_git() {
        let b = tree("f4r");
        for n in ["sh", "bash", "git", "python3", "python3.12", "python", "node", "perl", "env", "xargs"] {
            let f = b.join("bin").join(n);
            std::fs::copy(b.join("bin/fakeprog"), &f).unwrap();
            let q = spawn_question("/v", &sp(&plain(&f), &["-c", "x"], "/"));
            assert_eq!(q.session, None, "{n}");
            assert!(q.text.contains("shell, an interpreter, git"), "{}", q.text);
        }
        let ln = b.join("bin/innocent");
        std::os::unix::fs::symlink(b.join("bin/bash"), &ln).unwrap();
        assert_eq!(spawn_question("/v", &sp(&plain(&ln), &[], "/")).session, None, "symlink to bash");
        let mut a0 = sp(&plain(&b.join("bin/fakeprog")), &[], "/");
        a0.argv0 = Some("git".into());
        assert_eq!(spawn_question("/v", &a0).session, None, "asked for git");
        for n in ["pythonista", "nodes", "gitk", "shasum", "envoy"] {
            assert!(!is_runner(n), "{n}");
        }
        let _ = std::fs::remove_dir_all(&b);
    }

    // G3: the desktop openers run whatever their target maps to — never a session answer
    #[test]
    fn g3_no_session_for_desktop_openers() {
        let b = tree("g3o");
        for n in ["xdg-open", "gio", "kde-open", "gtk-launch", "exo-open"] {
            let f = b.join("bin").join(n);
            std::fs::copy(b.join("bin/fakeprog"), &f).unwrap();
            let q = spawn_question("/v", &sp(&plain(&f), &["/v/run.desktop"], "/v"));
            assert_eq!(q.session, None, "{n}");
            assert!(q.text.contains("/v/run.desktop"), "{}", q.text);
        }
        let _ = std::fs::remove_dir_all(&b);
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

    #[test]
    fn no_session_button_for_the_trust_prompt() {
        assert_eq!(trust_question("/v").session, None);
    }

    #[test]
    fn a_clear_forgets_and_a_stale_epoch_is_not_remembered() {
        let k = SessionKey {
            vault: "/consent-test-vault".into(),
            file: format!("/consent-test/{}", std::process::id()),
            argv0: None,
            args: vec![],
            cwd: "/".into(),
            env: vec![],
        };
        let _s = SESSION_TESTS.lock().unwrap_or_else(|e| e.into_inner());
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
