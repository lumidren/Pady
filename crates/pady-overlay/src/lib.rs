//! Native volume/brightness overlay for Pady.
//!
//! The level pill used to be a second WebView2 window, which kept ~7 WebView2
//! processes (~500 MB) alive even while Pady sat in the tray. On Windows it is
//! now a layered popup drawn with GDI+ on a thread of its own: a few MB, no
//! focus or clicks taken, and no work at all while hidden. Elsewhere
//! [`Overlay::spawn`] returns `None` and callers keep their own overlay.
//!
//! What doesn't touch the OS is plain functions, unit tested on every
//! platform: layout and placement (`layout`), motion (`anim`), colours
//! (`color`) and the drop shadow (`shadow`).

// Only the Windows backend uses these outside tests.
#[cfg_attr(not(windows), allow(dead_code))]
mod anim;
#[cfg_attr(not(windows), allow(dead_code))]
mod color;
#[cfg_attr(not(windows), allow(dead_code))]
mod glow;
#[cfg_attr(not(windows), allow(dead_code))]
mod layout;
#[cfg_attr(not(windows), allow(dead_code))]
mod notice;
#[cfg_attr(not(windows), allow(dead_code))]
mod shadow;

#[cfg(windows)]
mod gdiplus;
#[cfg(windows)]
mod toast;
#[cfg(windows)]
mod windows;

use pady_core::Level;

/// The volume/brightness "pill" shown near the bottom of the screen.
///
/// `Send + Sync`: the engine threads that produce levels can share one.
pub struct Overlay {
    #[cfg(windows)]
    inner: windows::Overlay,
}

impl Overlay {
    /// Starts the overlay's own UI thread with a hidden window. `None` on
    /// non-Windows platforms or if the window can't be created (callers fall back).
    pub fn spawn() -> Option<Overlay> {
        #[cfg(windows)]
        return windows::Overlay::spawn().map(|inner| Overlay { inner });
        #[cfg(not(windows))]
        None
    }

    /// Show `level` (animating from the previous value) and restart the hide timer.
    pub fn show(&self, level: Level) {
        #[cfg(windows)]
        self.inner.show(level);
        #[cfg(not(windows))]
        let _ = level;
    }
}

// Dropping an `Overlay` drops `inner`, which stops the thread and destroys the window.

/// A small notice in the top-right corner of the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// Something was copied (from anywhere; what was copied is never read).
    Copied,
    /// A gesture was ignored because a full-screen game is in front.
    Paused,
}

/// The corner notices: a layered window on a thread of its own, like the pill.
pub struct Toasts {
    #[cfg(windows)]
    inner: toast::Toasts,
}

impl Toasts {
    /// `None` on non-Windows platforms or if the window can't be created.
    pub fn spawn() -> Option<Toasts> {
        #[cfg(windows)]
        return toast::Toasts::spawn().map(|inner| Toasts { inner });
        #[cfg(not(windows))]
        None
    }

    pub fn show(&self, notice: Notice) {
        #[cfg(windows)]
        self.inner.show(notice);
        #[cfg(not(windows))]
        let _ = notice;
    }

    /// Show "Copied" whenever the clipboard changes (on) or never (off).
    pub fn watch_clipboard(&self, on: bool) {
        #[cfg(windows)]
        self.inner.watch_clipboard(on);
        #[cfg(not(windows))]
        let _ = on;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pady_core::LevelKind;

    #[test]
    fn overlay_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Overlay>();
    }

    /// Off Windows there is nothing to spawn; on Windows the thread starts,
    /// takes a level and shuts down when dropped (if there is a desktop).
    #[test]
    fn spawn_show_and_drop() {
        #[cfg(windows)]
        let _turn = windows::tests::GDIPLUS.lock().unwrap_or_else(|e| e.into_inner());
        let overlay = Overlay::spawn();
        if cfg!(not(windows)) {
            assert!(overlay.is_none());
        }
        if let Some(overlay) = overlay {
            overlay.show(Level { kind: LevelKind::Volume, percent: 40, muted: false });
        } // dropped here: joins the thread, so a hang would hang the test
    }
}
