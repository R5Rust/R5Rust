enable wgpu_binding_array;

// Materials, bindless (gfx.rs register_materials): every material's constants in one storage array and every texture
// they sample in one binding array, bound once per pass; a draw picks its material with a 4-byte immediate (pc.mat).
// Replaces a bind group per material: wgpu's per-switch cost was ~2 ms of CPU a frame (bench.rs, 2026-11-21).
// Prepended to world.wgsl and shadow.wgsl.
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
    uv_rs: array<vec4<f32>, 5>,  // c_uv1..5 RotScaleX.xy, RotScaleY.xy
    uv_tr: array<vec4<f32>, 5>,  // c_uv1..5 Translate.xy
    route: array<vec4<u32>, 4>,  // UV per texture slot (R_*): uv index 0..5 (0 = untransformed) | 8 world-planar
    detail: vec4<f32>,           // x has detail (L0), y has detail2 (L1), z detail2 overlaid (else 2x multiply), w material index
    vmfx: vec4<f32>,             // viewmodel items: x 1 = emissive x scrolling multiply, 2 = glass; y opacity; zw distortion
    vm_uv1: vec4<f32>,           // (angle, scaleU, scaleV, 0)
    vm_uv2: vec4<f32>,
    vm_tr: vec4<f32>,            // c_uv1Translate.xy, c_uv2Translate.xy (per second of c_gameTime)
    emis2: vec4<f32>,            // rgb c_L1_emissiveTint, w has emissive2Texture
    fx2: vec4<f32>,              // x c_uvN in rotation form (bit N-1), y has emissiveMultiplyTexture
    zup: vec4<f32>,
    opac: vec4<f32>,             // opacityMultiplyTexture (in the mask slot): x c_opacity, y distance-fade scale, z bias, w 1 = on
    opac_edge: vec4<f32>,        // alpha edge fade: x exponent, y inner, z outer              // zUp rock blend: x c_zUpBlendingMinAngleCos, y MaxAngleCos, z VertexAlpha, w 1 = on (layer 1 triplanar)
    // texture indices into texs: [albedo normal gloss spec] [ao emissive albedo2 normal2] [gloss2 spec2 blend detail_normal]
    // [detail detail2 emissive_mult emissive2] (viewmodels: slot 6 = emissive multiply, 10 = opacity, 11 = distortion)
    tix: array<vec4<u32>, 4>,
};
struct MatPc {
    mat: u32,
};
var<immediate> pc: MatPc;
@group(1) @binding(0) var<storage, read> mats: array<Mat>;
@group(1) @binding(1) var texs: binding_array<texture_2d<f32>>;
