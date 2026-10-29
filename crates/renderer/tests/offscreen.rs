//! Offscreen render + readback on a real GPU. Skips (passes) when no GPU
//! adapter is available, e.g. on headless CI.

#![allow(clippy::unwrap_used)]

use animation_core::{FrameIndex, Rgba, Vec2};
use document::drawing::{DrawingContent, Fill, FillShape, VectorDrawing};
use document::timeline::Keyframe;
use document::{CanvasSize, Document};
use renderer::{DrawListBuilder, OffscreenFrame, OffscreenRenderer, headless_device};

fn rect_fill(min: Vec2, max: Vec2, color: Rgba) -> Fill {
    let contour = vec![min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)];
    Fill::new(
        FillShape {
            contours: vec![contour],
        },
        color,
        1.0,
    )
}

/// A 100x50 document: opaque red on the left half, 50%-opacity blue on the
/// right half, and a hidden green layer over everything.
fn document() -> Document {
    let mut doc = Document::new(
        "T",
        CanvasSize {
            width: 100,
            height: 50,
        },
    );
    let layers = [
        (
            rect_fill(Vec2::ZERO, Vec2::new(50.0, 50.0), Rgba::rgb(1.0, 0.0, 0.0)),
            1.0,
            true,
        ),
        (
            rect_fill(
                Vec2::new(50.0, 0.0),
                Vec2::new(100.0, 50.0),
                Rgba::rgb(0.0, 0.0, 1.0),
            ),
            0.5,
            true,
        ),
        (
            rect_fill(Vec2::ZERO, Vec2::new(100.0, 50.0), Rgba::rgb(0.0, 1.0, 0.0)),
            1.0,
            false,
        ),
    ];
    for (fill, opacity, visible) in layers {
        let layer = doc.add_layer("L");
        let mut drawing = VectorDrawing::new();
        drawing.push_fill(fill);
        let id = doc.add_drawing(DrawingContent::Vector(drawing));
        doc.track_mut(layer)
            .unwrap()
            .set_key(FrameIndex(0), Keyframe::Drawing(id));
        let l = doc.layer_mut(layer).unwrap();
        l.opacity = opacity;
        l.visible = visible;
    }
    doc
}

fn pixel(p: &renderer::Pixels, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * p.width + x) * 4) as usize;
    p.rgba[i..i + 4].try_into().unwrap()
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2)
}

#[test]
fn renders_document_at_scale_and_reads_back_pixels() {
    let Ok((device, queue)) = pollster::block_on(headless_device()) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let doc = document();
    let mut offscreen = OffscreenRenderer::new(&device);
    let mut builder = DrawListBuilder::new(&device, offscreen.stroke_cache_mut());
    builder.render_list(&doc.evaluate(FrameIndex(0)));
    let draws = builder.finish();
    assert_eq!(draws.len(), 2, "hidden layer is excluded");

    // Transparent background at 50%: 50x25 pixels, straight alpha.
    let pixels = offscreen
        .render(
            &device,
            &queue,
            &OffscreenFrame {
                canvas_size: (100, 50),
                scale: 0.5,
                background: None,
                strokes: &draws,
            },
        )
        .unwrap();
    assert_eq!((pixels.width, pixels.height), (50, 25));
    assert_eq!(pixels.rgba.len(), 50 * 25 * 4);
    assert!(
        close(pixel(&pixels, 5, 12), [255, 0, 0, 255]),
        "{:?}",
        pixel(&pixels, 5, 12)
    );
    assert!(
        close(pixel(&pixels, 45, 12), [0, 0, 255, 128]),
        "{:?}",
        pixel(&pixels, 45, 12)
    );

    // White background at 200%: blue at 50% over white, blended in linear
    // space like the canvas (linear 0.5 encodes to sRGB 188).
    let pixels = offscreen
        .render(
            &device,
            &queue,
            &OffscreenFrame {
                canvas_size: (100, 50),
                scale: 2.0,
                background: Some(Rgba::WHITE),
                strokes: &draws,
            },
        )
        .unwrap();
    assert_eq!((pixels.width, pixels.height), (200, 100));
    assert!(close(pixel(&pixels, 10, 50), [255, 0, 0, 255]));
    assert!(
        close(pixel(&pixels, 190, 50), [188, 188, 255, 255]),
        "{:?}",
        pixel(&pixels, 190, 50)
    );
}
