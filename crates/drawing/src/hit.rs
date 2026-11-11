//! Hit-testing strokes and fills, used by the whole-stroke eraser (ADR-001,
//! ADR-007).

use animation_core::{FillId, StrokeId, Vec2};

use crate::fill::Fill;
use crate::stroke::{Stroke, VectorDrawing};
use crate::tessellate::collect_nodes;

impl Stroke {
    /// Whether a circle at `point` with `radius` (document units) touches the
    /// stroke's painted area.
    ///
    /// Uses the same shape as [`tessellate`](crate::tessellate): a circle of
    /// radius `width * pressure / 2` at every point, joined along each
    /// segment with the radius interpolated linearly. Exact geometry, so it
    /// does not depend on tessellation tolerance. A negative or non-finite
    /// `radius` counts as `0.0`; a non-finite `point` never hits.
    pub fn hit_test(&self, point: Vec2, radius: f32) -> bool {
        if !point.is_finite() {
            return false;
        }
        let width = self.style.width;
        if !width.is_finite() || width <= 0.0 {
            return false;
        }
        let radius = if radius.is_finite() {
            radius.max(0.0)
        } else {
            0.0
        };
        // Cheap reject using the stroke bounds.
        if let Some((min, max)) = self.bounds() {
            let r = Vec2::splat(radius);
            if point.cmplt(min - r).any() || point.cmpgt(max + r).any() {
                return false;
            }
        }

        let nodes = collect_nodes(self);
        let hit = |stroke_r: f32, dist: f32| stroke_r > 0.0 && dist <= stroke_r + radius;
        match nodes.as_slice() {
            [] => false,
            [(pos, r)] => hit(*r, pos.distance(point)),
            _ => nodes.windows(2).any(|w| {
                let ((p0, r0), (p1, r1)) = (w[0], w[1]);
                let seg = p1 - p0;
                let len_sq = seg.length_squared();
                let t = if len_sq > 0.0 {
                    ((point - p0).dot(seg) / len_sq).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let closest = p0 + seg * t;
                let r = r0 + (r1 - r0) * t;
                // Check the end circles too: with a radius change, the
                // closest point on the centre line is not always where the
                // painted edge is nearest.
                hit(r, closest.distance(point))
                    || hit(r0, p0.distance(point))
                    || hit(r1, p1.distance(point))
            }),
        }
    }
}

impl Fill {
    /// Whether a circle at `point` with `radius` (document units) touches the
    /// fill: the point is inside it (even-odd) or within `radius` of one of
    /// its contours. A negative or non-finite `radius` counts as `0.0`; a
    /// non-finite `point` never hits.
    pub fn hit_test(&self, point: Vec2, radius: f32) -> bool {
        if !point.is_finite() {
            return false;
        }
        let radius = if radius.is_finite() {
            radius.max(0.0)
        } else {
            0.0
        };
        let Some((min, max)) = self.bounds() else {
            return false;
        };
        let r = Vec2::splat(radius);
        if point.cmplt(min - r).any() || point.cmpgt(max + r).any() {
            return false;
        }
        if self.contains(point) {
            return true;
        }
        let r_sq = radius * radius;
        self.contours.iter().any(|contour| {
            let n = contour.len();
            (0..n).any(|i| {
                let (a, b) = (contour[i], contour[(i + 1) % n]);
                let ab = b - a;
                let len_sq = ab.length_squared();
                let t = if len_sq > 0.0 {
                    ((point - a).dot(ab) / len_sq).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (a + ab * t).distance_squared(point) <= r_sq
            })
        })
    }
}

impl VectorDrawing {
    /// IDs of all fills that a circle at `point` with `radius` touches, in
    /// drawing order (bottom-to-top).
    pub fn fills_hit(&self, point: Vec2, radius: f32) -> Vec<FillId> {
        self.fills()
            .iter()
            .filter(|f| f.hit_test(point, radius))
            .map(|f| f.id)
            .collect()
    }

