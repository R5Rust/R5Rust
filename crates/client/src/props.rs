//! Static props with runtime LOD, like Apex: export-map writes every prop model once per LOD (model space, in
//! world.glb's `propLibrary` materials) and `props.json` (instances, the model's LOD switch points and fade distance).
//! Every frame each instance picks its LOD the way the S21 client does (r5apex_dx12 FUN_1403d5e20 / FUN_1403d6e80,
//! "Gather static props second pass"): LOD i while dist^2 * k < (r_lod_switch_scale * view * 13.8 * switch[i+1])^2,
//! i.e. a plain distance (not a screen metric: no resolution dependence), r_lod_switch_scale clamped to >= 0.6
//! (DAT_1417f82d4), 13.8 = DAT_1417f8980. `k` is a per-instance factor in the prop record, taken as 1 / scale^2 here;
//! `view` (job float 9) as 1 (likely the zoom/FOV factor: no ADS here yet). It fades out with a dither over the last
//! stretch before its fade distance (the instance's own, else the model's; x `fadeDistScale`). Visible instances
//! are drawn instanced: one draw per (variant, LOD, mesh) bucket, transforms in a per-frame instance buffer.

use glam::{Quat, Vec3, Vec4};
use std::collections::HashMap;

/// One instance as the GPU reads it (instance-rate vertex buffer): model -> world rows, probe, fade alpha.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstGpu {
    pub rows: [[f32; 4]; 3],
    /// packed light probe | cubemap << 24, as `_LIGHTING` was
    pub probe: u32,
    /// 1 = opaque; < 1 = dithered out (fade band)
    pub fade: f32,
    /// [0] = loot highlight slot (highlight.rs, 0 = none); [1] = ModelInstance.diffuseModulation (the entity's
    /// rendercolor) as RGBA8, r in the low byte, 0 = white
    pub pad: [u32; 2],
}

pub const INST_STRIDE: u64 = std::mem::size_of::<InstGpu>() as u64;

#[derive(Clone, Copy)]
pub struct PropDraw {
    pub first_index: u32,
    pub index_count: u32,
    pub material: u32,
    pub key: u8,
    pub pipe: u8,
    pub shadow: bool,
}

pub struct Lod {
    pub switch: f32,
    pub draws: Vec<PropDraw>,
}

pub struct Variant {
    pub lods: Vec<Lod>,
}

pub struct Instance {
    pub variant: u32,
    /// LOD distance is measured from the origin, divided by the prop's scale
    pub origin: Vec3,
    pub inv_scale: f32,
    /// world-space bounding sphere
    pub center: Vec3,
    pub radius: f32,
    /// distance where it starts fading at fadeDistScale 1 (f32::INFINITY = never), `fade_distance`
    pub fade: f32,
    /// squared distance from the origin past which it is too small to draw (S30 screen-size cull), `size_cull_d2`
    pub size_d2: f32,
    /// the model's set-dress tier (mdl header @0xF0): hidden while it is above mat_set_dress_level
    pub tier: u8,
    pub gpu: InstGpu,
}

pub struct Props {
    pub variants: Vec<Variant>,
    pub instances: Vec<Instance>,
    pub lib_tris: u64,
    /// model path -> its library variant (skin 0, casts shadows): what the game places at runtime (loot)
    pub model_variant: HashMap<String, u32>,
    /// variant -> (bounding-sphere centre, radius) in model space
    pub bounds: Vec<(Vec3, f32)>,
    /// instances the game sets every frame (loot on the ground), drawn with the static ones
    pub dynamic: Vec<Instance>,
    /// the highlight slots of this frame's dynamic instances (index = InstGpu.pad[0]; 0 = none), highlight.rs
    pub highlight: Vec<crate::highlight::Setting>,
}

/// Room in the instance buffer for `Props::dynamic`.
pub const DYNAMIC_CAP: usize = 4096;

impl Props {
    /// A runtime instance of a library variant: Source angles (pitch, yaw, roll) degrees, `probe` = packed lighting.
    pub fn instance(&self, variant: u32, pos: Vec3, angles: Vec3, probe: u32) -> Instance {
        let q = Quat::from_rotation_z(angles.y.to_radians()) * Quat::from_rotation_y(angles.x.to_radians()) * Quat::from_rotation_x(angles.z.to_radians());
        let m = glam::Mat3::from_quat(q);
        let rows = [
            [m.x_axis.x, m.y_axis.x, m.z_axis.x, pos.x],
            [m.x_axis.y, m.y_axis.y, m.z_axis.y, pos.y],
            [m.x_axis.z, m.y_axis.z, m.z_axis.z, pos.z],
        ];
        let (c, r) = self.bounds.get(variant as usize).copied().unwrap_or((Vec3::ZERO, 16.0));
        Instance { variant, origin: pos, inv_scale: 1.0, center: pos + m * c, radius: r, fade: f32::INFINITY, size_d2: f32::INFINITY, tier: 0, gpu: InstGpu { rows, probe, fade: 1.0, pad: [0; 2] } }
    }
}

