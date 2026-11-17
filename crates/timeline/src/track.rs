//! Per-layer drawing tracks.

use crate::instance::InstanceKey;
use animation_core::{DrawingId, FrameIndex, FrameRange, Interpolation, SymbolId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::Bound;

/// A keyframe on a drawing track.
///
/// Not `Eq`: instance keys hold float placements.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Keyframe {
    /// Show the given drawing from this frame on.
    Drawing(DrawingId),
    /// Show nothing from this frame on (an empty drawing keyframe).
    Empty,
    /// Play a symbol's animation from this frame on (ADR-018).
    Instance(InstanceKey),
}

impl Keyframe {
    /// The drawing this keyframe shows, if any. Instance keys show no
    /// drawing of their own.
    pub fn drawing(&self) -> Option<DrawingId> {
        match self {
            Self::Drawing(id) => Some(*id),
            Self::Empty | Self::Instance(_) => None,
        }
    }

    /// The instance this keyframe places, if it is an instance key.
    pub fn instance(&self) -> Option<&InstanceKey> {
        match self {
            Self::Instance(key) => Some(key),
            Self::Drawing(_) | Self::Empty => None,
        }
    }

    /// The symbol this keyframe places, if it is an instance key.
    pub fn symbol(&self) -> Option<SymbolId> {
        self.instance().map(|key| key.symbol)
    }
}

/// An active motion tween between two instance keys of the same symbol
/// (ADR-019): frames `start..end` blend from the key on `start` toward the
/// key on `end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TweenSpan {
    /// The first key's frame (the key holding the tween).
    pub start: FrameIndex,
    /// The second key's frame.
    pub end: FrameIndex,
    /// The easing curve.
    pub ease: Interpolation,
}

impl TweenSpan {
    /// The eased blend factor at `frame`: 0 on `start`, rising toward 1 on
    /// `end` (clamped outside the span).
    pub fn factor(&self, frame: FrameIndex) -> f32 {
        let length = self.end.0.saturating_sub(self.start.0);
        if length == 0 {
            return 0.0;
        }
        let elapsed = frame.0.saturating_sub(self.start.0);
        self.ease.factor(elapsed as f32 / length as f32)
    }

    /// Whether `frame` lies in `start..end`.
    pub fn contains(&self, frame: FrameIndex) -> bool {
        self.start <= frame && frame < self.end
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

    /// The instance key exposed at `frame` and the frame it sits on, if the
    /// exposed key is an instance.
    pub fn instance_at(&self, frame: FrameIndex) -> Option<(FrameIndex, &InstanceKey)> {
        self.exposed_key(frame)
            .and_then(|(f, k)| k.instance().map(|i| (f, i)))
    }

    /// The motion tween starting at the key on `frame` (ADR-019), if that
    /// key is an instance with a tween and the next key is an instance of
    /// the same symbol. A tween whose pair is broken is inactive: `None`.
    pub fn tween_from(&self, frame: FrameIndex) -> Option<TweenSpan> {
        let start = self.key_at(frame)?.instance()?;
        let ease = start.tween?;
        let (end, next) = self.next_key_after(frame)?;
        let next = next.instance()?;
        (next.symbol == start.symbol).then_some(TweenSpan {
            start: frame,
            end,
            ease,
        })
    }

    /// The active motion tween whose span `start..end` holds `frame`.
    pub fn tween_at(&self, frame: FrameIndex) -> Option<TweenSpan> {
        let (start, _) = self.exposed_key(frame)?;
        self.tween_from(start)
    }

    /// Every active motion tween, in frame order.
    pub fn tweens(&self) -> impl Iterator<Item = TweenSpan> + '_ {
        self.keys.keys().filter_map(|&f| self.tween_from(f))
    }

    /// The instance shown at `frame` as it is displayed: the exposed
    /// instance key with its frame, its placement and opacity blended
    /// toward the next key when a motion tween is active there. Playback
    /// (`first_frame`, mode) stays the exposed key's.
    pub fn displayed_instance(&self, frame: FrameIndex) -> Option<(FrameIndex, InstanceKey)> {
        let (at, key) = self.instance_at(frame)?;
        let Some(span) = self.tween_from(at) else {
            return Some((at, *key));
        };
        let end = self.key_at(span.end).and_then(Keyframe::instance)?;
        Some((at, key.blended(end, span.factor(frame))))
    }

