//! Background map loading: GLB parse, texture decode + mips + upload, buffers, collision.
//! Runs on its own thread with clones of the device/queue so the main thread keeps drawing
//! the loading screen.

use rayon::prelude::*;
use shared::assets::Config;
use shared::collision::Collision;
use shared::glb::{self, Meta};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;
use wgpu::util::DeviceExt;

pub struct DrawCmd {
    pub first_index: u32,
    pub index_count: u32,
    pub material: u32,
    /// pipeline key: alpha_mode * 2 + double_sided
    pub key: u8,
    /// world-space bounds of this chunk
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Splits every draw into spatial chunks (grid cells in X/Y) so the camera and the shadow cascades can cull.
/// Returns the re-ordered index buffer and the chunk list, opaque first, blended last.
fn chunk_draws(world: &glb::World) -> (Vec<u32>, Vec<DrawCmd>) {
    const CELL: f32 = 4096.0;
    const MIN_SPLIT_TRIS: u32 = 6000;
    let pos = |i: u32| world.vertices[i as usize].pos;
    let mut out_idx: Vec<u32> = Vec::with_capacity(world.indices.len());
    let mut cmds = Vec::new();
    for d in &world.draws {
        let m = &world.materials[d.material as usize];
        let key = m.alpha_mode * 2 + m.double_sided as u8;
        let tris = &world.indices[d.first_index as usize..(d.first_index + d.index_count) as usize];
        let mut cells: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        let split = d.index_count / 3 >= MIN_SPLIT_TRIS;
        for t in tris.chunks_exact(3) {
            let k = if split {
                let (a, b, c) = (pos(t[0]), pos(t[1]), pos(t[2]));
                let cx = (a[0] + b[0] + c[0]) / 3.0;
                let cy = (a[1] + b[1] + c[1]) / 3.0;
                ((cx / CELL).floor() as i32, (cy / CELL).floor() as i32)
            } else {
                (0, 0)
            };
            cells.entry(k).or_default().extend_from_slice(t);
        }
        let mut keys: Vec<_> = cells.keys().copied().collect();
        keys.sort();
        for k in keys {
            let ix = &cells[&k];
            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
            for &i in ix {
                let p = pos(i);
                for a in 0..3 {
                    mn[a] = mn[a].min(p[a]);
                    mx[a] = mx[a].max(p[a]);
                }
            }
            cmds.push(DrawCmd { first_index: out_idx.len() as u32, index_count: ix.len() as u32, material: d.material, key, min: mn, max: mx });
            out_idx.extend_from_slice(ix);
        }
    }
    // stable: opaque, then alpha-tested, then blended (glb order kept inside each class)
    cmds.sort_by_key(|c| (c.key / 2, c.key, c.material));
    (out_idx, cmds)
}

pub struct LoadedWorld {
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    pub draws: Vec<DrawCmd>,
    pub mat_groups: Vec<wgpu::BindGroup>,
    pub lightmap: wgpu::TextureView,
    pub collision: Collision,
    pub meta: Meta,
    pub stats: String,
}

pub struct Progress(pub AtomicU32);
impl Progress {
    pub fn set(&self, f: f32) {
        self.0.store((f.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    }
    pub fn get(&self) -> f32 {
        self.0.load(Ordering::Relaxed) as f32 / 1000.0
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MatUniform {
    color: [f32; 4],
    params: [f32; 4],
}

fn mip_chain(img: image::RgbaImage, max: u32) -> Vec<image::RgbaImage> {
    let mut cur = img;
    let (w, h) = cur.dimensions();
    let m = w.max(h);
    if m > max {
        let s = max as f32 / m as f32;
        cur = image::imageops::resize(&cur, ((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1), image::imageops::FilterType::Triangle);
    }
    let mut out = vec![cur];
    loop {
        let (w, h) = out.last().unwrap().dimensions();
        if w == 1 && h == 1 {
            break;
        }
        let next = image::imageops::resize(out.last().unwrap(), (w / 2).max(1), (h / 2).max(1), image::imageops::FilterType::Triangle);
        out.push(next);
    }
    out
}

pub fn upload_rgba(device: &wgpu::Device, queue: &wgpu::Queue, mips: &[image::RgbaImage], format: wgpu::TextureFormat, label: &str) -> wgpu::Texture {
    let (w, h) = mips[0].dimensions();
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (lvl, m) in mips.iter().enumerate() {
        let (mw, mh) = m.dimensions();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: lvl as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            m.as_raw(),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * mw), rows_per_image: Some(mh) },
            wgpu::Extent3d { width: mw, height: mh, depth_or_array_layers: 1 },
        );
    }
    tex
}

pub fn load(cfg: &Config, device: &wgpu::Device, queue: &wgpu::Queue, mat_layout: &wgpu::BindGroupLayout, progress: &Progress) -> Result<LoadedWorld, String> {
    let t0 = Instant::now();
    let meta = glb::load_meta(&cfg.meta_json())?;
    progress.set(0.02);
    let mut world = glb::load_world(&cfg.world_glb(), glb::Mode::Full, false)?;
    let t_glb = t0.elapsed().as_secs_f64();
    progress.set(0.25);
    log::info!("glb: {} verts, {} tris, {} draws, {} materials, {} images in {:.1}s", world.vertices.len(), world.tri_count(), world.draws.len(), world.materials.len(), world.images.len(), t_glb);

    // Collision runs alongside texture work (it only needs positions + non-blend indices).
    let positions = std::mem::take(&mut world.positions);
    let mut col_idx: Vec<u32> = Vec::new();
    for d in &world.draws {
        let m = &world.materials[d.material as usize];
        if m.alpha_mode != 2 {
            col_idx.extend_from_slice(&world.indices[d.first_index as usize..(d.first_index + d.index_count) as usize]);
        }
    }
    let col_thread = std::thread::spawn(move || Collision::build(positions, &col_idx));

    // Geometry buffers (index buffer re-ordered into spatial chunks).
    let (chunk_idx, draws) = chunk_draws(&world);
    world.indices = chunk_idx;
    let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("world vb"), contents: bytemuck::cast_slice(&world.vertices), usage: wgpu::BufferUsages::VERTEX });
    let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("world ib"), contents: bytemuck::cast_slice(&world.indices), usage: wgpu::BufferUsages::INDEX });
    let vb_mb = (world.vertices.len() * 32) as f64 / 1e6;
    let tris = world.tri_count();
    world.vertices = Vec::new();
    world.indices = Vec::new();
    progress.set(0.3);

    // Textures: only the base-colour images the materials actually use.
    let mut used: Vec<u32> = world.materials.iter().filter_map(|m| m.base_tex).collect();
    used.sort();
    used.dedup();
    let done = AtomicU32::new(0);
    let total = used.len().max(1) as f32;
    let max = cfg.max_texture_size.max(16);
    let views: HashMap<u32, wgpu::TextureView> = used
        .par_iter()
        .filter_map(|&ii| {
            let bytes = world.image_bytes(ii)?;
            let img = match image::load_from_memory(bytes) {
                Ok(i) => i.to_rgba8(),
                Err(e) => {
                    log::warn!("image {ii}: {e}");
                    return None;
                }
            };
            let mips = mip_chain(img, max);
            let tex = upload_rgba(device, queue, &mips, wgpu::TextureFormat::Rgba8UnormSrgb, "base");
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            progress.set(0.3 + 0.6 * n as f32 / total);
            Some((ii, tex.create_view(&wgpu::TextureViewDescriptor::default())))
        })
        .collect();
    let t_tex = t0.elapsed().as_secs_f64();

    // Lightmap: linear data, no mips (the atlas packs unrelated islands next to each other).
    let lightmap = match world.lightmap_image.and_then(|i| world.image_bytes(i)).and_then(|b| image::load_from_memory(b).ok()) {
        Some(img) => {
            let img = img.to_rgba8();
            log::info!("lightmap {}x{}", img.width(), img.height());
            upload_rgba(device, queue, &[img], wgpu::TextureFormat::Rgba8Unorm, "lightmap")
        }
        None => {
            log::warn!("no lightmap in the GLB; lighting falls back to sun + sky");
            upload_rgba(device, queue, &[image::RgbaImage::from_pixel(1, 1, image::Rgba([64, 64, 64, 255]))], wgpu::TextureFormat::Rgba8Unorm, "lightmap")
        }
    }
    .create_view(&wgpu::TextureViewDescriptor::default());

    let white = upload_rgba(device, queue, &[image::RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))], wgpu::TextureFormat::Rgba8UnormSrgb, "white").create_view(&wgpu::TextureViewDescriptor::default());

    let mat_groups: Vec<wgpu::BindGroup> = world
        .materials
        .iter()
        .map(|m| {
            let u = MatUniform {
                color: m.base_color,
                params: [m.alpha_cutoff, if m.lightmapped { 1.0 } else { 0.0 }, m.alpha_mode as f32, 0.0],
            };
            let ub = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("mat"), contents: bytemuck::bytes_of(&u), usage: wgpu::BufferUsages::UNIFORM });
            let view = m.base_tex.and_then(|t| views.get(&t)).unwrap_or(&white);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mat"),
                layout: mat_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view) },
                ],
            })
        })
        .collect();


    progress.set(0.95);
    let collision = col_thread.join().map_err(|_| "collision build panicked".to_string())?;
    // make sure every upload has landed before the first frame uses it
    queue.submit([]);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    progress.set(1.0);
    let stats = format!(
        "{:.1} M tris, {} draws, {} textures, VB {:.0} MB, BVH {} nodes ({:.0} ms); glb {:.1}s, textures {:.1}s, total {:.1}s",
        tris as f64 / 1e6,
        draws.len(),
        views.len(),
        vb_mb,
        collision.node_count(),
        collision.build_ms,
        t_glb,
        t_tex - t_glb,
        t0.elapsed().as_secs_f64()
    );
    log::info!("{stats}");
    Ok(LoadedWorld { vbuf, ibuf, draws, mat_groups, lightmap, collision, meta, stats })
}

