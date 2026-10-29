//! Linux executor.
//!
//! Keys, media keys and the drag button go through a uinput virtual device,
//! which works the same on X11 and Wayland. Everything else is delegated to
//! the usual desktop command-line tools (wpctl/pactl/amixer, brightnessctl,
//! xdg-open, loginctl...), trying the common ones in turn, because Linux
//! desktops have no single API for volume, brightness or window focus.

use crate::Conflict;
use evdev::uinput::{VirtualDevice, VirtualDeviceBuilder};
use evdev::{AttributeSet, EventType, InputEvent, Key as EvKey, RelativeAxisType};
use pady_core::{Action, ActionError, Config, Executor, Key, KeyCombo, Level, MediaKey, Modifiers, Trigger};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, fs, io};

const UINPUT_PATH: &str = "/dev/uinput";
const UINPUT_FIX: &str = "Click Fix permissions (or run pady-cli fix-permissions) and enter your \
    password once. Packages from the Pady releases page set this up automatically.";
/// Installs the optional helper programs Pady uses (brightness, focusing
/// windows, media players) where missing. Failures don't stop the rest.
const HELPERS_SCRIPT: &str = "missing=''\n\
    for tool in brightnessctl xdotool wmctrl playerctl; do \
        command -v \"$tool\" >/dev/null 2>&1 || missing=\"$missing $tool\"; done\n\
    if [ -n \"$missing\" ]; then\n\
        if command -v apt-get >/dev/null 2>&1; then DEBIAN_FRONTEND=noninteractive apt-get install -y $missing || true\n\
        elif command -v dnf >/dev/null 2>&1; then dnf install -y $missing || true\n\
        elif command -v pacman >/dev/null 2>&1; then pacman -S --needed --noconfirm $missing || true\n\
        elif command -v zypper >/dev/null 2>&1; then zypper --non-interactive install $missing || true\n\
        fi\n\
    fi\n";
/// The udev rule the .deb and .rpm packages install; [`fix_permissions`]
/// installs the same file for AppImage and source builds.
const UDEV_RULES: &str = include_str!("../../../packaging/linux/70-pady.rules");
/// How long to wait before trying `/dev/uinput` again after it failed, so a
/// stream of drag movements does not hammer it (or the log).
const UINPUT_RETRY: Duration = Duration::from_secs(5);
/// udev and the compositor need a moment to pick up a new input device;
/// events sent before that are silently lost.
const UINPUT_SETTLE: Duration = Duration::from_millis(200);

const MODIFIER_KEYS: [EvKey; 4] = [EvKey::KEY_LEFTCTRL, EvKey::KEY_LEFTALT, EvKey::KEY_LEFTSHIFT, EvKey::KEY_LEFTMETA];
const MEDIA_KEYS: [EvKey; 7] = [
    EvKey::KEY_PLAYPAUSE,
    EvKey::KEY_NEXTSONG,
    EvKey::KEY_PREVIOUSSONG,
    EvKey::KEY_STOPCD,
    EvKey::KEY_VOLUMEUP,
    EvKey::KEY_VOLUMEDOWN,
    EvKey::KEY_MUTE,
];

pub struct LinuxExecutor {
    /// Created on first use, so a missing `/dev/uinput` only breaks the
    /// actions that need it.
    uinput: Option<VirtualDevice>,
    /// When creating the device last failed, and the user-facing reason.
    uinput_failure: Option<(Instant, String)>,
    /// The volume tool that worked last time, tried first next time.
    audio: Option<AudioTool>,
}

impl Default for LinuxExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxExecutor {
    pub fn new() -> Self {
        LinuxExecutor { uinput: None, uinput_failure: None, audio: None }
    }

    fn device(&mut self) -> Result<&mut VirtualDevice, ActionError> {
        if self.uinput.is_none() {
            if let Some((when, reason)) = &self.uinput_failure {
                if when.elapsed() < UINPUT_RETRY {
                    return Err(reason.clone().into());
                }
            }
            match create_virtual_device() {
                Ok(dev) => {
                    log::info!("created uinput virtual device");
                    self.uinput = Some(dev);
                    self.uinput_failure = None;
                }
                Err(e) => {
                    let reason = uinput_error(&e);
                    log::warn!("{reason}");
                    self.uinput_failure = Some((Instant::now(), reason.clone()));
                    return Err(reason.into());
                }
            }
        }
        self.uinput.as_mut().ok_or_else(|| "virtual input device unavailable".into())
    }

    fn send_combo(&mut self, combo: &KeyCombo) -> Result<(), ActionError> {
        let seq = combo_sequence(combo);
        let uinput_err = match self.device() {
            Ok(dev) => return emit_keys(dev, &seq).map_err(|e| format!("sending keys failed: {e}").into()),
            Err(e) => e,
        };
        // ydotool talks to its own (often root) daemon, so it may work where
        // we cannot open /dev/uinput ourselves.
        if run_quiet("ydotool", ydotool_args(&seq)).is_ok() {
            return Ok(());
        }
        if is_x11() && run_quiet("xdotool", ["key", &xdotool_combo(combo)]).is_ok() {
            return Ok(());
        }
        Err(uinput_err)
    }

    fn media(&mut self, key: MediaKey) -> Result<Option<Level>, ActionError> {
        let uinput_err = match self.device() {
            // The desktop shows its own OSD for media keys, so no level here.
            Ok(dev) => {
                return emit_keys(dev, &[(media_key(key), true), (media_key(key), false)])
                    .map(|()| None)
                    .map_err(Into::into)
            }
            Err(e) => e,
        };
        let player = |cmd: &str| {
            run_quiet("playerctl", [cmd]).map(|()| None).map_err(|e| format!("{uinput_err} (fallback: {e})").into())
        };
        match key {
            MediaKey::PlayPause => player("play-pause"),
            MediaKey::Next => player("next"),
            MediaKey::Previous => player("previous"),
            MediaKey::Stop => player("stop"),
            // Changed behind the desktop's back, so no OSD: report the level.
            MediaKey::VolumeUp => self.volume(5),
            MediaKey::VolumeDown => self.volume(-5),
            MediaKey::Mute => self.mute(),
        }
    }

    /// Change the volume and read back the result (`None` if no tool could
    /// report it). Turning it up while muted also unmutes, as volume keys do.
    fn volume(&mut self, step: i32) -> Result<Option<Level>, ActionError> {
        // pactl has no limit like wpctl's `-l 1.0`: stop the step at 100%
        // rather than over-amplify.
        let step = match (step > 0, self.audio) {
            (true, Some(AudioTool::Pactl)) => {
                AudioTool::Pactl.read_level().map_or(step, |l| step.min(100 - i32::from(l.percent)).max(0))
            }
            _ => step,
        };
        if step != 0 {
            self.audio_command(|tool| tool.volume_args(step))
                .map_err(|e| format!("could not change the volume: {e}"))?;
        }
        let mut level = self.read_volume();
        if let Some(level) = level.as_mut().filter(|l| step > 0 && l.muted) {
            if self.audio_command(AudioTool::unmute_args).is_ok() {
                level.muted = false;
            }
        }
        Ok(level)
    }

    fn mute(&mut self) -> Result<Option<Level>, ActionError> {
        self.audio_command(AudioTool::mute_args).map_err(|e| format!("could not toggle mute: {e}"))?;
        Ok(self.read_volume())
    }

    /// The remembered audio tool first, then the rest in order of preference.
    fn audio_tools(&self) -> Vec<AudioTool> {
        let mut order: Vec<AudioTool> = self.audio.into_iter().collect();
        order.extend(AudioTool::ALL.iter().filter(|&&t| Some(t) != self.audio));
        order
    }

