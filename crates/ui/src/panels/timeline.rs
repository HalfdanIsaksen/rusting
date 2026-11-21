//! The timeline panel: playback bar, key buttons, onion-skin settings and
//! the frame grid (one row per layer, keys and exposures, scrubbing). The
//! layer controls on the left of the grid live in [`super::layers`].

use super::library::{AssetDrag, ObjectDrag};
use super::shortcuts;
use super::status::fps_text;
use crate::icons::{self, IconFn};
use crate::onion::MAX_ONION_RANGE;
use crate::session::{Session, TimelineView};
use document::Document;
use document::animation_core::{Fps, FrameIndex, GroupId, Interpolation, LayerId};
use document::drawing::DrawingContent;
use document::scene::BlendMode;
use document::timeline::Keyframe;
use editor::commands::{
    AddKeyframe, ClearFrame, KeyframeKind, MoveKeyframe, MoveLayerFrames, RemoveKeyframe,
    SetAudioClip, SetFps, SetFrameCount,
};
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, vec2};
use std::collections::BTreeSet;

/// Height of one layer row.
pub(super) const ROW_HEIGHT: f32 = 22.0;
/// Height of the tag strip at the top of the ruler.
const TAG_HEIGHT: f32 = 16.0;
/// Height of the frame numbers under the tag strip.
pub(super) const NUMBERS_HEIGHT: f32 = 20.0;
/// Height of the ruler: tags plus frame numbers (and the layer column's header).
pub(super) const RULER_HEIGHT: f32 = TAG_HEIGHT + NUMBERS_HEIGHT;
const MAX_FPS: u32 = 120;
/// Colour of object (instance) exposures in the frame grid.
const INSTANCE_TINT: Color32 = Color32::from_rgb(70, 150, 190);
/// Colour of motion tween spans in the frame grid (ADR-019).
const TWEEN_TINT: Color32 = Color32::from_rgb(140, 110, 200);
const MAX_FRAMES: u32 = 100_000;
/// Width of the grab area right of the last frame that sets the length.
const LENGTH_HANDLE: f32 = 12.0;

/// Draws the timeline panel.
pub fn panel(ui: &mut egui::Ui, session: &mut Session) {
    ui.add_space(4.0);
    transport(ui, session);
    ui.add_space(2.0);
    keys_and_onion(ui, session);
    ui.separator();
    grid(ui, session);
    tag_rename_dialog(ui.ctx(), session);
}

/// Where the tag being renamed (index and edited name) is kept.
fn tag_rename_id() -> egui::Id {
    egui::Id::unique("timeline_tag_rename")
}

/// Opens the rename dialog for tag `index`.
fn start_tag_rename(ui: &egui::Ui, session: &Session, index: usize) {
    if let Some(tag) = session.document.tags.get(index) {
        let edit = (index, tag.name.clone());
        ui.ctx().data_mut(|d| d.insert_temp(tag_rename_id(), edit));
    }
}

/// The "Rename Tag" dialog, while one is open.
fn tag_rename_dialog(ctx: &egui::Context, session: &mut Session) {
    let id = tag_rename_id();
    let Some((index, mut name)) = ctx.data(|d| d.get_temp::<(usize, String)>(id)) else {
        return;
    };
    let mut done = None;
    let response = egui::Modal::new(id.with("modal")).show(ctx, |ui| {
        ui.heading("Rename Tag");
        let edit = ui.text_edit_singleline(&mut name);
        edit.request_focus();
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() || enter {
                done = Some(true);
            }
            if ui.button("Cancel").clicked() {
                done = Some(false);
            }
        });
    });
    if response.should_close() {
        done.get_or_insert(false);
    }
    match done {
        Some(ok) => {
            ctx.data_mut(|d| d.remove::<(usize, String)>(id));
            if ok {
                session.rename_tag(index, &name);
            }
        }
        None => {
            ctx.data_mut(|d| d.insert_temp(id, (index, name)));
        }
    }
}

/// Play/pause, stepping, loop, frame display, FPS and length.
fn transport(ui: &mut egui::Ui, session: &mut Session) {
    ui.horizontal_wrapped(|ui| {
        let playing = session.playback.is_playing();
        let last = FrameIndex(session.document.frame_count.saturating_sub(1));
        if ui.button("⏮").on_hover_text("First frame").clicked() {
            session.playback.pause();
            session.set_frame(FrameIndex::ZERO);
        }
        if ui.button("⏴").on_hover_text("Previous frame (←)").clicked() {
            session.playback.pause();
            session.step(-1);
        }
        let play = if playing { "⏸ Pause" } else { "▶ Play" };
        if ui
            .add(egui::Button::new(play).min_size(vec2(70.0, 0.0)))
            .on_hover_text("Space")
            .clicked()
        {
            session.toggle_playback();
        }
        if ui.button("⏵").on_hover_text("Next frame (→)").clicked() {
            session.playback.pause();
            session.step(1);
        }
        if ui.button("⏭").on_hover_text("Last frame").clicked() {
            session.playback.pause();
            session.set_frame(last);
        }
        let mut looping = session.playback.looping();
        if ui
            .toggle_value(&mut looping, "🔁 Loop")
            .on_hover_text("Loop playback")
            .changed()
        {
            session.playback.set_looping(looping);
        }

        ui.separator();
        ui.label("Frame");
        let mut shown = session.current_frame().display_number();
        let response = ui.add(
            egui::DragValue::new(&mut shown)
                .range(1..=session.document.frame_count.max(1))
                .speed(0.25),
        );
        if response.changed()
            && let Some(frame) = FrameIndex::from_display_number(shown)
        {
            session.playback.pause();
            session.set_frame(frame);
        }
        ui.label(format!("/ {}", session.document.frame_count));

        ui.separator();
        ui.label("FPS");
        let fps = session.document.fps;
        let whole = fps.as_f64().round() as u32;
        if let Some(value) = committed_drag_value(ui, "fps", whole, 1..=MAX_FPS)
            && let Some(new_fps) = Fps::whole(value)
        {
            let _ = session.execute(Box::new(SetFps::new(new_fps)));
        }
        if fps.denominator() != 1 {
            ui.weak(format!("({})", fps_text(fps)));
        }
        ui.label("Length");
        if let Some(count) = committed_drag_value(
            ui,
            "frame_count",
            session.document.frame_count,
            1..=MAX_FRAMES,
        ) {
            let _ = session.execute(Box::new(SetFrameCount::new(count)));
        }
        if session.playback.is_playing() && session.playback.dropped() > 0 {
            ui.weak(format!("{} dropped", session.playback.dropped()));
        }
    });
}

