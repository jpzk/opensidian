//! #281 (startup reveal): a window that boots into a vault is created HIDDEN and
//! shown ONCE, when the UI has the restored layout, the vault theme + snippets,
//! the file tree and the restored tabs in (main.js enterVault -> `boot_reveal`).
//! Before this the window was mapped at the config default size and the user
//! watched it fill in part by part (resize, sidebar, tree, ribbon, tabs, editor).
//!
//! Fail open: the UI may never report (a plugin hangs the boot, a script throws,
//! the webview stalls). A backend timer shows the window anyway after
//! [`FALLBACK`] — a hung plugin can delay the reveal by at most that, it can
//! never keep the window blank. Whichever comes first wins; the other is a no-op.
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// the longest the window stays hidden after setup if the UI never reports ready
pub const FALLBACK: Duration = Duration::from_millis(3000);

/// why the window was shown
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// the UI called `boot_reveal` (layout + theme + tree + tabs are in)
    Ready,
    /// [`FALLBACK`] passed first: shown anyway, whatever state the UI is in
    Timeout,
}

/// one reveal per process. `armed` = the window really was created hidden
/// (a vault boot); unarmed (the picker, a switch-started window that has its own
/// path) every `take` is a no-op.
pub struct Gate {
    armed: AtomicBool,
    shown: AtomicBool,
}

impl Gate {
    pub const fn new() -> Self {
        Gate { armed: AtomicBool::new(false), shown: AtomicBool::new(false) }
    }
    pub fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }
    pub fn armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }
    /// true for exactly ONE caller of an armed gate: that caller shows the window
    pub fn take(&self) -> bool {
        self.armed() && !self.shown.swap(true, Ordering::SeqCst)
    }
    pub fn shown(&self) -> bool {
        self.shown.load(Ordering::SeqCst)
    }
}

/// the UI said ready: show now (once)
pub fn ready(g: &Gate, show: impl FnOnce(Why)) -> bool {
    if g.take() {
        show(Why::Ready);
        true
    } else {
        false
    }
}

/// start the fail-open timer: after `after`, if nobody has shown the window, show it
pub fn arm_fallback(g: &'static Gate, after: Duration, show: impl FnOnce(Why) + Send + 'static) {
    std::thread::spawn(move || {
        std::thread::sleep(after);
        if g.take() {
            show(Why::Timeout);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use std::time::Instant;

    fn leak() -> &'static Gate {
        let g: &'static Gate = Box::leak(Box::new(Gate::new()));
        g.arm();
        g
    }

    /// the hung plugin: the UI never reports ready (its boot awaits a plugin's
    /// onload that never settles). The window is still shown, by the timer,
    /// within the bound — never blank forever.
    #[test]
    fn hung_plugin_cannot_keep_window_blank() {
        let g = leak();
        let got: &'static Mutex<Option<(Why, Instant)>> = Box::leak(Box::new(Mutex::new(None)));
        let t0 = Instant::now();
        arm_fallback(g, Duration::from_millis(60), move |w| *got.lock().unwrap() = Some((w, Instant::now())));
        // the UI side: nothing ever calls ready()
        let deadline = t0 + Duration::from_secs(5);
        while got.lock().unwrap().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let (why, at) = got.lock().unwrap().expect("window never shown: a hung plugin kept it blank");
        assert_eq!(why, Why::Timeout);
        assert!(at - t0 >= Duration::from_millis(60), "shown before the bound");
        assert!(g.shown());
    }

    /// the normal path: ready first -> shown once (Ready); the timer firing later shows nothing
    #[test]
    fn ready_wins_and_timer_is_noop() {
        let g = leak();
        let n: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(0)));
        arm_fallback(g, Duration::from_millis(40), move |w| {
            assert_eq!(w, Why::Timeout);
            n.fetch_add(100, Ordering::SeqCst);
        });
        assert!(ready(g, |w| {
            assert_eq!(w, Why::Ready);
            n.fetch_add(1, Ordering::SeqCst);
        }));
        assert!(!ready(g, |_| { n.fetch_add(1, Ordering::SeqCst); }), "second ready showed again");
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(n.load(Ordering::SeqCst), 1, "shown more than once");
    }

    /// an unarmed gate (picker boot, switch-started window): nothing to reveal
    #[test]
    fn unarmed_gate_never_shows() {
        let g = Gate::new();
        assert!(!ready(&g, |_| panic!("showed an unarmed window")));
        assert!(!g.shown());
    }

    /// the production bound itself is finite and short (the user-visible worst case)
    #[test]
    fn fallback_is_bounded() {
        assert!(FALLBACK > Duration::ZERO && FALLBACK <= Duration::from_secs(5), "{FALLBACK:?}");
    }
}
