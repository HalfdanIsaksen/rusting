//! CPU tessellation of strokes into triangle meshes (ADR-001).
//!
//! A stroke is drawn as the union of:
//! - a filled circle at every point (round caps at the ends, round joins in
//!   between), with radius `style.width * pressure / 2`, and
//! - a quad between each pair of neighbouring circles, bounded by the two
//!   outer tangent lines, so width changes smoothly with pressure.
//!
//! This is simple and robust (no miter maths, so 180° turns and duplicate
//! points cannot produce NaNs), at the cost of **overlapping triangles**. See
//! [`Mesh`] for what that means for the renderer.

use animation_core::Vec2;

use crate::stroke::Stroke;

/// Points closer than this (document units) are merged before tessellation.
const MERGE_EPSILON: f32 = 1e-4;
/// Radii at or below this are treated as zero (nothing drawn).
const RADIUS_EPSILON: f32 = 1e-5;
const MIN_CIRCLE_SEGMENTS: u32 = 8;
const MAX_CIRCLE_SEGMENTS: u32 = 128;

/// A triangle list in document coordinates.
///
/// Plain data, no GPU types: `vertices` can be uploaded as a `Float32x2`
/// vertex buffer and `indices` as a `Uint32` index buffer.
///
/// - Every 3 indices form one triangle. Winding is **not** consistent, so
///   draw with back-face culling disabled.
/// - Triangles of one stroke **overlap**. Colour and opacity are not baked in
///   (they are uniform per stroke, from [`BrushStyle`](crate::BrushStyle)).
///   To avoid darker spots where triangles overlap when `opacity < 1` or
///   `color.a < 1`, the renderer must make each pixel of a stroke count once:
///   e.g. draw the mesh opaque into an offscreen target and composite it with
///   the stroke's opacity, or use a stencil "draw once" pass. With opaque
///   strokes, plain alpha blending is already correct.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// Vertex positions, `[x, y]` in document coordinates.
    pub vertices: Vec<[f32; 2]>,
    /// Triangle list indices into `vertices`.
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Whether the mesh has no triangles.
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Number of triangles.
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Axis-aligned bounds `(min, max)` of all vertices, or `None` if empty.
    pub fn bounds(&self) -> Option<(Vec2, Vec2)> {
        let mut it = self.vertices.iter().map(|&v| Vec2::from(v));
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v))))
    }

    fn push_vertex(&mut self, v: Vec2) -> u32 {
        // A stroke would need billions of points to overflow u32.
        let index = self.vertices.len() as u32;
        self.vertices.push(v.to_array());
        index
    }

    fn push_triangle(&mut self, a: u32, b: u32, c: u32) {
        self.indices.extend_from_slice(&[a, b, c]);
    }
}

/// Tessellation quality settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TessellationOptions {
    /// Maximum distance (document units) between a true circle and its
    /// polygon approximation. Smaller is smoother but uses more triangles.
    /// When zoomed in a lot, the renderer may want to pass a smaller value.
    pub tolerance: f32,
}

impl Default for TessellationOptions {
    fn default() -> Self {
        Self { tolerance: 0.05 }
    }
}

/// Tessellates a stroke with default options. See [`tessellate_with`].
pub fn tessellate(stroke: &Stroke) -> Mesh {
    tessellate_with(stroke, &TessellationOptions::default())
}

/// Tessellates a stroke into a triangle mesh.
///
/// - Width at each point is `style.width * pressure` (pressure clamped to
///   `0.0..=1.0`).
/// - Points with a non-finite position are skipped; consecutive points that
///   (almost) coincide are merged, keeping the larger width.
/// - A one-point stroke becomes a filled circle.
/// - An empty stroke, or one with zero or non-finite width, gives an empty
///   mesh.
pub fn tessellate_with(stroke: &Stroke, options: &TessellationOptions) -> Mesh {
    let mut mesh = Mesh::default();
    let width = stroke.style.width;
    if !width.is_finite() || width <= 0.0 {
        return mesh;
    }
    let tolerance = if options.tolerance.is_finite() && options.tolerance > 0.0 {
        options.tolerance
    } else {
        TessellationOptions::default().tolerance
    };

    let nodes = collect_nodes(stroke);

    for &(pos, radius) in &nodes {
        push_circle(&mut mesh, pos, radius, tolerance);
    }
    for pair in nodes.windows(2) {
        push_segment(&mut mesh, pair[0], pair[1]);
    }
    mesh
}

