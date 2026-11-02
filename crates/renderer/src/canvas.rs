//! Drawing the canvas: workspace, drop shadow and outline, background, strokes.
//!
//! Strokes are drawn in groups: each run of Normal layers, and each layer
//! with another blend mode, is drawn into its own texture and composited
//! onto the canvas below it (`composite.wgsl`).

use crate::canvas_target::{
    CANVAS_DEPTH_FORMAT, CANVAS_RENDER_FORMAT, CANVAS_SAMPLE_COUNT, CanvasTarget,
};
use crate::draw_list::{fill_color, fill_gradient, stroke_color};
use crate::mesh::MeshSource;
use crate::stroke_cache::{DEFAULT_MAX_UNUSED_RENDERS, MeshId, StrokeCache, StrokeKey};
use crate::viewport::Viewport;
use animation_core::glam::{Mat4, Quat, Vec2};
use animation_core::{LayerId, Rgba};
use document::RenderList;
use document::scene::{BlendMode, Transform2D};
use drawing::{Fill, Gradient, Stroke};
use std::ops::Range;

const BACKGROUND_WGSL: &str = include_str!("../../../assets/shaders/background.wgsl");
const FRAME_WGSL: &str = include_str!("../../../assets/shaders/canvas_frame.wgsl");
const STROKE_WGSL: &str = include_str!("../../../assets/shaders/stroke.wgsl");
const COMPOSITE_WGSL: &str = include_str!("../../../assets/shaders/composite.wgsl");

/// Byte size of `Background` in `background.wgsl`.
const BACKGROUND_UNIFORM_SIZE: u64 = 144;
/// Byte size of `Frame` in `canvas_frame.wgsl`.
const FRAME_UNIFORM_SIZE: u64 = 80;
/// Byte size of `Globals` in `stroke.wgsl`.
const STROKE_GLOBALS_SIZE: u64 = 64;
/// Byte size of `Draw` in `stroke.wgsl`.
const STROKE_DRAW_SIZE: u64 = 128;

/// Colours and sizes of the canvas chrome. Colours are linear RGBA; sizes are
/// logical points (scaled by the DPI factor when drawn).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CanvasStyle {
    /// Workspace colour around the canvas.
    pub workspace: Rgba,
    /// Canvas outline colour.
    pub outline: Rgba,
    /// Canvas outline width.
    pub outline_width: f32,
    /// Drop shadow colour.
    pub shadow: Rgba,
    /// Drop shadow offset.
    pub shadow_offset: Vec2,
    /// Drop shadow blur radius.
    pub shadow_radius: f32,
    /// Checkerboard colours for transparent documents.
    pub checker: [Rgba; 2],
    /// Checkerboard cell size.
    pub checker_size: f32,
}

impl Default for CanvasStyle {
    fn default() -> Self {
        Self {
            workspace: Rgba::rgb(0.017, 0.017, 0.02),
            outline: Rgba::new(0.0, 0.0, 0.0, 0.85),
            outline_width: 1.0,
            shadow: Rgba::new(0.0, 0.0, 0.0, 0.5),
            shadow_offset: Vec2::new(0.0, 3.0),
            shadow_radius: 16.0,
            checker: [Rgba::rgb(0.8, 0.8, 0.8), Rgba::rgb(0.55, 0.55, 0.55)],
            checker_size: 8.0,
        }
    }
}

/// One stroke or fill to draw: which cached mesh, its colour and layer transform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeDraw {
    /// The stroke or fill whose cached mesh is drawn. Entries with no cached
    /// mesh are skipped.
    pub id: MeshId,
    /// Straight-alpha linear colour (opacity already applied).
    pub color: Rgba,
    /// Blends `color` into a second colour (opacity already applied);
    /// `None` draws flat.
    pub gradient: Option<Gradient>,
    /// Layer-to-document transform.
    pub transform: Mat4,
    /// The layer drawn, if any; consecutive draws of one layer are
    /// composited together.
    pub layer: Option<LayerId>,
    /// How the draw's layer combines with what is below it.
    pub blend: BlendMode,
}

