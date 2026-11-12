//! Onion skin: neighbouring drawings shown tinted and translucent.
//!
//! This only reads the document. The renderer draws the returned drawings
//! with the returned colour instead of the strokes' own colours.

use document::Document;
use document::animation_core::{FrameIndex, LayerId, Rgba};
use document::drawing::VectorDrawing;
use document::scene::Transform2D;

/// Tint for drawings before the current frame.
pub const BEFORE_TINT: Rgba = Rgba::rgb(0.85, 0.15, 0.15);
/// Tint for drawings after the current frame.
pub const AFTER_TINT: Rgba = Rgba::rgb(0.1, 0.55, 0.2);

/// Onion-skin settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnionSkin {
    /// Whether onion skins are drawn.
    pub enabled: bool,
    /// How many drawings before the current one to show.
    pub before: u32,
    /// How many drawings after the current one to show.
    pub after: u32,
    /// Opacity of the nearest skin; further ones fade out.
    pub opacity: f32,
}

impl Default for OnionSkin {
    fn default() -> Self {
        Self {
            enabled: false,
            before: 1,
            after: 1,
            opacity: 0.35,
        }
    }
}

/// Most skins in either direction.
pub const MAX_ONION_RANGE: u32 = 10;

/// One neighbouring drawing to draw as an onion skin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OnionDrawing<'a> {
    /// The layer it belongs to.
    pub layer: LayerId,
    /// Key offset from the current frame: `-1` is the previous drawing.
    pub offset: i32,
    /// Layer-to-document transform.
    pub transform: Transform2D,
    /// The drawing.
    pub drawing: &'a VectorDrawing,
    /// Colour for all its strokes (straight alpha, opacity applied).
    pub color: Rgba,
}

impl OnionSkin {
    /// The tint for a skin `offset` keys away, faded by distance.
    pub fn tint(&self, offset: i32) -> Rgba {
        let (base, range) = if offset < 0 {
            (BEFORE_TINT, self.before)
        } else {
            (AFTER_TINT, self.after)
        };
        let distance = offset.unsigned_abs().max(1);
        let fade = 1.0 - (distance - 1) as f32 / range.max(1) as f32;
        base.with_opacity(self.opacity * fade)
    }

    /// Neighbouring drawings of every visible layer at `frame`, back to
    /// front (furthest first). Empty when disabled.
    pub fn collect<'a>(&self, doc: &'a Document, frame: FrameIndex) -> Vec<OnionDrawing<'a>> {
        if !self.enabled {
            return Vec::new();
        }
        let before = self.before.min(MAX_ONION_RANGE);
        let after = self.after.min(MAX_ONION_RANGE);
        let mut skins = Vec::new();
        let stack = doc.layers();
        for layer in stack.iter().filter(|l| stack.is_visible(l.id)) {
            let Some(track) = doc.track(layer.id) else {
                continue;
            };
            let current = track.drawing_at(frame);
            let mut layer_skins: Vec<_> = track
                .onion_skin_frames(frame, before, after)
                .into_iter()
                .filter(|(_, id)| Some(*id) != current)
                .filter_map(|(offset, id)| {
                    let drawing = doc.drawing(id)?.as_vector()?;
                    Some(OnionDrawing {
                        layer: layer.id,
                        offset,
                        transform: layer.transform,
                        drawing,
                        color: self
                            .tint(offset)
                            .with_opacity(stack.effective_opacity(layer.id)),
                    })
                })
                .collect();
            // Furthest skins first so the nearest ones sit on top.
            layer_skins.sort_by_key(|s| std::cmp::Reverse(s.offset.unsigned_abs()));
            skins.extend(layer_skins);
        }
        skins
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::CanvasSize;
    use document::animation_core::Vec2;
    use document::drawing::{BrushStyle, DrawingContent, Stroke, StrokePoint};
    use document::timeline::Keyframe;

    fn doc_with_keys() -> (Document, LayerId) {
        let mut doc = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        let layer = doc.add_layer("L");
        for frame in [0, 4, 8] {
            let mut drawing = VectorDrawing::new();
            drawing.push_stroke(Stroke::new(
                vec![StrokePoint::new(Vec2::ZERO), StrokePoint::new(Vec2::ONE)],
                BrushStyle::default(),
            ));
            let id = doc.add_drawing(DrawingContent::Vector(drawing));
            doc.track_mut(layer)
                .unwrap()
                .set_key(FrameIndex(frame), Keyframe::Drawing(id));
        }
        (doc, layer)
    }

    #[test]
    fn shows_neighbours_not_current_and_leaves_document_alone() {
        let (doc, layer) = doc_with_keys();
        let before = doc.clone();
        let onion = OnionSkin {
            enabled: true,
            ..OnionSkin::default()
        };
        let skins = onion.collect(&doc, FrameIndex(5));
        let offsets: Vec<_> = skins.iter().map(|s| s.offset).collect();
        assert_eq!(offsets.len(), 2);
        assert!(offsets.contains(&-1) && offsets.contains(&1));
        let current = doc.drawing_at(layer, FrameIndex(5)).unwrap();
        for skin in &skins {
            assert!(!std::ptr::eq(
                skin.drawing,
                doc.drawing(current).unwrap().as_vector().unwrap()
            ));
            assert!(skin.color.a < 1.0);
        }
        assert_eq!(skins.iter().find(|s| s.offset < 0).unwrap().color.r, 0.85);
        assert_eq!(doc, before);
    }

    #[test]
    fn disabled_or_hidden_shows_nothing() {
        let (mut doc, layer) = doc_with_keys();
        let mut onion = OnionSkin::default();
        assert!(onion.collect(&doc, FrameIndex(4)).is_empty());
        onion.enabled = true;
        doc.layer_mut(layer).unwrap().visible = false;
        assert!(onion.collect(&doc, FrameIndex(4)).is_empty());
    }

    #[test]
    fn further_skins_fade_and_come_first() {
        let (doc, _) = doc_with_keys();
        let onion = OnionSkin {
            enabled: true,
            before: 2,
            after: 0,
            opacity: 0.5,
        };
        let skins = onion.collect(&doc, FrameIndex(8));
        let offsets: Vec<_> = skins.iter().map(|s| s.offset).collect();
        assert_eq!(offsets, [-2, -1]);
        assert!(skins[0].color.a < skins[1].color.a);
        assert_eq!(skins[1].color.a, 0.5);
    }
}
