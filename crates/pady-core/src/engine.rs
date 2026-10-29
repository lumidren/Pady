//! Glue between touch input and actions.
//!
//! Two threads:
//! * the **engine thread** owns the [`Recognizer`], reads frames and turns
//!   gestures into [`Job`]s;
//! * the **action thread** owns the platform [`Executor`], resolves each
//!   gesture to a binding (asking which app is in front, for per-app
//!   profiles) and runs jobs in order, so a slow action never delays touch
//!   handling.

use crate::action::Action;
use crate::config::{Config, Resolution, SharedConfig};
use crate::frame::{now_ms, TouchFrame};
use crate::gesture::Gesture;
use crate::recognizer::{Recognizer, SwipeProgress};
use crate::shapes::{self, NotAShape, StrokeTracker};
use crate::trigger::Trigger;
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

pub type ActionError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelKind {
    Volume,
    Brightness,
}

/// The level a volume or brightness action left behind, for the overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Level {
    pub kind: LevelKind,
    pub percent: u8,
    pub muted: bool,
}

impl Level {
    pub fn volume(percent: f32, muted: bool) -> Self {
        Level { kind: LevelKind::Volume, percent: percent.round().clamp(0.0, 100.0) as u8, muted }
    }

    pub fn brightness(percent: f32) -> Self {
        Level { kind: LevelKind::Brightness, percent: percent.round().clamp(0.0, 100.0) as u8, muted: false }
    }
}

/// Carries out actions on the host OS. Created on, and confined to, the
/// action thread, so implementations may hold thread-affine handles (COM).
pub trait Executor {
    /// Run an action. Volume and brightness actions return the new level
    /// when the OS reports it.
    fn run(&mut self, action: &Action) -> Result<Option<Level>, ActionError>;
    fn mouse_button(&mut self, down: bool) -> Result<(), ActionError>;
    /// Relative cursor movement in pixels.
    fn mouse_move(&mut self, dx: i32, dy: i32) -> Result<(), ActionError>;
    /// Executable name of the app in front (e.g. `chrome.exe`), for per-app
    /// profiles. Only asked when the config has profiles.
    fn foreground_app(&mut self) -> Option<String> {
        None
    }
    /// Is a game running in exclusive full screen? Gestures pause while it
    /// is, when `pause_in_fullscreen` is on.
    fn fullscreen_game(&mut self) -> bool {
        false
    }
}

/// Executor that only logs; used by `pady monitor` and tests.
#[derive(Default)]
pub struct LogExecutor;

impl Executor for LogExecutor {
    fn run(&mut self, action: &Action) -> Result<Option<Level>, ActionError> {
        log::info!("would run: {}", action.label());
        Ok(None)
    }
    fn mouse_button(&mut self, down: bool) -> Result<(), ActionError> {
        log::info!("would {} left button", if down { "press" } else { "release" });
        Ok(())
    }
    fn mouse_move(&mut self, _dx: i32, _dy: i32) -> Result<(), ActionError> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Job {
    /// Run this action now (the UI's "Test" button).
    Run(Action),
    /// A gesture fired; resolve its binding for the app in front and run it.
    Fire {
        gesture: Gesture,
        trigger: Trigger,
    },
    MouseButton(bool),
    MouseMove(i32, i32),
    /// Let go of the mouse button if a drag is holding it (on quit).
    ReleaseMouse(Ack),
}

/// Lets the sender of a [`Job`] wait until the action thread has handled it.
#[derive(Clone, Debug)]
pub struct Ack(Sender<()>);

impl PartialEq for Ack {
    fn eq(&self, _: &Self) -> bool {
        true // acknowledgements carry no data worth comparing
    }
}

/// What the engine reports to observers (UI, CLI).
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EngineEvent {
    Frame(TouchFrame),
    Gesture {
        gesture: Gesture,
        /// Config-style trigger string, e.g. `swipe:3:up`.
        trigger: Option<String>,
        /// Label of the action that ran, if one was bound.
        action: Option<String>,
        /// Name of the per-app profile the binding came from.
        profile: Option<String>,
    },
    Level(Level),
    ActionFailed {
        action: String,
        error: String,
    },
    /// A gesture was ignored because a full-screen game is in front.
    Paused {
        trigger: String,
    },
    /// A swipe is on its way (sent as the fingers move, before it fires).
    /// `bound`: something runs when it fires (ignoring per-app profiles).
    SwipeProgress {
        progress: SwipeProgress,
        bound: bool,
    },
    /// The swipe in progress either fired or was let go.
    SwipeEnded {
        fired: bool,
    },
    /// While recording a shape: the two-finger stroke just drawn, normalised
    /// (see [`crate::shapes::normalize`]), or why it can't be a shape.
    ShapeRecorded {
        points: Option<Vec<[f32; 2]>>,
        problem: Option<NotAShape>,
    },
}

pub type Observer = Arc<dyn Fn(EngineEvent) + Send + Sync>;

/// Hooks into the OS scheduler, so the engine itself stays OS independent.
/// The defaults do nothing.
#[derive(Clone, Copy, Debug)]
pub struct Scheduling {
    /// Runs first on the engine and action threads, e.g. to raise their
    /// priority so gestures stay instant while the computer is busy.
    pub thread_start: fn(),
    /// `true` asks for millisecond-accurate timers while a hold, multi-tap
    /// or drag-release timer is pending; `false` gives them back.
    pub precise_timers: fn(bool),
}

impl Default for Scheduling {
    fn default() -> Self {
        Scheduling { thread_start: || {}, precise_timers: |_| {} }
    }
}

pub struct Engine {
    jobs: Sender<Job>,
    recording: Arc<AtomicBool>,
    engine_thread: JoinHandle<()>,
    action_thread: JoinHandle<()>,
}

impl Engine {
    /// Start both threads. The engine stops when every sender for `frames`
    /// has been dropped.
    pub fn spawn<F>(
        config: Arc<SharedConfig>,
        frames: Receiver<TouchFrame>,
        make_executor: F,
        observer: Observer,
    ) -> Engine
    where
        F: FnOnce() -> Box<dyn Executor> + Send + 'static,
    {
        Engine::spawn_with(config, frames, make_executor, observer, Scheduling::default())
    }