/// Everything drawn on the canvas for one render.
#[derive(Clone, Copy, Debug, Default)]
pub struct CanvasFrame<'a> {
    /// Canvas fill colour; `None` draws a transparency checkerboard.
    pub background: Option<Rgba>,
    /// Strokes, bottom to top.
    pub strokes: &'a [StrokeDraw],
}

/// The layer-to-document transform as a matrix (scale, then rotate, then translate).
pub fn transform_matrix(transform: &Transform2D) -> Mat4 {
    Mat4::from_scale_rotation_translation(
        transform.scale.extend(1.0),
        Quat::from_rotation_z(transform.rotation),
        transform.translation.extend(0.0),
    )
}

/// What is drawn around the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Chrome {
    /// Workspace, shadow, outline and checkerboard, as in the editor.
    Editor,
    /// Only the canvas content, on a transparent clear (export).
    Bare,
}

/// A pipeline with one uniform buffer at group 0.
#[derive(Debug)]
struct UniformPass {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Renders the canvas into an offscreen 4x MSAA texture.
#[derive(Debug)]
pub struct CanvasRenderer {
    target: CanvasTarget,
    background: UniformPass,
    frame: UniformPass,
    stroke_globals: UniformPass,
    draw_layout: wgpu::BindGroupLayout,
    draw_buffer: wgpu::Buffer,
    draw_bind_group: wgpu::BindGroup,
    /// Distance between per-stroke uniforms in `draw_buffer`.
    draw_stride: u64,
    /// How many per-stroke uniforms fit in `draw_buffer`.
    draw_capacity: u64,
    composite_layout: wgpu::BindGroupLayout,
    /// One pipeline per mode, indexed like [`BlendMode::ALL`].
    composite: Vec<wgpu::RenderPipeline>,
    strokes: StrokeCache,
    /// Colours and sizes of the workspace, outline, shadow and checkerboard.
    pub style: CanvasStyle,
}

impl CanvasRenderer {
    /// Creates the pipelines and a 1x1 target; call [`Self::resize`] before rendering.
    pub fn new(device: &wgpu::Device) -> Self {
        let uniform_layout = |label, size, dynamic| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: dynamic,
                        min_binding_size: wgpu::BufferSize::new(size),
                    },
                    count: None,
                }],
            })
        };

        let background_layout = uniform_layout("canvas-background", BACKGROUND_UNIFORM_SIZE, false);
        let frame_layout = uniform_layout("canvas-frame", FRAME_UNIFORM_SIZE, false);
        let globals_layout = uniform_layout("stroke-globals", STROKE_GLOBALS_SIZE, false);
        let draw_layout = uniform_layout("stroke-draw", STROKE_DRAW_SIZE, true);

        let quad = |label, source, layout: &wgpu::BindGroupLayout| {
            create_pipeline(
                device,
                label,
                source,
                &[layout],
                &[],
                wgpu::PrimitiveTopology::TriangleStrip,
                false,
            )
        };
        let background_pipeline = quad("canvas-background", BACKGROUND_WGSL, &background_layout);
        let frame_pipeline = quad("canvas-frame", FRAME_WGSL, &frame_layout);
        let stroke_pipeline = create_pipeline(
            device,
            "stroke",
            STROKE_WGSL,
            &[&globals_layout, &draw_layout],
            &[Some(wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2],
            })],
            wgpu::PrimitiveTopology::TriangleList,
            true,
        );

        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("composite"),
            entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }),
        });
        let composite = (0..BlendMode::ALL.len())
            .map(|mode| create_composite_pipeline(device, &composite_layout, mode))
            .collect();

        let alignment = u64::from(device.limits().min_uniform_buffer_offset_alignment).max(1);
        let draw_stride = STROKE_DRAW_SIZE.div_ceil(alignment) * alignment;
        let draw_capacity = 64;
        let (draw_buffer, draw_bind_group) =
            create_draw_buffer(device, &draw_layout, draw_stride, draw_capacity);

        Self {
            target: CanvasTarget::new(device, 1, 1),
            background: uniform_pass(
                device,
                "canvas-background",
                background_pipeline,
                &background_layout,
                BACKGROUND_UNIFORM_SIZE,
            ),
            frame: uniform_pass(
                device,
                "canvas-frame",
                frame_pipeline,
                &frame_layout,
                FRAME_UNIFORM_SIZE,
            ),
            stroke_globals: uniform_pass(
                device,
                "stroke-globals",
                stroke_pipeline,
                &globals_layout,
                STROKE_GLOBALS_SIZE,
            ),
            draw_layout,
            draw_buffer,
            draw_bind_group,
            draw_stride,
            draw_capacity,
            composite_layout,
            composite,
            strokes: StrokeCache::new(),
            style: CanvasStyle::default(),
        }
    }

    /// Resizes the render target to `width` x `height` physical pixels.
    /// Returns true if the texture was recreated, so views registered
    /// elsewhere (for example with egui) must be updated.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        self.target.resize(device, width, height)
    }

    /// The render target.
    pub fn target(&self) -> &CanvasTarget {
        &self.target
    }

    /// The finished canvas, for display (see [`CanvasTarget::display_view`]).
    pub fn display_view(&self) -> &wgpu::TextureView {
        self.target.display_view()
    }

    /// The stroke mesh cache.
    pub fn stroke_cache(&self) -> &StrokeCache {
        &self.strokes
    }

    /// The stroke mesh cache, for uploading meshes and invalidating strokes.
    pub fn stroke_cache_mut(&mut self) -> &mut StrokeCache {
        &mut self.strokes
    }

    /// Makes sure every stroke and fill in `list` has a cached mesh (calling
    /// `tessellate` / `tessellate_fill` for new or changed ones) and returns
    /// the draw list, bottom to top: per layer, its fills, then its strokes.
    /// Colours get the brush/fill and layer opacity applied.
    pub fn prepare_render_list<M: MeshSource, F: MeshSource>(
        &mut self,
        device: &wgpu::Device,
        list: &RenderList<'_>,
        mut tessellate: impl FnMut(&Stroke) -> Option<M>,
        mut tessellate_fill: impl FnMut(&Fill) -> Option<F>,
    ) -> Vec<StrokeDraw> {
        let mut draws = Vec::new();
        for layer in &list.layers {
            let transform = transform_matrix(&layer.transform);
            for fill in layer.drawing.fills() {
                self.strokes
                    .ensure(device, fill.id, StrokeKey::of_fill(fill), || {
                        tessellate_fill(fill)
                    });
                draws.push(StrokeDraw {
                    id: fill.id.into(),
                    color: fill_color(fill, layer.opacity),
                    gradient: fill_gradient(fill, layer.opacity),
                    transform,
                    layer: Some(layer.layer_id),
                    blend: layer.blend,
                });
            }
            for stroke in layer.drawing.strokes() {
                self.strokes
                    .ensure(device, stroke.id, StrokeKey::of(stroke), || {
                        tessellate(stroke)
                    });
                draws.push(StrokeDraw {
                    id: stroke.id.into(),
                    color: stroke_color(stroke, layer.opacity),
                    gradient: None,
                    transform,
                    layer: Some(layer.layer_id),
                    blend: layer.blend,
                });
            }
        }
        draws
    }

    /// Records the canvas render into `encoder`. The result lands in
    /// [`Self::display_view`] once the encoder is submitted.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        viewport: &Viewport,
        frame: &CanvasFrame<'_>,
    ) {
        self.encode(device, queue, encoder, viewport, frame, Chrome::Editor);
    }

    /// Records a render of the bare canvas, for export and the preview
    /// window: no workspace, shadow or outline, a transparent clear, and no
    /// checkerboard (a `None` background stays transparent). `viewport`
    /// should map the canvas onto the whole target
    /// (see [`Viewport::filling`]).
    pub fn render_bare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        viewport: &Viewport,
        frame: &CanvasFrame<'_>,
    ) {
        self.encode(device, queue, encoder, viewport, frame, Chrome::Bare);
    }

    fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        viewport: &Viewport,
        frame: &CanvasFrame<'_>,
        chrome: Chrome,
    ) {
        self.strokes.tick();
        let (width, height) = self.target.size();
        let target_size = Vec2::new(width as f32, height as f32);
        // Physical pixels per logical point, from the actual target size so a
        // stale target still lines up with the viewport.
        let px_per_point = target_size / viewport.screen_size.max(Vec2::ONE);
        let scale = viewport.scale_factor;
        let (min, max) = viewport.canvas_screen_rect();
        let (rect_min, rect_max) = (min * px_per_point, max * px_per_point);
        let view_proj = viewport.view_projection().to_cols_array();
        let style = self.style;

        // Background.
        let mut u = Std140::default();
        u.floats(&view_proj);
        u.vec2(viewport.canvas_size);
        u.float(style.checker_size * scale);
        u.uint(u32::from(frame.background.is_none()));
        u.vec2(rect_min);
        u.vec2(Vec2::ZERO);
        u.color(frame.background.unwrap_or(Rgba::TRANSPARENT));
        u.color(style.checker[0]);
        u.color(style.checker[1]);
        queue.write_buffer(
            &self.background.buffer,
            0,
            &u.finish(BACKGROUND_UNIFORM_SIZE),
        );

        // Bounds outline and shadow.
        let shadow_radius = style.shadow_radius * scale;
        let shadow_offset = style.shadow_offset * scale;
        let mut u = Std140::default();
        u.vec2(rect_min);
        u.vec2(rect_max);
        u.vec2(target_size);
        u.float(
            shadow_radius + shadow_offset.abs().max_element() + style.outline_width * scale + 2.0,
        );
        u.float(style.outline_width * scale);
        u.color(style.shadow);
        u.color(style.outline);
        u.vec2(shadow_offset);
        u.float(shadow_radius);
        u.float(0.0);
        queue.write_buffer(&self.frame.buffer, 0, &u.finish(FRAME_UNIFORM_SIZE));

        // Strokes.
        let mut u = Std140::default();
        u.floats(&view_proj);
        queue.write_buffer(
            &self.stroke_globals.buffer,
            0,
            &u.finish(STROKE_GLOBALS_SIZE),
        );
        self.write_draws(device, queue, frame.strokes);

        let clear = match chrome {
            Chrome::Editor => wgpu::Color {
                r: f64::from(style.workspace.r),
                g: f64::from(style.workspace.g),
                b: f64::from(style.workspace.b),
                a: 1.0,
            },
            Chrome::Bare => wgpu::Color::TRANSPARENT,
        };
        // Strokes are clipped to the canvas.
        let scissor = scissor(rect_min, rect_max, (width, height));
        let groups = match scissor {
            Some(_) => blend_groups(frame.strokes),
            None => Vec::new(),
        };
        // Each group composites from one canvas texture into the other;
        // start where the last composite lands in the resolved texture.
        let canvases = [self.target.resolve_view(), self.target.scratch_view()];
        let mut current = groups.len() % 2;

        let mut pass = self.begin_msaa_pass(encoder, canvases[current], clear);
        if chrome == Chrome::Editor {
            pass.set_pipeline(&self.frame.pipeline);
            pass.set_bind_group(0, &self.frame.bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        if chrome == Chrome::Editor || frame.background.is_some() {
            pass.set_pipeline(&self.background.pipeline);
            pass.set_bind_group(0, &self.background.bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        drop(pass);

        for (range, blend) in groups {
            let mut pass =
                self.begin_msaa_pass(encoder, self.target.layer_view(), wgpu::Color::TRANSPARENT);
            if let Some((sx, sy, sw, sh)) = scissor {
                pass.set_scissor_rect(sx, sy, sw, sh);
            }
            pass.set_pipeline(&self.stroke_globals.pipeline);
            pass.set_bind_group(0, &self.stroke_globals.bind_group, &[]);
            for (slot, draw) in (range.start as u64..).zip(&frame.strokes[range]) {
                let Some(mesh) = self.strokes.get(draw.id) else {
                    continue;
                };
                let Some((vertices, indices)) = &mesh.buffers else {
                    continue;
                };
                let Ok(offset) = u32::try_from(slot * self.draw_stride) else {
                    break;
                };
                pass.set_bind_group(1, &self.draw_bind_group, &[offset]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            }
            drop(pass);

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("composite"),
                layout: &self.composite_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(canvases[current]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(self.target.layer_view()),
                    },
                ],
            });
            current = 1 - current;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: canvases[current],
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.composite[blend_index(blend)]);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.strokes.evict_unused(DEFAULT_MAX_UNUSED_RENDERS);
    }

    /// Starts a pass into the MSAA target (cleared to `clear`, depth cleared)
    /// that resolves into `resolve`.
    fn begin_msaa_pass<'e>(
        &self,
        encoder: &'e mut wgpu::CommandEncoder,
        resolve: &wgpu::TextureView,
        clear: wgpu::Color,
    ) -> wgpu::RenderPass<'e> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("canvas"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: self.target.msaa_view(),
                resolve_target: Some(resolve),
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: self.target.depth_view(),
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        })
    }

    /// Uploads the per-stroke uniforms, growing the buffer if needed.
    fn write_draws(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, draws: &[StrokeDraw]) {
        if draws.is_empty() {
            return;
        }
        let count = draws.len() as u64;
        if count > self.draw_capacity {
            self.draw_capacity = count.next_power_of_two();
            let (buffer, bind_group) = create_draw_buffer(
                device,
                &self.draw_layout,
                self.draw_stride,
                self.draw_capacity,
            );
            self.draw_buffer = buffer;
            self.draw_bind_group = bind_group;
        }
        let stride = usize::try_from(self.draw_stride).unwrap_or(256);
        let mut bytes = vec![0u8; stride * draws.len()];
        let count = draws.len() as f32;
        for (i, (chunk, draw)) in bytes.chunks_exact_mut(stride).zip(draws).enumerate() {
            let mut u = Std140::default();
            u.floats(&draw.transform.to_cols_array());
            u.color(draw.color);
            match draw.gradient {
                Some(g) => {
                    u.color(g.end_color);
                    u.vec2(g.start);
                    u.vec2(g.end);
                }
                None => {
                    u.color(draw.color);
                    u.floats(&[0.0; 4]);
                }
            }
            // Later strokes are nearer, so they pass the depth test over
            // earlier ones; a stroke's own overlapping triangles don't.
            u.float(stroke_depth(i, count));
            u.floats(&[0.0; 3]);
            let data = u.finish(STROKE_DRAW_SIZE);
            chunk[..data.len()].copy_from_slice(&data);
        }
        queue.write_buffer(&self.draw_buffer, 0, &bytes);
    }
}

