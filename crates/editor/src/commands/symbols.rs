//! Object commands (ADR-018): turning layers and frames, or canvas items,
//! into a symbol, placing and moving instances, motion tweens between
//! instance keys (ADR-019), and managing symbols in the library.

use super::{editable_layer, frame_in_range};
use crate::{Command, CommandError};
use animation_core::{DrawingId, FrameIndex, FrameRange, Interpolation, LayerId, SymbolId};
use document::drawing::{DrawingContent, ItemId};
use document::scene::Layer;
use document::timeline::{InstanceKey, InstanceMode, InstanceTransform, Keyframe, Track};
use document::{Document, Symbol};
use std::any::Any;
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

/// A new key on `frame` that continues what `track` shows there: the same
/// drawing, or the instance with its playback advanced and, inside a motion
/// tween, its displayed (blended) placement and opacity. `None` for an
/// empty key or no key.
fn continuation(doc: &Document, track: &Track, frame: FrameIndex) -> Option<Keyframe> {
    let (at, key) = track.exposed_key(frame)?;
    match key {
        Keyframe::Drawing(_) => Some(*key),
        Keyframe::Instance(_) => {
            let (_, shown) = track.displayed_instance(frame)?;
            let length = doc.symbol(shown.symbol).map_or(1, |s| s.frame_count);
            Some(Keyframe::Instance(shown.advanced(frame.0 - at.0, length)))
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
    symbol_id: SymbolId,
    layer_id: LayerId,
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
            symbol_id: SymbolId::new(),
            layer_id: LayerId::new(),
            built: None,
            previous: None,
        }
    }

    /// Names the symbol (default: the next free "Object N").
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// ID of the symbol this command creates (fixed now so redo reuses it).
    pub fn symbol_id(&self) -> SymbolId {
        self.symbol_id
    }

    /// ID of the new layer holding the instance.
    pub fn layer_id(&self) -> LayerId {
        self.layer_id
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
        symbol.id = self.symbol_id;
        let mut copies: BTreeMap<DrawingId, DrawingId> = BTreeMap::new();
        let mut drawings = Vec::new();
        let mut tracks = Vec::new();
        for &(_, id) in &layers {
            let source = stack.get(id).ok_or(CommandError::LayerNotFound)?;
            let track = doc.track(id).ok_or(CommandError::LayerNotFound)?;

            // The symbol's copy of the range.
            let mut inner = Track::new();
            if track.key_at(start).is_none()
                && let Some(key) = continuation(doc, track, start)
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
                .then(|| continuation(doc, track, end))
                .flatten();
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
        layer.id = self.layer_id;
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

/// Turns items selected on the canvas into a one-frame symbol (object).
///
/// The items are taken out of the drawing that `layer` shows at `frame`
/// and moved so the centre of their bounds is the symbol's origin. A new
/// layer right above `layer` places the object for the same frames that
/// drawing is exposed, with its translation set to that centre, so nothing
/// moves on the canvas.
#[derive(Debug)]
pub struct ConvertItemsToSymbol {
    layer: LayerId,
    frame: FrameIndex,
    items: Vec<ItemId>,
    name: Option<String>,
    symbol_id: SymbolId,
    layer_id: LayerId,
    built: Option<Conversion>,
    previous: Option<Previous>,
}

impl ConvertItemsToSymbol {
    /// Creates the command.
    pub fn new(layer: LayerId, frame: FrameIndex, items: Vec<ItemId>) -> Self {
        Self {
            layer,
            frame,
            items,
            name: None,
            symbol_id: SymbolId::new(),
            layer_id: LayerId::new(),
            built: None,
            previous: None,
        }
    }

    /// Names the symbol (default: the next free "Object N").
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// ID of the symbol this command creates (fixed now so redo reuses it).
    pub fn symbol_id(&self) -> SymbolId {
        self.symbol_id
    }

    /// ID of the new layer holding the instance.
    pub fn layer_id(&self) -> LayerId {
        self.layer_id
    }

    fn build(&self, doc: &Document) -> Result<Conversion, CommandError> {
        editable_layer(doc, self.layer)?;
        frame_in_range(doc, self.frame)?;
        let track = doc.track(self.layer).ok_or(CommandError::LayerNotFound)?;
        let source_id = track
            .drawing_at(self.frame)
            .ok_or(CommandError::NothingToConvert)?;
        let exposure = track
            .exposure_range(self.frame, doc.frame_count)
            .ok_or(CommandError::NothingToConvert)?;
        let source = doc
            .drawing(source_id)
            .and_then(DrawingContent::as_vector)
            .ok_or(CommandError::DrawingNotFound)?;
        let items: Vec<ItemId> = self
            .items
            .iter()
            .copied()
            .filter(|&i| source.contains_item(i))
            .collect();
        let (min, max) = source
            .items_bounds(&items)
            .ok_or(CommandError::NothingToConvert)?;
        let centre = (min + max) * 0.5;

        let mut kept = source.clone();
        let mut taken = source.clone();
        for item in source.all_items() {
            let target = if items.contains(&item) {
                &mut kept
            } else {
                &mut taken
            };
            match item {
                ItemId::Stroke(id) => drop(target.remove_stroke(id)),
                ItemId::Fill(id) => drop(target.remove_fill(id)),
                ItemId::Image(id) => drop(target.remove_image(id)),
            }
        }
        taken.translate(-centre);
        let content = DrawingContent::Vector(taken).duplicate();

        let name = self.name.clone().unwrap_or_else(|| doc.next_symbol_name());
        let mut symbol = Symbol::new(name.clone(), 1);
        symbol.id = self.symbol_id;
        let drawing = DrawingId::new();
        let mut inner = Track::new();
        inner.set_key(FrameIndex(0), Keyframe::Drawing(drawing));
        let layer_name = doc
            .layers()
            .get(self.layer)
            .map_or_else(|| "Layer 1".to_owned(), |l| l.name.clone());
        symbol.push_layer(Layer::new(layer_name), inner);

        let (mut layer, index) = object_layer(doc, &name, self.layer)?;
        layer.id = self.layer_id;
        let mut track = Track::new();
        let key =
            InstanceKey::new(symbol.id).with_transform(InstanceTransform::from_translation(centre));
        track.set_key(exposure.start, Keyframe::Instance(key));
        if exposure.end.0 < doc.frame_count {
            track.set_key(exposure.end, Keyframe::Empty);
        }
        Ok(Conversion {
            symbol,
            drawings: vec![(drawing, content)],
            tracks: Vec::new(),
            replaced_drawings: vec![(source_id, DrawingContent::Vector(kept))],
            layer,
            index,
            track,
        })
    }
}

impl Command for ConvertItemsToSymbol {
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

/// Where [`PlaceInstance`] puts its key.
#[derive(Debug, Clone)]
enum PlaceTarget {
    /// An existing layer.
    Layer(LayerId),
    /// A new layer inserted at `index`.
    NewLayer { layer: Box<Layer>, index: usize },
}

/// Places an object: sets an instance key at a frame, replacing any key
/// there, on an existing layer or on a new one.
#[derive(Debug)]
pub struct PlaceInstance {
    target: PlaceTarget,
    frame: FrameIndex,
    key: InstanceKey,
    replaced: Option<Option<Keyframe>>,
}

impl PlaceInstance {
    /// Places `key` on `layer` at `frame`.
    pub fn new(layer: LayerId, frame: FrameIndex, key: InstanceKey) -> Self {
        Self {
            target: PlaceTarget::Layer(layer),
            frame,
            key,
            replaced: None,
        }
    }

    /// Places `key` at `frame` on a new layer called `name`, inserted right
    /// above `above` (in its group) or on top of the stack when `None`.
    pub fn on_new_layer(
        doc: &Document,
        name: impl Into<String>,
        above: Option<LayerId>,
        frame: FrameIndex,
        key: InstanceKey,
    ) -> Self {
        let name = name.into();
        let (layer, index) = match above.and_then(|id| object_layer(doc, &name, id).ok()) {
            Some((mut layer, index)) => {
                layer.transform = document::scene::Transform2D::IDENTITY;
                (layer, index)
            }
            None => (Layer::new(name), doc.layers().len()),
        };
        Self {
            target: PlaceTarget::NewLayer {
                layer: Box::new(layer),
                index,
            },
            frame,
            key,
            replaced: None,
        }
    }

    /// The layer the key goes on.
    pub fn layer_id(&self) -> LayerId {
        match &self.target {
            PlaceTarget::Layer(id) => *id,
            PlaceTarget::NewLayer { layer, .. } => layer.id,
        }
    }
}

impl Command for PlaceInstance {
    fn label(&self) -> &str {
        "Place Object"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        frame_in_range(doc, self.frame)?;
        valid_instance(doc, &self.key)?;
        if let PlaceTarget::NewLayer { layer, index } = &self.target {
            doc.insert_layer(*index, (**layer).clone());
        }
        let layer = self.layer_id();
        if let Err(err) = editable_layer(doc, layer) {
            if matches!(self.target, PlaceTarget::NewLayer { .. }) {
                doc.remove_layer(layer);
            }
            return Err(err);
        }
        let track = doc.track_mut(layer).ok_or(CommandError::LayerNotFound)?;
        self.replaced = Some(track.set_key(self.frame, Keyframe::Instance(self.key)));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let replaced = self
            .replaced
            .take()
            .ok_or(CommandError::NoInstance(self.frame))?;
        let layer = self.layer_id();
        if matches!(self.target, PlaceTarget::NewLayer { .. }) {
            doc.remove_layer(layer).ok_or(CommandError::LayerNotFound)?;
            return Ok(());
        }
        let track = doc.track_mut(layer).ok_or(CommandError::LayerNotFound)?;
        match replaced {
            Some(key) => track.set_key(self.frame, key),
            None => track.remove_key(self.frame),
        };
        Ok(())
    }
}

/// Fails unless `key` places an existing symbol with sane values.
fn valid_instance(doc: &Document, key: &InstanceKey) -> Result<(), CommandError> {
    let symbol = doc.symbol(key.symbol).ok_or(CommandError::SymbolNotFound)?;
    if !key.transform.is_finite() {
        return Err(CommandError::InvalidValue(
            "the object's placement is not a number",
        ));
    }
    if !(0.0..=1.0).contains(&key.opacity) {
        return Err(CommandError::InvalidValue(
            "opacity must be between 0 and 100%",
        ));
    }
    if key.first_frame.0 >= symbol.frame_count {
        return Err(CommandError::InvalidValue(
            "the first frame is past the end of the object",
        ));
    }
    Ok(())
}

/// The instance key exposed on `layer` at `frame`, with its key frame.
fn exposed_instance(
    doc: &Document,
    layer: LayerId,
    frame: FrameIndex,
) -> Result<(FrameIndex, InstanceKey), CommandError> {
    doc.track(layer)
        .ok_or(CommandError::LayerNotFound)?
        .instance_at(frame)
        .map(|(f, k)| (f, *k))
        .ok_or(CommandError::NoInstance(frame))
}

/// Moves, scales or rotates the object shown on a layer at a frame.
///
/// On the instance key's own frame the key changes. On a later frame of
/// its exposure, a new instance key is made there, continuing the same
/// playback with the new placement. Inside a motion tween (ADR-019) the
/// new key starts from what that frame displays: its opacity is the
/// blended one, and it keeps the tween, so the tween now runs from the
/// first key to the new one and on from it to the old end key.
#[derive(Debug)]
pub struct SetInstanceTransform {
    layer: LayerId,
    frame: FrameIndex,
    transform: InstanceTransform,
    /// The key at `frame` before, or `None` if the command created it.
    previous: Option<Option<InstanceKey>>,
}

impl SetInstanceTransform {
    /// Creates the command.
    pub fn new(layer: LayerId, frame: FrameIndex, transform: InstanceTransform) -> Self {
        Self {
            layer,
            frame,
            transform,
            previous: None,
        }
    }
}

impl Command for SetInstanceTransform {
    fn label(&self) -> &str {
        "Move Object"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        if !self.transform.is_finite() {
            return Err(CommandError::InvalidValue(
                "the object's placement is not a number",
            ));
        }
        let (at, key) = exposed_instance(doc, self.layer, self.frame)?;
        let length = doc.symbol(key.symbol).map_or(1, |s| s.frame_count);
        let (previous, new) = if at == self.frame {
            (Some(key), key.with_transform(self.transform))
        } else {
            frame_in_range(doc, self.frame)?;
            let (_, shown) = doc
                .track(self.layer)
                .and_then(|t| t.displayed_instance(self.frame))
                .unwrap_or((at, key));
            let later = shown.advanced(self.frame.0 - at.0, length);
            (None, later.with_transform(self.transform))
        };
        doc.track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?
            .set_key(self.frame, Keyframe::Instance(new));
        self.previous = Some(previous);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let previous = self
            .previous
            .take()
            .ok_or(CommandError::NoInstance(self.frame))?;
        let track = doc
            .track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?;
        match previous {
            Some(key) => track.set_key(self.frame, Keyframe::Instance(key)),
            None => track.remove_key(self.frame),
        };
        Ok(())
    }
}

/// Adds, changes or removes the motion tween (ADR-019) from the instance
/// key on `frame` to the next key on the layer.
///
/// Setting a tween is refused with [`CommandError::NotTweenable`] unless
/// the key on `frame` and the next key are instances of the same object.
/// Removing one only needs the key on `frame` to be an instance, so a tween
/// whose pair was broken (and is inactive) can be removed too.
/// `Some(Interpolation::Step)` means no tween and removes it.
#[derive(Debug)]
pub struct SetTween {
    layer: LayerId,
    frame: FrameIndex,
    tween: Option<Interpolation>,
    /// The key's tween before the change.
    previous: Option<Option<Interpolation>>,
}

impl SetTween {
    /// Creates the command.
    pub fn new(layer: LayerId, frame: FrameIndex, tween: Option<Interpolation>) -> Self {
        Self {
            layer,
            frame,
            tween: tween.filter(|t| *t != Interpolation::Step),
            previous: None,
        }
    }

    /// Whether a tween can start on the key at `frame` of `layer`: it and
    /// the next key are instances of the same object.
    pub fn can_tween(doc: &Document, layer: LayerId, frame: FrameIndex) -> bool {
        let Some(track) = doc.track(layer) else {
            return false;
        };
        let Some(start) = track.key_at(frame).and_then(Keyframe::instance) else {
            return false;
        };
        track
            .next_key_after(frame)
            .and_then(|(_, k)| k.instance())
            .is_some_and(|end| end.symbol == start.symbol)
    }

    fn set(doc: &mut Document, layer: LayerId, frame: FrameIndex, tween: Option<Interpolation>) {
        if let Some((_, Keyframe::Instance(key))) = doc
            .track_mut(layer)
            .and_then(|t| t.keys_mut().find(|(f, _)| *f == frame))
        {
            key.tween = tween;
        }
    }
}

impl Command for SetTween {
    fn label(&self) -> &str {
        if self.tween.is_some() {
            "Create Motion Tween"
        } else {
            "Remove Tween"
        }
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        let key = doc
            .track(self.layer)
            .ok_or(CommandError::LayerNotFound)?
            .key_at(self.frame)
            .and_then(Keyframe::instance)
            .copied()
            .ok_or(CommandError::NotTweenable)?;
        if self.tween.is_some() && !Self::can_tween(doc, self.layer, self.frame) {
            return Err(CommandError::NotTweenable);
        }
        Self::set(doc, self.layer, self.frame, self.tween);
        self.previous = Some(key.tween);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let previous = self
            .previous
            .take()
            .ok_or(CommandError::NoInstance(self.frame))?;
        Self::set(doc, self.layer, self.frame, previous);
        Ok(())
    }
}

/// The playback settings of an instance (see [`SetInstanceProps`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InstanceProps {
    /// Loop or play once.
    pub mode: InstanceMode,
    /// The symbol frame shown on the key's own frame.
    pub first_frame: FrameIndex,
    /// Opacity in `0..=1`.
    pub opacity: f32,
}

impl InstanceProps {
    /// The settings of `key`.
    pub fn of(key: &InstanceKey) -> Self {
        Self {
            mode: key.mode,
            first_frame: key.first_frame,
            opacity: key.opacity,
        }
    }

    fn apply_to(self, key: InstanceKey) -> InstanceKey {
        InstanceKey {
            mode: self.mode,
            first_frame: self.first_frame,
            opacity: self.opacity,
            ..key
        }
    }
}

/// Changes Loop / Play once, the first frame and the opacity of the
/// instance key exposed on a layer at a frame.
#[derive(Debug)]
pub struct SetInstanceProps {
    layer: LayerId,
    frame: FrameIndex,
    props: InstanceProps,
    gesture: Option<u64>,
    /// The key frame and its settings before the change.
    previous: Option<(FrameIndex, InstanceProps)>,
}

impl SetInstanceProps {
    /// Creates the command.
    pub fn new(layer: LayerId, frame: FrameIndex, props: InstanceProps) -> Self {
        Self {
            layer,
            frame,
            props,
            gesture: None,
            previous: None,
        }
    }

    /// Marks the change as part of drag `gesture`: consecutive changes of
    /// the same instance in one gesture undo as one step.
    pub fn in_gesture(mut self, gesture: u64) -> Self {
        self.gesture = Some(gesture);
        self
    }
}

impl Command for SetInstanceProps {
    fn label(&self) -> &str {
        "Change Object Settings"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        let (at, key) = exposed_instance(doc, self.layer, self.frame)?;
        let new = self.props.apply_to(key);
        valid_instance(doc, &new)?;
        doc.track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?
            .set_key(at, Keyframe::Instance(new));
        self.previous = Some((at, InstanceProps::of(&key)));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let (at, props) = self
            .previous
            .take()
            .ok_or(CommandError::NoInstance(self.frame))?;
        let track = doc
            .track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?;
        let key = track
            .key_at(at)
            .and_then(Keyframe::instance)
            .copied()
            .ok_or(CommandError::NoInstance(at))?;
        track.set_key(at, Keyframe::Instance(props.apply_to(key)));
        Ok(())
    }

    fn merge(&mut self, next: &dyn Command) -> bool {
        let Some(next) = (next as &dyn Any).downcast_ref::<Self>() else {
            return false;
        };
        let same_key = match (&self.previous, &next.previous) {
            (Some((a, _)), Some((b, _))) => a == b,
            _ => false,
        };
        if self.gesture.is_none()
            || next.gesture != self.gesture
            || next.layer != self.layer
            || !same_key
        {
            return false;
        }
        self.props = next.props;
        true
    }
}

/// Renames a symbol (object) in the library.
#[derive(Debug)]
pub struct RenameSymbol {
    id: SymbolId,
    name: String,
}

impl RenameSymbol {
    /// Creates the command.
    pub fn new(id: SymbolId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }

    fn swap(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let symbol = doc
            .symbol_mut(self.id)
            .ok_or(CommandError::SymbolNotFound)?;
        std::mem::swap(&mut symbol.name, &mut self.name);
        Ok(())
    }
}

impl Command for RenameSymbol {
    fn label(&self) -> &str {
        "Rename Object"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }
}

/// Removes a symbol from the library. Refused with
/// [`CommandError::SymbolInUse`] while an instance key places it.
#[derive(Debug)]
pub struct RemoveSymbol {
    id: SymbolId,
    removed: Option<Symbol>,
}

impl RemoveSymbol {
    /// Creates the command.
    pub fn new(id: SymbolId) -> Self {
        Self { id, removed: None }
    }
}

impl Command for RemoveSymbol {
    fn label(&self) -> &str {
        "Delete Object"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let symbol = doc.symbol(self.id).ok_or(CommandError::SymbolNotFound)?;
        if doc.symbol_in_use(self.id) {
            return Err(CommandError::SymbolInUse(symbol.name.clone()));
        }
        self.removed = doc.remove_symbol(self.id);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let symbol = self.removed.take().ok_or(CommandError::SymbolNotFound)?;
        doc.insert_symbol(symbol);
        Ok(())
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
    /// every stroke drawn.
    fn picture(d: &Document) -> Vec<Vec<f32>> {
        (0..d.frame_count)
            .map(|f| {
                d.evaluate(FrameIndex(f))
                    .layers
                    .iter()
                    .flat_map(|l| {
                        l.drawing
                            .strokes()
                            .iter()
                            .map(|s| l.transform.transform_point2(s.points[0].pos).x)
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
    #[test]
    fn convert_items_centres_the_object_and_keeps_the_picture() {
        let mut d = doc();
        let l = d.add_layer("L");
        let mut v = VectorDrawing::new();
        let stroke = |x: f32| {
            Stroke::new(
                vec![
                    StrokePoint::new(Vec2::new(x, 10.0)),
                    StrokePoint::new(Vec2::new(x + 10.0, 30.0)),
                ],
                BrushStyle {
                    width: 0.0,
                    ..BrushStyle::default()
                },
            )
        };
        let (keep, take) = (stroke(0.0), stroke(40.0));
        let take_id = ItemId::Stroke(take.id);
        v.push_stroke(keep);
        v.push_stroke(take);
        let id = d.add_drawing(DrawingContent::Vector(v));
        let t = d.track_mut(l).unwrap();
        t.set_key(FrameIndex(2), Keyframe::Drawing(id));
        t.set_key(FrameIndex(7), Keyframe::Empty);
        let before = d.clone();
        let shown = picture(&d);

        let mut h = History::new();
        h.execute(
            &mut d,
            Box::new(ConvertItemsToSymbol::new(l, FrameIndex(4), vec![take_id])),
        )
        .unwrap();
        let sorted = |p: Vec<Vec<f32>>| {
            p.into_iter()
                .map(|mut v| {
                    v.sort_by(f32::total_cmp);
                    v
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(sorted(picture(&d)), sorted(shown));
        let symbol = d.symbols().values().next().unwrap();
        assert_eq!(symbol.frame_count, 1);
        let object = d.layers().iter().last().unwrap().id;
        let track = d.track(object).unwrap();
        let key = track.key_at(FrameIndex(2)).unwrap().instance().unwrap();
        // Bounds (40,10)-(50,30): centre (45,20).
        assert_eq!(key.transform.translation, Vec2::new(45.0, 20.0));
        assert_eq!(track.key_at(FrameIndex(7)), Some(&Keyframe::Empty));
        assert_eq!(
            d.drawing(id).unwrap().as_vector().unwrap().strokes().len(),
            1
        );

        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
        let mut h = History::new();
        assert_eq!(
            h.execute(
                &mut d,
                Box::new(ConvertItemsToSymbol::new(l, FrameIndex(4), vec![])),
            ),
            Err(CommandError::NothingToConvert)
        );
        assert_eq!(
            h.execute(
                &mut d,
                Box::new(ConvertItemsToSymbol::new(l, FrameIndex(8), vec![take_id])),
            ),
            Err(CommandError::NothingToConvert)
        );
    }
    /// A document with a 12-frame symbol and an empty layer.
    fn with_symbol() -> (Document, LayerId, SymbolId) {
        let mut d = doc();
        let l = d.add_layer("L");
        let mut symbol = Symbol::new("Ball", 12);
        let id = drawing(&mut d, 0.0);
        let mut track = Track::new();
        track.set_key(FrameIndex(0), Keyframe::Drawing(id));
        symbol.push_layer(Layer::new("in"), track);
        let sid = symbol.id;
        d.insert_symbol(symbol);
        (d, l, sid)
    }

    fn check_inverse(d: &mut Document, cmd: Box<dyn Command>) {
        let before = d.clone();
        let mut h = History::new();
        h.execute(d, cmd).unwrap();
        assert_ne!(*d, before);
        let after = d.clone();
        h.undo(d).unwrap();
        assert_eq!(*d, before);
        h.redo(d).unwrap();
        assert_eq!(*d, after);
        h.undo(d).unwrap();
    }

    #[test]
    fn place_instance_on_a_layer_or_a_new_one() {
        let (mut d, l, sid) = with_symbol();
        let key = InstanceKey::new(sid);
        check_inverse(&mut d, Box::new(PlaceInstance::new(l, FrameIndex(3), key)));
        // Replacing an existing key restores it on undo.
        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(3), Keyframe::Empty);
        check_inverse(&mut d, Box::new(PlaceInstance::new(l, FrameIndex(3), key)));

        let cmd = PlaceInstance::on_new_layer(&d, "Ball", Some(l), FrameIndex(0), key);
        let new = cmd.layer_id();
        let mut h = History::new();
        h.execute(&mut d, Box::new(cmd)).unwrap();
        assert_eq!(d.layers().index_of(new), Some(1));
        assert_eq!(
            d.track(new).unwrap().key_at(FrameIndex(0)),
            Some(&Keyframe::Instance(key))
        );
        h.undo(&mut d).unwrap();
        let on_top = PlaceInstance::on_new_layer(&d, "Ball", None, FrameIndex(0), key);
        check_inverse(&mut d, Box::new(on_top));

        let mut h = History::new();
        let missing = InstanceKey::new(SymbolId::new());
        assert_eq!(
            h.execute(
                &mut d,
                Box::new(PlaceInstance::new(l, FrameIndex(0), missing))
            ),
            Err(CommandError::SymbolNotFound)
        );
        let before = d.clone();
        d.layer_mut(l).unwrap().locked = true;
        let locked = d.clone();
        assert_eq!(
            h.execute(&mut d, Box::new(PlaceInstance::new(l, FrameIndex(0), key))),
            Err(CommandError::LayerLocked)
        );
        assert_eq!(d, locked);
        d = before;
        assert_eq!(
            h.execute(&mut d, Box::new(PlaceInstance::new(l, FrameIndex(30), key))),
            Err(CommandError::FrameOutOfRange {
                frame: FrameIndex(30),
                frame_count: 24
            })
        );
    }

    #[test]
    fn set_instance_transform_edits_the_key_or_starts_a_new_one() {
        let (mut d, l, sid) = with_symbol();
        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(2), Keyframe::Instance(InstanceKey::new(sid)));
        let moved = InstanceTransform::from_translation(Vec2::new(5.0, 6.0));
        check_inverse(
            &mut d,
            Box::new(SetInstanceTransform::new(l, FrameIndex(2), moved)),
        );
        check_inverse(
            &mut d,
            Box::new(SetInstanceTransform::new(l, FrameIndex(7), moved)),
        );

        let mut h = History::new();
        h.execute(
            &mut d,
            Box::new(SetInstanceTransform::new(l, FrameIndex(7), moved)),
        )
        .unwrap();
        let track = d.track(l).unwrap();
        let first = track.key_at(FrameIndex(2)).unwrap().instance().unwrap();
        let second = track.key_at(FrameIndex(7)).unwrap().instance().unwrap();
        assert_eq!(first.transform, InstanceTransform::IDENTITY);
        assert_eq!(second.transform, moved);
        // The new key continues the playback: frame 7 still shows inner frame 5.
        assert_eq!(second.first_frame, FrameIndex(5));

        assert_eq!(
            h.execute(
                &mut d,
                Box::new(SetInstanceTransform::new(l, FrameIndex(1), moved))
            ),
            Err(CommandError::NoInstance(FrameIndex(1)))
        );
    }

    /// `with_symbol` with instance keys on frames 2 (x = 0, opacity 1)
    /// and 12 (x = 100, opacity 0.5, continuing playback), no tween yet.
    fn two_instance_keys() -> (Document, LayerId, SymbolId) {
        let (mut d, l, sid) = with_symbol();
        let start = InstanceKey::new(sid);
        let end = InstanceKey {
            opacity: 0.5,
            ..start
                .advanced(10, 12)
                .with_transform(InstanceTransform::from_translation(Vec2::new(100.0, 0.0)))
        };
        let t = d.track_mut(l).unwrap();
        t.set_key(FrameIndex(2), Keyframe::Instance(start));
        t.set_key(FrameIndex(12), Keyframe::Instance(end));
        (d, l, sid)
    }

    fn tween_of(d: &Document, l: LayerId, frame: u32) -> Option<Interpolation> {
        d.track(l)
            .unwrap()
            .key_at(FrameIndex(frame))
            .and_then(Keyframe::instance)
            .unwrap()
            .tween
    }

    #[test]
    fn set_tween_adds_changes_and_removes_exactly() {
        let (mut d, l, _) = two_instance_keys();
        for ease in Interpolation::TWEENS {
            check_inverse(
                &mut d,
                Box::new(SetTween::new(l, FrameIndex(2), Some(ease))),
            );
        }
        let mut h = History::new();
        h.execute(
            &mut d,
            Box::new(SetTween::new(l, FrameIndex(2), Some(Interpolation::EaseIn))),
        )
        .unwrap();
        assert_eq!(tween_of(&d, l, 2), Some(Interpolation::EaseIn));
        check_inverse(
            &mut d,
            Box::new(SetTween::new(
                l,
                FrameIndex(2),
                Some(Interpolation::EaseOut),
            )),
        );
        check_inverse(&mut d, Box::new(SetTween::new(l, FrameIndex(2), None)));
        // Step means no tween.
        h.execute(
            &mut d,
            Box::new(SetTween::new(l, FrameIndex(2), Some(Interpolation::Step))),
        )
        .unwrap();
        assert_eq!(tween_of(&d, l, 2), None);
    }

    #[test]
    fn set_tween_only_works_between_keys_of_one_object() {
        let (mut d, l, sid) = two_instance_keys();
        let mut h = History::new();
        let mut refused = |d: &mut Document, frame: u32| {
            let before = d.clone();
            let result = h.execute(
                d,
                Box::new(SetTween::new(
                    l,
                    FrameIndex(frame),
                    Some(Interpolation::Linear),
                )),
            );
            assert_eq!(*d, before);
            result
        };
        // The last key has no next key; frame 5 is not a key.
        assert_eq!(refused(&mut d, 12), Err(CommandError::NotTweenable));
        assert_eq!(refused(&mut d, 5), Err(CommandError::NotTweenable));
        // A drawing key, or an instance of another object, as the next key.
        let other = Symbol::new("Other", 1);
        let other_id = other.id;
        d.insert_symbol(other);
        d.track_mut(l).unwrap().set_key(
            FrameIndex(12),
            Keyframe::Instance(InstanceKey::new(other_id)),
        );
        assert_eq!(refused(&mut d, 2), Err(CommandError::NotTweenable));
        let drawn = drawing(&mut d, 0.0);
        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(12), Keyframe::Drawing(drawn));
        assert_eq!(refused(&mut d, 2), Err(CommandError::NotTweenable));
        assert_eq!(refused(&mut d, 12), Err(CommandError::NotTweenable));
        // An inactive tween can still be removed.
        d.track_mut(l).unwrap().set_key(
            FrameIndex(2),
            Keyframe::Instance(InstanceKey {
                tween: Some(Interpolation::Linear),
                ..InstanceKey::new(sid)
            }),
        );
        check_inverse(&mut d, Box::new(SetTween::new(l, FrameIndex(2), None)));
        // Locked layers are refused.
        d.layer_mut(l).unwrap().locked = true;
        assert_eq!(
            History::new().execute(&mut d, Box::new(SetTween::new(l, FrameIndex(2), None))),
            Err(CommandError::LayerLocked)
        );
    }

    #[test]
    fn moving_inside_a_tween_starts_from_what_is_shown() {
        let (mut d, l, _) = two_instance_keys();
        let mut h = History::new();
        h.execute(
            &mut d,
            Box::new(SetTween::new(l, FrameIndex(2), Some(Interpolation::Linear))),
        )
        .unwrap();
        // Frame 7 is half way: x = 50, opacity 0.75, inner frame 5.
        let (_, shown) = d
            .track(l)
            .unwrap()
            .displayed_instance(FrameIndex(7))
            .unwrap();
        assert_eq!(shown.transform.translation.x, 50.0);
        // The tools move what is shown by (0, 20).
        let moved = InstanceTransform::from_translation(Vec2::new(50.0, 20.0));
        check_inverse(
            &mut d,
            Box::new(SetInstanceTransform::new(l, FrameIndex(7), moved)),
        );
        h.execute(
            &mut d,
            Box::new(SetInstanceTransform::new(l, FrameIndex(7), moved)),
        )
        .unwrap();
        let track = d.track(l).unwrap();
        let middle = *track.key_at(FrameIndex(7)).unwrap().instance().unwrap();
        // The new key shows exactly the moved picture, with the blended
        // opacity and continuing playback, and keeps tweening on.
        assert_eq!(middle.transform, moved);
        assert_eq!(middle.opacity, 0.75);
        assert_eq!(middle.first_frame, FrameIndex(5));
        assert_eq!(middle.tween, Some(Interpolation::Linear));
        assert_eq!(track.displayed_instance(FrameIndex(7)).unwrap().1, middle);
        // The tween now runs 2 → 7 → 12.
        let spans: Vec<_> = track.tweens().map(|s| (s.start.0, s.end.0)).collect();
        assert_eq!(spans, [(2, 7), (7, 12)]);
        let x = |f| {
            track
                .displayed_instance(FrameIndex(f))
                .unwrap()
                .1
                .transform
                .translation
        };
        assert_eq!(x(2), Vec2::ZERO);
        assert_eq!(x(12), Vec2::new(100.0, 0.0));
        assert!((x(9) - Vec2::new(70.0, 12.0)).length() < 1e-4);
    }

    #[test]
    fn set_instance_props_validates_and_merges_gestures() {
        let (mut d, l, sid) = with_symbol();
        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(2), Keyframe::Instance(InstanceKey::new(sid)));
        let props = InstanceProps {
            mode: InstanceMode::Once,
            first_frame: FrameIndex(4),
            opacity: 0.5,
        };
        check_inverse(
            &mut d,
            Box::new(SetInstanceProps::new(l, FrameIndex(9), props)),
        );

        let mut h = History::new();
        for bad in [
            InstanceProps {
                first_frame: FrameIndex(12),
                ..props
            },
            InstanceProps {
                opacity: 1.5,
                ..props
            },
        ] {
            assert!(matches!(
                h.execute(
                    &mut d,
                    Box::new(SetInstanceProps::new(l, FrameIndex(2), bad))
                ),
                Err(CommandError::InvalidValue(_))
            ));
        }

        let before = d.clone();
        for opacity in [0.9, 0.7, 0.4] {
            let p = InstanceProps { opacity, ..props };
            h.execute(
                &mut d,
                Box::new(SetInstanceProps::new(l, FrameIndex(3), p).in_gesture(7)),
            )
            .unwrap();
        }
        assert_eq!(h.undo_len(), 1);
        let key = d
            .track(l)
            .unwrap()
            .key_at(FrameIndex(2))
            .unwrap()
            .instance()
            .unwrap();
        assert_eq!(key.opacity, 0.4);
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
    }

    #[test]
    fn rename_and_remove_symbols() {
        let (mut d, l, sid) = with_symbol();
        check_inverse(&mut d, Box::new(RenameSymbol::new(sid, "Wheel")));
        check_inverse(&mut d, Box::new(RemoveSymbol::new(sid)));

        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(0), Keyframe::Instance(InstanceKey::new(sid)));
        let mut h = History::new();
        assert_eq!(
            h.execute(&mut d, Box::new(RemoveSymbol::new(sid))),
            Err(CommandError::SymbolInUse("Ball".into()))
        );
        assert_eq!(
            h.execute(&mut d, Box::new(RemoveSymbol::new(SymbolId::new()))),
            Err(CommandError::SymbolNotFound)
        );
    }
}
