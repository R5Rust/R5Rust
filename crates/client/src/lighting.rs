//! GPU side of the map's baked lighting, built from the raw BSP lumps the way the game's shaders consume them
//! (see docs/apex-lighting.md): sky lightmap pages, the real-time-light ID + BC5 visibility pages, the light list,
//! and decoded light probes for static props.

use shared::bsplight::{BspLighting, WorldLight};
use wgpu::util::DeviceExt;

/// One real-time light, mirroring the game's `HardwareLight` fields we need (80 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuLight {
    pub pos: [f32; 3],
    pub rcp_max_radius: f32,
    pub color: [f32; 3],
    /// 8 = inverse-square falloff, 16 = spot cone, 64 = skip (sun/sky entries)
    pub flags: u32,
    pub spot_dir: [f32; 3],
    pub spot_bias: f32,
    pub atten_linear: f32,
    pub atten_quadratic: f32,
    pub spot_exp_sel: f32,
    pub specular_intensity: f32,
    pub emitter_radius: f32,
    pub _pad: [f32; 3],
}

/// Decoded light probe (96 bytes).
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuProbe {
    /// per channel R,G,B: xyz = L1, w = L0
    pub sh: [[f32; 4]; 3],
    /// xyz = sky vector (visibility-scaled), w = baked sun visibility
    pub sky: [f32; 4],
    /// static lights: indices into the light list (-1 none)
    pub idx: [i32; 4],
    pub wts: [f32; 4],
}

pub struct GpuLighting {
    pub lm0: wgpu::TextureView,
    pub lm1: wgpu::TextureView,
    pub rtl_ids: wgpu::TextureView,
    pub rtl_vis: wgpu::TextureView,
    pub lights: wgpu::Buffer,
    pub rtl_index: wgpu::Buffer,
    pub probes: wgpu::Buffer,
    pub sun_color: [f32; 3],
    /// towards the sun
    pub sun_dir: [f32; 3],
    pub sky_color: [f32; 3],
    pub light_count: usize,
    pub probe_count: usize,
    /// the map's baked reflection cubemaps (BC6H HDR), one cube per BSP cubemap record
    pub cubes: wgpu::TextureView,
    /// per cubemap: 1 / luminance of its average (CUBEMAPS_AMBIENT_RCP stand-in; the shipped lump is zeros)
    pub cube_ambient: wgpu::Buffer,
}

const CUBE_AMBIENT_WGSL: &str = r#"
@group(0) @binding(0) var cubes: texture_cube_array<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<storage, read_write> outv: array<f32>;
@compute @workgroup_size(16)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = textureNumLayers(cubes);
    if (id.x >= n) { return; }
    let lvl = f32(textureNumLevels(cubes) - 1);
    var c = vec3<f32>(0.0);
    let dirs = array<vec3<f32>, 6>(vec3(1.0, 0.0, 0.0), vec3(-1.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0), vec3(0.0, -1.0, 0.0), vec3(0.0, 0.0, 1.0), vec3(0.0, 0.0, -1.0));
    for (var k = 0; k < 6; k++) {
        c += textureSampleLevel(cubes, samp, dirs[k], i32(id.x), lvl).rgb;
    }
    let l = dot(c / 6.0, vec3<f32>(0.2126, 0.7152, 0.0722));
    outv[id.x] = 1.0 / max(l, 1e-3);
}
"#;

fn load_cubes(device: &wgpu::Device, queue: &wgpu::Queue, path: &std::path::Path, count: usize) -> (wgpu::TextureView, wgpu::Buffer) {
    let n = count.max(1);
    let tex = match crate::dds::load(path) {
        Ok(d) if d.layers as usize >= 6 => crate::dds::upload(device, queue, &d, "cubemaps"),
        r => {
            if let Err(e) = r {
                log::warn!("cubemaps: {e}; reflections off");
            }
            let d = crate::dds::Dds { width: 4, height: 4, mips: 1, layers: 6, format: wgpu::TextureFormat::Bc6hRgbUfloat, data: vec![0; 16 * 6] };
            crate::dds::upload(device, queue, &d, "cubemaps (none)")
        }
    };
    let layers = tex.depth_or_array_layers();
    let view = tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::CubeArray), array_layer_count: Some(layers / 6 * 6), ..Default::default() });
    let buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("cube ambient"), size: (n.max((layers / 6) as usize) * 4) as u64, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
    let samp = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
    let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("cube ambient"), source: wgpu::ShaderSource::Wgsl(CUBE_AMBIENT_WGSL.into()) });
    let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("cube ambient"), layout: None, module: &sh, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cube ambient"),
        layout: &pipe.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&samp) },
            wgpu::BindGroupEntry { binding: 2, resource: buf.as_entire_binding() },
        ],
    });
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut cp = enc.begin_compute_pass(&Default::default());
        cp.set_pipeline(&pipe);
        cp.set_bind_group(0, &bg, &[]);
        cp.dispatch_workgroups((layers / 6).div_ceil(16), 1, 1);
    }
    queue.submit([enc.finish()]);
    log::info!("cubemaps: {} ({}x{}, {} mips)", layers / 6, tex.width(), tex.height(), tex.mip_level_count());
    (view, buf)
}

