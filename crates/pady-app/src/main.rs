//! Pady desktop app: a tray-resident gesture engine with a settings window.
//!
//! The window is a static web UI (`ui/`) talking to the commands below.
//! It is created when opened and destroyed when closed, so Pady in the tray
//! holds no WebView; the engine keeps running until "Quit".

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod update;

use crossbeam_channel::Sender;
use pady_actions::Conflict;
use pady_core::engine::Observer;
use pady_core::{Action, Config, Engine, EngineEvent, Level, Preset, SharedConfig, TouchFrame, PRESETS};
use pady_input::{DeviceInfo, InputHandle};
use serde::Serialize;
use serde_json::json;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, RunEvent, State, WindowEvent, Wry};

/// Must match `identifier` in tauri.conf.json: the log lives beside the
/// WebView data, so "Delete the application data" on uninstall removes it.
const IDENTIFIER: &str = "com.lumidren.pady";

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
    presets: &'static [Preset],
}

impl Pady {
    fn snapshot(&self) -> AppState {
        let config = self.config.get();
        let (devices, input_error) = match &*lock(&self.input) {
            Ok(input) => (input.devices(), None),
            Err(e) => (Vec::new(), Some(e.clone())),
        };
        let mut conflicts = pady_actions::conflicts(&config);
        if cfg!(target_os = "linux") && input_error.as_deref().is_some_and(|e| e.starts_with("permission denied")) {
            conflicts.insert(
                0,
                Conflict {
                    message: "Pady isn't allowed to read your touchpad yet.".into(),
                    fix: "Click Fix permissions and enter your password once.".into(),
                    can_open_settings: false,
                    can_fix: true,
                },
            );
        }
        AppState {
            conflicts,
            shadowed: config.shadowed_bindings().iter().map(ToString::to_string).collect(),
            autostart: pady_actions::autostart_enabled(),
            config_path: self.config_path.display().to_string(),
            platform: if cfg!(windows) { "windows" } else { "linux" },
            version: env!("CARGO_PKG_VERSION"),
            presets: PRESETS,
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
    // Resetting gestures shouldn't replay the first-run tutorial.
    let mut config = Config::default();
    config.settings.onboarded = pady.config.get().settings.onboarded;
    pady.apply(&app, config)
}

/// Replace the All-apps bindings with a starter preset's.
#[tauri::command]
fn apply_preset(app: AppHandle, pady: State<Pady>, id: String) -> Result<AppState, String> {
    let mut config = pady.config.get();
    if !config.apply_preset(&id) {
        return Err(format!("unknown preset `{id}`"));
    }
    pady.apply(&app, config)
}

#[tauri::command]
fn set_autostart(app: AppHandle, pady: State<Pady>, enabled: bool) -> Result<AppState, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    pady_actions::set_autostart(enabled, &exe).map_err(|e| e.to_string())?;
    let mut config = pady.config.get();
    config.settings.autostart_configured = true;
    pady.apply(&app, config)
}

/// Pady is meant to run all the time: on the first launch it registers itself
/// to start with the system (in the tray, `--hidden`). Turning that off in
/// Settings is remembered, and later launches leave it alone.
fn enable_autostart_once(app: &AppHandle) {
    let pady = app.state::<Pady>();
    let mut config = pady.config.get();
    if config.settings.autostart_configured {
        return;
    }
    let result = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
        if pady_actions::autostart_enabled() {
            Ok(())
        } else {
            pady_actions::set_autostart(true, &exe).map_err(|e| e.to_string())
        }
    });
    match result {
        Ok(()) => {
            log::info!("Pady now starts with the system");
            config.settings.autostart_configured = true;
            if let Err(e) = pady.apply(app, config) {
                log::warn!("could not save config: {e}");
            }
        }
        Err(e) => log::warn!("could not set up start-up: {e}"),
    }
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

