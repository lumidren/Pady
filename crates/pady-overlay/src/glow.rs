//! The animated aura around the corner notices: a soft purple glow outside
//! the pill and a thin bright rim on its edge, with colours that travel
//! around it.
//!
//! Everything that depends on geometry (distance to the edge, angle around
//! the centre) is worked out once per size; a frame is then one pass of
//! table lookups and blending, well under a millisecond, so the glow can
//! turn every frame without ever holding up the compositor.

use crate::layout::Rect;
use crate::shadow::rounded_rect_distance;

/// Colours around the ring (indigo, purple, pink, violet), back to the start.
const STOPS: [(f32, [f32; 3]); 5] = [
    (0.0, [99.0, 102.0, 241.0]),
    (0.25, [168.0, 85.0, 247.0]),
    (0.5, [236.0, 72.0, 153.0]),
    (0.75, [139.0, 92.0, 246.0]),
    (1.0, [99.0, 102.0, 241.0]),
];

/// How far the glow reaches, in logical pixels: a wide soft halo and a
/// tighter bright one.
const WIDE: f32 = 18.0;
const TIGHT: f32 = 6.0;
/// The rim's width on the pill's edge, in logical pixels.
const RIM: f32 = 1.6;

fn ring_colour(at: f32) -> [f32; 3] {
    let at = at.rem_euclid(1.0);
    for pair in STOPS.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if at <= b {
            let t = (at - a) / (b - a);
            return [ca[0] + (cb[0] - ca[0]) * t, ca[1] + (cb[1] - ca[1]) * t, ca[2] + (cb[2] - ca[2]) * t];
        }
    }
    STOPS[0].1
}

pub struct GlowField {
    /// Glow strength under the pill, 0–65535, per pixel.
    halo: Vec<u16>,
    /// Rim strength over the pill's edge, 0–255, per pixel.
    rim: Vec<u8>,
    /// Position around the pill, 0–255, per pixel.
    angle: Vec<u8>,
    /// The ring's colours, one per angle step.
    colours: [[f32; 3]; 256],
}

impl GlowField {
    /// For a `width`×`height` bitmap with the pill at `pill` (physical pixels).
    pub fn new(width: usize, height: usize, pill: Rect, radius: f32, scale: f32) -> GlowField {
        let n = width * height;
        let (mut halo, mut rim, mut angle) = (vec![0u16; n], vec![0u8; n], vec![0u8; n]);
        let (cx, cy) = (pill.center_x(), pill.center_y());
        let (hw, hh) = (pill.w / 2.0, pill.h / 2.0);
        for y in 0..height {
            for x in 0..width {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let d = rounded_rect_distance(px, py, pill, radius) / scale;
                let i = y * width + x;
                if d > 0.0 {
                    let wide = (-(d / WIDE).powi(2)).exp() * 0.62;
                    let tight = (-(d / TIGHT).powi(2)).exp() * 0.42;
                    halo[i] = ((wide + tight).min(1.0) * 65535.0) as u16;
                }
                // The rim sits just inside the edge, fading over its width.
                let r = 1.0 - ((d + RIM / 2.0).abs() / (RIM / 2.0 + 0.5));
                if r > 0.0 {
                    rim[i] = (r.min(1.0) * 255.0) as u8;
                }
                let turn = ((py - cy) / hh).atan2((px - cx) / hw) / std::f32::consts::TAU + 0.5;
                angle[i] = (turn * 256.0) as u8;
            }
        }
        let mut colours = [[0.0; 3]; 256];
        for (k, c) in colours.iter_mut().enumerate() {
            *c = ring_colour(k as f32 / 256.0);
        }
        GlowField { halo, rim, angle, colours }
    }

