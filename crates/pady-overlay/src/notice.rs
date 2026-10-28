//! Motion for the corner notices ("Copied", "Gestures paused"), as plain
//! functions of time so it is unit tested on every platform.
//!
//! A notice slides down a few pixels while fading in, rests, then fades out
//! drifting up. Everything is computed from the time since each phase began,
//! so frames that arrive late never make the motion stutter or speed up.

use std::time::Duration;

pub const ENTER: Duration = Duration::from_millis(220);
pub const LEAVE: Duration = Duration::from_millis(240);
/// Logical pixels travelled while entering and leaving.
pub const SLIDE_IN: f32 = 10.0;
pub const SLIDE_OUT: f32 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Hidden,
    Entering { since: Duration },
    Resting { until: Duration },
    Leaving { since: Duration },
}

/// What to show at one moment: opacity, and the vertical offset in logical
/// pixels (negative is up).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub opacity: f32,
    pub dy: f32,
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ease_in(t: f32) -> f32 {
    t * t * t
}

fn progress(now: Duration, since: Duration, length: Duration) -> f32 {
    (now.saturating_sub(since).as_secs_f32() / length.as_secs_f32()).clamp(0.0, 1.0)
}

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub phase: Phase,
    /// How long a notice rests before leaving.
    pub rest: Duration,
    /// Windows' animation effects; off means fades only, no sliding.
    pub slide: bool,
}

impl Default for Motion {
    fn default() -> Self {
        Motion { phase: Phase::Hidden, rest: Duration::from_millis(1400), slide: true }
    }
}

impl Motion {
    /// A notice arrives (again). Returns `true` if it was hidden, so the
    /// caller places the window before the first frame.
    pub fn show(&mut self, now: Duration, rest: Duration) -> bool {
        self.rest = rest;
        match self.phase {
            Phase::Hidden => {
                self.phase = Phase::Entering { since: now };
                true
            }
            // Already arriving: let it finish; the rest starts afterwards.
            Phase::Entering { .. } => false,
            Phase::Resting { .. } => {
                self.phase = Phase::Resting { until: now + rest };
                false
            }
            // Leaving: turn back from wherever it is, without a jump.
            Phase::Leaving { .. } => {
                let shown = self.pose(now).opacity;
                let back = ENTER.mul_f32(shown.clamp(0.0, 1.0));
                self.phase = Phase::Entering { since: now.saturating_sub(back) };
                false
            }
        }
    }

    /// Moves to the next phase when the current one is over.
    pub fn tick(&mut self, now: Duration) {
        self.phase = match self.phase {
            Phase::Entering { since } if now >= since + ENTER => Phase::Resting { until: now + self.rest },
            Phase::Resting { until } if now >= until => Phase::Leaving { since: now },
            Phase::Leaving { since } if now >= since + LEAVE => Phase::Hidden,
            phase => phase,
        };
    }

    pub fn pose(&self, now: Duration) -> Pose {
        let slide = if self.slide { 1.0 } else { 0.0 };
        match self.phase {
            Phase::Hidden => Pose { opacity: 0.0, dy: 0.0 },
            Phase::Entering { since } => {
                let t = ease_out(progress(now, since, ENTER));
                Pose { opacity: t, dy: -SLIDE_IN * (1.0 - t) * slide }
            }
            Phase::Resting { .. } => Pose { opacity: 1.0, dy: 0.0 },
            Phase::Leaving { since } => {
                let t = ease_in(progress(now, since, LEAVE));
                Pose { opacity: 1.0 - t, dy: -SLIDE_OUT * t * slide }
            }
        }
    }

    /// Frames are needed every refresh while moving.
    pub fn moving(&self) -> bool {
        matches!(self.phase, Phase::Entering { .. } | Phase::Leaving { .. })
    }

    /// When a resting notice should start leaving.
    pub fn rest_ends(&self) -> Option<Duration> {
        match self.phase {
            Phase::Resting { until } => Some(until),
            _ => None,
        }
    }

    pub fn visible(&self) -> bool {
        self.phase != Phase::Hidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Runs the motion at a steady frame rate from `from` to `to`.
    fn run(m: &mut Motion, from: u64, to: u64, step: u64) -> Vec<Pose> {
        (from..=to)
            .step_by(step as usize)
            .map(|t| {
                m.tick(ms(t));
                m.pose(ms(t))
            })
            .collect()
    }

    #[test]
    fn slides_in_rests_and_leaves() {
        let mut m = Motion::default();
        assert!(m.show(ms(0), ms(1000)));
        let poses = run(&mut m, 0, 1600, 8);
        assert_eq!(poses[0], Pose { opacity: 0.0, dy: -SLIDE_IN });
        // Fading in only ever brightens and slides down; then it rests in place.
        let entering: Vec<_> = poses.iter().take_while(|p| p.opacity < 1.0).collect();
        assert!(entering.windows(2).all(|w| w[1].opacity >= w[0].opacity && w[1].dy >= w[0].dy));
        assert_eq!(poses[700 / 8], Pose { opacity: 1.0, dy: 0.0 }, "resting in place");
        assert!(!m.visible(), "gone after enter + rest + leave");
    }

    #[test]
    fn a_late_frame_lands_where_the_clock_says() {
        // 60 fps and a frame that arrives 100 ms late reach the same pose.
        let mut smooth = Motion::default();
        smooth.show(ms(0), ms(1000));
        let mut janky = smooth;
        run(&mut smooth, 0, 150, 16);
        janky.tick(ms(150));
        assert_eq!(smooth.pose(ms(150)), janky.pose(ms(150)));
    }

    #[test]
    fn showing_again_extends_the_rest() {
        let mut m = Motion::default();
        m.show(ms(0), ms(1000));
        run(&mut m, 0, 600, 10);
        assert!(!m.show(ms(600), ms(1000)));
        assert_eq!(m.rest_ends(), Some(ms(1600)));
    }

    #[test]
    fn showing_while_leaving_turns_back_without_a_jump() {
        let mut m = Motion::default();
        m.show(ms(0), ms(200));
        run(&mut m, 0, 520, 4); // entered at 220, rest ends at 420, leaving since ~420
        let before = m.pose(ms(520)).opacity;
        assert!(before > 0.0 && before < 1.0, "half gone: {before}");
        m.show(ms(520), ms(1000));
        let after = m.pose(ms(520)).opacity;
        assert!((after - before).abs() < 0.15, "no jump: {before} → {after}");
        run(&mut m, 520, 800, 4);
        assert_eq!(m.pose(ms(800)).opacity, 1.0, "back to resting");
    }

    #[test]
    fn no_sliding_when_animations_are_off() {
        let mut m = Motion { slide: false, ..Motion::default() };
        m.show(ms(0), ms(500));
        assert_eq!(m.pose(ms(50)).dy, 0.0);
        assert!(m.pose(ms(50)).opacity > 0.0);
    }
}
