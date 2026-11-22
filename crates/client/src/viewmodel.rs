//! First-person weapon + arms (retail viewmodel, tools/build_weapon.py -> `weapons/<name>/view.bin`): skinned on the
//! GPU (world.wgsl `vs_viewmodel`), lit by the same `shade()` as props through the nearest light probe, and animated
//! with the game's own sequences: an absolute base pose (draw, ADS in/out, reload, sprint) with additive layers on
//! top (idle sway, fire kick), crossfaded when the base changes.
//!
//! Rig space: the Cast rig is Y up / Z forward / X left (r5ed's "vertex space"); `RIG_TO_SOURCE` turns it into
//! Source axes (X forward, Y left, Z up). The animated `jx_c_camera` bone is pinned to the eye, like the game
//! attaches the first-person camera to it.

use crate::loader::{solid, MatUniform};
use crate::texcache::{self, Kind};
use glam::{Affine3A, Mat3A, Quat, Vec3, Vec3A};
use std::path::Path;
use wgpu::util::DeviceExt;

/// Animation space -> Source view space. The skin matrices (anim world x inverse bind) land vertices in the
/// sequences' frame, not the bind pose's: there the camera bone sits at the origin, Z is up, the gun points down -Y
/// and X is sideways (checked on ads_out/draw: muzzle at y -26). Source: X forward, Y left, Z up. RSX's Cast
/// output is mirrored against the game (its triangles wind the other way, see export-map's parse_cast_lod), and
/// rendered with a plain turn the weapon came out upside down in the top right: (x, y, z) -> (-y, x, -z).
const RIG_TO_SOURCE: Mat3A = Mat3A::from_cols(Vec3A::new(0.0, 1.0, 0.0), Vec3A::new(-1.0, 0.0, 0.0), Vec3A::new(0.0, 0.0, 1.0));

pub struct Anim {
    pub fps: f32,
    pub frames: u32,
    pub additive: bool,
    /// frames x bones
    data: Vec<(Vec3, Quat)>,
}

impl Anim {
    pub fn duration(&self) -> f32 {
        (self.frames.max(1) - 1) as f32 / self.fps
    }
    /// local pose at `t` seconds (clamped), into `out`
    fn sample(&self, t: f32, bones: usize, out: &mut [(Vec3, Quat)]) {
        let f = (t * self.fps).clamp(0.0, (self.frames.max(1) - 1) as f32);
        let (a, k) = (f.floor() as usize, f.fract());
        let b = (a + 1).min(self.frames as usize - 1);
        for i in 0..bones {
            let (pa, qa) = self.data[a * bones + i];
            let (pb, qb) = self.data[b * bones + i];
            out[i] = (pa.lerp(pb, k), qa.slerp(qb, k));
        }
    }
}

struct Mesh {
    first: u32,
    count: u32,
    material: usize,
}

/// One sequence of the rig, as the game's seqdesc describes it (tools/vmbake.py "SEQS", from the retail QC).
#[derive(Clone, Debug, Default)]
pub struct Seq {
    pub name: String,
    pub activity: String,
    pub weight: f32,
    pub mods: Vec<String>,
    pub fadein: f32,
    pub fadeout: f32,
    /// node graph: `node "x"` = both, `transition "a" "b"` = entry a, exit b ("" = none)
    pub entry: String,
    pub exit: String,
    /// 1 loop, 2 delta (additive), 4 autoplay, 8 snap
    pub flags: u8,
    /// its animations (blend grid cells, row-major, `blendwidth` per row)
    pub anims: Vec<String>,
    pub blendwidth: usize,
    /// pose parameter, range
    pub blends: Vec<(String, f32, f32)>,
    /// addlayer sequences (played with this sequence's cycle and weight)
    pub layers: Vec<usize>,
    layer_names: Vec<String>,
    /// per-bone weights (None = every bone 1)
    pub weights: Option<Vec<f32>>,
    /// seconds at rate 1 (its first animation)
    pub duration: f32,
}

