//! Frame-level timeline edits that act on every layer at once.

use super::frame_in_range;
use crate::{Command, CommandError};
use animation_core::{DrawingId, FrameIndex, FrameRange, LayerId};
use document::drawing::DrawingContent;
use document::timeline::{Keyframe, RemovedFrames};
use document::{Document, FrameTag};

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
        shift_tags(doc, new, true);
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
        shift_tags(doc, new, false);
        Ok(())
    }
}

/// Inserts `count` blank frames at `at` on every layer: the exposure that
/// covers the frame before `at` gets longer and later keys move right. The
/// animation grows by `count`. A tag ending right before `at` grows with it.
#[derive(Debug)]
pub struct InsertFrames {
    at: FrameIndex,
    count: u32,
    /// The tags before the insert, swapped back on undo.
    tags: Vec<FrameTag>,
}

impl InsertFrames {
    /// Creates the command.
    pub fn new(at: FrameIndex, count: u32) -> Self {
        Self {
            at,
            count,
            tags: Vec::new(),
        }
    }
}

impl Command for InsertFrames {
    fn label(&self) -> &str {
        "Insert Frames"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        if self.at.0 > doc.frame_count {
            return Err(CommandError::FrameOutOfRange {
                frame: self.at,
                frame_count: doc.frame_count,
            });
        }
        let frame_count = doc
            .frame_count
            .checked_add(self.count)
            .ok_or(CommandError::FrameLimit)?;
        // Check every track first so a failure leaves the document unchanged.
        let overflows = doc.tracks().values().any(|track| {
            track.keys().last().is_some_and(|(last, _)| {
                last >= self.at && last.0.checked_add(self.count).is_none()
            })
        });
        if overflows {
            return Err(CommandError::FrameLimit);
        }
        let layers: Vec<LayerId> = doc.tracks().keys().copied().collect();
        for layer in layers {
            if let Some(track) = doc.track_mut(layer) {
                track
                    .insert_frames(self.at, self.count)
                    .map_err(|_| CommandError::FrameLimit)?;
            }
        }
        doc.frame_count = frame_count;
        self.tags = doc.tags.clone();
        for tag in &mut doc.tags {
            if tag.range.start >= self.at {
                tag.range.start.0 += self.count;
            }
            if tag.range.end >= self.at {
                tag.range.end.0 += self.count;
            }
        }
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let layers: Vec<LayerId> = doc.tracks().keys().copied().collect();
        for layer in layers {
            if let Some(track) = doc.track_mut(layer) {
                track.remove_frames(self.at, self.count);
            }
        }
        doc.frame_count = doc.frame_count.saturating_sub(self.count);
        std::mem::swap(&mut doc.tags, &mut self.tags);
        Ok(())
    }
}

/// What [`RemoveFrames`] took out of one layer.
#[derive(Debug)]
struct LayerRemoval {
    layer: LayerId,
    removed: RemovedFrames,
    /// A removed key whose exposure ran past the range was put back at the
    /// range start, so the frames after the range keep their picture.
    carried: bool,
}

/// Removes a range of frames on every layer: keys inside it are dropped and
/// later keys move left. The animation shrinks by the range length. Tags
/// shrink with it, and tags left with no frames are removed.
///
/// Fails with [`CommandError::ZeroFrameCount`] rather than removing every frame.
#[derive(Debug)]
pub struct RemoveFrames {
    range: FrameRange,
    removals: Vec<LayerRemoval>,
    /// The tags before the removal, swapped back on undo.
    tags: Vec<FrameTag>,
}

impl RemoveFrames {
    /// Creates the command.
    pub fn new(range: FrameRange) -> Self {
        Self {
            range,
            removals: Vec::new(),
            tags: Vec::new(),
        }
    }
}

