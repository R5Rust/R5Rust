// Final pass: either the loading screen (mode 0) or the HDR scene tonemapped with ACES (mode 1).

struct Post {
    params: vec4<f32>, // x = mode, y = progress 0..1, z = exposure, w = output gamma (1 = surface is sRGB)
    screen: vec4<f32>, // xy = target size, zw = loadscreen image size
    bloom: vec4<f32>,  // x = amount
};

@group(0) @binding(0) var<uniform> pp: Post;
@group(0) @binding(1) var hdr_tex: texture_2d<f32>;
@group(0) @binding(2) var load_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;
struct Expo {
    exposure: f32,
    sum: i32,
    count: u32,
    avg_log: f32,
};
@group(0) @binding(4) var<storage, read> expo: Expo;
@group(0) @binding(5) var bloom_tex: texture_2d<f32>;
@group(0) @binding(6) var grading_lut: texture_3d<f32>;

// SMPTE ST 2084 (PQ). input: luminance / 10000 nits
fn pq_encode(y: vec3<f32>) -> vec3<f32> {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let p = pow(max(y, vec3<f32>(0.0)), vec3<f32>(m1));
    return pow((c1 + c2 * p) / (1.0 + c3 * p), vec3<f32>(m2));
}
fn pq_decode(e: vec3<f32>) -> vec3<f32> {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let p = pow(max(e, vec3<f32>(0.0)), vec3<f32>(1.0 / m2));
    return pow(max(p - c1, vec3<f32>(0.0)) / (c2 - c3 * p), vec3<f32>(1.0 / m1));
}

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

fn aces(x: vec3<f32>) -> vec3<f32> {
    // Stephen Hill's fit (same curve family as three.js ACESFilmicToneMapping)
    let m1 = mat3x3<f32>(
        vec3<f32>(0.59719, 0.07600, 0.02840),
        vec3<f32>(0.35458, 0.90834, 0.13383),
        vec3<f32>(0.04823, 0.01566, 0.83777));
    let m2 = mat3x3<f32>(
        vec3<f32>(1.60475, -0.10208, -0.00327),
        vec3<f32>(-0.53108, 1.10813, -0.07276),
        vec3<f32>(-0.07367, -0.00605, 1.07602));
    let v = m1 * x;
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return clamp(m2 * (a / b), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn out_color(c: vec3<f32>) -> vec4<f32> {
    if (pp.params.w > 0.5) { return vec4<f32>(c, 1.0); }
    return vec4<f32>(pow(c, vec3<f32>(1.0 / 2.2)), 1.0);
}

@fragment
fn fs_main(i: VOut) -> @location(0) vec4<f32> {
    if (pp.params.x < 0.5) {
        // loading screen: cover-fit the image, progress bar along the bottom
        let sa = pp.screen.x / pp.screen.y;
        let ia = pp.screen.z / max(pp.screen.w, 1.0);
        var uv = i.uv;
        if (sa > ia) { uv.y = (uv.y - 0.5) * ia / sa + 0.5; } else { uv.x = (uv.x - 0.5) * sa / ia + 0.5; }
        var c = textureSampleLevel(load_tex, samp, uv, 0.0).rgb;
        let px = i.uv * pp.screen.xy;
        let bar_h = 6.0;
        let margin = pp.screen.x * 0.06;
        if (px.y > pp.screen.y - 40.0 - bar_h && px.y < pp.screen.y - 40.0 && px.x > margin && px.x < pp.screen.x - margin) {
            let t = (px.x - margin) / (pp.screen.x - 2.0 * margin);
            if (t < pp.params.y) { c = vec3<f32>(0.85, 0.12, 0.08); } else { c = mix(c, vec3<f32>(0.0), 0.6); }
        }
        return out_color(c);
    }
    // Apex composite: (scene + bloom) * exposure -> tonemap   (bloom added before exposure)
    let h = textureSampleLevel(hdr_tex, samp, i.uv, 0.0).rgb + textureSampleLevel(bloom_tex, samp, i.uv, 0.0).rgb * pp.bloom.x;
    var c = h * pp.params.z * max(expo.exposure, 1e-3);
    if (pp.bloom.y > 0.5) {
        // HDR colour correction (EnginePost 0xB93D3531CBE5AD02): 1.0 scene = 100 nits, hue-preserving clamp at
        // 10k nits, 33^3 LUT in PQ space, decoded back to scene units
        let m = max(max(c.r, c.g), c.b);
        let pq = pq_encode(c * 0.01 * min(1.0, 100.0 / max(m, 1e-6)));
        let g = textureSampleLevel(grading_lut, samp, pq * (32.0 / 33.0) + 0.5 / 33.0, 0.0).rgb;
        c = pq_decode(g) * 100.0;
    }
    // tonemap: Apex picks one of six at runtime (c_tonemapFunction); z = 0 ACES fit, 1 = function 0
    if (pp.bloom.z > 0.5) {
        let t = c * (10.0 * c + 0.3) / (c * (10.0 * c + 0.5) + 1.5);
        return out_color(clamp(t, vec3<f32>(0.0), vec3<f32>(1.0)));
    }
    return out_color(aces(c));
}