    /// [`Engine::spawn`], with hooks into the OS scheduler.
    pub fn spawn_with<F>(
        config: Arc<SharedConfig>,
        frames: Receiver<TouchFrame>,
        make_executor: F,
        observer: Observer,
        scheduling: Scheduling,
    ) -> Engine
    where
        F: FnOnce() -> Box<dyn Executor> + Send + 'static,
    {
        let (jobs_tx, jobs_rx) = unbounded::<Job>();

        let action_observer = observer.clone();
        let action_config = config.clone();
        let action_thread = std::thread::Builder::new()
            .name("pady-actions".into())
            .spawn(move || {
                (scheduling.thread_start)();
                ActionLoop::new(make_executor(), action_config, action_observer).run(jobs_rx)
            })
            .expect("spawn action thread");

        let engine_jobs = jobs_tx.clone();
        let recording = Arc::new(AtomicBool::new(false));
        let engine_recording = recording.clone();
        let engine_thread = std::thread::Builder::new()
            .name("pady-engine".into())
            .spawn(move || {
                (scheduling.thread_start)();
                EngineLoop::new(config, engine_jobs, observer, scheduling.precise_timers, engine_recording).run(frames)
            })
            .expect("spawn engine thread");

        Engine { jobs: jobs_tx, recording, engine_thread, action_thread }
    }

    /// Record the next two-finger stroke as a shape (reported as
    /// [`EngineEvent::ShapeRecorded`]) instead of matching it, or stop waiting.
    pub fn record_shape(&self, on: bool) {
        self.recording.store(on, Ordering::Relaxed);
    }

    /// Run an action right away (the UI's "Test" button). Like bindings,
    /// it is kept in range first: it comes straight from the UI.
    pub fn run_action(&self, mut action: Action) {
        action.sanitize();
        let _ = self.jobs.send(Job::Run(action));
    }

    /// Release the mouse button if a drag is holding it, waiting up to
    /// `timeout` for the action thread. Call before quitting so the button
    /// is never left pressed system-wide.
    pub fn release_mouse(&self, timeout: Duration) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if self.jobs.send(Job::ReleaseMouse(Ack(tx))).is_ok() {
            let _ = rx.recv_timeout(timeout);
        }
    }

    /// Wait for the engine to stop (after the frame source closes) and for
    /// queued actions to finish.
    pub fn join(self) {
        let Engine { jobs, engine_thread, action_thread, .. } = self;
        let _ = engine_thread.join();
        // The engine thread's sender is gone; dropping ours lets the action thread drain and exit.
        drop(jobs);
        let _ = action_thread.join();
    }
}

struct ActionLoop {
    exec: Box<dyn Executor>,
    shared: Arc<SharedConfig>,
    generation: u64,
    config: Config,
    observer: Observer,
    /// Did we press the mouse button (a drag in progress)?
    button_down: bool,
    /// A drag that started while gestures were paused: swallow it whole.
    drag_suppressed: bool,
    /// Were gestures paused for a full-screen game last time we looked?
    was_paused: bool,
}

impl ActionLoop {
    fn new(exec: Box<dyn Executor>, shared: Arc<SharedConfig>, observer: Observer) -> Self {
        ActionLoop {
            exec,
            generation: shared.generation(),
            config: shared.get(),
            shared,
            observer,
            button_down: false,
            drag_suppressed: false,
            was_paused: false,
        }
    }

    fn refresh_config(&mut self) {
        let generation = self.shared.generation();
        if generation != self.generation {
            self.generation = generation;
            self.config = self.shared.get();
        }
    }

    /// Gestures pause while a full-screen game runs (if the user wants that).
    fn paused(&mut self) -> bool {
        let paused = self.config.settings.pause_in_fullscreen && self.exec.fullscreen_game();
        if paused != self.was_paused {
            self.was_paused = paused;
            let app = self.exec.foreground_app().unwrap_or_else(|| "an unknown app".into());
            if paused {
                log::info!("gestures paused: {app} is a full-screen game");
            } else {
                log::info!("gestures resumed ({app} in front)");
            }
        }
        paused
    }

    fn run(mut self, jobs: Receiver<Job>) {
        for job in jobs {
            match job {
                Job::Run(action) => self.execute(&action),
                Job::Fire { gesture, trigger } => self.fire(gesture, trigger),
                Job::MouseButton(true) => {
                    self.refresh_config();
                    if self.paused() {
                        log::debug!("drag ignored: a full-screen game is running");
                        self.drag_suppressed = true;
                        continue;
                    }
                    let result = self.exec.mouse_button(true);
                    self.button_down = result.is_ok();
                    self.report("Mouse button", result);
                }
                Job::MouseButton(false) => {
                    if std::mem::take(&mut self.drag_suppressed) {
                        continue;
                    }
                    let result = self.exec.mouse_button(false);
                    self.button_down = false;
                    self.report("Mouse button", result);
                }
                Job::MouseMove(dx, dy) => {
                    if self.drag_suppressed {
                        continue;
                    }
                    let result = self.exec.mouse_move(dx, dy);
                    self.report("Mouse move", result);
                }
                Job::ReleaseMouse(Ack(done)) => {
                    if self.button_down {
                        let result = self.exec.mouse_button(false);
                        self.button_down = false;
                        self.report("Mouse button", result);
                    }
                    let _ = done.send(());
                }
            }
        }
    }

    fn fire(&mut self, gesture: Gesture, trigger: Trigger) {
        self.refresh_config();
        if self.paused() {
            log::debug!("{trigger} ignored: a full-screen game is running");
            (self.observer)(EngineEvent::Paused { trigger: trigger.to_string() });
            return;
        }
        let app = if self.config.profiles.is_empty() { None } else { self.exec.foreground_app() };
        let step = match gesture {
            Gesture::Swipe { step, .. } | Gesture::Pinch { step, .. } => step,
            _ => 0,
        };
        let (action, profile) = match self.config.resolution(trigger, app.as_deref()) {
            Resolution::Bound { binding, profile } if step == 0 || binding.repeat => {
                (Some(binding.action.clone()), profile.map(|p| p.name.clone()))
            }
            // Repeat steps that nothing repeats are noise; everything else is worth showing.
            _ if step > 0 => return,
            Resolution::Off { profile } => (None, Some(profile.name.clone())),
            _ => (None, None),
        };
        let glides = matches!(gesture, Gesture::Swipe { fingers, .. }
            if self.config.glide_fingers().get(fingers as usize).copied().unwrap_or(false));
        let action = action.map(|a| if glides && a.is_level() { a.glide_step() } else { a });
        (self.observer)(EngineEvent::Gesture {
            gesture,
            trigger: Some(trigger.to_string()),
            action: action.as_ref().map(Action::label),
            profile,
        });
        if let Some(action) = action {
            self.execute(&action);
        }
    }

    fn execute(&mut self, action: &Action) {
        match self.exec.run(action) {
            Ok(Some(level)) => (self.observer)(EngineEvent::Level(level)),
            Ok(None) => {}
            Err(e) => self.report(&action.label(), Err(e)),
        }
    }

