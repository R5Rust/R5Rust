// Weapon effects (fx.rs): camera-facing / axis-stretched sprites from the retail particle textures (a 2D array,
// tools/build_fx.py), and the ejected shells (mesh, instanced). Positions arrive camera-relative; `u.clip` is either
// the world projection or the viewmodel's (first-person effects live in the viewmodel's space, like retail's _FP
// effects). Sprites are premultiplied: rgb = light added, a = how much they cover (0 for pure glow).

struct U {
    clip: mat4x4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    to_sun: vec4<f32>,
    sun: vec4<f32>,
    sky: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var layers: texture_2d_array<f32>;
@group(0) @binding(2) var samp: sampler;

struct SIn {
    @location(0) pos_size: vec4<f32>,   // centre (camera-relative), half width
    @location(1) axis_rot: vec4<f32>,   // half-length axis for stretched sprites (0 = billboard), rotation (radians)
    @location(2) color: vec4<f32>,      // premultiplied multiplier
    @location(3) layer_aspect: vec4<f32>, // texture layer, height / width (billboards)
};

struct SOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: i32,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_sprite(@builtin(vertex_index) vi: u32, i: SIn) -> SOut {
    let corners = array<vec2<f32>, 6>(vec2(-1.0, -1.0), vec2(1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(-1.0, 1.0));
    let c = corners[vi];
    let p = i.pos_size.xyz;
    let w = i.pos_size.w;
    let axis = i.axis_rot.xyz;
    var world: vec3<f32>;
    if (dot(axis, axis) > 1e-8) {
        // stretched along `axis` (tracers, sparks, flame tongues), turned to face the camera around it
        let side = normalize(cross(axis, p + vec3<f32>(1e-4, 0.0, 0.0)));
        world = p + axis * c.y + side * (w * c.x);
    } else {
        let s = sin(i.axis_rot.w);
        let co = cos(i.axis_rot.w);
        let r = vec2<f32>(c.x * co - c.y * s, c.x * s + c.y * co);
        world = p + u.right.xyz * (r.x * w) + u.up.xyz * (r.y * w * i.layer_aspect.y);
    }
    var o: SOut;
    o.clip = u.clip * vec4<f32>(world, 1.0);
    // billboards upright; stretched sprites have the texture's top (its bright end) at the start of the axis
    o.uv = vec2<f32>(c.x * 0.5 + 0.5, select(0.5 - c.y * 0.5, 0.5 + c.y * 0.5, dot(axis, axis) > 1e-8));
    o.layer = i32(i.layer_aspect.x);
    o.color = i.color;
    return o;
}

@fragment
fn fs_sprite(i: SOut) -> @location(0) vec4<f32> {
    let t = textureSample(layers, samp, i.uv, i.layer);
    return t * i.color;
}

struct MIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) r0: vec4<f32>,
    @location(4) r1: vec4<f32>,
    @location(5) r2: vec4<f32>,
};

struct MOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) rel: vec3<f32>,
};

@vertex
fn vs_shell(i: MIn) -> MOut {
    let p = vec4<f32>(i.pos, 1.0);
    let w = vec3<f32>(dot(i.r0, p), dot(i.r1, p), dot(i.r2, p));
    let n = vec3<f32>(dot(i.r0.xyz, i.nrm), dot(i.r1.xyz, i.nrm), dot(i.r2.xyz, i.nrm));
    var o: MOut;
    o.clip = u.clip * vec4<f32>(w, 1.0);
    o.uv = i.uv;
    o.nrm = n;
    o.rel = w;
    return o;
}

@fragment
fn fs_shell(i: MOut) -> @location(0) vec4<f32> {
    // the shell's albedo layer (u.up.w), lit by the sun and sky like a small metal part
    let albedo = textureSample(layers, samp, i.uv, i32(u.up.w)).rgb;
    let n = normalize(i.nrm);
    let ndl = max(dot(n, u.to_sun.xyz), 0.0);
    let spec = pow(max(dot(reflect(-u.to_sun.xyz, n), normalize(-i.rel)), 0.0), 24.0);
    // dimmed to sit with the viewmodel's probe-lit look (the shell has no probe of its own)
    let amb = dot(u.sky.rgb, vec3<f32>(0.33, 0.33, 0.34)) * 0.06;
    let lit = albedo * (u.sun.rgb * ndl * 0.12 + vec3<f32>(amb)) + u.sun.rgb * spec * 0.06 * albedo;
    return vec4<f32>(lit, 1.0);
}
