//! The quick pop-up by the cursor, for two gestures: the clipboard history
//! (the last things you copied, to paste one) and a quick note (jot
//! something down; selected text goes in it).
//!
//! Like the translation pop-up (see popup.rs) the window (`quick.html`) is
//! made on first use, shown once it has told the backend its size, and
//! destroyed a while after it closes.
//!
//! The history lives in memory only and is never written to disk. It's kept
//! only while switched on, and skips whatever the copying app marks as
//! private (password managers do), like Windows' own history.

use crate::notes::{self, wall_ms, Notes};
use crate::popup::{cursor, focus, foreground, place};
use crate::{lock, Pady};
use pady_overlay::Icon;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const LABEL: &str = "quick";
/// A closed pop-up's WebView is kept this long for the next one.
const KEEP: Duration = Duration::from_secs(90);
const MAX_CLIPS: usize = 25;
/// Longer copies are kept cut to this many characters.
const MAX_CLIP_CHARS: usize = 20_000;

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

// ---------------------------------------------------------------------------
// The clipboard history

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Clip {
    text: String,
    /// When it was copied, in milliseconds since 1970.
    at: u64,
}

#[derive(Default)]
pub struct Clips {
    list: Mutex<VecDeque<Clip>>,
}

impl Clips {
    /// Newest first; copying something again moves it to the top.
    pub fn push(&self, text: String) {
        let text = match text.char_indices().nth(MAX_CLIP_CHARS) {
            Some((end, _)) => text[..end].to_string(),
            None => text,
        };
        let mut list = lock(&self.list);
        list.retain(|c| c.text != text);
        list.push_front(Clip { text, at: wall_ms() });
        list.truncate(MAX_CLIPS);
    }

    pub fn clear(&self) {
        lock(&self.list).clear();
    }

    fn list(&self) -> Vec<Clip> {
        lock(&self.list).iter().cloned().collect()
    }

    fn get(&self, index: usize) -> Option<String> {
        lock(&self.list).get(index).map(|c| c.text.clone())
    }
}

/// Something new was copied (called on the notices' thread, so the
/// clipboard is read on a thread of its own).
pub fn copied(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        // The copying app may still be filling in its formats.
        std::thread::sleep(Duration::from_millis(60));
        match pady_actions::copied_text() {
            Ok(Some(text)) => app.state::<Clips>().push(text),
            Ok(None) => {}
            Err(e) => log::debug!("couldn't read the copied text: {e}"),
        }
    });
}

// ---------------------------------------------------------------------------
// The pop-up

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum View {
    Clips { clips: Vec<Clip>, on: bool, supported: bool },
    Note { text: String, source: Option<String> },
}

#[derive(Clone, Debug, Serialize)]
pub struct Payload {
    id: u64,
    #[serde(flatten)]
    view: View,
}

struct Job {
    id: u64,
    view: View,
    /// The window in front when the gesture fired, to paste back into.
    target: isize,
    anchor: (f64, f64),
}

#[derive(Default)]
pub struct Quick {
    job: Mutex<Option<Job>>,
    /// Reading the selection for a note; a second gesture meanwhile is ignored.
    capturing: AtomicBool,
    shown: AtomicBool,
    moved: AtomicBool,
    closings: AtomicU64,
    next_id: AtomicU64,
}

impl Quick {
    fn payload(&self) -> Option<Payload> {
        lock(&self.job).as_ref().map(|j| Payload { id: j.id, view: j.view.clone() })
    }

    fn open(&self, app: &AppHandle, view: View, target: isize, anchor: (f64, f64)) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.job) = Some(Job { id, view, target, anchor });
        let Some(payload) = self.payload() else { return };
        if app.get_webview_window(LABEL).is_some() {
            let _ = app.emit_to(LABEL, "pady://quick", payload);
        } else {
            let Some(config) = app.config().app.windows.iter().find(|w| w.label == LABEL).cloned() else { return };
            if let Err(e) = tauri::WebviewWindowBuilder::from_config(app, &config).and_then(|b| b.build()) {
                log::error!("could not open the quick pop-up: {e}");
            }
        }
    }

    fn close(&self, app: &AppHandle) {
        self.shown.store(false, Ordering::SeqCst);
        self.moved.store(false, Ordering::SeqCst);
        if let Some(window) = app.get_webview_window(LABEL) {
            let _ = window.hide();
            let _ = app.emit_to(LABEL, "pady://quick-hidden", ());
        }
        let ticket = self.closings.fetch_add(1, Ordering::SeqCst) + 1;
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(KEEP);
            let quick = app.state::<Quick>();
            if quick.closings.load(Ordering::SeqCst) == ticket && !quick.shown.load(Ordering::SeqCst) {
                if let Some(window) = app.get_webview_window(LABEL) {
                    let _ = window.destroy();
                }
            }
        });
    }

    fn mode_is_clips(&self) -> bool {
        matches!(lock(&self.job).as_ref().map(|j| &j.view), Some(View::Clips { .. }))
    }
}

/// The Clipboard history gesture: show the list, or close it if it's open.
pub fn clipboard(app: AppHandle) {
    let quick = app.state::<Quick>();
    if quick.shown.load(Ordering::SeqCst) && quick.mode_is_clips() {
        let target = lock(&quick.job).as_ref().map_or(0, |j| j.target);
        quick.close(&app);
        focus(target);
        return;
    }
    let view = View::Clips {
        clips: app.state::<Clips>().list(),
        on: app.state::<Pady>().config.get().settings.clipboard_history,
        supported: cfg!(windows),
    };
    quick.open(&app, view, foreground(), cursor(&app));
}

