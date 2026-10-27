//! Frame indices, frame ranges and frame rates.
//!
//! Convention: frames are **zero-based** internally ([`FrameIndex`]) and
//! **one-based** when shown to users ([`FrameIndex::display_number`]).

use serde::{Deserialize, Serialize};
use std::fmt;

/// A zero-based frame number on the timeline.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct FrameIndex(pub u32);

impl FrameIndex {
    /// The first frame.
    pub const ZERO: Self = Self(0);

    /// The one-based number shown in the UI (frame 0 is shown as "1").
    pub const fn display_number(self) -> u32 {
        self.0 + 1
    }

    /// Converts a one-based UI number back to an index. Returns `None` for 0.
    pub const fn from_display_number(n: u32) -> Option<Self> {
        match n.checked_sub(1) {
            Some(i) => Some(Self(i)),
            None => None,
        }
    }

    /// The next frame, saturating at `u32::MAX`.
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// The previous frame, saturating at zero.
    pub const fn prev(self) -> Self {
        Self(self.0.saturating_sub(1))
    }
}

impl fmt::Display for FrameIndex {
    /// Displays the one-based frame number.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_number())
    }
}

/// A half-open range of frames: `start..end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameRange {
    /// First frame in the range.
    pub start: FrameIndex,
    /// One past the last frame in the range.
    pub end: FrameIndex,
}

impl FrameRange {
    /// Creates a range. If `end < start` the range is empty.
    pub const fn new(start: FrameIndex, end: FrameIndex) -> Self {
        Self { start, end }
    }

    /// A range covering the first `count` frames.
    pub const fn first_n(count: u32) -> Self {
        Self::new(FrameIndex(0), FrameIndex(count))
    }

    /// Number of frames in the range.
    pub const fn len(&self) -> u32 {
        self.end.0.saturating_sub(self.start.0)
    }

    /// Whether the range has no frames.
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `frame` lies inside the range.
    pub fn contains(&self, frame: FrameIndex) -> bool {
        frame >= self.start && frame < self.end
    }
}

/// A frame rate as a rational number of frames per second.
///
/// Rational form keeps rates such as 23.976 (24000/1001) exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fps {
    numerator: u32,
    denominator: u32,
}

impl Fps {
    /// 24 frames per second.
    pub const FPS_24: Self = Self {
        numerator: 24,
        denominator: 1,
    };

    /// Creates a frame rate. Returns `None` if either part is zero.
    pub const fn new(numerator: u32, denominator: u32) -> Option<Self> {
        if numerator == 0 || denominator == 0 {
            None
        } else {
            Some(Self {
                numerator,
                denominator,
            })
        }
    }

    /// A whole-number frame rate. Returns `None` for zero.
    pub const fn whole(fps: u32) -> Option<Self> {
        Self::new(fps, 1)
    }

    /// Numerator of the rate.
    pub const fn numerator(&self) -> u32 {
        self.numerator
    }

    /// Denominator of the rate.
    pub const fn denominator(&self) -> u32 {
        self.denominator
    }

    /// Frames per second as a float.
    pub fn as_f64(&self) -> f64 {
        f64::from(self.numerator) / f64::from(self.denominator)
    }

    /// Duration of one frame in seconds.
    pub fn frame_duration_secs(&self) -> f64 {
        f64::from(self.denominator) / f64::from(self.numerator)
    }

    /// Start time of `frame` in seconds.
    pub fn frame_to_secs(&self, frame: FrameIndex) -> f64 {
        f64::from(frame.0) * self.frame_duration_secs()
    }

    /// The frame showing at `secs` seconds (floor). Negative times give frame 0.
    pub fn secs_to_frame(&self, secs: f64) -> FrameIndex {
        if !secs.is_finite() || secs <= 0.0 {
            return FrameIndex::ZERO;
        }
        // The small epsilon keeps exact frame boundaries from rounding down.
        let scaled = secs * f64::from(self.numerator);
        let frames = (scaled / f64::from(self.denominator) + 1e-9).floor();
        FrameIndex(frames.min(f64::from(u32::MAX)) as u32)
    }
}

impl Default for Fps {
    fn default() -> Self {
        Self::FPS_24
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_numbers_are_one_based() {
        assert_eq!(FrameIndex(0).display_number(), 1);
        assert_eq!(FrameIndex(0).to_string(), "1");
        assert_eq!(FrameIndex::from_display_number(1), Some(FrameIndex(0)));
        assert_eq!(FrameIndex::from_display_number(0), None);
    }

    #[test]
    fn stepping_saturates() {
        assert_eq!(FrameIndex(0).prev(), FrameIndex(0));
        assert_eq!(FrameIndex(u32::MAX).next(), FrameIndex(u32::MAX));
        assert_eq!(FrameIndex(4).next(), FrameIndex(5));
    }

    #[test]
    fn frame_range_contains() {
        let r = FrameRange::first_n(3);
        assert_eq!(r.len(), 3);
        assert!(r.contains(FrameIndex(0)) && r.contains(FrameIndex(2)));
        assert!(!r.contains(FrameIndex(3)));
        assert!(FrameRange::new(FrameIndex(5), FrameIndex(2)).is_empty());
    }

    #[test]
    fn fps_rejects_zero() {
        assert!(Fps::new(0, 1).is_none());
        assert!(Fps::new(24, 0).is_none());
    }

    #[test]
    fn time_conversion_roundtrips() {
        let fps = Fps::whole(24).unwrap();
        for f in [0, 1, 23, 24, 1000] {
            assert_eq!(
                fps.secs_to_frame(fps.frame_to_secs(FrameIndex(f))),
                FrameIndex(f)
            );
        }
        assert_eq!(fps.secs_to_frame(1.0), FrameIndex(24));
        assert_eq!(fps.secs_to_frame(0.999), FrameIndex(23));
        assert_eq!(fps.secs_to_frame(-1.0), FrameIndex(0));
    }

    #[test]
    fn ntsc_rate_is_exact() {
        let fps = Fps::new(24000, 1001).unwrap();
        assert!((fps.as_f64() - 23.976).abs() < 0.001);
        assert_eq!(
            fps.secs_to_frame(fps.frame_to_secs(FrameIndex(1001))),
            FrameIndex(1001)
        );
    }
}
