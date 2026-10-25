//! Shows the native level pill the way Pady drives it: volume up and down,
//! mute and unmute, then brightness down and up (about 11 s per round).
//!
//! ```text
//! cargo run -p pady-overlay --example demo [-- <seconds> [<idle seconds>]]
//! ```
//!
//! Rounds repeat until `seconds` (default 10) have passed. `idle seconds`
//! then keeps the hidden overlay alive, to check that it costs no CPU.

use pady_core::Level;
use pady_overlay::Overlay;
use std::thread::sleep;
use std::time::{Duration, Instant};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("pady_overlay=debug")).init();
    let mut args = std::env::args().skip(1).map(|a| a.parse::<f64>().ok());
    let seconds = args.next().flatten().unwrap_or(10.0);
    let idle = args.next().flatten().unwrap_or(0.0);

    let Some(overlay) = Overlay::spawn() else {
        eprintln!("the native overlay isn't available here (it is Windows-only)");
        return;
    };
    println!("overlay running in process {}", std::process::id());
    run(overlay, seconds, idle);
    println!("done");
}

/// Plays rounds for `seconds`, idles, then drops the overlay (stopping its thread).
fn run(overlay: Overlay, seconds: f64, idle: f64) {
    let start = Instant::now();
    let mut round = 0;
    while start.elapsed().as_secs_f64() < seconds {
        round += 1;
        println!("round {round}");
        play_round(&overlay);
    }
    if idle > 0.0 {
        println!("idle for {idle} s");
        sleep(Duration::from_secs_f64(idle));
    }
}

/// The same sequence as the WebView overlay's browser preview (overlay.js).
fn play_round(overlay: &Overlay) {
    let wait = |ms| sleep(Duration::from_millis(ms));
    let (mut volume, mut brightness) = (36.0, 70.0);
    wait(300);
    for _ in 0..8 {
        volume += 4.0;
        overlay.show(Level::volume(volume, false));
        wait(130);
    }
    wait(1500); // long enough to linger and fade out
    for _ in 0..6 {
        volume -= 4.0;
        overlay.show(Level::volume(volume, false));
        wait(130);
    }
    wait(800); // still visible: muting greys it out in place
    overlay.show(Level::volume(volume, true));
    wait(1500);
    overlay.show(Level::volume(volume, false));
    wait(1500);
    for _ in 0..4 {
        brightness -= 10.0;
        overlay.show(Level::brightness(brightness));
        wait(180);
    }
    wait(600);
    for _ in 0..6 {
        brightness = f32::min(100.0, brightness + 10.0);
        overlay.show(Level::brightness(brightness));
        wait(180);
    }
    wait(1500);
}
