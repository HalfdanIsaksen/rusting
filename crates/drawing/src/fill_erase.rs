//! Partial erasing of fills: cutting the area an eraser passes over out of a
//! [`Fill`] (ADR-007).
//!
//! Like [`Stroke::erase`](crate::Stroke::erase), the eraser is a circle of
//! radius `R` dragged along a polyline, so the erased area is the union of
//! capsules around the path's segments. Fills are areas, so the cut is done
//! with the same raster machinery as the fill bucket ([`fill_region`]):
//!
//! 1. Rasterize the fill (even-odd, cell centres) into a grid over its
//!    bounds. Cells are [`DEFAULT_CELL_SIZE`] units, growing for very large
//!    fills so the grid stays under [`MAX_GRID_CELLS`].
//! 2. Clear every cell whose centre lies within `R` of the eraser path.
//! 3. Split what is left into 4-connected pieces, trace each piece's
//!    boundary (outer contours and holes) and simplify it (Douglas–Peucker),
//!    exactly as the fill bucket does.
//!
//! A fill the eraser does not touch is left alone (its exact contours are
//! kept). A touched fill is rebuilt from the grid, so its whole outline is
//! accurate to about one cell, like a bucket fill. Each remaining piece
//! becomes its own fill with a new ID and the original's colour, opacity
//! and gradient.
//!
//! [`FillEraser`] keeps the grid between calls, so a drag can cut a fill
//! segment by segment without re-rasterizing (and without the outline
//! drifting from repeated tracing). [`erase_fill`] is the one-shot form.
//!
//! [`fill_region`]: crate::fill_region

use animation_core::{FillId, Vec2};
use std::collections::VecDeque;

use crate::fill::{DEFAULT_CELL_SIZE, Fill, Grid, MAX_GRID_CELLS, trace_contours};

/// Free cells around the fill, so traced contours never touch the grid edge.
const PAD_CELLS: f32 = 2.0;

/// What erasing did to a fill. See [`erase_fill`].
#[derive(Clone, Debug, PartialEq)]
pub enum FillEraseResult {
    /// The eraser didn't touch the fill.
    Unchanged,
    /// The eraser covered the whole fill (or left only specks smaller than a
    /// grid cell).
    Removed,
    /// The fill was cut. These fills replace it; there is at least one. Each
    /// has a new ID and the original's colour, opacity and gradient.
    Split(Vec<Fill>),
}

impl FillEraseResult {
    /// The fills that replace the original, or `None` if it is unchanged.
    /// `Some(empty)` means the fill is removed.
    pub fn into_replacements(self) -> Option<Vec<Fill>> {
        match self {
            Self::Unchanged => None,
            Self::Removed => Some(Vec::new()),
            Self::Split(pieces) => Some(pieces),
        }
    }
}

/// Cuts the area an eraser circle of `radius` (document units) covers while
/// dragged along `eraser_path` out of `fill`.
///
/// A one-point path is a single dab. Non-finite path points are skipped; a
/// negative or non-finite radius counts as `0.0`. The fill itself is not
/// changed: the result says what should replace it. See the
/// [module docs](crate::fill_erase) for the method and its precision.
pub fn erase_fill(fill: &Fill, eraser_path: &[Vec2], radius: f32) -> FillEraseResult {
    match FillEraser::new(fill) {
        Some(mut eraser) => {
            eraser.erase(eraser_path, radius);
            eraser.result()
        }
        None => FillEraseResult::Unchanged,
    }
}

/// A fill being erased over several calls (one eraser drag).
///
/// Holds the original fill and its rasterized coverage. Each
/// [`erase`](Self::erase) clears more cells; [`result`](Self::result) traces
/// what is left.
#[derive(Clone, Debug)]
pub struct FillEraser {
    original: Fill,
    grid: Grid,
    mask: Vec<bool>,
    touched: bool,
}

impl FillEraser {
    /// Rasterizes `fill`. `None` if it has no finite, non-empty bounds.
    pub fn new(fill: &Fill) -> Option<Self> {
        let (lo, hi) = fill.bounds()?;
        if !lo.is_finite() || !hi.is_finite() {
            return None;
        }
        let extent = hi - lo;
        let cells = |cell: f32| {
            let size = extent / cell + Vec2::splat(2.0 * PAD_CELLS);
            size.x.ceil() as f64 * size.y.ceil() as f64
        };
        let mut cell = DEFAULT_CELL_SIZE;
        while cells(cell) > MAX_GRID_CELLS as f64 {
            cell *= 1.25;
        }
        let pad = PAD_CELLS * cell;
        let size = ((extent + Vec2::splat(2.0 * pad)) / cell).ceil();
        let grid = Grid {
            w: size.x as usize,
            h: size.y as usize,
            origin: lo - Vec2::splat(pad),
            cell,
        };
        let mask = rasterize_contours(&grid, &fill.contours);
        Some(Self {
            original: fill.clone(),
            grid,
            mask,
            touched: false,
        })
    }

