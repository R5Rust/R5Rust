// Bloom, Apex order (docs/apex-post.md section 2): prefilter with an exposure-aware soft knee into half res
// (result stays in UNEXPOSED units), blur via a downsample/upsample chain, added to the scene BEFORE exposure.

struct Expo {
    exposure: f32,
    sum: u32,
    count: u32,
    avg: f32,
};

struct Params {
    texel: vec2<f32>,   // 1 / source size
    cutoff: f32,        // c_bloomCutoff (exposed luminance)
    clamp_l: f32,       // c_bloomClamp
    weight: f32,        // upsample: weight of the smaller level added in
    mode: f32,
    pad0: f32,
    pad1: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<storage, read> expo: Expo;
@group(0) @binding(3) var<uniform> pr: Params;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VOut {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var o: VOut;
    o.clip = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

fn lum(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

// soft knee on exposed luminance: w = min(min(L, clamp), 2(L - cutoff)) / L, applied to the unexposed colour
fn knee(c: vec3<f32>, e: f32) -> vec3<f32> {
    let l = lum(max(c, vec3<f32>(0.0))) * e;
    if (l <= 0.0) { return vec3<f32>(0.0); }
    let w = min(min(l, pr.clamp_l), max(2.0 * (l - pr.cutoff), 0.0)) / l;
    return max(c, vec3<f32>(0.0)) * w;
}

// 4 bilinear taps at +-1 source texel = 4x4 box (Apex's _3 variant), knee per tap
@fragment
fn fs_prefilter(i: VOut) -> @location(0) vec4<f32> {
    let e = max(expo.exposure, 1e-4);
    let t = pr.texel;
    var c = knee(textureSampleLevel(src, samp, i.uv + vec2<f32>(-t.x, -t.y), 0.0).rgb, e);
    c += knee(textureSampleLevel(src, samp, i.uv + vec2<f32>(t.x, -t.y), 0.0).rgb, e);
    c += knee(textureSampleLevel(src, samp, i.uv + vec2<f32>(-t.x, t.y), 0.0).rgb, e);
    c += knee(textureSampleLevel(src, samp, i.uv + vec2<f32>(t.x, t.y), 0.0).rgb, e);
    return vec4<f32>(c * 0.25, 1.0);
}

// 13-tap downsample (stable, no fireflies)
@fragment
fn fs_down13(i: VOut) -> @location(0) vec4<f32> {
    let t = pr.texel;
    let a = textureSampleLevel(src, samp, i.uv + vec2<f32>(-2.0, -2.0) * t, 0.0).rgb;
    let b = textureSampleLevel(src, samp, i.uv + vec2<f32>(0.0, -2.0) * t, 0.0).rgb;
    let c = textureSampleLevel(src, samp, i.uv + vec2<f32>(2.0, -2.0) * t, 0.0).rgb;
    let d = textureSampleLevel(src, samp, i.uv + vec2<f32>(-2.0, 0.0) * t, 0.0).rgb;
    let e = textureSampleLevel(src, samp, i.uv, 0.0).rgb;
    let f = textureSampleLevel(src, samp, i.uv + vec2<f32>(2.0, 0.0) * t, 0.0).rgb;
    let g = textureSampleLevel(src, samp, i.uv + vec2<f32>(-2.0, 2.0) * t, 0.0).rgb;
    let h = textureSampleLevel(src, samp, i.uv + vec2<f32>(0.0, 2.0) * t, 0.0).rgb;
    let k = textureSampleLevel(src, samp, i.uv + vec2<f32>(2.0, 2.0) * t, 0.0).rgb;
    let l = textureSampleLevel(src, samp, i.uv + vec2<f32>(-1.0, -1.0) * t, 0.0).rgb;
    let m = textureSampleLevel(src, samp, i.uv + vec2<f32>(1.0, -1.0) * t, 0.0).rgb;
    let n = textureSampleLevel(src, samp, i.uv + vec2<f32>(-1.0, 1.0) * t, 0.0).rgb;
    let o = textureSampleLevel(src, samp, i.uv + vec2<f32>(1.0, 1.0) * t, 0.0).rgb;
    var r = e * 0.125 + (a + c + g + k) * 0.03125 + (b + d + f + h) * 0.0625 + (l + m + n + o) * 0.125;
    return vec4<f32>(r, 1.0);
}

// 3x3 tent upsample of the smaller level, blended additively onto the larger one
@fragment
fn fs_up(i: VOut) -> @location(0) vec4<f32> {
    let t = pr.texel;
    var r = textureSampleLevel(src, samp, i.uv, 0.0).rgb * 4.0;
    r += (textureSampleLevel(src, samp, i.uv + vec2<f32>(-t.x, 0.0), 0.0).rgb + textureSampleLevel(src, samp, i.uv + vec2<f32>(t.x, 0.0), 0.0).rgb
        + textureSampleLevel(src, samp, i.uv + vec2<f32>(0.0, -t.y), 0.0).rgb + textureSampleLevel(src, samp, i.uv + vec2<f32>(0.0, t.y), 0.0).rgb) * 2.0;
    r += textureSampleLevel(src, samp, i.uv + vec2<f32>(-t.x, -t.y), 0.0).rgb + textureSampleLevel(src, samp, i.uv + vec2<f32>(t.x, -t.y), 0.0).rgb
        + textureSampleLevel(src, samp, i.uv + vec2<f32>(-t.x, t.y), 0.0).rgb + textureSampleLevel(src, samp, i.uv + vec2<f32>(t.x, t.y), 0.0).rgb;
    return vec4<f32>(r / 16.0 * pr.weight, 1.0);
}
