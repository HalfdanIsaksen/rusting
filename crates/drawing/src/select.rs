//! Selection geometry: item ids, bounds, translation and hit-testing a point
//! or a rectangle against strokes and fills (the selection tool).
//!
//! Rectangles are `(min, max)` pairs in document coordinates, like
//! [`Stroke::bounds`].

use animation_core::{FillId, StrokeId, Vec2};

use crate::fill::Fill;
use crate::stroke::{Stroke, VectorDrawing};

/// One selectable thing in a drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemId {
    /// A stroke.
    Stroke(StrokeId),
    /// A fill.
    Fill(FillId),
}

impl From<StrokeId> for ItemId {
    fn from(id: StrokeId) -> Self {
        Self::Stroke(id)
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

impl Stroke {
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
    /// Moves every contour point by `delta`.
    pub fn translate(&mut self, delta: Vec2) {
        for point in self.contours.iter_mut().flatten() {
            *point += delta;
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

    /// Whether the drawing holds `item`.
    pub fn contains_item(&self, item: ItemId) -> bool {
        match item {
            ItemId::Stroke(id) => self.stroke(id).is_some(),
            ItemId::Fill(id) => self.fill(id).is_some(),
        }
    }

    /// Every item: fills bottom-to-top, then strokes bottom-to-top.
    pub fn all_items(&self) -> Vec<ItemId> {
        self.fills()
            .iter()
            .map(|f| ItemId::Fill(f.id))
            .chain(self.strokes().iter().map(|s| ItemId::Stroke(s.id)))
            .collect()
    }

    /// Bounds of one item (strokes include half the brush width). `None` if
    /// the item is missing or has no points.
    pub fn item_bounds(&self, item: ItemId) -> Option<(Vec2, Vec2)> {
        match item {
            ItemId::Stroke(id) => self.stroke(id)?.bounds(),
            ItemId::Fill(id) => self.fill(id)?.bounds(),
        }
    }

    /// Bounds of all `items` together; missing items are skipped.
    pub fn items_bounds(&self, items: &[ItemId]) -> Option<(Vec2, Vec2)> {
        items
            .iter()
            .filter_map(|&item| self.item_bounds(item))
            .reduce(rect_union)
    }

    /// The topmost item under `point`. Strokes are painted over fills, so they
    /// are checked first, top to bottom; `tolerance` (document units) widens
    /// strokes so thin lines are easy to click. Fills must contain the point.
    pub fn item_at(&self, point: Vec2, tolerance: f32) -> Option<ItemId> {
        self.strokes()
            .iter()
            .rev()
            .find(|s| s.hit_test(point, tolerance))
            .map(|s| ItemId::Stroke(s.id))
            .or_else(|| self.fill_at(point).map(ItemId::Fill))
    }

    /// Items whose bounds intersect the rectangle `(min, max)`, in the order
    /// of [`Self::all_items`].
    pub fn items_in_rect(&self, rect: (Vec2, Vec2)) -> Vec<ItemId> {
        self.all_items()
            .into_iter()
            .filter(|&item| {
                self.item_bounds(item)
                    .is_some_and(|b| rects_intersect(b, rect))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fill::FillShape;
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
    fn items_in_rect_uses_bounds() {
        let (d, fill, across, far) = scene();
        assert_eq!(
            d.items_in_rect((v(-50.0, 40.0), v(-5.0, 60.0))),
            vec![across]
        );
        assert_eq!(
            d.items_in_rect((v(20.0, 20.0), v(30.0, 30.0))),
            vec![fill],
            "inside the fill only"
        );
        assert_eq!(
            d.items_in_rect((v(0.0, 0.0), v(400.0, 400.0))),
            vec![fill, across, far]
        );
        assert!(
            d.items_in_rect((v(150.0, 150.0), v(200.0, 200.0)))
                .is_empty()
        );
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
