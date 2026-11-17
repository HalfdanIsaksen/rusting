//! Canvas commands: resizing the canvas around an anchor.

use crate::{Command, CommandError};
use animation_core::{DrawingId, LayerId, Vec2};
use document::drawing::DrawingContent;
use document::scene::Transform2D;
use document::timeline::{Keyframe, Track};
use document::{CanvasAnchor, CanvasSize, Document, Symbol};
use std::collections::{BTreeMap, BTreeSet};

/// Changes the canvas size and moves all artwork (every drawing on every
/// layer and frame) so it keeps its place relative to `anchor`.
///
/// Nothing is cropped: artwork that falls outside the new canvas is only
/// hidden, and comes back if the canvas grows again. Undo restores the exact
/// original geometry.
#[derive(Debug)]
pub struct ResizeCanvas {
    size: CanvasSize,
    anchor: CanvasAnchor,
    /// Old size, the drawings and the tracks with objects as they were
    /// before the move.
    originals: Option<Originals>,
}

/// What [`ResizeCanvas`] changed, for undo.
#[derive(Debug)]
struct Originals {
    canvas: CanvasSize,
    drawings: BTreeMap<DrawingId, DrawingContent>,
    tracks: BTreeMap<LayerId, Track>,
}

impl ResizeCanvas {
    /// Creates the command.
    pub fn new(size: CanvasSize, anchor: CanvasAnchor) -> Self {
        Self {
            size,
            anchor,
            originals: None,
        }
    }
}

/// `delta` (document units) expressed in a layer's local space, so moving
/// local geometry by the result moves it by `delta` on the canvas.
fn local_delta(transform: &Transform2D, delta: Vec2) -> Vec2 {
    let unrotated = Vec2::from_angle(-transform.rotation).rotate(delta);
    let local = unrotated / transform.scale;
    if local.is_finite() { local } else { delta }
}

impl Command for ResizeCanvas {
    fn label(&self) -> &str {
        "Resize Canvas"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        if self.size.width == 0 || self.size.height == 0 {
            return Err(CommandError::EmptyCanvas);
        }
        let old = doc.canvas;
        let delta = self.anchor.offset(old, self.size);

        // Each drawing moves in the space of the layer that shows it. Drawings
        // no layer shows (kept for undoing a removal) move by the plain delta.
        let mut per_drawing: BTreeMap<DrawingId, Vec2> = BTreeMap::new();
        for layer in doc.layers().iter() {
            let local = local_delta(&layer.transform, delta);
            if let Some(track) = doc.track(layer.id) {
                for id in track.keys().filter_map(|(_, key)| key.drawing()) {
                    per_drawing.insert(id, local);
                }
            }
        }

        // Objects keep their own coordinates: their drawings stay put, and
        // the instances placing them move instead.
        let in_symbols: BTreeSet<DrawingId> = doc
            .symbols()
            .values()
            .flat_map(Symbol::drawings)
            .filter(|id| !per_drawing.contains_key(id))
            .collect();

        let mut originals = BTreeMap::new();
        let mut tracks = BTreeMap::new();
        if delta != Vec2::ZERO {
            let ids: Vec<DrawingId> = doc
                .drawings()
                .keys()
                .filter(|id| !in_symbols.contains(id))
                .copied()
                .collect();
            for id in ids {
                let offset = per_drawing.get(&id).copied().unwrap_or(delta);
                if let Some(content) = doc.drawing_mut(id) {
                    originals.insert(id, content.clone());
                    if let Some(vector) = content.as_vector_mut() {
                        vector.translate(offset);
                    }
                }
            }
        }
        if delta != Vec2::ZERO {
            let layers: Vec<(LayerId, Vec2)> = doc
                .layers()
                .iter()
                .map(|l| (l.id, local_delta(&l.transform, delta)))
                .collect();
            for (layer, local) in layers {
                let Some(track) = doc.track_mut(layer) else {
                    continue;
                };
                if !track.keys().any(|(_, k)| k.instance().is_some()) {
                    continue;
                }
                tracks.insert(layer, track.clone());
                for (_, key) in track.keys_mut() {
                    if let Keyframe::Instance(instance) = key {
                        instance.transform.translation += local;
                    }
                }
            }
        }
        doc.canvas = self.size;
        self.originals = Some(Originals {
            canvas: old,
            drawings: originals,
            tracks,
        });
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let originals = self.originals.take().ok_or(CommandError::DrawingNotFound)?;
        for (id, content) in originals.drawings {
            doc.insert_drawing(id, content);
        }
        for (layer, track) in originals.tracks {
            if let Some(t) = doc.track_mut(layer) {
                *t = track;
            }
        }
        doc.canvas = originals.canvas;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use crate::commands::AddStroke;
    use animation_core::FrameIndex;
    use document::drawing::{BrushStyle, Stroke, StrokePoint};

    fn size(width: u32, height: u32) -> CanvasSize {
        CanvasSize { width, height }
    }

    fn first_point(doc: &Document) -> Vec2 {
        let drawing = doc.drawings().values().next().unwrap();
        drawing.as_vector().unwrap().strokes()[0].points[0].pos
    }

    fn setup() -> (Document, History) {
        let mut doc = Document::new("T", size(100, 100));
        let mut history = History::new();
        let layer = doc.add_layer("L");
        let stroke = Stroke::new(
            vec![
                StrokePoint::new(Vec2::new(10.3, 20.7)),
                StrokePoint::new(Vec2::new(50.0, 60.0)),
            ],
            BrushStyle::default(),
        );
        history
            .execute(
                &mut doc,
                Box::new(AddStroke::new(layer, FrameIndex(0), stroke)),
            )
            .unwrap();
        (doc, history)
    }

    #[test]
    fn every_anchor_moves_artwork_by_its_offset() {
        for anchor in CanvasAnchor::ALL {
            let (mut doc, mut history) = setup();
            let before = first_point(&doc);
            history
                .execute(
                    &mut doc,
                    Box::new(ResizeCanvas::new(size(200, 150), anchor)),
                )
                .unwrap();
            assert_eq!(doc.canvas, size(200, 150));
            let expected = before + anchor.offset(size(100, 100), size(200, 150));
            assert_eq!(first_point(&doc), expected, "{anchor:?}");
        }
    }

    #[test]
    fn undo_restores_size_and_exact_geometry() {
        let (mut doc, mut history) = setup();
        let before = doc.clone();
        history
            .execute(
                &mut doc,
                Box::new(ResizeCanvas::new(size(37, 251), CanvasAnchor::Center)),
            )
            .unwrap();
        assert_ne!(doc, before);
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, before);
        history.redo(&mut doc).unwrap();
        assert_eq!(doc.canvas, size(37, 251));
    }

