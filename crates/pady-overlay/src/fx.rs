//! Windows: the swipe glow at the screen edge and the "show touches" pad,
//! two click-through layered windows on one thread of their own.
//!
//! Smoothness comes from doing almost nothing per frame. The glow for an edge
//! is drawn once, when a swipe towards it starts; after that a frame only
//! hands Windows the same bitmap with a new opacity and position. The touch
//! pad stamps a few fingertips onto a pre-drawn background. While anything
//! moves, frames are paced by the compositor (`DwmFlush`); otherwise the
//! thread sleeps in `GetMessageW` and costs nothing.

use crate::fx_draw::{compose_touches, edge_glow, fingertip, orient, pad_background, Edge, EdgeMotion};
use crate::layout::{px, scale_for};
use crate::look::look;
use crate::windows::{monitor_under_cursor, Dib};
use std::collections::HashMap;
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromPoint, AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::*;

const WM_FX: u32 = WM_APP;
const WM_STOP: u32 = WM_APP + 1;

/// How far the glow reaches into the screen, in logical pixels.
const GLOW_DEPTH: f32 = 160.0;
/// The touch pad's width and the gap to the screen corner, logical pixels.
const PAD_WIDTH: f32 = 220.0;
const PAD_MARGIN: f32 = 24.0;
const PAD_INSET: f32 = 12.0;
const TIP: f32 = 34.0;

pub(crate) enum Msg {
    Progress(Edge, f32),
    Ended(bool),
    /// Fingertips in millimetres, and the touchpad's size in millimetres.
    Touches(Vec<(f32, f32)>, (f32, f32)),
}

pub struct Effects {
    hwnd: isize,
    inbox: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

impl Effects {
    pub fn spawn() -> Option<Effects> {
        let (inbox, messages) = mpsc::channel();
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let thread = match std::thread::Builder::new().name("pady-fx".into()).spawn(move || run(messages, ready_tx)) {
            Ok(thread) => thread,
            Err(e) => {
                log::warn!("animations unavailable: could not start their thread: {e}");
                return None;
            }
        };
        match ready.recv() {
            Ok(Ok(hwnd)) => Some(Effects { hwnd, inbox, thread: Some(thread) }),
            Ok(Err(e)) => {
                log::warn!("animations unavailable: {e}");
                let _ = thread.join();
                None
            }
            Err(_) => {
                let _ = thread.join();
                None
            }
        }
    }

    pub(crate) fn send(&self, msg: Msg) {
        if self.inbox.send(msg).is_ok() {
            let _ = unsafe { PostMessageW(HWND(self.hwnd as *mut c_void), WM_FX, WPARAM(0), LPARAM(0)) };
        }
    }
}

impl Drop for Effects {
    fn drop(&mut self) {
        let stopping = unsafe { PostMessageW(HWND(self.hwnd as *mut c_void), WM_STOP, WPARAM(0), LPARAM(0)) }.is_ok();
        if let Some(thread) = self.thread.take() {
            if stopping {
                let _ = thread.join();
            }
        }
    }
}

fn run(messages: Receiver<Msg>, ready: SyncSender<std::result::Result<isize, String>>) {
    unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let (glow, pad) = match (create_window(), create_window()) {
        (Ok(glow), Ok(pad)) => (glow, pad),
        (glow, pad) => {
            for hwnd in [glow, pad].into_iter().flatten() {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            let _ = ready.send(Err("could not create their windows".into()));
            return;
        }
    };
    if ready.send(Ok(glow.0 as isize)).is_err() {
        return;
    }
    let mut fx = Fx::new(glow, pad);
    let mut msg = MSG::default();
    'outer: loop {
        if fx.animating() {
            while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.as_bool() {
                if !fx.handle(&msg, &messages) {
                    break 'outer;
                }
            }
            if unsafe { DwmFlush() }.is_err() {
                std::thread::sleep(Duration::from_millis(7));
            }
            fx.step();
        } else {
            let got = unsafe { GetMessageW(&mut msg, None, 0, 0) }.0;
            if got == 0 || got == -1 || !fx.handle(&msg, &messages) {
                break;
            }
        }
    }
    unsafe {
        let _ = DestroyWindow(glow);
        let _ = DestroyWindow(pad);
    }
}

fn create_window() -> windows::core::Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: w!("PadyFx"),
            ..Default::default()
        };
        RegisterClassExW(&class); // fails harmlessly for the second window
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
            w!("PadyFx"),
            w!("Pady effects"),
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
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// The monitor under the cursor: its full rectangle and DPI.
fn monitor_under_cursor_rect() -> (RECT, u32) {
    unsafe {
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let rect = if GetMonitorInfoW(monitor, &mut info).as_bool() {
            info.rcMonitor
        } else {
            RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) }
        };
        let (mut dpi, mut dpi_y) = (0, 0);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y).is_err() {
            dpi = 96;
        }
        (rect, dpi)
    }
}

