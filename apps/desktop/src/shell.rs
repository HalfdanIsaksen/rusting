//! The egui shell: placeholder panels around the canvas, and canvas pan/zoom input.
//!
//! The panels are layout placeholders until the `ui` crate provides the real
//! toolbar, layer and timeline panels.

use document::Document;
use document::animation_core::glam::Vec2;
use document::animation_core::{Rgba, StrokeId};
use renderer::Viewport;

/// Mouse-wheel zoom speed: zoom factor per logical point of scroll.
const WHEEL_ZOOM_SPEED: f32 = 0.0025;
/// egui's desktop line height for line-based wheel deltas, in points.
const WHEEL_LINE_POINTS: f32 = 40.0;

/// Debug toggles shown in the shell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DebugOptions {
    /// Draw a hardcoded triangle through the stroke pipeline.
    pub test_triangle: bool,
    /// Ignore the document background and show the transparency checkerboard.
    pub transparent: bool,
}

impl DebugOptions {
    /// Reads `RUSTING_DEBUG_TRIANGLE` and `RUSTING_DEBUG_TRANSPARENT`.
    pub fn from_env() -> Self {
        let flag = |name| std::env::var_os(name).is_some_and(|v| v != "0" && !v.is_empty());
        Self {
            test_triangle: flag("RUSTING_DEBUG_TRIANGLE"),
            transparent: flag("RUSTING_DEBUG_TRANSPARENT"),
        }
    }
}

/// Editor state the shell shows and edits.
pub struct Shell {
    /// The open document.
    pub document: Document,
    /// Canvas placement; created on the first frame once the panel size is known.
    pub viewport: Option<Viewport>,
    /// Debug toggles.
    pub debug: DebugOptions,
    /// Stroke id used for the debug triangle's cached mesh.
    pub debug_triangle_id: StrokeId,
}

impl Shell {
    /// A shell showing `document`.
    pub fn new(document: Document) -> Self {
        Self {
            document,
            viewport: None,
            debug: DebugOptions::from_env(),
            debug_triangle_id: StrokeId::new(),
        }
    }

    /// The canvas background to draw, after debug overrides.
    pub fn background(&self) -> Option<Rgba> {
        if self.debug.transparent {
            None
        } else {
            self.document.background
        }
    }

    /// Builds the whole UI for one frame. `canvas_texture` is the egui id of
    /// the renderer's canvas texture.
    pub fn ui(&mut self, ui: &mut egui::Ui, canvas_texture: egui::TextureId) {
        egui::Panel::bottom("timeline_placeholder")
            .resizable(true)
            .min_size(140.0)
            .show(ui, |ui| {
                ui.heading("Timeline");
                ui.weak("Placeholder: the timeline panel arrives with the ui crate.");
            });

        egui::Panel::left("tools_placeholder")
            .resizable(true)
            .default_size(180.0)
            .show(ui, |ui| {
                ui.heading("Tools");
                ui.weak("Placeholder: the toolbar arrives with the ui crate.");
            });

        egui::Panel::right("properties_placeholder")
            .resizable(true)
            .default_size(220.0)
            .show(ui, |ui| {
                ui.heading("Layers");
                ui.weak("Placeholder: the layer panel arrives with the ui crate.");
                ui.separator();
                self.view_controls(ui);
            });

        egui::CentralPanel::no_frame().show(ui, |ui| self.canvas(ui, canvas_texture));
    }

