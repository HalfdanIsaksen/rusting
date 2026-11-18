//! Brushed strokes (ADR-022) on a real GPU. Skips (passes) when no GPU
//! adapter is available, e.g. on headless CI.

#![allow(clippy::unwrap_used)]

use animation_core::{AssetId, FrameIndex, Rgba, Vec2};
use document::drawing::{
    BrushData, BrushSettings, BrushStyle, DrawingContent, Fill, FillShape, ImageData, Stroke,
    StrokePoint, VectorDrawing,
};
use document::timeline::Keyframe;
use document::{Asset, AssetKind, CanvasSize, Document};
use renderer::{DrawListBuilder, OffscreenFrame, OffscreenRenderer, Pixels, headless_device};

fn png(img: image::GrayImage) -> ImageData {
    let (w, h) = img.dimensions();
    let mut bytes = Vec::new();
    img.write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )
    .unwrap();
    ImageData::new(w, h, bytes)
}

/// A white disc fading to black at the edge.
fn disc_tip() -> ImageData {
    png(image::GrayImage::from_fn(32, 32, |x, y| {
        let d = Vec2::new(x as f32 - 15.5, y as f32 - 15.5).length() / 16.0;
        image::Luma([if d < 0.8 { 255 } else { 0 }])
    }))
}

/// Vertical stripes: every other grain pixel takes no paint.
fn striped_grain() -> ImageData {
    png(image::GrayImage::from_fn(8, 8, |x, _| {
        image::Luma([if x % 2 == 0 { 255 } else { 0 }])
    }))
}

/// A 200x100 document with one horizontal black stroke 40 wide across the
/// middle, drawn with a library brush made from `settings` and `grain`.
fn document(settings: BrushSettings, grain: Option<ImageData>) -> (Document, AssetId) {
    let mut doc = Document::new(
        "B",
        CanvasSize {
            width: 200,
            height: 100,
        },
    );
    let brush = Asset::new(
        "Test",
        AssetKind::Brush(BrushData {
            tip: disc_tip(),
            grain,
            settings,
        }),
    );
    let id = brush.id;
    doc.insert_asset(brush);
    let layer = doc.add_layer("L");
    let mut drawing = VectorDrawing::new();
    drawing.push_stroke(Stroke::new(
        vec![
            StrokePoint::new(Vec2::new(30.0, 50.0)),
            StrokePoint::new(Vec2::new(170.0, 50.0)),
        ],
        BrushStyle {
            color: Rgba::BLACK,
            width: 40.0,
            opacity: 1.0,
            brush: Some(id),
        },
    ));
    let d = doc.add_drawing(DrawingContent::Vector(drawing));
    doc.track_mut(layer)
        .unwrap()
        .set_key(FrameIndex(0), Keyframe::Drawing(d));
    (doc, id)
}

fn render(doc: &Document, device: &wgpu::Device, queue: &wgpu::Queue) -> Pixels {
    let mut offscreen = OffscreenRenderer::new(device);
    let mut builder =
        DrawListBuilder::new(device, offscreen.stroke_cache_mut()).with_assets(doc.assets());
    builder.render_list(&doc.evaluate(FrameIndex(0)));
    let draws = builder.finish();
    offscreen
        .render(
            device,
            queue,
            &OffscreenFrame {
                canvas_size: (200, 100),
                scale: 1.0,
                background: None,
                strokes: &draws,
            },
        )
        .unwrap()
}

fn alpha(p: &Pixels, x: u32, y: u32) -> u8 {
    p.rgba[((y * p.width + x) * 4 + 3) as usize]
}

#[test]
fn grain_breaks_up_the_line_and_stamps_build_up() {
    let Ok((device, queue)) = pollster::block_on(headless_device()) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    // Low flow: one stamp is faint, many overlapping stamps are dark.
    let settings = BrushSettings {
        spacing: 0.05,
        flow: 0.2,
        grain_scale: 4.0,
        grain_strength: 1.0,
        ..BrushSettings::default()
    };
    let (doc, _) = document(settings, Some(striped_grain()));
    let pixels = render(&doc, &device, &queue);

    // Along the middle of the stroke: paint where the grain takes it, none
    // (or nearly none, at stripe edges) where it doesn't.
    let row: Vec<u8> = (60..140).map(|x| alpha(&pixels, x, 50)).collect();
    let max = *row.iter().max().unwrap();
    let min = *row.iter().min().unwrap();
    assert!(max > 200, "overlapping stamps build up: {row:?}");
    assert!(min < 40, "grain leaves gaps: {row:?}");
    // Nothing outside the stroke.
    assert_eq!(alpha(&pixels, 100, 5), 0);
    assert_eq!(alpha(&pixels, 5, 50), 0);

    // The same document renders the same pixels again.
    assert_eq!(render(&doc, &device, &queue).rgba, pixels.rgba);
}

#[test]
fn without_grain_the_line_is_even_and_a_later_fill_covers_it() {
    let Ok((device, queue)) = pollster::block_on(headless_device()) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (mut doc, _) = document(BrushSettings::default(), None);
    let pixels = render(&doc, &device, &queue);
    let row: Vec<u8> = (60..140).map(|x| alpha(&pixels, x, 50)).collect();
    assert!(row.iter().all(|&a| a > 250), "{row:?}");

    // A white fill on a layer above covers the brushed stroke.
    let layer = doc.add_layer("Top");
    let mut drawing = VectorDrawing::new();
    let (min, max) = (Vec2::new(90.0, 0.0), Vec2::new(110.0, 100.0));
    drawing.push_fill(Fill::new(
        FillShape {
            contours: vec![vec![
                min,
                Vec2::new(max.x, min.y),
                max,
                Vec2::new(min.x, max.y),
            ]],
        },
        Rgba::WHITE,
        1.0,
    ));
    let d = doc.add_drawing(DrawingContent::Vector(drawing));
    doc.track_mut(layer)
        .unwrap()
        .set_key(FrameIndex(0), Keyframe::Drawing(d));
    let pixels = render(&doc, &device, &queue);
    let i = ((50 * pixels.width + 100) * 4) as usize;
    assert_eq!(&pixels.rgba[i..i + 4], &[255, 255, 255, 255]);
}

#[test]
fn a_missing_brush_draws_solid() {
    let Ok((device, queue)) = pollster::block_on(headless_device()) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let (mut doc, id) = document(BrushSettings::default(), Some(striped_grain()));
    doc.remove_asset(id);
    let pixels = render(&doc, &device, &queue);
    let row: Vec<u8> = (60..140).map(|x| alpha(&pixels, x, 50)).collect();
    assert!(row.iter().all(|&a| a > 250), "{row:?}");
}
