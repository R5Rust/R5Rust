//! Weapon effects: muzzle flash, shell ejection, tracers and impact sparks, built from retail Apex's particle textures
//! and shell model (tools/build_fx.py). Retail draws them through effect assets (wpn_muzzleflash_smg_FP,
//! wpn_shelleject_pistol_FP, P_tracer_projectile_smg) whose parameters RSX can't read, so the spawning and timing
//! here is tuned by eye; the textures, their colour ramps and the shell are retail's.
//!
//! Two spaces: `View` effects (flash, shells, muzzle smoke) are camera-relative and drawn with the viewmodel's
//! projection, like retail's first-person (_FP) effects; `World` effects (tracers, impacts) are in world units and
//! drawn with the world's projection, depth-tested against the map.

use glam::{Mat3, Mat4, Quat, Vec3};
use std::path::Path;

/// One sprite instance (fx.wgsl SIn): centre, half width; half-length axis (0 = billboard), rotation; colour; layer, aspect.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Sprite {
    pub pos_size: [f32; 4],
    pub axis_rot: [f32; 4],
    pub color: [f32; 4],
    pub layer: [f32; 4],
}

/// One shell instance: rows of its camera-relative model matrix.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShellInst {
    pub rows: [[f32; 4]; 3],
}

const MAX_SPRITES: usize = 4096;
const MAX_SHELLS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Space {
    View,
    World,
}

#[derive(Clone, Copy)]
struct Particle {
    space: Space,
    pos: Vec3,
    vel: Vec3,
    age: f32,
    life: f32,
    /// half width at birth / death
    size: (f32, f32),
    /// height / width for billboards
    aspect: f32,
    /// > 0: stretched along the velocity, half length = speed * stretch (+ `len`)
    stretch: f32,
    /// fixed half-length along `dir` (flame tongues), grows by `grow` over the life
    len: f32,
    grow: f32,
    dir: Vec3,
    rot: f32,
    spin: f32,
    color: [f32; 4],
    layer: u32,
    frames: u32,
    drag: f32,
    gravity: f32,
}

struct Shell {
    pos: Vec3,
    vel: Vec3,
    rot: Quat,
    spin: Vec3,
    age: f32,
}

/// Texture layers of layers.bin (layers.json).
#[derive(Clone, Copy, Default)]
struct Layers {
    core: (u32, u32),
    cloud: u32,
    round: u32,
    spark: u32,
    glow: u32,
    trail: u32,
    beam: u32,
    smoke: (u32, u32),
    shell: u32,
}

pub struct Fx {
    parts: Vec<Particle>,
    shells: Vec<Shell>,
    seed: u32,
    l: Layers,
    /// effect strength multiplier (console `fx_intensity`)
    pub intensity: f32,
}

/// Shell gravity (units/s^2, Apex's 750 scaled for a light casing) and how long one stays.
const SHELL_G: f32 = 600.0;
const SHELL_LIFE: f32 = 0.7;

