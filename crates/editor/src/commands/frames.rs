//! Frame-level timeline edits that act on every layer at once.

use super::frame_in_range;
use crate::{Command, CommandError};
use animation_core::{DrawingId, FrameIndex, LayerId};
use document::Document;
use document::drawing::DrawingContent;
use document::timeline::Keyframe;

/// One layer's copy made by [`DuplicateFrame`].
#[derive(Debug, Clone)]
struct LayerCopy {
    layer: LayerId,
    drawing: DrawingId,
    content: DrawingContent,
}

/// Duplicates the frame at the playhead: a new frame is inserted right after
/// it on every layer, and the animation grows by one frame.
///
/// Tracks hold keyframes with stepped exposure, so on each layer:
/// - if a drawing is exposed at the frame, the new frame gets a keyframe with
///   a deep copy of it (new drawing ID, new stroke and fill IDs);
/// - otherwise (an empty key or nothing yet) the inserted frame simply
///   lengthens what is already there, which is the same picture.
///
/// Keys after the frame move one step right. Locked layers are included too,
/// so all layers stay in sync; the lock only guards drawing edits.
#[derive(Debug)]
pub struct DuplicateFrame {
    frame: FrameIndex,
    /// Built on the first apply and reused on redo, so redo keeps the IDs.
    copies: Option<Vec<LayerCopy>>,
}

impl DuplicateFrame {
    /// Creates the command for the frame at `frame`.
    pub fn new(frame: FrameIndex) -> Self {
        Self {
            frame,
            copies: None,
        }
    }

    /// The frame this command inserts (right after the duplicated one).
    pub fn new_frame(&self) -> FrameIndex {
        FrameIndex(self.frame.0.saturating_add(1))
    }

    /// The new drawing made for `layer`, once the command has been applied.
    pub fn copy_on(&self, layer: LayerId) -> Option<DrawingId> {
        self.copies
            .as_ref()?
            .iter()
            .find(|c| c.layer == layer)
            .map(|c| c.drawing)
    }

    fn build_copies(&self, doc: &Document) -> Result<Vec<LayerCopy>, CommandError> {
        let mut copies = Vec::new();
        for &layer in doc.tracks().keys() {
            if let Some(source) = doc.drawing_at(layer, self.frame) {
                let content = doc
                    .drawing(source)
                    .ok_or(CommandError::DrawingNotFound)?
                    .duplicate();
                copies.push(LayerCopy {
                    layer,
                    drawing: DrawingId::new(),
                    content,
                });
            }
        }
        Ok(copies)
    }
}

impl Command for DuplicateFrame {
    fn label(&self) -> &str {
        "Duplicate Frame"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        frame_in_range(doc, self.frame)?;
        let new = self
            .frame
            .0
            .checked_add(1)
            .ok_or(CommandError::FrameLimit)?;
        let frame_count = doc
            .frame_count
            .checked_add(1)
            .ok_or(CommandError::FrameLimit)?;
        // Check every track first so a failure leaves the document unchanged.
        let overflows = doc.tracks().values().any(|track| {
            track
                .keys()
                .last()
                .is_some_and(|(last, _)| last.0 >= new && last.0 == u32::MAX)
        });
        if overflows {
            return Err(CommandError::FrameLimit);
        }
        let copies = match self.copies.take() {
            Some(copies) => copies,
            None => self.build_copies(doc)?,
        };

        let new = FrameIndex(new);
        let layers: Vec<LayerId> = doc.tracks().keys().copied().collect();
        for layer in layers {
            if let Some(track) = doc.track_mut(layer) {
                track
                    .insert_frames(new, 1)
                    .map_err(|_| CommandError::FrameLimit)?;
            }
        }
        for copy in &copies {
            if let Some(track) = doc.track_mut(copy.layer) {
                track.set_key(new, Keyframe::Drawing(copy.drawing));
            }
            doc.insert_drawing(copy.drawing, copy.content.clone());
        }
        doc.frame_count = frame_count;
        self.copies = Some(copies);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let new = self.new_frame();
        for copy in self.copies.iter().flatten() {
            doc.track_mut(copy.layer)
                .ok_or(CommandError::LayerNotFound)?
                .remove_key(new)
                .ok_or(CommandError::KeyframeNotFound(new))?;
            doc.remove_drawing(copy.drawing)
                .ok_or(CommandError::DrawingNotFound)?;
        }
        let layers: Vec<LayerId> = doc.tracks().keys().copied().collect();
        for layer in layers {
            if let Some(track) = doc.track_mut(layer) {
                track.remove_frames(new, 1);
            }
        }
        doc.frame_count = doc.frame_count.saturating_sub(1);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use animation_core::Vec2;
    use document::CanvasSize;
    use document::drawing::{BrushStyle, Stroke, StrokePoint, VectorDrawing};

    fn doc() -> Document {
        Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        )
    }

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

    fn key_frames(d: &Document, layer: LayerId) -> Vec<u32> {
        d.track(layer).unwrap().keys().map(|(f, _)| f.0).collect()
    }

