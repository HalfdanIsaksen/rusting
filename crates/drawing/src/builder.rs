//! Turning raw pointer input into a [`Stroke`].

use crate::stroke::{BrushStyle, Stroke, StrokePoint};

/// Default minimum distance between kept points, in document units.
pub const DEFAULT_MIN_DISTANCE: f32 = 1.0;

/// Default number of smoothing passes applied by [`StrokeBuilder::finish`].
pub const DEFAULT_SMOOTHING_PASSES: u32 = 1;

/// Collects input points for one stroke while the pen is down.
///
/// - Points closer than [`min_distance`](Self::with_min_distance) to the last
///   kept point are dropped. The most recent dropped point is remembered and
///   added on [`finish`](Self::finish), so the stroke always ends where the
///   pen lifted.
/// - Non-finite positions are ignored and pressure is clamped to `0.0..=1.0`
///   (a non-finite pressure becomes `1.0`).
/// - [`finish`](Self::finish) applies light smoothing: a `[1/4, 1/2, 1/4]`
///   moving average over positions and pressures, repeated
///   [`smoothing passes`](Self::with_smoothing) times. The first and last
///   points never move.
///
/// ```
/// use drawing::{BrushStyle, StrokeBuilder, StrokePoint};
/// use animation_core::Vec2;
///
/// let mut b = StrokeBuilder::begin(BrushStyle::default());
/// b.add_point(StrokePoint::new(Vec2::new(0.0, 0.0)));
/// b.add_point(StrokePoint::new(Vec2::new(10.0, 0.0)));
/// let stroke = b.finish().expect("two points make a stroke");
/// assert_eq!(stroke.points.len(), 2);
/// ```
#[derive(Clone, Debug)]
pub struct StrokeBuilder {
    style: BrushStyle,
    points: Vec<StrokePoint>,
    /// The latest point that was dropped for being too close, if it came
    /// after the last kept point.
    pending: Option<StrokePoint>,
    min_distance: f32,
    smoothing_passes: u32,
}

impl StrokeBuilder {
    /// Starts a new stroke with the given brush and default settings.
    pub fn begin(style: BrushStyle) -> Self {
        Self {
            style,
            points: Vec::new(),
            pending: None,
            min_distance: DEFAULT_MIN_DISTANCE,
            smoothing_passes: DEFAULT_SMOOTHING_PASSES,
        }
    }

    /// Sets the minimum distance between kept points, in document units.
    /// Negative or non-finite values are treated as `0.0` (keep all points
    /// except exact duplicates).
    #[must_use]
    pub fn with_min_distance(mut self, min_distance: f32) -> Self {
        self.min_distance = if min_distance.is_finite() {
            min_distance.max(0.0)
        } else {
            0.0
        };
        self
    }

    /// Sets how many smoothing passes [`finish`](Self::finish) applies.
    /// `0` disables smoothing.
    #[must_use]
    pub fn with_smoothing(mut self, passes: u32) -> Self {
        self.smoothing_passes = passes;
        self
    }

    /// The brush this stroke uses.
    pub fn style(&self) -> &BrushStyle {
        &self.style
    }

    /// Points kept so far (unsmoothed). Useful for a live preview.
    pub fn points(&self) -> &[StrokePoint] {
        &self.points
    }

    /// Whether no point has been accepted yet.
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// Adds an input point. Returns `true` if it was kept, `false` if it was
    /// dropped (too close to the previous point, or not finite).
    pub fn add_point(&mut self, point: StrokePoint) -> bool {
        if !point.pos.is_finite() {
            return false;
        }
        let point = sanitize(point);
        if let Some(last) = self.points.last() {
            let d = last.pos.distance(point.pos);
            if d == 0.0 || d < self.min_distance {
                if d > 0.0 {
                    self.pending = Some(point);
                }
                return false;
            }
        }
        self.points.push(point);
        self.pending = None;
        true
    }

    /// Finishes the stroke. Returns `None` if no point was added. A single
    /// click gives a one-point stroke, which tessellates as a dot.
    pub fn finish(mut self) -> Option<Stroke> {
        if let Some(p) = self.pending.take() {
            self.points.push(p);
        }
        if self.points.is_empty() {
            return None;
        }
        for _ in 0..self.smoothing_passes {
            smooth_pass(&mut self.points);
        }
        Some(Stroke::new(self.points, self.style))
    }
}

fn sanitize(p: StrokePoint) -> StrokePoint {
    let pressure = if p.pressure.is_finite() {
        p.pressure.clamp(0.0, 1.0)
    } else {
        1.0
    };
    StrokePoint {
        pos: p.pos,
        pressure,
    }
}