    fn report(&self, label: &str, result: Result<(), ActionError>) {
        if let Err(e) = result {
            log::warn!("{label} failed: {e}");
            (self.observer)(EngineEvent::ActionFailed { action: label.to_string(), error: e.to_string() });
        }
    }
}

/// Converts finger travel (mm) to cursor pixels, keeping sub-pixel remainders
/// so slow drags don't stall and fast flicks get a little acceleration.
#[derive(Debug, Default)]
pub(crate) struct Pointer {
    rem: (f32, f32),
    last_time: u64,
}

impl Pointer {
    pub(crate) fn step(&mut self, dx_mm: f32, dy_mm: f32, t: u64, speed: f32, accel: f32) -> (i32, i32) {
        let dt = (t.saturating_sub(self.last_time)).clamp(1, 100) as f32 / 1000.0;
        self.last_time = t;
        let velocity = dx_mm.hypot(dy_mm) / dt; // mm/s
        let gain = speed * (1.0 + accel * (velocity / 120.0).min(2.5));
        let x = dx_mm * gain + self.rem.0;
        let y = dy_mm * gain + self.rem.1;
        let (ix, iy) = (x.trunc(), y.trunc());
        // A move too big for f32 must not leave a NaN behind that stalls the drag.
        let rem = |v: f32, i: f32| if (v - i).is_finite() { v - i } else { 0.0 };
        self.rem = (rem(x, ix), rem(y, iy));
        (ix as i32, iy as i32)
    }

    pub(crate) fn reset(&mut self, t: u64) {
        self.rem = (0.0, 0.0);
        self.last_time = t;
    }
}

struct EngineLoop {
    shared: Arc<SharedConfig>,
    generation: u64,
    config: Config,
    recognizer: Recognizer,
    jobs: Sender<Job>,
    observer: Observer,
    pointer: Pointer,
    precise_timers: fn(bool),
    /// Are precise timers requested right now?
    precise: bool,
    /// The last swipe progress reported, to send only changes.
    progress: Option<SwipeProgress>,
    /// Two-finger strokes, for shapes.
    strokes: StrokeTracker,
    /// The next stroke is being recorded rather than matched.
    recording: Arc<AtomicBool>,
}

impl EngineLoop {
    fn new(
        shared: Arc<SharedConfig>,
        jobs: Sender<Job>,
        observer: Observer,
        precise_timers: fn(bool),
        recording: Arc<AtomicBool>,
    ) -> Self {
        let config = shared.get();
        EngineLoop {
            generation: shared.generation(),
            recognizer: Recognizer::new(config.recognizer_settings()),
            config,
            shared,
            jobs,
            observer,
            pointer: Pointer::default(),
            precise_timers,
            precise: false,
            progress: None,
            strokes: StrokeTracker::default(),
            recording,
        }
    }

    /// Asks for precise timers only while one is pending, so idle costs nothing.
    fn want_precise_timers(&mut self, want: bool) {
        if want != self.precise {
            self.precise = want;
            (self.precise_timers)(want);
        }
    }

