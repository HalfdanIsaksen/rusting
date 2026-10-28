//! Fills and the fill bucket's region finder (ADR-007).
//!
//! A [`Fill`] is a filled area stored as vector contours in document
//! coordinates. It belongs to a [`VectorDrawing`] and is drawn under all of
//! that drawing's strokes.
//!
//! [`fill_region`] finds the area enclosed by strokes around a click. It
//! works on a temporary raster (a coverage grid), but only the traced vector
//! contours are kept:
//!
//! 1. Rasterize the strokes into an "ink" grid over their padded bounds.
//! 2. Close gaps: every cell within the gap radius of ink also blocks the
//!    flood ([`GapClosing`]).
//! 3. Flood fill from the click. Reaching the edge of the grid means the area
//!    is not enclosed.
//! 4. Grow the result back by the gap radius, and further under the lines
//!    (ink cells only) so there is no halo between fill and line.
//! 5. Trace the boundary (marching squares) and simplify (Douglas–Peucker).

use animation_core::{FillId, Rgba, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;

use crate::stroke::{Stroke, VectorDrawing};
use crate::tessellate::collect_nodes;

/// Gap radius for [`GapClosing::Small`], in document units. Gaps up to about
/// twice this wide are closed.
pub const SMALL_GAP_RADIUS: f32 = 4.0;
/// Gap radius for [`GapClosing::Medium`], in document units.
pub const MEDIUM_GAP_RADIUS: f32 = 10.0;
/// Grid cell size in document units for normal-sized drawings.
pub const DEFAULT_CELL_SIZE: f32 = 1.0;
/// Upper bound on grid cells. Larger drawings use bigger cells.
pub const MAX_GRID_CELLS: usize = 1_500_000;
/// Douglas–Peucker tolerance, in cells.
const SIMPLIFY_TOLERANCE_CELLS: f32 = 0.6;
/// Ink is rasterized slightly wider than it is so even hairlines form a
/// closed barrier on the grid (in cells, a bit over `sqrt(2) / 2`).
const INK_INFLATE_CELLS: f32 = 0.75;
/// Contours smaller than this (in square cells) are dropped as noise.
const MIN_CONTOUR_AREA_CELLS: f32 = 1.0;
/// Largest stroke half-width the fill tucks under, in document units.
const MAX_TUCK: f32 = 100.0;
/// Large but finite "no feature" distance for the distance transform.
const FAR: f64 = 1e20;

/// How hard the fill bucket tries to close gaps in the line art.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GapClosing {
    /// Only fully closed areas fill.
    #[default]
    None,
    /// Closes gaps up to about `2 * SMALL_GAP_RADIUS` wide.
    Small,
    /// Closes gaps up to about `2 * MEDIUM_GAP_RADIUS` wide.
    Medium,
}

impl GapClosing {
    /// Every setting, smallest first.
    pub const ALL: [Self; 3] = [Self::None, Self::Small, Self::Medium];

    /// The dilation radius in document units.
    pub const fn radius(self) -> f32 {
        match self {
            Self::None => 0.0,
            Self::Small => SMALL_GAP_RADIUS,
            Self::Medium => MEDIUM_GAP_RADIUS,
        }
    }

    /// User-visible name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "No gaps",
            Self::Small => "Small gaps",
            Self::Medium => "Medium gaps",
        }
    }
}

/// The geometry of a filled area: closed contours in document coordinates.
///
/// Contours are implicitly closed (the last point connects to the first).
/// Outer contours have a positive signed area in document coordinates
/// (y down; clockwise on screen) and holes a negative one; [`Self::contains`]
/// uses the even-odd rule, so either reading gives the same area.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FillShape {
    /// Outer boundaries and holes.
    pub contours: Vec<Vec<Vec2>>,
}

impl FillShape {
    /// Total signed area (outer contours minus holes).
    pub fn area(&self) -> f32 {
        self.contours.iter().map(|c| signed_area(c)).sum()
    }

    /// Axis-aligned bounds `(min, max)` of all contour points.
    pub fn bounds(&self) -> Option<(Vec2, Vec2)> {
        let mut it = self.contours.iter().flatten().copied();
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }

    /// Whether `point` is inside the shape (even-odd rule).
    pub fn contains(&self, point: Vec2) -> bool {
        contains_even_odd(&self.contours, point)
    }
}

