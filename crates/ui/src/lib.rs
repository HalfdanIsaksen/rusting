//! Editor UI: the editing session and its egui panels.
//!
//! [`Session`] holds the document, undo history and editor state, and
//! implements the editor actions (tools, playback, files). The panels in
//! [`panels`] only read session state and call those actions; every document
//! change is an [`editor::Command`] run through [`editor::History`].
//!
//! The canvas itself is not drawn here (ADR-002): the host converts pointer
//! input to document coordinates and calls the tool methods on [`Session`],
//! and renders the canvas with [`OnionSkin::collect`] and
//! [`ToolController::preview`].

pub mod icons;
pub mod onion;
pub mod panels;
pub mod playback;
pub mod session;
pub mod tools;

pub use onion::{OnionDrawing, OnionSkin};
pub use panels::show;
pub use playback::Playback;
pub use session::{DiscardAction, ExportKind, Request, Session};
pub use tools::{EraserMode, ToolController};
