//! Building the list of meshes to draw for a frame.
//!
//! The editor canvas and export both use [`DrawListBuilder`], so a frame
//! exports exactly as it looks on screen: the same tessellation, the same
//! colour and opacity rules, the same bottom-to-top order.
//!
//! A brushed stroke (ADR-022) is drawn with its library brush when the
//! builder knows the document's library ([`DrawListBuilder::with_assets`]);
//! without it, or if the brush is missing, it is drawn solid.

use crate::canvas::{BrushDraw, StrokeDraw, affine_matrix};
use crate::stroke_cache::{StrokeCache, StrokeKey};
use animation_core::glam::Mat4;
use animation_core::{AssetId, LayerId, Rgba};
use document::scene::BlendMode;
use document::{Asset, RenderList};
use drawing::{BrushData, Fill, Gradient, Image, PaintItem, Stroke, tessellate, tessellate_fill};
use std::collections::BTreeMap;

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

/// Draw gradient of a fill on a layer: its gradient with fill and layer
/// opacity applied to the end colour.
pub fn fill_gradient(fill: &Fill, layer_opacity: f32) -> Option<Gradient> {
    fill.gradient.map(|g| Gradient {
        end_color: g.end_color.with_opacity(fill.opacity * layer_opacity),
        ..g
    })
}

/// Collects [`StrokeDraw`]s bottom to top, tessellating and caching each
/// stroke's or fill's mesh the first time it (or a changed version) is added.
pub struct DrawListBuilder<'a> {
    device: &'a wgpu::Device,
    cache: &'a mut StrokeCache,
    draws: Vec<StrokeDraw>,
    layer: Option<LayerId>,
    blend: BlendMode,
    assets: Option<&'a BTreeMap<AssetId, Asset>>,
    /// Brushes not (yet) in the library, e.g. a built-in brush whose first
    /// stroke is being drawn.
    extra_brushes: Vec<(AssetId, &'a BrushData)>,
}

impl<'a> DrawListBuilder<'a> {
    /// A builder caching meshes in `cache`.
    pub fn new(device: &'a wgpu::Device, cache: &'a mut StrokeCache) -> Self {
        Self {
            device,
            cache,
            draws: Vec::new(),
            layer: None,
            blend: BlendMode::Normal,
            assets: None,
            extra_brushes: Vec::new(),
        }
    }

    /// Also knows brush `data` as `id`, for strokes whose brush is not in
    /// the library yet.
    pub fn with_brush(mut self, id: AssetId, data: &'a BrushData) -> Self {
        self.extra_brushes.push((id, data));
        self
    }

    /// Looks up the brushes of brushed strokes in `assets`, the document's
    /// library.
    pub fn with_assets(mut self, assets: &'a BTreeMap<AssetId, Asset>) -> Self {
        self.assets = Some(assets);
        self
    }

    /// Strokes and fills added from now on belong to `layer` and blend with
    /// `blend` (initially no layer, Normal).
    pub fn set_layer(&mut self, layer: Option<LayerId>, blend: BlendMode) {
        self.layer = layer;
        self.blend = blend;
    }

    /// Adds a stroke drawn in `color` through `transform`: stamped with its
    /// brush if it has one in the library, otherwise as a solid mesh.
    pub fn stroke(&mut self, stroke: &Stroke, color: Rgba, transform: Mat4) {
        let brush = stroke.style.brush.and_then(|id| {
            let data = self
                .assets
                .and_then(|assets| assets.get(&id)?.brush())
                .or_else(|| {
                    self.extra_brushes
                        .iter()
                        .find(|(extra, _)| *extra == id)
                        .map(|(_, data)| *data)
                })?;
            Some((id, data))
        });
        if let Some((asset, data)) = brush {
            self.cache.ensure_brushed(self.device, stroke, asset, data);
            let grain_strength = if data.grain.is_some() {
                data.settings.grain_strength
            } else {
                0.0
            };
            self.draws.push(StrokeDraw {
                id: crate::MeshId::Brushed(stroke.id),
                color,
                gradient: None,
                transform,
                layer: self.layer,
                blend: self.blend,
                brush: Some(BrushDraw {
                    asset,
                    grain_scale: data.settings.grain_scale,
                    grain_strength,
                    grain_seed: crate::canvas::grain_seed(stroke.id),
                }),
            });
            return;
        }
        self.cache
            .ensure(self.device, stroke.id, StrokeKey::of(stroke), || {
                Some(tessellate(stroke))
            });
        self.draws.push(StrokeDraw {
            id: stroke.id.into(),
            color,
            gradient: None,
            transform,
            layer: self.layer,
            blend: self.blend,
            brush: None,
        });
    }

    /// Adds a fill drawn in `color` (blending into `gradient`, if any)
    /// through `transform`.
    pub fn fill(&mut self, fill: &Fill, color: Rgba, gradient: Option<Gradient>, transform: Mat4) {
        self.cache
            .ensure(self.device, fill.id, StrokeKey::of_fill(fill), || {
                Some(tessellate_fill(fill))
            });
        self.draws.push(StrokeDraw {
            id: fill.id.into(),
            color,
            gradient,
            transform,
            layer: self.layer,
            blend: self.blend,
            brush: None,
        });
    }

    /// Adds an imported picture drawn with `opacity` (image and layer
    /// opacity together) through `transform`. Its texture is decoded now and
    /// uploaded by the next render.
    pub fn image(&mut self, image: &Image, opacity: f32, transform: Mat4) {
        self.cache.ensure_image(self.device, image);
        self.draws.push(StrokeDraw {
            id: image.id.into(),
            color: Rgba::WHITE.with_opacity(image.opacity * opacity),
            gradient: None,
            transform,
            layer: self.layer,
            blend: self.blend,
            brush: None,
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
    /// top), its items in paint order (ADR-010), with layer opacity and
    /// transform.
    /// Hidden layers are already left out of `list`.
    pub fn render_list(&mut self, list: &RenderList<'_>) {
        for layer in &list.layers {
            self.set_layer(Some(layer.layer_id), layer.blend);
            let transform = affine_matrix(&layer.transform);
            for item in layer.drawing.paint_order() {
                match item {
                    PaintItem::Image(image) => self.image(image, layer.opacity, transform),
                    PaintItem::Fill(fill) => self.fill(
                        fill,
                        fill_color(fill, layer.opacity),
                        fill_gradient(fill, layer.opacity),
                        transform,
                    ),
                    PaintItem::Stroke(stroke) => {
                        self.stroke(stroke, stroke_color(stroke, layer.opacity), transform);
                    }
                }
            }
        }
    }

    /// The draws collected, bottom to top.
    pub fn finish(self) -> Vec<StrokeDraw> {
        self.draws
    }
}
