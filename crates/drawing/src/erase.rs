//! Partial erasing: cutting the part of a stroke that an eraser passes over.
//!
//! The eraser is a circle of radius `R` dragged along a polyline, so the
//! erased area is the union of capsules around the path's segments.
//! [`Stroke::erase`] removes the stretches of the stroke's **centre line**
//! that the eraser touches and keeps the rest as new strokes.
//!
//! # Brush width and the approximation
//! A stroke is painted as circles of radius `r = width * pressure / 2` along
//! its centre line, with round caps. A centre point counts as erased when it
//! lies within `R + r` of the eraser path, that is, when the eraser touches
//! any part of the stroke's painted cross-section there. Each remaining piece
//! then ends exactly where its round cap is tangent to the eraser boundary,
//! so no ink is left inside the eraser circle, and where the eraser crosses
//! the stroke head-on the gap on the centre line is exactly the eraser's
//! width.
//!
//! The limit is that a centre-line cut always removes the full width of the
//! stroke: the eraser can't take a notch out of one side. So a thick stroke
//! loses slightly more than the eraser covered: ink beside the eraser near
//! the cut, and a whole cross-section where the eraser only grazes an edge.
//! Exact results would need outline (polygon) geometry, which strokes don't
//! have.
//!
//! # Precision
//! Cut points are found by sampling each segment at a spacing of at most half
//! the local `R + r`, then bisecting between a sample inside and one outside,
//! so they land on the eraser boundary (to well under 0.01 units), not on the
//! nearest original point. Pressure is interpolated linearly at each cut. A
//! graze shorter than the sample spacing can be missed; it would remove at
//! most a sliver.

use animation_core::Vec2;

use crate::stroke::{Stroke, StrokePoint};

/// Pieces shorter than this (document units, along the centre line) are
/// dropped after a cut instead of being kept as tiny blobs.
pub const MIN_FRAGMENT_LENGTH: f32 = 0.5;

/// Bisection steps per cut; 20 halvings resolve a cut on a 1000-unit segment
/// to about 0.001 units.
const BISECT_STEPS: u32 = 20;
/// Smallest sample spacing, so a zero-width stroke with a zero eraser radius
/// doesn't sample forever.
const MIN_SAMPLE_STEP: f32 = 0.05;
/// Upper bound on samples per segment.
const MAX_SAMPLES_PER_SEGMENT: f32 = 4096.0;
/// Segments shorter than this are treated as points.
const LENGTH_EPSILON: f32 = 1e-4;

/// What erasing did to a stroke. See [`Stroke::erase`].
#[derive(Clone, Debug, PartialEq)]
pub enum EraseResult {
    /// The eraser didn't touch the stroke.
    Unchanged,
    /// The eraser covered the whole stroke (or left only fragments shorter
    /// than [`MIN_FRAGMENT_LENGTH`]).
    Removed,
    /// The stroke was cut. These pieces replace it, in drawing order along
    /// the original. There is at least one piece; a cut at an end gives one
    /// shorter piece. Each piece has a new ID and the original's style.
    Split(Vec<Stroke>),
}

impl EraseResult {
    /// The strokes that replace the original, or `None` if it is unchanged.
    /// `Some(empty)` means the stroke is removed.
    pub fn into_replacements(self) -> Option<Vec<Stroke>> {
        match self {
            Self::Unchanged => None,
            Self::Removed => Some(Vec::new()),
            Self::Split(pieces) => Some(pieces),
        }
    }
}

