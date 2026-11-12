//! Merging same-colour strokes and fills into one fill (ADR-012).

use super::{editable_layer, vector_mut};
use crate::{Command, CommandError};
use animation_core::{DrawingId, FrameIndex, LayerId};
use document::Document;
use document::drawing::{Fill, ItemId, Stroke};

/// What [`MergeSameColour::apply`] changed, so undo can reverse exactly that.
#[derive(Debug)]
struct Applied {
    drawing: DrawingId,
    merged: Fill,
    /// Removed fills with the index each had when removed, in removal order.
    fills: Vec<(usize, Fill)>,
    /// Removed strokes with the index each had when removed, in removal order.
    strokes: Vec<(usize, Stroke)>,
}

/// Merges an item of the drawing a layer shows at a frame with every
/// stroke and fill of the same colour it touches, into one fill
/// (see [`VectorDrawing::merge_same_colour`](document::drawing::VectorDrawing::merge_same_colour)).
///
/// Run it right after adding the item, in the same [`Batch`](super::Batch),
/// so adding and merging are one undo step. If nothing merges (the item is
/// gone, has a gradient or touches nothing of its colour), the command does
/// nothing. The merged fill keeps the ID and place of the lowest fill it
/// replaces. Undo puts every original item back, with its ID, at its old
/// index. Fails on locked layers.
#[derive(Debug)]
pub struct MergeSameColour {
    layer: LayerId,
    frame: FrameIndex,
    item: ItemId,
    applied: Option<Applied>,
}

impl MergeSameColour {
    /// Creates the command for `item`.
    pub fn new(layer: LayerId, frame: FrameIndex, item: impl Into<ItemId>) -> Self {
        Self {
            layer,
            frame,
            item: item.into(),
            applied: None,
        }
    }

    /// The merged fill, once applied and if anything merged.
    pub fn merged(&self) -> Option<&Fill> {
        self.applied.as_ref().map(|a| &a.merged)
    }
}

impl Command for MergeSameColour {
    fn label(&self) -> &str {
        "Merge Same Colour"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        self.applied = None;
        let Some(drawing) = doc.drawing_at(self.layer, self.frame) else {
            return Ok(());
        };
        let vector = vector_mut(doc, drawing)?;
        let Some(merge) = vector.merge_same_colour(self.item) else {
            return Ok(());
        };
        let mut fills = Vec::with_capacity(merge.fills.len());
        for &id in &merge.fills {
            fills.push(
                vector
                    .remove_fill(id)
                    .ok_or(CommandError::DrawingNotFound)?,
            );
        }
        let mut strokes = Vec::with_capacity(merge.strokes.len());
        for &id in &merge.strokes {
            strokes.push(
                vector
                    .remove_stroke(id)
                    .ok_or(CommandError::DrawingNotFound)?,
            );
        }
        // The fills are removed lowest first, so the others were all above
        // the first and its index is still the right place.
        let index = fills.first().map_or(0, |(index, _)| *index);
        vector.insert_fill(index, merge.fill.clone());
        self.applied = Some(Applied {
            drawing,
            merged: merge.fill,
            fills,
            strokes,
        });
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let Some(applied) = self.applied.take() else {
            return Ok(());
        };
        let vector = vector_mut(doc, applied.drawing)?;
        vector
            .remove_fill(applied.merged.id)
            .ok_or(CommandError::DrawingNotFound)?;
        for (index, stroke) in applied.strokes.into_iter().rev() {
            vector.insert_stroke(index, stroke);
        }
        for (index, fill) in applied.fills.into_iter().rev() {
            vector.insert_fill(index, fill);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use crate::commands::{AddFill, AddStroke, Batch, SetLayerLocked};
    use animation_core::{Rgba, Vec2};
    use document::CanvasSize;
    use document::drawing::{BrushStyle, FillShape, StrokePoint, VectorDrawing};

    const BROWN: Rgba = Rgba {
        r: 0.4,
        g: 0.15,
        b: 0.12,
        a: 1.0,
    };

    fn setup() -> (Document, LayerId, History) {
        let mut doc = Document::new(
            "T",
            CanvasSize {
                width: 300,
                height: 300,
            },
        );
        let layer = doc.add_layer("L");
        (doc, layer, History::new())
    }

    fn square(color: Rgba) -> Fill {
        let (lo, hi) = (Vec2::splat(10.0), Vec2::splat(110.0));
        Fill::new(
            FillShape {
                contours: vec![vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)]],
            },
            color,
            1.0,
        )
    }

