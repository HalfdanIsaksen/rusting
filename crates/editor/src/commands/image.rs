//! Image commands: placing and removing imported pictures (ADR-015).

use super::{StrokeTarget, editable_layer, frame_in_range, vector_mut};
use crate::{Command, CommandError};
use animation_core::{DrawingId, FrameIndex, ImageId, LayerId};
use document::Document;
use document::drawing::{DrawingContent, Image, VectorDrawing};
use document::timeline::Keyframe;

/// Places an imported picture in the drawing a layer shows at a frame, on
/// top of the other images of its tier (images paint under fills and
/// strokes of the same tier).
///
/// Like [`AddStroke`](super::AddStroke): if the layer shows nothing at that
/// frame, a new drawing and a drawing keyframe are created, and undo removes
/// both. Fails on locked or hidden layers.
#[derive(Debug)]
pub struct AddImage {
    layer: LayerId,
    frame: FrameIndex,
    image: Image,
    new_drawing: DrawingId,
    target: Option<StrokeTarget>,
}

impl AddImage {
    /// Creates the command. The image keeps its own ID; the ID of a drawing
    /// that may need creating is fixed now so redo reuses it.
    pub fn new(layer: LayerId, frame: FrameIndex, image: Image) -> Self {
        Self {
            layer,
            frame,
            image,
            new_drawing: DrawingId::new(),
            target: None,
        }
    }

    /// ID of the image this command adds.
    pub fn image_id(&self) -> ImageId {
        self.image.id
    }
}

impl Command for AddImage {
    fn label(&self) -> &str {
        "Import Image"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        frame_in_range(doc, self.frame)?;
        let target = match doc.drawing_at(self.layer, self.frame) {
            Some(id) => {
                vector_mut(doc, id)?.push_image(self.image.clone());
                StrokeTarget::Existing(id)
            }
            None => {
                let mut drawing = VectorDrawing::new();
                drawing.push_image(self.image.clone());
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
                    .remove_image(self.image.id)
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

/// Removes an image from the drawing a layer shows at a frame. Undo puts it
/// back at its old place.
#[derive(Debug)]
pub struct RemoveImage {
    layer: LayerId,
    frame: FrameIndex,
    image: ImageId,
    removed: Option<(DrawingId, usize, Image)>,
}

impl RemoveImage {
    /// Creates the command.
    pub fn new(layer: LayerId, frame: FrameIndex, image: ImageId) -> Self {
        Self {
            layer,
            frame,
            image,
            removed: None,
        }
    }
}

impl Command for RemoveImage {
    fn label(&self) -> &str {
        "Remove Image"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        let drawing = doc
            .drawing_at(self.layer, self.frame)
            .ok_or(CommandError::DrawingNotFound)?;
        let (index, image) = vector_mut(doc, drawing)?
            .remove_image(self.image)
            .ok_or(CommandError::DrawingNotFound)?;
        self.removed = Some((drawing, index, image));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let (drawing, index, image) = self.removed.take().ok_or(CommandError::DrawingNotFound)?;
        vector_mut(doc, drawing)?.insert_image(index, image);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use crate::commands::{AddStroke, SetLayerLocked};
    use animation_core::Vec2;
    use document::CanvasSize;
    use document::drawing::{BrushStyle, ImageData, Stroke, StrokePoint};

    fn doc() -> (Document, LayerId) {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 100,
                height: 100,
            },
        );
        let layer = d.add_layer("L");
        (d, layer)
    }

    fn picture() -> Image {
        Image::new(
            ImageData::new(2, 2, vec![1, 2, 3]),
            Vec2::ZERO,
            Vec2::splat(50.0),
        )
    }

    fn images(d: &Document, layer: LayerId, frame: u32) -> Vec<ImageId> {
        d.drawing_at(layer, FrameIndex(frame))
            .and_then(|id| d.drawing(id)?.as_vector())
            .map(|v| v.images().iter().map(|i| i.id).collect())
            .unwrap_or_default()
    }

    #[test]
    fn import_on_empty_frame_creates_a_key_and_undo_removes_it() {
        let (mut d, layer) = doc();
        let mut h = History::new();
        let img = picture();
        let id = img.id;
        h.execute(&mut d, Box::new(AddImage::new(layer, FrameIndex(2), img)))
            .unwrap();
        assert_eq!(images(&d, layer, 2), [id]);
        h.undo(&mut d).unwrap();
        assert!(d.drawing_at(layer, FrameIndex(2)).is_none());
        h.redo(&mut d).unwrap();
        assert_eq!(images(&d, layer, 2), [id]);
    }

    #[test]
    fn import_into_existing_drawing_paints_under_its_strokes() {
        let (mut d, layer) = doc();
        let mut h = History::new();
        let stroke = Stroke::new(
            vec![StrokePoint::new(Vec2::ZERO), StrokePoint::new(Vec2::ONE)],
            BrushStyle::default(),
        );
        h.execute(
            &mut d,
            Box::new(AddStroke::new(layer, FrameIndex(0), stroke)),
        )
        .unwrap();
        let img = picture();
        let id = img.id;
        h.execute(&mut d, Box::new(AddImage::new(layer, FrameIndex(0), img)))
            .unwrap();
        let drawing = d.drawing_at(layer, FrameIndex(0)).unwrap();
        let vector = d.drawing(drawing).unwrap().as_vector().unwrap();
        assert_eq!(vector.all_items()[0], id.into());
        assert_eq!(vector.strokes().len(), 1);

        h.execute(&mut d, Box::new(RemoveImage::new(layer, FrameIndex(0), id)))
            .unwrap();
        assert!(images(&d, layer, 0).is_empty());
        h.undo(&mut d).unwrap();
        assert_eq!(images(&d, layer, 0), [id]);
        h.undo(&mut d).unwrap();
        assert!(images(&d, layer, 0).is_empty());
        assert!(d.drawing_at(layer, FrameIndex(0)).is_some(), "stroke stays");
    }

    #[test]
    fn locked_layer_refuses_import() {
        let (mut d, layer) = doc();
        let mut h = History::new();
        h.execute(&mut d, Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        let err = h
            .execute(
                &mut d,
                Box::new(AddImage::new(layer, FrameIndex(0), picture())),
            )
            .unwrap_err();
        assert!(matches!(err, CommandError::LayerLocked));
        assert!(images(&d, layer, 0).is_empty());
    }
}
