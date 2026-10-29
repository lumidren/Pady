//! Starter sets of bindings, so a first run is one click away from useful.
//!
//! Shortcuts differ a little between Windows and Linux desktops (task view,
//! switching desktops, screenshots), so presets pick the right ones per OS.

use crate::action::{Action, MediaKey};
use crate::config::Binding;
use crate::gesture::{Direction, PinchDirection};
use crate::trigger::Trigger;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "recommended",
        name: "Recommended",
        description: "Three-finger drag, taps for play/pause and next track, four-finger swipes for volume and brightness, pinch to zoom, three-finger pinch to copy and paste.",
    },
    Preset {
        id: "macos",
        name: "Like a Mac",
        description: "Feels like a MacBook trackpad: three-finger drag, four-finger swipes between desktops and to task view, pinch to zoom, three-finger pinch to copy and paste.",
    },
    Preset {
        id: "media",
        name: "Media",
        description: "Built for music and video: taps control playback, four-finger swipes scrub the volume and skip tracks.",
    },
    Preset {
        id: "productivity",
        name: "Productivity",
        description: "Window and desktop management: task view, show desktop, switching desktops, quick screenshots, lock, and three-finger pinch to copy and paste.",
    },
    Preset {
        id: "minimal",
        name: "Minimal",
        description: "Just three-finger drag. Start small and add your own.",
    },
];

fn keys(combo: &str) -> Action {
    Action::Keys { combo: combo.into() }
}

fn media(key: MediaKey) -> Action {
    Action::Media { key }
}

/// Shortcuts that differ between desktops.
struct Shortcuts {
    task_view: &'static str,
    show_desktop: &'static str,
    desktop_left: &'static str,
    desktop_right: &'static str,
    screenshot: &'static str,
}

const SHORTCUTS: Shortcuts = if cfg!(windows) {
    Shortcuts {
        task_view: "win+tab",
        show_desktop: "win+d",
        desktop_left: "win+ctrl+left",
        desktop_right: "win+ctrl+right",
        screenshot: "win+shift+s",
    }
} else {
    Shortcuts {
        task_view: "win",
        show_desktop: "win+d",
        desktop_left: "win+pageup",
        desktop_right: "win+pagedown",
        screenshot: "printscreen",
    }
};

/// The bindings of the preset with this id.
pub fn bindings(id: &str) -> Option<Vec<Binding>> {
    use Direction::{Down, Left, Right, Up};
    use PinchDirection::{In, Out};
    let b = |trigger, action, repeat| Binding { trigger, action, repeat };
    let drag = b(Trigger::Drag(3), Action::Drag, false);
    let zoom =
        [b(Trigger::Pinch(4, Out), keys("ctrl+equal"), true), b(Trigger::Pinch(4, In), keys("ctrl+minus"), true)];
    // Pinch three fingers together to copy, spread them to paste.
    let copy_paste =
        [b(Trigger::Pinch(3, In), keys("ctrl+c"), false), b(Trigger::Pinch(3, Out), keys("ctrl+v"), false)];
    let s = &SHORTCUTS;
    let list = match id {
        "recommended" => {
            let mut v = vec![
                drag,
                b(Trigger::Tap(3), media(MediaKey::PlayPause), false),
                b(Trigger::DoubleTap(3), media(MediaKey::Next), false),
                b(Trigger::Swipe(4, Up), Action::Volume { step: 4 }, true),
                b(Trigger::Swipe(4, Down), Action::Volume { step: -4 }, true),
                b(Trigger::Swipe(4, Right), Action::Brightness { step: 10 }, true),
                b(Trigger::Swipe(4, Left), Action::Brightness { step: -10 }, true),
                b(Trigger::Tap(4), keys(s.show_desktop), false),
                b(Trigger::Hold(4), Action::Lock, false),
            ];
            v.extend(zoom);
            v.extend(copy_paste);
            v
        }
        "macos" => {
            let mut v = vec![
                drag,
                b(Trigger::Swipe(4, Up), keys(s.task_view), false),
                b(Trigger::Swipe(4, Down), keys(s.show_desktop), false),
                // Like a Mac: fingers moving left bring in the desktop on the right.
                b(Trigger::Swipe(4, Left), keys(s.desktop_right), false),
                b(Trigger::Swipe(4, Right), keys(s.desktop_left), false),
                b(Trigger::Tap(3), media(MediaKey::PlayPause), false),
            ];
            v.extend(zoom);
            v.extend(copy_paste);
            v
        }
        "media" => vec![
            drag,
            b(Trigger::Tap(3), media(MediaKey::PlayPause), false),
            b(Trigger::DoubleTap(3), media(MediaKey::Next), false),
            b(Trigger::TripleTap(3), media(MediaKey::Previous), false),
            b(Trigger::Swipe(4, Up), Action::Volume { step: 4 }, true),
            b(Trigger::Swipe(4, Down), Action::Volume { step: -4 }, true),
            b(Trigger::Swipe(4, Left), media(MediaKey::Previous), false),
            b(Trigger::Swipe(4, Right), media(MediaKey::Next), false),
            b(Trigger::Tap(4), Action::Mute, false),
        ],
        "productivity" => {
            let mut v = vec![
                drag,
                b(Trigger::Swipe(4, Left), keys(s.desktop_right), false),
                b(Trigger::Swipe(4, Right), keys(s.desktop_left), false),
                b(Trigger::Swipe(4, Up), keys(s.task_view), false),
                b(Trigger::Swipe(4, Down), keys(s.show_desktop), false),
                b(Trigger::Tap(4), keys(s.screenshot), false),
                b(Trigger::Hold(4), Action::Lock, false),
                b(Trigger::Tap(3), media(MediaKey::PlayPause), false),
            ];
            v.extend(zoom);
            v.extend(copy_paste);
            v
        }
        "minimal" => vec![drag],
        _ => return None,
    };
    Some(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::keys::KeyCombo;

    #[test]
    fn every_preset_has_valid_bindings() {
        for p in PRESETS {
            let list = bindings(p.id).unwrap_or_else(|| panic!("{} has no bindings", p.id));
            let mut cfg = Config { bindings: list.clone(), ..Default::default() };
            cfg.normalize();
            assert_eq!(cfg.bindings, list, "{}: normalize must keep every binding", p.id);
            for b in &list {
                if let Action::Keys { combo } = &b.action {
                    combo.parse::<KeyCombo>().unwrap_or_else(|e| panic!("{}: {combo}: {e}", p.id));
                }
            }
        }
    }

    #[test]
    fn ids_are_unique_and_unknown_ids_are_rejected() {
        let mut ids: Vec<_> = PRESETS.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), PRESETS.len());
        assert!(bindings("nope").is_none());
    }

    #[test]
    fn recommended_is_the_default_config() {
        assert_eq!(Config::default().bindings, bindings("recommended").unwrap());
    }
}
