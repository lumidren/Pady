//! The selected text, read through the clipboard (Windows).
//!
//! Windows has no "what's selected" API that works in every app, but every
//! app copies with Ctrl+C. So Pady saves the clipboard, presses Ctrl+C,
//! reads the text and puts the clipboard back as it was: text, pictures,
//! files and everything else held in memory. Pady's own writes are kept out
//! of Windows' clipboard history.

use pady_core::ActionError;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
};

const CF_UNICODETEXT: u32 = 13;
/// How long an app gets to answer Ctrl+C.
const COPY_WAIT: Duration = Duration::from_millis(600);
/// How long to keep trying while another app has the clipboard open.
const OPEN_WAIT: Duration = Duration::from_millis(300);
/// Formats bigger than this aren't saved (a huge picture would stall the gesture).
const MAX_SAVED: usize = 64 << 20;

/// Formats that hold GDI or private handles rather than memory, so their
/// bytes can't be copied and put back.
fn is_handle_format(f: u32) -> bool {
    matches!(f, 2 | 3 | 9 | 14 | 0x80 | 0x82 | 0x83 | 0x8E) || (0x200..=0x3FF).contains(&f)
}

/// The clipboard's contents, copied out so they can be put back.
#[derive(Default)]
pub(crate) struct Saved {
    formats: Vec<(u32, Vec<u8>)>,
}

/// A hidden window to own the clipboard: Windows refuses to take data from
/// a clipboard opened without one.
struct Owner(HWND);

impl Owner {
    fn new() -> Result<Owner, ActionError> {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Pady clipboard"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                None,
                None,
                None,
            )
        }?;
        Ok(Owner(hwnd))
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}

/// The clipboard, open until dropped.
struct Open;

impl Open {
    fn new(owner: Option<&Owner>) -> Result<Open, ActionError> {
        let start = Instant::now();
        loop {
            let hwnd = owner.map(|o| o.0).unwrap_or_default();
            if unsafe { OpenClipboard(hwnd) }.is_ok() {
                return Ok(Open);
            }
            if start.elapsed() > OPEN_WAIT {
                return Err("another app is holding the clipboard".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

fn bytes_of(handle: HANDLE) -> Option<Vec<u8>> {
    let mem = HGLOBAL(handle.0);
    let size = unsafe { GlobalSize(mem) };
    if size == 0 || size > MAX_SAVED {
        return None;
    }
    let ptr = unsafe { GlobalLock(mem) };
    if ptr.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, size) }.to_vec();
    let _ = unsafe { GlobalUnlock(mem) };
    Some(bytes)
}

fn put(format: u32, bytes: &[u8]) -> Result<(), ActionError> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let ptr = GlobalLock(mem);
        if ptr.is_null() {
            let _ = GlobalFree(mem);
            return Err("out of memory".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
        let _ = GlobalUnlock(mem);
        // The clipboard owns the memory once this succeeds.
        if let Err(e) = SetClipboardData(format, HANDLE(mem.0)) {
            let _ = GlobalFree(mem);
            return Err(e.into());
        }
    }
    Ok(())
}

/// Marks what Pady writes so Windows' clipboard history (Win+V) skips it.
fn keep_out_of_history() {
    let format = unsafe { RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")) };
    if format != 0 {
        let _ = put(format, &[0]);
    }
}

pub(crate) fn save() -> Result<Saved, ActionError> {
    let _open = Open::new(None)?;
    let mut saved = Saved::default();
    let mut format = 0;
    loop {
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        if is_handle_format(format) {
            continue;
        }
        if let Some(bytes) = unsafe { GetClipboardData(format) }.ok().and_then(bytes_of) {
            saved.formats.push((format, bytes));
        }
    }
    Ok(saved)
}

pub(crate) fn restore(saved: &Saved) -> Result<(), ActionError> {
    let owner = Owner::new()?;
    let _open = Open::new(Some(&owner))?;
    unsafe { EmptyClipboard() }?;
    for (format, bytes) in &saved.formats {
        let _ = put(*format, bytes);
    }
    if !saved.formats.is_empty() {
        keep_out_of_history();
    }
    Ok(())
}

fn read_text() -> Result<Option<String>, ActionError> {
    let _open = Open::new(None)?;
    let Some(bytes) = unsafe { GetClipboardData(CF_UNICODETEXT) }.ok().and_then(bytes_of) else { return Ok(None) };
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    Ok(Some(String::from_utf16_lossy(&units[..end])))
}

/// Put text on the clipboard. `history`: let Win+V remember it.
pub(crate) fn set_text(text: &str, history: bool) -> Result<(), ActionError> {
    let owner = Owner::new()?;
    let _open = Open::new(Some(&owner))?;
    unsafe { EmptyClipboard() }?;
    let mut units: Vec<u16> = text.encode_utf16().collect();
    units.push(0);
    let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
    put(CF_UNICODETEXT, &bytes)?;
    if !history {
        keep_out_of_history();
    }
    Ok(())
}

/// Wait for the clipboard to change from `before`; `false` if it didn't.
fn changed_since(before: u32, wait: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < wait {
        if unsafe { GetClipboardSequenceNumber() } != before {
            // The app empties the clipboard first, then writes: give it a moment.
            std::thread::sleep(Duration::from_millis(25));
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// The text selected in the app in front, or `None` if nothing is (or it
/// isn't text). `copy` presses Ctrl+C. The clipboard ends up as it was.
pub(crate) fn selected_text(copy: impl Fn() -> Result<(), ActionError>) -> Result<Option<String>, ActionError> {
    let saved = save()?;
    let before = unsafe { GetClipboardSequenceNumber() };
    copy()?;
    if !changed_since(before, COPY_WAIT) {
        return Ok(None);
    }
    let text = read_text();
    let restored = restore(&saved);
    let text = text?;
    restored?;
    Ok(text.filter(|t| !t.trim().is_empty()))
}

/// Type `text` over the selection by pasting it, then put the clipboard back.
pub(crate) fn paste_text(text: &str, paste: impl Fn() -> Result<(), ActionError>) -> Result<(), ActionError> {
    let saved = save()?;
    set_text(text, false)?;
    paste()?;
    // The app reads the clipboard when it handles Ctrl+V, a moment later.
    std::thread::sleep(Duration::from_millis(400));
    restore(&saved)
}
