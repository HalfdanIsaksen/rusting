//! Rusting desktop application: opens the editor window and runs the event loop.
//!
//! winit drives the window, egui (via egui-winit and egui-wgpu) draws the
//! panels, and the renderer draws the canvas into its own texture, which egui
//! shows in the central panel (ADR-002). Frames are drawn only on demand.

mod shell;

use document::animation_core::glam::Mat4;
use document::animation_core::{FrameIndex, Rgba};
use document::{CanvasSize, Document};
use renderer::{CanvasFrame, CanvasRenderer, Gpu, Mesh, StrokeDraw, StrokeKey, Viewport};
use shell::Shell;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// GPU-side state, created once the window exists.
struct Graphics {
    gpu: Gpu,
    egui_renderer: egui_wgpu::Renderer,
    canvas: CanvasRenderer,
    /// The canvas texture as egui knows it.
    canvas_texture: egui::TextureId,
    /// What the canvas texture currently shows, to skip identical re-renders.
    last_canvas: Option<CanvasState>,
}

/// Everything that determines the canvas image.
#[derive(Clone, Debug, PartialEq)]
struct CanvasState {
    viewport: Viewport,
    size: (u32, u32),
    background: Option<Rgba>,
    draws: Vec<StrokeDraw>,
    geometry_generation: u64,
}

struct App {
    window: Option<Arc<Window>>,
    graphics: Option<Graphics>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    shell: Shell,
    /// When egui asked to be repainted next (for animations, tooltips, ...).
    next_repaint: Option<Instant>,
    /// Set when startup fails, so `main` can report it after the loop exits.
    fatal: Option<String>,
}

impl App {
    fn new() -> Self {
        let document = Document::new(
            "Untitled",
            CanvasSize {
                width: 1920,
                height: 1080,
            },
        );
        Self {
            window: None,
            graphics: None,
            egui_ctx: egui::Context::default(),
            egui_state: None,
            shell: Shell::new(document),
            next_repaint: None,
            fatal: None,
        }
    }

    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let attrs = Window::default_attributes()
            .with_title("Rusting")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let size = window.inner_size();
        let gpu = pollster::block_on(Gpu::new(window.clone(), size.width, size.height))
            .map_err(|e| e.to_string())?;

        let mut egui_renderer = egui_wgpu::Renderer::new(
            gpu.device(),
            gpu.surface_format(),
            egui_wgpu::RendererOptions::default(),
        );
        let canvas = CanvasRenderer::new(gpu.device());
        let canvas_texture = egui_renderer.register_native_texture(
            gpu.device(),
            canvas.display_view(),
            wgpu::FilterMode::Nearest,
        );
        let max_texture_side = gpu.device().limits().max_texture_dimension_2d as usize;
        self.egui_state = Some(egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(max_texture_side),
        ));
        window.request_redraw();
        self.window = Some(window);
        self.graphics = Some(Graphics {
            gpu,
            egui_renderer,
            canvas,
            canvas_texture,
            last_canvas: None,
        });
        Ok(())
    }

    fn redraw(&mut self) {
        let (Some(window), Some(graphics), Some(egui_state)) =
            (&self.window, &mut self.graphics, &mut self.egui_state)
        else {
            return;
        };

        // 1. Build the UI. This also applies pan/zoom input to the viewport.
        let raw_input = egui_state.take_egui_input(window);
        let shell = &mut self.shell;
        let texture = graphics.canvas_texture;
        let output = self.egui_ctx.run_ui(raw_input, |ui| shell.ui(ui, texture));
        egui_state.handle_platform_output(window, output.platform_output);

        let Graphics {
            gpu,
            egui_renderer,
            canvas,
            canvas_texture,
            last_canvas,
        } = graphics;

        // 2. Size the canvas texture to the panel and gather what to draw.
        let canvas_state = shell.viewport.map(|viewport| {
            let (width, height) = viewport.physical_size();
            if canvas.resize(gpu.device(), width, height) {
                egui_renderer.update_egui_texture_from_wgpu_texture(
                    gpu.device(),
                    canvas.display_view(),
                    wgpu::FilterMode::Nearest,
                    *canvas_texture,
                );
            }
            let draws = collect_draws(gpu.device(), canvas, shell);
            CanvasState {
                viewport,
                size: canvas.target().size(),
                background: shell.background(),
                draws,
                geometry_generation: canvas.stroke_cache().generation(),
            }
        });
        let canvas_state = canvas_state.filter(|state| last_canvas.as_ref() != Some(state));

        // 3. Upload egui textures and geometry, then draw canvas + UI.
        // egui 0.37 asserts that every texture delta is consumed.
        let mut textures_delta = output.textures_delta;
        for (id, deltas) in textures_delta.set.drain() {
            for delta in &deltas {
                egui_renderer.update_texture(gpu.device(), gpu.queue(), id, delta);
            }
        }
        let jobs = self
            .egui_ctx
            .tessellate(output.shapes, output.pixels_per_point);
        let (width, height) = gpu.surface_size();
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [width, height],
            pixels_per_point: output.pixels_per_point,
        };
        let presented = gpu.render_frame(|device, queue, encoder, view| {
            if let Some(state) = &canvas_state {
                canvas.render(
                    device,
                    queue,
                    encoder,
                    &state.viewport,
                    &CanvasFrame {
                        background: state.background,
                        strokes: &state.draws,
                    },
                );
            }
            let commands = egui_renderer.update_buffers(device, queue, encoder, &jobs, &screen);
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            egui_renderer.render(&mut pass, &jobs, &screen);
            commands
        });
        if presented && let Some(state) = canvas_state {
            *last_canvas = Some(state);
        }
        for id in textures_delta.free.drain() {
            egui_renderer.free_texture(&id);
        }

        // 4. Schedule the next frame only if egui wants one.
        let delay = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(Duration::MAX, |v| v.repaint_delay);
        if delay.is_zero() {
            window.request_redraw();
        } else {
            self.next_repaint = Instant::now().checked_add(delay);
        }
    }
}