struct Layer {
    dib: Dib,
    w: i32,
    h: i32,
}

fn push(hwnd: HWND, layer: &Layer, at: (i32, i32), opacity: f32) {
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let size = SIZE { cx: layer.w, cy: layer.h };
    let (origin, target) = (POINT::default(), POINT { x: at.0, y: at.1 });
    let result = unsafe {
        UpdateLayeredWindow(
            hwnd,
            None,
            Some(&target as *const POINT),
            Some(&size as *const SIZE),
            layer.dib.dc,
            Some(&origin as *const POINT),
            COLORREF(0),
            Some(&blend as *const BLENDFUNCTION),
            ULW_ALPHA,
        )
    };
    if let Err(e) = result {
        log::debug!("animation frame: {e}");
    }
}

fn show(hwnd: HWND, on: bool) {
    unsafe {
        if on {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags);
        } else {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

struct Pad {
    layer: Layer,
    bg: Vec<u32>,
    tip: Vec<u32>,
    at: (i32, i32),
    inset: f32,
    dpi: u32,
}

struct Fx {
    glow_hwnd: HWND,
    pad_hwnd: HWND,
    last: Instant,
    // The swipe glow.
    motion: EdgeMotion,
    edge: Option<Edge>,
    monitor: RECT,
    /// Which cached glow is showing: edge, length and depth in pixels.
    glow_key: Option<(Edge, i32, i32)>,
    glows: HashMap<(Edge, i32, i32), Layer>,
    glow_shown: bool,
    // Show touches.
    pad: Option<Pad>,
    points: Vec<(f32, f32)>,
    pad_mm: (f32, f32),
    touching: bool,
    touch_alpha: f32,
    pad_dirty: bool,
    pad_shown: bool,
}

impl Fx {
    fn new(glow_hwnd: HWND, pad_hwnd: HWND) -> Fx {
        Fx {
            glow_hwnd,
            pad_hwnd,
            last: Instant::now(),
            motion: EdgeMotion::default(),
            edge: None,
            monitor: RECT::default(),
            glow_key: None,
            glows: HashMap::new(),
            glow_shown: false,
            pad: None,
            points: Vec::new(),
            pad_mm: (100.0, 60.0),
            touching: false,
            touch_alpha: 0.0,
            pad_dirty: false,
            pad_shown: false,
        }
    }

    fn animating(&self) -> bool {
        self.motion.active() || self.touching || self.touch_alpha > 0.0
    }

    /// `false` when the thread should stop.
    fn handle(&mut self, msg: &MSG, messages: &Receiver<Msg>) -> bool {
        if msg.message == WM_QUIT || (msg.hwnd == self.glow_hwnd && msg.message == WM_STOP) {
            return false;
        }
        if msg.hwnd == self.glow_hwnd && msg.message == WM_FX {
            let was_animating = self.animating();
            for m in messages.try_iter() {
                self.apply(m);
            }
            if !was_animating {
                self.last = Instant::now(); // no giant first step after sleeping
            }
            return true;
        }
        unsafe { DispatchMessageW(msg) };
        true
    }

    fn apply(&mut self, msg: Msg) {
        match msg {
            Msg::Progress(edge, amount) => {
                if !self.motion.active() || self.edge != Some(edge) {
                    self.prepare_glow(edge);
                }
                self.motion.progress(amount);
            }
            Msg::Ended(fired) => self.motion.end(fired),
            Msg::Touches(points, mm) => {
                self.touching = !points.is_empty();
                if self.touching {
                    self.points = points;
                    self.pad_mm = mm;
                    if self.pad.is_none() || !self.pad_shown {
                        self.prepare_pad();
                    }
                }
                self.pad_dirty = true;
            }
        }
    }

    /// Draws (or finds) the glow for `edge` on the monitor under the cursor.
    fn prepare_glow(&mut self, edge: Edge) {
        let (monitor, dpi) = monitor_under_cursor_rect();
        let depth = px(GLOW_DEPTH, scale_for(dpi)) as i32;
        let len = match edge {
            Edge::Top | Edge::Bottom => monitor.right - monitor.left,
            Edge::Left | Edge::Right => monitor.bottom - monitor.top,
        };
        self.monitor = monitor;
        self.edge = Some(edge);
        self.glow_key = None;
        if len <= 0 || depth <= 0 {
            return;
        }
        let key = (edge, len, depth);
        if !self.glows.contains_key(&key) {
            // A handful of monitors × edges at most; drop old ones if displays change a lot.
            if self.glows.len() >= 8 {
                self.glows.clear();
            }
            let top = edge_glow(len as usize, depth as usize);
            let (pixels, w, h) = orient(&top, len as usize, depth as usize, edge);
            match Dib::new(w as i32, h as i32) {
                Ok(mut dib) => {
                    dib.pixels().copy_from_slice(&pixels);
                    self.glows.insert(key, Layer { dib, w: w as i32, h: h as i32 });
                }
                Err(e) => log::debug!("swipe glow: {e}"),
            }
        }
        if self.glows.contains_key(&key) {
            self.glow_key = Some(key);
        }
    }

    fn prepare_pad(&mut self) {
        let (work, dpi) = monitor_under_cursor();
        let s = scale_for(dpi);
        let (mw, mh) = (self.pad_mm.0.max(1.0), self.pad_mm.1.max(1.0));
        let w = px(PAD_WIDTH, s) as i32;
        let h = (w as f32 * (mh / mw)).clamp(px(110.0, s), px(170.0, s)).round() as i32;
        let margin = px(PAD_MARGIN, s) as i32;
        let at = (work.right - margin - w, work.bottom - margin - h);
        if self.pad.as_ref().is_some_and(|p| p.dpi == dpi && p.layer.w == w && p.layer.h == h) {
            if let Some(pad) = &mut self.pad {
                pad.at = at;
            }
            return;
        }
        let tip_size = px(TIP, s) as usize | 1;
        match Dib::new(w, h) {
            Ok(dib) => {
                self.pad = Some(Pad {
                    layer: Layer { dib, w, h },
                    bg: pad_background(w as usize, h as usize, s),
                    tip: fingertip(tip_size),
                    at,
                    inset: px(PAD_INSET, s),
                    dpi,
                });
            }
            Err(e) => log::debug!("touch pad: {e}"),
        }
    }

    fn step(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f32().min(0.05);
        self.last = now;
        self.draw(dt * look().speed);
        log::trace!(
            "fx frame: {:.2} ms since the last, drawn in {:.2} ms (glow {:.2}, touches {:.2})",
            dt * 1000.0,
            now.elapsed().as_secs_f64() * 1000.0,
            self.motion.intensity(),
            self.touch_alpha
        );
    }

    fn draw(&mut self, dt: f32) {
        // The glow: same bitmap every frame, only opacity and position change.
        self.motion.step(dt);
        let layer = self.glow_key.and_then(|key| self.glows.get(&key).map(|l| (key.0, l)));
        // The swipe glow is all glow: its strength scales it, and 0 hides it.
        let intensity = (self.motion.intensity() * look().glow).min(1.0 / 0.92);
        match layer {
            Some((edge, layer)) if self.motion.active() && intensity > 0.0 => {
                let m = self.monitor;
                let tuck = (self.motion.tucked()
                    * if matches!(edge, Edge::Top | Edge::Bottom) { layer.h } else { layer.w } as f32)
                    .round() as i32;
                let at = match edge {
                    Edge::Top => (m.left, m.top - tuck),
                    Edge::Bottom => (m.left, m.bottom - layer.h + tuck),
                    Edge::Left => (m.left - tuck, m.top),
                    Edge::Right => (m.right - layer.w + tuck, m.top),
                };
                push(self.glow_hwnd, layer, at, intensity * 0.92);
                if !self.glow_shown {
                    show(self.glow_hwnd, true);
                    self.glow_shown = true;
                }
            }
            _ => {
                if self.glow_shown {
                    show(self.glow_hwnd, false);
                    self.glow_shown = false;
                }
            }
        }

        // Touches: fade in quickly, linger and fade out after the fingers lift.
        let target = if self.touching { 1.0 } else { 0.0 };
        let tau = if self.touching { 0.06 } else { 0.28 };
        let before = self.touch_alpha;
        self.touch_alpha += (target - self.touch_alpha) * (1.0 - (-dt / tau).exp());
        if !self.touching && self.touch_alpha < 0.01 {
            self.touch_alpha = 0.0;
        }
        let Some(pad) = &mut self.pad else { return };
        if self.touch_alpha <= 0.0 {
            if self.pad_shown {
                show(self.pad_hwnd, false);
                self.pad_shown = false;
            }
            return;
        }
        if self.pad_dirty || (self.touch_alpha - before).abs() > 0.001 {
            let (w, h) = (pad.layer.w as usize, pad.layer.h as usize);
            let (sx, sy) = (
                (w as f32 - 2.0 * pad.inset) / self.pad_mm.0.max(1.0),
                (h as f32 - 2.0 * pad.inset) / self.pad_mm.1.max(1.0),
            );
            let points: Vec<(f32, f32)> =
                self.points.iter().map(|&(x, y)| (pad.inset + x * sx, pad.inset + y * sy)).collect();
            let fingers = if self.touching { self.touch_alpha.max(0.0) } else { 1.0 };
            compose_touches(pad.layer.dib.pixels(), &pad.bg, (w, h), &pad.tip, &points, fingers);
            self.pad_dirty = false;
        }
        push(self.pad_hwnd, &pad.layer, pad.at, self.touch_alpha);
        if !self.pad_shown {
            show(self.pad_hwnd, true);
            self.pad_shown = true;
        }
    }
}
