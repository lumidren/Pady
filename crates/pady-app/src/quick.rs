//! The quick note: a small note by the cursor to jot something down.
//! Saved notes go to the Notes page.
//!
//! It has to feel instant, so nothing waits on anything else:
//!
//! - While a Quick note gesture is bound, the window (`quick.html`) is kept
//!   ready and hidden, so the gesture only has to show it. With no such
//!   gesture there is no window at all (see [`sync`]).
//! - The gesture shows the note at once, at the size it had last time.
//!
//! With `note_selection` on, the note starts with the selected text. Only
//! then does Pady touch the clipboard: it presses Ctrl+C, reads the text
//! and puts the clipboard back. The note is on screen meanwhile, without
//! the focus (Ctrl+C must reach the app in front); the text and the focus
//! arrive a moment later.
//!
//! The same gesture again saves the note and closes it.

use crate::notes::{self, Notes};
use crate::{lock, Pady};
use pady_core::Action;
use pady_overlay::Icon;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

const LABEL: &str = "quick";
/// How long the app in front gets to answer Ctrl+C with its selection.
const COPY_WAIT: Duration = Duration::from_millis(400);
/// With no Quick note gesture bound (the note was opened by "Test"), a
/// closed note's WebView is kept this long for the next one.
const KEEP: Duration = Duration::from_secs(90);
/// The note's size (CSS pixels) until the page has reported its own.
const FIRST_SIZE: (f64, f64) = (380.0, 236.0);

/// A notice in the corner where there is one (Windows); a desktop
/// notification elsewhere.
pub fn notice(app: &AppHandle, text: String, icon: Icon) {
    match &app.state::<Pady>().toasts {
        Some(toasts) => toasts.show(pady_overlay::Notice::Text { text, icon }),
        #[cfg(target_os = "linux")]
        None => {
            let _ = std::process::Command::new("notify-send").args(["--app-name=Pady", "Pady", &text]).spawn();
        }
        #[cfg(not(target_os = "linux"))]
        None => {}
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Payload {
    id: u64,
    /// The text that was selected when the gesture fired.
    text: String,
    /// The app the note is taken in (`chrome.exe`).
    source: Option<String>,
    /// The selection is still being read: `text` comes in a moment.
    pending: bool,
}

struct Job {
    id: u64,
    text: String,
    source: Option<String>,
    pending: bool,
    /// The window in front when the gesture fired, to give the focus back to.
    target: isize,
    /// The cursor when the gesture fired, in physical pixels.
    anchor: (f64, f64),
}

#[derive(Default)]
pub struct Quick {
    job: Mutex<Option<Job>>,
    /// Reading the selection; a second gesture meanwhile is ignored.
    capturing: AtomicBool,
    /// Whether the note is on screen.
    shown: AtomicBool,
    /// The user dragged it: it stays where they put it until it closes.
    moved: AtomicBool,
    /// The page has loaded and listens: the window can be shown at once.
    ready: AtomicBool,
    /// Bumped whenever the note closes, so only the newest destroy timer acts.
    closings: AtomicU64,
    next_id: AtomicU64,
    /// The size the page reported last (CSS pixels).
    size: Mutex<Option<(f64, f64)>>,
}

impl Quick {
    fn payload(&self) -> Option<Payload> {
        lock(&self.job).as_ref().map(|j| Payload {
            id: j.id,
            text: j.text.clone(),
            source: j.source.clone(),
            pending: j.pending,
        })
    }

    /// Tell the page what to show; make the window first if there is none
    /// (the page then asks for it once loaded).
    fn send(&self, app: &AppHandle) {
        let Some(payload) = self.payload() else { return };
        if app.get_webview_window(LABEL).is_none() {
            create_window(app);
        } else if self.ready.load(Ordering::SeqCst) {
            let _ = app.emit_to(LABEL, "pady://quick", payload);
        }
    }

    /// Put the note on screen if it isn't (and its page is ready), and give
    /// it the keyboard if `focus`.
    fn present(&self, app: &AppHandle, focus: bool) {
        let Some(window) = app.get_webview_window(LABEL) else { return };
        if !self.ready.load(Ordering::SeqCst) {
            return; // still loading: quick_layout shows it
        }
        let Some(anchor) = lock(&self.job).as_ref().map(|j| j.anchor) else { return };
        if !self.shown.swap(true, Ordering::SeqCst) {
            self.closings.fetch_add(1, Ordering::SeqCst); // cancel a pending destroy
            let (width, height) = lock(&self.size).unwrap_or(FIRST_SIZE);
            place(app, &window, anchor, false, width, height);
            show(&window, focus);
        } else if focus {
            give_focus(&window);
        }
    }

    fn close(&self, app: &AppHandle) {
        self.shown.store(false, Ordering::SeqCst);
        self.moved.store(false, Ordering::SeqCst);
        // A late size report from the page must not bring the note back.
        *lock(&self.job) = None;
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.hide();
            // The page empties itself, ready for the next note.
            let _ = app.emit_to(LABEL, "pady://quick-hidden", ());
        }
        if bound(app) {
            return; // kept ready for the next gesture
        }
        let ticket = self.closings.fetch_add(1, Ordering::SeqCst) + 1;
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(KEEP);
            let quick = app.state::<Quick>();
            if quick.closings.load(Ordering::SeqCst) == ticket && !quick.shown.load(Ordering::SeqCst) && !bound(&app) {
                quick.destroy(&app);
            }
        });
    }

    fn destroy(&self, app: &AppHandle) {
        self.ready.store(false, Ordering::SeqCst);
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.destroy();
        }
    }
}

