//! Two-finger shapes: a circle, a V, a Z, a check mark drawn with two
//! fingers, recorded by the user and matched when they draw it again.
//!
//! Two fingers belong to the OS (scrolling), so only strokes that are
//! clearly not a scroll count: long enough and far from a straight line.
//! Matching is a $1-style template match without rotation, because a V and
//! an upside-down V are different shapes: both paths are resampled to the
//! same number of points, scaled to a unit box and centred, then compared
//! point by point.

use crate::frame::TouchFrame;
use serde::{Deserialize, Serialize};

/// Shape ids run from 1 to this.
pub const MAX_SHAPES: u8 = 32;
/// Points in a normalised shape.
pub const POINTS: usize = 32;
/// The shortest stroke that can be a shape, in millimetres of travel.
pub const MIN_LENGTH_MM: f32 = 15.0;
/// Below this score a stroke matches nothing.
pub const MIN_SCORE: f32 = 0.8;

/// A recorded shape: a name for people, an id for triggers (`shape:3`) and
/// the normalised path it was drawn with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: u8,
    pub name: String,
    pub points: Vec<[f32; 2]>,
}

/// Why a stroke can't be a shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotAShape {
    /// Too small to tell apart from a nudge.
    TooShort,
    /// A straight line, which is a scroll.
    Straight,
}

impl NotAShape {
    pub fn message(self) -> &'static str {
        match self {
            NotAShape::TooShort => "That was too small. Draw the shape a little bigger.",
            NotAShape::Straight => {
                "That was a straight line, which is how you scroll. Draw a shape: a circle, a V, a Z or a check mark."
            }
        }
    }
}

fn length(path: &[(f32, f32)]) -> f32 {
    path.windows(2).map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1)).sum()
}

