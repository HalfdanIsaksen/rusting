//! wgpu device and surface setup.

use animation_core::Rgba;

/// Errors from creating or using the GPU context.
#[derive(Debug, thiserror::Error)]
pub enum RendererError {
    /// The window surface could not be created.
    #[error("could not create window surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    /// No GPU adapter supports the surface.
    #[error("no compatible GPU adapter found: {0}")]
    NoAdapter(#[from] wgpu::RequestAdapterError),
    /// The GPU device could not be opened.
    #[error("could not open GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    /// The surface reports no usable configuration for this adapter.
    #[error("surface is not supported by the adapter")]
    UnsupportedSurface,
    /// An offscreen render is larger than the GPU's texture limit.
    #[error("{width}x{height} is larger than this GPU supports ({max} pixels per side)")]
    TooLarge {
        /// Requested width.
        width: u32,
        /// Requested height.
        height: u32,
        /// Largest supported side.
        max: u32,
    },
    /// Reading rendered pixels back from the GPU failed.
    #[error("could not read pixels back from the GPU: {0}")]
    ReadBack(String),
}

/// Opens a GPU device with no window, for offscreen rendering and tests.
pub async fn headless_device() -> Result<(wgpu::Device, wgpu::Queue), RendererError> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await?;
    Ok(adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("rusting-headless"),
            ..Default::default()
        })
        .await?)
}

/// The GPU context: device, queue and the configured window surface.
pub struct Gpu {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
}

impl Gpu {
    /// Creates a GPU context drawing to `target`, sized `width` x `height` physical pixels.
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
    ) -> Result<Self, RendererError> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(target)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("rusting-device"),
                ..Default::default()
            })
            .await?;
        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or(RendererError::UnsupportedSurface)?;
        // Prefer a non-sRGB (gamma-space) surface: egui-wgpu blends in gamma
        // space and warns on sRGB surfaces. The canvas itself is blended in
        // linear space in its own offscreen target (see `CanvasTarget`).
        let caps = surface.get_capabilities(&adapter);
        if let Some(format) = caps.formats.iter().copied().find(|f| !f.is_srgb()) {
            config.format = format;
        }
        // Lowest-latency vsync'd mode that never tears.
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        tracing::info!(adapter = ?adapter.get_info().name, format = ?config.format, "GPU initialised");
        Ok(Self {
            surface,
            device,
            queue,
            config,
        })
    }

    /// The wgpu device.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The wgpu queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// The surface texture format.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Resizes the surface. Zero sizes (minimised windows) are ignored.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Surface size in physical pixels.
    pub fn surface_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Acquires the next surface texture, lets `record` fill an encoder that
    /// targets it, then submits and presents. `record` returns any extra
    /// command buffers (for example from egui-wgpu) to submit first.
    /// Returns false if no frame was available (minimised, occluded, lost).
    pub fn render_frame(
        &mut self,
        record: impl FnOnce(
            &wgpu::Device,
            &wgpu::Queue,
            &mut wgpu::CommandEncoder,
            &wgpu::TextureView,
        ) -> Vec<wgpu::CommandBuffer>,
    ) -> bool {
        let Some(frame) = self.acquire_frame() else {
            return false;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        let mut buffers = record(&self.device, &self.queue, &mut encoder, &view);
        buffers.push(encoder.finish());
        self.queue.submit(buffers);
        self.queue.present(frame);
        true
    }

    /// Clears the window to `color` and presents it.
    pub fn render_clear(&mut self, color: Rgba) {
        let Some(frame) = self.acquire_frame() else {
            return;
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("clear"),
            });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(color.r),
                            g: f64::from(color.g),
                            b: f64::from(color.b),
                            a: f64::from(color.a),
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(frame);
    }

    /// Gets the next surface texture, reconfiguring the surface if it was lost.
    fn acquire_frame(&mut self) -> Option<wgpu::SurfaceTexture> {
        use wgpu::CurrentSurfaceTexture as Current;
        match self.surface.get_current_texture() {
            Current::Success(frame) => Some(frame),
            Current::Suboptimal(frame) => {
                // Use this frame, but reconfigure so the next one matches the window.
                self.surface.configure(&self.device, &self.config);
                Some(frame)
            }
            Current::Timeout | Current::Occluded => None,
            Current::Outdated => {
                self.surface.configure(&self.device, &self.config);
                None
            }
            Current::Lost | Current::Validation => {
                tracing::warn!("surface lost or invalid; skipping frame and reconfiguring");
                self.surface.configure(&self.device, &self.config);
                None
            }
        }
    }
}
