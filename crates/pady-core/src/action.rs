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
    /// The system's clipboard history (Win+V on Windows; the same shortcut
    /// on Linux desktops that have one, such as KDE), to paste something
    /// copied earlier.
    Clipboard,
    /// Open a small note by the cursor to jot something down; selected text
    /// goes in it. The app does this itself (it needs a window).
    QuickNote,
    /// The app switcher (Alt+Tab). On a swipe it stays open while the fingers
    /// keep sliding: each step picks the next app, sliding back the previous
    /// one, and lifting switches to it. On any other gesture, one Alt+Tab.
    AppSwitcher,
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

    /// Is every piece of text in this action of a sane length
    /// ([`crate::Config::MAX_TEXT`], [`crate::Config::MAX_ARGS`])?
    pub fn fits(&self) -> bool {
        use crate::Config;
        let ok = |s: &str| s.len() <= Config::MAX_TEXT;
        match self {
            Action::Keys { combo } => ok(combo),
            Action::OpenUrl { url, browser } => ok(url) && browser.as_deref().is_none_or(ok),
            Action::Launch { program, args } | Action::FocusOrLaunch { program, args } => {
                ok(program) && args.len() <= Config::MAX_ARGS && args.iter().all(|a| ok(a))
            }
            Action::Run { command } => ok(command),
            _ => true,
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

    /// `times` of this level change in one: `volume +2%` three times is
    /// `volume +6%`. Other actions come back unchanged.
    pub fn times(&self, times: u32) -> Action {
        let sum = |step: i32| (i64::from(step) * i64::from(times)).clamp(-100, 100) as i32;
        match *self {
            Action::Volume { step } => Action::Volume { step: sum(step) },
            Action::Brightness { step } => Action::Brightness { step: sum(step) },
            ref other => other.clone(),
        }
    }

    /// Actions the app runs itself: they need a window or its own state,
    /// which the engine and the executor have none of.
    pub fn is_app_action(&self) -> bool {
        matches!(self, Action::QuickNote)
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Action::None)
    }

    /// [`Action::label`] for the log file, which people attach to bug
    /// reports: a command is named by its program and a link by its site,
    /// because the rest (arguments, paths, queries) is where keys and tokens
    /// live. Everything else reads the same as its label.
    pub fn log_label(&self) -> String {
        use crate::logsafe::{command_name, link_site};
        match self {
            Action::Run { command } => match command_name(command) {
                "" => "Run a command".into(),
                program => format!("Run `{program} …`"),
            },
            Action::OpenUrl { url, browser: Some(b) } => format!("Open {} in {b}", link_site(url)),
            Action::OpenUrl { url, browser: None } => format!("Open {}", link_site(url)),
            other => other.label(),
        }
    }

    /// `text` (an error about this action) with the action's private parts
    /// taken out: executors quote the command, link or arguments they were
    /// given in their errors.
    pub fn redact(&self, text: &str) -> String {
        use crate::logsafe::{command_name, link_site};
        let swap = |text: String, secret: &str, shown: &str| {
            if secret.trim().is_empty() {
                text
            } else {
                text.replace(secret, shown)
            }
        };
        let text = text.to_string();
        match self {
            Action::Run { command } => swap(text, command, &format!("{} …", command_name(command))),
            Action::OpenUrl { url, .. } => swap(text, url, &link_site(url)),
            Action::Launch { args, .. } | Action::FocusOrLaunch { args, .. } => {
                let joined = swap(text, &args.join(" "), "…");
                args.iter().fold(joined, |text, arg| swap(text, arg, "…"))
            }
            _ => text,
        }
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
            Action::Clipboard => "Clipboard history".into(),
            Action::QuickNote => "Quick note".into(),
            Action::AppSwitcher => "App switcher".into(),
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
        let w: Wrap = toml::from_str(r#"action = { type = "quick_note" }"#).unwrap();
        assert_eq!(w.action, Action::QuickNote);
        assert!(w.action.is_app_action() && !Action::Clipboard.is_app_action());
        let w: Wrap = toml::from_str(r#"action = { type = "clipboard" }"#).unwrap();
        assert_eq!(w.action, Action::Clipboard);
        let w: Wrap = toml::from_str(r#"action = { type = "app_switcher" }"#).unwrap();
        assert_eq!(w.action, Action::AppSwitcher);
    }

    #[test]
    fn log_labels_leave_out_what_may_be_secret() {
        let run = Action::Run { command: "curl -H \"Authorization: Bearer abc123\" https://api.example.com".into() };
        assert_eq!(run.log_label(), "Run `curl …`");
        assert_eq!(
            run.redact("cannot run `curl -H \"Authorization: Bearer abc123\" https://api.example.com`: x"),
            "cannot run `curl …`: x"
        );
        let open =
            Action::OpenUrl { url: "https://example.com/key/abc123?u=1".into(), browser: Some("firefox".into()) };
        assert_eq!(open.log_label(), "Open https://example.com in firefox");
        assert_eq!(
            open.redact("could not open `https://example.com/key/abc123?u=1`"),
            "could not open `https://example.com`"
        );
        let launch = Action::Launch { program: "deploy.exe".into(), args: vec!["--token".into(), "abc123".into()] };
        assert_eq!(launch.log_label(), "Launch deploy.exe");
        assert_eq!(
            launch.redact("cannot launch deploy.exe --token abc123: not found"),
            "cannot launch deploy.exe …: not found"
        );
        assert_eq!(Action::Run { command: "  ".into() }.log_label(), "Run a command");
        // Nothing private in these: they read as they always did.
        for plain in [Action::Mute, Action::Volume { step: 5 }, Action::Keys { combo: "ctrl+c".into() }, Action::Lock] {
            assert_eq!(plain.log_label(), plain.label());
            assert_eq!(plain.redact("some error"), "some error");
        }
    }

    #[test]
    fn level_changes_add_up_and_stay_in_range() {
        assert_eq!(Action::Volume { step: 2 }.times(3), Action::Volume { step: 6 });
        assert_eq!(Action::Brightness { step: -2 }.times(10), Action::Brightness { step: -20 });
        assert_eq!(Action::Volume { step: 2 }.times(8_000), Action::Volume { step: 100 });
        assert_eq!(Action::Volume { step: -100 }.times(u32::MAX), Action::Volume { step: -100 });
        assert_eq!(Action::Mute.times(5), Action::Mute);
    }

    #[test]
    fn labels_show_sign() {
        assert_eq!(Action::Volume { step: 5 }.label(), "Volume +5%");
        assert_eq!(Action::Brightness { step: -10 }.label(), "Brightness -10%");
    }
}
