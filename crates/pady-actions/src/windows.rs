//! Windows executor.
//!
//! * keys, media keys and the mouse go through `SendInput`;
//! * volume uses the volume keys, so Windows shows its own volume slider;
//! * brightness uses WMI (laptop panels) and falls back to DDC/CI
//!   (external monitors);
//! * apps and links go through `ShellExecuteW`, which resolves App Paths, so
//!   `chrome.exe` works without a full path.

use crate::Conflict;
use pady_core::{Action, ActionError, Config, Direction, Executor, Key, KeyCombo, Level, MediaKey, Trigger};
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::{w, BSTR, HSTRING, PCWSTR, VARIANT};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, SetMonitorBrightness, PHYSICAL_MONITOR,
};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, MAX_PATH, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromWindow, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, EOAC_NONE,
    RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, CREATE_NO_WINDOW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, IWbemServices, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    WBEM_GENERIC_FLAG_TYPE, WBEM_INFINITE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{SHQueryUserNotificationState, ShellExecuteW, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextLengthW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, GW_OWNER,
    SW_MINIMIZE, SW_RESTORE, SW_SHOWNORMAL, WS_EX_TOOLWINDOW,
};

/// Whether a drag holds the left button right now, so a crash can let go
/// (see `release_held_button`).
static BUTTON_HELD: AtomicBool = AtomicBool::new(false);

/// The shell's last full-screen answer (it takes 17–50 ms to give one): the
/// window it was for, the answer, and when.
static FULLSCREEN: Mutex<Option<(isize, bool, Instant)>> = Mutex::new(None);
/// A background refresh of `FULLSCREEN` is running.
static FULLSCREEN_REFRESHING: AtomicBool = AtomicBool::new(false);

/// Release the left button if a drag holds it. Safe to call from a panic
/// hook: Windows doesn't let go of a button injected by a process that dies.
pub fn release_held_button() {
    if BUTTON_HELD.swap(false, Ordering::SeqCst) {
        let _ = send(&[mouse_input(0, 0, MOUSEEVENTF_LEFTUP)]);
    }
}

pub struct WindowsExecutor {
    com_ready: bool,
    wmi: Option<IWbemServices>,
    /// The panel's WmiSetBrightness target and a reusable argument object,
    /// looked up once: finding them took most of each step's time.
    brightness_method: Option<(BSTR, IWbemClassObject)>,
    /// WMI reports the old level for a moment after a change, so remember
    /// what we set to make quick repeated steps accumulate correctly.
    last_brightness: Option<(u32, Instant)>,
}

impl WindowsExecutor {
    pub fn new() -> Self {
        // Runs on the action thread, which owns COM for its whole life.
        let com_ready = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        let mut exec = WindowsExecutor { com_ready, wmi: None, brightness_method: None, last_brightness: None };
        // Connect to WMI now, while nothing waits: done on the first brightness
        // swipe instead, it held that step up by ~50 ms. Displays without WMI
        // brightness (external monitors) just fail here and use DDC/CI later.
        if let Ok(wmi) = exec.wmi().cloned() {
            let _ = exec.brightness_method(&wmi);
        }
        exec
    }
}

impl Default for WindowsExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor for WindowsExecutor {
    fn run(&mut self, action: &Action) -> Result<Option<Level>, ActionError> {
        if let Action::Brightness { step } = action {
            return self.change_brightness(*step).map(Some);
        }
        match action {
            Action::None | Action::Drag | Action::Translate | Action::Brightness { .. } => Ok(()),
            Action::Volume { step } => change_volume(*step),
            Action::Mute => tap_vk(VK_VOLUME_MUTE),
            Action::Keys { combo } => send_combo(&combo.parse::<KeyCombo>()?),
            Action::Media { key } => tap_vk(media_vk(*key)),
            Action::OpenUrl { url, browser } => {
                crate::check_url(url).and_then(|()| match browser.as_deref().filter(|b| !b.trim().is_empty()) {
                    Some(b) => shell_open(&browser_exe(b), Some(&quote_arg(url))),
                    None => shell_open(url, None),
                })
            }
            Action::Launch { program, args } => shell_open(&resolve_program(program), join_args(args).as_deref()),
            Action::FocusOrLaunch { program, args } => {
                if focus_or_minimize(program) {
                    Ok(())
                } else {
                    shell_open(&resolve_program(program), join_args(args).as_deref())
                }
            }
            Action::Run { command } => {
                std::process::Command::new("cmd").args(["/C", command]).creation_flags(CREATE_NO_WINDOW.0).spawn()?;
                Ok(())
            }
            Action::Lock => unsafe { LockWorkStation() }.map_err(Into::into),
        }
        .map(|()| None)
    }

