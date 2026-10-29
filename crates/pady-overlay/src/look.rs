//! How every animation looks, shared by the notice and effects threads: they
//! read it each frame, so a change in Settings shows on the next one.

use std::sync::atomic::{AtomicU32, Ordering};

/// Glow strength (0 = no glow), animation speed and how long notices stay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub glow: f32,
    pub speed: f32,
    pub stay: f32,
}

impl Default for Look {
    fn default() -> Self {
        Look { glow: 1.0, speed: 1.0, stay: 1.0 }
    }
}

static GLOW: AtomicU32 = AtomicU32::new(0x3f80_0000); // 1.0
static SPEED: AtomicU32 = AtomicU32::new(0x3f80_0000);
static STAY: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// Use `look` from the next frame on.
pub fn set_look(look: Look) {
    let fit = |v: f32, lo: f32, hi: f32| if v.is_finite() { v.clamp(lo, hi) } else { 1.0 };
    GLOW.store(fit(look.glow, 0.0, 2.0).to_bits(), Ordering::Relaxed);
    SPEED.store(fit(look.speed, 0.25, 4.0).to_bits(), Ordering::Relaxed);
    STAY.store(fit(look.stay, 0.25, 5.0).to_bits(), Ordering::Relaxed);
}

// Only the Windows-drawn animations read it.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn look() -> Look {
    let get = |a: &AtomicU32| f32::from_bits(a.load(Ordering::Relaxed));
    Look { glow: get(&GLOW), speed: get(&SPEED), stay: get(&STAY) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_normal_and_keeps_values_in_range() {
        assert_eq!(look(), Look::default());
        set_look(Look { glow: 9.0, speed: f32::NAN, stay: 0.0 });
        assert_eq!(look(), Look { glow: 2.0, speed: 1.0, stay: 0.25 });
        set_look(Look::default());
        assert_eq!(look(), Look::default());
    }
}
