//! The editor shell: the `ui` panels around the canvas, plus canvas input.
//!
//! The canvas lives here rather than in `ui` because it needs the renderer's
//! [`Viewport`] to turn pointer positions into document coordinates.

use document::animation_core::glam::Vec2;
use document::animation_core::{DocumentId, Rgba, StrokeId};
use editor::Tool;
use renderer::Viewport;
use ui::Session;

/// Mouse-wheel zoom speed: zoom factor per logical point of scroll.
const WHEEL_ZOOM_SPEED: f32 = 0.0025;
/// egui's desktop line height for line-based wheel deltas, in points.
const WHEEL_LINE_POINTS: f32 = 40.0;

/// Debug toggles, read from the environment.
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
    /// The editing session: document, history, tools, playback.
    pub session: Session,
    /// Canvas placement; created on the first frame once the panel size is known.
    pub viewport: Option<Viewport>,
    /// Debug toggles.
    pub debug: DebugOptions,
    /// Stroke id used for the debug triangle's cached mesh.
    pub debug_triangle_id: StrokeId,
    /// The document the viewport was last fitted to.
    fitted_document: Option<DocumentId>,
}

impl Shell {
    /// A shell editing `session`.
    pub fn new(session: Session) -> Self {
        Self {
            session,
            viewport: None,
            debug: DebugOptions::from_env(),
            debug_triangle_id: StrokeId::new(),
            fitted_document: None,
        }
    }

    /// The canvas background to draw, after debug overrides.
    pub fn background(&self) -> Option<Rgba> {
        if self.debug.transparent {
            None
        } else {
            self.session.document.background
        }
    }

    /// Builds the whole UI for one frame. `canvas_texture` is the egui id of
    /// the renderer's canvas texture.
    pub fn ui(&mut self, ui: &mut egui::Ui, canvas_texture: egui::TextureId) {
        let Self {
            session,
            viewport,
            fitted_document,
            ..
        } = self;
        ui::show(ui, session, |ui, session| {
            canvas(ui, session, viewport, fitted_document, canvas_texture);
        });
    }
}

/// The canvas: shows the renderer's texture, handles pan/zoom and tool input.
fn canvas(
    ui: &mut egui::Ui,
    session: &mut Session,
    viewport: &mut Option<Viewport>,
    fitted_document: &mut Option<DocumentId>,
    canvas_texture: egui::TextureId,
) {
    let ppp = ui.ctx().pixels_per_point();
    // Snap the panel to whole physical pixels so the texture maps 1:1.
    let max = ui.max_rect();
    let min = (max.min.to_vec2() * ppp).round() / ppp;
    let size = (max.size() * ppp).round() / ppp;
    let rect = egui::Rect::from_min_size(min.to_pos2(), size);
    let screen_size = Vec2::new(rect.width(), rect.height());

    let doc_canvas = &session.document.canvas;
    let canvas_size = Vec2::new(doc_canvas.width as f32, doc_canvas.height as f32);
    let viewport = viewport.get_or_insert_with(|| Viewport::new(canvas_size, screen_size, ppp));
    if viewport.screen_size != screen_size || viewport.scale_factor != ppp {
        viewport.set_screen_size(screen_size, ppp);
    }
    viewport.canvas_size = canvas_size;
    // A newly opened or created document starts fitted.
    if *fitted_document != Some(session.document.id) {
        viewport.fit_canvas();
        *fitted_document = Some(session.document.id);
    }

    let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
    let panning = handle_view_input(ui, &response, rect, viewport, session.tools.is_active());
    if panning && ui.input(|i| i.key_down(egui::Key::Space)) {
        session.space_panned = true;
    }
    handle_tool_input(ui, &response, rect, viewport, session, panning);

    ui.painter().image(
        canvas_texture,
        rect,
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        egui::Color32::WHITE,
    );
    paint_overlays(ui, &response, rect, viewport, session, panning);
    view_controls(ui, rect, viewport);
}

