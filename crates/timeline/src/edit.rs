//! Frame editing on a [`Track`]: inserting and removing frames, moving keys.
//!
//! Every edit returns enough information to undo it, so the editor can wrap
//! these operations in undoable commands without inspecting the track first.

use crate::track::{Keyframe, Track};
use animation_core::{FrameIndex, FrameRange};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Why a frame edit was rejected. A rejected edit leaves the track unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum FrameEditError {
    /// Shifting keys would push one past the largest representable frame.
    #[error("shifting keys by {count} frames would overflow the frame index")]
    Overflow {
        /// The requested shift.
        count: u32,
    },
    /// There is no keyframe at the given frame.
    #[error("no keyframe at frame {0}")]
    NoKeyAt(FrameIndex),
}

/// The undo record for [`Track::remove_frames`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedFrames {
    /// The frames that were removed.
    pub range: FrameRange,
    /// The keys that sat inside `range`, at their original frames.
    pub keys: Vec<(FrameIndex, Keyframe)>,
}

/// The undo record for [`Track::move_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovedKey {
    /// Where the key was.
    pub from: FrameIndex,
    /// Where the key is now.
    pub to: FrameIndex,
    /// The key that was moved.
    pub key: Keyframe,
    /// The key previously at `to`, which the move overwrote.
    pub replaced: Option<Keyframe>,
}

impl Track {
    /// Inserts `count` blank frames at `at`: every key at or after `at` moves
    /// right by `count`. Keys before `at` are untouched, so the exposure that
    /// covers `at` is lengthened.
    ///
    /// Undo with [`Track::remove_frames`]`(at, count)`, which removes no keys.
    pub fn insert_frames(&mut self, at: FrameIndex, count: u32) -> Result<(), FrameEditError> {
        if count == 0 {
            return Ok(());
        }
        if let Some((last, _)) = self.keys.last_key_value()
            && *last >= at
            && last.0.checked_add(count).is_none()
        {
            return Err(FrameEditError::Overflow { count });
        }
        self.shift_right(at, count);
        Ok(())
    }

    /// Removes the frames `at..at + count`: keys inside that span are dropped
    /// and keys after it move left by `count`.
    ///
    /// Returns the dropped keys. Undo with [`Track::restore_frames`].
    pub fn remove_frames(&mut self, at: FrameIndex, count: u32) -> RemovedFrames {
        let end = FrameIndex(at.0.saturating_add(count));
        let range = FrameRange::new(at, end);
        if range.is_empty() {
            return RemovedFrames {
                range,
                keys: Vec::new(),
            };
        }
        let mut inside = self.keys.split_off(&at);
        let after = inside.split_off(&end);
        let shift = range.len();
        self.keys
            .extend(after.into_iter().map(|(f, k)| (FrameIndex(f.0 - shift), k)));
        RemovedFrames {
            range,
            keys: inside.into_iter().collect(),
        }
    }

    /// Reverses a [`Track::remove_frames`]: re-opens the removed span and puts
    /// its keys back.
    pub fn restore_frames(&mut self, removed: &RemovedFrames) {
        self.shift_right(removed.range.start, removed.range.len());
        self.keys.extend(removed.keys.iter().copied());
    }

    /// Moves the key at `from` to `to`, overwriting any key already at `to`.
    ///
    /// Undo with [`Track::undo_move_key`].
    pub fn move_key(
        &mut self,
        from: FrameIndex,
        to: FrameIndex,
    ) -> Result<MovedKey, FrameEditError> {
        let key = self
            .keys
            .remove(&from)
            .ok_or(FrameEditError::NoKeyAt(from))?;
        let replaced = self.keys.insert(to, key);
        Ok(MovedKey {
            from,
            to,
            key,
            replaced,
        })
    }

    /// Reverses a [`Track::move_key`].
    pub fn undo_move_key(&mut self, moved: &MovedKey) {
        self.keys.remove(&moved.to);
        if let Some(old) = moved.replaced {
            self.keys.insert(moved.to, old);
        }
        self.keys.insert(moved.from, moved.key);
    }