impl Command for RemoveFrames {
    fn label(&self) -> &str {
        "Remove Frames"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let FrameRange { start, end } = self.range;
        if self.range.is_empty() || end.0 > doc.frame_count {
            return Err(CommandError::FrameOutOfRange {
                frame: end,
                frame_count: doc.frame_count,
            });
        }
        let len = self.range.len();
        if len >= doc.frame_count {
            return Err(CommandError::ZeroFrameCount);
        }
        let frame_count = doc.frame_count;
        let layers: Vec<LayerId> = doc.tracks().keys().copied().collect();
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
            self.removals.push(LayerRemoval {
                layer,
                removed,
                carried,
            });
        }
        doc.frame_count -= len;
        self.tags = doc.tags.clone();
        let map = |f: FrameIndex| {
            if f <= start {
                f
            } else if f >= end {
                FrameIndex(f.0 - len)
            } else {
                start
            }
        };
        for tag in &mut doc.tags {
            tag.range = FrameRange::new(map(tag.range.start), map(tag.range.end));
        }
        doc.tags.retain(|t| !t.range.is_empty());
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        for removal in self.removals.drain(..) {
            let track = doc
                .track_mut(removal.layer)
                .ok_or(CommandError::LayerNotFound)?;
            if removal.carried {
                track.remove_key(self.range.start);
            }
            track.restore_frames(&removal.removed);
        }
        doc.frame_count += self.range.len();
        std::mem::swap(&mut doc.tags, &mut self.tags);
        Ok(())
    }
}

/// Moves the keys inside a range of frames to start at `to`, on every layer.
/// Frames are not added or removed and tags stay where they are.
///
/// Fails with [`CommandError::KeyframeExists`] if a key outside the range
/// sits where a moved key would land, so a move never replaces a drawing.
// ponytail: only keys inside the range move; a drawing held into the range
// from an earlier key stays put. Carry it if users expect the picture to move.
#[derive(Debug)]
pub struct MoveFrames {
    range: FrameRange,
    to: FrameIndex,
}

impl MoveFrames {
    /// Creates the command.
    pub fn new(range: FrameRange, to: FrameIndex) -> Self {
        Self { range, to }
    }

    /// Where the range lands.
    pub fn target(&self) -> FrameRange {
        FrameRange::new(
            self.to,
            FrameIndex(self.to.0.saturating_add(self.range.len())),
        )
    }

    /// The first key outside the range that blocks the move, if any.
    pub fn blocked_by(&self, doc: &Document) -> Option<FrameIndex> {
        let target = self.target();
        doc.tracks().values().find_map(|track| {
            track
                .keys()
                .map(|(f, _)| f)
                .find(|f| target.contains(*f) && !self.range.contains(*f))
        })
    }
}

impl Command for MoveFrames {
    fn label(&self) -> &str {
        "Move Frames"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let target = self.target();
        if self.range.is_empty() || target.end.0 > doc.frame_count {
            return Err(CommandError::FrameOutOfRange {
                frame: target.end,
                frame_count: doc.frame_count,
            });
        }
        if let Some(frame) = self.blocked_by(doc) {
            return Err(CommandError::KeyframeExists(frame));
        }
        let layers: Vec<LayerId> = doc.tracks().keys().copied().collect();
        for layer in layers {
            let Some(track) = doc.track_mut(layer) else {
                continue;
            };
            let moved: Vec<(FrameIndex, Keyframe)> = track
                .keys()
                .filter(|(f, _)| self.range.contains(*f))
                .map(|(f, k)| (f, *k))
                .collect();
            for &(f, _) in &moved {
                track.remove_key(f);
            }
            for (f, key) in moved {
                track.set_key(FrameIndex(f.0 - self.range.start.0 + self.to.0), key);
            }
        }
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        MoveFrames::new(self.target(), self.range.start).apply(doc)
    }
}