/// Splits draws into groups composited one at a time: a run of Normal draws
/// is one group (source-over is associative, so they can share a texture);
/// a layer with another mode is its own group.
fn blend_groups(draws: &[StrokeDraw]) -> Vec<(Range<usize>, BlendMode)> {
    let mut groups: Vec<(Range<usize>, BlendMode)> = Vec::new();
    for (i, draw) in draws.iter().enumerate() {
        match groups.last_mut() {
            Some((range, blend))
                if *blend == draw.blend
                    && (draw.blend == BlendMode::Normal || draws[i - 1].layer == draw.layer) =>
            {
                range.end = i + 1;
            }
            _ => groups.push((i..i + 1, draw.blend)),
        }
    }
    groups
}

/// Position of `blend` in [`BlendMode::ALL`].
fn blend_index(blend: BlendMode) -> usize {
    BlendMode::ALL
        .iter()
        .position(|&b| b == blend)
        .unwrap_or_default()
}

/// Depth of the `index`-th of `count` strokes: from just under 1 (first,
/// furthest) down towards 0 (last, nearest).
fn stroke_depth(index: usize, count: f32) -> f32 {
    1.0 - (index as f32 + 1.0) / (count + 1.0)
}

/// Integer scissor rectangle covering `min..max`, clipped to the target.
fn scissor(min: Vec2, max: Vec2, (width, height): (u32, u32)) -> Option<(u32, u32, u32, u32)> {
    let limit = Vec2::new(width as f32, height as f32);
    let lo = min.floor().clamp(Vec2::ZERO, limit);
    let hi = max.ceil().clamp(Vec2::ZERO, limit);
    if !(lo.is_finite() && hi.is_finite()) || hi.x <= lo.x || hi.y <= lo.y {
        return None;
    }
    // Values are clamped to the target size, so the casts are exact.
    Some((
        lo.x as u32,
        lo.y as u32,
        (hi.x - lo.x) as u32,
        (hi.y - lo.y) as u32,
    ))
}

