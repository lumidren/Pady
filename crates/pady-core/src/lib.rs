//! Pady core: everything that does not depend on the operating system.
//!
//! Data flows in one direction:
//!
//! ```text
//! touchpad backend ──TouchFrame──▶ Recognizer ──Gesture──▶ Engine ──Action──▶ Executor
//! ```
//!
//! Backends (in `pady-input`) turn raw HID / evdev reports into [`TouchFrame`]s
//! measured in millimetres, so every threshold here is device independent.
//! Executors (in `pady-actions`) carry out [`Action`]s on the host OS.

pub mod action;
pub mod backups;
pub mod config;
pub mod engine;
pub mod frame;
pub mod gesture;
pub mod glyphs;
pub mod keys;
pub mod presets;
pub mod recognizer;
pub mod trigger;

pub use action::{Action, MediaKey};
pub use config::{Binding, Config, ConfigError, Profile, Resolution, Settings, SharedConfig};
pub use engine::{Ack, ActionError, Engine, EngineEvent, Executor, Job, Level, LevelKind, Scheduling};
pub use frame::{now_ms, Contact, TouchFrame};
pub use gesture::{Direction, Gesture, PinchDirection};
pub use glyphs::{Glyph, NotAGlyph};
pub use keys::{Key, KeyCombo, Modifiers};
pub use presets::{Preset, PRESETS};
pub use recognizer::{Recognizer, RecognizerSettings, SwipeProgress};
pub use trigger::Trigger;

/// Deterministic random input for the fuzz tests (xorshift64*): the same
/// cases on every run, with no dependencies.
#[cfg(test)]
pub(crate) struct Rng(u64);

#[cfg(test)]
impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A position in millimetres: usually on a pad, sometimes absurd (huge,
    /// negative, infinite or NaN) as a broken driver might report.
    pub(crate) fn mm(&mut self) -> f32 {
        match self.below(40) {
            0 => f32::NAN,
            1 => f32::INFINITY,
            2 => -1e9,
            3 => 1e9,
            _ => self.below(12_000) as f32 / 100.0,
        }
    }

    /// A string of `len` characters drawn from `alphabet`.
    pub(crate) fn text(&mut self, alphabet: &[char], len: usize) -> String {
        (0..len).map(|_| alphabet[self.below(alphabet.len() as u64) as usize]).collect()
    }
}
