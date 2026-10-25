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
use pady_core::{Action, ActionError, Config, Executor, Key, KeyCombo, MediaKey, Modifiers, Trigger};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use std::{env, fs, io};

const UINPUT_PATH: &str = "/dev/uinput";
const UINPUT_FIX: &str = "Create /etc/udev/rules.d/60-pady-uinput.rules containing \
    KERNEL==\"uinput\", GROUP=\"input\", MODE=\"0660\", add yourself to the input group \
    (sudo usermod -aG input $USER), then reboot (or log out and back in).";
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

    fn media(&mut self, key: MediaKey) -> Result<(), ActionError> {
        let uinput_err = match self.device() {
            Ok(dev) => return emit_keys(dev, &[(media_key(key), true), (media_key(key), false)]).map_err(Into::into),
            Err(e) => e,
        };
        let player =
            |cmd: &str| run_quiet("playerctl", [cmd]).map_err(|e| format!("{uinput_err} (fallback: {e})").into());
        match key {
            MediaKey::PlayPause => player("play-pause"),
            MediaKey::Next => player("next"),
            MediaKey::Previous => player("previous"),
            MediaKey::Stop => player("stop"),
            MediaKey::VolumeUp => self.volume(5),
            MediaKey::VolumeDown => self.volume(-5),
            MediaKey::Mute => self.mute(),
        }
    }

    fn volume(&mut self, step: i32) -> Result<(), ActionError> {
        if step == 0 {
            return Ok(());
        }
        self.audio_command(|tool| tool.volume_args(step))
            .map_err(|e| format!("could not change the volume: {e}").into())
    }

    fn mute(&mut self) -> Result<(), ActionError> {
        self.audio_command(AudioTool::mute_args).map_err(|e| format!("could not toggle mute: {e}").into())
    }

    /// Run the remembered audio tool, or probe them all in order.
    fn audio_command(&mut self, args: impl Fn(AudioTool) -> Vec<String>) -> Result<(), String> {
        let mut order: Vec<AudioTool> = self.audio.into_iter().collect();
        order.extend(AudioTool::ALL.iter().filter(|&&t| Some(t) != self.audio));
        let mut errors = Vec::new();
        for tool in order {
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
}

impl Executor for LinuxExecutor {
    fn run(&mut self, action: &Action) -> Result<(), ActionError> {
        match action {
            // Drag is driven through mouse_button / mouse_move.
            Action::None | Action::Drag => Ok(()),
            Action::Keys { combo } => {
                let parsed: KeyCombo = combo.parse().map_err(|e| format!("invalid shortcut `{combo}`: {e}"))?;
                self.send_combo(&parsed)
            }
            Action::Volume { step } => self.volume(*step),
            Action::Mute => self.mute(),
            Action::Brightness { step } => brightness(*step),
            Action::Media { key } => self.media(*key),
            Action::OpenUrl { url, browser } => open_url(url, browser.as_deref()),
            Action::Launch { program, args } => launch(program, args),
            Action::FocusOrLaunch { program, args } => focus_or_launch(program, args),
            Action::Run { command } => spawn_detached(Command::new("sh").arg("-c").arg(command))
                .map_err(|e| format!("cannot run `{command}`: {e}").into()),
            Action::Lock => lock(),
        }
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

fn is_x11() -> bool {
    env::var_os("DISPLAY").is_some_and(|d| !d.is_empty())
        && env::var_os("WAYLAND_DISPLAY").is_none()
        && env::var("XDG_SESSION_TYPE").map_or(true, |t| t != "wayland")
}

// ------------------------------------------------------------- processes

/// Run a short-lived helper and wait for it. The error is one line saying
/// why, for messages that list everything that was tried. Only for tools
/// that do not leave children behind holding the stderr pipe.
fn run_quiet<I, S>(program: &str, args: I) -> Result<(), String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out =
        Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
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

/// Stdout of a short-lived helper, if it ran successfully.
fn run_output<I, S>(program: &str, args: I) -> Option<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
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
        let args: &[&str] = match self {
            AudioTool::Wpctl => &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"],
            AudioTool::Pactl => &["set-sink-mute", "@DEFAULT_SINK@", "toggle"],
            AudioTool::Amixer => &["-q", "sset", "Master", "toggle"],
        };
        args.iter().map(|s| s.to_string()).collect()
    }
}

fn brightness(step: i32) -> Result<(), ActionError> {
    if step == 0 {
        return Ok(());
    }
    let n = step.unsigned_abs();
    let sign = if step > 0 { "+" } else { "-" };
    let mut tried = Vec::new();
    match run_quiet("brightnessctl", ["set", &format!("{n}%{sign}")]) {
        Ok(()) => return Ok(()),
        Err(e) => tried.push(e),
    }
    match sysfs_backlight(step) {
        Ok(()) => return Ok(()),
        Err(e) => tried.push(e),
    }
    // External monitors over DDC/CI (VCP feature 0x10 is brightness).
    match run_quiet("ddcutil", ["setvcp", "10", sign, &n.to_string()]) {
        Ok(()) => return Ok(()),
        Err(e) => tried.push(e),
    }
    Err(format!("could not change brightness: {}", tried.join("; ")).into())
}

/// Change the laptop backlight directly, or through logind (which lets the
/// session owner set it without root) when the sysfs file is not writable.
fn sysfs_backlight(step: i32) -> Result<(), String> {
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
    let value = backlight_value(read("brightness")?, read("max_brightness")?, step);
    let file = path.join("brightness");
    match fs::write(&file, value.to_string()) {
        Ok(()) => Ok(()),
        Err(write_err) => run_quiet(
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
        .map_err(|e| format!("cannot write {} ({write_err}) and logind refused ({e})", file.display())),
    }
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
    let has_display = env::var_os("DISPLAY").is_some_and(|d| !d.is_empty());
    if has_display && which("wmctrl").is_some() {
        if let Some(id) = run_output("wmctrl", ["-lx"]).and_then(|list| find_window(&list, program)) {
            let active = which("xdotool")
                .and_then(|_| run_output("xdotool", ["getactivewindow"]))
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

// --------------------------------------------------- desktop integration

fn current_desktop() -> String {
    env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_uppercase()
}

pub fn conflicts(config: &Config) -> Vec<Conflict> {
    conflicts_for(&current_desktop(), config, uinput_writable())
}

fn conflicts_for(desktop: &str, config: &Config, uinput_ok: bool) -> Vec<Conflict> {
    let bound = |pred: &dyn Fn(Trigger) -> bool| config.bindings.iter().any(|b| !b.action.is_none() && pred(b.trigger));
    let mut out = Vec::new();
    if desktop.contains("GNOME") && bound(&|t| matches!(t, Trigger::Swipe(3, _) | Trigger::Drag(3))) {
        out.push(Conflict {
            message: "GNOME uses three-finger swipes for workspaces and the overview, so it will react too.".into(),
            fix: "Use 4- or 5-finger bindings instead, or install a GNOME Shell extension that turns off \
                  the built-in gestures (for example \"Disable Gestures\" or \"Gesture Improvements\")."
                .into(),
            // GNOME Settings has no switch for these gestures.
            can_open_settings: false,
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
        });
    }
    // Only bindings that synthesise input need uinput; volume, apps and links don't.
    let needs_uinput =
        config.bindings.iter().any(|b| matches!(b.action, Action::Keys { .. } | Action::Media { .. } | Action::Drag));
    if !uinput_ok && needs_uinput {
        out.push(Conflict {
            message: format!(
                "Pady cannot write to {UINPUT_PATH}, so keyboard shortcuts, media keys and three-finger drag will not work."
            ),
            fix: UINPUT_FIX.into(),
            can_open_settings: false,
        });
    }
    out
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
    use pady_core::Direction;
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
        assert!(conflicts_for("UBUNTU:GNOME", &cfg, true).is_empty());
        assert!(conflicts_for("XFCE", &cfg, false).is_empty(), "no binding needs uinput yet");
        cfg.set_binding(Trigger::Swipe(3, Direction::Left), Action::Keys { combo: "alt+left".into() }, false);
        let c = conflicts_for("UBUNTU:GNOME", &cfg, true);
        assert_eq!(c.len(), 1);
        assert!(c[0].message.contains("GNOME"));
        assert!(conflicts_for("KDE", &cfg, true).is_empty());
        cfg.set_binding(Trigger::Swipe(4, Direction::Up), Action::Lock, false);
        let c = conflicts_for("KDE", &cfg, false);
        assert_eq!(c.len(), 2);
        assert!(c[0].can_open_settings);
        assert!(c[1].fix.contains("KERNEL==\"uinput\""));
        assert!(conflicts_for("XFCE", &cfg, true).is_empty());
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