/// Key buttons, onion-skin toggles and timeline zoom.
fn keys_and_onion(ui: &mut egui::Ui, session: &mut Session) {
    ui.horizontal_wrapped(|ui| {
        let frame = session.current_frame();
        let layer = session.state.active_layer;
        let key_here = layer
            .and_then(|id| session.document.track(id))
            .and_then(|t| t.key_at(frame))
            .is_some();
        // Off while playing: the key-at-frame state changes every frame.
        let editable = layer.is_some() && !session.playback.is_playing();
        let free = editable && !key_here;

        if icon_button(
            ui,
            icons::drawing_key,
            free,
            "Drawing key: new blank drawing at this frame on the active layer",
        ) && let Some(id) = layer
        {
            let _ = session.execute(Box::new(AddKeyframe::new(id, frame, KeyframeKind::Drawing)));
        }
        if icon_button(
            ui,
            icons::empty_key,
            free,
            "Empty key: end the exposure here (show nothing)",
        ) && let Some(id) = layer
        {
            let _ = session.execute(Box::new(AddKeyframe::new(id, frame, KeyframeKind::Empty)));
        }
        let duplicate_tip = format!(
            "Duplicate frame ({}): copy this frame to the next one, on the active layer",
            ui.ctx().format_shortcut(&shortcuts::DUPLICATE_FRAME)
        );
        if icon_button(ui, icons::duplicate, editable, &duplicate_tip) {
            session.duplicate_frame();
        }
        let drawn = editable
            && layer.is_some_and(|id| {
                let doc = &session.document;
                doc.layers().is_editable(id)
                    && doc
                        .drawing_at(id, frame)
                        .and_then(|d| doc.drawing(d))
                        .is_some_and(|d| *d != DrawingContent::default())
            });
        if icon_button(
            ui,
            icons::remove_key,
            drawn,
            "Clear frame: wipe the drawing shown at this frame, keeping its key \
             (right-click a key to remove it)",
        ) && let Some(id) = layer
        {
            let _ = session.execute(Box::new(ClearFrame::new(id, frame)));
        }
        let tag_tip = if session.frame_selection().is_some() {
            "Tag the selected frames: playback loops inside a tag"
        } else {
            "Tag this frame (select frames first to tag a range): playback loops inside a tag"
        };
        if icon_button(ui, icons::tag, !session.playback.is_playing(), tag_tip)
            && let Some(index) = session.add_tag()
        {
            start_tag_rename(ui, session, index);
        }

        ui.separator();
        ui.checkbox(&mut session.onion.enabled, "Onion skin");
        ui.add_enabled_ui(session.onion.enabled, |ui| {
            ui.label("before");
            ui.add(egui::DragValue::new(&mut session.onion.before).range(0..=MAX_ONION_RANGE));
            ui.label("after");
            ui.add(egui::DragValue::new(&mut session.onion.after).range(0..=MAX_ONION_RANGE));
        });

        ui.separator();
        ui.label("Zoom");
        ui.add(
            egui::Slider::new(
                &mut session.timeline.cell_width,
                TimelineView::MIN_CELL_WIDTH..=TimelineView::MAX_CELL_WIDTH,
            )
            .show_value(false),
        )
        .on_hover_text("Or Cmd/Ctrl+scroll over the frames");
    });
}

/// A button-sized icon button laid out inline. Returns whether it was clicked.
fn icon_button(ui: &mut egui::Ui, icon: IconFn, enabled: bool, tip: &str) -> bool {
    let size = ui.spacing().interact_size.y;
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(size + 4.0, size), sense);
    let visuals = ui.visuals();
    let style = if enabled {
        visuals.widgets.style(&response)
    } else {
        &visuals.widgets.inactive
    };
    ui.painter()
        .rect_filled(rect, style.corner_radius, style.weak_bg_fill);
    let color = if enabled {
        style.fg_stroke.color
    } else {
        visuals.weak_text_color().gamma_multiply(0.5)
    };
    let side = (size - 4.0).max(8.0);
    icon(
        &ui.painter_at(rect),
        Rect::from_center_size(rect.center(), vec2(side, side)),
        color,
    );
    let response = response.on_hover_text(tip);
    enabled && response.clicked()
}

/// A `DragValue` that only reports a new value once the user lets go of it
/// (drag released or text field left), so one edit is one undo step.
fn committed_drag_value(
    ui: &mut egui::Ui,
    salt: &str,
    current: u32,
    range: std::ops::RangeInclusive<u32>,
) -> Option<u32> {
    let id = ui.scope_id().with(salt);
    let mut value = ui.data(|d| d.get_temp::<u32>(id)).unwrap_or(current);
    let response = ui.add(egui::DragValue::new(&mut value).range(range));
    if response.dragged() || response.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, value));
        None
    } else {
        ui.data_mut(|d| d.remove::<u32>(id));
        (value != current).then_some(value)
    }
}

/// What a timeline row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum RowKind {
    /// A layer and its track.
    Layer(LayerId),
    /// A group's header, above its layers.
    Group(GroupId),
}

/// One timeline row: a layer, or a group header.
pub(super) struct Row {
    pub(super) kind: RowKind,
    pub(super) name: String,
    /// The row's own visibility flag.
    pub(super) visible: bool,
    /// Whether it is actually shown (its group is visible too).
    pub(super) shown: bool,
    pub(super) locked: bool,
    /// Layers only; groups are Normal.
    pub(super) blend: BlendMode,
    pub(super) opacity: f32,
    /// A layer's group (for the indent), or a header's own group.
    pub(super) group: Option<GroupId>,
    /// Group headers: whether the group is folded.
    pub(super) collapsed: bool,
    /// Whether it is an audio layer (ADR-020).
    pub(super) audio: bool,
}

impl Row {
    /// The layer of a layer row.
    pub(super) fn layer(&self) -> Option<LayerId> {
        match self.kind {
            RowKind::Layer(id) => Some(id),
            RowKind::Group(_) => None,
        }
    }
}

/// Rows top of the stack first: each group's header goes right above its
/// layers, which are left out while the group is collapsed.
pub(super) fn rows(doc: &Document, collapsed: &BTreeSet<GroupId>) -> Vec<Row> {
    let stack = doc.layers();
    let mut rows = Vec::new();
    let mut current = None;
    for l in stack.iter().rev() {
        if l.group != current
            && let Some(g) = l.group.and_then(|g| stack.group(g))
        {
            rows.push(Row {
                kind: RowKind::Group(g.id),
                name: g.name.clone(),
                visible: g.visible,
                shown: g.visible,
                locked: g.locked,
                blend: BlendMode::Normal,
                opacity: g.opacity,
                group: Some(g.id),
                collapsed: collapsed.contains(&g.id),
                audio: false,
            });
        }
        current = l.group;
        if l.group.is_some_and(|g| collapsed.contains(&g)) {
            continue;
        }
        rows.push(Row {
            kind: RowKind::Layer(l.id),
            name: l.name.clone(),
            visible: l.visible,
            shown: stack.is_visible(l.id),
            locked: l.locked,
            blend: l.blend,
            opacity: l.opacity,
            group: l.group,
            collapsed: false,
            audio: l.is_audio(),
        });
    }
    rows
}

