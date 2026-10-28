//! Fill commands: adding and removing filled areas (fill bucket, ADR-007).

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
            target: None,
        }
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
                vector_mut(doc, id)?.push_fill(self.fill.clone());
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
}
