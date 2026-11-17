//! Background map loading: GLB parse, texture decode + mips + upload, buffers, collision.
//! Runs on its own thread with clones of the device/queue so the main thread keeps drawing
//! the loading screen.

use rayon::prelude::*;
use shared::assets::Config;
use shared::collision::Collision;
use shared::collmesh;
use crate::lighting::{self, GpuLighting};
use crate::texcache::{self, Kind};
use shared::bsplight::BspLighting;
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
    /// casts sun (CSM) shadows
    pub shadow: bool,
    /// 3D skybox geometry: only the skybox pass draws it
    pub skybox: bool,
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
        let key = if m.sky { 6 } else { m.alpha_mode * 2 + m.double_sided as u8 };
        let (shadow, skybox) = (!m.no_shadow && !m.skybox, m.skybox);
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
            cmds.push(DrawCmd { first_index: out_idx.len() as u32, index_count: ix.len() as u32, material: d.material, key, min: mn, max: mx, shadow, skybox });
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
    /// per-vertex light probe index (u32::MAX = lightmapped / none), vertex stream 1
    pub probe_vbuf: wgpu::Buffer,
    pub light: GpuLighting,
    /// the sky material's cube texture (Firing Range: projected 'fake sky')
    pub sky_cube: wgpu::TextureView,
    pub fog: crate::fog::Fog,
    /// HDR colour-grading LUT (33^3, PQ in/out) from the map's color_correction entity
    pub grading: Option<wgpu::TextureView>,
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
    /// x = alpha cutoff, y = lightmapped, z = alpha mode, w = world surface (uses vertex colour)
    params: [f32; 4],
    /// x = has AO texture, y = has emissive texture
    params2: [f32; 4],
    /// c_L0_albedoTint (rgb)
    albedo_tint: [f32; 4],
    /// c_L0_emissiveTint (rgb)
    emissive_tint: [f32; 4],
    /// c_emissiveEdgeFade exponent, inner, outer
    emissive_edge: [f32; 4],
    /// x = two-layer blend, y = c_layerBlendRamp, z = layer 0 world-planar mapped, w = has detail normal
    blend: [f32; 4],
    /// c_uv1RotScaleX.xy, c_uv1RotScaleY.xy
    uv1: [f32; 4],
    /// c_uv1Translate.xy, c_uv2Translate.xy
    uv_t: [f32; 4],
    /// c_uv2RotScaleX.xy, c_uv2RotScaleY.xy
    uv2: [f32; 4],
    /// layer 1 albedo tint (rgb)
    albedo2_tint: [f32; 4],
    /// x = has gloss2, y = has spec2, z = perf gloss L0, w = perf gloss L1
    layer2: [f32; 4],
    /// perf spec colour L0 (rgb), w = has spec texture
    perf_spec: [f32; 4],
    /// perf spec colour L1 (rgb)
    perf_spec2: [f32; 4],
}

const TEX_SLOTS: [(&str, Kind); 12] = [
    ("albedo", Kind::Albedo),
    ("normal", Kind::Normal),
    ("gloss", Kind::Gloss),
    ("spec", Kind::Spec),
    ("ao", Kind::Ao),
    ("emissive", Kind::Emissive),
    ("albedo2", Kind::Albedo),
    ("normal2", Kind::Normal),
    ("gloss2", Kind::Gloss),
    ("spec2", Kind::Spec),
    ("blend", Kind::Gloss),
    ("detail_normal", Kind::Normal),
];

