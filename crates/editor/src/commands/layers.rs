//! Layer edits that copy a whole layer.

use crate::{Command, CommandError};
use animation_core::{DrawingId, LayerId};
use document::drawing::DrawingContent;
use document::scene::Layer;
use document::timeline::{Keyframe, Track};
use document::{Document, RemovedLayer};
use std::collections::BTreeMap;

/// The layer, track and drawings made by [`DuplicateLayer`].
#[derive(Debug, Clone)]
struct LayerCopy {
    layer: Layer,
    track: Track,
    drawings: Vec<(DrawingId, DrawingContent)>,
}

/// Duplicates a layer right above it: same properties, name with " copy",
/// and a track whose drawings are deep copies (new drawing, stroke and fill
/// IDs). Keys that share a drawing on the source share its copy.
#[derive(Debug)]
pub struct DuplicateLayer {
    source: LayerId,
    id: LayerId,
    /// Built on the first apply and reused on redo, so redo keeps the IDs.
    copy: Option<LayerCopy>,
}

impl DuplicateLayer {
    /// Creates the command. The copy's ID is fixed now so redo reuses it.
    pub fn new(source: LayerId) -> Self {
        Self {
            source,
            id: LayerId::new(),
            copy: None,
        }
    }

    /// ID of the layer this command adds.
    pub fn layer_id(&self) -> LayerId {
        self.id
    }

    fn build_copy(&self, doc: &Document) -> Result<LayerCopy, CommandError> {
        let source = doc
            .layers()
            .get(self.source)
            .ok_or(CommandError::LayerNotFound)?;
        let mut layer = source.clone();
        layer.id = self.id;
        layer.name = format!("{} copy", source.name);

        let mut copies: BTreeMap<DrawingId, DrawingId> = BTreeMap::new();
        let mut drawings = Vec::new();
        let mut track = Track::new();
        for (frame, key) in doc.track(self.source).into_iter().flat_map(Track::keys) {
            let key = match key {
                Keyframe::Drawing(id) => {
                    let copy = match copies.get(id) {
                        Some(&copy) => copy,
                        None => {
                            let content = doc
                                .drawing(*id)
                                .ok_or(CommandError::DrawingNotFound)?
                                .duplicate();
                            let copy = DrawingId::new();
                            copies.insert(*id, copy);
                            drawings.push((copy, content));
                            copy
                        }
                    };
                    Keyframe::Drawing(copy)
                }
                Keyframe::Empty => Keyframe::Empty,
            };
            track.set_key(frame, key);
        }
        Ok(LayerCopy {
            layer,
            track,
            drawings,
        })
    }
}

impl Command for DuplicateLayer {
    fn label(&self) -> &str {
        "Duplicate Layer"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let index = doc
            .layers()
            .index_of(self.source)
            .ok_or(CommandError::LayerNotFound)?;
        let copy = match self.copy.take() {
            Some(copy) => copy,
            None => self.build_copy(doc)?,
        };
        doc.restore_layer(RemovedLayer {
            index: index + 1,
            layer: copy.layer.clone(),
            track: copy.track.clone(),
        });
        for (id, content) in &copy.drawings {
            doc.insert_drawing(*id, content.clone());
        }
        self.copy = Some(copy);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        doc.remove_layer(self.id)
            .ok_or(CommandError::LayerNotFound)?;
        for (id, _) in self.copy.iter().flat_map(|c| &c.drawings) {
            doc.remove_drawing(*id)
                .ok_or(CommandError::DrawingNotFound)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use animation_core::{FrameIndex, Vec2};
    use document::CanvasSize;
    use document::drawing::{BrushStyle, Stroke, StrokePoint, VectorDrawing};

    fn drawing_with_stroke() -> DrawingContent {
        let mut v = VectorDrawing::new();
        v.push_stroke(Stroke::new(
            vec![
                StrokePoint::new(Vec2::ZERO),
                StrokePoint::new(Vec2::new(5.0, 5.0)),
            ],
            BrushStyle::default(),
        ));
        DrawingContent::Vector(v)
    }

    /// Layers A (bottom) and B; A has one drawing keyed at 0 and 4, and an
    /// empty key at 2.
    fn sample() -> (Document, LayerId, LayerId, DrawingId) {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        let a = d.add_layer("A");
        let b = d.add_layer("B");
        let drawing = d.add_drawing(drawing_with_stroke());
        let track = d.track_mut(a).unwrap();
        track.set_key(FrameIndex(0), Keyframe::Drawing(drawing));
        track.set_key(FrameIndex(2), Keyframe::Empty);
        track.set_key(FrameIndex(4), Keyframe::Drawing(drawing));
        d.layer_mut(a).unwrap().locked = true;
        (d, a, b, drawing)
    }

    #[test]
    fn copies_above_with_deep_drawings() {
        let (mut d, a, b, drawing) = sample();
        let mut cmd = DuplicateLayer::new(a);
        let copy = cmd.layer_id();
        cmd.apply(&mut d).unwrap();

        let order: Vec<_> = d.layers().iter().map(|l| l.id).collect();
        assert_eq!(order, [a, copy, b]);
        let layer = d.layers().get(copy).unwrap();
        assert_eq!(layer.name, "A copy");
        assert!(layer.locked);

        let keys: Vec<_> = d
            .track(copy)
            .unwrap()
            .keys()
            .map(|(f, k)| (f.0, *k))
            .collect();
        let Keyframe::Drawing(new) = keys[0].1 else {
            panic!("expected a drawing key");
        };
        assert_ne!(new, drawing);
        assert_eq!(
            keys,
            [
                (0, Keyframe::Drawing(new)),
                (2, Keyframe::Empty),
                (4, Keyframe::Drawing(new)),
            ]
        );
        let original = d.drawing(drawing).unwrap().as_vector().unwrap();
        let duplicate = d.drawing(new).unwrap().as_vector().unwrap();
        assert_ne!(original.strokes()[0].id, duplicate.strokes()[0].id);
        assert_eq!(original.strokes()[0].points, duplicate.strokes()[0].points);
    }

    #[test]
    fn undo_restores_and_redo_reuses_ids() {
        let (mut d, a, ..) = sample();
        let before = d.clone();
        let mut h = History::new();
        h.execute(&mut d, Box::new(DuplicateLayer::new(a))).unwrap();
        let after = d.clone();
        assert!(h.undo(&mut d).unwrap());
        assert_eq!(d, before);
        assert!(h.redo(&mut d).unwrap());
        assert_eq!(d, after, "redo keeps the copy's IDs");
    }

    #[test]
    fn missing_layer_fails_without_changes() {
        let (mut d, ..) = sample();
        let before = d.clone();
        assert_eq!(
            DuplicateLayer::new(LayerId::new()).apply(&mut d),
            Err(CommandError::LayerNotFound)
        );
        assert_eq!(d, before);
    }
}
