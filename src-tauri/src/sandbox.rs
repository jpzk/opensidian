// SPDX-License-Identifier: GPL-3.0-or-later
/* Landlock self-sandbox — OFF by default (operator 2026-09-30); OPENSIDIAN_LANDLOCK=1 opts in,
   OPENSIDIAN_NO_LANDLOCK=1 forces it off and wins over the opt-in. When enabled it is applied once in main()
   BEFORE tauri spawns webkit, so every thread/child process inherits it:
   filesystem writes are confined to the vault and the caches webkit/mesa/fontconfig need; the rest of the system is read-only
   and $HOME is not readable as a whole — dir listing only (for the picker),
   plus READ on the three R31.12 drop-source folders (see DROP_READ_DIRS),
   which is what makes drag & drop of an image possible at all.
   FAIL CLOSED (#293): a kernel without Landlock (< 5.13 / LSM disabled) or any enforce error
   with the switch on binds NO vault (boot_may_bind below). Threads restrict only
   themselves, hence "once, at boot" — switching vaults needs a restart.
   FIRST RUN (audit #3): a boot with the switch on but NO vault (no argv, no
   VAULT_DIR, no last vault) has nothing to confine to, so no ruleset is
   built and the boot picker runs unconfined. It must not bind the picked
   vault in THAT process: enforcing later, from the pick_vault command thread,
   would confine that one thread and leave tokio, GTK and webkit unrestricted.
   So pick_vault/create_vault save the choice and RESTART into it: a fresh
   process booted on that vault confines itself before webkit, the normal path.
   The old window hands over with the same switch handshake as "switch vault"
   and exits (main.rs hand_off). The unconfined process never reads or writes
   the vault, with one exception: create_vault makes the new folder and its
   Welcome.md seed before the restart. That is the folder the user asked for.
   boot_pick_must_restart() is that decision as a pure function. */
#[cfg(target_os = "linux")]
use landlock::{
    path_beneath_rules, Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset,
    RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI,
};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// vault root the process is confined to (None = not enforced)
static CONFINED: OnceLock<PathBuf> = OnceLock::new();

pub fn confined_to() -> Option<&'static Path> {
    CONFINED.get().map(|p| p.as_path())
}

/// true when `p` is usable as a vault under the active sandbox
pub fn allows(p: &Path) -> bool {
    match confined_to() {
        None => true,
        Some(root) => p.canonicalize().map(|c| c.starts_with(root)).unwrap_or(false),
    }
}

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).map(PathBuf::from).filter(|p| p.exists())
}

/* R31.12 DROP SOURCES — the sandbox is why drag & drop is not "read the file".
   A drop hands us a path the user picked in a file manager; the landlock
   ruleset is immutable after restrict_self(), so a folder not named HERE can
   never be read later, and ~/Pictures/cat.png fails with EACCES before a
   single byte is copied. /tmp was always RW, which is exactly why a drop test
   whose source lives in /tmp proves nothing about the feature.

   DECISION (narrowest of the three options in the brief): grant READ on the
   three XDG dirs a user actually drags images out of, and nothing more.
   NOT all of $HOME: that hands ~/.ssh, ~/.gnupg, ~/.aws and every
   token-bearing dotfile to a webkit process, and "still better than Obsidian
   1.13.7, which has no sandbox" is not a reason to leak keys.
   COST, stated plainly: a drop from anywhere else — ~/work/shots, ~/tmp, a
   second disk under /mnt — is REFUSED with a permission error, not copied.
   Paths under /media are not granted either; /run/media happens to be
   reachable because /run is already RW for the X11/dbus sockets.
   Widening further is an OPERATOR decision, one line away, deliberately not
   taken here. $HOME gets NO right at all (vaultbleed: a ReadDir rule is recursive
   and listed every other vault); a confined picker lists through the spawner. */
pub const DROP_READ_DIRS: [&str; 3] = ["Pictures", "Downloads", "Desktop"];

/// The read-only half of the ruleset, AS DATA. This kernel has no Landlock
/// (the enforce test below skips), so enforcement cannot be tested here and a
/// green suite proves nothing about it — what CAN be tested is the content of
/// this vector, which is what the kernel would be told. Paths that do not
/// exist are dropped by `path_beneath_rules` (landlock-0.4.7 fs.rs:617 returns
/// None on an unopenable path), so listing a missing ~/Desktop is harmless.
pub fn read_roots(home: &Path) -> Vec<PathBuf> {
    let mut ro: Vec<PathBuf> = ["/usr", "/etc", "/lib", "/lib64", "/bin", "/sbin", "/opt", "/proc", "/sys", "/var/lib"]
        .iter()
        .map(PathBuf::from)
        .collect();
    ro.extend([home.join(".config"), home.join(".local/share"), home.join(".fonts"), home.join(".Xauthority")]);
    // R31.12: drop sources, read-only and enumerated — never $HOME itself
    ro.extend(DROP_READ_DIRS.iter().map(|d| home.join(d)));
    ro.extend(["APPIMAGE", "APPDIR", "XAUTHORITY"].iter().filter_map(|k| env_path(k)));
    if let Ok(exe) = std::env::current_exe() {
        ro.push(exe);
    }
    ro
}

/// The read-write half, as data for the same reason. A drop source is NEVER in
/// here: opensidian copies out of those folders and never writes into them.
pub fn write_roots(home: &Path, vault: &Path) -> Vec<PathBuf> {
    // webkit/mesa/fontconfig scratch + sockets (X11, wayland, dbus, shm, gpu)
    let mut rw: Vec<PathBuf> = ["/tmp", "/dev", "/run", "/var/tmp"].iter().map(PathBuf::from).collect();
    rw.extend([
        home.join(".cache"),
        home.join(".local/share/dev.koto.opensidian"),
        vault.to_path_buf(),
    ]);
    rw
}

/* S3 — THE SWITCH AND THE RULESET, EACH IN ONE PLACE.
   The env var name was a string literal in TWO files (here and main.rs's
   startup guard), so "off" could have come to mean two different things. It is
   a const now, read through `no_landlock_requested()` by both. */
pub const NO_LANDLOCK_ENV: &str = "OPENSIDIAN_NO_LANDLOCK";

/// The documented off-switch (docs/features.md, req S3). Read it HERE, never
/// by spelling the variable's name a second time somewhere else.
pub fn no_landlock_requested() -> bool {
    std::env::var_os(NO_LANDLOCK_ENV).is_some()
}

/* lloff (operator 2026-09-30, "make landlock default off"): Landlock is OPT-IN.
   OPENSIDIAN_LANDLOCK=1 enables the ruleset above, unchanged. The old off-switch
   keeps its meaning and WINS: both set = no ruleset (dloss/tabclose launches
   set OPENSIDIAN_NO_LANDLOCK and must stay unsandboxed whatever else is set). */
pub const LANDLOCK_ENV: &str = "OPENSIDIAN_LANDLOCK";

/// The one stderr line a launch prints when no ruleset is built (default, or
/// the off-switch). Phase lloff greps for it verbatim.
pub const OFF_LINE: &str = "landlock: off (default; OPENSIDIAN_LANDLOCK=1 enables)";

