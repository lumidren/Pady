//! Where everything goes, in physical pixels, for a monitor's DPI.
//!
//! Sizes are given in logical (96 DPI) pixels, copied from the WebView pill's
//! CSS (`pady-app/ui/overlay.css`), then scaled and snapped to whole pixels so
//! straight edges stay crisp at 125 % and 150 % as well as 100 % and 200 %.

/// The transparent window around the pill, roomy enough for its shadow.
const WINDOW: (f32, f32) = (300.0, 90.0);
/// The pill itself (`.pill { width: 260px; height: 50px }`).
const PILL: (f32, f32) = (260.0, 50.0);
/// Gap between the window's bottom edge and the bottom of the work area.
const BOTTOM_MARGIN: f32 = 110.0;

/// A rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.w / 2.0
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.h / 2.0
    }

    /// Shrunk by `d` on every side (grown when negative), never below zero size.
    pub fn inset(&self, d: f32) -> Rect {
        Rect { x: self.x + d, y: self.y + d, w: (self.w - 2.0 * d).max(0.0), h: (self.h - 2.0 * d).max(0.0) }
    }
}

/// A monitor's work area, as Win32 reports it (right and bottom exclusive).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkArea {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Physical pixels per logical pixel; `0` (an unknown DPI) counts as 96.
pub fn scale_for(dpi: u32) -> f32 {
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

/// A logical length in physical pixels, snapped to a whole pixel.
pub fn px(logical: f32, scale: f32) -> f32 {
    (logical * scale).round()
}

/// The pill's geometry at one DPI.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub scale: f32,
    /// Window size.
    pub width: i32,
    pub height: i32,
    /// The pill, centred in the window, on whole pixels.
    pub pill: Rect,
    /// One logical pixel, snapped down but never thinner than a physical
    /// pixel, the way browsers draw CSS borders.
    pub border: f32,
    /// The square the icon is centred in, and the icon font's size.
    pub icon: Rect,
    pub icon_em: f32,
    pub track: Rect,
    /// Right edge of the percentage text.
    pub value_right: f32,
    /// Font sizes of the number and of its smaller "%".
    pub number_em: f32,
    pub unit_em: f32,
    /// Space between the number and the "%".
    pub unit_gap: f32,
    /// How far the bar's glow reaches past its end.
    pub glow: f32,
}

impl Layout {
    pub fn new(dpi: u32) -> Layout {
        let s = scale_for(dpi);
        let (width, height) = (px(WINDOW.0, s), px(WINDOW.1, s));
        let (pill_w, pill_h) = (px(PILL.0, s), px(PILL.1, s));
        let pill =
            Rect { x: ((width - pill_w) / 2.0).floor(), y: ((height - pill_h) / 2.0).floor(), w: pill_w, h: pill_h };

        // Inside the pill, left to right (CSS flexbox: 1px border, 16px left
        // padding, 24px icon, 14px gap, track, 14px gap, 44px value column,
        // 18px right padding): offsets from the pill's left edge.
        let icon_size = px(24.0, s);
        let icon = Rect {
            x: pill.x + px(17.0, s),
            y: (pill.center_y() - icon_size / 2.0).round(),
            w: icon_size,
            h: icon_size,
        };
        let track_h = px(6.0, s);
        let (track_left, track_right) = (pill.x + px(55.0, s), pill.x + px(183.0, s));
        let track = Rect {
            x: track_left,
            y: pill.y + ((pill_h - track_h) / 2.0).floor(),
            w: track_right - track_left,
            h: track_h,
        };

        Layout {
            scale: s,
            width: width as i32,
            height: height as i32,
            pill,
            border: s.floor().max(1.0),
            icon,
            icon_em: px(20.0, s),
            track,
            value_right: pill.x + px(241.0, s),
            number_em: px(17.0, s),
            unit_em: px(12.0, s),
            unit_gap: px(1.0, s),
            glow: 10.0 * s,
        }
    }
}

