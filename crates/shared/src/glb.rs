//! Hand-written GLB reader for the extracted map (`world.glb` + `meta.json`).
//!
//! The file is ~250 MB, so it is memory-mapped and every primitive is decoded in parallel.
//! Two modes: `Geometry` (positions + indices only, for the server / collision) and `Full`
//! (also normals, both UV sets, vertex colours, materials and image byte ranges for the client).
//! Coordinates are left exactly as in the file: source units (inches), Z up.

use memmap2::Mmap;
use rayon::prelude::*;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Geometry,
    Full,
}

#[derive(Clone, Debug, Default)]
pub struct Material {
    pub name: String,
    pub base_color: [f32; 4],
    pub base_tex: Option<u32>,
    pub normal_tex: Option<u32>,
    /// 0 opaque, 1 mask (alpha cutout), 2 blend
    pub alpha_mode: u8,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
    pub lightmapped: bool,
    /// sky-shaded surface (unlit, samples the sky cubemap by view direction)
    pub sky: bool,
    /// never casts sun shadows (static prop flag 0x10, decal-class world meshes); extras `noShadow`
    pub no_shadow: bool,
    /// 3D skybox geometry (static prop flag 0x40): drawn only from the sky camera; extras `skybox`
    pub skybox: bool,
}

impl Material {
    /// Does this surface block the player? Grass, leaves, bushes, wires, cables and decals don't
    /// (tree trunks, stumps and logs do). Blended (glass/fx) surfaces never do.
    pub fn solid(&self) -> bool {
        if self.alpha_mode == 2 || self.sky || self.skybox {
            return false;
        }
        let n = self.name.to_ascii_lowercase();
        const SOFT: [&str; 14] = ["grass", "leaves", "leaf", "branch", "bush", "plant_", "fern", "roots_small", "wire", "cable", "hose", "decal", "vine", "lens"];
        !SOFT.iter().any(|k| n.contains(k))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Draw {
    pub material: u32,
    pub first_index: u32,
    pub index_count: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct ImageRef {
    pub offset: usize,
    pub len: usize,
    pub png: bool,
}

/// GPU-ready layout (32 bytes, no padding).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub nrm: [i8; 4],
    pub uv0: [f32; 2],
    /// lightmap atlas UV, unorm16
    pub uv1: [u16; 2],
    pub color: [u8; 4],
}

pub struct World {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// Only filled in `Full` mode.
    pub vertices: Vec<Vertex>,
    /// Per vertex (parallel to `vertices`), from export-map's `_LIGHTING` attribute: a static prop's own light
    /// probe (bits 0-23) + env cubemap (bits 24-31); u32::MAX where the glb has none (world geometry). Empty when
    /// no primitive carries it (older exports).
    pub lighting: Vec<u32>,
    pub draws: Vec<Draw>,
    pub materials: Vec<Material>,
    pub images: Vec<ImageRef>,
    pub lightmap_image: Option<u32>,
    pub bin: Option<Arc<Mmap>>,
    pub bin_start: usize,
}

impl World {
    pub fn image_bytes(&self, i: u32) -> Option<&[u8]> {
        let r = self.images.get(i as usize)?;
        let m = self.bin.as_ref()?;
        m.get(self.bin_start + r.offset..self.bin_start + r.offset + r.len)
    }
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Clone, Debug)]
pub struct Meta {
    /// x, y, z (feet), yaw (degrees)
    pub player_spawns: Vec<[f32; 4]>,
    pub sun_dir: [f32; 3],
    pub sun_color: [f32; 3],
    /// sky/ambient colour (linear)
    pub sun_ambient: [f32; 3],
    pub bounds: [f32; 6],
    pub sky_camera: [f32; 3],
    /// sky camera pitch/yaw/roll (degrees), after the level script's override
    pub sky_angles: [f32; 3],
    /// world units per skybox unit (m_skybox3d.scale, the sky_camera's "skyscale"); 0 = no 3D skybox
    pub sky_scale: f32,
    /// the first skybox vista's own light probe (LUMP 0x66) and env cubemap index
    pub sky_probe: Option<(u32, u32)>,
}

fn f(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}

pub fn load_meta(path: &Path) -> Result<Meta, String> {
    let s = std::fs::read_to_string(path).map_err(|e| format!("meta.json: {e}"))?;
    let v: Value = serde_json::from_str(&s).map_err(|e| format!("meta.json parse: {e}"))?;
    let arr = |x: &Value, n: usize| -> Vec<f32> {
        x.as_array().map(|a| a.iter().map(f).collect::<Vec<f32>>()).unwrap_or_default().into_iter().chain(std::iter::repeat(0.0)).take(n).collect()
    };
    let mut spawns = vec![];
    if let Some(a) = v.get("playerSpawns").and_then(|x| x.as_array()) {
        for s in a {
            let r = arr(s, 4);
            spawns.push([r[0], r[1], r[2], r[3]]);
        }
    }
    let sun = v.get("sun").cloned().unwrap_or(Value::Null);
    let d = arr(sun.get("dir").unwrap_or(&Value::Null), 3);
    let c = arr(sun.get("color").unwrap_or(&Value::Null), 3);
    let am = arr(sun.get("ambient").unwrap_or(&Value::Null), 3);
    let b = arr(v.get("bounds").unwrap_or(&Value::Null), 6);
    let sc = arr(v.get("skyCamera").unwrap_or(&Value::Null), 3);
    let sa = arr(v.get("skyCameraAngles").unwrap_or(&Value::Null), 3);
    Ok(Meta {
        player_spawns: spawns,
        sun_dir: if d.iter().all(|x| *x == 0.0) { [-0.53, 0.34, -0.78] } else { [d[0], d[1], d[2]] },
        sun_color: if c.iter().all(|x| *x == 0.0) { [0.8, 0.78, 0.72] } else { [c[0], c[1], c[2]] },
        sun_ambient: if am.iter().all(|x| *x == 0.0) { [0.42, 0.61, 0.8] } else { [am[0], am[1], am[2]] },
        bounds: [b[0], b[1], b[2], b[3], b[4], b[5]],
        sky_camera: [sc[0], sc[1], sc[2]],
        sky_angles: [sa[0], sa[1], sa[2]],
        sky_scale: v.get("skyScale").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
        sky_probe: v.get("skyboxProps").and_then(|a| a.get(0)).and_then(|p| Some((p.get("probe")?.as_u64()? as u32, p.get("cubemap")?.as_u64()? as u32))),
    })
}

struct Gltf<'a> {
    j: Value,
    bin: &'a [u8],
}

