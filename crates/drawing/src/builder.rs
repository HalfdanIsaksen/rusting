//! Turning raw pointer input into a [`Stroke`].

use crate::stroke::{BrushStyle, Stroke, StrokePoint};
use animation_core::Vec2;

/// Default minimum distance between kept points, in document units.
pub const DEFAULT_MIN_DISTANCE: f32 = 1.0;

/// Default smoothing strength (Gaussian sigma), in document units. `0.0`
/// keeps the input as drawn.
pub const DEFAULT_SMOOTHING: f32 = 0.0;

/// Collects input points for one stroke while the pen is down.
///
/// - Points closer than [`min_distance`](Self::with_min_distance) to the last
///   kept point are dropped. The most recent dropped point is remembered and
///   added on [`finish`](Self::finish), so the stroke always ends where the
///   pen lifted.
/// - Non-finite positions are ignored and pressure is clamped to `0.0..=1.0`
///   (a non-finite pressure becomes `1.0`).
/// - With [`smoothing`](Self::with_smoothing) above zero, the input is
///   resampled at even spacing along its length and each point is replaced
///   by a Gaussian-weighted average of its neighbours (positions and
///   pressures). The window is centred, so the line doesn't lag behind the
///   pen, and it narrows towards the ends, so the first and last points
///   never move. [`smoothed_points`](Self::smoothed_points) gives the same
///   result mid-stroke, so a live preview matches the finished stroke.
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
    smoothing: f32,
}

impl StrokeBuilder {
    /// Starts a new stroke with the given brush and default settings.
    pub fn begin(style: BrushStyle) -> Self {
        Self {
            style,
            points: Vec::new(),
            pending: None,
            min_distance: DEFAULT_MIN_DISTANCE,
            smoothing: DEFAULT_SMOOTHING,
        }
    }

    /// Sets the minimum distance between kept points, in document units.
    /// Negative or non-finite values are treated as `0.0` (keep all points
    /// except exact duplicates).
    #[must_use]
    pub fn with_min_distance(mut self, min_distance: f32) -> Self {
        self.min_distance = non_negative(min_distance);
        self
    }

    /// Sets the smoothing strength: the Gaussian sigma, in document units.
    /// Wobbles much shorter than this are averaged away. `0.0` (or a
    /// negative or non-finite value) disables smoothing.
    #[must_use]
    pub fn with_smoothing(mut self, sigma: f32) -> Self {
        self.smoothing = non_negative(sigma);
        self
    }

    /// The brush this stroke uses.
    pub fn style(&self) -> &BrushStyle {
        &self.style
    }

    /// Points kept so far (unsmoothed).
    pub fn points(&self) -> &[StrokePoint] {
        &self.points
    }

    /// The stroke as [`finish`](Self::finish) would make it right now. Use
    /// this for the live preview.
    pub fn smoothed_points(&self) -> Vec<StrokePoint> {
        let mut raw = self.points.clone();
        raw.extend(self.pending);
        smooth(&raw, self.smoothing, self.min_distance)
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
    pub fn finish(self) -> Option<Stroke> {
        let points = self.smoothed_points();
        if points.is_empty() {
            return None;
        }
        Some(Stroke::new(points, self.style))
    }
}

fn non_negative(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
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

fn lerp_point(a: StrokePoint, b: StrokePoint, t: f32) -> StrokePoint {
    StrokePoint {
        pos: a.pos.lerp(b.pos, t),
        pressure: a.pressure + (b.pressure - a.pressure) * t,
    }
}

/// Upper bound on resampled points, so a huge stroke at a tiny spacing
/// can't blow up.
const MAX_SAMPLES: f32 = 20_000.0;

/// Gaussian smoothing over arc length with pinned endpoints.
fn smooth(points: &[StrokePoint], sigma: f32, min_spacing: f32) -> Vec<StrokePoint> {
    if points.len() < 3 || sigma <= 0.0 {
        return points.to_vec();
    }
    let length: f32 = points.windows(2).map(|w| w[0].pos.distance(w[1].pos)).sum();
    // A few samples per sigma is enough for a smooth curve.
    let spacing = (sigma / 4.0)
        .max(min_spacing)
        .max(length / MAX_SAMPLES)
        .max(1e-3);
    let samples = resample(points, spacing);
    let n = samples.len();
    if n < 3 {
        return samples;
    }
    let sigma_samples = sigma / spacing;
    let reach = (3.0 * sigma_samples).ceil() as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        // Symmetric window, narrowed near the ends so they stay put.
        let k = reach.min(i).min(n - 1 - i);
        if k == 0 {
            out.push(samples[i]);
            continue;
        }
        let s = sigma_samples.min(k as f32 / 3.0).max(0.5);
        let inv = 1.0 / (2.0 * s * s);
        let (mut pos, mut pressure, mut total) = (Vec2::ZERO, 0.0, 0.0);
        for (j, p) in samples.iter().enumerate().take(i + k + 1).skip(i - k) {
            let d = j as f32 - i as f32;
            let w = (-d * d * inv).exp();
            pos += p.pos * w;
            pressure += p.pressure * w;
            total += w;
        }
        out.push(StrokePoint {
            pos: pos / total,
            pressure: pressure / total,
        });
    }
    out
}

/// Points every `spacing` along the polyline, plus the exact last point.
fn resample(points: &[StrokePoint], spacing: f32) -> Vec<StrokePoint> {
    let mut out = vec![points[0]];
    // Distance travelled since the last emitted sample.
    let mut carried = 0.0;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg = a.pos.distance(b.pos);
        if seg <= 0.0 {
            continue;
        }
        let mut t = spacing - carried;
        while t <= seg {
            out.push(lerp_point(a, b, t / seg));
            t += spacing;
        }
        carried = seg - (t - spacing);
    }
    let last = points[points.len() - 1];
    let n = out.len();
    // Snap a sample that lands (almost) on the end instead of doubling it.
    if n > 1 && out[n - 1].pos.distance(last.pos) < spacing * 0.25 {
        out[n - 1] = last;
    } else {
        out.push(last);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
            .with_smoothing(0.0);
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
            .with_smoothing(0.0);
        b.add_point(p(0.0, 0.0));
        b.add_point(p(4.0, 0.0));
        b.add_point(p(5.0, 0.0)); // too close, but it's where the pen lifted
        let s = b.finish().unwrap();
        assert_eq!(s.points.last().unwrap().pos, Vec2::new(5.0, 0.0));
        assert_eq!(s.points.len(), 3);
    }