/// Even-odd point-in-polygon test over several closed contours.
fn contains_even_odd(contours: &[Vec<Vec2>], point: Vec2) -> bool {
    let mut inside = false;
    for contour in contours {
        let n = contour.len();
        for i in 0..n {
            let (a, b) = (contour[i], contour[(i + 1) % n]);
            if (a.y > point.y) != (b.y > point.y) {
                let x = a.x + (point.y - a.y) / (b.y - a.y) * (b.x - a.x);
                if point.x < x {
                    inside = !inside;
                }
            }
        }
    }
    inside
}

/// A filled area of a drawing, drawn under the drawing's strokes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fill {
    /// Stable identifier.
    pub id: FillId,
    /// Outer boundaries and holes, in document coordinates (see [`FillShape`]).
    pub contours: Vec<Vec<Vec2>>,
    /// Fill colour.
    pub color: Rgba,
    /// Opacity multiplier in `0.0..=1.0`.
    pub opacity: f32,
}

impl Fill {
    /// A fill with a fresh ID.
    pub fn new(shape: FillShape, color: Rgba, opacity: f32) -> Self {
        Self {
            id: FillId::new(),
            contours: shape.contours,
            color,
            opacity,
        }
    }

    /// The fill's geometry.
    pub fn shape(&self) -> FillShape {
        FillShape {
            contours: self.contours.clone(),
        }
    }

    /// Axis-aligned bounds of the contours.
    pub fn bounds(&self) -> Option<(Vec2, Vec2)> {
        let mut it = self.contours.iter().flatten().copied();
        let first = it.next()?;
        Some(it.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }

    /// Whether `point` is inside the fill (even-odd rule).
    pub fn contains(&self, point: Vec2) -> bool {
        contains_even_odd(&self.contours, point)
    }
}

/// Why [`fill_region`] found nothing to fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillError {
    /// The click is outside the canvas.
    OutsideCanvas,
    /// The drawing has no strokes to bound an area.
    NothingToFill,
    /// The click is on a line.
    OnLine,
    /// The area around the click is not enclosed: it leaks out past the line
    /// art. A larger gap setting may close it.
    NotClosed,
}

impl fmt::Display for FillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OutsideCanvas => "click inside the canvas to fill",
            Self::NothingToFill => "nothing to fill — draw a closed shape first",
            Self::OnLine => "click inside an area, not on a line",
            Self::NotClosed => "area not closed — try a larger gap setting",
        })
    }
}

impl std::error::Error for FillError {}

