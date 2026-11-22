//! wgpu device, render targets, pipelines and the per-frame draw.
//! World pass: 4x MSAA HDR (Rgba16Float) + reverse-Z depth, sky first, then opaque / alpha-tested
//! (alpha-to-coverage) / blended draws. Post pass: ACES tonemap, or the loading screen.

use crate::bloom::Bloom;
use crate::csm::{self, Csm};
use crate::graphics::GraphicsSettings;
use crate::loader::{upload_rgba, LoadedWorld};
use glam::{Mat4, Vec3, Vec4};
use shared::assets::Config;
use std::sync::Arc;
use winit::window::Window;

pub const MSAA: u32 = 4;
const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Bindless materials (mat.wgsl): texture binding arrays indexed per draw by an immediate.
const BINDLESS: wgpu::Features = wgpu::Features::TEXTURE_BINDING_ARRAY
    .union(wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING)
    .union(wgpu::Features::PARTIALLY_BOUND_BINDING_ARRAY)
    .union(wgpu::Features::IMMEDIATES);
/// Size of the material texture table (the world uses ~3000 distinct textures)
const MAX_MAT_TEXTURES: u32 = 16384;

/// The world shader: material table, the ray-traced sun (rt.rs) on or stubbed, then world.wgsl.
fn world_source(rt: bool) -> String {
    if rt {
        format!("enable wgpu_ray_query;\n{}{}{}", include_str!("mat.wgsl"), include_str!("rt_on.wgsl"), include_str!("world.wgsl"))
    } else {
        format!("{}{}{}", include_str!("mat.wgsl"), include_str!("rt_off.wgsl"), include_str!("world.wgsl"))
    }
}

pub const SHOT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Default)]
pub struct FrameUniform {
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub cam_pos: [f32; 4],
    pub to_sun: [f32; 4],
    pub sun_color: [f32; 4],
    pub sky: [f32; 4],
    /// video settings: shadow map res, cascades in use, specular on, alpha test by discard (no MSAA)
    pub quality: [f32; 4],
    pub fog: [f32; 4],
    pub misc: [f32; 4],
    pub shadow_vp: [[[f32; 4]; 4]; csm::N],
    /// cascade half-widths
    pub cascade_r: [f32; 4],
    /// Apex height fog c_fogParams k0..k4 (see world.wgsl fog())
    pub fog_k: [[f32; 4]; 5],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Default)]
struct PostUniform {
    params: [f32; 4],
    screen: [f32; 4],
    /// x = bloom amount
    bloom: [f32; 4],
}

pub struct Camera {
    pub debug: f32,
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// horizontal FOV at 4:3, like Source / Apex
    pub fov: f32,
}

