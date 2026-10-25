//! Turns a stream of [`TouchFrame`]s into [`Gesture`]s.
//!
//! A *session* lasts from the first finger landing until the last one lifts.
//! Within a session the recogniser is in one of these phases:
//!
//! ```text
//!            ┌──── fingers lift quickly, barely moved ───▶ Tap
//!  Pending ──┼──── fingers rest without moving ──────────▶ Hold      ─▶ Consumed
//!            ├──── fingers spread or close, pinch bound ─▶ Pinching  (step 0, 1, 2…)
//!            ├──── fingers travel, drag enabled ─────────▶ Dragging  (start/move/end)
//!            └──── fingers travel ───────────────────────▶ Swiping   (step 0, 1, 2…)
//! ```
//!
//! Movement and spread are measured from contacts that are present in
//! consecutive frames, so a finger landing or lifting never looks like a jump.

use crate::frame::{Contact, TouchFrame};
use crate::gesture::{Direction, Gesture, PinchDirection};

#[derive(Clone, Debug, PartialEq)]
pub struct RecognizerSettings {
    /// Fewer fingers than this belong to the OS (pointing, scrolling).
    pub min_fingers: u8,
    pub tap_max_ms: u64,
    pub tap_slop_mm: f32,
    pub swipe_threshold_mm: f32,
    pub swipe_step_mm: f32,
    /// Change in finger spread before a pinch is recognised.
    pub pinch_threshold_mm: f32,
    pub hold_ms: u64,
    pub drag_release_ms: u64,
    /// Indexed by finger count: does movement with this many fingers drag?
    pub drag_fingers: [bool; 6],
    /// Indexed by finger count: is a pinch bound? Pinches are only looked
    /// for when one is, so a sloppy swipe can never be mistaken for one.
    pub pinch_fingers: [bool; 6],
}

impl Default for RecognizerSettings {
    fn default() -> Self {
        crate::Config::default().recognizer_settings()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Idle,
    Pending,
    Swiping { axis: Axis, progress: f32, step: u32 },
    Pinching { progress: f32, step: u32 },
    Dragging { lifted_at: Option<u64> },
    Consumed,
}

#[derive(Debug)]
pub struct Recognizer {
    settings: RecognizerSettings,
    phase: Phase,
    /// Finger count the current phase is about.
    fingers: u8,
    session_start: u64,
    max_fingers: u8,
    /// Has any gesture fired this session? (Then the lift is not a tap.)
    fired: bool,
    /// Where each finger first landed, for the tap slop check.
    origins: Vec<(u32, f32, f32)>,
    /// Furthest any finger has wandered from where it landed.
    max_wander: f32,
    prev: Vec<Contact>,
    /// Movement accumulated since `anchor_time` with the current finger count.
    travel: (f32, f32),
    /// Change in finger spread since `anchor_time` (negative = closing).
    spread: f32,
    anchor_time: u64,
}

impl Recognizer {
    pub fn new(settings: RecognizerSettings) -> Self {
        Recognizer {
            settings,
            phase: Phase::Idle,
            fingers: 0,
            session_start: 0,
            max_fingers: 0,
            fired: false,
            origins: Vec::new(),
            max_wander: 0.0,
            prev: Vec::new(),
            travel: (0.0, 0.0),
            spread: 0.0,
            anchor_time: 0,
        }
    }

    pub fn settings(&self) -> &RecognizerSettings {
        &self.settings
    }

    /// Swap settings mid-stream. An in-flight drag is kept so the mouse
    /// button is never left stuck.
    pub fn set_settings(&mut self, settings: RecognizerSettings) {
        self.settings = settings;
    }

    pub fn is_dragging(&self) -> bool {
        matches!(self.phase, Phase::Dragging { .. })
    }

    /// Abandon the current session. Returns `DragEnd` if a drag was active so
    /// the caller can release the mouse button.
    pub fn reset(&mut self) -> Vec<Gesture> {
        let out = if self.is_dragging() { vec![Gesture::DragEnd] } else { Vec::new() };
        self.phase = Phase::Idle;
        self.prev.clear();
        self.origins.clear();
        self.fired = false;
        out
    }

