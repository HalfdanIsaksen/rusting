//! The tool panel: brush/eraser, size, colour and opacity.

use crate::session::Session;
use document::animation_core::Rgba;
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

/// Draws the tool panel.
pub fn panel(ui: &mut egui::Ui, session: &mut Session) {
    ui.heading("Tools");
    ui.add_space(4.0);
    let busy = session.tools.is_active();
    ui.add_enabled_ui(!busy, |ui| {
        ui.horizontal(|ui| {
            let tool = &mut session.state.tool;
            ui.selectable_value(tool, Tool::Brush, "✏ Brush")
                .on_hover_text("B");
            ui.selectable_value(tool, Tool::Eraser, "Eraser")
                .on_hover_text("E — removes whole strokes");
        });
    });
    ui.separator();

    match session.state.tool {
        Tool::Brush => {
            ui.label("Size");
            ui.add(
                egui::Slider::new(&mut session.state.brush.width, 0.5..=200.0)
                    .logarithmic(true)
                    .max_decimals(1),
            );
            ui.add_space(4.0);
            ui.label("Opacity");
            ui.add(egui::Slider::new(
                &mut session.state.brush.opacity,
                0.05..=1.0,
            ));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Colour");
                let Rgba { r, g, b, a } = session.state.brush.color;
                let mut rgb = [r, g, b];
                if ui.color_edit_button_rgb(&mut rgb).changed() {
                    session.state.brush.color = Rgba::new(rgb[0], rgb[1], rgb[2], a);
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for [r, g, b] in SWATCHES {
                    let color = egui::Rgba::from_rgb(r, g, b);
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
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
        Tool::Eraser => {
            ui.label("Eraser size");
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
            ui.weak("Removes every stroke it touches on the active layer and frame.");
        }
    }

    ui.separator();
    if let Some(reason) = session.draw_block_reason() {
        ui.colored_label(ui.visuals().warn_fg_color, format!("⚠ {reason}"));
    }
}