/// Moves tags for a frame inserted (or, undoing, removed) at `new`: tags
/// starting there or later shift, and a tag ending there or later grows so
/// the copy of its frame stays in it.
fn shift_tags(doc: &mut Document, new: FrameIndex, insert: bool) {
    let step = |f: &mut FrameIndex| {
        f.0 = if insert {
            f.0.saturating_add(1)
        } else {
            f.0.saturating_sub(1)
        }
    };
    // After an insert the affected bounds sit one past where they started.
    let at = if insert { new.0 } else { new.0 + 1 };
    for tag in &mut doc.tags {
        if tag.range.start.0 >= at {
            step(&mut tag.range.start);
            step(&mut tag.range.end);
        } else if tag.range.end.0 >= at {
            step(&mut tag.range.end);
        }
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
    fn tags_shift_and_grow_and_undo_restores_them() {
        use animation_core::FrameRange;
        use document::FrameTag;
        let (mut d, ..) = sample();
        let tag = |name: &str, s, e| FrameTag {
            name: name.into(),
            range: FrameRange::new(FrameIndex(s), FrameIndex(e)),
        };
        d.tags = vec![tag("in", 2, 5), tag("after", 5, 8), tag("before", 0, 2)];
        let before = d.clone();
        let mut h = History::new();
        h.execute(&mut d, Box::new(DuplicateFrame::new(FrameIndex(4))))
            .unwrap();
        assert_eq!(
            d.tags,
            [tag("in", 2, 6), tag("after", 6, 9), tag("before", 0, 2)]
        );
        assert!(h.undo(&mut d).unwrap());
        assert_eq!(d, before);
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

    #[test]
    fn insert_frames_lengthens_the_exposure_and_undoes() {
        let (mut d, a, b, _, _) = sample();
        d.tags.push(FrameTag {
            name: "T".into(),
            range: FrameRange::new(FrameIndex(0), FrameIndex(3)),
        });
        let before = d.clone();
        let mut h = History::new();
        h.execute(&mut d, Box::new(InsertFrames::new(FrameIndex(3), 2)))
            .unwrap();
        assert_eq!(d.frame_count, 26);
        assert_eq!(key_frames(&d, a), [0, 5, 8]);
        assert_eq!(key_frames(&d, b), [2]);
        assert_eq!(d.tags[0].range.end, FrameIndex(5));
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
    }

    #[test]
    fn remove_frames_keeps_the_picture_after_the_range() {
        let (mut d, a, b, _, a3) = sample();
        d.tags.push(FrameTag {
            name: "Gone".into(),
            range: FrameRange::new(FrameIndex(3), FrameIndex(5)),
        });
        d.tags.push(FrameTag {
            name: "Kept".into(),
            range: FrameRange::new(FrameIndex(4), FrameIndex(10)),
        });
        let before = d.clone();
        let mut h = History::new();
        h.execute(
            &mut d,
            Box::new(RemoveFrames::new(FrameRange::new(
                FrameIndex(3),
                FrameIndex(5),
            ))),
        )
        .unwrap();
        assert_eq!(d.frame_count, 22);
        // a: the key at 3 still showed at 5, so it is carried to 3; 6 -> 4.
        assert_eq!(key_frames(&d, a), [0, 3, 4]);
        assert_eq!(d.drawing_at(a, FrameIndex(3)), Some(a3));
        assert_eq!(key_frames(&d, b), [2]);
        assert_eq!(d.tags.len(), 1);
        assert_eq!(
            d.tags[0].range,
            FrameRange::new(FrameIndex(3), FrameIndex(8))
        );
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
    }

    #[test]
    fn remove_frames_never_removes_every_frame() {
        let (mut d, ..) = sample();
        let all = FrameRange::new(FrameIndex(0), FrameIndex(d.frame_count));
        assert_eq!(
            RemoveFrames::new(all).apply(&mut d),
            Err(CommandError::ZeroFrameCount)
        );
    }

    #[test]
    fn move_frames_shifts_keys_on_every_layer_and_undoes() {
        let (mut d, a, b, _, _) = sample();
        let before = d.clone();
        let mut h = History::new();
        // Frames 2..4 hold b's key at 2 and a's key at 3.
        let range = FrameRange::new(FrameIndex(2), FrameIndex(4));
        h.execute(&mut d, Box::new(MoveFrames::new(range, FrameIndex(4))))
            .unwrap();
        assert_eq!(key_frames(&d, a), [0, 5, 6]);
        assert_eq!(key_frames(&d, b), [4]);
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);

        // Landing on a's empty key at 6 is refused.
        let blocked = MoveFrames::new(range, FrameIndex(5));
        assert_eq!(blocked.blocked_by(&d), Some(FrameIndex(6)));
        assert_eq!(
            h.execute(&mut d, Box::new(blocked)),
            Err(CommandError::KeyframeExists(FrameIndex(6)))
        );
        assert_eq!(d, before);
    }
}
