//! The translation pop-up: a small window by the cursor with the selected
//! text translated, and Copy / Replace buttons.
//!
//! The window (`translate.html`) is created on first use and destroyed a
//! while after it closes, so a Pady sitting in the tray holds no WebView.
//! It tells the backend its size after every render; only then is it moved
//! next to the cursor and shown, so it never flashes empty or in the wrong
//! place.

use crate::translate::{self, Translation};
use crate::{lock, Pady};
use pady_core::languages;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize};

const LABEL: &str = "translate";
/// A closed pop-up's WebView is kept this long for the next translation.
const KEEP: Duration = Duration::from_secs(90);

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum View {
    Loading { from_name: Option<String>, to_name: String },
    Done { translation: Translation, rtl: bool },
    Failed { message: String, retry: bool },
}

#[derive(Clone, Debug, Serialize)]
pub struct Payload {
    id: u64,
    original: String,
    #[serde(flatten)]
    view: View,
}

struct Job {
    id: u64,
    original: String,
    view: View,
    /// The window the text was selected in (Windows `HWND`), to paste back into.
    target: isize,
    /// The cursor when the gesture fired, in physical pixels.
    anchor: (f64, f64),
}

#[derive(Default)]
pub struct Popup {
    job: Mutex<Option<Job>>,
    /// Reading the selection; a second gesture meanwhile is ignored.
    capturing: AtomicBool,
    /// Whether the window is on screen.
    shown: AtomicBool,
    /// The user dragged it: it stays where they put it until it closes.
    moved: AtomicBool,
    /// Bumped whenever the pop-up closes, so only the newest destroy timer acts.
    closings: AtomicU64,
    next_id: AtomicU64,
}

impl Popup {
    fn payload(&self) -> Option<Payload> {
        lock(&self.job).as_ref().map(|j| Payload { id: j.id, original: j.original.clone(), view: j.view.clone() })
    }

    fn send(&self, app: &AppHandle) {
        let Some(payload) = self.payload() else { return };
        if app.get_webview_window(LABEL).is_some() {
            let _ = app.emit_to(LABEL, "pady://translate", payload);
        } else {
            create_window(app); // it asks for the payload once loaded
        }
    }

    fn close(&self, app: &AppHandle) {
        self.shown.store(false, Ordering::SeqCst);
        self.moved.store(false, Ordering::SeqCst);
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.hide();
            // The page plays its entrance again next time it's shown.
            let _ = app.emit_to(LABEL, "pady://translate-hidden", ());
        }
        let ticket = self.closings.fetch_add(1, Ordering::SeqCst) + 1;
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(KEEP);
            let popup = app.state::<Popup>();
            if popup.closings.load(Ordering::SeqCst) == ticket && !popup.shown.load(Ordering::SeqCst) {
                if let Some(window) = app.get_webview_window(LABEL) {
                    let _ = window.destroy();
                }
            }
        });
    }
}

fn create_window(app: &AppHandle) {
    let Some(config) = app.config().app.windows.iter().find(|w| w.label == LABEL).cloned() else { return };
    if let Err(e) = tauri::WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build()) {
        log::error!("could not open the translation pop-up: {e}");
    }
}

/// A short corner notice where there is one (Windows); otherwise the
/// pop-up itself says it.
fn notify(app: &AppHandle, text: String) {
    if let Some(toasts) = &app.state::<Pady>().toasts {
        toasts.show(pady_overlay::Notice::Text { text, icon: pady_overlay::Icon::Touch });
        return;
    }
    let popup = app.state::<Popup>();
    let id = popup.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    let anchor = cursor(app);
    *lock(&popup.job) = Some(Job {
        id,
        original: String::new(),
        view: View::Failed { message: text, retry: false },
        target: 0,
        anchor,
    });
    popup.send(app);
}

fn cursor(app: &AppHandle) -> (f64, f64) {
    app.cursor_position().map(|p| (p.x, p.y)).unwrap_or((200.0, 200.0))
}

/// The Translate gesture fired: read the selection and translate it.
pub fn start(app: AppHandle) {
    let popup = app.state::<Popup>();
    if popup.capturing.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let popup = app.state::<Popup>();
        // An open pop-up has the focus: give it back to the app first, or
        // Ctrl+C would copy from the pop-up itself.
        if popup.shown.load(Ordering::SeqCst) {
            let target = lock(&popup.job).as_ref().map_or(0, |j| j.target);
            popup.close(&app);
            focus(target);
            std::thread::sleep(Duration::from_millis(150));
        }
        let target = foreground();
        let anchor = cursor(&app);
        pady_overlay::quiet_clipboard(Duration::from_millis(2_000));
        let selected = pady_actions::selected_text();
        popup.capturing.store(false, Ordering::SeqCst);
        match selected {
            Ok(Some(text)) => begin(&app, text, target, anchor),
            Ok(None) => notify(&app, "Select some text to translate".into()),
            Err(e) => notify(&app, format!("Couldn't read the selected text: {e}")),
        }
    });
}

