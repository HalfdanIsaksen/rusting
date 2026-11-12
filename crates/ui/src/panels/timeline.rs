//! The timeline panel: playback bar, key buttons, onion-skin settings and
//! the frame grid (one row per layer, keys and exposures, scrubbing). The
//! layer controls on the left of the grid live in [`super::layers`].

use super::shortcuts;
use super::status::fps_text;
use crate::icons::{self, IconFn};
use crate::onion::MAX_ONION_RANGE;
use crate::session::{Session, TimelineView};
use document::Document;
use document::animation_core::{Fps, FrameIndex, LayerId};
use document::drawing::DrawingContent;
use document::scene::BlendMode;
use document::timeline::Keyframe;
use editor::commands::{
    AddKeyframe, ClearFrame, KeyframeKind, MoveFrames, MoveKeyframe, RemoveKeyframe, SetFps,
    SetFrameCount,
};
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, vec2};

/// Height of one layer row.
pub(super) const ROW_HEIGHT: f32 = 22.0;
/// Height of the tag strip at the top of the ruler.
const TAG_HEIGHT: f32 = 16.0;
/// Height of the frame numbers under the tag strip.
pub(super) const NUMBERS_HEIGHT: f32 = 20.0;
/// Height of the ruler: tags plus frame numbers (and the layer column's header).
pub(super) const RULER_HEIGHT: f32 = TAG_HEIGHT + NUMBERS_HEIGHT;
const MAX_FPS: u32 = 120;
const MAX_FRAMES: u32 = 100_000;

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
            "Duplicate frame ({}): insert a copy of this frame right after it, on every layer",
            ui.ctx().format_shortcut(&shortcuts::DUPLICATE_FRAME)
        );
        if icon_button(
            ui,
            icons::duplicate,
            !session.playback.is_playing(),
            &duplicate_tip,
        ) {
            session.duplicate_frame();
        }
        let drawn = editable
            && layer.is_some_and(|id| {
                let doc = &session.document;
                doc.layers().get(id).is_some_and(|l| l.is_editable())
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

/// One timeline row.
pub(super) struct Row {
    pub(super) id: LayerId,
    pub(super) name: String,
    pub(super) visible: bool,
    pub(super) locked: bool,
    pub(super) blend: BlendMode,
    pub(super) opacity: f32,
}

fn rows(doc: &Document) -> Vec<Row> {
    doc.layers()
        .iter()
        .rev()
        .map(|l| Row {
            id: l.id,
            name: l.name.clone(),
            visible: l.visible,
            locked: l.locked,
            blend: l.blend,
            opacity: l.opacity,
        })
        .collect()
}

/// Layer controls on the left, frames on the right.
fn grid(ui: &mut egui::Ui, session: &mut Session) {
    let rows = rows(&session.document);
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
    rows.get(index as usize).map(|r| r.id)
}

fn frames(ui: &mut egui::Ui, session: &mut Session, rows: &[Row], height: f32) {
    let cell = session.timeline.cell_width;
    let count = session.document.frame_count.max(1);
    let (rect, response) =
        ui.allocate_exact_size(vec2(count as f32 * cell, height), Sense::click_and_drag());

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
        let unlocked = session
            .document
            .layers()
            .get(layer)
            .is_some_and(|l| !l.locked);
        let keyed = session.document.track(layer)?.key_at(frame).is_some();
        (unlocked && keyed).then_some((layer, frame))
    };

    // Drag inside the frame selection to move it, or drag a key to move
    // that key; the drop happens on release.
    let selection = session.frame_selection();
    let in_selection = |pos: Pos2| {
        pos.y - rect.top() >= RULER_HEIGHT && selection.is_some_and(|r| r.contains(frame_at(pos.x)))
    };
    let shift = ui.input(|i| i.modifiers.shift);
    let selection_drag_id = ui.scope_id().with("selection_drag");
    let drag_id = ui.scope_id().with("key_drag");
    if response.drag_started()
        && let Some(origin) = ui.input(|i| i.pointer.press_origin())
    {
        if !shift && in_selection(origin) {
            ui.data_mut(|d| d.insert_temp(selection_drag_id, frame_at(origin.x)));
        } else if let Some(key) = movable_key(origin) {
            ui.data_mut(|d| d.insert_temp(drag_id, key));
        }
    }
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
    if dragged_key.is_some() || selection_grab.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if response
        .hover_pos()
        .is_some_and(|pos| in_selection(pos) || movable_key(pos).is_some())
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    let right_clicked_key = response
        .secondary_clicked()
        .then(|| response.interact_pointer_pos().and_then(movable_key))
        .flatten();
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
    if response.secondary_clicked() {
        let tag = response.interact_pointer_pos().and_then(tag_under);
        ui.data_mut(|d| {
            d.insert_temp(menu_tag_id, tag);
            d.insert_temp(menu_key_id, right_clicked_key);
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
            session.select_frames_to(frame);
        } else if pressed && in_selection(pos) {
            // Maybe the start of a selection move; a click clears it below.
        } else if pressed {
            session.clear_frame_selection();
            session.set_frame(frame);
        } else if response.dragged() {
            session.select_frames_to(frame);
        }
        if pressed
            && !in_ruler
            && let Some(row) = row_at(rows, pos.y - rect.top())
        {
            if session.is_layer_selected(row) {
                session.set_active_layer(row);
            } else {
                session.select_layer(row);
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
        let blocked = MoveFrames::new(range, to)
            .blocked_by(&session.document)
            .is_some();
        let color = if blocked {
            Color32::from_rgb(230, 70, 60)
        } else {
            ui.visuals().selection.stroke.color
        };
        let x = rect.left() + to.0 as f32 * cell;
        ui.painter_at(rect).rect_stroke(
            Rect::from_x_y_ranges(
                x..=x + range.len() as f32 * cell,
                rect.top() + RULER_HEIGHT..=rect.bottom(),
            ),
            2.0,
            Stroke::new(2.0, color),
            StrokeKind::Inside,
        );
    }

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
    let Some(i) = rows.iter().position(|r| r.id == layer) else {
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
        if session.state.active_layer == Some(row.id) {
            painter.rect_filled(row_rect, 0.0, accent.gamma_multiply(0.18));
        } else if session.is_layer_selected(row.id) {
            painter.rect_filled(row_rect, 0.0, accent.gamma_multiply(0.1));
        }
        painter.hline(rect.x_range(), top + ROW_HEIGHT, line);
        let alpha = if row.visible { 1.0 } else { 0.4 };
        let Some(track) = doc.track(row.id) else {
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
            }
        }
    }

    // Selected frames.
    if let Some(sel) = session.frame_selection() {
        let span = col_x(sel.start.0)..=col_x(sel.end.0.min(count));
        painter.rect_filled(
            Rect::from_x_y_ranges(span.clone(), rect.top() + RULER_HEIGHT..=rect.bottom()),
            0.0,
            accent.gamma_multiply(0.25),
        );
        painter.rect_stroke(
            Rect::from_x_y_ranges(span, rect.top()..=rect.bottom()),
            0.0,
            Stroke::new(1.5, visuals.selection.stroke.color),
            StrokeKind::Inside,
        );
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
