//! The command trait and undo/redo history.

use document::Document;
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

/// Undo and redo stacks.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Box<dyn Command>>,
    redo: Vec<Box<dyn Command>>,
}

impl History {
    /// Creates an empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies `command` and records it. Clears the redo stack.
    /// If the command fails, nothing is recorded.
    pub fn execute(
        &mut self,
        doc: &mut Document,
        mut command: Box<dyn Command>,
    ) -> Result<(), CommandError> {
        command.apply(doc)?;
        self.undo.push(command);
        self.redo.clear();
        Ok(())
    }

    /// Undoes the last command. Returns `Ok(false)` if there was nothing to undo.
    pub fn undo(&mut self, doc: &mut Document) -> Result<bool, CommandError> {
        let Some(mut command) = self.undo.pop() else {
            return Ok(false);
        };
        if let Err(e) = command.undo(doc) {
            self.undo.push(command);
            return Err(e);
        }
        self.redo.push(command);
        Ok(true)
    }

    /// Redoes the last undone command. Returns `Ok(false)` if there was nothing to redo.
    pub fn redo(&mut self, doc: &mut Document) -> Result<bool, CommandError> {
        let Some(mut command) = self.redo.pop() else {
            return Ok(false);
        };
        if let Err(e) = command.apply(doc) {
            self.redo.push(command);
            return Err(e);
        }
        self.undo.push(command);
        Ok(true)
    }

    /// Label of the command that [`History::undo`] would revert.
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|c| c.label())
    }

    /// Label of the command that [`History::redo`] would re-apply.
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|c| c.label())
    }

    /// Whether there is anything to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is anything to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}
