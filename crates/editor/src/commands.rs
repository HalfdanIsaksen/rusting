//! Concrete document commands.

mod fill;
pub use fill::{AddFill, RemoveFill};

use crate::{Command, CommandError};
use animation_core::{DrawingId, Fps, FrameIndex, LayerId, StrokeId};
use document::drawing::{DrawingContent, Stroke, VectorDrawing};
use document::timeline::Keyframe;
use document::{Document, RemovedLayer, scene::Layer};

/// Adds a new layer on top of the stack.
#[derive(Debug)]
pub struct AddLayer {
    layer: Layer,
}

impl AddLayer {
    /// Creates the command. The layer ID is fixed now so redo reuses it.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            layer: Layer::new(name),
        }
    }

    /// ID of the layer this command adds.
    pub fn layer_id(&self) -> LayerId {
        self.layer.id
    }
}

impl Command for AddLayer {
    fn label(&self) -> &str {
        "Add Layer"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let top = doc.layers().len();
        doc.insert_layer(top, self.layer.clone());
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        doc.remove_layer(self.layer.id)
            .map(|_| ())
            .ok_or(CommandError::LayerNotFound)
    }
}

/// Removes a layer (and its track).
#[derive(Debug)]
pub struct RemoveLayer {
    id: LayerId,
    removed: Option<RemovedLayer>,
}

impl RemoveLayer {
    /// Creates the command.
    pub fn new(id: LayerId) -> Self {
        Self { id, removed: None }
    }
}

impl Command for RemoveLayer {
    fn label(&self) -> &str {
        "Remove Layer"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.removed = Some(
            doc.remove_layer(self.id)
                .ok_or(CommandError::LayerNotFound)?,
        );
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let removed = self.removed.take().ok_or(CommandError::LayerNotFound)?;
        doc.restore_layer(removed);
        Ok(())
    }
}

/// Renames a layer.
#[derive(Debug)]
pub struct RenameLayer {
    id: LayerId,
    name: String,
}

impl RenameLayer {
    /// Creates the command.
    pub fn new(id: LayerId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }

    fn swap(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = doc.layer_mut(self.id).ok_or(CommandError::LayerNotFound)?;
        std::mem::swap(&mut layer.name, &mut self.name);
        Ok(())
    }
}

impl Command for RenameLayer {
    fn label(&self) -> &str {
        "Rename Layer"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }
}

/// Fails with [`CommandError::LayerLocked`] unless `layer` exists and is editable.
fn editable_layer(doc: &Document, layer: LayerId) -> Result<(), CommandError> {
    let layer = doc.layers().get(layer).ok_or(CommandError::LayerNotFound)?;
    if layer.is_editable() {
        Ok(())
    } else {
        Err(CommandError::LayerLocked)
    }
}

/// Fails unless `frame` lies inside the animation.
fn frame_in_range(doc: &Document, frame: FrameIndex) -> Result<(), CommandError> {
    if frame.0 < doc.frame_count {
        Ok(())
    } else {
        Err(CommandError::FrameOutOfRange {
            frame,
            frame_count: doc.frame_count,
        })
    }
}

fn vector_mut(doc: &mut Document, id: DrawingId) -> Result<&mut VectorDrawing, CommandError> {
    doc.drawing_mut(id)
        .and_then(DrawingContent::as_vector_mut)
        .ok_or(CommandError::DrawingNotFound)
}

/// What [`AddStroke::apply`] did, so undo can reverse exactly that.
#[derive(Debug, Clone, Copy, PartialEq)]
enum StrokeTarget {
    /// The stroke went into a drawing that was already exposed.
    Existing(DrawingId),
    /// A new drawing and keyframe were created. `replaced` is the keyframe that
    /// was at the frame before (an empty key), if any.
    Created { replaced: Option<Keyframe> },
}

/// Adds a stroke to the drawing a layer shows at a frame.
///
/// If the layer shows nothing at that frame, a new drawing and a drawing
/// keyframe at that frame are created; undo removes both.
#[derive(Debug)]
pub struct AddStroke {
    layer: LayerId,
    frame: FrameIndex,
    stroke: Stroke,
    new_drawing: DrawingId,
    target: Option<StrokeTarget>,
}

impl AddStroke {
    /// Creates the command. The stroke keeps its own ID; the ID of a drawing
    /// that may need creating is fixed now so redo reuses it.
    pub fn new(layer: LayerId, frame: FrameIndex, stroke: Stroke) -> Self {
        Self {
            layer,
            frame,
            stroke,
            new_drawing: DrawingId::new(),
            target: None,
        }
    }

