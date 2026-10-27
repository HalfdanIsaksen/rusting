//! Interpolation kinds for property animation.

use serde::{Deserialize, Serialize};

/// How a property moves between two keyframes.
///
/// Drawing keyframes always use [`Interpolation::Step`]. Bezier will be added
/// later as a new variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Interpolation {
    /// Hold the previous value until the next keyframe.
    #[default]
    Step,
    /// Interpolate linearly between keyframes.
    Linear,
}

impl Interpolation {
    /// Blend factor for progress `t` in `0.0..=1.0` between two keys.
    pub fn factor(self, t: f32) -> f32 {
        match self {
            Self::Step => 0.0,
            Self::Linear => t.clamp(0.0, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_holds_and_linear_follows() {
        assert_eq!(Interpolation::Step.factor(0.7), 0.0);
        assert_eq!(Interpolation::Linear.factor(0.7), 0.7);
        assert_eq!(Interpolation::Linear.factor(1.5), 1.0);
    }
}