    #[test]
    fn ignores_non_finite_and_clamps_pressure() {
        let mut b = StrokeBuilder::begin(BrushStyle::default()).with_smoothing(0.0);
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

    /// Largest distance from the line y = 2 (the zigzag's centre).
    fn max_wobble(pts: &[StrokePoint]) -> f32 {
        pts.iter()
            .map(|p| (p.pos.y - 2.0).abs())
            .fold(0.0, f32::max)
    }

    fn zigzag() -> Vec<StrokePoint> {
        (0..40)
            .map(|i| p(i as f32 * 5.0, if i % 2 == 0 { 0.0 } else { 4.0 }))
            .collect()
    }

    #[test]
    fn smoothing_keeps_endpoints_and_flattens_jitter() {
        let raw = zigzag();
        let mut b = StrokeBuilder::begin(BrushStyle::default()).with_smoothing(8.0);
        for &pt in &raw {
            b.add_point(pt);
        }
        let s = b.finish().unwrap();
        assert_eq!(s.points.first(), raw.first());
        assert_eq!(s.points.last(), raw.last());
        // Away from the pinned ends the zigzag is nearly gone.
        let middle = &s.points[s.points.len() / 4..s.points.len() * 3 / 4];
        assert!(max_wobble(middle) < 0.2, "wobble {}", max_wobble(middle));
    }

    #[test]
    fn stronger_smoothing_is_smoother() {
        let wobble = |sigma: f32| {
            let mut b = StrokeBuilder::begin(BrushStyle::default()).with_smoothing(sigma);
            for pt in zigzag() {
                b.add_point(pt);
            }
            let pts = b.finish().unwrap().points;
            max_wobble(&pts[pts.len() / 4..pts.len() * 3 / 4])
        };
        assert!(wobble(3.0) < 2.0);
        assert!(wobble(6.0) < wobble(3.0));
    }

    #[test]
    fn zero_smoothing_keeps_input() {
        let raw = zigzag();
        let mut b = StrokeBuilder::begin(BrushStyle::default()).with_smoothing(0.0);
        for &pt in &raw {
            b.add_point(pt);
        }
        assert_eq!(b.finish().unwrap().points, raw);
    }

    #[test]
    fn preview_matches_finished_stroke() {
        let mut b = StrokeBuilder::begin(BrushStyle::default())
            .with_min_distance(2.0)
            .with_smoothing(5.0);
        for pt in zigzag() {
            b.add_point(pt);
        }
        b.add_point(p(195.5, 1.0)); // dropped, but still the end point
        let preview = b.smoothed_points();
        assert_eq!(preview.last().unwrap().pos, Vec2::new(195.5, 1.0));
        assert_eq!(b.finish().unwrap().points, preview);
    }

    #[test]
    fn smoothing_straight_line_stays_straight_and_even() {
        let mut b = StrokeBuilder::begin(BrushStyle::default()).with_smoothing(4.0);
        // Uneven input spacing, like a pen speeding up.
        for x in [0.0, 1.0, 2.0, 30.0, 31.0, 100.0] {
            b.add_point(p(x, 0.0));
        }
        let s = b.finish().unwrap();
        assert!(s.points.iter().all(|p| p.pos.y.abs() < 1e-4));
        assert!(s.points.windows(2).all(|w| w[1].pos.x > w[0].pos.x));
        let gaps: Vec<f32> = s
            .points
            .windows(2)
            .map(|w| w[1].pos.x - w[0].pos.x)
            .collect();
        let max_gap = gaps.iter().copied().fold(0.0, f32::max);
        assert!(max_gap < 2.0, "max gap {max_gap}");
    }

    #[test]
    fn smoothing_blends_pressure() {
        let mut b = StrokeBuilder::begin(BrushStyle::default()).with_smoothing(4.0);
        for i in 0..40 {
            b.add_point(pp(i as f32 * 2.0, 0.0, if i % 2 == 0 { 0.2 } else { 0.8 }));
        }
        let s = b.finish().unwrap();
        let mid = s.points[s.points.len() / 2].pressure;
        assert!((mid - 0.5).abs() < 0.05, "pressure {mid}");
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