/// Layer controls on the left, frames on the right.
pub(super) fn grid(ui: &mut egui::Ui, session: &mut Session) {
    let rows = rows(&session.document, &session.timeline.collapsed_groups);
    let height = RULER_HEIGHT + rows.len() as f32 * ROW_HEIGHT;
    egui::ScrollArea::vertical()
        .id_salt("timeline_vertical")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                super::layers::column(ui, session, &rows, height);
                egui::ScrollArea::horizontal()
                    .id_salt("timeline_horizontal")
                    .auto_shrink([false, true])
                    .show(ui, |ui| frames(ui, session, &rows, height));
            });
        });
}

fn row_at(rows: &[Row], y: f32) -> Option<LayerId> {
    let index = ((y - RULER_HEIGHT) / ROW_HEIGHT).floor();
    if index < 0.0 {
        return None;
    }
    rows.get(index as usize).and_then(Row::layer)
}

fn frames(ui: &mut egui::Ui, session: &mut Session, rows: &[Row], height: f32) {
    let cell = session.timeline.cell_width;
    let count = session.document.frame_count.max(1);
    // While the end handle is dragged past the end, the grid grows with it
    // so the scroll area follows.
    let length_id = ui.scope_id().with("length_drag");
    let preview: Option<u32> = ui.data(|d| d.get_temp(length_id));
    let shown = preview.unwrap_or(count).max(count);
    let (full, response) = ui.allocate_exact_size(
        vec2(shown as f32 * cell + LENGTH_HANDLE, height),
        Sense::click_and_drag(),
    );
    let rect = Rect::from_min_size(full.min, vec2(count as f32 * cell, height));

    // Zoom with Cmd/Ctrl+scroll or pinch.
    if response.hovered() {
        let zoom = ui.input(|i| i.zoom_delta());
        if zoom != 1.0 {
            session.timeline.cell_width =
                (cell * zoom).clamp(TimelineView::MIN_CELL_WIDTH, TimelineView::MAX_CELL_WIDTH);
        }
    }

    let frame_at = |x: f32| {
        FrameIndex(
            ((x - rect.left()) / cell)
                .floor()
                .clamp(0.0, (count - 1) as f32) as u32,
        )
    };
    // The key under `pos`, if its layer can be edited.
    let movable_key = |pos: Pos2| {
        let layer = row_at(rows, pos.y - rect.top())?;
        let frame = frame_at(pos.x);
        let unlocked = session.document.layers().is_editable(layer);
        let keyed = session.document.track(layer)?.key_at(frame).is_some();
        (unlocked && keyed).then_some((layer, frame))
    };

    // The audio clip under `pos`, if its layer can be edited: the layer and
    // the clip's start.
    let movable_clip = |pos: Pos2| {
        let layer = row_at(rows, pos.y - rect.top())?;
        let clip = session.document.layers().get(layer)?.audio_clip()?;
        let range = session.document.clip_range(clip)?;
        let unlocked = session.document.layers().is_editable(layer);
        (unlocked && range.contains(frame_at(pos.x))).then_some((layer, clip.start))
    };

    // Drag inside the frame selection to move it, drag a key to move that
    // key (the drop happens on release), or drag an audio clip to move its
    // start (live, one undo step).
    let selection = session.frame_selection();
    // The frame selection covers the selected layers' rows.
    let selected_layers = session.selected_layers();
    let in_selection = |pos: Pos2| {
        selection.is_some_and(|r| r.contains(frame_at(pos.x)))
            && row_at(rows, pos.y - rect.top()).is_some_and(|l| selected_layers.contains(&l))
    };
    let shift = ui.input(|i| i.modifiers.shift);
    let selection_drag_id = ui.scope_id().with("selection_drag");
    let drag_id = ui.scope_id().with("key_drag");
    let clip_drag_id = ui.scope_id().with("clip_drag");
    if response.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        if !shift && in_selection(origin) {
            ui.data_mut(|d| d.insert_temp(selection_drag_id, frame_at(origin.x)));
        } else if let Some(key) = movable_key(origin) {
            ui.data_mut(|d| d.insert_temp(drag_id, key));
        } else if let Some((layer, start)) = movable_clip(origin) {
            let drag = ClipDrag {
                layer,
                grab: frame_at(origin.x),
                start,
                gesture: ui.ctx().cumulative_pass_nr(),
            };
            ui.data_mut(|d| d.insert_temp(clip_drag_id, drag));
        }
    }
    let clip_drag: Option<ClipDrag> = ui.data(|d| d.get_temp(clip_drag_id));
    let selection_grab: Option<FrameIndex> = ui.data(|d| d.get_temp(selection_drag_id));
    let dragged_key: Option<(LayerId, FrameIndex)> = ui.data(|d| d.get_temp(drag_id));
    let drop_target = ui
        .input(|i| i.pointer.latest_pos())
        .map(|pos| frame_at(pos.x));
    // Where the selection would start if dropped now, kept inside the animation.
    let selection_to = selection_grab
        .zip(selection)
        .zip(drop_target)
        .map(|((grab, range), at)| {
            let start = i64::from(range.start.0) + i64::from(at.0) - i64::from(grab.0);
            FrameIndex(start.clamp(0, i64::from(count - range.len())) as u32)
        });
    if dragged_key.is_some() || selection_grab.is_some() || clip_drag.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if response.hover_pos().is_some_and(|pos| {
        in_selection(pos) || movable_key(pos).is_some() || movable_clip(pos).is_some()
    }) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    let right_clicked_key = response
        .secondary_clicked()
        .then(|| response.interact_pointer_pos().and_then(movable_key))
        .flatten();
    if let Some(drag) = clip_drag {
        if response.drag_stopped() {
            ui.data_mut(|d| d.remove::<ClipDrag>(clip_drag_id));
        } else if let Some(at) = drop_target {
            drag.follow(session, at, count);
        }
    }
    if response.drag_stopped() && selection_grab.is_some() {
        ui.data_mut(|d| d.remove::<FrameIndex>(selection_drag_id));
        if let Some(to) = selection_to {
            session.move_frame_selection(to);
        }
    }
    if response.drag_stopped()
        && let Some((layer, from)) = dragged_key
    {
        ui.data_mut(|d| d.remove::<(LayerId, FrameIndex)>(drag_id));
        if let Some(to) = drop_target
            && to != from
            && session
                .execute(Box::new(MoveKeyframe::new(layer, from, to)))
                .is_ok()
        {
            session.set_frame(to);
        }
    }

    // The tag strip: click a tag to select its frames, double-click to
    // rename it, right-click for more.
    let tags = session.document.tags.clone();
    let tag_under = |pos: Pos2| {
        (pos.y - rect.top() < TAG_HEIGHT)
            .then(|| tags.iter().rposition(|t| t.range.contains(frame_at(pos.x))))
            .flatten()
    };
    let clicked_tag = response
        .interact_pointer_pos()
        .and_then(tag_under)
        .filter(|_| response.clicked() || response.double_clicked());
    if let Some(index) = clicked_tag {
        session.select_tag(index);
        if response.double_clicked() {
            start_tag_rename(ui, session, index);
        }
    }
    let menu_tag_id = ui.scope_id().with("menu_tag");
    let menu_key_id = ui.scope_id().with("menu_key");
    let menu_cell_id = ui.scope_id().with("menu_cell");
    if response.secondary_clicked() {
        let pos = response.interact_pointer_pos();
        let tag = pos.and_then(tag_under);
        let cell = pos.and_then(|p| Some((row_at(rows, p.y - rect.top())?, frame_at(p.x))));
        ui.data_mut(|d| {
            d.insert_temp(menu_tag_id, tag);
            d.insert_temp(menu_key_id, right_clicked_key);
            d.insert_temp(menu_cell_id, cell);
        });
    }
    response.context_menu(|ui| {
        match ui
            .data(|d| d.get_temp::<Option<usize>>(menu_tag_id))
            .flatten()
        {
            Some(index) => {
                if ui.button("Rename Tag…").clicked() {
                    start_tag_rename(ui, session, index);
                }
                if ui.button("Select Frames").clicked() {
                    session.select_tag(index);
                }
                if ui.button("Remove Tag").clicked() {
                    session.remove_tag(index);
                }
            }
            None => {
                if let Some((layer, frame)) = ui
                    .data(|d| d.get_temp::<Option<(LayerId, FrameIndex)>>(menu_key_id))
                    .flatten()
                    && ui.button("Remove Key").clicked()
                {
                    let _ = session.execute(Box::new(RemoveKeyframe::new(layer, frame)));
                }
                let label = if session.frame_selection().is_some() {
                    "Tag Selected Frames"
                } else {
                    "Tag This Frame"
                };
                if ui.button(label).clicked()
                    && let Some(index) = session.add_tag()
                {
                    start_tag_rename(ui, session, index);
                }
                let cell = ui
                    .data(|d| d.get_temp::<Option<(LayerId, FrameIndex)>>(menu_cell_id))
                    .flatten();
                if let Some((layer, frame)) = cell
                    && ui
                        .add_enabled(session.can_paste(), egui::Button::new("Paste"))
                        .on_hover_text(
                            "Paste the copied canvas items on this frame and layer, where \
                             they were",
                        )
                        .clicked()
                {
                    session.paste_at(layer, frame);
                }
                ui.separator();
                if ui
                    .button("Convert to Object")
                    .on_hover_text(
                        "Turn the selected layers and frames (all frames when none are \
                         selected) into an object in the Library",
                    )
                    .clicked()
                {
                    session.convert_layers_to_object();
                }
                tween_menu(ui, session, cell);
            }
        }
    });

    // Press or drag in the ruler scrubs. On the cells a press moves the
    // playhead and clears the frame selection, Shift+press selects from the
    // playhead, and dragging selects from the pressed frame. Pressing on a row
    // also makes its layer active (keeping the layer selection if it is part
    // of it).
    if dragged_key.is_none()
        && selection_grab.is_none()
        && clip_drag.is_none()
        && response.is_pointer_button_down_on()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let frame = frame_at(pos.x);
        let in_ruler = ui
            .input(|i| i.pointer.press_origin())
            .is_some_and(|o| o.y - rect.top() < RULER_HEIGHT);
        let pressed = ui.input(|i| i.pointer.primary_pressed());
        if session.playback.is_playing() {
            session.playback.pause();
        }
        if in_ruler {
            if clicked_tag.is_some() {
                // Handled above.
            } else if session.current_frame() != frame {
                session.set_frame(frame);
            }
        } else if pressed && shift {
            // Shift+click: a block from the playhead and active layer.
            session.select_frames_to(frame);
            if let Some(row) = row_at(rows, pos.y - rect.top()) {
                session.select_layer_range(row);
            }
        } else if pressed && in_selection(pos) {
            // Maybe the start of a selection move; a click clears it below.
            if let Some(row) = row_at(rows, pos.y - rect.top()) {
                session.set_active_layer(row);
            }
        } else if pressed {
            session.clear_frame_selection();
            session.set_frame(frame);
            if let Some(row) = row_at(rows, pos.y - rect.top()) {
                session.select_layer(row);
            }
        } else if response.dragged() {
            // Dragging selects a block: frames from the playhead and layers
            // from the active one (where the drag started) to the pointer.
            session.select_frames_to(frame);
            if let Some(row) = row_at(rows, pos.y - rect.top()) {
                session.select_layer_range(row);
            }
        }
    }
    if response.clicked()
        && !shift
        && let Some(pos) = response.interact_pointer_pos()
        && in_selection(pos)
    {
        session.clear_frame_selection();
        session.set_frame(frame_at(pos.x));
    }
    if response.hovered() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        session.clear_frame_selection();
    }

    paint_frames(ui, session, rows, rect, cell, count);
    sound_drop(ui, session, &response, rect, cell, frame_at);
    if response.dragged()
        && let Some((layer, from)) = dragged_key
        && let Some(to) = drop_target
    {
        paint_key_drop(ui, session, rows, rect, cell, (layer, from), to);
    }
    if response.dragged()
        && let Some(range) = selection
        && let Some(to) = selection_to
    {
        let blocked = MoveLayerFrames::new(session.selected_layers(), range, to)
            .blocked_by(&session.document)
            .is_some();
        let color = if blocked {
            Color32::from_rgb(230, 70, 60)
        } else {
            ui.visuals().selection.stroke.color
        };
        let x = rect.left() + to.0 as f32 * cell;
        let painter = ui.painter_at(rect);
        for top in selected_row_tops(session, rows, rect) {
            painter.rect_stroke(
                Rect::from_x_y_ranges(x..=x + range.len() as f32 * cell, top..=top + ROW_HEIGHT),
                2.0,
                Stroke::new(2.0, color),
                StrokeKind::Inside,
            );
        }
    }

    // An object dragged from the library: outline the cell it would land
    // on, and place it there on release (ADR-018).
    let cell_at = |pos: Pos2| Some((row_at(rows, pos.y - rect.top())?, frame_at(pos.x)));
    let pointer = ui.input(|i| i.pointer.latest_pos());
    if let Some(drag) = response.dnd_release_payload::<ObjectDrag>() {
        if let Some((layer, frame)) = pointer.and_then(cell_at) {
            session.place_symbol_at(drag.0, layer, frame);
        }
    } else if response.dnd_hover_payload::<ObjectDrag>().is_some()
        && let Some((layer, frame)) = pointer.and_then(cell_at)
        && let Some(i) = rows.iter().position(|r| r.layer() == Some(layer))
    {
        // Objects can't go on an audio layer (ADR-020).
        let editable = session.document.layers().is_editable(layer) && !rows[i].audio;
        ui.ctx().set_cursor_icon(if editable {
            egui::CursorIcon::Copy
        } else {
            egui::CursorIcon::NoDrop
        });
        let top = rect.top() + RULER_HEIGHT + i as f32 * ROW_HEIGHT;
        let x = rect.left() + frame.0 as f32 * cell;
        let color = if editable {
            INSTANCE_TINT
        } else {
            Color32::from_rgb(230, 70, 60)
        };
        ui.painter_at(rect).rect_stroke(
            Rect::from_x_y_ranges(x..=x + cell, top..=top + ROW_HEIGHT),
            2.0,
            Stroke::new(2.0, color),
            StrokeKind::Inside,
        );
    }

    length_handle(ui, session, full, rect, cell, length_id);

    // Keep the playhead in view when it moves (playback, stepping).
    let current = session.current_frame();
    let last_id = ui.scope_id().with("last_playhead");
    let moved = ui.data(|d| d.get_temp::<FrameIndex>(last_id)) != Some(current);
    ui.data_mut(|d| d.insert_temp(last_id, current));
    let x = rect.left() + (current.0 as f32 + 0.5) * cell;
    let clip = ui.clip_rect();
    if moved && !(clip.left() + cell..=clip.right() - cell).contains(&x) {
        let y = clip.center().y;
        ui.scroll_to_rect(
            Rect::from_x_y_ranges(x - cell..=x + cell, y..=y + 1.0),
            Some(egui::Align::Center),
        );
    }
}

