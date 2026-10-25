use serde::Serialize;
use std::sync::OnceLock;
use std::time::Instant;

/// One finger touching the pad. Coordinates are millimetres from the pad's
/// top-left corner, so `y` grows downwards.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Contact {
    /// Stable identifier for as long as the finger stays down.
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

/// A snapshot of every finger currently on the pad.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TouchFrame {
    /// Milliseconds on the process-wide monotonic clock (see [`now_ms`]).
    pub time_ms: u64,
    pub contacts: Vec<Contact>,
    /// Physical size of the pad, used by the UI to draw it to scale.
    pub width_mm: f32,
    pub height_mm: f32,
}

impl TouchFrame {
    pub fn finger_count(&self) -> u8 {
        self.contacts.len().min(u8::MAX as usize) as u8
    }
}

/// Monotonic milliseconds since the first call. Backends stamp frames with
/// this and the engine uses it for timers, so both share one clock.
pub fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}
