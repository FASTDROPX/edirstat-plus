struct Uniforms { size: vec2<f32>, srgb: f32, pad: f32 }
@group(0) @binding(0) var<uniform> uniforms: Uniforms;
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) point: vec2<f32>,
    @location(1) @interpolate(flat) original: vec4<f32>,
    @location(2) @interpolate(flat) color: u32,
    @location(3) @interpolate(flat) border: u32,
    @location(4) @interpolate(flat) transition: f32,
    @location(5) @interpolate(flat) style: u32,
}
@vertex fn vs_main(@builtin(vertex_index) index: u32,
    @location(0) original: vec4<f32>,
    @location(1) color: u32, @location(2) border: u32,
    @location(3) transition: f32, @location(4) style: u32) -> Out {
    let corners = array<vec2<f32>, 6>(vec2(0.,0.),vec2(1.,0.),vec2(0.,1.),vec2(0.,1.),vec2(1.,0.),vec2(1.,1.));
    let lower = clamp(floor(original.xy + vec2(0.5)), vec2(0.), uniforms.size);
    let upper = clamp(floor(original.zw + vec2(0.5)), vec2(0.), uniforms.size);
    let p = mix(lower, upper, corners[index]);
    var out: Out;
    out.position = vec4(2. * p.x / uniforms.size.x - 1., 1. - 2. * p.y / uniforms.size.y, 0., 1.);
    out.point = p; out.original = original; out.color = color; out.border = border;
    out.transition = transition; out.style = style;
    return out;
}
fn unpack(c: u32) -> vec4<f32> {
    return vec4<f32>(f32(c & 255u), f32((c >> 8u) & 255u), f32((c >> 16u) & 255u), f32(c >> 24u)) / 255.;
}
@fragment fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let point = floor(in.point) + vec2(0.5);
    let uv = clamp((point - in.original.xy) / (in.original.zw - in.original.xy), vec2(0.), vec2(1.));
    var shade = 1.15 - 0.4 * uv.y;
    switch in.style {
        case 1u: { shade = 1.15 - 0.35 * uv.y - 0.06 * uv.x; }
        case 2u: { shade = 1.15 - 0.2 * (uv.x + uv.y); }
        case 3u: { shade = 1.25 - 0.6 * min(length(uv - vec2(0.4)), 1.); }
        case 4u: { shade = 1.; }
        case 7u, 8u: { shade = 0.5; }
        default: {}
    }
    let distances = min(point - in.original.xy, in.original.zw - point);
    var color = unpack(in.color);
    // Match the byte quantization of the old CPU texture, including animated shading.
    color = vec4(floor(clamp(color.rgb * (1. + (shade - 1.) * in.transition), vec3(0.), vec3(1.)) * 255.) / 255., color.a);
    if min(distances.x, distances.y) < 1. { color = unpack(in.border); }
    if uniforms.srgb > 0. {
        let linear = select(pow((color.rgb + vec3(0.055)) / 1.055, vec3(2.4)), color.rgb / 12.92, color.rgb <= vec3(0.04045));
        color = vec4(linear, color.a);
    }
    return color;
}
