//! Strokes, brush styles and vector drawings.

use animation_core::{FillId, Rgba, StrokeId, Vec2};
use serde::{Deserialize, Serialize};

use crate::fill::Fill;

/// One sampled input point of a stroke, in document coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokePoint {
    /// Position in document (canvas) coordinates.
    pub pos: Vec2,
    /// Pen pressure in `0.0..=1.0`. Mouse input uses `1.0`.
    pub pressure: f32,
}

impl StrokePoint {
    /// A point with full pressure (mouse input).
    pub const fn new(pos: Vec2) -> Self {
        Self { pos, pressure: 1.0 }
    }
}

/// Visual settings of a stroke.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushStyle {
    /// Stroke color.
    pub color: Rgba,
    /// Full-pressure width in document units.
    pub width: f32,
    /// Opacity multiplier in `0.0..=1.0`.
    pub opacity: f32,
}

impl Default for BrushStyle {
    fn default() -> Self {
        Self {
            color: Rgba::BLACK,
            width: 4.0,
            opacity: 1.0,
        }
    }
}

/// The draw-order tier of a stroke or fill (ADR-010, ADR-011).
///
/// Per drawing, items paint bottom to top: the back levels from the deepest
/// up (each level's fills, then its strokes), then normal fills, normal
/// strokes, front fills, front strokes. Within a group the stored order
/// holds (newer above older, unless an item was added at the bottom). So a
/// [`Placement::Front`] item stays above every normal item, including ones
/// drawn later, and a [`Placement::Back`] item stays under every normal
/// item and under every shallower back item.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Placement {
    /// Under every normal item. Larger levels paint further back. Each
    /// "Behind" commit gets a level one deeper than any already in its
    /// drawing ([`VectorDrawing::behind_placement`]), so it lands under
    /// everything there, fills included, while a shape's own fill and outline
    /// share one level and keep fill-under-outline (ADR-011).
    Back(u32),
    /// The ordinary tier: fills under strokes, as before ADR-010.
    #[default]
    Normal,
    /// Always drawn above the drawing's normal items.
    Front,
}

impl Placement {
    /// Whether this is the default tier (used to leave it out of files).
    pub fn is_normal(&self) -> bool {
        *self == Self::Normal
    }

    /// Sort key: lower paints first. Deeper back levels come first.
    fn rank(self) -> (u8, std::cmp::Reverse<u32>) {
        match self {
            Self::Back(level) => (0, std::cmp::Reverse(level)),
            Self::Normal => (1, std::cmp::Reverse(0)),
            Self::Front => (2, std::cmp::Reverse(0)),
        }
    }
}

/// A single editable brush stroke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// Stable identifier.
    pub id: StrokeId,
    /// Input points in drawing order.
    pub points: Vec<StrokePoint>,
    /// Brush settings used for the stroke.
    pub style: BrushStyle,
    /// Draw-order tier; absent in files means [`Placement::Normal`].
    #[serde(default, skip_serializing_if = "Placement::is_normal")]
    pub placement: Placement,
}

impl Stroke {
    /// Creates a stroke with a fresh ID.
    pub fn new(points: Vec<StrokePoint>, style: BrushStyle) -> Self {
        Self {
            id: StrokeId::new(),
            points,
            style,
            placement: Placement::Normal,
        }
    }

    /// The same stroke in `placement`'s tier.
    pub fn with_placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// Axis-aligned bounds `(min, max)` including half the brush width.
    /// Returns `None` for a stroke with no points.
    pub fn bounds(&self) -> Option<(Vec2, Vec2)> {
        let first = self.points.first()?.pos;
        let (min, max) = self
            .points
            .iter()
            .fold((first, first), |(lo, hi), p| (lo.min(p.pos), hi.max(p.pos)));
        let pad = Vec2::splat(self.style.width * 0.5);
        Some((min - pad, max + pad))
    }
}

