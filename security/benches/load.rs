//! Load, spike and soak measurements for Pady's engine:
//!
//!     cargo bench -p pady-security                       # about a minute
//!     PADY_SOAK_MINUTES=30 cargo bench -p pady-security  # plus a 30-minute soak
//!
//! A real touchpad sends 125-250 frames a second. This harness ramps the
//! frame rate far past that until something gives, and says what did:
//!
//! * the latency from the frame that completes a gesture to its action
//!   running, at 1x to 500x a touchpad's rate;
//! * the most frames a second the engine gets through, and what happens
//!   beyond that;
//! * how per-app profiles, config size and recorded glyphs scale;
//! * a burst of half a million frames, and how long recovery takes;
//! * (soak) memory and handles over time under steady load with the config
//!   changing and being saved, with every gesture accounted for.
//!
//! It only uses pady-core's public API, and no real input or actions.

use crossbeam_channel::{unbounded, Sender};
use pady_core::{
    backups, Action, ActionError, Config, Contact, Direction, Engine, Executor, Level, Profile, Recognizer,
    SharedConfig, TouchFrame, Trigger,
};
use pady_security::{footprint, Rng, Scratch};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn frame(t: u64, fingers: u32, x: f32, y: f32) -> TouchFrame {
    let contacts = (0..fingers).map(|i| Contact { id: i, x: 20.0 + x + 12.0 * i as f32, y: 30.0 + y }).collect();
    TouchFrame { time_ms: t, contacts, width_mm: 105.0, height_mm: 68.0 }
}

/// Records when each action ran.
struct Clock {
    ran: Arc<Mutex<Vec<Instant>>>,
    count: Arc<AtomicU64>,
}

impl Executor for Clock {
    fn run(&mut self, _: &Action) -> Result<Option<Level>, ActionError> {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.ran.lock().unwrap().push(Instant::now());
        Ok(None)
    }
    fn mouse_button(&mut self, _: bool) -> Result<(), ActionError> {
        Ok(())
    }
    fn mouse_move(&mut self, _: i32, _: i32) -> Result<(), ActionError> {
        Ok(())
    }
    fn foreground_app(&mut self) -> Option<String> {
        Some("code.exe".into())
    }
}

struct Rig {
    tx: Sender<TouchFrame>,
    engine: Engine,
    ran: Arc<Mutex<Vec<Instant>>>,
    count: Arc<AtomicU64>,
    shared: Arc<SharedConfig>,
}

fn tap_config() -> Config {
    let mut cfg = Config::default();
    cfg.bindings.clear();
    cfg.set_binding(Trigger::Tap(3), Action::Mute, false);
    cfg.set_binding(Trigger::Swipe(4, Direction::Up), Action::Volume { step: 2 }, true);
    cfg
}

