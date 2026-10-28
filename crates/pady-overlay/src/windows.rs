//! Windows backend: a layered popup drawn with GDI+ on a thread of its own.
//!
//! The window is `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE |
//! WS_EX_TOOLWINDOW | WS_EX_TOPMOST`: DWM composites it from a premultiplied
//! bitmap, clicks fall through it, it never takes focus and it stays out of
//! Alt+Tab. Frames are drawn with GDI+ into a 32-bit DIB and handed over with
//! `UpdateLayeredWindow`; fades only change the constant alpha. The thread's
//! timer runs only while something moves or a fade-out is due, so a hidden
//! overlay sleeps in `GetMessageW` and costs no CPU.

use crate::anim::{Animator, Frame};
use crate::color::Palette;
use crate::gdiplus::{self, Brush, Font, FontFamily, Graphics, Path, Pen, StringFormat};
use crate::layout::{self, Layout, Rect, WorkArea};
use crate::shadow;
use pady_core::{Level, LevelKind};
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetMonitorInfoW, MonitorFromPoint, SelectObject,
    AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HBITMAP, HDC,
    HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::Graphics::GdiPlus::{StringAlignmentCenter, StringAlignmentFar};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::*;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Posted by `show()`: levels are waiting in the channel.
const WM_WAKE: u32 = WM_APP;
/// Posted on drop: leave the message loop.
const WM_STOP: u32 = WM_APP + 1;
const TIMER: usize = 1;
const CLASS: PCWSTR = w!("PadyOverlay");

/// Segoe Fluent Icons is Windows 11's; Segoe MDL2 Assets has the same code
/// points on Windows 10.
pub(crate) const ICON_FONTS: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];
const GLYPH_VOLUME: u16 = 0xE767;
const GLYPH_MUTE: u16 = 0xE74F;
const GLYPH_BRIGHTNESS: u16 = 0xE706;
/// Semibold numerals, best first. GDI cuts family names at 31 characters,
/// which is how Windows 11's variable font names its semibold Display
/// instance; Windows 10 has the static Segoe UI Semibold.
pub(crate) const NUMBER_FONTS: [(&str, i32); 4] = [
    ("Segoe UI Variable Display Semib", gdiplus::REGULAR),
    ("Segoe UI Semibold", gdiplus::REGULAR),
    ("Segoe UI", gdiplus::BOLD),
    ("Arial", gdiplus::BOLD),
];

/// What [`crate::Overlay`] holds: a channel to the overlay thread and the
/// window to wake it through.
pub struct Overlay {
    /// The window as an integer, so the handle is `Send + Sync`; it is only
    /// ever posted to.
    hwnd: isize,
    levels: Sender<Level>,
    /// Set while a wake-up is queued, so a burst of levels posts one message.
    wake_queued: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Overlay {
    pub fn spawn() -> Option<Overlay> {
        let (levels, inbox) = mpsc::channel();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let wake_queued = Arc::new(AtomicBool::new(false));
        let queued = wake_queued.clone();
        let thread =
            std::thread::Builder::new().name("pady-overlay".into()).spawn(move || run(inbox, queued, ready_tx));
        let thread = match thread {
            Ok(thread) => thread,
            Err(e) => {
                log::warn!("native overlay unavailable: could not start its thread: {e}");
                return None;
            }
        };
        match ready.recv() {
            Ok(Ok(hwnd)) => Some(Overlay { hwnd, levels, wake_queued, thread: Some(thread) }),
            Ok(Err(e)) => {
                log::warn!("native overlay unavailable: {e}");
                let _ = thread.join();
                None
            }
            Err(_) => {
                log::warn!("native overlay unavailable: its thread exited during startup");
                let _ = thread.join();
                None
            }
        }
    }

    pub fn show(&self, level: Level) {
        if self.levels.send(level).is_err() {
            return; // the thread is gone
        }
        if !self.wake_queued.swap(true, Ordering::AcqRel) {
            let posted = unsafe { PostMessageW(self.hwnd(), WM_WAKE, WPARAM(0), LPARAM(0)) };
            if posted.is_err() {
                self.wake_queued.store(false, Ordering::Release);
            }
        }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut c_void)
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        let stopping = unsafe { PostMessageW(self.hwnd(), WM_STOP, WPARAM(0), LPARAM(0)) }.is_ok();
        if let Some(thread) = self.thread.take() {
            // Without the message the thread is already gone (or wedged): don't wait on it.
            if stopping {
                let _ = thread.join();
            }
        }
    }
}

