//! The layer column on the left of the timeline: one row per layer with
//! show/hide, lock, the name (click to select, Cmd/Ctrl+click or
//! Shift+click to select several, double-click or F2 to rename, drag to
//! reorder), the opacity and the blend mode, and a header with add, group,
//! ungroup, duplicate, remove, move up/down and show/lock-all toggles.
//!
//! Groups (folders) get a header row above their layers, with show/hide,
//! lock, a fold arrow, the name and the group opacity. Click a header to
//! select its layers, drag it to move the whole group, right-click it to
//! rename or ungroup. Drag a layer between a group's rows to put it in the
//! group, or out of them to take it out.

use super::timeline::{NUMBERS_HEIGHT, ROW_HEIGHT, RULER_HEIGHT, Row, RowKind};
use crate::icons::{self, IconFn};
use crate::session::Session;
use document::animation_core::{GroupId, LayerId};
use document::scene::{BlendMode, LayerStack};
use editor::Command;
use editor::commands::{
    AddLayer, Batch, DuplicateLayer, GroupLayers, MoveGroup, MoveLayer, RemoveLayer, RenameLayer,
    SetGroup, SetLayerBlend, SetLayerLocked, SetLayerVisible, SetOpacity, UngroupLayers,
};
use egui::{Pos2, Rect, Sense, Stroke, vec2};

/// Width of the layer column.
pub(super) const COLUMN_WIDTH: f32 = 316.0;
/// Width of the blend mode dropdown.
const BLEND_WIDTH: f32 = 90.0;
/// Width of the opacity field.
const OPACITY_WIDTH: f32 = 46.0;
/// Width of the eye and lock cells.
const TOGGLE_WIDTH: f32 = 22.0;
/// Width of a header button.
const BUTTON_WIDTH: f32 = 22.0;
/// Icon size inside a cell or button.
const ICON_SIZE: f32 = 14.0;
/// How far a group's layers are indented.
const INDENT: f32 = 14.0;
/// Width of a group header's fold arrow.
const FOLD_WIDTH: f32 = 16.0;

/// Rename-in-progress, kept in egui's temporary memory.
#[derive(Clone, Debug, Default)]
struct Renaming {
    target: Option<RowKind>,
    text: String,
    focus_requested: bool,
}

impl Renaming {
    fn start(row: &Row) -> Self {
        Self {
            target: Some(row.kind),
            text: row.name.clone(),
            focus_requested: false,
        }
    }
}

/// A header button's action.
#[derive(Clone, Copy)]
enum Action {
    Add,
    Group,
    Ungroup,
    Duplicate,
    Remove,
    Raise,
    Lower,
}

/// What a click on a row's name selects.
enum Select {
    Layer(LayerId, egui::Modifiers),
    Group(GroupId, egui::Modifiers),
}

