//! Selection geometry: item ids, bounds, translation, scale/rotate
//! transforms and hit-testing a point or a rectangle against strokes and
//! fills (the selection and transform tools).
//!
//! Rectangles are `(min, max)` pairs in document coordinates, like
//! [`Stroke::bounds`].

use animation_core::{FillId, ImageId, StrokeId, Vec2};
pub use glam::Affine2;

use crate::fill::{Fill, FillShape};
use crate::stroke::{PaintItem, Stroke, VectorDrawing};

/// One selectable thing in a drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemId {
    /// A stroke.
    Stroke(StrokeId),
    /// A fill.
    Fill(FillId),
    /// An imported picture.
    Image(ImageId),
}

impl From<ImageId> for ItemId {
    fn from(id: ImageId) -> Self {
        Self::Image(id)
    }
}

impl From<StrokeId> for ItemId {
    fn from(id: StrokeId) -> Self {
        Self::Stroke(id)
    }
}

impl From<PaintItem<'_>> for ItemId {
    fn from(item: PaintItem<'_>) -> Self {
        match item {
            PaintItem::Fill(f) => Self::Fill(f.id),
            PaintItem::Stroke(s) => Self::Stroke(s.id),
            PaintItem::Image(i) => Self::Image(i.id),
        }
    }
}

impl From<FillId> for ItemId {
    fn from(id: FillId) -> Self {
        Self::Fill(id)
    }
}

/// The rectangle spanned by two corners in any order.
pub fn rect_from_corners(a: Vec2, b: Vec2) -> (Vec2, Vec2) {
    (a.min(b), a.max(b))
}

