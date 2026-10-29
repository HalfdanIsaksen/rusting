//! The command trait and undo/redo history.

use animation_core::FrameIndex;
use document::Document;
use std::collections::VecDeque;
use std::fmt;

/// Why a command could not be applied or undone.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CommandError {
    /// The command referred to a layer that does not exist.
    #[error("layer not found")]
    LayerNotFound,
    /// The command referred to a drawing or stroke that does not exist.
    #[error("drawing content not found")]
    DrawingNotFound,
    /// The target layer is locked.
    #[error("layer is locked")]
    LayerLocked,
    /// A keyframe already exists at the frame.
    #[error("there is already a keyframe at frame {0}")]
    KeyframeExists(FrameIndex),
    /// There is no keyframe at the frame.
    #[error("there is no keyframe at frame {0}")]
    KeyframeNotFound(FrameIndex),
    /// The frame lies past the end of the animation.
    #[error("frame {frame} is past the end of the animation ({frame_count} frames)")]
    FrameOutOfRange {
        /// The requested frame.
        frame: FrameIndex,
        /// The document length at the time.
        frame_count: u32,
    },
    /// The animation must have at least one frame.
    #[error("the animation must have at least one frame")]
    ZeroFrameCount,
    /// The animation cannot grow past the largest frame index.
    #[error("the animation cannot have more frames")]
    FrameLimit,
    /// The canvas must be at least one pixel wide and high.
    #[error("the canvas must be at least 1×1 pixels")]
    EmptyCanvas,
}

/// An undoable change to a document.
///
/// `apply` and `undo` must be exact inverses: applying then undoing a command
/// leaves the document equal to how it was.
pub trait Command: fmt::Debug {
    /// Short, user-visible name, e.g. "Add Layer".
    fn label(&self) -> &str;
    /// Performs the change.
    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError>;
    /// Reverts the change.
    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError>;
}

/// Default maximum number of undo steps kept by [`History::new`].
pub const DEFAULT_UNDO_LIMIT: usize = 500;

/// A recorded command plus the token naming the document state after it.
#[derive(Debug)]
struct Entry {
    command: Box<dyn Command>,
    state_after: u64,
}

/// Undo and redo stacks, with a depth limit and a saved-state marker.
///
/// Every document state reachable through the history has a token. The app
/// calls [`History::mark_saved`] after saving; [`History::is_dirty`] then
/// reports whether the current state differs from the saved one, including
/// after undoing back past the save point.
#[derive(Debug)]
pub struct History {
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
    limit: usize,
    /// Token of the state below the oldest undo entry.
    base_state: u64,
    next_state: u64,
    saved_state: Option<u64>,
}

impl Default for History {
    fn default() -> Self {
        Self::with_limit(DEFAULT_UNDO_LIMIT)
    }
}

