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
    quality: vec4<f32>,    // video settings: x shadow map res, y cascades (0 = baked only), z unused, w alpha test by discard (no MSAA)
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

// struct Mat, mats, texs, pc: mat.wgsl (prepended)

// texture slots, in loader.rs ROUTE_SLOTS order
const R_ALBEDO: u32 = 0u;
const R_NORMAL: u32 = 1u;
const R_GLOSS: u32 = 2u;
const R_SPEC: u32 = 3u;
const R_AO: u32 = 4u;
const R_ALBEDO2: u32 = 5u;
const R_NORMAL2: u32 = 6u;
const R_GLOSS2: u32 = 7u;
const R_SPEC2: u32 = 8u;
const R_BLEND: u32 = 9u;
const R_DETAIL_N: u32 = 10u;
const R_DETAIL: u32 = 11u;
const R_DETAIL2: u32 = 12u;
const R_EMIS_MULT: u32 = 13u;
const R_EMIS2: u32 = 14u;
const R_EMIS: u32 = 15u;

// The UV a texture slot is sampled with, as the material's own PS does it (apex-lighting.md section 1.1): the mesh UV
// or world (x, -y), optionally through one of the c_uv1..5 transforms.
fn route_uv(slot: u32, uv0: vec2<f32>, planar: vec2<f32>) -> vec2<f32> {
    let code = mats[pc.mat].route[slot / 4u][slot % 4u];
    let b = select(uv0, planar, (code & 8u) != 0u);
    let n = code & 7u;
    if (n == 0u) { return b; }
    let rs = mats[pc.mat].uv_rs[n - 1u];
    let tr = mats[pc.mat].uv_tr[n - 1u].xy;
    if (((u32(mats[pc.mat].fx2.x) >> (n - 1u)) & 1u) != 0u) {
        // retail rotation form (the uber PS takes sin / cos of c_uvNRotScaleX.x): b + translate * time - centre,
        // rotated, scaled by (RotScaleX.y, RotScaleY.y), + centre; centre = two f16 in RotScaleY.x (high = u, low = v)
        let c = unpack2x16float(bitcast<u32>(rs.z));
        let q = b + tr * fr.misc.y - vec2<f32>(c.y, c.x);
        let sn = sin(rs.x);
        let cs = cos(rs.x);
        return vec2<f32>(dot(q, vec2<f32>(cs, -sn)) * rs.y + c.y, dot(q, vec2<f32>(sn, cs)) * rs.w + c.x);
    }
    return xform(b, rs, tr);
}

// detail albedo overlay (blend PS 0x2FBB46BF988A98AC, 0x937D8E4BC98DD7A9): Photoshop overlay in gamma-2 space,
// detail 128/255 = neutral; the multiply branch is taken for all channels if any detail channel is < 0.5
fn detail_overlay(a: vec3<f32>, d_in: vec3<f32>) -> vec3<f32> {
    let d = clamp(d_in - vec3<f32>(0.001961), vec3<f32>(0.0), vec3<f32>(1.0));
    let s = sqrt(max(a, vec3<f32>(0.0)));
    let r = select(1.0 - 2.0 * (1.0 - d) * (1.0 - s), 2.0 * d * s, any(d < vec3<f32>(0.5)));
    return r * r;
}

