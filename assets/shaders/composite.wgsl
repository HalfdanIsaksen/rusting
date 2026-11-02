// Composites one layer (or a run of Normal layers) onto the canvas below it
// with a blend mode. Both textures and the output hold premultiplied linear
// colour; each target pixel reads the same pixel of both.

// Index into `BlendMode::ALL`.
override MODE: u32 = 0u;

@group(0) @binding(0) var backdrop: texture_2d<f32>;
@group(0) @binding(1) var layer: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the whole target.
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// The blend function B(cb, cs) of straight backdrop and source colours
// (W3C Compositing and Blending, separable modes).
fn blend(cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch MODE {
        // Multiply.
        case 1u: {
            return cb * cs;
        }
        default: {
            return cs;
        }
    }
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let b = textureLoad(backdrop, pixel, 0);
    let s = textureLoad(layer, pixel, 0);
    let cb = b.rgb / max(b.a, 1e-6);
    let cs = s.rgb / max(s.a, 1e-6);
    // Source-over with the blended colour where both are present.
    let rgb = s.rgb * (1.0 - b.a) + b.rgb * (1.0 - s.a) + s.a * b.a * blend(cb, cs);
    return vec4<f32>(rgb, s.a + b.a * (1.0 - s.a));
}