    /// Moves every key at or after `at` right by `count`, saturating at the
    /// largest frame. Callers check for overflow first when it matters.
    fn shift_right(&mut self, at: FrameIndex, count: u32) {
        if count == 0 {
            return;
        }
        let tail = self.keys.split_off(&at);
        self.keys.extend(
            tail.into_iter()
                .map(|(f, k)| (FrameIndex(f.0.saturating_add(count)), k)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use animation_core::DrawingId;

    fn frames(t: &Track) -> Vec<u32> {
        t.keys().map(|(f, _)| f.0).collect()
    }

    fn sample() -> Track {
        let mut t = Track::new();
        for f in [0, 3, 6, 9] {
            t.set_key(FrameIndex(f), Keyframe::Drawing(DrawingId::new()));
        }
        t
    }

    #[test]
    fn insert_frames_shifts_keys_at_and_after() {
        let mut t = sample();
        let before = t.clone();
        t.insert_frames(FrameIndex(3), 2).expect("insert");
        assert_eq!(frames(&t), [0, 5, 8, 11]);
        assert_eq!(t.key_at(FrameIndex(5)), before.key_at(FrameIndex(3)));

        // Undo: removing the inserted frames drops nothing.
        let removed = t.remove_frames(FrameIndex(3), 2);
        assert!(removed.keys.is_empty());
        assert_eq!(t, before);
    }

    #[test]
    fn insert_inside_exposure_lengthens_it() {
        let mut t = sample();
        t.insert_frames(FrameIndex(1), 4).expect("insert");
        assert_eq!(
            t.exposure_range(FrameIndex(0), 100),
            Some(FrameRange::first_n(7))
        );
    }

    #[test]
    fn insert_zero_or_past_last_key_is_noop() {
        let mut t = sample();
        let before = t.clone();
        t.insert_frames(FrameIndex(3), 0).expect("insert");
        t.insert_frames(FrameIndex(50), 10).expect("insert");
        assert_eq!(t, before);
    }

    #[test]
    fn insert_rejects_overflow_and_leaves_track_alone() {
        let mut t = sample();
        t.set_key(FrameIndex(u32::MAX - 1), Keyframe::Empty);
        let before = t.clone();
        assert_eq!(
            t.insert_frames(FrameIndex(0), 5),
            Err(FrameEditError::Overflow { count: 5 })
        );
        assert_eq!(t, before);
        // Inserting after the last key never overflows.
        assert!(t.insert_frames(FrameIndex(u32::MAX), 5).is_ok());
    }

    #[test]
    fn remove_frames_drops_keys_in_range_and_shifts_later_ones() {
        let mut t = sample();
        let before = t.clone();
        let removed = t.remove_frames(FrameIndex(2), 5); // frames 2..7
        assert_eq!(removed.range, FrameRange::new(FrameIndex(2), FrameIndex(7)));
        assert_eq!(
            removed.keys.iter().map(|(f, _)| f.0).collect::<Vec<_>>(),
            [3, 6]
        );
        assert_eq!(frames(&t), [0, 4]);
        assert_eq!(t.key_at(FrameIndex(4)), before.key_at(FrameIndex(9)));

        t.restore_frames(&removed);
        assert_eq!(t, before);
    }

    #[test]
    fn remove_frames_saturates_at_frame_limit() {
        let mut t = sample();
        let removed = t.remove_frames(FrameIndex(5), u32::MAX);
        assert_eq!(removed.keys.len(), 2);
        assert_eq!(frames(&t), [0, 3]);
        let empty = t.remove_frames(FrameIndex(1), 0);
        assert!(empty.range.is_empty() && empty.keys.is_empty());
        assert_eq!(frames(&t), [0, 3]);
    }

    #[test]
    fn move_key_and_undo() {
        let mut t = sample();
        let before = t.clone();

        let moved = t.move_key(FrameIndex(3), FrameIndex(4)).expect("move");
        assert_eq!(moved.replaced, None);
        assert_eq!(frames(&t), [0, 4, 6, 9]);
        t.undo_move_key(&moved);
        assert_eq!(t, before);

        // Moving onto an existing key overwrites it; undo brings it back.
        let moved = t.move_key(FrameIndex(3), FrameIndex(6)).expect("move");
        assert_eq!(moved.replaced, before.key_at(FrameIndex(6)).copied());
        assert_eq!(frames(&t), [0, 6, 9]);
        t.undo_move_key(&moved);
        assert_eq!(t, before);

        // Moving onto itself is a no-op that still undoes cleanly.
        let moved = t.move_key(FrameIndex(0), FrameIndex(0)).expect("move");
        t.undo_move_key(&moved);
        assert_eq!(t, before);
    }

    #[test]
    fn move_missing_key_errors() {
        let mut t = sample();
        assert_eq!(
            t.move_key(FrameIndex(1), FrameIndex(2)),
            Err(FrameEditError::NoKeyAt(FrameIndex(1)))
        );
    }
}
