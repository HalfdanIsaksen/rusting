//! The tool settings panel on the right of the canvas: settings for the
//! active tool (size, opacity, colour, draw order, eraser mode, gap closing)
//! and the
//! "drawing is blocked" warning.

use super::toolbar::TOOLS;
use crate::paint_order::PaintOrder;
use crate::session::Session;
use crate::shapes::GradientDirection;
use crate::tools::{EraserMode, FillStyle};
use document::animation_core::Rgba;
use document::drawing::GapClosing;
use editor::Tool;

/// Quick colour swatches (linear RGB).
const SWATCHES: [[f32; 3]; 8] = [
    [0.0, 0.0, 0.0],
    [1.0, 1.0, 1.0],
    [0.216, 0.216, 0.216],
    [0.8, 0.02, 0.02],
    [0.95, 0.4, 0.0],
    [0.9, 0.75, 0.0],
    [0.05, 0.45, 0.08],
    [0.02, 0.15, 0.75],
];

const GRADIENT_HINT: &str = "Click where the gradient starts (the brush colour), then where it \
                             ends (this colour). The area under the first click is filled. \
                             Esc cancels.";
const EYEDROPPER_HINT: &str = "Click the canvas to pick the colour of the topmost stroke or \
                               fill on the active layer (empty canvas picks the background). \
                               Sets the brush and paint bucket colour. Alt-click with the \
                               brush, paint bucket or a shape tool picks without switching.";
const PARTIAL_HINT: &str = "Cuts away the part of each stroke and fill it passes over on the \
                            active layer and frame.";
const STROKE_HINT: &str = "Removes every stroke and fill it touches on the active layer and frame.";
const TEXT_HINT: &str = "Click the canvas and type. Enter starts a new line; Cmd/Ctrl+Enter, a \
                         click elsewhere or another tool finishes the text, Esc cancels it. \
                         The text becomes vector shapes (one fill) in the brush colour: \
                         select, move, transform or erase it like any fill.";
const FILL_HINT: &str = "Fills the area enclosed by lines, with the brush colour and opacity. \
                         Click a fill to recolour it.";

/// Eraser mode choices as selectable labels, laid out in the current
/// direction. Disabled while a gesture is in progress. Returns whether the
/// user picked one.
pub(crate) fn eraser_mode_choices(ui: &mut egui::Ui, session: &mut Session) -> bool {
    let busy = session.tools.is_active();
    ui.add_enabled_ui(!busy, |ui| {
        let mode = &mut session.tools.eraser_mode;
        let partial = ui
            .selectable_value(mode, EraserMode::Partial, "Partial eraser")
            .on_hover_text(PARTIAL_HINT);
        let stroke = ui
            .selectable_value(mode, EraserMode::Stroke, "Stroke eraser")
            .on_hover_text(STROKE_HINT);
        partial.clicked() || stroke.clicked()
    })
    .inner
}

/// Normal or gradient fill as selectable labels. Returns whether the user
/// picked one.
pub(crate) fn fill_style_choices(ui: &mut egui::Ui, session: &mut Session) -> bool {
    let style = &mut session.tools.fill_style;
    let normal = ui.selectable_value(style, FillStyle::Normal, "Normal fill");
    let gradient = ui.selectable_value(style, FillStyle::Gradient, "Gradient fill");
    normal.clicked() || gradient.clicked()
}

/// Gap closing choices as selectable labels. Returns whether the user picked
/// one.
pub(crate) fn gap_choices(ui: &mut egui::Ui, session: &mut Session) -> bool {
    let gap = &mut session.tools.fill_gap;
    let mut picked = false;
    for option in GapClosing::ALL {
        picked |= ui.selectable_value(gap, option, option.label()).clicked();
    }
    picked
}

/// The "Draw order" choice: Behind, Normal or In front (ADR-010).
fn paint_order(ui: &mut egui::Ui, order: &mut PaintOrder) {
    ui.label("Draw order");
    ui.horizontal_wrapped(|ui| {
        for option in PaintOrder::ALL {
            ui.selectable_value(order, option, option.label())
                .on_hover_text(option.hint());
        }
    });
}

const MERGE_HINT: &str = "A new stroke or fill that touches fills of the same colour and \
                          opacity becomes one fill with them, so they select, move and erase \
                          as one shape. Existing strokes are never merged, so line art keeps \
                          working with the paint bucket.";

