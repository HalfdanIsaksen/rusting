//! Selection commands: moving, transforming and deleting selected strokes
//! and fills.

use super::{Batch, RemoveStroke, editable_layer, vector_mut};
use crate::commands::RemoveFill;
use crate::{Command, CommandError};
use animation_core::{DrawingId, FrameIndex, LayerId, Vec2};
use document::Document;
use document::drawing::select::Affine2;
use document::drawing::{Fill, ItemId, Stroke};

/// Moves strokes and fills of the drawing a layer shows at a frame by one
/// offset, keeping their IDs and stacking order.
///
/// Undo puts back the exact original geometry (not a reverse move), so
/// positions are restored bit for bit. Fails, without changing anything, if
/// the layer is locked or hidden or any item is missing.
#[derive(Debug)]
pub struct TranslateItems {
    layer: LayerId,
    frame: FrameIndex,
    items: Vec<ItemId>,
    delta: Vec2,
    originals: Option<Originals>,
}

impl TranslateItems {
    /// Creates the command.
    pub fn new(layer: LayerId, frame: FrameIndex, items: Vec<ItemId>, delta: Vec2) -> Self {
        Self {
            layer,
            frame,
            items,
            delta,
            originals: None,
        }
    }

    /// The items this command moves.
    pub fn items(&self) -> &[ItemId] {
        &self.items
    }

    /// The offset, in document units.
    pub fn delta(&self) -> Vec2 {
        self.delta
    }
}

impl Command for TranslateItems {
    fn label(&self) -> &str {
        "Move"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let delta = self.delta;
        self.originals = Some(edit_items(
            doc,
            self.layer,
            self.frame,
            &self.items,
            |s| s.translate(delta),
            |f| f.translate(delta),
        )?);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        restore_items(doc, self.originals.take())
    }
}

/// Scales, rotates or otherwise maps strokes and fills of the drawing a layer
/// shows at a frame through one affine transform (see
/// [`document::drawing::select::scale_rotate_about`]), keeping their IDs and
/// stacking order. Brush widths scale with the transform's average scale.
///
/// Like [`TranslateItems`], undo puts back the exact original geometry, and a
/// locked or hidden layer or a missing item fails without changing anything.
#[derive(Debug)]
pub struct TransformItems {
    label: &'static str,
    layer: LayerId,
    frame: FrameIndex,
    items: Vec<ItemId>,
    transform: Affine2,
    originals: Option<Originals>,
}

impl TransformItems {
    /// Creates the command, labelled "Transform".
    pub fn new(layer: LayerId, frame: FrameIndex, items: Vec<ItemId>, transform: Affine2) -> Self {
        Self {
            label: "Transform",
            layer,
            frame,
            items,
            transform,
            originals: None,
        }
    }

    /// Sets the undo label, e.g. "Scale" or "Rotate".
    pub fn with_label(mut self, label: &'static str) -> Self {
        self.label = label;
        self
    }

    /// The items this command transforms.
    pub fn items(&self) -> &[ItemId] {
        &self.items
    }

    /// The transform, in document coordinates.
    pub fn transform(&self) -> Affine2 {
        self.transform
    }
}

impl Command for TransformItems {
    fn label(&self) -> &str {
        self.label
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let t = self.transform;
        self.originals = Some(edit_items(
            doc,
            self.layer,
            self.frame,
            &self.items,
            |s| s.transform(&t),
            |f| f.transform(&t),
        )?);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        restore_items(doc, self.originals.take())
    }
}

/// The drawing an edit touched and copies of its items from before.
type Originals = (DrawingId, Vec<Stroke>, Vec<Fill>);

/// Applies `stroke` / `fill` to each item of the drawing `layer` shows at
/// `frame` and returns the originals for undo. Nothing changes on error.
fn edit_items(
    doc: &mut Document,
    layer: LayerId,
    frame: FrameIndex,
    items: &[ItemId],
    mut stroke: impl FnMut(&mut Stroke),
    mut fill: impl FnMut(&mut Fill),
) -> Result<Originals, CommandError> {
    editable_layer(doc, layer)?;
    let drawing = doc
        .drawing_at(layer, frame)
        .ok_or(CommandError::DrawingNotFound)?;
    let vector = vector_mut(doc, drawing)?;
    // Collect first so a missing item leaves the drawing untouched.
    let mut strokes = Vec::new();
    let mut fills = Vec::new();
    for &item in items {
        match item {
            ItemId::Stroke(id) => strokes.push(
                vector
                    .stroke(id)
                    .ok_or(CommandError::DrawingNotFound)?
                    .clone(),
            ),
            ItemId::Fill(id) => fills.push(
                vector
                    .fill(id)
                    .ok_or(CommandError::DrawingNotFound)?
                    .clone(),
            ),
        }
    }
    for original in &strokes {
        if let Some(s) = vector.stroke_mut(original.id) {
            stroke(s);
        }
    }
    for original in &fills {
        if let Some(f) = vector.fill_mut(original.id) {
            fill(f);
        }
    }
    Ok((drawing, strokes, fills))
}

