use crate::action::{Action, MediaKey};
use crate::gesture::{Direction, PinchDirection};
use crate::recognizer::RecognizerSettings;
use crate::trigger::Trigger;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// Tuning knobs. All distances are millimetres on the touchpad surface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Master switch; when off, Pady watches but never acts.
    pub enabled: bool,
    /// A tap must lift within this many milliseconds...
    pub tap_max_ms: u64,
    /// ...and no finger may wander further than this.
    pub tap_slop_mm: f32,
    /// Distance the fingers travel before a swipe or drag is recognised.
    pub swipe_threshold_mm: f32,
    /// Extra travel per repeat for bindings with `repeat = true`.
    pub swipe_step_mm: f32,
    /// Change in finger spread before a pinch is recognised.
    pub pinch_threshold_mm: f32,
    /// Fingers resting this long without moving count as a hold.
    pub hold_ms: u64,
    /// Cursor pixels per millimetre of finger travel while dragging.
    pub drag_speed: f32,
    /// How much faster flicks move the cursor while dragging (0 = linear).
    pub drag_acceleration: f32,
    /// Keep the mouse button held this long after lifting, so a drag can be
    /// continued by putting the fingers back down (0 = release immediately).
    pub drag_release_ms: u64,
    /// Show a small on-screen level while volume or brightness changes.
    pub show_overlay: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            enabled: true,
            tap_max_ms: 220,
            tap_slop_mm: 4.0,
            swipe_threshold_mm: 10.0,
            swipe_step_mm: 7.0,
            pinch_threshold_mm: 8.0,
            hold_ms: 600,
            drag_speed: 9.0,
            drag_acceleration: 1.0,
            drag_release_ms: 350,
            show_overlay: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    pub trigger: Trigger,
    pub action: Action,
    /// For swipes: keep firing every `swipe_step_mm` while the fingers keep
    /// moving, so a single slide can scrub volume or brightness.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub repeat: bool,
}

/// Bindings that apply only while one of `apps` is in front. They override
/// the global bindings trigger by trigger; a binding with action `none`
/// switches that gesture off in these apps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    /// Executable names such as `chrome.exe` or `code` (case and `.exe` don't matter).
    #[serde(default)]
    pub apps: Vec<String>,
    #[serde(default, rename = "binding")]
    pub bindings: Vec<Binding>,
}

impl Profile {
    pub fn matches(&self, app: &str) -> bool {
        let app = app_key(app);
        !app.is_empty() && self.apps.iter().any(|a| app_key(a) == app)
    }

    fn binding(&self, trigger: Trigger) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.trigger == trigger)
    }
}

/// `C:\Apps\Code.EXE` / `code.exe` / `Code` → `code`
fn app_key(app: &str) -> String {
    let base = app.trim().trim_matches('"').rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    base.strip_suffix(".exe").map(str::to_string).unwrap_or(base)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
    #[serde(default, rename = "binding")]
    pub bindings: Vec<Binding>,
    #[serde(default, rename = "profile", skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<Profile>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not read or write {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path} is not valid Pady config: {source}")]
    Parse { path: PathBuf, source: Box<toml::de::Error> },
    #[error("could not serialise config: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("could not locate the user config directory")]
    NoConfigDir,
}

const HEADER: &str = r#"# Pady configuration.
#
# Triggers:  tap:N  hold:N  swipe:N:up|down|left|right  pinch:N:in|out  drag:N   (N = 3..5)
# Actions:   none | keys{combo} | volume{step} | mute | brightness{step}
#            media{key = play_pause|next|previous|stop|volume_up|volume_down|mute}
#            open_url{url, browser?} | launch{program, args?} | focus_or_launch{program, args?}
#            run{command} | lock | drag
#
# Per-app profiles override bindings while an app is in front:
#   [[profile]]
#   name = "Browser"
#   apps = ["chrome.exe", "msedge.exe"]
#   [[profile.binding]]
#   trigger = "swipe:4:left"
#   action = { type = "keys", combo = "ctrl+shift+tab" }
#
# Edit this file by hand or from the Pady window; changes apply instantly.

"#;