/// The overlay thread: owns GDI+, the window and everything drawn.
fn run(levels: Receiver<Level>, wake_queued: Arc<AtomicBool>, ready: SyncSender<std::result::Result<isize, String>>) {
    // Physical pixels whatever the host process declared, so the pill is
    // drawn for each monitor's real DPI instead of being stretched by DWM.
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    // Dropped last (locals drop in reverse): every GDI+ object is gone by then.
    let _gdiplus = match gdiplus::Session::start() {
        Ok(session) => session,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let window = match Window::create() {
        Ok(window) => window,
        Err(e) => {
            let _ = ready.send(Err(format!("could not create its window: {e}")));
            return;
        }
    };
    let mut painter = Painter::new(window.0);
    // Draw once now: proves drawing works before `spawn` reports success, and
    // makes the first appearance instant.
    let (_, dpi) = monitor_under_cursor();
    if let Err(e) = painter.prepare(Style { dpi, light: apps_use_light_theme() }) {
        let _ = ready.send(Err(format!("could not draw: {e}")));
        return;
    }
    if ready.send(Ok(window.0 .0 as isize)).is_err() {
        return;
    }

    let mut msg = MSG::default();
    loop {
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
        if got == 0 || got == -1 {
            break;
        }
        if msg.hwnd == window.0 {
            match msg.message {
                WM_WAKE => {
                    // Clear before draining, so a level sent meanwhile posts a new wake-up.
                    let _ = wake_queued.swap(false, Ordering::AcqRel);
                    // Only the newest level matters: the pill glides to it from wherever it is.
                    if let Some(level) = levels.try_iter().last() {
                        painter.show(level);
                    }
                    continue;
                }
                WM_TIMER if msg.wParam.0 == TIMER => {
                    painter.tick();
                    continue;
                }
                WM_STOP => break,
                _ => {}
            }
        }
        unsafe { DispatchMessageW(&msg) };
    }
}

/// The overlay's window, destroyed on drop.
struct Window(HWND);

impl Window {
    fn create() -> windows::core::Result<Window> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: CLASS,
                ..Default::default()
            };
            // Fails harmlessly if an earlier overlay in this process registered it.
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                CLASS,
                w!("Pady overlay"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                instance,
                None,
            )?;
            Ok(Window(hwnd))
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // WS_EX_TRANSPARENT already sends the mouse to the window below; these make sure.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // Only the overlay thread closes its window (on WM_STOP), not requests from outside.
        WM_CLOSE => LRESULT(0),
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The monitor under the cursor (the one the user is looking at): its work
/// area and DPI. Falls back to the primary monitor.
pub(crate) fn monitor_under_cursor() -> (WorkArea, u32) {
    unsafe {
        let mut cursor = POINT::default();
        let monitor = if GetCursorPos(&mut cursor).is_ok() {
            MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY)
        };
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let work = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcWork
        } else {
            RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) }
        };
        let (mut dpi, mut dpi_y) = (0, 0);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y).is_err() {
            dpi = 96;
        }
        (WorkArea { left: work.left, top: work.top, right: work.right, bottom: work.bottom }, dpi)
    }
}

/// Whether apps should be light ("Choose your default app mode" in Settings).
pub(crate) fn apps_use_light_theme() -> bool {
    let mut value = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    status.is_ok() && value == 1
}