    fn foreground_app(&mut self) -> Option<String> {
        foreground_app()
    }

    /// Only exclusive full-screen Direct3D games count. Full-screen video
    /// and presentations are left alone: people scrub volume there.
    ///
    ///
    /// This runs before every gesture, so it must be quick. The cheap window
    /// checks go first; the shell, which takes 17–50 ms to answer, is asked
    /// only about a window that covers its monitor. Its answer for that window
    /// is reused and refreshed in the background after 2 s, so only the first
    /// gesture in a newly full-screen window waits for it.
    fn fullscreen_game(&mut self) -> bool {
        let Some(hwnd) = foreground_fills_monitor() else { return false };
        let hwnd = hwnd.0 as isize;
        let known = *FULLSCREEN.lock().unwrap_or_else(|e| e.into_inner());
        match known {
            Some((window, answer, at)) if window == hwnd => {
                if at.elapsed() > Duration::from_secs(2) && !FULLSCREEN_REFRESHING.swap(true, Ordering::SeqCst) {
                    std::thread::spawn(move || {
                        ask_shell_fullscreen(hwnd);
                        FULLSCREEN_REFRESHING.store(false, Ordering::SeqCst);
                    });
                }
                answer
            }
            _ => ask_shell_fullscreen(hwnd),
        }
    }

    fn mouse_button(&mut self, down: bool) -> Result<(), ActionError> {
        let flags = if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP };
        send(&[mouse_input(0, 0, flags)])?;
        BUTTON_HELD.store(down, Ordering::SeqCst);
        Ok(())
    }

    fn mouse_move(&mut self, dx: i32, dy: i32) -> Result<(), ActionError> {
        send(&[mouse_input(dx, dy, MOUSEEVENTF_MOVE)])
    }
}

// ---------------------------------------------------------------------------
// Keyboard and mouse

fn send(inputs: &[INPUT]) -> Result<(), ActionError> {
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        // Usually UIPI: the foreground window belongs to an elevated app.
        Err("Windows blocked the simulated input (is an administrator window in front?)".into())
    }
}

