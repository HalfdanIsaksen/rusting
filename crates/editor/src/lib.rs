//! Editing behaviour: editor state and the undoable command system.
//!
//! Every change to the [`document::Document`] goes through a [`Command`] run by
//! [`History`], so it can be undone and redone. UI code never edits the
//! document directly.

pub mod command;
pub mod commands;
pub mod state;

pub use command::{Command, CommandError, History};
pub use state::{EditorState, Tool};
