//! Motion: what the pill looks like at any instant, and when the overlay
//! thread next has to wake up.
//!
//! No clocks or timers here, just times since an arbitrary epoch, so the
//! whole choreography (fade in, glide, linger, fade out, a new level
//! cancelling the fade-out, no wake-ups while hidden) is unit tested.

use pady_core::{Level, LevelKind};
use std::time::Duration;

/// Fade-in when the pill appears.
pub const FADE_IN: Duration = Duration::from_millis(100);
/// The bar and number glide to a new value.
pub const GLIDE: Duration = Duration::from_millis(120);
/// How long the pill stays after the last level.
pub const LINGER: Duration = Duration::from_millis(1100);
/// Fade-out once it has lingered.
pub const FADE_OUT: Duration = Duration::from_millis(180);
/// Frame interval while anything moves.
pub const FRAME: Duration = Duration::from_millis(16);

/// Quick start, gentle landing (CSS `ease-out`).
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Gentle at both ends, so the fade-out doesn't look like a blink.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `d × k` for `k` in 0–1, without `Duration::mul_f32`'s panics.
fn portion(d: Duration, k: f32) -> Duration {
    let k = if k.is_nan() { 0.0 } else { k.clamp(0.0, 1.0) };
    Duration::from_nanos((d.as_nanos() as f64 * k as f64) as u64)
}

/// A value moving from `from` to `to` over `duration` from `start`.
#[derive(Clone, Copy, Debug)]
struct Tween {
    from: f32,
    to: f32,
    start: Duration,
    duration: Duration,
    ease: fn(f32) -> f32,
}

impl Tween {
    fn hold(value: f32) -> Tween {
        Tween::new(value, value, Duration::ZERO, Duration::ZERO, ease_out_cubic)
    }

    fn new(from: f32, to: f32, start: Duration, duration: Duration, ease: fn(f32) -> f32) -> Tween {
        Tween { from, to, start, duration, ease }
    }

    fn at(&self, now: Duration) -> f32 {
        let elapsed = now.saturating_sub(self.start);
        if elapsed >= self.duration {
            return self.to;
        }
        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        self.from + (self.to - self.from) * (self.ease)(t)
    }

    fn running(&self, now: Duration) -> bool {
        self.from != self.to && now.saturating_sub(self.start) < self.duration
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Hidden,
    /// Fading in or fully shown, until the linger runs out.
    Shown,
    /// Fading out; hidden once that ends.
    Leaving,
}

/// What to draw at one instant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub kind: LevelKind,
    /// The muted state being shown (picks the icon).
    pub muted: bool,
    /// Window opacity, 0–1.
    pub opacity: f32,
    /// Bar position, 0–100, fractional while gliding.
    pub percent: f32,
    /// 0 = normal colours, 1 = muted colours.
    pub mute: f32,
}

impl Frame {
    /// The number shown, which counts along with the bar.
    pub fn number(&self) -> u8 {
        self.percent.round().clamp(0.0, 100.0) as u8
    }
}

/// The pill's state over time.
#[derive(Debug)]
pub struct Animator {
    phase: Phase,
    level: Level,
    opacity: Tween,
    value: Tween,
    mute: Tween,
    hide_at: Duration,
}

impl Default for Animator {
    fn default() -> Self {
        Animator {
            phase: Phase::Hidden,
            level: Level { kind: LevelKind::Volume, percent: 0, muted: false },
            opacity: Tween::hold(0.0),
            value: Tween::hold(0.0),
            mute: Tween::hold(0.0),
            hide_at: Duration::ZERO,
        }
    }
}

/// Brightness has no mute, whatever the flag says.
fn is_muted(level: &Level) -> bool {
    level.kind == LevelKind::Volume && level.muted
}

impl Animator {
    pub fn is_visible(&self) -> bool {
        self.phase != Phase::Hidden
    }

