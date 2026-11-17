//! Object commands (ADR-018): turning layers and frames, or canvas items,
//! into a symbol, placing and moving instances, and managing symbols in the
//! library.

use super::{editable_layer, frame_in_range};
use crate::{Command, CommandError};
use animation_core::{DrawingId, FrameIndex, FrameRange, LayerId, SymbolId};
use document::drawing::DrawingContent;
use document::scene::Layer;
use document::timeline::{InstanceKey, Keyframe, Track};
use document::{Document, Symbol};
use std::collections::BTreeMap;

/// Everything a conversion adds, built once on the first apply so redo
/// reproduces exactly the same IDs.
#[derive(Debug, Clone)]
struct Conversion {
    /// The new symbol.
    symbol: Symbol,
    /// Drawings the symbol shows (fresh copies).
    drawings: Vec<(DrawingId, DrawingContent)>,
    /// Document tracks after the conversion, by layer.
    tracks: Vec<(LayerId, Track)>,
    /// Drawings replaced in the document pool (ID, new content).
    replaced_drawings: Vec<(DrawingId, DrawingContent)>,
    /// The new layer holding the instance, its index and track.
    layer: Layer,
    index: usize,
    track: Track,
}

/// What a conversion replaced, for undo.
#[derive(Debug)]
struct Previous {
    tracks: Vec<(LayerId, Track)>,
    drawings: Vec<(DrawingId, DrawingContent)>,
}

/// Applies `conversion` and returns what it replaced.
fn apply_conversion(doc: &mut Document, c: &Conversion) -> Result<Previous, CommandError> {
    let mut previous = Previous {
        tracks: Vec::new(),
        drawings: Vec::new(),
    };
    for (layer, track) in &c.tracks {
        let slot = doc.track_mut(*layer).ok_or(CommandError::LayerNotFound)?;
        previous
            .tracks
            .push((*layer, std::mem::replace(slot, track.clone())));
    }
    for (id, content) in &c.replaced_drawings {
        let old = doc
            .insert_drawing(*id, content.clone())
            .ok_or(CommandError::DrawingNotFound)?;
        previous.drawings.push((*id, old));
    }
    for (id, content) in &c.drawings {
        doc.insert_drawing(*id, content.clone());
    }
    doc.insert_symbol(c.symbol.clone());
    doc.insert_layer(c.index, c.layer.clone());
    if let Some(track) = doc.track_mut(c.layer.id) {
        *track = c.track.clone();
    }
    Ok(previous)
}

/// Reverts [`apply_conversion`].
fn undo_conversion(
    doc: &mut Document,
    c: &Conversion,
    previous: Previous,
) -> Result<(), CommandError> {
    doc.remove_layer(c.layer.id)
        .ok_or(CommandError::LayerNotFound)?;
    doc.remove_symbol(c.symbol.id)
        .ok_or(CommandError::SymbolNotFound)?;
    for (id, _) in &c.drawings {
        doc.remove_drawing(*id);
    }
    for (id, content) in previous.drawings {
        doc.insert_drawing(id, content);
    }
    for (layer, track) in previous.tracks {
        *doc.track_mut(layer).ok_or(CommandError::LayerNotFound)? = track;
    }
    Ok(())
}

/// The new layer that holds an object, placed right above `below` and in
/// its group (so groups stay together).
fn object_layer(
    doc: &Document,
    name: &str,
    below: LayerId,
) -> Result<(Layer, usize), CommandError> {
    let stack = doc.layers();
    let source = stack.get(below).ok_or(CommandError::LayerNotFound)?;
    let index = stack.index_of(below).ok_or(CommandError::LayerNotFound)? + 1;
    let mut layer = Layer::new(name);
    layer.group = source.group;
    layer.transform = source.transform;
    Ok((layer, index))
}

/// The key that continues what `key` (sitting `elapsed` frames earlier)
/// shows, as a new key: the same drawing, or the instance with its playback
/// advanced. `None` for an empty key.
fn continuation(doc: &Document, key: &Keyframe, elapsed: u32) -> Option<Keyframe> {
    match key {
        Keyframe::Drawing(_) => Some(*key),
        Keyframe::Instance(i) => {
            let length = doc.symbol(i.symbol).map_or(1, |s| s.frame_count);
            Some(Keyframe::Instance(i.advanced(elapsed, length)))
        }
        Keyframe::Empty => None,
    }
}

