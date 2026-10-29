//! Pixels and motion for the swipe glow and the touch display, as plain
//! functions so they are unit tested on every platform.
//!
//! Everything slow is done once: the glow for an edge is drawn when a swipe
//! towards it starts, the touch pad's background and fingertip sprite when it
//! first shows. A frame then only changes a window's opacity and position
//! (the glow) or stamps a few sprites onto a copy of the background (touches).

use crate::glow::ring_colour;
use crate::layout::Rect;
use crate::shadow::{coverage, rounded_rect_distance};

/// The screen edge a swipe is heading for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

fn premultiplied(c: [f32; 3], a: f32) -> u32 {
    let a = a.clamp(0.0, 1.0);
    let ch = |v: f32| (v * a).round().clamp(0.0, 255.0) as u32;
    ((a * 255.0).round() as u32) << 24 | ch(c[0]) << 16 | ch(c[1]) << 8 | ch(c[2])
}

/// The glow for the top edge: `len` pixels along it, `depth` into the screen.
/// Brightest at the edge's middle, fading towards both ends and inwards,
/// with the brand colours running along it.
pub fn edge_glow(len: usize, depth: usize) -> Vec<u32> {
    let mut out = vec![0u32; len * depth];
    let d = depth.max(1) as f32;
    let falloff: Vec<f32> = (0..depth)
        .map(|y| {
            let y = y as f32 + 0.5;
            (-(y / (d * 0.45)).powi(2)).exp() * 0.75 + (-(y / (d * 0.08))).exp() * 0.45
        })
        .collect();
    for x in 0..len {
        let u = (x as f32 + 0.5) / len as f32;
        let along = (std::f32::consts::PI * u).sin().powf(1.4);
        let colour = ring_colour(0.05 + u * 0.55);
        for (y, f) in falloff.iter().enumerate() {
            out[y * len + x] = premultiplied(colour, (f * along).min(1.0));
        }
    }
    out
}

/// Turns a top-edge glow (`len` × `depth`) to face `edge`. Returns the
/// pixels and their width and height.
pub fn orient(top: &[u32], len: usize, depth: usize, edge: Edge) -> (Vec<u32>, usize, usize) {
    match edge {
        Edge::Top => (top.to_vec(), len, depth),
        Edge::Bottom => {
            let mut out = Vec::with_capacity(top.len());
            for y in (0..depth).rev() {
                out.extend_from_slice(&top[y * len..(y + 1) * len]);
            }
            (out, len, depth)
        }
        Edge::Left | Edge::Right => {
            // Width is the depth, height the length.
            let mut out = vec![0u32; top.len()];
            for l in 0..len {
                for k in 0..depth {
                    let x = if edge == Edge::Left { k } else { depth - 1 - k };
                    out[l * depth + x] = top[k * len + l];
                }
            }
            (out, depth, len)
        }
    }
}

/// How bright the swipe glow is over time. The level follows the swipe's
/// progress with a little smoothing, so uneven touch reports never make it
/// flicker; a swipe that fires flashes it, one let go fades it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeMotion {
    level: f32,
    target: f32,
    /// Seconds since the swipe fired, while its flash plays.
    flash: Option<f32>,
}

const FOLLOW: f32 = 0.045; // seconds: how quickly the level follows the fingers
const LET_GO: f32 = 0.09; // seconds: fading after the fingers lift
const FLASH_RISE: f32 = 0.07;
const FLASH_FALL: f32 = 0.42;
/// The brightest the glow gets while a swipe is only on its way.
pub const PROGRESS_MAX: f32 = 0.55;

impl EdgeMotion {
    /// The swipe has come `amount` (0..1) of the way.
    pub fn progress(&mut self, amount: f32) {
        self.flash = None;
        self.target = amount.clamp(0.0, 1.0).powf(1.3) * PROGRESS_MAX;
    }

    /// It fired (flash), or the fingers let go (fade).
    pub fn end(&mut self, fired: bool) {
        self.target = 0.0;
        if fired {
            self.flash = Some(0.0);
        }
    }

    /// Advance by `dt` seconds.
    pub fn step(&mut self, dt: f32) {
        let tau = if self.target == 0.0 { LET_GO } else { FOLLOW };
        self.level += (self.target - self.level) * (1.0 - (-dt.max(0.0) / tau).exp());
        if self.level < 0.002 && self.target == 0.0 {
            self.level = 0.0;
        }
        if let Some(t) = &mut self.flash {
            *t += dt.max(0.0);
            if *t > FLASH_RISE + FLASH_FALL * 4.0 {
                self.flash = None;
            }
        }
    }

    /// Opacity now, 0..1.
    pub fn intensity(&self) -> f32 {
        let flash = self.flash.map_or(0.0, |t| {
            if t < FLASH_RISE {
                let k = t / FLASH_RISE;
                k * (2.0 - k)
            } else {
                (-(t - FLASH_RISE) / FLASH_FALL).exp()
            }
        });
        self.level.max(flash).clamp(0.0, 1.0)
    }