/// The window is gone: the next note has to make it again.
pub fn gone(app: &AppHandle) {
    let quick = app.state::<Quick>();
    quick.ready.store(false, Ordering::SeqCst);
    quick.shown.store(false, Ordering::SeqCst);
}

fn create_window(app: &AppHandle) {
    let Some(config) = app.config().app.windows.iter().find(|w| w.label == LABEL).cloned() else { return };
    if let Err(e) = tauri::WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build()) {
        log::error!("could not open the quick note: {e}");
    }
}

/// Is a Quick note gesture set, in any profile?
fn bound(app: &AppHandle) -> bool {
    let Some(pady) = app.try_state::<Pady>() else { return false };
    let config = pady.config.get();
    let profiles = config.profiles.iter().flat_map(|p| p.bindings.iter());
    config.bindings.iter().chain(profiles).any(|b| b.action == Action::QuickNote)
}

/// Keep the note's window ready while a Quick note gesture is set, and
/// only then: without one, Pady in the tray holds no WebView. Called at
/// start-up and whenever the config changes.
pub fn sync(app: &AppHandle) {
    let app = app.clone();
    // On a thread of its own: a window can't be made from inside a command.
    std::thread::spawn(move || {
        let quick = app.state::<Quick>();
        let exists = app.get_webview_window(LABEL).is_some();
        match (bound(&app), exists) {
            (true, false) => create_window(&app),
            (false, true) if !quick.shown.load(Ordering::SeqCst) => quick.destroy(&app),
            _ => {}
        }
    });
}

/// Apps where Ctrl+C doesn't copy: with nothing selected a terminal takes
/// it as "stop the running program". Notes taken there start empty.
fn is_terminal(app: &str) -> bool {
    const TERMINALS: &[&str] = &[
        "windowsterminal",
        "wt",
        "cmd",
        "powershell",
        "pwsh",
        "conhost",
        "openconsole",
        "alacritty",
        "wezterm-gui",
        "wezterm",
        "mintty",
        "conemu",
        "conemu64",
        "hyper",
        "tabby",
        "putty",
        "kitty",
    ];
    let name = app.rsplit(['/', '\\']).next().unwrap_or(app).to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    TERMINALS.contains(&name)
}

/// The Quick note gesture: a note by the cursor, with the selection in it.
/// With a note already open, it saves and closes that one.
pub fn note(app: AppHandle) {
    let quick = app.state::<Quick>();
    if quick.shown.load(Ordering::SeqCst) {
        let _ = app.emit_to(LABEL, "pady://quick-dismiss", ());
        return;
    }
    if quick.capturing.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let quick = app.state::<Quick>();
        let target = foreground();
        let source = pady_actions::foreground_app();
        let id = quick.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let wanted = app.state::<Pady>().config.get().settings.note_selection;
        let capture = wanted && !source.as_deref().is_some_and(is_terminal);
        *lock(&quick.job) = Some(Job {
            id,
            text: String::new(),
            source: source.clone(),
            pending: capture,
            target,
            anchor: cursor(&app),
        });
        if !capture {
            // Nothing to wait for: on screen and ready to type in.
            quick.capturing.store(false, Ordering::SeqCst);
            quick.send(&app);
            quick.present(&app, true);
            return;
        }
        // On screen first; the selection follows.
        quick.send(&app);
        quick.present(&app, false);
        pady_overlay::quiet_clipboard(Duration::from_millis(2_000));
        let text =
            pady_actions::selected_text(COPY_WAIT).ok().flatten().map(|t| t.trim().to_string()).unwrap_or_default();
        if let Some(job) = lock(&quick.job).as_mut().filter(|j| j.id == id) {
            job.text = text;
            job.pending = false;
        }
        quick.capturing.store(false, Ordering::SeqCst);
        quick.send(&app);
        quick.present(&app, true);
    });
}

