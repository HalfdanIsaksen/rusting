//! Frame edits that act on one layer only and keep the animation length.

use super::{editable_layer, frame_in_range};
use crate::{Command, CommandError};
use animation_core::{DrawingId, FrameIndex, FrameRange, LayerId};
use document::Document;
use document::drawing::DrawingContent;
use document::timeline::{Keyframe, RemovedFrames};

/// The layers of `layers` that frame edits may change: drawing layers that
/// are not locked. Fails with [`CommandError::LayerLocked`] if none are left.
fn editable_layers(doc: &Document, layers: &[LayerId]) -> Result<Vec<LayerId>, CommandError> {
    let editable: Vec<LayerId> = layers
        .iter()
        .copied()
        .filter(|&id| editable_layer(doc, id).is_ok() && doc.track(id).is_some())
        .collect();
    if editable.is_empty() {
        Err(CommandError::LayerLocked)
    } else {
        Ok(editable)
    }
}

/// Fails unless `range` is a non-empty span inside the animation.
fn range_in_animation(doc: &Document, range: FrameRange) -> Result<(), CommandError> {
    if range.is_empty() || range.end.0 > doc.frame_count {
        Err(CommandError::FrameOutOfRange {
            frame: range.end,
            frame_count: doc.frame_count,
        })
    } else {
        Ok(())
    }
}

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

/// What [`RemoveLayerFrames`] took out of one layer.
#[derive(Debug)]
struct LayerRemoval {
    layer: LayerId,
    removed: RemovedFrames,
    /// A removed key whose exposure ran past the range was put back at the
    /// range start, so the frames after the range keep their picture.
    carried: bool,
    /// An empty key was added where the layer's frames now end.
    ended: Option<FrameIndex>,
}

/// Removes a range of frames on some layers: keys inside it are dropped and
/// the layer's later keys move left to close the gap. The layer's frames end
/// that many frames earlier (an empty key marks the new end); other layers,
/// the animation length, tags and audio clips are left alone. Locked and
/// audio layers are skipped.
#[derive(Debug)]
pub struct RemoveLayerFrames {
    layers: Vec<LayerId>,
    range: FrameRange,
    removals: Vec<LayerRemoval>,
}

impl RemoveLayerFrames {
    /// Creates the command.
    pub fn new(layers: Vec<LayerId>, range: FrameRange) -> Self {
        Self {
            layers,
            range,
            removals: Vec::new(),
        }
    }
}

impl Command for RemoveLayerFrames {
    fn label(&self) -> &str {
        "Remove Frames"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        range_in_animation(doc, self.range)?;
        let layers = editable_layers(doc, &self.layers)?;
        let FrameRange { start, end } = self.range;
        let len = self.range.len();
        let frame_count = doc.frame_count;
        let tail = FrameIndex(frame_count - len);
        self.removals.clear();
        for layer in layers {
            let Some(track) = doc.track_mut(layer) else {
                continue;
            };
            let held_past = end.0 < frame_count && track.key_at(end).is_none();
            let removed = track.remove_frames(start, len);
            let carried = match removed.keys.last() {
                Some(&(_, key)) if held_past => {
                    track.set_key(start, key);
                    true
                }
                _ => false,
            };
            // The frames pulled in at the end show nothing.
            let shows_at_tail = track
                .exposed_key(tail)
                .is_some_and(|(at, key)| at < tail && *key != Keyframe::Empty);
            let ended = shows_at_tail.then(|| {
                track.set_key(tail, Keyframe::Empty);
                tail
            });
            self.removals.push(LayerRemoval {
                layer,
                removed,
                carried,
                ended,
            });
        }
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        for removal in self.removals.drain(..).rev() {
            let track = doc
                .track_mut(removal.layer)
                .ok_or(CommandError::LayerNotFound)?;
            if let Some(at) = removal.ended {
                track.remove_key(at);
            }
            if removal.carried {
                track.remove_key(self.range.start);
            }
            track.restore_frames(&removal.removed);
        }
        Ok(())
    }
}

/// Moves the keys inside a range of frames to start at `to`, on some layers
/// only. Frames are not added or removed, and other layers, tags and audio
/// clips stay where they are. Locked and audio layers are skipped.
///
/// Fails with [`CommandError::KeyframeExists`] if a key outside the range
/// sits where a moved key would land, so a move never replaces a drawing.
#[derive(Debug)]
pub struct MoveLayerFrames {
    layers: Vec<LayerId>,
    range: FrameRange,
    to: FrameIndex,
    /// The layers actually moved, kept for undo.
    moved: Vec<LayerId>,
}

impl MoveLayerFrames {
    /// Creates the command.
    pub fn new(layers: Vec<LayerId>, range: FrameRange, to: FrameIndex) -> Self {
        Self {
            layers,
            range,
            to,
            moved: Vec::new(),
        }
    }

    /// Where the range lands.
    pub fn target(&self) -> FrameRange {
        FrameRange::new(
            self.to,
            FrameIndex(self.to.0.saturating_add(self.range.len())),
        )
    }

