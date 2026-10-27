// Canvas bounds: a thin outline just outside the canvas edge plus a soft drop
// shadow. Works in physical pixels so the outline stays crisp at any zoom.
// Pixels inside the canvas are discarded; the background pass fills them.

struct Frame {
    // Canvas rectangle in physical pixels: min.xy, max.xy.
    rect: vec4<f32>,
    // Render target size in physical pixels.
    target_size: vec2<f32>,
    // How far the drawn quad extends beyond the canvas, in pixels.
    margin: f32,
    outline_width: f32,
    shadow_color: vec4<f32>,
    outline_color: vec4<f32>,
    shadow_offset: vec2<f32>,
    shadow_radius: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: Frame;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32(index & 1u), f32((index >> 1u) & 1u));
    let px = mix(u.rect.xy - vec2<f32>(u.margin), u.rect.zw + vec2<f32>(u.margin), corner);
    let ndc = px / u.target_size * 2.0 - 1.0;
    return vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
}

// Signed distance from `p` to an axis-aligned box (negative inside).
fn sd_box(p: vec2<f32>, min_corner: vec2<f32>, max_corner: vec2<f32>) -> f32 {
    let center = (min_corner + max_corner) * 0.5;
    let half_size = (max_corner - min_corner) * 0.5;
    let q = abs(p - center) - half_size;
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = position.xy;
    let d = sd_box(p, u.rect.xy, u.rect.zw);
    if d < 0.0 {
        discard;
    }
    let outline_a = u.outline_color.a
        * (1.0 - smoothstep(u.outline_width - 0.5, u.outline_width + 0.5, d));

    let ds = sd_box(p - u.shadow_offset, u.rect.xy, u.rect.zw);
    let falloff = 1.0 - smoothstep(-0.25 * u.shadow_radius, u.shadow_radius, ds);
    let shadow_a = u.shadow_color.a * falloff * falloff;

    // Outline composited over the shadow (straight alpha).
    let a = outline_a + shadow_a * (1.0 - outline_a);
    if a <= 0.0 {
        discard;
    }
    let rgb = (u.outline_color.rgb * outline_a + u.shadow_color.rgb * shadow_a * (1.0 - outline_a)) / a;
    return vec4<f32>(rgb, a);
}
