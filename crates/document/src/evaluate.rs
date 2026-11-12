//! Evaluating a document at a frame.

use crate::Document;
use animation_core::{FrameIndex, LayerId};
use drawing::VectorDrawing;
use scene::{BlendMode, Transform2D};

/// One layer's visible content at a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderLayer<'a> {
    /// The layer being drawn.
    pub layer_id: LayerId,
    /// Layer opacity, times its group's.
    pub opacity: f32,
    /// How the layer combines with the layers below it.
    pub blend: BlendMode,
    /// Layer-to-document transform.
    pub transform: Transform2D,
    /// The drawing exposed at this frame.
    pub drawing: &'a VectorDrawing,
}

/// What to draw for one frame, bottom-to-top. Hidden layers (or layers in a
/// hidden group) and layers with no exposed drawing are left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderList<'a> {
    /// Layers in back-to-front order.
    pub layers: Vec<RenderLayer<'a>>,
}

impl Document {
    /// Resolves which drawing each visible layer shows at `frame`.
    pub fn evaluate(&self, frame: FrameIndex) -> RenderList<'_> {
        let stack = self.layers();
        let layers = stack
            .iter()
            .filter(|layer| stack.is_visible(layer.id))
            .filter_map(|layer| {
                let drawing_id = self.track(layer.id)?.drawing_at(frame)?;
                let drawing = self.drawing(drawing_id)?.as_vector()?;
                Some(RenderLayer {
                    layer_id: layer.id,
                    opacity: stack.effective_opacity(layer.id),
                    blend: layer.blend,
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
    use scene::LayerGroup;
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

    #[test]
    fn group_visibility_and_opacity_apply_to_members() {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        );
        let a = d.add_layer("a");
        let b = d.add_layer("b");
        for layer in [a, b] {
            let drawing = d.add_drawing(DrawingContent::default());
            d.track_mut(layer)
                .unwrap()
                .set_key(FrameIndex(0), Keyframe::Drawing(drawing));
        }
        let group = LayerGroup::new("G");
        let g = group.id;
        d.add_group(group);
        d.move_layer_to_group(b, 1, Some(g));
        d.layer_mut(b).unwrap().opacity = 0.5;
        d.group_mut(g).unwrap().opacity = 0.5;
        let list = d.evaluate(FrameIndex(0));
        assert_eq!(list.layers[0].opacity, 1.0);
        assert_eq!(list.layers[1].opacity, 0.25);
        d.group_mut(g).unwrap().visible = false;
        let list = d.evaluate(FrameIndex(0));
        assert_eq!(list.layers.len(), 1);
        assert_eq!(list.layers[0].layer_id, a);
    }
}