impl<'a> Gltf<'a> {
    /// Reads accessor `idx` as `ncomp` floats per element (any component type, normalized-aware).
    fn read_f32(&self, idx: usize, ncomp: usize) -> Vec<f32> {
        let acc = &self.j["accessors"][idx];
        let count = acc["count"].as_u64().unwrap_or(0) as usize;
        let ct = acc["componentType"].as_u64().unwrap_or(5126);
        let norm = acc["normalized"].as_bool().unwrap_or(false);
        let bv = &self.j["bufferViews"][acc["bufferView"].as_u64().unwrap_or(0) as usize];
        let base = bv["byteOffset"].as_u64().unwrap_or(0) as usize + acc["byteOffset"].as_u64().unwrap_or(0) as usize;
        let csize = match ct {
            5120 | 5121 => 1,
            5122 | 5123 => 2,
            _ => 4,
        };
        let stride = bv["byteStride"].as_u64().map(|s| s as usize).unwrap_or(csize * ncomp);
        let mut out = Vec::with_capacity(count * ncomp);
        for i in 0..count {
            let o = base + i * stride;
            for c in 0..ncomp {
                let p = o + c * csize;
                let v = match ct {
                    5126 => f32::from_le_bytes(self.bin[p..p + 4].try_into().unwrap()),
                    5120 => {
                        let x = self.bin[p] as i8 as f32;
                        if norm { (x / 127.0).max(-1.0) } else { x }
                    }
                    5121 => {
                        let x = self.bin[p] as f32;
                        if norm { x / 255.0 } else { x }
                    }
                    5122 => {
                        let x = i16::from_le_bytes(self.bin[p..p + 2].try_into().unwrap()) as f32;
                        if norm { (x / 32767.0).max(-1.0) } else { x }
                    }
                    5123 => {
                        let x = u16::from_le_bytes(self.bin[p..p + 2].try_into().unwrap()) as f32;
                        if norm { x / 65535.0 } else { x }
                    }
                    _ => u32::from_le_bytes(self.bin[p..p + 4].try_into().unwrap()) as f32,
                };
                out.push(v);
            }
        }
        out
    }