fn key_input(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    let mut flags = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
    if is_extended(vk) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let scan = unsafe { MapVirtualKeyW(vk.0 as u32, MAPVK_VK_TO_VSC) } as u16;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn mouse_input(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn tap_vk(vk: VIRTUAL_KEY) -> Result<(), ActionError> {
    send(&[key_input(vk, false), key_input(vk, true)])
}

/// Press modifiers, tap the key, release in reverse order — all in one
/// `SendInput` call so no real keystroke can interleave.
/// Press a shortcut such as `ctrl+c` (outside the executor, from any thread).
pub(crate) fn press(combo: &str) -> Result<(), ActionError> {
    send_combo(&combo.parse::<KeyCombo>()?)
}

fn send_combo(combo: &KeyCombo) -> Result<(), ActionError> {
    let m = combo.modifiers;
    let mods: Vec<VIRTUAL_KEY> =
        [(m.ctrl, VK_LCONTROL), (m.alt, VK_LMENU), (m.shift, VK_LSHIFT), (m.super_key, VK_LWIN)]
            .into_iter()
            .filter_map(|(on, vk)| on.then_some(vk))
            .collect();
    let mut inputs: Vec<INPUT> = mods.iter().map(|&vk| key_input(vk, false)).collect();
    if let Some(key) = combo.key {
        let vk = key_vk(key);
        inputs.push(key_input(vk, false));
        inputs.push(key_input(vk, true));
    }
    inputs.extend(mods.iter().rev().map(|&vk| key_input(vk, true)));
    send(&inputs)
}

fn is_extended(vk: VIRTUAL_KEY) -> bool {
    matches!(
        vk,
        VK_INSERT
            | VK_DELETE
            | VK_HOME
            | VK_END
            | VK_PRIOR
            | VK_NEXT
            | VK_LEFT
            | VK_RIGHT
            | VK_UP
            | VK_DOWN
            | VK_LWIN
            | VK_RWIN
            | VK_RCONTROL
            | VK_RMENU
            | VK_SNAPSHOT
            | VK_MEDIA_PLAY_PAUSE
            | VK_MEDIA_NEXT_TRACK
            | VK_MEDIA_PREV_TRACK
            | VK_MEDIA_STOP
            | VK_VOLUME_UP
            | VK_VOLUME_DOWN
            | VK_VOLUME_MUTE
    )
}

fn media_vk(key: MediaKey) -> VIRTUAL_KEY {
    match key {
        MediaKey::PlayPause => VK_MEDIA_PLAY_PAUSE,
        MediaKey::Next => VK_MEDIA_NEXT_TRACK,
        MediaKey::Previous => VK_MEDIA_PREV_TRACK,
        MediaKey::Stop => VK_MEDIA_STOP,
        MediaKey::VolumeUp => VK_VOLUME_UP,
        MediaKey::VolumeDown => VK_VOLUME_DOWN,
        MediaKey::Mute => VK_VOLUME_MUTE,
    }
}

fn key_vk(key: Key) -> VIRTUAL_KEY {
    use Key::*;
    let letter = |c: u8| VIRTUAL_KEY(c as u16);
    match key {
        A => letter(b'A'),
        B => letter(b'B'),
        C => letter(b'C'),
        D => letter(b'D'),
        E => letter(b'E'),
        F => letter(b'F'),
        G => letter(b'G'),
        H => letter(b'H'),
        I => letter(b'I'),
        J => letter(b'J'),
        K => letter(b'K'),
        L => letter(b'L'),
        M => letter(b'M'),
        N => letter(b'N'),
        O => letter(b'O'),
        P => letter(b'P'),
        Q => letter(b'Q'),
        R => letter(b'R'),
        S => letter(b'S'),
        T => letter(b'T'),
        U => letter(b'U'),
        V => letter(b'V'),
        W => letter(b'W'),
        X => letter(b'X'),
        Y => letter(b'Y'),
        Z => letter(b'Z'),
        Num0 => letter(b'0'),
        Num1 => letter(b'1'),
        Num2 => letter(b'2'),
        Num3 => letter(b'3'),
        Num4 => letter(b'4'),
        Num5 => letter(b'5'),
        Num6 => letter(b'6'),
        Num7 => letter(b'7'),
        Num8 => letter(b'8'),
        Num9 => letter(b'9'),
        F1 => VK_F1,
        F2 => VK_F2,
        F3 => VK_F3,
        F4 => VK_F4,
        F5 => VK_F5,
        F6 => VK_F6,
        F7 => VK_F7,
        F8 => VK_F8,
        F9 => VK_F9,
        F10 => VK_F10,
        F11 => VK_F11,
        F12 => VK_F12,
        Enter => VK_RETURN,
        Escape => VK_ESCAPE,
        Tab => VK_TAB,
        Space => VK_SPACE,
        Backspace => VK_BACK,
        Delete => VK_DELETE,
        Insert => VK_INSERT,
        Home => VK_HOME,
        End => VK_END,
        PageUp => VK_PRIOR,
        PageDown => VK_NEXT,
        Up => VK_UP,
        Down => VK_DOWN,
        Left => VK_LEFT,
        Right => VK_RIGHT,
        PrintScreen => VK_SNAPSHOT,
        Minus => VK_OEM_MINUS,
        Equal => VK_OEM_PLUS,
        Comma => VK_OEM_COMMA,
        Period => VK_OEM_PERIOD,
        Slash => VK_OEM_2,
        Backslash => VK_OEM_5,
        Semicolon => VK_OEM_1,
        Quote => VK_OEM_7,
        BracketLeft => VK_OEM_4,
        BracketRight => VK_OEM_6,
        Grave => VK_OEM_3,
    }
}

// ---------------------------------------------------------------------------
// Volume

/// Volume goes through the real volume keys (2% each), so Windows shows its
/// own volume slider, exactly as with the keyboard, and unmutes on the way up.
fn change_volume(step: i32) -> Result<(), ActionError> {
    let vk = if step >= 0 { VK_VOLUME_UP } else { VK_VOLUME_DOWN };
    let presses = step.unsigned_abs().div_ceil(2).clamp(1, 50) as usize;
    let inputs: Vec<INPUT> = (0..presses).flat_map(|_| [key_input(vk, false), key_input(vk, true)]).collect();
    send(&inputs)
}

// ---------------------------------------------------------------------------
// Brightness (WMI for built-in panels, DDC/CI for external monitors)

impl WindowsExecutor {
    fn change_brightness(&mut self, step: i32) -> Result<Level, ActionError> {
        match self.wmi_brightness(step) {
            Ok(percent) => Ok(Level::brightness(percent as f32)),
            Err(wmi_err) => {
                if let Some(percent) = ddc_brightness(step) {
                    Ok(Level::brightness(percent))
                } else {
                    Err(format!(
                        "no display accepted a brightness change (WMI: {wmi_err}; DDC/CI: no compatible monitor)"
                    )
                    .into())
                }
            }
        }
    }

    fn wmi(&mut self) -> Result<&IWbemServices, ActionError> {
        if self.wmi.is_none() {
            if !self.com_ready {
                return Err("COM is not available on the action thread".into());
            }
            let services = unsafe {
                let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
                let services = locator.ConnectServer(
                    &BSTR::from("ROOT\\WMI"),
                    &BSTR::new(),
                    &BSTR::new(),
                    &BSTR::new(),
                    0,
                    &BSTR::new(),
                    None,
                )?;
                CoSetProxyBlanket(
                    &services,
                    10, // RPC_C_AUTHN_WINNT
                    0,  // RPC_C_AUTHZ_NONE
                    PCWSTR::null(),
                    RPC_C_AUTHN_LEVEL_CALL,
                    RPC_C_IMP_LEVEL_IMPERSONATE,
                    None,
                    EOAC_NONE,
                )?;
                services
            };
            self.wmi = Some(services);
        }
        Ok(self.wmi.as_ref().expect("just set"))
    }

    /// Returns the new brightness in percent.
    fn wmi_brightness(&mut self, step: i32) -> Result<u32, ActionError> {
        let recent = self.last_brightness.filter(|(_, at)| at.elapsed() < Duration::from_millis(1500)).map(|(v, _)| v);
        let wmi = self.wmi()?.clone();
        let current = match recent {
            Some(v) => v,
            None => {
                let obj = wmi_first(&wmi, "SELECT CurrentBrightness FROM WmiMonitorBrightness WHERE Active=TRUE")?;
                let mut value = VARIANT::default();
                unsafe { obj.Get(w!("CurrentBrightness"), 0, &mut value, None, None)? };
                u32::try_from(&value)?
            }
        };
        let next = (current as i32).saturating_add(step).clamp(0, 100) as u32;
        let (path, params) = self.brightness_method(&wmi)?;
        let set = unsafe {
            params.Put(w!("Brightness"), 0, &VARIANT::from(next as u8), 0).and_then(|()| {
                wmi.ExecMethod(
                    &path,
                    &BSTR::from("WmiSetBrightness"),
                    WBEM_GENERIC_FLAG_TYPE(0),
                    None,
                    &params,
                    None,
                    None,
                )
            })
        };
        if let Err(e) = set {
            // The panel may have changed (a dock, a driver update): look it up again next time.
            self.brightness_method = None;
            return Err(e.into());
        }
        self.last_brightness = Some((next, Instant::now()));
        Ok(next)
    }

    fn brightness_method(&mut self, wmi: &IWbemServices) -> Result<(BSTR, IWbemClassObject), ActionError> {
        if let Some(found) = &self.brightness_method {
            return Ok(found.clone());
        }
        let found = unsafe {
            let instance = wmi_first(wmi, "SELECT * FROM WmiMonitorBrightnessMethods WHERE Active=TRUE")?;
            let mut path = VARIANT::default();
            instance.Get(w!("__PATH"), 0, &mut path, None, None)?;
            let path = BSTR::try_from(&path)?;

            let mut class = None;
            wmi.GetObject(
                &BSTR::from("WmiMonitorBrightnessMethods"),
                WBEM_GENERIC_FLAG_TYPE(0),
                None,
                Some(&mut class),
                None,
            )?;
            let class = class.ok_or("WmiMonitorBrightnessMethods class missing")?;
            let mut signature = None;
            class.GetMethod(w!("WmiSetBrightness"), 0, &mut signature, std::ptr::null_mut())?;
            let params = signature.ok_or("WmiSetBrightness signature missing")?.SpawnInstance(0)?;
            // WMI maps CIM uint32 to VT_I4 and uint8 to VT_UI1.
            params.Put(w!("Timeout"), 0, &VARIANT::from(0i32), 0)?;
            (path, params)
        };
        self.brightness_method = Some(found.clone());
        Ok(found)
    }
}

fn wmi_first(wmi: &IWbemServices, query: &str) -> Result<IWbemClassObject, ActionError> {
    unsafe {
        let rows = wmi.ExecQuery(
            &BSTR::from("WQL"),
            &BSTR::from(query),
            WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
            None,
        )?;
        let mut row = [None];
        let mut returned = 0u32;
        rows.Next(WBEM_INFINITE, &mut row, &mut returned).ok()?;
        row[0].take().ok_or_else(|| "this display does not support WMI brightness".into())
    }
}

/// Adjust every DDC/CI monitor; returns the new level of the last one changed.
fn ddc_brightness(step: i32) -> Option<f32> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let list = &mut *(data.0 as *mut Vec<HMONITOR>);
        list.push(monitor);
        true.into()
    }
    let mut monitors: Vec<HMONITOR> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut monitors as *mut _ as isize));
    }
    let mut level = None;
    for monitor in monitors {
        unsafe {
            let mut count = 0u32;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count).is_err() || count == 0 {
                continue;
            }
            let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
            if GetPhysicalMonitorsFromHMONITOR(monitor, &mut physical).is_err() {
                continue;
            }
            for p in &physical {
                let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
                if GetMonitorBrightness(p.hPhysicalMonitor, &mut min, &mut cur, &mut max) == 0 || max <= min {
                    continue;
                }
                let span = (max - min) as f32;
                let next = (cur as f32 + span * step as f32 / 100.0).round().clamp(min as f32, max as f32) as u32;
                if SetMonitorBrightness(p.hPhysicalMonitor, next) != 0 {
                    level = Some((next - min) as f32 * 100.0 / span);
                }
            }
            let _ = DestroyPhysicalMonitors(&physical);
        }
    }
    level
}

