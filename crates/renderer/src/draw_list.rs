//! Building the list of meshes to draw for a frame.
//!
//! The editor canvas and export both use [`DrawListBuilder`], so a frame
//! exports exactly as it looks on screen: the same tessellation, the same
//! colour and opacity rules, the same bottom-to-top order.

use crate::canvas::{StrokeDraw, transform_matrix};
use crate::stroke_cache::{StrokeCache, StrokeKey};
use animation_core::Rgba;
use animation_core::glam::Mat4;
use document::RenderList;
use drawing::{Fill, Stroke, tessellate, tessellate_fill};

/// Draw colour of a stroke on a layer: its colour with brush and layer
/// opacity applied.
pub fn stroke_color(stroke: &Stroke, layer_opacity: f32) -> Rgba {
    stroke
        .style
        .color
        .with_opacity(stroke.style.opacity * layer_opacity)
}

/// Draw colour of a fill on a layer: its colour with fill and layer opacity
/// applied.
pub fn fill_color(fill: &Fill, layer_opacity: f32) -> Rgba {
    fill.color.with_opacity(fill.opacity * layer_opacity)
}

/// Collects [`StrokeDraw`]s bottom to top, tessellating and caching each
/// stroke's or fill's mesh the first time it (or a changed version) is added.
pub struct DrawListBuilder<'a> {
    device: &'a wgpu::Device,
    cache: &'a mut StrokeCache,
    draws: Vec<StrokeDraw>,
}

impl<'a> DrawListBuilder<'a> {
    /// A builder caching meshes in `cache`.
    pub fn new(device: &'a wgpu::Device, cache: &'a mut StrokeCache) -> Self {
        Self {
            device,
            cache,
            draws: Vec::new(),
        }
    }

    /// Adds a stroke drawn in `color` through `transform`.
    pub fn stroke(&mut self, stroke: &Stroke, color: Rgba, transform: Mat4) {
        self.cache
            .ensure(self.device, stroke.id, StrokeKey::of(stroke), || {
                Some(tessellate(stroke))
            });
        self.draws.push(StrokeDraw {
            id: stroke.id.into(),
            color,
            transform,
        });
    }

    /// Adds a fill drawn in `color` through `transform`.
    pub fn fill(&mut self, fill: &Fill, color: Rgba, transform: Mat4) {
        self.cache
            .ensure(self.device, fill.id, StrokeKey::of_fill(fill), || {
                Some(tessellate_fill(fill))
            });
        self.draws.push(StrokeDraw {
            id: fill.id.into(),
            color,
            transform,
        });
    }

    /// Adds a draw whose mesh the caller caches itself (see [`Self::cache`]).
    pub fn push(&mut self, draw: StrokeDraw) {
        self.draws.push(draw);
    }

    /// The mesh cache, for meshes that aren't strokes or fills.
    pub fn cache(&mut self) -> &mut StrokeCache {
        self.cache
    }

    /// The device meshes are uploaded to.
    pub fn device(&self) -> &'a wgpu::Device {
        self.device
    }

    /// Adds a document frame as the canvas shows it: per layer (bottom to
    /// top), its fills, then its strokes, with layer opacity and transform.
    /// Hidden layers are already left out of `list`.
    pub fn render_list(&mut self, list: &RenderList<'_>) {
        for layer in &list.layers {
            let transform = transform_matrix(&layer.transform);
            for fill in layer.drawing.fills() {
                self.fill(fill, fill_color(fill, layer.opacity), transform);
            }
            for stroke in layer.drawing.strokes() {
                self.stroke(stroke, stroke_color(stroke, layer.opacity), transform);
            }
        }
    }

    /// The draws collected, bottom to top.
    pub fn finish(self) -> Vec<StrokeDraw> {
        self.draws
    }
}