/// The Quick note gesture: a note by the cursor, with the selection in it.
pub fn note(app: AppHandle) {
    let quick = app.state::<Quick>();
    if quick.capturing.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        let quick = app.state::<Quick>();
        // An open pop-up has the focus: give it back first, or Ctrl+C
        // would copy from the pop-up.
        if quick.shown.load(Ordering::SeqCst) {
            let target = lock(&quick.job).as_ref().map_or(0, |j| j.target);
            quick.close(&app);
            focus(target);
            std::thread::sleep(Duration::from_millis(150));
        }
        let target = foreground();
        let anchor = cursor(&app);
        let source = pady_actions::foreground_app();
        pady_overlay::quiet_clipboard(Duration::from_millis(2_000));
        let text = pady_actions::selected_text().ok().flatten().map(|t| t.trim().to_string()).unwrap_or_default();
        quick.capturing.store(false, Ordering::SeqCst);
        quick.open(&app, View::Note { text, source }, target, anchor);
    });
}

// ---------------------------------------------------------------------------
// Commands (the pop-up window only)

#[tauri::command]
pub fn quick_ready(quick: tauri::State<Quick>) -> Option<Payload> {
    quick.payload()
}

#[tauri::command]
pub fn quick_layout(app: AppHandle, quick: tauri::State<Quick>, width: f64, height: f64) {
    let Some(window) = app.get_webview_window(LABEL) else { return };
    let Some(anchor) = lock(&quick.job).as_ref().map(|j| j.anchor) else { return };
    place(&app, &window, anchor, quick.moved.load(Ordering::SeqCst), width, height);
    if !quick.shown.swap(true, Ordering::SeqCst) {
        quick.closings.fetch_add(1, Ordering::SeqCst); // cancel a pending destroy
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// What the pop-up asks for: `close`, `drag`; for the clipboard `paste` or
/// `copy` (item `index`), `clear` and `enable`; for a note `save` (with
/// `text`), and `notes` (open the Notes tab).
#[tauri::command]
pub fn quick_act(
    app: AppHandle,
    quick: tauri::State<Quick>,
    id: u64,
    kind: String,
    index: Option<usize>,
    text: Option<String>,
) -> Result<(), String> {
    let (current, target, source) = {
        let job = lock(&quick.job);
        let Some(job) = job.as_ref() else { return Ok(()) };
        let source = match &job.view {
            View::Note { source, .. } => source.clone(),
            View::Clips { .. } => None,
        };
        (job.id, job.target, source)
    };
    if id != current {
        return Ok(()); // a click on a pop-up that has moved on
    }
    let clip = || index.and_then(|i| app.state::<Clips>().get(i)).ok_or("that item is gone");
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
        "paste" => {
            let text = clip()?;
            quick.close(&app);
            app.state::<Clips>().push(text.clone());
            std::thread::spawn(move || paste(&app, target, &text));
        }
        "copy" => {
            let text = clip()?;
            pady_actions::copy_text(&text).map_err(|e| e.to_string())?;
            quick.close(&app);
            focus(target);
        }
        "clear" => {
            app.state::<Clips>().clear();
            refresh_clips(&app, &quick);
        }
        "enable" => {
            let pady = app.state::<Pady>();
            let mut config = pady.config.get();
            config.settings.clipboard_history = true;
            pady.apply(&app, config)?;
            refresh_clips(&app, &quick);
        }
        "save" => {
            let text = text.unwrap_or_default();
            quick.close(&app);
            focus(target);
            if !text.trim().is_empty() {
                app.state::<Notes>().save(None, &text, source)?;
                notes::changed(&app);
                notice(&app, "Saved to Notes".into(), Icon::Note);
            }
        }
        "notes" => {
            let text = text.unwrap_or_default();
            quick.close(&app);
            if !text.trim().is_empty() {
                app.state::<Notes>().save(None, &text, source)?;
            }
            crate::show_page(&app, "notes");
        }
        other => return Err(format!("unknown action `{other}`")),
    }
    Ok(())
}

fn refresh_clips(app: &AppHandle, quick: &Quick) {
    {
        let mut job = lock(&quick.job);
        let Some(job) = job.as_mut() else { return };
        job.view = View::Clips {
            clips: app.state::<Clips>().list(),
            on: app.state::<Pady>().config.get().settings.clipboard_history,
            supported: cfg!(windows),
        };
    }
    if let Some(payload) = quick.payload() {
        let _ = app.emit_to(LABEL, "pady://quick", payload);
    }
}

/// Paste `text` into the window the gesture was made in, then put the
/// clipboard back as it was.
fn paste(app: &AppHandle, target: isize, text: &str) {
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
        notice(app, format!("Couldn't paste: {e}"), Icon::Clipboard);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_the_newest_once_and_caps_its_length() {
        let clips = Clips::default();
        for i in 0..30 {
            clips.push(format!("item {i}"));
        }
        clips.push("item 20".into());
        let list = clips.list();
        assert_eq!(list.len(), MAX_CLIPS);
        assert_eq!(list[0].text, "item 20");
        assert_eq!(list[1].text, "item 29");
        assert_eq!(list.iter().filter(|c| c.text == "item 20").count(), 1);
        clips.push("x".repeat(MAX_CLIP_CHARS + 10));
        assert_eq!(clips.get(0).unwrap().chars().count(), MAX_CLIP_CHARS);
        clips.clear();
        assert!(clips.list().is_empty());
    }
}
