//! Per-layer drawing tracks.

use animation_core::{DrawingId, FrameIndex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
    keys: BTreeMap<FrameIndex, Keyframe>,
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