    /// Take a new level. `glide: false` (Windows animations turned off)
    /// jumps to new values instead of gliding; fades stay. Returns `true` if
    /// the pill is appearing, so the caller places and shows the window.
    pub fn show(&mut self, level: Level, now: Duration, glide: bool) -> bool {
        let percent = level.percent.min(100) as f32;
        let mute = if is_muted(&level) { 1.0 } else { 0.0 };
        let appearing = self.phase == Phase::Hidden;
        if appearing {
            // Start at the new level rather than sliding from a stale one (as overlay.js does).
            self.value = Tween::hold(percent);
            self.mute = Tween::hold(mute);
            self.opacity = Tween::new(0.0, 1.0, now, FADE_IN, ease_out_cubic);
        } else {
            let current = self.frame(now);
            let glide = if glide { GLIDE } else { Duration::ZERO };
            if level.percent != self.level.percent {
                self.value = Tween::new(current.percent, percent, now, glide, ease_out_cubic);
            }
            if is_muted(&level) != is_muted(&self.level) {
                self.mute = Tween::new(current.mute, mute, now, glide, ease_out_cubic);
            }
            if self.phase == Phase::Leaving {
                // Come back from wherever the fade-out got to.
                let back = portion(FADE_IN, 1.0 - current.opacity);
                self.opacity = Tween::new(current.opacity, 1.0, now, back, ease_out_cubic);
            }
        }
        self.phase = Phase::Shown;
        self.level = level;
        self.hide_at = now.saturating_add(LINGER);
        appearing
    }

    /// Advance the phases: start fading out once the linger is over, and
    /// become hidden when the fade-out ends.
    pub fn tick(&mut self, now: Duration) {
        if self.phase == Phase::Shown && now >= self.hide_at {
            let from = self.opacity.at(now);
            self.opacity = Tween::new(from, 0.0, now, portion(FADE_OUT, from), ease_in_out);
            self.phase = Phase::Leaving;
        }
        if self.phase == Phase::Leaving && !self.opacity.running(now) {
            self.phase = Phase::Hidden;
        }
    }

    pub fn frame(&self, now: Duration) -> Frame {
        Frame {
            kind: self.level.kind,
            muted: is_muted(&self.level),
            opacity: if self.is_visible() { self.opacity.at(now).clamp(0.0, 1.0) } else { 0.0 },
            percent: self.value.at(now),
            mute: self.mute.at(now),
        }
    }

