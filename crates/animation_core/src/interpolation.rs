//! Interpolation kinds for property animation.

use serde::{Deserialize, Serialize};

/// How a property moves between two keyframes.
///
/// Drawing keyframes always use [`Interpolation::Step`]. The eased variants
/// use cubic curves. Bezier will be added later as a new variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Interpolation {
    /// Hold the previous value until the next keyframe.
    #[default]
    Step,
    /// Interpolate linearly between keyframes.
    Linear,
    /// Start slowly and speed up (cubic, `t³`).
    EaseIn,
    /// Start quickly and slow down (cubic, `1 − (1 − t)³`).
    EaseOut,
    /// Start and end slowly (cubic ease in for the first half, ease out for
    /// the second).
    EaseInOut,
}

impl Interpolation {
    /// The kinds a motion tween can use, in menu order.
    pub const TWEENS: [Self; 4] = [Self::Linear, Self::EaseIn, Self::EaseOut, Self::EaseInOut];

    /// Blend factor for progress `t` in `0.0..=1.0` between two keys. `t`
    /// is clamped, and every curve maps 0 to 0 and 1 to 1 (except `Step`,
    /// which holds at 0) and never decreases.
    pub fn factor(self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        match self {
            Self::Step => 0.0,
            Self::Linear => t,
            Self::EaseIn => t * t * t,
            Self::EaseOut => {
                let u = 1.0 - t;
                1.0 - u * u * u
            }
            Self::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    let u = -2.0 * t + 2.0;
                    1.0 - u * u * u / 2.0
                }
            }
        }
    }

    /// Short user-visible name, e.g. "Ease In-Out".
    pub fn label(self) -> &'static str {
        match self {
            Self::Step => "Step",
            Self::Linear => "Linear",
            Self::EaseIn => "Ease In",
            Self::EaseOut => "Ease Out",
            Self::EaseInOut => "Ease In-Out",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Interpolation; 5] = [
        Interpolation::Step,
        Interpolation::Linear,
        Interpolation::EaseIn,
        Interpolation::EaseOut,
        Interpolation::EaseInOut,
    ];

    #[test]
    fn step_holds_and_linear_follows() {
        assert_eq!(Interpolation::Step.factor(0.7), 0.0);
        assert_eq!(Interpolation::Linear.factor(0.7), 0.7);
        assert_eq!(Interpolation::Linear.factor(1.5), 1.0);
    }

    #[test]
    fn eased_curves_hit_their_endpoints() {
        for kind in Interpolation::TWEENS {
            assert_eq!(kind.factor(0.0), 0.0, "{kind:?} at 0");
            assert!((kind.factor(1.0) - 1.0).abs() < 1e-6, "{kind:?} at 1");
            // Out of range and NaN progress is clamped.
            assert_eq!(kind.factor(-1.0), 0.0, "{kind:?} below 0");
            assert!((kind.factor(2.0) - 1.0).abs() < 1e-6, "{kind:?} above 1");
            assert_eq!(kind.factor(f32::NAN), 0.0, "{kind:?} at NaN");
        }
    }

    #[test]
    fn eased_curves_have_their_shape() {
        // Ease in lags behind linear, ease out leads it, ease in-out crosses
        // it at the midpoint.
        assert!((Interpolation::EaseIn.factor(0.5) - 0.125).abs() < 1e-6);
        assert!((Interpolation::EaseOut.factor(0.5) - 0.875).abs() < 1e-6);
        assert!((Interpolation::EaseInOut.factor(0.5) - 0.5).abs() < 1e-6);
        assert!(Interpolation::EaseInOut.factor(0.25) < 0.25);
        assert!(Interpolation::EaseInOut.factor(0.75) > 0.75);
    }

    #[test]
    fn every_curve_only_rises() {
        for kind in ALL {
            let mut previous = kind.factor(0.0);
            for i in 1..=1000 {
                let value = kind.factor(i as f32 / 1000.0);
                assert!(value >= previous, "{kind:?} falls at step {i}");
                assert!((0.0..=1.0).contains(&value), "{kind:?} leaves 0..=1");
                previous = value;
            }
        }
    }

    #[test]
    fn labels_are_distinct() {
        let labels: std::collections::BTreeSet<_> = ALL.iter().map(|k| k.label()).collect();
        assert_eq!(labels.len(), ALL.len());
    }
}
