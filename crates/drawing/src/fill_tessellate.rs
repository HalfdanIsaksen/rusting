//! Triangulating fills: polygons with holes into triangles (ADR-007).
//!
//! Holes are joined to their outer contour by a zero-width "bridge" to the
//! nearest visible vertex, and the resulting single polygon is cut into
//! triangles by ear clipping. Fill contours come out of `fill_region`
//! already simplified (tens to a few hundred points), so the simple
//! quadratic algorithm is fast enough and needs no dependency.

use animation_core::Vec2;

use crate::fill::{Fill, signed_area};
use crate::tessellate::Mesh;

/// Points closer than this (document units) are merged.
const MERGE_EPSILON: f32 = 1e-4;
/// Contours with less area than this are ignored.
const AREA_EPSILON: f32 = 1e-6;

/// Triangulates a fill. Unlike stroke meshes, the triangles do **not**
/// overlap, so plain alpha blending gives the right result at any opacity.
///
/// Outer contours have positive signed area and holes negative (see
/// [`FillShape`](crate::FillShape)); if every contour is reversed the fill
/// is read the other way round. Holes that lie in no outer contour, and
/// degenerate contours, are ignored.
pub fn tessellate_fill(fill: &Fill) -> Mesh {
    let mut contours: Vec<Vec<Vec2>> = fill
        .contours
        .iter()
        .map(|c| clean(c))
        .filter(|c| c.len() >= 3 && signed_area(c).abs() > AREA_EPSILON)
        .collect();
    if contours.iter().all(|c| signed_area(c) < 0.0) {
        contours.iter_mut().for_each(|c| c.reverse());
    }
    let (outers, holes): (Vec<_>, Vec<_>) =
        contours.into_iter().partition(|c| signed_area(c) > 0.0);

    // Each hole belongs to the smallest outer contour containing it.
    let mut groups: Vec<(Vec<Vec2>, Vec<Vec<Vec2>>)> =
        outers.into_iter().map(|o| (o, Vec::new())).collect();
    for hole in holes {
        let owner = groups
            .iter()
            .enumerate()
            .filter(|(_, (outer, _))| point_in_polygon(outer, hole[0]))
            .min_by(|a, b| signed_area(&a.1.0).total_cmp(&signed_area(&b.1.0)))
            .map(|(i, _)| i);
        if let Some(i) = owner {
            groups[i].1.push(hole);
        }
    }

    let mut mesh = Mesh::default();
    for (outer, holes) in groups {
        let polygon = bridge_holes(outer, holes);
        ear_clip(&polygon, &mut mesh);
    }
    mesh
}

/// Drops non-finite points and (near-)duplicate neighbours, including a
/// repeated first point at the end.
fn clean(contour: &[Vec2]) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(contour.len());
    for &p in contour.iter().filter(|p| p.is_finite()) {
        if out.last().is_none_or(|q| q.distance(p) > MERGE_EPSILON) {
            out.push(p);
        }
    }
    while out.len() > 1 && out[0].distance(out[out.len() - 1]) <= MERGE_EPSILON {
        out.pop();
    }
    out
}