/// Windows' "Animation effects" switch. With it off the bar jumps instead of
/// gliding, as the WebView pill did under `prefers-reduced-motion`.
pub(crate) fn animations_enabled() -> bool {
    let mut enabled = BOOL(1);
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut enabled as *mut BOOL as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_err() || enabled.as_bool()
}

/// Everything the overlay thread keeps between messages.
struct Painter {
    hwnd: HWND,
    epoch: Instant,
    animator: Animator,
    renderer: Option<Renderer>,
    /// Interval of the running timer, if one is running.
    timer: Option<u32>,
    /// Glide between values (Windows animation effects are on).
    glide: bool,
    /// Drawing failures are logged once, not at 60 fps.
    reported: bool,
}

impl Painter {
    fn new(hwnd: HWND) -> Painter {
        Painter {
            hwnd,
            epoch: Instant::now(),
            animator: Animator::default(),
            renderer: None,
            timer: None,
            glide: true,
            reported: false,
        }
    }

    /// Makes sure the renderer suits `style`, rebuilding it after a DPI or theme change.
    fn prepare(&mut self, style: Style) -> Result<()> {
        if self.renderer.as_ref().is_some_and(|r| r.style == style) {
            return Ok(());
        }
        self.renderer = None; // free the old bitmap and fonts first
        self.renderer = Some(Renderer::new(style)?);
        log::debug!("overlay drawn for {} dpi, {} theme", style.dpi, if style.light { "light" } else { "dark" });
        Ok(())
    }

    fn show(&mut self, level: Level) {
        let now = self.epoch.elapsed();
        let mut position = None;
        if !self.animator.is_visible() {
            // Appearing: on the monitor under the cursor, in the current theme.
            let (work, dpi) = monitor_under_cursor();
            if let Err(e) = self.prepare(Style { dpi, light: apps_use_light_theme() }) {
                self.report(&*e);
                return;
            }
            let Some(renderer) = &self.renderer else { return };
            let l = &renderer.layout;
            position = Some(layout::place(work, l.width, l.height, l.scale));
            self.glide = animations_enabled();
        }
        self.animator.show(level, now, self.glide);
        if position.is_some() && self.present(now, position) {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                // Above topmost windows that came along since, still without activating.
                let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
                let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
            }
        }
        self.schedule(now);
    }

    fn tick(&mut self) {
        let now = self.epoch.elapsed();
        self.animator.tick(now);
        if self.animator.is_visible() {
            self.present(now, None);
        } else {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
        self.schedule(now);
    }

    /// Draws the frame for `now` and hands it to the window, moving the
    /// window to `position` if given (only when the pill appears).
    fn present(&mut self, now: Duration, position: Option<(i32, i32)>) -> bool {
        let frame = self.animator.frame(now);
        let Some(renderer) = self.renderer.as_mut() else { return false };
        let result = renderer.draw(&frame).and_then(|()| renderer.push(self.hwnd, position, frame.opacity));
        match result {
            Ok(()) => true,
            Err(e) => {
                self.report(&*e);
                false
            }
        }
    }

    /// Runs the timer at frame rate while something moves, once more when
    /// the fade-out is due, and not at all while the pill is hidden.
    fn schedule(&mut self, now: Duration) {
        let wanted = self.animator.next_wake(now).map(|delay| {
            let ms = u32::try_from(delay.as_micros().div_ceil(1000)).unwrap_or(u32::MAX);
            ms.max(USER_TIMER_MINIMUM)
        });
        if wanted == self.timer {
            return;
        }
        match wanted {
            Some(ms) => {
                if unsafe { SetTimer(self.hwnd, TIMER, ms, None) } == 0 {
                    self.report(&"SetTimer failed");
                    self.timer = None;
                    return;
                }
            }
            None => unsafe {
                let _ = KillTimer(self.hwnd, TIMER);
            },
        }
        self.timer = wanted;
    }

    fn report(&mut self, error: &dyn std::fmt::Display) {
        if self.reported {
            log::debug!("overlay: {error}");
        } else {
            log::warn!("overlay: {error}");
            self.reported = true;
        }
    }
}

