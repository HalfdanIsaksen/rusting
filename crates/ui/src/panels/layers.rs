//! The layer panel: add, remove, select, rename, show/hide, lock, reorder.

use crate::session::Session;
use document::animation_core::LayerId;
use editor::Command;
use editor::commands::{
    AddLayer, Batch, MoveLayer, RemoveLayer, RenameLayer, SetLayerLocked, SetLayerVisible,
};

/// Rename-in-progress, kept in egui's temporary memory.
#[derive(Clone, Debug, Default)]
struct Renaming {
    layer: Option<LayerId>,
    text: String,
    focus_requested: bool,
}

/// Draws the layer panel.
pub fn panel(ui: &mut egui::Ui, session: &mut Session) {
    ui.heading("Layers");
    ui.add_space(4.0);
    buttons(ui, session);
    ui.separator();

    let rename_id = ui.scope_id().with("renaming");
    let mut renaming: Renaming = ui.data(|d| d.get_temp(rename_id)).unwrap_or_default();
    // F2 renames the active layer.
    let f2 = !ui.ctx().egui_wants_keyboard_input()
        && session.dialog().is_none()
        && ui.input(|i| i.key_pressed(egui::Key::F2));
    if f2
        && renaming.layer.is_none()
        && let Some(layer) = session
            .state
            .active_layer
            .and_then(|id| session.document.layers().get(id))
    {
        renaming = Renaming {
            layer: Some(layer.id),
            text: layer.name.clone(),
            focus_requested: false,
        };
    }
    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    let mut select = None;

    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            // Top of the stack first, like the canvas stacking.
            for layer in session.document.layers().iter().rev() {
                let active = session.state.active_layer == Some(layer.id);
                ui.horizontal(|ui| {
                    let eye = if layer.visible { "👁" } else { "–" };
                    if ui
                        .selectable_label(layer.visible, eye)
                        .on_hover_text(if layer.visible {
                            "Hide layer"
                        } else {
                            "Show layer"
                        })
                        .clicked()
                    {
                        commands.push(Box::new(SetLayerVisible::new(layer.id, !layer.visible)));
                    }
                    let lock = if layer.locked { "🔒" } else { "🔓" };
                    if ui
                        .selectable_label(layer.locked, lock)
                        .on_hover_text(if layer.locked {
                            "Unlock layer"
                        } else {
                            "Lock layer"
                        })
                        .clicked()
                    {
                        commands.push(Box::new(SetLayerLocked::new(layer.id, !layer.locked)));
                    }

                    if renaming.layer == Some(layer.id) {
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut renaming.text)
                                .desired_width(ui.available_width()),
                        );
                        if !renaming.focus_requested {
                            edit.request_focus();
                            renaming.focus_requested = true;
                        }
                        let (enter, escape) = ui.input(|i| {
                            (
                                i.key_pressed(egui::Key::Enter),
                                i.key_pressed(egui::Key::Escape),
                            )
                        });
                        if escape {
                            renaming = Renaming::default();
                        } else if edit.lost_focus() || enter {
                            let name = renaming.text.trim().to_owned();
                            if !name.is_empty() && name != layer.name {
                                commands.push(Box::new(RenameLayer::new(layer.id, name)));
                            }
                            renaming = Renaming::default();
                        }
                    } else {
                        let mut text = egui::RichText::new(&layer.name);
                        if !layer.visible {
                            text = text.weak();
                        }
                        let label = ui
                            .add_sized(
                                egui::vec2(ui.available_width(), 0.0),
                                egui::Button::selectable(active, text),
                            )
                            .on_hover_text("Double-click or F2 to rename");
                        if label.clicked() {
                            select = Some(layer.id);
                        }
                        if label.double_clicked() {
                            renaming = Renaming {
                                layer: Some(layer.id),
                                text: layer.name.clone(),
                                focus_requested: false,
                            };
                        }
                    }
                });
            }
        });

    ui.data_mut(|d| d.insert_temp(rename_id, renaming));
    if let Some(id) = select {
        session.select_layer(id);
    }
    for command in commands {
        if session.execute(command).is_err() {
            break;
        }
    }
}

fn buttons(ui: &mut egui::Ui, session: &mut Session) {
    let layers = session.document.layers();
    let active = session.state.active_layer;
    let index = active.and_then(|id| layers.index_of(id));
    let count = layers.len();
    ui.horizontal(|ui| {
        if ui
            .button("+ Add")
            .on_hover_text("Add a layer above the active one")
            .clicked()
        {
            add_layer(session, index);
        }
        if ui
            .add_enabled(active.is_some(), egui::Button::new("- Remove"))
            .on_hover_text("Remove the active layer")
            .clicked()
            && let Some(id) = active
        {
            let _ = session.execute(Box::new(RemoveLayer::new(id)));
        }
        let can_raise = index.is_some_and(|i| i + 1 < count);
        if ui
            .add_enabled(can_raise, egui::Button::new("⏶"))
            .on_hover_text("Move up")
            .clicked()
            && let (Some(id), Some(i)) = (active, index)
        {
            let _ = session.execute(Box::new(MoveLayer::new(id, i + 1)));
        }
        let can_lower = index.is_some_and(|i| i > 0);
        if ui
            .add_enabled(can_lower, egui::Button::new("⏷"))
            .on_hover_text("Move down")
            .clicked()
            && let (Some(id), Some(i)) = (active, index)
        {
            let _ = session.execute(Box::new(MoveLayer::new(id, i - 1)));
        }
    });
}

/// Adds a layer just above `above` (or on top), as one undo step, and selects it.
fn add_layer(session: &mut Session, above: Option<usize>) {
    let layers = session.document.layers();
    let name = unique_name(layers.iter().map(|l| l.name.as_str()));
    let add = AddLayer::new(name);
    let id = add.layer_id();
    let top = layers.len();
    let command: Box<dyn Command> = match above {
        Some(i) if i + 1 < top => Box::new(Batch::new(
            "Add Layer",
            vec![Box::new(add), Box::new(MoveLayer::new(id, i + 1))],
        )),
        _ => Box::new(add),
    };
    if session.execute(command).is_ok() {
        session.select_layer(id);
    }
}

/// "Layer N" with the smallest N above every existing "Layer N".
fn unique_name<'a>(names: impl Iterator<Item = &'a str>) -> String {
    let highest = names
        .filter_map(|n| n.strip_prefix("Layer ")?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("Layer {}", highest + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_count_up() {
        assert_eq!(
            unique_name(["Layer 1", "Ink", "Layer 3"].into_iter()),
            "Layer 4"
        );
        assert_eq!(unique_name(std::iter::empty()), "Layer 1");
    }

    #[test]
    fn add_layer_goes_above_active_in_one_step() {
        let mut s = Session::new();
        let bottom = s.state.active_layer.unwrap();
        add_layer(&mut s, Some(0));
        let top = s.state.active_layer.unwrap();
        s.select_layer(bottom);
        add_layer(&mut s, Some(0));
        let middle = s.state.active_layer.unwrap();
        let order: Vec<_> = s.document.layers().iter().map(|l| l.id).collect();
        assert_eq!(order, [bottom, middle, top]);
        s.undo();
        assert_eq!(s.document.layers().len(), 2);
        assert_eq!(
            s.state
                .active_layer
                .map(|id| s.document.layers().get(id).is_some()),
            Some(true)
        );
    }
}
