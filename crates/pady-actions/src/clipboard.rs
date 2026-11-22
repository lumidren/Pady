//! The selected text, read through the clipboard (Windows).
//!
//! Windows has no "what's selected" API that works in every app, but every
//! app copies with Ctrl+C. So Pady saves the clipboard, presses Ctrl+C,
//! reads the text and puts the clipboard back as it was: text, pictures,
//! files and everything else held in memory. Pady's own writes are kept out
//! of Windows' clipboard history.
//!
//! With "Trim spaces when pasting" on, a paste gesture also comes here
//! first: one copied line with blanks around it goes back without them,
//! and everything else that was copied with it (the page's own formatting,
//! say) stays as it was. The text is compared in memory and nothing else.

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

const CF_TEXT: u32 = 1;
const CF_OEMTEXT: u32 = 7;
const CF_UNICODETEXT: u32 = 13;
const CF_LOCALE: u32 = 16;
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
/// isn't text). `copy` presses Ctrl+C; the app gets `wait` to answer it.
/// The clipboard ends up as it was.
pub(crate) fn selected_text(
    copy: impl Fn() -> Result<(), ActionError>,
    wait: Duration,
) -> Result<Option<String>, ActionError> {
    let saved = save()?;
    let before = unsafe { GetClipboardSequenceNumber() };
    copy()?;
    if !changed_since(before, wait) {
        return Ok(None);
    }
    let text = read_text();
    let restored = restore(&saved);
    let text = text?;
    restored?;
    Ok(text.filter(|t| !t.trim().is_empty()))
}

/// The text formats, which Windows makes from one another: writing the
/// Unicode one is enough.
fn is_text_format(format: u32) -> bool {
    matches!(format, CF_TEXT | CF_OEMTEXT | CF_UNICODETEXT | CF_LOCALE)
}

fn utf16_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

/// `saved` with `line` as its text and every other format as it was.
fn with_text(mut saved: Saved, line: &str) -> Saved {
    saved.formats.retain(|(format, _)| !is_text_format(*format));
    saved.formats.insert(0, (CF_UNICODETEXT, utf16_bytes(line)));
    saved
}

/// Everything on the clipboard, or `None` if some of it can't be copied out
/// and put back (a picture held as a handle, something huge): then the
/// clipboard is better left alone.
fn save_whole() -> Result<Option<Saved>, ActionError> {
    let _open = Open::new(None)?;
    let mut formats = Vec::new();
    let mut format = 0;
    loop {
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        if is_handle_format(format) {
            return Ok(None);
        }
        formats.push(format);
    }
    let mut saved = Saved::default();
    for format in formats {
        match unsafe { GetClipboardData(format) }.ok().and_then(bytes_of) {
            Some(bytes) => saved.formats.push((format, bytes)),
            None => return Ok(None),
        }
    }
    Ok(Some(saved))
}

/// Before a paste: when what was copied is one line of text with blanks
/// around it (see [`pady_core::trimmed_line`]), the line goes back on the
/// clipboard without them, next to everything else that was copied with it.
/// `true` if the clipboard changed. The text is never logged or kept.
pub(crate) fn trim_for_paste() -> Result<bool, ActionError> {
    let before = unsafe { GetClipboardSequenceNumber() };
    let Some(text) = read_text()? else { return Ok(false) };
    let Some(line) = pady_core::trimmed_line(&text) else { return Ok(false) };
    let Some(saved) = save_whole()? else { return Ok(false) };
    if unsafe { GetClipboardSequenceNumber() } != before {
        return Ok(false); // something new was copied meanwhile: not ours to touch
    }
    crate::clipboard_write_coming();
    restore(&with_text(saved, line))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_of(saved: &Saved) -> Option<String> {
        let bytes = &saved.formats.iter().find(|(format, _)| *format == CF_UNICODETEXT)?.1;
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(String::from_utf16_lossy(&units[..units.len() - 1]))
    }

    #[test]
    fn the_trimmed_line_replaces_the_text_and_nothing_else() {
        let html = (49_500, b"Version:0.9 <b> Critical-DIA</b>".to_vec());
        let copied = Saved {
            formats: vec![
                (CF_UNICODETEXT, utf16_bytes("   Critical-DIA")),
                html.clone(),
                (CF_LOCALE, vec![9, 4, 0, 0]),
                (CF_TEXT, b"   Critical-DIA\0".to_vec()),
                (CF_OEMTEXT, b"   Critical-DIA\0".to_vec()),
            ],
        };
        let pasted = with_text(copied, "Critical-DIA");
        assert_eq!(text_of(&pasted).as_deref(), Some("Critical-DIA"));
        // The old text is gone in every form; the page's formatting is untouched.
        assert_eq!(pasted.formats.len(), 2, "the text and the HTML");
        assert_eq!(pasted.formats[1], html);
    }

    #[test]
    fn text_goes_to_the_clipboard_as_utf16_with_an_end_mark() {
        assert_eq!(utf16_bytes("a"), [b'a', 0, 0, 0]);
        assert_eq!(utf16_bytes(""), [0, 0]);
        assert_eq!(text_of(&with_text(Saved::default(), "مرحبا 👋")).as_deref(), Some("مرحبا 👋"));
    }

    /// Uses the real clipboard (and puts it back), so it runs only when asked:
    /// `cargo test -p pady-actions -- --ignored real_clipboard`.
    #[test]
    #[ignore]
    fn real_clipboard_one_line_is_trimmed_and_the_rest_is_kept() {
        let mine = save().unwrap();
        let html = unsafe { RegisterClipboardFormatW(w!("HTML Format")) };
        let page = b"Version:0.9\r\n<td> Critical-DIA</td>".to_vec();

        let copied =
            Saved { formats: vec![(CF_UNICODETEXT, utf16_bytes("\t  Critical-DIA \r\n")), (html, page.clone())] };
        restore(&copied).unwrap();
        assert!(trim_for_paste().unwrap(), "blanks around one line: trimmed");
        assert_eq!(read_text().unwrap().as_deref(), Some("Critical-DIA"));
        let after = save().unwrap();
        assert!(after.formats.contains(&(html, page)), "the HTML stays as it was");
        assert!(!trim_for_paste().unwrap(), "already clean: untouched");

        let code = "    if x {\r\n        y();\r\n    }\r\n";
        restore(&Saved { formats: vec![(CF_UNICODETEXT, utf16_bytes(code))] }).unwrap();
        assert!(!trim_for_paste().unwrap(), "several lines: untouched");
        assert_eq!(read_text().unwrap().as_deref(), Some(code));

        // A picture next to the text (a handle Pady can't copy): untouched.
        let dib = {
            let mut header = vec![0u8; 40 + 4];
            header[0] = 40; // biSize
            header[4] = 1; // 1 x 1
            header[8] = 1;
            header[12] = 1; // planes
            header[14] = 32; // bits per pixel
            header
        };
        restore(&Saved { formats: vec![(CF_UNICODETEXT, utf16_bytes(" cell ")), (8, dib)] }).unwrap();
        assert!(!trim_for_paste().unwrap(), "text with a picture: untouched");
        assert_eq!(read_text().unwrap().as_deref(), Some(" cell "));

        restore(&mine).unwrap();
    }
}
