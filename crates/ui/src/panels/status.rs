//! The status bar: the latest message and a summary of the editor state.

use crate::session::{STATUS_DURATION, Session};

/// Draws the status bar.
pub fn bar(ui: &mut egui::Ui, session: &mut Session) {
    ui.horizontal(|ui| {
        match session.status() {
            Some(status) => {
                let text = egui::RichText::new(&status.text);
                if status.is_error {
                    ui.colored_label(ui.visuals().error_fg_color, text);
                } else {
                    ui.label(text);
                }
                let left = STATUS_DURATION.saturating_sub(status.at.elapsed());
                ui.ctx().request_repaint_after(left);
            }
            None => {
                ui.weak("Ready");
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let doc = &session.document;
            let layer = session
                .state
                .active_layer
                .and_then(|id| doc.layers().get(id))
                .map_or("none", |l| l.name.as_str());
            ui.weak(format!(
                "Frame {} / {}   ·   {} fps   ·   Layer: {layer}{}",
                session.current_frame().display_number(),
                doc.frame_count,
                fps_text(doc.fps),
                if session.is_dirty() {
                    "   ·   unsaved"
                } else {
                    ""
                },
            ));
        });
    });
}

/// A frame rate for display: "24" or "23.976".
pub fn fps_text(fps: document::animation_core::Fps) -> String {
    if fps.denominator() == 1 {
        fps.numerator().to_string()
    } else {
        format!("{:.3}", fps.as_f64())
    }
}
