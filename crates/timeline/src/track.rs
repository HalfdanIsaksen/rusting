//! Per-layer drawing tracks.

use animation_core::{DrawingId, FrameIndex, FrameRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::Bound;

/// A keyframe on a drawing track.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Keyframe {
    /// Show the given drawing from this frame on.
    Drawing(DrawingId),
    /// Show nothing from this frame on (an empty drawing keyframe).
    Empty,
}

impl Keyframe {
    /// The drawing this keyframe shows, if any.
    pub fn drawing(&self) -> Option<DrawingId> {
        match self {
            Self::Drawing(id) => Some(*id),
            Self::Empty => None,
        }
    }
}

/// An ordered set of drawing keyframes for one layer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub(crate) keys: BTreeMap<FrameIndex, Keyframe>,
}

impl Track {
    /// Creates an empty track.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces the keyframe at `frame`. Returns the old one, if any.
    pub fn set_key(&mut self, frame: FrameIndex, key: Keyframe) -> Option<Keyframe> {
        self.keys.insert(frame, key)
    }

    /// Removes the keyframe at `frame`. Returns it, if one existed.
    pub fn remove_key(&mut self, frame: FrameIndex) -> Option<Keyframe> {
        self.keys.remove(&frame)
    }

    /// The keyframe placed exactly at `frame`.
    pub fn key_at(&self, frame: FrameIndex) -> Option<&Keyframe> {
        self.keys.get(&frame)
    }

    /// The keyframe whose exposure covers `frame`: the last key at or before it.
    pub fn exposed_key(&self, frame: FrameIndex) -> Option<(FrameIndex, &Keyframe)> {
        self.keys.range(..=frame).next_back().map(|(f, k)| (*f, k))
    }

    /// The first keyframe strictly after `frame`.
    pub fn next_key_after(&self, frame: FrameIndex) -> Option<(FrameIndex, &Keyframe)> {
        self.keys
            .range((Bound::Excluded(frame), Bound::Unbounded))
            .next()
            .map(|(f, k)| (*f, k))
    }

    /// The last keyframe strictly before `frame`.
    pub fn prev_key_before(&self, frame: FrameIndex) -> Option<(FrameIndex, &Keyframe)> {
        self.keys.range(..frame).next_back().map(|(f, k)| (*f, k))
    }

    /// The span of frames covered by the key exposed at `frame`.
    ///
    /// The span starts at that key and ends at the next key, or at
    /// `frame_count` (the document length), whichever comes first. Returns
    /// `None` if `frame` is past the document end or no key covers it. An
    /// [`Keyframe::Empty`] key also has an exposure range; check
    /// [`Track::exposed_key`] if only drawings matter.
    pub fn exposure_range(&self, frame: FrameIndex, frame_count: u32) -> Option<FrameRange> {
        if frame.0 >= frame_count {
            return None;
        }
        let (start, _) = self.exposed_key(frame)?;
        let end = self
            .next_key_after(start)
            .map_or(frame_count, |(f, _)| f.0.min(frame_count));
        Some(FrameRange::new(start, FrameIndex(end)))
    }

    /// The drawing visible at `frame`, if any.
    pub fn drawing_at(&self, frame: FrameIndex) -> Option<DrawingId> {
        self.exposed_key(frame).and_then(|(_, k)| k.drawing())
    }

    /// All keyframes in frame order.
    pub fn keys(&self) -> impl Iterator<Item = (FrameIndex, &Keyframe)> {
        self.keys.iter().map(|(f, k)| (*f, k))
    }

    /// Number of keyframes.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the track has no keyframes.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Track, DrawingId, DrawingId) {
        let (a, b) = (DrawingId::new(), DrawingId::new());
        let mut t = Track::new();
        t.set_key(FrameIndex(2), Keyframe::Drawing(a));
        t.set_key(FrameIndex(5), Keyframe::Empty);
        t.set_key(FrameIndex(9), Keyframe::Drawing(b));
        (t, a, b)
    }

    #[test]
    fn next_and_prev_key_are_strict() {
        let (t, ..) = sample();
        assert_eq!(
            t.next_key_after(FrameIndex(0)).map(|k| k.0),
            Some(FrameIndex(2))
        );
        assert_eq!(
            t.next_key_after(FrameIndex(2)).map(|k| k.0),
            Some(FrameIndex(5))
        );
        assert_eq!(
            t.next_key_after(FrameIndex(6)).map(|k| k.0),
            Some(FrameIndex(9))
        );
        assert_eq!(t.next_key_after(FrameIndex(9)), None);
        assert_eq!(
            t.prev_key_before(FrameIndex(9)).map(|k| k.0),
            Some(FrameIndex(5))
        );
        assert_eq!(
            t.prev_key_before(FrameIndex(4)).map(|k| k.0),
            Some(FrameIndex(2))
        );
        assert_eq!(t.prev_key_before(FrameIndex(2)), None);
        assert_eq!(Track::new().next_key_after(FrameIndex(0)), None);
    }

    #[test]
    fn exposure_range_ends_at_next_key_or_document_end() {
        let (t, ..) = sample();
        let r = |s, e| Some(FrameRange::new(FrameIndex(s), FrameIndex(e)));
        assert_eq!(t.exposure_range(FrameIndex(0), 24), None);
        assert_eq!(t.exposure_range(FrameIndex(2), 24), r(2, 5));
        assert_eq!(t.exposure_range(FrameIndex(4), 24), r(2, 5));
        assert_eq!(t.exposure_range(FrameIndex(7), 24), r(5, 9));
        assert_eq!(t.exposure_range(FrameIndex(20), 24), r(9, 24));
        // Document shorter than the next key clips the exposure.
        assert_eq!(t.exposure_range(FrameIndex(3), 4), r(2, 4));
        // Past the document end.
        assert_eq!(t.exposure_range(FrameIndex(24), 24), None);
    }

    #[test]
    fn stepped_exposure_holds_until_next_key() {
        let (a, b) = (DrawingId::new(), DrawingId::new());
        let mut t = Track::new();
        t.set_key(FrameIndex(0), Keyframe::Drawing(a));
        t.set_key(FrameIndex(4), Keyframe::Drawing(b));
        t.set_key(FrameIndex(8), Keyframe::Empty);

        assert_eq!(t.drawing_at(FrameIndex(0)), Some(a));
        assert_eq!(t.drawing_at(FrameIndex(3)), Some(a));
        assert_eq!(t.drawing_at(FrameIndex(4)), Some(b));
        assert_eq!(t.drawing_at(FrameIndex(7)), Some(b));
        assert_eq!(t.drawing_at(FrameIndex(8)), None);
        assert_eq!(t.drawing_at(FrameIndex(100)), None);
    }

    #[test]
    fn nothing_before_first_key() {
        let mut t = Track::new();
        t.set_key(FrameIndex(2), Keyframe::Drawing(DrawingId::new()));
        assert_eq!(t.drawing_at(FrameIndex(1)), None);
    }

    #[test]
    fn set_and_remove_keys() {
        let a = DrawingId::new();
        let mut t = Track::new();
        assert_eq!(t.set_key(FrameIndex(1), Keyframe::Empty), None);
        assert_eq!(
            t.set_key(FrameIndex(1), Keyframe::Drawing(a)),
            Some(Keyframe::Empty)
        );
        assert_eq!(t.len(), 1);
        assert_eq!(t.remove_key(FrameIndex(1)), Some(Keyframe::Drawing(a)));
        assert!(t.is_empty());
        assert_eq!(t.remove_key(FrameIndex(1)), None);
    }
}
