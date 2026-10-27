//! Evaluating a document at a frame.

use crate::Document;
use animation_core::{FrameIndex, LayerId};
use drawing::VectorDrawing;
use scene::Transform2D;

/// One layer's visible content at a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderLayer<'a> {
    /// The layer being drawn.
    pub layer_id: LayerId,
    /// Layer opacity.
    pub opacity: f32,
    /// Layer-to-document transform.
    pub transform: Transform2D,
    /// The drawing exposed at this frame.
    pub drawing: &'a VectorDrawing,
}

/// What to draw for one frame, bottom-to-top. Hidden layers and layers with no
/// exposed drawing are left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderList<'a> {
    /// Layers in back-to-front order.
    pub layers: Vec<RenderLayer<'a>>,
}

impl Document {
    /// Resolves which drawing each visible layer shows at `frame`.
    pub fn evaluate(&self, frame: FrameIndex) -> RenderList<'_> {
        let layers = self
            .layers()
            .iter()
            .filter(|layer| layer.visible)
            .filter_map(|layer| {
                let drawing_id = self.track(layer.id)?.drawing_at(frame)?;
                let drawing = self.drawing(drawing_id)?.as_vector()?;
                Some(RenderLayer {
                    layer_id: layer.id,
                    opacity: layer.opacity,
                    transform: layer.transform,
                    drawing,
                })
            })
            .collect();
        RenderList { layers }
    }
}

#[cfg(test)]
mod tests {
    use crate::{CanvasSize, Document};
    use animation_core::FrameIndex;
    use drawing::DrawingContent;
    use timeline::Keyframe;

    #[test]
    fn evaluate_respects_order_visibility_and_exposure() {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 100,
                height: 100,
            },
        );
        let bottom = d.add_layer("bottom");
        let top = d.add_layer("top");
        let hidden = d.add_layer("hidden");
        for layer in [bottom, top, hidden] {
            let drawing = d.add_drawing(DrawingContent::default());
            d.track_mut(layer)
                .unwrap()
                .set_key(FrameIndex(0), Keyframe::Drawing(drawing));
        }
        d.track_mut(top)
            .unwrap()
            .set_key(FrameIndex(5), Keyframe::Empty);
        d.layer_mut(hidden).unwrap().visible = false;

        let ids = |d: &Document, f| {
            d.evaluate(FrameIndex(f))
                .layers
                .iter()
                .map(|l| l.layer_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&d, 0), [bottom, top]);
        assert_eq!(ids(&d, 5), [bottom]);

        d.move_layer(top, 0);
        assert_eq!(ids(&d, 0), [top, bottom]);
    }
}
