struct Uniforms {
    size: vec2<f32>,
    gonio_scale: vec2<f32>,
    decay: f32,
    _pad: f32,
}

@group(0) @binding(0) var<uniform> uni: Uniforms;
@group(0) @binding(1) var prev_tex: texture_2d<f32>;
@group(0) @binding(2) var prev_samp: sampler;

struct FsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) size_px: vec2<f32>,
    @location(3) rounded: f32,
    @location(4) glow: f32,
}

@vertex
fn vs_fs(@builtin(vertex_index) vid: u32) -> FsOut {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: FsOut;
    out.clip = vec4<f32>(p[vid], 0.0, 1.0);
    out.uv = p[vid] * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
    out.color = vec4<f32>(1.0);
    out.size_px = uni.size;
    out.rounded = 0.0;
    out.glow = 0.0;
    return out;
}

@fragment
fn fs_fade(in: FsOut) -> @location(0) vec4<f32> {
    let px = vec2<f32>(1.4, 1.4) / max(uni.size, vec2<f32>(1.0));
    var acc = textureSample(prev_tex, prev_samp, in.uv) * 0.5;
    acc += textureSample(prev_tex, prev_samp, in.uv + vec2<f32>(px.x, 0.0)) * 0.125;
    acc += textureSample(prev_tex, prev_samp, in.uv - vec2<f32>(px.x, 0.0)) * 0.125;
    acc += textureSample(prev_tex, prev_samp, in.uv + vec2<f32>(0.0, px.y)) * 0.125;
    acc += textureSample(prev_tex, prev_samp, in.uv - vec2<f32>(0.0, px.y)) * 0.125;
    return acc * uni.decay;
}

@fragment
fn fs_blit(in: FsOut) -> @location(0) vec4<f32> {
    let c = textureSample(prev_tex, prev_samp, in.uv);
    return vec4<f32>(c.rgb, 1.0);
}

fn heat_rgb(t: f32) -> vec3<f32> {
    let x = clamp(t, 0.0, 1.0);
    let dark = vec3<f32>(0.04, 0.12, 0.38);
    let light = vec3<f32>(0.52, 0.82, 1.0);
    return mix(dark, light, x);
}

@vertex
fn vs_gonio(@builtin(vertex_index) vid: u32, @location(0) lr: vec2<f32>) -> FsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let mid = (lr.x + lr.y) * uni.gonio_scale.y;
    let side = (lr.y - lr.x) * uni.gonio_scale.x;
    let c = vec2<f32>(side, mid);
    let p = c + corners[vid % 6u] * 0.005;
    var out: FsOut;
    out.clip = vec4<f32>(p.x, p.y, 0.0, 1.0);
    out.uv = vec2<f32>(0.0);
    let amp = clamp(length(lr) * 3.2, 0.0, 1.0);
    out.color = vec4<f32>(heat_rgb(amp), 0.018 + 0.04 * amp);
    out.size_px = vec2<f32>(0.0);
    out.rounded = 0.0;
    out.glow = 0.0;
    return out;
}

@fragment
fn fs_add(in: FsOut) -> @location(0) vec4<f32> {
    return in.color;
}

@vertex
fn vs_note(
    @builtin(vertex_index) vid: u32,
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) extra: vec4<f32>,
) -> FsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let uv = corners[vid % 6u];
    let glow = extra.y;
    let pad = vec2<f32>(4.0, 4.0) * glow / max(uni.size, vec2<f32>(1.0));
    let p = rect.xy - pad + uv * (rect.zw + pad * 2.0);
    var out: FsOut;
    out.clip = vec4<f32>(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    out.color = color;
    out.size_px = rect.zw * uni.size;
    out.rounded = extra.x;
    out.glow = glow;
    return out;
}

fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_note(in: FsOut) -> @location(0) vec4<f32> {
    let inner = max(in.size_px, vec2<f32>(1.0));
    let pad_px = 4.0 * in.glow;
    let outer = inner + vec2<f32>(pad_px * 2.0);
    let p = (in.uv - vec2<f32>(0.5)) * outer;
    let r_note = min(7.0, min(inner.x, inner.y) * 0.5);
    let r_box = min(inner.x, inner.y) * 0.18;
    let r = select(0.0, select(r_box, r_note, in.glow >= 0.5), in.rounded >= 0.5);
    let d = sd_round_box(p, inner * 0.5, r);
    let aa = max(fwidth(d), 0.75);
    let fill = 1.0 - smoothstep(-aa, aa, d);
    let edge = 1.0 - smoothstep(-r * 0.35, r * 0.15, d);
    let rgb = in.color.rgb * (0.82 + 0.22 * edge);
    let rim = exp(-max(d, 0.0) / 1.15) * 0.28 * in.glow;
    let a = in.color.a * (fill + rim * (1.0 - fill));
    return vec4<f32>(rgb, a);
}