    /// Every key with its frame, mutably (to edit instance placements in
    /// place, e.g. when the canvas is resized).
    pub fn keys_mut(&mut self) -> impl Iterator<Item = (FrameIndex, &mut Keyframe)> {
        self.keys.iter_mut().map(|(f, k)| (*f, k))
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

    mod tweens {
        use super::*;
        use crate::{InstanceTransform, instance::InstanceKey};
        use animation_core::Vec2;

        /// Instance keys of one symbol on frames 0 (x = 0, opacity 1,
        /// tweened with `ease`) and 10 (x = 100, opacity 0).
        fn pair(ease: Interpolation) -> (Track, SymbolId) {
            let symbol = SymbolId::new();
            let mut t = Track::new();
            let start = InstanceKey {
                tween: Some(ease),
                ..InstanceKey::new(symbol)
            };
            let end = InstanceKey {
                opacity: 0.0,
                ..InstanceKey::new(symbol).with_transform(InstanceTransform {
                    translation: Vec2::new(100.0, 0.0),
                    scale: Vec2::splat(3.0),
                    rotation: 2.0,
                })
            };
            t.set_key(FrameIndex(0), Keyframe::Instance(start));
            t.set_key(FrameIndex(10), Keyframe::Instance(end));
            (t, symbol)
        }

        fn x_at(t: &Track, frame: u32) -> f32 {
            t.displayed_instance(FrameIndex(frame))
                .unwrap()
                .1
                .transform
                .translation
                .x
        }

        #[test]
        fn each_ease_blends_start_middle_and_end() {
            // (ease, value at the midpoint frame 5)
            let cases = [
                (Interpolation::Linear, 50.0),
                (Interpolation::EaseIn, 12.5),
                (Interpolation::EaseOut, 87.5),
                (Interpolation::EaseInOut, 50.0),
            ];
            for (ease, middle) in cases {
                let (t, _) = pair(ease);
                assert_eq!(x_at(&t, 0), 0.0, "{ease:?} start");
                assert!((x_at(&t, 5) - middle).abs() < 1e-3, "{ease:?} middle");
                // The second key shows its own placement exactly.
                assert_eq!(x_at(&t, 10), 100.0, "{ease:?} end");
                // Every part blends: scale, rotation and opacity too.
                let (at, mid) = t.displayed_instance(FrameIndex(5)).unwrap();
                assert_eq!(at, FrameIndex(0));
                let f = ease.factor(0.5);
                assert!((mid.transform.scale.x - (1.0 + 2.0 * f)).abs() < 1e-5);
                assert!((mid.transform.rotation - 2.0 * f).abs() < 1e-5);
                assert!((mid.opacity - (1.0 - f)).abs() < 1e-5);
                assert_eq!(mid.tween, Some(ease));
                // Just before the end it is nearly there, and never past it.
                let last = x_at(&t, 9);
                assert!(last > middle - 1e-3 && last < 100.0, "{ease:?} frame 9");
            }
        }

        #[test]
        fn span_and_lookup() {
            let (t, _) = pair(Interpolation::EaseIn);
            let span = t.tween_from(FrameIndex(0)).unwrap();
            assert_eq!((span.start, span.end), (FrameIndex(0), FrameIndex(10)));
            assert_eq!(t.tween_at(FrameIndex(7)), Some(span));
            assert!(span.contains(FrameIndex(9)) && !span.contains(FrameIndex(10)));
            assert_eq!(t.tween_at(FrameIndex(10)), None);
            assert_eq!(t.tween_from(FrameIndex(10)), None);
            assert_eq!(t.tweens().collect::<Vec<_>>(), [span]);
        }

        #[test]
        fn broken_pairs_leave_the_tween_inactive() {
            // Another object as the next key.
            let (mut t, symbol) = pair(Interpolation::Linear);
            let other = InstanceKey::new(SymbolId::new());
            t.set_key(FrameIndex(10), Keyframe::Instance(other));
            assert_eq!(t.tween_at(FrameIndex(5)), None);
            assert_eq!(x_at(&t, 5), 0.0);
            // A drawing or empty key, or no next key at all.
            t.set_key(FrameIndex(10), Keyframe::Empty);
            assert_eq!(x_at(&t, 5), 0.0);
            t.remove_key(FrameIndex(10));
            assert_eq!(x_at(&t, 5), 0.0);
            // The tween is still stored and works again once the pair is.
            assert_eq!(
                t.key_at(FrameIndex(0))
                    .and_then(Keyframe::instance)
                    .unwrap()
                    .tween,
                Some(Interpolation::Linear)
            );
            let end = InstanceKey::new(symbol)
                .with_transform(InstanceTransform::from_translation(Vec2::new(40.0, 0.0)));
            t.set_key(FrameIndex(4), Keyframe::Instance(end));
            assert_eq!(x_at(&t, 2), 20.0);
        }
    }
}
