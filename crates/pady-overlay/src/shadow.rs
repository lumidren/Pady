//! Soft drop shadows like CSS `box-shadow`, around the pill.
//!
//! GDI+ has no cheap blur, so shadows are computed here from the distance to
//! the pill's edge: a Gaussian-blurred straight edge is exactly `erfc`, and
//! the pill's ends are round and wide enough for that to look right. The
//! result only depends on DPI and theme, so it is computed once per style
//! into the bitmap's static layer, not per frame.

use crate::color::Color;
use crate::layout::Rect;

/// CSS `box-shadow: 0 <dy> <blur> <spread> <color>`, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub dy: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: Color,
}

/// Signed distance from `(x, y)` to a rectangle with rounded corners of
/// `radius`: negative inside, positive outside.
pub fn rounded_rect_distance(x: f32, y: f32, rect: Rect, radius: f32) -> f32 {
    let (half_w, half_h) = (rect.w / 2.0, rect.h / 2.0);
    let r = radius.min(half_w).min(half_h).max(0.0);
    let qx = (x - rect.center_x()).abs() - (half_w - r);
    let qy = (y - rect.center_y()).abs() - (half_h - r);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    let inside = qx.max(qy).min(0.0);
    outside + inside - r
}

/// How much of a pixel whose centre is `d` from a shape's edge the shape
/// covers (the usual one-pixel antialiasing ramp).
pub fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// Complementary error function (Abramowitz & Stegun 7.1.26; error < 2e-7).
fn erfc(x: f32) -> f32 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let poly = t * (0.254_829_6 + t * (-0.284_496_7 + t * (1.421_413_8 + t * (-1.453_152 + t * 1.061_405_4))));
    let tail = poly * (-z * z).exp();
    if x >= 0.0 {
        tail
    } else {
        2.0 - tail
    }
}

/// Premultiplied `0xAARRGGBB` pixels, row by row, of `shadows` cast by the
/// rounded rectangle `shape` in a `width`×`height` bitmap. `shape` and
/// `radius` are physical pixels; `shadows` are logical and scaled by
/// `scale`. Like CSS, nothing is drawn where the shape itself will be.
pub fn render(width: usize, height: usize, shape: Rect, radius: f32, shadows: &[Shadow], scale: f32) -> Vec<u32> {
    let mut pixels = vec![0u32; width * height];
    if width == 0 {
        return pixels;
    }
    // Each shadow's own shape: the pill grown by `spread` and moved down by `dy`.
    let casts: Vec<(Rect, f32, f32, Color)> = shadows
        .iter()
        .map(|s| {
            let spread = s.spread * scale;
            let rect = Rect { y: shape.y + s.dy * scale, ..shape }.inset(-spread);
            // erfc(d / (σ√2)) with σ = blur / 2, as CSS defines blur.
            let falloff = 1.0 / ((s.blur * scale / 2.0).max(0.01) * std::f32::consts::SQRT_2);
            (rect, (radius + spread).max(0.0), falloff, s.color)
        })
        .collect();

    for (i, pixel) in pixels.iter_mut().enumerate() {
        let (x, y) = ((i % width) as f32 + 0.5, (i / width) as f32 + 0.5);
        let outside = 1.0 - coverage(rounded_rect_distance(x, y, shape, radius));
        if outside <= 0.0 {
            continue;
        }
        // Composite the shadows premultiplied, each under the ones before it.
        let mut acc = [0.0f32; 4];
        for (rect, r, falloff, color) in &casts {
            let d = rounded_rect_distance(x, y, *rect, *r);
            let a = color.a * 0.5 * erfc(d * falloff) * outside * (1.0 - acc[3]);
            acc[0] += color.r * a;
            acc[1] += color.g * a;
            acc[2] += color.b * a;
            acc[3] += a;
        }
        if acc[3] > 0.0 {
            let a = acc[3];
            *pixel = Color { r: acc[0] / a, g: acc[1] / a, b: acc[2] / a, a }.premultiplied();
        }
    }
    pixels
}