    /// Reads a SCALAR u32 accessor raw (no float conversion, which would lose bits above 2^24).
    fn read_u32(&self, idx: usize) -> Option<Vec<u32>> {
        let acc = &self.j["accessors"][idx];
        if acc["componentType"].as_u64() != Some(5125) {
            return None;
        }
        let count = acc["count"].as_u64().unwrap_or(0) as usize;
        let bv = &self.j["bufferViews"][acc["bufferView"].as_u64().unwrap_or(0) as usize];
        let base = bv["byteOffset"].as_u64().unwrap_or(0) as usize + acc["byteOffset"].as_u64().unwrap_or(0) as usize;
        let stride = bv["byteStride"].as_u64().map(|s| s as usize).unwrap_or(4);
        self.bin.get(base..base + count.saturating_sub(1) * stride + 4)?;
        Some((0..count).map(|i| u32::from_le_bytes(self.bin[base + i * stride..base + i * stride + 4].try_into().unwrap())).collect())
    }

    fn read_indices(&self, idx: usize) -> Vec<u32> {
        let acc = &self.j["accessors"][idx];
        let count = acc["count"].as_u64().unwrap_or(0) as usize;
        let ct = acc["componentType"].as_u64().unwrap_or(5123);
        let bv = &self.j["bufferViews"][acc["bufferView"].as_u64().unwrap_or(0) as usize];
        let base = bv["byteOffset"].as_u64().unwrap_or(0) as usize + acc["byteOffset"].as_u64().unwrap_or(0) as usize;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            out.push(match ct {
                5121 => self.bin[base + i] as u32,
                5123 => u16::from_le_bytes(self.bin[base + i * 2..base + i * 2 + 2].try_into().unwrap()) as u32,
                _ => u32::from_le_bytes(self.bin[base + i * 4..base + i * 4 + 4].try_into().unwrap()),
            });
        }
        out
    }
}

struct PrimOut {
    material: u32,
    alpha: u8,
    pos: Vec<[f32; 3]>,
    idx: Vec<u32>,
    verts: Vec<Vertex>,
    lighting: Option<Vec<u32>>,
}

fn parse_materials(j: &Value) -> Vec<Material> {
    let mut out = vec![];
    for m in j["materials"].as_array().cloned().unwrap_or_default() {
        let pbr = &m["pbrMetallicRoughness"];
        let bc = pbr["baseColorFactor"].as_array().map(|a| a.iter().map(f).collect::<Vec<_>>()).unwrap_or_else(|| vec![1.0; 4]);
        let tex_img = |t: &Value| -> Option<u32> {
            let ti = t["index"].as_u64()? as usize;
            j["textures"][ti]["source"].as_u64().map(|s| s as u32)
        };
        let alpha_mode = match m["alphaMode"].as_str() {
            Some("MASK") => 1,
            Some("BLEND") => 2,
            _ => 0,
        };
        out.push(Material {
            name: m["name"].as_str().unwrap_or("").to_string(),
            base_color: [bc[0], bc[1], bc[2], bc.get(3).copied().unwrap_or(1.0)],
            base_tex: tex_img(&pbr["baseColorTexture"]),
            normal_tex: tex_img(&m["normalTexture"]),
            alpha_mode,
            alpha_cutoff: m["alphaCutoff"].as_f64().unwrap_or(0.5) as f32,
            double_sided: m["doubleSided"].as_bool().unwrap_or(false),
            lightmapped: m["extras"]["lightmapped"].as_bool().unwrap_or(false),
            sky: m["extras"]["sky"].as_bool().unwrap_or(false),
            no_shadow: m["extras"]["noShadow"].as_bool().unwrap_or(false),
            skybox: m["extras"]["skybox"].as_bool().unwrap_or(false),
        });
    }
    out
}

