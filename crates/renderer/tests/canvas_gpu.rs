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
        id,
        color: Rgba::rgb(0.0, 0.0, 1.0),
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