    /// Writes a frame: the glow, then the pill (`pill`, premultiplied
    /// `0xAARRGGBB`), then the rim. `strength` scales the glow (1 = normal),
    /// `turn` (0–1) rotates the colours around the pill.
    pub fn paint(&self, out: &mut [u32], pill: &[u32], strength: f32, turn: f32) {
        let shift = (turn.rem_euclid(1.0) * 256.0) as u8;
        let rim_strength = strength.clamp(0.0, 1.25) * 0.85;
        for (i, px) in out.iter_mut().enumerate() {
            let [mut a, mut r, mut g, mut b] = unpack(pill[i]);
            let halo = self.halo[i] as f32 / 65535.0 * strength;
            let rim = self.rim[i] as f32 / 255.0 * rim_strength;
            if halo > 0.0 || rim > 0.0 {
                let c = self.colours[self.angle[i].wrapping_add(shift) as usize];
                // The glow goes under the pill...
                let ha = halo.min(1.0);
                let k = 1.0 - a / 255.0;
                (r, g, b, a) = (r + c[0] * ha * k, g + c[1] * ha * k, b + c[2] * ha * k, a + 255.0 * ha * k);
                // ...and the rim over it.
                let ra = rim.min(1.0);
                let k = 1.0 - ra;
                (r, g, b, a) = (c[0] * ra + r * k, c[1] * ra + g * k, c[2] * ra + b * k, 255.0 * ra + a * k);
            }
            *px = pack(a, r, g, b);
        }
    }
}

fn unpack(p: u32) -> [f32; 4] {
    [(p >> 24) as f32, ((p >> 16) & 0xff) as f32, ((p >> 8) & 0xff) as f32, (p & 0xff) as f32]
}

fn pack(a: f32, r: f32, g: f32, b: f32) -> u32 {
    let a = a.round().clamp(0.0, 255.0);
    // Premultiplied: no channel above alpha.
    let c = |v: f32| v.round().clamp(0.0, a) as u32;
    (a as u32) << 24 | c(r) << 16 | c(g) << 8 | c(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field() -> (GlowField, usize, usize, Rect) {
        let pill = Rect { x: 40.0, y: 40.0, w: 200.0, h: 50.0 };
        (GlowField::new(280, 130, pill, 25.0, 1.0), 280, 130, pill)
    }

    #[test]
    fn glows_around_the_pill_and_fades_with_distance() {
        let (f, w, h, pill) = field();
        let mut out = vec![0u32; w * h];
        f.paint(&mut out, &vec![0u32; w * h], 1.0, 0.0);
        let alpha = |x: usize, y: usize| out[y * w + x] >> 24;
        let below = pill.center_x() as usize;
        let edge = (pill.y + pill.h) as usize;
        assert!(alpha(below, edge + 2) > alpha(below, edge + 12), "fades outwards");
        assert!(alpha(below, edge + 12) > alpha(below, edge + 30), "keeps fading");
        assert_eq!(alpha(0, 0), 0, "nothing in the far corner");
    }

    #[test]
    fn the_pill_stays_on_top_and_the_rim_lights_its_edge() {
        let (f, w, h, pill) = field();
        let glass = 0xEB16_171F_u32; // dark glass, ~92% opaque (premultiplied)
        let layer: Vec<u32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32 + 0.5, (i / w) as f32 + 0.5);
                if rounded_rect_distance(x, y, pill, 25.0) < 0.0 {
                    glass
                } else {
                    0
                }
            })
            .collect();
        let mut out = vec![0u32; w * h];
        f.paint(&mut out, &layer, 1.0, 0.25);
        let centre = (pill.center_y() as usize) * w + pill.center_x() as usize;
        assert_eq!(out[centre], glass, "the middle of the pill is untouched");
        let rim = (pill.y as usize + 1) * w + pill.center_x() as usize;
        let (r, b) = ((out[rim] >> 16) & 0xff, out[rim] & 0xff);
        assert!(r + b > 120, "a bright, coloured rim on the edge: {:08x}", out[rim]);
    }

    #[test]
    fn colours_travel_around_the_ring() {
        let (f, w, h, pill) = field();
        let (mut a, mut b) = (vec![0u32; w * h], vec![0u32; w * h]);
        let empty = vec![0u32; w * h];
        f.paint(&mut a, &empty, 1.0, 0.0);
        f.paint(&mut b, &empty, 1.0, 0.5);
        let spot = (pill.y as usize - 4) * w + pill.center_x() as usize;
        assert_ne!(a[spot] & 0x00ff_ffff, b[spot] & 0x00ff_ffff, "half a turn changes the colour there");
        assert_eq!(a[spot] >> 24, b[spot] >> 24, "but not how strong it is");
    }

    #[test]
    fn pixels_stay_premultiplied() {
        let (f, w, h, _) = field();
        let mut out = vec![0u32; w * h];
        f.paint(&mut out, &vec![0u32; w * h], 1.4, 0.3);
        for p in out {
            let [a, r, g, b] = unpack(p);
            assert!(r <= a && g <= a && b <= a, "{p:08x}");
        }
    }
}
