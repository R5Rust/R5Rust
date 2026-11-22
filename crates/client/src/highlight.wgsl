// Retail Apex's object highlight (S30 shaders, ported line by line; docs/highlight.md):
//   1. ids:        world.wgsl vs_loot_hl / fs_loot_hl -> idBuffer (slot / 255) + the highlight objects' own depth
//   2. fs_gather_h 0x49D049DD3D89373C_N  -> gather_h  (signed x offset to the nearest nearer owner, enc(v))
//   3. fs_gather_v 0xB323BB047AA8D57D_N  -> silhouette (signed (dx, dy) to the owning object pixel)
//   4. fs_apply    0x2B0CC0E11A850248 composite (inside / outline functions 135-138) packed into retail's RGBA8
//                  colorBuffer encoding, then 0x98A084626866C0FD_0 apply (decode, / exposure), blended
//                  ONE, ONE_MINUS_SRC_ALPHA into the HDR scene before bloom.
// Reversed-Z everywhere (bigger depth = nearer, 0 = cleared). enc(v) = v / 255 + 128 / 255; 127 = no candidate.

struct U {
    size: vec4<f32>,   // render width, height, 2 / width, 2 / height (c_invRenderTargetHalfSize)
    cam: vec4<f32>,    // near plane, 1 / proj[0][0], 1 / proj[1][1], time (s)
    misc: vec4<f32>,   // manual exposure, highlight setting count, gather taps N, 0
};

// SBufHighlightSettings (stride 48), indexed by the per-pixel slot id
struct Setting {
    p0: vec4<f32>,
    p1: vec4<f32>,
    packed: u32,       // bits 0-7 flags, 8-19 inside function, 20-31 outline function
    inside_op: f32,
    outline_op: f32,
    radius: f32,
};

