//! Brush and eraser gestures on the canvas.
//!
//! The host converts pointer positions to document coordinates (with
//! `renderer::Viewport::screen_to_canvas`) and calls
//! [`Session::pointer_down`], [`Session::pointer_move`] and
//! [`Session::pointer_up`]. A brush gesture builds a stroke and commits it as
//! one `AddStroke`. An eraser gesture either cuts away the part of each
//! stroke it passes over ([`EraserMode::Partial`], with a live preview of the
//! pieces) or collects whole strokes ([`EraserMode::Stroke`]); either way the
//! result is committed as one undo step on release. The document is not
//! touched until the gesture ends.
//!
//! The select tool keeps a selection of strokes and fills on the active
//! layer's drawing at the current frame. A press on an item selects it and
//! drags the selection (committed as one `TranslateItems` on release); a
//! press on empty space drags a marquee. See [`Session::sync_selection`] for
//! when the selection is cleared.

use crate::session::Session;
use document::animation_core::{FrameIndex, LayerId, StrokeId, Vec2};
use document::drawing::select::rect_from_corners;
use document::drawing::{
    BrushStyle, Fill, GapClosing, ItemId, Stroke, StrokeBuilder, StrokePoint, VectorDrawing,
};
use editor::Tool;
use editor::commands::{
    AddFill, AddStroke, Batch, RemoveFill, RemoveStroke, ReplaceStroke, TranslateItems,
    remove_items,
};

/// Smallest distance between recorded brush points, in screen points.
const MIN_POINT_SPACING_SCREEN: f32 = 0.75;

/// How far outside a stroke a click still selects it, in screen points.
const SELECT_TOLERANCE_SCREEN: f32 = 4.0;

/// How far the pointer must travel before a select press counts as a drag,
/// in screen points.
const DRAG_THRESHOLD_SCREEN: f32 = 3.0;

/// A gesture in progress.
#[derive(Clone, Debug)]
enum Gesture {
    Brush {
        layer: LayerId,
        frame: FrameIndex,
        builder: StrokeBuilder,
        /// Stroke id used for the live preview mesh.
        preview_id: StrokeId,
    },
    Erase {
        layer: LayerId,
        frame: FrameIndex,
        mode: EraserMode,
        last: Vec2,
        /// Touched strokes, in drawing order of first touch.
        hits: Vec<StrokeId>,
        /// Partial mode: what each touched stroke is cut down to so far,
        /// parallel to `hits`.
        pieces: Vec<Vec<Stroke>>,
    },
    /// Dragging the selection.
    Move {
        layer: LayerId,
        frame: FrameIndex,
        start: Vec2,
        delta: Vec2,
        zoom: f32,
        dragged: bool,
    },
    /// Dragging a selection box.
    Marquee {
        layer: LayerId,
        frame: FrameIndex,
        start: Vec2,
        end: Vec2,
        zoom: f32,
        dragged: bool,
        /// The selection to add to (Shift), or empty.
        base: Vec<ItemId>,
    },
}

/// Whether the pointer moved from `start` to `pos` far enough to be a drag.
fn past_drag_threshold(start: Vec2, pos: Vec2, zoom: f32) -> bool {
    (pos - start).length() * zoom > DRAG_THRESHOLD_SCREEN
}

/// How the eraser treats a stroke it touches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EraserMode {
    /// Cut away only the part the eraser passes over.
    #[default]
    Partial,
    /// Remove every stroke the eraser touches.
    Stroke,
}

/// Tool settings and the gesture in progress.
#[derive(Clone, Debug)]
pub struct ToolController {
    /// Eraser radius in document units.
    pub eraser_radius: f32,
    /// Partial or whole-stroke erasing.
    pub eraser_mode: EraserMode,
    /// How hard the fill bucket closes gaps in the line art.
    pub fill_gap: GapClosing,
    gesture: Option<Gesture>,
    /// Selected items, bottom to top as selected.
    selection: Vec<ItemId>,
    /// The layer and frame the selection belongs to.
    selection_at: Option<(LayerId, FrameIndex)>,
}

impl Default for ToolController {
    fn default() -> Self {
        Self {
            eraser_radius: 12.0,
            eraser_mode: EraserMode::default(),
            fill_gap: GapClosing::default(),
            gesture: None,
            selection: Vec::new(),
            selection_at: None,
        }
    }
}

impl ToolController {
    /// Drops the gesture in progress without changing the document.
    pub fn cancel(&mut self) {
        self.gesture = None;
    }

    /// Whether a gesture is in progress.
    pub fn is_active(&self) -> bool {
        self.gesture.is_some()
    }

    /// The selected strokes and fills.
    pub fn selection(&self) -> &[ItemId] {
        &self.selection
    }

