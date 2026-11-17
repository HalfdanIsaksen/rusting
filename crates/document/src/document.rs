//! The [`Document`] type.

use crate::assets::Asset;
use crate::symbol::Symbol;
use animation_core::{
    AssetId, DocumentId, DrawingId, Fps, FrameIndex, FrameRange, GroupId, LayerId, Rgba, SymbolId,
};
use drawing::DrawingContent;
use scene::{Arrangement, Layer, LayerGroup, LayerStack};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use timeline::Track;

/// Canvas size in document units (1 unit = 1 pixel at 100% zoom).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanvasSize {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// A named range of frames, e.g. "walk" or "jump". Playback loops inside the
/// tag the playhead is in, and sprite sheets put each tag on its own row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameTag {
    /// Display name.
    pub name: String,
    /// The tagged frames.
    pub range: FrameRange,
}

/// Everything removed by [`Document::remove_layer`], kept so the removal can be undone.
#[derive(Clone, Debug, PartialEq)]
pub struct RemovedLayer {
    /// Former position in the stack (0 = bottom).
    pub index: usize,
    /// The layer itself.
    pub layer: Layer,
    /// The layer's track.
    pub track: Track,
}

/// A complete animation project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Document {
    /// Stable document ID.
    pub id: DocumentId,
    /// Project name.
    pub name: String,
    /// Canvas size.
    pub canvas: CanvasSize,
    /// Background color. `None` means a transparent canvas.
    pub background: Option<Rgba>,
    /// Playback frame rate.
    pub fps: Fps,
    /// Length of the animation in frames.
    pub frame_count: u32,
    /// Frame tags, in the order they were made. They may overlap.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<FrameTag>,
    layers: LayerStack,
    tracks: BTreeMap<LayerId, Track>,
    drawings: BTreeMap<DrawingId, DrawingContent>,
    /// The asset library (ADR-017); absent in files means empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    assets: BTreeMap<AssetId, Asset>,
    /// Symbols (reusable animated objects, ADR-018); absent in files means
    /// none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    symbols: BTreeMap<SymbolId, Symbol>,
}

impl Document {
    /// Creates an empty document with a white background, 24 fps and 24 frames.
    pub fn new(name: impl Into<String>, canvas: CanvasSize) -> Self {
        Self {
            id: DocumentId::new(),
            name: name.into(),
            canvas,
            background: Some(Rgba::WHITE),
            fps: Fps::FPS_24,
            frame_count: 24,
            tags: Vec::new(),
            layers: LayerStack::new(),
            tracks: BTreeMap::new(),
            drawings: BTreeMap::new(),
            assets: BTreeMap::new(),
            symbols: BTreeMap::new(),
        }
    }

    /// The first tag containing `frame`.
    pub fn tag_at(&self, frame: FrameIndex) -> Option<&FrameTag> {
        self.tags.iter().find(|t| t.range.contains(frame))
    }

    /// The layer stack (bottom-to-top).
    pub fn layers(&self) -> &LayerStack {
        &self.layers
    }