    /// Feed one frame; returns any gestures it completed.
    pub fn update(&mut self, frame: &TouchFrame) -> Vec<Gesture> {
        let mut out = Vec::new();
        let t = frame.time_ms;
        let n = frame.finger_count();
        let (dx, dy) = shared_delta(&self.prev, &frame.contacts);
        let dspread = spread_delta(&self.prev, &frame.contacts);

        if n == 0 {
            self.on_lift(t, &mut out);
            self.prev.clear();
            return out;
        }

        match self.phase {
            Phase::Idle => self.begin_session(t, n),
            Phase::Dragging { lifted_at: Some(_) } => {
                if n >= 2 {
                    // Fingers came back within the grace period: keep dragging.
                    self.phase = Phase::Dragging { lifted_at: None };
                } else {
                    // A single finger is ordinary pointing; finish the drag first.
                    out.push(Gesture::DragEnd);
                    self.begin_session(t, n);
                }
            }
            _ => {}
        }
        self.track_wander(&frame.contacts);
        self.max_fingers = self.max_fingers.max(n);

        match self.phase {
            Phase::Pending => {
                if n != self.fingers {
                    self.fingers = n;
                    self.anchor(t);
                } else {
                    self.travel.0 += dx;
                    self.travel.1 += dy;
                    self.spread += dspread;
                }
                self.check_motion(&mut out);
                self.check_hold(t, &mut out);
            }
            Phase::Swiping { axis, mut progress, mut step } => {
                if n != self.fingers {
                    // Fingers lifting one by one at the end of a swipe.
                    self.phase = Phase::Consumed;
                } else {
                    progress += match axis {
                        Axis::Horizontal => dx,
                        Axis::Vertical => dy,
                    };
                    let size = self.settings.swipe_step_mm;
                    while progress.abs() >= size {
                        step += 1;
                        let direction = axis_direction(axis, progress);
                        out.push(Gesture::Swipe { fingers: self.fingers, direction, step });
                        progress -= size * progress.signum();
                    }
                    self.phase = Phase::Swiping { axis, progress, step };
                }
            }
            Phase::Pinching { mut progress, mut step } => {
                if n != self.fingers {
                    self.phase = Phase::Consumed;
                } else {
                    progress += dspread;
                    let size = self.settings.swipe_step_mm;
                    while progress.abs() >= size {
                        step += 1;
                        let direction = PinchDirection::from_change(progress);
                        out.push(Gesture::Pinch { fingers: self.fingers, direction, step });
                        progress -= size * progress.signum();
                    }
                    self.phase = Phase::Pinching { progress, step };
                }
            }
            Phase::Dragging { .. } => {
                // With one finger left the OS moves the cursor itself; we only
                // keep the button held.
                if n >= 2 && (dx != 0.0 || dy != 0.0) {
                    out.push(Gesture::DragMove { dx, dy });
                }
            }
            Phase::Consumed | Phase::Idle => {}
        }

        self.prev.clear();
        self.prev.extend_from_slice(&frame.contacts);
        out
    }

    /// Advance timers without a new frame. Some touchpads (and evdev) stop
    /// reporting while fingers rest, so holds and drag release need this.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Gesture> {
        let mut out = Vec::new();
        match self.phase {
            Phase::Pending if !self.prev.is_empty() => self.check_hold(now_ms, &mut out),
            Phase::Dragging { lifted_at: Some(at) } if now_ms.saturating_sub(at) >= self.settings.drag_release_ms => {
                out.push(Gesture::DragEnd);
                self.end_session();
            }
            _ => {}
        }
        out
    }

    fn begin_session(&mut self, t: u64, n: u8) {
        self.phase = Phase::Pending;
        self.session_start = t;
        self.max_fingers = n;
        self.fingers = n;
        self.fired = false;
        self.origins.clear();
        self.max_wander = 0.0;
        self.prev.clear();
        self.anchor(t);
    }

    fn end_session(&mut self) {
        self.phase = Phase::Idle;
        self.origins.clear();
        self.prev.clear();
    }

    fn anchor(&mut self, t: u64) {
        self.travel = (0.0, 0.0);
        self.spread = 0.0;
        self.anchor_time = t;
    }

    fn track_wander(&mut self, contacts: &[Contact]) {
        for c in contacts {
            match self.origins.iter().find(|o| o.0 == c.id) {
                Some(&(_, ox, oy)) => self.max_wander = self.max_wander.max((c.x - ox).hypot(c.y - oy)),
                None => self.origins.push((c.id, c.x, c.y)),
            }
        }
    }