/// Pure switch resolution, testable without touching process env (see the
/// `ruleset_plan` note on why tests must not `set_var`). `opt_in` / `no` are
/// the raw values of OPENSIDIAN_LANDLOCK / OPENSIDIAN_NO_LANDLOCK.
/// Opt-in means the value "1"; the off-switch is ANY presence, as before.
pub fn landlock_switch(opt_in: Option<&std::ffi::OsStr>, no: Option<&std::ffi::OsStr>) -> bool {
    if no.is_some() {
        return false;
    }
    opt_in.is_some_and(|v| v == "1")
}

/// Should main() build a ruleset? Reads both env vars, spelled once each.
pub fn landlock_enabled() -> bool {
    landlock_switch(
        std::env::var_os(LANDLOCK_ENV).as_deref(),
        std::env::var_os(NO_LANDLOCK_ENV).as_deref(),
    )
}

/// Audit #3, see the module comment. Decides what a boot picker that has no
/// vault bound yet does with the vault the user picked. true means restart
/// into it, because the switch is on and this process is not confined, and
/// binding here would open the vault unconfined. false means bind it here:
/// either the sandbox is off, or this process is already confined.
pub fn boot_pick_must_restart(landlock_on: bool, confined: bool) -> bool {
    landlock_on && !confined
}

/* #293 (audit F-7) FAIL CLOSED. The switch on means the user asked for the sandbox; a boot that
   could not enforce it used to print one stderr line ("landlock: off (...)") and bind the vault
   UNCONFINED — the picker's restart child (audit #3) and every vault-switch child take that same
   boot path, so "never bind unconfined" was defeated exactly where it was promised.
   Now: what enforce() achieved is an `Outcome`, published once (OUTCOME), and `boot_may_bind` is
   the ONE decision every bind consults (main() before bind_vault_boot_after, and claim_vault,
   which every bind goes through). Requested + anything short of the vault rules being enforced
   => Refuse: main() shows `refused_dialog` (Rust text, ONE button: Quit — no "continue
   unconfined") and exits REFUSED_EXIT. The only way past it is not asking for the sandbox
   (no OPENSIDIAN_LANDLOCK=1, or OPENSIDIAN_NO_LANDLOCK=1); there is no in-app switch for it.

   WHAT THE CRATE REPORTS, AND THE ABI/RULE MAPPING (landlock 0.4.7, enforce() below):
     ABI v1 (Linux 5.13) Execute WriteFile ReadFile ReadDir RemoveDir RemoveFile MakeChar MakeDir
       MakeReg MakeSock MakeFifo MakeBlock MakeSym — handled as CompatLevel::HardRequirement, and
       the read rules too: a kernel that cannot enforce any of them makes the crate return Err.
     ABI v2 (Linux 5.19) Refer — the ONLY BestEffort right (R24, on the write roots). Without it
       the kernel refuses every cross-directory rename/link, so its absence is STRICTER, never
       looser: it is the only entry of OPTIONAL_RIGHTS.
     v3+ (Truncate, network, ioctl, scopes) are not handled at all (audit F-16, a separate fix).
   So RulesetStatus::PartiallyEnforced can only mean "a BestEffort right was dropped"; which
   ones is asked of the crate the same way (a HardRequirement ruleset built and dropped per
   right, `kernel_knows`). A partial result whose dropped set is empty (unexplained) or names
   anything outside OPTIONAL_RIGHTS is treated as not enforced. */

/// rights of our ruleset whose absence leaves every filesystem rule we rely on enforced
pub const OPTIONAL_RIGHTS: [&str; 1] = ["refer"];

/// exit status of a boot that refused to bind its vault because the sandbox failed
pub const REFUSED_EXIT: i32 = 78;

/// the dialog's fixed text (#293): the reason and the vault path are the only inserted values
pub const REFUSED_TITLE: &str = "Sandbox unavailable";
pub const REFUSED_HEAD: &str = "The sandbox you turned on could not be enabled on this system, so this vault was not opened.";
pub const REFUSED_HOWTO: &str = "To open this vault without the sandbox, start Opensidian without OPENSIDIAN_LANDLOCK=1 (or with OPENSIDIAN_NO_LANDLOCK=1). Only do that for a vault and plugins you trust.";

/// what `enforce()` achieved, in the terms the bind decision needs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// RulesetStatus::FullyEnforced: every handled right is enforced
    Full,
    /// RulesetStatus::PartiallyEnforced; `dropped` = the rights of our ruleset this kernel lacks
    Partial { dropped: Vec<&'static str> },
    /// RulesetStatus::NotEnforced
    NotEnforced,
    /// enforce() returned Err (no Landlock, LSM off, canonicalize, restrict_self, non-Linux)
    Failed(String),
}

impl Outcome {
    /// main()'s one stderr line, worded as before #293 (phases llpick / lloff read it verbatim)
    pub fn log_line(&self) -> String {
        match self {
            Outcome::Full => "landlock: FullyEnforced".into(),
            Outcome::Partial { .. } => "landlock: PartiallyEnforced".into(),
            Outcome::NotEnforced => "landlock: NotEnforced".into(),
            Outcome::Failed(e) => format!("landlock: off ({e})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootBind {
    Bind,
    Refuse(String),
}

/// #293: may this process bind a vault? `requested` = the switch (landlock_enabled()),
/// `outcome` = what enforce() achieved in THIS process (None = it never ran). Pure.
pub fn boot_may_bind(requested: bool, outcome: Option<&Outcome>) -> BootBind {
    if !requested {
        return BootBind::Bind;
    }
    match outcome {
        Some(Outcome::Full) => BootBind::Bind,
        Some(Outcome::Partial { dropped }) if !dropped.is_empty() && dropped.iter().all(|r| OPTIONAL_RIGHTS.contains(r)) => BootBind::Bind,
        Some(Outcome::Partial { dropped }) if dropped.is_empty() => {
            BootBind::Refuse("Landlock reported a partially enforced ruleset and could not say which rules are missing".into())
        }
        Some(Outcome::Partial { dropped }) => BootBind::Refuse(format!("this kernel does not enforce these Landlock rules: {}", dropped.join(", "))),
        Some(Outcome::NotEnforced) => BootBind::Refuse("Landlock reported that no rule was enforced".into()),
        Some(Outcome::Failed(e)) => BootBind::Refuse(format!("Landlock could not be enforced: {e}")),
        None => BootBind::Refuse("no Landlock ruleset was applied to this process".into()),
    }
}

static OUTCOME: OnceLock<Outcome> = OnceLock::new();

/// what enforce() achieved in this process (None = it never ran)
pub fn outcome() -> Option<&'static Outcome> {
    OUTCOME.get()
}

/// main()'s one call: classify enforce()'s answer (`outcome_of`) and publish it, once
pub fn record(r: Result<RulesetStatus, Box<dyn std::error::Error>>) -> &'static Outcome {
    let o = outcome_of(r, dropped_rights);
    let _ = OUTCOME.set(o);
    OUTCOME.get().expect("set above")
}

/// enforce()'s answer as an Outcome; `dropped` is asked only for a partial result. Pure but
/// for the callback (the kernel probe in production, a fixed list in the tests).
pub fn outcome_of(r: Result<RulesetStatus, Box<dyn std::error::Error>>, dropped: impl FnOnce() -> Vec<&'static str>) -> Outcome {
    match r {
        Err(e) => Outcome::Failed(e.to_string()),
        Ok(s) => status_outcome(s, dropped),
    }
}

#[cfg(target_os = "linux")]
fn status_outcome(s: RulesetStatus, dropped: impl FnOnce() -> Vec<&'static str>) -> Outcome {
    match s {
        RulesetStatus::FullyEnforced => Outcome::Full,
        RulesetStatus::PartiallyEnforced => Outcome::Partial { dropped: dropped() },
        RulesetStatus::NotEnforced => Outcome::NotEnforced,
    }
}

#[cfg(not(target_os = "linux"))]
fn status_outcome(_s: RulesetStatus, _dropped: impl FnOnce() -> Vec<&'static str>) -> Outcome {
    Outcome::NotEnforced
}

/// Does the running kernel enforce `a`? Asked as a HardRequirement ruleset that is built and
/// dropped — nothing is restricted by it (the same probe the R24 test uses for REFER).
#[cfg(target_os = "linux")]
fn kernel_knows(a: AccessFs) -> bool {
    Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(a)
        .and_then(|r| r.create())
        .is_ok()
}

/// the BestEffort rights of enforce()'s ruleset this kernel does not enforce
#[cfg(target_os = "linux")]
fn dropped_rights() -> Vec<&'static str> {
    let mut v = Vec::new();
    if !kernel_knows(AccessFs::Refer) {
        v.push("refer");
    }
    v
}

