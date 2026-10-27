//! Vector drawing data (ADR-001).
//!
//! Drawings are stored as editable strokes in **document coordinates**, never
//! as screen pixels. Turning strokes into triangles (tessellation) also lives
//! in this crate so it can be tested without a GPU.

pub mod stroke;

pub use stroke::{BrushStyle, DrawingContent, Stroke, StrokePoint, VectorDrawing};