/// Top-left corner for a `width`×`height` window: centred in the work area,
/// its bottom edge 110 logical px above the work area's bottom (clear of
/// the taskbar), and never outside the work area however small it is.
pub fn place(work: WorkArea, width: i32, height: i32, scale: f32) -> (i32, i32) {
    let x = work.left + (work.right - work.left - width) / 2;
    let y = work.bottom - height - px(BOTTOM_MARGIN, scale) as i32;
    let x = x.min(work.right - width).max(work.left);
    let y = y.min(work.bottom - height).max(work.top);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn matches_the_css_at_100_percent() {
        let l = Layout::new(96);
        assert_eq!((l.scale, l.width, l.height), (1.0, 300, 90));
        assert_eq!(l.pill, r(20.0, 20.0, 260.0, 50.0));
        assert_eq!(l.border, 1.0);
        assert_eq!(l.icon, r(37.0, 33.0, 24.0, 24.0));
        assert_eq!(l.track, r(75.0, 42.0, 128.0, 6.0));
        assert_eq!(l.value_right, 261.0);
        assert_eq!((l.icon_em, l.number_em, l.unit_em, l.unit_gap), (20.0, 17.0, 12.0, 1.0));
        // The value column (44 px) ends 18 px of padding + 1 px of border before the pill does.
        assert_eq!(l.pill.right() - l.value_right, 19.0);
    }

    #[test]
    fn doubles_at_200_percent() {
        let (a, b) = (Layout::new(96), Layout::new(192));
        assert_eq!((b.width, b.height), (a.width * 2, a.height * 2));
        for (small, big) in [(a.pill, b.pill), (a.icon, b.icon), (a.track, b.track)] {
            assert_eq!(big, r(small.x * 2.0, small.y * 2.0, small.w * 2.0, small.h * 2.0));
        }
        assert_eq!(b.border, 2.0);
        assert_eq!(b.number_em, 34.0);
    }

    #[test]
    fn snaps_to_whole_pixels_at_fractional_scales() {
        for dpi in [96, 108, 120, 144, 168, 192, 240] {
            let l = Layout::new(dpi);
            for rect in [l.pill, l.icon, l.track] {
                for v in [rect.x, rect.y, rect.w, rect.h] {
                    assert_eq!(v.fract(), 0.0, "{dpi} dpi: {rect:?}");
                }
            }
            // Everything stays inside the window, in order, and roughly where the CSS puts it.
            assert!(l.pill.x >= 0.0 && l.pill.right() <= l.width as f32);
            assert!(l.pill.y >= 0.0 && l.pill.y + l.pill.h <= l.height as f32);
            assert!(l.icon.right() < l.track.x && l.track.right() < l.value_right);
            assert!((l.track.w - 128.0 * l.scale).abs() <= 1.0);
            assert!((l.track.center_y() - l.pill.center_y()).abs() <= 0.5);
            assert!(l.border >= 1.0);
        }
    }

    #[test]
    fn scales_at_125_percent() {
        let l = Layout::new(120);
        assert_eq!((l.width, l.height), (375, 113));
        assert_eq!(l.pill, r(25.0, 25.0, 325.0, 63.0));
        assert_eq!(l.border, 1.0);
        assert_eq!(l.track.h, 8.0);
    }

    #[test]
    fn unknown_dpi_is_treated_as_96() {
        assert_eq!(scale_for(0), 1.0);
        assert_eq!(Layout::new(0), Layout::new(96));
    }

    #[test]
    fn inset_shrinks_and_never_inverts() {
        assert_eq!(r(0.0, 0.0, 10.0, 6.0).inset(0.5), r(0.5, 0.5, 9.0, 5.0));
        assert_eq!(r(0.0, 0.0, 10.0, 6.0).inset(4.0).h, 0.0);
    }

    const FULL_HD: WorkArea = WorkArea { left: 0, top: 0, right: 1920, bottom: 1032 };

    #[test]
    fn places_bottom_centre_above_the_taskbar() {
        assert_eq!(place(FULL_HD, 300, 90, 1.0), (810, 1032 - 90 - 110));
    }

    #[test]
    fn scales_the_bottom_margin() {
        let l = Layout::new(144);
        let (x, y) = place(FULL_HD, l.width, l.height, l.scale);
        assert_eq!((l.width, l.height), (450, 135));
        assert_eq!((x, y), ((1920 - 450) / 2, 1032 - 135 - 165));
    }

    #[test]
    fn follows_monitors_left_of_and_above_the_primary() {
        let work = WorkArea { left: -2560, top: -400, right: 0, bottom: 1000 };
        assert_eq!(place(work, 300, 90, 1.0), (-2560 + 1130, 1000 - 200));
    }

    #[test]
    fn stays_inside_a_tiny_work_area() {
        let work = WorkArea { left: 100, top: 100, right: 300, bottom: 200 };
        let (x, y) = place(work, 300, 90, 1.0);
        assert_eq!((x, y), (100, 100));
        let roomy_but_short = WorkArea { left: 0, top: 50, right: 1000, bottom: 200 };
        assert_eq!(place(roomy_but_short, 300, 90, 1.0), (350, 50));
    }
}
