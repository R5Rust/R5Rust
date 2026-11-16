// World + static-prop shading, ported from Apex S21's own world (wldc) and prop (rgdp) pixel shaders.
// Formulas and constants: docs/apex-lighting.md (section numbers in comments). Camera-relative positions,
// linear HDR, pi-folded light units (never divide by pi), tonemap happens in post.

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    to_sun: vec4<f32>,     // c_sunDir (towards the sun)
    sun_color: vec4<f32>,  // c_sunColor
    sky: vec4<f32>,        // c_skyColor
    ground: vec4<f32>,
    fog: vec4<f32>,
    misc: vec4<f32>,       // y = time, z = exposure, w = debug view
    shadow_vp: array<mat4x4<f32>, 4>,
    cascade_r: vec4<f32>,
    fog_k: array<vec4<f32>, 5>,
};

struct Light {
    pos: vec3<f32>,
    rcp_r: f32,
    color: vec3<f32>,
    flags: u32,          // 8 inverse-square, 16 spot, 64 skip
    spot_dir: vec3<f32>,
    spot_bias: f32,
    atten_lin: f32,
    atten_quad: f32,
    spot_exp: f32,
    spec_int: f32,
    emitter_r: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
};

struct Probe {
    sh: array<vec4<f32>, 3>,
    sky: vec4<f32>,      // xyz sky vector, w baked sun visibility
    idx: vec4<i32>,
    wts: vec4<f32>,
};

@group(0) @binding(0) var<uniform> fr: Frame;
@group(0) @binding(1) var lm0_tex: texture_2d<f32>;
@group(0) @binding(2) var lm1_tex: texture_2d<f32>;
@group(0) @binding(3) var rtl_ids: texture_2d<u32>;
@group(0) @binding(4) var rtl_vis: texture_2d<f32>;
@group(0) @binding(5) var lm_samp: sampler;
@group(0) @binding(6) var mat_samp: sampler;
@group(0) @binding(7) var shadow_tex: texture_depth_2d_array;
@group(0) @binding(8) var shadow_samp: sampler_comparison;
@group(0) @binding(9) var<storage, read> lights: array<Light>;
@group(0) @binding(10) var<storage, read> rtl_index: array<u32>;
@group(0) @binding(11) var<storage, read> probes: array<Probe>;
@group(0) @binding(12) var cubes: texture_cube_array<f32>;
@group(0) @binding(13) var<storage, read> cube_ambient: array<f32>;
@group(0) @binding(14) var sky_cube: texture_cube<f32>;

struct Mat {
    color: vec4<f32>,
    params: vec4<f32>,   // x alpha cutoff, y lightmapped, z alpha mode, w world surface (vertex colour)
    params2: vec4<f32>,  // x has AO, y has emissive
    albedo_tint: vec4<f32>,
    emissive_tint: vec4<f32>,
    emissive_edge: vec4<f32>,  // exponent, inner, outer
    blend: vec4<f32>,          // x 1 two-layer / 2 transparent overlay, y c_layerBlendRamp, z layer 0 world-planar, w has detail normal
    uv1: vec4<f32>,            // c_uv1RotScaleX.xy, c_uv1RotScaleY.xy
    uv_t: vec4<f32>,           // c_uv1Translate.xy, c_uv2Translate.xy
    uv2: vec4<f32>,            // c_uv2RotScaleX.xy, c_uv2RotScaleY.xy
    albedo2_tint: vec4<f32>,
    layer2: vec4<f32>,         // x has gloss2, y has spec2, z perf gloss L0, w perf gloss L1
    perf_spec: vec4<f32>,      // rgb L0 spec constant, w has spec texture
    perf_spec2: vec4<f32>,
};
@group(1) @binding(0) var<uniform> mt: Mat;
@group(1) @binding(1) var albedo_tex: texture_2d<f32>;
@group(1) @binding(2) var normal_tex: texture_2d<f32>;
@group(1) @binding(3) var gloss_tex: texture_2d<f32>;
@group(1) @binding(4) var spec_tex: texture_2d<f32>;
@group(1) @binding(5) var ao_tex: texture_2d<f32>;
@group(1) @binding(6) var emis_tex: texture_2d<f32>;
@group(1) @binding(7) var albedo2_tex: texture_2d<f32>;
@group(1) @binding(8) var normal2_tex: texture_2d<f32>;
@group(1) @binding(9) var gloss2_tex: texture_2d<f32>;
@group(1) @binding(10) var spec2_tex: texture_2d<f32>;
@group(1) @binding(11) var blend_tex: texture_2d<f32>;
@group(1) @binding(12) var detail_n_tex: texture_2d<f32>;