    /// IDs of all strokes that a circle at `point` with `radius` touches, in
    /// drawing order (bottom-to-top). For "erase only the topmost", take the
    /// last element.
    pub fn strokes_hit(&self, point: Vec2, radius: f32) -> Vec<StrokeId> {
        self.strokes()
            .iter()
            .filter(|s| s.hit_test(point, radius))
            .map(|s| s.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use crate::stroke::{BrushStyle, Stroke, StrokePoint, VectorDrawing};
    use animation_core::Vec2;

    fn stroke(points: &[(f32, f32, f32)], width: f32) -> Stroke {
        Stroke::new(
            points
                .iter()
                .map(|&(x, y, pressure)| StrokePoint {
                    pos: Vec2::new(x, y),
                    pressure,
                })
                .collect(),
            BrushStyle {
                width,
                ..Default::default()
            },
        )
    }

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    #[test]
    fn line_hit_at_near_and_far() {
        // Horizontal line, half-width 2.
        let s = stroke(&[(0.0, 0.0, 1.0), (20.0, 0.0, 1.0)], 4.0);
        // On the centre line.
        assert!(s.hit_test(v(10.0, 0.0), 0.0));
        // Inside the painted width.
        assert!(s.hit_test(v(10.0, 1.9), 0.0));
        // Just outside the width, but the eraser radius reaches.
        assert!(!s.hit_test(v(10.0, 2.5), 0.0));
        assert!(s.hit_test(v(10.0, 2.5), 0.6));
        assert!(!s.hit_test(v(10.0, 2.5), 0.4));
        // Round caps.
        assert!(s.hit_test(v(-1.9, 0.0), 0.0));
        assert!(s.hit_test(v(21.0, 1.0), 0.0));
        assert!(!s.hit_test(v(22.5, 0.0), 0.0));
        // Far away.
        assert!(!s.hit_test(v(100.0, 100.0), 5.0));
        assert!(!s.hit_test(v(10.0, -50.0), 10.0));
    }

    #[test]
    fn dot_stroke() {
        let s = stroke(&[(5.0, 5.0, 1.0)], 10.0);
        assert!(s.hit_test(v(5.0, 5.0), 0.0));
        assert!(s.hit_test(v(9.9, 5.0), 0.0));
        assert!(!s.hit_test(v(11.0, 5.0), 0.0));
        assert!(s.hit_test(v(11.0, 5.0), 1.5));
    }

    #[test]
    fn pressure_affects_hit_width() {
        let s = stroke(&[(0.0, 0.0, 0.25), (20.0, 0.0, 0.25)], 8.0);
        assert!(s.hit_test(v(10.0, 0.9), 0.0));
        assert!(!s.hit_test(v(10.0, 1.5), 0.0));
        let taper = stroke(&[(0.0, 0.0, 1.0), (20.0, 0.0, 0.0)], 8.0);
        assert!(taper.hit_test(v(0.0, 3.5), 0.0));
        assert!(!taper.hit_test(v(19.0, 1.0), 0.0));
    }

    #[test]
    fn degenerate_inputs_never_panic() {
        assert!(!stroke(&[], 4.0).hit_test(v(0.0, 0.0), 10.0));
        let s = stroke(&[(0.0, 0.0, 1.0), (0.0, 0.0, 1.0)], 4.0);
        assert!(s.hit_test(v(1.0, 0.0), 0.0));
        assert!(!s.hit_test(v(f32::NAN, 0.0), 1.0));
        assert!(s.hit_test(v(1.0, 0.0), f32::NAN));
        assert!(!s.hit_test(v(3.0, 0.0), -5.0));
        assert!(!stroke(&[(0.0, 0.0, 1.0)], 0.0).hit_test(v(0.0, 0.0), 1.0));
        assert!(!stroke(&[(0.0, 0.0, 0.0)], 4.0).hit_test(v(0.0, 0.0), 1.0));
    }

    #[test]
    fn strokes_hit_returns_ids_in_order() {
        let mut d = VectorDrawing::new();
        let a = stroke(&[(0.0, 0.0, 1.0), (20.0, 0.0, 1.0)], 4.0);
        let b = stroke(&[(10.0, -10.0, 1.0), (10.0, 10.0, 1.0)], 4.0);
        let c = stroke(&[(100.0, 100.0, 1.0)], 4.0);
        let (ia, ib, ic) = (a.id, b.id, c.id);
        d.push_stroke(a);
        d.push_stroke(b);
        d.push_stroke(c);
        assert_eq!(d.strokes_hit(v(10.0, 0.0), 1.0), vec![ia, ib]);
        assert_eq!(d.strokes_hit(v(2.0, 0.0), 1.0), vec![ia]);
        assert_eq!(d.strokes_hit(v(100.0, 101.0), 0.0), vec![ic]);
        assert!(d.strokes_hit(v(50.0, 50.0), 2.0).is_empty());
        assert!(
            VectorDrawing::new()
                .strokes_hit(v(0.0, 0.0), 5.0)
                .is_empty()
        );
    }

    #[test]
    fn fill_hit_inside_near_edge_and_hole() {
        use crate::fill::{Fill, FillShape};
        use animation_core::Rgba;
        let square = |lo: f32, hi: f32| vec![v(lo, lo), v(hi, lo), v(hi, hi), v(lo, hi)];
        let ring = Fill::new(
            FillShape {
                contours: vec![square(0.0, 100.0), square(40.0, 60.0)],
            },
            Rgba::rgb(0.0, 0.0, 0.0),
            1.0,
        );
        assert!(ring.hit_test(v(10.0, 10.0), 0.0));
        // In the hole, only an eraser reaching the hole's edge hits.
        assert!(!ring.hit_test(v(50.0, 50.0), 5.0));
        assert!(ring.hit_test(v(50.0, 50.0), 10.5));
        // Outside, near the edge.
        assert!(!ring.hit_test(v(105.0, 50.0), 4.0));
        assert!(ring.hit_test(v(105.0, 50.0), 5.5));
        assert!(!ring.hit_test(v(f32::NAN, 50.0), 5.0));
        let mut d = VectorDrawing::new();
        d.push_fill(ring.clone());
        assert_eq!(d.fills_hit(v(10.0, 10.0), 1.0), vec![ring.id]);
        assert!(d.fills_hit(v(500.0, 10.0), 1.0).is_empty());
    }
}