    #[test]
    fn shrinking_keeps_artwork_outside_the_canvas() {
        let (mut doc, mut history) = setup();
        history
            .execute(
                &mut doc,
                Box::new(ResizeCanvas::new(size(5, 5), CanvasAnchor::TopLeft)),
            )
            .unwrap();
        let drawing = doc.drawings().values().next().unwrap();
        assert_eq!(drawing.as_vector().unwrap().strokes()[0].points.len(), 2);
    }

    #[test]
    fn zero_size_is_rejected() {
        let (mut doc, mut history) = setup();
        let before = doc.clone();
        let result = history.execute(
            &mut doc,
            Box::new(ResizeCanvas::new(size(0, 10), CanvasAnchor::Center)),
        );
        assert_eq!(result, Err(CommandError::EmptyCanvas));
        assert_eq!(doc, before);
    }

    #[test]
    fn scaled_layers_move_by_the_canvas_delta() {
        let t = Transform2D {
            translation: Vec2::new(5.0, 5.0),
            rotation: std::f32::consts::FRAC_PI_2,
            scale: Vec2::new(2.0, 2.0),
        };
        let delta = Vec2::new(10.0, 0.0);
        let p = Vec2::new(3.0, 4.0);
        let moved = t.apply(p + local_delta(&t, delta));
        assert!((moved - (t.apply(p) + delta)).length() < 1e-4);
    }

    #[test]
    fn objects_move_by_their_instances_not_their_drawings() {
        use document::timeline::InstanceKey;
        let (mut doc, mut history) = setup();
        // Move the existing drawing into a symbol placed on a new layer.
        let layer = doc.layers().iter().next().unwrap().id;
        let track = doc.track(layer).unwrap().clone();
        let mut symbol = Symbol::new("S", 1);
        symbol.push_layer(document::scene::Layer::new("in"), track);
        let sid = symbol.id;
        doc.insert_symbol(symbol);
        let obj = doc.add_layer("obj");
        doc.track_mut(obj)
            .unwrap()
            .set_key(FrameIndex(0), Keyframe::Instance(InstanceKey::new(sid)));
        doc.remove_layer(layer);
        let before = doc.clone();
        let point = first_point(&doc);

        history
            .execute(
                &mut doc,
                Box::new(ResizeCanvas::new(size(200, 150), CanvasAnchor::Center)),
            )
            .unwrap();
        let offset = CanvasAnchor::Center.offset(size(100, 100), size(200, 150));
        assert_eq!(first_point(&doc), point);
        let key = doc.track(obj).unwrap().key_at(FrameIndex(0)).unwrap();
        assert_eq!(key.instance().unwrap().transform.translation, offset);
        history.undo(&mut doc).unwrap();
        assert_eq!(doc, before);
    }
}
