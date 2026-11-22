//! Retail Apex's object highlight (the rarity outline on ground loot), ported from the S30 shaders
//! (docs/highlight.md; shaders in highlight.wgsl + world.wgsl vs/fs_loot_hl):
//!   ids -> horizontal gather -> vertical gather -> composite + apply into the HDR scene (before bloom).
//! The per-slot settings (`Setting`, retail SBufHighlightSettings) come from the loot (loot.rs `ground_instances`),
//! one slot per highlighted entity (1..255; 0 = none), the instance's `InstGpu.pad[0]` holding its slot.

use crate::props::SelDraw;

/// The highlight id buffer: slot / 255 (retail idBuffer, R8)
const ID_FMT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// The highlight objects' own depth (retail secondaryDepthBuffer), reversed-Z, cleared to 0
const DEPTH_FMT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const GATHER_H_FMT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
const SIL_FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rg8Unorm;
pub const MAX_SLOTS: usize = 256;

/// One slot's settings (retail SBufHighlightSettings, stride 48).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Setting {
    pub p0: [f32; 4],
    pub p1: [f32; 4],
    /// bits 0-7 flags, 8-19 inside function, 20-31 outline function
    pub packed: u32,
    pub inside_op: f32,
    pub outline_op: f32,
    pub radius: f32,
}

impl Setting {
    pub fn new(p0: [f32; 3], p1x: f32, inside: u32, outline: u32, radius: f32) -> Setting {
        Setting { p0: [p0[0], p0[1], p0[2], 0.0], p1: [p1x, 0.0, 0.0, 0.0], packed: (inside & 0xFFF) << 8 | (outline & 0xFFF) << 20, inside_op: 1.0, outline_op: 1.0, radius }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    size: [f32; 4],
    cam: [f32; 4],
    misc: [f32; 4],
}

/// Per-frame inputs from the renderer.
pub struct Frame<'a> {
    pub draws: &'a [SelDraw],
    pub vbuf: &'a wgpu::Buffer,
    pub ibuf: &'a wgpu::Buffer,
    pub inst_buf: &'a wgpu::Buffer,
    pub frame_bg: &'a wgpu::BindGroup,
    pub settings: &'a [Setting],
    /// near plane, 1 / proj[0][0], 1 / proj[1][1]
    pub proj: [f32; 3],
    pub time: f32,
    pub exposure: f32,
}

struct Targets {
    size: (u32, u32),
    id: wgpu::TextureView,
    depth: wgpu::TextureView,
    gather_h: wgpu::TextureView,
    sil: wgpu::TextureView,
    bg_h: wgpu::BindGroup,
    bg_v: wgpu::BindGroup,
    bg_apply: wgpu::BindGroup,
}

pub struct Highlight {
    id_pipe: wgpu::RenderPipeline,
    gather_h: wgpu::RenderPipeline,
    gather_v: wgpu::RenderPipeline,
    apply: wgpu::RenderPipeline,
    samples: u32,
    uniform: wgpu::Buffer,
    settings: wgpu::Buffer,
    t: Option<Targets>,
}

fn shader_src(samples: u32) -> String {
    let ty = if samples > 1 { "texture_depth_multisampled_2d" } else { "texture_depth_2d" };
    include_str!("highlight.wgsl").replace("PRIMARY_DEPTH_TYPE", ty).replace("PRIMARY_DEPTH_LOAD(src)", "textureLoad(primary_depth, src, 0)")
}

