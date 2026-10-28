//! Vector drawing data (ADR-001).
//!
//! Drawings are stored as editable strokes in **document coordinates**, never
//! as screen pixels. Turning strokes into triangles (tessellation) also lives
//! in this crate so it can be tested without a GPU.
//!
//! - [`StrokeBuilder`] collects pointer input while the pen is down, drops
//!   near-duplicate points and smooths the result into a [`Stroke`].
//! - [`tessellate`] turns a [`Stroke`] into a [`Mesh`] (round joins and caps,
//!   pressure-aware width). Colour and opacity stay on the stroke's
//!   [`BrushStyle`]; see [`Mesh`] for how the renderer should blend.
//! - [`Stroke::hit_test`] and [`VectorDrawing::strokes_hit`] support the
//!   whole-stroke eraser.
//! - [`Stroke::erase`] cuts away the part of a stroke an eraser passes over
//!   (the partial eraser); see [`erase`] for the approximation used.

pub mod builder;
pub mod erase;
mod hit;
pub mod stroke;
pub mod tessellate;

pub use builder::StrokeBuilder;
pub use erase::EraseResult;
pub use stroke::{BrushStyle, DrawingContent, Stroke, StrokePoint, VectorDrawing};
pub use tessellate::{Mesh, TessellationOptions, tessellate, tessellate_with};