/// Turns the selected layers' frames in `range` into a new symbol (object)
/// and places it on a new layer above them.
///
/// - The keys inside the range are copied into the symbol, starting at its
///   frame 0, with fresh copies of their drawings. A drawing (or object)
///   exposed at the start of the range by an earlier key gets a key at 0.
/// - The range is cleared on the original layers. What they showed before
///   and after the range stays as it was.
/// - The new "Object" layer gets an instance key at `range.start` and an
///   empty key at `range.end`, so the object plays exactly where the frames
///   were.
#[derive(Debug)]
pub struct ConvertToSymbol {
    layers: Vec<LayerId>,
    range: FrameRange,
    name: Option<String>,
    built: Option<Conversion>,
    previous: Option<Previous>,
}

impl ConvertToSymbol {
    /// Creates the command for `layers` (any order) and `range`.
    pub fn new(layers: Vec<LayerId>, range: FrameRange) -> Self {
        Self {
            layers,
            range,
            name: None,
            built: None,
            previous: None,
        }
    }

    /// Names the symbol (default: the next free "Object N").
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// ID of the symbol created by the last apply.
    pub fn symbol_id(&self) -> Option<SymbolId> {
        self.built.as_ref().map(|c| c.symbol.id)
    }

    /// ID of the layer holding the instance, after the first apply.
    pub fn layer_id(&self) -> Option<LayerId> {
        self.built.as_ref().map(|c| c.layer.id)
    }

    fn build(&self, doc: &Document) -> Result<Conversion, CommandError> {
        let start = self.range.start;
        let end = FrameIndex(self.range.end.0.min(doc.frame_count));
        if self.layers.is_empty() || end <= start {
            return Err(CommandError::NothingToConvert);
        }
        frame_in_range(doc, start)?;
        let stack = doc.layers();
        let mut layers = Vec::new();
        for &id in &self.layers {
            editable_layer(doc, id)?;
            let index = stack.index_of(id).ok_or(CommandError::LayerNotFound)?;
            if !layers.iter().any(|&(_, l)| l == id) {
                layers.push((index, id));
            }
        }
        layers.sort_unstable();

        let length = end.0 - start.0;
        let name = self.name.clone().unwrap_or_else(|| doc.next_symbol_name());
        let mut symbol = Symbol::new(name.clone(), length);
        let mut copies: BTreeMap<DrawingId, DrawingId> = BTreeMap::new();
        let mut drawings = Vec::new();
        let mut tracks = Vec::new();
        for &(_, id) in &layers {
            let source = stack.get(id).ok_or(CommandError::LayerNotFound)?;
            let track = doc.track(id).ok_or(CommandError::LayerNotFound)?;

            // The symbol's copy of the range.
            let mut inner = Track::new();
            if track.key_at(start).is_none()
                && let Some((at, key)) = track.exposed_key(start)
                && let Some(key) = continuation(doc, key, start.0 - at.0)
            {
                inner.set_key(FrameIndex(0), key);
            }
            for (frame, key) in track.keys() {
                if frame >= start && frame < end {
                    inner.set_key(FrameIndex(frame.0 - start.0), *key);
                }
            }
            let keys: Vec<(FrameIndex, Keyframe)> = inner.keys().map(|(f, k)| (f, *k)).collect();
            for (frame, key) in keys {
                if let Keyframe::Drawing(old) = key {
                    let copy = match copies.get(&old) {
                        Some(&copy) => copy,
                        None => {
                            let content = doc
                                .drawing(old)
                                .ok_or(CommandError::DrawingNotFound)?
                                .duplicate();
                            let copy = DrawingId::new();
                            copies.insert(old, copy);
                            drawings.push((copy, content));
                            copy
                        }
                    };
                    inner.set_key(frame, Keyframe::Drawing(copy));
                }
            }
            let mut inner_layer = Layer::new(source.name.clone());
            inner_layer.visible = source.visible;
            inner_layer.opacity = source.opacity;
            inner_layer.blend = source.blend;
            inner_layer.transform = source.transform;
            symbol.push_layer(inner_layer, inner);

            // The original layer without the range.
            let mut outer = track.clone();
            let after = (end.0 < doc.frame_count && track.key_at(end).is_none())
                .then(|| track.exposed_key(end))
                .flatten()
                .and_then(|(at, key)| continuation(doc, key, end.0 - at.0));
            let inside: Vec<FrameIndex> = outer
                .keys()
                .map(|(f, _)| f)
                .filter(|&f| f >= start && f < end)
                .collect();
            for frame in inside {
                outer.remove_key(frame);
            }
            if let Some(key) = after {
                outer.set_key(end, key);
            }
            if outer
                .exposed_key(start)
                .is_some_and(|(_, k)| !matches!(k, Keyframe::Empty))
            {
                outer.set_key(start, Keyframe::Empty);
            }
            tracks.push((id, outer));
        }

        let top = layers
            .last()
            .map(|&(_, id)| id)
            .ok_or(CommandError::NothingToConvert)?;
        let (layer, index) = object_layer(doc, &name, top)?;
        // Objects are placed in the parent's space; the symbol's layers
        // keep their own transforms, so the object layer adds none.
        let mut layer = layer;
        layer.transform = document::scene::Transform2D::IDENTITY;
        let mut track = Track::new();
        track.set_key(start, Keyframe::Instance(InstanceKey::new(symbol.id)));
        if end.0 < doc.frame_count {
            track.set_key(end, Keyframe::Empty);
        }
        Ok(Conversion {
            symbol,
            drawings,
            tracks,
            replaced_drawings: Vec::new(),
            layer,
            index,
            track,
        })
    }
}

