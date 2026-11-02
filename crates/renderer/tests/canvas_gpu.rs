//! Renders the canvas headlessly and checks pixels. Skips (passes) when the
//! machine has no GPU adapter, e.g. on CI runners without one.

use animation_core::glam::{Mat4, Vec2};
use animation_core::{Rgba, StrokeId};
use renderer::{CanvasFrame, CanvasRenderer, Mesh, StrokeDraw, StrokeKey, Viewport};

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()
}

/// Reads the canvas target back as tightly packed RGBA8 rows.
fn read_pixels(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    canvas: &CanvasRenderer,
) -> Vec<[u8; 4]> {
    let (width, height) = canvas.target().size();
    let row = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        canvas.target().texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |r| r.expect("map canvas readback"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll device");
    let data = buffer.get_mapped_range(..).expect("mapped range");
    let mut pixels = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let i = (y * row + x * 4) as usize;
            pixels.push([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        }
    }
    pixels
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 2)
}

#[test]
fn renders_workspace_background_and_strokes() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut canvas = CanvasRenderer::new(&device);
    // Canvas 100x50 in a 200x100 panel: fitted zoom 0.72, canvas at (64,32)-(136,68).
    let viewport = Viewport::new(Vec2::new(100.0, 50.0), Vec2::new(200.0, 100.0), 1.0);
    assert!(canvas.resize(&device, 200, 100));
    assert!(!canvas.resize(&device, 200, 100));

    let id = StrokeId::new();
    // A triangle covering the left half of the canvas.
    canvas
        .stroke_cache_mut()
        .ensure(&device, id, StrokeKey::from_raw(1), || {
            Some(Mesh {
                vertices: vec![[0.0, 0.0], [50.0, 0.0], [0.0, 50.0], [50.0, 50.0]],
                indices: vec![0, 1, 2, 1, 3, 2],
            })
        });
    let draws = [StrokeDraw {
        id: id.into(),
        color: Rgba::rgb(0.0, 0.0, 1.0),
        gradient: None,
        transform: Mat4::IDENTITY,
    }];
    let mut encoder = device.create_command_encoder(&Default::default());
    canvas.render(
        &device,
        &queue,
        &mut encoder,
        &viewport,
        &CanvasFrame {
            background: Some(Rgba::rgb(1.0, 0.0, 0.0)),
            strokes: &draws,
        },
    );
    queue.submit([encoder.finish()]);

    let pixels = read_pixels(&device, &queue, &canvas);
    let at = |x: usize, y: usize| pixels[y * 200 + x];
    assert!(
        close(at(120, 50), [255, 0, 0, 255]),
        "background: {:?}",
        at(120, 50)
    );
    assert!(
        close(at(80, 50), [0, 0, 255, 255]),
        "stroke: {:?}",
        at(80, 50)
    );
    // Far corner: plain workspace colour (dark, opaque, not red/blue).
    let corner = at(2, 2);
    assert_eq!(corner[3], 255);
    assert!(
        corner[0] < 60 && corner[0] == corner[1],
        "workspace: {corner:?}"
    );
    // Just below the canvas: darker than the workspace because of the shadow.
    assert!(
        at(100, 70)[0] < corner[0],
        "shadow: {:?} vs {corner:?}",
        at(100, 70)
    );
}

#[test]
fn transparent_background_draws_checkerboard() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut canvas = CanvasRenderer::new(&device);
    let viewport = Viewport::new(Vec2::new(100.0, 50.0), Vec2::new(200.0, 100.0), 1.0);
    canvas.resize(&device, 200, 100);
    let mut encoder = device.create_command_encoder(&Default::default());
    canvas.render(
        &device,
        &queue,
        &mut encoder,
        &viewport,
        &CanvasFrame::default(),
    );
    queue.submit([encoder.finish()]);
    let pixels = read_pixels(&device, &queue, &canvas);
    let at = |x: usize, y: usize| pixels[y * 200 + x];
    // Checker cells are 8 px, anchored at the canvas origin (64, 32).
    let (a, b) = (at(66, 34), at(74, 34));
    assert_ne!(a, b, "neighbouring cells differ");
    assert_eq!(a, at(74, 42), "diagonal cells match");
    assert!(a[0] > 150 && b[0] > 100, "light greys: {a:?} {b:?}");
}

#[test]
fn translucent_stroke_covers_each_pixel_once() {
    let Some((device, queue)) = device() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let mut canvas = CanvasRenderer::new(&device);
    let viewport = Viewport::new(Vec2::new(100.0, 50.0), Vec2::new(200.0, 100.0), 1.0);
    canvas.resize(&device, 200, 100);

    // One stroke made of two overlapping quads (x 0..30 and x 20..50), like
    // the overlapping segments of a tessellated stroke.
    let quad = |x0: f32, x1: f32, base: u32| {
        (
            vec![[x0, 0.0], [x1, 0.0], [x0, 50.0], [x1, 50.0]],
            vec![base, base + 1, base + 2, base + 1, base + 3, base + 2],
        )
    };
    let (mut vertices, mut indices) = quad(0.0, 30.0, 0);
    let (v2, i2) = quad(20.0, 50.0, 4);
    vertices.extend(v2);
    indices.extend(i2);
    let overlapping = StrokeId::new();
    let on_top = StrokeId::new();
    let cache = canvas.stroke_cache_mut();
    cache.ensure(&device, overlapping, StrokeKey::from_raw(1), || {
        Some(Mesh { vertices, indices })
    });
    cache.ensure(&device, on_top, StrokeKey::from_raw(2), || {
        let (vertices, indices) = quad(40.0, 50.0, 0);
        Some(Mesh { vertices, indices })
    });
    let half_blue = Rgba::new(0.0, 0.0, 1.0, 0.5);
    let draws = [
        StrokeDraw {
            id: overlapping.into(),
            color: half_blue,
            gradient: None,
            transform: Mat4::IDENTITY,
        },
        StrokeDraw {
            id: on_top.into(),
            color: half_blue,
            gradient: None,
            transform: Mat4::IDENTITY,
        },
    ];
    let mut encoder = device.create_command_encoder(&Default::default());
    canvas.render(
        &device,
        &queue,
        &mut encoder,
        &viewport,
        &CanvasFrame {
            background: Some(Rgba::WHITE),
            strokes: &draws,
        },
    );
    queue.submit([encoder.finish()]);

    // The viewport fits 100x50 into 200x100 with a margin; map canvas x to screen.
    let pixels = read_pixels(&device, &queue, &canvas);
    let (min, max) = viewport.canvas_screen_rect();
    let scale = (max.x - min.x) / 100.0;
    let at = |cx: f32| {
        let x = (min.x + cx * scale) as usize;
        let y = (min.y + 25.0 * scale) as usize;
        pixels[y * 200 + x]
    };
    let single = at(10.0);
    let overlap = at(25.0);
    let second = at(45.0);
    assert!(single[0] < 250, "stroke drawn: {single:?}");
    assert!(
        close(single, overlap),
        "overlap drawn once: {single:?} vs {overlap:?}"
    );
    assert!(
        second[0] + 20 < single[0],
        "a later stroke still blends on top: {second:?} vs {single:?}"
    );
}