/// Clamped, finite pressure (non-finite counts as full pressure, like the
/// tessellator).
fn pressure_of(p: &StrokePoint) -> f32 {
    if p.pressure.is_finite() {
        p.pressure.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn lerp_point(a: StrokePoint, b: StrokePoint, t: f32) -> StrokePoint {
    StrokePoint {
        pos: a.pos.lerp(b.pos, t),
        pressure: a.pressure + (b.pressure - a.pressure) * t,
    }
}

fn polyline_length(points: &[StrokePoint]) -> f32 {
    points.windows(2).map(|w| w[0].pos.distance(w[1].pos)).sum()
}

/// The eraser path: a polyline swept by a circle.
struct EraserPath<'a> {
    points: &'a [Vec2],
    radius: f32,
    min: Vec2,
    max: Vec2,
}

impl EraserPath<'_> {
    /// Distance from `p` to the path's centre line.
    fn distance(&self, p: Vec2) -> f32 {
        match self.points {
            [] => f32::INFINITY,
            [only] => only.distance(p),
            _ => self
                .points
                .windows(2)
                .map(|w| {
                    let (a, b) = (w[0], w[1]);
                    let ab = b - a;
                    let len_sq = ab.length_squared();
                    let t = if len_sq > 0.0 {
                        ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    (a + ab * t).distance(p)
                })
                .fold(f32::INFINITY, f32::min),
        }
    }

    /// Whether the eraser touches the painted circle of radius `half_width`
    /// around `point.pos`.
    fn covers(&self, point: StrokePoint, half_width: f32) -> bool {
        self.distance(point.pos) < self.radius + point.pressure * half_width
    }

    /// Whether a box `[lo, hi]` grown by `pad` can reach the path at all.
    fn may_touch(&self, lo: Vec2, hi: Vec2, pad: f32) -> bool {
        let pad = Vec2::splat(pad + self.radius);
        (lo - pad).cmple(self.max).all() && (hi + pad).cmpge(self.min).all()
    }
}

impl Stroke {
    /// Cuts away the parts of this stroke that an eraser circle of `radius`
    /// (document units) touches while it is dragged along `eraser_path`.
    ///
    /// A one-point path is a single dab. Non-finite path points are skipped;
    /// a negative or non-finite radius counts as `0.0`. Points of the stroke
    /// with a non-finite position are skipped. The stroke itself is not
    /// changed: the result says what should replace it.
    ///
    /// See the [module docs](crate::erase) for how brush width is handled
    /// and how precise the cut is.
    pub fn erase(&self, eraser_path: &[Vec2], radius: f32) -> EraseResult {
        let path_points: Vec<Vec2> = eraser_path
            .iter()
            .copied()
            .filter(|p| p.is_finite())
            .collect();
        let Some(&first) = path_points.first() else {
            return EraseResult::Unchanged;
        };
        let (min, max) = path_points
            .iter()
            .fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let eraser = EraserPath {
            points: &path_points,
            radius: if radius.is_finite() {
                radius.max(0.0)
            } else {
                0.0
            },
            min,
            max,
        };

        let half_width = if self.style.width.is_finite() {
            (self.style.width * 0.5).max(0.0)
        } else {
            0.0
        };
        let points: Vec<StrokePoint> = self
            .points
            .iter()
            .filter(|p| p.pos.is_finite())
            .map(|p| StrokePoint {
                pos: p.pos,
                pressure: pressure_of(p),
            })
            .collect();
        let Some(&start) = points.first() else {
            return EraseResult::Unchanged;
        };

        // Cheap reject using the stroke bounds.
        let (lo, hi) = points.iter().fold((start.pos, start.pos), |(lo, hi), p| {
            (lo.min(p.pos), hi.max(p.pos))
        });
        if !eraser.may_touch(lo, hi, half_width) {
            return EraseResult::Unchanged;
        }

        // A dot (or a stroke whose points all coincide) is all or nothing.
        if polyline_length(&points) < LENGTH_EPSILON {
            let hit = points.iter().any(|p| eraser.covers(*p, half_width));
            return if hit {
                EraseResult::Removed
            } else {
                EraseResult::Unchanged
            };
        }

        let mut pieces: Vec<Vec<StrokePoint>> = Vec::new();
        let mut current: Vec<StrokePoint> = Vec::new();
        let mut inside = eraser.covers(start, half_width);
        let mut cut = inside;
        if !inside {
            current.push(start);
        }

        for w in points.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = a.pos.distance(b.pos);
            let seg_lo = a.pos.min(b.pos);
            let seg_hi = a.pos.max(b.pos);
            let max_r = a.pressure.max(b.pressure) * half_width;
            let b_inside = eraser.covers(b, half_width);
            if len < LENGTH_EPSILON || !eraser.may_touch(seg_lo, seg_hi, max_r) {
                // Too short to cut, or too far away to touch: only the end
                // point's own state matters.
                if b_inside != inside {
                    cut = true;
                    if b_inside {
                        finish_piece(&mut pieces, &mut current);
                    }
                    inside = b_inside;
                }
                if !inside {
                    current.push(b);
                }
                continue;
            }

            let min_r = a.pressure.min(b.pressure) * half_width;
            let step = ((eraser.radius + min_r) * 0.5).max(MIN_SAMPLE_STEP);
            let samples = (len / step).ceil().clamp(1.0, MAX_SAMPLES_PER_SEGMENT) as u32;
            let mut t0 = 0.0;
            for k in 1..=samples {
                let t1 = k as f32 / samples as f32;
                let now_inside = if k == samples {
                    b_inside
                } else {
                    eraser.covers(lerp_point(a, b, t1), half_width)
                };
                if now_inside != inside {
                    cut = true;
                    let tc = bisect(&eraser, a, b, half_width, t0, t1, inside);
                    let cut_point = lerp_point(a, b, tc);
                    if now_inside {
                        current.push(cut_point);
                        finish_piece(&mut pieces, &mut current);
                    } else {
                        current.push(cut_point);
                    }
                    inside = now_inside;
                }
                t0 = t1;
            }
            if !inside {
                current.push(b);
            }
        }
        finish_piece(&mut pieces, &mut current);

        if !cut {
            return EraseResult::Unchanged;
        }
        let strokes: Vec<Stroke> = pieces
            .into_iter()
            .filter(|p| polyline_length(p) >= MIN_FRAGMENT_LENGTH)
            .map(|p| Stroke::new(p, self.style).with_placement(self.placement))
            .collect();
        if strokes.is_empty() {
            EraseResult::Removed
        } else {
            EraseResult::Split(strokes)
        }
    }
}

