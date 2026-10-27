//! Rusting desktop application: opens the editor window and runs the event loop.

use document::animation_core::Rgba;
use renderer::Gpu;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

/// Workspace background color around the canvas.
const WORKSPACE_COLOR: Rgba = Rgba::rgb(0.16, 0.16, 0.18);

struct App {
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    /// Set when startup fails, so `main` can report it after the loop exits.
    fatal: Option<String>,
}

impl App {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let attrs = Window::default_attributes()
            .with_title("Rusting")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let size = window.inner_size();
        let gpu = pollster::block_on(Gpu::new(window.clone(), size.width, size.height))
            .map_err(|e| e.to_string())?;
        window.request_redraw();
        self.window = Some(window);
        self.gpu = Some(gpu);
        Ok(())
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
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.render_clear(WORKSPACE_COLOR);
                }
            }
            _ => {}
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
    let mut app = App {
        window: None,
        gpu: None,
        fatal: None,
    };
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
