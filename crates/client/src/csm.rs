//! Sun cascaded shadow maps. The world and the sun are static, so a cascade is only re-rendered when the
//! camera has moved far enough from where it was last rendered (and its centre is snapped to texels).
//! Between re-renders the matrix stays fixed: no shimmering, and ~zero cost on most frames.

use crate::loader::LoadedWorld;
use glam::{Mat4, Vec3, Vec4Swizzles};

pub const RES: u32 = 2048;
pub const N: usize = 4;
/// half-width of each cascade, Source units (inches)
pub const RADII: [f32; N] = [600.0, 2000.0, 6500.0, 20000.0];
/// re-render when the camera is this fraction of the radius away from the cascade centre
const MOVE_FRAC: f32 = 0.2;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const DEPTH_RANGE: f32 = 60000.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CascadeUniform {
    light_vp: [[f32; 4]; 4],
}

pub struct Csm {
    pub array_view: wgpu::TextureView,
    layer_views: Vec<wgpu::TextureView>,
    pub cmp_samp: wgpu::Sampler,
    pipe_opaque: wgpu::RenderPipeline,
    pipe_mask: wgpu::RenderPipeline,
    ubs: Vec<wgpu::Buffer>,
    bgs: Vec<wgpu::BindGroup>,
    pub mats: [Mat4; N],
    centers: [Option<Vec3>; N],
    pub renders: u32,
}

fn light_basis(to_sun: Vec3) -> (Vec3, Vec3) {
    let dir = -to_sun.normalize();
    let up = if dir.z.abs() > 0.99 { Vec3::Y } else { Vec3::Z };
    (dir, up)
}

impl Csm {
    pub fn new(device: &wgpu::Device, mat_layout: &wgpu::BindGroupLayout, mat_samp: &wgpu::Sampler) -> Self {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("csm"),
            size: wgpu::Extent3d { width: RES, height: RES, depth_or_array_layers: N as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let array_view = tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let layer_views = (0..N as u32)
            .map(|l| tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2), base_array_layer: l, array_layer_count: Some(1), ..Default::default() }))
            .collect();
        let cmp_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("csm cmp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cascade"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let ubs: Vec<wgpu::Buffer> = (0..N)
            .map(|_| device.create_buffer(&wgpu::BufferDescriptor { label: Some("cascade"), size: 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }))
            .collect();
        let bgs = ubs
            .iter()
            .map(|ub| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("cascade"),
                    layout: &layout,
                    entries: &[wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(mat_samp) }],
                })
            })
            .collect();

        let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("shadow"), source: wgpu::ShaderSource::Wgsl(include_str!("shadow.wgsl").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("shadow"), bind_group_layouts: &[Some(&layout), Some(mat_layout)], immediate_size: 0 });
        let vattrs = wgpu::vertex_attr_array![0 => Float32x3, 2 => Float32x2];
        let vattrs = [vattrs[0], wgpu::VertexAttribute { offset: 16, ..vattrs[1] }];
        let vl = wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vattrs };
        let mk = |mask: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if mask { "shadow mask" } else { "shadow" }),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &sh, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[Some(vl.clone())] },
                // no culling: lots of Apex world geometry is single-sided
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState { constant: 0, slope_scale: 1.5, clamp: 0.0 },
                }),
                multisample: Default::default(),
                fragment: mask.then(|| wgpu::FragmentState { module: &sh, entry_point: Some("fs_mask"), compilation_options: Default::default(), targets: &[] }),
                multiview_mask: None,
                cache: None,
            })
        };
        Csm { array_view, layer_views, cmp_samp, pipe_opaque: mk(false), pipe_mask: mk(true), ubs, bgs, mats: [Mat4::IDENTITY; N], centers: [None; N], renders: 0 }
    }

    pub fn invalidate(&mut self) {
        self.centers = [None; N];
    }

    /// Re-renders the cascades the camera has moved away from. Returns how many were rendered.
    pub fn update(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, cam: Vec3, to_sun: Vec3, world: &LoadedWorld) -> u32 {
        let (dir, up) = light_basis(to_sun);
        let rot = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, dir, up);
        let mut enc: Option<wgpu::CommandEncoder> = None;
        let mut n = 0;
        for i in 0..N {
            let r = RADII[i];
            if let Some(c) = self.centers[i] {
                if (cam - c).length() < r * MOVE_FRAC {
                    continue;
                }
            }
            // snap the centre to the texel grid in light space
            let texel = 2.0 * r / RES as f32;
            let mut lc = (rot * cam.extend(1.0)).xyz();
            lc.x = (lc.x / texel).round() * texel;
            lc.y = (lc.y / texel).round() * texel;
            let center = (rot.inverse() * lc.extend(1.0)).xyz();
            let view = glam::camera::rh::view::look_to_mat4(center, dir, up);
            let proj = glam::camera::rh::proj::directx::orthographic(-r, r, -r, r, -DEPTH_RANGE, DEPTH_RANGE);
            let vp = proj * view;
            self.mats[i] = vp;
            self.centers[i] = Some(cam);
            queue.write_buffer(&self.ubs[i], 0, bytemuck::bytes_of(&CascadeUniform { light_vp: vp.to_cols_array_2d() }));

            let e = enc.get_or_insert_with(|| device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("csm") }));
            let mut rp = e.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("csm"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.layer_views[i],
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, &self.bgs[i], &[]);
            rp.set_vertex_buffer(0, world.vbuf.slice(..));
            rp.set_index_buffer(world.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            let mut cur = 255u8;
            for d in &world.draws {
                let alpha = d.key / 2;
                if alpha >= 2 || !d.shadow || !overlaps(&view, r, d.min, d.max) {
                    continue;
                }
                if alpha != cur {
                    cur = alpha;
                    rp.set_pipeline(if alpha == 1 { &self.pipe_mask } else { &self.pipe_opaque });
                }
                rp.set_bind_group(1, &world.mat_groups[d.material as usize], &[]);
                rp.draw_indexed(d.first_index..d.first_index + d.index_count, 0, 0..1);
            }
            n += 1;
        }
        if let Some(e) = enc {
            queue.submit([e.finish()]);
        }
        self.renders += n;
        n
    }
}

/// Does the box overlap the cascade's light-space X/Y square? (Depth is unlimited towards the sun.)
fn overlaps(view: &Mat4, r: f32, mn: [f32; 3], mx: [f32; 3]) -> bool {
    let c = (Vec3::from(mn) + Vec3::from(mx)) * 0.5;
    let e = (Vec3::from(mx) - Vec3::from(mn)) * 0.5;
    let lc = (*view * c.extend(1.0)).xyz();
    let m = glam::Mat3::from_mat4(*view);
    let ex = m.row(0).abs().dot(e);
    let ey = m.row(1).abs().dot(e);
    lc.x - ex < r && lc.x + ex > -r && lc.y - ey < r && lc.y + ey > -r
}