    /// Run the remembered audio tool, or probe them all in order.
    fn audio_command(&mut self, args: impl Fn(AudioTool) -> Vec<String>) -> Result<(), String> {
        let mut errors = Vec::new();
        for tool in self.audio_tools() {
            match run_quiet(tool.program(), args(tool)) {
                Ok(()) => {
                    self.audio = Some(tool);
                    return Ok(());
                }
                Err(e) => errors.push(e),
            }
        }
        self.audio = None;
        Err(errors.join("; "))
    }

    fn read_volume(&self) -> Option<Level> {
        self.audio_tools().into_iter().find_map(AudioTool::read_level)
    }
}

impl Executor for LinuxExecutor {
    fn run(&mut self, action: &Action) -> Result<Option<Level>, ActionError> {
        let done = match action {
            Action::Volume { step } => return self.volume(*step),
            Action::Mute => return self.mute(),
            Action::Brightness { step } => return brightness(*step),
            Action::Media { key } => return self.media(*key),
            // Drag is driven through mouse_button / mouse_move.
            Action::None | Action::Drag => Ok(()),
            Action::Keys { combo } => {
                let parsed: KeyCombo = combo.parse().map_err(|e| format!("invalid shortcut `{combo}`: {e}"))?;
                self.send_combo(&parsed)
            }
            Action::OpenUrl { url, browser } => open_url(url, browser.as_deref()),
            Action::Launch { program, args } => launch(program, args),
            Action::FocusOrLaunch { program, args } => focus_or_launch(program, args),
            Action::Run { command } => spawn_detached(Command::new("sh").arg("-c").arg(command))
                .map_err(|e| format!("cannot run `{command}`: {e}").into()),
            Action::Lock => lock(),
        };
        done.map(|()| None)
    }

    fn foreground_app(&mut self) -> Option<String> {
        process_name(active_window_pid()?)
    }

    fn mouse_button(&mut self, down: bool) -> Result<(), ActionError> {
        let dev = self.device()?;
        emit_keys(dev, &[(EvKey::BTN_LEFT, down)]).map_err(Into::into)
    }

    fn mouse_move(&mut self, dx: i32, dy: i32) -> Result<(), ActionError> {
        if dx == 0 && dy == 0 {
            return Ok(());
        }
        let dev = self.device()?;
        let mut events = Vec::with_capacity(2);
        if dx != 0 {
            events.push(InputEvent::new(EventType::RELATIVE, RelativeAxisType::REL_X.0, dx));
        }
        if dy != 0 {
            events.push(InputEvent::new(EventType::RELATIVE, RelativeAxisType::REL_Y.0, dy));
        }
        dev.emit(&events).map_err(Into::into)
    }
}

// ---------------------------------------------------------------- uinput

fn create_virtual_device() -> io::Result<VirtualDevice> {
    let mut keys = AttributeSet::<EvKey>::new();
    for &k in Key::ALL {
        keys.insert(ev_key(k));
    }
    for k in MODIFIER_KEYS.into_iter().chain(MEDIA_KEYS) {
        keys.insert(k);
    }
    keys.insert(EvKey::BTN_LEFT);
    let mut axes = AttributeSet::<RelativeAxisType>::new();
    axes.insert(RelativeAxisType::REL_X);
    axes.insert(RelativeAxisType::REL_Y);
    let dev =
        VirtualDeviceBuilder::new()?.name("Pady virtual input").with_keys(&keys)?.with_relative_axes(&axes)?.build()?;
    std::thread::sleep(UINPUT_SETTLE);
    Ok(dev)
}

fn uinput_error(e: &io::Error) -> String {
    let hint = if e.kind() == io::ErrorKind::NotFound {
        "Load the uinput kernel module (sudo modprobe uinput), then: "
    } else {
        ""
    };
    format!("cannot open {UINPUT_PATH} to send keys and mouse input ({e}). {hint}{UINPUT_FIX}")
}

fn uinput_writable() -> bool {
    fs::OpenOptions::new().read(true).write(true).open(UINPUT_PATH).is_ok()
}

/// Each `(key, down)` is its own report, as from a real keyboard.
fn emit_keys(dev: &mut VirtualDevice, seq: &[(EvKey, bool)]) -> io::Result<()> {
    for &(key, down) in seq {
        dev.emit(&[InputEvent::new(EventType::KEY, key.code(), i32::from(down))])?;
    }
    Ok(())
}

/// Press modifiers, tap the key, release modifiers in reverse. A combo of
/// modifiers only (`win`) taps them.
fn combo_sequence(combo: &KeyCombo) -> Vec<(EvKey, bool)> {
    let mods = modifier_keys(combo.modifiers);
    let mut seq: Vec<(EvKey, bool)> = mods.iter().map(|&k| (k, true)).collect();
    if let Some(key) = combo.key {
        seq.push((ev_key(key), true));
        seq.push((ev_key(key), false));
    }
    seq.extend(mods.iter().rev().map(|&k| (k, false)));
    seq
}

fn modifier_keys(m: Modifiers) -> Vec<EvKey> {
    [m.ctrl, m.alt, m.shift, m.super_key]
        .into_iter()
        .zip(MODIFIER_KEYS)
        .filter_map(|(on, key)| on.then_some(key))
        .collect()
}

fn ev_key(key: Key) -> EvKey {
    match key {
        Key::A => EvKey::KEY_A,
        Key::B => EvKey::KEY_B,
        Key::C => EvKey::KEY_C,
        Key::D => EvKey::KEY_D,
        Key::E => EvKey::KEY_E,
        Key::F => EvKey::KEY_F,
        Key::G => EvKey::KEY_G,
        Key::H => EvKey::KEY_H,
        Key::I => EvKey::KEY_I,
        Key::J => EvKey::KEY_J,
        Key::K => EvKey::KEY_K,
        Key::L => EvKey::KEY_L,
        Key::M => EvKey::KEY_M,
        Key::N => EvKey::KEY_N,
        Key::O => EvKey::KEY_O,
        Key::P => EvKey::KEY_P,
        Key::Q => EvKey::KEY_Q,
        Key::R => EvKey::KEY_R,
        Key::S => EvKey::KEY_S,
        Key::T => EvKey::KEY_T,
        Key::U => EvKey::KEY_U,
        Key::V => EvKey::KEY_V,
        Key::W => EvKey::KEY_W,
        Key::X => EvKey::KEY_X,
        Key::Y => EvKey::KEY_Y,
        Key::Z => EvKey::KEY_Z,
        Key::Num0 => EvKey::KEY_0,
        Key::Num1 => EvKey::KEY_1,
        Key::Num2 => EvKey::KEY_2,
        Key::Num3 => EvKey::KEY_3,
        Key::Num4 => EvKey::KEY_4,
        Key::Num5 => EvKey::KEY_5,
        Key::Num6 => EvKey::KEY_6,
        Key::Num7 => EvKey::KEY_7,
        Key::Num8 => EvKey::KEY_8,
        Key::Num9 => EvKey::KEY_9,
        Key::F1 => EvKey::KEY_F1,
        Key::F2 => EvKey::KEY_F2,
        Key::F3 => EvKey::KEY_F3,
        Key::F4 => EvKey::KEY_F4,
        Key::F5 => EvKey::KEY_F5,
        Key::F6 => EvKey::KEY_F6,
        Key::F7 => EvKey::KEY_F7,
        Key::F8 => EvKey::KEY_F8,
        Key::F9 => EvKey::KEY_F9,
        Key::F10 => EvKey::KEY_F10,
        Key::F11 => EvKey::KEY_F11,
        Key::F12 => EvKey::KEY_F12,
        Key::Enter => EvKey::KEY_ENTER,
        Key::Escape => EvKey::KEY_ESC,
        Key::Tab => EvKey::KEY_TAB,
        Key::Space => EvKey::KEY_SPACE,
        Key::Backspace => EvKey::KEY_BACKSPACE,
        Key::Delete => EvKey::KEY_DELETE,
        Key::Insert => EvKey::KEY_INSERT,
        Key::Home => EvKey::KEY_HOME,
        Key::End => EvKey::KEY_END,
        Key::PageUp => EvKey::KEY_PAGEUP,
        Key::PageDown => EvKey::KEY_PAGEDOWN,
        Key::Up => EvKey::KEY_UP,
        Key::Down => EvKey::KEY_DOWN,
        Key::Left => EvKey::KEY_LEFT,
        Key::Right => EvKey::KEY_RIGHT,
        // The Print Screen key reports SysRq on Linux keyboards.
        Key::PrintScreen => EvKey::KEY_SYSRQ,
        Key::Minus => EvKey::KEY_MINUS,
        Key::Equal => EvKey::KEY_EQUAL,
        Key::Comma => EvKey::KEY_COMMA,
        Key::Period => EvKey::KEY_DOT,
        Key::Slash => EvKey::KEY_SLASH,
        Key::Backslash => EvKey::KEY_BACKSLASH,
        Key::Semicolon => EvKey::KEY_SEMICOLON,
        Key::Quote => EvKey::KEY_APOSTROPHE,
        Key::BracketLeft => EvKey::KEY_LEFTBRACE,
        Key::BracketRight => EvKey::KEY_RIGHTBRACE,
        Key::Grave => EvKey::KEY_GRAVE,
    }
}