/// How LODs are picked this frame.
#[derive(Clone, Copy)]
pub struct LodParams {
    pub cam: Vec3,
    /// r_lod_switch_scale (Model Detail), already clamped like the game's
    pub switch_scale: f32,
    /// r_lod: -1 = auto, else force this LOD (clamped to the model's last)
    pub force: i32,
    /// fadeDistScale
    pub fade_scale: f32,
    /// mat_set_dress_level: props whose model tier is above it are not drawn
    pub dress_level: u8,
}

/// One draw of the selection: a mesh range drawn for `inst_count` instances starting at `inst_first`.
#[derive(Clone, Copy)]
pub struct SelDraw {
    pub first_index: u32,
    pub index_count: u32,
    pub material: u32,
    pub key: u8,
    pub pipe: u8,
    pub fading: bool,
    pub inst_first: u32,
    pub inst_count: u32,
}

#[derive(Default)]
pub struct Selection {
    pub draws: Vec<SelDraw>,
    /// the draws whose instances include highlighted loot (the outline mask pass draws these again)
    pub hl_draws: Vec<SelDraw>,
    pub data: Vec<InstGpu>,
    pub tris: u64,
    pub instances: u32,
    keys: Vec<(u64, u32)>,
}

impl Selection {
    /// Draws for the main view first (opaque + alpha-tested), split from the late ones (blended, sky material).
    pub fn split(&self) -> usize {
        self.draws.iter().position(|d| d.key / 2 >= 2).unwrap_or(self.draws.len())
    }
}

/// Distance units per switch-point unit (DAT_1417f8980 in the S21 client).
pub const LOD_UNIT: f32 = 13.8;
/// r_lod_switch_scale's floor (DAT_1417f82d4): Model Detail Low.
pub const MIN_SWITCH_SCALE: f32 = 0.6;

// Static-prop fade + cull, reversed from R5F S30 (r5apex_dx12_s30): setup FUN_1403f7620, gather FUN_1403f9e60 (job
// params FUN_14040c800), the dithered band as in FUN_1408bcef0. Distances are from the prop origin.
/// model_fadeRangeFraction: the prop dithers out between its fade distance F and F * (1 + this)
const FADE_RANGE_FRACTION: f32 = 0.1;
/// model_defaultFadeDistScale / model_defaultFadeDistMin: fade of a model without its own (0.5 * hull diagonal * scale)
const DEFAULT_FADE_SCALE: f32 = 40.0;
const DEFAULT_FADE_MIN: f32 = 400.0;
/// the engine's "never fades" distance (STUDIOHDR_FLAGS_NO_FORCED_FADE models)
const NO_FADE: f32 = 227_023.36;
/// staticProp_gather_size_weight * staticProp_max_scaled_dist: drawn while dist^2 < this * world-AABB diagonal^2
const SIZE_CULL: f32 = 5.0 * 2500.0;
/// staticProp_no_fade_scalar: never-fading props get this much less size weight (drawn farther)
const NO_FADE_SIZE_SCALAR: f32 = 0.7;

/// A static prop's fade start: its own (record +0x24), else the model's (header @204) * scale, else from its hull.
fn fade_distance(own: f32, model_fade: f32, no_forced_fade: bool, hull_diag: f32, scale: f32) -> f32 {
    let f = if own > 0.0 {
        own
    } else if no_forced_fade {
        NO_FADE
    } else if model_fade > 0.0 {
        model_fade * scale
    } else {
        (0.5 * hull_diag * scale * DEFAULT_FADE_SCALE).max(DEFAULT_FADE_MIN)
    };
    f.max(100.0)
}

/// Squared distance past which the prop's world-space hull AABB is too small on screen.
fn size_cull_d2(world_aabb_diag: f32, fade: f32) -> f32 {
    let weight = if fade >= NO_FADE { NO_FADE_SIZE_SCALAR } else { 1.0 };
    SIZE_CULL * world_aabb_diag * world_aabb_diag / weight
}

/// Dither alpha at squared distance `d2` for fade start `f` (already * fadeDistScale): 1 inside F, falling in
/// squared distance to 0 at F * 1.1; below 1/255 the engine drops it.
fn fade_alpha(f: f32, d2: f32) -> f32 {
    let f2 = f * f;
    let e2 = f2 * (1.0 + FADE_RANGE_FRACTION) * (1.0 + FADE_RANGE_FRACTION);
    ((e2 - d2) / (e2 - f2)).clamp(0.0, 1.0)
}