    /// The layer and frame the selection belongs to, if anything is selected.
    pub fn selection_at(&self) -> Option<(LayerId, FrameIndex)> {
        self.selection_at.filter(|_| !self.selection.is_empty())
    }

    /// How far the selection is being dragged (document units); draw the
    /// selected items moved by this. Zero when not dragging.
    pub fn move_offset(&self) -> Vec2 {
        match &self.gesture {
            Some(Gesture::Move {
                delta,
                dragged: true,
                ..
            }) => *delta,
            _ => Vec2::ZERO,
        }
    }

    /// The selection box being dragged, as `(min, max)` in document
    /// coordinates.
    pub fn marquee(&self) -> Option<(Vec2, Vec2)> {
        match &self.gesture {
            Some(Gesture::Marquee {
                start,
                end,
                dragged: true,
                ..
            }) => Some(rect_from_corners(*start, *end)),
            _ => None,
        }
    }

    fn set_selection(&mut self, at: (LayerId, FrameIndex), items: Vec<ItemId>) {
        self.selection_at = Some(at);
        self.selection = items;
    }

    fn clear_selection(&mut self) {
        self.selection.clear();
        self.selection_at = None;
        if matches!(
            self.gesture,
            Some(Gesture::Move { .. } | Gesture::Marquee { .. })
        ) {
            self.gesture = None;
        }
    }

    /// The stroke being drawn, for the live preview, and the layer it goes on.
    pub fn preview(&self) -> Option<(LayerId, Stroke)> {
        match &self.gesture {
            Some(Gesture::Brush {
                layer,
                builder,
                preview_id,
                ..
            }) if !builder.is_empty() => Some((
                *layer,
                Stroke {
                    id: *preview_id,
                    points: builder.points().to_vec(),
                    style: *builder.style(),
                },
            )),
            _ => None,
        }
    }

    /// Strokes the eraser gesture will remove or cut; hide them while
    /// erasing and draw [`ToolController::erase_preview`] in their place.
    pub fn erasing(&self) -> &[StrokeId] {
        match &self.gesture {
            Some(Gesture::Erase { hits, .. }) => hits,
            _ => &[],
        }
    }

    /// The pieces that will replace stroke `id` when the partial eraser
    /// gesture ends, bottom to top. Empty if the stroke is erased entirely or
    /// is not being erased.
    pub fn erase_preview(&self, id: StrokeId) -> &[Stroke] {
        match &self.gesture {
            Some(Gesture::Erase { hits, pieces, .. }) => hits
                .iter()
                .position(|h| *h == id)
                .and_then(|i| pieces.get(i))
                .map_or(&[], Vec::as_slice),
            _ => &[],
        }
    }
}

impl Session {
    /// Why the active tool can't draw right now, or `None` if it can.
    pub fn draw_block_reason(&self) -> Option<String> {
        let Some(layer) = self
            .state
            .active_layer
            .and_then(|id| self.document.layers().get(id))
        else {
            return Some("No layer selected — add a layer first".into());
        };
        if layer.locked {
            Some(format!("Layer “{}” is locked", layer.name))
        } else if !layer.visible {
            Some(format!("Layer “{}” is hidden", layer.name))
        } else {
            None
        }
    }

    /// Starts a gesture at `pos` (document coordinates). `zoom` is screen
    /// points per document unit. Returns false, with a status message, if the
    /// active layer can't be edited.
    pub fn pointer_down(&mut self, pos: Vec2, zoom: f32) -> bool {
        self.pointer_down_with(pos, zoom, false)
    }

    /// Like [`Session::pointer_down`], with Shift held or not. Shift makes
    /// the select tool add to or remove from the selection.
    pub fn pointer_down_with(&mut self, pos: Vec2, zoom: f32, shift: bool) -> bool {
        self.tools.cancel();
        if let Some(reason) = self.draw_block_reason() {
            self.set_error(reason);
            return false;
        }
        let Some(layer) = self.state.active_layer else {
            return false;
        };
        if self.playback.is_playing() {
            self.playback.pause();
        }
        let frame = self.current_frame();
        self.tools.gesture = Some(match self.state.tool {
            // A click, not a drag: fill right away, no gesture.
            Tool::Fill => {
                self.fill_at(layer, frame, pos);
                return true;
            }
            Tool::Select => match self.select_press(layer, frame, pos, zoom, shift) {
                Some(gesture) => gesture,
                None => return true,
            },
            Tool::Brush => {
                let style = BrushStyle {
                    width: self.state.brush.width.max(0.1),
                    ..self.state.brush
                };
                let min_distance = MIN_POINT_SPACING_SCREEN / zoom.max(f32::EPSILON);
                let mut builder = StrokeBuilder::begin(style).with_min_distance(min_distance);
                builder.add_point(StrokePoint::new(pos));
                Gesture::Brush {
                    layer,
                    frame,
                    builder,
                    preview_id: StrokeId::new(),
                }
            }
            Tool::Eraser => Gesture::Erase {
                layer,
                frame,
                mode: self.tools.eraser_mode,
                last: pos,
                hits: Vec::new(),
                pieces: Vec::new(),
            },
        });
        self.pointer_move(pos);
        true
    }

