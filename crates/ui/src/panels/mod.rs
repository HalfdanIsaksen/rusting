//! The egui panels around the canvas.

mod dialogs;
pub mod dock;
mod layers;
pub mod library;
mod menu;
mod preview;
mod resize_canvas;
mod shortcuts;
mod start_menu;
mod status;
mod timeline;
mod tool_options;
pub mod toolbar;

use crate::session::Session;

pub use preview::window as preview_window;

/// Draws the whole editor for one frame, or the start menu while it is up.
/// `canvas` fills the canvas pane, wherever the user docked it; the host draws
/// the canvas there and forwards pointer input to the session.
pub fn show(
    ui: &mut egui::Ui,
    session: &mut Session,
    canvas: impl FnOnce(&mut egui::Ui, &mut Session),
) {
    if session.start_menu().is_some() {
        start_menu::show(ui, session);
        return;
    }
    if session.update_playback() {
        ui.ctx().request_repaint();
    }
    if session.preview.update(&session.document) {
        ui.ctx().request_repaint();
    }
    shortcuts::handle(ui.ctx(), session);
    session.sync_selection();
    session.sync_text();

    egui::Panel::top("menu_bar").show(ui, |ui| menu::bar(ui, session));
    egui::Panel::bottom("status_bar").show(ui, |ui| status::bar(ui, session));
    // Everything else is a dockable pane (ADR-021). The layout is taken out
    // of the session while it is shown, so the panes can borrow the session.
    let mut layout = std::mem::take(&mut session.panels);
    egui::CentralPanel::no_frame().show(ui, |ui| layout.show(ui, session, canvas));
    session.panels = layout;

    dialogs::show(ui.ctx(), session);
    resize_canvas::show(ui.ctx(), session);
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::{AudioData, AudioFormat};

    #[test]
    fn editor_draws_audio_layers_without_changing_the_document() {
        let mut s = Session::new();
        let sound = AudioData {
            format: AudioFormat::Wav,
            sample_rate: 8000,
            channels: 1,
            duration: 0.5,
            peaks: (0..50).map(|i| i * 5).collect(),
            bytes: std::sync::Arc::from(vec![1u8]),
        };
        let layer = s.import_audio("song.wav", sound).unwrap();
        let ctx = egui::Context::default();
        for active in [false, true] {
            if active {
                s.set_active_layer(layer);
            }
            let before = s.document.clone();
            let mut output =
                ctx.run_ui(egui::RawInput::default(), |ui| show(ui, &mut s, |_, _| {}));
            output.textures_delta.clear();
            assert_eq!(s.document, before);
        }
    }
}