struct Expo {
    exposure: f32,
    sum: i32,
    count: u32,
    avg_log: f32,
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var id_tex: texture_2d<f32>;
@group(0) @binding(2) var hl_depth: texture_depth_2d;
@group(0) @binding(3) var gather_h_tex: texture_2d<f32>;
@group(0) @binding(4) var sil_tex: texture_2d<f32>;
@group(0) @binding(5) var<storage, read> settings: array<Setting>;
@group(0) @binding(6) var primary_depth: PRIMARY_DEPTH_TYPE;
@group(0) @binding(7) var<storage, read> expo: Expo;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn enc(v: i32) -> f32 { return f32(v) * 0.003921568859 + 0.501960814; }
fn dec(e: f32) -> i32 { return i32(u32(e * 255.0 + 0.5)) - 128; }
fn id_at(p: vec2<i32>) -> u32 { return u32(textureLoad(id_tex, p, 0).x * 255.0 + 0.5); }

fn dims() -> vec2<i32> { return vec2<i32>(textureDimensions(id_tex)); }
fn inside(p: vec2<i32>) -> bool { let d = dims(); return p.x >= 0 && p.y >= 0 && p.x < d.x && p.y < d.y; }
// D3D Load out of bounds returns 0; WGSL doesn't promise that
fn hdepth(p: vec2<i32>) -> f32 { if (!inside(p)) { return 0.0; } return textureLoad(hl_depth, p, 0); }
fn radius_of(id: u32) -> f32 { return settings[min(id, arrayLength(&settings) - 1u)].radius; }

// ---- 0x49D049DD3D89373C_N: horizontal gather
@fragment
fn fs_gather_h(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let n = i32(u.misc.z);
    let cid = id_at(p);
    let cdepth = hdepth(p);
    var result = select(0, 127, cid == 0u);
    var best = 127;
    var best_depth = cdepth;
    for (var j = 0; j < n; j++) {
        let x = p.x - (j + 1);
        if (x < 0) { break; }
        let q = vec2<i32>(x, p.y);
        let nd = hdepth(q);
        if (nd == 0.0 || nd == 1.0) { continue; }
        let nid = id_at(q);
        if (nid == cid) { continue; }
        let rad = i32(radius_of(nid));
        if ((nd > best_depth || nid == 0u) && min(best, rad) > j) {
            best_depth = max(best_depth, nd);
            best = j;
            result = -(j + 1);
        }
    }
    for (var j = 0; j < n; j++) {
        let x = p.x + (j + 1);
        if (x >= i32(u.size.x)) { break; }
        let q = vec2<i32>(x, p.y);
        let nd = hdepth(q);
        if (nd == 0.0 || nd == 1.0) { continue; }
        let nid = id_at(q);
        if (nid == cid) { continue; }
        let rad = i32(radius_of(nid));
        if ((nd > best_depth || nid == 0u) && min(best, rad) > j) {
            best_depth = max(best_depth, nd);
            best = j;
            result = j + 1;
        }
    }
    return vec4<f32>(enc(result), 0.0, 0.0, 0.0);
}

// ---- 0xB323BB047AA8D57D_N: vertical gather
struct VState {
    res: vec2<i32>,
    best_d2: f32,
    best_depth: f32,
    best_id: u32,
};

fn try_row(p: vec2<i32>, cid: u32, dy: i32, s: ptr<function, VState>) {
    let y = p.y + dy;
    let h = dec(textureLoad(gather_h_tex, vec2<i32>(p.x, y), 0).x);
    if (h == 127) { return; }
    let q = vec2<i32>(p.x + h, y);
    let nd = hdepth(q);
    let nid = id_at(q);
    let d2 = f32(h * h + dy * dy);
    let r = radius_of(nid);
    if (nid != cid && (nd > (*s).best_depth || nid == (*s).best_id) && d2 <= min((*s).best_d2, r * r)) {
        (*s).best_depth = max((*s).best_depth, nd);
        (*s).best_d2 = d2 + (-0.0010000000475);
        (*s).res = vec2<i32>(h, dy);
        (*s).best_id = nid;
    }
}

@fragment
fn fs_gather_v(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(pos.xy);
    let n = i32(u.misc.z);
    let cid = id_at(p);
    let init = select(0, 127, cid == 0u);
    var s = VState(vec2<i32>(init, init), 16129.0, hdepth(p), 0u);
    if (p.y >= 0) { try_row(p, cid, 0, &s); }
    for (var k = 1; k <= n; k++) {
        if (p.y - k < 0) { break; }
        try_row(p, cid, -k, &s);
    }
    for (var k = 1; k <= n; k++) {
        if (p.y + k >= i32(u.size.y)) { break; }
        try_row(p, cid, k, &s);
    }
    return vec4<f32>(enc(s.res.x), enc(s.res.y), 0.0, 0.0);
}

// ---- 0x2B0CC0E11A850248: composite (loot functions 135-138)
fn composite(fpos: vec2<f32>) -> vec4<f32> {
    let p = vec2<i32>(fpos);
    let sv = textureLoad(sil_tex, p, 0).xy;
    let dx = dec(sv.x);
    let dy = dec(sv.y);
    if (dx + dy == 254) { return vec4<f32>(0.0); }
    let src = p + vec2<i32>(dx, dy);
    let id = id_at(src);
    if (id > u32(u.misc.y)) { return vec4<f32>(0.0); }
    let st = settings[min(id, arrayLength(&settings) - 1u)];

    let near = u.cam.x;
    let z_scene = near / max(PRIMARY_DEPTH_LOAD(src), 1e-30);
    let d0 = hdepth(src);
    var dmax = d0;
    let dl = hdepth(src + vec2<i32>(-1, 0));
    let dr = hdepth(src + vec2<i32>(1, 0));
    let du = hdepth(src + vec2<i32>(0, -1));
    let dd = hdepth(src + vec2<i32>(0, 1));
    if (dl > 0.0) { dmax = max(dmax, dl); }
    if (dr > 0.0) { dmax = max(dmax, dr); }
    if (du > 0.0) { dmax = max(dmax, du); }
    if (dd > 0.0) { dmax = max(dmax, dd); }
    let z_obj = near / max(dmax, 1e-30);

    let p0 = st.p0.xyz;
    let p1x = st.p1.x;
    let ndc_x = fpos.x * u.size.z - 1.0;
    let ndc_y = 1.0 - fpos.y * u.size.w;
    // |rot(vScene - vObj)|^2 + dz^2 (retail adds dz on top of the rotated difference; a rotation keeps the length)
    let dz = z_scene - z_obj;
    let a = u.cam.y * ndc_x * dz;
    let b = u.cam.z * ndc_y * dz;
    let len = sqrt(a * a + b * b + dz * dz + dz * dz);

    var c = vec4<f32>(0.0);
    if (dx == 0 && dy == 0) {
        let f = (st.packed >> 8u) & 0xFFFu;
        let not_occl = select(1.0, 0.0, dz < -1.0);
        if (f == 135u) {
            let fade = saturate((z_obj - 64.0) * 0.004854369);
            let v = saturate(fade * not_occl * len);
            c = vec4<f32>(p1x * p0 * v, v * p1x);
        } else if (f == 136u) {
            let fade = saturate(z_obj - 1.0);
            let v = saturate(fade * not_occl * len);
            let pulse = sin(z_obj * 0.150000006 + u.cam.w * 5.0);
            let k = vec3<f32>(0.071775004, 0.012675000, 0.004425000);
            c = vec4<f32>((p0 + k + pulse * k) * v, v * 0.949999988);
        } else if (f == 137u) {
            let fade = saturate((z_obj - 64.0) * 0.004854369);
            let v = saturate(fade * not_occl * len);
            c = vec4<f32>(p1x * p0 * v, v * p1x);
            if (z_obj > z_scene) { c = vec4<f32>(p0 * 0.400000006, 0.400000006); }
        } else if (f != 0u) {
            c = vec4<f32>(1.0, 0.4, 0.5, 1.0);
        }
        c = c * st.inside_op;
    } else {
        let f = st.packed >> 20u;
        if (f == 135u) {
            let fade = 1.0 - saturate((z_obj - 128.0) * 0.0070422534);
            let vis = select(1.0, 0.0, (z_scene - z_obj) < -1.0);
            c = vec4<f32>(saturate(fade * vis) * p0, 0.0);
        } else if (f == 136u) {
            let vis = select(0.0, 1.0, (z_obj - z_scene) < 1.0);
            c = vec4<f32>(p0 * vis, 0.0);
        } else if (f == 138u) {
            let fade = 1.0 - saturate((z_obj - 128.0) * 0.0070422534);
            let vis = select(1.0, 0.0, (z_scene - z_obj) < -1.0);
            c = vec4<f32>(saturate(fade * vis) * p0, 0.0);
            if (z_obj > z_scene) { c = vec4<f32>(0.449999988 * (p0 * 0.800000012 + 0.200000003), 0.300000012); }
        } else if (f != 0u) {
            c = vec4<f32>(1.0, 0.4, 0.5, 1.0);
        }
        c = c * st.outline_op;
    }
    return c;
}

// ---- composite's RGBA8 encode, then 0x98A084626866C0FD_0 (the unflagged, exposure-compensated apply)
@fragment
fn fs_apply(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = composite(pos.xy);
    let flag = 0u;
    let a8 = u32(saturate(c.a) * 127.0) + (flag << 7u);
    let e = saturate(c.rgb) * 0.49803922 + saturate((c.rgb - 1.0) * 0.002004008) * 0.50196081;
    let t = round(e * 255.0) / 255.0;    // the RGBA8 colorBuffer
    if ((a8 & 128u) != 0u) { return vec4<f32>(0.0); }
    let alpha = f32(a8 & 127u) * 0.007874016;
    let rgb = saturate(t * 1.9921875) + saturate(t * 2.007874 - 1.0) * 499.0;
    let exposure = u.misc.x * max(expo.exposure, 1e-3);
    return vec4<f32>(rgb / exposure, alpha);
}