// tangent frame from screen derivatives of a UV set; N = normalize(T*x + B*y + Nvtx)
fn tbn_normal(nv: vec3<f32>, rel: vec3<f32>, uv: vec2<f32>, n2: vec2<f32>) -> vec3<f32> {
    let dp1 = dpdx(rel);
    let dp2 = dpdy(rel);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2p = cross(dp2, nv);
    let dp1p = cross(nv, dp1);
    var t = dp2p * duv1.x + dp1p * duv2.x;
    var b = dp2p * duv1.y + dp1p * duv2.y;
    let inv = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-20));
    return normalize(t * inv * n2.x + b * inv * n2.y + nv);
}

fn xform(p: vec2<f32>, rs: vec4<f32>, t: vec2<f32>) -> vec2<f32> {
    let q = p + t;
    return vec2<f32>(dot(q, rs.xy), dot(q, rs.zw));
}

struct VIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) col: vec4<f32>,
    @location(5) probe: u32,
};

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) col: vec4<f32>,
    @location(5) @interpolate(flat) probe: u32,
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
    o.probe = v.probe;
    return o;
}

fn sq(x: f32) -> f32 { return x * x; }
fn pow5(x: f32) -> f32 { let x2 = x * x; return x2 * x2 * x; }
fn lum(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

// ---------------------------------------------------------------- sun shadow (section 3)

const SHADOW_RES: f32 = 2048.0;

fn pcf(i: i32, uv: vec2<f32>, z: f32) -> f32 {
    // 3x3 tent: corners 1/16, edges 1/8, centre 1/4
    var s = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let w = select(select(1.0 / 16.0, 1.0 / 8.0, x == 0 || y == 0), 0.25, x == 0 && y == 0);
            s += w * textureSampleCompareLevel(shadow_tex, shadow_samp, uv + vec2<f32>(f32(x), f32(y)) / SHADOW_RES, i, z);
        }
    }
    return s;
}

// the cascade *replaces* the baked sun visibility inside its range, fading to the next one (or baked) at the edge
fn sun_shadow(wp: vec3<f32>, nv: vec3<f32>, baked: f32) -> f32 {
    for (var i = 0; i < 4; i++) {
        let texel = 2.0 * fr.cascade_r[i] / SHADOW_RES;
        let c = fr.shadow_vp[i] * vec4<f32>(wp + nv * texel * 1.5, 1.0);
        let m = max(abs(c.x), abs(c.y));
        if (m < 0.98 && c.z < 1.0) {
            let cur = pcf(i, vec2<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5), c.z);
            let w = clamp((0.98 - m) / 0.15, 0.0, 1.0);
            var next = baked;
            if (w < 1.0 && i < 3) {
                let t2 = 2.0 * fr.cascade_r[i + 1] / SHADOW_RES;
                let c2 = fr.shadow_vp[i + 1] * vec4<f32>(wp + nv * t2 * 1.5, 1.0);
                if (max(abs(c2.x), abs(c2.y)) < 0.98) {
                    next = pcf(i + 1, vec2<f32>(c2.x * 0.5 + 0.5, 0.5 - c2.y * 0.5), c2.z);
                }
            }
            return mix(next, cur, w);
        }
    }
    return baked;
}

// ---------------------------------------------------------------- BRDF (section 5)

struct Acc {
    diff: vec3<f32>,
    multi: vec3<f32>,
    spec: vec3<f32>,
};

struct Surf {
    n: vec3<f32>,
    v: vec3<f32>,
    f0: vec3<f32>,
    a: f32,
    a2: f32,
    nov: f32,
    one_minus_lf0: f32,
};