#[cfg(test)]
mod tests {
    use super::*;

    const PILL: Rect = Rect { x: 20.0, y: 20.0, w: 260.0, h: 50.0 };
    const SHADOW: Shadow = Shadow { dy: 8.0, blur: 22.0, spread: -10.0, color: Color::rgba(0, 0, 0, 0.65) };

    fn alpha(p: u32) -> u32 {
        p >> 24
    }

    #[test]
    fn distance_is_signed() {
        assert_eq!(rounded_rect_distance(150.0, 45.0, PILL, 25.0), -25.0);
        assert_eq!(rounded_rect_distance(150.0, 10.0, PILL, 25.0), 10.0);
        assert_eq!(rounded_rect_distance(10.0, 45.0, PILL, 25.0), 10.0);
        // Round ends: a corner of the bounding box is outside the pill.
        assert!(
            (rounded_rect_distance(20.0, 20.0, PILL, 25.0) - (25.0 * std::f32::consts::SQRT_2 - 25.0)).abs() < 1e-3
        );
    }

    #[test]
    fn coverage_ramps_over_one_pixel() {
        assert_eq!(coverage(-3.0), 1.0);
        assert_eq!(coverage(0.0), 0.5);
        assert_eq!(coverage(3.0), 0.0);
    }

    #[test]
    fn erfc_matches_known_values() {
        for (x, want) in [(0.0, 1.0), (0.5, 0.479_500), (1.0, 0.157_299), (2.0, 0.004_678), (-1.0, 1.842_701)] {
            assert!((erfc(x) - want).abs() < 1e-5, "erfc({x}) = {}", erfc(x));
        }
    }

    #[test]
    fn falls_below_the_pill_and_fades_with_distance() {
        let px = render(300, 90, PILL, 25.0, &[SHADOW], 1.0);
        let at = |x: usize, y: usize| alpha(px[y * 300 + x]);
        // Shifted down: darker under the pill than above it.
        assert!(at(150, 75) > at(150, 14));
        // Fades out away from the pill, and is gone at the window's corner.
        assert!(at(150, 72) > at(150, 80) && at(150, 80) > at(150, 88));
        assert_eq!(at(0, 0), 0);
        // Never more opaque than the shadow colour.
        assert!(px.iter().all(|&p| alpha(p) <= 166));
    }

    #[test]
    fn nothing_under_the_pill() {
        let px = render(300, 90, PILL, 25.0, &[SHADOW], 1.0);
        assert_eq!(px[45 * 300 + 150], 0);
        assert_eq!(px[25 * 300 + 30 + 20], 0);
    }

    #[test]
    fn scales_with_dpi() {
        let pill2 = Rect { x: 40.0, y: 40.0, w: 520.0, h: 100.0 };
        let (one, two) = (render(300, 90, PILL, 25.0, &[SHADOW], 1.0), render(600, 180, pill2, 50.0, &[SHADOW], 2.0));
        // The same spot, twice as far out at twice the scale, is about as dark.
        let (a, b) = (alpha(one[78 * 300 + 150]) as i32, alpha(two[157 * 600 + 300]) as i32);
        assert!((a - b).abs() <= 3, "{a} vs {b}");
    }

    #[test]
    fn stacks_shadows_and_keeps_their_colour() {
        let tint = Shadow { color: Color::rgba(20, 22, 40, 0.38), ..SHADOW };
        let px = render(300, 90, PILL, 25.0, &[tint, Shadow { dy: 2.0, blur: 6.0, spread: -2.0, ..tint }], 1.0);
        let p = px[72 * 300 + 150];
        let (a, b) = (alpha(p), p & 0xff);
        assert!(a > 0);
        // Premultiplied blue stays in proportion to the colour's blue (40/255 of alpha).
        assert!((b as f32 - a as f32 * 40.0 / 255.0).abs() <= 1.0);
    }

    #[test]
    fn empty_bitmap() {
        assert!(render(0, 0, PILL, 25.0, &[SHADOW], 1.0).is_empty());
    }
}
