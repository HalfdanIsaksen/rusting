//! GPU rendering with wgpu.
//!
//! The renderer owns the wgpu device and the window surface ([`Gpu`]) and draws
//! the canvas into an offscreen texture ([`CanvasRenderer`]). It knows nothing
//! about windows or UI toolkits: callers pass anything wgpu can make a surface
//! from (for example a `winit` window) and show the canvas texture however they
//! like. [`OffscreenRenderer`] renders frames at document resolution and reads
//! the pixels back, for export. [`Viewport`] holds the zoom/pan math and is GPU-free.

pub mod canvas;
pub mod canvas_target;
pub mod draw_list;
pub mod gpu;
pub mod mesh;
pub mod offscreen;
pub mod stroke_cache;
pub mod viewport;

pub use canvas::{CanvasFrame, CanvasRenderer, CanvasStyle, StrokeDraw, transform_matrix};
pub use canvas_target::{
    CANVAS_DEPTH_FORMAT, CANVAS_DISPLAY_FORMAT, CANVAS_RENDER_FORMAT, CANVAS_SAMPLE_COUNT,
    CanvasTarget,
};
pub use draw_list::{DrawListBuilder, fill_color, fill_gradient, stroke_color};
pub use gpu::{Gpu, RendererError, headless_device};
pub use mesh::{Mesh, MeshRef, MeshSource};
pub use offscreen::{OffscreenFrame, OffscreenRenderer, Pixels, output_size};
pub use stroke_cache::{MeshId, StrokeCache, StrokeKey};
pub use viewport::{MAX_ZOOM, MIN_ZOOM, Viewport};