// zUp rock PS (retail rgdp 0xD7462DFDD63CF478): layer 1 is mapped from world position on three planes, z-dominant
// (x, -y), y (x, z), x (y, z), each through the slot's c_uvN. Plane weights from the smooth normal where it agrees
// with the triangle's flat normal F = cross(ddx(wp), ddy(wp)) (oriented like N), else F itself:
// s = smoothstep(sat((dot(F, N) - 0.8) * 10)), n' = normalize(lerp(F, N, s)), w = (n'^2 / |n'^2|)^2 (sums to 1).
// Retail skips planes under a small weight; sampling all three gives the same blend.
fn tri_w(rel: vec3<f32>, nv: vec3<f32>) -> vec3<f32> {
    var f = normalize(cross(dpdx(rel), dpdy(rel)) + vec3<f32>(0.0, 0.0, 1e-12));
    if (dot(f, nv) < 0.0) { f = -f; }
    let s0 = clamp((dot(f, nv) - 0.8) * 10.0, 0.0, 1.0);
    let s = s0 * s0 * (3.0 - 2.0 * s0);
    let np = normalize(mix(f, nv, s));
    let w = normalize(np * np);
    return w * w;
}
fn tri_uv(slot: u32, p: vec2<f32>) -> vec2<f32> {
    var n = mats[pc.mat].route[slot / 4u][slot % 4u] & 7u;
    if (n == 0u) { n = 2u; }
    return xform(p, mats[pc.mat].uv_rs[n - 1u], mats[pc.mat].uv_tr[n - 1u].xy);
}
fn tri_sample(ti: u32, slot: u32, wp: vec3<f32>, w: vec3<f32>) -> vec4<f32> {
    return textureSample(texs[ti], mat_samp, tri_uv(slot, vec2<f32>(wp.x, -wp.y))) * w.z
        + textureSample(texs[ti], mat_samp, tri_uv(slot, vec2<f32>(wp.x, wp.z))) * w.y
        + textureSample(texs[ti], mat_samp, tri_uv(slot, vec2<f32>(wp.y, wp.z))) * w.x;
}
// the dominant plane's position (normal2 is sampled there only)
fn tri_dom(wp: vec3<f32>, w: vec3<f32>) -> vec2<f32> {
    if (w.z >= w.x && w.z >= w.y) { return vec2<f32>(wp.x, -wp.y); }
    if (w.y >= w.x) { return vec2<f32>(wp.x, wp.z); }
    return vec2<f32>(wp.y, wp.z);
}
// zUp blend factor: pre = sat((nz - maxCos) / (minCos - maxCos)) * vertexAlpha; bl = sat((mask + pre) * ramp + pre - ramp)
fn zup_bl(nz: f32, mask: f32) -> f32 {
    let z = mats[pc.mat].zup;
    let ramp = mats[pc.mat].blend.y;
    let pre = clamp((nz - z.y) / (z.x - z.y), 0.0, 1.0) * z.z;
    return clamp((mask + pre) * ramp + pre - ramp, 0.0, 1.0);
}

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

// heirloom PS (kunai 0x2D0879EE52FA8F3F, balisong 0x3D445FDD6444EA87) UV layer: c_uvNRotScaleX = (angle, scaleU),
// c_uvNRotScaleY = (pivot, scaleV) (pivot 0 on these), translate scrolls with c_gameTime, added before the rotation
fn vm_xf(uv: vec2<f32>, l: vec4<f32>, tr: vec2<f32>, t: f32) -> vec2<f32> {
    let p = uv + tr * t;
    let c = cos(l.x);
    let s = sin(l.x);
    return vec2<f32>(p.x * c - p.y * s, p.x * s + p.y * c) * l.yz;
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
    // static props fading out by distance (props.rs): 1 = solid
    @location(6) @interpolate(flat) fade: f32,
    // retail ModelInstance.diffuseModulation.rgb (the entity's rendercolor / 255): the uber PS multiplies it into the
    // emissive (body shields: the loot tier's FX colour, cl_survival_loot ApplyEquipmentColorAndFXOverrides)
    @location(7) @interpolate(flat) dmod: vec3<f32>,
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
    o.fade = 1.0;
    o.dmod = vec3<f32>(1.0);
    return o;
}

// Static props drawn instanced (props.rs): the mesh in model space, the instance's model -> world rows, its probe
// and fade from the instance stream.
struct PIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) col: vec4<f32>,
    @location(5) probe: u32,
    @location(6) r0: vec4<f32>,
    @location(7) r1: vec4<f32>,
    @location(8) r2: vec4<f32>,
    @location(9) fade: f32,
    @location(11) dmod: u32,
};

@vertex
fn vs_prop(v: PIn) -> VOut {
    var o: VOut;
    let p = vec4<f32>(v.pos, 1.0);
    let rel = vec3<f32>(dot(v.r0, p), dot(v.r1, p), dot(v.r2, p)) - fr.cam_pos.xyz;
    o.clip = fr.view_proj * vec4<f32>(rel, 1.0);
    o.rel = rel;
    // rotation x uniform scale: renormalise
    o.nrm = normalize(vec3<f32>(dot(v.r0.xyz, v.nrm.xyz), dot(v.r1.xyz, v.nrm.xyz), dot(v.r2.xyz, v.nrm.xyz)) + vec3<f32>(0.0, 0.0, 1e-9));
    o.uv0 = v.uv0;
    o.uv1 = v.uv1;
    o.col = v.col;
    o.probe = v.probe;
    o.fade = v.fade;
    o.dmod = select(unpack4x8unorm(v.dmod).rgb, vec3<f32>(1.0), v.dmod == 0u);
    return o;
}