fn brdf(acc: ptr<function, Acc>, s: Surf, ld: vec3<f32>, ls: vec3<f32>, atten: f32, c: vec3<f32>, spec_scale: f32) {
    if (atten <= 0.0) { return; }
    let nol = clamp(dot(s.n, ld), 0.0, 1.0);
    let hd = normalize(ld + s.v);
    let nohd = clamp(dot(s.n, hd), 0.0, 1.0);
    let facing = clamp(0.5 + 0.5 * dot(ld, s.v), 0.0, 1.0);
    let rough_d = facing * (0.9375 - 0.416667 * facing) * (nohd + 0.5) / max(nohd, 0.05) * s.one_minus_lf0;
    let smooth_d = 1.05 * sq(1.0 - 0.5 * pow5(1.0 - nol));
    let diff = mix(smooth_d, rough_d, s.a) * atten;
    let multi = 0.379271 * s.a * s.one_minus_lf0 * atten;
    var hs = ls + s.v;
    hs = select(s.n, normalize(hs), dot(hs, hs) > 1e-8);
    let noh = clamp(dot(s.n, hs), 0.0, 1.0);
    let nols = clamp(dot(s.n, ls), 0.0, 1.0);
    let f = s.f0 + (1.0 - s.f0) * pow5(1.0 - clamp(dot(ls, hs), 0.0, 1.0));
    let dd = sq(noh * noh * (s.a2 - 1.0) + 1.0);
    let vis = max(mix(2.0 * s.nov * nols, s.nov + nols, s.a), 0.00125);
    let spec = f * (s.a2 / (2.0 * vis * dd)) * atten * spec_scale;
    (*acc).diff += diff * c;
    (*acc).multi += multi * c;
    (*acc).spec += spec * c;
}

// ---------------------------------------------------------------- real-time lights (section 4.2)

fn eval_light(acc: ptr<function, Acc>, s: Surf, li: u32, wp: vec3<f32>, vis: f32) {
    let L = lights[li];
    if ((L.flags & 64u) != 0u || vis <= 0.0) { return; }
    let lv = L.pos - wp;
    let d2 = max(dot(lv, lv), 1e-4);
    let ld = lv * inverseSqrt(d2);
    var atten: f32;
    if ((L.flags & 8u) != 0u) {
        let x = d2 * L.rcp_r * L.rcp_r;
        atten = sq(max(1.0 - x * x, 0.0)) / (d2 + 1.0);
    } else {
        let x2 = min(L.rcp_r * L.rcp_r * max(d2, 1.0), 1.0);
        atten = clamp(1.0 + L.atten_lin * sqrt(x2) + L.atten_quad * x2, 0.0, 1.0);
    }
    if ((L.flags & 16u) != 0u) {
        let t = clamp(dot(L.spot_dir, ld) + L.spot_bias, 0.0, 1.0);
        atten *= t + L.spot_exp * (t * t - t);
    }
    let a = atten * vis * clamp(dot(s.n, ld), 0.0, 1.0);
    brdf(acc, s, ld, ld, a, L.color, L.spec_int);
}

// Apex height fog (apex-lighting.md section 7), k0..k4 = c_fogParams
fn fog(c: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let k0 = fr.fog_k[0]; let k1 = fr.fog_k[1]; let k2 = fr.fog_k[2]; let k3 = fr.fog_k[3]; let k4 = fr.fog_k[4];
    if (k0.w <= 0.0) { return c; }
    let wp = rel + fr.cam_pos.xyz;
    let dist = max(length(rel) + k1.w, 1e-5);
    let z_lo = min(wp.z, fr.cam_pos.z);
    let dz = max(max(wp.z, fr.cam_pos.z) - z_lo, 1e-5);
    let f = clamp((k4.xy - vec2<f32>(z_lo)) / dz, vec2<f32>(0.0), vec2<f32>(1.0));
    let a = f.x * dist;
    let b = dist * f.y - a;
    let cc = dist - dist * f.y;
    let dens0 = clamp((z_lo - k4.x) * k4.z, 0.0, 1.0) * k0.z + k0.x;
    let grad = clamp(dz * k4.z, 0.0, 1.0) * k0.z / dist;
    let od = (b * grad * 0.5 + dens0) * b + (cc * k0.y + k0.x * a);
    let amt = (1.0 - exp(-od)) * k0.w;
    let view = normalize(rel);
    let lobe = sq(clamp((dot(k3.xyz, view) - k2.w) * k3.w, 0.0, 1.0));
    return mix(c, k1.rgb + k2.rgb * lobe, amt);
}