    fn stroke(color: Rgba) -> Stroke {
        Stroke::new(
            vec![
                StrokePoint::new(Vec2::new(90.0, 60.0)),
                StrokePoint::new(Vec2::new(160.0, 60.0)),
            ],
            BrushStyle {
                color,
                width: 10.0,
                opacity: 1.0,
            },
        )
    }

    fn drawing(doc: &Document, layer: LayerId) -> &VectorDrawing {
        let id = doc.drawing_at(layer, FrameIndex(0)).unwrap();
        doc.drawing(id).unwrap().as_vector().unwrap()
    }

    fn add_and_merge(layer: LayerId, stroke: Stroke) -> Box<Batch> {
        let id = stroke.id;
        Box::new(Batch::new(
            "Add Stroke",
            vec![
                Box::new(AddStroke::new(layer, FrameIndex(0), stroke)),
                Box::new(MergeSameColour::new(layer, FrameIndex(0), id)),
            ],
        ))
    }

    #[test]
    fn stroke_merges_into_fill_and_undo_restores_both() {
        let (mut doc, layer, mut h) = setup();
        let fill = square(BROWN);
        let fill_id = fill.id;
        h.execute(
            &mut doc,
            Box::new(AddFill::new(layer, FrameIndex(0), fill.clone())),
        )
        .unwrap();
        let before = doc.clone();

        h.execute(&mut doc, add_and_merge(layer, stroke(BROWN)))
            .unwrap();
        let merged = drawing(&doc, layer);
        assert!(merged.strokes().is_empty());
        assert_eq!(merged.fills().len(), 1);
        assert_eq!(merged.fills()[0].id, fill_id);
        assert!(merged.fills()[0].contains(Vec2::new(150.0, 60.0)));
        let after = doc.clone();

        h.undo(&mut doc).unwrap();
        assert_eq!(doc, before);
        assert_eq!(drawing(&doc, layer).fills()[0], fill);
        h.redo(&mut doc).unwrap();
        assert_eq!(doc, after);
    }

    #[test]
    fn other_colours_are_left_alone() {
        let (mut doc, layer, mut h) = setup();
        h.execute(
            &mut doc,
            Box::new(AddFill::new(layer, FrameIndex(0), square(BROWN))),
        )
        .unwrap();
        h.execute(&mut doc, add_and_merge(layer, stroke(Rgba::BLACK)))
            .unwrap();
        let d = drawing(&doc, layer);
        assert_eq!((d.fills().len(), d.strokes().len()), (1, 1));
        h.undo(&mut doc).unwrap();
        assert!(drawing(&doc, layer).strokes().is_empty());
    }

    #[test]
    fn merged_fill_keeps_its_place_among_fills() {
        let (mut doc, layer, mut h) = setup();
        let below = square(Rgba::WHITE);
        let target = square(BROWN);
        let above = square(Rgba::BLACK);
        let ids = [below.id, target.id, above.id];
        for fill in [below, target, above] {
            h.execute(&mut doc, Box::new(AddFill::new(layer, FrameIndex(0), fill)))
                .unwrap();
        }
        h.execute(&mut doc, add_and_merge(layer, stroke(BROWN)))
            .unwrap();
        let order: Vec<_> = drawing(&doc, layer).fills().iter().map(|f| f.id).collect();
        assert_eq!(order, ids);
    }

    #[test]
    fn locked_layer_fails() {
        let (mut doc, layer, mut h) = setup();
        let fill = square(BROWN);
        let id = fill.id;
        h.execute(&mut doc, Box::new(AddFill::new(layer, FrameIndex(0), fill)))
            .unwrap();
        h.execute(&mut doc, Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        let result = h.execute(
            &mut doc,
            Box::new(MergeSameColour::new(layer, FrameIndex(0), id)),
        );
        assert_eq!(result, Err(CommandError::LayerLocked));
    }
}
