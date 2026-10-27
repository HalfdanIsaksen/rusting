// Stroke meshes: triangles in layer coordinates, one flat colour per stroke.

struct Globals {
    view_proj: mat4x4<f32>,
};

struct Draw {
    // Layer-to-document transform.
    model: mat4x4<f32>,
    // Straight-alpha linear colour.
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> draw: Draw;

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return globals.view_proj * draw.model * vec4<f32>(position, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return draw.color;
}