// wrapped sky term from a visibility-scaled bent normal (section 2)
fn sky_term(n: vec3<f32>, sky_vec: vec3<f32>) -> f32 {
    let len2 = dot(sky_vec, sky_vec);
    let nds = dot(n, sky_vec);
    return clamp((max(len2, nds) + nds) * sqrt(len2) * 0.5, 0.0, 1.0);
}

@fragment
fn fs_main(i: VOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // ---- material (section 1). Layer 0 goes through c_uv1 (world-planar for terrain blends: (x, -y)),
    // layer 1 and the blend mask use the mesh UV (blend PS 0x15B60B41971F5517).
    let wp0 = i.rel + fr.cam_pos.xyz;
    let planar_base = vec2<f32>(wp0.x, -wp0.y);
    let base_uv = select(i.uv0, planar_base, mt.blend.z > 0.5);
    let uv_l0 = xform(base_uv, mt.uv1, mt.uv_t.xy);
    let uv_d = xform(base_uv, mt.uv2, mt.uv_t.zw);
    let base = textureSample(albedo_tex, mat_samp, uv_l0);
    var albedo = base.rgb * mt.color.rgb * mt.albedo_tint.rgb;
    var vtx = vec3<f32>(1.0);
    if (mt.params.w > 0.5) {
        vtx = pow(abs(i.col.rgb), vec3<f32>(2.2));
        albedo *= vtx;
    }
    var n2 = textureSample(normal_tex, mat_samp, uv_l0).rg * 2.867661 - 1.439453;
    let dn = textureSample(detail_n_tex, mat_samp, uv_d).rg * 2.867661 - 1.439453;
    if (mt.blend.w > 0.5) { n2 += dn; }
    var gloss = textureSample(gloss_tex, mat_samp, uv_l0).r;
    var f0 = textureSample(spec_tex, mat_samp, uv_l0).rgb;
    if (mt.blend.x > 0.5 && mt.perf_spec.w < 0.5) { f0 = mt.perf_spec.rgb; }
    let ao_s = textureSample(ao_tex, mat_samp, uv_l0).r;
    let emis_t = textureSample(emis_tex, mat_samp, i.uv0).rgb;
    let alb2 = textureSample(albedo2_tex, mat_samp, i.uv0).rgb;
    let n2b = textureSample(normal2_tex, mat_samp, i.uv0).rg * 2.867661 - 1.439453;
    let gloss2 = textureSample(gloss2_tex, mat_samp, i.uv0).r;
    let spec2 = textureSample(spec2_tex, mat_samp, i.uv0).rgb;
    let mask = textureSample(blend_tex, mat_samp, i.uv0).r;
    let mask_l0 = textureSample(blend_tex, mat_samp, uv_l0).r;
    let lm0 = textureSample(lm0_tex, lm_samp, i.uv1);
    let lm1 = textureSample(lm1_tex, lm_samp, i.uv1);
    let vis_a = textureSample(rtl_vis, lm_samp, vec2<f32>(i.uv1.x, i.uv1.y * 0.5)).rg;
    let vis_b = textureSample(rtl_vis, lm_samp, vec2<f32>(i.uv1.x, i.uv1.y * 0.5 + 0.5)).rg;

    var nv = normalize(i.nrm + vec3<f32>(0.0, 0.0, 1e-6));
    if (!front) { nv = -nv; }
    var n = tbn_normal(nv, i.rel, uv_l0, n2);
    let n_l1 = tbn_normal(nv, i.rel, i.uv0, n2b);
    if (mt.blend.x > 0.5 && mt.blend.x < 1.5) {
        // blend = saturate(ramp * (mask - 1) + vertexAlpha * (ramp + 1)); every layer property lerps
        let ramp = mt.blend.y;
        let bl = clamp(ramp * (mask - 1.0) + i.col.a * (ramp + 1.0), 0.0, 1.0);
        albedo = mix(albedo, alb2 * mt.albedo2_tint.rgb, bl);
        n = normalize(mix(n, n_l1, bl));
        let g2 = select(mt.layer2.w, gloss2, mt.layer2.x > 0.5);
        gloss = mix(gloss, g2, bl);
        f0 = mix(f0, select(mt.perf_spec2.rgb, spec2, mt.layer2.y > 0.5), bl);
    }

    let r = max(1.0 - gloss, 0.04);
    var s: Surf;
    s.n = n;
    s.v = -normalize(i.rel);
    s.f0 = f0;
    s.a = r * r;
    s.a2 = s.a * s.a;
    s.nov = clamp(dot(n, s.v), 0.0, 1.0);
    s.one_minus_lf0 = 1.0 - lum(f0);
    let wp = i.rel + fr.cam_pos.xyz;
    var acc: Acc;
    acc.diff = vec3<f32>(0.0);
    acc.multi = vec3<f32>(0.0);
    acc.spec = vec3<f32>(0.0);

    // ---- baked indirect + baked sun visibility
    var indirect = vec3<f32>(0.0);
    var sun_baked = 1.0;
    var env_ambient_src = fr.sky.rgb * 0.5;
    var env_sky_len = 0.0;
    let ndnv = clamp(dot(n, nv), 0.0, 1.0);
    if (mt.params.y > 0.5) {
        let baked = lm0.rgb * (lm0.a * 3.984375 + 0.015625);
        let sky_vec = lm1.rgb * 2.007874 - 1.0;
        indirect = baked * ndnv + sky_term(n, sky_vec) * fr.sky.rgb;
        sun_baked = lm1.a;
        env_ambient_src = baked;
        env_sky_len = length(sky_vec);
        // real-time lights from the lightmap's light-ID page, masked by their baked visibility
        let dims = vec2<f32>(textureDimensions(rtl_ids));
        let id = textureLoad(rtl_ids, vec2<i32>(min(i.uv1 * dims, dims - 1.0)), 0).r;
        let grp = id >> 24u;
        let vis4 = array<f32, 4>(vis_a.x, vis_a.y, vis_b.x, vis_b.y);
        for (var k = 0u; k < 4u; k++) {
            let slot = (id >> (6u * k)) & 63u;
            if (slot == 63u) { continue; }
            let li = rtl_index[grp * 63u + slot];
            eval_light(&acc, s, li, wp, vis4[k]);
        }
    } else if ((i.probe & 0xffffffu) != 0xffffffu) {
        let p = probes[i.probe & 0xffffffu];
        let sh = max(vec3<f32>(dot(n, p.sh[0].xyz) + p.sh[0].w, dot(n, p.sh[1].xyz) + p.sh[1].w, dot(n, p.sh[2].xyz) + p.sh[2].w), vec3<f32>(0.0));
        indirect = sh * ndnv + sky_term(n, p.sky.xyz) * fr.sky.rgb;
        sun_baked = p.sky.w;
        env_ambient_src = vec3<f32>(p.sh[0].w, p.sh[1].w, p.sh[2].w);
        env_sky_len = length(p.sky.xyz);
        for (var k = 0; k < 4; k++) {
            if (p.idx[k] >= 0 && p.wts[k] > 0.0) {
                eval_light(&acc, s, u32(p.idx[k]), wp, p.wts[k]);
            }
        }
    } else {
        indirect = fr.sky.rgb * 0.5;
    }

    // ---- sun: gated by baked visibility, cascades replace it near the camera
    var sun = sun_baked;
    if (sun > 0.0) {
        sun = sun_shadow(wp, nv, sun_baked);
        sun *= clamp(dot(n, fr.to_sun.xyz), 0.0, 1.0);
        brdf(&acc, s, fr.to_sun.xyz, fr.to_sun.xyz, sun, fr.sun_color.rgb, 1.0);
    }

    // ---- composite (section 7); no cubemap reflections yet, no fog (level params unknown)
    var ao = 1.0;
    if (mt.params2.x > 0.5) { ao = floor(ao_s * 255.0) / 255.0; }
    // ---- environment specular from the baked cubemap (section 5)
    let cube = i32(i.probe >> 24u);
    let rv = -s.v + 2.0 * s.nov * n;
    let r2 = r * r;
    let mip_a = 22.6 * r2 * r2 - 44.9 * r2 * r + 26.8 * r2 + 3.69 * r - 0.185;
    let mip_b = -11.0 * r2 * r2 + 23.5 * r2 * r - 11.4 * r2 + 3.55 * r - 0.0882;
    let mip = clamp(mip_a + sq(1.0 - s.nov) * (mip_b - mip_a), 0.0, 8.0);
    let env = textureSampleLevel(cubes, mat_samp, rv, cube, mip).rgb;
    let fe = f0 + (1.0 - f0) * pow5(1.0 - s.nov);
    let kr = 1.0 - 0.5 * clamp(2.0 * r - 1.0, 0.0, 1.0);
    let horizon = clamp(1.0 + 1.3 * dot(rv, nv), 0.0, 1.0);
    let occ = clamp((lum(env_ambient_src) + env_sky_len * lum(fr.sky.rgb)) * cube_ambient[cube], 0.0, 1.0) * horizon;
    let env_spec = env * fe * kr * occ * (4.0 * s.nov * s.nov) / (2.0 * max(2.0 * s.nov * mix(s.nov, 1.0, s.a), 0.00125));

    // emissive = emissiveTexture * c_L0_emissiveTint * vertex colour (world emissive PS, 0xBA8A6B19D28B0FA2)
    let emis = emis_t * mt.emissive_tint.rgb * vtx * mt.params2.y;
    var c = albedo * (acc.diff + albedo * acc.multi) + acc.spec + env_spec + albedo * indirect * ao + emis;
    c = fog(c, i.rel);

    let dbg = fr.misc.w;
    if (dbg > 0.5 && dbg < 1.5) { c = vec3<f32>(sun); }
    if (dbg > 1.5 && dbg < 2.5) { c = indirect; }
    if (dbg > 2.5 && dbg < 3.5) { c = albedo; }
    if (dbg > 3.5 && dbg < 4.5) { c = n * 0.5 + 0.5; }
    if (dbg > 4.5 && dbg < 5.5) { c = acc.diff + acc.spec; }
    if (dbg > 5.5 && dbg < 6.5) { c = env_spec; }
    if (dbg > 6.5 && dbg < 7.5) { c = env; }

    var a = base.a * mt.color.a;
    if (mt.blend.x > 1.5) {
        // transparent overlay (PS 0x937D8E4BC98DD7A9 / 0x954097DFD1711C57): opacity = (1 - blend) * albedo.a
        let ramp = mt.blend.y;
        a *= 1.0 - clamp(ramp * (mask_l0 - 1.0) + i.col.a * (ramp + 1.0), 0.0, 1.0);
    }
    if (mt.params.z > 0.5 && mt.params.z < 1.5) {
        // alpha-tested: sharpen around the cutoff so alpha-to-coverage gives crisp, AA'd edges
        a = (a - mt.params.x) / max(fwidth(a), 1e-4) + 0.5;
    } else if (mt.params.z < 0.5) {
        a = 1.0;
    }
    if (fr.misc.w > 7.5 && fr.misc.w < 8.5) { return vec4<f32>(vec3<f32>(a, i.col.a, mask_l0), 1.0); }
    return vec4<f32>(c, a);
}