/// Install the system permissions Pady needs (Linux: one password prompt),
/// then try the touchpad again straight away.
#[tauri::command]
async fn fix_permissions(app: AppHandle) -> Result<AppState, String> {
    tauri::async_runtime::spawn_blocking(|| {
        pady_actions::fix_permissions().map_err(|e| e.to_string())?;
        // Give udev a moment to apply the new access rights.
        std::thread::sleep(Duration::from_millis(500));
        Ok::<(), String>(())
    })
    .await
    .map_err(|e| e.to_string())??;
    let pady = app.state::<Pady>();
    if lock(&pady.input).is_err() {
        if let Ok(input) = pady_input::start(pady.frames.clone()) {
            log::info!("touchpad readable after fixing permissions");
            *lock(&pady.input) = Ok(input);
        }
    }
    let state = pady.snapshot();
    let _ = app.emit("pady://state", &state);
    Ok(state)
}

/// Wait for the user to switch to another app and return its executable
/// name, so the profile editor can add it without anyone typing `.exe`
/// names. Pady's own windows don't count. `None` on timeout.
#[tauri::command]
async fn detect_app(timeout_ms: u64) -> Option<String> {
    let own =
        std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()));
    tauri::async_runtime::spawn_blocking(move || {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms.clamp(1_000, 30_000));
        while Instant::now() < deadline {
            if let Some(app) = pady_actions::foreground_app() {
                if Some(app.to_ascii_lowercase()) != own {
                    return Some(app);
                }
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        None
    })
    .await
    .ok()
    .flatten()
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

/// The WebView version of the volume/brightness pill, used where the native
/// one (`pady-overlay`, Windows) isn't available. It shows near the bottom
/// of the screen, can't take focus or clicks, and hides once levels stop.
struct WebOverlay {
    app: AppHandle,
    /// Bumped per level so only the newest hide timer acts.
    shown: Arc<AtomicU64>,
}

impl WebOverlay {
    const LINGER: Duration = Duration::from_millis(1300);

    fn show(&self, level: &Level) {
        let Some(window) = self.app.get_webview_window("overlay") else { return };
        let _ = window.emit("pady://level", level);
        if !overlay_window::is_visible(&window) {
            // Appear on the screen the user is looking at: the one under the cursor.
            let under_cursor =
                self.app.cursor_position().ok().and_then(|p| self.app.monitor_from_point(p.x, p.y).ok().flatten());
            let monitor = under_cursor.or_else(|| window.primary_monitor().ok().flatten());
            if let (Some(monitor), Ok(size)) = (monitor, window.outer_size()) {
                let (area, scale) = (monitor.size(), monitor.scale_factor());
                let x = monitor.position().x + (area.width as i32 - size.width as i32) / 2;
                let y = monitor.position().y + area.height as i32 - size.height as i32 - (110.0 * scale) as i32;
                let _ = window.set_position(PhysicalPosition::new(x, y));
            }
            let _ = window.set_ignore_cursor_events(true);
            overlay_window::show_without_focus(&window);
        }
        let ticket = self.shown.fetch_add(1, Ordering::SeqCst) + 1;
        let shown = self.shown.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Self::LINGER);
            if shown.load(Ordering::SeqCst) == ticket {
                overlay_window::hide(&window);
            }
        });
    }
}

/// Showing the overlay must never take keyboard focus from the app the
/// gesture was aimed at. On Windows, Tauri's `show()` activates the window
/// even when it isn't focusable, so the Win32 calls are made directly; the
/// async variants are safe from the engine threads that show the overlay.
#[cfg(windows)]
mod overlay_window {
    use tauri::WebviewWindow;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        IsWindowVisible, SetWindowPos, ShowWindowAsync, HWND_TOPMOST, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE,
    };

    fn hwnd(window: &WebviewWindow) -> Option<HWND> {
        window.hwnd().ok().map(|h| HWND(h.0 as _))
    }

    pub fn is_visible(window: &WebviewWindow) -> bool {
        hwnd(window).is_some_and(|h| unsafe { IsWindowVisible(h) }.as_bool())
    }

    pub fn show_without_focus(window: &WebviewWindow) {
        if let Some(h) = hwnd(window) {
            unsafe {
                let _ = ShowWindowAsync(h, SW_SHOWNOACTIVATE);
                let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS;
                let _ = SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, flags);
            }
        }
    }

    pub fn hide(window: &WebviewWindow) {
        if let Some(h) = hwnd(window) {
            unsafe {
                let _ = ShowWindowAsync(h, SW_HIDE);
            }
        }
    }
}