/// Puts back the items [`edit_items`] saved.
fn restore_items(doc: &mut Document, originals: Option<Originals>) -> Result<(), CommandError> {
    let (drawing, strokes, fills) = originals.ok_or(CommandError::DrawingNotFound)?;
    let vector = vector_mut(doc, drawing)?;
    for original in strokes {
        let slot = vector
            .stroke_mut(original.id)
            .ok_or(CommandError::DrawingNotFound)?;
        *slot = original;
    }
    for original in fills {
        let slot = vector
            .fill_mut(original.id)
            .ok_or(CommandError::DrawingNotFound)?;
        *slot = original;
    }
    Ok(())
}

/// One undo step that deletes `items` from the drawing a layer shows at a
/// frame. Undo puts each item back at its old place in the stacking order.
pub fn remove_items(layer: LayerId, frame: FrameIndex, items: &[ItemId]) -> Batch {
    let commands = items
        .iter()
        .map(|&item| -> Box<dyn Command> {
            match item {
                ItemId::Stroke(id) => Box::new(RemoveStroke::new(layer, frame, id)),
                ItemId::Fill(id) => Box::new(RemoveFill::new(layer, frame, id)),
            }
        })
        .collect();
    Batch::new("Delete", commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use crate::commands::{AddFill, AddStroke, SetLayerLocked};
    use animation_core::Rgba;
    use document::CanvasSize;
    use document::drawing::{BrushStyle, FillShape, StrokePoint, VectorDrawing};

    fn setup() -> (Document, History, LayerId, ItemId, ItemId, ItemId) {
        let mut doc = Document::new(
            "T",
            CanvasSize {
                width: 200,
                height: 200,
            },
        );
        let layer = doc.add_layer("L");
        let mut h = History::new();
        let stroke = |y: f32| {
            Stroke::new(
                vec![
                    StrokePoint::new(Vec2::new(10.1, y)),
                    StrokePoint {
                        pos: Vec2::new(50.3, y + 0.7),
                        pressure: 0.4,
                    },
                ],
                BrushStyle::default(),
            )
        };
        let (a, b) = (stroke(10.0), stroke(40.0));
        let fill = Fill::new(
            FillShape {
                contours: vec![vec![
                    Vec2::new(0.0, 0.0),
                    Vec2::new(30.0, 0.0),
                    Vec2::new(30.0, 30.0),
                ]],
            },
            Rgba::BLACK,
            1.0,
        );
        let ids = (
            ItemId::Stroke(a.id),
            ItemId::Stroke(b.id),
            ItemId::Fill(fill.id),
        );
        h.execute(&mut doc, Box::new(AddStroke::new(layer, FrameIndex(0), a)))
            .unwrap();
        h.execute(&mut doc, Box::new(AddStroke::new(layer, FrameIndex(0), b)))
            .unwrap();
        h.execute(&mut doc, Box::new(AddFill::new(layer, FrameIndex(0), fill)))
            .unwrap();
        (doc, h, layer, ids.0, ids.1, ids.2)
    }

    fn drawing(doc: &Document, layer: LayerId) -> &VectorDrawing {
        let id = doc.drawing_at(layer, FrameIndex(0)).unwrap();
        doc.drawing(id).unwrap().as_vector().unwrap()
    }

    #[test]
    fn translate_moves_only_the_items_and_undo_is_exact() {
        let (mut doc, mut h, layer, a, b, fill) = setup();
        let before = doc.clone();
        // An awkward offset, so `x + d - d != x` in floating point.
        let delta = Vec2::new(0.1, -0.37);
        h.execute(
            &mut doc,
            Box::new(TranslateItems::new(
                layer,
                FrameIndex(0),
                vec![a, fill],
                delta,
            )),
        )
        .unwrap();
        let d = drawing(&doc, layer);
        let old = drawing(&before, layer);
        assert_eq!(
            d.item_bounds(a).unwrap().0,
            old.item_bounds(a).unwrap().0 + delta
        );
        assert_eq!(
            d.item_bounds(fill).unwrap().0,
            old.item_bounds(fill).unwrap().0 + delta
        );
        assert_eq!(d.item_bounds(b), old.item_bounds(b), "b untouched");
        assert_eq!(d.all_items(), old.all_items(), "order and ids kept");
        let after = doc.clone();

        h.undo(&mut doc).unwrap();
        assert_eq!(doc, before);
        h.redo(&mut doc).unwrap();
        assert_eq!(doc, after);
        assert_eq!(h.undo_label(), Some("Move"));
    }

    #[test]
    fn translate_with_missing_item_changes_nothing() {
        let (mut doc, mut h, layer, a, _, _) = setup();
        let before = doc.clone();
        let missing = ItemId::Stroke(animation_core::StrokeId::new());
        let err = h
            .execute(
                &mut doc,
                Box::new(TranslateItems::new(
                    layer,
                    FrameIndex(0),
                    vec![a, missing],
                    Vec2::ONE,
                )),
            )
            .unwrap_err();
        assert_eq!(err, CommandError::DrawingNotFound);
        assert_eq!(doc, before);
    }

    #[test]
    fn translate_on_locked_layer_fails() {
        let (mut doc, mut h, layer, a, _, _) = setup();
        h.execute(&mut doc, Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        let before = doc.clone();
        let err = h
            .execute(
                &mut doc,
                Box::new(TranslateItems::new(
                    layer,
                    FrameIndex(0),
                    vec![a],
                    Vec2::ONE,
                )),
            )
            .unwrap_err();
        assert_eq!(err, CommandError::LayerLocked);
        assert_eq!(doc, before);
    }

    #[test]
    fn transform_scales_and_rotates_and_undo_is_exact() {
        use document::drawing::select::scale_rotate_about;
        let (mut doc, mut h, layer, a, b, fill) = setup();
        let before = doc.clone();
        let old = drawing(&before, layer);
        let pivot = Vec2::new(17.3, 9.1);
        let t = scale_rotate_about(pivot, Vec2::new(1.7, 0.6), 0.37);
        h.execute(
            &mut doc,
            Box::new(
                TransformItems::new(layer, FrameIndex(0), vec![a, fill], t).with_label("Rotate"),
            ),
        )
        .unwrap();
        assert_eq!(h.undo_label(), Some("Rotate"));
        let d = drawing(&doc, layer);
        let ItemId::Stroke(sid) = a else {
            unreachable!()
        };
        let (was, now) = (old.stroke(sid).unwrap(), d.stroke(sid).unwrap());
        for (p, q) in was.points.iter().zip(&now.points) {
            assert!((t.transform_point2(p.pos) - q.pos).length() < 1e-4);
            assert_eq!(p.pressure, q.pressure);
        }
        let expected_width = was.style.width * (1.7f32 * 0.6).sqrt();
        assert!((now.style.width - expected_width).abs() < 1e-4);
        let ItemId::Fill(fid) = fill else {
            unreachable!()
        };
        assert_eq!(d.fill(fid), Some(&old.fill(fid).unwrap().transformed(&t)));
        assert_eq!(d.item_bounds(b), old.item_bounds(b), "b untouched");
        assert_eq!(d.all_items(), old.all_items(), "order and ids kept");
        let after = doc.clone();

        h.undo(&mut doc).unwrap();
        assert_eq!(doc, before, "undo restores bit for bit");
        h.redo(&mut doc).unwrap();
        assert_eq!(doc, after);
    }

    #[test]
    fn transform_with_missing_item_or_locked_layer_changes_nothing() {
        let (mut doc, mut h, layer, a, _, _) = setup();
        let before = doc.clone();
        let t = Affine2::from_scale(Vec2::splat(2.0));
        let missing = ItemId::Fill(animation_core::FillId::new());
        let err = h
            .execute(
                &mut doc,
                Box::new(TransformItems::new(
                    layer,
                    FrameIndex(0),
                    vec![a, missing],
                    t,
                )),
            )
            .unwrap_err();
        assert_eq!(err, CommandError::DrawingNotFound);
        assert_eq!(doc, before);

        h.execute(&mut doc, Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        let before = doc.clone();
        let err = h
            .execute(
                &mut doc,
                Box::new(TransformItems::new(layer, FrameIndex(0), vec![a], t)),
            )
            .unwrap_err();
        assert_eq!(err, CommandError::LayerLocked);
        assert_eq!(doc, before);
    }

    #[test]
    fn remove_items_is_one_step_and_restores_order() {
        let (mut doc, mut h, layer, a, b, fill) = setup();
        let before = doc.clone();
        let undo_len = h.undo_len();
        h.execute(
            &mut doc,
            Box::new(remove_items(layer, FrameIndex(0), &[a, fill])),
        )
        .unwrap();
        assert_eq!(drawing(&doc, layer).all_items(), vec![b]);
        assert_eq!(h.undo_len(), undo_len + 1);
        assert_eq!(h.undo_label(), Some("Delete"));
        h.undo(&mut doc).unwrap();
        assert_eq!(doc, before);
    }
}
