//! Canvas resizing geometry: which point of the canvas stays put.

use crate::CanvasSize;
use animation_core::Vec2;

/// The point a canvas resize grows or shrinks from.
///
/// Artwork keeps its position relative to this point: with
/// [`CanvasAnchor::TopLeft`] content stays where it is and the canvas changes
/// on the right and bottom; with [`CanvasAnchor::Center`] content moves by half
/// the size change so it stays centred.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CanvasAnchor {
    /// Top-left corner.
    #[default]
    TopLeft,
    /// Middle of the top edge.
    Top,
    /// Top-right corner.
    TopRight,
    /// Middle of the left edge.
    Left,
    /// Centre.
    Center,
    /// Middle of the right edge.
    Right,
    /// Bottom-left corner.
    BottomLeft,
    /// Middle of the bottom edge.
    Bottom,
    /// Bottom-right corner.
    BottomRight,
}

impl CanvasAnchor {
    /// All anchors in reading order (rows top to bottom, left to right), as
    /// laid out in a 3×3 picker.
    pub const ALL: [Self; 9] = [
        Self::TopLeft,
        Self::Top,
        Self::TopRight,
        Self::Left,
        Self::Center,
        Self::Right,
        Self::BottomLeft,
        Self::Bottom,
        Self::BottomRight,
    ];

    /// Column and row in the 3×3 grid: 0 = left/top, 1 = middle, 2 = right/bottom.
    pub fn grid_position(self) -> (u8, u8) {
        let index = Self::ALL.iter().position(|&a| a == self).unwrap_or(0) as u8;
        (index % 3, index / 3)
    }

    /// Short user-visible name, e.g. "Top left".
    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "Top left",
            Self::Top => "Top",
            Self::TopRight => "Top right",
            Self::Left => "Left",
            Self::Center => "Center",
            Self::Right => "Right",
            Self::BottomLeft => "Bottom left",
            Self::Bottom => "Bottom",
            Self::BottomRight => "Bottom right",
        }
    }

    /// How far artwork moves, in document units, when the canvas changes from
    /// `old` to `new` around this anchor (y points down).
    ///
    /// Offsets are whole pixels so artwork stays on the pixel grid: an odd
    /// size change around a middle anchor puts the extra pixel on the right
    /// or bottom when growing and takes it from there when shrinking.
    pub fn offset(self, old: CanvasSize, new: CanvasSize) -> Vec2 {
        let (col, row) = self.grid_position();
        Vec2::new(
            axis_offset(old.width, new.width, col),
            axis_offset(old.height, new.height, row),
        )
    }
}

/// `delta * position / 2`, rounded toward zero.
fn axis_offset(old: u32, new: u32, position: u8) -> f32 {
    let delta = i64::from(new) - i64::from(old);
    (delta * i64::from(position) / 2) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size(width: u32, height: u32) -> CanvasSize {
        CanvasSize { width, height }
    }

    #[test]
    fn corners_and_center_when_growing() {
        let (old, new) = (size(100, 50), size(300, 150));
        assert_eq!(CanvasAnchor::TopLeft.offset(old, new), Vec2::new(0.0, 0.0));
        assert_eq!(
            CanvasAnchor::TopRight.offset(old, new),
            Vec2::new(200.0, 0.0)
        );
        assert_eq!(
            CanvasAnchor::BottomLeft.offset(old, new),
            Vec2::new(0.0, 100.0)
        );
        assert_eq!(
            CanvasAnchor::BottomRight.offset(old, new),
            Vec2::new(200.0, 100.0)
        );
        assert_eq!(
            CanvasAnchor::Center.offset(old, new),
            Vec2::new(100.0, 50.0)
        );
    }

    #[test]
    fn edges_when_growing() {
        let (old, new) = (size(100, 50), size(300, 150));
        assert_eq!(CanvasAnchor::Top.offset(old, new), Vec2::new(100.0, 0.0));
        assert_eq!(
            CanvasAnchor::Bottom.offset(old, new),
            Vec2::new(100.0, 100.0)
        );
        assert_eq!(CanvasAnchor::Left.offset(old, new), Vec2::new(0.0, 50.0));
        assert_eq!(CanvasAnchor::Right.offset(old, new), Vec2::new(200.0, 50.0));
    }

    #[test]
    fn shrinking_moves_content_the_other_way() {
        let (old, new) = (size(300, 150), size(100, 50));
        assert_eq!(CanvasAnchor::TopLeft.offset(old, new), Vec2::new(0.0, 0.0));
        assert_eq!(
            CanvasAnchor::BottomRight.offset(old, new),
            Vec2::new(-200.0, -100.0)
        );
        assert_eq!(
            CanvasAnchor::Center.offset(old, new),
            Vec2::new(-100.0, -50.0)
        );
    }

    #[test]
    fn odd_changes_round_to_whole_pixels() {
        assert_eq!(
            CanvasAnchor::Center.offset(size(100, 100), size(101, 103)),
            Vec2::new(0.0, 1.0)
        );
        assert_eq!(
            CanvasAnchor::Center.offset(size(101, 103), size(100, 100)),
            Vec2::new(0.0, -1.0)
        );
        // Corners never need rounding.
        assert_eq!(
            CanvasAnchor::BottomRight.offset(size(100, 100), size(101, 99)),
            Vec2::new(1.0, -1.0)
        );
    }

    #[test]
    fn same_size_never_moves() {
        let s = size(640, 480);
        for anchor in CanvasAnchor::ALL {
            assert_eq!(anchor.offset(s, s), Vec2::ZERO, "{anchor:?}");
        }
    }

    #[test]
    fn grid_positions_follow_reading_order() {
        assert_eq!(CanvasAnchor::TopLeft.grid_position(), (0, 0));
        assert_eq!(CanvasAnchor::Right.grid_position(), (2, 1));
        assert_eq!(CanvasAnchor::Bottom.grid_position(), (1, 2));
        assert_eq!(CanvasAnchor::BottomRight.grid_position(), (2, 2));
    }
}