/// Finds the area enclosed by `drawing`'s strokes around `point` (document
/// coordinates), closing gaps up to the size `gap` allows.
///
/// `canvas_size` is the document size; clicks outside `0..canvas_size` are
/// rejected. The result's contours tuck under the surrounding lines by about
/// half a line width, so fill and line meet with no gap.
pub fn fill_region(
    drawing: &VectorDrawing,
    point: Vec2,
    gap: GapClosing,
    canvas_size: Vec2,
) -> Result<FillShape, FillError> {
    if !point.is_finite()
        || point.x < 0.0
        || point.y < 0.0
        || point.x > canvas_size.x
        || point.y > canvas_size.y
    {
        return Err(FillError::OutsideCanvas);
    }
    let strokes: Vec<&Stroke> = drawing
        .strokes()
        .iter()
        .filter(|s| s.style.width.is_finite() && s.style.width > 0.0)
        .collect();
    let Some((lo, hi)) = strokes
        .iter()
        .filter_map(|s| s.bounds())
        .filter(|(lo, hi)| lo.is_finite() && hi.is_finite())
        .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)))
    else {
        return Err(FillError::NothingToFill);
    };
    if point.cmplt(lo).any() || point.cmpgt(hi).any() {
        return Err(FillError::NotClosed);
    }

    let gap_radius = gap.radius();
    let tuck = strokes
        .iter()
        .map(|s| s.style.width * 0.5)
        .fold(0.0f32, f32::max)
        .min(MAX_TUCK);

    // Grid over the stroke bounds plus room for growing and a free border.
    let extent = hi - lo;
    let mut cell = DEFAULT_CELL_SIZE;
    let estimate = |cell: f32| {
        let pad = gap_radius + tuck + 3.0 * cell;
        let size = (extent + Vec2::splat(2.0 * pad)) / cell;
        size.x.ceil() as f64 * size.y.ceil() as f64
    };
    while estimate(cell) > MAX_GRID_CELLS as f64 {
        cell *= 1.25;
    }
    let pad = gap_radius + tuck + 3.0 * cell;
    let origin = lo - Vec2::splat(pad);
    let size = ((extent + Vec2::splat(2.0 * pad)) / cell).ceil();
    let grid = Grid {
        w: size.x as usize,
        h: size.y as usize,
        origin,
        cell,
    };

    let ink = rasterize_ink(&grid, &strokes);
    let gap_cells = gap_radius / cell;
    let blocked: Vec<bool> = if gap_cells > 0.0 {
        let dist = distance_squared(&grid, &ink);
        let limit = f64::from(gap_cells * gap_cells);
        dist.iter().map(|&d| d <= limit).collect()
    } else {
        ink.clone()
    };

    let start = grid.cell_of(point).ok_or(FillError::NotClosed)?;
    let start = if ink[start] {
        return Err(FillError::OnLine);
    } else if blocked[start] {
        // Near a line, inside the gap-closing margin: start from the nearest
        // free cell reachable without crossing ink.
        nearest_free(&grid, start, &ink, &blocked).ok_or(FillError::OnLine)?
    } else {
        start
    };

    let region = flood(&grid, start, |i| !blocked[i]).ok_or(FillError::NotClosed)?;

    // Grow back over the gap margin. This rounds off corners and drops
    // narrow parts (a morphological opening), so put back every pocket of
    // gap-margin cells that touches the fill and is otherwise enclosed by
    // ink: corners and thin parts inside the shape, but not the gap itself,
    // which opens to the outside.
    let dist = distance_squared(&grid, &region);
    let grow = f64::from(gap_cells * gap_cells);
    let mut opened: Vec<bool> = (0..region.len())
        .map(|i| region[i] || (!ink[i] && dist[i] <= grow))
        .collect();
    if gap_cells > 0.0 {
        fill_pockets(&grid, &mut opened, &ink, &blocked);
    }
    // Then tuck under the lines: ink cells near the fill.
    let dist = distance_squared(&grid, &opened);
    let under = f64::from((tuck / cell + 1.0).powi(2));
    let grown: Vec<bool> = (0..region.len())
        .map(|i| opened[i] || (ink[i] && dist[i] <= under))
        .collect();
    // Keep only the part connected to the click.
    let filled = flood(&grid, start, |i| grown[i]).ok_or(FillError::NotClosed)?;

    let contours = trace_contours(&grid, &filled);
    if contours.is_empty() {
        return Err(FillError::NotClosed);
    }
    Ok(FillShape { contours })
}

/// The working raster: `w * h` square cells of side `cell`, the first one's
/// top-left corner at `origin` (document coordinates).
struct Grid {
    w: usize,
    h: usize,
    origin: Vec2,
    cell: f32,
}

impl Grid {
    fn len(&self) -> usize {
        self.w * self.h
    }

    fn cell_of(&self, p: Vec2) -> Option<usize> {
        let c = ((p - self.origin) / self.cell).floor();
        if c.x < 0.0 || c.y < 0.0 {
            return None;
        }
        let (x, y) = (c.x as usize, c.y as usize);
        (x < self.w && y < self.h).then_some(y * self.w + x)
    }

    fn center(&self, x: usize, y: usize) -> Vec2 {
        self.origin + (Vec2::new(x as f32, y as f32) + 0.5) * self.cell
    }

    /// Cell index range `[x0, x1) x [y0, y1)` covering document rect `lo..hi`.
    fn span(&self, lo: Vec2, hi: Vec2) -> (usize, usize, usize, usize) {
        let to = |v: f32, n: usize| (v.max(0.0) as usize).min(n);
        let a = (lo - self.origin) / self.cell;
        let b = (hi - self.origin) / self.cell;
        (
            to(a.x.floor(), self.w),
            to(b.x.ceil() + 1.0, self.w),
            to(a.y.floor(), self.h),
            to(b.y.ceil() + 1.0, self.h),
        )
    }

