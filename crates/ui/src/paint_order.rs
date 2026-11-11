//! Where a new brush stroke, shape or bucket fill goes in its drawing's
//! paint order (ADR-010, ADR-011).

use document::drawing::{Fill, Placement, Stroke, VectorDrawing};

/// The "Behind / Normal / In front" choice of the brush, shape and fill
/// tools.
///
/// Behind puts the new item on a new back level, under everything already
/// in the frame (fills included). Normal puts it on top of the normal tier.
/// In front puts it in the front tier, so it stays above everything drawn
/// in the normal tier, also later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaintOrder {
    /// Under all of the frame's existing items (a new, deepest back level).
    Behind,
    /// On top of the frame's existing items, but under front items.
    #[default]
    Normal,
    /// Always on top: above every normal item, including ones drawn later.
    InFront,
}

impl PaintOrder {
    /// Every choice, in menu order.
    pub const ALL: [Self; 3] = [Self::Behind, Self::Normal, Self::InFront];

    /// User-visible name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Behind => "Behind",
            Self::Normal => "Normal",
            Self::InFront => "In front",
        }
    }

    /// Hover help.
    pub const fn hint(self) -> &'static str {
        match self {
            Self::Behind => "Goes under the frame's existing strokes and fills.",
            Self::Normal => {
                "Goes on top of what is there; later drawing covers it. Items set to \
                 In front stay above it."
            }
            Self::InFront => {
                "Always on top: stays above everything else in the frame, also what you \
                 draw later. A shape's fill covers the lines under it."
            }
        }
    }

    /// The stored draw-order tier for an item added to `drawing` (the
    /// frame's drawing, `None` when the frame has none yet). Behind gives a
    /// back level deeper than any in `drawing`; a shape's fill and outline
    /// take the same placement.
    pub fn placement(self, drawing: Option<&VectorDrawing>) -> Placement {
        match self {
            Self::Behind => drawing.map_or(Placement::Back(1), VectorDrawing::behind_placement),
            Self::Normal => Placement::Normal,
            Self::InFront => Placement::Front,
        }
    }

    /// Whether the item goes at the bottom of its tier's list.
    pub const fn at_bottom(self) -> bool {
        matches!(self, Self::Behind)
    }
}

/// A copy of `drawing` with a preview's fill and stroke added where `order`
/// would commit them, so the live preview shows its final stacking.
pub fn with_preview(
    drawing: &VectorDrawing,
    fill: Option<Fill>,
    stroke: Option<Stroke>,
    order: PaintOrder,
) -> VectorDrawing {
    let placement = order.placement(Some(drawing));
    let mut copy = drawing.clone();
    if let Some(fill) = fill {
        copy.add_fill(fill.with_placement(placement), order.at_bottom());
    }
    if let Some(stroke) = stroke {
        copy.add_stroke(stroke.with_placement(placement), order.at_bottom());
    }
    copy
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::animation_core::{Rgba, Vec2};
    use document::drawing::{BrushStyle, FillShape, ItemId, StrokePoint};

    fn stroke() -> Stroke {
        Stroke::new(
            vec![StrokePoint::new(Vec2::ZERO), StrokePoint::new(Vec2::ONE)],
            BrushStyle::default(),
        )
    }

    fn fill() -> Fill {
        Fill::new(
            FillShape {
                contours: vec![vec![Vec2::ZERO, Vec2::X, Vec2::Y]],
            },
            Rgba::WHITE,
            1.0,
        )
    }

    #[test]
    fn preview_lands_where_the_commit_would() {
        let mut d = VectorDrawing::new();
        let existing = stroke();
        d.push_stroke(existing.clone());
        let (f, s) = (fill(), stroke());
        let at = |order| {
            with_preview(&d, Some(f.clone()), Some(s.clone()), order)
                .paint_order()
                .map(ItemId::from)
                .collect::<Vec<_>>()
        };
        let (f, s, e) = (
            ItemId::Fill(f.id),
            ItemId::Stroke(s.id),
            ItemId::Stroke(existing.id),
        );
        assert_eq!(at(PaintOrder::Behind), vec![f, s, e]);
        assert_eq!(at(PaintOrder::Normal), vec![f, e, s]);
        assert_eq!(at(PaintOrder::InFront), vec![e, f, s]);
    }
}
