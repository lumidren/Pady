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
use crate::recognizer::Recognizer;
use crate::trigger::Trigger;
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use serde::Serialize;
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
}

pub type Observer = Arc<dyn Fn(EngineEvent) + Send + Sync>;

pub struct Engine {
    jobs: Sender<Job>,
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
        let (jobs_tx, jobs_rx) = unbounded::<Job>();

        let action_observer = observer.clone();
        let action_config = config.clone();
        let action_thread = std::thread::Builder::new()
            .name("pady-actions".into())
            .spawn(move || ActionLoop::new(make_executor(), action_config, action_observer).run(jobs_rx))
            .expect("spawn action thread");

        let engine_jobs = jobs_tx.clone();
        let engine_thread = std::thread::Builder::new()
            .name("pady-engine".into())
            .spawn(move || EngineLoop::new(config, engine_jobs, observer).run(frames))
            .expect("spawn engine thread");

        Engine { jobs: jobs_tx, engine_thread, action_thread }
    }

    /// Run an action right away (the UI's "Test" button).
    pub fn run_action(&self, action: Action) {
        let _ = self.jobs.send(Job::Run(action));
    }

    /// Wait for the engine to stop (after the frame source closes) and for
    /// queued actions to finish.
    pub fn join(self) {
        let Engine { jobs, engine_thread, action_thread } = self;
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
}

impl ActionLoop {
    fn new(exec: Box<dyn Executor>, shared: Arc<SharedConfig>, observer: Observer) -> Self {
        ActionLoop { exec, generation: shared.generation(), config: shared.get(), shared, observer }
    }

    fn run(mut self, jobs: Receiver<Job>) {
        for job in jobs {
            match job {
                Job::Run(action) => self.execute(&action),
                Job::Fire { gesture, trigger } => self.fire(gesture, trigger),
                Job::MouseButton(down) => {
                    let result = self.exec.mouse_button(down);
                    self.report("Mouse button", result);
                }
                Job::MouseMove(dx, dy) => {
                    let result = self.exec.mouse_move(dx, dy);
                    self.report("Mouse move", result);
                }
            }
        }
    }

    fn fire(&mut self, gesture: Gesture, trigger: Trigger) {
        let generation = self.shared.generation();
        if generation != self.generation {
            self.generation = generation;
            self.config = self.shared.get();
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
        self.rem = (x - ix, y - iy);
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
}

impl EngineLoop {
    fn new(shared: Arc<SharedConfig>, jobs: Sender<Job>, observer: Observer) -> Self {
        let config = shared.get();
        EngineLoop {
            generation: shared.generation(),
            recognizer: Recognizer::new(config.recognizer_settings()),
            config,
            shared,
            jobs,
            observer,
            pointer: Pointer::default(),
        }
    }

    fn run(mut self, frames: Receiver<TouchFrame>) {
        loop {
            self.refresh_config();
            match frames.recv_timeout(Duration::from_millis(16)) {
                Ok(frame) => self.on_frame(frame),
                Err(RecvTimeoutError::Timeout) => {
                    let gestures = self.recognizer.tick(now_ms());
                    self.dispatch(gestures, now_ms());
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        let gestures = self.recognizer.reset();
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
        (self.observer)(EngineEvent::Frame(frame));
        self.dispatch(gestures, t);
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
    use std::sync::Mutex;

    struct Recorder {
        jobs: Arc<Mutex<Vec<Job>>>,
        app: Option<String>,
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
            move || Box::new(Recorder { jobs: rec, app }),
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
        assert_eq!(jobs, vec![Job::Run(Action::Volume { step: 2 }); 3]);
        let levels: Vec<_> = events.iter().filter(|e| matches!(e, EngineEvent::Level(_))).collect();
        assert_eq!(levels.len(), 3, "every volume change reports its level for the overlay");
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
    fn pointer_keeps_subpixel_remainder() {
        let mut p = Pointer::default();
        p.reset(0);
        let mut total = 0;
        for i in 1..=10 {
            total += p.step(0.05, 0.0, i * 10, 10.0, 0.0).0;
        }
        assert_eq!(total, 5, "10 × 0.05mm × 10px/mm = 5px, none lost to rounding");
    }
}