/// Six faces `<leaf>_albedoTexture_000..005.png` (D3D face order) of the sky material, as one cube texture.
fn load_sky_cube(device: &wgpu::Device, queue: &wgpu::Queue, root: &std::path::Path, map_dir: &std::path::Path, sky_mat: Option<&str>) -> wgpu::TextureView {
    // The game's HDR original (BC6H cube from the map pak). The extracted PNG faces are clipped to 8 bits.
    // `skies/<material leaf>.dds` first: the Firing Range dome's props draw skin 1, which swaps
    // freedm_cubemap_skulltown for freedm_cubemap_firingrange (docs/skybox.md). skycube.dds is the old fallback.
    let leaf = sky_mat.and_then(|n| n.rsplit('/').next());
    let named = leaf.map(|l| map_dir.join("skies").join(format!("{l}.dds")));
    for path in named.into_iter().chain(std::iter::once(map_dir.join("skycube.dds"))) {
        if let Ok(d) = crate::dds::load(&path) {
            if d.layers == 6 {
                let t = crate::dds::upload(device, queue, &d, "sky cube (hdr)");
                log::info!("sky cube: HDR {}x{} BC6H from {}", d.width, d.height, path.display());
                return t.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() });
            }
        }
    }
    let faces: Option<Vec<image::RgbaImage>> = sky_mat.and_then(|name| {
        let leaf = name.rsplit('/').next()?;
        (0..6)
            .map(|i| image::open(root.join(format!("images/textures/other/skies/{leaf}_albedoTexture_{i:03}.png"))).ok().map(|im| image::imageops::resize(&im.to_rgba8(), 1024, 1024, image::imageops::FilterType::Triangle)))
            .collect()
    });
    let faces = faces.unwrap_or_else(|| {
        log::warn!("no sky material faces; sky surfaces draw black");
        (0..6).map(|_| image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 0, 0, 255]))).collect()
    });
    let size = faces[0].width();
    let mips = 32 - size.leading_zeros();
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("sky cube"),
        size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (f, img) in faces.into_iter().enumerate() {
        for (lvl, m) in mip_chain(img, size).iter().enumerate() {
            let (w, h) = m.dimensions();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: lvl as u32, origin: wgpu::Origin3d { x: 0, y: 0, z: f as u32 }, aspect: wgpu::TextureAspect::All },
                m.as_raw(),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        }
    }
    tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() })
}

/// The map's color_correction `.raw_hdr` (4-byte header + 33^3 R10G10B10A2 texels, red fastest, PQ-encoded),
/// looked up by file name next to the map.
fn load_grading(device: &wgpu::Device, queue: &wgpu::Queue, map_dir: &std::path::Path, bsp_dir: &std::path::Path) -> Option<wgpu::TextureView> {
    let mut text = String::new();
    for e in std::fs::read_dir(bsp_dir).ok()?.flatten() {
        if e.path().extension().map(|x| x == "ent").unwrap_or(false) {
            text.push_str(&String::from_utf8_lossy(&std::fs::read(e.path()).unwrap_or_default()));
        }
    }
    let kv = crate::fog::parse_ent(&text, "color_correction")?;
    let file = kv.get("filename")?.rsplit('/').next()?.to_string();
    let b = std::fs::read(map_dir.join(&file)).map_err(|e| log::warn!("grading {file}: {e}")).ok()?;
    const N: u32 = 33;
    if b.len() != 4 + (N * N * N * 4) as usize {
        log::warn!("grading {file}: unexpected size {}", b.len());
        return None;
    }
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("grading lut"),
        size: wgpu::Extent3d { width: N, height: N, depth_or_array_layers: N },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgb10a2Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        &b[4..],
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(N * 4), rows_per_image: Some(N) },
        wgpu::Extent3d { width: N, height: N, depth_or_array_layers: N },
    );
    log::info!("grading: {file} (33^3 HDR LUT)");
    Some(tex.create_view(&Default::default()))
}

fn tint(v: &serde_json::Value) -> [f32; 4] {
    let a = v.as_array();
    let c = |i: usize| a.and_then(|a| a.get(i)).and_then(|x| x.as_f64()).unwrap_or(1.0) as f32;
    [c(0), c(1), c(2), 1.0]
}

fn tv(b: u32, v: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding: b, resource: wgpu::BindingResource::TextureView(v) }
}

