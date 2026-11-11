//! Fill commands: adding, removing and replacing filled areas (fill bucket
//! and eraser, ADR-007).

use super::{StrokeTarget, editable_layer, frame_in_range, vector_mut};
use crate::{Command, CommandError};
use animation_core::{DrawingId, FillId, FrameIndex, LayerId};
use document::Document;
use document::drawing::{DrawingContent, Fill, VectorDrawing};
use document::timeline::Keyframe;

/// Adds a fill to the drawing a layer shows at a frame.
///
/// Like [`AddStroke`](super::AddStroke): if the layer shows nothing at that
/// frame, a new drawing and a drawing keyframe are created, and undo removes
/// both. Fails on locked layers.
#[derive(Debug)]
pub struct AddFill {
    layer: LayerId,
    frame: FrameIndex,
    fill: Fill,
    new_drawing: DrawingId,
    behind: bool,
    target: Option<StrokeTarget>,
}

impl AddFill {
    /// Creates the command. The fill keeps its own ID; the ID of a drawing
    /// that may need creating is fixed now so redo reuses it.
    pub fn new(layer: LayerId, frame: FrameIndex, fill: Fill) -> Self {
        Self {
            layer,
            frame,
            fill,
            new_drawing: DrawingId::new(),
            behind: false,
            target: None,
        }
    }

    /// Puts the fill under the drawing's other fills of its tier instead of
    /// on top (ADR-010).
    pub fn behind(mut self, behind: bool) -> Self {
        self.behind = behind;
        self
    }

    /// ID of the fill this command adds.
    pub fn fill_id(&self) -> FillId {
        self.fill.id
    }
}

impl Command for AddFill {
    fn label(&self) -> &str {
        "Fill"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        frame_in_range(doc, self.frame)?;
        let target = match doc.drawing_at(self.layer, self.frame) {
            Some(id) => {
                vector_mut(doc, id)?.add_fill(self.fill.clone(), self.behind);
                StrokeTarget::Existing(id)
            }
            None => {
                let mut drawing = VectorDrawing::new();
                drawing.push_fill(self.fill.clone());
                let track = doc
                    .track_mut(self.layer)
                    .ok_or(CommandError::LayerNotFound)?;
                let replaced = track.set_key(self.frame, Keyframe::Drawing(self.new_drawing));
                doc.insert_drawing(self.new_drawing, DrawingContent::Vector(drawing));
                StrokeTarget::Created { replaced }
            }
        };
        self.target = Some(target);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        match self.target.take().ok_or(CommandError::DrawingNotFound)? {
            StrokeTarget::Existing(id) => {
                vector_mut(doc, id)?
                    .remove_fill(self.fill.id)
                    .ok_or(CommandError::DrawingNotFound)?;
            }
            StrokeTarget::Created { replaced } => {
                let track = doc
                    .track_mut(self.layer)
                    .ok_or(CommandError::LayerNotFound)?;
                match replaced {
                    Some(key) => track.set_key(self.frame, key),
                    None => track.remove_key(self.frame),
                };
                doc.remove_drawing(self.new_drawing)
                    .ok_or(CommandError::DrawingNotFound)?;
            }
        }
        Ok(())
    }
}

/// Removes a fill from the drawing a layer shows at a frame.
#[derive(Debug)]
pub struct RemoveFill {
    layer: LayerId,
    frame: FrameIndex,
    fill: FillId,
    removed: Option<(DrawingId, usize, Fill)>,
}

impl RemoveFill {
    /// Creates the command.
    pub fn new(layer: LayerId, frame: FrameIndex, fill: FillId) -> Self {
        Self {
            layer,
            frame,
            fill,
            removed: None,
        }
    }
}

impl Command for RemoveFill {
    fn label(&self) -> &str {
        "Remove Fill"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        let drawing = doc
            .drawing_at(self.layer, self.frame)
            .ok_or(CommandError::DrawingNotFound)?;
        let (index, fill) = vector_mut(doc, drawing)?
            .remove_fill(self.fill)
            .ok_or(CommandError::DrawingNotFound)?;
        self.removed = Some((drawing, index, fill));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let (drawing, index, fill) = self.removed.take().ok_or(CommandError::DrawingNotFound)?;
        vector_mut(doc, drawing)?.insert_fill(index, fill);
        Ok(())
    }
}

