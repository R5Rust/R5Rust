// Sun shadow cascade pass: depth only, alpha-tested materials discard by base alpha.

struct Cascade {
    light_vp: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> cs: Cascade;
@group(0) @binding(1) var samp: sampler;

// struct Mat, mats, texs, pc: mat.wgsl (prepended); the mask reads the albedo (slot 0) alpha

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

// static props, instanced (props.rs): model -> world rows from the instance stream
@vertex
fn vs_prop(@location(0) pos: vec3<f32>, @location(2) uv0: vec2<f32>, @location(6) r0: vec4<f32>, @location(7) r1: vec4<f32>, @location(8) r2: vec4<f32>) -> VOut {
    var o: VOut;
    let p = vec4<f32>(pos, 1.0);
    o.clip = cs.light_vp * vec4<f32>(dot(r0, p), dot(r1, p), dot(r2, p), 1.0);
    o.uv = uv0;
    return o;
}

@fragment
fn fs_mask(i: VOut) {
    let a = textureSample(texs[mats[pc.mat].tix[0].x], samp, i.uv).a * mats[pc.mat].color.a;
    if (a < mats[pc.mat].params.x) { discard; }
}
