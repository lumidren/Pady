//! Windows executor.
//!
//! * keys, media keys and the mouse go through `SendInput`;
//! * volume uses the Core Audio endpoint API for exact steps;
//! * brightness uses WMI (laptop panels) and falls back to DDC/CI
//!   (external monitors);
//! * apps and links go through `ShellExecuteW`, which resolves App Paths, so
//!   `chrome.exe` works without a full path.

use crate::Conflict;
use pady_core::{Action, ActionError, Config, Direction, Executor, Key, KeyCombo, Level, MediaKey, Trigger};
use std::ffi::c_void;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::time::{Duration, Instant};
use windows::core::{w, BSTR, HSTRING, PCWSTR, VARIANT};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, SetMonitorBrightness, PHYSICAL_MONITOR,
};
use windows::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, MAX_PATH, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE,
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
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, GW_OWNER, SW_MINIMIZE, SW_RESTORE,
    SW_SHOWNORMAL, WS_EX_TOOLWINDOW,
};

pub struct WindowsExecutor {
    com_ready: bool,
    wmi: Option<IWbemServices>,
    /// WMI reports the old level for a moment after a change, so remember
    /// what we set to make quick repeated steps accumulate correctly.
    last_brightness: Option<(u32, Instant)>,
}

impl WindowsExecutor {
    pub fn new() -> Self {
        // Runs on the action thread, which owns COM for its whole life.
        let com_ready = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
        WindowsExecutor { com_ready, wmi: None, last_brightness: None }
    }
}

impl Default for WindowsExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor for WindowsExecutor {
    fn run(&mut self, action: &Action) -> Result<Option<Level>, ActionError> {
        match action {
            Action::Volume { step } => return self.change_volume(*step).map(Some),
            Action::Mute => return self.toggle_mute().map(Some),
            Action::Brightness { step } => return self.change_brightness(*step).map(Some),
            _ => {}
        }
        match action {
            Action::None | Action::Drag | Action::Volume { .. } | Action::Mute | Action::Brightness { .. } => Ok(()),
            Action::Keys { combo } => send_combo(&combo.parse::<KeyCombo>()?),
            Action::Media { key } => tap_vk(media_vk(*key)),
            Action::OpenUrl { url, browser } => match browser.as_deref().filter(|b| !b.trim().is_empty()) {
                Some(b) => shell_open(&browser_exe(b), Some(&quote_arg(url))),
                None => shell_open(url, None),
            },
            Action::Launch { program, args } => shell_open(program, join_args(args).as_deref()),
            Action::FocusOrLaunch { program, args } => {
                if focus_or_minimize(program) {
                    Ok(())
                } else {
                    shell_open(program, join_args(args).as_deref())
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
        let hwnd = unsafe { GetForegroundWindow() };
        (!hwnd.0.is_null()).then(|| process_exe(hwnd)).flatten()
    }

    fn mouse_button(&mut self, down: bool) -> Result<(), ActionError> {
        let flags = if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP };
        send(&[mouse_input(0, 0, flags)])
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
// Volume (Core Audio)

impl WindowsExecutor {
    fn endpoint_volume(&self) -> Result<IAudioEndpointVolume, ActionError> {
        if !self.com_ready {
            return Err("COM is not available on the action thread".into());
        }
        unsafe {
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let speaker = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
            Ok(speaker.Activate(CLSCTX_ALL, None)?)
        }
    }

    fn change_volume(&mut self, step: i32) -> Result<Level, ActionError> {
        let vol = self.endpoint_volume()?;
        unsafe {
            let current = vol.GetMasterVolumeLevelScalar()? * 100.0;
            // Snap to the step grid so 4% steps land on 0, 4, 8… not 37, 41…
            let next = (((current + step as f32) / step.unsigned_abs().max(1) as f32).round()
                * step.unsigned_abs().max(1) as f32)
                .clamp(0.0, 100.0);
            vol.SetMasterVolumeLevelScalar(next / 100.0, std::ptr::null())?;
            let mut muted = vol.GetMute()?.as_bool();
            if step > 0 && muted {
                vol.SetMute(false, std::ptr::null())?;
                muted = false;
            }
            Ok(Level::volume(next, muted))
        }
    }

    fn toggle_mute(&mut self) -> Result<Level, ActionError> {
        let vol = self.endpoint_volume()?;
        unsafe {
            let muted = !vol.GetMute()?.as_bool();
            vol.SetMute(muted, std::ptr::null())?;
            Ok(Level::volume(vol.GetMasterVolumeLevelScalar()? * 100.0, muted))
        }
    }
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
        let next = (current as i32 + step).clamp(0, 100) as u32;
        unsafe {
            let instance = wmi_first(&wmi, "SELECT * FROM WmiMonitorBrightnessMethods WHERE Active=TRUE")?;
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
            params.Put(w!("Brightness"), 0, &VARIANT::from(next as u8), 0)?;
            wmi.ExecMethod(
                &path,
                &BSTR::from("WmiSetBrightness"),
                WBEM_GENERIC_FLAG_TYPE(0),
                None,
                &params,
                None,
                None,
            )?;
        }
        self.last_brightness = Some((next, Instant::now()));
        Ok(next)
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

fn quote_arg(arg: &str) -> String {
    if arg.contains([' ', '\t', '"']) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
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
fn exe_name(program: &str) -> String {
    let base = Path::new(program.trim().trim_matches('"'))
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
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
        let taps = config.binding(Trigger::Tap(fingers)).is_some() && enabled(tap);
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
}
