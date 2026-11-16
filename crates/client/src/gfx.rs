//! wgpu device, render targets, pipelines and the per-frame draw.
//! World pass: 4x MSAA HDR (Rgba16Float) + reverse-Z depth, sky first, then opaque / alpha-tested
//! (alpha-to-coverage) / blended draws. Post pass: ACES tonemap, or the loading screen.

use crate::bloom::Bloom;
use crate::csm::{self, Csm};
use crate::loader::{upload_rgba, LoadedWorld};
use glam::{Mat4, Vec3, Vec4};
use shared::assets::Config;
use std::sync::Arc;
use winit::window::Window;

pub const MSAA: u32 = 4;
const HDR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
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
    pub ground: [f32; 4],
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
    frame_layout: wgpu::BindGroupLayout,
    pub mat_layout: wgpu::BindGroupLayout,
    post_layout: wgpu::BindGroupLayout,
    world_pipes: Vec<wgpu::RenderPipeline>,
    sky_pipe: wgpu::RenderPipeline,
    post_pipe: wgpu::RenderPipeline,
    frame_ub: wgpu::Buffer,
    post_ub: wgpu::Buffer,
    lm_samp: wgpu::Sampler,
    mat_samp: wgpu::Sampler,
    post_samp: wgpu::Sampler,
    msaa_view: wgpu::TextureView,
    hdr_view: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    load_view: wgpu::TextureView,
    load_size: (u32, u32),
    post_bg: wgpu::BindGroup,
    frame_bg: Option<wgpu::BindGroup>,
    pub csm: Csm,
    bloom: Bloom,
    pub bloom_amount: f32,
    expo_state: wgpu::Buffer,
    expo_params: wgpu::Buffer,
    expo_layout: wgpu::BindGroupLayout,
    expo_bg: wgpu::BindGroup,
    expo_lum: wgpu::ComputePipeline,
    expo_adapt: wgpu::ComputePipeline,
    expo_last: std::cell::Cell<std::time::Instant>,
    /// snap exposure every frame (screenshot mode)
    pub instant_exposure: bool,
    /// auto-exposure settings (Apex mat_autoexposure_* equivalents)
    pub expo: shared::assets::AutoExposure,
    /// fixed exposure (calibration), bypasses the meter
    pub forced_exposure: Option<f32>,
    /// linear multiplier before the tonemap
    pub exposure: f32,
    pub drawn: std::cell::Cell<u32>,
    offscreen: Option<wgpu::TextureView>,
}

fn backends(name: &str) -> wgpu::Backends {
    match name.to_ascii_lowercase().as_str() {
        "vulkan" | "vk" => wgpu::Backends::VULKAN,
        "dx12" | "d3d12" => wgpu::Backends::DX12,
        "gl" | "opengl" => wgpu::Backends::GL,
        _ => wgpu::Backends::all(),
    }
}

