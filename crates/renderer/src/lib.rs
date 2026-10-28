//! GPU rendering with wgpu.
//!
//! The renderer owns the wgpu device and the window surface ([`Gpu`]) and draws
//! the canvas into an offscreen texture ([`CanvasRenderer`]). It knows nothing
//! about windows or UI toolkits: callers pass anything wgpu can make a surface
//! from (for example a `winit` window) and show the canvas texture however they
//! like. [`Viewport`] holds the zoom/pan math and is GPU-free.

pub mod canvas;
pub mod canvas_target;
pub mod gpu;
pub mod mesh;
pub mod stroke_cache;
pub mod viewport;

pub use canvas::{CanvasFrame, CanvasRenderer, CanvasStyle, StrokeDraw, transform_matrix};
pub use canvas_target::{
    CANVAS_DEPTH_FORMAT, CANVAS_DISPLAY_FORMAT, CANVAS_RENDER_FORMAT, CANVAS_SAMPLE_COUNT,
    CanvasTarget,
};
pub use gpu::{Gpu, RendererError};
pub use mesh::{Mesh, MeshRef, MeshSource};
pub use stroke_cache::{MeshId, StrokeCache, StrokeKey};
pub use viewport::{MAX_ZOOM, MIN_ZOOM, Viewport};
