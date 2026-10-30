//! Which corner notice, if any, a gesture deserves: "Pasted", the pinch
//! steps, a hint naming the gesture, or an action you couldn't otherwise see.

use pady_core::{Gesture, PinchDirection, Settings, Trigger};
use pady_overlay::{Icon, Notice};

fn step(gesture: &Gesture) -> u32 {
    match gesture {
        Gesture::Swipe { step, .. } | Gesture::Pinch { step, .. } => *step,
        _ => 0,
    }
}

fn is_combo(action: &str, combos: &[&str]) -> bool {
    action.strip_prefix("Press ").is_some_and(|c| combos.iter().any(|x| c.eq_ignore_ascii_case(x)))
}

/// `action` is the label of what ran (as in [`pady_core::EngineEvent::Gesture`]).
pub fn notice_for(s: &Settings, gesture: &Gesture, trigger: Option<&str>, action: Option<&str>) -> Option<Notice> {
    let action = action?;
    if matches!(gesture, Gesture::DragStart { .. } | Gesture::DragMove { .. } | Gesture::DragEnd) {
        return None;
    }
    // Copying already shows "Copied" (from the clipboard); pasting has nothing to watch, so it says so here.
    if is_combo(action, &["ctrl+c", "ctrl+x", "ctrl+insert"]) {
        return None;
    }
    // Switching the keyboard language shows the new language itself (when that notice is on).
    if is_combo(action, &["win+space"]) {
        return None;
    }
    if is_combo(action, &["ctrl+v", "shift+insert"]) {
        return (s.show_copy_notice && step(gesture) == 0).then_some(Notice::Pasted);
    }
    if let Gesture::Pinch { direction, step, .. } = gesture {
        if !s.anim_pinch {
            return None;
        }
        let what = if is_combo(action, &["ctrl+equal", "ctrl+plus", "ctrl+="]) {
            "Zoom in".to_string()
        } else if is_combo(action, &["ctrl+minus", "ctrl+-"]) {
            "Zoom out".to_string()
        } else {
            action.to_string()
        };
        let text = if *step > 0 { format!("{what} · {}", step + 1) } else { what };
        let icon = if *direction == PinchDirection::Out { Icon::ZoomIn } else { Icon::ZoomOut };
        return Some(Notice::Text { text, icon });
    }
    // Repeats (scrubbing volume) and levels have their own feedback.
    if step(gesture) > 0 || action.starts_with("Volume") || action.starts_with("Brightness") || action == "Toggle mute"
    {
        return None;
    }
    if s.anim_hints {
        let gesture = trigger.and_then(|t| t.parse::<Trigger>().ok()).map(Trigger::label)?;
        return Some(Notice::Text { text: format!("{gesture} · {action}"), icon: Icon::Touch });
    }
    if s.anim_notices && matches!(action, "Play / pause" | "Next track" | "Previous track" | "Stop") {
        return Some(Notice::Text { text: action.to_string(), icon: Icon::Music });
    }
    if s.anim_screenshot && is_combo(action, &["win+shift+s", "printscreen", "win+printscreen"]) {
        return Some(Notice::Text { text: "Screenshot".into(), icon: Icon::Camera });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pady_core::Direction;

    fn swipe(step: u32) -> Gesture {
        Gesture::Swipe { fingers: 4, direction: Direction::Up, step }
    }

    fn pinch(direction: PinchDirection, step: u32) -> Gesture {
        Gesture::Pinch { fingers: 4, direction, step }
    }

    #[test]
    fn copy_and_paste() {
        let s = Settings::default();
        let grab = pinch(PinchDirection::In, 0);
        assert_eq!(notice_for(&s, &grab, Some("pinch:3:in"), Some("Press ctrl+c")), None, "the clipboard says Copied");
        let drop = pinch(PinchDirection::Out, 0);
        assert_eq!(notice_for(&s, &drop, Some("pinch:3:out"), Some("Press ctrl+v")), Some(Notice::Pasted));
        let off = Settings { show_copy_notice: false, ..Settings::default() };
        assert_eq!(notice_for(&off, &drop, Some("pinch:3:out"), Some("Press ctrl+v")), None);
    }

    #[test]
    fn switching_the_keyboard_language_leaves_it_to_the_language_notice() {
        let hints = Settings { anim_hints: true, ..Settings::default() };
        let tap = Gesture::Tap { fingers: 4 };
        assert_eq!(notice_for(&hints, &tap, Some("tap:4"), Some("Press win+space")), None);
        assert!(
            notice_for(&hints, &tap, Some("tap:4"), Some("Press win+d")).is_some(),
            "other shortcuts still get a hint"
        );
    }

    #[test]
    fn pinch_steps_count_up() {
        let s = Settings::default();
        let first = notice_for(&s, &pinch(PinchDirection::Out, 0), Some("pinch:4:out"), Some("Press ctrl+equal"));
        assert_eq!(first, Some(Notice::Text { text: "Zoom in".into(), icon: Icon::ZoomIn }));
        let third = notice_for(&s, &pinch(PinchDirection::In, 2), Some("pinch:4:in"), Some("Press ctrl+minus"));
        assert_eq!(third, Some(Notice::Text { text: "Zoom out · 3".into(), icon: Icon::ZoomOut }));
        let off = Settings { anim_pinch: false, ..Settings::default() };
        assert_eq!(notice_for(&off, &pinch(PinchDirection::Out, 0), None, Some("Press ctrl+equal")), None);
    }

    #[test]
    fn hints_name_the_gesture_when_turned_on() {
        let off = Settings::default();
        assert_eq!(notice_for(&off, &swipe(0), Some("swipe:4:up"), Some("Press win+tab")), None, "off by default");
        let on = Settings { anim_hints: true, ..Settings::default() };
        let hint = notice_for(&on, &swipe(0), Some("swipe:4:up"), Some("Press win+tab"));
        assert_eq!(hint, Some(Notice::Text { text: "4-finger swipe up · Press win+tab".into(), icon: Icon::Touch }));
        assert_eq!(notice_for(&on, &swipe(3), Some("swipe:4:up"), Some("Press win+tab")), None, "not on repeats");
        assert_eq!(notice_for(&on, &swipe(0), Some("swipe:4:up"), Some("Volume +4%")), None, "the pill shows levels");
        // Corner notices go by their own switch, not the moving-animations one.
        let master_off = Settings { animations: false, ..on };
        assert_eq!(notice_for(&master_off, &swipe(0), Some("swipe:4:up"), Some("Press win+tab")), hint);
    }

    #[test]
    fn invisible_actions_get_a_notice() {
        let s = Settings::default();
        let tap = Gesture::Tap { fingers: 3 };
        let media = notice_for(&s, &tap, Some("tap:3"), Some("Next track"));
        assert_eq!(media, Some(Notice::Text { text: "Next track".into(), icon: Icon::Music }));
        let shot = notice_for(&s, &tap, Some("tap:3"), Some("Press win+shift+s"));
        assert_eq!(shot, None, "the screenshot notice is off by default");
        let on = Settings { anim_screenshot: true, ..Settings::default() };
        let shot = notice_for(&on, &tap, Some("tap:3"), Some("Press win+shift+s"));
        assert_eq!(shot, Some(Notice::Text { text: "Screenshot".into(), icon: Icon::Camera }));
        assert_eq!(notice_for(&s, &tap, Some("tap:3"), Some("Press win+d")), None);
        assert_eq!(notice_for(&s, &tap, Some("tap:3"), None), None, "nothing ran");
    }
}