fn rig(cfg: Config) -> Rig {
    let ran = Arc::new(Mutex::new(Vec::with_capacity(1 << 20)));
    let count = Arc::new(AtomicU64::new(0));
    let clock = Clock { ran: ran.clone(), count: count.clone() };
    let shared = Arc::new(SharedConfig::new(cfg));
    let (tx, rx) = unbounded();
    let engine = Engine::spawn(shared.clone(), rx, move || Box::new(clock), Arc::new(|_| {}));
    Rig { tx, engine, ran, count, shared }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn us(d: Duration) -> String {
    format!("{:>8.1}", d.as_secs_f64() * 1e6)
}

/// Three-finger taps (two frames each) offered at `rate` frames a second
/// for `seconds`. Returns the latencies lift → action, sorted, and the rate
/// actually offered.
fn paced_taps(rate: u64, seconds: f64) -> (Vec<Duration>, f64) {
    let r = rig(tap_config());
    let taps = ((rate as f64 * seconds) / 2.0) as usize;
    let gap = Duration::from_secs_f64(1.0 / rate as f64);
    let mut sent = Vec::with_capacity(taps);
    let start = Instant::now();
    let mut due = start;
    let wait = |due: Instant| {
        // Sleep most of the way, then spin: sleeping alone is far too coarse.
        loop {
            let now = Instant::now();
            if now >= due {
                break;
            }
            if due - now > Duration::from_millis(2) {
                std::thread::sleep(due - now - Duration::from_millis(1));
            } else {
                std::hint::spin_loop();
            }
        }
    };
    for i in 0..taps as u64 {
        // Taps 400 ms apart on the touchpad's own clock, so each is its own gesture.
        let t = 10_000 + i * 400;
        wait(due);
        r.tx.send(frame(t, 3, 0.0, 0.0)).unwrap();
        due += gap;
        wait(due);
        sent.push(Instant::now());
        r.tx.send(frame(t + 40, 0, 0.0, 0.0)).unwrap();
        due += gap;
    }
    let offered = (taps * 2) as f64 / start.elapsed().as_secs_f64();
    drop(r.tx);
    r.engine.join();
    let ran = r.ran.lock().unwrap();
    assert_eq!(ran.len(), taps, "every tap ran its action");
    let mut waits: Vec<Duration> =
        ran.iter().zip(&sent).map(|(ran, sent)| ran.saturating_duration_since(*sent)).collect();
    waits.sort();
    (waits, offered)
}

fn ramp() {
    println!("\nLatency, lift to action running, as the frame rate rises (3 s at each rate):");
    println!("  {:<28} {:>10} {:>9} {:>9} {:>9} {:>9}", "offered", "frames/s", "p50 µs", "p95 µs", "p99 µs", "max µs");
    for (pads, rate) in [(1, 125u64), (10, 1_250), (50, 6_250), (100, 12_500), (500, 62_500)] {
        let (waits, offered) = paced_taps(rate, 3.0);
        println!(
            "  {:<28} {:>10.0} {} {} {} {}",
            format!("{pads} touchpad{} ({rate}/s)", if pads == 1 { "" } else { "s" }),
            offered,
            us(percentile(&waits, 0.50)),
            us(percentile(&waits, 0.95)),
            us(percentile(&waits, 0.99)),
            us(*waits.last().unwrap()),
        );
    }
}

/// Frames as fast as one thread can send them: what the engine gets through,
/// and what gives when it is offered more.
fn overload() {
    println!("\nOverload: frames sent as fast as possible (2,000,000 frames, taps):");
    let r = rig(tap_config());
    let before = footprint();
    let total = 2_000_000u64;
    let start = Instant::now();
    let mut peak_queue = 0usize;
    for i in 0..total / 2 {
        let t = 10_000 + i * 400;
        r.tx.send(frame(t, 3, 0.0, 0.0)).unwrap();
        r.tx.send(frame(t + 40, 0, 0.0, 0.0)).unwrap();
        if i % 4096 == 0 {
            peak_queue = peak_queue.max(r.tx.len());
        }
    }
    let sent_in = start.elapsed();
    let peak = footprint();
    drop(r.tx);
    r.engine.join();
    let done_in = start.elapsed();
    let ran = r.count.load(Ordering::Relaxed);
    println!("  sent at          {:>12.0} frames/s", total as f64 / sent_in.as_secs_f64());
    println!(
        "  processed at     {:>12.0} frames/s (all {ran} gestures ran their action)",
        total as f64 / done_in.as_secs_f64()
    );
    println!("  frames waiting   {:>12} at the worst moment", peak_queue);
    println!("  memory           {:>12} KB before, {} KB at the peak", before.memory >> 10, peak.memory >> 10);
    assert_eq!(ran, total / 2, "no gesture was lost under overload");
}

fn recognizer_throughput() {
    println!("\nRecognizer alone, mixed gestures (2,000,000 frames):");
    let mut rng = Rng(0xBEEF);
    let mut frames = Vec::with_capacity(2_000_100);
    let mut t = 1_000u64;
    while frames.len() < 2_000_000 {
        let fingers = 1 + rng.below(5) as u32;
        let (dx, dy) = (rng.below(7) as f32 - 3.0, rng.below(7) as f32 - 3.0);
        for i in 0..(2 + rng.below(30)) {
            t += 8;
            frames.push(frame(t, fingers, dx * i as f32, dy * i as f32));
        }
        t += 300;
        frames.push(frame(t, 0, 0.0, 0.0));
    }
    let mut best = Duration::MAX;
    let mut gestures = 0;
    for _ in 0..3 {
        let mut rec = Recognizer::new(tap_config().recognizer_settings());
        let start = Instant::now();
        gestures = 0;
        for f in &frames {
            gestures += rec.update(f).len();
        }
        best = best.min(start.elapsed());
    }
    println!(
        "  {:.1} ns/frame, {:.1} million frames/s on one core ({gestures} gestures)",
        best.as_secs_f64() * 1e9 / frames.len() as f64,
        frames.len() as f64 / best.as_secs_f64() / 1e6
    );
}

fn profiles_scale() {
    println!("\nFinding the binding for the app in front, by number of profiles (20 apps each):");
    for count in [0usize, 1, 10, 50, 100, 200] {
        let mut cfg = tap_config();
        for i in 0..count {
            cfg.profiles.push(Profile {
                name: format!("profile {i}"),
                apps: (0..20).map(|a| format!("app-{i}-{a}.exe")).collect(),
                bindings: Vec::new(),
            });
        }
        cfg.normalize();
        let rounds = 20_000;
        let mut best = Duration::MAX;
        for _ in 0..3 {
            let start = Instant::now();
            for _ in 0..rounds {
                std::hint::black_box(cfg.resolve(Trigger::Tap(3), Some(r"C:\Program Files\Editor\not-listed.exe")));
            }
            best = best.min(start.elapsed());
        }
        println!(
            "  {:>4} profiles  {:>9.2} µs per gesture",
            cfg.profiles.len(),
            best.as_secs_f64() * 1e6 / rounds as f64
        );
    }
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

fn config_io() {
    println!("\nConfig on disk (median of 20):");
    let dir = Scratch::new("bench-io");
    for (label, cfg) in [
        ("default config", Config::default()),
        ("200 profiles x 20 apps", {
            let mut cfg = Config::default();
            for i in 0..200 {
                cfg.profiles.push(Profile {
                    name: format!("profile {i}"),
                    apps: (0..20).map(|a| format!("app-{i}-{a}.exe")).collect(),
                    bindings: Vec::new(),
                });
            }
            cfg.normalize();
            cfg
        }),
    ] {
        let path = dir.path(&format!("{}.toml", label.len()));
        let (mut saves, mut loads, mut copies) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..20 {
            let mut changed = cfg.clone();
            changed.settings.hold_ms = 600 + i;
            let t = Instant::now();
            changed.save(&path).unwrap();
            saves.push(t.elapsed());
            let t = Instant::now();
            std::hint::black_box(Config::load(&path).unwrap());
            loads.push(t.elapsed());
            let t = Instant::now();
            backups::snapshot(&path, true).unwrap();
            copies.push(t.elapsed());
        }
        let size = std::fs::metadata(&path).unwrap().len();
        println!(
            "  {label:<24} {:>7} bytes   save {} µs   load {} µs   backup {} µs",
            size,
            us(median(saves)),
            us(median(loads)),
            us(median(copies))
        );
    }
}

fn spike() {
    println!("\nSpike: 500,000 frames at once, then a tap every 8 ms:");
    let r = rig(tap_config());
    let start = Instant::now();
    for i in 0..250_000u64 {
        let t = 10_000 + i * 400;
        r.tx.send(frame(t, 3, 0.0, 0.0)).unwrap();
        r.tx.send(frame(t + 40, 0, 0.0, 0.0)).unwrap();
    }
    while r.count.load(Ordering::Relaxed) < 250_000 {
        std::thread::yield_now();
    }
    let drained = start.elapsed();
    let base = r.ran.lock().unwrap().len();
    let mut sent = Vec::new();
    for i in 0..200u64 {
        let t = 200_000_000 + i * 400;
        r.tx.send(frame(t, 3, 0.0, 0.0)).unwrap();
        std::thread::sleep(Duration::from_millis(8));
        sent.push(Instant::now());
        r.tx.send(frame(t + 40, 0, 0.0, 0.0)).unwrap();
        std::thread::sleep(Duration::from_millis(8));
    }
    drop(r.tx);
    r.engine.join();
    let ran = r.ran.lock().unwrap();
    let mut waits: Vec<Duration> =
        ran[base..].iter().zip(&sent).map(|(ran, sent)| ran.saturating_duration_since(*sent)).collect();
    waits.sort();
    println!("  the burst was worked off in {:.0} ms; nothing was lost", drained.as_secs_f64() * 1e3);
    println!(
        "  afterwards: p50 {} µs, p99 {} µs, max {} µs (no restart, no lingering backlog)",
        us(percentile(&waits, 0.5)).trim(),
        us(percentile(&waits, 0.99)).trim(),
        us(*waits.last().unwrap()).trim()
    );
}

/// Steady load for `minutes`: 1,000 frames a second (8 touchpads' worth) of
/// taps and repeating swipes, the config changed every second and saved and
/// backed up every five. Every tap must run exactly one action, the config
/// on disk must always read back whole, and memory and handles must stay flat.
fn soak(minutes: u64) {
    println!("\nSoak: {minutes} minutes at 1,000 frames/s, config changed every second and saved every 5 s:");
    let dir = Scratch::new("bench-soak");
    let path = dir.path("config.toml");
    let r = rig(tap_config());
    tap_config().save(&path).unwrap();
    let end = Instant::now() + Duration::from_secs(minutes * 60);
    let (mut t, mut taps, mut swipes, mut second) = (10_000u64, 0u64, 0u64, 0u64);
    let mut samples: Vec<(u64, pady_security::Footprint)> = Vec::new();
    let mut saves = 0u64;
    let started = Instant::now();
    println!("  {:>6} {:>12} {:>9} {:>12} {:>12} {:>8}", "minute", "memory KB", "handles", "taps", "actions", "saves");
    while Instant::now() < end {
        // One second of work: 100 taps (200 frames) and 20 swipes of 40 frames.
        let tick = Instant::now();
        for i in 0..100u64 {
            r.tx.send(frame(t, 3, 0.0, 0.0)).unwrap();
            r.tx.send(frame(t + 40, 0, 0.0, 0.0)).unwrap();
            t += 400;
            taps += 1;
            if i % 5 == 0 {
                for step in 0..39u64 {
                    r.tx.send(frame(t + step * 8, 4, 0.0, -(step as f32) * 1.5)).unwrap();
                }
                r.tx.send(frame(t + 320, 0, 0.0, 0.0)).unwrap();
                t += 800;
                swipes += 1;
            }
            std::thread::sleep(Duration::from_millis(9));
        }
        second += 1;
        // The config changes under the engine's feet, as the settings window does it.
        let config = r.shared.update(|c| c.settings.hold_ms = 500 + second % 200);
        if second % 5 == 0 {
            backups::snapshot(&path, false).unwrap();
            config.save(&path).unwrap();
            saves += 1;
            assert_eq!(Config::load(&path).unwrap(), config, "the config on disk reads back whole");
        }
        if second % 60 == 0 || second == 1 {
            let f = footprint();
            samples.push((second, f));
            println!(
                "  {:>6} {:>12} {:>9} {:>12} {:>12} {:>8}",
                second / 60,
                f.memory >> 10,
                f.handles,
                taps,
                r.count.load(Ordering::Relaxed),
                saves
            );
        }
        if let Some(rest) = Duration::from_secs(1).checked_sub(tick.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    let frames = taps * 2 + swipes * 40;
    drop(r.tx);
    r.engine.join();
    let actions = r.count.load(Ordering::Relaxed);
    let copies = backups::list(&path).len();
    let (first, last) = (samples[samples.len().min(2) - 1].1, samples.last().unwrap().1);
    println!(
        "  {} frames in {:.0} s; {taps} taps, {swipes} swipes, {actions} actions; {saves} saves, {copies} backups kept",
        frames,
        started.elapsed().as_secs_f64()
    );
    println!(
        "  memory {} -> {} KB, handles {} -> {}",
        first.memory >> 10,
        last.memory >> 10,
        first.handles,
        last.handles
    );
    // Every tap is one action; every swipe is at least its first step.
    assert!(actions >= taps + swipes, "{actions} actions for {taps} taps and {swipes} swipes: gestures were lost");
    assert!(copies <= backups::KEEP, "{copies} backups kept");
    assert!(last.memory <= first.memory + (16 << 20), "memory grew by {} KB", (last.memory - first.memory) >> 10);
    assert!(last.handles <= first.handles + 8, "handles grew from {} to {}", first.handles, last.handles);
    println!("  every gesture accounted for; memory and handles flat");
}

fn main() {
    // `cargo test` runs benches with --test to check that they build; do nothing then.
    if std::env::args().any(|a| a == "--test" || a == "--list") {
        return;
    }
    println!("Pady load test ({} build)", if cfg!(debug_assertions) { "debug" } else { "release" });
    recognizer_throughput();
    ramp();
    overload();
    profiles_scale();
    config_io();
    spike();
    if let Some(minutes) = std::env::var("PADY_SOAK_MINUTES").ok().and_then(|m| m.parse::<u64>().ok()) {
        soak(minutes.max(1));
    }
}