fn create_pipeline(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    layouts: &[&wgpu::BindGroupLayout],
    buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
    topology: wgpu::PrimitiveTopology,
    depth_test: bool,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layouts: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().copied().map(Some).collect();
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &layouts,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers,
        },
        primitive: wgpu::PrimitiveState {
            topology,
            ..Default::default()
        },
        // Strokes test and write depth; the quads ignore it.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: CANVAS_DEPTH_FORMAT,
            depth_write_enabled: Some(depth_test),
            depth_compare: Some(if depth_test {
                wgpu::CompareFunction::Less
            } else {
                wgpu::CompareFunction::Always
            }),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: CANVAS_SAMPLE_COUNT,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: CANVAS_RENDER_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_composite_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    mode: usize,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("composite"),
        source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("composite"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    let constants = [("MODE", mode as f64)];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("composite"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions {
                constants: &constants,
                ..Default::default()
            },
            targets: &[Some(wgpu::ColorTargetState {
                format: CANVAS_RENDER_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn uniform_pass(
    device: &wgpu::Device,
    label: &str,
    pipeline: wgpu::RenderPipeline,
    layout: &wgpu::BindGroupLayout,
    size: u64,
) -> UniformPass {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    });
    UniformPass {
        pipeline,
        buffer,
        bind_group,
    }
}

fn create_draw_buffer(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    stride: u64,
    capacity: u64,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("stroke-draws"),
        size: stride * capacity,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("stroke-draws"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: wgpu::BufferSize::new(STROKE_DRAW_SIZE),
            }),
        }],
    });
    (buffer, bind_group)
}

