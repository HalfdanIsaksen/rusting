//! The egui panels around the canvas.

mod dialogs;
mod layers;
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

/// Draws the whole editor for one frame, or the start menu while it is up. `canvas` fills the central area;
/// the host draws the canvas and forwards pointer input to the session.
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
    // Hidden panels are simply not added, so the canvas takes their space.
    if session.view.timeline {
        egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(210.0)
            .min_size(140.0)
            .show(ui, |ui| timeline::panel(ui, session));
    }
    if session.view.toolbar {
        egui::Panel::left("toolbar")
            .resizable(false)
            .exact_size(toolbar::WIDTH)
            .frame(
                egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(4, 0)),
            )
            .show(ui, |ui| toolbar::panel(ui, session));
    }
    if session.view.tool_settings {
        egui::Panel::right("tool_settings")
            .resizable(true)
            .default_size(220.0)
            .min_size(180.0)
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::same(8)))
            .show(ui, |ui| tool_options::panel(ui, session));
    }
    egui::CentralPanel::no_frame().show(ui, |ui| canvas(ui, session));

    dialogs::show(ui.ctx(), session);
    resize_canvas::show(ui.ctx(), session);
}
