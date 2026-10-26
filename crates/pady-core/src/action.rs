use serde::{Deserialize, Serialize};

/// Something Pady does on the computer when a gesture fires.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    #[default]
    None,
    /// A keyboard shortcut such as `ctrl+shift+t` or `win+d`.
    Keys { combo: String },
    /// Change the master volume by `step` percent (negative lowers it).
    Volume { step: i32 },
    /// Toggle mute on the default output device.
    Mute,
    /// Change display brightness by `step` percent (negative dims it).
    Brightness { step: i32 },
    /// A media key; these also show the OS volume overlay where available.
    Media { key: MediaKey },
    /// Open a link, optionally in a specific browser (`chrome`, `firefox`, `edge`, ...).
    OpenUrl {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        browser: Option<String>,
    },
    /// Start a program (path, name on PATH, or a URI such as `ms-settings:`).
    Launch {
        program: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    /// Bring the program's window to the front, minimise it if it is already
    /// in front, or launch it if it is not running.
    FocusOrLaunch {
        program: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    /// Run a shell command (`cmd /C` on Windows, `sh -c` on Linux).
    Run { command: String },
    /// Lock the session.
    Lock,
    /// Hold the left mouse button while the fingers move (drag triggers only).
    Drag,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKey {
    PlayPause,
    Next,
    Previous,
    Stop,
    VolumeUp,
    VolumeDown,
    Mute,
}

impl MediaKey {
    pub fn label(self) -> &'static str {
        match self {
            MediaKey::PlayPause => "Play / pause",
            MediaKey::Next => "Next track",
            MediaKey::Previous => "Previous track",
            MediaKey::Stop => "Stop",
            MediaKey::VolumeUp => "Volume up",
            MediaKey::VolumeDown => "Volume down",
            MediaKey::Mute => "Mute",
        }
    }
}

impl Action {
    /// Volume and brightness steps beyond ±100% mean nothing; keep what a
    /// hand-edited config says in range.
    pub fn sanitize(&mut self) {
        if let Action::Volume { step } | Action::Brightness { step } = self {
            *step = (*step).clamp(-100, 100);
        }
    }

    /// Volume or brightness: actions that move a level up or down.
    pub fn is_level(&self) -> bool {
        matches!(self, Action::Volume { .. } | Action::Brightness { .. })
    }

    /// The same level change, in gliding-sized (2%) steps.
    pub fn glide_step(&self) -> Action {
        match *self {
            Action::Volume { step } => Action::Volume { step: 2 * step.signum() },
            Action::Brightness { step } => Action::Brightness { step: 2 * step.signum() },
            ref other => other.clone(),
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Action::None)
    }

    /// Short human-readable description for logs, tray tooltips and the UI feed.
    pub fn label(&self) -> String {
        match self {
            Action::None => "Nothing".into(),
            Action::Keys { combo } => format!("Press {combo}"),
            Action::Volume { step } if *step >= 0 => format!("Volume +{step}%"),
            Action::Volume { step } => format!("Volume {step}%"),
            Action::Mute => "Toggle mute".into(),
            Action::Brightness { step } if *step >= 0 => format!("Brightness +{step}%"),
            Action::Brightness { step } => format!("Brightness {step}%"),
            Action::Media { key } => key.label().into(),
            Action::OpenUrl { url, browser: Some(b) } => format!("Open {url} in {b}"),
            Action::OpenUrl { url, browser: None } => format!("Open {url}"),
            Action::Launch { program, .. } => format!("Launch {program}"),
            Action::FocusOrLaunch { program, .. } => format!("Switch to {program}"),
            Action::Run { command } => format!("Run `{command}`"),
            Action::Lock => "Lock screen".into(),
            Action::Drag => "Drag (hold left button)".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct Wrap {
        action: Action,
    }

    #[test]
    fn toml_shape_is_friendly() {
        let w: Wrap =
            toml::from_str(r#"action = { type = "open_url", url = "https://example.com", browser = "chrome" }"#)
                .unwrap();
        assert_eq!(w.action, Action::OpenUrl { url: "https://example.com".into(), browser: Some("chrome".into()) });
        let w: Wrap = toml::from_str(r#"action = { type = "media", key = "play_pause" }"#).unwrap();
        assert_eq!(w.action, Action::Media { key: MediaKey::PlayPause });
        let w: Wrap = toml::from_str(r#"action = { type = "lock" }"#).unwrap();
        assert_eq!(w.action, Action::Lock);
    }

    #[test]
    fn labels_show_sign() {
        assert_eq!(Action::Volume { step: 5 }.label(), "Volume +5%");
        assert_eq!(Action::Brightness { step: -10 }.label(), "Brightness -10%");
    }
}