/// Strokes to draw this frame: the document at frame 0, plus the debug triangle.
fn collect_draws(
    device: &wgpu::Device,
    canvas: &mut CanvasRenderer,
    shell: &Shell,
) -> Vec<StrokeDraw> {
    let list = shell.document.evaluate(FrameIndex(0));
    // TODO(drawing): pass the drawing crate's tessellator here once it lands,
    // e.g. `|stroke| Some(drawing::tessellate(stroke))`.
    let mut draws = canvas.prepare_render_list(device, &list, |_stroke| None::<Mesh>);

    if shell.debug.test_triangle {
        let id = shell.debug_triangle_id;
        let canvas_size = &shell.document.canvas;
        let (w, h) = (canvas_size.width as f32, canvas_size.height as f32);
        canvas.stroke_cache_mut().ensure(
            device,
            id,
            StrokeKey::from_raw(u64::from(canvas_size.width) << 32 | u64::from(canvas_size.height)),
            || {
                Some(Mesh {
                    vertices: vec![[0.2 * w, 0.8 * h], [0.5 * w, 0.2 * h], [0.8 * w, 0.8 * h]],
                    indices: vec![0, 1, 2],
                })
            },
        );
        draws.push(StrokeDraw {
            id,
            color: Rgba::new(0.8, 0.05, 0.1, 0.85),
            transform: Mat4::IDENTITY,
        });
    }
    draws
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(err) = self.init(event_loop) {
            tracing::error!(%err, "startup failed");
            self.fatal = Some(err);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        // egui-winit asks for a repaint on `RedrawRequested` itself, which
        // would redraw forever; that event is ours alone.
        if !matches!(event, WindowEvent::RedrawRequested)
            && let Some(state) = &mut self.egui_state
            && state.on_window_event(&window, &event).repaint
        {
            window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(graphics) = &mut self.graphics {
                    graphics.gpu.resize(size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => window.request_redraw(),
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.next_repaint {
            Some(at) if at <= Instant::now() => {
                self.next_repaint = None;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu_core=warn,wgpu_hal=warn".into()),
        )
        .init();

    let event_loop = match EventLoop::new() {
        Ok(event_loop) => event_loop,
        Err(err) => {
            tracing::error!(%err, "could not create event loop");
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut app = App::new();
    if let Err(err) = event_loop.run_app(&mut app) {
        tracing::error!(%err, "event loop error");
        return std::process::ExitCode::FAILURE;
    }
    match app.fatal {
        Some(err) => {
            eprintln!("Rusting failed to start: {err}");
            std::process::ExitCode::FAILURE
        }
        None => std::process::ExitCode::SUCCESS,
    }
}
