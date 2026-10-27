//! The animation timeline: tracks, keyframes, exposure, frame editing,
//! playback timing and onion-skin queries.
//!
//! This crate knows nothing about the UI or rendering. Drawing exposure is
//! *stepped*: a drawing keyframe stays visible until the next keyframe on the
//! same track (see ADR-003). Playback derives frames from wall-clock time
//! (see ADR-005).

pub mod edit;
pub mod onion;
pub mod playback;
pub mod track;

pub use edit::{FrameEditError, MovedKey, RemovedFrames};
pub use onion::onion_skin_frames;
pub use playback::{ManualTimeSource, PlaybackClock, SystemTimeSource, Tick, TimeSource};
pub use track::{Keyframe, Track};