/// Can this stroke be a shape (rather than a scroll or a nudge)?
pub fn check(path: &[(f32, f32)]) -> Result<(), NotAShape> {
    let len = length(path);
    if path.len() < 4 || len.is_nan() || len < MIN_LENGTH_MM {
        return Err(NotAShape::TooShort);
    }
    // How far the stroke strays from the line through its ends, against its
    // length: a scroll hugs the line, a shape doesn't.
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
        return Err(NotAShape::Straight);
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

/// How alike two normalised shapes are: 1 for the same, 0 for nothing alike.
pub fn similarity(a: &[[f32; 2]], b: &[[f32; 2]]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let d = a.iter().zip(b).map(|(p, q)| (p[0] - q[0]).hypot(p[1] - q[1])).sum::<f32>() / a.len() as f32;
    (1.0 - d / 0.5).max(0.0)
}

/// The recorded shape this stroke matches best, if one matches well enough.
pub fn best_match(shapes: &[Shape], path: &[(f32, f32)]) -> Option<(u8, f32)> {
    check(path).ok()?;
    let drawn = normalize(path)?;
    shapes
        .iter()
        .map(|s| (s.id, similarity(&drawn, &s.points)))
        .filter(|&(_, score)| score >= MIN_SCORE)
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// Follows two-finger strokes: the path of the fingers' midpoint from the
/// moment two fingers land until they all lift. A stroke that ever had a
/// third finger, or started with one, is not a two-finger stroke.
#[derive(Debug, Default)]
pub struct StrokeTracker {
    path: Vec<(f32, f32)>,
    active: bool,
    two_only: bool,
}

/// Paths longer than this are not shapes anyone draws; stop collecting.
const MAX_PATH: usize = 4_000;

impl StrokeTracker {
    /// Feed every frame; returns a finished two-finger stroke when the fingers lift.
    pub fn update(&mut self, frame: &TouchFrame) -> Option<Vec<(f32, f32)>> {
        let n = frame.contacts.len();
        if n == 0 {
            let done = self.active && self.two_only && self.path.len() >= 2;
            self.active = false;
            return done.then(|| std::mem::take(&mut self.path));
        }
        if !self.active {
            self.active = true;
            self.two_only = n == 2;
            self.path.clear();
        }
        if n > 2 {
            self.two_only = false;
        }
        // One finger lifting first is the end of the shape, not part of it.
        if n == 2 && self.two_only && self.path.len() < MAX_PATH {
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

    fn shape(id: u8, path: &[(f32, f32)]) -> Shape {
        Shape { id, name: format!("shape {id}"), points: normalize(path).unwrap() }
    }

    #[test]
    fn scrolls_and_nudges_are_not_shapes() {
        assert_eq!(check(&polyline(&[(10.0, 10.0), (10.0, 50.0)])), Err(NotAShape::Straight));
        // A scroll that drifts a little sideways is still a scroll.
        assert_eq!(check(&polyline(&[(10.0, 10.0), (12.0, 30.0), (13.0, 50.0)])), Err(NotAShape::Straight));
        assert_eq!(check(&circle(1.5, 1.0)), Err(NotAShape::TooShort));
        assert_eq!(check(&circle(10.0, 1.0)), Ok(()));
        assert_eq!(check(&vee(20.0)), Ok(()));
    }

    #[test]
    fn a_shape_matches_itself_at_any_size_and_place() {
        let shapes = [shape(1, &circle(10.0, 1.0)), shape(2, &vee(20.0)), shape(3, &zed(20.0))];
        let moved: Vec<_> = circle(16.0, 1.0).iter().map(|p| (p.0 + 7.0, p.1 - 3.0)).collect();
        assert_eq!(best_match(&shapes, &moved).map(|m| m.0), Some(1));
        assert_eq!(best_match(&shapes, &vee(35.0)).map(|m| m.0), Some(2));
        assert_eq!(best_match(&shapes, &zed(28.0)).map(|m| m.0), Some(3));
    }

    #[test]
    fn a_sloppy_drawing_still_matches() {
        let shapes = [shape(1, &circle(10.0, 1.0)), shape(2, &vee(20.0)), shape(3, &zed(20.0))];
        let wobbly: Vec<_> =
            vee(24.0).iter().enumerate().map(|(i, p)| (p.0 + (i as f32 * 1.3).sin() * 0.8, p.1)).collect();
        assert_eq!(best_match(&shapes, &wobbly).map(|m| m.0), Some(2));
    }

    #[test]
    fn direction_and_orientation_matter() {
        let shapes = [shape(1, &vee(20.0))];
        let upside_down = polyline(&[(10.0, 30.0), (20.0, 10.0), (30.0, 30.0)]);
        assert_eq!(best_match(&shapes, &upside_down), None);
        let mut backwards = zed(20.0);
        backwards.reverse();
        assert_eq!(best_match(&[shape(3, &zed(20.0))], &backwards), None);
    }

    #[test]
    fn nothing_recorded_matches_nothing() {
        assert_eq!(best_match(&[], &circle(10.0, 1.0)), None);
        assert_eq!(best_match(&[shape(1, &circle(10.0, 1.0))], &zed(20.0)), None);
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
        assert_eq!(best_match(&[shape(2, &vee(20.0))], &path).map(|m| m.0), Some(2));
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
    fn degenerate_strokes_never_make_nan_shapes() {
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
        let shapes = [shape(1, &circle(10.0, 1.0)), shape(2, &vee(20.0))];
        for path in &paths {
            let _ = check(path);
            if let Some(points) = normalize(path) {
                assert!(finite(&points) && points.len() == POINTS, "{path:?} gave {points:?}");
            }
            if let Some((_, score)) = best_match(&shapes, path) {
                assert!(score.is_finite(), "{path:?}");
            }
        }
        assert_eq!(normalize(&[(5.0, 5.0); 500]), None, "a stroke that never moved has no shape");
        assert_eq!(check(&[(nan, nan); 10]), Err(NotAShape::TooShort));
    }

    #[test]
    fn similarity_is_never_nan() {
        let good = shape(1, &circle(10.0, 1.0)).points;
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
        assert_eq!(best_match(&[shape(2, &vee(20.0))], &path).map(|m| m.0), Some(2));
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
        let _ = best_match(&[shape(1, &circle(10.0, 1.0))], &path);
    }

    /// Random strokes with random finger counts, broken positions and lifts.
    #[test]
    fn fuzzed_strokes_never_panic_or_make_nan_shapes() {
        let mut rng = crate::Rng::new(0x2545_F491_4F6C_DD1D);
        let shapes: Vec<Shape> =
            (1..=MAX_SHAPES).map(|id| shape(id, &circle(5.0 + id as f32, 0.5 + id as f32 / 20.0))).collect();
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
                if let Some((id, score)) = best_match(&shapes, &path) {
                    assert!((1..=MAX_SHAPES).contains(&id) && score.is_finite());
                }
            }
            if rng.below(1_000) == 0 {
                t.reset();
            }
        }
        assert!(strokes > 100, "{strokes} strokes");
    }

    #[test]
    fn matching_against_every_shape_is_quick() {
        let shapes: Vec<Shape> =
            (1..=MAX_SHAPES).map(|id| shape(id, &circle(5.0 + id as f32, 0.5 + id as f32 / 20.0))).collect();
        // A long stroke: about two seconds of drawing at 125 Hz.
        let path: Vec<(f32, f32)> = circle(14.0, 1.0)
            .windows(2)
            .flat_map(|w| (0..4).map(move |i| (w[0].0 + (w[1].0 - w[0].0) * i as f32 / 4.0, w[0].1)))
            .collect();
        let runs = 2_000;
        let start = std::time::Instant::now();
        let mut matched = 0;
        for _ in 0..runs {
            matched += best_match(&shapes, std::hint::black_box(&path)).is_some() as u32;
        }
        let each = start.elapsed() / runs;
        // A few microseconds in release; the budget leaves room for slow CI runners.
        let budget = std::time::Duration::from_micros(if cfg!(debug_assertions) { 2_000 } else { 200 });
        assert!(each < budget, "{each:?} per stroke");
        assert_eq!(matched, runs, "the circle matches one of the circles");
    }
}