/// The top of each selected layer's row in the frame grid.
fn selected_row_tops<'a>(
    session: &'a Session,
    rows: &'a [Row],
    rect: Rect,
) -> impl Iterator<Item = f32> + 'a {
    rows.iter().enumerate().filter_map(move |(i, row)| {
        let selected = row.layer().is_some_and(|l| session.is_layer_selected(l));
        selected.then(|| rect.top() + RULER_HEIGHT + i as f32 * ROW_HEIGHT)
    })
}

/// The handle at the end of the frames: drag it right to make the
/// animation longer, or left to make it shorter. The new length is shown
/// while dragging and set on release, as one undo step.
fn length_handle(
    ui: &mut egui::Ui,
    session: &mut Session,
    full: Rect,
    rect: Rect,
    cell: f32,
    length_id: egui::Id,
) {
    let count = session.document.frame_count.max(1);
    let area = Rect::from_x_y_ranges(
        rect.right() - 4.0..=rect.right() + LENGTH_HANDLE,
        full.y_range(),
    );
    let handle = ui
        .interact(area, length_id.with("handle"), Sense::drag())
        .on_hover_text("Drag to change the animation length");
    let length_at = |x: f32| {
        ((x - rect.left()) / cell)
            .round()
            .clamp(1.0, MAX_FRAMES as f32) as u32
    };
    let mut preview: Option<u32> = ui.data(|d| d.get_temp(length_id));
    if handle.dragged()
        && let Some(pos) = handle.interact_pointer_pos()
    {
        let length = length_at(pos.x);
        preview = Some(length);
        ui.data_mut(|d| d.insert_temp(length_id, length));
        session.playback.pause();
        // Keep the dragged end in view.
        let y = ui.clip_rect().center().y;
        ui.scroll_to_rect(
            Rect::from_x_y_ranges(pos.x - cell..=pos.x + cell, y..=y + 1.0),
            None,
        );
    }
    if handle.drag_stopped() {
        ui.data_mut(|d| d.remove::<u32>(length_id));
        if let Some(length) = preview.take()
            && length != count
            && session
                .execute(Box::new(SetFrameCount::new(length)))
                .is_ok()
        {
            let frame = session.current_frame();
            session.set_frame(frame);
            session.set_status(format!("Animation length: {length} frames"));
        }
    }
    if handle.hovered() || handle.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    let visuals = ui.visuals();
    let painter = ui.painter_at(full);
    let active = handle.hovered() || handle.dragged();
    let color = if active {
        visuals.selection.stroke.color
    } else {
        visuals.widgets.noninteractive.fg_stroke.color
    };
    let end = preview.unwrap_or(count);
    let x = rect.left() + end as f32 * cell;
    if let Some(length) = preview {
        let span = if length >= count {
            rect.right()..=x
        } else {
            x..=rect.right()
        };
        let tint = if length >= count {
            visuals.selection.bg_fill.gamma_multiply(0.25)
        } else {
            Color32::from_rgb(230, 70, 60).gamma_multiply(0.25)
        };
        painter.rect_filled(
            Rect::from_x_y_ranges(span, full.top() + RULER_HEIGHT..=full.bottom()),
            0.0,
            tint,
        );
        painter.text(
            Pos2::new(x + 4.0, full.top() + TAG_HEIGHT + NUMBERS_HEIGHT * 0.5),
            egui::Align2::LEFT_CENTER,
            format!("{length} frames"),
            egui::FontId::proportional(10.0),
            visuals.strong_text_color(),
        );
    }
    painter.vline(
        x,
        full.y_range(),
        Stroke::new(if active { 3.0 } else { 2.0 }, color),
    );
    // A grip in the ruler so the handle is easy to find.
    let grip = Rect::from_center_size(
        Pos2::new(x + 3.0, full.top() + TAG_HEIGHT + NUMBERS_HEIGHT * 0.5),
        vec2(6.0, NUMBERS_HEIGHT - 6.0),
    );
    painter.rect_filled(grip, 2.0, color);
}