/// The "Merge same colour" checkbox (ADR-012).
fn merge_same_colour(ui: &mut egui::Ui, session: &mut Session) {
    ui.checkbox(&mut session.tools.merge_same_colour, "Merge same colour")
        .on_hover_text(MERGE_HINT);
}

/// Draws the tool settings panel.
pub fn panel(ui: &mut egui::Ui, session: &mut Session) {
    // Leave room for the value box beside each slider.
    ui.spacing_mut().slider_width = (ui.available_width() - 60.0).max(60.0);
    ui.horizontal(|ui| active_tool_label(ui, session));
    // The eyedropper only reads the canvas, so a locked layer is no problem.
    if let Some(reason) = session
        .draw_block_reason()
        .filter(|_| session.state.tool != Tool::Eyedropper)
    {
        ui.colored_label(ui.visuals().warn_fg_color, format!("⚠ {reason}"));
    }
    ui.separator();
    match session.state.tool {
        Tool::Select => {
            ui.weak(
                "Click or drag a box to select (Shift adds). Drag a selected item to move, \
                 arrows nudge (Shift ×10), Delete removes, ⌘A selects all, Esc deselects.",
            );
        }
        Tool::Lasso => {
            ui.weak(
                "Draw a loop around strokes and fills to select them (Shift adds). Drag a \
                 selected item to move, Delete removes, Esc deselects.",
            );
        }
        Tool::Transform => {
            ui.weak(
                "Drag a corner or edge to scale (Shift keeps proportions, Alt scales from \
                 the centre). Drag the knob or just outside a corner to rotate (Shift snaps \
                 to 15°). Drag inside to move; click elsewhere to select.",
            );
        }
        Tool::Brush => {
            brush_size(ui, session);
            brush_smoothing(ui, session);
            ui.separator();
            paint_order(ui, &mut session.tools.brush_order);
            merge_same_colour(ui, session);
            ui.separator();
            paint_settings(ui, session);
            ui.separator();
            ui.weak("Hold Shift while dragging to draw a straight line.");
        }
        Tool::Fill => {
            ui.label("Fill type");
            ui.horizontal(|ui| fill_style_choices(ui, session));
            if session.tools.fill_style == FillStyle::Gradient {
                ui.horizontal(|ui| {
                    ui.label("Gradient to").on_hover_text(GRADIENT_HINT);
                    color_button(ui, &mut session.tools.gradient_color);
                });
                ui.weak(GRADIENT_HINT);
            }
            ui.separator();
            ui.label("Close gaps").on_hover_text(FILL_HINT);
            gap_choices(ui, session);
            ui.separator();
            paint_order(ui, &mut session.tools.fill_order);
            merge_same_colour(ui, session);
            ui.separator();
            paint_settings(ui, session);
        }
        Tool::Rectangle | Tool::Ellipse => shape_settings(ui, session),
        Tool::Eyedropper => {
            ui.weak(EYEDROPPER_HINT);
            ui.separator();
            paint_settings(ui, session);
        }
        Tool::Text => {
            ui.label("Font size");
            ui.add(
                egui::Slider::new(&mut session.tools.text_size, 4.0..=500.0)
                    .logarithmic(true)
                    .max_decimals(1),
            )
            .on_hover_text("Height of one em in document units.");
            ui.separator();
            paint_order(ui, &mut session.tools.text_order);
            ui.separator();
            paint_settings(ui, session);
            ui.separator();
            ui.weak(TEXT_HINT);
        }
        Tool::Eraser => {
            eraser_size(ui, session);
            ui.separator();
            eraser_mode_choices(ui, session);
        }
    }
}

/// The active tool's icon and name, so the bar reads as belonging to it.
fn active_tool_label(ui: &mut egui::Ui, session: &Session) {
    let Some(entry) = TOOLS.iter().find(|e| e.tool == session.state.tool) else {
        return;
    };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    (entry.icon)(ui.painter(), rect, ui.visuals().strong_text_color());
    ui.strong(entry.name);
}

fn brush_size(ui: &mut egui::Ui, session: &mut Session) {
    ui.label("Size");
    ui.add(
        egui::Slider::new(&mut session.state.brush.width, 0.5..=200.0)
            .logarithmic(true)
            .max_decimals(1),
    );
}