impl Seq {
    pub fn looping(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn delta(&self) -> bool {
        self.flags & 2 != 0
    }
    pub fn autoplay(&self) -> bool {
        self.flags & 4 != 0
    }
    pub fn snap(&self) -> bool {
        self.flags & 8 != 0
    }
}

/// A sequence event (retail's QC `event`): `AE_CL_PLAYSOUND` + a Miles event name, `AE_WPN_CLIPBODYGROUP_HIDE`,
/// `AE_CL_CREATEPARTICLEEFFECT` + effect / attachment, ... at `t` seconds into the sequence.
#[derive(Clone, Debug)]
pub struct AnimEvent {
    pub t: f32,
    pub kind: String,
    pub opt: String,
}

/// GPU per-frame data (group 2)
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct VmFrame {
    clip: [[f32; 4]; 4],
    probe: u32,
    pad: [u32; 3],
}

pub struct Viewmodel {
    names: Vec<String>,
    parents: Vec<i32>,
    bind_local: Vec<(Vec3, Quat)>,
    inv_bind: Vec<Affine3A>,
    pub anims: std::collections::HashMap<String, Anim>,
    /// per anim, its events sorted by time (tools/build_weapon.py "EVTS" section)
    pub events: std::collections::HashMap<String, Vec<AnimEvent>>,
    /// per mesh, its Cast mesh name (bodygroups: "clip_0_" is the magazine)
    mesh_names: Vec<String>,
    /// the pose every frame starts from (the model's default bone pose; vmbake "REFP"), else the bind pose
    ref_local: Vec<(Vec3, Quat)>,
    /// every sequence of the rig (tools/vmbake.py "SEQS"), and by name
    pub seqs: Vec<Seq>,
    pub seq_by_name: std::collections::HashMap<String, usize>,
    /// meshes hidden by bodygroup events (bit per mesh)
    hidden: std::cell::Cell<u64>,
    camera_bone: usize,
    vbuf: wgpu::Buffer,
    sbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    meshes: Vec<Mesh>,
    /// its materials until Gfx registers them (gfx.load_viewmodel), then their first index in the global table
    pub(crate) mats: Option<crate::loader::MatSet>,
    pub(crate) mat_base: u32,
    /// per material: alpha blended (weapon.json "translucent"), drawn in a second pass
    translucent: Vec<bool>,
    bones_buf: wgpu::Buffer,
    frame_buf: wgpu::Buffer,
    pub group: wgpu::BindGroup,
    pub tris: u32,
}

/// The skinned vertex stream's layout (locations 10, 11) next to the world vertex layout.
pub const SKIN_STRIDE: u64 = 8;

pub fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("viewmodel"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            },
        ],
    })
}

struct Rd<'a>(&'a [u8], usize);
impl Rd<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self.0.get(self.1..self.1 + n).ok_or("view.bin truncated")?;
        self.1 += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32s<const N: usize>(&mut self) -> Result<[f32; N], String> {
        let b = self.take(4 * N)?;
        Ok(std::array::from_fn(|i| f32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap())))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = u16::from_le_bytes(self.take(2)?.try_into().unwrap()) as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }
}