pub struct Gfx {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_name: String,
    surface: Option<(wgpu::Surface<'static>, wgpu::SurfaceConfiguration)>,
    out_format: wgpu::TextureFormat,
    out_srgb: bool,
    pub width: u32,
    pub height: u32,
    /// the world's render size (window size x render scale)
    rw: u32,
    rh: u32,
    frame_layout: wgpu::BindGroupLayout,
    pub mat_layout: wgpu::BindGroupLayout,
    /// the bindless material table (mat.wgsl): views + constants of the world's and the viewmodels' materials
    mat_views: Vec<wgpu::TextureView>,
    mat_data: Vec<crate::loader::MatUniform>,
    pub mat_bg: Option<wgpu::BindGroup>,
    post_layout: wgpu::BindGroupLayout,
    world_sh: wgpu::ShaderModule,
    /// experimental ray-traced sun shadows (rt.rs): the acceleration structures once built, and whether they shade
    rt: Option<crate::rt::Rt>,
    pub rt_on: bool,
    world_pl: wgpu::PipelineLayout,
    sky_pl: wgpu::PipelineLayout,
    /// world + material + viewmodel (bones, projection) groups
    pub vm_layout: wgpu::BindGroupLayout,
    vm_pl: wgpu::PipelineLayout,
    /// the first-person weapon (viewmodel.rs), drawn last in the world pass when `vm_visible`
    pub viewmodel: Option<crate::viewmodel::Viewmodel>,
    /// the melee slot's viewmodel (empty hands or the heirloom, melee.rs), drawn instead when `vm_melee`
    pub melee_vm: Option<crate::viewmodel::Viewmodel>,
    /// the first-person proxy: bare arms on the pov_mp_core rig (tools/build_arms.py), shown instead of the held item
    /// during traversal (mantle hands), and whether it is the one on screen this frame
    pub proxy_vm: Option<crate::viewmodel::Viewmodel>,
    pub vm_proxy: bool,
    pub vm_melee: bool,
    pub vm_visible: bool,
    /// the blurred frame behind the HUD (backdrop.rs), refreshed after the post pass when `hud_backdrop_on`
    pub backdrop: Option<crate::backdrop::Backdrop>,
    pub hud_backdrop_on: bool,
    /// bullet impacts to draw this frame: (pos.xyz, size, normal.xyz, alpha)
    pub impacts: Vec<[f32; 12]>,
    /// retail bullet-hole decals (decals.rs); without them impacts draw as plain dark discs
    pub decals: Option<crate::decals::Decals>,
    impact_buf: wgpu::Buffer,
    /// weapon effects (fx.rs): GPU side once the weapon loads, and this frame's sprites / shells
    pub fx: Option<crate::fx::FxGpu>,
    pub fx_frame: crate::fx::FxFrame,
    /// world pipelines for the current MSAA sample count
    pipes: Pipes,
    post_pipe: wgpu::RenderPipeline,
    /// the video settings in effect (graphics.rs)
    pub settings: GraphicsSettings,
    has_mailbox: bool,
    frame_ub: wgpu::Buffer,
    /// the 3D skybox pass's frame uniform (sky camera), and its bind group
    sky_ub: wgpu::Buffer,
    sky_bg: Option<wgpu::BindGroup>,
    /// set by `frame_uniform` when the map has a 3D skybox
    sky_fu: std::cell::Cell<Option<FrameUniform>>,
    post_ub: wgpu::Buffer,
    lm_samp: wgpu::Sampler,
    mat_samp: wgpu::Sampler,
    post_samp: wgpu::Sampler,
    /// None with MSAA off (the world pass draws straight into `hdr_view`)
    msaa_view: Option<wgpu::TextureView>,
    hdr_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    /// retail's object highlight (loot rarity outline, highlight.rs)
    highlight: crate::highlight::Highlight,
    /// per-pass CPU / GPU times (R5_PROF=1), prof.rs
    pub prof: Option<crate::prof::Prof>,
    /// this frame's projection for the highlight composite: near, 1 / proj[0][0], 1 / proj[1][1]
    hl_proj: std::cell::Cell<[f32; 3]>,
    /// the ground loot's instance signature when the shadow cascades were last rendered
    loot_shadow_sig: u64,
    load_view: wgpu::TextureView,
    load_size: (u32, u32),
    post_bg: wgpu::BindGroup,
    frame_bg: Option<wgpu::BindGroup>,
    pub csm: Csm,
    bloom: Bloom,
    pub bloom_amount: f32,
    /// grading LUT in use (a dummy until a map provides one)
    grading: wgpu::TextureView,
    grading_on: bool,
    /// grading off until it matches the real game (R5_GRADE=1 turns it on)
    pub no_grading: bool,
    /// --debug 11 / 12 (surface ids / distance): show the HDR target as-is, no exposure / bloom / tonemap / grading
    pub raw_post: bool,
    /// 0 = ACES fit, 1 = Apex tonemap function 0 (env R5_TONEMAP)
    pub tonemap: u32,
    expo_state: wgpu::Buffer,
    expo_params: wgpu::Buffer,
    expo_layout: wgpu::BindGroupLayout,
    expo_bg: wgpu::BindGroup,
    expo_lum: wgpu::ComputePipeline,
    expo_adapt: wgpu::ComputePipeline,
    /// c_snapExposure for the next frame (set on world load, like the engine's level reset FUN_140962750)
    expo_snap: std::cell::Cell<bool>,
    /// snap exposure every frame (screenshot mode)
    pub instant_exposure: bool,
    /// auto-exposure settings (Apex mat_autoexposure_* equivalents)
    pub expo: shared::assets::AutoExposure,
    /// fixed exposure (calibration), bypasses the meter
    pub forced_exposure: Option<f32>,
    /// linear multiplier before the tonemap
    pub exposure: f32,
    pub drawn: std::cell::Cell<u32>,
    /// triangles in the chunks drawn last frame (perf readout)
    pub drawn_tris: std::cell::Cell<u64>,
    /// summed thread cycles in Gfx::draw: the post encoder's finish, and waiting for the world encoder + submit (bench.rs)
    pub submit_ms: std::cell::Cell<[f64; 2]>,
    /// R5_NOSKYBOX / R5_SKYBOX_ONLY / R5_NOSKYMAT / R5_NOBLEND debug switches, read once (not per draw)
    dbg: [bool; 4],
    /// static-prop LOD knobs the console can override (props.rs); Model Detail comes from `settings`
    pub lod: LodKnobs,
    /// this frame's visible props (prepare picks them, draw draws them)
    prop_sel: std::cell::RefCell<crate::props::Selection>,
    /// props drawn last frame: (instances, instanced draws)
    pub drawn_props: std::cell::Cell<(u32, u32)>,
    /// the LOD parameters of the last prepare (cascade timing re-uses them)
    last_lod: std::cell::Cell<Option<crate::props::LodParams>>,
    offscreen: Option<wgpu::TextureView>,
}

/// bullet impact marks kept on screen at once
pub const MAX_IMPACTS: usize = 512;

/// Console overrides for static-prop LOD (`r_lod`, `r_lod_switch_scale`, `fadedistscale`, `r_drawstaticprops`).
#[derive(Clone, Copy)]
pub struct LodKnobs {
    pub force: i32,
    pub switch_scale: Option<f32>,
    pub fade_scale: f32,
    pub draw_props: bool,
    pub dress_level: u8,
}

impl Default for LodKnobs {
    fn default() -> Self {
        LodKnobs { force: -1, switch_scale: None, fade_scale: 1.0, draw_props: true, dress_level: 1 }
    }
}

/// The world pass's pipelines, rebuilt when the MSAA sample count changes.
struct Pipes {
    samples: u32,
    /// key = alpha_mode * 2 + double_sided (0..5), 6 = sky material, 7..12 = 0..5 with the decal depth bias
    world: Vec<wgpu::RenderPipeline>,
    /// the same keys for instanced static props (props.rs), and their dithered fade-out copies (None for the sky key)
    prop: Vec<wgpu::RenderPipeline>,
    prop_fade: Vec<Option<wgpu::RenderPipeline>>,
    /// skinned first-person weapon (opaque, both sides), and its alpha-blended materials (glass) drawn after it
    viewmodel: wgpu::RenderPipeline,
    viewmodel_blend: wgpu::RenderPipeline,
    /// bullet impact marks (impacts.wgsl)
    impacts: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    /// fullscreen triangle that resets depth to the far plane (between the 3D skybox and the world)
    depth_reset: wgpu::RenderPipeline,
}

impl Pipes {
    fn new(device: &wgpu::Device, world_sh: &wgpu::ShaderModule, world_pl: &wgpu::PipelineLayout, sky_pl: &wgpu::PipelineLayout, vm_pl: &wgpu::PipelineLayout, samples: u32) -> Pipes {
        let vattrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm8x4, 2 => Float32x2, 3 => Unorm16x2, 4 => Unorm8x4];
        let vlayout = wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vattrs };
        let pattrs = wgpu::vertex_attr_array![5 => Uint32];
        let player = wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Vertex, attributes: &pattrs };
        // alpha-to-coverage needs MSAA; without it the shader cuts out by discard (Frame.quality.w)
        // static props: the instance stream replaces the per-vertex probe stream (props::InstGpu)
        // 10 = highlight slot (unused here), 11 = diffuseModulation (props::InstGpu.pad)
        let iattrs = wgpu::vertex_attr_array![6 => Float32x4, 7 => Float32x4, 8 => Float32x4, 5 => Uint32, 9 => Float32, 10 => Uint32, 11 => Uint32];
        let ilayout = wgpu::VertexBufferLayout { array_stride: crate::props::INST_STRIDE, step_mode: wgpu::VertexStepMode::Instance, attributes: &iattrs };
        let ms = |a2c: bool| wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: a2c && samples > 1 };
        // (vertex entry, fragment entry) per kind: world chunks, props, fading props
        let kinds: [(&str, &str, &wgpu::VertexBufferLayout); 3] = [("vs_main", "fs_main", &player), ("vs_prop", "fs_main", &ilayout), ("vs_prop", "fs_prop_fade", &ilayout)];
        let mk_pipe = |key: u8, bias: wgpu::DepthBiasState, kind: usize| {
            let alpha = key / 2;
            let double = key % 2 == 1;
            let (vs, fs, second) = kinds[kind];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(["world", "prop", "prop fade"][kind]),
                layout: Some(world_pl),
                vertex: wgpu::VertexState { module: world_sh, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[Some(vlayout.clone()), Some(second.clone())] },
                primitive: wgpu::PrimitiveState { cull_mode: if double { None } else { Some(wgpu::Face::Back) }, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(alpha != 2),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias,
                }),
                multisample: ms(alpha == 1),
                fragment: Some(wgpu::FragmentState {
                    module: world_sh,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: HDR, blend: if alpha == 2 { Some(wgpu::BlendState::ALPHA_BLENDING) } else { None }, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // key 6: sky-shaded surfaces (unlit cube lookup + edge-faded emissive), opaque, no culling
        let sky_mat = |kind: usize| {
            let (vs, _, second) = kinds[kind];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sky material"),
                layout: Some(world_pl),
                vertex: wgpu::VertexState { module: world_sh, entry_point: Some(vs), compilation_options: Default::default(), buffers: &[Some(vlayout.clone()), Some(second.clone())] },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: ms(false),
            fragment: Some(wgpu::FragmentState { module: world_sh, entry_point: Some("fs_skymat"), compilation_options: Default::default(), targets: &[Some(HDR.into())] }),
            multiview_mask: None,
            cache: None,
            })
        };
        // Decal-class materials (rasterizerFlags & 0x70 == 0x10): the engine's decal depth bias, negated for its reverse-Z
        // view like ours (r5apex_dx12 FUN_1405177d0: DepthBias = -mat_depthbias_decal = 16, SlopeScaledDepthBias =
        // -mat_slopescaledepthbias_decal = 4, DepthBiasClamp = -mat_depthbiasclamp_decal = 0.001). docs/apex-lighting.md 9.
        let decal = wgpu::DepthBiasState { constant: 16, slope_scale: 4.0, clamp: 0.001 };
        // index = pipeline key: 0..5, 6 sky material, 7..12 = 0..5 with the decal bias
        let set = |kind: usize| -> Vec<Option<wgpu::RenderPipeline>> {
            let mut v: Vec<Option<wgpu::RenderPipeline>> = (0..6u8).map(|k| Some(mk_pipe(k, Default::default(), kind))).collect();
            v.push(if kind == 2 { None } else { Some(sky_mat(kind)) });
            v.extend((0..6u8).map(|k| Some(mk_pipe(k, decal, kind))));
            v
        };
        let world: Vec<wgpu::RenderPipeline> = set(0).into_iter().map(Option::unwrap).collect();
        let prop: Vec<wgpu::RenderPipeline> = set(1).into_iter().map(Option::unwrap).collect();
        let prop_fade = set(2);
        let sattrs = wgpu::vertex_attr_array![10 => Uint8x4, 11 => Unorm8x4];
        let slayout = wgpu::VertexBufferLayout { array_stride: crate::viewmodel::SKIN_STRIDE, step_mode: wgpu::VertexStepMode::Vertex, attributes: &sattrs };
        let viewmodel = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewmodel"),
            layout: Some(vm_pl),
            vertex: wgpu::VertexState { module: world_sh, entry_point: Some("vs_viewmodel"), compilation_options: Default::default(), buffers: &[Some(vlayout.clone()), Some(slayout.clone())] },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: ms(false),
            fragment: Some(wgpu::FragmentState { module: world_sh, entry_point: Some("fs_main"), compilation_options: Default::default(), targets: &[Some(HDR.into())] }),
            multiview_mask: None,
            cache: None,
        });
        let viewmodel_blend = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewmodel blend"),
            layout: Some(vm_pl),
            vertex: wgpu::VertexState { module: world_sh, entry_point: Some("vs_viewmodel"), compilation_options: Default::default(), buffers: &[Some(vlayout.clone()), Some(slayout)] },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: ms(false),
            fragment: Some(wgpu::FragmentState {
                module: world_sh,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: HDR, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sky = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(sky_pl),
            vertex: wgpu::VertexState { module: world_sh, entry_point: Some("vs_sky"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
            multisample: ms(false),
            fragment: Some(wgpu::FragmentState { module: world_sh, entry_point: Some("fs_sky"), compilation_options: Default::default(), targets: &[Some(HDR.into())] }),
            multiview_mask: None,
            cache: None,
        });
        let depth_reset = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("depth reset"),
            layout: Some(sky_pl),
            vertex: wgpu::VertexState { module: world_sh, entry_point: Some("vs_sky"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
            multisample: ms(false),
            fragment: Some(wgpu::FragmentState {
                module: world_sh,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: HDR, blend: None, write_mask: wgpu::ColorWrites::empty() })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let imp_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("impacts"), source: wgpu::ShaderSource::Wgsl(include_str!("impacts.wgsl").into()) });
        let iattrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let impacts = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("impacts"),
            layout: Some(sky_pl),
            vertex: wgpu::VertexState {
                module: &imp_sh,
                entry_point: Some("vs_impact"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: 48, step_mode: wgpu::VertexStepMode::Instance, attributes: &iattrs })],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: wgpu::DepthBiasState { constant: 16, slope_scale: 4.0, clamp: 0.001 },
            }),
            multisample: ms(false),
            fragment: Some(wgpu::FragmentState {
                module: &imp_sh,
                entry_point: Some("fs_impact"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: HDR, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Pipes { samples, world, prop, prop_fade, viewmodel, viewmodel_blend, impacts, sky, depth_reset }
    }
}

/// window size x render scale (percent), at least 1x1
fn scaled(w: u32, h: u32, pct: u32) -> (u32, u32) {
    let s = pct.clamp(50, 100) as f32 / 100.0;
    (((w as f32 * s).round() as u32).max(1), ((h as f32 * s).round() as u32).max(1))
}

fn backends(name: &str) -> wgpu::Backends {
    match name.to_ascii_lowercase().as_str() {
        "vulkan" | "vk" => wgpu::Backends::VULKAN,
        "dx12" | "d3d12" => wgpu::Backends::DX12,
        "gl" | "opengl" => wgpu::Backends::GL,
        _ => wgpu::Backends::all(),
    }
}

fn targets(device: &wgpu::Device, w: u32, h: u32, samples: u32) -> (Option<wgpu::TextureView>, wgpu::TextureView, wgpu::TextureView) {
    let mk = |format, samples, usage, label| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default())
    };
    (
        (samples > 1).then(|| mk(HDR, samples, wgpu::TextureUsages::RENDER_ATTACHMENT, "msaa")),
        mk(HDR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, "hdr"),
        mk(DEPTH, samples, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, "depth"),
    )
}