impl Props {
    /// Reads props.json. `lib_first(material)` = where that library material's indices start in the index buffer.
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        path: &std::path::Path,
        lib_first: &HashMap<u32, u32>,
        material_key: impl Fn(u32) -> Option<(u8, u8, bool)>,
        probe_ok: impl Fn(u32) -> bool,
        nearest_probe: impl Fn(Vec3) -> u32,
    ) -> Result<Option<Props>, String> {
        let Ok(text) = std::fs::read(path) else { return Ok(None) };
        let j: serde_json::Value = serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let num = |v: &serde_json::Value| v.as_f64().unwrap_or(0.0) as f32;
        let mut variants = Vec::new();
        let mut vinfo: Vec<(Vec3, f32, f32, Option<[f32; 6]>, bool, u8)> = Vec::new();
        let mut model_variant = HashMap::new();
        let mut lib_tris = 0u64;
        for v in j["variants"].as_array().ok_or("props.json: no variants")? {
            let no_shadow = v["noShadow"].as_bool().unwrap_or(false);
            let mut lods = Vec::new();
            for l in v["lods"].as_array().cloned().unwrap_or_default() {
                let mut draws = Vec::new();
                for d in l["draws"].as_array().cloned().unwrap_or_default() {
                    let (m, f, c) = (d[0].as_u64().unwrap_or(0) as u32, d[1].as_u64().unwrap_or(0) as u32, d[2].as_u64().unwrap_or(0) as u32);
                    let (Some(&base), Some((key, pipe, casts))) = (lib_first.get(&m), material_key(m)) else { continue };
                    lib_tris += c as u64 / 3;
                    draws.push(PropDraw { first_index: base + f, index_count: c, material: m, key, pipe, shadow: casts && !no_shadow && key / 2 < 2 });
                }
                lods.push(Lod { switch: num(&l["switch"]), draws });
            }
            let hull = v["hull"].as_array().filter(|h| h.len() == 6).map(|h| std::array::from_fn(|i| num(&h[i])));
            let no_forced_fade = v["noForcedFade"].as_bool().unwrap_or(false);
            let tier = v["dressTier"].as_u64().unwrap_or(0).min(255) as u8;
            vinfo.push((Vec3::new(num(&v["center"][0]), num(&v["center"][1]), num(&v["center"][2])), num(&v["radius"]), num(&v["fade"]), hull, no_forced_fade, tier));
            if v["skin"].as_u64().unwrap_or(0) == 0 && !no_shadow {
                if let Some(m) = v["model"].as_str() {
                    model_variant.entry(m.to_ascii_lowercase()).or_insert(variants.len() as u32);
                }
            }
            variants.push(Variant { lods });
        }
        let mut instances = Vec::new();
        for r in j["instances"].as_array().ok_or("props.json: no instances")? {
            let Some(vi) = r[0].as_u64().map(|x| x as usize).filter(|&x| x < variants.len()) else { continue };
            let pos = Vec3::new(num(&r[1]), num(&r[2]), num(&r[3]));
            let q = Quat::from_xyzw(num(&r[4]), num(&r[5]), num(&r[6]), num(&r[7])).normalize();
            let s = num(&r[8]).max(1e-4);
            let m = glam::Mat3::from_quat(q) * s;
            let rows = [
                [m.x_axis.x, m.y_axis.x, m.z_axis.x, pos.x],
                [m.x_axis.y, m.y_axis.y, m.z_axis.y, pos.y],
                [m.x_axis.z, m.y_axis.z, m.z_axis.z, pos.z],
            ];
            let (vc, vr, vfade, hull, no_forced_fade, tier) = vinfo[vi];
            let center = pos + m * vc;
            // hull box (model header; older props.json: the mesh bounds' sphere) -> world AABB, like FUN_1403f7620
            let (lo, hi) = hull.map_or((vc - Vec3::splat(vr / 3f32.sqrt()), vc + Vec3::splat(vr / 3f32.sqrt())), |h| (Vec3::new(h[0], h[1], h[2]), Vec3::new(h[3], h[4], h[5])));
            let half = (hi - lo) * 0.5;
            let abs = glam::Mat3::from_cols(m.x_axis.abs(), m.y_axis.abs(), m.z_axis.abs());
            let world_diag = 2.0 * (abs * half).length();
            let fade = fade_distance(num(&r[10]), vfade, no_forced_fade, (hi - lo).length(), s);
            let size_d2 = size_cull_d2(world_diag, fade);
            let lit = r[9].as_i64().filter(|&l| l >= 0).map(|l| l as u32).filter(|&l| probe_ok(l));
            let probe = lit.unwrap_or_else(|| nearest_probe(center));
            instances.push(Instance { variant: vi as u32, origin: pos, inv_scale: 1.0 / s, center, radius: vr * s, fade, size_d2, tier, gpu: InstGpu { rows, probe, fade: 1.0, pad: [0; 2] } });
        }
        let bounds = vinfo.iter().map(|&(c, r, ..)| (c, r)).collect();
        Ok(Some(Props { variants, instances, lib_tris, model_variant, bounds, dynamic: Vec::new(), highlight: Vec::new() }))
    }

    /// Picks each visible instance's LOD and fade, groups them into instanced draws (sorted by pipeline then
    /// material) and fills `sel.data` in draw order. `visible(center, radius)` culls; `shadow` keeps only
    /// shadow-casting draws and never dithers.
    pub fn select(&self, sel: &mut Selection, p: &LodParams, shadow: bool, visible: impl Fn(Vec3, f32) -> bool) {
        sel.draws.clear();
        sel.hl_draws.clear();
        sel.data.clear();
        sel.keys.clear();
        sel.tris = 0;
        let n_static = self.instances.len();
        for (i, inst) in self.instances.iter().chain(self.dynamic.iter()).enumerate() {
            // S30 FUN_1403f89f0: a static prop whose model's set-dress tier is above mat_set_dress_level is never drawn
            if inst.tier > p.dress_level {
                continue;
            }
            // fadedistscale 0 = never fade or size-cull (r5rust's old default); retail runs fadeDistScale 1
            let d2 = (inst.origin - p.cam).length_squared();
            let alpha = if p.fade_scale <= 0.0 || !inst.fade.is_finite() {
                1.0
            } else if d2 >= inst.size_d2 {
                0.0
            } else {
                fade_alpha(inst.fade * p.fade_scale, d2)
            };
            if alpha < 1.0 / 255.0 || !visible(inst.center, inst.radius) {
                continue;
            }
            let v = &self.variants[inst.variant as usize];
            let last = v.lods.len().saturating_sub(1);
            let lod = if p.force >= 0 {
                (p.force as usize).min(last)
            } else {
                let dist = (inst.origin - p.cam).length() * inst.inv_scale;
                let unit = LOD_UNIT * p.switch_scale;
                v.lods.iter().rposition(|l| dist >= l.switch * unit).unwrap_or(0)
            };
            let fading = alpha < 1.0 && !shadow;
            sel.keys.push(((inst.variant as u64) << 8 | (lod as u64) << 1 | fading as u64, i as u32));
        }
        sel.keys.sort_unstable();
        sel.instances = sel.keys.len() as u32;
        let mut k = 0;
        while k < sel.keys.len() {
            let key = sel.keys[k].0;
            let start = sel.data.len() as u32;
            let mut highlighted = false;
            while k < sel.keys.len() && sel.keys[k].0 == key {
                let idx = sel.keys[k].1 as usize;
                let inst = if idx < n_static { &self.instances[idx] } else { &self.dynamic[idx - n_static] };
                let mut g = inst.gpu;
                if key & 1 == 1 {
                    g.fade = fade_alpha(inst.fade * p.fade_scale, (inst.origin - p.cam).length_squared());
                }
                highlighted |= g.pad[0] != 0;
                sel.data.push(g);
                k += 1;
            }
            let count = sel.data.len() as u32 - start;
            let lod = &self.variants[(key >> 8) as usize].lods[((key >> 1) & 0x7F) as usize];
            for d in &lod.draws {
                if shadow && !d.shadow {
                    continue;
                }
                sel.tris += d.index_count as u64 / 3 * count as u64;
                let draw = SelDraw {
                    first_index: d.first_index,
                    index_count: d.index_count,
                    material: d.material,
                    key: d.key,
                    pipe: d.pipe,
                    fading: key & 1 == 1,
                    inst_first: start,
                    inst_count: count,
                };
                if highlighted && !shadow {
                    sel.hl_draws.push(draw);
                }
                sel.draws.push(draw);
            }
        }
        // opaque, alpha-tested, blended, sky material; decals after their class; fewest state changes inside
        sel.draws.sort_by_key(|d| (d.key / 2, d.pipe >= crate::loader::DECAL_PIPES, d.fading, d.pipe, d.material));
    }
}

/// Frustum test for a sphere, camera-relative (the planes `gfx::frustum_planes` makes).
pub fn sphere_visible(planes: &[Vec4; 5], rel: Vec3, r: f32) -> bool {
    planes.iter().all(|pl| pl.truncate().dot(rel) + pl.w >= -r * pl.truncate().length())
}
