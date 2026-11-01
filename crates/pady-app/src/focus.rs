//! The focus timer: a gesture starts it, the same gesture says how long is
//! left, and doing it twice in a row stops it. A corner notice (and a soft
//! sound) says when it's done, and a short break can follow. Nothing else
//! changes: it's a timer, not a blocker.
//!
//! Finished and stopped focus timers are kept in `focus.json` next to the
//! config (when, how long, and the label you gave it) for the Focus page.

use crate::notes::wall_ms;
use crate::stats::Stats;
use crate::{lock, Pady};
use pady_overlay::Icon;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// A second gesture within this long of the last one stops the timer.
const AGAIN_MS: u64 = 4_000;
/// Sessions kept for the Focus page's history.
const KEEP_SESSIONS: usize = 300;
const MAX_LABEL: usize = 80;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Focus,
    Break,
}

#[derive(Clone, Debug)]
struct Run {
    /// Wall-clock milliseconds.
    started: u64,
    ends: u64,
    minutes: u32,
    kind: Kind,
    label: String,
    last_poke: u64,
}

/// One focus timer that finished or was stopped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub started: u64,
    /// Minutes actually focused.
    pub minutes: u32,
    /// Minutes it was set for.
    pub planned: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Ran to the end (not stopped early).
    pub done: bool,
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    sessions: Vec<Session>,
}

#[derive(Default)]
pub struct Focus {
    path: PathBuf,
    run: Mutex<Option<Run>>,
    /// Bumped on every start and stop, so only the newest timer thread ends a run.
    runs: AtomicU64,
    sessions: Mutex<Vec<Session>>,
}

#[derive(Clone, Serialize)]
pub struct FocusState {
    running: bool,
    started: u64,
    ends: u64,
    minutes: u32,
    kind: Kind,
    label: String,
}

impl Focus {
    pub fn load(config: &Path) -> Focus {
        let path = config.with_file_name("focus.json");
        let sessions = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<File>(&text).ok())
            .map(|f| f.sessions)
            .unwrap_or_default();
        Focus { path, sessions: Mutex::new(sessions), ..Focus::default() }
    }

    fn state(&self) -> FocusState {
        match &*lock(&self.run) {
            Some(r) => FocusState {
                running: true,
                started: r.started,
                ends: r.ends,
                minutes: r.minutes,
                kind: r.kind,
                label: r.label.clone(),
            },
            None => {
                FocusState { running: false, started: 0, ends: 0, minutes: 0, kind: Kind::Focus, label: String::new() }
            }
        }
    }

    fn log(&self, session: Session) {
        let mut sessions = lock(&self.sessions);
        sessions.push(session);
        let extra = sessions.len().saturating_sub(KEEP_SESSIONS);
        sessions.drain(..extra);
        self.write(&sessions);
    }

    fn write(&self, sessions: &[Session]) {
        let text = match serde_json::to_string(&File { sessions: sessions.to_vec() }) {
            Ok(text) => text,
            Err(e) => return log::warn!("could not save focus history: {e}"),
        };
        let tmp = self.path.with_extension("json.tmp");
        let written = self
            .path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&tmp, text))
            .and_then(|()| std::fs::rename(&tmp, &self.path));
        if let Err(e) = written {
            log::warn!("could not save focus history: {e}");
        }
    }
}

/// The Focus gesture.
pub fn poke(app: &AppHandle) {
    let focus = app.state::<Focus>();
    let now = wall_ms();
    let run = lock(&focus.run).clone();
    match run {
        None => start(app, None, Kind::Focus, String::new()),
        Some(r) if now.saturating_sub(r.last_poke) <= AGAIN_MS => stop(app),
        Some(r) => {
            if let Some(run) = lock(&focus.run).as_mut() {
                run.last_poke = now;
            }
            let left = r.ends.saturating_sub(now).div_ceil(60_000);
            let what = if r.kind == Kind::Break { "Break" } else { "Focus" };
            crate::quick::notice(app, format!("{what} · {left} min left · again to stop"), Icon::Timer);
        }
    }
}