/// GTK honours `focusable: false`, so Tauri's own calls never steal focus here.
#[cfg(not(windows))]
mod overlay_window {
    use tauri::WebviewWindow;

    pub fn is_visible(window: &WebviewWindow) -> bool {
        window.is_visible().unwrap_or(false)
    }

    pub fn show_without_focus(window: &WebviewWindow) {
        let _ = window.show();
    }

    pub fn hide(window: &WebviewWindow) {
        let _ = window.hide();
    }
}

fn observer(
    app: AppHandle,
    live: Arc<AtomicBool>,
    config: Arc<SharedConfig>,
    native_overlay: Option<pady_overlay::Overlay>,
) -> Observer {
    let last_frame = AtomicU64::new(0);
    let web_overlay = WebOverlay { app: app.clone(), shown: Arc::new(AtomicU64::new(0)) };
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
        EngineEvent::Gesture { gesture, trigger, action, profile } => {
            let payload = json!({ "gesture": gesture, "trigger": trigger, "action": action, "profile": profile });
            let _ = app.emit("pady://gesture", payload);
        }
        EngineEvent::Level(level) => {
            if config.get().settings.show_overlay {
                match &native_overlay {
                    Some(native) => native.show(level),
                    None => web_overlay.show(&level),
                }
            }
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
                std::thread::sleep(Duration::from_secs(2));
                let pady = app.state::<Pady>();
                let mut changed = false;

                if lock(&pady.input).is_err() && last_retry.elapsed() >= Duration::from_secs(10) {
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

/// Open the settings window, creating it if needed. It's destroyed when
/// closed, so a Pady sitting in the tray holds no WebView (or its memory).
pub(crate) fn show_main(app: &AppHandle) {
    let window = match app.get_webview_window("main") {
        Some(window) => window,
        None => {
            let Some(config) = app.config().app.windows.iter().find(|w| w.label == "main").cloned() else {
                return;
            };
            match tauri::WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build()) {
                Ok(window) => window,
                Err(e) => {
                    log::error!("could not open the Pady window: {e}");
                    return;
                }
            }
        }
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
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
    let updates = MenuItem::with_id(app, "update", update::TRAY_LABEL, true, None::<&str>)?;
    *lock(&app.state::<update::Updates>().tray_item) = Some(updates.clone());
    let quit = MenuItem::with_id(app, "quit", "Quit Pady", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &open, &updates, &PredefinedMenuItem::separator(app)?, &quit])?;

    let mut tray = TrayIconBuilder::with_id("pady")
        .tooltip("Pady — touchpad gestures")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_enabled(app),
            "open" => show_main(app),
            "update" => update::tray_clicked(app),
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

/// Log to a file as well as stderr (a release build has no console), so
/// support questions come with a trail. Panics are logged too. The file is
/// rotated once it passes 1 MB.
fn init_logging() {
    let dir = dirs::data_local_dir().map(|d| d.join(IDENTIFIER).join("logs"));
    let file = dir.as_ref().and_then(|dir| {
        std::fs::create_dir_all(dir).ok()?;
        let path = dir.join("pady.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1_000_000) {
            let _ = std::fs::rename(&path, dir.join("pady.old.log"));
        }
        std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
    });
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if let Some(file) = file {
        builder.target(env_logger::Target::Pipe(Box::new(Tee(file))));
    }
    builder.init();

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("panic: {info}");
        default_hook(info);
    }));
    if let Some(dir) = dir {
        log::info!("Pady {} starting; logging to {}", env!("CARGO_PKG_VERSION"), dir.display());
    }
}

/// Copies log output to stderr and the log file.
struct Tee(std::fs::File);

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stderr().write_all(buf);
        self.0.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// A newer release, if there is one (also announced as `pady://update`).
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<Option<update::Available>, String> {
    update::check(&app).await
}

/// Download, verify and install the newer release, then restart.
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    update::install(app).await
}