    fn check_motion(&mut self, out: &mut Vec<Gesture>) {
        let n = self.fingers;
        let (tx, ty) = self.travel;
        if n < self.settings.min_fingers {
            return;
        }
        let travel = tx.hypot(ty);
        let pinch_bound = self.settings.pinch_fingers.get(n as usize).copied().unwrap_or(false);
        if pinch_bound && self.spread.abs() >= self.settings.pinch_threshold_mm && self.spread.abs() > travel {
            self.fired = true;
            out.push(Gesture::Pinch { fingers: n, direction: PinchDirection::from_change(self.spread), step: 0 });
            self.phase = Phase::Pinching { progress: 0.0, step: 0 };
            return;
        }
        if travel < self.settings.swipe_threshold_mm {
            return;
        }
        self.fired = true;
        if self.settings.drag_fingers.get(n as usize).copied().unwrap_or(false) {
            out.push(Gesture::DragStart { fingers: n });
            // Deliver the threshold distance too, so the drag doesn't lag the fingers.
            out.push(Gesture::DragMove { dx: tx, dy: ty });
            self.phase = Phase::Dragging { lifted_at: None };
        } else {
            let axis = if tx.abs() >= ty.abs() { Axis::Horizontal } else { Axis::Vertical };
            out.push(Gesture::Swipe { fingers: n, direction: Direction::from_delta(tx, ty), step: 0 });
            self.phase = Phase::Swiping { axis, progress: 0.0, step: 0 };
        }
    }

    fn check_hold(&mut self, t: u64, out: &mut Vec<Gesture>) {
        let n = self.fingers;
        let s = &self.settings;
        if self.fired || n < s.min_fingers || t.saturating_sub(self.anchor_time) < s.hold_ms {
            return;
        }
        if self.travel.0.hypot(self.travel.1) <= s.tap_slop_mm && self.spread.abs() <= s.tap_slop_mm {
            out.push(Gesture::Hold { fingers: n });
            self.fired = true;
            self.phase = Phase::Consumed;
        }
    }

    fn on_lift(&mut self, t: u64, out: &mut Vec<Gesture>) {
        match self.phase {
            Phase::Idle => return,
            Phase::Dragging { lifted_at: Some(_) } => return,
            Phase::Dragging { lifted_at: None } => {
                if self.settings.drag_release_ms == 0 {
                    out.push(Gesture::DragEnd);
                    self.end_session();
                } else {
                    self.phase = Phase::Dragging { lifted_at: Some(t) };
                }
                return;
            }
            _ => {}
        }
        let s = &self.settings;
        let quick = t.saturating_sub(self.session_start) <= s.tap_max_ms;
        if !self.fired && quick && self.max_fingers >= s.min_fingers && self.max_wander <= s.tap_slop_mm {
            out.push(Gesture::Tap { fingers: self.max_fingers });
        }
        self.end_session();
    }
}

fn axis_direction(axis: Axis, progress: f32) -> Direction {
    match (axis, progress >= 0.0) {
        (Axis::Horizontal, true) => Direction::Right,
        (Axis::Horizontal, false) => Direction::Left,
        (Axis::Vertical, true) => Direction::Down,
        (Axis::Vertical, false) => Direction::Up,
    }
}

/// Change in how spread out the fingers present in both frames are: the
/// mean distance from their centroid now, minus the same measure before.
fn spread_delta(prev: &[Contact], cur: &[Contact]) -> f32 {
    let pairs: Vec<(&Contact, &Contact)> =
        cur.iter().filter_map(|c| prev.iter().find(|p| p.id == c.id).map(|p| (p, c))).collect();
    if pairs.len() < 2 {
        return 0.0;
    }
    let spread = |pts: &mut dyn Iterator<Item = &Contact>| {
        let pts: Vec<&Contact> = pts.collect();
        let n = pts.len() as f32;
        let (cx, cy) = (pts.iter().map(|c| c.x).sum::<f32>() / n, pts.iter().map(|c| c.y).sum::<f32>() / n);
        pts.iter().map(|c| (c.x - cx).hypot(c.y - cy)).sum::<f32>() / n
    };
    spread(&mut pairs.iter().map(|(_, c)| *c)) - spread(&mut pairs.iter().map(|(p, _)| *p))
}

