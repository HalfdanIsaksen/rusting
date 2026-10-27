//! The animation timeline: tracks, keyframes and exposure sampling.
//!
//! This crate knows nothing about the UI or rendering. Drawing exposure is
//! *stepped*: a drawing keyframe stays visible until the next keyframe on the
//! same track (see ADR-003).

pub mod track;

pub use track::{Keyframe, Track};
