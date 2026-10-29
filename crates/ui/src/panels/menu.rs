//! The menu bar: File, Edit, Canvas, View and Help.

use super::shortcuts;
use crate::session::{DiscardAction, ExportKind, Session};
use egui::{Button, KeyboardShortcut};

fn item<'a>(ui: &egui::Ui, text: &'a str, shortcut: &KeyboardShortcut) -> Button<'a> {
    Button::new(text).shortcut_text(ui.ctx().format_shortcut(shortcut))
}

/// Draws the menu bar.
pub fn bar(ui: &mut egui::Ui, session: &mut Session) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui.add(item(ui, "New", &shortcuts::NEW)).clicked() {
                session.request(DiscardAction::New);
            }
            if ui.add(item(ui, "Open…", &shortcuts::OPEN)).clicked() {
                session.request(DiscardAction::Open);
            }
            ui.separator();
            if ui.add(item(ui, "Save", &shortcuts::SAVE)).clicked() {
                session.save();
            }
            if ui.add(item(ui, "Save As…", &shortcuts::SAVE_AS)).clicked() {
                session.save_as();
            }
            ui.menu_button("Export", |ui| {
                if ui.button("Export Frame…").clicked() {
                    session.export(ExportKind::Frame);
                }
                if ui.button("Export Image Sequence…").clicked() {
                    session.export(ExportKind::Sequence);
                }
                if ui.button("Export Video (MP4)…").clicked() {
                    session.export(ExportKind::Video);
                }
            });
            ui.separator();
            if ui.add(item(ui, "Quit", &shortcuts::QUIT)).clicked() {
                session.request(DiscardAction::Quit);
            }
        });

        ui.menu_button("Edit", |ui| {
            let undo = match session.history.undo_label() {
                Some(label) => format!("Undo {label}"),
                None => "Undo".to_owned(),
            };
            let redo = match session.history.redo_label() {
                Some(label) => format!("Redo {label}"),
                None => "Redo".to_owned(),
            };
            let can_undo = session.history.can_undo();
            let can_redo = session.history.can_redo();
            if ui
                .add_enabled(can_undo, item(ui, &undo, &shortcuts::UNDO))
                .clicked()
            {
                session.undo();
            }
            if ui
                .add_enabled(can_redo, item(ui, &redo, &shortcuts::REDO))
                .clicked()
            {
                session.redo();
            }
        });

        ui.menu_button("Canvas", |ui| {
            if ui.button("Resize Canvas…").clicked() {
                session.open_resize_canvas();
            }
        });

        ui.menu_button("View", |ui| {
            ui.checkbox(&mut session.onion.enabled, "Onion skin");
            let mut preview = session.preview.is_open();
            if ui.checkbox(&mut preview, "Preview").changed() {
                session.preview.set_open(preview);
            }
        });

        ui.menu_button("Help", |ui| {
            ui.label("Shortcuts");
            ui.separator();
            for (keys, what) in [
                ("B / E", "Brush / Eraser"),
                ("Space", "Play / pause (hold and drag to pan)"),
                ("← / →", "Previous / next frame"),
                ("↑ / ↓", "Previous / next key"),
                ("Middle-drag", "Pan the canvas"),
                ("Wheel / pinch", "Zoom about the cursor"),
                ("F", "Fit canvas"),
                ("F2 / double-click", "Rename the active layer"),
            ] {
                ui.horizontal(|ui| {
                    ui.strong(keys);
                    ui.label(what);
                });
            }
        });
    });
}