    /// Continues the gesture to `pos` (document coordinates).
    pub fn pointer_move(&mut self, pos: Vec2) {
        let radius = self.tools.eraser_radius;
        match &mut self.tools.gesture {
            Some(Gesture::Move {
                start,
                delta,
                zoom,
                dragged,
                ..
            }) => {
                *dragged |= past_drag_threshold(*start, pos, *zoom);
                *delta = pos - *start;
            }
            Some(Gesture::Marquee {
                layer,
                frame,
                start,
                end,
                zoom,
                dragged,
                base,
            }) => {
                *end = pos;
                *dragged |= past_drag_threshold(*start, pos, *zoom);
                if *dragged {
                    let mut items = base.clone();
                    if let Some(drawing) = vector_at(&self.document, *layer, *frame) {
                        for item in drawing.items_in_rect(rect_from_corners(*start, *end)) {
                            if !items.contains(&item) {
                                items.push(item);
                            }
                        }
                    }
                    self.tools.selection = items;
                    self.tools.selection_at = Some((*layer, *frame));
                }
            }
            Some(Gesture::Brush { builder, .. }) => {
                builder.add_point(StrokePoint::new(pos));
            }
            Some(Gesture::Erase {
                layer,
                frame,
                mode,
                last,
                hits,
                pieces,
            }) => {
                let Some(drawing) = self
                    .document
                    .drawing_at(*layer, *frame)
                    .and_then(|id| self.document.drawing(id))
                    .and_then(|d| d.as_vector())
                else {
                    *last = pos;
                    return;
                };
                if *mode == EraserMode::Partial {
                    // The swept segment is one capsule, so fast moves can't
                    // skip anything.
                    let path = [*last, pos];
                    for stroke in drawing.strokes() {
                        match hits.iter().position(|h| *h == stroke.id) {
                            Some(i) => {
                                let cut: Vec<Stroke> = pieces[i]
                                    .iter()
                                    .flat_map(|piece| {
                                        piece
                                            .erase(&path, radius)
                                            .into_replacements()
                                            .unwrap_or_else(|| vec![piece.clone()])
                                    })
                                    .collect();
                                pieces[i] = cut;
                            }
                            None => {
                                if let Some(cut) = stroke.erase(&path, radius).into_replacements() {
                                    hits.push(stroke.id);
                                    pieces.push(cut);
                                }
                            }
                        }
                    }
                    *last = pos;
                    return;
                }
                // Sample the segment so fast moves don't skip strokes.
                let step = (radius * 0.5).max(0.5);
                let samples = ((pos - *last).length() / step).ceil().clamp(1.0, 1000.0) as u32;
                for i in 0..=samples {
                    let point = last.lerp(pos, i as f32 / samples as f32);
                    for id in drawing.strokes_hit(point, radius) {
                        if !hits.contains(&id) {
                            hits.push(id);
                        }
                    }
                }
                *last = pos;
            }
            None => {}
        }
    }

    /// Fill bucket click at `pos` (document coordinates) on `layer` at
    /// `frame`, with the brush colour and opacity. Clicking an existing fill
    /// recolours it (one undo step). Explains in the status bar when nothing
    /// can be filled.
    pub fn fill_at(&mut self, layer: LayerId, frame: FrameIndex, pos: Vec2) {
        let doc = &self.document;
        let canvas = Vec2::new(doc.canvas.width as f32, doc.canvas.height as f32);
        let drawing = doc
            .drawing_at(layer, frame)
            .and_then(|id| doc.drawing(id))
            .and_then(|d| d.as_vector());
        let color = self.state.brush.color;
        let opacity = self.state.brush.opacity;
        if let Some(existing) = drawing
            .and_then(|d| d.fill_at(pos))
            .and_then(|id| drawing.and_then(|d| d.fill(id)).cloned())
        {
            if existing.color == color && existing.opacity == opacity {
                return;
            }
            let recoloured = Fill::new(existing.shape(), color, opacity);
            let commands: Vec<Box<dyn editor::Command>> = vec![
                Box::new(RemoveFill::new(layer, frame, existing.id)),
                Box::new(AddFill::new(layer, frame, recoloured)),
            ];
            let _ = self.execute(Box::new(Batch::new("Recolour Fill", commands)));
            return;
        }
        let empty = Default::default();
        let drawing = drawing.unwrap_or(&empty);
        match document::drawing::fill_region(drawing, pos, self.tools.fill_gap, canvas) {
            Ok(shape) => {
                let fill = Fill::new(shape, color, opacity);
                let _ = self.execute(Box::new(AddFill::new(layer, frame, fill)));
            }
            Err(err) => {
                let mut text = err.to_string();
                if let Some(first) = text.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                self.set_error(text);
            }
        }
    }