fn media_key(key: MediaKey) -> EvKey {
    match key {
        MediaKey::PlayPause => EvKey::KEY_PLAYPAUSE,
        MediaKey::Next => EvKey::KEY_NEXTSONG,
        MediaKey::Previous => EvKey::KEY_PREVIOUSSONG,
        MediaKey::Stop => EvKey::KEY_STOPCD,
        MediaKey::VolumeUp => EvKey::KEY_VOLUMEUP,
        MediaKey::VolumeDown => EvKey::KEY_VOLUMEDOWN,
        MediaKey::Mute => EvKey::KEY_MUTE,
    }
}

/// ydotool 1.x syntax: `ydotool key 29:1 20:1 20:0 29:0`.
fn ydotool_args(seq: &[(EvKey, bool)]) -> Vec<String> {
    std::iter::once("key".to_string())
        .chain(seq.iter().map(|&(k, down)| format!("{}:{}", k.code(), u8::from(down))))
        .collect()
}

/// xdotool syntax: `ctrl+shift+Prior`.
fn xdotool_combo(combo: &KeyCombo) -> String {
    let m = combo.modifiers;
    let mut parts: Vec<&str> = [(m.ctrl, "ctrl"), (m.alt, "alt"), (m.shift, "shift"), (m.super_key, "super")]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
    if let Some(key) = combo.key {
        parts.push(x_keysym(key));
    }
    parts.join("+")
}

fn x_keysym(key: Key) -> &'static str {
    match key {
        Key::F1 => "F1",
        Key::F2 => "F2",
        Key::F3 => "F3",
        Key::F4 => "F4",
        Key::F5 => "F5",
        Key::F6 => "F6",
        Key::F7 => "F7",
        Key::F8 => "F8",
        Key::F9 => "F9",
        Key::F10 => "F10",
        Key::F11 => "F11",
        Key::F12 => "F12",
        Key::Enter => "Return",
        Key::Escape => "Escape",
        Key::Tab => "Tab",
        Key::Space => "space",
        Key::Backspace => "BackSpace",
        Key::Delete => "Delete",
        Key::Insert => "Insert",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "Prior",
        Key::PageDown => "Next",
        Key::Up => "Up",
        Key::Down => "Down",
        Key::Left => "Left",
        Key::Right => "Right",
        Key::PrintScreen => "Print",
        Key::Minus => "minus",
        Key::Equal => "equal",
        Key::Comma => "comma",
        Key::Period => "period",
        Key::Slash => "slash",
        Key::Backslash => "backslash",
        Key::Semicolon => "semicolon",
        Key::Quote => "apostrophe",
        Key::BracketLeft => "bracketleft",
        Key::BracketRight => "bracketright",
        Key::Grave => "grave",
        // Letters and digits are their own keysyms.
        other => other.name(),
    }
}

/// An X server (possibly XWayland) is reachable.
fn has_display() -> bool {
    env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
}

fn is_wayland() -> bool {
    env::var_os("WAYLAND_DISPLAY").is_some_and(|d| !d.is_empty())
        || env::var("XDG_SESSION_TYPE").is_ok_and(|t| t == "wayland")
}

fn is_x11() -> bool {
    has_display() && !is_wayland()
}

// ------------------------------------------------------------- processes

/// Run a short-lived helper, wait for it and return its stdout. The error is
/// one line saying why, for messages that list everything that was tried.
/// Output is untranslated (`LC_ALL=C`) because some of it is parsed. Only for
/// tools that do not leave children behind holding the pipes.
fn run_capture<I, S>(program: &str, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = Command::new(program).args(args).env("LC_ALL", "C").stdin(Stdio::null()).output();
    match out {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            match stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
                Some(line) => Err(format!("{program}: {line}")),
                None => Err(format!("{program} failed ({})", o.status)),
            }
        }
        Err(e) => Err(spawn_error(program, &e)),
    }
}

/// [`run_capture`] for commands whose output does not matter.
fn run_quiet<I, S>(program: &str, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run_capture(program, args).map(|_| ())
}

/// Like [`run_quiet`] for launchers whose children inherit our stdio: no
/// pipes, so waiting never depends on the launched program exiting.
fn run_status<I, S>(program: &str, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let status =
        Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{program} failed ({s})")),
        Err(e) => Err(spawn_error(program, &e)),
    }
}

fn spawn_error(program: &str, e: &io::Error) -> String {
    if e.kind() == io::ErrorKind::NotFound {
        format!("{program} is not installed")
    } else {
        format!("{program}: {e}")
    }
}

/// Start a program without waiting for it. It gets its own process group so
/// a Ctrl+C aimed at Pady's terminal does not take it down too.
fn spawn_detached(cmd: &mut Command) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn()?;
    // Reap it when it exits so it does not linger as a zombie.
    let reaper = std::thread::Builder::new().name("pady-reaper".into()).stack_size(64 * 1024).spawn(move || {
        let _ = child.wait();
    });
    if let Err(e) = reaper {
        log::debug!("cannot start reaper thread: {e}");
    }
    Ok(())
}

fn which(program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let executable = |p: &Path| fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0);
    if program.contains('/') {
        let p = PathBuf::from(program);
        return executable(&p).then_some(p);
    }
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths).map(|dir| dir.join(program)).find(|p| executable(p))
}

// ------------------------------------------------------------ the actions

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioTool {
    /// PipeWire (most current distributions).
    Wpctl,
    /// PulseAudio, or PipeWire's PulseAudio shim.
    Pactl,
    /// Plain ALSA.
    Amixer,
}

impl AudioTool {
    const ALL: [AudioTool; 3] = [AudioTool::Wpctl, AudioTool::Pactl, AudioTool::Amixer];

