//! The "Resize Canvas" dialog: new width and height plus a 3×3 anchor picker.

use crate::session::Session;
use document::CanvasAnchor;
use egui::{Rect, Sense, Stroke, Vec2, vec2};

/// Largest width or height offered. Matches the GPU texture limit that
/// rendering and export can rely on everywhere.
const MAX_SIDE: u32 = 8192;
/// Side of one anchor cell, in points.
const CELL: f32 = 28.0;

/// Shows the dialog while it is open.
pub fn show(ctx: &egui::Context, session: &mut Session) {
    let Some(mut dialog) = session.resize_canvas else {
        return;
    };
    let current = session.document.canvas;
    let mut apply = false;
    let mut cancel = false;

    let response = egui::Modal::new(egui::Id::unique("resize_canvas_dialog")).show(ctx, |ui| {
        ui.set_max_width(360.0);
        ui.heading("Resize Canvas");
        ui.add_space(4.0);
        ui.label(format!(
            "Current size: {} × {} px",
            current.width, current.height
        ));
        ui.add_space(8.0);

        egui::Grid::new("resize_canvas_size")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Width");
                ui.add(
                    egui::DragValue::new(&mut dialog.width)
                        .range(1..=MAX_SIDE)
                        .suffix(" px"),
                );
                ui.end_row();
                ui.label("Height");
                ui.add(
                    egui::DragValue::new(&mut dialog.height)
                        .range(1..=MAX_SIDE)
                        .suffix(" px"),
                );
                ui.end_row();
            });

        ui.add_space(10.0);
        ui.label("Anchor");
        ui.add_space(2.0);
        anchor_picker(ui, &mut dialog.anchor);
        ui.label(
            egui::RichText::new(format!(
                "Artwork stays in place relative to the {} of the canvas.",
                dialog.anchor.label().to_lowercase()
            ))
            .weak(),
        );

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            if ui.button("OK").clicked() {
                apply = true;
            }
            if ui.button("Cancel").clicked() {
                cancel = true;
            }
        });
    });

    let (escape, enter) = ctx.input(|i| {
        (
            i.key_pressed(egui::Key::Escape),
            i.key_pressed(egui::Key::Enter),
        )
    });
    // Enter applies unless a number field is being typed into; there it only
    // commits the number.
    let typing = ctx.memory(|m| m.focused().is_some());
    if enter && !typing {
        apply = true;
    }
    if response.should_close() || escape {
        cancel = true;
    }

    if apply {
        session.resize_canvas = None;
        session.resize_canvas(dialog.size(), dialog.anchor);
    } else if cancel {
        session.resize_canvas = None;
    } else {
        session.resize_canvas = Some(dialog);
    }
}

/// A 3×3 grid of cells; the chosen anchor is filled and arrows on its
/// neighbours show which way the canvas grows.
fn anchor_picker(ui: &mut egui::Ui, anchor: &mut CanvasAnchor) {
    let (selected_col, selected_row) = anchor.grid_position();
    let gap = 3.0;
    let side = CELL * 3.0 + gap * 2.0;
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
    let visuals = ui.visuals().clone();

    for candidate in CanvasAnchor::ALL {
        let (col, row) = candidate.grid_position();
        let min = rect.min + vec2(f32::from(col), f32::from(row)) * (CELL + gap);
        let cell = Rect::from_min_size(min, Vec2::splat(CELL));
        let response = ui
            .interact(
                cell,
                ui.scope_id().with(("resize_anchor", col, row)),
                Sense::click(),
            )
            .on_hover_text(candidate.label());
        if response.clicked() {
            *anchor = candidate;
        }

        let selected = candidate == *anchor;
        let widget = if selected {
            &visuals.widgets.active
        } else if response.hovered() {
            &visuals.widgets.hovered
        } else {
            &visuals.widgets.inactive
        };
        let painter = ui.painter();
        let fill = if selected {
            visuals.selection.bg_fill
        } else {
            widget.weak_bg_fill
        };
        painter.rect_filled(cell, 3.0, fill);
        painter.rect_stroke(cell, 3.0, widget.bg_stroke, egui::StrokeKind::Inside);

        if selected {
            painter.rect_filled(
                Rect::from_center_size(cell.center(), Vec2::splat(CELL * 0.35)),
                2.0,
                visuals.selection.stroke.color,
            );
            continue;
        }
        // Neighbours of the anchor get an arrow pointing away from it.
        let dx = f32::from(col) - f32::from(selected_col);
        let dy = f32::from(row) - f32::from(selected_row);
        if dx.abs() <= 1.0 && dy.abs() <= 1.0 {
            let dir = vec2(dx, dy).normalized() * CELL * 0.4;
            let start = cell.center() - dir * 0.5;
            let color = if response.hovered() {
                widget.fg_stroke.color
            } else {
                visuals.weak_text_color()
            };
            painter.arrow(start, dir, Stroke::new(1.5, color));
        }
    }
}