/// "Create Motion Tween ▸" and "Remove Tween" for the object keys the
/// frame selection (or the right-clicked `cell`) covers (ADR-019). Nothing
/// is shown where no tween can be made or removed.
fn tween_menu(ui: &mut egui::Ui, session: &mut Session, cell: Option<(LayerId, FrameIndex)>) {
    let targets = session.tween_targets(cell);
    if !targets.creatable.is_empty() {
        ui.menu_button("Create Motion Tween", |ui| {
            for ease in Interpolation::TWEENS {
                if ui
                    .selectable_label(targets.current == Some(ease), ease.label())
                    .clicked()
                {
                    session.set_tweens(&targets.creatable, Some(ease));
                    ui.close();
                }
            }
        })
        .response
        .on_hover_text("Move the object smoothly from this key to the next one");
    }
    if !targets.removable.is_empty() && ui.button("Remove Tween").clicked() {
        session.set_tweens(&targets.removable, None);
    }
}

/// An audio clip being dragged along its row.
#[derive(Clone, Copy, Debug)]
struct ClipDrag {
    layer: LayerId,
    /// The frame the drag started on.
    grab: FrameIndex,
    /// The clip's start when the drag began.
    start: FrameIndex,
    /// Every move of one drag merges into one undo step.
    gesture: u64,
}

