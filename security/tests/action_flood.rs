//! PADY-10: level changes queued faster than they could run, and kept
//! running long after the fingers had lifted.
//!
//! A repeating swipe (volume, brightness) sends one action per step to the
//! action thread, which ran them strictly one by one. A step costs under a
//! millisecond for volume on Windows, but 5 ms for brightness there and
//! 10-30 ms on Linux, where each step starts a helper program. A quick long
//! swipe, a jittery touchpad or a hostile device produces steps much faster
//! than that: the level went on changing for seconds after the swipe ended,
//! every later gesture waited behind the backlog, and each step started
//! another process.

use crossbeam_channel::unbounded;
use pady_core::{
    Action, ActionError, Config, Contact, Direction, Engine, Executor, Level, SharedConfig, TouchFrame, Trigger,
};
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A volume control that takes 5 ms per change, like a slow system call or
/// a helper program, and remembers what it was asked to do.
struct Slow {
    calls: Arc<AtomicUsize>,
    total: Arc<AtomicI64>,
    other: Arc<AtomicUsize>,
}

impl Executor for Slow {
    fn run(&mut self, action: &Action) -> Result<Option<Level>, ActionError> {
        match action {
            Action::Volume { step } => {
                std::thread::sleep(Duration::from_millis(5));
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.total.fetch_add(i64::from(*step), Ordering::SeqCst);
            }
            _ => {
                self.other.fetch_add(1, Ordering::SeqCst);
            }
        }
        Ok(None)
    }
    fn mouse_button(&mut self, _: bool) -> Result<(), ActionError> {
        Ok(())
    }
    fn mouse_move(&mut self, _: i32, _: i32) -> Result<(), ActionError> {
        Ok(())
    }
    fn foreground_app(&mut self) -> Option<String> {
        None
    }
}

fn frame(t: u64, fingers: u32, y: f32) -> TouchFrame {
    let contacts = (0..fingers).map(|i| Contact { id: i, x: 10.0 + 12.0 * i as f32, y }).collect();
    TouchFrame { time_ms: t, contacts, width_mm: 100.0, height_mm: 60.0 }
}

struct Run {
    calls: usize,
    total: i64,
    other: usize,
    drained_in: Duration,
}

/// A three-finger swipe up from `start` that travels `mm` in `frames` frames, then lifts,
/// then a four-finger tap (bound to mute). Returns what the executor saw and
/// how long the action thread needed after the last frame was sent.
fn swipe(start: f32, mm: f32, frames: u32) -> Run {
    let mut cfg = Config::default();
    cfg.bindings.clear();
    cfg.set_binding(Trigger::Swipe(3, Direction::Up), Action::Volume { step: 2 }, true);
    cfg.set_binding(Trigger::Tap(4), Action::Mute, false);
    let (calls, total, other) =
        (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicI64::new(0)), Arc::new(AtomicUsize::new(0)));
    let executor = Slow { calls: calls.clone(), total: total.clone(), other: other.clone() };

    let (tx, rx) = unbounded();
    let engine = Engine::spawn(Arc::new(SharedConfig::new(cfg)), rx, move || Box::new(executor), Arc::new(|_| {}));
    tx.send(frame(1_000, 3, start)).unwrap();
    for i in 1..=frames {
        tx.send(frame(1_000 + u64::from(i), 3, start - mm * i as f32 / frames as f32)).unwrap();
    }
    tx.send(frame(1_001 + u64::from(frames), 0, 0.0)).unwrap();
    // The next gesture: a tap that should not have to wait for stale steps.
    tx.send(frame(3_000, 4, 0.0)).unwrap();
    tx.send(frame(3_050, 0, 0.0)).unwrap();
    let sent = Instant::now();
    drop(tx);
    engine.join();
    Run {
        calls: calls.load(Ordering::SeqCst),
        total: total.load(Ordering::SeqCst),
        other: other.load(Ordering::SeqCst),
        drained_in: sent.elapsed(),
    }
}

#[test]
fn a_long_fast_swipe_does_not_leave_a_backlog_of_level_steps() {
    // 400 mm in 100 frames: 2 mm steps, so about 200 volume steps.
    let run = swipe(500.0, 400.0, 100);
    println!("{} volume calls adding up to {:+}, drained in {:?}", run.calls, run.total, run.drained_in);
    assert_eq!(run.other, 1, "the tap after the swipe still ran");
    // Every step still counts: the volume went up by the whole distance (it
    // tops out at 100).
    assert!(run.total >= 100, "the swipe raised the volume by only {}", run.total);
    // 200 steps one by one would be a second of work at 5 ms each.
    // (Generous limits: how many steps merge depends on how the threads are scheduled.)
    assert!(run.calls <= 80, "{} separate volume changes for one swipe", run.calls);
    assert!(run.drained_in < Duration::from_millis(600), "the action thread was busy for {:?}", run.drained_in);
}

#[test]
fn a_device_reporting_absurd_jumps_cannot_keep_the_action_thread_busy() {
    // A broken or hostile device: fingers racing 18 mm per frame across
    // eighteen metres (the furthest positions Pady accepts), for 1,000
    // frames. Each frame claims the most steps it may, 8: 8,000 queued,
    // 40 seconds of work at 5 ms each.
    let run = swipe(9_000.0, 18_000.0, 1_000);
    println!("{} volume calls adding up to {:+}, drained in {:?}", run.calls, run.total, run.drained_in);
    assert_eq!(run.other, 1, "the tap after the flood still ran");
    assert!(run.drained_in < Duration::from_secs(5), "the flood kept the action thread busy for {:?}", run.drained_in);
}

#[test]
fn an_ordinary_slow_swipe_still_changes_the_level_step_by_step() {
    // 20 mm, slowly: frames 30 ms apart leave the action thread idle between
    // steps, so nothing is merged and the level follows the fingers.
    let mut cfg = Config::default();
    cfg.bindings.clear();
    cfg.set_binding(Trigger::Swipe(3, Direction::Up), Action::Volume { step: 2 }, true);
    let (calls, total, other) =
        (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicI64::new(0)), Arc::new(AtomicUsize::new(0)));
    let executor = Slow { calls: calls.clone(), total: total.clone(), other };
    let (tx, rx) = unbounded();
    let engine = Engine::spawn(Arc::new(SharedConfig::new(cfg)), rx, move || Box::new(executor), Arc::new(|_| {}));
    tx.send(frame(1_000, 3, 0.0)).unwrap();
    for i in 1..=20u64 {
        tx.send(frame(1_000 + i * 30, 3, -(i as f32))).unwrap();
        std::thread::sleep(Duration::from_millis(30));
    }
    tx.send(frame(2_000, 0, 0.0)).unwrap();
    drop(tx);
    engine.join();
    let (calls, total) = (calls.load(Ordering::SeqCst), total.load(Ordering::SeqCst));
    println!("{calls} volume calls adding up to {total:+}");
    assert!(calls >= 5, "a slow swipe changed the volume in only {calls} steps");
    assert_eq!(total, 2 * calls as i64, "each call was one 2% step");
}