impl Default for Config {
    fn default() -> Self {
        let b = |trigger, action, repeat| Binding { trigger, action, repeat };
        Config {
            settings: Settings::default(),
            bindings: vec![
                b(Trigger::Drag(3), Action::Drag, false),
                b(Trigger::Tap(3), Action::Media { key: MediaKey::PlayPause }, false),
                b(Trigger::Swipe(4, Direction::Up), Action::Volume { step: 4 }, true),
                b(Trigger::Swipe(4, Direction::Down), Action::Volume { step: -4 }, true),
                b(Trigger::Swipe(4, Direction::Right), Action::Brightness { step: 10 }, true),
                b(Trigger::Swipe(4, Direction::Left), Action::Brightness { step: -10 }, true),
                b(Trigger::Tap(4), Action::Keys { combo: "win+d".into() }, false),
                b(Trigger::Hold(4), Action::Lock, false),
                b(Trigger::Pinch(4, PinchDirection::Out), Action::Keys { combo: "ctrl+equal".into() }, true),
                b(Trigger::Pinch(4, PinchDirection::In), Action::Keys { combo: "ctrl+minus".into() }, true),
            ],
            profiles: Vec::new(),
        }
    }
}

impl Config {
    /// `%APPDATA%\Pady\config.toml` on Windows, `~/.config/pady/config.toml` on Linux.
    pub fn default_path() -> Result<PathBuf, ConfigError> {
        let dir = dirs::config_dir().ok_or(ConfigError::NoConfigDir)?;
        let app = if cfg!(windows) { "Pady" } else { "pady" };
        Ok(dir.join(app).join("config.toml"))
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let mut cfg: Config = toml::from_str(text)?;
        cfg.normalize();
        Ok(cfg)
    }

    pub fn to_toml(&self) -> Result<String, ConfigError> {
        Ok(format!("{HEADER}{}", toml::to_string_pretty(self)?))
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io { path: path.into(), source })?;
        Config::from_toml(&text).map_err(|source| ConfigError::Parse { path: path.into(), source: Box::new(source) })
    }

    /// Load the file, creating it with defaults on first run.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        if path.exists() {
            Config::load(path)
        } else {
            let cfg = Config::default();
            cfg.save(path)?;
            Ok(cfg)
        }
    }

    /// Write atomically (temp file + rename) so a crash never leaves half a config.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |source| ConfigError::Io { path: path.into(), source };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.to_toml()?).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    pub fn binding(&self, trigger: Trigger) -> Option<&Binding> {
        self.bindings.iter().find(|b| b.trigger == trigger && !b.action.is_none())
    }

    /// The first profile for `app`, if any.
    pub fn profile_for(&self, app: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.matches(app))
    }

    /// The binding that applies to `trigger` while `app` is in front, and
    /// the profile it came from. `None` if unbound or switched off there.
    pub fn resolve(&self, trigger: Trigger, app: Option<&str>) -> Option<(&Binding, Option<&Profile>)> {
        if let Some(profile) = app.and_then(|a| self.profile_for(a)) {
            if let Some(b) = profile.binding(trigger) {
                return (!b.action.is_none()).then_some((b, Some(profile)));
            }
        }
        self.binding(trigger).map(|b| (b, None))
    }

    /// Replace (or remove, for `Action::None`) the binding for a trigger.
    pub fn set_binding(&mut self, trigger: Trigger, action: Action, repeat: bool) {
        self.bindings.retain(|b| b.trigger != trigger);
        if !action.is_none() {
            self.bindings.push(Binding { trigger, action, repeat });
        }
    }

    /// Drop duplicate triggers (last one wins, like CSS) and nonsense pairs
    /// such as a drag action on a tap.
    ///
    /// Profiles keep `none` bindings (they switch a gesture off) but not
    /// drags: whether movement drags is decided before the app is known.
    pub fn normalize(&mut self) {
        self.bindings = dedupe(std::mem::take(&mut self.bindings), |b| {
            !b.action.is_none() && matches!(b.trigger, Trigger::Drag(_)) == matches!(b.action, Action::Drag)
        });
        for p in &mut self.profiles {
            p.name = p.name.trim().to_string();
            p.apps.retain(|a| !a.trim().is_empty());
            p.bindings = dedupe(std::mem::take(&mut p.bindings), |b| {
                !matches!(b.trigger, Trigger::Drag(_)) && !matches!(b.action, Action::Drag)
            });
        }
    }

    /// Finger counts whose movement is a drag rather than a swipe. A drag
    /// binding wins over swipe bindings for the same finger count.
    pub fn drag_fingers(&self) -> [bool; 6] {
        let mut out = [false; 6];
        for b in &self.bindings {
            if let (Trigger::Drag(n), Action::Drag) = (b.trigger, &b.action) {
                if let Some(slot) = out.get_mut(n as usize) {
                    *slot = true;
                }
            }
        }
        out
    }

    /// Finger counts with a pinch bound anywhere (globally or in a profile).
    pub fn pinch_fingers(&self) -> [bool; 6] {
        let mut out = [false; 6];
        let all = self.bindings.iter().chain(self.profiles.iter().flat_map(|p| &p.bindings));
        for b in all.filter(|b| !b.action.is_none()) {
            if let Trigger::Pinch(n, _) = b.trigger {
                if let Some(slot) = out.get_mut(n as usize) {
                    *slot = true;
                }
            }
        }
        out
    }

    /// Swipe bindings (global or in a profile) that will never fire because
    /// a drag owns that finger count.
    pub fn shadowed_bindings(&self) -> Vec<Trigger> {
        let drag = self.drag_fingers();
        let mut out: Vec<Trigger> = Vec::new();
        for b in self.bindings.iter().chain(self.profiles.iter().flat_map(|p| &p.bindings)) {
            if matches!(b.trigger, Trigger::Swipe(n, _) if drag[n as usize]) && !out.contains(&b.trigger) {
                out.push(b.trigger);
            }
        }
        out
    }

    pub fn recognizer_settings(&self) -> RecognizerSettings {
        let s = &self.settings;
        RecognizerSettings {
            min_fingers: Trigger::MIN_FINGERS,
            tap_max_ms: s.tap_max_ms,
            tap_slop_mm: s.tap_slop_mm,
            swipe_threshold_mm: s.swipe_threshold_mm,
            swipe_step_mm: s.swipe_step_mm.max(1.0),
            pinch_threshold_mm: s.pinch_threshold_mm.max(1.0),
            hold_ms: s.hold_ms,
            drag_release_ms: s.drag_release_ms,
            drag_fingers: self.drag_fingers(),
            pinch_fingers: self.pinch_fingers(),
        }
    }
}