    /// ID of the fill being erased.
    pub fn id(&self) -> FillId {
        self.original.id
    }

    /// The fill being erased, as it was before.
    pub fn original(&self) -> &Fill {
        &self.original
    }

    /// Whether any call so far removed part of the fill.
    pub fn touched(&self) -> bool {
        self.touched
    }

    /// Clears the cells an eraser circle of `radius` covers along
    /// `eraser_path`. Returns whether this call removed anything.
    pub fn erase(&mut self, eraser_path: &[Vec2], radius: f32) -> bool {
        let radius = if radius.is_finite() {
            radius.max(0.0)
        } else {
            0.0
        };
        let points: Vec<Vec2> = eraser_path
            .iter()
            .copied()
            .filter(|p| p.is_finite())
            .collect();
        let segments: Vec<(Vec2, Vec2)> = match points.as_slice() {
            [] => return false,
            [only] => vec![(*only, *only)],
            _ => points.windows(2).map(|w| (w[0], w[1])).collect(),
        };
        let r_sq = radius * radius;
        let mut changed = false;
        for (a, b) in segments {
            let (x0, x1, y0, y1) = self.grid.span(
                a.min(b) - Vec2::splat(radius),
                a.max(b) + Vec2::splat(radius),
            );
            let ab = b - a;
            let len_sq = ab.length_squared();
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = y * self.grid.w + x;
                    if !self.mask[i] {
                        continue;
                    }
                    let c = self.grid.center(x, y);
                    let t = if len_sq > 0.0 {
                        ((c - a).dot(ab) / len_sq).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    if (a + ab * t).distance_squared(c) <= r_sq {
                        self.mask[i] = false;
                        changed = true;
                    }
                }
            }
        }
        self.touched |= changed;
        changed
    }

    /// What the fill becomes after the erasing so far.
    pub fn result(&self) -> FillEraseResult {
        if !self.touched {
            return FillEraseResult::Unchanged;
        }
        let pieces: Vec<Fill> = components(&self.grid, &self.mask)
            .into_iter()
            .filter_map(|(sub, mask)| {
                let contours = trace_contours(&sub, &mask);
                (!contours.is_empty()).then(|| Fill {
                    id: FillId::new(),
                    contours,
                    ..self.original.clone()
                })
            })
            .collect();
        if pieces.is_empty() {
            FillEraseResult::Removed
        } else {
            FillEraseResult::Split(pieces)
        }
    }
}

/// Cells whose centre is inside `contours` (even-odd), by scanlines.
fn rasterize_contours(grid: &Grid, contours: &[Vec<Vec2>]) -> Vec<bool> {
    let mut mask = vec![false; grid.len()];
    let mut xs: Vec<f32> = Vec::new();
    for y in 0..grid.h {
        let cy = grid.origin.y + (y as f32 + 0.5) * grid.cell;
        xs.clear();
        for contour in contours {
            let n = contour.len();
            for i in 0..n {
                let (a, b) = (contour[i], contour[(i + 1) % n]);
                if (a.y > cy) != (b.y > cy) {
                    xs.push(a.x + (cy - a.y) / (b.y - a.y) * (b.x - a.x));
                }
            }
        }
        xs.retain(|x| x.is_finite());
        xs.sort_by(f32::total_cmp);
        for pair in xs.as_chunks::<2>().0 {
            // Cells whose centre x lies in [pair[0], pair[1]).
            let from = ((pair[0] - grid.origin.x) / grid.cell - 0.5).ceil();
            let to = ((pair[1] - grid.origin.x) / grid.cell - 0.5).ceil();
            let from = (from.max(0.0) as usize).min(grid.w);
            let to = (to.max(0.0) as usize).min(grid.w);
            for cell in &mut mask[y * grid.w + from..y * grid.w + to.max(from)] {
                *cell = true;
            }
        }
    }
    mask
}