    fn run(mut self, frames: Receiver<TouchFrame>) {
        loop {
            self.refresh_config();
            // Sleep until the next frame or the next due timer (hold, drag
            // release, multi-tap); with nothing pending there are no wake-ups.
            let deadline = self.recognizer.next_deadline();
            self.want_precise_timers(deadline.is_some());
            let received = match deadline {
                Some(due) => frames.recv_timeout(Duration::from_millis(due.saturating_sub(now_ms()))),
                None => frames.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            // The config may have changed while we slept: pausing gestures
            // must stop the very next frame or timer, not the one after.
            self.refresh_config();
            match received {
                Ok(frame) => self.on_frame(frame),
                Err(RecvTimeoutError::Timeout) => {
                    let gestures = self.recognizer.tick(now_ms());
                    self.dispatch(gestures, now_ms());
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.want_precise_timers(false);
        // Deliver what is still pending (a tap waiting for a second one, a drag).
        let mut gestures = self.recognizer.tick(u64::MAX);
        gestures.extend(self.recognizer.reset());
        self.dispatch(gestures, now_ms());
    }

    fn refresh_config(&mut self) {
        let generation = self.shared.generation();
        if generation == self.generation {
            return;
        }
        self.generation = generation;
        self.config = self.shared.get();
        self.recognizer.set_settings(self.config.recognizer_settings());
        if !self.config.settings.enabled {
            let gestures = self.recognizer.reset();
            self.dispatch(gestures, now_ms());
        }
    }

    fn on_frame(&mut self, frame: TouchFrame) {
        let gestures = if self.config.settings.enabled { self.recognizer.update(&frame) } else { Vec::new() };
        let t = frame.time_ms;
        let stroke = self.strokes.update(&frame);
        (self.observer)(EngineEvent::Frame(frame));
        self.report_progress(&gestures);
        self.dispatch(gestures, t);
        if let Some(path) = stroke {
            self.on_stroke(&path, t);
        }
    }

    /// A two-finger stroke ended: record it, or fire the shape it matches.
    fn on_stroke(&mut self, path: &[(f32, f32)], t: u64) {
        if self.recording.swap(false, Ordering::Relaxed) {
            // A stroke too broken to measure is reported as too short, so the
            // UI always hears either a shape or what was wrong with it.
            let event = match shapes::check(path).and_then(|()| shapes::normalize(path).ok_or(NotAShape::TooShort)) {
                Ok(points) => EngineEvent::ShapeRecorded { points: Some(points), problem: None },
                Err(problem) => EngineEvent::ShapeRecorded { points: None, problem: Some(problem) },
            };
            (self.observer)(event);
            return;
        }
        if !self.config.settings.enabled || self.config.shapes.is_empty() {
            return;
        }
        if let Some((id, score)) = shapes::best_match(&self.config.shapes, path) {
            log::debug!("two-finger shape {id} ({score:.2})");
            self.handle(Gesture::Shape { id }, t);
        }
    }

    /// Tells observers how far a swipe has come, and when it fires or is let go.
    fn report_progress(&mut self, gestures: &[Gesture]) {
        let now = if self.config.settings.enabled { self.recognizer.swipe_progress() } else { None };
        match (self.progress, now) {
            (_, Some(p)) => {
                let changed = self.progress.is_none_or(|old| {
                    old.direction != p.direction || old.fingers != p.fingers || (old.amount - p.amount).abs() >= 0.01
                });
                if changed {
                    let trigger = Trigger::Swipe(p.fingers, p.direction);
                    let bound =
                        matches!(self.config.resolution(trigger, None), crate::config::Resolution::Bound { .. });
                    (self.observer)(EngineEvent::SwipeProgress { progress: p, bound });
                    self.progress = Some(p);
                }
            }
            (Some(_), None) => {
                let fired = gestures.iter().any(|g| matches!(g, Gesture::Swipe { step: 0, .. }));
                (self.observer)(EngineEvent::SwipeEnded { fired });
                self.progress = None;
            }
            (None, None) => {}
        }
    }

    fn dispatch(&mut self, gestures: Vec<Gesture>, t: u64) {
        for g in gestures {
            self.handle(g, t);
        }
    }

    fn handle(&mut self, gesture: Gesture, t: u64) {
        let s = &self.config.settings;
        match gesture {
            Gesture::DragMove { dx, dy } => {
                let (px, py) = self.pointer.step(dx, dy, t, s.drag_speed, s.drag_acceleration);
                if px != 0 || py != 0 {
                    let _ = self.jobs.send(Job::MouseMove(px, py));
                }
            }
            Gesture::DragStart { fingers } => {
                self.pointer.reset(t);
                let _ = self.jobs.send(Job::MouseButton(true));
                (self.observer)(EngineEvent::Gesture {
                    gesture,
                    trigger: Some(Trigger::Drag(fingers).to_string()),
                    action: Some(Action::Drag.label()),
                    profile: None,
                });
            }
            Gesture::DragEnd => {
                let _ = self.jobs.send(Job::MouseButton(false));
                (self.observer)(EngineEvent::Gesture { gesture, trigger: None, action: None, profile: None });
            }
            _ => {
                if let Some(trigger) = Trigger::from_gesture(&gesture) {
                    let _ = self.jobs.send(Job::Fire { gesture, trigger });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Binding, Profile};
    use crate::frame::Contact;
    use crate::gesture::Direction;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    struct Recorder {
        jobs: Arc<Mutex<Vec<Job>>>,
        app: Option<String>,
        fullscreen: bool,
    }

    impl Executor for Recorder {
        fn run(&mut self, action: &Action) -> Result<Option<Level>, ActionError> {
            self.jobs.lock().unwrap().push(Job::Run(action.clone()));
            match action {
                Action::Run { .. } => Err("boom".into()),
                Action::Volume { step } => Ok(Some(Level::volume(50.0 + *step as f32, false))),
                _ => Ok(None),
            }
        }
        fn mouse_button(&mut self, down: bool) -> Result<(), ActionError> {
            self.jobs.lock().unwrap().push(Job::MouseButton(down));
            Ok(())
        }
        fn mouse_move(&mut self, dx: i32, dy: i32) -> Result<(), ActionError> {
            self.jobs.lock().unwrap().push(Job::MouseMove(dx, dy));
            Ok(())
        }
        fn foreground_app(&mut self) -> Option<String> {
            self.app.clone()
        }
        fn fullscreen_game(&mut self) -> bool {
            self.fullscreen
        }
    }

    fn frame(t: u64, n: u32, x: f32, y: f32) -> TouchFrame {
        let contacts = (0..n).map(|i| Contact { id: i, x: 10.0 + i as f32 * 12.0 + x, y: 30.0 + y }).collect();
        TouchFrame { time_ms: t, contacts, width_mm: 100.0, height_mm: 60.0 }
    }

    fn run_engine_in(app: Option<&str>, cfg: Config, frames: Vec<TouchFrame>) -> (Vec<Job>, Vec<EngineEvent>) {
        let jobs = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = unbounded();
        let rec = jobs.clone();
        let ev = events.clone();
        let app = app.map(str::to_string);
        let engine = Engine::spawn(
            Arc::new(SharedConfig::new(cfg)),
            rx,
            move || Box::new(Recorder { jobs: rec, app, fullscreen: false }),
            Arc::new(move |e| ev.lock().unwrap().push(e)),
        );
        for f in frames {
            tx.send(f).unwrap();
        }
        drop(tx);
        engine.join();
        let jobs = jobs.lock().unwrap().clone();
        let events = events.lock().unwrap().clone();
        (jobs, events)
    }

    fn run_engine(cfg: Config, frames: Vec<TouchFrame>) -> (Vec<Job>, Vec<EngineEvent>) {
        run_engine_in(None, cfg, frames)
    }

    fn tap3() -> Vec<TouchFrame> {
        vec![frame(0, 3, 0.0, 0.0), frame(60, 0, 0.0, 0.0)]
    }

    #[test]
    fn swipe_repeat_scrubs_volume() {
        let mut cfg = Config::default();
        cfg.bindings.clear();
        cfg.set_binding(Trigger::Swipe(3, Direction::Up), Action::Volume { step: 2 }, true);
        cfg.set_binding(Trigger::Swipe(3, Direction::Down), Action::Volume { step: -2 }, false);
        let step = cfg.settings.swipe_step_mm;
        let thr = cfg.settings.swipe_threshold_mm;
        let (jobs, events) = run_engine(
            cfg,
            vec![
                frame(0, 3, 0.0, 0.0),
                frame(10, 3, 0.0, -thr),
                frame(20, 3, 0.0, -thr - 2.0 * step - 0.1),
                // Back down: Down is bound without repeat, so steps are ignored.
                frame(30, 3, 0.0, -thr + step),
                frame(40, 3, 0.0, 0.0),
                frame(50, 0, 0.0, 0.0),
            ],
        );
        // Volume swipes glide: 2% every step_mm / 3.5 (14.1 mm = 7 steps after the first).
        assert_eq!(jobs, vec![Job::Run(Action::Volume { step: 2 }); 8]);
        let levels: Vec<_> = events.iter().filter(|e| matches!(e, EngineEvent::Level(_))).collect();
        assert_eq!(levels.len(), 8, "every volume change reports its level for the overlay");
    }

    #[test]
    fn three_finger_drag_holds_button_and_moves_cursor() {
        let mut cfg = Config::default();
        cfg.settings.drag_release_ms = 0;
        let (jobs, events) = run_engine(
            cfg,
            vec![frame(0, 3, 0.0, 0.0), frame(10, 3, 10.0, 0.0), frame(20, 3, 20.0, 0.0), frame(30, 0, 0.0, 0.0)],
        );
        assert_eq!(jobs.first(), Some(&Job::MouseButton(true)));
        assert_eq!(jobs.last(), Some(&Job::MouseButton(false)));
        let moved: i32 = jobs.iter().map(|j| if let Job::MouseMove(x, _) = j { *x } else { 0 }).sum();
        assert!(moved > 100, "cursor should travel right, got {moved}px");
        assert!(events.iter().any(|e| matches!(
            e,
            EngineEvent::Gesture { gesture: Gesture::DragStart { fingers: 3 }, action: Some(_), .. }
        )));
    }

    #[test]
    fn swipe_progress_is_reported_until_the_swipe_fires_or_is_let_go() {
        let mut cfg = Config::default();
        cfg.bindings.clear();
        cfg.set_binding(Trigger::Swipe(4, Direction::Up), Action::Lock, false);
        let progress = |events: &[EngineEvent]| -> Vec<(f32, bool)> {
            events
                .iter()
                .filter_map(|e| match e {
                    EngineEvent::SwipeProgress { progress, bound } => Some((progress.amount, *bound)),
                    _ => None,
                })
                .collect()
        };
        let ended = |events: &[EngineEvent]| -> Vec<bool> {
            events
                .iter()
                .filter_map(|e| if let EngineEvent::SwipeEnded { fired } = e { Some(*fired) } else { None })
                .collect()
        };
        let fires = vec![
            frame(0, 4, 0.0, 0.0),
            frame(10, 4, 0.0, -4.0),
            frame(20, 4, 0.0, -7.0),
            frame(30, 4, 0.0, -12.0),
            frame(40, 0, 0.0, 0.0),
        ];
        let (_, events) = run_engine(cfg.clone(), fires);
        let p = progress(&events);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(p[0].0 < p[1].0 && p.iter().all(|&(_, bound)| bound), "{p:?}");
        assert_eq!(ended(&events), vec![true]);

        let let_go = vec![frame(0, 4, 0.0, 0.0), frame(10, 4, 0.0, 5.0), frame(20, 0, 0.0, 0.0)];
        let (_, events) = run_engine(cfg, let_go);
        assert_eq!(progress(&events).iter().map(|p| p.1).collect::<Vec<_>>(), vec![false], "swiping down isn't bound");
        assert_eq!(ended(&events), vec![false]);
    }

    /// Two fingers drawing a V (down-right, then up-right), then lifting.
    fn vee_frames(t0: u64) -> Vec<TouchFrame> {
        let mut out: Vec<TouchFrame> = (0..=20)
            .map(|i| {
                let x = i as f32 * 1.5;
                let y = if i <= 10 { i as f32 * 3.0 } else { (20 - i) as f32 * 3.0 };
                frame(t0 + i as u64 * 10, 2, x, y)
            })
            .collect();
        out.push(frame(t0 + 220, 0, 0.0, 0.0));
        out
    }

    #[test]
    fn a_recorded_two_finger_shape_fires_its_binding() {
        // Record: the stroke comes back as a shape instead of being matched.
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let (tx, rx) = unbounded();
        let engine = Engine::spawn(
            Arc::new(SharedConfig::new(Config::default())),
            rx,
            || Box::new(LogExecutor) as Box<dyn Executor>,
            Arc::new(move |e| ev.lock().unwrap().push(e)),
        );
        engine.record_shape(true);
        vee_frames(0).into_iter().for_each(|f| tx.send(f).unwrap());
        drop(tx);
        engine.join();
        let points = events
            .lock()
            .unwrap()
            .iter()
            .find_map(|e| match e {
                EngineEvent::ShapeRecorded { points, problem: None } => points.clone(),
                _ => None,
            })
            .expect("the V was recorded");

        // Draw it again: the shape's binding runs.
        let mut cfg = Config::default();
        cfg.bindings.clear();
        cfg.shapes.push(crate::Shape { id: 4, name: "V".into(), points });
        cfg.set_binding(Trigger::Shape(4), Action::Lock, false);
        let (jobs, events) = run_engine(cfg.clone(), vee_frames(1_000));
        assert_eq!(jobs, vec![Job::Run(Action::Lock)]);
        assert!(events.iter().any(|e| matches!(e, EngineEvent::Gesture { trigger: Some(t), .. } if t == "shape:4")));

        // A plain two-finger scroll is left to the OS.
        let scroll: Vec<_> =
            (0..=10).map(|i| frame(i * 10, 2, 0.0, i as f32 * 4.0)).chain([frame(120, 0, 0.0, 0.0)]).collect();
        assert!(run_engine(cfg, scroll).0.is_empty());
    }

    #[test]
    fn recording_a_scroll_says_why_it_is_not_a_shape() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let (tx, rx) = unbounded();
        let engine = Engine::spawn(
            Arc::new(SharedConfig::new(Config::default())),
            rx,
            || Box::new(LogExecutor) as Box<dyn Executor>,
            Arc::new(move |e| ev.lock().unwrap().push(e)),
        );
        engine.record_shape(true);
        (0..=10)
            .map(|i| frame(i * 10, 2, 0.0, i as f32 * 4.0))
            .chain([frame(120, 0, 0.0, 0.0)])
            .for_each(|f| tx.send(f).unwrap());
        drop(tx);
        engine.join();
        let got = events.lock().unwrap().iter().find_map(|e| match e {
            EngineEvent::ShapeRecorded { points, problem } => Some((points.is_some(), *problem)),
            _ => None,
        });
        assert_eq!(got, Some((false, Some(NotAShape::Straight))));
    }

    #[test]
    fn disabled_engine_still_reports_frames_but_never_acts() {
        let mut cfg = Config::default();
        cfg.settings.enabled = false;
        let (jobs, events) = run_engine(cfg, tap3());
        assert!(jobs.is_empty());
        assert_eq!(events.iter().filter(|e| matches!(e, EngineEvent::Frame(_))).count(), 2);
    }

    #[test]
    fn failures_are_reported() {
        let mut cfg = Config::default();
        cfg.set_binding(Trigger::Tap(3), Action::Run { command: "nope".into() }, false);
        let (_, events) = run_engine(cfg, tap3());
        assert!(events.iter().any(|e| matches!(e, EngineEvent::ActionFailed { error, .. } if error == "boom")));
    }

    fn with_browser_profile() -> Config {
        let mut cfg = Config::default();
        cfg.set_binding(Trigger::Tap(3), Action::Mute, false);
        cfg.profiles.push(Profile {
            name: "Browser".into(),
            apps: vec!["chrome.exe".into()],
            bindings: vec![Binding { trigger: Trigger::Tap(3), action: Action::Lock, repeat: false }],
        });
        cfg
    }

    #[test]
    fn profile_binding_wins_for_its_app() {
        let (jobs, events) = run_engine_in(Some("chrome.exe"), with_browser_profile(), tap3());
        assert_eq!(jobs, vec![Job::Run(Action::Lock)]);
        assert!(events.iter().any(|e| matches!(
            e,
            EngineEvent::Gesture { profile: Some(p), .. } if p == "Browser"
        )));
    }

    #[test]
    fn profiles_report_gestures_they_switch_off() {
        let mut cfg = with_browser_profile();
        cfg.profiles[0].bindings[0].action = Action::None;
        let (jobs, events) = run_engine_in(Some("chrome.exe"), cfg, tap3());
        assert!(jobs.is_empty(), "nothing runs: {jobs:?}");
        assert!(events.iter().any(|e| matches!(
            e,
            EngineEvent::Gesture { action: None, profile: Some(p), .. } if p == "Browser"
        )));
    }

    #[test]
    fn other_apps_fall_back_to_global_bindings() {
        let (jobs, _) = run_engine_in(Some("code.exe"), with_browser_profile(), tap3());
        assert_eq!(jobs, vec![Job::Run(Action::Mute)]);
    }

    #[test]
    fn scheduling_hooks_run_and_precise_timers_are_balanced() {
        use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
        static STARTS: AtomicUsize = AtomicUsize::new(0);
        static PRECISE: AtomicIsize = AtomicIsize::new(0);
        static REQUESTS: AtomicUsize = AtomicUsize::new(0);
        let scheduling = Scheduling {
            thread_start: || {
                STARTS.fetch_add(1, Ordering::SeqCst);
            },
            precise_timers: |on| {
                PRECISE.fetch_add(if on { 1 } else { -1 }, Ordering::SeqCst);
                if on {
                    REQUESTS.fetch_add(1, Ordering::SeqCst);
                }
            },
        };
        let jobs = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = unbounded::<TouchFrame>();
        let rec = jobs.clone();
        let engine = Engine::spawn_with(
            Arc::new(SharedConfig::new(Config::default())),
            rx,
            move || Box::new(Recorder { jobs: rec, app: None, fullscreen: false }),
            Arc::new(|_| {}),
            scheduling,
        );
        // Fingers resting: a hold timer is pending, so precise timers are on.
        tx.send(frame(0, 3, 0.0, 0.0)).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while REQUESTS.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(PRECISE.load(Ordering::SeqCst), 1, "on while a timer is pending");
        drop(tx);
        engine.join();
        assert_eq!(STARTS.load(Ordering::SeqCst), 2, "engine and action threads");
        assert_eq!(PRECISE.load(Ordering::SeqCst), 0, "given back when the engine stops");
    }

    #[test]
    fn release_mouse_lets_go_only_of_a_held_button() {
        let jobs = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = unbounded::<TouchFrame>();
        let rec = jobs.clone();
        let engine = Engine::spawn(
            Arc::new(SharedConfig::new(Config::default())),
            rx,
            move || Box::new(Recorder { jobs: rec, app: None, fullscreen: false }),
            Arc::new(|_| {}),
        );
        engine.release_mouse(Duration::from_secs(2));
        assert!(jobs.lock().unwrap().is_empty(), "nothing was held, so nothing is released");

        // Start a three-finger drag and quit mid-way.
        for f in [frame(0, 3, 0.0, 0.0), frame(10, 3, 12.0, 0.0)] {
            tx.send(f).unwrap();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !jobs.lock().unwrap().contains(&Job::MouseButton(true)) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        engine.release_mouse(Duration::from_secs(2));
        assert_eq!(jobs.lock().unwrap().last(), Some(&Job::MouseButton(false)));
        drop(tx);
        engine.join();
    }

    fn run_engine_fullscreen(cfg: Config, frames: Vec<TouchFrame>) -> Vec<Job> {
        let jobs = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = unbounded();
        let rec = jobs.clone();
        let engine = Engine::spawn(
            Arc::new(SharedConfig::new(cfg)),
            rx,
            move || Box::new(Recorder { jobs: rec, app: None, fullscreen: true }),
            Arc::new(|_| {}),
        );
        for f in frames {
            tx.send(f).unwrap();
        }
        drop(tx);
        engine.join();
        let jobs = jobs.lock().unwrap().clone();
        jobs
    }

    #[test]
    fn full_screen_games_pause_actions_and_drags() {
        let mut cfg = Config::default();
        cfg.settings.drag_release_ms = 0;
        let drag =
            vec![frame(0, 3, 0.0, 0.0), frame(10, 3, 12.0, 0.0), frame(20, 3, 20.0, 0.0), frame(30, 0, 0.0, 0.0)];
        assert!(run_engine_fullscreen(cfg.clone(), tap3()).is_empty(), "no action runs");
        assert!(run_engine_fullscreen(cfg.clone(), drag.clone()).is_empty(), "the drag never presses the button");

        cfg.settings.pause_in_fullscreen = false;
        assert!(!run_engine_fullscreen(cfg, drag).is_empty(), "unless the user turned pausing off");
    }

    #[test]
    fn double_tap_runs_its_own_action() {
        let mut cfg = Config::default();
        cfg.bindings.clear();
        cfg.set_binding(Trigger::Tap(3), Action::Mute, false);
        cfg.set_binding(Trigger::DoubleTap(3), Action::Lock, false);
        let frames =
            vec![frame(0, 3, 0.0, 0.0), frame(50, 0, 0.0, 0.0), frame(150, 3, 0.0, 0.0), frame(200, 0, 0.0, 0.0)];
        let (jobs, _) = run_engine(cfg, frames);
        assert_eq!(jobs, vec![Job::Run(Action::Lock)], "one double tap, not two single taps");
    }

    #[test]
    fn pointer_keeps_subpixel_remainder() {
        let mut p = Pointer::default();
        p.reset(0);
        let mut total = 0;
        for i in 1..=10 {
            total += p.step(0.05, 0.0, i * 10, 10.0, 0.0).0;
        }
        assert_eq!(total, 5, "10 × 0.05mm × 10px/mm = 5px, none lost to rounding");
    }

    #[test]
    fn test_actions_are_kept_in_range() {
        let jobs = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = unbounded::<TouchFrame>();
        let rec = jobs.clone();
        let engine = Engine::spawn(
            Arc::new(SharedConfig::new(Config::default())),
            rx,
            move || Box::new(Recorder { jobs: rec, app: None, fullscreen: false }),
            Arc::new(|_| {}),
        );
        engine.run_action(Action::Volume { step: i32::MAX });
        engine.run_action(Action::Brightness { step: i32::MIN });
        drop(tx);
        engine.join();
        assert_eq!(
            *jobs.lock().unwrap(),
            vec![Job::Run(Action::Volume { step: 100 }), Job::Run(Action::Brightness { step: -100 })]
        );
    }

    /// An engine whose events arrive on a channel, to wait for it to see a frame.
    struct Watched {
        engine: Engine,
        frames: Sender<TouchFrame>,
        config: Arc<SharedConfig>,
        events: Receiver<EngineEvent>,
        jobs: Arc<Mutex<Vec<Job>>>,
    }

    fn watched_engine(cfg: Config) -> Watched {
        let jobs = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = unbounded();
        let (ev_tx, ev_rx) = unbounded();
        let shared = Arc::new(SharedConfig::new(cfg));
        let rec = jobs.clone();
        let engine = Engine::spawn(
            shared.clone(),
            rx,
            move || Box::new(Recorder { jobs: rec, app: None, fullscreen: false }),
            Arc::new(move |e| {
                let _ = ev_tx.send(e);
            }),
        );
        Watched { engine, frames: tx, config: shared, events: ev_rx, jobs }
    }

    fn wait_for_frame(events: &Receiver<EngineEvent>) {
        loop {
            match events.recv_timeout(Duration::from_secs(10)) {
                Ok(EngineEvent::Frame(_)) => return,
                Ok(_) => {}
                Err(e) => panic!("the engine never saw the frame: {e}"),
            }
        }
    }

    #[test]
    fn a_lift_right_after_pausing_is_not_a_tap() {
        let mut cfg = Config { bindings: Vec::new(), ..Config::default() };
        cfg.set_binding(Trigger::Tap(3), Action::Lock, false);
        let Watched { engine, frames: tx, config: shared, events, jobs } = watched_engine(cfg);
        let t = now_ms();
        tx.send(frame(t, 3, 0.0, 0.0)).unwrap();
        wait_for_frame(&events);
        shared.update(|c| c.settings.enabled = false);
        tx.send(frame(t + 50, 0, 0.0, 0.0)).unwrap();
        drop(tx);
        engine.join();
        assert!(jobs.lock().unwrap().is_empty(), "{:?}", jobs.lock().unwrap());
    }

    #[test]
    fn resting_fingers_do_not_hold_once_gestures_are_paused() {
        let mut cfg = Config { bindings: Vec::new(), ..Config::default() };
        cfg.settings.hold_ms = 1_000;
        cfg.set_binding(Trigger::Hold(3), Action::Lock, false);
        let Watched { engine, frames: tx, config: shared, events, jobs } = watched_engine(cfg);
        tx.send(frame(now_ms(), 3, 0.0, 0.0)).unwrap();
        wait_for_frame(&events);
        // The hold timer is running; pausing must stop it.
        shared.update(|c| c.settings.enabled = false);
        std::thread::sleep(Duration::from_millis(1_300));
        drop(tx);
        engine.join();
        assert!(jobs.lock().unwrap().is_empty(), "{:?}", jobs.lock().unwrap());
    }

    #[test]
    fn a_stroke_too_broken_to_measure_says_so_when_recorded() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let (jobs, _) = unbounded();
        let mut engine = EngineLoop::new(
            Arc::new(SharedConfig::new(Config::default())),
            jobs,
            Arc::new(move |e| ev.lock().unwrap().push(e)),
            |_| {},
            Arc::new(AtomicBool::new(true)),
        );
        // Not straight and long enough, but too big to normalise in f32.
        let big = 3e38;
        engine.on_stroke(&[(0.0, 0.0), (big, 0.0), (big, big), (0.0, big)], 0);
        let events = events.lock().unwrap();
        assert!(
            matches!(events[..], [EngineEvent::ShapeRecorded { points: None, problem: Some(NotAShape::TooShort) }]),
            "{events:?}"
        );
    }

    #[test]
    fn a_huge_drag_step_does_not_stall_the_pointer() {
        let mut p = Pointer::default();
        p.reset(0);
        p.step(3e38, -3e38, 10, 9.0, 1.0);
        let total: i32 = (2..=11).map(|i| p.step(0.5, 0.0, i * 10, 10.0, 0.0).0).sum();
        assert_eq!(total, 50, "10 × 0.5mm × 10px/mm");
    }

    /// Records the mouse button, and can claim a full-screen game is running.
    struct Soak {
        buttons: Arc<Mutex<Vec<bool>>>,
        runs: Arc<AtomicUsize>,
        fullscreen: Arc<AtomicBool>,
    }

    impl Executor for Soak {
        fn run(&mut self, _: &Action) -> Result<Option<Level>, ActionError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
        fn mouse_button(&mut self, down: bool) -> Result<(), ActionError> {
            self.buttons.lock().unwrap().push(down);
            Ok(())
        }
        fn mouse_move(&mut self, _: i32, _: i32) -> Result<(), ActionError> {
            Ok(())
        }
        fn fullscreen_game(&mut self) -> bool {
            self.fullscreen.load(Ordering::SeqCst)
        }
    }

    struct SoakRun {
        buttons: Vec<bool>,
        runs: usize,
        frames: usize,
        events: Vec<EngineEvent>,
    }

    /// Random, often broken, frames and two-finger strokes, while the test
    /// thread swaps the config, toggles shape recording and full-screen
    /// games at random, unsynchronised with the engine.
    fn soak(seed: u64, frames: usize, change: impl Fn(&mut crate::Rng, &mut Config)) -> SoakRun {
        let mut rng = crate::Rng::new(seed);
        let (buttons, runs) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)));
        let fullscreen = Arc::new(AtomicBool::new(false));
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::new(AtomicUsize::new(0));
        let mut cfg = Config::default();
        change(&mut rng, &mut cfg);
        let shared = Arc::new(SharedConfig::new(cfg));
        let (tx, rx) = unbounded();
        let (b, r, f) = (buttons.clone(), runs.clone(), fullscreen.clone());
        let (ev, count) = (events.clone(), seen.clone());
        let engine = Engine::spawn(
            shared.clone(),
            rx,
            move || Box::new(Soak { buttons: b, runs: r, fullscreen: f }),
            Arc::new(move |e| match e {
                EngineEvent::Frame(_) => {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                e => ev.lock().unwrap().push(e),
            }),
        );
        let mut sent = 0;
        let mut t = now_ms();
        while sent < frames {
            match rng.below(200) {
                0 => {
                    shared.update(|c| change(&mut rng, c));
                }
                1 => engine.record_shape(rng.below(2) == 0),
                2 => fullscreen.store(rng.below(4) == 0, Ordering::SeqCst),
                _ => {}
            }
            let batch: Vec<TouchFrame> = if rng.below(30) == 0 {
                vee_frames(t.min(u64::MAX / 2))
            } else {
                t = match rng.below(50) {
                    0 => t.saturating_sub(rng.below(5_000)),
                    1 => t.saturating_add(rng.below(100_000)),
                    2 => u64::MAX - rng.below(100),
                    _ => t.saturating_add(rng.below(20)),
                };
                let n = [0, 0, 1, 2, 3, 3, 3, 4, 5, 6][rng.below(10) as usize];
                let mut f = frame(t, n, rng.below(30) as f32, rng.below(30) as f32);
                for c in &mut f.contacts {
                    if rng.below(30) == 0 {
                        c.x = rng.mm();
                    }
                    if rng.below(30) == 0 {
                        c.id = 0;
                    }
                }
                if rng.below(10) == 0 {
                    (f.width_mm, f.height_mm) = (0.0, 0.0);
                }
                vec![f]
            };
            sent += batch.len();
            batch.into_iter().for_each(|f| tx.send(f).unwrap());
        }
        // However far behind the engine is, at least one stroke is recorded.
        engine.record_shape(true);
        sent += vee_frames(now_ms()).into_iter().map(|f| tx.send(f).unwrap()).count();
        drop(tx);
        engine.join();
        let buttons = buttons.lock().unwrap().clone();
        let events = std::mem::take(&mut *events.lock().unwrap());
        let soaked =
            SoakRun { buttons, runs: runs.load(Ordering::SeqCst), frames: seen.load(Ordering::SeqCst), events };
        assert_eq!(soaked.frames, sent, "the engine stopped early (a panic?)");
        soaked
    }

    fn assert_button_balanced(buttons: &[bool]) {
        for (i, &down) in buttons.iter().enumerate() {
            assert_eq!(down, i % 2 == 0, "pressed twice or released twice: {buttons:?}");
        }
        assert!(buttons.len() % 2 == 0, "the button was left held: {buttons:?}");
    }

    fn assert_shapes_finite(events: &[EngineEvent]) {
        for e in events {
            if let EngineEvent::ShapeRecorded { points, problem } = e {
                assert!(points.is_some() != problem.is_some(), "{e:?}");
                assert!(points.iter().flatten().flatten().all(|v| v.is_finite()), "{e:?}");
            }
        }
    }

    #[test]
    fn soak_config_swaps_mid_drag_never_leave_the_button_held() {
        for seed in [1, 2, 3] {
            let run = soak(seed, 20_000, |rng, c| {
                c.settings.enabled = rng.below(4) != 0;
                c.settings.drag_release_ms = [0, 50, 350, 5_000][rng.below(4) as usize];
                c.settings.pause_in_fullscreen = rng.below(2) == 0;
                let drag = if rng.below(3) == 0 { Action::None } else { Action::Drag };
                c.set_binding(Trigger::Drag(3), drag, false);
                c.set_binding(Trigger::Tap(4), Action::Mute, false);
            });
            assert_button_balanced(&run.buttons);
            assert_shapes_finite(&run.events);
            assert!(!run.buttons.is_empty(), "seed {seed}: the soak should drag now and then");
        }
    }

    #[test]
    fn soak_while_paused_never_acts() {
        let run = soak(4, 20_000, |rng, c| {
            c.settings.enabled = false;
            c.settings.drag_release_ms = [0, 350][rng.below(2) as usize];
            c.set_binding(Trigger::Tap(3), Action::Mute, false);
        });
        assert!(run.buttons.is_empty() && run.runs == 0, "{:?}, {} actions", run.buttons, run.runs);
        assert_shapes_finite(&run.events);
        let gestures: Vec<_> = run.events.iter().filter(|e| matches!(e, EngineEvent::Gesture { .. })).collect();
        assert!(gestures.is_empty(), "{gestures:?}");
        assert!(run.events.iter().any(|e| matches!(e, EngineEvent::ShapeRecorded { .. })), "recording still works");
    }

    #[test]
    fn the_engine_keeps_up_with_a_flood_of_frames() {
        let mut frames = Vec::with_capacity(100_100);
        let mut t = 0;
        while frames.len() < 100_000 {
            for (n, dx, dy) in [(3, 0.0, 0.0), (4, 0.0, -1.5), (3, 1.2, 0.4), (4, 0.0, 0.0)] {
                for i in 0..40 {
                    frames.push(frame(t, n, dx * i as f32, dy * i as f32));
                    t += 8;
                }
                frames.push(frame(t, 0, 0.0, 0.0));
                t += 300;
            }
        }
        let count = frames.len();
        let seen = Arc::new(AtomicUsize::new(0));
        let s = seen.clone();
        let (tx, rx) = unbounded();
        let start = std::time::Instant::now();
        let engine = Engine::spawn(
            Arc::new(SharedConfig::new(Config::default())),
            rx,
            || Box::new(LogExecutor) as Box<dyn Executor>,
            Arc::new(move |e| {
                if let EngineEvent::Frame(_) = e {
                    s.fetch_add(1, Ordering::Relaxed);
                }
            }),
        );
        frames.into_iter().for_each(|f| tx.send(f).unwrap());
        drop(tx);
        engine.join();
        let took = start.elapsed();
        assert_eq!(seen.load(Ordering::Relaxed), count);
        // Tens of milliseconds in release; generous for debug builds on shared CI runners.
        let budget = Duration::from_millis(if cfg!(debug_assertions) { 15_000 } else { 500 });
        assert!(took < budget, "{count} frames took {took:?}");
    }

    /// The stroke [`vee_frames`] draws, normalised: a shape to bind.
    fn vee_shape(id: u8) -> crate::Shape {
        let mut strokes = StrokeTracker::default();
        let path = vee_frames(0).iter().find_map(|f| strokes.update(f)).expect("a stroke");
        crate::Shape { id, name: "V".into(), points: shapes::normalize(&path).expect("a shape") }
    }

    #[test]
    fn switching_recording_mid_stroke_decides_how_the_stroke_ends() {
        for record_at_the_end in [false, true] {
            let mut cfg = Config { bindings: Vec::new(), ..Config::default() };
            cfg.shapes.push(vee_shape(4));
            cfg.set_binding(Trigger::Shape(4), Action::Lock, false);
            let Watched { engine, frames, events, jobs, .. } = watched_engine(cfg);
            engine.record_shape(!record_at_the_end);
            let mut stroke = vee_frames(now_ms());
            let lift = stroke.pop().unwrap();
            for f in stroke {
                frames.send(f).unwrap();
                wait_for_frame(&events);
            }
            engine.record_shape(record_at_the_end);
            frames.send(lift).unwrap();
            drop(frames);
            engine.join();
            let recorded = events.try_iter().any(|e| matches!(e, EngineEvent::ShapeRecorded { points: Some(_), .. }));
            assert_eq!(recorded, record_at_the_end);
            let fired = *jobs.lock().unwrap() == vec![Job::Run(Action::Lock)];
            assert_eq!(fired, !record_at_the_end, "a recorded stroke doesn't also fire");
        }
    }
}
