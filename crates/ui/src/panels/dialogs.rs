//! Modal dialogs: errors and "discard unsaved changes?".

use crate::session::{Dialog, DiscardAction, Session};

/// Shows the session's modal dialog, if any.
pub fn show(ctx: &egui::Context, session: &mut Session) {
    let Some(dialog) = session.dialog().cloned() else {
        return;
    };
    let mut close = false;
    let mut discard = None;
    let response = egui::Modal::new(egui::Id::unique("editor_dialog")).show(ctx, |ui| {
        ui.set_max_width(420.0);
        match &dialog {
            Dialog::Error { title, message } => {
                ui.heading(title);
                ui.add_space(6.0);
                ui.label(message);
                ui.add_space(10.0);
                if ui.button("OK").clicked() {
                    close = true;
                }
            }
            Dialog::ConfirmDiscard(action) => {
                ui.heading("Discard unsaved changes?");
                ui.add_space(6.0);
                let what = match action {
                    DiscardAction::New => "Starting a new document",
                    DiscardAction::Open => "Opening another document",
                    DiscardAction::Quit => "Quitting",
                };
                ui.label(format!(
                    "{what} will lose the changes to “{}” since it was last saved.",
                    session.display_name()
                ));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Discard changes").clicked() {
                        discard = Some(*action);
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            }
        }
    });
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    if response.should_close() || escape || (enter && matches!(dialog, Dialog::Error { .. })) {
        close = true;
    }
    if let Some(action) = discard {
        session.close_dialog();
        session.perform(action);
    } else if close {
        session.close_dialog();
    }
}
