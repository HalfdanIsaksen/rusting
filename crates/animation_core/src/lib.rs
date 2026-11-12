//! Shared animation primitives used by every other Rusting crate.
//!
//! This crate is the single source of truth for frame numbers, frame rates,
//! stable identifiers, colors and interpolation kinds. Other crates must not
//! define their own versions of these types (see ADR-003 and ADR-004).

pub mod color;
pub mod frame;
pub mod ids;
pub mod interpolation;

pub use color::Rgba;
pub use frame::{Fps, FrameIndex, FrameRange};
pub use ids::{DocumentId, DrawingId, FillId, GroupId, LayerId, StrokeId};
pub use interpolation::Interpolation;

/// Re-export of the math library so every crate uses the same version.
pub use glam;
pub use glam::Vec2;
