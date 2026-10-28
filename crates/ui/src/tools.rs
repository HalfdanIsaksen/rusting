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

use crate::session::Session;
use document::animation_core::{FrameIndex, LayerId, StrokeId, Vec2};
use document::drawing::{BrushStyle, Fill, GapClosing, Stroke, StrokeBuilder, StrokePoint};
use editor::Tool;
use editor::commands::{AddFill, AddStroke, Batch, RemoveFill, RemoveStroke, ReplaceStroke};

/// Smallest distance between recorded brush points, in screen points.
const MIN_POINT_SPACING_SCREEN: f32 = 0.75;

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
}

impl Default for ToolController {
    fn default() -> Self {
        Self {
            eraser_radius: 12.0,
            eraser_mode: EraserMode::default(),
            fill_gap: GapClosing::default(),
            gesture: None,
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

    /// Ends the gesture and commits it as one undoable command.
    pub fn pointer_up(&mut self) {
        match self.tools.gesture.take() {
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
}