// Sky-shaded surfaces (sky shader set 0xE273F3D2055B839F, PS 0x926A177354932726): unlit
//   colour = cube(viewDir) * albedoTint + emissiveTex * emissiveTint * lerp(inner, outer, pow(1 - |N.V|, exponent))
@fragment
fn fs_skymat(i: VOut) -> @location(0) vec4<f32> {
    let v = normalize(i.rel);
    var c = textureSample(sky_cube, mat_samp, v).rgb * mt.albedo_tint.rgb;
    let n = normalize(i.nrm + vec3<f32>(0.0, 0.0, 1e-6));
    let edge = max(mix(mt.emissive_edge.y, mt.emissive_edge.z, pow(max(1.0 - abs(dot(n, -v)), 0.0), mt.emissive_edge.x)), 0.0);
    c += textureSample(emis_tex, mat_samp, i.uv0).rgb * mt.emissive_tint.rgb * edge * mt.params2.y;
    return vec4<f32>(fog(c, i.rel), 1.0);
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
    // c_skyColor is the sky's irradiance; the visible dome is brighter towards the horizon
    let zenith = fr.sky.rgb * 0.9;
    let horizon = mix(fr.sky.rgb, vec3<f32>(1.0), 0.55) * 1.1;
    var c = mix(horizon, zenith, pow(max(up, 0.0), 0.5));
    if (up < 0.0) { c = horizon * 0.6; }
    let sd = max(dot(dir, fr.to_sun.xyz), 0.0);
    c += fr.sun_color.rgb * (pow(sd, 900.0) * 40.0 + pow(sd, 12.0) * 0.25);
    return vec4<f32>(c, 1.0);
}