fn layout_entry(binding: u32, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty, count: None }
}
fn uniform_ty() -> wgpu::BindingType {
    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }
}
fn tex_ty() -> wgpu::BindingType {
    wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }
}
fn storage_ty() -> wgpu::BindingType {
    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }
}
fn samp_ty() -> wgpu::BindingType {
    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
}

impl Gfx {
    /// `window` = None renders offscreen only (screenshot mode).
    pub fn new(window: Option<Arc<Window>>, width: u32, height: u32, cfg: &Config, gs: &GraphicsSettings, loadscreen: Option<image::RgbaImage>) -> Result<Gfx, String> {
        let mut order = vec![backends(&cfg.backend)];
        for b in [wgpu::Backends::VULKAN, wgpu::Backends::DX12, wgpu::Backends::GL] {
            if !order.contains(&b) {
                order.push(b);
            }
        }
        let mut found = None;
        for b in order {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: b, ..wgpu::InstanceDescriptor::new_without_display_handle() });
            let surface = match &window {
                Some(w) => match instance.create_surface(w.clone()) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        log::warn!("{b:?}: surface: {e}");
                        continue;
                    }
                },
                None => None,
            };
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: surface.as_ref(),
                apply_limit_buckets: false,
            }));
            match adapter {
                Ok(a) => {
                    found = Some((instance, surface, a));
                    break;
                }
                Err(e) => log::warn!("{b:?}: no adapter: {e}"),
            }
        }
        let (_instance, surface, adapter) = found.ok_or("no usable GPU backend (tried Vulkan, DX12, GL)")?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?})", info.name, info.backend);
        log::info!("GPU: {adapter_name}");

        if !adapter.features().contains(BINDLESS) {
            return Err(format!("{adapter_name} lacks bindless textures + immediates ({:?}); r5rust needs Vulkan or DX12 with descriptor indexing", BINDLESS - adapter.features()));
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("r5rust"),
            // ray queries when the GPU has them (rt.rs; nothing uses them until r_rtshadows 1)
            required_features: wgpu::Features::TEXTURE_COMPRESSION_BC | BINDLESS | crate::prof::features(&adapter) | (adapter.features() & crate::rt::FEATURES),
            required_limits: adapter.limits(),
            // wgpu's ray queries are behind its experimental-features token
            experimental_features: if adapter.features().contains(crate::rt::FEATURES) { unsafe { wgpu::ExperimentalFeatures::enabled() } } else { wgpu::ExperimentalFeatures::disabled() },
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("request_device: {e}"))?;

        let (surface, out_format, has_mailbox) = match surface {
            Some(s) => {
                let caps = s.get_capabilities(&adapter);
                let fmt = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
                let present_mode = present_mode(cfg.vsync || gs.vsync, caps.present_modes.contains(&wgpu::PresentMode::Mailbox));
                let config = wgpu::SurfaceConfiguration {
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    format: fmt,
                    width: width.max(1),
                    height: height.max(1),
                    present_mode,
                    desired_maximum_frame_latency: 2,
                    alpha_mode: caps.alpha_modes[0],
                    view_formats: vec![],
                    color_space: wgpu::SurfaceColorSpace::Auto,
                };
                s.configure(&device, &config);
                (Some((s, config)), fmt, caps.present_modes.contains(&wgpu::PresentMode::Mailbox))
            }
            None => (None, SHOT_FORMAT, false),
        };
        let out_srgb = out_format.is_srgb();

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                layout_entry(0, uniform_ty()),
                layout_entry(1, tex_ty()),
                layout_entry(2, tex_ty()),
                layout_entry(3, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
                layout_entry(4, tex_ty()),
                layout_entry(5, samp_ty()),
                layout_entry(6, samp_ty()),
                layout_entry(7, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }),
                layout_entry(8, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)),
                layout_entry(9, storage_ty()),
                layout_entry(10, storage_ty()),
                layout_entry(11, storage_ty()),
                layout_entry(12, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::CubeArray, multisampled: false }),
                layout_entry(13, storage_ty()),
                layout_entry(14, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::Cube, multisampled: false }),
            ],
        });
        // bindless materials (mat.wgsl): every material's constants + every texture they use, one group for all
        let mat_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mat"),
            entries: &[
                layout_entry(0, storage_ty()),
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: tex_ty(), count: std::num::NonZeroU32::new(MAX_MAT_TEXTURES) },
            ],
        });
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[layout_entry(0, uniform_ty()), layout_entry(1, tex_ty()), layout_entry(2, tex_ty()), layout_entry(3, samp_ty()), layout_entry(4, storage_ty()), layout_entry(5, tex_ty()), layout_entry(6, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D3, multisampled: false })],
        });

        let world_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("world"), source: wgpu::ShaderSource::Wgsl(world_source(false).into()) });
        let post_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("post"), source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()) });

        let world_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("world"), bind_group_layouts: &[Some(&frame_layout), Some(&mat_layout)], immediate_size: 4 });
        let sky_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("sky"), bind_group_layouts: &[Some(&frame_layout)], immediate_size: 0 });
        let vm_layout = crate::viewmodel::layout(&device);
        let vm_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("viewmodel"), bind_group_layouts: &[Some(&frame_layout), Some(&mat_layout), Some(&vm_layout)], immediate_size: 4 });
        let post_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("post"), bind_group_layouts: &[Some(&post_layout)], immediate_size: 0 });

        let msaa = if gs.msaa >= 4 { MSAA } else { 1 };
        let impact_buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("impacts"), size: 48 * MAX_IMPACTS as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let pipes = Pipes::new(&device, &world_sh, &world_pl, &sky_pl, &vm_pl, msaa);
        let post_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post"),
            layout: Some(&post_pl),
            vertex: wgpu::VertexState { module: &post_sh, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &post_sh, entry_point: Some("fs_main"), compilation_options: Default::default(), targets: &[Some(out_format.into())] }),
            multiview_mask: None,
            cache: None,
        });

        let frame_ub = device.create_buffer(&wgpu::BufferDescriptor { label: Some("frame"), size: std::mem::size_of::<FrameUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let sky_ub = device.create_buffer(&wgpu::BufferDescriptor { label: Some("sky frame"), size: std::mem::size_of::<FrameUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let post_ub = device.create_buffer(&wgpu::BufferDescriptor { label: Some("post"), size: std::mem::size_of::<PostUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

        let lin = |addr, aniso: u16, mip| wgpu::SamplerDescriptor {
            address_mode_u: addr,
            address_mode_v: addr,
            address_mode_w: addr,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: mip,
            anisotropy_clamp: aniso,
            ..Default::default()
        };
        let lm_samp = device.create_sampler(&lin(wgpu::AddressMode::ClampToEdge, 1, wgpu::MipmapFilterMode::Nearest));
        let mat_samp = mat_sampler(&device, gs.anisotropic);
        let post_samp = device.create_sampler(&lin(wgpu::AddressMode::ClampToEdge, 1, wgpu::MipmapFilterMode::Nearest));

        let (res, count) = gs.sun_shadows.cascades();
        let csm = Csm::new(&device, &mat_layout, &mat_samp, res, count);

        let ls = loadscreen.unwrap_or_else(|| image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 10, 12, 255])));
        let load_size = ls.dimensions();
        let load_view = upload_rgba(&device, &queue, &[ls], wgpu::TextureFormat::Rgba8UnormSrgb, "loadscreen").create_view(&Default::default());

        let (bw, bh) = if gs.stretch[0] > 0 { (gs.stretch[0], gs.stretch[1]) } else { (width.max(1), height.max(1)) };
        let (rw, rh) = scaled(bw, bh, gs.render_scale);
        let (msaa_view, hdr_view, depth_view) = targets(&device, rw, rh, msaa);
        let expo_state = device.create_buffer(&wgpu::BufferDescriptor { label: Some("expo"), size: 16, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let expo_params = device.create_buffer(&wgpu::BufferDescriptor { label: Some("expo params"), size: 48, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let cs = wgpu::ShaderStages::COMPUTE;
        let expo_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("expo"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: cs, ty: tex_ty(), count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: cs, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: cs, ty: uniform_ty(), count: None },
            ],
        });
        let expo_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("expo"), source: wgpu::ShaderSource::Wgsl(include_str!("expo.wgsl").into()) });
        let expo_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("expo"), bind_group_layouts: &[Some(&expo_layout)], immediate_size: 0 });
        let mkc = |e: &str| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("expo"), layout: Some(&expo_pl), module: &expo_sh, entry_point: Some(e), compilation_options: Default::default(), cache: None });
        let (expo_lum, expo_adapt) = (mkc("lum"), mkc("adapt"));
        let expo_bg = Self::make_expo_bg(&device, &expo_layout, &hdr_view, &expo_state, &expo_params);
        let mut bloom = Bloom::new(&device, cfg.bloom_cutoff, cfg.bloom_clamp);
        bloom.resize(&device, &queue, &hdr_view, &expo_state, rw, rh);
        let grading = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("no grading"),
                size: wgpu::Extent3d { width: 2, height: 2, depth_or_array_layers: 2 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgb10a2Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let post_bg = Self::make_post_bg(&device, &post_layout, &post_ub, &hdr_view, &load_view, &post_samp, &expo_state, &bloom.output, &grading);

        let mut highlight = crate::highlight::Highlight::new(&device, &world_sh, &sky_pl, HDR, msaa);
        highlight.resize(&device, HDR, rw, rh, msaa, &depth_view, &expo_state);
        let backdrop = Some(crate::backdrop::Backdrop::new(&device, &queue, out_format, (width, height)));
        let prof = crate::prof::Prof::new(&device, &queue);
        Ok(Gfx {
            device,
            queue,
            adapter_name,
            surface,
            out_format,
            out_srgb,
            width: width.max(1),
            height: height.max(1),
            rw,
            rh,
            frame_layout,
            mat_layout,
            mat_views: Vec::new(),
            mat_data: Vec::new(),
            mat_bg: None,
            post_layout,
            world_sh,
            rt: None,
            rt_on: false,
            world_pl,
            sky_pl,
            vm_layout,
            vm_pl,
            viewmodel: None,
            melee_vm: None,
            proxy_vm: None,
            vm_proxy: false,
            vm_melee: false,
            vm_visible: false,
            backdrop,
            hud_backdrop_on: false,
            impacts: Vec::new(),
            decals: None,
            impact_buf,
            fx: None,
            fx_frame: Default::default(),
            pipes,
            post_pipe,
            settings: gs.clone(),
            has_mailbox,
            frame_ub,
            sky_ub,
            sky_bg: None,
            sky_fu: std::cell::Cell::new(None),
            post_ub,
            lm_samp,
            mat_samp,
            post_samp,
            msaa_view,
            hdr_view,
            depth_view,
            highlight,
            prof,
            hl_proj: std::cell::Cell::new([4.0, 1.0, 1.0]),
            loot_shadow_sig: 0,
            load_view,
            load_size,
            post_bg,
            frame_bg: None,
            csm,
            bloom,
            bloom_amount: cfg.bloom_amount,
            grading,
            grading_on: false,
            // game grading LUT + Apex tonemap function 0 measured closest to R5Flowstate (R5_NOGRADE=1 to compare)
            no_grading: std::env::var_os("R5_NOGRADE").is_some(),
            raw_post: false,
            tonemap: std::env::var("R5_TONEMAP").ok().and_then(|v| v.parse().ok()).unwrap_or(cfg.tonemap),
            expo_state,
            expo_params,
            expo_layout,
            expo_bg,
            expo_lum,
            expo_adapt,
            expo_snap: std::cell::Cell::new(true),
            instant_exposure: false,
            expo: cfg.auto_exposure.clone(),
            forced_exposure: None,
            exposure: cfg.exposure,
            drawn: std::cell::Cell::new(0),
            drawn_tris: std::cell::Cell::new(0),
            submit_ms: std::cell::Cell::new([0.0; 2]),
            // R5_LOD=<n>: force a static-prop LOD in screenshot runs (the console has r_lod)
            lod: LodKnobs { force: std::env::var("R5_LOD").ok().and_then(|v| v.parse().ok()).unwrap_or(-1), ..Default::default() },
            prop_sel: Default::default(),
            drawn_props: std::cell::Cell::new((0, 0)),
            last_lod: std::cell::Cell::new(None),
            dbg: ["R5_NOSKYBOX", "R5_SKYBOX_ONLY", "R5_NOSKYMAT", "R5_NOBLEND"].map(|k| std::env::var_os(k).is_some()),
            offscreen: None,
        })
    }

    fn make_expo_bg(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, hdr: &wgpu::TextureView, st: &wgpu::Buffer, pr: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("expo"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(hdr) },
                wgpu::BindGroupEntry { binding: 1, resource: st.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: pr.as_entire_binding() },
            ],
        })
    }

    /// (current exposure, average log2 luminance) read back from the GPU. Screenshot mode only (blocks).
    pub fn read_exposure(&self) -> (f32, f32) {
        let b = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("expo rb"), size: 16, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&self.expo_state, 0, &b, 0, 16);
        self.queue.submit([enc.finish()]);
        b.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let v = b.slice(..).get_mapped_range().map(|d| (f32::from_le_bytes(d[0..4].try_into().unwrap()), f32::from_le_bytes(d[12..16].try_into().unwrap()))).unwrap_or((0.0, 0.0));
        v
    }

    fn make_post_bg(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, ub: &wgpu::Buffer, hdr: &wgpu::TextureView, load: &wgpu::TextureView, samp: &wgpu::Sampler, expo: &wgpu::Buffer, bloom: &wgpu::TextureView, grading: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(hdr) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(load) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(samp) },
                wgpu::BindGroupEntry { binding: 4, resource: expo.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(bloom) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(grading) },
            ],
        })
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 || (w == self.width && h == self.height) {
            return;
        }
        self.width = w;
        self.height = h;
        if let Some((s, c)) = &mut self.surface {
            c.width = w;
            c.height = h;
            s.configure(&self.device, c);
        }
        self.rebuild_targets();
        self.backdrop = Some(crate::backdrop::Backdrop::new(&self.device, &self.queue, self.out_format, (w, h)));
    }

    /// Render targets (+ everything that samples them) for the current window size, render scale and MSAA.
    fn rebuild_targets(&mut self) {
        let [bw, bh] = self.game_screen();
        let (rw, rh) = scaled(bw, bh, self.settings.render_scale);
        (self.rw, self.rh) = (rw, rh);
        let (m, hd, d) = targets(&self.device, rw, rh, self.pipes.samples);
        self.msaa_view = m;
        self.hdr_view = hd;
        self.depth_view = d;
        self.highlight.resize(&self.device, HDR, rw, rh, self.pipes.samples, &self.depth_view, &self.expo_state);
        self.bloom.resize(&self.device, &self.queue, &self.hdr_view, &self.expo_state, rw, rh);
        self.post_bg = Self::make_post_bg(&self.device, &self.post_layout, &self.post_ub, &self.hdr_view, &self.load_view, &self.post_samp, &self.expo_state, &self.bloom.output, &self.grading);
        self.expo_bg = Self::make_expo_bg(&self.device, &self.expo_layout, &self.hdr_view, &self.expo_state, &self.expo_params);
    }

    /// Applies changed video settings live. `world` re-binds the frame resources (shadow maps, sampler) when a map is
    /// loaded. Texture quality is read by the loader, so it changes on the next map load only.
    pub fn apply_settings(&mut self, g: &GraphicsSettings, world: Option<&LoadedWorld>) {
        let old = std::mem::replace(&mut self.settings, g.clone());
        let mut rebind = false;
        let samples = if g.msaa >= 4 { MSAA } else { 1 };
        let pipes_changed = samples != self.pipes.samples;
        if pipes_changed {
            self.pipes = Pipes::new(&self.device, &self.world_sh, &self.world_pl, &self.sky_pl, &self.vm_pl, samples);
            if let Some(f) = &mut self.fx {
                f.rebuild(&self.device, HDR, DEPTH, samples);
            }
            if let Some(d) = &mut self.decals {
                d.rebuild(&self.device, HDR, DEPTH, samples);
            }
        }
        if pipes_changed || old.render_scale != g.render_scale || old.stretch != g.stretch {
            self.rebuild_targets();
        }
        if old.anisotropic != g.anisotropic {
            self.mat_samp = mat_sampler(&self.device, g.anisotropic);
            rebind = true;
        }
        if old.sun_shadows != g.sun_shadows {
            let (res, count) = g.sun_shadows.cascades();
            if res != self.csm.res {
                self.csm = Csm::new(&self.device, &self.mat_layout, &self.mat_samp, res, count);
                rebind = true;
            } else {
                self.csm.count = count;
                self.csm.invalidate();
            }
        }
        if old.vsync != g.vsync {
            let mode = present_mode(g.vsync, self.has_mailbox);
            if let Some((s, c)) = &mut self.surface {
                c.present_mode = mode;
                s.configure(&self.device, c);
            }
        }
        if let (true, Some(w)) = (rebind, world) {
            self.frame_bg = Some(self.make_frame_bg(w, &self.frame_ub, "frame"));
            self.sky_bg = Some(self.make_frame_bg(w, &self.sky_ub, "sky frame"));
        }
        log::info!("video settings: {:?}", self.settings);
    }

    /// Exposure the meter is overridden with (--exposure).
    fn fixed_exposure(&self) -> Option<f32> {
        self.forced_exposure
    }

    /// CBufExposureAdaptation + CBufLogLuminance values for this frame, filled the way S21's DoEnginePostProcessing
    /// (FUN_140961750) and its dispatch (FUN_1404bdf00) do it:
    ///   min = min_multiplier * mat_autoexposure_min, max = max_multiplier * mat_autoexposure_max (each also times a
    ///   script multiplier, 1 unless set); uncap -> min 0, max 100; max = max(min, max)
    ///   target_lum_max = max(target_lum_min, target_lum_max); compensation = mat_autoexposure_compensation (+ script bias)
    ///   c_adaptationRate = mat_autoexposure_speed, no dt; spot = clamp(ratio, 0.1, 1); max_luminance <= 0 -> no clamp
    /// Consumes the pending snap.
    fn expo_params(&self) -> [f32; 12] {
        let x = &self.expo;
        let (mut emin, mut emax) = if x.uncap { (0.0, 100.0) } else { (x.min_multiplier * x.min, x.max_multiplier * x.max) };
        emax = emax.max(emin);
        let mut rate = x.rate;
        let fixed = self.fixed_exposure();
        if let Some(f) = fixed {
            (emin, emax) = (f, f);
        }
        if self.instant_exposure || fixed.is_some() {
            rate = 1e6; // saturate() -> 1: the clamped target this frame
        }
        let snap = self.expo_snap.replace(false) && !self.instant_exposure && fixed.is_none();
        // the meter sums fixed point, so "no clamp" is capped where the sum still fits a u32
        let max_lum = if x.max_lum > 0.0 { x.max_lum.min(100.0) } else { 100.0 };
        [rate, x.target_min, x.target_max.max(x.target_min), x.compensation, emin, emax, x.spot.clamp(0.1, 1.0), max_lum, snap as u32 as f32, 0.0, 0.0, 0.0]
    }

    /// Forget the adapted exposure: the next frame takes the metered target (the engine's AutoExposureSnap()).
    pub fn snap_exposure(&self) {
        self.expo_snap.set(true);
    }

    /// Appends a material set to the bindless table and rebuilds its bind group; returns the set's first material index.
    pub fn register_materials(&mut self, set: &crate::loader::MatSet) -> u32 {
        let (base, tbase) = (self.mat_data.len() as u32, self.mat_views.len() as u32);
        self.mat_views.extend(set.views.iter().cloned());
        self.mat_data.extend(set.mats.iter().map(|m| {
            let mut m = *m;
            m.tix.iter_mut().flatten().for_each(|t| *t += tbase);
            m
        }));
        if self.mat_views.len() > MAX_MAT_TEXTURES as usize {
            log::error!("{} material textures, the table holds {MAX_MAT_TEXTURES}: raise MAX_MAT_TEXTURES", self.mat_views.len());
            self.mat_views.truncate(MAX_MAT_TEXTURES as usize);
        }
        use wgpu::util::DeviceExt;
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("materials"), contents: bytemuck::cast_slice(&self.mat_data), usage: wgpu::BufferUsages::STORAGE });
        let views: Vec<&wgpu::TextureView> = self.mat_views.iter().collect();
        self.mat_bg = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("materials"),
            layout: &self.mat_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureViewArray(&views) }],
        }));
        base
    }

    /// Loads a first-person model (viewmodel.rs) and adds its materials to the table.
    pub fn load_viewmodel(&mut self, root: &std::path::Path, dir: &std::path::Path) -> Result<crate::viewmodel::Viewmodel, String> {
        let mut vm = crate::viewmodel::Viewmodel::load(&self.device, &self.queue, &self.vm_layout, root, dir)?;
        if let Some(set) = vm.mats.take() {
            vm.mat_base = self.register_materials(&set);
        }
        Ok(vm)
    }

    /// Experimental ray-traced sun shadows on / off (console r_rtshadows): builds the acceleration structures the first
    /// time, then swaps the world shader and its pipelines for the ray query variant (rt_on.wgsl) or back.
    pub fn set_rt(&mut self, on: bool, world: Option<&LoadedWorld>) -> Result<(), String> {
        if on && self.rt.is_none() {
            let w = world.ok_or("no map loaded")?;
            self.rt = Some(crate::rt::build(&self.device, &self.queue, w)?);
        }
        self.world_sh = self.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("world"), source: wgpu::ShaderSource::Wgsl(world_source(on).into()) });
        let rtl = self.rt.as_ref().filter(|_| on).map(|r| &r.layout);
        self.world_pl = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("world"), bind_group_layouts: &[Some(&self.frame_layout), Some(&self.mat_layout), None, rtl], immediate_size: 4 });
        self.vm_pl = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("viewmodel"), bind_group_layouts: &[Some(&self.frame_layout), Some(&self.mat_layout), Some(&self.vm_layout), rtl], immediate_size: 4 });
        self.pipes = Pipes::new(&self.device, &self.world_sh, &self.world_pl, &self.sky_pl, &self.vm_pl, self.pipes.samples);
        self.rt_on = on;
        self.csm.invalidate();
        Ok(())
    }

    /// Call before loading viewmodels: the table restarts with the world's materials at index 0.
    pub fn set_world(&mut self, world: &LoadedWorld) {
        self.mat_views.clear();
        self.mat_data.clear();
        self.register_materials(&world.mats);
        // the level's env_exposure overrides the mat_autoexposure_* defaults, like the engine's exposure controller
        if let Some([speed, min, max, comp]) = world.exposure {
            self.expo.rate = speed;
            self.expo.min = min;
            self.expo.max = max;
            self.expo.compensation = comp;
        }
        self.expo_snap.set(true);
        self.frame_bg = Some(self.make_frame_bg(world, &self.frame_ub, "frame"));
        self.sky_bg = Some(self.make_frame_bg(world, &self.sky_ub, "sky frame"));
        self.csm.invalidate();
        if let Some(g) = &world.grading {
            self.grading = g.clone();
            self.grading_on = true;
            self.post_bg = Self::make_post_bg(&self.device, &self.post_layout, &self.post_ub, &self.hdr_view, &self.load_view, &self.post_samp, &self.expo_state, &self.bloom.output, &self.grading);
        }
    }

    fn make_frame_bg(&self, world: &LoadedWorld, ub: &wgpu::Buffer, label: &str) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&world.light.lm0) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&world.light.lm1) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&world.light.rtl_ids) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&world.light.rtl_vis) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&self.lm_samp) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.mat_samp) },
                wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&self.csm.array_view) },
                wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(&self.csm.cmp_samp) },
                wgpu::BindGroupEntry { binding: 9, resource: world.light.lights.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 10, resource: world.light.rtl_index.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 11, resource: world.light.probes.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 12, resource: wgpu::BindingResource::TextureView(&world.light.cubes) },
                wgpu::BindGroupEntry { binding: 13, resource: world.light.cube_ambient.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 14, resource: wgpu::BindingResource::TextureView(&world.sky_cube) },
            ],
        })
    }

    /// Per-frame work before drawing: re-renders the shadow cascades the camera has moved away from.
    pub fn prepare(&mut self, cam: &Camera, world: &LoadedWorld) -> u32 {
        // static props: LOD + fade + culling for this view, uploaded to the instance buffer's first region
        let lodp = self.lod_params(cam);
        self.last_lod.set(Some(lodp));
        if let (Some(props), Some(buf)) = (&world.props, &world.inst_buf) {
            let mut sel = self.prop_sel.borrow_mut();
            if self.lod.draw_props {
                let planes = frustum_planes(&self.view_proj(cam).0);
                props.select(&mut sel, &lodp, false, |c, r| crate::props::sphere_visible(&planes, c - cam.pos, r));
            } else {
                *sel = Default::default();
            }
            if !sel.data.is_empty() {
                self.queue.write_buffer(buf, 0, bytemuck::cast_slice(&sel.data));
            }
        }
        if self.csm.count == 0 {
            return 0;
        }
        // the cascades are cached until the camera moves away; ground loot (dynamic instances) changes under them
        // (spawn, pickup, drop, respawn): re-render them when it does, or items never cast shadows
        if let Some(props) = &world.props {
            let mut sig = props.dynamic.len() as u64;
            for i in &props.dynamic {
                for v in i.gpu.rows.iter().flatten() {
                    sig = sig.wrapping_mul(0x100000001B3).wrapping_add(v.to_bits() as u64);
                }
            }
            if sig != self.loot_shadow_sig {
                self.loot_shadow_sig = sig;
                self.csm.invalidate();
            }
        }
        let to_sun = Vec3::from(world.light.sun_dir).normalize_or(Vec3::Z);
        let Some(mbg) = &self.mat_bg else { return 0 };
        if self.rt_on {
            return 0;
        }
        self.csm.update(&self.device, &self.queue, cam.pos, to_sun, world, self.lod.draw_props.then_some(&lodp), mbg)
    }

    /// The resolution the game renders and lays the HUD out at: the stretched resolution if set (Apex-style stretch:
    /// drawn at that size, stretched to the window), else the window.
    pub fn game_screen(&self) -> [u32; 2] {
        match self.settings.stretch {
            [w, h] if w > 0 && h > 0 => [w, h],
            _ => [self.width.max(1), self.height.max(1)],
        }
    }

    /// Projection aspect: the game resolution's (the vertical FOV stays, a narrower stretch res sees less sideways).
    pub fn view_aspect(&self) -> f32 {
        let [w, h] = self.game_screen();
        w as f32 / h as f32
    }

    /// Camera-relative view-projection and vertical FOV (radians) for a camera.
    pub fn view_proj(&self, cam: &Camera) -> (Mat4, f32) {
        let aspect = self.view_aspect();
        let vfov = 2.0 * ((cam.fov.to_radians() * 0.5).tan() * 0.75).atan();
        let fwd = shared::sim::forward_vec(cam.yaw, cam.pitch);
        let view = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, fwd, Vec3::Z);
        let proj = glam::camera::rh::proj::directx::perspective_infinite_reverse(vfov, aspect, 4.0);
        (proj * view, vfov)
    }

    /// The first-person weapon's projection: its own FOV (horizontal at 4:3, like the player's), near plane 1 unit,
    /// camera-relative like the world's.
    pub fn viewmodel_clip(&self, cam: &Camera, fov_deg: f32) -> Mat4 {
        let aspect = self.view_aspect();
        let vfov = 2.0 * ((fov_deg.to_radians() * 0.5).tan() * 0.75).atan();
        let fwd = shared::sim::forward_vec(cam.yaw, cam.pitch);
        let view = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, fwd, Vec3::Z);
        glam::camera::rh::proj::directx::perspective_infinite_reverse(vfov, aspect, 1.0) * view
    }

    /// The static-prop LOD parameters for this view (props.rs): Model Detail and the console overrides.
    pub fn lod_params(&self, cam: &Camera) -> crate::props::LodParams {
        crate::props::LodParams {
            cam: cam.pos,
            switch_scale: self.lod.switch_scale.unwrap_or(self.settings.model_detail.lod_scale()).max(crate::props::MIN_SWITCH_SCALE),
            force: self.lod.force,
            fade_scale: self.lod.fade_scale,
            dress_level: self.lod.dress_level,
        }
    }

    pub fn frame_uniform(&self, cam: &Camera, world: &LoadedWorld, time: f32) -> FrameUniform {
        let aspect = self.view_aspect();
        let vfov = 2.0 * ((cam.fov.to_radians() * 0.5).tan() * 0.75).atan();
        let fwd = shared::sim::forward_vec(cam.yaw, cam.pitch);
        let view = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, fwd, Vec3::Z);
        let proj = glam::camera::rh::proj::directx::perspective_infinite_reverse(vfov, aspect, 4.0);
        let vp = proj * view;
        self.hl_proj.set([4.0, 1.0 / proj.x_axis.x, 1.0 / proj.y_axis.y]);
        let l = &world.light;
        let to_sun = Vec3::from(l.sun_dir).normalize_or(Vec3::Z);
        let fu = FrameUniform {
            view_proj: vp.to_cols_array_2d(),
            inv_view_proj: vp.inverse().to_cols_array_2d(),
            cam_pos: [cam.pos.x, cam.pos.y, cam.pos.z, 0.0],
            to_sun: [to_sun.x, to_sun.y, to_sun.z, 0.0],
            sun_color: [l.sun_color[0], l.sun_color[1], l.sun_color[2], 1.0],
            sky: [l.sky_color[0], l.sky_color[1], l.sky_color[2], 1.0],
            quality: [self.csm.res as f32, self.csm.count as f32, self.rt_on as u32 as f32, (self.pipes.samples == 1) as u32 as f32],
            fog: [0.0; 4],
            misc: [0.0, time, self.exposure, cam.debug],
            shadow_vp: self.csm.mats.map(|m| m.to_cols_array_2d()),
            cascade_r: csm::RADII,
            fog_k: world.fog.k,
        };
        self.sky_fu.set(sky_uniform(&fu, cam, world, vfov, aspect));
        fu
    }

    /// Draws one frame into `target` (the swapchain image or a screenshot texture).
    /// `world` = None draws the loading screen with `progress`.
    pub fn draw(&self, target: &wgpu::TextureView, world: Option<(&LoadedWorld, &FrameUniform)>, progress: f32) {
        std::thread::scope(|scope| self.draw_in(scope, target, world, progress));
    }

    /// One frame in two command buffers: the world pass's encoder is finished on a worker thread (wgpu validates and
    /// tracks every recorded command in `finish`, the biggest CPU cost of a frame) while this thread records and
    /// finishes the post chain (highlight, exposure, bloom, post); both are submitted in order.
    fn draw_in<'scope>(&self, scope: &'scope std::thread::Scope<'scope, '_>, target: &wgpu::TextureView, world: Option<(&LoadedWorld, &FrameUniform)>, progress: f32) {
        let mut world_job: Option<std::thread::ScopedJoinHandle<'scope, wgpu::CommandBuffer>> = None;
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let prof = self.prof.as_ref();
        if let Some(p) = prof {
            p.begin(&mut enc);
        }
        let mode = if let (Some((w, fu)), Some(fbg), Some(mbg)) = (world, &self.frame_bg, &self.mat_bg) {
            self.queue.write_buffer(&self.frame_ub, 0, bytemuck::bytes_of(fu));
            let (view, resolve, store) = match &self.msaa_view {
                Some(m) => (m, Some(&self.hdr_view), wgpu::StoreOp::Discard),
                None => (&self.hdr_view, None, wgpu::StoreOp::Store),
            };
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: resolve,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    // kept: the highlight composite reads it (retail primaryDepthBuffer)
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, fbg, &[]);
            // the material table, once per pass: wgpu re-tracks every texture in it on each set (~3000 here)
            rp.set_bind_group(1, mbg, &[]);
            let rt_bg = self.rt.as_ref().filter(|_| self.rt_on).map(|r| &r.group);
            if let Some(g) = rt_bg {
                rp.set_bind_group(3, g, &[]);
            }
            rp.set_pipeline(&self.pipes.sky);
            rp.draw(0..3, 0..1);
            rp.set_vertex_buffer(0, w.vbuf.slice(..));
            rp.set_vertex_buffer(1, w.probe_vbuf.slice(..));
            rp.set_index_buffer(w.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            let mut drawn = 0;
            // The 3D skybox (docs/skybox.md): its own camera and frame uniform, then depth back to the far
            // plane so the world draws over it, as Source's CSkyboxView does.
            if let (Some(sfu), Some(sbg)) = (self.sky_fu.get(), &self.sky_bg) {
                if !self.dbg[0] {
                    self.queue.write_buffer(&self.sky_ub, 0, bytemuck::bytes_of(&sfu));
                    rp.set_bind_group(0, sbg, &[]);
                    let planes = frustum_planes(&Mat4::from_cols_array_2d(&sfu.view_proj));
                    let cam = Vec3::new(sfu.cam_pos[0], sfu.cam_pos[1], sfu.cam_pos[2]);
                    let (mut cur_key, mut cur_mat) = (255u8, u32::MAX);
                    for d in w.draws.iter().filter(|d| d.skybox) {
                        if !visible(&planes, Vec3::from(d.min) - cam, Vec3::from(d.max) - cam) {
                            continue;
                        }
                        drawn += 1;
                        if d.pipe != cur_key {
                            cur_key = d.pipe;
                            rp.set_pipeline(&self.pipes.world[d.pipe as usize]);
                        }
                        if d.material != cur_mat {
                            cur_mat = d.material;
                            rp.set_immediates(0, &d.material.to_le_bytes());
                        }
                        rp.draw_indexed(d.first_index..d.first_index + d.index_count, 0, 0..1);
                    }
                    rp.set_pipeline(&self.pipes.depth_reset);
                    rp.draw(0..3, 0..1);
                    rp.set_bind_group(0, fbg, &[]);
                    if let Some(g) = rt_bg {
                        rp.set_bind_group(3, g, &[]);
                    }
                }
            }
            let [_, only_sky, no_skymat, no_blend] = self.dbg;
            let planes = frustum_planes(&Mat4::from_cols_array_2d(&fu.view_proj));
            let cam = Vec3::new(fu.cam_pos[0], fu.cam_pos[1], fu.cam_pos[2]);
            // chunk list order: opaque, alpha-tested, blended (front-to-back sorting measured no GPU gain on
            // 4096-unit chunks and cost CPU, 2026-11-18). Static props go in after the opaque + alpha-tested world
            // and again after the blended world for their own blended / sky meshes.
            let (mut cur_key, mut cur_mat, mut tris) = (255u8, u32::MAX, 0u64);
            let split = w.draws.iter().position(|d| d.key / 2 >= 2).unwrap_or(w.draws.len());
            let sel = self.prop_sel.borrow();
            let psplit = sel.split();
            let props_on = w.inst_buf.is_some() && !sel.draws.is_empty();
            let mut prop_draws = 0u32;
            let mut draw_props = |rp: &mut wgpu::RenderPass, range: std::ops::Range<usize>, cur_mat: &mut u32| {
                if !props_on || range.is_empty() {
                    return;
                }
                rp.set_vertex_buffer(1, w.inst_buf.as_ref().unwrap().slice(..));
                let mut cur = (255u8, false);
                for d in &sel.draws[range] {
                    if (d.pipe, d.fading) != cur {
                        cur = (d.pipe, d.fading);
                        let p = if d.fading { self.pipes.prop_fade[d.pipe as usize].as_ref().unwrap_or(&self.pipes.prop[d.pipe as usize]) } else { &self.pipes.prop[d.pipe as usize] };
                        rp.set_pipeline(p);
                    }
                    if d.material != *cur_mat {
                        *cur_mat = d.material;
                        rp.set_immediates(0, &d.material.to_le_bytes());
                    }
                    rp.draw_indexed(d.first_index..d.first_index + d.index_count, 0, d.inst_first..d.inst_first + d.inst_count);
                    prop_draws += 1;
                }
                rp.set_vertex_buffer(1, w.probe_vbuf.slice(..));
            };
            for (n, d) in w.draws.iter().enumerate() {
                if n == split {
                    draw_props(&mut rp, 0..psplit, &mut cur_mat);
                    cur_key = 255;
                }
                if d.skybox || only_sky || (d.key == 6 && no_skymat) || (d.key / 2 == 2 && no_blend) {
                    continue;
                }
                if !visible(&planes, Vec3::from(d.min) - cam, Vec3::from(d.max) - cam) {
                    continue;
                }
                drawn += 1;
                tris += d.index_count as u64 / 3;
                if d.pipe != cur_key {
                    cur_key = d.pipe;
                    rp.set_pipeline(&self.pipes.world[d.pipe as usize]);
                }
                if d.material != cur_mat {
                    cur_mat = d.material;
                    rp.set_immediates(0, &d.material.to_le_bytes());
                }
                rp.draw_indexed(d.first_index..d.first_index + d.index_count, 0, 0..1);
            }
            if split == w.draws.len() {
                draw_props(&mut rp, 0..psplit, &mut cur_mat);
            }
            draw_props(&mut rp, psplit..sel.draws.len(), &mut cur_mat);
            // bullet impact marks
            if !self.impacts.is_empty() {
                let n = self.impacts.len().min(MAX_IMPACTS);
                rp.set_vertex_buffer(0, self.impact_buf.slice(..));
                match &self.decals {
                    Some(d) => {
                        let inst: Vec<[f32; 12]> = self.impacts[..n].iter().map(|i| d.resolve(i)).collect();
                        self.queue.write_buffer(&self.impact_buf, 0, bytemuck::cast_slice(&inst));
                        rp.set_pipeline(&d.pipeline);
                        rp.set_bind_group(1, &d.group, &[]);
                    }
                    None => {
                        self.queue.write_buffer(&self.impact_buf, 0, bytemuck::cast_slice(&self.impacts[..n]));
                        rp.set_pipeline(&self.pipes.impacts);
                    }
                }
                rp.draw(0..6, 0..n as u32);
            }
            // weapon effects in the world (tracers, impact sparks and dust), under the gun
            if let Some(f) = &self.fx {
                f.draw_world(&self.queue, &mut rp, &self.fx_frame);
                rp.set_bind_group(0, fbg, &[]);
            }
            // the first-person weapon last, squeezed into the nearest 4% of the depth range (reverse-Z: 0.96..1) so
            // world geometry never cuts into it; world surfaces only reach that range closer than ~4 units
            if let (true, Some(vm)) = (self.vm_visible, if self.vm_proxy { &self.proxy_vm } else if self.vm_melee { &self.melee_vm } else { &self.viewmodel }) {
                let (rw, rh) = (self.rw, self.rh);
                rp.set_viewport(0.0, 0.0, rw as f32, rh as f32, 0.96, 1.0);
                rp.set_pipeline(&self.pipes.viewmodel);
                if let Some(g) = rt_bg {
                    rp.set_bind_group(3, g, &[]);
                }
                vm.draw(&mut rp, false);
                if vm.has_translucent() {
                    rp.set_pipeline(&self.pipes.viewmodel_blend);
                    vm.draw(&mut rp, true);
                }
                tris += vm.tris as u64;
                // first-person effects (muzzle flash, shells) in the same depth range, after the gun
                if let Some(f) = &self.fx {
                    f.draw_view(&self.queue, &mut rp, &self.fx_frame);
                }
            }
            tris += sel.tris;
            self.drawn_props.set((sel.instances, prop_draws));
            drop(sel);
            self.drawn.set(drawn);
            self.drawn_tris.set(tris);
            drop(rp);
            if let Some(p) = prof {
                p.mark(&mut enc, "world");
            }
            let world_enc = std::mem::replace(&mut enc, self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("post") }));
            world_job = Some(scope.spawn(move || world_enc.finish()));
            if let (Some(props), Some(ib)) = (&w.props, &w.inst_buf) {
                let sel = self.prop_sel.borrow();
                let frame = crate::highlight::Frame {
                    draws: &sel.hl_draws,
                    vbuf: &w.vbuf,
                    ibuf: &w.ibuf,
                    inst_buf: ib,
                    frame_bg: fbg,
                    settings: &props.highlight,
                    proj: self.hl_proj.get(),
                    time: fu.misc[1],
                    exposure: self.exposure,
                };
                self.highlight.encode(&mut enc, &self.queue, &frame, &self.hdr_view);
            }
            if let Some(p) = prof {
                p.mark(&mut enc, "highlight");
            }
            self.queue.write_buffer(&self.expo_params, 0, bytemuck::cast_slice(&self.expo_params()));
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("expo"), timestamp_writes: None });
            cp.set_bind_group(0, &self.expo_bg, &[]);
            cp.set_pipeline(&self.expo_lum);
            cp.dispatch_workgroups(128 / 16, 72 / 8, 1);
            cp.set_pipeline(&self.expo_adapt);
            cp.dispatch_workgroups(1, 1, 1);
            drop(cp);
            if let Some(p) = prof {
                p.mark(&mut enc, "expo");
            }
            self.bloom.encode(&mut enc);
            if let Some(p) = prof {
                p.mark(&mut enc, "bloom");
            }
            1.0
        } else {
            0.0
        };
        let pu = PostUniform {
            params: [mode, progress, self.exposure, if self.out_srgb { 1.0 } else { 0.0 }],
            screen: [self.width as f32, self.height as f32, self.load_size.0 as f32, self.load_size.1 as f32],
            bloom: [self.bloom_amount, if self.grading_on && !self.no_grading { 1.0 } else { 0.0 }, self.tonemap as f32, self.raw_post as u32 as f32],
        };
        self.queue.write_buffer(&self.post_ub, 0, bytemuck::bytes_of(&pu));
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.post_pipe);
            rp.set_bind_group(0, &self.post_bg, &[]);
            rp.draw(0..3, 0..1);
        }
        // the HUD's frosted backing: the same post pass into the quarter-res backdrop, then blurred
        if let (true, true, Some(bd)) = (self.hud_backdrop_on, mode > 0.5, &self.backdrop) {
            {
                let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("hud backdrop"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &bd.a_view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                rp.set_pipeline(&self.post_pipe);
                rp.set_bind_group(0, &self.post_bg, &[]);
                rp.draw(0..3, 0..1);
            }
            bd.blur(&mut enc);
        }
        if let Some(p) = prof {
            p.mark(&mut enc, "post");
            p.end(&mut enc);
        }
        let c0 = crate::bench::thread_cycles();
        let cb = enc.finish();
        let c1 = crate::bench::thread_cycles();
        let world_cb = world_job.map(|h| h.join().expect("world pass encoder thread panicked"));
        self.queue.submit(world_cb.into_iter().chain([cb]));
        let c2 = crate::bench::thread_cycles();
        let [f, s] = self.submit_ms.get();
        self.submit_ms.set([f + (c1 - c0) as f64, s + (c2 - c1) as f64]);
    }

    /// Draws to the window. Returns false if the frame was skipped.
    #[allow(dead_code)]
    pub fn present(&mut self, world: Option<(&LoadedWorld, &FrameUniform)>, progress: f32) -> bool {
        let Some((tex, view)) = self.begin_frame() else { return false };
        self.draw(&view, world, progress);
        self.queue.present(tex);
        true
    }

    /// The swapchain image for this frame (draw into the view, overlay UI, then `end_frame`).
    pub fn begin_frame(&mut self) -> Option<(wgpu::SurfaceTexture, wgpu::TextureView)> {
        let (s, c) = self.surface.as_ref()?;
        let tex = match s.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                s.configure(&self.device, c);
                t
            }
            _ => {
                s.configure(&self.device, c);
                return None;
            }
        };
        let view = tex.texture.create_view(&Default::default());
        Some((tex, view))
    }

    pub fn end_frame(&self, tex: wgpu::SurfaceTexture) {
        self.queue.present(tex);
    }

    /// (HDR target, depth, MSAA samples) of the world pass, for pipelines drawn inside it (fx.rs).
    pub fn frame_layout(&self) -> &wgpu::BindGroupLayout {
        &self.frame_layout
    }

    pub fn world_targets(&self) -> (wgpu::TextureFormat, wgpu::TextureFormat, u32) {
        (HDR, DEPTH, self.pipes.samples)
    }

    pub fn out_format(&self) -> wgpu::TextureFormat {
        self.out_format
    }

    /// Renders one frame offscreen and writes it as a PNG.
    pub fn screenshot(&self, path: &std::path::Path, world: Option<(&LoadedWorld, &FrameUniform)>, progress: f32) -> Result<(), String> {
        self.screenshot_with(path, world, progress, |_, _, _, _| {})
    }

    /// Like `screenshot`, with an overlay drawn over the finished frame (e.g. the menu).
    pub fn screenshot_with(&self, path: &std::path::Path, world: Option<(&LoadedWorld, &FrameUniform)>, progress: f32, mut overlay: impl FnMut(&wgpu::Device, &wgpu::Queue, &wgpu::TextureView, [u32; 2])) -> Result<(), String> {
        assert_eq!(self.out_format, SHOT_FORMAT, "screenshots need the offscreen (no window) Gfx");
        let (w, h) = (self.width, self.height);
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shot"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHOT_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        self.draw(&view, world, progress);
        overlay(&self.device, &self.queue, &view, [w, h]);
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("shot"), size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("poll: {e}"))?;
        let data = slice.get_mapped_range().map_err(|e| format!("map: {e:?}"))?;
        let mut img = image::RgbaImage::new(w, h);
        for y in 0..h {
            let src = &data[(y * row) as usize..(y * row + w * 4) as usize];
            img.as_mut()[(y * w * 4) as usize..((y + 1) * w * 4) as usize].copy_from_slice(src);
        }
        drop(data);
        buf.unmap();
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        img.save(path).map_err(|e| format!("save {}: {e}", path.display()))
    }

    /// Draws a frame offscreen without reading it back (GPU timing in screenshot mode).
    pub fn screenshot_free_draw(&mut self, world: &LoadedWorld, fu: &FrameUniform) {
        if self.offscreen.is_none() {
            self.offscreen = Some(
                self.device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("offscreen"),
                        size: wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: SHOT_FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&Default::default()),
            );
        }
        let v = self.offscreen.take().unwrap();
        self.draw(&v, Some((world, fu)), 1.0);
        self.offscreen = Some(v);
    }

    /// Screenshot-mode timing: re-renders each shadow cascade on its own, ms per cascade (what moving costs).
    pub fn time_cascades(&mut self, cam: Vec3, world: &LoadedWorld) -> Vec<f32> {
        let to_sun = Vec3::from(world.light.sun_dir).normalize_or(Vec3::Z);
        (0..self.csm.count)
            .map(|i| {
                self.wait();
                let t = std::time::Instant::now();
                for _ in 0..5 {
                    self.csm.invalidate_one(i);
                    let lod = self.last_lod.get();
                    if let Some(mbg) = &self.mat_bg {
                        self.csm.update(&self.device, &self.queue, cam, to_sun, world, lod.as_ref(), mbg);
                    }
                }
                self.wait();
                t.elapsed().as_secs_f32() * 1000.0 / 5.0
            })
            .collect()
    }

    /// Blocks until the GPU is idle (for timing in screenshot mode).
    /// Shadow cascades rendered since start (bench.rs).
    pub fn csm_renders(&self) -> u32 {
        self.csm.renders
    }

    pub fn wait(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

fn present_mode(vsync: bool, mailbox: bool) -> wgpu::PresentMode {
    if vsync {
        wgpu::PresentMode::AutoVsync
    } else if mailbox {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::AutoNoVsync
    }
}

/// The material sampler (trilinear, repeat) with Video > Anisotropic Filtering.
fn mat_sampler(device: &wgpu::Device, aniso: u32) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("material"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: aniso.clamp(1, 16) as u16,
        ..Default::default()
    })
}