    /// How far (0..1 of its depth) the glow sits beyond the screen edge: it
    /// slides in as the swipe builds.
    pub fn tucked(&self) -> f32 {
        if self.flash.is_some() {
            return 0.0;
        }
        (1.0 - self.level / PROGRESS_MAX).clamp(0.0, 1.0) * 0.35
    }

    /// Still something to draw (or to fade out)?
    pub fn active(&self) -> bool {
        self.level > 0.0 || self.target > 0.0 || self.flash.is_some()
    }
}

/// The small touchpad in the corner: glass with a hairline border.
pub fn pad_background(w: usize, h: usize, scale: f32) -> Vec<u32> {
    let rect = Rect { x: 0.5, y: 0.5, w: w as f32 - 1.0, h: h as f32 - 1.0 };
    let radius = 16.0 * scale;
    let border = scale.max(1.0);
    let mut out = vec![0u32; w * h];
    for y in 0..h {
        for x in 0..w {
            let d = rounded_rect_distance(x as f32 + 0.5, y as f32 + 0.5, rect, radius);
            let fill = coverage(d);
            if fill <= 0.0 {
                continue;
            }
            let edge = coverage(-d - border); // 0 on the border, 1 inside it
            let glass = [20.0, 19.0, 30.0];
            let line = [170.0, 160.0, 220.0];
            let k = 1.0 - edge;
            let c = [
                glass[0] + (line[0] - glass[0]) * k * 0.6,
                glass[1] + (line[1] - glass[1]) * k * 0.6,
                glass[2] + (line[2] - glass[2]) * k * 0.6,
            ];
            out[y * w + x] = premultiplied(c, fill * (0.86 + 0.1 * k));
        }
    }
    out
}

/// A glowing fingertip, `size` pixels square.
pub fn fingertip(size: usize) -> Vec<u32> {
    let r = size as f32 / 2.0;
    let core = r * 0.42;
    let mut out = vec![0u32; size * size];
    for y in 0..size {
        for x in 0..size {
            let d = ((x as f32 + 0.5 - r).powi(2) + (y as f32 + 0.5 - r).powi(2)).sqrt();
            let dot = coverage(d - core);
            let halo = (-((d - core).max(0.0) / (r - core).max(1.0) * 2.2).powi(2)).exp() * 0.55;
            // Lavender in the middle, purple at the rim.
            let k = (d / core).min(1.0);
            let c = [233.0 - 66.0 * k, 227.0 - 88.0 * k, 255.0 - 5.0 * k];
            let glow = [168.0, 85.0, 247.0];
            let a = dot + halo * (1.0 - dot);
            let mix = if a > 0.0 { dot / a } else { 0.0 };
            let col =
                [glow[0] + (c[0] - glow[0]) * mix, glow[1] + (c[1] - glow[1]) * mix, glow[2] + (c[2] - glow[2]) * mix];
            out[y * size + x] = premultiplied(col, a);
        }
    }
    out
}

