//! The Brush Editor window (ADR-024): a brush's tip, grain and stamp
//! settings, edited on a draft and applied as one undo step.

use crate::brush_editor::{BrushEditor, BrushPictureTarget};
use crate::session::Session;
use document::animation_core::LayerId;
use egui::{Slider, vec2};

/// Side of the tip and grain thumbnails, in points.
const THUMB: f32 = 56.0;

/// Size of the scratchpad, in points.
const SCRATCHPAD: egui::Vec2 = vec2(
    crate::brush_editor::SCRATCHPAD_SIZE[0],
    crate::brush_editor::SCRATCHPAD_SIZE[1],
);

/// What the user asked for in the window, done once it is drawn.
enum Action {
    FromSelection(BrushPictureTarget),
    FromLayer(LayerId, BrushPictureTarget),
    FromFile(BrushPictureTarget),
    GenerateTip,
    GenerateGrain,
    Apply,
    Revert,
    Export,
    Close,
}

/// Draws the Brush Editor if it is open.
pub fn show(ctx: &egui::Context, session: &mut Session) {
    let Some(mut editor) = session.brush_editor.take() else {
        return;
    };
    let can_select = session.can_create_brush();
    let layers: Vec<(LayerId, String)> = session
        .document
        .layers()
        .iter()
        .rev()
        .filter(|l| !l.is_audio())
        .map(|l| (l.id, l.name.clone()))
        .collect();
    let dirty = session.brush_editor_differs(&editor);
    let in_library = session.document.asset(editor.asset).is_some();
    let mut actions = Vec::new();
    let mut edited = false;
    let mut open = true;
    egui::Window::new("Brush Editor")
        .open(&mut open)
        .resizable(false)
        .collapsible(false)
        .default_width(330.0)
        .show(ctx, |ui| {
            if editor.built_in {
                ui.label(
                    egui::RichText::new("Built-in brush: Apply saves your changes as a copy.")
                        .weak(),
                );
            }
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut editor.name).desired_width(200.0));
            });
            ui.separator();
            pictures(
                ui,
                &mut editor,
                can_select,
                &layers,
                &mut actions,
                &mut edited,
            );
            ui.separator();
            edited |= settings(ui, &mut editor);
            ui.separator();
            ui.label("Scratchpad");
            scratchpad(ui, &editor);
            ui.separator();
            ui.horizontal(|ui| {
                let apply = if editor.built_in {
                    "Save as Copy"
                } else {
                    "Apply"
                };
                if ui
                    .add_enabled(dirty || editor.built_in, egui::Button::new(apply))
                    .on_hover_text("Save the changes to the brush; one undo step")
                    .clicked()
                {
                    actions.push(Action::Apply);
                }
                if ui
                    .add_enabled(dirty, egui::Button::new("Revert"))
                    .on_hover_text("Throw away the changes not applied yet")
                    .clicked()
                {
                    actions.push(Action::Revert);
                }
                if ui
                    .add_enabled(in_library && !dirty, egui::Button::new("Export…"))
                    .on_hover_text("Save the brush as a .rustbrush file (apply changes first)")
                    .clicked()
                {
                    actions.push(Action::Export);
                }
                if ui.button("Close").clicked() {
                    actions.push(Action::Close);
                }
            });
        });
    session.brush_editor = Some(editor);
    if edited {
        session.brush_editor_edited();
    }
    if !open {
        actions.push(Action::Close);
    }
    for action in actions {
        match action {
            Action::FromSelection(target) => session.brush_picture_from_selection(target),
            Action::FromLayer(layer, target) => session.brush_picture_from_layer(layer, target),
            Action::FromFile(target) => session.brush_picture_from_file(target),
            Action::GenerateTip => session.generate_brush_tip(),
            Action::GenerateGrain => session.generate_brush_grain(),
            Action::Apply => session.apply_brush_editor(),
            Action::Revert => session.revert_brush_editor(),
            Action::Export => {
                if let Some(id) = session.brush_editor.as_ref().map(|e| e.asset) {
                    session.export_brush(id);
                }
            }
            Action::Close => session.close_brush_editor(),
        }
    }
}