/// Side + near planes of a reverse-Z infinite projection (camera-relative space).
pub(crate) fn frustum_planes(vp: &Mat4) -> [Vec4; 5] {
    let (r0, r1, r2, r3) = (vp.row(0), vp.row(1), vp.row(2), vp.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2]
}

fn visible(planes: &[Vec4; 5], mn: Vec3, mx: Vec3) -> bool {
    planes.iter().all(|p| {
        let v = Vec3::new(if p.x >= 0.0 { mx.x } else { mn.x }, if p.y >= 0.0 { mx.y } else { mn.y }, if p.z >= 0.0 { mx.z } else { mn.z });
        p.x * v.x + p.y * v.y + p.z * v.z + p.w >= 0.0
    })
}

/// The 3D skybox's frame: the S21 client (FUN_14099df90, docs/skybox.md) builds
/// `sky = origin + R^T * world / scale` from the sky camera's origin, angles (R = AngleMatrix) and
/// m_skybox3d.scale, so the eye moves 1/scale as far through the vista and turns by R^T.
/// None when the map has no 3D skybox.
fn sky_uniform(world_fu: &FrameUniform, cam: &Camera, world: &LoadedWorld, vfov: f32, aspect: f32) -> Option<FrameUniform> {
    let m = &world.meta;
    if m.sky_scale <= 0.0 || !world.draws.iter().any(|d| d.skybox) {
        return None;
    }
    let a = m.sky_angles;
    let r = glam::Mat3::from_euler(glam::EulerRot::ZYX, a[1].to_radians(), a[0].to_radians(), a[2].to_radians());
    let rt = if std::env::var_os("R5_SKY_ROT_R").is_some() { r } else { r.transpose() };
    let eye = Vec3::from(m.sky_camera) + rt * cam.pos / m.sky_scale;
    let fwd = rt * shared::sim::forward_vec(cam.yaw, cam.pitch);
    let view = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, fwd, rt * Vec3::Z);
    let proj = glam::camera::rh::proj::directx::perspective_infinite_reverse(vfov, aspect, 4.0 / m.sky_scale);
    let vp = proj * view;
    let mut fu = *world_fu;
    fu.view_proj = vp.to_cols_array_2d();
    fu.inv_view_proj = vp.inverse().to_cols_array_2d();
    fu.cam_pos = [eye.x, eye.y, eye.z, 0.0];
    // no cascades out here: every shadow lookup lands outside them and falls back to the baked term
    let out = Mat4::from_cols(Vec4::ZERO, Vec4::ZERO, Vec4::ZERO, Vec4::new(10.0, 10.0, 0.0, 1.0));
    fu.shadow_vp = [out.to_cols_array_2d(); csm::N];
    // the world's height fog is in world units; the skybox has its own (not ported yet)
    fu.fog_k[0][3] = 0.0;
    Some(fu)
}