    fn program(self) -> &'static str {
        match self {
            AudioTool::Wpctl => "wpctl",
            AudioTool::Pactl => "pactl",
            AudioTool::Amixer => "amixer",
        }
    }

    fn volume_args(self, step: i32) -> Vec<String> {
        let n = step.unsigned_abs();
        let sign = if step >= 0 { '+' } else { '-' };
        let (base, amount): (&[&str], String) = match self {
            // `-l 1.0` stops at 100% instead of over-amplifying.
            AudioTool::Wpctl => (&["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@"], format!("{n}%{sign}")),
            AudioTool::Pactl => (&["set-sink-volume", "@DEFAULT_SINK@"], format!("{sign}{n}%")),
            AudioTool::Amixer => (&["-q", "sset", "Master"], format!("{n}%{sign}")),
        };
        base.iter().map(|s| s.to_string()).chain([amount]).collect()
    }

    fn mute_args(self) -> Vec<String> {
        self.set_mute_args("toggle")
    }

    fn unmute_args(self) -> Vec<String> {
        self.set_mute_args(match self {
            AudioTool::Wpctl | AudioTool::Pactl => "0",
            AudioTool::Amixer => "unmute",
        })
    }

    fn set_mute_args(self, value: &str) -> Vec<String> {
        let base: &[&str] = match self {
            AudioTool::Wpctl => &["set-mute", "@DEFAULT_AUDIO_SINK@"],
            AudioTool::Pactl => &["set-sink-mute", "@DEFAULT_SINK@"],
            AudioTool::Amixer => &["-q", "sset", "Master"],
        };
        base.iter().copied().chain([value]).map(String::from).collect()
    }

    /// The default output's volume and mute state, as this tool reports it.
    fn read_level(self) -> Option<Level> {
        match self {
            AudioTool::Wpctl => parse_wpctl_volume(&run_capture("wpctl", ["get-volume", "@DEFAULT_AUDIO_SINK@"]).ok()?),
            AudioTool::Pactl => {
                let volume = parse_pactl_volume(&run_capture("pactl", ["get-sink-volume", "@DEFAULT_SINK@"]).ok()?)?;
                let muted = parse_pactl_mute(&run_capture("pactl", ["get-sink-mute", "@DEFAULT_SINK@"]).ok()?)?;
                Some(Level::volume(volume, muted))
            }
            AudioTool::Amixer => parse_amixer(&run_capture("amixer", ["get", "Master"]).ok()?),
        }
    }
}

/// `wpctl get-volume`: `Volume: 0.45`, or `Volume: 0.45 [MUTED]`.
fn parse_wpctl_volume(out: &str) -> Option<Level> {
    let rest = out.trim().strip_prefix("Volume:")?;
    let volume: f32 = rest.split_whitespace().next()?.replace(',', ".").parse().ok()?;
    Some(Level::volume(volume * 100.0, rest.contains("[MUTED]")))
}

/// `pactl get-sink-volume`: `Volume: front-left: 29491 /  45% / -20.81 dB,
/// front-right: ...`; the average of the channels.
fn parse_pactl_volume(out: &str) -> Option<f32> {
    let percents: Vec<f32> =
        out.split_whitespace().filter_map(|t| t.strip_suffix('%')).filter_map(|n| n.parse().ok()).collect();
    average(&percents)
}

