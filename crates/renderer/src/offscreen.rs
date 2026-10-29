//! Rendering a frame at document resolution and reading the pixels back.
//!
//! Export uses this: the same pipelines, mesh cache and colour handling as
//! the editor canvas (4x MSAA, linear-space blending into an sRGB target),
//! but rendered at `canvas size × scale` with no workspace, shadow, outline
//! or checkerboard.

use crate::canvas::{CanvasFrame, CanvasRenderer, StrokeDraw};
use crate::gpu::RendererError;
use crate::stroke_cache::StrokeCache;
use crate::viewport::Viewport;
use animation_core::Rgba;
use animation_core::glam::{DVec2, Vec2};

/// One frame to render offscreen.
#[derive(Clone, Copy, Debug)]
pub struct OffscreenFrame<'a> {
    /// Document canvas size in document units.
    pub canvas_size: (u32, u32),
    /// Output pixels per document unit (1.0 = document resolution).
    pub scale: f32,
    /// Canvas colour; `None` leaves the background transparent.
    pub background: Option<Rgba>,
    /// What to draw, bottom to top (see [`crate::DrawListBuilder`]).
    pub strokes: &'a [StrokeDraw],
}

/// Pixels read back from the GPU: straight (not premultiplied) alpha,
/// sRGB-encoded RGBA8, top row first, no row padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pixels {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes.
    pub rgba: Vec<u8>,
}

/// Output size for a canvas at `scale`: rounded, at least 1x1.
pub fn output_size((width, height): (u32, u32), scale: f32) -> (u32, u32) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let size = |n: u32| ((n as f32 * scale).round() as u32).max(1);
    (size(width), size(height))
}

/// Renders frames into its own target and reads them back. It keeps its own
/// mesh cache and uniform buffers, so it never disturbs the editor canvas.
#[derive(Debug)]
pub struct OffscreenRenderer {
    canvas: CanvasRenderer,
    readback: Option<(wgpu::Buffer, u64)>,
}

impl OffscreenRenderer {
    /// Creates the pipelines.
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            canvas: CanvasRenderer::new(device),
            readback: None,
        }
    }

    /// The mesh cache draws must be prepared in (see
    /// [`crate::DrawListBuilder`]).
    pub fn stroke_cache_mut(&mut self) -> &mut StrokeCache {
        self.canvas.stroke_cache_mut()
    }

    /// Renders `frame`, waits for the GPU and returns the pixels.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &OffscreenFrame<'_>,
    ) -> Result<Pixels, RendererError> {
        let (width, height) = output_size(frame.canvas_size, frame.scale);
        let max = device.limits().max_texture_dimension_2d;
        if width > max || height > max {
            return Err(RendererError::TooLarge { width, height, max });
        }
        self.canvas.resize(device, width, height);

        // The canvas fills the target exactly: no pan, one zoom step per scale.
        let canvas_size = Vec2::new(frame.canvas_size.0 as f32, frame.canvas_size.1 as f32);
        let viewport = Viewport {
            canvas_size,
            zoom: f64::from(width) / f64::from(frame.canvas_size.0.max(1)),
            pan: DVec2::ZERO,
            screen_size: Vec2::new(width as f32, height as f32),
            scale_factor: 1.0,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offscreen"),
        });
        self.canvas.render_bare(
            device,
            queue,
            &mut encoder,
            &viewport,
            &CanvasFrame {
                background: frame.background,
                strokes: frame.strokes,
            },
        );

        let row_bytes = width * 4;
        let padded_row = padded_bytes_per_row(width);
        let size = u64::from(padded_row) * u64::from(height);
        let buffer = match &self.readback {
            Some((buffer, capacity)) if *capacity >= size => buffer.clone(),
            _ => {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("offscreen-readback"),
                    size,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                self.readback = Some((buffer.clone(), size));
                buffer
            }
        };
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: self.canvas.target().texture(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
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

        let slice = buffer.slice(..size);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| RendererError::ReadBack(e.to_string()))?;
        rx.recv()
            .map_err(|e| RendererError::ReadBack(e.to_string()))?
            .map_err(|e| RendererError::ReadBack(e.to_string()))?;
        let mut rgba = Vec::with_capacity(row_bytes as usize * height as usize);
        {
            let view = slice
                .get_mapped_range()
                .map_err(|e| RendererError::ReadBack(e.to_string()))?;
            for row in view.chunks(padded_row as usize).take(height as usize) {
                rgba.extend_from_slice(&row[..row_bytes as usize]);
            }
        }
        buffer.unmap();
        unpremultiply_srgb(&mut rgba);
        Ok(Pixels {
            width,
            height,
            rgba,
        })
    }
}

/// `width * 4` rounded up to wgpu's copy row alignment (256 bytes).
pub fn padded_bytes_per_row(width: u32) -> u32 {
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    (width * 4).div_ceil(align) * align
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Converts pixels from the canvas target's format to straight alpha.
///
/// Strokes blend onto a transparent clear in linear space, so the target
/// holds linear *premultiplied* colour, sRGB-encoded. Opaque pixels are
/// unchanged; translucent ones are decoded, divided by alpha and re-encoded.
pub fn unpremultiply_srgb(rgba: &mut [u8]) {
    let decode: Vec<f32> = (0..=255u8)
        .map(|v| srgb_to_linear(f32::from(v) / 255.0))
        .collect();
    for p in rgba.as_chunks_mut::<4>().0 {
        match p[3] {
            255 => {}
            0 => p[..3].fill(0),
            a => {
                let a = f32::from(a) / 255.0;
                for c in &mut p[..3] {
                    let straight = (decode[usize::from(*c)] / a).min(1.0);
                    *c = (linear_to_srgb(straight) * 255.0).round() as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_padded_to_256_bytes() {
        assert_eq!(padded_bytes_per_row(1), 256);
        assert_eq!(padded_bytes_per_row(64), 256);
        assert_eq!(padded_bytes_per_row(65), 512);
        assert_eq!(padded_bytes_per_row(1920), 7680);
    }

    #[test]
    fn output_size_scales_and_rounds() {
        assert_eq!(output_size((1920, 1080), 0.5), (960, 540));
        assert_eq!(output_size((1001, 3), 2.0), (2002, 6));
        assert_eq!(output_size((10, 10), 0.01), (1, 1));
        assert_eq!(output_size((10, 10), f32::NAN), (10, 10));
    }

    #[test]
    fn unpremultiply_restores_straight_colour() {
        // Pure red at 50% over transparent: linear 0.5 premultiplied,
        // sRGB-encoded as 188.
        let mut px = vec![188, 0, 0, 128, 7, 8, 9, 0, 10, 20, 30, 255];
        unpremultiply_srgb(&mut px);
        assert!(px[0] >= 254, "{px:?}");
        assert_eq!(&px[1..4], &[0, 0, 128]);
        assert_eq!(&px[4..8], &[0, 0, 0, 0]);
        assert_eq!(&px[8..12], &[10, 20, 30, 255]);
    }
}
