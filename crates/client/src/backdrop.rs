//! The blurred frame behind the HUD: the engine's RUI "code texture -4" (the weapon panel's frosted plate samples it).
//! The finished frame (the post pass, run again into a quarter-resolution target) blurred with a separable gaussian of
//! 10 screen pixels at 1080p (apex-content-kit public/js/rui.js BLUR_PX), in linear light (sRGB textures).

const SHADER: &str = r#"
struct B { dir: vec4<f32> };   // xy = one texel step along the blur, z = sigma in texels
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> b: B;
struct VO { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> VO {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VO;
    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}
@fragment fn fs(i: VO) -> @location(0) vec4<f32> {
    var s = vec4<f32>(0.0);
    var wsum = 0.0;
    let sig = max(b.dir.z, 0.5);
    for (var k = -12; k <= 12; k++) {
        let x = f32(k);
        let w = exp(-x * x / (2.0 * sig * sig));
        s += textureSampleLevel(src, samp, i.uv + b.dir.xy * x, 0.0) * w;
        wsum += w;
    }
    return s / wsum;
}
"#;

pub struct Backdrop {
    pub size: (u32, u32),
    a: wgpu::Texture,
    pub a_view: wgpu::TextureView,
    b_view: wgpu::TextureView,
    pipe: wgpu::RenderPipeline,
    groups: [wgpu::BindGroup; 2],
}

impl Backdrop {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, screen: (u32, u32)) -> Backdrop {
        let size = ((screen.0 / 4).max(1), (screen.1 / 4).max(1));
        let mk = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        let (a, b) = (mk("hud backdrop a"), mk("hud backdrop b"));
        let (a_view, b_view) = (a.create_view(&Default::default()), b.create_view(&Default::default()));
        let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("hud blur"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("hud blur"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud blur"),
            layout: Some(&pl),
            vertex: wgpu::VertexState { module: &sh, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState { module: &sh, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(format.into())] }),
            multiview_mask: None,
            cache: None,
        });
        let samp = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("hud blur"), mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        // 10 screen px at 1080p, in quarter-res texels
        let sigma = 10.0 * screen.1 as f32 / 1080.0 / 4.0;
        use wgpu::util::DeviceExt;
        let ub = |dir: [f32; 4]| device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("hud blur"), contents: bytemuck::cast_slice(&dir), usage: wgpu::BufferUsages::UNIFORM });
        let ux = ub([1.0 / size.0 as f32, 0.0, sigma, 0.0]);
        let uy = ub([0.0, 1.0 / size.1 as f32, sigma, 0.0]);
        let group = |src: &wgpu::TextureView, u: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hud blur"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&samp) },
                    wgpu::BindGroupEntry { binding: 2, resource: u.as_entire_binding() },
                ],
            })
        };
        let groups = [group(&a_view, &ux), group(&b_view, &uy)];
        let _ = queue;
        Backdrop { size, a, a_view, b_view, pipe, groups }
    }

    /// The frame (drawn into `a` by the caller) blurred: a -> b along x, b -> a along y.
    pub fn blur(&self, enc: &mut wgpu::CommandEncoder) {
        for (target, g) in [(&self.b_view, &self.groups[0]), (&self.a_view, &self.groups[1])] {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hud blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.pipe);
            rp.set_bind_group(0, g, &[]);
            rp.draw(0..3, 0..1);
        }
        let _ = &self.a;
    }
}
