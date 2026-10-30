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
//!
//! The transform tool works on the same selection. It shows a box around it
//! (see [`TransformBox`]): dragging a corner or edge handle scales from the
//! opposite side (Shift keeps the aspect ratio, Alt scales from the centre),
//! dragging the knob above the box or just outside a corner rotates about the
//! centre (Shift snaps to 15°), and dragging inside the box moves it. The
//! drag is previewed and committed as one `TransformItems` on release. A
//! press anywhere else selects, like the select tool.

use crate::session::Session;
use document::animation_core::{FrameIndex, LayerId, StrokeId, Vec2};
use document::drawing::select::{Affine2, rect_from_corners, scale_rotate_about};
use document::drawing::{
    BrushStyle, Fill, GapClosing, ItemId, Stroke, StrokeBuilder, StrokePoint, VectorDrawing,
};
use editor::Tool;
use editor::commands::{
    AddFill, AddStroke, Batch, RemoveFill, RemoveStroke, ReplaceStroke, TransformItems,
    TranslateItems, remove_items,
};

/// Smallest distance between recorded brush points, in screen points.
const MIN_POINT_SPACING_SCREEN: f32 = 0.75;

/// Brush smoothing sigma at 100 % smoothing, in screen points. Scaled by
/// zoom so the same setting feels the same at any zoom level.
const MAX_SMOOTHING_SCREEN: f32 = 15.0;

/// Default brush smoothing, in percent.
pub const DEFAULT_BRUSH_SMOOTHING: f32 = 50.0;

/// How far outside a stroke a click still selects it, in screen points.
const SELECT_TOLERANCE_SCREEN: f32 = 4.0;

/// How far the pointer must travel before a select press counts as a drag,
/// in screen points.
const DRAG_THRESHOLD_SCREEN: f32 = 3.0;

/// Gap between the selection's bounds and the box drawn around it, in
/// screen points.
pub const SELECTION_BOX_PAD_SCREEN: f32 = 3.0;

/// Half the side of a transform handle's hit area, in screen points.
pub const TRANSFORM_HANDLE_SCREEN: f32 = 6.0;

/// How far above the transform box the rotate knob sits, in screen points.
pub const ROTATE_KNOB_SCREEN: f32 = 22.0;

/// How far outside a corner of the transform box a press rotates, in screen
/// points.
const ROTATE_ZONE_SCREEN: f32 = 24.0;

/// Smallest scale factor the transform tool makes (keeps items from
/// collapsing to nothing, which can't be scaled back).
const MIN_SCALE: f32 = 0.01;

/// Rotation step with Shift held, in degrees.
const ROTATE_SNAP_DEGREES: f32 = 15.0;

/// A part of the transform box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformHandle {
    /// A scale handle. `x` and `y` are -1, 0 or 1: left/top, middle or
    /// right/bottom. Corners have both non-zero, edges one.
    Scale {
        /// Horizontal side.
        x: i8,
        /// Vertical side.
        y: i8,
    },
    /// The rotate knob, or the zone just outside a corner.
    Rotate,
    /// Inside the box: drags move the selection.
    Inside,
}

impl TransformHandle {
    /// The eight scale handles: corners first, then edge middles.
    pub const SCALE: [Self; 8] = [
        Self::Scale { x: -1, y: -1 },
        Self::Scale { x: 1, y: -1 },
        Self::Scale { x: 1, y: 1 },
        Self::Scale { x: -1, y: 1 },
        Self::Scale { x: 0, y: -1 },
        Self::Scale { x: 1, y: 0 },
        Self::Scale { x: 0, y: 1 },
        Self::Scale { x: -1, y: 0 },
    ];
}

/// The transform tool's box around the selection at one zoom level: the
/// selection's bounds padded by [`SELECTION_BOX_PAD_SCREEN`]. Coordinates
/// are document units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformBox {
    /// The selection's bounds, unpadded; transforms are worked out on these.
    pub bounds: (Vec2, Vec2),
    /// Top-left corner of the box.
    pub min: Vec2,
    /// Bottom-right corner of the box.
    pub max: Vec2,
    /// Screen points per document unit.
    pub zoom: f32,
}