    /// Select tool press: picks or toggles the item under `pos`, and returns
    /// the drag gesture to start, if any.
    fn select_press(
        &mut self,
        layer: LayerId,
        frame: FrameIndex,
        pos: Vec2,
        zoom: f32,
        shift: bool,
    ) -> Option<Gesture> {
        self.sync_selection();
        let at = (layer, frame);
        let zoom = zoom.max(f32::EPSILON);
        match self.item_under(pos, zoom) {
            Some(item) if shift => {
                let mut items = self.tools.selection.clone();
                match items.iter().position(|i| *i == item) {
                    Some(index) => {
                        items.remove(index);
                    }
                    None => items.push(item),
                }
                self.tools.set_selection(at, items);
                None
            }
            Some(item) => {
                if !self.tools.selection.contains(&item) {
                    self.tools.set_selection(at, vec![item]);
                }
                Some(Gesture::Move {
                    layer,
                    frame,
                    start: pos,
                    delta: Vec2::ZERO,
                    zoom,
                    dragged: false,
                })
            }
            None => {
                let base = if shift {
                    self.tools.selection.clone()
                } else {
                    self.tools.clear_selection();
                    Vec::new()
                };
                Some(Gesture::Marquee {
                    layer,
                    frame,
                    start: pos,
                    end: pos,
                    zoom,
                    dragged: false,
                    base,
                })
            }
        }
    }

    /// The drawing the active layer shows at the current frame.
    pub fn active_drawing(&self) -> Option<&VectorDrawing> {
        vector_at(
            &self.document,
            self.state.active_layer?,
            self.current_frame(),
        )
    }

    /// The topmost item of the active drawing under `pos` (document
    /// coordinates), with a few screen points of slack for thin strokes.
    /// `zoom` is screen points per document unit.
    pub fn item_under(&self, pos: Vec2, zoom: f32) -> Option<ItemId> {
        let tolerance = SELECT_TOLERANCE_SCREEN / zoom.max(f32::EPSILON);
        self.active_drawing()?.item_at(pos, tolerance)
    }

    /// Selects every stroke and fill of the active layer's drawing at the
    /// current frame. Explains in the status bar if the layer can't be
    /// edited.
    pub fn select_all(&mut self) {
        if let Some(reason) = self.draw_block_reason() {
            self.set_error(reason);
            return;
        }
        let Some(layer) = self.state.active_layer else {
            return;
        };
        let items = self
            .active_drawing()
            .map(|d| d.all_items())
            .unwrap_or_default();
        let frame = self.current_frame();
        self.tools.set_selection((layer, frame), items);
    }

    /// Deselects everything.
    pub fn clear_selection(&mut self) {
        self.tools.clear_selection();
    }

    /// Moves the selection by `delta` (document units) as one undo step.
    pub fn nudge_selection(&mut self, delta: Vec2) {
        self.sync_selection();
        let Some((layer, frame)) = self.tools.selection_at() else {
            return;
        };
        if self.tools.is_active() || delta == Vec2::ZERO {
            return;
        }
        let items = self.tools.selection.clone();
        let _ = self.execute(Box::new(TranslateItems::new(layer, frame, items, delta)));
    }

    /// Deletes the selected items as one undo step.
    pub fn delete_selection(&mut self) {
        self.sync_selection();
        let Some((layer, frame)) = self.tools.selection_at() else {
            return;
        };
        if self.tools.is_active() {
            return;
        }
        let command = remove_items(layer, frame, &self.tools.selection);
        if self.execute(Box::new(command)).is_ok() {
            self.tools.clear_selection();
        }
    }

    /// Keeps the selection valid: clears it when the select tool is not
    /// active, when the active layer or current frame differs from where it
    /// was made, or when the layer is locked or hidden; and drops items that
    /// no longer exist (e.g. after undo). Called after every document,
    /// frame and layer change, and once per UI frame.
    pub fn sync_selection(&mut self) {
        let Some((layer, frame)) = self.tools.selection_at else {
            return;
        };
        let editable = self
            .document
            .layers()
            .get(layer)
            .is_some_and(|l| l.is_editable());
        let same_place = self.state.active_layer == Some(layer) && self.current_frame() == frame;
        if self.state.tool != Tool::Select || !editable || !same_place {
            self.tools.clear_selection();
            return;
        }
        match vector_at(&self.document, layer, frame) {
            Some(drawing) => self.tools.selection.retain(|i| drawing.contains_item(*i)),
            None => self.tools.selection.clear(),
        }
        if self.tools.selection.is_empty() {
            self.tools.clear_selection();
        }
    }

