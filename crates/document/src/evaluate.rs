//! Evaluating a document at a frame.
//!
//! Instance keys (ADR-018) are expanded recursively: the symbol's own
//! layers are evaluated at the instance's inner frame and come out as more
//! [`RenderLayer`]s of the layer that holds the instance, with the
//! transforms combined and the opacities multiplied.

use crate::Document;
use animation_core::glam::Affine2;
use animation_core::{FrameIndex, LayerId, SymbolId, Vec2};
use drawing::VectorDrawing;
use drawing::select::rect_union;
use scene::BlendMode;
use timeline::{InstanceKey, Keyframe, Track};

/// How deep symbols may nest inside each other. Deeper instances (only
/// possible in a damaged file with a cycle) draw nothing.
pub const MAX_SYMBOL_DEPTH: usize = 16;

/// One layer's visible content at a frame.
///
/// An instance key yields one `RenderLayer` per visible symbol layer, all
/// with the `layer_id` and `blend` of the document layer that holds the
/// instance, so blend modes composite the object as a part of that layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderLayer<'a> {
    /// The document layer being drawn.
    pub layer_id: LayerId,
    /// Layer opacity, times its group's (and, inside an object, the
    /// instance's and the symbol layer's).
    pub opacity: f32,
    /// How the layer combines with the layers below it.
    pub blend: BlendMode,
    /// Drawing-to-document transform.
    pub transform: Affine2,
    /// The drawing exposed at this frame.
    pub drawing: &'a VectorDrawing,
    /// Whether the drawing comes from inside an object (an instance key)
    /// rather than from the layer's own drawing key. Editing tools leave
    /// such drawings alone.
    pub in_object: bool,
}

/// What to draw for one frame, bottom-to-top. Hidden layers (or layers in a
/// hidden group) and layers with no exposed drawing are left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderList<'a> {
    /// Layers in back-to-front order.
    pub layers: Vec<RenderLayer<'a>>,
}

impl RenderList<'_> {
    /// Document-space bounds of everything in the list (transformed item
    /// bounds), or `None` if it is empty.
    pub fn bounds(&self) -> Option<(Vec2, Vec2)> {
        self.layers
            .iter()
            .filter_map(|layer| {
                let items = layer.drawing.all_items();
                let (min, max) = layer.drawing.items_bounds(&items)?;
                Some(transform_rect(&layer.transform, (min, max)))
            })
            .reduce(rect_union)
    }
}