impl Viewmodel {
    /// `dir` = assets root / weapons / <weapon>
    pub fn load(device: &wgpu::Device, queue: &wgpu::Queue, vm_layout: &wgpu::BindGroupLayout, root: &Path, dir: &Path) -> Result<Viewmodel, String> {
        let b = std::fs::read(dir.join("view.bin")).map_err(|e| format!("{}: {e}", dir.join("view.bin").display()))?;
        if b.get(0..4) != Some(b"R5VM") {
            return Err("view.bin: bad magic".into());
        }
        let mut r = Rd(&b, 8);
        let nb = r.u32()? as usize;
        let (mut names, mut parents, mut bind_local) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..nb {
            names.push(r.str()?);
            parents.push(i32::from_le_bytes(r.take(4)?.try_into().unwrap()));
            let p = r.f32s::<3>()?;
            let q = r.f32s::<4>()?;
            bind_local.push((Vec3::from(p), Quat::from_array(q).normalize()));
        }
        let world = world_pose(&parents, &bind_local);
        let inv_bind: Vec<Affine3A> = world.iter().map(|m| m.inverse()).collect();

        let nm = r.u32()? as usize;
        let (mut verts, mut skin, mut idx, mut meshes, mut mat_names) = (Vec::<shared::glb::Vertex>::new(), Vec::<[u8; 8]>::new(), Vec::<u32>::new(), Vec::new(), Vec::<String>::new());
        for _ in 0..nm {
            let mat = r.str()?;
            let (nv, ni) = (r.u32()? as usize, r.u32()? as usize);
            let base = verts.len() as u32;
            for _ in 0..nv {
                let p = r.f32s::<3>()?;
                let n = r.f32s::<3>()?;
                let uv = r.f32s::<2>()?;
                let bw = r.take(8)?;
                let q = |x: f32| (x.clamp(-1.0, 1.0) * 127.0).round() as i8;
                verts.push(shared::glb::Vertex { pos: p, nrm: [q(n[0]), q(n[1]), q(n[2]), 0], uv0: uv, uv1: [0; 2], color: [255; 4] });
                skin.push(bw.try_into().unwrap());
            }
            let first = idx.len() as u32;
            for _ in 0..ni {
                idx.push(base + r.u32()?);
            }
            let mi = match mat_names.iter().position(|m| *m == mat) {
                Some(i) => i,
                None => {
                    mat_names.push(mat.clone());
                    mat_names.len() - 1
                }
            };
            meshes.push(Mesh { first, count: ni as u32, material: mi });
        }
        let na = r.u32()? as usize;
        let mut anims = std::collections::HashMap::new();
        for _ in 0..na {
            let name = r.str()?;
            let fps = r.f32s::<1>()?[0];
            let frames = r.u32()?;
            let additive = r.take(1)?[0] != 0;
            let mut data = Vec::with_capacity(frames as usize * nb);
            for _ in 0..frames as usize * nb {
                let p = r.f32s::<3>()?;
                let q = r.f32s::<4>()?;
                data.push((Vec3::from(p), Quat::from_array(q).normalize()));
            }
            anims.insert(name, Anim { fps, frames, additive, data });
        }
        // optional sections: "EVTS" (per anim: events), "MNMS" (mesh names)
        let mut events = std::collections::HashMap::new();
        let mut mesh_names = Vec::new();
        let mut seqs: Vec<Seq> = Vec::new();
        let mut ref_local = bind_local.clone();
        while let Ok(tag) = r.take(4).map(|t| t.to_vec()) {
            match &tag[..] {
                b"REFP" => {
                    let n = r.u32()? as usize;
                    for i in 0..n {
                        let p = r.f32s::<3>()?;
                        let q = r.f32s::<4>()?;
                        if i < ref_local.len() {
                            ref_local[i] = (Vec3::from(p), Quat::from_array(q).normalize());
                        }
                    }
                }
                b"SEQS" => {
                    for _ in 0..r.u32()? {
                        let mut s = Seq { name: r.str()?, activity: r.str()?, ..Default::default() };
                        s.weight = r.f32s::<1>()?[0];
                        for _ in 0..r.take(1)?[0] {
                            s.mods.push(r.str()?);
                        }
                        let [fi, fo] = r.f32s::<2>()?;
                        s.fadein = fi;
                        s.fadeout = fo;
                        s.entry = r.str()?;
                        s.exit = r.str()?;
                        s.flags = r.take(1)?[0];
                        let _fps = r.f32s::<1>()?[0];
                        for _ in 0..r.take(1)?[0] {
                            s.anims.push(r.str()?);
                        }
                        let bw = r.take(2)?.to_vec();
                        s.blendwidth = (bw[0] as usize).max(1);
                        for _ in 0..bw[1] {
                            let p = r.str()?;
                            let [lo, hi] = r.f32s::<2>()?;
                            s.blends.push((p, lo, hi));
                        }
                        for _ in 0..r.take(1)?[0] {
                            s.layer_names.push(r.str()?);
                        }
                        let nw = r.u32()? as usize;
                        if nw > 0 {
                            let mut w = Vec::with_capacity(nw);
                            for _ in 0..nw {
                                w.push(r.f32s::<1>()?[0]);
                            }
                            s.weights = Some(w);
                        }
                        seqs.push(s);
                    }
                }
                b"EVTS" => {
                    for _ in 0..r.u32()? {
                        let anim = r.str()?;
                        let n = r.u32()? as usize;
                        let mut v = Vec::with_capacity(n);
                        for _ in 0..n {
                            let t = r.f32s::<1>()?[0];
                            v.push(AnimEvent { t, kind: r.str()?, opt: r.str()? });
                        }
                        v.sort_by(|a: &AnimEvent, b| a.t.total_cmp(&b.t));
                        events.insert(anim, v);
                    }
                }
                b"MNMS" => {
                    for _ in 0..r.u32()? {
                        mesh_names.push(r.str()?);
                    }
                }
                _ => break,
            }
        }

        // materials: the retail textures by slot (weapon.json), through the props' material uniform
        let wj: serde_json::Value = std::fs::read_to_string(dir.join("weapon.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let cache = root.parent().unwrap_or(root).join("_cache").join("tex");
        use wgpu::TextureFormat as F;
        let white = solid(device, queue, [255; 4], F::Rgba8UnormSrgb, "white");
        let flat_n = solid(device, queue, [128, 128, 0, 255], F::Rgba8Unorm, "flat normal");
        let gloss_d = solid(device, queue, [64, 64, 64, 255], F::Rgba8Unorm, "gloss");
        let spec_d = solid(device, queue, [56, 56, 56, 255], F::Rgba8UnormSrgb, "spec 0.04");
        let one = solid(device, queue, [255; 4], F::Rgba8Unorm, "ao 1");
        let black = solid(device, queue, [0, 0, 0, 255], F::Rgba8UnormSrgb, "black");
        let grey = solid(device, queue, [128, 128, 128, 255], F::Rgba8Unorm, "detail 0.5");
        // heirloom extras (tools/heirloom_mats.py): the scrolling emissive multiply + its distortion normal, the glass
        // opacity mask; constant gloss / F0 where a material has no texture for them (retail's perf constants)
        let grey_v = |v: f32, srgb: bool, label: &str| {
            let b = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            solid(device, queue, [b, b, b, 255], if srgb { F::Rgba8UnormSrgb } else { F::Rgba8Unorm }, label)
        };
        let tex = |m: &str, slot: &str, kind: Kind| -> Option<wgpu::TextureView> {
            let rel = wj["materials"][m][slot].as_str()?;
            let e = texcache::load(root, &cache, rel, kind, 4096)?;
            Some(texcache::upload(device, queue, &e, 0).0.create_view(&Default::default()))
        };
        let translucent: Vec<bool> = mat_names.iter().map(|m| wj["materials"][m.as_str()]["translucent"].as_bool().unwrap_or(false)).collect();
        // per material its own textures (they're dropped with this scope; the views keep them alive)
        let owned: Vec<([Option<wgpu::TextureView>; 9], bool)> = mat_names
            .iter()
            .map(|m| {
                let (gl, sp) = (tex(m, "gloss", Kind::Gloss), tex(m, "spec", Kind::Spec));
                let jm = &wj["materials"][m.as_str()];
                let fx = |k: &str, i: usize, d: f32| jm[k][i].as_f64().map_or(d, |v| v as f32);
                let gl_c = (gl.is_none() && jm["perf_gloss"].is_number()).then(|| grey_v(jm["perf_gloss"].as_f64().unwrap_or(0.25) as f32, false, "perf gloss"));
                // F0 is linear; the spec slot is an sRGB texture
                let sp_c = (sp.is_none() && jm["perf_spec"].is_array()).then(|| grey_v(fx("perf_spec", 0, 0.04).powf(1.0 / 2.2), true, "perf spec"));
                let has_spec = sp.is_some();
                ([tex(m, "albedo", Kind::Albedo), tex(m, "normal", Kind::Normal), gl.or(gl_c), sp.or(sp_c), tex(m, "ao", Kind::Ao), tex(m, "emissive", Kind::Emissive), tex(m, "emissive_mul", Kind::Emissive), tex(m, "opacity_mul", Kind::Ao), tex(m, "distortion", Kind::Normal)], has_spec)
            })
            .collect();
        let mut ti = crate::loader::TexIndex::new();
        let mat_list: Vec<MatUniform> = mat_names
            .iter()
            .enumerate()
            .map(|(mi, m)| {
                let ([al, nr, gl, sp, ao, em, emul, opac, dist], has_spec) = &owned[mi];
                let jm = &wj["materials"][m.as_str()];
                let glass = jm["translucent"].as_bool().unwrap_or(false) && opac.is_some();
                let fx = |k: &str, i: usize, d: f32| jm[k][i].as_f64().map_or(d, |v| v as f32);
                let uvf = |n: usize, i: usize, d: f32| jm["uv"][n][i].as_f64().map_or(d, |v| v as f32);
                // uv rows: RotScaleX (angle, scaleU), RotScaleY (pivot, scaleV), Translate
                let uvl = |n: usize| [uvf(n, 0, 0.0), uvf(n, 1, 1.0), uvf(n, 3, 1.0), 0.0];
                if al.is_none() {
                    log::warn!("viewmodel material {m}: no textures in weapon.json");
                }
                let u = MatUniform {
                    color: [1.0; 4],
                    params: [0.5, 0.0, 0.0, 0.0],
                    params2: [ao.is_some() as u32 as f32, em.is_some() as u32 as f32, 0.0, 0.0],
                    albedo_tint: [1.0; 4],
                    // per material from weapon.json (heirlooms: off until their constants are read; retail tints the mask)
                    emissive_tint: { let t = &wj["materials"][m]["emissive_tint"]; [t[0].as_f64().unwrap_or(1.0) as f32, t[1].as_f64().unwrap_or(1.0) as f32, t[2].as_f64().unwrap_or(1.0) as f32, 1.0] },
                    emissive_edge: [0.0, 1.0, 1.0, 0.0],
                    blend: [0.0, 3.0, 0.0, 0.0],
                    uv1: [1.0, 0.0, 0.0, 1.0],
                    uv_t: [0.0; 4],
                    uv2: [1.0, 0.0, 0.0, 1.0],
                    albedo2_tint: [1.0; 4],
                    layer2: [0.0, 0.0, 0.25, 0.25],
                    perf_spec: [0.04, 0.04, 0.04, *has_spec as u32 as f32],
                    perf_spec2: [0.04, 0.04, 0.04, 1.0],
                    uv_rs: [[1.0, 0.0, 0.0, 1.0]; 5],
                    uv_tr: [[0.0; 4]; 5],
                    route: [[0; 4]; 4],
                    detail: [0.0, 0.0, 0.0, 1000.0 + mi as f32],
                    vmfx: [
                        if glass { 2.0 } else if emul.is_some() && em.is_some() { 1.0 } else { 0.0 },
                        fx("opacity", 0, 1.0),
                        fx("distortion", 0, 0.0),
                        fx("distortion", 1, 0.0),
                    ],
                    vm_uv1: uvl(0),
                    vm_uv2: uvl(1),
                    vm_tr: [uvf(0, 4, 0.0), uvf(0, 5, 0.0), uvf(1, 4, 0.0), uvf(1, 5, 0.0)],
                    emis2: [0.0; 4],
                    fx2: [0.0; 4],
                    zup: [0.0; 4],
                    opac: [0.0; 4],
                    opac_edge: [1.0, 1.0, 1.0, 0.0],
                    // slot 6 / 10 / 11 carry the heirloom extras (emissive multiply, opacity, distortion): mat.wgsl
                    tix: ti.tix([
                        al.as_ref().unwrap_or(&white),
                        nr.as_ref().unwrap_or(&flat_n),
                        gl.as_ref().unwrap_or(&gloss_d),
                        sp.as_ref().unwrap_or(&spec_d),
                        ao.as_ref().unwrap_or(&one),
                        em.as_ref().unwrap_or(&black),
                        emul.as_ref().unwrap_or(&white),
                        &flat_n,
                        &gloss_d,
                        &spec_d,
                        opac.as_ref().unwrap_or(&one),
                        dist.as_ref().unwrap_or(&flat_n),
                        &grey,
                        &grey,
                        &white,
                        &black,
                    ]),
                };
                u
            })
            .collect();
        let mats = Some(crate::loader::MatSet { views: ti.views(), mats: mat_list });

        let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("vm vb"), contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX });
        let sbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("vm skin"), contents: bytemuck::cast_slice(&skin), usage: wgpu::BufferUsages::VERTEX });
        let ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("vm ib"), contents: bytemuck::cast_slice(&idx), usage: wgpu::BufferUsages::INDEX });
        let bones_buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("vm bones"), size: (nb.max(1) * 48) as u64, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("vm frame"), size: std::mem::size_of::<VmFrame>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewmodel"),
            layout: vm_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: bones_buf.as_entire_binding() }, wgpu::BindGroupEntry { binding: 1, resource: frame_buf.as_entire_binding() }],
        });
        let camera_bone = names.iter().position(|n| n == "jx_c_camera").unwrap_or(0);
        log::info!("viewmodel {}: {nb} bones, {} meshes, {} tris, {} anims, materials {mat_names:?}", dir.display(), meshes.len(), idx.len() / 3, anims.len());
        let seq_by_name: std::collections::HashMap<String, usize> = seqs.iter().enumerate().map(|(i, s)| (s.name.clone(), i)).collect();
        for s in seqs.iter_mut() {
            s.layers = s.layer_names.iter().filter_map(|n| seq_by_name.get(n).copied()).collect();
            s.duration = s.anims.first().and_then(|a| anims.get(a)).map_or(0.0, |a: &Anim| a.duration());
        }
        Ok(Viewmodel { names, parents, bind_local, inv_bind, anims, events, mesh_names, ref_local, seqs, seq_by_name, hidden: std::cell::Cell::new(0), camera_bone, vbuf, sbuf, ibuf, meshes, mats, mat_base: 0, translucent, bones_buf, frame_buf, group, tris: (idx.len() / 3) as u32 })
    }

    pub fn bone_count(&self) -> usize {
        self.names.len()
    }

    /// The reference pose (InitPose): what every frame's animation starts from.
    pub fn bind_pose(&self) -> Vec<(Vec3, Quat)> {
        self.ref_local.clone()
    }

    /// Local pose of `anim` at `t` (absolute anims replace, additive ones are deltas).
    pub fn sample(&self, anim: &str, t: f32, out: &mut [(Vec3, Quat)]) -> bool {
        match self.anims.get(anim) {
            Some(a) => {
                a.sample(t, self.names.len(), out);
                true
            }
            None => false,
        }
    }

    /// Uploads the skin matrices for `pose` (local, rig space), placed at the eye with the view rotation `view`
    /// (camera-relative world space), with the sway matrix `sway` (viewmodel-local, Source axes; crate::sway), and the
    /// viewmodel projection.
    pub fn upload(&self, queue: &wgpu::Queue, pose: &[(Vec3, Quat)], view: Mat3A, sway: Affine3A, clip: glam::Mat4, probe: u32) {
        let world = world_pose(&self.parents, pose);
        // pin the animated camera bone to the eye (unswayed: sway moves the model, not the eye)
        let cam = RIG_TO_SOURCE * Vec3A::from(world[self.camera_bone].translation);
        let rows: Vec<[[f32; 4]; 3]> = world
            .iter()
            .zip(&self.inv_bind)
            .map(|(w, ib)| {
                let s = *w * *ib;
                let m = view * sway.matrix3 * RIG_TO_SOURCE * s.matrix3;
                let t = view * (sway.transform_point3a(RIG_TO_SOURCE * s.translation) - cam);
                [[m.x_axis.x, m.y_axis.x, m.z_axis.x, t.x], [m.x_axis.y, m.y_axis.y, m.z_axis.y, t.y], [m.x_axis.z, m.y_axis.z, m.z_axis.z, t.z]]
            })
            .collect();
        queue.write_buffer(&self.bones_buf, 0, bytemuck::cast_slice(&rows));
        queue.write_buffer(&self.frame_buf, 0, bytemuck::bytes_of(&VmFrame { clip: clip.to_cols_array_2d(), probe, pad: [0; 3] }));
    }

    /// A bone's model-space (rig) transform in `pose`.
    pub fn bone_world(&self, pose: &[(Vec3, Quat)], bone: usize) -> Affine3A {
        let world = world_pose(&self.parents, pose);
        world[bone.min(world.len() - 1)]
    }

    /// Uploads `pose` placed by `model` (rig space -> camera-relative world space), for a model anchored in the
    /// world rather than to the eye (the first-person proxy on a ledge).
    pub fn upload_world(&self, queue: &wgpu::Queue, pose: &[(Vec3, Quat)], model: Affine3A, clip: glam::Mat4, probe: u32) {
        let world = world_pose(&self.parents, pose);
        let rows: Vec<[[f32; 4]; 3]> = world
            .iter()
            .zip(&self.inv_bind)
            .map(|(w, ib)| {
                let s = model * *w * *ib;
                let (m, t) = (s.matrix3, s.translation);
                [[m.x_axis.x, m.y_axis.x, m.z_axis.x, t.x], [m.x_axis.y, m.y_axis.y, m.z_axis.y, t.y], [m.x_axis.z, m.y_axis.z, m.z_axis.z, t.z]]
            })
            .collect();
        queue.write_buffer(&self.bones_buf, 0, bytemuck::cast_slice(&rows));
        queue.write_buffer(&self.frame_buf, 0, bytemuck::bytes_of(&VmFrame { clip: clip.to_cols_array_2d(), probe, pad: [0; 3] }));
    }

    /// SelectWeightedSequence (S21 FUN_1409df7e0): among `act`'s sequences, score = +1 per modifier the player has,
    /// -1001 per modifier the sequence wants that the player lacks; the best score wins, ties weighted-random by the
    /// activity weight (`r` in 0..1). Extra player modifiers never count against a sequence.
    pub fn pick_seq(&self, act: &str, active: &[&str], r: f32) -> Option<usize> {
        let score = |s: &Seq| s.mods.iter().map(|m| if active.contains(&m.as_str()) { 1 } else { -1001 }).sum::<i32>();
        let cands: Vec<usize> = (0..self.seqs.len()).filter(|&i| self.seqs[i].activity == act).collect();
        let best = cands.iter().map(|&i| score(&self.seqs[i])).max()?;
        let top: Vec<usize> = cands.into_iter().filter(|&i| score(&self.seqs[i]) == best).collect();
        let total: f32 = top.iter().map(|&i| self.seqs[i].weight.max(0.0)).sum();
        let mut x = r * total;
        for &i in &top {
            x -= self.seqs[i].weight.max(0.0);
            if x <= 0.0 {
                return Some(i);
            }
        }
        top.last().copied()
    }

    /// FindTransitionSequence (S21 FUN_1409dfdf0): the sequence linking `cur`'s exit node to `goal`'s entry node (one
    /// hop), else `goal`. Of two sequences on the same pair, the one without an activity (the dedicated transition).
    pub fn find_transition(&self, cur: usize, goal: usize) -> usize {
        let (from, to) = (&self.seqs[cur].exit, &self.seqs[goal].entry);
        if from.is_empty() || to.is_empty() || from == to {
            return goal;
        }
        let hits: Vec<usize> = (0..self.seqs.len()).filter(|&i| &self.seqs[i].entry == from && &self.seqs[i].exit == to && self.seqs[i].entry != self.seqs[i].exit).collect();
        hits.iter().copied().find(|&i| self.seqs[i].activity.is_empty()).or(hits.first().copied()).unwrap_or(goal)
    }

    /// A sequence's local pose at `cycle` (0..1): its blend grid sampled by the pose parameters (CalcPoseSingle:
    /// `blendwidth` columns on the first blend, rows on the second; a single blend with width 1 runs down the rows).
    /// Absolute sequences give transforms, delta ones give deltas.
    pub fn sample_seq(&self, si: usize, cycle: f32, param: &dyn Fn(&str) -> f32, out: &mut [(Vec3, Quat)]) {
        let s = &self.seqs[si];
        let n = s.anims.len();
        if n == 0 {
            return;
        }
        let w = s.blendwidth.min(n).max(1);
        let rows = (n / w).max(1);
        let norm = |b: Option<&(String, f32, f32)>, cells: usize| -> (usize, usize, f32) {
            match b {
                Some((p, lo, hi)) if cells > 1 => {
                    let x = ((param(p) - lo) / (hi - lo).max(1e-6)).clamp(0.0, 1.0) * (cells - 1) as f32;
                    let i = (x.floor() as usize).min(cells - 1);
                    (i, (i + 1).min(cells - 1), x - i as f32)
                }
                _ => (0, 0, 0.0),
            }
        };
        let (cb, rb) = if w == 1 && rows > 1 { (None, s.blends.first()) } else { (s.blends.first(), s.blends.get(1)) };
        let (c0, c1, cf) = norm(cb, w);
        let (r0, r1, rf) = norm(rb, rows);
        let nb = self.names.len();
        let cell = |r: usize, c: usize, dst: &mut [(Vec3, Quat)]| {
            if let Some(a) = s.anims.get(r * w + c).and_then(|a| self.anims.get(a)) {
                a.sample(cycle * a.duration(), nb, dst);
            }
        };
        cell(r0, c0, out);
        let mut tmp = out.to_vec();
        if cf > 0.0 && c1 != c0 {
            cell(r0, c1, &mut tmp);
            blend(out, &tmp, cf);
        }
        if rf > 0.0 && r1 != r0 {
            let mut row = out.to_vec();
            cell(r1, c0, &mut row);
            if cf > 0.0 && c1 != c0 {
                cell(r1, c1, &mut tmp);
                blend(&mut row, &tmp, cf);
            }
            blend(out, &row, rf);
        }
    }

    pub fn parent(&self, b: usize) -> i32 {
        self.parents[b]
    }

    /// Events of `anim` with t0 < t <= t1 (t0 < 0 includes events at 0).
    pub fn events_between(&self, anim: &str, t0: f32, t1: f32, out: &mut Vec<AnimEvent>) {
        if let Some(v) = self.events.get(anim) {
            out.extend(v.iter().filter(|e| e.t > t0 && e.t <= t1).cloned());
        }
    }

    /// Shows / hides every mesh whose name starts with `prefix` (bodygroup events; "clip" = the magazine).
    pub fn set_group_visible(&self, prefix: &str, on: bool) {
        let mut m = self.hidden.get();
        for (i, n) in self.mesh_names.iter().enumerate().take(64) {
            if n.starts_with(prefix) {
                if on {
                    m &= !(1 << i);
                } else {
                    m |= 1 << i;
                }
            }
        }
        self.hidden.set(m);
    }

    pub fn bone_name(&self, i: usize) -> &str {
        self.names.get(i).map_or("?", String::as_str)
    }

    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }

    /// Bones (attachment points like `muzzle_flash`, `shell`) as drawn by `upload`: camera-relative position and
    /// world-oriented rotation, in the viewmodel's space.
    pub fn attachments(&self, pose: &[(Vec3, Quat)], view: Mat3A, sway: Affine3A, bones: &[usize]) -> Vec<(Vec3, glam::Mat3)> {
        let world = world_pose(&self.parents, pose);
        let cam = RIG_TO_SOURCE * Vec3A::from(world[self.camera_bone].translation);
        bones
            .iter()
            .map(|&b| {
                let w = world[b.min(world.len() - 1)];
                (Vec3::from(view * (sway.transform_point3a(RIG_TO_SOURCE * w.translation) - cam)), glam::Mat3::from(view * sway.matrix3 * RIG_TO_SOURCE * w.matrix3))
            })
            .collect()
    }

    pub fn has_translucent(&self) -> bool {
        self.translucent.iter().any(|&t| t)
    }

    /// Draws the opaque meshes, or (`translucent`) the alpha-blended ones.
    /// The material table (group 1) is bound by the caller for the whole pass (gfx.rs).
    pub fn draw(&self, rp: &mut wgpu::RenderPass<'_>, translucent: bool) {
        rp.set_bind_group(2, &self.group, &[]);
        rp.set_vertex_buffer(0, self.vbuf.slice(..));
        rp.set_vertex_buffer(1, self.sbuf.slice(..));
        rp.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        let hidden = self.hidden.get();
        for (i, m) in self.meshes.iter().enumerate() {
            if i < 64 && hidden & (1 << i) != 0 {
                continue;
            }
            if self.translucent.get(m.material).copied().unwrap_or(false) != translucent {
                continue;
            }
            rp.set_immediates(0, &(self.mat_base + m.material as u32).to_le_bytes());
            rp.draw_indexed(m.first..m.first + m.count, 0, 0..1);
        }
    }
}