    fn neighbours(&self, i: usize) -> impl Iterator<Item = usize> {
        let (x, y) = (i % self.w, i / self.w);
        let w = self.w;
        [
            (x > 0).then(|| i - 1),
            (x + 1 < self.w).then(|| i + 1),
            (y > 0).then(|| i - w),
            (y + 1 < self.h).then(|| i + w),
        ]
        .into_iter()
        .flatten()
    }

    fn on_border(&self, i: usize) -> bool {
        let (x, y) = (i % self.w, i / self.w);
        x == 0 || y == 0 || x + 1 == self.w || y + 1 == self.h
    }
}

/// Cells whose centre lies within a stroke's painted shape (slightly inflated).
fn rasterize_ink(grid: &Grid, strokes: &[&Stroke]) -> Vec<bool> {
    let mut ink = vec![false; grid.len()];
    let inflate = INK_INFLATE_CELLS * grid.cell;
    let mut stamp = |p0: Vec2, r0: f32, p1: Vec2, r1: f32| {
        let reach = r0.max(r1) + inflate;
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
                let r = r0 + (r1 - r0) * t + inflate;
                if (p0 + seg * t).distance_squared(c) <= r * r {
                    ink[y * grid.w + x] = true;
                }
            }
        }
    };
    for stroke in strokes {
        let nodes = collect_nodes(stroke);
        match nodes.as_slice() {
            [] => {}
            [(p, r)] => stamp(*p, *r, *p, *r),
            _ => {
                for w in nodes.windows(2) {
                    let ((p0, r0), (p1, r1)) = (w[0], w[1]);
                    stamp(p0, r0, p1, r1);
                    // End circles, as in the hit test.
                    stamp(p0, r0, p0, r0);
                    stamp(p1, r1, p1, r1);
                }
            }
        }
    }
    ink
}

/// Squared Euclidean distance (in cells) from every cell to the nearest
/// `feature` cell (Felzenszwalb–Huttenlocher, exact, O(cells)).
fn distance_squared(grid: &Grid, feature: &[bool]) -> Vec<f64> {
    let (w, h) = (grid.w, grid.h);
    let mut d: Vec<f64> = feature.iter().map(|&f| if f { 0.0 } else { FAR }).collect();
    let n = w.max(h);
    let mut f = vec![0.0; n];
    let mut out = vec![0.0; n];
    let mut v = vec![0usize; n];
    let mut z = vec![0.0; n + 1];
    for x in 0..w {
        for y in 0..h {
            f[y] = d[y * w + x];
        }
        edt_1d(&f[..h], &mut out[..h], &mut v, &mut z);
        for y in 0..h {
            d[y * w + x] = out[y];
        }
    }
    for y in 0..h {
        let row = &mut d[y * w..(y + 1) * w];
        f[..w].copy_from_slice(row);
        edt_1d(&f[..w], &mut out[..w], &mut v, &mut z);
        row.copy_from_slice(&out[..w]);
    }
    d
}

/// 1-D squared distance transform of sampled function `f` (lower envelope of
/// parabolas).
fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let sq = |q: usize| (q * q) as f64;
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        let mut s;
        loop {
            let p = v[k];
            s = ((f[q] + sq(q)) - (f[p] + sq(p))) / (2.0 * (q as f64 - p as f64));
            if s <= z[k] && k > 0 {
                k -= 1;
            } else {
                break;
            }
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f64::INFINITY;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        let dq = q as f64 - p as f64;
        *out = dq * dq + f[p];
    }
}

/// 4-connected flood from `start` over cells where `open` holds. `None` if it
/// reaches the grid border (the area is not enclosed).
fn flood(grid: &Grid, start: usize, open: impl Fn(usize) -> bool) -> Option<Vec<bool>> {
    let mut seen = vec![false; grid.len()];
    let mut queue = VecDeque::from([start]);
    seen[start] = true;
    while let Some(i) = queue.pop_front() {
        if grid.on_border(i) {
            return None;
        }
        for j in grid.neighbours(i) {
            if !seen[j] && open(j) {
                seen[j] = true;
                queue.push_back(j);
            }
        }
    }
    Some(seen)
}