fn average(values: &[f32]) -> Option<f32> {
    (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
}

/// `pactl get-sink-mute`: `Mute: yes` or `Mute: no`.
fn parse_pactl_mute(out: &str) -> Option<bool> {
    match out.trim().strip_prefix("Mute:")?.trim() {
        "yes" => Some(true),
        "no" => Some(false),
        _ => None,
    }
}

/// `amixer get Master`: channel lines such as `Front Left: Playback 29491
/// [45%] [-20.00dB] [on]`. Muted when every channel's switch is off.
fn parse_amixer(out: &str) -> Option<Level> {
    let mut percents = Vec::new();
    let (mut on, mut off) = (false, false);
    for field in out.split_whitespace() {
        let Some(inner) = field.strip_prefix('[').and_then(|f| f.strip_suffix(']')) else { continue };
        match inner {
            "on" => on = true,
            "off" => off = true,
            _ => percents.extend(inner.strip_suffix('%').and_then(|p| p.parse::<f32>().ok())),
        }
    }
    Some(Level::volume(average(&percents)?, off && !on))
}

/// Change the brightness and report the new level (`Ok(None)` when it
/// changed but the tool did not say to what).
fn brightness(step: i32) -> Result<Option<Level>, ActionError> {
    let mut tried = Vec::new();
    match brightnessctl(step) {
        Ok(percent) => return Ok(percent.map(Level::brightness)),
        Err(e) => tried.push(e),
    }
    match sysfs_backlight(step) {
        Ok(percent) => return Ok(Some(Level::brightness(percent))),
        Err(e) => tried.push(e),
    }
    match ddc_brightness(step) {
        Ok(percent) => return Ok(Some(Level::brightness(percent))),
        Err(e) => tried.push(e),
    }
    Err(format!("could not change brightness: {}", tried.join("; ")).into())
}

/// `brightnessctl -m` prints the device after the change (step 0 only reads).
fn brightnessctl(step: i32) -> Result<Option<f32>, String> {
    let amount = format!("{}%{}", step.unsigned_abs(), if step >= 0 { '+' } else { '-' });
    let args = if step == 0 { vec!["-m", "info"] } else { vec!["-m", "set", &amount] };
    run_capture("brightnessctl", args).map(|out| parse_brightnessctl(&out))
}

/// `brightnessctl -m`: `intel_backlight,backlight,12000,50%,24000`
/// (device, class, current, percent, max).
fn parse_brightnessctl(out: &str) -> Option<f32> {
    let line = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    let fields: Vec<&str> = line.split(',').collect();
    let [_, _, current, percent, max] = fields[..] else { return None };
    match (current.parse::<f64>(), max.parse::<f64>()) {
        (Ok(current), Ok(max)) if max > 0.0 => Some((current * 100.0 / max) as f32),
        _ => percent.strip_suffix('%')?.parse().ok(),
    }
}

/// External monitors over DDC/CI (VCP feature 0x10 is brightness). Reading
/// first and setting an absolute value costs the same two DDC round trips
/// as a relative `setvcp 10 + N` and tells us the result.
fn ddc_brightness(step: i32) -> Result<f32, String> {
    let out = run_capture("ddcutil", ["getvcp", "10", "--terse"])?;
    let (current, max) = parse_ddcutil_vcp(&out).ok_or("ddcutil: no monitor reported its brightness")?;
    if step == 0 {
        return Ok(backlight_percent(current, max));
    }
    let value = backlight_value(current, max, step);
    run_quiet("ddcutil", ["setvcp", "10", &value.to_string()])?;
    Ok(backlight_percent(value.into(), max))
}

/// `ddcutil getvcp 10 --terse`: `VCP 10 C 50 100` (current, max).
fn parse_ddcutil_vcp(out: &str) -> Option<(i64, i64)> {
    out.lines().find_map(|line| match line.split_whitespace().collect::<Vec<_>>()[..] {
        ["VCP", "10", "C", current, max] => Some((current.parse().ok()?, max.parse().ok()?)),
        _ => None,
    })
}

/// Change the laptop backlight directly, or through logind (which lets the
/// session owner set it without root) when the sysfs file is not writable.
/// Returns the new level in percent.
fn sysfs_backlight(step: i32) -> Result<f32, String> {
    let base = Path::new("/sys/class/backlight");
    let mut devices: Vec<(u8, PathBuf)> = fs::read_dir(base)
        .map_err(|e| format!("{}: {e}", base.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .map(|p| (backlight_priority(fs::read_to_string(p.join("type")).unwrap_or_default().trim()), p))
        .collect();
    devices.sort();
    let (_, path) = devices.first().ok_or_else(|| format!("no backlight device in {}", base.display()))?;
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    let read = |file: &str| -> Result<i64, String> {
        let p = path.join(file);
        let text = fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        text.trim().parse().map_err(|e| format!("{}: {e}", p.display()))
    };
    let (current, max) = (read("brightness")?, read("max_brightness")?);
    if step == 0 {
        return Ok(backlight_percent(current, max));
    }
    let value = backlight_value(current, max, step);
    let file = path.join("brightness");
    if let Err(write_err) = fs::write(&file, value.to_string()) {
        run_quiet(
            "busctl",
            [
                "call",
                "org.freedesktop.login1",
                "/org/freedesktop/login1/session/auto",
                "org.freedesktop.login1.Session",
                "SetBrightness",
                "ssu",
                "backlight",
                name,
                &value.to_string(),
            ],
        )
        .map_err(|e| format!("cannot write {} ({write_err}) and logind refused ({e})", file.display()))?;
    }
    Ok(backlight_percent(value.into(), max))
}

fn backlight_percent(value: i64, max: i64) -> f32 {
    (value as f64 * 100.0 / max.max(1) as f64) as f32
}

/// The kernel's advice: prefer firmware interfaces over platform drivers
/// over raw GPU registers.
fn backlight_priority(kind: &str) -> u8 {
    match kind {
        "firmware" => 0,
        "platform" => 1,
        "raw" => 2,
        _ => 3,
    }
}

/// New backlight value after a `step`% change: always at least one unit (so
/// coarse panels with a handful of levels still respond) and never 0, which
/// turns some panels fully off.
fn backlight_value(current: i64, max: i64, step: i32) -> u32 {
    let max = max.max(1);
    let delta = (max * i64::from(step.unsigned_abs()) + 50) / 100;
    let delta = delta.max(1) * i64::from(step.signum());
    let value = (current + delta).clamp(1, max);
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn browser_candidates(browser: &str) -> Vec<&str> {
    match browser.to_ascii_lowercase().as_str() {
        "chrome" | "google-chrome" => vec!["google-chrome-stable", "google-chrome", "chromium", "chromium-browser"],
        "chromium" => vec!["chromium", "chromium-browser"],
        "firefox" => vec!["firefox"],
        "edge" | "msedge" | "microsoft-edge" => vec!["microsoft-edge-stable", "microsoft-edge"],
        "brave" => vec!["brave-browser", "brave"],
        _ => vec![browser],
    }
}

fn open_url(url: &str, browser: Option<&str>) -> Result<(), ActionError> {
    crate::check_url(url)?;
    let Some(browser) = browser.map(str::trim).filter(|b| !b.is_empty()) else {
        return spawn_detached(Command::new("xdg-open").arg(url)).map_err(|e| spawn_error("xdg-open", &e).into());
    };
    let candidates = browser_candidates(browser);
    let program = candidates
        .iter()
        .find(|p| which(p).is_some())
        .ok_or_else(|| format!("{browser} is not installed (looked for {})", candidates.join(", ")))?;
    spawn_detached(Command::new(program).arg(url)).map_err(|e| spawn_error(program, &e).into())
}

fn launch(program: &str, args: &[String]) -> Result<(), ActionError> {
    match spawn_detached(Command::new(program).args(args)) {
        Ok(()) => Ok(()),
        // Maybe a desktop file id such as `org.gnome.Nautilus`.
        Err(e) if e.kind() == io::ErrorKind::NotFound && !program.contains('/') => {
            run_status("gtk-launch", std::iter::once(program).chain(args.iter().map(String::as_str)))
                .map_err(|gtk| format!("cannot launch {program}: not found on PATH, and {gtk}").into())
        }
        Err(e) => Err(format!("cannot launch {program}: {e}").into()),
    }
}

/// Window switching uses `wmctrl`/`xdotool`, i.e. X11. On Wayland those only
/// see XWayland windows (compositors do not let clients raise other apps'
/// windows), so for native Wayland apps this falls back to launching, and
/// most apps then activate their existing window themselves.
fn focus_or_launch(program: &str, args: &[String]) -> Result<(), ActionError> {
    if has_display() && which("wmctrl").is_some() {
        if let Some(id) = run_capture("wmctrl", ["-lx"]).ok().and_then(|list| find_window(&list, program)) {
            let active = which("xdotool")
                .and_then(|_| run_capture("xdotool", ["getactivewindow"]).ok())
                .and_then(|s| s.trim().parse::<u64>().ok());
            if active.is_some() && active == parse_window_id(&id) {
                return run_quiet("xdotool", ["windowminimize", &id]).map_err(Into::into);
            }
            return run_quiet("wmctrl", ["-i", "-a", &id]).map_err(Into::into);
        }
    }
    launch(program, args)
}

/// Find the first window in `wmctrl -lx` output (`id desktop instance.Class
/// host title`) whose WM_CLASS matches the program's name.
fn find_window(wmctrl_lx: &str, program: &str) -> Option<String> {
    let base = program.rsplit('/').next().unwrap_or(program).to_ascii_lowercase();
    let base = base.strip_suffix(".desktop").unwrap_or(&base);
    // google-chrome-stable runs as class "Google-chrome".
    let name = ["-stable", "-beta", "-dev"].iter().find_map(|s| base.strip_suffix(s)).unwrap_or(base);
    if name.is_empty() {
        return None;
    }
    wmctrl_lx.lines().find_map(|line| {
        let mut cols = line.split_whitespace();
        let id = cols.next()?;
        let class = cols.nth(1)?.to_ascii_lowercase();
        // Instance and class may themselves contain dots (org.gnome.Nautilus).
        let matches = class == name || class.starts_with(&format!("{name}.")) || class.ends_with(&format!(".{name}"));
        matches.then(|| id.to_string())
    })
}

fn parse_window_id(id: &str) -> Option<u64> {
    match id.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => id.parse().ok(),
    }
}

fn lock() -> Result<(), ActionError> {
    let Err(logind) = run_quiet("loginctl", ["lock-session"]) else { return Ok(()) };
    run_status("xdg-screensaver", ["lock"]).map_err(|e| format!("could not lock the session: {logind}; {e}").into())
}

// ------------------------------------------------------------ focused app

/// PID of the focused window's process.
///
/// X11 windows, XWayland ones included, publish it as `_NET_WM_PID`. Wayland
/// has no portable way to ask which window has focus: on KDE `kdotool` asks
/// KWin; elsewhere only XWayland windows are seen, so a native Wayland app in
/// front gets no per-app profile (or, if the compositor leaves the X hint
/// stale, the last X11 app's).
fn active_window_pid() -> Option<u32> {
    let pid_from = |program: &str, args: &[&str]| parse_pid(&run_capture(program, args).ok()?);
    if is_wayland() && current_desktop().contains("KDE") {
        if let Some(pid) = pid_from("kdotool", &["getactivewindow", "getwindowpid"]) {
            return Some(pid);
        }
    }
    if !has_display() {
        return None;
    }
    if let Some(pid) = pid_from("xdotool", &["getactivewindow", "getwindowpid"]) {
        return Some(pid);
    }
    let window = parse_xprop_window(&run_capture("xprop", ["-root", "_NET_ACTIVE_WINDOW"]).ok()?)?;
    parse_xprop_pid(&run_capture("xprop", ["-id", &window, "_NET_WM_PID"]).ok()?)
}

/// A pid printed on a line of its own (`xdotool`/`kdotool getwindowpid`).
/// Pid 1 never owns a window here: it is what a containerised app reports
/// from inside its own pid namespace.
fn parse_pid(out: &str) -> Option<u32> {
    out.lines().rev().find_map(|l| l.trim().parse().ok()).filter(|&pid| pid > 1)
}

/// `xprop -root _NET_ACTIVE_WINDOW`: `_NET_ACTIVE_WINDOW(WINDOW): window id #
/// 0x3a00007` (0x0 when nothing has focus).
fn parse_xprop_window(out: &str) -> Option<String> {
    let id = out.split("window id #").nth(1)?.split([',', ' ', '\n']).find(|s| !s.is_empty())?;
    (parse_window_id(id)? != 0).then(|| id.to_string())
}

/// `xprop -id <window> _NET_WM_PID`: `_NET_WM_PID(CARDINAL) = 12345`.
fn parse_xprop_pid(out: &str) -> Option<u32> {
    parse_pid(out.split_once('=')?.1)
}

/// Executable name of a process: the target of `/proc/<pid>/exe`, or its
/// `comm` when that link is hidden (another user's or a non-dumpable
/// process). Kernel threads and vanished processes give `None`, which also
/// covers sandboxed apps (Flatpak) whose pid is only valid in their own pid
/// namespace and so usually names a kernel thread on the host.
fn process_name(pid: u32) -> Option<String> {
    let dir = PathBuf::from(format!("/proc/{pid}"));
    match fs::read_link(dir.join("exe")) {
        Ok(target) => exe_basename(&target),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            let comm = fs::read_to_string(dir.join("comm")).ok()?;
            Some(comm.trim().to_string()).filter(|c| !c.is_empty())
        }
        Err(_) => None,
    }
}

/// `/usr/lib/firefox/firefox` → `firefox`. The link gains ` (deleted)` when
/// an update replaced the binary while it was running.
fn exe_basename(target: &Path) -> Option<String> {
    let name = target.file_name()?.to_str()?;
    let name = name.strip_suffix(" (deleted)").unwrap_or(name);
    (!name.is_empty()).then(|| name.to_string())
}

// --------------------------------------------------- desktop integration

fn current_desktop() -> String {
    env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_uppercase()
}

pub fn conflicts(config: &Config) -> Vec<Conflict> {
    conflicts_for(&current_desktop(), config, uinput_writable(), &missing_helpers(config))
}

/// Helper programs that the user's bindings rely on but aren't installed,
/// with what they're for. Fix permissions installs them.
fn missing_helpers(config: &Config) -> Vec<(&'static str, &'static str)> {
    let bindings: Vec<_> = config.bindings.iter().chain(config.profiles.iter().flat_map(|p| &p.bindings)).collect();
    let mut out = Vec::new();
    let x11 = has_display();
    if x11 && which("wmctrl").is_none() && bindings.iter().any(|b| matches!(b.action, Action::FocusOrLaunch { .. })) {
        out.push(("wmctrl", "switching to apps"));
    }
    if x11 && !config.profiles.is_empty() && which("xdotool").is_none() && which("xprop").is_none() {
        out.push(("xdotool", "per-app profiles"));
    }
    out
}

fn conflicts_for(desktop: &str, config: &Config, uinput_ok: bool, missing: &[(&str, &str)]) -> Vec<Conflict> {
    // Per-app profile bindings count too: they fire whenever their app is in front.
    let bindings = || {
        config.bindings.iter().chain(config.profiles.iter().flat_map(|p| &p.bindings)).filter(|b| !b.action.is_none())
    };
    let bound = |pred: &dyn Fn(Trigger) -> bool| bindings().any(|b| pred(b.trigger));
    let mut out = Vec::new();
    if desktop.contains("GNOME") && bound(&|t| matches!(t, Trigger::Swipe(3, _) | Trigger::Drag(3))) {
        out.push(Conflict {
            message: "GNOME uses three-finger swipes for workspaces and the overview, so it will react too.".into(),
            fix: "Use 4- or 5-finger bindings instead, or install a GNOME Shell extension that turns off \
                  the built-in gestures (for example \"Disable Gestures\" or \"Gesture Improvements\")."
                .into(),
            // GNOME Settings has no switch for these gestures.
            can_open_settings: false,
            can_fix: false,
        });
    }
    if desktop.contains("KDE") && bound(&|t| matches!(t, Trigger::Swipe(4, _))) {
        out.push(Conflict {
            message:
                "KDE Plasma uses four-finger swipes for the overview and switching desktops, so it will react too."
                    .into(),
            fix: "Use 3- or 5-finger bindings instead, or turn off the four-finger gestures in \
                  System Settings › Touchpad / Gestures."
                .into(),
            can_open_settings: true,
            can_fix: false,
        });
    }
    // Only bindings that synthesise input need uinput; volume, apps and links don't.
    let needs_uinput =
        bindings().any(|b| matches!(b.action, Action::Keys { .. } | Action::Media { .. } | Action::Drag));
    if !uinput_ok && needs_uinput {
        out.push(Conflict {
            message: format!(
                "Pady cannot write to {UINPUT_PATH}, so keyboard shortcuts, media keys and three-finger drag will not work."
            ),
            fix: UINPUT_FIX.into(),
            can_open_settings: false,
            can_fix: true,
        });
    }
    if !missing.is_empty() {
        let list: Vec<String> = missing.iter().map(|(tool, why)| format!("{tool} ({why})")).collect();
        out.push(Conflict {
            message: format!("Some of your gestures need programs that aren't installed: {}.", list.join(", ")),
            fix: "Click Fix permissions to install them along with Pady's permissions (one password prompt).".into(),
            can_open_settings: false,
            can_fix: true,
        });
    }
    out
}

/// Install Pady's udev rule (and load uinput at boot) as root via pkexec,
/// apply it to the running session, and install any missing helper programs
/// with the distribution's package manager, all behind one password prompt.
pub fn fix_permissions() -> Result<(), ActionError> {
    let script = fix_permissions_script();
    let status = Command::new("pkexec").args(["sh", "-c", &script]).status().map_err(|e| {
        format!("could not ask for administrator rights (pkexec: {e}); run this as root instead:\n{script}")
    })?;
    match status.code() {
        Some(0) => Ok(()),
        Some(126) => Err("the administrator password prompt was dismissed".into()),
        Some(127) => Err("polkit could not authorise this (is a polkit authentication agent running?)".into()),
        _ => Err(format!("installing the udev rule failed ({status})").into()),
    }
}

fn fix_permissions_script() -> String {
    format!(
        "set -e\n\
         mkdir -p /etc/udev/rules.d /etc/modules-load.d\n\
         cat > /etc/udev/rules.d/70-pady.rules <<'PADY_RULES'\n{UDEV_RULES}PADY_RULES\n\
         echo uinput > /etc/modules-load.d/pady.conf\n\
         modprobe uinput || true\n\
         udevadm control --reload-rules\n\
         udevadm trigger --subsystem-match=input --action=change\n\
         udevadm trigger --subsystem-match=misc --sysname-match=uinput --action=change\n\
         {HELPERS_SCRIPT}"
    )
}

pub fn open_touchpad_settings() -> Result<(), ActionError> {
    let desktop = current_desktop();
    if desktop.contains("GNOME") {
        return spawn_detached(Command::new("gnome-control-center").arg("mouse"))
            .map_err(|e| spawn_error("gnome-control-center", &e).into());
    }
    if desktop.contains("KDE") {
        // Plasma 6 ships `systemsettings`, Plasma 5 `systemsettings5`.
        let program = ["systemsettings", "systemsettings5"].into_iter().find(|p| which(p).is_some());
        return match program {
            Some(p) => spawn_detached(Command::new(p).arg("kcm_touchpad")).map_err(|e| spawn_error(p, &e).into()),
            None => Err("System Settings (systemsettings) is not installed".into()),
        };
    }
    let name = if desktop.is_empty() { "unknown" } else { desktop.as_str() };
    Err(format!("Pady does not know where the touchpad settings are on this desktop ({name})").into())
}

/// `$XDG_CONFIG_HOME/autostart/pady.desktop` (default `~/.config`), the XDG
/// autostart location every major desktop honours.
fn autostart_file() -> Option<PathBuf> {
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").filter(|h| !h.is_empty()).map(|h| PathBuf::from(h).join(".config")))?;
    Some(config.join("autostart").join("pady.desktop"))
}

pub fn autostart_enabled() -> bool {
    autostart_file().is_some_and(|p| p.is_file())
}

pub fn set_autostart(enabled: bool, exe: &Path) -> Result<(), ActionError> {
    let path = autostart_file().ok_or("cannot find the config directory (neither XDG_CONFIG_HOME nor HOME is set)")?;
    if enabled {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        fs::write(&path, desktop_entry(exe)).map_err(|e| format!("cannot write {}: {e}", path.display()).into())
    } else {
        match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                Err(format!("cannot remove {}: {e}", path.display()).into())
            }
            _ => Ok(()),
        }
    }
}