/// What the renderer is built for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Style {
    dpi: u32,
    light: bool,
}

/// What the bitmap shows apart from opacity, so fades don't redraw it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Content {
    glyph: u16,
    number: u8,
    percent: f32,
    mute: f32,
}

impl Content {
    fn of(frame: &Frame) -> Content {
        Content { glyph: glyph(frame), number: frame.number(), percent: frame.percent, mute: frame.mute }
    }
}

fn glyph(frame: &Frame) -> u16 {
    match (frame.kind, frame.muted) {
        (LevelKind::Brightness, _) => GLYPH_BRIGHTNESS,
        (LevelKind::Volume, true) => GLYPH_MUTE,
        (LevelKind::Volume, false) => GLYPH_VOLUME,
    }
}

/// The bitmap, fonts and static layer for one DPI and theme.
struct Renderer {
    style: Style,
    layout: Layout,
    palette: &'static Palette,
    fonts: Fonts,
    /// The track's shape; the bar is clipped to it.
    track: Path,
    /// Shadow, glass and empty track, copied in before each frame's moving parts are drawn.
    base: Vec<u32>,
    surface: Surface,
    drawn: Option<Content>,
}

impl Renderer {
    fn new(style: Style) -> Result<Renderer> {
        let layout = Layout::new(style.dpi);
        let palette = Palette::for_theme(style.light);
        let fonts = Fonts::new(&layout)?;
        let mut surface = Surface::new(layout.width, layout.height)?;
        let mut track = Path::new()?;
        track.set_rounded_rect(layout.track, layout.track.h / 2.0)?;

        // The static layer: the shadow is computed, the glass and track drawn over it.
        let (width, height) = (layout.width as usize, layout.height as usize);
        let pill_radius = layout.pill.h / 2.0;
        let shadows = shadow::render(width, height, layout.pill, pill_radius, &palette.shadows, layout.scale);
        copy_pixels(surface.dib.pixels(), &shadows)?;
        draw_glass(&surface.graphics, &layout, palette, &track)?;
        surface.graphics.flush()?;
        let base = surface.dib.pixels().to_vec();
        Ok(Renderer { style, layout, palette, fonts, track, base, surface, drawn: None })
    }

    /// Draws `frame` into the bitmap, unless it already shows it.
    fn draw(&mut self, frame: &Frame) -> Result<()> {
        let content = Content::of(frame);
        if self.drawn == Some(content) {
            return Ok(());
        }
        self.drawn = None;
        copy_pixels(self.surface.dib.pixels(), &self.base)?;
        let (g, l, p) = (&self.surface.graphics, &self.layout, self.palette);
        draw_bar(g, l, p, &self.track, frame)?;
        draw_icon(g, l, p, &self.fonts, frame)?;
        draw_value(g, l, p, &self.fonts, frame)?;
        g.flush()?;
        self.drawn = Some(content);
        Ok(())
    }

    /// Hands the bitmap to the window at `opacity`, moving it to `position` if given.
    fn push(&self, hwnd: HWND, position: Option<(i32, i32)>, opacity: f32) -> Result<()> {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let size = SIZE { cx: self.layout.width, cy: self.layout.height };
        let origin = POINT::default();
        let target = position.map(|(x, y)| POINT { x, y });
        unsafe {
            UpdateLayeredWindow(
                hwnd,
                None,
                target.as_ref().map(|p| p as *const POINT),
                Some(&size as *const SIZE),
                self.surface.dib.dc,
                Some(&origin as *const POINT),
                COLORREF(0),
                Some(&blend as *const BLENDFUNCTION),
                ULW_ALPHA,
            )?;
        }
        Ok(())
    }
}

