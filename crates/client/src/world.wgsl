// World pass: albedo x (baked lightmap + hemisphere ambient + sun shadowed by lightmap alpha).
// Ported from r5-web-client's viewer (three.js MeshStandard with lightMapIntensity 4*pi).
// Everything is camera-relative: positions arrive in Source units and the camera sits at origin.

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    to_sun: vec4<f32>,     // xyz = direction towards the sun
    sun_color: vec4<f32>,  // rgb, w = intensity
    sky: vec4<f32>,        // hemisphere sky colour, w = intensity
    ground: vec4<f32>,     // hemisphere ground colour
    fog: vec4<f32>,        // rgb, w = start distance
    misc: vec4<f32>,       // x = fog end, y = time, z = exposure
};

@group(0) @binding(0) var<uniform> fr: Frame;
@group(0) @binding(1) var lm_tex: texture_2d<f32>;
@group(0) @binding(2) var lm_samp: sampler;
@group(0) @binding(3) var mat_samp: sampler;

struct Mat {
    color: vec4<f32>,
    params: vec4<f32>,  // x = alpha cutoff (mask), y = lightmapped, z = alpha mode
};
@group(1) @binding(0) var<uniform> mt: Mat;
@group(1) @binding(1) var base_tex: texture_2d<f32>;

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) col: vec4<f32>,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) col: vec4<f32>,
};

@vertex
fn vs_main(v: VIn) -> VOut {
    var o: VOut;
    let rel = v.pos - fr.cam_pos.xyz;
    o.clip = fr.view_proj * vec4<f32>(rel, 1.0);
    o.rel = rel;
    o.nrm = v.nrm.xyz;
    o.uv0 = v.uv0;
    o.uv1 = v.uv1;
    o.col = v.col;
    return o;
}

fn fog_mix(c: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let d = length(rel);
    let f = clamp((d - fr.fog.w) / (fr.misc.x - fr.fog.w), 0.0, 1.0);
    return mix(c, fr.fog.rgb, f * f * (3.0 - 2.0 * f) * 0.85);
}

@fragment
fn fs_main(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let tex = textureSample(base_tex, mat_samp, i.uv0);
    let lm = textureSample(lm_tex, lm_samp, i.uv1);
    var albedo = tex * mt.color * i.col;

    var n = normalize(i.nrm + vec3<f32>(0.0, 0.0, 1e-5));
    if (!front) { n = -n; }

    let ndl = max(dot(n, fr.to_sun.xyz), 0.0);
    // lightmapped: the bake already holds sky + bounce light, so no hemisphere fill on top
    var light = mix(fr.ground.rgb, fr.sky.rgb, n.z * 0.5 + 0.5) * fr.sky.w;
    var sun_vis = 1.0;
    if (mt.params.y > 0.5) {
        // + a little hemisphere fill: the bake has no interior lights, so indoors would go black
        light = lm.rgb * 4.0 + light * 0.45;
        sun_vis = smoothstep(0.3, 0.6, lm.a);
    }
    light += fr.sun_color.rgb * fr.sun_color.w * ndl * sun_vis;
    var c = fog_mix(albedo.rgb * light, i.rel);
    if (fr.misc.w > 0.5 && fr.misc.w < 1.5) { c = vec3<f32>(lm.a); }
    if (fr.misc.w > 1.5 && fr.misc.w < 2.5) { c = lm.rgb * 4.0; }
    if (fr.misc.w > 2.5) { c = albedo.rgb; }

    var a = albedo.a;
    if (mt.params.z > 0.5 && mt.params.z < 1.5) {
        // alpha-tested: sharpen around the cutoff so alpha-to-coverage gives crisp, AA'd edges
        a = (a - mt.params.x) / max(fwidth(a), 1e-4) + 0.5;
    } else if (mt.params.z < 0.5) {
        a = 1.0;
    }
    return vec4<f32>(c, a);
}

// Sky: fullscreen triangle at the far plane (reverse-Z: depth 0), drawn first.
struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SkyOut {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u)) * 2.0 - 1.0;
    var o: SkyOut;
    o.clip = vec4<f32>(p, 0.0, 1.0);
    o.ndc = p;
    return o;
}

@fragment
fn fs_sky(i: SkyOut) -> @location(0) vec4<f32> {
    let w = fr.inv_view_proj * vec4<f32>(i.ndc, 0.5, 1.0);
    let dir = normalize(w.xyz / w.w);
    let up = clamp(dir.z, -1.0, 1.0);
    let zenith = vec3<f32>(0.16, 0.32, 0.62);
    var c = mix(fr.fog.rgb, zenith, pow(max(up, 0.0), 0.55));
    if (up < 0.0) { c = mix(fr.fog.rgb, fr.ground.rgb * 0.8, min(-up * 4.0, 1.0)); }
    let s = max(dot(dir, fr.to_sun.xyz), 0.0);
    c += fr.sun_color.rgb * (pow(s, 900.0) * 30.0 + pow(s, 12.0) * 0.35);
    return vec4<f32>(c, 1.0);
}
