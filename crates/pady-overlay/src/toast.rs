//! Windows: notices in the top-right corner ("Copied", "Gestures paused"):
//! a glass pill wrapped in a soft purple aura whose colours turn around it.
//!
//! Like the level pill it is a click-through layered window on a thread of
//! its own. The pill itself is drawn once per notice with GDI+; each frame
//! only blends the aura under it (see `glow`), moves the window and sets its
//! opacity. While a notice is on screen every frame waits for the compositor
//! (`DwmFlush`), so motion stays in step with the display; hidden, the thread
//! sleeps in `GetMessageW`.
//!
//! "Copied" comes from the clipboard's change notification: Pady is told
//! *that* something was copied and never reads what. Plenty of programs
//! write the clipboard on their own (clipboard managers, cloud clipboard
//! sync, password managers clearing it, apps filling in a format while you
//! paste), so a change only counts when the app you are using made it, right
//! after you did something, and not while you are pasting.

use crate::color::{Color, Palette};
use crate::gdiplus::{self, Brush, Path, Pen, StringFormat};
use crate::glow::GlowField;
use crate::layout::{px, Rect};
use crate::notice::Motion;
use crate::windows::{animations_enabled, apps_use_light_theme, monitor_under_cursor, Surface, TextFont};
use crate::windows::{ICON_FONTS, NUMBER_FONTS};
use crate::{Icon, Notice};
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION};
use windows::Win32::Graphics::GdiPlus::{StringAlignmentCenter, StringAlignmentNear};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CountClipboardFormats, GetClipboardOwner, GetClipboardSequenceNumber,
    RemoveClipboardFormatListener,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetLastInputInfo, LASTINPUTINFO, VK_CONTROL, VK_INSERT, VK_SHIFT, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::*;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const WM_NOTICE: u32 = WM_APP;
const WM_STOP: u32 = WM_APP + 1;
const WM_COPIED: u32 = WM_APP + 2;
const WM_WATCH: u32 = WM_APP + 3;
const CLASS: PCWSTR = w!("PadyToast");

const GLYPH_CHECK: u16 = 0xE73E;
const GLYPH_PAUSE: u16 = 0xE769;
/// One full turn of the aura's colours.
const TURN: f32 = 2.6;

impl Notice {
    fn text(&self) -> &str {
        match self {
            Notice::Copied => "Copied",
            Notice::Pasted => "Pasted",
            Notice::Paused => "Gestures paused · full-screen game",
            Notice::Text { text, .. } => text,
        }
    }

    fn glyph(&self) -> u16 {
        match self {
            Notice::Copied => GLYPH_CHECK,
            Notice::Pasted => 0xE77F,
            Notice::Paused => GLYPH_PAUSE,
            Notice::Text { icon, .. } => match icon {
                Icon::Touch => 0xE7C9,
                Icon::ZoomIn => 0xE8A3,
                Icon::ZoomOut => 0xE71F,
                Icon::Camera => 0xE722,
                Icon::Music => 0xE8D6,
                Icon::Lock => 0xE72E,
                Icon::Done => GLYPH_CHECK,
            },
        }
    }

    fn rest(&self) -> Duration {
        match self {
            Notice::Copied | Notice::Pasted => Duration::from_millis(1500),
            Notice::Paused => Duration::from_millis(2800),
            Notice::Text { .. } => Duration::from_millis(1700),
        }
    }
}

pub struct Toasts {
    /// The window as an integer, so the handle is `Send + Sync`; it is only posted to.
    hwnd: isize,
    inbox: Sender<Notice>,
    thread: Option<JoinHandle<()>>,
}

