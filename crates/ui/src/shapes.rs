//! The rectangle and ellipse tools: their settings and geometry.
//!
//! A shape is dragged from one corner to the other (Shift makes it a square
//! or circle, Alt grows it from the press point as its centre). It is
//! committed as an ordinary [`Fill`] for the inside and a closed [`Stroke`]
//! for the outline, so it can be selected, transformed, erased and recoloured
//! like hand-drawn art.

use document::animation_core::{FillId, Rgba, StrokeId, Vec2};
use document::drawing::{
    BrushStyle, Fill, FillShape, Gradient, Stroke, StrokePoint, VectorDrawing,
};

use crate::paint_order::PaintOrder;

/// Smallest side of a shape, in document units. Smaller drags (a click)
/// draw nothing.
pub const MIN_SHAPE_SIZE: f32 = 0.5;

/// Which shape a shape tool draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    /// An axis-aligned rectangle.
    Rectangle,
    /// An axis-aligned ellipse.
    Ellipse,
}

impl ShapeKind {
    /// User-visible name, also the undo label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
        }
    }
}

/// Which way a shape's gradient fill runs across it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GradientDirection {
    /// Fill colour on the left, gradient colour on the right.
    #[default]
    LeftToRight,
    /// Fill colour at the top, gradient colour at the bottom.
    TopToBottom,
    /// Fill colour at the top-left corner, gradient colour at the
    /// bottom-right one.
    Diagonal,
}

impl GradientDirection {
    /// Every direction, in menu order.
    pub const ALL: [Self; 3] = [Self::LeftToRight, Self::TopToBottom, Self::Diagonal];

    /// User-visible name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::LeftToRight => "Left → right",
            Self::TopToBottom => "Top → bottom",
            Self::Diagonal => "Diagonal",
        }
    }

    /// The gradient's start and end points across the box `(min, max)`.
    pub fn endpoints(self, (min, max): (Vec2, Vec2)) -> (Vec2, Vec2) {
        let center = (min + max) * 0.5;
        match self {
            Self::LeftToRight => (Vec2::new(min.x, center.y), Vec2::new(max.x, center.y)),
            Self::TopToBottom => (Vec2::new(center.x, min.y), Vec2::new(center.x, max.y)),
            Self::Diagonal => (min, max),
        }
    }
}

/// Stroke and fill settings of the shape tools.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeStyle {
    /// Draw the outline.
    pub stroke: bool,
    /// Outline width in document units.
    pub stroke_width: f32,
    /// Outline colour.
    pub stroke_color: Rgba,
    /// Fill the inside.
    pub fill: bool,
    /// Fill colour (the start colour of a gradient fill).
    pub fill_color: Rgba,
    /// Blend the fill into `gradient_color` instead of a solid colour.
    pub gradient: bool,
    /// Colour the gradient fill blends into.
    pub gradient_color: Rgba,
    /// Which way the gradient runs.
    pub gradient_direction: GradientDirection,
    /// Where the shape's fill and outline go in the frame's paint order.
    pub order: PaintOrder,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        Self {
            stroke: true,
            stroke_width: 4.0,
            stroke_color: Rgba::BLACK,
            fill: true,
            fill_color: Rgba::WHITE,
            gradient: false,
            gradient_color: Rgba::BLACK,
            gradient_direction: GradientDirection::default(),
            order: PaintOrder::default(),
        }
    }
}

/// The box a drag from `start` to `end` makes, as `(min, max)`. `square`
/// makes both sides as long as the longer one; `centered` puts `start` in
/// the middle.
pub fn shape_box(start: Vec2, end: Vec2, square: bool, centered: bool) -> (Vec2, Vec2) {
    let mut delta = end - start;
    if square {
        let side = delta.x.abs().max(delta.y.abs());
        let sign = |v: f32| if v < 0.0 { -1.0 } else { 1.0 };
        delta = Vec2::new(sign(delta.x) * side, sign(delta.y) * side);
    }
    let (a, b) = if centered {
        (start - delta, start + delta)
    } else {
        (start, start + delta)
    };
    (a.min(b), a.max(b))
}

/// The outline of `kind` filling the box `(min, max)`, as a closed polygon
/// with positive signed area (clockwise on screen, as fills expect).
pub fn outline(kind: ShapeKind, (min, max): (Vec2, Vec2)) -> Vec<Vec2> {
    match kind {
        ShapeKind::Rectangle => vec![min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)],
        ShapeKind::Ellipse => {
            let center = (min + max) * 0.5;
            let radius = (max - min) * 0.5;
            // About one segment per 4 units of perimeter, within limits.
            let perimeter = std::f32::consts::TAU * ((radius.x + radius.y) * 0.5);
            let segments = ((perimeter / 4.0).ceil() as usize).clamp(32, 256);
            (0..segments)
                .map(|i| {
                    let angle = i as f32 / segments as f32 * std::f32::consts::TAU;
                    center + Vec2::new(angle.cos(), angle.sin()) * radius
                })
                .collect()
        }
    }
}