impl Highlight {
    /// `world_sh` / `frame_pl`: the world shader and the frame-only pipeline layout (the id pass shares vs_prop's
    /// transform). `hdr` = the scene colour format the apply pass blends into. `samples` = the world pass MSAA (the
    /// main depth the composite reads).
    pub fn new(device: &wgpu::Device, world_sh: &wgpu::ShaderModule, frame_pl: &wgpu::PipelineLayout, hdr: wgpu::TextureFormat, samples: u32) -> Highlight {
        let vlayout = wgpu::VertexBufferLayout {
            array_stride: 32,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 }],
        };
        let ilayout = wgpu::VertexBufferLayout {
            array_stride: crate::props::INST_STRIDE,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 6 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 7 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 8 },
                // InstGpu.pad[0] = the slot
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 56, shader_location: 10 },
            ],
        };
        let id_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("highlight ids"),
            layout: Some(frame_pl),
            vertex: wgpu::VertexState { module: world_sh, entry_point: Some("vs_loot_hl"), compilation_options: Default::default(), buffers: &[Some(vlayout), Some(ilayout)] },
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FMT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: world_sh,
                entry_point: Some("fs_loot_hl"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: ID_FMT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let (gather_h, gather_v, apply) = Self::fullscreen(device, hdr, samples);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor { label: Some("highlight uniform"), size: std::mem::size_of::<Uniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let settings = device.create_buffer(&wgpu::BufferDescriptor { label: Some("highlight settings"), size: (MAX_SLOTS * std::mem::size_of::<Setting>()) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Highlight { id_pipe, gather_h, gather_v, apply, samples, uniform, settings, t: None }
    }

    fn fullscreen(device: &wgpu::Device, hdr: wgpu::TextureFormat, samples: u32) -> (wgpu::RenderPipeline, wgpu::RenderPipeline, wgpu::RenderPipeline) {
        let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("highlight"), source: wgpu::ShaderSource::Wgsl(shader_src(samples).into()) });
        let mk = |fs: &str, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: None,
                vertex: wgpu::VertexState { module: &sh, entry_point: Some("vs_full"), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState { module: &sh, entry_point: Some(fs), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })] }),
                multiview_mask: None,
                cache: None,
            })
        };
        // premultiplied: scene * (1 - a) + rgb (outlines write a = 0: purely additive); the scene's alpha is kept
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        };
        (mk("fs_gather_h", GATHER_H_FMT, None), mk("fs_gather_v", SIL_FMT, None), mk("fs_apply", hdr, Some(blend)))
    }

    /// (Re)create the targets for the render size; `main_depth` = the world pass depth (sampled by the composite),
    /// `expo` = the auto-exposure state buffer. Rebuilds the shaders when the world pass MSAA changed.
    pub fn resize(&mut self, device: &wgpu::Device, hdr: wgpu::TextureFormat, w: u32, h: u32, samples: u32, main_depth: &wgpu::TextureView, expo: &wgpu::Buffer) {
        if samples != self.samples {
            (self.gather_h, self.gather_v, self.apply) = Self::fullscreen(device, hdr, samples);
            self.samples = samples;
        }
        let mk = |format, usage, label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let id = mk(ID_FMT, rt, "highlight ids");
        let depth = mk(DEPTH_FMT, rt, "highlight depth");
        let gather_h = mk(GATHER_H_FMT, rt, "highlight gather h");
        let sil = mk(SIL_FMT, rt, "highlight silhouette");
        let uni = wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() };
        let set = wgpu::BindGroupEntry { binding: 5, resource: self.settings.as_entire_binding() };
        let bg_h = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("highlight gather h"),
            layout: &self.gather_h.get_bind_group_layout(0),
            entries: &[uni.clone(), tex(1, &id), tex(2, &depth), set.clone()],
        });
        let bg_v = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("highlight gather v"),
            layout: &self.gather_v.get_bind_group_layout(0),
            entries: &[uni.clone(), tex(1, &id), tex(2, &depth), tex(3, &gather_h), set.clone()],
        });
        let bg_apply = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("highlight apply"),
            layout: &self.apply.get_bind_group_layout(0),
            entries: &[uni, tex(1, &id), tex(2, &depth), tex(4, &sil), set, tex(6, main_depth), wgpu::BindGroupEntry { binding: 7, resource: expo.as_entire_binding() }],
        });
        self.t = Some(Targets { size: (w, h), id, depth, gather_h, sil, bg_h, bg_v, bg_apply });
    }

    /// The four passes, after the world pass (its depth stored) and before bloom / exposure. Draws nothing when no
    /// loot is highlighted.
    pub fn encode(&self, enc: &mut wgpu::CommandEncoder, queue: &wgpu::Queue, f: &Frame, scene: &wgpu::TextureView) {
        let Some(t) = &self.t else { return };
        if f.draws.is_empty() || f.settings.len() < 2 {
            return;
        }
        let n = f.settings.len().min(MAX_SLOTS);
        queue.write_buffer(&self.settings, 0, bytemuck::cast_slice(&f.settings[..n]));
        // gather taps: the largest radius in use (retail ships N = 0..8)
        let taps = f.settings[1..n].iter().map(|s| s.radius.ceil() as i32).max().unwrap_or(1).clamp(0, 8);
        let (w, h) = (t.size.0 as f32, t.size.1 as f32);
        let u = Uniform { size: [w, h, 2.0 / w, 2.0 / h], cam: [f.proj[0], f.proj[1], f.proj[2], f.time], misc: [f.exposure, (n - 1) as f32, taps as f32, 0.0] };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));

        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("highlight ids"),
                color_attachments: &[Some(color(&t.id, true))],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.id_pipe);
            rp.set_bind_group(0, f.frame_bg, &[]);
            rp.set_vertex_buffer(0, f.vbuf.slice(..));
            rp.set_vertex_buffer(1, f.inst_buf.slice(..));
            rp.set_index_buffer(f.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for d in f.draws {
                rp.draw_indexed(d.first_index..d.first_index + d.index_count, 0, d.inst_first..d.inst_first + d.inst_count);
            }
        }
        for (pipe, bg, target, clear, label) in [
            (&self.gather_h, &t.bg_h, &t.gather_h, true, "highlight gather h"),
            (&self.gather_v, &t.bg_v, &t.sil, true, "highlight gather v"),
            (&self.apply, &t.bg_apply, scene, false, "highlight apply"),
        ] {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(color(target, clear))],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(pipe);
            rp.set_bind_group(0, bg, &[]);
            rp.draw(0..3, 0..1);
        }
    }
}

fn tex(binding: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(v) }
}

fn color(view: &wgpu::TextureView, clear: bool) -> wgpu::RenderPassColorAttachment<'_> {
    wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations { load: if clear { wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT) } else { wgpu::LoadOp::Load }, store: wgpu::StoreOp::Store },
    }
}
