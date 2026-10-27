//! The [`Document`] type.

use animation_core::{DocumentId, DrawingId, Fps, FrameIndex, LayerId, Rgba};
use drawing::DrawingContent;
use scene::{Layer, LayerStack};
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
    layers: LayerStack,
    tracks: BTreeMap<LayerId, Track>,
    drawings: BTreeMap<DrawingId, DrawingContent>,
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
            layers: LayerStack::new(),
            tracks: BTreeMap::new(),
            drawings: BTreeMap::new(),
        }
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

    /// Every drawing that some track's keyframe refers to.
    pub fn referenced_drawings(&self) -> BTreeSet<DrawingId> {
        self.tracks
            .values()
            .flat_map(|track| track.keys().filter_map(|(_, key)| key.drawing()))
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
    fn last_frame_is_zero_based() {
        let mut d = doc();
        assert_eq!(d.last_frame(), Some(FrameIndex(23)));
        d.frame_count = 0;
        assert_eq!(d.last_frame(), None);
    }
}