/// The axis-aligned bounds of rectangle `rect` mapped through `transform`.
pub fn transform_rect(transform: &Affine2, (min, max): (Vec2, Vec2)) -> (Vec2, Vec2) {
    let corners = [min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
        .map(|p| transform.transform_point2(p));
    corners
        .iter()
        .fold((corners[0], corners[0]), |(lo, hi), &p| {
            (lo.min(p), hi.max(p))
        })
}

/// Where the drawings of one track land: the document layer they count as,
/// and the accumulated transform and opacity.
#[derive(Clone, Copy)]
struct Context {
    layer_id: LayerId,
    blend: BlendMode,
    opacity: f32,
    transform: Affine2,
    depth: usize,
}

impl Document {
    /// Resolves which drawings each visible layer shows at `frame`,
    /// expanding objects (instance keys) into their own layers.
    pub fn evaluate(&self, frame: FrameIndex) -> RenderList<'_> {
        let stack = self.layers();
        let mut layers = Vec::new();
        for layer in stack.iter().filter(|layer| stack.is_visible(layer.id)) {
            let Some(track) = self.track(layer.id) else {
                continue;
            };
            let context = Context {
                layer_id: layer.id,
                blend: layer.blend,
                opacity: stack.effective_opacity(layer.id),
                transform: layer.transform.to_affine(),
                depth: 0,
            };
            self.push_exposed(track, frame, context, &mut layers);
        }
        RenderList { layers }
    }

    /// What one object shows at its own `frame`, at the identity placement
    /// (symbol coordinates), as layers of `layer_id`. Empty if the symbol
    /// is missing or the frame is past its end.
    pub fn evaluate_symbol(
        &self,
        symbol: SymbolId,
        frame: FrameIndex,
        layer_id: LayerId,
    ) -> RenderList<'_> {
        let mut layers = Vec::new();
        if let Some(s) = self.symbol(symbol)
            && frame.0 < s.frame_count
        {
            let key = InstanceKey {
                first_frame: frame,
                ..InstanceKey::new(symbol)
            };
            let context = Context {
                layer_id,
                blend: BlendMode::Normal,
                opacity: 1.0,
                transform: Affine2::IDENTITY,
                depth: 0,
            };
            self.push_instance(&key, 0, context, &mut layers);
        }
        RenderList { layers }
    }

    /// Document-space bounds of the object shown on `layer` at `frame`
    /// (the instance key exposed there), including the layer transform.
    /// `None` if no instance is exposed or it shows nothing.
    pub fn instance_bounds(&self, layer: LayerId, frame: FrameIndex) -> Option<(Vec2, Vec2)> {
        let l = self.layers().get(layer)?;
        let (at, key) = self.track(layer)?.instance_at(frame)?;
        let mut layers = Vec::new();
        let context = Context {
            layer_id: layer,
            blend: l.blend,
            opacity: 1.0,
            transform: l.transform.to_affine(),
            depth: 0,
        };
        self.push_instance(key, frame.0 - at.0, context, &mut layers);
        RenderList { layers }.bounds()
    }

    /// Pushes what `track` shows at `frame`.
    fn push_exposed<'a>(
        &'a self,
        track: &'a Track,
        frame: FrameIndex,
        context: Context,
        out: &mut Vec<RenderLayer<'a>>,
    ) {
        match track.exposed_key(frame) {
            Some((_, Keyframe::Drawing(id))) => {
                if let Some(drawing) = self.drawing(*id).and_then(|d| d.as_vector()) {
                    out.push(RenderLayer {
                        layer_id: context.layer_id,
                        opacity: context.opacity,
                        blend: context.blend,
                        transform: context.transform,
                        drawing,
                        in_object: context.depth > 0,
                    });
                }
            }
            Some((at, Keyframe::Instance(key))) => {
                self.push_instance(key, frame.0 - at.0, context, out);
            }
            Some((_, Keyframe::Empty)) | None => {}
        }
    }

    /// Pushes what instance `key` shows `elapsed` frames after its key.
    fn push_instance<'a>(
        &'a self,
        key: &InstanceKey,
        elapsed: u32,
        context: Context,
        out: &mut Vec<RenderLayer<'a>>,
    ) {
        if context.depth >= MAX_SYMBOL_DEPTH {
            return;
        }
        let Some(symbol) = self.symbol(key.symbol) else {
            return;
        };
        let Some(inner) = key.inner_frame(elapsed, symbol.frame_count) else {
            return;
        };
        let placed = context.transform * key.transform.to_affine();
        let opacity = context.opacity * key.opacity;
        let stack = &symbol.layers;
        for layer in stack.iter().filter(|l| stack.is_visible(l.id)) {
            let Some(track) = symbol.tracks.get(&layer.id) else {
                continue;
            };
            let inner_context = Context {
                layer_id: context.layer_id,
                blend: context.blend,
                opacity: opacity * stack.effective_opacity(layer.id),
                transform: placed * layer.transform.to_affine(),
                depth: context.depth + 1,
            };
            self.push_exposed(track, inner, inner_context, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CanvasSize, Symbol};
    use animation_core::DrawingId;
    use drawing::{BrushStyle, DrawingContent, Stroke, StrokePoint};
    use scene::{Layer, LayerGroup};
    use timeline::{InstanceMode, InstanceTransform};

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

    /// A 12-frame symbol whose single layer shows a distinct drawing on
    /// each frame, and a document with one layer placing it at frame 2.
    fn twelve_frame_object() -> (Document, LayerId, SymbolId, Vec<DrawingId>) {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 100,
                height: 100,
            },
        );
        d.frame_count = 20;
        let mut symbol = Symbol::new("Ball", 12);
        let mut track = Track::new();
        let mut drawings = Vec::new();
        for f in 0..12 {
            let mut v = VectorDrawing::new();
            v.push_stroke(Stroke::new(
                vec![
                    StrokePoint::new(Vec2::new(0.0, 0.0)),
                    StrokePoint::new(Vec2::new(10.0, 10.0)),
                ],
                BrushStyle::default(),
            ));
            let id = d.add_drawing(DrawingContent::Vector(v));
            drawings.push(id);
            track.set_key(FrameIndex(f), Keyframe::Drawing(id));
        }
        symbol.push_layer(Layer::new("inner"), track);
        let sid = symbol.id;
        d.insert_symbol(symbol);
        let layer = d.add_layer("obj");
        d.track_mut(layer)
            .unwrap()
            .set_key(FrameIndex(2), Keyframe::Instance(InstanceKey::new(sid)));
        (d, layer, sid, drawings)
    }

    fn shown(d: &Document, drawings: &[DrawingId], frame: u32) -> Option<usize> {
        let list = d.evaluate(FrameIndex(frame));
        let layer = list.layers.first()?;
        drawings.iter().position(|id| {
            std::ptr::eq(d.drawing(*id).unwrap().as_vector().unwrap(), layer.drawing)
        })
    }

    #[test]
    fn instance_loops_or_plays_once_over_its_exposure() {
        let (mut d, layer, _, drawings) = twelve_frame_object();
        assert_eq!(shown(&d, &drawings, 1), None);
        // Exposed for 18 frames (2..20): 0..11 then 0..5.
        let looped: Vec<_> = (2..20).map(|f| shown(&d, &drawings, f).unwrap()).collect();
        let expected: Vec<usize> = (0..12).chain(0..6).collect();
        assert_eq!(looped, expected);
        assert!(d.evaluate(FrameIndex(5)).layers[0].in_object);

        let track = d.track_mut(layer).unwrap();
        let Some(Keyframe::Instance(key)) = track.key_at(FrameIndex(2)).copied() else {
            panic!("instance key expected");
        };
        track.set_key(
            FrameIndex(2),
            Keyframe::Instance(InstanceKey {
                mode: InstanceMode::Once,
                ..key
            }),
        );
        let once: Vec<_> = (2..20).map(|f| shown(&d, &drawings, f).unwrap()).collect();
        let expected: Vec<usize> = (0..12).chain([11; 6]).collect();
        assert_eq!(once, expected);
    }

    #[test]
    fn instance_combines_transforms_and_opacities() {
        let (mut d, layer, sid, _) = twelve_frame_object();
        d.layer_mut(layer).unwrap().opacity = 0.5;
        d.layer_mut(layer).unwrap().blend = BlendMode::Multiply;
        let key = InstanceKey {
            opacity: 0.5,
            ..InstanceKey::new(sid).with_transform(InstanceTransform {
                translation: Vec2::new(30.0, 40.0),
                scale: Vec2::splat(2.0),
                rotation: 0.0,
            })
        };
        d.track_mut(layer)
            .unwrap()
            .set_key(FrameIndex(2), Keyframe::Instance(key));
        let inner = d.symbol_mut(sid).unwrap();
        let inner_layer = inner.layers.iter().next().unwrap().id;
        inner.layers.get_mut(inner_layer).unwrap().opacity = 0.5;

        let list = d.evaluate(FrameIndex(2));
        let l = &list.layers[0];
        assert_eq!(l.layer_id, layer);
        assert_eq!(l.blend, BlendMode::Multiply);
        assert_eq!(l.opacity, 0.125);
        let p = l.transform.transform_point2(Vec2::new(10.0, 10.0));
        assert_eq!(p, Vec2::new(50.0, 60.0));
        let (min, max) = d.instance_bounds(layer, FrameIndex(4)).unwrap();
        assert!(min.x <= 30.0 && min.x > 10.0 && max.x >= 50.0 && max.x < 70.0);
        assert!(d.instance_bounds(layer, FrameIndex(1)).is_none());

        let alone = d.evaluate_symbol(sid, FrameIndex(3), layer);
        assert_eq!(alone.layers.len(), 1);
        assert_eq!(alone.layers[0].transform, Affine2::IDENTITY);
        assert!(
            d.evaluate_symbol(sid, FrameIndex(12), layer)
                .layers
                .is_empty()
        );
    }

    #[test]
    fn nested_symbols_and_cycles_are_bounded() {
        let (mut d, layer, sid, _) = twelve_frame_object();
        // An outer symbol placing the 12-frame one, translated.
        let mut outer = Symbol::new("Outer", 3);
        let mut track = Track::new();
        track.set_key(
            FrameIndex(0),
            Keyframe::Instance(
                InstanceKey::new(sid)
                    .with_transform(InstanceTransform::from_translation(Vec2::new(5.0, 0.0))),
            ),
        );
        outer.push_layer(Layer::new("o"), track);
        let oid = outer.id;
        d.insert_symbol(outer);
        d.track_mut(layer)
            .unwrap()
            .set_key(FrameIndex(0), Keyframe::Instance(InstanceKey::new(oid)));
        let list = d.evaluate(FrameIndex(0));
        assert_eq!(list.layers.len(), 1);
        assert_eq!(
            list.layers[0].transform.transform_point2(Vec2::ZERO),
            Vec2::new(5.0, 0.0)
        );

        // A symbol placing itself (a damaged file) stops at the depth limit.
        let mut cycle = Symbol::new("Cycle", 1);
        let cid = cycle.id;
        let mut track = Track::new();
        track.set_key(FrameIndex(0), Keyframe::Instance(InstanceKey::new(cid)));
        cycle.push_layer(Layer::new("c"), track);
        d.insert_symbol(cycle);
        d.track_mut(layer)
            .unwrap()
            .set_key(FrameIndex(0), Keyframe::Instance(InstanceKey::new(cid)));
        assert!(d.evaluate(FrameIndex(0)).layers.is_empty());
    }
}
