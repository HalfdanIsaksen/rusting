// Canvas background: a quad covering the canvas in document space, filled with
// a solid colour or (for transparent documents) a checkerboard.

struct Background {
    view_proj: mat4x4<f32>,
    // Canvas size in document units.
    canvas_size: vec2<f32>,
    // Checker cell size in physical pixels.
    checker_size: f32,
    // 0 = solid colour, 1 = checkerboard.
    mode: u32,
    // Canvas origin in physical pixels, so the checkerboard moves with the canvas.
    origin_px: vec2<f32>,
    _pad: vec2<f32>,
    color: vec4<f32>,
    checker_a: vec4<f32>,
    checker_b: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Background;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // Triangle strip: (0,0) (1,0) (0,1) (1,1).
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    return u.view_proj * vec4<f32>(corner * u.canvas_size, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    if u.mode == 0u {
        return u.color;
    }
    let cell = floor((position.xy - u.origin_px) / u.checker_size);
    let odd = (i32(cell.x) + i32(cell.y)) & 1;
    return select(u.checker_a, u.checker_b, odd == 1);
}