#[cfg(not(target_os = "linux"))]
fn dropped_rights() -> Vec<&'static str> {
    Vec::new()
}

/// Settings › Security (#293): "off" (not requested), "enforced", or "not-enforced", from THIS
/// process's switch and outcome; `detail` names what is missing / why.
pub fn state_json() -> serde_json::Value {
    let requested = landlock_enabled();
    let (state, detail) = state_of(requested, outcome());
    serde_json::json!({ "state": state, "detail": detail })
}

/// pure half of state_json
pub fn state_of(requested: bool, o: Option<&Outcome>) -> (&'static str, String) {
    if !requested {
        return ("off", String::new());
    }
    match boot_may_bind(true, o) {
        BootBind::Bind => match o {
            Some(Outcome::Partial { dropped }) => ("enforced", format!("without: {}", dropped.join(", "))),
            _ => ("enforced", String::new()),
        },
        BootBind::Refuse(why) => ("not-enforced", why),
    }
}

/// the dialog body: reason, vault, how to turn the sandbox off (all text but `why`/`vault` fixed)
pub fn refused_text(why: &str, vault: &Path) -> String {
    format!("Reason: {why}\n\nVault: {}\n\n{REFUSED_HOWTO}", vault.display())
}

/// #293: the boot refused to bind `vault` — say so in a BLOCKING native dialog with ONE button
/// (Quit; there is no way to continue from here), then exit REFUSED_EXIT. Runs before any
/// window or webview exists. No display => stderr only, same exit.
pub fn refuse_and_exit(why: &str, vault: &Path) -> ! {
    let body = refused_text(why, vault);
    eprintln!("[sandbox] refused: {REFUSED_HEAD} {body:?} (exit {REFUSED_EXIT})");
    refused_dialog(&body);
    eprintln!("[sandbox] refused: exiting {REFUSED_EXIT}, vault not bound");
    std::process::exit(REFUSED_EXIT)
}

#[cfg(target_os = "linux")]
fn refused_dialog(body: &str) {
    use gtk::prelude::*;
    if gtk::init().is_err() {
        eprintln!("[sandbox] refused: no display for the dialog");
        return;
    }
    let d = gtk::MessageDialog::new(None::<&gtk::Window>, gtk::DialogFlags::MODAL, gtk::MessageType::Error, gtk::ButtonsType::None, REFUSED_HEAD);
    d.set_title(REFUSED_TITLE);
    d.set_secondary_text(Some(body));
    let _ = d.add_button("_Quit", gtk::ResponseType::Close);
    d.set_default_response(gtk::ResponseType::Close);
    d.set_keep_above(true);
    eprintln!("[sandbox] refused dialog shown");
    let r = d.run();
    eprintln!("[sandbox] refused dialog closed ({r:?})");
    // SAFETY: the dialog is ours alone; nothing else holds it after run() returned
    unsafe { d.destroy() };
    while gtk::events_pending() {
        gtk::main_iteration();
    }
}

#[cfg(not(target_os = "linux"))]
fn refused_dialog(_body: &str) {}

/// The ruleset opensidian hands the kernel, AS DATA — the three vectors and the
/// access class each is granted. `enforce()` below builds its rules from THIS
/// value and nothing else, so a test that reads a plan reads what the kernel
/// would be told, on a kernel that can enforce it and on this one, which
/// cannot.
#[derive(Debug, PartialEq, Eq)]
pub struct RulesetPlan {
    /// `AccessFs::from_read` — readable, never writable
    pub read: Vec<PathBuf>,
    /// `AccessFs::from_all` — read AND write
    pub write: Vec<PathBuf>,
    /// `AccessFs::ReadDir` only — the picker lists these, it cannot read a file
    pub list_only: Vec<PathBuf>,
}

/// `None` = NO RULESET IS BUILT AT ALL, i.e. the process stays unsandboxed.
/// That is what `OPENSIDIAN_NO_LANDLOCK=1` buys, and it is the whole meaning of
/// the switch: not "a looser ruleset", but no ruleset.
///
/// `disabled` is a PARAMETER rather than an env read so both branches are
/// testable without mutating process-global env — cargo runs this binary's
/// tests as threads in ONE process, so a `set_var` here would race
/// `confines_reads_to_vault` (which calls `enforce`) and make the suite
/// flaky-green. The env read itself is `no_landlock_requested()`, one line.
pub fn ruleset_plan(disabled: bool, home: &Path, vault: &Path) -> Option<RulesetPlan> {
    if disabled {
        return None;
    }
    Some(RulesetPlan {
        read: read_roots(home),
        write: write_roots(home, vault),
        // vaultbleed: NOTHING list-only. A ReadDir rule on $HOME is inherited
        // by every directory beneath it, i.e. it listed every other vault; a
        // confined window's picker lists through the spawner instead
        list_only: vec![],
    })
}

/// Non-Linux targets have no Landlock: the stand-in status type keeps callers and
/// tests compiling, and `enforce` always reports unsupported, which main() logs as
/// "landlock: off (...)" and the tests treat as "skipped".
#[cfg(not(target_os = "linux"))]
#[derive(Debug, PartialEq, Eq)]
pub enum RulesetStatus {
    NotEnforced,
}

#[cfg(not(target_os = "linux"))]
pub fn enforce(_vault: &Path) -> Result<RulesetStatus, Box<dyn std::error::Error>> {
    Err("Landlock is Linux-only; not available on this OS".into())
}