    /// How long the thread may sleep: a frame while anything moves, until
    /// the fade-out while the pill rests, and forever while it's hidden.
    pub fn next_wake(&self, now: Duration) -> Option<Duration> {
        match self.phase {
            Phase::Hidden => None,
            _ if [self.opacity, self.value, self.mute].iter().any(|t| t.running(now)) => Some(FRAME),
            Phase::Leaving => Some(Duration::ZERO),
            Phase::Shown => Some(self.hide_at.saturating_sub(now)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn volume(percent: u8, muted: bool) -> Level {
        Level { kind: LevelKind::Volume, percent, muted }
    }

    /// Ticks every frame from `from` to `to`, like the overlay thread.
    fn run(a: &mut Animator, from: u64, to: u64) {
        let mut t = from;
        while t <= to {
            a.tick(ms(t));
            t += 16;
        }
    }

    #[test]
    fn easing_ends_and_shapes() {
        for ease in [ease_out_cubic, ease_in_out] {
            assert_eq!(ease(0.0), 0.0);
            assert_eq!(ease(1.0), 1.0);
            assert_eq!(ease(-1.0), 0.0);
            assert_eq!(ease(2.0), 1.0);
            let samples: Vec<f32> = (0..=20).map(|i| ease(i as f32 / 20.0)).collect();
            assert!(samples.windows(2).all(|w| w[0] <= w[1]), "monotonic");
        }
        // Ease-out covers most of the way early; ease-in-out is symmetric.
        assert!(ease_out_cubic(0.5) > 0.85);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn portion_never_panics() {
        assert_eq!(portion(ms(100), 0.5), ms(50));
        assert_eq!(portion(ms(100), -0.1), ms(0));
        assert_eq!(portion(ms(100), 1.0000001), ms(100));
        assert_eq!(portion(ms(100), f32::NAN), ms(0));
    }

    #[test]
    fn appears_at_the_new_level_and_fades_in() {
        let mut a = Animator::default();
        assert!(a.show(volume(64, false), ms(1000), true));
        let f = a.frame(ms(1000));
        assert_eq!((f.percent, f.number(), f.opacity), (64.0, 64, 0.0));
        let mid = a.frame(ms(1050)).opacity;
        assert!(mid > 0.5 && mid < 1.0, "{mid}");
        assert_eq!(a.frame(ms(1100)).opacity, 1.0);
        // A second level while visible isn't an appearance.
        assert!(!a.show(volume(66, false), ms(1200), true));
    }

    #[test]
    fn glides_from_the_previous_value() {
        let mut a = Animator::default();
        a.show(volume(40, false), ms(0), true);
        a.show(volume(60, false), ms(500), true);
        assert_eq!(a.frame(ms(500)).percent, 40.0);
        let mid = a.frame(ms(560));
        assert!(mid.percent > 50.0 && mid.percent < 60.0, "{mid:?}");
        assert_eq!(mid.number(), mid.percent.round() as u8);
        assert_eq!(a.frame(ms(620)).percent, 60.0);
        // Retargeting mid-glide starts from where the bar is, without a jump.
        a.show(volume(80, false), ms(800), true);
        a.show(volume(20, false), ms(830), true);
        let at = a.frame(ms(830)).percent;
        assert!(at > 60.0 && at < 80.0, "{at}");
    }

    #[test]
    fn jumps_when_animations_are_off() {
        let mut a = Animator::default();
        a.show(volume(40, false), ms(0), false);
        a.show(volume(60, true), ms(300), false);
        let f = a.frame(ms(300));
        assert_eq!((f.percent, f.mute, f.muted), (60.0, 1.0, true));
    }

    #[test]
    fn mute_blends_colours_and_switches_the_icon_at_once() {
        let mut a = Animator::default();
        a.show(volume(50, false), ms(0), true);
        a.show(volume(50, true), ms(400), true);
        let f = a.frame(ms(460));
        assert!(f.muted && f.mute > 0.0 && f.mute < 1.0, "{f:?}");
        assert_eq!(a.frame(ms(520)).mute, 1.0);
    }

    #[test]
    fn brightness_is_never_muted() {
        let mut a = Animator::default();
        a.show(Level { kind: LevelKind::Brightness, percent: 70, muted: true }, ms(0), true);
        let f = a.frame(ms(200));
        assert_eq!((f.kind, f.muted, f.mute), (LevelKind::Brightness, false, 0.0));
    }

    #[test]
    fn lingers_then_fades_out_and_hides() {
        let mut a = Animator::default();
        a.show(volume(30, false), ms(0), true);
        run(&mut a, 16, 1090);
        assert!(a.is_visible());
        assert_eq!(a.frame(ms(1090)).opacity, 1.0);
        a.tick(ms(1100));
        let fading = a.frame(ms(1190)).opacity;
        assert!(fading > 0.0 && fading < 1.0, "{fading}");
        a.tick(ms(1279));
        assert!(a.is_visible());
        a.tick(ms(1280));
        assert!(!a.is_visible());
        assert_eq!(a.frame(ms(1280)).opacity, 0.0);
        assert_eq!(a.next_wake(ms(1280)), None);
    }

    #[test]
    fn linger_restarts_with_every_level() {
        let mut a = Animator::default();
        a.show(volume(30, false), ms(0), true);
        a.show(volume(31, false), ms(1000), true);
        a.tick(ms(1500));
        assert_eq!(a.frame(ms(1500)).opacity, 1.0);
        assert_eq!(a.next_wake(ms(1500)), Some(ms(600)));
    }

    #[test]
    fn a_level_during_the_fade_out_cancels_it() {
        let mut a = Animator::default();
        a.show(volume(30, false), ms(0), true);
        a.tick(ms(1100));
        a.tick(ms(1190));
        let low = a.frame(ms(1190)).opacity;
        assert!(!a.show(volume(32, false), ms(1190), true), "not a new appearance: no reposition");
        assert_eq!(a.frame(ms(1190)).opacity, low);
        run(&mut a, 1190, 1400);
        assert!(a.is_visible());
        assert_eq!(a.frame(ms(1400)).opacity, 1.0);
        // And it lingers a full 1.1 s from that level.
        run(&mut a, 1400, 2280);
        assert!(a.is_visible());
        run(&mut a, 2290, 2490);
        assert!(!a.is_visible());
    }

    #[test]
    fn wakes_per_frame_only_while_something_moves() {
        let mut a = Animator::default();
        assert_eq!(a.next_wake(ms(0)), None, "hidden: no timer at all");
        a.show(volume(10, false), ms(0), true);
        assert_eq!(a.next_wake(ms(50)), Some(FRAME), "fading in");
        assert_eq!(a.next_wake(ms(100)), Some(ms(1000)), "resting: sleep until the fade-out");
        a.show(volume(20, false), ms(200), true);
        assert_eq!(a.next_wake(ms(250)), Some(FRAME), "gliding");
        assert_eq!(a.next_wake(ms(320)), Some(ms(980)));
        // The same level again moves nothing; it only pushes the fade-out back.
        a.show(volume(20, false), ms(400), true);
        assert_eq!(a.next_wake(ms(400)), Some(ms(1100)));
        a.tick(ms(1500));
        assert_eq!(a.next_wake(ms(1520)), Some(FRAME), "fading out");
        a.tick(ms(1700));
        assert_eq!(a.next_wake(ms(1700)), None);
    }
}