fn begin(app: &AppHandle, text: String, target: isize, anchor: (f64, f64)) {
    let popup = app.state::<Popup>();
    let settings = app.state::<Pady>().config.get().settings;
    let Some(to) = languages::by_code(&settings.translate_to).or_else(|| languages::by_code("en")) else { return };
    let text = text.trim().to_string();
    // The free service detects here, so the pop-up can name the language at once.
    let from_name =
        (settings.translate_engine == "free").then(|| translate::detect(&text)).flatten().map(languages::name);
    let id = popup.next_id.fetch_add(1, Ordering::SeqCst) + 1;
    *lock(&popup.job) = Some(Job {
        id,
        original: text.clone(),
        view: View::Loading { from_name, to_name: to.name.into() },
        target,
        anchor,
    });
    popup.send(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = translate::translate(&text, to, &settings).await;
        let view = match result {
            Ok(translation) => View::Done { rtl: languages::is_rtl(&translation.to), translation },
            Err(problem) => {
                log::warn!("translation failed: {problem:?}");
                View::Failed { retry: problem.retryable(), message: problem.message() }
            }
        };
        let popup = app.state::<Popup>();
        {
            let mut job = lock(&popup.job);
            match job.as_mut() {
                Some(job) if job.id == id => job.view = view,
                _ => return, // a newer translation took over
            }
        }
        popup.send(&app);
    });
}

// ---------------------------------------------------------------------------
// Commands (the pop-up window only)

/// The pop-up loaded: what to show.
#[tauri::command]
pub fn translate_ready(popup: tauri::State<Popup>) -> Option<Payload> {
    popup.payload()
}

/// The pop-up rendered at this size (CSS pixels): place it by the cursor
/// and show it.
#[tauri::command]
pub fn translate_layout(app: AppHandle, popup: tauri::State<Popup>, width: f64, height: f64) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    let Some(mut anchor) = lock(&popup.job).as_ref().map(|j| j.anchor) else { return };
    // Dragged somewhere: keep its top-left there and only fit it on screen.
    let kept = popup.moved.load(Ordering::SeqCst).then(|| window.outer_position().ok()).flatten();
    if let Some(at) = kept {
        anchor = (at.x as f64, at.y as f64);
    }
    let monitor =
        app.monitor_from_point(anchor.0, anchor.1).ok().flatten().or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    let scale = monitor.scale_factor();
    let (w, h) = ((width.clamp(120.0, 900.0) * scale).round(), (height.clamp(40.0, 900.0) * scale).round());
    let area = monitor.work_area();
    let (left, top) = (area.position.x as f64, area.position.y as f64);
    let (right, bottom) = (left + area.size.width as f64, top + area.size.height as f64);
    let gap = 14.0 * scale;
    // Below and to the right of the cursor; above it if there's no room below.
    let (mut x, mut y) = match kept {
        Some(_) => anchor,
        None => (anchor.0 - 18.0 * scale, anchor.1 + gap),
    };
    if kept.is_none() && y + h > bottom - 8.0 {
        y = anchor.1 - gap - h;
    }
    x = x.min(right - w - 8.0).max(left + 8.0);
    y = y.min(bottom - h - 8.0).max(top + 8.0);
    let _ = window.set_size(PhysicalSize::new(w as u32, h as u32));
    let _ = window.set_position(PhysicalPosition::new(x as i32, y as i32));
    if !popup.shown.swap(true, Ordering::SeqCst) {
        popup.closings.fetch_add(1, Ordering::SeqCst); // cancel a pending destroy
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// A button in the pop-up: `copy`, `replace`, `retry` or `close`; or `drag`,
/// pressed on the card to move it.
#[tauri::command]
pub fn translate_act(app: AppHandle, popup: tauri::State<Popup>, id: u64, kind: String) -> Result<(), String> {
    let (current, original, text, target) = {
        let job = lock(&popup.job);
        let Some(job) = job.as_ref() else { return Ok(()) };
        let text = match &job.view {
            View::Done { translation, .. } => Some(translation.text.clone()),
            _ => None,
        };
        (job.id, job.original.clone(), text, job.target)
    };
    if id != current {
        return Ok(()); // a click on a pop-up that has moved on
    }
    match kind.as_str() {
        "close" => popup.close(&app),
        "drag" => {
            popup.moved.store(true, Ordering::SeqCst);
            if let Some(window) = app.get_webview_window(LABEL) {
                window.start_dragging().map_err(|e| e.to_string())?;
            }
        }
        "copy" => {
            let text = text.ok_or("nothing to copy yet")?;
            pady_actions::copy_text(&text).map_err(|e| e.to_string())?;
            popup.close(&app);
        }
        "replace" => {
            let text = text.ok_or("nothing to paste yet")?;
            popup.close(&app);
            std::thread::spawn(move || replace(&app, target, &text));
        }
        "retry" => {
            let anchor = lock(&popup.job).as_ref().map_or((200.0, 200.0), |j| j.anchor);
            begin(&app, original, target, anchor);
        }
        other => return Err(format!("unknown action `{other}`")),
    }
    Ok(())
}

/// Paste the translation over the original selection.
fn replace(app: &AppHandle, target: isize, text: &str) {
    focus(target);
    std::thread::sleep(Duration::from_millis(150));
    pady_overlay::quiet_clipboard(Duration::from_millis(2_500));
    #[cfg(windows)]
    let result = pady_actions::replace_selection(text);
    #[cfg(not(windows))]
    let result = pady_actions::copy_text(text).map(|()| {
        app.state::<Pady>().engine.run_action(pady_core::Action::Keys { combo: "ctrl+v".into() });
    });
    if let Err(e) = result {
        notify(app, format!("Couldn't replace the text: {e}"));
    }
}

#[cfg(windows)]
fn foreground() -> isize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    unsafe { GetForegroundWindow() }.0 as isize
}

#[cfg(not(windows))]
fn foreground() -> isize {
    0
}

#[cfg(windows)]
fn focus(target: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SetForegroundWindow};
    let hwnd = HWND(target as *mut std::ffi::c_void);
    if target != 0 && unsafe { IsWindow(hwnd) }.as_bool() {
        let _ = unsafe { SetForegroundWindow(hwnd) };
    }
}

#[cfg(not(windows))]
fn focus(_target: isize) {}