    /// Ends the gesture and commits it as one undoable command.
    pub fn pointer_up(&mut self) {
        match self.tools.gesture.take() {
            Some(Gesture::Move {
                layer,
                frame,
                delta,
                dragged: true,
                ..
            }) if delta != Vec2::ZERO => {
                let items = self.tools.selection.clone();
                let _ = self.execute(Box::new(TranslateItems::new(layer, frame, items, delta)));
            }
            Some(Gesture::Move { .. } | Gesture::Marquee { .. }) => {}
            Some(Gesture::Brush {
                layer,
                frame,
                builder,
                ..
            }) => {
                if let Some(stroke) = builder.finish() {
                    let _ = self.execute(Box::new(AddStroke::new(layer, frame, stroke)));
                }
            }
            Some(Gesture::Erase {
                layer,
                frame,
                mode: EraserMode::Partial,
                hits,
                pieces,
                ..
            }) => {
                if hits.is_empty() {
                    return;
                }
                let commands = hits
                    .into_iter()
                    .zip(pieces)
                    .map(|(id, cut)| Box::new(ReplaceStroke::new(layer, frame, id, cut)) as _)
                    .collect();
                let _ = self.execute(Box::new(Batch::new("Erase", commands)));
            }
            Some(Gesture::Erase {
                layer, frame, hits, ..
            }) => {
                if hits.is_empty() {
                    return;
                }
                let label = if hits.len() == 1 {
                    "Erase Stroke"
                } else {
                    "Erase Strokes"
                };
                let commands = hits
                    .into_iter()
                    .map(|id| Box::new(RemoveStroke::new(layer, frame, id)) as _)
                    .collect();
                let _ = self.execute(Box::new(Batch::new(label, commands)));
            }
            None => {}
        }
    }
}

