//! Instance keys: a symbol (a reusable animated object) placed on a track
//! (ADR-018).
//!
//! An [`InstanceKey`] is exposed like any other key, from its frame until
//! the next key. While it is exposed it plays the symbol's own animation:
//! frame `f` of the track shows inner frame
//! `first_frame + (f - key frame)`, wrapped around the symbol's length when
//! [`InstanceMode::Loop`], or held on its last frame when
//! [`InstanceMode::Once`] (see [`InstanceKey::inner_frame`]).

use animation_core::glam::Affine2;
use animation_core::{FrameIndex, Interpolation, SymbolId, Vec2};
use serde::{Deserialize, Serialize};

/// How an instance plays its symbol's animation past the symbol's end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InstanceMode {
    /// Start over from the symbol's first frame.
    #[default]
    Loop,
    /// Hold the symbol's last frame.
    Once,
}

impl InstanceMode {
    /// Short user-visible name, e.g. "Loop".
    pub fn label(self) -> &'static str {
        match self {
            Self::Loop => "Loop",
            Self::Once => "Play once",
        }
    }
}

/// Where an instance sits, kept decomposed (not as a matrix) so two keys can
/// be blended part by part: `to_affine` scales, then rotates, then
/// translates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstanceTransform {
    /// Offset in the parent's space (document units at the top level).
    pub translation: Vec2,
    /// Scale factor per axis.
    pub scale: Vec2,
    /// Rotation in radians, as in `Affine2::from_angle` (clockwise on the
    /// y-down canvas).
    pub rotation: f32,
}

impl InstanceTransform {
    /// No change: the symbol's contents land where they were drawn.
    pub const IDENTITY: Self = Self {
        translation: Vec2::ZERO,
        scale: Vec2::ONE,
        rotation: 0.0,
    };

    /// A pure translation.
    pub fn from_translation(translation: Vec2) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    /// The transform as a matrix mapping symbol space to the parent's space.
    pub fn to_affine(&self) -> Affine2 {
        Affine2::from_scale_angle_translation(self.scale, self.rotation, self.translation)
    }

    /// The decomposed form of `affine`. Shear cannot be represented and is
    /// dropped (an approximation used after free transforms).
    pub fn from_affine(affine: &Affine2) -> Self {
        let (scale, rotation, translation) = affine.to_scale_angle_translation();
        let t = Self {
            translation,
            scale,
            rotation,
        };
        if t.is_finite() { t } else { Self::IDENTITY }
    }

    /// Blends part by part: `t = 0` gives `self`, `t = 1` gives `other`.
    /// Rotation takes the direct path between the two angles as stored, so
    /// a full turn stored as `2π` spins once.
    pub fn lerp(&self, other: &Self, t: f32) -> Self {
        Self {
            translation: self.translation.lerp(other.translation, t),
            scale: self.scale.lerp(other.scale, t),
            rotation: self.rotation + (other.rotation - self.rotation) * t,
        }
    }

    /// Whether every component is finite.
    pub fn is_finite(&self) -> bool {
        self.translation.is_finite() && self.scale.is_finite() && self.rotation.is_finite()
    }
}

impl Default for InstanceTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// A key that shows a symbol's animation from its frame on.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstanceKey {
    /// The symbol shown.
    pub symbol: SymbolId,
    /// Symbol-to-parent placement.
    pub transform: InstanceTransform,
    /// Opacity in `0..=1`, multiplied into the symbol's own layers.
    pub opacity: f32,
    /// Loop or play once.
    pub mode: InstanceMode,
    /// The symbol frame shown on the key's own frame.
    pub first_frame: FrameIndex,
    /// A motion tween from this key to the next key on the track (ADR-019).
    /// It only takes effect while the next key is an instance of the same
    /// symbol; otherwise this key holds still. Never `Some(Step)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tween: Option<Interpolation>,
}

impl InstanceKey {
    /// An instance of `symbol` at the identity transform, fully opaque,
    /// looping from the symbol's first frame.
    pub fn new(symbol: SymbolId) -> Self {
        Self {
            symbol,
            transform: InstanceTransform::IDENTITY,
            opacity: 1.0,
            mode: InstanceMode::Loop,
            first_frame: FrameIndex(0),
            tween: None,
        }
    }

    /// The same key with another transform.
    pub fn with_transform(mut self, transform: InstanceTransform) -> Self {
        self.transform = transform;
        self
    }

    /// This key blended toward `end` by `factor` (0 gives `self`, 1 gives
    /// `end`'s placement): transform and opacity blend, while the symbol,
    /// playback (mode, first frame) and tween stay this key's, so the
    /// object keeps animating through a motion tween.
    pub fn blended(&self, end: &InstanceKey, factor: f32) -> Self {
        Self {
            transform: self.transform.lerp(&end.transform, factor),
            opacity: self.opacity + (end.opacity - self.opacity) * factor,
            ..*self
        }
    }

