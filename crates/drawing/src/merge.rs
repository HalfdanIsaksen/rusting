//! Merging touching strokes and fills of the same colour into one fill
//! (ADR-012).
//!
//! A brush stroke painted over or against a fill of the same colour should
//! become part of that fill, so the two select, move and erase as one shape,
//! like "merge drawing" in other animation tools. [`VectorDrawing::merge_same_colour`]
//! finds every item of the same look connected to a starting item and
//! returns the single fill that replaces them.
//!
//! A new fill also absorbs the existing strokes of its look that touch it,
//! or touch a fill it merges with, so a fill painted against a same-colour
//! outline becomes one shape with it. Strokes never merge with strokes on
//! their own: a stroke joins only through a fill it touches, so a new stroke
//! never pulls in other strokes, and line art in other colours (or not
//! touching a same-colour fill) stays strokes and keeps bounding later
//! bucket fills (which only see strokes, ADR-007).
//!
//! Like the fill bucket and the fill eraser, the union is computed on a
//! temporary raster:
//!
//! 1. Candidates are the start item and the drawing's solid (not gradient)
//!    fills with the same colour, opacity and draw-order tier, plus, when
//!    the start item is a fill, the strokes with that look.
//! 2. Each candidate near the start item is rasterized (cell centres inside
//!    the fill, or inside the stroke's painted shape) on one grid of
//!    [`MERGE_CELL_SIZE`] cells, growing for very large areas so the grid
//!    stays under [`MAX_GRID_CELLS`].
//! 3. Starting from the start item, every fill that overlaps or touches
//!    (within one cell) what is collected so far joins it, and every stroke
//!    candidate that overlaps or touches a joined fill.
//! 4. If more than one item joined, the boundary of their union is traced
//!    and simplified exactly as for a bucket fill.
//!
//! The merged outline is accurate to about one cell, like an erased fill.

use animation_core::{FillId, Rgba, StrokeId, Vec2};

use crate::fill::{Fill, Grid, MAX_GRID_CELLS, trace_contours};
use crate::select::{ItemId, rects_intersect};
use crate::stroke::{Placement, Stroke, VectorDrawing};
use crate::tessellate::collect_nodes;

/// Grid cell size for merging, in document units. Finer than the fill
/// bucket's grid: a merged fill's whole outline is re-traced, including the
/// parts that were exact before.
pub const MERGE_CELL_SIZE: f32 = 0.5;
/// Free cells around the grid, so traced contours never touch its edge.
const PAD_CELLS: f32 = 2.0;
/// Colour components and opacities closer than this count as equal.
const LOOK_EPSILON: f32 = 1e-4;

/// The result of [`VectorDrawing::merge_same_colour`]: one fill replacing
/// several items.
#[derive(Clone, Debug, PartialEq)]
pub struct Merge {
    /// The merged fill. It has the ID, colour, opacity and tier of the
    /// lowest fill (by stored index) among [`Self::fills`], and takes that
    /// fill's place in the drawing.
    pub fill: Fill,
    /// Every fill the merge replaces, in stored order; the first is the one
    /// whose ID [`Self::fill`] keeps.
    pub fills: Vec<FillId>,
    /// The strokes the merge absorbs, in stored order: the start item if it
    /// was a stroke, or the touching strokes of the new fill's look.
    pub strokes: Vec<StrokeId>,
}

impl Merge {
    /// Every item the merge replaces.
    pub fn items(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.fills
            .iter()
            .map(|&id| ItemId::Fill(id))
            .chain(self.strokes.iter().map(|&id| ItemId::Stroke(id)))
    }
}

/// How an item looks for merging: straight colour, opacity, tier.
#[derive(Clone, Copy, Debug)]
struct Look {
    color: Rgba,
    opacity: f32,
    placement: Placement,
}

impl Look {
    fn of_stroke(stroke: &Stroke) -> Self {
        Self {
            color: stroke.style.color,
            opacity: stroke.style.opacity,
            placement: stroke.placement,
        }
    }

    /// `None` for gradient fills, which never merge.
    fn of_fill(fill: &Fill) -> Option<Self> {
        fill.gradient.is_none().then_some(Self {
            color: fill.color,
            opacity: fill.opacity,
            placement: fill.placement,
        })
    }

