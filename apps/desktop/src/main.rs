//! Rusting desktop application: opens the editor window and runs the event loop.
//!
//! winit drives the window, egui (via egui-winit and egui-wgpu) draws the
//! panels, and the renderer draws the canvas into its own texture, which egui
//! shows in the central panel (ADR-002). Frames are drawn only on demand.

mod export;
mod shell;

use document::animation_core::Rgba;
use document::animation_core::Vec2;
use document::animation_core::glam::Mat4;
use document::drawing::{Fill, ItemId, Stroke};
use document::scene::BlendMode;
use renderer::{
    CanvasFrame, CanvasRenderer, DrawListBuilder, Gpu, Mesh, OffscreenRenderer, StrokeDraw,
    StrokeKey, Viewport, fill_color, fill_gradient, stroke_color, transform_matrix,
};
use shell::Shell;
use std::path::{Path, PathBuf};
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
    /// Renders export frames; created on the first export.
    offscreen: Option<OffscreenRenderer>,
    /// Renders the View → Preview image; created when it first opens.
    preview: Option<PreviewGraphics>,
}

/// The preview window's own small canvas. It has its own mesh cache and
/// uniform buffers, so it never disturbs the editor canvas, and it only
/// re-renders when what it shows changes.
struct PreviewGraphics {
    canvas: CanvasRenderer,
    texture: egui::TextureId,
    /// What the preview texture currently shows.
    last: Option<PreviewState>,
}

impl PreviewGraphics {
    fn new(device: &wgpu::Device, egui_renderer: &mut egui_wgpu::Renderer) -> Self {
        let canvas = CanvasRenderer::new(device);
        let texture = egui_renderer.register_native_texture(
            device,
            canvas.display_view(),
            wgpu::FilterMode::Linear,
        );
        Self {
            canvas,
            texture,
            last: None,
        }
    }
}

/// Everything that determines the preview image.
#[derive(Clone, Debug, PartialEq)]
struct PreviewState {
    size: (u32, u32),
    canvas_size: Vec2,
    background: Option<Rgba>,
    draws: Vec<StrokeDraw>,
    geometry_generation: u64,
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
    /// The window title last set, to avoid redundant updates.
    title: String,
    /// Set when startup fails, so `main` can report it after the loop exits.
    fatal: Option<String>,
}

