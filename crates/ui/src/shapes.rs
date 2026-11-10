//! The rectangle and ellipse tools: their settings and geometry.
//!
//! A shape is dragged from one corner to the other (Shift makes it a square
//! or circle, Alt grows it from the press point as its centre). It is
//! committed as an ordinary [`Fill`] for the inside and a closed [`Stroke`]
//! for the outline, so it can be selected, transformed, erased and recoloured
//! like hand-drawn art.

use document::animation_core::{FillId, Rgba, StrokeId, Vec2};
use document::drawing::{BrushStyle, Fill, FillShape, Stroke, StrokePoint};

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
    /// Fill colour.
    pub fill_color: Rgba,
}

impl Default for ShapeStyle {
    fn default() -> Self {
        Self {
            stroke: true,
            stroke_width: 4.0,
            stroke_color: Rgba::BLACK,
            fill: true,
            fill_color: Rgba::WHITE,
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
/// given IDs (fixed ones for a live preview, fresh ones to commit). `None`
/// for a part that is switched off, or for both when the box is too small.
pub fn shape_items(
    kind: ShapeKind,
    bounds: (Vec2, Vec2),
    style: &ShapeStyle,
    fill_id: FillId,
    stroke_id: StrokeId,
) -> (Option<Fill>, Option<Stroke>) {
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
        fill
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
    fn items_respect_toggles_and_size() {
        let bounds = (Vec2::ZERO, Vec2::new(10.0, 10.0));
        let style = ShapeStyle::default();
        let (fill, stroke) = shape_items(
            ShapeKind::Ellipse,
            bounds,
            &style,
            FillId::new(),
            StrokeId::new(),
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
            FillId::new(),
            StrokeId::new(),
        );
        assert!(fill.is_none() && stroke.is_some());

        let tiny = (Vec2::ZERO, Vec2::splat(0.1));
        let (fill, stroke) = shape_items(
            ShapeKind::Rectangle,
            tiny,
            &style,
            FillId::new(),
            StrokeId::new(),
        );
        assert!(fill.is_none() && stroke.is_none());
    }
}
