//! Two-finger glyphs: a circle, a V, a Z, a check mark drawn with two
//! fingers, recorded by the user and matched when they draw it again.
//!
//! Two fingers belong to the OS (scrolling), so only strokes that are
//! clearly not a scroll count: long enough and far from a straight line.
//! Matching forgives the way people draw: both paths are resampled to the
//! same number of points, scaled to a unit box and centred, their
//! proportions evened out, then compared point by point with the drawing
//! turned up to 40 degrees either way and in either direction ($1-style),
//! and as unordered clouds of points ($P-style). A V and an upside-down V
//! stay different glyphs.

use crate::frame::TouchFrame;
use serde::{Deserialize, Serialize};

/// Glyph ids run from 1 to this.
pub const MAX_GLYPHS: u8 = 32;
/// Points in a normalised glyph.
pub const POINTS: usize = 32;
/// The shortest stroke that can be a glyph, in millimetres of travel.
pub const MIN_LENGTH_MM: f32 = 15.0;

/// A recorded glyph: a name for people, an id for triggers (`glyph:3`) and
/// the normalised path it was drawn with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Glyph {
    pub id: u8,
    pub name: String,
    pub points: Vec<[f32; 2]>,
}

/// Why a stroke can't be a glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotAGlyph {
    /// Too small to tell apart from a nudge.
    TooShort,
    /// A straight line, which is a scroll.
    Straight,
}

impl NotAGlyph {
    pub fn message(self) -> &'static str {
        match self {
            NotAGlyph::TooShort => "That was too small. Draw the glyph a little bigger.",
            NotAGlyph::Straight => {
                "That was a straight line, which is how you scroll. Draw a glyph: a circle, a V, a Z or a check mark."
            }
        }
    }
}

fn length(path: &[(f32, f32)]) -> f32 {
    path.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum()
}

/// Can this stroke be a glyph (rather than a scroll or a nudge)?
pub fn check(path: &[(f32, f32)]) -> Result<(), NotAGlyph> {
    let len = length(path);
    if path.len() < 4 || len.is_nan() || len < MIN_LENGTH_MM {
        return Err(NotAGlyph::TooShort);
    }
    // How far the stroke strays from the line through its ends, against its
    // length: a scroll hugs the line, a glyph doesn't.
    let (a, b) = (path[0], path[path.len() - 1]);
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let chord = dx.hypot(dy);
    let stray = path
        .iter()
        .map(|p| {
            if chord < 1e-3 {
                (p.0 - a.0).hypot(p.1 - a.1)
            } else {
                ((p.0 - a.0) * dy - (p.1 - a.1) * dx).abs() / chord
            }
        })
        .fold(0.0f32, f32::max);
    if stray < len * 0.12 {
        return Err(NotAGlyph::Straight);
    }
    Ok(())
}

/// The path resampled to [`POINTS`] evenly spaced points, scaled so its
/// longer side is 1 (keeping its proportions) and centred on the origin.
/// `None` for a path with no length, or one too broken to measure.
pub fn normalize(path: &[(f32, f32)]) -> Option<Vec<[f32; 2]>> {
    let total = length(path);
    if path.len() < 2 || !total.is_finite() || total <= 0.0 {
        return None;
    }
    let step = total / (POINTS - 1) as f32;
    let mut out = vec![path[0]];
    let mut carried = 0.0;
    for w in path.windows(2) {
        let (mut a, b) = (w[0], w[1]);
        let mut seg = (b.0 - a.0).hypot(b.1 - a.1);
        while carried + seg >= step && out.len() < POINTS {
            let t = (step - carried) / seg;
            a = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            out.push(a);
            seg = (b.0 - a.0).hypot(b.1 - a.1);
            carried = 0.0;
        }
        carried += seg;
    }
    while out.len() < POINTS {
        out.push(path[path.len() - 1]);
    }
    let (min_x, max_x) = out.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
    let (min_y, max_y) = out.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.1), hi.max(p.1)));
    let size = (max_x - min_x).max(max_y - min_y).max(1e-3);
    let (cx, cy) =
        (out.iter().map(|p| p.0).sum::<f32>() / POINTS as f32, out.iter().map(|p| p.1).sum::<f32>() / POINTS as f32);
    let points: Vec<[f32; 2]> = out.iter().map(|p| [(p.0 - cx) / size, (p.1 - cy) / size]).collect();
    points.iter().flatten().all(|v| v.is_finite()).then_some(points)
}

/// How alike two normalised glyphs are point by point, as drawn: 1 for the
/// same, 0 for nothing alike. The strict measure; [`Matcher`] forgives more.
pub fn similarity(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    (1.0 - mean_distance(a, b) / 0.5).max(0.0)
}

