//! The layer column on the left of the timeline: one row per layer with
//! show/hide, lock and the name (click to select, double-click or F2 to
//! rename, drag to reorder), and a header with add, remove, move up/down and
//! show/lock-all toggles.

use super::timeline::{ROW_HEIGHT, RULER_HEIGHT, Row};
use crate::icons::{self, IconFn};
use crate::session::Session;
use document::animation_core::LayerId;
use editor::Command;
use editor::commands::{
    AddLayer, Batch, MoveLayer, RemoveLayer, RenameLayer, SetLayerLocked, SetLayerVisible,
};
use egui::{Pos2, Rect, Sense, Stroke, vec2};

/// Width of the layer column.
pub(super) const COLUMN_WIDTH: f32 = 190.0;
/// Width of the eye and lock cells.
const TOGGLE_WIDTH: f32 = 22.0;
/// Width of a header button.
const BUTTON_WIDTH: f32 = 22.0;
/// Icon size inside a cell or button.
const ICON_SIZE: f32 = 14.0;

/// Rename-in-progress, kept in egui's temporary memory.
#[derive(Clone, Debug, Default)]
struct Renaming {
    layer: Option<LayerId>,
    text: String,
    focus_requested: bool,
}

/// A header button's action.
#[derive(Clone, Copy)]
enum Action {
    Add,
    Remove,
    Raise,
    Lower,
}