/// Draws the layer column, `height` tall (ruler row plus one row per
/// timeline row). `rows` are top of the stack first.
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
    // The controls line up with the frame numbers, under the tag strip.
    let controls = Rect::from_x_y_ranges(
        header.x_range(),
        header.bottom() - NUMBERS_HEIGHT..=header.bottom(),
    );
    header_controls(ui, session, rows, controls, &mut commands, &mut action);

    let rename_id = ui.scope_id().with("layer_renaming");
    let mut renaming: Renaming = ui.data(|d| d.get_temp(rename_id)).unwrap_or_default();
    // F2 renames the active layer.
    let f2 = !ui.ctx().egui_wants_keyboard_input()
        && session.dialog().is_none()
        && ui.input(|i| i.key_pressed(egui::Key::F2));
    if f2
        && renaming.target.is_none()
        && let Some(row) = rows
            .iter()
            .find(|r| r.layer().is_some() && r.layer() == session.state.active_layer)
    {
        renaming = Renaming::start(row);
    }

    let drag_id = ui.scope_id().with("layer_drag");
    let mut select = None;
    let mut fold = None;
    let rows_top = header.bottom();
    for (i, row) in rows.iter().enumerate() {
        let top = rows_top + i as f32 * ROW_HEIGHT;
        let row_rect =
            Rect::from_min_size(Pos2::new(rect.left(), top), vec2(COLUMN_WIDTH, ROW_HEIGHT));
        let (active, selected) = match row.kind {
            RowKind::Layer(id) => (
                session.state.active_layer == Some(id),
                session.is_layer_selected(id),
            ),
            RowKind::Group(g) => (false, session.is_group_selected(g)),
        };
        if let RowKind::Group(_) = row.kind {
            painter.rect_filled(row_rect, 0.0, visuals.faint_bg_color);
        }
        if selected {
            let alpha = if active { 0.5 } else { 0.3 };
            painter.rect_filled(
                row_rect,
                0.0,
                visuals.selection.bg_fill.gamma_multiply(alpha),
            );
        }
        let eye_rect = Rect::from_min_size(row_rect.min, vec2(TOGGLE_WIDTH, ROW_HEIGHT));
        let lock_rect = eye_rect.translate(vec2(TOGGLE_WIDTH, 0.0));
        let blend_rect = Rect::from_x_y_ranges(
            row_rect.right() - BLEND_WIDTH..=row_rect.right(),
            row_rect.y_range(),
        );
        let opacity_rect = Rect::from_x_y_ranges(
            blend_rect.left() - OPACITY_WIDTH..=blend_rect.left(),
            row_rect.y_range(),
        );
        let indent = match row.kind {
            RowKind::Layer(_) if row.group.is_some() => INDENT,
            _ => 0.0,
        };
        let mut name_rect = Rect::from_x_y_ranges(
            lock_rect.right() + indent..=opacity_rect.left(),
            row_rect.y_range(),
        );
        if indent > 0.0 {
            // A guide line ties the layer to its group header.
            let x = lock_rect.right() + INDENT * 0.5;
            painter.vline(x, row_rect.y_range(), line);
        }

        if let Some((opacity, gesture)) =
            opacity_field(ui, opacity_rect, ("row_opacity", row.kind), row.opacity)
        {
            let command = match row.kind {
                RowKind::Layer(id) => SetOpacity::layer(id, opacity),
                RowKind::Group(g) => SetOpacity::group(g, opacity),
            };
            commands.push(Box::new(command.in_gesture(gesture)));
        }

        if let RowKind::Layer(id) = row.kind {
            let mut blend = row.blend;
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(blend_rect.shrink2(vec2(3.0, 1.0))),
                |ui| {
                    egui::ComboBox::from_id_salt(("layer_blend", id))
                        .selected_text(blend.label())
                        .width(BLEND_WIDTH - 6.0)
                        .show_ui(ui, |ui| {
                            for mode in BlendMode::ALL {
                                ui.selectable_value(&mut blend, mode, mode.label());
                            }
                        })
                        .response
                        .on_hover_text("Blend mode");
                },
            );
            if blend != row.blend {
                commands.push(Box::new(SetLayerBlend::new(id, blend)));
            }
        }

        let what = match row.kind {
            RowKind::Layer(_) => "layer",
            RowKind::Group(_) => "group",
        };
        let (eye, eye_tip, eye_strong) = if row.visible {
            (icons::eye as IconFn, format!("Hide {what}"), true)
        } else {
            (icons::eye_closed as IconFn, format!("Show {what}"), false)
        };
        if icon_button(
            ui,
            eye_rect,
            ("row_eye", row.kind),
            eye,
            true,
            eye_strong,
            &eye_tip,
        ) {
            commands.push(match row.kind {
                RowKind::Layer(id) => Box::new(SetLayerVisible::new(id, !row.visible)),
                RowKind::Group(g) => Box::new(SetGroup::visible(g, !row.visible)),
            });
        }
        let (lock, lock_tip) = if row.locked {
            (icons::lock_closed as IconFn, format!("Unlock {what}"))
        } else {
            (icons::lock_open as IconFn, format!("Lock {what}"))
        };
        if icon_button(
            ui,
            lock_rect,
            ("row_lock", row.kind),
            lock,
            true,
            row.locked,
            &lock_tip,
        ) {
            commands.push(match row.kind {
                RowKind::Layer(id) => Box::new(SetLayerLocked::new(id, !row.locked)),
                RowKind::Group(g) => Box::new(SetGroup::locked(g, !row.locked)),
            });
        }

        if let RowKind::Group(g) = row.kind {
            let fold_rect = Rect::from_min_size(name_rect.min, vec2(FOLD_WIDTH, ROW_HEIGHT));
            let (icon, tip) = if row.collapsed {
                (icons::chevron_right as IconFn, "Show the group's layers")
            } else {
                (icons::chevron_down as IconFn, "Fold the group")
            };
            if icon_button(ui, fold_rect, ("group_fold", g), icon, true, true, tip) {
                fold = Some(g);
            }
            let folder_rect = Rect::from_center_size(
                Pos2::new(fold_rect.right() + 8.0, fold_rect.center().y),
                vec2(ICON_SIZE, ICON_SIZE),
            );
            let color = if row.shown {
                visuals.text_color()
            } else {
                visuals.text_color().gamma_multiply(0.4)
            };
            icons::folder(&painter, folder_rect, color);
            name_rect.min.x = folder_rect.right() + 2.0;
        }

        if renaming.target == Some(row.kind) {
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
                    commands.push(match row.kind {
                        RowKind::Layer(id) => Box::new(RenameLayer::new(id, name)),
                        RowKind::Group(g) => Box::new(SetGroup::rename(g, name)),
                    });
                }
                renaming = Renaming::default();
            }
        } else {
            let tip = match row.kind {
                RowKind::Layer(_) => {
                    "Cmd/Ctrl+click or Shift+click to select several, \
                     double-click or F2 to rename, drag to reorder or to move \
                     into or out of a group"
                }
                RowKind::Group(_) => {
                    "Click to select the group's layers, double-click to rename, \
                     drag to move the group, right-click to ungroup"
                }
            };
            let response = ui
                .interact(
                    name_rect,
                    ui.scope_id().with(("row_name", row.kind)),
                    Sense::click_and_drag(),
                )
                .on_hover_text(tip);
            if response.hovered() && !selected {
                painter.rect_filled(
                    name_rect,
                    0.0,
                    visuals.widgets.hovered.weak_bg_fill.gamma_multiply(0.5),
                );
            }
            if response.clicked() || response.drag_started() {
                let modifiers = if response.drag_started() {
                    egui::Modifiers::NONE
                } else {
                    ui.input(|i| i.modifiers)
                };
                select = Some(match row.kind {
                    RowKind::Layer(id) => Select::Layer(id, modifiers),
                    RowKind::Group(g) => Select::Group(g, modifiers),
                });
            }
            if response.drag_started() {
                ui.data_mut(|d| d.insert_temp(drag_id, row.kind));
            }
            if response.double_clicked() {
                renaming = Renaming::start(row);
            }
            if let RowKind::Group(g) = row.kind {
                response.context_menu(|ui| {
                    if ui.button("Rename Group").clicked() {
                        renaming = Renaming::start(row);
                        ui.close();
                    }
                    if ui.button("Ungroup").clicked() {
                        commands.push(Box::new(UngroupLayers::new(g)));
                        ui.close();
                    }
                });
            }
            if response.drag_stopped() {
                ui.data_mut(|d| d.remove::<RowKind>(drag_id));
                if let Some(y) = response.interact_pointer_pos().map(|p| p.y) {
                    let slot = drop_slot(y, rows_top, rows.len());
                    let layers = session.document.layers();
                    match row.kind {
                        RowKind::Layer(id) => {
                            if let Some((to, group)) = layer_drop(layers, rows, id, slot) {
                                commands.push(Box::new(MoveLayer::into_group(id, to, group)));
                            }
                        }
                        RowKind::Group(g) => {
                            if let Some(to) = group_drop(layers, rows, g, slot) {
                                commands.push(Box::new(MoveGroup::new(g, to)));
                            }
                        }
                    }
                }
            }
            let mut color = if selected {
                visuals.strong_text_color()
            } else {
                visuals.text_color()
            };
            if !row.shown {
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
        painter.vline(opacity_rect.left(), row_rect.y_range(), line);
        painter.vline(blend_rect.left(), row_rect.y_range(), line);
        painter.hline(rect.x_range(), row_rect.bottom(), line);
    }
    painter.hline(rect.x_range(), header.bottom(), line);
    painter.vline(rect.right() - 0.5, rect.y_range(), line);

    // Where a dragged row will land; indented when it goes into a group.
    let dragged: Option<RowKind> = ui.data(|d| d.get_temp(drag_id));
    if let Some(dragged) = dragged
        && let Some(y) = ui.input(|i| i.pointer.latest_pos()).map(|p| p.y)
    {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        let slot = drop_slot(y, rows_top, rows.len());
        let layers = session.document.layers();
        let target = match dragged {
            RowKind::Layer(id) => layer_drop(layers, rows, id, slot).map(|(_, g)| g.is_some()),
            RowKind::Group(g) => group_drop(layers, rows, g, slot).map(|_| false),
        };
        if let Some(into_group) = target {
            let y = rows_top + slot as f32 * ROW_HEIGHT;
            let left = rect.left() + 2.0 * TOGGLE_WIDTH + if into_group { INDENT } else { 0.0 };
            painter.hline(
                left..=rect.right(),
                y,
                Stroke::new(2.0, visuals.selection.stroke.color),
            );
        }
    }

    ui.data_mut(|d| d.insert_temp(rename_id, renaming));
    if let Some(g) = fold
        && !session.timeline.collapsed_groups.remove(&g)
    {
        session.timeline.collapsed_groups.insert(g);
    }
    match select {
        Some(Select::Layer(id, m)) if m.command => session.toggle_layer_selected(id),
        Some(Select::Layer(id, m)) if m.shift => session.select_layer_range(id),
        Some(Select::Layer(id, _)) => session.select_layer(id),
        Some(Select::Group(g, m)) => session.select_group(g, m.command || m.shift),
        None => {}
    }
    for command in commands {
        if session.execute(command).is_err() {
            break;
        }
    }
    let layers = session.document.layers();
    let active = session.state.active_layer;
    let index = active.and_then(|id| layers.index_of(id));
    let active_group = active.and_then(|id| layers.get(id)).and_then(|l| l.group);
    match (action, active, index) {
        (Some(Action::Add), _, _) => add_layer(session, index),
        (Some(Action::Group), _, _) => group_layers(session),
        (Some(Action::Ungroup), _, _) => {
            if let Some(g) = active_group {
                let _ = session.execute(Box::new(UngroupLayers::new(g)));
            }
        }
        (Some(Action::Duplicate), _, _) => duplicate_layers(session),
        (Some(Action::Remove), _, _) => remove_layers(session),
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
    let rows: Vec<&Row> = rows.iter().filter(|r| r.layer().is_some()).collect();
    let layer_of = |r: &&Row| r.layer().unwrap_or_default();
    let any = !rows.is_empty();
    let eye_rect = Rect::from_min_size(header.min, vec2(TOGGLE_WIDTH, header.height()));
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
            .map(|r| Box::new(SetLayerVisible::new(layer_of(r), show)) as Box<dyn Command>)
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
            .map(|r| Box::new(SetLayerLocked::new(layer_of(r), lock)) as Box<dyn Command>)
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
    let in_group = session
        .state
        .active_layer
        .and_then(|id| layers.get(id))
        .is_some_and(|l| l.group.is_some());
    let buttons: [(IconFn, &str, bool, Action); 7] = [
        (
            icons::plus,
            "Add a layer above the active one",
            true,
            Action::Add,
        ),
        (
            icons::group_layers,
            "Group the selected layers into a folder",
            index.is_some(),
            Action::Group,
        ),
        (
            icons::ungroup_layers,
            "Ungroup the active layer's group (keeps its layers)",
            in_group,
            Action::Ungroup,
        ),
        (
            icons::duplicate,
            "Duplicate the selected layers",
            index.is_some(),
            Action::Duplicate,
        ),
        (
            icons::trash,
            "Remove the selected layers",
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
        let rect = Rect::from_min_size(
            Pos2::new(x, header.top()),
            vec2(BUTTON_WIDTH, header.height()),
        );
        if icon_button(ui, rect, ("layers_button", i), icon, enabled, false, tip) {
            *action = Some(act);
        }
        x += BUTTON_WIDTH;
    }
}

/// An opacity field in `rect` showing `opacity` as a percentage: drag it
/// sideways or click to type. Returns the new opacity and the gesture it
/// belongs to (the pass the drag or edit started in), so the caller can make
/// one undo step per gesture.
fn opacity_field(
    ui: &mut egui::Ui,
    rect: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    opacity: f32,
) -> Option<(f32, u64)> {
    let gesture_id = ui.scope_id().with(id).with("gesture");
    let mut percent = (opacity * 100.0).round();
    let before = percent;
    let response = ui
        .put(
            rect.shrink2(vec2(3.0, 2.0)),
            egui::DragValue::new(&mut percent)
                .range(0.0..=100.0)
                .speed(0.5)
                .fixed_decimals(0)
                .suffix("%"),
        )
        .on_hover_text("Opacity: drag sideways, or click to type");
    let mut gesture: Option<u64> = ui.data(|d| d.get_temp(gesture_id));
    let changed = percent != before;
    if changed && gesture.is_none() {
        gesture = Some(ui.ctx().cumulative_pass_nr());
    }
    if !(response.dragged() || response.has_focus()) {
        ui.data_mut(|d| d.remove::<u64>(gesture_id));
    } else if let Some(g) = gesture {
        ui.data_mut(|d| d.insert_temp(gesture_id, g));
    }
    let gesture = gesture?;
    changed.then_some((percent / 100.0, gesture))
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

/// The stack layer a drop in gap `slot` goes right above: the first layer
/// at or below the gap (for a group header, the group's top layer), skipping
/// `skip`'s layers. `None`: the drop goes to the bottom.
fn anchor_below(
    stack: &LayerStack,
    rows: &[Row],
    slot: usize,
    skip: Option<GroupId>,
) -> Option<usize> {
    rows.iter().skip(slot).find_map(|row| match row.kind {
        RowKind::Layer(id) => {
            let layer = stack.get(id)?;
            (skip.is_none() || layer.group != skip).then(|| stack.index_of(id))?
        }
        RowKind::Group(g) if Some(g) != skip => stack.group_range(g).map(|r| *r.end()),
        RowKind::Group(_) => None,
    })
}

/// The group a drop in gap `slot` lands in: the group of the layer row right
/// below the gap, or of an unfolded group header right above it.
fn group_at_gap(rows: &[Row], slot: usize) -> Option<GroupId> {
    let below = rows
        .get(slot)
        .filter(|r| r.layer().is_some())
        .and_then(|r| r.group);
    let header = slot
        .checked_sub(1)
        .and_then(|i| rows.get(i))
        .and_then(|r| match r.kind {
            RowKind::Group(g) if !r.collapsed => Some(g),
            _ => None,
        });
    below.or(header)
}

/// Where layer `id` goes when dropped in gap `slot` (0 = above the first
/// row): its new stack index (0 = bottom) and group, or `None` if it stays.
fn layer_drop(
    stack: &LayerStack,
    rows: &[Row],
    id: LayerId,
    slot: usize,
) -> Option<(usize, Option<GroupId>)> {
    let from = stack.index_of(id)?;
    let group = group_at_gap(rows, slot);
    let to = match anchor_below(stack, rows, slot, None) {
        None => 0,
        Some(i) if i == from => from,
        Some(i) if from < i => i,
        Some(i) => i + 1,
    };
    let current = stack.get(id)?.group;
    (to != from || group != current).then_some((to, group))
}

/// Where group `g` goes when its header is dropped in gap `slot`: the stack
/// index of its new bottom layer, counted without the group's layers, or
/// `None` if it stays or would land inside a group.
fn group_drop(stack: &LayerStack, rows: &[Row], g: GroupId, slot: usize) -> Option<usize> {
    if group_at_gap(rows, slot).is_some() {
        return None;
    }
    let range = stack.group_range(g)?;
    let size = range.end() - range.start() + 1;
    let to = match anchor_below(stack, rows, slot, Some(g)) {
        None => 0,
        Some(i) if i > *range.end() => i + 1 - size,
        Some(i) => i + 1,
    };
    (to != *range.start()).then_some(to)
}

/// Adds a layer just above `above` (or on top), in the same group, as one
/// undo step, and selects it.
fn add_layer(session: &mut Session, above: Option<usize>) {
    let layers = session.document.layers();
    let name = unique_name("Layer", layers.iter().map(|l| l.name.as_str()));
    let add = AddLayer::new(name);
    let id = add.layer_id();
    let top = layers.len();
    let group = above
        .and_then(|i| layers.iter().nth(i))
        .and_then(|l| l.group);
    let command: Box<dyn Command> = match above {
        Some(i) if i + 1 < top || group.is_some() => Box::new(Batch::new(
            "Add Layer",
            vec![
                Box::new(add),
                Box::new(MoveLayer::into_group(id, i + 1, group)),
            ],
        )),
        _ => Box::new(add),
    };
    if session.execute(command).is_ok() {
        session.select_layer(id);
    }
}

/// Puts every selected layer into a new group, as one undo step. The group
/// starts unfolded.
fn group_layers(session: &mut Session) {
    let selected = session.selected_layers();
    if selected.is_empty() {
        return;
    }
    let layers = session.document.layers();
    let name = unique_name("Group", layers.groups().map(|g| g.name.as_str()));
    let command = GroupLayers::new(selected, name);
    let group = command.group_id();
    if session.execute(Box::new(command)).is_ok() {
        session.timeline.collapsed_groups.remove(&group);
    }
}

/// Duplicates every selected layer right above itself, as one undo step,
/// and selects the copies.
fn duplicate_layers(session: &mut Session) {
    let selected = session.selected_layers();
    let commands: Vec<DuplicateLayer> =
        selected.iter().map(|&id| DuplicateLayer::new(id)).collect();
    let copies: Vec<LayerId> = commands.iter().map(DuplicateLayer::layer_id).collect();
    let active_copy = selected
        .iter()
        .position(|&id| Some(id) == session.state.active_layer)
        .map(|i| copies[i]);
    let label = if copies.len() == 1 {
        "Duplicate Layer"
    } else {
        "Duplicate Layers"
    };
    let batch = commands
        .into_iter()
        .map(|c| Box::new(c) as Box<dyn Command>)
        .collect();
    if session.execute(Box::new(Batch::new(label, batch))).is_ok()
        && let Some(active_copy) = active_copy
    {
        session.select_layers(active_copy, copies);
    }
}

/// Removes every selected layer, as one undo step.
fn remove_layers(session: &mut Session) {
    let batch: Vec<Box<dyn Command>> = session
        .selected_layers()
        .into_iter()
        .map(|id| Box::new(RemoveLayer::new(id)) as Box<dyn Command>)
        .collect();
    let label = if batch.len() == 1 {
        "Remove Layer"
    } else {
        "Remove Layers"
    };
    if !batch.is_empty() {
        let _ = session.execute(Box::new(Batch::new(label, batch)));
    }
}

/// "`prefix` N" with the smallest N above every existing "`prefix` N".
fn unique_name<'a>(prefix: &str, names: impl Iterator<Item = &'a str>) -> String {
    let highest = names
        .filter_map(|n| {
            n.strip_prefix(prefix)?
                .strip_prefix(' ')?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    format!("{prefix} {}", highest + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::timeline::rows;
    use std::collections::BTreeSet;

    /// Layers A..D, bottom to top, with B and C grouped (if `group`).
    fn stack(group: bool) -> (Session, Vec<LayerId>, Option<GroupId>) {
        let mut s = Session::new();
        let a = s.state.active_layer.unwrap();
        let mut ids = vec![a];
        for name in ["B", "C", "D"] {
            let add = AddLayer::new(name);
            ids.push(add.layer_id());
            s.execute(Box::new(add)).unwrap();
        }
        let g = group.then(|| {
            let command = GroupLayers::new(vec![ids[1], ids[2]], "G");
            let g = command.group_id();
            s.execute(Box::new(command)).unwrap();
            g
        });
        (s, ids, g)
    }

    #[test]
    fn flat_drops_reorder() {
        let (s, ids, _) = stack(false);
        let stack = s.document.layers();
        let rows = rows(&s.document, &BTreeSet::new());
        // Rows top first: D, C, B, A.
        assert_eq!(layer_drop(stack, &rows, ids[3], 0), None);
        assert_eq!(layer_drop(stack, &rows, ids[3], 1), None);
        assert_eq!(layer_drop(stack, &rows, ids[3], 4), Some((0, None))); // top to bottom
        assert_eq!(layer_drop(stack, &rows, ids[0], 0), Some((3, None))); // bottom to top
        assert_eq!(layer_drop(stack, &rows, ids[0], 2), Some((1, None))); // between B and C
        assert_eq!(drop_slot(-50.0, 0.0, 3), 0);
        assert_eq!(drop_slot(ROW_HEIGHT * 1.4, 0.0, 3), 1);
        assert_eq!(drop_slot(1000.0, 0.0, 3), 3);
    }

    #[test]
    fn drops_into_and_around_groups() {
        let (s, ids, g) = stack(true);
        let stack = s.document.layers();
        let rows = rows(&s.document, &BTreeSet::new());
        // Rows: D, [G], C, B, A.
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[1].kind, RowKind::Group(g.unwrap()));
        // D under the header: top of G.
        assert_eq!(layer_drop(stack, &rows, ids[3], 2), Some((3, g)));
        // A above the header: above the group, outside it.
        assert_eq!(layer_drop(stack, &rows, ids[0], 1), Some((2, None)));
        // C between B and A: below the group's
        // bottom layer is outside the group.
        assert_eq!(layer_drop(stack, &rows, ids[2], 4), Some((1, None)));
        // B out of G, to the bottom.
        assert_eq!(layer_drop(stack, &rows, ids[1], 5), Some((0, None)));

        let g = g.unwrap();
        assert_eq!(group_drop(stack, &rows, g, 0), Some(2)); // to the top
        assert_eq!(group_drop(stack, &rows, g, 5), Some(0)); // to the bottom
        assert_eq!(group_drop(stack, &rows, g, 1), None); // where it is
        assert_eq!(group_drop(stack, &rows, g, 3), None); // inside itself

        // Folded: the gap under the header is outside the group.
        let rows = super::super::timeline::rows(&s.document, &BTreeSet::from([g]));
        assert_eq!(rows.len(), 3);
        assert_eq!(layer_drop(stack, &rows, ids[3], 2), Some((1, None)));
    }

    #[test]
    fn grouping_and_adding_inside_a_group() {
        let (mut s, ids, g) = stack(true);
        let g = g.unwrap();
        s.select_layer(ids[1]);
        add_layer(&mut s, Some(1));
        let new = s.state.active_layer.unwrap();
        assert_eq!(s.document.layers().get(new).unwrap().group, Some(g));
        assert_eq!(s.document.layers().index_of(new), Some(2));
        s.undo();
        assert_eq!(s.document.layers().len(), 4);

        s.select_layer(ids[0]);
        s.toggle_layer_selected(ids[3]);
        group_layers(&mut s);
        let layers = s.document.layers();
        let group = layers.get(ids[0]).unwrap().group.unwrap();
        assert_eq!(layers.get(ids[3]).unwrap().group, Some(group));
        assert_eq!(layers.group(group).unwrap().name, "Group 1");
        assert_eq!(layers.check_groups(), Ok(()));

        s.select_group(g, false);
        assert!(s.is_group_selected(g));
        assert_eq!(s.state.active_layer, Some(ids[2]));
    }

    #[test]
    fn names_count_up() {
        assert_eq!(
            unique_name("Layer", ["Layer 1", "Ink", "Layer 3"].into_iter()),
            "Layer 4"
        );
        assert_eq!(unique_name("Layer", std::iter::empty()), "Layer 1");
        assert_eq!(
            unique_name("Group", ["Group 2", "Layer 7"].into_iter()),
            "Group 3"
        );
    }

    #[test]
    fn duplicate_and_remove_act_on_the_selection_in_one_step() {
        let mut s = Session::new();
        let bottom = s.state.active_layer.unwrap();
        add_layer(&mut s, Some(0));
        let top = s.state.active_layer.unwrap();
        s.toggle_layer_selected(bottom);
        assert_eq!(s.state.active_layer, Some(bottom));

        duplicate_layers(&mut s);
        let names: Vec<_> = s.document.layers().iter().map(|l| l.name.clone()).collect();
        assert_eq!(
            names,
            ["Layer 1", "Layer 1 copy", "Layer 2", "Layer 2 copy"]
        );
        let copies = s.selected_layers();
        assert_eq!(copies.len(), 2);
        assert!(!copies.contains(&top) && !copies.contains(&bottom));
        assert_eq!(
            s.state.active_layer,
            s.document.layers().iter().nth(1).map(|l| l.id)
        );

        remove_layers(&mut s);
        let order: Vec<_> = s.document.layers().iter().map(|l| l.id).collect();
        assert_eq!(order, [bottom, top]);
        s.undo();
        assert_eq!(s.document.layers().len(), 4);
        s.undo();
        assert_eq!(s.document.layers().len(), 2);
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