impl ClipDrag {
    /// Moves the clip so it starts as far from `start` as the pointer (at
    /// frame `at`) is from where it was grabbed, kept inside the animation.
    fn follow(self, session: &mut Session, at: FrameIndex, count: u32) {
        let to = i64::from(self.start.0) + i64::from(at.0) - i64::from(self.grab.0);
        let to = FrameIndex(to.clamp(0, i64::from(count.saturating_sub(1))) as u32);
        let Some(clip) = session
            .document
            .layers()
            .get(self.layer)
            .and_then(|l| l.audio_clip())
            .copied()
        else {
            return;
        };
        if clip.start != to {
            let moved = document::scene::AudioClip { start: to, ..clip };
            let _ = session.execute(Box::new(
                SetAudioClip::new(self.layer, moved).in_gesture(self.gesture),
            ));
        }
    }
}

/// A library sound dragged over the frames: marks the frame it would start
/// on, and on release puts it on a new audio layer starting there.
fn sound_drop(
    ui: &egui::Ui,
    session: &mut Session,
    response: &egui::Response,
    rect: Rect,
    cell: f32,
    frame_at: impl Fn(f32) -> FrameIndex,
) {
    let is_sound = |drag: &AssetDrag| {
        session
            .document
            .asset(drag.0)
            .is_some_and(|a| a.audio().is_some())
    };
    if let Some(drag) = response.dnd_hover_payload::<AssetDrag>()
        && is_sound(&drag)
        && let Some(pos) = response.hover_pos()
    {
        let x = rect.left() + frame_at(pos.x).0 as f32 * cell;
        let color = ui.visuals().selection.stroke.color;
        let painter = ui.painter_at(rect);
        painter.vline(x, rect.top()..=rect.bottom(), Stroke::new(2.0, color));
        painter.rect_stroke(
            Rect::from_x_y_ranges(x..=x + cell, rect.top()..=rect.top() + RULER_HEIGHT),
            2.0,
            Stroke::new(2.0, color),
            StrokeKind::Inside,
        );
        ui.ctx().set_cursor_icon(egui::CursorIcon::Copy);
    }
    if let Some(drag) = response.dnd_release_payload::<AssetDrag>()
        && is_sound(&drag)
        && let Some(pos) = response.hover_pos().or(response.interact_pointer_pos())
    {
        session.place_audio(drag.0, frame_at(pos.x));
    }
}

/// Draws an audio layer's clip in its row: a bar from its start for its
/// length, with the sound's waveform, drawn only for frames `first..last`.
#[allow(clippy::too_many_arguments)]
fn paint_clip(
    painter: &egui::Painter,
    visuals: &egui::Visuals,
    doc: &Document,
    layer: LayerId,
    (rect, top): (Rect, f32),
    cell: f32,
    (first, last): (u32, u32),
    alpha: f32,
) {
    let Some(clip) = doc.layers().get(layer).and_then(|l| l.audio_clip()) else {
        return;
    };
    let (Some(range), Some(sound)) = (doc.clip_range(clip), doc.clip_sound(clip)) else {
        return;
    };
    let col_x = |f: u32| rect.left() + f as f32 * cell;
    let bar = Rect::from_x_y_ranges(
        col_x(range.start.0) + 1.0..=col_x(range.end.0) - 1.0,
        top + 3.0..=top + ROW_HEIGHT - 3.0,
    );
    let accent = visuals.selection.bg_fill;
    painter.rect_filled(bar, 3.0, accent.gamma_multiply(0.3 * alpha));
    painter.rect_stroke(
        bar,
        3.0,
        Stroke::new(1.0, accent.gamma_multiply(alpha)),
        StrokeKind::Inside,
    );
    // One line per two points, as tall as the loudest sample under it.
    let from = range.start.0.max(first);
    let to = range.end.0.min(last.saturating_add(1));
    if from >= to {
        return;
    }
    let secs_per_point = doc.fps.frame_duration_secs() / f64::from(cell);
    let wave = visuals.strong_text_color().gamma_multiply(0.8 * alpha);
    let half = (bar.height() * 0.5 - 1.0).max(1.0);
    let mut x = col_x(from).max(bar.left());
    let end = col_x(to).min(bar.right());
    while x < end {
        let secs = f64::from(x - col_x(range.start.0)) * secs_per_point;
        let peak = sound.peak_between(secs, secs + secs_per_point * 2.0);
        let h = (peak * half).max(0.5);
        painter.vline(
            x,
            bar.center().y - h..=bar.center().y + h,
            Stroke::new(1.0, wave),
        );
        x += 2.0;
    }
}

/// Outlines where a dragged key will land: accent if free, red if taken.
fn paint_key_drop(
    ui: &egui::Ui,
    session: &Session,
    rows: &[Row],
    rect: Rect,
    cell: f32,
    (layer, from): (LayerId, FrameIndex),
    to: FrameIndex,
) {
    let Some(i) = rows.iter().position(|r| r.layer() == Some(layer)) else {
        return;
    };
    let taken = to != from
        && session
            .document
            .track(layer)
            .is_some_and(|t| t.key_at(to).is_some());
    let color = if taken {
        Color32::from_rgb(230, 70, 60)
    } else {
        ui.visuals().selection.stroke.color
    };
    let top = rect.top() + RULER_HEIGHT + i as f32 * ROW_HEIGHT;
    let x = rect.left() + to.0 as f32 * cell;
    ui.painter_at(rect).rect_stroke(
        Rect::from_x_y_ranges(x..=x + cell, top..=top + ROW_HEIGHT),
        2.0,
        Stroke::new(2.0, color),
        StrokeKind::Inside,
    );
}