/// Packs uniform data. Callers lay fields out in WGSL order with explicit
/// padding; `finish` checks the total against the shader struct size.
#[derive(Default)]
struct Std140(Vec<u8>);

impl Std140 {
    fn float(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn uint(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }

    fn floats(&mut self, values: &[f32]) {
        values.iter().for_each(|&v| self.float(v));
    }

    fn vec2(&mut self, v: Vec2) {
        self.floats(&[v.x, v.y]);
    }

    fn color(&mut self, c: Rgba) {
        self.floats(&c.to_array());
    }

    fn finish(self, expected: u64) -> Vec<u8> {
        debug_assert_eq!(self.0.len() as u64, expected, "uniform layout mismatch");
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_runs_merge_and_other_modes_split_by_layer() {
        let (a, b) = (LayerId::new(), LayerId::new());
        let draw = |layer, blend| StrokeDraw {
            id: crate::MeshId::from(document::animation_core::StrokeId::new()),
            color: Rgba::WHITE,
            gradient: None,
            transform: Mat4::IDENTITY,
            layer: Some(layer),
            blend,
        };
        let n = BlendMode::Normal;
        let draws = [draw(a, n), draw(b, n), draw(b, n)];
        assert_eq!(blend_groups(&draws), [(0..3, n)]);
        let m = BlendMode::Multiply;
        let draws = [draw(a, m), draw(a, m), draw(b, m), draw(b, n), draw(a, n)];
        assert_eq!(blend_groups(&draws), [(0..2, m), (2..3, m), (3..5, n)]);
        assert_eq!(blend_groups(&[]), []);
    }

    #[test]
    fn scissor_clips_to_target() {
        assert_eq!(
            scissor(Vec2::new(-5.0, 10.2), Vec2::new(50.5, 300.0), (100, 200)),
            Some((0, 10, 51, 190))
        );
        assert_eq!(
            scissor(Vec2::new(150.0, 0.0), Vec2::new(160.0, 10.0), (100, 200)),
            None
        );
    }

    #[test]
    fn transform_matrix_matches_transform2d() {
        let t = Transform2D {
            translation: Vec2::new(10.0, -4.0),
            rotation: 0.7,
            scale: Vec2::new(2.0, 0.5),
        };
        let p = Vec2::new(3.0, 8.0);
        let m = transform_matrix(&t).transform_point3(p.extend(0.0));
        let expected = t.apply(p);
        assert!((Vec2::new(m.x, m.y) - expected).length() < 1e-4);
    }
}