/// A vector drawing. Items paint in [`VectorDrawing::paint_order`]: back
/// levels (deepest first, fills then strokes), normal fills, normal strokes,
/// front fills, front strokes, each group in stored order (bottom-to-top).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VectorDrawing {
    strokes: Vec<Stroke>,
    /// Filled regions, drawn under the strokes of the same tier (file format
    /// v2+).
    fills: Vec<Fill>,
}

impl VectorDrawing {
    /// Creates an empty drawing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a stroke on top of the others.
    pub fn push_stroke(&mut self, stroke: Stroke) {
        self.strokes.push(stroke);
    }

    /// Inserts a stroke at `index` (clamped), used to undo a removal.
    pub fn insert_stroke(&mut self, index: usize, stroke: Stroke) {
        let index = index.min(self.strokes.len());
        self.strokes.insert(index, stroke);
    }

    /// Removes a stroke by ID, returning its former index and the stroke.
    pub fn remove_stroke(&mut self, id: StrokeId) -> Option<(usize, Stroke)> {
        let index = self.strokes.iter().position(|s| s.id == id)?;
        Some((index, self.strokes.remove(index)))
    }

    /// Looks up a stroke by ID.
    pub fn stroke(&self, id: StrokeId) -> Option<&Stroke> {
        self.strokes.iter().find(|s| s.id == id)
    }

    /// All strokes in stored order. Front strokes paint above normal ones
    /// whatever their index; see [`Self::paint_order`].
    pub fn strokes(&self) -> &[Stroke] {
        &self.strokes
    }

    pub(crate) fn strokes_mut(&mut self) -> &mut [Stroke] {
        &mut self.strokes
    }

    pub(crate) fn fills_mut(&mut self) -> &mut [Fill] {
        &mut self.fills
    }

    /// Whether the drawing has no strokes and no fills.
    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty() && self.fills.is_empty()
    }

    /// Appends a fill on top of the other fills of its tier (still under the
    /// strokes of that tier).
    pub fn push_fill(&mut self, fill: Fill) {
        self.fills.push(fill);
    }

    /// Inserts a fill at `index` (clamped), used to undo a removal.
    pub fn insert_fill(&mut self, index: usize, fill: Fill) {
        let index = index.min(self.fills.len());
        self.fills.insert(index, fill);
    }

    /// Removes a fill by ID, returning its former index and the fill.
    pub fn remove_fill(&mut self, id: FillId) -> Option<(usize, Fill)> {
        let index = self.fills.iter().position(|f| f.id == id)?;
        Some((index, self.fills.remove(index)))
    }

    /// Looks up a fill by ID.
    pub fn fill(&self, id: FillId) -> Option<&Fill> {
        self.fills.iter().find(|f| f.id == id)
    }

    /// All fills in stored order. Front fills paint above normal ones
    /// whatever their index; see [`Self::paint_order`].
    pub fn fills(&self) -> &[Fill] {
        &self.fills
    }

    /// Adds a stroke to its tier: at the bottom of the drawing's strokes when
    /// `at_bottom`, else on top.
    pub fn add_stroke(&mut self, stroke: Stroke, at_bottom: bool) {
        if at_bottom {
            self.insert_stroke(0, stroke);
        } else {
            self.push_stroke(stroke);
        }
    }

    /// Adds a fill to its tier: at the bottom of the drawing's fills when
    /// `at_bottom`, else on top.
    pub fn add_fill(&mut self, fill: Fill, at_bottom: bool) {
        if at_bottom {
            self.insert_fill(0, fill);
        } else {
            self.push_fill(fill);
        }
    }

    /// The placement for an item added "Behind": a back level one deeper
    /// than any in this drawing, so it paints under everything already here
    /// (ADR-011). Give a shape's fill and outline the same placement.
    pub fn behind_placement(&self) -> Placement {
        let deepest = self
            .fills
            .iter()
            .map(|f| f.placement)
            .chain(self.strokes.iter().map(|s| s.placement))
            .filter_map(|p| match p {
                Placement::Back(level) => Some(level),
                Placement::Normal | Placement::Front => None,
            })
            .max()
            .unwrap_or(0);
        Placement::Back(deepest.saturating_add(1))
    }

    /// Every item in the order it is painted, bottom to top: back levels
    /// from the deepest up, then normal fills, normal strokes, front fills,
    /// front strokes (ADR-010, ADR-011). Within a level fills paint under
    /// strokes, and each kind keeps its stored order. Reverse it for
    /// topmost-first hit-testing.
    pub fn paint_order(&self) -> impl DoubleEndedIterator<Item = PaintItem<'_>> + '_ {
        let mut items: Vec<PaintItem<'_>> = self
            .fills
            .iter()
            .map(PaintItem::Fill)
            .chain(self.strokes.iter().map(PaintItem::Stroke))
            .collect();
        // Stable, so stored order holds within each (tier, kind) group.
        items.sort_by_key(|item| {
            (
                item.placement().rank(),
                matches!(item, PaintItem::Stroke(_)),
            )
        });
        items.into_iter()
    }

    /// Strokes in paint order, bottom to top.
    pub fn strokes_painted(&self) -> impl DoubleEndedIterator<Item = &Stroke> + '_ {
        self.paint_order().filter_map(|item| match item {
            PaintItem::Stroke(s) => Some(s),
            PaintItem::Fill(_) => None,
        })
    }

    /// Fills in paint order, bottom to top.
    pub fn fills_painted(&self) -> impl DoubleEndedIterator<Item = &Fill> + '_ {
        self.paint_order().filter_map(|item| match item {
            PaintItem::Fill(f) => Some(f),
            PaintItem::Stroke(_) => None,
        })
    }

    /// The topmost fill covering `point`, if any.
    pub fn fill_at(&self, point: Vec2) -> Option<FillId> {
        self.fills_painted()
            .rev()
            .find(|f| f.contains(point))
            .map(|f| f.id)
    }
}