impl Toasts {
    pub fn spawn() -> Option<Toasts> {
        let (inbox, notices) = mpsc::channel();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let thread = match std::thread::Builder::new().name("pady-toast".into()).spawn(move || run(notices, ready_tx)) {
            Ok(thread) => thread,
            Err(e) => {
                log::warn!("notices unavailable: could not start their thread: {e}");
                return None;
            }
        };
        match ready.recv() {
            Ok(Ok(hwnd)) => Some(Toasts { hwnd, inbox, thread: Some(thread) }),
            Ok(Err(e)) => {
                log::warn!("notices unavailable: {e}");
                let _ = thread.join();
                None
            }
            Err(_) => {
                let _ = thread.join();
                None
            }
        }
    }

    pub fn show(&self, notice: Notice) {
        if self.inbox.send(notice).is_ok() {
            self.post(WM_NOTICE, 0);
        }
    }

    pub fn watch_clipboard(&self, on: bool) {
        self.post(WM_WATCH, usize::from(on));
    }

    fn post(&self, msg: u32, wparam: usize) {
        let _ = unsafe { PostMessageW(HWND(self.hwnd as *mut c_void), msg, WPARAM(wparam), LPARAM(0)) };
    }
}

impl Drop for Toasts {
    fn drop(&mut self) {
        let stopping = unsafe { PostMessageW(HWND(self.hwnd as *mut c_void), WM_STOP, WPARAM(0), LPARAM(0)) }.is_ok();
        if let Some(thread) = self.thread.take() {
            if stopping {
                let _ = thread.join();
            }
        }
    }
}

fn run(notices: Receiver<Notice>, ready: SyncSender<std::result::Result<isize, String>>) {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let _gdiplus = match gdiplus::Session::start() {
        Ok(session) => session,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let hwnd = match create_window() {
        Ok(hwnd) => hwnd,
        Err(e) => {
            let _ = ready.send(Err(format!("could not create their window: {e}")));
            return;
        }
    };
    let mut toaster = Toaster::new(hwnd);
    // Fonts and drawing proven before reporting success; also makes the first notice instant.
    let (_, dpi) = monitor_under_cursor();
    let prepared = toaster
        .prepare(Style { dpi, light: apps_use_light_theme() })
        .and_then(|()| toaster.renderer.as_mut().ok_or("no renderer".into()).and_then(|r| r.draw(&Notice::Copied)));
    if let Err(e) = prepared {
        let _ = ready.send(Err(format!("could not draw: {e}")));
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return;
    }
    if ready.send(Ok(hwnd.0 as isize)).is_err() {
        return;
    }

    let mut msg = MSG::default();
    'outer: loop {
        if toaster.motion.visible() {
            // On screen: handle whatever arrived, then draw the next frame as
            // soon as the compositor has shown the last one.
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                if !toaster.handle(&msg, &notices) {
                    break 'outer;
                }
            }
            if unsafe { DwmFlush() }.is_err() {
                std::thread::sleep(Duration::from_millis(7));
            }
            toaster.step();
        } else {
            let got = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
            if got == 0 || got == -1 || !toaster.handle(&msg, &notices) {
                break;
            }
        }
    }
    toaster.watch(false);
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