impl TransformBox {
    /// The box around `bounds` at `zoom`.
    pub fn new(bounds: (Vec2, Vec2), zoom: f32) -> Self {
        let zoom = zoom.max(f32::EPSILON);
        let pad = Vec2::splat(SELECTION_BOX_PAD_SCREEN / zoom);
        Self {
            bounds,
            min: bounds.0 - pad,
            max: bounds.1 + pad,
            zoom,
        }
    }

    /// Centre of the box (and of the selection's bounds).
    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    /// Where a scale handle sits; `None` for the other handles.
    pub fn handle_pos(&self, handle: TransformHandle) -> Option<Vec2> {
        match handle {
            TransformHandle::Scale { x, y } => {
                let half = (self.max - self.min) * 0.5;
                Some(self.center() + Vec2::new(f32::from(x), f32::from(y)) * half)
            }
            _ => None,
        }
    }

    /// Where the rotate knob sits: above the middle of the top edge.
    pub fn rotate_knob(&self) -> Vec2 {
        Vec2::new(self.center().x, self.min.y - ROTATE_KNOB_SCREEN / self.zoom)
    }

    /// The part of the box under `pos`, if any. Handles win over the
    /// inside, and the inside over the rotate zones outside the corners.
    pub fn hit(&self, pos: Vec2) -> Option<TransformHandle> {
        let reach = TRANSFORM_HANDLE_SCREEN / self.zoom;
        let on_handle = TransformHandle::SCALE.into_iter().find(|&h| {
            self.handle_pos(h)
                .is_some_and(|p| (pos - p).abs().max_element() <= reach)
        });
        if on_handle.is_some() {
            return on_handle;
        }
        if (pos - self.rotate_knob()).length() <= reach {
            return Some(TransformHandle::Rotate);
        }
        if pos.cmpge(self.min).all() && pos.cmple(self.max).all() {
            return Some(TransformHandle::Inside);
        }
        let zone = ROTATE_ZONE_SCREEN / self.zoom;
        let near_corner = TransformHandle::SCALE[..4].iter().any(|&h| {
            self.handle_pos(h)
                .is_some_and(|p| (pos - p).length() <= zone)
        });
        near_corner.then_some(TransformHandle::Rotate)
    }
}

/// What a transform drag does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum TransformMode {
    /// Scale with handle `(x, y)` (see [`TransformHandle::Scale`]) of the
    /// selection's `bounds`.
    Scale { x: i8, y: i8, bounds: (Vec2, Vec2) },
    /// Rotate about `pivot`.
    Rotate { pivot: Vec2 },
}

/// Keeps a scale factor at least [`MIN_SCALE`] away from zero, keeping its
/// sign (mirroring is allowed).
fn clamp_scale(s: f32) -> f32 {
    if s.abs() >= MIN_SCALE {
        s
    } else if s < 0.0 {
        -MIN_SCALE
    } else {
        MIN_SCALE
    }
}

