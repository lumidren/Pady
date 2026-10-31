//! Native executors for Pady actions, plus a few OS integrations the app
//! needs (start-with-system, detecting gestures the OS also handles).

#[cfg(windows)]
mod clipboard;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

use pady_core::Config;
pub use pady_core::{ActionError, Executor};
use serde::Serialize;
use std::path::Path;

/// The executor for this OS. Individual actions report their own errors, so
/// creating one never fails (e.g. a missing `/dev/uinput` only breaks keys).
pub fn executor() -> Box<dyn Executor> {
    #[cfg(windows)]
    return Box::new(windows::WindowsExecutor::new());
    #[cfg(target_os = "linux")]
    return Box::new(linux::LinuxExecutor::new());
    #[cfg(not(any(windows, target_os = "linux")))]
    return Box::new(pady_core::engine::LogExecutor);
}

/// A link is handed to a browser or the system opener as a command-line
/// argument, so one starting with `-` would be read as an option (Chromium's
/// `--gpu-launcher=` runs a program). No real link starts with one.
#[cfg(any(windows, target_os = "linux"))]
fn check_url(url: &str) -> Result<(), ActionError> {
    if url.trim_start().starts_with('-') {
        return Err(format!("`{url}` is not a link (links can't start with `-`)").into());
    }
    Ok(())
}

/// Let go of the mouse button if a drag holds it; for crash handlers. On
/// Linux the kernel releases it by itself when Pady's virtual device closes.
pub fn release_held_button() {
    #[cfg(windows)]
    windows::release_held_button();
}

/// Executable name of the app in front (e.g. `chrome.exe` or `firefox`), as
/// matched by per-app profiles. `None` where the OS won't say (e.g. native
/// Wayland without a helper).
pub fn foreground_app() -> Option<String> {
    #[cfg(windows)]
    return windows::foreground_app();
    #[cfg(target_os = "linux")]
    return linux::LinuxExecutor::new().foreground_app();
    #[cfg(not(any(windows, target_os = "linux")))]
    None
}

/// The text selected in the app in front, or `None` if nothing is.
///
/// Windows: copied with Ctrl+C, with the clipboard put back afterwards.
/// Linux: the primary selection (whatever is highlighted), read with
/// wl-paste, xclip or xsel. Takes up to about half a second.
pub fn selected_text() -> Result<Option<String>, ActionError> {
    #[cfg(windows)]
    return clipboard::selected_text(|| windows::press("ctrl+c"));
    #[cfg(target_os = "linux")]
    return linux::primary_selection();
    #[cfg(not(any(windows, target_os = "linux")))]
    Ok(None)
}

/// Put text on the clipboard, as if the user had copied it.
pub fn copy_text(text: &str) -> Result<(), ActionError> {
    #[cfg(windows)]
    return clipboard::set_text(text, true);
    #[cfg(target_os = "linux")]
    return linux::set_clipboard(text);
    #[cfg(not(any(windows, target_os = "linux")))]
    Err(format!("can't copy {} characters here", text.len()).into())
}

/// Replace the selection in the app in front with `text`, by pasting it.
/// The clipboard is put back afterwards. Windows only: on Linux, copy the
/// text and press Ctrl+V through the executor (it owns the keyboard).
#[cfg(windows)]
pub fn replace_selection(text: &str) -> Result<(), ActionError> {
    clipboard::paste_text(text, || windows::press("ctrl+v"))
}

/// Something on this system that will fight with the user's bindings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conflict {
    pub message: String,
    /// What the user can do about it.
    pub fix: String,
    /// Whether [`open_touchpad_settings`] leads somewhere useful for it.
    pub can_open_settings: bool,
    /// Whether [`fix_permissions`] solves it.
    pub can_fix: bool,
}

/// OS gesture handling or permission problems that clash with `config`.
pub fn conflicts(config: &Config) -> Vec<Conflict> {
    #[cfg(windows)]
    return windows::conflicts(config);
    #[cfg(target_os = "linux")]
    return linux::conflicts(config);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = config;
        Vec::new()
    }
}

/// Grant what only an administrator can: on Linux, install the udev rule
/// that lets the signed-in user read touchpads and use `/dev/uinput` (one
/// polkit password prompt; the .deb and .rpm packages already include it).
/// Windows needs nothing extra.
pub fn fix_permissions() -> Result<(), ActionError> {
    #[cfg(target_os = "linux")]
    return linux::fix_permissions();
    #[cfg(not(target_os = "linux"))]
    Err("Pady needs no extra permissions on this OS".into())
}

/// Open the OS touchpad settings page.
pub fn open_touchpad_settings() -> Result<(), ActionError> {
    #[cfg(windows)]
    return windows::open_touchpad_settings();
    #[cfg(target_os = "linux")]
    return linux::open_touchpad_settings();
    #[cfg(not(any(windows, target_os = "linux")))]
    Err("not supported on this OS".into())
}

/// Is Pady registered to start when the user logs in?
pub fn autostart_enabled() -> bool {
    #[cfg(windows)]
    return windows::autostart_enabled();
    #[cfg(target_os = "linux")]
    return linux::autostart_enabled();
    #[cfg(not(any(windows, target_os = "linux")))]
    false
}

/// Register or unregister `exe` (started with `--hidden`) to run at login.
pub fn set_autostart(enabled: bool, exe: &Path) -> Result<(), ActionError> {
    #[cfg(windows)]
    return windows::set_autostart(enabled, exe);
    #[cfg(target_os = "linux")]
    return linux::set_autostart(enabled, exe);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (enabled, exe);
        Err("not supported on this OS".into())
    }
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    #[test]
    fn links_cannot_smuggle_in_options() {
        for bad in ["--gpu-launcher=calc.exe", " -profile /tmp/x", "-", "\t--help"] {
            assert!(super::check_url(bad).is_err(), "{bad:?}");
        }
        for good in ["https://example.com/a-b", "mailto:me@example.com", "ms-settings:display", "spotify:track:1"] {
            assert!(super::check_url(good).is_ok(), "{good:?}");
        }
    }
}