/// Mean movement of fingers present in both frames.
fn shared_delta(prev: &[Contact], cur: &[Contact]) -> (f32, f32) {
    let (mut sx, mut sy, mut k) = (0.0, 0.0, 0u32);
    for c in cur {
        if let Some(p) = prev.iter().find(|p| p.id == c.id) {
            sx += c.x - p.x;
            sy += c.y - p.y;
            k += 1;
        }
    }
    if k == 0 {
        (0.0, 0.0)
    } else {
        (sx / k as f32, sy / k as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> RecognizerSettings {
        RecognizerSettings {
            min_fingers: 3,
            tap_max_ms: 200,
            tap_slop_mm: 3.0,
            swipe_threshold_mm: 10.0,
            swipe_step_mm: 5.0,
            hold_ms: 500,
            drag_release_ms: 0,
            drag_fingers: [false; 6],
            pinch_threshold_mm: 8.0,
            pinch_fingers: [false; 6],
        }
    }

    /// Scripted fingers: each call places `n` fingers at (x, y) offsets.
    struct Script {
        rec: Recognizer,
        out: Vec<Gesture>,
    }

    impl Script {
        fn new(s: RecognizerSettings) -> Self {
            Script { rec: Recognizer::new(s), out: Vec::new() }
        }
        fn frame(&mut self, t: u64, n: u32, x: f32, y: f32) -> &mut Self {
            let contacts = (0..n).map(|i| Contact { id: i, x: 20.0 + i as f32 * 15.0 + x, y: 30.0 + y }).collect();
            let frame = TouchFrame { time_ms: t, contacts, width_mm: 100.0, height_mm: 60.0 };
            self.out.extend(self.rec.update(&frame));
            self
        }
        /// `n` fingers in a row centred at x = 50, `gap` mm apart.
        fn spread(&mut self, t: u64, n: u32, gap: f32) -> &mut Self {
            let mid = (n as f32 - 1.0) / 2.0;
            let contacts = (0..n).map(|i| Contact { id: i, x: 50.0 + (i as f32 - mid) * gap, y: 30.0 }).collect();
            let frame = TouchFrame { time_ms: t, contacts, width_mm: 100.0, height_mm: 60.0 };
            self.out.extend(self.rec.update(&frame));
            self
        }
        fn lift(&mut self, t: u64) -> &mut Self {
            self.out.extend(self.rec.update(&TouchFrame { time_ms: t, ..Default::default() }));
            self
        }
        fn tick(&mut self, t: u64) -> &mut Self {
            self.out.extend(self.rec.tick(t));
            self
        }
    }

    #[test]
    fn three_finger_tap() {
        let mut s = Script::new(settings());
        s.frame(0, 3, 0.0, 0.0).frame(50, 3, 0.5, 0.5).lift(120);
        assert_eq!(s.out, vec![Gesture::Tap { fingers: 3 }]);
    }

    #[test]
    fn fingers_landing_one_by_one_still_tap_with_max_count() {
        let mut s = Script::new(settings());
        s.frame(0, 1, 0.0, 0.0).frame(20, 2, 0.0, 0.0).frame(40, 4, 0.0, 0.0).frame(80, 2, 0.0, 0.0).lift(110);
        assert_eq!(s.out, vec![Gesture::Tap { fingers: 4 }]);
    }

    #[test]
    fn slow_or_wandering_touches_are_not_taps() {
        let mut s = Script::new(settings());
        s.frame(0, 3, 0.0, 0.0).lift(400);
        assert!(s.out.is_empty(), "too slow");

        let mut s = Script::new(settings());
        s.frame(0, 3, 0.0, 0.0).frame(50, 3, 5.0, 0.0).lift(100);
        assert!(s.out.is_empty(), "moved too far: {:?}", s.out);
    }

    #[test]
    fn two_fingers_belong_to_the_os() {
        let mut s = Script::new(settings());
        s.frame(0, 2, 0.0, 0.0).lift(50);
        s.frame(100, 2, 0.0, 0.0).frame(150, 2, 0.0, -40.0).lift(200);
        assert!(s.out.is_empty());
    }

    #[test]
    fn swipe_up_fires_once_at_threshold() {
        let mut s = Script::new(settings());
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 0.0, -4.0).frame(20, 3, 1.0, -11.0);
        assert_eq!(s.out, vec![Gesture::Swipe { fingers: 3, direction: Direction::Up, step: 0 }]);
        s.lift(40);
        assert_eq!(s.out.len(), 1, "lift after swipe is not a tap");
    }

    #[test]
    fn swipe_direction_uses_dominant_axis() {
        for (dx, dy, dir) in
            [(12.0, 3.0, Direction::Right), (-12.0, -3.0, Direction::Left), (2.0, 12.0, Direction::Down)]
        {
            let mut s = Script::new(settings());
            s.frame(0, 4, 0.0, 0.0).frame(10, 4, dx, dy);
            assert_eq!(s.out, vec![Gesture::Swipe { fingers: 4, direction: dir, step: 0 }]);
        }
    }

    #[test]
    fn continued_swipe_emits_steps_and_can_reverse() {
        let mut s = Script::new(settings());
        // Commit at -10, then 16mm more up (three 5mm steps), then 15mm back down.
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 0.0, -10.0).frame(20, 3, 0.0, -26.0).frame(30, 3, 0.0, -11.0);
        let steps: Vec<_> = s
            .out
            .iter()
            .map(|g| match g {
                Gesture::Swipe { direction, step, .. } => (*direction, *step),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            steps,
            vec![
                (Direction::Up, 0),
                (Direction::Up, 1),
                (Direction::Up, 2),
                (Direction::Up, 3),
                (Direction::Down, 4),
                (Direction::Down, 5),
            ]
        );
    }

    #[test]
    fn adding_a_finger_reanchors_instead_of_jumping() {
        let mut s = Script::new(settings());
        // Two-finger scroll travels 30mm, then a third finger lands: no swipe yet.
        s.frame(0, 2, 0.0, 0.0).frame(10, 2, 0.0, -30.0).frame(20, 3, 0.0, -30.0);
        assert!(s.out.is_empty());
        s.frame(30, 3, 0.0, -41.0);
        assert_eq!(s.out, vec![Gesture::Swipe { fingers: 3, direction: Direction::Up, step: 0 }]);
    }

    #[test]
    fn hold_fires_from_tick_when_device_goes_quiet() {
        let mut s = Script::new(settings());
        s.frame(0, 4, 0.0, 0.0).tick(300);
        assert!(s.out.is_empty());
        s.tick(520);
        assert_eq!(s.out, vec![Gesture::Hold { fingers: 4 }]);
        s.tick(900).lift(1000);
        assert_eq!(s.out.len(), 1, "hold fires once and the lift is not a tap");
    }

    #[test]
    fn moving_fingers_do_not_hold() {
        let mut s = Script::new(settings());
        s.frame(0, 3, 0.0, 0.0).frame(300, 3, 6.0, 0.0).tick(600);
        assert!(s.out.is_empty());
    }

    #[test]
    fn drag_start_move_end() {
        let mut cfg = settings();
        cfg.drag_fingers[3] = true;
        let mut s = Script::new(cfg);
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 8.0, 6.0).frame(20, 3, 10.0, 8.0).lift(30);
        assert_eq!(
            s.out,
            vec![
                Gesture::DragStart { fingers: 3 },
                Gesture::DragMove { dx: 8.0, dy: 6.0 },
                Gesture::DragMove { dx: 2.0, dy: 2.0 },
                Gesture::DragEnd,
            ]
        );
    }

    #[test]
    fn drag_survives_brief_lift_within_grace_period() {
        let mut cfg = settings();
        cfg.drag_fingers[3] = true;
        cfg.drag_release_ms = 300;
        let mut s = Script::new(cfg);
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 12.0, 0.0).lift(20).tick(100);
        assert!(!s.out.contains(&Gesture::DragEnd), "still inside grace period");
        s.frame(150, 3, 0.0, 0.0).frame(160, 3, 4.0, 0.0);
        assert_eq!(s.out.last(), Some(&Gesture::DragMove { dx: 4.0, dy: 0.0 }));
        s.lift(200).tick(400);
        assert!(!s.out.contains(&Gesture::DragEnd), "grace restarted at second lift");
        s.tick(510);
        assert_eq!(s.out.last(), Some(&Gesture::DragEnd));
        assert_eq!(s.out.iter().filter(|g| **g == Gesture::DragEnd).count(), 1);
    }

    #[test]
    fn single_finger_during_grace_ends_drag() {
        let mut cfg = settings();
        cfg.drag_fingers[3] = true;
        cfg.drag_release_ms = 300;
        let mut s = Script::new(cfg);
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 12.0, 0.0).lift(20).frame(60, 1, 0.0, 0.0);
        assert_eq!(s.out.last(), Some(&Gesture::DragEnd));
        assert!(!s.rec.is_dragging());
    }

    #[test]
    fn drag_with_one_finger_left_keeps_button_but_stops_moving() {
        let mut cfg = settings();
        cfg.drag_fingers[3] = true;
        let mut s = Script::new(cfg);
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 12.0, 0.0);
        let before = s.out.len();
        s.frame(20, 1, 20.0, 0.0).frame(30, 1, 30.0, 0.0);
        assert_eq!(s.out.len(), before);
        assert!(s.rec.is_dragging());
    }

    #[test]
    fn reset_releases_drag() {
        let mut cfg = settings();
        cfg.drag_fingers[3] = true;
        let mut s = Script::new(cfg);
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 12.0, 0.0);
        assert_eq!(s.rec.reset(), vec![Gesture::DragEnd]);
        assert_eq!(s.rec.reset(), vec![]);
    }

    #[test]
    fn drag_on_three_does_not_affect_four_finger_swipes() {
        let mut cfg = settings();
        cfg.drag_fingers[3] = true;
        let mut s = Script::new(cfg);
        s.frame(0, 4, 0.0, 0.0).frame(10, 4, -12.0, 0.0);
        assert_eq!(s.out, vec![Gesture::Swipe { fingers: 4, direction: Direction::Left, step: 0 }]);
    }

    fn pinch_settings() -> RecognizerSettings {
        let mut s = settings();
        s.pinch_fingers[3] = true;
        s
    }

    #[test]
    fn pinch_out_and_in() {
        // Three fingers in a row: spread = 2/3 of the gap, so 15 → 28 mm is +8.7 mm.
        let mut s = Script::new(pinch_settings());
        s.spread(0, 3, 15.0).spread(10, 3, 28.0);
        assert_eq!(s.out, vec![Gesture::Pinch { fingers: 3, direction: PinchDirection::Out, step: 0 }]);

        let mut s = Script::new(pinch_settings());
        s.spread(0, 3, 28.0).spread(10, 3, 15.0);
        assert_eq!(s.out, vec![Gesture::Pinch { fingers: 3, direction: PinchDirection::In, step: 0 }]);
        s.lift(30);
        assert_eq!(s.out.len(), 1, "lift after a pinch is not a tap");
    }

    #[test]
    fn pinch_keeps_stepping_and_reverses() {
        let mut s = Script::new(pinch_settings());
        // Commit (+8.7), then +10 more spread (two 5 mm steps), then back -10.
        s.spread(0, 3, 15.0).spread(10, 3, 28.0).spread(20, 3, 43.0).spread(30, 3, 28.0);
        let steps: Vec<_> = s
            .out
            .iter()
            .map(|g| match g {
                Gesture::Pinch { direction, step, .. } => (*direction, *step),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            steps,
            vec![
                (PinchDirection::Out, 0),
                (PinchDirection::Out, 1),
                (PinchDirection::Out, 2),
                (PinchDirection::In, 3),
                (PinchDirection::In, 4),
            ]
        );
    }

    #[test]
    fn pinch_needs_a_binding() {
        let mut s = Script::new(settings());
        s.spread(0, 3, 15.0).spread(10, 3, 28.0).spread(20, 3, 40.0);
        assert!(s.out.is_empty(), "unbound pinches are ignored: {:?}", s.out);
    }

    #[test]
    fn swipes_still_work_when_pinch_is_bound() {
        let mut s = Script::new(pinch_settings());
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 0.0, -12.0);
        assert_eq!(s.out, vec![Gesture::Swipe { fingers: 3, direction: Direction::Up, step: 0 }]);
    }

    #[test]
    fn pinching_fingers_do_not_hold() {
        let mut s = Script::new(settings());
        s.spread(0, 3, 15.0).spread(300, 3, 22.0).tick(700);
        assert!(s.out.is_empty(), "spread changed by more than the slop: {:?}", s.out);
    }
}