pub(crate) fn copy_pixels(to: &mut [u32], from: &[u32]) -> Result<()> {
    if to.len() != from.len() {
        return Err("overlay bitmap size mismatch".into());
    }
    to.copy_from_slice(from);
    Ok(())
}

/// The glass (`.pill`): background, a sheen fading out halfway down, a
/// border inside the edge, then the empty track.
fn draw_glass(g: &Graphics, l: &Layout, p: &Palette, track: &Path) -> Result<()> {
    let radius = l.pill.h / 2.0;
    let mut pill = Path::new()?;
    pill.set_rounded_rect(l.pill, radius)?;
    g.fill_path(&Brush::solid(p.glass)?, &pill)?;
    let clear = p.shine.fade(0.0);
    // A pixel past each end keeps GDI+'s gradient edges clean.
    let sheen = Rect { y: l.pill.y - 1.0, h: l.pill.h + 2.0, ..l.pill };
    g.fill_path(&Brush::vertical(sheen, &[(0.0, p.shine), (0.55, clear), (1.0, clear)])?, &pill)?;
    let inset = l.border / 2.0;
    pill.set_rounded_rect(l.pill.inset(inset), radius - inset)?;
    g.draw_path(&Pen::new(p.border, l.border)?, &pill)?;
    g.fill_path(&Brush::solid(p.track)?, track)?;
    Ok(())
}

/// The bar (`.pill-fill`): a gradient, greyed out as it mutes, with a soft
/// glow trailing its end, clipped to the track.
fn draw_bar(g: &Graphics, l: &Layout, p: &Palette, track: &Path, frame: &Frame) -> Result<()> {
    let width = l.track.w * frame.percent.clamp(0.0, 100.0) / 100.0;
    if width < 0.5 {
        return Ok(());
    }
    g.clip_to(track)?;
    let drawn = draw_bar_clipped(g, l, p, frame, width);
    g.reset_clip()?;
    drawn
}

fn draw_bar_clipped(g: &Graphics, l: &Layout, p: &Palette, frame: &Frame, width: f32) -> Result<()> {
    let bar = Rect { w: width, ..l.track };
    let radius = bar.h.min(bar.w) / 2.0;
    // The bar's `box-shadow: 0 0 10px` as the track clips it: a glow past the
    // end, fading like a Gaussian; it starts under the round tip.
    let glow = p.glow.fade(0.23 * (1.0 - frame.mute));
    if glow.a > 0.0 && bar.right() < l.track.right() {
        let start = bar.right() - radius;
        let stops = [(0.0, glow), (0.3, glow.fade(0.62)), (0.55, glow.fade(0.32)), (1.0, glow.fade(0.0))];
        let brush = Brush::horizontal(start, bar.right() + l.glow, &stops)?;
        g.fill_rect(&brush, Rect { x: start, w: radius + l.glow, ..l.track })?;
    }
    let muted = p.muted_fill();
    let (from, to) = (p.fill.0.lerp(muted, frame.mute), p.fill.1.lerp(muted, frame.mute));
    let mut path = Path::new()?;
    path.set_rounded_rect(bar, radius)?;
    g.fill_path(&Brush::horizontal(bar.x, bar.right(), &[(0.0, from), (1.0, to)])?, &path)?;
    Ok(())
}

/// The icon glyph, centred in its box; icon fonts draw within the em square.
fn draw_icon(g: &Graphics, l: &Layout, p: &Palette, fonts: &Fonts, frame: &Frame) -> Result<()> {
    let Some(icon) = &fonts.icon else { return Ok(()) };
    let color = p.icon.lerp(p.text_dim, frame.mute);
    let line = icon.ascent + icon.descent;
    let at = Rect {
        x: (l.icon.center_x() - l.icon_em / 2.0).round(),
        y: (l.icon.center_y() - line / 2.0).round(),
        w: l.icon_em,
        h: line,
    };
    g.draw_text(&[glyph(frame)], &icon.font, at, &fonts.centered, &Brush::solid(color)?)?;
    Ok(())
}

