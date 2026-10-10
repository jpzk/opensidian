// SPDX-License-Identifier: GPL-3.0-or-later
/* goal winsize — the window rectangle survives a restart, per vault, in
   stock's schema. Stock evidence and the design: the goal's notes/stock.md and
   notes/parity.md §1 (not in this repo); the decisions, in short:
     * WHERE: ~/.opensidian.json "windows": { <canonical vault path>: record }.
       Stock keeps the same record in <config>/obsidian/<vault-id>.json; ids only
       exist in stock's own registry, so our key is the path. The window rect is
       a property of the MACHINE (R28.2), never of the synced vault.
     * WHAT: stock's record, stock's names — {x, y, width, height, isMaximized}:
       x/y the outer position, width/height the content size, logical px,
       integers. Written with cfgstore::Op::MergeIn, so keys we do not write
       (stock's devTools/zoom, anything newer) survive.
     * WHEN: on a clean close only (CloseRequested, and the old window of a
       vault switch), exactly stock's timing — a SIGKILL loses the change, as
       it does in stock. Maximized saves the NORMAL bounds + isMaximized:true.
     * RESTORE: before the first show, through `sanitise` below. One deliberate
       difference from stock 1.14.4: isMaximized IS restored (the operator's
       brief asks for it; stock writes it and ignores it). A zero size falls back
       to the default (stock: 300x200), the brief's rule.
   Everything that decides is a pure function here, unit-tested without a
   display; main.rs only reads monitors, applies the plan and tracks events. */
use serde_json::{json, Value};

/// the config key: one record per vault
pub const KEY: &str = "windows";
/// stock's minimum (E6: 50x40 came back as 300x200); also tauri.conf minWidth/minHeight
pub const MIN_W: f64 = 300.0;
pub const MIN_H: f64 = 200.0;

/// a rectangle in logical px (a window, or a monitor's work area)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn overlap(&self, o: &Rect) -> f64 {
        let w = (self.x + self.w).min(o.x + o.w) - self.x.max(o.x);
        let h = (self.y + self.h).min(o.y + o.h) - self.y.max(o.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }
}

/// what the window is given before it is shown
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub w: f64,
    pub h: f64,
    /// None = centre it (no saved position, an off-screen one, or Wayland)
    pub pos: Option<(f64, f64)>,
    pub max: bool,
    /// which rule decided — logged as [winsize] so a gate reads it, not infers it
    pub why: &'static str,
}

fn num(rec: &Value, k: &str) -> Result<Option<f64>, ()> {
    match rec.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n.as_f64().map(Some).ok_or(()),
        Some(_) => Err(()),
    }
}

/// record + monitors (work areas, logical px; [0] = primary) -> where the
/// window goes. `default` is the config size. `wayland`: a client cannot
/// position itself, so the position is never applied (size and max are).
pub fn sanitise(rec: Option<&Value>, mons: &[Rect], default: (f64, f64), wayland: bool) -> Plan {
    let full = |why| Plan { w: default.0, h: default.1, pos: None, max: false, why };
    let Some(rec) = rec.filter(|r| r.is_object()) else { return full("no-record") };
    // E7/E9/E10: a field of the wrong type is a record nobody can trust
    let (Ok(x), Ok(y), Ok(w), Ok(h)) = (num(rec, "x"), num(rec, "y"), num(rec, "width"), num(rec, "height")) else {
        return full("bad-type");
    };
    let (Some(mut w), Some(mut h)) = (w, h) else { return full("no-size") };
    // brief: never restore a broken / zero size (stock E4 gives 300x200 here)
    if !(w.is_finite() && h.is_finite()) || w <= 0.0 || h <= 0.0 {
        return full("zero-size");
    }
    let max = rec.get("isMaximized").and_then(Value::as_bool).unwrap_or(false);
    let mut why = "restored";
    // E6: below the minimum is raised to it, at the saved position
    if w < MIN_W || h < MIN_H {
        w = w.max(MIN_W);
        h = h.max(MIN_H);
        why = "min";
    }
    let mut pos = match (x, y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() && !wayland => Some((x, y)),
        _ => None,
    };
    // the monitor this rect is on (most overlap), else the primary
    let mut on = mons.first().copied();
    if let Some((px, py)) = pos {
        let r = Rect { x: px, y: py, w, h };
        let best = mons.iter().map(|m| (r.overlap(m), *m)).max_by(|a, b| a.0.total_cmp(&b.0));
        match best {
            // E1/E3: on no monitor at all -> the full default, so it comes back visible
            Some((a, _)) if a <= 0.0 => return full("off-screen"),
            Some((_, m)) => on = Some(m),
            None => {}
        }
    }
    // E5: larger than the screen -> clamped to it (and pulled onto it)
    if let Some(m) = on {
        if w > m.w || h > m.h {
            w = w.min(m.w);
            h = h.min(m.h);
            pos = pos.map(|(px, py)| (px.clamp(m.x, m.x + m.w - w), py.clamp(m.y, m.y + m.h - h)));
            why = "clamped";
        }
    }
    let pos = pos.map(|(px, py)| (px.round(), py.round()));
    Plan { w: w.round(), h: h.round(), pos, max, why: if pos.is_none() && why == "restored" { "centred" } else { why } }
}