    /// Mutable access to a layer's properties.
    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.layers.get_mut(id)
    }

    /// Moves a layer to `new_index` in the stack. Returns the old index.
    pub fn move_layer(&mut self, id: LayerId, new_index: usize) -> Option<usize> {
        self.layers.move_to(id, new_index)
    }

    /// Moves a layer to `new_index` and into `group` (see
    /// [`LayerStack::move_to_group`]). Returns the old index and group.
    pub fn move_layer_to_group(
        &mut self,
        id: LayerId,
        new_index: usize,
        group: Option<GroupId>,
    ) -> Option<(usize, Option<GroupId>)> {
        self.layers.move_to_group(id, new_index, group)
    }

    /// Adds a group record (see [`LayerStack::add_group`]).
    pub fn add_group(&mut self, group: LayerGroup) {
        self.layers.add_group(group);
    }

    /// Removes a group, taking its layers out of it. Returns its index in
    /// the group list and the group.
    pub fn remove_group(&mut self, id: GroupId) -> Option<(usize, LayerGroup)> {
        self.layers.remove_group(id)
    }

    /// Puts back a group removed by [`Document::remove_group`].
    pub fn insert_group(&mut self, index: usize, group: LayerGroup) {
        self.layers.insert_group(index, group);
    }

    /// Mutable access to a group's properties.
    pub fn group_mut(&mut self, id: GroupId) -> Option<&mut LayerGroup> {
        self.layers.group_mut(id)
    }

    /// Moves a whole group (see [`LayerStack::move_group`]).
    pub fn move_group(&mut self, id: GroupId, new_bottom: usize) -> Option<usize> {
        self.layers.move_group(id, new_bottom)
    }

    /// Reorders and regroups the layers (see [`LayerStack::set_arrangement`]).
    pub fn set_arrangement(&mut self, arrangement: &Arrangement) -> bool {
        self.layers.set_arrangement(arrangement)
    }

    /// Drops groups with no layers (only on a copy about to be saved, or
    /// when the undo history is cleared). Returns how many were dropped.
    pub fn remove_empty_groups(&mut self) -> usize {
        self.layers.remove_empty_groups()
    }

    /// Whether some group has no layers.
    pub fn has_empty_groups(&self) -> bool {
        self.layers
            .groups()
            .any(|g| self.layers.group_range(g.id).is_none())
    }

    /// Inserts a layer at `index` with an empty track. Returns its ID.
    pub fn insert_layer(&mut self, index: usize, layer: Layer) -> LayerId {
        let id = layer.id;
        self.layers.insert(index, layer);
        self.tracks.entry(id).or_default();
        id
    }

    /// Adds a new layer on top. Returns its ID.
    pub fn add_layer(&mut self, name: impl Into<String>) -> LayerId {
        let index = self.layers.len();
        self.insert_layer(index, Layer::new(name))
    }

    /// Removes a layer and its track. Drawings stay in the document so the
    /// removal can be undone; unused drawings are cleaned up on save later.
    pub fn remove_layer(&mut self, id: LayerId) -> Option<RemovedLayer> {
        let (index, layer) = self.layers.remove(id)?;
        let track = self.tracks.remove(&id).unwrap_or_default();
        Some(RemovedLayer {
            index,
            layer,
            track,
        })
    }

    /// Puts back a layer removed by [`Document::remove_layer`].
    pub fn restore_layer(&mut self, removed: RemovedLayer) {
        let id = removed.layer.id;
        self.layers.insert(removed.index, removed.layer);
        self.tracks.insert(id, removed.track);
    }

    /// The track of a layer.
    pub fn track(&self, layer: LayerId) -> Option<&Track> {
        self.tracks.get(&layer)
    }

    /// Mutable access to the track of a layer.
    pub fn track_mut(&mut self, layer: LayerId) -> Option<&mut Track> {
        self.tracks.get_mut(&layer)
    }

    /// All tracks keyed by layer.
    pub fn tracks(&self) -> &BTreeMap<LayerId, Track> {
        &self.tracks
    }

    /// Stores drawing content and returns its new ID.
    pub fn add_drawing(&mut self, content: DrawingContent) -> DrawingId {
        let id = DrawingId::new();
        self.drawings.insert(id, content);
        id
    }

    /// Looks up a drawing.
    pub fn drawing(&self, id: DrawingId) -> Option<&DrawingContent> {
        self.drawings.get(&id)
    }

    /// Mutable lookup of a drawing.
    pub fn drawing_mut(&mut self, id: DrawingId) -> Option<&mut DrawingContent> {
        self.drawings.get_mut(&id)
    }

    /// All drawings keyed by ID.
    pub fn drawings(&self) -> &BTreeMap<DrawingId, DrawingContent> {
        &self.drawings
    }

    /// Stores drawing content under a known ID, replacing and returning any
    /// previous content with that ID. Commands use this so redo reuses the ID
    /// chosen when the command was built.
    pub fn insert_drawing(
        &mut self,
        id: DrawingId,
        content: DrawingContent,
    ) -> Option<DrawingContent> {
        self.drawings.insert(id, content)
    }

    /// Removes a drawing from the pool. Keyframes that still refer to it are
    /// left alone; callers (undo of a command that created it) must have
    /// removed those first.
    pub fn remove_drawing(&mut self, id: DrawingId) -> Option<DrawingContent> {
        self.drawings.remove(&id)
    }

    /// The drawing exposed on `layer` at `frame`: the drawing of the last key at
    /// or before `frame`, or `None` if that key is empty, there is no key, or
    /// the layer does not exist.
    pub fn drawing_at(&self, layer: LayerId, frame: FrameIndex) -> Option<DrawingId> {
        self.track(layer)?.drawing_at(frame)
    }

    /// Every drawing that some track's keyframe refers to, including the
    /// tracks of every symbol in the library (symbols stay until the user
    /// deletes them, so their drawings must too).
    pub fn referenced_drawings(&self) -> BTreeSet<DrawingId> {
        self.tracks
            .values()
            .flat_map(|track| track.keys().filter_map(|(_, key)| key.drawing()))
            .chain(self.symbols.values().flat_map(Symbol::drawings))
            .collect()
    }

    /// Whether the pool holds drawings that no keyframe refers to.
    pub fn has_unreferenced_drawings(&self) -> bool {
        let referenced = self.referenced_drawings();
        self.drawings.keys().any(|id| !referenced.contains(id))
    }

    /// Drops every drawing that no track refers to and returns their IDs.
    ///
    /// Only call this on a copy that is about to be saved, or when the undo
    /// history is cleared: undo of a removed layer or keyframe needs the
    /// drawing to still be in the pool. `file_format` does this for you.
    pub fn garbage_collect_drawings(&mut self) -> Vec<DrawingId> {
        let referenced = self.referenced_drawings();
        let unused: Vec<DrawingId> = self
            .drawings
            .keys()
            .filter(|id| !referenced.contains(id))
            .copied()
            .collect();
        for id in &unused {
            self.drawings.remove(id);
        }
        unused
    }

    /// The asset library, keyed by ID (ADR-017).
    pub fn assets(&self) -> &BTreeMap<AssetId, Asset> {
        &self.assets
    }

    /// Looks up an asset.
    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.get(&id)
    }

    /// Mutable lookup of an asset.
    pub fn asset_mut(&mut self, id: AssetId) -> Option<&mut Asset> {
        self.assets.get_mut(&id)
    }

    /// Adds an asset under its own ID, replacing and returning any previous
    /// asset with that ID.
    pub fn insert_asset(&mut self, asset: Asset) -> Option<Asset> {
        self.assets.insert(asset.id, asset)
    }

    /// Removes an asset from the library. Items that still refer to it are
    /// left alone; check [`Self::asset_in_use`] first.
    pub fn remove_asset(&mut self, id: AssetId) -> Option<Asset> {
        self.assets.remove(&id)
    }

    /// Whether something the animation shows refers to asset `id`: an image
    /// in a drawing that some keyframe refers to (of the document or of a
    /// symbol in the library), or an audio layer's sound. Drawings and
    /// layers kept only for undo don't count.
    pub fn asset_in_use(&self, id: AssetId) -> bool {
        self.used_assets().contains(&id)
    }

    /// Every asset that something the animation shows refers to (see
    /// [`Self::asset_in_use`]).
    pub fn used_assets(&self) -> BTreeSet<AssetId> {
        self.referenced_drawings()
            .into_iter()
            .filter_map(|id| self.drawings.get(&id)?.as_vector())
            .flat_map(|v| v.images().iter().filter_map(|i| i.asset))
            .chain(self.audio_layers().map(|(_, clip)| clip.asset))
            .collect()
    }

    /// The symbols (objects) in the library, keyed by ID (ADR-018).
    pub fn symbols(&self) -> &BTreeMap<SymbolId, Symbol> {
        &self.symbols
    }

    /// Looks up a symbol.
    pub fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.get(&id)
    }

    /// Mutable lookup of a symbol.
    pub fn symbol_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
        self.symbols.get_mut(&id)
    }

    /// Adds a symbol under its own ID, replacing and returning any previous
    /// symbol with that ID.
    pub fn insert_symbol(&mut self, symbol: Symbol) -> Option<Symbol> {
        self.symbols.insert(symbol.id, symbol)
    }

    /// Removes a symbol. Instance keys that still place it are left alone
    /// (they show nothing); check [`Self::symbol_in_use`] first.
    pub fn remove_symbol(&mut self, id: SymbolId) -> Option<Symbol> {
        self.symbols.remove(&id)
    }

    /// Whether an instance key places symbol `id`, on a document track or
    /// inside another symbol.
    pub fn symbol_in_use(&self, id: SymbolId) -> bool {
        let placed = |track: &Track| track.keys().any(|(_, k)| k.symbol() == Some(id));
        self.tracks.values().any(placed)
            || self
                .symbols
                .values()
                .filter(|s| s.id != id)
                .any(|s| s.tracks.values().any(placed))
    }

    /// A name for a new symbol that no other symbol has: "Object 1",
    /// "Object 2", …
    pub fn next_symbol_name(&self) -> String {
        (1..)
            .map(|n| format!("Object {n}"))
            .find(|name| self.symbols.values().all(|s| &s.name != name))
            .unwrap_or_else(|| "Object".to_owned())
    }

    /// The last frame of the animation, if it has any frames.
    pub fn last_frame(&self) -> Option<FrameIndex> {
        self.frame_count.checked_sub(1).map(FrameIndex)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use timeline::Keyframe;

    fn doc() -> Document {
        Document::new(
            "Test",
            CanvasSize {
                width: 1920,
                height: 1080,
            },
        )
    }

    #[test]
    fn add_layer_creates_track() {
        let mut d = doc();
        let id = d.add_layer("Layer 1");
        assert_eq!(d.layers().len(), 1);
        assert!(d.track(id).is_some_and(Track::is_empty));
    }

    #[test]
    fn remove_and_restore_layer_roundtrips() {
        let mut d = doc();
        let a = d.add_layer("A");
        let b = d.add_layer("B");
        d.add_layer("C");
        let drawing = d.add_drawing(DrawingContent::default());
        d.track_mut(b)
            .unwrap()
            .set_key(FrameIndex(0), Keyframe::Drawing(drawing));
        let before = d.clone();

        let removed = d.remove_layer(b).unwrap();
        assert_eq!(removed.index, 1);
        assert!(d.track(b).is_none());
        assert!(d.layers().get(a).is_some());

        d.restore_layer(removed);
        assert_eq!(d, before);
    }

    #[test]
    fn drawing_at_follows_exposure() {
        let mut d = doc();
        let l = d.add_layer("L");
        let a = d.add_drawing(DrawingContent::default());
        assert_eq!(d.drawing_at(l, FrameIndex(0)), None);
        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(2), Keyframe::Drawing(a));
        assert_eq!(d.drawing_at(l, FrameIndex(1)), None);
        assert_eq!(d.drawing_at(l, FrameIndex(7)), Some(a));
        assert_eq!(d.drawing_at(LayerId::new(), FrameIndex(7)), None);
    }

    #[test]
    fn garbage_collect_keeps_only_referenced_drawings() {
        let mut d = doc();
        let l = d.add_layer("L");
        let used = d.add_drawing(DrawingContent::default());
        let unused = d.add_drawing(DrawingContent::default());
        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(0), Keyframe::Drawing(used));
        assert!(d.has_unreferenced_drawings());
        assert_eq!(d.garbage_collect_drawings(), [unused]);
        assert!(d.drawing(used).is_some());
        assert!(d.drawing(unused).is_none());
        assert!(!d.has_unreferenced_drawings());
        assert!(d.garbage_collect_drawings().is_empty());
    }

    #[test]
    fn insert_and_remove_drawing_with_known_id() {
        let mut d = doc();
        let id = DrawingId::new();
        assert!(d.insert_drawing(id, DrawingContent::default()).is_none());
        assert!(d.drawing(id).is_some());
        assert!(d.remove_drawing(id).is_some());
        assert!(d.drawing(id).is_none());
    }

    #[test]
    fn assets_insert_remove_and_usage() {
        use crate::assets::AssetKind;
        use drawing::{Image, ImageData, VectorDrawing};
        let mut d = doc();
        let l = d.add_layer("L");
        let data = ImageData::new(2, 2, vec![1, 2]);
        let asset = Asset::new("pic.png", AssetKind::Image(data.clone()));
        let id = asset.id;
        assert!(d.insert_asset(asset.clone()).is_none());
        assert_eq!(d.asset(id), Some(&asset));
        assert!(!d.asset_in_use(id));

        let mut v = VectorDrawing::new();
        v.push_image(Image::new(data, glam_zero(), glam_zero()).with_asset(id));
        let drawing = d.add_drawing(DrawingContent::Vector(v));
        // Only drawings a keyframe shows count.
        assert!(!d.asset_in_use(id));
        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(0), Keyframe::Drawing(drawing));
        assert!(d.asset_in_use(id));
        assert_eq!(d.used_assets().into_iter().collect::<Vec<_>>(), [id]);

        // An audio layer uses its sound.
        let sound = AssetId::new();
        let audio = d.add_layer("A");
        *d.layer_mut(audio).unwrap() = scene::Layer {
            id: audio,
            ..scene::Layer::audio("A", scene::AudioClip::new(sound, FrameIndex(0)))
        };
        assert!(d.asset_in_use(sound));
        d.remove_layer(audio);
        assert!(!d.asset_in_use(sound));

        d.asset_mut(id).unwrap().name = "renamed".into();
        assert_eq!(d.remove_asset(id).unwrap().name, "renamed");
        assert!(d.assets().is_empty());
    }

    fn glam_zero() -> animation_core::Vec2 {
        animation_core::Vec2::ZERO
    }

    #[test]
    fn symbols_keep_their_drawings_and_track_usage() {
        use crate::symbol::Symbol;
        use scene::Layer;
        use timeline::InstanceKey;
        let mut d = doc();
        let l = d.add_layer("L");
        let inner = d.add_drawing(DrawingContent::default());
        let mut symbol = Symbol::new(d.next_symbol_name(), 12);
        let mut track = Track::new();
        track.set_key(FrameIndex(0), Keyframe::Drawing(inner));
        symbol.push_layer(Layer::new("in"), track);
        let id = symbol.id;
        assert_eq!(symbol.name, "Object 1");
        d.insert_symbol(symbol);
        assert_eq!(d.next_symbol_name(), "Object 2");

        // A symbol's drawings survive garbage collection while it is in the
        // library, placed or not.
        assert!(!d.symbol_in_use(id));
        assert!(d.garbage_collect_drawings().is_empty());
        assert!(d.drawing(inner).is_some());

        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(3), Keyframe::Instance(InstanceKey::new(id)));
        assert!(d.symbol_in_use(id));
        assert_eq!(d.drawing_at(l, FrameIndex(3)), None);

        d.symbol_mut(id).unwrap().name = "Ball".into();
        assert_eq!(d.symbol(id).unwrap().name, "Ball");
        assert!(d.remove_symbol(id).is_some());
        assert!(d.symbols().is_empty());
        assert_eq!(d.garbage_collect_drawings(), [inner]);
    }

    #[test]
    fn last_frame_is_zero_based() {
        let mut d = doc();
        assert_eq!(d.last_frame(), Some(FrameIndex(23)));
        d.frame_count = 0;
        assert_eq!(d.last_frame(), None);
    }
}