fn tex(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, format: wgpu::TextureFormat, data: &[u8], bytes_per_row: u32, rows: u32, label: &str) -> wgpu::TextureView {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(rows) },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    t.create_view(&Default::default())
}

fn gpu_light(l: &WorldLight) -> GpuLight {
    let r = l.radius;
    let mut flags = 0u32;
    if l.flags & 0x20 != 0 {
        flags |= 8;
    }
    if l.emit_type == 3 || l.emit_type == 5 {
        flags |= 64;
    }
    let (mut spot_dir, mut spot_bias) = ([0.0; 3], 0.0);
    if l.emit_type == 2 {
        flags |= 16;
        let d = (l.stopdot - l.stopdot2).max(1e-4);
        spot_dir = [-l.normal[0] / d, -l.normal[1] / d, -l.normal[2] / d];
        spot_bias = -l.stopdot2 / d;
    }
    GpuLight {
        pos: l.origin,
        rcp_max_radius: if r > 0.0 { 1.0 / r } else { 0.0 },
        color: l.intensity,
        flags,
        spot_dir,
        spot_bias,
        atten_linear: l.linear_attn * r,
        atten_quadratic: l.quadratic_attn * r * r,
        spot_exp_sel: (l.exponent - 1.0).clamp(0.0, 1.0),
        specular_intensity: 1.0,
        emitter_radius: 0.0,
        _pad: [0.0; 3],
    }
}

impl GpuLighting {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, bsp: &BspLighting, cube_path: &std::path::Path) -> Self {
        let (aw, ah) = (bsp.sky0.width, bsp.sky0.height);
        let lm0 = tex(device, queue, aw, ah, wgpu::TextureFormat::Rgba8Unorm, &bsp.sky0.rgba, aw * 4, ah, "lm0");
        let lm1 = tex(device, queue, aw, ah, wgpu::TextureFormat::Rgba8Unorm, &bsp.sky1.rgba, aw * 4, ah, "lm1");

        // RTL: per page, R32 light IDs (w*h*4) then BC5 visibility at w x 2h. Atlas pages side by side.
        let rtl = &bsp.rtl_raw;
        let mut ids = vec![0u8; (aw * ah * 4) as usize];
        let vis_h = ah * 2;
        let (bw_atlas, bh_atlas) = (aw / 4, vis_h / 4);
        let mut vis = vec![0u8; (bw_atlas * bh_atlas * 16) as usize];
        let (mut at, mut x0) = (0usize, 0u32);
        for h in &bsp.headers {
            let (w, hh) = (h.width as usize, h.height as usize);
            let nid = w * hh * 4;
            let (pw, ph) = (w.div_ceil(4) * 4, hh.div_ceil(4) * 4);
            let nvis = 2 * pw * ph;
            if at + nid + nvis > rtl.len() {
                log::warn!("RTL lump shorter than headers; real-time light visibility skipped");
                break;
            }
            for y in 0..hh {
                let s = &rtl[at + y * w * 4..at + (y + 1) * w * 4];
                let d = ((y * aw as usize) + x0 as usize) * 4;
                ids[d..d + w * 4].copy_from_slice(s);
            }
            let src = &rtl[at + nid..at + nid + nvis];
            let (pbw, pbh) = (pw / 4, 2 * ph / 4);
            for by in 0..pbh {
                let s = &src[by * pbw * 16..(by + 1) * pbw * 16];
                let d = (by * bw_atlas as usize + (x0 / 4) as usize) * 16;
                vis[d..d + pbw * 16].copy_from_slice(s);
            }
            at += nid + nvis;
            x0 += h.width;
        }
        let rtl_ids = tex(device, queue, aw, ah, wgpu::TextureFormat::R32Uint, &ids, aw * 4, ah, "rtl ids");
        let rtl_vis = tex(device, queue, aw, vis_h, wgpu::TextureFormat::Bc5RgUnorm, &vis, bw_atlas * 16, bh_atlas, "rtl vis");

