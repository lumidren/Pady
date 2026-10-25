//! Cost of the per-frame path, without extra dependencies:
//!
//!     cargo bench -p pady-core
//!
//! Reports time and heap allocations per touch frame through the recognizer
//! for common finger patterns, and the latency from a finger frame (or a
//! "Test" click) to the action thread running the bound action.

use crossbeam_channel::{unbounded, Sender};
use pady_core::{
    now_ms, Action, ActionError, Config, Contact, Engine, Executor, Level, Recognizer, SharedConfig, TouchFrame,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Counts heap allocations so the report can show allocations per frame.
struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        System.realloc(ptr, layout, size)
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const PAD: (f32, f32) = (105.0, 68.0);

fn frame(time_ms: u64, points: &[(f32, f32)]) -> TouchFrame {
    TouchFrame {
        time_ms,
        contacts: points.iter().enumerate().map(|(i, &(x, y))| Contact { id: i as u32, x, y }).collect(),
        width_mm: PAD.0,
        height_mm: PAD.1,
    }
}

/// One touch session at 125 Hz: `fingers` start in a row and each frame
/// moves them by `step(i)`, then they lift.
fn session(fingers: usize, frames: usize, step: impl Fn(usize, usize) -> (f32, f32)) -> Vec<TouchFrame> {
    let mut out = Vec::with_capacity(frames + 1);
    for i in 0..frames {
        let points: Vec<(f32, f32)> = (0..fingers)
            .map(|f| {
                let (dx, dy) = step(i, f);
                (30.0 + f as f32 * 12.0 + dx, 40.0 + dy)
            })
            .collect();
        out.push(frame(i as u64 * 8, &points));
    }
    out.push(frame(frames as u64 * 8, &[]));
    out
}

fn recognizer_case(name: &str, mut frames: Vec<TouchFrame>, rounds: usize) {
    let settings = Config::default().recognizer_settings();
    let mut recognizer = Recognizer::new(settings);
    let total = frames.len() * rounds;
    let mut gestures = 0usize;
    let before = ALLOCATIONS.load(Relaxed);
    let start = Instant::now();
    for _ in 0..rounds {
        for f in frames.iter_mut() {
            gestures += recognizer.update(f).len();
            f.time_ms += 10_000; // the next round is a later session
        }
    }
    let elapsed = start.elapsed();
    let allocations = ALLOCATIONS.load(Relaxed) - before;
    println!(
        "  {name:<34} {:>7.0} ns/frame  {:>5.2} allocs/frame  {:>6} gestures",
        elapsed.as_nanos() as f64 / total as f64,
        allocations as f64 / total as f64,
        gestures
    );
}

struct BenchExecutor {
    ran: Sender<Instant>,
}

impl Executor for BenchExecutor {
    fn run(&mut self, _action: &Action) -> Result<Option<Level>, ActionError> {
        let _ = self.ran.send(Instant::now());
        Ok(None)
    }
    fn mouse_button(&mut self, _down: bool) -> Result<(), ActionError> {
        Ok(())
    }
    fn mouse_move(&mut self, _dx: i32, _dy: i32) -> Result<(), ActionError> {
        Ok(())
    }
}

fn percentile(sorted: &[Duration], p: f64) -> f64 {
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i].as_secs_f64() * 1e6
}

fn report(name: &str, mut samples: Vec<Duration>) {
    samples.sort();
    println!(
        "  {name:<34} p50 {:>6.1} µs   p99 {:>6.1} µs   max {:>7.1} µs   ({} runs)",
        percentile(&samples, 0.50),
        percentile(&samples, 0.99),
        percentile(&samples, 1.0),
        samples.len()
    );
}

fn engine_latency(runs: usize) {
    let config =
        Config::from_toml("[[binding]]\ntrigger = \"tap:5\"\naction = { type = \"mute\" }\n").expect("bench config");
    let (frames_tx, frames_rx) = unbounded();
    let (ran_tx, ran_rx) = unbounded();
    let engine = Engine::spawn(
        Arc::new(SharedConfig::new(config)),
        frames_rx,
        move || Box::new(BenchExecutor { ran: ran_tx }),
        Arc::new(|_| {}),
    );
    let five: Vec<(f32, f32)> = (0..5).map(|f| (20.0 + f as f32 * 14.0, 40.0)).collect();

    // Finger frame → recognizer → engine thread → action thread → executor.
    let mut tap = Vec::with_capacity(runs);
    for _ in 0..runs {
        frames_tx.send(frame(now_ms(), &five)).unwrap();
        std::thread::sleep(Duration::from_millis(2));
        let sent = Instant::now();
        frames_tx.send(frame(now_ms(), &[])).unwrap();
        let ran = ran_rx.recv_timeout(Duration::from_secs(2)).expect("tap action ran");
        tap.push(ran - sent);
        std::thread::sleep(Duration::from_millis(2));
    }
    report("5-finger tap: lift → action runs", tap);

    // The UI's "Test" button: straight to the action thread, which is asleep
    // between clicks (as it is between real gestures).
    let mut test = Vec::with_capacity(runs);
    for _ in 0..runs {
        std::thread::sleep(Duration::from_millis(2));
        let sent = Instant::now();
        engine.run_action(Action::Mute);
        test.push(ran_rx.recv_timeout(Duration::from_secs(2)).expect("test action ran") - sent);
    }
    report("Test button → action runs", test);

    drop(frames_tx);
    engine.join();
}

fn main() {
    println!("Recognizer, per touch frame (Recommended preset settings):");
    // 1 finger moving the pointer: the most common input, ignored by Pady.
    let pointing = session(1, 500, |i, _| (i as f32 * 0.2, i as f32 * 0.1));
    let scrolling = session(2, 500, |i, _| (0.0, i as f32 * 0.15));
    let swipe = session(4, 120, |i, _| (0.0, -(i as f32) * 0.5));
    let drag = session(3, 500, |i, _| (i as f32 * 0.1, i as f32 * 0.05));
    let pinch = session(4, 120, |i, f| ((f as f32 - 1.5) * i as f32 * 0.15, 0.0));
    recognizer_case("1 finger pointing", pointing, 400);
    recognizer_case("2 fingers scrolling", scrolling, 400);
    recognizer_case("4-finger swipe with repeats", swipe, 1500);
    recognizer_case("3-finger drag", drag, 400);
    recognizer_case("4-finger pinch with repeats", pinch, 1500);

    println!("\nEngine, frame or click to the action thread:");
    engine_latency(400);
}