/// Replaces one fill with others at the same place in the fill order, e.g.
/// the pieces left after a partial erase. An empty replacement list removes
/// the fill.
///
/// Undo removes the replacements and puts back the exact original fill, with
/// its ID, at its old index. Fails on locked layers.
#[derive(Debug)]
pub struct ReplaceFill {
    layer: LayerId,
    frame: FrameIndex,
    original: FillId,
    replacements: Vec<Fill>,
    removed: Option<(DrawingId, usize, Fill)>,
}

impl ReplaceFill {
    /// Creates the command. The replacements keep their IDs, so redo
    /// recreates the same fills.
    pub fn new(
        layer: LayerId,
        frame: FrameIndex,
        original: FillId,
        replacements: Vec<Fill>,
    ) -> Self {
        Self {
            layer,
            frame,
            original,
            replacements,
            removed: None,
        }
    }

    /// ID of the fill this command replaces.
    pub fn original_id(&self) -> FillId {
        self.original
    }

    /// The fills that take the original's place, bottom to top.
    pub fn replacements(&self) -> &[Fill] {
        &self.replacements
    }
}

impl Command for ReplaceFill {
    fn label(&self) -> &str {
        "Replace Fill"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        let drawing = doc
            .drawing_at(self.layer, self.frame)
            .ok_or(CommandError::DrawingNotFound)?;
        let vector = vector_mut(doc, drawing)?;
        let (index, fill) = vector
            .remove_fill(self.original)
            .ok_or(CommandError::DrawingNotFound)?;
        for (offset, piece) in self.replacements.iter().enumerate() {
            vector.insert_fill(index + offset, piece.clone());
        }
        self.removed = Some((drawing, index, fill));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let (drawing, index, fill) = self.removed.take().ok_or(CommandError::DrawingNotFound)?;
        let vector = vector_mut(doc, drawing)?;
        for piece in &self.replacements {
            vector
                .remove_fill(piece.id)
                .ok_or(CommandError::DrawingNotFound)?;
        }
        vector.insert_fill(index, fill);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use crate::commands::{AddStroke, SetLayerLocked};
    use animation_core::{Rgba, Vec2};
    use document::CanvasSize;
    use document::drawing::{BrushStyle, FillShape, Stroke, StrokePoint};

    fn square() -> Fill {
        Fill::new(
            FillShape {
                contours: vec![vec![
                    Vec2::ZERO,
                    Vec2::new(10.0, 0.0),
                    Vec2::splat(10.0),
                    Vec2::new(0.0, 10.0),
                ]],
            },
            Rgba::WHITE,
            1.0,
        )
    }

    fn setup() -> (Document, LayerId) {
        let mut doc = Document::new(
            "T",
            CanvasSize {
                width: 100,
                height: 100,
            },
        );
        let layer = doc.add_layer("L");
        (doc, layer)
    }

    fn fills(doc: &Document, layer: LayerId, frame: u32) -> usize {
        doc.drawing_at(layer, FrameIndex(frame))
            .and_then(|id| doc.drawing(id))
            .and_then(|d| d.as_vector())
            .map_or(0, |d| d.fills().len())
    }

    #[test]
    fn add_fill_creates_drawing_and_undoes_exactly() {
        let (mut doc, layer) = setup();
        let before = doc.clone();
        let mut history = History::new();
        history
            .execute(
                &mut doc,
                Box::new(AddFill::new(layer, FrameIndex(3), square())),
            )
            .unwrap();
        assert_eq!(fills(&doc, layer, 3), 1);
        let after = doc.clone();
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, before);
        history.redo(&mut doc).unwrap();
        assert_eq!(doc, after);
    }

