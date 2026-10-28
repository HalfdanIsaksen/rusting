//! The timeline panel: playback bar, key buttons, onion-skin settings and
//! the frame grid (one row per layer, keys and exposures, scrubbing).

use super::status::fps_text;
use crate::onion::MAX_ONION_RANGE;
use crate::session::{Session, TimelineView};
use document::Document;
use document::animation_core::{Fps, FrameIndex, LayerId};
use document::timeline::Keyframe;
use editor::commands::{AddKeyframe, KeyframeKind, RemoveKeyframe, SetFps, SetFrameCount};
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, vec2};

const NAME_WIDTH: f32 = 120.0;
const ROW_HEIGHT: f32 = 22.0;
const RULER_HEIGHT: f32 = 20.0;
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
        let key_here = editable && key_here;

        if ui
            .add_enabled(free, egui::Button::new("+ Drawing key"))
            .on_hover_text("New blank drawing at this frame on the active layer")
            .clicked()
            && let Some(id) = layer
        {
            let _ = session.execute(Box::new(AddKeyframe::new(id, frame, KeyframeKind::Drawing)));
        }
        if ui
            .add_enabled(free, egui::Button::new("○ Empty key"))
            .on_hover_text("End the exposure here (show nothing)")
            .clicked()
            && let Some(id) = layer
        {
            let _ = session.execute(Box::new(AddKeyframe::new(id, frame, KeyframeKind::Empty)));
        }
        if ui
            .add_enabled(key_here, egui::Button::new("✖ Remove key"))
            .on_hover_text("Remove the key at this frame on the active layer")
            .clicked()
            && let Some(id) = layer
        {
            let _ = session.execute(Box::new(RemoveKeyframe::new(id, frame)));
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
struct Row {
    id: LayerId,
    name: String,
    visible: bool,
    locked: bool,
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
        })
        .collect()
}

/// Layer names on the left, frames on the right.
fn grid(ui: &mut egui::Ui, session: &mut Session) {
    let rows = rows(&session.document);
    let height = RULER_HEIGHT + rows.len() as f32 * ROW_HEIGHT;
    egui::ScrollArea::vertical()
        .id_salt("timeline_vertical")
        .auto_shrink(false)
        .show(ui, |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                names(ui, session, &rows, height);
                egui::ScrollArea::horizontal()
                    .id_salt("timeline_horizontal")
                    .auto_shrink([false, true])
                    .show(ui, |ui| frames(ui, session, &rows, height));
            });
        });
}

fn names(ui: &mut egui::Ui, session: &mut Session, rows: &[Row], height: f32) {
    let (rect, response) = ui.allocate_exact_size(vec2(NAME_WIDTH, height), Sense::click());
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    let font = egui::FontId::proportional(13.0);
    painter.text(
        Pos2::new(rect.left() + 6.0, rect.top() + RULER_HEIGHT * 0.5),
        egui::Align2::LEFT_CENTER,
        "Layer",
        egui::FontId::proportional(11.0),
        visuals.weak_text_color(),
    );
    for (i, row) in rows.iter().enumerate() {
        let top = rect.top() + RULER_HEIGHT + i as f32 * ROW_HEIGHT;
        let row_rect =
            Rect::from_min_size(Pos2::new(rect.left(), top), vec2(NAME_WIDTH, ROW_HEIGHT));
        if session.state.active_layer == Some(row.id) {
            painter.rect_filled(row_rect, 0.0, visuals.selection.bg_fill.gamma_multiply(0.5));
        }
        let mut color = visuals.text_color();
        if !row.visible {
            color = color.gamma_multiply(0.4);
        }
        let marker = if row.locked { "🔒 " } else { "" };
        painter.text(
            Pos2::new(rect.left() + 6.0, top + ROW_HEIGHT * 0.5),
            egui::Align2::LEFT_CENTER,
            format!("{marker}{}", row.name),
            font.clone(),
            color,
        );
        painter.hline(
            rect.x_range(),
            top + ROW_HEIGHT,
            visuals.widgets.noninteractive.bg_stroke,
        );
    }
    if response.clicked()
        && let Some(pos) = response.interact_pointer_pos()
        && let Some(row) = row_at(rows, pos.y - rect.top())
    {
        session.select_layer(row);
    }
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

    // Scrub: press or drag anywhere; pressing on a row also selects its layer.
    if response.is_pointer_button_down_on()
        && let Some(pos) = response.interact_pointer_pos()
    {
        let frame = ((pos.x - rect.left()) / cell)
            .floor()
            .clamp(0.0, (count - 1) as f32) as u32;
        if session.playback.is_playing() {
            session.playback.pause();
        }
        if session.current_frame() != FrameIndex(frame) {
            session.set_frame(FrameIndex(frame));
        }
        if ui.input(|i| i.pointer.primary_pressed())
            && let Some(row) = row_at(rows, pos.y - rect.top())
        {
            session.select_layer(row);
        }
    }

    paint_frames(ui, session, rows, rect, cell, count);

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
                Pos2::new(x + 2.0, rect.top() + RULER_HEIGHT * 0.5),
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

    // Playhead.
    let playhead = Color32::from_rgb(230, 70, 60);
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
            rect.top()..=rect.top() + RULER_HEIGHT,
        ),
        2.0,
        playhead,
    );
    painter.text(
        Pos2::new(x0 + 2.0, rect.top() + RULER_HEIGHT * 0.5),
        egui::Align2::LEFT_CENTER,
        current.display_number().to_string(),
        egui::FontId::proportional(10.0),
        Color32::WHITE,
    );
}