// ---------------------------------------------------------------------------
// Apps, links, windows

fn browser_exe(name: &str) -> String {
    match name.trim().to_ascii_lowercase().as_str() {
        "chrome" | "google chrome" => "chrome.exe".into(),
        "edge" | "msedge" | "microsoft edge" => "msedge.exe".into(),
        "firefox" => "firefox.exe".into(),
        "brave" => "brave.exe".into(),
        "opera" => "opera.exe".into(),
        other => other.into(),
    }
}

/// One argument as programs split their command line back up
/// (`CommandLineToArgvW`): inside quotes, backslashes only need doubling
/// before a quote, the closing one included (`"C:\My Dir\\"`).
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        if c == '"' {
            out.push_str(&"\\".repeat(backslashes + 1));
        }
        backslashes = if c == '\\' { backslashes + 1 } else { 0 };
        out.push(c);
    }
    out.push_str(&"\\".repeat(backslashes));
    out.push('"');
    out
}

fn join_args(args: &[String]) -> Option<String> {
    (!args.is_empty()).then(|| args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" "))
}

fn shell_open(file: &str, params: Option<&str>) -> Result<(), ActionError> {
    let file_w = HSTRING::from(file);
    let params_w = params.map(HSTRING::from);
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &file_w,
            params_w.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a fake HINSTANCE: > 32 means success.
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err(format!("Windows could not open `{file}` (error {})", result.0 as isize).into())
    }
}