/// Keep bindings passing `keep`, dropping earlier duplicates of a trigger
/// (the last one wins, like CSS).
fn dedupe(bindings: Vec<Binding>, keep: impl Fn(&Binding) -> bool) -> Vec<Binding> {
    let mut seen = Vec::new();
    let mut kept: Vec<Binding> = Vec::new();
    for b in bindings.into_iter().rev() {
        if keep(&b) && !seen.contains(&b.trigger) {
            seen.push(b.trigger);
            kept.push(b);
        }
    }
    kept.reverse();
    kept
}

/// Config shared between the engine and the UI. Readers cache a copy and
/// compare [`SharedConfig::generation`] to notice edits without locking on
/// every touch frame.
#[derive(Debug)]
pub struct SharedConfig {
    inner: RwLock<Config>,
    generation: AtomicU64,
}

impl SharedConfig {
    pub fn new(cfg: Config) -> Self {
        SharedConfig { inner: RwLock::new(cfg), generation: AtomicU64::new(1) }
    }

    pub fn get(&self) -> Config {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn set(&self, mut cfg: Config) {
        cfg.normalize();
        *self.inner.write().unwrap_or_else(|e| e.into_inner()) = cfg;
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn update(&self, f: impl FnOnce(&mut Config)) -> Config {
        let mut cfg = self.get();
        f(&mut cfg);
        self.set(cfg.clone());
        cfg
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_round_trips_through_toml() {
        let cfg = Config::default();
        let text = cfg.to_toml().unwrap();
        assert!(text.starts_with("# Pady configuration."));
        assert_eq!(Config::from_toml(&text).unwrap(), cfg);
    }

    #[test]
    fn hand_written_config_parses() {
        let cfg = Config::from_toml(
            r#"
            [settings]
            tap_max_ms = 300

            [[binding]]
            trigger = "swipe:3:up"
            action = { type = "volume", step = 5 }
            repeat = true

            [[binding]]
            trigger = "tap:4"
            action = { type = "focus_or_launch", program = "chrome.exe" }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.settings.tap_max_ms, 300);
        assert_eq!(cfg.settings.hold_ms, Settings::default().hold_ms, "missing settings use defaults");
        let b = cfg.binding(Trigger::Swipe(3, Direction::Up)).unwrap();
        assert_eq!(b.action, Action::Volume { step: 5 });
        assert!(b.repeat);
    }

    #[test]
    fn normalize_keeps_last_duplicate_and_drops_invalid() {
        let cfg = Config::from_toml(
            r#"
            [[binding]]
            trigger = "tap:3"
            action = { type = "mute" }
            [[binding]]
            trigger = "tap:3"
            action = { type = "lock" }
            [[binding]]
            trigger = "tap:4"
            action = { type = "drag" }
            [[binding]]
            trigger = "drag:4"
            action = { type = "mute" }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.bindings.len(), 1);
        assert_eq!(cfg.binding(Trigger::Tap(3)).unwrap().action, Action::Lock);
    }

    #[test]
    fn drag_shadows_swipes() {
        let mut cfg = Config::default();
        cfg.set_binding(Trigger::Swipe(3, Direction::Left), Action::Mute, false);
        assert!(cfg.drag_fingers()[3]);
        assert_eq!(cfg.shadowed_bindings(), vec![Trigger::Swipe(3, Direction::Left)]);
    }

    #[test]
    fn set_binding_none_removes() {
        let mut cfg = Config::default();
        cfg.set_binding(Trigger::Tap(3), Action::None, false);
        assert!(cfg.binding(Trigger::Tap(3)).is_none());
    }

    #[test]
    fn save_and_load() {
        let dir = std::env::temp_dir().join(format!("pady-test-{}", std::process::id()));
        let path = dir.join("nested").join("config.toml");
        let cfg = Config::load_or_create(&path).unwrap();
        assert_eq!(cfg, Config::default());
        let mut changed = cfg.clone();
        changed.settings.drag_speed = 3.5;
        changed.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), changed);
        std::fs::remove_dir_all(dir).ok();
    }

    const WITH_PROFILE: &str = r#"
        [[binding]]
        trigger = "swipe:4:left"
        action = { type = "keys", combo = "alt+left" }

        [[binding]]
        trigger = "tap:4"
        action = { type = "mute" }

        [[profile]]
        name = "Browser"
        apps = ["Chrome.EXE", "firefox"]

        [[profile.binding]]
        trigger = "swipe:4:left"
        action = { type = "keys", combo = "ctrl+shift+tab" }

        [[profile.binding]]
        trigger = "tap:4"
        action = { type = "none" }

        [[profile.binding]]
        trigger = "pinch:5:in"
        action = { type = "keys", combo = "ctrl+minus" }
        repeat = true
    "#;

    #[test]
    fn profiles_override_and_disable_per_app() {
        let cfg = Config::from_toml(WITH_PROFILE).unwrap();
        let left = Trigger::Swipe(4, Direction::Left);

        let (b, p) = cfg.resolve(left, Some(r"C:\Program Files\Google\Chrome\chrome.exe")).unwrap();
        assert_eq!(b.action, Action::Keys { combo: "ctrl+shift+tab".into() });
        assert_eq!(p.unwrap().name, "Browser");

        let (b, p) = cfg.resolve(left, Some("code.exe")).unwrap();
        assert_eq!(b.action, Action::Keys { combo: "alt+left".into() });
        assert!(p.is_none(), "other apps use the global binding");
        assert!(cfg.resolve(left, None).is_some());

        assert!(cfg.resolve(Trigger::Tap(4), Some("firefox.exe")).is_none(), "none switches it off");
        assert!(cfg.resolve(Trigger::Tap(4), Some("notepad")).is_some());
    }

    #[test]
    fn profiles_round_trip_and_enable_pinch_detection() {
        let cfg = Config::from_toml(WITH_PROFILE).unwrap();
        let text = cfg.to_toml().unwrap();
        assert!(text.contains("[[profile.binding]]"), "{text}");
        assert_eq!(Config::from_toml(&text).unwrap(), cfg);
        assert!(cfg.pinch_fingers()[5]);
        assert!(!cfg.pinch_fingers()[3]);
    }

    #[test]
    fn profiles_cannot_drag() {
        let mut cfg = Config::default();
        cfg.profiles.push(Profile {
            name: " Game ".into(),
            apps: vec!["game.exe".into(), " ".into()],
            bindings: vec![Binding { trigger: Trigger::Drag(4), action: Action::Drag, repeat: false }],
        });
        cfg.normalize();
        assert_eq!(cfg.profiles[0].name, "Game");
        assert_eq!(cfg.profiles[0].apps, vec!["game.exe".to_string()]);
        assert!(cfg.profiles[0].bindings.is_empty());
    }

    #[test]
    fn app_keys_ignore_path_case_and_extension() {
        assert_eq!(app_key(r"C:\Apps\Code.EXE"), "code");
        assert_eq!(app_key("/usr/bin/firefox"), "firefox");
        assert_eq!(app_key("\"Spotify.exe\""), "spotify");
    }

    #[test]
    fn shared_config_bumps_generation() {
        let shared = SharedConfig::new(Config::default());
        let g = shared.generation();
        shared.update(|c| c.settings.enabled = false);
        assert!(shared.generation() > g);
        assert!(!shared.get().settings.enabled);
    }
}