/// One item of a drawing, as yielded by [`VectorDrawing::paint_order`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PaintItem<'a> {
    /// A fill.
    Fill(&'a Fill),
    /// A stroke.
    Stroke(&'a Stroke),
}

impl PaintItem<'_> {
    /// The item's draw-order tier.
    pub fn placement(&self) -> Placement {
        match self {
            Self::Fill(f) => f.placement,
            Self::Stroke(s) => s.placement,
        }
    }
}

/// The content of a drawing. Vector is the only kind in the MVP; a raster
/// variant can be added later without changing the timeline or document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DrawingContent {
    /// Editable vector strokes.
    Vector(VectorDrawing),
}

impl Default for DrawingContent {
    fn default() -> Self {
        Self::Vector(VectorDrawing::new())
    }
}

impl DrawingContent {
    /// The vector drawing, if this content is vector.
    pub fn as_vector(&self) -> Option<&VectorDrawing> {
        match self {
            Self::Vector(v) => Some(v),
        }
    }

    /// Mutable access to the vector drawing, if this content is vector.
    pub fn as_vector_mut(&mut self) -> Option<&mut VectorDrawing> {
        match self {
            Self::Vector(v) => Some(v),
        }
    }

    /// A deep copy whose strokes and fills get fresh IDs, so the copy can be
    /// edited (and its items selected) independently of the original.
    pub fn duplicate(&self) -> Self {
        match self {
            Self::Vector(v) => Self::Vector(v.duplicate()),
        }
    }
}