/// Bring the program's main window forward (or minimise it if it already is
/// in front). Returns false if no window of that program is open.
fn focus_or_minimize(program: &str) -> bool {
    let wanted = exe_name(program);
    let Some(hwnd) = find_window(&wanted) else { return false };
    unsafe {
        if GetForegroundWindow() == hwnd {
            let _ = ShowWindow(hwnd, SW_MINIMIZE);
            return true;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        // Windows only lets the process that received the last input event
        // take the foreground, so inject a harmless Alt press around the call.
        let _ = send(&[key_input(VK_MENU, false)]);
        let ok = SetForegroundWindow(hwnd).as_bool();
        let _ = send(&[key_input(VK_MENU, true)]);
        ok
    }
}

/// `C:\Apps\Code.exe` / `code` / `Code.EXE` → `code.exe`
/// A program given by name only ("Antigravity" or "Antigravity.lnk"), the way
/// you'd find it by typing in Start: look for a Start menu shortcut with that
/// name. Paths, URIs and names with another extension (`chrome.exe`, found
/// through App Paths) are used as they are.
fn resolve_program(program: &str) -> String {
    let p = program.trim().trim_matches('"');
    let lower = p.to_ascii_lowercase();
    let has_other_extension = Path::new(&lower).extension().is_some_and(|e| e != "lnk");
    if p.is_empty() || p.contains(['\\', '/', ':']) || has_other_extension {
        return p.to_string();
    }
    let wanted = lower.strip_suffix(".lnk").unwrap_or(&lower).to_string();
    // Searching the Start menu takes 20–90 ms, so remember what it found:
    // a shortcut while it still exists, a miss for a minute (an app may get installed).
    type Found = HashMap<String, (Option<PathBuf>, Instant)>;
    static FOUND: Mutex<Option<Found>> = Mutex::new(None);
    let mut found = FOUND.lock().unwrap_or_else(|e| e.into_inner());
    let known = found.get_or_insert_with(HashMap::new).get(&wanted).cloned();
    let shortcut = match known {
        Some((Some(path), _)) if path.exists() => Some(path),
        Some((None, at)) if at.elapsed() < Duration::from_secs(60) => None,
        _ => {
            let menus = [std::env::var_os("APPDATA"), std::env::var_os("ProgramData")];
            let path = menus
                .into_iter()
                .flatten()
                .map(|base| Path::new(&base).join(r"Microsoft\Windows\Start Menu\Programs"))
                .find_map(|dir| find_shortcut(&dir, &wanted, 3));
            found.get_or_insert_with(HashMap::new).insert(wanted, (path.clone(), Instant::now()));
            path
        }
    };
    shortcut.map(|path| path.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string())
}

/// A `.lnk` named `wanted` (case-insensitive) in `dir` or up to `depth`
/// folders below it.
fn find_shortcut(dir: &Path, wanted: &str, depth: u32) -> Option<std::path::PathBuf> {
    let entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    let is_match = |path: &Path| {
        path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
            && path.file_stem().is_some_and(|s| s.to_string_lossy().to_ascii_lowercase() == wanted)
    };
    if let Some(found) = entries.iter().map(|e| e.path()).find(|p| is_match(p)) {
        return Some(found);
    }
    if depth == 0 {
        return None;
    }
    entries.iter().map(|e| e.path()).filter(|p| p.is_dir()).find_map(|sub| find_shortcut(&sub, wanted, depth - 1))
}

fn exe_name(program: &str) -> String {
    let path = Path::new(program.trim().trim_matches('"'));
    // A shortcut usually carries its program's name: Antigravity.lnk → antigravity.exe.
    let is_shortcut = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
    let name = if is_shortcut { path.file_stem() } else { path.file_name() };
    let base = name.map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if base.ends_with(".exe") {
        base
    } else {
        format!("{base}.exe")
    }
}

fn find_window(exe: &str) -> Option<HWND> {
    struct Search<'a> {
        exe: &'a str,
        found: Option<HWND>,
    }
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> BOOL {
        let search = &mut *(data.0 as *mut Search);
        let is_app_window = IsWindowVisible(hwnd).as_bool()
            && GetWindow(hwnd, GW_OWNER).map_or(true, |o| o.0.is_null())
            && GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0
            && GetWindowTextLengthW(hwnd) > 0;
        if is_app_window && process_exe(hwnd).as_deref() == Some(search.exe) {
            search.found = Some(hwnd);
            return false.into(); // stop: EnumWindows goes front-to-back
        }
        true.into()
    }
    let mut search = Search { exe, found: None };
    unsafe {
        let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut _ as isize));
    }
    search.found
}

