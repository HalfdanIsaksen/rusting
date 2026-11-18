//! Frame edits that act on one layer only and keep the animation length.

use super::{editable_layer, frame_in_range};
use crate::{Command, CommandError};
use animation_core::{DrawingId, FrameIndex, LayerId};
use document::Document;
use document::drawing::DrawingContent;
use document::timeline::Keyframe;

/// Fails with [`CommandError::NoRoomOnLayer`] if shifting the keys of
/// `layer` at or after `at` one frame right would push one past the end.
fn room_to_shift(doc: &Document, layer: LayerId, at: FrameIndex) -> Result<bool, CommandError> {
    let track = doc.track(layer).ok_or(CommandError::LayerNotFound)?;
    match track.keys().last() {
        Some((last, _)) if last >= at => {
            if last.0.saturating_add(1) < doc.frame_count {
                Ok(true)
            } else {
                Err(CommandError::NoRoomOnLayer)
            }
        }
        _ => Ok(false),
    }
}

/// Duplicates the frame at `frame` on one layer: the next frame gets a key
/// showing the same picture.
///
/// - A drawing is deep-copied (new drawing, stroke and fill IDs).
/// - An object gets an instance key that shows the same object frame again.
/// - Nothing (an empty key or no key yet) gives an empty key.
///
/// If the next frame already has a key, the layer's keys from there on move
/// one frame right to make room; other layers and the animation length are
/// left alone. Fails if the next frame is past the end, or if a key would be
/// pushed past it.
#[derive(Debug)]
pub struct DuplicateLayerFrame {
    layer: LayerId,
    frame: FrameIndex,
    /// The copy, built on the first apply and reused on redo.
    copy: Option<(DrawingId, DrawingContent)>,
    /// Whether later keys were moved right.
    shifted: bool,
}

impl DuplicateLayerFrame {
    /// Creates the command for `frame` on `layer`.
    pub fn new(layer: LayerId, frame: FrameIndex) -> Self {
        Self {
            layer,
            frame,
            copy: None,
            shifted: false,
        }
    }

    /// The frame this command keys (right after the duplicated one).
    pub fn new_frame(&self) -> FrameIndex {
        FrameIndex(self.frame.0.saturating_add(1))
    }

    /// The new drawing, once the command has been applied to a drawing frame.
    pub fn copy(&self) -> Option<DrawingId> {
        self.copy.as_ref().map(|(id, _)| *id)
    }
}

impl Command for DuplicateLayerFrame {
    fn label(&self) -> &str {
        "Duplicate Frame"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        frame_in_range(doc, self.frame)?;
        let new = self.new_frame();
        frame_in_range(doc, new)?;
        let track = doc.track(self.layer).ok_or(CommandError::LayerNotFound)?;
        let shift = track.key_at(new).is_some();
        if shift {
            room_to_shift(doc, self.layer, new)?;
        }

        let key = if let Some(source) = doc.drawing_at(self.layer, self.frame) {
            let copy = match self.copy.take() {
                Some(copy) => copy,
                None => (
                    DrawingId::new(),
                    doc.drawing(source)
                        .ok_or(CommandError::DrawingNotFound)?
                        .duplicate(),
                ),
            };
            let key = Keyframe::Drawing(copy.0);
            doc.insert_drawing(copy.0, copy.1.clone());
            self.copy = Some(copy);
            key
        } else if let Some((at, key)) = track.displayed_instance(self.frame) {
            // Inside a motion tween, the copy shows what this frame
            // displays (ADR-019).
            let length = doc.symbol(key.symbol).map_or(1, |s| s.frame_count);
            Keyframe::Instance(key.advanced(self.frame.0 - at.0, length))
        } else {
            Keyframe::Empty
        };

        let track = doc
            .track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?;
        if shift {
            track
                .insert_frames(new, 1)
                .map_err(|_| CommandError::FrameLimit)?;
        }
        track.set_key(new, key);
        self.shifted = shift;
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let new = self.new_frame();
        let track = doc
            .track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?;
        track
            .remove_key(new)
            .ok_or(CommandError::KeyframeNotFound(new))?;
        if self.shifted {
            track.remove_frames(new, 1);
        }
        if let Some((id, _)) = &self.copy {
            doc.remove_drawing(*id)
                .ok_or(CommandError::DrawingNotFound)?;
        }
        Ok(())
    }
}

/// Inserts a blank frame at `at` on one layer: the exposure covering the
/// frame before gets one frame longer and the layer's later keys move right.
/// Other layers and the animation length are left alone; fails if a key
/// would be pushed past the end.
#[derive(Debug)]
pub struct InsertLayerFrame {
    layer: LayerId,
    at: FrameIndex,
    shifted: bool,
}