const SMOOTHING_HINT: &str = "Irons out wobble from your hand or tablet. 0 keeps the line \
                              exactly as drawn; higher values give smoother, rounder curves.";

fn brush_smoothing(ui: &mut egui::Ui, session: &mut Session) {
    ui.label("Smoothing").on_hover_text(SMOOTHING_HINT);
    ui.add(
        egui::Slider::new(&mut session.tools.brush_smoothing, 0.0..=100.0)
            .max_decimals(0)
            .suffix("%"),
    )
    .on_hover_text(SMOOTHING_HINT);
}

const SHAPE_HINT: &str = "Drag to draw. Shift makes a square or circle, Alt draws from the \
                          centre.";

/// Stroke width and colour, and fill colour, of the rectangle and ellipse
/// tools.
fn shape_settings(ui: &mut egui::Ui, session: &mut Session) {
    let style = &mut session.tools.shape_style;
    ui.checkbox(&mut style.stroke, "Stroke");
    ui.add_enabled_ui(style.stroke, |ui| {
        ui.label("Stroke width");
        ui.add(
            egui::Slider::new(&mut style.stroke_width, 0.5..=200.0)
                .logarithmic(true)
                .max_decimals(1),
        );
        ui.horizontal(|ui| {
            ui.label("Stroke colour");
            color_button(ui, &mut style.stroke_color);
        });
        swatches(ui, &mut style.stroke_color);
    });
    ui.separator();
    ui.checkbox(&mut style.fill, "Fill");
    ui.add_enabled_ui(style.fill, |ui| {
        ui.horizontal(|ui| {
            ui.label("Fill colour");
            color_button(ui, &mut style.fill_color);
        });
        swatches(ui, &mut style.fill_color);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut style.gradient, false, "Solid");
            ui.selectable_value(&mut style.gradient, true, "Gradient");
        });
        if style.gradient {
            ui.horizontal(|ui| {
                ui.label("Gradient to")
                    .on_hover_text("The fill blends from the fill colour into this one.");
                color_button(ui, &mut style.gradient_color);
            });
            swatches(ui, &mut style.gradient_color);
            ui.label("Direction");
            ui.horizontal_wrapped(|ui| {
                for direction in GradientDirection::ALL {
                    ui.selectable_value(
                        &mut style.gradient_direction,
                        direction,
                        direction.label(),
                    );
                }
            });
        }
    });
    ui.separator();
    paint_order(ui, &mut style.order);
    merge_same_colour(ui, session);
    ui.separator();
    ui.weak(SHAPE_HINT);
}

fn eraser_size(ui: &mut egui::Ui, session: &mut Session) {
    ui.label("Size");
    let mut diameter = session.tools.eraser_radius * 2.0;
    if ui
        .add(
            egui::Slider::new(&mut diameter, 1.0..=400.0)
                .logarithmic(true)
                .max_decimals(1),
        )
        .changed()
    {
        session.tools.eraser_radius = diameter * 0.5;
    }
}

/// Opacity, colour and swatches, shared by the brush and the paint bucket.
fn paint_settings(ui: &mut egui::Ui, session: &mut Session) {
    ui.label("Opacity");
    ui.add(egui::Slider::new(
        &mut session.state.brush.opacity,
        0.05..=1.0,
    ));
    ui.horizontal(|ui| {
        ui.label("Colour");
        color_button(ui, &mut session.state.brush.color);
    });
    swatches(ui, &mut session.state.brush.color);
}

/// The quick colour swatches; clicking one sets `color`.
fn swatches(ui: &mut egui::Ui, color: &mut Rgba) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
        for [r, g, b] in SWATCHES {
            let swatch = egui::Rgba::from_rgb(r, g, b);
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 3.0, swatch);
            ui.painter().rect_stroke(
                rect,
                3.0,
                ui.visuals().widgets.noninteractive.bg_stroke,
                egui::StrokeKind::Inside,
            );
            if response.clicked() {
                *color = Rgba::rgb(r, g, b);
            }
        }
    });
}

/// A colour picker button for an RGB colour, keeping its alpha.
fn color_button(ui: &mut egui::Ui, color: &mut Rgba) {
    let Rgba { r, g, b, a } = *color;
    let mut rgb = [r, g, b];
    if ui.color_edit_button_rgb(&mut rgb).changed() {
        *color = Rgba::new(rgb[0], rgb[1], rgb[2], a);
    }
}