/// Asks the shell whether an exclusive full-screen Direct3D app runs, and
/// remembers the answer for `hwnd`.
fn ask_shell_fullscreen(hwnd: isize) -> bool {
    let answer = unsafe { SHQueryUserNotificationState() }.is_ok_and(|state| state == QUNS_RUNNING_D3D_FULL_SCREEN);
    *FULLSCREEN.lock().unwrap_or_else(|e| e.into_inner()) = Some((hwnd, answer, Instant::now()));
    answer
}

/// Executable of the window in front, e.g. `chrome.exe`. Needs no COM, so
/// any thread may call it.
pub fn foreground_app() -> Option<String> {
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.0.is_null()).then(|| process_exe(hwnd)).flatten()
}

/// Windows can keep reporting a full-screen Direct3D app after waking from
/// sleep or unlocking (the lock screen is one), which used to pause every
/// gesture until another window came to the front. So also require that the
/// window in front really covers its monitor and isn't part of the shell.
/// Returns that window if so.
fn foreground_fills_monitor() -> Option<HWND> {
    const SHELL: &[&str] = &[
        "explorer.exe",
        "lockapp.exe",
        "logonui.exe",
        "shellexperiencehost.exe",
        "startmenuexperiencehost.exe",
        "searchhost.exe",
    ];
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() || !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return None;
        }
        let m = info.rcMonitor;
        let fills = rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom;
        // The shell check opens the process, so it waits until the size matches.
        if !fills || process_exe(hwnd).is_some_and(|exe| SHELL.contains(&exe.to_ascii_lowercase().as_str())) {
            return None;
        }
        Some(hwnd)
    }
}

