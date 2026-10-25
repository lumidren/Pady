//! Pady desktop app: a tray-resident gesture engine with a settings window.
//!
//! The window is a static web UI (`ui/`) talking to the commands below.
//! Closing it hides to the tray; the engine keeps running until "Quit".

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use crossbeam_channel::Sender;
use pady_actions::Conflict;
use pady_core::engine::Observer;
use pady_core::{Action, Config, Engine, EngineEvent, SharedConfig, TouchFrame};
use pady_input::{DeviceInfo, InputHandle};
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent, Wry};

struct Pady {
    config: Arc<SharedConfig>,
    config_path: PathBuf,
    /// Last modification time we know about, to spot hand edits of the file.
    config_mtime: Mutex<Option<SystemTime>>,
    engine: Engine,
    frames: Sender<TouchFrame>,
    input: Mutex<Result<InputHandle, String>>,
    /// Stream frames to the UI only while its Live page is open.
    live: Arc<AtomicBool>,
    tray_toggle: Mutex<Option<CheckMenuItem<Wry>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Everything the UI renders, sent whole so it never drifts from the backend.
#[derive(Serialize)]
struct AppState {
    config: Config,
    devices: Vec<DeviceInfo>,
    input_error: Option<String>,
    conflicts: Vec<Conflict>,
    shadowed: Vec<String>,
    autostart: bool,
    config_path: String,
    platform: &'static str,
    version: &'static str,
}

impl Pady {
    fn snapshot(&self) -> AppState {
        let config = self.config.get();
        let (devices, input_error) = match &*lock(&self.input) {
            Ok(input) => (input.devices(), None),
            Err(e) => (Vec::new(), Some(e.clone())),
        };
        AppState {
            conflicts: pady_actions::conflicts(&config),
            shadowed: config.shadowed_bindings().iter().map(ToString::to_string).collect(),
            autostart: pady_actions::autostart_enabled(),
            config_path: self.config_path.display().to_string(),
            platform: if cfg!(windows) { "windows" } else { "linux" },
            version: env!("CARGO_PKG_VERSION"),
            config,
            devices,
            input_error,
        }
    }

    /// Persist and activate a config, then tell every listener.
    fn apply(&self, app: &AppHandle, mut config: Config) -> Result<AppState, String> {
        config.normalize();
        config.save(&self.config_path).map_err(|e| e.to_string())?;
        *lock(&self.config_mtime) = modified(&self.config_path);
        self.config.set(config);
        self.sync_tray();
        let state = self.snapshot();
        let _ = app.emit("pady://state", &state);
        Ok(state)
    }

