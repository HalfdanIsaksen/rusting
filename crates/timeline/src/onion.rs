//! Onion-skin neighbour queries.
//!
//! Onion skins show the drawings on neighbouring *keys*, not neighbouring
//! frames: with a drawing held for four frames, the previous skin is the
//! drawing before that hold, not the same drawing again.

use crate::track::Track;
use animation_core::{DrawingId, FrameIndex};
use std::ops::Bound;

/// The drawings to onion-skin around `frame`.
///
/// Returns up to `before` drawings from keys before the key exposed at
/// `frame` (offsets `-1`, `-2`, …, nearest first) and up to `after` drawings
/// from keys after `frame` (offsets `1`, `2`, …). [`Keyframe::Empty`] keys
/// are skipped and do not use up an offset. The result is sorted by offset,
/// so it runs from the furthest past skin to the furthest future one.
///
/// [`Keyframe::Empty`]: crate::Keyframe::Empty
pub fn onion_skin_frames(
    track: &Track,
    frame: FrameIndex,
    before: u32,
    after: u32,
) -> Vec<(i32, DrawingId)> {
    // Keys before the current exposure. With no key at or before `frame`
    // this range is empty anyway.
    let anchor = track.exposed_key(frame).map_or(frame, |(f, _)| f);
    let past = track
        .keys
        .range(..anchor)
        .rev()
        .filter_map(|(_, k)| k.drawing())
        .take(before as usize)
        .zip(1..)
        .map(|(id, n): (DrawingId, i32)| (-n, id));
    let future = track
        .keys
        .range((Bound::Excluded(frame), Bound::Unbounded))
        .filter_map(|(_, k)| k.drawing())
        .take(after as usize)
        .zip(1..)
        .map(|(id, n): (DrawingId, i32)| (n, id));

    let mut skins: Vec<_> = past.collect();
    skins.reverse();
    skins.extend(future);
    skins
}

impl Track {
    /// Method form of [`onion_skin_frames`].
    pub fn onion_skin_frames(
        &self,
        frame: FrameIndex,
        before: u32,
        after: u32,
    ) -> Vec<(i32, DrawingId)> {
        onion_skin_frames(self, frame, before, after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Keyframe;

    /// Keys: 0=A, 4=B, 6=Empty, 8=C, 12=D, 16=E.
    fn sample() -> (Track, [DrawingId; 5]) {
        let ids = [(); 5].map(|_| DrawingId::new());
        let mut t = Track::new();
        for (f, id) in [0, 4, 8, 12, 16].into_iter().zip(ids) {
            t.set_key(FrameIndex(f), Keyframe::Drawing(id));
        }
        t.set_key(FrameIndex(6), Keyframe::Empty);
        (t, ids)
    }

    #[test]
    fn uses_neighbouring_keys_not_frames() {
        let (t, [a, b, _, d, _]) = sample();
        // Frame 10 is inside C's hold (8..12).
        assert_eq!(
            onion_skin_frames(&t, FrameIndex(10), 2, 1),
            [(-2, a), (-1, b), (1, d)]
        );
        // Same answer on the key itself.
        assert_eq!(
            t.onion_skin_frames(FrameIndex(8), 2, 1),
            [(-2, a), (-1, b), (1, d)]
        );
    }

    #[test]
    fn skips_empty_keys() {
        let (t, [a, b, c, ..]) = sample();
        // Frame 6 exposes the Empty key: B is still the previous drawing.
        assert_eq!(
            onion_skin_frames(&t, FrameIndex(6), 1, 1),
            [(-1, b), (1, c)]
        );
        // From C, stepping back skips the Empty key at 6.
        assert_eq!(
            onion_skin_frames(&t, FrameIndex(8), 2, 0),
            [(-2, a), (-1, b)]
        );
        // From B, the next drawing is C, past the Empty key.
        assert_eq!(onion_skin_frames(&t, FrameIndex(4), 0, 1), [(1, c)]);
    }

    #[test]
    fn stops_at_track_ends() {
        let (t, [a, b, c, d, _]) = sample();
        assert_eq!(onion_skin_frames(&t, FrameIndex(0), 3, 1), [(1, b)]);
        assert_eq!(
            onion_skin_frames(&t, FrameIndex(30), 9, 9),
            [(-4, a), (-3, b), (-2, c), (-1, d)]
        );
        assert!(onion_skin_frames(&t, FrameIndex(10), 0, 0).is_empty());
        assert!(onion_skin_frames(&Track::new(), FrameIndex(3), 2, 2).is_empty());
    }

    #[test]
    fn before_first_key_only_shows_upcoming() {
        let mut t = Track::new();
        let (a, b) = (DrawingId::new(), DrawingId::new());
        t.set_key(FrameIndex(5), Keyframe::Drawing(a));
        t.set_key(FrameIndex(9), Keyframe::Drawing(b));
        assert_eq!(onion_skin_frames(&t, FrameIndex(2), 2, 2), [(1, a), (2, b)]);
    }
}
