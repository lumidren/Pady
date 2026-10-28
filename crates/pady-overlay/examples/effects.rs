//! Plays the swipe glow (one swipe per edge) and the show-touches pad, for
//! eyeballing the motion: `cargo run -p pady-overlay --example effects`.
//! With `RUST_LOG=pady_overlay=trace` every frame logs its timing.

use pady_core::{Contact, Direction, TouchFrame};
use pady_overlay::Effects;
use std::time::Duration;

fn main() {
    env_logger::init();
    let Some(fx) = Effects::spawn() else {
        eprintln!("these animations are only drawn on Windows");
        return;
    };
    let tick = Duration::from_millis(8); // a touchpad reporting at 125 Hz
    for (i, direction) in Direction::ALL.into_iter().enumerate() {
        for k in 0..=40 {
            fx.swipe_progress(direction, k as f32 / 40.0);
            std::thread::sleep(tick);
        }
        fx.swipe_ended(i != 1); // the second one is let go: it fades instead of flashing
        std::thread::sleep(Duration::from_millis(700));
    }
    for k in 0..240 {
        let a = k as f32 / 40.0;
        let contacts = (0..3)
            .map(|i| Contact { id: i, x: 50.0 + 25.0 * a.cos() + i as f32 * 12.0 - 12.0, y: 30.0 + 15.0 * a.sin() })
            .collect();
        fx.touches(&TouchFrame { time_ms: k * 8, contacts, width_mm: 100.0, height_mm: 60.0 });
        std::thread::sleep(tick);
    }
    fx.touches(&TouchFrame::default());
    std::thread::sleep(Duration::from_millis(1500));
}