/// The tip and grain thumbnails, each with a menu of where a new picture
/// comes from.
fn pictures(
    ui: &mut egui::Ui,
    editor: &mut BrushEditor,
    can_select: bool,
    layers: &[(LayerId, String)],
    actions: &mut Vec<Action>,
    edited: &mut bool,
) {
    ui.horizontal(|ui| {
        thumbnail(ui, editor.textures.tip, true);
        ui.vertical(|ui| {
            ui.strong("Tip");
            ui.label(
                egui::RichText::new(format!(
                    "{}×{} px · white paints",
                    editor.draft.tip.width, editor.draft.tip.height
                ))
                .weak(),
            );
            ui.menu_button("Change Tip", |ui| {
                sources(ui, BrushPictureTarget::Tip, can_select, layers, actions);
                ui.separator();
                ui.label("Generate a round or flat tip");
                ui.add(percent(&mut editor.hardness, "Hardness"))
                    .on_hover_text("How much of the tip is solid before it softens");
                ui.add(percent(&mut editor.aspect, "Roundness"))
                    .on_hover_text("100% is round; lower is a flat, chisel-like tip");
                if ui.button("Generate Tip").clicked() {
                    actions.push(Action::GenerateTip);
                    ui.close();
                }
            });
        });
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        thumbnail(ui, editor.textures.grain, editor.draft.grain.is_some());
        ui.vertical(|ui| {
            ui.strong("Paper grain");
            let info = match &editor.draft.grain {
                Some(g) => format!("{}×{} px · dark stays empty", g.width, g.height),
                None => "None: the paper takes paint evenly".to_owned(),
            };
            ui.label(egui::RichText::new(info).weak());
            ui.menu_button("Change Grain", |ui| {
                sources(ui, BrushPictureTarget::Grain, can_select, layers, actions);
                ui.separator();
                ui.add(percent(&mut editor.coarseness, "Coarseness"))
                    .on_hover_text("Size of the paper's tooth");
                if ui.button("Generate Paper Grain").clicked() {
                    actions.push(Action::GenerateGrain);
                    ui.close();
                }
                ui.separator();
                if ui
                    .add_enabled(editor.draft.grain.is_some(), egui::Button::new("No Grain"))
                    .clicked()
                {
                    editor.draft.grain = None;
                    *edited = true;
                    ui.close();
                }
            });
        });
    });
}

/// Menu items for a picture from the selection, a layer or a file.
fn sources(
    ui: &mut egui::Ui,
    target: BrushPictureTarget,
    can_select: bool,
    layers: &[(LayerId, String)],
    actions: &mut Vec<Action>,
) {
    if ui
        .add_enabled(can_select, egui::Button::new("From Selection"))
        .on_hover_text("Use the strokes, fills and pictures selected on the canvas")
        .clicked()
    {
        actions.push(Action::FromSelection(target));
        ui.close();
    }
    ui.menu_button("From Layer at Current Frame", |ui| {
        if layers.is_empty() {
            ui.label("No drawing layers");
        }
        for (id, name) in layers {
            if ui.button(name).clicked() {
                actions.push(Action::FromLayer(*id, target));
                ui.close();
            }
        }
    });
    if ui
        .button("Import Picture…")
        .on_hover_text("A PNG or JPEG picture")
        .clicked()
    {
        actions.push(Action::FromFile(target));
        ui.close();
    }
}

