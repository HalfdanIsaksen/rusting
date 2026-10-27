//! The project document: canvas settings, layers, tracks and drawings.
//!
//! [`Document::evaluate`] turns the document into a [`RenderList`] for a
//! given frame. That is the only thing the renderer needs to know about
//! animation, which keeps timeline logic out of the rendering code.

pub mod document;
pub mod evaluate;

pub use document::{CanvasSize, Document, RemovedLayer};
pub use evaluate::{RenderLayer, RenderList};

// Re-exported so downstream crates use exactly the same types.
pub use animation_core;
pub use drawing;
pub use scene;
pub use timeline;