impl TransformMode {
    /// The transform for a drag from `start` to `pos` (document units).
    /// `shift` keeps the aspect ratio when scaling and snaps the angle when
    /// rotating; `alt` scales from the centre instead of the opposite side.
    fn transform(self, start: Vec2, pos: Vec2, shift: bool, alt: bool) -> Affine2 {
        match self {
            Self::Scale { x, y, bounds } => {
                let center = (bounds.0 + bounds.1) * 0.5;
                let side = Vec2::new(f32::from(x), f32::from(y));
                let half = (bounds.1 - bounds.0) * 0.5;
                let handle = center + side * half;
                let anchor = if alt { center } else { center - side * half };
                let reach = handle - anchor;
                let moved = handle + (pos - start) - anchor;
                // Per-axis factor; `None` on an axis the handle doesn't drag
                // or on a zero-size selection.
                let axis =
                    |s: f32, r: f32, m: f32| (s != 0.0 && r.abs() > f32::EPSILON).then(|| m / r);
                let (sx, sy) = (
                    axis(side.x, reach.x, moved.x),
                    axis(side.y, reach.y, moved.y),
                );
                let scale = if shift {
                    let uniform = match (sx, sy) {
                        // Project onto the diagonal so both axes follow.
                        (Some(_), Some(_)) => moved.dot(reach) / reach.length_squared(),
                        (Some(s), None) | (None, Some(s)) => s,
                        (None, None) => 1.0,
                    };
                    Vec2::splat(uniform)
                } else {
                    Vec2::new(sx.unwrap_or(1.0), sy.unwrap_or(1.0))
                };
                let scale = Vec2::new(clamp_scale(scale.x), clamp_scale(scale.y));
                scale_rotate_about(anchor, scale, 0.0)
            }
            Self::Rotate { pivot } => {
                let (from, to) = (start - pivot, pos - pivot);
                let mut angle = from.angle_to(to);
                if !angle.is_finite() {
                    angle = 0.0;
                }
                if shift {
                    let step = ROTATE_SNAP_DEGREES.to_radians();
                    angle = (angle / step).round() * step;
                }
                scale_rotate_about(pivot, Vec2::ONE, angle)
            }
        }
    }
}

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
    /// Dragging a selection box, or a lasso loop.
    Marquee {
        layer: LayerId,
        frame: FrameIndex,
        /// Box: the press and the latest position. Lasso: every position.
        path: Vec<Vec2>,
        lasso: bool,
        zoom: f32,
        dragged: bool,
        /// The selection to add to (Shift), or empty.
        base: Vec<ItemId>,
        /// The item under the press; a click without a drag selects it.
        hit: Option<ItemId>,
    },
    /// Scaling or rotating the selection with the transform tool.
    Transform {
        layer: LayerId,
        frame: FrameIndex,
        mode: TransformMode,
        start: Vec2,
        /// Latest pointer position, to redo the transform when a modifier
        /// changes without the pointer moving.
        last: Vec2,
        zoom: f32,
        dragged: bool,
        transform: Affine2,
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
    /// Brush smoothing in percent (`0.0..=100.0`). Higher irons out more of
    /// the hand and tablet wobble.
    pub brush_smoothing: f32,
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
    /// Shift held: keep the aspect ratio / snap the angle while transforming.
    shift: bool,
    /// Alt held: scale from the centre while transforming.
    alt: bool,
}

