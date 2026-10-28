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
//! - [`fill_region`] finds the area enclosed by strokes around a point (the
//!   fill bucket), optionally closing small gaps, and returns it as vector
//!   contours; [`tessellate_fill`] turns a [`Fill`] into a [`Mesh`].

pub mod builder;
pub mod fill;
mod fill_tessellate;
mod hit;
pub mod stroke;
pub mod tessellate;

pub use builder::StrokeBuilder;
pub use fill::{Fill, FillError, FillShape, GapClosing, fill_region};
pub use fill_tessellate::tessellate_fill;
pub use stroke::{BrushStyle, DrawingContent, Stroke, StrokePoint, VectorDrawing};
pub use tessellate::{Mesh, TessellationOptions, tessellate, tessellate_with};