impl Command for ConvertToSymbol {
    fn label(&self) -> &str {
        "Convert to Object"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let built = match self.built.take() {
            Some(built) => built,
            None => self.build(doc)?,
        };
        self.previous = Some(apply_conversion(doc, &built)?);
        self.built = Some(built);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let built = self.built.as_ref().ok_or(CommandError::SymbolNotFound)?;
        let previous = self.previous.take().ok_or(CommandError::SymbolNotFound)?;
        undo_conversion(doc, built, previous)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use animation_core::Vec2;
    use document::CanvasSize;
    use document::drawing::{BrushStyle, Stroke, StrokePoint, VectorDrawing};

    pub(super) fn doc() -> Document {
        let mut d = Document::new(
            "T",
            CanvasSize {
                width: 100,
                height: 100,
            },
        );
        d.frame_count = 24;
        d
    }

    pub(super) fn drawing(d: &mut Document, x: f32) -> DrawingId {
        let mut v = VectorDrawing::new();
        v.push_stroke(Stroke::new(
            vec![
                StrokePoint::new(Vec2::new(x, 10.0)),
                StrokePoint::new(Vec2::new(x + 10.0, 20.0)),
            ],
            BrushStyle::default(),
        ));
        d.add_drawing(DrawingContent::Vector(v))
    }

    /// Two layers: `a` with keys at 0 (held into the range), 4 and 8;
    /// `b` with a key at 2 and an empty key at 6.
    fn two_layers() -> (Document, LayerId, LayerId) {
        let mut d = doc();
        let a = d.add_layer("A");
        let b = d.add_layer("B");
        for f in [0, 4, 8] {
            let id = drawing(&mut d, f as f32);
            d.track_mut(a)
                .unwrap()
                .set_key(FrameIndex(f), Keyframe::Drawing(id));
        }
        let id = drawing(&mut d, 50.0);
        let t = d.track_mut(b).unwrap();
        t.set_key(FrameIndex(2), Keyframe::Drawing(id));
        t.set_key(FrameIndex(6), Keyframe::Empty);
        (d, a, b)
    }

    /// The document frames, as drawn: per frame, the first point's x of
    /// every rendered layer.
    fn picture(d: &Document) -> Vec<Vec<f32>> {
        (0..d.frame_count)
            .map(|f| {
                d.evaluate(FrameIndex(f))
                    .layers
                    .iter()
                    .map(|l| {
                        let p = l.drawing.strokes()[0].points[0].pos;
                        l.transform.transform_point2(p).x
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn convert_layers_keeps_the_picture_and_undoes_exactly() {
        let (mut d, a, b) = two_layers();
        let before = d.clone();
        let shown = picture(&d);
        let mut h = History::new();
        let cmd = ConvertToSymbol::new(vec![b, a], FrameRange::new(FrameIndex(2), FrameIndex(10)));
        h.execute(&mut d, Box::new(cmd)).unwrap();

        // Every frame still shows the same artwork (order within a frame
        // may change: the object layer sits on top).
        let sorted = |p: Vec<Vec<f32>>| {
            p.into_iter()
                .map(|mut v| {
                    v.sort_by(f32::total_cmp);
                    v
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(sorted(picture(&d)), sorted(shown));

        let (sid, symbol) = d.symbols().iter().next().unwrap();
        assert_eq!(symbol.frame_count, 8);
        assert_eq!(symbol.layers.len(), 2);
        assert_eq!(symbol.name, "Object 1");
        // The new layer sits on top with the instance and an end key.
        let top = d.layers().iter().last().unwrap();
        assert_eq!(top.name, "Object 1");
        let track = d.track(top.id).unwrap();
        assert_eq!(track.key_at(FrameIndex(2)).unwrap().symbol(), Some(*sid));
        assert_eq!(track.key_at(FrameIndex(10)), Some(&Keyframe::Empty));
        // Layer a: key 0 stays, cut by an empty key at 2, its drawing from
        // 8 continues at 10.
        let ta = d.track(a).unwrap();
        assert_eq!(ta.key_at(FrameIndex(2)), Some(&Keyframe::Empty));
        assert!(ta.key_at(FrameIndex(4)).is_none());
        assert!(ta.key_at(FrameIndex(10)).unwrap().drawing().is_some());

        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
        h.redo(&mut d).unwrap();
        let again = d.clone();
        h.undo(&mut d).unwrap();
        h.redo(&mut d).unwrap();
        assert_eq!(d, again);
    }

    #[test]
    fn convert_refuses_bad_input() {
        let (mut d, a, _) = two_layers();
        let mut h = History::new();
        let empty = FrameRange::new(FrameIndex(3), FrameIndex(3));
        assert_eq!(
            h.execute(&mut d, Box::new(ConvertToSymbol::new(vec![a], empty))),
            Err(CommandError::NothingToConvert)
        );
        let range = FrameRange::new(FrameIndex(0), FrameIndex(4));
        assert_eq!(
            h.execute(&mut d, Box::new(ConvertToSymbol::new(vec![], range))),
            Err(CommandError::NothingToConvert)
        );
        d.layer_mut(a).unwrap().locked = true;
        assert_eq!(
            h.execute(&mut d, Box::new(ConvertToSymbol::new(vec![a], range))),
            Err(CommandError::LayerLocked)
        );
    }

    #[test]
    fn converting_an_object_nests_it_and_continues_its_playback() {
        let (mut d, a, _) = two_layers();
        let mut h = History::new();
        let first = ConvertToSymbol::new(vec![a], FrameRange::new(FrameIndex(0), FrameIndex(12)));
        h.execute(&mut d, Box::new(first)).unwrap();
        let object_layer = d.layers().iter().last().unwrap().id;
        let shown = picture(&d);
        // Wrap frames 3..9 of the object layer (the instance key sits at 0).
        let second = ConvertToSymbol::new(
            vec![object_layer],
            FrameRange::new(FrameIndex(3), FrameIndex(9)),
        );
        h.execute(&mut d, Box::new(second)).unwrap();
        assert_eq!(d.symbols().len(), 2);
        assert_eq!(d.next_symbol_name(), "Object 3");
        let sorted = |p: Vec<Vec<f32>>| {
            p.into_iter()
                .map(|mut v| {
                    v.sort_by(f32::total_cmp);
                    v
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(sorted(picture(&d)), sorted(shown));
    }
}
