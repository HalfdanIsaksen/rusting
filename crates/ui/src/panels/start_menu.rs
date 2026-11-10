//! The start menu: name, canvas size, frame rate and length of a new
//! document, with templates that fill them in. Shown instead of the editor
//! at launch and for File → New.

use super::shortcuts;
use crate::session::{DiscardAction, Session};
use document::Template;
use document::new_document::{MAX_CANVAS_SIDE, MAX_FPS, MAX_FRAME_COUNT};
use egui::{Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Vec2, vec2};

/// Size of a template card, in points.
const CARD_SIZE: Vec2 = vec2(150.0, 118.0);
/// Template cards per row.
const CARDS_PER_ROW: usize = 3;

/// Draws the start menu over the whole window.
pub fn show(ui: &mut egui::Ui, session: &mut Session) {
    let modal_open = session.dialog().is_some();
    let typing = ui.ctx().egui_wants_keyboard_input();
    let (open, quit, enter, escape) = ui.ctx().input_mut(|i| {
        (
            i.consume_shortcut(&shortcuts::OPEN),
            i.consume_shortcut(&shortcuts::QUIT),
            i.key_pressed(egui::Key::Enter),
            i.key_pressed(egui::Key::Escape),
        )
    });

    let mut create = false;
    let mut open_clicked = false;
    let mut cancel = false;
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(32.0);
                let (logo, _) = ui.allocate_exact_size(vec2(56.0, 56.0), Sense::hover());
                let colors = crate::logo::colors(crate::theme::palette(ui.visuals().dark_mode));
                crate::logo::paint(ui.painter(), logo, colors);
                ui.add_space(8.0);
                ui.heading(egui::RichText::new("Rusting").size(28.0).strong());
                ui.label("Start a new animation");
                ui.add_space(20.0);
            });
            let Some(menu) = session.start_menu_mut() else {
                return;
            };
            let width = CARD_SIZE.x * CARDS_PER_ROW as f32 + 8.0 * 2.0 + 300.0 + 32.0;
            let margin = ((ui.available_width() - width) * 0.5).max(0.0);
            ui.horizontal_top(|ui| {
                ui.add_space(margin);
                ui.vertical(|ui| {
                    ui.strong("Templates");
                    ui.add_space(4.0);
                    templates(ui, menu);
                });
                ui.add_space(32.0);
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    let actions = form(ui, menu);
                    create = actions.create;
                    open_clicked = actions.open;
                    cancel = actions.cancel;
                });
            });
        });
    });

    if !modal_open {
        // Enter in a text field also creates: the name is the last thing typed.
        if enter {
            create = true;
        }
        if escape && !typing {
            cancel = true;
        }
        if open {
            open_clicked = true;
        }
        if quit {
            session.request(DiscardAction::Quit);
        }
        if create {
            // Invalid settings stay in the form, which explains the problem.
            let _ = session.create_from_start_menu();
        } else if open_clicked {
            // Any unsaved changes were already confirmed by File → New.
            session.perform(DiscardAction::Open);
        } else if cancel {
            session.cancel_start_menu();
        }
    }
    super::dialogs::show(ui.ctx(), session);
}

/// The template cards plus "Custom".
fn templates(ui: &mut egui::Ui, menu: &mut crate::session::StartMenu) {
    let current = if menu.custom {
        None
    } else {
        menu.settings.template()
    };
    egui::Grid::new("start_menu_templates")
        .spacing(vec2(8.0, 8.0))
        .show(ui, |ui| {
            for (i, template) in Template::ALL.iter().enumerate() {
                let selected = current == Some(template);
                if card(ui, Some(template), selected).clicked() {
                    menu.settings.apply_template(template);
                    menu.custom = false;
                }
                if (i + 1) % CARDS_PER_ROW == 0 {
                    ui.end_row();
                }
            }
            if card(ui, None, current.is_none()).clicked() {
                menu.custom = true;
            }
        });
}