impl Fx {
    fn rnd(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed >> 8) as f32 / 16_777_216.0
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.rnd()
    }
    fn unit(&mut self) -> Vec3 {
        let z = self.range(-1.0, 1.0);
        let t = self.range(0.0, std::f32::consts::TAU);
        let r = (1.0 - z * z).max(0.0).sqrt();
        Vec3::new(r * t.cos(), r * t.sin(), z)
    }

    fn base(space: Space, pos: Vec3, layer: u32, life: f32, size: (f32, f32), color: [f32; 4]) -> Particle {
        Particle {
            space,
            pos,
            vel: Vec3::ZERO,
            age: 0.0,
            life,
            size,
            aspect: 1.0,
            stretch: 0.0,
            len: 0.0,
            grow: 0.0,
            dir: Vec3::ZERO,
            rot: 0.0,
            spin: 0.0,
            color,
            layer,
            frames: 1,
            drag: 0.0,
            gravity: 0.0,
        }
    }

    fn push(&mut self, p: Particle) {
        if self.parts.len() < MAX_SPRITES {
            self.parts.push(p);
        }
    }

    /// One shot's muzzle flash at the viewmodel's `muzzle_flash` attachment (view space): two flame tongues along
    /// the barrel, the round flash, the cloud, a soft glow, a spark or two, and now and then a wisp of smoke.
    pub fn muzzle_flash(&mut self, at: Vec3, fwd: Vec3, up: Vec3) {
        let k = self.intensity;
        let l = self.l;
        // the barrel points almost straight away from the eye, so tongues laid along it would be edge-on: lay them
        // along the barrel's direction on screen (towards its vanishing point) instead
        let ray = at.normalize_or(fwd);
        let on_screen = (fwd - ray * fwd.dot(ray)).normalize_or(up);
        for n in 0..2 {
            let len = self.range(2.6, 3.6) * if n == 0 { 1.0 } else { 0.75 };
            let v = (self.rnd() * l.core.1 as f32) as u32 % l.core.1.max(1);
            let mut p = Self::base(Space::View, at + fwd * 0.4, l.core.0 + v, self.range(0.03, 0.045), (self.range(0.75, 0.95), 1.1), [24.0 * k, 21.0 * k, 11.0 * k, 0.0]);
            let wobble = ray.cross(on_screen).normalize_or_zero() * self.range(-0.25, 0.25);
            p.dir = (on_screen + wobble).normalize();
            p.len = len;
            p.grow = 0.3;
            self.push(p);
        }
        // petals: short tongues out to the sides, turned a random amount about the barrel
        let side0 = fwd.cross(up).normalize_or_zero();
        let up0 = side0.cross(fwd).normalize_or_zero();
        let turn = self.range(0.0, std::f32::consts::TAU);
        let petals = 3 + (self.rnd() * 2.0) as u32;
        for i in 0..petals {
            let a = turn + i as f32 * std::f32::consts::TAU / petals as f32;
            let d = (side0 * a.cos() + up0 * a.sin() + fwd * 0.5).normalize();
            let v = (self.rnd() * l.core.1 as f32) as u32 % l.core.1.max(1);
            let mut p = Self::base(Space::View, at + fwd * 0.3, l.core.0 + v, self.range(0.025, 0.04), (self.range(0.3, 0.42), 0.5), [18.0 * k, 15.5 * k, 8.0 * k, 0.0]);
            p.dir = d;
            p.len = self.range(0.9, 1.6);
            p.grow = 0.3;
            self.push(p);
        }
        let mut r = Self::base(Space::View, at + fwd * 0.6, l.round, self.range(0.028, 0.04), (self.range(0.9, 1.2), 1.4), [4.0 * k, 3.8 * k, 2.6 * k, 0.0]);
        r.rot = self.range(0.0, std::f32::consts::TAU);
        self.push(r);
        let mut c = Self::base(Space::View, at + fwd * 1.6, l.cloud, self.range(0.04, 0.06), (0.8, 1.4), [0.45 * k, 0.45 * k, 0.45 * k, 0.0]);
        c.rot = self.range(0.0, std::f32::consts::TAU);
        c.vel = fwd * 40.0;
        self.push(c);
        let g = Self::base(Space::View, at + fwd * 0.5, l.glow, 0.05, (2.0, 2.4), [0.5 * k, 0.32 * k, 0.18 * k, 0.0]);
        self.push(g);
        let sparks = (self.rnd() * 2.6) as u32;
        for _ in 0..sparks {
            let d = (fwd + self.unit() * 0.35).normalize();
            let mut s = Self::base(Space::View, at + fwd * 0.4, l.spark, self.range(0.05, 0.11), (0.07, 0.05), [9.0 * k, 9.0 * k, 9.0 * k, 0.0]);
            s.vel = d * self.range(220.0, 420.0);
            s.stretch = 0.012;
            s.drag = 4.0;
            s.gravity = 200.0;
            self.push(s);
        }
        if self.rnd() < 0.35 {
            let mut m = Self::base(Space::View, at + fwd * 1.0, l.smoke.0, self.range(0.5, 0.9), (0.8, 4.5), [0.05, 0.05, 0.05, 0.05]);
            m.frames = l.smoke.1;
            m.vel = fwd * self.range(12.0, 30.0) + up * self.range(4.0, 10.0);
            m.drag = 2.0;
            m.rot = self.range(0.0, std::f32::consts::TAU);
            m.spin = self.range(-1.0, 1.0);
            self.push(m);
        }
    }

    /// A shell out of the ejection port (view space), to the right and up, tumbling.
    pub fn shell(&mut self, at: Vec3, right: Vec3, up: Vec3, fwd: Vec3, rot: Mat3) {
        if self.shells.len() >= MAX_SHELLS {
            self.shells.remove(0);
        }
        let vel = right * self.range(70.0, 110.0) + up * self.range(55.0, 85.0) + fwd * self.range(-15.0, 10.0);
        let spin = self.unit() * self.range(15.0, 30.0);
        self.shells.push(Shell { pos: at, vel, rot: Quat::from_mat3(&rot), spin, age: 0.0 });
    }

    /// Bullet hit (world space): a few sparks off the surface and a puff of dust.
    pub fn impact(&mut self, pos: Vec3, normal: Vec3) {
        let k = self.intensity;
        let l = self.l;
        let n = 3 + (self.rnd() * 3.0) as u32;
        for _ in 0..n {
            let d = (normal + self.unit() * 0.8).normalize();
            let mut s = Self::base(Space::World, pos + normal * 0.5, l.spark, self.range(0.08, 0.22), (0.18, 0.12), [8.0 * k, 8.0 * k, 8.0 * k, 0.0]);
            s.vel = d * self.range(150.0, 420.0);
            s.stretch = 0.01;
            s.drag = 3.0;
            s.gravity = 750.0;
            self.push(s);
        }
        let f = Self::base(Space::World, pos + normal * 1.0, l.glow, 0.05, (6.0, 8.0), [1.5 * k, 0.9 * k, 0.5 * k, 0.0]);
        self.push(f);
        let mut d = Self::base(Space::World, pos + normal * 2.0, l.smoke.0, self.range(0.5, 0.8), (3.0, 12.0), [0.16, 0.15, 0.13, 0.22]);
        d.frames = l.smoke.1;
        d.vel = normal * self.range(20.0, 40.0);
        d.drag = 3.0;
        d.rot = self.range(0.0, std::f32::consts::TAU);
        d.spin = self.range(-1.5, 1.5);
        self.push(d);
    }

    pub fn tick(&mut self, dt: f32) {
        for p in &mut self.parts {
            p.age += dt;
            p.vel *= (1.0 - p.drag * dt).max(0.0);
            p.vel.z -= p.gravity * dt;
            p.pos += p.vel * dt;
            p.rot += p.spin * dt;
        }
        self.parts.retain(|p| p.age < p.life);
        for s in &mut self.shells {
            s.age += dt;
            s.vel.z -= SHELL_G * dt;
            s.pos += s.vel * dt;
            let w = s.spin * dt;
            s.rot = (Quat::from_scaled_axis(w) * s.rot).normalize();
        }
        self.shells.retain(|s| s.age < SHELL_LIFE);
    }

    pub fn clear(&mut self) {
        self.parts.clear();
        self.shells.clear();
    }

    /// Sprite instances for one space; `cam` (world) makes world positions camera-relative.
    pub fn sprites(&self, space: Space, cam: Vec3, out: &mut Vec<Sprite>) {
        for p in self.parts.iter().filter(|p| p.space == space) {
            let t = (p.age / p.life).clamp(0.0, 1.0);
            let size = p.size.0 + (p.size.1 - p.size.0) * t;
            // glow fades fast at the end, smoke fades over its whole life
            let fade = if p.color[3] > 0.0 { (1.0 - t) * (1.0 - t) } else { 1.0 - t * t };
            let c = p.color.map(|x| x * fade);
            let rel = if space == Space::World { p.pos - cam } else { p.pos };
            let (centre, axis) = if p.len > 0.0 {
                let half = p.len * (1.0 + p.grow * t) * 0.5;
                (rel + p.dir * half, p.dir * half)
            } else if p.stretch > 0.0 {
                // head first: the texture's bright end (top) on the particle, the streak trailing behind
                let a = p.vel * p.stretch * 0.5;
                (rel - a, -a)
            } else {
                (rel, Vec3::ZERO)
            };
            let layer = p.layer + ((t * p.frames as f32) as u32).min(p.frames.saturating_sub(1));
            out.push(Sprite {
                pos_size: [centre.x, centre.y, centre.z, size],
                axis_rot: [axis.x, axis.y, axis.z, p.rot],
                color: c,
                layer: [layer as f32, p.aspect, 0.0, 0.0],
            });
        }
    }

    /// A tracer for a bullet: a warm streak from `tail` to `head` plus a small glow on its head (world space).
    pub fn tracer(&self, tail: Vec3, head: Vec3, cam: Vec3, out: &mut Vec<Sprite>) {
        let k = self.intensity;
        let rel = head - cam;
        let dist = rel.length();
        // keep it at least ~1.5 px wide far away (1080p, ~100 deg)
        let w = (dist * 0.0014).max(0.3);
        let half = (head - tail) * 0.5;
        out.push(Sprite { pos_size: [rel.x - half.x, rel.y - half.y, rel.z - half.z, w], axis_rot: [half.x, half.y, half.z, 0.0], color: [5.0 * k, 3.4 * k, 1.9 * k, 0.0], layer: [self.l.beam as f32, 1.0, 0.0, 0.0] });
        out.push(Sprite { pos_size: [rel.x - half.x, rel.y - half.y, rel.z - half.z, w * 2.5], axis_rot: [half.x, half.y, half.z, 0.0], color: [0.9 * k, 0.5 * k, 0.25 * k, 0.0], layer: [self.l.trail as f32, 1.0, 0.0, 0.0] });
        out.push(Sprite { pos_size: [rel.x, rel.y, rel.z, w * 4.0], axis_rot: [0.0; 4], color: [1.2 * k, 0.8 * k, 0.45 * k, 0.0], layer: [self.l.glow as f32, 1.0, 0.0, 0.0] });
    }

    pub fn shell_instances(&self, out: &mut Vec<ShellInst>) {
        for s in &self.shells {
            let m = Mat3::from_quat(s.rot);
            out.push(ShellInst {
                rows: [
                    [m.x_axis.x, m.y_axis.x, m.z_axis.x, s.pos.x],
                    [m.x_axis.y, m.y_axis.y, m.z_axis.y, s.pos.y],
                    [m.x_axis.z, m.y_axis.z, m.z_axis.z, s.pos.z],
                ],
            });
        }
    }

    pub fn shell_layer(&self) -> u32 {
        self.l.shell
    }
}

