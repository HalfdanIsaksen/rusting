//! Brush and eraser gestures on the canvas.
//!
//! The host converts pointer positions to document coordinates (with
//! `renderer::Viewport::screen_to_canvas`) and calls
//! [`Session::pointer_down`], [`Session::pointer_move`] and
//! [`Session::pointer_up`]. A brush gesture builds a stroke and commits it as
//! one `AddStroke`; an eraser gesture collects hit strokes and removes them as
//! one undo step. The document is not touched until the gesture ends.

use crate::session::Session;
use document::animation_core::{FrameIndex, LayerId, StrokeId, Vec2};
use document::drawing::{BrushStyle, Stroke, StrokeBuilder, StrokePoint};
use editor::Tool;
use editor::commands::{AddStroke, Batch, RemoveStroke};

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
        last: Vec2,
        hits: Vec<StrokeId>,
    },
}

/// Tool settings and the gesture in progress.
#[derive(Clone, Debug)]
pub struct ToolController {
    /// Eraser radius in document units.
    pub eraser_radius: f32,
    gesture: Option<Gesture>,
}

impl Default for ToolController {
    fn default() -> Self {
        Self {
            eraser_radius: 12.0,
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

    /// Strokes the eraser gesture will remove; hide them while erasing.
    pub fn erasing(&self) -> &[StrokeId] {
        match &self.gesture {
            Some(Gesture::Erase { hits, .. }) => hits,
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
                last: pos,
                hits: Vec::new(),
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
                last,
                hits,
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
