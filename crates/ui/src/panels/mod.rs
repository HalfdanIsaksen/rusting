//! The egui panels around the canvas.

mod dialogs;
mod layers;
mod menu;
mod shortcuts;
mod status;
mod timeline;
mod tool_options;
pub mod toolbar;

use crate::session::Session;

/// Draws the whole editor for one frame. `canvas` fills the central area;
/// the host draws the canvas and forwards pointer input to the session.
pub fn show(
    ui: &mut egui::Ui,
    session: &mut Session,
    canvas: impl FnOnce(&mut egui::Ui, &mut Session),
) {
    if session.update_playback() {
        ui.ctx().request_repaint();
    }
    shortcuts::handle(ui.ctx(), session);

    egui::Panel::top("menu_bar").show(ui, |ui| menu::bar(ui, session));
    egui::Panel::bottom("status_bar").show(ui, |ui| status::bar(ui, session));
    egui::Panel::bottom("timeline")
        .resizable(true)
        .default_size(210.0)
        .min_size(140.0)
        .show(ui, |ui| timeline::panel(ui, session));
    egui::Panel::left("toolbar")
        .resizable(false)
        .exact_size(toolbar::WIDTH)
        .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(4, 0)))
        .show(ui, |ui| toolbar::panel(ui, session));
    egui::Panel::right("layers")
        .resizable(true)
        .default_size(240.0)
        .show(ui, |ui| layers::panel(ui, session));
    // Added after the side panels so it spans only the canvas.
    egui::Panel::top("tool_options")
        .resizable(false)
        .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 4)))
        .show(ui, |ui| tool_options::bar(ui, session));
    egui::CentralPanel::no_frame().show(ui, |ui| canvas(ui, session));

    dialogs::show(ui.ctx(), session);
}