/// the record written at close. Wayland: no position (the compositor owns it),
/// so MergeIn keeps whatever x/y an X11 session saved before.
pub fn record(r: Rect, max: bool, wayland: bool) -> Value {
    let mut v = json!({ "width": r.w.round() as i64, "height": r.h.round() as i64, "isMaximized": max });
    if !wayland {
        v["x"] = json!(r.x.round() as i64);
        v["y"] = json!(r.y.round() as i64);
    }
    v
}

/// The NORMAL bounds: what the window was before it was maximized. Samples are
/// taken from Moved/Resized while the window reports neither maximized nor
/// fullscreen; the state flag and the configure that carries the maximized size
/// can arrive in either order, so `normal` skips a newest sample that already
/// has the maximized size.
#[derive(Debug, Default, Clone)]
pub struct Track {
    ring: Vec<Rect>,
}

impl Track {
    const N: usize = 4;
    pub const fn new() -> Self {
        Track { ring: Vec::new() }
    }
    pub fn see(&mut self, r: Rect) {
        if !(r.w >= 100.0 && r.h >= 100.0) || self.ring.last() == Some(&r) {
            return; // tao's 1x1 pre-map rect is not a window (spawn::plausible_size)
        }
        self.ring.push(r);
        if self.ring.len() > Self::N {
            self.ring.remove(0);
        }
    }
    /// the bounds to save; `max_now` = the current rect when maximized
    pub fn normal(&self, max_now: Option<Rect>) -> Option<Rect> {
        let mut it = self.ring.iter().rev();
        match max_now {
            None => it.next().copied(),
            Some(m) => it.find(|r| (r.w, r.h) != (m.w, m.h)).copied(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: (f64, f64) = (1100.0, 700.0);
    fn mon() -> Vec<Rect> {
        vec![Rect { x: 0.0, y: 0.0, w: 1400.0, h: 900.0 }]
    }
    fn plan(v: Value) -> Plan {
        sanitise(Some(&v), &mon(), D, false)
    }
    fn is_default(p: &Plan) -> bool {
        (p.w, p.h, p.pos, p.max) == (D.0, D.1, None, false)
    }

    #[test]
    fn winsize_round_trip_restores_size_position_and_maximized() {
        let p = plan(json!({"x":200,"y":120,"width":900,"height":600,"isMaximized":false}));
        assert_eq!((p.w, p.h, p.pos, p.max, p.why), (900.0, 600.0, Some((200.0, 120.0)), false, "restored"));
        let p = plan(json!({"x":200,"y":120,"width":900,"height":600,"isMaximized":true}));
        assert_eq!((p.w, p.h, p.pos, p.max), (900.0, 600.0, Some((200.0, 120.0)), true), "normal bounds first, then maximized");
        // stock's own extra keys are read past
        let p = plan(json!({"x":1,"y":2,"width":900,"height":600,"devTools":true,"zoom":0}));
        assert_eq!((p.w, p.pos), (900.0, Some((1.0, 2.0))));
    }

    #[test]
    fn winsize_no_record_or_not_an_object_is_the_full_default() {
        assert!(is_default(&sanitise(None, &mon(), D, false)));
        assert!(is_default(&plan(json!("garbage"))));
        assert!(is_default(&plan(json!([1, 2, 3]))));
        assert!(is_default(&plan(json!({}))));
    }

    #[test]
    fn winsize_wrong_types_are_the_full_default() {
        assert_eq!(plan(json!({"x":"a","y":0,"width":900,"height":600})).why, "bad-type");
        assert!(is_default(&plan(json!({"x":0,"y":0,"width":"900","height":600}))));
        assert!(is_default(&plan(json!({"x":0,"y":0,"width":900,"height":true}))));
        // isMaximized of the wrong type is just "not maximized"
        assert_eq!(plan(json!({"x":0,"y":0,"width":900,"height":600,"isMaximized":"yes"})).max, false);
    }

    #[test]
    fn winsize_zero_negative_size_is_the_full_default_not_stocks_300x200() {
        for (w, h) in [(0, 0), (0, 600), (900, 0), (-5, 600), (900, -1)] {
            let p = plan(json!({"x":10,"y":10,"width":w,"height":h,"isMaximized":true}));
            assert!(is_default(&p), "{w}x{h} -> {p:?}");
            assert_eq!(p.why, "zero-size");
        }
    }

    #[test]
    fn winsize_below_minimum_is_raised_at_the_saved_position() {
        let p = plan(json!({"x":50,"y":60,"width":50,"height":40}));
        assert_eq!((p.w, p.h, p.pos, p.why), (MIN_W, MIN_H, Some((50.0, 60.0)), "min"));
        let p = plan(json!({"x":50,"y":60,"width":800,"height":40}));
        assert_eq!((p.w, p.h), (800.0, MIN_H));
    }

    #[test]
    fn winsize_oversized_is_clamped_to_the_monitor_and_pulled_onto_it() {
        let p = plan(json!({"x":0,"y":0,"width":5000,"height":4000}));
        assert_eq!((p.w, p.h, p.pos, p.why), (1400.0, 900.0, Some((0.0, 0.0)), "clamped"));
        let p = plan(json!({"x":-4900,"y":0,"width":5000,"height":800}));
        assert_eq!((p.w, p.pos), (1400.0, Some((0.0, 0.0))), "a clamped rect must still be on the screen");
    }

    #[test]
    fn winsize_fully_off_screen_is_the_full_default_partly_on_is_kept() {
        for (x, y) in [(5000, 5000), (-3000, 100), (100, -2000), (1400, 0)] {
            let p = plan(json!({"x":x,"y":y,"width":900,"height":600}));
            assert!(is_default(&p), "{x},{y} -> {p:?}");
            assert_eq!(p.why, "off-screen");
        }
        // E2: 1300,850 with 1024x800 on 1400x900 -> 100x50 visible -> as-is
        let p = plan(json!({"x":1300,"y":850,"width":1024,"height":800}));
        assert_eq!((p.w, p.h, p.pos, p.why), (1024.0, 800.0, Some((1300.0, 850.0)), "restored"));
    }

    #[test]
    fn winsize_missing_position_keeps_the_size_centred() {
        let p = plan(json!({"width":800,"height":500}));
        assert_eq!((p.w, p.h, p.pos, p.why), (800.0, 500.0, None, "centred"));
        let p = plan(json!({"x":10,"width":800,"height":500}));
        assert_eq!(p.pos, None);
    }

    #[test]
    fn winsize_multi_monitor_uses_the_monitor_the_rect_is_on() {
        let mons = vec![Rect { x: 0.0, y: 0.0, w: 1400.0, h: 900.0 }, Rect { x: 1400.0, y: 0.0, w: 1920.0, h: 1080.0 }];
        let v = json!({"x":1500,"y":100,"width":1800,"height":1000});
        let p = sanitise(Some(&v), &mons, D, false);
        // bigger than the primary, fits the second monitor it is on: NOT clamped
        assert_eq!((p.w, p.h, p.pos, p.why), (1800.0, 1000.0, Some((1500.0, 100.0)), "restored"));
        // on the second monitor only: not off-screen
        let v = json!({"x":2000,"y":100,"width":900,"height":600});
        assert_eq!(sanitise(Some(&v), &mons, D, false).pos, Some((2000.0, 100.0)));
        // too tall for the second monitor -> clamped to IT, not to the primary
        let v = json!({"x":1500,"y":0,"width":1800,"height":3000});
        let p = sanitise(Some(&v), &mons, D, false);
        assert_eq!((p.w, p.h, p.why), (1800.0, 1080.0, "clamped"));
    }

    #[test]
    fn winsize_wayland_applies_size_and_max_never_position_and_saves_no_position() {
        let v = json!({"x":200,"y":120,"width":900,"height":600,"isMaximized":true});
        let p = sanitise(Some(&v), &mon(), D, true);
        assert_eq!((p.w, p.h, p.pos, p.max), (900.0, 600.0, None, true));
        let r = Rect { x: 0.0, y: 0.0, w: 900.4, h: 600.6 };
        assert_eq!(record(r, false, true), json!({"width":900,"height":601,"isMaximized":false}));
        assert_eq!(record(Rect { x: 12.4, y: -3.6, ..r }, true, false), json!({"x":12,"y":-4,"width":900,"height":601,"isMaximized":true}));
    }

    #[test]
    fn winsize_no_monitors_known_skips_the_screen_rules() {
        let v = json!({"x":9000,"y":9000,"width":5000,"height":4000});
        let p = sanitise(Some(&v), &[], D, false);
        assert_eq!((p.w, p.h, p.pos), (5000.0, 4000.0, Some((9000.0, 9000.0))));
    }

    #[test]
    fn winsize_track_keeps_the_normal_bounds_across_a_maximize() {
        let mut t = Track::default();
        t.see(Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 }); // pre-map junk
        assert_eq!(t.normal(None), None);
        let a = Rect { x: 200.0, y: 120.0, w: 900.0, h: 600.0 };
        t.see(a);
        assert_eq!(t.normal(None), Some(a));
        let m = Rect { x: 0.0, y: 0.0, w: 1400.0, h: 900.0 };
        assert_eq!(t.normal(Some(m)), Some(a));
        // the maximized configure sampled before the state flag flipped
        t.see(m);
        assert_eq!(t.normal(Some(m)), Some(a), "the maximized size is never the normal bounds");
        for i in 0..10 {
            t.see(Rect { x: i as f64, ..a });
        }
        assert_eq!(t.normal(None), Some(Rect { x: 9.0, ..a }));
    }
}