/// The percentage (`.pill-value`): the number, then a smaller, dimmer "%"
/// on the same baseline, right-aligned; the line is centred like CSS does.
/// Right-aligned boxes let GDI+ place the digits with the hinted advances it
/// draws with, which at small sizes differ from what it measures.
fn draw_value(g: &Graphics, l: &Layout, p: &Palette, fonts: &Fonts, frame: &Frame) -> Result<()> {
    let (number, unit) = (&fonts.number, &fonts.unit);
    let digits: Vec<u16> = frame.number().to_string().encode_utf16().collect();
    let percent_sign = [u16::from(b'%')];
    let baseline = (l.pill.center_y() + (number.ascent - number.descent) / 2.0).round();
    let digits_right = (l.value_right - g.measure(&percent_sign, &unit.font, &fonts.right)? - l.unit_gap).round();
    let left = l.value_right - 4.0 * l.number_em; // anywhere well left of "100"

    let color = p.text.lerp(p.text_dim, frame.mute);
    let at = Rect { x: left, y: baseline - number.ascent, w: digits_right - left, h: number.line() };
    g.draw_text(&digits, &number.font, at, &fonts.right, &Brush::solid(color)?)?;
    let at = Rect { x: left, y: baseline - unit.ascent, w: l.value_right - left, h: unit.line() };
    g.draw_text(&percent_sign, &unit.font, at, &fonts.right, &Brush::solid(p.text_dim)?)?;
    Ok(())
}

/// A font and where its baseline sits.
pub(crate) struct TextFont {
    // Field order is drop order: the font before its family.
    pub(crate) font: Font,
    _family: FontFamily,
    name: &'static str,
    style: i32,
    /// Cell ascent and descent, in pixels.
    pub(crate) ascent: f32,
    pub(crate) descent: f32,
}

impl TextFont {
    pub(crate) fn new(name: &'static str, style: i32, px: f32) -> gdiplus::Result<TextFont> {
        let family = FontFamily::named(name)?;
        let font = Font::new(&family, px, style)?;
        let (ascent, descent) = family.metrics(style)?;
        Ok(TextFont { font, _family: family, name, style, ascent: ascent * px, descent: descent * px })
    }

    pub(crate) fn line(&self) -> f32 {
        self.ascent + self.descent
    }
}

struct Fonts {
    icon: Option<TextFont>,
    number: TextFont,
    unit: TextFont,
    right: StringFormat,
    centered: StringFormat,
}

impl Fonts {
    fn new(l: &Layout) -> Result<Fonts> {
        let icon = ICON_FONTS.iter().find_map(|name| TextFont::new(name, gdiplus::REGULAR, l.icon_em).ok());
        if icon.is_none() {
            log::warn!("overlay: no icon font (Segoe Fluent Icons or Segoe MDL2 Assets), so no icon");
        }
        let number = NUMBER_FONTS
            .iter()
            .find_map(|&(name, style)| TextFont::new(name, style, l.number_em).ok())
            .ok_or("no font for the overlay's numbers")?;
        let unit = TextFont::new(number.name, number.style, l.unit_em)?;
        log::debug!("overlay fonts: {}, icons from {}", number.name, icon.as_ref().map_or("nowhere", |i| i.name));
        Ok(Fonts {
            icon,
            number,
            unit,
            right: StringFormat::typographic(StringAlignmentFar)?,
            centered: StringFormat::typographic(StringAlignmentCenter)?,
        })
    }
}

/// A DIB section in a memory DC (what `UpdateLayeredWindow` reads) with a
/// GDI+ graphics drawing straight into its pixels.
pub(crate) struct Surface {
    // Field order is drop order: GDI+ lets go of the pixels before the DIB frees them.
    pub(crate) graphics: Graphics,
    _bitmap: gdiplus::Bitmap,
    pub(crate) dib: Dib,
}