/// Moves `current` into `pieces` (dropping consecutive duplicate points).
fn finish_piece(pieces: &mut Vec<Vec<StrokePoint>>, current: &mut Vec<StrokePoint>) {
    let mut piece = std::mem::take(current);
    piece.dedup_by(|b, a| a.pos.distance(b.pos) < LENGTH_EPSILON);
    if !piece.is_empty() {
        pieces.push(piece);
    }
}

/// Finds the boundary between `t_lo` (state `lo_inside`) and `t_hi` (the
/// other state) on segment `a`–`b`.
fn bisect(
    eraser: &EraserPath<'_>,
    a: StrokePoint,
    b: StrokePoint,
    half_width: f32,
    mut t_lo: f32,
    mut t_hi: f32,
    lo_inside: bool,
) -> f32 {
    for _ in 0..BISECT_STEPS {
        let mid = 0.5 * (t_lo + t_hi);
        if eraser.covers(lerp_point(a, b, mid), half_width) == lo_inside {
            t_lo = mid;
        } else {
            t_hi = mid;
        }
    }
    0.5 * (t_lo + t_hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::BrushStyle;
    use animation_core::Rgba;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    fn style(width: f32) -> BrushStyle {
        BrushStyle {
            width,
            color: Rgba::rgb(0.1, 0.2, 0.3),
            opacity: 0.7,
        }
    }

    fn stroke(points: &[(f32, f32)], width: f32) -> Stroke {
        Stroke::new(
            points
                .iter()
                .map(|&(x, y)| StrokePoint::new(v(x, y)))
                .collect(),
            style(width),
        )
    }

    fn split(result: EraseResult) -> Vec<Stroke> {
        match result {
            EraseResult::Split(pieces) => pieces,
            other => panic!("expected a split, got {other:?}"),
        }
    }

    fn assert_finite(pieces: &[Stroke]) {
        for p in pieces {
            for pt in &p.points {
                assert!(pt.pos.is_finite() && pt.pressure.is_finite(), "{pt:?}");
            }
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-2
    }

    #[test]
    fn cut_in_the_middle_gives_two_pieces_on_the_boundary() {
        // Two far-apart points: the cut must not snap to them.
        let s = stroke(&[(0.0, 0.0), (100.0, 0.0)], 4.0);
        let pieces = split(s.erase(&[v(50.0, -30.0), v(50.0, 30.0)], 5.0));
        assert_eq!(pieces.len(), 2);
        // Erased where the centre line is within R + r = 5 + 2 of the path.
        let left = pieces[0].points.last().unwrap().pos;
        let right = pieces[1].points.first().unwrap().pos;
        assert!(close(left.x, 43.0), "{left}");
        assert!(close(right.x, 57.0), "{right}");
        assert_eq!(pieces[0].points.first().unwrap().pos, v(0.0, 0.0));
        assert_eq!(pieces[1].points.last().unwrap().pos, v(100.0, 0.0));
        // New IDs, same style.
        for p in &pieces {
            assert_ne!(p.id, s.id);
            assert_eq!(p.style, s.style);
        }
        assert_ne!(pieces[0].id, pieces[1].id);
    }

    #[test]
    fn cut_at_an_end_gives_one_shorter_piece() {
        let s = stroke(&[(0.0, 0.0), (50.0, 0.0), (100.0, 0.0)], 2.0);
        let pieces = split(s.erase(&[v(100.0, 0.0)], 10.0));
        assert_eq!(pieces.len(), 1);
        let end = pieces[0].points.last().unwrap().pos;
        assert!(close(end.x, 89.0), "{end}");
        assert_eq!(pieces[0].points[..2], s.points[..2]);
    }

    #[test]
    fn full_cover_removes_the_stroke() {
        let s = stroke(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)], 2.0);
        assert_eq!(s.erase(&[v(5.0, 5.0)], 20.0), EraseResult::Removed);
        // A drag along the whole stroke too.
        assert_eq!(
            s.erase(&[v(-5.0, 0.0), v(10.0, 0.0), v(10.0, 15.0)], 2.0),
            EraseResult::Removed
        );
    }

    #[test]
    fn a_miss_is_unchanged() {
        let s = stroke(&[(0.0, 0.0), (100.0, 0.0)], 4.0);
        // Just outside R + r.
        assert_eq!(
            s.erase(&[v(50.0, 7.5), v(60.0, 7.5)], 5.0),
            EraseResult::Unchanged
        );
        assert_eq!(s.erase(&[v(500.0, 500.0)], 5.0), EraseResult::Unchanged);
        assert_eq!(s.erase(&[], 5.0), EraseResult::Unchanged);
        // Grazing the painted edge counts as a touch.
        assert!(matches!(
            s.erase(&[v(50.0, 6.5)], 5.0),
            EraseResult::Split(_)
        ));
    }

    #[test]
    fn several_cuts_in_one_drag() {
        let s = stroke(&[(0.0, 0.0), (100.0, 0.0)], 2.0);
        // A zig-zag crossing the line three times.
        let path = [
            v(20.0, -10.0),
            v(20.0, 10.0),
            v(50.0, 10.0),
            v(50.0, -10.0),
            v(80.0, -10.0),
            v(80.0, 10.0),
        ];
        let pieces = split(s.erase(&path, 2.0));
        assert_eq!(pieces.len(), 4);
        assert_finite(&pieces);
        let ends: Vec<(f32, f32)> = pieces
            .iter()
            .map(|p| (p.points[0].pos.x, p.points.last().unwrap().pos.x))
            .collect();
        for (got, want) in ends
            .iter()
            .zip([(0.0, 17.0), (23.0, 47.0), (53.0, 77.0), (83.0, 100.0)])
        {
            assert!(close(got.0, want.0) && close(got.1, want.1), "{ends:?}");
        }
    }

    #[test]
    fn dot_strokes_are_all_or_nothing() {
        let dot = stroke(&[(5.0, 5.0)], 10.0);
        assert_eq!(dot.erase(&[v(5.0, 5.0)], 1.0), EraseResult::Removed);
        // Touches the painted disc (radius 5) from outside.
        assert_eq!(dot.erase(&[v(12.0, 5.0)], 2.5), EraseResult::Removed);
        assert_eq!(dot.erase(&[v(12.0, 5.0)], 1.5), EraseResult::Unchanged);
        // Repeated points behave like a dot.
        let pile = stroke(&[(5.0, 5.0), (5.0, 5.0), (5.0, 5.0)], 10.0);
        assert_eq!(pile.erase(&[v(5.0, 5.0)], 1.0), EraseResult::Removed);
    }

    #[test]
    fn a_180_degree_turn() {
        // Out to x = 100 and straight back.
        let s = stroke(&[(0.0, 0.0), (100.0, 0.0), (0.0, 0.0)], 2.0);
        // Cutting the middle crosses both passes: three pieces.
        let pieces = split(s.erase(&[v(50.0, -10.0), v(50.0, 10.0)], 4.0));
        assert_eq!(pieces.len(), 3);
        assert_finite(&pieces);
        assert_eq!(pieces[1].points.len(), 3, "the tip keeps its turn point");
        assert_eq!(pieces[1].points[1].pos, v(100.0, 0.0));
        // Cutting the tip leaves the two passes.
        let pieces = split(s.erase(&[v(100.0, 0.0)], 4.0));
        assert_eq!(pieces.len(), 2);
        assert_finite(&pieces);
        assert!(close(pieces[0].points.last().unwrap().pos.x, 95.0));
        assert!(close(pieces[1].points[0].pos.x, 95.0));
    }

    #[test]
    fn pressure_is_interpolated_and_narrows_the_cut() {
        let s = Stroke::new(
            vec![
                StrokePoint {
                    pos: v(0.0, 0.0),
                    pressure: 0.0,
                },
                StrokePoint {
                    pos: v(100.0, 0.0),
                    pressure: 1.0,
                },
            ],
            style(10.0),
        );
        let pieces = split(s.erase(&[v(50.0, -30.0), v(50.0, 30.0)], 5.0));
        assert_eq!(pieces.len(), 2);
        let cut = *pieces[0].points.last().unwrap();
        // Pressure follows position along the segment.
        assert!(close(cut.pressure, cut.pos.x / 100.0), "{cut:?}");
        // At the cut, the distance to the eraser equals R + r.
        let r = 5.0 * cut.pressure;
        assert!(close(50.0 - cut.pos.x, 5.0 + r), "{cut:?}");
    }

    #[test]
    fn tiny_fragments_are_dropped() {
        let s = stroke(&[(0.0, 0.0), (100.0, 0.0)], 2.0);
        // Erases 0.2..95.8 (R + r = 47.8): the 0.2 start is dropped.
        let pieces = split(s.erase(&[v(48.0, 0.0)], 46.8));
        assert_eq!(pieces.len(), 1);
        assert!(pieces[0].points[0].pos.x > 90.0);
    }

    #[test]
    fn erasing_a_piece_again_keeps_working() {
        let s = stroke(&[(0.0, 0.0), (100.0, 0.0)], 2.0);
        let first = split(s.erase(&[v(30.0, 0.0)], 3.0));
        let again: Vec<Stroke> = first
            .iter()
            .flat_map(|p| match p.erase(&[v(70.0, 0.0)], 3.0) {
                EraseResult::Unchanged => vec![p.clone()],
                other => other.into_replacements().unwrap(),
            })
            .collect();
        assert_eq!(again.len(), 3);
        assert_eq!(again[0].id, first[0].id, "untouched piece keeps its id");
    }

    #[test]
    fn degenerate_inputs_never_panic_or_nan() {
        let s = stroke(&[(0.0, 0.0), (10.0, 0.0)], 2.0);
        assert_eq!(s.erase(&[v(f32::NAN, 0.0)], 5.0), EraseResult::Unchanged);
        assert!(matches!(
            s.erase(&[v(5.0, 0.0), v(f32::INFINITY, 0.0)], 1.0),
            EraseResult::Split(_)
        ));
        // Non-finite or negative radius counts as zero.
        for r in [f32::NAN, -3.0, f32::INFINITY] {
            let pieces = split(s.erase(&[v(5.0, 0.0)], r));
            assert_eq!(pieces.len(), 2);
            assert_finite(&pieces);
        }
        // Zero width: a crossing with zero radius covers no area, a small
        // radius cuts.
        let thin = stroke(&[(0.0, 0.0), (10.0, 0.0)], 0.0);
        let cross = [v(5.0, -1.0), v(5.0, 1.0)];
        assert_eq!(thin.erase(&cross, 0.0), EraseResult::Unchanged);
        assert_eq!(split(thin.erase(&cross, 0.5)).len(), 2);
        // NaN points in the stroke are skipped; NaN pressure counts as 1.
        let mut odd = stroke(&[(0.0, 0.0), (5.0, 0.0), (10.0, 0.0)], 2.0);
        odd.points[1].pos = v(f32::NAN, 0.0);
        odd.points[2].pressure = f32::NAN;
        let pieces = split(odd.erase(&[v(5.0, 0.0)], 1.0));
        assert_finite(&pieces);
        assert_eq!(
            Stroke::new(vec![], style(2.0)).erase(&[v(0.0, 0.0)], 5.0),
            EraseResult::Unchanged
        );
    }

    #[test]
    fn into_replacements() {
        assert_eq!(EraseResult::Unchanged.into_replacements(), None);
        assert_eq!(EraseResult::Removed.into_replacements(), Some(vec![]));
    }
}