/// Cleaned-up `(position, radius)` list.
pub(crate) fn collect_nodes(stroke: &Stroke) -> Vec<(Vec2, f32)> {
    let half = stroke.style.width * 0.5;
    let mut nodes: Vec<(Vec2, f32)> = Vec::with_capacity(stroke.points.len());
    for p in &stroke.points {
        if !p.pos.is_finite() {
            continue;
        }
        let pressure = if p.pressure.is_finite() {
            p.pressure.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let radius = (half * pressure).max(0.0);
        match nodes.last_mut() {
            Some(last) if last.0.distance(p.pos) < MERGE_EPSILON => {
                last.1 = last.1.max(radius);
            }
            _ => nodes.push((p.pos, radius)),
        }
    }
    nodes
}

/// Number of polygon sides for a circle, a multiple of 4 so the polygon
/// touches the circle's axis-aligned extremes.
fn circle_segments(radius: f32, tolerance: f32) -> u32 {
    // Chord error for step angle a is r * (1 - cos(a / 2)).
    let c = (1.0 - tolerance / radius).clamp(-1.0, 1.0);
    let half_step = c.acos();
    let n = if half_step > 0.0 {
        (std::f32::consts::PI / half_step).ceil()
    } else {
        MAX_CIRCLE_SEGMENTS as f32
    };
    let n = (n as u32).clamp(MIN_CIRCLE_SEGMENTS, MAX_CIRCLE_SEGMENTS);
    n.div_ceil(4) * 4
}

fn push_circle(mesh: &mut Mesh, center: Vec2, radius: f32, tolerance: f32) {
    if radius <= RADIUS_EPSILON {
        return;
    }
    let n = circle_segments(radius, tolerance);
    let c = mesh.push_vertex(center);
    let first = c + 1;
    for i in 0..n {
        let angle = std::f32::consts::TAU * i as f32 / n as f32;
        mesh.push_vertex(center + Vec2::from_angle(angle) * radius);
    }
    for i in 0..n {
        mesh.push_triangle(c, first + i, first + (i + 1) % n);
    }
}

/// Quad between two circles along their outer tangents.
fn push_segment(mesh: &mut Mesh, (p0, r0): (Vec2, f32), (p1, r1): (Vec2, f32)) {
    let delta = p1 - p0;
    let d = delta.length();
    // If one circle contains the other, the circles already cover the
    // segment and there are no outer tangents.
    if d <= (r0 - r1).abs() + RADIUS_EPSILON || (r0 <= RADIUS_EPSILON && r1 <= RADIUS_EPSILON) {
        return;
    }
    let u = delta / d;
    let n = u.perp();
    let sin = (r0 - r1) / d;
    let cos = (1.0 - sin * sin).max(0.0).sqrt();
    // Unit directions from each centre to its tangent points.
    let w_left = n * cos + u * sin;
    let w_right = -n * cos + u * sin;

    let a0 = mesh.push_vertex(p0 + w_left * r0);
    let b0 = mesh.push_vertex(p0 + w_right * r0);
    let a1 = mesh.push_vertex(p1 + w_left * r1);
    let b1 = mesh.push_vertex(p1 + w_right * r1);
    mesh.push_triangle(a0, a1, b1);
    mesh.push_triangle(a0, b1, b0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{BrushStyle, StrokePoint};

    fn style(width: f32) -> BrushStyle {
        BrushStyle {
            width,
            ..Default::default()
        }
    }

    fn stroke(points: &[(f32, f32, f32)], width: f32) -> Stroke {
        Stroke::new(
            points
                .iter()
                .map(|&(x, y, pressure)| StrokePoint {
                    pos: Vec2::new(x, y),
                    pressure,
                })
                .collect(),
            style(width),
        )
    }

    /// Index validity, no NaNs, non-degenerate counts.
    fn assert_valid(mesh: &Mesh) {
        assert_eq!(mesh.indices.len() % 3, 0, "indices must form triangles");
        let n = mesh.vertices.len() as u32;
        assert!(mesh.indices.iter().all(|&i| i < n), "index out of range");
        assert!(
            mesh.vertices.iter().flatten().all(|c| c.is_finite()),
            "non-finite vertex"
        );
    }

    fn point_in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
        let d1 = (b - a).perp_dot(p - a);
        let d2 = (c - b).perp_dot(p - b);
        let d3 = (a - c).perp_dot(p - c);
        let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(has_neg && has_pos)
    }

    fn covers(mesh: &Mesh, p: Vec2) -> bool {
        mesh.indices.as_chunks::<3>().0.iter().any(|t| {
            let v = |i: u32| Vec2::from(mesh.vertices[i as usize]);
            point_in_triangle(p, v(t[0]), v(t[1]), v(t[2]))
        })
    }

    /// Tiny deterministic PRNG so tests need no extra dependency.
    struct Lcg(u64);
    impl Lcg {
        fn next_f32(&mut self) -> f32 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    #[test]
    fn empty_stroke_gives_empty_mesh() {
        let mesh = tessellate(&stroke(&[], 4.0));
        assert!(mesh.is_empty());
        assert!(mesh.bounds().is_none());
    }

    #[test]
    fn zero_or_bad_width_gives_empty_mesh() {
        for w in [0.0, -3.0, f32::NAN, f32::INFINITY] {
            let mesh = tessellate(&stroke(&[(0.0, 0.0, 1.0), (5.0, 0.0, 1.0)], w));
            assert!(mesh.is_empty(), "width {w}");
        }
        let zero_pressure = tessellate(&stroke(&[(0.0, 0.0, 0.0), (5.0, 0.0, 0.0)], 4.0));
        assert!(zero_pressure.is_empty());
        assert_valid(&zero_pressure);
    }

    #[test]
    fn single_point_is_a_circle() {
        let mesh = tessellate(&stroke(&[(10.0, 20.0, 1.0)], 6.0));
        assert_valid(&mesh);
        assert!(mesh.triangle_count() >= MIN_CIRCLE_SEGMENTS as usize);
        let (min, max) = mesh.bounds().unwrap();
        assert!(min.abs_diff_eq(Vec2::new(7.0, 17.0), 1e-4), "{min}");
        assert!(max.abs_diff_eq(Vec2::new(13.0, 23.0), 1e-4), "{max}");
        assert!(covers(&mesh, Vec2::new(10.0, 20.0)));
        assert!(covers(&mesh, Vec2::new(12.5, 20.0)));
        assert!(!covers(&mesh, Vec2::new(13.5, 20.0)));
    }

    #[test]
    fn straight_line_covers_capsule() {
        let s = stroke(&[(0.0, 0.0, 1.0), (20.0, 0.0, 1.0)], 4.0);
        let mesh = tessellate(&s);
        assert_valid(&mesh);
        for x in [-1.9, 0.0, 5.0, 10.0, 19.0, 21.9] {
            assert!(covers(&mesh, Vec2::new(x, 0.0)), "x={x}");
        }
        assert!(covers(&mesh, Vec2::new(10.0, 1.9)));
        assert!(covers(&mesh, Vec2::new(10.0, -1.9)));
        assert!(!covers(&mesh, Vec2::new(10.0, 2.1)));
        assert!(!covers(&mesh, Vec2::new(-2.1, 0.0)));
        assert!(!covers(&mesh, Vec2::new(22.1, 0.0)));
    }

    #[test]
    fn mesh_bounds_match_stroke_bounds() {
        let s = stroke(
            &[(0.0, 0.0, 1.0), (30.0, 10.0, 1.0), (15.0, 40.0, 1.0)],
            8.0,
        );
        let mesh = tessellate(&s);
        assert_valid(&mesh);
        let (mmin, mmax) = mesh.bounds().unwrap();
        let (smin, smax) = s.bounds().unwrap();
        let tol = TessellationOptions::default().tolerance + 1e-3;
        assert!(mmin.abs_diff_eq(smin, tol), "{mmin} vs {smin}");
        assert!(mmax.abs_diff_eq(smax, tol), "{mmax} vs {smax}");
    }

    #[test]
    fn pressure_scales_width() {
        let s = stroke(&[(0.0, 0.0, 0.5), (20.0, 0.0, 0.5)], 8.0);
        let (min, max) = tessellate(&s).bounds().unwrap();
        assert!((max.y - 2.0).abs() < 1e-3 && (min.y + 2.0).abs() < 1e-3);
        // Tapering stroke: wide at start, thin at the end.
        let taper = tessellate(&stroke(&[(0.0, 0.0, 1.0), (20.0, 0.0, 0.25)], 8.0));
        assert_valid(&taper);
        assert!(covers(&taper, Vec2::new(0.0, 3.9)));
        assert!(!covers(&taper, Vec2::new(20.0, 1.5)));
        assert!(covers(&taper, Vec2::new(10.0, 2.0)));
    }

    #[test]
    fn duplicate_points_and_reversals_are_finite() {
        let cases: &[&[(f32, f32, f32)]] = &[
            &[(0.0, 0.0, 1.0), (0.0, 0.0, 1.0)],
            &[
                (0.0, 0.0, 1.0),
                (0.0, 0.0, 1.0),
                (5.0, 0.0, 1.0),
                (5.0, 0.0, 1.0),
            ],
            // 180° turn.
            &[(0.0, 0.0, 1.0), (10.0, 0.0, 1.0), (0.0, 0.0, 1.0)],
            // Repeated back-and-forth with tiny steps.
            &[
                (0.0, 0.0, 1.0),
                (1e-6, 0.0, 1.0),
                (0.0, 0.0, 1.0),
                (1e-6, 0.0, 1.0),
            ],
            // Big radius change over a tiny distance (one circle inside the other).
            &[(0.0, 0.0, 1.0), (0.1, 0.0, 0.1)],
            &[(0.0, 0.0, 0.0), (5.0, 0.0, 1.0), (10.0, 0.0, 0.0)],
            &[(0.0, 0.0, f32::NAN), (f32::NAN, 1.0, 1.0), (5.0, 5.0, 1.0)],
        ];
        for pts in cases {
            let mesh = tessellate(&stroke(pts, 4.0));
            assert_valid(&mesh);
            assert!(!mesh.is_empty(), "{pts:?}");
        }
        let reversal = tessellate(&stroke(
            &[(0.0, 0.0, 1.0), (10.0, 0.0, 1.0), (0.0, 0.0, 1.0)],
            4.0,
        ));
        assert!(covers(&reversal, Vec2::new(11.9, 0.0)));
        assert!(!covers(&reversal, Vec2::new(12.1, 0.0)));
    }

    #[test]
    fn duplicate_points_are_merged() {
        let once = tessellate(&stroke(&[(0.0, 0.0, 1.0), (5.0, 0.0, 1.0)], 4.0));
        let dup = tessellate(&stroke(
            &[
                (0.0, 0.0, 1.0),
                (0.0, 0.0, 1.0),
                (5.0, 0.0, 1.0),
                (5.0, 0.0, 1.0),
            ],
            4.0,
        ));
        assert_eq!(once, dup);
    }

    #[test]
    fn random_strokes_are_valid() {
        let mut rng = Lcg(42);
        for _ in 0..200 {
            let len = (rng.next_f32() * 40.0) as usize;
            let pts: Vec<(f32, f32, f32)> = (0..len)
                .map(|_| (rng.next_f32() * 50.0, rng.next_f32() * 50.0, rng.next_f32()))
                .collect();
            let s = stroke(&pts, rng.next_f32() * 20.0 + 0.01);
            let mesh = tessellate(&s);
            assert_valid(&mesh);
            if let (Some((mmin, mmax)), Some((smin, smax))) = (mesh.bounds(), s.bounds()) {
                // Pressure <= 1, so the mesh never exceeds the stroke bounds.
                assert!(mmin.cmpge(smin - 1e-3).all() && mmax.cmple(smax + 1e-3).all());
            }
        }
    }

    #[test]
    fn tolerance_controls_detail() {
        let s = stroke(&[(0.0, 0.0, 1.0)], 100.0);
        let coarse = tessellate_with(&s, &TessellationOptions { tolerance: 5.0 });
        let fine = tessellate_with(&s, &TessellationOptions { tolerance: 0.01 });
        assert!(coarse.triangle_count() < fine.triangle_count());
        assert!(fine.triangle_count() <= MAX_CIRCLE_SEGMENTS as usize);
        let bad = tessellate_with(
            &s,
            &TessellationOptions {
                tolerance: f32::NAN,
            },
        );
        assert_valid(&bad);
        assert_eq!(bad, tessellate(&s));
        for n in [circle_segments(0.01, 0.05), circle_segments(1e6, 1e-6)] {
            assert_eq!(n % 4, 0);
            assert!((MIN_CIRCLE_SEGMENTS..=MAX_CIRCLE_SEGMENTS).contains(&n));
        }
    }
}
