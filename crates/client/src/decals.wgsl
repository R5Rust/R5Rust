// Retail bullet-hole decals (decals.rs). One quad per impact on the hit surface; the decal multiplies the lit
// surface under it: retail decal colour is mod2x (128 grey leaves the surface alone), coverage = alpha. The decal's
// normal map shades it against the sun (relative to the flat surface), so the crater reads in 3D.
// Instances: (pos.xyz, size), (normal.xyz, alpha), (layer, angle, 0, 0).

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    to_sun: vec4<f32>,
};
@group(0) @binding(0) var<uniform> fr: Frame;
@group(1) @binding(0) var col_t: texture_2d_array<f32>;
@group(1) @binding(1) var nml_t: texture_2d_array<f32>;
@group(1) @binding(2) var samp: sampler;

struct DIn {
    @location(0) pos_size: vec4<f32>,
    @location(1) nrm_alpha: vec4<f32>,
    @location(2) extra: vec4<f32>,
};

struct DOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    @location(2) @interpolate(flat) layer: i32,
    @location(3) t: vec3<f32>,
    @location(4) b: vec3<f32>,
    @location(5) n: vec3<f32>,
};

@vertex
fn vs_decal(@builtin(vertex_index) vi: u32, i: DIn) -> DOut {
    let corners = array<vec2<f32>, 6>(vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    let c = corners[vi];
    let n = normalize(i.nrm_alpha.xyz);
    let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) > 0.9);
    let t0 = normalize(cross(helper, n));
    let b0 = cross(n, t0);
    // random spin per impact
    let ca = cos(i.extra.y);
    let sa = sin(i.extra.y);
    let t = t0 * ca + b0 * sa;
    let b = b0 * ca - t0 * sa;
    let r = i.pos_size.w * 0.5;
    let p = i.pos_size.xyz + n * 0.1 + (t * c.x + b * c.y) * r - fr.cam_pos.xyz;
    var o: DOut;
    o.clip = fr.view_proj * vec4<f32>(p, 1.0);
    o.uv = vec2<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5);
    o.alpha = i.nrm_alpha.w;
    o.layer = i32(i.extra.x);
    o.t = t;
    o.b = b;
    o.n = n;
    return o;
}

@fragment
fn fs_decal(i: DOut) -> @location(0) vec4<f32> {
    let col = textureSample(col_t, samp, i.uv, i.layer);
    let nm = textureSample(nml_t, samp, i.uv, i.layer).rg * 2.0 - 1.0;
    let nd = vec3<f32>(nm.x, -nm.y, sqrt(max(1.0 - dot(nm, nm), 0.0)));
    // sun in the decal's tangent frame, lifted towards the surface normal so a decal in shade still reads
    let l = fr.to_sun.xyz;
    let lt = normalize(vec3<f32>(dot(l, i.t), dot(l, i.b), dot(l, i.n)) + vec3<f32>(0.0, 0.0, 0.6));
    let shade = clamp((0.5 + 0.5 * dot(nd, lt)) / (0.5 + 0.5 * lt.z), 0.35, 1.8);
    // mod2x: linear 0.2158 = sRGB 128
    let f = clamp(col.rgb / 0.2158, vec3<f32>(0.0), vec3<f32>(3.0)) * shade;
    let a = clamp(col.a * i.alpha, 0.0, 1.0);
    if (a <= 0.002) { discard; }
    // multiply blend: dst * this
    return vec4<f32>(mix(vec3<f32>(1.0), f, a), 1.0);
}