/// Draws the layer column, `height` tall (ruler row plus one row per layer).
/// `rows` are top of the stack first.
pub(super) fn column(ui: &mut egui::Ui, session: &mut Session, rows: &[Row], height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(COLUMN_WIDTH, height), Sense::hover());
    let visuals = ui.visuals().clone();
    let line = visuals.widgets.noninteractive.bg_stroke;
    let painter = ui.painter_at(rect);
    let header = Rect::from_min_size(rect.min, vec2(COLUMN_WIDTH, RULER_HEIGHT));
    painter.rect_filled(header, 0.0, visuals.faint_bg_color);
    painter.rect_filled(
        Rect::from_x_y_ranges(rect.x_range(), header.bottom()..=rect.bottom()),
        0.0,
        visuals.panel_fill,
    );

    let mut commands: Vec<Box<dyn Command>> = Vec::new();
    let mut action = None;
    header_controls(ui, session, rows, header, &mut commands, &mut action);

    let rename_id = ui.scope_id().with("layer_renaming");
    let mut renaming: Renaming = ui.data(|d| d.get_temp(rename_id)).unwrap_or_default();
    // F2 renames the active layer.
    let f2 = !ui.ctx().egui_wants_keyboard_input()
        && session.dialog().is_none()
        && ui.input(|i| i.key_pressed(egui::Key::F2));
    if f2
        && renaming.layer.is_none()
        && let Some(row) = rows
            .iter()
            .find(|r| session.state.active_layer == Some(r.id))
    {
        renaming = Renaming {
            layer: Some(row.id),
            text: row.name.clone(),
            focus_requested: false,
        };
    }

    let drag_id = ui.scope_id().with("layer_drag");
    let mut select = None;
    let rows_top = header.bottom();
    for (i, row) in rows.iter().enumerate() {
        let top = rows_top + i as f32 * ROW_HEIGHT;
        let row_rect =
            Rect::from_min_size(Pos2::new(rect.left(), top), vec2(COLUMN_WIDTH, ROW_HEIGHT));
        let active = session.state.active_layer == Some(row.id);
        if active {
            painter.rect_filled(row_rect, 0.0, visuals.selection.bg_fill.gamma_multiply(0.5));
        }
        let eye_rect = Rect::from_min_size(row_rect.min, vec2(TOGGLE_WIDTH, ROW_HEIGHT));
        let lock_rect = eye_rect.translate(vec2(TOGGLE_WIDTH, 0.0));
        let name_rect =
            Rect::from_x_y_ranges(lock_rect.right()..=row_rect.right(), row_rect.y_range());

        let (eye, eye_tip, eye_strong) = if row.visible {
            (icons::eye as IconFn, "Hide layer", true)
        } else {
            (icons::eye_closed as IconFn, "Show layer", false)
        };
        if icon_button(
            ui,
            eye_rect,
            ("layer_eye", row.id),
            eye,
            true,
            eye_strong,
            eye_tip,
        ) {
            commands.push(Box::new(SetLayerVisible::new(row.id, !row.visible)));
        }
        let (lock, lock_tip) = if row.locked {
            (icons::lock_closed as IconFn, "Unlock layer")
        } else {
            (icons::lock_open as IconFn, "Lock layer")
        };
        if icon_button(
            ui,
            lock_rect,
            ("layer_lock", row.id),
            lock,
            true,
            row.locked,
            lock_tip,
        ) {
            commands.push(Box::new(SetLayerLocked::new(row.id, !row.locked)));
        }

        if renaming.layer == Some(row.id) {
            let edit = ui.put(
                name_rect.shrink2(vec2(2.0, 1.0)),
                egui::TextEdit::singleline(&mut renaming.text).margin(vec2(3.0, 1.0)),
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
                if !name.is_empty() && name != row.name {
                    commands.push(Box::new(RenameLayer::new(row.id, name)));
                }
                renaming = Renaming::default();
            }
        } else {
            let response = ui
                .interact(
                    name_rect,
                    ui.scope_id().with(("layer_name", row.id)),
                    Sense::click_and_drag(),
                )
                .on_hover_text("Double-click or F2 to rename, drag to reorder");
            if response.hovered() && !active {
                painter.rect_filled(
                    name_rect,
                    0.0,
                    visuals.widgets.hovered.weak_bg_fill.gamma_multiply(0.5),
                );
            }
            if response.clicked() || response.drag_started() {
                select = Some(row.id);
            }
            if response.drag_started() {
                ui.data_mut(|d| d.insert_temp(drag_id, row.id));
            }
            if response.double_clicked() {
                renaming = Renaming {
                    layer: Some(row.id),
                    text: row.name.clone(),
                    focus_requested: false,
                };
            }
            if response.drag_stopped() {
                ui.data_mut(|d| d.remove::<LayerId>(drag_id));
                if let Some(y) = response.interact_pointer_pos().map(|p| p.y)
                    && let Some(to) =
                        reorder_target(i, drop_slot(y, rows_top, rows.len()), rows.len())
                {
                    commands.push(Box::new(MoveLayer::new(row.id, to)));
                }
            }
            let mut color = if active {
                visuals.strong_text_color()
            } else {
                visuals.text_color()
            };
            if !row.visible {
                color = color.gamma_multiply(0.4);
            }
            ui.painter_at(name_rect.shrink2(vec2(4.0, 0.0))).text(
                Pos2::new(name_rect.left() + 6.0, name_rect.center().y),
                egui::Align2::LEFT_CENTER,
                &row.name,
                egui::FontId::proportional(13.0),
                color,
            );
        }
        painter.vline(eye_rect.right(), row_rect.y_range(), line);
        painter.vline(lock_rect.right(), row_rect.y_range(), line);
        painter.hline(rect.x_range(), row_rect.bottom(), line);
    }
    painter.hline(rect.x_range(), header.bottom(), line);
    painter.vline(rect.right() - 0.5, rect.y_range(), line);

    // Where a dragged layer will land.
    let dragged: Option<LayerId> = ui.data(|d| d.get_temp(drag_id));
    if let Some(dragged) = dragged
        && let Some(from) = rows.iter().position(|r| r.id == dragged)
        && let Some(y) = ui.input(|i| i.pointer.latest_pos()).map(|p| p.y)
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let slot = drop_slot(y, rows_top, rows.len());
        if reorder_target(from, slot, rows.len()).is_some() {
            let y = rows_top + slot as f32 * ROW_HEIGHT;
            painter.hline(
                rect.x_range(),
                y,
                Stroke::new(2.0, visuals.selection.stroke.color),
            );
        }
    }

    ui.data_mut(|d| d.insert_temp(rename_id, renaming));
    if let Some(id) = select {
        session.select_layer(id);
    }
    for command in commands {
        if session.execute(command).is_err() {
            break;
        }
    }
    let layers = session.document.layers();
    let active = session.state.active_layer;
    let index = active.and_then(|id| layers.index_of(id));
    match (action, active, index) {
        (Some(Action::Add), _, _) => add_layer(session, index),
        (Some(Action::Remove), Some(id), _) => {
            let _ = session.execute(Box::new(RemoveLayer::new(id)));
        }
        (Some(Action::Raise), Some(id), Some(i)) => {
            let _ = session.execute(Box::new(MoveLayer::new(id, i + 1)));
        }
        (Some(Action::Lower), Some(id), Some(i)) if i > 0 => {
            let _ = session.execute(Box::new(MoveLayer::new(id, i - 1)));
        }
        _ => {}
    }
}