    /// ID of the stroke this command adds.
    pub fn stroke_id(&self) -> StrokeId {
        self.stroke.id
    }

    /// ID of the drawing this command creates if the frame has no drawing.
    pub fn new_drawing_id(&self) -> DrawingId {
        self.new_drawing
    }
}

impl Command for AddStroke {
    fn label(&self) -> &str {
        "Add Stroke"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        frame_in_range(doc, self.frame)?;
        let target = match doc.drawing_at(self.layer, self.frame) {
            Some(id) => {
                vector_mut(doc, id)?.push_stroke(self.stroke.clone());
                StrokeTarget::Existing(id)
            }
            None => {
                let mut drawing = VectorDrawing::new();
                drawing.push_stroke(self.stroke.clone());
                let track = doc
                    .track_mut(self.layer)
                    .ok_or(CommandError::LayerNotFound)?;
                let replaced = track.set_key(self.frame, Keyframe::Drawing(self.new_drawing));
                doc.insert_drawing(self.new_drawing, DrawingContent::Vector(drawing));
                StrokeTarget::Created { replaced }
            }
        };
        self.target = Some(target);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        match self.target.take().ok_or(CommandError::DrawingNotFound)? {
            StrokeTarget::Existing(id) => {
                vector_mut(doc, id)?
                    .remove_stroke(self.stroke.id)
                    .ok_or(CommandError::DrawingNotFound)?;
            }
            StrokeTarget::Created { replaced } => {
                let track = doc
                    .track_mut(self.layer)
                    .ok_or(CommandError::LayerNotFound)?;
                match replaced {
                    Some(key) => track.set_key(self.frame, key),
                    None => track.remove_key(self.frame),
                };
                doc.remove_drawing(self.new_drawing)
                    .ok_or(CommandError::DrawingNotFound)?;
            }
        }
        Ok(())
    }
}

/// Removes a stroke from the drawing a layer shows at a frame.
#[derive(Debug)]
pub struct RemoveStroke {
    layer: LayerId,
    frame: FrameIndex,
    stroke: StrokeId,
    removed: Option<(DrawingId, usize, Stroke)>,
}

impl RemoveStroke {
    /// Creates the command.
    pub fn new(layer: LayerId, frame: FrameIndex, stroke: StrokeId) -> Self {
        Self {
            layer,
            frame,
            stroke,
            removed: None,
        }
    }
}

impl Command for RemoveStroke {
    fn label(&self) -> &str {
        "Remove Stroke"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        editable_layer(doc, self.layer)?;
        let drawing = doc
            .drawing_at(self.layer, self.frame)
            .ok_or(CommandError::DrawingNotFound)?;
        let (index, stroke) = vector_mut(doc, drawing)?
            .remove_stroke(self.stroke)
            .ok_or(CommandError::DrawingNotFound)?;
        self.removed = Some((drawing, index, stroke));
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let (drawing, index, stroke) = self.removed.take().ok_or(CommandError::DrawingNotFound)?;
        vector_mut(doc, drawing)?.insert_stroke(index, stroke);
        Ok(())
    }
}

/// Moves a layer to a new position in the stack (0 = bottom).
#[derive(Debug)]
pub struct MoveLayer {
    id: LayerId,
    to: usize,
    from: Option<usize>,
}

impl MoveLayer {
    /// Creates the command. `to` is clamped to the top of the stack.
    pub fn new(id: LayerId, to: usize) -> Self {
        Self { id, to, from: None }
    }
}

impl Command for MoveLayer {
    fn label(&self) -> &str {
        "Move Layer"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.from = Some(
            doc.move_layer(self.id, self.to)
                .ok_or(CommandError::LayerNotFound)?,
        );
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let from = self.from.take().ok_or(CommandError::LayerNotFound)?;
        doc.move_layer(self.id, from)
            .map(|_| ())
            .ok_or(CommandError::LayerNotFound)
    }
}

/// Shows or hides a layer.
#[derive(Debug)]
pub struct SetLayerVisible {
    id: LayerId,
    visible: bool,
}

impl SetLayerVisible {
    /// Creates the command.
    pub fn new(id: LayerId, visible: bool) -> Self {
        Self { id, visible }
    }

    fn swap(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = doc.layer_mut(self.id).ok_or(CommandError::LayerNotFound)?;
        std::mem::swap(&mut layer.visible, &mut self.visible);
        Ok(())
    }
}

