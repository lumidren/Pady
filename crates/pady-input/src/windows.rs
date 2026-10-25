//! Windows Precision Touchpad backend using Raw Input.
//!
//! A hidden message-only window registers for the touchpad top-level
//! collection (usage page 0x0D Digitizer, usage 0x05 Touch Pad) with
//! `RIDEV_INPUTSINK`, so reports arrive even while another app has focus.
//! Each report is decoded with the device's preparsed HID data.
//!
//! Raw Input is read-only: Windows keeps running its own gestures. Users
//! should set the 3/4-finger gestures they rebind to "Nothing" in Settings
//! (the app detects and explains this).

use crate::hid::{pad_size_mm, AxisCaps, FrameAssembler, SlotReading};
use crate::{DeviceInfo, InputError, InputHandle};
use crossbeam_channel::{bounded, Sender};
use pady_core::{now_ms, Contact, TouchFrame};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::{Arc, Mutex};
use windows::core::{w, PCWSTR};
use windows::Win32::Devices::HumanInterfaceDevice::*;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const PAGE_GENERIC: u16 = 0x01;
const PAGE_DIGITIZER: u16 = 0x0D;
const USAGE_X: u16 = 0x30;
const USAGE_Y: u16 = 0x31;
const USAGE_TOUCHPAD: u16 = 0x05;
const USAGE_TIP_SWITCH: u16 = 0x42;
const USAGE_CONFIDENCE: u16 = 0x47;
const USAGE_CONTACT_ID: u16 = 0x51;
const USAGE_CONTACT_COUNT: u16 = 0x54;

pub(crate) fn start(frames: Sender<TouchFrame>) -> Result<InputHandle, InputError> {
    let devices = Arc::new(Mutex::new(Vec::new()));
    let (ready_tx, ready_rx) = bounded::<Result<(), InputError>>(1);
    let thread_devices = devices.clone();
    std::thread::Builder::new()
        .name("pady-touchpad".into())
        .spawn(move || run(frames, thread_devices, ready_tx))
        .map_err(|e| InputError::Os(e.to_string()))?;
    ready_rx.recv().map_err(|_| InputError::Os("touchpad thread exited during startup".into()))??;
    Ok(InputHandle::new(devices))
}

