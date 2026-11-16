//! Nothing Pady does for every gesture may leak: it runs for weeks.
//!
//! These tests repeat the per-gesture work tens of thousands of times and
//! compare what the process holds before and after: open handles (file
//! descriptors on Linux) and committed memory. Nothing was found leaking;
//! the tests keep it that way.

use crossbeam_channel::unbounded;
use pady_core::{
    Action, ActionError, Config, Contact, Direction, Engine, Executor, Level, SharedConfig, TouchFrame, Trigger,
};
use pady_security::{footprint, Rng};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const MB: u64 = 1 << 20;

/// What the engine asks the operating system on every gesture: which app is
/// in front, whether a full-screen game is running, whether Windows' own
/// gestures clash, whether Pady starts with the system, and where a program
/// lives. Each opens handles (a process, registry keys, folders). Windows
/// only: on Linux the same questions start helper programs, which a test
/// shouldn't do twenty thousand times.
#[cfg(windows)]
#[test]
fn per_gesture_system_calls_do_not_leak_handles_or_memory() {
    let config = Config::default();
    let mut executor = pady_actions::executor();
    let work = |executor: &mut Box<dyn Executor>, rounds: usize| {
        for i in 0..rounds {
            let _ = pady_actions::foreground_app();
            let _ = executor.foreground_app();
            let _ = executor.fullscreen_game();
            if i % 20 == 0 {
                let _ = pady_actions::conflicts(&config);
                let _ = pady_actions::autostart_enabled();
            }
            if i % 200 == 0 {
                // Looks in App Paths, the Windows folders and all of PATH, and finds nothing.
                let missing = Action::Launch { program: "pady-no-such-program".into(), args: Vec::new() };
                assert!(executor.run(&missing).is_err());
            }
        }
    };
    // Warm up: caches fill, threads start, lazily loaded libraries load.
    work(&mut executor, 2_000);
    let before = footprint();
    work(&mut executor, 20_000);
    let after = footprint();
    println!(
        "handles {} -> {}, memory {} -> {} KB",
        before.handles,
        after.handles,
        before.memory >> 10,
        after.memory >> 10
    );
    assert!(after.handles <= before.handles + 8, "handles grew from {} to {}", before.handles, after.handles);
    assert!(after.memory <= before.memory + 8 * MB, "memory grew by {} KB", (after.memory - before.memory) >> 10);
}

struct Count(Arc<AtomicUsize>);

impl Executor for Count {
    fn run(&mut self, _: &Action) -> Result<Option<Level>, ActionError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(Some(Level::volume(50.0, false)))
    }
    fn mouse_button(&mut self, _: bool) -> Result<(), ActionError> {
        Ok(())
    }
    fn mouse_move(&mut self, _: i32, _: i32) -> Result<(), ActionError> {
        Ok(())
    }
    fn foreground_app(&mut self) -> Option<String> {
        Some("app.exe".into())
    }
}

/// One random session: fingers land, wander (or rest, or jump) and lift.
fn session(rng: &mut Rng, t: &mut u64, out: &mut Vec<TouchFrame>) {
    let fingers = 1 + rng.below(5) as u32;
    let (mut x, mut y) = (rng.below(60) as f32, rng.below(40) as f32);
    let (dx, dy) = (rng.below(9) as f32 - 4.0, rng.below(9) as f32 - 4.0);
    for _ in 0..(2 + rng.below(40)) {
        *t += 1 + rng.below(12);
        if rng.chance(50) {
            x = f32::NAN; // a driver's garbage, now and then
        } else if !x.is_finite() {
            x = 20.0;
        }
        x += dx;
        y += dy;
        let contacts = (0..fingers).map(|i| Contact { id: i, x: x + 12.0 * i as f32, y }).collect();
        out.push(TouchFrame { time_ms: *t, contacts, width_mm: 100.0, height_mm: 60.0 });
    }
    *t += 1 + rng.below(400);
    out.push(TouchFrame { time_ms: *t, contacts: Vec::new(), width_mm: 100.0, height_mm: 60.0 });
}

fn busy_config() -> Config {
    let mut cfg = Config::default();
    for fingers in 3..=5 {
        for (direction, step) in [(Direction::Up, 2), (Direction::Down, -2)] {
            cfg.set_binding(Trigger::Swipe(fingers, direction), Action::Volume { step }, true);
        }
        cfg.set_binding(Trigger::Tap(fingers), Action::Mute, false);
        cfg.set_binding(Trigger::DoubleTap(fingers), Action::Lock, false);
        cfg.set_binding(Trigger::Hold(fingers), Action::Keys { combo: "ctrl+c".into() }, false);
    }
    cfg.normalize();
    cfg
}

/// A million frames of random gestures through the whole engine (both
/// threads, the recognizer, the config and the queue between them).
#[test]
fn a_million_frames_leave_the_engine_no_bigger() {
    let ran = Arc::new(AtomicUsize::new(0));
    let events = Arc::new(AtomicUsize::new(0));
    let (counter, heard) = (ran.clone(), events.clone());
    let (tx, rx) = unbounded();
    let engine = Engine::spawn(
        Arc::new(SharedConfig::new(busy_config())),
        rx,
        move || Box::new(Count(counter)),
        Arc::new(move |_| {
            heard.fetch_add(1, Ordering::Relaxed);
        }),
    );

    let mut rng = Rng(0x5EED_CAFE);
    let mut t = 1_000u64;
    let mut frames = Vec::new();
    let mut feed = |total: usize| {
        let mut sent = 0;
        while sent < total {
            frames.clear();
            session(&mut rng, &mut t, &mut frames);
            sent += frames.len();
            for frame in frames.drain(..) {
                tx.send(frame).unwrap();
            }
            // Don't outrun the engine: this measures what it keeps, not the queue.
            while tx.len() > 2_000 {
                std::thread::yield_now();
            }
        }
    };
    feed(100_000);
    let settle = |tx: &crossbeam_channel::Sender<TouchFrame>| {
        while !tx.is_empty() {
            std::thread::yield_now();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    settle(&tx);
    let before = footprint();
    feed(1_000_000);
    settle(&tx);
    let after = footprint();
    drop(feed);
    drop(tx);
    engine.join();

    let (ran, events) = (ran.load(Ordering::Relaxed), events.load(Ordering::Relaxed));
    println!(
        "{ran} actions, {events} events; handles {} -> {}, memory {} -> {} KB",
        before.handles,
        after.handles,
        before.memory >> 10,
        after.memory >> 10
    );
    assert!(ran > 1_000, "the gestures did something ({ran} actions)");
    assert!(after.handles <= before.handles + 4, "handles grew from {} to {}", before.handles, after.handles);
    assert!(
        after.memory <= before.memory + 16 * MB,
        "memory grew by {} KB",
        (after.memory.saturating_sub(before.memory)) >> 10
    );
}