impl Command for SetLayerVisible {
    fn label(&self) -> &str {
        "Set Layer Visibility"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }
}

/// Locks or unlocks a layer.
#[derive(Debug)]
pub struct SetLayerLocked {
    id: LayerId,
    locked: bool,
}

impl SetLayerLocked {
    /// Creates the command.
    pub fn new(id: LayerId, locked: bool) -> Self {
        Self { id, locked }
    }

    fn swap(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let layer = doc.layer_mut(self.id).ok_or(CommandError::LayerNotFound)?;
        std::mem::swap(&mut layer.locked, &mut self.locked);
        Ok(())
    }
}

impl Command for SetLayerLocked {
    fn label(&self) -> &str {
        "Set Layer Lock"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.swap(doc)
    }
}

/// The kind of keyframe [`AddKeyframe`] inserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeKind {
    /// A key showing a new, empty drawing.
    Drawing,
    /// A key showing nothing.
    Empty,
}

/// Inserts a keyframe on a layer's track.
///
/// Fails with [`CommandError::KeyframeExists`] if the frame already has a key.
#[derive(Debug)]
pub struct AddKeyframe {
    layer: LayerId,
    frame: FrameIndex,
    key: Keyframe,
}

impl AddKeyframe {
    /// Creates the command. For [`KeyframeKind::Drawing`] the new drawing's ID
    /// is fixed now so redo reuses it.
    pub fn new(layer: LayerId, frame: FrameIndex, kind: KeyframeKind) -> Self {
        let key = match kind {
            KeyframeKind::Drawing => Keyframe::Drawing(DrawingId::new()),
            KeyframeKind::Empty => Keyframe::Empty,
        };
        Self { layer, frame, key }
    }

    /// The keyframe this command inserts.
    pub fn keyframe(&self) -> Keyframe {
        self.key
    }
}

impl Command for AddKeyframe {
    fn label(&self) -> &str {
        match self.key {
            Keyframe::Drawing(_) => "Add Keyframe",
            Keyframe::Empty => "Add Empty Keyframe",
        }
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        frame_in_range(doc, self.frame)?;
        let track = doc
            .track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?;
        if track.key_at(self.frame).is_some() {
            return Err(CommandError::KeyframeExists(self.frame));
        }
        track.set_key(self.frame, self.key);
        if let Keyframe::Drawing(id) = self.key {
            doc.insert_drawing(id, DrawingContent::default());
        }
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        doc.track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?
            .remove_key(self.frame)
            .ok_or(CommandError::KeyframeNotFound(self.frame))?;
        if let Keyframe::Drawing(id) = self.key {
            doc.remove_drawing(id)
                .ok_or(CommandError::DrawingNotFound)?;
        }
        Ok(())
    }
}

/// Removes the keyframe at a frame. The drawing it showed stays in the
/// document's pool so undo can restore it; it is dropped on save if unused.
#[derive(Debug)]
pub struct RemoveKeyframe {
    layer: LayerId,
    frame: FrameIndex,
    removed: Option<Keyframe>,
}

impl RemoveKeyframe {
    /// Creates the command.
    pub fn new(layer: LayerId, frame: FrameIndex) -> Self {
        Self {
            layer,
            frame,
            removed: None,
        }
    }
}

impl Command for RemoveKeyframe {
    fn label(&self) -> &str {
        "Remove Keyframe"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let key = doc
            .track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?
            .remove_key(self.frame)
            .ok_or(CommandError::KeyframeNotFound(self.frame))?;
        self.removed = Some(key);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let key = self
            .removed
            .take()
            .ok_or(CommandError::KeyframeNotFound(self.frame))?;
        doc.track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?
            .set_key(self.frame, key);
        Ok(())
    }
}

/// Moves the keyframe at one frame to another on the same layer.
///
/// Fails with [`CommandError::KeyframeExists`] if the target frame already
/// has a key, so a move never silently replaces a drawing.
#[derive(Debug)]
pub struct MoveKeyframe {
    layer: LayerId,
    from: FrameIndex,
    to: FrameIndex,
}

impl MoveKeyframe {
    /// Creates the command.
    pub fn new(layer: LayerId, from: FrameIndex, to: FrameIndex) -> Self {
        Self { layer, from, to }
    }
}