/// Loads and merges the whole map. `skip_blend` drops alpha-blended primitives (glass/fx) — used
/// for collision so players don't snag on invisible-ish decoration.
pub fn load_world(path: &Path, mode: Mode, skip_blend: bool) -> Result<World, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let mmap = unsafe { Mmap::map(&file) }.map_err(|e| format!("mmap: {e}"))?;
    if mmap.len() < 20 || &mmap[0..4] != b"glTF" {
        return Err("not a GLB".into());
    }
    let jlen = u32::from_le_bytes(mmap[12..16].try_into().unwrap()) as usize;
    let jstart = 20;
    let j: Value = serde_json::from_slice(&mmap[jstart..jstart + jlen]).map_err(|e| format!("json: {e}"))?;
    let bstart = jstart + jlen + 8;
    let blen = u32::from_le_bytes(mmap[jstart + jlen..jstart + jlen + 4].try_into().unwrap()) as usize;
    let bin_slice = &mmap[bstart..(bstart + blen).min(mmap.len())];

    let g = Gltf { j, bin: bin_slice };
    let materials = parse_materials(&g.j);

    // Gather primitives (node -> mesh, all identity transforms in this map).
    let mut jobs: Vec<(usize, usize)> = vec![]; // (mesh, prim)
    for (mi, m) in g.j["meshes"].as_array().cloned().unwrap_or_default().iter().enumerate() {
        for pi in 0..m["primitives"].as_array().map(|a| a.len()).unwrap_or(0) {
            jobs.push((mi, pi));
        }
    }

    let full = mode == Mode::Full;
    let mut prims: Vec<PrimOut> = jobs
        .par_iter()
        .filter_map(|&(mi, pi)| {
            let p = &g.j["meshes"][mi]["primitives"][pi];
            if p["mode"].as_u64().unwrap_or(4) != 4 {
                return None;
            }
            let matid = p["material"].as_u64().unwrap_or(0) as usize;
            let alpha = materials.get(matid).map(|m| m.alpha_mode).unwrap_or(0);
            if skip_blend && alpha == 2 {
                return None;
            }
            let a = &p["attributes"];
            let ipos = a["POSITION"].as_u64()? as usize;
            let posf = g.read_f32(ipos, 3);
            let n = posf.len() / 3;
            let idx = g.read_indices(p["indices"].as_u64()? as usize);
            let pos: Vec<[f32; 3]> = posf.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
            let mut verts = Vec::new();
            let mut lighting = None;
            if full {
                lighting = a["_LIGHTING"].as_u64().and_then(|i| g.read_u32(i as usize)).filter(|l| l.len() == n);
                let nrm = a["NORMAL"].as_u64().map(|i| g.read_f32(i as usize, 3));
                let uv0 = a["TEXCOORD_0"].as_u64().map(|i| g.read_f32(i as usize, 2));
                let uv1 = a["TEXCOORD_1"].as_u64().map(|i| g.read_f32(i as usize, 2));
                let col = a["COLOR_0"].as_u64().map(|i| {
                    let ncomp = if g.j["accessors"][i as usize]["type"].as_str() == Some("VEC3") { 3 } else { 4 };
                    (g.read_f32(i as usize, ncomp), ncomp)
                });
                verts.reserve(n);
                for i in 0..n {
                    let mut v = Vertex { pos: pos[i], nrm: [0, 0, 127, 0], uv0: [0.0; 2], uv1: [0; 2], color: [255; 4] };
                    if let Some(nr) = &nrm {
                        let q = |x: f32| (x.clamp(-1.0, 1.0) * 127.0).round() as i8;
                        v.nrm = [q(nr[i * 3]), q(nr[i * 3 + 1]), q(nr[i * 3 + 2]), 0];
                    }
                    if let Some(u) = &uv0 {
                        v.uv0 = [u[i * 2], u[i * 2 + 1]];
                    }
                    if let Some(u) = &uv1 {
                        let q = |x: f32| (x.clamp(0.0, 1.0) * 65535.0).round() as u16;
                        v.uv1 = [q(u[i * 2]), q(u[i * 2 + 1])];
                    }
                    if let Some((c, nc)) = &col {
                        let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
                        v.color = [q(c[i * nc]), q(c[i * nc + 1]), q(c[i * nc + 2]), if *nc == 4 { q(c[i * nc + 3]) } else { 255 }];
                    }
                    verts.push(v);
                }
            }
            Some(PrimOut { material: matid as u32, alpha, pos: if full { Vec::new() } else { pos }, idx, verts, lighting })
        })
        .collect();

    // Vertex offsets are in original primitive order; index order is by (alpha, material).
    let mut base = Vec::with_capacity(prims.len());
    let mut acc = 0u32;
    for p in &prims {
        base.push(acc);
        acc += if full { p.verts.len() as u32 } else { p.pos.len() as u32 };
    }
    let mut order: Vec<usize> = (0..prims.len()).collect();
    order.sort_by_key(|&i| (prims[i].alpha, prims[i].material));

    let total_idx: usize = prims.iter().map(|p| p.idx.len()).sum();
    let mut positions: Vec<[f32; 3]> = Vec::with_capacity(acc as usize);
    let mut vertices: Vec<Vertex> = Vec::new();
    if full {
        vertices.reserve(acc as usize);
    }
    let mut lighting: Vec<u32> = Vec::new();
    let any_lighting = prims.iter().any(|p| p.lighting.is_some());
    if any_lighting {
        lighting.reserve(acc as usize);
    }
    for p in prims.iter_mut() {
        if any_lighting {
            match p.lighting.take() {
                Some(l) => lighting.extend_from_slice(&l),
                None => lighting.resize(lighting.len() + p.verts.len(), u32::MAX),
            }
        }
        if full {
            vertices.append(&mut p.verts);
        } else {
            positions.append(&mut p.pos);
        }
    }
    let mut indices: Vec<u32> = Vec::with_capacity(total_idx);
    let mut draws: Vec<Draw> = vec![];
    for &i in &order {
        let p = &prims[i];
        let first = indices.len() as u32;
        let b = base[i];
        indices.extend(p.idx.iter().map(|x| x + b));
        match draws.last_mut() {
            Some(d) if d.material == p.material && d.first_index + d.index_count == first => d.index_count += p.idx.len() as u32,
            _ => draws.push(Draw { material: p.material, first_index: first, index_count: p.idx.len() as u32 }),
        }
    }
    if full {
        positions = vertices.iter().map(|v| v.pos).collect();
    }

    let mut images = vec![];
    let mut lightmap_image = None;
    if full {
        for (ii, im) in g.j["images"].as_array().cloned().unwrap_or_default().iter().enumerate() {
            let bv = &g.j["bufferViews"][im["bufferView"].as_u64().unwrap_or(0) as usize];
            images.push(ImageRef {
                offset: bv["byteOffset"].as_u64().unwrap_or(0) as usize,
                len: bv["byteLength"].as_u64().unwrap_or(0) as usize,
                png: im["mimeType"].as_str() == Some("image/png"),
            });
            if im["name"].as_str() == Some("lightmap") {
                lightmap_image = Some(ii as u32);
            }
        }
        if lightmap_image.is_none() {
            // fall back to the lightmapTexture named in the first lightmapped material's extras
            for m in g.j["materials"].as_array().cloned().unwrap_or_default() {
                if let Some(t) = m["extras"]["lightmapTexture"].as_u64() {
                    lightmap_image = Some(t as u32);
                    break;
                }
            }
        }
    }

    Ok(World {
        positions,
        indices,
        vertices,
        lighting,
        draws,
        materials,
        images,
        lightmap_image,
        bin: if full { Some(Arc::new(mmap)) } else { None },
        bin_start: bstart,
    })
}