/// Brush and eraser: press starts a gesture, every pointer move extends it,
/// release commits it. Positions go through `Viewport::screen_to_canvas`.
fn handle_tool_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: egui::Rect,
    viewport: &Viewport,
    session: &mut Session,
    panning: bool,
) {
    let to_canvas =
        |p: egui::Pos2| viewport.screen_to_canvas(Vec2::new(p.x - rect.min.x, p.y - rect.min.y));
    let typing = ui.ctx().egui_wants_keyboard_input();
    let (pressed, released, down, origin, space, moves) = ui.input(|i| {
        let moves: Vec<egui::Pos2> = i
            .events
            .iter()
            .filter_map(|e| match e {
                egui::Event::PointerMoved(p) => Some(*p),
                _ => None,
            })
            .collect();
        (
            i.pointer.primary_pressed(),
            i.pointer.primary_released(),
            i.pointer.primary_down(),
            i.pointer.press_origin(),
            i.key_down(egui::Key::Space) && !typing,
            moves,
        )
    });

    if pressed
        && response.hovered()
        && !space
        && !panning
        && !session.tools.is_active()
        && let Some(origin) = origin
    {
        session.pointer_down(to_canvas(origin), viewport.zoom as f32);
    }
    if session.tools.is_active() {
        for p in moves {
            session.pointer_move(to_canvas(p));
        }
        if released || !down {
            session.pointer_up();
        }
        ui.ctx().request_repaint();
    }
}

/// Brush/eraser cursor and the "can't draw here" banner.
fn paint_overlays(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: egui::Rect,
    viewport: &Viewport,
    session: &Session,
    panning: bool,
) {
    let painter = ui.painter_at(rect);
    let blocked = session.draw_block_reason();
    if let Some(reason) = &blocked {
        let text = format!("🔒 {reason} — drawing is disabled");
        let galley =
            painter.layout_no_wrap(text, egui::FontId::proportional(14.0), egui::Color32::WHITE);
        let pos = egui::pos2(rect.center().x - galley.size().x * 0.5, rect.top() + 12.0);
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand2(egui::vec2(10.0, 5.0));
        painter.rect_filled(
            bg,
            6.0,
            egui::Color32::from_rgba_unmultiplied(170, 40, 30, 220),
        );
        painter.galley(pos, galley, egui::Color32::WHITE);
    }

    let space = ui.input(|i| i.key_down(egui::Key::Space));
    if panning || space || !(response.hovered() || session.tools.is_active()) {
        return;
    }
    if blocked.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::NotAllowed);
        return;
    }
    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
    let Some(hover) = ui.input(|i| i.pointer.latest_pos()) else {
        return;
    };
    let zoom = viewport.zoom as f32;
    let radius = match session.state.tool {
        Tool::Brush => session.state.brush.width * 0.5 * zoom,
        Tool::Eraser => session.tools.eraser_radius * zoom,
        Tool::Fill => 0.0,
    };
    if radius >= 2.0 {
        painter.circle_stroke(
            hover,
            radius,
            egui::Stroke::new(1.5, egui::Color32::from_black_alpha(140)),
        );
        painter.circle_stroke(
            hover,
            radius,
            egui::Stroke::new(0.75, egui::Color32::from_white_alpha(200)),
        );
    }
}

/// Zoom readout with Fit and 100% buttons, floating at the canvas corner.
fn view_controls(ui: &egui::Ui, rect: egui::Rect, viewport: &mut Viewport) {
    egui::Area::new(egui::Id::unique("canvas_view_controls"))
        .fixed_pos(rect.left_bottom() + egui::vec2(8.0, -36.0))
        .order(egui::Order::Middle)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(format!("{:.0}%", viewport.zoom * 100.0));
                    if ui
                        .small_button("Fit")
                        .on_hover_text("F or Cmd/Ctrl+0")
                        .clicked()
                    {
                        viewport.fit_canvas();
                    }
                    if ui
                        .small_button("100%")
                        .on_hover_text("Cmd/Ctrl+1")
                        .clicked()
                    {
                        let centre = viewport.screen_size * 0.5;
                        viewport.set_zoom_about(centre, 1.0);
                    }
                });
            });
        });
}

/// Pan with middle-drag or Space+primary-drag; zoom about the cursor with the
/// wheel, pinch or Cmd/Ctrl+scroll; F / Cmd+0 fits, Cmd+1 is 100%. Returns
/// whether the view is being panned.
fn handle_view_input(
    ui: &egui::Ui,
    response: &egui::Response,
    rect: egui::Rect,
    viewport: &mut Viewport,
    drawing: bool,
) -> bool {
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
        || (space && !typing && !drawing && response.dragged_by(egui::PointerButton::Primary));
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
    panning
}