fn solid(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4], format: wgpu::TextureFormat, label: &str) -> wgpu::TextureView {
    upload_rgba(device, queue, &[image::RgbaImage::from_pixel(4, 4, image::Rgba(rgba))], format, label).create_view(&Default::default())
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

    // Collision runs alongside texture work. The game's own collision (collision.bin, see
    // docs/collision.md) when it was exported, else world.glb's solid render triangles.
    let positions = std::mem::take(&mut world.positions);
    let bin = cfg.collision_bin();
    let render_idx = collmesh::render_indices(&world);
    let col_thread = std::thread::spawn(move || {
        if bin.is_file() {
            match collmesh::CollMesh::load(&bin) {
                Ok(m) => {
                    let idx = m.player_indices();
                    log::info!("collision: game collision.bin, {} of {} triangles block players", idx.len() / 3, m.tris.len());
                    return Collision::build(m.positions, &idx);
                }
                Err(e) => log::warn!("{e}"),
            }
        }
        log::warn!("collision: no usable collision.bin, colliding with world.glb render triangles");
        Collision::build(positions, &render_idx)
    });

    // Baked lighting from the raw BSP lumps + one probe per static prop.
    let bsp = BspLighting::load(&cfg.bsp_dir())?;
    let sky_probe = meta.sky_probe.map(|(p, c)| (p & 0xFFFFFF) | ((c & 0xFF) << 24));
    let probe_ix = lighting::assign_probes(&bsp, &world.vertices, &world.indices, &world.draws, |m| world.materials[m as usize].lightmapped, |m| {
        if world.materials[m as usize].skybox { sky_probe } else { None }
    });
    let probe_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("probe vb"), contents: bytemuck::cast_slice(&probe_ix), usage: wgpu::BufferUsages::VERTEX });
    drop(probe_ix);
    let light = GpuLighting::new(device, queue, &bsp, &cfg.map_dir().join("cubemaps.dds"));
    drop(bsp);
    let t_light = t0.elapsed().as_secs_f64();

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

    // The game's own material textures (materials.json from tools/build_materials.py), block-compressed + cached.
    let mats_json: serde_json::Value = std::fs::read_to_string(cfg.map_dir().join("materials.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| {
            log::warn!("no materials.json (run tools/build_materials.py): using world.glb base colours only");
            serde_json::Value::Null
        });
    let root = cfg.assets_root();
    let cache = root.parent().unwrap_or(&root).join("_cache").join("tex");
    let cap = cfg.max_texture_size.max(16);
    let mut wanted: Vec<(String, Kind)> = Vec::new();
    for m in &world.materials {
        let e = &mats_json[&m.name];
        for (k, kind) in TEX_SLOTS {
            if kind == Kind::Albedo && m.alpha_mode != 0 && !(m.alpha_mode == 2 && e["blend"].is_string()) {
                continue; // cut-out / blended surfaces keep the glb albedo, which carries their alpha
            }
            if let Some(rel) = e[k].as_str() {
                wanted.push((rel.to_string(), kind));
            }
        }
    }
    wanted.sort_by(|a, b| a.0.cmp(&b.0).then((a.1 as u8).cmp(&(b.1 as u8))));
    wanted.dedup();
    let done = AtomicU32::new(0);
    let total = wanted.len().max(1) as f32;
    let mat_tex: HashMap<(String, u8), wgpu::TextureView> = wanted
        .par_iter()
        .filter_map(|(rel, kind)| {
            let e = texcache::load(&root, &cache, rel, *kind, cap)?;
            let t = texcache::upload(device, queue, &e);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            progress.set(0.3 + 0.5 * n as f32 / total);
            Some(((rel.clone(), *kind as u8), t.create_view(&Default::default())))
        })
        .collect();

    // glb base colours: fallback albedo, and the alpha source for cut-outs
    let mut used: Vec<u32> = world.materials.iter().filter(|m| m.alpha_mode != 0 || mats_json[&m.name]["albedo"].as_str().is_none()).filter_map(|m| m.base_tex).collect();
    used.sort();
    used.dedup();
    let max = cfg.max_texture_size.max(16);
    let views: HashMap<u32, wgpu::TextureView> = used
        .par_iter()
        .filter_map(|&ii| {
            let bytes = world.image_bytes(ii)?;
            let img = image::load_from_memory(bytes).ok()?.to_rgba8();
            let tex = upload_rgba(device, queue, &mip_chain(img, max), wgpu::TextureFormat::Rgba8UnormSrgb, "base");
            Some((ii, tex.create_view(&wgpu::TextureViewDescriptor::default())))
        })
        .collect();
    let t_tex = t0.elapsed().as_secs_f64();

    let sky_cube = load_sky_cube(device, queue, &root, &cfg.map_dir(), world.materials.iter().find(|m| m.sky).map(|m| m.name.as_str()));

    use wgpu::TextureFormat as F;
    let white = solid(device, queue, [255; 4], F::Rgba8UnormSrgb, "white");
    let flat_n = solid(device, queue, [128, 128, 0, 255], F::Rgba8Unorm, "flat normal");
    let def_gloss = solid(device, queue, [64, 64, 64, 255], F::Rgba8Unorm, "gloss");
    let def_spec = solid(device, queue, [56, 56, 56, 255], F::Rgba8UnormSrgb, "spec 0.04");
    let one = solid(device, queue, [255; 4], F::Rgba8Unorm, "ao 1");
    let black = solid(device, queue, [0, 0, 0, 255], F::Rgba8UnormSrgb, "black");

    let mat_groups: Vec<wgpu::BindGroup> = world
        .materials
        .iter()
        .map(|m| {
            let e = &mats_json[&m.name];
            let get = |k: &str, kind: Kind| e[k].as_str().and_then(|r| mat_tex.get(&(r.to_string(), kind as u8)));
            let albedo = if m.alpha_mode == 0 || (m.alpha_mode == 2 && e["blend"].is_string()) { get("albedo", Kind::Albedo) } else { None }.or_else(|| m.base_tex.and_then(|t| views.get(&t))).unwrap_or(&white);
            let normal = get("normal", Kind::Normal).unwrap_or(&flat_n);
            let gloss = get("gloss", Kind::Gloss).unwrap_or(&def_gloss);
            let spec = get("spec", Kind::Spec).unwrap_or(&def_spec);
            let ao = get("ao", Kind::Ao);
            // two-layer blend materials use vertex alpha as the blend factor, not the RGB as a tint
            let is_blend = e["all"].get("layerBlendTexture").is_some() || e["all"].get("bm").is_some();
            let emis = get("emissive", Kind::Emissive);
            let albedo2 = get("albedo2", Kind::Albedo);
            let normal2 = get("normal2", Kind::Normal);
            let gloss2 = get("gloss2", Kind::Gloss);
            let spec2 = get("spec2", Kind::Spec);
            let mask = get("blend", Kind::Gloss);
            let detail_n = get("detail_normal", Kind::Normal);
            let f2 = |k: &str, d: [f32; 2]| -> [f32; 2] {
                let a = e[k].as_array();
                let c = |i: usize| a.and_then(|a| a.get(i)).and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(d[i]);
                [c(0), c(1)]
            };
            let f1 = |k: &str, d: f32| e[k].as_f64().map(|x| x as f32).unwrap_or(d);
            let (rx, ry, t1) = (f2("uv1RotScaleX", [1.0, 0.0]), f2("uv1RotScaleY", [0.0, 1.0]), f2("uv1Translate", [0.0, 0.0]));
            let (rx2, ry2, t2) = (f2("uv2RotScaleX", [1.0, 0.0]), f2("uv2RotScaleY", [0.0, 1.0]), f2("uv2Translate", [0.0, 0.0]));
            let two_layer = albedo2.is_some();
            // blended "transparent" overlays (e.g. sand over the range floor): opacity = (1 - blend) * albedo alpha
            let overlay = !two_layer && m.alpha_mode == 2 && mask.is_some();
            // a tiny layer-0 UV scale means the shader maps layer 0 from world position (terrain-style)
            let planar = (two_layer || overlay) && (rx[0].abs() + rx[1].abs()) < 0.05;
            if m.alpha_mode == 2 && std::env::var_os("R5_LOG_BLEND").is_some() {
                log::info!("blend mat {} overlay={overlay} two_layer={two_layer} mask={} json_blend={}", m.name, mask.is_some(), e["blend"]);
            }
            let u = MatUniform {
                color: m.base_color,
                params: [m.alpha_cutoff, if m.lightmapped { 1.0 } else { 0.0 }, m.alpha_mode as f32, if m.name.starts_with("world/") && !is_blend { 1.0 } else { 0.0 }],
                params2: [ao.is_some() as u32 as f32, emis.is_some() as u32 as f32, 0.0, 0.0],
                albedo_tint: tint(&e["albedo_tint"]),
                emissive_tint: tint(&e["emissive_tint"]),
                emissive_edge: {
                    let a = e["emissive_edge"].as_array();
                    let c = |i: usize, d: f32| a.and_then(|a| a.get(i)).and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(d);
                    [c(0, 0.0), c(1, 1.0), c(2, 1.0), 0.0]
                },
                blend: [if overlay { if e["old_style"].as_bool().unwrap_or(false) { 3.0 } else { 2.0 } } else { two_layer as u32 as f32 }, f1("blend_ramp", 3.0), planar as u32 as f32, detail_n.is_some() as u32 as f32],
                uv1: [rx[0], rx[1], ry[0], ry[1]],
                uv_t: [t1[0], t1[1], t2[0], t2[1]],
                uv2: [rx2[0], rx2[1], ry2[0], ry2[1]],
                albedo2_tint: tint(&e["albedo2_tint"]),
                layer2: [gloss2.is_some() as u32 as f32, spec2.is_some() as u32 as f32, f1("perf_gloss", 0.25), f1("perf_gloss2", 0.25)],
                perf_spec: { let t = tint(&e["perf_spec"]); [t[0], t[1], t[2], (!std::ptr::eq(spec, &def_spec)) as u32 as f32] },
                perf_spec2: tint(&e["perf_spec2"]),
            };
            let ub = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("mat"), contents: bytemuck::bytes_of(&u), usage: wgpu::BufferUsages::UNIFORM });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mat"),
                layout: mat_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                    tv(1, albedo),
                    tv(2, normal),
                    tv(3, gloss),
                    tv(4, spec),
                    tv(5, ao.unwrap_or(&one)),
                    tv(6, emis.unwrap_or(&black)),
                    tv(7, albedo2.unwrap_or(&white)),
                    tv(8, normal2.unwrap_or(&flat_n)),
                    tv(9, gloss2.unwrap_or(&def_gloss)),
                    tv(10, spec2.unwrap_or(&def_spec)),
                    tv(11, mask.unwrap_or(&one)),
                    tv(12, detail_n.unwrap_or(&flat_n)),
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
        "{:.1} M tris, {} draws, {} material textures + {} glb, {} lights, {} probes, VB {:.0} MB, BVH {} nodes ({:.0} ms); glb {:.1}s, lighting {:.1}s, textures {:.1}s, total {:.1}s",
        tris as f64 / 1e6,
        draws.len(),
        mat_tex.len(),
        views.len(),
        light.light_count,
        light.probe_count,
        vb_mb,
        collision.node_count(),
        collision.build_ms,
        t_glb,
        t_light - t_glb,
        t_tex - t_light,
        t0.elapsed().as_secs_f64()
    );
    log::info!("{stats}");
    let fog = crate::fog::Fog::from_map(&cfg.bsp_dir(), light.sun_dir);
    let grading = load_grading(device, queue, &cfg.map_dir(), &cfg.bsp_dir());
    Ok(LoadedWorld { vbuf, ibuf, draws, mat_groups, probe_vbuf, light, sky_cube, fog, grading, collision, meta, stats })
}