    /// The first key outside the range, on the layers that move, that
    /// blocks the move, if any.
    pub fn blocked_by(&self, doc: &Document) -> Option<FrameIndex> {
        let target = self.target();
        self.layers
            .iter()
            .filter(|&&id| doc.layers().is_editable(id))
            .filter_map(|&id| doc.track(id))
            .find_map(|track| {
                track
                    .keys()
                    .map(|(f, _)| f)
                    .find(|f| target.contains(*f) && !self.range.contains(*f))
            })
    }
}

/// Moves the keys of `layers` inside `range` to start at `to`. The caller
/// has checked that nothing is in the way.
fn move_keys(doc: &mut Document, layers: &[LayerId], range: FrameRange, to: FrameIndex) {
    for &layer in layers {
        let Some(track) = doc.track_mut(layer) else {
            continue;
        };
        let moved: Vec<(FrameIndex, Keyframe)> = track
            .keys()
            .filter(|(f, _)| range.contains(*f))
            .map(|(f, k)| (f, *k))
            .collect();
        for &(f, _) in &moved {
            track.remove_key(f);
        }
        for (f, key) in moved {
            track.set_key(FrameIndex(f.0 - range.start.0 + to.0), key);
        }
    }
}

impl Command for MoveLayerFrames {
    fn label(&self) -> &str {
        "Move Frames"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        range_in_animation(doc, self.range)?;
        range_in_animation(doc, self.target())?;
        let layers = editable_layers(doc, &self.layers)?;
        if let Some(frame) = self.blocked_by(doc) {
            return Err(CommandError::KeyframeExists(frame));
        }
        move_keys(doc, &layers, self.range, self.to);
        self.moved = layers;
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        move_keys(doc, &self.moved, self.target(), self.range.start);
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

    fn range(start: u32, end: u32) -> FrameRange {
        FrameRange::new(FrameIndex(start), FrameIndex(end))
    }

    #[test]
    fn remove_pulls_one_layers_keys_left_and_keeps_the_length() {
        let (mut d, a, b) = sample();
        // a: 0, 3, empty 6. Removing 1..3 pulls 3 → 1 and 6 → 4.
        roundtrip(
            &mut d,
            Box::new(RemoveLayerFrames::new(vec![a], range(1, 3))),
        );
        assert_eq!(key_frames(&d, a), [0, 1, 4]);
        assert_eq!(key_frames(&d, b), [2], "other layers are untouched");
        assert_eq!(d.frame_count, 24);
    }

    #[test]
    fn remove_ends_a_held_layer_earlier() {
        let (mut d, _, b) = sample();
        // b's drawing at 2 is held to the end; removing 5..9 ends it at 20.
        roundtrip(
            &mut d,
            Box::new(RemoveLayerFrames::new(vec![b], range(5, 9))),
        );
        assert_eq!(key_frames(&d, b), [2, 20]);
        assert_eq!(
            d.track(b).unwrap().key_at(FrameIndex(20)),
            Some(&Keyframe::Empty)
        );
    }

    #[test]
    fn remove_keeps_the_picture_after_the_range() {
        let (mut d, a, _) = sample();
        let shown = d.drawing_at(a, FrameIndex(5));
        // Removing 3..5 drops the key at 3, but 5 showed it: it moves to 3.
        roundtrip(
            &mut d,
            Box::new(RemoveLayerFrames::new(vec![a], range(3, 5))),
        );
        assert_eq!(d.drawing_at(a, FrameIndex(3)), shown);
        assert_eq!(key_frames(&d, a), [0, 3, 4]);
    }

    #[test]
    fn remove_skips_locked_layers_and_refuses_if_all_are() {
        let (mut d, a, b) = sample();
        d.layer_mut(a).unwrap().locked = true;
        roundtrip(
            &mut d,
            Box::new(RemoveLayerFrames::new(vec![a, b], range(0, 1))),
        );
        assert_eq!(key_frames(&d, a), [0, 3, 6]);
        assert_eq!(key_frames(&d, b), [1, 23]);
        assert_eq!(
            RemoveLayerFrames::new(vec![a], range(0, 1)).apply(&mut d),
            Err(CommandError::LayerLocked)
        );
    }

    #[test]
    fn move_shifts_only_the_given_layers() {
        let (mut d, a, b) = sample();
        roundtrip(
            &mut d,
            Box::new(MoveLayerFrames::new(vec![b], range(2, 3), FrameIndex(5))),
        );
        assert_eq!(key_frames(&d, b), [5]);
        assert_eq!(key_frames(&d, a), [0, 3, 6]);
    }

    #[test]
    fn move_is_blocked_only_by_keys_on_the_moving_layers() {
        let (mut d, a, b) = sample();
        // b's key at 2 moving to 3 would hit a's key at 3, but a is not moving.
        let cmd = MoveLayerFrames::new(vec![b], range(2, 3), FrameIndex(3));
        assert_eq!(cmd.blocked_by(&d), None);
        let mut cmd = MoveLayerFrames::new(vec![a], range(3, 4), FrameIndex(6));
        assert_eq!(
            cmd.apply(&mut d),
            Err(CommandError::KeyframeExists(FrameIndex(6)))
        );
        let mut past = MoveLayerFrames::new(vec![a], range(3, 4), FrameIndex(24));
        assert!(matches!(
            past.apply(&mut d),
            Err(CommandError::FrameOutOfRange { .. })
        ));
    }
}
