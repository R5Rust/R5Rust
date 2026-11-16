// Sun shadow cascade pass: depth only, alpha-tested materials discard by base alpha.

struct Cascade {
    light_vp: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> cs: Cascade;
@group(0) @binding(1) var samp: sampler;

struct Mat {
    color: vec4<f32>,
    params: vec4<f32>,  // x = alpha cutoff, z = alpha mode
};
@group(1) @binding(0) var<uniform> mt: Mat;
@group(1) @binding(1) var base_tex: texture_2d<f32>;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec3<f32>, @location(2) uv0: vec2<f32>) -> VOut {
    var o: VOut;
    o.clip = cs.light_vp * vec4<f32>(pos, 1.0);
    o.uv = uv0;
    return o;
}

@fragment
fn fs_mask(i: VOut) {
    let a = textureSample(base_tex, samp, i.uv).a * mt.color.a;
    if (a < mt.params.x) { discard; }
}