/// One `[1/4, 1/2, 1/4]` pass; endpoints stay fixed.
fn smooth_pass(points: &mut [StrokePoint]) {
    if points.len() < 3 {
        return;
    }
    let mut prev = points[0];
    for i in 1..points.len() - 1 {
        let cur = points[i];
        let next = points[i + 1];
        points[i] = StrokePoint {
            pos: prev.pos * 0.25 + cur.pos * 0.5 + next.pos * 0.25,
            pressure: prev.pressure * 0.25 + cur.pressure * 0.5 + next.pressure * 0.25,
        };
        prev = cur;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use animation_core::Vec2;

    fn p(x: f32, y: f32) -> StrokePoint {
        StrokePoint::new(Vec2::new(x, y))
    }

    fn pp(x: f32, y: f32, pressure: f32) -> StrokePoint {
        StrokePoint {
            pos: Vec2::new(x, y),
            pressure,
        }
    }

    #[test]
    fn empty_input_gives_none() {
        assert!(
            StrokeBuilder::begin(BrushStyle::default())
                .finish()
                .is_none()
        );
    }

    #[test]
    fn single_click_is_a_dot() {
        let mut b = StrokeBuilder::begin(BrushStyle::default());
        b.add_point(p(3.0, 4.0));
        // Exact duplicates of a click are ignored.
        b.add_point(p(3.0, 4.0));
        let s = b.finish().unwrap();
        assert_eq!(s.points, vec![p(3.0, 4.0)]);
    }

    #[test]
    fn drops_points_closer_than_min_distance() {
        let mut b = StrokeBuilder::begin(BrushStyle::default())
            .with_min_distance(2.0)
            .with_smoothing(0);
        assert!(b.add_point(p(0.0, 0.0)));
        assert!(!b.add_point(p(0.5, 0.0)));
        assert!(!b.add_point(p(1.9, 0.0)));
        assert!(b.add_point(p(2.0, 0.0)));
        assert!(!b.add_point(p(2.0, 0.0)));
        assert!(b.add_point(p(5.0, 0.0)));
        assert_eq!(b.points(), &[p(0.0, 0.0), p(2.0, 0.0), p(5.0, 0.0)]);
    }

    #[test]
    fn last_dropped_point_is_kept_as_endpoint() {
        let mut b = StrokeBuilder::begin(BrushStyle::default())
            .with_min_distance(2.0)
            .with_smoothing(0);
        b.add_point(p(0.0, 0.0));
        b.add_point(p(4.0, 0.0));
        b.add_point(p(5.0, 0.0)); // too close, but it's where the pen lifted
        let s = b.finish().unwrap();
        assert_eq!(s.points.last().unwrap().pos, Vec2::new(5.0, 0.0));
        assert_eq!(s.points.len(), 3);
    }

    #[test]
    fn ignores_non_finite_and_clamps_pressure() {
        let mut b = StrokeBuilder::begin(BrushStyle::default()).with_smoothing(0);
        assert!(!b.add_point(p(f32::NAN, 0.0)));
        assert!(!b.add_point(p(0.0, f32::INFINITY)));
        assert!(b.is_empty());
        b.add_point(pp(0.0, 0.0, 3.0));
        b.add_point(pp(10.0, 0.0, -1.0));
        b.add_point(pp(20.0, 0.0, f32::NAN));
        let s = b.finish().unwrap();
        let pressures: Vec<f32> = s.points.iter().map(|p| p.pressure).collect();
        assert_eq!(pressures, vec![1.0, 0.0, 1.0]);
    }

    #[test]
    fn smoothing_keeps_endpoints_and_reduces_zigzag() {
        let raw: Vec<StrokePoint> = (0..10)
            .map(|i| p(i as f32 * 5.0, if i % 2 == 0 { 0.0 } else { 4.0 }))
            .collect();
        let mut b = StrokeBuilder::begin(BrushStyle::default()).with_smoothing(2);
        for &pt in &raw {
            b.add_point(pt);
        }
        let s = b.finish().unwrap();
        assert_eq!(s.points.len(), raw.len());
        assert_eq!(s.points.first(), raw.first());
        assert_eq!(s.points.last(), raw.last());
        let zigzag = |pts: &[StrokePoint]| -> f32 {
            pts.windows(2)
                .map(|w| (w[1].pos.y - w[0].pos.y).abs())
                .sum()
        };
        assert!(zigzag(&s.points) < zigzag(&raw) * 0.5);
    }

    #[test]
    fn smoothing_keeps_style() {
        let style = BrushStyle {
            width: 9.0,
            ..Default::default()
        };
        let mut b = StrokeBuilder::begin(style);
        b.add_point(p(0.0, 0.0));
        b.add_point(p(10.0, 0.0));
        assert_eq!(b.finish().unwrap().style, style);
    }
}
