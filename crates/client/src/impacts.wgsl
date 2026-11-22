// Bullet impact marks (weapon.rs Impact): one small dark disc per hit, on the hit surface, fading with age.
// Uses the world's frame uniform (group 0); instances = (pos.xyz, size, normal.xyz, alpha).

struct Frame {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
};
@group(0) @binding(0) var<uniform> fr: Frame;

struct IIn {
    @location(0) pos_size: vec4<f32>,
    @location(1) nrm_alpha: vec4<f32>,
};

struct IOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
};

@vertex
fn vs_impact(@builtin(vertex_index) vi: u32, i: IIn) -> IOut {
    let corners = array<vec2<f32>, 6>(vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    let c = corners[vi];
    let n = normalize(i.nrm_alpha.xyz);
    let helper = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.z) > 0.9);
    let t = normalize(cross(helper, n));
    let b = cross(n, t);
    let p = i.pos_size.xyz + n * 0.15 + (t * c.x + b * c.y) * i.pos_size.w - fr.cam_pos.xyz;
    var o: IOut;
    o.clip = fr.view_proj * vec4<f32>(p, 1.0);
    o.uv = c;
    o.alpha = i.nrm_alpha.w;
    return o;
}

@fragment
fn fs_impact(i: IOut) -> @location(0) vec4<f32> {
    let r = length(i.uv);
    // a dark core with a soft scorched rim
    let a = (1.0 - smoothstep(0.35, 1.0, r)) * i.alpha;
    if (a <= 0.003) { discard; }
    return vec4<f32>(vec3<f32>(0.015, 0.013, 0.012), a * 0.9);
}