fn targets(device: &wgpu::Device, w: u32, h: u32) -> (wgpu::TextureView, wgpu::TextureView, wgpu::TextureView) {
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
        mk(HDR, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT, "msaa"),
        mk(HDR, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, "hdr"),
        mk(DEPTH, MSAA, wgpu::TextureUsages::RENDER_ATTACHMENT, "depth"),
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
    pub fn new(window: Option<Arc<Window>>, width: u32, height: u32, cfg: &Config, loadscreen: Option<image::RgbaImage>) -> Result<Gfx, String> {
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

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("r5rust"),
            required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("request_device: {e}"))?;

        let (surface, out_format) = match surface {
            Some(s) => {
                let caps = s.get_capabilities(&adapter);
                let fmt = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
                let present_mode = if cfg.vsync { wgpu::PresentMode::AutoVsync } else if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) { wgpu::PresentMode::Mailbox } else { wgpu::PresentMode::AutoNoVsync };
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
                (Some((s, config)), fmt)
            }
            None => (None, SHOT_FORMAT),
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
        let mat_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mat"),
            entries: &[layout_entry(0, uniform_ty()), layout_entry(1, tex_ty()), layout_entry(2, tex_ty()), layout_entry(3, tex_ty()), layout_entry(4, tex_ty()), layout_entry(5, tex_ty()), layout_entry(6, tex_ty())],
        });
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[layout_entry(0, uniform_ty()), layout_entry(1, tex_ty()), layout_entry(2, tex_ty()), layout_entry(3, samp_ty()), layout_entry(4, storage_ty()), layout_entry(5, tex_ty())],
        });

        let world_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("world"), source: wgpu::ShaderSource::Wgsl(include_str!("world.wgsl").into()) });
        let post_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("post"), source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()) });

        let world_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("world"), bind_group_layouts: &[Some(&frame_layout), Some(&mat_layout)], immediate_size: 0 });
        let sky_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("sky"), bind_group_layouts: &[Some(&frame_layout)], immediate_size: 0 });
        let post_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("post"), bind_group_layouts: &[Some(&post_layout)], immediate_size: 0 });

        let vattrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm8x4, 2 => Float32x2, 3 => Unorm16x2, 4 => Unorm8x4];
        let vlayout = wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vattrs };
        let pattrs = wgpu::vertex_attr_array![5 => Uint32];
        let player = wgpu::VertexBufferLayout { array_stride: 4, step_mode: wgpu::VertexStepMode::Vertex, attributes: &pattrs };

        // key = alpha_mode * 2 + double_sided
        let mut world_pipes = Vec::new();
        for key in 0..6u8 {
            let alpha = key / 2;
            let double = key % 2 == 1;
            world_pipes.push(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("world"),
                layout: Some(&world_pl),
                vertex: wgpu::VertexState { module: &world_sh, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[Some(vlayout.clone()), Some(player.clone())] },
                primitive: wgpu::PrimitiveState { cull_mode: if double { None } else { Some(wgpu::Face::Back) }, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(alpha != 2),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: alpha == 1 },
                fragment: Some(wgpu::FragmentState {
                    module: &world_sh,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: HDR, blend: if alpha == 2 { Some(wgpu::BlendState::ALPHA_BLENDING) } else { None }, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            }));
        }
        // key 6: sky-shaded surfaces (unlit cube lookup + edge-faded emissive), opaque, no culling
        world_pipes.push(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky material"),
            layout: Some(&world_pl),
            vertex: wgpu::VertexState { module: &world_sh, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[Some(vlayout.clone()), Some(player.clone())] },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState { module: &world_sh, entry_point: Some("fs_skymat"), compilation_options: Default::default(), targets: &[Some(HDR.into())] }),
            multiview_mask: None,
            cache: None,
        }));
        let sky_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&sky_pl),
            vertex: wgpu::VertexState { module: &world_sh, entry_point: Some("vs_sky"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: MSAA, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState { module: &world_sh, entry_point: Some("fs_sky"), compilation_options: Default::default(), targets: &[Some(HDR.into())] }),
            multiview_mask: None,
            cache: None,
        });
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
        let mat_samp = device.create_sampler(&lin(wgpu::AddressMode::Repeat, 16, wgpu::MipmapFilterMode::Linear));
        let post_samp = device.create_sampler(&lin(wgpu::AddressMode::ClampToEdge, 1, wgpu::MipmapFilterMode::Nearest));

        let csm = Csm::new(&device, &mat_layout, &mat_samp);

        let ls = loadscreen.unwrap_or_else(|| image::RgbaImage::from_pixel(1, 1, image::Rgba([10, 10, 12, 255])));
        let load_size = ls.dimensions();
        let load_view = upload_rgba(&device, &queue, &[ls], wgpu::TextureFormat::Rgba8UnormSrgb, "loadscreen").create_view(&Default::default());

        let (msaa_view, hdr_view, depth_view) = targets(&device, width.max(1), height.max(1));
        let expo_state = device.create_buffer(&wgpu::BufferDescriptor { label: Some("expo"), size: 16, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let expo_params = device.create_buffer(&wgpu::BufferDescriptor { label: Some("expo params"), size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
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
        bloom.resize(&device, &queue, &hdr_view, &expo_state, width.max(1), height.max(1));
        let post_bg = Self::make_post_bg(&device, &post_layout, &post_ub, &hdr_view, &load_view, &post_samp, &expo_state, &bloom.output);

        Ok(Gfx {
            device,
            queue,
            adapter_name,
            surface,
            out_format,
            out_srgb,
            width: width.max(1),
            height: height.max(1),
            frame_layout,
            mat_layout,
            post_layout,
            world_pipes,
            sky_pipe,
            post_pipe,
            frame_ub,
            post_ub,
            lm_samp,
            mat_samp,
            post_samp,
            msaa_view,
            hdr_view,
            depth_view,
            load_view,
            load_size,
            post_bg,
            frame_bg: None,
            csm,
            bloom,
            bloom_amount: cfg.bloom_amount,
            expo_state,
            expo_params,
            expo_layout,
            expo_bg,
            expo_lum,
            expo_adapt,
            expo_last: std::cell::Cell::new(std::time::Instant::now()),
            instant_exposure: false,
            expo: cfg.auto_exposure.clone(),
            forced_exposure: None,
            exposure: cfg.exposure,
            drawn: std::cell::Cell::new(0),
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

    fn make_post_bg(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, ub: &wgpu::Buffer, hdr: &wgpu::TextureView, load: &wgpu::TextureView, samp: &wgpu::Sampler, expo: &wgpu::Buffer, bloom: &wgpu::TextureView) -> wgpu::BindGroup {
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
        let (m, hd, d) = targets(&self.device, w, h);
        self.msaa_view = m;
        self.hdr_view = hd;
        self.depth_view = d;
        self.bloom.resize(&self.device, &self.queue, &self.hdr_view, &self.expo_state, w, h);
        self.post_bg = Self::make_post_bg(&self.device, &self.post_layout, &self.post_ub, &self.hdr_view, &self.load_view, &self.post_samp, &self.expo_state, &self.bloom.output);
        self.expo_bg = Self::make_expo_bg(&self.device, &self.expo_layout, &self.hdr_view, &self.expo_state, &self.expo_params);
    }

    pub fn set_world(&mut self, world: &LoadedWorld) {
        self.frame_bg = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.frame_ub.as_entire_binding() },
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
        }));
        self.csm.invalidate();
    }

    /// Per-frame work before drawing: re-renders the shadow cascades the camera has moved away from.
    pub fn prepare(&mut self, cam: &Camera, world: &LoadedWorld) -> u32 {
        let to_sun = Vec3::from(world.light.sun_dir).normalize_or(Vec3::Z);
        self.csm.update(&self.device, &self.queue, cam.pos, to_sun, world)
    }

    pub fn frame_uniform(&self, cam: &Camera, world: &LoadedWorld, time: f32) -> FrameUniform {
        let aspect = self.width as f32 / self.height as f32;
        let vfov = 2.0 * ((cam.fov.to_radians() * 0.5).tan() * 0.75).atan();
        let fwd = shared::sim::forward_vec(cam.yaw, cam.pitch);
        let view = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, fwd, Vec3::Z);
        let proj = glam::camera::rh::proj::directx::perspective_infinite_reverse(vfov, aspect, 4.0);
        let vp = proj * view;
        let l = &world.light;
        let to_sun = Vec3::from(l.sun_dir).normalize_or(Vec3::Z);
        FrameUniform {
            view_proj: vp.to_cols_array_2d(),
            inv_view_proj: vp.inverse().to_cols_array_2d(),
            cam_pos: [cam.pos.x, cam.pos.y, cam.pos.z, 0.0],
            to_sun: [to_sun.x, to_sun.y, to_sun.z, 0.0],
            sun_color: [l.sun_color[0], l.sun_color[1], l.sun_color[2], 1.0],
            sky: [l.sky_color[0], l.sky_color[1], l.sky_color[2], 1.0],
            ground: [0.0; 4],
            fog: [0.0; 4],
            misc: [0.0, time, self.exposure, cam.debug],
            shadow_vp: self.csm.mats.map(|m| m.to_cols_array_2d()),
            cascade_r: csm::RADII,
            fog_k: world.fog.k,
        }
    }

    /// Draws one frame into `target` (the swapchain image or a screenshot texture).
    /// `world` = None draws the loading screen with `progress`.
    pub fn draw(&self, target: &wgpu::TextureView, world: Option<(&LoadedWorld, &FrameUniform)>, progress: f32) {
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let mode = if let (Some((w, fu)), Some(fbg)) = (world, &self.frame_bg) {
            self.queue.write_buffer(&self.frame_ub, 0, bytemuck::bytes_of(fu));
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&self.hdr_view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, fbg, &[]);
            rp.set_pipeline(&self.sky_pipe);
            rp.draw(0..3, 0..1);
            rp.set_vertex_buffer(0, w.vbuf.slice(..));
            rp.set_vertex_buffer(1, w.probe_vbuf.slice(..));
            rp.set_index_buffer(w.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            let mut cur_key = 255u8;
            let planes = frustum_planes(&Mat4::from_cols_array_2d(&fu.view_proj));
            let cam = Vec3::new(fu.cam_pos[0], fu.cam_pos[1], fu.cam_pos[2]);
            let mut drawn = 0;
            for d in &w.draws {
                if !visible(&planes, Vec3::from(d.min) - cam, Vec3::from(d.max) - cam) || (d.key == 6 && std::env::var_os("R5_NOSKYMAT").is_some()) {
                    continue;
                }
                drawn += 1;
                if d.key != cur_key {
                    cur_key = d.key;
                    rp.set_pipeline(&self.world_pipes[d.key as usize]);
                }
                rp.set_bind_group(1, &w.mat_groups[d.material as usize], &[]);
                rp.draw_indexed(d.first_index..d.first_index + d.index_count, 0, 0..1);
            }
            self.drawn.set(drawn);
            drop(rp);
            let now = std::time::Instant::now();
            let dt = (now - self.expo_last.get()).as_secs_f32().min(0.5);
            self.expo_last.set(now);
            let x = &self.expo;
            let rate = if self.instant_exposure || self.forced_exposure.is_some() { 1e6 } else { x.rate * dt };
            let (emin, emax) = match self.forced_exposure { Some(f) => (f, f), None => (x.min, x.max) };
            self.queue.write_buffer(&self.expo_params, 0, bytemuck::cast_slice(&[rate, x.target_min, x.target_max, x.compensation, emin, emax, x.spot, x.max_lum]));
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("expo"), timestamp_writes: None });
            cp.set_bind_group(0, &self.expo_bg, &[]);
            cp.set_pipeline(&self.expo_lum);
            cp.dispatch_workgroups(128 / 16, 72 / 8, 1);
            cp.set_pipeline(&self.expo_adapt);
            cp.dispatch_workgroups(1, 1, 1);
            drop(cp);
            self.bloom.encode(&mut enc);
            1.0
        } else {
            0.0
        };
        let pu = PostUniform {
            params: [mode, progress, self.exposure, if self.out_srgb { 1.0 } else { 0.0 }],
            screen: [self.width as f32, self.height as f32, self.load_size.0 as f32, self.load_size.1 as f32],
            bloom: [self.bloom_amount, 0.0, 0.0, 0.0],
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
        self.queue.submit([enc.finish()]);
    }

    /// Draws to the window. Returns false if the frame was skipped.
    pub fn present(&mut self, world: Option<(&LoadedWorld, &FrameUniform)>, progress: f32) -> bool {
        let Some((s, c)) = &self.surface else { return false };
        let tex = match s.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                s.configure(&self.device, c);
                t
            }
            _ => {
                s.configure(&self.device, c);
                return false;
            }
        };
        let view = tex.texture.create_view(&Default::default());
        self.draw(&view, world, progress);
        self.queue.present(tex);
        true
    }

    /// Renders one frame offscreen and writes it as a PNG.
    pub fn screenshot(&self, path: &std::path::Path, world: Option<(&LoadedWorld, &FrameUniform)>, progress: f32) -> Result<(), String> {
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
        self.draw(&tex.create_view(&Default::default()), world, progress);
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

    /// Blocks until the GPU is idle (for timing in screenshot mode).
    pub fn wait(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }
}

/// Side + near planes of a reverse-Z infinite projection (camera-relative space).
fn frustum_planes(vp: &Mat4) -> [Vec4; 5] {
    let (r0, r1, r2, r3) = (vp.row(0), vp.row(1), vp.row(2), vp.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2]
}

fn visible(planes: &[Vec4; 5], mn: Vec3, mx: Vec3) -> bool {
    planes.iter().all(|p| {
        let v = Vec3::new(if p.x >= 0.0 { mx.x } else { mn.x }, if p.y >= 0.0 { mx.y } else { mn.y }, if p.z >= 0.0 { mx.z } else { mn.z });
        p.x * v.x + p.y * v.y + p.z * v.z + p.w >= 0.0
    })
}
