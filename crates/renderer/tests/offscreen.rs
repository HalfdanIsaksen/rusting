//! Offscreen render + readback on a real GPU. Skips (passes) when no GPU
//! adapter is available, e.g. on headless CI.

#![allow(clippy::unwrap_used)]

use animation_core::{FrameIndex, Rgba, Vec2};
use document::drawing::{DrawingContent, Fill, FillShape, Gradient, VectorDrawing};
use document::scene::BlendMode;
use document::timeline::Keyframe;
use document::{CanvasAnchor, CanvasSize, Document};
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

#[test]
fn renders_resized_canvas_with_moved_artwork() {
    let Ok((device, queue)) = pollster::block_on(headless_device()) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut doc = document();
    let mut offscreen = OffscreenRenderer::new(&device);
    let render = |doc: &Document, offscreen: &mut OffscreenRenderer| {
        let mut builder = DrawListBuilder::new(&device, offscreen.stroke_cache_mut());
        builder.render_list(&doc.evaluate(FrameIndex(0)));
        let draws = builder.finish();
        offscreen
            .render(
                &device,
                &queue,
                &OffscreenFrame {
                    canvas_size: (doc.canvas.width, doc.canvas.height),
                    scale: 1.0,
                    background: None,
                    strokes: &draws,
                },
            )
            .unwrap()
    };
    // Warm the mesh cache at the old size.
    render(&doc, &mut offscreen);

    // Double the width anchored on the right: artwork moves 100 px right,
    // the way the editor's Resize Canvas moves it.
    let new = CanvasSize {
        width: 200,
        height: 50,
    };
    let offset = CanvasAnchor::Right.offset(doc.canvas, new);
    let ids: Vec<_> = doc.drawings().keys().copied().collect();
    for id in ids {
        doc.drawing_mut(id)
            .and_then(DrawingContent::as_vector_mut)
            .unwrap()
            .translate(offset);
    }
    doc.canvas = new;

    let pixels = render(&doc, &mut offscreen);
    assert_eq!((pixels.width, pixels.height), (200, 50));
    assert_eq!(pixel(&pixels, 20, 25)[3], 0, "new area is empty");
    assert!(close(pixel(&pixels, 120, 25), [255, 0, 0, 255]));
    assert!(close(pixel(&pixels, 180, 25), [0, 0, 255, 128]));
}

#[test]
fn renders_gradient_fill() {
    let Ok((device, queue)) = pollster::block_on(headless_device()) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut doc = Document::new(
        "T",
        CanvasSize {
            width: 100,
            height: 50,
        },
    );
    // Red at the top, blending to blue at the bottom.
    let fill = rect_fill(Vec2::ZERO, Vec2::new(100.0, 50.0), Rgba::rgb(1.0, 0.0, 0.0));
    let gradient = Some(Gradient {
        start: Vec2::new(50.0, 0.0),
        end: Vec2::new(50.0, 50.0),
        end_color: Rgba::rgb(0.0, 0.0, 1.0),
    });
    let layer = doc.add_layer("L");
    let mut drawing = VectorDrawing::new();
    drawing.push_fill(fill.with_gradient(gradient));
    let id = doc.add_drawing(DrawingContent::Vector(drawing));
    doc.track_mut(layer)
        .unwrap()
        .set_key(FrameIndex(0), Keyframe::Drawing(id));

    let mut offscreen = OffscreenRenderer::new(&device);
    let mut builder = DrawListBuilder::new(&device, offscreen.stroke_cache_mut());
    builder.render_list(&doc.evaluate(FrameIndex(0)));
    let draws = builder.finish();
    let pixels = offscreen
        .render(
            &device,
            &queue,
            &OffscreenFrame {
                canvas_size: (100, 50),
                scale: 1.0,
                background: None,
                strokes: &draws,
            },
        )
        .unwrap();
    let (top, mid, bottom) = (
        pixel(&pixels, 50, 0),
        pixel(&pixels, 50, 25),
        pixel(&pixels, 50, 49),
    );
    assert!(top[0] > 240 && top[2] < 30, "{top:?}");
    assert!(bottom[2] > 240 && bottom[0] < 30, "{bottom:?}");
    // Linear 0.5 encodes to sRGB 188.
    assert!(close(mid, [188, 0, 188, 255]), "{mid:?}");
}

/// Hard Light: Multiply or Screen by twice the source.
fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        cb * 2.0 * cs
    } else {
        let s = 2.0 * cs - 1.0;
        cb + s - cb * s
    }
}

/// The blend function B(cb, cs) of one straight linear channel, as the W3C
/// compositing spec defines it.
fn blend(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    match mode {
        BlendMode::Normal => cs,
        BlendMode::Multiply => cb * cs,
        BlendMode::Screen => cb + cs - cb * cs,
        BlendMode::Overlay => hard_light(cs, cb),
    }
}

fn srgb_byte(linear: f32) -> u8 {
    let c = if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round() as u8
}

#[test]
fn blend_modes_match_their_formulas() {
    let Ok((device, queue)) = pollster::block_on(headless_device()) else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let backdrop = Rgba::rgb(0.2, 0.5, 0.8);
    let source = Rgba::rgb(0.7, 0.3, 0.6);
    let mut offscreen = OffscreenRenderer::new(&device);
    for mode in BlendMode::ALL {
        // Backdrop on the left half, the blended layer over everything.
        let mut doc = Document::new(
            "T",
            CanvasSize {
                width: 20,
                height: 10,
            },
        );
        for (fill, blend) in [
            (
                rect_fill(Vec2::ZERO, Vec2::new(10.0, 10.0), backdrop),
                BlendMode::Normal,
            ),
            (rect_fill(Vec2::ZERO, Vec2::new(20.0, 10.0), source), mode),
        ] {
            let layer = doc.add_layer("L");
            let mut drawing = VectorDrawing::new();
            drawing.push_fill(fill);
            let id = doc.add_drawing(DrawingContent::Vector(drawing));
            doc.track_mut(layer)
                .unwrap()
                .set_key(FrameIndex(0), Keyframe::Drawing(id));
            doc.layer_mut(layer).unwrap().blend = blend;
        }
        let mut builder = DrawListBuilder::new(&device, offscreen.stroke_cache_mut());
        builder.render_list(&doc.evaluate(FrameIndex(0)));
        let draws = builder.finish();
        let pixels = offscreen
            .render(
                &device,
                &queue,
                &OffscreenFrame {
                    canvas_size: (20, 10),
                    scale: 1.0,
                    background: None,
                    strokes: &draws,
                },
            )
            .unwrap();

        let [br, bg, bb, _] = backdrop.to_array();
        let [sr, sg, sb, _] = source.to_array();
        let over = [(br, sr), (bg, sg), (bb, sb)].map(|(b, s)| srgb_byte(blend(mode, b, s)));
        let alone = [sr, sg, sb].map(srgb_byte);
        let (left, right) = (pixel(&pixels, 5, 5), pixel(&pixels, 15, 5));
        assert!(
            close(left, [over[0], over[1], over[2], 255]),
            "{mode:?} over backdrop: {left:?} != {over:?}"
        );
        assert!(
            close(right, [alone[0], alone[1], alone[2], 255]),
            "{mode:?} over transparent: {right:?} != {alone:?}"
        );
    }
}