// ---------------------------------------------------------------------------
// Commands (the note's window only)

/// The page loaded: what to show, if a note is waiting.
#[tauri::command]
pub fn quick_ready(quick: tauri::State<Quick>) -> Option<Payload> {
    quick.ready.store(true, Ordering::SeqCst);
    quick.payload()
}

/// The page rendered at this size (CSS pixels): size the window to it, and
/// show it if it isn't on screen yet (the first note after the window was
/// made).
#[tauri::command]
pub fn quick_layout(app: AppHandle, quick: tauri::State<Quick>, id: u64, width: f64, height: f64) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    *lock(&quick.size) = Some((width, height));
    let Some(anchor) = lock(&quick.job).as_ref().filter(|j| j.id == id).map(|j| j.anchor) else { return };
    let was_shown = quick.shown.swap(true, Ordering::SeqCst);
    place(&app, &window, anchor, was_shown && quick.moved.load(Ordering::SeqCst), width, height);
    if !was_shown {
        quick.closings.fetch_add(1, Ordering::SeqCst); // cancel a pending destroy
        show(&window, !quick.capturing.load(Ordering::SeqCst));
    }
}

/// What the note asks for: `close`, `drag`, `save` (with `text`) and
/// `notes` (save, then open the Notes page).
#[tauri::command]
pub fn quick_act(
    app: AppHandle,
    quick: tauri::State<Quick>,
    id: u64,
    kind: String,
    text: Option<String>,
) -> Result<(), String> {
    let (current, target, source) = {
        let job = lock(&quick.job);
        let Some(job) = job.as_ref() else { return Ok(()) };
        (job.id, job.target, job.source.clone())
    };
    if id != current {
        return Ok(()); // a click on a note that has moved on
    }
    match kind.as_str() {
        "close" => {
            quick.close(&app);
            focus(target);
        }
        "drag" => {
            quick.moved.store(true, Ordering::SeqCst);
            if let Some(window) = app.get_webview_window(LABEL) {
                window.start_dragging().map_err(|e| e.to_string())?;
            }
        }
        "save" => {
            let text = text.unwrap_or_default();
            quick.close(&app);
            focus(target);
            if !text.trim().is_empty() {
                app.state::<Notes>().save(None, None, &text, source)?;
                notes::changed(&app);
                notice(&app, "Saved to Notes".into(), Icon::Note);
            }
        }
        "notes" => {
            let text = text.unwrap_or_default();
            quick.close(&app);
            if !text.trim().is_empty() {
                app.state::<Notes>().save(None, None, &text, source)?;
                notes::changed(&app);
            }
            crate::show_page(&app, "notes");
        }
        other => return Err(format!("unknown action `{other}`")),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows and the cursor

fn cursor(app: &AppHandle) -> (f64, f64) {
    app.cursor_position().map(|p| (p.x, p.y)).unwrap_or((200.0, 200.0))
}

/// Size the note for its content (CSS pixels) and put it below and to the
/// right of `anchor` (the cursor), or above it if there's no room below.
/// `moved`: the user dragged it, so it stays where they put it and is only
/// kept on screen.
fn place(app: &AppHandle, window: &WebviewWindow, mut anchor: (f64, f64), moved: bool, width: f64, height: f64) {
    let kept = moved.then(|| window.outer_position().ok()).flatten();
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
}

/// Put the window on screen. Without `focus` it must not take the keyboard
/// from the app in front, whose selection is still being read (with Ctrl+C,
/// which has to reach that app): it is shown as a window that can't be
/// focused, until [`give_focus`].
fn show(window: &WebviewWindow, focus: bool) {
    let _ = window.set_focusable(focus);
    let _ = window.show();
    if focus {
        let _ = window.set_focus();
    }
}

fn give_focus(window: &WebviewWindow) {
    let _ = window.set_focusable(true);
    let _ = window.set_focus();
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

#[cfg(test)]
mod tests {
    use super::is_terminal;

    #[test]
    fn terminals_are_never_sent_ctrl_c() {
        for app in ["WindowsTerminal.exe", "cmd.exe", "pwsh.exe", "alacritty", "C:\\Windows\\System32\\cmd.exe"] {
            assert!(is_terminal(app), "{app}");
        }
        for app in ["chrome.exe", "Code.exe", "notepad.exe", "firefox", "cmder-notes.exe", ""] {
            assert!(!is_terminal(app), "{app}");
        }
    }
}