    #[test]
    fn add_fill_goes_into_existing_drawing_under_strokes() {
        let (mut doc, layer) = setup();
        let mut history = History::new();
        let stroke = Stroke::new(
            vec![StrokePoint::new(Vec2::ZERO), StrokePoint::new(Vec2::ONE)],
            BrushStyle::default(),
        );
        history
            .execute(
                &mut doc,
                Box::new(AddStroke::new(layer, FrameIndex(0), stroke)),
            )
            .unwrap();
        let fill = square();
        let id = fill.id;
        history
            .execute(&mut doc, Box::new(AddFill::new(layer, FrameIndex(0), fill)))
            .unwrap();
        let drawing = doc.drawing_at(layer, FrameIndex(0)).unwrap();
        let v = doc.drawing(drawing).unwrap().as_vector().unwrap();
        assert_eq!((v.strokes().len(), v.fills().len()), (1, 1));

        let with_fill = doc.clone();
        history
            .execute(
                &mut doc,
                Box::new(RemoveFill::new(layer, FrameIndex(0), id)),
            )
            .unwrap();
        assert_eq!(fills(&doc, layer, 0), 0);
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, with_fill);
    }

    #[test]
    fn add_fill_behind_goes_under_other_fills_and_undoes() {
        let (mut doc, layer) = setup();
        let mut history = History::new();
        let (top, bottom) = (square(), square());
        let ids = [bottom.id, top.id];
        history
            .execute(&mut doc, Box::new(AddFill::new(layer, FrameIndex(0), top)))
            .unwrap();
        let before = doc.clone();
        history
            .execute(
                &mut doc,
                Box::new(AddFill::new(layer, FrameIndex(0), bottom).behind(true)),
            )
            .unwrap();
        let drawing = doc.drawing_at(layer, FrameIndex(0)).unwrap();
        let v = doc.drawing(drawing).unwrap().as_vector().unwrap();
        let order: Vec<_> = v.fills().iter().map(|f| f.id).collect();
        assert_eq!(order, ids);
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, before);
    }

    #[test]
    fn locked_layer_refuses_fills() {
        let (mut doc, layer) = setup();
        let mut history = History::new();
        history
            .execute(&mut doc, Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        let err = history
            .execute(
                &mut doc,
                Box::new(AddFill::new(layer, FrameIndex(0), square())),
            )
            .unwrap_err();
        assert_eq!(err, CommandError::LayerLocked);
        assert_eq!(fills(&doc, layer, 0), 0);
    }

    #[test]
    fn replace_fill_keeps_order_and_undoes_exactly() {
        let (mut doc, layer) = setup();
        let mut history = History::new();
        let (a, b) = (square(), square());
        let (a_id, b_id) = (a.id, b.id);
        for fill in [a, b] {
            history
                .execute(&mut doc, Box::new(AddFill::new(layer, FrameIndex(0), fill)))
                .unwrap();
        }
        let before = doc.clone();
        let (p, q) = (square(), square());
        let (p_id, q_id) = (p.id, q.id);
        history
            .execute(
                &mut doc,
                Box::new(ReplaceFill::new(layer, FrameIndex(0), a_id, vec![p, q])),
            )
            .unwrap();
        let drawing = doc.drawing_at(layer, FrameIndex(0)).unwrap();
        let ids: Vec<FillId> = doc
            .drawing(drawing)
            .unwrap()
            .as_vector()
            .unwrap()
            .fills()
            .iter()
            .map(|f| f.id)
            .collect();
        assert_eq!(ids, [p_id, q_id, b_id], "pieces stay under the other fill");
        let after = doc.clone();
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, before);
        history.redo(&mut doc).unwrap();
        assert_eq!(doc, after);
        // An empty list removes the fill.
        history
            .execute(
                &mut doc,
                Box::new(ReplaceFill::new(layer, FrameIndex(0), b_id, Vec::new())),
            )
            .unwrap();
        assert_eq!(fills(&doc, layer, 0), 2);
        // Locked layers refuse.
        history
            .execute(&mut doc, Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        let err = history
            .execute(
                &mut doc,
                Box::new(ReplaceFill::new(layer, FrameIndex(0), p_id, Vec::new())),
            )
            .unwrap_err();
        assert_eq!(err, CommandError::LayerLocked);
    }
}