/// The stamp settings. Returns whether any changed.
fn settings(ui: &mut egui::Ui, editor: &mut BrushEditor) -> bool {
    let s = &mut editor.draft.settings;
    let has_grain = editor.draft.grain.is_some();
    let mut changed = false;
    egui::CollapsingHeader::new("Stroke")
        .default_open(true)
        .show(ui, |ui| {
            changed |= ui
                .add(
                    Slider::new(&mut s.spacing, 0.01..=4.0)
                        .logarithmic(true)
                        .text("Spacing")
                        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
                        .custom_parser(parse_percent),
                )
                .on_hover_text("Distance between stamps, as a share of the brush size")
                .changed();
            changed |= ui
                .add(percent(&mut s.flow, "Flow"))
                .on_hover_text("Opacity of one stamp; overlapping stamps build up")
                .changed();
        });
    egui::CollapsingHeader::new("Shape")
        .default_open(true)
        .show(ui, |ui| {
            changed |= ui
                .add(
                    Slider::new(&mut s.angle, -180.0..=180.0)
                        .text("Angle")
                        .suffix("°"),
                )
                .on_hover_text("Rotation of the tip")
                .changed();
            changed |= ui
                .checkbox(&mut s.follow_direction, "Follow stroke direction")
                .on_hover_text("Turn the tip with the line, for flat tips")
                .changed();
            changed |= ui
                .add(percent(&mut s.angle_jitter, "Angle jitter"))
                .on_hover_text("Random rotation per stamp")
                .changed();
            changed |= ui
                .add(percent(&mut s.size_jitter, "Size jitter"))
                .on_hover_text("Random shrink per stamp")
                .changed();
            changed |= ui
                .add(percent(&mut s.scatter, "Scatter"))
                .on_hover_text("Random sideways offset; stamps stay inside the line")
                .changed();
        });
    egui::CollapsingHeader::new("Opacity")
        .default_open(false)
        .show(ui, |ui| {
            changed |= ui
                .add(percent(&mut s.opacity_jitter, "Opacity jitter"))
                .on_hover_text("Random opacity drop per stamp")
                .changed();
            changed |= ui
                .add(percent(&mut s.opacity_pressure, "Pressure → opacity"))
                .on_hover_text(
                    "How much light pressure lowers opacity (pen pressure isn't read yet)",
                )
                .changed();
        });
    egui::CollapsingHeader::new("Paper grain")
        .default_open(has_grain)
        .show(ui, |ui| {
            ui.add_enabled_ui(has_grain, |ui| {
                changed |= ui
                    .add(
                        Slider::new(&mut s.grain_scale, 0.05..=64.0)
                            .logarithmic(true)
                            .text("Grain scale"),
                    )
                    .on_hover_text("Size of one grain pixel on the canvas")
                    .changed();
                changed |= ui
                    .add(percent(&mut s.grain_strength, "Grain strength"))
                    .on_hover_text("How much of the paper stays empty")
                    .changed();
            });
        });
    changed
}

/// A 0–100% slider for a `0.0..=1.0` value.
fn percent<'a>(value: &'a mut f32, text: &str) -> Slider<'a> {
    Slider::new(value, 0.0..=1.0)
        .text(text)
        .custom_formatter(|v, _| format!("{:.0}%", v * 100.0))
        .custom_parser(parse_percent)
}

fn parse_percent(s: &str) -> Option<f64> {
    s.trim()
        .trim_end_matches('%')
        .trim()
        .parse::<f64>()
        .ok()
        .map(|v| v / 100.0)
}

/// A picture the host renders, or an empty frame.
fn thumbnail(ui: &mut egui::Ui, texture: Option<egui::TextureId>, has_picture: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(THUMB, THUMB), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
    if let (Some(texture), true) = (texture, has_picture) {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(texture, rect.shrink(2.0), uv, egui::Color32::WHITE);
    }
    painter.rect_stroke(
        rect,
        4.0,
        ui.visuals().widgets.noninteractive.bg_stroke,
        egui::StrokeKind::Inside,
    );
}

/// The host's test stroke drawn with the draft.
fn scratchpad(ui: &mut egui::Ui, editor: &BrushEditor) {
    let (rect, _) = ui.allocate_exact_size(SCRATCHPAD, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, egui::Color32::WHITE);
    if let Some(texture) = editor.textures.scratchpad {
        let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(texture, rect, uv, egui::Color32::WHITE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use document::drawing::{BrushData, ImageData};

    #[test]
    fn percent_text_parses() {
        assert_eq!(parse_percent("50%"), Some(0.5));
        assert_eq!(parse_percent(" 25 "), Some(0.25));
        assert_eq!(parse_percent("x"), None);
    }

    #[test]
    fn the_window_draws_without_changing_the_document() {
        let mut s = Session::new();
        let id = s
            .import_brush(
                "Mine",
                BrushData {
                    tip: ImageData::new(4, 4, vec![1]),
                    ..BrushData::default()
                },
            )
            .unwrap();
        assert!(s.open_brush_editor(id));
        let before = s.document.clone();
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| show(ui.ctx(), &mut s));
            output.textures_delta.clear();
        }
        assert_eq!(s.document, before);
        assert!(s.brush_editor.is_some());
    }
}