fn point_in_polygon(polygon: &[Vec2], p: Vec2) -> bool {
    let n = polygon.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (polygon[i], polygon[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

/// Whether segments `a0-a1` and `b0-b1` cross at a point that is not a
/// shared endpoint.
fn segments_cross(a0: Vec2, a1: Vec2, b0: Vec2, b1: Vec2) -> bool {
    let shared = |p: Vec2| p.distance(b0) <= MERGE_EPSILON || p.distance(b1) <= MERGE_EPSILON;
    if shared(a0) || shared(a1) {
        return false;
    }
    let d1 = (a1 - a0).perp_dot(b0 - a0);
    let d2 = (a1 - a0).perp_dot(b1 - a0);
    let d3 = (b1 - b0).perp_dot(a0 - b0);
    let d4 = (b1 - b0).perp_dot(a1 - b0);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    // Collinear overlap.
    let on = |p: Vec2, q0: Vec2, q1: Vec2| {
        (q1 - q0).perp_dot(p - q0).abs() <= 1e-6
            && p.cmpge(q0.min(q1)).all()
            && p.cmple(q0.max(q1)).all()
    };
    (d1 == 0.0 && on(b0, a0, a1))
        || (d2 == 0.0 && on(b1, a0, a1))
        || (d3 == 0.0 && on(a0, b0, b1))
        || (d4 == 0.0 && on(a1, b0, b1))
}

fn crosses_any(a: Vec2, b: Vec2, loops: &[&[Vec2]]) -> bool {
    loops.iter().any(|ring| {
        let n = ring.len();
        (0..n).any(|i| segments_cross(a, b, ring[i], ring[(i + 1) % n]))
    })
}

/// Joins every hole into `outer` with a two-way bridge edge, giving one
/// simple (weakly) polygon with positive orientation.
fn bridge_holes(outer: Vec<Vec2>, mut holes: Vec<Vec<Vec2>>) -> Vec<Vec2> {
    let max_x = |h: &Vec<Vec2>| h.iter().map(|p| p.x).fold(f32::MIN, f32::max);
    // Rightmost holes first, so bridges to the right don't cross later holes.
    holes.sort_by(|a, b| max_x(b).total_cmp(&max_x(a)));
    let mut polygon = outer;
    for (k, hole) in holes.iter().enumerate() {
        let m = (0..hole.len())
            .max_by(|&a, &b| hole[a].x.total_cmp(&hole[b].x))
            .unwrap_or(0);
        let from = hole[m];
        let mut candidates: Vec<usize> = (0..polygon.len()).collect();
        candidates.sort_by(|&a, &b| {
            from.distance_squared(polygon[a])
                .total_cmp(&from.distance_squared(polygon[b]))
        });
        let others: Vec<&[Vec2]> = holes[k..].iter().map(Vec::as_slice).collect();
        let visible = candidates.iter().copied().find(|&j| {
            let to = polygon[j];
            !crosses_any(from, to, &[&polygon]) && !crosses_any(from, to, &others)
        });
        let Some(j) = visible.or(candidates.first().copied()) else {
            continue;
        };
        let mut merged = Vec::with_capacity(polygon.len() + hole.len() + 2);
        merged.extend_from_slice(&polygon[..=j]);
        merged.extend_from_slice(&hole[m..]);
        merged.extend_from_slice(&hole[..=m]);
        merged.extend_from_slice(&polygon[j..]);
        polygon = merged;
    }
    polygon
}

fn strictly_inside_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    (b - a).perp_dot(p - a) > 0.0 && (c - b).perp_dot(p - b) > 0.0 && (a - c).perp_dot(p - c) > 0.0
}

fn inside_or_on_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    (b - a).perp_dot(p - a) >= 0.0
        && (c - b).perp_dot(p - b) >= 0.0
        && (a - c).perp_dot(p - c) >= 0.0
}

/// Ear clipping of a positively oriented polygon (bridges allowed). Appends
/// the polygon's points as vertices and its triangles to `mesh`.
fn ear_clip(polygon: &[Vec2], mesh: &mut Mesh) {
    if polygon.len() < 3 {
        return;
    }
    // Polygons have at most a few thousand points, far below u32::MAX.
    let base = mesh.vertices.len() as u32;
    mesh.vertices.extend(polygon.iter().map(|p| p.to_array()));
    let mut ring: Vec<usize> = (0..polygon.len()).collect();
    let mut i = 0usize;
    let mut misses = 0usize;
    while ring.len() > 3 {
        let k = ring.len();
        i %= k;
        let (ia, ib, ic) = (ring[(i + k - 1) % k], ring[i], ring[(i + 1) % k]);
        let (a, b, c) = (polygon[ia], polygon[ib], polygon[ic]);
        let cross = (b - a).perp_dot(c - b);
        let degenerate = cross.abs() <= f32::EPSILON * (b - a).length() * (c - b).length()
            || a.distance(b) <= MERGE_EPSILON
            || b.distance(c) <= MERGE_EPSILON;
        // A vertex on the triangle's edge blocks the ear too when the
        // boundary turns into the triangle there (a reflex or straight
        // vertex): traced fill outlines lie on a half-cell grid, so such
        // touching vertices are common, and clipping past them makes
        // triangles that overlap or reach across erased channels.
        let is_ear = !degenerate
            && cross > 0.0
            && !ring.iter().enumerate().any(|(n, &j)| {
                let p = polygon[j];
                if j == ia || j == ib || j == ic || p == a || p == b || p == c {
                    return false;
                }
                if strictly_inside_triangle(p, a, b, c) {
                    return true;
                }
                let prev = polygon[ring[(n + k - 1) % k]];
                let next = polygon[ring[(n + 1) % k]];
                (p - prev).perp_dot(next - p) <= 0.0 && inside_or_on_triangle(p, a, b, c)
            });
        if is_ear || (degenerate && !is_spike(a, b, c)) || misses >= k {
            // A forced removal (no ear found: bad input) may be reflex; it
            // then adds no triangle rather than a flipped one.
            if !degenerate && cross > 0.0 {
                mesh.indices
                    .extend([base + ia as u32, base + ib as u32, base + ic as u32]);
            }
            ring.remove(i);
            misses = 0;
            // Re-check the previous vertex, whose neighbour changed.
            i = (i + ring.len() - 1) % ring.len();
        } else {
            i += 1;
            misses += 1;
        }
    }
    let (a, b, c) = (ring[0], ring[1], ring[2]);
    if (polygon[b] - polygon[a]).perp_dot(polygon[c] - polygon[b]) > 0.0 {
        mesh.indices
            .extend([base + a as u32, base + b as u32, base + c as u32]);
    }
}

/// Whether `b` turns back on itself (a zero-width spike, as at a hole
/// bridge), which must not be removed before the hole is clipped.
fn is_spike(a: Vec2, b: Vec2, c: Vec2) -> bool {
    (b - a).dot(c - b) < 0.0 && a.distance(b) > MERGE_EPSILON && b.distance(c) > MERGE_EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fill::FillShape;
    use animation_core::Rgba;

    fn fill(contours: Vec<Vec<Vec2>>) -> Fill {
        Fill::new(FillShape { contours }, Rgba::BLACK, 1.0)
    }

    fn rect(lo: Vec2, hi: Vec2) -> Vec<Vec2> {
        // Positive (clockwise on screen, y down).
        vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)]
    }

    fn mesh_area(mesh: &Mesh) -> f32 {
        mesh.indices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| {
                let v = |i: u32| Vec2::from(mesh.vertices[i as usize]);
                (v(t[1]) - v(t[0])).perp_dot(v(t[2]) - v(t[0])) * 0.5
            })
            .sum()
    }

    fn covers(mesh: &Mesh, p: Vec2) -> bool {
        mesh.indices.as_chunks::<3>().0.iter().any(|t| {
            let v = |i: u32| Vec2::from(mesh.vertices[i as usize]);
            let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
            (b - a).perp_dot(p - a) >= 0.0
                && (c - b).perp_dot(p - b) >= 0.0
                && (a - c).perp_dot(p - c) >= 0.0
        })
    }

    fn assert_valid(mesh: &Mesh) {
        assert_eq!(mesh.indices.len() % 3, 0);
        let n = mesh.vertices.len() as u32;
        assert!(mesh.indices.iter().all(|&i| i < n));
        // No overlaps and no flipped triangles.
        for t in mesh.indices.as_chunks::<3>().0 {
            let v = |i: u32| Vec2::from(mesh.vertices[i as usize]);
            assert!((v(t[1]) - v(t[0])).perp_dot(v(t[2]) - v(t[0])) > 0.0);
        }
    }

    #[test]
    fn square_is_two_triangles() {
        let mesh = tessellate_fill(&fill(vec![rect(Vec2::ZERO, Vec2::splat(10.0))]));
        assert_valid(&mesh);
        assert_eq!(mesh.triangle_count(), 2);
        assert!((mesh_area(&mesh) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn reversed_contours_still_fill() {
        let mut square = rect(Vec2::ZERO, Vec2::splat(10.0));
        square.reverse();
        let mesh = tessellate_fill(&fill(vec![square]));
        assert!((mesh_area(&mesh) - 100.0).abs() < 1e-3);
    }

    #[test]
    fn concave_polygon_keeps_its_notch() {
        // A "U" shape.
        let u = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(3.0, 0.0),
            Vec2::new(3.0, 7.0),
            Vec2::new(7.0, 7.0),
            Vec2::new(7.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(10.0, 10.0),
            Vec2::new(0.0, 10.0),
        ];
        let mesh = tessellate_fill(&fill(vec![u]));
        assert_valid(&mesh);
        assert!((mesh_area(&mesh) - 72.0).abs() < 1e-3);
        assert!(!covers(&mesh, Vec2::new(5.0, 3.0)));
        assert!(covers(&mesh, Vec2::new(5.0, 9.0)));
    }

    #[test]
    fn holes_are_cut_out() {
        let mut hole_a = rect(Vec2::splat(2.0), Vec2::splat(4.0));
        let mut hole_b = rect(Vec2::new(6.0, 5.0), Vec2::new(8.0, 8.0));
        hole_a.reverse();
        hole_b.reverse();
        let mesh = tessellate_fill(&fill(vec![
            rect(Vec2::ZERO, Vec2::splat(10.0)),
            hole_a,
            hole_b,
        ]));
        assert_valid(&mesh);
        assert!((mesh_area(&mesh) - (100.0 - 4.0 - 6.0)).abs() < 1e-3);
        assert!(!covers(&mesh, Vec2::splat(3.0)));
        assert!(!covers(&mesh, Vec2::new(7.0, 6.5)));
        assert!(covers(&mesh, Vec2::new(5.0, 1.0)));
    }

    #[test]
    fn ring_from_fill_region_tessellates_to_its_area() {
        use crate::fill::{GapClosing, fill_region};
        use crate::stroke::{BrushStyle, Stroke, StrokePoint, VectorDrawing};
        let c = Vec2::splat(100.0);
        let circle = |r: f32| {
            Stroke::new(
                (0..=48)
                    .map(|i| {
                        StrokePoint::new(
                            c + Vec2::from_angle(i as f32 / 48.0 * std::f32::consts::TAU) * r,
                        )
                    })
                    .collect(),
                BrushStyle::default(),
            )
        };
        let mut d = VectorDrawing::new();
        d.push_stroke(circle(60.0));
        d.push_stroke(circle(25.0));
        let shape = fill_region(
            &d,
            c + Vec2::new(40.0, 0.0),
            GapClosing::None,
            Vec2::splat(200.0),
        )
        .unwrap();
        let expected = shape.area();
        let mesh = tessellate_fill(&Fill::new(shape, Rgba::BLACK, 1.0));
        assert_valid(&mesh);
        assert!((mesh_area(&mesh) - expected).abs() < expected * 1e-3);
        assert!(!covers(&mesh, c));
        assert!(covers(&mesh, c + Vec2::new(0.0, 40.0)));
    }

    #[test]
    fn junk_gives_an_empty_mesh() {
        let mesh = tessellate_fill(&fill(vec![
            vec![],
            vec![Vec2::ZERO, Vec2::ONE],
            vec![Vec2::ZERO, Vec2::ONE, Vec2::splat(2.0)],
            vec![Vec2::NAN, Vec2::ONE, Vec2::X],
        ]));
        assert!(mesh.is_empty());
    }
}