/// Whether two `(min, max)` rectangles overlap or touch.
pub fn rects_intersect(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> bool {
    a.0.x <= b.1.x && b.0.x <= a.1.x && a.0.y <= b.1.y && b.0.y <= a.1.y
}

/// The smallest rectangle containing both.
pub fn rect_union(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> (Vec2, Vec2) {
    (a.0.min(b.0), a.1.max(b.1))
}

/// Scales by `scale` and then rotates by `angle` (radians, positive turns
/// +x towards +y), both about `pivot`. The scale is along the document axes,
/// before rotation, so it matches a box handle dragged on axis-aligned
/// bounds. Negative factors mirror.
pub fn scale_rotate_about(pivot: Vec2, scale: Vec2, angle: f32) -> Affine2 {
    Affine2::from_translation(pivot)
        * Affine2::from_angle(angle)
        * Affine2::from_scale(scale)
        * Affine2::from_translation(-pivot)
}

/// How much a transform scales lengths on average: the square root of its
/// area scale. Used for brush widths, which have no direction.
pub fn width_scale(transform: &Affine2) -> f32 {
    transform.matrix2.determinant().abs().sqrt()
}

impl Stroke {
    /// Maps every point through `transform` and scales the brush width by
    /// [`width_scale`], so a uniformly scaled stroke keeps its look.
    pub fn transform(&mut self, transform: &Affine2) {
        for point in &mut self.points {
            point.pos = transform.transform_point2(point.pos);
        }
        self.style.width *= width_scale(transform);
    }

    /// A transformed copy, keeping the same ID.
    pub fn transformed(&self, transform: &Affine2) -> Self {
        let mut out = self.clone();
        out.transform(transform);
        out
    }

    /// Moves every point by `delta`.
    pub fn translate(&mut self, delta: Vec2) {
        for point in &mut self.points {
            point.pos += delta;
        }
    }

    /// A copy moved by `delta`, keeping the same ID.
    pub fn translated(&self, delta: Vec2) -> Self {
        let mut moved = self.clone();
        moved.translate(delta);
        moved
    }
}

impl Fill {
    /// Maps every contour point through `transform`. A mirroring transform
    /// would flip each contour's winding, which tells outer contours from
    /// holes, so contours are then reversed to keep it.
    pub fn transform(&mut self, transform: &Affine2) {
        let mirrored = transform.matrix2.determinant() < 0.0;
        for contour in &mut self.contours {
            for point in contour.iter_mut() {
                *point = transform.transform_point2(*point);
            }
            if mirrored {
                contour.reverse();
            }
        }
        if let Some(g) = &mut self.gradient {
            g.start = transform.transform_point2(g.start);
            g.end = transform.transform_point2(g.end);
        }
    }

    /// A transformed copy, keeping the same ID.
    pub fn transformed(&self, transform: &Affine2) -> Self {
        let mut out = self.clone();
        out.transform(transform);
        out
    }

    /// Moves every contour point by `delta`.
    pub fn translate(&mut self, delta: Vec2) {
        for point in self.contours.iter_mut().flatten() {
            *point += delta;
        }
        if let Some(g) = &mut self.gradient {
            g.start += delta;
            g.end += delta;
        }
    }

    /// A copy moved by `delta`, keeping the same ID.
    pub fn translated(&self, delta: Vec2) -> Self {
        let mut moved = self.clone();
        moved.translate(delta);
        moved
    }
}

impl VectorDrawing {
    /// Mutable access to a stroke by ID.
    pub fn stroke_mut(&mut self, id: StrokeId) -> Option<&mut Stroke> {
        self.strokes_mut().iter_mut().find(|s| s.id == id)
    }

    /// Mutable access to a fill by ID.
    pub fn fill_mut(&mut self, id: FillId) -> Option<&mut Fill> {
        self.fills_mut().iter_mut().find(|f| f.id == id)
    }

    /// Moves every stroke and fill by `delta`, keeping IDs and order.
    pub fn translate(&mut self, delta: Vec2) {
        for stroke in self.strokes_mut() {
            stroke.translate(delta);
        }
        for fill in self.fills_mut() {
            fill.translate(delta);
        }
        for image in self.images_mut() {
            image.translate(delta);
        }
    }

    /// Whether the drawing holds `item`.
    pub fn contains_item(&self, item: ItemId) -> bool {
        match item {
            ItemId::Stroke(id) => self.stroke(id).is_some(),
            ItemId::Fill(id) => self.fill(id).is_some(),
            ItemId::Image(id) => self.image(id).is_some(),
        }
    }

    /// Every item in paint order, bottom to top (see
    /// [`VectorDrawing::paint_order`]).
    pub fn all_items(&self) -> Vec<ItemId> {
        self.paint_order().map(ItemId::from).collect()
    }

    /// Bounds of one item (strokes include half the brush width). `None` if
    /// the item is missing or has no points.
    pub fn item_bounds(&self, item: ItemId) -> Option<(Vec2, Vec2)> {
        match item {
            ItemId::Stroke(id) => self.stroke(id)?.bounds(),
            ItemId::Fill(id) => self.fill(id)?.bounds(),
            ItemId::Image(id) => Some(self.image(id)?.bounds()),
        }
    }

    /// Bounds of all `items` together; missing items are skipped.
    pub fn items_bounds(&self, items: &[ItemId]) -> Option<(Vec2, Vec2)> {
        items
            .iter()
            .filter_map(|&item| self.item_bounds(item))
            .reduce(rect_union)
    }

    /// The topmost item under `point`, checking items top to bottom in paint
    /// order; `tolerance` (document units) widens strokes so thin lines are
    /// easy to click. Fills must contain the point.
    pub fn item_at(&self, point: Vec2, tolerance: f32) -> Option<ItemId> {
        self.paint_order()
            .rev()
            .find(|item| match item {
                PaintItem::Stroke(s) => s.hit_test(point, tolerance),
                PaintItem::Fill(f) => f.contains(point),
                PaintItem::Image(i) => i.contains(point),
            })
            .map(ItemId::from)
    }

    /// Items with any point (stroke point, fill contour point or image
    /// corner) inside
    /// `polygon`, in the order of [`Self::all_items`]. A fill the polygon
    /// lies entirely within is not hit, so a box drawn over a background
    /// fill leaves it alone.
    // ponytail: tests points only, so a box can miss a sparse stroke segment
    // passing through it; add a segment-crossing test if that bites.
    pub fn items_in_polygon(&self, polygon: &[Vec2]) -> Vec<ItemId> {
        let shape = FillShape {
            contours: vec![polygon.to_vec()],
        };
        let Some(area) = shape.bounds() else {
            return Vec::new();
        };
        self.all_items()
            .into_iter()
            .filter(|&item| {
                self.item_bounds(item)
                    .is_some_and(|b| rects_intersect(b, area))
            })
            .filter(|&item| match item {
                ItemId::Stroke(id) => self
                    .stroke(id)
                    .is_some_and(|s| s.points.iter().any(|p| shape.contains(p.pos))),
                ItemId::Fill(id) => self
                    .fill(id)
                    .is_some_and(|f| f.contours.iter().flatten().any(|&p| shape.contains(p))),
                ItemId::Image(id) => self
                    .image(id)
                    .is_some_and(|i| i.corners().iter().any(|&p| shape.contains(p))),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{BrushStyle, StrokePoint};
    use animation_core::Rgba;

    fn v(x: f32, y: f32) -> Vec2 {
        Vec2::new(x, y)
    }

    fn line(a: Vec2, b: Vec2) -> Stroke {
        Stroke::new(
            vec![StrokePoint::new(a), StrokePoint::new(b)],
            BrushStyle {
                width: 2.0,
                ..Default::default()
            },
        )
    }

    fn square(min: Vec2, size: f32) -> Fill {
        Fill::new(
            FillShape {
                contours: vec![vec![
                    min,
                    min + v(size, 0.0),
                    min + v(size, size),
                    min + v(0.0, size),
                ]],
            },
            Rgba::BLACK,
            1.0,
        )
    }

    /// A fill square at (0,0)-(100,100) with a line across its middle, and a
    /// far-away line.
    fn scene() -> (VectorDrawing, ItemId, ItemId, ItemId) {
        let mut d = VectorDrawing::new();
        let fill = square(v(0.0, 0.0), 100.0);
        let across = line(v(-10.0, 50.0), v(110.0, 50.0));
        let far = line(v(300.0, 300.0), v(320.0, 300.0));
        let ids = (
            ItemId::Fill(fill.id),
            ItemId::Stroke(across.id),
            ItemId::Stroke(far.id),
        );
        d.push_fill(fill);
        d.push_stroke(across);
        d.push_stroke(far);
        (d, ids.0, ids.1, ids.2)
    }

    #[test]
    fn rect_helpers() {
        assert_eq!(
            rect_from_corners(v(5.0, 1.0), v(2.0, 4.0)),
            (v(2.0, 1.0), v(5.0, 4.0))
        );
        let a = (v(0.0, 0.0), v(10.0, 10.0));
        assert!(
            rects_intersect(a, (v(10.0, 10.0), v(20.0, 20.0))),
            "touching counts"
        );
        assert!(rects_intersect(a, (v(2.0, 2.0), v(3.0, 3.0))), "contained");
        assert!(!rects_intersect(a, (v(11.0, 0.0), v(20.0, 5.0))));
        assert!(!rects_intersect(a, (v(0.0, -5.0), v(5.0, -1.0))));
        assert_eq!(
            rect_union(a, (v(-5.0, 3.0), v(4.0, 20.0))),
            (v(-5.0, 0.0), v(10.0, 20.0))
        );
    }

    #[test]
    fn translate_moves_geometry_and_keeps_ids() {
        let s = line(v(0.0, 0.0), v(10.0, 0.0));
        let moved = s.translated(v(3.0, -2.0));
        assert_eq!(moved.id, s.id);
        assert_eq!(moved.points[1].pos, v(13.0, -2.0));
        assert_eq!(moved.points[1].pressure, s.points[1].pressure);
        assert_eq!(moved.style, s.style);

        let f = square(v(0.0, 0.0), 10.0);
        let moved = f.translated(v(5.0, 5.0));
        assert_eq!(moved.id, f.id);
        assert_eq!(moved.bounds(), Some((v(5.0, 5.0), v(15.0, 15.0))));
        assert!(moved.contains(v(14.0, 14.0)) && !f.contains(v(14.0, 14.0)));
    }

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-4
    }

    #[test]
    fn scale_about_pivot_keeps_the_pivot() {
        let t = scale_rotate_about(v(10.0, 20.0), v(2.0, 3.0), 0.0);
        assert!(close(t.transform_point2(v(10.0, 20.0)), v(10.0, 20.0)));
        assert!(close(t.transform_point2(v(11.0, 21.0)), v(12.0, 23.0)));
        assert!(close(t.transform_point2(v(0.0, 0.0)), v(-10.0, -40.0)));
        assert!((width_scale(&t) - 6f32.sqrt()).abs() < 1e-5);
    }

    #[test]
    fn rotate_about_pivot() {
        let quarter = std::f32::consts::FRAC_PI_2;
        let t = scale_rotate_about(v(5.0, 5.0), Vec2::ONE, quarter);
        assert!(close(t.transform_point2(v(5.0, 5.0)), v(5.0, 5.0)));
        // +x turns towards +y (down on screen: clockwise).
        assert!(close(t.transform_point2(v(6.0, 5.0)), v(5.0, 6.0)));
        assert!(close(t.transform_point2(v(5.0, 6.0)), v(4.0, 5.0)));
        assert!((width_scale(&t) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn scale_happens_before_rotation() {
        // Stretch x by 2, then turn a quarter: a point right of the pivot
        // ends up twice as far below it.
        let t = scale_rotate_about(Vec2::ZERO, v(2.0, 1.0), std::f32::consts::FRAC_PI_2);
        assert!(close(t.transform_point2(v(1.0, 0.0)), v(0.0, 2.0)));
        assert!(close(t.transform_point2(v(0.0, 1.0)), v(-1.0, 0.0)));
    }

    #[test]
    fn transform_strokes_and_fills() {
        let s = line(v(0.0, 0.0), v(10.0, 0.0));
        let t = scale_rotate_about(v(5.0, 0.0), Vec2::splat(2.0), 0.0);
        let big = s.transformed(&t);
        assert_eq!(big.id, s.id);
        assert!(close(big.points[0].pos, v(-5.0, 0.0)));
        assert!(close(big.points[1].pos, v(15.0, 0.0)));
        assert!((big.style.width - 4.0).abs() < 1e-5, "width scales too");
        assert_eq!(big.points[1].pressure, s.points[1].pressure);

        let f = square(v(0.0, 0.0), 10.0);
        let moved = f.transformed(&t);
        assert_eq!(moved.id, f.id);
        let (min, max) = moved.bounds().unwrap();
        assert!(close(min, v(-5.0, 0.0)) && close(max, v(15.0, 20.0)));
    }

    #[test]
    fn mirroring_a_fill_keeps_its_winding() {
        use crate::fill::signed_area;
        let f = square(v(0.0, 0.0), 10.0);
        let flipped = f.transformed(&scale_rotate_about(v(5.0, 5.0), v(-1.0, 1.0), 0.0));
        assert_eq!(
            signed_area(&flipped.contours[0]).signum(),
            signed_area(&f.contours[0]).signum()
        );
        assert!(flipped.contains(v(2.0, 2.0)));
    }

    #[test]
    fn item_at_prefers_strokes_then_fills() {
        let (d, fill, across, far) = scene();
        assert_eq!(d.item_at(v(50.0, 50.0), 0.0), Some(across));
        assert_eq!(
            d.item_at(v(50.0, 53.0), 0.0),
            Some(fill),
            "just off the line"
        );
        assert_eq!(
            d.item_at(v(50.0, 53.0), 3.0),
            Some(across),
            "tolerance reaches"
        );
        assert_eq!(d.item_at(v(310.0, 300.5), 0.0), Some(far));
        assert_eq!(d.item_at(v(200.0, 200.0), 5.0), None);
        assert_eq!(VectorDrawing::new().item_at(v(0.0, 0.0), 5.0), None);
    }

    #[test]
    fn item_at_picks_the_topmost_stroke() {
        let mut d = VectorDrawing::new();
        let bottom = line(v(0.0, 0.0), v(10.0, 0.0));
        let top = line(v(5.0, -5.0), v(5.0, 5.0));
        let top_id = top.id;
        d.push_stroke(bottom);
        d.push_stroke(top);
        assert_eq!(d.item_at(v(5.0, 0.0), 0.0), Some(ItemId::Stroke(top_id)));
    }

    #[test]
    fn items_in_polygon_needs_a_point_inside() {
        let (d, fill, across, far) = scene();
        let rect = |min: Vec2, max: Vec2| [min, v(max.x, min.y), max, v(min.x, max.y)];
        assert_eq!(
            d.items_in_polygon(&rect(v(-50.0, 40.0), v(-5.0, 60.0))),
            vec![across]
        );
        assert!(
            d.items_in_polygon(&rect(v(20.0, 20.0), v(30.0, 30.0)))
                .is_empty(),
            "inside the fill, but none of its points"
        );
        assert_eq!(
            d.items_in_polygon(&rect(v(-5.0, -5.0), v(5.0, 5.0))),
            vec![fill],
            "a fill corner"
        );
        assert_eq!(
            d.items_in_polygon(&rect(v(-20.0, -20.0), v(400.0, 400.0))),
            vec![fill, across, far]
        );
        // A triangle around the far line's start only.
        assert_eq!(
            d.items_in_polygon(&[v(290.0, 290.0), v(310.0, 295.0), v(290.0, 310.0)]),
            vec![far]
        );
        assert!(d.items_in_polygon(&[]).is_empty());
    }

    #[test]
    fn bounds_and_lookup() {
        let (d, fill, across, far) = scene();
        assert_eq!(d.all_items(), vec![fill, across, far]);
        assert_eq!(d.item_bounds(fill), Some((v(0.0, 0.0), v(100.0, 100.0))));
        assert_eq!(
            d.item_bounds(across),
            Some((v(-11.0, 49.0), v(111.0, 51.0)))
        );
        assert_eq!(
            d.items_bounds(&[fill, across]),
            Some((v(-11.0, 0.0), v(111.0, 100.0)))
        );
        let missing = ItemId::Stroke(StrokeId::new());
        assert!(!d.contains_item(missing) && d.contains_item(far));
        assert_eq!(d.item_bounds(missing), None);
        assert_eq!(d.items_bounds(&[missing]), None);
        assert_eq!(d.items_bounds(&[missing, fill]), d.item_bounds(fill));
    }

    #[test]
    fn images_select_move_and_paint_under_fills() {
        use crate::image::{Image, ImageData};
        let (mut d, fill, _, _) = scene();
        let img = Image::new(
            ImageData::new(2, 2, vec![0]),
            v(50.0, 50.0),
            v(100.0, 100.0),
        );
        let id = ItemId::Image(img.id);
        d.push_image(img);
        assert_eq!(d.all_items()[0], id, "images paint first in a tier");
        assert_eq!(d.item_at(v(120.0, 120.0), 0.0), Some(id));
        assert_eq!(d.item_at(v(60.0, 60.0), 0.0), Some(fill), "fill is above");
        assert_eq!(d.item_bounds(id), Some((v(50.0, 50.0), v(150.0, 150.0))));
        let rect = [
            v(140.0, 140.0),
            v(160.0, 140.0),
            v(160.0, 160.0),
            v(140.0, 160.0),
        ];
        assert_eq!(d.items_in_polygon(&rect), vec![id]);
        d.translate(v(10.0, 0.0));
        assert_eq!(d.item_bounds(id), Some((v(60.0, 50.0), v(160.0, 150.0))));
        assert!(d.contains_item(id));
        let copy = d.duplicate();
        assert_eq!(copy.images().len(), 1);
        assert_ne!(copy.images()[0].id, d.images()[0].id);
    }

    #[test]
    fn mutable_lookup() {
        let (mut d, fill, across, _) = scene();
        let (ItemId::Stroke(sid), ItemId::Fill(fid)) = (across, fill) else {
            unreachable!()
        };
        d.stroke_mut(sid).unwrap().translate(v(1.0, 1.0));
        d.fill_mut(fid).unwrap().translate(v(1.0, 1.0));
        assert_eq!(d.item_bounds(fill), Some((v(1.0, 1.0), v(101.0, 101.0))));
        assert!(d.stroke_mut(StrokeId::new()).is_none());
        assert!(d.fill_mut(FillId::new()).is_none());
    }
}