fn start(app: &AppHandle, minutes: Option<u32>, kind: Kind, label: String) {
    let settings = app.state::<Pady>().config.get().settings;
    let minutes = match kind {
        Kind::Focus => minutes.unwrap_or(settings.focus_minutes).clamp(1, 240),
        Kind::Break => minutes.unwrap_or(settings.break_minutes).clamp(1, 60),
    };
    let label: String = label.trim().chars().take(MAX_LABEL).collect();
    let now = wall_ms();
    let ends = now + u64::from(minutes) * 60_000;
    let focus = app.state::<Focus>();
    let ticket = focus.runs.fetch_add(1, Ordering::SeqCst) + 1;
    // Starting over a running focus timer keeps what was done so far.
    let old = lock(&focus.run).replace(Run { started: now, ends, minutes, kind, label: label.clone(), last_poke: now });
    if let Some(old) = old {
        finish(app, &old, false);
    }
    let text = match (kind, label.is_empty()) {
        (Kind::Break, _) => format!("Break · {minutes} min"),
        (Kind::Focus, true) => format!("Focus · {minutes} min"),
        (Kind::Focus, false) => format!("Focus · {minutes} min · {label}"),
    };
    crate::quick::notice(app, text, Icon::Timer);
    changed(app);
    let app = app.clone();
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
        let Some(run) = lock(&focus.run).take() else { return };
        finish(&app, &run, true);
        let settings = app.state::<Pady>().config.get().settings;
        if settings.focus_sound {
            chime();
        }
        match run.kind {
            Kind::Focus if settings.focus_auto_break => start(&app, None, Kind::Break, String::new()),
            Kind::Focus => {
                crate::quick::notice(&app, format!("Focus done · {} min · take a break", run.minutes), Icon::Done);
                changed(&app);
            }
            Kind::Break => {
                crate::quick::notice(&app, "Break's over · ready to focus".to_string(), Icon::Done);
                changed(&app);
            }
        }
    });
}

fn stop(app: &AppHandle) {
    let focus = app.state::<Focus>();
    focus.runs.fetch_add(1, Ordering::SeqCst);
    let Some(run) = lock(&focus.run).take() else { return };
    let minutes = finish(app, &run, false);
    let what = if run.kind == Kind::Break { "Break" } else { "Focus" };
    crate::quick::notice(app, format!("{what} stopped · {minutes} min"), Icon::Timer);
    changed(app);
}

/// Count and keep a focus run that ended; breaks aren't kept. Returns the
/// minutes it ran.
fn finish(app: &AppHandle, run: &Run, done: bool) -> u32 {
    let minutes = if done { run.minutes } else { (wall_ms().saturating_sub(run.started) / 60_000) as u32 };
    if run.kind == Kind::Break {
        return minutes;
    }
    if app.state::<Pady>().config.get().settings.keep_stats {
        app.state::<Stats>().focused(minutes);
    }
    // A timer stopped within the first minute isn't worth a line.
    if done || minutes > 0 {
        app.state::<Focus>().log(Session {
            started: run.started,
            minutes,
            planned: run.minutes,
            label: run.label.clone(),
            done,
        });
    }
    minutes
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
pub fn focus_start(app: AppHandle, minutes: u32, kind: Option<Kind>, label: Option<String>) {
    start(&app, Some(minutes), kind.unwrap_or_default(), label.unwrap_or_default());
}

#[tauri::command]
pub fn focus_stop(app: AppHandle) {
    stop(&app);
}

/// Past focus timers, newest last.
#[tauri::command]
pub fn focus_history(focus: State<Focus>) -> Vec<Session> {
    lock(&focus.sessions).clone()
}

#[tauri::command]
pub fn focus_history_clear(focus: State<Focus>) {
    let mut sessions = lock(&focus.sessions);
    sessions.clear();
    focus.write(&sessions);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_kept_capped_and_read_back() {
        let dir = std::env::temp_dir().join(format!("pady-focus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let focus = Focus::load(&dir.join("config.toml"));
        for i in 0..KEEP_SESSIONS as u64 + 5 {
            focus.log(Session { started: i, minutes: 25, planned: 25, label: "write".into(), done: true });
        }
        let again = Focus::load(&dir.join("config.toml"));
        let sessions = lock(&again.sessions);
        assert_eq!(sessions.len(), KEEP_SESSIONS);
        assert_eq!(sessions[0].started, 5);
        assert_eq!(sessions.last().unwrap().label, "write");
    }
}
