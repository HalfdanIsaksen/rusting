//! GPU rendering with wgpu.
//!
//! The renderer owns the wgpu device and the window surface. It knows nothing
//! about windows or UI toolkits: callers pass anything wgpu can make a surface
//! from (for example a `winit` window).

pub mod gpu;

pub use gpu::{Gpu, RendererError};