/// The 4-connected pieces of `mask`, each as its own small grid (its bounds
/// plus one free cell around) and mask, in scan order.
fn components(grid: &Grid, mask: &[bool]) -> Vec<(Grid, Vec<bool>)> {
    let mut seen = vec![false; grid.len()];
    let mut out = Vec::new();
    let mut cells = Vec::new();
    for seed in 0..grid.len() {
        if seen[seed] || !mask[seed] {
            continue;
        }
        cells.clear();
        let mut queue = VecDeque::from([seed]);
        seen[seed] = true;
        let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
        while let Some(i) = queue.pop_front() {
            cells.push(i);
            let (x, y) = (i % grid.w, i / grid.w);
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
            for j in grid.neighbours(i) {
                if !seen[j] && mask[j] {
                    seen[j] = true;
                    queue.push_back(j);
                }
            }
        }
        // Sub-grid with a one-cell free border: local (1, 1) is (x0, y0).
        let sub = Grid {
            w: x1 - x0 + 3,
            h: y1 - y0 + 3,
            origin: grid.origin + (Vec2::new(x0 as f32, y0 as f32) - 1.0) * grid.cell,
            cell: grid.cell,
        };
        let mut sub_mask = vec![false; sub.len()];
        for &i in &cells {
            let (x, y) = (i % grid.w - x0 + 1, i / grid.w - y0 + 1);
            sub_mask[y * sub.w + x] = true;
        }
        out.push((sub, sub_mask));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fill::{FillShape, Gradient, signed_area};
    use animation_core::Rgba;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    fn rect(lo: Vec2, hi: Vec2) -> Vec<Vec2> {
        vec![lo, v(hi.x, lo.y), hi, v(lo.x, hi.y)]
    }

    /// A 100x100 square fill at (100, 100) with a gradient.
    fn square() -> Fill {
        Fill::new(
            FillShape {
                contours: vec![rect(v(100.0, 100.0), v(200.0, 200.0))],
            },
            Rgba::rgb(0.2, 0.4, 0.6),
            0.8,
        )
        .with_gradient(Some(Gradient {
            start: v(100.0, 100.0),
            end: v(200.0, 200.0),
            end_color: Rgba::rgb(1.0, 0.0, 0.0),
        }))
    }

    fn split(result: FillEraseResult) -> Vec<Fill> {
        match result {
            FillEraseResult::Split(pieces) => pieces,
            other => panic!("expected a split, got {other:?}"),
        }
    }

    fn area(fill: &Fill) -> f32 {
        fill.shape().area()
    }

    fn assert_same_look(piece: &Fill, original: &Fill) {
        assert_ne!(piece.id, original.id);
        assert_eq!(piece.color, original.color);
        assert_eq!(piece.opacity, original.opacity);
        assert_eq!(piece.gradient, original.gradient);
    }

    #[test]
    fn a_dab_in_the_middle_punches_a_hole() {
        let fill = square();
        let pieces = split(erase_fill(&fill, &[v(150.0, 150.0)], 10.0));
        assert_eq!(pieces.len(), 1);
        let piece = &pieces[0];
        assert_same_look(piece, &fill);
        // One outer contour and one hole.
        assert_eq!(piece.contours.len(), 2);
        let holes = piece
            .contours
            .iter()
            .filter(|c| signed_area(c) < 0.0)
            .count();
        assert_eq!(holes, 1);
        assert!(!piece.contains(v(150.0, 150.0)));
        assert!(!piece.contains(v(157.0, 150.0)));
        assert!(piece.contains(v(165.0, 150.0)));
        assert!(piece.contains(v(110.0, 110.0)));
        // Area: square minus a disc of radius 10, to about a cell along the
        // outline (the trace cuts the square's corners by half a cell).
        let want = 100.0 * 100.0 - std::f32::consts::PI * 100.0;
        assert!((area(piece) - want).abs() < 200.0, "{}", area(piece));
        let hole: f32 = piece
            .contours
            .iter()
            .map(|c| signed_area(c))
            .filter(|a| *a < 0.0)
            .sum();
        assert!((hole + std::f32::consts::PI * 100.0).abs() < 15.0, "{hole}");
    }

    #[test]
    fn a_stroke_across_splits_the_fill_in_two() {
        let fill = square();
        let pieces = split(erase_fill(&fill, &[v(150.0, 80.0), v(150.0, 220.0)], 5.0));
        assert_eq!(pieces.len(), 2);
        for p in &pieces {
            assert_same_look(p, &fill);
            assert_eq!(p.contours.len(), 1);
            // Each half is 45 wide (100 - 10 erased, halved).
            assert!((area(p) - 45.0 * 100.0).abs() < 150.0, "{}", area(p));
        }
        assert_ne!(pieces[0].id, pieces[1].id);
        let (left, right) = if pieces[0].contains(v(120.0, 150.0)) {
            (&pieces[0], &pieces[1])
        } else {
            (&pieces[1], &pieces[0])
        };
        assert!(left.contains(v(120.0, 150.0)) && !left.contains(v(180.0, 150.0)));
        assert!(right.contains(v(180.0, 150.0)) && !right.contains(v(120.0, 150.0)));
        assert!(!left.contains(v(150.0, 150.0)) && !right.contains(v(150.0, 150.0)));
    }

    #[test]
    fn covering_everything_removes_the_fill() {
        let fill = square();
        assert_eq!(
            erase_fill(&fill, &[v(150.0, 150.0)], 80.0),
            FillEraseResult::Removed
        );
        // A scribble over the whole square.
        let path: Vec<Vec2> = (0..=10)
            .flat_map(|i| {
                let y = 100.0 + i as f32 * 10.0;
                [v(90.0, y), v(210.0, y)]
            })
            .collect();
        assert_eq!(erase_fill(&fill, &path, 6.0), FillEraseResult::Removed);
    }

    #[test]
    fn a_miss_leaves_the_fill_untouched() {
        let fill = square();
        assert_eq!(
            erase_fill(&fill, &[v(300.0, 300.0), v(400.0, 100.0)], 10.0),
            FillEraseResult::Unchanged
        );
        // Inside the bounds of a ring's hole touches nothing.
        let ring = Fill::new(
            FillShape {
                contours: vec![
                    rect(v(0.0, 0.0), v(100.0, 100.0)),
                    rect(v(30.0, 30.0), v(70.0, 70.0))
                        .into_iter()
                        .rev()
                        .collect(),
                ],
            },
            Rgba::rgb(0.0, 0.0, 0.0),
            1.0,
        );
        assert_eq!(
            erase_fill(&ring, &[v(50.0, 50.0)], 10.0),
            FillEraseResult::Unchanged
        );
        assert_eq!(erase_fill(&fill, &[], 10.0), FillEraseResult::Unchanged);
        assert_eq!(
            erase_fill(&fill, &[v(f32::NAN, 150.0)], 10.0),
            FillEraseResult::Unchanged
        );
        // Grazing just outside the edge.
        assert_eq!(
            erase_fill(&fill, &[v(150.0, 89.0)], 10.0),
            FillEraseResult::Unchanged
        );
    }

    #[test]
    fn erasing_in_steps_matches_one_shot() {
        let fill = square();
        let path = [v(90.0, 150.0), v(150.0, 150.0), v(150.0, 210.0)];
        let mut eraser = FillEraser::new(&fill).unwrap();
        assert!(!eraser.touched());
        assert!(eraser.erase(&path[..2], 5.0));
        assert!(eraser.erase(&path[1..], 5.0));
        assert!(!eraser.erase(&path[1..], 5.0), "nothing left to clear");
        let stepped = split(eraser.result());
        let once = split(erase_fill(&fill, &path, 5.0));
        assert_eq!(stepped.len(), 2);
        assert_eq!(stepped.len(), once.len());
        for (a, b) in stepped.iter().zip(&once) {
            assert_eq!(a.contours, b.contours);
        }
        assert_eq!(eraser.id(), fill.id);
    }

    #[test]
    fn degenerate_inputs_never_panic() {
        let fill = square();
        for r in [f32::NAN, -3.0, f32::INFINITY, 0.0] {
            assert_eq!(
                erase_fill(&fill, &[v(150.0, 150.0)], r),
                FillEraseResult::Unchanged
            );
        }
        let empty = Fill::new(FillShape::default(), Rgba::rgb(0.0, 0.0, 0.0), 1.0);
        assert_eq!(
            erase_fill(&empty, &[v(0.0, 0.0)], 5.0),
            FillEraseResult::Unchanged
        );
        assert_eq!(FillEraseResult::Unchanged.into_replacements(), None);
        assert_eq!(FillEraseResult::Removed.into_replacements(), Some(vec![]));
    }

    #[test]
    fn huge_fills_use_coarser_cells() {
        let big = Fill::new(
            FillShape {
                contours: vec![rect(v(0.0, 0.0), v(4000.0, 4000.0))],
            },
            Rgba::rgb(0.0, 0.0, 0.0),
            1.0,
        );
        let eraser = FillEraser::new(&big).unwrap();
        assert!(eraser.grid.len() <= MAX_GRID_CELLS);
        let pieces = split(erase_fill(&big, &[v(2000.0, 2000.0)], 100.0));
        assert_eq!(pieces.len(), 1);
        assert!(!pieces[0].contains(v(2000.0, 2000.0)));
    }

    /// Triangles of `fill`'s mesh covering `p` (edges included).
    fn mesh_hits(mesh: &crate::Mesh, p: Vec2) -> usize {
        mesh.indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| {
                let [a, b, c] = t.map(|i| Vec2::from(mesh.vertices[i as usize]));
                (b - a).perp_dot(p - a) >= 0.0
                    && (c - b).perp_dot(p - b) >= 0.0
                    && (a - c).perp_dot(p - c) >= 0.0
            })
            .count()
    }

    /// Points every `step` along the segments of `path`.
    fn along(path: &[Vec2], step: f32) -> Vec<Vec2> {
        path.windows(2)
            .flat_map(|w| {
                let n = (w[0].distance(w[1]) / step).ceil().max(1.0) as usize;
                (0..=n).map(move |i| w[0].lerp(w[1], i as f32 / n as f32))
            })
            .collect()
    }

    #[test]
    fn a_fast_drag_cuts_one_unbroken_channel() {
        // Widely spaced samples (60 units apart, eraser radius 5): the cut
        // must be the swept capsules, with no fill left between samples, in
        // the traced shape and in the rendered mesh.
        let fill = square();
        let path = [
            v(80.0, 120.0),
            v(140.0, 125.0),
            v(200.0, 160.0),
            v(130.0, 185.0),
            v(110.0, 140.0),
        ];
        let mut eraser = FillEraser::new(&fill).unwrap();
        for pair in path.windows(2) {
            eraser.erase(pair, 5.0);
        }
        let pieces = split(eraser.result());
        let meshes: Vec<_> = pieces.iter().map(crate::tessellate_fill).collect();
        for p in along(&path, 0.5) {
            for perp in [-3.0, 0.0, 3.0] {
                let q = p + Vec2::new(0.0, perp);
                assert!(!pieces.iter().any(|f| f.contains(q)), "fill left at {q}");
                assert!(
                    meshes.iter().all(|m| mesh_hits(m, q) == 0),
                    "mesh bridges the cut at {q}"
                );
            }
        }
        // Next to the cut the fill is still there.
        assert!(pieces.iter().any(|f| f.contains(v(110.0, 110.0))));
        assert!(pieces.iter().any(|f| f.contains(v(190.0, 190.0))));
    }

    #[test]
    fn scribbled_pieces_tessellate_to_exactly_their_shape() {
        // Scribbles in and out of a wavy blob, erased again on the pieces,
        // leave thin necks and notches on the half-cell grid. The mesh must
        // cover each piece once and nothing outside it: overlapping or
        // stray triangles show as fill bridging the erased channels. Equal
        // area and every triangle inside means exactly covered.
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rand = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 40) as f32 / (1u64 << 24) as f32
        };
        let c = v(150.0, 150.0);
        let blob: Vec<Vec2> = (0..160)
            .map(|i| {
                let a = i as f32 / 160.0 * std::f32::consts::TAU;
                c + Vec2::from_angle(a) * (100.0 + 25.0 * (a * 5.0).sin())
            })
            .collect();
        let blob = Fill::new(
            FillShape {
                contours: vec![blob],
            },
            Rgba::BLACK,
            1.0,
        );
        for _ in 0..4 {
            let mut pieces = vec![blob.clone()];
            for _ in 0..3 {
                let mut at = c + (v(rand(), rand()) - 0.5) * 260.0;
                let path: Vec<Vec2> = (0..12)
                    .map(|_| {
                        at += (v(rand(), rand()) - 0.5) * 90.0;
                        at
                    })
                    .collect();
                let radius = 2.0 + rand() * 8.0;
                pieces = pieces
                    .iter()
                    .flat_map(|f| match erase_fill(f, &path, radius) {
                        FillEraseResult::Unchanged => vec![f.clone()],
                        other => other.into_replacements().unwrap_or_default(),
                    })
                    .collect();
            }
            for piece in &pieces {
                let mesh = crate::tessellate_fill(piece);
                let mesh_area: f32 = mesh
                    .indices
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|t| {
                        let [a, b, c] = t.map(|i| Vec2::from(mesh.vertices[i as usize]));
                        (b - a).perp_dot(c - a) * 0.5
                    })
                    .sum();
                let want = area(piece);
                assert!((mesh_area - want).abs() < 0.5, "{mesh_area} vs {want}");
                for t in mesh.indices.as_chunks::<3>().0 {
                    let [a, b, c] = t.map(|i| Vec2::from(mesh.vertices[i as usize]));
                    let centroid = (a + b + c) / 3.0;
                    assert!(piece.contains(centroid), "triangle outside at {centroid}");
                }
            }
        }
    }
}