fn create_window() -> windows::core::Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&class);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            CLASS,
            w!("Pady notice"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            instance,
            None,
        )
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_CLOSE => LRESULT(0),
        // Sent straight here by Windows; handled in the loop like everything else.
        WM_CLIPBOARDUPDATE => {
            let _ = PostMessageW(hwnd, WM_COPIED, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Style {
    dpi: u32,
    light: bool,
}

struct Toaster {
    hwnd: HWND,
    epoch: Instant,
    motion: Motion,
    renderer: Option<Renderer>,
    /// The window's top-left while resting, and the work area's right/top edge it hangs from.
    origin: (i32, i32),
    anchor: (i32, i32),
    watching: bool,
    /// The clipboard's sequence number when "Copied" was last shown, so one
    /// copy announced twice (some apps write it in two goes) shows once.
    last_copy: u32,
    reported: bool,
}

/// What Pady knows about a clipboard change; never its content.
#[derive(Debug, Default)]
struct ClipboardChange {
    formats: i32,
    /// The process that wrote the clipboard (`None`: no window owns it).
    owner: Option<u32>,
    /// The processes behind the window in front: the app itself, plus any
    /// it hosts (Store apps live inside ApplicationFrameHost).
    foreground: Vec<u32>,
    /// Since the last key press, click or touch.
    idle: Option<Duration>,
    /// Ctrl+V or Shift+Insert held: apps fill in clipboard formats on paste.
    pasting: bool,
}

/// Long enough for a slow app to finish writing after the key press.
const COPY_WINDOW: Duration = Duration::from_millis(1500);

fn looks_like_your_copy(c: &ClipboardChange) -> bool {
    // A cleared clipboard (no formats) is not a copy.
    c.formats > 0
        && !c.pasting
        && c.idle.is_some_and(|idle| idle <= COPY_WINDOW)
        && c.owner.is_some_and(|owner| c.foreground.contains(&owner))
}

impl ClipboardChange {
    fn now() -> ClipboardChange {
        unsafe {
            let pid = |hwnd: HWND| {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                (pid != 0).then_some(pid)
            };
            let owner = GetClipboardOwner().ok().filter(|h| !h.is_invalid()).and_then(pid);
            let mut foreground = Vec::new();
            let front = GetForegroundWindow();
            if !front.is_invalid() {
                foreground.extend(pid(front));
                unsafe extern "system" fn add(child: HWND, list: LPARAM) -> windows::Win32::Foundation::BOOL {
                    let list = &mut *(list.0 as *mut Vec<u32>);
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(child, Some(&mut pid));
                    if pid != 0 && !list.contains(&pid) {
                        list.push(pid);
                    }
                    true.into()
                }
                let _ = EnumChildWindows(front, Some(add), LPARAM(&mut foreground as *mut Vec<u32> as isize));
            }
            let mut info = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
            let idle = GetLastInputInfo(&mut info)
                .as_bool()
                .then(|| Duration::from_millis(u64::from(GetTickCount().wrapping_sub(info.dwTime))));
            let down = |key: u16| GetAsyncKeyState(i32::from(key)) < 0;
            let pasting = (down(VK_CONTROL.0) && down(VK_V.0)) || (down(VK_SHIFT.0) && down(VK_INSERT.0));
            ClipboardChange { formats: CountClipboardFormats(), owner, foreground, idle, pasting }
        }
    }
}

impl Toaster {
    fn new(hwnd: HWND) -> Toaster {
        Toaster {
            hwnd,
            epoch: Instant::now(),
            motion: Motion::default(),
            renderer: None,
            origin: (0, 0),
            anchor: (0, 0),
            watching: false,
            last_copy: 0,
            reported: false,
        }
    }

    /// `false` when the thread should stop.
    fn handle(&mut self, msg: &MSG, notices: &Receiver<Notice>) -> bool {
        if msg.message == WM_QUIT {
            return false;
        }
        if msg.hwnd == self.hwnd {
            match msg.message {
                WM_NOTICE => {
                    if let Some(notice) = notices.try_iter().last() {
                        self.show(notice);
                    }
                    return true;
                }
                WM_COPIED => {
                    if self.watching {
                        let seq = unsafe { GetClipboardSequenceNumber() };
                        if seq != self.last_copy && looks_like_your_copy(&ClipboardChange::now()) {
                            self.last_copy = seq;
                            self.show(Notice::Copied);
                        }
                    }
                    return true;
                }
                WM_WATCH => {
                    self.watch(msg.wParam.0 != 0);
                    return true;
                }
                WM_STOP => return false,
                _ => {}
            }
        }
        unsafe { DispatchMessageW(msg) };
        true
    }

    fn watch(&mut self, on: bool) {
        if on == self.watching {
            return;
        }
        let result = unsafe {
            if on {
                AddClipboardFormatListener(self.hwnd)
            } else {
                RemoveClipboardFormatListener(self.hwnd)
            }
        };
        match result {
            Ok(()) => self.watching = on,
            Err(e) => log::warn!("could not {} copy notices: {e}", if on { "turn on" } else { "turn off" }),
        }
    }

    fn prepare(&mut self, style: Style) -> Result<()> {
        if self.renderer.as_ref().is_some_and(|r| r.style == style) {
            return Ok(());
        }
        self.renderer = None;
        self.renderer = Some(Renderer::new(style)?);
        Ok(())
    }

    fn show(&mut self, notice: Notice) {
        let now = self.epoch.elapsed();
        let appearing = !self.motion.visible();
        if appearing {
            let (work, dpi) = monitor_under_cursor();
            if let Err(e) = self.prepare(Style { dpi, light: apps_use_light_theme() }) {
                self.report(&*e);
                return;
            }
            self.anchor = (work.right, work.top);
            self.motion.slide = animations_enabled();
        }
        let Some(renderer) = self.renderer.as_mut() else { return };
        let resized = match renderer.draw(&notice) {
            Ok(resized) => resized,
            Err(e) => {
                self.report(&*e);
                return;
            }
        };
        if appearing || resized {
            // The pill's top-right corner sits 18 logical px in from the work area's.
            let (pill, s) = (renderer.pill, renderer.scale);
            let inset = px(18.0, s);
            self.origin = (
                self.anchor.0 - (inset + pill.right()).round() as i32,
                self.anchor.1 + (inset - pill.y).round() as i32,
            );
        }
        if self.motion.show(now, notice.rest()) {
            // The first frame (fully transparent) is in place before the window shows.
            self.step();
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
                let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
            }
        }
    }

    /// Draws the frame for now, or hides the window once the notice is gone.
    fn step(&mut self) {
        let now = self.epoch.elapsed();
        self.motion.tick(now);
        if !self.motion.visible() {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            return;
        }
        let pose = self.motion.pose(now);
        let glow = self.motion.glow(now);
        let turn = (now.as_secs_f32() / TURN).fract();
        log::trace!(
            "notice frame at {:.1} ms: opacity {:.2}, glow {glow:.2}",
            now.as_secs_f64() * 1000.0,
            pose.opacity
        );
        let Some(renderer) = self.renderer.as_mut() else { return };
        let y = self.origin.1 + (pose.dy * renderer.scale).round() as i32;
        let result =
            renderer.compose(glow, turn).and_then(|()| renderer.push(self.hwnd, (self.origin.0, y), pose.opacity));
        if let Err(e) = result {
            self.report(&*e);
        }
    }

    fn report(&mut self, error: &dyn std::fmt::Display) {
        if self.reported {
            log::debug!("notice: {error}");
        } else {
            log::warn!("notice: {error}");
            self.reported = true;
        }
    }
}

/// Fonts, the drawn pill and the aura for one DPI and theme.
struct Renderer {
    style: Style,
    scale: f32,
    palette: &'static Palette,
    icon: Option<TextFont>,
    text: TextFont,
    near: StringFormat,
    centered: StringFormat,
    /// A 1×1 surface, just to measure text before sizing the real one.
    measure: Surface,
    /// What the window shows: rewritten every frame.
    surface: Option<Surface>,
    /// The pill alone (premultiplied pixels), drawn once per notice.
    pill_layer: Vec<u32>,
    glow: Option<GlowField>,
    size: (i32, i32),
    pill: Rect,
    drawn: Option<Notice>,
}

impl Renderer {
    fn new(style: Style) -> Result<Renderer> {
        let scale = crate::layout::scale_for(style.dpi);
        let icon = ICON_FONTS.iter().find_map(|name| TextFont::new(name, gdiplus::REGULAR, px(15.0, scale)).ok());
        let text = std::iter::once(("Segoe UI Variable Text Semibold", gdiplus::REGULAR))
            .chain(NUMBER_FONTS.iter().copied().skip(1))
            .find_map(|(name, weight)| TextFont::new(name, weight, px(16.0, scale)).ok())
            .ok_or("no font for notices")?;
        Ok(Renderer {
            style,
            scale,
            palette: Palette::for_theme(style.light),
            icon,
            text,
            near: StringFormat::typographic(StringAlignmentNear)?,
            centered: StringFormat::typographic(StringAlignmentCenter)?,
            measure: Surface::new(1, 1)?,
            surface: None,
            pill_layer: Vec::new(),
            glow: None,
            size: (0, 0),
            pill: Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 },
            drawn: None,
        })
    }

    /// Draws the pill for `notice` unless it is already drawn. `true` if the window's size changed.
    fn draw(&mut self, notice: &Notice) -> Result<bool> {
        if self.drawn.as_ref() == Some(notice) {
            return Ok(false);
        }
        let s = self.scale;
        let text: Vec<u16> = notice.text().encode_utf16().collect();
        let text_w = self.measure.graphics.measure(&text, &self.text.font, &self.near)?.ceil();
        // Room around the pill for the aura to fade out in.
        let margin = px(40.0, s);
        let height = px(50.0, s);
        let (pad_left, dot, gap, pad_right) = (px(9.0, s), px(32.0, s), px(12.0, s), px(22.0, s));
        let pill = Rect { x: margin, y: margin, w: pad_left + dot + gap + text_w + pad_right, h: height };
        let size = ((pill.w + 2.0 * margin) as i32, (pill.h + 2.0 * margin) as i32);
        let resized = size != self.size;
        if resized || self.surface.is_none() {
            self.surface = None;
            self.surface = Some(Surface::new(size.0, size.1)?);
            self.size = size;
            self.glow = Some(GlowField::new(size.0 as usize, size.1 as usize, pill, pill.h / 2.0, s));
        }
        self.pill = pill;
        let p = self.palette;
        let surface = self.surface.as_mut().ok_or("no surface")?;
        surface.dib.pixels().fill(0);
        let g = &surface.graphics;

        // Glass, sheen and a hairline border, as on the level pill.
        let radius = pill.h / 2.0;
        let mut shape = Path::new()?;
        shape.set_rounded_rect(pill, radius)?;
        // Solid, so the aura behind never shows through the glass.
        g.fill_path(&Brush::solid(Color { a: 1.0, ..p.glass })?, &shape)?;
        let clear = p.shine.fade(0.0);
        let sheen = Rect { y: pill.y - 1.0, h: pill.h + 2.0, ..pill };
        g.fill_path(&Brush::vertical(sheen, &[(0.0, p.shine), (0.55, clear), (1.0, clear)])?, &shape)?;
        let border = s.floor().max(1.0);
        shape.set_rounded_rect(pill.inset(border / 2.0), radius - border / 2.0)?;
        g.draw_path(&Pen::new(p.border, border)?, &shape)?;

        // The icon in a gradient dot.
        let circle = Rect { x: pill.x + pad_left, y: (pill.center_y() - dot / 2.0).round(), w: dot, h: dot };
        let mut round = Path::new()?;
        round.set_rounded_rect(circle, dot / 2.0)?;
        g.fill_path(&Brush::horizontal(circle.x, circle.right(), &[(0.0, p.fill.0), (1.0, p.fill.1)])?, &round)?;
        if let Some(icon) = &self.icon {
            let line = icon.ascent + icon.descent;
            let em = px(15.0, s);
            let at = Rect {
                x: (circle.center_x() - em / 2.0).round(),
                y: (circle.center_y() - line / 2.0).round(),
                w: em,
                h: line,
            };
            g.draw_text(&[notice.glyph()], &icon.font, at, &self.centered, &Brush::solid(Color::rgb(255, 255, 255))?)?;
        }

        // The words, centred on the pill's middle line.
        let t = &self.text;
        let baseline = (pill.center_y() + (t.ascent - t.descent) / 2.0).round();
        let at = Rect { x: circle.right() + gap, y: baseline - t.ascent, w: text_w + px(4.0, s), h: t.line() };
        g.draw_text(&text, &t.font, at, &self.near, &Brush::solid(p.text)?)?;
        g.flush()?;
        self.pill_layer = surface.dib.pixels().to_vec();
        self.drawn = Some(notice.clone());
        Ok(resized)
    }

    /// Puts this frame's aura around the pill.
    fn compose(&mut self, glow: f32, turn: f32) -> Result<()> {
        let (Some(surface), Some(field)) = (self.surface.as_mut(), self.glow.as_ref()) else {
            return Err("nothing drawn yet".into());
        };
        let out = surface.dib.pixels();
        if out.len() != self.pill_layer.len() {
            return Err("notice bitmap size mismatch".into());
        }
        field.paint(out, &self.pill_layer, glow, turn);
        Ok(())
    }

    fn push(&self, hwnd: HWND, at: (i32, i32), opacity: f32) -> Result<()> {
        let surface = self.surface.as_ref().ok_or("nothing drawn yet")?;
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size = SIZE { cx: self.size.0, cy: self.size.1 };
        let (origin, target) = (POINT::default(), POINT { x: at.0, y: at.1 });
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                Some(&target as *const POINT),
                Some(&size as *const SIZE),
                surface.dib.dc,
                Some(&origin as *const POINT),
                COLORREF(0),
                Some(&blend as *const BLENDFUNCTION),
                ULW_ALPHA,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::tests::GDIPLUS;

    fn copy_in_front() -> ClipboardChange {
        ClipboardChange {
            formats: 3,
            owner: Some(42),
            foreground: vec![42],
            idle: Some(Duration::from_millis(80)),
            pasting: false,
        }
    }

    #[test]
    fn a_copy_in_the_app_you_use_counts() {
        assert!(looks_like_your_copy(&copy_in_front()));
        // A Store app: the frame is one process, the app inside it another.
        let hosted = ClipboardChange { foreground: vec![7, 42], ..copy_in_front() };
        assert!(looks_like_your_copy(&hosted));
    }

    #[test]
    fn clipboard_writes_you_did_not_make_do_not() {
        let background = ClipboardChange { owner: Some(99), ..copy_in_front() };
        assert!(!looks_like_your_copy(&background), "another program wrote it");
        let unowned = ClipboardChange { owner: None, ..copy_in_front() };
        assert!(!looks_like_your_copy(&unowned), "no window owns it");
        let away = ClipboardChange { idle: Some(Duration::from_secs(20)), ..copy_in_front() };
        assert!(!looks_like_your_copy(&away), "nobody touched anything");
        let pasting = ClipboardChange { pasting: true, ..copy_in_front() };
        assert!(!looks_like_your_copy(&pasting), "an app filling in formats on paste");
        let cleared = ClipboardChange { formats: 0, ..copy_in_front() };
        assert!(!looks_like_your_copy(&cleared), "emptied");
    }

    #[test]
    fn draws_both_notices_at_every_dpi() {
        let _turn = GDIPLUS.lock().unwrap_or_else(|e| e.into_inner());
        let _gdiplus = gdiplus::Session::start().expect("GDI+");
        for dpi in [96, 120, 144, 192] {
            for light in [false, true] {
                let mut r = Renderer::new(Style { dpi, light }).expect("renderer");
                assert!(r.draw(&Notice::Copied).expect("draw"), "first draw sizes the window");
                let copied = r.size;
                assert!(!r.draw(&Notice::Copied).expect("draw"), "same notice: nothing to do");
                assert!(r.draw(&Notice::Paused).expect("draw"), "longer text, wider window");
                assert!(r.size.0 > copied.0);
                r.compose(1.0, 0.3).expect("compose");
                let w = r.size.0 as usize;
                let (cx, cy) = (r.pill.right() - px(10.0, r.scale), r.pill.center_y());
                let px_at = |r: &mut Renderer, x: f32, y: f32| {
                    r.surface.as_mut().unwrap().dib.pixels()[y as usize * w + x as usize]
                };
                assert_eq!(px_at(&mut r, cx, cy) >> 24, 255, "solid glass at {dpi} dpi");
                let (bx, by) = (r.pill.center_x(), r.pill.y + r.pill.h + px(4.0, r.scale));
                let below = px_at(&mut r, bx, by);
                assert!(below >> 24 > 40, "the aura glows under the pill: {below:08x}");
                assert_eq!(px_at(&mut r, 0.0, 0.0) >> 24, 0, "nothing in the far corner");
            }
        }
    }

    #[test]
    fn a_frame_is_quick() {
        let _turn = GDIPLUS.lock().unwrap_or_else(|e| e.into_inner());
        let _gdiplus = gdiplus::Session::start().expect("GDI+");
        let mut r = Renderer::new(Style { dpi: 192, light: false }).expect("renderer");
        r.draw(&Notice::Paused).expect("draw");
        let start = Instant::now();
        for i in 0..120 {
            r.compose(1.0 + (i % 10) as f32 * 0.03, i as f32 / 120.0).expect("compose");
        }
        let per_frame = start.elapsed() / 120;
        // Debug builds are slower; release is several times quicker still.
        assert!(per_frame < Duration::from_millis(8), "{per_frame:?} per frame at 200%");
    }

    /// Writes renders over a backdrop to `%TEMP%\pady-toast-preview`:
    /// `cargo test -p pady-overlay -- --ignored toast_preview`.
    #[test]
    #[ignore]
    fn toast_preview() {
        let _turn = GDIPLUS.lock().unwrap_or_else(|e| e.into_inner());
        let _gdiplus = gdiplus::Session::start().expect("GDI+");
        let dir = std::env::temp_dir().join("pady-toast-preview");
        std::fs::create_dir_all(&dir).expect("dir");
        for (dpi, light) in [(144, false), (144, true)] {
            let mut r = Renderer::new(Style { dpi, light }).expect("renderer");
            let hint = Notice::Text { text: "4-finger swipe up · Task view".into(), icon: Icon::Touch };
            for notice in [Notice::Copied, Notice::Pasted, Notice::Paused, hint] {
                r.draw(&notice).expect("draw");
                for (frame, (glow, turn)) in [(1.0f32, 0.0f32), (1.45, 0.35)].into_iter().enumerate() {
                    r.compose(glow, turn).expect("compose");
                    let (w, h) = r.size;
                    let (top, bottom) = if light {
                        (Color::rgb(236, 238, 250), Color::rgb(250, 248, 255))
                    } else {
                        (Color::rgb(34, 36, 60), Color::rgb(12, 13, 20))
                    };
                    let mut px = r.surface.as_mut().unwrap().dib.pixels().to_vec();
                    for (i, p) in px.iter_mut().enumerate() {
                        let bg = top.lerp(bottom, (i / w as usize) as f32 / h as f32);
                        let (a, sr, sg, sb) = (*p >> 24, (*p >> 16) & 0xff, (*p >> 8) & 0xff, *p & 0xff);
                        let k = 1.0 - a as f32 / 255.0;
                        let mix = |s: u32, b: f32| (s as f32 + b * k).round().min(255.0) as u32;
                        *p = 0xff00_0000 | mix(sr, bg.r) << 16 | mix(sg, bg.g) << 8 | mix(sb, bg.b);
                    }
                    let bitmap = unsafe { gdiplus::Bitmap::over_pixels(w, h, px.as_mut_ptr().cast()) }.expect("bitmap");
                    let kind = format!("{notice:?}").chars().take_while(|c| c.is_alphanumeric()).collect::<String>();
                    let name = format!("{kind}-{}-{frame}.png", if light { "light" } else { "dark" });
                    bitmap.save_png(&dir.join(name)).expect("png");
                }
            }
        }
        println!("previews in {}", dir.display());
    }
}