impl Command for MoveKeyframe {
    fn label(&self) -> &str {
        "Move Keyframe"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        frame_in_range(doc, self.to)?;
        let track = doc
            .track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?;
        if track.key_at(self.to).is_some() {
            return Err(CommandError::KeyframeExists(self.to));
        }
        track
            .move_key(self.from, self.to)
            .map_err(|_| CommandError::KeyframeNotFound(self.from))?;
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        doc.track_mut(self.layer)
            .ok_or(CommandError::LayerNotFound)?
            .move_key(self.to, self.from)
            .map_err(|_| CommandError::KeyframeNotFound(self.to))?;
        Ok(())
    }
}

/// Changes the playback frame rate.
#[derive(Debug)]
pub struct SetFps {
    fps: Fps,
}

impl SetFps {
    /// Creates the command.
    pub fn new(fps: Fps) -> Self {
        Self { fps }
    }
}

impl Command for SetFps {
    fn label(&self) -> &str {
        "Set Frame Rate"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        std::mem::swap(&mut doc.fps, &mut self.fps);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        self.apply(doc)
    }
}

/// Changes the animation length.
///
/// Shortening does not delete keyframes past the new end: they are kept (and
/// saved) so that lengthening again, or undo, brings them back.
#[derive(Debug)]
pub struct SetFrameCount {
    frame_count: u32,
}

impl SetFrameCount {
    /// Creates the command. Applying it fails for a count of zero.
    pub fn new(frame_count: u32) -> Self {
        Self { frame_count }
    }
}

impl Command for SetFrameCount {
    fn label(&self) -> &str {
        "Set Frame Count"
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        if self.frame_count == 0 {
            return Err(CommandError::ZeroFrameCount);
        }
        std::mem::swap(&mut doc.frame_count, &mut self.frame_count);
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        std::mem::swap(&mut doc.frame_count, &mut self.frame_count);
        Ok(())
    }
}

/// Several commands run as one undo step: applied in order, undone in
/// reverse. If one fails, the ones before it are rolled back, so the document
/// is left unchanged.
#[derive(Debug)]
pub struct Batch {
    label: String,
    commands: Vec<Box<dyn Command>>,
}

impl Batch {
    /// Groups `commands` under one undo label.
    pub fn new(label: impl Into<String>, commands: Vec<Box<dyn Command>>) -> Self {
        Self {
            label: label.into(),
            commands,
        }
    }

    /// Whether the batch holds no commands.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Number of grouped commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }
}

impl Command for Batch {
    fn label(&self) -> &str {
        &self.label
    }

    fn apply(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        for i in 0..self.commands.len() {
            if let Err(err) = self.commands[i].apply(doc) {
                for done in self.commands[..i].iter_mut().rev() {
                    // Best effort: these just applied, so undo should succeed.
                    let _ = done.undo(doc);
                }
                return Err(err);
            }
        }
        Ok(())
    }