impl InsertLayerFrame {
    /// Creates the command.
    pub fn new(layer: LayerId, at: FrameIndex) -> Self {
        Self {
            layer,
            at,
            shifted: false,
        }
    }
}

impl Command for InsertLayerFrame {
    fn label(&self) -> &str {
        "Insert Frame"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        frame_in_range(doc, self.at)?;
        self.shifted = room_to_shift(doc, self.layer, self.at)?;
        if self.shifted {
            doc.track_mut(self.layer)
                .ok_or(CommandError::LayerNotFound)?
                .insert_frames(self.at, 1)
                .map_err(|_| CommandError::FrameLimit)?;
        }
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        if self.shifted {
            doc.track_mut(self.layer)
                .ok_or(CommandError::LayerNotFound)?
                .remove_frames(self.at, 1);
        }
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

    /// 24 frames. Layer `a` has drawings at 0 and 3 and an empty key at 6;
    /// layer `b` has one drawing at 2.
    fn sample() -> (Document, LayerId, LayerId) {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        let a = d.add_layer("A");
        let b = d.add_layer("B");
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
        (d, a, b)
    }

    fn roundtrip(d: &mut Document, cmd: Box<dyn Command>) {
        let before = d.clone();
        let mut h = History::new();
        h.execute(d, cmd).unwrap();
        let after = d.clone();
        h.undo(d).unwrap();
        assert_eq!(*d, before);
        h.redo(d).unwrap();
        assert_eq!(*d, after, "redo gives the same result");
    }

    #[test]
    fn duplicate_keys_the_next_frame_on_one_layer_only() {
        let (mut d, a, b) = sample();
        let mut cmd = DuplicateLayerFrame::new(a, FrameIndex(4));
        cmd.apply(&mut d).unwrap();
        assert_eq!(d.frame_count, 24, "the length stays");
        assert_eq!(key_frames(&d, a), [0, 3, 5, 6], "no shift: 5 had no key");
        assert_eq!(key_frames(&d, b), [2], "other layers are untouched");
        let copy = cmd.copy().unwrap();
        assert_eq!(d.drawing_at(a, FrameIndex(5)), Some(copy));
        assert_ne!(Some(copy), d.drawing_at(a, FrameIndex(4)));
    }

    #[test]
    fn duplicate_onto_a_key_shifts_that_layer() {
        let (mut d, a, b) = sample();
        roundtrip(&mut d, Box::new(DuplicateLayerFrame::new(a, FrameIndex(2))));
        assert_eq!(key_frames(&d, a), [0, 3, 4, 7]);
        assert_eq!(key_frames(&d, b), [2]);
        assert_eq!(d.frame_count, 24);
    }

    #[test]
    fn duplicating_nothing_adds_an_empty_key() {
        let (mut d, a, _) = sample();
        roundtrip(&mut d, Box::new(DuplicateLayerFrame::new(a, FrameIndex(8))));
        assert_eq!(
            d.track(a).unwrap().key_at(FrameIndex(9)),
            Some(&Keyframe::Empty)
        );
    }

    #[test]
    fn duplicate_refuses_the_last_frame_and_full_layers() {
        let (mut d, a, _) = sample();
        let before = d.clone();
        assert!(matches!(
            DuplicateLayerFrame::new(a, FrameIndex(23)).apply(&mut d),
            Err(CommandError::FrameOutOfRange { .. })
        ));
        d.track_mut(a)
            .unwrap()
            .set_key(FrameIndex(23), Keyframe::Empty);
        let full = d.clone();
        assert_eq!(
            DuplicateLayerFrame::new(a, FrameIndex(2)).apply(&mut d),
            Err(CommandError::NoRoomOnLayer)
        );
        assert_eq!(d, full);
        assert_ne!(d, before);
    }

    #[test]
    fn duplicate_refuses_locked_layers() {
        let (mut d, a, _) = sample();
        d.layer_mut(a).unwrap().locked = true;
        assert_eq!(
            DuplicateLayerFrame::new(a, FrameIndex(3)).apply(&mut d),
            Err(CommandError::LayerLocked)
        );
    }

    #[test]
    fn insert_shifts_one_layer_and_keeps_the_length() {
        let (mut d, a, b) = sample();
        roundtrip(&mut d, Box::new(InsertLayerFrame::new(a, FrameIndex(4))));
        assert_eq!(key_frames(&d, a), [0, 3, 7]);
        assert_eq!(key_frames(&d, b), [2]);
        assert_eq!(d.frame_count, 24);
    }

    #[test]
    fn insert_after_the_last_key_changes_nothing() {
        let (mut d, _, b) = sample();
        let before = d.clone();
        roundtrip(&mut d, Box::new(InsertLayerFrame::new(b, FrameIndex(5))));
        assert_eq!(d, before);
    }
}