    /// Zoom readout, fit/100% buttons and debug toggles.
    fn view_controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("View");
        if let Some(viewport) = &mut self.viewport {
            ui.label(format!("Zoom: {:.1}%", viewport.zoom * 100.0));
            ui.horizontal(|ui| {
                if ui.button("Fit").on_hover_text("F or Cmd/Ctrl+0").clicked() {
                    viewport.fit_canvas();
                }
                if ui.button("100%").on_hover_text("Cmd/Ctrl+1").clicked() {
                    let centre = viewport.screen_size * 0.5;
                    viewport.set_zoom_about(centre, 1.0);
                }
            });
        }
        ui.weak("Pan: middle-drag or Space+drag. Zoom: wheel or pinch.");
        ui.separator();
        ui.heading("Debug");
        ui.checkbox(&mut self.debug.test_triangle, "Test triangle");
        ui.checkbox(&mut self.debug.transparent, "Transparent background");
    }

    /// The canvas: shows the renderer's texture and handles pan/zoom input.
    fn canvas(&mut self, ui: &mut egui::Ui, canvas_texture: egui::TextureId) {
        let ppp = ui.ctx().pixels_per_point();
        // Snap the panel to whole physical pixels so the texture maps 1:1.
        let max = ui.max_rect();
        let min = (max.min.to_vec2() * ppp).round() / ppp;
        let size = (max.size() * ppp).round() / ppp;
        let rect = egui::Rect::from_min_size(min.to_pos2(), size);
        let screen_size = Vec2::new(rect.width(), rect.height());

        let canvas = &self.document.canvas;
        let canvas_size = Vec2::new(canvas.width as f32, canvas.height as f32);
        let viewport = self
            .viewport
            .get_or_insert_with(|| Viewport::new(canvas_size, screen_size, ppp));
        if viewport.screen_size != screen_size || viewport.scale_factor != ppp {
            viewport.set_screen_size(screen_size, ppp);
        }
        viewport.canvas_size = canvas_size;

        let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        handle_view_input(ui, &response, rect, viewport);

        ui.painter().image(
            canvas_texture,
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
}

/// Pan with middle-drag or Space+primary-drag; zoom about the cursor with the
/// wheel, pinch or Cmd/Ctrl+scroll; F / Cmd+0 fits, Cmd+1 is 100%.
fn handle_view_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: egui::Rect,
    viewport: &mut Viewport,
) {
    let (space, zoom_delta, wheel, hover, fit, actual) = ui.input(|i| {
        let wheel: f32 = i
            .events
            .iter()
            .filter_map(|event| match event {
                // Cmd/Ctrl+scroll is already turned into `zoom_delta` by egui.
                egui::Event::MouseWheel {
                    unit,
                    delta,
                    modifiers,
                    ..
                } if !modifiers.command => Some(match unit {
                    egui::MouseWheelUnit::Point => delta.y,
                    egui::MouseWheelUnit::Line => delta.y * WHEEL_LINE_POINTS,
                    egui::MouseWheelUnit::Page => delta.y * rect.height(),
                }),
                _ => None,
            })
            .sum();
        let fit = i.key_pressed(egui::Key::F) && i.modifiers.is_none()
            || i.modifiers.command && i.key_pressed(egui::Key::Num0);
        let actual = i.modifiers.command && i.key_pressed(egui::Key::Num1);
        (
            i.key_down(egui::Key::Space),
            i.zoom_delta(),
            wheel,
            i.pointer.hover_pos(),
            fit,
            actual,
        )
    });
    let typing = ui.ctx().egui_wants_keyboard_input();

    let panning = response.dragged_by(egui::PointerButton::Middle)
        || (space && !typing && response.dragged_by(egui::PointerButton::Primary));
    if panning {
        let d = response.drag_delta();
        viewport.pan_by(Vec2::new(d.x, d.y));
    }

    if response.hovered()
        && let Some(pos) = hover
    {
        let cursor = Vec2::new(pos.x - rect.min.x, pos.y - rect.min.y);
        if zoom_delta != 1.0 {
            viewport.zoom_about(cursor, zoom_delta);
        }
        if wheel != 0.0 {
            viewport.zoom_about(cursor, (wheel * WHEEL_ZOOM_SPEED).exp());
        }
    }

    if !typing && fit {
        viewport.fit_canvas();
    }
    if !typing && actual {
        viewport.set_zoom_about(viewport.screen_size * 0.5, 1.0);
    }

    if panning {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if space && response.hovered() && !typing {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
}
