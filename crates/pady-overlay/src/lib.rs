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
mod fx_draw;
#[cfg_attr(not(windows), allow(dead_code))]
mod glow;
#[cfg_attr(not(windows), allow(dead_code))]
mod layout;
mod look;
#[cfg_attr(not(windows), allow(dead_code))]
mod notice;
#[cfg_attr(not(windows), allow(dead_code))]
mod shadow;

#[cfg(windows)]
mod fx;
#[cfg(windows)]
mod gdiplus;
#[cfg(windows)]
mod toast;
#[cfg(windows)]
mod windows;

use pady_core::{Direction, Level, TouchFrame};

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

pub use look::{set_look, Look};

/// A small notice in the top-right corner of the screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Notice {
    /// Something was copied (from anywhere; what was copied is never read).
    Copied,
    /// A gesture pasted.
    Pasted,
    /// A gesture was ignored because a full-screen game is in front.
    Paused,
    /// Anything else: a gesture and what it did, a pinch step, an action.
    Text { text: String, icon: Icon },
}

/// The symbol in a [`Notice::Text`]'s dot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Touch,
    ZoomIn,
    ZoomOut,
    Camera,
    Music,
    Lock,
    Done,
    Keyboard,
}

/// Don't show "Copied" for clipboard changes in the next `time`: Pady is
/// using the clipboard itself (to read or replace the selected text).
pub fn quiet_clipboard(time: std::time::Duration) {
    #[cfg(windows)]
    toast::quiet_clipboard(time);
    #[cfg(not(windows))]
    let _ = time;
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

    /// Name the keyboard language whenever it's switched (on) or never (off).
    pub fn watch_language(&self, on: bool) {
        #[cfg(windows)]
        self.inner.watch_language(on);
        #[cfg(not(windows))]
        let _ = on;
    }

    /// A gesture just ran and may have switched the keyboard language: check
    /// closely for a moment, so its notice follows the switch at once.
    pub fn language_may_change(&self) {
        #[cfg(windows)]
        self.inner.language_may_change();
    }
}

/// Animations that follow the fingers: a glow at the screen edge a swipe is
/// heading for, and glowing fingertips on a small touchpad in the corner.
pub struct Effects {
    #[cfg(windows)]
    inner: fx::Effects,
}

impl Effects {
    /// `None` on non-Windows platforms or if the windows can't be created.
    pub fn spawn() -> Option<Effects> {
        #[cfg(windows)]
        return fx::Effects::spawn().map(|inner| Effects { inner });
        #[cfg(not(windows))]
        None
    }

    /// A swipe in `direction` has come `amount` (0..1) of the way.
    pub fn swipe_progress(&self, direction: Direction, amount: f32) {
        #[cfg(windows)]
        {
            let edge = match direction {
                Direction::Up => fx_draw::Edge::Top,
                Direction::Down => fx_draw::Edge::Bottom,
                Direction::Left => fx_draw::Edge::Left,
                Direction::Right => fx_draw::Edge::Right,
            };
            self.inner.send(fx::Msg::Progress(edge, amount));
        }
        #[cfg(not(windows))]
        let _ = (direction, amount);
    }

    /// The swipe fired (the glow flashes) or was let go (it fades).
    pub fn swipe_ended(&self, fired: bool) {
        #[cfg(windows)]
        self.inner.send(fx::Msg::Ended(fired));
        #[cfg(not(windows))]
        let _ = fired;
    }

    /// Where the fingers are now (an empty frame: lifted).
    pub fn touches(&self, frame: &TouchFrame) {
        #[cfg(windows)]
        {
            let points = frame.contacts.iter().map(|c| (c.x, c.y)).collect();
            self.inner.send(fx::Msg::Touches(points, (frame.width_mm, frame.height_mm)));
        }
        #[cfg(not(windows))]
        let _ = frame;
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