impl History {
    /// Creates an empty history keeping at most [`DEFAULT_UNDO_LIMIT`] undo
    /// steps. The current state counts as saved.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an empty history keeping at most `limit` undo steps. Older
    /// steps are dropped first. The current state counts as saved.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit,
            base_state: 0,
            next_state: 1,
            saved_state: Some(0),
        }
    }

    /// The maximum number of undo steps kept.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Changes the undo limit, dropping the oldest steps if needed.
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
        self.trim();
    }

    /// Applies `command` and records it. Clears the redo stack.
    /// If the command fails, nothing is recorded.
    pub fn execute(
        &mut self,
        doc: &mut Document,
        mut command: Box<dyn Command>,
    ) -> Result<(), CommandError> {
        command.apply(doc)?;
        let state_after = self.next_state;
        self.next_state += 1;
        self.undo.push_back(Entry {
            command,
            state_after,
        });
        self.redo.clear();
        self.trim();
        Ok(())
    }

    fn trim(&mut self) {
        while self.undo.len() > self.limit {
            if let Some(oldest) = self.undo.pop_front() {
                self.base_state = oldest.state_after;
            }
        }
    }

    /// Undoes the last command. Returns `Ok(false)` if there was nothing to undo.
    pub fn undo(&mut self, doc: &mut Document) -> Result<bool, CommandError> {
        let Some(mut entry) = self.undo.pop_back() else {
            return Ok(false);
        };
        if let Err(e) = entry.command.undo(doc) {
            self.undo.push_back(entry);
            return Err(e);
        }
        self.redo.push(entry);
        Ok(true)
    }

    /// Redoes the last undone command. Returns `Ok(false)` if there was nothing to redo.
    pub fn redo(&mut self, doc: &mut Document) -> Result<bool, CommandError> {
        let Some(mut entry) = self.redo.pop() else {
            return Ok(false);
        };
        if let Err(e) = entry.command.apply(doc) {
            self.redo.push(entry);
            return Err(e);
        }
        self.undo.push_back(entry);
        Ok(true)
    }

    /// Label of the command that [`History::undo`] would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.back().map(|e| e.command.label())
    }

    /// Label of the command that [`History::redo`] would re-apply.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|e| e.command.label())
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is anything to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Number of steps that can be undone.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Number of steps that can be redone.
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    fn current_state(&self) -> u64 {
        self.undo.back().map_or(self.base_state, |e| e.state_after)
    }

    /// Records the current document state as saved.
    pub fn mark_saved(&mut self) {
        self.saved_state = Some(self.current_state());
    }

    /// Records that no reachable state matches the file on disk, e.g. after the
    /// file was deleted or a save failed half way.
    pub fn mark_unsaved(&mut self) {
        self.saved_state = None;
    }

    /// Whether the document differs from the last saved state.
    pub fn is_dirty(&self) -> bool {
        self.saved_state != Some(self.current_state())
    }

    /// Drops all undo and redo steps (e.g. after opening another document).
    /// The current state counts as saved.
    pub fn clear(&mut self) {
        *self = Self::with_limit(self.limit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{AddLayer, RenameLayer};
    use document::CanvasSize;

    fn doc() -> Document {
        Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        )
    }

    #[test]
    fn limit_drops_oldest_steps() {
        let mut d = doc();
        let mut h = History::with_limit(3);
        for i in 0..5 {
            h.execute(&mut d, Box::new(AddLayer::new(format!("L{i}"))))
                .unwrap();
        }
        assert_eq!(h.undo_len(), 3);
        while h.undo(&mut d).unwrap() {}
        let names: Vec<_> = d.layers().iter().map(|l| l.name.clone()).collect();
        assert_eq!(names, ["L0", "L1"]);

        h.set_limit(1);
        assert_eq!(h.undo_len(), 0);
        assert_eq!(h.redo_len(), 3);
    }

    #[test]
    fn zero_limit_records_nothing_but_tracks_dirty() {
        let mut d = doc();
        let mut h = History::with_limit(0);
        h.execute(&mut d, Box::new(AddLayer::new("A"))).unwrap();
        assert!(!h.can_undo());
        assert!(h.is_dirty());
        h.mark_saved();
        assert!(!h.is_dirty());
    }

    #[test]
    fn dirty_tracks_the_saved_point() {
        let mut d = doc();
        let mut h = History::new();
        assert!(!h.is_dirty());
        let add = AddLayer::new("A");
        let id = add.layer_id();
        h.execute(&mut d, Box::new(add)).unwrap();
        assert!(h.is_dirty());
        h.mark_saved();
        assert!(!h.is_dirty());

        h.execute(&mut d, Box::new(RenameLayer::new(id, "B")))
            .unwrap();
        assert!(h.is_dirty());
        h.undo(&mut d).unwrap();
        assert!(!h.is_dirty());
        h.undo(&mut d).unwrap();
        assert!(h.is_dirty());
        h.redo(&mut d).unwrap();
        assert!(!h.is_dirty());

        // Branching off the saved state makes it unreachable through redo.
        h.undo(&mut d).unwrap();
        h.execute(&mut d, Box::new(AddLayer::new("C"))).unwrap();
        assert!(h.is_dirty());
        h.undo(&mut d).unwrap();
        assert!(h.is_dirty());

        h.mark_unsaved();
        h.clear();
        assert!(!h.is_dirty());
        assert!(!h.can_undo() && !h.can_redo());
    }

    #[test]
    fn saved_point_survives_trimming() {
        let mut d = doc();
        let mut h = History::with_limit(2);
        h.execute(&mut d, Box::new(AddLayer::new("A"))).unwrap();
        h.mark_saved();
        h.execute(&mut d, Box::new(AddLayer::new("B"))).unwrap();
        h.execute(&mut d, Box::new(AddLayer::new("C"))).unwrap();
        // "A" was trimmed; its state is now the base and still the saved one.
        h.undo(&mut d).unwrap();
        h.undo(&mut d).unwrap();
        assert!(!h.can_undo());
        assert!(!h.is_dirty());
    }
}