impl App {
    /// The app showing the project at `open` if given and readable, or else
    /// the start menu for a new document.
    fn new(open: Option<&Path>) -> Self {
        let mut session = ui::Session::new();
        if !open.is_some_and(|path| session.open_path(path)) {
            session.show_start_menu(ui::StartMenu::new(Default::default(), false));
        }
        Self {
            window: None,
            graphics: None,
            egui_ctx: {
                let ctx = egui::Context::default();
                ui::theme::apply(&ctx);
                ctx
            },
            egui_state: None,
            shell: Shell::new(session),
            next_repaint: None,
            title: String::new(),
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
            offscreen: None,
            preview: None,
        });
        Ok(())
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(graphics), Some(egui_state)) =
            (&self.window, &mut self.graphics, &mut self.egui_state)
        else {
            return;
        };

        // 1. Build the UI. This also applies pan/zoom input to the viewport.
        let raw_input = egui_state.take_egui_input(window);
        let shell = &mut self.shell;
        let texture = graphics.canvas_texture;
        if shell.session.preview.is_open() && graphics.preview.is_none() {
            graphics.preview = Some(PreviewGraphics::new(
                graphics.gpu.device(),
                &mut graphics.egui_renderer,
            ));
        }
        let preview_texture = graphics.preview.as_ref().map(|p| p.texture);
        let output = self
            .egui_ctx
            .run_ui(raw_input, |ui| shell.ui(ui, texture, preview_texture));
        egui_state.handle_platform_output(window, output.platform_output);
        let title = shell.session.title();
        if title != self.title {
            window.set_title(&title);
            self.title = title;
        }

        let Graphics {
            gpu,
            egui_renderer,
            canvas,
            canvas_texture,
            last_canvas,
            offscreen,
            preview,
        } = graphics;

        // 2. Size the canvas texture to the panel and gather what to draw.
        // Nothing to draw while the start menu covers the window.
        let viewport = shell
            .viewport
            .filter(|_| shell.session.start_menu().is_none());
        // The workspace around the canvas follows the theme.
        let workspace = egui::Rgba::from(
            ui::theme::palette(self.egui_ctx.global_style().visuals.dark_mode).code_bg,
        );
        canvas.style.workspace =
            document::animation_core::Rgba::rgb(workspace.r(), workspace.g(), workspace.b());
        let canvas_state = viewport.map(|viewport| {
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
        let preview_state = match (shell.preview_size, preview.as_mut()) {
            (Some(size), Some(preview)) => {
                prepare_preview(gpu, egui_renderer, preview, shell, size)
            }
            _ => None,
        };

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
            if let (Some(state), Some(preview)) = (&preview_state, preview.as_mut()) {
                preview.canvas.render_bare(
                    device,
                    queue,
                    encoder,
                    &Viewport::filling(state.canvas_size, state.size),
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
        if presented && let (Some(state), Some(preview)) = (preview_state, preview.as_mut()) {
            preview.last = Some(state);
        }
        for id in textures_delta.free.drain() {
            egui_renderer.free_texture(&id);
        }

        // 4. Native dialogs and quitting, requested by the UI.
        let window = window.clone();
        for request in self.shell.session.take_requests() {
            handle_request(&mut self.shell, request, event_loop);
            window.request_redraw();
        }

        // 5. Export: ask where to save confirmed options, then render a few
        // frames of the running export (the worker thread encodes them).
        let shell = &mut self.shell;
        if let Some(settings) = shell.export.take_confirmed()
            && let Some(path) = export::ask_path(&settings, &shell.session)
            && let Err(err) = shell.export.start(settings, &path, &shell.session)
        {
            shell.session.show_error("Export failed", err);
        }
        if shell.export.is_running() {
            let offscreen = offscreen.get_or_insert_with(|| OffscreenRenderer::new(gpu.device()));
            if let Some(outcome) = shell.export.pump(gpu.device(), gpu.queue(), offscreen) {
                export::report(outcome, &mut shell.session);
            }
            window.request_redraw();
        }

        // 6. Schedule the next frame only if egui wants one.
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

/// Sizes the preview texture and gathers the document content at the
/// preview's frame: no onion skins, tool previews or selection, exactly as
/// export draws it. `None` when the preview already shows this.
fn prepare_preview(
    gpu: &Gpu,
    egui_renderer: &mut egui_wgpu::Renderer,
    preview: &mut PreviewGraphics,
    shell: &Shell,
    (width, height): (u32, u32),
) -> Option<PreviewState> {
    let device = gpu.device();
    if preview.canvas.resize(device, width, height) {
        egui_renderer.update_egui_texture_from_wgpu_texture(
            device,
            preview.canvas.display_view(),
            wgpu::FilterMode::Linear,
            preview.texture,
        );
    }
    let doc = &shell.session.document;
    let mut draws = DrawListBuilder::new(device, preview.canvas.stroke_cache_mut());
    draws.render_list(&doc.evaluate(shell.session.preview_frame()));
    let draws = draws.finish();
    let state = PreviewState {
        size: preview.canvas.target().size(),
        canvas_size: Vec2::new(doc.canvas.width as f32, doc.canvas.height as f32),
        background: shell.background(),
        draws,
        geometry_generation: preview.canvas.stroke_cache().generation(),
    };
    (preview.last.as_ref() != Some(&state)).then_some(state)
}

/// Meshes to draw this frame, bottom to top: onion skins, the document at
/// the current frame (each drawing's fills under its strokes; strokes being
/// erased are replaced by what is left of them, and selected items being
/// dragged are moved), the stroke being drawn, and the debug triangle.
/// Export draws the same document content through the same
/// [`DrawListBuilder::render_list`] rules, without the editor-only parts.
fn collect_draws(
    device: &wgpu::Device,
    canvas: &mut CanvasRenderer,
    shell: &Shell,
) -> Vec<StrokeDraw> {
    let session = &shell.session;
    let doc = &session.document;
    let frame = session.current_frame();
    let mut draws = DrawListBuilder::new(device, canvas.stroke_cache_mut());

    for skin in session.onion.collect(doc, frame) {
        let transform = transform_matrix(&skin.transform);
        for fill in skin.drawing.fills() {
            draws.fill(fill, skin.color.with_opacity(fill.opacity), None, transform);
        }
        for stroke in skin.drawing.strokes() {
            draws.stroke(
                stroke,
                skin.color.with_opacity(stroke.style.opacity),
                transform,
            );
        }
    }

    let erasing = session.tools.erasing();
    // The stroke or shape being drawn: its layer, fill and stroke.
    let mut preview = session
        .tools
        .preview()
        .map(|(layer, stroke)| (layer, None, Some(stroke)))
        .or_else(|| session.tools.shape_preview());
    // Selected items being dragged are drawn moved by the drag offset, or
    // scaled/rotated copies while the transform tool drags (copies, so brush
    // widths match what release commits).
    let offset = session.tools.move_offset();
    let reshape = session.tools.transform_preview();
    let selection_layer = session.tools.selection_at().map(|(layer, _)| layer);
    let selected = session.tools.selection();
    for layer in &doc.evaluate(frame).layers {
        draws.set_layer(Some(layer.layer_id), layer.blend);
        let transform = transform_matrix(&layer.transform);
        let moved = transform * Mat4::from_translation(offset.extend(0.0));
        let is_selection_layer = selection_layer == Some(layer.layer_id);
        let is_moving = is_selection_layer && offset != Vec2::ZERO;
        let reshaping =
            |item: ItemId| reshape.filter(|_| is_selection_layer && selected.contains(&item));
        let place = |item: ItemId| {
            if is_moving && selected.contains(&item) {
                moved
            } else {
                transform
            }
        };
        for fill in layer.drawing.fills() {
            let item = ItemId::Fill(fill.id);
            if let Some(t) = reshaping(item) {
                let fill = fill.transformed(&t);
                draws.fill(
                    &fill,
                    fill_color(&fill, layer.opacity),
                    fill_gradient(&fill, layer.opacity),
                    transform,
                );
            } else {
                draws.fill(
                    fill,
                    fill_color(fill, layer.opacity),
                    fill_gradient(fill, layer.opacity),
                    place(item),
                );
            }
        }
        for stroke in layer.drawing.strokes() {
            let item = ItemId::Stroke(stroke.id);
            if let Some(t) = reshaping(item) {
                let stroke = stroke.transformed(&t);
                draws.stroke(&stroke, stroke_color(&stroke, layer.opacity), transform);
            } else if !erasing.contains(&stroke.id) {
                draws.stroke(stroke, stroke_color(stroke, layer.opacity), place(item));
            } else {
                for piece in session.tools.erase_preview(stroke.id) {
                    draws.stroke(piece, stroke_color(piece, layer.opacity), transform);
                }
            }
        }
        if let Some((_, fill, stroke)) = preview.take_if(|(id, ..)| *id == layer.layer_id) {
            draw_preview(&mut draws, fill, stroke, layer.opacity, transform);
        }
    }
    // The active layer shows nothing yet at this frame: draw the preview on top.
    if let Some((layer_id, fill, stroke)) = preview {
        let layer = doc.layers().get(layer_id);
        let transform = layer.map_or(Mat4::IDENTITY, |l| transform_matrix(&l.transform));
        let opacity = layer.map_or(1.0, |l| l.opacity);
        draws.set_layer(Some(layer_id), layer.map_or(BlendMode::Normal, |l| l.blend));
        draw_preview(&mut draws, fill, stroke, opacity, transform);
    }

    if shell.debug.test_triangle {
        let id = shell.debug_triangle_id;
        let canvas_size = &doc.canvas;
        let (w, h) = (canvas_size.width as f32, canvas_size.height as f32);
        let device = draws.device();
        draws.cache().ensure(
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
            id: id.into(),
            color: Rgba::new(0.8, 0.05, 0.1, 0.85),
            gradient: None,
            transform: Mat4::IDENTITY,
            layer: None,
            blend: BlendMode::Normal,
        });
    }
    draws.finish()
}

/// Adds the stroke or shape being drawn: its fill, then its stroke.
fn draw_preview(
    draws: &mut DrawListBuilder<'_>,
    fill: Option<Fill>,
    stroke: Option<Stroke>,
    opacity: f32,
    transform: Mat4,
) {
    if let Some(fill) = fill {
        draws.fill(
            &fill,
            fill_color(&fill, opacity),
            fill_gradient(&fill, opacity),
            transform,
        );
    }
    if let Some(stroke) = stroke {
        draws.stroke(&stroke, stroke_color(&stroke, opacity), transform);
    }
}

/// Native file dialogs (via `rfd`) and quitting, on behalf of the UI.
fn handle_request(shell: &mut Shell, request: ui::Request, event_loop: &ActiveEventLoop) {
    if let ui::Request::Export(kind) = request {
        shell.export.open(kind, &shell.session);
        return;
    }
    let session = &mut shell.session;
    let dialog = || {
        let dialog =
            rfd::FileDialog::new().add_filter("Rusting project", &[ui::session::PROJECT_EXTENSION]);
        match session.path.as_deref().and_then(Path::parent) {
            Some(dir) => dialog.set_directory(dir),
            None => dialog,
        }
    };
    match request {
        ui::Request::OpenDialog => {
            if let Some(path) = dialog().set_title("Open Project").pick_file() {
                session.open_path(&path);
            }
        }
        ui::Request::SaveAsDialog => {
            let name = session
                .path
                .as_deref()
                .and_then(Path::file_name)
                .map_or_else(
                    || {
                        format!(
                            "{}.{}",
                            session.document.name,
                            ui::session::PROJECT_EXTENSION
                        )
                    },
                    |n| n.to_string_lossy().into_owned(),
                );
            if let Some(path) = dialog()
                .set_title("Save Project")
                .set_file_name(name)
                .save_file()
            {
                session.save_to(&path);
            }
        }
        ui::Request::Quit => event_loop.exit(),
        ui::Request::Export(_) => {}
    }
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
            WindowEvent::CloseRequested => {
                // Asks first if there are unsaved changes; quits via a request.
                self.shell.session.request(ui::DiscardAction::Quit);
                window.request_redraw();
            }
            WindowEvent::Resized(size) => {
                if let Some(graphics) = &mut self.graphics {
                    graphics.gpu.resize(size.width, size.height);
                }
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => window.request_redraw(),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
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
    // `rusting path/to/file.rusting` opens that project.
    let open = std::env::args_os().nth(1).map(PathBuf::from);
    let mut app = App::new(open.as_deref());
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
