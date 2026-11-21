//! The Brush Editor's pictures (ADR-024): thumbnails of the draft's tip and
//! grain, and a scratchpad stroke drawn with the draft, rendered off-screen
//! whenever the draft changes.

use crate::brush_pictures;
use document::animation_core::{AssetId, FrameIndex, Rgba, Vec2};
use document::drawing::{BrushData, BrushStyle, ImageData, Stroke, StrokePoint};
use document::{Asset, AssetKind, CanvasSize, Document};
use editor::Command;
use editor::commands::{CopiedItems, paste_items};
use renderer::OffscreenRenderer;
use ui::Session;

/// The Brush Editor's textures, kept while it is open.
#[derive(Default)]
pub struct BrushEditorView {
    /// What the textures show: the editor's revision and the pixel density.
    shown: Option<(u64, u32)>,
    tip: Option<egui::TextureHandle>,
    grain: Option<egui::TextureHandle>,
    scratchpad: Option<egui::TextureHandle>,
}

impl BrushEditorView {
    /// Re-renders the pictures if the draft changed, and hands their
    /// textures to the editor. Drops them when the editor is closed.
    /// Returns whether anything was redrawn.
    pub fn update(
        &mut self,
        ctx: &egui::Context,
        session: &mut Session,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        offscreen: &mut Option<OffscreenRenderer>,
    ) -> bool {
        let Some(editor) = &session.brush_editor else {
            *self = Self::default();
            return false;
        };
        let ppp = ctx.pixels_per_point();
        let key = (editor.revision(), ppp.to_bits());
        if self.shown == Some(key) && editor.textures.scratchpad.is_some() {
            return false;
        }
        let draft = editor.draft.clone();
        let width = session.state.brush.width;
        self.tip = thumbnail(ctx, "brush-editor-tip", &draft.tip, true);
        self.grain = draft
            .grain
            .as_ref()
            .and_then(|g| thumbnail(ctx, "brush-editor-grain", g, false));
        let offscreen = offscreen.get_or_insert_with(|| OffscreenRenderer::new(device));
        self.scratchpad = match scratchpad(device, queue, offscreen, &draft, width, ppp) {
            Ok(image) => Some(ctx.load_texture(
                "brush-editor-scratchpad",
                image,
                egui::TextureOptions::LINEAR,
            )),
            Err(err) => {
                tracing::warn!("could not draw the brush scratchpad: {err}");
                None
            }
        };
        self.shown = Some(key);
        if let Some(editor) = &mut session.brush_editor {
            editor.textures = ui::BrushEditorTextures {
                tip: self.tip.as_ref().map(egui::TextureHandle::id),
                grain: self.grain.as_ref().map(egui::TextureHandle::id),
                scratchpad: self.scratchpad.as_ref().map(egui::TextureHandle::id),
            };
        }
        true
    }
}

/// A brush picture as a texture: a tip as dark paint on white (as it will
/// look on paper), a grain as it is.
fn thumbnail(
    ctx: &egui::Context,
    name: &str,
    data: &ImageData,
    tip: bool,
) -> Option<egui::TextureHandle> {
    let (w, h, rgba) = brush_pictures::decode(data).ok()?;
    let pixels = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            let v = if tip { 255 - p[0] } else { p[0] };
            egui::Color32::from_gray(v)
        })
        .collect();
    let image = egui::ColorImage::new([w as usize, h as usize], pixels);
    Some(ctx.load_texture(name, image, egui::TextureOptions::LINEAR))
}

/// The scratchpad's document: one S-shaped black stroke, `width` wide and
/// tapering at both ends, drawn with `brush`, on white paper the size of
/// the scratchpad in points.
fn scratchpad_document(brush: &BrushData, width: f32) -> Result<Document, String> {
    let [w, h] = ui::brush_editor::SCRATCHPAD_SIZE;
    let mut document = Document::new(
        "Scratchpad",
        CanvasSize {
            width: w as u32,
            height: h as u32,
        },
    );
    document.background = Some(Rgba::WHITE);
    let asset = Asset {
        id: AssetId::new(),
        name: "Draft".into(),
        kind: AssetKind::Brush(brush.clone()),
    };
    let id = asset.id;
    document.insert_asset(asset);
    let width = width.clamp(4.0, h * 0.4);
    let margin = width;
    let points = (0..=80)
        .map(|i| {
            let t = i as f32 / 80.0;
            let x = margin + t * (w - 2.0 * margin);
            let y = h / 2.0 + (t * std::f32::consts::TAU).sin() * (h / 2.0 - margin);
            StrokePoint {
                pos: Vec2::new(x, y),
                pressure: 0.35 + 0.65 * (t * std::f32::consts::PI).sin(),
            }
        })
        .collect();
    let stroke = Stroke::new(
        points,
        BrushStyle {
            color: Rgba::BLACK,
            width,
            opacity: 1.0,
            brush: Some(id),
        },
    );
    let layer = document.add_layer("Stroke");
    let items = CopiedItems {
        strokes: vec![stroke],
        ..CopiedItems::default()
    };
    let (mut command, _) = paste_items(layer, FrameIndex(0), &items);
    command.apply(&mut document).map_err(|e| e.to_string())?;
    Ok(document)
}

/// Renders the scratchpad at `ppp` pixels per point.
fn scratchpad(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    offscreen: &mut OffscreenRenderer,
    brush: &BrushData,
    width: f32,
    ppp: f32,
) -> Result<egui::ColorImage, String> {
    let document = scratchpad_document(brush, width)?;
    let image = crate::export::render_frame(
        device,
        queue,
        offscreen,
        &document,
        FrameIndex(0),
        ppp,
        document.background,
    )
    .map_err(|e| e.to_string())?;
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        [image.width as usize, image.height as usize],
        &image.pixels,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brush() -> BrushData {
        BrushData {
            tip: brush_pictures::ellipse_tip(0.5, 1.0),
            grain: None,
            settings: Default::default(),
        }
    }

    #[test]
    fn the_scratchpad_holds_one_brushed_stroke() {
        let doc = scratchpad_document(&brush(), 1000.0).unwrap();
        let list = doc.evaluate(FrameIndex(0));
        assert_eq!(list.layers.len(), 1);
        let [w, h] = ui::brush_editor::SCRATCHPAD_SIZE;
        assert_eq!((doc.canvas.width, doc.canvas.height), (w as u32, h as u32));
        let brushes: Vec<_> = doc.assets().values().filter_map(|a| a.brush()).collect();
        assert_eq!(brushes.len(), 1);
    }

    #[test]
    fn the_scratchpad_renders_paint_on_white() {
        let Ok(gpu) = pollster::block_on(renderer::headless_device()) else {
            eprintln!("skipped: no GPU adapter");
            return;
        };
        let mut offscreen = OffscreenRenderer::new(&gpu.0);
        let image = scratchpad(&gpu.0, &gpu.1, &mut offscreen, &brush(), 20.0, 1.0).unwrap();
        let dark = image.pixels.iter().filter(|p| p.r() < 64).count();
        let white = image.pixels.iter().filter(|p| p.r() > 250).count();
        assert!(dark > 100, "the stroke paints ({dark} dark pixels)");
        assert!(white > image.pixels.len() / 2, "the paper stays white");
        // Set RUSTING_BRUSH_OUT to a folder to look at the result.
        if let Some(dir) = std::env::var_os("RUSTING_BRUSH_OUT") {
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
            let [w, h] = image.size;
            image::RgbaImage::from_raw(w as u32, h as u32, rgba)
                .unwrap()
                .save(std::path::Path::new(&dir).join("scratchpad.png"))
                .unwrap();
        }
    }
}