// Loot highlight, pass 1 (highlight.rs): every highlighted entity into the id buffer (its slot / 255, retail
// idBuffer) and its own depth buffer (retail secondaryDepthBuffer, reversed-Z, nearest wins). Same transform as
// vs_prop so the depth matches the colour pass.
struct HlIn {
    @location(0) pos: vec3<f32>,
    @location(6) r0: vec4<f32>,
    @location(7) r1: vec4<f32>,
    @location(8) r2: vec4<f32>,
    @location(10) slot: u32,
};
struct HlOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) slot: u32,
};

@vertex
fn vs_loot_hl(v: HlIn) -> HlOut {
    var o: HlOut;
    let p = vec4<f32>(v.pos, 1.0);
    let rel = vec3<f32>(dot(v.r0, p), dot(v.r1, p), dot(v.r2, p)) - fr.cam_pos.xyz;
    o.clip = fr.view_proj * vec4<f32>(rel, 1.0);
    o.slot = v.slot;
    return o;
}

@fragment
fn fs_loot_hl(i: HlOut) -> @location(0) vec4<f32> {
    if (i.slot == 0u) { discard; }
    return vec4<f32>(f32(i.slot) / 255.0, 0.0, 0.0, 1.0);
}

fn sq(x: f32) -> f32 { return x * x; }
fn pow5(x: f32) -> f32 { let x2 = x * x; return x2 * x2 * x; }
fn lum(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

// ---------------------------------------------------------------- sun shadow (section 3)

fn pcf(i: i32, uv: vec2<f32>, z: f32) -> f32 {
    // 3x3 tent: corners 1/16, edges 1/8, centre 1/4
    var s = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let w = select(select(1.0 / 16.0, 1.0 / 8.0, x == 0 || y == 0), 0.25, x == 0 && y == 0);
            s += w * textureSampleCompareLevel(shadow_tex, shadow_samp, uv + vec2<f32>(f32(x), f32(y)) / fr.quality.x, i, z);
        }
    }
    return s;
}

