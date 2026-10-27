//! Linear RGBA color.

use serde::{Deserialize, Serialize};

/// A straight (non-premultiplied) RGBA color with components in `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rgba {
    /// Red.
    pub r: f32,
    /// Green.
    pub g: f32,
    /// Blue.
    pub b: f32,
    /// Alpha (opacity).
    pub a: f32,
}

impl Rgba {
    /// Opaque black.
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);
    /// Opaque white.
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
    /// Fully transparent.
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);

    /// Creates a color from components.
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Creates an opaque color.
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::new(r, g, b, 1.0)
    }

    /// Returns the color with alpha multiplied by `opacity` (clamped to 0..=1).
    pub fn with_opacity(self, opacity: f32) -> Self {
        Self {
            a: self.a * opacity.clamp(0.0, 1.0),
            ..self
        }
    }

    /// Components as an array, `[r, g, b, a]`.
    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_multiplies_alpha() {
        assert_eq!(Rgba::new(1.0, 0.0, 0.0, 0.5).with_opacity(0.5).a, 0.25);
        assert_eq!(Rgba::WHITE.with_opacity(2.0).a, 1.0);
    }
}