        // Light list = worldlights with lump flag 0x8, in lump order (what 0x7A and probe indices point into).
        let list: Vec<&WorldLight> = bsp.lights.iter().filter(|l| l.flags & 0x8 != 0).collect();
        let mut gl: Vec<GpuLight> = list.iter().map(|l| gpu_light(l)).collect();
        if gl.is_empty() {
            gl.push(GpuLight { flags: 64, ..Default::default() });
        }
        let lights = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("lights"), contents: bytemuck::cast_slice(&gl), usage: wgpu::BufferUsages::STORAGE });
        let mut ri: Vec<u32> = bsp.rtl_index.iter().map(|&v| v as u32).collect();
        if ri.is_empty() {
            ri.push(0);
        }
        let rtl_index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("rtl index"), contents: bytemuck::cast_slice(&ri), usage: wgpu::BufferUsages::STORAGE });

        const S: f32 = 1.0 / 8192.0;
        let mut gp: Vec<GpuProbe> = bsp
            .probes
            .iter()
            .map(|p| {
                let a = p.ambient_sh;
                let ch = |c: usize| [a[c * 4] as f32 * S, a[c * 4 + 1] as f32 * S, a[c * 4 + 2] as f32 * S, a[c * 4 + 3] as f32 * S];
                let s = p.sky_dir_sun_vis;
                GpuProbe {
                    sh: [ch(0), ch(1), ch(2)],
                    sky: [s[0] as f32 * S, s[1] as f32 * S, s[2] as f32 * S, (s[3] as f32 * S).clamp(0.0, 1.0)],
                    idx: p.static_light_indexes.map(|i| i as i32),
                    wts: p.static_light_weights.map(|w| w as f32 / 255.0),
                }
            })
            .collect();
        if gp.is_empty() {
            gp.push(GpuProbe::default());
        }
        let probes = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("probes"), contents: bytemuck::cast_slice(&gp), usage: wgpu::BufferUsages::STORAGE });

        let (cubes, cube_ambient) = load_cubes(device, queue, cube_path, bsp.cubemaps.len());
        let sun = bsp.lights.iter().find(|l| l.emit_type == 3);
        let sky = bsp.lights.iter().find(|l| l.emit_type == 5);
        let sun_color = sun.map(|l| l.intensity).unwrap_or([1.5, 1.4, 1.2]);
        let sun_dir = sun.map(|l| [-l.normal[0], -l.normal[1], -l.normal[2]]).unwrap_or([0.5, -0.3, 0.8]);
        let sky_color = sky.map(|l| l.intensity).unwrap_or([0.22, 0.49, 0.9]);
        log::info!(
            "lighting: {} rtl lights, {} probes, sun {:?} dir {:?}, sky {:?}",
            list.len(),
            bsp.probes.len(),
            sun_color,
            sun_dir,
            sky_color
        );
        GpuLighting { lm0, lm1, rtl_ids, rtl_vis, lights, rtl_index, probes, sun_color, sun_dir, sky_color, light_count: gl.len(), probe_count: gp.len(), cubes, cube_ambient }
    }
}

/// One probe per static-prop instance, like the game: instances aren't kept in world.glb, so each connected
/// component of a non-lightmapped draw stands in for one, and takes the probe nearest its centre.
/// Returns per vertex: probe index in bits 0-23 (0xFFFFFF = none / lightmapped), cubemap index in bits 24-31.
pub fn assign_probes(bsp: &BspLighting, vertices: &[shared::glb::Vertex], indices: &[u32], draws: &[shared::glb::Draw], lightmapped: impl Fn(u32) -> bool) -> Vec<u32> {
    let n = vertices.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    let mut prop_vertex = vec![false; n];
    for d in draws {
        if lightmapped(d.material) {
            continue;
        }
        for t in indices[d.first_index as usize..(d.first_index + d.index_count) as usize].chunks_exact(3) {
            for &v in t {
                prop_vertex[v as usize] = true;
            }
            let a = find(&mut parent, t[0]);
            for &v in &t[1..] {
                let b = find(&mut parent, v);
                if a != b {
                    parent[b as usize] = a;
                }
            }
        }
    }
    // component centres
    let mut sum: std::collections::HashMap<u32, ([f64; 3], u32)> = std::collections::HashMap::new();
    for v in 0..n {
        if !prop_vertex[v] {
            continue;
        }
        let r = find(&mut parent, v as u32);
        let e = sum.entry(r).or_insert(([0.0; 3], 0));
        for k in 0..3 {
            e.0[k] += vertices[v].pos[k] as f64;
        }
        e.1 += 1;
    }
    let probe_of: std::collections::HashMap<u32, u32> = sum
        .iter()
        .map(|(&r, (s, c))| {
            let c = *c as f64;
            let p = [(s[0] / c) as f32, (s[1] / c) as f32, (s[2] / c) as f32];
            let packed = match bsp.nearest_probe(p) {
                Some(r) => (r.probe & 0xFFFFFF) | ((r.cube_index as u32 & 0xFF) << 24),
                None => 0xFFFFFF | (bsp.nearest_cubemap(p) << 24),
            };
            (r, packed)
        })
        .collect();
    let roots: Vec<u32> = (0..n).map(|v| if prop_vertex[v] { find(&mut parent, v as u32) } else { u32::MAX }).collect();
    use rayon::prelude::*;
    (0..n)
        .into_par_iter()
        .map(|v| match roots[v] {
            u32::MAX => 0xFFFFFF | (bsp.nearest_cubemap(vertices[v].pos) << 24),
            r => probe_of.get(&r).copied().unwrap_or(0xFFFFFF | (bsp.nearest_cubemap(vertices[v].pos) << 24)),
        })
        .collect()
}