/// Adds to `fill` every 4-connected pocket of free (non-ink, unfilled) cells
/// that touches `fill`, lies entirely inside the gap margin (`blocked`) and
/// does not reach the grid border.
fn fill_pockets(grid: &Grid, fill: &mut [bool], ink: &[bool], blocked: &[bool]) {
    let free = |fill: &[bool], i: usize| !ink[i] && !fill[i];
    let mut seen = vec![false; grid.len()];
    let mut component = Vec::new();
    for seed in 0..grid.len() {
        if seen[seed] || !free(fill, seed) {
            continue;
        }
        component.clear();
        let (mut enclosed, mut touches_fill) = (true, false);
        let mut queue = VecDeque::from([seed]);
        seen[seed] = true;
        while let Some(i) = queue.pop_front() {
            component.push(i);
            if !blocked[i] || grid.on_border(i) {
                enclosed = false;
            }
            for j in grid.neighbours(i) {
                if fill[j] {
                    touches_fill = true;
                } else if !seen[j] && !ink[j] {
                    seen[j] = true;
                    queue.push_back(j);
                }
            }
        }
        if enclosed && touches_fill {
            for &i in &component {
                fill[i] = true;
            }
        }
    }
}

/// Breadth-first search from `start` through non-ink cells for the nearest
/// cell that is not blocked.
fn nearest_free(grid: &Grid, start: usize, ink: &[bool], blocked: &[bool]) -> Option<usize> {
    let mut seen = vec![false; grid.len()];
    let mut queue = VecDeque::from([start]);
    seen[start] = true;
    while let Some(i) = queue.pop_front() {
        if !blocked[i] {
            return Some(i);
        }
        for j in grid.neighbours(i) {
            if !seen[j] && !ink[j] {
                seen[j] = true;
                queue.push_back(j);
            }
        }
    }
    None
}

/// Boundary loops of `mask` as simplified contours in document coordinates.
///
/// Follows the cracks between inside and outside cells with the inside on the
/// right, so outer loops come out with positive signed area (y down) and
/// holes negative. At a corner where two inside cells touch only diagonally,
/// the trace turns right (they count as separate, matching the 4-connected
/// flood). Each loop's points are the midpoints of its crack edges, which is
/// the marching-squares outline of the mask.
fn trace_contours(grid: &Grid, mask: &[bool]) -> Vec<Vec<Vec2>> {
    // Directions: 0 = +x, 1 = +y, 2 = -x, 3 = -y.
    const STEP: [(i64, i64); 4] = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    let (w, h) = (grid.w, grid.h);
    let vw = w + 1;
    let inside = |x: i64, y: i64| {
        x >= 0
            && y >= 0
            && (x as usize) < w
            && (y as usize) < h
            && mask[y as usize * w + x as usize]
    };
    // Outgoing crack directions per corner vertex, as bit masks.
    let mut out = vec![0u8; vw * (h + 1)];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if !inside(x, y) {
                continue;
            }
            let (ux, uy) = (x as usize, y as usize);
            if !inside(x, y - 1) {
                out[uy * vw + ux] |= 1 << 0;
            }
            if !inside(x + 1, y) {
                out[uy * vw + ux + 1] |= 1 << 1;
            }
            if !inside(x, y + 1) {
                out[(uy + 1) * vw + ux + 1] |= 1 << 2;
            }
            if !inside(x - 1, y) {
                out[(uy + 1) * vw + ux] |= 1 << 3;
            }
        }
    }

    let tolerance = SIMPLIFY_TOLERANCE_CELLS;
    let mut contours = Vec::new();
    for start in 0..out.len() {
        while out[start] != 0 {
            let mut corners: Vec<Vec2> = Vec::new();
            let mut v = start;
            let mut dir = out[v].trailing_zeros() as usize;
            loop {
                out[v] &= !(1 << dir);
                let (x, y) = ((v % vw) as i64, (v / vw) as i64);
                corners.push(Vec2::new(x as f32, y as f32));
                let (dx, dy) = STEP[dir];
                v = ((y + dy) as usize) * vw + (x + dx) as usize;
                if v == start || out[v] == 0 {
                    break;
                }
                let right = (dir + 1) % 4;
                dir = if out[v] & (1 << right) != 0 {
                    right
                } else if out[v] & (1 << dir) != 0 {
                    dir
                } else {
                    out[v].trailing_zeros() as usize
                };
            }
            if corners.len() < 4 {
                continue;
            }
            let n = corners.len();
            let mids: Vec<Vec2> = (0..n)
                .map(|i| (corners[i] + corners[(i + 1) % n]) * 0.5)
                .collect();
            let simplified = simplify_closed(&mids, tolerance);
            if simplified.len() < 3 || signed_area(&simplified).abs() < MIN_CONTOUR_AREA_CELLS {
                continue;
            }
            contours.push(
                simplified
                    .into_iter()
                    .map(|p| grid.origin + p * grid.cell)
                    .collect(),
            );
        }
    }
    contours
}

