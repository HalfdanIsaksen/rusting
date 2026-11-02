// Stroke and fill meshes: triangles in layer coordinates, one flat colour per
// draw, or a linear gradient from `color` to `end_color`.

struct Globals {
    view_proj: mat4x4<f32>,
};

struct Draw {
    // Layer-to-document transform.
    model: mat4x4<f32>,
    // Straight-alpha linear colour (at the gradient start).
    color: vec4<f32>,
    // Straight-alpha linear colour at the gradient end; equals `color` for
    // flat draws.
    end_color: vec4<f32>,
    // Gradient start (xy) and end (zw) in layer coordinates.
    gradient: vec4<f32>,
    // Depth in 0..1; later strokes are nearer. Each stroke covers a pixel
    // once, so overlapping triangles of a translucent stroke don't darken.
    depth: f32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) local: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> draw: Draw;

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> VertexOut {
    var out: VertexOut;
    out.clip = globals.view_proj * draw.model * vec4<f32>(position, 0.0, 1.0);
    out.clip.z = draw.depth * out.clip.w;
    out.local = position;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let start = draw.gradient.xy;
    let axis = draw.gradient.zw - start;
    let len2 = dot(axis, axis);
    var t = 0.0;
    if len2 > 1e-8 {
        t = clamp(dot(in.local - start, axis) / len2, 0.0, 1.0);
    }
    return mix(draw.color, draw.end_color, t);
}
