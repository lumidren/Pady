//! Turns a stream of [`TouchFrame`]s into [`Gesture`]s.
//!
//! A *session* lasts from the first finger landing until the last one lifts.
//! Within a session the recogniser is in one of these phases:
//!
//! ```text
//!            ┌──── fingers lift quickly, barely moved ───▶ Tap (or double / triple tap)
//!  Pending ──┼──── fingers rest without moving ──────────▶ Hold      ─▶ Consumed
//!            ├──── fingers spread or close, pinch bound ─▶ Pinching  (step 0, 1, 2…)
//!            ├──── fingers travel, drag enabled ─────────▶ Dragging  (start/move/end)
//!            └──── fingers travel ───────────────────────▶ Swiping   (step 0, 1, 2…)
//! ```
//!
//! Movement and spread are measured from contacts that are present in
//! consecutive frames, so a finger landing or lifting never looks like a jump.
//!
//! Taps are counted across sessions: when a double or triple tap is bound
//! for a finger count, a tap waits up to `multi_tap_ms` for the next one
//! before it is reported. With no multi-tap bound, taps fire immediately.

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
    /// A tap starting within this long of the previous one's lift counts
    /// towards a double or triple tap.
    pub multi_tap_ms: u64,
    /// Indexed by finger count: the most taps in a row that are bound (1–3).
    pub multi_tap_fingers: [u8; 6],
    /// Indexed by finger count: swipes scrub a level (volume, brightness), so
    /// they repeat every [`GLIDE_DIVISOR`]th of `swipe_step_mm` and glide.
    pub glide_fingers: [bool; 6],
}

/// Gliding swipes repeat this many times as often as other swipes.
pub const GLIDE_DIVISOR: f32 = 3.5;

/// A repeating pinch steps every this many `swipe_step_mm` of span change:
/// the span moves about twice as fast as each finger does.
pub const PINCH_STEP_FACTOR: f32 = 2.0;

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

/// A tap that may still become a double or triple tap.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PendingTap {
    fingers: u8,
    count: u8,
    lifted_at: u64,
}

fn tap_gesture(fingers: u8, count: u8) -> Gesture {
    match count {
        0 | 1 => Gesture::Tap { fingers },
        2 => Gesture::DoubleTap { fingers },
        _ => Gesture::TripleTap { fingers },
    }
}

