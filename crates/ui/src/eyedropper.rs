//! The eyedropper: picks the colour of the artwork under the cursor.
//!
//! A click with [`Tool::Eyedropper`] (or an Alt-click with the brush, the
//! paint bucket or a shape tool) looks at the active layer's drawing at the
//! current frame. The topmost item under the cursor wins, the same item a
//! selection click would hit ([`Session::item_under`]): strokes over fills,
//! later items over earlier ones. A gradient fill gives its colour at the
//! clicked point. Empty canvas gives the document background, if any.
//!
//! Picking only changes tool settings, never the document, so it is not an
//! undo step.

use crate::session::Session;
use document::animation_core::{Rgba, Vec2};
use document::drawing::{Fill, ItemId};
use editor::Tool;

/// Lowest brush opacity the tool settings allow.
const MIN_BRUSH_OPACITY: f32 = 0.05;

/// What the eyedropper picked from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickSource {
    /// A stroke.
    Stroke,
    /// A fill (at the clicked point, for a gradient).
    Fill,
    /// The document background.
    Background,
}

impl PickSource {
    /// User-visible name, for the status bar.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Stroke => "stroke",
            Self::Fill => "fill",
            Self::Background => "background",
        }
    }
}

/// A colour found under the cursor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickedColor {
    /// Straight-alpha colour, as the item stores it.
    pub color: Rgba,
    /// The item's opacity multiplier (`1.0` for the background).
    pub opacity: f32,
    /// What it came from.
    pub source: PickSource,
}

/// The colour of `fill` at `pos`: its solid colour, or for a gradient the
/// blend at `pos`, computed like the renderer's shader.
pub fn fill_color_at(fill: &Fill, pos: Vec2) -> Rgba {
    let Some(g) = fill.gradient else {
        return fill.color;
    };
    let axis = g.end - g.start;
    let len2 = axis.length_squared();
    let t = if len2 > 1e-8 {
        ((pos - g.start).dot(axis) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (a, b) = (fill.color, g.end_color);
    let mix = |x: f32, y: f32| x + (y - x) * t;
    Rgba::new(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a))
}

impl Session {
    /// The colour under `pos` (document coordinates) on the active layer at
    /// the current frame, or the document background on empty canvas.
    /// A hidden layer shows nothing, so only the background is found there.
    /// `zoom` is screen points per document unit. `None` when there is
    /// nothing to pick (empty canvas with a transparent background).
    pub fn color_under(&self, pos: Vec2, zoom: f32) -> Option<PickedColor> {
        let visible = self
            .state
            .active_layer
            .and_then(|id| self.document.layers().get(id))
            .is_some_and(|layer| layer.visible);
        let item = visible.then(|| self.item_under(pos, zoom)).flatten();
        let drawing = self.active_drawing();
        let picked = match item {
            Some(ItemId::Stroke(id)) => drawing.and_then(|d| d.stroke(id)).map(|s| PickedColor {
                color: s.style.color,
                opacity: s.style.opacity,
                source: PickSource::Stroke,
            }),
            Some(ItemId::Fill(id)) => drawing.and_then(|d| d.fill(id)).map(|f| PickedColor {
                color: fill_color_at(f, pos),
                opacity: f.opacity,
                source: PickSource::Fill,
            }),
            None => None,
        };
        picked.or_else(|| {
            self.document.background.map(|color| PickedColor {
                color,
                opacity: 1.0,
                source: PickSource::Background,
            })
        })
    }

    /// Picks the colour under `pos` (see [`Self::color_under`]) into the
    /// settings of the active tool. The rectangle and ellipse tools take a
    /// stroke's colour as their stroke colour and anything else as their
    /// fill colour, with the opacity folded into the alpha (shapes are drawn
    /// at full opacity). Every other tool sets the brush colour and opacity,
    /// which the brush and the paint bucket share. Returns whether a colour
    /// was picked; says what in the status bar either way.
    pub fn pick_color(&mut self, pos: Vec2, zoom: f32) -> bool {
        let Some(picked) = self.color_under(pos, zoom) else {
            self.set_status("Nothing to pick here");
            return false;
        };
        let PickedColor {
            color,
            opacity,
            source,
        } = picked;
        match self.state.tool {
            Tool::Rectangle | Tool::Ellipse => {
                let style = &mut self.tools.shape_style;
                let folded = Rgba::new(color.r, color.g, color.b, color.a * opacity);
                if source == PickSource::Stroke {
                    style.stroke_color = folded;
                } else {
                    style.fill_color = folded;
                }
            }
            _ => {
                self.state.brush.color = color;
                self.state.brush.opacity = opacity.clamp(MIN_BRUSH_OPACITY, 1.0);
            }
        }
        self.set_status(format!("Picked the {} colour", source.label()));
        true
    }
}