fn main() {
    init_logging();
    let start_hidden = std::env::args().any(|a| a == "--hidden");

    tauri::Builder::default()
        // A second launch (e.g. from the Start menu while autostarted) just
        // shows the running instance instead of doubling every gesture.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show_main(app)))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(update::Updates::default())
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
            detect_app,
            fix_permissions,
            apply_preset,
            check_update,
            install_update,
        ])
        .setup(move |app| {
            let config_path = Config::default_path()?;
            let config = Arc::new(SharedConfig::new(load_config(&config_path)));
            let live = Arc::new(AtomicBool::new(false));

            // A native pill on Windows: no WebView kept alive just to show levels.
            let native_overlay = pady_overlay::Overlay::spawn();
            let web_overlay_needed = native_overlay.is_none();

            let (frames_tx, frames_rx) = crossbeam_channel::unbounded();
            let input = pady_input::start(frames_tx.clone()).map_err(|e| e.to_string());
            if let Err(e) = &input {
                log::warn!("touchpad unavailable: {e}");
            }
            let engine = Engine::spawn(
                config.clone(),
                frames_rx,
                pady_actions::executor,
                observer(app.handle().clone(), live.clone(), config.clone(), native_overlay),
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

            // Windows are created only now (`create: false` in tauri.conf.json):
            // a fast page load would otherwise call get_state before `manage` ran.
            // The settings window waits until it's opened (see show_main).
            let wanted = |label: &str| label != "main" && (label != "overlay" || web_overlay_needed);
            for window in app.config().app.windows.iter().filter(|w| wanted(&w.label)) {
                tauri::WebviewWindowBuilder::from_config(app.handle(), window)?.build()?;
            }

            let toggle = build_tray(app.handle())?;
            *lock(&app.state::<Pady>().tray_toggle) = Some(toggle);
            app.state::<Pady>().sync_tray();
            spawn_watcher(app.handle().clone());
            update::spawn_checks(app.handle().clone());
            enable_autostart_once(app.handle());

            if !start_hidden {
                show_main(app.handle());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the settings window destroys it (freeing its WebView);
            // the engine and the tray icon keep running.
            if let (WindowEvent::Destroyed, "main") = (event, window.label()) {
                window.state::<Pady>().live.store(false, Ordering::Relaxed);
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Pady")
        .run(|app, event| match event {
            // The last window closing is not "Quit": keep gestures working from the tray.
            RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
            RunEvent::Exit => {
                // Quitting mid-drag must not leave the mouse button pressed system-wide.
                if let Some(pady) = app.try_state::<Pady>() {
                    pady.engine.release_mouse(Duration::from_millis(300));
                }
                log::info!("Pady exiting");
            }
            _ => {}
        });
}

#[cfg(test)]
mod tests {
    /// The names between `open` and the next `close` in `text`, split on commas.
    fn names<'a>(text: &'a str, open: &str, close: char) -> Vec<&'a str> {
        let start = text.find(open).expect("list start") + open.len();
        let end = start + text[start..].find(close).expect("list end");
        text[start..end].split(',').map(|s| s.trim().trim_matches('"')).filter(|s| !s.is_empty()).collect()
    }

    /// A command missing from build.rs or the main window's capability fails
    /// at runtime ("not allowed"), so the three lists must agree.
    #[test]
    fn every_command_is_permitted_for_the_main_window_only() {
        let handlers = names(include_str!("main.rs"), "generate_handler![", ']');
        let declared = names(include_str!("../build.rs"), "const COMMANDS: &[&str] = &[", ']');
        let main_window = include_str!("../capabilities/default.json");
        let overlay = include_str!("../capabilities/overlay.json");
        assert!(!handlers.is_empty());
        assert_eq!(handlers, declared, "build.rs COMMANDS must match generate_handler!");
        for command in handlers {
            let permission = format!("\"allow-{}\"", command.replace('_', "-"));
            assert!(main_window.contains(&permission), "{permission} missing from capabilities/default.json");
            assert!(!overlay.contains(&permission), "the overlay must not get {permission}");
        }
    }
}