    /// Same tier, and the same colour once opacity is applied.
    fn matches(&self, other: &Self) -> bool {
        let a = self.color.with_opacity(self.opacity).to_array();
        let b = other.color.with_opacity(other.opacity).to_array();
        self.placement == other.placement
            && a.iter().zip(b).all(|(x, y)| (x - y).abs() <= LOOK_EPSILON)
    }
}

/// A merge candidate: an item and its bounds.
struct Candidate<'a> {
    kind: Kind<'a>,
    bounds: (Vec2, Vec2),
}

enum Kind<'a> {
    Fill(usize, &'a Fill),
    Stroke(usize, &'a Stroke),
}

impl VectorDrawing {
    /// Merges `item` (a stroke or a fill) with every fill of the same colour,
    /// opacity and tier that it overlaps or touches, directly or through
    /// other such fills, into one fill. If `item` is a fill, the strokes of
    /// that look touching any of those fills are merged too. See the
    /// [module docs](crate::merge).
    ///
    /// Returns `None` if nothing merges: `item` is missing, an image or a
    /// gradient fill, or it touches no fill of its look. The drawing is not changed;
    /// apply the [`Merge`] by removing its items and inserting its fill where
    /// its first fill was.
    pub fn merge_same_colour(&self, item: ItemId) -> Option<Merge> {
        let look = match item {
            ItemId::Fill(id) => Look::of_fill(self.fill(id)?)?,
            ItemId::Stroke(id) => Look::of_stroke(self.stroke(id)?),
            ItemId::Image(_) => return None,
        };
        let mut candidates: Vec<Candidate<'_>> = Vec::new();
        let mut start = None;
        for (i, fill) in self.fills().iter().enumerate() {
            if Look::of_fill(fill).is_some_and(|l| l.matches(&look))
                && let Some(bounds) = finite_bounds(fill.bounds())
            {
                if item == ItemId::Fill(fill.id) {
                    start = Some(candidates.len());
                }
                candidates.push(Candidate {
                    kind: Kind::Fill(i, fill),
                    bounds,
                });
            }
        }
        for (i, stroke) in self.strokes().iter().enumerate() {
            let wanted = match item {
                ItemId::Stroke(id) => stroke.id == id,
                // A new fill absorbs the same-look strokes it touches.
                ItemId::Fill(_) => Look::of_stroke(stroke).matches(&look),
                ItemId::Image(_) => false,
            };
            if wanted && let Some(bounds) = finite_bounds(stroke.bounds()) {
                if item == ItemId::Stroke(stroke.id) {
                    start = Some(candidates.len());
                }
                candidates.push(Candidate {
                    kind: Kind::Stroke(i, stroke),
                    bounds,
                });
            }
        }
        let start = start?;

        // Narrow down to candidates whose bounds connect to the start item,
        // so the grid only spans what can possibly merge.
        let near = bounds_component(&candidates, start);
        if near.len() < 2 {
            return None;
        }
        let (lo, hi) = near
            .iter()
            .map(|&i| candidates[i].bounds)
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))?;
        let grid = grid_over(lo, hi);
        let cells: Vec<Vec<usize>> = near
            .iter()
            .map(|&i| match candidates[i].kind {
                Kind::Fill(_, fill) => fill_cells(&grid, &fill.contours),
                Kind::Stroke(_, stroke) => stroke_cells(&grid, stroke),
            })
            .collect();

        // Grow the merged area from the start item. Fills join anything
        // joined; strokes other than the start item only join fills, so
        // strokes never chain through strokes.
        let is_fill = |k: usize| matches!(candidates[near[k]].kind, Kind::Fill(..));
        let mut mask = vec![false; grid.len()];
        let mut reach = vec![false; grid.len()];
        let mut fill_reach = vec![false; grid.len()];
        let mut joined = vec![false; near.len()];
        let first = near.iter().position(|&i| i == start)?;
        if cells[first].is_empty() {
            return None;
        }
        let add = |k: usize, mask: &mut [bool], reach: &mut [bool], fill_reach: &mut [bool]| {
            let fill = is_fill(k);
            for &c in &cells[k] {
                mask[c] = true;
                for n in neighbours8(&grid, c) {
                    reach[n] = true;
                    if fill {
                        fill_reach[n] = true;
                    }
                }
            }
        };
        joined[first] = true;
        add(first, &mut mask, &mut reach, &mut fill_reach);
        loop {
            let mut grew = false;
            for k in 0..near.len() {
                let touch: &[bool] = if is_fill(k) { &reach } else { &fill_reach };
                if !joined[k] && cells[k].iter().any(|&c| touch[c]) {
                    joined[k] = true;
                    add(k, &mut mask, &mut reach, &mut fill_reach);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }

        let mut fills: Vec<(usize, &Fill)> = Vec::new();
        let mut strokes: Vec<(usize, StrokeId)> = Vec::new();
        for (k, &i) in near.iter().enumerate() {
            if !joined[k] {
                continue;
            }
            match candidates[i].kind {
                Kind::Fill(index, fill) => fills.push((index, fill)),
                Kind::Stroke(index, stroke) => strokes.push((index, stroke.id)),
            }
        }
        if fills.is_empty() || fills.len() + strokes.len() < 2 {
            return None;
        }
        fills.sort_by_key(|&(index, _)| index);
        strokes.sort_by_key(|&(index, _)| index);
        let contours = trace_contours(&grid, &mask);
        if contours.is_empty() {
            return None;
        }
        let base = fills[0].1;
        Some(Merge {
            fill: Fill {
                contours,
                ..base.clone()
            },
            fills: fills.iter().map(|(_, f)| f.id).collect(),
            strokes: strokes.iter().map(|&(_, id)| id).collect(),
        })
    }
}

fn finite_bounds(bounds: Option<(Vec2, Vec2)>) -> Option<(Vec2, Vec2)> {
    bounds.filter(|(lo, hi)| lo.is_finite() && hi.is_finite())
}

/// Indices of the candidates connected to `start` through overlapping
/// (slightly padded) bounds, `start` included.
fn bounds_component(candidates: &[Candidate<'_>], start: usize) -> Vec<usize> {
    let pad = Vec2::splat(2.0 * MERGE_CELL_SIZE);
    let grown = |i: usize| (candidates[i].bounds.0 - pad, candidates[i].bounds.1 + pad);
    let mut seen = vec![false; candidates.len()];
    seen[start] = true;
    let mut stack = vec![start];
    let mut out = Vec::new();
    while let Some(i) = stack.pop() {
        out.push(i);
        for j in 0..candidates.len() {
            if !seen[j] && rects_intersect(grown(i), candidates[j].bounds) {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    out
}

/// A grid of [`MERGE_CELL_SIZE`] cells (larger if needed) over `lo..hi` plus
/// a free border.
fn grid_over(lo: Vec2, hi: Vec2) -> Grid {
    let extent = hi - lo;
    let cells = |cell: f32| {
        let size = extent / cell + Vec2::splat(2.0 * PAD_CELLS + 1.0);
        size.x.ceil() as f64 * size.y.ceil() as f64
    };
    let mut cell = MERGE_CELL_SIZE;
    while cells(cell) > MAX_GRID_CELLS as f64 {
        cell *= 1.25;
    }
    let pad = PAD_CELLS * cell;
    let size = ((extent + Vec2::splat(2.0 * pad)) / cell).ceil() + 1.0;
    Grid {
        w: size.x as usize,
        h: size.y as usize,
        origin: lo - Vec2::splat(pad),
        cell,
    }
}

/// The up to eight cells around `i`, and `i` itself.
fn neighbours8(grid: &Grid, i: usize) -> impl Iterator<Item = usize> {
    let (x, y) = ((i % grid.w) as i64, (i / grid.w) as i64);
    let (w, h) = (grid.w as i64, grid.h as i64);
    (-1..=1)
        .flat_map(move |dy| (-1..=1).map(move |dx| (x + dx, y + dy)))
        .filter(move |&(x, y)| x >= 0 && y >= 0 && x < w && y < h)
        .map(move |(x, y)| (y * w + x) as usize)
}

/// Cells whose centre is inside `contours` (even-odd), by scanlines.
fn fill_cells(grid: &Grid, contours: &[Vec<Vec2>]) -> Vec<usize> {
    let mut out = Vec::new();
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
            let to_cell = |x: f32| {
                let c = ((x - grid.origin.x) / grid.cell - 0.5).ceil();
                (c.max(0.0) as usize).min(grid.w)
            };
            let (from, to) = (to_cell(pair[0]), to_cell(pair[1]));
            out.extend((from..to.max(from)).map(|x| y * grid.w + x));
        }
    }
    out
}

/// Cells whose centre lies within the stroke's painted shape: round dabs at
/// its points joined by tapered segments, as [`tessellate`](crate::tessellate)
/// draws it. May list a cell more than once.
fn stroke_cells(grid: &Grid, stroke: &Stroke) -> Vec<usize> {
    let mut out = Vec::new();
    let mut stamp = |p0: Vec2, r0: f32, p1: Vec2, r1: f32| {
        let reach = r0.max(r1);
        let (x0, x1, y0, y1) = grid.span(p0.min(p1) - reach, p0.max(p1) + reach);
        let seg = p1 - p0;
        let len_sq = seg.length_squared();
        for y in y0..y1 {
            for x in x0..x1 {
                let c = grid.center(x, y);
                let t = if len_sq > 0.0 {
                    ((c - p0).dot(seg) / len_sq).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let r = r0 + (r1 - r0) * t;
                if (p0 + seg * t).distance_squared(c) <= r * r {
                    out.push(y * grid.w + x);
                }
            }
        }
    };
    let nodes = collect_nodes(stroke);
    for &(p, r) in &nodes {
        stamp(p, r, p, r);
    }
    for w in nodes.windows(2) {
        let ((p0, r0), (p1, r1)) = (w[0], w[1]);
        stamp(p0, r0, p1, r1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fill::{FillShape, Gradient};
    use crate::stroke::{BrushStyle, StrokePoint};

    const BROWN: Rgba = Rgba {
        r: 0.4,
        g: 0.15,
        b: 0.12,
        a: 1.0,
    };

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    fn square(lo: Vec2, hi: Vec2, color: Rgba) -> Fill {
        Fill::new(
            FillShape {
                contours: vec![vec![lo, v(hi.x, lo.y), hi, v(lo.x, hi.y)]],
            },
            color,
            1.0,
        )
    }

    fn line(a: Vec2, b: Vec2, width: f32, color: Rgba) -> Stroke {
        Stroke::new(
            vec![StrokePoint::new(a), StrokePoint::new(b)],
            BrushStyle {
                color,
                width,
                opacity: 1.0,
            },
        )
    }

    #[test]
    fn stroke_over_same_colour_fill_merges_into_it() {
        let mut d = VectorDrawing::new();
        let fill = square(v(0.0, 0.0), v(100.0, 100.0), BROWN);
        // Pokes 40 units out of the right edge, 10 units wide.
        let stroke = line(v(80.0, 50.0), v(140.0, 50.0), 10.0, BROWN);
        let (fill_id, stroke_id) = (fill.id, stroke.id);
        d.push_fill(fill);
        d.push_stroke(stroke);

        let merge = d.merge_same_colour(ItemId::Stroke(stroke_id)).unwrap();
        assert_eq!(merge.fills, [fill_id]);
        assert_eq!(merge.strokes, [stroke_id]);
        assert_eq!(merge.fill.id, fill_id);
        assert_eq!(merge.fill.color, BROWN);
        // The fill now reaches the stroke's tip, and still covers its body.
        assert!(merge.fill.contains(v(138.0, 50.0)));
        assert!(merge.fill.contains(v(120.0, 46.0)));
        assert!(!merge.fill.contains(v(120.0, 40.0)));
        assert!(merge.fill.contains(v(50.0, 50.0)));
        assert!(merge.fill.contains(v(2.0, 98.0)));
        // Area: square plus the part of the stroke outside it.
        let outside = 40.0 * 10.0 + std::f32::consts::PI * 25.0 / 2.0;
        let area = merge.fill.shape().area();
        assert!((area - (10_000.0 + outside)).abs() < 150.0, "area {area}");
    }

    #[test]
    fn different_colours_do_not_merge() {
        let mut d = VectorDrawing::new();
        d.push_fill(square(v(0.0, 0.0), v(100.0, 100.0), BROWN));
        let stroke = line(v(80.0, 50.0), v(140.0, 50.0), 10.0, Rgba::BLACK);
        let id = stroke.id;
        d.push_stroke(stroke);
        assert_eq!(d.merge_same_colour(ItemId::Stroke(id)), None);
    }

    #[test]
    fn different_opacity_or_tier_does_not_merge() {
        let mut d = VectorDrawing::new();
        d.push_fill(square(v(0.0, 0.0), v(100.0, 100.0), BROWN));
        let mut faint = line(v(80.0, 50.0), v(140.0, 50.0), 10.0, BROWN);
        faint.style.opacity = 0.5;
        let front =
            line(v(80.0, 20.0), v(140.0, 20.0), 10.0, BROWN).with_placement(Placement::Front);
        let (faint_id, front_id) = (faint.id, front.id);
        d.push_stroke(faint);
        d.push_stroke(front);
        assert_eq!(d.merge_same_colour(ItemId::Stroke(faint_id)), None);
        assert_eq!(d.merge_same_colour(ItemId::Stroke(front_id)), None);
    }

    #[test]
    fn separate_items_do_not_merge() {
        let mut d = VectorDrawing::new();
        d.push_fill(square(v(0.0, 0.0), v(100.0, 100.0), BROWN));
        let stroke = line(v(110.0, 50.0), v(140.0, 50.0), 10.0, BROWN);
        let id = stroke.id;
        d.push_stroke(stroke);
        assert_eq!(d.merge_same_colour(ItemId::Stroke(id)), None);
    }

    #[test]
    fn strokes_alone_never_merge() {
        let mut d = VectorDrawing::new();
        let a = line(v(0.0, 0.0), v(100.0, 0.0), 10.0, BROWN);
        let b = line(v(50.0, -20.0), v(50.0, 20.0), 10.0, BROWN);
        let id = b.id;
        d.push_stroke(a);
        d.push_stroke(b);
        assert_eq!(d.merge_same_colour(ItemId::Stroke(id)), None);
    }

    #[test]
    fn a_stroke_bridges_two_fills_into_one() {
        let mut d = VectorDrawing::new();
        let left = square(v(0.0, 0.0), v(50.0, 50.0), BROWN);
        let right = square(v(100.0, 0.0), v(150.0, 50.0), BROWN);
        let other = square(v(300.0, 0.0), v(350.0, 50.0), BROWN);
        let bridge = line(v(40.0, 25.0), v(110.0, 25.0), 8.0, BROWN);
        let (left_id, right_id, bridge_id) = (left.id, right.id, bridge.id);
        d.push_fill(right);
        d.push_fill(left);
        d.push_fill(other);
        d.push_stroke(bridge);

        let merge = d.merge_same_colour(ItemId::Stroke(bridge_id)).unwrap();
        // Stored order, so the lower fill keeps its ID.
        assert_eq!(merge.fills, [right_id, left_id]);
        assert_eq!(merge.fill.id, right_id);
        assert_eq!(merge.strokes, [bridge_id]);
        for p in [v(25.0, 25.0), v(75.0, 25.0), v(125.0, 25.0)] {
            assert!(merge.fill.contains(p), "{p}");
        }
        assert!(!merge.fill.contains(v(75.0, 5.0)));
        assert!(!merge.fill.contains(v(325.0, 25.0)));
    }

    #[test]
    fn a_new_fill_merges_with_fills_and_their_outline() {
        let mut d = VectorDrawing::new();
        // A same-colour outline both fills touch: it becomes part of them.
        let outline = line(v(0.0, 50.0), v(100.0, 50.0), 6.0, BROWN);
        let old = square(v(0.0, 50.0), v(100.0, 100.0), BROWN);
        let new = square(v(0.0, 0.0), v(100.0, 50.0), BROWN);
        let (outline_id, old_id, new_id) = (outline.id, old.id, new.id);
        d.push_stroke(outline);
        d.push_fill(old);
        d.push_fill(new);
        assert_eq!(
            d.merge_same_colour(ItemId::Fill(old_id))
                .unwrap()
                .fills
                .len(),
            2
        );
        let merge = d.merge_same_colour(ItemId::Fill(new_id)).unwrap();
        assert_eq!(merge.fills, [old_id, new_id]);
        assert_eq!(merge.strokes, [outline_id]);
        assert!(merge.fill.contains(v(50.0, 25.0)));
        assert!(merge.fill.contains(v(50.0, 75.0)));
        let area = merge.fill.shape().area();
        assert!((area - 10_000.0).abs() < 100.0, "area {area}");
    }

    #[test]
    fn a_new_fill_absorbs_its_same_colour_outline() {
        let mut d = VectorDrawing::new();
        // The stroke pokes 20 units out past the fill on the right.
        let outline = line(v(0.0, 50.0), v(120.0, 50.0), 6.0, BROWN);
        let fill = square(v(0.0, 0.0), v(100.0, 50.0), BROWN);
        let (outline_id, fill_id) = (outline.id, fill.id);
        d.push_stroke(outline);
        d.push_fill(fill);
        let merge = d.merge_same_colour(ItemId::Fill(fill_id)).unwrap();
        assert_eq!(merge.fills, [fill_id]);
        assert_eq!(merge.strokes, [outline_id]);
        // The fill now covers the whole stroke, not just up to its centre.
        assert!(merge.fill.contains(v(50.0, 52.0)));
        assert!(merge.fill.contains(v(115.0, 50.0)));
        assert!(merge.fill.contains(v(50.0, 25.0)));
        assert!(!merge.fill.contains(v(110.0, 25.0)));
    }

    #[test]
    fn a_new_fill_leaves_other_colours_and_far_strokes_alone() {
        let mut d = VectorDrawing::new();
        let black = line(v(0.0, 50.0), v(100.0, 50.0), 6.0, Rgba::BLACK);
        let far = line(v(200.0, 0.0), v(200.0, 50.0), 6.0, BROWN);
        let fill = square(v(0.0, 0.0), v(100.0, 50.0), BROWN);
        let fill_id = fill.id;
        d.push_stroke(black);
        d.push_stroke(far);
        d.push_fill(fill);
        assert_eq!(d.merge_same_colour(ItemId::Fill(fill_id)), None);
    }

    #[test]
    fn a_new_fill_does_not_chain_through_strokes() {
        let mut d = VectorDrawing::new();
        // `touching` touches the fill; `beyond` only touches `touching`.
        let touching = line(v(100.0, 25.0), v(150.0, 25.0), 6.0, BROWN);
        let beyond = line(v(150.0, 0.0), v(150.0, 50.0), 6.0, BROWN);
        let fill = square(v(0.0, 0.0), v(100.0, 50.0), BROWN);
        let (touching_id, beyond_id, fill_id) = (touching.id, beyond.id, fill.id);
        d.push_stroke(touching);
        d.push_stroke(beyond);
        d.push_fill(fill);
        let merge = d.merge_same_colour(ItemId::Fill(fill_id)).unwrap();
        assert_eq!(merge.strokes, [touching_id]);
        assert!(!merge.items().any(|i| i == ItemId::Stroke(beyond_id)));
    }

    #[test]
    fn a_stroke_does_not_pull_in_other_strokes() {
        let mut d = VectorDrawing::new();
        d.push_fill(square(v(0.0, 0.0), v(100.0, 100.0), BROWN));
        let other = line(v(130.0, 20.0), v(130.0, 80.0), 10.0, BROWN);
        let new = line(v(80.0, 50.0), v(140.0, 50.0), 10.0, BROWN);
        let (other_id, new_id) = (other.id, new.id);
        d.push_stroke(other);
        d.push_stroke(new);
        let merge = d.merge_same_colour(ItemId::Stroke(new_id)).unwrap();
        assert_eq!(merge.strokes, [new_id]);
        assert!(!merge.items().any(|i| i == ItemId::Stroke(other_id)));
    }

    #[test]
    fn gradient_fills_do_not_merge() {
        let mut d = VectorDrawing::new();
        let fill = square(v(0.0, 0.0), v(100.0, 100.0), BROWN).with_gradient(Some(Gradient {
            start: v(0.0, 0.0),
            end: v(100.0, 0.0),
            end_color: Rgba::WHITE,
        }));
        let stroke = line(v(80.0, 50.0), v(140.0, 50.0), 10.0, BROWN);
        let (fill_id, stroke_id) = (fill.id, stroke.id);
        d.push_fill(fill);
        d.push_stroke(stroke);
        assert_eq!(d.merge_same_colour(ItemId::Stroke(stroke_id)), None);
        assert_eq!(d.merge_same_colour(ItemId::Fill(fill_id)), None);
    }

    #[test]
    fn missing_item_is_none() {
        let d = VectorDrawing::new();
        assert_eq!(d.merge_same_colour(ItemId::Fill(FillId::new())), None);
    }
}