    fn sync_tray(&self) {
        let enabled = self.config.get().settings.enabled;
        if let Some(item) = &*lock(&self.tray_toggle) {
            let _ = item.set_checked(enabled);
            let _ = item.set_text(if enabled { "Gestures on" } else { "Gestures paused" });
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

// ---------------------------------------------------------------------------
// Commands

#[tauri::command]
fn get_state(pady: State<Pady>) -> AppState {
    pady.snapshot()
}

#[tauri::command]
fn save_config(app: AppHandle, pady: State<Pady>, config: Config) -> Result<AppState, String> {
    pady.apply(&app, config)
}

#[tauri::command]
fn set_enabled(app: AppHandle, pady: State<Pady>, enabled: bool) -> Result<AppState, String> {
    let mut config = pady.config.get();
    config.settings.enabled = enabled;
    pady.apply(&app, config)
}

#[tauri::command]
fn reset_config(app: AppHandle, pady: State<Pady>) -> Result<AppState, String> {
    pady.apply(&app, Config::default())
}

#[tauri::command]
fn set_autostart(pady: State<Pady>, enabled: bool) -> Result<AppState, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    pady_actions::set_autostart(enabled, &exe).map_err(|e| e.to_string())?;
    Ok(pady.snapshot())
}

#[tauri::command]
fn test_action(pady: State<Pady>, action: Action) {
    pady.engine.run_action(action);
}

#[tauri::command]
fn set_live(pady: State<Pady>, live: bool) {
    pady.live.store(live, Ordering::Relaxed);
}

#[tauri::command]
fn open_config_folder(pady: State<Pady>) -> Result<(), String> {
    let dir = pady.config_path.parent().ok_or("config has no folder")?;
    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
    std::process::Command::new(opener).arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_touchpad_settings() -> Result<(), String> {
    pady_actions::open_touchpad_settings().map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Setup

fn load_config(path: &Path) -> Config {
    match Config::load_or_create(path) {
        Ok(cfg) => cfg,
        Err(e) => {
            // Keep the user's broken file for reference rather than losing it.
            log::warn!("{e}; starting with defaults");
            let backup = path.with_extension("broken.toml");
            if std::fs::rename(path, &backup).is_ok() {
                log::warn!("moved the unreadable config to {}", backup.display());
            }
            let cfg = Config::default();
            let _ = cfg.save(path);
            cfg
        }
    }
}

fn observer(app: AppHandle, live: Arc<AtomicBool>) -> Observer {
    let last_frame = AtomicU64::new(0);
    Arc::new(move |event| match event {
        EngineEvent::Frame(frame) => {
            if !live.load(Ordering::Relaxed) {
                return;
            }
            // ~60 fps is plenty for drawing; always send the "all lifted" frame.
            let last = last_frame.load(Ordering::Relaxed);
            if frame.contacts.is_empty() || frame.time_ms.saturating_sub(last) >= 16 {
                last_frame.store(frame.time_ms, Ordering::Relaxed);
                let _ = app.emit("pady://frame", &frame);
            }
        }
        EngineEvent::Gesture { gesture, trigger, action } => {
            let _ = app.emit("pady://gesture", json!({ "gesture": gesture, "trigger": trigger, "action": action }));
        }
        EngineEvent::ActionFailed { action, error } => {
            let _ = app.emit("pady://action-failed", json!({ "action": action, "error": error }));
        }
    })
}

/// Background housekeeping: retry the touchpad if it was missing (e.g. a
/// USB touchpad plugged in later) and pick up hand edits to the config file.
fn spawn_watcher(app: AppHandle) {
    std::thread::Builder::new()
        .name("pady-watcher".into())
        .spawn(move || {
            let mut last_retry = Instant::now();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let pady = app.state::<Pady>();
                let mut changed = false;

                if lock(&pady.input).is_err() && last_retry.elapsed() >= Duration::from_secs(5) {
                    last_retry = Instant::now();
                    if let Ok(input) = pady_input::start(pady.frames.clone()) {
                        log::info!("touchpad found");
                        *lock(&pady.input) = Ok(input);
                        changed = true;
                    }
                }

                let mtime = modified(&pady.config_path);
                if mtime != *lock(&pady.config_mtime) {
                    *lock(&pady.config_mtime) = mtime;
                    match Config::load(&pady.config_path) {
                        Ok(cfg) if cfg != pady.config.get() => {
                            log::info!("config file changed on disk; reloading");
                            pady.config.set(cfg);
                            pady.sync_tray();
                            changed = true;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            let _ = app.emit(
                                "pady://action-failed",
                                json!({ "action": "Reload config", "error": e.to_string() }),
                            );
                        }
                    }
                }

                if changed {
                    let _ = app.emit("pady://state", &pady.snapshot());
                }
            }
        })
        .expect("spawn watcher thread");
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn toggle_enabled(app: &AppHandle) {
    let pady = app.state::<Pady>();
    let mut config = pady.config.get();
    config.settings.enabled = !config.settings.enabled;
    if let Err(e) = pady.apply(app, config) {
        log::warn!("could not save config: {e}");
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<CheckMenuItem<Wry>> {
    let enabled = app.state::<Pady>().config.get().settings.enabled;
    let toggle = CheckMenuItem::with_id(app, "toggle", "Gestures on", true, enabled, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Pady…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Pady", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &open, &PredefinedMenuItem::separator(app)?, &quit])?;

    let mut tray = TrayIconBuilder::with_id("pady")
        .tooltip("Pady — touchpad gestures")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_enabled(app),
            "open" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(toggle)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let start_hidden = std::env::args().any(|a| a == "--hidden");

    tauri::Builder::default()
        // A second launch (e.g. from the Start menu while autostarted) just
        // shows the running instance instead of doubling every gesture.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_config,
            set_enabled,
            reset_config,
            set_autostart,
            test_action,
            set_live,
            open_config_folder,
            open_touchpad_settings,
        ])
        .setup(move |app| {
            let config_path = Config::default_path()?;
            let config = Arc::new(SharedConfig::new(load_config(&config_path)));
            let live = Arc::new(AtomicBool::new(false));

            let (frames_tx, frames_rx) = crossbeam_channel::unbounded();
            let input = pady_input::start(frames_tx.clone()).map_err(|e| e.to_string());
            if let Err(e) = &input {
                log::warn!("touchpad unavailable: {e}");
            }
            let engine = Engine::spawn(
                config.clone(),
                frames_rx,
                pady_actions::executor,
                observer(app.handle().clone(), live.clone()),
            );

            app.manage(Pady {
                config_mtime: Mutex::new(modified(&config_path)),
                config,
                config_path,
                engine,
                frames: frames_tx,
                input: Mutex::new(input),
                live,
                tray_toggle: Mutex::new(None),
            });

            let toggle = build_tray(app.handle())?;
            *lock(&app.state::<Pady>().tray_toggle) = Some(toggle);
            app.state::<Pady>().sync_tray();
            spawn_watcher(app.handle().clone());

            if !start_hidden {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // Keep gestures working: hide to the tray instead of quitting.
                api.prevent_close();
                let _ = window.hide();
                window.state::<Pady>().live.store(false, Ordering::Relaxed);
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running Pady");
}
