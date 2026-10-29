//! View → Preview: a floating window showing only the canvas content.

use crate::session::Session;
use egui::{Color32, Rect, Sense, vec2};

/// Checkerboard cell size behind transparent documents, in points.
const CHECKER: f32 = 8.0;

/// Draws the preview window if it is open. `texture` is the host's preview
/// image (`None` until the host has created it); `transparent` says the
/// document has no background colour. Returns the image size in physical
/// pixels the host should render, or `None` when the window is closed.
pub fn window(
    ctx: &egui::Context,
    session: &mut Session,
    texture: Option<egui::TextureId>,
    transparent: bool,
) -> Option<(u32, u32)> {
    if !session.preview.is_open() || session.start_menu().is_some() {
        return None;
    }
    let mut open = true;
    let mut size = None;
    egui::Window::new("Preview")
        .open(&mut open)
        .resizable(true)
        .collapsible(false)
        .default_size([360.0, 260.0])
        .min_size([160.0, 120.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let play = if session.preview.is_playing() {
                    "⏸ Pause"
                } else {
                    "▶ Play"
                };
                if ui
                    .add(egui::Button::new(play).min_size(vec2(70.0, 0.0)))
                    .on_hover_text("Loop the animation here without moving the playhead")
                    .clicked()
                {
                    session.toggle_preview_playback();
                }
                let frame = session.preview_frame();
                let count = session.document.frame_count.max(1);
                ui.label(format!("Frame {} / {count}", frame.0 + 1));
            });

            let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
            let canvas = &session.document.canvas;
            let ppp = ctx.pixels_per_point();
            let Some((image, pixels)) =
                fit(area, vec2(canvas.width as f32, canvas.height as f32), ppp)
            else {
                return;
            };
            let painter = ui.painter_at(area);
            if transparent {
                paint_checker(&painter, image);
            }
            if let Some(texture) = texture {
                let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                painter.image(texture, image, uv, Color32::WHITE);
            }
            size = Some(pixels);
        });
    if !open {
        session.preview.set_open(false);
    }
    size
}

/// The largest rect with the canvas's aspect ratio centred in `area`,
/// snapped to whole physical pixels, and its size in physical pixels.
fn fit(area: Rect, canvas: egui::Vec2, ppp: f32) -> Option<(Rect, (u32, u32))> {
    if area.width() < 1.0 || area.height() < 1.0 || canvas.x < 1.0 || canvas.y < 1.0 {
        return None;
    }
    let scale = (area.size() / canvas).min_elem();
    let pixels = (canvas * scale * ppp).round().max(vec2(1.0, 1.0));
    let min = ((area.center() - pixels / ppp * 0.5) * ppp).round() / ppp;
    let rect = Rect::from_min_size(min, pixels / ppp);
    Some((rect, (pixels.x as u32, pixels.y as u32)))
}

fn paint_checker(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(rect, 0.0, Color32::from_gray(204));
    let dark = Color32::from_gray(140);
    let (cols, rows) = (
        (rect.width() / CHECKER).ceil() as i32,
        (rect.height() / CHECKER).ceil() as i32,
    );
    for row in 0..rows {
        for col in (row % 2..cols).step_by(2) {
            let min = rect.min + vec2(col as f32, row as f32) * CHECKER;
            let cell = Rect::from_min_size(min, vec2(CHECKER, CHECKER)).intersect(rect);
            painter.rect_filled(cell, 0.0, dark);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect_ratio_and_centres() {
        let area = Rect::from_min_size(egui::pos2(10.0, 20.0), vec2(400.0, 300.0));
        let (rect, pixels) = fit(area, vec2(1920.0, 1080.0), 2.0).unwrap();
        assert_eq!(pixels, (800, 450));
        assert_eq!(rect.size(), vec2(400.0, 225.0));
        assert!((rect.center().y - area.center().y).abs() <= 0.5);
        assert!(fit(area, vec2(0.0, 10.0), 1.0).is_none());
    }
}