/// Show-all/lock-all toggles over the eye and lock columns, and the add,
/// remove and move buttons.
fn header_controls(
    ui: &mut egui::Ui,
    session: &Session,
    rows: &[Row],
    header: Rect,
    commands: &mut Vec<Box<dyn Command>>,
    action: &mut Option<Action>,
) {
    let any = !rows.is_empty();
    let eye_rect = Rect::from_min_size(header.min, vec2(TOGGLE_WIDTH, RULER_HEIGHT));
    let lock_rect = eye_rect.translate(vec2(TOGGLE_WIDTH, 0.0));
    let all_visible = rows.iter().all(|r| r.visible);
    let tip = if all_visible {
        "Hide all layers"
    } else {
        "Show all layers"
    };
    if icon_button(ui, eye_rect, "layers_eye_all", icons::eye, any, false, tip) {
        let show = !all_visible;
        let batch = rows
            .iter()
            .filter(|r| r.visible != show)
            .map(|r| Box::new(SetLayerVisible::new(r.id, show)) as Box<dyn Command>)
            .collect();
        let label = if show {
            "Show All Layers"
        } else {
            "Hide All Layers"
        };
        commands.push(Box::new(Batch::new(label, batch)));
    }
    let all_locked = any && rows.iter().all(|r| r.locked);
    let tip = if all_locked {
        "Unlock all layers"
    } else {
        "Lock all layers"
    };
    if icon_button(
        ui,
        lock_rect,
        "layers_lock_all",
        icons::lock_closed,
        any,
        false,
        tip,
    ) {
        let lock = !all_locked;
        let batch = rows
            .iter()
            .filter(|r| r.locked != lock)
            .map(|r| Box::new(SetLayerLocked::new(r.id, lock)) as Box<dyn Command>)
            .collect();
        let label = if lock {
            "Lock All Layers"
        } else {
            "Unlock All Layers"
        };
        commands.push(Box::new(Batch::new(label, batch)));
    }

    let layers = session.document.layers();
    let index = session
        .state
        .active_layer
        .and_then(|id| layers.index_of(id));
    let buttons: [(IconFn, &str, bool, Action); 4] = [
        (
            icons::plus,
            "Add a layer above the active one",
            true,
            Action::Add,
        ),
        (
            icons::trash,
            "Remove the active layer",
            index.is_some(),
            Action::Remove,
        ),
        (
            icons::chevron_up,
            "Move the active layer up",
            index.is_some_and(|i| i + 1 < layers.len()),
            Action::Raise,
        ),
        (
            icons::chevron_down,
            "Move the active layer down",
            index.is_some_and(|i| i > 0),
            Action::Lower,
        ),
    ];
    let mut x = header.right() - BUTTON_WIDTH * buttons.len() as f32 - 2.0;
    for (i, (icon, tip, enabled, act)) in buttons.into_iter().enumerate() {
        let rect =
            Rect::from_min_size(Pos2::new(x, header.top()), vec2(BUTTON_WIDTH, RULER_HEIGHT));
        if icon_button(ui, rect, ("layers_button", i), icon, enabled, false, tip) {
            *action = Some(act);
        }
        x += BUTTON_WIDTH;
    }
}

/// A clickable icon in `rect`. `strong` draws it at full contrast, otherwise
/// it is a little dimmed (an "off" toggle). Returns whether it was clicked.
fn icon_button(
    ui: &mut egui::Ui,
    rect: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    icon: IconFn,
    enabled: bool,
    strong: bool,
    tip: &str,
) -> bool {
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let response = ui.interact(rect, ui.scope_id().with(id), sense);
    let visuals = ui.visuals();
    let color = if !enabled {
        visuals.weak_text_color().gamma_multiply(0.5)
    } else if response.hovered() || strong {
        visuals.strong_text_color()
    } else {
        visuals.text_color().gamma_multiply(0.7)
    };
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect.shrink(2.0), 3.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let icon_rect = Rect::from_center_size(rect.center(), vec2(ICON_SIZE, ICON_SIZE));
    icon(&ui.painter_at(rect), icon_rect, color);
    let response = response.on_hover_text(tip);
    enabled && response.clicked()
}

/// The gap between rows nearest `y`: 0 is above the first row, `count` below
/// the last.
fn drop_slot(y: f32, rows_top: f32, count: usize) -> usize {
    ((y - rows_top) / ROW_HEIGHT)
        .round()
        .clamp(0.0, count as f32) as usize
}

/// The stack index (0 = bottom) to move row `from` to when dropped in gap
/// `slot`, or `None` if that leaves it where it is. Rows are top first.
fn reorder_target(from: usize, slot: usize, count: usize) -> Option<usize> {
    if slot == from || slot == from + 1 {
        return None;
    }
    let row = if slot > from { slot - 1 } else { slot };
    Some(count - 1 - row)
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
    fn reorder_targets() {
        // Rows top first: 0 is the top of a 3-layer stack (stack index 2).
        assert_eq!(reorder_target(0, 0, 3), None);
        assert_eq!(reorder_target(0, 1, 3), None);
        assert_eq!(reorder_target(0, 3, 3), Some(0)); // top to bottom
        assert_eq!(reorder_target(2, 0, 3), Some(2)); // bottom to top
        assert_eq!(reorder_target(2, 1, 3), Some(1));
        assert_eq!(drop_slot(-50.0, 0.0, 3), 0);
        assert_eq!(drop_slot(ROW_HEIGHT * 1.4, 0.0, 3), 1);
        assert_eq!(drop_slot(1000.0, 0.0, 3), 3);
    }

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