/// A swipe on its way, before it fires: for feedback such as a glow at the
/// screen edge it is heading for.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct SwipeProgress {
    pub fingers: u8,
    pub direction: Direction,
    /// 0 as the fingers start to move, 1 when the swipe fires.
    pub amount: f32,
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
    /// Change in the hand's span (the two fingers furthest apart) since
    /// `anchor_time`: what pinches are judged by. Unlike the spread, it moves
    /// by the full distance when one finger (often the thumb) does all the
    /// pinching, as it does with a tilted hand.
    span: f32,
    anchor_time: u64,
    pending_tap: Option<PendingTap>,
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
            span: 0.0,
            anchor_time: 0,
            pending_tap: None,
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

    /// How far a swipe that hasn't fired yet has come. `None` while nothing
    /// is heading for one: too few fingers, a drag, a pinch, or resting fingers.
    pub fn swipe_progress(&self) -> Option<SwipeProgress> {
        let s = &self.settings;
        let n = self.fingers;
        if self.phase != Phase::Pending || self.fired || self.prev.is_empty() || n < s.min_fingers {
            return None;
        }
        if s.drag_fingers.get(n as usize).copied().unwrap_or(false) {
            return None;
        }
        let (tx, ty) = self.travel;
        let travel = tx.hypot(ty);
        // Still resting (or a tap), or closing and opening more than sliding: a pinch.
        if travel <= s.tap_slop_mm / 2.0 || (self.pinch_bound(n) && self.span.abs() > travel) {
            return None;
        }
        let amount = ((travel - s.tap_slop_mm / 2.0) / (s.swipe_threshold_mm - s.tap_slop_mm / 2.0).max(0.1)).min(1.0);
        Some(SwipeProgress { fingers: n, direction: Direction::from_delta(tx, ty), amount })
    }

    /// Abandon the current session. Returns `DragEnd` if a drag was active so
    /// the caller can release the mouse button.
    pub fn reset(&mut self) -> Vec<Gesture> {
        let out = if self.is_dragging() { vec![Gesture::DragEnd] } else { Vec::new() };
        self.pending_tap = None;
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
        let dspan = span_delta(&self.prev, &frame.contacts);

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
                if n < self.fingers
                    && self.pinch_bound(self.fingers)
                    && self.span <= -self.settings.pinch_threshold_mm / 2.0
                {
                    // A fast pinch in squeezes fingers together until the pad
                    // merges two of them: the pinch was already well under way.
                    self.flush_tap(&mut out);
                    self.fired = true;
                    out.push(Gesture::Pinch { fingers: self.fingers, direction: PinchDirection::In, step: 0 });
                    self.phase = Phase::Consumed;
                } else if n != self.fingers {
                    self.fingers = n;
                    self.anchor(t);
                } else {
                    self.travel.0 += dx;
                    self.travel.1 += dy;
                    self.spread += dspread;
                    self.span += dspan;
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
                    let direction = axis_direction(axis, progress);
                    let glide = self.settings.glide_fingers.get(self.fingers as usize).copied().unwrap_or(false);
                    let size = self.settings.swipe_step_mm / if glide { GLIDE_DIVISOR } else { 1.0 };
                    for _ in 0..take_steps(&mut progress, size) {
                        step += 1;
                        out.push(Gesture::Swipe { fingers: self.fingers, direction, step });
                    }
                    self.phase = Phase::Swiping { axis, progress, step };
                }
            }
            Phase::Pinching { mut progress, mut step } => {
                if n != self.fingers {
                    self.phase = Phase::Consumed;
                } else {
                    progress += dspan;
                    let direction = PinchDirection::from_change(progress);
                    for _ in 0..take_steps(&mut progress, self.settings.swipe_step_mm * PINCH_STEP_FACTOR) {
                        step += 1;
                        out.push(Gesture::Pinch { fingers: self.fingers, direction, step });
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
        if let Some(p) = self.pending_tap {
            let expired = now_ms.saturating_sub(p.lifted_at) > self.settings.multi_tap_ms;
            if expired && !self.next_tap_underway(p) {
                self.flush_tap(&mut out);
            }
        }
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

    /// When `tick` next has work to do, if ever. The engine sleeps until
    /// then instead of waking up dozens of times a second while idle.
    pub fn next_deadline(&self) -> Option<u64> {
        let s = &self.settings;
        let timer = match self.phase {
            Phase::Pending
                if !self.fired
                    && !self.prev.is_empty()
                    && self.fingers >= s.min_fingers
                    && self.travel.0.hypot(self.travel.1) <= s.tap_slop_mm
                    && self.spread.abs() <= s.tap_slop_mm =>
            {
                Some(self.anchor_time.saturating_add(s.hold_ms))
            }
            Phase::Dragging { lifted_at: Some(at) } => Some(at.saturating_add(s.drag_release_ms)),
            _ => None,
        };
        let tap = self
            .pending_tap
            .filter(|&p| !self.next_tap_underway(p))
            .map(|p| p.lifted_at.saturating_add(s.multi_tap_ms).saturating_add(1));
        timer.into_iter().chain(tap).min()
    }

    /// Fingers came down soon enough to be the next tap of a series, so a
    /// pending tap must wait for this session to finish.
    fn next_tap_underway(&self, p: PendingTap) -> bool {
        self.phase != Phase::Idle && self.session_start.saturating_sub(p.lifted_at) <= self.settings.multi_tap_ms
    }

    /// A tap just finished: report it now, or hold it back while a double or
    /// triple tap is still possible.
    fn count_tap(&mut self, fingers: u8, lifted_at: u64, out: &mut Vec<Gesture>) {
        let most = self.settings.multi_tap_fingers.get(fingers as usize).copied().unwrap_or(1).clamp(1, 3);
        let count = match self.pending_tap {
            Some(p)
                if p.fingers == fingers
                    && self.session_start.saturating_sub(p.lifted_at) <= self.settings.multi_tap_ms =>
            {
                p.count + 1
            }
            _ => {
                self.flush_tap(out);
                1
            }
        };
        if count >= most {
            self.pending_tap = None;
            out.push(tap_gesture(fingers, count));
        } else {
            self.pending_tap = Some(PendingTap { fingers, count, lifted_at });
        }
    }

    /// Report a held-back tap (before anything that happens after it).
    fn flush_tap(&mut self, out: &mut Vec<Gesture>) {
        if let Some(p) = self.pending_tap.take() {
            out.push(tap_gesture(p.fingers, p.count));
        }
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
        self.span = 0.0;
        self.anchor_time = t;
    }

    fn track_wander(&mut self, contacts: &[Contact]) {
        for c in contacts.iter().filter(|c| sane(c)) {
            match self.origins.iter().find(|o| o.0 == c.id) {
                Some(&(_, ox, oy)) => self.max_wander = self.max_wander.max((c.x - ox).hypot(c.y - oy)),
                // A session that never ends (a resting finger while others come
                // and go) would otherwise remember every finger forever.
                None if self.origins.len() < MAX_ORIGINS => self.origins.push((c.id, c.x, c.y)),
                None => {}
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
        let span = self.span.abs();
        let pinch_bound = self.pinch_bound(n);
        // The hand closing or opening more than it slides is a pinch, whatever
        // its angle and however lopsided.
        if pinch_bound && span >= self.settings.pinch_threshold_mm && span > travel {
            self.flush_tap(out);
            self.fired = true;
            out.push(Gesture::Pinch { fingers: n, direction: PinchDirection::from_change(self.span), step: 0 });
            self.phase = Phase::Pinching { progress: 0.0, step: 0 };
            return;
        }
        if travel < self.settings.swipe_threshold_mm {
            return;
        }
        // Close call: a pinch on its way still moves the hand's centre. Don't
        // commit to a swipe until the hand has clearly travelled further.
        if pinch_bound
            && span >= self.settings.pinch_threshold_mm / 2.0
            && span * 2.0 >= travel
            && travel < self.settings.swipe_threshold_mm * 1.5
        {
            return;
        }
        self.flush_tap(out);
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

    fn pinch_bound(&self, fingers: u8) -> bool {
        self.settings.pinch_fingers.get(fingers as usize).copied().unwrap_or(false)
    }

    fn check_hold(&mut self, t: u64, out: &mut Vec<Gesture>) {
        let n = self.fingers;
        let s = &self.settings;
        if self.fired || n < s.min_fingers || t.saturating_sub(self.anchor_time) < s.hold_ms {
            return;
        }
        if self.travel.0.hypot(self.travel.1) <= s.tap_slop_mm && self.spread.abs() <= s.tap_slop_mm {
            self.flush_tap(out);
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
            self.count_tap(self.max_fingers, t, out);
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

/// Fingers whose landing spot a session remembers, far more than fit on a pad.
const MAX_ORIGINS: usize = 32;

/// Most repeat steps one frame may produce. Fingers move well under two
/// steps per frame; a bigger jump can only come from a broken driver.
const MAX_STEPS_PER_FRAME: u32 = 8;

/// Take whole `size` steps out of `progress` (towards zero), returning how
/// many. An impossible jump is dropped after [`MAX_STEPS_PER_FRAME`] steps.
fn take_steps(progress: &mut f32, size: f32) -> u32 {
    let mut steps = 0;
    while progress.abs() >= size {
        if steps == MAX_STEPS_PER_FRAME {
            *progress = 0.0;
            break;
        }
        steps += 1;
        *progress -= size * progress.signum();
    }
    steps
}

/// Fingers present in both frames, as (before, now). One at a broken
/// position is left out, as if it had briefly lifted: it would otherwise
/// fire a swipe, or stall one for good once a total turns NaN.
fn pairs<'a>(prev: &'a [Contact], cur: &'a [Contact]) -> impl Iterator<Item = (&'a Contact, &'a Contact)> {
    cur.iter().filter(|c| sane(c)).filter_map(|c| prev.iter().find(|p| p.id == c.id && sane(p)).map(|p| (p, c)))
}

/// Within ten metres of the pad's corner: finite, and not a driver's garbage.
fn sane(c: &Contact) -> bool {
    c.x.abs() <= 10_000.0 && c.y.abs() <= 10_000.0
}

/// Change in how spread out the fingers present in both frames are: the
/// mean distance from their centroid now, minus the same measure before.
fn spread_delta(prev: &[Contact], cur: &[Contact]) -> f32 {
    let pairs: Vec<(&Contact, &Contact)> = pairs(prev, cur).collect();
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

/// Change in the hand's span (the largest distance between two fingers),
/// for the fingers present in both frames. Independent of the hand's angle.
fn span_delta(prev: &[Contact], cur: &[Contact]) -> f32 {
    let pairs: Vec<(&Contact, &Contact)> = pairs(prev, cur).collect();
    if pairs.len() < 2 {
        return 0.0;
    }
    let now: Vec<(f32, f32)> = pairs.iter().map(|(_, c)| (c.x, c.y)).collect();
    let before: Vec<(f32, f32)> = pairs.iter().map(|(p, _)| (p.x, p.y)).collect();
    widest(&now) - widest(&before)
}

/// The largest distance between any two of `points`.
fn widest(points: &[(f32, f32)]) -> f32 {
    let mut widest = 0.0f32;
    for (i, &(ax, ay)) in points.iter().enumerate() {
        for &(bx, by) in &points[i + 1..] {
            widest = widest.max((ax - bx).hypot(ay - by));
        }
    }
    widest
}

/// Mean movement of fingers present in both frames.
fn shared_delta(prev: &[Contact], cur: &[Contact]) -> (f32, f32) {
    let (mut sx, mut sy, mut k) = (0.0, 0.0, 0u32);
    for (p, c) in pairs(prev, cur) {
        sx += c.x - p.x;
        sy += c.y - p.y;
        k += 1;
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
            multi_tap_ms: 300,
            multi_tap_fingers: [1; 6],
            glide_fingers: [false; 6],
        }
    }

    #[test]
    fn gliding_swipes_repeat_more_often() {
        let repeats = |glide: bool| {
            let mut s = Script::new(RecognizerSettings { glide_fingers: [glide; 6], ..settings() });
            // Past the 10 mm threshold, then 10.5 mm more in two moves.
            s.frame(0, 4, 0.0, 0.0).frame(10, 4, 0.0, -10.5).frame(20, 4, 0.0, -15.5).frame(30, 4, 0.0, -21.0).lift(40);
            s.out.iter().filter(|g| matches!(g, Gesture::Swipe { step, .. } if *step > 0)).count()
        };
        assert_eq!(repeats(false), 2, "5 mm steps");
        assert_eq!(repeats(true), 7, "5 / 3.5 mm steps");
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
    fn swipe_progress_grows_then_ends_when_the_swipe_fires() {
        let mut s = Script::new(settings());
        s.frame(0, 3, 0.0, 0.0);
        assert_eq!(s.rec.swipe_progress(), None, "resting fingers");
        s.frame(10, 3, 0.0, -5.0);
        let p = s.rec.swipe_progress().expect("on its way");
        assert_eq!((p.fingers, p.direction), (3, Direction::Up));
        assert!(p.amount > 0.2 && p.amount < 0.8, "{p:?}");
        s.frame(20, 3, 0.0, -8.0);
        assert!(s.rec.swipe_progress().unwrap().amount > p.amount, "grows as the fingers travel");
        s.frame(30, 3, 0.0, -12.0);
        assert_eq!(s.out, vec![Gesture::Swipe { fingers: 3, direction: Direction::Up, step: 0 }]);
        assert_eq!(s.rec.swipe_progress(), None, "fired");
    }

    #[test]
    fn no_swipe_progress_for_drags_pinches_or_two_fingers() {
        let mut drag = Script::new(RecognizerSettings { drag_fingers: [true; 6], ..settings() });
        drag.frame(0, 3, 0.0, 0.0).frame(10, 3, 6.0, 0.0);
        assert_eq!(drag.rec.swipe_progress(), None);
        let mut two = Script::new(settings());
        two.frame(0, 2, 0.0, 0.0).frame(10, 2, 6.0, 0.0);
        assert_eq!(two.rec.swipe_progress(), None);
        let mut pinch = Script::new(pinch_settings());
        pinch.spread(0, 3, 12.0).spread(10, 3, 15.0);
        assert_eq!(pinch.rec.swipe_progress(), None);
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
        // Three fingers in a row: the span is twice the gap, so 15 → 28 mm is +26 mm.
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
        // Commit, then +30 mm more span (three 10 mm steps), then back -30.
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
                (PinchDirection::Out, 3),
                (PinchDirection::In, 4),
                (PinchDirection::In, 5),
                (PinchDirection::In, 6),
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

    /// Frames of explicit contacts, for hands that aren't a neat row.
    fn hand(s: &mut Script, t: u64, fingers: &[(f32, f32)]) {
        let contacts = fingers.iter().enumerate().map(|(i, &(x, y))| Contact { id: i as u32, x, y }).collect();
        let frame = TouchFrame { time_ms: t, contacts, width_mm: 100.0, height_mm: 60.0 };
        s.out.extend(s.rec.update(&frame));
    }

    fn four_finger_pinch() -> RecognizerSettings {
        let mut s = settings();
        s.pinch_fingers[4] = true;
        s
    }

    #[test]
    fn a_tilted_hand_pinches() {
        // Three fingers on a 35° line, spreading from 12 to 22 mm apart.
        let (c, sn) = (35f32.to_radians().cos(), 35f32.to_radians().sin());
        let row = |gap: f32| [(50.0 - gap * c, 30.0 + gap * sn), (50.0, 30.0), (50.0 + gap * c, 30.0 - gap * sn)];
        let mut s = Script::new(pinch_settings());
        hand(&mut s, 0, &row(12.0));
        hand(&mut s, 10, &row(17.0));
        hand(&mut s, 20, &row(22.0));
        // Recognised on the first move, then keeps stepping as it opens further.
        assert!(
            !s.out.is_empty()
                && s.out.iter().all(|g| matches!(g, Gesture::Pinch { direction: PinchDirection::Out, .. }))
        );
        assert_eq!(s.out[0], Gesture::Pinch { fingers: 3, direction: PinchDirection::Out, step: 0 });
    }

    #[test]
    fn a_thumb_led_pinch_is_a_pinch_not_a_swipe() {
        // Three fingers stay put; the thumb sweeps 22 mm up towards them, as a
        // tilted hand does. The hand's centre moves too, but less than it closes.
        let fingers = [(50.0, 22.0), (62.0, 20.0), (74.0, 22.0)];
        let mut s = Script::new(four_finger_pinch());
        for (i, k) in [0.0f32, 0.35, 0.7, 1.0].iter().enumerate() {
            let thumb = (24.0 + 14.0 * k, 52.0 - 17.0 * k);
            hand(&mut s, i as u64 * 10, &[thumb, fingers[0], fingers[1], fingers[2]]);
        }
        assert_eq!(s.out.first(), Some(&Gesture::Pinch { fingers: 4, direction: PinchDirection::In, step: 0 }));
        assert!(!s.out.iter().any(|g| matches!(g, Gesture::Swipe { .. })), "{:?}", s.out);
    }

    #[test]
    fn a_fast_pinch_in_one_frame_counts() {
        // The whole pinch out between two frames 8 ms apart.
        let mut s = Script::new(pinch_settings());
        s.spread(0, 3, 10.0).spread(8, 3, 30.0);
        assert_eq!(s.out, vec![Gesture::Pinch { fingers: 3, direction: PinchDirection::Out, step: 0 }]);
    }

    #[test]
    fn a_fast_pinch_in_that_merges_fingers_counts() {
        // Pinching in so hard that the pad merges two contacts half way.
        let mut s = Script::new(pinch_settings());
        s.spread(0, 3, 20.0).spread(8, 3, 17.0);
        hand(&mut s, 16, &[(35.0, 30.0), (50.0, 30.0)]); // one finger gone
        s.lift(30);
        assert_eq!(s.out, vec![Gesture::Pinch { fingers: 3, direction: PinchDirection::In, step: 0 }]);
    }

    #[test]
    fn a_wobbly_swipe_is_still_a_swipe() {
        // Fingers drift apart by a couple of millimetres while swiping up.
        let mut s = Script::new(pinch_settings());
        let row = |gap: f32, y: f32| [(35.0 - gap, 30.0 + y), (50.0, 30.0 + y), (65.0 + gap, 30.0 + y)];
        hand(&mut s, 0, &row(0.0, 0.0));
        hand(&mut s, 10, &row(0.8, -6.0));
        hand(&mut s, 20, &row(1.5, -13.0));
        assert_eq!(s.out, vec![Gesture::Swipe { fingers: 3, direction: Direction::Up, step: 0 }]);
    }

    #[test]
    fn pinching_fingers_do_not_hold() {
        let mut s = Script::new(settings());
        s.spread(0, 3, 15.0).spread(300, 3, 22.0).tick(700);
        assert!(s.out.is_empty(), "spread changed by more than the slop: {:?}", s.out);
    }

    fn multi_tap_settings(most: u8) -> RecognizerSettings {
        let mut s = settings();
        s.multi_tap_fingers[3] = most;
        s
    }

    /// A quick three-finger tap that lifts 50 ms after landing.
    fn tap(s: &mut Script, t: u64) {
        s.frame(t, 3, 0.0, 0.0).lift(t + 50);
    }

    #[test]
    fn double_tap_replaces_two_single_taps() {
        let mut s = Script::new(multi_tap_settings(2));
        tap(&mut s, 0);
        assert!(s.out.is_empty(), "the first tap waits for a possible second");
        tap(&mut s, 200);
        assert_eq!(s.out, vec![Gesture::DoubleTap { fingers: 3 }]);
        s.tick(2000);
        assert_eq!(s.out.len(), 1, "nothing left over");
    }

    #[test]
    fn a_lone_tap_is_reported_once_the_window_passes() {
        let mut s = Script::new(multi_tap_settings(2));
        tap(&mut s, 0);
        s.tick(300);
        assert!(s.out.is_empty(), "still inside the 300 ms window");
        s.tick(351);
        assert_eq!(s.out, vec![Gesture::Tap { fingers: 3 }]);
    }

    #[test]
    fn taps_too_far_apart_stay_single() {
        let mut s = Script::new(multi_tap_settings(2));
        tap(&mut s, 0);
        tap(&mut s, 700);
        assert_eq!(s.out, vec![Gesture::Tap { fingers: 3 }], "the first flushes when the late one lands");
        s.tick(5000);
        assert_eq!(s.out, vec![Gesture::Tap { fingers: 3 }, Gesture::Tap { fingers: 3 }]);
    }

    #[test]
    fn triple_tap_and_double_when_the_third_never_comes() {
        let mut s = Script::new(multi_tap_settings(3));
        tap(&mut s, 0);
        tap(&mut s, 150);
        tap(&mut s, 300);
        assert_eq!(s.out, vec![Gesture::TripleTap { fingers: 3 }]);

        let mut s = Script::new(multi_tap_settings(3));
        tap(&mut s, 0);
        tap(&mut s, 150);
        assert!(s.out.is_empty());
        s.tick(600);
        assert_eq!(s.out, vec![Gesture::DoubleTap { fingers: 3 }]);
    }

    #[test]
    fn a_swipe_after_a_tap_reports_the_tap_first() {
        let mut s = Script::new(multi_tap_settings(2));
        tap(&mut s, 0);
        s.frame(100, 3, 0.0, 0.0).frame(110, 3, 0.0, -12.0);
        assert_eq!(
            s.out,
            vec![Gesture::Tap { fingers: 3 }, Gesture::Swipe { fingers: 3, direction: Direction::Up, step: 0 }]
        );
    }

    #[test]
    fn taps_without_multi_tap_bindings_fire_immediately() {
        let mut s = Script::new(settings());
        tap(&mut s, 0);
        tap(&mut s, 100);
        assert_eq!(s.out, vec![Gesture::Tap { fingers: 3 }, Gesture::Tap { fingers: 3 }]);
    }

    #[test]
    fn next_deadline_only_while_something_is_pending() {
        let mut s = Script::new(multi_tap_settings(2));
        assert_eq!(s.rec.next_deadline(), None, "idle: sleep until the next frame");
        s.frame(0, 3, 0.0, 0.0);
        assert_eq!(s.rec.next_deadline(), Some(500), "resting fingers: the hold timer");
        s.frame(10, 3, 6.0, 0.0);
        assert_eq!(s.rec.next_deadline(), None, "moved too far to hold");
        s.lift(40);
        s.frame(1000, 3, 0.0, 0.0).lift(1050);
        assert_eq!(s.rec.next_deadline(), Some(1351), "a tap waiting for a second one");
        s.tick(1351);
        assert_eq!(s.rec.next_deadline(), None);
    }

    /// Frames come from device drivers: odd finger counts, duplicate ids,
    /// absurd or NaN positions and time jumps must never panic, and no frame
    /// may produce a flood of gestures.
    #[test]
    fn fuzzed_frames_never_panic() {
        let settings = RecognizerSettings {
            drag_fingers: [false, false, false, true, false, false],
            pinch_fingers: [false, false, false, false, true, true],
            multi_tap_fingers: [0, 0, 0, 3, 2, 1],
            ..RecognizerSettings::default()
        };
        let mut rec = Recognizer::new(settings);
        let mut rng = crate::Rng::new(0xA076_1D64_78BD_642F);
        let mut t: u64 = 1_000;
        for _ in 0..50_000 {
            // Mostly small steps forward; sometimes a long gap or a step back.
            t = match rng.below(50) {
                0 => t.saturating_sub(rng.below(5_000)),
                1 => t + rng.below(100_000),
                _ => t + rng.below(20),
            };
            let n = [0, 0, 1, 2, 3, 3, 4, 4, 5, 5, 6, 10][rng.below(12) as usize];
            let contacts = (0..n)
                .map(|i| Contact { id: if rng.below(10) == 0 { 7 } else { i }, x: rng.mm(), y: rng.mm() })
                .collect();
            let gestures = rec.update(&TouchFrame { time_ms: t, contacts, width_mm: 100.0, height_mm: 70.0 });
            assert!(gestures.len() <= 2 * MAX_STEPS_PER_FRAME as usize, "{} gestures from one frame", gestures.len());
            if rng.below(5) == 0 {
                let due = rec.next_deadline().unwrap_or(t);
                assert!(rec.tick(due + rng.below(1_000)).len() <= 4);
            }
        }
        rec.tick(u64::MAX);
        rec.reset();
    }

    #[test]
    fn an_impossible_jump_is_capped_and_dropped() {
        let mut progress = 1e9;
        assert_eq!(take_steps(&mut progress, 7.0), MAX_STEPS_PER_FRAME);
        assert_eq!(progress, 0.0);
        let mut progress = f32::INFINITY;
        assert_eq!(take_steps(&mut progress, 7.0), MAX_STEPS_PER_FRAME);
        let mut progress = f32::NAN;
        assert_eq!(take_steps(&mut progress, 7.0), 0);
        let mut progress = -15.0;
        assert_eq!((take_steps(&mut progress, 7.0), progress), (2, -1.0));
    }

    /// [`Script::frame`] with three fingers, the first at `bad` (NaN,
    /// infinite, absurd): a frame a broken driver might send mid-touch.
    fn broken_frame(s: &mut Script, t: u64, bad: f32, x: f32, y: f32) {
        hand(s, t, &[(bad, bad), (35.0 + x, 30.0 + y), (50.0 + x, 30.0 + y)]);
    }

    #[test]
    fn a_broken_position_fires_nothing() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e9, -3e38] {
            for cfg in [settings(), pinch_settings(), RecognizerSettings { drag_fingers: [true; 6], ..settings() }] {
                let mut s = Script::new(cfg);
                s.frame(0, 3, 0.0, 0.0).frame(20, 3, 0.2, 0.0);
                broken_frame(&mut s, 40, bad, 0.2, 0.0);
                s.frame(60, 3, 0.2, 0.0).frame(80, 3, 0.0, 0.0).lift(120);
                assert_eq!(s.out, vec![Gesture::Tap { fingers: 3 }], "{bad}: {:?}", s.out);
            }
        }
    }

    #[test]
    fn a_swipe_keeps_stepping_after_a_broken_frame() {
        let mut s = Script::new(settings());
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 0.0, -12.0);
        broken_frame(&mut s, 20, f32::NAN, 0.0, -12.0);
        s.frame(30, 3, 0.0, -12.0).frame(40, 3, 0.0, -18.0).frame(50, 3, 0.0, -24.0);
        let up: Vec<_> = s.out.iter().map(|g| matches!(g, Gesture::Swipe { direction: Direction::Up, .. })).collect();
        assert_eq!(up, vec![true, true, true], "{:?}", s.out);
    }

    #[test]
    fn a_drag_never_moves_by_nan() {
        let mut s = Script::new(RecognizerSettings { drag_fingers: [true; 6], ..settings() });
        s.frame(0, 3, 0.0, 0.0).frame(10, 3, 12.0, 0.0);
        broken_frame(&mut s, 20, f32::INFINITY, 12.0, 0.0);
        s.frame(30, 3, 12.0, 0.0).frame(40, 3, 16.0, 0.0).lift(50);
        for g in &s.out {
            if let Gesture::DragMove { dx, dy } = g {
                assert!(dx.is_finite() && dy.is_finite(), "{:?}", s.out);
            }
        }
        assert_eq!(s.out.last(), Some(&Gesture::DragEnd));
        assert!(s.out.contains(&Gesture::DragMove { dx: 4.0, dy: 0.0 }), "moves again once positions are sane");
    }

    #[test]
    fn timestamps_at_the_end_of_time_do_not_overflow() {
        let end = u64::MAX - 5;
        let mut s = Script::new(RecognizerSettings { drag_release_ms: 300, ..multi_tap_settings(2) });
        s.frame(end - 100, 3, 0.0, 0.0);
        assert_eq!(s.rec.next_deadline(), Some(u64::MAX), "the hold timer saturates");
        s.lift(end - 60);
        assert_eq!(s.rec.next_deadline(), Some(u64::MAX), "so does the multi-tap timer");
        s.tick(u64::MAX);
        assert!(s.out.is_empty(), "a timer that can't run out doesn't: {:?}", s.out);

        let mut s = Script::new(RecognizerSettings { drag_release_ms: 300, drag_fingers: [true; 6], ..settings() });
        s.frame(end - 10, 3, 0.0, 0.0).frame(end - 5, 3, 12.0, 0.0).lift(end);
        assert_eq!(s.rec.next_deadline(), Some(u64::MAX), "and the drag release timer");
        s.tick(u64::MAX);
        s.out.extend(s.rec.reset());
        assert_eq!(s.out.last(), Some(&Gesture::DragEnd));
    }

    #[test]
    fn a_session_that_never_ends_keeps_its_memory_bounded() {
        // A resting finger while a driver hands out a new id every frame.
        let mut rec = Recognizer::new(settings());
        for i in 1..100_000u32 {
            let contacts = vec![Contact { id: 0, x: 20.0, y: 20.0 }, Contact { id: i, x: 60.0, y: 30.0 }];
            rec.update(&TouchFrame { time_ms: i as u64, contacts, width_mm: 100.0, height_mm: 60.0 });
        }
        assert!(rec.origins.len() <= MAX_ORIGINS, "{} origins", rec.origins.len());
        assert!(rec.prev.len() <= 2);
    }

    /// Random, often broken, frames with settings swapped and the session
    /// reset at random: the mouse button is never pressed twice, released
    /// without a press, or left held, and a drag only moves by finite amounts.
    #[test]
    fn soak_drags_always_end() {
        let mut rng = crate::Rng::new(0x9E37_79B9_7F4A_7C15);
        let drag = [false, false, false, true, true, false];
        let mut rec = Recognizer::new(RecognizerSettings { drag_fingers: drag, ..settings() });
        let (mut held, mut starts, mut t) = (false, 0, 1_000u64);
        let mut check = |gestures: Vec<Gesture>, held: &mut bool| {
            for g in gestures {
                match g {
                    Gesture::DragStart { .. } => {
                        assert!(!*held, "a second DragStart while dragging");
                        *held = true;
                        starts += 1;
                    }
                    Gesture::DragEnd => {
                        assert!(*held, "DragEnd without a drag");
                        *held = false;
                    }
                    Gesture::DragMove { dx, dy } => assert!(*held && dx.is_finite() && dy.is_finite(), "{g:?}"),
                    _ => {}
                }
            }
        };
        for _ in 0..100_000 {
            t = match rng.below(60) {
                0 => t.saturating_sub(rng.below(10_000)),
                1 => t.saturating_add(rng.below(1_000_000)),
                2 => u64::MAX - rng.below(1_000),
                3 => rng.below(1_000),
                _ => t.saturating_add(rng.below(20)),
            };
            match rng.below(400) {
                0 => {
                    let mut s = rec.settings().clone();
                    s.drag_fingers[3] = rng.below(2) == 0;
                    s.drag_fingers[4] = rng.below(2) == 0;
                    s.drag_release_ms = [0, 50, 350, 5_000][rng.below(4) as usize];
                    rec.set_settings(s);
                }
                1 => check(rec.reset(), &mut held),
                2 => check(rec.tick(t), &mut held),
                _ => {}
            }
            let n = [0, 0, 1, 2, 3, 3, 3, 4, 4, 5, 6][rng.below(11) as usize];
            let mut x = 20.0 + rng.below(40) as f32;
            let contacts = (0..n)
                .map(|i| {
                    x += rng.below(3) as f32;
                    let (x, y) = if rng.below(50) == 0 { (rng.mm(), rng.mm()) } else { (x + i as f32 * 12.0, 30.0) };
                    Contact { id: if rng.below(20) == 0 { 0 } else { i }, x, y }
                })
                .collect();
            let (w, h) = if rng.below(10) == 0 { (0.0, 0.0) } else { (100.0, 70.0) };
            check(rec.update(&TouchFrame { time_ms: t, contacts, width_mm: w, height_mm: h }), &mut held);
        }
        check(rec.tick(u64::MAX), &mut held);
        check(rec.reset(), &mut held);
        assert!(!held, "the button was left held");
        assert!(starts > 10, "the soak should drag now and then, dragged {starts} times");
    }

    /// Mixed sessions at 125 Hz: taps, swipes, pinches, drags and holds.
    fn mixed_frames(count: usize) -> Vec<TouchFrame> {
        let mut rng = crate::Rng::new(0xD1B5_4A32_D192_ED03);
        let mut out = Vec::with_capacity(count + 100);
        let mut t = 0u64;
        while out.len() < count {
            let n = 3 + rng.below(3) as u32;
            let kind = rng.below(5) as usize;
            for i in 0..[6, 30, 30, 40, 90][kind] {
                let k = i as f32;
                let (dx, dy, gap) = match kind {
                    1 => (0.0, -k * 1.5, 12.0),
                    2 => (0.0, 0.0, 12.0 + k * 0.8),
                    3 => (k * 1.2, k * 0.5, 12.0),
                    _ => (0.0, 0.0, 12.0),
                };
                let contacts = (0..n).map(|f| Contact { id: f, x: 20.0 + f as f32 * gap + dx, y: 40.0 + dy }).collect();
                out.push(TouchFrame { time_ms: t, contacts, width_mm: 105.0, height_mm: 68.0 });
                t += 8;
            }
            out.push(TouchFrame { time_ms: t, ..Default::default() });
            t += 200;
        }
        out
    }

    #[test]
    fn a_hundred_thousand_frames_are_recognised_quickly() {
        let frames = mixed_frames(100_000);
        let mut cfg = crate::Config::default();
        cfg.set_binding(crate::Trigger::Pinch(4, PinchDirection::Out), crate::Action::Mute, false);
        cfg.set_binding(crate::Trigger::DoubleTap(5), crate::Action::Mute, false);
        let mut rec = Recognizer::new(cfg.recognizer_settings());
        let start = std::time::Instant::now();
        let mut gestures = 0;
        for f in &frames {
            gestures += rec.update(f).len();
            gestures += rec.tick(f.time_ms).len();
        }
        let took = start.elapsed();
        // A few milliseconds in release; debug builds on shared CI runners are much slower.
        let budget = std::time::Duration::from_millis(if cfg!(debug_assertions) { 5_000 } else { 250 });
        assert!(took < budget, "{} frames took {took:?}", frames.len());
        assert!(gestures > 1_000, "the frames should make gestures, made {gestures}");
    }

    /// Every frame on its own, not just the total: a rare slow frame is a
    /// stutter. Along with the recognizer, each frame also feeds the
    /// two-finger stroke tracker, as in the engine.
    #[test]
    fn no_frame_takes_long() {
        let frames = mixed_frames(50_000);
        let mut cfg = crate::Config::default();
        cfg.set_binding(crate::Trigger::Pinch(4, PinchDirection::Out), crate::Action::Mute, false);
        let mut rec = Recognizer::new(cfg.recognizer_settings());
        let mut strokes = crate::glyphs::StrokeTracker::default();
        let mut times: Vec<std::time::Duration> = Vec::with_capacity(frames.len());
        for f in &frames {
            let start = std::time::Instant::now();
            std::hint::black_box(rec.update(f));
            std::hint::black_box(rec.tick(f.time_ms));
            std::hint::black_box(strokes.update(f));
            times.push(start.elapsed());
        }
        times.sort();
        let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
        let mean = times.iter().sum::<std::time::Duration>() / times.len() as u32;
        eprintln!("per frame: mean {mean:?}, median {:?}, p99 {:?}, max {:?}", at(0.5), at(0.99), at(1.0));
        // Touchpads send a frame every 7-8 ms; one frame's work is microseconds.
        let p99 = std::time::Duration::from_micros(if cfg!(debug_assertions) { 2_000 } else { 100 });
        assert!(at(0.99) < p99, "p99 {:?}", at(0.99));
    }
}