#[cfg(target_os = "linux")]
pub fn enforce(vault: &Path) -> Result<RulesetStatus, Box<dyn std::error::Error>> {
    let off = no_landlock_requested();
    if off {
        return Ok(RulesetStatus::NotEnforced);
    }
    let vault = vault.canonicalize()?;
    let home = env_path("HOME").unwrap_or_else(|| PathBuf::from("/"));
    let abi = ABI::V1;
    /* R24 — MOVING A FILE BETWEEN TWO DIRECTORIES IS A LANDLOCK RIGHT OF ITS OWN.
       ABI v1 has no REFER, and a v1 ruleset denies EVERY cross-directory
       rename(2)/link(2) unconditionally — no rule can permit it. The kernel
       answers EXDEV, "Invalid cross-device link", which is precisely what the
       explorer's Delete hit on the gate box (census
       [notice:Invalid cross-device link (os error 18)], 2026-09-16): moving
       vault/ZF-Doomed.md to vault/.trash/ZF-Doomed.md was refused inside our
       own sandbox with BOTH ends in the same granted hierarchy, one mount, one
       device. R24.6's drag-to-move and any rename into another folder are the
       same call and were broken the same way. So REFER is handled, and granted
       on the WRITE roots only.

       BEST EFFORT, unlike the v1 base above (which stays a HardRequirement):
       REFER arrived with ABI v2 / Linux 5.19. On an older kernel the right is
       dropped and the process gets exactly today's ruleset — cross-directory
       moves keep failing, visibly and with the message above, rather than the
       app refusing to start.

       IT HANDS THE PROCESS NO NEW REACH. REFER permits a rename only between
       two hierarchies that BOTH grant it, i.e. write root to write root; every
       one of those is already readable and writable, where copy+unlink was
       always available. The read-only roots keep from_read() and gain nothing,
       so no file can be moved INTO or OUT OF them — a webkit that wanted
       ~/Pictures/cat.png in the vault must still copy it, and cannot touch
       anything outside the write set at all. */
    let refer = AccessFs::Refer;
    // ONE source of truth: the vectors below are the tested ones, or the tests
    // are testing a ruleset the kernel never sees.
    let plan = match ruleset_plan(off, &home, &vault) {
        Some(p) => p,
        None => return Ok(RulesetStatus::NotEnforced),
    };
    for d in [home.join(".cache"), home.join(".local/share/dev.koto.opensidian")] {
        let _ = std::fs::create_dir_all(d);
    }
    let mut created = Ruleset::default()
        .set_compatibility(CompatLevel::HardRequirement)
        .handle_access(AccessFs::from_all(abi))?
        .set_compatibility(CompatLevel::BestEffort)
        .handle_access(refer)?
        .create()?
        .set_compatibility(CompatLevel::HardRequirement)
        .add_rules(path_beneath_rules(&plan.read, AccessFs::from_read(abi)))?
        .set_compatibility(CompatLevel::BestEffort)
        .add_rules(path_beneath_rules(&plan.write, AccessFs::from_all(abi) | refer))?
        .set_compatibility(CompatLevel::HardRequirement);
    for d in &plan.list_only {
        created = created.add_rule(PathBeneath::new(PathFd::new(d)?, AccessFs::ReadDir))?;
    }
    let status = created.restrict_self()?;
    if status.ruleset != RulesetStatus::NotEnforced {
        let _ = CONFINED.set(vault);
    }
    Ok(status.ruleset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /* R31.12 THE RULESET CONTENT IS THE ONLY TESTABLE PART HERE. This kernel
       has no Landlock, so `confines_reads_to_vault` below SKIPS and a green
       suite says nothing about enforcement. These three tests assert what the
       kernel would be told instead — the vectors themselves — so the drop
       feature cannot be quietly un-shipped by deleting one line from
       read_roots(), and all of $HOME cannot be quietly granted either. */

    /// the drop source folders are READABLE, or drag & drop is dead on arrival
    /// for every real user (and only /tmp, already RW, would keep working).
    #[test]
    fn ro_vec_grants_read_on_the_drop_source_dirs() {
        let home = Path::new("/home/u");
        let ro = read_roots(home);
        for d in DROP_READ_DIRS {
            assert!(ro.contains(&home.join(d)), "read_roots is missing ~/{d}: {ro:?}");
        }
        assert_eq!(DROP_READ_DIRS.to_vec(), vec!["Pictures", "Downloads", "Desktop"]);
    }

    /// ...and NOTHING more: option (b) of the brief (read all of $HOME) is the
    /// one that must stay un-taken, because it hands ~/.ssh to webkit.
    #[test]
    fn ro_vec_does_not_widen_to_all_of_home() {
        let home = Path::new("/home/u");
        let (ro, rw) = (read_roots(home), write_roots(home, Path::new("/home/u/vault")));
        assert!(!ro.contains(&home.to_path_buf()), "$HOME itself must never be readable: {ro:?}");
        // no granted root may be an ancestor of a secret-bearing dotfile
        for dotfile in [".ssh/id_ed25519", ".gnupg/secring.gpg", ".aws/credentials", ".netrc", ".bash_history", ".mozilla/firefox"] {
            let p = home.join(dotfile);
            for (label, v) in [("ro", &ro), ("rw", &rw)] {
                assert!(!v.iter().any(|r| p.starts_with(r)), "{label} grants ~/{dotfile} via one of {v:?}");
            }
        }
        // /home and / must not appear by accident either
        for wide in ["/", "/home", "/home/u", "/root", "/mnt", "/media"] {
            assert!(!ro.contains(&PathBuf::from(wide)), "ro must not contain {wide}");
            assert!(!rw.contains(&PathBuf::from(wide)), "rw must not contain {wide}");
        }
    }

    /// a drop READS its source and WRITES only into the vault: the source dirs
    /// must not be writable, and the vault + config must be.
    #[test]
    fn rw_vec_is_the_vault_not_the_drop_sources() {
        let home = Path::new("/home/u");
        let (vault, cfg) = (home.join("vault"), home.join(".opensidian.json"));
        let rw = write_roots(home, &vault);
        // vaultbleed D: the config is NOT a rule (inode-bound rule vs atomic rename) —
        // a confined window reaches it only through the unconfined spawner
        assert!(rw.contains(&vault) && !rw.contains(&cfg), "{rw:?}");
        for d in DROP_READ_DIRS {
            assert!(!rw.contains(&home.join(d)), "~/{d} must be READ-only: {rw:?}");
        }
    }

    /* S3 — THE RULESET PLAN. The three tests above are R31.12's (the drop
       sources). These four are S3 itself: the switch documented in
       docs/features.md, and the shape of the ruleset `enforce()` actually
       hands the kernel. They assert DATA, which is all that can be asserted on
       a kernel with no Landlock — see the module header and progress.md. */

    /// `OPENSIDIAN_NO_LANDLOCK=1` means NO RULESET, not a looser one. If this
    /// ever returns Some, the switch has quietly become a no-op and the app
    /// would sandbox itself in the very configuration documented as "off".
    #[test]
    fn ruleset_plan_is_none_when_the_no_landlock_switch_is_set() {
        let (home, vault) = (Path::new("/home/u"), Path::new("/home/u/vault"));
        assert_eq!(ruleset_plan(true, home, vault), None, "the off-switch must build no ruleset at all");
        assert!(ruleset_plan(false, home, vault).is_some(), "switch not set to off: a ruleset must be built (main() only calls this when landlock_enabled())");
        // ...and the switch is that variable, spelled once (main.rs reads it
        // through no_landlock_requested(), features.md names the same string).
        assert_eq!(NO_LANDLOCK_ENV, "OPENSIDIAN_NO_LANDLOCK");
    }

    /* lloff — the switch resolution main() uses (operator 2026-09-30). */

    /// DEFAULT IS OFF: with neither variable set, no ruleset is built.
    #[test]
    fn landlock_switch_default_is_off() {
        assert!(!landlock_switch(None, None), "no env -> landlock must be OFF");
        assert_eq!(LANDLOCK_ENV, "OPENSIDIAN_LANDLOCK");
        assert_eq!(OFF_LINE, "landlock: off (default; OPENSIDIAN_LANDLOCK=1 enables)");
    }

    /// OPENSIDIAN_LANDLOCK=1 opts in; any other value does not.
    #[test]
    fn landlock_switch_opt_in_is_exactly_1() {
        use std::ffi::OsStr;
        assert!(landlock_switch(Some(OsStr::new("1")), None), "OPENSIDIAN_LANDLOCK=1 must enable");
        for v in ["", "0", "true", "yes", "2"] {
            assert!(!landlock_switch(Some(OsStr::new(v)), None), "OPENSIDIAN_LANDLOCK={v:?} must not enable");
        }
    }

    /// OPENSIDIAN_NO_LANDLOCK (any value, even empty) WINS over the opt-in.
    #[test]
    fn landlock_switch_no_landlock_wins() {
        use std::ffi::OsStr;
        for n in ["1", "", "0"] {
            assert!(!landlock_switch(Some(OsStr::new("1")), Some(OsStr::new(n))), "NO_LANDLOCK={n:?} must win over LANDLOCK=1");
            assert!(!landlock_switch(None, Some(OsStr::new(n))));
        }
    }

    /// the plan IS the ruleset: what a test reads must be what `enforce()`
    /// hands the kernel, or these tests guard a vector nobody applies.
    #[test]
    fn ruleset_plan_is_built_from_the_same_vectors_enforce_applies() {
        let (home, vault) = (Path::new("/home/u"), Path::new("/home/u/vault"));
        let p = ruleset_plan(false, home, vault).expect("switch is off");
        assert_eq!(p.read, read_roots(home), "plan.read must BE read_roots()");
        assert_eq!(p.write, write_roots(home, vault), "plan.write must BE write_roots()");
        // vaultbleed crit 5: NO list-only $HOME — ReadDir is inherited by every
        // directory beneath, so it listed every other vault; the picker of a
        // confined window lists through the spawner (Req::ListDirs)
        assert!(p.list_only.is_empty(), "{:?}", p.list_only);
        assert!(!p.read.contains(&home.to_path_buf()) && !p.write.contains(&home.to_path_buf()));
    }

    /// S3's actual invariant: WRITES are confined to the vault, its config and
    /// named scratch. Nothing writable may be an ancestor of $HOME or of the
    /// system — a widening here is the one that turns the sandbox into
    /// decoration, and it is the mutation in docs/negctl-lands control B.
    #[test]
    fn ruleset_plan_confines_writes_to_the_vault_and_named_scratch() {
        let (home, vault, cfg) = (Path::new("/home/u"), Path::new("/home/u/vault"), Path::new("/home/u/.opensidian.json"));
        let w = ruleset_plan(false, home, vault).expect("switch is off").write;
        assert!(w.contains(&vault.to_path_buf()) && !w.contains(&cfg.to_path_buf()), "{w:?}");
        let allowed: Vec<PathBuf> = ["/tmp", "/dev", "/run", "/var/tmp"]
            .iter()
            .map(PathBuf::from)
            .chain([home.join(".cache"), home.join(".local/share/dev.koto.opensidian"), vault.to_path_buf()])
            .collect();
        assert_eq!(w, allowed, "the writable set grew or shrank — say so in features.md before changing it");
        // no writable root may CONTAIN the home dir, the vault's parent or /
        for wide in ["/", "/home", "/home/u", "/usr", "/etc", "/var", "/var/lib", "/opt"] {
            assert!(!w.contains(&PathBuf::from(wide)), "{wide} must not be writable: {w:?}");
        }
        for root in &w {
            assert!(!home.starts_with(root), "{root:?} is an ancestor of $HOME — every dotfile is writable through it");
        }
    }

    /// the read half may not overlap the write half's intent: a path granted
    /// read-only must not also appear in the writable set (that silently
    /// upgrades it to RW, since landlock unions the rules for a path).
    #[test]
    fn ruleset_plan_read_only_roots_are_not_also_writable() {
        let (home, vault) = (Path::new("/home/u"), Path::new("/home/u/vault"));
        let p = ruleset_plan(false, home, vault).expect("switch is off");
        for r in &p.read {
            assert!(!p.write.contains(r), "{r:?} is in BOTH halves — read-only is a lie for it");
        }
        for d in DROP_READ_DIRS {
            assert!(p.read.contains(&home.join(d)) && !p.write.contains(&home.join(d)), "~/{d}");
        }
    }

    /* ENFORCEMENT IS PROCESS STATE, SO EACH ENFORCING TEST GETS A PROCESS.
       `CONFINED` is a OnceLock and `restrict_self()` cannot be undone, so two
       tests that both call enforce() inside ONE test binary cannot both be
       right: whichever thread gets there first owns CONFINED, and `allows()`
       then answers about the other test's vault. That is not hypothetical —
       adding the R24 move test below turned `confines_reads_to_vault` red on
       the box at 3dab929 (17:50: left (.., false, false), right (.., true,
       false)) while every unsandboxed test stayed green. A #[test] that
       mutates global state must therefore not share one.

       `reexec_alone` re-runs THIS binary for the single named test: one test,
       one process, one ruleset, and the child's panic text is inherited so the
       failure reads the same as an ordinary one. Guarded by an env var, so the
       child runs the body instead of spawning a grandchild.

       THE SAME TRAP, THE SAME ANSWER, SECOND TIME: `perf::tests::
       otel_sink_survives_the_sandbox_that_kills_a_path_open` (perf.rs:1094)
       hit this exact collision earlier and re-execs itself the same way — its
       comment records the identical left/right mismatch. This helper is that
       precedent generalised for the two tests in this module; a third caller
       of enforce() must use one of them, not invent a third copy. */
    fn reexec_alone(name: &str) -> bool {
        if std::env::var_os("OPENSIDIAN_LL_ALONE").is_some() {
            return false; // we ARE the child: run the body
        }
        let exe = std::env::current_exe().expect("the test binary's own path");
        let st = std::process::Command::new(exe)
            .args([name, "--exact", "--nocapture", "--test-threads=1"])
            .env("OPENSIDIAN_LL_ALONE", "1")
            .status()
            .expect("re-exec the test binary");
        assert!(st.success(), "{name} FAILED in its own process (its output is above)");
        true
    }

    /// restrict_self is per-thread: enforce + probe in a child thread, the
    /// unrestricted parent cleans up. Skips on kernels without Landlock.
    #[cfg(unix)]
    #[test]
    fn confines_reads_to_vault() {
        if reexec_alone("sandbox::tests::confines_reads_to_vault") {
            return;
        }
        let home = env_path("HOME").expect("HOME");
        let tmp = std::env::temp_dir().join(format!("opensidian-ll-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("vault")).unwrap();
        let probe = home.join(format!(".opensidian-probe-{}", std::process::id()));
        fs::write(&probe, "secret").unwrap();
        let (vault, probe2) = (tmp.join("vault"), probe.clone());
        let res = std::thread::spawn(move || {
            if enforce(&vault).is_err() {
                return None; // kernel without landlock (e.g. this firecracker guest)
            }
            fs::write(vault.join("a.md"), "x").expect("vault writable");
            let read_ok = fs::read(&probe2).is_ok();
            let home_write_ok = fs::write(home.join(".opensidian-probe-w"), "x").is_ok();
            let etc_ok = fs::read_dir("/etc").is_ok();
            Some((read_ok, home_write_ok, etc_ok, allows(&vault), allows(&home)))
        })
        .join()
        .unwrap();
        let _ = fs::remove_file(&probe);
        let _ = fs::remove_dir_all(&tmp);
        match res {
            None => eprintln!("landlock unsupported here — test skipped"),
            Some(r) => assert_eq!(r, (false, false, true, true, false)),
        }
    }

    /* vaultbleed D — THE SPAWNER UNDER A REAL RULESET. The serving thread is
       started BEFORE the confined thread restricts itself (restrict_self is
       per-thread), so it stands where the real spawner process stands:
       outside the domain. Everything lives under $HOME (no rights), NOT /tmp
       (a write root, where nothing would be refused and nothing proved). */
    struct TestSpawner {
        cfg: PathBuf,
        locks: PathBuf,
    }
    impl crate::spawner::Handler for TestSpawner {
        fn open(&self, p: &Path, _sw: Option<&crate::spawn::Switch>) -> Result<u32, String> {
            // the spawner can see what the confined window cannot
            fs::read(p.join("secret.md")).map(|_| 7).map_err(|e| e.to_string())
        }
        fn create(&self, parent: &str, name: &str) -> Result<PathBuf, String> {
            let p = Path::new(parent).join(name);
            fs::create_dir(&p).map(|_| p).map_err(|e| e.to_string())
        }
        fn cfg(&self, ops: &[crate::cfgstore::Op]) -> Result<(), String> {
            crate::cfgstore::update_in(&self.locks, &self.cfg, ops).map(|_| ())
        }
        fn read_cfg(&self) -> serde_json::Value {
            crate::cfgstore::read_value_in(&self.locks, &self.cfg)
        }
        fn list_dirs(&self, path: &str) -> Vec<String> {
            let mut v: Vec<String> = fs::read_dir(path).into_iter().flatten().flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
            v.sort();
            v
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_confined_window_reaches_the_config_and_other_vaults_only_through_the_spawner() {
        if reexec_alone("sandbox::tests::a_confined_window_reaches_the_config_and_other_vaults_only_through_the_spawner") {
            return;
        }
        use crate::cfgstore::Op;
        let home = env_path("HOME").expect("HOME");
        let base = home.join(format!(".opensidian-ll-sp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (a, b) = (base.join("A"), base.join("B"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(b.join("secret.md"), "B's note").unwrap();
        let (cfg, locks) = (base.join(".opensidian.json"), base.join("locks"));
        // an unconfined window wrote the config first: rename -> a fresh inode
        crate::cfgstore::update_in(&locks, &cfg, &[Op::PushRecent(a.display().to_string())]).unwrap();
        let (mine, theirs) = std::os::unix::net::UnixStream::pair().unwrap();
        let h = TestSpawner { cfg: cfg.clone(), locks };
        let srv = std::thread::spawn(move || {
            crate::spawner::serve(std::io::BufReader::new(theirs.try_clone().unwrap()), theirs, &h)
        });
        let (a2, b2, cfg2, base2) = (a.clone(), b.clone(), cfg.clone(), base.clone());
        let res = std::thread::spawn(move || {
            let c = crate::spawner::Client::new(mine).unwrap();
            match enforce(&a2) {
                Err(_) | Ok(RulesetStatus::NotEnforced) => return None,
                Ok(_) => {}
            }
            let errno = |r: std::io::Result<()>| r.err().and_then(|e| e.raw_os_error());
            fs::write(a2.join("own.md"), "x").expect("own vault writable");
            let direct = (
                errno(fs::read(b2.join("secret.md")).map(|_| ())),
                errno(fs::write(b2.join("bleed.md"), "x")),
                errno(fs::read(&cfg2).map(|_| ())),
                errno(fs::write(&cfg2, "{}")),
                // crit 5: the other vault's DIRECTORY, and $HOME above it
                errno(fs::read_dir(&b2).map(|_| ())),
                errno(fs::read_dir(&base2).map(|_| ())),
            );
            let via = (
                c.open(&b2, None),
                c.cfg(&[Op::PushRecent(b2.display().to_string())]),
                c.read_cfg().map(|v| v["last"].clone()),
                c.create(&base2.display().to_string(), "C").is_ok(),
                c.list_dirs(&b2.display().to_string()),
            );
            Some((direct, via))
        })
        .join()
        .unwrap();
        srv.join().unwrap(); // the confined end dropped -> EOF -> served out
        let c_made = base.join("C").is_dir();
        let bleed = b.join("bleed.md").exists();
        let _ = fs::remove_dir_all(&base);
        match res {
            None => eprintln!("landlock unsupported here — test skipped"),
            Some((direct, via)) => {
                eprintln!("[spawner-ll] direct errno (read B, write B, read cfg, write cfg, list B, list base) = {direct:?}");
                assert_eq!(direct, (Some(13), Some(13), Some(13), Some(13), Some(13), Some(13)), "a confined window must get EACCES on the other vault (files AND its directory), the config, and listing above it");
                assert!(!bleed);
                assert_eq!(via.0, Ok(7), "open of another vault via the spawner");
                assert_eq!(via.1, Ok(()), "config write via the spawner");
                assert_eq!(via.2, Ok(serde_json::json!(b.display().to_string())), "config read via the spawner sees the write");
                assert!(via.3 && c_made, "create-vault via the spawner");
                assert_eq!(via.4, Ok(vec!["secret.md".to_string()]), "the picker lists through the spawner");
            }
        }
    }

    /// true when the running kernel knows REFER (ABI v2, Linux 5.19). Asked as
    /// a HardRequirement ruleset that is BUILT AND DROPPED — nothing is
    /// restricted by it, so the caller is as unconfined afterwards as before.
    #[cfg(target_os = "linux")] // landlock types: Linux only
    fn kernel_has_refer() -> bool {
        Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::Refer)
            .and_then(|r| r.create())
            .is_ok()
    }

    /// R24 REGRESSION: the vault's own `.trash` is one rename(2) away, and
    /// under an ABI v1 ruleset the kernel refuses it with EXDEV no matter what
    /// the rules say — which is how the explorer's Delete shipped broken and
    /// green (the phase never reached Confirm until 2026-09-16; the unit tests
    /// below run UNSANDBOXED and cannot see it). So the assertion lives here,
    /// inside the ruleset, where the failure actually was.
    ///
    /// Three outcomes, each distinct on purpose: no landlock -> skip (nothing
    /// is enforced, nothing is proved); landlock without REFER -> skip, naming
    /// the kernel, because on such a kernel the move CANNOT be permitted;
    /// landlock with REFER -> the move must succeed, and an EXDEV here is the
    /// grant having been dropped from enforce().
    #[test]
    #[cfg(target_os = "linux")] // uses kernel_has_refer (landlock)
    fn a_move_into_the_vault_trash_survives_our_own_sandbox() {
        if reexec_alone("sandbox::tests::a_move_into_the_vault_trash_survives_our_own_sandbox") {
            return;
        }
        let refer = kernel_has_refer();
        let tmp = std::env::temp_dir().join(format!("opensidian-ll-mv-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("vault/.trash")).unwrap();
        fs::create_dir_all(tmp.join("vault/sub")).unwrap();
        let vault = tmp.join("vault");
        let home = env_path("HOME").expect("HOME");
        let res = std::thread::spawn(move || {
            if enforce(&vault).is_err() {
                return None; // kernel without landlock
            }
            fs::write(vault.join("N.md"), "x").expect("vault writable");
            // R24.7 delete: vault root -> .trash. R24.6 move: root -> subfolder.
            let trashed = fs::rename(vault.join("N.md"), vault.join(".trash/N.md"));
            fs::write(vault.join("M.md"), "y").expect("vault writable");
            let moved = fs::rename(vault.join("M.md"), vault.join("sub/M.md"));
            // and the confinement is UNCHANGED: REFER may not carry a file into
            // a hierarchy that does not grant it. $HOME has no right at all, so a
            // rename into it must still be refused — with EACCES, not EXDEV.
            // (NOT /tmp as the target: /tmp is itself a write root, where a
            // copy was always permitted, and a move that lands there proves
            // nothing about the boundary.)
            let escaped = fs::rename(vault.join(".trash/N.md"), home.join("escaped.md"));
            Some((
                trashed.as_ref().err().and_then(|e| e.raw_os_error()),
                moved.as_ref().err().and_then(|e| e.raw_os_error()),
                escaped.is_ok(),
            ))
        })
        .join()
        .unwrap();
        let _ = fs::remove_dir_all(&tmp);
        match (res, refer) {
            (None, _) => eprintln!("landlock unsupported here — test skipped"),
            (Some(_), false) => eprintln!(
                "landlock without REFER (kernel < 5.19) — a cross-directory move cannot be permitted on this kernel; test skipped"
            ),
            (Some((trashed, moved, escaped)), true) => {
                assert_eq!(trashed, None, "moving a note into the vault's .trash was refused by our own ruleset (18 = EXDEV = the REFER grant is gone)");
                assert_eq!(moved, None, "moving a note into a vault subfolder was refused by our own ruleset (18 = EXDEV)");
                assert!(!escaped, "a note was moved OUT of the vault — REFER widened the confinement");
            }
        }
    }

    /* AUDIT #3: the first-run picker. With the switch on and no boot vault,
       the picked vault must be handed to a fresh process. The restarted
       process is already confined, so it binds and does not restart again,
       which means there is no restart loop. With the switch off nothing
       changes. */
    #[test]
    fn appguard_landlock_boot_pick_restarts_only_when_unconfined() {
        assert!(boot_pick_must_restart(true, false), "switch on, unconfined picker: binding here opens the vault unconfined");
        assert!(!boot_pick_must_restart(true, true), "the restarted process is confined: it binds (no restart loop)");
        assert!(!boot_pick_must_restart(false, false), "switch off: the picker binds as before");
        assert!(!boot_pick_must_restart(false, true));
    }

    /* AUDIT #3, what the restart buys. This is the restarted process: its own
       process, booted on the picked vault, with enforce() run before anything
       else. A write inside the vault lands. A write to $HOME, or into another
       vault under $HOME, is refused with EACCES. Before the fix, the picked
       vault was bound in the unconfined picker process, where both writes
       succeed. */
    #[cfg(unix)]
    #[test]
    fn appguard_landlock_restarted_process_cannot_write_outside_vault() {
        if reexec_alone("sandbox::tests::appguard_landlock_restarted_process_cannot_write_outside_vault") {
            return;
        }
        let home = env_path("HOME").expect("HOME");
        let pid = std::process::id();
        let tmp = std::env::temp_dir().join(format!("opensidian-agll-{pid}"));
        let other = home.join(format!("opensidian-agll-other-{pid}"));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("picked")).unwrap();
        fs::create_dir_all(&other).unwrap();
        let (vault, home2, other2) = (tmp.join("picked"), home.clone(), other.clone());
        let res = std::thread::spawn(move || {
            match enforce(&vault) {
                Ok(RulesetStatus::NotEnforced) | Err(_) => return None,
                Ok(s) => eprintln!("appguard_landlock: enforced {s:?}"),
            }
            let inside = fs::write(vault.join("Welcome.md"), "x").map_err(|e| e.raw_os_error());
            let home_w = fs::write(home2.join(".opensidian-agll-w"), "x").map_err(|e| e.raw_os_error());
            let other_w = fs::write(other2.join("Stolen.md"), "x").map_err(|e| e.raw_os_error());
            Some((inside, home_w, other_w))
        })
        .join()
        .unwrap();
        let _ = fs::remove_file(home.join(".opensidian-agll-w"));
        let _ = fs::remove_dir_all(&other);
        let _ = fs::remove_dir_all(&tmp);
        match res {
            None => eprintln!("landlock unsupported here — test skipped"),
            Some((inside, home_w, other_w)) => {
                assert_eq!(inside, Ok(()), "the picked vault must be writable in the restarted process");
                assert_eq!(home_w, Err(Some(13)), "a write to $HOME must be EACCES");
                assert_eq!(other_w, Err(Some(13)), "a write into another vault under $HOME must be EACCES");
            }
        }
    }

    /* #293 (audit F-7): the boot bind decision. Requested + not enforced => Refuse, whatever the
       way it was not enforced; not requested => Bind, whatever enforce() did or did not do. */
    fn refuses(b: BootBind) -> bool {
        matches!(b, BootBind::Refuse(ref why) if !why.is_empty())
    }

    #[test]
    fn ll293_requested_and_enforce_err_refuses() {
        assert!(refuses(boot_may_bind(true, Some(&Outcome::Failed("Landlock is not supported by the running kernel".into())))));
        let o = outcome_of(Err("canonicalize: No such file or directory".into()), || panic!("dropped is asked only for a partial result"));
        assert_eq!(o, Outcome::Failed("canonicalize: No such file or directory".into()));
        match boot_may_bind(true, Some(&o)) {
            BootBind::Refuse(why) => assert!(why.contains("canonicalize"), "the reason names the error: {why}"),
            b => panic!("an enforce error must refuse: {b:?}"),
        }
    }

    #[test]
    fn ll293_requested_and_not_enforced_refuses() {
        assert!(refuses(boot_may_bind(true, Some(&Outcome::NotEnforced))));
        assert_eq!(outcome_of(Ok(RulesetStatus::NotEnforced), || panic!("not partial")), Outcome::NotEnforced);
    }

    #[test]
    fn ll293_requested_and_never_enforced_refuses() {
        // a process that never ran enforce() (e.g. the unconfined first-run picker) binds nothing
        assert!(refuses(boot_may_bind(true, None)));
    }

    #[test]
    fn ll293_requested_and_partial_missing_a_filesystem_rule_refuses() {
        for dropped in [vec!["write_file"], vec!["refer", "read_file"], vec!["make_sym"], vec!["truncate"]] {
            match boot_may_bind(true, Some(&Outcome::Partial { dropped: dropped.clone() })) {
                BootBind::Refuse(why) => assert!(why.contains(dropped.iter().copied().find(|r| !OPTIONAL_RIGHTS.contains(r)).unwrap()), "{why}"),
                b => panic!("partial without {dropped:?} must refuse: {b:?}"),
            }
        }
        // a partial result nobody can explain is not enforced either
        assert!(refuses(boot_may_bind(true, Some(&Outcome::Partial { dropped: vec![] }))));
    }

    #[test]
    #[cfg(target_os = "linux")] // RulesetStatus::PartiallyEnforced is the crate's (Linux only)
    fn ll293_partial_status_asks_which_rights_were_dropped() {
        assert_eq!(outcome_of(Ok(RulesetStatus::PartiallyEnforced), || vec!["refer"]), Outcome::Partial { dropped: vec!["refer"] });
        assert_eq!(outcome_of(Ok(RulesetStatus::PartiallyEnforced), Vec::new), Outcome::Partial { dropped: vec![] });
        assert!(refuses(boot_may_bind(true, Some(&outcome_of(Ok(RulesetStatus::PartiallyEnforced), Vec::new)))));
        assert_eq!(outcome_of(Ok(RulesetStatus::FullyEnforced), || panic!("not partial")), Outcome::Full);
    }

    #[test]
    fn ll293_requested_and_fully_enforced_binds() {
        assert_eq!(boot_may_bind(true, Some(&Outcome::Full)), BootBind::Bind);
        // only REFER missing (ABI v1 kernel): stricter, every v1 rule is enforced
        assert_eq!(boot_may_bind(true, Some(&Outcome::Partial { dropped: vec!["refer"] })), BootBind::Bind);
        assert_eq!(OPTIONAL_RIGHTS, ["refer"], "a new optional right needs a reason in the #293 comment");
    }

    #[test]
    fn ll293_not_requested_binds_whatever_the_outcome() {
        for o in [None, Some(Outcome::Full), Some(Outcome::NotEnforced), Some(Outcome::Failed("x".into())), Some(Outcome::Partial { dropped: vec!["write_file"] })] {
            assert_eq!(boot_may_bind(false, o.as_ref()), BootBind::Bind, "{o:?}");
        }
    }

    #[test]
    fn ll293_settings_state_is_off_enforced_or_not_enforced() {
        assert_eq!(state_of(false, None).0, "off");
        assert_eq!(state_of(false, Some(&Outcome::Full)).0, "off");
        assert_eq!(state_of(true, Some(&Outcome::Full)), ("enforced", String::new()));
        assert_eq!(state_of(true, Some(&Outcome::Partial { dropped: vec!["refer"] })).0, "enforced");
        for o in [None, Some(Outcome::NotEnforced), Some(Outcome::Failed("e".into())), Some(Outcome::Partial { dropped: vec!["write_file"] })] {
            let (s, why) = state_of(true, o.as_ref());
            assert_eq!(s, "not-enforced", "{o:?}");
            assert!(!why.is_empty(), "{o:?}");
        }
    }

    #[test]
    fn ll293_dialog_text_is_fixed_names_the_reason_and_the_way_out() {
        assert_eq!(REFUSED_HEAD, "The sandbox you turned on could not be enabled on this system, so this vault was not opened.");
        let t = refused_text("Landlock could not be enforced: E2BIG", Path::new("/home/u/v"));
        assert!(t.contains("Reason: Landlock could not be enforced: E2BIG") && t.contains("/home/u/v"), "{t}");
        assert!(t.contains(LANDLOCK_ENV) && t.contains(NO_LANDLOCK_ENV), "how to turn it off: {t}");
        assert_ne!(REFUSED_EXIT, 0);
        // fail closed: the dialog offers no way on — one Quit button, nothing that continues
        let src = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/sandbox.rs")).unwrap();
        let d = src.find("\nfn refused_dialog(body: &str) {").expect("refused_dialog");
        let db = &src[d..d + src[d..].find("\n}\n").expect("end")];
        assert_eq!(db.matches(".add_button(").count(), 1, "exactly one button");
        assert!(db.contains(r#".add_button("_Quit", gtk::ResponseType::Close)"#));
        let lower = db.to_lowercase();
        for no in ["continue", "unconfined", "anyway", "accept", "apply"] {
            assert!(!lower.contains(no), "the refused dialog must not offer '{no}'");
        }
        let r = src.find("\npub fn refuse_and_exit(").expect("refuse_and_exit");
        let rb = &src[r..r + src[r..].find("\n}\n").expect("end")];
        assert!(rb.find("refused_dialog(&body)").unwrap() < rb.find("std::process::exit(REFUSED_EXIT)").unwrap(), "exit after the dialog, always");
    }

    /// Settings › Security shows the state Rust reports, in all six locales: every secsb-* key is
    /// in every locale file, the English text is the page's fallback, and the page knows exactly
    /// the three states state_of() can answer
    #[test]
    fn ll293_settings_row_strings_in_every_locale_and_the_page_reads_rust() {
        let keys = ["secsb-section", "secsb-label", "secsb-enforced", "secsb-not-enforced", "secsb-off", "secsb-desc-enforced", "secsb-desc-not-enforced", "secsb-desc-off"];
        let ui = include_str!("../../ui/security.js");
        for lang in ["en", "zh-CN", "ko", "ru", "es", "ja"] {
            let src = crate::i18n::ftl(lang).unwrap_or_else(|| panic!("{lang}"));
            for k in keys {
                let line = src.lines().find(|l| l.starts_with(&format!("{k} = "))).unwrap_or_else(|| panic!("{lang} lacks {k}"));
                let val = &line[k.len() + 3..];
                assert!(!val.trim().is_empty(), "{lang} {k} is empty");
                if lang == "en" {
                    assert!(ui.contains(&format!("\"{k}\", \"{val}\"")), "ui/security.js fallback for {k} is not the English text {val:?}");
                } else if k.starts_with("secsb-desc-") {
                    assert!(val.contains("OPENSIDIAN_LANDLOCK=1") || k == "secsb-desc-not-enforced", "{lang} {k} must name the switch");
                }
            }
        }
        for s in ["enforced", "not-enforced", "off"] {
            assert!(ui.contains(&format!("\n    \"{s}\": [\"secsb-{s}\"")), "the page has no text for state {s}");
        }
        assert!(ui.contains("const S = R && R.sandbox, t = S && SB_TEXT[S.state];"), "the row is drawn from security_get .sandbox");
        assert!(ui.contains("    sandboxRow(pg);\n  }\n  function tok() {"), "build() draws the row");
    }

    /// the boot's stderr line keeps its pre-#293 words: phases llpick / lloff match them verbatim
    #[test]
    fn ll293_log_line_keeps_its_words() {
        assert_eq!(Outcome::Full.log_line(), "landlock: FullyEnforced");
        assert_eq!(Outcome::Partial { dropped: vec!["refer"] }.log_line(), "landlock: PartiallyEnforced");
        assert_eq!(Outcome::NotEnforced.log_line(), "landlock: NotEnforced");
        assert_eq!(Outcome::Failed("E2BIG".into()).log_line(), "landlock: off (E2BIG)");
        assert_ne!(Outcome::Failed("E2BIG".into()).log_line(), OFF_LINE, "a failure never reads like the default off");
    }
}