    /// The symbol frame shown `elapsed` frames after the key's own frame,
    /// for a symbol `length` frames long. `None` if the symbol has no
    /// frames.
    ///
    /// A 12-frame symbol exposed for 18 frames from `first_frame` 0 shows
    /// inner frames 0‥11 then 0‥5 when looping, and 0‥11 then 11 six times
    /// when playing once.
    pub fn inner_frame(&self, elapsed: u32, length: u32) -> Option<FrameIndex> {
        if length == 0 {
            return None;
        }
        let first = u64::from(self.first_frame.0.min(length - 1));
        let raw = first + u64::from(elapsed);
        let inner = match self.mode {
            InstanceMode::Loop => raw % u64::from(length),
            InstanceMode::Once => raw.min(u64::from(length - 1)),
        };
        // `inner < length`, so it fits a u32.
        Some(FrameIndex(u32::try_from(inner).unwrap_or(length - 1)))
    }

    /// This key as it would be if it started `elapsed` frames later and
    /// continued the same playback: same symbol and placement, with
    /// `first_frame` advanced. Used to split an exposure into two keys.
    pub fn advanced(&self, elapsed: u32, length: u32) -> Self {
        Self {
            first_frame: self
                .inner_frame(elapsed, length)
                .unwrap_or(self.first_frame),
            ..*self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(key: &InstanceKey, exposed: u32, length: u32) -> Vec<u32> {
        (0..exposed)
            .map(|e| key.inner_frame(e, length).unwrap().0)
            .collect()
    }

    #[test]
    fn twelve_frame_symbol_over_eighteen_frames_loops_one_and_a_half_times() {
        let key = InstanceKey::new(SymbolId::new());
        let expected: Vec<u32> = (0..12).chain(0..6).collect();
        assert_eq!(frames(&key, 18, 12), expected);
    }

    #[test]
    fn play_once_holds_the_last_frame() {
        let key = InstanceKey {
            mode: InstanceMode::Once,
            ..InstanceKey::new(SymbolId::new())
        };
        let expected: Vec<u32> = (0..12).chain([11; 6]).collect();
        assert_eq!(frames(&key, 18, 12), expected);
    }

    #[test]
    fn first_frame_offsets_playback() {
        let mut key = InstanceKey::new(SymbolId::new());
        key.first_frame = FrameIndex(10);
        assert_eq!(frames(&key, 4, 12), [10, 11, 0, 1]);
        key.mode = InstanceMode::Once;
        assert_eq!(frames(&key, 4, 12), [10, 11, 11, 11]);
        // Out of range first frames are clamped, and empty symbols show nothing.
        key.first_frame = FrameIndex(50);
        assert_eq!(key.inner_frame(0, 12), Some(FrameIndex(11)));
        assert_eq!(key.inner_frame(0, 0), None);
        // Huge elapsed counts do not overflow.
        key.mode = InstanceMode::Loop;
        assert!(key.inner_frame(u32::MAX, u32::MAX).is_some());
    }

    #[test]
    fn advanced_continues_playback() {
        let key = InstanceKey::new(SymbolId::new());
        let later = key.advanced(5, 12);
        assert_eq!(later.first_frame, FrameIndex(5));
        assert_eq!(later.inner_frame(3, 12), key.inner_frame(8, 12));
        assert_eq!(key.advanced(14, 12).first_frame, FrameIndex(2));
    }

    #[test]
    fn transform_maps_scale_then_rotate_then_translate() {
        let t = InstanceTransform {
            translation: Vec2::new(10.0, 5.0),
            scale: Vec2::new(2.0, 3.0),
            rotation: std::f32::consts::FRAC_PI_2,
        };
        let p = t.to_affine().transform_point2(Vec2::new(1.0, 1.0));
        // (1,1) → scale (2,3) → rotate 90° (-3,2) → translate (7,7).
        assert!((p - Vec2::new(7.0, 7.0)).length() < 1e-5);
        let back = InstanceTransform::from_affine(&t.to_affine());
        assert!((back.translation - t.translation).length() < 1e-5);
        assert!((back.scale - t.scale).length() < 1e-5);
        assert!((back.rotation - t.rotation).abs() < 1e-5);
    }

    #[test]
    fn lerp_blends_each_part() {
        let a = InstanceTransform::IDENTITY;
        let b = InstanceTransform {
            translation: Vec2::new(10.0, 0.0),
            scale: Vec2::splat(3.0),
            rotation: 1.0,
        };
        assert_eq!(a.lerp(&b, 0.0), a);
        assert_eq!(a.lerp(&b, 1.0), b);
        let mid = a.lerp(&b, 0.5);
        assert_eq!(mid.translation, Vec2::new(5.0, 0.0));
        assert_eq!(mid.scale, Vec2::splat(2.0));
        assert_eq!(mid.rotation, 0.5);
    }
}