    /// Layer `a` has drawings at 0 and 3 and an empty key at 6; layer `b`
    /// has one drawing at 2; layer `c` is empty.
    fn sample() -> (Document, LayerId, LayerId, LayerId, DrawingId) {
        let mut d = doc();
        let a = d.add_layer("A");
        let b = d.add_layer("B");
        let c = d.add_layer("C");
        let a0 = d.add_drawing(drawing_with_stroke());
        let a3 = d.add_drawing(drawing_with_stroke());
        let b2 = d.add_drawing(drawing_with_stroke());
        let ta = d.track_mut(a).unwrap();
        ta.set_key(FrameIndex(0), Keyframe::Drawing(a0));
        ta.set_key(FrameIndex(3), Keyframe::Drawing(a3));
        ta.set_key(FrameIndex(6), Keyframe::Empty);
        d.track_mut(b)
            .unwrap()
            .set_key(FrameIndex(2), Keyframe::Drawing(b2));
        (d, a, b, c, a3)
    }

    #[test]
    fn inserts_after_the_frame_and_shifts_later_keys() {
        let (mut d, a, b, c, _) = sample();
        let mut cmd = DuplicateFrame::new(FrameIndex(3));
        cmd.apply(&mut d).unwrap();

        assert_eq!(cmd.new_frame(), FrameIndex(4));
        assert_eq!(d.frame_count, 25);
        // a: copy keyed at 4, the empty key moved from 6 to 7.
        assert_eq!(key_frames(&d, a), [0, 3, 4, 7]);
        assert_eq!(
            d.track(a).unwrap().key_at(FrameIndex(7)),
            Some(&Keyframe::Empty)
        );
        // b: its drawing (keyed at 2) is exposed at 3, so it gets a copy too.
        assert_eq!(key_frames(&d, b), [2, 4]);
        // c: nothing to copy, nothing changes.
        assert!(d.track(c).unwrap().is_empty());
        assert!(cmd.copy_on(c).is_none());
        assert_eq!(d.drawing_at(a, FrameIndex(4)), cmd.copy_on(a));
    }

    #[test]
    fn copy_is_deep_with_fresh_ids() {
        let (mut d, a, _, _, a3) = sample();
        let mut cmd = DuplicateFrame::new(FrameIndex(3));
        cmd.apply(&mut d).unwrap();
        let copy = cmd.copy_on(a).unwrap();
        assert_ne!(copy, a3);

        let original = d.drawing(a3).unwrap().as_vector().unwrap().clone();
        let duplicate = d.drawing(copy).unwrap().as_vector().unwrap().clone();
        assert_eq!(original.strokes().len(), duplicate.strokes().len());
        assert_ne!(original.strokes()[0].id, duplicate.strokes()[0].id);
        assert_eq!(original.strokes()[0].points, duplicate.strokes()[0].points);

        // Editing the copy leaves the original alone.
        let stroke = duplicate.strokes()[0].id;
        d.drawing_mut(copy)
            .and_then(DrawingContent::as_vector_mut)
            .unwrap()
            .remove_stroke(stroke)
            .unwrap();
        assert_eq!(d.drawing(a3).unwrap().as_vector().unwrap(), &original);
    }

    #[test]
    fn duplicating_an_empty_exposure_adds_no_key() {
        let (mut d, a, ..) = sample();
        let mut cmd = DuplicateFrame::new(FrameIndex(8)); // inside the empty key at 6
        cmd.apply(&mut d).unwrap();
        assert_eq!(key_frames(&d, a), [0, 3, 6]);
        assert!(cmd.copy_on(a).is_none());
        assert_eq!(d.drawing_at(a, FrameIndex(9)), None);
    }

    #[test]
    fn duplicating_the_last_frame_grows_the_animation() {
        let (mut d, a, _, _, a3) = sample();
        d.track_mut(a)
            .unwrap()
            .set_key(FrameIndex(23), Keyframe::Drawing(a3));
        let mut cmd = DuplicateFrame::new(FrameIndex(23));
        cmd.apply(&mut d).unwrap();
        assert_eq!(d.frame_count, 25);
        assert_eq!(d.last_frame(), Some(FrameIndex(24)));
        assert_eq!(d.drawing_at(a, FrameIndex(24)), cmd.copy_on(a));
    }

    #[test]
    fn undo_restores_and_redo_reuses_ids() {
        let (mut d, a, b, ..) = sample();
        let before = d.clone();
        let mut h = History::new();
        h.execute(&mut d, Box::new(DuplicateFrame::new(FrameIndex(3))))
            .unwrap();
        let after = d.clone();
        assert_eq!(h.undo_label(), Some("Duplicate Frame"));

        assert!(h.undo(&mut d).unwrap());
        assert_eq!(d, before);

        assert!(h.redo(&mut d).unwrap());
        assert_eq!(d, after, "redo keeps the copy's IDs");
        assert!(d.drawing_at(a, FrameIndex(4)).is_some());
        assert!(d.drawing_at(b, FrameIndex(4)).is_some());
    }

    #[test]
    fn rejects_frames_past_the_end() {
        let (mut d, ..) = sample();
        let before = d.clone();
        let err = DuplicateFrame::new(FrameIndex(24)).apply(&mut d);
        assert!(matches!(err, Err(CommandError::FrameOutOfRange { .. })));
        assert_eq!(d, before);
    }

    #[test]
    fn rejects_overflow_and_leaves_document_unchanged() {
        let (mut d, a, ..) = sample();
        d.track_mut(a)
            .unwrap()
            .set_key(FrameIndex(u32::MAX), Keyframe::Empty);
        let before = d.clone();
        assert_eq!(
            DuplicateFrame::new(FrameIndex(3)).apply(&mut d),
            Err(CommandError::FrameLimit)
        );
        assert_eq!(d, before);
    }
}