fn process_exe(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; MAX_PATH as usize * 2];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        ok.ok()?;
        Some(exe_name(&String::from_utf16_lossy(&buf[..len as usize])))
    }
}

// ---------------------------------------------------------------------------
// System integration

const TOUCHPAD_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\PrecisionTouchPad");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("Pady");

fn read_dword(key: PCWSTR, value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut _ as *mut c_void),
            Some(&mut size),
        )
    };
    status.is_ok().then_some(data)
}

/// Windows' own 3/4-finger gestures run alongside Pady (Raw Input can't
/// swallow them). Settings › Touchpad stores them under PrecisionTouchPad;
/// 0 means "Nothing", and a missing value means the default (enabled).
pub fn conflicts(config: &Config) -> Vec<Conflict> {
    let enabled = |name: PCWSTR| read_dword(TOUCHPAD_KEY, name).is_none_or(|v| v != 0);
    let mut out = Vec::new();
    for fingers in [3u8, 4] {
        let word = if fingers == 3 { "three" } else { "four" };
        let (slide, tap) = if fingers == 3 {
            (w!("ThreeFingerSlideEnabled"), w!("ThreeFingerTapEnabled"))
        } else {
            (w!("FourFingerSlideEnabled"), w!("FourFingerTapEnabled"))
        };
        let uses_motion = config.bindings.iter().any(|b| {
            matches!(b.trigger, Trigger::Drag(n) if n == fingers)
                || Direction::ALL.iter().any(|&d| b.trigger == Trigger::Swipe(fingers, d))
        });
        let swipes = uses_motion && enabled(slide);
        let taps = [Trigger::Tap(fingers), Trigger::DoubleTap(fingers), Trigger::TripleTap(fingers)]
            .into_iter()
            .any(|t| config.binding(t).is_some())
            && enabled(tap);
        out.extend(touchpad_conflict(word, swipes, taps));
    }
    out
}

/// One conflict per finger count, naming whichever of swipes/taps clash.
fn touchpad_conflict(word: &str, swipes: bool, taps: bool) -> Option<Conflict> {
    let (what, setting) = match (swipes, taps) {
        (true, true) => ("swipes and taps", "Swipes and Taps"),
        (true, false) => ("swipes", "Swipes"),
        (false, true) => ("taps", "Taps"),
        (false, false) => return None,
    };
    Some(Conflict {
        message: format!("Windows also reacts to {word}-finger {what}."),
        fix: format!("In Touchpad settings › {}-finger gestures, set {setting} to \"Nothing\".", capitalize(word)),
        can_open_settings: true,
        can_fix: false,
    })
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

pub fn open_touchpad_settings() -> Result<(), ActionError> {
    shell_open("ms-settings:devices-touchpad", None)
}

pub fn autostart_enabled() -> bool {
    let mut size = 0u32;
    let status =
        unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size)) };
    status.is_ok()
}

