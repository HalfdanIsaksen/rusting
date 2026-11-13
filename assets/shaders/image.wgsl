// Imported pictures (ADR-015): a quad in layer coordinates whose four
// vertices are the picture's top-left, top-right, bottom-right and
// bottom-left corners, so texture coordinates follow from the vertex index.
// Shares `Globals` and `Draw` with stroke.wgsl; `draw.color.a` is the
// image and layer opacity.

struct Globals {
    view_proj: mat4x4<f32>,
};

struct Draw {
    model: mat4x4<f32>,
    color: vec4<f32>,
    end_color: vec4<f32>,
    gradient: vec4<f32>,
    depth: f32,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> draw: Draw;
@group(2) @binding(0) var picture: texture_2d<f32>;
@group(2) @binding(1) var picture_sampler: sampler;

@vertex
fn vs_main(@location(0) position: vec2<f32>, @builtin(vertex_index) index: u32) -> VertexOut {
    var corners = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    var out: VertexOut;
    out.clip = globals.view_proj * draw.model * vec4<f32>(position, 0.0, 1.0);
    out.clip.z = draw.depth * out.clip.w;
    out.uv = corners[index % 4u];
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(picture, picture_sampler, in.uv);
    return vec4<f32>(texel.rgb, texel.a * draw.color.a);
}