/// The vector drawing `layer` shows at `frame`.
fn vector_at(
    doc: &document::Document,
    layer: LayerId,
    frame: FrameIndex,
) -> Option<&VectorDrawing> {
    doc.drawing_at(layer, frame)
        .and_then(|id| doc.drawing(id))
        .and_then(|d| d.as_vector())
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor::commands::SetLayerLocked;

    fn strokes_at(s: &Session, frame: u32) -> usize {
        let layer = s.state.active_layer.unwrap();
        s.document
            .drawing_at(layer, FrameIndex(frame))
            .and_then(|id| s.document.drawing(id))
            .and_then(|d| d.as_vector())
            .map_or(0, |d| d.strokes().len())
    }

    fn draw(s: &mut Session, from: Vec2, to: Vec2) {
        assert!(s.pointer_down(from, 1.0));
        for i in 1..=10 {
            s.pointer_move(from.lerp(to, i as f32 / 10.0));
        }
        s.pointer_up();
    }

    #[test]
    fn brush_gesture_adds_one_undoable_stroke() {
        let mut s = Session::new();
        s.state.brush.width = 7.0;
        assert!(s.pointer_down(Vec2::new(10.0, 10.0), 1.0));
        s.pointer_move(Vec2::new(50.0, 10.0));
        let (layer, preview) = s.tools.preview().unwrap();
        assert_eq!(Some(layer), s.state.active_layer);
        assert_eq!(preview.points.len(), 2);
        assert_eq!(strokes_at(&s, 0), 0, "document untouched while drawing");
        s.pointer_move(Vec2::new(90.0, 40.0));
        s.pointer_up();
        assert!(s.tools.preview().is_none());
        assert_eq!(strokes_at(&s, 0), 1);
        let layer = s.state.active_layer.unwrap();
        let drawing = s.document.drawing_at(layer, FrameIndex(0)).unwrap();
        let stroke = &s
            .document
            .drawing(drawing)
            .unwrap()
            .as_vector()
            .unwrap()
            .strokes()[0];
        assert_eq!(stroke.style.width, 7.0);
        assert_eq!(stroke.points[0].pos, Vec2::new(10.0, 10.0));
        s.undo();
        assert_eq!(strokes_at(&s, 0), 0);
        s.redo();
        assert_eq!(strokes_at(&s, 0), 1);
    }

    #[test]
    fn locked_layer_blocks_drawing_with_feedback() {
        let mut s = Session::new();
        let layer = s.state.active_layer.unwrap();
        s.execute(Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        assert!(s.draw_block_reason().unwrap().contains("locked"));
        assert!(!s.pointer_down(Vec2::ZERO, 1.0));
        s.pointer_move(Vec2::ONE);
        s.pointer_up();
        assert!(s.status().unwrap().is_error);
        assert_eq!(strokes_at(&s, 0), 0);
        assert_eq!(s.history.undo_len(), 1);
    }

    #[test]
    fn eraser_removes_hit_strokes_in_one_step() {
        let mut s = Session::new();
        draw(&mut s, Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0));
        draw(&mut s, Vec2::new(0.0, 50.0), Vec2::new(100.0, 50.0));
        draw(&mut s, Vec2::new(0.0, 200.0), Vec2::new(100.0, 200.0));
        assert_eq!(strokes_at(&s, 0), 3);

        s.state.tool = Tool::Eraser;
        s.tools.eraser_mode = EraserMode::Stroke;
        s.tools.eraser_radius = 5.0;
        // A fast vertical swipe across the first two strokes only.
        assert!(s.pointer_down(Vec2::new(50.0, -20.0), 1.0));
        s.pointer_move(Vec2::new(50.0, 80.0));
        assert_eq!(s.tools.erasing().len(), 2);
        assert_eq!(strokes_at(&s, 0), 3, "document untouched while erasing");
        s.pointer_up();
        assert_eq!(strokes_at(&s, 0), 1);
        s.undo();
        assert_eq!(strokes_at(&s, 0), 3);
    }

    fn strokes(s: &Session) -> Vec<Stroke> {
        let layer = s.state.active_layer.unwrap();
        s.document
            .drawing_at(layer, FrameIndex(0))
            .and_then(|id| s.document.drawing(id))
            .and_then(|d| d.as_vector())
            .map_or_else(Vec::new, |d| d.strokes().to_vec())
    }

    #[test]
    fn partial_eraser_splits_with_preview_and_one_undo_step() {
        let mut s = Session::new();
        draw(&mut s, Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0));
        draw(&mut s, Vec2::new(0.0, 100.0), Vec2::new(200.0, 100.0));
        let original = strokes(&s);
        let undo_len = s.history.undo_len();

        s.state.tool = Tool::Eraser;
        assert_eq!(s.tools.eraser_mode, EraserMode::Partial);
        s.tools.eraser_radius = 5.0;
        // Several moves across the first line only.
        assert!(s.pointer_down(Vec2::new(100.0, -30.0), 1.0));
        for y in [-10.0, 0.0, 10.0, 30.0] {
            s.pointer_move(Vec2::new(100.0, y));
        }
        assert_eq!(s.tools.erasing(), [original[0].id]);
        let preview = s.tools.erase_preview(original[0].id).to_vec();
        assert_eq!(preview.len(), 2);
        assert!(s.tools.erase_preview(original[1].id).is_empty());
        assert_eq!(strokes(&s), original, "document untouched while erasing");
        assert_eq!(s.history.undo_len(), undo_len);

        s.pointer_up();
        assert!(s.tools.erasing().is_empty());
        let after = strokes(&s);
        assert_eq!(after.len(), 3);
        assert_eq!(after[..2], preview[..], "commits what the preview showed");
        assert_eq!(after[2], original[1], "z-order kept");
        assert_eq!(s.history.undo_len(), undo_len + 1);
        s.undo();
        assert_eq!(strokes(&s), original);
        s.redo();
        assert_eq!(strokes(&s), after);
    }

    #[test]
    fn split_pieces_survive_save_and_reload() {
        let mut s = Session::new();
        draw(&mut s, Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0));
        s.state.tool = Tool::Eraser;
        assert!(s.pointer_down(Vec2::new(100.0, -20.0), 1.0));
        s.pointer_move(Vec2::new(100.0, 20.0));
        s.pointer_up();
        let pieces = strokes(&s);
        assert_eq!(pieces.len(), 2);

        let path = std::env::temp_dir().join(format!(
            "rusting-erase-{}.{}",
            std::process::id(),
            crate::session::PROJECT_EXTENSION
        ));
        assert!(s.save_to(&path));
        let mut reopened = Session::new();
        assert!(reopened.open_path(&path));
        let _ = std::fs::remove_file(&path);
        assert_eq!(strokes(&reopened), pieces);
    }

    #[test]
    fn partial_eraser_cuts_pieces_again_in_the_same_drag() {
        let mut s = Session::new();
        draw(&mut s, Vec2::new(0.0, 0.0), Vec2::new(200.0, 0.0));
        let id = strokes(&s)[0].id;
        s.state.tool = Tool::Eraser;
        s.tools.eraser_radius = 3.0;
        assert!(s.pointer_down(Vec2::new(50.0, -20.0), 1.0));
        s.pointer_move(Vec2::new(50.0, 20.0));
        s.pointer_move(Vec2::new(150.0, 20.0));
        s.pointer_move(Vec2::new(150.0, -20.0));
        assert_eq!(s.tools.erase_preview(id).len(), 3);
        s.pointer_up();
        assert_eq!(strokes(&s).len(), 3);
        assert_eq!(s.history.undo_label(), Some("Erase"));
    }

    #[test]
    fn eraser_miss_records_nothing() {
        let mut s = Session::new();
        s.state.tool = Tool::Eraser;
        assert!(s.pointer_down(Vec2::new(5.0, 5.0), 1.0));
        s.pointer_up();
        assert!(!s.history.can_undo());
    }

    #[test]
    fn strokes_land_on_the_current_frame() {
        let mut s = Session::new();
        s.set_frame(FrameIndex(8));
        draw(&mut s, Vec2::ZERO, Vec2::new(30.0, 30.0));
        let layer = s.state.active_layer.unwrap();
        let track = s.document.track(layer).unwrap();
        assert!(track.key_at(FrameIndex(8)).is_some());
        assert!(s.document.drawing_at(layer, FrameIndex(7)).is_none());
    }

    // ----- Select tool ---------------------------------------------------

    fn items(s: &Session) -> Vec<ItemId> {
        s.active_drawing()
            .map(|d| d.all_items())
            .unwrap_or_default()
    }

    /// Three horizontal lines at y = 0, 50 and 200, then the select tool.
    fn select_scene() -> (Session, Vec<ItemId>) {
        let mut s = Session::new();
        draw(&mut s, Vec2::new(0.0, 0.0), Vec2::new(100.0, 0.0));
        draw(&mut s, Vec2::new(0.0, 50.0), Vec2::new(100.0, 50.0));
        draw(&mut s, Vec2::new(0.0, 200.0), Vec2::new(100.0, 200.0));
        s.state.tool = Tool::Select;
        let ids = items(&s);
        (s, ids)
    }

    fn click(s: &mut Session, pos: Vec2, shift: bool) {
        assert!(s.pointer_down_with(pos, 1.0, shift));
        s.pointer_up();
    }

    fn drag(s: &mut Session, from: Vec2, to: Vec2, shift: bool) {
        assert!(s.pointer_down_with(from, 1.0, shift));
        for i in 1..=5 {
            s.pointer_move(from.lerp(to, i as f32 / 5.0));
        }
        s.pointer_up();
    }

    #[test]
    fn click_selects_and_shift_click_toggles() {
        let (mut s, ids) = select_scene();
        click(&mut s, Vec2::new(50.0, 1.0), false);
        assert_eq!(s.tools.selection(), [ids[0]]);
        click(&mut s, Vec2::new(50.0, 51.0), false);
        assert_eq!(s.tools.selection(), [ids[1]], "plain click replaces");
        click(&mut s, Vec2::new(50.0, 199.0), true);
        assert_eq!(s.tools.selection(), [ids[1], ids[2]]);
        click(&mut s, Vec2::new(50.0, 50.0), true);
        assert_eq!(s.tools.selection(), [ids[2]], "shift-click removes");
        click(&mut s, Vec2::new(50.0, 120.0), false);
        assert!(s.tools.selection().is_empty(), "empty click clears");
        assert_eq!(s.history.undo_len(), 3, "selecting records nothing");
    }

    #[test]
    fn marquee_selects_intersecting_items() {
        let (mut s, ids) = select_scene();
        drag(
            &mut s,
            Vec2::new(-20.0, -20.0),
            Vec2::new(10.0, 60.0),
            false,
        );
        assert_eq!(s.tools.selection(), [ids[0], ids[1]]);
        assert!(s.tools.marquee().is_none(), "marquee gone after release");
        drag(
            &mut s,
            Vec2::new(90.0, 190.0),
            Vec2::new(150.0, 250.0),
            true,
        );
        assert_eq!(s.tools.selection(), [ids[0], ids[1], ids[2]], "shift adds");
        drag(
            &mut s,
            Vec2::new(90.0, 190.0),
            Vec2::new(150.0, 250.0),
            false,
        );
        assert_eq!(s.tools.selection(), [ids[2]], "plain marquee replaces");
    }

    #[test]
    fn dragging_moves_the_selection_in_one_undo_step() {
        let (mut s, ids) = select_scene();
        let before = strokes(&s);
        s.select_all();
        let undo_len = s.history.undo_len();
        assert!(s.pointer_down_with(Vec2::new(50.0, 50.0), 1.0, false));
        s.pointer_move(Vec2::new(60.0, 45.0));
        assert_eq!(s.tools.move_offset(), Vec2::new(10.0, -5.0));
        assert_eq!(strokes(&s), before, "document untouched while dragging");
        s.pointer_move(Vec2::new(70.0, 40.0));
        s.pointer_up();
        assert_eq!(s.tools.move_offset(), Vec2::ZERO);
        assert_eq!(s.history.undo_len(), undo_len + 1);
        assert_eq!(s.history.undo_label(), Some("Move"));
        let after = strokes(&s);
        for (a, b) in before.iter().zip(&after) {
            assert_eq!(a.id, b.id);
            assert_eq!(b.points[0].pos, a.points[0].pos + Vec2::new(20.0, -10.0));
        }
        assert_eq!(s.tools.selection(), ids, "still selected after the move");
        s.undo();
        assert_eq!(strokes(&s), before);
        s.redo();
        assert_eq!(strokes(&s), after);
    }

    #[test]
    fn tiny_drag_or_click_does_not_move() {
        let (mut s, _) = select_scene();
        let undo_len = s.history.undo_len();
        drag(&mut s, Vec2::new(50.0, 0.0), Vec2::new(51.0, 1.0), false);
        assert_eq!(s.history.undo_len(), undo_len);
        assert_eq!(s.tools.selection().len(), 1);
    }

    #[test]
    fn nudge_and_delete_are_undoable() {
        let (mut s, ids) = select_scene();
        let before = strokes(&s);
        click(&mut s, Vec2::new(50.0, 0.0), false);
        s.nudge_selection(Vec2::new(1.0, 0.0));
        s.nudge_selection(Vec2::new(0.0, 10.0));
        assert_eq!(
            strokes(&s)[0].points[0].pos,
            before[0].points[0].pos + Vec2::new(1.0, 10.0)
        );
        assert_eq!(strokes(&s)[1], before[1]);
        s.undo();
        s.undo();
        assert_eq!(strokes(&s), before);

        s.state.tool = Tool::Select;
        click(&mut s, Vec2::new(50.0, 0.0), false);
        click(&mut s, Vec2::new(50.0, 200.0), true);
        s.delete_selection();
        assert_eq!(items(&s), [ids[1]]);
        assert!(s.tools.selection().is_empty());
        assert_eq!(s.history.undo_label(), Some("Delete"));
        s.undo();
        assert_eq!(strokes(&s), before);
    }

    #[test]
    fn selection_clears_on_context_changes() {
        let (mut s, ids) = select_scene();
        s.select_all();
        assert_eq!(s.tools.selection().len(), 3);
        s.set_frame(FrameIndex(1));
        assert!(s.tools.selection().is_empty(), "frame change");

        s.set_frame(FrameIndex(0));
        s.select_all();
        s.clear_selection();
        assert!(s.tools.selection().is_empty(), "Esc");

        s.select_all();
        s.state.tool = Tool::Brush;
        s.sync_selection();
        assert!(s.tools.selection().is_empty(), "tool change");

        // Undo removes a selected stroke: only that one is dropped.
        s.state.tool = Tool::Select;
        s.select_all();
        s.undo();
        assert_eq!(s.tools.selection(), [ids[0], ids[1]]);

        let layer = s.state.active_layer.unwrap();
        s.execute(Box::new(SetLayerLocked::new(layer, true)))
            .unwrap();
        assert!(s.tools.selection().is_empty(), "locked layer");
        assert!(!s.pointer_down_with(Vec2::new(50.0, 0.0), 1.0, false));
        s.select_all();
        assert!(s.tools.selection().is_empty());

        s.perform(crate::session::DiscardAction::New);
        assert!(s.tools.selection().is_empty());
    }

    #[test]
    fn fills_can_be_selected_and_moved() {
        let mut s = Session::new();
        // A closed square of strokes, then fill inside it.
        let corners = [
            Vec2::new(0.0, 0.0),
            Vec2::new(100.0, 0.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(0.0, 100.0),
        ];
        for i in 0..4 {
            draw(&mut s, corners[i], corners[(i + 1) % 4]);
        }
        s.state.tool = Tool::Fill;
        assert!(s.pointer_down(Vec2::new(50.0, 50.0), 1.0));
        let fill = s.active_drawing().unwrap().fills()[0].clone();

        s.state.tool = Tool::Select;
        click(&mut s, Vec2::new(50.0, 50.0), false);
        assert_eq!(s.tools.selection(), [ItemId::Fill(fill.id)]);
        drag(&mut s, Vec2::new(50.0, 50.0), Vec2::new(60.0, 50.0), false);
        let moved = s.active_drawing().unwrap().fills()[0].clone();
        assert_eq!(moved, fill.translated(Vec2::new(10.0, 0.0)));
        s.undo();
        assert_eq!(s.active_drawing().unwrap().fills()[0], fill);
    }
}
