//! Shows each corner notice once, for eyeballing the look and the motion:
//! `cargo run -p pady-overlay --example notices`.

use pady_overlay::{Notice, Toasts};
use std::time::Duration;

fn main() {
    env_logger::init();
    let Some(toasts) = Toasts::spawn() else {
        eprintln!("corner notices are only drawn on Windows");
        return;
    };
    toasts.show(Notice::Copied);
    std::thread::sleep(Duration::from_millis(2500));
    toasts.show(Notice::Paused);
    std::thread::sleep(Duration::from_millis(4000));
}
