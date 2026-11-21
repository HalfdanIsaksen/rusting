//! The menu bar: File, Edit, Canvas, View and Help.

use super::shortcuts;
use crate::session::{DiscardAction, ExportKind, Session};
use egui::{Atom, Button, Checkbox, KeyboardShortcut, RichText};

fn item<'a>(ui: &egui::Ui, text: &'a str, shortcut: &KeyboardShortcut) -> Button<'a> {
    Button::new(text).shortcut_text(ui.ctx().format_shortcut(shortcut))
}

/// A checkable View-menu item bound to `on`, with an optional shortcut hint.
/// The checkmark always shows the current value.
fn toggle(ui: &mut egui::Ui, text: &str, on: &mut bool, shortcut: Option<&KeyboardShortcut>) {
    let hint = shortcut
        .map(|s| ui.ctx().format_shortcut(s))
        .unwrap_or_default();
    ui.add(Checkbox::new(
        on,
        (text, Atom::grow(), RichText::new(hint).weak()),
    ));
}

/// Copy, Cut and Paste of canvas items, for the Edit menu and the canvas's
/// right-click menu. Copy and Cut also put
/// [`crate::clipboard::CLIPBOARD_MARKER`] on the system clipboard, so
/// Cmd/Ctrl+V pastes too.
pub fn clipboard_items(ui: &mut egui::Ui, session: &mut Session) {
    let can_copy = session.can_copy();
    if ui
        .add_enabled(can_copy, item(ui, "Cut", &shortcuts::CUT))
        .clicked()
        && session.cut_selection()
    {
        ui.ctx()
            .copy_text(crate::clipboard::CLIPBOARD_MARKER.to_owned());
    }
    if ui
        .add_enabled(can_copy, item(ui, "Copy", &shortcuts::COPY))
        .clicked()
        && session.copy_selection()
    {
        ui.ctx()
            .copy_text(crate::clipboard::CLIPBOARD_MARKER.to_owned());
    }
    if ui
        .add_enabled(session.can_paste(), item(ui, "Paste", &shortcuts::PASTE))
        .on_hover_text("Paste the copied items on this frame, where they were")
        .clicked()
    {
        session.paste();
    }
}

/// The canvas's right-click menu: Cut, Copy, Paste and Delete for the
/// selected items. The host opens it on the canvas response.
pub fn canvas_menu(ui: &mut egui::Ui, session: &mut Session) {
    clipboard_items(ui, session);
    ui.separator();
    if ui
        .add_enabled(session.can_copy(), Button::new("Delete"))
        .clicked()
    {
        session.delete_selection();
    }
    ui.separator();
    create_brush_item(ui, session);
}

/// Create Brush from Selection, for the Edit menu and the canvas's
/// right-click menu (ADR-024).
fn create_brush_item(ui: &mut egui::Ui, session: &mut Session) {
    if ui
        .add_enabled(
            session.can_create_brush(),
            Button::new("Create Brush from Selection"),
        )
        .on_hover_text(
            "Make the selected strokes, fills and pictures the tip of a new brush in the \
             Library (dark marks paint)",
        )
        .clicked()
    {
        session.create_brush_from_selection();
    }
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
                if ui.button("Export Video (MP4/MOV)…").clicked() {
                    session.export(ExportKind::Video);
                }
                if ui.button("Export GIF…").clicked() {
                    session.export(ExportKind::Gif);
                }
                if ui.button("Export Sprite Sheet…").clicked() {
                    session.export(ExportKind::SpriteSheet);
                }
            });
            if ui
                .button("Import…")
                .on_hover_text(
                    "Place PNG or JPEG pictures on the current layer and frame, or put MP3 or \
                     WAV sounds on new audio layers",
                )
                .clicked()
            {
                session.import();
            }
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
            ui.separator();
            clipboard_items(ui, session);
            ui.separator();
            if ui
                .add(item(ui, "Duplicate Frame", &shortcuts::DUPLICATE_FRAME))
                .on_hover_text("Copy this frame to the next one, on the active layer")
                .clicked()
            {
                session.duplicate_frame();
            }
            if ui
                .add(item(ui, "Insert Frame", &shortcuts::INSERT_FRAME))
                .on_hover_text("Hold this frame one frame longer on the active layer, moving its later keys right")
                .clicked()
            {
                session.insert_frame();
            }
            if ui
                .add(item(ui, "Delete Frames", &shortcuts::DELETE_FRAMES))
                .on_hover_text("Remove the selected frames (or this frame) on the selected layers, pulling their later keys left")
                .clicked()
            {
                session.delete_frames();
            }
            ui.separator();
            if ui
                .add(item(ui, "Convert to Object", &shortcuts::CONVERT_TO_OBJECT))
                .on_hover_text(
                    "Turn the items selected on the canvas, or the selected layers and frames, \
                     into an object in the Library",
                )
                .clicked()
            {
                if session.tools.selection_at().is_some() {
                    session.convert_selection_to_object();
                } else {
                    session.convert_layers_to_object();
                }
            }
            create_brush_item(ui, session);
        });

        ui.menu_button("Canvas", |ui| {
            if ui.button("Resize Canvas…").clicked() {
                session.open_resize_canvas();
            }
        });

        // One `toggle` line per panel or window.
        ui.menu_button("View", |ui| {
            let view = &mut session.view;
            toggle(
                ui,
                "Toolbar",
                &mut view.toolbar,
                Some(&shortcuts::TOGGLE_TOOLBAR),
            );
            toggle(
                ui,
                "Timeline",
                &mut view.timeline,
                Some(&shortcuts::TOGGLE_TIMELINE),
            );
            toggle(
                ui,
                "Tool Settings",
                &mut view.tool_settings,
                Some(&shortcuts::TOGGLE_TOOL_SETTINGS),
            );
            toggle(
                ui,
                "Library",
                &mut view.library,
                Some(&shortcuts::TOGGLE_LIBRARY),
            );
            if ui
                .button("Reset Panel Layout")
                .on_hover_text(
                    "Put every panel back in its original place and size, and show them all. \
                     Drag a panel's tab to move it.",
                )
                .clicked()
            {
                session.reset_panel_layout();
            }
            ui.separator();
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
                ("I / Alt+click", "Eyedropper: pick a colour from the canvas"),
                ("T", "Text: click the canvas and type"),
                (
                    "Enter / Cmd/Ctrl+Enter",
                    "Text: new line / finish (Esc cancels)",
                ),
                ("Space", "Play / pause (hold and drag to pan)"),
                ("← / →", "Previous / next frame"),
                ("↑ / ↓", "Previous / next key"),
                ("Cmd/Ctrl+C / X / V", "Copy / cut / paste the selection, in place"),
                ("Cmd/Ctrl+D", "Duplicate frame"),
                ("Middle-drag", "Pan the canvas"),
                ("Wheel / pinch", "Zoom about the cursor"),
                ("F", "Fit canvas"),
                ("F2 / double-click", "Rename the active layer"),
                (
                    "Drag from Library",
                    "Place a picture where you drop it on the canvas",
                ),
                (
                    "F8",
                    "Convert the canvas selection (or selected frames) to an object",
                ),
                (
                    "Drag an object",
                    "From the Library onto the canvas or a timeline cell to place it",
                ),
                ("Cmd/Ctrl/Shift+click", "Select several layers"),
            ] {
                ui.horizontal(|ui| {
                    ui.strong(keys);
                    ui.label(what);
                });
            }
        });
    });
}