    fn undo(&mut self, doc: &mut Document) -> Result<(), CommandError> {
        let count = self.commands.len();
        for i in (0..count).rev() {
            if let Err(err) = self.commands[i].undo(doc) {
                for done in &mut self.commands[i + 1..] {
                    let _ = done.apply(doc);
                }
                return Err(err);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::History;
    use document::CanvasSize;

    fn doc() -> Document {
        Document::new(
            "T",
            CanvasSize {
                width: 10,
                height: 10,
            },
        )
    }

    #[test]
    fn add_layer_undo_redo_keeps_id() {
        let mut d = doc();
        let mut h = History::new();
        let cmd = AddLayer::new("L1");
        let id = cmd.layer_id();
        h.execute(&mut d, Box::new(cmd)).unwrap();
        assert!(d.layers().get(id).is_some());
        assert!(h.undo(&mut d).unwrap());
        assert!(d.layers().is_empty());
        assert!(h.redo(&mut d).unwrap());
        assert_eq!(d.layers().get(id).map(|l| l.name.as_str()), Some("L1"));
    }

    #[test]
    fn remove_layer_undo_restores_exact_document() {
        let mut d = doc();
        let a = d.add_layer("A");
        d.add_layer("B");
        let before = d.clone();
        let mut h = History::new();
        h.execute(&mut d, Box::new(RemoveLayer::new(a))).unwrap();
        assert_ne!(d, before);
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
    }

    #[test]
    fn rename_undo_redo() {
        let mut d = doc();
        let id = d.add_layer("Old");
        let mut h = History::new();
        h.execute(&mut d, Box::new(RenameLayer::new(id, "New")))
            .unwrap();
        assert_eq!(d.layers().get(id).unwrap().name, "New");
        h.undo(&mut d).unwrap();
        assert_eq!(d.layers().get(id).unwrap().name, "Old");
        h.redo(&mut d).unwrap();
        assert_eq!(d.layers().get(id).unwrap().name, "New");
    }

    #[test]
    fn failed_command_is_not_recorded_and_new_edit_clears_redo() {
        let mut d = doc();
        let mut h = History::new();
        let missing = RemoveLayer::new(LayerId::new());
        assert_eq!(
            h.execute(&mut d, Box::new(missing)),
            Err(CommandError::LayerNotFound)
        );
        assert!(!h.can_undo());

        h.execute(&mut d, Box::new(AddLayer::new("A"))).unwrap();
        h.undo(&mut d).unwrap();
        assert!(h.can_redo());
        h.execute(&mut d, Box::new(AddLayer::new("B"))).unwrap();
        assert!(!h.can_redo());
        assert_eq!(h.undo_label(), Some("Add Layer"));
    }

    fn stroke() -> Stroke {
        use animation_core::Vec2;
        use document::drawing::{BrushStyle, StrokePoint};
        Stroke::new(
            vec![StrokePoint::new(Vec2::ZERO), StrokePoint::new(Vec2::ONE)],
            BrushStyle::default(),
        )
    }

    #[test]
    fn add_stroke_creates_drawing_and_key_then_reuses_it() {
        let mut d = doc();
        let l = d.add_layer("L");
        let before = d.clone();
        let mut h = History::new();
        let first = AddStroke::new(l, FrameIndex(2), stroke());
        let drawing = first.new_drawing_id();
        h.execute(&mut d, Box::new(first)).unwrap();
        assert_eq!(d.drawing_at(l, FrameIndex(2)), Some(drawing));
        h.execute(&mut d, Box::new(AddStroke::new(l, FrameIndex(5), stroke())))
            .unwrap();
        let strokes = d.drawing(drawing).unwrap().as_vector().unwrap().strokes();
        assert_eq!(strokes.len(), 2);
        assert_eq!(d.track(l).unwrap().len(), 1);

        h.undo(&mut d).unwrap();
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
        h.redo(&mut d).unwrap();
        assert_eq!(d.drawing_at(l, FrameIndex(2)), Some(drawing));
    }

    #[test]
    fn add_stroke_over_empty_key_restores_it_on_undo() {
        let mut d = doc();
        let l = d.add_layer("L");
        d.track_mut(l)
            .unwrap()
            .set_key(FrameIndex(1), Keyframe::Empty);
        let before = d.clone();
        let mut h = History::new();
        h.execute(&mut d, Box::new(AddStroke::new(l, FrameIndex(1), stroke())))
            .unwrap();
        assert!(d.drawing_at(l, FrameIndex(1)).is_some());
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
    }

    #[test]
    fn stroke_commands_reject_locked_layers() {
        let mut d = doc();
        let l = d.add_layer("L");
        let mut h = History::new();
        let add = AddStroke::new(l, FrameIndex(0), stroke());
        let sid = add.stroke_id();
        h.execute(&mut d, Box::new(add)).unwrap();
        h.execute(&mut d, Box::new(SetLayerLocked::new(l, true)))
            .unwrap();
        let before = d.clone();
        assert_eq!(
            h.execute(&mut d, Box::new(AddStroke::new(l, FrameIndex(0), stroke()))),
            Err(CommandError::LayerLocked)
        );
        assert_eq!(
            h.execute(&mut d, Box::new(RemoveStroke::new(l, FrameIndex(0), sid))),
            Err(CommandError::LayerLocked)
        );
        assert_eq!(d, before);
        h.undo(&mut d).unwrap();
        h.execute(&mut d, Box::new(RemoveStroke::new(l, FrameIndex(0), sid)))
            .unwrap();
    }

    #[test]
    fn keyframe_commands_check_frames() {
        let mut d = doc();
        let l = d.add_layer("L");
        let mut h = History::new();
        h.execute(
            &mut d,
            Box::new(AddKeyframe::new(l, FrameIndex(3), KeyframeKind::Empty)),
        )
        .unwrap();
        assert_eq!(
            h.execute(
                &mut d,
                Box::new(AddKeyframe::new(l, FrameIndex(3), KeyframeKind::Drawing))
            ),
            Err(CommandError::KeyframeExists(FrameIndex(3)))
        );
        assert_eq!(
            h.execute(
                &mut d,
                Box::new(AddKeyframe::new(l, FrameIndex(24), KeyframeKind::Empty))
            ),
            Err(CommandError::FrameOutOfRange {
                frame: FrameIndex(24),
                frame_count: 24
            })
        );
        assert_eq!(
            h.execute(&mut d, Box::new(RemoveKeyframe::new(l, FrameIndex(4)))),
            Err(CommandError::KeyframeNotFound(FrameIndex(4)))
        );
        assert_eq!(
            h.execute(&mut d, Box::new(SetFrameCount::new(0))),
            Err(CommandError::ZeroFrameCount)
        );
    }

    #[test]
    fn remove_keyframe_keeps_drawing_for_undo() {
        let mut d = doc();
        let l = d.add_layer("L");
        let mut h = History::new();
        let add = AddKeyframe::new(l, FrameIndex(0), KeyframeKind::Drawing);
        let drawing = add.keyframe().drawing().unwrap();
        h.execute(&mut d, Box::new(add)).unwrap();
        let before = d.clone();
        h.execute(&mut d, Box::new(RemoveKeyframe::new(l, FrameIndex(0))))
            .unwrap();
        assert!(d.drawing(drawing).is_some());
        assert_eq!(d.drawing_at(l, FrameIndex(0)), None);
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);
    }

    #[test]
    fn move_keyframe_undo_and_rejects_occupied_target() {
        let mut d = doc();
        d.frame_count = 10;
        let l = d.add_layer("L");
        let mut h = History::new();
        let add = AddKeyframe::new(l, FrameIndex(2), KeyframeKind::Drawing);
        let drawing = add.keyframe().drawing().unwrap();
        h.execute(&mut d, Box::new(add)).unwrap();
        h.execute(
            &mut d,
            Box::new(AddKeyframe::new(l, FrameIndex(6), KeyframeKind::Empty)),
        )
        .unwrap();
        let before = d.clone();

        h.execute(
            &mut d,
            Box::new(MoveKeyframe::new(l, FrameIndex(2), FrameIndex(4))),
        )
        .unwrap();
        assert_eq!(d.drawing_at(l, FrameIndex(4)), Some(drawing));
        assert!(d.track(l).unwrap().key_at(FrameIndex(2)).is_none());
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);

        assert_eq!(
            h.execute(
                &mut d,
                Box::new(MoveKeyframe::new(l, FrameIndex(2), FrameIndex(6)))
            ),
            Err(CommandError::KeyframeExists(FrameIndex(6)))
        );
        assert!(matches!(
            h.execute(
                &mut d,
                Box::new(MoveKeyframe::new(l, FrameIndex(2), FrameIndex(10)))
            ),
            Err(CommandError::FrameOutOfRange { .. })
        ));
        assert_eq!(d, before);
    }

    #[test]
    fn batch_is_one_step_and_rolls_back_on_failure() {
        let mut d = doc();
        let l = d.add_layer("L");
        let a = Stroke::new(
            vec![
                document::drawing::StrokePoint::new(animation_core::Vec2::ZERO),
                document::drawing::StrokePoint::new(animation_core::Vec2::ONE),
            ],
            document::drawing::BrushStyle::default(),
        );
        let b = Stroke::new(a.points.clone(), a.style);
        let (ia, ib) = (a.id, b.id);
        let mut h = History::new();
        h.execute(&mut d, Box::new(AddStroke::new(l, FrameIndex(0), a)))
            .unwrap();
        h.execute(&mut d, Box::new(AddStroke::new(l, FrameIndex(0), b)))
            .unwrap();
        let before = d.clone();

        let erase = Batch::new(
            "Erase",
            vec![
                Box::new(RemoveStroke::new(l, FrameIndex(0), ia)),
                Box::new(RemoveStroke::new(l, FrameIndex(0), ib)),
            ],
        );
        assert_eq!(erase.len(), 2);
        h.execute(&mut d, Box::new(erase)).unwrap();
        assert_eq!(h.undo_label(), Some("Erase"));
        let drawing = d.drawing_at(l, FrameIndex(0)).unwrap();
        assert!(d.drawing(drawing).unwrap().as_vector().unwrap().is_empty());
        h.undo(&mut d).unwrap();
        assert_eq!(d, before);

        // The second removal fails, so the first is rolled back.
        let failing = Batch::new(
            "Erase",
            vec![
                Box::new(RemoveStroke::new(l, FrameIndex(0), ia)),
                Box::new(RemoveStroke::new(l, FrameIndex(0), StrokeId::new())),
            ],
        );
        assert!(h.execute(&mut d, Box::new(failing)).is_err());
        assert_eq!(d, before);
        assert!(Batch::new("Nothing", Vec::new()).is_empty());
    }
}