impl VectorDrawing {
    /// A deep copy with fresh stroke and fill IDs, in the same paint order.
    pub fn duplicate(&self) -> Self {
        let mut copy = self.clone();
        for stroke in &mut copy.strokes {
            stroke.id = StrokeId::new();
        }
        for fill in &mut copy.fills {
            fill.id = FillId::new();
        }
        copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Stroke {
        Stroke::new(
            vec![
                StrokePoint::new(Vec2::new(0.0, 0.0)),
                StrokePoint::new(Vec2::new(10.0, 5.0)),
            ],
            BrushStyle {
                width: 2.0,
                ..Default::default()
            },
        )
    }

    #[test]
    fn bounds_include_brush_width() {
        let (min, max) = line().bounds().unwrap();
        assert_eq!(min, Vec2::new(-1.0, -1.0));
        assert_eq!(max, Vec2::new(11.0, 6.0));
        assert!(
            Stroke::new(vec![], BrushStyle::default())
                .bounds()
                .is_none()
        );
    }

    #[test]
    fn remove_and_reinsert_keeps_order() {
        let mut d = VectorDrawing::new();
        let (a, b, c) = (line(), line(), line());
        let ids = [a.id, b.id, c.id];
        d.push_stroke(a);
        d.push_stroke(b);
        d.push_stroke(c);
        let (idx, removed) = d.remove_stroke(ids[1]).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(d.strokes().len(), 2);
        d.insert_stroke(idx, removed);
        let order: Vec<_> = d.strokes().iter().map(|s| s.id).collect();
        assert_eq!(order, ids);
    }

    #[test]
    fn removing_unknown_stroke_is_none() {
        assert!(
            VectorDrawing::new()
                .remove_stroke(StrokeId::new())
                .is_none()
        );
    }

    #[test]
    fn duplicate_is_deep_with_fresh_ids() {
        let mut d = VectorDrawing::new();
        d.push_stroke(line());
        d.push_stroke(line());
        d.push_fill(crate::fill::Fill::new(
            crate::fill::FillShape {
                contours: vec![vec![Vec2::ZERO, Vec2::X, Vec2::Y]],
            },
            Rgba::WHITE,
            1.0,
        ));
        let copy = d.duplicate();
        assert_eq!(copy.strokes().len(), 2);
        assert_eq!(copy.fills().len(), 1);
        for (a, b) in d.strokes().iter().zip(copy.strokes()) {
            assert_ne!(a.id, b.id);
            assert_eq!(a.points, b.points);
            assert_eq!(a.style, b.style);
        }
        assert_ne!(d.fills()[0].id, copy.fills()[0].id);
        assert_eq!(d.fills()[0].contours, copy.fills()[0].contours);
    }

    fn square_fill() -> Fill {
        crate::fill::Fill::new(
            crate::fill::FillShape {
                contours: vec![vec![
                    Vec2::new(-5.0, -5.0),
                    Vec2::new(20.0, -5.0),
                    Vec2::new(20.0, 20.0),
                    Vec2::new(-5.0, 20.0),
                ]],
            },
            Rgba::WHITE,
            1.0,
        )
    }

    fn order(d: &VectorDrawing) -> Vec<crate::ItemId> {
        d.paint_order().map(crate::ItemId::from).collect()
    }

    #[test]
    fn paint_order_groups_by_tier_then_kind() {
        let mut d = VectorDrawing::new();
        let front_stroke = line().with_placement(Placement::Front);
        let normal_stroke = line();
        let front_fill = square_fill().with_placement(Placement::Front);
        let normal_fill = square_fill();
        let later_stroke = line();
        let ids = (
            front_stroke.id,
            normal_stroke.id,
            front_fill.id,
            normal_fill.id,
            later_stroke.id,
        );
        // Added in an order that mixes tiers and kinds.
        d.push_stroke(front_stroke);
        d.push_fill(front_fill);
        d.push_stroke(normal_stroke);
        d.push_fill(normal_fill);
        d.push_stroke(later_stroke);
        use crate::ItemId::{Fill as F, Stroke as S};
        assert_eq!(
            order(&d),
            vec![F(ids.3), S(ids.1), S(ids.4), F(ids.2), S(ids.0)]
        );
        // Hit-testing finds the topmost painted item: the front stroke,
        // then (off the line) the front fill covering the normal strokes.
        assert_eq!(d.item_at(Vec2::new(5.0, 2.5), 0.5), Some(S(ids.0)));
        assert_eq!(d.item_at(Vec2::new(15.0, 15.0), 0.5), Some(F(ids.2)));
        assert_eq!(d.fill_at(Vec2::new(15.0, 15.0)), Some(ids.2));
        assert_eq!(
            d.strokes_hit(Vec2::new(5.0, 2.5), 0.5),
            vec![ids.1, ids.4, ids.0]
        );
        assert_eq!(d.all_items(), order(&d));
    }

    #[test]
    fn behind_goes_under_every_existing_fill_and_stroke() {
        use crate::ItemId::{Fill as F, Stroke as S};
        let mut d = VectorDrawing::new();
        let (normal_fill, normal_stroke) = (square_fill(), line());
        let ids = (normal_fill.id, normal_stroke.id);
        d.add_fill(normal_fill, false);
        d.add_stroke(normal_stroke, false);
        assert_eq!(d.behind_placement(), Placement::Back(1));

        // A behind brush stroke paints under the normal fill.
        let first = line().with_placement(d.behind_placement());
        let first_id = first.id;
        d.add_stroke(first, true);
        assert_eq!(order(&d), vec![S(first_id), F(ids.0), S(ids.1)]);

        // A behind shape (fill + outline on one level) goes under that
        // stroke, with its outline still above its own fill.
        let level = d.behind_placement();
        assert_eq!(level, Placement::Back(2));
        let (shape_fill, outline) = (
            square_fill().with_placement(level),
            line().with_placement(level),
        );
        let shape = (shape_fill.id, outline.id);
        d.add_fill(shape_fill, true);
        d.add_stroke(outline, true);
        assert_eq!(
            order(&d),
            vec![F(shape.0), S(shape.1), S(first_id), F(ids.0), S(ids.1)]
        );

        // The next behind stroke goes under the behind shape's fill too.
        let last = line().with_placement(d.behind_placement());
        let last_id = last.id;
        d.add_stroke(last, true);
        assert_eq!(order(&d)[0], S(last_id));
        assert_eq!(d.all_items(), order(&d));
        // Hit-testing: the normal fill covers the behind items under it.
        assert_eq!(d.fill_at(Vec2::new(15.0, 15.0)), Some(ids.0));
        assert_eq!(d.strokes_painted().last().map(|s| s.id), Some(ids.1));
    }

    #[test]
    fn erase_transform_and_duplicate_keep_the_back_level() {
        let stroke = line().with_placement(Placement::Back(3));
        let pieces = stroke
            .erase(&[Vec2::new(5.0, 2.5)], 1.0)
            .into_replacements()
            .unwrap();
        assert!(!pieces.is_empty());
        assert!(pieces.iter().all(|p| p.placement == Placement::Back(3)));
        let moved = stroke.transformed(&crate::select::Affine2::from_angle(1.0));
        assert_eq!(moved.placement, Placement::Back(3));
        let fill = square_fill().with_placement(Placement::Back(3));
        assert_eq!(fill.translated(Vec2::ONE).placement, Placement::Back(3));
        let mut d = VectorDrawing::new();
        d.push_fill(fill);
        assert_eq!(d.duplicate().fills()[0].placement, Placement::Back(3));
    }

    #[test]
    fn add_at_bottom_stays_in_its_tier() {
        let mut d = VectorDrawing::new();
        let front = line().with_placement(Placement::Front);
        let top = line();
        let bottom = line();
        let ids = [bottom.id, top.id, front.id];
        d.add_stroke(front, false);
        d.add_stroke(top, false);
        d.add_stroke(bottom, true);
        let painted: Vec<_> = d.strokes_painted().map(|s| s.id).collect();
        assert_eq!(painted, ids);
    }

    #[test]
    fn erase_and_transform_keep_the_tier() {
        let stroke = line().with_placement(Placement::Front);
        let pieces = stroke
            .erase(&[Vec2::new(5.0, 2.5)], 1.0)
            .into_replacements()
            .unwrap();
        assert!(!pieces.is_empty());
        assert!(pieces.iter().all(|p| p.placement == Placement::Front));
        let moved = stroke.transformed(&crate::select::Affine2::from_angle(1.0));
        assert_eq!(moved.placement, Placement::Front);
        let fill = square_fill().with_placement(Placement::Front);
        assert_eq!(fill.translated(Vec2::ONE).placement, Placement::Front);
        let copy = {
            let mut d = VectorDrawing::new();
            d.push_fill(fill);
            d.duplicate()
        };
        assert_eq!(copy.fills()[0].placement, Placement::Front);
    }
}
