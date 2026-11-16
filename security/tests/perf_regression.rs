//! Fails when a key performance number gets more than 20% worse than the
//! baseline in `security/perf-baseline.json`.
//!
//!     cargo test -p pady-security --test perf_regression                 # allocations (exact)
//!     cargo test -p pady-security --test perf_regression --release       # plus time
//!     PADY_PERF_RECORD=1 cargo test -p pady-security --test perf_regression --release
//!                                                                        # record a new baseline
//!
//! Two kinds of number are kept:
//!
//! * **Heap allocations per 1,000 touch frames**, for each kind of gesture.
//!   Counted exactly, so the same on every machine. Pointing and scrolling
//!   must stay at zero: Pady sees every frame the touchpad sends.
//! * **Time per frame**, relative to a fixed piece of arithmetic timed in the
//!   same run. The ratio, not the nanoseconds, is compared, so a slower
//!   machine doesn't fail the test. Only in release builds: unoptimised
//!   timings say little about what ships. The 20% limit holds on the
//!   machine the baseline was recorded on. On CI the timings are printed
//!   but never fail the build: GitHub's machines differ in processor and
//!   operating system from that one and from each other, and the same code
//!   measured anywhere from 13% under to 64% over the baseline there. A
//!   limit loose enough for that would catch nothing; set
//!   `PADY_PERF_ENFORCE_TIME=1` to enforce it anyway.

use pady_core::{Action, Config, Contact, Direction, Profile, Recognizer, TouchFrame, Trigger};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeMap;
use std::time::Instant;

/// Counts heap allocations per thread, so tests running side by side don't
/// count each other's.
struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

fn count() {
    // `try_with`: a thread that is shutting down may allocate after its
    // counter is gone.
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
}