/// The fill and outline stroke for a shape in the box `bounds`, with the
/// given IDs (fixed ones for a live preview, fresh ones to commit), both in
/// the tier `style.order` picks for `drawing`, the frame's drawing they go
/// into (ADR-011). `None` for a part that is switched off, or for both when
/// the box is too small.
pub fn shape_items(
    kind: ShapeKind,
    bounds: (Vec2, Vec2),
    style: &ShapeStyle,
    (fill_id, stroke_id): (FillId, StrokeId),
    drawing: Option<&VectorDrawing>,
) -> (Option<Fill>, Option<Stroke>) {
    let placement = style.order.placement(drawing);
    let size = bounds.1 - bounds.0;
    if !(size.x >= MIN_SHAPE_SIZE || size.y >= MIN_SHAPE_SIZE) || !size.is_finite() {
        return (None, None);
    }
    let polygon = outline(kind, bounds);
    let fill = (style.fill && size.min_element() >= MIN_SHAPE_SIZE).then(|| {
        let mut fill = Fill::new(
            FillShape {
                contours: vec![polygon.clone()],
            },
            style.fill_color,
            1.0,
        );
        fill.id = fill_id;
        let gradient = style.gradient.then(|| {
            let (start, end) = style.gradient_direction.endpoints(bounds);
            Gradient {
                start,
                end,
                end_color: style.gradient_color,
            }
        });
        fill.with_gradient(gradient).with_placement(placement)
    });
    let stroke = style.stroke.then(|| {
        let mut points: Vec<StrokePoint> = polygon.iter().copied().map(StrokePoint::new).collect();
        points.push(StrokePoint::new(polygon[0]));
        Stroke {
            id: stroke_id,
            points,
            style: BrushStyle {
                color: style.stroke_color,
                width: style.stroke_width.max(0.1),
                opacity: 1.0,
            },
            placement,
        }
    });
    (fill, stroke)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_follows_drag_in_any_direction() {
        let b = shape_box(Vec2::new(10.0, 10.0), Vec2::new(0.0, 30.0), false, false);
        assert_eq!(b, (Vec2::new(0.0, 10.0), Vec2::new(10.0, 30.0)));
    }

    #[test]
    fn square_and_centered() {
        let b = shape_box(Vec2::ZERO, Vec2::new(-4.0, 10.0), true, false);
        assert_eq!(b, (Vec2::new(-10.0, 0.0), Vec2::new(0.0, 10.0)));
        let b = shape_box(Vec2::new(5.0, 5.0), Vec2::new(7.0, 6.0), false, true);
        assert_eq!(b, (Vec2::new(3.0, 4.0), Vec2::new(7.0, 6.0)));
    }

    #[test]
    fn outlines_have_positive_area() {
        let bounds = (Vec2::ZERO, Vec2::new(40.0, 20.0));
        for kind in [ShapeKind::Rectangle, ShapeKind::Ellipse] {
            let shape = FillShape {
                contours: vec![outline(kind, bounds)],
            };
            assert!(shape.area() > 0.0, "{kind:?}");
            assert!(shape.contains(Vec2::new(20.0, 10.0)));
        }
        let rect = FillShape {
            contours: vec![outline(ShapeKind::Rectangle, bounds)],
        };
        assert_eq!(rect.area(), 800.0);
    }

    #[test]
    fn gradient_fill_spans_the_box() {
        let bounds = (Vec2::new(10.0, 20.0), Vec2::new(50.0, 80.0));
        let style = ShapeStyle {
            gradient: true,
            gradient_direction: GradientDirection::TopToBottom,
            ..ShapeStyle::default()
        };
        let (fill, _) = shape_items(
            ShapeKind::Rectangle,
            bounds,
            &style,
            (FillId::new(), StrokeId::new()),
            None,
        );
        let g = fill.unwrap().gradient.unwrap();
        assert_eq!(g.start, Vec2::new(30.0, 20.0));
        assert_eq!(g.end, Vec2::new(30.0, 80.0));
        assert_eq!(g.end_color, style.gradient_color);
        let solid = shape_items(
            ShapeKind::Rectangle,
            bounds,
            &ShapeStyle::default(),
            (FillId::new(), StrokeId::new()),
            None,
        );
        assert!(solid.0.unwrap().gradient.is_none());
    }

    #[test]
    fn items_respect_toggles_and_size() {
        let bounds = (Vec2::ZERO, Vec2::new(10.0, 10.0));
        let style = ShapeStyle::default();
        let (fill, stroke) = shape_items(
            ShapeKind::Ellipse,
            bounds,
            &style,
            (FillId::new(), StrokeId::new()),
            None,
        );
        let stroke = stroke.unwrap();
        assert_eq!(stroke.points.first(), stroke.points.last(), "closed");
        assert_eq!(fill.unwrap().color, style.fill_color);

        let no_fill = ShapeStyle {
            fill: false,
            ..style
        };
        let (fill, stroke) = shape_items(
            ShapeKind::Rectangle,
            bounds,
            &no_fill,
            (FillId::new(), StrokeId::new()),
            None,
        );
        assert!(fill.is_none() && stroke.is_some());

        let tiny = (Vec2::ZERO, Vec2::splat(0.1));
        let (fill, stroke) = shape_items(
            ShapeKind::Rectangle,
            tiny,
            &style,
            (FillId::new(), StrokeId::new()),
            None,
        );
        assert!(fill.is_none() && stroke.is_none());
    }
}
