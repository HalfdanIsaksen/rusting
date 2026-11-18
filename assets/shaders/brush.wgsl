// Brushed strokes (ADR-022): one quad per stamp of the brush tip, in layer
// coordinates. Each vertex carries the tip texture coordinates and the
// stamp's opacity. The paper grain is sampled in layer coordinates, so it
// stays fixed to the paper while strokes move over it.
//
// Shares `Globals` and `Draw` with stroke.wgsl. `draw.color` is the stroke
// colour with stroke and layer opacity; `draw.grain_scale` is layer units
// per grain pixel and `draw.grain_strength` the grain strength (0 = no grain; 1 = only
// the brighter half of the grain takes paint).
//
// All stamps of a stroke share one depth and the pipeline tests with
// LessEqual, so overlapping stamps build up like graphite, while later
// items still cover the stroke.

struct Globals {
    view_proj: mat4x4<f32>,
};

struct Draw {
    model: mat4x4<f32>,
    color: vec4<f32>,
    end_color: vec4<f32>,
    gradient: vec4<f32>,
    depth: f32,
    grain_scale: f32,
    grain_strength: f32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) opacity: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> draw: Draw;
@group(2) @binding(0) var tip: texture_2d<f32>;
@group(2) @binding(1) var tip_sampler: sampler;
@group(2) @binding(2) var grain: texture_2d<f32>;
@group(2) @binding(3) var grain_sampler: sampler;

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) opacity: f32,
) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * draw.model * vec4<f32>(position, 0.0, 1.0);
    out.clip.z = draw.depth * out.clip.w;
    out.local = position;
    out.uv = uv;
    out.opacity = opacity;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let t = textureSample(tip, tip_sampler, in.uv);
    let coverage = t.r * t.a;
    let size = vec2<f32>(textureDimensions(grain, 0));
    let g = textureSample(grain, grain_sampler, in.local / (max(draw.grain_scale, 1e-3) * size)).r;
    // Height mode: the paper takes paint only where the grain is above a
    // threshold that rises with the strength. Valleys below it stay empty
    // however many stamps pass over them, so the grain survives build-up.
    let strength = clamp(draw.grain_strength, 0.0, 1.0);
    let paper = smoothstep(strength - 0.5, strength, g);
    let alpha = coverage * in.opacity * paper * draw.color.a;
    if alpha <= 0.0 {
        discard;
    }
    return vec4<f32>(draw.color.rgb, alpha);
}