impl Surface {
    pub(crate) fn new(width: i32, height: i32) -> Result<Surface> {
        let dib = Dib::new(width, height)?;
        // SAFETY: the DIB's pixels are width × height × 4 bytes and outlive the bitmap (field order).
        let bitmap = unsafe { gdiplus::Bitmap::over_pixels(width, height, dib.bits.cast()) }?;
        let graphics = Graphics::for_bitmap(&bitmap)?;
        Ok(Surface { graphics, _bitmap: bitmap, dib })
    }
}

/// A top-down 32-bit DIB section selected into its own memory DC.
pub(crate) struct Dib {
    pub(crate) dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
    len: usize,
}

impl Dib {
    fn new(width: i32, height: i32) -> Result<Dib> {
        if width <= 0 || height <= 0 {
            return Err(format!("bad overlay size {width}×{height}").into());
        }
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return Err("CreateCompatibleDC failed".into());
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    biHeight: -height, // negative: top row first, as GDI+ expects
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) if !bits.is_null() => bitmap,
                result => {
                    if let Ok(bitmap) = result {
                        let _ = DeleteObject(bitmap);
                    }
                    let _ = DeleteDC(dc);
                    return Err("CreateDIBSection failed".into());
                }
            };
            let previous = SelectObject(dc, bitmap);
            Ok(Dib { dc, bitmap, previous, bits: bits.cast(), len: width as usize * height as usize })
        }
    }

    pub(crate) fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: `bits` is the DIB's width × height pixels, alive until drop.
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.len) }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            let _ = DeleteObject(self.bitmap);
            let _ = DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::color::Color;
    use std::sync::Mutex;

    /// GDI+ start-up isn't documented as thread-safe; tests take turns.
    pub(crate) static GDIPLUS: Mutex<()> = Mutex::new(());

    fn frame(kind: LevelKind, percent: f32, muted: bool) -> Frame {
        Frame { kind, muted, opacity: 1.0, percent, mute: if muted { 1.0 } else { 0.0 } }
    }

    fn argb(p: u32) -> (u32, u32, u32, u32) {
        (p >> 24, (p >> 16) & 0xff, (p >> 8) & 0xff, p & 0xff)
    }

    #[test]
    fn renders_the_pill() {
        let _turn = GDIPLUS.lock().unwrap_or_else(|e| e.into_inner());
        let _gdiplus = gdiplus::Session::start().expect("GDI+");
        let mut r = Renderer::new(Style { dpi: 96, light: false }).expect("renderer");
        r.draw(&frame(LevelKind::Volume, 64.0, false)).expect("draw");
        let has_icon = r.fonts.icon.is_some();
        let l = r.layout.clone();
        let px = r.surface.dib.pixels().to_vec();
        let at = |x: f32, y: f32| argb(px[y as usize * l.width as usize + x as usize]);
        let cy = l.pill.center_y();

        assert_eq!(at(0.0, 0.0), (0, 0, 0, 0), "outside the pill and its shadow");
        let (a, ..) = at(l.pill.x + 10.0, cy);
        assert!((230..=240).contains(&a), "dark glass, slightly see-through: {a}");
        let (a, r_, g_, b_) = at(l.track.x + 10.0, cy);
        assert!(a == 255 && b_ > 200 && b_ > r_ && b_ > g_, "indigo end of the bar: {:?}", (a, r_, g_, b_));
        let (_, r_, _, b_) = at(l.track.right() - 4.0, cy);
        assert!(r_.abs_diff(b_) < 12, "empty track is grey: {r_} {b_}");
        // White text in the value column; the lavender icon in its box.
        let bright = |rect: Rect, test: &dyn Fn((u32, u32, u32, u32)) -> bool| {
            (rect.y as usize..(rect.y + rect.h) as usize)
                .any(|y| (rect.x as usize..rect.right() as usize).any(|x| test(at(x as f32, y as f32))))
        };
        let value = Rect { x: l.value_right - 44.0, y: l.pill.y, w: 44.0, h: l.pill.h };
        assert!(bright(value, &|(a, r, g, b)| a > 200 && r > 200 && g > 200 && b > 200));
        if has_icon {
            assert!(bright(l.icon, &|(a, r, g, b)| a > 200 && b > 200 && r > 150 && g < r));
        }
        // Same content again: nothing is redrawn.
        r.draw(&frame(LevelKind::Volume, 64.0, false)).expect("draw");
        assert_eq!(r.drawn, Some(Content::of(&frame(LevelKind::Volume, 64.0, false))));
    }

    #[test]
    fn redraws_cleanly_thousands_of_times() {
        let _turn = GDIPLUS.lock().unwrap_or_else(|e| e.into_inner());
        let _gdiplus = gdiplus::Session::start().expect("GDI+");
        let mut r = Renderer::new(Style { dpi: 120, light: true }).expect("renderer");
        for i in 0..2000 {
            let kind = if i % 3 == 0 { LevelKind::Brightness } else { LevelKind::Volume };
            r.draw(&frame(kind, (i % 101) as f32, i % 7 == 0)).expect("draw");
        }
    }

    /// Writes sample renders to `%TEMP%\pady-overlay-preview`, over a
    /// wallpaper-ish backdrop, for eyeballing:
    /// `cargo test -p pady-overlay -- --ignored preview`.
    #[test]
    #[ignore]
    fn preview() {
        let _turn = GDIPLUS.lock().unwrap_or_else(|e| e.into_inner());
        let _gdiplus = gdiplus::Session::start().expect("GDI+");
        let dir = std::env::temp_dir().join("pady-overlay-preview");
        std::fs::create_dir_all(&dir).expect("preview dir");
        let samples = [
            ("volume", frame(LevelKind::Volume, 64.0, false)),
            ("muted", frame(LevelKind::Volume, 64.0, true)),
            ("brightness", frame(LevelKind::Brightness, 100.0, false)),
            ("low", frame(LevelKind::Volume, 3.0, false)),
        ];
        for dpi in [96, 120, 144, 192] {
            for light in [false, true] {
                let mut r = Renderer::new(Style { dpi, light }).expect("renderer");
                for (name, f) in &samples {
                    r.draw(f).expect("draw");
                    let theme = if light { "light" } else { "dark" };
                    let path = dir.join(format!("{name}-{theme}-{dpi}.png"));
                    save_over_backdrop(&mut r, light, &path);
                }
            }
        }
        println!("previews in {}", dir.display());
    }

    fn save_over_backdrop(r: &mut Renderer, light: bool, path: &std::path::Path) {
        let (w, h) = (r.layout.width, r.layout.height);
        let (top, bottom) = if light {
            (Color::rgb(230, 233, 255), Color::rgb(248, 240, 255))
        } else {
            (Color::rgb(60, 64, 120), Color::rgb(14, 15, 24))
        };
        let mut px = r.surface.dib.pixels().to_vec();
        for (i, p) in px.iter_mut().enumerate() {
            let bg = top.lerp(bottom, (i / w as usize) as f32 / h as f32);
            let (a, sr, sg, sb) = argb(*p);
            let k = 1.0 - a as f32 / 255.0;
            let mix = |s: u32, b: f32| (s as f32 + b * k).round().min(255.0) as u32;
            *p = 0xff00_0000 | mix(sr, bg.r) << 16 | mix(sg, bg.g) << 8 | mix(sb, bg.b);
        }
        let bitmap = unsafe { gdiplus::Bitmap::over_pixels(w, h, px.as_mut_ptr().cast()) }.expect("bitmap");
        bitmap.save_png(path).expect("png");
    }
}