/// Below this score a stroke matches nothing.
pub const MIN_SCORE: f32 = 0.82;
/// When two glyphs score within this of each other, the stroke is too
/// ambiguous to run either.
const MIN_LEAD: f32 = 0.03;
/// How far a drawing may be turned, either way, and still count: tilted is
/// fine, upside down (a V against a ^) is a different glyph.
const MAX_TURN: f32 = 40.0 * std::f32::consts::PI / 180.0;
/// Drawing a glyph backwards, or only roughly in order, costs a little.
const BACKWARDS_COST: f32 = 0.02;
const UNORDERED_COST: f32 = 0.03;

fn mean_distance(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    // Points are normalised (about -1..1), so a plain square root is safe and quick.
    let d = a.iter().zip(b).map(|(p, q)| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()).sum::<f32>()
        / a.len().max(1) as f32;
    if d.is_nan() {
        f32::MAX
    } else {
        d
    }
}

/// A glyph with its proportions evened out, so an oval drawing matches a
/// circle. A flat one (a line-like squiggle) keeps its proportions, since
/// stretching it would blow its wobble up into a shape.
fn squared(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let (min_x, max_x) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
    let (min_y, max_y) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
    let (w, h) = (max_x - min_x, max_y - min_y);
    let big = w.max(h).max(1e-6);
    let (sx, sy) = if w.min(h) >= 0.3 * big { (big / w, big / h) } else { (1.0, 1.0) };
    let n = points.len().max(1) as f32;
    let (cx, cy) = (points.iter().map(|p| p[0]).sum::<f32>() / n, points.iter().map(|p| p[1]).sum::<f32>() / n);
    points.iter().map(|p| [(p[0] - cx) * sx / big, (p[1] - cy) * sy / big]).collect()
}

