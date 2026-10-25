//! How long each system action takes on this machine:
//!
//!     cargo run -p pady-actions --release --example time_actions
//!
//! It really changes the volume, mute state and brightness, always in pairs
//! that cancel out (down then up, mute then unmute), so everything ends as it
//! started unless a level was already at 0%. The first call of each kind
//! includes connecting to the OS service.

use pady_core::{Action, Executor};
use std::time::{Duration, Instant};

fn time(exec: &mut dyn Executor, action: &Action) -> Result<Duration, String> {
    let start = Instant::now();
    exec.run(action).map_err(|e| e.to_string())?;
    Ok(start.elapsed())
}

/// Runs `there` and `back` alternately `pairs` times; prints first and typical cost.
fn pair(exec: &mut dyn Executor, name: &str, there: Action, back: Action, pairs: usize) {
    let mut samples = Vec::new();
    for _ in 0..pairs {
        for action in [&there, &back] {
            match time(exec, action) {
                Ok(d) => samples.push(d),
                Err(e) => {
                    println!("  {name:<22} unavailable here: {e}");
                    return;
                }
            }
        }
    }
    let first = samples[0];
    let mut rest = samples[1..].to_vec();
    rest.sort();
    println!(
        "  {name:<22} first {:>7.2} ms   then median {:>6.2} ms, max {:>6.2} ms   ({} calls)",
        ms(first),
        ms(rest[rest.len() / 2]),
        ms(*rest.last().unwrap()),
        samples.len()
    );
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn main() {
    let mut exec = pady_actions::executor();
    println!("Action timings (changes are undone):");
    // Down first: levels often sit at 100%, where "up then down" wouldn't cancel.
    pair(exec.as_mut(), "volume -2 / +2", Action::Volume { step: -2 }, Action::Volume { step: 2 }, 20);
    pair(exec.as_mut(), "mute / unmute", Action::Mute, Action::Mute, 10);
    pair(exec.as_mut(), "brightness -10 / +10", Action::Brightness { step: -10 }, Action::Brightness { step: 10 }, 5);

    let start = Instant::now();
    let app = exec.foreground_app();
    println!("  {:<22} {:>7.2} ms   ({})", "app in front", ms(start.elapsed()), app.as_deref().unwrap_or("none"));
    let start = Instant::now();
    let game = exec.fullscreen_game();
    println!("  {:<22} {:>7.2} ms   ({game})", "full-screen game check", ms(start.elapsed()));
}