/// This frame's effect draw data (set by main.rs, drawn by gfx.rs in the world pass).
#[derive(Default)]
pub struct FxFrame {
    pub world: Vec<Sprite>,
    pub view: Vec<Sprite>,
    pub shells: Vec<ShellInst>,
    pub world_clip: Mat4,
    pub view_clip: Mat4,
    pub right: Vec3,
    pub up: Vec3,
    pub to_sun: Vec3,
    pub sun: [f32; 3],
    pub sky: [f32; 3],
    pub shell_layer: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FxUniform {
    clip: [[f32; 4]; 4],
    right: [f32; 4],
    up: [f32; 4],
    to_sun: [f32; 4],
    sun: [f32; 4],
    sky: [f32; 4],
}

/// GPU side: the texture array, pipelines (rebuilt with the MSAA sample count), buffers.
pub struct FxGpu {
    layout: wgpu::BindGroupLayout,
    pl: wgpu::PipelineLayout,
    sh: wgpu::ShaderModule,
    pub samples: u32,
    sprite: wgpu::RenderPipeline,
    shell: wgpu::RenderPipeline,
    ub: [wgpu::Buffer; 2],
    bg: [wgpu::BindGroup; 2],
    inst: [wgpu::Buffer; 2],
    shell_vbuf: wgpu::Buffer,
    shell_ibuf: wgpu::Buffer,
    shell_n: u32,
    shell_inst: wgpu::Buffer,
}

impl FxGpu {
    /// Loads layers.bin / layers.json / shell.bin from `dir` and returns the GPU side and the simulation.
    pub fn load(device: &wgpu::Device, queue: &wgpu::Queue, dir: &Path, hdr: wgpu::TextureFormat, depth: wgpu::TextureFormat, samples: u32) -> Result<(FxGpu, Fx), String> {
        let raw = std::fs::read(dir.join("layers.bin")).map_err(|e| format!("{}: {e}", dir.join("layers.bin").display()))?;
        let rd = |o: usize| u32::from_le_bytes(raw[o..o + 4].try_into().unwrap());
        let (size, count) = (rd(0), rd(4));
        let per = (size * size * 4) as usize;
        if raw.len() < 8 + per * count as usize {
            return Err("layers.bin truncated".into());
        }
        let names: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("layers.json")).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let ly = |n: &str| (names[n]["layer"].as_u64().unwrap_or(0) as u32, names[n]["frames"].as_u64().unwrap_or(1) as u32);
        let l = Layers {
            core: ly("flash_core"),
            cloud: ly("flash_cloud").0,
            round: ly("flash_round").0,
            spark: ly("spark").0,
            glow: ly("glow").0,
            trail: ly("trail").0,
            beam: ly("beam").0,
            smoke: ly("smoke"),
            shell: ly("shell").0,
        };
        // the array with a full mip chain (box-filtered on the CPU: the sprites shrink a lot with distance)
        let mips = 32 - size.leading_zeros();
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fx layers"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: count },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for layer in 0..count {
            let mut img = raw[8 + per * layer as usize..8 + per * (layer as usize + 1)].to_vec();
            let mut s = size;
            for mip in 0..mips {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: mip, origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
                    &img,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * s), rows_per_image: Some(s) },
                    wgpu::Extent3d { width: s, height: s, depth_or_array_layers: 1 },
                );
                if s == 1 {
                    break;
                }
                let h = s / 2;
                let mut next = vec![0u8; (h * h * 4) as usize];
                for y in 0..h as usize {
                    for x in 0..h as usize {
                        for c in 0..4 {
                            let at = |xx: usize, yy: usize| img[(yy * s as usize + xx) * 4 + c] as u32;
                            next[(y * h as usize + x) * 4 + c] = ((at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1) + 2) / 4) as u8;
                        }
                    }
                }
                img = next;
                s = h;
            }
        }
        let view = tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let samp = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: vf, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let mk_ub = || device.create_buffer(&wgpu::BufferDescriptor { label: Some("fx uniform"), size: std::mem::size_of::<FxUniform>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let ub = [mk_ub(), mk_ub()];
        let mk_bg = |b: &wgpu::Buffer| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fx"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: b.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&samp) },
                ],
            })
        };
        let bg = [mk_bg(&ub[0]), mk_bg(&ub[1])];
        let mk_inst = |n: usize, stride: usize| device.create_buffer(&wgpu::BufferDescriptor { label: Some("fx instances"), size: (n * stride) as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let inst = [mk_inst(MAX_SPRITES, std::mem::size_of::<Sprite>()), mk_inst(MAX_SPRITES, std::mem::size_of::<Sprite>())];
        let shell_inst = mk_inst(MAX_SHELLS, std::mem::size_of::<ShellInst>());
        // the shell mesh
        let sb = std::fs::read(dir.join("shell.bin")).map_err(|e| format!("{}: {e}", dir.join("shell.bin").display()))?;
        let nv = u32::from_le_bytes(sb[0..4].try_into().unwrap()) as usize;
        let vbytes = &sb[4..4 + nv * 32];
        let ni = u32::from_le_bytes(sb[4 + nv * 32..8 + nv * 32].try_into().unwrap()) as usize;
        let ibytes = &sb[8 + nv * 32..8 + nv * 32 + ni * 4];
        use wgpu::util::DeviceExt;
        let shell_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("shell verts"), contents: vbytes, usage: wgpu::BufferUsages::VERTEX });
        let shell_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("shell idx"), contents: ibytes, usage: wgpu::BufferUsages::INDEX });
        let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("fx"), source: wgpu::ShaderSource::Wgsl(include_str!("fx.wgsl").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("fx"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let (sprite, shell) = Self::pipelines(device, &pl, &sh, hdr, depth, samples);
        let fx = Fx { parts: Vec::new(), shells: Vec::new(), seed: 0x9E37_79B9, l, intensity: 1.0 };
        log::info!("fx: {count} layers {size}px, shell {nv} verts {} tris", ni / 3);
        Ok((FxGpu { layout, pl, sh, samples, sprite, shell, ub, bg, inst, shell_vbuf, shell_ibuf, shell_n: ni as u32, shell_inst }, fx))
    }

    fn pipelines(device: &wgpu::Device, pl: &wgpu::PipelineLayout, sh: &wgpu::ShaderModule, hdr: wgpu::TextureFormat, depth: wgpu::TextureFormat, samples: u32) -> (wgpu::RenderPipeline, wgpu::RenderPipeline) {
        let ms = wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false };
        let premul = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        };
        let sattrs = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];
        let sprite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fx sprites"),
            layout: Some(pl),
            vertex: wgpu::VertexState {
                module: sh,
                entry_point: Some("vs_sprite"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Sprite>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &sattrs })],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState { format: depth, depth_write_enabled: Some(false), depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: ms,
            fragment: Some(wgpu::FragmentState { module: sh, entry_point: Some("fs_sprite"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: hdr, blend: Some(premul), write_mask: wgpu::ColorWrites::ALL })] }),
            multiview_mask: None,
            cache: None,
        });
        let vattrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let iattrs = wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4];
        let shell = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fx shells"),
            layout: Some(pl),
            vertex: wgpu::VertexState {
                module: sh,
                entry_point: Some("vs_shell"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout { array_stride: 32, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vattrs }),
                    Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<ShellInst>() as u64, step_mode: wgpu::VertexStepMode::Instance, attributes: &iattrs }),
                ],
            },
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: depth, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: ms,
            fragment: Some(wgpu::FragmentState { module: sh, entry_point: Some("fs_shell"), compilation_options: Default::default(), targets: &[Some(hdr.into())] }),
            multiview_mask: None,
            cache: None,
        });
        (sprite, shell)
    }

    /// The MSAA sample count changed (Video settings).
    pub fn rebuild(&mut self, device: &wgpu::Device, hdr: wgpu::TextureFormat, depth: wgpu::TextureFormat, samples: u32) {
        let (a, b) = Self::pipelines(device, &self.pl, &self.sh, hdr, depth, samples);
        self.sprite = a;
        self.shell = b;
        self.samples = samples;
        let _ = &self.layout;
    }

    fn uniform(f: &FxFrame, clip: Mat4, shell_layer: u32) -> FxUniform {
        FxUniform {
            clip: clip.to_cols_array_2d(),
            right: [f.right.x, f.right.y, f.right.z, 0.0],
            up: [f.up.x, f.up.y, f.up.z, shell_layer as f32],
            to_sun: [f.to_sun.x, f.to_sun.y, f.to_sun.z, 0.0],
            sun: [f.sun[0], f.sun[1], f.sun[2], 0.0],
            sky: [f.sky[0], f.sky[1], f.sky[2], 0.0],
        }
    }

    /// World-space effects (before the viewmodel, so the gun covers them).
    pub fn draw_world(&self, queue: &wgpu::Queue, rp: &mut wgpu::RenderPass<'_>, f: &FxFrame) {
        if f.world.is_empty() {
            return;
        }
        let n = f.world.len().min(MAX_SPRITES);
        queue.write_buffer(&self.ub[0], 0, bytemuck::bytes_of(&Self::uniform(f, f.world_clip, f.shell_layer)));
        queue.write_buffer(&self.inst[0], 0, bytemuck::cast_slice(&f.world[..n]));
        rp.set_pipeline(&self.sprite);
        rp.set_bind_group(0, &self.bg[0], &[]);
        rp.set_vertex_buffer(0, self.inst[0].slice(..));
        rp.draw(0..6, 0..n as u32);
    }

    /// First-person effects, in the viewmodel's depth range (set by the caller): shells, then the flash.
    pub fn draw_view(&self, queue: &wgpu::Queue, rp: &mut wgpu::RenderPass<'_>, f: &FxFrame) {
        if f.view.is_empty() && f.shells.is_empty() {
            return;
        }
        queue.write_buffer(&self.ub[1], 0, bytemuck::bytes_of(&Self::uniform(f, f.view_clip, f.shell_layer)));
        rp.set_bind_group(0, &self.bg[1], &[]);
        if !f.shells.is_empty() {
            let n = f.shells.len().min(MAX_SHELLS);
            queue.write_buffer(&self.shell_inst, 0, bytemuck::cast_slice(&f.shells[..n]));
            rp.set_pipeline(&self.shell);
            rp.set_vertex_buffer(0, self.shell_vbuf.slice(..));
            rp.set_vertex_buffer(1, self.shell_inst.slice(..));
            rp.set_index_buffer(self.shell_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..self.shell_n, 0, 0..n as u32);
        }
        if !f.view.is_empty() {
            let n = f.view.len().min(MAX_SPRITES);
            queue.write_buffer(&self.inst[1], 0, bytemuck::cast_slice(&f.view[..n]));
            rp.set_pipeline(&self.sprite);
            rp.set_vertex_buffer(0, self.inst[1].slice(..));
            rp.draw(0..6, 0..n as u32);
        }
    }
}