/// A glyph sized by its spread around its centre rather than by its box,
/// which a tilt would change: so a tilted drawing turned back lines up.
fn spread_scaled(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let n = points.len().max(1) as f32;
    let (cx, cy) = (points.iter().map(|p| p[0]).sum::<f32>() / n, points.iter().map(|p| p[1]).sum::<f32>() / n);
    let rms = (points.iter().map(|p| (p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sum::<f32>() / n).sqrt();
    let k = if rms > 1e-6 { 0.35 / rms } else { 1.0 };
    points.iter().map(|p| [(p[0] - cx) * k, (p[1] - cy) * k]).collect()
}

fn turned(points: &[[f32; 2]], angle: f32, out: &mut Vec<[f32; 2]>) {
    let (sin, cos) = angle.sin_cos();
    out.clear();
    out.extend(points.iter().map(|p| [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos]));
}

/// The closest the drawing comes to the template when turned by up to
/// [`MAX_TURN`] (a golden-section search, as in the $1 recognizer).
fn turned_distance(drawn: &[[f32; 2]], template: &[[f32; 2]], scratch: &mut Vec<[f32; 2]>) -> f32 {
    let phi = 0.5 * (5f32.sqrt() - 1.0);
    let mut at = |angle: f32| {
        turned(drawn, angle, scratch);
        mean_distance(scratch, template)
    };
    let (mut a, mut b) = (-MAX_TURN, MAX_TURN);
    let (mut x1, mut x2) = (phi * a + (1.0 - phi) * b, (1.0 - phi) * a + phi * b);
    let (mut f1, mut f2) = (at(x1), at(x2));
    while (b - a).abs() > 2f32.to_radians() {
        if f1 < f2 {
            b = x2;
            x2 = x1;
            f2 = f1;
            x1 = phi * a + (1.0 - phi) * b;
            f1 = at(x1);
        } else {
            a = x1;
            x1 = x2;
            f1 = f2;
            x2 = (1.0 - phi) * a + phi * b;
            f2 = at(x2);
        }
    }
    f1.min(f2).min(at(0.0))
}

/// Every other point: enough for comparing clouds, at a quarter of the work.
fn cloud(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    points.iter().step_by(2).copied().collect()
}

/// How close two drawings are as clouds of points (see [`cloud`]), whatever
/// order they were drawn in: the $P recognizer's greedy match.
fn cloud_distance(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return f32::MAX;
    }
    let pass = |from: &[[f32; 2]], to: &[[f32; 2]], start: usize| {
        let mut used = [false; POINTS];
        let (mut sum, mut weights) = (0.0, 0.0);
        for k in 0..n {
            let p = from[(start + k) % n];
            // The nearest unused point, by squared distance: one root per point, not per pair.
            let (mut best, mut at) = (f32::MAX, 0);
            for (j, q) in to.iter().enumerate().take(n) {
                let d2 = (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2);
                if !used[j] && d2 < best {
                    (best, at) = (d2, j);
                }
            }
            used[at] = true;
            let w = 1.0 - k as f32 / n as f32;
            sum += w * best.sqrt();
            weights += w;
        }
        sum / weights
    };
    let step = ((n as f32).sqrt() as usize).max(1);
    // Both ways: the drawing near the glyph and the glyph near the drawing,
    // so a scribble can't pass by covering a simple glyph.
    let best = (0..n).step_by(step).map(|s| 0.5 * (pass(a, b, s) + pass(b, a, s))).fold(f32::MAX, f32::min);
    if best.is_nan() {
        f32::MAX
    } else {
        best
    }
}

/// Sharp corners along a normalised glyph: a V or a check mark has one, a Z
/// two, a circle none, a scribble many. Turns are measured over a few
/// points, so a shaky hand doesn't add corners.
fn corners(points: &[[f32; 2]]) -> u32 {
    const SPAN: usize = 3;
    let mut count = 0;
    let mut last = None;
    for i in SPAN..points.len().saturating_sub(SPAN) {
        let (a, b, c) = (points[i - SPAN], points[i], points[i + SPAN]);
        let (u, v) = ([b[0] - a[0], b[1] - a[1]], [c[0] - b[0], c[1] - b[1]]);
        let cos = (u[0] * v[0] + u[1] * v[1]) / (u[0].hypot(u[1]) * v[0].hypot(v[1])).max(1e-9);
        // Sharper than about 60°, and not the same corner seen again.
        if cos < 0.5 && last.is_none_or(|l| i - l > SPAN) {
            count += 1;
            last = Some(i);
        }
    }
    count
}

/// Each corner more or fewer than the glyph has costs this much.
const CORNER_COST: f32 = 0.05;

/// Recorded glyphs made ready for matching: forgiving of size, place,
/// proportions, a tilt, the direction drawn, an uneven pace and a rough
/// hand, but not of a different shape. Build one when the glyphs change.
#[derive(Clone, Debug, Default)]
pub struct Matcher {
    glyphs: Vec<Prepared>,
}

/// A glyph as drawn, with its proportions evened out, and its corners.
#[derive(Clone, Debug)]
struct Prepared {
    id: u8,
    drawn: Vec<[f32; 2]>,
    squared: Vec<[f32; 2]>,
    cloud: Vec<[f32; 2]>,
    corners: u32,
}

impl Prepared {
    fn new(id: u8, points: &[[f32; 2]]) -> Prepared {
        let squared = squared(points);
        Prepared { id, drawn: spread_scaled(points), cloud: cloud(&squared), squared, corners: corners(points) }
    }

    fn backwards(&self) -> Prepared {
        let rev = |v: &Vec<[f32; 2]>| v.iter().rev().copied().collect();
        let squared = rev(&self.squared);
        Prepared { id: self.id, drawn: rev(&self.drawn), cloud: cloud(&squared), squared, corners: self.corners }
    }
}

impl Matcher {
    pub fn new(glyphs: &[Glyph]) -> Matcher {
        let glyphs =
            glyphs.iter().filter(|g| g.points.len() == POINTS).map(|g| Prepared::new(g.id, &g.points)).collect();
        Matcher { glyphs }
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }

    /// How alike a drawing is to a glyph (0..1): turned at its true
    /// proportions (a tilt), at even proportions unturned (an oval), or as
    /// an unordered cloud of points (a rough hand), whichever is closest;
    /// less for corners it has too many or too few of.
    fn score(drawn: &Prepared, backwards: &Prepared, glyph: &Prepared, scratch: &mut Vec<[f32; 2]>) -> f32 {
        let turned = turned_distance(&drawn.drawn, &glyph.drawn, scratch)
            .min(turned_distance(&backwards.drawn, &glyph.drawn, scratch) + BACKWARDS_COST);
        let even = mean_distance(&drawn.squared, &glyph.squared)
            .min(mean_distance(&backwards.squared, &glyph.squared) + BACKWARDS_COST);
        let cloud = cloud_distance(&drawn.cloud, &glyph.cloud) + UNORDERED_COST;
        let corners = drawn.corners.abs_diff(glyph.corners) as f32 * CORNER_COST;
        let s = 1.0 - turned.min(even).min(cloud) / 0.5 - corners;
        if s.is_finite() {
            s.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Every glyph's score for this stroke, best first.
    pub fn scores(&self, path: &[(f32, f32)]) -> Vec<(u8, f32)> {
        let Some(points) = normalize(path) else { return Vec::new() };
        let drawn = Prepared::new(0, &points);
        let backwards = drawn.backwards();
        let mut scratch = Vec::with_capacity(POINTS);
        let mut out: Vec<(u8, f32)> =
            self.glyphs.iter().map(|g| (g.id, Self::score(&drawn, &backwards, g, &mut scratch))).collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        out
    }

    /// The glyph this stroke is, if one stands out clearly enough.
    pub fn best(&self, path: &[(f32, f32)]) -> Option<(u8, f32)> {
        check(path).ok()?;
        let scores = self.scores(path);
        let (id, score) = *scores.first()?;
        let lead = scores.get(1).map_or(1.0, |second| score - second.1);
        (score >= MIN_SCORE && lead >= MIN_LEAD).then_some((id, score))
    }
}

/// The recorded glyph this stroke matches best, if one matches well enough.
/// (Prepares the glyphs each time; keep a [`Matcher`] to match many strokes.)
pub fn best_match(glyphs: &[Glyph], path: &[(f32, f32)]) -> Option<(u8, f32)> {
    Matcher::new(glyphs).best(path)
}

/// Follows two-finger strokes: the path of the fingers' midpoint while two
/// fingers are down, reported when they all lift. Fingers rarely land in the
/// same instant, so one landing a moment before the other is fine; a third
/// finger at any point makes it something else. Once a finger lifts the path
/// ends, so putting it back can't add a jump.
#[derive(Debug, Default)]
pub struct StrokeTracker {
    path: Vec<(f32, f32)>,
    active: bool,
    /// A third finger touched: not a two-finger stroke.
    crowded: bool,
    /// Fewer than two fingers since the path began: it's over.
    ended: bool,
}

/// Paths longer than this are not glyphs anyone draws; stop collecting.
const MAX_PATH: usize = 4_000;

impl StrokeTracker {
    /// Feed every frame; returns a finished two-finger stroke when the fingers lift.
    pub fn update(&mut self, frame: &TouchFrame) -> Option<Vec<(f32, f32)>> {
        let n = frame.contacts.len();
        if n == 0 {
            let done = self.active && !self.crowded && self.path.len() >= 2;
            self.active = false;
            return done.then(|| std::mem::take(&mut self.path));
        }
        if !self.active {
            self.active = true;
            self.crowded = false;
            self.ended = false;
            self.path.clear();
        }
        if n > 2 {
            self.crowded = true;
        }
        // One finger lifting first is the end of the glyph, not part of it.
        if n < 2 && !self.path.is_empty() {
            self.ended = true;
        }
        if n == 2 && !self.crowded && !self.ended && self.path.len() < MAX_PATH {
            let c = &frame.contacts;
            let mid = ((c[0].x + c[1].x) / 2.0, (c[0].y + c[1].y) / 2.0);
            let moved = self.path.last().is_none_or(|l| (mid.0 - l.0).hypot(mid.1 - l.1) >= 0.25);
            // A broken position (NaN, infinite) is skipped, not drawn.
            if moved && mid.0.is_finite() && mid.1.is_finite() {
                self.path.push(mid);
            }
        }
        None
    }

    /// Forget the stroke in progress.
    pub fn reset(&mut self) {
        self.active = false;
        self.path.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Contact;
    use std::f32::consts::TAU;

    fn circle(r: f32, turns: f32) -> Vec<(f32, f32)> {
        (0..=60).map(|i| i as f32 / 60.0 * TAU * turns).map(|a| (40.0 + r * a.cos(), 30.0 + r * a.sin())).collect()
    }

    fn polyline(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
        let mut out = Vec::new();
        for w in points.windows(2) {
            for i in 0..10 {
                let t = i as f32 / 10.0;
                out.push((w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t));
            }
        }
        out.push(*points.last().unwrap());
        out
    }

    fn vee(size: f32) -> Vec<(f32, f32)> {
        polyline(&[(10.0, 10.0), (10.0 + size / 2.0, 10.0 + size), (10.0 + size, 10.0)])
    }

    fn zed(size: f32) -> Vec<(f32, f32)> {
        polyline(&[(10.0, 10.0), (10.0 + size, 10.0), (10.0, 10.0 + size), (10.0 + size, 10.0 + size)])
    }

    fn glyph(id: u8, path: &[(f32, f32)]) -> Glyph {
        Glyph { id, name: format!("glyph {id}"), points: normalize(path).unwrap() }
    }

    #[test]
    fn scrolls_and_nudges_are_not_glyphs() {
        assert_eq!(check(&polyline(&[(10.0, 10.0), (10.0, 50.0)])), Err(NotAGlyph::Straight));
        // A scroll that drifts a little sideways is still a scroll.
        assert_eq!(check(&polyline(&[(10.0, 10.0), (12.0, 30.0), (13.0, 50.0)])), Err(NotAGlyph::Straight));
        assert_eq!(check(&circle(1.5, 1.0)), Err(NotAGlyph::TooShort));
        assert_eq!(check(&circle(10.0, 1.0)), Ok(()));
        assert_eq!(check(&vee(20.0)), Ok(()));
    }

    #[test]
    fn a_glyph_matches_itself_at_any_size_and_place() {
        let glyphs = [glyph(1, &circle(10.0, 1.0)), glyph(2, &vee(20.0)), glyph(3, &zed(20.0))];
        let moved: Vec<_> = circle(16.0, 1.0).iter().map(|p| (p.0 + 7.0, p.1 - 3.0)).collect();
        assert_eq!(best_match(&glyphs, &moved).map(|m| m.0), Some(1));
        assert_eq!(best_match(&glyphs, &vee(35.0)).map(|m| m.0), Some(2));
        assert_eq!(best_match(&glyphs, &zed(28.0)).map(|m| m.0), Some(3));
    }

    #[test]
    fn a_sloppy_drawing_still_matches() {
        let glyphs = [glyph(1, &circle(10.0, 1.0)), glyph(2, &vee(20.0)), glyph(3, &zed(20.0))];
        let wobbly: Vec<_> =
            vee(24.0).iter().enumerate().map(|(i, p)| (p.0 + (i as f32 * 1.3).sin() * 0.8, p.1)).collect();
        assert_eq!(best_match(&glyphs, &wobbly).map(|m| m.0), Some(2));
    }

    #[test]
    fn upside_down_is_another_glyph_but_backwards_is_the_same() {
        let glyphs = [glyph(1, &vee(20.0))];
        let upside_down = polyline(&[(10.0, 30.0), (20.0, 10.0), (30.0, 30.0)]);
        assert_eq!(best_match(&glyphs, &upside_down), None);
        let mut backwards = zed(20.0);
        backwards.reverse();
        assert_eq!(best_match(&[glyph(3, &zed(20.0))], &backwards).map(|m| m.0), Some(3));
    }

    fn check_mark(size: f32) -> Vec<(f32, f32)> {
        polyline(&[(10.0, 10.0 + size * 0.5), (10.0 + size * 0.35, 10.0 + size), (10.0 + size, 10.0)])
    }

    fn turn(path: &[(f32, f32)], degrees: f32) -> Vec<(f32, f32)> {
        let (sin, cos) = degrees.to_radians().sin_cos();
        path.iter()
            .map(|&(x, y)| (40.0 + (x - 40.0) * cos - (y - 30.0) * sin, 30.0 + (x - 40.0) * sin + (y - 30.0) * cos))
            .collect()
    }

    fn shaky(path: &[(f32, f32)], mm: f32, seed: u64) -> Vec<(f32, f32)> {
        let mut rng = crate::Rng::new(seed);
        let mut jitter = || (rng.below(2_001) as f32 / 1_000.0 - 1.0) * mm;
        path.iter().map(|&(x, y)| (x + jitter(), y + jitter())).collect()
    }

    /// A circle, a V, a Z and a check mark, as recorded.
    fn everyday() -> [Glyph; 4] {
        [glyph(1, &circle(10.0, 1.0)), glyph(2, &vee(20.0)), glyph(3, &zed(20.0)), glyph(4, &check_mark(24.0))]
    }

    #[test]
    fn sloppy_drawings_still_match_their_glyph() {
        let glyphs = everyday();
        // What it is, the drawing, and the glyph it should match.
        type Case = (&'static str, Vec<(f32, f32)>, u8);
        let cases: Vec<Case> = vec![
            ("tilted 25°", turn(&vee(22.0), 25.0), 2),
            ("tilted -30°", turn(&zed(26.0), -30.0), 3),
            ("an oval", circle(10.0, 1.0).iter().map(|&(x, y)| (x * 1.5, y * 0.8)).collect(), 1),
            ("big and far", circle(30.0, 1.0).iter().map(|&(x, y)| (x + 50.0, y + 20.0)).collect(), 1),
            ("not quite closed", circle(12.0, 0.85), 1),
            ("one turn and a bit", circle(12.0, 1.15), 1),
            ("circled the other way", circle(12.0, 1.0).into_iter().rev().collect(), 1),
            ("shaky", shaky(&vee(24.0), 1.2, 7), 2),
            ("shaky and tilted", turn(&shaky(&check_mark(30.0), 1.0, 9), 15.0), 4),
            ("a wide V", vee(30.0).iter().map(|&(x, y)| (x * 1.6, y * 0.8)).collect(), 2),
        ];
        for (what, path, want) in cases {
            let scores = Matcher::new(&glyphs).scores(&path);
            eprintln!("{what}: {:?}", &scores[..2]);
            assert_eq!(best_match(&glyphs, &path).map(|m| m.0), Some(want), "{what}: {scores:?}");
        }
    }

    #[test]
    fn a_slow_or_fast_hand_draws_the_same_glyph() {
        let glyphs = everyday();
        // The same Z with its points bunched where the hand slowed down.
        let uneven: Vec<(f32, f32)> = zed(24.0)
            .chunks(10)
            .enumerate()
            .flat_map(|(i, c)| {
                let keep = if i % 2 == 0 { 1 } else { 5 };
                c.iter().step_by(keep).copied().collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(best_match(&glyphs, &uneven).map(|m| m.0), Some(3));
    }

    /// How many of 1000 random scribbles (4 to 12 strokes in random
    /// directions) match one of the everyday glyphs.
    fn scribbles_matched(seed: u64) -> Vec<(u8, f32)> {
        let glyphs = everyday();
        let mut rng = crate::Rng::new(seed);
        let (mut tries, mut matched) = (0, Vec::new());
        while tries < 1_000 {
            // A random walk of 4 to 12 strokes in random directions.
            let mut p = (50.0f32, 30.0f32);
            let mut path = vec![p];
            for _ in 0..4 + rng.below(9) {
                let angle = rng.below(360) as f32 * std::f32::consts::PI / 180.0;
                let len = 5.0 + rng.below(25) as f32;
                let to = (p.0 + angle.cos() * len, p.1 + angle.sin() * len);
                for i in 1..=8 {
                    path.push((p.0 + (to.0 - p.0) * i as f32 / 8.0, p.1 + (to.1 - p.1) * i as f32 / 8.0));
                }
                p = to;
            }
            if check(&path).is_err() {
                continue;
            }
            tries += 1;
            if let Some(m) = best_match(&glyphs, &path) {
                matched.push(m);
            }
        }
        matched
    }

    #[test]
    fn scribbles_match_nothing() {
        // A scribble can look like a glyph by chance; it mustn't happen often.
        for seed in [0x5EED_CAFE, 1, 2, 3] {
            let matched = scribbles_matched(seed);
            eprintln!("seed {seed}: {} of 1000 scribbles matched", matched.len());
            assert!(matched.len() <= 10, "{} of 1000 scribbles matched: {matched:?}", matched.len());
        }
    }

    #[test]
    fn two_glyphs_too_alike_fire_neither() {
        let glyphs = [glyph(1, &circle(10.0, 1.0)), glyph(2, &circle(14.0, 1.0))];
        assert_eq!(best_match(&glyphs, &circle(12.0, 1.0)), None);
    }

    #[test]
    fn nothing_recorded_matches_nothing() {
        assert_eq!(best_match(&[], &circle(10.0, 1.0)), None);
        assert_eq!(best_match(&[glyph(1, &circle(10.0, 1.0))], &zed(20.0)), None);
    }

    fn frame(points: &[(f32, f32)]) -> TouchFrame {
        TouchFrame {
            time_ms: 0,
            contacts: points.iter().enumerate().map(|(i, &(x, y))| Contact { id: i as u32, x, y }).collect(),
            width_mm: 100.0,
            height_mm: 60.0,
        }
    }

    #[test]
    fn the_tracker_follows_two_fingers_until_they_lift() {
        let mut t = StrokeTracker::default();
        for p in vee(20.0) {
            assert_eq!(t.update(&frame(&[(p.0 - 5.0, p.1), (p.0 + 5.0, p.1)])), None);
        }
        assert_eq!(t.update(&frame(&[(1.0, 1.0)])), None, "one finger lifting first ends nothing yet");
        let path = t.update(&frame(&[])).expect("a stroke");
        assert_eq!(best_match(&[glyph(2, &vee(20.0))], &path).map(|m| m.0), Some(2));
    }

    #[test]
    fn fingers_landing_and_lifting_one_at_a_time_still_draw() {
        // As on a real touchpad: one finger lands a frame before the other,
        // and they lift one after the other.
        let mut t = StrokeTracker::default();
        let path = vee(20.0);
        t.update(&frame(&[(path[0].0 - 5.0, path[0].1)]));
        for p in &path {
            t.update(&frame(&[(p.0 - 5.0, p.1), (p.0 + 5.0, p.1)]));
        }
        let last = path[path.len() - 1];
        t.update(&frame(&[(last.0 + 5.0, last.1)]));
        // Putting the finger back doesn't add to a glyph that's over.
        t.update(&frame(&[(90.0, 90.0), (95.0, 90.0)]));
        let drawn = t.update(&frame(&[])).expect("a stroke");
        assert_eq!(best_match(&[glyph(2, &vee(20.0))], &drawn).map(|m| m.0), Some(2));
        assert!(drawn.iter().all(|p| p.0 < 60.0), "nothing after the lift: {drawn:?}");
    }

    #[test]
    fn three_fingers_or_one_finger_strokes_are_not_tracked() {
        let mut t = StrokeTracker::default();
        t.update(&frame(&[(1.0, 1.0), (5.0, 1.0)]));
        t.update(&frame(&[(1.0, 1.0), (5.0, 1.0), (9.0, 1.0)]));
        t.update(&frame(&[(1.0, 9.0), (5.0, 9.0)]));
        assert_eq!(t.update(&frame(&[])), None);
        t.update(&frame(&[(1.0, 1.0)]));
        t.update(&frame(&[(9.0, 9.0)]));
        assert_eq!(t.update(&frame(&[])), None);
    }

    fn finite(points: &[[f32; 2]]) -> bool {
        points.iter().flatten().all(|v| v.is_finite())
    }

    #[test]
    fn degenerate_strokes_never_make_nan_glyphs() {
        let (nan, inf, big) = (f32::NAN, f32::INFINITY, 3e38);
        let mut paths: Vec<Vec<(f32, f32)>> = vec![
            vec![],
            vec![(5.0, 5.0)],
            vec![(5.0, 5.0); 2],
            vec![(5.0, 5.0); 500],
            vec![(nan, nan); 10],
            vec![(0.0, 0.0), (inf, 0.0), (0.0, 30.0), (30.0, 30.0)],
            vec![(-big, -big), (big, big), (-big, big), (big, -big), (0.0, 0.0)],
            vec![(1e30, 1e30), (1e30 + 1e25, 1e30), (1e30, 1e30 + 1e25), (1e30, 1e30)],
        ];
        let mut with_nan = circle(10.0, 1.0);
        with_nan[0] = (nan, 3.0);
        paths.push(with_nan);
        let mut with_inf = vee(20.0);
        with_inf[7] = (inf, -inf);
        paths.push(with_inf);
        let glyphs = [glyph(1, &circle(10.0, 1.0)), glyph(2, &vee(20.0))];
        for path in &paths {
            let _ = check(path);
            if let Some(points) = normalize(path) {
                assert!(finite(&points) && points.len() == POINTS, "{path:?} gave {points:?}");
            }
            if let Some((_, score)) = best_match(&glyphs, path) {
                assert!(score.is_finite(), "{path:?}");
            }
        }
        assert_eq!(normalize(&[(5.0, 5.0); 500]), None, "a stroke that never moved has no glyph");
        assert_eq!(check(&[(nan, nan); 10]), Err(NotAGlyph::TooShort));
    }

    #[test]
    fn similarity_is_never_nan() {
        let good = glyph(1, &circle(10.0, 1.0)).points;
        let huge = vec![[3e38f32, -3e38]; POINTS];
        for other in [&good, &huge, &vec![[0.0, 0.0]; POINTS], &vec![[0.0, 0.0]; 3]] {
            let s = similarity(&good, other);
            assert!((0.0..=1.0).contains(&s), "{s}");
        }
        assert_eq!(similarity(&huge, &huge), 1.0);
    }

    #[test]
    fn the_tracker_skips_broken_positions() {
        let mut t = StrokeTracker::default();
        t.update(&frame(&[(f32::NAN, 1.0), (5.0, 1.0)]));
        for p in vee(20.0) {
            t.update(&frame(&[(p.0 - 5.0, p.1), (p.0 + 5.0, p.1)]));
            t.update(&frame(&[(f32::INFINITY, p.1), (p.0 + 5.0, p.1)]));
        }
        let path = t.update(&frame(&[])).expect("a stroke");
        assert!(path.iter().all(|p| p.0.is_finite() && p.1.is_finite()), "{path:?}");
        assert_eq!(best_match(&[glyph(2, &vee(20.0))], &path).map(|m| m.0), Some(2));
    }

    #[test]
    fn the_tracker_caps_a_stroke_that_never_ends() {
        let mut t = StrokeTracker::default();
        for i in 0..100_000 {
            let x = (i % 400) as f32 * 0.3;
            t.update(&frame(&[(x, (i / 400) as f32), (x + 10.0, 0.0)]));
        }
        let path = t.update(&frame(&[])).expect("a stroke");
        assert_eq!(path.len(), MAX_PATH);
        let _ = best_match(&[glyph(1, &circle(10.0, 1.0))], &path);
    }

    /// Random strokes with random finger counts, broken positions and lifts.
    #[test]
    fn fuzzed_strokes_never_panic_or_make_nan_glyphs() {
        let mut rng = crate::Rng::new(0x2545_F491_4F6C_DD1D);
        let glyphs: Vec<Glyph> =
            (1..=MAX_GLYPHS).map(|id| glyph(id, &circle(5.0 + id as f32, 0.5 + id as f32 / 20.0))).collect();
        let mut t = StrokeTracker::default();
        let mut strokes = 0;
        for _ in 0..50_000 {
            let n = [0, 0, 1, 2, 2, 2, 2, 3][rng.below(8) as usize];
            let points: Vec<(f32, f32)> = (0..n).map(|_| (rng.mm(), rng.mm())).collect();
            if let Some(path) = t.update(&frame(&points)) {
                strokes += 1;
                if let Some(points) = normalize(&path) {
                    assert!(finite(&points));
                }
                if let Some((id, score)) = best_match(&glyphs, &path) {
                    assert!((1..=MAX_GLYPHS).contains(&id) && score.is_finite());
                }
            }
            if rng.below(1_000) == 0 {
                t.reset();
            }
        }
        assert!(strokes > 100, "{strokes} strokes");
    }

    /// 32 different glyphs: polygons, stars and spirals of all sorts.
    fn many_glyphs() -> Vec<Glyph> {
        (1..=MAX_GLYPHS)
            .map(|id| {
                let k = id as f32;
                let path: Vec<(f32, f32)> = (0..=80)
                    .map(|i| {
                        let t = i as f32 / 80.0;
                        let a = t * std::f32::consts::TAU * (1.0 + k / 9.0);
                        let r = 10.0 + 6.0 * (a * (2.0 + (id % 5) as f32)).sin() + k * t;
                        (40.0 + r * a.cos(), 30.0 + r * a.sin())
                    })
                    .collect();
                glyph(id, &path)
            })
            .collect()
    }

    #[test]
    fn matching_against_every_glyph_is_quick() {
        let mut glyphs = many_glyphs();
        glyphs[0] = glyph(1, &circle(10.0, 1.0));
        // A long stroke: about two seconds of drawing at 125 Hz.
        let path: Vec<(f32, f32)> = circle(14.0, 1.0)
            .windows(2)
            .flat_map(|w| (0..4).map(move |i| (w[0].0 + (w[1].0 - w[0].0) * i as f32 / 4.0, w[0].1)))
            .collect();
        let matcher = Matcher::new(&glyphs);
        let runs = 500;
        let start = std::time::Instant::now();
        let mut matched = 0;
        for _ in 0..runs {
            matched += (matcher.best(std::hint::black_box(&path)).map(|m| m.0) == Some(1)) as u32;
        }
        let each = start.elapsed() / runs;
        // Well under a millisecond in release (about 0.1 ms); the budget
        // leaves room for slow CI runners. A stroke ends once per lift.
        let budget = std::time::Duration::from_micros(if cfg!(debug_assertions) { 20_000 } else { 1_000 });
        assert!(each < budget, "{each:?} per stroke");
        assert_eq!(matched, runs, "the circle matches the circle");
        eprintln!("matching a stroke against {} glyphs: {each:?}", glyphs.len());
    }

    #[test]
    fn matching_time_grows_with_the_glyphs_not_the_stroke() {
        let all = many_glyphs();
        let short = circle(12.0, 1.0);
        // Three seconds of drawing at 125 Hz: still resampled to 32 points.
        let long: Vec<(f32, f32)> = (0..375)
            .map(|i| {
                let a = i as f32 / 375.0 * std::f32::consts::TAU;
                (40.0 + 12.0 * a.cos(), 30.0 + 12.0 * a.sin())
            })
            .collect();
        let time = |m: &Matcher, path: &[(f32, f32)]| {
            let start = std::time::Instant::now();
            for _ in 0..200 {
                std::hint::black_box(m.best(std::hint::black_box(path)));
            }
            start.elapsed() / 200
        };
        for n in [1, 8, 32] {
            let m = Matcher::new(&all[..n]);
            eprintln!("{n:>2} glyphs: short stroke {:?}, long stroke {:?}", time(&m, &short), time(&m, &long));
        }
        let m = Matcher::new(&all);
        let (s, l) = (time(&m, &short), time(&m, &long));
        // Resampling makes stroke length nearly free: well within 3x.
        assert!(l < s * 3 + std::time::Duration::from_micros(50), "short {s:?}, long {l:?}");
    }

    #[test]
    fn preparing_every_glyph_is_quick() {
        let glyphs = many_glyphs();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(Matcher::new(std::hint::black_box(&glyphs)));
        }
        let each = start.elapsed() / 100;
        eprintln!("preparing {} glyphs: {each:?}", glyphs.len());
        // Done once when the config changes, not per stroke.
        let budget = std::time::Duration::from_micros(if cfg!(debug_assertions) { 20_000 } else { 1_000 });
        assert!(each < budget, "{each:?}");
    }
}