impl Default for ToolController {
    fn default() -> Self {
        Self {
            brush_smoothing: DEFAULT_BRUSH_SMOOTHING,
            eraser_radius: 12.0,
            eraser_mode: EraserMode::default(),
            fill_gap: GapClosing::default(),
            gesture: None,
            selection: Vec::new(),
            selection_at: None,
            shift: false,
            alt: false,
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

    /// How the selection is being dragged, as one transform: a translation
    /// while moving it, a scale or rotation with the transform tool.
    /// Identity when not dragging.
    pub fn selection_transform(&self) -> Affine2 {
        match &self.gesture {
            Some(Gesture::Transform {
                transform,
                dragged: true,
                ..
            }) => *transform,
            _ => Affine2::from_translation(self.move_offset()),
        }
    }

    /// The scale or rotation being dragged with the transform tool; draw the
    /// selected items through it (with [`Stroke::transformed`], so brush
    /// widths match what release commits). `None` when not transforming.
    pub fn transform_preview(&self) -> Option<Affine2> {
        match &self.gesture {
            Some(Gesture::Transform {
                transform,
                dragged: true,
                ..
            }) => Some(*transform),
            _ => None,
        }
    }

    /// The transform handle being dragged, if any (for the cursor).
    pub fn transform_dragging(&self) -> Option<TransformHandle> {
        match &self.gesture {
            Some(Gesture::Transform { mode, .. }) => Some(match *mode {
                TransformMode::Scale { x, y, .. } => TransformHandle::Scale { x, y },
                TransformMode::Rotate { .. } => TransformHandle::Rotate,
            }),
            _ => None,
        }
    }

    /// Tells the transform tool which modifiers are held: Shift keeps the
    /// aspect ratio (or snaps the angle), Alt scales from the centre. The
    /// drag in progress follows at once.
    pub fn set_modifiers(&mut self, shift: bool, alt: bool) {
        if (shift, alt) == (self.shift, self.alt) {
            return;
        }
        self.shift = shift;
        self.alt = alt;
        if let Some(Gesture::Transform {
            mode,
            start,
            last,
            transform,
            ..
        }) = &mut self.gesture
        {
            *transform = mode.transform(*start, *last, shift, alt);
        }
    }

    /// The selection box being dragged, as `(min, max)` in document
    /// coordinates.
    pub fn marquee(&self) -> Option<(Vec2, Vec2)> {
        match &self.gesture {
            Some(Gesture::Marquee {
                path,
                lasso: false,
                dragged: true,
                ..
            }) => Some(rect_from_corners(path[0], path[path.len() - 1])),
            _ => None,
        }
    }

    /// The lasso loop being drawn, in document coordinates.
    pub fn lasso(&self) -> Option<&[Vec2]> {
        match &self.gesture {
            Some(Gesture::Marquee {
                path,
                lasso: true,
                dragged: true,
                ..
            }) => Some(path),
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
            Some(Gesture::Move { .. } | Gesture::Marquee { .. } | Gesture::Transform { .. })
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
                    points: builder.smoothed_points(),
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
        self.tools.shift = shift;
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
            Tool::Select | Tool::Lasso => {
                let lasso = self.state.tool == Tool::Lasso;
                match self.select_press(layer, frame, pos, zoom, shift, lasso) {
                    Some(gesture) => gesture,
                    None => return true,
                }
            }
            Tool::Transform => match self.transform_press(layer, frame, pos, zoom, shift) {
                Some(gesture) => gesture,
                None => return true,
            },
            Tool::Brush => {
                let style = BrushStyle {
                    width: self.state.brush.width.max(0.1),
                    ..self.state.brush
                };
                let zoom = zoom.max(f32::EPSILON);
                let min_distance = MIN_POINT_SPACING_SCREEN / zoom;
                let smoothing = self.tools.brush_smoothing.clamp(0.0, 100.0) / 100.0
                    * MAX_SMOOTHING_SCREEN
                    / zoom;
                let mut builder = StrokeBuilder::begin(style)
                    .with_min_distance(min_distance)
                    .with_smoothing(smoothing);
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
        let (shift, alt) = (self.tools.shift, self.tools.alt);
        match &mut self.tools.gesture {
            Some(Gesture::Transform {
                mode,
                start,
                last,
                zoom,
                dragged,
                transform,
                ..
            }) => {
                *last = pos;
                *dragged |= past_drag_threshold(*start, pos, *zoom);
                *transform = mode.transform(*start, pos, shift, alt);
            }
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
                path,
                lasso,
                zoom,
                dragged,
                base,
                ..
            }) => {
                if !*lasso && path.len() == 2 {
                    path[1] = pos;
                } else if path.last() != Some(&pos) {
                    path.push(pos);
                }
                *dragged |= past_drag_threshold(path[0], pos, *zoom);
                if *dragged {
                    let polygon = if *lasso {
                        path.clone()
                    } else {
                        let (min, max) = rect_from_corners(path[0], pos);
                        vec![min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
                    };
                    let mut items = base.clone();
                    if let Some(drawing) = vector_at(&self.document, *layer, *frame) {
                        for item in drawing.items_in_polygon(&polygon) {
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

    /// Select and lasso tool press: toggles the item under `pos` with Shift,
    /// starts moving the selection when `pos` is on it, and otherwise starts
    /// a box (or a lasso loop). Returns the drag gesture to start, if any.
    fn select_press(
        &mut self,
        layer: LayerId,
        frame: FrameIndex,
        pos: Vec2,
        zoom: f32,
        shift: bool,
        lasso: bool,
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
            Some(item) if self.tools.selection.contains(&item) => Some(Gesture::Move {
                layer,
                frame,
                start: pos,
                delta: Vec2::ZERO,
                zoom,
                dragged: false,
            }),
            // Unselected item or empty canvas: dragging draws a box, so a
            // background fill doesn't get dragged around by accident.
            hit => {
                let base = if shift {
                    self.tools.selection.clone()
                } else {
                    self.tools.clear_selection();
                    Vec::new()
                };
                Some(Gesture::Marquee {
                    layer,
                    frame,
                    path: vec![pos],
                    lasso,
                    zoom,
                    dragged: false,
                    base,
                    hit,
                })
            }
        }
    }

    /// The transform tool's box around the selection at `zoom` (screen
    /// points per document unit), or `None` when that tool is not active or
    /// nothing is selected.
    pub fn transform_box(&self, zoom: f32) -> Option<TransformBox> {
        if self.state.tool != Tool::Transform {
            return None;
        }
        self.tools.selection_at()?;
        let bounds = self.active_drawing()?.items_bounds(&self.tools.selection)?;
        Some(TransformBox::new(bounds, zoom))
    }

    /// Transform tool press: starts scaling, rotating or moving when `pos`
    /// is on the selection's box, and otherwise selects like the select
    /// tool.
    fn transform_press(
        &mut self,
        layer: LayerId,
        frame: FrameIndex,
        pos: Vec2,
        zoom: f32,
        shift: bool,
    ) -> Option<Gesture> {
        self.sync_selection();
        let zoom = zoom.max(f32::EPSILON);
        let hit = self
            .transform_box(zoom)
            .filter(|_| self.tools.selection_at() == Some((layer, frame)))
            .and_then(|b| b.hit(pos).map(|h| (b, h)));
        let mode = match hit {
            Some((b, TransformHandle::Scale { x, y })) => TransformMode::Scale {
                x,
                y,
                bounds: b.bounds,
            },
            Some((b, TransformHandle::Rotate)) => TransformMode::Rotate {
                pivot: (b.bounds.0 + b.bounds.1) * 0.5,
            },
            Some((_, TransformHandle::Inside)) if !shift => {
                return Some(Gesture::Move {
                    layer,
                    frame,
                    start: pos,
                    delta: Vec2::ZERO,
                    zoom,
                    dragged: false,
                });
            }
            _ => return self.select_press(layer, frame, pos, zoom, shift, false),
        };
        Some(Gesture::Transform {
            layer,
            frame,
            mode,
            start: pos,
            last: pos,
            zoom,
            dragged: false,
            transform: Affine2::IDENTITY,
        })
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

    /// Keeps the selection valid: clears it when neither the select nor the
    /// transform tool is active, when the active layer or current frame differs from where it
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
        if !self.state.tool.uses_selection() || !editable || !same_place {
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
            Some(Gesture::Transform {
                layer,
                frame,
                mode,
                dragged: true,
                transform,
                ..
            }) if transform != Affine2::IDENTITY => {
                let label = match mode {
                    TransformMode::Scale { .. } => "Scale",
                    TransformMode::Rotate { .. } => "Rotate",
                };
                let items = self.tools.selection.clone();
                let command = TransformItems::new(layer, frame, items, transform).with_label(label);
                let _ = self.execute(Box::new(command));
            }
            Some(Gesture::Marquee {
                layer,
                frame,
                dragged: false,
                hit: Some(item),
                ..
            }) => self.tools.set_selection((layer, frame), vec![item]),
            Some(Gesture::Move { .. } | Gesture::Marquee { .. } | Gesture::Transform { .. }) => {}
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
    fn dragging_from_an_unselected_item_draws_a_box() {
        let (mut s, ids) = select_scene();
        let before = strokes(&s);
        let undo_len = s.history.undo_len();
        drag(&mut s, Vec2::new(50.0, 0.0), Vec2::new(120.0, 60.0), false);
        assert_eq!(strokes(&s), before, "nothing moved");
        assert_eq!(s.history.undo_len(), undo_len);
        assert_eq!(s.tools.selection(), [ids[0], ids[1]]);

        // Now on the selection: the same drag moves it.
        drag(&mut s, Vec2::new(50.0, 0.0), Vec2::new(60.0, 0.0), false);
        assert_eq!(s.history.undo_label(), Some("Move"));
        assert_eq!(s.tools.selection(), [ids[0], ids[1]]);
    }

    #[test]
    fn box_over_a_background_fill_leaves_it_alone() {
        let (mut s, ids) = select_scene();
        let layer = s.state.active_layer.unwrap();
        let background = Fill::new(
            document::drawing::FillShape {
                contours: vec![vec![
                    Vec2::new(-500.0, -500.0),
                    Vec2::new(500.0, -500.0),
                    Vec2::new(500.0, 500.0),
                    Vec2::new(-500.0, 500.0),
                ]],
            },
            document::animation_core::Rgba::BLACK,
            1.0,
        );
        s.execute(Box::new(AddFill::new(layer, FrameIndex(0), background)))
            .unwrap();
        drag(
            &mut s,
            Vec2::new(-20.0, 40.0),
            Vec2::new(120.0, 60.0),
            false,
        );
        assert_eq!(s.tools.selection(), [ids[1]]);
    }

    #[test]
    fn lasso_selects_what_the_loop_goes_around() {
        let (mut s, ids) = select_scene();
        s.state.tool = Tool::Lasso;
        assert!(s.tools.lasso().is_none());
        // A loop around the start of the first two lines only.
        let path = [
            Vec2::new(-10.0, -10.0),
            Vec2::new(20.0, -10.0),
            Vec2::new(20.0, 60.0),
            Vec2::new(-10.0, 60.0),
        ];
        assert!(s.pointer_down_with(path[0], 1.0, false));
        for p in &path[1..] {
            s.pointer_move(*p);
        }
        assert_eq!(s.tools.lasso().map(<[Vec2]>::len), Some(4));
        assert!(s.tools.marquee().is_none(), "no box with the lasso");
        s.pointer_up();
        assert_eq!(s.tools.selection(), [ids[0], ids[1]]);
        assert!(s.tools.lasso().is_none(), "loop gone after release");

        // Shift adds; a loop around the end of the third line.
        assert!(s.pointer_down_with(Vec2::new(90.0, 190.0), 1.0, true));
        s.pointer_move(Vec2::new(150.0, 190.0));
        s.pointer_move(Vec2::new(150.0, 250.0));
        s.pointer_up();
        assert_eq!(s.tools.selection(), [ids[0], ids[1], ids[2]]);

        // A straight drag encloses nothing.
        drag(
            &mut s,
            Vec2::new(-10.0, 25.0),
            Vec2::new(120.0, 25.0),
            false,
        );
        assert!(s.tools.selection().is_empty());
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

    // ----- Transform tool ------------------------------------------------

    fn close(a: Vec2, b: Vec2) -> bool {
        (a - b).length() < 1e-3
    }

    /// The select scene with everything selected and the transform tool on.
    fn transform_scene() -> (Session, TransformBox) {
        let (mut s, _) = select_scene();
        s.select_all();
        s.state.tool = Tool::Transform;
        s.sync_selection();
        assert_eq!(
            s.tools.selection().len(),
            3,
            "selection survives the switch"
        );
        let b = s.transform_box(1.0).unwrap();
        (s, b)
    }

    #[test]
    fn transform_box_hit_test() {
        let b = TransformBox::new((Vec2::ZERO, Vec2::new(100.0, 50.0)), 2.0);
        let pad = SELECTION_BOX_PAD_SCREEN / 2.0;
        assert_eq!(b.min, Vec2::splat(-pad));
        assert_eq!(b.hit(b.max), Some(TransformHandle::Scale { x: 1, y: 1 }));
        assert_eq!(
            b.hit(Vec2::new(50.0, 50.0 + pad + 1.0)),
            Some(TransformHandle::Scale { x: 0, y: 1 })
        );
        assert_eq!(b.hit(b.rotate_knob()), Some(TransformHandle::Rotate));
        assert_eq!(b.hit(Vec2::new(30.0, 20.0)), Some(TransformHandle::Inside));
        // Just outside a corner rotates; far away is nothing.
        assert_eq!(
            b.hit(b.max + Vec2::splat(6.0)),
            Some(TransformHandle::Rotate)
        );
        assert_eq!(b.hit(Vec2::new(300.0, 300.0)), None);
    }

    #[test]
    fn scale_math_follows_the_handle() {
        let bounds = (Vec2::ZERO, Vec2::new(100.0, 50.0));
        let corner = TransformMode::Scale { x: 1, y: 1, bounds };
        // Bottom-right dragged by (+100, +50): double size, top-left fixed.
        let t = corner.transform(Vec2::ZERO, Vec2::new(100.0, 50.0), false, false);
        assert!(close(t.transform_point2(Vec2::ZERO), Vec2::ZERO));
        assert!(close(t.transform_point2(bounds.1), Vec2::new(200.0, 100.0)));
        // Alt: scale from the centre instead, so both sides move.
        let t = corner.transform(Vec2::ZERO, Vec2::new(50.0, 25.0), false, true);
        assert!(close(
            t.transform_point2(Vec2::new(50.0, 25.0)),
            Vec2::new(50.0, 25.0)
        ));
        assert!(close(t.transform_point2(bounds.1), Vec2::new(150.0, 75.0)));
        // Shift: uneven drag still scales both axes alike.
        let t = corner.transform(Vec2::ZERO, Vec2::new(100.0, 0.0), true, false);
        let size = t.transform_point2(bounds.1);
        assert!((size.x / size.y - 2.0).abs() < 1e-4, "aspect kept: {size}");
        // An edge handle only scales its own axis, unless Shift is held.
        let right = TransformMode::Scale { x: 1, y: 0, bounds };
        let t = right.transform(Vec2::ZERO, Vec2::new(-50.0, 30.0), false, false);
        assert!(close(t.transform_point2(bounds.1), Vec2::new(50.0, 50.0)));
        let t = right.transform(Vec2::ZERO, Vec2::new(-50.0, 30.0), true, false);
        assert!(close(t.transform_point2(bounds.1), Vec2::new(50.0, 37.5)));
        // Dragging past the anchor mirrors, but never collapses to zero.
        let t = right.transform(Vec2::ZERO, Vec2::new(-100.0, 0.0), false, false);
        assert!(t.matrix2.determinant().abs() > 0.0);
        let t = right.transform(Vec2::ZERO, Vec2::new(-200.0, 0.0), false, false);
        assert!(close(t.transform_point2(bounds.1), Vec2::new(-100.0, 50.0)));
    }

    #[test]
    fn rotate_math_and_snap() {
        let rotate = TransformMode::Rotate { pivot: Vec2::ZERO };
        // From straight up to straight right: a quarter turn.
        let t = rotate.transform(Vec2::new(0.0, -10.0), Vec2::new(10.0, 0.0), false, false);
        assert!(close(
            t.transform_point2(Vec2::new(0.0, -1.0)),
            Vec2::new(1.0, 0.0)
        ));
        // 20° snaps to 15° with Shift.
        let end = Vec2::from_angle(20f32.to_radians()) * 10.0;
        let t = rotate.transform(Vec2::new(10.0, 0.0), end, true, false);
        let got = Vec2::X.angle_to(t.transform_point2(Vec2::X)).to_degrees();
        assert!((got - 15.0).abs() < 1e-3, "{got}");
    }

    #[test]
    fn dragging_a_corner_scales_in_one_undo_step() {
        let (mut s, b) = transform_scene();
        let before = strokes(&s);
        let undo_len = s.history.undo_len();
        let handle = b.handle_pos(TransformHandle::Scale { x: 1, y: 1 }).unwrap();
        let (min, max) = b.bounds;
        assert!(s.pointer_down_with(handle, 1.0, false));
        s.pointer_move(handle + (max - min) * 0.25);
        let preview = s.tools.transform_preview().expect("live preview");
        assert_eq!(strokes(&s), before, "document untouched while dragging");
        s.pointer_move(handle + (max - min));
        s.pointer_up();
        assert!(s.tools.transform_preview().is_none());
        assert_ne!(preview, Affine2::IDENTITY);
        assert_eq!(s.history.undo_len(), undo_len + 1);
        assert_eq!(s.history.undo_label(), Some("Scale"));
        let after = strokes(&s);
        for (a, z) in before.iter().zip(&after) {
            assert_eq!(a.id, z.id);
            for (p, q) in a.points.iter().zip(&z.points) {
                // Doubled about the top-left corner of the bounds.
                assert!(close(q.pos, min + (p.pos - min) * 2.0));
            }
            assert!((z.style.width - a.style.width * 2.0).abs() < 1e-3);
        }
        assert_eq!(s.tools.selection().len(), 3, "still selected");
        s.undo();
        assert_eq!(strokes(&s), before);
        s.redo();
        assert_eq!(strokes(&s), after);
    }

    #[test]
    fn dragging_the_knob_rotates_about_the_centre() {
        let (mut s, b) = transform_scene();
        let before = strokes(&s);
        let center = (b.bounds.0 + b.bounds.1) * 0.5;
        let knob = b.rotate_knob();
        assert!(s.pointer_down_with(knob, 1.0, false));
        // Swing from above the centre to its right: a quarter turn.
        s.pointer_move(center + Vec2::new((knob - center).length(), 0.0));
        s.pointer_up();
        assert_eq!(s.history.undo_label(), Some("Rotate"));
        let after = strokes(&s);
        let quarter = |p: Vec2| center + Vec2::new(-(p - center).y, (p - center).x);
        for (a, z) in before.iter().zip(&after) {
            for (p, q) in a.points.iter().zip(&z.points) {
                assert!(
                    close(q.pos, quarter(p.pos)),
                    "{} vs {}",
                    q.pos,
                    quarter(p.pos)
                );
            }
            assert!((z.style.width - a.style.width).abs() < 1e-4);
        }
        s.undo();
        assert_eq!(strokes(&s), before);
    }

    #[test]
    fn modifiers_update_the_drag_in_place() {
        let (mut s, b) = transform_scene();
        let handle = b.handle_pos(TransformHandle::Scale { x: 1, y: 1 }).unwrap();
        assert!(s.pointer_down_with(handle, 1.0, false));
        s.pointer_move(handle + Vec2::new(100.0, 0.0));
        let free = s.tools.transform_preview().unwrap();
        s.tools.set_modifiers(true, false);
        let kept = s.tools.transform_preview().unwrap();
        assert_ne!(free, kept);
        let d = kept.matrix2;
        assert!((d.x_axis.x - d.y_axis.y).abs() < 1e-5, "uniform with Shift");
        s.tools.set_modifiers(false, false);
        assert_eq!(s.tools.transform_preview(), Some(free));
        s.tools.cancel();
    }

    #[test]
    fn dragging_inside_the_box_moves() {
        let (mut s, b) = transform_scene();
        let before = strokes(&s);
        // Empty space inside the box, between the lines.
        let inside = Vec2::new(50.0, 120.0);
        assert_eq!(b.hit(inside), Some(TransformHandle::Inside));
        drag(&mut s, inside, inside + Vec2::new(10.0, 5.0), false);
        assert_eq!(s.history.undo_label(), Some("Move"));
        assert_eq!(
            strokes(&s)[0].points[0].pos,
            before[0].points[0].pos + Vec2::new(10.0, 5.0)
        );
    }

    #[test]
    fn transform_without_a_selection_just_selects() {
        let (mut s, ids) = select_scene();
        s.state.tool = Tool::Transform;
        let before = strokes(&s);
        let undo_len = s.history.undo_len();
        assert!(s.transform_box(1.0).is_none());
        // A drag on empty space is a marquee, not a transform.
        drag(
            &mut s,
            Vec2::new(-20.0, -20.0),
            Vec2::new(10.0, 60.0),
            false,
        );
        assert_eq!(s.tools.selection(), [ids[0], ids[1]]);
        // A click elsewhere selects that item instead.
        click(&mut s, Vec2::new(50.0, 200.0), false);
        assert_eq!(s.tools.selection(), [ids[2]]);
        assert_eq!(strokes(&s), before);
        assert_eq!(s.history.undo_len(), undo_len);
        // Other tools drop the selection.
        s.state.tool = Tool::Brush;
        s.sync_selection();
        assert!(s.tools.selection().is_empty());
    }

    #[test]
    fn a_click_on_a_handle_records_nothing() {
        let (mut s, b) = transform_scene();
        let undo_len = s.history.undo_len();
        let handle = b
            .handle_pos(TransformHandle::Scale { x: -1, y: 0 })
            .unwrap();
        click(&mut s, handle, false);
        drag(&mut s, handle, handle + Vec2::new(1.0, 0.0), false);
        assert_eq!(s.history.undo_len(), undo_len);
    }
}