fn allocated() -> u64 {
    ALLOCATIONS.with(Cell::get)
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        System.realloc(ptr, layout, size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// How much worse than the baseline a number may get.
const ALLOWED: f64 = 1.20;

fn frame(t: u64, fingers: u32, x: f32, y: f32) -> TouchFrame {
    let contacts = (0..fingers).map(|i| Contact { id: i, x: 20.0 + x + 12.0 * i as f32, y: 30.0 + y }).collect();
    TouchFrame { time_ms: t, contacts, width_mm: 105.0, height_mm: 68.0 }
}

/// 10,000 frames at 125 Hz: sessions of `fingers` moving by `step` per
/// frame for `length` frames, then lifting.
fn script(fingers: u32, length: u64, step: (f32, f32)) -> Vec<TouchFrame> {
    let mut frames = Vec::with_capacity(10_100);
    let mut t = 1_000u64;
    while frames.len() < 10_000 {
        for i in 0..length {
            t += 8;
            frames.push(frame(t, fingers, step.0 * i as f32, step.1 * i as f32));
        }
        t += 400;
        frames.push(frame(t, 0, 0.0, 0.0));
    }
    frames
}

fn config() -> Config {
    let mut cfg = Config::default();
    cfg.bindings.clear();
    cfg.set_binding(Trigger::Tap(3), Action::Mute, false);
    cfg.set_binding(Trigger::Swipe(4, Direction::Up), Action::Volume { step: 2 }, true);
    cfg.set_binding(Trigger::Swipe(4, Direction::Down), Action::Volume { step: -2 }, true);
    cfg.set_binding(Trigger::Drag(3), Action::Drag, false);
    for i in 0..20 {
        cfg.profiles.push(Profile {
            name: format!("profile {i}"),
            apps: (0..10).map(|a| format!("app-{i}-{a}.exe")).collect(),
            bindings: Vec::new(),
        });
    }
    cfg.normalize();
    cfg
}

fn scenarios() -> Vec<(&'static str, Vec<TouchFrame>)> {
    vec![
        ("pointing_1_finger", script(1, 60, (0.4, 0.2))),
        ("scrolling_2_fingers", script(2, 60, (0.0, 0.5))),
        ("tap_3_fingers", script(3, 3, (0.0, 0.0))),
        ("swipe_4_fingers_repeating", script(4, 40, (0.0, -1.5))),
        ("drag_3_fingers", script(3, 60, (0.6, 0.3))),
    ]
}

/// Allocations per 1,000 frames through the recognizer.
fn allocations(frames: &[TouchFrame]) -> f64 {
    let mut rec = Recognizer::new(config().recognizer_settings());
    // Once through to warm up buffers the recognizer keeps.
    for f in frames {
        std::hint::black_box(rec.update(f));
    }
    let before = allocated();
    for f in frames {
        std::hint::black_box(rec.update(f));
    }
    (allocated() - before) as f64 * 1000.0 / frames.len() as f64
}

/// Allocations per 1,000 lookups of the binding for the app in front.
fn lookup_allocations(cfg: &Config) -> f64 {
    let before = allocated();
    for _ in 0..1000 {
        std::hint::black_box(cfg.resolve(Trigger::Tap(3), Some(r"C:\Program Files\Editor\code.exe")));
    }
    (allocated() - before) as f64
}

/// A fixed piece of arithmetic, the yardstick the timings are divided by:
/// one timing of it, in seconds per round.
fn yardstick() -> f64 {
    let start = Instant::now();
    let (mut seed, mut sum) = (0x2545_F491_4F6C_DD1Du64, 0f32);
    for _ in 0..200_000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let (x, y) = ((seed & 0xFFFF) as f32 / 655.0, ((seed >> 16) & 0xFFFF) as f32 / 655.0);
        sum += x.hypot(y).min(80.0) - (x - y).abs();
    }
    std::hint::black_box(sum);
    start.elapsed().as_secs_f64() / 200_000.0
}

/// How long one of `items` pieces of `work` takes, in yardsticks. Each
/// repetition times the yardstick and then the work back to back, so both
/// run on the same core at the same clock speed (laptops mix fast and slow
/// cores and change speed all the time), and the median of the ratios is
/// what counts: one repetition disturbed by something else doesn't.
fn in_yardsticks(items: usize, mut work: impl FnMut()) -> f64 {
    let mut ratios: Vec<f64> = (0..31)
        .map(|_| {
            let unit = yardstick();
            let start = Instant::now();
            work();
            start.elapsed().as_secs_f64() / items as f64 / unit
        })
        .collect();
    ratios.sort_by(f64::total_cmp);
    ratios[ratios.len() / 2]
}

type Numbers = BTreeMap<String, f64>;

/// Allocations per 1,000 frames (or lookups), exact.
fn measure_allocations() -> Numbers {
    let mut allocs: Numbers =
        scenarios().iter().map(|(name, frames)| (name.to_string(), allocations(frames))).collect();
    allocs.insert("profile_lookup".into(), lookup_allocations(&config()));
    allocs
}

/// Time per frame (or lookup), in yardsticks.
fn measure_times() -> Numbers {
    let cfg = config();
    let settings = cfg.recognizer_settings();
    let mut ratios = Numbers::new();
    for (name, frames) in scenarios() {
        let time = in_yardsticks(frames.len(), || {
            let mut rec = Recognizer::new(settings.clone());
            for f in &frames {
                std::hint::black_box(rec.update(f));
            }
        });
        ratios.insert(name.to_string(), time);
    }
    let time = in_yardsticks(2_000, || {
        for _ in 0..2_000 {
            std::hint::black_box(cfg.resolve(Trigger::Tap(3), Some(r"C:\Program Files\Editor\not-listed.exe")));
        }
    });
    ratios.insert("profile_lookup".into(), time);
    ratios
}

fn baseline_path() -> std::path::PathBuf {
    pady_security::repo().join("security").join("perf-baseline.json")
}

#[test]
fn performance_has_not_regressed() {
    let allocs = measure_allocations();
    println!("allocations per 1,000 frames: {allocs:#?}");
    // Timings only mean something in a release build.
    let mut ratios = if cfg!(debug_assertions) { Numbers::new() } else { measure_times() };

    if std::env::var_os("PADY_PERF_RECORD").is_some() {
        assert!(!ratios.is_empty(), "record the baseline from a release build (--release)");
        println!("time per frame, in yardsticks: {ratios:#?}");
        let json = serde_json::json!({
            "about": "Baseline for security/tests/perf_regression.rs. Record with PADY_PERF_RECORD=1 (release build).",
            "allocations_per_1000_frames": allocs,
            "time_in_yardsticks": ratios,
        });
        std::fs::write(baseline_path(), serde_json::to_string_pretty(&json).unwrap() + "\n").unwrap();
        println!("baseline recorded");
        return;
    }

    let baseline: serde_json::Value =
        serde_json::from_str(&pady_security::read("security/perf-baseline.json")).unwrap();
    let mut worse = Vec::new();
    for (name, now) in &allocs {
        let was =
            baseline["allocations_per_1000_frames"][name].as_f64().unwrap_or_else(|| panic!("no baseline for {name}"));
        // "20% worse than zero" is still zero: nothing may start allocating.
        if *now > was * ALLOWED + 0.001 {
            worse.push(format!("{name}: {now:.1} allocations per 1,000 frames, was {was:.1}"));
        }
    }

    // Timings are held to the baseline on the machine it was recorded on,
    // not on CI's (see the note at the top).
    let enforce_time = std::env::var_os("CI").is_none() || std::env::var_os("PADY_PERF_ENFORCE_TIME").is_some();
    let allowed_time = ALLOWED;
    let was =
        |name: &str| baseline["time_in_yardsticks"][name].as_f64().unwrap_or_else(|| panic!("no baseline for {name}"));
    // A real regression is slow every time it is measured; a busy moment on
    // the machine is not. Anything over the limit is measured again (twice
    // at most), and the best of the measurements is what counts.
    for _ in 0..2 {
        if !enforce_time || ratios.iter().all(|(name, now)| *now <= was(name) * allowed_time) {
            break;
        }
        for (name, again) in measure_times() {
            let best = ratios.get_mut(&name).expect("the same scenarios");
            *best = best.min(again);
        }
    }
    if !ratios.is_empty() {
        println!("time per frame, in yardsticks: {ratios:#?}");
    }
    for (name, now) in &ratios {
        let was = was(name);
        if *now > was * allowed_time && !enforce_time {
            println!("(not enforced here) {name}: {:.0}% over the baseline", (now / was - 1.0) * 100.0);
        } else if *now > was * allowed_time {
            worse.push(format!("{name}: {:.0}% slower ({now:.3} yardsticks, was {was:.3})", (now / was - 1.0) * 100.0));
        }
    }
    assert!(worse.is_empty(), "worse than security/perf-baseline.json allows:\n  {}", worse.join("\n  "));
}

/// The numbers the README promises, whatever the baseline says.
#[test]
fn pointing_and_scrolling_never_allocate() {
    for (name, frames) in scenarios().into_iter().take(2) {
        assert_eq!(allocations(&frames), 0.0, "{name} allocates");
    }
    assert_eq!(lookup_allocations(&config()), 0.0, "looking up the app's profile allocates");
}
