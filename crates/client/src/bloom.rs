//! Bloom chain: prefilter (Apex soft knee on exposed luminance, stays unexposed) into half res, 13-tap downsamples,
//! tent upsamples accumulated back up. The post pass adds mip 0 to the scene before the exposure multiply.

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const LEVELS: u32 = 6;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    texel: [f32; 2],
    cutoff: f32,
    clamp: f32,
    weight: f32,
    mode: f32,
    pad: [f32; 2],
}

struct Pass {
    bg: wgpu::BindGroup,
    target: usize,
    pipe: u8, // 0 prefilter, 1 down, 2 up
}

pub struct Bloom {
    layout: wgpu::BindGroupLayout,
    prefilter: wgpu::RenderPipeline,
    down: wgpu::RenderPipeline,
    up: wgpu::RenderPipeline,
    samp: wgpu::Sampler,
    views: Vec<wgpu::TextureView>,
    params: Vec<wgpu::Buffer>,
    passes: Vec<Pass>,
    /// mip 0 = the final bloom, half resolution
    pub output: wgpu::TextureView,
    pub cutoff: f32,
    pub clamp: f32,
}

impl Bloom {
    pub fn new(device: &wgpu::Device, cutoff: f32, clamp: f32) -> Self {
        let fs = wgpu::ShaderStages::FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: fs, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: fs, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: fs, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: fs, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("bloom"), source: wgpu::ShaderSource::Wgsl(include_str!("bloom.wgsl").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("bloom"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let mk = |entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &sh, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState { module: &sh, entry_point: Some(entry), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend, write_mask: wgpu::ColorWrites::ALL })] }),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent::REPLACE };
        let samp = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let dummy = device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: FORMAT, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] });
        Bloom {
            layout,
            prefilter: mk("fs_prefilter", None),
            down: mk("fs_down13", None),
            up: mk("fs_up", Some(add)),
            samp,
            views: Vec::new(),
            params: Vec::new(),
            passes: Vec::new(),
            output: dummy.create_view(&Default::default()),
            cutoff,
            clamp,
        }
    }

    /// (Re)builds the chain for a `w x h` HDR source.
    pub fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, hdr: &wgpu::TextureView, expo: &wgpu::Buffer, w: u32, h: u32) {
        let (w0, h0) = ((w / 2).max(1), (h / 2).max(1));
        let levels = LEVELS.min(32 - w0.min(h0).leading_zeros()).max(1);
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bloom"),
            size: wgpu::Extent3d { width: w0, height: h0, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.views = (0..levels).map(|m| tex.create_view(&wgpu::TextureViewDescriptor { base_mip_level: m, mip_level_count: Some(1), ..Default::default() })).collect();
        self.output = tex.create_view(&wgpu::TextureViewDescriptor { base_mip_level: 0, mip_level_count: Some(1), ..Default::default() });
        let size = |m: u32| ((w0 >> m).max(1) as f32, (h0 >> m).max(1) as f32);
        self.params.clear();
        self.passes.clear();
        let add = |src: &wgpu::TextureView, texel: [f32; 2], weight: f32, target: usize, pipe: u8, s: &mut Self| {
            let p = Params { texel, cutoff: s.cutoff, clamp: s.clamp, weight, mode: 0.0, pad: [0.0; 2] };
            let b = device.create_buffer(&wgpu::BufferDescriptor { label: Some("bloom params"), size: 32, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            queue.write_buffer(&b, 0, bytemuck::bytes_of(&p));
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom"),
                layout: &s.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&s.samp) },
                    wgpu::BindGroupEntry { binding: 2, resource: expo.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: b.as_entire_binding() },
                ],
            });
            s.params.push(b);
            s.passes.push(Pass { bg, target, pipe });
        };
        let views = self.views.clone();
        add(hdr, [1.0 / w as f32, 1.0 / h as f32], 1.0, 0, 0, self);
        for m in 1..levels as usize {
            let (sw, shh) = size(m as u32 - 1);
            add(&views[m - 1], [1.0 / sw, 1.0 / shh], 1.0, m, 1, self);
        }
        for m in (0..levels as usize - 1).rev() {
            let (sw, shh) = size(m as u32 + 1);
            add(&views[m + 1], [1.0 / sw, 1.0 / shh], 1.0, m, 2, self);
        }
    }

    pub fn encode(&self, enc: &mut wgpu::CommandEncoder) {
        for p in &self.passes {
            let load = if p.pipe == 2 { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) };
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &self.views[p.target], depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(match p.pipe {
                0 => &self.prefilter,
                1 => &self.down,
                _ => &self.up,
            });
            rp.set_bind_group(0, &p.bg, &[]);
            rp.draw(0..3, 0..1);
        }
    }
}