pub fn set_autostart(enabled: bool, exe: &Path) -> Result<(), ActionError> {
    unsafe {
        if enabled {
            let command = HSTRING::from(format!("\"{}\" --hidden", exe.display()));
            let bytes = (command.len() + 1) * 2;
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                RUN_VALUE,
                REG_SZ.0,
                Some(command.as_ptr() as *const c_void),
                bytes as u32,
            )
            .ok()?;
        } else {
            let status = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
            if status.is_err() && autostart_enabled() {
                status.ok()?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exe_names_are_normalised() {
        assert_eq!(exe_name("chrome"), "chrome.exe");
        assert_eq!(exe_name(r"C:\Program Files\Microsoft VS Code\Code.exe"), "code.exe");
        assert_eq!(exe_name("\"Spotify.EXE\""), "spotify.exe");
        assert_eq!(exe_name(r"C:\Start Menu\Programs\Antigravity.lnk"), "antigravity.exe");
        assert_eq!(exe_name("Antigravity.lnk"), "antigravity.exe");
    }

    #[test]
    fn programs_are_found_by_shortcut_name() {
        let dir = std::env::temp_dir().join(format!("pady-menu-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Some Folder")).unwrap();
        std::fs::write(dir.join("Some Folder").join("Antigravity IDE.lnk"), b"").unwrap();
        assert_eq!(
            find_shortcut(&dir, "antigravity ide", 3),
            Some(dir.join("Some Folder").join("Antigravity IDE.lnk"))
        );
        assert_eq!(find_shortcut(&dir, "nothing", 3), None);
        std::fs::remove_dir_all(&dir).unwrap();
        // Paths, URIs and other extensions are left alone.
        assert_eq!(resolve_program(r"C:\Apps\x.exe"), r"C:\Apps\x.exe");
        assert_eq!(resolve_program("ms-settings:display"), "ms-settings:display");
        assert_eq!(resolve_program("chrome.exe"), "chrome.exe");
    }

    #[test]
    fn every_key_has_a_virtual_key() {
        for &k in Key::ALL {
            assert_ne!(key_vk(k).0, 0, "{k:?}");
        }
    }

    #[test]
    fn arguments_with_spaces_are_quoted() {
        assert_eq!(join_args(&["--new-window".into(), "a b".into()]).unwrap(), "--new-window \"a b\"");
        assert_eq!(join_args(&[]), None);
    }

    #[test]
    fn touchpad_conflicts_merge_per_finger_count() {
        assert_eq!(touchpad_conflict("three", false, false), None);
        let c = touchpad_conflict("three", true, true).unwrap();
        assert_eq!(c.message, "Windows also reacts to three-finger swipes and taps.");
        assert!(c.fix.contains("Three-finger gestures, set Swipes and Taps"));
        assert!(touchpad_conflict("four", false, true).unwrap().message.ends_with("four-finger taps."));
    }

    #[test]
    fn browsers_map_to_app_paths() {
        assert_eq!(browser_exe("Chrome"), "chrome.exe");
        assert_eq!(browser_exe("edge"), "msedge.exe");
        assert_eq!(browser_exe("vivaldi.exe"), "vivaldi.exe");
    }

    /// How a program splits its command line back into arguments.
    fn split(command_line: &str) -> Vec<String> {
        use windows::Win32::UI::Shell::CommandLineToArgvW;
        let wide = HSTRING::from(command_line);
        let mut count = 0;
        let argv = unsafe { CommandLineToArgvW(&wide, &mut count) };
        assert!(!argv.is_null());
        (0..count as usize).map(|i| unsafe { (*argv.add(i)).to_string().unwrap() }).collect()
    }

    #[test]
    fn arguments_reach_the_program_exactly_as_written() {
        let tricky = [
            r"C:\My Dir\",
            r"C:\My Dir\\",
            r#"say "hi""#,
            r#"a\"b"#,
            r#"a\\"b c"#,
            "",
            " ",
            "\t",
            "--flag=\"x y\"",
            r"\\server\share\",
            "é 🙂",
        ];
        for arg in tricky {
            let args = vec![arg.to_string(), "--next".to_string()];
            let line = format!("prog.exe {}", join_args(&args).unwrap());
            assert_eq!(split(&line)[1..], args[..], "{line}");
        }
        let alphabet = ['a', ' ', '\t', '"', '\\', 'é', '-'];
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed as usize % n
        };
        for _ in 0..5_000 {
            let args: Vec<String> =
                (0..1 + next(4)).map(|_| (0..next(9)).map(|_| alphabet[next(alphabet.len())]).collect()).collect();
            let line = format!("prog.exe {}", join_args(&args).unwrap());
            assert_eq!(split(&line)[1..], args[..], "{line}");
        }
    }

    /// How long finding a window and a Start menu shortcut take here:
    /// `cargo test -p pady-actions --release lookup_timings -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lookup_timings() {
        for exe in ["chrome.exe", "spotify.exe", "nothing-like-this.exe"] {
            let start = Instant::now();
            let found = find_window(exe).is_some();
            println!("find_window({exe}) {:.2} ms, found {found}", start.elapsed().as_secs_f64() * 1000.0);
        }
        for name in ["Spotify", "Notepad", "Nothing Like This", "Spotify", "Nothing Like This"] {
            let start = Instant::now();
            let path = resolve_program(name);
            println!("resolve_program({name}) {:.2} ms → {path}", start.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