/// Local (pos, rot) per bone -> model-space transforms (parents come before children).
fn world_pose(parents: &[i32], local: &[(Vec3, Quat)]) -> Vec<Affine3A> {
    let mut out: Vec<Affine3A> = Vec::with_capacity(local.len());
    for (i, &(p, q)) in local.iter().enumerate() {
        let l = Affine3A::from_rotation_translation(q, p);
        let w = match parents[i] {
            par if par >= 0 && (par as usize) < i => out[par as usize] * l,
            _ => l,
        };
        out.push(w);
    }
    out
}

/// Fires a sequence's events as its time advances: everything between the last call and now, or from the start
/// when the sequence changed or restarted.
#[derive(Default)]
pub struct EventClock {
    last: Option<(String, f32)>,
}

impl EventClock {
    pub fn advance(&mut self, vm: &Viewmodel, anim: &str, t: f32, out: &mut Vec<AnimEvent>) {
        match &self.last {
            Some((a, t0)) if a == anim && t >= *t0 => vm.events_between(anim, *t0, t, out),
            _ => vm.events_between(anim, -1.0, t, out),
        }
        self.last = Some((anim.to_string(), t));
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// Additive layer: `delta` (from an additive anim) on top of `base`, weighted. Root bones are skipped: the
/// sequences' "additive" root curve (jx_c_delta) carries the anim frame's constant -90 degree turn about Z, not a
/// delta, and adding it rolled the whole viewmodel onto its side (2026-11-18).
pub fn add_layer(vm: &Viewmodel, base: &mut [(Vec3, Quat)], delta: &[(Vec3, Quat)], w: f32) {
    for ((b, d), &parent) in base.iter_mut().zip(delta).zip(&vm.parents) {
        if parent < 0 {
            continue;
        }
        b.0 += d.0 * w;
        b.1 = (b.1 * Quat::IDENTITY.slerp(d.1, w)).normalize();
    }
}

/// Crossfade: `a` towards `b` by `t` (0 = a).
pub fn blend(a: &mut [(Vec3, Quat)], b: &[(Vec3, Quat)], t: f32) {
    for (x, y) in a.iter_mut().zip(b) {
        x.0 = x.0.lerp(y.0, t);
        x.1 = x.1.slerp(y.1, t);
    }
}