fn paint_frames(ui: &egui::Ui, session: &Session, rows: &[Row], rect: Rect, cell: f32, count: u32) {
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    let doc = &session.document;
    let current = session.current_frame();
    let clip = ui.clip_rect().intersect(rect);
    let first = (((clip.left() - rect.left()) / cell).floor().max(0.0) as u32).min(count);
    let last = (((clip.right() - rect.left()) / cell).ceil().max(0.0) as u32).min(count);
    let col_x = |f: u32| rect.left() + f as f32 * cell;
    let line = visuals.widgets.noninteractive.bg_stroke;
    let faint = Stroke::new(line.width, line.color.gamma_multiply(0.45));

    painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
    let ruler = Rect::from_min_size(rect.min, vec2(rect.width(), RULER_HEIGHT));
    painter.rect_filled(ruler, 0.0, visuals.faint_bg_color);
    let numbers_y = rect.top() + TAG_HEIGHT + NUMBERS_HEIGHT * 0.5;

    // Tags, later ones on top.
    for tag in &doc.tags {
        let span = col_x(tag.range.start.0.min(count))..=col_x(tag.range.end.0.min(count));
        if span.start() >= span.end() {
            continue;
        }
        let bar = Rect::from_x_y_ranges(span, rect.top() + 2.0..=rect.top() + TAG_HEIGHT - 1.0);
        painter.rect_filled(bar, 3.0, visuals.selection.bg_fill.gamma_multiply(0.6));
        painter.rect_stroke(
            bar,
            3.0,
            Stroke::new(1.0, visuals.selection.stroke.color),
            StrokeKind::Inside,
        );
        let text = painter.layout_no_wrap(
            tag.name.clone(),
            egui::FontId::proportional(10.0),
            visuals.strong_text_color(),
        );
        ui.painter_at(bar.shrink2(vec2(3.0, 0.0))).galley(
            Pos2::new(bar.left() + 4.0, bar.center().y - text.size().y * 0.5),
            text,
            visuals.strong_text_color(),
        );
    }

    // Every fifth column is shaded, every column gets a faint line.
    let label_every = if cell >= 14.0 {
        5
    } else if cell >= 8.0 {
        10
    } else {
        20
    };
    for f in first..last {
        let n = f + 1;
        let x = col_x(f);
        if n % 5 == 0 {
            painter.rect_filled(
                Rect::from_x_y_ranges(x..=x + cell, rect.top() + RULER_HEIGHT..=rect.bottom()),
                0.0,
                visuals.faint_bg_color,
            );
        }
        if cell >= 8.0 || n % 5 == 1 {
            painter.vline(x, rect.top() + RULER_HEIGHT..=rect.bottom(), faint);
        }
        if n == 1 || n % label_every == 0 {
            painter.text(
                Pos2::new(x + 2.0, numbers_y),
                egui::Align2::LEFT_CENTER,
                n.to_string(),
                egui::FontId::proportional(10.0),
                visuals.weak_text_color(),
            );
        }
    }

    let accent = visuals.selection.bg_fill;
    for (i, row) in rows.iter().enumerate() {
        let top = rect.top() + RULER_HEIGHT + i as f32 * ROW_HEIGHT;
        let row_rect = Rect::from_x_y_ranges(rect.x_range(), top..=top + ROW_HEIGHT);
        let id = match row.kind {
            RowKind::Layer(id) => id,
            RowKind::Group(_) => {
                painter.rect_filled(row_rect, 0.0, visuals.faint_bg_color);
                painter.hline(rect.x_range(), top + ROW_HEIGHT, line);
                continue;
            }
        };
        if session.state.active_layer == Some(id) {
            painter.rect_filled(row_rect, 0.0, accent.gamma_multiply(0.18));
        } else if session.is_layer_selected(id) {
            painter.rect_filled(row_rect, 0.0, accent.gamma_multiply(0.1));
        }
        painter.hline(rect.x_range(), top + ROW_HEIGHT, line);
        let alpha = if row.shown { 1.0 } else { 0.4 };
        if row.audio {
            paint_clip(
                &painter,
                visuals,
                doc,
                id,
                (rect, top),
                cell,
                (first, last),
                alpha,
            );
            continue;
        }
        let Some(track) = doc.track(id) else {
            continue;
        };
        for (frame, key) in track.keys() {
            if frame.0 >= count || frame.0 > last {
                continue;
            }
            let center = Pos2::new(col_x(frame.0) + cell * 0.5, top + ROW_HEIGHT * 0.5);
            let radius = (cell.min(ROW_HEIGHT) * 0.3).clamp(2.0, 5.0);
            match key {
                Keyframe::Drawing(id) => {
                    let has_strokes = doc
                        .drawing(*id)
                        .and_then(|d| d.as_vector())
                        .is_some_and(|d| !d.is_empty());
                    if let Some(range) = track.exposure_range(frame, count) {
                        let bar = Rect::from_x_y_ranges(
                            col_x(range.start.0) + 1.0..=col_x(range.end.0) - 1.0,
                            top + 4.0..=top + ROW_HEIGHT - 4.0,
                        );
                        let fill = if has_strokes {
                            accent.gamma_multiply(0.55 * alpha)
                        } else {
                            accent.gamma_multiply(0.22 * alpha)
                        };
                        painter.rect_filled(bar, 3.0, fill);
                    }
                    let dot = visuals.strong_text_color().gamma_multiply(alpha);
                    if has_strokes {
                        painter.circle_filled(center, radius, dot);
                    } else {
                        painter.circle_filled(center, radius, dot.gamma_multiply(0.35));
                        painter.circle_stroke(center, radius, Stroke::new(1.0, dot));
                    }
                }
                Keyframe::Empty => {
                    let color = visuals.text_color().gamma_multiply(alpha);
                    painter.circle_stroke(center, radius, Stroke::new(1.2, color));
                }
                Keyframe::Instance(instance) => {
                    let tint = INSTANCE_TINT.gamma_multiply(alpha);
                    let tween = track.tween_from(frame);
                    if let Some(range) = track.exposure_range(frame, count) {
                        let bar = Rect::from_x_y_ranges(
                            col_x(range.start.0) + 1.0..=col_x(range.end.0) - 1.0,
                            top + 4.0..=top + ROW_HEIGHT - 4.0,
                        );
                        let band = if tween.is_some() {
                            TWEEN_TINT
                        } else {
                            INSTANCE_TINT
                        };
                        painter.rect_filled(bar, 3.0, band.gamma_multiply(0.45 * alpha));
                        // A motion tween: an arrow from this key to the next.
                        // A tween waiting for a partner key: a dashed line.
                        let y = top + ROW_HEIGHT * 0.5;
                        let from = center.x + radius + 3.0;
                        let to = col_x(range.end.0) - 4.0;
                        let arrow = visuals.strong_text_color().gamma_multiply(0.8 * alpha);
                        if tween.is_some() && to - from > 6.0 {
                            painter.hline(from..=to, y, (1.2, arrow));
                            let head = 4.0;
                            painter.line_segment(
                                [Pos2::new(to - head, y - head * 0.8), Pos2::new(to, y)],
                                (1.2, arrow),
                            );
                            painter.line_segment(
                                [Pos2::new(to - head, y + head * 0.8), Pos2::new(to, y)],
                                (1.2, arrow),
                            );
                        } else if instance.tween.is_some() && to > from {
                            painter.extend(egui::Shape::dashed_line(
                                &[Pos2::new(from, y), Pos2::new(to, y)],
                                Stroke::new(1.0, arrow.gamma_multiply(0.6)),
                                3.0,
                                3.0,
                            ));
                        }
                        // A tick where each loop of the object starts over.
                        let length = doc.symbol(instance.symbol).map_or(0, |s| s.frame_count);
                        let tick = visuals.strong_text_color().gamma_multiply(0.6 * alpha);
                        for f in range.start.0 + 1..range.end.0.min(last + 1) {
                            if instance.inner_frame(f - range.start.0, length)
                                == Some(FrameIndex(0))
                            {
                                let x = col_x(f);
                                painter.vline(x, top + 5.0..=top + ROW_HEIGHT - 5.0, (1.5, tick));
                            }
                        }
                    }
                    // A diamond marks an object key.
                    let r = radius + 1.0;
                    let diamond = vec![
                        center + Vec2::new(0.0, -r),
                        center + Vec2::new(r, 0.0),
                        center + Vec2::new(0.0, r),
                        center + Vec2::new(-r, 0.0),
                    ];
                    painter.add(egui::Shape::convex_polygon(
                        diamond,
                        visuals.strong_text_color().gamma_multiply(alpha),
                        Stroke::new(1.0, tint),
                    ));
                }
            }
        }
    }

    // Selected frames.
    if let Some(sel) = session.frame_selection() {
        let span = col_x(sel.start.0)..=col_x(sel.end.0.min(count));
        // Mark the frames in the ruler, and the block on each selected row.
        painter.rect_filled(
            Rect::from_x_y_ranges(
                span.clone(),
                rect.top() + TAG_HEIGHT..=rect.top() + RULER_HEIGHT,
            ),
            0.0,
            accent.gamma_multiply(0.25),
        );
        for top in selected_row_tops(session, rows, rect) {
            let block = Rect::from_x_y_ranges(span.clone(), top..=top + ROW_HEIGHT);
            painter.rect_filled(block, 0.0, accent.gamma_multiply(0.25));
            painter.rect_stroke(
                block,
                0.0,
                Stroke::new(1.5, visuals.selection.stroke.color),
                StrokeKind::Inside,
            );
        }
    }

    // Playhead.
    let palette = crate::theme::palette(visuals.dark_mode);
    let playhead = palette.accent;
    let x0 = col_x(current.0);
    painter.rect_stroke(
        Rect::from_x_y_ranges(x0..=x0 + cell, rect.top()..=rect.bottom()),
        0.0,
        Stroke::new(1.0, playhead.gamma_multiply(0.6)),
        StrokeKind::Inside,
    );
    painter.rect_filled(
        Rect::from_x_y_ranges(
            x0..=x0 + cell.max(18.0),
            rect.top() + TAG_HEIGHT..=rect.top() + RULER_HEIGHT,
        ),
        2.0,
        playhead,
    );
    painter.text(
        Pos2::new(x0 + 2.0, numbers_y),
        egui::Align2::LEFT_CENTER,
        current.display_number().to_string(),
        egui::FontId::proportional(10.0),
        palette.accent_text,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::{AudioData, AudioFormat};

    #[test]
    fn dragging_a_clip_moves_its_start_in_one_undo_step() {
        let mut s = Session::new();
        let sound = AudioData {
            format: AudioFormat::Wav,
            sample_rate: 8000,
            channels: 1,
            duration: 0.5,
            peaks: Vec::new(),
            bytes: std::sync::Arc::from(vec![1u8]),
        };
        let layer = s.import_audio("song.wav", sound).unwrap();
        let steps = s.history.undo_len();
        let drag = ClipDrag {
            layer,
            grab: FrameIndex(2),
            start: FrameIndex(0),
            gesture: 9,
        };
        let count = s.document.frame_count;
        let start = |s: &Session| {
            s.document
                .layers()
                .get(layer)
                .unwrap()
                .audio_clip()
                .unwrap()
                .start
        };
        drag.follow(&mut s, FrameIndex(5), count);
        assert_eq!(start(&s), FrameIndex(3));
        drag.follow(&mut s, FrameIndex(0), count);
        assert_eq!(start(&s), FrameIndex(0), "kept inside the animation");
        drag.follow(&mut s, FrameIndex(999), count);
        assert_eq!(start(&s), FrameIndex(count - 1));
        assert_eq!(s.history.undo_len(), steps + 1);
        s.undo();
        assert_eq!(start(&s), FrameIndex(0));
        let rows = rows(&s.document, &BTreeSet::new());
        assert!(rows.iter().any(|r| r.audio && r.layer() == Some(layer)));
    }

    /// Drags the end handle from the end of a 24-frame animation (10 px
    /// cells) to `to_x`, one egui pass per step, and returns the session.
    fn drag_length_handle(to_x: f32) -> Session {
        let mut s = Session::new();
        assert_eq!(s.document.frame_count, 24);
        let ctx = egui::Context::default();
        let start = Pos2::new(241.0, 60.0);
        let end = Pos2::new(to_x, 60.0);
        let steps = [
            vec![egui::Event::PointerMoved(start)],
            vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            vec![egui::Event::PointerMoved(Pos2::new(
                (start.x + end.x) / 2.0,
                60.0,
            ))],
            vec![egui::Event::PointerMoved(end)],
            vec![egui::Event::PointerButton {
                pos: end,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            vec![],
        ];
        for events in steps {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 400.0))),
                events,
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                let count = s.document.frame_count as f32;
                let rect = Rect::from_min_size(Pos2::ZERO, vec2(count * 10.0, 200.0));
                let full = rect.with_max_x(rect.right() + LENGTH_HANDLE);
                let id = ui.scope_id().with("len");
                length_handle(ui, &mut s, full, rect, 10.0, id);
            })
            .drop_without_applying_deltas();
        }
        s
    }

    #[test]
    fn dragging_the_end_handle_sets_the_length_in_one_undo_step() {
        let mut s = drag_length_handle(400.0);
        assert_eq!(s.document.frame_count, 40);
        assert_eq!(s.history.undo_label(), Some("Set Frame Count"));
        s.undo();
        assert_eq!(s.document.frame_count, 24);

        let s = drag_length_handle(100.0);
        assert_eq!(s.document.frame_count, 10, "dragging left shortens");
        let s = drag_length_handle(-50.0);
        assert_eq!(s.document.frame_count, 1, "never below one frame");
    }
}