struct State {
    frames: Sender<TouchFrame>,
    devices: Arc<Mutex<Vec<DeviceInfo>>>,
    parsers: HashMap<isize, Option<Touchpad>>,
    buf: Vec<u64>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn run(frames: Sender<TouchFrame>, devices: Arc<Mutex<Vec<DeviceInfo>>>, ready: Sender<Result<(), InputError>>) {
    let mut parsers = HashMap::new();
    for handle in enumerate_touchpads() {
        if let Some(pad) = Touchpad::open(handle) {
            devices.lock().unwrap_or_else(|e| e.into_inner()).push(pad.info.clone());
            parsers.insert(handle.0 as isize, Some(pad));
        }
    }
    if parsers.is_empty() {
        let _ = ready.send(Err(InputError::NoTouchpad));
        return;
    }
    STATE.with(|s| *s.borrow_mut() = Some(State { frames, devices, parsers, buf: Vec::new() }));

    let hwnd = match unsafe { create_sink_window() } {
        Ok(h) => h,
        Err(e) => {
            let _ = ready.send(Err(InputError::Os(format!("could not register for touchpad input: {e}"))));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    log::info!("listening for precision touchpad input");

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
        let _ = DestroyWindow(hwnd);
    }
}

unsafe fn create_sink_window() -> windows::core::Result<HWND> {
    let instance = GetModuleHandleW(None)?;
    let class = w!("PadyRawInputSink");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: instance.into(),
        lpszClassName: class,
        ..Default::default()
    };
    RegisterClassW(&wc);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        PCWSTR::null(),
        WINDOW_STYLE(0),
        0,
        0,
        0,
        0,
        HWND_MESSAGE,
        None,
        instance,
        None,
    )?;
    let device = RAWINPUTDEVICE {
        usUsagePage: PAGE_DIGITIZER,
        usUsage: USAGE_TOUCHPAD,
        dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
        hwndTarget: hwnd,
    };
    RegisterRawInputDevices(&[device], size_of::<RAWINPUTDEVICE>() as u32)?;
    Ok(hwnd)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_INPUT => {
            STATE.with(|s| {
                if let Some(state) = s.borrow_mut().as_mut() {
                    on_input(state, HRAWINPUT(lparam.0 as *mut c_void));
                }
            });
            // WM_INPUT must still reach DefWindowProc so the system can clean up.
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_INPUT_DEVICE_CHANGE => {
            let handle = HANDLE(lparam.0 as *mut c_void);
            STATE.with(|s| {
                if let Some(state) = s.borrow_mut().as_mut() {
                    on_device_change(state, handle, wparam.0 as u32 == GIDC_ARRIVAL);
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn on_device_change(state: &mut State, handle: HANDLE, arrived: bool) {
    let key = handle.0 as isize;
    let mut devices = state.devices.lock().unwrap_or_else(|e| e.into_inner());
    if arrived {
        if state.parsers.contains_key(&key) {
            return;
        }
        let pad = Touchpad::open(handle);
        if let Some(p) = &pad {
            log::info!("touchpad connected: {}", p.info.name);
            devices.push(p.info.clone());
        }
        state.parsers.insert(key, pad);
    } else if let Some(Some(pad)) = state.parsers.remove(&key) {
        log::info!("touchpad removed: {}", pad.info.name);
        if let Some(i) = devices.iter().position(|d| *d == pad.info) {
            devices.remove(i);
        }
    }
}

fn on_input(state: &mut State, input: HRAWINPUT) {
    let header = size_of::<RAWINPUTHEADER>() as u32;
    let mut size = 0u32;
    unsafe {
        GetRawInputData(input, RID_INPUT, None, &mut size, header);
    }
    if size == 0 {
        return;
    }
    // u64 backing keeps RAWINPUT suitably aligned.
    state.buf.resize((size as usize).div_ceil(8), 0);
    let ptr = state.buf.as_mut_ptr() as *mut c_void;
    let read = unsafe { GetRawInputData(input, RID_INPUT, Some(ptr), &mut size, header) };
    if read == u32::MAX || read < header {
        return;
    }
    let raw = unsafe { &*(ptr as *const RAWINPUT) };
    if raw.header.dwType != RIM_TYPEHID.0 {
        return;
    }
    let key = raw.header.hDevice.0 as isize;
    let pad = state.parsers.entry(key).or_insert_with(|| {
        let pad = Touchpad::open(raw.header.hDevice);
        if let Some(p) = &pad {
            state.devices.lock().unwrap_or_else(|e| e.into_inner()).push(p.info.clone());
        }
        pad
    });
    let Some(pad) = pad.as_mut() else { return };

    let hid = unsafe { &raw.data.hid };
    let (report_size, count) = (hid.dwSizeHid as usize, hid.dwCount as usize);
    let total = report_size * count;
    // bRawData is a flexible array member that runs past the struct.
    let data_ptr = hid.bRawData.as_ptr() as *mut u8;
    let available = read as usize - (data_ptr as usize - ptr as usize);
    if report_size == 0 || total > available {
        return;
    }
    let data = unsafe { std::slice::from_raw_parts_mut(data_ptr, total) };
    for report in data.chunks_exact_mut(report_size) {
        if let Some(frame) = pad.parse(report) {
            if state.frames.send(frame).is_err() {
                unsafe { PostQuitMessage(0) };
                return;
            }
        }
    }
}

fn enumerate_touchpads() -> Vec<HANDLE> {
    let entry = size_of::<RAWINPUTDEVICELIST>() as u32;
    let mut count = 0u32;
    unsafe {
        if GetRawInputDeviceList(None, &mut count, entry) == u32::MAX || count == 0 {
            return Vec::new();
        }
        let mut list = vec![RAWINPUTDEVICELIST::default(); count as usize];
        let n = GetRawInputDeviceList(Some(list.as_mut_ptr()), &mut count, entry);
        if n == u32::MAX {
            return Vec::new();
        }
        list.truncate(n as usize);
        list.into_iter().filter(|d| d.dwType == RIM_TYPEHID && is_touchpad(d.hDevice)).map(|d| d.hDevice).collect()
    }
}

fn is_touchpad(device: HANDLE) -> bool {
    let mut info = RID_DEVICE_INFO { cbSize: size_of::<RID_DEVICE_INFO>() as u32, ..Default::default() };
    let mut size = info.cbSize;
    let ok =
        unsafe { GetRawInputDeviceInfoW(device, RIDI_DEVICEINFO, Some(&mut info as *mut _ as *mut c_void), &mut size) };
    if ok == u32::MAX || info.dwType != RIM_TYPEHID {
        return false;
    }
    let hid = unsafe { info.Anonymous.hid };
    hid.usUsagePage == PAGE_DIGITIZER && hid.usUsage == USAGE_TOUCHPAD
}

/// One finger slot = one HID link collection holding X and Y.
struct Finger {
    link: u16,
    has_id: bool,
}

struct Touchpad {
    info: DeviceInfo,
    preparsed: Vec<u64>,
    fingers: Vec<Finger>,
    x: AxisCaps,
    y: AxisCaps,
    contact_count_link: Option<u16>,
    has_confidence: bool,
    assembler: FrameAssembler,
    usages: Vec<u16>,
}

impl Touchpad {
    /// Read the report descriptor; `None` if this isn't a usable touchpad.
    fn open(device: HANDLE) -> Option<Touchpad> {
        let preparsed = preparsed_data(device)?;
        let pp = PHIDP_PREPARSED_DATA(preparsed.as_ptr() as isize);

        let mut caps = HIDP_CAPS::default();
        if unsafe { HidP_GetCaps(pp, &mut caps) } != HIDP_STATUS_SUCCESS {
            return None;
        }

        let mut n = caps.NumberInputValueCaps;
        let mut values = vec![HIDP_VALUE_CAPS::default(); n as usize];
        if unsafe { HidP_GetValueCaps(HidP_Input, values.as_mut_ptr(), &mut n, pp) } != HIDP_STATUS_SUCCESS {
            return None;
        }
        values.truncate(n as usize);

        let mut n = caps.NumberInputButtonCaps;
        let mut buttons = vec![HIDP_BUTTON_CAPS::default(); n as usize];
        if n > 0 && unsafe { HidP_GetButtonCaps(HidP_Input, buttons.as_mut_ptr(), &mut n, pp) } != HIDP_STATUS_SUCCESS {
            return None;
        }
        buttons.truncate(n as usize);

        let mut fingers: Vec<Finger> = Vec::new();
        let (mut x, mut y, mut contact_count_link) = (None, None, None);
        for v in &values {
            let usage = value_usage(v);
            match (v.UsagePage, usage) {
                (PAGE_GENERIC, USAGE_X) => {
                    x.get_or_insert_with(|| axis(v));
                    if !fingers.iter().any(|f| f.link == v.LinkCollection) {
                        fingers.push(Finger { link: v.LinkCollection, has_id: false });
                    }
                }
                (PAGE_GENERIC, USAGE_Y) => {
                    y.get_or_insert_with(|| axis(v));
                }
                (PAGE_DIGITIZER, USAGE_CONTACT_COUNT) => contact_count_link = Some(v.LinkCollection),
                _ => {}
            }
        }
        for v in values.iter().filter(|v| v.UsagePage == PAGE_DIGITIZER && value_usage(v) == USAGE_CONTACT_ID) {
            if let Some(f) = fingers.iter_mut().find(|f| f.link == v.LinkCollection) {
                f.has_id = true;
            }
        }
        let has_confidence = buttons.iter().any(|b| b.UsagePage == PAGE_DIGITIZER && button_has(b, USAGE_CONFIDENCE));
        let (x, y) = (x?, y?);
        if fingers.is_empty() {
            return None;
        }
        let (width_mm, height_mm) = pad_size_mm(&x, &y);
        let info = DeviceInfo {
            // Many pads report a bus name ("HIDI2C Device"); keep it as a hint only.
            name: match product_name(device) {
                Some(n) if n.to_ascii_lowercase().contains("touch") => n,
                Some(n) => format!("Precision Touchpad ({n})"),
                None => "Precision Touchpad".into(),
            },
            width_mm,
            height_mm,
            max_contacts: fingers.len().min(u8::MAX as usize) as u8,
        };
        log::info!(
            "touchpad `{}`: {:.0}×{:.0} mm, {} finger slots{}",
            info.name,
            width_mm,
            height_mm,
            fingers.len(),
            if has_confidence { ", palm rejection" } else { "" }
        );
        let max_usages = unsafe { HidP_MaxUsageListLength(HidP_Input, PAGE_DIGITIZER, pp) }.max(16) as usize;
        Some(Touchpad {
            info,
            preparsed,
            fingers,
            x,
            y,
            contact_count_link,
            has_confidence,
            assembler: FrameAssembler::default(),
            usages: vec![0; max_usages],
        })
    }

    fn parse(&mut self, report: &mut [u8]) -> Option<TouchFrame> {
        let pp = PHIDP_PREPARSED_DATA(self.preparsed.as_ptr() as isize);
        let value = |page: u16, link: u16, usage: u16, report: &[u8]| -> Option<u32> {
            let mut v = 0u32;
            let status = unsafe { HidP_GetUsageValue(HidP_Input, page, link, usage, &mut v, pp, report) };
            (status == HIDP_STATUS_SUCCESS).then_some(v)
        };

        let contact_count = self
            .contact_count_link
            .and_then(|link| value(PAGE_DIGITIZER, link, USAGE_CONTACT_COUNT, report))
            .unwrap_or(0);

        let mut slots = Vec::with_capacity(self.fingers.len());
        for (i, f) in self.fingers.iter().enumerate() {
            let mut len = self.usages.len() as u32;
            let status = unsafe {
                HidP_GetUsages(HidP_Input, PAGE_DIGITIZER, f.link, self.usages.as_mut_ptr(), &mut len, pp, report)
            };
            let pressed = if status == HIDP_STATUS_SUCCESS { &self.usages[..len as usize] } else { &[][..] };
            let x = value(PAGE_GENERIC, f.link, USAGE_X, report).unwrap_or(0);
            let y = value(PAGE_GENERIC, f.link, USAGE_Y, report).unwrap_or(0);
            let id = if f.has_id { value(PAGE_DIGITIZER, f.link, USAGE_CONTACT_ID, report) } else { None };
            slots.push(SlotReading {
                id: id.unwrap_or(i as u32),
                x: sign_extend(x, &self.x),
                y: sign_extend(y, &self.y),
                tip: pressed.contains(&USAGE_TIP_SWITCH),
                confident: !self.has_confidence || pressed.contains(&USAGE_CONFIDENCE),
            });
        }

        let touching = self.assembler.push(contact_count, &slots)?;
        let (w, h) = (self.info.width_mm, self.info.height_mm);
        Some(TouchFrame {
            time_ms: now_ms(),
            contacts: touching
                .iter()
                .map(|s| Contact { id: s.id, x: self.x.to_mm(s.x, w), y: self.y.to_mm(s.y, h) })
                .collect(),
            width_mm: w,
            height_mm: h,
        })
    }
}

/// HidP_GetUsageValue returns the raw bits; logical ranges can be signed.
fn sign_extend(v: u32, caps: &AxisCaps) -> i32 {
    if caps.logical_min < 0 {
        v as i32
    } else {
        v.min(i32::MAX as u32) as i32
    }
}

fn axis(v: &HIDP_VALUE_CAPS) -> AxisCaps {
    AxisCaps {
        logical_min: v.LogicalMin,
        logical_max: v.LogicalMax,
        physical_min: v.PhysicalMin,
        physical_max: v.PhysicalMax,
        units: v.Units,
        unit_exp: v.UnitsExp,
    }
}

fn value_usage(v: &HIDP_VALUE_CAPS) -> u16 {
    unsafe {
        if v.IsRange.as_bool() {
            v.Anonymous.Range.UsageMin
        } else {
            v.Anonymous.NotRange.Usage
        }
    }
}

fn button_has(b: &HIDP_BUTTON_CAPS, usage: u16) -> bool {
    unsafe {
        if b.IsRange.as_bool() {
            (b.Anonymous.Range.UsageMin..=b.Anonymous.Range.UsageMax).contains(&usage)
        } else {
            b.Anonymous.NotRange.Usage == usage
        }
    }
}

fn preparsed_data(device: HANDLE) -> Option<Vec<u64>> {
    let mut size = 0u32;
    unsafe {
        GetRawInputDeviceInfoW(device, RIDI_PREPARSEDDATA, None, &mut size);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u64; (size as usize).div_ceil(8)];
        let n = GetRawInputDeviceInfoW(device, RIDI_PREPARSEDDATA, Some(buf.as_mut_ptr() as *mut c_void), &mut size);
        (n != u32::MAX && n > 0).then_some(buf)
    }
}

/// Friendly product string ("Synaptics HID Touchpad"), via the device path.
fn product_name(device: HANDLE) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    let mut len = 0u32;
    unsafe {
        GetRawInputDeviceInfoW(device, RIDI_DEVICENAME, None, &mut len);
        if len == 0 {
            return None;
        }
        let mut path = vec![0u16; len as usize + 1];
        if GetRawInputDeviceInfoW(device, RIDI_DEVICENAME, Some(path.as_mut_ptr() as *mut c_void), &mut len) == u32::MAX
        {
            return None;
        }
        // Zero access rights are enough for HidD_* string queries.
        let file = CreateFileW(
            PCWSTR(path.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()?;
        let mut name = [0u16; 128];
        let ok = HidD_GetProductString(file, name.as_mut_ptr() as *mut c_void, (name.len() * 2) as u32);
        let _ = CloseHandle(file);
        if !ok.as_bool() {
            return None;
        }
        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        let s = String::from_utf16_lossy(&name[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}
