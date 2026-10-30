//! The tool settings panel on the right of the canvas: settings for the
//! active tool (size, opacity, colour, eraser mode, gap closing) and the
//! "drawing is blocked" warning.

use super::toolbar::TOOLS;
use crate::session::Session;
use crate::tools::EraserMode;
use document::animation_core::Rgba;
use document::drawing::GapClosing;
use editor::Tool;

/// Quick colour swatches (linear RGB).
const SWATCHES: [[f32; 3]; 8] = [
    [0.0, 0.0, 0.0],
    [1.0, 1.0, 1.0],
    [0.216, 0.216, 0.216],
    [0.8, 0.02, 0.02],
    [0.95, 0.4, 0.0],
    [0.9, 0.75, 0.0],
    [0.05, 0.45, 0.08],
    [0.02, 0.15, 0.75],
];

const PARTIAL_HINT: &str =
    "Cuts away the part of each stroke it passes over on the active layer and frame.";
const STROKE_HINT: &str = "Removes every stroke it touches on the active layer and frame.";
const FILL_HINT: &str = "Fills the area enclosed by lines, with the brush colour and opacity. \
                         Click a fill to recolour it.";

/// Eraser mode choices as selectable labels, laid out in the current
/// direction. Disabled while a gesture is in progress. Returns whether the
/// user picked one.
pub(crate) fn eraser_mode_choices(ui: &mut egui::Ui, session: &mut Session) -> bool {
    let busy = session.tools.is_active();
    ui.add_enabled_ui(!busy, |ui| {
        let mode = &mut session.tools.eraser_mode;
        let partial = ui
            .selectable_value(mode, EraserMode::Partial, "Partial eraser")
            .on_hover_text(PARTIAL_HINT);
        let stroke = ui
            .selectable_value(mode, EraserMode::Stroke, "Stroke eraser")
            .on_hover_text(STROKE_HINT);
        partial.clicked() || stroke.clicked()
    })
    .inner
}

/// Gap closing choices as selectable labels. Returns whether the user picked
/// one.
pub(crate) fn gap_choices(ui: &mut egui::Ui, session: &mut Session) -> bool {
    let gap = &mut session.tools.fill_gap;
    let mut picked = false;
    for option in GapClosing::ALL {
        picked |= ui.selectable_value(gap, option, option.label()).clicked();
    }
    picked
}

/// Draws the tool settings panel.
pub fn panel(ui: &mut egui::Ui, session: &mut Session) {
    // Leave room for the value box beside each slider.
    ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(60.0);
    ui.horizontal(|ui| active_tool_label(ui, session));
    if let Some(reason) = session.draw_block_reason() {
        ui.colored_label(ui.visuals().warn_fg_color, format!("⚠ {reason}"));
    }
    ui.separator();
    match session.state.tool {
        Tool::Select => {
            ui.weak(
                "Click or drag a box to select (Shift adds). Drag a selected item to move, \
                 arrows nudge (Shift ×10), Delete removes, ⌘A selects all, Esc deselects.",
            );
        }
        Tool::Lasso => {
            ui.weak(
                "Draw a loop around strokes and fills to select them (Shift adds). Drag a \
                 selected item to move, Delete removes, Esc deselects.",
            );
        }
        Tool::Transform => {
            ui.weak(
                "Drag a corner or edge to scale (Shift keeps proportions, Alt scales from \
                 the centre). Drag the knob or just outside a corner to rotate (Shift snaps \
                 to 15°). Drag inside to move; click elsewhere to select.",
            );
        }
        Tool::Brush => {
            brush_size(ui, session);
            brush_smoothing(ui, session);
            ui.separator();
            paint_settings(ui, session);
        }
        Tool::Fill => {
            ui.label("Close gaps").on_hover_text(FILL_HINT);
            gap_choices(ui, session);
            ui.separator();
            paint_settings(ui, session);
        }
        Tool::Eraser => {
            eraser_size(ui, session);
            ui.separator();
            eraser_mode_choices(ui, session);
        }
    }
}

/// The active tool's icon and name, so the bar reads as belonging to it.
fn active_tool_label(ui: &mut egui::Ui, session: &Session) {
    let Some(entry) = TOOLS.iter().find(|e| e.tool == session.state.tool) else {
        return;
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    (entry.icon)(ui.painter(), rect, ui.visuals().strong_text_color());
    ui.strong(entry.name);
}

fn brush_size(ui: &mut egui::Ui, session: &mut Session) {
    ui.label("Size");
    ui.add(
        egui::Slider::new(&mut session.state.brush.width, 0.5..=200.0)
            .logarithmic(true)
            .max_decimals(1),
    );
}

const SMOOTHING_HINT: &str = "Irons out wobble from your hand or tablet. 0 keeps the line \
                              exactly as drawn; higher values give smoother, rounder curves.";

fn brush_smoothing(ui: &mut egui::Ui, session: &mut Session) {
    ui.label("Smoothing").on_hover_text(SMOOTHING_HINT);
    ui.add(
        egui::Slider::new(&mut session.tools.brush_smoothing, 0.0..=100.0)
            .max_decimals(0)
            .suffix("%"),
    )
    .on_hover_text(SMOOTHING_HINT);
}

fn eraser_size(ui: &mut egui::Ui, session: &mut Session) {
    ui.label("Size");
    let mut diameter = session.tools.eraser_radius * 2.0;
    if ui
        .add(
            egui::Slider::new(&mut diameter, 1.0..=400.0)
                .logarithmic(true)
                .max_decimals(1),
        )
        .changed()
    {
        session.tools.eraser_radius = diameter * 0.5;
    }
}

/// Opacity, colour and swatches, shared by the brush and the paint bucket.
fn paint_settings(ui: &mut egui::Ui, session: &mut Session) {
    ui.label("Opacity");
    ui.add(egui::Slider::new(
        &mut session.state.brush.opacity,
        0.05..=1.0,
    ));
    ui.horizontal(|ui| {
        ui.label("Colour");
        let Rgba { r, g, b, a } = session.state.brush.color;
        let mut rgb = [r, g, b];
        if ui.color_edit_button_rgb(&mut rgb).changed() {
            session.state.brush.color = Rgba::new(rgb[0], rgb[1], rgb[2], a);
        }
    });
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for [r, g, b] in SWATCHES {
            let color = egui::Rgba::from_rgb(r, g, b);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 3.0, color);
            ui.painter().rect_stroke(
                rect,
                3.0,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );
            if response.clicked() {
                session.state.brush.color = Rgba::rgb(r, g, b);
            }
        }
    });
}