// the cascade *replaces* the baked sun visibility inside its range, fading to the next one (or baked) at the edge
fn sun_shadow(wp: vec3<f32>, nv: vec3<f32>, baked: f32) -> f32 {
    let SHADOW_RES = fr.quality.x;
    let n = i32(fr.quality.y);
    for (var i = 0; i < n; i++) {
        let texel = 2.0 * fr.cascade_r[i] / SHADOW_RES;
        let c = fr.shadow_vp[i] * vec4<f32>(wp + nv * texel * 1.5, 1.0);
        let m = max(abs(c.x), abs(c.y));
        if (m < 0.98 && c.z < 1.0) {
            let cur = pcf(i, vec2<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5), c.z);
            let w = clamp((0.98 - m) / 0.15, 0.0, 1.0);
            var next = baked;
            if (w < 1.0 && i + 1 < n) {
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

// First-person weapon + arms (viewmodel.rs): skinned, up to 4 bones per vertex; the bone matrices map the rig's bind
// pose straight to camera-relative world space. Its own projection (vm.clip) and one light probe (the eye's).
struct VmBone {
    r0: vec4<f32>,
    r1: vec4<f32>,
    r2: vec4<f32>,
};
struct VmFrame {
    clip: mat4x4<f32>,
    probe: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};
@group(2) @binding(0) var<storage, read> vm_bones: array<VmBone>;
@group(2) @binding(1) var<uniform> vm: VmFrame;

struct VmIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) col: vec4<f32>,
    @location(10) bones: vec4<u32>,
    @location(11) wts: vec4<f32>,
};

@vertex
fn vs_viewmodel(v: VmIn) -> VOut {
    var r0 = vec4<f32>(0.0);
    var r1 = vec4<f32>(0.0);
    var r2 = vec4<f32>(0.0);
    for (var k = 0u; k < 4u; k++) {
        let w = v.wts[k];
        if (w > 0.0) {
            let b = vm_bones[v.bones[k]];
            r0 += b.r0 * w;
            r1 += b.r1 * w;
            r2 += b.r2 * w;
        }
    }
    let p = vec4<f32>(v.pos, 1.0);
    let rel = vec3<f32>(dot(r0, p), dot(r1, p), dot(r2, p));
    var o: VOut;
    o.clip = vm.clip * vec4<f32>(rel, 1.0);
    o.rel = rel;
    o.nrm = normalize(vec3<f32>(dot(r0.xyz, v.nrm.xyz), dot(r1.xyz, v.nrm.xyz), dot(r2.xyz, v.nrm.xyz)) + vec3<f32>(0.0, 0.0, 1e-9));
    o.uv0 = v.uv0;
    o.uv1 = v.uv1;
    o.col = vec4<f32>(1.0);
    o.probe = vm.probe;
    o.dmod = vec3<f32>(1.0);
    o.fade = 1.0;
    return o;
}
@fragment
fn fs_main(i: VOut) -> @location(0) vec4<f32> {
    return shade(i);
}

// 4x4 ordered dither: a prop in its fade band keeps `fade` of its pixels (Apex dithers props out the same way)
fn fade_kept(clip: vec4<f32>, fade: f32) -> bool {
    let b = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let p = vec2<u32>(clip.xy) & vec2<u32>(3u, 3u);
    return fade > (b[p.y * 4u + p.x] + 0.5) / 16.0;
}

@fragment
fn fs_prop_fade(i: VOut) -> @location(0) vec4<f32> {
    if (!fade_kept(i.clip, i.fade)) { discard; }
    return shade(i);
}

fn shade(i: VOut) -> vec4<f32> {
    // ---- material (section 1). Every texture uses the UV its PS routes it through (route_uv): terrain blends map
    // their layers from world position, other materials from the mesh UV.
    let wp0 = i.rel + fr.cam_pos.xyz;
    let pl = vec2<f32>(wp0.x, -wp0.y);
    let uv_alb = route_uv(R_ALBEDO, i.uv0, pl);
    let uv_nrm = route_uv(R_NORMAL, i.uv0, pl);
    let uv_nrm2 = route_uv(R_NORMAL2, i.uv0, pl);
    let base = textureSample(texs[mats[pc.mat].tix[0].x], mat_samp, uv_alb);
    var base_rgb = base.rgb;
    // the detail map is only applied where its formula was read from the shader: two-layer blends + layer-blend overlays
    // (mats[pc.mat] is uniform: an immediate index into read-only storage, so sampling in these branches stays in uniform control flow)
    if (mats[pc.mat].blend.x > 0.5 && mats[pc.mat].blend.x < 2.5 && mats[pc.mat].detail.x > 0.5) {
        base_rgb = detail_overlay(base_rgb, textureSample(texs[mats[pc.mat].tix[3].x], mat_samp, route_uv(R_DETAIL, i.uv0, pl)).rgb);
    }
    var albedo = base_rgb * mats[pc.mat].color.rgb * mats[pc.mat].albedo_tint.rgb;
    var vtx = vec3<f32>(1.0);
    if (mats[pc.mat].params.w > 0.5) {
        // the world VS (0x834C6E518B33674, every world material) outputs pow(|COLOR.rgb|, 2.2); the PS multiplies
        // albedo * c_L0_albedoTint by it (building_walls_metal_03 PS 0x355A436B0B1BB536)
        vtx = pow(abs(i.col.rgb), vec3<f32>(2.2));
        albedo *= vtx;
    }
    var n2 = textureSample(texs[mats[pc.mat].tix[0].y], mat_samp, uv_nrm).rg * 2.867661 - 1.439453;
    let dn = textureSample(texs[mats[pc.mat].tix[2].w], mat_samp, route_uv(R_DETAIL_N, i.uv0, pl)).rg * 2.867661 - 1.439453;
    if (mats[pc.mat].blend.w > 0.5) { n2 += dn; }
    var gloss = textureSample(texs[mats[pc.mat].tix[0].z], mat_samp, route_uv(R_GLOSS, i.uv0, pl)).r;
    var f0 = textureSample(texs[mats[pc.mat].tix[0].w], mat_samp, route_uv(R_SPEC, i.uv0, pl)).rgb;
    if (mats[pc.mat].blend.x > 0.5 && mats[pc.mat].perf_spec.w < 0.5) { f0 = mats[pc.mat].perf_spec.rgb; }
    // PS without a gloss / spec texture: the material's perf constants (loader fx2.zw)
    if (mats[pc.mat].fx2.z > 0.5) { gloss = mats[pc.mat].layer2.z; }
    if (mats[pc.mat].fx2.w > 0.5) { f0 = mats[pc.mat].perf_spec.rgb; }
    let ao_s = textureSample(texs[mats[pc.mat].tix[1].x], mat_samp, route_uv(R_AO, i.uv0, pl)).r;
    let emis_t = textureSample(texs[mats[pc.mat].tix[1].y], mat_samp, route_uv(R_EMIS, i.uv0, pl)).rgb;
    let emis_m = textureSample(texs[mats[pc.mat].tix[3].z], mat_samp, route_uv(R_EMIS_MULT, i.uv0, pl)).rgb;
    let emis2_t = textureSample(texs[mats[pc.mat].tix[3].w], mat_samp, route_uv(R_EMIS2, i.uv0, pl)).rgb;
    var layer_bl = 0.0;
    var alb2 = textureSample(texs[mats[pc.mat].tix[1].z], mat_samp, route_uv(R_ALBEDO2, i.uv0, pl)).rgb;
    if (mats[pc.mat].detail.y > 0.5) {
        let det2 = textureSample(texs[mats[pc.mat].tix[3].y], mat_samp, route_uv(R_DETAIL2, i.uv0, pl)).rgb;
        alb2 = select(alb2 * clamp(det2 - vec3<f32>(0.001961), vec3<f32>(0.0), vec3<f32>(1.0)) * 2.0, detail_overlay(alb2, det2), mats[pc.mat].detail.z > 0.5);
    }
    var n2b = textureSample(texs[mats[pc.mat].tix[1].w], mat_samp, uv_nrm2).rg * 2.867661 - 1.439453;
    var gloss2 = textureSample(texs[mats[pc.mat].tix[2].x], mat_samp, route_uv(R_GLOSS2, i.uv0, pl)).r;
    var spec2 = textureSample(texs[mats[pc.mat].tix[2].y], mat_samp, route_uv(R_SPEC2, i.uv0, pl)).rgb;
    let mask = textureSample(texs[mats[pc.mat].tix[2].z], mat_samp, route_uv(R_BLEND, i.uv0, pl)).r;
    let lm0 = textureSample(lm0_tex, lm_samp, i.uv1);
    let lm1 = textureSample(lm1_tex, lm_samp, i.uv1);
    let vis_a = textureSample(rtl_vis, lm_samp, vec2<f32>(i.uv1.x, i.uv1.y * 0.5)).rg;
    let vis_b = textureSample(rtl_vis, lm_samp, vec2<f32>(i.uv1.x, i.uv1.y * 0.5 + 0.5)).rg;

    // No back-face flip: neither the world (wldc) nor the prop (rgdp) PS reads SV_IsFrontFace, so a double-sided
    // surface is lit with its vertex normal on both sides. Foliage relies on it (grass normals point up; flipped,
    // the back of every blade faced the ground and went black under its dim per-prop probe).
    let nv = normalize(i.nrm + vec3<f32>(0.0, 0.0, 1e-6));
    var n = tbn_normal(nv, i.rel, uv_nrm, n2);
    var uv_n2 = uv_nrm2;
    if (mats[pc.mat].zup.w > 0.5) {
        // zUp rock blend: every layer-1 texture triplanar from world position (normal2 on the dominant plane only)
        let w = tri_w(i.rel, nv);
        alb2 = tri_sample(mats[pc.mat].tix[1].z, R_ALBEDO2, wp0, w).rgb;
        if (mats[pc.mat].detail.y > 0.5) {
            let det2 = tri_sample(mats[pc.mat].tix[3].y, R_DETAIL2, wp0, w).rgb;
            alb2 = select(alb2 * clamp(det2 - vec3<f32>(0.001961), vec3<f32>(0.0), vec3<f32>(1.0)) * 2.0, detail_overlay(alb2, det2), mats[pc.mat].detail.z > 0.5);
        }
        gloss2 = tri_sample(mats[pc.mat].tix[2].x, R_GLOSS2, wp0, w).r;
        spec2 = tri_sample(mats[pc.mat].tix[2].y, R_SPEC2, wp0, w).rgb;
        uv_n2 = tri_uv(R_NORMAL2, tri_dom(wp0, w));
        n2b = textureSample(texs[mats[pc.mat].tix[1].w], mat_samp, uv_n2).rg * 2.867661 - 1.439453;
    }
    let n_l1 = tbn_normal(nv, i.rel, uv_n2, n2b);
    if (mats[pc.mat].blend.x > 0.5 && mats[pc.mat].blend.x < 1.5) {
        // blend = saturate(ramp * (mask - 1) + vertexAlpha * (ramp + 1)); every layer property lerps
        let ramp = mats[pc.mat].blend.y;
        var bl = clamp(ramp * (mask - 1.0) + i.col.a * (ramp + 1.0), 0.0, 1.0);
        if (mats[pc.mat].zup.w > 0.5) {
            // zUp: by up-facing, not vertex alpha. Retail evaluates it on the vertex normal first, then again on
            // the z of the normal-mapped layers lerped by that first blend (the one it shades with)
            let bl0 = zup_bl(nv.z, mask);
            bl = zup_bl(mix(n, n_l1, bl0).z, mask);
        }
        layer_bl = bl;
        // retail wldc PS (0x933050CFFFA99C41): lerp(albedo * L0 tint, albedo2 * L1 tint, bl) * TEXCOORD3.rgb, the vertex
        // colour multiplies the blended result, so layer 1 takes it too (it used to stay untinted: every *_bm_vc wall
        // that blends to layer 1 rendered near white)
        albedo = mix(albedo, alb2 * mats[pc.mat].albedo2_tint.rgb * vtx, bl);
        n = normalize(mix(n, n_l1, bl));
        let g2 = select(mats[pc.mat].layer2.w, gloss2, mats[pc.mat].layer2.x > 0.5);
        gloss = mix(gloss, g2, bl);
        f0 = mix(f0, select(mats[pc.mat].perf_spec2.rgb, spec2, mats[pc.mat].layer2.y > 0.5), bl);
    }

    if (mats[pc.mat].blend.x > 1.5 && mats[pc.mat].blend.x < 2.5) {
        // transparent overlay (PS 0x937D8E4BC98DD7A9): roughness = lerp(1 - gloss, 1, blend), rough where it fades out
        gloss *= 1.0 - clamp(mats[pc.mat].blend.y * (mask - 1.0) + i.col.a * (mats[pc.mat].blend.y + 1.0), 0.0, 1.0);
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
    if (mats[pc.mat].params.y > 0.5) {
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
    // experimental ray-traced sun (rt.rs, quality.z = 1): replaces the baked visibility and the cascades
    if (fr.quality.z > 0.5) {
        sun = rt_sun(wp, nv);
    }
    if (sun > 0.0) {
        if (fr.quality.z < 0.5) {
            sun = sun_shadow(wp, nv, sun_baked);
        }
        sun *= clamp(dot(n, fr.to_sun.xyz), 0.0, 1.0);
        brdf(&acc, s, fr.to_sun.xyz, fr.to_sun.xyz, sun, fr.sun_color.rgb, 1.0);
    }

    // ---- composite (section 7); no cubemap reflections yet, no fog (level params unknown)
    var ao = 1.0;
    if (mats[pc.mat].params2.x > 0.5) { ao = floor(ao_s * 255.0) / 255.0; }
    // foliage PS (0x8BFA55EB7A9AA9D): AO byte = u8(pow(|COLOR.r|, 2.2) * 255 * aoTexture.r)
    if (mats[pc.mat].params2.w > 0.5) { ao = floor(pow(abs(i.col.r), 2.2) * 255.0 * select(1.0, ao_s, mats[pc.mat].params2.x > 0.5)) / 255.0; }
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
    let spec_direct = acc.spec;

    // emissive = emissiveTexture * c_L0_emissiveTint * vertex colour (world emissive PS, 0xBA8A6B19D28B0FA2)
    var emis = emis_t * mats[pc.mat].emissive_tint.rgb * vtx * mats[pc.mat].params2.y * i.dmod;
    // retail uber PS: * emissiveMultiplyTexture (the Nemesis' scrolling energy mask); two-layer blends lerp to the
    // layer-1 emissive (emissive2Texture * c_L1_emissiveTint) by the layer blend
    if (mats[pc.mat].fx2.y > 0.5) { emis *= emis_m; }
    if (mats[pc.mat].blend.x > 0.5 && mats[pc.mat].blend.x < 1.5 && mats[pc.mat].emis2.w > 0.5) { emis = mix(emis, emis2_t * mats[pc.mat].emis2.rgb, layer_bl); }
    // heirlooms: * emissiveMultiplyTexture on uv1, scrolled and pushed around by the uvDistortionTexture (uv2)
    if (mats[pc.mat].vmfx.x > 0.5 && mats[pc.mat].vmfx.x < 1.5) {
        let tm = fr.misc.y;
        let uv1 = vm_xf(i.uv0, mats[pc.mat].vm_uv1, mats[pc.mat].vm_tr.xy, tm);
        let uv2 = vm_xf(i.uv0, mats[pc.mat].vm_uv2, mats[pc.mat].vm_tr.zw, tm);
        let dn = textureSample(texs[mats[pc.mat].tix[2].w], mat_samp, uv2).rg * 2.0 - 1.0;
        let uv1d = uv1 + dn * mats[pc.mat].vmfx.zw * cos(mats[pc.mat].vm_uv1.x) * mats[pc.mat].vm_uv1.yz;
        emis *= textureSample(texs[mats[pc.mat].tix[1].z], mat_samp, uv1d).rgb;
    }
    if (mats[pc.mat].vmfx.x > 1.5) {
        // heirloom glass (PS 0x18E3336CD5934DE2): alpha = albedo.a * opacityMultiplyTexture.r * c_opacity; output
        // premultiplied: the diffuse fades with alpha, the reflections stay (spec / alpha * alpha)
        let ga = clamp(base.a * mask * mats[pc.mat].vmfx.y, 0.0, 1.0);
        let diffuse = albedo * (acc.diff + albedo * acc.multi) + albedo * indirect * ao;
        return vec4<f32>(diffuse * ga + spec_direct + env_spec, ga);
    }
    var c = albedo * (acc.diff + albedo * acc.multi) + spec_direct + env_spec + albedo * indirect * ao + emis;
    // unlit materials (params2.z: the 3D skybox's HDR sky dome): the PS outputs albedo * c_L0_albedoTint, then fog
    if (mats[pc.mat].params2.z > 0.5) {
        c = albedo;
    }
    c = fog(c, i.rel);
    // the sky dome stands in for the sky: the sun shows on it (sun_disc)
    if (mats[pc.mat].params2.z > 0.5) { c += sun_disc(normalize(i.rel)); }

    let dbg = fr.misc.w;
    if (dbg > 0.5 && dbg < 1.5) { c = vec3<f32>(sun); }
    if (dbg > 9.5 && dbg < 10.5) { c = vec3<f32>(sun_baked); }
    if (dbg > 1.5 && dbg < 2.5) { c = indirect; }
    // 13 = the sky term alone, 14 = lightmap 1 raw (sky direction rgb, sun visibility a), 15 = lightmap 0 decoded
    if (dbg > 12.5 && dbg < 13.5) { c = sky_term(n, lm1.rgb * 2.007874 - 1.0) * fr.sky.rgb; }
    if (dbg > 13.5 && dbg < 14.5) { c = lm1.rgb; }
    if (dbg > 14.5 && dbg < 15.5) { c = lm0.rgb * (lm0.a * 3.984375 + 0.015625); }
    if (dbg > 2.5 && dbg < 3.5) { c = albedo; }
    if (dbg > 3.5 && dbg < 4.5) { c = n * 0.5 + 0.5; }
    if (dbg > 4.5 && dbg < 5.5) { c = acc.diff + acc.spec; }
    if (dbg > 5.5 && dbg < 6.5) { c = env_spec; }
    if (dbg > 6.5 && dbg < 7.5) { c = env; }

    var a = base.a * mats[pc.mat].color.a;
    if (mats[pc.mat].opac.w > 0.5) {
        // retail uber PS alpha: albedo.a * vertex alpha * opacityMultiplyTexture.r * c_opacity
        // * sat(dist * c_alphaDistanceFadeScale + Bias) * max(0, (1 - |N.V|)^exp * (outer - inner) + inner)
        let o = mats[pc.mat].opac;
        let oe = mats[pc.mat].opac_edge;
        let ndv = abs(dot(normalize(i.rel), nv));
        let edge = max(0.0, pow(clamp(1.0 - ndv, 0.0, 1.0), oe.x) * (oe.z - oe.y) + oe.y);
        let distf = clamp(length(i.rel) * o.y + o.z, 0.0, 1.0);
        a = a * i.col.a * mask * o.x * distf * edge;
    }
    if (mats[pc.mat].blend.x > 1.5) {
        let ramp = mats[pc.mat].blend.y;
        let bl = clamp(ramp * (mask - 1.0) + i.col.a * (ramp + 1.0), 0.0, 1.0);
        if (mats[pc.mat].blend.x < 2.5) {
            // transparent layer-blend overlay (PS 0x937D8E4BC98DD7A9 / 0x954097DFD1711C57): opacity = (1 - blend) * albedo.a.
            // Also the old-style _col/_nml/_gls/_det/_bm road tracks + erosion sand (same slot layout, PS not extracted).
            a *= 1.0 - bl;
        } else {
            // old-style overlays whose mask slot is a guess (sand_blend_firing_range, decals): opacity = blend, the
            // closer of the two rules on the map00 reference. Unverified against a shader.
            a *= bl;
        }
    }
    if (mats[pc.mat].params.z > 0.5 && mats[pc.mat].params.z < 1.5) {
        // alpha-tested: sharpen around the cutoff so alpha-to-coverage gives crisp, AA'd edges
        a = (a - mats[pc.mat].params.x) / max(fwidth(a), 1e-4) + 0.5;
        // no MSAA = no alpha-to-coverage: a plain cut-out
        if (fr.quality.w > 0.5) {
            if (a < 0.5) { discard; }
            a = 1.0;
        }
    } else if (mats[pc.mat].params.z < 0.5) {
        a = 1.0;
    }
    if (fr.misc.w > 7.5 && fr.misc.w < 8.5) { return vec4<f32>(vec3<f32>(a, i.col.a, mask), 1.0); }
    if (fr.misc.w > 8.5 && fr.misc.w < 9.5) { return vec4<f32>(i.col.r, i.col.a, mats[pc.mat].blend.x * 0.5, 1.0); }
    if (fr.misc.w > 10.5 && fr.misc.w < 12.5) {
        // z-fighting scan (tools/zscan.py), written so the 8-bit sRGB screenshot holds these exact bytes:
        // 11 = a flat colour per material, 12 = view distance in 1/64 units as a 24-bit number.
        // Blended surfaces count where they are > half opaque.
        if (mats[pc.mat].params.z > 1.5 && a < 0.5) { discard; }
        var h = hash_u32(u32(mats[pc.mat].detail.w) + 1u);
        if (fr.misc.w > 11.5) { h = u32(clamp(length(i.rel) * 64.0, 0.0, 16777215.0)); }
        let b = vec3<f32>(f32(h & 255u), f32((h >> 8u) & 255u), f32((h >> 16u) & 255u)) / 255.0;
        let lin = select(pow((b + 0.055) / 1.055, vec3<f32>(2.4)), b / 12.92, b <= vec3<f32>(0.04045));
        return vec4<f32>(lin, select(1.0, a, mats[pc.mat].params.z > 0.5 && mats[pc.mat].params.z < 1.5));
    }
    return vec4<f32>(c, a);
}

fn hash_u32(x0: u32) -> u32 {
    var x = x0;
    x = ((x >> 16u) ^ x) * 0x45d9f3bu;
    x = ((x >> 16u) ^ x) * 0x45d9f3bu;
    return (x >> 16u) ^ x;
}

// The sun as seen in the sky: a hot disc plus a soft halo along the light_environment direction (the bloom
// turns it into retail's glare). Added on everything that shows sky: the far-plane sky, the 3D skybox's unlit
// dome and the sky-material glass, which used to cover the far-plane sun everywhere.
fn sun_disc(dir: vec3<f32>) -> vec3<f32> {
    let sd = max(dot(dir, fr.to_sun.xyz), 0.0);
    return fr.sun_color.rgb * (pow(sd, 900.0) * 40.0 + pow(sd, 12.0) * 0.25);
}

// Sky-shaded surfaces (sky shader set 0xE273F3D2055B839F, PS 0x926A177354932726): unlit
//   colour = cube(viewDir) * albedoTint + emissiveTex * emissiveTint * lerp(inner, outer, pow(1 - |N.V|, exponent))
@fragment
fn fs_skymat(i: VOut) -> @location(0) vec4<f32> {
    let v = normalize(i.rel);
    var c = textureSample(sky_cube, mat_samp, v).rgb * mats[pc.mat].albedo_tint.rgb;
    let n = normalize(i.nrm + vec3<f32>(0.0, 0.0, 1e-6));
    let edge = max(mix(mats[pc.mat].emissive_edge.y, mats[pc.mat].emissive_edge.z, pow(max(1.0 - abs(dot(n, -v)), 0.0), mats[pc.mat].emissive_edge.x)), 0.0);
    c += textureSample(texs[mats[pc.mat].tix[1].y], mat_samp, i.uv0).rgb * mats[pc.mat].emissive_tint.rgb * edge * mats[pc.mat].params2.y;
    return vec4<f32>(fog(c, i.rel) + sun_disc(v), 1.0);
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
    c += sun_disc(dir);
    return vec4<f32>(c, 1.0);
}
