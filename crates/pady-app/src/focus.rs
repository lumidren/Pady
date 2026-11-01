//! The focus timer: a gesture starts it, the same gesture says how long is
//! left, and doing it twice in a row stops it. A corner notice (and a soft
//! sound) says when it's done. Nothing else changes: it's a timer, not a
//! blocker.

use crate::notes::wall_ms;
use crate::stats::Stats;
use crate::{lock, Pady};
use pady_overlay::Icon;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// A second gesture within this long of the last one stops the timer.
const AGAIN_MS: u64 = 4_000;

#[derive(Clone, Copy, Debug, Serialize)]
struct Run {
    /// Wall-clock milliseconds.
    started: u64,
    ends: u64,
    minutes: u32,
    #[serde(skip)]
    last_poke: u64,
}

#[derive(Default)]
pub struct Focus {
    run: Mutex<Option<Run>>,
    /// Bumped on every start and stop, so only the newest timer thread ends a run.
    runs: AtomicU64,
}

#[derive(Clone, Serialize)]
pub struct FocusState {
    running: bool,
    started: u64,
    ends: u64,
    minutes: u32,
}

impl Focus {
    fn state(&self) -> FocusState {
        match *lock(&self.run) {
            Some(r) => FocusState { running: true, started: r.started, ends: r.ends, minutes: r.minutes },
            None => FocusState { running: false, started: 0, ends: 0, minutes: 0 },
        }
    }
}

/// The Focus gesture.
pub fn poke(app: &AppHandle) {
    let focus = app.state::<Focus>();
    let now = wall_ms();
    let run = *lock(&focus.run);
    match run {
        None => start(app, None),
        Some(r) if now.saturating_sub(r.last_poke) <= AGAIN_MS => stop(app),
        Some(r) => {
            if let Some(run) = lock(&focus.run).as_mut() {
                run.last_poke = now;
            }
            let left = r.ends.saturating_sub(now).div_ceil(60_000);
            crate::quick::notice(app, format!("Focus · {left} min left · again to stop"), Icon::Timer);
        }
    }
}

fn start(app: &AppHandle, minutes: Option<u32>) {
    let minutes = minutes.unwrap_or_else(|| app.state::<Pady>().config.get().settings.focus_minutes).clamp(1, 240);
    let now = wall_ms();
    let focus = app.state::<Focus>();
    *lock(&focus.run) = Some(Run { started: now, ends: now + u64::from(minutes) * 60_000, minutes, last_poke: now });
    let ticket = focus.runs.fetch_add(1, Ordering::SeqCst) + 1;
    crate::quick::notice(app, format!("Focus · {minutes} min"), Icon::Timer);
    changed(app);
    let app = app.clone();
    let ends = now + u64::from(minutes) * 60_000;
    std::thread::spawn(move || {
        // By the wall clock, so time asleep (the laptop's lid shut) counts.
        let focus = app.state::<Focus>();
        while wall_ms() < ends {
            std::thread::sleep(Duration::from_millis(ends.saturating_sub(wall_ms()).clamp(1, 5_000)));
            if focus.runs.load(Ordering::SeqCst) != ticket {
                return; // stopped, or started again
            }
        }
        if focus.runs.load(Ordering::SeqCst) != ticket {
            return;
        }
        lock(&focus.run).take();
        record(&app, minutes);
        crate::quick::notice(&app, format!("Focus done · {minutes} min · take a break"), Icon::Done);
        chime();
        changed(&app);
    });
}

fn stop(app: &AppHandle) {
    let focus = app.state::<Focus>();
    focus.runs.fetch_add(1, Ordering::SeqCst);
    let Some(run) = lock(&focus.run).take() else { return };
    let minutes = (wall_ms().saturating_sub(run.started) / 60_000) as u32;
    record(app, minutes);
    crate::quick::notice(app, format!("Focus stopped · {minutes} min"), Icon::Timer);
    changed(app);
}

fn record(app: &AppHandle, minutes: u32) {
    if app.state::<Pady>().config.get().settings.keep_stats {
        app.state::<Stats>().focused(minutes);
    }
}

fn changed(app: &AppHandle) {
    let _ = app.emit_to("main", "pady://focus", app.state::<Focus>().state());
}

#[cfg(windows)]
fn chime() {
    use windows::Win32::System::Diagnostics::Debug::MessageBeep;
    use windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK;
    let _ = unsafe { MessageBeep(MB_ICONASTERISK) };
}

#[cfg(not(windows))]
fn chime() {}

// ---------------------------------------------------------------------------
// Commands (the main window)

#[tauri::command]
pub fn focus_get(focus: State<Focus>) -> FocusState {
    focus.state()
}

#[tauri::command]
pub fn focus_start(app: AppHandle, minutes: u32) {
    start(&app, Some(minutes));
}

#[tauri::command]
pub fn focus_stop(app: AppHandle) {
    stop(&app);
}
