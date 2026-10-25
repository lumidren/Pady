//! Native executors for Pady actions, plus a few OS integrations the app
//! needs (start-with-system, detecting gestures the OS also handles).

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

/// Something on this system that will fight with the user's bindings.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conflict {
    pub message: String,
    /// What the user can do about it.
    pub fix: String,
    /// Whether [`open_touchpad_settings`] leads somewhere useful for it.
    pub can_open_settings: bool,
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
