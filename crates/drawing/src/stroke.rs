//! Strokes, brush styles and vector drawings.

use animation_core::{Rgba, StrokeId, Vec2};
use serde::{Deserialize, Serialize};

/// One sampled input point of a stroke, in document coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrokePoint {
    /// Position in document (canvas) coordinates.
    pub pos: Vec2,
    /// Pen pressure in `0.0..=1.0`. Mouse input uses `1.0`.
    pub pressure: f32,
}

impl StrokePoint {
    /// A point with full pressure (mouse input).
    pub const fn new(pos: Vec2) -> Self {
        Self { pos, pressure: 1.0 }
    }
}

/// Visual settings of a stroke.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BrushStyle {
    /// Stroke color.
    pub color: Rgba,
    /// Full-pressure width in document units.
    pub width: f32,
    /// Opacity multiplier in `0.0..=1.0`.
    pub opacity: f32,
}

impl Default for BrushStyle {
    fn default() -> Self {
        Self {
            color: Rgba::BLACK,
            width: 4.0,
            opacity: 1.0,
        }
    }
}

/// A single editable brush stroke.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stroke {
    /// Stable identifier.
    pub id: StrokeId,
    /// Input points in drawing order.
    pub points: Vec<StrokePoint>,
    /// Brush settings used for the stroke.
    pub style: BrushStyle,
}

impl Stroke {
    /// Creates a stroke with a fresh ID.
    pub fn new(points: Vec<StrokePoint>, style: BrushStyle) -> Self {
        Self {
            id: StrokeId::new(),
            points,
            style,
        }
    }

    /// Axis-aligned bounds `(min, max)` including half the brush width.
    /// Returns `None` for a stroke with no points.
    pub fn bounds(&self) -> Option<(Vec2, Vec2)> {
        let first = self.points.first()?.pos;
        let (min, max) = self
            .points
            .iter()
            .fold((first, first), |(lo, hi), p| (lo.min(p.pos), hi.max(p.pos)));
        let pad = Vec2::splat(self.style.width * 0.5);
        Some((min - pad, max + pad))
    }
}

/// A vector drawing: strokes painted bottom-to-top.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VectorDrawing {
    strokes: Vec<Stroke>,
}

impl VectorDrawing {
    /// Creates an empty drawing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a stroke on top of the others.
    pub fn push_stroke(&mut self, stroke: Stroke) {
        self.strokes.push(stroke);
    }

    /// Inserts a stroke at `index` (clamped), used to undo a removal.
    pub fn insert_stroke(&mut self, index: usize, stroke: Stroke) {
        let index = index.min(self.strokes.len());
        self.strokes.insert(index, stroke);
    }

    /// Removes a stroke by ID, returning its former index and the stroke.
    pub fn remove_stroke(&mut self, id: StrokeId) -> Option<(usize, Stroke)> {
        let index = self.strokes.iter().position(|s| s.id == id)?;
        Some((index, self.strokes.remove(index)))
    }

    /// Looks up a stroke by ID.
    pub fn stroke(&self, id: StrokeId) -> Option<&Stroke> {
        self.strokes.iter().find(|s| s.id == id)
    }

    /// All strokes, bottom-to-top.
    pub fn strokes(&self) -> &[Stroke] {
        &self.strokes
    }

    /// Whether the drawing has no strokes.
    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }
}

/// The content of a drawing. Vector is the only kind in the MVP; a raster
/// variant can be added later without changing the timeline or document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DrawingContent {
    /// Editable vector strokes.
    Vector(VectorDrawing),
}

impl Default for DrawingContent {
    fn default() -> Self {
        Self::Vector(VectorDrawing::new())
    }
}

impl DrawingContent {
    /// The vector drawing, if this content is vector.
    pub fn as_vector(&self) -> Option<&VectorDrawing> {
        match self {
            Self::Vector(v) => Some(v),
        }
    }

    /// Mutable access to the vector drawing, if this content is vector.
    pub fn as_vector_mut(&mut self) -> Option<&mut VectorDrawing> {
        match self {
            Self::Vector(v) => Some(v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Stroke {
        Stroke::new(
            vec![
                StrokePoint::new(Vec2::new(0.0, 0.0)),
                StrokePoint::new(Vec2::new(10.0, 5.0)),
            ],
            BrushStyle {
                width: 2.0,
                ..Default::default()
            },
        )
    }

    #[test]
    fn bounds_include_brush_width() {
        let (min, max) = line().bounds().unwrap();
        assert_eq!(min, Vec2::new(-1.0, -1.0));
        assert_eq!(max, Vec2::new(11.0, 6.0));
        assert!(
            Stroke::new(vec![], BrushStyle::default())
                .bounds()
                .is_none()
        );
    }

    #[test]
    fn remove_and_reinsert_keeps_order() {
        let mut d = VectorDrawing::new();
        let (a, b, c) = (line(), line(), line());
        let ids = [a.id, b.id, c.id];
        d.push_stroke(a);
        d.push_stroke(b);
        d.push_stroke(c);
        let (idx, removed) = d.remove_stroke(ids[1]).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(d.strokes().len(), 2);
        d.insert_stroke(idx, removed);
        let order: Vec<_> = d.strokes().iter().map(|s| s.id).collect();
        assert_eq!(order, ids);
    }

    #[test]
    fn removing_unknown_stroke_is_none() {
        assert!(
            VectorDrawing::new()
                .remove_stroke(StrokeId::new())
                .is_none()
        );
    }
}