/// Copies the background (`w` × `h`) into `out` and stamps a fingertip
/// (square; scaled by `alpha`) centred on each point.
pub fn compose_touches(
    out: &mut [u32],
    bg: &[u32],
    (w, h): (usize, usize),
    tip: &[u32],
    points: &[(f32, f32)],
    alpha: f32,
) {
    let tip_size = (tip.len() as f64).sqrt() as usize;
    out.copy_from_slice(bg);
    let a = (alpha.clamp(0.0, 1.0) * 256.0) as u32;
    // A broken position (NaN, infinite) has no fingertip; a huge one is off the pad.
    for &(px, py) in points.iter().filter(|p| p.0.is_finite() && p.1.is_finite()) {
        let x0 = (px.round() as i64).saturating_sub(tip_size as i64 / 2);
        let y0 = (py.round() as i64).saturating_sub(tip_size as i64 / 2);
        for ty in 0..tip_size {
            let y = y0.saturating_add(ty as i64);
            if y < 0 || y >= h as i64 {
                continue;
            }
            for tx in 0..tip_size {
                let x = x0.saturating_add(tx as i64);
                if x < 0 || x >= w as i64 {
                    continue;
                }
                let s = tip[ty * tip_size + tx];
                if s == 0 {
                    continue;
                }
                let scale = |v: u32| (v * a) >> 8;
                let (sa, sr, sg, sb) =
                    (scale(s >> 24), scale((s >> 16) & 0xff), scale((s >> 8) & 0xff), scale(s & 0xff));
                let i = y as usize * w + x as usize;
                let d = out[i];
                let k = 255 - sa;
                let over = |sv: u32, dv: u32| (sv + dv * k / 255).min(255);
                out[i] = over(sa, d >> 24) << 24
                    | over(sr, (d >> 16) & 0xff) << 16
                    | over(sg, (d >> 8) & 0xff) << 8
                    | over(sb, d & 0xff);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(p: u32) -> u32 {
        p >> 24
    }

    #[test]
    fn the_glow_is_brightest_at_the_edge_middle() {
        let (len, depth) = (200, 60);
        let g = edge_glow(len, depth);
        let at = |x: usize, y: usize| alpha(g[y * len + x]);
        assert!(at(100, 0) > 200, "bright at the edge: {}", at(100, 0));
        assert!(at(100, 0) > at(100, 30) && at(100, 30) > at(100, 59), "fades inwards");
        assert!(at(100, 0) > at(10, 0), "fades towards the ends");
        for p in &g {
            let a = p >> 24;
            assert!((p >> 16 & 0xff) <= a && (p >> 8 & 0xff) <= a && (p & 0xff) <= a, "premultiplied");
        }
    }

    #[test]
    fn orienting_puts_the_bright_side_on_the_right_edge() {
        let (len, depth) = (40, 10);
        let top = edge_glow(len, depth);
        let (b, w, h) = orient(&top, len, depth, Edge::Bottom);
        assert_eq!((w, h), (40, 10));
        assert!(alpha(b[9 * w + 20]) > alpha(b[20]), "bottom: bright on the last row");
        let (l, w, h) = orient(&top, len, depth, Edge::Left);
        assert_eq!((w, h), (10, 40));
        assert!(alpha(l[20 * w]) > alpha(l[20 * w + 9]), "left: bright in the first column");
        let (r, w, _) = orient(&top, len, depth, Edge::Right);
        assert!(alpha(r[20 * w + 9]) > alpha(r[20 * w]), "right: bright in the last column");
    }

    #[test]
    fn the_glow_follows_the_swipe_then_flashes_or_fades() {
        let mut m = EdgeMotion::default();
        assert!(!m.active());
        m.progress(0.5);
        for _ in 0..30 {
            m.step(1.0 / 120.0);
        }
        let half = m.intensity();
        assert!(half > 0.1 && half < PROGRESS_MAX, "{half}");
        m.progress(1.0);
        for _ in 0..30 {
            m.step(1.0 / 120.0);
        }
        assert!(m.intensity() > half, "brighter as the fingers travel");
        m.end(true);
        m.step(0.06);
        assert!(m.intensity() > 0.85, "flash: {}", m.intensity());
        for _ in 0..240 {
            m.step(1.0 / 120.0);
        }
        assert!(!m.active(), "gone after the flash");

        let mut let_go = EdgeMotion::default();
        let_go.progress(0.6);
        let_go.step(0.2);
        let_go.end(false);
        let_go.step(0.5);
        assert!(!let_go.active() && let_go.intensity() == 0.0);
    }

    #[test]
    fn uneven_touch_reports_dont_jolt_the_glow() {
        // Reports arriving at 125 Hz with a late one: the level never jumps more than a little per display frame.
        let mut m = EdgeMotion::default();
        let mut last = 0.0;
        for i in 0..60 {
            m.progress((i as f32 / 60.0).min(1.0) + if i == 30 { 0.2 } else { 0.0 });
            m.step(1.0 / 60.0);
            let now = m.intensity();
            assert!((now - last).abs() < 0.12, "frame {i}: {last} -> {now}");
            last = now;
        }
    }

    #[test]
    fn touches_are_stamped_on_the_pad() {
        let (w, h) = (120, 80);
        let bg = pad_background(w, h, 1.0);
        assert_eq!(alpha(bg[0]), 0, "rounded corner");
        assert!(alpha(bg[40 * w + 60]) > 200, "glass in the middle");
        let tip = fingertip(24);
        let mut out = vec![0u32; w * h];
        compose_touches(&mut out, &bg, (w, h), &tip, &[(30.0, 40.0), (118.0, 2.0)], 1.0);
        let lit = out[40 * w + 30];
        assert!((lit >> 16 & 0xff) > (bg[40 * w + 30] >> 16 & 0xff) + 100, "bright fingertip: {lit:08x}");
        assert_eq!(out[10 * w + 90], bg[10 * w + 90], "untouched elsewhere");
        let mut faded = vec![0u32; w * h];
        compose_touches(&mut faded, &bg, (w, h), &tip, &[(30.0, 40.0)], 0.3);
        assert!((faded[40 * w + 30] >> 16 & 0xff) < (lit >> 16 & 0xff), "fading out dims it");
    }

    #[test]
    fn absurd_touch_positions_draw_nothing_and_never_panic() {
        let (w, h) = (120, 80);
        let bg = pad_background(w, h, 1.0);
        let tip = fingertip(24);
        let mut out = vec![0u32; w * h];
        let (nan, inf, big) = (f32::NAN, f32::INFINITY, 1e30);
        let absurd = [(big, 40.0), (40.0, big), (-big, 40.0), (big, big), (inf, 40.0), (40.0, -inf), (nan, 40.0)];
        for point in absurd {
            compose_touches(&mut out, &bg, (w, h), &tip, &[point], 1.0);
            assert!(out == bg, "{point:?} drew on the pad");
        }
    }
}