/// One template card: a thumbnail of the canvas shape, the name, size and
/// rate. `None` is the "Custom" card.
fn card(ui: &mut egui::Ui, template: Option<&Template>, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(CARD_SIZE, Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let visuals = ui.visuals();
    let widget = if response.hovered() {
        &visuals.widgets.hovered
    } else {
        &visuals.widgets.inactive
    };
    let accent = visuals.selection.stroke.color;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0, widget.weak_bg_fill);
    let border = if selected {
        Stroke::new(2.0, accent)
    } else {
        widget.bg_stroke
    };
    painter.rect_stroke(rect, 8.0, border, StrokeKind::Inside);

    // Thumbnail: the canvas shape scaled into a fixed box.
    let text = visuals.text_color();
    let weak = visuals.weak_text_color();
    let thumb_box = Rect::from_center_size(rect.center_top() + vec2(0.0, 32.0), vec2(64.0, 40.0));
    match template {
        Some(t) => {
            let aspect = t.width as f32 / t.height as f32;
            let size = if aspect >= thumb_box.aspect_ratio() {
                vec2(thumb_box.width(), thumb_box.width() / aspect)
            } else {
                vec2(thumb_box.height() * aspect, thumb_box.height())
            };
            let thumb = Rect::from_center_size(thumb_box.center(), size);
            let fill = if selected {
                accent.gamma_multiply(0.35)
            } else {
                Color32::from_gray(235).gamma_multiply(0.15)
            };
            painter.rect_filled(thumb, 2.0, fill);
            painter.rect_stroke(thumb, 2.0, Stroke::new(1.0, weak), StrokeKind::Inside);
        }
        None => {
            painter.rect_stroke(
                thumb_box.shrink2(vec2(10.0, 2.0)),
                2.0,
                Stroke::new(1.0, weak),
                StrokeKind::Inside,
            );
            painter.text(
                thumb_box.center(),
                Align2::CENTER_CENTER,
                "+",
                FontId::proportional(20.0),
                weak,
            );
        }
    }

    let (name, size, rate) = match template {
        Some(t) => (
            t.name.to_owned(),
            format!("{} × {}", t.width, t.height),
            format!("{} fps · {} frames", t.fps, t.frame_count),
        ),
        None => (
            "Custom".to_owned(),
            "Any size".to_owned(),
            "Set your own values".to_owned(),
        ),
    };
    let x = rect.center().x;
    painter.text(
        egui::pos2(x, rect.top() + 66.0),
        Align2::CENTER_TOP,
        name,
        FontId::proportional(14.0),
        text,
    );
    painter.text(
        egui::pos2(x, rect.top() + 84.0),
        Align2::CENTER_TOP,
        size,
        FontId::proportional(12.0),
        weak,
    );
    painter.text(
        egui::pos2(x, rect.top() + 99.0),
        Align2::CENTER_TOP,
        rate,
        FontId::proportional(12.0),
        weak,
    );
    response
}

/// Buttons pressed in the form this frame.
#[derive(Default)]
struct FormActions {
    create: bool,
    open: bool,
    cancel: bool,
}

/// Name, size, rate and length fields, and the Create / Open / Cancel buttons.
fn form(ui: &mut egui::Ui, menu: &mut crate::session::StartMenu) -> FormActions {
    let mut actions = FormActions::default();
    let s = &mut menu.settings;
    let focus_name = &mut menu.focus_name;
    ui.strong("Document");
    ui.add_space(4.0);
    egui::Grid::new("start_menu_form")
        .num_columns(2)
        .spacing(vec2(12.0, 8.0))
        .show(ui, |ui| {
            ui.label("Name");
            let name = ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(180.0));
            // Focus the name field when the menu first opens.
            if std::mem::take(focus_name) {
                name.request_focus();
            }
            ui.end_row();

            ui.label("Width");
            ui.add(
                egui::DragValue::new(&mut s.width)
                    .range(1..=MAX_CANVAS_SIDE)
                    .suffix(" px"),
            );
            ui.end_row();

            ui.label("Height");
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut s.height)
                        .range(1..=MAX_CANVAS_SIDE)
                        .suffix(" px"),
                );
                if ui
                    .small_button("⇄")
                    .on_hover_text("Swap width and height")
                    .clicked()
                {
                    std::mem::swap(&mut s.width, &mut s.height);
                }
            });
            ui.end_row();

            ui.label("Frame rate");
            ui.add(
                egui::DragValue::new(&mut s.fps)
                    .range(1..=MAX_FPS)
                    .suffix(" fps"),
            );
            ui.end_row();

            ui.label("Frames");
            ui.add(egui::DragValue::new(&mut s.frame_count).range(1..=MAX_FRAME_COUNT));
            ui.end_row();

            ui.label("Duration");
            ui.label(format!("{:.2} s", s.duration_secs()));
            ui.end_row();
        });

    ui.add_space(12.0);
    let valid = match s.validate() {
        Ok(()) => true,
        Err(err) => {
            ui.colored_label(ui.visuals().error_fg_color, err.to_string());
            ui.add_space(6.0);
            false
        }
    };
    ui.horizontal(|ui| {
        let create = egui::Button::new(egui::RichText::new("Create").strong())
            .min_size(vec2(90.0, 28.0))
            .fill(ui.visuals().selection.bg_fill);
        if ui.add_enabled(valid, create).clicked() {
            actions.create = true;
        }
        let open = egui::Button::new("Open…")
            .min_size(vec2(70.0, 28.0))
            .shortcut_text(ui.ctx().format_shortcut(&shortcuts::OPEN));
        if ui.add(open).clicked() {
            actions.open = true;
        }
        if menu.can_cancel
            && ui
                .add(egui::Button::new("Cancel").min_size(vec2(70.0, 28.0)))
                .clicked()
        {
            actions.cancel = true;
        }
    });
    actions
}