fn desktop_entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Pady\n\
         Comment=Touchpad gestures for your PC\n\
         Exec={} --hidden\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        quote_exec_arg(&exe.to_string_lossy())
    )
}

/// Quote one `Exec=` argument per the Desktop Entry spec: inside double
/// quotes, `"` `` ` `` `$` `\` are backslash-escaped, and then the whole
/// value is string-escaped again (`\` becomes `\\`). `%` starts a field code
/// so it is doubled.
fn quote_exec_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    for c in arg.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '\\' => out.push_str("\\\\\\\\"),
            '%' => out.push_str("%%"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pady_core::{Binding, Direction, Profile};
    use std::collections::HashSet;

    #[test]
    fn every_key_maps_to_a_distinct_evdev_key() {
        let mapped: HashSet<EvKey> = Key::ALL.iter().map(|&k| ev_key(k)).collect();
        assert_eq!(mapped.len(), Key::ALL.len());
        for k in MODIFIER_KEYS.iter().chain(&MEDIA_KEYS) {
            assert!(!mapped.contains(k));
        }
    }

    #[test]
    fn every_key_has_an_x_keysym() {
        for &k in Key::ALL {
            let sym = x_keysym(k);
            assert!(sym.chars().all(|c| c.is_ascii_alphanumeric()), "{sym}");
        }
        assert_eq!(xdotool_combo(&"ctrl+alt+pgup".parse().unwrap()), "ctrl+alt+Prior");
        assert_eq!(xdotool_combo(&"win".parse().unwrap()), "super");
    }

    #[test]
    fn combos_press_then_release_in_reverse() {
        let seq = combo_sequence(&"ctrl+shift+t".parse().unwrap());
        use EvKey as K;
        assert_eq!(
            seq,
            vec![
                (K::KEY_LEFTCTRL, true),
                (K::KEY_LEFTSHIFT, true),
                (K::KEY_T, true),
                (K::KEY_T, false),
                (K::KEY_LEFTSHIFT, false),
                (K::KEY_LEFTCTRL, false),
            ]
        );
        let seq = combo_sequence(&"super".parse().unwrap());
        assert_eq!(seq, vec![(K::KEY_LEFTMETA, true), (K::KEY_LEFTMETA, false)]);
        assert_eq!(ydotool_args(&seq), vec!["key", "125:1", "125:0"]);
    }

    #[test]
    fn audio_arguments() {
        assert_eq!(AudioTool::Wpctl.volume_args(5), vec!["set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", "5%+"]);
        assert_eq!(AudioTool::Wpctl.volume_args(-5).last().unwrap(), "5%-");
        assert_eq!(AudioTool::Pactl.volume_args(-10), vec!["set-sink-volume", "@DEFAULT_SINK@", "-10%"]);
        assert_eq!(AudioTool::Amixer.volume_args(3), vec!["-q", "sset", "Master", "3%+"]);
        assert_eq!(AudioTool::Pactl.mute_args(), vec!["set-sink-mute", "@DEFAULT_SINK@", "toggle"]);
    }

    #[test]
    fn backlight_math() {
        assert_eq!(backlight_value(50, 100, 10), 60);
        assert_eq!(backlight_value(50, 100, -10), 40);
        assert_eq!(backlight_value(95, 100, 10), 100);
        assert_eq!(backlight_value(5, 100, -10), 1, "never fully off");
        assert_eq!(backlight_value(3, 7, 5), 4, "coarse panels move at least one level");
        assert_eq!(backlight_value(37_500, 120_000, 5), 43_500);
        assert_eq!(backlight_value(10, 0, 5), 1);
        assert!(backlight_priority("firmware") < backlight_priority("raw"));
    }

    #[test]
    fn browsers() {
        assert_eq!(browser_candidates("Chrome")[0], "google-chrome-stable");
        assert_eq!(browser_candidates("edge"), vec!["microsoft-edge-stable", "microsoft-edge"]);
        assert_eq!(browser_candidates("vivaldi"), vec!["vivaldi"]);
    }

    #[test]
    fn wmctrl_window_lookup() {
        let list = "0x02a00003  0 gnome-terminal-server.Gnome-terminal  host  Terminal\n\
                    0x03a00007  0 Navigator.firefox     host Mozilla Firefox\n\
                    0x04000001  1 google-chrome.Google-chrome  host New Tab - Google Chrome\n\
                    0x05000002 -1 org.gnome.Nautilus.Org.gnome.Nautilus  host Files\n";
        assert_eq!(find_window(list, "/usr/bin/firefox").as_deref(), Some("0x03a00007"));
        assert_eq!(find_window(list, "google-chrome-stable").as_deref(), Some("0x04000001"));
        assert_eq!(find_window(list, "org.gnome.Nautilus.desktop").as_deref(), Some("0x05000002"));
        assert_eq!(find_window(list, "gnome-terminal").as_deref(), Some("0x02a00003"));
        assert_eq!(find_window(list, "code"), None);
        assert_eq!(find_window("garbage", "firefox"), None);
        assert_eq!(parse_window_id("0x03a00007"), Some(0x03a0_0007));
        assert_eq!(parse_window_id("60817415"), Some(60_817_415));
    }

    #[test]
    fn desktop_conflicts() {
        let mut cfg = Config { bindings: Vec::new(), ..Default::default() };
        assert!(conflicts_for("UBUNTU:GNOME", &cfg, true, &[]).is_empty());
        assert!(conflicts_for("XFCE", &cfg, false, &[]).is_empty(), "no binding needs uinput yet");
        cfg.set_binding(Trigger::Swipe(3, Direction::Left), Action::Keys { combo: "alt+left".into() }, false);
        let c = conflicts_for("UBUNTU:GNOME", &cfg, true, &[]);
        assert_eq!(c.len(), 1);
        assert!(c[0].message.contains("GNOME"));
        assert!(conflicts_for("KDE", &cfg, true, &[]).is_empty());
        cfg.set_binding(Trigger::Swipe(4, Direction::Up), Action::Lock, false);
        let c = conflicts_for("KDE", &cfg, false, &[]);
        assert_eq!(c.len(), 2);
        assert!(c[0].can_open_settings);
        assert!(c[1].can_fix && c[1].fix.contains("Fix permissions"));
        assert!(conflicts_for("XFCE", &cfg, true, &[]).is_empty());
    }

    #[test]
    fn missing_helpers_become_a_fixable_conflict() {
        let cfg = Config { bindings: Vec::new(), ..Default::default() };
        let c = conflicts_for("XFCE", &cfg, true, &[("wmctrl", "switching to apps")]);
        assert_eq!(c.len(), 1);
        assert!(c[0].can_fix && c[0].message.contains("wmctrl (switching to apps)"));
    }

    #[test]
    fn fix_permissions_script_installs_the_packaged_rule() {
        let script = fix_permissions_script();
        assert!(script.starts_with(
            "set -e
"
        ));
        assert!(script.contains("TAG+=\"uaccess\""), "embeds packaging/linux/70-pady.rules");
        // The heredoc terminator must sit on a line of its own.
        assert!(script.contains(
            "
PADY_RULES
"
        ));
        assert!(script.contains("> /etc/udev/rules.d/70-pady.rules"));
        assert!(script.contains("udevadm trigger --subsystem-match=input"));
        assert!(script.contains("apt-get install -y $missing"), "installs helpers too");
    }

    #[test]
    fn profile_bindings_conflict_too() {
        let mut cfg = Config { bindings: Vec::new(), ..Default::default() };
        cfg.profiles.push(Profile {
            name: "Browser".into(),
            apps: vec!["firefox".into()],
            bindings: vec![
                Binding { trigger: Trigger::Swipe(3, Direction::Left), action: Action::None, repeat: false },
                Binding { trigger: Trigger::Tap(4), action: Action::Keys { combo: "ctrl+w".into() }, repeat: false },
            ],
        });
        // The `none` swipe only switches a gesture off; the shortcut needs uinput.
        assert!(conflicts_for("GNOME", &cfg, true, &[]).is_empty());
        let c = conflicts_for("GNOME", &cfg, false, &[]);
        assert_eq!(c.len(), 1);
        assert!(c[0].message.contains("/dev/uinput"));
        cfg.profiles[0].bindings[0].action = Action::Keys { combo: "alt+left".into() };
        assert_eq!(conflicts_for("GNOME", &cfg, true, &[]).len(), 1);
    }

    #[test]
    fn volume_read_back() {
        assert_eq!(parse_wpctl_volume("Volume: 0.45\n"), Some(Level::volume(45.0, false)));
        assert_eq!(parse_wpctl_volume("Volume: 0.30 [MUTED]\n"), Some(Level::volume(30.0, true)));
        assert_eq!(parse_wpctl_volume("Volume: 1.20").map(|l| l.percent), Some(100));
        assert_eq!(parse_wpctl_volume("Volume: 0,5").map(|l| l.percent), Some(50));
        assert_eq!(parse_wpctl_volume("Couldn't get default node"), None);

        let pactl = "Volume: front-left: 26214 /  40% / -23.88 dB,   front-right: 32768 /  50% / -18.06 dB\n        \
                     balance 0.10\n";
        assert_eq!(parse_pactl_volume(pactl), Some(45.0));
        assert_eq!(parse_pactl_volume("Volume: mono: 65536 / 100% / 0.00 dB"), Some(100.0));
        assert_eq!(parse_pactl_volume("Failure: No such entity"), None);
        assert_eq!(parse_pactl_mute("Mute: yes\n"), Some(true));
        assert_eq!(parse_pactl_mute("Mute: no"), Some(false));
        assert_eq!(parse_pactl_mute("Stumm: ja"), None);

        let amixer = "Simple mixer control 'Master',0\n  Capabilities: pvolume pswitch pswitch-joined\n  \
                      Playback channels: Front Left - Front Right\n  Limits: Playback 0 - 87\n  Mono:\n  \
                      Front Left: Playback 39 [45%] [-36.00dB] [on]\n  \
                      Front Right: Playback 39 [45%] [-36.00dB] [on]\n";
        assert_eq!(parse_amixer(amixer), Some(Level::volume(45.0, false)));
        assert_eq!(parse_amixer(&amixer.replace("[on]", "[off]")), Some(Level::volume(45.0, true)));
        assert_eq!(parse_amixer("Simple mixer control 'Master',0\n  Capabilities: pswitch\n"), None);
    }

    #[test]
    fn mute_arguments() {
        assert_eq!(AudioTool::Wpctl.mute_args(), vec!["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
        assert_eq!(AudioTool::Wpctl.unmute_args(), vec!["set-mute", "@DEFAULT_AUDIO_SINK@", "0"]);
        assert_eq!(AudioTool::Pactl.unmute_args(), vec!["set-sink-mute", "@DEFAULT_SINK@", "0"]);
        assert_eq!(AudioTool::Amixer.unmute_args(), vec!["-q", "sset", "Master", "unmute"]);
    }

    #[test]
    fn brightness_read_back() {
        assert_eq!(parse_brightnessctl("intel_backlight,backlight,12000,50%,24000\n"), Some(50.0));
        let amd = parse_brightnessctl("amdgpu_bl0,backlight,128,50%,255").unwrap();
        assert!((amd - 50.196).abs() < 0.01, "{amd}");
        assert_eq!(parse_brightnessctl("odd,backlight,x,30%,0"), Some(30.0));
        assert_eq!(parse_brightnessctl("Device 'intel_backlight' of class 'backlight':"), None);
        assert_eq!(parse_ddcutil_vcp("VCP 10 C 50 100\n"), Some((50, 100)));
        assert_eq!(parse_ddcutil_vcp("VCP 10 ERR"), None);
        assert_eq!(backlight_percent(12_000, 24_000), 50.0);
    }

    #[test]
    fn focused_app_lookup() {
        assert_eq!(parse_pid("12345\n"), Some(12345));
        assert_eq!(parse_pid("{c2a5}\n4242\n"), Some(4242));
        assert_eq!(parse_pid("1\n"), None);
        assert_eq!(parse_pid(""), None);
        assert_eq!(
            parse_xprop_window("_NET_ACTIVE_WINDOW(WINDOW): window id # 0x3a00007\n").as_deref(),
            Some("0x3a00007")
        );
        assert_eq!(
            parse_xprop_window("_NET_ACTIVE_WINDOW(WINDOW): window id # 0x3a00007, 0x0").as_deref(),
            Some("0x3a00007")
        );
        assert_eq!(parse_xprop_window("_NET_ACTIVE_WINDOW(WINDOW): window id # 0x0"), None);
        assert_eq!(parse_xprop_window("_NET_ACTIVE_WINDOW:  not found."), None);
        assert_eq!(parse_xprop_pid("_NET_WM_PID(CARDINAL) = 4321\n"), Some(4321));
        assert_eq!(parse_xprop_pid("_NET_WM_PID:  not found.\n"), None);
        assert_eq!(exe_basename(Path::new("/usr/share/code/code")).as_deref(), Some("code"));
        assert_eq!(exe_basename(Path::new("/usr/lib/firefox/firefox (deleted)")).as_deref(), Some("firefox"));
        assert_eq!(exe_basename(Path::new("/")), None);
    }

    #[test]
    fn autostart_entry_quotes_the_path() {
        let entry = desktop_entry(Path::new("/opt/My Apps/pady"));
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=\"/opt/My Apps/pady\" --hidden\n"));
        assert!(entry.contains("\nX-GNOME-Autostart-enabled=true\n"));
        assert_eq!(quote_exec_arg("a\"b$c%d\\e"), r#""a\\"b\\$c%%d\\\\e""#);
    }
}