/// Signed area by the shoelace formula; positive for loops that turn
/// clockwise on screen (y down).
pub(crate) fn signed_area(points: &[Vec2]) -> f32 {
    let n = points.len();
    (0..n)
        .map(|i| points[i].perp_dot(points[(i + 1) % n]))
        .sum::<f32>()
        * 0.5
}

/// Douglas–Peucker on a closed loop: split at the point furthest from the
/// first one and simplify both halves.
fn simplify_closed(points: &[Vec2], tolerance: f32) -> Vec<Vec2> {
    let n = points.len();
    if n <= 4 {
        return points.to_vec();
    }
    let far = (1..n)
        .max_by(|&a, &b| {
            points[0]
                .distance_squared(points[a])
                .total_cmp(&points[0].distance_squared(points[b]))
        })
        .unwrap_or(n / 2);
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[far] = true;
    mark_simplified(points, 0, far, tolerance, &mut keep);
    let mut tail: Vec<Vec2> = points[far..].to_vec();
    tail.push(points[0]);
    let mut keep_tail = vec![false; tail.len()];
    let last = tail.len() - 1;
    mark_simplified(&tail, 0, last, tolerance, &mut keep_tail);
    for (i, k) in keep_tail.iter().enumerate().take(last) {
        if *k {
            keep[far + i] = true;
        }
    }
    points
        .iter()
        .zip(&keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

/// Marks the points of `points[first..=last]` that Douglas–Peucker keeps
/// (iteratively, so long loops cannot overflow the stack).
fn mark_simplified(points: &[Vec2], first: usize, last: usize, tolerance: f32, keep: &mut [bool]) {
    let mut stack = vec![(first, last)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (points[a], points[b]);
        let seg = pb - pa;
        let len = seg.length();
        let (mut best, mut best_d) = (a, 0.0f32);
        for (i, &p) in points.iter().enumerate().take(b).skip(a + 1) {
            let d = if len > 0.0 {
                seg.perp_dot(p - pa).abs() / len
            } else {
                p.distance(pa)
            };
            if d > best_d {
                best = i;
                best_d = d;
            }
        }
        if best_d > tolerance {
            keep[best] = true;
            stack.push((a, best));
            stack.push((best, b));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{BrushStyle, StrokePoint};

    const CANVAS: Vec2 = Vec2::new(400.0, 400.0);

    fn polyline(points: &[(f32, f32)], width: f32) -> Stroke {
        Stroke::new(
            points
                .iter()
                .map(|&(x, y)| StrokePoint::new(Vec2::new(x, y)))
                .collect(),
            BrushStyle {
                width,
                ..Default::default()
            },
        )
    }

    fn drawing(strokes: Vec<Stroke>) -> VectorDrawing {
        let mut d = VectorDrawing::new();
        strokes.into_iter().for_each(|s| d.push_stroke(s));
        d
    }

    /// A 100x100 square at (100, 100) drawn with width 4, with a gap of
    /// `gap` units between the line ends (cap to cap) in the top edge.
    fn square_with_gap(gap: f32) -> VectorDrawing {
        // Line ends have round caps of radius 2.
        let half = gap * 0.5 + 2.0;
        if gap <= 0.0 {
            return drawing(vec![polyline(
                &[
                    (100.0, 100.0),
                    (200.0, 100.0),
                    (200.0, 200.0),
                    (100.0, 200.0),
                    (100.0, 100.0),
                ],
                4.0,
            )]);
        }
        drawing(vec![polyline(
            &[
                (150.0 + half, 100.0),
                (200.0, 100.0),
                (200.0, 200.0),
                (100.0, 200.0),
                (100.0, 100.0),
                (150.0 - half, 100.0),
            ],
            4.0,
        )])
    }

    fn circle(center: Vec2, radius: f32, width: f32) -> Stroke {
        let points: Vec<(f32, f32)> = (0..=64)
            .map(|i| {
                let p = center + Vec2::from_angle(i as f32 / 64.0 * std::f32::consts::TAU) * radius;
                (p.x, p.y)
            })
            .collect();
        polyline(&points, width)
    }

    fn assert_inside(shape: &FillShape, lo: Vec2, hi: Vec2) {
        let (min, max) = shape.bounds().unwrap();
        assert!(min.cmpge(lo).all() && max.cmple(hi).all(), "{min} {max}");
    }

    #[test]
    fn closed_square_fills() {
        let d = square_with_gap(0.0);
        let shape = fill_region(&d, Vec2::new(150.0, 150.0), GapClosing::None, CANVAS).unwrap();
        assert!(shape.contains(Vec2::new(150.0, 150.0)));
        assert!(
            shape.contains(Vec2::new(102.5, 197.5)),
            "reaches the corners"
        );
        assert!(!shape.contains(Vec2::new(250.0, 150.0)));
        // Tucks under the line (centre at 100..200, width 4) but not past it.
        assert_inside(&shape, Vec2::splat(97.5), Vec2::splat(202.5));
        let area = shape.area();
        assert!((96.0 * 96.0..=104.0 * 104.0).contains(&area), "{area}");
        assert!(
            shape.contours.iter().all(|c| c.len() < 200),
            "simplified: {:?}",
            shape.contours.iter().map(Vec::len).collect::<Vec<_>>()
        );
    }

    #[test]
    fn small_gap_needs_small_setting() {
        let d = square_with_gap(5.0);
        let click = Vec2::new(150.0, 150.0);
        assert_eq!(
            fill_region(&d, click, GapClosing::None, CANVAS),
            Err(FillError::NotClosed)
        );
        let shape = fill_region(&d, click, GapClosing::Small, CANVAS).unwrap();
        assert!(shape.contains(click));
        // Fills the square, not the outside, and barely bulges out of the gap.
        assert_inside(&shape, Vec2::splat(97.0), Vec2::splat(203.0));
        assert!(shape.area() > 90.0 * 90.0);
    }

    #[test]
    fn medium_gap_needs_medium_setting() {
        let d = square_with_gap(14.0);
        let click = Vec2::new(150.0, 150.0);
        assert_eq!(
            fill_region(&d, click, GapClosing::Small, CANVAS),
            Err(FillError::NotClosed)
        );
        let shape = fill_region(&d, click, GapClosing::Medium, CANVAS).unwrap();
        assert!(shape.contains(click));
        assert_inside(&shape, Vec2::splat(97.0), Vec2::splat(203.0));
        assert!(shape.area() > 85.0 * 85.0);
    }

    #[test]
    fn click_outside_shape_or_on_line_or_canvas_fails() {
        let d = square_with_gap(0.0);
        for gap in GapClosing::ALL {
            assert_eq!(
                fill_region(&d, Vec2::new(250.0, 150.0), gap, CANVAS),
                Err(FillError::NotClosed)
            );
            assert_eq!(
                fill_region(&d, Vec2::new(150.0, 100.5), gap, CANVAS),
                Err(FillError::OnLine)
            );
            assert_eq!(
                fill_region(&d, Vec2::new(-5.0, 150.0), gap, CANVAS),
                Err(FillError::OutsideCanvas)
            );
        }
        // Inside the bounds of an open shape: leaks around it.
        let open = drawing(vec![polyline(&[(100.0, 100.0), (200.0, 200.0)], 4.0)]);
        assert_eq!(
            fill_region(&open, Vec2::new(170.0, 130.0), GapClosing::Medium, CANVAS),
            Err(FillError::NotClosed)
        );
        assert_eq!(
            fill_region(&VectorDrawing::new(), Vec2::ONE, GapClosing::None, CANVAS),
            Err(FillError::NothingToFill)
        );
    }

    #[test]
    fn gap_closing_keeps_sharp_corners() {
        let d = square_with_gap(0.0);
        for gap in GapClosing::ALL {
            let shape = fill_region(&d, Vec2::new(150.0, 150.0), gap, CANVAS).unwrap();
            assert_eq!(shape.contours.len(), 1, "{gap:?}: no pinholes");
            for corner in [
                (102.5, 102.5),
                (197.5, 102.5),
                (197.5, 197.5),
                (102.5, 197.5),
            ] {
                assert!(shape.contains(Vec2::from(corner)), "{gap:?} {corner:?}");
            }
            assert!(
                shape.contours[0].len() <= 12,
                "{gap:?}: {:?}",
                shape.contours
            );
        }
    }

    #[test]
    fn click_close_to_a_line_with_gap_closing_still_fills() {
        let d = square_with_gap(0.0);
        let shape = fill_region(&d, Vec2::new(104.0, 150.0), GapClosing::Medium, CANVAS).unwrap();
        assert!(shape.contains(Vec2::new(150.0, 150.0)));
    }

    #[test]
    fn ring_keeps_its_hole() {
        let c = Vec2::new(200.0, 200.0);
        let d = drawing(vec![circle(c, 80.0, 3.0), circle(c, 40.0, 3.0)]);
        let shape = fill_region(&d, c + Vec2::new(60.0, 0.0), GapClosing::None, CANVAS).unwrap();
        assert_eq!(shape.contours.len(), 2);
        let areas: Vec<f32> = shape.contours.iter().map(|c| signed_area(c)).collect();
        assert!(areas.iter().any(|&a| a > 0.0) && areas.iter().any(|&a| a < 0.0));
        assert!(shape.contains(c + Vec2::new(0.0, 60.0)));
        assert!(!shape.contains(c), "hole stays empty");
        assert!(!shape.contains(c + Vec2::new(100.0, 0.0)));
        assert_inside(&shape, c - 82.0, c + 82.0);
        let expected = std::f32::consts::PI * (80.0f32.powi(2) - 40.0f32.powi(2));
        assert!((shape.area() - expected).abs() < expected * 0.05);

        // The inner disc fills on its own, without the ring.
        let inner = fill_region(&d, c, GapClosing::None, CANVAS).unwrap();
        assert_eq!(inner.contours.len(), 1);
        assert!(!inner.contains(c + Vec2::new(60.0, 0.0)));
    }

    #[test]
    fn fill_hit_test_and_drawing_fill_list() {
        let mut d = square_with_gap(0.0);
        let shape = fill_region(&d, Vec2::new(150.0, 150.0), GapClosing::None, CANVAS).unwrap();
        let fill = Fill::new(shape, Rgba::WHITE, 1.0);
        let id = fill.id;
        d.push_fill(fill);
        assert_eq!(d.fill_at(Vec2::new(150.0, 150.0)), Some(id));
        assert_eq!(d.fill_at(Vec2::new(300.0, 150.0)), None);
        let (index, removed) = d.remove_fill(id).unwrap();
        assert!(d.fills().is_empty());
        d.insert_fill(index, removed);
        assert!(d.fill(id).is_some());
    }

    #[test]
    fn distance_transform_is_exact() {
        let grid = Grid {
            w: 7,
            h: 5,
            origin: Vec2::ZERO,
            cell: 1.0,
        };
        let mut feature = vec![false; grid.len()];
        feature[2 * 7 + 3] = true;
        let d = distance_squared(&grid, &feature);
        for y in 0..5 {
            for x in 0..7 {
                let expected = ((x as f64 - 3.0).powi(2)) + ((y as f64 - 2.0).powi(2));
                assert_eq!(d[y * 7 + x], expected, "({x}, {y})");
            }
        }
    }

    #[test]
    fn simplify_drops_collinear_points() {
        let square: Vec<Vec2> = (0..40)
            .map(|i| match i / 10 {
                0 => Vec2::new(i as f32, 0.0),
                1 => Vec2::new(10.0, (i - 10) as f32),
                2 => Vec2::new(10.0 - (i - 20) as f32, 10.0),
                _ => Vec2::new(0.0, 10.0 - (i - 30) as f32),
            })
            .collect();
        let s = simplify_closed(&square, 0.1);
        assert_eq!(s.len(), 4, "{s:?}");
        assert!((signed_area(&s) - 100.0).abs() < 1e-3);
    }
}
