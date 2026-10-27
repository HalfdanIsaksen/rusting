//! The offscreen texture the canvas is drawn into.

/// Format the canvas pipelines render in. Blending happens in linear space and
/// the result is stored sRGB-encoded.
pub const CANVAS_RENDER_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
/// Format of [`CanvasTarget::display_view`]: the same bytes viewed without
/// sRGB decoding, which is what egui-wgpu expects for user textures.
pub const CANVAS_DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// MSAA sample count of the canvas.
pub const CANVAS_SAMPLE_COUNT: u32 = 4;

/// A 4x MSAA colour buffer and the single-sample texture it resolves into.
#[derive(Debug)]
pub struct CanvasTarget {
    size: (u32, u32),
    msaa_view: wgpu::TextureView,
    resolved: wgpu::Texture,
    resolve_view: wgpu::TextureView,
    display_view: wgpu::TextureView,
}

impl CanvasTarget {
    /// Creates a target of `width` x `height` physical pixels (minimum 1x1).
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let size = clamp_size(device, width, height);
        let extent = wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        };
        let msaa = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas-msaa"),
            size: extent,
            mip_level_count: 1,
            sample_count: CANVAS_SAMPLE_COUNT,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_RENDER_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let resolved = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas-resolved"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: CANVAS_DISPLAY_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[CANVAS_RENDER_FORMAT],
        });
        let msaa_view = msaa.create_view(&wgpu::TextureViewDescriptor::default());
        let resolve_view = resolved.create_view(&wgpu::TextureViewDescriptor {
            label: Some("canvas-resolve-srgb"),
            format: Some(CANVAS_RENDER_FORMAT),
            ..Default::default()
        });
        let display_view = resolved.create_view(&wgpu::TextureViewDescriptor {
            label: Some("canvas-display"),
            format: Some(CANVAS_DISPLAY_FORMAT),
            ..Default::default()
        });
        Self {
            size,
            msaa_view,
            resolved,
            resolve_view,
            display_view,
        }
    }

    /// Recreates the textures if the size changed. Returns true if it did, in
    /// which case views obtained earlier (e.g. registered with egui) are stale.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        let wanted = clamp_size(device, width, height);
        if wanted == self.size {
            return false;
        }
        *self = Self::new(device, wanted.0, wanted.1);
        true
    }

    /// Size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The resolved texture (format [`CANVAS_DISPLAY_FORMAT`]).
    pub fn texture(&self) -> &wgpu::Texture {
        &self.resolved
    }

    /// View for displaying or sampling the finished canvas
    /// ([`CANVAS_DISPLAY_FORMAT`], sRGB-encoded bytes).
    pub fn display_view(&self) -> &wgpu::TextureView {
        &self.display_view
    }

    pub(crate) fn msaa_view(&self) -> &wgpu::TextureView {
        &self.msaa_view
    }

    pub(crate) fn resolve_view(&self) -> &wgpu::TextureView {
        &self.resolve_view
    }
}

/// Clamps a requested size to `1..=max_texture_dimension_2d` on both axes.
fn clamp_size(device: &wgpu::Device, width: u32, height: u32) -> (u32, u32) {
    let max = device.limits().max_texture_dimension_2d.max(1);
    (width.clamp(1, max), height.clamp(1, max))
}
